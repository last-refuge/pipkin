//! Map Pi's replicated `ConversationView` onto Pipkin's transcript items.
//!
//! Pure and total: the input is data from another process, so no shape may panic, and nothing
//! unrecognised is silently dropped or passed off as assistant text; it becomes a visible
//! notice. Item ids are derived from engine entry ids, so the same entry keeps the same id
//! across remaps and the transcript can reconcile instead of rebuilding.

use std::collections::HashMap;

use pipkin_core::{
    Attachment, Delivery, ItemId, ItemKind, LOCAL_ITEM_BASE, NoticeLevel, QueueId, QueueMode,
    QueuedPrompt, SessionUsage, ToolCall, ToolStatus, TranscriptItem, UsageAmount, preview_output,
};
use serde_json::Value;

use super::attach::split_message;

/// Slots reserved per entry for the items one entry may produce.
const SLOTS_PER_ENTRY: u64 = 1024;
/// Live (not yet durable) items live below the local-item range, above any entry-derived id.
const LIVE_BASE: u64 = LOCAL_ITEM_BASE / 2;

/// Read committed ledger totals, not estimates reconstructed from the visible message page.
/// An invalid ledger is unavailable rather than a plausible-looking partial total.
pub fn usage(view: &Value, session_id: &str) -> Option<SessionUsage> {
    let doc = view.get("docs")?.get("pi.usage")?;
    let bucket = |name: &str| -> Option<Vec<(String, UsageAmount)>> {
        let mut entries: Vec<_> = doc
            .get(name)?
            .as_object()?
            .iter()
            .map(|(key, value)| {
                let count = |field| value.get(field)?.as_u64();
                let optional_count = |field| match value.get(field) {
                    None => Some(None),
                    Some(v) => v.as_u64().map(Some),
                };
                let cost = value
                    .get("cost")
                    .and_then(|c| c.get("total"))
                    .and_then(Value::as_f64)
                    .filter(|n| n.is_finite() && *n >= 0.0);
                Some((
                    key.clone(),
                    UsageAmount {
                        input: count("input")?,
                        output: count("output")?,
                        cache_read: count("cacheRead")?,
                        cache_write: count("cacheWrite")?,
                        cache_write_1h: optional_count("cacheWrite1h")?,
                        reasoning: optional_count("reasoning")?,
                        total_tokens: count("totalTokens")?,
                        cost_usd: cost,
                    },
                ))
            })
            .collect::<Option<_>>()?;
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        Some(entries)
    };
    Some(SessionUsage {
        session_id: session_id.to_owned(),
        models: bucket("models")?,
        tools: bucket("tools")?,
    })
}

#[derive(Debug, Default, PartialEq)]
pub struct Mapped {
    pub items: Vec<TranscriptItem>,
    /// A run is in progress (`pi.live.run` is present).
    pub busy: bool,
    /// Steers and follow-ups the engine holds for the next boundary (`pi.inbox`), oldest first.
    pub queue: Vec<QueuedPrompt>,
    /// Entries shown as "unsupported" notices.
    pub unsupported: usize,
}

struct Builder {
    items: Vec<TranscriptItem>,
    tools: HashMap<String, usize>,
    unsupported: usize,
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .map(|b| match b.get("type").and_then(Value::as_str) {
                Some("text") => b
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                Some("image") => "[image]".to_owned(),
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// The text a person typed in a user message's content, and how many images came with it.
fn user_parts(content: &Value) -> (String, usize) {
    match content {
        Value::Array(blocks) => {
            let mut text = String::new();
            let mut images = 0;
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        text.push_str(block.get("text").and_then(Value::as_str).unwrap_or(""))
                    }
                    Some("image") => images += 1,
                    _ => {}
                }
            }
            (text, images)
        }
        other => (text_of(other), 0),
    }
}

/// Attachment chips for a user message: files recorded in marked blocks, then images.
fn chips_for(content: &Value) -> (String, Vec<Attachment>) {
    let (raw, images) = user_parts(content);
    let (text, mut chips) = split_message(&raw);
    for n in 1..=images {
        chips.push(Attachment {
            path: String::new(),
            name: if images == 1 {
                "Image".to_owned()
            } else {
                format!("Image {n}")
            },
            size: None,
            error: None,
        });
    }
    (text, chips)
}

