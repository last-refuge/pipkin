//! Shared HTTP plumbing for wire adapters: one client, request-level retry before headers,
//! idle-timed SSE reading with cancellation, and reference-format error strings.
//!
//! The engine's client takes its proxy from an explicit [`ProxyConfig`] (the engine `Env` plus
//! settings `httpProxy`; reqwest's ambient system-proxy lookup is never used) and its per-read
//! idle timeout from settings `httpIdleTimeoutMs` ([`NetSettings`]). Request bodies and
//! credentials never reach logs from here.

use std::ops::ControlFlow;
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::{RequestBuilder, Response, StatusCode};
use tokio_util::sync::CancellationToken;

use super::proxy::ProxyConfig;
use super::sse::{SseDecoder, SseEvent};

/// Default header/idle timeout (reference `httpIdleTimeoutMs`).
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// Largest provider-requested retry delay honoured (reference `maxRetryDelayMs`).
pub const MAX_RETRY_DELAY: Duration = Duration::from_secs(60);
/// Error bodies are cut to this many characters (reference `error-body.ts`).
const MAX_ERROR_BODY_CHARS: usize = 4000;

/// A client with no proxy and no idle timeout (adapter tests against local servers).
pub fn client() -> reqwest::Client {
    client_with(&NetSettings::default())
}

/// What the engine-wide HTTP client is built from; the engine rebuilds it when this changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetSettings {
    pub proxy: ProxyConfig,
    /// Reference `httpIdleTimeoutMs` as the dispatcher's header/body idle timeout: no bytes
    /// for this long on a connection fails the read. `None` disables it.
    pub idle: Option<Duration>,
}

impl NetSettings {
    /// From the engine environment and the global settings (`httpProxy` is global-only in the
    /// reference, `main.ts:594,871`).
    pub fn new(env: &crate::config::Env, global: &crate::config::Settings) -> NetSettings {
        NetSettings {
            proxy: ProxyConfig::from_env(env, global.http_proxy.as_deref()),
            idle: global.http_idle_timeout(),
        }
    }
}

/// Where a request whose configured proxy is unusable (SOCKS, PAC, malformed) is sent: a
/// reserved name that never resolves (RFC 6761), so the request fails instead of silently
/// bypassing the proxy. Model requests are refused earlier with the reference message.
const UNUSABLE_PROXY: &str = "http://unusable-proxy.invalid:9";

