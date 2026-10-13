//! Trace export (plan §12, OBS-2): the diagnostics records of OBS-1 become OpenTelemetry
//! spans posted as OTLP/HTTP JSON to a collector the person configures (`export` in
//! settings). Langfuse is reached through its OTLP endpoint; the spans carry its observation
//! attributes, so no second mapping is needed.
//!
//! The spool is the diagnostics table itself: the exporter keeps a durable cursor per
//! session (`<data dir>/export/state.json`) and only ever posts records newer than the
//! moment export was enabled, so a restart resends nothing and enabling export never pushes
//! history. One batch is in flight at a time, bounded at [`BATCH`] records; a failed post is
//! retried with exponential backoff up to [`MAX_ATTEMPTS`], then the batch is dropped and
//! counted. Records that age out of the spool before being sent are counted as dropped too.
//! Nothing here is on the path of a model or tool operation: the runtime writes its records
//! and pokes the exporter; the exporter reads, builds and posts on its own task, and a
//! collector that is slow, down or rejecting only changes the health view.
//!
//! Content level: `metadata` only (ids, timings, counts, outcomes, bounded error texts). No
//! prompt, tool body, credential, environment or source text is ever in a payload.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pipkin_engine_protocol::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Notify;

use super::Shared;
use crate::config::{ExportSettings, Settings};
use crate::store::diagnostics::DiagRow;
use crate::util::now_ms;

/// Most records in one post.
pub const BATCH: u32 = 200;
/// Attempts per batch before it is dropped.
pub const MAX_ATTEMPTS: u32 = 5;
pub const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// How often the exporter looks for new records without being poked.
pub const TICK: Duration = Duration::from_secs(2);
const SEND_TIMEOUT: Duration = Duration::from_secs(10);
const ERROR_CHARS: usize = 200;
pub const CONTENT_METADATA: &str = "metadata";

/// Durable exporter state.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct State {
    since_ms: Option<i64>,
    /// Last exported record per session.
    cursors: BTreeMap<String, u64>,
    sent: u64,
    dropped: u64,
}

pub struct ExportRuntime {
    /// Poked after a diagnostics record is written.
    pub notify: Notify,
    health: Mutex<ExportHealthView>,
    state: Mutex<State>,
    done: Arc<Notify>,
}

impl Default for ExportRuntime {
    fn default() -> Self {
        ExportRuntime {
            notify: Notify::new(),
            health: Mutex::new(ExportHealthView {
                state: "off".into(),
                content: CONTENT_METADATA.into(),
                ..ExportHealthView::default()
            }),
            state: Mutex::new(State::default()),
            done: Arc::new(Notify::new()),
        }
    }
}

impl ExportRuntime {
    pub fn health(&self) -> ExportHealthView {
        self.health
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Notified when the exporter task has stopped (shutdown).
    pub fn done(&self) -> Arc<Notify> {
        self.done.clone()
    }
}

/// What a settings value means for export; `None` when export is off or unusable.
pub struct Target {
    pub endpoint: String,
    pub headers: Vec<(String, String)>,
    pub service_name: String,
    pub retry_base: Duration,
}

pub fn target(settings: &ExportSettings) -> Result<Option<Target>, String> {
    if !settings.enabled {
        return Ok(None);
    }
    if let Some(kind) = settings.kind.as_deref().filter(|k| !k.is_empty())
        && kind != "otlp"
    {
        return Err(format!("export kind {kind:?} is not supported (only otlp)"));
    }
    if let Some(content) = settings.content.as_deref().filter(|c| !c.is_empty())
        && content != CONTENT_METADATA
    {
        return Err(format!(
            "export content {content:?} is not supported yet (only metadata)"
        ));
    }
    let Some(endpoint) = settings
        .endpoint
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
    else {
        return Err("export.endpoint is not set".into());
    };
    if !(endpoint.starts_with("http://") || endpoint.starts_with("https://")) {
        return Err("export.endpoint must be an http(s) URL".into());
    }
    let headers = settings
        .headers
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
        .collect();
    Ok(Some(Target {
        endpoint: endpoint.to_owned(),
        headers,
        service_name: settings
            .service_name
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "pipkin-engine".into()),
        retry_base: Duration::from_millis(settings.retry_base_ms.unwrap_or(1000).max(1)),
    }))
}

/// The host of an endpoint, for the health view (never the path or query).
pub fn endpoint_host(endpoint: &str) -> String {
    let rest = endpoint
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    rest.split('/').next().unwrap_or(rest).to_owned()
}

