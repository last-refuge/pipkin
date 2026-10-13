//! Out-of-process extensions: any program that speaks a small JSON-lines protocol on its
//! standard input and output.
//!
//! # Launch
//!
//! The engine runs the configured argv directly (no shell), in its own process group, with
//! stdin, stdout and stderr piped and `PIPKIN_EXTENSION_API=1` in its environment. The engine
//! owns the whole group: when the extension stops for any reason, every process left in the
//! group is killed. A stopped or crashed extension is never restarted.
//!
//! # Framing
//!
//! One JSON object per line (`\n`, an optional `\r` before it is ignored), UTF-8, in both
//! directions. A line from the extension longer than `max_line_bytes` (1 MiB by default), a
//! line that is not UTF-8 or not a JSON object, or an object that is neither a request nor a
//! known message, is a protocol violation: the extension is stopped and an error notice says
//! why. Blank lines are ignored. Stderr is not part of the protocol; its last line is quoted in
//! the notice when the extension crashes.
//!
//! # Lifecycle
//!
//! ```text
//! engine -> {"type":"hello","api":1,"extension":"<id>","session":"<session id>"}
//! ext    -> {"type":"hello","api":1}                 (within 10 s by default)
//! engine -> {"type":"activate"}
//!           ... requests and replies ...
//! engine -> {"type":"deactivate"}                    (the session is closing)
//!           stdin is closed; the extension has 2 s by default to exit, then the group is killed
//! ```
//!
//! An extension answering with a different `api` is stopped with a notice. An extension that
//! exits with status 0 has simply finished; any other exit (or a signal) is reported as a
//! crash in an error notice.
//!
//! # Requests (extension -> engine)
//!
//! `{"id": <string or number>, "method": "<method>", "params": {...}}`; the engine replies
//! `{"id": <same>, "result": <value>}` or
//! `{"id": <same>, "error": {"code": "<code>", "message": "<text>"}}`. Requests run
//! concurrently (at most 64 in flight) and replies arrive in completion order. An id must not
//! be reused while its request is in flight. `{"type":"cancel","id":<id>}` withdraws an
//! in-flight request (an `ask` then resolves `null`). Messages with an unknown `type` are
//! ignored, so later engines can add some.
//!
//! | method | params | result |
//! | --- | --- | --- |
//! | `ask` | `kind` (`select`/`confirm`/`input`), `title`, `message`?, `options`? (`[{value,label,description?}]`, select), `placeholder`?, `defaultValue`?, `timeoutMs`? (default 120000, 0 waits indefinitely) | the answer (a string, or a boolean for `confirm`), or `null` when unanswered |
//! | `notify` | `level` (`info`/`warning`/`error`), `message` | `null` |
//! | `setStatus` | `key`, `text` (a string, or `null` to clear) | `null` |
//! | `prompt` / `steer` / `followUp` | `text`, `requestKey`? | `{"operationId": "opaque-key", "run": n or null, "entry": n or null}` |
//! | `cancelQueued` | `entry` | `{"outcome": "cancelled" or "alreadyConsumed" or "notFound"}` |
//! | `lookup` | `requestKey` | the protocol's `LookupResult` (`{"state":"unknown"}` or `{"state":"known",...}`) |
//! | `waitForPrompt` | `operationId` from admission | `{"status":"done","text":"...","reason":null}` or `{"status":"unanswered","text":null,"reason":"..."}` |
//! | `abort` | none | `{"run": n or null}` |
//! | `compact` | `instructions`? | `null` |
//! | `historyPage` | `before`?, `limit` (at most 200) | `{"entries": [Entry...], "hasOlder": bool}` (oldest first) |
//!
//! Error codes are the protocol's `ErrorCode` names (`invalid`, `unsupported` for an unknown
//! method, `busy`, `tooLarge` for a reply over the line limit, `notFound`, `conflict`, ...).
//! `notify` and `setStatus` take effect in the order they were sent.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pipkin_engine_protocol::{Answer, NoticeLevel, ProtoError, QuestionKind, QuestionOption};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

