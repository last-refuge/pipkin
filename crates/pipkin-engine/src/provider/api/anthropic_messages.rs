//! `anthropic-messages`: the Anthropic Messages API and compatible endpoints (GitHub Copilot,
//! Cloudflare AI Gateway, OpenRouter, other Anthropic-compatible gateways).
//!
//! Reference: `packages/ai/src/api/anthropic-messages.ts` (+ `github-copilot-headers.ts`). The
//! reference uses `@anthropic-ai/sdk` `client.beta.messages.create(...).asResponse()` and its own
//! SSE reader; this sends the same request (`POST {baseUrl}/v1/messages?beta=true`, betas in
//! `anthropic-beta`) and normalizes the same events.
//!
//! Auth: `x-api-key` for API keys; `Authorization: Bearer` for `Credential::Bearer`
//! (`ANTHROPIC_AUTH_TOKEN`) and for GitHub Copilot tokens (plus Copilot's dynamic headers);
//! configured `authorization` / `x-api-key` / `cf-aig-authorization` headers may carry auth
//! instead. Claude subscription OAuth uses Pi-compatible identity, betas and tool naming.
//! Subscription portions ported from Pi d2a311097cbcf669e699479587332ae3988a49d0,
//! Copyright (c) 2025 Mario Zechner, MIT (packaging/upstream-notices/pi/LICENSE).
//!
//! Request options read from `ProviderRequest::options` (reference option names): `temperature`,
//! `toolChoice`, `metadata`, `thinkingDisplay`, `interleavedThinking`, `thinkingBudgets`,
//! `toolConstrainedSampling`, `maxRetries`, `maxRetryDelayMs`.

use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;
use std::sync::Arc;

use pipkin_engine_protocol::{Block, Message, StopReason};
use serde_json::{Map, Value, json};

use super::common::schema::{resolve_strict, tool_parameters};
use super::common::{
    Headers, adjust_max_tokens_for_thinking, base_max_tokens, clamp_max_tokens_to_context,
    clamp_thinking_budget_to_answer_room, has_configured_header, idle_timeout, opt, opt_str,
    parse_json_with_repair, post_json_observing, reasoning_effort, sanitize_tool_call_id,
    transform_messages,
};
use crate::provider::http::{self, ReadError};
use crate::provider::model::ModelCost;
use crate::provider::sse::SseEvent;
use crate::provider::{
    ApiAdapter, BoxFuture, CacheRetention, Credential, Emitter, Model, ProviderRequest, StreamCtx,
    ThinkingLevel,
};

// Pinned with the reference adapter; provider protocol compatibility, not a Pipkin version.
const CLAUDE_CODE_VERSION: &str = "2.1.280";
const SUBSCRIPTION_SYSTEM: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
const CLAUDE_TOOLS: &[&str] = &[
    "Read",
    "Write",
    "Edit",
    "Bash",
    "Grep",
    "Glob",
    "AskUserQuestion",
    "EnterPlanMode",
    "ExitPlanMode",
    "KillShell",
    "NotebookEdit",
    "Skill",
    "Task",
    "TaskOutput",
    "TodoWrite",
    "WebFetch",
    "WebSearch",
];

fn is_subscription(req: &ProviderRequest) -> bool {
    if req.model.provider == "github-copilot" {
        return false;
    }
    match &req.credential {
        Credential::OAuth { access, .. } if req.model.provider == "anthropic" => !access.is_empty(),
        Credential::ApiKey(access) | Credential::OAuth { access, .. } => {
            access.contains("sk-ant-oat")
        }
        _ => false,
    }
}

fn wire_tool_name<'a>(req: &ProviderRequest, name: &'a str) -> &'a str {
    if is_subscription(req) {
        CLAUDE_TOOLS
            .iter()
            .copied()
            .find(|n| n.eq_ignore_ascii_case(name))
            .unwrap_or(name)
    } else {
        name
    }
}

fn local_tool_name<'a>(req: &'a ProviderRequest, name: &'a str) -> &'a str {
    if is_subscription(req) {
        req.tools
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
            .map(|t| t.name.as_str())
            .unwrap_or(name)
    } else {
        name
    }
}

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const FINE_GRAINED_TOOL_STREAMING_BETA: &str = "fine-grained-tool-streaming-2025-05-14";
const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";
const SERVER_SIDE_FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const MID_CONVERSATION_OUTPUT_CONFIG_BETA: &str = "mid-conversation-output-config-2026-07-01";
const THINKING_BINDING_CONTROLS_BETA: &str = "thinking-binding-controls-2026-08-01";
const INLINE_TOOLS_BETA: &str = "inline-tools-2026-09-15";
const MESSAGE_EVENTS: &[&str] = &[
    "message_start",
    "message_delta",
    "message_stop",
    "content_block_start",
    "content_block_delta",
    "content_block_stop",
];

#[path = "anthropic_federation.rs"]
mod federation;

#[derive(Default)]
pub struct AnthropicMessages {
    federation: federation::Cache,
}

pub fn adapter() -> Arc<dyn ApiAdapter> {
    Arc::new(AnthropicMessages::default())
}

impl ApiAdapter for AnthropicMessages {
    fn api(&self) -> &'static str {
        "anthropic-messages"
    }

    fn stream<'a>(&'a self, ctx: StreamCtx<'a>) -> BoxFuture<'a, Message> {
        Box::pin(run(&self.federation, ctx))
    }
}

// ---------------------------------------------------------------------------------------------
// Compat and auth
// ---------------------------------------------------------------------------------------------

struct Compat {
    supports_eager_tool_input_streaming: bool,
    supports_long_cache_retention: bool,
    send_session_affinity_headers: bool,
    openrouter_affinity: bool,
    supports_cache_control_on_tools: bool,
    supports_temperature: bool,
    allow_empty_signature: bool,
    supports_strict_tools: bool,
    supports_mid_convo_system_messages: bool,
    supports_mid_convo_tool_changes: bool,
    supports_mid_convo_effort: bool,
    force_adaptive_thinking: bool,
    allowed_fallback_models: Vec<Value>,
}

fn compat(model: &Model) -> Compat {
    let open_router = model.provider == "openrouter" || model.base_url.contains("openrouter.ai");
    let b = |key: &str, default: bool| model.compat_bool(key).unwrap_or(default);
    Compat {
        supports_eager_tool_input_streaming: b("supportsEagerToolInputStreaming", true),
        supports_long_cache_retention: b("supportsLongCacheRetention", true),
        send_session_affinity_headers: b("sendSessionAffinityHeaders", open_router),
        openrouter_affinity: model
            .compat_str("sessionAffinityFormat")
            .map_or(open_router, |f| f == "openrouter"),
        supports_cache_control_on_tools: b("supportsCacheControlOnTools", true),
        supports_temperature: b("supportsTemperature", true),
        allow_empty_signature: b("allowEmptySignature", false),
        supports_strict_tools: b("supportsStrictTools", false),
        supports_mid_convo_system_messages: b("supportsMidConvoSystemMessages", false),
        supports_mid_convo_tool_changes: b("supportsMidConvoToolChanges", false),
        supports_mid_convo_effort: model.compat_bool("supportsMidConvoEffort") == Some(true),
        force_adaptive_thinking: model.compat_bool("forceAdaptiveThinking") == Some(true),
        allowed_fallback_models: model
            .compat
            .get("allowedFallbackModels")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    }
}

#[derive(Debug, PartialEq)]
enum Auth {
    ApiKey(String),
    Bearer(String),
    Subscription(String),
    Federation,
    /// Configured headers carry auth.
    Headers,
}

fn has_request_auth_header(req: &ProviderRequest) -> bool {
    ["authorization", "x-api-key", "cf-aig-authorization"]
        .iter()
        .any(|h| has_configured_header(req, h))
}

fn resolve_auth(req: &ProviderRequest) -> Result<Auth, String> {
    let provider = req.model.provider.as_str();
    let copilot = provider == "github-copilot";
    let key = |k: &str| {
        if is_subscription(req) {
            Auth::Subscription(k.to_owned())
        } else if copilot {
            Auth::Bearer(k.to_owned())
        } else {
            Auth::ApiKey(k.to_owned())
        }
    };
    match &req.credential {
        Credential::OAuth { access, .. } if !access.is_empty() => Ok(key(access)),
        Credential::ApiKey(k) if !k.is_empty() => Ok(key(k)),
        Credential::Bearer(t) if !t.is_empty() => Ok(Auth::Bearer(t.clone())),
        _ if has_request_auth_header(req) => Ok(Auth::Headers),
        _ if federation::Config::from_request(req).is_some() => Ok(Auth::Federation),
        _ => Err(format!("No API key for provider: {provider}")),
    }
}

// ---------------------------------------------------------------------------------------------
// Thinking
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
struct Thinking {
    /// `Some(false)` disables thinking explicitly; `Some(true)` enables it.
    enabled: bool,
    effort: Option<String>,
    budget: Option<u64>,
    max_tokens: u64,
}

const EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// Reference `mapThinkingLevelToEffort`: a string mapping wins, else minimal/low -> low.
fn effort_for(model: &Model, level: ThinkingLevel) -> String {
    if let Some(mapped) = model.thinking_value(level) {
        return mapped.to_owned();
    }
    match level {
        ThinkingLevel::Minimal | ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        _ => "high",
    }
    .to_owned()
}

/// Reference `streamSimple`: map the level onto adaptive effort or a token budget.
fn plan_thinking(req: &ProviderRequest, c: &Compat) -> Thinking {
    let base = base_max_tokens(req);
    // The agent sends no reasoning option for `off` (reference `streamSimple`: thinking
    // disabled), so a requested `off` is never clamped up to a minimum level.
    let Some(level) = reasoning_effort(req) else {
        return Thinking {
            enabled: false,
            effort: None,
            budget: None,
            max_tokens: base,
        };
    };
    if c.force_adaptive_thinking {
        return Thinking {
            enabled: true,
            effort: Some(effort_for(&req.model, level)),
            budget: None,
            max_tokens: base,
        };
    }
    let (adjusted, budget) = adjust_max_tokens_for_thinking(req, Some(base), level);
    let max_tokens = clamp_max_tokens_to_context(req, adjusted);
    Thinking {
        enabled: true,
        effort: None,
        budget: Some(clamp_thinking_budget_to_answer_room(budget, max_tokens)),
        max_tokens,
    }
}

