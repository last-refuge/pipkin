//! Shared pieces for the Google adapters (`google-generative-ai`, `google-vertex`), plus the
//! cross-adapter request helpers they and `anthropic-messages` both need.
//!
//! Reference: `packages/ai/src/api/google-shared.ts`, `api/google-generative-ai.ts` (stream loop),
//! `api/transform-messages.ts`, `api/simple-options.ts`, `api/constrained-sampling.ts`,
//! `utils/estimate.ts`, `utils/provider-retry.ts`.
//!
//! Cross-adapter helpers (`transform_messages`, context estimation, thinking budgets, strict
//! JSON-schema conversion, header merging, request sending) live here because this module is
//! the shared one these adapters own; they have no Google-specific behaviour.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::ControlFlow;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use pipkin_engine_protocol::{Block, Message, StopReason};
use regex::Regex;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Map, Value, json};

use crate::provider::http::{self, ReadError};
use crate::provider::sse::SseEvent;
use crate::provider::{Emitter, Model, ProviderRequest, StreamCtx, ThinkingLevel, ToolSpec};

// ---------------------------------------------------------------------------------------------
// Request options
// ---------------------------------------------------------------------------------------------

/// A provider option from `ProviderRequest::options`.
pub(crate) fn opt<'a>(req: &'a ProviderRequest, key: &str) -> Option<&'a Value> {
    req.options.get(key).filter(|v| !v.is_null())
}

pub(crate) fn opt_str<'a>(req: &'a ProviderRequest, key: &str) -> Option<&'a str> {
    opt(req, key).and_then(Value::as_str)
}

/// SDK-style request retry policy: `maxRetries` (default 0) and `maxRetryDelayMs` (default
/// 60000; 0 disables the cap), as the reference `retryProviderRequest` reads them.
pub(crate) fn retry_policy(req: &ProviderRequest) -> (u32, Duration) {
    let retries = opt(req, "maxRetries")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(u32::MAX as u64) as u32;
    let delay = opt(req, "maxRetryDelayMs")
        .and_then(Value::as_u64)
        .map(Duration::from_millis)
        .unwrap_or(http::MAX_RETRY_DELAY);
    (retries, delay)
}

/// The idle timeout for reading a stream.
pub(crate) fn idle_timeout(req: &ProviderRequest) -> Duration {
    if req.timeout.is_zero() {
        http::DEFAULT_IDLE_TIMEOUT
    } else {
        req.timeout
    }
}

// ---------------------------------------------------------------------------------------------
// Headers
// ---------------------------------------------------------------------------------------------

/// An ordered, case-insensitive header set. Later writes replace earlier ones; a configured
/// `None` deletes (reference `ProviderHeaders` semantics).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Headers(Vec<(String, String)>);

impl Headers {
    pub(crate) fn set(&mut self, name: &str, value: impl Into<String>) {
        self.remove(name);
        self.0.push((name.to_owned(), value.into()));
    }

    /// Set only when no header of that name exists.
    pub(crate) fn set_default(&mut self, name: &str, value: impl Into<String>) {
        if !self.has(name) {
            self.0.push((name.to_owned(), value.into()));
        }
    }

    pub(crate) fn remove(&mut self, name: &str) {
        self.0.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
    }

    pub(crate) fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub(crate) fn has(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    pub(crate) fn merge(&mut self, headers: &BTreeMap<String, String>) {
        for (k, v) in headers {
            self.set(k, v.clone());
        }
    }

    pub(crate) fn merge_optional(&mut self, headers: &BTreeMap<String, Option<String>>) {
        for (k, v) in headers {
            match v {
                Some(v) => self.set(k, v.clone()),
                None => self.remove(k),
            }
        }
    }

    pub(crate) fn to_header_map(&self) -> Result<HeaderMap, String> {
        let mut map = HeaderMap::new();
        for (k, v) in &self.0 {
            let name = HeaderName::from_bytes(k.as_bytes())
                .map_err(|_| format!("Invalid header name: {k}"))?;
            let value =
                HeaderValue::from_str(v).map_err(|_| format!("Invalid value for header {k}"))?;
            map.insert(name, value);
        }
        Ok(map)
    }
}

/// True when a configured header (request or model) already carries this credential.
pub(crate) fn has_configured_header(req: &ProviderRequest, name: &str) -> bool {
    let request = req.headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case(name) && v.as_deref().is_some_and(|v| !v.trim().is_empty())
    });
    let model = req
        .model
        .headers
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case(name) && !v.trim().is_empty());
    request || model
}

