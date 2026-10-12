use super::*;
use crate::provider::api::google_shared::tests::{model, request};
use crate::provider::mock::{MockResponse, MockServer};
use crate::provider::{Credential, StreamCtx};
use serde_json::json;
use std::path::Path;
use tokio_util::sync::CancellationToken;

fn reference() -> Value {
    serde_json::from_str(include_str!(
        "../../../../../fixtures/native-google-executable/reference.json"
    ))
    .unwrap()
}
fn setup(dir: &Path, case: &Value) -> Value {
    let temp = dir.to_str().unwrap();
    std::fs::write(
        dir.join("helper.sh"),
        reference()["script"].as_str().unwrap(),
    )
    .unwrap();
    std::fs::write(dir.join("reply"), case["response"].to_string()).unwrap();
    if let Some(cache) = case["cache"].as_str() {
        std::fs::write(dir.join("output"), cache).unwrap();
    }
    serde_json::from_str(&case["document"].to_string().replace("$TEMP", temp)).unwrap()
}
fn env() -> BTreeMap<String, String> {
    BTreeMap::from([
        (ALLOW.into(), "1".into()),
        ("PIPKIN_HELPER_CUSTOM".into(), "custom-value".into()),
    ])
}
fn req(dir: &Path, doc: &Value) -> ProviderRequest {
    let path = dir.join("adc.json");
    std::fs::write(&path, doc.to_string()).unwrap();
    let mut r = request(model(
        json!({"id":"gemini-2.5-pro","api":"google-vertex","provider":"google-vertex","baseUrl":"https://example.invalid"}),
    ));
    r.credential = Credential::None;
    r.options.insert(ISOLATED_ENV.into(), json!(true));
    r.env = env().into_iter().collect();
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
fn successful() -> Value {
    reference()["cases"][0].clone()
}

#[tokio::test]
async fn helper_and_cache_outcomes_match_thirteen_executed_sdk_cases() {
    for case in reference()["cases"].as_array().unwrap() {
        let dir = tempfile::tempdir().unwrap();
        let doc = setup(dir.path(), case);
        let mut env = env();
        if case["name"] == "denied" {
            env.insert(ALLOW.into(), "0".into());
        }
        let result = subject(&doc, &env).await;
        if let Some(expected) = case["outcome"]["token"].as_str() {
            assert_eq!(result.as_deref(), Ok(expected), "{}", case["name"]);
        } else {
            assert!(result.is_err(), "{}", case["name"]);
        }
        let actual = std::fs::read_to_string(dir.path().join("env"))
            .ok()
            .map(|s| {
                s.trim_end()
                    .split('\n')
                    .map(|v| v.replace(dir.path().to_str().unwrap(), "$TEMP"))
                    .collect::<Vec<_>>()
            });
        assert_eq!(json!(actual), case["env"], "{}", case["name"]);
    }
    for case in reference()["parsing"].as_array().unwrap() {
        assert_eq!(
            json!(command(case["command"].as_str().unwrap()).unwrap()),
            case["args"]
        );
    }
}

#[tokio::test]
async fn helper_refresh_cancellation_and_environment_isolate_cached_identity() {
    let server = MockServer::start(|_, _| {
        MockResponse::json(200, &json!({"access_token":"federated","expires_in":3600}))
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let mut doc = setup(dir.path(), &successful());
    doc.as_object_mut()
        .unwrap()
        .remove("service_account_impersonation_url");
    doc["token_url"] = json!(format!("{}/sts", server.url));
    std::fs::write(
        dir.path().join("helper.sh"),
        "/bin/sleep 0.2\n/bin/cat \"$1\"\n",
    )
    .unwrap();
    let r = req(dir.path(), &doc);
    assert!(!execution_context(&r, Some(&doc)).0.contains_key("PATH"));
    let cancel = CancellationToken::new();
    let c = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(40)).await;
        c.cancel();
    });
    assert_eq!(token(&r, cancel).await.unwrap_err(), "Request aborted");
    assert_eq!(
        token(&r, CancellationToken::new()).await.unwrap(),
        "federated"
    );
    assert_eq!(server.requests().len(), 1);
    // Changing helper environment creates a distinct identity even with the same ADC file.
    let mut other = r.clone();
    other
        .env
        .insert("PIPKIN_HELPER_CUSTOM".into(), "other".into());
    assert_eq!(
        token(&other, CancellationToken::new()).await.unwrap(),
        "federated"
    );
    assert_eq!(server.requests().len(), 2);
    other.env.remove(ALLOW);
    assert!(
        token(&other, CancellationToken::new())
            .await
            .unwrap_err()
            .contains(ALLOW)
    );
    assert_eq!(server.requests().len(), 2);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("adc.json")).unwrap(),
        doc.to_string()
    );
}

