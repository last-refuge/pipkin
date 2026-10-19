//! Shared subprocess supervisor qualified by synthetic process tests.
//! No credential integration or inference admission policy.
//! Output is bounded; stderr is counted, never retained. Unix process groups are owned
//! by this invocation. This is not a sandbox (a child can deliberately escape its group).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep_until};
use tokio_util::sync::CancellationToken;

use super::stream::{Decoder, StreamSummary};

pub enum OutputMode {
    Capture,
    Stream,
}

pub struct Spec {
    pub executable: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    // Snapshot the environment once for preflight and execution. Never Debug this struct.
    pub env: BTreeMap<OsString, OsString>,
    pub input: Vec<u8>,
    pub mode: OutputMode,
    pub timeout: Duration,
    pub grace: Duration,
    pub max_stdout: usize,
    pub max_stderr: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stop {
    Cancelled,
    TimedOut,
    OutputLimit,
    InvalidStream,
    ObserverFailure,
    PipeFailure,
    CleanupTimeout,
}

/// Closed OS categories only: no paths, stderr or arbitrary io::Error text. Never retry.
#[cfg(unix)]
fn spawn_failure(error: &std::io::Error) -> &'static str {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::ExecutableFileBusy => "selected executable is busy; no automatic retry",
        ErrorKind::WouldBlock => "process resources unavailable; no automatic retry",
        ErrorKind::PermissionDenied => "selected executable access denied",
        ErrorKind::NotFound => "selected executable or interpreter unavailable",
        _ => "failed to spawn selected executable",
    }
}

#[derive(Serialize)]
pub struct Report {
    pub exit_code: Option<i32>,
    pub stop: Option<Stop>,
    pub stdin_delivered: bool,
    pub root_reaped: bool,
    pub pipes_drained: bool,
    pub cleanup_timed_out: bool,
    // None for ordinary metadata/smoke calls; Some(false) means cleanup proceeded
    // without confirmed approval revocation and must remain an uncertain outcome.
    pub approval_revocation_confirmed: Option<bool>,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub elapsed_ms: u64,
    pub stream: Option<StreamSummary>,
    pub stream_error: Option<&'static str>,
    // Metadata probes use this internally; never serialize raw CLI output.
    #[serde(skip)]
    pub captured: Vec<u8>,
}

impl Report {
    pub fn clean_exit(&self) -> bool {
        self.exit_code == Some(0)
            && self.stop.is_none()
            && self.stdin_delivered
            && self.root_reaped
            && self.pipes_drained
            && !self.cleanup_timed_out
            && self.approval_revocation_confirmed != Some(false)
    }
}

impl std::fmt::Debug for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Keep raw probe output out of diagnostics as well as serialized evidence.
        write!(
            f,
            "{}",
            serde_json::to_string(self).map_err(|_| std::fmt::Error)?
        )
    }
}

enum Packet {
    Data(bool, Vec<u8>), // true = stdout
    Eof,
    ReadFailed,
    InputDone(bool),
}

struct OwnedTasks(Vec<JoinHandle<()>>);
impl Drop for OwnedTasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

/// Single-attempt revocation fence called before the first owned-group signal.
pub type BeforeSignal = Box<dyn FnMut() -> bool + Send>;

/// Maximum raw stdout bytes supplied in one observer call (not a record boundary).
pub const MAX_OBSERVER_CHUNK_BYTES: usize = 8192;

/// Synchronous raw-stdout observer. It MUST return promptly without blocking, waiting
/// on I/O/locks, or retaining an unbounded transcript. Any downstream queue MUST be
/// bounded and use nonblocking submission (e.g. `try_send`); full/closed means `Err`.
/// A caller decoder must separately finish at EOF and check the process report before
/// establishing success. Borrowed chunks may split UTF-8 or NDJSON records.
///
/// Errors are discarded, replaced with a static generic diagnostic, and trigger the
/// same fenced termination/drain/reap path as other failures. Observations are not
/// authority to publish success, retry, or infer that external effects did not happen.
pub type StdoutObserver = Box<dyn FnMut(&[u8]) -> Result<(), &'static str> + Send>;

