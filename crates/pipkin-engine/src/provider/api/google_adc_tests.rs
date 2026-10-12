use super::*;
use crate::provider::api::google_shared::tests::{model, request};
use crate::provider::mock::{MockResponse, MockServer};
use crate::provider::{Credential, ISOLATED_ENV, ProviderRequest};

fn reference() -> Value {
    serde_json::from_str(include_str!(
        "../../../../../fixtures/native-google-adc/reference.json"
    ))
    .unwrap()
}
fn req(dir: &Path) -> ProviderRequest {
    let mut r = request(model(
        json!({"id":"gemini-2.5-pro","api":"google-vertex","provider":"google-vertex","baseUrl":"https://example.invalid"}),
    ));
    r.credential = Credential::None;
    r.env.clear();
    r.env.insert("HOME".into(), dir.display().to_string());
    r.options.insert(ISOLATED_ENV.into(), json!(true));
    r
}
async fn token(req: &ProviderRequest, cancel: CancellationToken) -> Result<AccessToken, String> {
    let http = crate::provider::http::client();
    resolve(
        &StreamCtx {
            req,
            http: &http,
            events: None,
            cancel,
        },
        None,
    )
    .await
}
fn set_doc(req: &mut ProviderRequest, dir: &Path, doc: &Value) {
    let path = dir.join("adc.json");
    std::fs::write(&path, doc.to_string()).unwrap();
    req.env.insert(
        "GOOGLE_APPLICATION_CREDENTIALS".into(),
        path.display().to_string(),
    );
}
fn user(refresh: &str) -> Value {
    json!({"type":"authorized_user","client_id":"client","client_secret":"secret","refresh_token":refresh,"quota_project_id":"file-quota"})
}

#[tokio::test]
async fn impersonation_matches_executed_sdk_grants_delegates_and_billing() {
    for case in &reference()["cases"].as_array().unwrap()[..2] {
        let server=MockServer::start(|r,_|if r.path=="/token"{MockResponse::json(200,&json!({"access_token":"source-token","expires_in":3600}))}else{MockResponse::json(200,&json!({"accessToken":"impersonated-token","expireTime":"2099-01-01T00:00:00Z"}))}).await;
        let dir = tempfile::tempdir().unwrap();
        let mut req = req(dir.path());
        let mut doc = case["document"].clone();
        doc["endpoint"] = json!(server.url);
        set_doc(&mut req, dir.path(), &doc);
        req.options.insert(
            "googleTokenUrl".into(),
            json!(format!("{}/token", server.url)),
        );
        let c = token(&req, CancellationToken::new()).await.unwrap();
        assert_eq!(c.token, case["token"]);
        assert_eq!(c.quota_project.as_deref(), case["quota"].as_str());
        token(&req, CancellationToken::new()).await.unwrap();
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        let form: serde_json::Map<String, Value> = url::form_urlencoded::parse(&requests[0].body)
            .map(|(k, v)| (k.into_owned(), json!(v)))
            .collect();
        assert_eq!(json!(form), case["requests"][0]["body"]);
        let expected = &case["requests"][1];
        assert_eq!(requests[1].json(), expected["body"]);
        assert_eq!(
            requests[1].path,
            url::Url::parse(expected["url"].as_str().unwrap())
                .unwrap()
                .path()
        );
        assert_eq!(
            requests[1].header("authorization"),
            expected["authorization"].as_str()
        );
        assert_eq!(
            requests[1].header("x-goog-user-project"),
            expected["quota"].as_str()
        );
    }
}

