use super::*;
use crate::provider::api::google_shared::tests::{model, request};
use crate::provider::mock::{MockResponse, MockServer};
use crate::provider::{Credential, ISOLATED_ENV, StreamCtx};
use std::path::Path;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn reference() -> Value {
    serde_json::from_str(include_str!(
        "../../../../../fixtures/native-google-aws/reference.json"
    ))
    .unwrap()
}
fn case(name: &str) -> Value {
    reference()["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap()
        .clone()
}
fn decode(subject: &str) -> Value {
    let form = format!("token={subject}");
    serde_json::from_str(
        &url::form_urlencoded::parse(form.as_bytes())
            .next()
            .unwrap()
            .1,
    )
    .unwrap()
}
fn header<'a>(proof: &'a Value, key: &str) -> &'a str {
    proof["headers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["key"] == key)
        .and_then(|h| h["value"].as_str())
        .unwrap_or("")
}
fn resign(subject: &str, time: chrono::DateTime<chrono::Utc>) -> String {
    let proof = decode(subject);
    let auth = header(&proof, "authorization");
    let parts: Vec<_> = auth
        .split("Credential=")
        .nth(1)
        .unwrap()
        .split(',')
        .next()
        .unwrap()
        .split('/')
        .collect();
    let credentials = AwsCredentials::new(
        parts[0],
        if parts[0].starts_with("ENV") {
            "env-secret"
        } else {
            "meta-secret"
        },
        Some(header(&proof, "x-amz-security-token")),
    );
    signed_subject(
        proof["url"].as_str().unwrap(),
        parts[2],
        header(&proof, "x-goog-cloud-target-resource"),
        &credentials,
        time,
    )
    .unwrap()
}
fn local_doc(case: &Value, server: &str) -> Value {
    serde_json::from_str(
        &case["document"]
            .to_string()
            .replace("http://metadata.example", server)
            .replace("https://sts.example", server),
    )
    .unwrap()
}
fn req(dir: &Path, doc: &Value, env: &Value) -> ProviderRequest {
    let path = dir.join("adc.json");
    std::fs::write(&path, doc.to_string()).unwrap();
    let mut r = request(model(
        json!({"id":"gemini-2.5-pro","api":"google-vertex","provider":"google-vertex","baseUrl":"https://example.invalid"}),
    ));
    r.credential = Credential::None;
    r.options.insert(ISOLATED_ENV.into(), json!(true));
    r.env = env
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().into()))
        .collect();
    r.env.insert(
        "GOOGLE_APPLICATION_CREDENTIALS".into(),
        path.display().to_string(),
    );
    r
}
async fn token(req: &ProviderRequest, cancel: CancellationToken) -> Result<String, String> {
    let http = crate::provider::http::client();
    crate::provider::api::google_auth::resolve(
        &StreamCtx {
            net: None,
            req,
            http: &http,
            events: None,
            cancel,
        },
        None,
    )
    .await
    .map(|t| t.token)
}
fn raw(text: &str) -> MockResponse {
    MockResponse {
        status: 200,
        headers: vec![],
        chunks: vec![text.as_bytes().to_vec()],
        pause: Duration::ZERO,
        hang: false,
    }
}
fn form(r: &crate::provider::mock::Recorded) -> Value {
    json!(
        url::form_urlencoded::parse(&r.body)
            .map(|(k, v)| (k.into_owned(), json!(v)))
            .collect::<serde_json::Map<_, _>>()
    )
}

#[test]
fn signed_proof_bytes_match_executed_google_sdk_vectors() {
    let fixture = reference();
    let fixed = chrono::DateTime::parse_from_rfc3339(fixture["timestamp"].as_str().unwrap())
        .unwrap()
        .to_utc();
    let mut vectors = 0;
    for case in fixture["cases"].as_array().unwrap() {
        for value in case["subjects"].as_array().unwrap() {
            let subject = value.as_str().unwrap();
            if !subject.starts_with('%') {
                continue;
            }
            assert_eq!(resign(subject, fixed), subject, "{}", case["name"]);
            vectors += 1;
        }
    }
    assert_eq!(vectors, 20);
}

