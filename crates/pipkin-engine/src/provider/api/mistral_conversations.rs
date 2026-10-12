//! `mistral-conversations`: Mistral's native chat-completions endpoint over SSE.
//!
//! Ported from the reference `api/mistral-conversations.ts`:
//! - `POST {baseUrl}/v1/chat/completions` with `authorization: Bearer`, `accept:
//!   text/event-stream`, model headers then request headers (a `None` value deletes), and
//!   `x-affinity: <sessionId>` when caching unless a header override names it.
//! - One whole-request timeout covering the body stream too (reference default 60 s; here the
//!   request's timeout, or 60 s when it is zero; `options.timeoutMs` overrides). No retry.
//! - Reasoning: models with a `thinkingLevelMap` send `reasoning_effort` (`map[level] ??
//!   "high"`, `map.off` when off); other reasoning models send `prompt_mode: "reasoning"`.
//! - Tool-call ids replayed across models become 9-character alphanumeric ids (deterministic
//!   hash with collision handling); thinking replays as a `thinking` content chunk.
//! - Errors keep the reference text: `Mistral API error (<status>): <body>`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use pipkin_engine_protocol::{Block, Message, StopReason};
use serde_json::{Map, Value, json};

use super::common::transform::NON_VISION_USER_IMAGE_PLACEHOLDER;
use super::common::{
    Headers, base_max_tokens, data_url, short_hash, transform_messages, truncate_error_text,
};
use crate::provider::http;
use crate::provider::sse::{SseDecoder, SseEvent};
use crate::provider::{
    ApiAdapter, BoxFuture, CacheRetention, Emitter, Model, ProviderRequest, StreamCtx,
    ThinkingLevel,
};

pub const API: &str = "mistral-conversations";
const TOOL_CALL_ID_LENGTH: usize = 9;
const MAX_ERROR_BODY_CHARS: usize = 4000;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

pub fn adapter() -> Arc<dyn ApiAdapter> {
    Arc::new(MistralConversations)
}

pub struct MistralConversations;

impl ApiAdapter for MistralConversations {
    fn api(&self) -> &'static str {
        API
    }

    fn stream<'a>(&'a self, ctx: StreamCtx<'a>) -> BoxFuture<'a, Message> {
        Box::pin(run(ctx))
    }
}

/// Reference `hasMistralHeaderOverride`: the model or request headers name `name` (any value,
/// even a deleting `null`).
fn has_header_override(req: &ProviderRequest, name: &str) -> bool {
    req.model
        .headers
        .keys()
        .chain(req.headers.keys())
        .any(|k| k.eq_ignore_ascii_case(name))
}

// ---------------------------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------------------------

fn derive_tool_call_id(id: &str, attempt: u32) -> String {
    let normalized: String = id.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if attempt == 0 && normalized.len() == TOOL_CALL_ID_LENGTH {
        return normalized;
    }
    let base = if normalized.is_empty() {
        id
    } else {
        &normalized
    };
    let seed = if attempt == 0 {
        base.to_owned()
    } else {
        format!("{base}:{attempt}")
    };
    short_hash(&seed)
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(TOOL_CALL_ID_LENGTH)
        .collect()
}

/// Per-request id normalizer: deterministic, collision-free within the request.
fn tool_call_id_normalizer() -> impl FnMut(&str) -> String {
    let mut ids: HashMap<String, String> = HashMap::new();
    let mut owners: HashMap<String, String> = HashMap::new();
    move |id: &str| {
        if let Some(existing) = ids.get(id) {
            return existing.clone();
        }
        let mut attempt = 0;
        loop {
            let candidate = derive_tool_call_id(id, attempt);
            match owners.get(&candidate) {
                Some(owner) if owner != id => attempt += 1,
                _ => {
                    ids.insert(id.to_owned(), candidate.clone());
                    owners.insert(candidate.clone(), id.to_owned());
                    return candidate;
                }
            }
        }
    }
}

fn caching(req: &ProviderRequest) -> bool {
    req.cache_retention != CacheRetention::None && !req.session_id.is_empty()
}

fn tool_result_text(text: &str, has_images: bool, vision: bool, is_error: bool) -> String {
    let trimmed = text.trim();
    let prefix = if is_error { "[tool error] " } else { "" };
    if !trimmed.is_empty() {
        let suffix = if has_images && !vision {
            "\n[tool image omitted: model does not support images]"
        } else {
            ""
        };
        return format!("{prefix}{trimmed}{suffix}");
    }
    if has_images {
        return if vision {
            format!("{prefix}(see attached image)")
        } else {
            format!("{prefix}(image omitted: model does not support images)")
        };
    }
    format!("{prefix}(no tool output)")
}

