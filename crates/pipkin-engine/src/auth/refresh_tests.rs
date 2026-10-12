//! Stored OAuth credentials through real refresh HTTP exchanges and the atomic auth store.
use super::tests::{Fixture, form, http, model};
use super::*;
use crate::provider::mock::{MockResponse, MockServer};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn copilot_models() -> Vec<String> {
    crate::provider::model::builtin_models()
        .into_iter()
        .filter(|m| m.provider == "github-copilot")
        .map(|m| m.id)
        .collect()
}
fn expired_copilot(f: &Fixture, ids: &[String]) {
    f.write_auth(&json!({"github-copilot":{"type":"oauth","access":"old-access","refresh":"github-refresh","expires":1,"enterpriseUrl":"https://company.ghe.com/path","availableModelIds":ids},"unrelated":{"type":"api_key","key":"preserved"}}));
}
fn catalog_ids(auth: &Auth) -> Vec<String> {
    auth.catalog()
        .models
        .iter()
        .filter(|m| m.provider == "github-copilot")
        .map(|m| m.id.clone())
        .collect()
}
async fn wait(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "refresh did not finish");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn copilot_refresh_deduplicates_persists_rotated_access_and_catalog_and_survives_reopen() {
    let ids = copilot_models();
    let new_id = ids[1].clone();
    let expected = new_id.clone();
    let expires_at = crate::util::now_ms() / 1000 + 7200;
    let server = MockServer::start(move |req, _| match req.path.as_str() {
        "/copilot_internal/v2/token" => {
            assert_eq!(req.header("authorization"), Some("Bearer github-refresh"));
            assert_eq!(req.header("editor-version"), Some("vscode/1.107.0"));
            MockResponse::json(200, &json!({"token":"new-access","expires_at":expires_at}))
        }
        "/models" => {
            assert_eq!(req.header("authorization"), Some("Bearer new-access"));
            assert_eq!(req.header("x-github-api-version"), Some("2026-06-01"));
            MockResponse::json(
                200,
                &json!({"data":[{"id":new_id,"model_picker_enabled":true}]}),
            )
        }
        other => panic!("unexpected refresh request: {other}"),
    })
    .await;
    let f = Fixture::new(&[
        ("PIPKIN_OAUTH_GITHUB_API_URL", &server.url),
        ("PIPKIN_OAUTH_COPILOT_API_URL", &server.url),
    ]);
    expired_copilot(&f, &ids[..1]);
    let auth = Arc::new(f.auth());
    assert_eq!(catalog_ids(&auth), ids[..1]);
    let selected = model(&auth, "github-copilot");
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let (auth, selected) = (auth.clone(), selected.clone());
        tasks.push(tokio::spawn(async move {
            auth.resolve(&selected, &http()).await
        }));
    }
    for task in tasks {
        let resolved = task.await.unwrap().unwrap();
        assert_eq!(resolved.credential.secret(), Some("new-access"));
        assert_eq!(
            resolved.base_url.as_deref(),
            Some("https://copilot-api.company.ghe.com")
        );
    }
    assert_eq!(
        server.requests().len(),
        2,
        "one token and one catalog exchange"
    );
    let saved = f.read_auth();
    assert_eq!(saved["github-copilot"]["refresh"], "github-refresh");
    assert_eq!(
        saved["github-copilot"]["expires"],
        expires_at * 1000 - 300_000
    );
    assert_eq!(saved["github-copilot"]["enterpriseUrl"], "company.ghe.com");
    assert_eq!(
        saved["github-copilot"]["availableModelIds"],
        json!([expected])
    );
    assert_eq!(saved["unrelated"]["key"], "preserved");
    assert_eq!(catalog_ids(&auth), std::slice::from_ref(&expected));
    drop(auth);
    let reopened = f.auth();
    assert_eq!(catalog_ids(&reopened), [expected]);
    reopened
        .resolve(&model(&reopened, "github-copilot"), &http())
        .await
        .unwrap();
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn copilot_refresh_failure_preserves_credentials_and_never_retries_or_falls_back() {
    for scenario in [
        "token-denied",
        "token-invalid",
        "catalog-rate-limit",
        "catalog-invalid",
    ] {
        let server = MockServer::start(move |req, _| match req.path.as_str() {
            "/copilot_internal/v2/token" => match scenario {
                "token-denied" => MockResponse::json(401, &json!({"error":"denied"})),
                "token-invalid" => MockResponse::json(200, &json!({"token":"incomplete"})),
                _ => MockResponse::json(
                    200,
                    &json!({"token":"new-access","expires_at": crate::util::now_ms()/1000+3600}),
                ),
            },
            "/models" if scenario == "catalog-rate-limit" => {
                MockResponse::json(429, &json!({"error":"rate limit"}))
                    .with_header("retry-after", "0")
            }
            "/models" => MockResponse::json(200, &json!({"invalid":true})),
            other => panic!("{other}"),
        })
        .await;
        let f = Fixture::new(&[
            ("PIPKIN_OAUTH_GITHUB_API_URL", &server.url),
            ("PIPKIN_OAUTH_COPILOT_API_URL", &server.url),
        ]);
        expired_copilot(&f, &copilot_models()[..1]);
        f.write_models(&json!({"providers":{"github-copilot":{"apiKey":"must-not-fallback"}}}));
        let before = std::fs::read(f.auth_path()).unwrap();
        let auth = f.auth();
        let error = auth
            .resolve(&model(&auth, "github-copilot"), &http())
            .await
            .unwrap_err();
        assert!(
            error
                .message()
                .starts_with("OAuth refresh failed for github-copilot:"),
            "{scenario}: {error:?}"
        );
        assert!(!error.message().contains("must-not-fallback"));
        assert_eq!(std::fs::read(f.auth_path()).unwrap(), before);
        assert_eq!(
            server.requests().len(),
            if scenario.starts_with("token-") { 1 } else { 2 }
        );
    }
}

