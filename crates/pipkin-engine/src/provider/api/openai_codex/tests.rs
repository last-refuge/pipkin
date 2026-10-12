//! Tests for the Codex adapter.
//!
//! `fixtures/*.spec.json` + `*.golden.json` are differential goldens: each golden was produced by
//! running the reference `openai-codex-responses.ts` `stream` (pinned rev, read-only import,
//! npm packages stubbed, an injected `fetch` capturing the request and replaying the spec's
//! scripted responses). The Rust adapter must send the same request (headers and the
//! zstd-decoded body) and produce the same message from the same bytes.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use pipkin_engine_protocol::{Block, Delta, Message, StopReason};
use reqwest::header::HeaderValue;
use serde_json::{Map, Value, json};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::provider::mock::{MockResponse, MockServer, Recorded};
use crate::provider::{
    ApiAdapter, CacheRetention, Credential, Model, ProviderRequest, StreamCtx, StreamEvent,
    ThinkingLevel, ToolSpec, errors,
};

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

fn test_model(base: &str) -> Model {
    serde_json::from_value(json!({
        "id": "gpt-5.5",
        "name": "GPT-5.5",
        "api": API,
        "provider": "openai-codex",
        "baseUrl": base,
        "reasoning": true,
        "input": ["text", "image"],
        "cost": {"input": 5, "output": 30, "cacheRead": 0.5, "cacheWrite": 0},
        "contextWindow": 272000,
        "maxTokens": 128000,
        "thinkingLevelMap": {"xhigh": "xhigh", "minimal": "low"},
    }))
    .unwrap()
}

fn oauth(token: &str, account: Option<&str>) -> Credential {
    let mut metadata = Map::new();
    if let Some(a) = account {
        metadata.insert(ACCOUNT_ID_METADATA_KEY.into(), Value::String(a.into()));
    }
    Credential::OAuth {
        access: token.into(),
        metadata,
    }
}

fn base_request(base: &str, messages: Vec<Message>) -> ProviderRequest {
    ProviderRequest {
        model: test_model(base),
        system: "You are Pipkin.".into(),
        messages,
        tools: vec![],
        thinking: ThinkingLevel::Off,
        session_id: String::new(),
        max_tokens: None,
        cache_retention: CacheRetention::Short,
        credential: oauth("tok", Some("acct_meta")),
        headers: BTreeMap::new(),
        base_url: None,
        timeout: Duration::from_secs(30),
        env: HashMap::new(),
        options: Map::new(),
    }
}

fn user(text: &str) -> Message {
    Message::user_text(text, 1)
}

async fn run_with(
    adapter: &OpenAICodexResponses,
    req: &ProviderRequest,
    cancel: CancellationToken,
) -> (Message, Vec<Delta>) {
    let http = crate::provider::http::client();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let msg = adapter
        .stream(StreamCtx {
            req,
            http: &http,
            events: Some(tx.into()),
            cancel,
        })
        .await;
    let mut deltas = Vec::new();
    while let Ok(StreamEvent::Delta(d)) = rx.try_recv() {
        deltas.push(d);
    }
    (msg, deltas)
}

async fn run(adapter: &OpenAICodexResponses, req: &ProviderRequest) -> Message {
    run_with(adapter, req, CancellationToken::new()).await.0
}

fn stop_and_error(msg: &Message) -> (StopReason, Option<String>) {
    match msg {
        Message::Assistant {
            stop_reason,
            error_message,
            ..
        } => (*stop_reason, error_message.clone()),
        _ => panic!("not an assistant message"),
    }
}

fn extra(msg: &Message) -> &Map<String, Value> {
    match msg {
        Message::Assistant { extra, .. } => extra,
        _ => panic!("not an assistant message"),
    }
}

fn sse_text(events: &[Value]) -> String {
    let mut out = String::new();
    for e in events {
        out.push_str(&format!(
            "event: {}\ndata: {e}\n\n",
            e["type"].as_str().unwrap()
        ));
    }
    out
}

fn message_events(resp: &str, msg_id: &str, text: &str) -> Vec<Value> {
    vec![
        json!({"type": "response.created", "response": {"id": resp}}),
        json!({"type": "response.output_item.added", "output_index": 0,
               "item": {"type": "message", "id": msg_id, "role": "assistant", "content": []}}),
        json!({"type": "response.output_text.delta", "output_index": 0, "delta": text}),
        json!({"type": "response.output_item.done", "output_index": 0,
               "item": {"type": "message", "id": msg_id, "role": "assistant",
                        "content": [{"type": "output_text", "text": text, "annotations": []}]}}),
        json!({"type": "response.completed",
               "response": {"id": resp, "status": "completed",
                            "usage": {"input_tokens": 10, "output_tokens": 2, "total_tokens": 12}}}),
    ]
}

fn decoded_body(r: &Recorded) -> Value {
    assert_eq!(r.header("content-encoding"), Some("zstd"));
    let raw = zstd::decode_all(&r.body[..]).expect("valid zstd body");
    serde_json::from_slice(&raw).expect("JSON body")
}

// ---------------------------------------------------------------------------------------------
// Differential goldens (reference-produced)
// ---------------------------------------------------------------------------------------------

fn request_from_spec(spec: &Value, base: &str) -> ProviderRequest {
    let mut model: Model = serde_json::from_value(spec["model"].clone()).unwrap();
    model.base_url = base.into();
    let messages: Vec<Message> = serde_json::from_value(spec["messages"].clone()).unwrap();
    let tools = spec["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| ToolSpec {
            constrained_sampling: t.get("constrainedSampling").cloned(),
            name: t["name"].as_str().unwrap().into(),
            description: t["description"].as_str().unwrap().into(),
            parameters: t["parameters"].clone(),
        })
        .collect();
    let opts = spec["options"].as_object().unwrap();
    let mut options = Map::new();
    let mut headers = BTreeMap::new();
    for (k, v) in opts {
        match k.as_str() {
            "apiKey" | "sessionId" | "cacheRetention" => {}
            "headers" => {
                for (hk, hv) in v.as_object().unwrap() {
                    headers.insert(hk.clone(), hv.as_str().map(str::to_owned));
                }
            }
            _ => {
                options.insert(k.clone(), v.clone());
            }
        }
    }
    ProviderRequest {
        model,
        system: spec["systemPrompt"].as_str().unwrap_or_default().into(),
        messages,
        tools,
        thinking: ThinkingLevel::Off,
        session_id: opts
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        max_tokens: None,
        cache_retention: match opts.get("cacheRetention").and_then(Value::as_str) {
            Some("none") => CacheRetention::None,
            Some("long") => CacheRetention::Long,
            _ => CacheRetention::Short,
        },
        // No metadata: the account id comes from the JWT, as in the reference.
        credential: oauth(opts["apiKey"].as_str().unwrap(), None),
        headers,
        base_url: None,
        timeout: Duration::from_secs(30),
        env: HashMap::new(),
        options,
    }
}