/// Owned, bounded chunk delivery with cancellable backpressure. Used by the provider,
/// whose decoder owns framing/terminal validation (no second smoke decoder).
pub type AsyncStdoutObserver = Box<
    dyn FnMut(
            Vec<u8>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), &'static str>> + Send>,
        > + Send,
>;

enum Observer {
    Sync(StdoutObserver),
    Async(AsyncStdoutObserver),
}
impl Observer {
    async fn observe(&mut self, bytes: &[u8]) -> Result<(), &'static str> {
        use futures_util::FutureExt;
        match self {
            Self::Sync(callback) => {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(bytes)))
                    .unwrap_or(Err("observer panicked"))
            }
            Self::Async(callback) => {
                let future = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    callback(bytes.to_vec())
                }))
                .map_err(|_| "observer panicked")?;
                std::panic::AssertUnwindSafe(future)
                    .catch_unwind()
                    .await
                    .unwrap_or(Err("observer panicked"))
            }
        }
    }
}

struct OwnedGroup {
    pid: u32,
    before_signal: Option<BeforeSignal>,
    fence_confirmed: Option<bool>,
}
impl OwnedGroup {
    fn fence(&mut self) {
        if self.fence_confirmed.is_none()
            && let Some(fence) = &mut self.before_signal
        {
            // Exactly one attempt. A failed fence never abandons process cleanup.
            // SQLite busy timeout is not a hard deadline for underlying filesystem I/O.
            // Mark the attempt BEFORE calling foreign code. A panic must neither repeat
            // revocation in Drop nor prevent the owned group from being terminated.
            self.fence_confirmed = Some(false);
            self.fence_confirmed = Some(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(fence)).unwrap_or(false),
            );
        }
    }

    #[cfg(unix)]
    fn signal(&mut self, signal: i32) {
        self.fence();
        // SAFETY: positive pid returned by our newly spawned process_group(0) child.
        // We never accept a process id from configuration, a transcript, or CLI output.
        unsafe { libc::kill(-(self.pid as i32), signal) };
    }
}
impl Drop for OwnedGroup {
    fn drop(&mut self) {
        #[cfg(unix)]
        self.signal(libc::SIGKILL);
    }
}

fn reader(
    mut pipe: impl AsyncRead + Unpin + Send + 'static,
    stdout: bool,
    tx: mpsc::Sender<Packet>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut buf = [0u8; MAX_OBSERVER_CHUNK_BYTES];
        loop {
            let packet = match pipe.read(&mut buf).await {
                Ok(0) => {
                    let _ = tx.send(Packet::Eof).await;
                    break;
                }
                Ok(n) => Packet::Data(stdout, buf[..n].to_vec()),
                Err(_) => {
                    let _ = tx.send(Packet::ReadFailed).await;
                    break;
                }
            };
            if tx.send(packet).await.is_err() {
                break;
            }
        }
    })
}

/// Run exactly once. The caller must not interpret a flush as remote completion or retry
/// an uncertain invocation. Futures dropped by the caller abort pumps and kill the group.
pub async fn run(spec: Spec, cancel: CancellationToken) -> Result<Report, &'static str> {
    run_observed(spec, cancel, None, None).await
}

/// Optional synchronous, single-attempt fixture-ledger fence before the FIRST group signal,
/// including timeout/protocol/pipe failures, root-exit cleanup and future drop. Not a
/// production broker callback: durable I/O here runs outside any UI/render path.
pub async fn run_fenced(
    spec: Spec,
    cancel: CancellationToken,
    before_signal: Option<BeforeSignal>,
) -> Result<Report, &'static str> {
    run_observed(spec, cancel, before_signal, None).await
}

/// Run with an optional bounded, nonblocking raw-stdout observer (see [`StdoutObserver`]).
/// Called only for stdout within the byte/time/cancellation limits, before capture or
/// summary decoding, and never again after a stop. The internal pipe queue holds at
/// most eight chunks, each at most [`MAX_OBSERVER_CHUNK_BYTES`]. The observer must
/// enforce its own retention/queue bound; a synchronous callback cannot be preempted.
/// No EOF callback is sent: finish caller-owned decoding after the report, checking
/// `pipes_drained`, `stop`, and `clean_exit()` rather than treating EOF as success.
/// Callback rejection leaves the outcome uncertain even when cleanup succeeds.
pub async fn run_observed(
    spec: Spec,
    cancel: CancellationToken,
    before_signal: Option<BeforeSignal>,
    observer: Option<StdoutObserver>,
) -> Result<Report, &'static str> {
    run_owned(spec, cancel, before_signal, observer.map(Observer::Sync)).await
}

