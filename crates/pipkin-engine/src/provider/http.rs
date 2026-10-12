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
    pub extra_roots: super::trust::ExtraRoots,
    /// Reference `httpIdleTimeoutMs` as the dispatcher's header/body idle timeout: no bytes
    /// for this long on a connection fails the read. `None` disables it.
    pub idle: Option<Duration>,
    /// A supplied invalid network setting refuses requests until the next valid snapshot.
    pub validation_error: Option<String>,
    /// Local test CAs, never an insecure TLS mode.
    #[cfg(test)]
    pub test_roots: Vec<Vec<u8>>,
}

impl NetSettings {
    /// From the engine environment and the global settings (`httpProxy` is global-only in the
    /// reference, `main.ts:594,871`).
    pub fn new(env: &crate::config::Env, global: &crate::config::Settings) -> NetSettings {
        let (idle, validation_error) = match global.http_idle_timeout() {
            Ok(idle) => (idle, None),
            Err(error) => (None, Some(error)),
        };
        NetSettings {
            proxy: ProxyConfig::from_env(env, global.http_proxy.as_deref()),
            extra_roots: super::trust::ExtraRoots::from_env(env),
            idle,
            validation_error,
            #[cfg(test)]
            test_roots: vec![],
        }
    }
}

/// The engine client: explicit proxy selection (reqwest's ambient lookup is off) and the idle
/// timeout as a per-read timeout.
pub fn client_with(net: &NetSettings) -> reqwest::Client {
    builder(net)
        .build()
        .expect("the HTTP client configuration is valid")
}

pub(crate) fn client_with_identity(
    net: &NetSettings,
    identity: reqwest::Identity,
) -> Result<reqwest::Client, String> {
    builder(net)
        .identity(identity)
        .build()
        .map_err(|_| "Cannot build Google certificate TLS client".into())
}

