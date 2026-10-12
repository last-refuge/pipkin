//! Auth integration tests: precedence, isolation, headers, refresh, reserved behavior and the
//! sign-in lifecycle. Network access only reaches the local mock server.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pipkin_engine_protocol::{AuthSnapshot, AuthStatus, ErrorCode};
use serde_json::{Value, json};

use super::signin::*;
use super::*;
use crate::config::Env;
use crate::provider::mock::{MockResponse, MockServer, Recorded};

pub(crate) struct Fixture {
    pub dir: tempfile::TempDir,
    pub config: EngineConfig,
}

impl Fixture {
    pub fn new(vars: &[(&str, &str)]) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let mut config = EngineConfig::new(dir.path().join("data"), dir.path().join("cfg"));
        config.env = Env::isolated(
            vars.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        );
        Fixture { dir, config }
    }

    pub fn auth_path(&self) -> std::path::PathBuf {
        self.config.auth_path()
    }

    pub fn write_auth(&self, value: &Value) {
        self.write_auth_text(&serde_json::to_string_pretty(value).unwrap());
    }

    pub fn write_auth_text(&self, text: &str) {
        std::fs::create_dir_all(&self.config.config_dir).unwrap();
        std::fs::write(self.auth_path(), text).unwrap();
    }

    pub fn read_auth(&self) -> Value {
        serde_json::from_str(&std::fs::read_to_string(self.auth_path()).unwrap()).unwrap()
    }

    pub fn write_models(&self, value: &Value) {
        std::fs::create_dir_all(&self.config.config_dir).unwrap();
        std::fs::write(self.config.models_path(), value.to_string()).unwrap();
    }

    pub fn auth(&self) -> Auth {
        Auth::new(&self.config)
    }
}

pub(crate) fn http() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

pub(crate) fn model(auth: &Auth, provider: &str) -> Model {
    auth.catalog()
        .models
        .iter()
        .find(|m| m.provider == provider)
        .unwrap_or_else(|| panic!("no model for {provider}"))
        .clone()
}