fn wire_messages(req: &ProviderRequest, messages: &[Message]) -> Vec<Value> {
    let vision = req.model.supports_images();
    let mut out = Vec::new();
    if !req.system.is_empty() {
        out.push(json!({"role": "system", "content": req.system}));
    }
    for msg in messages {
        match msg {
            Message::System(system) => {
                let text = crate::context::system::update_text(system);
                if !text.is_empty() {
                    out.push(json!({"role": "system", "content": text}));
                }
            }
            Message::User { content, .. } => {
                let had_images = content.iter().any(|b| matches!(b, Block::Image { .. }));
                let chunks: Vec<Value> = content
                    .iter()
                    .filter_map(|b| match b {
                        Block::Text { text, .. } => Some(json!({"type": "text", "text": text})),
                        Block::Image {
                            data, mime_type, ..
                        } if vision => Some(
                            json!({"type": "image_url", "image_url": data_url(mime_type, data)}),
                        ),
                        _ => None,
                    })
                    .collect();
                if !chunks.is_empty() {
                    out.push(json!({"role": "user", "content": chunks}));
                } else if had_images && !vision {
                    out.push(json!({"role": "user", "content": NON_VISION_USER_IMAGE_PLACEHOLDER}));
                }
            }
            Message::Assistant { content, .. } => {
                let mut parts = Vec::new();
                let mut calls = Vec::new();
                for block in content {
                    match block {
                        Block::Text { text, .. } if !text.trim().is_empty() => {
                            parts.push(json!({"type": "text", "text": text}));
                        }
                        Block::Thinking { thinking, .. } if !thinking.trim().is_empty() => {
                            parts.push(json!({
                                "type": "thinking",
                                "thinking": [{"type": "text", "text": thinking}],
                            }));
                        }
                        Block::ToolCall {
                            id,
                            name,
                            arguments,
                            ..
                        } => {
                            let args = if arguments.is_null() {
                                "{}".to_owned()
                            } else {
                                arguments.to_string()
                            };
                            calls.push(json!({
                                "id": id,
                                "type": "function",
                                "function": {"name": name, "arguments": args},
                                "index": 0,
                            }));
                        }
                        _ => {}
                    }
                }
                if parts.is_empty() && calls.is_empty() {
                    continue;
                }
                let mut m = json!({"role": "assistant", "prefix": false});
                if !parts.is_empty() {
                    m["content"] = Value::Array(parts);
                }
                if !calls.is_empty() {
                    m["tool_calls"] = Value::Array(calls);
                }
                out.push(m);
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
                let has_images = content.iter().any(|b| matches!(b, Block::Image { .. }));
                let mut chunks = vec![json!({
                    "type": "text",
                    "text": tool_result_text(&text, has_images, vision, *is_error),
                })];
                if vision {
                    for b in content {
                        if let Block::Image {
                            data, mime_type, ..
                        } = b
                        {
                            chunks.push(
                                json!({"type": "image_url", "image_url": data_url(mime_type, data)}),
                            );
                        }
                    }
                }
                out.push(json!({
                    "role": "tool",
                    "tool_call_id": tool_call_id,
                    "name": tool_name,
                    "content": chunks,
                }));
            }
        }
    }
    out
}

fn tool_choice(value: &Value) -> Option<Value> {
    match value {
        Value::String(s) if matches!(s.as_str(), "auto" | "none" | "any" | "required") => {
            Some(value.clone())
        }
        Value::Object(o) => {
            let name = o
                .get("function")
                .and_then(|f| f.get("name"))
                .or_else(|| o.get("name"))
                .and_then(Value::as_str)?;
            Some(json!({"type": "function", "function": {"name": name}}))
        }
        _ => None,
    }
}

/// `(prompt_mode, reasoning_effort)` per the reference `streamSimple`.
fn reasoning_controls(model: &Model, level: ThinkingLevel) -> (Option<String>, Option<String>) {
    let reasoning = match level {
        ThinkingLevel::Off => None,
        l => Some(model.clamp_thinking(l)).filter(|l| *l != ThinkingLevel::Off),
    };
    let has_map = model.reasoning && !model.thinking_level_map.is_empty();
    let effort = if has_map {
        match reasoning {
            Some(l) => Some(
                model
                    .thinking_value(l)
                    .map(str::to_owned)
                    .unwrap_or_else(|| "high".into()),
            ),
            None => model.thinking_value(ThinkingLevel::Off).map(str::to_owned),
        }
    } else {
        None
    };
    let prompt_mode =
        (model.reasoning && !has_map && reasoning.is_some()).then(|| "reasoning".into());
    (prompt_mode, effort)
}

