//! Map the engine's typed history entries onto Pipkin's transcript items.
//!
//! The same rules and the same item ids as the Pi mapping (`adapters/pi/transcript.rs`): an
//! entry's items are numbered `entry id * 1024 + slot`, so a conversation imported from Pi keeps
//! the item ids its saved copy and search hits refer to. Tool calls pair with their results,
//! output is bounded by `preview_output`, attachment blocks become chips, redacted thinking is
//! shown only as "Thinking...", and nothing unrecognised is silently dropped or passed off as
//! assistant text. Retired entries (a stopped instruction and its partial reply) are still shown:
//! they happened, they just no longer reach the model.

use std::collections::HashMap;

use pipkin_core::{
    Attachment, Delivery, ItemId, ItemKind, LOCAL_ITEM_BASE, NoticeLevel, QueueId, QueueMode,
    QueuedPrompt, SessionUsage, ToolCall, ToolStatus, TranscriptItem, UsageAmount, preview_output,
};
use pipkin_engine_protocol::{
    Block, Entry, EntryKind, InputMode, Message, QueueEntryView, StopReason, UsageView,
};

use crate::adapters::pi::attach::split_message;

/// Slots reserved per entry for the items one entry may produce (as in the Pi mapping).
const SLOTS_PER_ENTRY: u64 = 1024;
/// Live (not yet durable) items live below the local-item range, above any entry-derived id.
const LIVE_BASE: u64 = LOCAL_ITEM_BASE / 2;

/// The first item id of an entry: identical to the Pi mapping's for a numeric entry id.
fn entry_base(id: u64) -> u64 {
    (id % (LIVE_BASE / SLOTS_PER_ENTRY)) * SLOTS_PER_ENTRY
}

/// The engine entry an item was made from, or `None` for an item not from an entry.
pub fn entry_of_item(item: ItemId) -> Option<u64> {
    (item.0 < LIVE_BASE).then_some(item.0 / SLOTS_PER_ENTRY)
}

fn seconds(ms: i64) -> i64 {
    ms / 1000
}

#[derive(Debug, Default, PartialEq)]
pub struct Mapped {
    pub items: Vec<TranscriptItem>,
    /// Entries shown as "unsupported" notices.
    pub unsupported: usize,
}

struct Builder<'a> {
    items: Vec<TranscriptItem>,
    tools: HashMap<String, usize>,
    unsupported: usize,
    progress: &'a HashMap<String, String>,
}