/// The engine client: explicit proxy selection (reqwest's ambient lookup is off) and the idle
/// timeout as a per-read timeout.
pub fn client_with(net: &NetSettings) -> reqwest::Client {
    let mut builder = reqwest::Client::builder()
        .user_agent(concat!("pipkin/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(30))
        .pool_idle_timeout(Duration::from_secs(90))
        .no_proxy();
    if !net.proxy.is_empty() {
        for warning in net.proxy.warnings() {
            log::warn!("proxy configuration: {warning}");
        }
        let proxy = net.proxy.clone();
        builder = builder.proxy(reqwest::Proxy::custom(move |url| {
            match proxy.proxy_for(url) {
                Ok(p) => p,
                Err(_) => Some(reqwest::Url::parse(UNUSABLE_PROXY).expect("valid URL")),
            }
        }));
    }
    if let Some(idle) = net.idle {
        builder = builder.read_timeout(idle);
    }
    builder
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
    send_observing(build, retries, max_delay, cancel, |_| {}).await
}

/// Observe response status before retries or error-body consumption (credential invalidation).
pub(crate) async fn send_observing(
    build: impl Fn() -> RequestBuilder,
    retries: u32,
    max_delay: Duration,
    cancel: &CancellationToken,
    observe: impl Fn(StatusCode),
) -> Result<Response, String> {
    let mut attempt = 0u32;
    loop {
        let result = tokio::select! {
            _ = cancel.cancelled() => return Err("Request aborted".into()),
            r = build().send() => r,
        };
        if let Ok(response) = &result {
            observe(response.status());
        }
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::config::Env;
    use crate::provider::mock::{MockResponse, MockServer};

    fn net(vars: &[(&str, &str)], idle: Option<Duration>) -> NetSettings {
        let env = Env::isolated(
            vars.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<HashMap<_, _>>(),
        );
        NetSettings {
            proxy: ProxyConfig::from_env(&env, None),
            idle,
        }
    }

    fn ok() -> MockResponse {
        MockResponse::json(200, &serde_json::json!({"ok": true}))
    }

    #[tokio::test]
    async fn native_user_agent_is_sent_and_explicit_provider_identity_takes_precedence() {
        let server = MockServer::start(|_, _| ok()).await;
        let http = client();
        http.get(&server.url).send().await.unwrap();
        http.get(&server.url)
            .header("User-Agent", "provider-identity")
            .header("Authorization", "Bearer fixture-token")
            .send()
            .await
            .unwrap();
        let seen = server.requests();
        assert_eq!(
            seen[0].header("user-agent"),
            Some(concat!("pipkin/", env!("CARGO_PKG_VERSION")))
        );
        assert_eq!(seen[1].header("user-agent"), Some("provider-identity"));
        assert_eq!(
            seen[1].header("authorization"),
            Some("Bearer fixture-token")
        );
        assert_eq!(
            seen[1]
                .headers
                .iter()
                .filter(|(k, _)| k == "user-agent")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn plain_http_requests_are_forwarded_through_the_configured_proxy() {
        let proxy = MockServer::start(|_, _| ok()).await;
        let client = client_with(&net(&[("HTTP_PROXY", &proxy.url)], None));
        let resp = client
            .post("http://provider.test:8080/v1/chat")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let seen = proxy.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].method, "POST");
        assert_eq!(seen[0].path, "http://provider.test:8080/v1/chat");
    }

    #[tokio::test]
    async fn https_requests_open_a_connect_tunnel_through_the_proxy() {
        // The mock is not a real proxy: it records the CONNECT and the TLS handshake fails.
        let proxy = MockServer::start(|_, _| ok()).await;
        let client = client_with(&net(&[("https_proxy", &proxy.url)], None));
        let result = client.get("https://provider.test/v1/models").send().await;
        assert!(result.is_err());
        let seen = proxy.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].method, "CONNECT");
        assert_eq!(seen[0].path, "provider.test:443");
    }

    #[tokio::test]
    async fn no_proxy_targets_and_unproxied_schemes_connect_directly() {
        let proxy = MockServer::start(|_, _| ok()).await;
        let target = MockServer::start(|_, _| ok()).await;
        let client = client_with(&net(
            &[("HTTP_PROXY", &proxy.url), ("NO_PROXY", "127.0.0.1")],
            None,
        ));
        let resp = client
            .get(format!("{}/x", target.url))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        // Only HTTPS is proxied here, so a plain HTTP request goes direct.
        let https_only = client_with(&net(&[("HTTPS_PROXY", &proxy.url)], None));
        https_only
            .get(format!("{}/y", target.url))
            .send()
            .await
            .unwrap();
        assert_eq!(target.requests().len(), 2);
        assert!(proxy.requests().is_empty());
    }

    #[tokio::test]
    async fn an_unusable_proxy_fails_the_request_instead_of_bypassing_it() {
        let target = MockServer::start(|_, _| ok()).await;
        for proxy in ["socks5://127.0.0.1:1080", "http://[bad"] {
            let client = client_with(&net(&[("HTTP_PROXY", proxy)], None));
            let result = client.get(format!("{}/x", target.url)).send().await;
            assert!(result.is_err(), "{proxy}");
        }
        assert!(target.requests().is_empty(), "never sent directly");
    }

    #[tokio::test]
    async fn the_idle_timeout_fails_a_stalled_body_and_zero_disables_it() {
        let stalled = || {
            let mut r = MockResponse::sse(vec!["data: {}\n\n".into()]);
            r.hang = true;
            r
        };
        let server = MockServer::start(move |_, _| stalled()).await;
        let client = client_with(&net(&[], Some(Duration::from_millis(200))));
        let mut resp = client.get(&server.url).send().await.unwrap();
        assert!(resp.chunk().await.unwrap().is_some());
        let started = std::time::Instant::now();
        let err = resp.chunk().await.unwrap_err();
        assert!(transport_error(&err).contains("timed out"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5));

        // Disabled: the read is still pending after well over the old timeout.
        let client = client_with(&net(&[], None));
        let mut resp = client.get(&server.url).send().await.unwrap();
        assert!(resp.chunk().await.unwrap().is_some());
        let pending = tokio::time::timeout(Duration::from_millis(600), resp.chunk()).await;
        assert!(pending.is_err(), "no idle timeout when disabled");
    }
}