/// The wire body (snake_case), as sent.
pub(crate) fn build_payload(req: &ProviderRequest) -> Result<Value, String> {
    let resolved =
        req.resolve_systems(req.model.compat_bool("supportsMidConvoSystemMessages") == Some(true));
    let req = resolved.as_ref();
    let mut ids = tool_call_id_normalizer();
    let mut normalize = |id: &str, _: &Message| ids(id);
    let messages = transform_messages(&req.messages, &req.model, Some(&mut normalize));
    let mut body = Map::new();
    body.insert("model".into(), json!(req.model.id));
    body.insert("stream".into(), json!(true));
    body.insert(
        "messages".into(),
        Value::Array(wire_messages(req, &messages)),
    );
    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                let strict = super::common::schema::resolve_strict(req, t, true, None)?;
                Ok(json!({"type": "function", "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": super::common::schema::tool_parameters(t, strict)?,
                    "strict": strict.unwrap_or(false),
                }}))
            })
            .collect::<Result<_, String>>()?;
        body.insert("tools".into(), Value::Array(tools));
    }
    if let Some(t) = req.options.get("temperature").filter(|v| v.is_number()) {
        body.insert("temperature".into(), t.clone());
    }
    body.insert(
        "max_tokens".into(),
        // Reference `buildBaseOptions`: the cap is clamped to what fits in the context window.
        json!(base_max_tokens(req)),
    );
    if let Some(choice) = req.options.get("toolChoice").and_then(tool_choice) {
        body.insert("tool_choice".into(), choice);
    }
    let (prompt_mode, effort) = reasoning_controls(&req.model, req.thinking);
    if let Some(mode) = prompt_mode {
        body.insert("prompt_mode".into(), json!(mode));
    }
    if let Some(effort) = effort {
        body.insert("reasoning_effort".into(), json!(effort));
    }
    if caching(req) {
        body.insert("prompt_cache_key".into(), json!(req.session_id));
    }
    Ok(Value::Object(body))
}

pub(crate) fn build_headers(req: &ProviderRequest, key: &str) -> Headers {
    let mut headers = Headers::default();
    headers.set("accept", "text/event-stream");
    headers.set("authorization", format!("Bearer {key}"));
    headers.set("content-type", "application/json");
    headers.merge(&req.model.headers);
    headers.merge_optional(&req.headers);
    if caching(req) && !has_header_override(req, "x-affinity") {
        headers.set("x-affinity", req.session_id.clone());
    }
    headers
}

fn endpoint(req: &ProviderRequest) -> String {
    format!(
        "{}/v1/chat/completions",
        req.base_url().trim_end_matches('/')
    )
}

fn whole_request_timeout(req: &ProviderRequest) -> Duration {
    if let Some(ms) = req.options.get("timeoutMs").and_then(Value::as_u64) {
        return Duration::from_millis(ms);
    }
    if req.timeout.is_zero() {
        DEFAULT_TIMEOUT
    } else {
        req.timeout
    }
}

// ---------------------------------------------------------------------------------------------
// Stream
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Current {
    None,
    Text(usize),
    Thinking(usize),
}

#[derive(Hash, PartialEq, Eq, Clone)]
enum ToolKey {
    Index(u64),
    Id(String),
}

struct State {
    current: Current,
    tools: HashMap<ToolKey, usize>,
    stop: Option<StopReason>,
    stop_error: Option<String>,
    response_id: bool,
}

const TIMEOUT_MESSAGE: &str = "The operation was aborted due to timeout";

fn mistral_stop(reason: &str) -> (StopReason, Option<String>) {
    match reason {
        "stop" => (StopReason::Stop, None),
        "length" | "model_length" => (StopReason::Length, None),
        "tool_calls" => (StopReason::ToolUse, None),
        other => (
            StopReason::Error,
            Some(format!("Provider stopped with: {other}")),
        ),
    }
}

fn cached_prompt_tokens(usage: &Value, prompt: u64) -> u64 {
    let raw = usage
        .pointer("/promptTokensDetails/cachedTokens")
        .or_else(|| usage.pointer("/prompt_tokens_details/cached_tokens"))
        .or_else(|| usage.pointer("/promptTokenDetails/cachedTokens"))
        .or_else(|| usage.pointer("/prompt_token_details/cached_tokens"))
        .or_else(|| usage.get("numCachedTokens"))
        .or_else(|| usage.get("num_cached_tokens"))
        .filter(|v| !v.is_null());
    let cached = raw.and_then(Value::as_f64).unwrap_or(0.0).max(0.0) as u64;
    cached.min(prompt)
}

