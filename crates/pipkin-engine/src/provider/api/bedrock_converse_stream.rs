//! `bedrock-converse-stream`: Amazon Bedrock `ConverseStream` without the AWS SDK.
//!
//! Ported from the reference `api/bedrock-converse-stream.ts` and `providers/amazon-bedrock.ts`:
//! - `POST {endpoint}/model/{modelId}/converse-stream`, JSON body (`messages`, `system`,
//!   `inferenceConfig`, `toolConfig`, `additionalModelRequestFields`, `requestMetadata`),
//!   response in the AWS binary event-stream framing ([`super::bedrock_eventstream`]).
//! - Auth: `AWS_BEDROCK_SKIP_AUTH=1` (dummy SigV4 credentials, for unauthenticated proxies) >
//!   bearer token (`options.bearerToken` > the request credential > `AWS_BEARER_TOKEN_BEDROCK`) >
//!   SigV4 with the credential chain in [`super::aws_auth`]. A profile chosen for this provider
//!   (`options.profile`, the request-scoped `AWS_PROFILE`, or `profile` in an ambient
//!   credential's metadata) wins over environment keys.
//! - Region: inference-profile ARN region > `options.region` > `AWS_REGION` >
//!   `AWS_DEFAULT_REGION` > the catalog endpoint's region (when that endpoint is used) >
//!   `us-east-1` without an ambient `AWS_PROFILE` > the profile's configured region.
//!   The catalog `baseUrl` is used as the endpoint when it is not a standard Bedrock runtime
//!   host (VPC endpoints, proxies) or when no region/profile is configured.
//! - Claude models get cache points (system + last user message) and thinking controls:
//!   adaptive thinking with `output_config.effort` for Opus/Sonnet 4.6+ and the 5-series
//!   (with `block_binding` + beta on models that accept it), else a token budget with the
//!   interleaved-thinking beta. `AWS_BEDROCK_FORCE_CACHE=1` forces cache points for
//!   application inference profiles whose ids do not name the model.
//! - Errors keep the reference prefixes (`Throttling error:`, `Service unavailable:`, ...)
//!   that `provider/errors.rs` classifies. Request-level retry follows the SDK's standard
//!   mode (3 attempts, throttling/transient errors, jittered exponential backoff).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use futures_util::StreamExt;
use pipkin_engine_protocol::{Block, Message, StopReason};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;
use url::Url;

use super::aws_auth::{self, AwsCredentials, ChainInput, SigningParams};
use super::bedrock_eventstream::{EventMessage, EventStreamDecoder};
use super::common::{
    adjust_max_tokens_for_thinking, base_max_tokens, clamp_max_tokens_to_context,
    clamp_thinking_budget_to_answer_room, header_map, sanitize_tool_call_id,
    thinking_budget_for_level, transform_messages,
};
use crate::provider::http;
use crate::provider::{
    ApiAdapter, BoxFuture, CacheRetention, Credential, Emitter, Model, ProviderRequest, StreamCtx,
    ThinkingLevel,
};

pub const API: &str = "bedrock-converse-stream";
const EMPTY_TEXT_PLACEHOLDER: &str = "<empty>";
const THINKING_BINDING_CONTROLS_BETA: &str = "thinking-binding-controls-2026-08-01";
const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";
const DATA_RETENTION_DOCS_URL: &str =
    "https://docs.aws.amazon.com/bedrock/latest/userguide/data-retention.html";
const MAX_ATTEMPTS: u32 = 3;

/// Reads the process environment (the "ambient" layer below request env overrides).
pub type AmbientEnv = fn(&str) -> Option<String>;

fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

pub fn adapter() -> Arc<dyn ApiAdapter> {
    Arc::new(BedrockConverseStream {
        ambient: process_env,
    })
}

pub struct BedrockConverseStream {
    ambient: AmbientEnv,
}

impl ApiAdapter for BedrockConverseStream {
    fn api(&self) -> &'static str {
        API
    }

    fn stream<'a>(&'a self, ctx: StreamCtx<'a>) -> BoxFuture<'a, Message> {
        Box::pin(self.run(ctx))
    }
}

// ---------------------------------------------------------------------------------------------
// Model capability checks (reference :753-904)
// ---------------------------------------------------------------------------------------------

fn candidates(model: &Model) -> Vec<String> {
    let mut values = vec![model.id.as_str()];
    if !model.name.is_empty() {
        values.push(model.name.as_str());
    }
    let mut out = Vec::new();
    for v in values {
        let lower = v.to_lowercase();
        let mut dashed = String::with_capacity(lower.len());
        let mut in_sep = false;
        for c in lower.chars() {
            if c.is_whitespace() || matches!(c, '_' | '.' | ':') {
                if !in_sep {
                    dashed.push('-');
                }
                in_sep = true;
            } else {
                dashed.push(c);
                in_sep = false;
            }
        }
        out.push(lower);
        out.push(dashed);
    }
    out
}

fn any_candidate(model: &Model, needles: &[&str]) -> bool {
    candidates(model)
        .iter()
        .any(|c| needles.iter().any(|n| c.contains(n)))
}

pub(crate) fn is_claude(model: &Model) -> bool {
    let id = model.id.to_lowercase();
    let name = model.name.to_lowercase();
    id.contains("anthropic.claude")
        || id.contains("anthropic/claude")
        || name.contains("anthropic.claude")
        || name.contains("anthropic/claude")
        || name.contains("claude")
}

/// Opus 4.6+, Sonnet 4.6 and the 5-series take adaptive thinking.
pub(crate) fn supports_adaptive_thinking(model: &Model) -> bool {
    any_candidate(
        model,
        &[
            "opus-4-6",
            "opus-4-7",
            "opus-4-8",
            "opus-5",
            "sonnet-4-6",
            "sonnet-5",
            "fable-5",
        ],
    )
}

const NATIVE_XHIGH: &[&str] = &["opus-4-7", "opus-4-8", "opus-5", "sonnet-5", "fable-5"];

fn supports_native_xhigh(model: &Model) -> bool {
    any_candidate(model, NATIVE_XHIGH)
}

/// Opus/Sonnet 4.6 reject `thinking.block_binding`.
fn supports_block_binding(model: &Model) -> bool {
    any_candidate(model, NATIVE_XHIGH)
}

fn supports_prompt_caching(model: &Model, force: bool) -> bool {
    let c = candidates(model);
    if !c.iter().any(|s| s.contains("claude")) {
        return force;
    }
    c.iter().any(|s| {
        s.contains("fable-5")
            || s.contains("opus-5")
            || s.contains("sonnet-5")
            || s.contains("-4-")
            || s.contains("claude-3-7-sonnet")
            || s.contains("claude-3-5-haiku")
    })
}

fn effort(model: &Model, level: ThinkingLevel) -> String {
    if level == ThinkingLevel::Xhigh && supports_native_xhigh(model) {
        return "xhigh".into();
    }
    if let Some(mapped) = model.thinking_value(level) {
        return mapped.to_owned();
    }
    match level {
        ThinkingLevel::Minimal | ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        _ => "high",
    }
    .into()
}

// ---------------------------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------------------------

struct Env<'a> {
    req: &'a ProviderRequest,
    ambient: AmbientEnv,
}

impl Env<'_> {
    /// Request overrides (an empty override masks the process value), then the process.
    fn get(&self, name: &str) -> Option<String> {
        match self.req.env.get(name) {
            Some(v) => Some(v.clone()),
            None => (self.ambient)(name),
        }
        .filter(|v| !v.is_empty())
    }

    fn option(&self, name: &str) -> Option<String> {
        let ambient_meta = match &self.req.credential {
            Credential::Ambient(m) => m.get(name).and_then(Value::as_str),
            _ => None,
        };
        self.req
            .options
            .get(name)
            .and_then(Value::as_str)
            .or(ambient_meta)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    }

    fn configured_region(&self) -> Option<String> {
        self.option("region")
            .or_else(|| self.get("AWS_REGION"))
            .or_else(|| self.get("AWS_DEFAULT_REGION"))
    }

    /// A profile chosen for this provider (option, request-scoped `AWS_PROFILE`, credential
    /// metadata).
    fn explicit_profile(&self) -> Option<String> {
        self.option("profile").or_else(|| {
            self.req
                .env
                .get("AWS_PROFILE")
                .filter(|v| !v.is_empty())
                .cloned()
        })
    }

    fn ambient_profile(&self) -> Option<String> {
        (self.ambient)("AWS_PROFILE")
    }
}