fn queued(view: &Value) -> Vec<QueuedPrompt> {
    let items = view
        .get("docs")
        .and_then(|d| d.get("pi.inbox"))
        .and_then(|i| i.get("items"))
        .and_then(Value::as_array);
    items
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let mode = match item.get("mode").and_then(Value::as_str)? {
                "steer" => QueueMode::Steer,
                "followUp" => QueueMode::FollowUp,
                // Passive entry writes are the engine's own business.
                _ => return None,
            };
            let id = item.get("id").and_then(Value::as_u64)?;
            let (text, chips) = chips_for(item.get("content").unwrap_or(&Value::Null));
            let text = match chips.len() {
                0 => text,
                1 => format!("{text} [1 attachment]"),
                n => format!("{text} [{n} attachments]"),
            };
            Some(QueuedPrompt {
                id: QueueId(id),
                text,
                mode,
            })
        })
        .collect()
}

fn entry_base(entry: &Value) -> u64 {
    let raw = match entry.get("id") {
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
        Some(Value::String(s)) => s.parse::<u64>().unwrap_or_else(|_| fnv(s)),
        _ => 0,
    };
    // Keep every derived id below the live range.
    (raw % (LIVE_BASE / SLOTS_PER_ENTRY)) * SLOTS_PER_ENTRY
}

fn fnv(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

fn seconds(message: &Value) -> i64 {
    message
        .get("timestamp")
        .and_then(Value::as_i64)
        .map_or(0, |ms| ms / 1000)
}

impl Builder {
    fn push(&mut self, id: u64, at: i64, kind: ItemKind) -> usize {
        self.items.push(TranscriptItem {
            native_only: false,
            id: ItemId(id),
            at,
            kind,
        });
        self.items.len() - 1
    }

    fn notice(&mut self, id: u64, at: i64, level: NoticeLevel, text: String) {
        self.push(id, at, ItemKind::Notice { text, level });
    }

    fn tool(&mut self, id: u64, at: i64, call_id: &str, name: &str, input: String) {
        let index = self.push(
            id,
            at,
            ItemKind::Tool(ToolCall {
                call_ref: None,
                call_id: Some(call_id.to_owned()),
                name: name.to_owned(),
                input,
                output: String::new(),
                truncated: false,
                full_len: 0,
                status: ToolStatus::Running,
            }),
        );
        self.tools.insert(call_id.to_owned(), index);
    }

    fn set_tool_output(&mut self, call_id: &str, output: &str, status: ToolStatus) -> bool {
        let Some(&index) = self.tools.get(call_id) else {
            return false;
        };
        if let ItemKind::Tool(tool) = &mut self.items[index].kind {
            let (preview, truncated) = preview_output(output);
            tool.output = preview;
            tool.truncated = truncated;
            tool.full_len = output.len();
            tool.status = status;
        }
        true
    }

    /// Items for one assistant message's content blocks, in order.
    fn assistant(&mut self, base: u64, at: i64, message: &Value, streaming: bool) {
        let mut slot = 0u64;
        let next = |slot: &mut u64| {
            let id = base + (*slot).min(SLOTS_PER_ENTRY - 1);
            *slot += 1;
            id
        };
        let mut text = String::new();
        let mut text_id = None;
        let flush =
            |b: &mut Builder, text: &mut String, text_id: &mut Option<u64>, streaming: bool| {
                if let Some(id) = text_id.take() {
                    b.push(
                        id,
                        at,
                        ItemKind::Assistant {
                            text: std::mem::take(text),
                            streaming,
                        },
                    );
                }
            };
        let blocks = message
            .get("content")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for block in &blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if text_id.is_none() {
                        text_id = Some(next(&mut slot));
                    }
                    text.push_str(block.get("text").and_then(Value::as_str).unwrap_or(""));
                }
                Some("thinking") => {
                    flush(self, &mut text, &mut text_id, false);
                    let id = next(&mut slot);
                    let redacted = block
                        .get("redacted")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let body = block.get("thinking").and_then(Value::as_str).unwrap_or("");
                    let label = if redacted || body.is_empty() {
                        "Thinking...".to_owned()
                    } else {
                        format!("Thinking\n{body}")
                    };
                    self.notice(id, at, NoticeLevel::Info, label);
                }
                Some("toolCall") => {
                    flush(self, &mut text, &mut text_id, false);
                    let id = next(&mut slot);
                    let call_id = block.get("id").and_then(Value::as_str).unwrap_or("");
                    let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
                    let input = block
                        .get("arguments")
                        .map(|a| serde_json::to_string(a).unwrap_or_default())
                        .unwrap_or_default();
                    self.tool(id, at, call_id, name, input);
                }
                other => {
                    flush(self, &mut text, &mut text_id, false);
                    let id = next(&mut slot);
                    self.unsupported += 1;
                    self.notice(
                        id,
                        at,
                        NoticeLevel::Info,
                        format!("[unsupported content: {}]", other.unwrap_or("unknown")),
                    );
                }
            }
        }
        // Only the final text block of a streaming partial is still growing.
        flush(self, &mut text, &mut text_id, streaming);
        if message.get("stopReason").and_then(Value::as_str) == Some("error") {
            let id = next(&mut slot);
            let reason = message
                .get("errorMessage")
                .and_then(Value::as_str)
                .unwrap_or("The model run ended with an error.");
            self.notice(id, at, NoticeLevel::Error, reason.to_owned());
        }
    }

    fn message(&mut self, base: u64, index: u64, message: &Value) {
        let at = seconds(message);
        let id = base + index.min(SLOTS_PER_ENTRY - 1);
        match message.get("role").and_then(Value::as_str) {
            Some("user") => {
                let (text, attachments) = chips_for(message.get("content").unwrap_or(&Value::Null));
                self.push(
                    id,
                    at,
                    ItemKind::User {
                        text,
                        attachments,
                        delivery: Delivery::Sent,
                        steer: false,
                    },
                );
            }
            // Instructions and tool definitions for the model, not conversation.
            Some("system") => {}
            Some("assistant") => self.assistant(base, at, message, false),
            Some("toolResult") => {
                let call_id = message
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let output = text_of(message.get("content").unwrap_or(&Value::Null));
                let status = if message
                    .get("isError")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    ToolStatus::Failed
                } else {
                    ToolStatus::Ok
                };
                if !self.set_tool_output(call_id, &output, status) {
                    // A result whose call is not visible (compacted away): keep it, labelled.
                    let name = message
                        .get("toolName")
                        .and_then(Value::as_str)
                        .unwrap_or("tool");
                    self.tool(id, at, call_id, name, String::new());
                    self.set_tool_output(call_id, &output, status);
                }
            }
            other => {
                self.unsupported += 1;
                self.notice(
                    id,
                    at,
                    NoticeLevel::Info,
                    format!("[unsupported message role: {}]", other.unwrap_or("unknown")),
                );
            }
        }
    }
}