pub(crate) async fn wait_for(
    auth: &Auth,
    client: u64,
    what: &str,
    f: impl Fn(&AuthSnapshot) -> bool,
) -> AuthSnapshot {
    for _ in 0..1000 {
        let s = auth.snapshot(client);
        if f(&s) {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let s = auth.snapshot(client);
    panic!(
        "timed out waiting for {what}: status {:?} message {:?}",
        s.status, s.message
    );
}

pub(crate) fn form(r: &Recorded) -> HashMap<String, String> {
    url::form_urlencoded::parse(&r.body).into_owned().collect()
}

fn far_future() -> i64 {
    crate::util::now_ms() + 3_600_000
}

async fn resolve(auth: &Auth, provider: &str) -> Result<ResolvedAuth, AuthError> {
    let m = model(auth, provider);
    auth.resolve(&m, &http()).await
}

// ----------------------------------------------------------------------------------- precedence

#[tokio::test]
async fn auth_json_then_models_json_then_environment() {
    let f = Fixture::new(&[("OPENAI_API_KEY", "env-key")]);
    f.write_models(&json!({"providers": {"openai": {"apiKey": "cfg-key"}}}));
    f.write_auth(&json!({"openai": {"type": "api_key", "key": "stored-key"}}));
    let auth = f.auth();
    assert_eq!(
        resolve(&auth, "openai").await.unwrap().credential,
        Credential::ApiKey("stored-key".into())
    );
    std::fs::remove_file(f.auth_path()).unwrap();
    assert_eq!(
        resolve(&auth, "openai").await.unwrap().credential,
        Credential::ApiKey("cfg-key".into())
    );
    std::fs::remove_file(f.config.models_path()).unwrap();
    auth.reload();
    assert_eq!(
        resolve(&auth, "openai").await.unwrap().credential,
        Credential::ApiKey("env-key".into())
    );
    assert!(auth.availability(&model(&auth, "openai")).is_ok());
    assert!(matches!(
        auth.availability(&model(&auth, "groq")),
        Err(AuthError::NotConfigured(_))
    ));
}

#[tokio::test]
async fn the_process_environment_is_never_consulted_when_isolated() {
    // HOME and PATH exist in the test process; an isolated engine environment hides them.
    let f = Fixture::new(&[]);
    f.write_models(&json!({"providers": {"local": {
        "baseUrl": "http://127.0.0.1:9/v1", "api": "openai-completions", "apiKey": "$HOME",
        "models": [{"id": "m"}]}}}));
    let auth = f.auth();
    let m = model(&auth, "local");
    assert!(matches!(
        auth.availability(&m),
        Err(AuthError::NotConfigured(_))
    ));
    assert_eq!(
        auth.resolve(&m, &http()).await.unwrap_err(),
        AuthError::Failed(
            "Failed to resolve API key for provider \"local\" from environment variable: HOME"
                .into()
        )
    );
}

#[tokio::test]
async fn stored_keys_are_config_values_and_availability_runs_nothing() {
    let f = Fixture::new(&[
        ("OPENAI_API_KEY", "env-key"),
        ("GROQ_VAR", "from-config-env"),
    ]);
    let marker = f.dir.path().join("ran");
    f.write_auth(&json!({
        "xai": {"type": "api_key", "key": "$XK", "env": {"XK": "from-cred-env"}},
        "groq": {"type": "api_key", "key": "${GROQ_VAR}-suffix"},
        "mistral": {"type": "api_key", "key": format!("!touch {} && echo cmd-key", marker.display())},
        "openai": {"type": "api_key", "key": "$MISSING"}
    }));
    let auth = f.auth();
    assert!(auth.availability(&model(&auth, "mistral")).is_ok());
    assert!(!marker.exists(), "availability must not run commands");
    assert_eq!(
        resolve(&auth, "xai").await.unwrap().credential,
        Credential::ApiKey("from-cred-env".into())
    );
    assert_eq!(
        resolve(&auth, "groq").await.unwrap().credential,
        Credential::ApiKey("from-config-env-suffix".into())
    );
    assert_eq!(
        resolve(&auth, "mistral").await.unwrap().credential,
        Credential::ApiKey("cmd-key".into())
    );
    assert!(marker.exists());
    // An unresolvable stored key leaves the provider's ambient key in play (reference).
    assert_eq!(
        resolve(&auth, "openai").await.unwrap().credential,
        Credential::ApiKey("env-key".into())
    );
}

#[tokio::test]
async fn configured_headers_merge_in_reference_order() {
    let f = Fixture::new(&[("HV", "header-value")]);
    f.write_models(&json!({"providers": {"local": {
        "baseUrl": "http://127.0.0.1:9/v1", "api": "openai-completions", "apiKey": "k1",
        "authHeader": true,
        "headers": {"X-A": "$HV", "Authorization": "Basic nope"},
        "models": [{"id": "m", "headers": {"X-M": "from-model"}}],
        "modelOverrides": {"m": {"headers": {"X-M": "from-override", "X-O": "o"}}}
    }}}));
    let auth = f.auth();
    let r = resolve(&auth, "local").await.unwrap();
    assert_eq!(r.credential, Credential::ApiKey("k1".into()));
    let h: Vec<(&str, Option<&str>)> = r
        .headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_deref()))
        .collect();
    assert_eq!(
        h,
        vec![
            ("Authorization", Some("Bearer k1")),
            ("X-A", Some("header-value")),
            ("X-M", Some("from-model")),
            ("X-O", Some("o")),
        ]
    );
    // A header that cannot resolve fails the request with the reference text.
    f.write_models(&json!({"providers": {"local": {
        "baseUrl": "http://127.0.0.1:9/v1", "api": "openai-completions", "apiKey": "k1",
        "headers": {"X-A": "$NOPE"}, "models": [{"id": "m"}]}}}));
    auth.reload();
    assert_eq!(
        resolve(&auth, "local").await.unwrap_err(),
        AuthError::Failed(
            "Failed to resolve provider \"local\" header \"X-A\" from environment variable: NOPE"
                .into()
        )
    );
}

