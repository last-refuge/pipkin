//! Builders for context tests.

use pipkin_engine_protocol::{
    Block, Entry, EntryId, EntryKind, InputMode, Message, NoticeLevel, StopReason, Usage,
};
use serde_json::{Map, json};

/// `text(label, tokens)` from the reference tests: `label` plus `x` padding to `tokens * 4`
/// characters.
pub fn text(label: &str, tokens: usize) -> String {
    format!(
        "{label} {}",
        "x".repeat((tokens * 4).saturating_sub(label.len() + 1))
    )
}

pub fn user(content: &str) -> Message {
    Message::user_text(content, 0)
}

pub fn assistant_with(content: &str, calls: &[&str], stop: StopReason, usage: u64) -> Message {
    let mut blocks = vec![Block::text(content)];
    for id in calls {
        blocks.push(Block::tool_call(*id, "read", json!({})));
    }
    Message::Assistant {
        content: blocks,
        api: "faux".into(),
        provider: "faux".into(),
        model: "faux-1".into(),
        usage: Usage {
            total_tokens: usage,
            ..Usage::default()
        },
        stop_reason: stop,
        error_message: None,
        timestamp: 0,
        extra: Map::new(),
    }
}

pub fn assistant(content: &str, calls: &[&str]) -> Message {
    assistant_with(content, calls, StopReason::Stop, 0)
}

pub fn tool_result(call_id: &str, content: &str) -> Message {
    Message::ToolResult {
        tool_call_id: call_id.into(),
        tool_name: "read".into(),
        content: vec![Block::text(content)],
        is_error: false,
        timestamp: 0,
        extra: Map::new(),
    }
}

pub fn entry(id: EntryId, kind: EntryKind) -> Entry {
    Entry {
        id,
        at_ms: id as i64 * 10,
        run: None,
        retired: false,
        kind,
    }
}

pub fn message(id: EntryId, message: Message) -> Entry {
    entry(
        id,
        EntryKind::Message {
            message,
            mode: InputMode::Prompt,
            request: None,
            partial: false,
        },
    )
}

pub fn compaction(id: EntryId, summary: &str, first_kept: EntryId) -> Entry {
    entry(
        id,
        EntryKind::Compaction {
            summary: summary.into(),
            first_kept,
            tokens_before: 0,
        },
    )
}

pub fn notice(id: EntryId) -> Entry {
    entry(
        id,
        EntryKind::Notice {
            level: NoticeLevel::Info,
            text: "note".into(),
        },
    )
}

/// `role:text` (results as `result:<id>:<text>`), like the reference's `describeMessage`.
pub fn describe(message: &Message) -> String {
    match message {
        Message::User { .. } => format!("user:{}", message.text()),
        Message::Assistant { .. } => format!("assistant:{}", message.text()),
        Message::ToolResult {
            tool_call_id,
            is_error,
            ..
        } => {
            if *is_error {
                format!("result:{tool_call_id}:error")
            } else {
                format!("result:{tool_call_id}:{}", message.text())
            }
        }
    }
}