/// The engine's entry an item was made from, or `None` for an item that is not from an entry
/// (a live partial, or one made locally).
pub fn entry_of_item(item: ItemId) -> Option<u64> {
    (item.0 < LIVE_BASE).then_some(item.0 / SLOTS_PER_ENTRY)
}

/// The oldest content entry in a view (head markers carry no content).
pub fn oldest_entry_id(view: &Value) -> Option<u64> {
    view.get("entries")?
        .as_array()?
        .iter()
        .filter(|e| e.is_object() && e.get("head").is_none())
        .filter_map(|e| e.get("id").and_then(Value::as_u64))
        .min()
}

/// Items for a page of history, which the engine serves newest first.
pub fn map_history_page(entries_newest_first: &[Value]) -> Mapped {
    let oldest_first: Vec<Value> = entries_newest_first.iter().rev().cloned().collect();
    map_view(&serde_json::json!({ "entries": oldest_first, "docs": {} }))
}

/// The complete result text of the tool call `call_id`, from entries in any order or from a
/// view's live tool slots.
pub fn tool_result_text(entries: &[Value], call_id: &str) -> Option<String> {
    entries.iter().find_map(|entry| {
        entry
            .get("model")?
            .as_array()?
            .iter()
            .find(|m| {
                m.get("role").and_then(Value::as_str) == Some("toolResult")
                    && m.get("toolCallId").and_then(Value::as_str) == Some(call_id)
            })
            .map(|m| text_of(m.get("content").unwrap_or(&Value::Null)))
    })
}

/// Like [`tool_result_text`], for a whole view: its entries, then its running tools.
pub fn tool_result_in_view(view: &Value, call_id: &str) -> Option<String> {
    let entries = view.get("entries").and_then(Value::as_array)?;
    tool_result_text(entries, call_id).or_else(|| {
        view.get("docs")?
            .get("pi.live")?
            .get("tools")?
            .as_array()?
            .iter()
            .find(|s| s.get("callId").and_then(Value::as_str) == Some(call_id))
            .and_then(|s| s.get("output").and_then(Value::as_str).map(str::to_owned))
    })
}

