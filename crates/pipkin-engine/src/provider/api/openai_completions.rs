//! `openai-completions`: OpenAI Chat Completions and the many OpenAI-compatible endpoints
//! (reference `api/openai-completions.ts`).
//!
//! Endpoint differences are resolved by [`Compat`]: detected from the provider id and base URL
//! (`detectCompat`), then overridden key by key by `model.compat`. Thinking is sent in one of
//! eleven `thinkingFormat` shapes. The stream keeps one text block and one thinking block for
//! the whole response (later deltas append to them), tool calls keyed by stream index then id,
//! and reasoning replay data (`reasoning_details`, or the name of the raw reasoning field) in
//! the thinking block's signature.

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::sync::Arc;

use pipkin_engine_protocol::{Block, Message, StopReason};
use serde_json::{Map, Value, json};

use super::common::{
    Mapped, NO_TIMEOUT, ToolArgs, base_max_tokens, build_headers,
    clamp_thinking_budget_to_answer_room, data_url, finalize_tool_args, has_header, idle_timeout,
    js_truthy, mapped_level, post_json, reasoning_effort, resolve_sampling_params,
    sanitize_id_chars, short_hash, take_chars, thinking_budget_for_level, transform_messages,
};
use crate::provider::model::ThinkingLevel;
use crate::provider::sse::SseEvent;
use crate::provider::{
    ApiAdapter, BoxFuture, CacheRetention, Emitter, Model, ProviderRequest, StreamCtx, ToolSpec,
    http,
};

pub const API: &str = "openai-completions";

pub struct OpenAICompletions;

pub fn adapter() -> Arc<dyn ApiAdapter> {
    Arc::new(OpenAICompletions)
}

impl ApiAdapter for OpenAICompletions {
    fn api(&self) -> &'static str {
        API
    }

    fn stream<'a>(&'a self, ctx: StreamCtx<'a>) -> BoxFuture<'a, Message> {
        Box::pin(run(ctx))
    }
}

// ---------------------------------------------------------------------------------------------
// Shared OpenAI helpers (also used by the Responses adapters)
// ---------------------------------------------------------------------------------------------

/// OpenAI prompt cache keys are limited to 64 characters.
pub(crate) fn clamp_prompt_cache_key(key: &str) -> Option<String> {
    if key.is_empty() {
        return None;
    }
    Some(take_chars(key, 64))
}

/// The key the OpenAI SDK sends: the credential's secret, else `"unused"` when the configured
/// headers carry authorization themselves (reference `getClientApiKey`).
pub(crate) fn client_api_key(req: &ProviderRequest) -> Result<String, String> {
    if let Some(secret) = req.credential.secret().filter(|s| !s.is_empty()) {
        return Ok(secret.to_owned());
    }
    if has_header(&req.headers, "authorization") || has_header(&req.headers, "cf-aig-authorization")
    {
        return Ok("unused".into());
    }
    Err(format!("No API key for provider: {}", req.model.provider))
}

/// GitHub Copilot's per-request headers (reference `github-copilot-headers.ts`).
pub(crate) fn copilot_headers(messages: &[Message]) -> Vec<(&'static str, String)> {
    let initiator = match messages.last() {
        Some(Message::User { .. }) | None => "user",
        Some(_) => "agent",
    };
    let mut out = vec![
        ("X-Initiator", initiator.to_owned()),
        ("Openai-Intent", "conversation-edits".to_owned()),
    ];
    let has_images = messages.iter().any(|m| match m {
        Message::User { content, .. } | Message::ToolResult { content, .. } => {
            content.iter().any(|b| matches!(b, Block::Image { .. }))
        }
        Message::Assistant { .. } | Message::System(_) => false,
    });
    if has_images {
        out.push(("Copilot-Vision-Request", "true".to_owned()));
    }
    out
}

