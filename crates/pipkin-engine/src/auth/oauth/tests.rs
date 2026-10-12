//! Every OAuth flow, end to end through the sign-in lifecycle, against the local mock server.
//! The "browser" is the test itself: it follows the published URL to the loopback callback.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use pipkin_engine_protocol::AuthStatus;
use serde_json::{Value, json};

use crate::auth::signin::MSG_DONE;
use crate::auth::tests::{Fixture, form, http, model, wait_for};
use crate::auth::{Auth, store::StoredCredential};
use crate::provider::Credential;
use crate::provider::mock::{MockResponse, MockServer, Recorded};

fn fast(extra: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = vec![
        ("PIPKIN_OAUTH_CALLBACK_PORT".into(), "0".into()),
        ("PIPKIN_OAUTH_POLL_INTERVAL_MS".into(), "1".into()),
        ("PIPKIN_OAUTH_RETRY_BASE_MS".into(), "1".into()),
    ];
    v.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    v
}

fn fixture(extra: &[(&str, &str)]) -> Fixture {
    let vars = fast(extra);
    let refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    Fixture::new(&refs)
}

fn query(url: &str) -> HashMap<String, String> {
    url::Url::parse(url)
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect()
}

/// Follow a loopback redirect like a browser would (always via 127.0.0.1).
async fn browser_get(url: &str) -> (u16, String) {
    let url = url.replace("://localhost:", "://127.0.0.1:");
    let resp = http().get(url).send().await.unwrap();
    let status = resp.status().as_u16();
    (status, resp.text().await.unwrap())
}

