//! Shared pieces for the Google adapters (`google-generative-ai`, `google-vertex`).
//!
//! Reference: `packages/ai/src/api/google-shared.ts`, `api/google-generative-ai.ts` (stream loop).
//! Cross-adapter helpers (replay normalization, thinking budgets, strict JSON schemas, headers,
//! request sending) are in [`super::common`].

use std::collections::HashSet;
use std::ops::ControlFlow;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use pipkin_engine_protocol::{Block, Message, StopReason};
use regex::Regex;
use serde_json::{Map, Value, json};

use super::common::schema::{resolve_strict, tool_parameters};
use super::common::{
    Headers, base_max_tokens, custom_budget, idle_timeout, opt, opt_str, post_json,
    reasoning_effort, sanitize_tool_call_id, transform_messages,
};
use crate::provider::http::{self, ReadError};
use crate::provider::sse::SseEvent;
use crate::provider::{Emitter, Model, ProviderRequest, StreamCtx, ThinkingLevel};

// ---------------------------------------------------------------------------------------------
// Google: thinking
// ---------------------------------------------------------------------------------------------

fn regex(pattern: &'static str, cell: &'static OnceLock<Regex>) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("static pattern"))
}

/// Models that use Gemini's discrete `thinkingLevel` instead of `thinkingBudget`.
pub(crate) fn uses_google_thinking_level(model: &Model) -> bool {
    static PRO_FLASH: OnceLock<Regex> = OnceLock::new();
    static GEMMA: OnceLock<Regex> = OnceLock::new();
    let id = model.id.to_lowercase();
    regex(r"gemini-3(?:\.\d+)?-(?:pro|flash)", &PRO_FLASH).is_match(&id)
        || id == "gemini-flash-latest"
        || id == "gemini-flash-lite-latest"
        || regex(r"gemma-?4", &GEMMA).is_match(&id)
}

/// A supported level (or the model's mapping of it) as a standard Google level.
pub(crate) fn resolve_google_thinking_level(
    model: &Model,
    level: ThinkingLevel,
) -> Result<ThinkingLevel, String> {
    let mapped = model.thinking_level_map.get(level.as_str());
    let resolved = match mapped {
        Some(Some(v)) => v.to_lowercase(),
        _ => level.as_str().to_owned(),
    };
    match ThinkingLevel::parse(&resolved) {
        Some(
            l @ (ThinkingLevel::Minimal
            | ThinkingLevel::Low
            | ThinkingLevel::Medium
            | ThinkingLevel::High),
        ) => Ok(l),
        _ => Err(format!(
            "Unsupported Google thinking level mapping for {}/{}: {} -> {}",
            model.provider,
            model.id,
            level.as_str(),
            match mapped {
                Some(Some(v)) => v.clone(),
                Some(None) => "null".into(),
                None => "undefined".into(),
            }
        )),
    }
}

pub(crate) fn to_google_thinking_level(level: ThinkingLevel) -> &'static str {
    match level {
        ThinkingLevel::Minimal => "MINIMAL",
        ThinkingLevel::Low => "LOW",
        ThinkingLevel::Medium => "MEDIUM",
        _ => "HIGH",
    }
}

pub(crate) fn disabled_thinking_config(model: &Model) -> Result<Value, String> {
    if !uses_google_thinking_level(model) {
        return Ok(json!({"thinkingBudget": 0}));
    }
    let fallback = model.clamp_thinking(ThinkingLevel::Off);
    if fallback == ThinkingLevel::Off {
        return Ok(json!({"thinkingBudget": 0}));
    }
    let resolved = resolve_google_thinking_level(model, fallback)?;
    Ok(json!({"thinkingLevel": to_google_thinking_level(resolved)}))
}

/// Model-specific thinking budgets (`-1` = dynamic). Vertex has no flash-lite table.
pub(crate) fn google_budget(
    req: &ProviderRequest,
    level: ThinkingLevel,
    has_flash_lite_table: bool,
) -> i64 {
    if let Some(custom) = custom_budget(req, level) {
        return custom as i64;
    }
    let id = &req.model.id;
    let table: [i64; 4] = if id.contains("2.5-pro") {
        [128, 2048, 8192, 32768]
    } else if has_flash_lite_table && id.contains("2.5-flash-lite") {
        [512, 2048, 8192, 24576]
    } else if id.contains("2.5-flash") {
        [128, 2048, 8192, 24576]
    } else {
        return -1;
    };
    match level {
        ThinkingLevel::Minimal => table[0],
        ThinkingLevel::Low => table[1],
        ThinkingLevel::Medium => table[2],
        _ => table[3],
    }
}