// ---------------------------------------------------------------------------------------------
// Request conversion
// ---------------------------------------------------------------------------------------------

fn cache_control(req: &ProviderRequest, c: &Compat) -> Option<Value> {
    match req.cache_retention {
        CacheRetention::None => None,
        CacheRetention::Long if c.supports_long_cache_retention => {
            Some(json!({"type": "ephemeral", "ttl": "1h"}))
        }
        _ => Some(json!({"type": "ephemeral"})),
    }
}

fn image(mime_type: &str, data: &str) -> Value {
    json!({"type": "image", "source": {"type": "base64", "media_type": mime_type, "data": data}})
}

/// Tool-result content: a string when text-only, else blocks (image-only gets a placeholder).
fn convert_content_blocks(content: &[Block]) -> Value {
    let has_images = content.iter().any(|b| matches!(b, Block::Image { .. }));
    if !has_images {
        let text: Vec<&str> = content.iter().filter_map(Block::as_text).collect();
        return Value::String(text.join("\n"));
    }
    let mut blocks: Vec<Value> = content
        .iter()
        .filter_map(|b| match b {
            Block::Text { text, .. } => Some(json!({"type": "text", "text": text})),
            Block::Image {
                data, mime_type, ..
            } => Some(image(mime_type, data)),
            _ => None,
        })
        .collect();
    if !blocks.iter().any(|b| b["type"] == "text") {
        blocks.insert(0, json!({"type": "text", "text": "(see attached image)"}));
    }
    Value::Array(blocks)
}

/// Messages on the wire, plus the effort each historical managed-effort assistant used.
fn convert_messages(
    req: &ProviderRequest,
    cache_control: Option<&Value>,
    allow_empty_signature: bool,
    managed_provider: Option<&str>,
) -> Result<(Vec<Value>, HashMap<usize, String>), String> {
    let mut normalize = |id: &str, _: &Message| sanitize_tool_call_id(id);
    let transformed = transform_messages(&req.messages, &req.model, Some(&mut normalize));
    let mut params: Vec<Value> = Vec::new();
    let mut levels = HashMap::new();
    let mut pending_systems = Vec::new();
    let mut i = 0;
    while i < transformed.len() {
        match &transformed[i] {
            Message::System(system) => {
                let text = crate::context::system::update_text(system);
                let mut blocks = Vec::new();
                if !text.is_empty() {
                    blocks.push(json!({"type": "text", "text": text}));
                }
                if req
                    .options
                    .get("pipkin.systemNativeTools")
                    .and_then(Value::as_bool)
                    == Some(true)
                {
                    for removed in &system.tools_removed {
                        if !system.tools_added.iter().any(|t| t.name == removed.name) {
                            blocks.push(json!({"type": "tool_removal", "tool": {"type": "tool_reference", "name": wire_tool_name(req, &removed.name)}}));
                        }
                    }
                    let mut addition_request = req.clone();
                    addition_request.tools = system
                        .tools_added
                        .iter()
                        .map(crate::provider::ToolSpec::from)
                        .collect();
                    for definition in convert_tools(&addition_request, &compat(&req.model), None)? {
                        blocks.push(json!({"type": "tool_addition", "tool": {"type": "tool_definition", "definition": definition}}));
                    }
                }
                if !blocks.is_empty() {
                    pending_systems.push(json!({"role": "system", "content": blocks}));
                }
            }
            Message::User { content, .. } => {
                let blocks: Vec<Value> = content
                    .iter()
                    .filter_map(|b| match b {
                        Block::Text { text, .. } if !text.trim().is_empty() => {
                            Some(json!({"type": "text", "text": text}))
                        }
                        Block::Image {
                            data, mime_type, ..
                        } => Some(image(mime_type, data)),
                        _ => None,
                    })
                    .collect();
                if !blocks.is_empty() {
                    params.push(json!({"role": "user", "content": blocks}));
                }
            }
            Message::Assistant {
                content,
                api,
                provider,
                extra,
                ..
            } => {
                params.append(&mut pending_systems);
                let mut blocks = Vec::new();
                for block in content {
                    match block {
                        Block::Text { text, .. } => {
                            if !text.trim().is_empty() {
                                blocks.push(json!({"type": "text", "text": text}));
                            }
                        }
                        Block::Thinking {
                            thinking,
                            signature,
                            redacted,
                            ..
                        } => {
                            if *redacted {
                                blocks.push(json!({"type": "redacted_thinking",
                                    "data": signature.clone().unwrap_or_default()}));
                                continue;
                            }
                            let signed = signature.as_deref().is_some_and(|s| !s.trim().is_empty());
                            if thinking.trim().is_empty() && !signed {
                                continue;
                            }
                            if signed {
                                blocks.push(json!({"type": "thinking", "thinking": thinking,
                                    "signature": signature}));
                            } else if allow_empty_signature {
                                blocks.push(json!({"type": "thinking", "thinking": thinking,
                                    "signature": ""}));
                            } else {
                                blocks.push(json!({"type": "text", "text": thinking}));
                            }
                        }
                        Block::ToolCall {
                            id,
                            name,
                            arguments,
                            ..
                        } => {
                            let input = if arguments.is_null() {
                                json!({})
                            } else {
                                arguments.clone()
                            };
                            blocks.push(
                                json!({"type": "tool_use", "id": id, "name": wire_tool_name(req, name), "input": input}),
                            );
                        }
                        Block::Image { .. } => {}
                    }
                }
                if !blocks.is_empty() {
                    let index = params.len();
                    params.push(json!({"role": "assistant", "content": blocks}));
                    if let Some(managed) = managed_provider
                        && api == "anthropic-messages"
                        && provider == managed
                        && let Some(level) = extra
                            .get("providerThinkingLevel")
                            .and_then(Value::as_str)
                            .filter(|l| EFFORTS.contains(l))
                    {
                        levels.insert(index, level.to_owned());
                    }
                }
            }
            Message::ToolResult { .. } => {
                // Consecutive tool results become one user message.
                let mut results = Vec::new();
                while let Some(Message::ToolResult {
                    tool_call_id,
                    content,
                    is_error,
                    ..
                }) = transformed.get(i)
                {
                    results.push(json!({"type": "tool_result", "tool_use_id": tool_call_id,
                        "content": convert_content_blocks(content), "is_error": is_error}));
                    i += 1;
                }
                params.push(json!({"role": "user", "content": results}));
                continue;
            }
        }
        i += 1;
    }
    params.append(&mut pending_systems);
    if let Some(cc) = cache_control
        && let Some(last) = params.last_mut()
        && (last["role"] == "user" || last["role"] == "system")
        && let Some(block) = last["content"]
            .as_array_mut()
            .and_then(|blocks| blocks.last_mut())
        && matches!(
            block["type"].as_str(),
            Some("text" | "image" | "tool_result" | "tool_addition" | "tool_removal")
        )
    {
        block["cache_control"] = cc.clone();
    }
    Ok((params, levels))
}

/// Per-turn effort messages for managed-effort models.
fn insert_thinking_level_messages(
    messages: Vec<Value>,
    levels: &HashMap<usize, String>,
    active: &str,
) -> Vec<Value> {
    let mut out = Vec::with_capacity(messages.len() + levels.len() + 1);
    for (index, message) in messages.into_iter().enumerate() {
        if let Some(effort) = levels.get(&index) {
            out.push(json!({"role": "system", "content": [], "output_config": {"effort": effort}}));
        }
        out.push(message);
    }
    out.push(json!({"role": "system", "content": [], "output_config": {"effort": active}}));
    out
}

/// Keywords Anthropic strict tool use rejects for the whole request.
fn anthropic_strict_unsupported(key: &str, value: &Value) -> bool {
    const UNSUPPORTED: &[&str] = &[
        "minimum",
        "maximum",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "multipleOf",
        "maxItems",
        "uniqueItems",
        "minContains",
        "maxContains",
        "minProperties",
        "maxProperties",
    ];
    const FORMATS: &[&str] = &[
        "date-time",
        "time",
        "date",
        "duration",
        "email",
        "hostname",
        "uri",
        "ipv4",
        "ipv6",
        "uuid",
    ];
    if UNSUPPORTED.contains(&key) {
        return true;
    }
    match key {
        "minItems" => value.as_f64() != Some(0.0) && value.as_f64() != Some(1.0),
        "format" => !value.as_str().is_some_and(|f| FORMATS.contains(&f)),
        _ => false,
    }
}

fn convert_tools(
    req: &ProviderRequest,
    c: &Compat,
    cache_control: Option<&Value>,
) -> Result<Vec<Value>, String> {
    let mut out = Vec::with_capacity(req.tools.len());
    let last = req.tools.len().saturating_sub(1);
    for (index, tool) in req.tools.iter().enumerate() {
        let strict = resolve_strict(
            req,
            tool,
            c.supports_strict_tools,
            Some(&anthropic_strict_unsupported),
        )?;
        let parameters = tool_parameters(tool, strict)?;
        let legacy = json!({
            "type": "object",
            "properties": parameters.get("properties").cloned().unwrap_or_else(|| json!({})),
            "required": parameters.get("required").cloned().unwrap_or_else(|| json!([])),
        });
        let input_schema = if strict == Some(true) {
            let mut merged = parameters.as_object().cloned().unwrap_or_default();
            for (k, v) in legacy.as_object().into_iter().flatten() {
                merged.insert(k.clone(), v.clone());
            }
            Value::Object(merged)
        } else {
            legacy
        };
        let mut t = Map::new();
        t.insert("name".into(), json!(wire_tool_name(req, &tool.name)));
        t.insert("description".into(), json!(tool.description));
        if c.supports_eager_tool_input_streaming {
            t.insert("eager_input_streaming".into(), json!(true));
        }
        if strict == Some(true) {
            t.insert("strict".into(), json!(true));
        }
        t.insert("input_schema".into(), input_schema);
        if let Some(cc) = cache_control
            && index == last
        {
            t.insert("cache_control".into(), cc.clone());
        }
        out.push(Value::Object(t));
    }
    Ok(out)
}

