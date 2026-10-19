//! Background jobs (`jobs`, schema 17): one row per supervised command, owned by a session.
//! The row is the durable record; the process belongs to the engine instance that spawned
//! it (`engine_epoch` + `nonce`, carried as the `PIPKIN_JOB` marker). Launch intent is
//! written before the spawn and the receipt (pid, started_at) after it, so a crash between
//! the two is recognizable: such a row is reported `unknown` and never relaunched.

use rusqlite::{Connection, OptionalExtension, params};

use super::{Result, Store};

/// Most jobs listed per session (active first, then the newest finished).
pub const MAX_LISTED_JOBS: usize = 60;

#[derive(Clone, Debug, PartialEq)]
pub struct JobRow {
    pub session: String,
    pub id: u64,
    pub name: Option<String>,
    pub command: String,
    pub cwd: String,
    /// `model` or `user`.
    pub origin: String,
    pub call_id: Option<String>,
    pub run: Option<u64>,
    pub task: Option<u64>,
    pub timeout_secs: Option<f64>,
    pub wake: bool,
    /// `starting`, `running`, `cancelling`, `succeeded`, `failed`, `timed_out`, `cancelled`,
    /// `interrupted`, `unknown`.
    pub state: String,
    pub exit_code: Option<i32>,
    pub reason: Option<String>,
    pub engine_epoch: u64,
    pub pid: Option<u32>,
    pub nonce: String,
    pub log_path: Option<String>,
    pub log_bytes: u64,
    pub log_truncated: bool,
    /// `pending`, `notified`, `woken`, `suppressed`, `none`.
    pub delivery: String,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
}

impl JobRow {
    pub fn active(&self) -> bool {
        matches!(self.state.as_str(), "starting" | "running" | "cancelling")
    }

    /// The marker value the process carries.
    pub fn token(&self) -> String {
        job_token(self.engine_epoch, &self.session, self.id, &self.nonce)
    }
}

pub fn job_token(epoch: u64, session: &str, id: u64, nonce: &str) -> String {
    format!("{epoch}/{session}/{id}/{nonce}")
}

const COLUMNS: &str = "session, id, name, command, cwd, origin, call_id, run, task, timeout_secs, wake, \
    state, exit_code, reason, engine_epoch, pid, nonce, log_path, log_bytes, log_truncated, delivery, \
    created_at, started_at, ended_at";

fn row(r: &rusqlite::Row) -> rusqlite::Result<JobRow> {
    Ok(JobRow {
        session: r.get("session")?,
        id: r.get::<_, i64>("id")? as u64,
        name: r.get("name")?,
        command: r.get("command")?,
        cwd: r.get("cwd")?,
        origin: r.get("origin")?,
        call_id: r.get("call_id")?,
        run: r.get::<_, Option<i64>>("run")?.map(|v| v as u64),
        task: r.get::<_, Option<i64>>("task")?.map(|v| v as u64),
        timeout_secs: r.get("timeout_secs")?,
        wake: r.get::<_, i64>("wake")? != 0,
        state: r.get("state")?,
        exit_code: r.get::<_, Option<i64>>("exit_code")?.map(|v| v as i32),
        reason: r.get("reason")?,
        engine_epoch: r.get::<_, i64>("engine_epoch")? as u64,
        pid: r.get::<_, Option<i64>>("pid")?.map(|v| v as u32),
        nonce: r.get("nonce")?,
        log_path: r.get("log_path")?,
        log_bytes: r.get::<_, i64>("log_bytes")? as u64,
        log_truncated: r.get::<_, i64>("log_truncated")? != 0,
        delivery: r.get("delivery")?,
        created_at: r.get("created_at")?,
        started_at: r.get("started_at")?,
        ended_at: r.get("ended_at")?,
    })
}

/// What a start records before the spawn.
pub struct JobIntent<'a> {
    pub name: Option<&'a str>,
    pub command: &'a str,
    pub cwd: &'a str,
    pub origin: &'a str,
    pub call_id: Option<&'a str>,
    pub run: Option<u64>,
    pub task: Option<u64>,
    pub timeout_secs: Option<f64>,
    pub wake: bool,
    pub engine_epoch: u64,
    pub nonce: &'a str,
    pub log_path: &'a str,
}

