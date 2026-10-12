use super::*;
use crate::provider::api::aws_auth::resolve_credentials;
use crate::provider::mock::{MockResponse, MockServer};
use tokio_util::sync::CancellationToken;

fn reference() -> Value {
    serde_json::from_str(include_str!(
        "../../../../../fixtures/native-aws-login/reference.json"
    ))
    .unwrap()
}
struct Fixture {
    dir: tempfile::TempDir,
    vars: HashMap<String, String>,
    path: PathBuf,
}
impl Fixture {
    fn new(endpoint: &str, token: &Value) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let r = reference();
        std::fs::write(
            dir.path().join("config"),
            format!(
                "[profile test]\nlogin_session={}\nregion=us-west-2\n",
                r["session"].as_str().unwrap()
            ),
        )
        .unwrap();
        std::fs::write(dir.path().join("credentials"), "").unwrap();
        let path = dir.path().join(r["cacheName"].as_str().unwrap());
        std::fs::write(&path, token.to_string()).unwrap();
        let vars = HashMap::from([
            ("HOME".into(), dir.path().display().to_string()),
            (
                "AWS_LOGIN_CACHE_DIRECTORY".into(),
                dir.path().display().to_string(),
            ),
            (
                "AWS_CONFIG_FILE".into(),
                dir.path().join("config").display().to_string(),
            ),
            (
                "AWS_SHARED_CREDENTIALS_FILE".into(),
                dir.path().join("credentials").display().to_string(),
            ),
            ("AWS_ENDPOINT_URL_SIGNIN".into(), endpoint.into()),
            ("AWS_EC2_METADATA_DISABLED".into(), "true".into()),
        ]);
        Self { dir, vars, path }
    }
    async fn resolve(&self, cancel: &CancellationToken) -> Result<AwsCredentials, String> {
        let lookup = |k: &str| self.vars.get(k).cloned();
        let http = crate::provider::http::client();
        resolve_credentials(&ChainInput {
            env: &lookup,
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
fn refreshed() -> Value {
    json!({"accessToken":{"accessKeyId":"NEW-ID","secretAccessKey":"new-secret","sessionToken":"new-session"},"refreshToken":"new-refresh","expiresIn":3600})
}
fn check_proof(proof: &str, endpoint: &str) -> (Value, Value) {
    let parts: Vec<_> = proof.split('.').collect();
    assert_eq!(parts.len(), 3);
    let header: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
    let payload: Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
    assert_eq!(header["alg"], "ES256");
    assert_eq!(header["typ"], "dpop+jwt");
    assert_eq!(header["jwk"], reference()["publicJwk"]);
    assert_eq!(payload["htm"], "POST");
    assert_eq!(payload["htu"], endpoint);
    assert_eq!(payload["jti"].as_str().unwrap().len(), 36);
    assert!(payload["iat"].as_u64().unwrap().abs_diff(now_secs()) < 10);
    let mut public = vec![4];
    public.extend(
        URL_SAFE_NO_PAD
            .decode(header["jwk"]["x"].as_str().unwrap())
            .unwrap(),
    );
    public.extend(
        URL_SAFE_NO_PAD
            .decode(header["jwk"]["y"].as_str().unwrap())
            .unwrap(),
    );
    ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_FIXED, public)
        .verify(
            format!("{}.{}", parts[0], parts[1]).as_bytes(),
            &URL_SAFE_NO_PAD.decode(parts[2]).unwrap(),
        )
        .unwrap();
    (header, payload)
}

#[test]
fn sec1_cli_key_produces_valid_p256_dpop_proofs() {
    let r = reference();
    let pem = r["dpopKey"].as_str().unwrap();
    let endpoint = "https://us-west-2.signin.aws.amazon.com/v1/token";
    let a = proof(pem, endpoint).unwrap();
    let b = proof(pem, endpoint).unwrap();
    let (_, a) = check_proof(&a, endpoint);
    let (_, b) = check_proof(&b, endpoint);
    assert_ne!(a["jti"], b["jti"]);
    assert!(proof("not a key", endpoint).is_err());
    let lookup = |key: &str| {
        (key == "AWS_ENDPOINT_URL_SIGNIN")
            .then(|| "https://US-WEST-2.signin.aws.amazon.com:443/".to_owned())
    };
    let http = crate::provider::http::client();
    let cancel = CancellationToken::new();
    let normalized = super::endpoint(
        &ChainInput {
            env: &lookup,
            explicit_profile: None,
            ambient_profile: None,
            region: "us-west-2".into(),
            http: &http,
            cancel: &cancel,
        },
        &HashMap::new(),
    )
    .unwrap();
    assert_eq!(normalized, endpoint);
    check_proof(&proof(pem, &normalized).unwrap(), endpoint);
}

#[tokio::test]
async fn login_cache_and_refresh_match_executed_sdk_cases() {
    for case in reference()["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap().to_owned();
        let behavior = name.clone();
        let server = MockServer::start(move |_, _| {
            if behavior.ends_with("failure") {
                MockResponse::json(500, &json!({"message":"do-not-echo"}))
            } else if behavior == "access-denied" {
                MockResponse::json(
                    403,
                    &json!({"error":"TOKEN_EXPIRED","message":"do-not-echo"}),
                )
                .with_header("x-amzn-errortype", "AccessDeniedException")
            } else {
                let mut response = refreshed();
                if behavior == "refresh-default-expiry" {
                    response.as_object_mut().unwrap().remove("expiresIn");
                }
                MockResponse::json(200, &response)
            }
        })
        .await;
        let mut token = case["token"].clone();
        if name == "transient-failure" || name == "access-denied" {
            token["accessToken"]["expiresAt"] = json!(
                chrono::DateTime::from_timestamp((now_secs() + 120) as i64, 0)
                    .unwrap()
                    .to_rfc3339()
            );
        }
        let f = Fixture::new(&server.url, &token);
        let result = f.resolve(&CancellationToken::new()).await;
        if case.get("error").is_some() {
            let err = result.unwrap_err();
            assert!(err.contains("aws login"), "{err}");
            assert!(!err.contains("do-not-echo"));
        } else {
            let c = result.unwrap();
            assert_eq!(c.access_key_id, case["credentials"]["accessKeyId"]);
            assert_eq!(c.secret_access_key, case["credentials"]["secretAccessKey"]);
            assert_eq!(
                c.session_token.as_deref(),
                case["credentials"]["sessionToken"].as_str()
            );
            if name.starts_with("refresh") {
                let lifetime = c.expires_at.unwrap() - now_secs();
                let expected = if name == "refresh" { 3600 } else { 900 };
                assert!((expected - 5..=expected).contains(&lifetime));
            }
        }
        assert_eq!(
            f.saved()["accessToken"]["accessKeyId"],
            case["saved"]["accessKeyId"]
        );
        assert_eq!(f.saved()["refreshToken"], case["saved"]["refreshToken"]);
        assert_eq!(f.saved()["custom"], case["saved"]["custom"]);
        let requests = server.requests();
        assert_eq!(requests.len(), case["requests"].as_array().unwrap().len());
        if let Some(request) = requests.first() {
            assert_eq!(request.method, case["requests"][0]["method"]);
            assert_eq!(request.path, case["requests"][0]["path"]);
            assert_eq!(request.json(), case["requests"][0]["body"]);
            check_proof(
                request.header("dpop").unwrap(),
                &format!("{}/v1/token", server.url),
            );
            assert!(request.header("authorization").is_none());
        }
    }
}

#[tokio::test]
async fn cancelled_waiter_finishes_rotation_and_concurrent_callers_reuse_it() {
    let server = MockServer::start(|_, _| {
        let mut r = MockResponse::json(200, &refreshed());
        r.pause = Duration::from_millis(150);
        r
    })
    .await;
    let f = Fixture::new(&server.url, &reference()["cases"][1]["token"]);
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
    let (r, ()) = tokio::join!(f.resolve(&cancel), stop);
    assert_eq!(r.unwrap_err(), "Request was aborted");
    let fresh = CancellationToken::new();
    let (a, b) = tokio::join!(f.resolve(&fresh), f.resolve(&fresh));
    assert_eq!(a.unwrap().access_key_id, "NEW-ID");
    assert_eq!(b.unwrap().access_key_id, "NEW-ID");
    assert_eq!(server.requests().len(), 1);
    assert_eq!(f.saved()["refreshToken"], "new-refresh");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&f.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[tokio::test]
async fn refresh_write_does_not_overwrite_a_different_login() {
    let f = Fixture::new("https://localhost", &reference()["cases"][1]["token"]);
    let old = f.saved();
    let mut other = old.clone();
    other["refreshToken"] = json!("new-login");
    std::fs::write(&f.path, other.to_string()).unwrap();
    let mut new = old.clone();
    new["refreshToken"] = json!("refreshed");
    save(f.path.clone(), old, new).await.unwrap();
    assert_eq!(f.saved(), other);
}

#[tokio::test]
async fn home_cache_location_and_missing_key_validation() {
    let server =
        MockServer::start(|_, _| panic!("valid or malformed local cache must not call network"))
            .await;
    let mut f = Fixture::new(&server.url, &reference()["cases"][0]["token"]);
    f.vars.remove("AWS_LOGIN_CACHE_DIRECTORY");
    let path = f
        .dir
        .path()
        .join(".aws/login/cache")
        .join(f.path.file_name().unwrap());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::rename(&f.path, &path).unwrap();
    f.path = path;
    assert_eq!(
        f.resolve(&CancellationToken::new())
            .await
            .unwrap()
            .access_key_id,
        "OLD-ID"
    );
    let mut bad = f.saved();
    bad.as_object_mut().unwrap().remove("dpopKey");
    std::fs::write(&f.path, bad.to_string()).unwrap();
    assert!(
        f.resolve(&CancellationToken::new())
            .await
            .unwrap_err()
            .contains("dpopKey")
    );
    std::fs::write(&f.path, " ".repeat(LIMIT + 1)).unwrap();
    assert!(
        f.resolve(&CancellationToken::new())
            .await
            .unwrap_err()
            .contains("too large")
    );
}

#[tokio::test]
async fn malformed_denial_and_bad_refresh_payloads_do_not_replace_credentials() {
    for (status, body) in [
        (403, b"not JSON".to_vec()),
        (200, json!({"accessToken":{}}).to_string().into_bytes()),
        (
            200,
            json!({"large":"x".repeat(LIMIT)}).to_string().into_bytes(),
        ),
    ] {
        let server = MockServer::start(move |_, _| {
            let mut r = MockResponse::json(status, &json!({}));
            r.chunks = vec![body.clone()];
            r
        })
        .await;
        let mut old = reference()["cases"][1]["token"].clone();
        if status == 403 {
            old["accessToken"]["expiresAt"] = json!(
                chrono::DateTime::from_timestamp((now_secs() + 120) as i64, 0)
                    .unwrap()
                    .to_rfc3339()
            );
        }
        let f = Fixture::new(&server.url, &old);
        let err = f.resolve(&CancellationToken::new()).await.unwrap_err();
        if status == 403 {
            assert!(err.contains("denied"), "{err}");
        }
        assert_eq!(f.saved(), old);
    }
}

#[tokio::test]
async fn login_refresh_budget_releases_a_stalled_owner() {
    let server = MockServer::start(|_, _| {
        let mut r = MockResponse::json(200, &json!({}));
        r.chunks.clear();
        r.hang = true;
        r
    })
    .await;
    let f = Fixture::new(&server.url, &reference()["cases"][1]["token"]);
    let start = std::time::Instant::now();
    let err = f.resolve(&CancellationToken::new()).await.unwrap_err();
    assert!(
        err.contains("timed out") || err.contains("aws login"),
        "{err}"
    );
    assert!(start.elapsed() >= Duration::from_secs(29));
    assert!(start.elapsed() < Duration::from_secs(35));
    let lock = owner(&f.path).unwrap();
    let _guard = tokio::time::timeout(Duration::from_secs(1), lock.lock())
        .await
        .unwrap();
    assert_eq!(f.saved()["refreshToken"], "old-refresh");
}