/// The configured `anthropic-beta` header: `Some(None)` when configured as null (no betas).
fn configured_betas(req: &ProviderRequest) -> Option<Option<String>> {
    let mut configured = None;
    for (k, v) in &req.model.headers {
        if k.eq_ignore_ascii_case("anthropic-beta") {
            configured = Some(Some(v.clone()));
        }
    }
    for (k, v) in &req.headers {
        if k.eq_ignore_ascii_case("anthropic-beta") {
            configured = Some(v.clone());
        }
    }
    configured
}

fn dedup(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|s| seen.insert(s.clone()))
        .collect()
}

fn betas(
    req: &ProviderRequest,
    c: &Compat,
    thinking: &Thinking,
    native_tools: bool,
) -> Vec<String> {
    match configured_betas(req) {
        Some(None) => return vec![],
        Some(Some(v)) => {
            return dedup(
                v.split(',')
                    .map(str::trim)
                    .filter(|f| !f.is_empty())
                    .map(str::to_owned),
            );
        }
        None => {}
    }
    let mut features = Vec::new();
    if is_subscription(req) {
        features.extend(["claude-code-20250219", "oauth-2025-04-20"]);
    }
    if !req.tools.is_empty() && !c.supports_eager_tool_input_streaming {
        features.push(FINE_GRAINED_TOOL_STREAMING_BETA);
    }
    if req.model.reasoning
        && thinking.enabled
        && opt(req, "interleavedThinking").and_then(Value::as_bool) != Some(false)
        && !c.force_adaptive_thinking
    {
        features.push(INTERLEAVED_THINKING_BETA);
    }
    if !c.allowed_fallback_models.is_empty() {
        features.push(SERVER_SIDE_FALLBACK_BETA);
    }
    if c.supports_mid_convo_effort {
        features.push(MID_CONVERSATION_OUTPUT_CONFIG_BETA);
        features.push(THINKING_BINDING_CONTROLS_BETA);
    }
    if native_tools {
        features.push(INLINE_TOOLS_BETA);
    }
    dedup(features.into_iter().map(str::to_owned))
}

/// The request body and the betas for its `anthropic-beta` header.
fn build_params(
    req: &ProviderRequest,
    c: &Compat,
    thinking: &Thinking,
) -> Result<(Value, Vec<String>), String> {
    let resolved = req.resolve_systems(c.supports_mid_convo_system_messages);
    let req = resolved.as_ref();
    let model = &req.model;
    let cc = cache_control(req, c);
    // Native tool changes keep the request-level tool list fixed (plus a deferred placeholder
    // that keeps Anthropic's tool-change scaffolding in the cached prefix).
    let native_tools = c.supports_mid_convo_system_messages
        && c.supports_mid_convo_tool_changes
        && !req.tools.is_empty()
        && req
            .options
            .get("pipkin.systemNativeTools")
            .and_then(Value::as_bool)
            .unwrap_or(true);
    let (messages, levels) = convert_messages(
        req,
        cc.as_ref(),
        c.allow_empty_signature,
        c.supports_mid_convo_effort
            .then_some(model.provider.as_str()),
    )?;
    let active_effort = thinking.effort.clone().unwrap_or_else(|| "high".into());
    let betas = betas(req, c, thinking, native_tools);
    let mut p = Map::new();
    p.insert("model".into(), json!(model.id));
    p.insert(
        "messages".into(),
        Value::Array(if c.supports_mid_convo_effort {
            insert_thinking_level_messages(messages, &levels, &active_effort)
        } else {
            messages
        }),
    );
    p.insert("max_tokens".into(), json!(thinking.max_tokens));
    p.insert("stream".into(), json!(true));
    let mut system = Vec::new();
    if is_subscription(req) {
        system.push(SUBSCRIPTION_SYSTEM);
    }
    if !req.system.is_empty() {
        system.push(&req.system);
    }
    if !system.is_empty() {
        let blocks: Vec<Value> = system
            .into_iter()
            .map(|text| {
                let mut block = json!({"type": "text", "text": text});
                if let Some(cc) = &cc {
                    block["cache_control"] = cc.clone();
                }
                block
            })
            .collect();
        p.insert("system".into(), json!(blocks));
    }
    if let Some(t) = opt(req, "temperature").filter(|v| v.is_number())
        && !thinking.enabled
        && !c.supports_mid_convo_effort
        && c.supports_temperature
    {
        p.insert("temperature".into(), t.clone());
    }
    let tool_cc = cc.as_ref().filter(|_| c.supports_cache_control_on_tools);
    if native_tools {
        let mut tools = convert_tools(req, c, tool_cc)?;
        tools.push(json!({
            "name": "__pi_deferred_placeholder__",
            "description": "Reserved placeholder. Never available. Never call this.",
            "input_schema": {"type": "object", "properties": {}, "required": []},
            "defer_loading": true,
        }));
        p.insert("tools".into(), Value::Array(tools));
    } else if !req.tools.is_empty() {
        p.insert(
            "tools".into(),
            Value::Array(convert_tools(req, c, tool_cc)?),
        );
    }
    let display = opt_str(req, "thinkingDisplay").unwrap_or("summarized");
    if c.supports_mid_convo_effort {
        p.insert(
            "thinking".into(),
            json!({"type": "adaptive", "display": display,
                "block_binding": {"prefix_mismatch_behavior": "drop_block"}}),
        );
        p.insert("output_config".into(), json!({"effort": "high"}));
    } else if model.reasoning {
        if thinking.enabled {
            if c.force_adaptive_thinking {
                p.insert(
                    "thinking".into(),
                    json!({"type": "adaptive", "display": display}),
                );
                if let Some(effort) = &thinking.effort {
                    p.insert("output_config".into(), json!({"effort": effort}));
                }
            } else {
                let budget = thinking.budget.filter(|b| *b > 0).unwrap_or(1024);
                p.insert(
                    "thinking".into(),
                    json!({"type": "enabled", "budget_tokens": budget, "display": display}),
                );
            }
        } else if !matches!(model.thinking_level_map.get("off"), Some(None)) {
            p.insert("thinking".into(), json!({"type": "disabled"}));
        }
    }
    if let Some(user_id) = opt(req, "metadata")
        .and_then(|m| m.get("user_id"))
        .and_then(Value::as_str)
    {
        p.insert("metadata".into(), json!({"user_id": user_id}));
    }
    match opt(req, "toolChoice") {
        Some(Value::String(s)) if !s.is_empty() => {
            p.insert("tool_choice".into(), json!({"type": s}));
        }
        Some(v @ Value::Object(_)) => {
            p.insert("tool_choice".into(), v.clone());
        }
        _ => {}
    }
    if !c.allowed_fallback_models.is_empty() {
        let fallbacks: Vec<Value> = c
            .allowed_fallback_models
            .iter()
            .map(|f| json!({"model": f.get("model").cloned().unwrap_or(Value::Null)}))
            .collect();
        p.insert("fallbacks".into(), Value::Array(fallbacks));
    }
    Ok((Value::Object(p), betas))
}

/// Copilot's per-request headers (`github-copilot-headers.ts`).
fn copilot_headers(req: &ProviderRequest) -> Vec<(&'static str, &'static str)> {
    let initiator = match req.messages.last() {
        Some(Message::User { .. }) => "user",
        _ => "agent",
    };
    let mut h = vec![
        ("X-Initiator", initiator),
        ("Openai-Intent", "conversation-edits"),
    ];
    let vision = req.messages.iter().any(|m| {
        matches!(m, Message::User { .. } | Message::ToolResult { .. })
            && m.content().iter().any(|b| matches!(b, Block::Image { .. }))
    });
    if vision {
        h.push(("Copilot-Vision-Request", "true"));
    }
    h
}

fn build_headers(req: &ProviderRequest, c: &Compat, auth: &Auth, betas: &[String]) -> Headers {
    let mut h = Headers::default();
    h.set("accept", "application/json");
    h.set("anthropic-version", "2023-06-01");
    h.set("anthropic-dangerous-direct-browser-access", "true");
    match auth {
        Auth::ApiKey(k) => h.set("x-api-key", k.clone()),
        Auth::Bearer(t) => h.set("authorization", format!("Bearer {t}")),
        Auth::Subscription(t) => {
            h.set("authorization", format!("Bearer {t}"));
            h.set("user-agent", format!("claude-cli/{CLAUDE_CODE_VERSION}"));
            h.set("x-app", "cli");
        }
        Auth::Headers | Auth::Federation => {}
    }
    if req.model.provider == "github-copilot" {
        h.merge(&req.model.headers);
        for (k, v) in copilot_headers(req) {
            h.set(k, v);
        }
    } else {
        if !is_subscription(req)
            && req.cache_retention != CacheRetention::None
            && !req.session_id.is_empty()
            && c.send_session_affinity_headers
        {
            let name = if c.openrouter_affinity {
                "x-session-id"
            } else {
                "x-session-affinity"
            };
            h.set(name, req.session_id.clone());
        }
        h.merge(&req.model.headers);
    }
    h.merge_optional(&req.headers);
    if !betas.is_empty() {
        h.set("anthropic-beta", betas.join(","));
    }
    h
}

/// `{baseUrl}/v1/messages?beta=true`, with Cloudflare gateway placeholders filled from env.
fn request_url(req: &ProviderRequest) -> String {
    let mut base = req.base_url().trim().to_owned();
    for name in ["CLOUDFLARE_ACCOUNT_ID", "CLOUDFLARE_GATEWAY_ID"] {
        let placeholder = format!("{{{name}}}");
        if base.contains(&placeholder)
            && let Some(v) = req.env_var(name)
        {
            base = base.replace(&placeholder, &v);
        }
    }
    let base = base.trim_end_matches('/');
    let base = if base.is_empty() {
        DEFAULT_BASE_URL
    } else {
        base
    };
    format!("{base}/v1/messages?beta=true")
}

