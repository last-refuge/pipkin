//! Cross-provider replay normalization (reference `api/transform-messages.ts`) and the id
//! helpers the adapters' tool-call-id normalizers are built from (reference `utils/hash.ts`
//! `shortHash` and the `/[^a-zA-Z0-9_-]/g` sanitizers).

use std::collections::{HashMap, HashSet};

use pipkin_engine_protocol::{Block, Message, StopReason};
use serde_json::Map;

use crate::provider::model::Model;

pub const NON_VISION_USER_IMAGE_PLACEHOLDER: &str =
    "(image omitted: model does not support images)";
pub const NON_VISION_TOOL_IMAGE_PLACEHOLDER: &str =
    "(tool image omitted: model does not support images)";

/// Maps a tool call id from a different model to one the target API accepts. Receives the id
/// and the assistant message that made the call (reference `normalizeToolCallId(id, model,
/// source)`; the target model is captured by the closure). Stateful normalizers (Mistral's
/// collision-free ids) are `FnMut`.
pub type NormalizeToolCallId<'a> = &'a mut dyn FnMut(&str, &Message) -> String;

fn replace_images(content: &[Block], placeholder: &str) -> Vec<Block> {
    let mut out = Vec::with_capacity(content.len());
    let mut previous_was_placeholder = false;
    for block in content {
        if let Block::Image { .. } = block {
            if !previous_was_placeholder {
                out.push(Block::text(placeholder));
            }
            previous_was_placeholder = true;
            continue;
        }
        previous_was_placeholder = matches!(block, Block::Text { text, .. } if text == placeholder);
        out.push(block.clone());
    }
    out
}

/// Is this assistant message from exactly the target provider, API and model?
pub fn is_same_model(message: &Message, model: &Model) -> bool {
    matches!(message, Message::Assistant { provider, api, model: id, .. }
        if *provider == model.provider && *api == model.api && *id == model.id)
}

/// Normalize a context for replay to `model` (reference `transformMessages`,
/// `transform-messages.ts:64-235`):
///
/// - images become a placeholder when the model has no image input;
/// - assistant turns from a different model lose redacted thinking and provider signatures,
///   readable thinking becomes text, `thoughtSignature` is stripped from tool calls, and tool
///   call ids go through `normalize` (tool results follow the mapping);
/// - errored or aborted assistant turns are dropped;
/// - tool calls without a result get a synthetic error result before the next assistant or
///   user message (or at the end).
///
/// The engine context has no system messages (the system prompt is a flat string on the
/// request), so the reference's held-back system message handling never applies.
pub fn transform_messages(
    messages: &[Message],
    model: &Model,
    mut normalize: Option<NormalizeToolCallId<'_>>,
) -> Vec<Message> {
    let vision = model.supports_images();
    let mut id_map: HashMap<String, String> = HashMap::new();
    let mut transformed: Vec<Message> = Vec::with_capacity(messages.len());

    for msg in messages {
        match msg {
            Message::User {
                content,
                timestamp,
                extra,
            } => transformed.push(Message::User {
                content: if vision {
                    content.clone()
                } else {
                    replace_images(content, NON_VISION_USER_IMAGE_PLACEHOLDER)
                },
                timestamp: *timestamp,
                extra: extra.clone(),
            }),
            Message::ToolResult {
                tool_call_id,
                tool_name,
                content,
                is_error,
                timestamp,
                extra,
            } => transformed.push(Message::ToolResult {
                tool_call_id: id_map
                    .get(tool_call_id)
                    .cloned()
                    .unwrap_or_else(|| tool_call_id.clone()),
                tool_name: tool_name.clone(),
                content: if vision {
                    content.clone()
                } else {
                    replace_images(content, NON_VISION_TOOL_IMAGE_PLACEHOLDER)
                },
                is_error: *is_error,
                timestamp: *timestamp,
                extra: extra.clone(),
            }),
            Message::Assistant { content, .. } => {
                let same = is_same_model(msg, model);
                let mut out = Vec::with_capacity(content.len());
                for block in content {
                    match block {
                        Block::Thinking {
                            thinking,
                            signature,
                            redacted,
                            ..
                        } => {
                            if *redacted {
                                if same {
                                    out.push(block.clone());
                                }
                                continue;
                            }
                            if same && signature.as_deref().is_some_and(|s| !s.is_empty()) {
                                out.push(block.clone());
                                continue;
                            }
                            if thinking.trim().is_empty() {
                                continue;
                            }
                            if same {
                                out.push(block.clone());
                            } else {
                                out.push(Block::text(thinking.clone()));
                            }
                        }
                        Block::Text { text, .. } => {
                            if same {
                                out.push(block.clone());
                            } else {
                                out.push(Block::text(text.clone()));
                            }
                        }
                        Block::ToolCall {
                            id,
                            name,
                            arguments,
                            extra,
                        } => {
                            let mut extra = extra.clone();
                            let mut new_id = id.clone();
                            if !same {
                                extra.remove("thoughtSignature");
                                if let Some(normalize) = normalize.as_mut() {
                                    let normalized = normalize(id, msg);
                                    if normalized != *id {
                                        id_map.insert(id.clone(), normalized.clone());
                                        new_id = normalized;
                                    }
                                }
                            }
                            out.push(Block::ToolCall {
                                id: new_id,
                                name: name.clone(),
                                arguments: arguments.clone(),
                                extra,
                            });
                        }
                        Block::Image { .. } => out.push(block.clone()),
                    }
                }
                let mut copy = msg.clone();
                if let Message::Assistant { content, .. } = &mut copy {
                    *content = out;
                }
                transformed.push(copy);
            }
        }
    }

    // Second pass: synthetic results for orphaned tool calls.
    let mut result: Vec<Message> = Vec::with_capacity(transformed.len());
    let mut pending: Vec<(String, String)> = Vec::new();
    let mut answered: HashSet<String> = HashSet::new();
    let close = |result: &mut Vec<Message>,
                 pending: &mut Vec<(String, String)>,
                 answered: &mut HashSet<String>| {
        for (id, name) in pending.drain(..) {
            if !answered.contains(&id) {
                result.push(Message::ToolResult {
                    tool_call_id: id,
                    tool_name: name,
                    content: vec![Block::text("No result provided")],
                    is_error: true,
                    timestamp: crate::util::now_ms(),
                    extra: Map::new(),
                });
            }
        }
        answered.clear();
    };
    for msg in transformed {
        match &msg {
            Message::Assistant {
                stop_reason,
                content,
                ..
            } => {
                close(&mut result, &mut pending, &mut answered);
                if matches!(stop_reason, StopReason::Error | StopReason::Aborted) {
                    continue;
                }
                let calls: Vec<(String, String)> = content
                    .iter()
                    .filter_map(|b| match b {
                        Block::ToolCall { id, name, .. } => Some((id.clone(), name.clone())),
                        _ => None,
                    })
                    .collect();
                if !calls.is_empty() {
                    pending = calls;
                    answered.clear();
                }
                result.push(msg);
            }
            Message::ToolResult { tool_call_id, .. } => {
                answered.insert(tool_call_id.clone());
                result.push(msg);
            }
            Message::User { .. } => {
                close(&mut result, &mut pending, &mut answered);
                result.push(msg);
            }
        }
    }
    close(&mut result, &mut pending, &mut answered);
    result
}