use super::questions::clip_utf16;
use super::{BoxFuture, EXTENSION_API_VERSION, Extension, ExtensionHost, Question};

pub const DEFAULT_MAX_LINE_BYTES: usize = 1 << 20;
pub const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_STOP_GRACE: Duration = Duration::from_secs(2);
/// Most requests one extension may have in flight.
pub const MAX_IN_FLIGHT: usize = 64;
/// How much stderr is kept for a crash notice.
const STDERR_TAIL_BYTES: usize = 4096;

/// How to run one process extension.
#[derive(Clone, Debug)]
pub struct ProcessConfig {
    pub id: String,
    /// Program and arguments; run directly, never through a shell.
    pub argv: Vec<String>,
    pub cwd: Option<PathBuf>,
    /// Extra environment variables (the engine's environment is inherited).
    pub env: Vec<(String, String)>,
    pub max_line_bytes: usize,
    pub handshake_timeout: Duration,
    pub stop_grace: Duration,
}

impl ProcessConfig {
    pub fn new(id: impl Into<String>, argv: Vec<String>) -> ProcessConfig {
        ProcessConfig {
            id: id.into(),
            argv,
            cwd: None,
            env: Vec::new(),
            max_line_bytes: DEFAULT_MAX_LINE_BYTES,
            handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
            stop_grace: DEFAULT_STOP_GRACE,
        }
    }
}

/// An [`Extension`] backed by a child process speaking the protocol above.
pub struct ProcessExtension {
    config: ProcessConfig,
    stop: Mutex<Option<CancellationToken>>,
}

impl ProcessExtension {
    pub fn new(config: ProcessConfig) -> ProcessExtension {
        ProcessExtension {
            config,
            stop: Mutex::new(None),
        }
    }
}

impl Extension for ProcessExtension {
    fn id(&self) -> &str {
        &self.config.id
    }

    fn activate(&self, host: Arc<dyn ExtensionHost>) -> BoxFuture<'static, ()> {
        let stop = CancellationToken::new();
        if let Some(previous) = self
            .stop
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace(stop.clone())
        {
            previous.cancel();
        }
        let config = self.config.clone();
        Box::pin(async move {
            if let Err(message) = run(&config, host.clone(), stop).await {
                log::warn!("{message}");
                host.notify(NoticeLevel::Error, &message);
            }
        })
    }

    fn deactivate(&self) {
        if let Some(stop) = self.stop.lock().unwrap_or_else(|e| e.into_inner()).take() {
            stop.cancel();
        }
    }
}

/// Kills the extension's process group when dropped (including when the activation task is
/// aborted).
struct ProcessGroup {
    pgid: Option<i32>,
}

impl ProcessGroup {
    fn kill(&mut self) {
        #[cfg(unix)]
        if let Some(pgid) = self.pgid.take()
            && pgid > 0
        {
            // SAFETY: kill(2) with a negative pid signals the process group we created.
            unsafe {
                libc::kill(-pgid, libc::SIGKILL);
            }
        }
        #[cfg(not(unix))]
        {
            self.pgid = None;
        }
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.kill();
    }
}

enum Incoming {
    Line(String),
    TooLong,
    Broken(String),
}

enum Ending {
    Stopped,
    Eof,
    Exited(ExitStatus),
    Violation(String),
}

type Tokens = Arc<Mutex<HashMap<String, CancellationToken>>>;

