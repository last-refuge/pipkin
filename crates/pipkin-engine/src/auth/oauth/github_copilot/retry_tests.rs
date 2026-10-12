use super::*;
use crate::auth::oauth::Endpoints;
use crate::provider::mock::{MockResponse, MockServer};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

fn context() -> FlowCtx {
    let mut endpoints = Endpoints::from_env(&crate::config::Env::isolated(Default::default()));
    endpoints.retry_base = Some(Duration::from_millis(1));
    FlowCtx {
        http: crate::auth::tests::http(),
        endpoints,
        device_id: None,
        copilot_model_ids: vec![],
    }
}

#[test]
fn retry_after_decisions_match_executed_reference() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/native-copilot-retry/reference.json"
    )))
    .unwrap();
    let now = fixture["nowMs"].as_i64().unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let delay = match case["header"].as_str().filter(|s| !s.is_empty()) {
            Some(raw) => retry_after(raw, now),
            None => Some(Duration::from_millis(500)),
        }
        .filter(|d| *d < Duration::from_secs(5));
        let expected = case["delays"][0]
            .as_f64()
            .map(|ms| Duration::from_secs_f64(ms / 1000.0));
        assert_eq!(delay, expected, "header: {}", case["header"]);
    }
}

#[tokio::test]
async fn dates_prefix_numbers_and_invalid_headers_control_actual_http_retries() {
    for (header, expected) in [
        ("Sun, 11 Oct 2020 12:00:00 GMT", 3),
        ("0 trailing", 3),
        ("-1", 3),
        ("", 3),
        ("NaN", 1),
        ("Infinity", 1),
        ("1e999", 1),
        ("1e250", 1),
        ("9999", 1),
        ("not a date", 1),
    ] {
        let server = MockServer::start(move |_, _| {
            MockResponse::json(429, &json!({"error":"limited"})).with_header("retry-after", header)
        })
        .await;
        let ctx = context();
        let result = send_with_rate_limit_retry(
            &ctx,
            || ctx.http.get(&server.url),
            2,
            Duration::from_secs(5),
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.status, 429);
        assert_eq!(server.requests().len(), expected, "{header}");
    }
}

#[tokio::test]
async fn discards_a_stalled_rate_limit_body_before_retrying() {
    let server = MockServer::start(|_, n| {
        if n == 0 {
            MockResponse {
                hang: true,
                ..MockResponse::json(429, &json!({"limited":true})).with_header("retry-after", "0")
            }
        } else {
            MockResponse::json(200, &json!({"data":[]}))
        }
    })
    .await;
    let ctx = context();
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        send_with_rate_limit_retry(
            &ctx,
            || ctx.http.get(&server.url),
            2,
            Duration::from_secs(5),
            None,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result.status, 200);
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn overall_budget_bounds_a_later_attempt_and_its_body() {
    let server = MockServer::start(|_, n| {
        if n == 0 {
            MockResponse::json(429, &Value::Null).with_header("retry-after", "0")
        } else {
            MockResponse {
                chunks: vec![b"{".to_vec()],
                hang: true,
                ..MockResponse::json(200, &Value::Null)
            }
        }
    })
    .await;
    let ctx = context();
    let start = std::time::Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        send_with_rate_limit_retry(
            &ctx,
            || ctx.http.get(&server.url),
            2,
            Duration::from_millis(150),
            None,
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        result.err().as_deref(),
        Some("Copilot login retry budget expired")
    );
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn cancellation_interrupts_backoff_without_another_request() {
    let server = MockServer::start(|_, _| {
        MockResponse::json(429, &Value::Null).with_header("retry-after", "2")
    })
    .await;
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let url = server.url.clone();
    let task = tokio::spawn(async move {
        let ctx = context();
        send_with_rate_limit_retry(
            &ctx,
            || ctx.http.get(&url),
            2,
            Duration::from_secs(5),
            Some(&token),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while server.requests().is_empty() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_millis(200), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.err().as_deref(), Some(super::super::CANCELLED));
    assert_eq!(server.requests().len(), 1);
}