/// POST a JSON body with the shared request-level retry policy. A gap of `req.timeout` per
/// attempt before response headers is a timeout.
pub(crate) async fn post_json(
    ctx: &StreamCtx<'_>,
    url: &str,
    headers: &Headers,
    body: &Value,
) -> Result<reqwest::Response, String> {
    let mut headers = headers.clone();
    headers.set_default("content-type", "application/json");
    let map = headers.to_header_map()?;
    let bytes = serde_json::to_vec(body).map_err(|e| e.to_string())?;
    let (retries, max_delay) = retry_policy(ctx.req);
    let send = http::send(
        || ctx.http.post(url).headers(map.clone()).body(bytes.clone()),
        retries,
        max_delay,
        &ctx.cancel,
    );
    if ctx.req.timeout.is_zero() {
        return send.await;
    }
    let budget = ctx.req.timeout.saturating_mul(retries.saturating_add(1))
        + max_delay.saturating_mul(retries);
    match tokio::time::timeout(budget, send).await {
        Ok(r) => r,
        Err(_) => Err(format!(
            "Request timed out: no response headers within {} seconds",
            budget.as_secs()
        )),
    }
}

// ---------------------------------------------------------------------------------------------
// Cross-provider replay normalization (reference `transform-messages.ts`)
// ---------------------------------------------------------------------------------------------

const NON_VISION_USER_IMAGE_PLACEHOLDER: &str = "(image omitted: model does not support images)";
const NON_VISION_TOOL_IMAGE_PLACEHOLDER: &str =
    "(tool image omitted: model does not support images)";

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

/// Is this assistant message from exactly the target provider, API and model?
pub(crate) fn is_same_model(message: &Message, model: &Model) -> bool {
    matches!(message, Message::Assistant { provider, api, model: id, .. }
        if *provider == model.provider && *api == model.api && *id == model.id)
}