pub async fn run_observed_async(
    spec: Spec,
    cancel: CancellationToken,
    before_signal: Option<BeforeSignal>,
    observer: AsyncStdoutObserver,
) -> Result<Report, &'static str> {
    run_owned(spec, cancel, before_signal, Some(Observer::Async(observer))).await
}

#[cfg(unix)]
async fn run_owned(
    spec: Spec,
    cancel: CancellationToken,
    before_signal: Option<BeforeSignal>,
    mut observer: Option<Observer>,
) -> Result<Report, &'static str> {
    if cancel.is_cancelled() {
        return Err("cancelled before spawn");
    }
    let start = Instant::now();
    let mut cmd = tokio::process::Command::new(&spec.executable);
    cmd.args(&spec.args)
        .current_dir(&spec.cwd)
        .env_clear()
        .envs(&spec.env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|error| spawn_failure(&error))?;
    let mut group = OwnedGroup {
        pid: child.id().ok_or("child has no process id")?,
        before_signal,
        fence_confirmed: None,
    };
    let (tx, mut rx) = mpsc::channel(8);
    let stdout = child.stdout.take().ok_or("missing stdout pipe")?;
    let stderr = child.stderr.take().ok_or("missing stderr pipe")?;
    let mut stdin = child.stdin.take().ok_or("missing stdin pipe")?;
    let mut tasks = OwnedTasks(vec![
        reader(stdout, true, tx.clone()),
        reader(stderr, false, tx.clone()),
    ]);
    tasks.0.push(tokio::spawn(async move {
        let ok = async {
            stdin.write_all(&spec.input).await?;
            stdin.shutdown().await
        }
        .await
        .is_ok();
        drop(stdin);
        let _ = tx.send(Packet::InputDone(ok)).await;
    }));

    let mut report = Report {
        exit_code: None,
        stop: None,
        stdin_delivered: false,
        root_reaped: false,
        pipes_drained: false,
        cleanup_timed_out: false,
        approval_revocation_confirmed: None,
        stdout_bytes: 0,
        stderr_bytes: 0,
        elapsed_ms: 0,
        stream: None,
        stream_error: None,
        captured: Vec::new(),
    };
    let provider = matches!(observer, Some(Observer::Async(_)));
    let mut decoder = (matches!(spec.mode, OutputMode::Stream) && !provider).then(Decoder::default);
    let mut exited = false;
    // Do not reap the group leader until all group signals are finished. Keeping its
    // PID reserved prevents a delayed signal from reaching a reused, unrelated PGID.
    let mut exit_poll = tokio::time::interval(Duration::from_millis(10));
    exit_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut eofs = 0;
    let mut input_done = false;
    let mut channel_open = true;
    let mut kill_at = None;
    let mut finish_at = None;
    let deadline = start + spec.timeout;
    loop {
        if exited && eofs == 2 && input_done && kill_at.is_none() {
            break;
        }
        let mut stop = None;
        tokio::select! {
            biased;
            _ = cancel.cancelled(), if report.stop.is_none() => stop = Some(Stop::Cancelled),
            _ = sleep_until(deadline), if report.stop.is_none() => stop = Some(Stop::TimedOut),
            _ = sleep_optional(kill_at), if kill_at.is_some() => {
                group.signal(libc::SIGKILL);
                kill_at = None;
            }
            _ = sleep_optional(finish_at), if finish_at.is_some() => {
                report.stop.get_or_insert(Stop::CleanupTimeout);
                report.cleanup_timed_out = true;
                break;
            }
            _ = exit_poll.tick(), if !exited => {
                if exited_without_reaping(group.pid)? {
                    exited = true;
                    // Root exit doesn't imply its descendants exited or released pipes.
                    group.signal(libc::SIGTERM);
                    kill_at.get_or_insert(Instant::now() + spec.grace);
                    // Provider delivery can be backpressured after the root exits.
                    // Keep its overall turn deadline; the short cleanup deadline applies
                    // after a real stop, not while draining healthy queued output.
                    finish_at.get_or_insert(if provider { deadline } else { Instant::now() + spec.grace + Duration::from_secs(1) });
                }
            }
            packet = rx.recv(), if channel_open => match packet {
                Some(Packet::Data(stdout, bytes)) => {
                    let count = if stdout { &mut report.stdout_bytes } else { &mut report.stderr_bytes };
                    *count = count.saturating_add(bytes.len() as u64);
                    let limit = if stdout { spec.max_stdout } else { spec.max_stderr };
                    if *count > limit as u64 {
                        stop = Some(Stop::OutputLimit);
                    } else if stdout && report.stop.is_none() {
                        let observed = if let Some(observer) = observer.as_mut() {
                            let delivery = observer.observe(&bytes);
                            tokio::pin!(delivery);
                            loop {
                                tokio::select! {
                                    biased;
                                    _ = cancel.cancelled() => { stop = Some(Stop::Cancelled); break Ok(()) },
                                    _ = sleep_until(deadline) => { stop = Some(Stop::TimedOut); break Ok(()) },
                                    _ = sleep_optional(kill_at), if kill_at.is_some() => { group.signal(libc::SIGKILL); kill_at = None; },
                                    _ = sleep_optional(finish_at), if finish_at.is_some() => { stop = Some(Stop::CleanupTimeout); break Ok(()) },
                                    result = &mut delivery => break result,
                                }
                            }
                        } else { Ok(()) };
                        if observed.is_err() {
                            report.stream_error = Some("stdout observer failed");
                            stop = Some(Stop::ObserverFailure);
                        } else if let Some(decoder) = &mut decoder {
                            if let Err(error) = decoder.feed(&bytes) {
                                report.stream_error = Some(error);
                                stop = Some(Stop::InvalidStream);
                            }
                        } else if !provider {
                            report.captured.extend_from_slice(&bytes);
                        }
                    }
                }
                Some(Packet::Eof) => eofs += 1,
                Some(Packet::ReadFailed) => stop = Some(Stop::PipeFailure),
                Some(Packet::InputDone(ok)) => {
                    input_done = true;
                    report.stdin_delivered = ok;
                    if !ok { stop = Some(Stop::PipeFailure); }
                }
                None => channel_open = false,
            }
        }
        if let Some(stop) = stop
            && report.stop.is_none()
        {
            report.stop = Some(stop);
            group.signal(libc::SIGTERM);
            kill_at = Some(Instant::now() + spec.grace);
            finish_at = Some(Instant::now() + spec.grace + Duration::from_secs(1));
        }
    }
    // Also kill same-group descendants that closed their inherited pipes. Detached/escaped
    // descendants require a stronger OS containment boundary before production support.
    group.fence();
    report.approval_revocation_confirmed = group.fence_confirmed;
    drop(group);
    // No group identity or signal callback remains after this point. Only now reap.
    let status = tokio::time::timeout(Duration::from_secs(1), child.wait())
        .await
        .map_err(|_| "CLI termination unconfirmed; do not retry")?
        .map_err(|_| "failed to reap CLI process")?;
    report.exit_code = status.code();
    report.root_reaped = true;
    report.pipes_drained = eofs == 2;
    drop(tasks);
    if let Some(decoder) = decoder {
        match decoder.finish() {
            Ok(summary) => report.stream = Some(summary),
            Err(error) => {
                report.stream_error.get_or_insert(error);
                report.stop.get_or_insert(Stop::InvalidStream);
            }
        }
    }
    report.elapsed_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
    Ok(report)
}

