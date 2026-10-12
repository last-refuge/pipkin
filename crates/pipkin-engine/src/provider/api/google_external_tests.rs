use super::*;
use crate::provider::api::google_shared::tests::{model, request};
use crate::provider::mock::{MockResponse, MockServer};
use crate::provider::{Credential, ISOLATED_ENV, ProviderRequest, StreamCtx};
use std::path::Path;
use tokio_util::sync::CancellationToken;

fn reference() -> Value {
    serde_json::from_str(include_str!(
        "../../../../../fixtures/native-google-external/reference.json"
    ))
    .unwrap()
}
fn req(dir: &Path, doc: &Value) -> ProviderRequest {
    let path = dir.join("adc.json");
    std::fs::write(&path, doc.to_string()).unwrap();
    let mut r = request(model(
        json!({"id":"gemini-2.5-pro","api":"google-vertex","provider":"google-vertex","baseUrl":"https://example.invalid"}),
    ));
    r.credential = Credential::None;
    r.env.clear();
    r.options.insert(ISOLATED_ENV.into(), json!(true));
    r.env.insert(
        "GOOGLE_APPLICATION_CREDENTIALS".into(),
        path.display().to_string(),
    );
    r
}
fn document(case: &Value, dir: &Path, server: &str) -> Value {
    let mut doc = case["document"].clone();
    if doc["credential_source"].get("file").is_some() {
        let path = dir.join("subject");
        std::fs::write(&path, case["subject"].as_str().unwrap()).unwrap();
        doc["credential_source"]["file"] = json!(path.display().to_string());
    }
    for key in ["token_url", "service_account_impersonation_url"] {
        if let Some(url) = doc[key].as_str() {
            let url = url::Url::parse(url).unwrap();
            doc[key] = json!(format!("{server}{}", url.path()));
        }
    }
    if let Some(url) = doc["credential_source"]["url"].as_str() {
        let url = url::Url::parse(url).unwrap();
        doc["credential_source"]["url"] = json!(format!("{server}{}", url.path()));
    }
    doc
}
async fn token(
    req: &ProviderRequest,
    cancel: CancellationToken,
) -> Result<super::super::AccessToken, String> {
    let http = crate::provider::http::client();
    super::super::resolve(
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
fn form(req: &crate::provider::mock::Recorded) -> Value {
    json!(
        url::form_urlencoded::parse(&req.body)
            .map(|(k, v)| (k.into_owned(), json!(v)))
            .collect::<serde_json::Map<String, Value>>()
    )
}

#[tokio::test]
async fn federation_and_external_user_grants_match_eight_executed_sdk_cases() {
    for case in reference()["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap().to_owned();
        let behavior = name.clone();
        let server = MockServer::start(move |r, n| {
            if r.path == "/subject" {
                MockResponse::json(200, &json!({"jwt":"url-token"}))
            } else if r.path.ends_with("generateAccessToken") {
                MockResponse::json(200, &json!({"accessToken":"impersonated-token","expireTime":"2099-01-01T00:00:00Z"}))
            } else {
                let mut body = json!({"access_token":"federated-token","expires_in":3600});
                if behavior == "no-expiry" {
                    body.as_object_mut().unwrap().remove("expires_in");
                }
                if behavior == "external-user" {
                    body["expires_in"] = json!(1);
                    body["refresh_token"] = json!(format!("rotated-{}", n+1));
                }
                MockResponse::json(200, &body)
            }
        }).await;
        let dir = tempfile::tempdir().unwrap();
        let doc = document(case, dir.path(), &server.url);
        let req = req(dir.path(), &doc);
        let first = token(&req, CancellationToken::new()).await.unwrap();
        assert_eq!(first.token, case["tokens"][0]);
        assert_eq!(first.quota_project.as_deref(), case["quota"].as_str());
        if name == "no-expiry" {
            std::fs::remove_file(dir.path().join("subject")).unwrap();
        }
        token(&req, CancellationToken::new()).await.unwrap();
        let actual = server.requests();
        let expected = case["requests"].as_array().unwrap();
        assert_eq!(actual.len(), expected.len(), "{name}");
        for (a, e) in actual.iter().zip(expected) {
            assert_eq!(a.method, e["method"]);
            assert_eq!(
                a.path,
                url::Url::parse(e["url"].as_str().unwrap()).unwrap().path()
            );
            assert_eq!(a.header("authorization"), e["authorization"].as_str());
            assert_eq!(a.header("x-identity"), e["identity"].as_str());
            let body = if a.path.ends_with("generateAccessToken") {
                a.json()
            } else if a.method == "GET" {
                Value::Null
            } else {
                form(a)
            };
            assert_eq!(body, e["body"], "{name}");
        }
        // The SDK keeps rotated external-user refresh tokens in memory; it does not edit ADC files.
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(dir.path().join("adc.json")).unwrap())
                .unwrap(),
            doc
        );
    }
}

