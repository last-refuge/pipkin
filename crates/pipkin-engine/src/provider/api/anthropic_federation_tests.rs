use super::super::{
    AnthropicMessages,
    tests::{claude, error_text, simple_reply},
};
use super::*;
use crate::provider::api::google_shared::tests::request;
use crate::provider::mock::{MockResponse, MockServer};
use crate::provider::{ApiAdapter, Credential};
use pipkin_engine_protocol::{Message, StopReason};
use tokio_util::sync::CancellationToken;

fn setup(base: &str) -> (tempfile::TempDir, ProviderRequest) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("identity-token");
    std::fs::write(&path, "  assertion-one\n").unwrap();
    let mut req = request(claude("anthropic", json!({})));
    req.credential = Credential::None;
    req.base_url = Some(base.into());
    req.messages = vec![Message::user_text("hello", 1)];
    req.env.extend([
        ("ANTHROPIC_FEDERATION_RULE_ID".into(), "rule".into()),
        ("ANTHROPIC_ORGANIZATION_ID".into(), "org".into()),
        (
            "ANTHROPIC_IDENTITY_TOKEN_FILE".into(),
            path.display().to_string(),
        ),
    ]);
    (dir, req)
}

async fn run(
    adapter: &AnthropicMessages,
    req: &ProviderRequest,
    cancel: CancellationToken,
) -> Message {
    adapter
        .stream(StreamCtx {
            req,
            http: &crate::provider::http::client(),
            events: None,
            cancel,
        })
        .await
}

fn token(value: &str, expiry: u64) -> MockResponse {
    MockResponse::json(
        200,
        &json!({"access_token":value,"expires_in":expiry,"token_type":"Bearer"}),
    )
}

async fn until(done: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !done() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn federation_exchange_cache_rotation_and_workspace_scope_preserve_wire_contract() {
    let server = MockServer::start(|r, _| {
        if r.path == "/v1/oauth/token" {
            token(
                &format!("token-{}", r.json()["assertion"].as_str().unwrap()),
                3600,
            )
        } else {
            MockResponse::sse(vec![simple_reply("ok")])
        }
    })
    .await;
    let (_dir, mut req) = setup(&server.url);
    req.env
        .insert("ANTHROPIC_SERVICE_ACCOUNT_ID".into(), "service".into());
    req.env
        .insert("ANTHROPIC_WORKSPACE_ID".into(), "workspace-a".into());
    let adapter = AnthropicMessages::default();
    for _ in 0..2 {
        assert_eq!(
            run(&adapter, &req, CancellationToken::new()).await.text(),
            "ok"
        );
    }
    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests[0].json(),
        json!({"grant_type":"urn:ietf:params:oauth:grant-type:jwt-bearer",
        "assertion":"assertion-one","federation_rule_id":"rule","organization_id":"org",
        "service_account_id":"service","workspace_id":"workspace-a"})
    );
    assert_eq!(
        requests[0].header("anthropic-beta"),
        Some("oauth-2025-04-20,oidc-federation-2026-04-01")
    );
    assert!(
        requests[0]
            .header("user-agent")
            .unwrap()
            .starts_with("pipkin/")
    );
    for sent in &requests[1..] {
        assert_eq!(sent.path, "/v1/messages?beta=true");
        assert_eq!(
            sent.header("authorization"),
            Some("Bearer token-assertion-one")
        );
        assert!(
            sent.header("anthropic-beta")
                .unwrap()
                .split(',')
                .any(|v| v == OAUTH_BETA)
        );
        assert_eq!(sent.header("x-api-key"), None);
        assert_eq!(sent.header("x-app"), None);
        assert_eq!(sent.header("anthropic-workspace-id"), None);
        assert!(!sent.json()["system"].to_string().contains("Claude Code"));
    }
    std::fs::write(
        req.env_var("ANTHROPIC_IDENTITY_TOKEN_FILE").unwrap(),
        "assertion-two",
    )
    .unwrap();
    req.env
        .insert("ANTHROPIC_WORKSPACE_ID".into(), "workspace-b".into());
    assert_eq!(
        run(&adapter, &req, CancellationToken::new()).await.text(),
        "ok"
    );
    let requests = server.requests();
    assert_eq!(requests[3].json()["workspace_id"], "workspace-b");
    assert_eq!(requests[3].json()["assertion"], "assertion-two");
    assert_eq!(
        requests[4].header("authorization"),
        Some("Bearer token-assertion-two")
    );
}