/// The reference `shortHash` (a 53-bit cyrb-style hash over UTF-16 code units, base 36).
/// Ids derived from it must match what the reference engine produced for the same input.
pub fn short_hash(s: &str) -> String {
    let mut h1: u32 = 0xdead_beef;
    let mut h2: u32 = 0x41c6_ce57;
    for ch in s.encode_utf16() {
        let ch = u32::from(ch);
        h1 = (h1 ^ ch).wrapping_mul(2_654_435_761);
        h2 = (h2 ^ ch).wrapping_mul(1_597_334_677);
    }
    h1 = (h1 ^ (h1 >> 16)).wrapping_mul(2_246_822_507)
        ^ (h2 ^ (h2 >> 13)).wrapping_mul(3_266_489_909);
    h2 = (h2 ^ (h2 >> 16)).wrapping_mul(2_246_822_507)
        ^ (h1 ^ (h1 >> 13)).wrapping_mul(3_266_489_909);
    format!("{}{}", base36(h2), base36(h1))
}

fn base36(mut n: u32) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// Replace every UTF-16 code unit outside `[A-Za-z0-9_-]` with `_` (the reference regex runs
/// without the `u` flag, so an astral character becomes two underscores).
pub fn sanitize_id_chars(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            out.push(c);
        } else {
            for _ in 0..c.len_utf16() {
                out.push('_');
            }
        }
    }
    out
}

