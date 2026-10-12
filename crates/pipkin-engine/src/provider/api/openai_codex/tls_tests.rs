//! Local verified TLS fixtures: both hops are independent TLS sessions, never insecure mode.
use super::super::OpenAICodexResponses;
use super::*;
use crate::provider::api::constrained_tests::{grammar_events, grammar_tool};
use crate::provider::api::openai_completions::tests::{request, tool_result};
use crate::provider::{ApiAdapter, Credential, StreamCtx};
use pipkin_engine_protocol::{Block, Message, StopReason};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        ServerConfig,
        pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
    },
};

const CA: &[u8] = include_bytes!("../../../../../../fixtures/native-tls/ca.der");
const CERT: &[u8] = include_bytes!("../../../../../../fixtures/native-tls/localhost.der");
const KEY: &[u8] = include_bytes!("../../../../../../fixtures/native-tls/localhost-key.der");

fn trusted() -> Arc<ClientConfig> {
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(CA.to_vec())).unwrap();
    Arc::new(
        ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}
fn acceptor() -> TlsAcceptor {
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(KEY.to_vec()));
    TlsAcceptor::from(Arc::new(
        ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![CertificateDer::from(CERT.to_vec())], key)
            .unwrap(),
    ))
}

struct Fixture {
    url: String,
    heads: Arc<Mutex<Vec<String>>>,
    frames: Arc<Mutex<Vec<Value>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    // Tungstenite fixes the callback error type to an HTTP response.
    #[allow(clippy::result_large_err)]
    async fn start(tls: bool, proxy: Option<String>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!(
            "{}://{}:{port}",
            if tls { "https" } else { "http" },
            if tls { "localhost" } else { "127.0.0.1" }
        );
        let heads = Arc::new(Mutex::new(Vec::new()));
        let frames = Arc::new(Mutex::new(Vec::new()));
        let recorded = heads.clone();
        let output = frames.clone();
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted=listener.accept() => {
                        let Ok((tcp,_))=accepted else { break };
                        let proxy=proxy.clone(); let heads=recorded.clone(); let frames=output.clone();
                        connections.spawn(async move {
                            let mut socket:Transport = if tls {
                                let Ok(stream)=acceptor().accept(tcp).await else {return};
                                Box::new(stream)
                            } else {Box::new(tcp)};
                            if let Some(response)=proxy {
                                let mut head=Vec::new();
                                while !head.ends_with(b"\r\n\r\n") {
                                    let mut byte=[0];
                                    if head.len()>MAX_PROXY_HEAD || socket.read(&mut byte).await.unwrap_or(0)==0 {return}
                                    head.push(byte[0]);
                                }
                                let head=String::from_utf8(head).unwrap();
                                heads.lock().unwrap().push(head.clone());
                                if socket.write_all(response.as_bytes()).await.is_err() {return}
                                if !response.starts_with("HTTP/1.1 200 ") {return}
                                let authority=head.split_whitespace().nth(1).unwrap();
                                let Ok(mut upstream)=tokio::net::TcpStream::connect(authority).await else {return};
                                let _=tokio::io::copy_bidirectional(&mut socket,&mut upstream).await;
                            } else {
                                let Ok(mut ws)=tokio_tungstenite::accept_hdr_async(socket,move |req:&tokio_tungstenite::tungstenite::handshake::server::Request,response| {
                                    heads.lock().unwrap().push(format!("{:?}",req.headers()));
                                    Ok(response)
                                }).await else {return};
                                while let Some(Ok(WsMessage::Text(frame)))=ws.next().await {
                                    let index={let mut frames=frames.lock().unwrap();let i=frames.len(); frames.push(serde_json::from_str::<Value>(&frame).unwrap());i};
                                    let mut events=if index==0 {grammar_events("verified TLS patch")} else {vec![
                                        json!({"type":"response.output_item.done","output_index":0,"item":{"type":"message","role":"assistant","id":"msg_done","content":[{"type":"output_text","text":"done"}]}}),
                                        json!({"type":"response.completed","response":{"status":"completed","output":[]}})
                                    ]};
                                    events.last_mut().unwrap()["response"]["id"]=json!(format!("resp_{index}"));
                                    for event in events {if ws.send(WsMessage::Text(event.to_string().into())).await.is_err(){return}}
                                }
                            }
                        });
                    }
                    _=connections.join_next(), if !connections.is_empty() => {}
                }
            }
        });
        Self {
            url,
            heads,
            frames,
            task,
        }
    }
    async fn proxy(tls: bool) -> Self {
        Self::start(
            tls,
            Some("HTTP/1.1 200 Connection Established\r\n\r\n".into()),
        )
        .await
    }
    fn websocket_url(&self) -> String {
        self.url.replace("http", "ws") + "/responses"
    }
}

