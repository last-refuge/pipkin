//! Environment isolation for network proxies, in its own test binary because it sets proxy
//! variables in the real process environment (no other test in this process can race it).
//!
//! An engine started with `Env::isolated` must not route model requests through a proxy named
//! only in the process environment; an engine using the process environment must.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pipkin_engine::config::{EngineConfig, Env};
use pipkin_engine::runtime::EngineHandle;
use pipkin_engine_protocol::*;

/// A one-purpose HTTP server: records each request line and answers a non-retryable 400.
struct Recorder {
    url: String,
    lines: Arc<Mutex<Vec<String>>>,
}

impl Recorder {
    fn start() -> Recorder {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let lines: Arc<Mutex<Vec<String>>> = Arc::default();
        let recorded = lines.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let recorded = recorded.clone();
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut first = String::new();
                    if reader.read_line(&mut first).is_err() || first.is_empty() {
                        return;
                    }
                    let mut length = 0usize;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            return;
                        }
                        if line == "\r\n" {
                            break;
                        }
                        if let Some((k, v)) = line.split_once(':')
                            && k.eq_ignore_ascii_case("content-length")
                        {
                            length = v.trim().parse().unwrap_or(0);
                        }
                    }
                    let mut body = vec![0u8; length];
                    let _ = reader.read_exact(&mut body);
                    recorded.lock().unwrap().push(first.trim_end().to_owned());
                    let reply = r#"{"error":{"message":"refused by test"}}"#;
                    let _ = write!(
                        stream,
                        "HTTP/1.1 400 Bad Request\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                        reply.len()
                    );
                });
            }
        });
        Recorder { url, lines }
    }

    fn lines(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }
}

struct Client {
    _handle: EngineHandle,
    conn: pipkin_engine::host::Connection,
    next: u64,
}

impl Client {
    fn start(root: &Path, env: Env, provider_url: &str) -> Client {
        let config_dir = root.join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("models.json"),
            serde_json::json!({"providers": {"local": {
                "baseUrl": format!("{provider_url}/v1"), "api": "openai-completions",
                "apiKey": "test-key", "models": [{"id": "m"}]}}})
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            config_dir.join("settings.json"),
            r#"{"retry": {"enabled": false}}"#,
        )
        .unwrap();
        let mut config = EngineConfig::new(root.join("data"), config_dir);
        config.env = env;
        config.default_model = Some("local/m".into());
        let handle = EngineHandle::start(config).unwrap();
        let conn = handle.connect("proxy-isolation");
        let mut client = Client {
            _handle: handle,
            conn,
            next: 0,
        };
        client.call(Call::Hello {
            protocol: PROTOCOL_VERSION,
            client: "proxy-isolation".into(),
        });
        client
    }

    fn call(&mut self, call: Call) -> Reply {
        self.next += 1;
        let id = self.next;
        self.conn
            .send(ClientFrame {
                id,
                lane: call.lane(),
                call,
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.conn.recv_timeout(left).expect("reply in time") {
                ServerFrame::Reply { id: got, result } if got == id => {
                    return result.expect("call succeeds");
                }
                _ => {}
            }
        }
    }

    /// Prompt a new session and wait for the run to settle.
    fn prompt_once(&mut self, cwd: &Path) -> RunOutcome {
        let Reply::Session { summary } = self.call(Call::CreateSession {
            cwd: cwd.display().to_string(),
            request: "create".into(),
        }) else {
            panic!("no session")
        };
        self.call(Call::Prompt {
            session: summary.id.clone(),
            request: "r1".into(),
            text: "hello".into(),
            images: vec![],
            model: None,
        });
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "the run did not settle");
            if let Ok(ServerFrame::Event {
                event:
                    Event::Session {
                        change: SessionChange::RunSettled { outcome, .. },
                        ..
                    },
                ..
            }) = self.conn.recv_timeout(left)
            {
                return outcome;
            }
        }
    }
}

#[test]
fn an_isolated_engine_ignores_proxies_in_the_process_environment() {
    let provider = Recorder::start();
    let proxy = Recorder::start();
    // SAFETY: this binary has this single test; nothing else reads the environment
    // concurrently while it is changed.
    unsafe {
        for name in [
            "HTTP_PROXY",
            "http_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
        ] {
            std::env::set_var(name, &proxy.url);
        }
        std::env::remove_var("NO_PROXY");
        std::env::remove_var("no_proxy");
    }
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    // Isolated: the request goes straight to the provider.
    let mut isolated = Client::start(
        &dir.path().join("isolated"),
        Env::isolated(Default::default()),
        &provider.url,
    );
    assert!(matches!(
        isolated.prompt_once(&project),
        RunOutcome::ModelError { .. }
    ));
    assert_eq!(
        provider.lines(),
        vec!["POST /v1/chat/completions HTTP/1.1".to_owned()]
    );
    assert!(proxy.lines().is_empty(), "{:?}", proxy.lines());

    // The process environment: the same request attempts a CONNECT tunnel through the proxy.
    let mut ambient = Client::start(&dir.path().join("ambient"), Env::process(), &provider.url);
    assert!(matches!(
        ambient.prompt_once(&project),
        RunOutcome::ModelError { .. }
    ));
    assert_eq!(provider.lines().len(), 1);
    assert_eq!(
        proxy.lines(),
        vec![format!(
            "CONNECT {} HTTP/1.1",
            provider.url.strip_prefix("http://").unwrap()
        )]
    );
}
