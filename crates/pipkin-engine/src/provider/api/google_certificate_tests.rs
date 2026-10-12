//! Synthetic certificate federation against real, verified TLS sockets.
use super::*;
use crate::provider::api::google_shared::tests::{model, request};
use crate::provider::mock::MockResponse;
use crate::provider::{Credential, ISOLATED_ENV, StreamCtx};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{RootCertStore, ServerConfig, pki_types::PrivateKeyDer, server::WebPkiClientVerifier},
};
use tokio_util::sync::CancellationToken;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/native-google-certificate")
}
fn reference() -> Value {
    serde_json::from_str(include_str!(
        "../../../../../fixtures/native-google-certificate/reference.json"
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
fn pem(name: &str) -> Vec<u8> {
    std::fs::read(root().join(name)).unwrap()
}
fn der(name: &str) -> CertificateDer<'static> {
    CertificateDer::from_pem_slice(&pem(name))
        .unwrap()
        .into_owned()
}
fn net() -> http::NetSettings {
    http::NetSettings {
        test_roots: vec![der("ca.pem").to_vec()],
        ..Default::default()
    }
}
fn setup(dir: &Path, case: &Value, tls: &str) -> (Value, BTreeMap<String, String>) {
    let expand = |text: &str| {
        text.replace("$TEMP", dir.to_str().unwrap())
            .replace("$FIXTURE", root().to_str().unwrap())
            .replace("$TLS", tls)
    };
    for (path, contents) in case["files"].as_object().unwrap() {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, expand(contents.as_str().unwrap())).unwrap();
    }
    let doc = serde_json::from_str(&expand(&case["document"].to_string())).unwrap();
    let env = case["environment"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), expand(v.as_str().unwrap())))
        .collect();
    (doc, env)
}
fn req(dir: &Path, doc: &Value, env: &BTreeMap<String, String>) -> ProviderRequest {
    let path = dir.join("adc.json");
    std::fs::write(&path, doc.to_string()).unwrap();
    let mut r = request(model(
        json!({"id":"gemini-2.5-pro","provider":"google-vertex","api":"google-vertex","baseUrl":"https://example.invalid"}),
    ));
    r.credential = Credential::None;
    r.options.insert(ISOLATED_ENV.into(), json!(true));
    r.env = env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    r.env.insert(
        "GOOGLE_APPLICATION_CREDENTIALS".into(),
        path.display().to_string(),
    );
    r
}
async fn token(
    req: &ProviderRequest,
    net: &http::NetSettings,
    cancel: CancellationToken,
) -> Result<String, String> {
    let http = http::client_with(net);
    crate::provider::api::google_auth::resolve(
        &StreamCtx {
            req,
            http: &http,
            net: Some(net),
            events: None,
            cancel,
        },
        None,
    )
    .await
    .map(|t| t.token)
}
#[derive(Clone)]
struct Recorded {
    method: String,
    path: String,
    authorization: Option<String>,
    body: Value,
    certificate: Option<String>,
}
impl Recorded {
    fn value(&self) -> Value {
        json!({"method":self.method,"path":self.path,"authorization":self.authorization,"body":self.body,"certificate":self.certificate})
    }
}
struct Server {
    url: String,
    records: Arc<Mutex<Vec<Recorded>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Server {
    async fn start(
        optional: bool,
        handler: impl Fn(&Recorded, usize) -> MockResponse + Send + Sync + 'static,
    ) -> Self {
        let mut roots = RootCertStore::empty();
        roots.add(der("ca.pem")).unwrap();
        let builder = WebPkiClientVerifier::builder(Arc::new(roots));
        let verifier = if optional {
            builder.allow_unauthenticated().build()
        } else {
            builder.build()
        }
        .unwrap();
        let config = ServerConfig::builder()
            .with_client_cert_verifier(verifier)
            .with_single_cert(
                vec![der("server.pem")],
                PrivateKeyDer::from_pem_slice(&pem("server-key.pem")).unwrap(),
            )
            .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "https://127.0.0.1:{}",
            listener.local_addr().unwrap().port()
        );
        let records = Arc::new(Mutex::new(Vec::new()));
        let saved = records.clone();
        let handler = Arc::new(handler);
        let task = tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted=listener.accept()=>{
                        let Ok((tcp,_))=accepted else{break};let acceptor=acceptor.clone();let saved=saved.clone();let handler=handler.clone();
                        tasks.spawn(async move {
                            let Ok(mut socket)=acceptor.accept(tcp).await else{return};
                            let certificate=socket.get_ref().1.peer_certificates().and_then(|c|c.first()).map(|c|{
                                crate::provider::api::aws_auth::sha256_hex(c.as_ref()).as_bytes().chunks(2).map(|c|std::str::from_utf8(c).unwrap().to_uppercase()).collect::<Vec<_>>().join(":")
                            });
                            let mut head=Vec::new();
                            while !head.ends_with(b"\r\n\r\n") {
                                let mut b=[0];if head.len()>16384||socket.read(&mut b).await.unwrap_or(0)==0{return}head.push(b[0]);
                            }
                            let head=String::from_utf8(head).unwrap();let mut words=head.lines().next().unwrap().split_whitespace();
                            let method=words.next().unwrap().to_owned();let path=words.next().unwrap().to_owned();
                            let headers: BTreeMap<_,_>=head.lines().skip(1).filter_map(|l|l.split_once(':')).map(|(k,v)|(k.to_ascii_lowercase(),v.trim().to_owned())).collect();
                            let length=headers.get("content-length").and_then(|n|n.parse().ok()).unwrap_or(0usize);
                            if length>LIMIT*2{return}let mut body=vec![0;length];if socket.read_exact(&mut body).await.is_err(){return}
                            let body=if headers.get("content-type").is_some_and(|v|v.contains("json")){serde_json::from_slice(&body).unwrap()}else{json!(url::form_urlencoded::parse(&body).map(|(k,v)|(k.into_owned(),json!(v))).collect::<serde_json::Map<_,_>>())};
                            let record=Recorded{method,path,authorization:headers.get("authorization").cloned(),body,certificate};
                            let n={let mut all=saved.lock().unwrap();let n=all.len();all.push(record.clone());n};
                            let response=handler(&record,n);
                            let size:usize=response.chunks.iter().map(Vec::len).sum();
                            let mut response_head=format!("HTTP/1.1 {} X\r\nConnection: close\r\nContent-Length: {}\r\n",response.status,if response.hang {size+1}else{size});
                            for (k,v) in response.headers {response_head.push_str(&format!("{k}: {v}\r\n"));}response_head.push_str("\r\n");
                            if socket.write_all(response_head.as_bytes()).await.is_err(){return}
                            for chunk in response.chunks {tokio::time::sleep(response.pause).await;if socket.write_all(&chunk).await.is_err(){return}}
                            if response.hang {tokio::time::sleep(Duration::from_secs(60)).await;}
                        });
                    },
                    _=tasks.join_next(),if !tasks.is_empty()=>{}
                }
            }
        });
        Self { url, records, task }
    }
    fn records(&self) -> Vec<Recorded> {
        self.records.lock().unwrap().clone()
    }
}
fn grant(r: &Recorded, _: usize) -> MockResponse {
    if r.path == "/token" {
        MockResponse::json(
            200,
            &json!({"access_token":"certificate-token","expires_in":3600}),
        )
    } else {
        MockResponse::json(
            200,
            &json!({"accessToken":"impersonated-token","expireTime":"2099-01-01T00:00:00Z"}),
        )
    }
}

