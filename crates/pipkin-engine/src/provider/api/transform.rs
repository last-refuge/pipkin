//! Shared adapter helpers.
//!
//! The core is [`transform_messages`], the cross-provider replay normalization every wire
//! adapter runs before converting context (reference `api/transform-messages.ts`). The rest are
//! small reference-exact utilities several adapters need: the reference `shortHash`, JSON
//! repair for streamed tool arguments, thinking-level lookups, thinking budgets, sampling
//! parameter merging, header assembly and the request-level send with retries.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;

use pipkin_engine_protocol::{Block, Message, StopReason};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde_json::{Map, Value};

use crate::provider::model::{Model, ThinkingLevel};
use crate::provider::{ProviderRequest, StreamCtx, http};

const NON_VISION_USER_IMAGE_PLACEHOLDER: &str = "(image omitted: model does not support images)";
const NON_VISION_TOOL_IMAGE_PLACEHOLDER: &str =
    "(tool image omitted: model does not support images)";

/// Maps a tool call id from a different model to one the target API accepts. Receives the id
/// and the assistant message that made the call.
pub type NormalizeToolCallId<'a> = &'a dyn Fn(&str, &Message) -> String;

fn replace_images(content: &[Block], placeholder: &str) -> Vec<Block> {
    let mut out = Vec::with_capacity(content.len());
    let mut previous_was_placeholder = false;
    for block in content {
        match block {
            Block::Image { .. } => {
                if !previous_was_placeholder {
                    out.push(Block::text(placeholder));
                }
                previous_was_placeholder = true;
            }
            other => {
                previous_was_placeholder =
                    matches!(other, Block::Text { text, .. } if text == placeholder);
                out.push(other.clone());
            }
        }
    }
    out
}

fn same_model(message: &Message, model: &Model) -> bool {
    matches!(message, Message::Assistant { provider, api, model: id, .. }
        if *provider == model.provider && *api == model.api && *id == model.id)
}

