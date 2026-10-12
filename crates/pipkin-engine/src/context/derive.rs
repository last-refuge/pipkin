//! Model context from durable entries (reference `durable/src/harness/context.ts`).
//!
//! Input is [`crate::store::Store::context_entries`]: the newest head marker (a compaction or
//! a reset) first when there is one, then every later entry of the live range, oldest first.
//! Each entry contributes zero or more messages:
//!
//! - the head marker: a compaction contributes its summary as a user message; a reset
//!   contributes its handoff message, if any;
//! - a message entry contributes its message, unless it is retired (a stopped instruction and
//!   its partial reply), still streaming (`partial`), or an assistant message that ended
//!   `aborted` or `error`;
//! - notices, model/thinking changes and custom entries contribute nothing; so does any head
//!   marker after the first (the reference drops older markers from the active range).
//!
//! Then tool results are placed directly after their assistant in call order, a missing
//! result is synthesized and unmatched results are dropped ([`order_tool_results`]).

use pipkin_engine_protocol::{Block, Entry, EntryKind, Message, StopReason};
use serde_json::{Map, Value, json};

/// Text of a tool result synthesized for a call whose result is not in the context.
pub const MISSING_RESULT_TEXT: &str =
    "Tool result unavailable: history ends before this call completed.";

/// Prefix and suffix of the user message that carries a compaction summary.
pub const SUMMARY_PREFIX: &str = "The conversation history before this point was compacted into the following summary:\n\n<summary>\n";
pub const SUMMARY_SUFFIX: &str = "\n</summary>";

/// The compaction summary as the model sees it.
pub fn summary_message_text(summary: &str) -> String {
    format!("{SUMMARY_PREFIX}{summary}{SUMMARY_SUFFIX}")
}

/// The entry is a head marker (a compaction or a reset).
pub fn is_head_marker(entry: &Entry) -> bool {
    matches!(
        entry.kind,
        EntryKind::Compaction { .. } | EntryKind::Reset { .. }
    )
}

fn excluded(message: &Message) -> bool {
    matches!(
        message,
        Message::Assistant {
            stop_reason: StopReason::Aborted | StopReason::Error,
            ..
        }
    )
}

/// What one entry contributes to the model context. `head` says whether the entry is the
/// range's head marker (index 0 of the context entries).
pub fn contribution(entry: &Entry, head: bool) -> Vec<Message> {
    if entry.retired {
        return vec![];
    }
    let messages = match &entry.kind {
        EntryKind::Message {
            message, partial, ..
        } => {
            if *partial {
                vec![]
            } else {
                vec![message.clone()]
            }
        }
        EntryKind::Compaction { summary, .. } if head => vec![Message::User {
            content: vec![Block::text(summary_message_text(summary))],
            timestamp: entry.at_ms,
            extra: Map::new(),
        }],
        EntryKind::Reset { handoff } if head => handoff.iter().cloned().collect(),
        // Legacy native imports kept pi.system in its original record. Replay it too.
        EntryKind::Custom { custom_type, data } if custom_type == "pi.system" => data
            .get("model")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|m| serde_json::from_value::<Message>(m.clone()).ok())
            .filter(|m| matches!(m, Message::System(_)))
            .collect(),
        _ => vec![],
    };
    messages.into_iter().filter(|m| !excluded(m)).collect()
}

/// Per-entry contributions of the context entries (the first entry is the head marker when
/// it is a compaction or reset).
pub fn contributions(entries: &[Entry]) -> Vec<Vec<Message>> {
    let has_head = entries.first().is_some_and(is_head_marker);
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| contribution(entry, has_head && index == 0))
        .collect()
}

/// The next request's messages, oldest first.
pub fn model_messages(entries: &[Entry]) -> Vec<Message> {
    order_tool_results(contributions(entries).into_iter().flatten().collect())
}

/// One slot of an ordered context: an input message by index, or a synthesized result.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Slot {
    Input(usize),
    Missing {
        call_id: String,
        tool_name: String,
        timestamp: i64,
    },
}

/// `orderToolResults` over indices, so callers can trace a derived message to its source.
pub(crate) fn order_slots(messages: &[Message]) -> Vec<Slot> {
    let mut ordered = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        if matches!(message, Message::ToolResult { .. }) {
            continue;
        }
        ordered.push(Slot::Input(index));
        let Message::Assistant { timestamp, .. } = message else {
            continue;
        };
        let calls = message.tool_calls();
        if calls.is_empty() {
            continue;
        }
        // The first result per call id among the messages before the next assistant.
        let mut results: Vec<(&str, usize)> = Vec::new();
        for (next, candidate) in messages.iter().enumerate().skip(index + 1) {
            match candidate {
                Message::Assistant { .. } => break,
                Message::ToolResult { tool_call_id, .. } => {
                    if !results.iter().any(|(id, _)| id == tool_call_id) {
                        results.push((tool_call_id, next));
                    }
                }
                Message::User { .. } | Message::System(_) => {}
            }
        }
        for (id, name, _) in calls {
            match results.iter().find(|(result_id, _)| *result_id == id) {
                Some((_, at)) => ordered.push(Slot::Input(*at)),
                None => ordered.push(Slot::Missing {
                    call_id: id.to_string(),
                    tool_name: name.to_string(),
                    timestamp: *timestamp,
                }),
            }
        }
    }
    ordered
}

/// The synthesized result for a call without one.
pub fn missing_result(call_id: &str, tool_name: &str, timestamp: i64) -> Message {
    let mut extra = Map::new();
    extra.insert("details".to_string(), json!({"reason": "missing_result"}));
    Message::ToolResult {
        tool_call_id: call_id.to_string(),
        tool_name: tool_name.to_string(),
        content: vec![Block::text(MISSING_RESULT_TEXT)],
        is_error: true,
        timestamp,
        extra,
    }
}

pub(crate) fn materialize(messages: &[Message], slots: &[Slot]) -> Vec<Message> {
    slots
        .iter()
        .map(|slot| match slot {
            Slot::Input(index) => messages[*index].clone(),
            Slot::Missing {
                call_id,
                tool_name,
                timestamp,
            } => missing_result(call_id, tool_name, *timestamp),
        })
        .collect()
}

/// Place each assistant's tool results directly after it in call order. Results are taken
/// from the messages before the next assistant (the first one per call id); a missing result
/// is synthesized (`isError`, `details: {reason: "missing_result"}`, the assistant's
/// timestamp) and results that match no preceding call are dropped.
pub fn order_tool_results(messages: Vec<Message>) -> Vec<Message> {
    let slots = order_slots(&messages);
    materialize(&messages, &slots)
}

/// `details` of a tool result, if any (convenience for callers and tests).
pub fn tool_result_details(message: &Message) -> Option<&Value> {
    match message {
        Message::ToolResult { extra, .. } => extra.get("details"),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
