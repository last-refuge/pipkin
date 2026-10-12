//! `openai-codex-responses`: the ChatGPT subscription ("Codex") Responses backend.
//!
//! Port of the reference `packages/ai/src/api/openai-codex-responses.ts` (pinned rev
//! d2a311097cbc). The SSE transport (zstd-compressed body, the adapter's own retry loop) and the
//! WebSocket transport (per-session connection reuse, `previous_response_id` continuation,
//! permanent per-session SSE fallback) are both ported.
//!
//! # Credential contract
//!
//! The adapter expects `Credential::OAuth { access, metadata }`:
//! - `access`: the ChatGPT OAuth access token (a JWT), sent as `Authorization: Bearer`.
//! - `metadata["accountId"]` (string): the ChatGPT account id, sent as `chatgpt-account-id`.
//!   When it is absent the adapter derives it from the JWT claim
//!   `"https://api.openai.com/auth".chatgpt_account_id`, exactly like the reference
//!   `extractAccountId`. `Credential::ApiKey`/`Bearer` holding the access token are accepted
//!   too (account id from the JWT); no credential is an error.
//!
//! # Request options (`ProviderRequest::options`, reference `OpenAICodexResponsesOptions`)
//!
//! | key | default | meaning |
//! | --- | --- | --- |
//! | `transport` | `"auto"` | `sse`, `websocket`, `websocket-cached` or `auto` |
//! | `maxRetries` | `0` | SSE retry attempts (reference `maxRetries`) |
//! | `maxRetryDelayMs` | `60000` | largest honoured `retry-after`; `0` = no cap |
//! | `websocketConnectTimeoutMs` | `15000` | `0` disables |
//! | `reasoningEffort` | from `thinking` | explicit effort (`none`, `minimal`, ... `max`) |
//! | `reasoningSummary` | `"auto"` | `reasoning.summary` |
//! | `textVerbosity` | `"low"` | `text.verbosity` |
//! | `serviceTier` | none | `service_tier`; also prices the response |
//! | `toolChoice` | `"auto"` | `tool_choice` |
//! | `temperature` | none | `temperature` |
//!
//! `ProviderRequest::timeout` is the SSE response-header timeout and the WebSocket per-frame
//! idle timeout (reference `timeoutMs`); it also bounds SSE body idle gaps here (the reference
//! relies on the global undici `bodyTimeout` for that). A zero timeout disables it.
//!
//! The responses-input conversion and the Responses stream processor are the shared ones in
//! [`super::openai_responses_shared`]; `openai_codex/events.rs` adds the Codex event mapping.

#[path = "openai_codex/events.rs"]
mod events;
#[path = "openai_codex/ws.rs"]
mod ws;

#[cfg(test)]
#[path = "openai_codex/tests.rs"]
mod tests;

use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use pipkin_engine_protocol::Message;
use regex::RegexBuilder;
use reqwest::StatusCode;
use reqwest::header::HeaderMap;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use self::events::{ResponsesState, Step};
pub use self::ws::WebSocketDebugStats;
use super::common::{self, Headers, js_truthy, pi_user_agent};
use super::openai_completions::clamp_prompt_cache_key;
use super::openai_responses_shared::{
    apply_service_tier_pricing, convert_request_messages, convert_tools, service_tier_multiplier,
};
use crate::provider::http::{self, ReadError};
use crate::provider::proxy::ProxyConfig;
use crate::provider::{
    ApiAdapter, BoxFuture, CacheRetention, Credential, Emitter, ProviderRequest, StreamCtx,
};

/// The API kind this adapter serves.
pub const API: &str = "openai-codex-responses";
/// Providers whose `call_id|item_id` tool-call ids are kept (reference `CODEX_TOOL_CALL_PROVIDERS`).
const CODEX_TOOL_CALL_PROVIDERS: [&str; 3] = ["openai", "openai-codex", "opencode"];
/// Reference `DEFAULT_CODEX_BASE_URL`.
const DEFAULT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api";
/// Reference `JWT_CLAIM_PATH`.
const JWT_CLAIM_PATH: &str = "https://api.openai.com/auth";
/// Metadata key on `Credential::OAuth` carrying the ChatGPT account id.
pub const ACCOUNT_ID_METADATA_KEY: &str = "accountId";
const DEFAULT_MAX_RETRIES: u32 = 0;
const BASE_DELAY_MS: u64 = 1000;
const DEFAULT_MAX_RETRY_DELAY_MS: u64 = 60_000;
const DEFAULT_WEBSOCKET_CONNECT_TIMEOUT_MS: u64 = 15_000;
/// Reference `REQUEST_COMPRESSION_ZSTD_LEVEL`.
const REQUEST_COMPRESSION_ZSTD_LEVEL: i32 = 3;
const WEBSOCKET_CONNECTION_LIMIT_REACHED_CODE: &str = "websocket_connection_limit_reached";
const PREVIOUS_RESPONSE_NOT_FOUND_CODE: &str = "previous_response_not_found";
const ABORTED: &str = "Request was aborted";

