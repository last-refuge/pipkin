//! Performance harness for the native engine (plan §7). Every test is `#[ignore]` so normal
//! CI stays fast. Run on a release build, one test at a time:
//!
//! ```text
//! cargo test --release -p pipkin-engine --test perf -- --ignored --nocapture --test-threads=1
//! ```
//!
//! or `scripts/bench-native.sh`, which also records the machine and package sizes. Raw JSON goes
//! to `$PIPKIN_BENCH_OUT` (default `<target>/tmp/native-bench`). Scratch profiles live under
//! `$PIPKIN_BENCH_SCRATCH` (default `<target>/tmp`, i.e. the same disk as the build, not tmpfs:
//! the store fsyncs, so the filesystem matters).
//!
//! What is driven: the real `pipkin-engine` worker binary over its stdio frame protocol, with a
//! `models.json` custom provider (`openai-completions`) pointed at a local scripted HTTP stub in
//! this process. So every number includes the real adapter, runtime, durable store (WAL,
//! `synchronous=FULL`) and frame encoding; nothing touches the network or a real provider.
//!
//! Clocks: the stub and the client run in this process and share `Instant`. "Chunk arrival" is
//! when the stub's `write` of that SSE chunk to the loopback socket returned; "committed
//! delta at the client" is when the client finished reading the frame carrying it.

use std::collections::{HashMap, VecDeque};
use std::io::{BufReader, BufWriter, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pipkin_engine_protocol::frame;
use pipkin_engine_protocol::*;
use serde_json::{Value, json};

#[path = "perf/mixed.rs"]
mod mixed;

// ---------------------------------------------------------------------------------------------
// Shared plumbing
// ---------------------------------------------------------------------------------------------

/// The tests measure one engine at a time even if the harness is run without
/// `--test-threads=1`.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn out_dir() -> PathBuf {
    let dir = std::env::var_os("PIPKIN_BENCH_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_TARGET_TMPDIR")).join("native-bench"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn scratch_root() -> PathBuf {
    let dir = std::env::var_os("PIPKIN_BENCH_SCRATCH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_TARGET_TMPDIR")));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_json(name: &str, value: &Value) {
    let path = out_dir().join(format!("{name}.json"));
    std::fs::write(&path, serde_json::to_string_pretty(value).unwrap()).unwrap();
    println!("wrote {}", path.display());
}

fn loadavg() -> String {
    std::fs::read_to_string("/proc/loadavg")
        .unwrap_or_default()
        .trim()
        .to_owned()
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 1e6).round() / 1e3
}

fn round3(x: f64) -> f64 {
    (x * 1e3).round() / 1e3
}

/// Nearest-rank percentile (the same rule as the reference driver's `pct`).
fn pct(xs: &[f64], p: f64) -> f64 {
    let mut s = xs.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let rank = ((p / 100.0) * s.len() as f64).ceil() as usize;
    s[rank.clamp(1, s.len()) - 1]
}

fn summary(xs: &[f64]) -> Value {
    if xs.is_empty() {
        return json!({ "n": 0 });
    }
    let mean = xs.iter().sum::<f64>() / xs.len() as f64;
    json!({
        "n": xs.len(),
        "p50": round3(pct(xs, 50.0)),
        "p95": round3(pct(xs, 95.0)),
        "min": round3(xs.iter().cloned().fold(f64::INFINITY, f64::min)),
        "max": round3(xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max)),
        "mean": round3(mean),
    })
}

/// Least-squares slope of y over x.
fn slope(points: &[(f64, f64)]) -> f64 {
    let n = points.len() as f64;
    if n < 2.0 {
        return 0.0;
    }
    let mx = points.iter().map(|p| p.0).sum::<f64>() / n;
    let my = points.iter().map(|p| p.1).sum::<f64>() / n;
    let num: f64 = points.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
    let den: f64 = points.iter().map(|p| (p.0 - mx).powi(2)).sum();
    if den == 0.0 { 0.0 } else { num / den }
}

// ---------------------------------------------------------------------------------------------
// /proc readers
// ---------------------------------------------------------------------------------------------

mod procfs {
    use std::collections::HashMap;

    pub fn status(pid: u32) -> HashMap<String, u64> {
        let text = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
        text.lines()
            .filter_map(|l| {
                let (k, v) = l.split_once(':')?;
                let n = v.split_whitespace().next()?.parse().ok()?;
                Some((k.to_owned(), n))
            })
            .collect()
    }

    pub fn rss_kib(pid: u32) -> u64 {
        status(pid).get("VmRSS").copied().unwrap_or(0)
    }

    pub fn smaps_rollup(pid: u32) -> HashMap<String, u64> {
        let text = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).unwrap_or_default();
        text.lines()
            .filter_map(|l| {
                let (k, v) = l.split_once(':')?;
                let n = v.split_whitespace().next()?.parse().ok()?;
                Some((k.trim().to_owned(), n))
            })
            .collect()
    }

    pub fn fds(pid: u32) -> u64 {
        std::fs::read_dir(format!("/proc/{pid}/fd"))
            .map(|d| d.count() as u64)
            .unwrap_or(0)
    }

    /// (state, ppid, utime+stime ticks, comm)
    pub fn stat(pid: u32) -> Option<(char, u32, u64, String)> {
        let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let open = text.find('(')?;
        let close = text.rfind(')')?;
        let comm = text[open + 1..close].to_owned();
        let rest: Vec<&str> = text[close + 1..].split_whitespace().collect();
        let state = rest.first()?.chars().next()?;
        let ppid = rest.get(1)?.parse().ok()?;
        let utime: u64 = rest.get(11)?.parse().ok()?;
        let stime: u64 = rest.get(12)?.parse().ok()?;
        Some((state, ppid, utime + stime, comm))
    }

    pub fn clk_tck() -> u64 {
        // SAFETY: sysconf has no preconditions.
        let t = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        if t > 0 { t as u64 } else { 100 }
    }

    /// CPU seconds (user + system, all threads) of a live process.
    pub fn cpu_secs(pid: u32) -> f64 {
        stat(pid).map(|s| s.2).unwrap_or(0) as f64 / clk_tck() as f64
    }

    pub fn io(pid: u32) -> HashMap<String, u64> {
        let text = std::fs::read_to_string(format!("/proc/{pid}/io")).unwrap_or_default();
        text.lines()
            .filter_map(|l| {
                let (k, v) = l.split_once(':')?;
                Some((k.trim().to_owned(), v.trim().parse().ok()?))
            })
            .collect()
    }

    /// Every live descendant of `pid`: (pid, comm).
    pub fn descendants(root: u32) -> Vec<(u32, String)> {
        let mut parent: Vec<(u32, u32, String)> = Vec::new();
        if let Ok(dir) = std::fs::read_dir("/proc") {
            for e in dir.flatten() {
                let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
                    continue;
                };
                if let Some((state, ppid, _, comm)) = stat(pid)
                    && state != 'Z'
                {
                    parent.push((pid, ppid, comm));
                }
            }
        }
        let mut out = Vec::new();
        let mut frontier = vec![root];
        while let Some(p) = frontier.pop() {
            for (pid, ppid, comm) in &parent {
                if *ppid == p {
                    out.push((*pid, comm.clone()));
                    frontier.push(*pid);
                }
            }
        }
        out
    }
}

fn mem_sample(pid: u32) -> Value {
    let st = procfs::status(pid);
    let sm = procfs::smaps_rollup(pid);
    json!({
        "rssKiB": st.get("VmRSS"),
        "hwmKiB": st.get("VmHWM"),
        "rssAnonKiB": st.get("RssAnon"),
        "rssFileKiB": st.get("RssFile"),
        "pssKiB": sm.get("Pss"),
        "pssAnonKiB": sm.get("Pss_Anon"),
        "threads": st.get("Threads"),
        "fds": procfs::fds(pid),
        "children": procfs::descendants(pid).len(),
        "cpuSecs": round3(procfs::cpu_secs(pid)),
    })
}

// ---------------------------------------------------------------------------------------------
// Scripted OpenAI-compatible stub
// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
struct Piece {
    data: String,
    marker: Option<u64>,
}