fn mock_reply(reply: &Value, fragment: bool) -> MockResponse {
    if let Some(pieces) = reply.get("sse").and_then(Value::as_array) {
        let text: String = pieces.iter().map(|p| p.as_str().unwrap()).collect();
        let bytes = text.into_bytes();
        // 7-byte chunks split UTF-8 sequences, field names and JSON payloads.
        let chunks = if fragment {
            bytes.chunks(7).map(<[u8]>::to_vec).collect()
        } else {
            vec![bytes]
        };
        return MockResponse {
            chunks,
            ..MockResponse::sse(vec![])
        };
    }
    let mut resp = MockResponse {
        status: reply["status"].as_u64().unwrap() as u16,
        headers: vec![],
        chunks: vec![reply["body"].as_str().unwrap().as_bytes().to_vec()],
        pause: Duration::ZERO,
        hang: false,
    };
    if let Some(h) = reply.get("headers").and_then(Value::as_object) {
        for (k, v) in h {
            resp = resp.with_header(k, v.as_str().unwrap());
        }
    }
    resp
}

fn assert_block_eq(name: &str, got: &Value, want: &Value) {
    if want["type"] == "thinking" {
        assert_eq!(got["thinking"], want["thinking"], "{name}: thinking text");
        let parse = |v: &Value| -> Value {
            v.as_str()
                .map(|s| serde_json::from_str(s).unwrap())
                .unwrap_or(Value::Null)
        };
        assert_eq!(
            parse(&got["thinkingSignature"]),
            parse(&want["thinkingSignature"]),
            "{name}: thinking signature"
        );
    } else {
        assert_eq!(got, want, "{name}: block");
    }
}

async fn check_golden(name: &str, spec: &str, golden: &str) {
    let spec: Value = serde_json::from_str(spec).unwrap();
    let golden: Value = serde_json::from_str(golden).unwrap();
    let replies = spec["replies"].as_array().unwrap().clone();
    let fragment = name == "full";
    let server =
        MockServer::start(move |_, i| mock_reply(&replies[i.min(replies.len() - 1)], fragment))
            .await;
    let req = request_from_spec(&spec, &server.url);
    let msg = run(&OpenAICodexResponses::new(), &req).await;

    // Requests.
    let recorded = server.requests();
    let want_requests = golden["requests"].as_array().unwrap();
    assert_eq!(recorded.len(), want_requests.len(), "{name}: request count");
    for (got, want) in recorded.iter().zip(want_requests) {
        assert_eq!(got.method, "POST", "{name}");
        assert_eq!(got.path, "/codex/responses", "{name}");
        let want_headers = want["headers"].as_object().unwrap();
        for (k, v) in want_headers {
            if k == "user-agent" {
                assert!(v.as_str().unwrap().starts_with("pi ("), "{name}");
                assert_eq!(got.header(k), Some(pi_user_agent().as_str()), "{name}: {k}");
            } else {
                assert_eq!(got.header(k), v.as_str(), "{name}: header {k}");
            }
        }
        for (k, _) in &got.headers {
            assert!(
                want_headers.contains_key(k) || k == "host" || k == "content-length",
                "{name}: unexpected header {k}"
            );
        }
        assert_eq!(decoded_body(got), want["body"], "{name}: body");
    }

    // Message.
    let want = &golden["message"];
    let got = serde_json::to_value(&msg).unwrap();
    assert_eq!(got["stopReason"], want["stopReason"], "{name}: stopReason");
    match name {
        "invalid_json" => {
            let text = got["errorMessage"].as_str().unwrap();
            assert!(text.starts_with("Invalid Codex SSE JSON: "), "{text}");
        }
        _ => assert_eq!(
            got.get("errorMessage"),
            want.get("errorMessage"),
            "{name}: errorMessage"
        ),
    }
    let got_content = got["content"].as_array().unwrap();
    let want_content = want["content"].as_array().unwrap();
    assert_eq!(got_content.len(), want_content.len(), "{name}: content");
    for (g, w) in got_content.iter().zip(want_content) {
        assert_block_eq(name, g, w);
    }
    for key in [
        "input",
        "output",
        "cacheRead",
        "cacheWrite",
        "cacheWrite1h",
        "reasoning",
        "totalTokens",
    ] {
        assert_eq!(got["usage"][key], want["usage"][key], "{name}: usage.{key}");
    }
    for key in ["input", "output", "cacheRead", "cacheWrite", "total"] {
        let g = got["usage"]["cost"][key].as_f64().unwrap();
        let w = want["usage"]["cost"][key].as_f64().unwrap();
        assert!((g - w).abs() < 1e-12, "{name}: cost.{key} {g} vs {w}");
    }
    for key in ["responseId", "rawStopReason", "endTurn"] {
        assert_eq!(got.get(key), want.get(key), "{name}: {key}");
    }
}

macro_rules! goldens {
    ($($name:ident),* $(,)?) => {
        mod golden {
            $(
                #[tokio::test]
                async fn $name() {
                    super::check_golden(
                        stringify!($name),
                        include_str!(concat!("fixtures/", stringify!($name), ".spec.json")),
                        include_str!(concat!("fixtures/", stringify!($name), ".golden.json")),
                    )
                    .await;
                }
            )*
        }
    };
}

