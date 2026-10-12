use super::*;
use crate::auth::tests::Fixture;
use crate::provider::mock::{MockResponse, MockServer};
use serde_json::json;
use std::sync::Arc;

fn catalog() -> Value {
    json!({"models":[{"id":"catalog-test","provider":"untrusted-other-provider","api":"openai-completions","name":"Catalog model","contextWindow":99999},
        {"id":"future-type","type":"unsupported-future"}, {"not-a-model":true}]})
}
fn response(value: &Value) -> MockResponse {
    MockResponse::json(200, value)
        .with_header("etag", "\"catalog-1\"")
        .with_header("last-modified", "Sun, 11 Oct 2026 12:00:00 GMT")
}
fn stale(fx: &Fixture) {
    let path = fx.config.config_dir.join("models-store.json");
    let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["openai"]["checkedAt"] = 0.into();
    std::fs::write(path, value.to_string()).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn refresh_restores_revalidates_and_keeps_user_overrides_without_sending_credentials() {
    let server = MockServer::start(|_, n| {
        if n == 0 {
            response(&catalog())
        } else {
            MockResponse::json(304, &Value::Null)
        }
    })
    .await;
    let mut fx = Fixture::new(&[("OPENAI_API_KEY", "secret-catalog-gate")]);
    fx.config.catalog_base_url = server.url.clone();
    std::fs::create_dir_all(&fx.config.config_dir).unwrap();
    std::fs::write(fx.config.models_path(),json!({"providers":{"openai":{"modelOverrides":{"catalog-test":{"name":"Local name","contextWindow":12345}}}}}).to_string()).unwrap();
    let auth = Auth::new(&fx.config);
    auth.refresh_models(&CancellationToken::new()).await;
    let first = auth.catalog();
    let model = first.find("openai/catalog-test").unwrap();
    assert_eq!(model.name, "Local name");
    assert_eq!(model.context_window, 12345);
    assert!(first.find("openai/future-type").is_none());
    assert!(
        first
            .find("untrusted-other-provider/catalog-test")
            .is_none()
    );
    assert!(first.warnings.is_empty(), "{:?}", first.warnings);
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0]
            .path
            .starts_with("/api/models/providers/openai?types=")
    );
    assert!(requests[0].header("authorization").is_none());
    assert!(requests[0].header("x-api-key").is_none());
    assert!(requests[0].header("if-none-match").is_none());
    assert!(!format!("{:?}", requests[0]).contains("secret-catalog-gate"));
    auth.refresh_models(&CancellationToken::new()).await;
    assert_eq!(server.requests().len(), 1, "fresh cache suppresses network");
    stale(&fx);
    auth.refresh_models(&CancellationToken::new()).await;
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].header("if-none-match"), Some("\"catalog-1\""));
    let cache: Value = serde_json::from_slice(
        &std::fs::read(fx.config.config_dir.join("models-store.json")).unwrap(),
    )
    .unwrap();
    assert!(cache["openai"]["checkedAt"].as_i64().unwrap() > 0);
    drop(auth);
    fx.config.env = crate::config::Env::isolated([("PI_OFFLINE".into(), "".into())].into());
    let offline = Auth::new(&fx.config);
    assert!(
        offline.catalog().find("openai/catalog-test").is_some(),
        "restore without auth or network"
    );
    offline.refresh_models(&CancellationToken::new()).await;
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn offline_presence_and_unconfigured_providers_never_contact_catalog() {
    let server = MockServer::start(|_, _| response(&catalog())).await;
    for vars in [
        vec![],
        vec![("OPENAI_API_KEY", "key"), ("PI_OFFLINE", "")],
        vec![("OPENAI_API_KEY", "key"), ("PI_OFFLINE", "0")],
    ] {
        let mut fx = Fixture::new(&vars);
        fx.config.catalog_base_url = server.url.clone();
        let auth = Auth::new(&fx.config);
        auth.refresh_models(&CancellationToken::new()).await;
    }
    assert!(server.requests().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn failures_retry_preserve_the_body_and_validator_and_publish_a_warning() {
    let server = MockServer::start(|_, n| {
        if n == 0 {
            response(&catalog())
        } else {
            MockResponse::json(503, &json!({"error":"unavailable"}))
        }
    })
    .await;
    let mut fx = Fixture::new(&[("OPENAI_API_KEY", "key")]);
    fx.config.catalog_base_url = server.url.clone();
    let auth = Auth::new(&fx.config);
    auth.refresh_models(&CancellationToken::new()).await;
    stale(&fx);
    auth.refresh_models(&CancellationToken::new()).await;
    assert_eq!(
        server.requests().len(),
        4,
        "initial fetch then three attempts"
    );
    assert!(
        server.requests()[1..]
            .iter()
            .all(|r| r.header("if-none-match") == Some("\"catalog-1\""))
    );
    assert!(auth.catalog().find("openai/catalog-test").is_some());
    assert!(auth.catalog().warnings.iter().any(|w| w.contains("503")));
    let saved: Value = serde_json::from_slice(
        &std::fs::read(fx.config.config_dir.join("models-store.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["openai"]["etag"], "\"catalog-1\"");
    assert!(saved["openai"]["checkedAt"].as_i64().unwrap() > 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn unsupported_catalog_status_keeps_current_overlay_but_invalidates_it_for_restore() {
    for status in [404, 501] {
        let server = MockServer::start(move |_, n| {
            if n == 0 {
                response(&catalog())
            } else {
                MockResponse::json(status, &Value::Null)
            }
        })
        .await;
        let mut fx = Fixture::new(&[("OPENAI_API_KEY", "key")]);
        fx.config.catalog_base_url = server.url.clone();
        let auth = Auth::new(&fx.config);
        auth.refresh_models(&CancellationToken::new()).await;
        stale(&fx);
        auth.refresh_models(&CancellationToken::new()).await;
        assert!(auth.catalog().find("openai/catalog-test").is_some());
        let reopened = Auth::new(&fx.config);
        assert!(reopened.catalog().find("openai/catalog-test").is_none());
        let saved: Value = serde_json::from_slice(
            &std::fs::read(fx.config.config_dir.join("models-store.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(saved["openai"]["lastModified"], 0);
        assert!(saved["openai"].get("etag").is_none());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn older_and_unversioned_remote_bodies_do_not_override_embedded_models() {
    for date in [None, Some("Sun, 04 Oct 2026 12:00:00 GMT")] {
        let server = MockServer::start(move |_, _| {
            let r = MockResponse::json(200, &catalog());
            if let Some(date) = date {
                r.with_header("last-modified", date)
            } else {
                r
            }
        })
        .await;
        let mut fx = Fixture::new(&[("OPENAI_API_KEY", "key")]);
        fx.config.catalog_base_url = server.url.clone();
        let auth = Auth::new(&fx.config);
        auth.refresh_models(&CancellationToken::new()).await;
        assert!(auth.catalog().find("openai/catalog-test").is_none());
        assert!(fx.config.config_dir.join("models-store.json").exists());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn superseded_and_cancelled_refreshes_never_publish_or_persist_old_results() {
    let server = MockServer::start(|_, n| {
        if n == 0 {
            let mut r = response(&json!([{"id":"old","api":"openai-completions"}]));
            r.pause = Duration::from_secs(30);
            r
        } else {
            response(&catalog())
        }
    })
    .await;
    let mut fx = Fixture::new(&[("OPENAI_API_KEY", "key")]);
    fx.config.catalog_base_url = server.url.clone();
    let auth = Arc::new(Auth::new(&fx.config));
    let first = auth.clone();
    let task = tokio::spawn(async move { first.refresh_models(&CancellationToken::new()).await });
    tokio::time::timeout(Duration::from_secs(3), async {
        while server.requests().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    auth.refresh_models(&CancellationToken::new()).await;
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    assert!(auth.catalog().find("openai/catalog-test").is_some());
    assert!(auth.catalog().find("openai/old").is_none());
    let saved = std::fs::read(fx.config.config_dir.join("models-store.json")).unwrap();
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    auth.refresh_models(&cancelled).await;
    assert_eq!(
        std::fs::read(fx.config.config_dir.join("models-store.json")).unwrap(),
        saved
    );
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn catalog_parser_accepts_reference_shapes_and_preserves_model_types() {
    for body in [
        json!([{"id":"x"}]),
        json!({"models":[{"id":"x"}]}),
        json!({"x":{"id":"x"}}),
    ] {
        assert_eq!(parse("openai", body).unwrap()[0].provider, "openai");
    }
    let models = parse("openai",json!([{"id":"same"},{"id":"same","type":"image"},{"id":"same","type":"classifier"},{"id":"skip","type":null}])).unwrap();
    assert_eq!(models.len(), 3);
    assert!(parse("openai", json!("invalid")).is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn corrupt_cache_is_preserved_and_failed_persistence_does_not_publish_new_models() {
    let server = MockServer::start(|_, _| response(&catalog())).await;
    let mut fx = Fixture::new(&[("OPENAI_API_KEY", "key")]);
    fx.config.catalog_base_url = server.url.clone();
    std::fs::create_dir_all(&fx.config.config_dir).unwrap();
    let path = fx.config.config_dir.join("models-store.json");
    std::fs::write(&path, b"{broken cache").unwrap();
    let auth = Auth::new(&fx.config);
    auth.refresh_models(&CancellationToken::new()).await;
    assert_eq!(std::fs::read(path).unwrap(), b"{broken cache");
    assert!(auth.catalog().find("openai/catalog-test").is_none());
    assert!(
        auth.catalog()
            .warnings
            .iter()
            .any(|w| w.contains("Could not save"))
    );
    assert!(auth.catalog().models.iter().any(|m| m.provider == "openai"));
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_cached_bodies_have_no_validator_and_malformed_refresh_retains_valid_cache() {
    let server = MockServer::start(|_, n| {
        if n == 0 {
            response(&catalog())
        } else {
            MockResponse::json(200, &json!("malformed catalog"))
        }
    })
    .await;
    let mut fx = Fixture::new(&[("OPENAI_API_KEY", "key")]);
    fx.config.catalog_base_url = server.url.clone();
    std::fs::create_dir_all(&fx.config.config_dir).unwrap();
    let path = fx.config.config_dir.join("models-store.json");
    std::fs::write(&path,json!({"openai":{"models":[],"checkedAt":0,"etag":"empty-body-tag"},"unrelated":{"retained":"opaque"}}).to_string()).unwrap();
    let auth = Auth::new(&fx.config);
    auth.refresh_models(&CancellationToken::new()).await;
    assert!(server.requests()[0].header("if-none-match").is_none());
    stale(&fx);
    let saved = std::fs::read(&path).unwrap();
    auth.refresh_models(&CancellationToken::new()).await;
    assert_eq!(std::fs::read(&path).unwrap(), saved);
    assert!(auth.catalog().find("openai/catalog-test").is_some());
    assert!(
        auth.catalog()
            .warnings
            .iter()
            .any(|w| w.contains("Invalid model catalog"))
    );
    let value: Value = serde_json::from_slice(&saved).unwrap();
    assert_eq!(value["unrelated"]["retained"], "opaque");
}

#[tokio::test(flavor = "multi_thread")]
async fn header_timeouts_retry_at_most_twice_and_body_stalls_are_bounded() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = accepted.clone();
    let task = tokio::spawn(async move {
        let mut held = vec![];
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
            count.fetch_add(1, Ordering::SeqCst);
        }
    });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let start = std::time::Instant::now();
    let result = fetch(
        &client,
        &base,
        "openai",
        None,
        generated_at(),
        &CancellationToken::new(),
        Duration::from_millis(50),
    )
    .await;
    assert!(result.is_err());
    assert_eq!(accepted.load(Ordering::SeqCst), 3);
    assert!(start.elapsed() < Duration::from_secs(2));
    task.abort();
    let server = MockServer::start(|_, _| {
        let mut r = response(&catalog());
        r.hang = true;
        r
    })
    .await;
    let result = fetch(
        &client,
        &server.url,
        "openai",
        None,
        generated_at(),
        &CancellationToken::new(),
        Duration::from_millis(50),
    )
    .await;
    assert!(result.is_err());
    assert_eq!(
        server.requests().len(),
        1,
        "body parsing happens after the management retry loop"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn refresh_uses_the_engine_proxy_without_provider_headers() {
    let target = MockServer::start(|_, _| response(&catalog())).await;
    let proxy = crate::provider::mock::TunnelProxy::start(&target.url, false).await;
    let mut fx = Fixture::new(&[
        ("OPENAI_API_KEY", "private-key"),
        ("HTTP_PROXY", &proxy.url),
    ]);
    fx.config.catalog_base_url = "http://catalog.invalid".into();
    let auth = Auth::new(&fx.config);
    auth.refresh_models(&CancellationToken::new()).await;
    assert!(auth.catalog().find("openai/catalog-test").is_some());
    let requests = proxy.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "CONNECT");
    assert_eq!(requests[0].path, "catalog.invalid:80");
    assert!(requests[0].header("authorization").is_none());
    let origin = target.requests();
    assert_eq!(origin.len(), 1);
    assert!(origin[0].path.starts_with("/api/models/providers/openai?"));
    assert!(origin[0].header("authorization").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn cancellation_during_a_response_leaves_no_cache_and_does_not_cancel_a_later_refresh() {
    let server = MockServer::start(|_, n| {
        let mut r = response(&catalog());
        if n == 0 {
            r.hang = true;
        }
        r
    })
    .await;
    let mut fx = Fixture::new(&[("OPENAI_API_KEY", "key")]);
    fx.config.catalog_base_url = server.url.clone();
    let auth = Arc::new(Auth::new(&fx.config));
    let first = auth.clone();
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let task = tokio::spawn(async move { first.refresh_models(&token).await });
    tokio::time::timeout(Duration::from_secs(3), async {
        while server.requests().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let precancelled = CancellationToken::new();
    precancelled.cancel();
    auth.refresh_models(&precancelled).await;
    assert!(
        !task.is_finished(),
        "an already cancelled request must not supersede live work"
    );
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    assert!(!fx.config.config_dir.join("models-store.json").exists());
    assert!(auth.catalog().find("openai/catalog-test").is_none());
    auth.refresh_models(&CancellationToken::new()).await;
    assert!(auth.catalog().find("openai/catalog-test").is_some());
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn explicit_configuration_reload_restores_a_newly_imported_cache_without_network() {
    let fx = Fixture::new(&[("OPENAI_API_KEY", "key"), ("PI_OFFLINE", "")]);
    let auth = Auth::new(&fx.config);
    assert!(auth.catalog().find("openai/catalog-test").is_none());
    std::fs::create_dir_all(&fx.config.config_dir).unwrap();
    let entry = Entry {
        models: parse("openai", catalog()).unwrap(),
        last_modified: Some(generated_at() + 1000),
        ..Default::default()
    };
    std::fs::write(
        fx.config.config_dir.join("models-store.json"),
        json!({"openai":entry}).to_string(),
    )
    .unwrap();
    auth.reload();
    assert!(auth.catalog().find("openai/catalog-test").is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn overlay_and_revalidation_match_eleven_executed_reference_cases() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../../fixtures/native-catalog/reference.json"
    ))
    .unwrap();
    assert_eq!(fixture["generatedAt"], generated_at());
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let response_case = case.clone();
        let server = MockServer::start(move |_, _| {
            let mut r = MockResponse::json(
                response_case["status"].as_u64().unwrap() as u16,
                &response_case["body"],
            )
            .with_header("etag", "\"new\"");
            if let Some(date) = response_case["lastModified"].as_str() {
                r = r.with_header("last-modified", date);
            }
            r
        })
        .await;
        let mut stored: Option<Entry> = case
            .get("stored")
            .map(|v| serde_json::from_value(v.clone()).unwrap());
        if case["name"] == "fresh" {
            stored.as_mut().unwrap().checked_at = Some(crate::util::now_ms());
        }
        let directory = tempfile::tempdir().unwrap();
        let cache = Cache::new(directory.path().join("models-store.json"));
        let restored = stored
            .as_ref()
            .map_or_else(Vec::new, |entry| cache.visible("openai", entry));
        let update = if case["offline"] == true {
            Update::default()
        } else {
            fetch(
                &client,
                &server.url,
                "openai",
                stored,
                generated_at(),
                &CancellationToken::new(),
                Duration::from_secs(2),
            )
            .await
            .unwrap()
        };
        let models: Vec<_> = update.models.unwrap_or(restored).iter().map(|m| json!({"id":m.id,"type":m.kind.as_deref().unwrap_or("chat"),"provider":m.provider})).collect();
        let requests: Vec<_> = server.requests().iter().map(|r| json!({"path":r.path.split('?').next().unwrap(),"validator":r.header("if-none-match")})).collect();
        let persisted = update.persist.map_or(Value::Null,|entry| json!({"checked":entry.checked_at.is_some_and(|at|at>0),"lastModified":entry.last_modified,"etag":entry.etag}));
        let expected = &case["expected"];
        assert_eq!(json!(models), expected["models"], "{} models", case["name"]);
        assert_eq!(
            json!(requests),
            expected["requests"],
            "{} requests",
            case["name"]
        );
        assert_eq!(persisted, expected["persisted"], "{} cache", case["name"]);
        assert_eq!(
            update.warning.is_some(),
            !expected["error"].is_null(),
            "{} error",
            case["name"]
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn image_and_classifier_models_cannot_shadow_a_chat_model_with_the_same_id() {
    let server = MockServer::start(|_, _| {
        response(&json!([
            {"id":"same","type":"image","api":"openrouter-images"},
            {"id":"same","type":"classifier","api":"typesafe"},
            {"id":"same","type":"chat","api":"openai-completions"}
        ]))
    })
    .await;
    let mut fx = Fixture::new(&[("OPENAI_API_KEY", "key")]);
    fx.config.catalog_base_url = server.url.clone();
    let auth = Auth::new(&fx.config);
    auth.refresh_models(&CancellationToken::new()).await;
    let catalog = auth.catalog();
    assert_eq!(
        catalog
            .models
            .iter()
            .filter(|m| m.provider == "openai" && m.id == "same")
            .count(),
        3
    );
    assert_eq!(
        catalog.find("openai/same").unwrap().api,
        "openai-completions"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_while_another_process_holds_the_cache_lock_does_not_publish_late() {
    use crate::auth::store::CredentialStore;
    let server = MockServer::start(|_, _| response(&catalog())).await;
    let mut fx = Fixture::new(&[("OPENAI_API_KEY", "key")]);
    fx.config.catalog_base_url = server.url.clone();
    let auth = Arc::new(Auth::new(&fx.config));
    let held = FileStore::new(fx.config.config_dir.join("models-store.json"))
        .begin()
        .unwrap();
    let token = CancellationToken::new();
    let call_token = token.clone();
    let first = auth.clone();
    let task = tokio::spawn(async move {
        first.refresh_models(&call_token).await;
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while server.requests().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    token.cancel();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    drop(held);
    assert!(!fx.config.config_dir.join("models-store.json").exists());
    assert!(auth.catalog().find("openai/catalog-test").is_none());
    auth.refresh_models(&CancellationToken::new()).await;
    assert!(auth.catalog().find("openai/catalog-test").is_some());
}

#[tokio::test]
async fn catalog_reports_invalid_proxy_configuration() {
    use crate::provider::{http, proxy::ProxyConfig};
    let server = MockServer::start(|_, _| response(&catalog())).await;
    let net = http::NetSettings {
        proxy: ProxyConfig::from_lookup(
            |key| (key == "HTTP_PROXY").then(|| "socks5://localhost:1080".into()),
            None,
        ),
        ..Default::default()
    };
    let client = http::client_with(&net);
    let error = fetch(
        &client,
        &server.url,
        "openai",
        None,
        0,
        &CancellationToken::new(),
        Duration::from_secs(1),
    )
    .await
    .err()
    .unwrap();
    assert!(
        error.contains(crate::provider::proxy::UNSUPPORTED_PROXY_PROTOCOL_MESSAGE),
        "{error}"
    );
    assert!(server.requests().is_empty());
}
