//! Effective model context on both sides of an import, so an import can prove it preserves
//! what the next provider request would see.
//!
//! The reference side ports `durable/src/harness/context.ts` (`deriveContext`, `selectActive`,
//! `orderToolResults`) for an unforked conversation. The native side mirrors
//! [`Store::context_entries`](crate::store::Store::context_entries) followed by the documented
//! context-builder rules (retired entries leave the context; a compaction contributes its
//! wrapped summary; a reset its handoff). Positional system messages are compared and replayed
//! too, including imports made before they became a native message variant.

use pipkin_engine_protocol::{Entry, EntryKind, Message};
use serde_json::{Value, json};

use super::legacy::LegacyEntry;

/// `durable/src/harness/compaction.ts:57-59`.
pub const SUMMARY_PREFIX: &str = "The conversation history before this point was compacted into the following summary:\n\n<summary>\n";
pub const SUMMARY_SUFFIX: &str = "\n</summary>";
/// `context.ts:9`.
const MISSING_RESULT_TEXT: &str =
    "Tool result unavailable: history ends before this call completed.";
/// `context.ts:8`.
const EXCLUDED_STOP_REASONS: [&str; 3] = ["aborted", "error", "deferred"];

/// Reference message JSON as the native model stores it: a user/tool-result `content` string
/// becomes one text block, then the message goes through the native type. Returns the native
/// message and whether its JSON differs from the source JSON.
pub fn normalize_message(source: &Value) -> Result<(Message, bool), String> {
    let mut value = source.clone();
    let mut changed = false;
    if value.get("role").and_then(Value::as_str) != Some("system")
        && let Some(Value::String(text)) = value.get("content")
    {
        let block = json!([{"type": "text", "text": text}]);
        value["content"] = block;
        changed = true;
    }
    let message: Message = serde_json::from_value(value.clone())
        .map_err(|e| format!("does not match the native message model ({e})"))?;
    let back = serde_json::to_value(&message).map_err(|e| e.to_string())?;
    Ok((message, changed || !same_json(&back, &value)))
}

/// JSON equality where numbers compare by value (`0` and `0.0` are one JSON number, as in the
/// reference's JavaScript; the native model stores costs as floats).
pub fn same_json(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            if x.is_f64() || y.is_f64() {
                x.as_f64() == y.as_f64()
            } else {
                x == y
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same_json(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| same_json(v, w)))
        }
        _ => a == b,
    }
}

fn stop_reason_excluded(message: &Value) -> bool {
    message.get("role").and_then(Value::as_str) == Some("assistant")
        && message
            .get("stopReason")
            .and_then(Value::as_str)
            .is_some_and(|r| EXCLUDED_STOP_REASONS.contains(&r))
}

