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
//! Content levels (OBS-3): `metadata` (ids, timings, counts, outcomes, bounded error texts)
//! is the default once export is enabled; `content` adds the run's input and output, each
//! generation's completion and each tool call's arguments and output, bounded and passed
//! through a redactor that strips the stored collector credential, header values from the
//! settings file and common token shapes. Images and web source bodies are never exported.
//! The collector credential lives in the engine's private credential file
//! (`export-auth.json`, 0600) through the existing credential store, never in settings; a
//! preview shows exactly what a payload would carry at a level before anything is sent.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pipkin_engine_protocol::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Notify;

use super::Shared;
use crate::auth::store::{CredentialStore, FileStore, StoredCredential};
use crate::config::{ExportSettings, Settings};
use crate::store::Store;
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
pub const CONTENT_CONTENT: &str = "content";
/// Longest text one content-level attribute carries (characters).
pub const CONTENT_TEXT_CHARS: usize = 4_096;
/// Longest preview returned (bytes).
pub const PREVIEW_BYTES: usize = 64 * 1024;
/// The credential's id in the private credential file.
const CREDENTIAL_ID: &str = "export";
const REDACTED: &str = "[redacted]";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentLevel {
    Metadata,
    Content,
}

impl ContentLevel {
    pub fn parse(text: &str) -> Option<ContentLevel> {
        match text.trim() {
            "" | CONTENT_METADATA => Some(ContentLevel::Metadata),
            CONTENT_CONTENT => Some(ContentLevel::Content),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ContentLevel::Metadata => CONTENT_METADATA,
            ContentLevel::Content => CONTENT_CONTENT,
        }
    }
}

/// Strips secrets from exported text: the values it was given (the stored credential,
/// header values from the settings file) and common token shapes.
pub struct Redactor {
    secrets: Vec<String>,
    patterns: Vec<regex::Regex>,
}

impl Redactor {
    pub fn new(secrets: Vec<String>) -> Redactor {
        let patterns = [
            r"(?i)bearer\s+[A-Za-z0-9._~+/=-]{8,}",
            r"(?i)basic\s+[A-Za-z0-9+/=]{8,}",
            r"\bsk-[A-Za-z0-9_-]{8,}",
            r"(?i)\b(api[_-]?key|secret|token|password)\b\s*[:=]\s*\S{6,}",
        ]
        .iter()
        .filter_map(|p| regex::Regex::new(p).ok())
        .collect();
        Redactor {
            secrets: secrets.into_iter().filter(|s| s.len() >= 6).collect(),
            patterns,
        }
    }

    pub fn redact(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for secret in &self.secrets {
            if out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), REDACTED);
            }
        }
        for pattern in &self.patterns {
            out = pattern.replace_all(&out, REDACTED).into_owned();
        }
        out
    }
}