/// The region of a standard `bedrock-runtime[-fips].<region>.amazonaws.com[.cn]` host.
fn standard_endpoint_region(base_url: &str) -> Option<String> {
    let host = Url::parse(base_url).ok()?.host_str()?.to_lowercase();
    let rest = host
        .strip_prefix("bedrock-runtime-fips.")
        .or_else(|| host.strip_prefix("bedrock-runtime."))?;
    let region = rest
        .strip_suffix(".amazonaws.com.cn")
        .or_else(|| rest.strip_suffix(".amazonaws.com"))?;
    (!region.is_empty()
        && region
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
    .then(|| region.to_owned())
}

fn arn_region(model_id: &str) -> Option<String> {
    let rest = model_id.strip_prefix("arn:aws")?;
    let rest = match rest.strip_prefix(':') {
        Some(r) => r,
        None => {
            let (_partition, r) = rest.strip_prefix('-')?.split_once(':')?;
            r
        }
    };
    let rest = rest.strip_prefix("bedrock:")?;
    let (region, _) = rest.split_once(':')?;
    (!region.is_empty()
        && region
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
    .then(|| region.to_owned())
}

#[derive(Debug, PartialEq)]
pub(crate) struct Target {
    pub url: String,
    pub region: String,
}

fn resolve_target(env: &Env<'_>) -> Target {
    let req = env.req;
    let base = req.base_url().trim();
    let configured = env.configured_region();
    let ambient_profile = env.ambient_profile();
    let endpoint_region = standard_endpoint_region(base);
    let use_explicit = !base.is_empty()
        && (endpoint_region.is_none() || (configured.is_none() && ambient_profile.is_none()));
    let region = arn_region(&req.model.id)
        .or(configured)
        .or_else(|| endpoint_region.clone().filter(|_| use_explicit))
        .or_else(|| ambient_profile.is_none().then(|| "us-east-1".to_owned()))
        .or_else(|| {
            let profile = env.explicit_profile().or(ambient_profile.clone())?;
            let lookup = |k: &str| env.get(k);
            aws_auth::profile_region(&lookup, &profile)
        })
        .unwrap_or_else(|| "us-east-1".to_owned());
    let endpoint = if use_explicit {
        base.trim_end_matches('/').to_owned()
    } else {
        let suffix = if region.starts_with("cn-") {
            "amazonaws.com.cn"
        } else {
            "amazonaws.com"
        };
        format!("https://bedrock-runtime.{region}.{suffix}")
    };
    Target {
        url: format!(
            "{endpoint}/model/{}/converse-stream",
            aws_auth::encode_path_segment(&req.model.id)
        ),
        region,
    }
}

fn is_govcloud(env: &Env<'_>) -> bool {
    if env
        .configured_region()
        .is_some_and(|r| r.to_lowercase().starts_with("us-gov-"))
    {
        return true;
    }
    let id = env.req.model.id.to_lowercase();
    id.starts_with("us-gov.") || id.starts_with("arn:aws-us-gov:")
}

// ---------------------------------------------------------------------------------------------
// Request body
// ---------------------------------------------------------------------------------------------

fn non_blank(text: &str) -> Option<Value> {
    (!text.trim().is_empty()).then(|| json!({"text": text}))
}

fn image_block(mime: &str, data: &str) -> Result<Value, String> {
    let format = match mime {
        "image/jpeg" | "image/jpg" => "jpeg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        other => return Err(format!("Unknown image type: {other}")),
    };
    Ok(json!({"format": format, "source": {"bytes": data}}))
}

/// Bedrock rejects empty object keys in tool-use documents.
fn sanitize_document(v: &Value) -> Value {
    match v {
        Value::Array(a) => Value::Array(a.iter().map(sanitize_document).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .filter(|(k, _)| !k.is_empty())
                .map(|(k, v)| (k.clone(), sanitize_document(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn tool_result_content(content: &[Block]) -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    for b in content {
        match b {
            Block::Image {
                data, mime_type, ..
            } => out.push(json!({"image": image_block(mime_type, data)?})),
            Block::Text { text, .. } => out.extend(non_blank(text)),
            _ => {}
        }
    }
    if out.is_empty() {
        out.push(json!({"text": EMPTY_TEXT_PLACEHOLDER}));
    }
    Ok(out)
}

fn cache_point(retention: CacheRetention) -> Value {
    if retention == CacheRetention::Long {
        json!({"cachePoint": {"type": "default", "ttl": "1h"}})
    } else {
        json!({"cachePoint": {"type": "default"}})
    }
}

fn convert_messages(req: &ProviderRequest, caching: bool) -> Result<Vec<Value>, String> {
    let model = &req.model;
    let claude = is_claude(model);
    let mut normalize = |id: &str, _: &Message| sanitize_tool_call_id(id);
    let messages = transform_messages(&req.messages, model, Some(&mut normalize));
    let mut out: Vec<Value> = Vec::new();
    let mut i = 0;
    while i < messages.len() {
        match &messages[i] {
            Message::System(_) => {
                unreachable!("Bedrock system messages must be collapsed before conversion")
            }
            Message::User { content, .. } => {
                let mut blocks = Vec::new();
                for b in content {
                    match b {
                        Block::Text { text, .. } => blocks.extend(non_blank(text)),
                        Block::Image {
                            data, mime_type, ..
                        } => blocks.push(json!({"image": image_block(mime_type, data)?})),
                        _ => {}
                    }
                }
                if blocks.is_empty() {
                    blocks.push(json!({"text": EMPTY_TEXT_PLACEHOLDER}));
                }
                out.push(json!({"role": "user", "content": blocks}));
            }
            Message::Assistant { content, .. } => {
                let mut blocks = Vec::new();
                for b in content {
                    match b {
                        Block::Text { text, .. } => blocks.extend(non_blank(text)),
                        Block::ToolCall {
                            id,
                            name,
                            arguments,
                            ..
                        } => blocks.push(json!({"toolUse": {
                            "toolUseId": id,
                            "name": name,
                            "input": sanitize_document(arguments),
                        }})),
                        Block::Thinking {
                            thinking,
                            signature,
                            redacted,
                            ..
                        } => {
                            if *redacted {
                                let decoded = signature.as_deref().and_then(|s| {
                                    base64::engine::general_purpose::STANDARD.decode(s).ok()
                                });
                                if let Some(bytes) = decoded.filter(|b| !b.is_empty()) {
                                    let b64 =
                                        base64::engine::general_purpose::STANDARD.encode(bytes);
                                    blocks.push(
                                        json!({"reasoningContent": {"redactedContent": b64}}),
                                    );
                                }
                                continue;
                            }
                            if thinking.trim().is_empty() {
                                continue;
                            }
                            if claude {
                                match signature.as_deref().filter(|s| !s.trim().is_empty()) {
                                    None => blocks.push(json!({"text": thinking})),
                                    Some(sig) => blocks.push(json!({"reasoningContent": {
                                        "reasoningText": {"text": thinking, "signature": sig}
                                    }})),
                                }
                            } else {
                                blocks.push(json!({"reasoningContent": {
                                    "reasoningText": {"text": thinking}
                                }}));
                            }
                        }
                        Block::Image { .. } => {}
                    }
                }
                if !blocks.is_empty() {
                    out.push(json!({"role": "assistant", "content": blocks}));
                }
            }
            Message::ToolResult { .. } => {
                let mut results = Vec::new();
                while let Some(Message::ToolResult {
                    tool_call_id,
                    content,
                    is_error,
                    ..
                }) = messages.get(i)
                {
                    results.push(json!({"toolResult": {
                        "toolUseId": tool_call_id,
                        "content": tool_result_content(content)?,
                        "status": if *is_error { "error" } else { "success" },
                    }}));
                    i += 1;
                }
                out.push(json!({"role": "user", "content": results}));
                continue;
            }
        }
        i += 1;
    }
    if caching
        && let Some(last) = out.last_mut()
        && last["role"] == "user"
        && let Some(content) = last["content"].as_array_mut()
    {
        content.push(cache_point(req.cache_retention));
    }
    Ok(out)
}

fn tool_config(req: &ProviderRequest) -> Result<Option<Value>, String> {
    if req.tools.is_empty() {
        return Ok(None);
    }
    let choice = req.options.get("toolChoice");
    if choice.and_then(Value::as_str) == Some("none") {
        return Ok(None);
    }
    let tools: Vec<Value> = req
        .tools
        .iter()
        .map(|t| {
            let strict = super::common::schema::resolve_strict(
                req,
                t,
                req.model.compat_bool("supportsStrictMode").unwrap_or(false),
                None,
            )?;
            let mut tool = json!({"toolSpec": {
                "name": t.name,
                "description": t.description,
                "inputSchema": {"json": super::common::schema::tool_parameters(t, strict)?},
            }});
            if strict == Some(true) {
                tool["toolSpec"]["strict"] = json!(true);
            }
            Ok(tool)
        })
        .collect::<Result<_, String>>()?;
    let mut config = json!({"tools": tools});
    let tool_choice = match choice {
        Some(Value::String(s)) if s == "auto" => Some(json!({"auto": {}})),
        Some(Value::String(s)) if s == "any" => Some(json!({"any": {}})),
        Some(Value::Object(o)) if o.get("type").and_then(Value::as_str) == Some("tool") => o
            .get("name")
            .and_then(Value::as_str)
            .map(|n| json!({"tool": {"name": n}})),
        _ => None,
    };
    if let Some(tc) = tool_choice {
        config["toolChoice"] = tc;
    }
    Ok(Some(config))
}

/// `(maxTokens, thinking budget)` for budget-based Claude thinking (reference `streamSimple`,
/// `bedrock-converse-stream.ts:555-571`: `adjustMaxTokensForThinking` on the context-clamped
/// base, then `clampMaxTokensToContext` again).
fn budget_and_max_tokens(req: &ProviderRequest, level: ThinkingLevel) -> (u64, u64) {
    let (adjusted, budget) = adjust_max_tokens_for_thinking(req, Some(base_max_tokens(req)), level);
    let max_tokens = clamp_max_tokens_to_context(req, adjusted);
    (
        max_tokens,
        clamp_thinking_budget_to_answer_room(budget, max_tokens),
    )
}

fn additional_fields(env: &Env<'_>, budget: Option<u64>) -> Option<Value> {
    let req = env.req;
    let model = &req.model;
    if req.thinking == ThinkingLevel::Off || !model.reasoning || !is_claude(model) {
        return None;
    }
    let gov = is_govcloud(env);
    let display = if gov {
        None
    } else {
        Some(
            req.options
                .get("thinkingDisplay")
                .and_then(Value::as_str)
                .unwrap_or("summarized")
                .to_owned(),
        )
    };
    let adaptive = supports_adaptive_thinking(model);
    let mut out = Map::new();
    if adaptive {
        let binding = !gov && supports_block_binding(model);
        let mut thinking = json!({"type": "adaptive"});
        if let Some(d) = &display {
            thinking["display"] = json!(d);
        }
        if binding {
            thinking["block_binding"] = json!({"prefix_mismatch_behavior": "drop_block"});
        }
        out.insert("thinking".into(), thinking);
        out.insert(
            "output_config".into(),
            json!({"effort": effort(model, req.thinking)}),
        );
        if binding {
            out.insert(
                "anthropic_beta".into(),
                json!([THINKING_BINDING_CONTROLS_BETA]),
            );
        }
    } else {
        let budget = budget.unwrap_or_else(|| thinking_budget_for_level(req, req.thinking));
        let mut thinking = json!({"type": "enabled", "budget_tokens": budget});
        if let Some(d) = &display {
            thinking["display"] = json!(d);
        }
        out.insert("thinking".into(), thinking);
        if req
            .options
            .get("interleavedThinking")
            .and_then(Value::as_bool)
            .unwrap_or(true)
        {
            out.insert("anthropic_beta".into(), json!([INTERLEAVED_THINKING_BETA]));
        }
    }
    Some(Value::Object(out))
}

/// The JSON body sent (the model id travels in the path).
fn build_body(env: &Env<'_>) -> Result<Value, String> {
    let resolved = env.req.resolve_systems(false);
    let req = resolved.as_ref();
    let model = &req.model;
    let caching = req.cache_retention != CacheRetention::None
        && supports_prompt_caching(
            model,
            env.get("AWS_BEDROCK_FORCE_CACHE").as_deref() == Some("1"),
        );
    let thinking_on = req.thinking != ThinkingLevel::Off;
    let (max_tokens, budget) =
        if thinking_on && model.reasoning && is_claude(model) && !supports_adaptive_thinking(model)
        {
            let (m, b) = budget_and_max_tokens(req, req.thinking);
            (m, Some(b))
        } else {
            // Reference `buildBaseOptions`: the cap is clamped to the context window.
            (base_max_tokens(req), None)
        };
    let mut body = Map::new();
    body.insert(
        "messages".into(),
        Value::Array(convert_messages(req, caching)?),
    );
    if !req.system.is_empty() {
        let mut system = vec![json!({"text": req.system})];
        if caching {
            system.push(cache_point(req.cache_retention));
        }
        body.insert("system".into(), Value::Array(system));
    }
    let mut inference = json!({"maxTokens": max_tokens});
    if let Some(t) = req.options.get("temperature").filter(|v| v.is_number()) {
        inference["temperature"] = t.clone();
    }
    body.insert("inferenceConfig".into(), inference);
    if let Some(tc) = tool_config(req)? {
        body.insert("toolConfig".into(), tc);
    }
    if let Some(fields) = additional_fields(env, budget) {
        body.insert("additionalModelRequestFields".into(), fields);
    }
    if let Some(meta) = req.options.get("requestMetadata").filter(|v| v.is_object()) {
        body.insert("requestMetadata".into(), meta.clone());
    }
    Ok(Value::Object(body))
}

// ---------------------------------------------------------------------------------------------
// Errors and retry
// ---------------------------------------------------------------------------------------------

/// Human-readable prefixes for Bedrock exception names (reference `BEDROCK_ERROR_PREFIXES`).
fn error_prefix(name: &str) -> &str {
    match name {
        "InternalServerException" => "Internal server error",
        "ModelStreamErrorException" => "Model stream error",
        "ValidationException" => "Validation error",
        "ThrottlingException" => "Throttling error",
        "ServiceUnavailableException" => "Service unavailable",
        other => other,
    }
}

fn with_data_retention_hint(message: String) -> String {
    if message.to_lowercase().contains("data retention mode") {
        format!("{message} See {DATA_RETENTION_DOCS_URL} for supported data retention modes.")
    } else {
        message
    }
}

/// The modeled error code of an HTTP error response.
fn error_code(headers: &reqwest::header::HeaderMap, body: &Value) -> Option<String> {
    let raw = headers
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .or_else(|| {
            ["__type", "code", "Code"]
                .iter()
                .find_map(|k| body.get(*k).and_then(Value::as_str).map(str::to_owned))
        })?;
    let code = raw.split(':').next().unwrap_or("");
    let code = code.rsplit('#').next().unwrap_or("").trim();
    (!code.is_empty()).then(|| code.to_owned())
}

const THROTTLING_CODES: &[&str] = &[
    "BandwidthLimitExceeded",
    "EC2ThrottledException",
    "LimitExceededException",
    "PriorRequestNotComplete",
    "ProvisionedThroughputExceededException",
    "RequestLimitExceeded",
    "RequestThrottled",
    "RequestThrottledException",
    "SlowDown",
    "ThrottledException",
    "Throttling",
    "ThrottlingException",
    "TooManyRequestsException",
    "TransactionInProgressException",
];
const TRANSIENT_CODES: &[&str] = &["TimeoutError", "RequestTimeout", "RequestTimeoutException"];

struct HttpFailure {
    message: String,
    status: u16,
    code: Option<String>,
    request_id: Option<String>,
    throttling: bool,
    retryable: bool,
}

fn http_failure(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    text: &str,
) -> HttpFailure {
    let body: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let code = error_code(headers, &body);
    let message = ["message", "Message", "errorMessage"]
        .iter()
        .find_map(|k| body.get(*k).and_then(Value::as_str))
        .filter(|m| !m.trim().is_empty())
        .map(str::to_owned);
    let core = message.unwrap_or_else(|| http::format_error(Some(status), text, ""));
    let text = match &code {
        Some(code) => format!("{}: {core}", error_prefix(code)),
        None => core,
    };
    let throttling = status.as_u16() == 429
        || code
            .as_deref()
            .is_some_and(|c| THROTTLING_CODES.contains(&c));
    let transient = matches!(status.as_u16(), 500 | 502 | 503 | 504)
        || code
            .as_deref()
            .is_some_and(|c| TRANSIENT_CODES.contains(&c));
    HttpFailure {
        message: with_data_retention_hint(text),
        status: status.as_u16(),
        code,
        request_id: headers
            .get("x-amzn-requestid")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned),
        throttling,
        retryable: throttling || transient,
    }
}

fn backoff(attempt: u32, throttling: bool) -> Duration {
    let base = if throttling { 500.0 } else { 100.0 };
    let mut b = [0u8; 4];
    let _ = getrandom::getrandom(&mut b);
    let r = u32::from_le_bytes(b) as f64 / u32::MAX as f64;
    let ms = (r * 2f64.powi(attempt as i32) * base).min(20_000.0);
    Duration::from_millis(ms as u64)
}

fn diagnostic(status: Option<u16>, code: Option<&str>, request_id: Option<&str>) -> Option<Value> {
    let mut details = Map::new();
    if let Some(s) = status {
        details.insert("status".into(), json!(s));
    }
    if let Some(c) = code.filter(|c| c.ends_with("Exception") && c.len() <= 200) {
        details.insert("errorCode".into(), json!(c));
    }
    if let Some(r) = request_id
        .map(str::trim)
        .filter(|r| !r.is_empty() && r.len() <= 200)
    {
        details.insert("requestId".into(), json!(r));
    }
    (!details.is_empty()).then(|| {
        json!([{
            "type": "bedrock_response_failure",
            "timestamp": crate::util::now_ms(),
            "details": details,
        }])
    })
}

// ---------------------------------------------------------------------------------------------
// Stream
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Text,
    Thinking,
    Tool,
}

#[derive(Default)]
struct State {
    /// Bedrock `contentBlockIndex` -> (emitter index, kind) for open blocks.
    open: HashMap<u64, (usize, Kind)>,
    redacted: HashSet<usize>,
    redacted_bytes: HashMap<usize, Vec<u8>>,
    stop: Option<(StopReason, Option<String>)>,
    error_code: Option<String>,
}

fn flush_redacted(e: &mut Emitter, st: &mut State, index: usize) {
    if let Some(bytes) = st.redacted_bytes.remove(&index) {
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        e.thinking_signature(index, &b64, false);
    }
}

fn map_stop(reason: Option<&str>) -> (StopReason, Option<String>) {
    match reason {
        Some("end_turn" | "stop_sequence") => (StopReason::Stop, None),
        Some("max_tokens" | "model_context_window_exceeded") => (StopReason::Length, None),
        Some("tool_use") => (StopReason::ToolUse, None),
        Some(other) => (
            StopReason::Error,
            Some(format!("Provider stopped with: {other}")),
        ),
        None => (StopReason::Error, None),
    }
}

/// `"throttlingException"` -> `"ThrottlingException"`.
fn exception_name(event_type: &str) -> String {
    let mut c = event_type.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

const MODELED_STREAM_EXCEPTIONS: &[&str] = &[
    "InternalServerException",
    "ModelStreamErrorException",
    "ValidationException",
    "ThrottlingException",
    "ServiceUnavailableException",
];

fn on_message(e: &mut Emitter, st: &mut State, msg: &EventMessage) -> Result<(), String> {
    let payload: Value = if msg.payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&msg.payload).map_err(|err| err.to_string())?
    };
    match msg.header(":message-type").unwrap_or("event") {
        "exception" => {
            let name = exception_name(msg.header(":exception-type").unwrap_or("Unknown"));
            let message = payload
                .get("message")
                .or_else(|| payload.get("Message"))
                .and_then(Value::as_str)
                .unwrap_or(&name)
                .to_owned();
            st.error_code = Some(name.clone());
            return Err(if MODELED_STREAM_EXCEPTIONS.contains(&name.as_str()) {
                format!("{}: {message}", error_prefix(&name))
            } else {
                message
            });
        }
        "error" => {
            let code = msg.header(":error-code").unwrap_or("UnknownError");
            st.error_code = Some(code.to_owned());
            return Err(msg.header(":error-message").unwrap_or(code).to_owned());
        }
        _ => {}
    }
    let index_of = |p: &Value| {
        p.get("contentBlockIndex")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    match msg.header(":event-type").unwrap_or("") {
        "messageStart" => {
            if payload.get("role").and_then(Value::as_str) != Some("assistant") {
                return Err(
                    "Unexpected assistant message start but got user message start instead".into(),
                );
            }
        }
        "contentBlockStart" => {
            if let Some(tool) = payload.pointer("/start/toolUse") {
                let id = tool.get("toolUseId").and_then(Value::as_str).unwrap_or("");
                let name = tool.get("name").and_then(Value::as_str).unwrap_or("");
                let i = e.start_tool_call(id, name);
                st.open.insert(index_of(&payload), (i, Kind::Tool));
            }
        }
        "contentBlockDelta" => {
            let cbi = index_of(&payload);
            let delta = payload.get("delta").cloned().unwrap_or(Value::Null);
            let open = st.open.get(&cbi).copied();
            if let Some(text) = delta.get("text").and_then(Value::as_str) {
                let (i, kind) = match open {
                    Some(o) => o,
                    None => {
                        let i = e.start_text();
                        st.open.insert(cbi, (i, Kind::Text));
                        (i, Kind::Text)
                    }
                };
                if kind == Kind::Text {
                    e.text(i, text);
                }
            } else if let Some(tool) = delta.get("toolUse") {
                if let Some((i, Kind::Tool)) = open {
                    e.tool_args(i, tool.get("input").and_then(Value::as_str).unwrap_or(""));
                }
            } else if let Some(reasoning) = delta.get("reasoningContent") {
                let redacted = reasoning
                    .get("redactedContent")
                    .and_then(Value::as_str)
                    .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
                    .filter(|b| !b.is_empty());
                let (mut i, kind) = match open {
                    Some(o) => o,
                    None if redacted.is_some() => {
                        let i = e.redacted_thinking("");
                        st.redacted.insert(i);
                        st.open.insert(cbi, (i, Kind::Thinking));
                        (i, Kind::Thinking)
                    }
                    None => {
                        let i = e.start_thinking();
                        st.open.insert(cbi, (i, Kind::Thinking));
                        (i, Kind::Thinking)
                    }
                };
                if kind != Kind::Thinking {
                    return Ok(());
                }
                if let Some(text) = reasoning.get("text").and_then(Value::as_str) {
                    e.thinking(i, text);
                }
                if let Some(sig) = reasoning.get("signature").and_then(Value::as_str)
                    && !sig.is_empty()
                    && !st.redacted.contains(&i)
                {
                    e.thinking_signature(i, sig, true);
                }
                if let Some(bytes) = redacted {
                    if !st.redacted.contains(&i) {
                        // Readable reasoning turned opaque mid-block: keep the payload in its own
                        // redacted block so it replays verbatim.
                        i = e.redacted_thinking("");
                        st.redacted.insert(i);
                        st.open.insert(cbi, (i, Kind::Thinking));
                    }
                    st.redacted_bytes.entry(i).or_default().extend(bytes);
                }
            }
        }
        "contentBlockStop" => {
            if let Some((i, kind)) = st.open.remove(&index_of(&payload))
                && kind == Kind::Thinking
            {
                flush_redacted(e, st, i);
            }
        }
        "messageStop" => {
            let reason = payload.get("stopReason").and_then(Value::as_str);
            if let Some(r) = reason {
                e.message_extra("rawStopReason", json!(r));
            }
            st.stop = Some(map_stop(reason));
        }
        "metadata" => {
            if let Some(usage) = payload.get("usage").filter(|u| u.is_object()) {
                let n = |k: &str| usage.get(k).and_then(Value::as_u64).unwrap_or(0);
                let one_hour: u64 = usage
                    .get("cacheDetails")
                    .and_then(Value::as_array)
                    .map(|details| {
                        details
                            .iter()
                            .filter(|d| d.get("ttl").and_then(Value::as_str) == Some("1h"))
                            .filter_map(|d| d.get("inputTokens").and_then(Value::as_u64))
                            .sum()
                    })
                    .unwrap_or(0);
                let u = e.usage_mut();
                u.input = n("inputTokens");
                u.output = n("outputTokens");
                u.cache_read = n("cacheReadInputTokens");
                u.cache_write = n("cacheWriteInputTokens");
                u.total_tokens = match n("totalTokens") {
                    0 => u.input + u.output,
                    t => t,
                };
                e.set_cache_write_1h(one_hour);
            }
        }
        _ => {}
    }
    Ok(())
}

enum Auth {
    Bearer(String),
    SigV4(AwsCredentials),
}

impl BedrockConverseStream {
    async fn auth(&self, env: &Env<'_>, region: &str, ctx: &StreamCtx<'_>) -> Result<Auth, String> {
        if env.get("AWS_BEDROCK_SKIP_AUTH").as_deref() == Some("1") {
            return Ok(Auth::SigV4(AwsCredentials::new(
                "dummy-access-key",
                "dummy-secret-key",
                None,
            )));
        }
        let bearer = env
            .option("bearerToken")
            .or_else(|| env.req.credential.secret().map(str::to_owned))
            .or_else(|| env.get("AWS_BEARER_TOKEN_BEDROCK"));
        if let Some(token) = bearer {
            return Ok(Auth::Bearer(token));
        }
        let lookup = |k: &str| env.get(k);
        aws_auth::resolve_credentials(&ChainInput {
            env: &lookup,
            explicit_profile: env.explicit_profile(),
            ambient_profile: env.ambient_profile(),
            region: region.to_owned(),
            http: ctx.http,
            cancel: &ctx.cancel,
        })
        .await
        .map(Auth::SigV4)
    }

    async fn run(&self, ctx: StreamCtx<'_>) -> Message {
        let req = ctx.req;
        let mut e = ctx.emitter();
        let env = Env {
            req,
            ambient: self.ambient,
        };
        let target = resolve_target(&env);
        let body = match build_body(&env) {
            Ok(b) => serde_json::to_vec(&b).unwrap_or_default(),
            Err(err) => return ctx.failed(e, with_data_retention_hint(err)),
        };
        let auth = match self.auth(&env, &target.region, &ctx).await {
            Ok(a) => a,
            Err(err) => return ctx.failed(e, err),
        };
        let Ok(url) = Url::parse(&target.url) else {
            return ctx.failed(e, format!("Invalid Bedrock endpoint: {}", target.url));
        };

        let mut custom: Vec<(String, String)> =
            vec![("content-type".into(), "application/json".into())];
        for (k, v) in req
            .model
            .headers
            .iter()
            .map(|(k, v)| (k, Some(v)))
            .chain(req.headers.iter().map(|(k, v)| (k, v.as_ref())))
        {
            let lower = k.to_ascii_lowercase();
            if lower == "authorization" || lower == "host" || lower.starts_with("x-amz-") {
                continue;
            }
            custom.retain(|(n, _)| *n != lower);
            if let Some(v) = v {
                custom.push((lower, v.clone()));
            }
        }

        let mut attempt = 0;
        let resp = loop {
            let mut headers = custom.clone();
            match &auth {
                Auth::Bearer(token) => {
                    headers.push(("authorization".into(), format!("Bearer {token}")))
                }
                Auth::SigV4(creds) => {
                    aws_auth::sign(
                        "POST",
                        &url,
                        &mut headers,
                        &body,
                        &SigningParams {
                            credentials: creds,
                            region: &target.region,
                            service: "bedrock",
                            time: SystemTime::now(),
                            content_sha256_header: true,
                        },
                    );
                }
            }
            let headers = match header_map(&headers) {
                Ok(h) => h,
                Err(err) => return ctx.failed(e, err),
            };
            let send = ctx
                .http
                .post(url.clone())
                .headers(headers)
                .body(body.clone())
                .send();
            let result = tokio::select! {
                _ = ctx.cancel.cancelled() => return ctx.failed(e, "Request was aborted"),
                r = send => r,
            };
            let (failure, throttling) = match result {
                Ok(r) if r.status().is_success() => break r,
                Ok(r) => {
                    let status = r.status();
                    let headers = r.headers().clone();
                    let text = tokio::select! {
                        _ = ctx.cancel.cancelled() => return ctx.failed(e, "Request was aborted"),
                        t = r.text() => t.unwrap_or_default(),
                    };
                    let f = http_failure(status, &headers, &text);
                    if !f.retryable || attempt + 1 >= MAX_ATTEMPTS {
                        if let Some(d) =
                            diagnostic(Some(f.status), f.code.as_deref(), f.request_id.as_deref())
                        {
                            e.message_extra("diagnostics", d);
                        }
                        return ctx.failed(e, f.message);
                    }
                    let throttling = f.throttling;
                    (f.message, throttling)
                }
                Err(err) => {
                    let text = with_data_retention_hint(http::transport_error(&err));
                    if attempt + 1 >= MAX_ATTEMPTS {
                        return ctx.failed(e, text);
                    }
                    (text, false)
                }
            };
            log::debug!("bedrock request attempt {} failed: {failure}", attempt + 1);
            let wait = backoff(attempt, throttling);
            attempt += 1;
            tokio::select! {
                _ = ctx.cancel.cancelled() => return ctx.failed(e, "Request was aborted"),
                _ = tokio::time::sleep(wait) => {}
            }
        };
        let request_id = resp
            .headers()
            .get("x-amzn-requestid")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);

        let mut st = State::default();
        let outcome = read_stream(resp, req.timeout, &ctx.cancel, &mut e, &mut st).await;
        for i in st.redacted_bytes.keys().copied().collect::<Vec<_>>() {
            flush_redacted(&mut e, &mut st, i);
        }
        if ctx.cancel.is_cancelled() {
            return ctx.failed(e, "Request was aborted");
        }
        let fail = |mut e: Emitter, st: &State, message: String| {
            if let Some(d) = diagnostic(None, st.error_code.as_deref(), request_id.as_deref()) {
                e.message_extra("diagnostics", d);
            }
            ctx.failed(e, with_data_retention_hint(message))
        };
        if let Err(err) = outcome {
            return fail(e, &st, err);
        }
        match st.stop.take() {
            None => fail(e, &st, "Bedrock stream ended without a stop reason".into()),
            Some((StopReason::Error, message)) => fail(
                e,
                &st,
                message.unwrap_or_else(|| "An unknown error occurred".into()),
            ),
            Some((stop, _)) => e.finish(req, stop, None),
        }
    }
}

async fn read_stream(
    resp: reqwest::Response,
    idle: Duration,
    cancel: &CancellationToken,
    e: &mut Emitter,
    st: &mut State,
) -> Result<(), String> {
    let mut stream = resp.bytes_stream();
    let mut decoder = EventStreamDecoder::new();
    loop {
        let next = tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            n = async {
                if idle.is_zero() {
                    Ok(stream.next().await)
                } else {
                    tokio::time::timeout(idle, stream.next()).await
                }
            } => n,
        };
        match next {
            Err(_) => {
                return Err(format!(
                    "Stream idle timeout: no data for {} seconds (timed out)",
                    idle.as_secs()
                ));
            }
            Ok(None) => return decoder.finish(),
            Ok(Some(Err(err))) => return Err(http::transport_error(&err)),
            Ok(Some(Ok(bytes))) => {
                for msg in decoder.push(&bytes)? {
                    on_message(e, st, &msg)?;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::api::bedrock_eventstream::{encode, encode_event};
    use crate::provider::mock::{MockResponse, MockServer};
    use crate::provider::{StreamEvent, ToolSpec};
    use pipkin_engine_protocol::Usage;
    use std::collections::BTreeMap;

    fn no_ambient(_: &str) -> Option<String> {
        None
    }

    fn adapter_for_tests() -> BedrockConverseStream {
        BedrockConverseStream {
            ambient: no_ambient,
        }
    }

    fn model(id: &str, extra: Value) -> Model {
        let mut base = json!({
            "id": id,
            "name": "",
            "api": API,
            "provider": "amazon-bedrock",
            "baseUrl": "https://bedrock-runtime.us-east-1.amazonaws.com",
            "reasoning": true,
            "input": ["text", "image"],
            "cost": {"input": 3.0, "output": 15.0, "cacheRead": 0.3, "cacheWrite": 3.75},
            "contextWindow": 200000,
            "maxTokens": 64000
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
            session_id: "s".into(),
            max_tokens: Some(8000),
            cache_retention: CacheRetention::Short,
            credential: Credential::None,
            headers: BTreeMap::new(),
            base_url: None,
            timeout: Duration::from_secs(5),
            env: HashMap::from([
                ("AWS_ACCESS_KEY_ID".into(), "AKIDTEST".into()),
                ("AWS_SECRET_ACCESS_KEY".into(), "secret".into()),
                ("AWS_REGION".into(), "us-west-2".into()),
            ]),
            options: Map::new(),
        }
    }

    fn env(req: &ProviderRequest) -> Env<'_> {
        Env {
            req,
            ambient: no_ambient,
        }
    }

    async fn exchange(
        req: &ProviderRequest,
        cancel: CancellationToken,
    ) -> (Message, Vec<StreamEvent>) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let http = http::client();
        let msg = adapter_for_tests()
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

    fn eventstream(bytes: Vec<u8>, piece: usize) -> MockResponse {
        MockResponse {
            status: 200,
            headers: vec![
                (
                    "content-type".into(),
                    "application/vnd.amazon.eventstream".into(),
                ),
                ("x-amzn-requestid".into(), "req-123".into()),
            ],
            chunks: bytes.chunks(piece).map(<[u8]>::to_vec).collect(),
            pause: Duration::from_millis(1),
            hang: false,
        }
    }

    fn assistant(content: Vec<Block>, provider: &str, model_id: &str) -> Message {
        Message::Assistant {
            content,
            api: API.into(),
            provider: provider.into(),
            model: model_id.into(),
            usage: Usage::default(),
            stop_reason: StopReason::ToolUse,
            error_message: None,
            timestamp: 0,
            extra: Map::new(),
        }
    }

    #[test]
    fn region_and_endpoint_resolution() {
        let m = model("anthropic.claude-sonnet-4-5-20250929-v1:0", json!({}));
        // AWS_REGION configured: the standard endpoint follows the region.
        let req = request(m.clone());
        assert_eq!(
            resolve_target(&env(&req)),
            Target {
                url: "https://bedrock-runtime.us-west-2.amazonaws.com/model/anthropic.claude-sonnet-4-5-20250929-v1%3A0/converse-stream".into(),
                region: "us-west-2".into(),
            }
        );
        // Nothing configured: the catalog endpoint and its region.
        let mut req = request(model(
            "eu.anthropic.claude-x",
            json!({"baseUrl": "https://bedrock-runtime.eu-central-1.amazonaws.com"}),
        ));
        req.env.remove("AWS_REGION");
        let t = resolve_target(&env(&req));
        assert_eq!(t.region, "eu-central-1");
        assert!(
            t.url
                .starts_with("https://bedrock-runtime.eu-central-1.amazonaws.com/model/")
        );
        // A custom endpoint always passes through, with the configured region for signing.
        let mut req = request(m.clone());
        req.base_url = Some("https://bedrock-vpc.example.com/".into());
        let t = resolve_target(&env(&req));
        assert_eq!(t.region, "us-west-2");
        assert!(t.url.starts_with("https://bedrock-vpc.example.com/model/"));
        // An inference-profile ARN carries its own region.
        let req = request(model(
            "arn:aws:bedrock:ap-south-1:123:application-inference-profile/abc",
            json!({}),
        ));
        let t = resolve_target(&env(&req));
        assert_eq!(t.region, "ap-south-1");
        assert!(t.url.ends_with("/model/arn%3Aaws%3Abedrock%3Aap-south-1%3A123%3Aapplication-inference-profile%2Fabc/converse-stream"));
        assert_eq!(
            arn_region("arn:aws-us-gov:bedrock:us-gov-west-1:1:x"),
            Some("us-gov-west-1".into())
        );
        assert_eq!(
            standard_endpoint_region("https://bedrock-runtime-fips.us-east-2.amazonaws.com"),
            Some("us-east-2".into())
        );
        assert_eq!(
            standard_endpoint_region("https://bedrock-runtime.cn-north-1.amazonaws.com.cn"),
            Some("cn-north-1".into())
        );
    }

    #[test]
    fn capability_checks_follow_the_reference_model_list() {
        let m = |id: &str, name: &str| model(id, json!({"name": name}));
        assert!(supports_adaptive_thinking(&m(
            "us.anthropic.claude-opus-4-6-v1",
            ""
        )));
        assert!(supports_adaptive_thinking(&m(
            "arn:aws:bedrock:us-east-1:1:application-inference-profile/x",
            "Claude Sonnet 4.6"
        )));
        assert!(!supports_adaptive_thinking(&m(
            "anthropic.claude-sonnet-4-5-20250929-v1:0",
            ""
        )));
        assert!(supports_native_xhigh(&m("anthropic.claude-opus-4-7", "")));
        assert!(!supports_block_binding(&m(
            "anthropic.claude-sonnet-4-6",
            ""
        )));
        assert!(supports_block_binding(&m("anthropic.claude-fable-5", "")));
        assert!(supports_prompt_caching(
            &m("anthropic.claude-3-7-sonnet-20250219-v1:0", ""),
            false
        ));
        assert!(!supports_prompt_caching(
            &m("anthropic.claude-3-haiku-20240307-v1:0", ""),
            false
        ));
        assert!(!supports_prompt_caching(
            &m("amazon.nova-pro-v1:0", ""),
            false
        ));
        assert!(supports_prompt_caching(
            &m(
                "arn:aws:bedrock:us-east-1:1:application-inference-profile/x",
                ""
            ),
            true
        ));
        assert!(!is_claude(&m("openai.gpt-oss-120b-1:0", "")));
    }

    #[test]
    fn golden_body_with_tools_images_cache_and_replay() {
        let mut req = request(model(
            "anthropic.claude-sonnet-4-5-20250929-v1:0",
            json!({}),
        ));
        req.cache_retention = CacheRetention::Long;
        req.thinking = ThinkingLevel::Medium;
        req.options.insert("temperature".into(), json!(0.2));
        req.options.insert("toolChoice".into(), json!("auto"));
        req.options
            .insert("thinkingBudgets".into(), json!({"medium": 4000}));
        req.options
            .insert("requestMetadata".into(), json!({"team": "a"}));
        req.tools = vec![ToolSpec {
            constrained_sampling: None,
            name: "read".into(),
            description: "Read".into(),
            parameters: json!({"type": "object"}),
        }];
        let same = "anthropic.claude-sonnet-4-5-20250929-v1:0";
        req.messages = vec![
            Message::User {
                content: vec![
                    Block::text("see"),
                    Block::image("image/png", "iVBO"),
                    Block::text("  "),
                ],
                timestamp: 0,
                extra: Map::new(),
            },
            assistant(
                vec![
                    Block::Thinking {
                        thinking: "plan".into(),
                        signature: Some("sig".into()),
                        redacted: false,
                        extra: Map::new(),
                    },
                    Block::Thinking {
                        thinking: "[Reasoning redacted]".into(),
                        signature: Some("AQID".into()),
                        redacted: true,
                        extra: Map::new(),
                    },
                    Block::text(""),
                    Block::tool_call("tool:1", "read", json!({"path": "a", "": 1})),
                    Block::tool_call("tool:2", "read", json!({})),
                ],
                "amazon-bedrock",
                same,
            ),
            Message::ToolResult {
                tool_call_id: "tool:1".into(),
                tool_name: "read".into(),
                content: vec![Block::text("ok")],
                is_error: false,
                timestamp: 0,
                extra: Map::new(),
            },
        ];
        let body = build_body(&env(&req)).unwrap();
        assert_eq!(
            body,
            json!({
                "messages": [
                    {"role": "user", "content": [
                        {"text": "see"},
                        {"image": {"format": "png", "source": {"bytes": "iVBO"}}}
                    ]},
                    {"role": "assistant", "content": [
                        {"reasoningContent": {"reasoningText": {"text": "plan", "signature": "sig"}}},
                        {"reasoningContent": {"redactedContent": "AQID"}},
                        {"toolUse": {"toolUseId": "tool:1", "name": "read", "input": {"path": "a"}}},
                        {"toolUse": {"toolUseId": "tool:2", "name": "read", "input": {}}}
                    ]},
                    {"role": "user", "content": [
                        {"toolResult": {"toolUseId": "tool:1", "content": [{"text": "ok"}], "status": "success"}},
                        {"toolResult": {"toolUseId": "tool:2", "content": [{"text": "No result provided"}], "status": "error"}},
                        {"cachePoint": {"type": "default", "ttl": "1h"}}
                    ]}
                ],
                "system": [{"text": "Be brief."}, {"cachePoint": {"type": "default", "ttl": "1h"}}],
                "inferenceConfig": {"maxTokens": 12000, "temperature": 0.2},
                "toolConfig": {"tools": [{"toolSpec": {"name": "read", "description": "Read",
                    "inputSchema": {"json": {"type": "object"}}}}], "toolChoice": {"auto": {}}},
                "additionalModelRequestFields": {
                    "thinking": {"type": "enabled", "budget_tokens": 4000, "display": "summarized"},
                    "anthropic_beta": ["interleaved-thinking-2025-05-14"]
                },
                "requestMetadata": {"team": "a"}
            })
        );
    }

    #[test]
    fn adaptive_thinking_govcloud_and_cross_model_replay() {
        let mut req = request(model("us.anthropic.claude-opus-4-7-v1", json!({})));
        req.thinking = ThinkingLevel::Xhigh;
        let fields = additional_fields(&env(&req), None).unwrap();
        assert_eq!(
            fields,
            json!({
                "thinking": {"type": "adaptive", "display": "summarized",
                    "block_binding": {"prefix_mismatch_behavior": "drop_block"}},
                "output_config": {"effort": "xhigh"},
                "anthropic_beta": ["thinking-binding-controls-2026-08-01"]
            })
        );
        req.env.insert("AWS_REGION".into(), "us-gov-west-1".into());
        assert_eq!(
            additional_fields(&env(&req), None).unwrap(),
            json!({"thinking": {"type": "adaptive"}, "output_config": {"effort": "xhigh"}})
        );
        let mut req = request(model("anthropic.claude-sonnet-4-6", json!({})));
        req.thinking = ThinkingLevel::Xhigh;
        assert_eq!(
            additional_fields(&env(&req), None).unwrap()["output_config"],
            json!({"effort": "high"})
        );
        // Non-Claude models: no thinking fields, no signatures, no cache points.
        let mut req = request(model("openai.gpt-oss-120b-1:0", json!({})));
        req.thinking = ThinkingLevel::High;
        req.messages = vec![
            Message::user_text("q", 0),
            assistant(
                vec![
                    Block::Thinking {
                        thinking: "t".into(),
                        signature: Some("s".into()),
                        redacted: false,
                        extra: Map::new(),
                    },
                    Block::tool_call("call|x", "read", json!({})),
                ],
                "openai",
                "gpt",
            ),
        ];
        let body = build_body(&env(&req)).unwrap();
        assert!(body.get("additionalModelRequestFields").is_none());
        assert_eq!(body["system"], json!([{"text": "Be brief."}]));
        assert_eq!(
            body["messages"][1]["content"],
            json!([{"text": "t"}, {"toolUse": {"toolUseId": "call_x", "name": "read", "input": {}}}])
        );
        assert_eq!(
            body["messages"][2]["content"][0]["toolResult"]["toolUseId"],
            "call_x"
        );
        // Unknown image types fail the request like the reference.
        req.messages = vec![Message::User {
            content: vec![Block::image("image/tiff", "x")],
            timestamp: 0,
            extra: Map::new(),
        }];
        assert_eq!(
            build_body(&env(&req)).unwrap_err(),
            "Unknown image type: image/tiff"
        );
    }

    fn golden_stream() -> Vec<u8> {
        let mut s = Vec::new();
        s.extend(encode_event("messageStart", &json!({"role": "assistant"})));
        s.extend(encode_event(
            "contentBlockDelta",
            &json!({"contentBlockIndex": 0, "delta": {"reasoningContent": {"text": "Think "}}}),
        ));
        s.extend(encode_event(
            "contentBlockDelta",
            &json!({"contentBlockIndex": 0, "delta": {"reasoningContent": {"text": "hard"}}}),
        ));
        s.extend(encode_event(
            "contentBlockDelta",
            &json!({"contentBlockIndex": 0, "delta": {"reasoningContent": {"signature": "SIG"}}}),
        ));
        s.extend(encode_event(
            "contentBlockStop",
            &json!({"contentBlockIndex": 0}),
        ));
        s.extend(encode_event(
            "contentBlockDelta",
            &json!({"contentBlockIndex": 1, "delta": {"text": "Hé "}}),
        ));
        s.extend(encode_event(
            "contentBlockDelta",
            &json!({"contentBlockIndex": 1, "delta": {"text": "😀"}}),
        ));
        s.extend(encode_event(
            "contentBlockStop",
            &json!({"contentBlockIndex": 1}),
        ));
        s.extend(encode_event("contentBlockStart", &json!({"contentBlockIndex": 2, "start": {"toolUse": {"toolUseId": "tu_1", "name": "read"}}})));
        s.extend(encode_event(
            "contentBlockDelta",
            &json!({"contentBlockIndex": 2, "delta": {"toolUse": {"input": "{\"path\":"}}}),
        ));
        s.extend(encode_event(
            "contentBlockDelta",
            &json!({"contentBlockIndex": 2, "delta": {"toolUse": {"input": "\"a.txt\"}"}}}),
        ));
        s.extend(encode_event(
            "contentBlockStop",
            &json!({"contentBlockIndex": 2}),
        ));
        s.extend(encode_event("contentBlockDelta", &json!({"contentBlockIndex": 3, "delta": {"reasoningContent": {"redactedContent": "AQI="}}})));
        s.extend(encode_event("contentBlockDelta", &json!({"contentBlockIndex": 3, "delta": {"reasoningContent": {"redactedContent": "Aw=="}}})));
        s.extend(encode_event(
            "contentBlockStop",
            &json!({"contentBlockIndex": 3}),
        ));
        s.extend(encode_event(
            "messageStop",
            &json!({"stopReason": "tool_use"}),
        ));
        s.extend(encode_event(
            "metadata",
            &json!({"usage": {"inputTokens": 100, "outputTokens": 50,
            "cacheReadInputTokens": 1000, "cacheWriteInputTokens": 200, "totalTokens": 1350,
            "cacheDetails": [{"ttl": "1h", "inputTokens": 150}, {"ttl": "5m", "inputTokens": 50}]},
            "metrics": {"latencyMs": 10}}),
        ));
        s
    }

    #[tokio::test]
    async fn golden_exchange_signed_and_fragmented() {
        let bytes = golden_stream();
        let server = MockServer::start(move |_, _| eventstream(bytes.clone(), 5)).await;
        let mut req = request(model(
            "anthropic.claude-sonnet-4-5-20250929-v1:0",
            json!({}),
        ));
        req.base_url = Some(server.url.clone());
        req.env.insert("AWS_SESSION_TOKEN".into(), "SESSION".into());
        req.headers.insert("X-Custom".into(), Some("v".into()));
        req.headers
            .insert("X-Amz-Date".into(), Some("forged".into()));
        let (msg, events) = exchange(&req, CancellationToken::new()).await;

        let r = &server.requests()[0];
        assert_eq!(r.method, "POST");
        assert_eq!(
            r.path,
            "/model/anthropic.claude-sonnet-4-5-20250929-v1%3A0/converse-stream"
        );
        let auth = r.header("authorization").unwrap();
        assert!(
            auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIDTEST/"),
            "{auth}"
        );
        assert!(auth.contains("/us-west-2/bedrock/aws4_request, SignedHeaders=content-type;host;x-amz-content-sha256;x-amz-date;x-amz-security-token;x-custom, Signature="), "{auth}");
        assert_eq!(r.header("x-amz-security-token"), Some("SESSION"));
        assert_ne!(r.header("x-amz-date"), Some("forged"));
        assert_eq!(
            r.header("x-amz-content-sha256").unwrap(),
            aws_auth::sha256_hex(&r.body)
        );
        assert_eq!(
            r.json()["messages"][0]["content"][1],
            json!({"cachePoint": {"type": "default"}})
        );

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
        assert_eq!(content.len(), 4);
        assert!(
            matches!(&content[0], Block::Thinking { thinking, signature: Some(s), redacted: false, .. } if thinking == "Think hard" && s == "SIG")
        );
        assert_eq!(content[1], Block::text("Hé 😀"));
        assert!(
            matches!(&content[2], Block::ToolCall { id, arguments, .. } if id == "tu_1" && *arguments == json!({"path": "a.txt"}))
        );
        assert!(
            matches!(&content[3], Block::Thinking { redacted: true, signature: Some(s), .. } if s == "AQID")
        );
        assert_eq!(
            (
                usage.input,
                usage.output,
                usage.cache_read,
                usage.cache_write,
                usage.total_tokens
            ),
            (100, 50, 1000, 200, 1350)
        );
        assert_eq!(usage.cache_write_1h, Some(150));
        assert_eq!(usage.reasoning, None);
        // 150 one-hour cache-write tokens cost 2x input; 50 five-minute ones the cache-write rate.
        let expected = (50.0 * 3.75 + 150.0 * 6.0) / 1e6;
        assert!(
            (usage.cost.cache_write - expected).abs() < 1e-12,
            "{}",
            usage.cost.cache_write
        );
        assert_eq!(extra["rawStopReason"], json!("tool_use"));
        assert!(events.len() >= 5);
    }

    #[tokio::test]
    async fn bearer_token_and_skip_auth() {
        let bytes = {
            let mut s = encode_event("messageStart", &json!({"role": "assistant"}));
            s.extend(encode_event(
                "contentBlockDelta",
                &json!({"contentBlockIndex": 0, "delta": {"text": "ok"}}),
            ));
            s.extend(encode_event(
                "messageStop",
                &json!({"stopReason": "end_turn"}),
            ));
            s
        };
        let b2 = bytes.clone();
        let server = MockServer::start(move |_, _| eventstream(b2.clone(), 64)).await;
        let mut req = request(model("amazon.nova-pro-v1:0", json!({"reasoning": false})));
        req.base_url = Some(server.url.clone());
        req.env
            .insert("AWS_BEARER_TOKEN_BEDROCK".into(), "brr".into());
        let (msg, _) = exchange(&req, CancellationToken::new()).await;
        assert!(
            matches!(
                &msg,
                Message::Assistant {
                    stop_reason: StopReason::Stop,
                    ..
                }
            ),
            "{msg:?}"
        );
        let r = &server.requests()[0];
        assert_eq!(r.header("authorization"), Some("Bearer brr"));
        assert!(r.header("x-amz-date").is_none());
        // The request credential beats the environment token.
        req.credential = Credential::ApiKey("stored".into());
        exchange(&req, CancellationToken::new()).await;
        assert_eq!(
            server.requests()[1].header("authorization"),
            Some("Bearer stored")
        );
        // Skip-auth signs with dummy credentials and ignores the bearer token.
        req.env.insert("AWS_BEDROCK_SKIP_AUTH".into(), "1".into());
        exchange(&req, CancellationToken::new()).await;
        let auth = server.requests()[2]
            .header("authorization")
            .unwrap()
            .to_owned();
        assert!(
            auth.starts_with("AWS4-HMAC-SHA256 Credential=dummy-access-key/"),
            "{auth}"
        );
    }

    #[tokio::test]
    async fn http_errors_keep_reference_prefixes_and_retry_throttling() {
        let server = MockServer::start(|_, n| {
            if n < 2 {
                MockResponse::json(
                    429,
                    &json!({"message": "Too many requests, please wait before trying again."}),
                )
                .with_header(
                    "x-amzn-errortype",
                    "ThrottlingException:http://internal.amazon.com/coral/com.amazon.bedrock/",
                )
                .with_header("x-amzn-requestid", "rid-1")
            } else {
                MockResponse::json(
                    400,
                    &json!({"message": "Input is too long for requested model."}),
                )
                .with_header(
                    "x-amzn-errortype",
                    "ValidationException:http://internal.amazon.com/coral/com.amazon.bedrock/",
                )
            }
        })
        .await;
        let mut req = request(model(
            "anthropic.claude-sonnet-4-5-20250929-v1:0",
            json!({}),
        ));
        req.base_url = Some(server.url.clone());
        let (msg, _) = exchange(&req, CancellationToken::new()).await;
        assert_eq!(
            server.requests().len(),
            3,
            "two throttled attempts are retried"
        );
        let Message::Assistant {
            error_message,
            extra,
            ..
        } = &msg
        else {
            panic!()
        };
        assert_eq!(
            error_message.as_deref(),
            Some("Validation error: Input is too long for requested model.")
        );
        assert!(crate::provider::errors::is_context_overflow(&msg, 200_000));
        assert_eq!(
            extra["diagnostics"][0]["details"],
            json!({"status": 400, "errorCode": "ValidationException"})
        );

        let server = MockServer::start(|_, _| {
            MockResponse::json(429, &json!({"message": "Too many tokens, please wait."}))
                .with_header("x-amzn-errortype", "ThrottlingException")
        })
        .await;
        req.base_url = Some(server.url.clone());
        let (msg, _) = exchange(&req, CancellationToken::new()).await;
        let Message::Assistant { error_message, .. } = &msg else {
            panic!()
        };
        assert_eq!(
            error_message.as_deref(),
            Some("Throttling error: Too many tokens, please wait.")
        );
        // The prefix keeps "too many tokens" throttling from reading as a context overflow.
        assert!(!crate::provider::errors::is_context_overflow(&msg, 200_000));
        assert_eq!(server.requests().len(), 3);

        let server = MockServer::start(|_, _| {
            let mut r = MockResponse::json(403, &json!({}));
            r.chunks = vec![b"<html>Forbidden</html>".to_vec()];
            r
        })
        .await;
        req.base_url = Some(server.url.clone());
        let (msg, _) = exchange(&req, CancellationToken::new()).await;
        assert!(
            matches!(&msg, Message::Assistant { error_message: Some(e), .. } if e == "403: <html>Forbidden</html>"),
            "{msg:?}"
        );

        let server = MockServer::start(|_, _| {
            MockResponse::json(400, &json!({"message": "The data retention mode 'default' is not available for this model."}))
                .with_header("x-amzn-errortype", "ValidationException")
        })
        .await;
        req.base_url = Some(server.url.clone());
        let (msg, _) = exchange(&req, CancellationToken::new()).await;
        assert!(
            matches!(&msg, Message::Assistant { error_message: Some(e), .. }
            if e.starts_with("Validation error: The data retention mode") && e.ends_with("for supported data retention modes."))
        );
    }

    #[tokio::test]
    async fn stream_exceptions_crc_failures_and_missing_stop() {
        let mut bytes = encode_event("messageStart", &json!({"role": "assistant"}));
        bytes.extend(encode_event(
            "contentBlockDelta",
            &json!({"contentBlockIndex": 0, "delta": {"text": "part"}}),
        ));
        bytes.extend(encode(
            &[
                (":message-type", "exception"),
                (":exception-type", "serviceUnavailableException"),
                (":content-type", "application/json"),
            ],
            br#"{"message":"Bedrock is unable to process your request."}"#,
        ));
        let server = MockServer::start(move |_, _| eventstream(bytes.clone(), 3)).await;
        let mut req = request(model(
            "anthropic.claude-sonnet-4-5-20250929-v1:0",
            json!({}),
        ));
        req.base_url = Some(server.url.clone());
        let (msg, _) = exchange(&req, CancellationToken::new()).await;
        let Message::Assistant {
            stop_reason,
            error_message,
            content,
            extra,
            ..
        } = &msg
        else {
            panic!()
        };
        assert_eq!(*stop_reason, StopReason::Error);
        assert_eq!(
            error_message.as_deref(),
            Some("Service unavailable: Bedrock is unable to process your request.")
        );
        assert_eq!(content, &vec![Block::text("part")]);
        assert_eq!(
            extra["diagnostics"][0]["details"],
            json!({"errorCode": "ServiceUnavailableException", "requestId": "req-123"})
        );
        assert!(crate::provider::errors::is_retryable(&msg));

        let mut corrupt = encode_event("messageStart", &json!({"role": "assistant"}));
        let n = corrupt.len();
        corrupt[n - 1] ^= 0xff;
        let server = MockServer::start(move |_, _| eventstream(corrupt.clone(), 7)).await;
        req.base_url = Some(server.url.clone());
        let (msg, _) = exchange(&req, CancellationToken::new()).await;
        assert!(
            matches!(&msg, Message::Assistant { error_message: Some(e), .. } if e.contains("message checksum")),
            "{msg:?}"
        );

        let unterminated = encode_event("messageStart", &json!({"role": "assistant"}));
        let server = MockServer::start(move |_, _| eventstream(unterminated.clone(), 7)).await;
        req.base_url = Some(server.url.clone());
        let (msg, _) = exchange(&req, CancellationToken::new()).await;
        assert!(
            matches!(&msg, Message::Assistant { error_message: Some(e), .. } if e == "Bedrock stream ended without a stop reason")
        );

        let mut guard = encode_event("messageStart", &json!({"role": "assistant"}));
        guard.extend(encode_event(
            "messageStop",
            &json!({"stopReason": "guardrail_intervened"}),
        ));
        let server = MockServer::start(move |_, _| eventstream(guard.clone(), 7)).await;
        req.base_url = Some(server.url.clone());
        let (msg, _) = exchange(&req, CancellationToken::new()).await;
        assert!(
            matches!(&msg, Message::Assistant { error_message: Some(e), .. } if e == "Provider stopped with: guardrail_intervened")
        );
    }

    #[tokio::test]
    async fn cancellation_mid_stream_is_aborted() {
        let mut bytes = encode_event("messageStart", &json!({"role": "assistant"}));
        bytes.extend(encode_event(
            "contentBlockDelta",
            &json!({"contentBlockIndex": 0, "delta": {"text": "partial"}}),
        ));
        let server = MockServer::start(move |_, _| {
            let mut r = eventstream(bytes.clone(), 1000);
            r.hang = true;
            r
        })
        .await;
        let mut req = request(model(
            "anthropic.claude-sonnet-4-5-20250929-v1:0",
            json!({}),
        ));
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

        // A stalled stream hits the idle timeout.
        req.timeout = Duration::from_millis(200);
        let (msg, _) = exchange(&req, CancellationToken::new()).await;
        assert!(
            matches!(&msg, Message::Assistant { error_message: Some(e), .. } if e.contains("idle timeout"))
        );
        assert!(crate::provider::errors::is_retryable(&msg));
    }

    #[tokio::test]
    async fn missing_credentials_fail_clearly() {
        let mut req = request(model(
            "anthropic.claude-sonnet-4-5-20250929-v1:0",
            json!({}),
        ));
        req.env = HashMap::from([
            ("AWS_REGION".into(), "us-east-1".into()),
            ("AWS_EC2_METADATA_DISABLED".into(), "true".into()),
            ("HOME".into(), "/nonexistent-pipkin-test".into()),
        ]);
        let (msg, _) = exchange(&req, CancellationToken::new()).await;
        assert!(
            matches!(&msg, Message::Assistant { error_message: Some(e), .. } if e == "Could not load credentials from any providers"),
            "{msg:?}"
        );
    }

    /// Reference `clampMaxTokensToContext` (`simple-options.ts:18-22`) through
    /// `buildBaseOptions`: 10000 window - 1100 estimated - 4096 safety = 4804.
    fn crowd_context(req: &mut ProviderRequest) {
        req.system = "x".repeat(4000);
        req.messages = vec![Message::user_text("y".repeat(400), 1)];
    }

    #[test]
    fn max_tokens_and_budgets_are_clamped_to_the_context_window() {
        // Plain models: the base cap (`buildBaseOptions`).
        let mut req = request(model(
            "openai.gpt-oss-120b-1:0",
            json!({"contextWindow": 10000, "maxTokens": 32000}),
        ));
        req.max_tokens = Some(32000);
        crowd_context(&mut req);
        let body = build_body(&env(&req)).unwrap();
        assert_eq!(body["inferenceConfig"]["maxTokens"], json!(4804));

        // Budget-based Claude: adjust the clamped base (4804 + 8192), clamp again, and leave
        // the answer 1024 tokens (`bedrock-converse-stream.ts:555-571`).
        let mut req = request(model(
            "us.anthropic.claude-3-7-sonnet-20250219-v1:0",
            json!({"contextWindow": 10000}),
        ));
        req.max_tokens = None;
        req.thinking = ThinkingLevel::Medium;
        crowd_context(&mut req);
        let body = build_body(&env(&req)).unwrap();
        assert_eq!(body["inferenceConfig"]["maxTokens"], json!(4804));
        assert_eq!(
            body["additionalModelRequestFields"]["thinking"]["budget_tokens"],
            json!(3780)
        );
    }
    #[test]
    fn constrained_tool_metadata_reaches_bedrock_and_rejects_unsupported_require() {
        let mut req = request(serde_json::from_value(json!({"id":"claude","provider":"amazon-bedrock","api":"bedrock-converse-stream","compat":{"supportsStrictMode":true}})).unwrap());
        let mut tool = super::super::constrained_tests::grammar_tool();
        tool.constrained_sampling = Some(json!({"type":"json_schema","strict":"require"}));
        req.tools = vec![tool];
        let body = tool_config(&req).unwrap().unwrap();
        assert_eq!(body["tools"][0]["toolSpec"]["strict"], true);
        assert_eq!(
            body["tools"][0]["toolSpec"]["inputSchema"]["json"]["additionalProperties"],
            false
        );
        req.model
            .compat
            .insert("supportsStrictMode".into(), json!(false));
        assert!(
            tool_config(&req)
                .unwrap_err()
                .contains("strict tools are unsupported")
        );
    }
}