#[derive(Clone, Default)]
struct Plan {
    pieces: Vec<Piece>,
    /// Pause between pieces.
    gap: Duration,
    /// After the pieces, keep the response open (sending keep-alive comments) until the
    /// client closes it.
    hang: bool,
}

type Script = dyn Fn(u64, &Value) -> Plan + Send + Sync;

struct StubState {
    script: Mutex<Arc<Script>>,
    count: AtomicU64,
    /// Marker -> when its chunk was written.
    marks: Mutex<HashMap<u64, Instant>>,
    /// When a hanging response saw the client close the connection.
    closed: Mutex<Vec<Instant>>,
    sse_bytes: AtomicU64,
    request_bytes: AtomicU64,
    stop: AtomicBool,
}

struct Stub {
    state: Arc<StubState>,
    port: u16,
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::SeqCst);
    }
}

fn sse(delta: Value, finish: Option<&str>, usage: bool) -> String {
    let mut chunk = json!({
        "id": "stub-1",
        "object": "chat.completion.chunk",
        "created": 0,
        "model": "scripted",
        "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }],
    });
    if usage {
        chunk["usage"] = json!({ "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 });
    }
    format!("data: {chunk}\n\n")
}

fn piece(data: String) -> Piece {
    Piece { data, marker: None }
}

/// The reference stub's text reply: role, one content chunk, finish with usage, `[DONE]`.
fn text_plan(text: &str) -> Plan {
    Plan {
        pieces: vec![
            piece(sse(
                json!({"role": "assistant", "content": ""}),
                None,
                false,
            )),
            piece(sse(json!({ "content": text }), None, false)),
            piece(sse(json!({}), Some("stop"), true)),
            piece("data: [DONE]\n\n".into()),
        ],
        ..Plan::default()
    }
}

/// `count` content chunks of about `bytes` each, every one tagged `#<marker>;` so the client
/// can match it to the delta that carried it.
fn marked_plan(count: u64, bytes: usize, gap: Duration, first: u64) -> Plan {
    let filler = "lorem ipsum dolor sit amet ".repeat(bytes / 27 + 2);
    let mut pieces = vec![piece(sse(
        json!({"role": "assistant", "content": ""}),
        None,
        false,
    ))];
    for k in 0..count {
        let m = first + k;
        let mut text = format!("#{m:09};");
        let room = bytes.saturating_sub(text.len());
        text.push_str(&filler[..room]);
        pieces.push(Piece {
            data: sse(json!({ "content": text }), None, false),
            marker: Some(m),
        });
    }
    pieces.push(piece(sse(json!({}), Some("stop"), true)));
    pieces.push(piece("data: [DONE]\n\n".into()));
    Plan {
        pieces,
        gap,
        hang: false,
    }
}

fn hang_plan() -> Plan {
    Plan {
        pieces: vec![
            piece(sse(
                json!({"role": "assistant", "content": ""}),
                None,
                false,
            )),
            piece(sse(json!({"content": "partial reply "}), None, false)),
        ],
        gap: Duration::ZERO,
        hang: true,
    }
}

fn tool_plan(n: u64, command: &str) -> Plan {
    let args = json!({ "command": command }).to_string();
    Plan {
        pieces: vec![
            piece(sse(
                json!({"role": "assistant", "content": ""}),
                None,
                false,
            )),
            piece(sse(
                json!({"tool_calls": [{"index": 0, "id": format!("call_{n}"), "type": "function",
                    "function": {"name": "bash", "arguments": args}}]}),
                None,
                false,
            )),
            piece(sse(json!({}), Some("tool_calls"), true)),
            piece("data: [DONE]\n\n".into()),
        ],
        ..Plan::default()
    }
}

fn last_role(body: &Value) -> String {
    body["messages"]
        .as_array()
        .and_then(|m| m.last())
        .and_then(|m| m["role"].as_str())
        .unwrap_or("")
        .to_owned()
}

impl Stub {
    fn start(script: impl Fn(u64, &Value) -> Plan + Send + Sync + 'static) -> Stub {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(StubState {
            script: Mutex::new(Arc::new(script)),
            count: AtomicU64::new(0),
            marks: Mutex::new(HashMap::new()),
            closed: Mutex::new(Vec::new()),
            sse_bytes: AtomicU64::new(0),
            request_bytes: AtomicU64::new(0),
            stop: AtomicBool::new(false),
        });
        let st = state.clone();
        std::thread::Builder::new()
            .name("stub-accept".into())
            .spawn(move || {
                for conn in listener.incoming() {
                    if st.stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(conn) = conn else { continue };
                    let st = st.clone();
                    std::thread::spawn(move || {
                        let _ = handle(conn, &st);
                    });
                }
            })
            .unwrap();
        Stub { state, port }
    }

    fn set_script(&self, script: impl Fn(u64, &Value) -> Plan + Send + Sync + 'static) {
        *self.state.script.lock().unwrap() = Arc::new(script);
    }

    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }
}

fn read_request(s: &mut TcpStream) -> std::io::Result<(String, Vec<u8>)> {
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
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let method = head.split_whitespace().next().unwrap_or("").to_owned();
    let header = |name: &str| {
        head.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case(name)
                .then(|| v.trim().to_owned())
        })
    };
    let mut body = buf[head_end..].to_vec();
    if let Some(len) = header("content-length").and_then(|v| v.parse::<usize>().ok()) {
        while body.len() < len {
            let n = s.read(&mut tmp)?;
            if n == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            body.extend_from_slice(&tmp[..n]);
        }
    } else if header("transfer-encoding").is_some_and(|v| v.contains("chunked")) {
        // Decode a chunked request body.
        let mut raw = body;
        let mut out = Vec::new();
        loop {
            let Some(p) = raw.windows(2).position(|w| w == b"\r\n") else {
                let n = s.read(&mut tmp)?;
                if n == 0 {
                    return Err(std::io::ErrorKind::UnexpectedEof.into());
                }
                raw.extend_from_slice(&tmp[..n]);
                continue;
            };
            let size = usize::from_str_radix(
                String::from_utf8_lossy(&raw[..p])
                    .split(';')
                    .next()
                    .unwrap()
                    .trim(),
                16,
            )
            .unwrap_or(0);
            while raw.len() < p + 2 + size + 2 {
                let n = s.read(&mut tmp)?;
                if n == 0 {
                    return Err(std::io::ErrorKind::UnexpectedEof.into());
                }
                raw.extend_from_slice(&tmp[..n]);
            }
            if size == 0 {
                break;
            }
            out.extend_from_slice(&raw[p + 2..p + 2 + size]);
            raw.drain(..p + 2 + size + 2);
        }
        body = out;
    }
    Ok((method, body))
}

fn write_chunk(s: &mut TcpStream, data: &[u8]) -> std::io::Result<()> {
    let mut out = format!("{:x}\r\n", data.len()).into_bytes();
    out.extend_from_slice(data);
    out.extend_from_slice(b"\r\n");
    s.write_all(&out)
}