#[tokio::test]
async fn helper_validation_and_output_limits_do_not_expose_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let mut doc = setup(dir.path(), &successful());
    for millis in [0, 4999, 120001] {
        doc["credential_source"]["executable"]["timeout_millis"] = json!(millis);
        assert!(validate(&doc["credential_source"]["executable"]).is_err());
    }
    doc["credential_source"]["executable"]["timeout_millis"] = json!(5000);
    let valid = successful()["response"].clone();
    let bad = vec![
        "secret-not-json".to_string(),
        json!({"version":1,"success":false,"code":"secret-code","message":"secret-message"}).to_string(),
        json!({"version":1,"success":true,"token_type":"secret-type"}).to_string(),
        json!({"version":1,"success":true,"token_type":"urn:ietf:params:oauth:token-type:jwt","id_token":"secret-token","expiration_time":1}).to_string(),
        "secret".repeat(LIMIT / 6 + 1),
    ];
    for text in bad {
        std::fs::write(dir.path().join("reply"), &text).unwrap();
        let err = subject(&doc, &env()).await.unwrap_err();
        assert!(!err.contains("secret"), "{err}");
        doc["credential_source"]["executable"]["output_file"] = json!(dir.path().join("output"));
        std::fs::write(dir.path().join("output"), &text).unwrap();
        // Expired/failure caches are misses, with the same failed reply from the helper.
        assert!(!subject(&doc, &env()).await.unwrap_err().contains("secret"));
        doc["credential_source"]["executable"]
            .as_object_mut()
            .unwrap()
            .remove("output_file");
    }
    std::fs::write(dir.path().join("reply"), valid.to_string()).unwrap();
    assert_eq!(subject(&doc, &env()).await.unwrap(), "helper-token");
    let mut no_expiry = valid;
    no_expiry.as_object_mut().unwrap().remove("expiration_time");
    std::fs::write(dir.path().join("reply"), no_expiry.to_string()).unwrap();
    assert_eq!(subject(&doc, &env()).await.unwrap(), "helper-token");
    // Closing pipes is not the same as process exit; wait for its successful status.
    std::fs::write(
        dir.path().join("helper.sh"),
        "/bin/cat \"$1\"\nexec 1>&- 2>&-\n/bin/sleep 0.1\n",
    )
    .unwrap();
    assert_eq!(subject(&doc, &env()).await.unwrap(), "helper-token");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn helper_timeout_kills_descendants_and_releases_refresh_for_retry() {
    let server = MockServer::start(|_, _| {
        MockResponse::json(200, &json!({"access_token":"federated","expires_in":3600}))
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let mut doc = setup(dir.path(), &successful());
    doc.as_object_mut()
        .unwrap()
        .remove("service_account_impersonation_url");
    doc["token_url"] = json!(format!("{}/sts", server.url));
    std::fs::write(
        dir.path().join("helper.sh"),
        "/bin/sleep 20 &\nprintf '%s' \"$!\" > \"$2\"\nwait\n",
    )
    .unwrap();
    let r = req(dir.path(), &doc);
    assert!(
        token(&r, CancellationToken::new())
            .await
            .unwrap_err()
            .contains("timed out")
    );
    let pid: u32 = std::fs::read_to_string(dir.path().join("env"))
        .unwrap()
        .parse()
        .unwrap();
    // A reparented dead child may be a zombie briefly; either absence or Z proves no execution.
    for n in 0..100 {
        let state = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok();
        if state.is_none_or(|s| s.rsplit_once(") ").is_some_and(|(_, t)| t.starts_with('Z'))) {
            break;
        }
        assert!(n < 99, "credential helper descendant still running");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(server.requests().is_empty());
    std::fs::write(dir.path().join("helper.sh"), "/bin/cat \"$1\"\n").unwrap();
    assert_eq!(
        token(&r, CancellationToken::new()).await.unwrap(),
        "federated"
    );
}

#[tokio::test]
async fn configured_helper_budget_can_exceed_thirty_seconds() {
    let server = MockServer::start(|_, _| {
        MockResponse::json(200, &json!({"access_token":"federated","expires_in":3600}))
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let mut doc = setup(dir.path(), &successful());
    doc.as_object_mut()
        .unwrap()
        .remove("service_account_impersonation_url");
    doc["token_url"] = json!(format!("{}/sts", server.url));
    doc["credential_source"]["executable"]["timeout_millis"] = json!(35000);
    std::fs::write(
        dir.path().join("helper.sh"),
        "/bin/sleep 31\n/bin/cat \"$1\"\n",
    )
    .unwrap();
    let r = req(dir.path(), &doc);
    assert_eq!(
        token(&r, CancellationToken::new()).await.unwrap(),
        "federated"
    );
    let (_, budget) = execution_context(&r, Some(&doc));
    assert_eq!(budget, Duration::from_secs(65));
    doc["credential_source"]["executable"]["timeout_millis"] = json!(120000);
    assert_eq!(
        execution_context(&r, Some(&doc)).1,
        Duration::from_secs(150)
    );
    let nested = json!({"type":"impersonated_service_account","source_credentials":doc});
    assert_eq!(
        execution_context(&r, Some(&nested)).1,
        Duration::from_secs(150)
    );
}