#[tokio::test]
async fn ambient_bedrock_vertex_and_cloudflare() {
    let dir = tempfile::tempdir().unwrap();
    let adc = dir.path().join("adc.json");
    std::fs::write(&adc, "{}").unwrap();
    let adc = adc.display().to_string();
    let f = Fixture::new(&[
        ("AWS_PROFILE", "work"),
        ("GOOGLE_APPLICATION_CREDENTIALS", &adc),
        ("GOOGLE_CLOUD_PROJECT", "p"),
        ("GOOGLE_CLOUD_LOCATION", "us-central1"),
        ("CLOUDFLARE_ACCOUNT_ID", "acct"),
    ]);
    f.write_auth(&json!({"cloudflare-workers-ai": {"type": "api_key", "key": "cf"}}));
    let auth = f.auth();
    assert!(matches!(
        resolve(&auth, "amazon-bedrock").await.unwrap().credential,
        Credential::Ambient(_)
    ));
    assert!(matches!(
        resolve(&auth, "google-vertex").await.unwrap().credential,
        Credential::Ambient(_)
    ));
    let cf = resolve(&auth, "cloudflare-workers-ai").await.unwrap();
    assert_eq!(cf.credential, Credential::ApiKey("cf".into()));
    assert_eq!(
        cf.env.get("CLOUDFLARE_ACCOUNT_ID").map(String::as_str),
        Some("acct")
    );
}

#[tokio::test]
async fn copilot_token_sets_base_url_and_filters_models() {
    let f = Fixture::new(&[]);
    let builtin: Vec<String> = crate::provider::model::builtin_models()
        .into_iter()
        .filter(|m| m.provider == "github-copilot")
        .map(|m| m.id)
        .collect();
    assert!(builtin.len() > 2);
    f.write_auth(&json!({"github-copilot": {"type": "oauth",
        "access": "tid=1;exp=2;proxy-ep=proxy.business.githubcopilot.com;st=x",
        "refresh": "gho_x", "expires": far_future(), "availableModelIds": [builtin[1]]}}));
    let auth = f.auth();
    let ids: Vec<String> = auth
        .catalog()
        .models
        .iter()
        .filter(|m| m.provider == "github-copilot")
        .map(|m| m.id.clone())
        .collect();
    assert_eq!(ids, vec![builtin[1].clone()]);
    let r = resolve(&auth, "github-copilot").await.unwrap();
    assert_eq!(
        r.base_url.as_deref(),
        Some("https://api.business.githubcopilot.com")
    );
    assert!(
        matches!(r.credential, Credential::OAuth { ref access, .. } if access.starts_with("tid=1"))
    );
    // The env token path uses the catalog base URL.
    let f = Fixture::new(&[("COPILOT_GITHUB_TOKEN", "ghu")]);
    let auth = f.auth();
    let r = resolve(&auth, "github-copilot").await.unwrap();
    assert_eq!(r.credential, Credential::ApiKey("ghu".into()));
    assert_eq!(r.base_url, None);
}

#[tokio::test]
async fn unsupported_entries_own_their_provider() {
    let f = Fixture::new(&[("OPENAI_API_KEY", "env")]);
    f.write_auth(&json!({"openai": {"type": "future_kind"}, "openai-codex": {"type": "oauth"}}));
    let auth = f.auth();
    assert!(matches!(
        resolve(&auth, "openai").await,
        Err(AuthError::Unavailable(_))
    ));
    assert!(matches!(
        auth.availability(&model(&auth, "openai")),
        Err(AuthError::Unavailable(_))
    ));
    // An invalid file fails closed and is reported.
    f.write_auth_text("{oops");
    assert!(matches!(
        resolve(&auth, "openai").await,
        Err(AuthError::Failed(_))
    ));
    assert!(auth.snapshot(1).credential_lookup_failed);
    assert!(
        auth.reload()
            .warnings
            .iter()
            .any(|w| w.contains("auth.json"))
    );
}

