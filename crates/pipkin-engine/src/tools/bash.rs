//! `bash` (`tools/bash.ts`, `env/node.ts` `exec`).
//!
//! The command runs as `<shell> -c <command>` in the call's cwd, with the process environment
//! plus [`ToolCtx::env`], in its own process group, stdin closed. stdout and stderr are drained
//! concurrently by two reader tasks and forwarded, decoded per stream, as they arrive through
//! [`ToolCtx::output`]; `run` keeps the tail window and publishes throttled progress. Once the
//! output passes 50 KiB or 2000 lines, all of it (the earlier prefix included, raw bytes in
//! arrival order) is also written to a private spill file reported as `Full output: <path>`.
//!
//! Timeout and cancellation kill the whole process group with SIGKILL at once, like the
//! reference (`process.kill(-pid, "SIGKILL")`). We deliberately do not send SIGTERM first: the
//! group id is only guaranteed not to be reused while the leader is unreaped, and a command that
//! traps TERM would otherwise delay Stop. The leader is then reaped; after it exits, stdio gets
//! 100 ms (re-armed by each chunk) before the call settles even if a descendant holds the pipes.
//!
//! Non-unix builds use the configured shell or `bash` from PATH, no process group, and
//! `Child::start_kill` for timeout and cancellation (descendants are not killed). Untested.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::path::spawn_error;
use super::process::{exit_code, kill_group, spawn_reader};
use super::text::{DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, StreamDecoder, js_number};
use super::{
    Diagnostic, OutputLimits, Retain, Severity, Tool, ToolCtx, ToolError, ToolOutput,
    spec_from_json,
};
use crate::provider::{BoxFuture, ToolSpec};

const SCHEMA: &str = r#"{"type":"object","required":["command"],"properties":{"command":{"type":"string","description":"Bash command to execute"},"timeout":{"type":"number","description":"Timeout in seconds (optional, no default timeout)"}}}"#;

const DESCRIPTION: &str = "Execute a bash command in the current working directory. Returns combined stdout and stderr. Output is truncated to last 2000 lines or 50KB (whichever is hit first). If truncated, full output is saved to a temp file. Optionally provide a timeout in seconds.";

const MAX_TIMEOUT_SECONDS: f64 = 2_147_483.647;
/// After the process exits, how long stdio may stay quiet before the call settles.
pub const EXIT_STDIO_GRACE: Duration = Duration::from_millis(100);

pub struct BashTool;

impl Tool for BashTool {
    fn spec(&self) -> ToolSpec {
        spec_from_json("bash", DESCRIPTION, SCHEMA)
    }

    fn limits(&self) -> OutputLimits {
        OutputLimits {
            retain: Retain::Tail,
            ..OutputLimits::default()
        }
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolCtx,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let command = args["command"].as_str().unwrap_or_default().to_owned();
            let timeout = args.get("timeout").and_then(Value::as_f64);
            validate_timeout(timeout)?;
            exec(&command, timeout, ctx).await
        })
    }
}

fn validate_timeout(timeout: Option<f64>) -> Result<(), ToolError> {
    let Some(t) = timeout else { return Ok(()) };
    if !t.is_finite() || t <= 0.0 {
        return Err(ToolError::new(
            "Invalid timeout: must be a finite number of seconds",
        ));
    }
    if t > MAX_TIMEOUT_SECONDS {
        return Err(ToolError::new(format!(
            "Invalid timeout: maximum is {} seconds",
            js_number(MAX_TIMEOUT_SECONDS)
        )));
    }
    Ok(())
}

fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            m.is_file() && m.permissions().mode() & 0o111 != 0
        }
        #[cfg(not(unix))]
        {
            m.is_file()
        }
    })
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable_file(candidate))
}

