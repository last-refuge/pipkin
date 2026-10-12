//! Crash-point qualification with a real process kill (plan §9): the headless `pipkin-engine`
//! worker over its stdio frames, a `models.json` custom `openai-completions` provider pointed at
//! a scripted HTTP stub in this process, SIGKILL at each durable boundary, a restart on the same
//! data, and checks of lookups, entry counts, the next provider request, and leftover processes.
//!
//! Two boundaries cannot be separated from the outside (the engine commits a Stop and acts on it
//! in one step), so the worker kills itself there when `PIPKIN_ENGINE_CRASH_POINT` names the
//! point; that is still a real SIGKILL of the real process. No network, credentials or GUI.

#![cfg(target_os = "linux")]

use std::io::{BufReader, BufWriter, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use pipkin_engine_protocol::frame;
use pipkin_engine_protocol::*;
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(30);

// ------------------------------------------------------------------------------- the stub

/// What the stub answers to one request.
#[derive(Clone, Default)]
struct Plan {
    pieces: Vec<String>,
    /// Keep the response open after the pieces until the client goes away.
    hang: bool,
}

type Script = dyn Fn(usize, &Value) -> Plan + Send + Sync;

struct Stub {
    port: u16,
    script: Arc<Mutex<Arc<Script>>>,
    bodies: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the accept loop so it sees the flag.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn sse(delta: Value, finish: Option<&str>) -> String {
    let mut chunk = json!({
        "id": "stub-1",
        "object": "chat.completion.chunk",
        "created": 0,
        "model": "scripted",
        "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }],
    });
    if finish.is_some() {
        chunk["usage"] = json!({ "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 });
    }
    format!("data: {chunk}\n\n")
}

fn text_plan(text: &str) -> Plan {
    Plan {
        pieces: vec![
            sse(json!({"role": "assistant", "content": ""}), None),
            sse(json!({ "content": text }), None),
            sse(json!({}), Some("stop")),
            "data: [DONE]\n\n".into(),
        ],
        hang: false,
    }
}

/// Some content, then nothing more until the client goes away.
fn partial_then_hang(parts: &[&str]) -> Plan {
    let mut pieces = vec![sse(json!({"role": "assistant", "content": ""}), None)];
    pieces.extend(parts.iter().map(|p| sse(json!({ "content": p }), None)));
    Plan { pieces, hang: true }
}

fn hang_plan() -> Plan {
    partial_then_hang(&[])
}

fn tool_plan(n: usize, name: &str, args: Value) -> Plan {
    Plan {
        pieces: vec![
            sse(json!({"role": "assistant", "content": ""}), None),
            sse(
                json!({"tool_calls": [{"index": 0, "id": format!("call_{n}"), "type": "function",
                    "function": {"name": name, "arguments": args.to_string()}}]}),
                None,
            ),
            sse(json!({}), Some("tool_calls")),
            "data: [DONE]\n\n".into(),
        ],
        hang: false,
    }
}

impl Stub {
    fn start(script: impl Fn(usize, &Value) -> Plan + Send + Sync + 'static) -> Stub {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let script: Arc<Mutex<Arc<Script>>> = Arc::new(Mutex::new(Arc::new(script)));
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (sc, bo, st) = (script.clone(), bodies.clone(), stop.clone());
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                if st.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(conn) = conn else { continue };
                let (sc, bo, st) = (sc.clone(), bo.clone(), st.clone());
                std::thread::spawn(move || {
                    let _ = serve(conn, &sc, &bo, &st);
                });
            }
        });
        Stub {
            port,
            script,
            bodies,
            stop,
        }
    }

    fn set_script(&self, script: impl Fn(usize, &Value) -> Plan + Send + Sync + 'static) {
        *self.script.lock().unwrap() = Arc::new(script);
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }

    fn bodies(&self) -> Vec<Value> {
        self.bodies.lock().unwrap().clone()
    }
}

fn read_request(s: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 65536];
    let head_end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
        let n = s.read(&mut tmp)?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        buf.extend_from_slice(&tmp[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_lowercase();
    let mut body = buf[head_end..].to_vec();
    let length = head.lines().find_map(|l| {
        l.strip_prefix("content-length:")
            .and_then(|v| v.trim().parse::<usize>().ok())
    });
    if let Some(len) = length {
        while body.len() < len {
            let n = s.read(&mut tmp)?;
            if n == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            body.extend_from_slice(&tmp[..n]);
        }
        return Ok(body);
    }
    // Chunked request body.
    let mut out = Vec::new();
    loop {
        let Some(p) = body.windows(2).position(|w| w == b"\r\n") else {
            let n = s.read(&mut tmp)?;
            if n == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            body.extend_from_slice(&tmp[..n]);
            continue;
        };
        let size = usize::from_str_radix(
            String::from_utf8_lossy(&body[..p])
                .split(';')
                .next()
                .unwrap()
                .trim(),
            16,
        )
        .unwrap_or(0);
        while body.len() < p + 2 + size + 2 {
            let n = s.read(&mut tmp)?;
            if n == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            body.extend_from_slice(&tmp[..n]);
        }
        if size == 0 {
            return Ok(out);
        }
        out.extend_from_slice(&body[p + 2..p + 2 + size]);
        body.drain(..p + 2 + size + 2);
    }
}

fn chunk(s: &mut TcpStream, data: &[u8]) -> std::io::Result<()> {
    let mut out = format!("{:x}\r\n", data.len()).into_bytes();
    out.extend_from_slice(data);
    out.extend_from_slice(b"\r\n");
    s.write_all(&out)
}

fn serve(
    mut s: TcpStream,
    script: &Mutex<Arc<Script>>,
    bodies: &Mutex<Vec<Value>>,
    stop: &AtomicBool,
) -> std::io::Result<()> {
    let body = read_request(&mut s)?;
    let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let n = {
        let mut b = bodies.lock().unwrap();
        b.push(parsed.clone());
        b.len() - 1
    };
    let script = script.lock().unwrap().clone();
    let plan = script(n, &parsed);
    s.write_all(
        b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n",
    )?;
    for p in &plan.pieces {
        chunk(&mut s, p.as_bytes())?;
    }
    if plan.hang {
        while !stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(100));
            chunk(&mut s, b": keep-alive\n\n")?;
        }
        return Ok(());
    }
    s.write_all(b"0\r\n\r\n")
}