/// The `thinkingConfig` for this request, if any (reference `streamSimple` + `buildParams`).
pub(crate) fn thinking_config(
    req: &ProviderRequest,
    has_flash_lite_table: bool,
) -> Result<Option<Value>, String> {
    let model = &req.model;
    if !model.reasoning {
        return Ok(None);
    }
    // The agent sends no reasoning option for `off` (reference `streamSimple`: thinking
    // disabled), so a requested `off` is never clamped up to a minimum level.
    let Some(level) = reasoning_effort(req) else {
        return disabled_thinking_config(model).map(Some);
    };
    let resolved = resolve_google_thinking_level(model, level)?;
    if uses_google_thinking_level(model) {
        return Ok(Some(json!({
            "includeThoughts": true,
            "thinkingLevel": to_google_thinking_level(resolved),
        })));
    }
    Ok(Some(json!({
        "includeThoughts": true,
        "thinkingBudget": google_budget(req, resolved, has_flash_lite_table),
    })))
}

// ---------------------------------------------------------------------------------------------
// Google: request conversion
// ---------------------------------------------------------------------------------------------

fn gemini_major_version(model_id: &str) -> Option<u32> {
    static R: OnceLock<Regex> = OnceLock::new();
    regex(r"^gemini(?:-live)?-(\d+)", &R)
        .captures(&model_id.to_lowercase())?
        .get(1)?
        .as_str()
        .parse()
        .ok()
}

/// Models behind Google APIs that need explicit tool call ids.
pub(crate) fn requires_tool_call_id(model_id: &str) -> bool {
    model_id.starts_with("claude-")
        || model_id.starts_with("gpt-oss-")
        || gemini_major_version(model_id).is_some_and(|v| v >= 3)
}

fn supports_multimodal_function_response(model_id: &str) -> bool {
    gemini_major_version(model_id).is_none_or(|v| v >= 3)
}

/// Gemini 3+ enforces required parameters in validated tool-calling modes.
pub(crate) fn supports_google_strict_tool_sampling(model_id: &str) -> bool {
    gemini_major_version(model_id).is_some_and(|v| v >= 3)
}

/// Thought signatures must be valid base64 (TYPE_BYTES).
fn is_valid_thought_signature(signature: Option<&str>) -> bool {
    static R: OnceLock<Regex> = OnceLock::new();
    let Some(s) = signature.filter(|s| !s.is_empty()) else {
        return false;
    };
    s.len() % 4 == 0 && regex(r"^[A-Za-z0-9+/]+={0,2}$", &R).is_match(s)
}

fn resolve_thought_signature(same: bool, signature: Option<&str>) -> Option<String> {
    (same && is_valid_thought_signature(signature)).then(|| signature.unwrap_or("").to_owned())
}

fn extra_str<'a>(extra: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    extra.get(key).and_then(Value::as_str)
}

fn with_signature(mut part: Value, signature: Option<String>) -> Value {
    if let Some(sig) = signature {
        part["thoughtSignature"] = Value::String(sig);
    }
    part
}

