//! Capture a real worker's entire outbound byte stream and debug log across auth actions.
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

#[path = "../src/provider/mock.rs"]
#[allow(dead_code)]
mod mock;
use mock::{MockResponse, MockServer};
use pipkin_engine_protocol::{
    AuthStatus, Call, ClientFrame, PROTOCOL_VERSION, Reply, Secret, ServerFrame, frame,
};
use serde_json::json;

struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct Capture<R> {
    read: R,
    bytes: Arc<Mutex<Vec<u8>>>,
}
impl<R: Read> Read for Capture<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.read.read(buf)?;
        self.bytes.lock().unwrap().extend_from_slice(&buf[..n]);
        Ok(n)
    }
}
fn call(input: &mut impl Write, rx: &mpsc::Receiver<ServerFrame>, id: u64, call: Call) -> Reply {
    frame::write(
        input,
        &ClientFrame {
            id,
            lane: call.lane(),
            call,
        },
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let ServerFrame::Reply { id: got, result } = rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap()
            && got == id
        {
            return result.unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_secrets_never_reach_worker_frames_or_debug_logs() {
    const KEY: &str = "FIXTURE_API_KEY_DO_NOT_PUBLISH_1928";
    const ACCESS: &str = "FIXTURE_ACCESS_DO_NOT_PUBLISH_2857";
    const REFRESH: &str = "FIXTURE_REFRESH_DO_NOT_PUBLISH_3486";
    const ERROR: &str = "FIXTURE_PROVIDER_BODY_DO_NOT_PUBLISH_4975";
    let server = MockServer::start(|_, _| {
        MockResponse::json(500, &json!({"error": ERROR, "error_description": ERROR}))
    })
    .await;
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config");
    std::fs::create_dir(&config).unwrap();
    std::fs::write(
        config.join("auth.json"),
        json!({"openai-codex": {
            "type": "oauth", "access": ACCESS, "refresh": REFRESH, "expires": 4102444800000_i64
        }})
        .to_string(),
    )
    .unwrap();
    let mut worker = Worker(
        Command::new(env!("CARGO_BIN_EXE_pipkin-engine"))
            .arg("--data-dir")
            .arg(root.path().join("data"))
            .arg("--config-dir")
            .arg(&config)
            .env_clear()
            .env("PIPKIN_ENGINE_LOG", "debug")
            .env("PIPKIN_OAUTH_XAI_URL", &server.url)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let mut output = Capture {
        read: worker.0.stdout.take().unwrap(),
        bytes: bytes.clone(),
    };
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        while let Ok(frame) = frame::read::<ServerFrame>(&mut output) {
            let _ = tx.send(frame);
        }
    });
    let mut stderr = worker.0.stderr.take().unwrap();
    let logger = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let mut input = worker.0.stdin.take().unwrap();
    assert!(matches!(
        call(
            &mut input,
            &rx,
            1,
            Call::Hello {
                protocol: PROTOCOL_VERSION,
                client: "redaction-test".into()
            }
        ),
        Reply::Welcome(_)
    ));
    assert!(matches!(
        call(&mut input, &rx, 2, Call::AuthState),
        Reply::Auth(_)
    ));
    call(
        &mut input,
        &rx,
        3,
        Call::SetApiKey {
            provider: "openai".into(),
            key: Secret(KEY.into()),
        },
    );
    call(
        &mut input,
        &rx,
        4,
        Call::AuthReuse {
            provider: "openai-codex".into(),
        },
    );
    call(
        &mut input,
        &rx,
        5,
        Call::AuthStart {
            provider: "xai".into(),
        },
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut id = 6;
    loop {
        if matches!(call(&mut input, &rx, id, Call::AuthState), Reply::Auth(s) if s.status == AuthStatus::Failed)
        {
            break;
        }
        assert!(Instant::now() < deadline, "auth failure must be exercised");
        id += 1;
        std::thread::sleep(Duration::from_millis(10));
    }
    call(
        &mut input,
        &rx,
        id + 1,
        Call::AuthRemove {
            provider: "openai".into(),
        },
    );
    call(
        &mut input,
        &rx,
        id + 2,
        Call::AuthRemove {
            provider: "openai-codex".into(),
        },
    );
    drop(input);
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = worker.0.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "worker must exit");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success());
    reader.join().unwrap();
    let logs = logger.join().unwrap();
    assert!(
        !server.requests().is_empty(),
        "provider error path was reached"
    );
    assert!(String::from_utf8_lossy(&logs).contains("sign-in for xai did not complete"));
    for secret in [KEY, ACCESS, REFRESH, ERROR] {
        for captured in [&*bytes.lock().unwrap(), &logs] {
            assert!(
                !captured
                    .windows(secret.len())
                    .any(|w| w == secret.as_bytes()),
                "fixture secret appeared in outbound frames or debug logs"
            );
        }
    }
}