async fn run(
    config: &ProcessConfig,
    host: Arc<dyn ExtensionHost>,
    stop: CancellationToken,
) -> Result<(), String> {
    let id = &config.id;
    let Some((program, args)) = config.argv.split_first() else {
        return Err(format!("Extension {id} has no command to run."));
    };
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env("PIPKIN_EXTENSION_API", EXTENSION_API_VERSION.to_string());
    #[cfg(unix)]
    command.process_group(0);
    if let Some(cwd) = &config.cwd {
        command.current_dir(cwd);
    }
    for (key, value) in &config.env {
        command.env(key, value);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Extension {id} could not start: {e}."))?;
    let mut group = ProcessGroup {
        pgid: child.id().and_then(|pid| i32::try_from(pid).ok()),
    };
    let (Some(stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return Err(format!("Extension {id} could not start: no pipes."));
    };

    let stderr_tail = Arc::new(Mutex::new(Vec::<u8>::new()));
    let stderr_task = tokio::spawn(keep_tail(stderr, stderr_tail.clone()));
    let (lines_tx, mut lines) = mpsc::channel::<Incoming>(16);
    let reader_task = tokio::spawn(read_lines(stdout, lines_tx, config.max_line_bytes));
    let (out, out_rx) = mpsc::channel::<String>(256);
    let writer_task = tokio::spawn(write_lines(stdin, out_rx));

    let ending = session(config, &host, &stop, &mut child, &mut lines, out).await;

    let result = match ending {
        Ending::Stopped => {
            if tokio::time::timeout(config.stop_grace, child.wait())
                .await
                .is_err()
            {
                log::debug!("extension {id} did not exit within the grace period");
            }
            group.kill();
            let _ = child.wait().await;
            Ok(())
        }
        Ending::Eof => {
            let status = match tokio::time::timeout(config.stop_grace, child.wait()).await {
                Ok(Ok(status)) => Some(status),
                _ => None,
            };
            group.kill();
            let status = match status {
                Some(status) => Ok(status),
                None => child.wait().await,
            };
            finished(id, status, &stderr_tail, stderr_task).await
        }
        Ending::Exited(status) => {
            group.kill();
            // Apply what it said before it exited (notices, status lines).
            let drain = async {
                while let Some(Incoming::Line(line)) = lines.recv().await {
                    if let Ok(Value::Object(message)) = serde_json::from_str::<Value>(&line)
                        && let (Some(method), params) = (
                            message.get("method").and_then(Value::as_str),
                            message.get("params"),
                        )
                    {
                        apply_immediate(host.as_ref(), method, params);
                    }
                }
            };
            let _ = tokio::time::timeout(config.stop_grace, drain).await;
            finished(id, Ok(status), &stderr_tail, stderr_task).await
        }
        Ending::Violation(why) => {
            group.kill();
            let _ = child.wait().await;
            Err(format!("Extension {id} {why}; it was stopped."))
        }
    };
    reader_task.abort();
    writer_task.abort();
    result
}

/// The handshake and the request loop, until the extension ends or is stopped.
async fn session(
    config: &ProcessConfig,
    host: &Arc<dyn ExtensionHost>,
    stop: &CancellationToken,
    child: &mut Child,
    lines: &mut mpsc::Receiver<Incoming>,
    out: mpsc::Sender<String>,
) -> Ending {
    let hello = json!({
        "type": "hello",
        "api": EXTENSION_API_VERSION,
        "extension": config.id,
        "session": host.session(),
    });
    if out.send(hello.to_string()).await.is_err() {
        return Ending::Eof;
    }
    let first = tokio::select! {
        _ = stop.cancelled() => return Ending::Stopped,
        status = child.wait() => return exited(status),
        first = tokio::time::timeout(config.handshake_timeout, lines.recv()) => first,
    };
    match first {
        Err(_) => {
            return Ending::Violation(format!(
                "did not answer the hello within {} s",
                config.handshake_timeout.as_secs_f32()
            ));
        }
        Ok(None) => return Ending::Eof,
        Ok(Some(Incoming::Line(line))) => {
            let message = serde_json::from_str::<Value>(&line).unwrap_or(Value::Null);
            if message.get("type").and_then(Value::as_str) != Some("hello") {
                return Ending::Violation("did not start with a hello".to_string());
            }
            let api = message.get("api").and_then(Value::as_u64);
            if api != Some(u64::from(EXTENSION_API_VERSION)) {
                let api = message.get("api").cloned().unwrap_or(Value::Null);
                return Ending::Violation(format!(
                    "speaks extension API {api}, but this engine speaks {EXTENSION_API_VERSION}"
                ));
            }
        }
        Ok(Some(other)) => return violation(other, config.max_line_bytes),
    }
    if out
        .send(json!({"type": "activate"}).to_string())
        .await
        .is_err()
    {
        return Ending::Eof;
    }

    let tokens: Tokens = Arc::new(Mutex::new(HashMap::new()));
    let permits = Arc::new(Semaphore::new(MAX_IN_FLIGHT));
    let mut requests = JoinSet::new();
    let ending = loop {
        tokio::select! {
            biased;
            _ = stop.cancelled() => break Ending::Stopped,
            incoming = lines.recv() => match incoming {
                Some(Incoming::Line(line)) => {
                    if let Err(why) = dispatch_line(
                        &line, host, &out, &tokens, &permits, &mut requests, config.max_line_bytes,
                    ) {
                        break Ending::Violation(why);
                    }
                }
                Some(other) => break violation(other, config.max_line_bytes),
                None => break Ending::Eof,
            },
            status = child.wait() => break exited(status),
            Some(_) = requests.join_next(), if !requests.is_empty() => {}
        }
    };

    // Whatever is still in flight ends with the extension.
    for (_, token) in tokens.lock().unwrap_or_else(|e| e.into_inner()).drain() {
        token.cancel();
    }
    requests.abort_all();
    while requests.join_next().await.is_some() {}
    if matches!(ending, Ending::Stopped) {
        let deactivate = json!({"type": "deactivate"}).to_string();
        let _ = tokio::time::timeout(config.stop_grace, out.send(deactivate)).await;
    }
    // Dropping the last sender lets the writer finish and close the extension's stdin.
    drop(out);
    ending
}

fn exited(status: std::io::Result<ExitStatus>) -> Ending {
    match status {
        Ok(status) => Ending::Exited(status),
        Err(_) => Ending::Eof,
    }
}

fn violation(incoming: Incoming, max: usize) -> Ending {
    match incoming {
        Incoming::TooLong => Ending::Violation(format!("sent a line longer than {max} bytes")),
        Incoming::Broken(why) => Ending::Violation(why),
        Incoming::Line(_) => Ending::Violation("broke the protocol".to_string()),
    }
}

async fn finished(
    id: &str,
    status: std::io::Result<ExitStatus>,
    stderr_tail: &Arc<Mutex<Vec<u8>>>,
    stderr_task: JoinHandle<()>,
) -> Result<(), String> {
    let status = match status {
        Ok(status) if status.success() => return Ok(()),
        Ok(status) => describe(status),
        Err(e) => format!("its status is unknown: {e}"),
    };
    let _ = tokio::time::timeout(Duration::from_millis(500), stderr_task).await;
    let tail = stderr_tail
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let tail = String::from_utf8_lossy(&tail);
    let last = tail.lines().rev().map(str::trim).find(|l| !l.is_empty());
    Err(match last {
        Some(last) => format!(
            "Extension {id} stopped unexpectedly ({status}). It said: {}",
            clip_utf16(last, 300)
        ),
        None => format!("Extension {id} stopped unexpectedly ({status})."),
    })
}

fn describe(status: ExitStatus) -> String {
    if let Some(code) = status.code() {
        return format!("exit code {code}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!("signal {signal}");
        }
    }
    "no exit code".to_string()
}

/// Route one line from the extension. `Err` is a protocol violation.
fn dispatch_line(
    line: &str,
    host: &Arc<dyn ExtensionHost>,
    out: &mpsc::Sender<String>,
    tokens: &Tokens,
    permits: &Arc<Semaphore>,
    requests: &mut JoinSet<()>,
    max_line: usize,
) -> Result<(), String> {
    if line.trim().is_empty() {
        return Ok(());
    }
    let Ok(Value::Object(message)) = serde_json::from_str::<Value>(line) else {
        return Err("sent a line that is not a JSON object".to_string());
    };
    if let Some(kind) = message.get("type") {
        if kind.as_str() == Some("cancel")
            && let Some(key) = message.get("id").and_then(request_key)
            && let Some(token) = tokens.lock().unwrap_or_else(|e| e.into_inner()).get(&key)
        {
            token.cancel();
        }
        return Ok(());
    }
    let (Some(id), Some(method)) = (
        message.get("id").filter(|id| request_key(id).is_some()),
        message.get("method").and_then(Value::as_str),
    ) else {
        return Err("sent a message that is neither a request nor a known message".to_string());
    };
    let id = id.clone();
    let key = request_key(&id).unwrap_or_default();
    let params = message.get("params").cloned().unwrap_or(Value::Null);

    // Notices and status lines apply in the order they were sent.
    if matches!(method, "notify" | "setStatus") {
        let result = apply(host.as_ref(), method, &params);
        send_reply(out, encode_reply(&id, result, max_line));
        return Ok(());
    }
    let Ok(permit) = permits.clone().try_acquire_owned() else {
        let error = RpcError::new("busy", "Too many requests in flight.");
        send_reply(out, encode_reply(&id, Err(error), max_line));
        return Ok(());
    };
    let token = CancellationToken::new();
    {
        let mut tokens = tokens.lock().unwrap_or_else(|e| e.into_inner());
        if tokens.contains_key(&key) {
            let error = RpcError::new("invalid", "That request id is already in flight.");
            send_reply(out, encode_reply(&id, Err(error), max_line));
            return Ok(());
        }
        tokens.insert(key.clone(), token.clone());
    }
    let host = host.clone();
    let out = out.clone();
    let tokens = tokens.clone();
    let method = method.to_string();
    requests.spawn(async move {
        let result = handle(host.as_ref(), &method, params, token).await;
        tokens
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&key);
        let _ = out.send(encode_reply(&id, result, max_line)).await;
        drop(permit);
    });
    Ok(())
}

/// A request id: a string or a number.
fn request_key(id: &Value) -> Option<String> {
    match id {
        Value::String(s) => Some(format!("s:{s}")),
        Value::Number(n) => Some(format!("n:{n}")),
        _ => None,
    }
}

fn send_reply(out: &mpsc::Sender<String>, line: String) {
    if out.try_send(line).is_err() {
        log::debug!("extension output is full or closed; a reply was dropped");
    }
}

#[derive(Debug)]
struct RpcError {
    code: String,
    message: String,
}

impl RpcError {
    fn new(code: &str, message: impl Into<String>) -> RpcError {
        RpcError {
            code: code.to_string(),
            message: message.into(),
        }
    }

    fn invalid(method: &str, error: serde_json::Error) -> RpcError {
        RpcError::new("invalid", format!("Invalid params for {method}: {error}."))
    }
}

impl From<ProtoError> for RpcError {
    fn from(error: ProtoError) -> RpcError {
        let code = serde_json::to_value(error.code)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| "internal".to_string());
        RpcError {
            code,
            message: error.message,
        }
    }
}