/// Messages as Gemini `contents`.
pub(crate) fn convert_messages(model: &Model, messages: &[Message]) -> Vec<Value> {
    let needs_id = requires_tool_call_id(&model.id);
    let mut normalize = |id: &str, _: &Message| sanitize_tool_call_id(id);
    let transformed = transform_messages(
        messages,
        model,
        if needs_id { Some(&mut normalize) } else { None },
    );
    let mut contents: Vec<Value> = Vec::new();
    for msg in &transformed {
        match msg {
            Message::User { content, .. } => {
                let parts: Vec<Value> = content
                    .iter()
                    .filter_map(|b| match b {
                        Block::Text { text, .. } => Some(json!({"text": text})),
                        Block::Image {
                            data, mime_type, ..
                        } => Some(json!({"inlineData": {"mimeType": mime_type, "data": data}})),
                        _ => None,
                    })
                    .collect();
                if parts.is_empty() {
                    continue;
                }
                contents.push(json!({"role": "user", "parts": parts}));
            }
            Message::Assistant {
                content, provider, ..
            } => {
                let same = *provider == model.provider
                    && matches!(msg, Message::Assistant { model: m, .. } if *m == model.id);
                let mut parts = Vec::new();
                for block in content {
                    match block {
                        Block::Text { text, extra } => {
                            let sig =
                                resolve_thought_signature(same, extra_str(extra, "textSignature"));
                            if text.trim().is_empty() && sig.is_none() {
                                continue;
                            }
                            parts.push(with_signature(json!({"text": text}), sig));
                        }
                        Block::Thinking {
                            thinking,
                            signature,
                            ..
                        } => {
                            if same {
                                let sig = resolve_thought_signature(same, signature.as_deref());
                                if thinking.trim().is_empty() && sig.is_none() {
                                    continue;
                                }
                                parts.push(with_signature(
                                    json!({"thought": true, "text": thinking}),
                                    sig,
                                ));
                            } else {
                                if thinking.trim().is_empty() {
                                    continue;
                                }
                                parts.push(json!({"text": thinking}));
                            }
                        }
                        Block::ToolCall {
                            id,
                            name,
                            arguments,
                            extra,
                        } => {
                            let sig = resolve_thought_signature(
                                same,
                                extra_str(extra, "thoughtSignature"),
                            );
                            let args = if arguments.is_null() {
                                json!({})
                            } else {
                                arguments.clone()
                            };
                            let mut call = json!({"name": name, "args": args});
                            if needs_id {
                                call["id"] = Value::String(id.clone());
                            }
                            parts.push(with_signature(json!({"functionCall": call}), sig));
                        }
                        Block::Image { .. } => {}
                    }
                }
                if parts.is_empty() {
                    continue;
                }
                contents.push(json!({"role": "model", "parts": parts}));
            }
            Message::ToolResult {
                tool_call_id,
                tool_name,
                content,
                is_error,
                ..
            } => {
                let text = content
                    .iter()
                    .filter_map(Block::as_text)
                    .collect::<Vec<_>>()
                    .join("\n");
                let images: Vec<Value> = if model.supports_images() {
                    content
                        .iter()
                        .filter_map(|b| match b {
                            Block::Image {
                                data, mime_type, ..
                            } => Some(json!({"inlineData": {"mimeType": mime_type, "data": data}})),
                            _ => None,
                        })
                        .collect()
                } else {
                    vec![]
                };
                let has_text = !text.is_empty();
                let has_images = !images.is_empty();
                let multimodal = supports_multimodal_function_response(&model.id);
                let value = if has_text {
                    text
                } else if has_images {
                    "(see attached image)".into()
                } else {
                    String::new()
                };
                let mut response = json!({
                    "name": tool_name,
                    "response": if *is_error { json!({"error": value}) } else { json!({"output": value}) },
                });
                if has_images && multimodal {
                    response["parts"] = Value::Array(images.clone());
                }
                if needs_id {
                    response["id"] = Value::String(tool_call_id.clone());
                }
                let part = json!({"functionResponse": response});
                let merge = contents.last().is_some_and(|c| {
                    c["role"] == "user"
                        && c["parts"]
                            .as_array()
                            .is_some_and(|p| p.iter().any(|p| p.get("functionResponse").is_some()))
                });
                if merge {
                    if let Some(parts) = contents.last_mut().and_then(|c| c["parts"].as_array_mut())
                    {
                        parts.push(part);
                    }
                } else {
                    contents.push(json!({"role": "user", "parts": [part]}));
                }
                if has_images && !multimodal {
                    let mut parts = vec![json!({"text": "Tool result image:"})];
                    parts.extend(images);
                    contents.push(json!({"role": "user", "parts": parts}));
                }
            }
        }
    }
    contents
}

/// Tools as Gemini function declarations (`parametersJsonSchema`).
pub(crate) fn convert_tools(
    req: &ProviderRequest,
    supports_strict: bool,
) -> Result<Option<Value>, String> {
    if req.tools.is_empty() {
        return Ok(None);
    }
    let mut decls = Vec::with_capacity(req.tools.len());
    for tool in &req.tools {
        let strict = resolve_strict(req, tool, supports_strict, None)?;
        decls.push(json!({
            "name": tool.name,
            "description": tool.description,
            "parametersJsonSchema": tool_parameters(tool, strict)?,
        }));
    }
    Ok(Some(json!([{"functionDeclarations": decls}])))
}