#[tokio::test]
async fn aws_discovery_and_two_refreshes_match_thirteen_executed_sdk_cases() {
    for case in reference()["cases"].as_array().unwrap() {
        let counts = std::sync::Mutex::new([0; 4]);
        let server = MockServer::start(move |r, _| {
            let mut n = counts.lock().unwrap();
            match r.path.as_str() {
                "/imds" => { n[0] += 1; raw(&format!("metadata-token-{}", n[0])) },
                "/region" => { n[1] += 1; raw(if n[1] == 1 { "ap-southeast-2a" } else { "unwanted-second-region-a" }) },
                "/role" => { n[2] += 1; raw(&format!("role-{}", n[2])) },
                path if path.starts_with("/role/") => { n[3] += 1; MockResponse::json(200, &json!({"AccessKeyId":format!("METAKEY{}",n[3]), "SecretAccessKey":"meta-secret", "Token":format!("meta-session-{}",n[3])})) },
                _ => MockResponse::json(200, &json!({"access_token":"federated", "expires_in":1})),
            }
        }).await;
        let dir = tempfile::tempdir().unwrap();
        let doc = local_doc(case, &server.url);
        let req = req(dir.path(), &doc, &case["environment"]);
        if case["outcome"]["error"] == true {
            assert!(
                token(&req, CancellationToken::new()).await.is_err(),
                "{}",
                case["name"]
            );
        } else {
            for _ in 0..2 {
                assert_eq!(
                    token(&req, CancellationToken::new()).await.unwrap(),
                    "federated",
                    "{}",
                    case["name"]
                );
            }
        }
        let requests = server.requests();
        let expected = case["requests"].as_array().unwrap();
        assert_eq!(requests.len(), expected.len(), "{}", case["name"]);
        for (actual, expected) in requests.iter().zip(expected) {
            assert_eq!(actual.method, expected["method"]);
            assert_eq!(
                actual.path,
                url::Url::parse(expected["url"].as_str().unwrap())
                    .unwrap()
                    .path()
            );
            assert_eq!(
                actual.header("x-aws-ec2-metadata-token"),
                expected["metadataToken"].as_str()
            );
            assert_eq!(
                actual.header("x-aws-ec2-metadata-token-ttl-seconds"),
                expected["ttl"].as_str()
            );
            if expected["body"].is_null() {
                assert!(actual.body.is_empty());
                continue;
            }
            let actual_form = form(actual);
            let mut expected_form = expected["body"].clone();
            if expected_form["subject_token"]
                .as_str()
                .unwrap()
                .starts_with('%')
            {
                let proof = decode(actual_form["subject_token"].as_str().unwrap());
                let date = chrono::NaiveDateTime::parse_from_str(
                    header(&proof, "x-amz-date"),
                    "%Y%m%dT%H%M%SZ",
                )
                .unwrap()
                .and_utc();
                let now: chrono::DateTime<chrono::Utc> = std::time::SystemTime::now().into();
                assert!((now - date).num_seconds().abs() < 10);
                expected_form["subject_token"] = json!(resign(
                    expected_form["subject_token"].as_str().unwrap(),
                    date
                ));
            }
            assert_eq!(actual_form, expected_form, "{}", case["name"]);
        }
        assert_eq!(
            std::fs::read_to_string(dir.path().join("adc.json")).unwrap(),
            doc.to_string()
        );
    }
}