#[tokio::test]
async fn models_json_problems_become_warnings() {
    let f = Fixture::new(&[]);
    f.write_models(&json!({"providers": {"openai": {"baseUrl": 7}}}));
    let auth = f.auth();
    let c = auth.catalog();
    assert!(c.warnings[0].starts_with("Invalid models.json schema"));
    assert!(c.models.iter().any(|m| m.provider == "openai"));
    let mut config = f.config.clone();
    config.extra_models = vec![
        serde_json::from_value(json!({
        "id": "s", "api": "scripted", "provider": "scripted"}))
        .unwrap(),
    ];
    let auth = Auth::new(&config);
    let s = auth.catalog().find("scripted/s").unwrap().clone();
    assert!(auth.availability(&s).is_ok());
    assert_eq!(
        auth.resolve(&s, &http()).await.unwrap().credential,
        Credential::None
    );
}

// ------------------------------------------------------------------------------------- refresh

fn xai_expired(f: &Fixture) {
    f.write_auth(&json!({"xai": {"type": "oauth", "access": "old-access",
        "refresh": "old-refresh", "expires": crate::util::now_ms() + 60_000}}));
}

async fn xai_mock(fail: bool) -> (MockServer, Arc<AtomicUsize>) {
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let server = MockServer::start(move |req, _| {
        assert_eq!(req.path, "/oauth2/token");
        c.fetch_add(1, Ordering::SeqCst);
        if fail {
            MockResponse::json(
                400,
                &json!({"error": "invalid_grant", "error_description": "revoked"}),
            )
        } else {
            MockResponse::json(
                200,
                &json!({"access_token": "new-access", "expires_in": 7200}),
            )
        }
    })
    .await;
    (server, count)
}