#[tokio::test]
async fn certificate_sources_and_mutual_tls_match_nineteen_executed_sdk_cases() {
    for case in reference()["cases"].as_array().unwrap() {
        let server = Server::start(false, grant).await;
        let dir = tempfile::tempdir().unwrap();
        let (doc, env) = setup(dir.path(), case, &server.url);
        let r = req(dir.path(), &doc, &env);
        let result = token(&r, &net(), CancellationToken::new()).await;
        if let Some(expected) = case["outcome"]["token"].as_str() {
            assert_eq!(result.as_deref(), Ok(expected), "{}", case["name"]);
        } else {
            let error = result.unwrap_err();
            assert!(
                !error.contains("secret") && !error.contains("PRIVATE KEY"),
                "{error}"
            );
        }
        assert_eq!(
            json!(
                server
                    .records()
                    .iter()
                    .map(Recorded::value)
                    .collect::<Vec<_>>()
            ),
            case["requests"],
            "{}",
            case["name"]
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("adc.json")).unwrap(),
            doc.to_string()
        );
    }
}

#[tokio::test]
async fn certificate_refresh_reloads_identity_and_rejects_a_mismatched_key() {
    let server = Server::start(false, |_, _| {
        MockResponse::json(
            200,
            &json!({"access_token":"certificate-token","expires_in":1}),
        )
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let (doc, env) = setup(dir.path(), &case("leaf"), &server.url);
    let config = dir.path().join("config.json");
    let r = req(dir.path(), &doc, &env);
    token(&r, &net(), CancellationToken::new()).await.unwrap();
    let first = std::fs::read_to_string(&config).unwrap();
    std::fs::write(&config, first.replace("client.pem", "rotated.pem")).unwrap();
    assert!(token(&r, &net(), CancellationToken::new()).await.is_err());
    assert_eq!(server.records().len(), 1);
    std::fs::write(
        &config,
        first
            .replace("client.pem", "rotated.pem")
            .replace("client-key.pem", "rotated-key.pem"),
    )
    .unwrap();
    token(&r, &net(), CancellationToken::new()).await.unwrap();
    let records = server.records();
    assert_eq!(records.len(), 2);
    assert_ne!(records[0].certificate, records[1].certificate);
    assert_ne!(
        records[0].body["subject_token"],
        records[1].body["subject_token"]
    );
}

#[tokio::test]
async fn canceled_certificate_waiter_preserves_cached_exchange_and_network_identity() {
    let server = Server::start(false, |r, n| {
        let mut response = grant(r, n);
        response.pause = Duration::from_millis(150);
        response
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let (doc, env) = setup(dir.path(), &case("leaf"), &server.url);
    let r = req(dir.path(), &doc, &env);
    let cancel = CancellationToken::new();
    let c = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(40)).await;
        c.cancel();
    });
    assert_eq!(
        token(&r, &net(), cancel).await.unwrap_err(),
        "Request aborted"
    );
    token(&r, &net(), CancellationToken::new()).await.unwrap();
    assert_eq!(server.records().len(), 1);
    let config = dir.path().join("config.json");
    std::fs::remove_file(&config).unwrap();
    token(&r, &net(), CancellationToken::new()).await.unwrap();
    assert_eq!(server.records().len(), 1);
    let mut changed = net();
    changed.idle = Some(Duration::from_secs(1));
    assert!(token(&r, &changed, CancellationToken::new()).await.is_err());
}

#[tokio::test]
async fn certificate_files_are_bounded_and_untrusted_servers_are_rejected() {
    let server = Server::start(false, grant).await;
    for file in ["config.json", "chain.pem", "cert.pem", "key.pem"] {
        let dir = tempfile::tempdir().unwrap();
        let (mut doc, env) = setup(dir.path(), &case("leaf"), &server.url);
        let content = "secret".repeat(LIMIT / 6 + 1);
        std::fs::write(dir.path().join(file), content).unwrap();
        if file == "chain.pem" {
            doc["credential_source"]["certificate"]["trust_chain_path"] =
                json!(dir.path().join(file));
        }
        if file == "cert.pem" || file == "key.pem" {
            let path = dir.path().join("config.json");
            let mut config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            config["cert_configs"]["workload"][if file == "cert.pem" {
                "cert_path"
            } else {
                "key_path"
            }] = json!(dir.path().join(file));
            std::fs::write(path, config.to_string()).unwrap();
        }
        let r = req(dir.path(), &doc, &env);
        let error = token(&r, &net(), CancellationToken::new())
            .await
            .unwrap_err();
        assert!(error.contains("too large"), "{file}: {error}");
        assert!(!error.contains("secret"));
    }
    let dir = tempfile::tempdir().unwrap();
    let (doc, env) = setup(dir.path(), &case("leaf"), &server.url);
    let r = req(dir.path(), &doc, &env);
    assert!(
        token(&r, &http::NetSettings::default(), CancellationToken::new())
            .await
            .is_err()
    );
    assert!(server.records().is_empty());
}

struct Proxy {
    url: String,
    heads: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Proxy {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let heads = Arc::new(Mutex::new(Vec::new()));
        let saved = heads.clone();
        let task = tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted=listener.accept()=>{let Ok((mut socket,_))=accepted else{break};let saved=saved.clone();tasks.spawn(async move{
                        let mut head=Vec::new();while !head.ends_with(b"\r\n\r\n"){let mut b=[0];if head.len()>8192||socket.read(&mut b).await.unwrap_or(0)==0{return}head.push(b[0]);}
                        let head=String::from_utf8(head).unwrap();saved.lock().unwrap().push(head.clone());let target=head.split_whitespace().nth(1).unwrap();
                        let mut upstream=tokio::net::TcpStream::connect(target).await.unwrap();socket.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await.unwrap();let _=tokio::io::copy_bidirectional(&mut socket,&mut upstream).await;
                    });}, _=tasks.join_next(),if !tasks.is_empty()=>{}
                }
            }
        });
        Self { url, heads, task }
    }
}

