//! A local HTTP server for adapter and OAuth tests. It records every request and answers from
//! a script, optionally streaming the body in chunks with pauses (fragmented SSE). It never
//! contacts a real provider.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Clone, Debug)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    /// Header names lowercased.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == &name.to_ascii_lowercase())
            .map(|(_, v)| v.as_str())
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
    }
}

#[derive(Clone, Debug)]
pub struct MockResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// Sent one after another with `pause` between them (chunked transfer).
    pub chunks: Vec<Vec<u8>>,
    pub pause: Duration,
    /// Keep the connection open after the last chunk (a stalled stream).
    pub hang: bool,
}

impl MockResponse {
    pub fn json(status: u16, body: &serde_json::Value) -> MockResponse {
        MockResponse {
            status,
            headers: vec![("content-type".into(), "application/json".into())],
            chunks: vec![body.to_string().into_bytes()],
            pause: Duration::ZERO,
            hang: false,
        }
    }

    /// An SSE body split into the given pieces.
    pub fn sse(pieces: Vec<String>) -> MockResponse {
        MockResponse {
            status: 200,
            headers: vec![("content-type".into(), "text/event-stream".into())],
            chunks: pieces.into_iter().map(String::into_bytes).collect(),
            pause: Duration::from_millis(1),
            hang: false,
        }
    }

    pub fn with_header(mut self, k: &str, v: &str) -> MockResponse {
        self.headers.push((k.into(), v.into()));
        self
    }
}

type Handler = dyn Fn(&Recorded, usize) -> MockResponse + Send + Sync;

pub struct MockServer {
    pub url: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockServer {
    /// Start on 127.0.0.1 with a handler called per request (with its index).
    pub async fn start(
        handler: impl Fn(&Recorded, usize) -> MockResponse + Send + Sync + 'static,
    ) -> MockServer {
        Self::start_inner(handler, false).await
    }

    pub async fn start_tls(
        handler: impl Fn(&Recorded, usize) -> MockResponse + Send + Sync + 'static,
    ) -> MockServer {
        Self::start_inner(handler, true).await
    }

    async fn start_inner(
        handler: impl Fn(&Recorded, usize) -> MockResponse + Send + Sync + 'static,
        tls: bool,
    ) -> MockServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let url = if tls {
            format!(
                "https://localhost:{}",
                listener.local_addr().unwrap().port()
            )
        } else {
            format!("http://{}", listener.local_addr().unwrap())
        };
        let acceptor = test_tls_acceptor();
        let requests: Arc<Mutex<Vec<Recorded>>> = Arc::default();
        let handler: Arc<Handler> = Arc::new(handler);
        let recorded = requests.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((tcp, _)) = listener.accept().await else {
                    return;
                };
                let handler = handler.clone();
                let recorded = recorded.clone();
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let mut sock: Box<dyn TunnelSocket> = if tls {
                        let Ok(stream) = acceptor.accept(tcp).await else {
                            return;
                        };
                        Box::new(stream)
                    } else {
                        Box::new(tcp)
                    };
                    let Some(req) = read_request(&mut sock).await else {
                        return;
                    };
                    let n = {
                        let mut all = recorded.lock().unwrap();
                        all.push(req.clone());
                        all.len() - 1
                    };
                    let resp = handler(&req, n);
                    let mut head = format!(
                        "HTTP/1.1 {} X\r\ntransfer-encoding: chunked\r\nconnection: close\r\n",
                        resp.status
                    );
                    for (k, v) in &resp.headers {
                        head.push_str(&format!("{k}: {v}\r\n"));
                    }
                    head.push_str("\r\n");
                    if sock.write_all(head.as_bytes()).await.is_err() {
                        return;
                    }
                    for chunk in &resp.chunks {
                        if chunk.is_empty() {
                            continue;
                        }
                        let framed = [
                            format!("{:x}\r\n", chunk.len()).into_bytes(),
                            chunk.clone(),
                            b"\r\n".to_vec(),
                        ]
                        .concat();
                        if sock.write_all(&framed).await.is_err() {
                            return;
                        }
                        let _ = sock.flush().await;
                        if !resp.pause.is_zero() {
                            tokio::time::sleep(resp.pause).await;
                        }
                    }
                    if resp.hang {
                        tokio::time::sleep(Duration::from_secs(3600)).await;
                    }
                    let _ = sock.write_all(b"0\r\n\r\n").await;
                });
            }
        });
        MockServer {
            url,
            requests,
            task,
        }
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }
}

pub(crate) async fn read_request(
    sock: &mut (impl tokio::io::AsyncRead + Unpin),
) -> Option<Recorded> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        let n = sock.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..end]).to_string();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?.to_owned();
    let path = first.next()?.to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let len = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buf[end..].to_vec();
    while body.len() < len {
        let n = sock.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some(Recorded {
        method,
        path,
        headers,
        body,
    })
}

/// A real local CONNECT proxy, forwarding only to the explicitly supplied test endpoint.
/// The proxy never resolves the authority requested by the client.
pub struct TunnelProxy {
    pub url: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for TunnelProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}
trait TunnelSocket: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> TunnelSocket for T {}
impl TunnelProxy {
    pub async fn start(target_url: &str, tls: bool) -> Self {
        let target = url::Url::parse(target_url).unwrap();
        let upstream = (
            target.host_str().unwrap().to_owned(),
            target.port_or_known_default().unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "{}://{}:{}",
            if tls { "https" } else { "http" },
            if tls { "localhost" } else { "127.0.0.1" },
            listener.local_addr().unwrap().port()
        );
        let acceptor = test_tls_acceptor();
        let requests: Arc<Mutex<Vec<Recorded>>> = Arc::default();
        let recorded = requests.clone();
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted=listener.accept()=> {
                        let Ok((tcp,_))=accepted else {return};let upstream=upstream.clone();let recorded=recorded.clone();let acceptor=acceptor.clone();
                        connections.spawn(async move {
                            let mut socket:Box<dyn TunnelSocket>=if tls {let Ok(s)=acceptor.accept(tcp).await else {return};Box::new(s)}else{Box::new(tcp)};
                            let Some(request)=read_request(&mut socket).await else {return};
                            recorded.lock().unwrap().push(request.clone());
                            if request.method!="CONNECT" {let _=socket.write_all(b"HTTP/1.1 405 CONNECT required\r\nContent-Length: 0\r\n\r\n").await;return;}
                            let Ok(mut destination)=tokio::net::TcpStream::connect(upstream).await else {return};
                            if socket.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await.is_err(){return;}
                            let _=tokio::io::copy_bidirectional(&mut socket,&mut destination).await;
                        });
                    },
                    _=connections.join_next(),if !connections.is_empty()=>{}
                }
            }
        });
        Self {
            url,
            requests,
            task,
        }
    }
    pub fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }
}

fn test_tls_acceptor() -> tokio_rustls::TlsAcceptor {
    use tokio_rustls::rustls::{
        ServerConfig,
        pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
    };
    tokio_rustls::TlsAcceptor::from(Arc::new(
        ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(
                    include_bytes!("../../../../fixtures/native-tls/localhost.der").to_vec(),
                )],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
                    include_bytes!("../../../../fixtures/native-tls/localhost-key.der").to_vec(),
                )),
            )
            .unwrap(),
    ))
}