#[tokio::test]
async fn expiring_tokens_refresh_once_and_persist_atomically() {
    let (server, count) = xai_mock(false).await;
    let f = Fixture::new(&[("PIPKIN_OAUTH_XAI_URL", &server.url)]);
    xai_expired(&f);
    let auth = Arc::new(f.auth());
    let m = model(&auth, "xai");
    let tasks: Vec<_> = (0..8)
        .map(|_| {
            let auth = auth.clone();
            let m = m.clone();
            tokio::spawn(async move { auth.resolve(&m, &http()).await })
        })
        .collect();
    for t in tasks {
        let r = t.await.unwrap().unwrap();
        assert_eq!(
            r.credential.secret(),
            Some("new-access"),
            "every caller sees the refreshed token"
        );
    }
    assert_eq!(count.load(Ordering::SeqCst), 1, "refresh is deduplicated");
    let saved = f.read_auth();
    assert_eq!(saved["xai"]["access"], "new-access");
    assert_eq!(
        saved["xai"]["refresh"], "old-refresh",
        "xAI keeps the old refresh token"
    );
    assert!(saved["xai"]["expires"].as_i64().unwrap() > far_future());
    let mode = std::fs::metadata(f.auth_path())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
    let req = &server.requests()[0];
    let body = form(req);
    assert_eq!(body["grant_type"], "refresh_token");
    assert_eq!(body["refresh_token"], "old-refresh");
    // Still valid now: no further refresh.
    auth.resolve(&m, &http()).await.unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_failed_refresh_never_falls_back_to_another_credential() {
    let (server, count) = xai_mock(true).await;
    let f = Fixture::new(&[
        ("PIPKIN_OAUTH_XAI_URL", &server.url),
        ("XAI_API_KEY", "billing-key"),
    ]);
    xai_expired(&f);
    let before = std::fs::read_to_string(f.auth_path()).unwrap();
    let auth = f.auth();
    let err = resolve(&auth, "xai").await.unwrap_err();
    let AuthError::Failed(msg) = err else {
        panic!("{err:?}")
    };
    assert!(msg.starts_with("OAuth refresh failed for xai"), "{msg}");
    assert!(!msg.contains("billing-key"));
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(std::fs::read_to_string(f.auth_path()).unwrap(), before);
    // Availability stays metadata-only (the stored credential owns the provider).
    assert!(auth.availability(&model(&auth, "xai")).is_ok());
}

#[tokio::test]
async fn refresh_is_bounded_by_a_timeout() {
    let server = MockServer::start(|_, _| MockResponse {
        status: 200,
        headers: vec![("content-type".into(), "application/json".into())],
        chunks: vec![b"{".to_vec()],
        pause: Duration::ZERO,
        hang: true,
    })
    .await;
    let f = Fixture::new(&[("PIPKIN_OAUTH_XAI_URL", &server.url)]);
    xai_expired(&f);
    let auth = f.auth();
    auth.set_refresh_timeout(Duration::from_millis(200));
    let started = std::time::Instant::now();
    let err = resolve(&auth, "xai").await.unwrap_err();
    assert_eq!(
        err,
        AuthError::Failed("OAuth refresh failed for xai: timed out".into())
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn another_process_refreshing_under_the_lock_wins() {
    let (server, count) = xai_mock(false).await;
    let f = Fixture::new(&[("PIPKIN_OAUTH_XAI_URL", &server.url)]);
    xai_expired(&f);
    let auth = f.auth();
    // "Another process" holds the lock, refreshes, and commits while we wait.
    let other = store::FileStore::new(f.auth_path());
    let mut txn = other.begin().unwrap();
    let path = f.auth_path();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        txn.set(
            "xai",
            StoredCredential::OAuth(OAuthCredential {
                access: "other-process".into(),
                refresh: "r2".into(),
                expires: crate::util::now_ms() + 7_200_000,
                extra: Default::default(),
            }),
        );
        txn.commit().unwrap();
        path
    });
    let r = resolve(&auth, "xai").await.unwrap();
    releaser.join().unwrap();
    assert_eq!(r.credential.secret(), Some("other-process"));
    assert_eq!(count.load(Ordering::SeqCst), 0);
}

// --------------------------------------------------------------------- Claude subscription

#[tokio::test]
async fn subscription_credentials_are_reused_removed_and_never_swapped_for_api_keys() {
    let f = Fixture::new(&[("ANTHROPIC_API_KEY", "billing-key")]);
    let stored = format!(
        r#"{{"anthropic": {{"type":"oauth","access":"a","refresh":"r","expires":{},"x":[1,2]}}, "openai": {{"type":"api_key","key":"o"}}}}"#,
        far_future()
    );
    f.write_auth_text(&stored);
    let auth = f.auth();
    let m = model(&auth, "anthropic");
    assert!(auth.availability(&m).is_ok());
    assert_eq!(
        auth.resolve(&m, &http()).await.unwrap().credential,
        Credential::OAuth {
            access: "a".into(),
            metadata: serde_json::from_value(json!({"x":[1,2]})).unwrap()
        }
    );
    auth.reuse(1, "anthropic").await.unwrap();
    let snap = auth.snapshot(1);
    assert_eq!(snap.status, AuthStatus::Done);
    let a = snap.providers.iter().find(|p| p.id == "anthropic").unwrap();
    assert!(a.available && a.has_credentials && a.unavailable_reason.is_none());
    let e = auth.set_api_key("anthropic", "sk-ant".into()).unwrap_err();
    assert_eq!(e.code, ErrorCode::Conflict);
    auth.set_api_key("xai", "xk".into()).unwrap();
    assert_eq!(
        f.read_auth()["anthropic"],
        serde_json::from_str::<Value>(&stored).unwrap()["anthropic"]
    );
    auth.remove("anthropic").unwrap();
    assert!(f.read_auth().get("anthropic").is_none());
    assert_eq!(
        auth.resolve(&m, &http()).await.unwrap().credential,
        Credential::ApiKey("billing-key".into())
    );
}

#[tokio::test]
async fn ordinary_anthropic_api_keys_work() {
    let f = Fixture::new(&[]);
    f.write_auth(&json!({"anthropic": {"type": "api_key", "key": "sk-ant-api"}}));
    let auth = f.auth();
    assert_eq!(
        resolve(&auth, "anthropic").await.unwrap().credential,
        Credential::ApiKey("sk-ant-api".into())
    );
    let f = Fixture::new(&[
        ("ANTHROPIC_OAUTH_TOKEN", "sk-ant-oat"),
        ("ANTHROPIC_API_KEY", "k"),
    ]);
    let auth = f.auth();
    assert_eq!(
        resolve(&auth, "anthropic").await.unwrap().credential,
        Credential::OAuth {
            access: "sk-ant-oat".into(),
            metadata: Default::default()
        }
    );
}

// ----------------------------------------------------------------------------------- lifecycle

#[tokio::test]
async fn set_api_key_and_remove_update_metadata_and_notify() {
    let f = Fixture::new(&[]);
    let auth = f.auth();
    let seen: Arc<Mutex<Vec<(u64, AuthSnapshot)>>> = Arc::default();
    let s = seen.clone();
    auth.set_listener(Arc::new(move |c, snap| s.lock().unwrap().push((c, snap))));
    assert!(!auth.snapshot(7).providers.iter().any(|p| p.has_credentials));
    assert_eq!(
        auth.set_api_key("openrouter", "  ".into())
            .unwrap_err()
            .code,
        ErrorCode::Invalid
    );
    auth.set_api_key("openrouter", "sk-or".into()).unwrap();
    assert_eq!(
        f.read_auth()["openrouter"],
        json!({"type": "api_key", "key": "sk-or"})
    );
    let last = seen.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last.0, 7);
    assert!(
        last.1
            .providers
            .iter()
            .any(|p| p.id == "openrouter" && p.has_credentials)
    );
    assert!(auth.availability(&model(&auth, "openrouter")).is_ok());
    auth.remove("openrouter").unwrap();
    auth.remove("openrouter").unwrap();
    assert!(f.read_auth().get("openrouter").is_none());
    assert!(!format!("{:?}", seen.lock().unwrap()).contains("sk-or"));
}