pub fn adapter() -> Arc<dyn ApiAdapter> {
    Arc::new(OpenAICodexResponses::new())
}

/// Error classes the transport logic distinguishes (reference error classes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FailureKind {
    Aborted,
    /// `CodexApiError` (an `error` / `response.failed` event).
    Api,
    /// `CodexProtocolError` (invalid JSON frame).
    Protocol,
    /// `WebSocketCloseError` with its close code.
    WsClose(Option<u16>),
    /// `RetryDelayExceededError`.
    RetryDelay,
    /// Any other `Error` (transport, timeouts, processor checks, HTTP errors).
    Error,
}

#[derive(Clone, Debug)]
pub(crate) struct Failure {
    pub(crate) kind: FailureKind,
    pub(crate) message: String,
    pub(crate) code: Option<String>,
}

impl Failure {
    pub(crate) fn error(message: impl Into<String>) -> Failure {
        Failure {
            kind: FailureKind::Error,
            message: message.into(),
            code: None,
        }
    }

    pub(crate) fn aborted() -> Failure {
        Failure {
            kind: FailureKind::Aborted,
            message: ABORTED.into(),
            code: None,
        }
    }

    fn name(&self) -> &'static str {
        match self.kind {
            FailureKind::Api => "CodexApiError",
            FailureKind::Protocol => "CodexProtocolError",
            FailureKind::WsClose(_) => "WebSocketCloseError",
            _ => "Error",
        }
    }

    /// Reference `isCodexNonTransportError`.
    fn non_transport(&self) -> bool {
        matches!(self.kind, FailureKind::Api | FailureKind::Protocol)
    }

    fn api_code(&self, code: &str) -> bool {
        self.kind == FailureKind::Api && self.code.as_deref() == Some(code)
    }
}

/// The adapter. Holds the process-lifetime WebSocket pool and SSE-fallback session set.
#[derive(Default)]
pub struct OpenAICodexResponses {
    ws: ws::Shared,
}

impl ApiAdapter for OpenAICodexResponses {
    fn api(&self) -> &'static str {
        API
    }

    fn stream<'a>(&'a self, ctx: StreamCtx<'a>) -> BoxFuture<'a, Message> {
        Box::pin(self.run(ctx))
    }

    fn close_sessions(&self, session_id: Option<&str>) {
        self.close_websocket_sessions(session_id);
    }
}

/// Parsed `ProviderRequest::options`.
#[derive(Clone, Debug)]
struct CodexOptions {
    transport: String,
    max_retries: u32,
    max_retry_delay_ms: u64,
    websocket_connect_timeout: Option<Duration>,
    reasoning_effort: Option<String>,
    reasoning_summary: Option<String>,
    text_verbosity: Option<String>,
    service_tier: Option<String>,
    tool_choice: Option<Value>,
    temperature: Option<Value>,
}

fn nonempty_option(options: &Map<String, Value>, key: &str) -> Option<String> {
    options
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Reference `normalizeTimeoutMs` for numeric options.
fn opt_ms(options: &Map<String, Value>, key: &str) -> Result<Option<u64>, String> {
    match options.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => match n.as_f64() {
            Some(f) if f.is_finite() && f >= 0.0 => Ok(Some(f.floor() as u64)),
            _ => Err(format!("Invalid timeoutMs: {n}")),
        },
        Some(other) => Err(format!("Invalid timeoutMs: {other}")),
    }
}