/// The shell: settings `shellPath` (must exist), else `/bin/bash`, else `bash` on PATH, else `sh`.
pub fn resolve_shell(custom: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(custom) = custom.filter(|p| !p.as_os_str().is_empty()) {
        return if custom.exists() {
            Ok(custom.to_path_buf())
        } else {
            Err(format!("Custom shell path not found: {}", custom.display()))
        };
    }
    #[cfg(unix)]
    {
        if Path::new("/bin/bash").exists() {
            return Ok(PathBuf::from("/bin/bash"));
        }
        if let Some(bash) = find_on_path("bash") {
            return Ok(bash);
        }
        Ok(PathBuf::from("sh"))
    }
    #[cfg(not(unix))]
    {
        Ok(find_on_path("bash.exe")
            .or_else(|| find_on_path("bash"))
            .unwrap_or_else(|| PathBuf::from("bash")))
    }
}

/// Raw output in arrival order, before and after the spill starts.
struct Spill {
    after_bytes: u64,
    after_lines: u64,
    seen_bytes: u64,
    seen_newlines: u64,
    prefix: Vec<Vec<u8>>,
    file: Option<tokio::fs::File>,
    path: Option<PathBuf>,
    error: Option<String>,
}

impl Spill {
    fn new() -> Spill {
        Spill {
            after_bytes: DEFAULT_MAX_BYTES as u64,
            after_lines: DEFAULT_MAX_LINES as u64,
            seen_bytes: 0,
            seen_newlines: 0,
            prefix: Vec::new(),
            file: None,
            path: None,
            error: None,
        }
    }

    /// Feed a raw chunk; returns `false` when the spill failed (the command must be stopped).
    async fn push(&mut self, chunk: &[u8]) -> bool {
        if self.error.is_some() || chunk.is_empty() {
            return self.error.is_none();
        }
        if self.file.is_none() {
            self.seen_bytes += chunk.len() as u64;
            self.seen_newlines += chunk.iter().filter(|b| **b == b'\n').count() as u64;
            let lines = self.seen_newlines + u64::from(chunk.last() != Some(&b'\n'));
            if self.seen_bytes <= self.after_bytes && lines <= self.after_lines {
                self.prefix.push(chunk.to_vec());
                return true;
            }
            match create_spill_file().await {
                Ok((path, file)) => {
                    self.path = Some(path);
                    self.file = Some(file);
                }
                Err(e) => {
                    self.fail(e);
                    return false;
                }
            }
            let prefix = std::mem::take(&mut self.prefix);
            for piece in prefix {
                if !self.write(&piece).await {
                    return false;
                }
            }
        }
        self.write(chunk).await
    }

    async fn write(&mut self, bytes: &[u8]) -> bool {
        let Some(file) = &mut self.file else {
            return true;
        };
        match file.write_all(bytes).await {
            Ok(()) => true,
            Err(e) => {
                self.fail(e.to_string());
                false
            }
        }
    }

    fn fail(&mut self, message: String) {
        if self.error.is_none() {
            self.error = Some(format!(
                "Failed to preserve complete shell output: {message}"
            ));
        }
    }

    async fn finish(&mut self) {
        if let Some(file) = &mut self.file
            && self.error.is_none()
            && let Err(e) = file.flush().await
        {
            self.fail(e.to_string());
        }
        self.file = None;
    }
}