#[tokio::test]
async fn federation_concurrent_requests_share_refresh_and_canceling_first_waiter_keeps_it_alive() {
    let server = MockServer::start(|r, _| {
        if r.path == "/v1/oauth/token" {
            let mut response = token("shared", 3600);
            let bytes = response.chunks.pop().unwrap();
            response.chunks = vec![bytes[..1].to_vec(), bytes[1..].to_vec()];
            response.pause = Duration::from_millis(60);
            response
        } else {
            MockResponse::sse(vec![simple_reply("ok")])
        }
    })
    .await;
    let (_dir, req) = setup(&server.url);
    let adapter = AnthropicMessages::default();
    let cancel = CancellationToken::new();
    let first = run(&adapter, &req, cancel.clone());
    let cancel_and_join = async {
        until(|| !server.requests().is_empty()).await;
        cancel.cancel();
        futures_util::future::join_all(
            (0..8).map(|_| run(&adapter, &req, CancellationToken::new())),
        )
        .await
    };
    let (first, others) = tokio::join!(first, cancel_and_join);
    assert!(matches!(
        first,
        Message::Assistant {
            stop_reason: StopReason::Aborted,
            ..
        }
    ));
    assert!(others.iter().all(|m| m.text() == "ok"));
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|r| r.path == "/v1/oauth/token")
            .count(),
        1
    );
    assert_eq!(server.requests().len(), 9);
}

#[tokio::test]
async fn federation_advisory_refresh_backs_off_but_mandatory_refresh_reloads_rotated_file() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let server = MockServer::start(move |r, _| {
        if r.path == "/v1/oauth/token" {
            match seen.fetch_add(1, Ordering::SeqCst) {
                0 => token("old", 60),
                1 => MockResponse::json(503, &json!({"error":"temporarily_unavailable"})),
                _ => token(r.json()["assertion"].as_str().unwrap(), 3600),
            }
        } else {
            MockResponse::sse(vec![simple_reply("ok")])
        }
    })
    .await;
    let (_dir, req) = setup(&server.url);
    let adapter = AnthropicMessages::default();
    assert_eq!(
        run(&adapter, &req, CancellationToken::new()).await.text(),
        "ok"
    );
    assert_eq!(
        run(&adapter, &req, CancellationToken::new()).await.text(),
        "ok"
    );
    let state = adapter
        .federation
        .current
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .1
        .clone();
    until(|| state.data.lock().unwrap().advisory_error.is_some()).await;
    assert_eq!(
        run(&adapter, &req, CancellationToken::new()).await.text(),
        "ok"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2, "advisory error backs off");
    // Advance this token to the mandatory window without sleeping through a 30-second TTL.
    state.data.lock().unwrap().cached.as_mut().unwrap().expires =
        Instant::now() + Duration::from_secs(10);
    std::fs::write(
        req.env_var("ANTHROPIC_IDENTITY_TOKEN_FILE").unwrap(),
        "rotated",
    )
    .unwrap();
    assert_eq!(
        run(&adapter, &req, CancellationToken::new()).await.text(),
        "ok"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "mandatory refresh bypasses advisory backoff"
    );
    assert_eq!(
        server.requests().last().unwrap().header("authorization"),
        Some("Bearer rotated")
    );
}