#[cfg(not(unix))]
async fn run_owned(
    _spec: Spec,
    _cancel: CancellationToken,
    _before_signal: Option<BeforeSignal>,
    _observer: Option<Observer>,
) -> Result<Report, &'static str> {
    Err("the Phase 0 process supervisor currently supports Unix only")
}

#[cfg(unix)]
fn exited_without_reaping(pid: u32) -> Result<bool, &'static str> {
    // SAFETY: waitid writes to initialized siginfo for our own positive child pid;
    // WNOHANG is nonblocking and WNOWAIT keeps ownership until the final child.wait().
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            pid,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result != 0 {
        return if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            Ok(false)
        } else {
            Err("could not observe CLI exit without reaping")
        };
    }
    // SAFETY: waitid populated siginfo; si_pid is the child identity (zero if still live).
    Ok(unsafe { info.si_pid() } != 0)
}

async fn sleep_optional(at: Option<Instant>) {
    match at {
        Some(at) => sleep_until(at).await,
        None => std::future::pending().await,
    }
}

#[cfg(all(test, unix))]
#[path = "process_observer_tests.rs"]
mod observer_tests;

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn spec(script: &str, cwd: &std::path::Path) -> Spec {
        Spec {
            executable: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            cwd: cwd.into(),
            env: std::env::vars_os().collect(),
            input: Vec::new(),
            mode: OutputMode::Capture,
            timeout: Duration::from_secs(5),
            grace: Duration::from_millis(50),
            max_stdout: 128 * 1024,
            max_stderr: 128 * 1024,
        }
    }

    #[tokio::test]
    async fn stdin_is_data_not_shell_or_arguments_and_stderr_is_never_reported() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = spec("cat; printf 'secret-do-not-serialize' >&2", dir.path());
        s.input = b"literal $(touch do-not-create) \"quotes\"\n".to_vec();
        let expected = s.input.clone();
        let result = run(s, CancellationToken::new()).await.unwrap();
        assert!(result.clean_exit(), "{result:?}");
        assert_eq!(result.captured, expected);
        assert!(!dir.path().join("do-not-create").exists());
        let json = serde_json::to_string(&result).unwrap();
        assert!(!json.contains("secret-do-not-serialize"));
        assert!(!json.contains("touch"));
        assert!(result.stderr_bytes > 0);
    }

    #[tokio::test]
    async fn flooded_pipes_are_bounded_and_terminated() {
        let dir = tempfile::tempdir().unwrap();
        for script in ["yes stdout", "yes stderr >&2"] {
            let mut s = spec(script, dir.path());
            s.max_stdout = 1024;
            s.max_stderr = 1024;
            let result = run(s, CancellationToken::new()).await.unwrap();
            assert_eq!(result.stop, Some(Stop::OutputLimit));
            assert!(result.captured.len() <= 1024);
        }
    }

    #[tokio::test]
    async fn timeout_kills_a_child_that_ignores_term() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = spec("trap '' TERM; while :; do sleep 1; done", dir.path());
        s.timeout = Duration::from_millis(100);
        let result = run(s, CancellationToken::new()).await.unwrap();
        assert_eq!(result.stop, Some(Stop::TimedOut));
        assert!(result.elapsed_ms < 3000);
        assert!(!result.clean_exit());
    }

    #[tokio::test]
    async fn cancellation_before_spawn_never_executes() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(
            run(spec("touch should-not-exist", dir.path()), cancel)
                .await
                .is_err()
        );
        assert!(!dir.path().join("should-not-exist").exists());
    }

    #[tokio::test]
    async fn cancellation_during_run_is_not_success() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let other = cancel.clone();
        let timer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            other.cancel();
        });
        let result = run(spec("sleep 30", dir.path()), cancel).await.unwrap();
        timer.await.unwrap();
        assert_eq!(result.stop, Some(Stop::Cancelled));
        assert!(result.root_reaped && result.pipes_drained && !result.cleanup_timed_out);
        assert!(!result.clean_exit());
    }

    #[tokio::test]
    async fn root_exit_cleans_same_group_descendant_holding_pipes() {
        let dir = tempfile::tempdir().unwrap();
        let result = run(
            spec("sleep 30 & exit 0", dir.path()),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(result.elapsed_ms < 3000);
        assert_eq!(result.exit_code, Some(0));
    }

    #[tokio::test]
    async fn descendants_get_term_grace_even_after_closing_pipes() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = spec(
            "sh -c 'trap \"sleep 0.03; touch cleanup-complete; exit 0\" TERM; exec >/dev/null 2>&1; touch ready; while :; do sleep 1; done' & while [ ! -f ready ]; do sleep 0.01; done; exit 0",
            dir.path(),
        );
        s.grace = Duration::from_millis(400);
        let report = run(s, CancellationToken::new()).await.unwrap();
        assert!(report.clean_exit(), "{report:?}");
        assert!(dir.path().join("cleanup-complete").exists());
        assert!(report.elapsed_ms >= 400);
    }

    #[tokio::test]
    async fn observing_exit_does_not_reap_or_release_the_owned_pid() {
        let mut child = tokio::process::Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pid = child.id().unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !exited_without_reaping(pid).unwrap() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        // Repeated WNOWAIT observes the same child, so the numeric PID cannot be reused.
        assert!(exited_without_reaping(pid).unwrap());
        assert!(child.wait().await.unwrap().success());
    }

    #[tokio::test]
    async fn every_supervisor_signal_path_fences_first_and_only_once() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        for case in [
            "timeout",
            "invalid_stream",
            "output_limit",
            "pipe_failure",
            "root_exit",
            "cancel",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let script = if case == "root_exit" {
                "exit 0"
            } else if case == "invalid_stream" {
                // Avoid a foreground sleep spawned just after TERM: shells may defer a
                // trap until that child exits, making a short-grace ordering test flaky.
                "trap 'test -f fence && echo yes > term-after-fence; exit 0' TERM; printf 'invalid-json\\n'; while :; do :; done"
            } else if case == "output_limit" {
                "trap 'test -f fence && echo yes > term-after-fence; exit 0' TERM; while :; do printf 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\\n'; done"
            } else if case == "pipe_failure" {
                "trap 'test -f fence && echo yes > term-after-fence; exit 0' TERM; exec 0<&-; while :; do :; done"
            } else {
                "trap 'test -f fence && echo yes > term-after-fence; exit 0' TERM; touch ready; while :; do :; done"
            };
            let mut spec = spec(script, dir.path());
            spec.grace = Duration::from_millis(150);
            if case == "timeout" {
                spec.timeout = Duration::from_millis(300);
            }
            if case == "invalid_stream" {
                spec.mode = OutputMode::Stream;
            }
            if case == "output_limit" {
                spec.max_stdout = 1024;
            }
            if case == "pipe_failure" {
                spec.input = vec![b'x'; 2 * 1024 * 1024];
            }
            let calls = Arc::new(AtomicUsize::new(0));
            let observed = calls.clone();
            let marker = dir.path().join("fence");
            let fence = Box::new(move || {
                observed.fetch_add(1, Ordering::SeqCst);
                std::fs::write(&marker, b"revoked").is_ok()
            });
            let cancel = CancellationToken::new();
            let canceller = cancel.clone();
            let ready = dir.path().join("ready");
            let cancel_task = tokio::spawn(async move {
                if case == "cancel" {
                    tokio::time::timeout(Duration::from_secs(3), async {
                        while !ready.exists() {
                            tokio::time::sleep(Duration::from_millis(5)).await;
                        }
                    })
                    .await
                    .unwrap();
                    canceller.cancel();
                }
            });
            let report = run_fenced(spec, cancel, Some(fence)).await.unwrap();
            cancel_task.await.unwrap();
            assert_eq!(report.approval_revocation_confirmed, Some(true));
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert!(report.root_reaped && report.pipes_drained && !report.cleanup_timed_out);
            if case != "root_exit" {
                assert_eq!(
                    std::fs::read(dir.path().join("term-after-fence")).unwrap_or_else(
                        |error| panic!("{case}: TERM fixture did not report: {error}; {report:?}")
                    ),
                    b"yes\n"
                );
            }
        }
    }

    #[tokio::test]
    async fn dropping_supervisor_future_fences_before_its_group_kill() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("fenced");
        let written = marker.clone();
        {
            let future = run_fenced(
                spec("touch ready; sleep 30", dir.path()),
                CancellationToken::new(),
                Some(Box::new(move || {
                    std::fs::write(&written, b"revoked").is_ok()
                })),
            );
            tokio::pin!(future);
            tokio::select! {
                result = &mut future => panic!("unexpected early exit: {result:?}"),
                ready = tokio::time::timeout(Duration::from_secs(3), async {
                    while !dir.path().join("ready").exists() { tokio::time::sleep(Duration::from_millis(5)).await; }
                }) => ready.unwrap(),
            }
        }
        assert_eq!(std::fs::read(marker).unwrap(), b"revoked");
    }

    #[tokio::test]
    async fn failed_fence_does_not_abandon_cleanup_or_report_clean_success() {
        let dir = tempfile::tempdir().unwrap();
        let report = run_fenced(
            spec("exit 0", dir.path()),
            CancellationToken::new(),
            Some(Box::new(|| false)),
        )
        .await
        .unwrap();
        assert!(report.root_reaped);
        assert_eq!(report.approval_revocation_confirmed, Some(false));
        assert!(!report.clean_exit());
    }

    #[tokio::test]
    async fn missing_executable_is_a_sanitized_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = spec("", dir.path());
        s.executable = dir.path().join("secret-path-do-not-print");
        let error = run(s, CancellationToken::new()).await.unwrap_err();
        assert_eq!(error, "selected executable or interpreter unavailable");
    }

    #[test]
    fn spawn_failure_categories_do_not_echo_arbitrary_error_text() {
        use std::io::{Error, ErrorKind};
        for (kind, expected) in [
            (
                ErrorKind::ExecutableFileBusy,
                "selected executable is busy; no automatic retry",
            ),
            (
                ErrorKind::WouldBlock,
                "process resources unavailable; no automatic retry",
            ),
            (
                ErrorKind::PermissionDenied,
                "selected executable access denied",
            ),
            (ErrorKind::Other, "failed to spawn selected executable"),
        ] {
            assert_eq!(
                spawn_failure(&Error::new(kind, "private path/argument")),
                expected
            );
        }
    }

    #[tokio::test]
    async fn stream_success_requires_both_result_and_clean_exit() {
        let dir = tempfile::tempdir().unwrap();
        // Fabricated protocol envelopes, not captured Claude output.
        let init = r#"{"type":"system","subtype":"init","session_id":"11111111-1111-4111-8111-111111111111"}"#;
        let result = r#"{"type":"result","subtype":"success","is_error":false,"session_id":"11111111-1111-4111-8111-111111111111"}"#;
        for (lines, exit, expected_result) in [
            (format!("{init}\n{result}"), 0, true),
            (format!("{init}\n{result}"), 7, true),
            (init.to_owned(), 0, false),
        ] {
            let mut s = spec(
                &format!("printf '%s\\n' '{lines}'; exit {exit}"),
                dir.path(),
            );
            s.mode = OutputMode::Stream;
            let report = run(s, CancellationToken::new()).await.unwrap();
            assert_eq!(
                report.stream.as_ref().unwrap().result_success,
                expected_result
            );
            assert_eq!(report.clean_exit(), exit == 0);
            assert_eq!(
                report.clean_exit() && report.stream.as_ref().unwrap().result_success,
                expected_result && exit == 0
            );
            assert!(report.captured.is_empty());
        }
    }

    #[tokio::test]
    async fn both_pipes_are_drained_concurrently() {
        let dir = tempfile::tempdir().unwrap();
        let script = "i=0; while [ $i -lt 4096 ]; do printf 'output-data\\n'; printf 'stderr-data\\n' >&2; i=$((i+1)); done";
        let report = run(spec(script, dir.path()), CancellationToken::new())
            .await
            .unwrap();
        assert!(report.clean_exit(), "{report:?}");
        assert!(report.stdout_bytes > 32 * 1024);
        assert!(report.stderr_bytes > 32 * 1024);
    }

    #[tokio::test]
    async fn malformed_stream_stops_the_process_without_echoing_content() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = spec("printf 'secret-invalid-json\\n'; sleep 30", dir.path());
        s.mode = OutputMode::Stream;
        let result = run(s, CancellationToken::new()).await.unwrap();
        assert_eq!(result.stop, Some(Stop::InvalidStream));
        assert!(
            !serde_json::to_string(&result)
                .unwrap()
                .contains("secret-invalid-json")
        );
    }
}