/// Attachment chips for a user message: files recorded in marked blocks, then images.
fn chips_for(content: &[Block]) -> (String, Vec<Attachment>) {
    let mut raw = String::new();
    let mut images = 0;
    for block in content {
        match block {
            Block::Text { text, .. } => raw.push_str(text),
            Block::Image { .. } => images += 1,
            _ => {}
        }
    }
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

/// Plain text of tool-result content; images become a marker.
fn result_text(content: &[Block]) -> String {
    content
        .iter()
        .map(|b| match b {
            Block::Text { text, .. } => text.clone(),
            Block::Image { .. } => "[image]".to_owned(),
            _ => String::new(),
        })
        .collect()
}

impl Builder<'_> {
    fn push(&mut self, id: u64, at: i64, kind: ItemKind) -> usize {
        self.items.push(TranscriptItem {
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
        // Output streamed so far, until the result entry replaces it.
        if let Some(progress) = self.progress.get(call_id).cloned() {
            self.set_tool_output(call_id, &progress, ToolStatus::Running);
        }
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
    fn assistant(&mut self, base: u64, at: i64, message: &Message, streaming: bool) {
        let Message::Assistant {
            content,
            stop_reason,
            error_message,
            ..
        } = message
        else {
            return;
        };
        let mut slot = 0u64;
        let next = |slot: &mut u64| {
            let id = base + (*slot).min(SLOTS_PER_ENTRY - 1);
            *slot += 1;
            id
        };
        let mut text = String::new();
        let mut text_id = None;
        let flush = |b: &mut Self, text: &mut String, text_id: &mut Option<u64>, streaming| {
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
        for block in content {
            match block {
                Block::Text { text: t, .. } => {
                    if text_id.is_none() {
                        text_id = Some(next(&mut slot));
                    }
                    text.push_str(t);
                }
                Block::Thinking {
                    thinking, redacted, ..
                } => {
                    flush(self, &mut text, &mut text_id, false);
                    let id = next(&mut slot);
                    let label = if *redacted || thinking.is_empty() {
                        "Thinking...".to_owned()
                    } else {
                        format!("Thinking\n{thinking}")
                    };
                    self.notice(id, at, NoticeLevel::Info, label);
                }
                Block::ToolCall {
                    id: call_id,
                    name,
                    arguments,
                    ..
                } => {
                    flush(self, &mut text, &mut text_id, false);
                    let id = next(&mut slot);
                    let input = serde_json::to_string(arguments).unwrap_or_default();
                    self.tool(id, at, call_id, name, input);
                }
                Block::Image { .. } => {
                    flush(self, &mut text, &mut text_id, false);
                    let id = next(&mut slot);
                    self.unsupported += 1;
                    self.notice(
                        id,
                        at,
                        NoticeLevel::Info,
                        "[unsupported content: image]".into(),
                    );
                }
            }
        }
        // Only the final text block of a streaming partial is still growing.
        flush(self, &mut text, &mut text_id, streaming);
        if *stop_reason == StopReason::Error {
            let id = next(&mut slot);
            let reason = error_message
                .clone()
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| "The model run ended with an error.".to_owned());
            self.notice(id, at, NoticeLevel::Error, reason);
        }
    }

    fn message(
        &mut self,
        base: u64,
        entry: &Entry,
        message: &Message,
        mode: InputMode,
        partial: bool,
    ) {
        let at = seconds(match message.timestamp() {
            0 => entry.at_ms,
            ts => ts,
        });
        match message {
            Message::User { content, .. } => {
                let (text, attachments) = chips_for(content);
                self.push(
                    base,
                    at,
                    ItemKind::User {
                        text,
                        attachments,
                        delivery: Delivery::Sent,
                        steer: mode == InputMode::Steer,
                    },
                );
            }
            Message::Assistant { .. } => self.assistant(base, at, message, partial),
            Message::ToolResult {
                tool_call_id,
                tool_name,
                content,
                is_error,
                ..
            } => {
                let output = result_text(content);
                let status = if *is_error {
                    ToolStatus::Failed
                } else {
                    ToolStatus::Ok
                };
                if !self.set_tool_output(tool_call_id, &output, status) {
                    // A result whose call is not visible (compacted or paged away): keep it.
                    self.tool(base, at, tool_call_id, tool_name, String::new());
                    self.set_tool_output(tool_call_id, &output, status);
                }
            }
        }
    }

    fn entry(&mut self, entry: &Entry) {
        let base = entry_base(entry.id);
        let at = seconds(entry.at_ms);
        match &entry.kind {
            EntryKind::Message {
                message,
                mode,
                partial,
                ..
            } => self.message(base, entry, message, *mode, *partial),
            EntryKind::Compaction { .. } => self.notice(
                base,
                at,
                NoticeLevel::Info,
                "Earlier messages were summarized to keep the conversation within the model's context."
                    .into(),
            ),
            EntryKind::Reset { .. } => self.notice(
                base,
                at,
                NoticeLevel::Info,
                "The model's context was reset here; earlier messages are no longer sent to it."
                    .into(),
            ),
            // The transcript has two levels; a warning is said in words.
            EntryKind::Notice { level, text } => match level {
                pipkin_engine_protocol::NoticeLevel::Info => {
                    self.notice(base, at, NoticeLevel::Info, text.clone())
                }
                pipkin_engine_protocol::NoticeLevel::Warning => {
                    self.notice(base, at, NoticeLevel::Info, format!("Warning: {text}"))
                }
                pipkin_engine_protocol::NoticeLevel::Error => {
                    self.notice(base, at, NoticeLevel::Error, text.clone())
                }
            },
            // Settings changes are shown by the model and effort controls, not in the history.
            EntryKind::ModelChange { .. } | EntryKind::ThinkingChange { .. } => {}
            // Preserved but uninterpreted: say it is there, as the Pi mapping does.
            EntryKind::Custom { custom_type, .. } => {
                self.unsupported += 1;
                self.notice(base, at, NoticeLevel::Info, format!("[{custom_type}]"));
            }
        }
    }
}

/// Map entries (oldest first). `progress` holds streamed output of tool calls still running.
pub fn map_entries(entries: &[Entry], progress: &HashMap<String, String>) -> Mapped {
    let mut b = Builder {
        items: Vec::new(),
        tools: HashMap::new(),
        unsupported: 0,
        progress,
    };
    for entry in entries {
        b.entry(entry);
    }
    Mapped {
        items: b.items,
        unsupported: b.unsupported,
    }
}

/// The complete result text of `call_id` among `entries`, if its result entry is there.
pub fn tool_result_text(entries: &[Entry], call_id: &str) -> Option<String> {
    entries.iter().find_map(|entry| match &entry.kind {
        EntryKind::Message {
            message:
                Message::ToolResult {
                    tool_call_id,
                    content,
                    ..
                },
            ..
        } if tool_call_id == call_id => Some(result_text(content)),
        _ => None,
    })
}

/// The engine's queue as the core shows it, oldest first.
pub fn queue(views: &[QueueEntryView]) -> Vec<QueuedPrompt> {
    views
        .iter()
        .filter_map(|q| {
            let mode = match q.mode {
                InputMode::Steer => QueueMode::Steer,
                InputMode::FollowUp => QueueMode::FollowUp,
                // A plain prompt is never queued; skip anything else rather than mislabel it.
                InputMode::Prompt => return None,
            };
            let (text, chips) = split_message(&q.text);
            let attachments = chips.len() + q.images as usize;
            let text = match attachments {
                0 => text,
                1 => format!("{text} [1 attachment]"),
                n => format!("{text} [{n} attachments]"),
            };
            Some(QueuedPrompt {
                id: QueueId(q.entry),
                text,
                mode,
            })
        })
        .collect()
}

/// The committed ledger, sorted by key. Cost is reported per bucket by the engine.
pub fn usage(view: &UsageView, session_id: &str) -> SessionUsage {
    let bucket = |rows: &[(String, pipkin_engine_protocol::Usage)]| {
        let mut out: Vec<(String, UsageAmount)> = rows
            .iter()
            .map(|(key, u)| {
                (
                    key.clone(),
                    UsageAmount {
                        input: u.input,
                        output: u.output,
                        cache_read: u.cache_read,
                        cache_write: u.cache_write,
                        cache_write_1h: u.cache_write_1h,
                        reasoning: u.reasoning,
                        total_tokens: u.total_tokens,
                        cost_usd: Some(u.cost.total).filter(|c| c.is_finite() && *c >= 0.0),
                    },
                )
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    };
    SessionUsage {
        session_id: session_id.to_owned(),
        models: bucket(&view.models),
        tools: bucket(&view.tools),
    }
}

#[cfg(test)]
mod tests {
    use pipkin_engine_protocol::{Cost, Usage};
    use serde_json::{Map, json};

    use super::*;

    pub fn entry(id: u64, kind: EntryKind) -> Entry {
        Entry {
            id,
            at_ms: 1_700_000_000_000,
            run: None,
            retired: false,
            kind,
        }
    }

    fn message(id: u64, message: Message) -> Entry {
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

    fn user(id: u64, text: &str) -> Entry {
        message(id, Message::user_text(text, 1_700_000_000_000))
    }

    fn assistant(id: u64, content: Vec<Block>) -> Entry {
        message(
            id,
            Message::Assistant {
                content,
                api: "a".into(),
                provider: "p".into(),
                model: "m".into(),
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 1_700_000_005_000,
                extra: Map::new(),
            },
        )
    }

    fn result(id: u64, call: &str, name: &str, text: &str, is_error: bool) -> Entry {
        message(
            id,
            Message::ToolResult {
                tool_call_id: call.into(),
                tool_name: name.into(),
                content: vec![Block::text(text)],
                is_error,
                timestamp: 0,
                extra: Map::new(),
            },
        )
    }

    fn thinking(text: &str, redacted: bool) -> Block {
        Block::Thinking {
            thinking: text.into(),
            signature: None,
            redacted,
            extra: Map::new(),
        }
    }

    fn map(entries: &[Entry]) -> Mapped {
        map_entries(entries, &HashMap::new())
    }

    /// The same history as a Pi `ConversationView`, for id comparison.
    fn pi_view(entries: &[Entry]) -> serde_json::Value {
        let rows: Vec<_> = entries
            .iter()
            .map(|e| match &e.kind {
                EntryKind::Message { message, .. } => {
                    json!({ "id": e.id, "kind": "pi.message", "model": [message] })
                }
                _ => json!({ "id": e.id }),
            })
            .collect();
        json!({ "entries": rows, "docs": {} })
    }

    #[test]
    fn item_ids_match_the_pi_mapping_for_the_same_history() {
        let history = vec![
            user(10, "fix it"),
            assistant(
                11,
                vec![
                    thinking("hm", false),
                    Block::text("Reading."),
                    Block::tool_call("c1", "read", json!({ "path": "a.rs" })),
                    Block::tool_call("c2", "bash", json!({ "cmd": "false" })),
                ],
            ),
            result(12, "c1", "read", "fn main() {}", false),
            result(13, "c2", "bash", "exit 1", true),
            assistant(14, vec![Block::text("Done.")]),
            user(15, "thanks"),
        ];
        let native = map(&history);
        let pi = crate::adapters::pi::transcript::map_view(&pi_view(&history));
        assert_eq!(native.items, pi.items);
        assert_eq!(native.unsupported, 0);
    }

    #[test]
    fn tool_calls_pair_with_results_and_unmatched_results_are_kept() {
        let m = map(&[
            assistant(2, vec![Block::tool_call("c", "bash", json!({}))]),
            result(3, "c", "bash", "ok", false),
            result(9, "gone", "grep", "late", false),
        ]);
        assert_eq!(m.items.len(), 2);
        assert!(
            matches!(&m.items[0].kind, ItemKind::Tool(t) if t.status == ToolStatus::Ok && t.output == "ok")
        );
        assert!(
            matches!(&m.items[1].kind, ItemKind::Tool(t) if t.name == "grep" && t.output == "late")
        );
        assert_eq!(m.items[1].id, ItemId(9 * SLOTS_PER_ENTRY));
    }

    #[test]
    fn running_tools_show_streamed_progress_until_their_result() {
        let mut progress = HashMap::new();
        progress.insert("c".to_owned(), "building…".to_owned());
        let calls = vec![assistant(2, vec![Block::tool_call("c", "bash", json!({}))])];
        let m = map_entries(&calls, &progress);
        assert!(matches!(&m.items[0].kind, ItemKind::Tool(t)
            if t.status == ToolStatus::Running && t.output == "building…"));
        let mut done = calls.clone();
        done.push(result(3, "c", "bash", "built", false));
        let m = map_entries(&done, &progress);
        assert!(matches!(&m.items[0].kind, ItemKind::Tool(t)
            if t.status == ToolStatus::Ok && t.output == "built"));
    }

    #[test]
    fn large_output_is_bounded_and_reports_its_size() {
        let big = "x".repeat(40_000);
        let m = map(&[
            assistant(2, vec![Block::tool_call("c", "bash", json!({}))]),
            result(3, "c", "bash", &big, false),
        ]);
        let ItemKind::Tool(t) = &m.items[0].kind else {
            panic!()
        };
        assert!(t.truncated && t.full_len == 40_000);
        assert!(t.output.len() <= pipkin_core::TOOL_OUTPUT_PREVIEW_BYTES);
    }

    #[test]
    fn thinking_is_labelled_and_redacted_thinking_is_hidden() {
        let m = map(&[assistant(
            2,
            vec![
                thinking("weigh it", false),
                thinking("secret", true),
                Block::text("Answer"),
            ],
        )]);
        assert!(
            matches!(&m.items[0].kind, ItemKind::Notice { text, .. } if text == "Thinking\nweigh it")
        );
        assert!(matches!(&m.items[1].kind, ItemKind::Notice { text, .. } if text == "Thinking..."));
        assert!(
            matches!(&m.items[2].kind, ItemKind::Assistant { text, streaming: false } if text == "Answer")
        );
    }

    #[test]
    fn a_partial_message_streams_and_an_error_shows_its_reason() {
        let mut partial = assistant(4, vec![Block::text("Work")]);
        if let EntryKind::Message { partial: p, .. } = &mut partial.kind {
            *p = true;
        }
        let m = map(&[partial]);
        assert!(
            matches!(&m.items[0].kind, ItemKind::Assistant { text, streaming: true } if text == "Work")
        );
        let errored = message(
            5,
            Message::Assistant {
                content: vec![],
                api: String::new(),
                provider: String::new(),
                model: String::new(),
                usage: Usage::default(),
                stop_reason: StopReason::Error,
                error_message: Some("provider unavailable".into()),
                timestamp: 0,
                extra: Map::new(),
            },
        );
        let m = map(&[errored]);
        assert!(matches!(&m.items[0].kind,
            ItemKind::Notice { text, level: NoticeLevel::Error } if text == "provider unavailable"));
        assert_eq!(m.items[0].at, 1_700_000_000, "falls back to the entry time");
    }

    #[test]
    fn user_messages_get_attachment_chips_and_steers_are_marked() {
        let text = "see\n\n<attached-file name=\"a.txt\" path=\"/p/a.txt\" bytes=\"2\">\nhi\n</attached-file>";
        let mut e = message(
            1,
            Message::User {
                content: vec![Block::text(text), Block::image("image/png", "AA==")],
                timestamp: 0,
                extra: Map::new(),
            },
        );
        if let EntryKind::Message { mode, .. } = &mut e.kind {
            *mode = InputMode::Steer;
        }
        let m = map(&[e]);
        let ItemKind::User {
            text,
            attachments,
            steer,
            ..
        } = &m.items[0].kind
        else {
            panic!()
        };
        assert_eq!(text, "see");
        assert_eq!(attachments.len(), 2);
        assert_eq!(attachments[0].name, "a.txt");
        assert_eq!(attachments[1].name, "Image");
        assert!(*steer);
    }

    #[test]
    fn notices_and_boundaries_are_visible_and_settings_changes_are_not() {
        let m = map(&[
            entry(
                1,
                EntryKind::Notice {
                    level: pipkin_engine_protocol::NoticeLevel::Warning,
                    text: "retrying".into(),
                },
            ),
            entry(
                2,
                EntryKind::Compaction {
                    summary: "s".into(),
                    first_kept: 1,
                    tokens_before: 10,
                },
            ),
            entry(3, EntryKind::Reset { handoff: None }),
            entry(
                4,
                EntryKind::ModelChange {
                    model: "a/b".into(),
                },
            ),
            entry(
                5,
                EntryKind::ThinkingChange {
                    level: "high".into(),
                },
            ),
            entry(
                6,
                EntryKind::Custom {
                    custom_type: "ext.state".into(),
                    data: json!({}),
                },
            ),
        ]);
        assert_eq!(m.items.len(), 4);
        assert!(matches!(&m.items[0].kind,
            ItemKind::Notice { text, level: NoticeLevel::Info } if text == "Warning: retrying"));
        assert!(matches!(&m.items[3].kind, ItemKind::Notice { text, .. } if text == "[ext.state]"));
        assert_eq!(m.unsupported, 1);
    }

    #[test]
    fn retired_entries_are_still_shown() {
        let mut stopped = user(7, "do the wrong thing");
        stopped.retired = true;
        let m = map(&[stopped]);
        assert!(
            matches!(&m.items[0].kind, ItemKind::User { text, .. } if text == "do the wrong thing")
        );
    }

    #[test]
    fn ids_stay_in_range_and_round_trip_to_their_entry() {
        let m = map(&[user(u64::MAX, "x"), user(42, "y")]);
        assert!(m.items.iter().all(|i| i.id.0 < LIVE_BASE));
        assert_eq!(entry_of_item(m.items[1].id), Some(42));
        assert_eq!(entry_of_item(ItemId(LIVE_BASE)), None);
    }

    #[test]
    fn queue_and_usage_map_from_the_engine_views() {
        let q = queue(&[
            QueueEntryView {
                entry: 3,
                request: "r1".into(),
                mode: InputMode::Steer,
                text: "use tabs".into(),
                images: 0,
            },
            QueueEntryView {
                entry: 4,
                request: "r2".into(),
                mode: InputMode::FollowUp,
                text: "ship it".into(),
                images: 2,
            },
            QueueEntryView {
                entry: 5,
                request: "r3".into(),
                mode: InputMode::Prompt,
                text: "never queued".into(),
                images: 0,
            },
        ]);
        assert_eq!(
            q,
            vec![
                QueuedPrompt {
                    id: QueueId(3),
                    text: "use tabs".into(),
                    mode: QueueMode::Steer
                },
                QueuedPrompt {
                    id: QueueId(4),
                    text: "ship it [2 attachments]".into(),
                    mode: QueueMode::FollowUp
                },
            ]
        );
        let u = usage(
            &UsageView {
                models: vec![
                    (
                        "z/m".into(),
                        Usage {
                            input: 100,
                            output: 20,
                            cache_read: 50,
                            cache_write: 10,
                            cache_write_1h: Some(5),
                            reasoning: Some(7),
                            total_tokens: 180,
                            cost: Cost {
                                total: 0.12,
                                ..Cost::default()
                            },
                        },
                    ),
                    ("a/m".into(), Usage::default()),
                ],
                tools: vec![(
                    "classifier".into(),
                    Usage {
                        total_tokens: 10,
                        cost: Cost {
                            total: 0.01,
                            ..Cost::default()
                        },
                        ..Usage::default()
                    },
                )],
            },
            "s1",
        );
        assert_eq!(u.session_id, "s1");
        assert_eq!(u.models[0].0, "a/m", "sorted by key");
        assert_eq!(u.total().total_tokens, 190);
        assert_eq!(u.total().cache_write_1h, Some(5));
        assert_eq!(u.total().reasoning, Some(7));
        assert_eq!(u.total().cost_usd, Some(0.13));
    }

    #[test]
    fn tool_results_are_found_by_call_id() {
        let entries = [result(3, "c", "bash", "full output", false)];
        assert_eq!(
            tool_result_text(&entries, "c").as_deref(),
            Some("full output")
        );
        assert_eq!(tool_result_text(&entries, "other"), None);
    }
}