impl CodexOptions {
    fn parse(options: &Map<String, Value>) -> Result<CodexOptions, String> {
        let count = |key: &str| {
            options
                .get(key)
                .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f.max(0.0) as u64)))
        };
        let connect_ms = opt_ms(options, "websocketConnectTimeoutMs")?
            .unwrap_or(DEFAULT_WEBSOCKET_CONNECT_TIMEOUT_MS);
        Ok(CodexOptions {
            transport: nonempty_option(options, "transport").unwrap_or_else(|| "auto".into()),
            max_retries: count("maxRetries")
                .map(|n| n.min(u64::from(u32::MAX)) as u32)
                .unwrap_or(DEFAULT_MAX_RETRIES),
            max_retry_delay_ms: count("maxRetryDelayMs").unwrap_or(DEFAULT_MAX_RETRY_DELAY_MS),
            websocket_connect_timeout: (connect_ms > 0).then(|| Duration::from_millis(connect_ms)),
            reasoning_effort: nonempty_option(options, "reasoningEffort"),
            reasoning_summary: nonempty_option(options, "reasoningSummary"),
            text_verbosity: nonempty_option(options, "textVerbosity"),
            service_tier: nonempty_option(options, "serviceTier"),
            tool_choice: options.get("toolChoice").filter(|v| !v.is_null()).cloned(),
            temperature: options
                .get("temperature")
                .filter(|v| v.is_number())
                .cloned(),
        })
    }
}

/// Reference `buildBaseCodexHeaders`.
fn base_headers(req: &ProviderRequest, account_id: &str, token: &str) -> Headers {
    let mut headers = Headers::default();
    headers.merge(&req.model.headers);
    headers.merge_optional(&req.headers);
    headers.set("Authorization", format!("Bearer {token}"));
    headers.set("chatgpt-account-id", account_id);
    headers.set("originator", "pi");
    headers.set("User-Agent", pi_user_agent());
    headers
}

/// Reference `buildSSEHeaders`.
fn sse_headers(
    req: &ProviderRequest,
    account_id: &str,
    token: &str,
    session: Option<&str>,
) -> Headers {
    let mut headers = base_headers(req, account_id, token);
    headers.set("OpenAI-Beta", "responses=experimental");
    headers.set("accept", "text/event-stream");
    headers.set("content-type", "application/json");
    if let Some(session) = session {
        headers.set("session-id", session);
        headers.set("x-client-request-id", session);
    }
    headers
}

/// Reference `buildWebSocketHeaders`. (The reference's later `delete wsHeaders["OpenAI-Beta"]`
/// in `connectWebSocket` runs on lowercased keys and is a no-op, so the beta header is sent.)
fn websocket_headers(
    req: &ProviderRequest,
    account_id: &str,
    token: &str,
    request_id: &str,
) -> Headers {
    let mut headers = base_headers(req, account_id, token);
    headers.remove("accept");
    headers.remove("content-type");
    headers.remove("OpenAI-Beta");
    headers.set("OpenAI-Beta", ws::OPENAI_BETA_RESPONSES_WEBSOCKETS);
    headers.set("x-client-request-id", request_id);
    headers.set("session-id", request_id);
    headers
}

/// Reference `resolveCodexUrl`.
fn resolve_codex_url(base_url: &str) -> String {
    let raw = if base_url.trim().is_empty() {
        DEFAULT_CODEX_BASE_URL
    } else {
        base_url
    };
    let normalized = raw.trim_end_matches('/');
    if normalized.ends_with("/codex/responses") {
        normalized.to_owned()
    } else if normalized.ends_with("/codex") {
        format!("{normalized}/responses")
    } else {
        format!("{normalized}/codex/responses")
    }
}

/// Reference `resolveCodexWebSocketUrl`.
fn resolve_codex_websocket_url(base_url: &str) -> String {
    let url = resolve_codex_url(base_url);
    let normalized = url::Url::parse(&url).map_or(url, |u| u.to_string());
    if let Some(rest) = normalized.strip_prefix("https:") {
        format!("wss:{rest}")
    } else if let Some(rest) = normalized.strip_prefix("http:") {
        format!("ws:{rest}")
    } else {
        normalized
    }
}

/// Reference `extractAccountId` (decoding is lenient about base64 alphabet and padding).
fn account_id_from_jwt(token: &str) -> Option<String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let payload = parts[1].trim_end_matches('=');
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(payload))
        .ok()?;
    let json: Value = serde_json::from_slice(&bytes).ok()?;
    json.get(JWT_CLAIM_PATH)?
        .get("chatgpt_account_id")?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// The access token and account id for the request.
fn resolve_auth(req: &ProviderRequest) -> Result<(String, String), String> {
    let (token, metadata) = match &req.credential {
        Credential::OAuth { access, metadata } => (access.clone(), Some(metadata)),
        Credential::ApiKey(k) | Credential::Bearer(k) => (k.clone(), None),
        Credential::None | Credential::Ambient(_) => (String::new(), None),
    };
    if token.is_empty() {
        return Err(format!("No API key for provider: {}", req.model.provider));
    }
    let account = metadata
        .and_then(|m| m.get(ACCOUNT_ID_METADATA_KEY))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| account_id_from_jwt(&token))
        .ok_or_else(|| "Failed to extract accountId from token".to_owned())?;
    Ok((token, account))
}