pub(crate) fn builder(net: &NetSettings) -> reqwest::ClientBuilder {
    let mut builder = reqwest::Client::builder()
        .http_proxy_tunnel(true)
        .user_agent(concat!("pipkin/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(30))
        .pool_idle_timeout(Duration::from_secs(90))
        .no_proxy();
    if net.validation_error.is_some() || !net.proxy.is_empty() {
        for warning in net.proxy.warnings() {
            log::warn!("proxy configuration: {warning}");
        }
        let validation = net.proxy.clone();
        let error = net.validation_error.clone();
        builder = builder.request_validator(move |url| {
            if let Some(error) = &error {
                return Err(NetworkConfigurationError(error.clone()));
            }
            validation
                .proxy_for(url)
                .map(|_| ())
                .map_err(NetworkConfigurationError)
        });
        let proxy = net.proxy.clone();
        // The same immutable policy rejects errors before dispatch, including redirects.
        builder = builder.proxy(reqwest::Proxy::custom(move |url| {
            proxy.proxy_for(url).ok().flatten()
        }));
    }
    if let Some(idle) = net.idle {
        builder = builder.read_timeout(idle);
    }
    #[cfg(test)]
    for root in &net.test_roots {
        builder = builder
            .add_root_certificate(reqwest::Certificate::from_der(root).expect("valid test CA"));
    }
    net.extra_roots.configure_http(builder)
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

#[derive(Debug)]
struct NetworkConfigurationError(String);

impl std::fmt::Display for NetworkConfigurationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NetworkConfigurationError {}

/// A transport error's text, including its cause chain (so "timed out", "connection refused"
/// and similar reach the retry classifier).
pub fn transport_error(e: &reqwest::Error) -> String {
    let mut text = e.to_string();
    let mut source = std::error::Error::source(e);
    while let Some(s) = source {
        if let Some(proxy) = s.downcast_ref::<NetworkConfigurationError>() {
            return proxy.to_string();
        }
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
            Err(e) => (!e.is_builder(), transport_error(&e), None),
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
            validation_error: None,
            test_roots: vec![],
            extra_roots: Default::default(),
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
    async fn http_proxy_connect_matches_eight_executed_undici_cases() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../fixtures/native-http-tunnel/reference.json"
        ))
        .unwrap();
        let cases = fixture["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 8);
        for case in cases {
            let target = if case["targetTls"] == true {
                MockServer::start_tls(|_, _| ok()).await
            } else {
                MockServer::start(|_, _| ok()).await
            };
            let proxy =
                crate::provider::mock::TunnelProxy::start(&target.url, case["proxyTls"] == true)
                    .await;
            let proxy_url = proxy.url.replacen("://", "://us%65r:p%40ss@", 1);
            let mut settings = net(
                &[("HTTP_PROXY", &proxy_url), ("HTTPS_PROXY", &proxy_url)],
                None,
            );
            settings.test_roots =
                vec![include_bytes!("../../../../fixtures/native-tls/ca.der").to_vec()];
            let client = client_with(&settings);
            let response = client
                .post(case["url"].as_str().unwrap())
                .header("Authorization", "Bearer origin-secret")
                .body("{}")
                .send()
                .await
                .unwrap();
            assert_eq!(
                response.status().as_u16(),
                case["status"].as_u64().unwrap() as u16
            );
            assert_eq!(
                response.json::<serde_json::Value>().await.unwrap(),
                case["body"]
            );
            let requests = proxy.requests();
            assert_eq!(requests.len(), 1, "{}", case["name"]);
            let connect = &requests[0];
            let actual = serde_json::json!({"method":connect.method,"authority":connect.path,"authorization":connect.header("authorization"),"proxyAuthorization":connect.header("proxy-authorization")});
            assert_eq!(actual, case["connects"][0], "{}", case["name"]);
            assert!(connect.body.is_empty());
            let requests = target.requests();
            assert_eq!(requests.len(), 1);
            let origin = &requests[0];
            let actual = serde_json::json!({"method":origin.method,"path":origin.path,"authorization":origin.header("authorization"),"proxyAuthorization":origin.header("proxy-authorization"),"body":String::from_utf8_lossy(&origin.body)});
            assert_eq!(actual, case["originRequests"][0], "{}", case["name"]);
            // Default-port normalization must preserve the brackets around IPv6 authorities.
            if case["targetTls"] == false {
                client
                    .get("http://[::1]/ipv6")
                    .send()
                    .await
                    .unwrap()
                    .bytes()
                    .await
                    .unwrap();
                assert_eq!(proxy.requests().last().unwrap().path, "[::1]:80");
                assert_eq!(target.requests().last().unwrap().path, "/ipv6");
            }
        }
    }

    #[tokio::test]
    async fn upstream_proxy_default_still_forwards_without_the_opt_in() {
        let proxy = MockServer::start(|_, _| ok()).await;
        let client = reqwest::Client::builder()
            .no_proxy()
            .proxy(reqwest::Proxy::all(&proxy.url).unwrap())
            .build()
            .unwrap();
        client
            .get("http://provider.test/default")
            .send()
            .await
            .unwrap();
        let requests = proxy.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "GET");
        assert_eq!(requests[0].path, "http://provider.test/default");
    }

    #[tokio::test]
    async fn a_plain_http_tunnel_is_pooled_and_connect_timeout_closes_its_socket() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            for _ in 0..2 {
                let req = crate::provider::mock::read_request(&mut socket)
                    .await
                    .unwrap();
                assert_eq!(req.path, "/pooled");
                socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .await
                    .unwrap();
            }
        });
        let proxy = crate::provider::mock::TunnelProxy::start(&target, false).await;
        let client = client_with(&net(&[("HTTP_PROXY", &proxy.url)], None));
        for _ in 0..2 {
            assert_eq!(
                client
                    .get("http://provider.test/pooled")
                    .send()
                    .await
                    .unwrap()
                    .text()
                    .await
                    .unwrap(),
                "ok"
            );
        }
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(proxy.requests().len(), 1);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let req = crate::provider::mock::read_request(&mut socket)
                .await
                .unwrap();
            assert_eq!(req.method, "CONNECT");
            let mut byte = [0];
            tokio::time::timeout(Duration::from_secs(5), socket.read(&mut byte))
                .await
                .unwrap()
                .unwrap()
        });
        let client = client_with(&net(&[("HTTP_PROXY", &url)], None));
        let error = client
            .get("http://provider.test/stall")
            .timeout(Duration::from_millis(100))
            .send()
            .await
            .unwrap_err();
        assert!(error.is_timeout());
        assert_eq!(
            task.await.unwrap(),
            0,
            "a timed-out CONNECT must close its socket"
        );
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
        for proxy in [
            "socks5://127.0.0.1:1080",
            "pac+http://wpad/proxy.pac",
            "http://secret:password@[bad",
        ] {
            let settings = net(&[("HTTP_PROXY", proxy)], None);
            let url = format!("{}/x", target.url);
            let expected = settings
                .proxy
                .proxy_for(&url::Url::parse(&url).unwrap())
                .unwrap_err();
            let client = client_with(&settings);
            let error = client.get(&url).send().await.unwrap_err();
            assert!(error.is_builder());
            assert_eq!(transport_error(&error), expected);
            assert!(!transport_error(&error).contains("password"));
            assert!(!settings.proxy.warnings().join(" ").contains("password"));
            let attempts = std::sync::atomic::AtomicUsize::new(0);
            let error = send(
                || {
                    attempts.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    client.get(&url)
                },
                3,
                MAX_RETRY_DELAY,
                &CancellationToken::new(),
            )
            .await
            .unwrap_err();
            assert_eq!(error, expected);
            assert_eq!(attempts.load(std::sync::atomic::Ordering::Relaxed), 1);
        }
        assert!(target.requests().is_empty(), "never sent directly");
    }

    #[tokio::test]
    async fn proxy_validation_follows_redirects_and_no_proxy_exclusions() {
        let target = MockServer::start(|_, _| ok()).await;
        let destination = target.url.replace("127.0.0.1", "localhost");
        let redirect = MockServer::start(move |_, _| {
            let mut response = MockResponse::json(302, &serde_json::json!({}));
            response
                .headers
                .push(("location".into(), destination.clone()));
            response
        })
        .await;
        let settings = net(
            &[
                ("HTTP_PROXY", "socks5://127.0.0.1:1080"),
                ("NO_PROXY", "127.0.0.1"),
            ],
            None,
        );
        let client = client_with(&settings);
        client.get(&target.url).send().await.unwrap();
        let error = client.get(&redirect.url).send().await.unwrap_err();
        assert_eq!(
            transport_error(&error),
            format!(
                "{} Got socks5:",
                super::super::proxy::UNSUPPORTED_PROXY_PROTOCOL_MESSAGE
            )
        );
        assert_eq!(redirect.requests().len(), 1);
        assert_eq!(
            target.requests().len(),
            1,
            "redirect never bypasses invalid proxy"
        );
    }

    #[tokio::test]
    async fn request_validation_applies_to_reused_connections() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![];
            loop {
                request.push(stream.read_u8().await.unwrap());
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK")
                .await
                .unwrap();
            let mut byte = [0];
            // A denied second request must not write on the pooled connection.
            tokio::time::timeout(Duration::from_millis(200), stream.read(&mut byte)).await
        });
        let allowed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let validation = allowed.clone();
        let client = reqwest::Client::builder()
            .no_proxy()
            .request_validator(move |_| {
                if validation.load(std::sync::atomic::Ordering::Relaxed) {
                    Ok(())
                } else {
                    Err("denied")
                }
            })
            .build()
            .unwrap();
        assert_eq!(
            client.get(&url).send().await.unwrap().text().await.unwrap(),
            "OK"
        );
        allowed.store(false, std::sync::atomic::Ordering::Relaxed);
        assert!(client.get(&url).send().await.unwrap_err().is_builder());
        assert!(
            server.await.unwrap().is_err(),
            "pooled socket receives no second request"
        );
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