#[tokio::test]
async fn certificate_transport_preserves_proxy_and_idle_timeout_then_recovers() {
    let server = Server::start(false, |r, n| {
        let mut response = grant(r, n);
        if n == 0 {
            response.hang = true;
        }
        response
    })
    .await;
    let proxy = Proxy::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (doc, env) = setup(dir.path(), &case("leaf"), &server.url);
    let r = req(dir.path(), &doc, &env);
    let mut config_env = crate::config::Env::isolated(Default::default());
    config_env
        .vars
        .insert("HTTPS_PROXY".into(), proxy.url.clone());
    let mut net = net();
    net.proxy = crate::provider::proxy::ProxyConfig::from_env(&config_env, None);
    net.idle = Some(Duration::from_millis(100));
    let started = std::time::Instant::now();
    assert!(
        token(&r, &net, CancellationToken::new())
            .await
            .unwrap_err()
            .contains("timed out")
    );
    assert!(started.elapsed() < Duration::from_secs(3));
    token(&r, &net, CancellationToken::new()).await.unwrap();
    assert_eq!(server.records().len(), 2);
    assert_eq!(proxy.heads.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn vertex_model_streams_receive_bearer_headers_without_the_auth_certificate() {
    let server=Server::start(true,|r,n|if r.path=="/token"{grant(r,n)}else{MockResponse::sse(vec!["data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hi\"}],\"role\":\"model\"},\"finishReason\":\"STOP\"}]}\n\n".into()])}).await;
    let dir = tempfile::tempdir().unwrap();
    let (doc, env) = setup(dir.path(), &case("leaf"), &server.url);
    let mut r = req(dir.path(), &doc, &env);
    r.base_url = Some(format!("{}/model", server.url));
    r.options.insert("project".into(), json!("project"));
    r.options.insert("location".into(), json!("global"));
    let net = net();
    let http = http::client_with(&net);
    for _ in 0..2 {
        let message = crate::provider::api::google_vertex::adapter()
            .stream(StreamCtx {
                req: &r,
                http: &http,
                net: Some(&net),
                events: None,
                cancel: CancellationToken::new(),
            })
            .await;
        assert!(
            matches!(
                &message,
                pipkin_engine_protocol::Message::Assistant {
                    stop_reason: pipkin_engine_protocol::StopReason::Stop,
                    ..
                }
            ),
            "{message:?}"
        );
        assert_eq!(message.text(), "hi");
    }
    let records = server.records();
    assert_eq!(records.len(), 3);
    assert!(records[0].certificate.is_some());
    for record in &records[1..] {
        assert_eq!(
            record.authorization.as_deref(),
            Some("Bearer certificate-token")
        );
        assert!(record.certificate.is_none());
    }
}