/// Normalize history for replay to `model`: downgrade images the model cannot read, turn
/// foreign reasoning into text (or drop it), strip foreign signatures, normalize foreign tool
/// call ids (and their results), skip errored/aborted turns and close orphan tool calls with a
/// synthetic error result.
pub(crate) fn transform_messages(
    messages: &[Message],
    model: &Model,
    normalize_tool_call_id: Option<&dyn Fn(&str) -> String>,
) -> Vec<Message> {
    let mut id_map: HashMap<String, String> = HashMap::new();
    let images = model.supports_images();
    let mut transformed = Vec::with_capacity(messages.len());
    for msg in messages {
        let msg = match msg {
            Message::User {
                content,
                timestamp,
                extra,
            } if !images => Message::User {
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
            } => Message::ToolResult {
                tool_call_id: id_map
                    .get(tool_call_id)
                    .cloned()
                    .unwrap_or_else(|| tool_call_id.clone()),
                tool_name: tool_name.clone(),
                content: if images {
                    content.clone()
                } else {
                    replace_images(content, NON_VISION_TOOL_IMAGE_PLACEHOLDER)
                },
                is_error: *is_error,
                timestamp: *timestamp,
                extra: extra.clone(),
            },
            Message::Assistant {
                content,
                api,
                provider,
                model: id,
                usage,
                stop_reason,
                error_message,
                timestamp,
                extra,
            } => {
                let same = is_same_model(msg, model);
                let mut blocks = Vec::with_capacity(content.len());
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
                                    blocks.push(block.clone());
                                }
                                continue;
                            }
                            if same && signature.as_deref().is_some_and(|s| !s.is_empty()) {
                                blocks.push(block.clone());
                                continue;
                            }
                            if thinking.trim().is_empty() {
                                continue;
                            }
                            if same {
                                blocks.push(block.clone());
                            } else {
                                blocks.push(Block::text(thinking.clone()));
                            }
                        }
                        Block::Text { text, .. } => {
                            if same {
                                blocks.push(block.clone());
                            } else {
                                blocks.push(Block::text(text.clone()));
                            }
                        }
                        Block::ToolCall {
                            id: call_id,
                            name,
                            arguments,
                            extra,
                        } => {
                            let mut extra = extra.clone();
                            let mut call_id = call_id.clone();
                            if !same {
                                extra.remove("thoughtSignature");
                                if let Some(normalize) = normalize_tool_call_id {
                                    let normalized = normalize(&call_id);
                                    if normalized != call_id {
                                        id_map.insert(call_id.clone(), normalized.clone());
                                        call_id = normalized;
                                    }
                                }
                            }
                            blocks.push(Block::ToolCall {
                                id: call_id,
                                name: name.clone(),
                                arguments: arguments.clone(),
                                extra,
                            });
                        }
                        Block::Image { .. } => blocks.push(block.clone()),
                    }
                }
                Message::Assistant {
                    content: blocks,
                    api: api.clone(),
                    provider: provider.clone(),
                    model: id.clone(),
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

    // Second pass: skip incomplete turns, synthesize results for orphan tool calls.
    let mut result = Vec::with_capacity(transformed.len());
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
            Message::Assistant { stop_reason, .. } => {
                close(&mut result, &mut pending, &mut answered);
                if matches!(stop_reason, StopReason::Error | StopReason::Aborted) {
                    continue;
                }
                let calls: Vec<(String, String)> = msg
                    .tool_calls()
                    .into_iter()
                    .map(|(id, name, _)| (id.to_owned(), name.to_owned()))
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

/// Replace characters outside `[a-zA-Z0-9_-]` with `_` and cut to 64 UTF-16 units (the
/// reference regex works per UTF-16 unit, so a non-BMP character becomes two underscores).
pub(crate) fn normalize_tool_call_id(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    for c in id.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            out.push(c);
        } else {
            for _ in 0..c.len_utf16() {
                out.push('_');
            }
        }
    }
    out.truncate(64);
    out
}

// ---------------------------------------------------------------------------------------------
// Context estimation and max-token clamping (reference `utils/estimate.ts`, `simple-options.ts`)
// ---------------------------------------------------------------------------------------------

const CHARS_PER_TOKEN: u64 = 4;
const ESTIMATED_IMAGE_CHARS: u64 = 4800;
const CONTEXT_SAFETY_TOKENS: i64 = 4096;
/// Tokens always left for the answer when a thinking budget shares the response ceiling.
pub(crate) const MIN_ANSWER_TOKENS: u64 = 1024;

fn js_len(text: &str) -> u64 {
    text.encode_utf16().count() as u64
}

fn ceil_tokens(chars: u64) -> u64 {
    chars.div_ceil(CHARS_PER_TOKEN)
}

fn content_chars(content: &[Block]) -> u64 {
    content
        .iter()
        .map(|b| match b {
            Block::Text { text, .. } => js_len(text),
            Block::Image { .. } => ESTIMATED_IMAGE_CHARS,
            Block::Thinking { thinking, .. } => js_len(thinking),
            Block::ToolCall {
                name, arguments, ..
            } => js_len(name) + js_len(&arguments.to_string()),
        })
        .sum()
}

fn message_tokens(message: &Message) -> u64 {
    ceil_tokens(content_chars(message.content()))
}

fn usage_tokens(u: &pipkin_engine_protocol::Usage) -> u64 {
    if u.total_tokens > 0 {
        u.total_tokens
    } else {
        u.input + u.output + u.cache_read + u.cache_write
    }
}

