//! Local diagnostics (plan §12, OBS-1): the engine records bounded structured facts about
//! each session's work, keyed by the identities it already has (session, run, request key,
//! tool call id, parent session and call for a child), so a person or a later exporter can
//! correlate them without scraping logs:
//!
//! - `run.start`: queue wait (admission to start), input mode, parent/call for a child;
//! - `provider.request`: model, attempt, wall time, time to the first streamed token,
//!   outcome (`completed` / `error` / `cancelled`), token counts, a bounded error text;
//! - `provider.retry`: the attempt that failed and the delay before the next;
//! - `tool.call`: name, wall time, outcome (`ok` / `error` / `cancelled` / `interrupted` /
//!   `unavailable`), retained and dropped output bytes;
//! - `run.end`: outcome, duration, whether Stop was requested and how long it took;
//! - `run.resumed`: a run picked up again after a restart.
//!
//! Records never carry prompts, tool bodies, credentials or source text. Each session keeps
//! its newest 2,000 (`store::diagnostics::MAX_PER_SESSION`); they survive restarts with the
//! session and go with it when it is removed. Everything here is engine-measured model and
//! tool latency; the desktop's own input-to-frame figures are a separate measurement.

use std::path::PathBuf;

use pipkin_engine_protocol::*;
use serde_json::{Value, json};

use super::Shared;
use crate::store::Store;
use crate::store::diagnostics::DiagRow;
use crate::util::now_ms;

/// Largest page one read returns.
pub const MAX_PAGE: u32 = 500;
/// Error texts are cut to this many characters (they are provider messages, not bodies).
pub const ERROR_CHARS: usize = 200;

pub fn bounded_text(text: &str) -> String {
    let mut out: String = text.chars().take(ERROR_CHARS).collect();
    if text.chars().count() > ERROR_CHARS {
        out.push('…');
    }
    out
}

pub fn outcome_name(outcome: &RunOutcome) -> (&'static str, Option<String>) {
    match outcome {
        RunOutcome::Completed => ("completed", None),
        RunOutcome::Aborted => ("aborted", None),
        RunOutcome::ModelError { detail } => ("modelError", Some(bounded_text(detail))),
        RunOutcome::NoModel { detail } => ("noModel", Some(bounded_text(detail))),
        RunOutcome::Interrupted { detail } => ("interrupted", Some(bounded_text(detail))),
        RunOutcome::Failed { detail } => ("failed", Some(bounded_text(detail))),
    }
}

fn percentile(values: &mut [u64], p: f64) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let rank = ((values.len() as f64 - 1.0) * p).round() as usize;
    values.get(rank.min(values.len() - 1)).copied()
}

fn ms(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(Value::as_u64)
}

/// Figures over the retained rows (oldest or newest first; order does not matter).
pub fn summarize(rows: &[DiagRow]) -> DiagnosticsSummary {
    let mut s = DiagnosticsSummary {
        records: rows.len() as u64,
        ..DiagnosticsSummary::default()
    };
    let mut provider: Vec<u64> = Vec::new();
    let mut first: Vec<u64> = Vec::new();
    let mut tools: Vec<u64> = Vec::new();
    for r in rows {
        match r.kind.as_str() {
            "run.start" => {
                s.runs += 1;
                if let Some(w) = ms(&r.record, "queueWaitMs") {
                    s.queue_wait_max_ms = Some(s.queue_wait_max_ms.map_or(w, |m| m.max(w)));
                }
            }
            "run.end" => {
                if r.record.get("outcome").and_then(Value::as_str) == Some("aborted") {
                    s.cancellations += 1;
                }
                if let Some(l) = ms(&r.record, "stopLatencyMs") {
                    s.stop_latency_max_ms = Some(s.stop_latency_max_ms.map_or(l, |m| m.max(l)));
                }
            }
            "provider.request" => {
                s.provider_requests += 1;
                match r.record.get("outcome").and_then(Value::as_str) {
                    Some("error") => s.provider_errors += 1,
                    Some("cancelled") => {}
                    _ => {}
                }
                if let Some(d) = ms(&r.record, "durationMs") {
                    provider.push(d);
                }
                if let Some(f) = ms(&r.record, "firstTokenMs") {
                    first.push(f);
                }
            }
            "provider.retry" => s.retries += 1,
            "tool.call" => {
                s.tool_calls += 1;
                if r.record.get("outcome").and_then(Value::as_str) == Some("error") {
                    s.tool_errors += 1;
                }
                if let Some(d) = ms(&r.record, "durationMs") {
                    tools.push(d);
                }
            }
            _ => {}
        }
    }
    s.provider_p50_ms = percentile(&mut provider, 0.5);
    s.provider_p95_ms = percentile(&mut provider, 0.95);
    s.first_token_p50_ms = percentile(&mut first, 0.5);
    s.tool_p50_ms = percentile(&mut tools, 0.5);
    s.tool_p95_ms = percentile(&mut tools, 0.95);
    s
}