fn map_tool_choice(choice: &str) -> &'static str {
    match choice {
        "none" => "NONE",
        "any" => "ANY",
        _ => "AUTO",
    }
}

/// Reference `resolveGoogleFunctionCallingMode`.
pub(crate) fn function_calling_mode(
    req: &ProviderRequest,
    supports_strict: bool,
) -> Result<Option<&'static str>, String> {
    let choice = opt_str(req, "toolChoice");
    let mut strict = false;
    for tool in &req.tools {
        if resolve_strict(req, tool, supports_strict, None)? == Some(true) {
            strict = true;
        }
    }
    if let Some(c @ ("none" | "any")) = choice {
        return Ok(Some(map_tool_choice(c)));
    }
    if strict {
        return Ok(Some("VALIDATED"));
    }
    Ok(choice.map(map_tool_choice))
}

/// The REST `generateContent` body shared by both Google adapters.
pub(crate) fn build_body(
    req: &ProviderRequest,
    has_flash_lite_table: bool,
) -> Result<Value, String> {
    let model = &req.model;
    let contents = convert_messages(model, &req.messages);
    let mut generation = Map::new();
    if let Some(t) = opt(req, "temperature").filter(|v| v.is_number()) {
        generation.insert("temperature".into(), t.clone());
    }
    generation.insert("maxOutputTokens".into(), json!(base_max_tokens(req)));
    if let Some(thinking) = thinking_config(req, has_flash_lite_table)? {
        generation.insert("thinkingConfig".into(), thinking);
    }
    let mut body = Map::new();
    body.insert("contents".into(), Value::Array(contents));
    body.insert("generationConfig".into(), Value::Object(generation));
    if !req.system.is_empty() {
        body.insert(
            "systemInstruction".into(),
            json!({"role": "user", "parts": [{"text": req.system}]}),
        );
    }
    let supports_strict = supports_google_strict_tool_sampling(&model.id);
    if let Some(tools) = convert_tools(req, supports_strict)? {
        body.insert("tools".into(), tools);
        if let Some(mode) = function_calling_mode(req, supports_strict)? {
            body.insert(
                "toolConfig".into(),
                json!({"functionCallingConfig": {"mode": mode}}),
            );
        }
    }
    Ok(Value::Object(body))
}

/// Headers both Google adapters start from: model headers, then request headers.
pub(crate) fn base_headers(req: &ProviderRequest) -> Headers {
    let mut h = Headers::default();
    h.merge(&req.model.headers);
    h.merge_optional(&req.headers);
    h
}

// ---------------------------------------------------------------------------------------------
// Google: streaming
// ---------------------------------------------------------------------------------------------

/// Map a Gemini finish reason. Everything but STOP / MAX_TOKENS is an error.
pub(crate) fn map_stop_reason(reason: &str) -> StopReason {
    match reason {
        "STOP" => StopReason::Stop,
        "MAX_TOKENS" => StopReason::Length,
        _ => StopReason::Error,
    }
}

static TOOL_CALL_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, PartialEq)]
enum Current {
    Text(usize),
    Thinking(usize),
}

#[derive(Default)]
struct GoogleStream {
    current: Option<Current>,
    stop: Option<StopReason>,
    raw_stop: Option<String>,
    error: Option<String>,
    response_id: bool,
    tool_ids: HashSet<String>,
}