fn encode_reply(id: &Value, result: Result<Value, RpcError>, max_line: usize) -> String {
    let line = match &result {
        Ok(value) => format!("{{\"id\":{id},\"result\":{value}}}"),
        Err(error) => error_line(id, &error.code, &error.message),
    };
    if line.len() > max_line {
        return error_line(
            id,
            "tooLarge",
            &format!("The reply is longer than {max_line} bytes."),
        );
    }
    line
}

fn error_line(id: &Value, code: &str, message: &str) -> String {
    let error = json!({"code": code, "message": message});
    format!("{{\"id\":{id},\"error\":{error}}}")
}

fn params<T: for<'de> Deserialize<'de>>(method: &str, params: &Value) -> Result<T, RpcError> {
    let params = if params.is_null() {
        Value::Object(Default::default())
    } else {
        params.clone()
    };
    serde_json::from_value(params).map_err(|e| RpcError::invalid(method, e))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AskParams {
    kind: QuestionKind,
    title: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    options: Vec<QuestionOption>,
    #[serde(default)]
    placeholder: Option<String>,
    #[serde(default)]
    default_value: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Deserialize)]
struct NotifyParams {
    level: NoticeLevel,
    message: String,
}

#[derive(Deserialize)]
struct StatusParams {
    key: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InputParams {
    text: String,
    #[serde(default)]
    request_key: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LookupParams {
    request_key: String,
}

#[derive(Deserialize)]
struct CancelQueuedParams {
    entry: u64,
}

#[derive(Deserialize)]
struct CompactParams {
    #[serde(default)]
    instructions: Option<String>,
}

#[derive(Deserialize)]
struct HistoryParams {
    #[serde(default)]
    before: Option<u64>,
    limit: u32,
}

/// `notify` and `setStatus`: synchronous, applied in order.
fn apply(host: &dyn ExtensionHost, method: &str, raw: &Value) -> Result<Value, RpcError> {
    match method {
        "notify" => {
            let p: NotifyParams = params(method, raw)?;
            host.notify(p.level, &p.message);
        }
        _ => {
            let p: StatusParams = params(method, raw)?;
            host.set_status(&p.key, p.text.as_deref());
        }
    }
    Ok(Value::Null)
}

/// Apply a `notify`/`setStatus` sent just before the extension exited; anything else is
/// dropped (nobody is left to read its reply).
fn apply_immediate(host: &dyn ExtensionHost, method: &str, raw: Option<&Value>) {
    if matches!(method, "notify" | "setStatus") {
        let _ = apply(host, method, raw.unwrap_or(&Value::Null));
    }
}

fn to_json<T: serde::Serialize>(value: &T) -> Result<Value, RpcError> {
    serde_json::to_value(value).map_err(|e| RpcError::new("internal", e.to_string()))
}

async fn handle(
    host: &dyn ExtensionHost,
    method: &str,
    raw: Value,
    cancel: CancellationToken,
) -> Result<Value, RpcError> {
    match method {
        "ask" => {
            let p: AskParams = params(method, &raw)?;
            let question = Question {
                kind: p.kind,
                title: p.title,
                message: p.message,
                options: p.options,
                placeholder: p.placeholder,
                default_value: p.default_value,
                // Extensions ask single questions; questionnaires are the model tool's.
                form: None,
                source: None,
            };
            let timeout = p.timeout_ms.map(Duration::from_millis);
            Ok(match host.ask(question, timeout, cancel).await {
                None => Value::Null,
                Some(Answer::Confirm { value }) => Value::Bool(value),
                Some(Answer::Choice { value } | Answer::Text { value }) => Value::String(value),
                Some(form @ Answer::Form { .. }) => {
                    serde_json::to_value(form).unwrap_or(Value::Null)
                }
            })
        }
        "prompt" | "steer" | "followUp" => {
            let p: InputParams = params(method, &raw)?;
            let admitted = match method {
                "prompt" => host.prompt(p.text, p.request_key).await?,
                "steer" => host.steer(p.text, p.request_key).await?,
                _ => host.follow_up(p.text, p.request_key).await?,
            };
            Ok(
                json!({"run": admitted.run, "entry": admitted.entry, "operationId": admitted.operation_id}),
            )
        }
        "cancelQueued" => {
            let p: CancelQueuedParams = params(method, &raw)?;
            let outcome = host.cancel_queued(p.entry).await?;
            Ok(json!({"outcome": to_json(&outcome)?}))
        }
        "lookup" => {
            let p: LookupParams = params(method, &raw)?;
            to_json(&host.lookup(p.request_key).await?)
        }
        "waitForPrompt" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct WaitParams {
                operation_id: String,
            }
            let p: WaitParams = params(method, &raw)?;
            tokio::select! {
                biased;
                _ = cancel.cancelled() => Err(RpcError::new("cancelled", "Prompt wait cancelled")),
                result = host.wait_for_prompt(p.operation_id) => to_json(&result?),
            }
        }
        "abort" => {
            let run = host.abort().await?;
            Ok(json!({"run": run}))
        }
        "compact" => {
            let p: CompactParams = params(method, &raw)?;
            host.compact(p.instructions).await?;
            Ok(Value::Null)
        }
        "historyPage" => {
            let p: HistoryParams = params(method, &raw)?;
            let page = host.history_page(p.before, p.limit).await?;
            Ok(json!({"entries": to_json(&page.entries)?, "hasOlder": page.has_older}))
        }
        "notify" | "setStatus" => apply(host, method, &raw),
        _ => Err(RpcError::new(
            "unsupported",
            format!("Unknown method {method}."),
        )),
    }
}

async fn read_lines(stdout: ChildStdout, tx: mpsc::Sender<Incoming>, max: usize) {
    let mut reader = BufReader::new(stdout);
    let mut buf = Vec::new();
    loop {
        let incoming = match read_line_bounded(&mut reader, &mut buf, max).await {
            Ok(LineRead::Eof) => return,
            Ok(LineRead::TooLong) => Incoming::TooLong,
            Ok(LineRead::Line) => {
                if buf.last() == Some(&b'\r') {
                    buf.pop();
                }
                match String::from_utf8(std::mem::take(&mut buf)) {
                    Ok(line) => Incoming::Line(line),
                    Err(_) => Incoming::Broken("sent a line that is not UTF-8".to_string()),
                }
            }
            Err(e) => Incoming::Broken(format!("could not be read: {e}")),
        };
        let last = !matches!(incoming, Incoming::Line(_));
        if tx.send(incoming).await.is_err() || last {
            return;
        }
    }
}

enum LineRead {
    Line,
    TooLong,
    Eof,
}

/// Read one `\n`-terminated line (without it) into `buf`, refusing to hold more than `max`
/// bytes. A final unterminated line counts as a line.
async fn read_line_bounded<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    buf: &mut Vec<u8>,
    max: usize,
) -> std::io::Result<LineRead> {
    buf.clear();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(if buf.is_empty() {
                LineRead::Eof
            } else {
                LineRead::Line
            });
        }
        let (take, done) = match available.iter().position(|b| *b == b'\n') {
            Some(at) => (at, true),
            None => (available.len(), false),
        };
        if buf.len() + take > max {
            return Ok(LineRead::TooLong);
        }
        buf.extend_from_slice(&available[..take]);
        reader.consume(if done { take + 1 } else { take });
        if done {
            return Ok(LineRead::Line);
        }
    }
}

async fn write_lines(mut stdin: ChildStdin, mut rx: mpsc::Receiver<String>) {
    while let Some(mut line) = rx.recv().await {
        line.push('\n');
        if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
            return;
        }
    }
    let _ = stdin.shutdown().await;
}

async fn keep_tail(mut stderr: tokio::process::ChildStderr, tail: Arc<Mutex<Vec<u8>>>) {
    let mut chunk = [0u8; 1024];
    loop {
        match stderr.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                let mut tail = tail.lock().unwrap_or_else(|e| e.into_inner());
                tail.extend_from_slice(&chunk[..n]);
                if tail.len() > STDERR_TAIL_BYTES {
                    let excess = tail.len() - STDERR_TAIL_BYTES;
                    tail.drain(..excess);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