goldens!(
    full,
    plain,
    cross_model,
    content_filter,
    failed_event,
    error_event,
    no_terminal,
    unfinished_tool,
    invalid_json,
    http_400,
    http_500_text,
    http_429_rate,
    usage_limit_no_reset,
    retry_503,
    retry_delay_cap,
    retry_400_catch,
    retry_usage_limit_429,
    retry_terminal_429,
);

// ---------------------------------------------------------------------------------------------
// SSE behaviour beyond the goldens
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn fragmented_stream_streams_deltas_and_records_signatures() {
    let spec: Value = serde_json::from_str(include_str!("fixtures/full.spec.json")).unwrap();
    let replies = spec["replies"].as_array().unwrap().clone();
    let server = MockServer::start(move |_, _| mock_reply(&replies[0], true)).await;
    let req = request_from_spec(&spec, &server.url);
    let (msg, deltas) =
        run_with(&OpenAICodexResponses::new(), &req, CancellationToken::new()).await;
    let text: String = deltas
        .iter()
        .filter_map(|d| match d {
            Delta::Text { block: 1, text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "Let me read h\u{e9}llo \u{1f600} the file.");
    assert!(deltas.contains(&Delta::ToolCall {
        block: 2,
        id: "call_9|fc_9".into(),
        name: "read".into()
    }));
    let Message::Assistant { content, .. } = &msg else {
        panic!()
    };
    let Block::Text {
        extra: block_extra, ..
    } = &content[1]
    else {
        panic!()
    };
    assert_eq!(
        block_extra["textSignature"],
        r#"{"v":1,"id":"msg_9","phase":"commentary"}"#
    );
    assert_eq!(extra(&msg)["responseId"], "resp_1");
    assert_eq!(extra(&msg)["endTurn"], false);
}

#[tokio::test]
async fn cancellation_mid_stream_is_aborted_and_keeps_partial_content() {
    let pieces = sse_text(&[
        json!({"type": "response.created", "response": {"id": "resp_c"}}),
        json!({"type": "response.output_item.added", "output_index": 0,
               "item": {"type": "message", "id": "m", "role": "assistant", "content": []}}),
        json!({"type": "response.output_text.delta", "output_index": 0, "delta": "partial"}),
    ]);
    let server = MockServer::start(move |_, _| MockResponse {
        hang: true,
        ..MockResponse::sse(vec![pieces.clone()])
    })
    .await;
    let mut req = base_request(&server.url, vec![user("hi")]);
    req.options.insert("transport".into(), json!("sse"));
    let adapter = OpenAICodexResponses::new();
    let http = crate::provider::http::client();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    let stream = adapter.stream(StreamCtx {
        req: &req,
        http: &http,
        events: Some(tx.into()),
        cancel,
    });
    let watcher = async move {
        while let Some(StreamEvent::Delta(d)) = rx.recv().await {
            if matches!(d, Delta::Text { .. }) {
                trigger.cancel();
                break;
            }
        }
    };
    let (msg, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(stream, watcher)
    })
    .await
    .expect("cancellation ends the stream");
    assert_eq!(stop_and_error(&msg), (StopReason::Aborted, None));
    assert_eq!(msg.text(), "partial");
}

#[tokio::test]
async fn usage_limit_error_reports_plan_and_reset_and_is_not_retryable() {
    let resets_at = crate::util::now_ms() / 1000 + 600;
    let body = json!({"error": {"code": "usage_limit_reached", "plan_type": "PLUS",
                                "resets_at": resets_at, "message": "limit"}});
    let server = MockServer::start(move |_, _| MockResponse::json(429, &body)).await;
    let mut req = base_request(&server.url, vec![user("hi")]);
    req.options.insert("transport".into(), json!("sse"));
    let msg = run(&OpenAICodexResponses::new(), &req).await;
    let (stop, error) = stop_and_error(&msg);
    assert_eq!(stop, StopReason::Error);
    let error = error.unwrap();
    assert!(
        error == "You have hit your ChatGPT usage limit (plus plan). Try again in ~10 min."
            || error == "You have hit your ChatGPT usage limit (plus plan). Try again in ~9 min.",
        "{error}"
    );
    assert!(!errors::is_retryable(&msg));
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn header_timeout_is_reported_like_the_reference() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let hold = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((sock, _)) = listener.accept().await {
            held.push(sock);
        }
    });
    let mut req = base_request(&url, vec![user("hi")]);
    req.options.insert("transport".into(), json!("sse"));
    req.timeout = Duration::from_millis(200);
    let msg = run(&OpenAICodexResponses::new(), &req).await;
    assert_eq!(
        stop_and_error(&msg),
        (
            StopReason::Error,
            Some("Codex SSE response headers timed out after 200ms".into())
        )
    );
    assert!(errors::is_retryable(&msg));
    hold.abort();
}

