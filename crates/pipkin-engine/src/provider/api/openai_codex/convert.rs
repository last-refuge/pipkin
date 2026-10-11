//! Responses API input conversion for the Codex adapter.
//!
//! PRIVATE DUPLICATE: this is a port of the reference `api/transform-messages.ts`
//! (`transformMessages`) and `api/openai-responses-shared.ts` (`convertResponsesMessages`
//! :145-354, `convertResponsesTools` :360-397) written inside the Codex adapter so it does not
//! block on the shared `openai_responses_shared.rs` module being written concurrently. Once that
//! module lands, this file should be replaced by calls into it.
//!
//! Engine differences from the reference context model, all by construction:
//! - The system prompt is a flat string on the request (sent as `instructions`), so there are no
//!   system messages in the transcript, no mid-conversation system messages and no
//!   `toolsAdded` anchors (`additional_tools` / client `tool_search_*` items are never produced).
//! - `ToolSpec` has no `constrainedSampling`, so grammar (`custom`) tools and strict JSON-schema
//!   sampling never apply; every tool is a `function` tool with `strict: null`.
//! - Rust strings are always valid UTF-8, so `sanitizeSurrogates` is a no-op.

use std::collections::HashMap;

use pipkin_engine_protocol::{Block, Message, StopReason};
use serde_json::{Map, Value, json};

use crate::provider::{Model, ToolSpec};

/// Providers whose `call_id|item_id` tool-call ids are kept (reference `CODEX_TOOL_CALL_PROVIDERS`).
const CODEX_TOOL_CALL_PROVIDERS: [&str; 3] = ["openai", "openai-codex", "opencode"];

const NON_VISION_USER_IMAGE_PLACEHOLDER: &str = "(image omitted: model does not support images)";
const NON_VISION_TOOL_IMAGE_PLACEHOLDER: &str =
    "(tool image omitted: model does not support images)";

/// Reference `utils/hash.ts` `shortHash`, bit for bit (UTF-16 code units, 32-bit `imul`).
pub(crate) fn short_hash(s: &str) -> String {
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

/// `part.replace(/[^a-zA-Z0-9_-]/g, "_")`, cut to 64 UTF-16 units, trailing `_` removed.
fn normalize_id_part(part: &str) -> String {
    let mut out = String::new();
    for c in part.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            out.push(c);
        } else {
            // JS replaces each UTF-16 code unit.
            for _ in 0..c.len_utf16() {
                out.push('_');
            }
        }
    }
    out.truncate(64);
    out.trim_end_matches('_').to_owned()
}

fn foreign_item_id(item_id: &str) -> String {
    let mut id = format!("fc_{}", short_hash(item_id));
    id.truncate(64);
    id
}

fn assistant_identity(msg: &Message) -> Option<(&str, &str, &str)> {
    match msg {
        Message::Assistant {
            provider,
            api,
            model,
            ..
        } => Some((provider, api, model)),
        _ => None,
    }
}

/// Reference `normalizeToolCallId` closure inside `convertResponsesMessages`.
fn normalize_tool_call_id(id: &str, model: &Model, source: &Message) -> String {
    if !CODEX_TOOL_CALL_PROVIDERS.contains(&model.provider.as_str()) {
        return normalize_id_part(id);
    }
    if !id.contains('|') {
        return normalize_id_part(id);
    }
    let mut parts = id.split('|');
    let call_id = parts.next().unwrap_or_default();
    let item_id = parts.next().unwrap_or_default();
    let call_id = normalize_id_part(call_id);
    let (src_provider, src_api, _) = assistant_identity(source).unwrap_or_default();
    let foreign = src_provider != model.provider || src_api != model.api;
    let mut item = if foreign {
        foreign_item_id(item_id)
    } else {
        normalize_id_part(item_id)
    };
    if !item.starts_with("fc_") {
        item = normalize_id_part(&format!("fc_{item}"));
    }
    format!("{call_id}|{item}")
}