#[tokio::test]
async fn federation_401_invalidates_only_that_token_and_next_request_reexchanges() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let server = MockServer::start(move |r, _| {
        if r.path == "/v1/oauth/token" {
            token(
                &format!("access-{}", seen.fetch_add(1, Ordering::SeqCst)),
                3600,
            )
        } else if r.header("authorization") == Some("Bearer access-0") {
            MockResponse::json(401, &json!({"error":"revoked"}))
        } else {
            MockResponse::sse(vec![simple_reply("ok")])
        }
    })
    .await;
    let (_dir, req) = setup(&server.url);
    let adapter = AnthropicMessages::default();
    assert!(error_text(&run(&adapter, &req, CancellationToken::new()).await).contains("401"));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the pinned Pi request disables SDK retries"
    );
    assert_eq!(
        run(&adapter, &req, CancellationToken::new()).await.text(),
        "ok"
    );
    let state = adapter
        .federation
        .current
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .1
        .clone();
    Lease {
        value: "access-0".into(),
        state,
    }
    .observe(reqwest::StatusCode::UNAUTHORIZED);
    assert_eq!(
        run(&adapter, &req, CancellationToken::new()).await.text(),
        "ok"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "late rejection cannot discard a newer token"
    );
}

#[tokio::test]
async fn federation_does_not_override_explicit_credentials_or_gateway_headers() {
    let server = MockServer::start(|r, _| {
        assert_ne!(r.path, "/v1/oauth/token");
        MockResponse::sse(vec![simple_reply("ok")])
    })
    .await;
    let (_dir, mut req) = setup(&server.url);
    req.env.insert(
        "ANTHROPIC_IDENTITY_TOKEN_FILE".into(),
        "/nonexistent-token".into(),
    );
    for credential in [
        Credential::ApiKey("api".into()),
        Credential::Bearer("bearer".into()),
        Credential::OAuth {
            access: "subscription".into(),
            metadata: Default::default(),
        },
    ] {
        req.credential = credential;
        assert_eq!(
            run(
                &AnthropicMessages::default(),
                &req,
                CancellationToken::new()
            )
            .await
            .text(),
            "ok"
        );
    }
    req.credential = Credential::None;
    req.headers
        .insert("x-api-key".into(), Some("header-key".into()));
    assert_eq!(
        run(
            &AnthropicMessages::default(),
            &req,
            CancellationToken::new()
        )
        .await
        .text(),
        "ok"
    );
    req.headers.clear();
    req.model.provider = "other-gateway".into();
    assert!(
        error_text(
            &run(
                &AnthropicMessages::default(),
                &req,
                CancellationToken::new()
            )
            .await
        )
        .contains("No API key")
    );
    assert_eq!(server.requests().len(), 4);
}