fn bounded(text: &str) -> String {
    let mut out: String = text.chars().take(ERROR_CHARS).collect();
    if text.chars().count() > ERROR_CHARS {
        out.push('…');
    }
    out
}

// ------------------------------------------------------------------------------------ spans

fn hex_id(parts: &[&str], len: usize) -> String {
    let joined = parts.join("\u{1f}");
    crate::web::sha256_hex(joined.as_bytes())[..len].to_owned()
}

pub fn trace_id(root: &str) -> String {
    hex_id(&["trace", root], 32)
}

pub fn run_span_id(session: &str, run: u64) -> String {
    hex_id(&["run", session, &run.to_string()], 16)
}

pub fn tool_span_id(session: &str, call_id: &str) -> String {
    hex_id(&["tool", session, call_id], 16)
}

fn attr(key: &str, value: Value) -> Option<Value> {
    let v = match value {
        Value::Null => return None,
        Value::String(s) => json!({"stringValue": s}),
        Value::Bool(b) => json!({"boolValue": b}),
        Value::Number(n) if n.is_i64() || n.is_u64() => json!({"intValue": n.to_string()}),
        Value::Number(n) => json!({"doubleValue": n}),
        other => json!({"stringValue": other.to_string()}),
    };
    Some(json!({"key": key, "value": v}))
}

fn nanos(ms: i64) -> String {
    (ms.max(0) as u128 * 1_000_000).to_string()
}

/// What one record contributes to its span.
struct SpanParts {
    name: String,
    span_id: String,
    parent: Option<String>,
    start: i64,
    kind: u8,
    more: Vec<Option<Value>>,
    status: Option<Value>,
}

