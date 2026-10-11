//! Helpers shared by the wire adapters, one implementation each.
//!
//! - [`transform`]: cross-provider replay normalization (`transform-messages.ts`), `shortHash`
//!   and the tool-call-id sanitizers.
//! - [`headers`]: case-insensitive header layering, `hasHeader`.
//! - [`http`]: request-level send with SDK-style retries, idle timeouts, error-text truncation,
//!   the Pi user agent.
//! - [`thinking`]: thinking levels and budgets, sampling parameters, context estimation and
//!   `clampMaxTokensToContext`.
//! - [`json`]: JSON repair for streamed tool arguments and tool-argument settling.
//! - [`schema`]: strict JSON-schema conversion for constrained sampling.
//!
//! The OpenAI Responses conversion and stream processor shared by `openai-responses`,
//! `azure-openai-responses` and `openai-codex-responses` live in
//! [`super::openai_responses_shared`].
//!
//! # Prompt-cache retention (`PI_CACHE_RETENTION`)
//!
//! Adapters never read `PI_CACHE_RETENTION` (or any cache env) themselves. The engine resolves
//! it once into [`ProviderRequest::cache_retention`](crate::provider::ProviderRequest):
//! `Long` when `PI_CACHE_RETENTION=long`, else `Short`; `None` means caching is off. This is the
//! reference `resolveCacheRetention(options.cacheRetention, env)` result, so every adapter
//! reads only the request field:
//!
//! | adapter | `None` | `Short` | `Long` |
//! | --- | --- | --- | --- |
//! | anthropic-messages | no `cache_control` | ephemeral | ephemeral `ttl: 1h` (if supported) |
//! | bedrock-converse-stream | no cache points | cache points | cache points `ttl: 1h` |
//! | openai-completions | no cache key/markers | `prompt_cache_key` (OpenAI) / markers | + `24h` retention or `1h` markers where supported |
//! | openai-responses | no `prompt_cache_key` (explicit cache mode where supported) | `prompt_cache_key` | + `24h` retention / `30m` explicit TTL where supported |
//! | azure-openai-responses | ignored: `prompt_cache_key` is always the session id (reference `azure-openai-responses.ts:308`) | | |
//! | openai-codex-responses | no session/cache key | session id as cache key | same as `Short` |
//! | mistral-conversations | no affinity/cache key | `x-affinity` + `prompt_cache_key` | same as `Short` |
//! | pi-messages | `cacheRetention: "none"` | `"short"` (the reference sends `undefined` when nothing was configured; the engine cannot tell the two apart) | `"long"` |
//! | google-* | not used by the reference | | |

pub mod headers;
pub mod http;
pub mod json;
pub mod schema;
pub mod thinking;
pub mod transform;

use serde_json::Value;

use crate::provider::ProviderRequest;

pub use self::headers::{Headers, build_headers, has_configured_header, has_header, header_map};
pub use self::http::{
    NO_TIMEOUT, idle_timeout, pi_user_agent, post_json, retry_policy, truncate_error_text,
};
pub use self::json::{ToolArgs, finalize_tool_args, parse_json_with_repair, repair_json};
pub use self::thinking::{
    MIN_ANSWER_TOKENS, Mapped, adjust_max_tokens_for_thinking, base_max_tokens,
    clamp_max_tokens_to_context, clamp_reasoning, clamp_thinking_budget_to_answer_room,
    custom_budget, estimate_context_tokens, mapped_level, reasoning_effort,
    resolve_sampling_params, thinking_budget_for_level,
};
pub use self::transform::{
    NormalizeToolCallId, is_same_model, sanitize_id_chars, sanitize_tool_call_id, short_hash,
    take_chars, transform_messages,
};

/// A provider option from `ProviderRequest::options` (`null` counts as unset).
pub fn opt<'a>(req: &'a ProviderRequest, key: &str) -> Option<&'a Value> {
    req.options.get(key).filter(|v| !v.is_null())
}

/// A string option.
pub fn opt_str<'a>(req: &'a ProviderRequest, key: &str) -> Option<&'a str> {
    opt(req, key).and_then(Value::as_str)
}

/// JavaScript truthiness of a JSON value (`null`, `false`, `0`, `""` are falsy).
pub fn js_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// A base64 data URL for an image block.
pub fn data_url(mime: &str, data: &str) -> String {
    format!("data:{mime};base64,{data}")
}