/// The text of a request message (string or content parts).
fn content_text(m: &Value) -> String {
    match &m["content"] {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

fn messages_with_role(body: &Value, role: &str) -> Vec<String> {
    body["messages"]
        .as_array()
        .map(|ms| {
            ms.iter()
                .filter(|m| m["role"] == role)
                .map(content_text)
                .collect()
        })
        .unwrap_or_default()
}

fn offers_tool(body: &Value, name: &str) -> bool {
    body["tools"]
        .as_array()
        .is_some_and(|ts| ts.iter().any(|t| t["function"]["name"] == name))
}

// ----------------------------------------------------------------------------- the profile

struct Profile {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    data: PathBuf,
    config: PathBuf,
    project: PathBuf,
    home: PathBuf,
}

impl Profile {
    fn new(base_url: &str) -> Profile {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let p = Profile {
            data: root.join("data"),
            config: root.join("config"),
            project: root.join("project"),
            home: root.join("home"),
            root,
            _tmp: tmp,
        };
        for d in [&p.config, &p.project, &p.home] {
            std::fs::create_dir_all(d).unwrap();
        }
        p.point_at(base_url);
        p
    }

    /// Point the custom provider at `base_url` (models.json is re-read at start).
    fn point_at(&self, base_url: &str) {
        std::fs::write(
            self.config.join("models.json"),
            json!({
                "providers": { "stub": {
                    "baseUrl": base_url,
                    "api": "openai-completions",
                    "apiKey": "stub",
                    "models": [{ "id": "scripted", "name": "Scripted stub", "input": ["text"] }]
                }}
            })
            .to_string(),
        )
        .unwrap();
    }
}

// ------------------------------------------------------------------------------ the worker

struct Worker {
    child: Child,
    input: Option<BufWriter<ChildStdin>>,
    rx: mpsc::Receiver<ServerFrame>,
    next: u64,
    events: Vec<Event>,
}

impl Worker {
    fn start(p: &Profile, crash_point: Option<&str>) -> Worker {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_pipkin-engine"));
        cmd.arg("--data-dir")
            .arg(&p.data)
            .arg("--config-dir")
            .arg(&p.config)
            .args(["--default-model", "stub/scripted"])
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", &p.home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if let Some(point) = crash_point {
            cmd.env("PIPKIN_ENGINE_CRASH_POINT", point);
        }
        let mut child = cmd.spawn().expect("spawn pipkin-engine");
        let input = BufWriter::new(child.stdin.take().unwrap());
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(frame) = frame::read::<ServerFrame>(&mut output) {
                if tx.send(frame).is_err() {
                    break;
                }
            }
        });
        let mut w = Worker {
            child,
            input: Some(input),
            rx,
            next: 0,
            events: vec![],
        };
        match w.call(Call::Hello {
            protocol: PROTOCOL_VERSION,
            client: "crash".into(),
        }) {
            Some(Ok(Reply::Welcome(_))) => {}
            other => panic!("{other:?}"),
        }
        w
    }

    /// `None` when the engine died before replying.
    fn call(&mut self, call: Call) -> Option<Result<Reply, ProtoError>> {
        self.next += 1;
        let id = self.next;
        let input = self.input.as_mut().unwrap();
        if frame::write(
            input,
            &ClientFrame {
                id,
                lane: call.lane(),
                call,
            },
        )
        .is_err()
        {
            return None;
        }
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left) {
                Ok(ServerFrame::Reply { id: got, result }) if got == id => return Some(result),
                Ok(ServerFrame::Reply { .. }) => {}
                Ok(ServerFrame::Event { event, .. }) => self.events.push(event),
                Err(mpsc::RecvTimeoutError::Disconnected) => return None,
                Err(mpsc::RecvTimeoutError::Timeout) => panic!("no reply in time"),
            }
        }
    }

    fn ok(&mut self, call: Call) -> Reply {
        match self.call(call) {
            Some(Ok(r)) => r,
            other => panic!("{other:?}"),
        }
    }

    fn until(&mut self, what: &str, done: impl Fn(&[Event]) -> bool) {
        let deadline = Instant::now() + WAIT;
        while !done(&self.events) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            if let Ok(ServerFrame::Event { event, .. }) =
                self.rx.recv_timeout(Duration::from_millis(50))
            {
                self.events.push(event);
            }
        }
    }

    fn create(&mut self, cwd: &Path) -> String {
        match self.ok(Call::CreateSession {
            cwd: cwd.display().to_string(),
            request: "c1".into(),
        }) {
            Reply::Session { summary } => summary.id,
            other => panic!("{other:?}"),
        }
    }

    fn prompt(
        &mut self,
        session: &str,
        key: &str,
        text: &str,
    ) -> Option<Result<Reply, ProtoError>> {
        self.call(Call::Prompt {
            session: session.into(),
            request: key.into(),
            text: text.into(),
            images: vec![],
            model: None,
        })
    }

    fn lookup(&mut self, session: &str, key: &str) -> LookupResult {
        match self.ok(Call::Lookup {
            session: session.into(),
            request: key.into(),
        }) {
            Reply::Lookup(l) => l,
            other => panic!("{other:?}"),
        }
    }

    fn settled(&mut self, session: &str, key: &str) -> RunOutcome {
        let deadline = Instant::now() + WAIT;
        loop {
            if let LookupResult::Known {
                open: false,
                outcome: Some(outcome),
                ..
            } = self.lookup(session, key)
            {
                return outcome;
            }
            assert!(Instant::now() < deadline, "{key} did not settle");
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn snapshot(&mut self, session: &str) -> SessionSnapshot {
        match self.ok(Call::OpenSession {
            session: session.into(),
            window: None,
        }) {
            Reply::Snapshot(s) => *s,
            other => panic!("{other:?}"),
        }
    }

    fn history(&mut self, session: &str) -> Vec<Entry> {
        match self.ok(Call::HistoryPage {
            session: session.into(),
            before: None,
            limit: 100,
        }) {
            Reply::Page {
                entries,
                has_older: false,
                ..
            } => entries,
            other => panic!("fixture should fit one history page: {other:?}"),
        }
    }

    /// SIGKILL: no orderly shutdown of any kind.
    fn kill(mut self) {
        self.child.kill().unwrap();
        self.reap_killed();
    }

    fn reap_killed(&mut self) {
        use std::os::unix::process::ExitStatusExt;
        let status = self.child.wait().unwrap();
        assert_eq!(status.signal(), Some(libc::SIGKILL), "{status:?}");
    }

    /// Orderly exit (stdin closed).
    fn close(mut self) {
        drop(self.input.take());
        assert!(self.child.wait().unwrap().success());
    }
}

/// `(role, text, retired)` for each message entry.
fn texts(s: &SessionSnapshot) -> Vec<(String, String, bool)> {
    s.entries
        .iter()
        .filter_map(|e| match &e.kind {
            EntryKind::Message { message, .. } => Some((
                match message {
                    Message::System(_) => return None,
                    Message::User { .. } => "user",
                    Message::Assistant { .. } => "assistant",
                    Message::ToolResult { .. } => "tool",
                }
                .to_owned(),
                message.text(),
                e.retired,
            )),
            _ => None,
        })
        .collect()
}

fn count(s: &SessionSnapshot, role: &str) -> usize {
    texts(s).iter().filter(|t| t.0 == role).count()
}

/// Processes still running for `session` (or its subagent children): every process a tool call
/// starts carries `PI_SESSION_ID`.
fn session_processes(session: &str) -> Vec<u32> {
    let needle = format!("PI_SESSION_ID={session}");
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| {
            std::fs::read(format!("/proc/{pid}/environ")).is_ok_and(|env| {
                env.split(|b| *b == 0)
                    .any(|v| v.starts_with(needle.as_bytes()))
            })
        })
        .collect()
}

