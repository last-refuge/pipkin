//! Background jobs (plan §9, JOB-1): commands supervised by the engine outside the foreground
//! model turn. A job's durable record is a `jobs` row; its process belongs to this engine
//! instance (epoch + nonce, carried as the `PIPKIN_JOB` marker on the process). The model
//! starts a job and gets its id back at once; it reads the log, waits with a bound, or
//! cancels. Completion is a durable delivery: the session's clients see `JobsChanged`, and a
//! model-origin job that asked to be woken gets one follow-up under a fixed request key
//! (idempotent across restarts). A job started by a run the person stopped never wakes the
//! model.
//!
//! Boundaries: a job lives as long as this engine process. On startup, rows still marked
//! active belong to a dead supervisor: processes carrying an old marker are killed, running
//! rows become `interrupted`, and rows without a spawn receipt become `unknown` (the command
//! may or may not have run; it is never relaunched). Logs survive as private files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pipkin_engine_protocol::*;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;

use super::Shared;
use crate::store::Store;
use crate::store::jobs::{JobIntent, JobRow, job_token};
use crate::tools::process::{exit_code, kill_group, spawn_reader};
use crate::util::now_ms;

/// The marker a job's process (and its descendants) carries.
pub const JOB_ENV: &str = "PIPKIN_JOB";
/// Active jobs per session.
pub const MAX_ACTIVE_JOBS: usize = 8;
/// Log quota per job; later output is dropped and the log marked truncated.
pub const MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;
/// Largest log slice one read returns.
pub const MAX_LOG_SLICE: u64 = 64 * 1024;
pub const DEFAULT_LOG_SLICE: u64 = 8 * 1024;
/// Longest a single `job_wait` blocks.
pub const MAX_WAIT_SECS: u64 = 600;
/// Longest a job may run when a timeout is given.
pub const MAX_TIMEOUT_SECS: f64 = 86_400.0;
/// Tail included in the completion follow-up.
const WAKE_TAIL_BYTES: u64 = 2 * 1024;
/// How often log growth is committed and published while a job runs.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(500);
const EXIT_STDIO_GRACE: Duration = Duration::from_millis(100);

/// What a start asks for.
#[derive(Clone, Debug, PartialEq)]
pub struct JobSpec {
    pub command: String,
    pub name: Option<String>,
    pub cwd: Option<String>,
    pub timeout_secs: Option<f64>,
    pub wake: bool,
    pub task: Option<u64>,
}

/// A supervised process this engine owns.
struct Running {
    cancel: CancellationToken,
    done: Arc<Notify>,
}

#[derive(Default)]
pub struct JobsRuntime {
    running: Mutex<HashMap<(String, u64), Running>>,
}

fn invalid(message: impl Into<String>) -> ProtoError {
    ProtoError::new(ErrorCode::Invalid, message.into())
}

pub fn view_of(row: &JobRow) -> JobView {
    JobView {
        id: row.id,
        name: row.name.clone(),
        command: row.command.clone(),
        cwd: row.cwd.clone(),
        origin: row.origin.clone(),
        state: JobState::parse(&row.state),
        exit_code: row.exit_code,
        reason: row.reason.clone(),
        call_id: row.call_id.clone(),
        run: row.run,
        task: row.task,
        timeout_secs: row.timeout_secs,
        wake: row.wake,
        delivery: row.delivery.clone(),
        log_bytes: row.log_bytes,
        log_truncated: row.log_truncated,
        created_at_ms: row.created_at,
        started_at_ms: row.started_at,
        ended_at_ms: row.ended_at,
    }
}

fn jobs_view(rows: &[JobRow], truncated: bool) -> JobsView {
    JobsView {
        active: rows.iter().filter(|r| r.active()).count(),
        jobs: rows.iter().map(view_of).collect(),
        truncated,
    }
}

impl Shared {
    fn jobs_dir(&self, session: &str) -> PathBuf {
        self.config.data_dir.join("jobs").join(session)
    }

    pub fn jobs_view(&self, session: &str) -> Result<JobsView, ProtoError> {
        if self.store.session(session)?.is_none() {
            return Err(ProtoError::new(
                ErrorCode::NotFound,
                format!("Session {session} does not exist"),
            ));
        }
        let (rows, truncated) = self.store.read(|c| Store::jobs(c, session))?;
        Ok(jobs_view(&rows, truncated))
    }