fn req_for(target: &Fixture, proxy: &Fixture) -> crate::provider::ProviderRequest {
    let mut req = request(
        json!({"id":"test","provider":"openai-codex","api":"openai-codex-responses","baseUrl":target.url,"compat":{"supportsOpenAIGrammarTools":true}}),
    );
    req.credential = Credential::OAuth {
        access: "fixture-token".into(),
        metadata: serde_json::from_value(json!({"accountId":"fixture-account"})).unwrap(),
    };
    req.messages = vec![Message::user_text("patch", 1)];
    req.tools = vec![grammar_tool()];
    req.session_id = "tls-session".into();
    req.options
        .insert("transport".into(), json!("websocket-cached"));
    req.options
        .insert("websocketConnectTimeoutMs".into(), json!(1000));
    req.timeout = Duration::from_secs(2);
    let with_auth = proxy.url.replacen("://", "://us%65r:p%40ss@", 1);
    req.env.insert("HTTP_PROXY".into(), with_auth.clone());
    req.env.insert("HTTPS_PROXY".into(), with_auth);
    req
}
async fn run_adapter(
    adapter: &OpenAICodexResponses,
    req: &crate::provider::ProviderRequest,
) -> Message {
    adapter
        .stream(StreamCtx {
            net: None,
            req,
            http: &crate::provider::http::client(),
            events: None,
            cancel: CancellationToken::new(),
        })
        .await
}

#[tokio::test]
async fn http_and_https_proxies_carry_ws_and_wss_with_grammar_continuation() {
    for proxy_tls in [false, true] {
        for target_tls in [false, true] {
            let target = Fixture::start(target_tls, None).await;
            let proxy = Fixture::proxy(proxy_tls).await;
            let adapter = OpenAICodexResponses::new();
            lock(&adapter.ws).tls = Some(trusted());
            let mut req = req_for(&target, &proxy);
            let first = run_adapter(&adapter, &req).await;
            assert!(
                matches!(
                    &first,
                    Message::Assistant {
                        stop_reason: StopReason::ToolUse,
                        ..
                    }
                ),
                "proxy TLS={proxy_tls}, target TLS={target_tls}: {first:?}"
            );
            assert!(
                matches!(&first.content()[0],Block::ToolCall{arguments,..} if arguments["patch"]=="verified TLS patch")
            );
            req.messages.extend([
                first,
                tool_result("call_one|ctc_one", "patch", vec![Block::text("applied")]),
                Message::user_text("continue", 2),
            ]);
            assert_eq!(run_adapter(&adapter, &req).await.text(), "done");
            let frames = target.frames.lock().unwrap().clone();
            assert_eq!(frames[0]["tools"][0]["type"], "custom");
            assert_eq!(
                frames[1]["previous_response_id"], "resp_0",
                "grammar continuation must reuse the previous response"
            );
            assert_eq!(frames[1]["input"].as_array().unwrap().len(), 2);
            assert_eq!(frames[1]["input"][0]["type"], "custom_tool_call_output");
            let heads = proxy.heads.lock().unwrap().clone();
            assert_eq!(heads.len(), 1, "pooled connection must reuse the tunnel");
            assert!(heads[0].contains("Proxy-Authorization: Basic dXNlcjpwQHNz\r\n"));
            let handshake = target.heads.lock().unwrap()[0].to_lowercase();
            assert!(handshake.contains("fixture-token"));
            assert!(!handshake.contains("proxy-authorization"));
            assert!(!heads[0].contains("fixture-token"));
            adapter.close_websocket_sessions(Some("tls-session"));
            assert_eq!(run_adapter(&adapter, &req).await.text(), "done");
            assert_eq!(proxy.heads.lock().unwrap().len(), 2);
            assert!(
                target.frames.lock().unwrap()[2]
                    .get("previous_response_id")
                    .is_none()
            );
            adapter.close_websocket_sessions(None);
        }
    }
}

