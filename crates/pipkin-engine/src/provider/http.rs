//! Shared HTTP plumbing for wire adapters: one client, request-level retry before headers,
//! idle-timed SSE reading with cancellation, and reference-format error strings.
//!
//! The client honours `HTTP(S)_PROXY`/`ALL_PROXY`/`NO_PROXY` (reqwest's system proxy support)
//! and the platform roots plus webpki roots for TLS. Request bodies and credentials never reach
//! logs from here.

use std::ops::ControlFlow;
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::{RequestBuilder, Response, StatusCode};
use tokio_util::sync::CancellationToken;

use super::sse::{SseDecoder, SseEvent};

/// Default header/idle timeout (reference `httpIdleTimeoutMs`).
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// Largest provider-requested retry delay honoured (reference `maxRetryDelayMs`).
pub const MAX_RETRY_DELAY: Duration = Duration::from_secs(60);
/// Error bodies are cut to this many characters (reference `error-body.ts`).
const MAX_ERROR_BODY_CHARS: usize = 4000;

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("pipkin/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(30))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
        .expect("the HTTP client configuration is valid")
}

/// `"<status>: <body>"` (or the transport message when there is no response).
pub fn format_error(status: Option<StatusCode>, body: &str, message: &str) -> String {
    let body: String = body.chars().take(MAX_ERROR_BODY_CHARS).collect();
    match status {
        Some(s) if !body.trim().is_empty() => format!("{}: {}", s.as_u16(), body.trim()),
        Some(s) => format!("{} status code (no body)", s.as_u16()),
        None => message.to_owned(),
    }
}

/// A transport error's text, including its cause chain (so "timed out", "connection refused"
/// and similar reach the retry classifier).
pub fn transport_error(e: &reqwest::Error) -> String {
    let mut text = e.to_string();
    let mut source = std::error::Error::source(e);
    while let Some(s) = source {
        text.push_str(": ");
        text.push_str(&s.to_string());
        source = s.source();
    }
    if e.is_timeout() && !text.contains("timed out") {
        text.push_str(" (timed out)");
    }
    text
}

fn retryable_status(status: StatusCode, headers: &reqwest::header::HeaderMap) -> bool {
    if let Some(v) = headers.get("x-should-retry").and_then(|v| v.to_str().ok()) {
        return v == "true";
    }
    matches!(status.as_u16(), 408 | 409 | 429) || status.is_server_error()
}

fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    if let Some(ms) = headers
        .get("retry-after-ms")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<f64>().ok())
    {
        return Some(Duration::from_millis(ms.max(0.0) as u64));
    }
    let raw = headers.get("retry-after")?.to_str().ok()?;
    raw.trim()
        .parse::<f64>()
        .ok()
        .map(|s| Duration::from_millis((s.max(0.0) * 1000.0) as u64))
}

/// Send with SDK-style retries before response headers: network errors and 408/409/429/5xx
/// (or `x-should-retry`), honouring `retry-after` up to `max_delay`. Never retries mid-stream.
/// Returns a successful response or the formatted error text.
pub async fn send(
    build: impl Fn() -> RequestBuilder,
    retries: u32,
    max_delay: Duration,
    cancel: &CancellationToken,
) -> Result<Response, String> {
    let mut attempt = 0u32;
    loop {
        let result = tokio::select! {
            _ = cancel.cancelled() => return Err("Request aborted".into()),
            r = build().send() => r,
        };
        let (retry, error, wait) = match result {
            Ok(resp) if resp.status().is_success() => return Ok(resp),
            Ok(resp) => {
                let status = resp.status();
                let headers = resp.headers().clone();
                let body = resp.text().await.unwrap_or_default();
                let wait = retry_after(&headers);
                if let Some(w) = wait
                    && w > max_delay
                    && !max_delay.is_zero()
                {
                    return Err(format!(
                        "Server requested {}s retry delay (max: {}s)",
                        w.as_secs(),
                        max_delay.as_secs()
                    ));
                }
                (
                    retryable_status(status, &headers),
                    format_error(Some(status), &body, ""),
                    wait,
                )
            }
            Err(e) => (true, transport_error(&e), None),
        };
        if !retry || attempt >= retries {
            return Err(error);
        }
        let backoff = wait.unwrap_or_else(|| {
            let secs = (0.5 * 2f64.powi(attempt as i32)).min(8.0);
            let jitter = 1.0 - (rand_unit() * 0.25);
            Duration::from_secs_f64(secs * jitter)
        });
        attempt += 1;
        tokio::select! {
            _ = cancel.cancelled() => return Err("Request aborted".into()),
            _ = tokio::time::sleep(backoff) => {}
        }
    }
}

fn rand_unit() -> f64 {
    let mut b = [0u8; 4];
    let _ = getrandom::getrandom(&mut b);
    u32::from_le_bytes(b) as f64 / u32::MAX as f64
}

/// Why reading a stream stopped early.
#[derive(Debug, PartialEq, Eq)]
pub enum ReadError {
    Aborted,
    /// Transport failure or idle timeout, with reference-classifiable text.
    Failed(String),
}

/// Read an SSE body, calling `on_event` per event until it breaks or the body ends. A gap of
/// `idle` without bytes is a timeout. Cancellation wins over everything.
pub async fn read_sse(
    resp: Response,
    idle: Duration,
    cancel: &CancellationToken,
    mut on_event: impl FnMut(SseEvent) -> ControlFlow<()>,
) -> Result<(), ReadError> {
    let mut stream = resp.bytes_stream();
    let mut decoder = SseDecoder::new();
    loop {
        let next = tokio::select! {
            _ = cancel.cancelled() => return Err(ReadError::Aborted),
            n = tokio::time::timeout(idle, stream.next()) => n,
        };
        match next {
            Err(_) => {
                return Err(ReadError::Failed(format!(
                    "Stream idle timeout: no data for {} seconds (timed out)",
                    idle.as_secs()
                )));
            }
            Ok(None) => {
                if let Some(event) = decoder.finish() {
                    let _ = on_event(event);
                }
                return Ok(());
            }
            Ok(Some(Err(e))) => return Err(ReadError::Failed(transport_error(&e))),
            Ok(Some(Ok(bytes))) => {
                let events = decoder.push(&bytes).map_err(ReadError::Failed)?;
                for event in events {
                    if on_event(event).is_break() {
                        return Ok(());
                    }
                }
            }
        }
    }
}