#[tokio::test]
async fn credentials_and_account_ids_resolve_like_the_reference() {
    let server = MockServer::start(|_, _| {
        MockResponse::sse(vec![sse_text(&message_events("r", "m", "ok"))])
    })
    .await;
    let adapter = OpenAICodexResponses::new();
    let mut req = base_request(&server.url, vec![user("hi")]);
    req.options.insert("transport".into(), json!("sse"));

    // Metadata wins and a non-JWT token is fine with it.
    let msg = run(&adapter, &req).await;
    assert_eq!(stop_and_error(&msg).0, StopReason::Stop);
    let r = &server.requests()[0];
    assert_eq!(r.header("chatgpt-account-id"), Some("acct_meta"));
    assert_eq!(r.header("authorization"), Some("Bearer tok"));

    // Without metadata a non-JWT token cannot yield an account id.
    req.credential = oauth("tok", None);
    let msg = run(&adapter, &req).await;
    assert_eq!(
        stop_and_error(&msg),
        (
            StopReason::Error,
            Some("Failed to extract accountId from token".into())
        )
    );

    req.credential = Credential::None;
    let msg = run(&adapter, &req).await;
    assert_eq!(
        stop_and_error(&msg).1.as_deref(),
        Some("No API key for provider: openai-codex")
    );
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn reasoning_effort_follows_the_thinking_level() {
    let mut req = base_request("http://x", vec![user("hi")]);
    let opts = CodexOptions::parse(&Map::new()).unwrap();
    let effort =
        |req: &ProviderRequest| build_request_body(req, &opts, None).unwrap()["reasoning"].clone();
    assert_eq!(effort(&req), json!({"effort": "none"}));
    req.thinking = ThinkingLevel::High;
    assert_eq!(effort(&req), json!({"effort": "high", "summary": "auto"}));
    req.thinking = ThinkingLevel::Minimal;
    assert_eq!(effort(&req), json!({"effort": "low", "summary": "auto"}));
    // `max` is unmapped for this model: clamps down to `xhigh`.
    req.thinking = ThinkingLevel::Max;
    assert_eq!(effort(&req), json!({"effort": "xhigh", "summary": "auto"}));
    req.model.reasoning = false;
    req.thinking = ThinkingLevel::Off;
    assert_eq!(effort(&req), Value::Null);
    let explicit = CodexOptions::parse(
        json!({"reasoningEffort": "none", "reasoningSummary": "detailed"})
            .as_object()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        build_request_body(&req, &explicit, None).unwrap()["reasoning"],
        json!({"effort": "none", "summary": "detailed"})
    );
}

#[test]
fn urls_resolve_like_the_reference() {
    assert_eq!(
        resolve_codex_url(""),
        "https://chatgpt.com/backend-api/codex/responses"
    );
    assert_eq!(
        resolve_codex_url("https://x.test/api/"),
        "https://x.test/api/codex/responses"
    );
    assert_eq!(
        resolve_codex_url("https://x.test/codex"),
        "https://x.test/codex/responses"
    );
    assert_eq!(
        resolve_codex_url("https://x.test/codex/responses//"),
        "https://x.test/codex/responses"
    );
    assert_eq!(
        resolve_codex_websocket_url("https://chatgpt.com/backend-api"),
        "wss://chatgpt.com/backend-api/codex/responses"
    );
    assert_eq!(
        resolve_codex_websocket_url("http://127.0.0.1:9"),
        "ws://127.0.0.1:9/codex/responses"
    );
}

#[test]
fn retry_after_parsing_matches_the_reference() {
    assert_eq!(
        parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT"),
        Some(784_111_777_000)
    );
    let mut h = HeaderMap::new();
    h.insert("retry-after-ms", HeaderValue::from_static("250"));
    h.insert("retry-after", HeaderValue::from_static("9"));
    assert_eq!(retry_after_delay_ms(&h), Some(250));
    h.remove("retry-after-ms");
    assert_eq!(retry_after_delay_ms(&h), Some(9000));
    h.insert(
        "retry-after",
        HeaderValue::from_static("Sun, 06 Nov 1994 08:49:37 GMT"),
    );
    assert_eq!(retry_after_delay_ms(&h), Some(0));
    assert!(is_retryable_error(503, ""));
    assert!(!is_retryable_error(429, "insufficient_quota"));
    assert!(is_retryable_error(400, "upstream connect error"));
    assert!(!is_retryable_error(400, "bad request"));
}

#[test]
fn jwt_account_ids_decode() {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let payload =
        URL_SAFE_NO_PAD.encode(br#"{"https://api.openai.com/auth":{"chatgpt_account_id":"a-1"}}"#);
    assert_eq!(
        account_id_from_jwt(&format!("x.{payload}.y")),
        Some("a-1".into())
    );
    assert_eq!(account_id_from_jwt("not-a-jwt"), None);
    assert_eq!(uuid_v7().len(), 36);
    assert_eq!(&uuid_v7()[14..15], "7");
}

// ---------------------------------------------------------------------------------------------
// WebSocket transport
// ---------------------------------------------------------------------------------------------

enum WsReply {
    Event(Value),
    Close(u16, &'static str),
}

type Script = dyn Fn(&Value, usize, usize) -> Vec<WsReply> + Send + Sync;

/// A local WebSocket server: records handshake headers and `response.create` frames and
/// answers each frame from a script `(frame, frame index, connection index)`.
struct WsMock {
    url: String,
    frames: Arc<Mutex<Vec<Value>>>,
    handshakes: Arc<Mutex<Vec<HashMap<String, String>>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for WsMock {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl WsMock {
    async fn start(
        script: impl Fn(&Value, usize, usize) -> Vec<WsReply> + Send + Sync + 'static,
    ) -> WsMock {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let frames: Arc<Mutex<Vec<Value>>> = Arc::default();
        let handshakes: Arc<Mutex<Vec<HashMap<String, String>>>> = Arc::default();
        let script: Arc<Script> = Arc::new(script);
        let connections = Arc::new(AtomicUsize::new(0));
        let (f, h) = (frames.clone(), handshakes.clone());
        let task = tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                let (frames, handshakes, script) = (f.clone(), h.clone(), script.clone());
                let conn = connections.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    // The callback signature is fixed by tungstenite.
                    #[allow(clippy::result_large_err)]
                    let record = move |req: &Request, resp: Response| {
                        let headers = req
                            .headers()
                            .iter()
                            .map(|(k, v)| {
                                (k.as_str().to_owned(), v.to_str().unwrap_or("").to_owned())
                            })
                            .collect();
                        handshakes.lock().unwrap().push(headers);
                        Ok(resp)
                    };
                    let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(sock, record).await else {
                        return;
                    };
                    while let Some(Ok(msg)) = ws.next().await {
                        let WsMessage::Text(text) = msg else { continue };
                        let frame: Value = serde_json::from_str(text.as_str()).unwrap();
                        let index = {
                            let mut all = frames.lock().unwrap();
                            all.push(frame.clone());
                            all.len() - 1
                        };
                        for reply in script(&frame, index, conn) {
                            match reply {
                                WsReply::Event(e) => {
                                    if ws
                                        .send(WsMessage::Text(e.to_string().into()))
                                        .await
                                        .is_err()
                                    {
                                        return;
                                    }
                                }
                                WsReply::Close(code, reason) => {
                                    let _ = ws
                                        .close(Some(CloseFrame {
                                            code: code.into(),
                                            reason: reason.into(),
                                        }))
                                        .await;
                                    // Drain until the peer closes.
                                    while let Some(Ok(_)) = ws.next().await {}
                                    return;
                                }
                            }
                        }
                    }
                });
            }
        });
        WsMock {
            url,
            frames,
            handshakes,
            task,
        }
    }

    fn frames(&self) -> Vec<Value> {
        self.frames.lock().unwrap().clone()
    }

    fn handshakes(&self) -> Vec<HashMap<String, String>> {
        self.handshakes.lock().unwrap().clone()
    }
}