#[tokio::test]
async fn answers_are_guarded_by_attempt_challenge_options_and_length() {
    let f = Fixture::new(&[]);
    let auth = f.auth();
    let attempt = auth.start(1, "openai-codex").await.unwrap();
    let s = wait_for(&auth, 1, "select prompt", |s| s.challenge.is_some()).await;
    assert_eq!(s.status, AuthStatus::Prompt);
    assert_eq!(s.attempt.as_deref(), Some(attempt.as_str()));
    let ch = s.challenge.unwrap();
    assert_eq!(ch.kind, "select");
    assert_eq!(ch.options.len(), 2);
    let expired = |r: Result<(), ProtoError>| {
        let e = r.unwrap_err();
        assert_eq!(e.message, MSG_EXPIRED);
        assert_eq!(e.code, ErrorCode::NotFound);
    };
    expired(auth.answer(1, "other-attempt", &ch.id, "browser".into()));
    expired(auth.answer(1, &attempt, "other-challenge", "browser".into()));
    expired(auth.answer(2, &attempt, &ch.id, "browser".into()));
    expired(auth.answer(1, &attempt, &ch.id, "x".repeat(MAX_ANSWER_CHARS + 1)));
    let e = auth
        .answer(1, &attempt, &ch.id, "carrier-pigeon".into())
        .unwrap_err();
    assert_eq!(
        (e.code, e.message.as_str()),
        (ErrorCode::Invalid, MSG_BAD_OPTION)
    );
    // A new start supersedes the attempt; the old question has expired.
    let second = auth.start(1, "openai-codex").await.unwrap();
    assert_ne!(second, attempt);
    expired(auth.answer(1, &attempt, &ch.id, "browser".into()));
    let s = wait_for(&auth, 1, "second prompt", |s| {
        s.challenge.as_ref().is_some_and(|c| c.id != ch.id)
    })
    .await;
    // Cancel resets to idle; the question expires.
    auth.cancel(1, "not-current");
    assert_eq!(auth.snapshot(1).status, AuthStatus::Prompt);
    auth.cancel(1, &second);
    let idle = auth.snapshot(1);
    assert_eq!(idle.status, AuthStatus::Idle);
    assert_eq!(idle.attempt, None);
    expired(auth.answer(1, &second, &s.challenge.unwrap().id, "browser".into()));
    // Other providers and unknown ids.
    let e = auth.start(1, "nope").await.unwrap_err();
    assert_eq!(
        (e.code, e.message.as_str()),
        (ErrorCode::Unsupported, MSG_NOT_AVAILABLE)
    );
    // A client that goes away loses its attempt.
    let third = auth.start(3, "openai-codex").await.unwrap();
    wait_for(&auth, 3, "prompt", |s| s.challenge.is_some()).await;
    auth.client_gone(3);
    let fresh = auth.snapshot(3);
    assert_eq!(fresh.status, AuthStatus::Idle);
    expired(auth.answer(3, &third, "any", "browser".into()));
}