/// `orderToolResults` (`context.ts:129-163`) over message JSON.
pub fn order_tool_results(messages: Vec<Value>) -> Vec<Value> {
    let role = |m: &Value| m.get("role").and_then(Value::as_str).map(str::to_owned);
    let mut ordered = Vec::with_capacity(messages.len());
    for (index, message) in messages.iter().enumerate() {
        let r = role(message);
        if r.as_deref() == Some("toolResult") {
            continue;
        }
        ordered.push(message.clone());
        if r.as_deref() != Some("assistant") {
            continue;
        }
        let calls: Vec<(String, String)> = message
            .get("content")
            .and_then(Value::as_array)
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("toolCall"))
                    .map(|b| {
                        (
                            b.get("id").and_then(Value::as_str).unwrap_or("").to_owned(),
                            b.get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_owned(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        if calls.is_empty() {
            continue;
        }
        let mut results: Vec<(String, usize)> = Vec::new();
        for (next, candidate) in messages.iter().enumerate().skip(index + 1) {
            match role(candidate).as_deref() {
                Some("assistant") => break,
                Some("toolResult") => {
                    let id = candidate
                        .get("toolCallId")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    if !results.iter().any(|(r, _)| *r == id) {
                        results.push((id, next));
                    }
                }
                _ => {}
            }
        }
        let timestamp = message.get("timestamp").cloned().unwrap_or(json!(0));
        for (id, name) in calls {
            match results.iter().find(|(r, _)| *r == id) {
                Some((_, at)) => ordered.push(messages[*at].clone()),
                None => ordered.push(json!({
                    "role": "toolResult",
                    "toolCallId": id,
                    "toolName": name,
                    "content": [{"type": "text", "text": MISSING_RESULT_TEXT}],
                    "isError": true,
                    "details": {"reason": "missing_result"},
                    "timestamp": timestamp,
                })),
            }
        }
    }
    ordered
}

/// The reference model context (including system messages) of one unforked conversation whose
/// entries are given in id order. Messages are normalized with [`normalize_message`].
pub fn reference_messages(entries: &[&LegacyEntry]) -> Result<Vec<Value>, String> {
    let head = entries.iter().rev().find(|e| e.head().is_some());
    let from = match head {
        Some(h) => h
            .head()
            .and_then(Value::as_i64)
            .ok_or_else(|| format!("entry {} has a non-numeric head", h.id))?,
        None => i64::MIN,
    };
    let range: Vec<&LegacyEntry> = entries.iter().copied().filter(|e| e.id >= from).collect();
    let mut edits: std::collections::HashMap<i64, Value> = Default::default();
    for entry in &range {
        for edit in entry.edits().into_iter().flatten() {
            if let Some(target) = edit.get("target").and_then(Value::as_i64) {
                edits.insert(target, edit.clone());
            }
        }
    }
    let mut active: Vec<&LegacyEntry> = Vec::new();
    if let Some(h) = head {
        active.push(h);
        active.extend(range.iter().copied().filter(|e| e.head().is_none()));
    } else {
        active = range;
    }
    let mut messages = Vec::new();
    for entry in active {
        let contributed: Vec<Value> = match edits.get(&entry.id) {
            Some(edit) if edit.get("action").and_then(Value::as_str) == Some("omit") => vec![],
            Some(edit) if edit.get("action").and_then(Value::as_str) == Some("replace") => edit
                .get("messages")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            _ => entry.model().cloned().unwrap_or_default(),
        };
        messages.extend(contributed.into_iter().filter(|m| !stop_reason_excluded(m)));
    }
    let ordered = order_tool_results(messages);
    let mut out = Vec::with_capacity(ordered.len());
    for message in ordered {
        let (native, _) = normalize_message(&message)?;
        out.push(serde_json::to_value(native).map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// What [`Store::context_entries`](crate::store::Store::context_entries) returns, computed
/// over a whole (committed) entry list in id order.
pub fn native_range(entries: &[Entry]) -> Vec<Entry> {
    let marker = entries.iter().rev().find_map(|e| match &e.kind {
        EntryKind::Compaction { first_kept, .. } => Some((e.id, *first_kept)),
        EntryKind::Reset { .. } => Some((e.id, e.id + 1)),
        _ => None,
    });
    let mut out = Vec::new();
    let from = match marker {
        Some((id, from)) => {
            if let Some(m) = entries.iter().find(|e| e.id == id) {
                out.push(m.clone());
            }
            from
        }
        None => 0,
    };
    out.extend(
        entries
            .iter()
            .filter(|e| {
                e.id >= from
                    && !matches!(
                        e.kind,
                        EntryKind::Compaction { .. } | EntryKind::Reset { .. }
                    )
                    && !matches!(e.kind, EntryKind::Message { partial: true, .. })
            })
            .cloned(),
    );
    out
}

/// The native model context (including system messages) of a context range.
pub fn native_messages(range: &[Entry]) -> Vec<Value> {
    crate::context::derive::model_messages(range)
        .iter()
        .map(|m| serde_json::to_value(m).expect("valid message"))
        .collect()
}