/// Apply one SSE event. `Err` is a stream error; `Ok(true)` means `[DONE]`.
fn on_event(e: &mut Emitter, st: &mut State, event: &SseEvent) -> Result<bool, String> {
    let data = event.data.trim();
    if data.is_empty() {
        return Ok(false);
    }
    if data == "[DONE]" {
        return Ok(true);
    }
    let chunk: Value = serde_json::from_str(data).map_err(|err| err.to_string())?;
    let Some(choices) = chunk.get("choices").and_then(Value::as_array) else {
        return Err("Invalid Mistral streaming event".into());
    };
    if !st.response_id
        && let Some(id) = chunk.get("id").and_then(Value::as_str)
        && !id.is_empty()
    {
        e.set_response_id(id);
        st.response_id = true;
    }
    if let Some(usage) = chunk.get("usage").filter(|u| u.is_object()) {
        let num = |k: &str| usage.get(k).and_then(Value::as_u64).unwrap_or(0);
        let prompt = num("prompt_tokens");
        let cached = cached_prompt_tokens(usage, prompt);
        let u = e.usage_mut();
        u.input = prompt.saturating_sub(cached);
        u.output = num("completion_tokens");
        u.cache_read = cached;
        u.cache_write = 0;
        u.total_tokens = match num("total_tokens") {
            0 => u.input + u.output + u.cache_read,
            n => n,
        };
    }
    let Some(choice) = choices.first() else {
        return Ok(false);
    };
    if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
        e.message_extra("rawStopReason", json!(reason));
        let (stop, error) = mistral_stop(reason);
        st.stop = Some(stop);
        if error.is_some() {
            st.stop_error = error;
        }
    }
    let delta = choice.get("delta").cloned().unwrap_or(Value::Null);
    let items: Vec<Value> = match delta.get("content") {
        Some(Value::String(s)) => vec![Value::String(s.clone())],
        Some(Value::Array(a)) => a.clone(),
        _ => vec![],
    };
    for item in items {
        let (is_thinking, text) = match &item {
            Value::String(s) => (false, s.clone()),
            Value::Object(o) if o.get("type").and_then(Value::as_str) == Some("thinking") => (
                true,
                o.get("thinking")
                    .and_then(Value::as_array)
                    .map(|parts| {
                        parts
                            .iter()
                            .filter_map(|p| p.get("text").and_then(Value::as_str))
                            .collect::<String>()
                    })
                    .unwrap_or_default(),
            ),
            Value::Object(o) if o.get("type").and_then(Value::as_str) == Some("text") => (
                false,
                o.get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            ),
            _ => continue,
        };
        // Empty deltas (GLM on Mistral) must not split blocks.
        if text.is_empty() {
            continue;
        }
        if is_thinking {
            let index = match st.current {
                Current::Thinking(i) => i,
                _ => e.start_thinking(),
            };
            st.current = Current::Thinking(index);
            e.thinking(index, &text);
        } else {
            let index = match st.current {
                Current::Text(i) => i,
                _ => e.start_text(),
            };
            st.current = Current::Text(index);
            e.text(index, &text);
        }
    }
    if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            st.current = Current::None;
            let position = call.get("index").and_then(Value::as_u64);
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty() && *id != "null")
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    derive_tool_call_id(&format!("toolcall:{}", position.unwrap_or(0)), 0)
                });
            let key = position.map_or_else(|| ToolKey::Id(id.clone()), ToolKey::Index);
            let function = call.get("function").cloned().unwrap_or(Value::Null);
            let index = match st.tools.get(&key) {
                Some(i) => *i,
                None => {
                    let name = function.get("name").and_then(Value::as_str).unwrap_or("");
                    let i = e.start_tool_call(&id, name);
                    st.tools.insert(key, i);
                    i
                }
            };
            let args = match function.get("arguments") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Null) | None => "{}".into(),
                Some(other) => other.to_string(),
            };
            e.tool_args(index, &args);
        }
    }
    Ok(false)
}