/// Reference `clampOpenAIPromptCacheKey` (64 code points).
/// A UUIDv7 (reference `uuidv7`, used as the WebSocket request id without a session).
fn uuid_v7() -> String {
    let mut b = [0u8; 16];
    let _ = getrandom::getrandom(&mut b);
    let ms = (crate::util::now_ms().max(0) as u64) & 0xffff_ffff_ffff;
    b[..6].copy_from_slice(&ms.to_be_bytes()[2..]);
    b[6] = (b[6] & 0x0f) | 0x70;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// The effort requested for this turn (reference `streamSimple`: `off` means no effort).
fn reasoning_effort(req: &ProviderRequest, opts: &CodexOptions) -> Option<String> {
    opts.reasoning_effort
        .clone()
        .or_else(|| common::reasoning_effort(req).map(|l| l.as_str().to_owned()))
}

/// Reference `buildRequestBody` (key order kept).
fn build_request_body(
    req: &ProviderRequest,
    opts: &CodexOptions,
    prompt_cache_key: Option<&str>,
) -> Result<Value, String> {
    let resolved =
        req.resolve_systems(req.model.compat_bool("supportsMidConvoSystemMessages") == Some(true));
    let req = resolved.as_ref();
    let model = &req.model;
    let input = convert_request_messages(req, false, &CODEX_TOOL_CALL_PROVIDERS)?;
    let mut body = Map::new();
    body.insert("model".into(), Value::String(model.id.clone()));
    body.insert("store".into(), Value::Bool(false));
    body.insert("stream".into(), Value::Bool(true));
    body.insert(
        "instructions".into(),
        Value::String(if req.system.is_empty() {
            "You are a helpful assistant.".into()
        } else {
            req.system.clone()
        }),
    );
    body.insert("input".into(), Value::Array(input));
    body.insert(
        "text".into(),
        json!({"verbosity": opts.text_verbosity.clone().unwrap_or_else(|| "low".into())}),
    );
    body.insert("include".into(), json!(["reasoning.encrypted_content"]));
    if let Some(key) = prompt_cache_key {
        body.insert("prompt_cache_key".into(), Value::String(key.to_owned()));
    }
    body.insert(
        "tool_choice".into(),
        opts.tool_choice
            .clone()
            .unwrap_or_else(|| Value::String("auto".into())),
    );
    body.insert("parallel_tool_calls".into(), Value::Bool(true));
    if let Some(t) = &opts.temperature {
        body.insert("temperature".into(), t.clone());
    }
    if let Some(tier) = &opts.service_tier {
        body.insert("service_tier".into(), Value::String(tier.clone()));
    }
    if !req.tools.is_empty() {
        let strict = model.compat_bool("supportsStrictMode").unwrap_or(true);
        body.insert(
            "tools".into(),
            Value::Array(convert_tools(req, &req.tools, strict, None)?),
        );
    }
    let map = &model.thinking_level_map;
    if let Some(level) = reasoning_effort(req, opts) {
        let effort = if level == "none" {
            match map.get("off") {
                None => Some("none".to_owned()),
                Some(v) => v.clone(),
            }
        } else {
            // `map[level] ?? level`: a null mapping also falls back to the level.
            Some(map.get(&level).cloned().flatten().unwrap_or(level))
        };
        if let Some(effort) = effort {
            body.insert(
                "reasoning".into(),
                json!({
                    "effort": effort,
                    "summary": opts.reasoning_summary.clone().unwrap_or_else(|| "auto".into()),
                }),
            );
        }
    } else if model.reasoning && map.get("off") != Some(&None) {
        let effort = map
            .get("off")
            .cloned()
            .flatten()
            .unwrap_or_else(|| "none".into());
        body.insert("reasoning".into(), json!({ "effort": effort }));
    }
    Ok(Value::Object(body))
}

/// Reference `isTerminalRateLimitError`.
fn is_terminal_rate_limit_error(text: &str) -> bool {
    RegexBuilder::new(
        "GoUsageLimitError|FreeUsageLimitError|Monthly usage limit reached|available balance|insufficient_quota|out of budget|quota exceeded|billing",
    )
    .case_insensitive(true)
    .build()
    .is_ok_and(|r| r.is_match(text))
}

/// Reference `isRetryableError`.
fn is_retryable_error(status: u16, text: &str) -> bool {
    if status == 429 && is_terminal_rate_limit_error(text) {
        return false;
    }
    if matches!(status, 429 | 500 | 502 | 503 | 504) {
        return true;
    }
    RegexBuilder::new(
        "rate.?limit|overloaded|service.?unavailable|upstream.?connect|connection.?refused",
    )
    .case_insensitive(true)
    .build()
    .is_ok_and(|r| r.is_match(text))
}

/// `Number(text)` for header values: trimmed, empty is 0, else a finite float.
fn js_number(text: &str) -> Option<f64> {
    let t = text.trim();
    if t.is_empty() {
        return Some(0.0);
    }
    t.parse::<f64>().ok().filter(|f| f.is_finite())
}

/// IMF-fixdate (`Sun, 06 Nov 1994 08:49:37 GMT`) to Unix milliseconds.
fn parse_http_date(text: &str) -> Option<i64> {
    let parts: Vec<&str> = text.split_whitespace().collect();
    if parts.len() != 6 || parts[5] != "GMT" {
        return None;
    }
    let day: i64 = parts[1].parse().ok()?;
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .iter()
    .position(|m| *m == parts[2])? as i64
        + 1;
    let year: i64 = parts[3].parse().ok()?;
    let hms: Vec<i64> = parts[4]
        .split(':')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    if hms.len() != 3 {
        return None;
    }
    // Days from civil (Howard Hinnant).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 86_400) + hms[0] * 3600 + hms[1] * 60 + hms[2]) * 1000)
}

/// Reference `getRetryAfterDelayMs`.
fn retry_after_delay_ms(headers: &HeaderMap) -> Option<u64> {
    if let Some(v) = headers.get("retry-after-ms").and_then(|v| v.to_str().ok())
        && let Some(ms) = js_number(v)
    {
        return Some(ms.max(0.0) as u64);
    }
    let raw = headers.get("retry-after").and_then(|v| v.to_str().ok())?;
    if raw.is_empty() {
        return None;
    }
    if let Some(secs) = js_number(raw) {
        return Some((secs * 1000.0).max(0.0) as u64);
    }
    parse_http_date(raw.trim()).map(|at| (at - crate::util::now_ms()).max(0) as u64)
}

/// Reference `validateRetryDelayMs`.
fn validate_retry_delay(delay_ms: u64, opts: &CodexOptions) -> Result<u64, Failure> {
    let max = opts.max_retry_delay_ms;
    if max > 0 && delay_ms > max {
        return Err(Failure {
            kind: FailureKind::RetryDelay,
            message: format!(
                "Server requested {}s retry delay (max: {}s)",
                delay_ms.div_ceil(1000),
                max.div_ceil(1000)
            ),
            code: None,
        });
    }
    Ok(delay_ms)
}

/// Reference `parseErrorResponse`: `(message, friendlyMessage)`.
fn parse_error_response(status: u16, raw: &str, status_text: &str) -> (String, Option<String>) {
    let mut message = if !raw.is_empty() {
        raw.to_owned()
    } else if !status_text.is_empty() {
        status_text.to_owned()
    } else {
        "Request failed".to_owned()
    };
    let mut friendly = None;
    if let Ok(parsed) = serde_json::from_str::<Value>(raw)
        && let Some(err) = parsed.get("error").filter(|e| js_truthy(e))
    {
        let field = |k: &str| err.get(k).and_then(Value::as_str).filter(|s| !s.is_empty());
        let code = field("code").or_else(|| field("type")).unwrap_or("");
        let limit = RegexBuilder::new("usage_limit_reached|usage_not_included|rate_limit_exceeded")
            .case_insensitive(true)
            .build()
            .is_ok_and(|r| r.is_match(code));
        if limit || status == 429 {
            let plan = field("plan_type")
                .map(|p| format!(" ({} plan)", p.to_lowercase()))
                .unwrap_or_default();
            let mins = err
                .get("resets_at")
                .and_then(Value::as_f64)
                .filter(|r| *r != 0.0)
                .map(|resets_at| {
                    let now = crate::util::now_ms() as f64;
                    ((resets_at * 1000.0 - now) / 60_000.0).round().max(0.0) as i64
                });
            let when = mins
                .map(|m| format!(" Try again in ~{m} min."))
                .unwrap_or_default();
            friendly = Some(
                format!("You have hit your ChatGPT usage limit{plan}.{when}")
                    .trim()
                    .to_owned(),
            );
        }
        message = field("message")
            .map(str::to_owned)
            .or_else(|| friendly.clone())
            .unwrap_or(message);
    }
    (message, friendly)
}

/// Abortable sleep (reference `sleep`).
async fn sleep(ms: u64, cancel: &CancellationToken) -> Result<(), Failure> {
    tokio::select! {
        _ = cancel.cancelled() => Err(Failure::aborted()),
        _ = tokio::time::sleep(Duration::from_millis(ms)) => Ok(()),
    }
}

fn backoff_ms(attempt: u32) -> u64 {
    BASE_DELAY_MS.saturating_mul(1u64.checked_shl(attempt).unwrap_or(u64::MAX))
}

/// One exchange's results before the final message is built.
struct Exchange {
    emitter: Emitter,
    state: ResponsesState,
    result: Result<(), Failure>,
}

impl OpenAICodexResponses {
    pub fn new() -> OpenAICodexResponses {
        OpenAICodexResponses::default()
    }

    /// Reference `getOpenAICodexWebSocketDebugStats`.
    pub fn websocket_debug_stats(&self, session_id: &str) -> Option<WebSocketDebugStats> {
        ws::debug_stats(&self.ws, session_id)
    }

    /// Reference `resetOpenAICodexWebSocketDebugStats` (also clears the SSE fallback).
    pub fn reset_websocket_debug_stats(&self, session_id: Option<&str>) {
        ws::reset_debug_stats(&self.ws, session_id);
    }

    /// Reference `closeOpenAICodexWebSocketSessions` (session resource cleanup).
    pub fn close_websocket_sessions(&self, session_id: Option<&str>) {
        ws::close_sessions(&self.ws, session_id);
    }

    async fn run(&self, ctx: StreamCtx<'_>) -> Message {
        let mut diagnostics = Vec::new();
        let Exchange {
            mut emitter,
            mut state,
            result,
        } = self.exchange(&ctx, &mut diagnostics).await;
        let outcome = result.and_then(|()| {
            if ctx.cancel.is_cancelled() {
                Err(Failure::aborted())
            } else {
                state.outcome(&mut emitter).map_err(Failure::error)
            }
        });
        if outcome.is_err() {
            state.settle(&mut emitter);
        }
        let mut message = match outcome {
            Ok(stop) => emitter.finish(ctx.req, stop, None),
            Err(failure) => ctx.failed(emitter, failure.message),
        };
        if let Message::Assistant { usage, extra, .. } = &mut message {
            if let Some(tier) = &state.priced_tier {
                apply_service_tier_pricing(
                    usage,
                    service_tier_multiplier(&ctx.req.model.id, tier.as_deref(), false),
                );
            }
            if let Some(end_turn) = state.end_turn {
                extra.insert("endTurn".into(), Value::Bool(end_turn));
            }
            if !diagnostics.is_empty() {
                extra.insert("diagnostics".into(), Value::Array(diagnostics));
            }
        }
        message
    }

    /// Reference `stream` body (lines 264-483) up to the final success/error decision.
    async fn exchange(&self, ctx: &StreamCtx<'_>, diagnostics: &mut Vec<Value>) -> Exchange {
        let req = ctx.req;
        let setup_failure = |message: String| Exchange {
            emitter: ctx.emitter(),
            state: ResponsesState::new(None),
            result: Err(Failure::error(message)),
        };
        let opts = match CodexOptions::parse(&req.options) {
            Ok(o) => o,
            Err(e) => return setup_failure(e),
        };
        let (token, account_id) = match resolve_auth(req) {
            Ok(a) => a,
            Err(e) => return setup_failure(e),
        };
        // The pool and fallback set key on the unclamped id; headers/body use the clamped one.
        let cache_session = (req.cache_retention != CacheRetention::None
            && !req.session_id.is_empty())
        .then_some(req.session_id.as_str());
        let codex_session = cache_session.and_then(clamp_prompt_cache_key);
        let body = match build_request_body(req, &opts, codex_session.as_deref()) {
            Ok(b) => b,
            Err(e) => return setup_failure(e),
        };
        let websocket_request_id = codex_session.clone().unwrap_or_else(uuid_v7);
        let sse = sse_headers(req, &account_id, &token, codex_session.as_deref());
        let websocket = websocket_headers(req, &account_id, &token, &websocket_request_id);
        let body_json = body.to_string();
        let timeout = (!req.timeout.is_zero()).then_some(req.timeout);
        let transport = opts.transport.as_str();

        let websocket_disabled =
            transport != "sse" && ws::is_fallback_active(&self.ws, cache_session);
        if websocket_disabled {
            ws::record_sse_fallback(&self.ws, cache_session);
        }
        if transport != "sse" && !websocket_disabled {
            let url = resolve_codex_websocket_url(req.base_url());
            // The proxy comes from the request env (the engine puts its proxy settings there;
            // a credential's own variables win), never from the process environment.
            let proxy = match url::Url::parse(&url) {
                Ok(target) => match ProxyConfig::from_map(&req.env).proxy_for(&target) {
                    Ok(p) => p,
                    Err(e) => return setup_failure(e),
                },
                Err(_) => None,
            };
            let mut retried_connection_limit = false;
            let mut retried_missing_continuation = false;
            loop {
                let mut emitter = ctx.emitter();
                let mut state = match ResponsesState::for_request(req, opts.service_tier.clone()) {
                    Ok(s) => s,
                    Err(m) => return setup_failure(m),
                };
                let result = ws::run(
                    &self.ws,
                    ws::WsRequest {
                        url: &url,
                        body: &body,
                        headers: websocket.pairs(),
                        request: req,
                        idle_timeout: timeout,
                        connect_timeout: opts.websocket_connect_timeout,
                        proxy: proxy.as_ref(),
                        session: cache_session,
                        account: &account_id,
                        transport,
                        cancel: &ctx.cancel,
                    },
                    &mut emitter,
                    &mut state,
                )
                .await;
                let failure = match result {
                    Ok(()) => {
                        return Exchange {
                            emitter,
                            state,
                            result: Ok(()),
                        };
                    }
                    Err(f) => f,
                };
                let started = state.started();
                let aborted = ctx.cancel.is_cancelled();
                let limit_before_start =
                    !started && failure.api_code(WEBSOCKET_CONNECTION_LIMIT_REACHED_CODE);
                let previous_not_found = failure.api_code(PREVIOUS_RESPONSE_NOT_FOUND_CODE);
                if !aborted && previous_not_found && !retried_missing_continuation {
                    retried_missing_continuation = true;
                    continue;
                }
                if !aborted && limit_before_start && !retried_connection_limit {
                    retried_connection_limit = true;
                    continue;
                }
                if aborted || (failure.non_transport() && !limit_before_start) {
                    return Exchange {
                        emitter,
                        state,
                        result: Err(failure),
                    };
                }
                diagnostics.push(transport_diagnostic(
                    &failure,
                    transport,
                    started,
                    body_json.len(),
                ));
                ws::record_failure(&self.ws, cache_session, &failure.message);
                if started {
                    return Exchange {
                        emitter,
                        state,
                        result: Err(failure),
                    };
                }
                ws::record_sse_fallback(&self.ws, cache_session);
                break;
            }
        }

        let mut emitter = ctx.emitter();
        let mut state = match ResponsesState::for_request(req, opts.service_tier.clone()) {
            Ok(s) => s,
            Err(m) => return setup_failure(m),
        };
        let result = match self.send_sse(ctx, &opts, &sse, &body_json, timeout).await {
            Ok(resp) => {
                let idle = timeout.unwrap_or(Duration::from_millis(2_147_483_647));
                read_sse_events(resp, idle, &ctx.cancel, &mut emitter, &mut state).await
            }
            Err(f) => Err(f),
        };
        Exchange {
            emitter,
            state,
            result,
        }
    }

    /// Reference SSE fetch with its own retry loop (lines 376-469).
    async fn send_sse(
        &self,
        ctx: &StreamCtx<'_>,
        opts: &CodexOptions,
        headers: &Headers,
        body_json: &str,
        timeout: Option<Duration>,
    ) -> Result<reqwest::Response, Failure> {
        let mut headers = headers.clone();
        let body: Vec<u8> =
            match zstd::bulk::compress(body_json.as_bytes(), REQUEST_COMPRESSION_ZSTD_LEVEL) {
                Ok(compressed) => {
                    headers.set("content-encoding", "zstd");
                    compressed
                }
                Err(_) => body_json.as_bytes().to_vec(),
            };
        let headers = headers.to_header_map().map_err(Failure::error)?;
        let url = resolve_codex_url(ctx.req.base_url());
        let max_retries = opts.max_retries;
        let cancel = &ctx.cancel;
        let mut attempt = 0u32;
        'attempts: loop {
            if cancel.is_cancelled() {
                return Err(Failure::aborted());
            }
            let failure: Failure = 'attempt: {
                let send = ctx
                    .http
                    .post(&url)
                    .headers(headers.clone())
                    .body(body.clone())
                    .send();
                let sent = tokio::select! {
                    _ = cancel.cancelled() => return Err(Failure::aborted()),
                    r = async {
                        match timeout {
                            Some(t) => tokio::time::timeout(t, send).await.map_err(|_| t),
                            None => Ok(send.await),
                        }
                    } => r,
                };
                let resp = match sent {
                    Err(t) => {
                        break 'attempt Failure::error(format!(
                            "Codex SSE response headers timed out after {}ms",
                            t.as_millis()
                        ));
                    }
                    Ok(Err(e)) => break 'attempt Failure::error(http::transport_error(&e)),
                    Ok(Ok(resp)) => resp,
                };
                if resp.status().is_success() {
                    return Ok(resp);
                }
                let status: StatusCode = resp.status();
                let resp_headers = resp.headers().clone();
                let text = tokio::select! {
                    _ = cancel.cancelled() => return Err(Failure::aborted()),
                    t = resp.text() => t.unwrap_or_default(),
                };
                if attempt < max_retries && is_retryable_error(status.as_u16(), &text) {
                    let delay = match retry_after_delay_ms(&resp_headers) {
                        None => backoff_ms(attempt),
                        Some(d) => match validate_retry_delay(d, opts) {
                            Ok(d) => d,
                            Err(f) => break 'attempt f,
                        },
                    };
                    sleep(delay, cancel).await?;
                    attempt += 1;
                    continue 'attempts;
                }
                let (message, friendly) = parse_error_response(
                    status.as_u16(),
                    &text,
                    status.canonical_reason().unwrap_or(""),
                );
                Failure::error(friendly.unwrap_or(message))
            };
            // Reference catch block: everything but aborts, retry-delay refusals and usage
            // limits is retried with backoff while attempts remain.
            if failure.kind == FailureKind::Aborted || failure.message == ABORTED {
                return Err(Failure::aborted());
            }
            if attempt < max_retries
                && failure.kind != FailureKind::RetryDelay
                && !failure.message.contains("usage limit")
            {
                sleep(backoff_ms(attempt), cancel).await?;
                attempt += 1;
                continue;
            }
            return Err(failure);
        }
    }
}