/// Within a second: well before any command these tests start would end by itself.
fn no_processes_left(session: &str) {
    let deadline = Instant::now() + Duration::from_secs(1);
    while !session_processes(session).is_empty() {
        assert!(
            Instant::now() < deadline,
            "orphaned processes: {:?}",
            session_processes(session)
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ----------------------------------------------------------------------------- crash points

/// (a) Killed after admission was acknowledged, before the provider received anything: the
/// provider is a socket that never accepts. After the restart the run resumes on its own, the
/// prompt is asked once, and nothing is duplicated.
#[test]
fn sigkill_after_admission_before_the_model_is_asked() {
    let black_hole = TcpListener::bind("127.0.0.1:0").unwrap();
    let p = Profile::new(&format!(
        "http://127.0.0.1:{}/v1",
        black_hole.local_addr().unwrap().port()
    ));
    let mut w = Worker::start(&p, None);
    let s = w.create(&p.project);
    let Some(Ok(Reply::Admitted { run: Some(_), .. })) = w.prompt(&s, "k1", "hello once") else {
        panic!()
    };
    std::thread::sleep(Duration::from_millis(300));
    w.kill();
    // Never accepted, never answered.
    drop(black_hole);

    let stub = Stub::start(|_, _| text_plan("the answer"));
    p.point_at(&stub.url());
    let mut w = Worker::start(&p, None);
    assert_eq!(w.settled(&s, "k1"), RunOutcome::Completed);
    let bodies = stub.bodies();
    assert_eq!(bodies.len(), 1, "asked once");
    assert_eq!(messages_with_role(&bodies[0], "user"), ["hello once"]);
    assert!(messages_with_role(&bodies[0], "assistant").is_empty());
    let snap = w.snapshot(&s);
    assert_eq!(
        texts(&snap),
        vec![
            ("user".into(), "hello once".into(), false),
            ("assistant".into(), "the answer".into(), false)
        ]
    );
    // The same key again is the original admission.
    let Some(Ok(Reply::Admitted { .. })) = w.prompt(&s, "k1", "hello once") else {
        panic!()
    };
    assert_eq!(stub.bodies().len(), 1);
    no_processes_left(&s);
    w.close();
}

/// (a') Killed after admission committed but before its acknowledgment was written: the client
/// never heard back, and a lookup after the restart finds the admission (never a second one).
#[test]
fn sigkill_after_admission_before_the_acknowledgment() {
    let stub = Stub::start(|_, _| text_plan("answered once"));
    let p = Profile::new(&stub.url());
    let mut w = Worker::start(&p, Some("after-admission"));
    let s = w.create(&p.project);
    assert!(
        w.prompt(&s, "k1", "hello").is_none(),
        "died before replying"
    );
    w.reap_killed();
    assert!(stub.bodies().is_empty(), "the model was not asked");

    let mut w = Worker::start(&p, None);
    assert!(matches!(w.lookup(&s, "k1"), LookupResult::Known { .. }));
    assert_eq!(w.settled(&s, "k1"), RunOutcome::Completed);
    // The desktop's reconciliation resend under the same key is the original.
    let Some(Ok(Reply::Admitted { .. })) = w.prompt(&s, "k1", "hello") else {
        panic!()
    };
    let snap = w.snapshot(&s);
    assert_eq!(count(&snap, "user"), 1);
    assert_eq!(stub.bodies().len(), 1);
    assert_eq!(messages_with_role(&stub.bodies()[0], "user"), ["hello"]);
    w.close();
}

/// (b) Killed mid-stream after partial deltas were committed (and published). The restart
/// resumes the run: the next request carries the prompt once and no half reply; the committed
/// part is kept in history, finalized as aborted.
#[test]
fn sigkill_mid_stream_after_partial_deltas_were_committed() {
    let stub = Stub::start(|_, _| partial_then_hang(&["alpha ", "beta ", "gamma "]));
    let p = Profile::new(&stub.url());
    let mut w = Worker::start(&p, None);
    let s = w.create(&p.project);
    w.prompt(&s, "k1", "go").unwrap().unwrap();
    w.until("committed deltas", |ev| {
        let text: String = ev
            .iter()
            .filter_map(|e| match e {
                Event::Session {
                    change:
                        SessionChange::EntryDelta {
                            delta: Delta::Text { text, .. },
                            ..
                        },
                    ..
                } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        text.contains("gamma")
    });
    w.kill();

    stub.set_script(|_, _| text_plan("final answer"));
    let mut w = Worker::start(&p, None);
    assert_eq!(w.settled(&s, "k1"), RunOutcome::Completed);
    let bodies = stub.bodies();
    assert_eq!(bodies.len(), 2);
    assert_eq!(messages_with_role(&bodies[1], "user"), ["go"]);
    assert!(
        messages_with_role(&bodies[1], "assistant").is_empty(),
        "the half reply is not context: {}",
        bodies[1]["messages"]
    );
    let snap = w.snapshot(&s);
    let all = texts(&snap);
    assert_eq!(count(&snap, "user"), 1, "{all:?}");
    let assistants: Vec<_> = all.iter().filter(|t| t.0 == "assistant").collect();
    assert_eq!(assistants.len(), 2, "{all:?}");
    assert_eq!(assistants[0].1, "alpha beta gamma ");
    assert_eq!(assistants[1].1, "final answer");
    assert!(snap.entries.iter().any(|e| matches!(
        &e.kind,
        EntryKind::Message {
            message: Message::Assistant {
                stop_reason: StopReason::Aborted,
                ..
            },
            partial: false,
            ..
        }
    )));
    no_processes_left(&s);
    w.close();
}

/// (c) Killed after a tool intent was committed, while `bash` runs. What the command left
/// running is killed at the restart (its second half never runs), the restart records an
/// interrupted result instead of rerunning it, and the model is told so.
#[test]
fn sigkill_while_a_command_runs_records_it_interrupted_and_never_reruns_it() {
    let tmp = tempfile::tempdir().unwrap();
    let started = tmp.path().join("started.txt");
    let finished = tmp.path().join("finished.txt");
    let command = format!(
        "echo started >> {}; printf 'committed tool prefix\\n'; sleep 4; echo finished > {}",
        started.display(),
        finished.display()
    );
    let stub = Stub::start(move |n, _| tool_plan(n, "bash", json!({ "command": command })));
    let p = Profile::new(&stub.url());
    let mut w = Worker::start(&p, None);
    let s = w.create(&p.project);
    let begun = Instant::now();
    w.prompt(&s, "k1", "run the job").unwrap().unwrap();
    let deadline = Instant::now() + WAIT;
    while !started.exists() {
        assert!(Instant::now() < deadline, "the command never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    w.until("tool prefix", |events| events.iter().any(|event| matches!(event,Event::Session { change:SessionChange::ToolProgress { text,.. },.. } if text.contains("committed tool prefix"))));
    w.kill();
    assert!(
        !session_processes(&s).is_empty(),
        "the command outlives the killed engine until a restart reaps it"
    );

    stub.set_script(|_, _| text_plan("noted"));
    let mut w = Worker::start(&p, None);
    assert_eq!(w.settled(&s, "k1"), RunOutcome::Completed);
    assert!(
        begun.elapsed() < Duration::from_secs(3),
        "{:?}",
        begun.elapsed()
    );
    no_processes_left(&s);
    // Past the command's own duration: its second half never ran, and it ran once.
    let left = Duration::from_secs(5).saturating_sub(begun.elapsed());
    std::thread::sleep(left);
    assert!(
        !finished.exists(),
        "the interrupted command was not left to finish"
    );
    assert_eq!(std::fs::read_to_string(&started).unwrap(), "started\n");
    let snap = w.snapshot(&s);
    let all = texts(&snap);
    assert_eq!(count(&snap, "user"), 1, "{all:?}");
    let tool: Vec<_> = all.iter().filter(|t| t.0 == "tool").collect();
    assert_eq!(tool.len(), 1, "{all:?}");
    assert!(
        tool[0]
            .1
            .contains("Tool bash was interrupted and may have partially run"),
        "{all:?}"
    );
    assert!(
        tool[0].1.contains("committed tool prefix"),
        "last published partial output must survive: {tool:?}"
    );
    let bodies = stub.bodies();
    assert_eq!(bodies.len(), 2, "the tool call is not asked for again");
    let results = messages_with_role(&bodies[1], "tool");
    assert_eq!(results.len(), 1);
    assert!(results[0].contains("was interrupted"), "{results:?}");
    assert!(results[0].contains("committed tool prefix"));
    assert_eq!(messages_with_role(&bodies[1], "user"), ["run the job"]);
    w.close();
}

#[test]
fn sigkill_after_tool_checkpoint_before_progress_publication_keeps_the_output() {
    let stub = Stub::start(|n, _| {
        tool_plan(
            n,
            "bash",
            json!({"command": "printf 'checkpoint-only-prefix'; sleep 30"}),
        )
    });
    let p = Profile::new(&stub.url());
    let mut w = Worker::start(&p, Some("after_tool_progress_checkpoint"));
    let s = w.create(&p.project);
    w.prompt(&s, "k1", "run it").unwrap().unwrap();
    let deadline = Instant::now() + WAIT;
    while w.child.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "checkpoint crash did not happen");
        std::thread::sleep(Duration::from_millis(20));
    }
    w.reap_killed();
    // Drain even events not consumed while waiting for the process to die.
    w.events
        .extend(w.rx.try_iter().filter_map(|frame| match frame {
            ServerFrame::Event { event, .. } => Some(event),
            _ => None,
        }));
    assert!(!w.events.iter().any(|event| matches!(
        event,
        Event::Session {
            change: SessionChange::ToolProgress { .. },
            ..
        }
    )));
    stub.set_script(|_, _| text_plan("recovered"));
    let mut w = Worker::start(&p, None);
    assert_eq!(w.settled(&s, "k1"), RunOutcome::Completed);
    let results = messages_with_role(&stub.bodies()[1], "tool");
    assert_eq!(results.len(), 1);
    assert!(results[0].contains("checkpoint-only-prefix"));
    assert!(results[0].contains("was interrupted"));
    no_processes_left(&s);
    w.close();
}

#[test]
fn sigkill_tool_recovery_preserves_bounded_output_and_stop_intent() {
    for stop in [false, true] {
        let stub = Stub::start(|n, _| {
            tool_plan(
                n,
                "bash",
                json!({"command": "printf '%60000s' x; printf '\\nTAIL-✓\\n'; sleep 30"}),
            )
        });
        let p = Profile::new(&stub.url());
        let mut w = Worker::start(&p, stop.then_some("after-stop-intent"));
        let s = w.create(&p.project);
        w.prompt(&s, "k1", "run it").unwrap().unwrap();
        w.until("bounded tail", |events| {
            events.iter().any(|event| {
                matches!(event,
                    Event::Session { change: SessionChange::ToolProgress { text, .. }, .. }
                        if text.contains("TAIL-✓")
                )
            })
        });
        let (call_id, published) = w
            .events
            .iter()
            .rev()
            .find_map(|event| match event {
                Event::Session {
                    change: SessionChange::ToolProgress { call_id, text, .. },
                    ..
                } if text.contains("TAIL-✓") => Some((call_id.clone(), text.clone())),
                _ => None,
            })
            .unwrap();
        assert!(published.len() <= 50 * 1024);
        if stop {
            assert!(w.call(Call::Stop { session: s.clone() }).is_none());
            w.reap_killed();
        } else {
            w.kill();
        }
        stub.set_script(|_, _| text_plan("noted"));
        let mut w = Worker::start(&p, None);
        assert_eq!(
            w.settled(&s, "k1"),
            if stop {
                RunOutcome::Aborted
            } else {
                RunOutcome::Completed
            }
        );
        let snap = w.snapshot(&s);
        let all = texts(&snap);
        let results: Vec<_> = all.iter().filter(|t| t.0 == "tool").collect();
        assert_eq!(results.len(), 1);
        assert!(results[0].1.contains(&published));
        assert!(results[0].1.contains("Output truncated"));
        assert!(results[0].1.contains(if stop {
            "was aborted"
        } else {
            "was interrupted"
        }));
        let Reply::ToolOutput(output) = w.ok(Call::ToolOutput {
            session: s.clone(),
            call_id: call_id.clone(),
        }) else {
            panic!()
        };
        assert_eq!(output.text, published);
        assert_eq!(output.retained_bytes, published.len() as u64);
        assert!(output.dropped_bytes > 0);
        assert_eq!(stub.bodies().len(), if stop { 1 } else { 2 });
        no_processes_left(&s);
        w.close();
        let store = pipkin_engine::store::Store::open(&p.data).unwrap();
        let remaining = store
            .read(|c| {
                Ok(c.query_row(
                    "SELECT COUNT(*) FROM kv WHERE session = ?1 AND key LIKE 'tool_progress:%'",
                    [&s],
                    |row| row.get::<_, i64>(0),
                )?)
            })
            .unwrap();
        assert_eq!(
            remaining, 0,
            "settlement clears transient checkpoints atomically"
        );
        assert!(
            store
                .tool_call(&s, &call_id)
                .unwrap()
                .unwrap()
                .result_entry
                .is_some()
        );
    }
}

/// (d) Killed after the Stop intent was committed, before the run settled. The restart settles
/// it Aborted without asking the model again, and the stopped prompt is retired.
#[test]
fn sigkill_after_the_stop_intent_before_settlement() {
    let stub = Stub::start(|_, _| hang_plan());
    let p = Profile::new(&stub.url());
    let mut w = Worker::start(&p, Some("after-stop-intent"));
    let s = w.create(&p.project);
    w.prompt(&s, "k1", "never mind").unwrap().unwrap();
    let deadline = Instant::now() + WAIT;
    while stub.bodies().is_empty() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        w.call(Call::Stop { session: s.clone() }).is_none(),
        "died at the crash point"
    );
    w.reap_killed();
    {
        // What the dead engine left: the Stop is durable, the run is not settled.
        let store = pipkin_engine::store::Store::open(&p.data).unwrap();
        let runs = store.unsettled_runs().unwrap();
        assert_eq!(runs.len(), 1);
        assert!(runs[0].stop_requested);
    }

    stub.set_script(|_, _| text_plan("must not be asked"));
    let mut w = Worker::start(&p, None);
    assert_eq!(w.settled(&s, "k1"), RunOutcome::Aborted);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(stub.bodies().len(), 1, "the model is not asked again");
    let snap = w.snapshot(&s);
    assert!(snap.run.is_none());
    assert_eq!(texts(&snap)[0], ("user".into(), "never mind".into(), true));
    assert_eq!(count(&snap, "user"), 1);
    // The next prompt does not see the stopped one.
    w.prompt(&s, "k2", "something else").unwrap().unwrap();
    assert_eq!(w.settled(&s, "k2"), RunOutcome::Completed);
    let bodies = stub.bodies();
    assert_eq!(messages_with_role(&bodies[1], "user"), ["something else"]);
    no_processes_left(&s);
    w.close();
}

/// (e) Killed while a subagent child runs. After the restart the parent's replay-safe call
/// finds the same child (exactly one), the child resumes, and the parent receives one result.
#[test]
fn sigkill_during_a_subagent_child_run_resumes_the_one_child() {
    let child_hangs = Arc::new(AtomicBool::new(true));
    let hangs = child_hangs.clone();
    let stub = Stub::start(move |n, body| {
        if !offers_tool(body, "subagent") {
            return if hangs.load(Ordering::SeqCst) {
                hang_plan()
            } else {
                text_plan("child result")
            };
        }
        let results = messages_with_role(body, "tool");
        if results.is_empty() {
            tool_plan(n, "subagent", json!({"task": "survey the files"}))
        } else {
            text_plan(&format!("parent got: {}", results.join(" | ")))
        }
    });
    let p = Profile::new(&stub.url());
    let mut w = Worker::start(&p, None);
    let s = w.create(&p.project);
    w.prompt(&s, "k1", "delegate it").unwrap().unwrap();
    let child_asked = |stub: &Stub| {
        stub.bodies()
            .iter()
            .filter(|b| !offers_tool(b, "subagent"))
            .count()
    };
    let deadline = Instant::now() + WAIT;
    while child_asked(&stub) == 0 {
        assert!(Instant::now() < deadline, "the child never asked its model");
        std::thread::sleep(Duration::from_millis(20));
    }
    w.kill();

    child_hangs.store(false, Ordering::SeqCst);
    let mut w = Worker::start(&p, None);
    assert_eq!(w.settled(&s, "k1"), RunOutcome::Completed);
    let Reply::Subagents(view) = w.ok(Call::Subagents { session: s.clone() }) else {
        panic!()
    };
    assert_eq!(view.children.len(), 1, "exactly one child");
    assert_eq!(view.children[0].status, "done");
    let child = view.children[0].session.clone();
    assert_eq!(
        w.settled(&child, &format!("subagent:{}", view.children[0].call_id)),
        RunOutcome::Completed
    );
    let snap = w.snapshot(&s);
    let all = texts(&snap);
    assert_eq!(count(&snap, "user"), 1, "{all:?}");
    let tool: Vec<_> = all.iter().filter(|t| t.0 == "tool").collect();
    assert_eq!(tool.len(), 1, "one result: {all:?}");
    assert_eq!(tool[0].1, "child result");
    assert_eq!(all.last().unwrap().1, "parent got: child result");
    let bodies = stub.bodies();
    let parents: Vec<_> = bodies
        .iter()
        .filter(|b| offers_tool(b, "subagent"))
        .collect();
    assert_eq!(parents.len(), 2, "the call was not asked for again");
    assert_eq!(messages_with_role(parents[1], "tool"), ["child result"]);
    assert_eq!(child_asked(&stub), 2, "the one child resumed its turn");
    let Reply::Page { entries, .. } = w.ok(Call::SubagentHistory {
        session: s.clone(),
        child: view.children[0].child,
        before: None,
        limit: 50,
    }) else {
        panic!()
    };
    let child_users = entries
        .iter()
        .filter(|e| {
            matches!(
                &e.kind,
                EntryKind::Message {
                    message: Message::User { .. },
                    ..
                }
            )
        })
        .count();
    assert_eq!(child_users, 1, "the child's task was admitted once");
    no_processes_left(&s);
    w.close();
}

/// A damaged engine database stops the engine at start with an error naming the file, and the
/// file is left as it was.
#[test]
fn a_damaged_database_stops_the_engine_at_start_and_is_left_untouched() {
    let p = Profile::new("http://127.0.0.1:9/v1");
    std::fs::create_dir_all(&p.data).unwrap();
    let db = p.data.join("engine.sqlite");
    std::fs::write(
        &db,
        b"this is not an engine database, it is a damaged file\n".repeat(200),
    )
    .unwrap();
    let before = std::fs::read(&db).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pipkin-engine"))
        .arg("--data-dir")
        .arg(&p.data)
        .arg("--config-dir")
        .arg(&p.config)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(&db.display().to_string()), "{stderr}");
    assert!(stderr.contains("is damaged"), "{stderr}");
    assert_eq!(std::fs::read(&db).unwrap(), before);
    assert!(!p.data.join("engine.sqlite-wal").exists());
    let _ = &p.root;
}

#[test]
fn sigkill_around_retry_classification_preserves_the_deadline_and_attempt_budget() {
    for (point, retries) in [
        ("after_generation_result", 1),
        ("after_retry_checkpoint", 1),
        ("after_retry_checkpoint", 0),
    ] {
        let times = Arc::new(Mutex::new(Vec::new()));
        let recorded = times.clone();
        let stub = Stub::start(move |_, _| {
            recorded.lock().unwrap().push(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as i64,
            );
            Plan { pieces: vec!["data: {\"error\":{\"message\":\"503: overloaded\",\"type\":\"server_error\"}}\n\n".into()], hang: false }
        });
        let p = Profile::new(&stub.url());
        std::fs::write(
            p.config.join("settings.json"),
            json!({"retry":{"maxRetries":retries,"baseDelayMs":900,"provider":{"maxRetries":0}}})
                .to_string(),
        )
        .unwrap();
        let mut w = Worker::start(&p, Some(point));
        let s = w.create(&p.project);
        let _ = w.prompt(&s, "r1", "work");
        w.reap_killed();
        let conn = rusqlite::Connection::open(p.data.join("engine.sqlite")).unwrap();
        let until: Option<i64> = conn
            .query_row("SELECT retry_until FROM generation_state", [], |r| r.get(0))
            .ok()
            .flatten();
        if point == "after_retry_checkpoint" && retries > 0 {
            assert!(until.is_some());
        }
        drop(conn);
        let mut w = Worker::start(&p, None);
        assert!(
            matches!(w.settled(&s, "r1"), RunOutcome::ModelError { .. }),
            "{point}"
        );
        assert_eq!(
            stub.bodies().len(),
            1 + retries,
            "{point}: no replenished budget"
        );
        if let Some(until) = until {
            assert!(times.lock().unwrap()[1] >= until, "{point}: no early retry");
        }
        w.close();
        let mut w = Worker::start(&p, None);
        assert!(matches!(w.settled(&s, "r1"), RunOutcome::ModelError { .. }));
        assert_eq!(
            stub.bodies().len(),
            1 + retries,
            "settled errors never resume"
        );
        w.close();
    }
}

#[test]
fn sigkill_between_state_commit_and_publication_is_recovered_from_the_outbox() {
    let stub = Stub::start(|_, _| text_plan("answered once"));
    let p = Profile::new(&stub.url());
    let mut w = Worker::start(&p, Some("after_state_commit_before_delivery"));
    let s = w.create(&p.project);
    let Reply::Events(before) = w.ok(Call::CatchUp {
        after: 0,
        limit: 256,
    }) else {
        panic!()
    };
    w.ok(Call::AckEvents {
        through: before.through,
    });
    assert!(w.prompt(&s, "k1", "hello").is_none());
    w.reap_killed();
    let mut w = Worker::start(&p, None);
    let Reply::Events(after) = w.ok(Call::CatchUp {
        after: before.through,
        limit: 256,
    }) else {
        panic!()
    };
    assert!(!after.reset);
    assert!(
        after
            .events
            .iter()
            .any(|e| e.session == s && e.cursor > before.through)
    );
    assert_eq!(w.settled(&s, "k1"), RunOutcome::Completed);
    let snap = w.snapshot(&s);
    assert_eq!(count(&snap, "user"), 1);
    assert_eq!(count(&snap, "assistant"), 1);
    assert_eq!(stub.bodies().len(), 1);
    w.ok(Call::AckEvents {
        through: after.through,
    });
    w.close();
}

fn compaction_settings(p: &Profile, enabled: bool, retries: u32) {
    std::fs::write(
        p.config.join("settings.json"),
        json!({
            "compaction": {"enabled":enabled,"keepRecentTokens":5},
            "retry":{"maxRetries":retries,"baseDelayMs":900,"provider":{"maxRetries":0}}
        })
        .to_string(),
    )
    .unwrap();
}

fn seed_compaction(w: &mut Worker, p: &Profile) -> String {
    let s = w.create(&p.project);
    for i in 0..4 {
        let key = format!("seed-{i}");
        w.prompt(&s, &key, "old conversation with several words")
            .unwrap()
            .unwrap();
        assert_eq!(w.settled(&s, &key), RunOutcome::Completed);
    }
    s
}

fn wait_compactions(p: &Profile) -> Value {
    let conn = rusqlite::Connection::open(p.data.join("engine.sqlite")).unwrap();
    let deadline = Instant::now() + WAIT;
    loop {
        let (state, progress): (String, String) = conn
            .query_row(
                "SELECT state, progress FROM compactions ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        if state == "settled" {
            return serde_json::from_str(&progress).unwrap();
        }
        assert!(
            Instant::now() < deadline,
            "compaction did not settle: {progress}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn sigkill_compaction_boundaries_recover_without_open_and_account_once() {
    for point in [
        "compaction_admitted",
        "compaction_selected",
        "compaction_response_saved",
        "compaction_classified",
    ] {
        let stub = Stub::start(|_, _| text_plan("saved summary or answer"));
        let p = Profile::new(&stub.url());
        compaction_settings(&p, false, 1);
        let mut w = Worker::start(&p, Some(point));
        let s = seed_compaction(&mut w, &p);
        let _ = w.call(Call::Compact {
            session: s.clone(),
            instructions: Some("focus on durable state".into()),
        });
        w.reap_killed();
        let conn = rusqlite::Connection::open(p.data.join("engine.sqlite")).unwrap();
        let saved: String = conn
            .query_row("SELECT progress FROM compactions", [], |r| r.get(0))
            .unwrap();
        let saved: Value = serde_json::from_str(&saved).unwrap();
        // A selected request must not be reconstructed from a changed session model/settings.
        if point != "compaction_admitted" {
            conn.execute(
                "UPDATE sessions SET model = 'missing/model' WHERE id = ?1",
                [&s],
            )
            .unwrap();
            std::fs::write(
                p.config.join("settings.json"),
                json!({"compaction":{"keepRecentTokens":1000000},"retry":{"enabled":false}})
                    .to_string(),
            )
            .unwrap();
        }
        drop(conn);
        let mut w = Worker::start(&p, None);
        // No OpenSession/Compact request: startup alone recovers the journal.
        let terminal = wait_compactions(&p);
        assert_eq!(
            terminal["result"]["outcome"], "submitted",
            "{point}: {terminal}"
        );
        assert_eq!(
            stub.bodies().len(),
            5,
            "{point}: saved responses must not be sent again"
        );
        if let Some(request) = saved.get("request") {
            let body = stub.bodies().last().unwrap().clone();
            assert_eq!(
                messages_with_role(&body, "user"),
                [request["messages"][0]["content"][0]["text"]
                    .as_str()
                    .unwrap()]
            );
        }
        let snap = w.snapshot(&s);
        assert_eq!(
            snap.entries
                .iter()
                .filter(|e| matches!(e.kind, EntryKind::Compaction { .. }))
                .count(),
            1
        );
        let conn = rusqlite::Connection::open(p.data.join("engine.sqlite")).unwrap();
        let total: u64 = conn
            .query_row(
                "SELECT json_extract(value, '$.totalTokens') FROM usage WHERE bucket = 'model'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            total, 75,
            "{point}: four coding turns and exactly one summary"
        );
        drop(conn);
        w.close();
        let w = Worker::start(&p, None);
        assert_eq!(wait_compactions(&p), terminal);
        assert_eq!(stub.bodies().len(), 5);
        w.close();
    }
}

#[test]
fn sigkill_compaction_retry_keeps_deadline_and_attempt_count() {
    for point in ["compaction_response_saved", "compaction_classified"] {
        let times = Arc::new(Mutex::new(Vec::new()));
        let recorded = times.clone();
        let stub = Stub::start(move |n, _| {
            if n < 4 {
                return text_plan("seed answer");
            }
            recorded.lock().unwrap().push(pipkin_engine::util::now_ms());
            Plan { pieces: vec!["data: {\"error\":{\"message\":\"503: overloaded\",\"type\":\"server_error\"}}\n\n".into()], hang: false }
        });
        let p = Profile::new(&stub.url());
        compaction_settings(&p, false, 1);
        let mut w = Worker::start(&p, Some(point));
        let s = seed_compaction(&mut w, &p);
        let _ = w.call(Call::Compact {
            session: s,
            instructions: None,
        });
        w.reap_killed();
        let conn = rusqlite::Connection::open(p.data.join("engine.sqlite")).unwrap();
        let progress: String = conn
            .query_row("SELECT progress FROM compactions", [], |r| r.get(0))
            .unwrap();
        let progress: Value = serde_json::from_str(&progress).unwrap();
        let until = progress["until"]
            .as_i64()
            .unwrap_or_else(|| progress["response"]["receivedAt"].as_i64().unwrap() + 900);
        drop(conn);
        let w = Worker::start(&p, None);
        assert_eq!(wait_compactions(&p)["result"]["outcome"], "failed");
        let recorded = times.lock().unwrap();
        assert_eq!(
            recorded.len(),
            2,
            "{point}: one original attempt plus one retry"
        );
        assert!(
            recorded[1] >= until,
            "{point}: retry occurred before saved deadline"
        );
        drop(recorded);
        w.close();
    }
}

#[test]
fn sigkill_after_overflow_summary_reuses_it_before_retrying_coding() {
    let stub = Stub::start(|n, _| {
        if n == 4 {
            Plan { pieces: vec!["data: {\"error\":{\"message\":\"prompt is too long: 999999 tokens > 200000 maximum\"}}\n\n".into()], hang: false }
        } else {
            text_plan("answer or summary")
        }
    });
    let p = Profile::new(&stub.url());
    compaction_settings(&p, true, 0);
    let mut w = Worker::start(&p, Some("compaction_classified"));
    let s = seed_compaction(&mut w, &p);
    let _ = w.prompt(&s, "overflow", "trigger overflow");
    w.reap_killed();
    compaction_settings(&p, false, 0); // Even disabling compaction cannot discard its handoff.
    let mut w = Worker::start(&p, None);
    assert_eq!(w.settled(&s, "overflow"), RunOutcome::Completed);
    assert_eq!(
        stub.bodies().len(),
        7,
        "four seeds, overflow, one summary, successful retry"
    );
    let snap = w.snapshot(&s);
    assert_eq!(
        snap.entries
            .iter()
            .filter(|e| matches!(e.kind, EntryKind::Compaction { .. }))
            .count(),
        1
    );
    let bodies = stub.bodies();
    assert!(
        messages_with_role(&bodies[6], "user")
            .iter()
            .any(|text| text == "trigger overflow")
    );
    w.close();
}

#[test]
fn sigkill_around_tool_boundary_places_summary_once_and_consumes_only_one_steer() {
    for point in ["after_tools_before_boundary", "after_tool_boundary"] {
        let stub = Stub::start(|n, _| match n {
            4 => tool_plan(
                n,
                "bash",
                json!({"command": "printf x >> effects; touch tool-started; while ! test -f release-tool; do sleep 0.01; done; printf tool-done"}),
            ),
            5 => text_plan("queued summary"),
            _ => text_plan("answer"),
        });
        let p = Profile::new(&stub.url());
        compaction_settings(&p, false, 0);
        let mut w = Worker::start(&p, Some(point));
        let s = seed_compaction(&mut w, &p);
        w.prompt(&s, "tools", "use a tool").unwrap().unwrap();
        let deadline = Instant::now() + WAIT;
        while !p.project.join("tool-started").exists() {
            assert!(Instant::now() < deadline, "tool did not start");
            std::thread::sleep(Duration::from_millis(10));
        }
        for (key, mode) in [
            ("steer-one", InputMode::Steer),
            ("steer-two", InputMode::Steer),
            ("follow", InputMode::FollowUp),
        ] {
            w.ok(Call::Enqueue {
                session: s.clone(),
                request: key.into(),
                mode,
                text: key.into(),
                images: vec![],
            });
        }
        w.ok(Call::Compact {
            session: s.clone(),
            instructions: None,
        });
        let conn = rusqlite::Connection::open(p.data.join("engine.sqlite")).unwrap();
        let queued: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM summary_writes WHERE state = 'queued'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(queued, 1);
        std::fs::write(p.project.join("release-tool"), "go").unwrap();
        w.reap_killed();
        let mut w = Worker::start(&p, None);
        for key in ["tools", "steer-one", "steer-two", "follow"] {
            assert_eq!(w.settled(&s, key), RunOutcome::Completed);
        }
        let mut snap = w.snapshot(&s);
        // OpenSession intentionally starts at the newest head marker; inspect the
        // pageable history too when proving pre-compaction tool/user ordering.
        snap.entries = w.history(&s);
        let summaries: Vec<_> = snap
            .entries
            .iter()
            .filter(|e| matches!(e.kind, EntryKind::Compaction { .. }))
            .collect();
        assert_eq!(summaries.len(), 1, "{point}");
        let user = |text: &str| {
            snap.entries.iter().find(|e| matches!(&e.kind, EntryKind::Message { message: m @ Message::User { .. }, .. } if m.text() == text)).unwrap_or_else(|| panic!("missing {text}: {:?}", texts(&snap)))
        };
        assert_eq!(user("use a tool").run, user("steer-one").run);
        assert_ne!(
            user("steer-one").run,
            user("steer-two").run,
            "{point}: one-at-a-time must survive replay"
        );
        assert_ne!(user("steer-one").run, user("follow").run);
        let result = snap
            .entries
            .iter()
            .find(|e| {
                matches!(
                    &e.kind,
                    EntryKind::Message {
                        message: Message::ToolResult { .. },
                        ..
                    }
                )
            })
            .unwrap();
        assert!(result.id < summaries[0].id && summaries[0].id < user("steer-one").id);
        assert_eq!(
            std::fs::read_to_string(p.project.join("effects")).unwrap(),
            "x",
            "{point}: completed tool must not replay"
        );
        let bodies = stub.bodies();
        let first_resumed = messages_with_role(&bodies[6], "user");
        assert!(first_resumed.iter().any(|text| text == "steer-one"));
        assert!(
            !first_resumed
                .iter()
                .any(|text| text == "steer-two" || text == "follow")
        );
        let (placed, boundaries): (u64,u64) = conn.query_row("SELECT (SELECT COUNT(*) FROM summary_writes WHERE state = 'placed'), (SELECT COUNT(*) FROM tool_boundaries)", [], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!((placed, boundaries), (1, 1));
        w.close();
    }
}

#[test]
fn sigkill_after_summary_submission_recovers_queued_write_without_resummarizing() {
    let stub = Stub::start(|n, _| match n {
        4 => hang_plan(),
        5 => text_plan("durable queued summary"),
        _ => text_plan("answer"),
    });
    let p = Profile::new(&stub.url());
    compaction_settings(&p, false, 0);
    let mut w = Worker::start(&p, Some("compaction_classified"));
    let s = seed_compaction(&mut w, &p);
    w.prompt(&s, "busy", "continue coding").unwrap().unwrap();
    let deadline = Instant::now() + WAIT;
    while stub.bodies().len() < 5 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = w.call(Call::Compact {
        session: s.clone(),
        instructions: None,
    });
    w.reap_killed();
    let conn = rusqlite::Connection::open(p.data.join("engine.sqlite")).unwrap();
    let queued: u64 = conn
        .query_row(
            "SELECT COUNT(*) FROM summary_writes WHERE state = 'queued'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(queued, 1);
    let mut w = Worker::start(&p, None);
    assert_eq!(w.settled(&s, "busy"), RunOutcome::Completed);
    let mut snap = w.snapshot(&s);
    snap.entries = w.history(&s);
    let summary = snap.entries.iter().find(|e| matches!(&e.kind, EntryKind::Compaction { summary, .. } if summary == "durable queued summary")).unwrap();
    let answer = snap
        .entries
        .iter()
        .rev()
        .find(|e| {
            matches!(
                &e.kind,
                EntryKind::Message {
                    message: Message::Assistant { .. },
                    partial: false,
                    ..
                }
            )
        })
        .unwrap();
    assert!(
        answer.id < summary.id,
        "write waits until the final boundary"
    );
    assert_eq!(
        stub.bodies().len(),
        7,
        "four seeds, interrupted coding, summary and coding retry"
    );
    let tokens: u64 = conn
        .query_row(
            "SELECT json_extract(value, '$.totalTokens') FROM usage WHERE bucket = 'model'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tokens, 90, "saved summary usage is counted exactly once");
    w.close();
    let w = Worker::start(&p, None);
    assert_eq!(stub.bodies().len(), 7);
    w.close();
}

/// Removal must stop external effects before dropping their recovery records, including
/// when SIGKILL lands on either side of the database deletion transaction.
#[test]
fn removal_reaps_live_tools_and_recovers_both_durable_crash_boundaries() {
    for point in [None, Some("removal_intent"), Some("removal_deleted")] {
        let stub = Stub::start(|n, _| {
            tool_plan(
                n,
                "bash",
                json!({"command": "echo started >> started.txt; sleep 30; echo forbidden > finished.txt"}),
            )
        });
        let p = Profile::new(&stub.url());
        let mut w = Worker::start(&p, point);
        let s = w.create(&p.project);
        w.prompt(&s, "job", "start a long tool").unwrap().unwrap();
        let deadline = Instant::now() + WAIT;
        while !p.project.join("started.txt").exists() {
            assert!(Instant::now() < deadline, "tool did not start: {point:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
        let Reply::Events(page) = w.ok(Call::CatchUp {
            after: 0,
            limit: 256,
        }) else {
            panic!()
        };
        let cursor = page.head;
        let result = w.call(Call::RemoveSession { session: s.clone() });
        if point.is_some() {
            assert!(result.is_none(), "must die at {point:?}: {result:?}");
            w.reap_killed();
        } else {
            assert_eq!(result, Some(Ok(Reply::Ok)));
            no_processes_left(&s);
            w.close();
        }
        let mut w = Worker::start(&p, None);
        assert_eq!(w.ok(Call::RemoveSession { session: s.clone() }), Reply::Ok);
        assert_eq!(
            w.ok(Call::ListSessions),
            Reply::Sessions { sessions: vec![] }
        );
        assert_eq!(
            w.call(Call::OpenSession {
                session: s.clone(),
                window: None
            })
            .unwrap()
            .unwrap_err()
            .code,
            ErrorCode::NotFound
        );
        assert_eq!(
            w.call(Call::CreateSession {
                cwd: p.project.display().to_string(),
                request: "c1".into()
            })
            .unwrap()
            .unwrap_err()
            .code,
            ErrorCode::NotFound
        );
        no_processes_left(&s);
        assert_eq!(
            stub.bodies().len(),
            1,
            "deleted work must not recover: {point:?}"
        );
        assert_eq!(
            std::fs::read_to_string(p.project.join("started.txt")).unwrap(),
            "started\n"
        );
        assert!(!p.project.join("finished.txt").exists());
        w.close();
        let store = pipkin_engine::store::Store::open(&p.data).unwrap();
        store.integrity_check().unwrap();
        assert!(store.all_sessions().unwrap().is_empty());
        assert!(
            store
                .catch_up(cursor, 256)
                .unwrap()
                .events
                .iter()
                .any(|e| e.session == s)
        );
    }
}