fn events(list: Vec<Value>) -> Vec<WsReply> {
    list.into_iter().map(WsReply::Event).collect()
}

fn answer(index: usize) -> Vec<WsReply> {
    events(message_events(
        &format!("resp_{index}"),
        &format!("msg_{index}"),
        &format!("answer {index}"),
    ))
}

fn ws_request(base: &str, messages: Vec<Message>) -> ProviderRequest {
    let mut req = base_request(base, messages);
    req.session_id = "ws-sess".into();
    req
}

#[tokio::test]
async fn websocket_reuses_the_connection_and_sends_only_the_input_delta() {
    let mock = WsMock::start(|_, index, _| answer(index)).await;
    let adapter = OpenAICodexResponses::new();
    let first = run(&adapter, &ws_request(&mock.url, vec![user("hi")])).await;
    assert_eq!(stop_and_error(&first), (StopReason::Stop, None));
    assert_eq!(first.text(), "answer 0");
    let second_req = ws_request(&mock.url, vec![user("hi"), first.clone(), user("more")]);
    let second = run(&adapter, &second_req).await;
    assert_eq!(second.text(), "answer 1");
    assert_eq!(extra(&second)["responseId"], "resp_1");

    let handshakes = mock.handshakes();
    assert_eq!(handshakes.len(), 1, "the pooled connection is reused");
    let h = &handshakes[0];
    assert_eq!(h["authorization"], "Bearer tok");
    assert_eq!(h["chatgpt-account-id"], "acct_meta");
    assert_eq!(h["openai-beta"], "responses_websockets=2026-02-06");
    assert_eq!(h["session-id"], "ws-sess");
    assert_eq!(h["x-client-request-id"], "ws-sess");
    assert_eq!(h["originator"], "pi");
    assert_eq!(h["user-agent"], pi_user_agent());
    for absent in ["accept", "content-type", "content-encoding"] {
        assert!(!h.contains_key(absent), "{absent}");
    }

    let frames = mock.frames();
    assert_eq!(frames.len(), 2);
    // The first frame is the full body plus the message type.
    let mut expected = build_request_body(
        &ws_request(&mock.url, vec![user("hi")]),
        &CodexOptions::parse(&Map::new()).unwrap(),
        Some("ws-sess"),
    )
    .unwrap();
    expected["type"] = json!("response.create");
    assert_eq!(frames[0], expected);
    // The second continues the first response with only the new input.
    assert_eq!(frames[1]["previous_response_id"], "resp_0");
    assert_eq!(
        frames[1]["input"],
        json!([{"role": "user", "content": [{"type": "input_text", "text": "more"}]}])
    );

    let stats = adapter.websocket_debug_stats("ws-sess").unwrap();
    assert_eq!(stats.requests, 2);
    assert_eq!(stats.connections_created, 1);
    assert_eq!(stats.connections_reused, 1);
    assert_eq!(stats.delta_requests, 1);
    assert_eq!(stats.full_context_requests, 1);
    assert_eq!(stats.last_previous_response_id.as_deref(), Some("resp_0"));
    assert_eq!(stats.websocket_failures, 0);
}

#[tokio::test]
async fn websocket_plain_transport_never_uses_previous_response_ids() {
    let mock = WsMock::start(|_, index, _| answer(index)).await;
    let adapter = OpenAICodexResponses::new();
    let mut req = ws_request(&mock.url, vec![user("hi")]);
    req.options.insert("transport".into(), json!("websocket"));
    let first = run(&adapter, &req).await;
    req.messages = vec![user("hi"), first, user("more")];
    let second = run(&adapter, &req).await;
    assert_eq!(second.text(), "answer 1");
    let frames = mock.frames();
    assert!(frames[1].get("previous_response_id").is_none());
    assert_eq!(frames[1]["input"].as_array().unwrap().len(), 3);
    assert_eq!(mock.handshakes().len(), 1);
}

#[tokio::test]
async fn previous_response_not_found_retries_once_with_the_full_context() {
    let mock = WsMock::start(|frame, index, _| {
        if frame.get("previous_response_id").is_some() {
            events(vec![json!({"type": "error", "status": 400, "error": {
                "type": "invalid_request_error", "code": "previous_response_not_found",
                "message": "Previous response with id 'resp_0' not found."}})])
        } else {
            answer(index)
        }
    })
    .await;
    let adapter = OpenAICodexResponses::new();
    let first = run(&adapter, &ws_request(&mock.url, vec![user("hi")])).await;
    let second = run(
        &adapter,
        &ws_request(&mock.url, vec![user("hi"), first, user("more")]),
    )
    .await;
    assert_eq!(stop_and_error(&second), (StopReason::Stop, None));
    assert_eq!(second.text(), "answer 2");
    let frames = mock.frames();
    assert_eq!(frames.len(), 3);
    assert!(frames[1].get("previous_response_id").is_some());
    assert!(frames[2].get("previous_response_id").is_none());
    assert_eq!(frames[2]["input"].as_array().unwrap().len(), 3);
    // The failed connection was dropped; the retry opened a new one.
    assert_eq!(mock.handshakes().len(), 2);
    assert!(extra(&second).get("diagnostics").is_none());
}

#[tokio::test]
async fn connection_limit_before_start_retries_once() {
    let mock = WsMock::start(|_, index, conn| {
        if conn == 0 {
            events(vec![json!({"type": "error", "error": {
                "code": "websocket_connection_limit_reached", "message": "Too many connections"}})])
        } else {
            answer(index)
        }
    })
    .await;
    let msg = run(
        &OpenAICodexResponses::new(),
        &ws_request(&mock.url, vec![user("hi")]),
    )
    .await;
    assert_eq!(stop_and_error(&msg), (StopReason::Stop, None));
    assert_eq!(mock.handshakes().len(), 2);
}

