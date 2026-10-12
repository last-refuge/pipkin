use super::*;
use crate::provider::api::aws_auth::resolve_credentials;
use crate::provider::mock::{MockResponse, MockServer};
use tokio_util::sync::CancellationToken;

struct Fixture {
    dir: tempfile::TempDir,
    vars: HashMap<String, String>,
    path: PathBuf,
}

impl Fixture {
    fn new(modern: bool, endpoint: &str, token: Value) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config = if modern {
            "[profile test]\nsso_session=corp\nsso_account_id=123456789012\nsso_role_name=Developer\n[sso-session corp]\nsso_region=us-west-2\nsso_start_url=https://example.awsapps.com/start\n"
        } else {
            "[profile test]\nsso_account_id=123456789012\nsso_role_name=Developer\nsso_region=us-west-2\nsso_start_url=https://example.awsapps.com/start\n"
        };
        std::fs::write(dir.path().join("config"), config).unwrap();
        std::fs::write(dir.path().join("credentials"), "").unwrap();
        let home = dir.path().to_str().unwrap();
        let path = token_path(
            home,
            if modern {
                "corp"
            } else {
                "https://example.awsapps.com/start"
            },
        );
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, token.to_string()).unwrap();
        let vars = HashMap::from([
            ("HOME".into(), home.into()),
            (
                "AWS_CONFIG_FILE".into(),
                dir.path().join("config").display().to_string(),
            ),
            (
                "AWS_SHARED_CREDENTIALS_FILE".into(),
                dir.path().join("credentials").display().to_string(),
            ),
            ("AWS_ENDPOINT_URL_SSO".into(), endpoint.into()),
            ("AWS_ENDPOINT_URL_SSO_OIDC".into(), endpoint.into()),
            ("AWS_EC2_METADATA_DISABLED".into(), "true".into()),
        ]);
        Self { dir, vars, path }
    }

    async fn resolve(&self, cancel: &CancellationToken) -> Result<AwsCredentials, String> {
        let lookup = |key: &str| self.vars.get(key).cloned();
        let http = crate::provider::http::client();
        resolve_credentials(&ChainInput {
            env: &lookup,
            process_env: &self
                .vars
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            explicit_profile: Some("test".into()),
            ambient_profile: None,
            region: "us-west-2".into(),
            http: &http,
            cancel,
        })
        .await
    }

    fn saved(&self) -> Value {
        serde_json::from_slice(&std::fs::read(&self.path).unwrap()).unwrap()
    }
}

fn token(expiry: &str) -> Value {
    json!({"accessToken":"old-access","expiresAt":expiry,"clientId":"client","clientSecret":"secret","refreshToken":"old-refresh","custom":{"keep":true}})
}

fn role() -> MockResponse {
    MockResponse::json(
        200,
        &json!({"roleCredentials":{"accessKeyId":"SSO-ID","secretAccessKey":"sso-secret","sessionToken":"sso-session","expiration":4070908800000u64}}),
    )
}

fn refresh() -> MockResponse {
    MockResponse::json(
        200,
        &json!({"accessToken":"new-access","refreshToken":"new-refresh","expiresIn":3600}),
    )
}