/// Map a `ConversationView` value.
pub fn map_view(view: &Value) -> Mapped {
    let mut b = Builder {
        items: Vec::new(),
        tools: HashMap::new(),
        unsupported: 0,
    };
    let empty = Vec::new();
    let entries = view
        .get("entries")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    for entry in entries {
        // A head marker only selects which entries are active; it carries no content.
        if entry.get("head").is_some() || !entry.is_object() {
            continue;
        }
        let base = entry_base(entry);
        match entry.get("model").and_then(Value::as_array) {
            Some(messages) if !messages.is_empty() => {
                for (i, message) in messages.iter().enumerate() {
                    b.message(base, i as u64, message);
                }
            }
            _ => {
                if entry.get("data").is_some() {
                    let kind = entry
                        .get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    b.unsupported += 1;
                    b.notice(base, 0, NoticeLevel::Info, format!("[{kind}]"));
                }
            }
        }
    }

    let live = view.get("docs").and_then(|d| d.get("pi.live"));
    let busy = live
        .and_then(|l| l.get("run"))
        .is_some_and(|r| !r.is_null());
    if let Some(generation) = live.and_then(|l| l.get("generation"))
        && let Some(partial) = generation.get("message")
        && partial.is_object()
    {
        b.assistant(LIVE_BASE, 0, partial, true);
    }
    if let Some(slots) = live.and_then(|l| l.get("tools")).and_then(Value::as_array) {
        for (i, slot) in slots.iter().enumerate() {
            let call_id = slot.get("callId").and_then(Value::as_str).unwrap_or("");
            let name = slot.get("name").and_then(Value::as_str).unwrap_or("tool");
            if !b.tools.contains_key(call_id) {
                b.tool(LIVE_BASE + 512 + i as u64, 0, call_id, name, String::new());
            }
            let output = slot.get("output").and_then(Value::as_str).unwrap_or("");
            match slot.get("status").and_then(Value::as_str) {
                // A finished slot with a result entry is already represented by that entry.
                Some("done") if slot.get("entry").is_some() => {}
                // Done with no result entry: the tool faulted or was orphaned.
                Some("done") => {
                    b.set_tool_output(call_id, output, ToolStatus::Failed);
                }
                _ => {
                    b.set_tool_output(call_id, output, ToolStatus::Running);
                }
            }
        }
    }
    Mapped {
        items: b.items,
        busy,
        queue: queued(view),
        unsupported: b.unsupported,
    }
}