/// Normalize a context for replay to `model` (reference `transformMessages`):
///
/// - images become a placeholder when the model has no image input;
/// - assistant turns from a different model lose redacted thinking and provider signatures,
///   readable thinking becomes text, and tool call ids go through `normalize` (tool results
///   follow the mapping);
/// - errored or aborted assistant turns are dropped;
/// - tool calls without a result get a synthetic error result before the next assistant or
///   user message (or at the end).
pub fn transform_messages(
    messages: &[Message],
    model: &Model,
    normalize: Option<NormalizeToolCallId<'_>>,
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
                let same = same_model(msg, model);
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
                                if let Some(normalize) = normalize {
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
        let ch = ch as u32;
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
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).expect("ascii digits")
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

/// First `n` characters (by UTF-16 units for the ASCII ids this is used on).
pub fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Repair malformed JSON string literals the way the reference `repairJson` does: escape raw
/// control characters inside strings and double backslashes before invalid escapes.
pub fn repair_json(json: &str) -> String {
    let chars: Vec<char> = json.chars().collect();
    let mut out = String::with_capacity(json.len() + 8);
    let mut in_string = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if !in_string {
            out.push(c);
            if c == '"' {
                in_string = true;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            out.push(c);
            in_string = false;
            i += 1;
            continue;
        }
        if c == '\\' {
            match chars.get(i + 1) {
                None => {
                    out.push_str("\\\\");
                    i += 1;
                }
                Some('u')
                    if chars.len() >= i + 6
                        && chars[i + 2..i + 6].iter().all(|d| d.is_ascii_hexdigit()) =>
                {
                    out.push_str("\\u");
                    out.extend(&chars[i + 2..i + 6]);
                    i += 6;
                }
                Some(n) if matches!(n, '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't' | 'u') => {
                    out.push('\\');
                    out.push(*n);
                    i += 2;
                }
                Some(_) => {
                    out.push_str("\\\\");
                    i += 1;
                }
            }
            continue;
        }
        if (c as u32) <= 0x1f {
            match c {
                '\u{8}' => out.push_str("\\b"),
                '\u{c}' => out.push_str("\\f"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                _ => out.push_str(&format!("\\u{:04x}", c as u32)),
            }
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

/// Parse complete JSON, retrying once with [`repair_json`]. `None` when it is still invalid
/// (incomplete arguments are never guessed at).
pub fn parse_json_with_repair(json: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str(json) {
        return Some(v);
    }
    let repaired = repair_json(json);
    if repaired != json {
        return serde_json::from_str(&repaired).ok();
    }
    None
}

/// A model's `thinkingLevelMap` entry: missing, explicitly `null`, or a native value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mapped<'a> {
    Missing,
    Null,
    Value(&'a str),
}

pub fn mapped_level<'a>(model: &'a Model, level: &str) -> Mapped<'a> {
    match model.thinking_level_map.get(level) {
        None => Mapped::Missing,
        Some(None) => Mapped::Null,
        Some(Some(v)) => Mapped::Value(v),
    }
}

impl<'a> Mapped<'a> {
    /// `map[level] ?? fallback`.
    pub fn or<'b>(self, fallback: &'b str) -> &'b str
    where
        'a: 'b,
    {
        match self {
            Mapped::Value(v) => v,
            _ => fallback,
        }
    }
}

/// The effort a request asks for: the requested level clamped to what the model supports,
/// `None` when that is `off` (reference `streamSimple`; the agent sends no level at all for
/// `off`, so `off` is never clamped up to a model's minimum level here).
pub fn reasoning_effort(req: &ProviderRequest) -> Option<ThinkingLevel> {
    if req.thinking == ThinkingLevel::Off {
        return None;
    }
    match req.model.clamp_thinking(req.thinking) {
        ThinkingLevel::Off => None,
        level => Some(level),
    }
}

/// Tokens always left for the answer when a thinking budget shares the response ceiling.
pub const MIN_ANSWER_TOKENS: u64 = 1024;

/// Default thinking budgets by level (`xhigh`/`max` use `high`), overridable through the
/// `thinkingBudgets` request option.
pub fn thinking_budget_for_level(level: ThinkingLevel, custom: Option<&Value>) -> u64 {
    let level = match level {
        ThinkingLevel::Xhigh | ThinkingLevel::Max => ThinkingLevel::High,
        other => other,
    };
    let default = match level {
        ThinkingLevel::Minimal => 1024,
        ThinkingLevel::Low => 2048,
        ThinkingLevel::Medium => 8192,
        _ => 16384,
    };
    custom
        .and_then(|c| c.get(level.as_str()))
        .and_then(Value::as_u64)
        .unwrap_or(default)
}

pub fn clamp_thinking_budget_to_answer_room(budget: u64, ceiling: u64) -> u64 {
    budget.min(ceiling.saturating_sub(MIN_ANSWER_TOKENS))
}

/// Model sampling defaults < per-thinking-level defaults < request `samplingParams`
/// (reference `resolveSamplingParams`). `None` when none is set.
pub fn resolve_sampling_params(
    req: &ProviderRequest,
    effort: Option<ThinkingLevel>,
) -> Option<Map<String, Value>> {
    let model = &req.model;
    let level = model.clamp_thinking(effort.unwrap_or(ThinkingLevel::Off));
    let base = model.extra.get("samplingParams").and_then(Value::as_object);
    let by_level = model
        .extra
        .get("samplingParamsByThinkingLevel")
        .and_then(|m| m.get(level.as_str()))
        .and_then(Value::as_object);
    let request = req.options.get("samplingParams").and_then(Value::as_object);
    if base.is_none() && by_level.is_none() && request.is_none() {
        return None;
    }
    let mut out = Map::new();
    for source in [base, by_level, request].into_iter().flatten() {
        for (k, v) in source {
            out.insert(k.clone(), v.clone());
        }
    }
    Some(out)
}

/// Assemble request headers in SDK order: authentication, then model headers, then dynamic
/// headers, then request headers. Later layers win; a `None` request value deletes a header.
pub fn build_headers(
    auth: &[(&str, String)],
    model_headers: &BTreeMap<String, String>,
    dynamic: &[(&str, String)],
    request: &BTreeMap<String, Option<String>>,
) -> Result<HeaderMap, String> {
    let mut map = HeaderMap::new();
    let set = |map: &mut HeaderMap, k: &str, v: Option<&str>| -> Result<(), String> {
        let name = HeaderName::from_bytes(k.as_bytes())
            .map_err(|_| format!("Invalid header name: {k}"))?;
        match v {
            Some(v) => {
                let value = HeaderValue::from_str(v)
                    .map_err(|_| format!("Invalid value for header {k}"))?;
                map.insert(name, value);
            }
            None => {
                map.remove(name);
            }
        }
        Ok(())
    };
    for (k, v) in auth {
        set(&mut map, k, Some(v))?;
    }
    for (k, v) in model_headers {
        set(&mut map, k, Some(v))?;
    }
    for (k, v) in dynamic {
        set(&mut map, k, Some(v))?;
    }
    for (k, v) in request {
        set(&mut map, k, v.as_deref())?;
    }
    Ok(map)
}

/// Whether a request header with a non-blank value is configured (case-insensitive).
pub fn has_header(headers: &BTreeMap<String, Option<String>>, name: &str) -> bool {
    headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case(name) && v.as_deref().is_some_and(|v| !v.trim().is_empty())
    })
}