#[tokio::test]
async fn api_errors_on_the_websocket_do_not_fall_back() {
    let mock = WsMock::start(|_, _, _| {
        events(vec![json!({"type": "error", "error": {
            "code": "context_length_exceeded", "message": "Your input exceeds the context window of this model."}})])
    })
    .await;
    let adapter = OpenAICodexResponses::new();
    let msg = run(&adapter, &ws_request(&mock.url, vec![user("hi")])).await;
    assert_eq!(
        stop_and_error(&msg),
        (
            StopReason::Error,
            Some("Codex error: Your input exceeds the context window of this model.".into())
        )
    );
    assert!(errors::is_context_overflow(&msg, 272_000));
    assert!(extra(&msg).get("diagnostics").is_none());
    assert_eq!(
        adapter
            .websocket_debug_stats("ws-sess")
            .unwrap()
            .websocket_failures,
        0
    );
}

#[tokio::test]
async fn websocket_close_after_start_is_a_retryable_error_without_fallback() {
    let mock = WsMock::start(|_, _, _| {
        vec![
            WsReply::Event(json!({"type": "response.created", "response": {"id": "resp_x"}})),
            WsReply::Close(1011, "server error"),
        ]
    })
    .await;
    let adapter = OpenAICodexResponses::new();
    let msg = run(&adapter, &ws_request(&mock.url, vec![user("hi")])).await;
    assert_eq!(
        stop_and_error(&msg),
        (
            StopReason::Error,
            Some("WebSocket closed 1011 server error".into())
        )
    );
    assert!(errors::is_retryable(&msg));
    let diagnostics = extra(&msg)["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["type"], "provider_transport_failure");
    assert_eq!(diagnostics[0]["error"]["name"], "WebSocketCloseError");
    assert_eq!(diagnostics[0]["error"]["code"], 1011);
    assert_eq!(
        diagnostics[0]["details"]["phase"],
        "after_message_stream_start"
    );
    assert_eq!(diagnostics[0]["details"]["eventsEmitted"], true);
    assert!(diagnostics[0]["details"].get("fallbackTransport").is_none());
    // The session now prefers SSE.
    let stats = adapter.websocket_debug_stats("ws-sess").unwrap();
    assert_eq!(stats.websocket_failures, 1);
    assert_eq!(stats.websocket_fallback_active, Some(true));
}

#[tokio::test]
async fn websocket_failure_before_start_falls_back_to_sse_for_the_session() {
    // An HTTP-only server: the WebSocket upgrade fails, the SSE POST succeeds.
    let server = MockServer::start(|r, _| {
        if r.method == "GET" {
            MockResponse::json(404, &json!({"error": "no websockets here"}))
        } else {
            MockResponse::sse(vec![sse_text(&message_events("resp_s", "m", "via sse"))])
        }
    })
    .await;
    let adapter = OpenAICodexResponses::new();
    let req = ws_request(&server.url, vec![user("hi")]);
    let first = run(&adapter, &req).await;
    assert_eq!(stop_and_error(&first), (StopReason::Stop, None));
    assert_eq!(first.text(), "via sse");
    let diagnostics = extra(&first)["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 1);
    let d = &diagnostics[0]["details"];
    assert_eq!(d["configuredTransport"], "auto");
    assert_eq!(d["fallbackTransport"], "sse");
    assert_eq!(d["phase"], "before_message_stream_start");
    assert_eq!(d["eventsEmitted"], false);
    assert!(d["requestBytes"].as_u64().unwrap() > 0);

    let second = run(&adapter, &req).await;
    assert_eq!(second.text(), "via sse");
    assert!(extra(&second).get("diagnostics").is_none());
    let methods: Vec<String> = server.requests().iter().map(|r| r.method.clone()).collect();
    assert_eq!(
        methods,
        ["GET", "POST", "POST"],
        "no second WebSocket attempt"
    );
    let stats = adapter.websocket_debug_stats("ws-sess").unwrap();
    assert_eq!(stats.websocket_failures, 1);
    assert_eq!(stats.sse_fallbacks, 2);
    assert_eq!(stats.websocket_fallback_active, Some(true));

    adapter.reset_websocket_debug_stats(Some("ws-sess"));
    assert!(adapter.websocket_debug_stats("ws-sess").is_none());
    let _ = run(&adapter, &req).await;
    assert_eq!(server.requests().last().unwrap().method, "POST");
    assert_eq!(
        server.requests().len(),
        5,
        "WebSocket is tried again after reset"
    );
}

#[tokio::test]
async fn websocket_idle_timeout_after_start_is_an_error() {
    let mock = WsMock::start(|_, _, _| {
        events(vec![
            json!({"type": "response.created", "response": {"id": "resp_i"}}),
        ])
    })
    .await;
    let mut req = ws_request(&mock.url, vec![user("hi")]);
    req.timeout = Duration::from_millis(200);
    let msg = run(&OpenAICodexResponses::new(), &req).await;
    assert_eq!(
        stop_and_error(&msg),
        (
            StopReason::Error,
            Some("WebSocket idle timeout after 200ms".into())
        )
    );
    assert!(errors::is_retryable(&msg));
}

/// Accept the WebSocket connection but never send handshake headers. Ordinary HTTP
/// requests pass through to a real SSE fixture on another local port.
struct StalledUpgrade {
    url: String,
    upgrades: Arc<AtomicUsize>,
    closed: Arc<AtomicUsize>,
    fallback: MockServer,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for StalledUpgrade {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl StalledUpgrade {
    async fn start() -> Self {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let fallback = MockServer::start(|_, _| {
            MockResponse::sse(vec![sse_text(&message_events(
                "resp_timeout",
                "m",
                "via sse",
            ))])
        })
        .await;
        let target = fallback.url.strip_prefix("http://").unwrap().to_owned();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let upgrades = Arc::new(AtomicUsize::new(0));
        let closed = Arc::new(AtomicUsize::new(0));
        let (seen, ended) = (upgrades.clone(), closed.clone());
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (mut socket, _) = accepted.unwrap();
                        let (target, seen, ended) = (target.clone(), seen.clone(), ended.clone());
                        connections.spawn(async move {
                            let mut prefix = [0; 4];
                            socket.read_exact(&mut prefix).await.unwrap();
                            if &prefix == b"GET " {
                                seen.fetch_add(1, Ordering::SeqCst);
                                let mut buffer = [0; 4096];
                                while socket.read(&mut buffer).await.is_ok_and(|n| n > 0) {}
                                ended.fetch_add(1, Ordering::SeqCst);
                            } else {
                                let mut upstream = tokio::net::TcpStream::connect(target).await.unwrap();
                                upstream.write_all(&prefix).await.unwrap();
                                let _ = tokio::io::copy_bidirectional(&mut socket, &mut upstream).await;
                            }
                        });
                    }
                    joined = connections.join_next(), if !connections.is_empty() => {
                        joined.unwrap().unwrap();
                    }
                }
            }
        });
        Self {
            url,
            upgrades,
            closed,
            fallback,
            task,
        }
    }
}