fn jwt(payload: &Value) -> String {
    let b = |v: &[u8]| super::pkce::b64url(v);
    format!(
        "{}.{}.sig",
        b(br#"{"alg":"none"}"#),
        b(payload.to_string().as_bytes())
    )
}

async fn answer(auth: &Auth, client: u64, kind: &str, response: &str) {
    let s = wait_for(auth, client, kind, |s| {
        s.challenge.as_ref().is_some_and(|c| c.kind == kind)
    })
    .await;
    auth.answer(
        client,
        s.attempt.as_deref().unwrap(),
        &s.challenge.unwrap().id,
        response.into(),
    )
    .unwrap();
}

async fn done(auth: &Auth, client: u64) {
    let s = wait_for(auth, client, "done", |s| {
        matches!(s.status, AuthStatus::Done | AuthStatus::Failed)
    })
    .await;
    assert_eq!((s.status, s.message.as_str()), (AuthStatus::Done, MSG_DONE));
    assert!(s.url.is_none() && s.device_code.is_none() && s.challenge.is_none());
}

fn stored(f: &Fixture, provider: &str) -> crate::auth::store::OAuthCredential {
    let v = f.read_auth();
    match StoredCredential::from_value(&v[provider]) {
        StoredCredential::OAuth(c) => c,
        other => panic!("{other:?}"),
    }
}

fn codex_token(n: usize) -> Value {
    json!({
        "access_token": jwt(&json!({"https://api.openai.com/auth": {"chatgpt_account_id": "acct-1"}, "n": n})),
        "refresh_token": format!("refresh-{n}"),
        "expires_in": 3600
    })
}

#[tokio::test]
async fn openai_codex_browser_login_through_the_loopback_callback() {
    let server = MockServer::start(|req, n| {
        assert_eq!(req.path, "/oauth/token");
        MockResponse::json(200, &codex_token(n))
    })
    .await;
    let f = fixture(&[("PIPKIN_OAUTH_OPENAI_URL", &server.url)]);
    let auth = f.auth();
    auth.start(1, "openai-codex").await.unwrap();
    answer(&auth, 1, "select", "browser").await;
    let s = wait_for(&auth, 1, "auth url and paste prompt", |s| {
        s.url.is_some() && s.challenge.is_some()
    })
    .await;
    assert_eq!(
        s.status,
        AuthStatus::Prompt,
        "the manual paste prompt races the callback"
    );
    let url = s.url.unwrap();
    assert!(url.starts_with(&format!("{}/oauth/authorize?", server.url)));
    let q = query(&url);
    assert_eq!(q["client_id"], "app_EMoamEEZ73f0CkXaXp7hrann");
    assert_eq!(q["code_challenge_method"], "S256");
    assert_eq!(q["originator"], "pi");
    let redirect = q["redirect_uri"].clone();
    assert!(redirect.starts_with("http://localhost:") && redirect.ends_with("/auth/callback"));
    // Wrong route and wrong state are rejected without finishing the sign-in.
    let base = redirect.trim_end_matches("/auth/callback");
    assert_eq!(browser_get(&format!("{base}/elsewhere")).await.0, 404);
    assert_eq!(
        browser_get(&format!("{redirect}?code=c&state=wrong"))
            .await
            .0,
        400
    );
    let (status, page) =
        browser_get(&format!("{redirect}?code=the-code&state={}", q["state"])).await;
    assert_eq!(status, 200, "{page}");
    done(&auth, 1).await;
    // The loopback server is closed once the sign-in finished.
    assert!(
        http()
            .get(redirect.replace("://localhost:", "://127.0.0.1:"))
            .send()
            .await
            .is_err()
    );
    let req = &server.requests()[0];
    let body = form(req);
    assert_eq!(body["grant_type"], "authorization_code");
    assert_eq!(body["code"], "the-code");
    assert_eq!(body["redirect_uri"], redirect);
    assert_eq!(
        super::pkce::challenge_for(&body["code_verifier"]),
        q["code_challenge"]
    );
    let c = stored(&f, "openai-codex");
    assert_eq!(c.refresh, "refresh-0");
    assert_eq!(c.extra["accountId"], "acct-1");
    // The credential resolves for requests and refreshes through the same endpoint.
    let r = auth
        .resolve(&model(&auth, "openai-codex"), &http())
        .await
        .unwrap();
    let Credential::OAuth { access, metadata } = r.credential else {
        panic!()
    };
    assert_eq!(access, c.access);
    assert_eq!(metadata["accountId"], "acct-1");
}

#[tokio::test]
async fn openai_codex_manual_paste_and_refresh() {
    let server = MockServer::start(|_, n| MockResponse::json(200, &codex_token(n))).await;
    let f = fixture(&[("PIPKIN_OAUTH_OPENAI_URL", &server.url)]);
    let auth = f.auth();
    auth.start(1, "openai-codex").await.unwrap();
    answer(&auth, 1, "select", "browser").await;
    let s = wait_for(&auth, 1, "manual prompt", |s| {
        s.challenge
            .as_ref()
            .is_some_and(|c| c.kind == "manual_code")
    })
    .await;
    let state = query(s.url.as_deref().unwrap())["state"].clone();
    let ch = s.challenge.unwrap();
    auth.answer(
        1,
        s.attempt.as_deref().unwrap(),
        &ch.id,
        format!("pasted-code#{state}"),
    )
    .unwrap();
    done(&auth, 1).await;
    assert_eq!(form(&server.requests()[0])["code"], "pasted-code");
    // Refresh near expiry, persisted with the new account data.
    let mut c = stored(&f, "openai-codex");
    c.expires = crate::util::now_ms();
    f.write_auth(&json!({"openai-codex": StoredCredential::OAuth(c).to_value()}));
    let r = auth
        .resolve(&model(&auth, "openai-codex"), &http())
        .await
        .unwrap();
    let body = form(&server.requests()[1]);
    assert_eq!(body["grant_type"], "refresh_token");
    assert_eq!(body["refresh_token"], "refresh-0");
    assert_eq!(stored(&f, "openai-codex").refresh, "refresh-1");
    assert_eq!(
        r.credential.secret(),
        Some(stored(&f, "openai-codex").access.as_str())
    );
}

#[tokio::test]
async fn a_taken_callback_port_means_paste_for_codex_and_an_error_for_chatgpt() {
    let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = busy.local_addr().unwrap().port().to_string();
    let server = MockServer::start(|_, n| MockResponse::json(200, &codex_token(n))).await;
    let f = fixture(&[
        ("PIPKIN_OAUTH_OPENAI_URL", &server.url),
        ("PIPKIN_OAUTH_CALLBACK_PORT", &port),
    ]);
    let auth = f.auth();
    auth.start(1, "openai-codex").await.unwrap();
    answer(&auth, 1, "select", "browser").await;
    let s = wait_for(&auth, 1, "manual prompt", |s| {
        s.challenge
            .as_ref()
            .is_some_and(|c| c.kind == "manual_code")
    })
    .await;
    let q = query(s.url.as_deref().unwrap());
    assert_eq!(
        q["redirect_uri"],
        format!("http://localhost:{port}/auth/callback")
    );
    auth.answer(
        1,
        s.attempt.as_deref().unwrap(),
        &s.challenge.unwrap().id,
        format!(
            "http://localhost:{port}/auth/callback?code=p&state={}",
            q["state"]
        ),
    )
    .unwrap();
    done(&auth, 1).await;
    // A pasted URL with another state is refused.
    auth.start(1, "openai-codex").await.unwrap();
    answer(&auth, 1, "select", "browser").await;
    answer(&auth, 1, "manual_code", "p#not-the-state").await;
    let s = wait_for(&auth, 1, "failure", |s| s.status == AuthStatus::Failed).await;
    assert_eq!(s.message, crate::auth::signin::MSG_FAILED);

    auth.start(2, "openai").await.unwrap();
    let s = wait_for(&auth, 2, "failure", |s| s.status == AuthStatus::Failed).await;
    assert_eq!(s.message, crate::auth::signin::MSG_FAILED);
    drop(busy);
}

#[tokio::test]
async fn openai_codex_device_code_login() {
    let polls = Arc::new(AtomicUsize::new(0));
    let p = polls.clone();
    let server = MockServer::start(move |req: &Recorded, _| match req.path.as_str() {
        "/api/accounts/deviceauth/usercode" => {
            assert_eq!(req.json()["client_id"], "app_EMoamEEZ73f0CkXaXp7hrann");
            MockResponse::json(
                200,
                &json!({"device_auth_id": "dev-1", "user_code": "ABCD-EFGH", "interval": "0"}),
            )
        }
        "/api/accounts/deviceauth/token" => {
            assert_eq!(
                req.json(),
                json!({"device_auth_id": "dev-1", "user_code": "ABCD-EFGH"})
            );
            match p.fetch_add(1, Ordering::SeqCst) {
                0 => MockResponse::json(403, &json!({})),
                1 => MockResponse::json(
                    400,
                    &json!({"error": {"code": "deviceauth_authorization_pending"}}),
                ),
                _ => MockResponse::json(
                    200,
                    &json!({"authorization_code": "dc", "code_verifier": "dv"}),
                ),
            }
        }
        "/oauth/token" => MockResponse::json(200, &codex_token(0)),
        other => panic!("{other}"),
    })
    .await;
    let f = fixture(&[("PIPKIN_OAUTH_OPENAI_URL", &server.url)]);
    let auth = f.auth();
    auth.start(1, "openai-codex").await.unwrap();
    answer(&auth, 1, "select", "device_code").await;
    let s = wait_for(&auth, 1, "device code", |s| s.device_code.is_some()).await;
    assert_eq!(s.device_code.as_deref(), Some("ABCD-EFGH"));
    assert_eq!(s.url, Some(format!("{}/codex/device", server.url)));
    done(&auth, 1).await;
    assert_eq!(polls.load(Ordering::SeqCst), 3);
    let exchange = server
        .requests()
        .into_iter()
        .find(|r| r.path == "/oauth/token")
        .unwrap();
    let body = form(&exchange);
    assert_eq!(
        (body["code"].as_str(), body["code_verifier"].as_str()),
        ("dc", "dv")
    );
    assert_eq!(
        body["redirect_uri"],
        format!("{}/deviceauth/callback", server.url)
    );
}

#[tokio::test]
async fn sign_in_with_chatgpt_registers_a_client_and_refreshes_with_it() {
    let server = MockServer::start(|req, n| {
        assert_eq!(req.path, "/api/accounts/oauth/token");
        let mut body = json!({"access_token": format!("at-{n}"), "refresh_token": format!("rt-{n}"),
            "expires_in": 3600, "scope": "openid resource.invoke chatgpt.tokens.use.direct"});
        if n == 0 {
            body["id_token"] = json!("idt");
        }
        MockResponse::json(200, &body)
    })
    .await;
    let f = fixture(&[("PIPKIN_OAUTH_OPENAI_URL", &server.url)]);
    let auth = f.auth();
    auth.start(1, "openai").await.unwrap();
    let s = wait_for(&auth, 1, "auth url", |s| s.url.is_some()).await;
    let q = query(s.url.as_deref().unwrap());
    let settings: Value =
        serde_json::from_str(&std::fs::read_to_string(f.config.settings_path()).unwrap()).unwrap();
    let device = settings["deviceId"].as_str().unwrap();
    assert_eq!(q["ext_agent_host_id"], format!("urn:uuid:{device}"));
    assert_eq!(q["client_id"], "dynamic_agent_client");
    assert_eq!(q["resource"], "https://api.openai.com/v1");
    let redirect = q["redirect_uri"].clone();
    assert!(redirect.starts_with("http://127.0.0.1:"));
    // A callback without the issued client id keeps waiting.
    assert_eq!(
        browser_get(&format!("{redirect}?code=c&state={}", q["state"]))
            .await
            .0,
        400
    );
    let (status, _) = browser_get(&format!(
        "{redirect}?code=c&state={}&client_id=issued-client",
        q["state"]
    ))
    .await;
    assert_eq!(status, 200);
    done(&auth, 1).await;
    let body = form(&server.requests()[0]);
    assert_eq!(body["client_id"], "issued-client");
    assert_eq!(body["resource"], "https://api.openai.com/v1");
    let mut c = stored(&f, "openai");
    assert_eq!(c.extra["clientId"], "issued-client");
    assert_eq!(c.extra["scopes"][2], "chatgpt.tokens.use.direct");
    c.expires = 0;
    f.write_auth(&json!({"openai": StoredCredential::OAuth(c).to_value()}));
    let r = auth
        .resolve(&model(&auth, "openai"), &http())
        .await
        .unwrap();
    assert_eq!(r.credential.secret(), Some("at-1"));
    let body = form(&server.requests()[1]);
    assert_eq!(
        (
            body["grant_type"].as_str(),
            body["client_id"].as_str(),
            body["refresh_token"].as_str()
        ),
        ("refresh_token", "issued-client", "rt-0")
    );
    // The same device id is reused next time.
    auth.start(2, "openai").await.unwrap();
    wait_for(&auth, 2, "auth url", |s| s.url.is_some()).await;
    let again: Value =
        serde_json::from_str(&std::fs::read_to_string(f.config.settings_path()).unwrap()).unwrap();
    assert_eq!(again["deviceId"], settings["deviceId"]);
}

#[tokio::test]
async fn github_copilot_device_flow_reads_models_and_enables_policies() {
    let known: Vec<String> = crate::provider::model::builtin_models()
        .into_iter()
        .filter(|m| m.provider == "github-copilot")
        .map(|m| m.id)
        .collect();
    let (a, b) = (known[0].clone(), known[1].clone());
    let polls = Arc::new(AtomicUsize::new(0));
    let p = polls.clone();
    let (ma, mb) = (a.clone(), b.clone());
    let server = MockServer::start(move |req: &Recorded, _| match req.path.as_str() {
        "/login/device/code" => {
            assert_eq!(form(req)["scope"], "read:user");
            MockResponse::json(200, &json!({"device_code": "dc", "user_code": "U-1",
                "verification_uri": "https://github.com/login/device", "interval": 0, "expires_in": 900}))
        }
        "/login/oauth/access_token" => match p.fetch_add(1, Ordering::SeqCst) {
            0 => MockResponse::json(200, &json!({"error": "authorization_pending"})),
            1 => MockResponse::json(200, &json!({"error": "slow_down", "interval": 0})),
            _ => {
                assert_eq!(form(req)["grant_type"], "urn:ietf:params:oauth:grant-type:device_code");
                MockResponse::json(200, &json!({"access_token": "gho_user"}))
            }
        },
        "/copilot_internal/v2/token" => {
            assert_eq!(req.header("authorization"), Some("Bearer gho_user"));
            assert_eq!(req.header("editor-version"), Some("vscode/1.107.0"));
            MockResponse::json(200, &json!({"token": "tid=1;proxy-ep=proxy.enterprise.githubcopilot.com;",
                "expires_at": crate::util::now_ms() / 1000 + 1800}))
        }
        "/models" => {
            assert_eq!(req.header("x-github-api-version"), Some("2026-06-01"));
            MockResponse::json(200, &json!({"data": [
                {"id": ma, "model_picker_enabled": true},
                {"id": mb, "model_picker_enabled": true, "policy": {"state": "unconfigured"}},
                {"id": "unknown", "model_picker_enabled": true, "policy": {"state": "disabled"}}
            ]}))
        }
        path if path.ends_with("/policy") => {
            assert_eq!(req.body, br#"{"state":"enabled"}"#);
            MockResponse::json(200, &json!({}))
        }
        other => panic!("{other}"),
    })
    .await;
    let f = fixture(&[
        ("PIPKIN_OAUTH_GITHUB_URL", &server.url),
        ("PIPKIN_OAUTH_GITHUB_API_URL", &server.url),
        ("PIPKIN_OAUTH_COPILOT_API_URL", &server.url),
    ]);
    let auth = f.auth();
    auth.start(1, "github-copilot").await.unwrap();
    answer(&auth, 1, "text", "").await;
    let s = wait_for(&auth, 1, "device code", |s| s.device_code.is_some()).await;
    assert_eq!(s.url.as_deref(), Some("https://github.com/login/device"));
    done(&auth, 1).await;
    assert!(
        server
            .requests()
            .iter()
            .any(|r| r.path == format!("/models/{b}/policy"))
    );
    let c = stored(&f, "github-copilot");
    assert_eq!(c.refresh, "gho_user");
    assert_eq!(c.extra["availableModelIds"], json!([a, b]));
    // The catalog is filtered and requests go to the token's endpoint.
    let ids: Vec<String> = auth
        .catalog()
        .models
        .iter()
        .filter(|m| m.provider == "github-copilot")
        .map(|m| m.id.clone())
        .collect();
    assert_eq!(ids.len(), 2);
    let r = auth
        .resolve(&model(&auth, "github-copilot"), &http())
        .await
        .unwrap();
    assert_eq!(
        r.base_url.as_deref(),
        Some("https://api.enterprise.githubcopilot.com")
    );
}

#[tokio::test]
async fn openrouter_exchanges_the_code_for_a_key_before_the_page() {
    let server = MockServer::start(|req: &Recorded, _| {
        assert_eq!(req.path, "/api/v1/auth/keys");
        let body = req.json();
        assert_eq!(body["code"], "or-code");
        assert_eq!(body["code_challenge_method"], "S256");
        MockResponse::json(200, &json!({"key": "sk-or-v1"}))
    })
    .await;
    let f = fixture(&[("PIPKIN_OAUTH_OPENROUTER_URL", &server.url)]);
    let auth = f.auth();
    auth.start(1, "openrouter").await.unwrap();
    let s = wait_for(&auth, 1, "auth url", |s| s.url.is_some()).await;
    let q = query(s.url.as_deref().unwrap());
    let callback = q["callback_url"].clone();
    assert!(callback.contains("/oauth/callback/"));
    let (status, page) = browser_get(&format!("{callback}?code=or-code")).await;
    assert_eq!(status, 200);
    assert!(page.contains("Signed in to OpenRouter"));
    done(&auth, 1).await;
    let body = server.requests()[0].json();
    assert_eq!(
        super::pkce::challenge_for(body["code_verifier"].as_str().unwrap()),
        q["code_challenge"]
    );
    let c = stored(&f, "openrouter");
    assert_eq!(
        (c.access.as_str(), c.expires),
        ("sk-or-v1", super::openrouter::NEVER)
    );
    let r = auth
        .resolve(&model(&auth, "openrouter"), &http())
        .await
        .unwrap();
    assert_eq!(r.credential.secret(), Some("sk-or-v1"));
}

#[tokio::test]
async fn kimi_device_flow_header_auth_and_refresh_retries() {
    let polls = Arc::new(AtomicUsize::new(0));
    let refreshes = Arc::new(AtomicUsize::new(0));
    // Keep authorization pending until the client actually observes the code. The
    // accelerated test clock can otherwise finish login between snapshot polls.
    let approved = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let approval = approved.clone();
    let (p, r) = (polls.clone(), refreshes.clone());
    let server = MockServer::start(move |req: &Recorded, _| match req.path.as_str() {
        "/api/oauth/device_authorization" => MockResponse::json(200, &json!({
            "device_code": "kdc", "user_code": "K-1",
            "verification_uri": "https://kimi.example/device",
            "verification_uri_complete": "https://kimi.example/device?code=K-1",
            "interval": 5, "expires_in": 900})),
        "/api/oauth/token" => {
            let body = form(req);
            if body["grant_type"] == "refresh_token" {
                return match r.fetch_add(1, Ordering::SeqCst) {
                    0 | 1 => MockResponse::json(503, &json!({})),
                    _ => MockResponse::json(200, &json!({"access_token": "k-at-2", "refresh_token": "k-rt-2", "expires_in": 3600})),
                };
            }
            match p.fetch_add(1, Ordering::SeqCst) {
                n if n == 0 || !approval.load(Ordering::SeqCst) => MockResponse::json(400, &json!({"error": "authorization_pending"})),
                _ => MockResponse::json(200, &json!({"access_token": "k-at", "refresh_token": "k-rt", "expires_in": 3600})),
            }
        }
        other => panic!("{other}"),
    })
    .await;
    let f = fixture(&[("KIMI_CODE_OAUTH_HOST", &server.url)]);
    let auth = f.auth();
    auth.start(1, "kimi-coding").await.unwrap();
    let s = wait_for(&auth, 1, "device code", |s| s.device_code.is_some()).await;
    assert_eq!(
        s.url.as_deref(),
        Some("https://kimi.example/device?code=K-1")
    );
    approved.store(true, Ordering::SeqCst);
    done(&auth, 1).await;
    assert!(polls.load(Ordering::SeqCst) >= 2);
    let m = model(&auth, "kimi-coding");
    let res = auth.resolve(&m, &http()).await.unwrap();
    assert_eq!(res.credential, Credential::None);
    assert_eq!(res.headers["Authorization"].as_deref(), Some("Bearer k-at"));
    let mut c = stored(&f, "kimi-coding");
    c.expires = 0;
    f.write_auth(&json!({"kimi-coding": StoredCredential::OAuth(c).to_value()}));
    let res = auth.resolve(&m, &http()).await.unwrap();
    assert_eq!(
        res.headers["Authorization"].as_deref(),
        Some("Bearer k-at-2")
    );
    assert_eq!(refreshes.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn meta_device_flow_mints_a_key_and_a_dead_session_fails() {
    let mints = Arc::new(AtomicUsize::new(0));
    let m2 = mints.clone();
    let server = MockServer::start(move |req: &Recorded, _| match req.path.as_str() {
        "/oidc/device/authorization/" => MockResponse::json(200, &json!({
            "device_code": "mdc", "user_code": "M-1", "verification_uri": "https://meta.example/device"})),
        "/oidc/device/token/" => MockResponse::json(200, &json!({"access_token": "identity"})),
        "/muse-code/key" => {
            assert_eq!(req.header("authorization"), Some("Bearer identity"));
            assert_eq!(req.header("x-api-version"), Some("1.0.0"));
            match m2.fetch_add(1, Ordering::SeqCst) {
                0 => MockResponse::json(200, &json!({"api_key": "meta-key"})),
                _ => MockResponse::json(401, &json!({"detail": "expired"})),
            }
        }
        other => panic!("{other}"),
    })
    .await;
    let f = fixture(&[
        ("PIPKIN_OAUTH_META_AUTH_URL", &server.url),
        ("PIPKIN_OAUTH_META_API_URL", &server.url),
        ("META_API_KEY", "fallback-key"),
    ]);
    let auth = f.auth();
    auth.start(1, "meta").await.unwrap();
    done(&auth, 1).await;
    let mut c = stored(&f, "meta");
    assert_eq!(
        (c.access.as_str(), c.refresh.as_str()),
        ("meta-key", "identity")
    );
    c.expires = 0;
    f.write_auth(&json!({"meta": StoredCredential::OAuth(c).to_value()}));
    let err = auth
        .resolve(&model(&auth, "meta"), &http())
        .await
        .unwrap_err();
    assert!(
        err.message().contains("Meta session expired (status 401)"),
        "{err:?}"
    );
}

#[tokio::test]
async fn radius_browser_login_against_the_configured_gateway() {
    let server = MockServer::start(|req: &Recorded, _| match req.path.as_str() {
        "/v1/oauth" => MockResponse::json(200, &json!({"authorizationEndpoint": "https://radius.example/authorize"})),
        "/v1/oauth/token" => {
            let body = form(req);
            assert_eq!(body["grant_type"], "authorization_code");
            assert_eq!(body["client_id"], "pi-gateway");
            MockResponse::json(200, &json!({"access_token": "rad-at", "refresh_token": "rad-rt", "expires_in": 3600, "scope": "gateway"}))
        }
        other => panic!("{other}"),
    })
    .await;
    let f = fixture(&[("PI_RADIUS_GATEWAY", &server.url)]);
    let auth = f.auth();
    auth.start(1, "radius").await.unwrap();
    answer(&auth, 1, "select", "browser").await;
    let s = wait_for(&auth, 1, "auth url", |s| s.url.is_some()).await;
    let q = query(s.url.as_deref().unwrap());
    assert_eq!(q["handoff"], "url");
    let redirect = q["redirect_uri"].clone();
    assert!(redirect.starts_with("http://127.0.0.1:") && redirect.ends_with("/oauth/callback"));
    let (status, _) = browser_get(&format!("{redirect}?code=rc&state={}", q["state"])).await;
    assert_eq!(status, 200);
    done(&auth, 1).await;
    let token_req = server
        .requests()
        .into_iter()
        .find(|r| r.path == "/v1/oauth/token")
        .unwrap();
    assert_eq!(form(&token_req)["redirect_uri"], redirect);
    let c = stored(&f, "radius");
    assert_eq!(c.extra["scope"], "gateway");
}

#[tokio::test]
async fn radius_device_login_for_a_models_json_gateway() {
    let polls = Arc::new(AtomicUsize::new(0));
    let p = polls.clone();
    let server = MockServer::start(move |req: &Recorded, _| match req.path.as_str() {
        "/v1/oauth/device" => MockResponse::json(
            200,
            &json!({"device_code": "rdc", "user_code": "R-1",
            "verification_uri": "https://gw.example/device", "expires_in": 600, "interval": 0}),
        ),
        "/v1/oauth/token" => match p.fetch_add(1, Ordering::SeqCst) {
            0 => MockResponse::json(400, &json!({"error": "authorization_pending"})),
            _ => MockResponse::json(
                200,
                &json!({"access_token": "team-at", "refresh_token": "team-rt", "expires_in": 3600}),
            ),
        },
        other => panic!("{other}"),
    })
    .await;
    let f = fixture(&[]);
    f.write_models(
        &json!({"providers": {"team": {"oauth": "radius", "name": "Team Gateway",
        "baseUrl": format!("{}/v1", server.url),
        "models": [{"id": "fast", "api": "pi-messages"}]}}}),
    );
    let auth = f.auth();
    auth.start(1, "team").await.unwrap();
    answer(&auth, 1, "select", "device-code").await;
    done(&auth, 1).await;
    assert_eq!(stored(&f, "team").access, "team-at");
    let r = auth.resolve(&model(&auth, "team"), &http()).await.unwrap();
    assert_eq!(r.credential.secret(), Some("team-at"));
}

#[tokio::test]
async fn xai_device_flow_and_cancellation() {
    let approved = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let approval = approved.clone();
    let server = MockServer::start(move |req: &Recorded, _| match req.path.as_str() {
        "/oauth2/device/code" => {
            assert_eq!(form(req)["referrer"], "pi");
            MockResponse::json(200, &json!({"device_code": "xdc", "user_code": "X-1",
                "verification_uri": "https://accounts.x.ai/device",
                "verification_uri_complete": "https://accounts.x.ai/device?c=X-1", "expires_in": 900}))
        }
        "/oauth2/token" => match approval.load(Ordering::SeqCst) {
            false => MockResponse::json(400, &json!({"error": "authorization_pending"})),
            true => MockResponse::json(200, &json!({"access_token": "x-at", "refresh_token": "x-rt", "expires_in": 3600})),
        },
        other => panic!("{other}"),
    })
    .await;
    let f = fixture(&[("PIPKIN_OAUTH_XAI_URL", &server.url)]);
    let auth = f.auth();
    auth.start(1, "xai").await.unwrap();
    let s = wait_for(&auth, 1, "device code", |s| s.device_code.is_some()).await;
    assert_eq!(s.url.as_deref(), Some("https://accounts.x.ai/device?c=X-1"));
    wait_for(&auth, 1, "pending device poll", |_| {
        server.requests().len() >= 2
    })
    .await;
    approved.store(true, Ordering::SeqCst);
    done(&auth, 1).await;
    assert_eq!(stored(&f, "xai").access, "x-at");

    // A pending device flow stops polling when cancelled, and nothing is saved.
    let server2 = MockServer::start(|req: &Recorded, _| match req.path.as_str() {
        "/oauth2/device/code" => MockResponse::json(
            200,
            &json!({"device_code": "d", "user_code": "u",
            "verification_uri": "https://accounts.x.ai/device", "expires_in": 900}),
        ),
        _ => MockResponse::json(400, &json!({"error": "authorization_pending"})),
    })
    .await;
    let f = fixture(&[
        ("PIPKIN_OAUTH_XAI_URL", &server2.url),
        ("PIPKIN_OAUTH_POLL_INTERVAL_MS", "20"),
    ]);
    let auth = f.auth();
    let attempt = auth.start(1, "xai").await.unwrap();
    wait_for(&auth, 1, "polling", |_| server2.requests().len() >= 3).await;
    auth.cancel(1, &attempt);
    assert_eq!(auth.snapshot(1).status, AuthStatus::Idle);
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    let n = server2.requests().len();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(server2.requests().len(), n, "polling stopped");
    assert!(!f.auth_path().exists());
}

// Claude subscription port: no live accounts, credentials or provider requests.
fn anthropic_fixture(url: &str) -> Fixture {
    fixture(&[
        ("PIPKIN_OAUTH_ANTHROPIC_AUTHORIZE_URL", url),
        ("PIPKIN_OAUTH_ANTHROPIC_PLATFORM_URL", url),
        ("ANTHROPIC_API_KEY", "must-not-fallback"),
    ])
}
fn anthropic_tokens(n: usize) -> Value {
    json!({"access_token": format!("sk-ant-oat-test-{n}"),
        "refresh_token": format!("refresh-{n}"), "expires_in":3600})
}
async fn anthropic_prompt(
    auth: &Auth,
    client: u64,
    method: &str,
) -> pipkin_engine_protocol::AuthSnapshot {
    auth.start(client, "anthropic").await.unwrap();
    answer(auth, client, "select", method).await;
    wait_for(auth, client, "Claude callback or pasted code", |s| {
        s.url.is_some() && s.challenge.is_some()
    })
    .await
}

#[tokio::test]
async fn anthropic_browser_login_validates_state_and_exchanges_pkce() {
    let server = MockServer::start(|req, n| {
        assert_eq!(req.path, "/v1/oauth/token");
        MockResponse::json(200, &anthropic_tokens(n))
    })
    .await;
    let f = anthropic_fixture(&server.url);
    let auth = f.auth();
    let s = anthropic_prompt(&auth, 1, "browser").await;
    let q = query(s.url.as_ref().unwrap());
    assert_eq!(q["client_id"], "9d1c250a-e61b-44d9-88ed-5944d1962f5e");
    assert_eq!(q["code_challenge_method"], "S256");
    assert!(q["scope"].contains("user:inference"));
    let redirect = &q["redirect_uri"];
    assert!(redirect.starts_with("http://localhost:") && redirect.ends_with("/callback"));
    assert_eq!(
        browser_get(&format!("{redirect}?code=code&state=wrong"))
            .await
            .0,
        400
    );
    assert!(server.requests().is_empty());
    assert_eq!(
        browser_get(&format!("{redirect}?code=code&state={}", q["state"]))
            .await
            .0,
        200
    );
    done(&auth, 1).await;
    let body = server.requests()[0].json();
    assert_eq!(body["grant_type"], "authorization_code");
    assert_eq!(body["code"], "code");
    assert_eq!(body["state"], q["state"]);
    assert_eq!(body["redirect_uri"], *redirect);
    assert_eq!(
        super::pkce::challenge_for(body["code_verifier"].as_str().unwrap()),
        q["code_challenge"]
    );
    assert_eq!(stored(&f, "anthropic").access, "sk-ant-oat-test-0");
    assert!(
        http()
            .get(redirect.replace("://localhost:", "://127.0.0.1:"))
            .send()
            .await
            .is_err()
    );
    assert_eq!(auth.snapshot(2).status, AuthStatus::Idle);
}

#[tokio::test]
async fn anthropic_copy_code_accepts_reference_paste_forms() {
    for form in ["pair", "url", "query", "code"] {
        let server = MockServer::start(|_, n| MockResponse::json(200, &anthropic_tokens(n))).await;
        let f = anthropic_fixture(&server.url);
        let auth = f.auth();
        let s = anthropic_prompt(&auth, 1, "copy_code").await;
        let q = query(s.url.as_ref().unwrap());
        assert_eq!(
            q["redirect_uri"],
            format!("{}/oauth/code/callback", server.url)
        );
        assert_eq!(s.challenge.as_ref().unwrap().kind, "manual_code");
        let code = match form {
            "pair" => format!("code#{}", q["state"]),
            "url" => format!(
                "https://example.test/callback?code=code&state={}",
                q["state"]
            ),
            "query" => format!("code=code&state={}", q["state"]),
            _ => "code".into(),
        };
        answer(&auth, 1, "manual_code", &code).await;
        done(&auth, 1).await;
        let body = server.requests()[0].json();
        assert_eq!(body["code"], "code");
        assert_eq!(body["state"], q["state"]);
        assert_eq!(body["redirect_uri"], q["redirect_uri"]);
        assert_eq!(
            super::pkce::challenge_for(body["code_verifier"].as_str().unwrap()),
            q["code_challenge"]
        );
    }
}

#[tokio::test]
async fn anthropic_wrong_state_cancel_and_retry_never_save_stale_credentials() {
    let server = MockServer::start(|_, n| MockResponse::json(200, &anthropic_tokens(n))).await;
    let f = anthropic_fixture(&server.url);
    let auth = f.auth();
    let stale = anthropic_prompt(&auth, 1, "copy_code").await;
    answer(&auth, 1, "manual_code", "code#wrong-state").await;
    let failed = wait_for(&auth, 1, "failed state", |s| s.status == AuthStatus::Failed).await;
    assert!(failed.url.is_none() && failed.challenge.is_none());
    assert!(server.requests().is_empty());
    let current = anthropic_prompt(&auth, 1, "copy_code").await;
    assert!(
        auth.answer(
            1,
            stale.attempt.as_deref().unwrap(),
            &stale.challenge.unwrap().id,
            "code".into()
        )
        .is_err()
    );
    auth.cancel(1, current.attempt.as_deref().unwrap());
    assert!(!f.auth_path().exists());
    assert!(server.requests().is_empty());
    anthropic_prompt(&auth, 1, "copy_code").await;
    answer(&auth, 1, "manual_code", "new-code").await;
    done(&auth, 1).await;
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn anthropic_refresh_is_serialized_persists_rotation_and_preserves_metadata() {
    let server = MockServer::start(|req, n| {
        assert_eq!(req.json()["grant_type"], "refresh_token");
        assert_eq!(req.json()["refresh_token"], "old-refresh");
        MockResponse::json(200, &anthropic_tokens(n))
    })
    .await;
    let f = anthropic_fixture(&server.url);
    f.write_auth(&json!({"anthropic":{"type":"oauth","access":"old","refresh":"old-refresh","expires":1,"custom":"preserved"}}));
    // Separate Auth instances exercise the file lock as well as the in-process mutex.
    let first = Arc::new(f.auth());
    let second = Arc::new(f.auth());
    let mut tasks = Vec::new();
    for i in 0..8 {
        let auth = if i % 2 == 0 {
            first.clone()
        } else {
            second.clone()
        };
        tasks.push(tokio::spawn(async move {
            auth.resolve(&model(&auth, "anthropic"), &http())
                .await
                .unwrap()
        }));
    }
    for t in tasks {
        let r = t.await.unwrap();
        assert!(
            matches!(r.credential,Credential::OAuth { ref access,.. } if access=="sk-ant-oat-test-0")
        );
    }
    assert_eq!(server.requests().len(), 1);
    let saved = stored(&f, "anthropic");
    assert_eq!(saved.refresh, "refresh-0");
    assert_eq!(saved.extra["custom"], "preserved");
    let lifetime = saved.expires - crate::util::now_ms();
    assert!(
        lifetime > 3_500_000 && lifetime <= 3_600_000,
        "margin is applied by Auth, not twice: {lifetime}"
    );
}

#[tokio::test]
async fn anthropic_refresh_failure_is_redacted_and_never_uses_api_key_billing() {
    for response in [
        MockResponse::json(401, &json!({"error":"SECRET access-token refresh-token"})),
        MockResponse::json(
            200,
            &json!({"access_token":"SECRET","refresh_token":"r","expires_in":-1}),
        ),
    ] {
        let server = MockServer::start(move |_, _| response.clone()).await;
        let f = anthropic_fixture(&server.url);
        let initial = json!({"anthropic":{"type":"oauth","access":"old","refresh":"old-refresh","expires":1}});
        f.write_auth(&initial);
        let auth = f.auth();
        let err = auth
            .resolve(&model(&auth, "anthropic"), &http())
            .await
            .unwrap_err();
        assert!(matches!(err, crate::auth::AuthError::Failed(_)));
        assert!(!format!("{err:?}").contains("SECRET"));
        assert_eq!(f.read_auth(), initial);
        assert_eq!(server.requests().len(), 1);
    }
}