/// Estimated context tokens: the last applicable assistant usage plus estimates of what came
/// after it, or a character estimate of everything (system prompt and tools included).
pub(crate) fn estimate_context_tokens(req: &ProviderRequest) -> u64 {
    let messages = &req.messages;
    let mut latest_prefix = 0i64; // the leading system message has timestamp 0
    let mut last: Option<(usize, u64)> = None;
    for (i, m) in messages.iter().enumerate() {
        if let Message::Assistant {
            usage,
            stop_reason,
            timestamp,
            ..
        } = m
        {
            let tokens = usage_tokens(usage);
            if *timestamp >= latest_prefix
                && !matches!(stop_reason, StopReason::Aborted | StopReason::Error)
                && tokens > 0
            {
                last = Some((i, tokens));
            }
        }
        latest_prefix = latest_prefix.max(m.timestamp());
    }
    if let Some((index, tokens)) = last {
        return tokens
            + messages[index + 1..]
                .iter()
                .map(message_tokens)
                .sum::<u64>();
    }
    let mut tokens = ceil_tokens(js_len(&req.system));
    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| json!({"name": t.name, "description": t.description, "parameters": t.parameters}))
            .collect();
        tokens += ceil_tokens(js_len(&Value::Array(tools).to_string()));
    }
    tokens + messages.iter().map(message_tokens).sum::<u64>()
}

/// Cap an output-token request to what fits in the context window (at least 1).
pub(crate) fn clamp_max_tokens_to_context(req: &ProviderRequest, max_tokens: u64) -> u64 {
    let window = req.model.context_window;
    if window == 0 {
        return max_tokens.max(1);
    }
    let available = window as i64 - estimate_context_tokens(req) as i64 - CONTEXT_SAFETY_TOKENS;
    max_tokens.min(available.max(1) as u64)
}

/// The base output cap: the caller's `max_tokens` or the model's, clamped to the context.
pub(crate) fn base_max_tokens(req: &ProviderRequest) -> u64 {
    clamp_max_tokens_to_context(req, req.max_tokens.unwrap_or(req.model.max_tokens))
}

/// The supported level for this request (`Off` when reasoning is unavailable).
pub(crate) fn effective_thinking(req: &ProviderRequest) -> ThinkingLevel {
    req.model.clamp_thinking(req.thinking)
}

/// `xhigh`/`max` behave as `high` for token budgets.
pub(crate) fn clamp_reasoning(level: ThinkingLevel) -> ThinkingLevel {
    match level {
        ThinkingLevel::Xhigh | ThinkingLevel::Max => ThinkingLevel::High,
        other => other,
    }
}

/// A custom budget from the `thinkingBudgets` option.
pub(crate) fn custom_budget(req: &ProviderRequest, level: ThinkingLevel) -> Option<u64> {
    opt(req, "thinkingBudgets")?.get(level.as_str())?.as_u64()
}

pub(crate) fn thinking_budget_for_level(req: &ProviderRequest, level: ThinkingLevel) -> u64 {
    let level = clamp_reasoning(level);
    custom_budget(req, level).unwrap_or(match level {
        ThinkingLevel::Minimal => 1024,
        ThinkingLevel::Low => 2048,
        ThinkingLevel::Medium => 8192,
        _ => 16384,
    })
}

/// Reference `adjustMaxTokensForThinking`: `(maxTokens, thinkingBudget)`.
pub(crate) fn adjust_max_tokens_for_thinking(
    req: &ProviderRequest,
    base_max_tokens: Option<u64>,
    level: ThinkingLevel,
) -> (u64, u64) {
    let mut budget = thinking_budget_for_level(req, level);
    let model_max = req.model.max_tokens;
    let max_tokens = match base_max_tokens {
        None => model_max,
        Some(base) => base.saturating_add(budget).min(model_max),
    };
    if max_tokens <= budget {
        budget = budget.min(max_tokens.saturating_sub(MIN_ANSWER_TOKENS));
    }
    (max_tokens, budget)
}