#[tokio::test]
async fn websocket_connect_timeout_bounds_stalled_handshake_and_persists_sse_fallback() {
    let server = StalledUpgrade::start().await;
    let adapter = OpenAICodexResponses::new();
    let mut req = ws_request(&server.url, vec![user("hi")]);
    req.options
        .insert("websocketConnectTimeoutMs".into(), json!(200));
    req.timeout = Duration::from_secs(5);
    let started = tokio::time::Instant::now();
    let first = tokio::time::timeout(Duration::from_secs(3), run(&adapter, &req))
        .await
        .unwrap();
    assert!(started.elapsed() >= Duration::from_millis(200));
    assert_eq!(stop_and_error(&first), (StopReason::Stop, None));
    assert_eq!(first.text(), "via sse");
    let diagnostic = &extra(&first)["diagnostics"][0];
    assert_eq!(
        diagnostic["error"]["message"],
        "WebSocket connect timeout after 200ms"
    );
    assert_eq!(
        diagnostic["details"]["phase"],
        "before_message_stream_start"
    );
    assert_eq!(diagnostic["details"]["fallbackTransport"], "sse");
    let second = run(&adapter, &req).await;
    assert_eq!(second.text(), "via sse");
    assert!(extra(&second).get("diagnostics").is_none());
    assert_eq!(server.upgrades.load(Ordering::SeqCst), 1);
    assert_eq!(server.closed.load(Ordering::SeqCst), 1);
    assert_eq!(server.fallback.requests().len(), 2);
}

#[tokio::test]
async fn websocket_disabled_connect_timeout_still_cancels_a_stalled_handshake() {
    let server = StalledUpgrade::start().await;
    let adapter = OpenAICodexResponses::new();
    let mut req = ws_request(&server.url, vec![user("hi")]);
    req.options
        .insert("websocketConnectTimeoutMs".into(), json!(0));
    // The stream-idle timeout must not become a connect timeout when connect is disabled.
    req.timeout = Duration::from_millis(100);
    let cancel = CancellationToken::new();
    let future = run_with(&adapter, &req, cancel.clone());
    tokio::pin!(future);
    assert!(
        tokio::time::timeout(Duration::from_millis(350), &mut future)
            .await
            .is_err()
    );
    assert_eq!(server.upgrades.load(Ordering::SeqCst), 1);
    assert!(server.fallback.requests().is_empty());
    cancel.cancel();
    let (message, deltas) = tokio::time::timeout(Duration::from_secs(2), future)
        .await
        .unwrap();
    assert_eq!(stop_and_error(&message), (StopReason::Aborted, None));
    assert!(deltas.is_empty());
    assert_eq!(
        adapter
            .websocket_debug_stats("ws-sess")
            .map_or(0, |stats| stats.websocket_failures),
        0
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.closed.load(Ordering::SeqCst) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn websocket_cancellation_is_aborted() {
    let mock = WsMock::start(|_, _, _| {
        events(vec![
            json!({"type": "response.created", "response": {"id": "resp_c"}}),
            json!({"type": "response.output_item.added", "output_index": 0,
                   "item": {"type": "message", "id": "m", "role": "assistant", "content": []}}),
            json!({"type": "response.output_text.delta", "output_index": 0, "delta": "partial"}),
        ])
    })
    .await;
    let req = ws_request(&mock.url, vec![user("hi")]);
    let adapter = OpenAICodexResponses::new();
    let http = crate::provider::http::client();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    let stream = adapter.stream(StreamCtx {
        req: &req,
        http: &http,
        events: Some(tx.into()),
        cancel,
    });
    let watcher = async move {
        while let Some(StreamEvent::Delta(d)) = rx.recv().await {
            if matches!(d, Delta::Text { .. }) {
                trigger.cancel();
                break;
            }
        }
    };
    let (msg, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(stream, watcher)
    })
    .await
    .expect("cancellation ends the stream");
    assert_eq!(stop_and_error(&msg), (StopReason::Aborted, None));
    assert_eq!(msg.text(), "partial");
    // An aborted exchange never parks its connection or records a transport failure.
    let stats = adapter.websocket_debug_stats("ws-sess").unwrap();
    assert_eq!(stats.websocket_failures, 0);
}

#[tokio::test]
async fn closing_sessions_drops_pooled_connections() {
    let mock = WsMock::start(|_, index, _| answer(index)).await;
    let adapter = OpenAICodexResponses::new();
    let req = ws_request(&mock.url, vec![user("hi")]);
    let _ = run(&adapter, &req).await;
    adapter.close_websocket_sessions(Some("ws-sess"));
    let _ = run(&adapter, &req).await;
    assert_eq!(mock.handshakes().len(), 2);
}

#[tokio::test]
async fn retiring_a_session_discards_pooled_connections_and_fallback_state() {
    let mock = WsMock::start(|_, index, _| answer(index)).await;
    let adapter = OpenAICodexResponses::new();
    let req = ws_request(&mock.url, vec![user("hi")]);
    let _ = run(&adapter, &req).await;
    ws::record_failure(&adapter.ws, Some("ws-sess"), "old lifetime failed");
    assert!(ws::is_fallback_active(&adapter.ws, Some("ws-sess")));
    adapter.retire_session("ws-sess");
    assert!(adapter.websocket_debug_stats("ws-sess").is_none());
    assert!(!ws::is_fallback_active(&adapter.ws, Some("ws-sess")));
    let reply = run(&adapter, &req).await;
    assert_eq!(reply.text(), "answer 1");
    assert_eq!(mock.handshakes().len(), 2);
}

#[tokio::test]
async fn a_server_closed_idle_connection_is_replaced_not_failed() {
    // The server closes every connection right after answering.
    let mock = WsMock::start(|_, index, _| {
        let mut replies = answer(index);
        replies.push(WsReply::Close(1000, "bye"));
        replies
    })
    .await;
    let adapter = OpenAICodexResponses::new();
    let req = ws_request(&mock.url, vec![user("hi")]);
    let first = run(&adapter, &req).await;
    assert_eq!(first.text(), "answer 0");
    // Let the idle task observe the close.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let second = run(&adapter, &ws_request(&mock.url, vec![user("again")])).await;
    assert_eq!(stop_and_error(&second), (StopReason::Stop, None));
    assert_eq!(second.text(), "answer 1");
    assert_eq!(mock.handshakes().len(), 2);
    assert_eq!(
        adapter
            .websocket_debug_stats("ws-sess")
            .unwrap()
            .websocket_failures,
        0
    );
}

// ---------------------------------------------------------------------------------------------
// Proxies
// ---------------------------------------------------------------------------------------------

/// A local HTTP `CONNECT` proxy: records each request head, then (when `allow`) answers 200
/// and relays bytes to the requested authority, else answers 403.
struct ConnectProxy {
    url: String,
    heads: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for ConnectProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl ConnectProxy {
    async fn start(allow: bool) -> ConnectProxy {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let heads: Arc<Mutex<Vec<String>>> = Arc::default();
        let recorded = heads.clone();
        let task = tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let recorded = recorded.clone();
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut byte = [0u8; 1];
                    while !head.ends_with(b"\r\n\r\n") {
                        if sock.read(&mut byte).await.unwrap_or(0) == 0 {
                            return;
                        }
                        head.push(byte[0]);
                    }
                    let head = String::from_utf8_lossy(&head).into_owned();
                    recorded.lock().unwrap().push(head.clone());
                    if !allow {
                        let _ = sock
                            .write_all(b"HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\n\r\n")
                            .await;
                        return;
                    }
                    let authority = head.split_whitespace().nth(1).unwrap().to_owned();
                    let Ok(mut upstream) = tokio::net::TcpStream::connect(authority).await else {
                        return;
                    };
                    let _ = sock
                        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                        .await;
                    let _ = tokio::io::copy_bidirectional(&mut sock, &mut upstream).await;
                });
            }
        });
        ConnectProxy { url, heads, task }
    }

    fn heads(&self) -> Vec<String> {
        self.heads.lock().unwrap().clone()
    }
}