#[tokio::test]
async fn failures_publish_a_generic_message_only() {
    let server = MockServer::start(|_, _| {
        MockResponse::json(
            500,
            &json!({"error": "boom", "error_description": "token=SECRET123"}),
        )
    })
    .await;
    let f = Fixture::new(&[("PIPKIN_OAUTH_XAI_URL", &server.url)]);
    let auth = f.auth();
    let events: Arc<Mutex<Vec<AuthSnapshot>>> = Arc::default();
    let e = events.clone();
    auth.set_listener(Arc::new(move |_, s| e.lock().unwrap().push(s)));
    auth.start(1, "xai").await.unwrap();
    let s = wait_for(&auth, 1, "failure", |s| s.status == AuthStatus::Failed).await;
    assert_eq!(s.message, MSG_FAILED);
    let all = serde_json::to_string(&*events.lock().unwrap()).unwrap();
    assert!(!all.contains("SECRET123"), "{all}");
    assert!(
        events
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.status == AuthStatus::Connecting && s.message == MSG_CONNECTING)
    );
}

#[tokio::test]
async fn reuse_checks_saved_credentials_without_network() {
    let f = Fixture::new(&[("PIPKIN_OAUTH_XAI_URL", "http://127.0.0.1:9")]);
    let auth = f.auth();
    auth.reuse(1, "xai").await.unwrap();
    let s = auth.snapshot(1);
    assert_eq!(
        (s.status, s.message.as_str()),
        (AuthStatus::Failed, MSG_REUSE_FAILED)
    );
    // Expired but refreshable still counts: reuse never refreshes.
    xai_expired(&f);
    auth.reuse(1, "xai").await.unwrap();
    let s = auth.snapshot(1);
    assert_eq!(
        (s.status, s.message.as_str()),
        (AuthStatus::Done, MSG_REUSED)
    );
    assert_eq!(s.provider.as_deref(), Some("xai"));
}