fn handle(mut s: TcpStream, st: &StubState) -> std::io::Result<()> {
    s.set_nodelay(true)?;
    let (method, body) = read_request(&mut s)?;
    st.request_bytes
        .fetch_add(body.len() as u64, Ordering::Relaxed);
    if method == "GET" {
        let b =
            json!({"object": "list", "data": [{"id": "scripted", "object": "model"}]}).to_string();
        write!(
            s,
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{b}",
            b.len()
        )?;
        return Ok(());
    }
    let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let n = st.count.fetch_add(1, Ordering::SeqCst);
    let script = st.script.lock().unwrap().clone();
    let plan = script(n, &parsed);
    s.write_all(
        b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n",
    )?;
    for (i, p) in plan.pieces.iter().enumerate() {
        if i > 0 && !plan.gap.is_zero() {
            std::thread::sleep(plan.gap);
        }
        write_chunk(&mut s, p.data.as_bytes())?;
        let at = Instant::now();
        st.sse_bytes
            .fetch_add(p.data.len() as u64, Ordering::Relaxed);
        if let Some(m) = p.marker {
            st.marks.lock().unwrap().insert(m, at);
        }
    }
    if plan.hang {
        s.set_read_timeout(Some(Duration::from_millis(2)))?;
        let mut last = Instant::now();
        let mut buf = [0u8; 1024];
        loop {
            if st.stop.load(Ordering::SeqCst) {
                return Ok(());
            }
            match s.read(&mut buf) {
                Ok(0) => {
                    st.closed.lock().unwrap().push(Instant::now());
                    return Ok(());
                }
                Ok(_) => {}
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => {
                    st.closed.lock().unwrap().push(Instant::now());
                    return Ok(());
                }
            }
            if last.elapsed() >= Duration::from_millis(200) {
                if write_chunk(&mut s, b": keep-alive\n\n").is_err() {
                    st.closed.lock().unwrap().push(Instant::now());
                    return Ok(());
                }
                last = Instant::now();
            }
        }
    }
    s.write_all(b"0\r\n\r\n")?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Engine worker client
// ---------------------------------------------------------------------------------------------

struct Profile {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    data: PathBuf,
    config: PathBuf,
    project: PathBuf,
    home: PathBuf,
}

fn git(dir: &Path, home: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args([
            "-c",
            "user.name=Pipkin Bench",
            "-c",
            "user.email=bench@example.invalid",
            "-c",
            "init.defaultBranch=main",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .expect("git");
    assert!(status.success(), "git {args:?}");
}

impl Profile {
    /// Fresh data/config/project/home directories; `models.json` points a custom
    /// `openai-completions` provider at `base_url` (the same shape as the reference baseline).
    fn new(name: &str, base_url: &str) -> Profile {
        let tmp = tempfile::Builder::new()
            .prefix(&format!("perf-{name}-"))
            .tempdir_in(scratch_root())
            .unwrap();
        let root = tmp.path().to_path_buf();
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
        std::fs::write(
            p.config.join("models.json"),
            serde_json::to_string_pretty(&json!({
                "providers": { "stub": {
                    "baseUrl": base_url,
                    "api": "openai-completions",
                    "apiKey": "stub",
                    "models": [
                        { "id": "scripted", "name": "Scripted stub", "input": ["text", "image"] },
                        { "id": "scripted-think", "name": "Scripted stub (reasoning)", "reasoning": true, "input": ["text"] }
                    ]
                }}
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(p.project.join("README.md"), "# Fixture project\n").unwrap();
        git(&p.project, &p.home, &["init", "-q"]);
        git(&p.project, &p.home, &["add", "-A"]);
        git(&p.project, &p.home, &["commit", "-q", "-m", "initial"]);
        p
    }

    fn db_bytes(&self) -> (u64, u64) {
        let size = |n: &str| {
            std::fs::metadata(self.data.join(n))
                .map(|m| m.len())
                .unwrap_or(0)
        };
        (size("engine.sqlite"), size("engine.sqlite-wal"))
    }
}

struct Arrived {
    at: Instant,
    len: usize,
    frame: ServerFrame,
    markers: Vec<u64>,
}

#[derive(Default)]
struct RxStats {
    frames: AtomicU64,
    bytes: AtomicU64,
    deltas: AtomicU64,
    max_frame: AtomicU64,
}

struct Engine {
    child: Child,
    pid: u32,
    spawned: Instant,
    input: Option<BufWriter<ChildStdin>>,
    rx: mpsc::Receiver<Arrived>,
    backlog: VecDeque<Arrived>,
    next: u64,
    stats: Arc<RxStats>,
}

fn markers_in(text: &str) -> Vec<u64> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 11 <= b.len() {
        if b[i] == b'#' && b[i + 10] == b';' && b[i + 1..i + 10].iter().all(u8::is_ascii_digit) {
            out.push(text[i + 1..i + 10].parse().unwrap());
            i += 11;
        } else {
            i += 1;
        }
    }
    out
}

impl Engine {
    fn spawn(p: &Profile) -> Engine {
        Engine::spawn_with(p, &[])
    }

    fn spawn_with(p: &Profile, extra_env: &[(&str, &str)]) -> Engine {
        let log = std::fs::File::create(p.root.join("engine.log")).unwrap();
        let spawned = Instant::now();
        let mut child = Command::new(env!("CARGO_BIN_EXE_pipkin-engine"))
            .arg("--data-dir")
            .arg(&p.data)
            .arg("--config-dir")
            .arg(&p.config)
            .args(["--default-model", "stub/scripted"])
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", &p.home)
            .envs(extra_env.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(log))
            .spawn()
            .expect("spawn pipkin-engine");
        let pid = child.id();
        let input = BufWriter::new(child.stdin.take().unwrap());
        let mut output = BufReader::with_capacity(1 << 16, child.stdout.take().unwrap());
        let stats = Arc::new(RxStats::default());
        let (tx, rx) = mpsc::channel();
        let st = stats.clone();
        std::thread::Builder::new()
            .name("perf-reader".into())
            .spawn(move || {
                loop {
                    let mut header = [0u8; 4];
                    if output.read_exact(&mut header).is_err() {
                        break;
                    }
                    let len = u32::from_be_bytes(header) as usize;
                    let mut body = vec![0u8; len];
                    if output.read_exact(&mut body).is_err() {
                        break;
                    }
                    let at = Instant::now();
                    st.frames.fetch_add(1, Ordering::Relaxed);
                    st.bytes.fetch_add(len as u64 + 4, Ordering::Relaxed);
                    st.max_frame.fetch_max(len as u64 + 4, Ordering::Relaxed);
                    let frame: ServerFrame = serde_json::from_slice(&body).expect("decode frame");
                    let mut markers = Vec::new();
                    if let ServerFrame::Event {
                        event:
                            Event::Session {
                                change: SessionChange::EntryDelta { delta, .. },
                                ..
                            },
                        ..
                    } = &frame
                    {
                        st.deltas.fetch_add(1, Ordering::Relaxed);
                        if let Delta::Text { text, .. } = delta {
                            markers = markers_in(text);
                        }
                    }
                    if tx
                        .send(Arrived {
                            at,
                            len: len + 4,
                            frame,
                            markers,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .unwrap();
        Engine {
            child,
            pid,
            spawned,
            input: Some(input),
            rx,
            backlog: VecDeque::new(),
            next: 0,
            stats,
        }
    }

    fn send(&mut self, call: Call) -> u64 {
        self.next += 1;
        let id = self.next;
        let input = self.input.as_mut().expect("engine input open");
        frame::write(
            input,
            &ClientFrame {
                id,
                lane: call.lane(),
                call,
            },
        )
        .expect("write frame");
        id
    }

    fn recv(&mut self, deadline: Instant) -> Arrived {
        if let Some(a) = self.backlog.pop_front() {
            return a;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        match self.rx.recv_timeout(left) {
            Ok(a) => a,
            Err(e) => panic!(
                "engine {}: no frame before the deadline ({e}); see engine.log",
                self.pid
            ),
        }
    }

    /// Wait for the reply to `id`; events seen meanwhile stay queued for later waits.
    fn reply(&mut self, id: u64) -> (Result<Reply, ProtoError>, Instant, usize) {
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut held = VecDeque::new();
        let mut from_backlog = std::mem::take(&mut self.backlog);
        loop {
            let a = match from_backlog.pop_front() {
                Some(a) => a,
                None => self.recv(deadline),
            };
            match &a.frame {
                ServerFrame::Reply { id: got, result } if *got == id => {
                    held.extend(from_backlog);
                    self.backlog = held;
                    return (result.clone(), a.at, a.len);
                }
                _ => held.push_back(a),
            }
        }
    }

    fn call(&mut self, call: Call) -> Reply {
        let what = format!("{call:?}");
        let id = self.send(call);
        match self.reply(id).0 {
            Ok(r) => r,
            Err(e) => panic!("{}: {e}", &what[..what.len().min(80)]),
        }
    }

    /// Consume events until `pred` matches; every consumed frame is shown to `seen` first.
    fn until(
        &mut self,
        timeout: Duration,
        mut seen: impl FnMut(&Arrived),
        mut pred: impl FnMut(&Arrived) -> bool,
    ) -> Arrived {
        let deadline = Instant::now() + timeout;
        loop {
            let a = self.recv(deadline);
            seen(&a);
            if pred(&a) {
                return a;
            }
        }
    }

    fn settled(&mut self, run: u64, seen: impl FnMut(&Arrived)) -> (RunOutcome, Instant) {
        let a = self.until(Duration::from_secs(300), seen, |a| {
            matches!(&a.frame, ServerFrame::Event { event: Event::Session {
                change: SessionChange::RunSettled { run: r, .. }, .. }, .. } if *r == run)
        });
        match a.frame {
            ServerFrame::Event {
                event:
                    Event::Session {
                        change: SessionChange::RunSettled { outcome, .. },
                        ..
                    },
                ..
            } => (outcome, a.at),
            _ => unreachable!(),
        }
    }

    fn hello(&mut self) -> Instant {
        let id = self.send(Call::Hello {
            protocol: PROTOCOL_VERSION,
            client: "perf".into(),
        });
        let (r, at, _) = self.reply(id);
        assert!(matches!(r, Ok(Reply::Welcome(_))), "{r:?}");
        at
    }

    fn create(&mut self, p: &Profile) -> SessionId {
        match self.call(Call::CreateSession {
            cwd: p.project.display().to_string(),
            request: "create-1".into(),
        }) {
            Reply::Session { summary } => summary.id,
            other => panic!("{other:?}"),
        }
    }

    fn open(&mut self, session: &str, window: Option<u32>) -> (SessionSnapshot, Duration, usize) {
        let t0 = Instant::now();
        let id = self.send(Call::OpenSession {
            session: session.into(),
            window,
        });
        let (r, at, len) = self.reply(id);
        match r {
            Ok(Reply::Snapshot(s)) => (*s, at - t0, len),
            other => panic!("{other:?}"),
        }
    }

    /// Admit a prompt; returns its run.
    fn prompt(&mut self, session: &str, key: &str, text: &str) -> u64 {
        match self.call(Call::Prompt {
            session: session.into(),
            request: key.into(),
            text: text.into(),
            images: vec![],
            model: None,
        }) {
            Reply::Admitted { run: Some(run), .. } => run,
            other => panic!("{other:?}"),
        }
    }

    /// Prompt and wait until the run settles; returns (outcome, latency).
    fn turn(&mut self, session: &str, key: &str, text: &str) -> (RunOutcome, Duration) {
        let t0 = Instant::now();
        let run = self.prompt(session, key, text);
        let (outcome, at) = self.settled(run, |_| {});
        self.backlog.clear();
        (outcome, at - t0)
    }

    /// Close stdin (orderly exit) and wait; returns the exit duration.
    fn shutdown(mut self) -> Duration {
        let t0 = Instant::now();
        drop(self.input.take());
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "engine exit {status:?}");
                return t0.elapsed();
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                panic!("engine did not exit within 30 s of closing stdin");
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if self.input.is_some() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn is_delta(a: &Arrived) -> bool {
    matches!(
        &a.frame,
        ServerFrame::Event {
            event: Event::Session {
                change: SessionChange::EntryDelta { .. },
                ..
            },
            ..
        }
    )
}

/// The text of a finalized assistant entry carried by an `EntryReplaced`/`EntryAppended`.
fn final_text(a: &Arrived) -> Option<String> {
    let ServerFrame::Event {
        event: Event::Session { change, .. },
        ..
    } = &a.frame
    else {
        return None;
    };
    let entry = match change {
        SessionChange::EntryReplaced { entry } | SessionChange::EntryAppended { entry } => entry,
        _ => return None,
    };
    match &entry.kind {
        EntryKind::Message {
            message: message @ Message::Assistant { .. },
            partial: false,
            ..
        } => Some(message.text()),
        _ => None,
    }
}

fn header(test: &str) -> Value {
    json!({
        "test": test,
        "engine": env!("CARGO_BIN_EXE_pipkin-engine"),
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "scratch": scratch_root().display().to_string(),
        "loadavgStart": loadavg(),
        "unixTime": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
    })
}

fn finish(mut out: Value, name: &str) {
    out["loadavgEnd"] = json!(loadavg());
    write_json(name, &out);
}

fn soak_script() -> impl Fn(u64, &Value) -> Plan + Send + Sync + 'static {
    |n, _| text_plan(&format!("answer {}: {}", n + 1, "words ".repeat(40)))
}

// ---------------------------------------------------------------------------------------------
// 1. Readiness and idle memory
// ---------------------------------------------------------------------------------------------

#[test]
#[ignore = "benchmark: run with --release --ignored"]
fn readiness() {
    let _g = serial();
    let runs = env_usize("PERF_RUNS", 20);
    let warmups = 2;
    let stub = Stub::start(|_, _| text_plan("ready"));
    let mut rows = Vec::new();
    for i in 0..warmups + runs {
        let p = Profile::new("ready", &stub.base_url());
        let mut e = Engine::spawn(&p);
        let welcome = e.hello() - e.spawned;
        let session = e.create(&p);
        let created = Instant::now() - e.spawned;
        let (snap, _, _) = e.open(&session, None);
        assert!(snap.entries.is_empty());
        let models = e.call(Call::Models {
            session: Some(session.clone()),
        });
        let selected = match models {
            Reply::Models(m) => m.selected,
            _ => None,
        };
        let ready = Instant::now() - e.spawned;
        let (outcome, first) = e.turn(&session, "k1", "ping");
        assert_eq!(outcome, RunOutcome::Completed, "first prompt");
        std::thread::sleep(Duration::from_secs(2));
        let idle = mem_sample(e.pid);
        let children = procfs::descendants(e.pid);
        let exit = e.shutdown();
        // Restart on the profile just used (the usual desktop start: existing database).
        let mut e = Engine::spawn(&p);
        let welcome_existing = e.hello() - e.spawned;
        let (snap, _, _) = e.open(&session, None);
        // Native persists the effective system context as well as the user/reply pair.
        // Check identities/content so an unexpected duplicate never passes as metadata.
        let [prompt, context, answer] = snap.entries.as_slice() else {
            panic!("expected one prompt, one system snapshot and one reply after restart");
        };
        assert!(matches!(&prompt.kind, EntryKind::Message {
            message: message @ Message::User { .. }, request: Some(key), partial: false, ..
        } if message.text() == "ping" && key == "k1"));
        assert!(matches!(
            &context.kind,
            EntryKind::Message {
                message: Message::System(_),
                partial: false,
                ..
            }
        ));
        assert!(matches!(&answer.kind, EntryKind::Message {
            message: message @ Message::Assistant { stop_reason: StopReason::Stop, .. }, partial: false, ..
        } if message.text() == "ready"));
        let ready_existing = Instant::now() - e.spawned;
        let _ = e.shutdown();
        // Floor: exec + dynamic linking + argument parsing, no engine start.
        let t0 = Instant::now();
        let v = Command::new(env!("CARGO_BIN_EXE_pipkin-engine"))
            .arg("--version")
            .output()
            .unwrap();
        assert!(v.status.success());
        let version_exec = t0.elapsed();
        let row = json!({
            "run": i,
            "warmup": i < warmups,
            "welcomeMs": ms(welcome),
            "sessionCreatedMs": ms(created),
            "promptReadyMs": ms(ready),
            "welcomeExistingProfileMs": ms(welcome_existing),
            "sessionOpenExistingProfileMs": ms(ready_existing),
            "versionExecMs": ms(version_exec),
            "firstPromptRoundTripMs": ms(first),
            "selectedModel": selected,
            "idle": idle,
            "childrenBeforeExit": children.len(),
            "exitAfterStdinCloseMs": ms(exit),
        });
        println!("{row}");
        rows.push(row);
    }
    let measured: Vec<&Value> = rows.iter().filter(|r| r["warmup"] == false).collect();
    let col = |k: &str| -> Vec<f64> { measured.iter().map(|r| r[k].as_f64().unwrap()).collect() };
    let idle = |k: &str| -> Vec<f64> {
        measured
            .iter()
            .map(|r| r["idle"][k].as_f64().unwrap())
            .collect()
    };
    let mut out = header("readiness");
    out["summary"] = json!({
        "runs": measured.len(),
        "warmupsDiscarded": warmups,
        "welcomeMs": summary(&col("welcomeMs")),
        "sessionCreatedMs": summary(&col("sessionCreatedMs")),
        "promptReadyMs": summary(&col("promptReadyMs")),
        "welcomeExistingProfileMs": summary(&col("welcomeExistingProfileMs")),
        "sessionOpenExistingProfileMs": summary(&col("sessionOpenExistingProfileMs")),
        "versionExecMs": summary(&col("versionExecMs")),
        "firstPromptRoundTripMs": summary(&col("firstPromptRoundTripMs")),
        "idleRssKiB": summary(&idle("rssKiB")),
        "idlePssKiB": summary(&idle("pssKiB")),
        "idleThreads": summary(&idle("threads")),
        "idleFds": summary(&idle("fds")),
        "idleChildren": summary(&idle("children")),
        "exitAfterStdinCloseMs": summary(&col("exitAfterStdinCloseMs")),
    });
    println!("{}", serde_json::to_string_pretty(&out["summary"]).unwrap());
    out["rows"] = json!(rows);
    finish(out, "readiness");
}

// ---------------------------------------------------------------------------------------------
// 2. Soak: memory, fds, processes over many scripted text turns
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct SoakOpts<'a> {
    /// Start a new session every this many turns (0: one session throughout).
    new_session_every: usize,
    /// Extra environment for the engine process (allocator diagnostics).
    engine_env: &'a [(&'a str, &'a str)],
}

fn soak(name: &str, warmup: usize, turns: usize, every: usize) {
    soak_with(name, warmup, turns, every, SoakOpts::default());
}

fn soak_with(name: &str, warmup: usize, turns: usize, every: usize, opts: SoakOpts) {
    let stub = Stub::start(soak_script());
    let p = Profile::new(name, &stub.base_url());
    let mut e = Engine::spawn_with(&p, opts.engine_env);
    e.hello();
    let mut session = e.create(&p);
    e.open(&session, None);
    std::thread::sleep(Duration::from_secs(2));
    let pid = e.pid;
    let mut samples = vec![json!({ "turn": 0, "phase": "idle-before", "mem": mem_sample(pid) })];
    // 50 ms background sampler: peak RSS and the most processes (engine + descendants).
    let stop = Arc::new(AtomicBool::new(false));
    let sampler = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let (mut peak, mut procs, mut fds) = (0u64, 0usize, 0u64);
            while !stop.load(Ordering::SeqCst) {
                peak = peak.max(procfs::rss_kib(pid));
                procs = procs.max(1 + procfs::descendants(pid).len());
                fds = fds.max(procfs::fds(pid));
                std::thread::sleep(Duration::from_millis(50));
            }
            (peak, procs, fds)
        })
    };
    let total = warmup + turns;
    let mut lat = Vec::new();
    let cpu0 = procfs::cpu_secs(pid);
    let mut cpu_warm = cpu0;
    let t_all = Instant::now();
    for t in 1..=total {
        if opts.new_session_every > 0 && t > 1 && (t - 1) % opts.new_session_every == 0 {
            session = match e.call(Call::CreateSession {
                cwd: p.project.display().to_string(),
                request: format!("create-{t}"),
            }) {
                Reply::Session { summary } => summary.id,
                other => panic!("{other:?}"),
            };
        }
        let (outcome, d) = e.turn(&session, &format!("k{t}"), &format!("prompt {t}"));
        assert_eq!(outcome, RunOutcome::Completed, "turn {t}");
        if t > warmup {
            lat.push(ms(d));
        }
        if t == warmup {
            cpu_warm = procfs::cpu_secs(pid);
        }
        if t == 1 || t % every == 0 || t == warmup {
            let m = mem_sample(pid);
            println!(
                "{name} turn {t}: rss {} KiB pss {} KiB fds {} threads {} turn {:.1} ms",
                m["rssKiB"],
                m["pssKiB"],
                m["fds"],
                m["threads"],
                ms(d)
            );
            samples.push(json!({ "turn": t, "mem": m, "turnMs": ms(d) }));
        }
    }
    let wall = t_all.elapsed();
    let cpu_end = procfs::cpu_secs(pid);
    stop.store(true, Ordering::SeqCst);
    let (peak, max_procs, max_fds) = sampler.join().unwrap();
    std::thread::sleep(Duration::from_secs(2));
    samples.push(json!({ "turn": total, "phase": "idle-after", "mem": mem_sample(pid) }));
    let (db, wal) = p.db_bytes();
    let (snap, open_d, open_len) = e.open(&session, None);
    let exit = e.shutdown();
    let get = |s: &Value, k: &str| s["mem"][k].as_f64().unwrap_or(0.0);
    let measured: Vec<&Value> = samples
        .iter()
        .filter(|s| s["turn"].as_u64().unwrap() as usize >= warmup && s.get("phase").is_none())
        .collect();
    let pts = |k: &str| -> Vec<(f64, f64)> {
        measured
            .iter()
            .map(|s| (s["turn"].as_f64().unwrap(), get(s, k)))
            .collect()
    };
    let half: Vec<(f64, f64)> = pts("rssKiB")
        .into_iter()
        .filter(|(t, _)| *t as usize >= warmup + turns / 2)
        .collect();
    let first = &samples[0];
    let last_turn = &samples[samples.len() - 2];
    let after = &samples[samples.len() - 1];
    let mut out = header(name);
    out["summary"] = json!({
        "warmupTurns": warmup,
        "measuredTurns": turns,
        "newSessionEvery": opts.new_session_every,
        "engineEnv": opts.engine_env.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>(),
        "reply": "answer <n>: + \"words \" x 40 (no tools), as the reference soak",
        "rssKiBIdleBefore": get(first, "rssKiB"),
        "pssKiBIdleBefore": get(first, "pssKiB"),
        "rssKiBAfterTurn1": get(&samples[1], "rssKiB"),
        "rssKiBAfterLastTurn": get(last_turn, "rssKiB"),
        "pssKiBAfterLastTurn": get(last_turn, "pssKiB"),
        "rssKiBIdleAfter": get(after, "rssKiB"),
        "pssKiBIdleAfter": get(after, "pssKiB"),
        "rssKiBPeakSampled50ms": peak,
        "vmHwmKiB": get(after, "hwmKiB"),
        "maxProcessesSeen": max_procs,
        "maxFdsSeen": max_fds,
        "fdsIdleBefore": get(first, "fds"),
        "fdsIdleAfter": get(after, "fds"),
        "threadsIdleAfter": get(after, "threads"),
        "rssSlopeKiBPerTurnMeasured": round3(slope(&pts("rssKiB"))),
        "rssSlopeKiBPerTurnSecondHalf": round3(slope(&half)),
        "pssSlopeKiBPerTurnMeasured": round3(slope(&pts("pssKiB"))),
        "fdSlopePerTurnMeasured": round3(slope(&pts("fds"))),
        "turnLatencyMs": summary(&lat),
        "wallSecs": round3(wall.as_secs_f64()),
        "engineCpuSecsTotal": round3(cpu_end - cpu0),
        "engineCpuMsPerMeasuredTurn": round3((cpu_end - cpu_warm) * 1e3 / turns as f64),
        "dbBytes": db,
        "walBytes": wal,
        "providerRequestBytesTotal": stub.state.request_bytes.load(Ordering::Relaxed),
        "openSessionAfterSoak": { "ms": ms(open_d), "replyBytes": open_len, "entries": snap.entries.len(), "hasOlder": snap.has_older },
        "exitAfterStdinCloseMs": ms(exit),
    });
    println!("{}", serde_json::to_string_pretty(&out["summary"]).unwrap());
    out["samples"] = json!(samples);
    finish(out, name);
    // Plan §7's native worker memory budget, with the original 100 + 1500 turn workload.
    // Keep this a release/ignored qualification test, not a timing-sensitive default unit test.
    if name == "soak-1500" && warmup == 100 && turns == 1500 {
        assert!(
            get(after, "rssKiB") <= 100.0 * 1024.0,
            "idle RSS exceeds 100 MiB"
        );
        assert!(
            get(after, "hwmKiB") <= 100.0 * 1024.0,
            "peak RSS exceeds 100 MiB"
        );
        assert_eq!(
            get(after, "fds"),
            get(first, "fds"),
            "file descriptors grew"
        );
        assert_eq!(max_procs, 1, "text soak created descendant processes");
    }
}

#[test]
#[ignore = "benchmark: run with --release --ignored"]
fn soak_200() {
    let _g = serial();
    soak("soak-200", 0, env_usize("PERF_SOAK_TURNS", 200), 10);
}

#[test]
#[ignore = "benchmark: run with --release --ignored"]
fn soak_1500() {
    let _g = serial();
    soak(
        "soak-1500",
        100,
        env_usize("PERF_LONG_SOAK_TURNS", 1500),
        50,
    );
}

/// Diagnostics for the soak's RSS growth: is it retained per turn regardless of context size
/// (a leak), or does it follow the growing context (allocator retention of ever larger
/// transient buffers)?
#[test]
#[ignore = "benchmark: run with --release --ignored"]
fn soak_diagnostic() {
    let _g = serial();
    let turns = env_usize("PERF_LONG_SOAK_TURNS", 1500);
    // Same workload, but a new session every 10 turns: the context stays small.
    soak_with(
        "soak-diag-new-session-every-10",
        100,
        turns,
        50,
        SoakOpts {
            new_session_every: 10,
            ..SoakOpts::default()
        },
    );
    // The single-session workload with glibc's dynamic mmap threshold pinned (allocations of
    // 128 KiB and more are mmapped and returned on free) and one malloc arena.
    soak_with(
        "soak-diag-glibc-fixed-mmap-threshold",
        100,
        turns,
        50,
        SoakOpts {
            engine_env: &[(
                "GLIBC_TUNABLES",
                "glibc.malloc.mmap_threshold=131072:glibc.malloc.arena_max=1",
            )],
            ..SoakOpts::default()
        },
    );
}

// ---------------------------------------------------------------------------------------------
// 3. Streaming overhead: chunk written by the provider -> committed EntryDelta at the client
// ---------------------------------------------------------------------------------------------

#[test]
#[ignore = "benchmark: run with --release --ignored"]
fn stream_latency() {
    let _g = serial();
    let stub = Stub::start(|_, _| text_plan("warm"));
    let p = Profile::new("latency", &stub.base_url());
    let mut e = Engine::spawn(&p);
    e.hello();
    let session = e.create(&p);
    e.open(&session, None);
    let (o, _) = e.turn(&session, "warm", "warm up");
    assert_eq!(o, RunOutcome::Completed);
    // (gap between chunks, chunk count, bytes per chunk)
    let cadences: [(u64, u64, usize); 5] = [
        (0, 2000, 32),
        (2, 1000, 32),
        (5, 600, 32),
        (20, 250, 32),
        (100, 60, 32),
    ];
    let mut results = Vec::new();
    let mut first = 0u64;
    for (k, (gap, count, bytes)) in cadences.into_iter().enumerate() {
        let base = first;
        stub.set_script(move |_, _| marked_plan(count, bytes, Duration::from_millis(gap), base));
        stub.state.marks.lock().unwrap().clear();
        let mut arrivals: HashMap<u64, Instant> = HashMap::new();
        let mut delta_frames = 0u64;
        let t0 = Instant::now();
        let run = e.prompt(&session, &format!("lat{k}"), "stream please");
        let mut via_final = 0u64;
        let (outcome, done) = e.settled(run, |a| {
            if is_delta(a) {
                delta_frames += 1;
            }
            for m in &a.markers {
                arrivals.entry(*m).or_insert(a.at);
            }
            // Deltas still pending when the stream ends are not published separately: the
            // final committed entry carries them.
            if let Some(text) = final_text(a) {
                for m in markers_in(&text) {
                    arrivals.entry(m).or_insert_with(|| {
                        via_final += 1;
                        a.at
                    });
                }
            }
        });
        e.backlog.clear();
        assert_eq!(outcome, RunOutcome::Completed);
        let marks = stub.state.marks.lock().unwrap().clone();
        let mut lat = Vec::new();
        let mut missing = 0;
        for m in base..base + count {
            match (marks.get(&m), arrivals.get(&m)) {
                (Some(w), Some(a)) => lat.push(ms(a.saturating_duration_since(*w))),
                _ => missing += 1,
            }
        }
        assert_eq!(missing, 0, "stream markers were lost at {gap} ms cadence");
        let row = json!({
            "gapMs": gap,
            "chunks": count,
            "bytesPerChunk": bytes,
            "chunkToCommittedDeltaMs": summary(&lat),
            "missingMarkers": missing,
            "markersFirstSeenInFinalEntry": via_final,
            "deltaFrames": delta_frames,
            "promptToSettledMs": ms(done - t0),
        });
        println!("{row}");
        results.push(row);
        first += count;
    }
    let _ = e.shutdown();
    let mut out = header("stream-latency");
    out["note"] = json!(
        "engine flush policy at this revision: synchronous commit backpressure at 16 KiB or 128 deltas, otherwise 40 ms from the oldest pending delta (runtime/run/progress.rs); every commit is a WAL transaction with synchronous=FULL"
    );
    out["cadences"] = json!(results);
    finish(out, "stream-latency");
}

// ---------------------------------------------------------------------------------------------
// 4. Streaming scaling: 1/2/4/8/10 MiB single replies
// ---------------------------------------------------------------------------------------------

fn scaling_case(stub: &Stub, mib: f64, chunk: usize) -> Value {
    let bytes = (mib * 1024.0 * 1024.0) as u64;
    let count = bytes / chunk as u64;
    stub.set_script(|_, _| text_plan("warm"));
    let p = Profile::new("scale", &stub.base_url());
    let mut e = Engine::spawn(&p);
    e.hello();
    let session = e.create(&p);
    e.open(&session, None);
    let (o, _) = e.turn(&session, "warm", "warm up");
    assert_eq!(o, RunOutcome::Completed);
    std::thread::sleep(Duration::from_millis(200));
    stub.set_script(move |_, _| marked_plan(count, chunk, Duration::ZERO, 0));
    let sse0 = stub.state.sse_bytes.load(Ordering::Relaxed);
    let (db0, wal0) = p.db_bytes();
    let io0 = procfs::io(e.pid);
    let cpu0 = procfs::cpu_secs(e.pid);
    let rss0 = procfs::rss_kib(e.pid);
    let frames0 = e.stats.frames.load(Ordering::Relaxed);
    let bytes0 = e.stats.bytes.load(Ordering::Relaxed);
    let deltas0 = e.stats.deltas.load(Ordering::Relaxed);
    e.stats.max_frame.store(0, Ordering::Relaxed);
    let t0 = Instant::now();
    let run = e.prompt(&session, "big", "stream a big reply");
    let mut first_delta: Option<Instant> = None;
    let (outcome, done) = e.settled(run, |a| {
        if first_delta.is_none() && is_delta(a) {
            first_delta = Some(a.at);
        }
    });
    e.backlog.clear();
    assert_eq!(outcome, RunOutcome::Completed);
    let wall = done - t0;
    let cpu = procfs::cpu_secs(e.pid) - cpu0;
    let io1 = procfs::io(e.pid);
    let (db1, wal1) = p.db_bytes();
    let st = procfs::status(e.pid);
    let client_bytes = e.stats.bytes.load(Ordering::Relaxed) - bytes0;
    let client_frames = e.stats.frames.load(Ordering::Relaxed) - frames0;
    let client_deltas = e.stats.deltas.load(Ordering::Relaxed) - deltas0;
    let max_frame = e.stats.max_frame.load(Ordering::Relaxed);
    let d = |k: &str| {
        io1.get(k)
            .unwrap_or(&0)
            .saturating_sub(*io0.get(k).unwrap_or(&0))
    };
    let row_before_exit = json!({
        "rssKiBBefore": rss0,
        "rssKiBAfter": st.get("VmRSS"),
        "vmHwmKiB": st.get("VmHWM"),
    });
    let (snap, open_d, open_len) = e.open(&session, None);
    let final_len: usize = snap
        .entries
        .iter()
        .filter_map(|en| match &en.kind {
            EntryKind::Message {
                message: Message::Assistant { content, .. },
                ..
            } => Some(
                content
                    .iter()
                    .map(|b| match b {
                        Block::Text { text, .. } => text.len(),
                        _ => 0,
                    })
                    .sum::<usize>(),
            ),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    assert_eq!(
        final_len as u64, bytes,
        "final stored output must be complete"
    );
    let after_open = mem_sample(e.pid);
    let _ = e.shutdown();
    // After an orderly exit: the database alone.
    let conn = rusqlite::Connection::open_with_flags(
        p.data.join("engine.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let q = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
    let page_size = q("PRAGMA page_size");
    let pages = q("PRAGMA page_count");
    let free = q("PRAGMA freelist_count");
    let chunks_left = q("SELECT COUNT(*) FROM entry_chunks");
    assert_eq!(chunks_left, 0, "finalized stream chunks must be released");
    let (db2, wal2) = p.db_bytes();
    json!({
        "mib": mib,
        "chunkBytes": chunk,
        "chunks": count,
        "replyTextBytes": bytes,
        "finalEntryTextBytes": final_len,
        "wallMs": ms(wall),
        "firstDeltaMs": first_delta.map(|f| ms(f - t0)),
        "engineCpuSecs": round3(cpu),
        "providerSseBytes": stub.state.sse_bytes.load(Ordering::Relaxed) - sse0,
        "clientFrames": client_frames,
        "clientBytes": client_bytes,
        "clientDeltaFrames": client_deltas,
        "clientLargestFrameBytes": max_frame,
        "io": {
            "wcharBytes": d("wchar"),
            "wcharMinusClientBytes": d("wchar").saturating_sub(client_bytes),
            "writeBytes": d("write_bytes"),
            "syscw": d("syscw"),
            "rcharBytes": d("rchar"),
        },
        "dbFileBytesBefore": db0, "walBytesBefore": wal0,
        "dbFileBytesAfter": db1, "walBytesAfter": wal1,
        "dbFileBytesAfterExit": db2, "walBytesAfterExit": wal2,
        "dbPagesAfterExit": pages, "dbPageSize": page_size, "dbFreelistPages": free,
        "entryChunksLeft": chunks_left,
        "memory": row_before_exit,
        "openSessionAfter": { "ms": ms(open_d), "replyBytes": open_len, "memory": after_open },
    })
}

#[test]
#[ignore = "benchmark: run with --release --ignored"]
fn stream_scaling() {
    let _g = serial();
    let reps = env_usize("PERF_SCALING_REPS", 3);
    let stub = Stub::start(|_, _| text_plan("warm"));
    let cases = [
        (1.0, 1024),
        (2.0, 1024),
        (4.0, 1024),
        (8.0, 1024),
        (10.0, 1024),
        (0.25, 64),
        (0.5, 64),
        (1.0, 64),
        (2.0, 64),
    ];
    let mut rows = Vec::new();
    for rep in 0..reps {
        for (mib, chunk) in cases {
            let mut row = scaling_case(&stub, mib, chunk);
            row["rep"] = json!(rep);
            println!("{row}");
            rows.push(row);
        }
    }
    // Per case, the median of each metric over the repetitions.
    let metric = |r: &Value, k: &str| -> f64 {
        k.split('.')
            .fold(r, |v, part| &v[part])
            .as_f64()
            .unwrap_or(0.0)
    };
    let keys = [
        "wallMs",
        "engineCpuSecs",
        "clientBytes",
        "clientFrames",
        "io.wcharMinusClientBytes",
        "io.writeBytes",
        "dbFileBytesAfterExit",
        "memory.vmHwmKiB",
    ];
    let mut medians = Vec::new();
    for (mib, chunk) in cases {
        let same: Vec<&Value> = rows
            .iter()
            .filter(|r| r["mib"].as_f64() == Some(mib) && r["chunkBytes"] == chunk)
            .collect();
        let mut m = json!({ "mib": mib, "chunkBytes": chunk, "reps": same.len() });
        for k in keys {
            let v: Vec<f64> = same.iter().map(|r| metric(r, k)).collect();
            m[k] = json!(round3(pct(&v, 50.0)));
        }
        medians.push(m);
    }
    // Ratios of the medians against the smallest size of the same chunk size: linear work
    // tracks sizeRatio; quadratic work would track sizeRatio squared.
    let mut linearity = Vec::new();
    for chunk in [1024, 64] {
        let group: Vec<&Value> = medians
            .iter()
            .filter(|m| m["chunkBytes"] == chunk)
            .collect();
        let base = group[0];
        for m in &group {
            let mut r = json!({
                "chunkBytes": chunk,
                "mib": m["mib"],
                "sizeRatio": m["mib"].as_f64().unwrap() / base["mib"].as_f64().unwrap(),
            });
            for k in keys {
                r[k] = json!(round3(
                    m[k].as_f64().unwrap() / base[k].as_f64().unwrap().max(1e-9)
                ));
            }
            linearity.push(r);
        }
    }
    let mut out = header("stream-scaling");
    out["method"] = json!(
        "one fresh engine per case; one warm-up turn; then one reply of the given size streamed as fast as the stub can write it (no gaps), chunkBytes of text per SSE chunk; engine CPU from /proc/<pid>/stat utime+stime (10 ms ticks); io from /proc/<pid>/io (wchar includes the stdout frames to the client, so wcharMinusClientBytes approximates database+WAL write syscalls); database size after an orderly exit (WAL checkpointed)"
    );
    out["medians"] = json!(medians);
    out["linearity"] = json!(linearity);
    out["rows"] = json!(rows);
    println!(
        "{}",
        serde_json::to_string_pretty(&out["linearity"]).unwrap()
    );
    finish(out, "stream-scaling");
}

// ---------------------------------------------------------------------------------------------
// 5. History: 1k / 10k / 100k stored entries, OpenSession and HistoryPage
// ---------------------------------------------------------------------------------------------

/// Build a session with `n` committed entries through the public store API (alternating user
/// prompts and assistant replies of realistic size), then close the store.
fn build_history(p: &Profile, n: u64) -> SessionId {
    use pipkin_engine::store::Store;
    let store = Store::open(&p.data).expect("open store");
    let id = format!("perf-history-{n}");
    let now = 1_790_000_000_000i64;
    store
        .create_session(
            &id,
            &p.project.display().to_string(),
            Some("history"),
            &Origin::Native,
            Some("stub/scripted"),
            None,
            None,
            now,
        )
        .expect("create session");
    let words = "words ".repeat(40);
    store
        .tx(|tx| {
            for i in 0..n {
                let turn = i / 2 + 1;
                let at = now + i as i64 * 1000;
                let message = if i % 2 == 0 {
                    Message::user_text(format!("prompt {turn}: please look at this"), at)
                } else {
                    Message::Assistant {
                        content: vec![Block::text(format!("answer {turn}: {words}"))],
                        api: "openai-completions".into(),
                        provider: "stub".into(),
                        model: "scripted".into(),
                        usage: Usage::default(),
                        stop_reason: StopReason::Stop,
                        error_message: None,
                        timestamp: at,
                        extra: Default::default(),
                    }
                };
                let kind = EntryKind::Message {
                    message,
                    mode: InputMode::Prompt,
                    request: (i % 2 == 0).then(|| format!("k{turn}")),
                    partial: false,
                };
                Store::append_entry(tx, &id, at, Some(turn), &kind)?;
            }
            Ok(())
        })
        .expect("insert entries");
    drop(store);
    id
}

fn history_case(stub: &Stub, n: u64) -> Value {
    let p = Profile::new("history", &stub.base_url());
    let t_build = Instant::now();
    let session = build_history(&p, n);
    let build = t_build.elapsed();
    let (db, wal) = p.db_bytes();
    let mut e = Engine::spawn(&p);
    e.hello();
    let (snap, first_open, first_len) = e.open(&session, None);
    assert_eq!(snap.entries.len() as u64, n.min(200));
    let mut opens = Vec::new();
    let mut open_len = 0;
    for _ in 0..20 {
        let (s, d, len) = e.open(&session, None);
        assert_eq!(s.entries.len() as u64, n.min(200));
        opens.push(ms(d));
        open_len = len;
    }
    // Pages at 20 positions spread over the whole history, desktop size (50) and 200.
    let mut page = |limit: u32| -> (Vec<f64>, usize) {
        let mut v = Vec::new();
        let mut bytes = 0;
        for i in 0..20u64 {
            let before = 1 + (n * (i + 1)) / 21;
            let t0 = Instant::now();
            let id = e.send(Call::HistoryPage {
                session: session.clone(),
                before: Some(before.max(2)),
                limit,
            });
            let (r, at, len) = e.reply(id);
            assert!(matches!(r, Ok(Reply::Page { .. })), "{r:?}");
            v.push(ms(at - t0));
            bytes = bytes.max(len);
        }
        (v, bytes)
    };
    let (p50s, b50) = page(50);
    let (p200s, b200) = page(200);
    // Walk the whole history backwards in 500-entry pages.
    let t_walk = Instant::now();
    let mut before = snap.entries.first().map(|en| en.id);
    let mut pages = 0;
    let mut seen = snap.entries.len() as u64;
    let mut walk = Vec::new();
    while let Some(b) = before {
        let t0 = Instant::now();
        match e.call(Call::HistoryPage {
            session: session.clone(),
            before: Some(b),
            limit: 500,
        }) {
            Reply::Page {
                entries, has_older, ..
            } => {
                walk.push(ms(t0.elapsed()));
                pages += 1;
                seen += entries.len() as u64;
                before = if has_older {
                    entries.first().map(|en| en.id)
                } else {
                    None
                };
            }
            other => panic!("{other:?}"),
        }
    }
    let walk_total = t_walk.elapsed();
    assert_eq!(seen, n, "paging reaches every entry exactly once");
    let mem = mem_sample(e.pid);
    let _ = e.shutdown();
    json!({
        "entries": n,
        "buildSecs": round3(build.as_secs_f64()),
        "dbBytes": db, "walBytes": wal,
        "openSessionFirstMs": ms(first_open),
        "openSessionFirstReplyBytes": first_len,
        "openSessionWarmMs": summary(&opens),
        "openSessionReplyBytes": open_len,
        "historyPage50Ms": summary(&p50s),
        "historyPage50MaxReplyBytes": b50,
        "historyPage200Ms": summary(&p200s),
        "historyPage200MaxReplyBytes": b200,
        "fullWalk500": { "pages": pages, "totalMs": ms(walk_total), "perPageMs": summary(&walk) },
        "engineAfter": mem,
    })
}

#[test]
#[ignore = "benchmark: run with --release --ignored"]
fn history() {
    let _g = serial();
    let stub = Stub::start(|_, _| text_plan("unused"));
    let mut rows = Vec::new();
    for n in [1_000, 10_000, 100_000] {
        let row = history_case(&stub, n);
        println!("{row}");
        rows.push(row);
    }
    let mut out = header("history");
    out["method"] = json!(
        "entries inserted with pipkin_engine::store::Store (append_entry in one transaction), store closed, then the worker binary opened over that data dir; window = DEFAULT_WINDOW_ENTRIES (200)"
    );
    out["rows"] = json!(rows);
    finish(out, "history");
}

// ---------------------------------------------------------------------------------------------
// 6. Stop latency: while streaming, and while a bash tool runs
// ---------------------------------------------------------------------------------------------

#[test]
#[ignore = "benchmark: run with --release --ignored"]
fn stop_latency() {
    let _g = serial();
    let runs = env_usize("PERF_RUNS", 20);
    let warmups = 2;
    let stub = Stub::start(|_, _| hang_plan());
    let p = Profile::new("stop", &stub.base_url());
    let mut e = Engine::spawn(&p);
    e.hello();
    let session = e.create(&p);
    e.open(&session, None);

    // (a) While streaming: the provider sent a partial reply and keeps the stream open.
    let mut stream_rows = Vec::new();
    for i in 0..warmups + runs {
        stub.set_script(|_, _| hang_plan());
        let closed_before = stub.state.closed.lock().unwrap().len();
        let run = e.prompt(&session, &format!("s{i}"), "stream then wait");
        e.until(Duration::from_secs(30), |_| {}, is_delta);
        std::thread::sleep(Duration::from_millis(20));
        let t0 = Instant::now();
        let id = e.send(Call::Stop {
            session: session.clone(),
        });
        let (r, reply_at, _) = e.reply(id);
        assert!(matches!(r, Ok(Reply::Stopping { .. })), "{r:?}");
        let (outcome, settled_at) = e.settled(run, |_| {});
        e.backlog.clear();
        let deadline = Instant::now() + Duration::from_secs(5);
        let closed = loop {
            if let Some(c) = stub.state.closed.lock().unwrap().get(closed_before) {
                break Some(*c);
            }
            if Instant::now() > deadline {
                break None;
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        let row = json!({
            "run": i, "warmup": i < warmups,
            "outcome": format!("{outcome:?}"),
            "stopReplyMs": ms(reply_at - t0),
            "stopToSettledMs": ms(settled_at - t0),
            "stopToProviderConnectionClosedMs": closed.map(|c| ms(c.saturating_duration_since(t0))),
        });
        assert_eq!(outcome, RunOutcome::Aborted, "{row}");
        stream_rows.push(row);
    }

    // (b) While `bash` runs `sleep 30`: Stop must kill the process group.
    let mut tool_rows = Vec::new();
    for i in 0..warmups + runs {
        stub.set_script(|n, body| {
            if last_role(body) == "tool" {
                text_plan("done")
            } else {
                tool_plan(n, "sleep 30")
            }
        });
        let run = e.prompt(&session, &format!("t{i}"), "run a slow command");
        let deadline = Instant::now() + Duration::from_secs(30);
        let sleeper = loop {
            if let Some(s) = procfs::descendants(e.pid)
                .into_iter()
                .find(|(_, c)| c == "sleep")
            {
                break s.0;
            }
            assert!(
                Instant::now() < deadline,
                "the bash tool never started sleep"
            );
            std::thread::sleep(Duration::from_millis(1));
        };
        std::thread::sleep(Duration::from_millis(20));
        let t0 = Instant::now();
        let id = e.send(Call::Stop {
            session: session.clone(),
        });
        let (r, reply_at, _) = e.reply(id);
        assert!(matches!(r, Ok(Reply::Stopping { .. })), "{r:?}");
        let (outcome, settled_at) = e.settled(run, |_| {});
        e.backlog.clear();
        let gone_at = {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let alive = procfs::stat(sleeper).is_some_and(|s| s.0 != 'Z');
                if !alive {
                    break Some(Instant::now());
                }
                if Instant::now() > deadline {
                    break None;
                }
                std::thread::sleep(Duration::from_micros(200));
            }
        };
        let left = procfs::descendants(e.pid);
        let row = json!({
            "run": i, "warmup": i < warmups,
            "outcome": format!("{outcome:?}"),
            "stopReplyMs": ms(reply_at - t0),
            "stopToSettledMs": ms(settled_at - t0),
            "stopToToolProcessGoneMs": gone_at.map(|g| ms(g.saturating_duration_since(t0))),
            "toolProcessGoneBeforeSettled": gone_at.is_some_and(|g| g <= settled_at),
            "descendantsAfter": left.len(),
        });
        assert_eq!(outcome, RunOutcome::Aborted, "{row}");
        tool_rows.push(row);
    }
    let _ = e.shutdown();
    let col = |rows: &[Value], k: &str| -> Vec<f64> {
        rows.iter()
            .filter(|r| r["warmup"] == false)
            .filter_map(|r| r[k].as_f64())
            .collect()
    };
    let mut out = header("stop-latency");
    out["summary"] = json!({
        "runs": runs,
        "streaming": {
            "stopReplyMs": summary(&col(&stream_rows, "stopReplyMs")),
            "stopToSettledAbortedMs": summary(&col(&stream_rows, "stopToSettledMs")),
            "stopToProviderConnectionClosedMs": summary(&col(&stream_rows, "stopToProviderConnectionClosedMs")),
        },
        "bashSleepTool": {
            "stopReplyMs": summary(&col(&tool_rows, "stopReplyMs")),
            "stopToSettledAbortedMs": summary(&col(&tool_rows, "stopToSettledMs")),
            "stopToToolProcessGoneMs": summary(&col(&tool_rows, "stopToToolProcessGoneMs")),
            "maxDescendantsAfter": tool_rows.iter().filter_map(|r| r["descendantsAfter"].as_u64()).max(),
        },
    });
    println!("{}", serde_json::to_string_pretty(&out["summary"]).unwrap());
    out["streaming"] = json!(stream_rows);
    out["bashSleepTool"] = json!(tool_rows);
    finish(out, "stop-latency");
}
