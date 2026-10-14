//! The saved copy of a conversation's transcript, for reading and searching without the engine.
//!
//! A copy is the shown items as JSON, bounded in count and size, and the text of its messages
//! for full-text search. Decoding is total: an item that cannot be understood (a newer build
//! wrote it, or the file was edited) is skipped, never a reason to lose the rest.

use pipkin_core::{
    Attachment, BackgroundSource, Delivery, ItemId, ItemKind, NoticeLevel, ToolCall, ToolStatus,
    TranscriptItem,
};
use serde_json::{Value, json};

/// Most items a saved copy keeps; the newest are kept.
pub const MAX_ITEMS: usize = 400;
/// Largest saved copy, in bytes of JSON; the oldest items are dropped to fit.
pub const MAX_BYTES: usize = 2 * 1024 * 1024;
/// Most text of one message that is searchable.
pub const MAX_SEARCH_TEXT: usize = 20 * 1024;

fn encode_item(item: &TranscriptItem) -> Value {
    let (kind, body) = match &item.kind {
        ItemKind::User {
            text,
            attachments,
            steer,
            ..
        } => (
            "user",
            json!({
                "text": text,
                "steer": steer,
                "attachments": attachments.iter().map(|a| json!({
                    "path": a.path, "name": a.name, "size": a.size,
                })).collect::<Vec<_>>(),
            }),
        ),
        ItemKind::Background { text, source } => (
            "background",
            json!({ "text": text, "source": match source {
                BackgroundSource::Job => "job",
                BackgroundSource::Subagent => "subagent",
            }}),
        ),
        ItemKind::Assistant { text, .. } => ("assistant", json!({ "text": text })),
        ItemKind::Tool(t) => (
            "tool",
            json!({
                "call_id": t.call_id, "name": t.name, "input": t.input, "output": t.output,
                "truncated": t.truncated, "full_len": t.full_len,
                "status": match t.status {
                    ToolStatus::Running => "running",
                    ToolStatus::Ok => "ok",
                    ToolStatus::Failed => "failed",
                },
            }),
        ),
        ItemKind::Notice { text, level } => (
            "notice",
            json!({ "text": text, "error": *level == NoticeLevel::Error }),
        ),
    };
    json!({ "id": item.id.0, "at": item.at, "native_only": item.native_only, "kind": kind, "body": body })
}