#[cfg(test)]
mod usage_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn committed_ledger_is_exact_and_includes_tool_spend() {
        let view = json!({ "docs": { "pi.usage": {
            "models": { "z/model": { "input": 100, "output": 20, "cacheRead": 50, "cacheWrite": 10,
                "cacheWrite1h": 5, "reasoning": 7, "totalTokens": 180, "cost": { "total": 0.12 } } },
            "tools": { "classifier": { "input": 8, "output": 2, "cacheRead": 0, "cacheWrite": 0,
                "totalTokens": 10, "cost": { "total": 0.01 } } }
        } } });
        let parsed = usage(&view, "pi-session").unwrap();
        assert_eq!(parsed.session_id, "pi-session");
        assert_eq!(parsed.total().total_tokens, 190);
        assert_eq!(parsed.total().cache_read, 50);
        assert_eq!(parsed.total().cache_write_1h, Some(5));
        assert_eq!(parsed.total().reasoning, Some(7));
        assert_eq!(parsed.total().cost_usd, Some(0.13));
    }

    #[test]
    fn unavailable_is_not_zero_and_missing_cost_is_not_invented() {
        assert!(usage(&json!({ "docs": {} }), "s").is_none());
        let view = json!({ "docs": { "pi.usage": {
            "models": { "p/m": { "input": 2, "output": 3, "cacheRead": 0, "cacheWrite": 0,
                "totalTokens": 5 } }, "tools": {}
        } } });
        let parsed = usage(&view, "s").unwrap();
        assert_eq!(parsed.total().total_tokens, 5);
        assert_eq!(parsed.total().cost_usd, None);
        assert_eq!(parsed.total().reasoning, None);
        assert_eq!(parsed.total().cache_write_1h, None);
        let mut malformed = view.clone();
        malformed["docs"]["pi.usage"]["models"]["p/m"]["reasoning"] = json!("3");
        assert!(usage(&malformed, "s").is_none());
        let invalid = json!({ "docs": { "pi.usage": { "models": { "p/m": { "input": "2" } }, "tools": {} } } });
        assert!(usage(&invalid, "s").is_none());
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn user(id: u64, text: &str) -> Value {
        json!({ "id": id, "conversationId": 1, "kind": "pi.message",
                "model": [{ "role": "user", "content": text, "timestamp": 1_700_000_000_000i64 }] })
    }

    fn assistant(id: u64, content: Value) -> Value {
        json!({ "id": id, "conversationId": 1, "kind": "pi.message",
                "model": [{ "role": "assistant", "content": content, "stopReason": "stop", "timestamp": 1_700_000_005_000i64 }] })
    }

    fn view(entries: Vec<Value>) -> Value {
        json!({ "conversation": { "id": 1 }, "entries": entries, "docs": {} })
    }

    fn kinds(m: &Mapped) -> Vec<&ItemKind> {
        m.items.iter().map(|i| &i.kind).collect()
    }

    #[test]
    fn maps_user_and_assistant_text() {
        let m = map_view(&view(vec![
            user(1, "fix the bug"),
            assistant(
                2,
                json!([{ "type": "text", "text": "Looking " }, { "type": "text", "text": "now." }]),
            ),
        ]));
        assert_eq!(m.items.len(), 2);
        assert!(
            matches!(&m.items[0].kind, ItemKind::User { text, delivery: Delivery::Sent, .. } if text == "fix the bug")
        );
        assert_eq!(m.items[0].at, 1_700_000_000);
        assert!(
            matches!(&m.items[1].kind, ItemKind::Assistant { text, streaming: false } if text == "Looking now.")
        );
        assert!(!m.busy);
    }

    #[test]
    fn user_content_blocks_keep_text_and_mark_images() {
        let entry = json!({ "id": 1, "kind": "k", "model": [{ "role": "user", "timestamp": 0,
            "content": [{ "type": "text", "text": "see " }, { "type": "image", "data": "AAAA", "mimeType": "image/png" }] }] });
        let m = map_view(&view(vec![entry]));
        let ItemKind::User {
            text, attachments, ..
        } = &m.items[0].kind
        else {
            panic!("a user item");
        };
        assert_eq!(text, "see ");
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].name, "Image");
    }

    #[test]
    fn an_inbox_becomes_the_queue_and_ignores_passive_writes() {
        let mut v = view(vec![]);
        v["docs"] = json!({ "pi.inbox": { "items": [
            { "id": 11, "mode": "steer", "content": "use tabs" },
            { "id": 12, "mode": "write", "entry": { "kind": "x" } },
            { "id": 13, "mode": "followUp", "content": [
                { "type": "text", "text": "then ship it" },
                { "type": "image", "data": "AA", "mimeType": "image/png" } ] },
            { "mode": "followUp", "content": "no id: skipped" },
            { "id": 14, "mode": "mystery", "content": "skipped" },
        ]}});
        let m = map_view(&v);
        assert_eq!(
            m.queue,
            vec![
                QueuedPrompt {
                    id: QueueId(11),
                    text: "use tabs".into(),
                    mode: QueueMode::Steer
                },
                QueuedPrompt {
                    id: QueueId(13),
                    text: "then ship it [1 attachment]".into(),
                    mode: QueueMode::FollowUp
                },
            ]
        );
        assert!(map_view(&view(vec![])).queue.is_empty());
    }

    #[test]
    fn tool_calls_pair_with_their_results() {
        let m = map_view(&view(vec![
            assistant(
                2,
                json!([
                    { "type": "text", "text": "Reading." },
                    { "type": "toolCall", "id": "c1", "name": "read_file", "arguments": { "path": "a.rs" } },
                    { "type": "toolCall", "id": "c2", "name": "bash", "arguments": { "cmd": "false" } },
                ]),
            ),
            json!({ "id": 3, "kind": "pi.message", "model": [{ "role": "toolResult", "toolCallId": "c1", "toolName": "read_file",
                "content": [{ "type": "text", "text": "fn main() {}" }], "isError": false, "timestamp": 0 }] }),
            json!({ "id": 4, "kind": "pi.message", "model": [{ "role": "toolResult", "toolCallId": "c2", "toolName": "bash",
                "content": [{ "type": "text", "text": "exit 1" }], "isError": true, "timestamp": 0 }] }),
        ]));
        let tools: Vec<&ToolCall> = m
            .items
            .iter()
            .filter_map(|i| {
                if let ItemKind::Tool(t) = &i.kind {
                    Some(t)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(tools.len(), 2);
        assert_eq!(
            (tools[0].name.as_str(), tools[0].status),
            ("read_file", ToolStatus::Ok)
        );
        assert_eq!(tools[0].output, "fn main() {}");
        assert_eq!(tools[0].input, r#"{"path":"a.rs"}"#);
        assert_eq!(
            (tools[1].name.as_str(), tools[1].status),
            ("bash", ToolStatus::Failed)
        );
        // A result adds no item of its own when its call is visible.
        assert_eq!(m.items.len(), 3);
    }

    #[test]
    fn an_unmatched_tool_result_is_kept_and_labelled() {
        let m = map_view(&view(vec![
            json!({ "id": 9, "kind": "k", "model": [{ "role": "toolResult",
            "toolCallId": "gone", "toolName": "bash", "content": "late", "isError": false, "timestamp": 0 }] }),
        ]));
        assert!(
            matches!(&m.items[0].kind, ItemKind::Tool(t) if t.name == "bash" && t.output == "late")
        );
    }

    #[test]
    fn thinking_is_visible_and_redacted_thinking_is_brief() {
        let m = map_view(&view(vec![assistant(
            2,
            json!([
                { "type": "thinking", "thinking": "consider the cache" },
                { "type": "thinking", "thinking": "", "redacted": true, "thinkingSignature": "opaque" },
                { "type": "text", "text": "Done." },
            ]),
        )]));
        assert!(
            matches!(&m.items[0].kind, ItemKind::Notice { text, level: NoticeLevel::Info } if text == "Thinking\nconsider the cache")
        );
        assert!(matches!(&m.items[1].kind, ItemKind::Notice { text, .. } if text == "Thinking..."));
        assert!(matches!(&m.items[2].kind, ItemKind::Assistant { text, .. } if text == "Done."));
    }

    #[test]
    fn text_split_by_a_tool_call_becomes_two_messages() {
        let m = map_view(&view(vec![assistant(
            2,
            json!([
                { "type": "text", "text": "before" },
                { "type": "toolCall", "id": "c", "name": "t", "arguments": {} },
                { "type": "text", "text": "after" },
            ]),
        )]));
        let shape: Vec<&str> = kinds(&m)
            .iter()
            .map(|k| match k {
                ItemKind::Assistant { .. } => "assistant",
                ItemKind::Tool(_) => "tool",
                _ => "other",
            })
            .collect();
        assert_eq!(shape, ["assistant", "tool", "assistant"]);
    }

    #[test]
    fn an_errored_run_shows_its_reason() {
        let entry = json!({ "id": 2, "kind": "k", "model": [{ "role": "assistant", "content": [], "stopReason": "error",
            "errorMessage": "provider unavailable", "timestamp": 0 }] });
        let m = map_view(&view(vec![entry]));
        assert!(
            matches!(&m.items[0].kind, ItemKind::Notice { text, level: NoticeLevel::Error } if text == "provider unavailable")
        );
    }

    #[test]
    fn head_markers_are_skipped_and_system_messages_are_not_conversation() {
        let marker = json!({ "id": 5, "kind": "pi.head", "head": 3, "data": {} });
        let system = json!({ "id": 1, "kind": "k", "model": [{ "role": "system", "content": "be terse", "timestamp": 0 }] });
        let m = map_view(&view(vec![marker, system, user(6, "hi")]));
        assert_eq!(m.items.len(), 1);
        assert_eq!(m.unsupported, 0);
    }

    #[test]
    fn unknown_entries_and_content_stay_visible() {
        let odd = json!({ "id": 7, "kind": "pi.compaction", "data": { "summary": "x" } });
        let block = assistant(8, json!([{ "type": "hologram" }]));
        let role = json!({ "id": 9, "kind": "k", "model": [{ "role": "oracle", "timestamp": 0 }] });
        let m = map_view(&view(vec![odd, block, role]));
        assert_eq!(m.items.len(), 3);
        assert_eq!(m.unsupported, 3);
        assert!(
            matches!(&m.items[0].kind, ItemKind::Notice { text, .. } if text == "[pi.compaction]")
        );
        assert!(
            matches!(&m.items[1].kind, ItemKind::Notice { text, .. } if text.contains("hologram"))
        );
        assert!(
            matches!(&m.items[2].kind, ItemKind::Notice { text, .. } if text.contains("oracle"))
        );
    }

    #[test]
    fn large_tool_output_is_bounded_but_reports_its_size() {
        let big = "x".repeat(40_000);
        let m = map_view(&view(vec![
            assistant(
                2,
                json!([{ "type": "toolCall", "id": "c", "name": "bash", "arguments": {} }]),
            ),
            json!({ "id": 3, "kind": "k", "model": [{ "role": "toolResult", "toolCallId": "c", "toolName": "bash",
                "content": [{ "type": "text", "text": big }], "isError": false, "timestamp": 0 }] }),
        ]));
        let ItemKind::Tool(t) = &m.items[0].kind else {
            panic!()
        };
        assert!(
            t.truncated
                && t.full_len == 40_000
                && t.output.len() <= pipkin_core::TOOL_OUTPUT_PREVIEW_BYTES
        );
    }

    #[test]
    fn live_partial_streams_as_the_last_item() {
        let mut v = view(vec![user(1, "go")]);
        v["docs"] = json!({ "pi.live": {
            "run": { "taskId": 1, "inputs": [1] },
            "generation": { "attempt": 1, "message": { "role": "assistant", "stopReason": "stop", "timestamp": 0,
                "content": [{ "type": "text", "text": "Working on" }] } },
        }});
        let m = map_view(&v);
        assert!(m.busy);
        assert_eq!(m.items.len(), 2);
        assert!(
            matches!(&m.items[1].kind, ItemKind::Assistant { text, streaming: true } if text == "Working on")
        );
        assert!(m.items[1].id.0 >= LIVE_BASE && m.items[1].id.0 < LOCAL_ITEM_BASE);
    }

    #[test]
    fn running_tool_slots_show_progress_and_faulted_slots_show_failure() {
        let mut v = view(vec![assistant(
            2,
            json!([
                { "type": "toolCall", "id": "run", "name": "bash", "arguments": {} },
                { "type": "toolCall", "id": "dead", "name": "grep", "arguments": {} },
                { "type": "toolCall", "id": "fin", "name": "ls", "arguments": {} },
            ]),
        )]);
        v["docs"] = json!({ "pi.live": { "tools": [
            { "callId": "run", "name": "bash", "status": "running", "output": "building…" },
            { "callId": "dead", "name": "grep", "status": "done" },
            { "callId": "fin", "name": "ls", "status": "done", "entry": 9 },
        ]}});
        let m = map_view(&v);
        let tool = |n: &str| -> &ToolCall {
            m.items
                .iter()
                .find_map(|i| match &i.kind {
                    ItemKind::Tool(t) if t.name == n => Some(t),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(
            (tool("bash").status, tool("bash").output.as_str()),
            (ToolStatus::Running, "building…")
        );
        assert_eq!(tool("grep").status, ToolStatus::Failed);
        // Finished with a result entry that is not in this view yet: stays as the call shows.
        assert_eq!(tool("ls").status, ToolStatus::Running);
    }

    #[test]
    fn a_tool_slot_without_a_visible_call_still_appears() {
        let mut v = view(vec![]);
        v["docs"] = json!({ "pi.live": { "tools": [{ "callId": "x", "name": "bash", "status": "running", "output": "hi" }] }});
        let m = map_view(&v);
        assert!(matches!(&m.items[0].kind, ItemKind::Tool(t) if t.output == "hi"));
    }

    #[test]
    fn ids_are_stable_across_remaps_and_unique_within_a_transcript() {
        let entries = vec![
            user(10, "a"),
            assistant(
                11,
                json!([{ "type": "text", "text": "x" }, { "type": "toolCall", "id": "c", "name": "t", "arguments": {} }]),
            ),
            user(12, "b"),
        ];
        let first = map_view(&view(entries.clone()));
        let second = map_view(&view(entries));
        assert_eq!(first, second);
        let mut ids: Vec<u64> = first.items.iter().map(|i| i.id.0).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), first.items.len());
        assert!(ids.iter().all(|id| *id < LOCAL_ITEM_BASE));
        // Appending entries never renumbers earlier items.
        let more = map_view(&view(vec![
            user(10, "a"),
            assistant(11, json!([{ "type": "text", "text": "x" }])),
            user(12, "b"),
            user(13, "c"),
        ]));
        assert_eq!(more.items[0].id, first.items[0].id);
    }

    #[test]
    fn hostile_and_malformed_shapes_never_panic() {
        for v in [
            json!(null),
            json!([]),
            json!("view"),
            json!({ "entries": "nope" }),
            json!({ "entries": [null, 1, "x", [], {}] }),
            json!({ "entries": [{ "id": "not-a-number", "model": [null, 3, "m", { "role": 7 }, { "role": "user" }] }] }),
            json!({ "entries": [{ "id": u64::MAX, "model": [{ "role": "user", "content": { "x": 1 } }] }] }),
            json!({ "entries": [], "docs": { "pi.live": { "tools": [null, 1, { "status": 5 }], "generation": { "message": 3 } } } }),
            json!({ "entries": [{ "id": 1, "model": [{ "role": "assistant", "content": [null, 1, { "type": 3 }, { "type": "toolCall" }] }] }] }),
        ] {
            let _ = map_view(&v);
        }
    }

    #[test]
    fn a_huge_entry_id_stays_in_range() {
        let m = map_view(&view(vec![user(u64::MAX, "x")]));
        assert!(m.items[0].id.0 < LOCAL_ITEM_BASE);
    }
}

/// The mapper against REAL `ConversationView` values captured from Pi's durable harness with
/// scripted faux responses (`scripts/capture-pi-views.ts`). These pin the shapes the engine
/// actually produces, as opposed to the hand-written ones above.
#[cfg(test)]
mod real_views {
    use super::*;

    fn fixture(name: &str) -> Mapped {
        let raw = match name {
            "text" => include_str!("../../../../../fixtures/pi/text.json"),
            "thinking" => include_str!("../../../../../fixtures/pi/thinking.json"),
            "tool-call" => include_str!("../../../../../fixtures/pi/tool-call.json"),
            "error" => include_str!("../../../../../fixtures/pi/error.json"),
            "multi-turn" => include_str!("../../../../../fixtures/pi/multi-turn.json"),
            "busy" => include_str!("../../../../../fixtures/pi/busy.json"),
            other => panic!("unknown fixture {other}"),
        };
        map_view(&serde_json::from_str(raw).expect("fixture is JSON"))
    }

    const ALL: [&str; 6] = [
        "text",
        "thinking",
        "tool-call",
        "error",
        "multi-turn",
        "busy",
    ];

    #[test]
    fn a_plain_exchange_maps_to_a_user_and_an_assistant_message() {
        let m = fixture("text");
        assert_eq!(m.items.len(), 2);
        assert!(
            matches!(&m.items[0].kind, ItemKind::User { text, delivery: Delivery::Sent, .. } if text == "hello")
        );
        assert!(
            matches!(&m.items[1].kind, ItemKind::Assistant { text, streaming: false } if text == "hello back")
        );
        assert!(
            m.items[0].at > 1_700_000_000,
            "timestamps are converted from milliseconds"
        );
        assert!(!m.busy);
    }

    #[test]
    fn thinking_is_shown_before_the_answer() {
        let m = fixture("thinking");
        assert_eq!(m.items.len(), 3, "{:?}", m.items);
        assert!(
            matches!(&m.items[1].kind, ItemKind::Notice { text, .. } if text == "Thinking\nweigh the options")
        );
        assert!(
            matches!(&m.items[2].kind, ItemKind::Assistant { text, .. } if text == "Here is my answer.")
        );
    }

    #[test]
    fn a_tool_call_pairs_with_its_real_result() {
        let m = fixture("tool-call");
        let shape: Vec<&str> = m
            .items
            .iter()
            .map(|i| match &i.kind {
                ItemKind::User { .. } => "user",
                ItemKind::Assistant { .. } => "assistant",
                ItemKind::Tool(_) => "tool",
                ItemKind::Notice { .. } => "notice",
            })
            .collect();
        assert_eq!(shape, ["user", "assistant", "tool", "assistant"]);
        let ItemKind::Tool(tool) = &m.items[2].kind else {
            panic!()
        };
        assert_eq!(tool.name, "read_file");
        assert_eq!(tool.input, r#"{"path":"a.rs"}"#);
        // The harness reported the tool as unavailable, which is a real error result.
        assert_eq!(tool.status, ToolStatus::Failed);
        assert!(
            tool.output.contains("Tool read_file is not available"),
            "{}",
            tool.output
        );
        assert!(
            matches!(&m.items[3].kind, ItemKind::Assistant { text, .. } if text == "Done reading.")
        );
    }

    #[test]
    fn a_model_error_shows_its_reason() {
        let m = fixture("error");
        assert_eq!(m.items.len(), 2);
        assert!(
            matches!(&m.items[1].kind, ItemKind::Notice { text, level: NoticeLevel::Error } if text == "provider unavailable")
        );
    }

    #[test]
    fn several_turns_keep_their_order() {
        let m = fixture("multi-turn");
        let texts: Vec<&str> = m
            .items
            .iter()
            .map(|i| match &i.kind {
                ItemKind::User { text, .. } | ItemKind::Assistant { text, .. } => text.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(texts, ["one", "first reply", "two", "second reply"]);
    }

    #[test]
    fn a_run_in_progress_is_busy_without_inventing_content() {
        let m = fixture("busy");
        assert!(m.busy);
        assert_eq!(
            m.items.len(),
            1,
            "only the prompt; no partial answer has arrived"
        );
        assert!(matches!(&m.items[0].kind, ItemKind::User { text, .. } if text == "work on it"));
    }

    #[test]
    fn nothing_real_is_reported_as_unsupported_and_ids_are_sound() {
        for name in ALL {
            let m = fixture(name);
            assert_eq!(m.unsupported, 0, "{name}: {:?}", m.items);
            let mut ids: Vec<u64> = m.items.iter().map(|i| i.id.0).collect();
            let sorted = ids.clone();
            ids.sort_unstable();
            assert_eq!(ids, sorted, "{name}: items follow entry order");
            ids.dedup();
            assert_eq!(ids.len(), m.items.len(), "{name}: ids are unique");
            assert!(ids.iter().all(|id| *id < LOCAL_ITEM_BASE), "{name}");
        }
    }
}