// ---------------------------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------------------------

fn map_stop_reason(
    reason: &str,
    details: Option<&Value>,
) -> Result<(StopReason, Option<String>), String> {
    Ok(match reason {
        "end_turn" | "pause_turn" | "stop_sequence" => (StopReason::Stop, None),
        "max_tokens" => (StopReason::Length, None),
        "tool_use" => (StopReason::ToolUse, None),
        "refusal" => (
            StopReason::Error,
            Some(
                details
                    .and_then(|d| d.get("explanation"))
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("The model refused to complete the request")
                    .to_owned(),
            ),
        ),
        "sensitive" => (
            StopReason::Error,
            Some("Provider stopped with: sensitive".into()),
        ),
        other => return Err(format!("Unhandled stop reason: {other}")),
    })
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Text,
    Thinking,
    Tool,
}

#[derive(Default)]
struct AnthropicStream {
    blocks: HashMap<u64, (usize, Kind)>,
    tool_json: HashMap<usize, String>,
    saw_start: bool,
    saw_stop: bool,
    stop: Option<StopReason>,
    error_message: Option<String>,
    raw_stop: Option<String>,
    fatal: Option<String>,
    fallback_cost: Option<ModelCost>,
    response_model: Option<String>,
    transformations: Option<Vec<Value>>,
}

fn n(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(Value::as_u64)
}

impl AnthropicStream {
    fn on_event(
        &mut self,
        e: &mut Emitter,
        req: &ProviderRequest,
        ev: SseEvent,
    ) -> ControlFlow<()> {
        let name = ev.event.as_deref().unwrap_or("");
        if name == "error" {
            self.fatal = Some(ev.data);
            return ControlFlow::Break(());
        }
        if !MESSAGE_EVENTS.contains(&name) {
            return ControlFlow::Continue(());
        }
        let event = match parse_json_with_repair(&ev.data) {
            Ok(v) => v,
            Err(err) => {
                self.fatal = Some(format!(
                    "Could not parse Anthropic SSE event {name}: {err}; data={}; raw=event: {name}\\ndata: {}",
                    ev.data, ev.data
                ));
                return ControlFlow::Break(());
            }
        };
        let result = match event.get("type").and_then(Value::as_str).unwrap_or("") {
            "message_start" => {
                self.message_start(e, req, &event);
                Ok(())
            }
            "content_block_start" => self.block_start(e, req, &event),
            "content_block_delta" => {
                self.block_delta(e, &event);
                Ok(())
            }
            "content_block_stop" => {
                self.block_stop(e, &event);
                Ok(())
            }
            "message_delta" => self.message_delta(e, &event),
            "message_stop" => {
                self.saw_stop = true;
                Ok(())
            }
            _ => Ok(()),
        };
        match result {
            Ok(()) => ControlFlow::Continue(()),
            Err(err) => {
                self.fatal = Some(err);
                ControlFlow::Break(())
            }
        }
    }

    fn message_start(&mut self, e: &mut Emitter, req: &ProviderRequest, event: &Value) {
        self.saw_start = true;
        let message = &event["message"];
        if let Some(id) = message.get("id").and_then(Value::as_str) {
            e.set_response_id(id);
        }
        if let Some(t) = message
            .get("input_transformations")
            .and_then(Value::as_array)
        {
            self.transformations = Some(t.clone());
        }
        if let Some(response_model) = message.get("model").and_then(Value::as_str)
            && response_model != req.model.id
        {
            self.response_model = Some(response_model.to_owned());
            self.fallback_cost = compat(&req.model)
                .allowed_fallback_models
                .iter()
                .find(|f| {
                    f.get("provider").and_then(Value::as_str) == Some(req.model.provider.as_str())
                        && f.get("model").and_then(Value::as_str) == Some(response_model)
                })
                .and_then(|f| f.get("cost"))
                .and_then(|c| serde_json::from_value(c.clone()).ok());
        }
        let usage = &message["usage"];
        let cache_1h = usage
            .get("cache_creation")
            .and_then(|c| n(c, "ephemeral_1h_input_tokens"))
            .unwrap_or(0);
        e.set_cache_write_1h(cache_1h);
        let u = e.usage_mut();
        u.input = n(usage, "input_tokens").unwrap_or(0);
        u.output = n(usage, "output_tokens").unwrap_or(0);
        u.cache_read = n(usage, "cache_read_input_tokens").unwrap_or(0);
        u.cache_write = n(usage, "cache_creation_input_tokens").unwrap_or(0);
        u.total_tokens = u.input + u.output + u.cache_read + u.cache_write;
    }

    fn block_start(
        &mut self,
        e: &mut Emitter,
        req: &ProviderRequest,
        event: &Value,
    ) -> Result<(), String> {
        let index = n(event, "index").unwrap_or(0);
        let block = &event["content_block"];
        let str_of = |k: &str| block.get(k).and_then(Value::as_str);
        let entry = match str_of("type").unwrap_or("") {
            "fallback" => {
                if !e.content().is_empty() {
                    return Err(
                        "Anthropic performed an unsupported mid-output model fallback".into(),
                    );
                }
                return Ok(());
            }
            "text" => {
                let i = e.start_text();
                e.text(i, str_of("text").unwrap_or(""));
                (i, Kind::Text)
            }
            "thinking" => {
                let i = e.start_thinking();
                e.thinking(i, str_of("thinking").unwrap_or(""));
                e.thinking_signature(i, str_of("signature").unwrap_or(""), false);
                (i, Kind::Thinking)
            }
            "redacted_thinking" => (
                e.redacted_thinking(str_of("data").unwrap_or("")),
                Kind::Thinking,
            ),
            "tool_use" => {
                let i = e.start_tool_call(
                    str_of("id").unwrap_or(""),
                    local_tool_name(req, str_of("name").unwrap_or("")),
                );
                self.tool_json.insert(i, String::new());
                (i, Kind::Tool)
            }
            _ => return Ok(()),
        };
        self.blocks.insert(index, entry);
        Ok(())
    }

    fn block_delta(&mut self, e: &mut Emitter, event: &Value) {
        let Some(&(i, kind)) = n(event, "index").and_then(|x| self.blocks.get(&x)) else {
            return;
        };
        let delta = &event["delta"];
        let s = |k: &str| delta.get(k).and_then(Value::as_str).unwrap_or("");
        match (
            delta.get("type").and_then(Value::as_str).unwrap_or(""),
            kind,
        ) {
            ("text_delta", Kind::Text) => e.text(i, s("text")),
            ("thinking_delta", Kind::Thinking) => e.thinking(i, s("thinking")),
            ("input_json_delta", Kind::Tool) => {
                let part = s("partial_json");
                e.tool_args(i, part);
                if let Some(buf) = self.tool_json.get_mut(&i) {
                    buf.push_str(part);
                }
            }
            ("signature_delta", Kind::Thinking) => e.thinking_signature(i, s("signature"), true),
            _ => {}
        }
    }

    fn block_stop(&mut self, e: &mut Emitter, event: &Value) {
        let Some((i, kind)) = n(event, "index").and_then(|x| self.blocks.remove(&x)) else {
            return;
        };
        if kind == Kind::Tool
            && let Some(raw) = self.tool_json.remove(&i)
            && !raw.trim().is_empty()
            && serde_json::from_str::<Value>(&raw).is_err()
            && let Ok(v @ Value::Object(_)) = parse_json_with_repair(&raw)
        {
            e.tool_args_value(i, v);
        }
    }

    fn message_delta(&mut self, e: &mut Emitter, event: &Value) -> Result<(), String> {
        if let Some(t) = event.get("input_transformations").and_then(Value::as_array) {
            self.transformations = Some(t.clone());
        }
        let delta = &event["delta"];
        if let Some(reason) = delta.get("stop_reason").and_then(Value::as_str) {
            self.raw_stop = Some(reason.to_owned());
            let (stop, message) = map_stop_reason(reason, delta.get("stop_details"))?;
            self.stop = Some(stop);
            if message.is_some() {
                self.error_message = message;
            }
        }
        if let Some(usage) = event.get("usage").filter(|u| u.is_object()) {
            if let Some(c) = usage
                .get("cache_creation")
                .and_then(|c| n(c, "ephemeral_1h_input_tokens"))
            {
                e.set_cache_write_1h(c);
            }
            let u = e.usage_mut();
            if let Some(v) = n(usage, "input_tokens") {
                u.input = v;
            }
            if let Some(v) = usage
                .get("output_tokens_details")
                .and_then(|d| n(d, "thinking_tokens"))
            {
                u.reasoning = Some(v);
            }
            if let Some(v) = n(usage, "output_tokens") {
                u.output = v;
            }
            if let Some(v) = n(usage, "cache_read_input_tokens") {
                u.cache_read = v;
            }
            if let Some(v) = n(usage, "cache_creation_input_tokens") {
                u.cache_write = v;
            }
        }
        let u = e.usage_mut();
        u.total_tokens = u.input + u.output + u.cache_read + u.cache_write;
        Ok(())
    }
}