pub fn record_view(r: &DiagRow) -> DiagnosticRecord {
    DiagnosticRecord {
        seq: r.seq,
        at_ms: r.at_ms,
        kind: r.kind.clone(),
        run: r.run,
        request: r.request.clone(),
        call_id: r.call_id.clone(),
        data: r.record.clone(),
    }
}

impl Shared {
    /// `run.start`, in the transaction that starts the run: queue wait from the first
    /// input's admission, its mode, how many inputs joined, and the parent and call for a
    /// child session.
    pub(super) fn diag_run_start(
        tx: &rusqlite::Connection,
        session: &str,
        run: u64,
        first: &crate::store::RequestRow,
        inputs: usize,
        now: i64,
    ) -> crate::store::Result<()> {
        let (parent, parent_call) = Store::task_by_session(tx, session)?
            .map_or((None, None), |t| {
                (Some(t.parent_session), Some(t.parent_call))
            });
        Store::diag_append(
            tx,
            session,
            now,
            "run.start",
            Some(run),
            Some(&first.key),
            None,
            &json!({
                "queueWaitMs": now.saturating_sub(first.created_at_ms).max(0),
                "mode": first.mode,
                "inputs": inputs,
                "parent": parent,
                "parentCall": parent_call,
            }),
        )?;
        Ok(())
    }

    /// Record a fact in its own transaction. A failure to record is logged and never
    /// fails the work it describes.
    pub(super) fn diag(
        &self,
        session: &str,
        kind: &str,
        run: Option<u64>,
        request: Option<&str>,
        call_id: Option<&str>,
        data: Value,
    ) {
        let result = tokio::task::block_in_place(|| {
            self.store.tx(|tx| {
                Store::assert_session_live(tx, session)?;
                Store::diag_append(tx, session, now_ms(), kind, run, request, call_id, &data)?;
                Ok(())
            })
        });
        if let Err(error) = result {
            log::debug!("session {session}: diagnostics not recorded ({kind}): {error}");
        }
    }

    pub fn diagnostics_view(
        &self,
        session: &str,
        before: Option<u64>,
        limit: u32,
    ) -> Result<DiagnosticsView, ProtoError> {
        if self.store.session(session)?.is_none() {
            return Err(ProtoError::new(ErrorCode::NotFound, "Unknown session"));
        }
        let limit = limit.clamp(1, MAX_PAGE);
        let (summary, records, has_older) = self.store.read(|c| {
            let all = Store::diagnostics_all(c, session)?;
            let (page, has_older) = Store::diagnostics_page(c, session, before, limit)?;
            Ok((summarize(&all), page, has_older))
        })?;
        Ok(DiagnosticsView {
            session: session.to_owned(),
            summary,
            records: records.iter().map(record_view).collect(),
            has_older,
        })
    }

    /// Write every retained record as JSON lines under the engine's data directory.
    pub fn export_diagnostics(&self, session: &str) -> Result<(PathBuf, u64), ProtoError> {
        if self.store.session(session)?.is_none() {
            return Err(ProtoError::new(ErrorCode::NotFound, "Unknown session"));
        }
        let rows = self.store.read(|c| Store::diagnostics_all(c, session))?;
        let dir = self.config.data_dir.join("diagnostics");
        std::fs::create_dir_all(&dir).map_err(|e| {
            ProtoError::new(
                ErrorCode::Storage,
                format!("Cannot create {}: {e}", dir.display()),
            )
        })?;
        let safe: String = session
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let path = dir.join(format!("{safe}.jsonl"));
        let mut body = String::new();
        for r in &rows {
            let line = json!({
                "session": r.session,
                "seq": r.seq,
                "atMs": r.at_ms,
                "kind": r.kind,
                "run": r.run,
                "request": r.request,
                "callId": r.call_id,
                "data": r.record,
                "source": "pipkin-engine",
            });
            body.push_str(&line.to_string());
            body.push('\n');
        }
        std::fs::write(&path, body).map_err(|e| {
            ProtoError::new(
                ErrorCode::Storage,
                format!("Cannot write {}: {e}", path.display()),
            )
        })?;
        Ok((path, rows.len() as u64))
    }
}
