//! Context derivation and system prompt used by the run loop.
//!
//! This is the runtime's narrow seam onto `crate::context` (the pure prompt/context/compaction
//! module); it keeps the system prompt byte-stable per session for provider prompt caching.

use pipkin_engine_protocol::*;
use serde_json::{Map, json};

/// The next request's messages from the raw context range (newest head marker first).
pub fn model_messages(entries: &[Entry]) -> Vec<Message> {
    let mut out: Vec<Message> = Vec::new();
    for entry in entries {
        if entry.retired {
            continue;
        }
        match &entry.kind {
            EntryKind::Compaction { summary, .. } => out.push(Message::User {
                content: vec![Block::text(format!(
                    "The conversation history before this point was compacted into the following summary:\n\n<summary>\n{summary}\n</summary>"
                ))],
                timestamp: entry.at_ms,
                extra: Map::new(),
            }),
            EntryKind::Reset {
                handoff: Some(message),
            } => out.push(message.clone()),
            EntryKind::Message {
                message, partial, ..
            } => {
                if *partial {
                    continue;
                }
                if let Message::Assistant { stop_reason, .. } = message
                    && matches!(stop_reason, StopReason::Aborted | StopReason::Error)
                {
                    continue;
                }
                out.push(message.clone());
            }
            _ => {}
        }
    }
    order_tool_results(out)
}

/// Each assistant's tool results directly after it, in call order; a missing result is
/// synthesized; results without a matching call are dropped (reference `orderToolResults`).
pub fn order_tool_results(messages: Vec<Message>) -> Vec<Message> {
    let mut results: Vec<Option<Message>> = Vec::new();
    let mut others: Vec<Message> = Vec::new();
    for m in messages {
        match m {
            Message::ToolResult { .. } => results.push(Some(m)),
            other => others.push(other),
        }
    }
    let mut out = Vec::new();
    for m in others {
        let calls: Vec<(String, String)> = m
            .tool_calls()
            .into_iter()
            .map(|(id, name, _)| (id.to_owned(), name.to_owned()))
            .collect();
        let ts = m.timestamp();
        out.push(m);
        for (id, name) in calls {
            let found = results.iter_mut().find(|r| {
                matches!(r, Some(Message::ToolResult { tool_call_id, .. }) if *tool_call_id == id)
            });
            match found.and_then(Option::take) {
                Some(r) => out.push(r),
                None => {
                    let mut extra = Map::new();
                    extra.insert("details".into(), json!({"reason": "missing_result"}));
                    out.push(Message::ToolResult {
                        tool_call_id: id,
                        tool_name: name,
                        content: vec![Block::text(
                            "Tool result unavailable: history ends before this call completed.",
                        )],
                        is_error: true,
                        timestamp: ts,
                        extra,
                    });
                }
            }
        }
    }
    out
}

const PREAMBLE: &str = "You are an expert coding assistant operating inside pi, a coding agent harness. You help users by reading files, executing commands, editing code, and writing new files.";

/// A minimal system prompt (preamble, tools, cwd).
pub fn system_prompt(cwd: &str, tools: &[String]) -> String {
    let mut parts = vec![PREAMBLE.to_owned()];
    if !tools.is_empty() {
        parts.push(format!("<tools>\n{}\n</tools>", tools.join("\n")));
    }
    parts.push(format!("<cwd>\n{}\n</cwd>", cwd.replace('\\', "/")));
    parts.join("\n\n")
}