#[tokio::test]
async fn cli_profiles_match_executed_sdk_requests_cache_and_results() {
    let reference: Value = serde_json::from_str(include_str!(
        "../../../../../fixtures/native-aws-sso/reference.json"
    ))
    .unwrap();
    for case in reference["cases"].as_array().unwrap() {
        let server = MockServer::start(|req, _| {
            if req.path == "/token" {
                refresh()
            } else {
                role()
            }
        })
        .await;
        let f = Fixture::new(
            case["name"] == "session-refresh",
            &server.url,
            case["token"].clone(),
        );
        assert_eq!(
            f.path.file_name().unwrap().to_str().unwrap(),
            case["cacheName"]
        );
        let result = f.resolve(&CancellationToken::new()).await;
        if case.get("error").is_some() {
            assert!(result.unwrap_err().contains("expired"));
        } else {
            let c = result.unwrap();
            assert_eq!(c.access_key_id, case["credentials"]["accessKeyId"]);
            assert_eq!(c.secret_access_key, case["credentials"]["secretAccessKey"]);
            assert_eq!(
                c.session_token.as_deref(),
                case["credentials"]["sessionToken"].as_str()
            );
            assert_eq!(c.expires_at, expiry(&case["credentials"]["expiration"]));
            // Reuse unexpired role credentials without repeating either request.
            f.resolve(&CancellationToken::new()).await.unwrap();
        }
        let actual: Vec<Value> = server.requests().iter().map(|req| {
            let url = url::Url::parse(&format!("http://local{}", req.path)).unwrap();
            let query: serde_json::Map<String, Value> = url.query_pairs().map(|(k,v)|(k.into_owned(), json!(v))).collect();
            json!({"method":req.method,"path":url.path(),"query":query,"bearer":req.header("x-amz-sso_bearer_token"),"body":req.json()})
        }).collect();
        assert_eq!(json!(actual), case["requests"], "{}", case["name"]);
        for key in ["accessToken", "refreshToken", "custom"] {
            assert_eq!(f.saved()[key], case["saved"][key]);
        }
        if case["name"] == "session-refresh" {
            assert!(expiry(&f.saved()["expiresAt"]).unwrap() > now_secs() + 3500);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    std::fs::metadata(&f.path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
        }
    }
    assert!(
        reference["mfaError"]
            .as_str()
            .unwrap()
            .contains("no MFA code callback")
    );
}

#[tokio::test]
async fn stopped_waiter_does_not_abandon_rotating_token_or_duplicate_refresh() {
    let server = MockServer::start(|req, _| {
        if req.path == "/token" {
            let mut r = refresh();
            r.pause = Duration::from_millis(150);
            r
        } else {
            role()
        }
    })
    .await;
    let f = Fixture::new(true, &server.url, token("2000-01-01T00:00:00Z"));
    let cancel = CancellationToken::new();
    let first = f.resolve(&cancel);
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
    let (first, ()) = tokio::join!(first, stop);
    assert_eq!(first.unwrap_err(), "Request was aborted");
    let fresh = CancellationToken::new();
    let (a, b) = tokio::join!(f.resolve(&fresh), f.resolve(&fresh));
    assert_eq!(a.unwrap().access_key_id, "SSO-ID");
    assert_eq!(b.unwrap().access_key_id, "SSO-ID");
    assert_eq!(server.requests().len(), 2);
    assert_eq!(f.saved()["refreshToken"], "new-refresh");
}

#[tokio::test]
async fn refresh_failure_keeps_valid_token_and_throttles_retries() {
    let server = MockServer::start(|req, _| {
        if req.path == "/token" {
            MockResponse::json(400, &json!({"secret":"do-not-echo"}))
        } else {
            role()
        }
    })
    .await;
    let near = chrono::DateTime::from_timestamp((now_secs() + 120) as i64, 0)
        .unwrap()
        .to_rfc3339();
    let f = Fixture::new(true, &server.url, token(&near));
    let cancel = CancellationToken::new();
    f.resolve(&cancel).await.unwrap();
    f.resolve(&cancel).await.unwrap();
    assert_eq!(server.requests().len(), 2);
    assert_eq!(
        server.requests()[1].header("x-amz-sso_bearer_token"),
        Some("old-access")
    );
    assert_eq!(f.saved()["accessToken"], "old-access");
    // Expiry must never turn refresh failure into use of an expired token.
    std::fs::write(&f.path, token("2000-01-01T00:00:00Z").to_string()).unwrap();
    let err = f.resolve(&cancel).await.unwrap_err();
    assert!(err.contains("expired"));
    assert!(!err.contains("do-not-echo"));
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn cache_rotation_and_role_changes_do_not_reuse_wrong_credentials() {
    let server = MockServer::start(|_, _| role()).await;
    let f = Fixture::new(false, &server.url, token("2099-01-01T00:00:00Z"));
    let cancel = CancellationToken::new();
    f.resolve(&cancel).await.unwrap();
    let mut rotated = f.saved();
    rotated["accessToken"] = json!("rotated-access");
    std::fs::write(&f.path, rotated.to_string()).unwrap();
    f.resolve(&cancel).await.unwrap();
    let config = std::fs::read_to_string(f.dir.path().join("config"))
        .unwrap()
        .replace("Developer", "OtherRole");
    std::fs::write(f.dir.path().join("config"), config).unwrap();
    f.resolve(&cancel).await.unwrap();
    assert_eq!(server.requests().len(), 3);
    assert_eq!(
        server.requests()[1].header("x-amz-sso_bearer_token"),
        Some("rotated-access")
    );
    assert!(server.requests()[2].path.contains("role_name=OtherRole"));
}

#[tokio::test]
async fn atomic_refresh_preserves_a_new_login_and_unknown_fields() {
    let f = Fixture::new(true, "https://localhost", token("2000-01-01T00:00:00Z"));
    let old = f.saved();
    let mut new = old.clone();
    new["accessToken"] = json!("new-access");
    new["expiresAt"] = json!("2099-01-01T00:00:00Z");
    let mut login = old.clone();
    login["accessToken"] = json!("different-login");
    std::fs::write(&f.path, login.to_string()).unwrap();
    persist(f.path.clone(), old.clone(), new.clone()).await;
    assert_eq!(f.saved(), login);
    std::fs::write(&f.path, old.to_string()).unwrap();
    new.as_object_mut().unwrap().remove("refreshToken");
    persist(f.path.clone(), old, new).await;
    assert!(f.saved().get("refreshToken").is_none());
    assert_eq!(f.saved()["custom"], json!({"keep":true}));
    assert_eq!(f.saved()["clientSecret"], "secret");
}

#[tokio::test]
async fn malformed_expired_and_oversized_caches_fail_without_network() {
    let server = MockServer::start(|_, _| panic!("invalid cache made a request")).await;
    for contents in [
        "{not-json".to_owned(),
        "[]".into(),
        token("bad-date").to_string(),
        token("2000-01-01T00:00:00Z").to_string(),
        " ".repeat(LIMIT as usize + 1),
    ] {
        let f = Fixture::new(false, &server.url, json!({}));
        std::fs::write(&f.path, contents).unwrap();
        assert!(f.resolve(&CancellationToken::new()).await.is_err());
    }
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn response_limits_and_invalid_role_credentials_are_enforced() {
    for response in [
        json!({}),
        json!({"roleCredentials":{"accessKeyId":"A","secretAccessKey":"secret","sessionToken":"token","expiration":1}}),
        json!({"large":"x".repeat(LIMIT as usize)}),
    ] {
        let server = MockServer::start(move |_, _| MockResponse::json(200, &response)).await;
        let f = Fixture::new(false, &server.url, token("2099-01-01T00:00:00Z"));
        let err = f.resolve(&CancellationToken::new()).await.unwrap_err();
        assert!(
            err.contains("invalid role credentials") || err.contains("too large"),
            "{err}"
        );
        assert!(!err.contains("secret"));
    }
}

#[tokio::test]
async fn sso_can_supply_an_assume_role_source_profile() {
    let server = MockServer::start(|req,_| {
        if req.path.starts_with("/federation/credentials?") { role() } else {
            let mut response = MockResponse::json(200,&json!({}));
            response.chunks = vec![b"<AssumeRoleResponse><AssumeRoleResult><Credentials><AccessKeyId>ASSUMED-SSO</AccessKeyId><SecretAccessKey>secret</SecretAccessKey><SessionToken>session</SessionToken><Expiration>2099-01-01T00:00:00Z</Expiration></Credentials></AssumeRoleResult></AssumeRoleResponse>".to_vec()];
            response
        }
    }).await;
    let mut f = Fixture::new(false, &server.url, token("2099-01-01T00:00:00Z"));
    let path = f.dir.path().join("config");
    let config = std::fs::read_to_string(&path)
        .unwrap()
        .replace("[profile test]", "[profile source]");
    std::fs::write(path,format!("{config}\n[profile test]\nrole_arn=arn:aws:iam::123456789012:role/sso-test\nsource_profile=source\n")).unwrap();
    f.vars
        .insert("AWS_ENDPOINT_URL_STS".into(), server.url.clone());
    assert_eq!(
        f.resolve(&CancellationToken::new())
            .await
            .unwrap()
            .access_key_id,
        "ASSUMED-SSO"
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1]
            .header("authorization")
            .unwrap()
            .starts_with("AWS4-HMAC-SHA256 Credential=SSO-ID/")
    );
    assert_eq!(
        requests[1].header("x-amz-security-token"),
        Some("sso-session")
    );
}

#[tokio::test]
async fn stalled_exchange_releases_its_owner_after_the_total_budget() {
    let server = MockServer::start(|_, _| {
        let mut response = refresh();
        response.chunks.clear();
        response.hang = true;
        response
    })
    .await;
    let f = Fixture::new(true, &server.url, token("2000-01-01T00:00:00Z"));
    let started = Instant::now();
    let err = f.resolve(&CancellationToken::new()).await.unwrap_err();
    assert!(
        err.contains("timed out") || err.contains("could not refresh"),
        "{err}"
    );
    assert!(started.elapsed() >= Duration::from_secs(29));
    assert!(started.elapsed() < Duration::from_secs(35));
    let shared = state(&f.path).unwrap();
    let guard = tokio::time::timeout(Duration::from_secs(1), shared.lock())
        .await
        .unwrap();
    assert!(guard.credentials.is_empty());
    assert_eq!(f.saved()["accessToken"], "old-access");
}