fn clip(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

/// Replace every string attribute value and status message through the redactor.
pub fn redact_span(span: &mut Value, redactor: &Redactor) {
    if let Some(attrs) = span.get_mut("attributes").and_then(Value::as_array_mut) {
        for a in attrs {
            if let Some(Value::String(text)) =
                a.get_mut("value").and_then(|v| v.get_mut("stringValue"))
            {
                *text = redactor.redact(text);
            }
        }
    }
    if let Some(Value::String(message)) = span.get_mut("status").and_then(|s| s.get_mut("message"))
    {
        *message = redactor.redact(message);
    }
}

fn push_attr(span: &mut Value, key: &str, text: String) {
    if let Some(a) = attr(key, Value::String(text))
        && let Some(attrs) = span.get_mut("attributes").and_then(Value::as_array_mut)
    {
        attrs.push(a);
    }
}

/// Content level: the texts a span's fact refers to, read from the store on the same
/// connection, bounded and redacted.
fn enrich(c: &rusqlite::Connection, span: &mut Value, row: &DiagRow, redactor: &Redactor) {
    let Some(run) = row.run else { return };
    let text_of = |text: &str| clip(&redactor.redact(text), CONTENT_TEXT_CHARS);
    match row.kind.as_str() {
        "run.end" => {
            let Ok(entries) = Store::entries_of_run(c, &row.session, run) else {
                return;
            };
            let input = entries.iter().find_map(|e| match &e.kind {
                EntryKind::Message {
                    message: m @ Message::User { .. },
                    ..
                } => Some(m.text()),
                _ => None,
            });
            let output = entries.iter().rev().find_map(|e| match &e.kind {
                EntryKind::Message {
                    message: m @ Message::Assistant { .. },
                    partial: false,
                    ..
                } => Some(m.text()),
                _ => None,
            });
            if let Some(input) = input {
                push_attr(span, "pipkin.input", text_of(&input));
            }
            if let Some(output) = output {
                push_attr(span, "pipkin.output", text_of(&output));
            }
        }
        "provider.request" => {
            if let Some(id) = row.record.get("entry").and_then(Value::as_u64)
                && let Ok(Some(entry)) = Store::entry_in(c, &row.session, id)
                && let EntryKind::Message { message, .. } = &entry.kind
            {
                push_attr(span, "gen_ai.completion", text_of(&message.text()));
            }
        }
        "tool.call" => {
            let Some(call_id) = row.call_id.as_deref() else {
                return;
            };
            if let Ok(Some(call)) = Store::tool_call_in(c, &row.session, call_id) {
                push_attr(
                    span,
                    "pipkin.tool.arguments",
                    text_of(&call.arguments.to_string()),
                );
            }
            let output: Option<String> = c
                .query_row(
                    "SELECT text FROM tool_outputs WHERE session = ?1 AND call_id = ?2",
                    rusqlite::params![row.session, call_id],
                    |r| r.get(0),
                )
                .ok();
            if let Some(output) = output {
                push_attr(span, "pipkin.tool.output", text_of(&output));
            }
        }
        _ => {}
    }
}

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
    health_revision: AtomicU64,
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
            health_revision: AtomicU64::new(0),
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
    pub content: ContentLevel,
}

/// The content level a settings value names (`metadata` when unset).
pub fn content_level(settings: &ExportSettings) -> Result<ContentLevel, String> {
    let text = settings.content.as_deref().unwrap_or("");
    ContentLevel::parse(text)
        .ok_or_else(|| format!("export content {text:?} is not supported (metadata or content)"))
}

/// `credential`: the stored collector header, added to the request.
pub fn target(
    settings: &ExportSettings,
    credential: Option<&(String, String)>,
) -> Result<Option<Target>, String> {
    if !settings.enabled {
        return Ok(None);
    }
    if let Some(kind) = settings.kind.as_deref().filter(|k| !k.is_empty())
        && kind != "otlp"
    {
        return Err(format!("export kind {kind:?} is not supported (only otlp)"));
    }
    let content = content_level(settings)?;
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
    let mut headers: Vec<(String, String)> = settings
        .headers
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
        .collect();
    if let Some((name, value)) = credential {
        headers.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
        headers.push((name.clone(), value.clone()));
    }
    Ok(Some(Target {
        endpoint: endpoint.to_owned(),
        headers,
        service_name: service_name_of(settings),
        retry_base: Duration::from_millis(settings.retry_base_ms.unwrap_or(1000).max(1)),
        content,
    }))
}

fn service_name_of(settings: &ExportSettings) -> String {
    settings
        .service_name
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "pipkin-engine".into())
}