// ---------------------------------------------------------------------------------------------
// Strict JSON-schema tools (reference `constrained-sampling.ts`)
// ---------------------------------------------------------------------------------------------

/// Returns true when a provider's strict mode rejects this keyword with this value.
pub(crate) type UnsupportedKeyword = dyn Fn(&str, &Value) -> bool;

const UNSUPPORTED_STRICT_SCHEMA_KEYS: &[&str] = &[
    "$ref",
    "$defs",
    "definitions",
    "allOf",
    "oneOf",
    "patternProperties",
    "dependentSchemas",
    "dependencies",
    "unevaluatedProperties",
    "propertyNames",
    "contains",
    "prefixItems",
    "not",
    "if",
    "then",
    "else",
];

fn is_structured_schema(schema: &Value) -> bool {
    let Some(obj) = schema.as_object() else {
        return false;
    };
    let types: Vec<&str> = match obj.get("type") {
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
        _ => vec![],
    };
    types.contains(&"object")
        || types.contains(&"array")
        || obj.contains_key("properties")
        || obj.contains_key("items")
}

fn schema_allows_null(schema: &Value) -> bool {
    let Some(obj) = schema.as_object() else {
        return false;
    };
    match obj.get("type") {
        Some(Value::String(s)) if s == "null" => return true,
        Some(Value::Array(a)) if a.iter().any(|t| t == "null") => return true,
        _ => {}
    }
    if obj.get("const") == Some(&Value::Null) {
        return true;
    }
    if let Some(Value::Array(e)) = obj.get("enum")
        && e.contains(&Value::Null)
    {
        return true;
    }
    matches!(obj.get("anyOf"), Some(Value::Array(v)) if v.iter().any(schema_allows_null))
}

fn make_node_strict(schema: &mut Value, check: Option<&UnsupportedKeyword>) -> Result<(), String> {
    let Some(obj) = schema.as_object_mut() else {
        return Err("boolean schemas are unsupported".into());
    };
    for key in UNSUPPORTED_STRICT_SCHEMA_KEYS {
        if obj.contains_key(*key) {
            return Err(format!("{key} schemas are unsupported"));
        }
    }
    if let Some(check) = check {
        for (k, v) in obj.iter() {
            if check(k, v) {
                return Err(format!("{k}: {v} is unsupported"));
            }
        }
    }
    if let Some(any_of) = obj.get_mut("anyOf") {
        let Some(variants) = any_of.as_array_mut().filter(|a| !a.is_empty()) else {
            return Err("anyOf must contain at least one schema".into());
        };
        for variant in variants {
            if is_structured_schema(variant) {
                return Err("object and array unions are unsupported".into());
            }
            make_node_strict(variant, check)?;
        }
    }
    if let Some(items) = obj.get_mut("items") {
        if items.is_array() {
            return Err("tuple schemas are unsupported".into());
        }
        make_node_strict(items, check)?;
    }
    let is_object = obj.get("type").and_then(Value::as_str) == Some("object");
    if obj.contains_key("properties") && !is_object {
        return Err("properties require type object".into());
    }
    if !is_object {
        return Ok(());
    }
    if let Some(ap) = obj.get("additionalProperties")
        && *ap != Value::Bool(false)
    {
        return Err("schema-valued or true additionalProperties is unsupported".into());
    }
    if let Some(p) = obj.get("properties")
        && !p.is_object()
    {
        return Err("object properties must be a schema map".into());
    }
    let required: Vec<String> = match obj.get("required") {
        None => vec![],
        Some(Value::Array(a)) if a.iter().all(Value::is_string) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        Some(_) => return Err("object required must be a string array".into()),
    };
    let mut properties = obj
        .remove("properties")
        .and_then(|p| match p {
            Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default();
    if required.iter().any(|k| !properties.contains_key(k)) {
        return Err("required contains an unknown property".into());
    }
    let names: Vec<String> = properties.keys().cloned().collect();
    for (key, property) in properties.iter_mut() {
        make_node_strict(property, check)?;
        if !required.contains(key) && !schema_allows_null(property) {
            *property = json!({"anyOf": [property.take(), {"type": "null"}]});
        }
    }
    obj.insert("properties".into(), Value::Object(properties));
    obj.insert(
        "required".into(),
        Value::Array(names.into_iter().map(Value::String).collect()),
    );
    obj.insert("additionalProperties".into(), Value::Bool(false));
    Ok(())
}

/// Convert a tool schema to the strict subset expected by provider constrained sampling.
pub(crate) fn make_strict_json_schema(
    schema: &Value,
    check: Option<&UnsupportedKeyword>,
) -> Result<Value, String> {
    let mut cloned = schema.clone();
    if !cloned.is_object() {
        return Err("root schema must have type object".into());
    }
    make_node_strict(&mut cloned, check)?;
    if cloned.get("type").and_then(Value::as_str) != Some("object") {
        return Err("root schema must have type object".into());
    }
    Ok(cloned)
}

/// A tool's constrained-sampling configuration. `ToolSpec` has no field for it, so it comes
/// from the `toolConstrainedSampling` option: `{ "<tool name>": {"type": "json_schema",
/// "strict": "prefer" | "require"} }` (reference `Tool.constrainedSampling`).
pub(crate) fn constrained_sampling<'a>(req: &'a ProviderRequest, tool: &str) -> Option<&'a Value> {
    opt(req, "toolConstrainedSampling")?.get(tool)
}