/// Reference `parseSSE` + `processStream` over the shared SSE reader.
async fn read_sse_events(
    resp: reqwest::Response,
    idle: Duration,
    cancel: &CancellationToken,
    emitter: &mut Emitter,
    state: &mut ResponsesState,
) -> Result<(), Failure> {
    let mut failure: Option<Failure> = None;
    let read = http::read_sse(resp, idle, cancel, |event| {
        let data = event.data.trim();
        if data.is_empty() || data == "[DONE]" {
            return ControlFlow::Continue(());
        }
        let value: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(e) => {
                failure = Some(Failure {
                    kind: FailureKind::Protocol,
                    message: format!("Invalid Codex SSE JSON: {e}"),
                    code: None,
                });
                return ControlFlow::Break(());
            }
        };
        match state.handle(emitter, value) {
            Ok(Step::Continue) => ControlFlow::Continue(()),
            Ok(Step::Done) => ControlFlow::Break(()),
            Err(f) => {
                failure = Some(f);
                ControlFlow::Break(())
            }
        }
    })
    .await;
    if let Some(f) = failure {
        return Err(f);
    }
    match read {
        Ok(()) => Ok(()),
        Err(ReadError::Aborted) => Err(Failure::aborted()),
        Err(ReadError::Failed(text)) => Err(Failure::error(text)),
    }
}