#[tokio::test]
async fn snapshot_lists_sign_in_providers_with_metadata_only() {
    let f = Fixture::new(&[]);
    f.write_models(&json!({"providers": {"team": {"oauth": "radius", "baseUrl": "https://gw.example/v1", "name": "Team"}}}));
    f.write_auth(&json!({"xai": {"type": "oauth", "access": "a", "refresh": "r", "expires": 1}}));
    let auth = f.auth();
    let s = auth.snapshot(1);
    let ids: Vec<&str> = s.providers.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "anthropic",
            "openai-codex",
            "openai",
            "github-copilot",
            "openrouter",
            "kimi-coding",
            "meta",
            "radius",
            "xai",
            "team"
        ]
    );
    assert!(
        s.providers
            .iter()
            .all(|p| p.kind == pipkin_engine_protocol::CredentialKind::OAuth)
    );
    assert!(
        s.providers
            .iter()
            .find(|p| p.id == "xai")
            .unwrap()
            .has_credentials
    );
    assert!(
        !s.providers
            .iter()
            .find(|p| p.id == "openai")
            .unwrap()
            .has_credentials
    );
    assert!(!format!("{s:?}").contains("\"a\""));
    assert!(Path::new(&f.auth_path()).exists());
}

#[tokio::test]
async fn radius_gateway_catalogs_restore_from_the_stored_credential() {
    let f = Fixture::new(&[]);
    f.write_auth(
        &json!({"radius": {"type": "oauth", "access": "a", "refresh": "r",
        "expires": far_future(), "gatewayConfig": {"baseUrl": "https://gw.example/v1", "models": [
            {"id": "dyn", "name": "Dynamic", "reasoning": false, "input": ["text"],
             "cost": {"input": 1, "output": 2, "cacheRead": 0, "cacheWrite": 0},
             "contextWindow": 1000, "maxTokens": 100},
            {"id": "broken"}
        ]}}}),
    );
    let auth = f.auth();
    let m = auth.catalog().find("radius/dyn").cloned().unwrap();
    assert_eq!(
        (m.api.as_str(), m.base_url.as_str()),
        ("pi-messages", "https://gw.example/v1")
    );
    assert!(auth.catalog().find("radius/broken").is_none());
    // The gateway catalog is not request metadata.
    let r = auth.resolve(&m, &http()).await.unwrap();
    let Credential::OAuth { metadata, .. } = r.credential else {
        panic!()
    };
    assert!(metadata.get("gatewayConfig").is_none());
    assert!(
        !format!(
            "{:?}",
            Credential::OAuth {
                access: "tok".into(),
                metadata
            }
        )
        .contains("tok")
    );
}

#[tokio::test]
async fn concurrent_removal_and_login_wait_for_the_credential_commit() {
    let f = Fixture::new(&[]);
    let auth = Arc::new(f.auth());
    auth.set_api_key("openai-codex", "fixture-key".into())
        .unwrap();
    auth.start(9, "openai-codex").await.unwrap();
    wait_for(&auth, 9, "login choice", |s| s.challenge.is_some()).await;
    let transaction = auth.inner.store.begin().unwrap();
    let worker_auth = auth.clone();
    let removal = std::thread::spawn(move || worker_auth.remove("openai-codex"));
    wait_for(&auth, 9, "removal entered its commit phase", |s| {
        s.message == MSG_REMOVED
    })
    .await;
    let error = auth.remove("openai-codex").unwrap_err();
    assert_eq!(error.code, ErrorCode::Busy);
    assert_eq!(error.message, MSG_REMOVING);
    let error = auth.start(9, "openai-codex").await.unwrap_err();
    assert_eq!(error.code, ErrorCode::Busy);
    assert_eq!(error.message, MSG_REMOVING);
    assert!(f.read_auth().get("openai-codex").is_some());
    drop(transaction);
    removal.join().unwrap().unwrap();
    assert!(f.read_auth().get("openai-codex").is_none());
    let attempt = auth.start(9, "openai-codex").await.unwrap();
    auth.cancel(9, &attempt);
}
