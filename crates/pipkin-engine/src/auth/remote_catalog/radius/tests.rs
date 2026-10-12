use super::*;
use crate::auth::tests::{Fixture, http};
use crate::provider::mock::{MockResponse, MockServer};
use serde_json::json;
use std::sync::Arc;

fn config(base: &str, id: &str) -> Value {
    json!({"baseUrl":base,"models":[{"id":id,"name":"Gateway model","reasoning":true,"input":["text","image"],"cost":{"input":1,"output":2,"cacheRead":0,"cacheWrite":0},"contextWindow":32768,"maxTokens":4096},{"id":"invalid"}]})
}
fn saved(f: &Fixture) -> Value {
    serde_json::from_slice(&std::fs::read(f.config.config_dir.join("models-store.json")).unwrap())
        .unwrap()
}
async fn wait(server: &MockServer, count: usize) {
    for _ in 0..1000 {
        if server.requests().len() >= count {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("request did not arrive");
}

#[tokio::test]
async fn discovers_default_and_empty_custom_gateway_with_separate_credentials_and_local_overrides()
{
    let default = MockServer::start(|req, _| {
        assert_eq!(req.path, "/v1/config");
        assert_eq!(req.header("authorization"), Some("Bearer default-oauth"));
        MockResponse::json(200, &config("https://default.example/v1", "default-model"))
    })
    .await;
    let custom = MockServer::start(|req, _| {
        assert_eq!(req.path, "/v1/config");
        assert_eq!(req.header("authorization"), Some("Bearer custom-key"));
        MockResponse::json(200, &config("https://custom.example/v1", "custom-model"))
    })
    .await;
    let f = Fixture::new(&[("PI_RADIUS_GATEWAY", &default.url)]);
    f.write_models(&json!({"providers":{"team":{"oauth":"radius","baseUrl":format!("{}/v1", custom.url),"modelOverrides":{"custom-model":{"name":"Local name","contextWindow":12345}}}}}));
    f.write_auth(&json!({"radius":{"type":"oauth","access":"default-oauth","refresh":"r","expires":crate::util::now_ms()+3_600_000},"team":{"type":"api_key","key":"custom-key"}}));
    let auth = f.auth();
    assert!(
        !auth
            .catalog()
            .models
            .iter()
            .any(|m| m.provider == "team" || m.provider == "radius"),
        "custom gateway must not advertise default gateway models"
    );
    auth.refresh_models(&CancellationToken::new()).await;
    let catalog = auth.catalog();
    assert!(catalog.warnings.is_empty(), "{:?}", catalog.warnings);
    let a = catalog.find("radius/default-model").unwrap();
    assert_eq!(a.base_url, "https://default.example/v1");
    assert_eq!(a.api, "pi-messages");
    let b = catalog.find("team/custom-model").unwrap();
    assert_eq!(b.name, "Local name");
    assert_eq!(b.context_window, 12345);
    assert_eq!(b.base_url, "https://custom.example/v1");
    assert!(catalog.find("team/invalid").is_none());
    let cache = saved(&f);
    assert!(cache["team"]["lastModified"].is_null());
    assert!(cache["team"]["checkedAt"].as_i64().unwrap() > 0);
    assert!(!cache.to_string().contains("custom-key"));
    assert!(!cache.to_string().contains("default-oauth"));
    drop(auth);
    let reopened = f.auth();
    assert_eq!(
        reopened.catalog().find("team/custom-model").unwrap().name,
        "Local name"
    );
    assert_eq!((default.requests().len(), custom.requests().len()), (1, 1));
}

#[tokio::test]
async fn custom_radius_uses_radius_environment_key_and_fetches_each_explicit_refresh() {
    let server = MockServer::start(|req, _| {
        assert_eq!(
            req.header("authorization"),
            Some("Bearer shared-radius-key")
        );
        MockResponse::json(200, &config("https://gateway.example/v1", "discovered"))
    })
    .await;
    // Point both declared gateways locally; both intentionally use Radius's env credential.
    let f = Fixture::new(&[
        ("PI_RADIUS_GATEWAY", &server.url),
        ("RADIUS_API_KEY", "shared-radius-key"),
    ]);
    f.write_models(
        &json!({"providers":{"team":{"oauth":"radius","baseUrl":format!("{}/v1",server.url)}}}),
    );
    let auth = f.auth();
    for _ in 0..2 {
        auth.refresh_models(&CancellationToken::new()).await;
    }
    assert_eq!(
        server.requests().len(),
        4,
        "private catalog is not suppressed by public freshness TTL"
    );
    assert!(auth.catalog().find("team/discovered").is_some());
    let m = auth.catalog().find("team/discovered").unwrap().clone();
    assert_eq!(
        auth.resolve(&m, &http()).await.unwrap().credential.secret(),
        Some("shared-radius-key")
    );
}

#[tokio::test]
async fn legacy_catalog_survives_direct_token_rotation_then_migrates_offline_independently() {
    let server = MockServer::start(|req, _| {
        assert_eq!(req.path, "/v1/oauth/token");
        MockResponse::json(
            200,
            &json!({"access_token":"new","refresh_token":"rotated","expires_in":3600}),
        )
    })
    .await;
    let f = Fixture::new(&[("PI_RADIUS_GATEWAY", &server.url), ("PI_OFFLINE", "")]);
    f.write_auth(&json!({"radius":{"type":"oauth","access":"old","refresh":"r","expires":1,"gatewayConfig":config("https://legacy.example/v1","legacy")}}));
    let auth = f.auth();
    let m = auth.catalog().find("radius/legacy").unwrap().clone();
    auth.resolve(&m, &http()).await.unwrap();
    assert_eq!(f.read_auth()["radius"]["refresh"], "rotated");
    assert!(auth.catalog().find("radius/legacy").is_some());
    auth.refresh_models(&CancellationToken::new()).await;
    assert_eq!(saved(&f)["radius"]["models"][0]["id"], "legacy");
    f.write_auth(&json!({}));
    drop(auth);
    let reopened = f.auth();
    reopened.refresh_models(&CancellationToken::new()).await;
    assert!(
        reopened.catalog().find("radius/legacy").is_some(),
        "cached models outlive the legacy credential"
    );
    assert_eq!(
        server.requests().len(),
        1,
        "offline catalog restore makes no network requests"
    );
}

#[tokio::test]
async fn failed_catalogs_keep_cached_models_and_never_expose_response_secrets() {
    for failure in [
        MockResponse::json(401, &json!({"echo":"private-key"})),
        MockResponse::json(200, &json!({"invalid":"private-key"})),
        MockResponse::json(200, &json!({"baseUrl":"bad","models":"wrong"})),
        MockResponse {
            chunks: vec![vec![b'x'; MAX_BODY + 1]],
            ..MockResponse::json(200, &Value::Null)
        },
    ] {
        let server = MockServer::start(move |_, n| {
            if n == 0 {
                MockResponse::json(200, &config("https://gateway.example/v1", "previous"))
            } else {
                failure.clone()
            }
        })
        .await;
        let f = Fixture::new(&[
            ("PI_RADIUS_GATEWAY", &server.url),
            ("RADIUS_API_KEY", "private-key"),
        ]);
        let auth = f.auth();
        auth.refresh_models(&CancellationToken::new()).await;
        let before = saved(&f);
        auth.refresh_models(&CancellationToken::new()).await;
        assert_eq!(saved(&f), before);
        assert!(auth.catalog().find("radius/previous").is_some());
        assert!(!auth.catalog().warnings.is_empty());
        assert!(!format!("{:?}", auth.catalog().warnings).contains("private-key"));
        assert_eq!(server.requests().len(), 2);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn superseded_refresh_cannot_overwrite_new_catalog_and_cancellation_keeps_cache() {
    let server = MockServer::start(|_, n| {
        let value = config(
            "https://gateway.example/v1",
            match n {
                0 => "initial",
                1 => "stale",
                2 => "current",
                _ => "cancelled",
            },
        );
        let response = MockResponse::json(200, &value);
        if n == 1 || n == 3 {
            let bytes = value.to_string().into_bytes();
            MockResponse {
                chunks: vec![bytes[..1].to_vec(), bytes[1..].to_vec()],
                pause: Duration::from_millis(200),
                ..response
            }
        } else {
            response
        }
    })
    .await;
    let f = Fixture::new(&[
        ("PI_RADIUS_GATEWAY", &server.url),
        ("RADIUS_API_KEY", "key"),
    ]);
    let auth = Arc::new(f.auth());
    auth.refresh_models(&CancellationToken::new()).await;
    let old = auth.clone();
    let task = tokio::spawn(async move { old.refresh_models(&CancellationToken::new()).await });
    wait(&server, 2).await;
    auth.refresh_models(&CancellationToken::new()).await;
    task.await.unwrap();
    assert!(auth.catalog().find("radius/current").is_some());
    assert!(auth.catalog().find("radius/stale").is_none());
    let before = saved(&f);
    let cancel = CancellationToken::new();
    let (owned, token) = (auth.clone(), cancel.clone());
    let task = tokio::spawn(async move { owned.refresh_models(&token).await });
    wait(&server, 4).await;
    cancel.cancel();
    task.await.unwrap();
    assert_eq!(saved(&f), before);
    assert!(auth.catalog().find("radius/current").is_some());
}

#[tokio::test]
async fn empty_gateway_catalog_replaces_old_overlay_and_wrong_provider_cache_is_filtered() {
    let server = MockServer::start(|_, _| {
        MockResponse::json(
            200,
            &json!({"baseUrl":"https://gateway.example/v1","models":[]}),
        )
    })
    .await;
    let f = Fixture::new(&[
        ("PI_RADIUS_GATEWAY", &server.url),
        ("RADIUS_API_KEY", "key"),
    ]);
    f.write_auth(&json!({}));
    let mut models = parse("radius", &config("https://old.example/v1", "old")).unwrap();
    models.extend(parse("other", &config("https://other.example/v1", "foreign")).unwrap());
    std::fs::write(
        f.config.config_dir.join("models-store.json"),
        json!({"radius":{"models":models,"checkedAt":1}}).to_string(),
    )
    .unwrap();
    let auth = f.auth();
    assert!(auth.catalog().find("radius/old").is_some());
    assert!(auth.catalog().find("other/foreign").is_none());
    auth.refresh_models(&CancellationToken::new()).await;
    assert!(!auth.catalog().models.iter().any(|m| m.provider == "radius"));
    assert_eq!(saved(&f)["radius"]["models"], json!([]));
}

#[test]
fn sanitizer_and_provider_mapping_match_executed_reference_cases() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/native-radius-catalog/reference.json"
    )))
    .unwrap();
    assert_eq!(
        fixture["reference"],
        "d2a311097cbcf669e699479587332ae3988a49d0"
    );
    for case in fixture["cases"].as_array().unwrap() {
        let actual = parse(case["provider"].as_str().unwrap(), &case["config"]).unwrap_or_default();
        let expected: Vec<Model> = serde_json::from_value(case["expected"].clone()).unwrap();
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(expected).unwrap(),
            "{}",
            case["name"]
        );
    }
}