/// Reference `createAssistantMessageDiagnostic("provider_transport_failure", ...)`.
fn transport_diagnostic(
    failure: &Failure,
    transport: &str,
    started: bool,
    request_bytes: usize,
) -> Value {
    let mut error = Map::new();
    error.insert("name".into(), Value::String(failure.name().into()));
    error.insert("message".into(), Value::String(failure.message.clone()));
    match (&failure.kind, &failure.code) {
        (FailureKind::WsClose(Some(code)), _) => {
            error.insert("code".into(), json!(code));
        }
        (_, Some(code)) => {
            error.insert("code".into(), Value::String(code.clone()));
        }
        _ => {}
    }
    let mut details = Map::new();
    details.insert(
        "configuredTransport".into(),
        Value::String(transport.into()),
    );
    if !started {
        details.insert("fallbackTransport".into(), Value::String("sse".into()));
    }
    details.insert("eventsEmitted".into(), Value::Bool(started));
    details.insert(
        "phase".into(),
        Value::String(
            if started {
                "after_message_stream_start"
            } else {
                "before_message_stream_start"
            }
            .into(),
        ),
    );
    details.insert("requestBytes".into(), json!(request_bytes));
    json!({
        "type": "provider_transport_failure",
        "timestamp": crate::util::now_ms(),
        "error": Value::Object(error),
        "details": Value::Object(details),
    })
}