async fn run(cache: &federation::Cache, ctx: StreamCtx<'_>) -> Message {
    let req = ctx.req;
    let mut e = ctx.emitter();
    let c = compat(&req.model);
    let mut auth = match resolve_auth(req) {
        Ok(a) => a,
        Err(err) => return ctx.failed(e, err),
    };
    let federation = if auth == Auth::Federation {
        match cache
            .get(
                federation::Config::from_request(req).expect("resolved federation"),
                &ctx,
            )
            .await
        {
            Ok(token) => {
                auth = Auth::Bearer(token.value.clone());
                Some(token)
            }
            Err(error) => return ctx.failed(e, error),
        }
    } else {
        None
    };
    let thinking = plan_thinking(req, &c);
    if c.supports_mid_convo_effort {
        let level = thinking.effort.clone().unwrap_or_else(|| "high".into());
        e.message_extra("providerThinkingLevel", Value::String(level));
    }
    let (body, mut betas) = match build_params(req, &c, &thinking) {
        Ok(p) => p,
        Err(err) => return ctx.failed(e, err),
    };
    if federation.is_some()
        && !betas.iter().any(|b| {
            b.split(',')
                .any(|part| part.trim() == federation::OAUTH_BETA)
        })
    {
        betas.push(federation::OAUTH_BETA.into());
    }
    let headers = match build_headers(req, &c, &auth, &betas).to_header_map() {
        Ok(h) => h,
        Err(err) => return ctx.failed(e, err),
    };
    let resp = match post_json_observing(&ctx, &request_url(req), headers, &body, |status| {
        if let Some(token) = &federation {
            token.observe(status);
        }
    })
    .await
    {
        Ok(r) => r,
        Err(err) => return ctx.failed(e, err),
    };
    let mut state = AnthropicStream::default();
    let read = http::read_sse(
        resp,
        idle_timeout(req, http::DEFAULT_IDLE_TIMEOUT),
        &ctx.cancel,
        |ev| state.on_event(&mut e, req, ev),
    )
    .await;
    if let Some(m) = &state.response_model {
        e.message_extra("responseModel", Value::String(m.clone()));
    }
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
    if let Some(err) = state.fatal {
        return ctx.failed(e, err);
    }
    if state.saw_start && !state.saw_stop {
        return ctx.failed(e, "Anthropic stream ended before message_stop");
    }
    let stop = match state.stop {
        None => return ctx.failed(e, "Anthropic stream ended without a stop reason"),
        Some(StopReason::Error | StopReason::Aborted) => {
            let text = state
                .error_message
                .unwrap_or_else(|| "An unknown error occurred".into());
            return ctx.failed(e, text);
        }
        Some(s) => s,
    };
    if let Some(t) = state.transformations.filter(|t| !t.is_empty()) {
        let items: Vec<Value> = t
            .iter()
            .map(|x| {
                let mut o = Map::new();
                for k in ["type", "path", "reason"] {
                    if let Some(v) = x.get(k).filter(|v| !v.is_null()) {
                        o.insert(k.into(), v.clone());
                    }
                }
                Value::Object(o)
            })
            .collect();
        e.message_extra(
            "diagnostics",
            json!([{"type": "anthropic_input_transformations", "timestamp": crate::util::now_ms(),
                "details": {"transformations": items}}]),
        );
    }
    match state.fallback_cost {
        Some(cost) => {
            let mut priced = req.clone();
            priced.model.cost = cost;
            e.finish(&priced, stop, None)
        }
        None => e.finish(req, stop, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::api::google_shared::tests::{assistant, model, request, tool_result};
    use crate::provider::mock::{MockResponse, MockServer};
    use crate::provider::{StreamEvent, ToolSpec};
    use pipkin_engine_protocol::Delta;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    pub(super) fn claude(provider: &str, compat: Value) -> Model {
        model(json!({
            "id": "claude-test", "name": "Claude Test", "api": "anthropic-messages",
            "provider": provider, "baseUrl": "https://api.anthropic.com", "reasoning": true,
            "input": ["text", "image"], "contextWindow": 200000, "maxTokens": 64000,
            "cost": {"input": 3.0, "output": 15.0, "cacheRead": 0.3, "cacheWrite": 3.75},
            "compat": compat
        }))
    }

    #[test]
    fn system_updates_and_native_tool_changes_follow_completed_tool_results() {
        let mut req = request(claude(
            "anthropic",
            json!({"supportsMidConvoSystemMessages":true,"supportsMidConvoToolChanges":true}),
        ));
        req.system.clear();
        req.messages = serde_json::from_value(json!([
            {"role":"system","content":"","sections":{"p":"base"},"toolsAdded":[
                {"name":"read","description":"old","parameters":{"type":"object"}},
                {"name":"edit","description":"edit","parameters":{"type":"object"}}],"timestamp":1},
            {"role":"user","content":[{"type":"text","text":"work"}],"timestamp":2},
            {"role":"assistant","provider":"anthropic","api":"anthropic-messages","model":"claude-test","content":[{"type":"toolCall","id":"c","name":"read","arguments":{}}],"stopReason":"toolUse","timestamp":3},
            {"role":"system","content":"","sections":{"p":"updated"},"toolsRemoved":[{"name":"read"},{"name":"edit"}],"toolsAdded":[{"name":"read","description":"new","parameters":{"type":"object"}}],"timestamp":4},
            {"role":"toolResult","toolCallId":"c","toolName":"read","content":[{"type":"text","text":"done"}],"timestamp":5}
        ])).unwrap();
        let c = compat(&req.model);
        let (body, _) = build_params(&req, &c, &plan_thinking(&req, &c)).unwrap();
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(
            messages
                .iter()
                .map(|m| m["role"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["user", "assistant", "user", "system"]
        );
        assert_eq!(messages[2]["content"][0]["type"], "tool_result");
        let update = &messages[3]["content"];
        assert_eq!(
            update[0]["text"],
            "Updated system prompt section \"p\":\n\nupdated"
        );
        assert_eq!(update[1]["type"], "tool_removal");
        assert_eq!(update[1]["tool"]["name"], "edit");
        assert_eq!(update[2]["type"], "tool_addition");
        assert_eq!(update[2]["tool"]["definition"]["description"], "new");
        assert_eq!(
            body["tools"][0]["description"], "old",
            "initial declarations stay at the top"
        );
        req.model
            .compat
            .insert("supportsMidConvoSystemMessages".into(), json!(false));
        let c = compat(&req.model);
        let (body, _) = build_params(&req, &c, &plan_thinking(&req, &c)).unwrap();
        assert_eq!(body["system"][0]["text"], "updated");
        assert_eq!(body["tools"].as_array().unwrap().len(), 1);
        assert_eq!(body["tools"][0]["description"], "new");
        assert_eq!(body["messages"].as_array().unwrap().len(), 3);
    }

    fn read_tool() -> ToolSpec {
        ToolSpec {
            constrained_sampling: None,
            name: "read".into(),
            description: "Read a file".into(),
            parameters: json!({"type": "object", "properties": {"path": {"type": "string"}},
                "required": ["path"]}),
        }
    }

    async fn run_with(
        req: &ProviderRequest,
        cancel: CancellationToken,
    ) -> (Message, Vec<StreamEvent>) {
        let http = crate::provider::http::client();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let ctx = StreamCtx {
            req,
            http: &http,
            events: Some(tx.into()),
            cancel,
        };
        let msg = adapter().stream(ctx).await;
        let mut events = vec![];
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
        (msg, events)
    }

    async fn run_req(req: &ProviderRequest) -> Message {
        run_with(req, CancellationToken::new()).await.0
    }

    fn sse(events: &[Value]) -> String {
        events
            .iter()
            .map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap()))
            .collect()
    }

    fn fragmented(body: &str, size: usize) -> MockResponse {
        let mut r = MockResponse::sse(vec![]);
        r.chunks = body.as_bytes().chunks(size).map(<[u8]>::to_vec).collect();
        r
    }

    pub(super) fn simple_reply(text: &str) -> String {
        sse(&[
            json!({"type": "message_start", "message": {"id": "msg_x", "model": "claude-test",
                "usage": {"input_tokens": 5, "output_tokens": 1}}}),
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}}),
            json!({"type": "content_block_stop", "index": 0}),
            json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 3}}),
            json!({"type": "message_stop"}),
        ])
    }

    pub(super) fn error_text(m: &Message) -> String {
        match m {
            Message::Assistant {
                stop_reason: StopReason::Error,
                error_message: Some(e),
                ..
            } => e.clone(),
            other => panic!("expected an error message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn claude_subscription_uses_bearer_identity_betas_and_separate_cached_system() {
        let server = MockServer::start(|_, _| MockResponse::sse(vec![simple_reply("hi")])).await;
        let mut req = request(claude("anthropic", json!({})));
        req.base_url = Some(server.url.clone());
        req.credential = Credential::OAuth {
            access: "subscription-token".into(),
            metadata: Map::new(),
        };
        req.system = "Pipkin project instructions".into();
        req.tools = vec![read_tool()];
        req.cache_retention = CacheRetention::Long;
        let (m, _) = run_with(&req, CancellationToken::new()).await;
        assert!(matches!(
            m,
            Message::Assistant {
                stop_reason: StopReason::Stop,
                ..
            }
        ));
        let requests = server.requests();
        let sent = &requests[0];
        assert_eq!(
            sent.header("authorization"),
            Some("Bearer subscription-token")
        );
        assert!(sent.header("x-api-key").is_none());
        assert_eq!(sent.header("user-agent"), Some("claude-cli/2.1.280"));
        assert_eq!(sent.header("x-app"), Some("cli"));
        assert!(
            sent.header("anthropic-beta")
                .unwrap()
                .contains("oauth-2025-04-20")
        );
        assert!(
            sent.header("anthropic-beta")
                .unwrap()
                .contains("claude-code-20250219")
        );
        let body: Value = serde_json::from_slice(&sent.body).unwrap();
        assert_eq!(body["system"][0]["text"], SUBSCRIPTION_SYSTEM);
        assert_eq!(body["system"][1]["text"], req.system);
        assert_eq!(body["system"][0]["cache_control"]["ttl"], "1h");
        assert_eq!(body["system"][1]["cache_control"]["ttl"], "1h");
        assert_eq!(body["tools"][0]["name"], "Read");
    }

    #[tokio::test]
    async fn subscription_tool_names_round_trip_with_history_and_cache_usage() {
        let reply = sse(&[
            json!({"type":"message_start","message":{"usage":{"input_tokens":7,"output_tokens":1,"cache_read_input_tokens":20,"cache_creation_input_tokens":8,"cache_creation":{"ephemeral_1h_input_tokens":3}}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call-new","name":"Read","input":{}}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"file.rs\"}"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":4}}),
            json!({"type":"message_stop"}),
        ]);
        let server = MockServer::start(move |_, _| fragmented(&reply, 3)).await;
        let mut req = request(claude("anthropic", json!({})));
        req.base_url = Some(server.url.clone());
        // Pi also detects subscription tokens stored as legacy API-key entries.
        req.credential = Credential::ApiKey("sk-ant-oat-legacy".into());
        req.tools = vec![read_tool()];
        req.tools.push(ToolSpec {
            constrained_sampling: None,
            name: "myCustomTool".into(),
            description: "Custom".into(),
            parameters: json!({"type":"object"}),
        });
        req.messages = vec![
            Message::user_text("inspect", 1),
            assistant(
                "anthropic",
                "anthropic-messages",
                "claude-test",
                vec![Block::ToolCall {
                    id: "old".into(),
                    name: "read".into(),
                    arguments: json!({"path":"old.rs"}),
                    extra: Map::new(),
                }],
            ),
            tool_result("old", "read", vec![Block::text("old contents")], false),
        ];
        let (message, _) = run_with(&req, CancellationToken::new()).await;
        let Message::Assistant {
            content,
            stop_reason,
            usage,
            ..
        } = message
        else {
            panic!()
        };
        assert_eq!(stop_reason, StopReason::ToolUse);
        assert!(
            matches!(&content[0],Block::ToolCall { name,arguments,.. } if name=="read" && arguments["path"]=="file.rs")
        );
        assert_eq!(
            (
                usage.input,
                usage.output,
                usage.cache_read,
                usage.cache_write
            ),
            (7, 4, 20, 8)
        );
        let requests = server.requests();
        let sent = &requests[0];
        assert_eq!(
            sent.header("authorization"),
            Some("Bearer sk-ant-oat-legacy")
        );
        assert!(sent.header("x-api-key").is_none());
        let body = sent.json();
        assert_eq!(body["messages"][1]["content"][0]["name"], "Read");
        assert_eq!(body["tools"][1]["name"], "myCustomTool");
        assert_eq!(
            req.tools[0].name, "read",
            "wire conversion must not mutate local tools"
        );
        assert_eq!(wire_tool_name(&req, "gReP"), "Grep");
        assert_eq!(local_tool_name(&req, "UnknownTool"), "UnknownTool");
    }

    #[tokio::test]
    async fn subscription_honors_header_overrides_and_cancellation_without_api_fallback() {
        let server =
            MockServer::start(|_, _| MockResponse::json(401, &json!({"error":"expired"}))).await;
        let mut req = request(claude("anthropic", json!({})));
        req.base_url = Some(server.url.clone());
        req.credential = Credential::OAuth {
            access: "expired-subscription".into(),
            metadata: Map::new(),
        };
        req.headers.insert("anthropic-beta".into(), None);
        req.headers
            .insert("user-agent".into(), Some("configured-agent".into()));
        req.options.insert("maxRetries".into(), json!(0));
        let message = run_req(&req).await;
        assert!(error_text(&message).contains("401"));
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].header("user-agent"), Some("configured-agent"));
        assert!(requests[0].header("anthropic-beta").is_none());
        assert!(requests[0].header("x-api-key").is_none());
        let cancel = CancellationToken::new();
        cancel.cancel();
        let (message, _) = run_with(&req, cancel).await;
        assert!(matches!(
            message,
            Message::Assistant {
                stop_reason: StopReason::Aborted,
                ..
            }
        ));
        assert_eq!(server.requests().len(), 1);
    }

    #[tokio::test]
    async fn golden_request_with_budget_thinking_caching_images_and_merged_tool_results() {
        let server = MockServer::start(|_, _| MockResponse::sse(vec![simple_reply("done")])).await;
        let mut req = request(claude("anthropic", json!({})));
        req.base_url = Some(format!("{}/", server.url));
        req.credential = Credential::ApiKey("sk-ant-api-test".into());
        req.cache_retention = CacheRetention::Long;
        req.thinking = ThinkingLevel::Medium;
        req.max_tokens = Some(8000);
        req.tools = vec![read_tool()];
        req.options.insert("temperature".into(), json!(0.2));
        req.options.insert("toolChoice".into(), json!("auto"));
        req.options
            .insert("metadata".into(), json!({"user_id": "u-1"}));
        req.headers.insert("x-config".into(), Some("c".into()));
        let same = |content| assistant("anthropic", "anthropic-messages", "claude-test", content);
        req.messages = vec![
            Message::User {
                content: vec![
                    Block::text("look"),
                    Block::text("  "),
                    Block::image("image/png", "iVBO"),
                ],
                timestamp: 1,
                extra: Map::new(),
            },
            same(vec![
                Block::Thinking {
                    thinking: "plan".into(),
                    signature: Some("sig1".into()),
                    redacted: false,
                    extra: Map::new(),
                },
                Block::Thinking {
                    thinking: "[Reasoning redacted]".into(),
                    signature: Some("enc".into()),
                    redacted: true,
                    extra: Map::new(),
                },
                Block::Thinking {
                    thinking: "unsigned".into(),
                    signature: None,
                    redacted: false,
                    extra: Map::new(),
                },
                Block::text("ok"),
                Block::tool_call("toolu_1", "read", json!({"path": "a"})),
                Block::tool_call("toolu_2", "read", json!({"path": "b.png"})),
            ]),
            tool_result(
                "toolu_1",
                "read",
                vec![Block::text("file a"), Block::text("more")],
                false,
            ),
            tool_result(
                "toolu_2",
                "read",
                vec![Block::image("image/png", "PNG")],
                true,
            ),
        ];
        let m = run_req(&req).await;
        assert_eq!(m.text(), "done");

        let r = &server.requests()[0];
        assert_eq!(r.path, "/v1/messages?beta=true");
        assert_eq!(r.header("x-api-key"), Some("sk-ant-api-test"));
        assert_eq!(r.header("authorization"), None);
        assert_eq!(r.header("anthropic-version"), Some("2023-06-01"));
        assert_eq!(r.header("anthropic-beta"), Some(INTERLEAVED_THINKING_BETA));
        assert_eq!(r.header("accept"), Some("application/json"));
        assert_eq!(
            r.header("anthropic-dangerous-direct-browser-access"),
            Some("true")
        );
        assert_eq!(r.header("x-config"), Some("c"));
        assert_eq!(r.header("x-session-affinity"), None);
        let cc = json!({"type": "ephemeral", "ttl": "1h"});
        assert_eq!(
            r.json(),
            json!({
                "model": "claude-test",
                "max_tokens": 16192,
                "stream": true,
                "system": [{"type": "text", "text": "You are helpful.", "cache_control": cc}],
                "messages": [
                    {"role": "user", "content": [{"type": "text", "text": "look"},
                        {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBO"}}]},
                    {"role": "assistant", "content": [
                        {"type": "thinking", "thinking": "plan", "signature": "sig1"},
                        {"type": "redacted_thinking", "data": "enc"},
                        {"type": "text", "text": "unsigned"},
                        {"type": "text", "text": "ok"},
                        {"type": "tool_use", "id": "toolu_1", "name": "read", "input": {"path": "a"}},
                        {"type": "tool_use", "id": "toolu_2", "name": "read", "input": {"path": "b.png"}}]},
                    {"role": "user", "content": [
                        {"type": "tool_result", "tool_use_id": "toolu_1", "content": "file a\nmore", "is_error": false},
                        {"type": "tool_result", "tool_use_id": "toolu_2", "is_error": true, "content": [
                            {"type": "text", "text": "(see attached image)"},
                            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "PNG"}}],
                         "cache_control": cc}]}
                ],
                "tools": [{"name": "read", "description": "Read a file", "eager_input_streaming": true,
                    "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]},
                    "cache_control": cc}],
                "thinking": {"type": "enabled", "budget_tokens": 8192, "display": "summarized"},
                "metadata": {"user_id": "u-1"},
                "tool_choice": {"type": "auto"}
            })
        );
    }

    #[tokio::test]
    async fn fragmented_stream_with_thinking_signatures_redaction_tools_and_cache_usage() {
        let body = sse(&[
            json!({"type": "message_start", "message": {"id": "msg_1", "model": "claude-test",
                "usage": {"input_tokens": 10, "output_tokens": 1, "cache_read_input_tokens": 100,
                    "cache_creation_input_tokens": 50, "cache_creation": {"ephemeral_1h_input_tokens": 20}}}}),
            json!({"type": "ping"}),
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "Thinking \u{e9}\u{1f600}"}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "sig-a"}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "sig-b"}}),
            json!({"type": "content_block_stop", "index": 0}),
            json!({"type": "content_block_start", "index": 1, "content_block": {"type": "redacted_thinking", "data": "enc-data"}}),
            json!({"type": "content_block_stop", "index": 1}),
            json!({"type": "content_block_start", "index": 2, "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_delta", "index": 2, "delta": {"type": "text_delta", "text": "Hello "}}),
            json!({"type": "content_block_delta", "index": 2, "delta": {"type": "text_delta", "text": "w\u{f6}rld"}}),
            json!({"type": "content_block_stop", "index": 2}),
            json!({"type": "content_block_start", "index": 3, "content_block": {"type": "tool_use", "id": "toolu_9", "name": "read", "input": {}}}),
            json!({"type": "content_block_delta", "index": 3, "delta": {"type": "input_json_delta", "partial_json": "{\"path\": \"a"}}),
            json!({"type": "content_block_delta", "index": 3, "delta": {"type": "input_json_delta", "partial_json": "\tb.txt\"}"}}),
            json!({"type": "content_block_stop", "index": 3}),
            json!({"type": "message_delta", "delta": {"stop_reason": "tool_use", "stop_sequence": null},
                "usage": {"output_tokens": 42, "input_tokens": null, "output_tokens_details": {"thinking_tokens": 11}}}),
            json!({"type": "message_stop"}),
        ]);
        let server = MockServer::start(move |_, _| fragmented(&body, 5)).await;
        let mut req = request(claude("anthropic", json!({})));
        req.base_url = Some(server.url.clone());
        req.messages = vec![Message::user_text("go", 1)];
        let (m, events) = run_with(&req, CancellationToken::new()).await;
        let Message::Assistant {
            content,
            usage,
            stop_reason,
            extra,
            error_message,
            ..
        } = &m
        else {
            panic!()
        };
        assert_eq!(*stop_reason, StopReason::ToolUse, "{error_message:?}");
        assert_eq!(
            content,
            &vec![
                Block::Thinking {
                    thinking: "Thinking \u{e9}\u{1f600}".into(),
                    signature: Some("sig-asig-b".into()),
                    redacted: false,
                    extra: Map::new()
                },
                Block::Thinking {
                    thinking: "[Reasoning redacted]".into(),
                    signature: Some("enc-data".into()),
                    redacted: true,
                    extra: Map::new()
                },
                Block::text("Hello w\u{f6}rld"),
                Block::tool_call("toolu_9", "read", json!({"path": "a\tb.txt"})),
            ]
        );
        assert_eq!(
            (
                usage.input,
                usage.output,
                usage.cache_read,
                usage.cache_write,
                usage.total_tokens
            ),
            (10, 42, 100, 50, 202)
        );
        assert_eq!(usage.reasoning, Some(11));
        assert_eq!(usage.cache_write_1h, Some(20));
        let expected_write = (30.0 * 3.75 + 20.0 * 6.0) / 1e6;
        assert!((usage.cost.cache_write - expected_write).abs() < 1e-12);
        assert!((usage.cost.output - 42.0 * 15.0 / 1e6).abs() < 1e-12);
        assert_eq!(extra["responseId"], "msg_1");
        assert_eq!(extra["rawStopReason"], "tool_use");
        assert!(extra.get("responseModel").is_none());
        let deltas: Vec<_> = events
            .iter()
            .map(|StreamEvent::Delta(d)| d.clone())
            .collect();
        assert!(deltas.contains(&Delta::Thinking {
            block: 0,
            text: "Thinking \u{e9}\u{1f600}".into()
        }));
        assert!(deltas.contains(&Delta::Text {
            block: 2,
            text: "w\u{f6}rld".into()
        }));
        assert!(
            deltas
                .iter()
                .any(|d| matches!(d, Delta::ToolCall { block: 3, .. }))
        );
    }

    #[tokio::test]
    async fn managed_effort_adaptive_thinking_native_tools_and_per_turn_effort() {
        let server = MockServer::start(|_, _| MockResponse::sse(vec![simple_reply("ok")])).await;
        let mut m = claude(
            "anthropic",
            json!({"supportsMidConvoEffort": true, "supportsMidConvoSystemMessages": true,
                "supportsMidConvoToolChanges": true, "forceAdaptiveThinking": true,
                "supportsTemperature": false}),
        );
        m.thinking_level_map = serde_json::from_value(json!({"off": null, "minimal": null,
            "low": "low", "medium": "medium", "high": "high", "xhigh": "xhigh", "max": "max"}))
        .unwrap();
        let mut req = request(m);
        req.base_url = Some(server.url.clone());
        req.thinking = ThinkingLevel::Xhigh;
        req.cache_retention = CacheRetention::Short;
        req.tools = vec![read_tool()];
        req.options.insert("temperature".into(), json!(0.7));
        let mut earlier = assistant(
            "anthropic",
            "anthropic-messages",
            "claude-older",
            vec![Block::text("before")],
        );
        if let Message::Assistant {
            extra, stop_reason, ..
        } = &mut earlier
        {
            extra.insert("providerThinkingLevel".into(), json!("low"));
            *stop_reason = StopReason::Stop;
        }
        req.messages = vec![
            Message::user_text("q1", 1),
            earlier,
            Message::user_text("q2", 3),
        ];
        let msg = run_req(&req).await;
        let Message::Assistant { extra, .. } = &msg else {
            panic!()
        };
        assert_eq!(extra["providerThinkingLevel"], "xhigh");

        let r = &server.requests()[0];
        assert_eq!(
            r.header("anthropic-beta"),
            Some(
                "mid-conversation-output-config-2026-07-01,thinking-binding-controls-2026-08-01,inline-tools-2026-09-15"
            )
        );
        let body = r.json();
        let cc = json!({"type": "ephemeral"});
        assert_eq!(
            body["messages"],
            json!([
                {"role": "user", "content": [{"type": "text", "text": "q1"}]},
                {"role": "system", "content": [], "output_config": {"effort": "low"}},
                {"role": "assistant", "content": [{"type": "text", "text": "before"}]},
                {"role": "user", "content": [{"type": "text", "text": "q2", "cache_control": cc}]},
                {"role": "system", "content": [], "output_config": {"effort": "xhigh"}}
            ])
        );
        assert_eq!(
            body["thinking"],
            json!({"type": "adaptive", "display": "summarized",
                "block_binding": {"prefix_mismatch_behavior": "drop_block"}})
        );
        assert_eq!(body["output_config"], json!({"effort": "high"}));
        assert!(body.get("temperature").is_none());
        assert_eq!(body["max_tokens"], 64000);
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["cache_control"], cc);
        assert_eq!(tools[1]["name"], "__pi_deferred_placeholder__");
        assert_eq!(tools[1]["defer_loading"], true);
    }

    #[tokio::test]
    async fn adaptive_effort_mapping_and_disabled_thinking() {
        let server = MockServer::start(|_, _| MockResponse::sse(vec![simple_reply("ok")])).await;
        let mut req = request(claude("anthropic", json!({"forceAdaptiveThinking": true})));
        req.base_url = Some(server.url.clone());
        req.messages = vec![Message::user_text("q", 1)];
        req.thinking = ThinkingLevel::Minimal;
        run_req(&req).await;
        req.thinking = ThinkingLevel::Off;
        req.options.insert("temperature".into(), json!(0.3));
        run_req(&req).await;
        let all = server.requests();
        let first = all[0].json();
        assert_eq!(
            first["thinking"],
            json!({"type": "adaptive", "display": "summarized"})
        );
        assert_eq!(first["output_config"], json!({"effort": "low"}));
        assert_eq!(all[0].header("anthropic-beta"), None);
        let second = all[1].json();
        assert_eq!(second["thinking"], json!({"type": "disabled"}));
        assert_eq!(second["temperature"], json!(0.3));
        assert!(second.get("output_config").is_none());
    }

    #[tokio::test]
    async fn copilot_bearer_auth_token_and_header_owned_auth() {
        let server = MockServer::start(|_, _| MockResponse::sse(vec![simple_reply("ok")])).await;
        let mut copilot = claude("github-copilot", json!({"forceAdaptiveThinking": true}));
        copilot
            .headers
            .insert("Copilot-Integration-Id".into(), "vscode-chat".into());
        let mut req = request(copilot);
        req.base_url = Some(server.url.clone());
        req.credential = Credential::ApiKey("copilot-token".into());
        req.messages = vec![
            Message::user_text("q", 1),
            assistant(
                "github-copilot",
                "anthropic-messages",
                "claude-test",
                vec![Block::tool_call("toolu_1", "shot", json!({}))],
            ),
            tool_result(
                "toolu_1",
                "shot",
                vec![Block::image("image/png", "PNG")],
                false,
            ),
        ];
        run_req(&req).await;
        let r = &server.requests()[0];
        assert_eq!(r.header("authorization"), Some("Bearer copilot-token"));
        assert_eq!(r.header("x-api-key"), None);
        assert_eq!(r.header("x-initiator"), Some("agent"));
        assert_eq!(r.header("openai-intent"), Some("conversation-edits"));
        assert_eq!(r.header("copilot-vision-request"), Some("true"));
        assert_eq!(r.header("copilot-integration-id"), Some("vscode-chat"));

        // ANTHROPIC_AUTH_TOKEN-style bearer for the anthropic provider.
        let mut req = request(claude("anthropic", json!({})));
        req.base_url = Some(server.url.clone());
        req.credential = Credential::Bearer("auth-token".into());
        req.messages = vec![Message::user_text("q", 1)];
        run_req(&req).await;
        let r = &server.requests()[1];
        assert_eq!(r.header("authorization"), Some("Bearer auth-token"));
        assert_eq!(r.header("x-api-key"), None);
        assert_eq!(r.header("x-initiator"), None);

        // Cloudflare AI Gateway: configured headers carry auth; placeholders come from env.
        let mut gw = claude("cloudflare-ai-gateway", json!({}));
        gw.base_url = format!(
            "{}/v1/{{CLOUDFLARE_ACCOUNT_ID}}/{{CLOUDFLARE_GATEWAY_ID}}/anthropic",
            server.url
        );
        let mut req = request(gw);
        req.credential = Credential::None;
        req.headers
            .insert("cf-aig-authorization".into(), Some("Bearer cf".into()));
        req.env
            .insert("CLOUDFLARE_ACCOUNT_ID".into(), "acct".into());
        req.env.insert("CLOUDFLARE_GATEWAY_ID".into(), "gw".into());
        req.messages = vec![Message::user_text("q", 1)];
        assert_eq!(run_req(&req).await.text(), "ok");
        let r = &server.requests()[2];
        assert_eq!(r.path, "/v1/acct/gw/anthropic/v1/messages?beta=true");
        assert_eq!(r.header("cf-aig-authorization"), Some("Bearer cf"));
        assert_eq!(r.header("x-api-key"), None);
        assert_eq!(r.header("authorization"), None);

        // No credential and no auth header: refused before sending.
        req.headers.clear();
        req.env
            .insert("ANTHROPIC_FEDERATION_RULE_ID".into(), String::new());
        let m = run_req(&req).await;
        assert_eq!(
            error_text(&m),
            "No API key for provider: cloudflare-ai-gateway"
        );
        assert_eq!(server.requests().len(), 3);
    }

    #[tokio::test]
    async fn openrouter_affinity_strict_tools_and_no_cache() {
        let server = MockServer::start(|_, _| MockResponse::sse(vec![simple_reply("ok")])).await;
        let mut m = claude("openrouter", json!({"supportsStrictTools": true}));
        m.reasoning = false;
        let mut req = request(m);
        req.base_url = Some(server.url.clone());
        req.messages = vec![Message::user_text("q", 1)];
        let mut ranged = read_tool();
        ranged.name = "count".into();
        ranged.parameters =
            json!({"type": "object", "properties": {"n": {"type": "integer", "minimum": 1}}});
        req.tools = vec![read_tool(), ranged];
        req.options.insert(
            "toolConstrainedSampling".into(),
            json!({"read": {"type": "json_schema", "strict": "prefer"},
                   "count": {"type": "json_schema", "strict": "prefer"}}),
        );
        run_req(&req).await;
        let r = &server.requests()[0];
        assert_eq!(r.header("x-session-id"), Some("sess-1"));
        let body = r.json();
        assert_eq!(body["tools"][0]["strict"], true);
        assert_eq!(
            body["tools"][0]["input_schema"],
            json!({"type": "object", "properties": {"path": {"type": "string"}},
                "required": ["path"], "additionalProperties": false})
        );
        assert!(
            body["tools"][1].get("strict").is_none(),
            "minimum is unsupported in strict mode"
        );
        assert!(body.get("thinking").is_none());

        // "require" with an unsupported schema fails before sending.
        req.tools[1].constrained_sampling = Some(json!({"type":"json_schema","strict":"require"}));
        let m = run_req(&req).await;
        assert!(
            error_text(&m).starts_with("Tool \"count\" requires JSON-schema constrained sampling")
        );
        assert_eq!(server.requests().len(), 1);

        req.options.remove("toolConstrainedSampling");
        req.tools[1].constrained_sampling = None;
        req.cache_retention = CacheRetention::None;
        run_req(&req).await;
        let r = &server.requests()[1];
        assert_eq!(r.header("x-session-id"), None);
        assert!(!r.json().to_string().contains("cache_control"));
    }

    #[tokio::test]
    async fn fallback_models_are_requested_and_priced() {
        let body = sse(&[
            json!({"type": "message_start", "message": {"id": "msg_f", "model": "claude-fallback",
                "usage": {"input_tokens": 1000, "output_tokens": 0}}}),
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "fallback"}}),
            json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": "hi"}}),
            json!({"type": "content_block_stop", "index": 1}),
            json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 1000}}),
            json!({"type": "message_stop"}),
        ]);
        let server = MockServer::start(move |_, _| MockResponse::sse(vec![body.clone()])).await;
        let mut req = request(claude(
            "anthropic",
            json!({"allowedFallbackModels": [{"provider": "anthropic", "model": "claude-fallback",
                "cost": {"input": 1.0, "output": 2.0, "cacheRead": 0.0, "cacheWrite": 0.0}}]}),
        ));
        req.base_url = Some(server.url.clone());
        req.messages = vec![Message::user_text("q", 1)];
        let m = run_req(&req).await;
        let r = &server.requests()[0];
        assert_eq!(r.json()["fallbacks"], json!([{"model": "claude-fallback"}]));
        assert_eq!(r.header("anthropic-beta"), Some(SERVER_SIDE_FALLBACK_BETA));
        let Message::Assistant { usage, extra, .. } = &m else {
            panic!()
        };
        assert_eq!(m.text(), "hi");
        assert_eq!(extra["responseModel"], "claude-fallback");
        assert!((usage.cost.total - (1000.0 * 1.0 + 1000.0 * 2.0) / 1e6).abs() < 1e-12);
    }

    #[tokio::test]
    async fn stream_and_http_errors_keep_reference_texts() {
        let refusal = sse(&[
            json!({"type": "message_start", "message": {"id": "m", "model": "claude-test", "usage": {"input_tokens": 1}}}),
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": "I can"}}),
            json!({"type": "message_delta", "delta": {"stop_reason": "refusal",
                "stop_details": {"type": "refusal", "explanation": "Declined for safety"}}}),
            json!({"type": "message_stop"}),
        ]);
        let truncated = sse(&[
            json!({"type": "message_start", "message": {"id": "m", "model": "claude-test", "usage": {}}}),
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": "par"}}),
        ]);
        let unknown = sse(&[
            json!({"type": "message_start", "message": {"id": "m", "model": "claude-test", "usage": {}}}),
            json!({"type": "message_delta", "delta": {"stop_reason": "brand_new"}}),
            json!({"type": "message_stop"}),
        ]);
        let sensitive = sse(&[
            json!({"type": "message_start", "message": {"id": "m", "model": "claude-test", "usage": {}}}),
            json!({"type": "message_delta", "delta": {"stop_reason": "sensitive"}}),
            json!({"type": "message_stop"}),
        ]);
        let server = MockServer::start(move |_, n| match n {
            0 => MockResponse::sse(vec![refusal.clone()]),
            1 => MockResponse::sse(vec![
                "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n".into(),
            ]),
            2 => MockResponse::json(529, &json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}})),
            3 => MockResponse::sse(vec![truncated.clone()]),
            4 => MockResponse::sse(vec![unknown.clone()]),
            5 => MockResponse::sse(vec![sensitive.clone()]),
            6 => MockResponse::sse(vec!["event: message_start\ndata: {not json\n\n".into()]),
            _ => MockResponse::json(400, &json!({"type": "error", "error": {"type": "invalid_request_error",
                "message": "prompt is too long: 300000 tokens > 200000 maximum"}})),
        })
        .await;
        let mut req = request(claude("anthropic", json!({})));
        req.base_url = Some(server.url.clone());
        req.messages = vec![Message::user_text("q", 1)];

        let m = run_req(&req).await;
        assert_eq!(error_text(&m), "Declined for safety");
        assert_eq!(m.text(), "I can");
        let m = run_req(&req).await;
        assert!(error_text(&m).contains("overloaded_error"));
        assert!(crate::provider::errors::is_retryable(&m));
        let m = run_req(&req).await;
        assert!(error_text(&m).starts_with("529: {"), "{}", error_text(&m));
        let m = run_req(&req).await;
        assert_eq!(error_text(&m), "Anthropic stream ended before message_stop");
        assert!(crate::provider::errors::is_retryable(&m));
        assert_eq!(m.text(), "par");
        let m = run_req(&req).await;
        assert_eq!(error_text(&m), "Unhandled stop reason: brand_new");
        let m = run_req(&req).await;
        assert_eq!(error_text(&m), "Provider stopped with: sensitive");
        let m = run_req(&req).await;
        assert!(error_text(&m).starts_with("Could not parse Anthropic SSE event message_start:"));
        let m = run_req(&req).await;
        assert!(crate::provider::errors::is_context_overflow(&m, 200000));
    }

    #[tokio::test]
    async fn cancellation_mid_stream_is_aborted_with_partial_content() {
        let server = MockServer::start(|_, _| {
            let mut r = MockResponse::sse(vec![sse(&[
                json!({"type": "message_start", "message": {"id": "m", "model": "claude-test", "usage": {"input_tokens": 3}}}),
                json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
                json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "partial"}}),
            ])]);
            r.hang = true;
            r
        })
        .await;
        let mut req = request(claude("anthropic", json!({})));
        req.base_url = Some(server.url.clone());
        req.messages = vec![Message::user_text("q", 1)];
        let cancel = CancellationToken::new();
        let http = crate::provider::http::client();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let ctx = StreamCtx {
            req: &req,
            http: &http,
            events: Some(tx.into()),
            cancel: cancel.clone(),
        };
        let stopper = async {
            rx.recv().await;
            cancel.cancel();
        };
        let (m, _) = {
            let adapter = adapter();
            tokio::join!(adapter.stream(ctx), stopper)
        };
        let Message::Assistant {
            stop_reason,
            error_message,
            usage,
            ..
        } = &m
        else {
            panic!()
        };
        assert_eq!(*stop_reason, StopReason::Aborted);
        assert_eq!(error_message, &None);
        assert_eq!(usage.input, 3);
        assert_eq!(m.text(), "partial");
    }

    #[tokio::test]
    async fn empty_signatures_and_configured_betas() {
        let server = MockServer::start(|_, _| MockResponse::sse(vec![simple_reply("ok")])).await;
        let mut req = request(claude("anthropic", json!({"allowEmptySignature": true})));
        req.base_url = Some(server.url.clone());
        req.thinking = ThinkingLevel::Low;
        req.headers
            .insert("Anthropic-Beta".into(), Some("a, b,a".into()));
        req.messages = vec![
            Message::user_text("q", 1),
            assistant(
                "anthropic",
                "anthropic-messages",
                "claude-test",
                vec![
                    Block::Thinking {
                        thinking: "t".into(),
                        signature: None,
                        redacted: false,
                        extra: Map::new(),
                    },
                    Block::text("x"),
                ],
            ),
            Message::user_text("q2", 3),
        ];
        run_req(&req).await;
        let r = &server.requests()[0];
        assert_eq!(r.header("anthropic-beta"), Some("a,b"));
        assert_eq!(
            r.json()["messages"][1]["content"][0],
            json!({"type": "thinking", "thinking": "t", "signature": ""})
        );

        req.headers.insert("anthropic-beta".into(), None);
        req.headers.remove("Anthropic-Beta");
        run_req(&req).await;
        assert_eq!(server.requests()[1].header("anthropic-beta"), None);
    }

    #[test]
    fn json_repair_matches_the_reference() {
        assert_eq!(
            crate::provider::api::common::repair_json("{\"a\":\"x\ty\"}"),
            "{\"a\":\"x\\ty\"}"
        );
        assert_eq!(
            crate::provider::api::common::repair_json("{\"a\":\"c:\\d\"}"),
            "{\"a\":\"c:\\\\d\"}"
        );
        assert_eq!(
            crate::provider::api::common::repair_json("{\"a\":\"\\u00e9\"}"),
            "{\"a\":\"\\u00e9\"}"
        );
        assert!(parse_json_with_repair("{\"a\":\"x\u{1}\"}").is_ok());
    }
}