#[tokio::test]
async fn tls_proxy_and_target_certificates_are_verified_independently() {
    let target = Fixture::start(true, None).await;
    let proxy = Fixture::proxy(true).await;
    let cancel = CancellationToken::new();
    let timeout = Some(Duration::from_secs(1));
    // Untrusted proxy: no CONNECT (nor its credentials) can be sent before verification.
    let url = url::Url::parse(&proxy.url).unwrap();
    let result = connect(
        &target.websocket_url(),
        &[],
        &cancel,
        timeout,
        Some(&url),
        None,
    )
    .await;
    assert!(result.err().unwrap().message.contains("Proxy TLS"));
    assert!(proxy.heads.lock().unwrap().is_empty());
    // Trusted CA but wrong proxy hostname.
    let wrong_proxy = url::Url::parse(&proxy.url.replace("localhost", "127.0.0.1")).unwrap();
    let result = connect(
        &target.websocket_url(),
        &[],
        &cancel,
        timeout,
        Some(&wrong_proxy),
        Some(trusted()),
    )
    .await;
    assert!(result.err().unwrap().message.contains("Proxy TLS"));
    assert!(proxy.heads.lock().unwrap().is_empty());
    // Correct proxy; wrong target name must still fail inside the tunnel.
    let wrong_target = target.websocket_url().replace("localhost", "127.0.0.1");
    let result = connect(
        &wrong_target,
        &[],
        &cancel,
        timeout,
        Some(&url),
        Some(trusted()),
    )
    .await;
    assert!(result.is_err());
    assert_eq!(proxy.heads.lock().unwrap().len(), 1);
    assert!(target.heads.lock().unwrap().is_empty());
    // A plain proxy cannot bypass the target's CA verification either.
    let plain_proxy = Fixture::proxy(false).await;
    let url = url::Url::parse(&plain_proxy.url).unwrap();
    let result = connect(
        &target.websocket_url(),
        &[],
        &cancel,
        timeout,
        Some(&url),
        None,
    )
    .await;
    assert!(result.is_err());
    assert_eq!(plain_proxy.heads.lock().unwrap().len(), 1);
    assert!(target.heads.lock().unwrap().is_empty());
}

#[tokio::test]
async fn no_proxy_bypasses_the_tls_proxy_without_disabling_target_tls() {
    let target = Fixture::start(true, None).await;
    let proxy = Fixture::proxy(true).await;
    let adapter = OpenAICodexResponses::new();
    lock(&adapter.ws).tls = Some(trusted());
    let mut req = req_for(&target, &proxy);
    req.env.insert("NO_PROXY".into(), "localhost".into());
    let result = run_adapter(&adapter, &req).await;
    assert!(matches!(
        result,
        Message::Assistant {
            stop_reason: StopReason::ToolUse,
            ..
        }
    ));
    assert!(proxy.heads.lock().unwrap().is_empty());
    assert_eq!(target.heads.lock().unwrap().len(), 1);
    adapter.close_websocket_sessions(None);
}

#[tokio::test]
async fn proxy_tls_handshake_obeys_connect_timeout_and_cancellation() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = url::Url::parse(&format!(
        "https://localhost:{}",
        listener.local_addr().unwrap().port()
    ))
    .unwrap();
    let hold = tokio::spawn(async move {
        let mut connections = Vec::new();
        while let Ok((tcp, _)) = listener.accept().await {
            connections.push(tcp);
        }
    });
    let result = connect(
        "ws://localhost:9/responses",
        &[],
        &CancellationToken::new(),
        Some(Duration::from_millis(50)),
        Some(&proxy),
        Some(trusted()),
    )
    .await;
    assert!(
        result
            .err()
            .unwrap()
            .message
            .contains("WebSocket connect timeout after 50ms")
    );
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    let stop = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        trigger.cancel();
    });
    let result = connect(
        "ws://localhost:9/responses",
        &[],
        &cancel,
        Some(Duration::from_secs(1)),
        Some(&proxy),
        Some(trusted()),
    )
    .await;
    assert_eq!(result.err().unwrap().kind, FailureKind::Aborted);
    stop.await.unwrap();
    hold.abort();
}

#[tokio::test]
async fn tls_connect_rejects_refused_malformed_and_oversized_proxy_responses() {
    let target = Fixture::start(false, None).await;
    for response in [
        "HTTP/1.1 407 Proxy Authentication Required\r\n\r\n".into(),
        "NOTHTTP 200 Wrong\r\n\r\n".into(),
        "HTTP/1.1 2bad Nope\r\n\r\n".into(),
        format!(
            "HTTP/1.1 200 OK\r\nx-long: {}\r\n\r\n",
            "x".repeat(MAX_PROXY_HEAD)
        ),
    ] {
        let proxy = Fixture::start(true, Some(response)).await;
        let url = url::Url::parse(&proxy.url).unwrap();
        let failure = connect(
            &target.websocket_url(),
            &[],
            &CancellationToken::new(),
            Some(Duration::from_secs(1)),
            Some(&url),
            Some(trusted()),
        )
        .await
        .err()
        .unwrap();
        assert!(failure.message.contains("Proxy CONNECT"));
        assert!(target.heads.lock().unwrap().is_empty());
    }
}