/// First `n` characters (equal to UTF-16 units for the ASCII ids this is used on).
pub fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `id.replace(/[^a-zA-Z0-9_-]/g, "_").slice(0, 64)`: the tool call id normalizer of the
/// Anthropic, Bedrock and Google adapters.
pub fn sanitize_tool_call_id(id: &str) -> String {
    take_chars(&sanitize_id_chars(id), 64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn model(provider: &str, api: &str, id: &str, images: bool) -> Model {
        serde_json::from_value(json!({
            "id": id, "provider": provider, "api": api,
            "input": if images { json!(["text", "image"]) } else { json!(["text"]) }
        }))
        .unwrap()
    }

    fn assistant(
        provider: &str,
        api: &str,
        id: &str,
        content: Vec<Block>,
        stop: StopReason,
    ) -> Message {
        Message::Assistant {
            content,
            api: api.into(),
            provider: provider.into(),
            model: id.into(),
            usage: Default::default(),
            stop_reason: stop,
            error_message: None,
            timestamp: 1,
            extra: Map::new(),
        }
    }

    fn tool_result(id: &str, text: &str) -> Message {
        Message::ToolResult {
            tool_call_id: id.into(),
            tool_name: "read".into(),
            content: vec![Block::text(text)],
            is_error: false,
            timestamp: 2,
            extra: Map::new(),
        }
    }

    #[test]
    fn short_hash_matches_the_reference_values() {
        // Values produced by the reference implementation under Node.
        assert_eq!(short_hash(""), "k4n83c7h0j2b");
        assert_eq!(short_hash("abc"), "y0biex7f9bbh");
        assert_eq!(
            short_hash(&format!("call_abc|fc_{}", "x".repeat(100))),
            "15aeto34stmtr"
        );
        assert_eq!(short_hash("h\u{e9}llo \u{1f600}"), "1t90xbiinqdrp");
        assert_eq!(short_hash("item+/="), "xfoazco3biir");
        assert_eq!(short_hash("call_abc|fc_123"), "jp01dsxufnom");
        assert_eq!(short_hash("rs_foreign_item_id_0123456789"), "1fvsux1lx7hr4");
    }

    #[test]
    fn tool_call_ids_are_sanitized_per_utf16_unit() {
        assert_eq!(sanitize_tool_call_id("a|b.c"), "a_b_c");
        assert_eq!(sanitize_tool_call_id("\u{1f600}"), "__");
        assert_eq!(sanitize_tool_call_id(&"x".repeat(100)).len(), 64);
    }

    #[test]
    fn cross_model_replay_is_normalized_and_orphans_get_results() {
        let target = model("openai", "openai-completions", "gpt", false);
        let mut call_extra = Map::new();
        call_extra.insert("thoughtSignature".into(), json!("sig"));
        let mut text_extra = Map::new();
        text_extra.insert("textSignature".into(), json!("msg_1"));
        let messages = vec![
            Message::User {
                content: vec![
                    Block::text("look"),
                    Block::image("image/png", "AA"),
                    Block::image("image/png", "BB"),
                ],
                timestamp: 0,
                extra: Map::new(),
            },
            assistant(
                "anthropic",
                "anthropic-messages",
                "claude",
                vec![
                    Block::Thinking {
                        thinking: "hidden".into(),
                        signature: Some("x".into()),
                        redacted: true,
                        extra: Map::new(),
                    },
                    Block::Thinking {
                        thinking: "plan".into(),
                        signature: Some("s".into()),
                        redacted: false,
                        extra: Map::new(),
                    },
                    Block::Thinking {
                        thinking: "  ".into(),
                        signature: None,
                        redacted: false,
                        extra: Map::new(),
                    },
                    Block::Text {
                        text: "hi".into(),
                        extra: text_extra,
                    },
                    Block::ToolCall {
                        id: "a|b".into(),
                        name: "read".into(),
                        arguments: json!({}),
                        extra: call_extra,
                    },
                    Block::tool_call("orphan", "bash", json!({})),
                ],
                StopReason::ToolUse,
            ),
            tool_result("a|b", "ok"),
            assistant(
                "openai",
                "openai-completions",
                "gpt",
                vec![Block::text("partial")],
                StopReason::Error,
            ),
            Message::user_text("next", 3),
        ];
        let mut normalize = |id: &str, _: &Message| id.replace('|', "_");
        let out = transform_messages(&messages, &target, Some(&mut normalize));
        assert_eq!(out.len(), 5, "{out:#?}");
        assert_eq!(
            out[0].content(),
            &[
                Block::text("look"),
                Block::text(NON_VISION_USER_IMAGE_PLACEHOLDER)
            ]
        );
        let Message::Assistant { content, .. } = &out[1] else {
            panic!()
        };
        assert_eq!(content[0], Block::text("plan"));
        assert_eq!(content[1], Block::text("hi"));
        assert_eq!(content[2], Block::tool_call("a_b", "read", json!({})));
        assert_eq!(content.len(), 4);
        let Message::ToolResult { tool_call_id, .. } = &out[2] else {
            panic!()
        };
        assert_eq!(tool_call_id, "a_b");
        let Message::ToolResult {
            tool_call_id,
            is_error,
            content,
            ..
        } = &out[3]
        else {
            panic!()
        };
        assert_eq!(tool_call_id, "orphan");
        assert!(*is_error);
        assert_eq!(content, &vec![Block::text("No result provided")]);
        assert!(matches!(out[4], Message::User { .. }));
    }

    #[test]
    fn same_model_replay_keeps_signatures_and_redacted_reasoning() {
        let target = model("openai", "openai-responses", "gpt", true);
        let blocks = vec![
            Block::Thinking {
                thinking: String::new(),
                signature: Some("{\"type\":\"reasoning\"}".into()),
                redacted: false,
                extra: Map::new(),
            },
            Block::Thinking {
                thinking: "r".into(),
                signature: Some("p".into()),
                redacted: true,
                extra: Map::new(),
            },
        ];
        let messages = vec![assistant(
            "openai",
            "openai-responses",
            "gpt",
            blocks.clone(),
            StopReason::Stop,
        )];
        let out = transform_messages(&messages, &target, None);
        assert_eq!(out[0].content(), blocks.as_slice());
    }
}