/// Format a failed exchange the way the reference formats an OpenAI SDK error
/// (`formatProviderError(normalizeProviderError(e), prefix)`).
///
/// HTTP failures arrive from [`http::send`] as `"<status>: <body>"`. The SDK keeps the body's
/// `error` object as the error body (`"<status>: <error json>"`, or
/// `"<prefix> (<status>): <error json>"`); a non-JSON body becomes the SDK message
/// `"<status> <text>"`. OpenRouter's `error.metadata.raw` is appended when not already present.
/// Anything else (transport, stream) is returned unchanged.
pub(crate) fn sdk_error(text: &str, prefix: Option<&str>) -> String {
    let Some((status, rest)) = split_status(text) else {
        return text.to_owned();
    };
    let with_prefix = |body: &str| match prefix {
        Some(p) => format!("{p} ({status}): {body}"),
        None => format!("{status}: {body}"),
    };
    let message_only = |message: String| match prefix {
        Some(p) => format!("{p} ({status}): {message}"),
        None => message,
    };
    let Some(body) = rest else {
        return message_only(format!("{status} status code (no body)"));
    };
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let mut out = match parsed.as_ref().and_then(|v| v.get("error")) {
        Some(Value::Object(err)) if !err.is_empty() => {
            let error_body = Value::Object(err.clone()).to_string();
            let message = match err.get("message") {
                Some(Value::String(m)) => format!("{status} {m}"),
                Some(m) => format!("{status} {m}"),
                None => format!("{status} {error_body}"),
            };
            if message.contains(&error_body) {
                message_only(message)
            } else {
                with_prefix(&error_body)
            }
        }
        Some(other) if !other.is_null() => message_only(format!("{status} {other}")),
        // A JSON body without an `error` field: keep it (the SDK would drop it).
        _ if parsed.is_some() => with_prefix(body),
        _ => message_only(format!("{status} {body}")),
    };
    if let Some(raw) = parsed
        .as_ref()
        .and_then(|v| v.pointer("/error/metadata/raw"))
        .filter(|v| !v.is_null())
    {
        let raw = match raw {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        if !raw.is_empty() && !out.contains(&raw) {
            out.push('\n');
            out.push_str(&raw);
        }
    }
    out
}

/// `"<status>: <body>"` -> `(status, Some(body))`; `"<status> status code (no body)"` ->
/// `(status, None)`.
fn split_status(text: &str) -> Option<(u16, Option<&str>)> {
    let digits = text.get(..3)?;
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let status: u16 = digits.parse().ok()?;
    let rest = &text[3..];
    if let Some(body) = rest.strip_prefix(": ") {
        return Some((status, Some(body)));
    }
    if rest == " status code (no body)" {
        return Some((status, None));
    }
    None
}

// ---------------------------------------------------------------------------------------------
// Compat
// ---------------------------------------------------------------------------------------------

/// Resolved compatibility settings (reference `getCompat`).
#[derive(Clone, Debug, PartialEq)]
pub struct Compat {
    pub supports_store: bool,
    pub supports_developer_role: bool,
    pub supports_reasoning_effort: bool,
    pub supports_usage_in_streaming: bool,
    pub supports_finish_reason: bool,
    pub max_tokens_field: String,
    pub requires_tool_result_name: bool,
    pub requires_assistant_after_tool_result: bool,
    pub requires_thinking_as_text: bool,
    pub requires_reasoning_content_on_assistant_messages: bool,
    pub thinking_format: String,
    pub chat_template_kwargs: Map<String, Value>,
    pub chat_template_args: Map<String, Value>,
    pub zai_tool_stream: bool,
    pub supports_thinking_token_budget: bool,
    pub thinking_token_budget_field: Option<String>,
    pub supports_strict_mode: bool,
    pub cache_control_format: Option<String>,
    pub send_session_affinity_headers: bool,
    pub session_affinity_format: String,
    pub supports_long_cache_retention: bool,
    pub vllm_priority: Option<Value>,
}

impl Compat {
    /// Detect from provider and base URL (reference `detectCompat`).
    pub fn detect(model: &Model, base_url: &str) -> Compat {
        let provider = model.provider.as_str();
        let has = |s: &str| base_url.contains(s);
        let is_zai = matches!(provider, "zai" | "zai-coding-cn")
            || has("api.z.ai")
            || has("open.bigmodel.cn");
        let is_together =
            provider == "together" || has("api.together.ai") || has("api.together.xyz");
        let is_moonshot =
            matches!(provider, "moonshotai" | "moonshotai-cn") || has("api.moonshot.");
        let is_openrouter = provider == "openrouter" || has("openrouter.ai");
        let is_cf_workers = provider == "cloudflare-workers-ai" || has("api.cloudflare.com");
        let is_cf_gateway = provider == "cloudflare-ai-gateway" || has("gateway.ai.cloudflare.com");
        let is_nvidia = provider == "nvidia" || has("integrate.api.nvidia.com");
        let is_ant_ling = provider == "ant-ling" || has("api.ant-ling.com");
        let is_cerebras = provider == "cerebras" || has("cerebras.ai");
        let is_deepseek =
            provider == "deepseek" || base_url.to_lowercase().contains("deepseek.com");
        let is_grok = provider == "xai" || has("api.x.ai");
        let is_non_standard = is_nvidia
            || is_cerebras
            || is_grok
            || is_together
            || has("chutes.ai")
            || is_deepseek
            || is_zai
            || is_moonshot
            || provider == "opencode"
            || has("opencode.ai")
            || is_cf_workers
            || is_cf_gateway
            || is_ant_ling;
        let use_max_tokens = has("chutes.ai")
            || is_deepseek
            || is_moonshot
            || is_cf_gateway
            || is_together
            || is_nvidia
            || is_ant_ling
            || is_zai;
        let openrouter_developer = is_openrouter
            && (model.id.starts_with("anthropic/") || model.id.starts_with("openai/"));
        let thinking_format = if is_deepseek {
            "deepseek"
        } else if is_zai {
            "zai"
        } else if is_together {
            "together"
        } else if is_ant_ling {
            "ant-ling"
        } else if is_openrouter {
            "openrouter"
        } else {
            "openai"
        };
        Compat {
            supports_store: !is_non_standard,
            supports_developer_role: openrouter_developer || (!is_non_standard && !is_openrouter),
            supports_reasoning_effort: !is_grok
                && !is_zai
                && !is_moonshot
                && !is_together
                && !is_cf_gateway
                && !is_nvidia
                && !is_ant_ling,
            supports_usage_in_streaming: true,
            supports_finish_reason: true,
            max_tokens_field: if use_max_tokens {
                "max_tokens"
            } else {
                "max_completion_tokens"
            }
            .into(),
            requires_tool_result_name: false,
            requires_assistant_after_tool_result: false,
            requires_thinking_as_text: false,
            requires_reasoning_content_on_assistant_messages: is_deepseek,
            thinking_format: thinking_format.into(),
            chat_template_kwargs: Map::new(),
            chat_template_args: Map::new(),
            zai_tool_stream: false,
            supports_thinking_token_budget: false,
            thinking_token_budget_field: None,
            supports_strict_mode: false,
            cache_control_format: (provider == "openrouter" && model.id.starts_with("anthropic/"))
                .then(|| "anthropic".into()),
            send_session_affinity_headers: is_openrouter,
            session_affinity_format: if is_openrouter {
                "openrouter"
            } else {
                "openai"
            }
            .into(),
            supports_long_cache_retention: !(is_together
                || is_cf_workers
                || is_cf_gateway
                || is_nvidia
                || is_ant_ling),
            vllm_priority: None,
        }
    }

    /// Detected settings overridden by explicit `model.compat` keys.
    pub fn resolve(model: &Model, base_url: &str) -> Compat {
        let mut c = Compat::detect(model, base_url);
        let m = model;
        let b = |key: &str, slot: &mut bool| {
            if let Some(v) = m.compat_bool(key) {
                *slot = v;
            }
        };
        b("supportsStore", &mut c.supports_store);
        b("supportsDeveloperRole", &mut c.supports_developer_role);
        b("supportsReasoningEffort", &mut c.supports_reasoning_effort);
        b(
            "supportsUsageInStreaming",
            &mut c.supports_usage_in_streaming,
        );
        b("supportsFinishReason", &mut c.supports_finish_reason);
        b("requiresToolResultName", &mut c.requires_tool_result_name);
        b(
            "requiresAssistantAfterToolResult",
            &mut c.requires_assistant_after_tool_result,
        );
        b("requiresThinkingAsText", &mut c.requires_thinking_as_text);
        b(
            "requiresReasoningContentOnAssistantMessages",
            &mut c.requires_reasoning_content_on_assistant_messages,
        );
        b("zaiToolStream", &mut c.zai_tool_stream);
        b(
            "supportsThinkingTokenBudget",
            &mut c.supports_thinking_token_budget,
        );
        b("supportsStrictMode", &mut c.supports_strict_mode);
        b(
            "sendSessionAffinityHeaders",
            &mut c.send_session_affinity_headers,
        );
        b(
            "supportsLongCacheRetention",
            &mut c.supports_long_cache_retention,
        );
        if let Some(v) = m.compat_str("maxTokensField") {
            c.max_tokens_field = v.into();
        }
        if let Some(v) = m.compat_str("thinkingFormat") {
            c.thinking_format = v.into();
        }
        if let Some(v) = m.compat_str("thinkingTokenBudgetField") {
            c.thinking_token_budget_field = Some(v.into());
        }
        if let Some(v) = m.compat_str("cacheControlFormat") {
            c.cache_control_format = Some(v.into());
        }
        if let Some(v) = m.compat_str("sessionAffinityFormat") {
            c.session_affinity_format = v.into();
        }
        if let Some(v) = m
            .compat
            .get("chatTemplateKwargs")
            .and_then(Value::as_object)
        {
            c.chat_template_kwargs = v.clone();
        }
        if let Some(v) = m.compat.get("chatTemplateArgs").and_then(Value::as_object) {
            c.chat_template_args = v.clone();
        }
        c.vllm_priority = m
            .compat
            .get("vllmPriority")
            .filter(|v| !v.is_null())
            .cloned();
        c
    }
}

// ---------------------------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------------------------

const REASONING_FIELDS: [&str; 3] = ["reasoning", "reasoning_content", "reasoning_text"];

fn is_reasoning_detail(v: &Value) -> bool {
    let Some(o) = v.as_object() else {
        return false;
    };
    let id_ok = o.get("id").is_none_or(|i| i.is_null() || i.is_string());
    let format_ok = o.get("format").is_none_or(Value::is_string);
    let index_ok = o.get("index").is_none_or(Value::is_number);
    if !(id_ok && format_ok && index_ok) {
        return false;
    }
    match o.get("type").and_then(Value::as_str) {
        Some("reasoning.summary") => o.get("summary").is_some_and(Value::is_string),
        Some("reasoning.encrypted") => o.get("data").is_some_and(Value::is_string),
        Some("reasoning.text") => {
            o.get("text").is_some_and(Value::is_string)
                && o.get("signature")
                    .is_none_or(|s| s.is_null() || s.is_string())
        }
        _ => false,
    }
}

fn parse_reasoning_details(signature: Option<&str>) -> Option<Vec<Value>> {
    let parsed: Value = serde_json::from_str(signature.filter(|s| !s.is_empty())?).ok()?;
    let arr = parsed.as_array()?;
    (!arr.is_empty() && arr.iter().all(is_reasoning_detail)).then(|| arr.clone())
}

fn parse_legacy_encrypted_detail(signature: Option<&str>) -> Option<Value> {
    let parsed: Value = serde_json::from_str(signature.filter(|s| !s.is_empty())?).ok()?;
    let ok = is_reasoning_detail(&parsed)
        && parsed["type"] == "reasoning.encrypted"
        && parsed["id"].as_str().is_some_and(|s| !s.is_empty())
        && parsed["data"].as_str().is_some_and(|s| !s.is_empty());
    ok.then_some(parsed)
}

fn has_tool_history(messages: &[Message]) -> bool {
    messages.iter().any(|m| match m {
        Message::ToolResult { .. } => true,
        Message::Assistant { content, .. } => {
            content.iter().any(|b| matches!(b, Block::ToolCall { .. }))
        }
        Message::User { .. } | Message::System(_) => false,
    })
}

/// Chat Completions tool call ids: Responses-style `call|item` ids collapse to
/// `<call>_<item>` (40 chars max, hashed when longer); OpenAI truncates to 40.
fn normalize_tool_call_id(provider: &str, id: &str) -> String {
    if let Some((call, item)) = id.split_once('|') {
        let call_id = sanitize_id_chars(call);
        let item_id = sanitize_id_chars(item);
        let combined = if item_id.is_empty() {
            call_id.clone()
        } else {
            format!("{call_id}_{item_id}")
        };
        if combined.len() <= 40 {
            return combined;
        }
        let hash: String = short_hash(id).chars().take(8).collect();
        let prefix: String = call_id
            .chars()
            .take(1usize.max(40 - hash.len() - 1))
            .collect();
        return format!("{prefix}_{hash}");
    }
    if provider == "openai" && id.chars().count() > 40 {
        return id.chars().take(40).collect();
    }
    id.to_owned()
}

fn convert_tools(
    req: &ProviderRequest,
    tools: &[ToolSpec],
    compat: &Compat,
) -> Result<Vec<Value>, String> {
    tools
        .iter()
        .map(|t| {
            let strict =
                super::common::schema::resolve_strict(req, t, compat.supports_strict_mode, None)?;
            let mut function = json!({"name":t.name, "description":t.description,
            "parameters":super::common::schema::tool_parameters(t, strict)?});
            if compat.supports_strict_mode {
                function["strict"] = json!(strict.unwrap_or(false));
            }
            Ok(json!({"type":"function", "function":function}))
        })
        .collect()
}

fn text_of(content: &[Block]) -> Vec<&str> {
    content
        .iter()
        .filter_map(|b| match b {
            Block::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// Context -> Chat Completions `messages` (reference `convertMessages`).
pub fn convert_messages(req: &ProviderRequest, compat: &Compat) -> Result<Vec<Value>, String> {
    let model = &req.model;
    let mut params: Vec<Value> = Vec::new();
    let instruction_role = if model.reasoning && compat.supports_developer_role {
        "developer"
    } else {
        "system"
    };
    if !req.system.is_empty() {
        params.push(json!({"role": instruction_role, "content": req.system}));
    }
    let provider = model.provider.clone();
    let mut normalize = move |id: &str, _: &Message| normalize_tool_call_id(&provider, id);
    let messages = transform_messages(&req.messages, model, Some(&mut normalize));

    let mut last_role: Option<&'static str> = None;
    let mut i = 0;
    while i < messages.len() {
        let msg = &messages[i];
        if compat.requires_assistant_after_tool_result
            && last_role == Some("toolResult")
            && matches!(msg, Message::User { .. })
        {
            params.push(
                json!({"role": "assistant", "content": "I have processed the tool results."}),
            );
        }
        match msg {
            Message::System(system) => {
                if req
                    .options
                    .get("pipkin.systemToolAdditions")
                    .and_then(Value::as_bool)
                    == Some(true)
                    && !system.tools_added.is_empty()
                {
                    let tools: Vec<_> = system.tools_added.iter().map(ToolSpec::from).collect();
                    params.push(
                        json!({"role": "system", "tools": convert_tools(req, &tools, compat)?}),
                    );
                }
                let text = crate::context::system::update_text(system);
                if !text.is_empty() {
                    params.push(json!({"role": instruction_role, "content": text}));
                }
            }
            Message::User { content, .. } => {
                let parts: Vec<Value> = content
                    .iter()
                    .filter_map(|b| match b {
                        Block::Text { text, .. } if text.is_empty() => None,
                        Block::Text { text, .. } => Some(json!({"type": "text", "text": text})),
                        Block::Image {
                            data, mime_type, ..
                        } => Some(json!({
                            "type": "image_url",
                            "image_url": {"url": data_url(mime_type, data)}
                        })),
                        _ => None,
                    })
                    .collect();
                if !parts.is_empty() {
                    params.push(json!({"role": "user", "content": parts}));
                }
                last_role = Some("user");
            }
            Message::Assistant { content, .. } => {
                last_role = Some("assistant");
                let mut out = Map::new();
                out.insert("role".into(), json!("assistant"));
                out.insert(
                    "content".into(),
                    if compat.requires_assistant_after_tool_result {
                        json!("")
                    } else {
                        Value::Null
                    },
                );
                let text_parts: Vec<&str> = text_of(content)
                    .into_iter()
                    .filter(|t| !t.trim().is_empty())
                    .collect();
                let assistant_text = text_parts.concat();
                let thinking: Vec<(&str, Option<&str>)> = content
                    .iter()
                    .filter_map(|b| match b {
                        Block::Thinking {
                            thinking,
                            signature,
                            ..
                        } => Some((thinking.as_str(), signature.as_deref())),
                        _ => None,
                    })
                    .collect();
                let calls: Vec<&Block> = content
                    .iter()
                    .filter(|b| matches!(b, Block::ToolCall { .. }))
                    .collect();
                let signed = thinking
                    .iter()
                    .find_map(|(_, sig)| parse_reasoning_details(*sig));
                let legacy: Vec<Value> = calls
                    .iter()
                    .filter_map(|b| match b {
                        Block::ToolCall { extra, .. } => parse_legacy_encrypted_detail(
                            extra.get("thoughtSignature").and_then(Value::as_str),
                        ),
                        _ => None,
                    })
                    .collect();
                let preserved = signed.or((!legacy.is_empty()).then_some(legacy));
                let non_empty: Vec<&(&str, Option<&str>)> = thinking
                    .iter()
                    .filter(|(t, _)| !t.trim().is_empty())
                    .collect();
                if !non_empty.is_empty() {
                    if compat.requires_thinking_as_text {
                        let thinking_text = non_empty
                            .iter()
                            .map(|(t, _)| *t)
                            .collect::<Vec<_>>()
                            .join("\n\n");
                        let mut parts = vec![json!({"type": "text", "text": thinking_text})];
                        parts.extend(
                            text_parts
                                .iter()
                                .map(|t| json!({"type": "text", "text": t})),
                        );
                        out.insert("content".into(), Value::Array(parts));
                    } else {
                        if !assistant_text.is_empty() {
                            out.insert("content".into(), json!(assistant_text));
                        }
                        if preserved.is_none() {
                            let mut signature = non_empty[0].1;
                            if model.provider == "opencode-go" && signature == Some("reasoning") {
                                signature = Some("reasoning_content");
                            }
                            if let Some(field) = signature.filter(|s| REASONING_FIELDS.contains(s))
                            {
                                let joined = non_empty
                                    .iter()
                                    .map(|(t, _)| *t)
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                out.insert(field.into(), json!(joined));
                            }
                        }
                    }
                } else if !assistant_text.is_empty() {
                    out.insert("content".into(), json!(assistant_text));
                }
                if !calls.is_empty() {
                    let tool_calls: Vec<Value> = calls
                        .iter()
                        .filter_map(|b| match b {
                            Block::ToolCall {
                                id,
                                name,
                                arguments,
                                ..
                            } => Some(json!({
                                "id": id,
                                "type": "function",
                                "function": {"name": name, "arguments": arguments.to_string()}
                            })),
                            _ => None,
                        })
                        .collect();
                    out.insert("tool_calls".into(), Value::Array(tool_calls));
                }
                if let Some(details) = preserved {
                    out.insert("reasoning_details".into(), Value::Array(details));
                }
                if compat.requires_reasoning_content_on_assistant_messages
                    && model.reasoning
                    && !out.contains_key("reasoning_content")
                {
                    out.insert("reasoning_content".into(), json!(""));
                }
                let has_content = match &out["content"] {
                    Value::String(s) => !s.is_empty(),
                    Value::Array(a) => !a.is_empty(),
                    _ => false,
                };
                if has_content || out.contains_key("tool_calls") {
                    params.push(Value::Object(out));
                }
            }
            Message::ToolResult { .. } => {
                let mut images: Vec<Value> = Vec::new();
                let mut j = i;
                while let Some(Message::ToolResult {
                    tool_call_id,
                    tool_name,
                    content,
                    ..
                }) = messages.get(j)
                {
                    let text = text_of(content).join("\n");
                    let has_images = content.iter().any(|b| matches!(b, Block::Image { .. }));
                    let body = if !text.is_empty() {
                        text
                    } else if has_images {
                        "(see attached image)".into()
                    } else {
                        "(no tool output)".into()
                    };
                    let mut m =
                        json!({"role": "tool", "content": body, "tool_call_id": tool_call_id});
                    if compat.requires_tool_result_name && !tool_name.is_empty() {
                        m["name"] = json!(tool_name);
                    }
                    params.push(m);
                    if has_images && model.supports_images() {
                        for b in content {
                            if let Block::Image {
                                data, mime_type, ..
                            } = b
                            {
                                images.push(json!({
                                    "type": "image_url",
                                    "image_url": {"url": data_url(mime_type, data)}
                                }));
                            }
                        }
                    }
                    j += 1;
                }
                i = j;
                if !images.is_empty() {
                    if compat.requires_assistant_after_tool_result {
                        params.push(json!({"role": "assistant", "content": "I have processed the tool results."}));
                    }
                    let mut parts = vec![
                        json!({"type": "text", "text": "Attached image(s) from tool result:"}),
                    ];
                    parts.extend(images);
                    params.push(json!({"role": "user", "content": parts}));
                    last_role = Some("user");
                } else {
                    last_role = Some("toolResult");
                }
                continue;
            }
        }
        i += 1;
    }
    Ok(params)
}

fn cache_control_value(compat: &Compat, retention: CacheRetention) -> Option<Value> {
    if compat.cache_control_format.as_deref() != Some("anthropic")
        || retention == CacheRetention::None
    {
        return None;
    }
    let mut cc = json!({"type": "ephemeral"});
    if retention == CacheRetention::Long && compat.supports_long_cache_retention {
        cc["ttl"] = json!("1h");
    }
    Some(cc)
}

fn add_cache_control_to_text(message: &mut Value, cc: &Value) -> bool {
    match message.get_mut("content") {
        Some(Value::String(s)) => {
            if s.is_empty() {
                return false;
            }
            let text = std::mem::take(s);
            message["content"] = json!([{"type": "text", "text": text, "cache_control": cc}]);
            true
        }
        Some(Value::Array(parts)) => {
            for part in parts.iter_mut().rev() {
                if part.get("type").and_then(Value::as_str) == Some("text") {
                    part["cache_control"] = cc.clone();
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

fn apply_anthropic_cache_control(
    messages: &mut [Value],
    tools: Option<&mut Vec<Value>>,
    cc: &Value,
) {
    if let Some(first) = messages.iter_mut().find(|m| {
        matches!(
            m.get("role").and_then(Value::as_str),
            Some("system" | "developer")
        )
    }) {
        add_cache_control_to_text(first, cc);
    }
    if let Some(last) = tools.and_then(|t| t.last_mut()) {
        last["cache_control"] = cc.clone();
    }
    for m in messages.iter_mut().rev() {
        if matches!(
            m.get("role").and_then(Value::as_str),
            Some("user" | "assistant" | "tool")
        ) && add_cache_control_to_text(m, cc)
        {
            return;
        }
    }
}

fn resolve_template_value(
    model: &Model,
    effort: Option<ThinkingLevel>,
    value: &Value,
    budget: Option<u64>,
) -> Option<Value> {
    let Value::Object(o) = value else {
        return Some(value.clone());
    };
    if effort.is_none() && o.get("omitWhenOff").is_some_and(js_truthy) {
        return None;
    }
    match o.get("$var").and_then(Value::as_str) {
        Some("thinking.enabled") => return Some(json!(effort.is_some())),
        Some("thinking.budget") => return budget.map(|b| json!(b)),
        _ => {}
    }
    let mapped = mapped_level(model, effort.map_or("off", ThinkingLevel::as_str));
    match mapped {
        Mapped::Missing => effort.map(|e| json!(e.as_str())),
        Mapped::Value(v) => Some(json!(v)),
        Mapped::Null => None,
    }
}

fn template_values(
    model: &Model,
    effort: Option<ThinkingLevel>,
    values: &Map<String, Value>,
    budget: Option<u64>,
) -> Option<Value> {
    let mut out = Map::new();
    for (k, v) in values {
        if let Some(r) = resolve_template_value(model, effort, v, budget) {
            out.insert(k.clone(), r);
        }
    }
    (!out.is_empty()).then_some(Value::Object(out))
}

/// The full request body (reference `buildParams`).
pub fn build_params(req: &ProviderRequest, compat: &Compat) -> Result<Value, String> {
    let resolved =
        req.resolve_systems(req.model.compat_bool("supportsMidConvoSystemMessages") == Some(true));
    let req = resolved.as_ref();
    let model = &req.model;
    let retention = req.cache_retention;
    let effort = reasoning_effort(req);
    let mut messages = convert_messages(req, compat)?;
    let cc = cache_control_value(compat, retention);
    let mut p = Map::new();
    p.insert("model".into(), json!(model.id));
    p.insert("stream".into(), json!(true));
    let long = retention == CacheRetention::Long && compat.supports_long_cache_retention;
    if ((req.base_url().contains("api.openai.com") && retention != CacheRetention::None) || long)
        && let Some(key) = clamp_prompt_cache_key(&req.session_id)
    {
        p.insert("prompt_cache_key".into(), json!(key));
    }
    if long {
        p.insert("prompt_cache_retention".into(), json!("24h"));
    }
    if compat.supports_usage_in_streaming {
        p.insert("stream_options".into(), json!({"include_usage": true}));
    }
    if compat.supports_store {
        p.insert("store".into(), json!(false));
    }
    // Reference `buildBaseOptions`: the cap is clamped to what fits in the context window.
    let max_tokens = base_max_tokens(req);
    if max_tokens > 0 {
        let field = if compat.max_tokens_field == "max_tokens" {
            "max_tokens"
        } else {
            "max_completion_tokens"
        };
        p.insert(field.into(), json!(max_tokens));
    }
    if let Some(t) = req.options.get("temperature").filter(|v| !v.is_null()) {
        p.insert("temperature".into(), t.clone());
    }
    let mut tools: Option<Vec<Value>> = None;
    if !req.tools.is_empty() {
        tools = Some(convert_tools(req, &req.tools, compat)?);
        if compat.zai_tool_stream {
            p.insert("tool_stream".into(), json!(true));
        }
    } else if has_tool_history(&req.messages) {
        tools = Some(Vec::new());
    }
    if let Some(cc) = &cc {
        apply_anthropic_cache_control(&mut messages, tools.as_mut(), cc);
    }
    p.insert("messages".into(), Value::Array(messages));
    if let Some(tools) = tools {
        p.insert("tools".into(), Value::Array(tools));
    }
    if let Some(choice) = req.options.get("toolChoice").filter(|v| js_truthy(v)) {
        p.insert("tool_choice".into(), choice.clone());
    }
    if let Some(priority) = &compat.vllm_priority {
        p.insert("priority".into(), priority.clone());
    }

    let budget_field = compat.thinking_token_budget_field.clone().or_else(|| {
        compat
            .supports_thinking_token_budget
            .then(|| "thinking_token_budget".to_owned())
    });
    let budget = match effort {
        Some(level) if model.reasoning => {
            let ceiling = p
                .get("max_tokens")
                .or_else(|| p.get("max_completion_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or(model.max_tokens);
            let b = clamp_thinking_budget_to_answer_room(
                thinking_budget_for_level(req, level),
                ceiling,
            );
            (b > 0).then_some(b)
        }
        _ => None,
    };
    apply_thinking(&mut p, model, compat, effort, budget);
    if let (Some(field), Some(budget)) = (budget_field, budget) {
        p.insert(field, json!(budget));
    }

    if let Some(routing) = model
        .compat
        .get("openRouterRouting")
        .filter(|v| js_truthy(v))
    {
        p.insert("provider".into(), routing.clone());
    }
    if let Some(routing) = model
        .compat
        .get("vercelGatewayRouting")
        .filter(|v| js_truthy(v))
    {
        let only = routing.get("only").filter(|v| js_truthy(v));
        let order = routing.get("order").filter(|v| js_truthy(v));
        if only.is_some() || order.is_some() {
            let mut gateway = Map::new();
            if let Some(o) = only {
                gateway.insert("only".into(), o.clone());
            }
            if let Some(o) = order {
                gateway.insert("order".into(), o.clone());
            }
            p.insert("providerOptions".into(), json!({"gateway": gateway}));
        }
    }
    if let Some(sampling) = resolve_sampling_params(req, effort) {
        for (k, v) in sampling {
            p.insert(k, v);
        }
    }
    Ok(Value::Object(p))
}

/// The eleven `thinkingFormat` shapes.
fn apply_thinking(
    p: &mut Map<String, Value>,
    model: &Model,
    compat: &Compat,
    effort: Option<ThinkingLevel>,
    budget: Option<u64>,
) {
    let level = effort.map(ThinkingLevel::as_str);
    let mapped = |l: &str| mapped_level(model, l);
    let off = mapped("off");
    let reasoning = model.reasoning;
    match compat.thinking_format.as_str() {
        "zai" if reasoning => {
            p.insert(
                "thinking".into(),
                if level.is_some() {
                    json!({"type": "enabled", "clear_thinking": false})
                } else {
                    json!({"type": "disabled"})
                },
            );
            if let Some(l) = level
                && compat.supports_reasoning_effort
            {
                match mapped(l) {
                    Mapped::Missing => {
                        p.insert("reasoning_effort".into(), json!(l));
                    }
                    Mapped::Value(v) => {
                        p.insert("reasoning_effort".into(), json!(v));
                    }
                    Mapped::Null => {}
                }
            }
        }
        "qwen" if reasoning => {
            p.insert("enable_thinking".into(), json!(level.is_some()));
            if let Some(l) = level
                && compat.supports_reasoning_effort
            {
                p.insert("reasoning_effort".into(), json!(mapped(l).or(l)));
            }
        }
        "qwen-chat-template" if reasoning => {
            p.insert(
                "chat_template_kwargs".into(),
                json!({"enable_thinking": level.is_some(), "preserve_thinking": true}),
            );
        }
        "chat-template" if reasoning => {
            if let Some(v) = template_values(model, effort, &compat.chat_template_kwargs, budget) {
                p.insert("chat_template_kwargs".into(), v);
            }
        }
        "baseten" if reasoning => {
            if let Some(v) = template_values(model, effort, &compat.chat_template_args, budget) {
                p.insert("chat_template_args".into(), v);
            }
            if compat.supports_reasoning_effort {
                let m = mapped(level.unwrap_or("off"));
                let value = match m {
                    Mapped::Missing => level,
                    Mapped::Value(v) => Some(v),
                    Mapped::Null => None,
                };
                if let Some(v) = value {
                    p.insert("reasoning_effort".into(), json!(v));
                }
            }
        }
        "deepseek" if reasoning => {
            if level.is_some() {
                p.insert("thinking".into(), json!({"type": "enabled"}));
            } else if off != Mapped::Null {
                p.insert("thinking".into(), json!({"type": "disabled"}));
            }
            if let Some(l) = level
                && compat.supports_reasoning_effort
            {
                p.insert("reasoning_effort".into(), json!(mapped(l).or(l)));
            }
        }
        "openrouter" if reasoning => {
            if let Some(l) = level {
                p.insert("reasoning".into(), json!({"effort": mapped(l).or(l)}));
            } else if off != Mapped::Null {
                p.insert("reasoning".into(), json!({"effort": off.or("none")}));
            }
        }
        "ant-ling" if reasoning && level.is_some() => {
            if let Some(l) = level
                && let Mapped::Value(v) = mapped(l)
            {
                p.insert("reasoning".into(), json!({"effort": v}));
            }
        }
        "together" if reasoning => {
            p.insert("reasoning".into(), json!({"enabled": level.is_some()}));
            if let Some(l) = level
                && compat.supports_reasoning_effort
            {
                p.insert("reasoning_effort".into(), json!(mapped(l).or(l)));
            }
        }
        "string-thinking" if reasoning => {
            if let Some(l) = level {
                p.insert("thinking".into(), json!(mapped(l).or(l)));
            } else if off != Mapped::Null {
                p.insert("thinking".into(), json!(off.or("none")));
            }
        }
        _ => {
            if reasoning && compat.supports_reasoning_effort {
                if let Some(l) = level {
                    p.insert("reasoning_effort".into(), json!(mapped(l).or(l)));
                } else if let Mapped::Value(v) = off {
                    p.insert("reasoning_effort".into(), json!(v));
                }
            }
        }
    }
}

fn build_request_headers(
    req: &ProviderRequest,
    compat: &Compat,
    api_key: &str,
) -> Result<reqwest::header::HeaderMap, String> {
    let mut dynamic: Vec<(&str, String)> = Vec::new();
    if req.model.provider == "github-copilot" {
        dynamic.extend(copilot_headers(&req.messages));
    }
    if req.cache_retention != CacheRetention::None
        && !req.session_id.is_empty()
        && compat.send_session_affinity_headers
    {
        let sid = req.session_id.clone();
        if compat.session_affinity_format == "openrouter" {
            dynamic.push(("x-session-id", sid));
        } else {
            if compat.session_affinity_format == "openai" {
                dynamic.push(("session_id", sid.clone()));
            }
            dynamic.push(("x-client-request-id", sid.clone()));
            dynamic.push(("x-session-affinity", sid));
        }
    }
    build_headers(
        &[("authorization", format!("Bearer {api_key}"))],
        &req.model.headers,
        &dynamic,
        &req.headers,
    )
}

// ---------------------------------------------------------------------------------------------
// Stream
// ---------------------------------------------------------------------------------------------

fn finish_reason(reason: &str) -> (StopReason, Option<String>) {
    match reason {
        "stop" | "end" => (StopReason::Stop, None),
        "length" => (StopReason::Length, None),
        "function_call" | "tool_calls" => (StopReason::ToolUse, None),
        other => (
            StopReason::Error,
            Some(format!("Provider finish_reason: {other}")),
        ),
    }
}

fn parse_usage(e: &mut Emitter, raw: &Value) {
    let n = |p: &str| raw.pointer(p).and_then(Value::as_u64);
    let prompt = n("/prompt_tokens").unwrap_or(0);
    let cache_read = n("/prompt_tokens_details/cached_tokens")
        .or_else(|| n("/prompt_cache_hit_tokens"))
        .or_else(|| n("/cached_tokens"))
        .unwrap_or(0);
    let cache_write = n("/prompt_tokens_details/cache_write_tokens").unwrap_or(0);
    let input = prompt
        .saturating_sub(cache_read)
        .saturating_sub(cache_write);
    let output = n("/completion_tokens").unwrap_or(0);
    let u = e.usage_mut();
    u.input = input;
    u.output = output;
    u.reasoning = Some(n("/completion_tokens_details/reasoning_tokens").unwrap_or(0));
    u.cache_read = cache_read;
    u.cache_write = cache_write;
    u.total_tokens = input + output + cache_read + cache_write;
}

#[derive(Default)]
struct ToolSlot {
    stream_index: Option<i64>,
    raw: String,
    /// Grammar/custom tool input (`{"input": ...}`), when the call is a custom tool call.
    custom: Option<String>,
}

#[derive(Default)]
struct State {
    text: Option<usize>,
    thinking: Option<usize>,
    by_index: HashMap<i64, usize>,
    by_id: HashMap<String, usize>,
    tools: HashMap<usize, ToolSlot>,
    reasoning_details: Option<Vec<Value>>,
    has_finish: bool,
    stop: Option<StopReason>,
    error: Option<String>,
    response_id: bool,
    response_model: bool,
    fatal: Option<String>,
}

fn block_identity(e: &Emitter, index: usize) -> (String, String) {
    match e.content().get(index) {
        Some(Block::ToolCall { id, name, .. }) => (id.clone(), name.clone()),
        _ => (String::new(), String::new()),
    }
}

fn append_reasoning_detail(details: &mut Vec<Value>, detail: &Value) {
    let ty = detail["type"].as_str().unwrap_or_default();
    if let Some(last) = details.last_mut()
        && last["type"] == ty
        && matches!(ty, "reasoning.text" | "reasoning.summary")
    {
        let field = if ty == "reasoning.text" {
            "text"
        } else {
            "summary"
        };
        let add = detail[field].as_str().unwrap_or_default().to_owned();
        let merged = format!("{}{}", last[field].as_str().unwrap_or_default(), add);
        last[field] = json!(merged);
        if ty == "reasoning.text"
            && !last.get("signature").is_some_and(js_truthy)
            && let Some(sig) = detail.get("signature")
        {
            last["signature"] = sig.clone();
        }
        if last.get("id").is_none_or(Value::is_null)
            && let Some(id) = detail.get("id")
        {
            last["id"] = id.clone();
        }
        if !last.get("format").is_some_and(js_truthy)
            && let Some(f) = detail.get("format")
        {
            last["format"] = f.clone();
        }
        if last.get("index").is_none_or(Value::is_null)
            && let Some(i) = detail.get("index")
        {
            last["index"] = i.clone();
        }
        return;
    }
    details.push(detail.clone());
}

impl State {
    fn ensure_text(&mut self, e: &mut Emitter) -> usize {
        *self.text.get_or_insert_with(|| e.start_text())
    }

    fn ensure_thinking(&mut self, e: &mut Emitter, signature: &str) -> usize {
        if let Some(i) = self.thinking {
            return i;
        }
        let i = e.start_thinking();
        if !signature.is_empty() {
            e.thinking_signature(i, signature, false);
        }
        self.thinking = Some(i);
        i
    }

    fn ensure_tool(&mut self, e: &mut Emitter, tc: &Value) -> usize {
        let stream_index = tc.get("index").and_then(Value::as_i64);
        let function = tc.get("function").filter(|f| f.is_object());
        let custom = tc.get("custom").filter(|c| c.is_object());
        let name = function
            .and_then(|f| f.get("name"))
            .or_else(|| custom.and_then(|c| c.get("name")))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let id = tc.get("id").and_then(Value::as_str).unwrap_or_default();
        let mut index = stream_index.and_then(|i| self.by_index.get(&i).copied());
        if index.is_none() && !id.is_empty() {
            index = self.by_id.get(id).copied();
        }
        let index = match index {
            Some(i) => i,
            None => {
                let i = e.start_tool_call(id, name);
                let slot = ToolSlot {
                    stream_index,
                    raw: String::new(),
                    custom: (custom.is_some() && function.is_none()).then(String::new),
                };
                self.tools.insert(i, slot);
                if let Some(si) = stream_index {
                    self.by_index.insert(si, i);
                }
                i
            }
        };
        let slot = self.tools.entry(index).or_default();
        if let Some(si) = stream_index
            && slot.stream_index.is_none()
        {
            slot.stream_index = Some(si);
            self.by_index.insert(si, index);
        }
        if custom.is_some() && function.is_none() && slot.custom.is_none() {
            slot.custom = Some(String::new());
        }
        if !id.is_empty() {
            self.by_id.insert(id.to_owned(), index);
        }
        let (_, current_name) = block_identity(e, index);
        if current_name.is_empty() && !name.is_empty() {
            e.set_tool_identity(index, None, Some(name));
        }
        index
    }

    fn on_event(&mut self, e: &mut Emitter, model: &Model, ev: SseEvent) -> ControlFlow<()> {
        let data = ev.data.trim();
        if data.is_empty() {
            return ControlFlow::Continue(());
        }
        if data.starts_with("[DONE]") {
            return ControlFlow::Break(());
        }
        let chunk: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(err) => {
                self.fatal = Some(format!("Could not parse stream chunk: {err}"));
                return ControlFlow::Break(());
            }
        };
        if let Some(err) = chunk.get("error").filter(|v| js_truthy(v)) {
            let mut message = match err.get("message") {
                Some(Value::String(m)) => m.clone(),
                Some(m) if !m.is_null() => m.to_string(),
                _ => err.to_string(),
            };
            if let Some(raw) = err.pointer("/metadata/raw").filter(|v| !v.is_null()) {
                let raw = raw.as_str().map_or_else(|| raw.to_string(), str::to_owned);
                if !message.contains(&raw) {
                    message.push('\n');
                    message.push_str(&raw);
                }
            }
            self.fatal = Some(message);
            return ControlFlow::Break(());
        }
        self.chunk(e, model, &chunk);
        ControlFlow::Continue(())
    }

    fn chunk(&mut self, e: &mut Emitter, model: &Model, chunk: &Value) {
        if !chunk.is_object() {
            return;
        }
        if !self.response_id
            && let Some(id) = chunk
                .get("id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        {
            e.set_response_id(id);
            self.response_id = true;
        }
        if !self.response_model
            && let Some(m) = chunk.get("model").and_then(Value::as_str)
            && !m.is_empty()
            && m != model.id
        {
            e.message_extra("responseModel", json!(m));
            self.response_model = true;
        }
        let usage = chunk.get("usage").filter(|u| js_truthy(u));
        if let Some(u) = usage {
            parse_usage(e, u);
        }
        let Some(choice) = chunk
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|c| c.first())
        else {
            return;
        };
        if usage.is_none()
            && let Some(u) = choice.get("usage").filter(|u| js_truthy(u))
        {
            parse_usage(e, u);
        }
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str)
            && !reason.is_empty()
        {
            e.message_extra("rawStopReason", json!(reason));
            let (stop, error) = finish_reason(reason);
            self.stop = Some(stop);
            if error.is_some() {
                self.error = error;
            }
            self.has_finish = true;
        }
        let Some(delta) = choice.get("delta").filter(|d| d.is_object()) else {
            return;
        };
        if let Some(text) = delta.get("content").and_then(Value::as_str)
            && !text.is_empty()
        {
            let i = self.ensure_text(e);
            e.text(i, text);
        }
        let found = ["reasoning_content", "reasoning", "reasoning_text"]
            .into_iter()
            .find_map(|f| {
                delta
                    .get(f)
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(|s| (f, s))
            });
        if let Some((field, text)) = found {
            let signature = if model.provider == "opencode-go" && field == "reasoning" {
                "reasoning_content"
            } else {
                field
            };
            let i = self.ensure_thinking(e, signature);
            e.thinking(i, text);
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for tc in calls {
                let index = self.ensure_tool(e, tc);
                let (cur_id, cur_name) = block_identity(e, index);
                if cur_id.is_empty()
                    && let Some(id) = tc
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                {
                    e.set_tool_identity(index, Some(id), None);
                    self.by_id.insert(id.to_owned(), index);
                }
                let name = tc
                    .pointer("/function/name")
                    .or_else(|| tc.pointer("/custom/name"))
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty());
                if cur_name.is_empty()
                    && let Some(name) = name
                {
                    e.set_tool_identity(index, None, Some(name));
                }
                let slot = self.tools.entry(index).or_default();
                if let Some(args) = tc
                    .pointer("/function/arguments")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    slot.raw.push_str(args);
                } else if let Some(input) = tc
                    .pointer("/custom/input")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    && let Some(custom) = slot.custom.as_mut()
                {
                    custom.push_str(input);
                }
            }
        }
        if let Some(details) = delta.get("reasoning_details").and_then(Value::as_array) {
            for detail in details {
                if !is_reasoning_detail(detail) {
                    continue;
                }
                self.ensure_thinking(e, "");
                append_reasoning_detail(
                    self.reasoning_details.get_or_insert_with(Vec::new),
                    detail,
                );
            }
        }
    }

    /// Store replay data and settle tool arguments (also on failure paths).
    fn finalize(&mut self, e: &mut Emitter) {
        if let (Some(i), Some(details)) = (self.thinking, &self.reasoning_details) {
            e.thinking_signature(i, &Value::Array(details.clone()).to_string(), false);
        }
        finalize_tool_args(
            e,
            std::mem::take(&mut self.tools).into_iter().map(|(i, s)| {
                (
                    i,
                    s.custom
                        .map_or(ToolArgs::Json(s.raw), |input| ToolArgs::Custom {
                            property: "input".into(),
                            input,
                        }),
                )
            }),
        );
    }
}

async fn run(ctx: StreamCtx<'_>) -> Message {
    let req = ctx.req;
    let mut e = ctx.emitter();
    let compat = Compat::resolve(&req.model, req.base_url());
    let api_key = match client_api_key(req) {
        Ok(k) => k,
        Err(m) => return ctx.failed(e, m),
    };
    let headers = match build_request_headers(req, &compat, &api_key) {
        Ok(h) => h,
        Err(m) => return ctx.failed(e, m),
    };
    let body = match build_params(req, &compat) {
        Ok(v) => v,
        Err(m) => return ctx.failed(e, m),
    };
    let url = format!("{}/chat/completions", req.base_url().trim_end_matches('/'));
    let resp = match post_json(&ctx, &url, headers, &body).await {
        Ok(r) => r,
        Err(m) => return ctx.failed(e, sdk_error(&m, None)),
    };
    let mut state = State::default();
    let read = http::read_sse(resp, idle_timeout(req, NO_TIMEOUT), &ctx.cancel, |ev| {
        state.on_event(&mut e, &req.model, ev)
    })
    .await;
    state.finalize(&mut e);
    if let Err(err) = read {
        let message = match err {
            http::ReadError::Aborted => "Request was aborted".to_owned(),
            http::ReadError::Failed(m) => m,
        };
        return ctx.failed(e, message);
    }
    if let Some(m) = state.fatal.take() {
        return ctx.failed(e, m);
    }
    if ctx.cancel.is_cancelled() {
        return ctx.failed(e, "Request was aborted");
    }
    if !state.has_finish && !compat.supports_finish_reason {
        state.stop = Some(if e.has_tool_calls() {
            StopReason::ToolUse
        } else {
            StopReason::Stop
        });
    }
    if state.stop == Some(StopReason::Error) {
        let m = state
            .error
            .take()
            .unwrap_or_else(|| "Provider returned an error stop reason".into());
        return ctx.failed(e, m);
    }
    let Some(stop) = state
        .stop
        .filter(|_| state.has_finish || !compat.supports_finish_reason)
    else {
        return ctx.failed(e, "Stream ended without finish_reason");
    };
    e.finish(req, stop, None)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::time::Duration;

    use pipkin_engine_protocol::{Block, Delta, Message, StopReason};
    use serde_json::{Map, Value, json};
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::provider::mock::{MockResponse, MockServer, Recorded};
    use crate::provider::{CacheRetention, Credential, ProviderRequest, StreamCtx, StreamEvent};

    // ----- harness (shared with the Responses tests) -----

    pub(crate) fn request(model: Value) -> ProviderRequest {
        ProviderRequest {
            model: serde_json::from_value(model).unwrap(),
            system: String::new(),
            messages: vec![],
            tools: vec![],
            thinking: ThinkingLevel::Off,
            session_id: String::new(),
            max_tokens: None,
            cache_retention: CacheRetention::Short,
            credential: Credential::ApiKey("sk-test".into()),
            headers: BTreeMap::new(),
            base_url: None,
            timeout: Duration::from_secs(10),
            env: HashMap::new(),
            options: Map::new(),
        }
    }

    pub(crate) fn user(content: Vec<Block>) -> Message {
        Message::User {
            content,
            timestamp: 1,
            extra: Map::new(),
        }
    }

    pub(crate) fn assistant(model: &Model, content: Vec<Block>, stop: StopReason) -> Message {
        Message::Assistant {
            content,
            api: model.api.clone(),
            provider: model.provider.clone(),
            model: model.id.clone(),
            usage: Default::default(),
            stop_reason: stop,
            error_message: None,
            timestamp: 2,
            extra: Map::new(),
        }
    }

    pub(crate) fn tool_result(id: &str, name: &str, content: Vec<Block>) -> Message {
        Message::ToolResult {
            tool_call_id: id.into(),
            tool_name: name.into(),
            content,
            is_error: false,
            timestamp: 3,
            extra: Map::new(),
        }
    }

    pub(crate) fn read_tool() -> ToolSpec {
        ToolSpec {
            constrained_sampling: None,
            name: "read".into(),
            description: "Read a file".into(),
            parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        }
    }

    /// `data:` lines for chat-completion chunks plus the `[DONE]` marker.
    pub(crate) fn completion_sse(chunks: &[Value]) -> String {
        let mut s = String::new();
        for c in chunks {
            s.push_str(&format!("data: {c}\n\n"));
        }
        s.push_str("data: [DONE]\n\n");
        s
    }

    /// Typed Responses events.
    pub(crate) fn responses_sse(events: &[Value]) -> String {
        let mut s = String::new();
        for e in events {
            s.push_str(&format!(
                "event: {}\ndata: {e}\n\n",
                e["type"].as_str().unwrap()
            ));
        }
        s
    }

    /// An SSE body cut into `step`-byte chunks (splitting UTF-8 sequences and JSON).
    pub(crate) fn fragmented(body: &str, step: usize) -> MockResponse {
        let mut r = MockResponse::sse(vec![]);
        r.chunks = body.as_bytes().chunks(step).map(<[u8]>::to_vec).collect();
        r.pause = Duration::ZERO;
        r
    }

    pub(crate) struct Run {
        pub message: Message,
        pub deltas: Vec<Delta>,
        pub requests: Vec<Recorded>,
    }

    /// Run one exchange against a mock server answering with `responses` in order (the last
    /// one repeats). `path` is appended to the server URL as the base URL.
    pub(crate) async fn run_mock(
        adapter: Arc<dyn ApiAdapter>,
        mut req: ProviderRequest,
        path: &str,
        responses: Vec<MockResponse>,
    ) -> Run {
        let server =
            MockServer::start(move |_, n| responses[n.min(responses.len() - 1)].clone()).await;
        req.base_url = Some(format!("{}{path}", server.url));
        let client = http::client();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let message = adapter
            .stream(StreamCtx {
                net: None,
                req: &req,
                http: &client,
                events: Some(tx.into()),
                cancel: CancellationToken::new(),
            })
            .await;
        let mut deltas = vec![];
        while let Ok(StreamEvent::Delta(d)) = rx.try_recv() {
            deltas.push(d);
        }
        Run {
            message,
            deltas,
            requests: server.requests(),
        }
    }

    /// Like [`run_mock`], but cancels after the first delta arrives. The server stalls after
    /// sending `body`.
    pub(crate) async fn run_cancelled(
        adapter: Arc<dyn ApiAdapter>,
        mut req: ProviderRequest,
        body: String,
    ) -> Message {
        let mut resp = MockResponse::sse(vec![body]);
        resp.hang = true;
        let server = MockServer::start(move |_, _| resp.clone()).await;
        req.base_url = Some(server.url.clone());
        let client = http::client();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let ctx = StreamCtx {
            net: None,
            req: &req,
            http: &client,
            events: Some(tx.into()),
            cancel: cancel.clone(),
        };
        let watcher = async {
            let _ = rx.recv().await;
            cancel.cancel();
        };
        let (message, ()) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(adapter.stream(ctx), watcher)
        })
        .await
        .expect("cancellation ends the exchange");
        message
    }

    pub(crate) fn parts(m: &Message) -> (&[Block], StopReason, Option<&str>, &Map<String, Value>) {
        match m {
            Message::Assistant {
                content,
                stop_reason,
                error_message,
                extra,
                ..
            } => (content, *stop_reason, error_message.as_deref(), extra),
            _ => panic!("not an assistant message"),
        }
    }

    fn body(req: &ProviderRequest) -> Value {
        build_params(req, &Compat::resolve(&req.model, req.base_url())).unwrap()
    }

    fn openai_model() -> Value {
        json!({"id": "gpt-5", "provider": "openai", "api": "openai-completions",
               "baseUrl": "https://api.openai.com/v1", "reasoning": true,
               "input": ["text", "image"], "maxTokens": 32000})
    }

    // ----- request bodies -----

    #[test]
    fn positional_tool_additions_are_anchored_and_removals_fall_back_to_current_tools() {
        let mut req = request(openai_model());
        req.system.clear();
        req.model
            .compat
            .insert("supportsMidConvoSystemMessages".into(), json!(true));
        req.model
            .compat
            .insert("supportsMidConvoToolAdditions".into(), json!(true));
        req.messages = serde_json::from_value(json!([
            {"role":"system","content":"base", "toolsAdded":[{"name":"read","description":"read","parameters":{"type":"object"}}]},
            {"role":"user","content":[{"type":"text","text":"work"}]},
            {"role":"system","content":"changed", "toolsAdded":[{"name":"write","description":"write","parameters":{"type":"object"}}]}
        ])).unwrap();
        let b = body(&req);
        assert_eq!(b["tools"].as_array().unwrap().len(), 1);
        assert_eq!(b["messages"][2]["tools"][0]["function"]["name"], "write");
        assert_eq!(b["messages"][3]["content"], "changed");
        if let Message::System(s) = &mut req.messages[2] {
            s.tools_removed.push(pipkin_engine_protocol::ToolReference {
                name: "read".into(),
            });
        }
        let b = body(&req);
        assert_eq!(b["tools"].as_array().unwrap().len(), 1);
        assert_eq!(b["tools"][0]["function"]["name"], "write");
        assert_eq!(b["messages"].as_array().unwrap().len(), 3);
        assert!(b["messages"][2].get("tools").is_none());
    }

    #[test]
    fn openai_profile_full_request() {
        let mut req = request(openai_model());
        let model = req.model.clone();
        req.system = "Be brief.".into();
        req.thinking = ThinkingLevel::High;
        req.session_id = "session-1".into();
        req.max_tokens = Some(4096);
        req.tools = vec![read_tool()];
        req.messages = vec![
            user(vec![Block::text("look"), Block::image("image/png", "AAAA")]),
            assistant(
                &model,
                vec![
                    Block::text("Reading."),
                    Block::tool_call("call_1", "read", json!({"path": "a.png"})),
                ],
                StopReason::ToolUse,
            ),
            tool_result(
                "call_1",
                "read",
                vec![Block::text("binary"), Block::image("image/png", "BBBB")],
            ),
            user(vec![Block::text("thanks"), Block::text("")]),
        ];
        assert_eq!(
            body(&req),
            json!({
                "model": "gpt-5",
                "stream": true,
                "stream_options": {"include_usage": true},
                "store": false,
                "prompt_cache_key": "session-1",
                "max_completion_tokens": 4096,
                "reasoning_effort": "high",
                "messages": [
                    {"role": "developer", "content": "Be brief."},
                    {"role": "user", "content": [
                        {"type": "text", "text": "look"},
                        {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}}
                    ]},
                    {"role": "assistant", "content": "Reading.", "tool_calls": [
                        {"id": "call_1", "type": "function",
                         "function": {"name": "read", "arguments": "{\"path\":\"a.png\"}"}}
                    ]},
                    {"role": "tool", "content": "binary", "tool_call_id": "call_1"},
                    {"role": "user", "content": [
                        {"type": "text", "text": "Attached image(s) from tool result:"},
                        {"type": "image_url", "image_url": {"url": "data:image/png;base64,BBBB"}}
                    ]},
                    {"role": "user", "content": [{"type": "text", "text": "thanks"}]}
                ],
                "tools": [{"type": "function", "function": {
                    "name": "read", "description": "Read a file",
                    "parameters": {"type": "object", "properties": {"path": {"type": "string"}}}
                }}]
            })
        );
    }

    #[test]
    fn openai_thinking_off_uses_the_mapped_off_value_and_long_retention() {
        let mut m = openai_model();
        m["thinkingLevelMap"] = json!({"off": "minimal", "high": "high"});
        let mut req = request(m);
        req.session_id = "x".repeat(80);
        req.cache_retention = CacheRetention::Long;
        let b = body(&req);
        assert_eq!(b["reasoning_effort"], "minimal");
        assert_eq!(b["prompt_cache_key"], "x".repeat(64));
        assert_eq!(b["prompt_cache_retention"], "24h");
        // Without an `off` mapping nothing is sent; `none` retention drops the cache key.
        let mut req = request(openai_model());
        req.session_id = "s".into();
        req.cache_retention = CacheRetention::None;
        let b = body(&req);
        assert!(b.get("reasoning_effort").is_none());
        assert!(b.get("prompt_cache_key").is_none());
    }

    #[test]
    fn openrouter_anthropic_profile_adds_cache_control_and_reasoning_object() {
        let mut req = request(json!({
            "id": "anthropic/claude-sonnet-4", "provider": "openrouter", "api": "openai-completions",
            "baseUrl": "https://openrouter.ai/api/v1", "reasoning": true, "input": ["text"],
            "maxTokens": 8000, "compat": {"openRouterRouting": {"only": ["anthropic"]}}
        }));
        let model = req.model.clone();
        req.system = "sys".into();
        req.thinking = ThinkingLevel::Medium;
        req.cache_retention = CacheRetention::Long;
        req.session_id = "s1".into();
        req.tools = vec![
            read_tool(),
            ToolSpec {
                constrained_sampling: None,
                name: "bash".into(),
                description: "Run".into(),
                parameters: json!({"type": "object"}),
            },
        ];
        req.messages = vec![
            user(vec![Block::text("hi")]),
            assistant(&model, vec![Block::text("hello")], StopReason::Stop),
            user(vec![Block::text("again"), Block::text("more")]),
        ];
        let b = body(&req);
        assert_eq!(b["reasoning"], json!({"effort": "medium"}));
        assert!(b.get("reasoning_effort").is_none());
        assert_eq!(b["store"], false, "openrouter is a standard host for store");
        assert_eq!(b["max_completion_tokens"], 8000);
        assert_eq!(b["provider"], json!({"only": ["anthropic"]}));
        assert_eq!(b["prompt_cache_key"], "s1");
        assert_eq!(b["prompt_cache_retention"], "24h");
        let cc = json!({"type": "ephemeral", "ttl": "1h"});
        assert_eq!(
            b["messages"],
            json!([
                {"role": "developer", "content": [{"type": "text", "text": "sys", "cache_control": cc}]},
                {"role": "user", "content": [{"type": "text", "text": "hi"}]},
                {"role": "assistant", "content": "hello"},
                {"role": "user", "content": [{"type": "text", "text": "again"},
                    {"type": "text", "text": "more", "cache_control": cc}]}
            ])
        );
        assert_eq!(b["tools"][1]["cache_control"], cc);
        assert!(b["tools"][0].get("cache_control").is_none());
        // Thinking off: `reasoning: {effort: "none"}` unless the off mapping is null.
        req.thinking = ThinkingLevel::Off;
        req.cache_retention = CacheRetention::Short;
        let b = body(&req);
        assert_eq!(b["reasoning"], json!({"effort": "none"}));
        assert_eq!(
            b["messages"][0]["content"][0]["cache_control"],
            json!({"type": "ephemeral"})
        );
        req.model.thinking_level_map.insert("off".into(), None);
        assert!(body(&req).get("reasoning").is_none());
    }

    #[test]
    fn deepseek_profile() {
        let mut req = request(json!({
            "id": "deepseek-reasoner", "provider": "deepseek", "api": "openai-completions",
            "baseUrl": "https://api.deepseek.com", "reasoning": true, "maxTokens": 8000
        }));
        let model = req.model.clone();
        req.system = "sys".into();
        req.thinking = ThinkingLevel::High;
        let thinking = Block::Thinking {
            thinking: "pondering".into(),
            signature: Some("reasoning_content".into()),
            redacted: false,
            extra: Map::new(),
        };
        req.messages = vec![
            user(vec![Block::text("q")]),
            assistant(&model, vec![thinking, Block::text("a")], StopReason::Stop),
            assistant(&model, vec![Block::text("b")], StopReason::Stop),
            user(vec![Block::text("q2")]),
        ];
        let b = body(&req);
        assert_eq!(b["thinking"], json!({"type": "enabled"}));
        assert_eq!(b["reasoning_effort"], "high");
        assert_eq!(b["max_tokens"], 8000);
        assert!(b.get("max_completion_tokens").is_none());
        assert!(b.get("store").is_none());
        assert_eq!(
            b["messages"][0],
            json!({"role": "system", "content": "sys"})
        );
        assert_eq!(
            b["messages"][2],
            json!({"role": "assistant", "content": "a", "reasoning_content": "pondering"})
        );
        assert_eq!(
            b["messages"][3],
            json!({"role": "assistant", "content": "b", "reasoning_content": ""})
        );
        req.thinking = ThinkingLevel::Off;
        let b = body(&req);
        assert_eq!(b["thinking"], json!({"type": "disabled"}));
        assert!(b.get("reasoning_effort").is_none());
    }

    #[test]
    fn zai_profile_with_tool_stream() {
        let mut req = request(json!({
            "id": "glm-4.6", "provider": "zai", "api": "openai-completions",
            "baseUrl": "https://api.z.ai/api/coding/paas/v4", "reasoning": true,
            "compat": {"zaiToolStream": true}
        }));
        req.thinking = ThinkingLevel::Low;
        req.tools = vec![read_tool()];
        let b = body(&req);
        assert_eq!(
            b["thinking"],
            json!({"type": "enabled", "clear_thinking": false})
        );
        assert!(
            b.get("reasoning_effort").is_none(),
            "zai does not support reasoning_effort"
        );
        assert_eq!(b["tool_stream"], true);
        assert_eq!(b["max_tokens"], 16384);
        req.thinking = ThinkingLevel::Off;
        assert_eq!(body(&req)["thinking"], json!({"type": "disabled"}));
        // With effort support and a null mapping, the effort is dropped.
        req.thinking = ThinkingLevel::Low;
        req.model
            .compat
            .insert("supportsReasoningEffort".into(), json!(true));
        req.model
            .thinking_level_map
            .insert("low".into(), Some("lite".into()));
        assert_eq!(body(&req)["reasoning_effort"], "lite");
    }

    #[test]
    fn qwen_together_groq_and_cerebras_defaults() {
        let mut req = request(json!({
            "id": "qwen3", "provider": "custom", "api": "openai-completions",
            "baseUrl": "http://localhost:8000/v1", "reasoning": true,
            "compat": {"thinkingFormat": "qwen", "thinkingTokenBudgetField": "thinking_budget"}
        }));
        req.thinking = ThinkingLevel::Medium;
        req.max_tokens = Some(4000);
        let b = body(&req);
        assert_eq!(b["enable_thinking"], true);
        assert_eq!(b["reasoning_effort"], "medium");
        assert_eq!(
            b["thinking_budget"],
            4000 - 1024,
            "budget leaves room for the answer"
        );
        req.thinking = ThinkingLevel::Off;
        let b = body(&req);
        assert_eq!(b["enable_thinking"], false);
        assert!(b.get("thinking_budget").is_none());

        let mut req = request(json!({
            "id": "deepseek-ai/DeepSeek-R1", "provider": "together", "api": "openai-completions",
            "baseUrl": "https://api.together.xyz/v1", "reasoning": true
        }));
        req.thinking = ThinkingLevel::High;
        req.cache_retention = CacheRetention::Long;
        req.session_id = "s".into();
        let b = body(&req);
        assert_eq!(b["reasoning"], json!({"enabled": true}));
        assert!(b.get("reasoning_effort").is_none());
        assert!(
            b.get("prompt_cache_key").is_none(),
            "together has no long retention"
        );
        assert_eq!(b["max_tokens"], 16384);

        let req = request(json!({
            "id": "openai/gpt-oss-120b", "provider": "groq", "api": "openai-completions",
            "baseUrl": "https://api.groq.com/openai/v1", "reasoning": true
        }));
        let b = body(&req);
        assert_eq!(b["store"], false);
        assert_eq!(b["max_completion_tokens"], 16384);
        assert_eq!(b["stream_options"], json!({"include_usage": true}));

        let mut req = request(json!({
            "id": "gpt-oss-120b", "provider": "cerebras", "api": "openai-completions",
            "baseUrl": "https://api.cerebras.ai/v1", "reasoning": true
        }));
        req.system = "s".into();
        req.thinking = ThinkingLevel::Low;
        let b = body(&req);
        assert!(b.get("store").is_none());
        assert_eq!(b["messages"][0]["role"], "system");
        assert_eq!(b["max_completion_tokens"], 16384);
        assert_eq!(b["reasoning_effort"], "low");
    }

    #[test]
    fn remaining_thinking_formats() {
        let base = |format: &str, extra: Value| {
            let mut compat = json!({"thinkingFormat": format});
            for (k, v) in extra.as_object().unwrap() {
                compat[k] = v.clone();
            }
            let mut req = request(json!({
                "id": "m", "provider": "custom", "api": "openai-completions",
                "baseUrl": "http://localhost/v1", "reasoning": true, "maxTokens": 10000,
                "thinkingLevelMap": {"high": "deep", "off": null},
                "compat": compat
            }));
            req.thinking = ThinkingLevel::High;
            req
        };
        let b = body(&base("qwen-chat-template", json!({})));
        assert_eq!(
            b["chat_template_kwargs"],
            json!({"enable_thinking": true, "preserve_thinking": true})
        );

        let req = base(
            "chat-template",
            json!({"chatTemplateKwargs": {
                "enable_thinking": {"$var": "thinking.enabled"},
                "effort": {"$var": "thinking.effort"},
                "budget": {"$var": "thinking.budget", "omitWhenOff": true},
                "fixed": 3
            }}),
        );
        assert_eq!(
            body(&req)["chat_template_kwargs"],
            json!({"enable_thinking": true, "effort": "deep", "budget": 10000 - 1024, "fixed": 3})
        );
        let mut off = req.clone();
        off.thinking = ThinkingLevel::Off;
        // off: enabled=false, effort mapped to null (dropped), budget omitted when off.
        assert_eq!(
            body(&off)["chat_template_kwargs"],
            json!({"enable_thinking": false, "fixed": 3})
        );

        let req = base(
            "baseten",
            json!({"chatTemplateArgs": {"enable_thinking": {"$var": "thinking.enabled"}}}),
        );
        let b = body(&req);
        assert_eq!(b["chat_template_args"], json!({"enable_thinking": true}));
        assert_eq!(b["reasoning_effort"], "deep");

        let b = body(&base("string-thinking", json!({})));
        assert_eq!(b["thinking"], "deep");
        let mut off = base("string-thinking", json!({}));
        off.thinking = ThinkingLevel::Off;
        assert!(
            body(&off).get("thinking").is_none(),
            "null off mapping sends nothing"
        );

        let b = body(&base("ant-ling", json!({})));
        assert_eq!(b["reasoning"], json!({"effort": "deep"}));
        let mut unmapped = base("ant-ling", json!({}));
        unmapped.thinking = ThinkingLevel::Low;
        assert!(
            body(&unmapped).get("reasoning").is_none(),
            "ant-ling needs a mapped string"
        );

        let b = body(&base(
            "openai",
            json!({"vllmPriority": 5, "supportsThinkingTokenBudget": true}),
        ));
        assert_eq!(b["reasoning_effort"], "deep");
        assert_eq!(b["priority"], 5);
        assert_eq!(b["thinking_token_budget"], 10000 - 1024);
    }

    #[test]
    fn sampling_params_and_options_override_named_fields() {
        let mut m = openai_model();
        m["samplingParams"] = json!({"temperature": 0.2, "top_k": 40});
        let mut req = request(m);
        req.options.insert("temperature".into(), json!(0.9));
        req.options.insert("toolChoice".into(), json!("auto"));
        req.options
            .insert("samplingParams".into(), json!({"top_k": 10}));
        let b = body(&req);
        assert_eq!(b["temperature"], 0.2, "sampling params are merged last");
        assert_eq!(b["top_k"], 10);
        assert_eq!(b["tool_choice"], "auto");
    }

    #[test]
    fn tool_history_without_tools_sends_an_empty_tools_array() {
        let mut req = request(openai_model());
        let model = req.model.clone();
        req.messages = vec![
            user(vec![Block::text("q")]),
            assistant(
                &model,
                vec![Block::tool_call("c1", "read", json!({}))],
                StopReason::ToolUse,
            ),
            tool_result("c1", "read", vec![]),
        ];
        let b = body(&req);
        assert_eq!(b["tools"], json!([]));
        assert_eq!(
            b["messages"][2],
            json!({"role": "tool", "content": "(no tool output)", "tool_call_id": "c1"})
        );
        req.messages.truncate(1);
        assert!(body(&req).get("tools").is_none());
    }

    #[test]
    fn compat_message_flags() {
        let mut req = request(json!({
            "id": "m", "provider": "custom", "api": "openai-completions",
            "baseUrl": "http://localhost/v1", "reasoning": true, "input": ["text", "image"],
            "compat": {"requiresAssistantAfterToolResult": true, "requiresToolResultName": true,
                       "requiresThinkingAsText": true, "supportsStrictMode": true}
        }));
        let model = req.model.clone();
        req.tools = vec![read_tool()];
        let thinking = Block::Thinking {
            thinking: "hmm".into(),
            signature: None,
            redacted: false,
            extra: Map::new(),
        };
        req.messages = vec![
            user(vec![Block::text("q")]),
            assistant(
                &model,
                vec![
                    thinking,
                    Block::text("ok"),
                    Block::tool_call("c1", "read", json!({})),
                ],
                StopReason::ToolUse,
            ),
            tool_result("c1", "read", vec![Block::text("done")]),
            user(vec![Block::text("next")]),
        ];
        let b = body(&req);
        assert_eq!(b["tools"][0]["function"]["strict"], false);
        let msgs = b["messages"].as_array().unwrap();
        assert_eq!(
            msgs[1]["content"],
            json!([{"type": "text", "text": "hmm"}, {"type": "text", "text": "ok"}])
        );
        assert_eq!(
            msgs[2],
            json!({"role": "tool", "content": "done", "tool_call_id": "c1", "name": "read"})
        );
        assert_eq!(
            msgs[3],
            json!({"role": "assistant", "content": "I have processed the tool results."})
        );
        assert_eq!(msgs[4]["role"], "user");
    }

    #[test]
    fn tool_call_ids_are_normalized_for_chat_completions() {
        let mut req = request(openai_model());
        let foreign = assistant(
            &serde_json::from_value(
                json!({"id": "codex", "provider": "openai-codex", "api": "openai-codex-responses"}),
            )
            .unwrap(),
            vec![
                Block::tool_call("call_abc|fc_123", "read", json!({})),
                Block::tool_call(
                    format!("call_{}|fc_{}", "a".repeat(30), "b+/=".repeat(10)),
                    "read",
                    json!({}),
                ),
                Block::tool_call("x".repeat(50), "read", json!({})),
            ],
            StopReason::ToolUse,
        );
        req.messages = vec![user(vec![Block::text("q")]), foreign];
        let b = body(&req);
        let calls = b["messages"][1]["tool_calls"].as_array().unwrap();
        assert_eq!(calls[0]["id"], "call_abc_fc_123");
        let long = calls[1]["id"].as_str().unwrap();
        assert!(long.len() <= 40 && long.starts_with("call_aaaa"), "{long}");
        assert_eq!(calls[2]["id"], "x".repeat(40));
        // Results follow the mapping (the synthetic orphan results use the new ids).
        assert_eq!(b["messages"][2]["tool_call_id"], "call_abc_fc_123");
    }

    #[test]
    fn compat_detection_table() {
        let detect = |provider: &str, base: &str, id: &str| {
            let m: Model =
                serde_json::from_value(json!({"id": id, "provider": provider, "baseUrl": base}))
                    .unwrap();
            Compat::detect(&m, base)
        };
        let c = detect("openrouter", "https://openrouter.ai/api/v1", "openai/gpt-5");
        assert!(c.supports_developer_role && c.send_session_affinity_headers);
        assert_eq!(c.session_affinity_format, "openrouter");
        assert_eq!(c.cache_control_format, None);
        let c = detect("xai", "https://api.x.ai/v1", "grok");
        assert!(!c.supports_reasoning_effort && !c.supports_store);
        let c = detect("moonshotai", "https://api.moonshot.ai/v1", "kimi");
        assert_eq!(c.max_tokens_field, "max_tokens");
        let c = detect("ant-ling", "https://api.ant-ling.com/v1", "ling");
        assert_eq!(c.thinking_format, "ant-ling");
        assert!(!c.supports_long_cache_retention);
        let c = detect("x", "https://llm.chutes.ai/v1", "m");
        assert_eq!(c.max_tokens_field, "max_tokens");
        let c = detect("openai", "https://api.openai.com/v1", "gpt");
        assert!(c.supports_store && c.supports_developer_role && c.supports_reasoning_effort);
        assert!(!c.supports_strict_mode && c.supports_finish_reason);
    }

    #[test]
    fn sdk_error_formats_match_the_reference() {
        let body = r#"{"error":{"message":"bad things","type":"invalid_request_error"}}"#;
        let formatted = sdk_error(&format!("400: {body}"), None);
        assert!(formatted.starts_with("400: {"), "{formatted}");
        assert!(formatted.contains("\"message\":\"bad things\""));
        let formatted = sdk_error(&format!("400: {body}"), Some("OpenAI API error"));
        assert!(formatted.starts_with("OpenAI API error (400): {"));
        assert_eq!(sdk_error("502: upstream down", None), "502 upstream down");
        assert_eq!(
            sdk_error("502: upstream down", Some("OpenAI API error")),
            "OpenAI API error (502): 502 upstream down"
        );
        assert_eq!(
            sdk_error("400 status code (no body)", None),
            "400 status code (no body)"
        );
        assert_eq!(sdk_error("connection refused", None), "connection refused");
        let or = r#"{"error":{"message":"Provider returned error","code":502,"metadata":{"raw":"upstream \"boom\""}}}"#;
        let formatted = sdk_error(&format!("502: {or}"), None);
        assert!(formatted.ends_with("\nupstream \"boom\""), "{formatted}");
    }

    #[test]
    fn reasoning_details_replay_is_preferred_over_raw_fields() {
        let mut req = request(json!({
            "id": "openai/gpt-5", "provider": "openrouter", "api": "openai-completions",
            "baseUrl": "https://openrouter.ai/api/v1", "reasoning": true
        }));
        let model = req.model.clone();
        let details = json!([{"type": "reasoning.encrypted", "data": "enc", "id": "r1"}]);
        let thinking = Block::Thinking {
            thinking: "t".into(),
            signature: Some(details.to_string()),
            redacted: false,
            extra: Map::new(),
        };
        let mut legacy_extra = Map::new();
        legacy_extra.insert(
            "thoughtSignature".into(),
            json!(json!({"type": "reasoning.encrypted", "data": "old", "id": "r0"}).to_string()),
        );
        req.messages = vec![
            user(vec![Block::text("q")]),
            assistant(&model, vec![thinking, Block::text("a")], StopReason::Stop),
            assistant(
                &model,
                vec![Block::ToolCall {
                    id: "c".into(),
                    name: "read".into(),
                    arguments: json!({}),
                    extra: legacy_extra,
                }],
                StopReason::ToolUse,
            ),
        ];
        let b = body(&req);
        assert_eq!(
            b["messages"][1],
            json!({"role": "assistant", "content": "a", "reasoning_details": details})
        );
        assert_eq!(
            b["messages"][2]["reasoning_details"],
            json!([{"type": "reasoning.encrypted", "data": "old", "id": "r0"}])
        );
    }

    // ----- streams -----

    fn chunk(delta: Value) -> Value {
        json!({"id": "chatcmpl-1", "model": "gpt-5-2025", "choices": [{"index": 0, "delta": delta}]})
    }

    #[tokio::test]
    async fn fragmented_stream_with_reasoning_text_tools_and_usage() {
        let chunks = vec![
            chunk(json!({"role": "assistant", "reasoning_content": "Think "})),
            chunk(json!({"reasoning_content": "h\u{e9}re \u{1f600}", "reasoning": "dup"})),
            chunk(json!({"content": "Hello "})),
            chunk(
                json!({"tool_calls": [{"index": 0, "id": "call_1", "type": "function", "function": {"name": "read", "arguments": "{\"pa"}}]}),
            ),
            chunk(
                json!({"tool_calls": [{"index": 0, "function": {"arguments": "th\":\"a\u{e9}.rs\"}"}}]}),
            ),
            chunk(
                json!({"tool_calls": [{"index": 1, "id": "call_2", "function": {"name": "bash", "arguments": ""}}]}),
            ),
            chunk(json!({"content": "world"})),
            json!({"id": "chatcmpl-1", "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]}),
            json!({"id": "chatcmpl-1", "choices": [], "usage": {
                "prompt_tokens": 100, "completion_tokens": 20,
                "prompt_tokens_details": {"cached_tokens": 30, "cache_write_tokens": 10},
                "completion_tokens_details": {"reasoning_tokens": 5}}}),
        ];
        let sse = completion_sse(&chunks);
        for step in [1, 7, 64] {
            let mut req = request(openai_model());
            req.model.cost.input = 1.0;
            req.thinking = ThinkingLevel::High;
            let run = run_mock(adapter(), req, "/v1", vec![fragmented(&sse, step)]).await;
            let (content, stop, error, extra) = parts(&run.message);
            assert_eq!(error, None, "step {step}");
            assert_eq!(stop, StopReason::ToolUse);
            assert_eq!(
                content,
                &[
                    Block::Thinking {
                        thinking: "Think h\u{e9}re \u{1f600}".into(),
                        signature: Some("reasoning_content".into()),
                        redacted: false,
                        extra: Map::new()
                    },
                    Block::text("Hello world"),
                    Block::tool_call("call_1", "read", json!({"path": "a\u{e9}.rs"})),
                    Block::tool_call("call_2", "bash", json!({})),
                ]
            );
            assert_eq!(extra["responseId"], "chatcmpl-1");
            assert_eq!(extra["responseModel"], "gpt-5-2025");
            assert_eq!(extra["rawStopReason"], "tool_calls");
            let Message::Assistant { usage, .. } = &run.message else {
                panic!()
            };
            assert_eq!(
                (
                    usage.input,
                    usage.output,
                    usage.cache_read,
                    usage.cache_write,
                    usage.total_tokens
                ),
                (60, 20, 30, 10, 120)
            );
            assert_eq!(usage.reasoning, Some(5));
            assert!((usage.cost.input - 60.0 / 1e6).abs() < 1e-12);
            assert_eq!(
                run.deltas
                    .iter()
                    .filter(|d| matches!(d, Delta::Text { .. }))
                    .count(),
                2
            );
            assert!(run.deltas.contains(&Delta::ToolCall {
                block: 2,
                id: "call_1".into(),
                name: "read".into()
            }));
            let r = &run.requests[0];
            assert_eq!(r.path, "/v1/chat/completions");
            assert_eq!(r.header("authorization"), Some("Bearer sk-test"));
            assert_eq!(r.header("content-type"), Some("application/json"));
        }
    }

    #[tokio::test]
    async fn reasoning_details_are_captured_and_replayed() {
        let chunks = vec![
            chunk(
                json!({"reasoning": "Let me ", "reasoning_details": [{"type": "reasoning.text", "text": "Let me ", "format": "anthropic-claude-v1", "index": 0}]}),
            ),
            chunk(
                json!({"reasoning": "think", "reasoning_details": [{"type": "reasoning.text", "text": "think", "signature": "sig-1", "index": 0}]}),
            ),
            chunk(
                json!({"reasoning_details": [{"type": "reasoning.encrypted", "data": "blob", "id": "rs_1"}, {"type": "bogus"}]}),
            ),
            chunk(json!({"content": "Answer"})),
            json!({"id": "x", "choices": [{"delta": {}, "finish_reason": "stop"}]}),
        ];
        let model = json!({"id": "anthropic/claude-sonnet-4", "provider": "openrouter", "api": "openai-completions",
                           "baseUrl": "https://openrouter.ai/api/v1", "reasoning": true});
        let mut req = request(model.clone());
        req.session_id = "sess".into();
        req.thinking = ThinkingLevel::High;
        let run = run_mock(
            adapter(),
            req,
            "",
            vec![fragmented(&completion_sse(&chunks), 5)],
        )
        .await;
        let (content, stop, _, _) = parts(&run.message);
        assert_eq!(stop, StopReason::Stop);
        let Block::Thinking {
            thinking,
            signature,
            ..
        } = &content[0]
        else {
            panic!("{content:?}")
        };
        assert_eq!(thinking, "Let me think");
        let details: Value = serde_json::from_str(signature.as_deref().unwrap()).unwrap();
        assert_eq!(
            details,
            json!([
                {"type": "reasoning.text", "text": "Let me think", "format": "anthropic-claude-v1", "index": 0, "signature": "sig-1"},
                {"type": "reasoning.encrypted", "data": "blob", "id": "rs_1"}
            ])
        );
        assert_eq!(run.requests[0].header("x-session-id"), Some("sess"));
        assert_eq!(run.requests[0].header("x-client-request-id"), None);

        // Round trip: the stored details go back verbatim, not as a raw reasoning field.
        let mut next = request(model);
        next.messages = vec![
            user(vec![Block::text("q")]),
            run.message.clone(),
            user(vec![Block::text("more")]),
        ];
        let b = body(&next);
        assert_eq!(b["messages"][1]["reasoning_details"], details);
        assert!(b["messages"][1].get("reasoning").is_none());
        assert_eq!(b["messages"][1]["content"], "Answer");
    }

    #[tokio::test]
    async fn session_affinity_headers_and_header_overrides() {
        let mut req = request(
            json!({"id": "m", "provider": "custom", "api": "openai-completions",
            "headers": {"X-Model": "1", "X-Drop": "x"},
            "compat": {"sendSessionAffinityHeaders": true}}),
        );
        req.session_id = "abc".into();
        req.headers.insert("X-Drop".into(), None);
        req.headers.insert("X-Extra".into(), Some("2".into()));
        let sse = completion_sse(&[
            json!({"choices": [{"delta": {"content": "hi"}, "finish_reason": "stop"}]}),
        ]);
        let run = run_mock(adapter(), req, "", vec![fragmented(&sse, 1000)]).await;
        let r = &run.requests[0];
        assert_eq!(r.header("session_id"), Some("abc"));
        assert_eq!(r.header("x-client-request-id"), Some("abc"));
        assert_eq!(r.header("x-session-affinity"), Some("abc"));
        assert_eq!(r.header("x-model"), Some("1"));
        assert_eq!(r.header("x-extra"), Some("2"));
        assert_eq!(r.header("x-drop"), None);
        assert_eq!(parts(&run.message).0, &[Block::text("hi")]);
    }

    #[tokio::test]
    async fn missing_finish_reason_is_an_error_unless_the_endpoint_never_sends_one() {
        let sse = completion_sse(&[
            chunk(json!({"content": "partial"})),
            chunk(
                json!({"tool_calls": [{"index": 0, "id": "c", "function": {"name": "read", "arguments": "{}"}}]}),
            ),
        ]);
        let run = run_mock(
            adapter(),
            request(openai_model()),
            "",
            vec![fragmented(&sse, 9)],
        )
        .await;
        let (content, stop, error, _) = parts(&run.message);
        assert_eq!(stop, StopReason::Error);
        assert_eq!(error, Some("Stream ended without finish_reason"));
        assert_eq!(content[0], Block::text("partial"));

        let mut m = openai_model();
        m["compat"] = json!({"supportsFinishReason": false});
        let run = run_mock(adapter(), request(m), "", vec![fragmented(&sse, 9)]).await;
        assert_eq!(parts(&run.message).1, StopReason::ToolUse);
    }

    #[tokio::test]
    async fn content_filter_and_moonshot_style_usage() {
        let sse = completion_sse(&[
            json!({"choices": [{"delta": {"content": "x"}, "finish_reason": "content_filter",
            "usage": {"prompt_tokens": 10, "completion_tokens": 2, "cached_tokens": 4}}]}),
        ]);
        let run = run_mock(
            adapter(),
            request(openai_model()),
            "",
            vec![fragmented(&sse, 4)],
        )
        .await;
        let (_, stop, error, _) = parts(&run.message);
        assert_eq!(stop, StopReason::Error);
        assert_eq!(error, Some("Provider finish_reason: content_filter"));
        let Message::Assistant { usage, .. } = &run.message else {
            panic!()
        };
        assert_eq!((usage.input, usage.cache_read, usage.output), (6, 4, 2));
    }

    #[tokio::test]
    async fn http_errors_retries_and_stream_errors() {
        let err = MockResponse::json(
            400,
            &json!({"error": {"message": "context_length_exceeded: too many tokens", "code": "context_length_exceeded"}}),
        );
        let run = run_mock(adapter(), request(openai_model()), "", vec![err]).await;
        let (_, stop, error, _) = parts(&run.message);
        assert_eq!(stop, StopReason::Error);
        let error = error.unwrap();
        assert!(error.starts_with("400: {"), "{error}");
        assert!(crate::provider::errors::is_context_overflow(
            &run.message,
            0
        ));

        // 429 then success with one retry allowed.
        let mut req = request(openai_model());
        req.options.insert("maxRetries".into(), json!(1));
        req.messages = vec![Message::user_text("retry payload 🦀\n".repeat(65536), 0)];
        let busy = MockResponse::json(429, &json!({"error": {"message": "slow down"}}))
            .with_header("retry-after-ms", "1");
        let ok = fragmented(
            &completion_sse(&[
                json!({"choices": [{"delta": {"content": "ok"}, "finish_reason": "stop"}]}),
            ]),
            100,
        );
        let run = run_mock(adapter(), req, "", vec![busy.clone(), ok]).await;
        assert_eq!(parts(&run.message).1, StopReason::Stop);
        assert_eq!(run.requests.len(), 2);
        assert_eq!(run.requests[0].body, run.requests[1].body);
        assert!(run.requests[0].body.len() > 1024 * 1024);
        // Without retries the 429 surfaces and is retryable at the turn level.
        let run = run_mock(adapter(), request(openai_model()), "", vec![busy]).await;
        assert!(crate::provider::errors::is_retryable(&run.message));

        let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\ndata: {\"error\":{\"message\":\"Provider returned error\",\"code\":502,\"metadata\":{\"raw\":\"upstream overloaded\"}}}\n\n";
        let run = run_mock(
            adapter(),
            request(openai_model()),
            "",
            vec![fragmented(sse, 11)],
        )
        .await;
        let (content, stop, error, _) = parts(&run.message);
        assert_eq!(stop, StopReason::Error);
        assert_eq!(error, Some("Provider returned error\nupstream overloaded"));
        assert_eq!(content, &[Block::text("a")]);

        let run = run_mock(
            adapter(),
            request(openai_model()),
            "",
            vec![fragmented("data: {not json\n\n", 3)],
        )
        .await;
        assert!(
            parts(&run.message)
                .2
                .unwrap()
                .starts_with("Could not parse stream chunk")
        );
    }

    #[tokio::test]
    async fn invalid_tool_arguments_never_become_a_call() {
        let sse = completion_sse(&[
            chunk(
                json!({"tool_calls": [{"index": 0, "id": "c", "function": {"name": "write", "arguments": "{\"text\":\"line\nbreak\"}"}}]}),
            ),
            chunk(
                json!({"tool_calls": [{"index": 1, "id": "d", "function": {"name": "read", "arguments": "{\"path\":\"cut"}}]}),
            ),
            json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}),
        ]);
        let run = run_mock(
            adapter(),
            request(openai_model()),
            "",
            vec![fragmented(&sse, 13)],
        )
        .await;
        let (content, stop, error, _) = parts(&run.message);
        // A raw newline inside a string is repaired; a cut-off argument string is not guessed.
        assert_eq!(
            content[0],
            Block::tool_call("c", "write", json!({"text": "line\nbreak"}))
        );
        assert_eq!(content[1], Block::tool_call("d", "read", json!({})));
        assert_eq!(stop, StopReason::Error);
        assert!(error.unwrap().contains("read"));
    }

    #[tokio::test]
    async fn cancellation_mid_stream_is_aborted_with_partial_content() {
        let body = format!("data: {}\n\n", chunk(json!({"content": "partial"})));
        let message = run_cancelled(adapter(), request(openai_model()), body).await;
        let (content, stop, error, _) = parts(&message);
        assert_eq!(stop, StopReason::Aborted);
        assert_eq!(error, None);
        assert_eq!(content, &[Block::text("partial")]);
    }

    #[tokio::test]
    async fn missing_credentials_and_header_auth() {
        let mut req = request(openai_model());
        req.credential = Credential::None;
        let run = run_mock(
            adapter(),
            req.clone(),
            "",
            vec![MockResponse::json(200, &json!({}))],
        )
        .await;
        assert_eq!(
            parts(&run.message).2,
            Some("No API key for provider: openai")
        );
        assert!(run.requests.is_empty());
        req.headers
            .insert("Authorization".into(), Some("Token abc".into()));
        let sse = completion_sse(&[
            json!({"choices": [{"delta": {"content": "k"}, "finish_reason": "stop"}]}),
        ]);
        let run = run_mock(adapter(), req, "", vec![fragmented(&sse, 50)]).await;
        assert_eq!(run.requests[0].header("authorization"), Some("Token abc"));
        assert_eq!(parts(&run.message).1, StopReason::Stop);
    }

    /// Reference `clampMaxTokensToContext` (`simple-options.ts:18-22`) through
    /// `buildBaseOptions`: 10000 window - 1100 estimated - 4096 safety = 4804.
    fn crowd_context(req: &mut ProviderRequest) {
        req.system = "x".repeat(4000);
        req.messages = vec![Message::user_text("y".repeat(400), 1)];
    }

    #[test]
    fn max_tokens_are_clamped_to_the_context_window() {
        let mut req = request(json!({"id": "gpt", "provider": "openai",
            "api": "openai-completions", "baseUrl": "https://api.openai.com/v1",
            "contextWindow": 10000, "maxTokens": 32000}));
        crowd_context(&mut req);
        assert_eq!(body(&req)["max_completion_tokens"], json!(4804));
        // A cap that already fits is kept.
        req.max_tokens = Some(1000);
        assert_eq!(body(&req)["max_completion_tokens"], json!(1000));
    }
}