#[tokio::test]
async fn cancelled_copilot_refresh_caller_cannot_leave_the_catalog_stale() {
    let ids = copilot_models();
    let new_id = ids[1].clone();
    let expected = new_id.clone();
    let server = MockServer::start(move |req, _| match req.path.as_str() {
        "/copilot_internal/v2/token" => MockResponse::json(
            200,
            &json!({"token":"new-access","expires_at": crate::util::now_ms()/1000+3600}),
        ),
        "/models" => {
            let mut response = MockResponse::json(
                200,
                &json!({"data":[{"id":new_id,"model_picker_enabled":true}]}),
            );
            let body = response.chunks.pop().unwrap();
            response.chunks = vec![body[..1].to_vec(), body[1..].to_vec()];
            response.pause = Duration::from_millis(250);
            response
        }
        other => panic!("{other}"),
    })
    .await;
    let f = Fixture::new(&[
        ("PIPKIN_OAUTH_GITHUB_API_URL", &server.url),
        ("PIPKIN_OAUTH_COPILOT_API_URL", &server.url),
    ]);
    expired_copilot(&f, &ids[..1]);
    let auth = Arc::new(f.auth());
    let owned = auth.clone();
    let selected = model(&auth, "github-copilot");
    let caller = tokio::spawn(async move { owned.resolve(&selected, &http()).await });
    wait(|| server.requests().len() == 2).await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    wait(|| f.read_auth()["github-copilot"]["access"] == "new-access").await;
    wait(|| catalog_ids(&auth) == [expected.clone()]).await;
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn radius_refresh_rotates_tokens_on_its_gateway_and_persists_expiry_scope() {
    for provider in ["radius", "team"] {
        let server = MockServer::start(|req,_| {
            assert_eq!(req.path,"/v1/oauth/token");
            let body = form(req);
            assert_eq!(body["grant_type"], "refresh_token");
            assert_eq!(body["client_id"], "pi-gateway");
            assert_eq!(body["refresh_token"], "old-refresh");
            MockResponse::json(200,&json!({"access_token":"new-access","refresh_token":"rotated-refresh","expires_in":3600,"scope":"gateway offline_access"}))
        }).await;
        let f = Fixture::new(&[("PI_RADIUS_GATEWAY", &server.url)]);
        if provider == "team" {
            f.write_models(&json!({"providers":{"team":{"oauth":"radius","baseUrl":format!("{}/v1", server.url),"models":[{"id":"m","api":"pi-messages"}]}}}));
        }
        f.write_auth(&json!({provider:{"type":"oauth","access":"old-access","refresh":"old-refresh","expires":1},"unrelated":{"type":"api_key","key":"preserved"}}));
        let auth = Arc::new(f.auth());
        let before = crate::util::now_ms();
        let selected = model(&auth, provider);
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let (auth, selected) = (auth.clone(), selected.clone());
            tasks.push(tokio::spawn(async move {
                auth.resolve(&selected, &http()).await
            }));
        }
        for task in tasks {
            assert_eq!(
                task.await.unwrap().unwrap().credential.secret(),
                Some("new-access")
            );
        }
        let after = crate::util::now_ms();
        let saved = f.read_auth();
        assert_eq!(saved[provider]["refresh"], "rotated-refresh");
        assert_eq!(saved[provider]["scope"], "gateway offline_access");
        assert_eq!(saved["unrelated"]["key"], "preserved");
        assert!(
            (before + 3_540_000..=after + 3_540_000)
                .contains(&saved[provider]["expires"].as_i64().unwrap())
        );
        drop(auth);
        let reopened = f.auth();
        assert_eq!(
            reopened
                .resolve(&model(&reopened, provider), &http())
                .await
                .unwrap()
                .credential
                .secret(),
            Some("new-access")
        );
        assert_eq!(server.requests().len(), 1);
    }
}

#[tokio::test]
async fn radius_refresh_failure_preserves_rotating_credentials_without_billing_fallback() {
    for response in [
        MockResponse::json(400, &json!({"error":"invalid_grant"})),
        MockResponse::json(
            200,
            &json!({"access_token":"missing-refresh","expires_in":3600}),
        ),
        MockResponse::json(429, &json!({"error":"rate_limit"})).with_header("retry-after", "0"),
    ] {
        let server = MockServer::start(move |_, _| response.clone()).await;
        let f = Fixture::new(&[
            ("PI_RADIUS_GATEWAY", &server.url),
            ("RADIUS_API_KEY", "must-not-fallback"),
        ]);
        f.write_auth(
            &json!({"radius":{"type":"oauth","access":"old","refresh":"rotating","expires":1}}),
        );
        let before = std::fs::read(f.auth_path()).unwrap();
        let auth = f.auth();
        let error = auth
            .resolve(&model(&auth, "radius"), &http())
            .await
            .unwrap_err();
        assert!(
            error
                .message()
                .starts_with("OAuth refresh failed for radius:"),
            "{error:?}"
        );
        assert!(!error.message().contains("must-not-fallback"));
        assert_eq!(std::fs::read(f.auth_path()).unwrap(), before);
        assert_eq!(server.requests().len(), 1);
    }
}