/// `$TMPDIR/tmp-XXXXXX/pi-output-<uuid>.log`: a private directory (0700) holding a private
/// file (0600). Spill files are never deleted by the engine, like the reference.
async fn create_spill_file() -> Result<(PathBuf, tokio::fs::File), String> {
    let base = std::env::temp_dir();
    let dir = tokio::task::spawn_blocking(move || -> std::io::Result<PathBuf> {
        loop {
            let dir = base.join(format!("tmp-{}", crate::util::random_hex(3)));
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&dir) {
                Ok(()) => return Ok(dir),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    let path = dir.join(format!("pi-output-{}.log", crate::util::uuid_v4()));
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options.open(&path).await.map_err(|e| e.to_string())?;
    Ok((path, file))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stop {
    Cancelled,
    TimedOut,
    SpillFailed,
}

async fn exec(command: &str, timeout: Option<f64>, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
    if ctx.cancel.is_cancelled() {
        return Err(ToolError::new("aborted"));
    }
    let shell = resolve_shell(ctx.shell.as_deref()).map_err(ToolError::new)?;
    let cwd = ctx.cwd.clone();
    if !cwd.exists() {
        return Err(ToolError::new(format!(
            "Working directory does not exist: {}\nCannot execute bash commands.",
            cwd.display()
        )));
    }
    let _permit = tokio::select! {
        permit = ctx.processes.clone().acquire_owned() => permit
            .map_err(|_| ToolError::new("Tool processes are unavailable: the engine is shutting down"))?,
        _ = ctx.cancel.cancelled() => return Err(ToolError::new("aborted")),
    };

    let mut cmd = tokio::process::Command::new(&shell);
    cmd.arg("-c")
        .arg(command)
        .current_dir(&cwd)
        .envs(ctx.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd
        .spawn()
        .map_err(|e| ToolError::new(spawn_error(&e, &shell)))?;

    let (tx, mut rx) = mpsc::channel::<(usize, Vec<u8>)>(16);
    let mut readers = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        readers.push(spawn_reader(stdout, 0, tx.clone()));
    }
    if let Some(stderr) = child.stderr.take() {
        readers.push(spawn_reader(stderr, 1, tx.clone()));
    }
    drop(tx);

    let mut decoders = [StreamDecoder::default(), StreamDecoder::default()];
    let mut spill = Spill::new();
    let deadline = timeout.map(|t| Instant::now() + Duration::from_secs_f64(t));
    let mut stop: Option<Stop> = None;
    let mut exited: Option<std::process::ExitStatus> = None;
    let mut streams_open = true;
    let mut idle_until: Option<Instant> = None;

    loop {
        if exited.is_some() && !streams_open {
            break;
        }
        tokio::select! {
            biased;
            _ = ctx.cancel.cancelled(), if stop.is_none() && exited.is_none() => {
                stop = Some(Stop::Cancelled);
                kill_group(&mut child);
            }
            _ = sleep_until_opt(deadline), if deadline.is_some() && stop.is_none() && exited.is_none() => {
                stop = Some(Stop::TimedOut);
                kill_group(&mut child);
            }
            message = rx.recv(), if streams_open => match message {
                Some((stream, bytes)) => {
                    let text = decoders[stream].decode(&bytes, false);
                    if !text.is_empty() {
                        ctx.output(&text);
                    }
                    if !spill.push(&bytes).await && stop.is_none() {
                        stop = Some(Stop::SpillFailed);
                        kill_group(&mut child);
                    }
                    if exited.is_some() {
                        idle_until = Some(Instant::now() + EXIT_STDIO_GRACE);
                    }
                }
                None => streams_open = false,
            },
            status = child.wait(), if exited.is_none() => {
                exited = Some(status.map_err(|e| ToolError::new(e.to_string()))?);
                idle_until = Some(Instant::now() + EXIT_STDIO_GRACE);
            }
            _ = sleep_until_opt(idle_until), if exited.is_some() && idle_until.is_some() => break,
        }
    }
    // A descendant may still hold the pipes: stop reading them.
    for reader in &readers {
        reader.abort();
    }
    for decoder in &mut decoders {
        let text = decoder.decode(&[], true);
        if !text.is_empty() {
            ctx.output(&text);
        }
    }
    spill.finish().await;

    let mut diagnostics = Vec::new();
    if let Some(path) = &spill.path {
        diagnostics.push(Diagnostic::new(
            Severity::Info,
            "full_output",
            format!("Full output: {}", path.display()),
        ));
    }
    let fail = |message: String, diagnostics: Vec<Diagnostic>| ToolError {
        message,
        diagnostics,
        details: None,
    };
    if stop == Some(Stop::TimedOut) {
        return Err(fail(
            format!(
                "Command timed out after {} seconds",
                js_number(timeout.unwrap_or_default())
            ),
            diagnostics,
        ));
    }
    if stop == Some(Stop::Cancelled) || ctx.cancel.is_cancelled() {
        return Err(fail("Command aborted".into(), diagnostics));
    }
    if let Some(error) = spill.error.clone() {
        return Err(fail(error, diagnostics));
    }
    let code = exited.map_or(1, exit_code);
    if code != 0 {
        return Err(fail(
            format!("Command exited with code {code}"),
            diagnostics,
        ));
    }
    Ok(ToolOutput {
        content: None,
        diagnostics,
        ..ToolOutput::default()
    })
}

async fn sleep_until_opt(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}