impl Store {
    /// None is a still-pending native result; false is missing/failed/retired handoff.
    pub(crate) fn cli_job_handoff(
        &self,
        session: &str,
        call: &str,
        run: u64,
    ) -> Result<Option<bool>> {
        self.read(|c|Ok(c.query_row("SELECT CASE WHEN bc.state='completed' AND t.state='done' AND json_extract(bc.result,'$.isError')=0 THEN 1 WHEN bc.state='intent' AND t.state='intent' THEN NULL ELSE 0 END FROM jobs j JOIN cli_bridge_calls bc ON bc.session=j.session AND bc.call_id=j.call_id JOIN cli_tool_bridges b ON b.id=bc.bridge AND b.session=j.session AND b.run=j.run JOIN tool_calls t ON t.session=j.session AND t.call_id=j.call_id AND t.run=j.run WHERE j.session=?1 AND j.call_id=?2 AND j.run=?3 AND bc.name='job_start' AND t.name=bc.name",params![session,call,run],|r|r.get::<_,Option<bool>>(0)).optional()?.unwrap_or(Some(false))))
    }
    /// Recovery may stop a marked process and settle evidence, never launch/resume it.
    /// A CLI session name alone is insufficient: join the durable authenticated native
    /// job_start call, native tool/run and immutable CLI binding that admitted this job.
    pub(crate) fn cli_job_cleanup_owned(c: &Connection, row: &JobRow) -> Result<bool> {
        Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM sessions s JOIN cli_agent_bindings a ON a.session=s.id
            JOIN cli_tool_bridges b ON b.session=s.id JOIN cli_bridge_calls bc ON bc.bridge=b.id AND bc.session=s.id
            JOIN tool_calls t ON t.session=s.id AND t.call_id=bc.call_id
            WHERE s.id=?1 AND s.execution_backend='claudeCliAgent' AND bc.call_id=?2 AND b.run=?3
            AND t.run=b.run AND bc.name='job_start' AND t.name='job_start')",
            params![row.session,row.call_id,row.run.map(|v|v as i64)],|r|r.get(0))?)
    }
    /// Record the launch intent (state `starting`, no pid yet).
    pub fn job_create(
        tx: &Connection,
        session: &str,
        intent: &JobIntent<'_>,
        now: i64,
    ) -> Result<JobRow> {
        Self::assert_session_live(tx, session)?;
        let id = tx.query_row(
            "SELECT COALESCE(MAX(id), 0) + 1 FROM jobs WHERE session = ?1",
            [session],
            |r| r.get::<_, i64>(0),
        )? as u64;
        tx.execute(
            &format!(
                "INSERT INTO jobs({COLUMNS}, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, \
                 'starting', NULL, NULL, ?12, NULL, ?13, ?14, 0, 0, 'pending', ?15, NULL, NULL, ?15)"
            ),
            params![
                session,
                id as i64,
                intent.name,
                intent.command,
                intent.cwd,
                intent.origin,
                intent.call_id,
                intent.run.map(|r| r as i64),
                intent.task.map(|t| t as i64),
                intent.timeout_secs,
                intent.wake as i64,
                intent.engine_epoch as i64,
                intent.nonce,
                intent.log_path,
                now,
            ],
        )?;
        Self::job(tx, session, id)?.ok_or(super::StoreError::Sqlite(
            rusqlite::Error::QueryReturnedNoRows,
        ))
    }

    /// The spawn succeeded: the process identity and the start time.
    pub fn job_receipt(tx: &Connection, session: &str, id: u64, pid: u32, now: i64) -> Result<()> {
        tx.execute(
            "UPDATE jobs SET state = 'running', pid = ?3, started_at = ?4, updated_at = ?4
             WHERE session = ?1 AND id = ?2 AND state = 'starting'",
            params![session, id as i64, pid as i64, now],
        )?;
        Ok(())
    }

    /// Move to a state; terminal states record the exit code, reason and end time. Returns
    /// whether the row changed (a job already settled stays as it was).
    pub fn job_set_state(
        tx: &Connection,
        session: &str,
        id: u64,
        state: &str,
        exit_code: Option<i32>,
        reason: Option<&str>,
        now: i64,
    ) -> Result<bool> {
        let terminal = !matches!(state, "starting" | "running" | "cancelling");
        let changed = tx.execute(
            "UPDATE jobs SET state = ?3, exit_code = COALESCE(?4, exit_code), reason = ?5,
                ended_at = CASE WHEN ?6 THEN ?7 ELSE ended_at END, updated_at = ?7
             WHERE session = ?1 AND id = ?2 AND state IN ('starting', 'running', 'cancelling')",
            params![
                session,
                id as i64,
                state,
                exit_code.map(|c| c as i64),
                reason,
                terminal,
                now
            ],
        )?;
        Ok(changed > 0)
    }

    pub fn job_log_progress(
        tx: &Connection,
        session: &str,
        id: u64,
        bytes: u64,
        truncated: bool,
        now: i64,
    ) -> Result<()> {
        tx.execute(
            "UPDATE jobs SET log_bytes = ?3, log_truncated = ?4, updated_at = ?5
             WHERE session = ?1 AND id = ?2",
            params![session, id as i64, bytes as i64, truncated as i64, now],
        )?;
        Ok(())
    }

    /// Record how completion was delivered; only a pending delivery can be set (idempotent
    /// across restarts: a delivered job is never delivered again).
    pub fn job_set_delivery(
        tx: &Connection,
        session: &str,
        id: u64,
        delivery: &str,
        now: i64,
    ) -> Result<bool> {
        let changed = tx.execute(
            "UPDATE jobs SET delivery = ?3, updated_at = ?4
             WHERE session = ?1 AND id = ?2 AND delivery = 'pending'",
            params![session, id as i64, delivery, now],
        )?;
        Ok(changed > 0)
    }

    pub fn job(c: &Connection, session: &str, id: u64) -> Result<Option<JobRow>> {
        Ok(c.query_row(
            &format!("SELECT {COLUMNS} FROM jobs WHERE session = ?1 AND id = ?2"),
            params![session, id as i64],
            row,
        )
        .optional()?)
    }

    /// Active jobs first (oldest first), then finished ones newest first, bounded. The flag
    /// says whether older finished jobs were left out.
    pub fn jobs(c: &Connection, session: &str) -> Result<(Vec<JobRow>, bool)> {
        let mut stmt = c.prepare_cached(&format!(
            "SELECT {COLUMNS} FROM jobs WHERE session = ?1 ORDER BY id"
        ))?;
        let all: Vec<JobRow> = stmt
            .query_map([session], row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut active: Vec<JobRow> = all.iter().filter(|j| j.active()).cloned().collect();
        let mut finished: Vec<JobRow> = all.into_iter().filter(|j| !j.active()).collect();
        finished.reverse();
        let room = MAX_LISTED_JOBS.saturating_sub(active.len());
        let truncated = finished.len() > room;
        finished.truncate(room);
        active.extend(finished);
        Ok((active, truncated))
    }

    pub fn jobs_active_count(c: &Connection, session: &str) -> Result<usize> {
        Ok(c.query_row(
            "SELECT COUNT(*) FROM jobs WHERE session = ?1 AND state IN ('starting', 'running', 'cancelling')",
            [session],
            |r| r.get::<_, i64>(0),
        )? as usize)
    }

    /// Every job still marked active, across sessions (startup reconciliation).
    pub fn jobs_active_all(c: &Connection) -> Result<Vec<JobRow>> {
        let mut stmt = c.prepare_cached(&format!(
            "SELECT {COLUMNS} FROM jobs WHERE state IN ('starting', 'running', 'cancelling') ORDER BY session, id"
        ))?;
        Ok(stmt
            .query_map([], row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Finished jobs whose completion was never delivered (startup catch-up).
    pub fn jobs_undelivered(c: &Connection) -> Result<Vec<JobRow>> {
        let mut stmt = c.prepare_cached(&format!(
            "SELECT {COLUMNS} FROM jobs WHERE delivery = 'pending' AND state NOT IN ('starting', 'running', 'cancelling') ORDER BY session, id"
        ))?;
        Ok(stmt
            .query_map([], row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The outcome JSON of a settled run, if any (`None` while open or unknown).
    pub fn run_outcome(c: &Connection, run: u64) -> Result<Option<String>> {
        Ok(c.query_row(
            "SELECT outcome FROM runs WHERE id = ?1",
            [run as i64],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
    }
}