#[tokio::test]
async fn stalled_gateway_body_is_bounded_and_failed_cache_writes_keep_previous_models() {
    let server = MockServer::start(|_, n| match n {
        0 => MockResponse::json(200, &config("https://gateway.example/v1", "previous")),
        1 => MockResponse {
            chunks: vec![b"{".to_vec()],
            hang: true,
            ..MockResponse::json(200, &Value::Null)
        },
        _ => MockResponse::json(200, &config("https://gateway.example/v1", "replacement")),
    })
    .await;
    let f = Fixture::new(&[
        ("PI_RADIUS_GATEWAY", &server.url),
        ("RADIUS_API_KEY", "key"),
    ]);
    let auth = f.auth();
    auth.refresh_models(&CancellationToken::new()).await;
    let before = saved(&f);
    let start = std::time::Instant::now();
    tokio::time::timeout(
        Duration::from_secs(6),
        auth.refresh_models(&CancellationToken::new()),
    )
    .await
    .unwrap();
    assert!(start.elapsed() >= Duration::from_secs(3));
    assert_eq!(saved(&f), before);
    let path = f.config.config_dir.join("models-store.json");
    std::fs::write(&path, b"invalid cache; preserve me").unwrap();
    auth.refresh_models(&CancellationToken::new()).await;
    assert_eq!(std::fs::read(&path).unwrap(), b"invalid cache; preserve me");
    assert!(auth.catalog().find("radius/previous").is_some());
    assert!(auth.catalog().find("radius/replacement").is_none());
    assert!(
        auth.catalog()
            .warnings
            .iter()
            .any(|s| s.contains("Could not save"))
    );
}