#[tokio::test]
async fn subject_files_reload_on_refresh_and_camel_case_aliases_are_accepted() {
    let server = MockServer::start(|r, _| {
        MockResponse::json(
            200,
            &json!({"access_token":form(r)["subject_token"],"expires_in":1}),
        )
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let mut doc = document(&reference()["cases"][0], dir.path(), &server.url);
    for (snake, camel) in [
        ("credential_source", "credentialSource"),
        ("subject_token_type", "subjectTokenType"),
        ("token_url", "tokenUrl"),
        ("quota_project_id", "quotaProjectId"),
    ] {
        let v = doc.as_object_mut().unwrap().remove(snake).unwrap();
        doc[camel] = v;
    }
    let req = req(dir.path(), &doc);
    assert_eq!(
        token(&req, CancellationToken::new()).await.unwrap().token,
        "raw-token\n"
    );
    std::fs::write(dir.path().join("subject"), "changed-subject\n").unwrap();
    assert_eq!(
        token(&req, CancellationToken::new()).await.unwrap().token,
        "changed-subject\n"
    );
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn cancelled_external_user_refresh_retains_the_rotated_token() {
    let server=MockServer::start(|_,n|{let mut r=MockResponse::json(200,&json!({"access_token":"token","expires_in":1,"refresh_token":format!("rotated-{}",n+1)}));if n==0{r.pause=Duration::from_millis(150);}r}).await;
    let dir = tempfile::tempdir().unwrap();
    let doc = document(&reference()["cases"][7], dir.path(), &server.url);
    let req = req(dir.path(), &doc);
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
    let (r, ()) = tokio::join!(token(&req, cancel.clone()), stop);
    assert_eq!(r.unwrap_err(), "Request aborted");
    token(&req, CancellationToken::new()).await.unwrap();
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(form(&requests[1])["refresh_token"], "rotated-1");
}

#[tokio::test]
async fn invalid_subjects_and_configuration_fail_before_sts() {
    let server = MockServer::start(|_, _| panic!("invalid source must not reach STS")).await;
    for variant in [
        "empty",
        "bad-json",
        "missing-field",
        "oversized",
        "ambiguous",
        "invalid-workforce",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut doc = document(&reference()["cases"][1], dir.path(), &server.url);
        match variant {
            "empty" => {
                doc["credential_source"]["format"] = json!({"type":"text"});
                std::fs::write(dir.path().join("subject"), "").unwrap();
            }
            "bad-json" => std::fs::write(dir.path().join("subject"), "not json").unwrap(),
            "missing-field" => std::fs::write(dir.path().join("subject"), "{}").unwrap(),
            "oversized" => {
                std::fs::write(dir.path().join("subject"), "x".repeat(LIMIT + 1)).unwrap()
            }
            "ambiguous" => doc["credential_source"]["url"] = json!(server.url),
            "invalid-workforce" => {
                doc["workforce_pool_user_project"] = json!("not-a-workforce-audience")
            }
            _ => unreachable!(),
        }
        assert!(
            token(&req(dir.path(), &doc), CancellationToken::new())
                .await
                .is_err(),
            "{variant}"
        );
    }
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn failed_and_oversized_subject_and_sts_responses_expose_no_remote_secrets() {
    for stage in [
        "subject-error",
        "subject-big",
        "sts-error",
        "sts-big",
        "bad-expiry",
    ] {
        let server = MockServer::start(move |r, _| {
            if r.path == "/subject" {
                if stage == "subject-error" {
                    MockResponse::json(403, &json!({"error":"private-remote-secret"}))
                } else if stage == "subject-big" {
                    MockResponse::json(200, &json!({"jwt":"x".repeat(LIMIT)}))
                } else {
                    MockResponse::json(200, &json!({"jwt":"subject-token"}))
                }
            } else if stage == "sts-error" {
                MockResponse::json(400, &json!({"error":"private-remote-secret"}))
            } else if stage == "sts-big" {
                MockResponse::json(200, &json!({"access_token":"x".repeat(LIMIT)}))
            } else {
                MockResponse::json(200, &json!({"access_token":"token","expires_in":u64::MAX}))
            }
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let doc = document(&reference()["cases"][2], dir.path(), &server.url);
        let err = token(&req(dir.path(), &doc), CancellationToken::new())
            .await
            .unwrap_err();
        assert!(!err.contains("private-remote-secret"), "{err}");
        assert!(!err.contains("subject-token"));
    }
}

#[tokio::test]
async fn stalled_subject_supplier_releases_the_owner_and_allows_a_later_exchange() {
    let server = MockServer::start(|r, n| {
        if n == 0 {
            let mut response = MockResponse::json(200, &json!({}));
            response.chunks.clear();
            response.hang = true;
            response
        } else if r.path == "/subject" {
            MockResponse::json(200, &json!({"jwt":"subject-token"}))
        } else {
            MockResponse::json(
                200,
                &json!({"access_token":"recovered-token","expires_in":3600}),
            )
        }
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let doc = document(&reference()["cases"][2], dir.path(), &server.url);
    let req = req(dir.path(), &doc);
    let start = std::time::Instant::now();
    assert!(
        tokio::time::timeout(
            Duration::from_secs(35),
            token(&req, CancellationToken::new())
        )
        .await
        .unwrap()
        .is_err()
    );
    assert!(start.elapsed() >= Duration::from_secs(29));
    let next = tokio::time::timeout(
        Duration::from_secs(3),
        token(&req, CancellationToken::new()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(next.token, "recovered-token");
    assert_eq!(server.requests().len(), 3);
}