/// Header/idle timeout for a request; zero disables it (reference: 0 => 2^31-1 ms).
pub fn idle_timeout(req: &ProviderRequest) -> Duration {
    if req.timeout.is_zero() {
        Duration::from_millis(i32::MAX as u64)
    } else {
        req.timeout
    }
}

/// POST a JSON body with request-level retries (`maxRetries`, default 0; `maxRetryDelayMs`,
/// default 60000, 0 = no cap) until response headers arrive. Returns the formatted error text
/// on failure (`"<status>: <body>"` for HTTP errors).
pub async fn post_json(
    ctx: &StreamCtx<'_>,
    url: &str,
    headers: HeaderMap,
    body: &Value,
) -> Result<reqwest::Response, String> {
    let retries = ctx
        .req
        .options
        .get("maxRetries")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(u32::MAX as u64) as u32;
    let max_delay = ctx
        .req
        .options
        .get("maxRetryDelayMs")
        .and_then(Value::as_u64)
        .map(Duration::from_millis)
        .unwrap_or(http::MAX_RETRY_DELAY);
    let bytes = serde_json::to_vec(body).map_err(|e| e.to_string())?;
    let build = || {
        ctx.http
            .post(url)
            .headers(headers.clone())
            .header(CONTENT_TYPE, "application/json")
            .body(bytes.clone())
    };
    let per_attempt = idle_timeout(ctx.req);
    let budget = per_attempt
        .saturating_mul(retries.saturating_add(1))
        .saturating_add(max_delay.saturating_mul(retries));
    match tokio::time::timeout(budget, http::send(build, retries, max_delay, &ctx.cancel)).await {
        Ok(result) => result,
        Err(_) => Err("Request timed out waiting for response headers".into()),
    }
}

/// A string option from `ProviderRequest::options`.
pub fn option_str<'a>(req: &'a ProviderRequest, key: &str) -> Option<&'a str> {
    req.options.get(key).and_then(Value::as_str)
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
    }

    #[test]
    fn repair_escapes_control_characters_and_bad_escapes() {
        let raw = "{\"a\":\"line\nbreak\",\"b\":\"C:\\path\",\"c\":\"\\u00e9\"}";
        let v = parse_json_with_repair(raw).unwrap();
        assert_eq!(v["a"], "line\nbreak");
        assert_eq!(v["b"], "C:\\path");
        assert_eq!(v["c"], "\u{e9}");
        assert!(parse_json_with_repair("{\"a\":\"cut").is_none());
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
        let normalize = |id: &str, _: &Message| id.replace('|', "_");
        let out = transform_messages(&messages, &target, Some(&normalize));
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

    #[test]
    fn sampling_params_merge_in_order() {
        let mut req = crate::provider::api::openai_completions::tests::request(json!({
            "id": "m", "provider": "x", "api": "openai-completions", "reasoning": true,
            "samplingParams": {"temperature": 0.1, "top_p": 0.9},
            "samplingParamsByThinkingLevel": {"high": {"temperature": 0.5}}
        }));
        req.options
            .insert("samplingParams".into(), json!({"top_p": 0.5}));
        let merged = resolve_sampling_params(&req, Some(ThinkingLevel::High)).unwrap();
        assert_eq!(
            Value::Object(merged),
            json!({"temperature": 0.5, "top_p": 0.5})
        );
        assert_eq!(
            thinking_budget_for_level(ThinkingLevel::Max, Some(&json!({"high": 99}))),
            99
        );
        assert_eq!(clamp_thinking_budget_to_answer_room(16384, 2000), 976);
    }
}