/// The spans one record becomes (zero or one). `root` is the session's trace root.
pub fn span_of(row: &DiagRow, root: &str) -> Option<Value> {
    let d = &row.record;
    let s = |k: &str| d.get(k).cloned().unwrap_or(Value::Null);
    let num = |k: &str| d.get(k).and_then(Value::as_i64);
    let run = row.run?;
    let run_span = run_span_id(&row.session, run);
    let end = row.at_ms;
    let mut attrs: Vec<Value> = vec![
        attr("pipkin.session", json!(row.session)),
        attr("pipkin.run", json!(run)),
        attr("pipkin.seq", json!(row.seq)),
        attr(
            "pipkin.request",
            s("request")
                .is_null()
                .then(|| row.request.clone())
                .flatten()
                .map_or(Value::Null, Value::String),
        ),
    ]
    .into_iter()
    .flatten()
    .collect();
    let parts: SpanParts = match row.kind.as_str() {
        "run.end" => {
            let parent = match (
                d.get("parent").and_then(Value::as_str),
                d.get("parentCall").and_then(Value::as_str),
            ) {
                (Some(p), Some(c)) => Some(tool_span_id(p, c)),
                _ => None,
            };
            let outcome = d.get("outcome").and_then(Value::as_str).unwrap_or("");
            let status = (outcome != "completed").then(|| {
                json!({"code": 2, "message": format!("{outcome}{}", d.get("detail").and_then(Value::as_str).map(|t| format!(": {t}")).unwrap_or_default())})
            });
            SpanParts {
                name: "run".into(),
                span_id: run_span,
                parent,
                start: end - num("durationMs").unwrap_or(0),
                kind: 1,
                more: vec![
                    attr("pipkin.outcome", s("outcome")),
                    attr("pipkin.queue_wait_ms", s("queueWaitMs")),
                    attr("pipkin.stop_requested", s("stopRequested")),
                    attr("pipkin.stop_latency_ms", s("stopLatencyMs")),
                    attr("pipkin.parent_session", s("parent")),
                    attr("pipkin.parent_call", s("parentCall")),
                    attr("langfuse.observation.type", json!("agent")),
                ],
                status,
            }
        }
        "provider.request" => {
            let outcome = d.get("outcome").and_then(Value::as_str).unwrap_or("");
            let status = (outcome == "error").then(|| {
                json!({"code": 2, "message": d.get("error").and_then(Value::as_str).unwrap_or("error")})
            });
            let tokens = d.get("tokens").cloned().unwrap_or(Value::Null);
            let tok = |k: &str| tokens.get(k).cloned().unwrap_or(Value::Null);
            SpanParts {
                name: "generation".into(),
                span_id: hex_id(&["gen", &row.session, &row.seq.to_string()], 16),
                parent: Some(run_span),
                start: end - num("durationMs").unwrap_or(0),
                kind: 3,
                more: vec![
                    attr("gen_ai.request.model", s("model")),
                    attr("gen_ai.usage.input_tokens", tok("input")),
                    attr("gen_ai.usage.output_tokens", tok("output")),
                    attr("pipkin.cache_read_tokens", tok("cacheRead")),
                    attr("pipkin.cache_write_tokens", tok("cacheWrite")),
                    attr("pipkin.attempt", s("attempt")),
                    attr("pipkin.first_token_ms", s("firstTokenMs")),
                    attr("pipkin.outcome", s("outcome")),
                    attr("langfuse.observation.type", json!("generation")),
                ],
                status,
            }
        }
        "tool.call" => {
            let call_id = row.call_id.clone().unwrap_or_default();
            let tool = d.get("name").and_then(Value::as_str).unwrap_or("tool");
            let outcome = d.get("outcome").and_then(Value::as_str).unwrap_or("");
            let status = (outcome != "ok").then(|| json!({"code": 2, "message": outcome}));
            SpanParts {
                name: format!("tool {tool}"),
                span_id: tool_span_id(&row.session, &call_id),
                parent: Some(run_span),
                start: end - num("durationMs").unwrap_or(0),
                kind: 1,
                more: vec![
                    attr("pipkin.tool", s("name")),
                    attr("pipkin.call_id", json!(call_id)),
                    attr("pipkin.outcome", s("outcome")),
                    attr("pipkin.retained_bytes", s("retainedBytes")),
                    attr("pipkin.dropped_bytes", s("droppedBytes")),
                    attr("langfuse.observation.type", json!("tool")),
                ],
                status,
            }
        }
        "provider.retry" => SpanParts {
            name: "retry".into(),
            span_id: hex_id(&["retry", &row.session, &row.seq.to_string()], 16),
            parent: Some(run_span),
            start: end,
            kind: 1,
            more: vec![
                attr("pipkin.attempt", s("attempt")),
                attr("pipkin.delay_ms", s("delayMs")),
                attr("langfuse.observation.type", json!("event")),
            ],
            status: None,
        },
        "run.resumed" => SpanParts {
            name: "resumed".into(),
            span_id: hex_id(&["resumed", &row.session, &row.seq.to_string()], 16),
            parent: Some(run_span),
            start: end,
            kind: 1,
            more: vec![
                attr("pipkin.reason", s("reason")),
                attr("langfuse.observation.type", json!("event")),
            ],
            status: None,
        },
        _ => return None,
    };
    let SpanParts {
        name,
        span_id,
        parent,
        start,
        kind,
        more,
        status,
    } = parts;
    attrs.extend(more.into_iter().flatten());
    let mut span = json!({
        "traceId": trace_id(root),
        "spanId": span_id,
        "name": name,
        "kind": kind,
        "startTimeUnixNano": nanos(start),
        "endTimeUnixNano": nanos(end),
        "attributes": attrs,
    });
    if let Some(parent) = parent {
        span["parentSpanId"] = json!(parent);
    }
    if let Some(status) = status {
        span["status"] = status;
    }
    Some(span)
}

/// The OTLP/HTTP JSON body for a batch.
pub fn payload(spans: Vec<Value>, service_name: &str) -> Value {
    json!({
        "resourceSpans": [{
            "resource": {"attributes": [
                {"key": "service.name", "value": {"stringValue": service_name}},
                {"key": "service.version", "value": {"stringValue": env!("CARGO_PKG_VERSION")}},
            ]},
            "scopeSpans": [{
                "scope": {"name": "pipkin-engine", "version": env!("CARGO_PKG_VERSION")},
                "spans": spans,
            }],
        }]
    })
}

// ---------------------------------------------------------------------------------- runtime

struct Batch {
    /// `(session, last seq)` to advance on success.
    cursors: Vec<(String, u64)>,
    spans: Vec<Value>,
    records: u64,
}

impl Shared {
    fn export_state_path(&self) -> PathBuf {
        self.config.data_dir.join("export").join("state.json")
    }

    fn export_load_state(&self) {
        if let Ok(text) = std::fs::read_to_string(self.export_state_path())
            && let Ok(state) = serde_json::from_str::<State>(&text)
        {
            *self.export.state.lock().unwrap_or_else(|e| e.into_inner()) = state;
        }
    }

    fn export_save_state(&self) {
        let path = self.export_state_path();
        let state = self
            .export
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string(&state) {
            let _ = std::fs::write(&path, text);
        }
    }