fn decode_item(value: &Value) -> Option<TranscriptItem> {
    let id = ItemId(value.get("id")?.as_u64()?);
    let at = value.get("at")?.as_i64()?;
    let body = value.get("body")?;
    let text = || body.get("text").and_then(Value::as_str).map(str::to_owned);
    let kind = match value.get("kind")?.as_str()? {
        "user" => ItemKind::User {
            text: text()?,
            attachments: body
                .get("attachments")?
                .as_array()?
                .iter()
                .filter_map(|a| {
                    Some(Attachment {
                        path: a.get("path")?.as_str()?.to_owned(),
                        name: a.get("name")?.as_str()?.to_owned(),
                        size: a.get("size").and_then(Value::as_u64),
                        error: None,
                    })
                })
                .collect(),
            // What the engine had when it was copied.
            delivery: Delivery::Sent,
            steer: body.get("steer").and_then(Value::as_bool).unwrap_or(false),
        },
        "background" => ItemKind::Background {
            text: text()?,
            source: match body.get("source")?.as_str()? {
                "job" => BackgroundSource::Job,
                "subagent" => BackgroundSource::Subagent,
                _ => return None,
            },
        },
        "assistant" => ItemKind::Assistant {
            text: text()?,
            streaming: false,
        },
        "tool" => ItemKind::Tool(ToolCall {
            call_ref: None,
            call_id: body
                .get("call_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            name: body.get("name")?.as_str()?.to_owned(),
            input: body.get("input")?.as_str()?.to_owned(),
            output: body.get("output")?.as_str()?.to_owned(),
            truncated: body.get("truncated")?.as_bool()?,
            full_len: body.get("full_len")?.as_u64()? as usize,
            // A copy never shows a tool as still running: that was true when it was taken.
            status: match body.get("status")?.as_str()? {
                "failed" => ToolStatus::Failed,
                _ => ToolStatus::Ok,
            },
        }),
        "notice" => ItemKind::Notice {
            text: text()?,
            level: if body.get("error").and_then(Value::as_bool).unwrap_or(false) {
                NoticeLevel::Error
            } else {
                NoticeLevel::Info
            },
        },
        _ => return None,
    };
    Some(TranscriptItem {
        native_only: value
            .get("native_only")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        id,
        at,
        kind,
    })
}

/// The newest items that fit the bounds, as JSON, and how many were kept.
pub fn encode_items(items: &[TranscriptItem]) -> (String, usize) {
    let start = items.len().saturating_sub(MAX_ITEMS);
    let mut encoded: Vec<String> = items[start..]
        .iter()
        .map(|i| encode_item(i).to_string())
        .collect();
    let mut bytes: usize = encoded.iter().map(|e| e.len() + 1).sum();
    let mut drop = 0;
    while bytes > MAX_BYTES && drop + 1 < encoded.len() {
        bytes -= encoded[drop].len() + 1;
        drop += 1;
    }
    encoded.drain(..drop);
    (format!("[{}]", encoded.join(",")), encoded.len())
}

pub fn decode_items(json: &str) -> Vec<TranscriptItem> {
    serde_json::from_str::<Value>(json)
        .ok()
        .and_then(|v| {
            v.as_array()
                .map(|a| a.iter().filter_map(decode_item).collect())
        })
        .unwrap_or_default()
}

/// The text of a message worth searching: what a person said or the model answered.
pub fn search_text(item: &TranscriptItem) -> Option<String> {
    let text = match &item.kind {
        ItemKind::User { text, .. }
        | ItemKind::Background { text, .. }
        | ItemKind::Assistant { text, .. } => text,
        _ => return None,
    };
    if text.trim().is_empty() {
        return None;
    }
    let mut end = text.len().min(MAX_SEARCH_TEXT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Some(text[..end].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_provenance_survives_offline_copy_and_search() {
        let items: Vec<_> = [BackgroundSource::Job, BackgroundSource::Subagent]
            .into_iter()
            .enumerate()
            .map(|(i, source)| TranscriptItem {
                native_only: true,
                id: ItemId(i as u64 + 1),
                at: 42,
                kind: ItemKind::Background {
                    text: "completion text".into(),
                    source,
                },
            })
            .collect();
        let (json, count) = encode_items(&items);
        assert_eq!(count, 2);
        assert_eq!(decode_items(&json), items);
        assert_eq!(search_text(&items[0]).as_deref(), Some("completion text"));
    }

    #[test]
    fn native_only_history_labels_survive_offline_copies_without_changing_text() {
        let mut original = items();
        for item in &mut original[1..] {
            item.native_only = true;
        }
        let encoded = encode_items(&original).0;
        let decoded = decode_items(&encoded);
        assert!(!decoded[0].native_only);
        assert!(decoded[1..].iter().all(|item| item.native_only));
        for (a, b) in original.iter().zip(&decoded) {
            assert_eq!(a.id, b.id);
            assert_eq!(search_text(a), search_text(b));
        }
        let mut legacy: Value = serde_json::from_str(&encoded).unwrap();
        for item in legacy.as_array_mut().unwrap() {
            item.as_object_mut().unwrap().remove("native_only");
        }
        assert!(
            decode_items(&legacy.to_string())
                .iter()
                .all(|item| !item.native_only)
        );
    }

    fn items() -> Vec<TranscriptItem> {
        vec![
            TranscriptItem {
                native_only: false,
                id: ItemId(1024),
                at: 10,
                kind: ItemKind::User {
                    text: "fix the \u{1F680} bug".into(),
                    attachments: vec![Attachment {
                        path: "/p/a.txt".into(),
                        name: "a.txt".into(),
                        size: Some(5),
                        error: Some("ignored".into()),
                    }],
                    delivery: Delivery::Unknown,
                    steer: true,
                },
            },
            TranscriptItem {
                native_only: false,
                id: ItemId(2048),
                at: 11,
                kind: ItemKind::Assistant {
                    text: "done".into(),
                    streaming: true,
                },
            },
            TranscriptItem {
                native_only: false,
                id: ItemId(3072),
                at: 12,
                kind: ItemKind::Tool(ToolCall {
                    call_ref: Some((pipkin_core::OperationId(3), 1)),
                    call_id: Some("call_1".into()),
                    name: "bash".into(),
                    input: "{\"command\":\"ls\"}".into(),
                    output: "a\nb".into(),
                    truncated: true,
                    full_len: 99_999,
                    status: ToolStatus::Running,
                }),
            },
            TranscriptItem {
                native_only: false,
                id: ItemId(4096),
                at: 13,
                kind: ItemKind::Notice {
                    text: "boom".into(),
                    level: NoticeLevel::Error,
                },
            },
        ]
    }

    #[test]
    fn a_copy_reads_back_as_what_the_engine_had() {
        let (json, kept) = encode_items(&items());
        assert_eq!(kept, 4);
        let back = decode_items(&json);
        assert_eq!(back.len(), 4);
        assert_eq!(back[0].id, ItemId(1024));
        assert!(
            matches!(&back[0].kind, ItemKind::User { text, delivery: Delivery::Sent, steer: true, attachments }
            if text == "fix the \u{1F680} bug" && attachments.len() == 1 && attachments[0].error.is_none()
                && attachments[0].size == Some(5))
        );
        assert!(
            matches!(&back[1].kind, ItemKind::Assistant { text, streaming: false } if text == "done")
        );
        let ItemKind::Tool(t) = &back[2].kind else {
            panic!()
        };
        assert_eq!(t.call_id.as_deref(), Some("call_1"));
        assert_eq!(
            (t.truncated, t.full_len, t.status),
            (true, 99_999, ToolStatus::Ok)
        );
        assert!(
            t.call_ref.is_none(),
            "an operation of an earlier run means nothing now"
        );
        assert!(matches!(
            &back[3].kind,
            ItemKind::Notice {
                level: NoticeLevel::Error,
                ..
            }
        ));
    }

    #[test]
    fn the_newest_items_are_kept_within_the_count_and_size_bounds() {
        let many: Vec<_> = (0..MAX_ITEMS as u64 + 50)
            .map(|n| TranscriptItem {
                native_only: false,
                id: ItemId(n * 1024),
                at: n as i64,
                kind: ItemKind::Assistant {
                    text: format!("m{n}"),
                    streaming: false,
                },
            })
            .collect();
        let (json, kept) = encode_items(&many);
        assert_eq!(kept, MAX_ITEMS);
        let back = decode_items(&json);
        assert_eq!(back.first().unwrap().id, ItemId(50 * 1024));
        assert_eq!(
            back.last().unwrap().id,
            ItemId((MAX_ITEMS as u64 + 49) * 1024)
        );

        // Three 1 MB messages cannot all fit in 2 MB: the oldest goes.
        let big = |n: u64| TranscriptItem {
            native_only: false,
            id: ItemId(n),
            at: 0,
            kind: ItemKind::Assistant {
                text: "x".repeat(1024 * 1024),
                streaming: false,
            },
        };
        let (json, kept) = encode_items(&[big(1), big(2), big(3)]);
        assert_eq!(kept, 1);
        assert!(json.len() <= MAX_BYTES + 1024);
        assert_eq!(decode_items(&json)[0].id, ItemId(3));
    }

    #[test]
    fn what_cannot_be_read_is_skipped_not_fatal() {
        let good = encode_item(&items()[1]).to_string();
        let json = format!(
            "[{good}, {{\"id\":1}}, {{\"id\":2,\"at\":0,\"kind\":\"hologram\",\"body\":{{}}}}, 7, null, {good}]"
        );
        assert_eq!(decode_items(&json).len(), 2);
        assert!(decode_items("not json").is_empty());
        assert!(decode_items("{}").is_empty());
    }

    #[test]
    fn only_what_people_said_is_searchable_and_it_is_bounded() {
        let it = items();
        assert!(search_text(&it[0]).is_some() && search_text(&it[1]).is_some());
        assert!(search_text(&it[2]).is_none() && search_text(&it[3]).is_none());
        let long = TranscriptItem {
            native_only: false,
            id: ItemId(1),
            at: 0,
            kind: ItemKind::Assistant {
                text: "é".repeat(MAX_SEARCH_TEXT),
                streaming: false,
            },
        };
        let t = search_text(&long).unwrap();
        assert!(t.len() <= MAX_SEARCH_TEXT && t.chars().all(|c| c == 'é'));
        let blank = TranscriptItem {
            native_only: false,
            id: ItemId(1),
            at: 0,
            kind: ItemKind::User {
                text: "  \n".into(),
                attachments: vec![],
                delivery: Delivery::Sent,
                steer: false,
            },
        };
        assert!(search_text(&blank).is_none());
    }
}