#[tokio::test]
async fn websocket_tunnels_through_the_request_env_http_proxy() {
    let mock = WsMock::start(|_, index, _| answer(index)).await;
    let proxy = ConnectProxy::start(true).await;
    let mut req = ws_request(&mock.url, vec![user("hi")]);
    let with_auth = proxy.url.replace("http://", "http://us%65r:p%40ss@");
    req.env.insert("http_proxy".into(), with_auth);
    let msg = run(&OpenAICodexResponses::new(), &req).await;
    assert_eq!(stop_and_error(&msg), (StopReason::Stop, None));
    assert_eq!(msg.text(), "answer 0");
    let authority = mock.url.trim_start_matches("http://");
    let heads = proxy.heads();
    assert_eq!(heads.len(), 1, "{heads:?}");
    assert!(
        heads[0].starts_with(&format!(
            "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n"
        )),
        "{}",
        heads[0]
    );
    // `user:p@ss`, percent-decoded from the proxy URL.
    assert!(heads[0].contains("Proxy-Authorization: Basic dXNlcjpwQHNz\r\n"));
    assert_eq!(mock.handshakes().len(), 1);
    assert!(extra(&msg).get("diagnostics").is_none());
}

#[tokio::test]
async fn websocket_no_proxy_connects_directly() {
    let mock = WsMock::start(|_, index, _| answer(index)).await;
    let proxy = ConnectProxy::start(true).await;
    let mut req = ws_request(&mock.url, vec![user("hi")]);
    req.env.insert("HTTP_PROXY".into(), proxy.url.clone());
    req.env.insert("NO_PROXY".into(), "127.0.0.1".into());
    let msg = run(&OpenAICodexResponses::new(), &req).await;
    assert_eq!(msg.text(), "answer 0");
    assert!(proxy.heads().is_empty());
}

#[tokio::test]
async fn a_refused_websocket_tunnel_falls_back_to_sse() {
    // The SSE fallback goes through the shared client (no proxy in this test) to the mock.
    let server = MockServer::start(|_, _| {
        MockResponse::sse(
            message_events("resp_s", "msg_s", "over sse")
                .iter()
                .map(|e| format!("data: {e}\n\n"))
                .collect(),
        )
    })
    .await;
    let proxy = ConnectProxy::start(false).await;
    let mut req = ws_request(&server.url, vec![user("hi")]);
    req.env.insert("http_proxy".into(), proxy.url.clone());
    let msg = run(&OpenAICodexResponses::new(), &req).await;
    assert_eq!(stop_and_error(&msg), (StopReason::Stop, None));
    assert_eq!(msg.text(), "over sse");
    assert_eq!(proxy.heads().len(), 1);
    let diagnostics = extra(&msg)["diagnostics"].to_string();
    assert!(diagnostics.contains("403 Forbidden"), "{diagnostics}");
}

#[tokio::test]
async fn an_unsupported_proxy_fails_the_websocket_request_with_the_reference_message() {
    let mut req = ws_request("http://127.0.0.1:9", vec![user("hi")]);
    req.env
        .insert("HTTP_PROXY".into(), "socks5://127.0.0.1:1080".into());
    let msg = run(&OpenAICodexResponses::new(), &req).await;
    assert_eq!(
        stop_and_error(&msg),
        (
            StopReason::Error,
            Some(format!(
                "{} Got socks5:",
                crate::provider::proxy::UNSUPPORTED_PROXY_PROTOCOL_MESSAGE
            ))
        )
    );
}