/// Reference `resolveJsonSchemaStrictSampling`: `Ok(Some(true))` to send the tool strict,
/// `Ok(None)` for non-strict, `Err` when a tool requires strict mode that cannot be used.
pub(crate) fn resolve_strict(
    req: &ProviderRequest,
    tool: &ToolSpec,
    supports_strict: bool,
    check: Option<&UnsupportedKeyword>,
) -> Result<Option<bool>, String> {
    let Some(config) = constrained_sampling(req, &tool.name) else {
        return Ok(None);
    };
    if config.get("type").and_then(Value::as_str) != Some("json_schema") {
        return Ok(None);
    }
    let require = config.get("strict").and_then(Value::as_str) == Some("require");
    if supports_strict {
        return match make_strict_json_schema(&tool.parameters, check) {
            Ok(_) => Ok(Some(true)),
            Err(_) if !require => Ok(None),
            Err(e) => Err(format!(
                "Tool \"{}\" requires JSON-schema constrained sampling, but {e}.",
                tool.name
            )),
        };
    }
    if require {
        return Err(format!(
            "Tool \"{}\" requires JSON-schema constrained sampling, but strict tools are unsupported.",
            tool.name
        ));
    }
    Ok(None)
}

/// The parameters to send: the strict form when `strict`, else the schema as given.
pub(crate) fn tool_parameters(tool: &ToolSpec, strict: Option<bool>) -> Result<Value, String> {
    if strict == Some(true) {
        make_strict_json_schema(&tool.parameters, None)
    } else {
        Ok(tool.parameters.clone())
    }
}

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
    let level = effective_thinking(req);
    if level == ThinkingLevel::Off {
        return disabled_thinking_config(model).map(Some);
    }
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
    let normalize = |id: &str| normalize_tool_call_id(id);
    let transformed = transform_messages(
        messages,
        model,
        if needs_id { Some(&normalize) } else { None },
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
    let resp = match post_json(ctx, url, headers, body).await {
        Ok(r) => r,
        Err(err) => return ctx.failed(e, err),
    };
    let mut state = GoogleStream::default();
    let read = http::read_sse(resp, idle_timeout(ctx.req), &ctx.cancel, |event| {
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
    use super::*;
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
        let out = transform_messages(&messages, &m, Some(&normalize_tool_call_id));
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
        assert_eq!(normalize_tool_call_id("a|b.c"), "a_b_c");
        assert_eq!(normalize_tool_call_id("\u{1f600}"), "__");
        assert_eq!(normalize_tool_call_id(&"x".repeat(100)).len(), 64);
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