#[tokio::test]
async fn federation_rejects_invalid_files_responses_and_insecure_endpoints_without_model_requests()
{
    let cases = [
        json!({"expires_in":3600}),
        json!({"access_token":"secret","expires_in":"NaN"}),
        json!({"access_token":"secret","expires_in":3600,"token_type":"Basic"}),
    ];
    for body in cases {
        let server = MockServer::start(move |_, _| MockResponse::json(200, &body)).await;
        let (_dir, req) = setup(&server.url);
        let error = error_text(
            &run(
                &AnthropicMessages::default(),
                &req,
                CancellationToken::new(),
            )
            .await,
        );
        assert!(!error.contains("secret"));
        assert_eq!(server.requests().len(), 1);
    }
    let server = MockServer::start(|_, _| MockResponse::json(401, &json!({
        "error":"invalid_grant", "error_description":"assertion-one is invalid", "assertion":"assertion-one", "access_token":"leaked"
    }))).await;
    let (_dir, mut req) = setup(&server.url);
    let error = error_text(
        &run(
            &AnthropicMessages::default(),
            &req,
            CancellationToken::new(),
        )
        .await,
    );
    assert!(error.contains("invalid_grant") && error.contains("ANTHROPIC_WORKSPACE_ID"));
    assert!(!error.contains("assertion-one") && !error.contains("leaked"));
    for contents in [String::new(), "x".repeat(16 * 1024 + 1)] {
        std::fs::write(
            req.env_var("ANTHROPIC_IDENTITY_TOKEN_FILE").unwrap(),
            contents,
        )
        .unwrap();
        let error = error_text(
            &run(
                &AnthropicMessages::default(),
                &req,
                CancellationToken::new(),
            )
            .await,
        );
        assert!(error.contains("empty") || error.contains("16 KiB"));
    }
    req.base_url = Some("http://example.invalid".into());
    assert!(
        error_text(
            &run(
                &AnthropicMessages::default(),
                &req,
                CancellationToken::new()
            )
            .await
        )
        .contains("non-HTTPS")
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn federation_exchange_has_a_total_budget_for_a_stalled_body() {
    let server = MockServer::start(|_, _| {
        let mut response = MockResponse::json(200, &json!({}));
        response.chunks = vec![b"{".to_vec()];
        response.hang = true;
        response
    })
    .await;
    let (_dir, mut req) = setup(&server.url);
    req.timeout = Duration::from_millis(80);
    let message = tokio::time::timeout(
        Duration::from_secs(2),
        run(
            &AnthropicMessages::default(),
            &req,
            CancellationToken::new(),
        ),
    )
    .await
    .unwrap();
    assert!(error_text(&message).contains("token exchange timed out"));
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn federation_wire_and_cache_match_the_executed_pinned_sdk() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../../fixtures/native-anthropic-federation/reference.json"
    ))
    .unwrap();
    for case in fixture["wire"].as_array().unwrap() {
        let server = MockServer::start(|_, _| token("captured-token", 3600)).await;
        let (_dir, mut req) = setup(&server.url);
        for (env, key) in [
            ("ANTHROPIC_WORKSPACE_ID", "workspace_id"),
            ("ANTHROPIC_SERVICE_ACCOUNT_ID", "service_account_id"),
        ] {
            if let Some(value) = case["body"][key].as_str() {
                req.env.insert(env.into(), value.into());
            }
        }
        let cache = Cache::default();
        let http = crate::provider::http::client();
        let ctx = StreamCtx {
            req: &req,
            http: &http,
            events: None,
            cancel: CancellationToken::new(),
        };
        assert_eq!(
            cache
                .get(Config::from_request(&req).unwrap(), &ctx)
                .await
                .unwrap()
                .value,
            case["token"]["token"]
        );
        let sent = &server.requests()[0];
        assert_eq!(sent.json(), case["body"]);
        assert_eq!(sent.method, case["method"]);
        assert_eq!(sent.path, "/v1/oauth/token");
        for name in ["Content-Type", "anthropic-beta"] {
            assert_eq!(sent.header(name), case["headers"][name].as_str());
        }
    }
    let count = Arc::new(AtomicUsize::new(0));
    let captured = count.clone();
    let server = MockServer::start(move |_, _| {
        token(
            &format!("token-{}", captured.fetch_add(1, Ordering::SeqCst) + 1),
            3600,
        )
    })
    .await;
    let (_dir, req) = setup(&server.url);
    let cache = Cache::default();
    let http = crate::provider::http::client();
    let ctx = StreamCtx {
        req: &req,
        http: &http,
        events: None,
        cancel: CancellationToken::new(),
    };
    let mut previous: Option<Lease> = None;
    for case in fixture["cacheCases"].as_array().unwrap() {
        match case["label"].as_str().unwrap() {
            "advisory" | "mandatory" => {
                let remaining = if case["label"] == "advisory" { 100 } else { 10 };
                let state = cache.current.lock().unwrap().as_ref().unwrap().1.clone();
                state.data.lock().unwrap().cached.as_mut().unwrap().expires =
                    Instant::now() + Duration::from_secs(remaining);
            }
            "invalidated" => previous
                .as_ref()
                .unwrap()
                .observe(reqwest::StatusCode::UNAUTHORIZED),
            _ => {}
        }
        let lease = cache
            .get(Config::from_request(&req).unwrap(), &ctx)
            .await
            .unwrap();
        assert_eq!(lease.value, case["token"], "{}", case["label"]);
        if case["label"] == "advisory" {
            until(|| lease.state.data.lock().unwrap().pending.is_none()).await;
        }
        assert_eq!(
            count.load(Ordering::SeqCst) as u64,
            case["calls"].as_u64().unwrap()
        );
        previous = Some(lease);
    }
}