#[tokio::test]
async fn aws_environment_isolation_and_cancellation_preserve_shared_exchange() {
    let server = MockServer::start(|_, _| {
        let mut response =
            MockResponse::json(200, &json!({"access_token":"federated", "expires_in":3600}));
        response.pause = Duration::from_millis(180);
        response
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let case = case("session");
    let doc = local_doc(&case, &server.url);
    let r = req(dir.path(), &doc, &case["environment"]);
    let cancel = CancellationToken::new();
    let c = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        c.cancel();
    });
    assert_eq!(token(&r, cancel).await.unwrap_err(), "Request aborted");
    assert_eq!(
        token(&r, CancellationToken::new()).await.unwrap(),
        "federated"
    );
    assert_eq!(server.requests().len(), 1);
    let mut changed = r.clone();
    changed
        .env
        .insert("AWS_PROFILE".into(), "irrelevant-for-google".into());
    token(&changed, CancellationToken::new()).await.unwrap();
    assert_eq!(server.requests().len(), 1);
    changed
        .env
        .insert("AWS_SESSION_TOKEN".into(), "other-session".into());
    token(&changed, CancellationToken::new()).await.unwrap();
    assert_eq!(server.requests().len(), 2);
    let proof = decode(
        form(&server.requests()[1])["subject_token"]
            .as_str()
            .unwrap(),
    );
    assert_eq!(header(&proof, "x-amz-security-token"), "other-session");
    assert_eq!(super::super::execution_context(&r, Some(&doc)).0.len(), 4);
}

#[tokio::test]
async fn invalid_or_oversized_aws_metadata_never_reaches_sts_or_exposes_secrets() {
    // Malformed explicit selectors must fail, not silently pick an ordinary URL supplier.
    for selector in [json!(1), json!(true), json!({}), json!([])] {
        let mut doc = case("metadata-v1")["document"].clone();
        doc["credential_source"]["environment_id"] = selector;
        assert!(super::super::validate(&doc).is_err());
    }
    for failure in [
        "imds-error",
        "large-imds",
        "empty-zone",
        "empty-role",
        "bad-credentials",
        "large-credentials",
    ] {
        let label = failure.to_owned();
        let server = MockServer::start(move |r, _| match r.path.as_str() {
            "/imds" if label == "imds-error" => {
                MockResponse::json(403, &json!({"secret":"private-error"}))
            }
            "/imds" if label == "large-imds" => raw(&"secret".repeat(200_000)),
            "/imds" => raw("metadata-session"),
            "/region" => raw(if label == "empty-zone" {
                ""
            } else {
                "us-east-1a"
            }),
            "/role" => raw(if label == "empty-role" {
                ""
            } else {
                "test-role"
            }),
            _ if label == "large-credentials" => raw(&"secret".repeat(200_000)),
            _ => MockResponse::json(200, &json!({"secret":"private-response"})),
        })
        .await;
        let case = case("metadata-v2");
        let dir = tempfile::tempdir().unwrap();
        let doc = local_doc(&case, &server.url);
        let r = req(dir.path(), &doc, &json!({}));
        let error = token(&r, CancellationToken::new()).await.unwrap_err();
        assert!(
            !error.contains("secret") && !error.contains("private"),
            "{error}"
        );
        assert!(!server.requests().iter().any(|r| r.path == "/v1/token"));
    }
}

#[tokio::test]
async fn aws_metadata_timeout_releases_owner_and_allows_recovery() {
    let server = MockServer::start(|r,n| {
        if n == 0 { let mut response = raw("unused"); response.hang = true; return response; }
        match r.path.as_str() {
            "/region" => raw("us-east-1a"), "/role" => raw("role"),
            "/role/role" => MockResponse::json(200, &json!({"AccessKeyId":"METAKEY", "SecretAccessKey":"meta-secret", "Token":"meta-token"})),
            _ => MockResponse::json(200, &json!({"access_token":"federated", "expires_in":3600})),
        }
    }).await;
    let case = case("metadata-v1");
    let dir = tempfile::tempdir().unwrap();
    let doc = local_doc(&case, &server.url);
    let r = req(dir.path(), &doc, &json!({}));
    assert!(token(&r, CancellationToken::new()).await.is_err());
    assert_eq!(
        token(&r, CancellationToken::new()).await.unwrap(),
        "federated"
    );
    assert_eq!(server.requests().len(), 5);
}