impl GoogleStream {
    fn on_event(&mut self, e: &mut Emitter, event: SseEvent) -> ControlFlow<()> {
        let data = event.data.trim();
        if data.is_empty() {
            return ControlFlow::Continue(());
        }
        let chunk: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(err) => {
                self.error = Some(format!(
                    "Could not parse Google stream event: {err}; data={data}"
                ));
                return ControlFlow::Break(());
            }
        };
        if let Some(error) = chunk.get("error").filter(|v| v.is_object()) {
            let status = error
                .get("status")
                .map(|s| s.as_str().map_or_else(|| s.to_string(), str::to_owned))
                .unwrap_or_else(|| "undefined".into());
            self.error = Some(format!("got status: {status}. {data}"));
            return ControlFlow::Break(());
        }
        if !self.response_id
            && let Some(id) = chunk
                .get("responseId")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        {
            e.set_response_id(id);
            self.response_id = true;
        }
        let candidate = chunk.get("candidates").and_then(|c| c.get(0));
        if let Some(parts) = candidate
            .and_then(|c| c.get("content"))
            .and_then(|c| c.get("parts"))
            .and_then(Value::as_array)
        {
            for part in parts {
                self.on_part(e, part);
            }
        }
        if let Some(reason) = candidate
            .and_then(|c| c.get("finishReason"))
            .and_then(Value::as_str)
        {
            let mut stop = map_stop_reason(reason);
            if stop == StopReason::Stop && e.has_tool_calls() {
                stop = StopReason::ToolUse;
            }
            self.stop = Some(stop);
            self.raw_stop = Some(reason.to_owned());
        }
        if let Some(meta) = chunk.get("usageMetadata").filter(|m| m.is_object()) {
            let n = |k: &str| meta.get(k).and_then(Value::as_u64).unwrap_or(0);
            let u = e.usage_mut();
            u.input = n("promptTokenCount").saturating_sub(n("cachedContentTokenCount"));
            u.output = n("candidatesTokenCount") + n("thoughtsTokenCount");
            u.reasoning = Some(n("thoughtsTokenCount"));
            u.cache_read = n("cachedContentTokenCount");
            u.cache_write = 0;
            u.total_tokens = n("totalTokenCount");
        }
        ControlFlow::Continue(())
    }

    fn on_part(&mut self, e: &mut Emitter, part: &Value) {
        let signature = part
            .get("thoughtSignature")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            let thinking = part.get("thought").and_then(Value::as_bool) == Some(true);
            let block = match self.current {
                Some(Current::Thinking(i)) if thinking => Current::Thinking(i),
                Some(Current::Text(i)) if !thinking => Current::Text(i),
                _ if thinking => Current::Thinking(e.start_thinking()),
                _ => Current::Text(e.start_text()),
            };
            self.current = Some(block);
            match block {
                Current::Thinking(i) => {
                    e.thinking(i, text);
                    if let Some(sig) = signature {
                        e.thinking_signature(i, sig, false);
                    }
                }
                Current::Text(i) => {
                    e.text(i, text);
                    if let Some(sig) = signature {
                        e.block_extra(i, "textSignature", Value::String(sig.to_owned()));
                    }
                }
            }
        }
        if let Some(call) = part.get("functionCall").filter(|c| c.is_object()) {
            self.current = None;
            let name = call.get("name").and_then(Value::as_str).unwrap_or("");
            let provided = call
                .get("id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty());
            let id = match provided {
                Some(id) if !self.tool_ids.contains(id) => id.to_owned(),
                _ => format!(
                    "{name}_{}_{}",
                    crate::util::now_ms(),
                    TOOL_CALL_COUNTER.fetch_add(1, Ordering::Relaxed) + 1
                ),
            };
            self.tool_ids.insert(id.clone());
            let index = e.start_tool_call(&id, name);
            let args = match call.get("args") {
                Some(v) if !v.is_null() => v.clone(),
                _ => json!({}),
            };
            e.tool_args_value(index, args);
            if let Some(sig) = signature {
                e.block_extra(index, "thoughtSignature", Value::String(sig.to_owned()));
            }
        }
    }
}