fn valid_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
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
        self.export_health_for(None, edit);
    }

    fn export_health_for(&self, revision: Option<u64>, edit: impl FnOnce(&mut ExportHealthView)) {
        // Keep publication in the same order as health updates.
        let mut health = self.export.health.lock().unwrap_or_else(|e| e.into_inner());
        let changed = {
            if let Some(revision) = revision {
                if revision != self.export.health_revision.load(Ordering::Acquire) {
                    return;
                }
            } else {
                // The settings RPC supersedes every already-captured exporter snapshot.
                // Compare and advance under the health lock, so an old cycle cannot put
                // a removed credential or disabled configuration back into the view.
                self.export.health_revision.fetch_add(1, Ordering::Release);
            }
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
    fn export_collect(
        &self,
        level: ContentLevel,
        redactor: &Redactor,
    ) -> crate::store::Result<(Batch, u64, u64)> {
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
                for r in &rows {
                    if let Some(mut span) = span_of(r, &root) {
                        if level == ContentLevel::Content {
                            enrich(c, &mut span, r, redactor);
                        }
                        redact_span(&mut span, redactor);
                        batch.spans.push(span);
                    }
                }
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
            let health_revision = self.export.health_revision.load(Ordering::Acquire);
            let settings = Settings::load(&self.config.config_dir, None).0.export;
            let credential = self.export_credential();
            let target = match target(&settings, credential.as_ref()) {
                Ok(Some(t)) => t,
                Ok(None) | Err(_) => {
                    let error = target(&settings, credential.as_ref()).err();
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
                    let describe = self.export_description(&settings, credential.as_ref());
                    self.export_health_for(Some(health_revision), |h| {
                        describe(h);
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
            let describe = self.export_description(&settings, credential.as_ref());
            let redactor = self.export_redactor(&settings, credential.as_ref());
            let (batch, queued, aged) = match self.export_collect(target.content, &redactor) {
                Ok(c) => c,
                Err(error) => {
                    self.export_health_for(Some(health_revision), |h| {
                        describe(h);
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
                self.export_health_for(Some(health_revision), |h| {
                    describe(h);
                    h.endpoint = Some(host.clone());
                    h.queued = queued;
                    if h.state != "failed" {
                        h.state = "idle".into();
                    }
                });
                self.export_wait(None).await;
                continue;
            }
            self.export_health_for(Some(health_revision), |h| {
                describe(h);
                h.endpoint = Some(host.clone());
                h.queued = queued;
                h.state = "sending".into();
                h.failures = attempts;
            });
            // The credential may have changed while the batch was collected: post with the
            // one stored now.
            let target = match self::target(&settings, self.export_credential().as_ref()) {
                Ok(Some(t)) => t,
                _ => target,
            };
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
                    self.export_health_for(Some(health_revision), |h| {
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
                        self.export_health_for(Some(health_revision), |h| {
                            h.state = "failed".into();
                            h.failures = 0;
                            h.last_error = Some(error);
                            h.queued = queued.saturating_sub(batch.records);
                        });
                    } else {
                        let delay =
                            (target.retry_base * (1u32 << (attempts - 1).min(6))).min(MAX_BACKOFF);
                        backoff_until = Some(tokio::time::Instant::now() + delay);
                        self.export_health_for(Some(health_revision), |h| {
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

    /// The settings-derived fields of the health view (never a header value).
    fn export_description(
        &self,
        settings: &ExportSettings,
        credential: Option<&(String, String)>,
    ) -> impl Fn(&mut ExportHealthView) + use<> {
        let enabled = settings.enabled;
        let kind = settings
            .kind
            .clone()
            .filter(|k| !k.is_empty())
            .unwrap_or_else(|| "otlp".into());
        let content = content_level(settings)
            .map(|l| l.name().to_owned())
            .unwrap_or_else(|_| settings.content.clone().unwrap_or_default());
        let service_name = Some(service_name_of(settings));
        let endpoint_url = settings
            .endpoint
            .as_deref()
            .map(|e| e.split('?').next().unwrap_or(e).trim().to_owned())
            .filter(|e| !e.is_empty());
        let credential_name = credential.map(|(name, _)| name.clone());
        let headers_in_settings = !settings.headers.is_empty();
        move |h: &mut ExportHealthView| {
            h.enabled = enabled;
            h.kind = kind.clone();
            h.content = content.clone();
            h.service_name = service_name.clone();
            h.endpoint_url = endpoint_url.clone();
            h.credential = credential_name.clone();
            h.headers_in_settings = headers_in_settings;
        }
    }

    fn export_redactor(
        &self,
        settings: &ExportSettings,
        credential: Option<&(String, String)>,
    ) -> Redactor {
        let mut secrets: Vec<String> = settings
            .headers
            .values()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect();
        if let Some((_, value)) = credential {
            secrets.push(value.clone());
        }
        Redactor::new(secrets)
    }

    fn export_credential_store(&self) -> FileStore {
        FileStore::new(self.config.config_dir.join("export-auth.json"))
    }

    /// The stored collector credential as `(header name, value)`.
    pub fn export_credential(&self) -> Option<(String, String)> {
        match self.export_credential_store().read(CREDENTIAL_ID) {
            Ok(Some(StoredCredential::ApiKey {
                key: Some(value),
                extra,
                ..
            })) => Some((
                extra
                    .get("header")
                    .and_then(Value::as_str)
                    .unwrap_or("Authorization")
                    .to_owned(),
                value,
            )),
            _ => None,
        }
    }

    /// Re-read settings and the credential into the health view now (after a change made
    /// through a call), without waiting for the exporter's next cycle.
    fn export_refresh_health(&self) {
        let settings = Settings::load(&self.config.config_dir, None).0.export;
        let credential = self.export_credential();
        let resolved = target(&settings, credential.as_ref());
        let describe = self.export_description(&settings, credential.as_ref());
        self.export_health(|h| {
            describe(h);
            match &resolved {
                Ok(None) => {
                    h.state = "off".into();
                    h.endpoint = None;
                    h.last_error = None;
                }
                Ok(Some(t)) => {
                    h.endpoint = Some(endpoint_host(&t.endpoint));
                    if h.state == "off" || h.state == "failed" {
                        h.state = "idle".into();
                        h.last_error = None;
                    }
                }
                Err(error) => {
                    h.state = "failed".into();
                    h.endpoint = None;
                    h.last_error = Some(error.clone());
                }
            }
        });
        self.export.notify.notify_one();
    }

    pub fn export_set_credential(&self, header: &str, value: &str) -> Result<(), ProtoError> {
        let header = header.trim();
        if !valid_header_name(header) {
            return Err(ProtoError::new(
                ErrorCode::Invalid,
                "The header name must be letters, digits, '-' or '_' (at most 64)",
            ));
        }
        if value.trim().is_empty() || value.len() > 4096 {
            return Err(ProtoError::new(
                ErrorCode::Invalid,
                "The credential value is empty or too long",
            ));
        }
        let storage = |e: crate::auth::store::StoreError| {
            ProtoError::new(
                ErrorCode::Storage,
                format!("Cannot store the credential: {e}"),
            )
        };
        let mut txn = self.export_credential_store().begin().map_err(storage)?;
        let mut extra = serde_json::Map::new();
        extra.insert("header".into(), json!(header));
        txn.set(
            CREDENTIAL_ID,
            StoredCredential::ApiKey {
                key: Some(value.trim().to_owned()),
                env: Default::default(),
                extra,
            },
        );
        txn.commit().map_err(storage)?;
        self.export_refresh_health();
        Ok(())
    }

    pub fn export_clear_credential(&self) -> Result<(), ProtoError> {
        let storage = |e: crate::auth::store::StoreError| {
            ProtoError::new(
                ErrorCode::Storage,
                format!("Cannot update the credential: {e}"),
            )
        };
        let mut txn = self.export_credential_store().begin().map_err(storage)?;
        txn.remove(CREDENTIAL_ID);
        txn.commit().map_err(storage)?;
        self.export_refresh_health();
        Ok(())
    }

    /// Write `export` in the user settings file and apply it.
    pub fn export_configure(
        &self,
        enabled: bool,
        endpoint: Option<String>,
        content: &str,
        service_name: Option<String>,
    ) -> Result<ExportHealthView, ProtoError> {
        let level = ContentLevel::parse(content).ok_or_else(|| {
            ProtoError::new(
                ErrorCode::Invalid,
                format!("Content level {content:?} must be metadata or content"),
            )
        })?;
        let endpoint = endpoint
            .map(|e| e.trim().to_owned())
            .filter(|e| !e.is_empty());
        if enabled {
            match &endpoint {
                None => {
                    return Err(ProtoError::new(
                        ErrorCode::Invalid,
                        "An endpoint URL is needed to enable export",
                    ));
                }
                Some(e) if !(e.starts_with("http://") || e.starts_with("https://")) => {
                    return Err(ProtoError::new(
                        ErrorCode::Invalid,
                        "The endpoint must be an http(s) URL",
                    ));
                }
                _ => {}
            }
        }
        let service_name = service_name
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        let path = self.config.config_dir.join("settings.json");
        Settings::save_at(&path, |value| {
            let root = value.as_object_mut().expect("object");
            let export = root
                .entry("export")
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            if !export.is_object() {
                *export = Value::Object(serde_json::Map::new());
            }
            let e = export.as_object_mut().expect("object");
            e.insert("enabled".into(), json!(enabled));
            e.insert("kind".into(), json!("otlp"));
            e.insert("content".into(), json!(level.name()));
            match &endpoint {
                Some(url) => {
                    e.insert("endpoint".into(), json!(url));
                }
                None => {
                    e.remove("endpoint");
                }
            }
            match &service_name {
                Some(name) => {
                    e.insert("serviceName".into(), json!(name));
                }
                None => {
                    e.remove("serviceName");
                }
            }
        })
        .map_err(|e| {
            ProtoError::new(
                ErrorCode::Storage,
                format!("Cannot write {}: {e}", path.display()),
            )
        })?;
        self.export_refresh_health();
        Ok(self.export.health())
    }

    /// An example payload at `content` level from the session's newest records (or a
    /// built-in sample when there are none), pretty-printed and bounded.
    pub fn export_preview(
        &self,
        session: Option<&str>,
        content: &str,
    ) -> Result<String, ProtoError> {
        let level = ContentLevel::parse(content).ok_or_else(|| {
            ProtoError::new(
                ErrorCode::Invalid,
                format!("Content level {content:?} must be metadata or content"),
            )
        })?;
        let settings = Settings::load(&self.config.config_dir, None).0.export;
        let credential = self.export_credential();
        let redactor = self.export_redactor(&settings, credential.as_ref());
        let mut spans: Vec<Value> = Vec::new();
        if let Some(session) = session {
            if self.store.session(session)?.is_none() {
                return Err(ProtoError::new(ErrorCode::NotFound, "Unknown session"));
            }
            spans = self.store.read(|c| {
                let (mut rows, _) = Store::diagnostics_page(c, session, None, 8)?;
                rows.reverse();
                let root: String = c
                    .query_row(
                        "SELECT COALESCE(root, id) FROM sessions WHERE id = ?1",
                        [session],
                        |r| r.get(0),
                    )
                    .unwrap_or_else(|_| session.to_owned());
                let mut spans = Vec::new();
                for r in &rows {
                    if let Some(mut span) = span_of(r, &root) {
                        if level == ContentLevel::Content {
                            enrich(c, &mut span, r, &redactor);
                        }
                        redact_span(&mut span, &redactor);
                        spans.push(span);
                    }
                }
                Ok(spans)
            })?;
        }
        if spans.is_empty() {
            spans = sample_spans(level);
        }
        let body = payload(spans, &service_name_of(&settings));
        let mut text = serde_json::to_string_pretty(&body).unwrap_or_default();
        if text.len() > PREVIEW_BYTES {
            let mut end = PREVIEW_BYTES;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            text.push_str("\n… (preview cut)");
        }
        Ok(text)
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

/// A made-up turn, for a preview when the session has no records yet.
fn sample_spans(level: ContentLevel) -> Vec<Value> {
    let session = "sample-session";
    let at = now_ms();
    let rows = [
        DiagRow {
            session: session.into(),
            seq: 1,
            at_ms: at - 1_200,
            kind: "provider.request".into(),
            run: Some(1),
            request: None,
            call_id: None,
            record: json!({"model": "example/model", "attempt": 1, "durationMs": 900,
                "firstTokenMs": 250, "outcome": "completed",
                "tokens": {"input": 1200, "output": 80, "cacheRead": 0, "cacheWrite": 0}}),
        },
        DiagRow {
            session: session.into(),
            seq: 2,
            at_ms: at - 1_000,
            kind: "tool.call".into(),
            run: Some(1),
            request: None,
            call_id: Some("call_1".into()),
            record: json!({"name": "read", "outcome": "ok", "durationMs": 12,
                "retainedBytes": 2048, "droppedBytes": 0, "replaySafe": true}),
        },
        DiagRow {
            session: session.into(),
            seq: 3,
            at_ms: at,
            kind: "run.end".into(),
            run: Some(1),
            request: Some("r1".into()),
            call_id: None,
            record: json!({"outcome": "completed", "durationMs": 2_100, "stopRequested": false,
                "queueWaitMs": 3}),
        },
    ];
    let mut spans: Vec<Value> = rows.iter().filter_map(|r| span_of(r, session)).collect();
    if level == ContentLevel::Content {
        for span in &mut spans {
            match span["name"].as_str() {
                Some("generation") => push_attr(
                    span,
                    "gen_ai.completion",
                    "(the model's reply text, up to 4,096 characters)".into(),
                ),
                Some("tool read") => {
                    push_attr(
                        span,
                        "pipkin.tool.arguments",
                        "{\"path\": \"src/main.rs\"}".into(),
                    );
                    push_attr(
                        span,
                        "pipkin.tool.output",
                        "(the tool's output, up to 4,096 characters)".into(),
                    );
                }
                Some("run") => {
                    push_attr(
                        span,
                        "pipkin.input",
                        "(the person's prompt, up to 4,096 characters)".into(),
                    );
                    push_attr(
                        span,
                        "pipkin.output",
                        "(the final reply, up to 4,096 characters)".into(),
                    );
                }
                _ => {}
            }
        }
    }
    spans
}

pub(super) fn start(shared: &Arc<Shared>) {
    let shared = shared.clone();
    let runtime = shared.clone();
    runtime.spawn(async move { shared.export_run().await });
}

#[cfg(test)]
mod health_race_tests {
    use super::*;
    use crate::provider::scripted::Scripted;
    use crate::runtime::tests::Fixture;

    #[test]
    fn an_old_export_cycle_cannot_restore_removed_credential_or_enabled_status() {
        let fx = Fixture::new();
        let provider = Scripted::echo("unused");
        let engine = fx.start(&provider);
        let shared = engine.handle.shared();
        shared
            .export_set_credential("Authorization", "fixture-only-secret")
            .unwrap();
        let stale = shared.export.health_revision.load(Ordering::Acquire);
        shared.export_clear_credential().unwrap();
        shared
            .export_configure(false, None, "metadata", None)
            .unwrap();
        let current = shared.export.health();
        assert!(current.credential.is_none());
        assert!(!current.enabled);
        assert_eq!(current.state, "off");
        shared.export_health_for(Some(stale), |h| {
            h.credential = Some("Authorization".into());
            h.enabled = true;
            h.state = "sending".into();
        });
        assert_eq!(shared.export.health(), current);
        assert!(provider.requests().is_empty());
    }
}