    fn export_health(&self, edit: impl FnOnce(&mut ExportHealthView)) {
        let changed = {
            let mut health = self.export.health.lock().unwrap_or_else(|e| e.into_inner());
            let before = health.clone();
            edit(&mut health);
            let state = self.export.state.lock().unwrap_or_else(|e| e.into_inner());
            health.sent = state.sent;
            health.dropped = state.dropped;
            health.since_ms = state.since_ms;
            (*health != before).then(|| health.clone())
        };
        if let Some(export) = changed {
            self.publish(Event::ExportChanged { export });
        }
    }

    /// Records past the cursors (newest first per session is irrelevant: oldest first), the
    /// spans they become, and how many were aged out of the spool before being sent.
    fn export_collect(&self) -> crate::store::Result<(Batch, u64, u64)> {
        let state = self
            .export
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let since = state.since_ms.unwrap_or(i64::MAX);
        self.store.read(|c| {
            let mut stmt = c.prepare("SELECT DISTINCT session FROM diagnostics")?;
            let sessions: Vec<String> = stmt
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut batch = Batch {
                cursors: Vec::new(),
                spans: Vec::new(),
                records: 0,
            };
            let mut queued = 0u64;
            let mut aged = 0u64;
            let mut room = BATCH as i64;
            for session in sessions {
                let cursor = state.cursors.get(&session).copied().unwrap_or(0);
                // Only from the moment export was enabled.
                let oldest: Option<i64> = c
                    .query_row(
                        "SELECT MIN(seq) FROM diagnostics WHERE session = ?1 AND at >= ?2",
                        rusqlite::params![session, since],
                        |r| r.get(0),
                    )
                    .ok()
                    .flatten();
                if let Some(oldest) = oldest
                    && cursor > 0
                    && oldest as u64 > cursor + 1
                {
                    aged += oldest as u64 - cursor - 1;
                }
                let pending: i64 = c.query_row(
                    "SELECT COUNT(*) FROM diagnostics WHERE session = ?1 AND seq > ?2 AND at >= ?3",
                    rusqlite::params![session, cursor as i64, since],
                    |r| r.get(0),
                )?;
                queued += pending.max(0) as u64;
                if room <= 0 || pending == 0 {
                    continue;
                }
                let root: Option<String> = c
                    .query_row(
                        "SELECT COALESCE(root, id) FROM sessions WHERE id = ?1",
                        [&session],
                        |r| r.get(0),
                    )
                    .ok();
                let root = root.unwrap_or_else(|| session.clone());
                let mut stmt = c.prepare_cached(
                    "SELECT * FROM diagnostics WHERE session = ?1 AND seq > ?2 AND at >= ?3
                     ORDER BY seq LIMIT ?4",
                )?;
                let rows = stmt
                    .query_map(
                        rusqlite::params![session, cursor as i64, since, room],
                        crate::store::diagnostics::row,
                    )?
                    .collect::<rusqlite::Result<Vec<DiagRow>>>()?;
                let Some(last) = rows.last() else { continue };
                room -= rows.len() as i64;
                batch.records += rows.len() as u64;
                batch.cursors.push((session.clone(), last.seq));
                batch
                    .spans
                    .extend(rows.iter().filter_map(|r| span_of(r, &root)));
            }
            Ok((batch, queued, aged))
        })
    }