async fn run(ctx: StreamCtx<'_>) -> Message {
    let req = ctx.req;
    let mut e = ctx.emitter();
    let Some(key) = req.credential.secret().map(str::to_owned) else {
        return ctx.failed(
            e,
            format!("No API key for provider: {}", req.model.provider),
        );
    };
    let body = match build_payload(req) {
        Ok(v) => v,
        Err(m) => return ctx.failed(e, m),
    };
    let headers = match build_headers(req, &key).to_header_map() {
        Ok(h) => h,
        Err(m) => return ctx.failed(e, m),
    };
    let deadline = tokio::time::Instant::now() + whole_request_timeout(req);

    let send = ctx
        .http
        .post(endpoint(req))
        .headers(headers)
        .json(&body)
        .send();
    let resp = tokio::select! {
        _ = ctx.cancel.cancelled() => return ctx.failed(e, "Request was aborted"),
        _ = tokio::time::sleep_until(deadline) => return ctx.failed(e, TIMEOUT_MESSAGE),
        r = send => r,
    };
    let resp = match resp {
        Ok(r) => r,
        Err(err) => return ctx.failed(e, http::transport_error(&err)),
    };
    let status = resp.status();
    if !status.is_success() {
        let text = tokio::select! {
            _ = ctx.cancel.cancelled() => return ctx.failed(e, "Request was aborted"),
            _ = tokio::time::sleep_until(deadline) => return ctx.failed(e, TIMEOUT_MESSAGE),
            t = resp.text() => t.unwrap_or_default(),
        };
        let text = text.trim();
        let message = if text.is_empty() {
            let reason = status
                .canonical_reason()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("Request failed with status {}", status.as_u16()));
            format!("Mistral API error ({}): {reason}", status.as_u16())
        } else {
            format!(
                "Mistral API error ({}): {}",
                status.as_u16(),
                truncate_error_text(text, MAX_ERROR_BODY_CHARS)
            )
        };
        return ctx.failed(e, message);
    }

    let mut st = State {
        current: Current::None,
        tools: HashMap::new(),
        stop: None,
        stop_error: None,
        response_id: false,
    };
    let mut decoder = SseDecoder::new();
    let mut stream = resp.bytes_stream();
    loop {
        let next = tokio::select! {
            _ = ctx.cancel.cancelled() => return ctx.failed(e, "Request was aborted"),
            _ = tokio::time::sleep_until(deadline) => return ctx.failed(e, TIMEOUT_MESSAGE),
            n = stream.next() => n,
        };
        let (events, last) = match next {
            None => (decoder.finish().into_iter().collect::<Vec<_>>(), true),
            Some(Err(err)) => return ctx.failed(e, http::transport_error(&err)),
            Some(Ok(bytes)) => match decoder.push(&bytes) {
                Ok(events) => (events, false),
                Err(err) => return ctx.failed(e, err),
            },
        };
        let mut done = last;
        for event in &events {
            match on_event(&mut e, &mut st, event) {
                Err(err) => return ctx.failed(e, err),
                Ok(true) => {
                    done = true;
                    break;
                }
                Ok(false) => {}
            }
        }
        if done {
            break;
        }
    }
    if ctx.cancel.is_cancelled() {
        return ctx.failed(e, "Request was aborted");
    }
    match st.stop {
        None => ctx.failed(e, "Mistral stream ended without a finish reason"),
        Some(StopReason::Error) => {
            let message = st
                .stop_error
                .unwrap_or_else(|| "An unknown error occurred".into());
            ctx.failed(e, message)
        }
        Some(stop) => e.finish(req, stop, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::mock::{MockResponse, MockServer};
    use crate::provider::{Credential, StreamEvent, ToolSpec};
    use pipkin_engine_protocol::Usage;
    use std::collections::BTreeMap;
    use tokio_util::sync::CancellationToken;

    fn model(extra: Value) -> Model {
        let mut base = json!({
            "id": "mistral-large-latest",
            "name": "Mistral Large",
            "api": API,
            "provider": "mistral",
            "baseUrl": "https://api.mistral.ai",
            "input": ["text", "image"],
            "cost": {"input": 2.0, "output": 6.0, "cacheRead": 0.2},
            "contextWindow": 128000,
            "maxTokens": 4096
        });
        if let (Value::Object(b), Value::Object(x)) = (&mut base, extra) {
            b.extend(x);
        }
        serde_json::from_value(base).unwrap()
    }

    fn request(model: Model) -> ProviderRequest {
        ProviderRequest {
            model,
            system: "Be brief.".into(),
            messages: vec![Message::user_text("hi", 1)],
            tools: vec![],
            thinking: ThinkingLevel::Off,
            session_id: "session-1".into(),
            max_tokens: Some(123),
            cache_retention: CacheRetention::Short,
            credential: Credential::ApiKey("secret".into()),
            headers: BTreeMap::new(),
            base_url: None,
            timeout: Duration::from_secs(5),
            env: HashMap::new(),
            options: Map::new(),
        }
    }

    async fn exchange(
        req: &ProviderRequest,
        cancel: CancellationToken,
    ) -> (Message, Vec<StreamEvent>) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let http = http::client();
        let msg = MistralConversations
            .stream(StreamCtx {
                req,
                http: &http,
                events: Some(tx),
                cancel,
            })
            .await;
        let mut events = vec![];
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        (msg, events)
    }

    fn sse(chunks: &[Value]) -> String {
        let mut s = String::new();
        for c in chunks {
            s.push_str(&format!("data: {c}\n\n"));
        }
        s.push_str("data: [DONE]\n\n");
        s
    }

    fn split_every(s: &str, n: usize) -> Vec<String> {
        let b = s.as_bytes();
        b.chunks(n)
            .map(|c| unsafe { String::from_utf8_unchecked(c.to_vec()) })
            .collect()
    }

    #[test]
    fn short_hash_matches_the_reference_values() {
        // Computed with the reference implementation (utils/hash.ts) under Node.
        assert_eq!(short_hash(""), "k4n83c7h0j2b");
        assert_eq!(short_hash("call_abc"), "py36n51ekigv6");
        assert_eq!(short_hash("h\u{e9}llo \u{1f600}"), "1t90xbiinqdrp");
        assert_eq!(derive_tool_call_id("call|from|openai_xyz", 0), "1jdyro11m");
        assert_eq!(derive_tool_call_id("call|from|openai_xyz", 1), "1mszk1t1w");
        assert_eq!(derive_tool_call_id("toolcall:0", 0), "toolcall0");
        assert_eq!(derive_tool_call_id("abc-def-ghi", 1), "mlj703uel");
    }

    #[test]
    fn normalizer_avoids_collisions() {
        let mut n = tool_call_id_normalizer();
        let a = n("abc-def-ghi");
        let b = n("abcdefghi");
        assert_eq!(a, "abcdefghi");
        assert_ne!(a, b);
        assert_eq!(n("abc-def-ghi"), a);
    }

    #[test]
    fn reasoning_controls_follow_the_reference() {
        let magistral = model(json!({"reasoning": true}));
        assert_eq!(
            reasoning_controls(&magistral, ThinkingLevel::High),
            (Some("reasoning".into()), None)
        );
        assert_eq!(
            reasoning_controls(&magistral, ThinkingLevel::Off),
            (None, None)
        );
        let mapped = model(json!({"reasoning": true, "thinkingLevelMap": {
            "off": "none", "minimal": null, "low": null, "medium": "high", "high": "high"
        }}));
        assert_eq!(
            reasoning_controls(&mapped, ThinkingLevel::Off),
            (None, Some("none".into()))
        );
        assert_eq!(
            reasoning_controls(&mapped, ThinkingLevel::Minimal),
            (None, Some("high".into()))
        );
        assert_eq!(
            reasoning_controls(&mapped, ThinkingLevel::Medium),
            (None, Some("high".into()))
        );
        let plain = model(json!({}));
        assert_eq!(
            reasoning_controls(&plain, ThinkingLevel::High),
            (None, None)
        );
    }

    #[test]
    fn golden_request_body() {
        let mut req = request(model(json!({"reasoning": true})));
        req.thinking = ThinkingLevel::High;
        req.options.insert("temperature".into(), json!(0.5));
        req.options.insert(
            "toolChoice".into(),
            json!({"type": "function", "function": {"name": "read"}}),
        );
        req.tools = vec![ToolSpec {
            constrained_sampling: None,
            name: "read".into(),
            description: "Read a file".into(),
            parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        }];
        req.messages = vec![
            Message::User {
                content: vec![Block::text("look"), Block::image("image/png", "AAAA")],
                timestamp: 1,
                extra: Map::new(),
            },
            Message::Assistant {
                content: vec![
                    Block::Thinking {
                        thinking: "hmm".into(),
                        signature: None,
                        redacted: false,
                        extra: Map::new(),
                    },
                    Block::text("ok"),
                    Block::tool_call("call|from|openai_xyz", "read", json!({"path": "a"})),
                ],
                api: "openai-responses".into(),
                provider: "openai".into(),
                model: "gpt".into(),
                usage: Usage::default(),
                stop_reason: StopReason::ToolUse,
                error_message: None,
                timestamp: 2,
                extra: Map::new(),
            },
            Message::ToolResult {
                tool_call_id: "call|from|openai_xyz".into(),
                tool_name: "read".into(),
                content: vec![Block::text("contents")],
                is_error: true,
                timestamp: 3,
                extra: Map::new(),
            },
        ];
        let body = build_payload(&req).unwrap();
        let id = "1jdyro11m";
        assert_eq!(
            body,
            json!({
                "model": "mistral-large-latest",
                "stream": true,
                "messages": [
                    {"role": "system", "content": "Be brief."},
                    {"role": "user", "content": [
                        {"type": "text", "text": "look"},
                        {"type": "image_url", "image_url": "data:image/png;base64,AAAA"}
                    ]},
                    {"role": "assistant", "prefix": false, "content": [
                        {"type": "text", "text": "hmm"},
                        {"type": "text", "text": "ok"}
                    ], "tool_calls": [{"id": id, "type": "function",
                        "function": {"name": "read", "arguments": "{\"path\":\"a\"}"}, "index": 0}]},
                    {"role": "tool", "tool_call_id": id, "name": "read",
                        "content": [{"type": "text", "text": "[tool error] contents"}]}
                ],
                "tools": [{"type": "function", "function": {"name": "read",
                    "description": "Read a file",
                    "parameters": {"type": "object", "properties": {"path": {"type": "string"}}},
                    "strict": false}}],
                "temperature": 0.5,
                "max_tokens": 123,
                "tool_choice": {"type": "function", "function": {"name": "read"}},
                "prompt_mode": "reasoning",
                "prompt_cache_key": "session-1"
            })
        );
    }

    #[test]
    fn transform_closes_orphans_and_skips_failed_turns() {
        let m = model(json!({"input": ["text"]}));
        let assistant = |stop, calls: Vec<Block>| Message::Assistant {
            content: calls,
            api: API.into(),
            provider: "mistral".into(),
            model: "mistral-large-latest".into(),
            usage: Usage::default(),
            stop_reason: stop,
            error_message: None,
            timestamp: 0,
            extra: Map::new(),
        };
        let msgs = vec![
            Message::User {
                content: vec![
                    Block::image("image/png", "x"),
                    Block::image("image/png", "y"),
                ],
                timestamp: 0,
                extra: Map::new(),
            },
            assistant(
                StopReason::ToolUse,
                vec![Block::tool_call("a", "t", json!({}))],
            ),
            assistant(StopReason::Error, vec![Block::text("partial")]),
            Message::user_text("next", 0),
        ];
        let out = transform_messages(&msgs, &m, None);
        assert_eq!(out.len(), 4);
        assert!(
            matches!(&out[0], Message::User { content, .. } if content == &vec![Block::text(NON_VISION_USER_IMAGE_PLACEHOLDER)])
        );
        assert!(
            matches!(&out[2], Message::ToolResult { tool_call_id, is_error: true, .. } if tool_call_id == "a")
        );
    }

    #[tokio::test]
    async fn golden_exchange_streams_text_thinking_tools_and_usage() {
        let chunks = [
            json!({"id": "resp-1", "choices": [{"delta": {"content": [{"type": "thinking", "thinking": [{"type": "text", "text": "Let me thínk"}]}]}}]}),
            json!({"id": "resp-2", "choices": [{"delta": {"content": ""}}]}),
            json!({"choices": [{"delta": {"content": "Hé 😀"}}]}),
            json!({"choices": [{"delta": {"content": [{"type": "text", "text": " there"}]}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"id": "null", "index": 0, "function": {"name": "read", "arguments": "{\"pa"}}]}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "function": {"name": "read", "arguments": "th\":\"x\"}"}}]}}]}),
            json!({"usage": {"prompt_tokens": 100, "completion_tokens": 20, "total_tokens": 120, "prompt_tokens_details": {"cached_tokens": 40}},
                   "choices": [{"delta": {}, "finish_reason": "tool_calls"}]}),
        ];
        let body = sse(&chunks);
        let server = MockServer::start(move |_, _| MockResponse::sse(split_every(&body, 7))).await;
        let mut req = request(model(json!({})));
        req.base_url = Some(format!("{}/", server.url));
        req.headers.insert("X-Custom".into(), Some("value".into()));
        req.headers.insert("accept".into(), None);
        let (msg, events) = exchange(&req, CancellationToken::new()).await;

        let recorded = &server.requests()[0];
        assert_eq!(recorded.method, "POST");
        assert_eq!(recorded.path, "/v1/chat/completions");
        assert_eq!(recorded.header("authorization"), Some("Bearer secret"));
        assert_eq!(recorded.header("x-affinity"), Some("session-1"));
        assert_eq!(recorded.header("x-custom"), Some("value"));
        assert_eq!(recorded.header("content-type"), Some("application/json"));
        assert_eq!(recorded.json()["max_tokens"], json!(123));

        let Message::Assistant {
            content,
            usage,
            stop_reason,
            extra,
            ..
        } = &msg
        else {
            panic!()
        };
        assert_eq!(*stop_reason, StopReason::ToolUse, "{msg:?}");
        assert_eq!(content.len(), 3);
        assert!(
            matches!(&content[0], Block::Thinking { thinking, .. } if thinking == "Let me thínk")
        );
        assert_eq!(content[1], Block::text("Hé 😀 there"));
        assert!(
            matches!(&content[2], Block::ToolCall { id, name, arguments, .. }
                if id == "toolcall0" && name == "read" && *arguments == json!({"path": "x"}))
        );
        assert_eq!((usage.input, usage.output, usage.cache_read), (60, 20, 40));
        assert_eq!(usage.total_tokens, 120);
        assert!((usage.cost.input - 60.0 * 2.0 / 1e6).abs() < 1e-12);
        assert_eq!(extra["responseId"], json!("resp-1"));
        assert_eq!(extra["rawStopReason"], json!("tool_calls"));
        assert!(!events.is_empty());
    }

    #[tokio::test]
    async fn http_errors_keep_the_reference_format() {
        let server = MockServer::start(|_, _| {
            MockResponse::json(429, &json!({"message": "Rate limit exceeded"}))
        })
        .await;
        let mut req = request(model(json!({})));
        req.base_url = Some(server.url.clone());
        let (msg, _) = exchange(&req, CancellationToken::new()).await;
        let Message::Assistant {
            stop_reason,
            error_message,
            ..
        } = &msg
        else {
            panic!()
        };
        assert_eq!(*stop_reason, StopReason::Error);
        assert_eq!(
            error_message.as_deref(),
            Some("Mistral API error (429): {\"message\":\"Rate limit exceeded\"}")
        );
        assert!(crate::provider::errors::is_retryable(&msg));
        assert_eq!(server.requests().len(), 1, "no request-level retry");
    }

    #[tokio::test]
    async fn missing_key_unknown_finish_and_unterminated_streams_are_errors() {
        let mut req = request(model(json!({})));
        req.credential = Credential::None;
        let (msg, _) = exchange(&req, CancellationToken::new()).await;
        assert!(
            matches!(&msg, Message::Assistant { error_message: Some(e), .. } if e == "No API key for provider: mistral")
        );

        let body = sse(&[
            json!({"choices": [{"delta": {"content": "x"}, "finish_reason": "content_filter"}]}),
        ]);
        let server = MockServer::start(move |_, _| MockResponse::sse(vec![body.clone()])).await;
        let mut req = request(model(json!({})));
        req.base_url = Some(server.url.clone());
        let (msg, _) = exchange(&req, CancellationToken::new()).await;
        assert!(
            matches!(&msg, Message::Assistant { stop_reason: StopReason::Error, error_message: Some(e), .. } if e == "Provider stopped with: content_filter")
        );

        let server = MockServer::start(|_, _| {
            MockResponse::sse(vec![
                "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n".into(),
            ])
        })
        .await;
        req.base_url = Some(server.url.clone());
        let (msg, _) = exchange(&req, CancellationToken::new()).await;
        assert!(
            matches!(&msg, Message::Assistant { error_message: Some(e), .. } if e == "Mistral stream ended without a finish reason"),
            "{msg:?}"
        );

        let server =
            MockServer::start(|_, _| MockResponse::sse(vec!["data: {\"nope\":1}\n\n".into()]))
                .await;
        req.base_url = Some(server.url.clone());
        let (msg, _) = exchange(&req, CancellationToken::new()).await;
        assert!(
            matches!(&msg, Message::Assistant { error_message: Some(e), .. } if e == "Invalid Mistral streaming event")
        );
    }

    #[tokio::test]
    async fn cancellation_and_whole_request_timeout() {
        let server = MockServer::start(|_, _| {
            let mut r = MockResponse::sse(vec![
                "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n".into(),
            ]);
            r.hang = true;
            r
        })
        .await;
        let mut req = request(model(json!({})));
        req.base_url = Some(server.url.clone());
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            c2.cancel();
        });
        let (msg, _) = exchange(&req, cancel).await;
        let Message::Assistant {
            stop_reason,
            content,
            error_message,
            ..
        } = &msg
        else {
            panic!()
        };
        assert_eq!(*stop_reason, StopReason::Aborted);
        assert_eq!(error_message, &None);
        assert_eq!(content, &vec![Block::text("partial")]);

        // The whole-request timeout also bounds a stream that keeps the connection open.
        req.options.insert("timeoutMs".into(), json!(200));
        let (msg, _) = exchange(&req, CancellationToken::new()).await;
        assert!(
            matches!(&msg, Message::Assistant { stop_reason: StopReason::Error, error_message: Some(e), .. } if e == TIMEOUT_MESSAGE)
        );
        assert!(crate::provider::errors::is_retryable(&msg));
    }

    /// Reference `clampMaxTokensToContext` (`simple-options.ts:18-22`) through
    /// `buildBaseOptions`: 10000 window - 1100 estimated - 4096 safety = 4804.
    fn crowd_context(req: &mut ProviderRequest) {
        req.system = "x".repeat(4000);
        req.messages = vec![Message::user_text("y".repeat(400), 1)];
    }

    #[test]
    fn max_tokens_are_clamped_to_the_context_window() {
        let mut req = request(model(json!({"contextWindow": 10000, "maxTokens": 32000})));
        req.max_tokens = None;
        crowd_context(&mut req);
        assert_eq!(build_payload(&req).unwrap()["max_tokens"], json!(4804));
    }
}