/// Send a `streamGenerateContent?alt=sse` request and normalize the stream.
pub(crate) async fn stream(
    ctx: &StreamCtx<'_>,
    url: &str,
    headers: &Headers,
    body: &Value,
) -> Message {
    let mut e = ctx.emitter();
    let headers = match headers.to_header_map() {
        Ok(h) => h,
        Err(err) => return ctx.failed(e, err),
    };
    let resp = match post_json(ctx, url, headers, body).await {
        Ok(r) => r,
        Err(err) => return ctx.failed(e, err),
    };
    let mut state = GoogleStream::default();
    let idle = idle_timeout(ctx.req, http::DEFAULT_IDLE_TIMEOUT);
    let read = http::read_sse(resp, idle, &ctx.cancel, |event| {
        state.on_event(&mut e, event)
    })
    .await;
    if let Some(raw) = &state.raw_stop {
        e.message_extra("rawStopReason", Value::String(raw.clone()));
    }
    if ctx.cancel.is_cancelled() {
        return ctx.failed(e, "Request was aborted");
    }
    match read {
        Err(ReadError::Aborted) => return ctx.failed(e, "Request was aborted"),
        Err(ReadError::Failed(text)) => return ctx.failed(e, text),
        Ok(()) => {}
    }
    if let Some(err) = state.error {
        return ctx.failed(e, err);
    }
    match state.stop {
        None => ctx.failed(e, "Google stream ended without a finish reason"),
        Some(StopReason::Error) => {
            let text = state
                .raw_stop
                .map(|r| format!("Provider stopped with: {r}"))
                .unwrap_or_else(|| "An unknown error occurred".into());
            ctx.failed(e, text)
        }
        Some(stop) => e.finish(ctx.req, stop, None),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::time::Duration;

    use super::*;
    use crate::provider::api::common::schema::make_strict_json_schema;
    use crate::provider::api::common::transform::NON_VISION_USER_IMAGE_PLACEHOLDER;
    use crate::provider::api::common::{adjust_max_tokens_for_thinking, estimate_context_tokens};
    use crate::provider::{CacheRetention, Credential};
    use pipkin_engine_protocol::Usage;

    pub(crate) fn model(v: Value) -> Model {
        serde_json::from_value(v).unwrap()
    }

    pub(crate) fn request(model: Model) -> ProviderRequest {
        ProviderRequest {
            model,
            system: "You are helpful.".into(),
            messages: vec![],
            tools: vec![],
            thinking: ThinkingLevel::Off,
            session_id: "sess-1".into(),
            max_tokens: None,
            cache_retention: CacheRetention::Short,
            credential: Credential::ApiKey("test-key".into()),
            headers: BTreeMap::new(),
            base_url: None,
            timeout: Duration::from_secs(10),
            env: HashMap::new(),
            options: Map::new(),
        }
    }

    pub(crate) fn assistant(provider: &str, api: &str, id: &str, content: Vec<Block>) -> Message {
        Message::Assistant {
            content,
            api: api.into(),
            provider: provider.into(),
            model: id.into(),
            usage: Usage::default(),
            stop_reason: StopReason::ToolUse,
            error_message: None,
            timestamp: 2,
            extra: Map::new(),
        }
    }

    pub(crate) fn tool_result(id: &str, name: &str, content: Vec<Block>, err: bool) -> Message {
        Message::ToolResult {
            tool_call_id: id.into(),
            tool_name: name.into(),
            content,
            is_error: err,
            timestamp: 3,
            extra: Map::new(),
        }
    }

    #[test]
    fn transform_downgrades_images_and_foreign_reasoning_and_closes_orphans() {
        let m = model(json!({"id": "m", "api": "x", "provider": "p", "input": ["text"]}));
        let messages = vec![
            Message::User {
                content: vec![
                    Block::image("image/png", "AA"),
                    Block::image("image/png", "BB"),
                ],
                timestamp: 1,
                extra: Map::new(),
            },
            assistant(
                "other",
                "y",
                "z",
                vec![
                    Block::Thinking {
                        thinking: "hmm".into(),
                        signature: Some("sig".into()),
                        redacted: false,
                        extra: Map::new(),
                    },
                    Block::Thinking {
                        thinking: "x".into(),
                        signature: Some("enc".into()),
                        redacted: true,
                        extra: Map::new(),
                    },
                    Block::tool_call("call|1", "read", json!({})),
                ],
            ),
            Message::user_text("next", 4),
        ];
        let mut normalize = |id: &str, _: &Message| sanitize_tool_call_id(id);
        let out = transform_messages(&messages, &m, Some(&mut normalize));
        assert_eq!(
            out[0].content(),
            &[Block::text(NON_VISION_USER_IMAGE_PLACEHOLDER)]
        );
        assert_eq!(
            out[1].content(),
            &[
                Block::text("hmm"),
                Block::tool_call("call_1", "read", json!({}))
            ]
        );
        match &out[2] {
            Message::ToolResult {
                tool_call_id,
                is_error,
                ..
            } => {
                assert_eq!(tool_call_id, "call_1");
                assert!(*is_error);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(out.len(), 4);
    }

    #[test]
    fn strict_schema_conversion_follows_the_reference() {
        let schema = json!({"type": "object", "properties": {
            "a": {"type": "string"}, "b": {"type": "integer"}}, "required": ["a"]});
        let strict = make_strict_json_schema(&schema, None).unwrap();
        assert_eq!(
            strict,
            json!({"type": "object", "properties": {"a": {"type": "string"},
                "b": {"anyOf": [{"type": "integer"}, {"type": "null"}]}},
                "required": ["a", "b"], "additionalProperties": false})
        );
        assert!(make_strict_json_schema(&json!({"type": "object", "oneOf": []}), None).is_err());
    }

    #[test]
    fn thinking_budget_adjustment_matches_simple_options() {
        let m = model(json!({"id": "m", "maxTokens": 32000, "reasoning": true}));
        let req = request(m);
        assert_eq!(
            adjust_max_tokens_for_thinking(&req, Some(8000), ThinkingLevel::Medium),
            (16192, 8192)
        );
        assert_eq!(
            adjust_max_tokens_for_thinking(&req, Some(30000), ThinkingLevel::Max),
            (32000, 16384)
        );
        let small = request(model(
            json!({"id": "m", "maxTokens": 4000, "reasoning": true}),
        ));
        assert_eq!(
            adjust_max_tokens_for_thinking(&small, Some(4000), ThinkingLevel::High),
            (4000, 2976)
        );
    }

    #[test]
    fn google_thinking_levels_and_budgets() {
        let g3 = model(json!({"id": "gemini-3.1-pro-preview", "reasoning": true,
            "thinkingLevelMap": {"off": null, "minimal": null}}));
        assert!(uses_google_thinking_level(&g3));
        assert_eq!(
            disabled_thinking_config(&g3).unwrap(),
            json!({"thinkingLevel": "LOW"})
        );
        let gemma = model(json!({"id": "gemma-4-31b-it", "reasoning": true,
            "thinkingLevelMap": {"off": null, "minimal": "MINIMAL", "low": null, "medium": null, "high": "HIGH"}}));
        let mut req = request(gemma);
        req.thinking = ThinkingLevel::Medium;
        assert_eq!(
            thinking_config(&req, true).unwrap(),
            Some(json!({"includeThoughts": true, "thinkingLevel": "HIGH"}))
        );
        let flash = model(json!({"id": "gemini-2.5-flash-lite", "reasoning": true}));
        let mut req = request(flash);
        req.thinking = ThinkingLevel::Minimal;
        assert_eq!(google_budget(&req, ThinkingLevel::Minimal, true), 512);
        assert_eq!(google_budget(&req, ThinkingLevel::Minimal, false), 128);
        req.options
            .insert("thinkingBudgets".into(), json!({"minimal": 7}));
        assert_eq!(google_budget(&req, ThinkingLevel::Minimal, true), 7);
    }

    #[test]
    fn tool_ids_normalize_like_the_reference() {
        assert_eq!(sanitize_tool_call_id("a|b.c"), "a_b_c");
        assert_eq!(sanitize_tool_call_id("\u{1f600}"), "__");
        assert_eq!(sanitize_tool_call_id(&"x".repeat(100)).len(), 64);
    }

    #[test]
    fn context_estimate_prefers_the_last_usage() {
        let mut req = request(model(
            json!({"id": "m", "contextWindow": 10000, "maxTokens": 9000}),
        ));
        req.system = "x".repeat(400);
        req.messages = vec![Message::user_text("y".repeat(40), 1)];
        assert_eq!(estimate_context_tokens(&req), 110);
        assert_eq!(base_max_tokens(&req), 10000 - 110 - 4096);
        let mut a = assistant("p", "a", "m", vec![Block::text("ok")]);
        if let Message::Assistant { usage, .. } = &mut a {
            usage.total_tokens = 5000;
        }
        req.messages.push(a);
        req.messages.push(Message::user_text("z".repeat(8), 5));
        assert_eq!(estimate_context_tokens(&req), 5002);
        assert_eq!(base_max_tokens(&req), 902);
    }
}