    pub fn job_view(&self, session: &str, id: u64) -> Result<JobView, ProtoError> {
        let row = self
            .store
            .read(|c| Store::job(c, session, id))?
            .ok_or_else(|| ProtoError::new(ErrorCode::NotFound, format!("No job #{id}")))?;
        Ok(view_of(&row))
    }

    /// Start a job: intent committed, process spawned, receipt committed, supervisor spawned.
    /// `origin` is `model` (with the call and run) or `user`.
    pub async fn job_start(
        self: &Arc<Self>,
        session: &str,
        origin: &str,
        call_id: Option<&str>,
        run: Option<u64>,
        spec: JobSpec,
    ) -> Result<JobView, ProtoError> {
        self.require_native_execution(session)?;
        let command = spec.command.trim().to_owned();
        if command.is_empty() {
            return Err(invalid("The job needs a command"));
        }
        if let Some(t) = spec.timeout_secs
            && (!t.is_finite() || t <= 0.0 || t > MAX_TIMEOUT_SECS)
        {
            return Err(invalid(format!(
                "timeout must be between 0 and {MAX_TIMEOUT_SECS} seconds"
            )));
        }
        let sess = self.session(session)?;
        let row = self
            .store
            .session(session)?
            .ok_or_else(|| ProtoError::new(ErrorCode::NotFound, "Session does not exist"))?;
        let cwd = match &spec.cwd {
            Some(c) if !c.trim().is_empty() => {
                let p = Path::new(c.trim());
                if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    Path::new(&row.cwd).join(p)
                }
            }
            _ => PathBuf::from(&row.cwd),
        };
        if !cwd.is_dir() {
            return Err(invalid(format!(
                "Working directory does not exist: {}",
                cwd.display()
            )));
        }
        let settings = self.settings_for(session);
        let shell =
            crate::tools::bash::resolve_shell(settings.shell_path.as_deref().map(Path::new))
                .map_err(invalid)?;
        let active = self.store.read(|c| Store::jobs_active_count(c, session))?;
        if active >= MAX_ACTIVE_JOBS {
            return Err(ProtoError::new(
                ErrorCode::TooLarge,
                format!(
                    "At most {MAX_ACTIVE_JOBS} jobs may run at once in a session; cancel one or wait"
                ),
            ));
        }
        let nonce = crate::util::random_hex(8);
        let dir = self.jobs_dir(session);
        std::fs::create_dir_all(&dir).map_err(|e| {
            ProtoError::new(
                ErrorCode::Storage,
                format!("cannot create the jobs directory: {e}"),
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        // 1. Intent (durable before any effect).
        let epoch = self.epoch;
        let cwd_text = cwd.display().to_string();
        let name = spec
            .name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(|n| n.chars().take(80).collect::<String>());
        let created = self.commit(&sess, |tx| {
            // The log path needs the id; reserve by creating with a placeholder, then fix.
            let row = Store::job_create(
                tx,
                session,
                &JobIntent {
                    name: name.as_deref(),
                    command: &command,
                    cwd: &cwd_text,
                    origin,
                    call_id,
                    run,
                    task: spec.task,
                    timeout_secs: spec.timeout_secs,
                    wake: spec.wake && origin == "model",
                    engine_epoch: epoch,
                    nonce: &nonce,
                    log_path: "",
                },
                now_ms(),
            )?;
            let log_path = dir.join(format!("{}.log", row.id));
            tx.execute(
                "UPDATE jobs SET log_path = ?3 WHERE session = ?1 AND id = ?2",
                rusqlite::params![session, row.id as i64, log_path.display().to_string()],
            )?;
            let (rows, truncated) = Store::jobs(tx, session)?;
            Ok((
                row.id,
                vec![SessionChange::JobsChanged {
                    jobs: jobs_view(&rows, truncated),
                }],
            ))
        })?;
        let id = created;
        let log_path = dir.join(format!("{id}.log"));
        super::faults::crash_point("after_job_intent");
        // 2. Spawn.
        let token = job_token(epoch, session, id, &nonce);
        let mut env: Vec<(String, String)> =
            super::run::tool_env(session, call_id.unwrap_or("user"), None, None);
        env.push((JOB_ENV.to_owned(), token.clone()));
        let mut cmd = tokio::process::Command::new(&shell);
        cmd.arg("-c")
            .arg(&command)
            .current_dir(&cwd)
            .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0);
        let child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => {
                let reason = crate::tools::path::spawn_error(&e, &shell);
                self.job_settle(session, id, "failed", None, Some(&reason))
                    .await;
                return Err(invalid(format!("The job could not start: {reason}")));
            }
        };
        let pid = child.id().unwrap_or(0);
        // 3. Receipt.
        self.commit(&sess, |tx| {
            Store::job_receipt(tx, session, id, pid, now_ms())?;
            let (rows, truncated) = Store::jobs(tx, session)?;
            Ok((
                (),
                vec![SessionChange::JobsChanged {
                    jobs: jobs_view(&rows, truncated),
                }],
            ))
        })?;
        super::faults::crash_point("after_job_receipt");
        // 4. Supervise.
        let cancel = CancellationToken::new();
        let done = Arc::new(Notify::new());
        self.jobs
            .running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                (session.to_owned(), id),
                Running {
                    cancel: cancel.clone(),
                    done: done.clone(),
                },
            );
        let shared = self.clone();
        let session_owned = session.to_owned();
        let timeout = spec.timeout_secs;
        self.spawn(async move {
            shared
                .supervise(session_owned, id, child, log_path, timeout, cancel, done)
                .await;
        });
        self.job_view(session, id)
    }

    #[allow(clippy::too_many_arguments)]
    async fn supervise(
        self: Arc<Self>,
        session: String,
        id: u64,
        mut child: tokio::process::Child,
        log_path: PathBuf,
        timeout: Option<f64>,
        cancel: CancellationToken,
        done: Arc<Notify>,
    ) {
        let (tx, mut rx) = mpsc::channel::<(usize, Vec<u8>)>(16);
        let mut readers = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            readers.push(spawn_reader(stdout, 0, tx.clone()));
        }
        if let Some(stderr) = child.stderr.take() {
            readers.push(spawn_reader(stderr, 1, tx.clone()));
        }
        drop(tx);
        let mut log = {
            let mut options = tokio::fs::OpenOptions::new();
            options.create(true).append(true);
            #[cfg(unix)]
            options.mode(0o600);
            options.open(&log_path).await.ok()
        };
        let mut written: u64 = 0;
        let mut truncated = false;
        let mut dirty = false;
        let mut last_progress = tokio::time::Instant::now();
        let deadline = timeout.map(|t| tokio::time::Instant::now() + Duration::from_secs_f64(t));
        let mut outcome: Option<(&str, Option<i32>, Option<String>)> = None;
        let mut exited = false;
        let mut streams_open = true;
        let mut idle_until: Option<tokio::time::Instant> = None;
        loop {
            if exited && !streams_open {
                break;
            }
            tokio::select! {
                biased;
                _ = cancel.cancelled(), if outcome.is_none() && !exited => {
                    outcome = Some(("cancelled", None, Some("Stopped".into())));
                    self.job_mark(&session, id, "cancelling", None, None).await;
                    kill_group(&mut child);
                }
                _ = sleep_until_opt(deadline), if deadline.is_some() && outcome.is_none() && !exited => {
                    outcome = Some(("timed_out", None, Some(format!(
                        "Timed out after {} seconds",
                        crate::tools::text::js_number(timeout.unwrap_or_default())
                    ))));
                    kill_group(&mut child);
                }
                message = rx.recv(), if streams_open => match message {
                    Some((_, bytes)) => {
                        if !truncated {
                            let room = MAX_LOG_BYTES.saturating_sub(written);
                            let take = (bytes.len() as u64).min(room) as usize;
                            if take > 0
                                && let Some(file) = log.as_mut()
                                && file.write_all(&bytes[..take]).await.is_ok()
                            {
                                written += take as u64;
                                dirty = true;
                            }
                            if (bytes.len() as u64) > room {
                                truncated = true;
                                dirty = true;
                                log = None;
                            }
                        }
                        if dirty && last_progress.elapsed() >= PROGRESS_INTERVAL {
                            self.job_progress(&session, id, written, truncated).await;
                            dirty = false;
                            last_progress = tokio::time::Instant::now();
                        }
                        if exited {
                            idle_until = Some(tokio::time::Instant::now() + EXIT_STDIO_GRACE);
                        }
                    }
                    None => streams_open = false,
                },
                status = child.wait(), if !exited => {
                    exited = true;
                    idle_until = Some(tokio::time::Instant::now() + EXIT_STDIO_GRACE);
                    if outcome.is_none() {
                        match status {
                            Ok(status) => {
                                let code = exit_code(status);
                                outcome = Some((
                                    if code == 0 { "succeeded" } else { "failed" },
                                    Some(code),
                                    (code != 0).then(|| format!("Exited with code {code}")),
                                ));
                            }
                            Err(e) => outcome = Some(("failed", None, Some(e.to_string()))),
                        }
                    }
                }
                _ = sleep_until_opt(idle_until), if exited && idle_until.is_some() => break,
                // A quiet job still reports what it wrote so far.
                _ = tokio::time::sleep_until(last_progress + PROGRESS_INTERVAL), if dirty => {
                    self.job_progress(&session, id, written, truncated).await;
                    dirty = false;
                    last_progress = tokio::time::Instant::now();
                }
            }
        }
        for reader in &readers {
            reader.abort();
        }
        if let Some(file) = log.as_mut() {
            let _ = file.flush().await;
        }
        // The group leader is reaped; stragglers carrying the marker are killed too.
        let (state, code, reason) =
            outcome.unwrap_or(("failed", None, Some("exited without a status".into())));
        if state != "succeeded" {
            let token = self
                .store
                .read(|c| Store::job(c, &session, id))
                .ok()
                .flatten()
                .map(|r| r.token());
            if let Some(token) = token {
                let _ = tokio::task::spawn_blocking(move || kill_marked(&token)).await;
            }
        }
        self.job_progress(&session, id, written, truncated).await;
        self.job_settle(&session, id, state, code, reason.as_deref())
            .await;
        self.jobs
            .running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(session.clone(), id));
        done.notify_waiters();
    }

    async fn job_progress(&self, session: &str, id: u64, bytes: u64, truncated: bool) {
        let Ok(sess) = self.session(session) else {
            return;
        };
        let _ = self.commit(&sess, |tx| {
            Store::job_log_progress(tx, session, id, bytes, truncated, now_ms())?;
            let (rows, t) = Store::jobs(tx, session)?;
            Ok((
                (),
                vec![SessionChange::JobsChanged {
                    jobs: jobs_view(&rows, t),
                }],
            ))
        });
    }

    async fn job_mark(
        &self,
        session: &str,
        id: u64,
        state: &str,
        code: Option<i32>,
        reason: Option<&str>,
    ) {
        let Ok(sess) = self.session(session) else {
            return;
        };
        let _ = self.commit(&sess, |tx| {
            Store::job_set_state(tx, session, id, state, code, reason, now_ms())?;
            let (rows, t) = Store::jobs(tx, session)?;
            Ok((
                (),
                vec![SessionChange::JobsChanged {
                    jobs: jobs_view(&rows, t),
                }],
            ))
        });
    }

    /// Terminal state, then completion delivery.
    async fn job_settle(
        self: &Arc<Self>,
        session: &str,
        id: u64,
        state: &str,
        code: Option<i32>,
        reason: Option<&str>,
    ) {
        self.job_mark(session, id, state, code, reason).await;
        self.job_deliver(session, id).await;
    }

    /// Deliver a finished job's completion once: the clients already saw `JobsChanged`; a
    /// model-origin job with `wake` gets one follow-up unless the run that started it was
    /// stopped by the person, in which case delivery is `suppressed`.
    async fn job_deliver(self: &Arc<Self>, session: &str, id: u64) {
        let Ok(Some(row)) = self.store.read(|c| Store::job(c, session, id)) else {
            return;
        };
        if row.active() || row.delivery != "pending" {
            return;
        }
        let stopped = row
            .run
            .and_then(|run| {
                self.store
                    .read(|c| Store::run_outcome(c, run))
                    .ok()
                    .flatten()
            })
            .is_some_and(|outcome| outcome.contains("aborted"));
        let delivery = if !row.wake || row.origin != "model" {
            "notified"
        } else if stopped {
            "suppressed"
        } else {
            "woken"
        };
        if delivery == "woken" {
            let key = format!("job:{id}:done");
            let text = self.wake_text(&row);
            match self
                .enqueue(session, &key, InputMode::FollowUp, &text, &[])
                .await
            {
                Ok(_) => {}
                Err(e) if e.code == ErrorCode::Conflict => {}
                Err(e) => {
                    log::warn!(
                        "job {session}/{id}: completion could not wake the model: {}",
                        e.message
                    );
                    self.job_set_delivery(session, id, "notified");
                    return;
                }
            }
        }
        self.job_set_delivery(session, id, delivery);
    }

    fn job_set_delivery(&self, session: &str, id: u64, delivery: &str) {
        let Ok(sess) = self.session(session) else {
            return;
        };
        let _ = self.commit(&sess, |tx| {
            Store::job_set_delivery(tx, session, id, delivery, now_ms())?;
            let (rows, t) = Store::jobs(tx, session)?;
            Ok((
                (),
                vec![SessionChange::JobsChanged {
                    jobs: jobs_view(&rows, t),
                }],
            ))
        });
    }

    fn wake_text(&self, row: &JobRow) -> String {
        let label = row.name.clone().unwrap_or_else(|| row.command.clone());
        let elapsed = match (row.started_at, row.ended_at) {
            (Some(s), Some(e)) if e >= s => format!(" after {}", format_elapsed(e - s)),
            _ => String::new(),
        };
        let mut text = format!(
            "Background job #{} ({label}) {}{elapsed}.",
            row.id,
            describe_state(&row.state, row.exit_code)
        );
        if let Some(reason) = &row.reason
            && row.state != "failed"
        {
            text.push_str(&format!(" {reason}"));
        }
        let tail_from = row.log_bytes.saturating_sub(WAKE_TAIL_BYTES);
        if let Ok(slice) = self.job_log(&row.session, row.id, tail_from, Some(WAKE_TAIL_BYTES))
            && !slice.text.trim().is_empty()
        {
            text.push_str(&format!(
                "\nLast output{}:\n```\n{}\n```",
                if tail_from > 0 { " (tail)" } else { "" },
                slice.text.trim_end()
            ));
        }
        text.push_str(&format!(
            "\nUse job_log with id {} for the full log.",
            row.id
        ));
        text
    }

    /// A slice of the log file by byte offset, bounded.
    pub fn job_log(
        &self,
        session: &str,
        id: u64,
        offset: u64,
        limit: Option<u64>,
    ) -> Result<JobLogView, ProtoError> {
        let row = self
            .store
            .read(|c| Store::job(c, session, id))?
            .ok_or_else(|| ProtoError::new(ErrorCode::NotFound, format!("No job #{id}")))?;
        let limit = limit.unwrap_or(DEFAULT_LOG_SLICE).clamp(1, MAX_LOG_SLICE);
        let total = row.log_bytes;
        let mut text = String::new();
        let mut next = offset.min(total);
        if let Some(path) = &row.log_path
            && offset < total
        {
            use std::io::{Read, Seek, SeekFrom};
            if let Ok(mut file) = std::fs::File::open(path) {
                let end = (offset + limit).min(total);
                let mut buf = vec![0u8; (end - offset) as usize];
                if file.seek(SeekFrom::Start(offset)).is_ok() {
                    let mut got = 0;
                    while got < buf.len() {
                        match file.read(&mut buf[got..]) {
                            Ok(0) => break,
                            Ok(n) => got += n,
                            Err(_) => break,
                        }
                    }
                    buf.truncate(got);
                }
                next = offset + buf.len() as u64;
                text = String::from_utf8_lossy(&buf).into_owned();
            }
        }
        Ok(JobLogView {
            id,
            offset: offset.min(total),
            text,
            next_offset: next,
            total_bytes: total,
            truncated: row.log_truncated,
            done: !row.active(),
        })
    }

    /// Stop a job's whole process group. Finished jobs are left as they are.
    pub async fn job_cancel(
        self: &Arc<Self>,
        session: &str,
        id: u64,
    ) -> Result<JobView, ProtoError> {
        let row = self
            .store
            .read(|c| Store::job(c, session, id))?
            .ok_or_else(|| ProtoError::new(ErrorCode::NotFound, format!("No job #{id}")))?;
        if !row.active() {
            return Ok(view_of(&row));
        }
        let running = self
            .jobs
            .running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(session.to_owned(), id))
            .map(|r| (r.cancel.clone(), r.done.clone()));
        match running {
            Some((cancel, done)) => {
                let waiter = done.notified();
                tokio::pin!(waiter);
                waiter.as_mut().enable();
                cancel.cancel();
                let _ = tokio::time::timeout(Duration::from_secs(10), waiter).await;
            }
            None => {
                // No supervisor in this process (a row from before a restart that recovery
                // has not reached yet): kill by marker and settle it here.
                let token = row.token();
                let _ = tokio::task::spawn_blocking(move || kill_marked(&token)).await;
                self.job_settle(session, id, "cancelled", None, Some("Stopped"))
                    .await;
            }
        }
        self.job_view(session, id)
    }

    /// Wait (bounded) for a job to finish; returns its view either way.
    pub async fn job_wait(
        &self,
        session: &str,
        id: u64,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<JobView, ProtoError> {
        let deadline =
            tokio::time::Instant::now() + timeout.min(Duration::from_secs(MAX_WAIT_SECS));
        loop {
            let view = self.job_view(session, id)?;
            if !view.state.active() {
                return Ok(view);
            }
            let done = self
                .jobs
                .running
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&(session.to_owned(), id))
                .map(|r| r.done.clone());
            let Some(done) = done else {
                // Active in the store but not supervised here: a stale row; recovery marks it.
                return Ok(view);
            };
            let notified = done.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            // Settlement happens after the notify; re-read on wake, poll as a safety net.
            tokio::select! {
                _ = notified => {}
                _ = tokio::time::sleep(Duration::from_millis(250)) => {}
                _ = tokio::time::sleep_until(deadline) => return self.job_view(session, id),
                _ = cancel.cancelled() => return self.job_view(session, id),
            }
        }
    }

    /// Startup: rows still active belong to a dead supervisor.
    pub(super) fn jobs_recover(self: &Arc<Self>) {
        let Ok(rows) = self.store.read(Store::jobs_active_all) else {
            return;
        };
        for row in rows {
            if self.require_native_execution(&row.session).is_err() {
                continue;
            }
            if row.engine_epoch == self.epoch {
                continue;
            }
            let (state, reason) = if row.pid.is_none() {
                (
                    "unknown",
                    "The engine stopped between recording the job and starting it; it may or may not have run",
                )
            } else {
                let killed = kill_marked(&row.token());
                if killed > 0 {
                    (
                        "interrupted",
                        "The engine stopped while it ran; its process was stopped on restart",
                    )
                } else {
                    (
                        "interrupted",
                        "The engine stopped while it ran; the process is gone",
                    )
                }
            };
            let session = row.session.clone();
            if let Ok(sess) = self.session(&session) {
                let _ = self.commit(&sess, |tx| {
                    Store::job_set_state(
                        tx,
                        &session,
                        row.id,
                        state,
                        None,
                        Some(reason),
                        now_ms(),
                    )?;
                    // Nothing to deliver for an interrupted job beyond the state itself.
                    Store::job_set_delivery(tx, &session, row.id, "none", now_ms())?;
                    let (rows, t) = Store::jobs(tx, &session)?;
                    Ok((
                        (),
                        vec![SessionChange::JobsChanged {
                            jobs: jobs_view(&rows, t),
                        }],
                    ))
                });
            }
        }
        // Completions whose delivery was cut off by the restart.
        let shared = self.clone();
        self.spawn(async move {
            let Ok(rows) = shared.store.read(Store::jobs_undelivered) else {
                return;
            };
            for row in rows {
                if shared.require_native_execution(&row.session).is_ok() {
                    shared.job_deliver(&row.session, row.id).await;
                }
            }
        });
    }

    /// Orderly shutdown: every supervised job is stopped and marked interrupted.
    pub(super) fn jobs_shutdown(&self) -> Vec<Arc<Notify>> {
        let running: Vec<(CancellationToken, Arc<Notify>)> = self
            .jobs
            .running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|r| (r.cancel.clone(), r.done.clone()))
            .collect();
        running
            .into_iter()
            .map(|(cancel, done)| {
                cancel.cancel();
                done
            })
            .collect()
    }

    /// A session being removed takes its jobs with it.
    pub(super) async fn jobs_remove_session(self: &Arc<Self>, session: &str) {
        let keys: Vec<(String, u64)> = self
            .jobs
            .running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .filter(|(s, _)| s == session)
            .cloned()
            .collect();
        for (s, id) in keys {
            let _ = self.job_cancel(&s, id).await;
        }
        let _ = std::fs::remove_dir_all(self.jobs_dir(session));
    }
}

async fn sleep_until_opt(at: Option<tokio::time::Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// Kill every process carrying this job's marker (Linux: matched by environment).
pub fn kill_marked(token: &str) -> usize {
    super::faults::kill_marked_env(JOB_ENV, token)
}

pub fn describe_state(state: &str, code: Option<i32>) -> String {
    match state {
        "succeeded" => "succeeded (exit code 0)".into(),
        "failed" => match code {
            Some(c) => format!("failed (exit code {c})"),
            None => "failed".into(),
        },
        "timed_out" => "timed out".into(),
        "cancelled" => "was cancelled".into(),
        "interrupted" => "was interrupted".into(),
        "unknown" => "has an unknown outcome".into(),
        "cancelling" => "is being cancelled".into(),
        "running" => "is running".into(),
        _ => "is starting".into(),
    }
}

pub fn format_elapsed(ms: i64) -> String {
    let secs = ms / 1000;
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}