#[tokio::test]
async fn absent_default_file_uses_metadata_with_sdk_scope_and_quota_override() {
    let server = MockServer::start(|_, _| {
        MockResponse::json(
            200,
            &json!({"access_token":"metadata-token","expires_in":3600}),
        )
        .with_header("Metadata-Flavor", "Google")
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let mut req = req(dir.path());
    req.env
        .insert("GCE_METADATA_HOST".into(), server.url.clone());
    req.env
        .insert("GOOGLE_CLOUD_QUOTA_PROJECT".into(), "env-quota".into());
    req.env
        .insert("METADATA_SERVER_DETECTION".into(), " BIOS-ONLY ".into());
    req.env
        .insert("K_SERVICE".into(), "synthetic-cloud-run".into());
    let c = token(&req, CancellationToken::new()).await.unwrap();
    assert_eq!(c.token, reference()["cases"][2]["token"]);
    assert_eq!(c.quota_project.as_deref(), Some("env-quota"));
    token(&req, CancellationToken::new()).await.unwrap();
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let url = url::Url::parse(&format!("{}{}", server.url, requests[0].path)).unwrap();
    let expected = &reference()["cases"][2]["calls"][0];
    assert_eq!(
        url.path(),
        format!(
            "/computeMetadata/v1/instance/{}",
            expected["property"].as_str().unwrap()
        )
    );
    assert_eq!(
        url.query_pairs().find(|(k, _)| k == "scopes").unwrap().1,
        expected["params"]["scopes"].as_str().unwrap()
    );
    assert_eq!(requests[0].header("metadata-flavor"), Some("Google"));
}

#[tokio::test]
async fn explicit_bad_file_and_disabled_discovery_never_fall_back_to_metadata() {
    let server = MockServer::start(|_, _| panic!("must not try metadata")).await;
    let dir = tempfile::tempdir().unwrap();
    let mut req = req(dir.path());
    req.env
        .insert("GCE_METADATA_HOST".into(), server.url.clone());
    req.env.insert(
        "GOOGLE_APPLICATION_CREDENTIALS".into(),
        dir.path().join("missing.json").display().to_string(),
    );
    assert_eq!(
        token(&req, CancellationToken::new()).await.unwrap_err(),
        NO_CREDENTIALS
    );
    req.env.remove("GOOGLE_APPLICATION_CREDENTIALS");
    req.env
        .insert("METADATA_SERVER_DETECTION".into(), " NONE ".into());
    assert_eq!(
        token(&req, CancellationToken::new()).await.unwrap_err(),
        NO_CREDENTIALS
    );
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn credential_file_replacement_and_quota_changes_invalidate_the_old_identity() {
    let server = MockServer::start(|req, _| {
        let form: HashMap<_, _> = url::form_urlencoded::parse(&req.body).collect();
        MockResponse::json(
            200,
            &json!({"access_token":form["refresh_token"],"expires_in":3600}),
        )
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let mut req = req(dir.path());
    req.options.insert(
        "googleTokenUrl".into(),
        json!(format!("{}/token", server.url)),
    );
    set_doc(&mut req, dir.path(), &user("identity-a"));
    assert_eq!(
        token(&req, CancellationToken::new()).await.unwrap().token,
        "identity-a"
    );
    set_doc(&mut req, dir.path(), &user("identity-b"));
    let b = token(&req, CancellationToken::new()).await.unwrap();
    assert_eq!(b.token, "identity-b");
    assert_eq!(b.quota_project.as_deref(), Some("file-quota"));
    req.env
        .insert("GOOGLE_CLOUD_QUOTA_PROJECT".into(), "env-quota".into());
    let b = token(&req, CancellationToken::new()).await.unwrap();
    assert_eq!(b.token, "identity-b");
    assert_eq!(b.quota_project.as_deref(), Some("env-quota"));
    assert_eq!(server.requests().len(), 3);
}

#[tokio::test]
async fn cancelled_refresh_is_shared_without_blocking_another_identity() {
    let server = MockServer::start(|req, _| {
        let form: HashMap<_, _> = url::form_urlencoded::parse(&req.body).collect();
        let mut response = MockResponse::json(
            200,
            &json!({"access_token":form["refresh_token"],"expires_in":3600}),
        );
        if form["refresh_token"] == "slow" {
            response.pause = Duration::from_millis(300);
        }
        response
    })
    .await;
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let mut ra = req(a.path());
    let mut rb = req(b.path());
    for r in [&mut ra, &mut rb] {
        r.options.insert(
            "googleTokenUrl".into(),
            json!(format!("{}/token", server.url)),
        );
    }
    set_doc(&mut ra, a.path(), &user("slow"));
    set_doc(&mut rb, b.path(), &user("fast"));
    let cancel = CancellationToken::new();
    let stop = async {
        tokio::time::timeout(Duration::from_secs(2), async {
            while server.requests().is_empty() {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        cancel.cancel();
    };
    let (r, ()) = tokio::join!(token(&ra, cancel.clone()), stop);
    assert_eq!(r.unwrap_err(), "Request aborted");
    let fast = tokio::time::timeout(
        Duration::from_millis(200),
        token(&rb, CancellationToken::new()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(fast.token, "fast");
    assert_eq!(
        token(&ra, CancellationToken::new()).await.unwrap().token,
        "slow"
    );
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn invalid_universe_and_nesting_are_rejected_before_exchanging_source_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let mut req = req(dir.path());
    let mut doc = reference()["cases"][0]["document"].clone();
    doc["universe_domain"] = json!("different.example");
    set_doc(&mut req, dir.path(), &doc);
    assert!(
        token(&req, CancellationToken::new())
            .await
            .unwrap_err()
            .contains("universe")
    );
    let wrapper = reference()["cases"][0]["document"].clone();
    let mut inner = wrapper.clone();
    inner["source_credentials"]["universe_domain"] = json!("different.example");
    let mut outer = wrapper.clone();
    outer["source_credentials"] = inner;
    assert_eq!(universe(&outer), "different.example");
    validate(&outer, 0).unwrap();
    outer["universe_domain"] = json!("googleapis.com");
    set_doc(&mut req, dir.path(), &outer);
    assert!(
        token(&req, CancellationToken::new())
            .await
            .unwrap_err()
            .contains("universe")
    );
    let mut nested = user("r");
    for _ in 0..10 {
        let mut next = wrapper.clone();
        next["source_credentials"] = nested;
        nested = next;
    }
    set_doc(&mut req, dir.path(), &nested);
    assert!(
        token(&req, CancellationToken::new())
            .await
            .unwrap_err()
            .contains("nesting")
    );
}

#[tokio::test]
async fn metadata_requires_flavor_and_a_bounded_complete_response() {
    for behavior in ["flavor", "oversized", "stalled"] {
        let server = MockServer::start(move |_, _| {
            let mut r = MockResponse::json(200, &json!({"access_token":"token","expires_in":3600}));
            if behavior != "flavor" {
                r = r.with_header("Metadata-Flavor", "Google");
            }
            if behavior == "oversized" {
                r.chunks = vec![vec![b' '; LIMIT + 1]];
            }
            if behavior == "stalled" {
                r.chunks.clear();
                r.hang = true;
            }
            r
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let mut req = req(dir.path());
        req.env
            .insert("GCE_METADATA_HOST".into(), server.url.clone());
        let err = tokio::time::timeout(
            Duration::from_secs(5),
            token(&req, CancellationToken::new()),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(!err.contains("access_token"));
    }
}