fn replace_images(content: &[Block], placeholder: &str) -> Vec<Block> {
    let mut out = Vec::new();
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

/// Reference `transformMessages` (`api/transform-messages.ts:64-235`) with the Responses id
/// normalizer.
pub(crate) fn transform_messages(messages: &[Message], model: &Model) -> Vec<Message> {
    let vision = model.supports_images();
    let mut id_map: HashMap<String, String> = HashMap::new();
    let mut transformed = Vec::with_capacity(messages.len());
    for msg in messages {
        let msg = match msg {
            Message::User {
                content,
                timestamp,
                extra,
            } if !vision => Message::User {
                content: replace_images(content, NON_VISION_USER_IMAGE_PLACEHOLDER),
                timestamp: *timestamp,
                extra: extra.clone(),
            },
            Message::ToolResult {
                tool_call_id,
                tool_name,
                content,
                is_error,
                timestamp,
                extra,
            } => {
                let content = if vision {
                    content.clone()
                } else {
                    replace_images(content, NON_VISION_TOOL_IMAGE_PLACEHOLDER)
                };
                let tool_call_id = id_map
                    .get(tool_call_id)
                    .cloned()
                    .unwrap_or_else(|| tool_call_id.clone());
                Message::ToolResult {
                    tool_call_id,
                    tool_name: tool_name.clone(),
                    content,
                    is_error: *is_error,
                    timestamp: *timestamp,
                    extra: extra.clone(),
                }
            }
            Message::Assistant {
                content,
                api,
                provider,
                model: msg_model,
                usage,
                stop_reason,
                error_message,
                timestamp,
                extra,
            } => {
                let same_model =
                    *provider == model.provider && *api == model.api && *msg_model == model.id;
                let mut out = Vec::new();
                for block in content {
                    match block {
                        Block::Thinking {
                            thinking,
                            signature,
                            redacted,
                            ..
                        } => {
                            if *redacted {
                                if same_model {
                                    out.push(block.clone());
                                }
                                continue;
                            }
                            if same_model && signature.as_deref().is_some_and(|s| !s.is_empty()) {
                                out.push(block.clone());
                                continue;
                            }
                            if thinking.trim().is_empty() {
                                continue;
                            }
                            if same_model {
                                out.push(block.clone());
                            } else {
                                out.push(Block::text(thinking.clone()));
                            }
                        }
                        Block::Text { text, .. } => {
                            if same_model {
                                out.push(block.clone());
                            } else {
                                out.push(Block::text(text.clone()));
                            }
                        }
                        Block::ToolCall {
                            id,
                            name,
                            arguments,
                            extra: block_extra,
                        } => {
                            let mut block_extra = block_extra.clone();
                            let mut id = id.clone();
                            if !same_model {
                                block_extra.remove("thoughtSignature");
                                let normalized = normalize_tool_call_id(&id, model, msg);
                                if normalized != id {
                                    id_map.insert(id.clone(), normalized.clone());
                                    id = normalized;
                                }
                            }
                            out.push(Block::ToolCall {
                                id,
                                name: name.clone(),
                                arguments: arguments.clone(),
                                extra: block_extra,
                            });
                        }
                        Block::Image { .. } => out.push(block.clone()),
                    }
                }
                Message::Assistant {
                    content: out,
                    api: api.clone(),
                    provider: provider.clone(),
                    model: msg_model.clone(),
                    usage: usage.clone(),
                    stop_reason: *stop_reason,
                    error_message: error_message.clone(),
                    timestamp: *timestamp,
                    extra: extra.clone(),
                }
            }
            other => other.clone(),
        };
        transformed.push(msg);
    }

    // Second pass: skip errored/aborted turns and close orphaned tool calls.
    let mut result = Vec::with_capacity(transformed.len());
    let mut pending: Vec<(String, String)> = Vec::new();
    let mut answered: std::collections::HashSet<String> = Default::default();
    let close = |result: &mut Vec<Message>,
                 pending: &mut Vec<(String, String)>,
                 answered: &mut std::collections::HashSet<String>| {
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

/// Reference `parseTextSignature`: `TextSignatureV1` JSON or a legacy plain id.
fn parse_text_signature(signature: Option<&str>) -> Option<(String, Option<String>)> {
    let signature = signature.filter(|s| !s.is_empty())?;
    if signature.starts_with('{')
        && let Ok(Value::Object(parsed)) = serde_json::from_str::<Value>(signature)
        && parsed.get("v").and_then(Value::as_f64) == Some(1.0)
        && let Some(id) = parsed.get("id").and_then(Value::as_str)
    {
        let phase = parsed
            .get("phase")
            .and_then(Value::as_str)
            .filter(|p| *p == "commentary" || *p == "final_answer")
            .map(str::to_owned);
        return Some((id.to_owned(), phase));
    }
    Some((signature.to_owned(), None))
}

/// Reference `encodeTextSignatureV1` (key order `v`, `id`, `phase`).
pub(crate) fn encode_text_signature(id: &str, phase: Option<&str>) -> String {
    let id = serde_json::to_string(id).unwrap_or_else(|_| "\"\"".into());
    match phase {
        Some(phase) => format!(
            "{{\"v\":1,\"id\":{id},\"phase\":{}}}",
            serde_json::to_string(phase).unwrap_or_else(|_| "\"\"".into())
        ),
        None => format!("{{\"v\":1,\"id\":{id}}}"),
    }
}

fn data_url(mime: &str, data: &str) -> String {
    format!("data:{mime};base64,{data}")
}

/// Reference `convertToolResultOutput`.
fn tool_result_output(model: &Model, content: &[Block]) -> Value {
    let text = content
        .iter()
        .filter_map(Block::as_text)
        .collect::<Vec<_>>()
        .join("\n");
    let images: Vec<(&str, &str)> = content
        .iter()
        .filter_map(|b| match b {
            Block::Image {
                data, mime_type, ..
            } => Some((mime_type.as_str(), data.as_str())),
            _ => None,
        })
        .collect();
    if images.is_empty() || !model.supports_images() {
        let s = if !text.is_empty() {
            text
        } else if !images.is_empty() {
            "(see attached image)".into()
        } else {
            "(no tool output)".into()
        };
        return Value::String(s);
    }
    let mut out = Vec::new();
    if !text.is_empty() {
        out.push(json!({"type": "input_text", "text": text}));
    }
    for (mime, data) in images {
        out.push(
            json!({"type": "input_image", "detail": "auto", "image_url": data_url(mime, data)}),
        );
    }
    Value::Array(out)
}

/// Reference `convertResponsesMessages` with `includeSystemPrompt: false` and no grammar tools.
/// Fails only when a same-model reasoning signature is not valid JSON (the reference throws).
pub(crate) fn convert_messages(model: &Model, messages: &[Message]) -> Result<Vec<Value>, String> {
    let transformed = transform_messages(messages, model);
    let mut items: Vec<Value> = Vec::new();
    // `msgIndex` only advances for messages that produced output (the reference `continue`s
    // past the increment for empty user/assistant messages).
    let mut msg_index = 0usize;
    for msg in &transformed {
        match msg {
            Message::User { content, .. } => {
                let parts: Vec<Value> = content
                    .iter()
                    .filter_map(|b| match b {
                        Block::Text { text, .. } => {
                            Some(json!({"type": "input_text", "text": text}))
                        }
                        Block::Image {
                            data, mime_type, ..
                        } => Some(json!({
                            "type": "input_image",
                            "detail": "auto",
                            "image_url": data_url(mime_type, data),
                        })),
                        _ => None,
                    })
                    .collect();
                if parts.is_empty() {
                    continue;
                }
                items.push(json!({"role": "user", "content": parts}));
            }
            Message::Assistant {
                content,
                provider,
                api,
                model: msg_model,
                ..
            } => {
                let same_provider_api = *provider == model.provider && *api == model.api;
                let same_model = same_provider_api && *msg_model == model.id;
                let different_model = same_provider_api && *msg_model != model.id;
                let mut output = Vec::new();
                let mut text_block_index = 0usize;
                for block in content {
                    match block {
                        Block::Thinking { signature, .. } => {
                            if let Some(sig) = signature.as_deref().filter(|s| !s.is_empty()) {
                                let item: Value = serde_json::from_str(sig).map_err(|e| {
                                    format!("Invalid reasoning signature JSON: {e}")
                                })?;
                                output.push(item);
                            }
                        }
                        Block::Text { text, extra } => {
                            let parsed = parse_text_signature(
                                extra.get("textSignature").and_then(Value::as_str),
                            );
                            let fallback = if text_block_index == 0 {
                                format!("msg_pi_{msg_index}")
                            } else {
                                format!("msg_pi_{msg_index}_{text_block_index}")
                            };
                            text_block_index += 1;
                            let (id, phase) = match parsed {
                                None => (fallback, None),
                                Some((id, phase)) if id.is_empty() => (fallback, phase),
                                Some((id, phase)) if id.encode_utf16().count() > 64 => {
                                    (format!("msg_{}", short_hash(&id)), phase)
                                }
                                Some(p) => p,
                            };
                            let mut item = Map::new();
                            item.insert("type".into(), "message".into());
                            item.insert("role".into(), "assistant".into());
                            item.insert(
                                "content".into(),
                                json!([{"type": "output_text", "text": text, "annotations": []}]),
                            );
                            item.insert("status".into(), "completed".into());
                            item.insert("id".into(), Value::String(id));
                            if let Some(phase) = phase {
                                item.insert("phase".into(), Value::String(phase));
                            }
                            output.push(Value::Object(item));
                        }
                        Block::ToolCall {
                            id,
                            name,
                            arguments,
                            extra,
                        } => {
                            let mut parts = id.split('|');
                            let call_id = parts.next().unwrap_or_default();
                            let item_id = parts
                                .next()
                                .filter(|i| !different_model && i.starts_with("fc_"));
                            let mut item = Map::new();
                            item.insert("type".into(), "function_call".into());
                            if let Some(item_id) = item_id {
                                item.insert("id".into(), Value::String(item_id.to_owned()));
                            }
                            item.insert("call_id".into(), Value::String(call_id.to_owned()));
                            item.insert("name".into(), Value::String(name.clone()));
                            item.insert(
                                "arguments".into(),
                                Value::String(
                                    serde_json::to_string(arguments)
                                        .unwrap_or_else(|_| "{}".into()),
                                ),
                            );
                            if same_model && let Some(ns) = extra.get("namespace") {
                                item.insert("namespace".into(), ns.clone());
                            }
                            output.push(Value::Object(item));
                        }
                        Block::Image { .. } => {}
                    }
                }
                if output.is_empty() {
                    continue;
                }
                items.extend(output);
            }
            Message::ToolResult {
                tool_call_id,
                content,
                ..
            } => {
                let call_id = tool_call_id.split('|').next().unwrap_or_default();
                items.push(json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": tool_result_output(model, content),
                }));
            }
        }
        msg_index += 1;
    }
    Ok(items)
}

/// Reference `convertResponsesTools` with `strict: null` and no constrained sampling.
pub(crate) fn convert_tools(tools: &[ToolSpec], supports_strict_mode: bool) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            let mut item = Map::new();
            item.insert("type".into(), "function".into());
            item.insert("name".into(), Value::String(tool.name.clone()));
            item.insert(
                "description".into(),
                Value::String(tool.description.clone()),
            );
            item.insert("parameters".into(), tool.parameters.clone());
            if supports_strict_mode {
                item.insert("strict".into(), Value::Null);
            }
            Value::Object(item)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_hash_matches_the_reference_values() {
        // Values computed with the reference `shortHash` (utils/hash.ts) under Node.
        assert_eq!(short_hash(""), "k4n83c7h0j2b");
        assert_eq!(short_hash("call_abc|fc_123"), "jp01dsxufnom");
        assert_eq!(short_hash("h\u{e9}llo \u{1f600}"), "1t90xbiinqdrp");
        assert_eq!(short_hash("rs_foreign_item_id_0123456789"), "1fvsux1lx7hr4");
    }

    #[test]
    fn id_parts_are_normalized_like_the_reference() {
        assert_eq!(normalize_id_part("call_1.2"), "call_1_2");
        assert_eq!(normalize_id_part("a\u{1f600}"), "a");
        assert_eq!(normalize_id_part(&"x".repeat(80)).len(), 64);
        assert_eq!(normalize_id_part("ab__"), "ab");
    }

    #[test]
    fn text_signatures_round_trip() {
        let sig = encode_text_signature("msg_1", Some("final_answer"));
        assert_eq!(sig, r#"{"v":1,"id":"msg_1","phase":"final_answer"}"#);
        assert_eq!(
            parse_text_signature(Some(&sig)),
            Some(("msg_1".into(), Some("final_answer".into())))
        );
        assert_eq!(
            parse_text_signature(Some("legacy")),
            Some(("legacy".into(), None))
        );
    }
}