    async fn export_post(&self, target: &Target, body: &Value) -> Result<(), String> {
        let mut req = self
            .http()
            .post(&target.endpoint)
            .timeout(SEND_TIMEOUT)
            .header("content-type", "application/json");
        for (k, v) in &target.headers {
            req = req.header(k, v);
        }
        let response = req
            .json(body)
            .send()
            .await
            .map_err(|e| bounded(&format!("{e}")))?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(bounded(&format!("the collector answered {status}")))
        }
    }

    /// The exporter's loop. Runs for the engine's lifetime; stops at shutdown.
    async fn export_run(self: Arc<Self>) {
        self.export_load_state();
        let mut attempts: u32 = 0;
        let mut backoff_until: Option<tokio::time::Instant> = None;
        loop {
            if self.stopping.is_cancelled() {
                break;
            }
            let settings = Settings::load(&self.config.config_dir, None).0.export;
            let target = match target(&settings) {
                Ok(Some(t)) => t,
                Ok(None) | Err(_) => {
                    let error = target(&settings).err();
                    let was_on = self
                        .export
                        .state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .since_ms
                        .is_some();
                    if was_on {
                        self.export
                            .state
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .since_ms = None;
                        self.export_save_state();
                    }
                    attempts = 0;
                    backoff_until = None;
                    self.export_health(|h| {
                        h.enabled = settings.enabled;
                        h.state = if settings.enabled { "failed" } else { "off" }.into();
                        h.endpoint = None;
                        h.queued = 0;
                        h.failures = 0;
                        h.last_error = error.clone();
                    });
                    self.export_wait(None).await;
                    continue;
                }
            };
            {
                let mut state = self.export.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.since_ms.is_none() {
                    state.since_ms = Some(now_ms());
                    drop(state);
                    self.export_save_state();
                }
            }
            if let Some(until) = backoff_until {
                if tokio::time::Instant::now() < until {
                    self.export_wait(Some(until)).await;
                    continue;
                }
                backoff_until = None;
            }
            let (batch, queued, aged) = match self.export_collect() {
                Ok(c) => c,
                Err(error) => {
                    self.export_health(|h| {
                        h.enabled = true;
                        h.endpoint = Some(endpoint_host(&target.endpoint));
                        h.state = "failed".into();
                        h.last_error = Some(bounded(&error.to_string()));
                    });
                    self.export_wait(None).await;
                    continue;
                }
            };
            if aged > 0 {
                let mut state = self.export.state.lock().unwrap_or_else(|e| e.into_inner());
                state.dropped += aged;
                for (session, _) in &batch.cursors {
                    state.cursors.remove(session);
                }
                drop(state);
                self.export_save_state();
            }
            let host = endpoint_host(&target.endpoint);
            if batch.records == 0 {
                self.export_health(|h| {
                    h.enabled = true;
                    h.endpoint = Some(host.clone());
                    h.queued = queued;
                    if h.state != "failed" {
                        h.state = "idle".into();
                    }
                });
                self.export_wait(None).await;
                continue;
            }
            self.export_health(|h| {
                h.enabled = true;
                h.endpoint = Some(host.clone());
                h.queued = queued;
                h.state = "sending".into();
                h.failures = attempts;
            });
            let body = payload(batch.spans.clone(), &target.service_name);
            let sent = tokio::select! {
                r = self.export_post(&target, &body) => r,
                _ = self.stopping.cancelled() => break,
            };
            match sent {
                Ok(()) => {
                    {
                        let mut state = self.export.state.lock().unwrap_or_else(|e| e.into_inner());
                        for (session, seq) in &batch.cursors {
                            state.cursors.insert(session.clone(), *seq);
                        }
                        state.sent += batch.records;
                    }
                    self.export_save_state();
                    attempts = 0;
                    self.export_health(|h| {
                        h.state = "idle".into();
                        h.failures = 0;
                        h.last_error = None;
                        h.last_success_ms = Some(now_ms());
                        h.queued = queued.saturating_sub(batch.records);
                    });
                }
                Err(error) => {
                    attempts += 1;
                    if attempts >= MAX_ATTEMPTS {
                        {
                            let mut state =
                                self.export.state.lock().unwrap_or_else(|e| e.into_inner());
                            for (session, seq) in &batch.cursors {
                                state.cursors.insert(session.clone(), *seq);
                            }
                            state.dropped += batch.records;
                        }
                        self.export_save_state();
                        attempts = 0;
                        self.export_health(|h| {
                            h.state = "failed".into();
                            h.failures = 0;
                            h.last_error = Some(error);
                            h.queued = queued.saturating_sub(batch.records);
                        });
                    } else {
                        let delay =
                            (target.retry_base * (1u32 << (attempts - 1).min(6))).min(MAX_BACKOFF);
                        backoff_until = Some(tokio::time::Instant::now() + delay);
                        self.export_health(|h| {
                            h.state = "backoff".into();
                            h.failures = attempts;
                            h.last_error = Some(error);
                        });
                    }
                }
            }
        }
        self.export.done.notify_waiters();
        self.export.done.notify_one();
    }

    async fn export_wait(&self, until: Option<tokio::time::Instant>) {
        let tick = until.unwrap_or_else(|| tokio::time::Instant::now() + TICK);
        tokio::select! {
            _ = self.export.notify.notified() => {}
            _ = tokio::time::sleep_until(tick) => {}
            _ = self.stopping.cancelled() => {}
        }
    }
}

pub(super) fn start(shared: &Arc<Shared>) {
    let shared = shared.clone();
    let runtime = shared.clone();
    runtime.spawn(async move { shared.export_run().await });
}
