//! Request-level sending shared by the SDK-style adapters (reference
//! `utils/provider-retry.ts` `retryProviderRequest` plus the SDK header timeout), stream
//! timeouts, error-text truncation (reference `utils/error-body.ts`) and the Pi user agent
//! (reference `utils/pi-user-agent.ts`).

use std::time::Duration;

use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::Value;

use super::opt;
use crate::provider::{ProviderRequest, StreamCtx, http};

/// The largest timer Node accepts (2^31 - 1 ms): what a zero (disabled) SDK timeout becomes.
pub const NO_TIMEOUT: Duration = Duration::from_millis(i32::MAX as u64);

/// SDK-style request retry policy: `maxRetries` (default 0) and `maxRetryDelayMs` (default
/// 60000; 0 disables the cap), as the reference `retryProviderRequest` reads them.
pub fn retry_policy(req: &ProviderRequest) -> (u32, Duration) {
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

/// The stream idle timeout: the request's timeout, or `when_zero` when it is disabled. The
/// SDK adapters pass [`NO_TIMEOUT`] (OpenAI SDK semantics); the fetch-based ones pass
/// [`http::DEFAULT_IDLE_TIMEOUT`].
pub fn idle_timeout(req: &ProviderRequest, when_zero: Duration) -> Duration {
    if req.timeout.is_zero() {
        when_zero
    } else {
        req.timeout
    }
}

/// POST a JSON body with the shared request-level retry policy until response headers arrive.
/// `content-type: application/json` is added unless a configured header already sets it. A gap
/// of `req.timeout` per attempt (plus the allowed retry delays) before response headers is a
/// timeout; a zero timeout disables that. Returns the formatted error text on failure
/// (`"<status>: <body>"` for HTTP errors).
pub async fn post_json(
    ctx: &StreamCtx<'_>,
    url: &str,
    headers: HeaderMap,
    body: &Value,
) -> Result<reqwest::Response, String> {
    post_json_observing(ctx, url, headers, body, |_| {}).await
}

pub(crate) async fn post_json_observing(
    ctx: &StreamCtx<'_>,
    url: &str,
    mut headers: HeaderMap,
    body: &Value,
    observe: impl Fn(reqwest::StatusCode),
) -> Result<reqwest::Response, String> {
    if !headers.contains_key(CONTENT_TYPE) {
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    }
    let bytes = serde_json::to_vec(body).map_err(|e| e.to_string())?;
    let (retries, max_delay) = retry_policy(ctx.req);
    let send = http::send_observing(
        || {
            ctx.http
                .post(url)
                .headers(headers.clone())
                .body(bytes.clone())
        },
        retries,
        max_delay,
        &ctx.cancel,
        observe,
    );
    if ctx.req.timeout.is_zero() {
        return send.await;
    }
    let budget = ctx
        .req
        .timeout
        .saturating_mul(retries.saturating_add(1))
        .saturating_add(max_delay.saturating_mul(retries));
    match tokio::time::timeout(budget, send).await {
        Ok(r) => r,
        Err(_) => Err(format!(
            "Request timed out: no response headers within {} seconds",
            budget.as_secs()
        )),
    }
}

/// `text` cut to `max` characters with the reference truncation suffix
/// (`error-body.ts:139`).
pub fn truncate_error_text(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_owned();
    }
    let head: String = text.chars().take(max).collect();
    format!("{head}... [truncated {} chars]", count - max)
}

/// Reference `getPiUserAgent`: `pi (<platform> <release>; <arch>)` with Node's names.
pub fn pi_user_agent() -> String {
    let platform = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "ia32",
        "powerpc64" => "ppc64",
        other => other,
    };
    format!("pi ({platform} {}; {arch})", os_release())
}

#[cfg(unix)]
fn os_release() -> String {
    // SAFETY: `uname` fills a caller-owned, zeroed struct; the release field is NUL-terminated.
    unsafe {
        let mut name: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut name) != 0 {
            return String::new();
        }
        std::ffi::CStr::from_ptr(name.release.as_ptr())
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(not(unix))]
fn os_release() -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_text_is_truncated_with_the_reference_suffix() {
        assert_eq!(truncate_error_text("abc", 3), "abc");
        assert_eq!(
            truncate_error_text("abcdef", 2),
            "ab... [truncated 4 chars]"
        );
        assert!(pi_user_agent().starts_with("pi ("));
    }
}
