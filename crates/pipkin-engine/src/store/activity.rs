//! The raw facts behind the activity directory, read under one store lock so rows, counts and
//! the outbox cursor agree. Nothing here opens a session or touches a runtime; the runtime
//! composes these facts into rows (`runtime/activity.rs`).
use std::collections::{HashMap, HashSet};

use pipkin_engine_protocol::limits::SUBAGENT_TASK_EXCERPT_CHARS;

use super::*;

/// The newest run of a session, settled or not.
#[derive(Clone, Debug, PartialEq)]
pub struct LastRun {
    pub run: RunRow,
    pub ended_at_ms: Option<i64>,
}

/// The newest external (local Claude CLI) prompt of a session and its run's state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LastExternalRun {
    /// The external prompt id, which is what an external Stop is checked against.
    pub id: u64,
    pub state: String,
    pub outcome: Option<RunOutcome>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// What the directory needs of a child task. The task text arrives as an excerpt and the
/// result as a flag, so a directory read never loads task bodies, results or launch
/// instructions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityTask {
    pub root: String,
    pub child: u64,
    pub parent_session: String,
    pub child_session: String,
    /// The first `SUBAGENT_TASK_EXCERPT_CHARS` characters of the task.
    pub excerpt: String,
    pub status: String,
    pub has_result: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    /// The launch record's profile.
    pub profile: Option<String>,
    pub origin: String,
    pub note: Option<String>,
    /// The child this one follows up (contracts §7).
    pub continues: Option<u64>,
}

impl ActivityTask {
    pub fn finished(&self) -> bool {
        matches!(self.status.as_str(), "done" | "failed" | "aborted")
    }
}

#[derive(Debug, Default)]
pub struct ActivitySource {
    /// Every session that is not being removed, roots and children.
    pub sessions: Vec<SessionRow>,
    /// Child tasks by child session.
    pub tasks: HashMap<String, ActivityTask>,
    pub last_runs: HashMap<String, LastRun>,
    pub last_external: HashMap<String, LastExternalRun>,
    /// Sessions with an admitted queue entry.
    pub queued: HashSet<String>,
    /// Sessions with an unexpired pending CLI approval, by when the oldest was created.
    pub approvals: HashMap<String, i64>,
    pub jobs_active: HashMap<String, u32>,
    /// The newest outbox cursor visible to this read.
    pub through: u64,
}

impl Store {
    pub fn activity_source(&self, now: i64) -> Result<ActivitySource> {
        self.read(|c| {
            let mut out = ActivitySource::default();
            let mut stmt = c.prepare(
                "SELECT s.id,s.cwd,s.title,s.created_at,s.updated_at,s.origin,s.execution_backend,
                        s.model,s.thinking,s.subagents_enabled,s.parent,s.root,s.revision,s.blocked
                 FROM sessions s WHERE NOT EXISTS (SELECT 1 FROM session_removals r
                     WHERE r.root = s.id OR r.root = s.root)",
            )?;
            out.sessions = stmt
                .query_map([], |r| {
                    let origin: String = r.get(5)?;
                    Ok(SessionRow {
                        id: r.get(0)?, cwd: r.get(1)?, title: r.get(2)?,
                        created_at_ms: r.get(3)?, updated_at_ms: r.get(4)?,
                        origin: serde_json::from_str(&origin).unwrap_or_default(),
                        execution_backend: match r.get::<_, String>(6)?.as_str() {
                            "nativeAgent" => ExecutionBackend::NativeAgent,
                            "localClaudeCli" => ExecutionBackend::LocalClaudeCli,
                            "claudeCliAgent" => ExecutionBackend::ClaudeCliAgent,
                            _ => ExecutionBackend::Unsupported,
                        },
                        model: r.get(7)?, thinking: r.get(8)?,
                        subagents_enabled: r.get::<_, i64>(9)? != 0,
                        parent: r.get(10)?, root: r.get(11)?,
                        revision: r.get::<_, i64>(12)? as u64, blocked: r.get(13)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            // The literal excerpt limit must match the v35 expression index. Binding a
            // parameter here would make SQLite read the large source records again.
            let task_sql = format!(
                "SELECT root, child, parent_session, child_session, substr(task, 1, {SUBAGENT_TASK_EXCERPT_CHARS}),
                        status, result IS NOT NULL, created_at, updated_at,
                        CASE WHEN json_valid(launch) AND json_type(launch, '$.profile') = 'text'
                             THEN json_extract(launch, '$.profile') END,
                        origin, note, continues
                 FROM subagent_tasks INDEXED BY subagent_tasks_activity_cover",
            );
            let mut stmt = c.prepare(&task_sql)?;
            let tasks = stmt.query_map([], |r| {
                Ok(ActivityTask {
                    root: r.get(0)?,
                    child: r.get::<_, i64>(1)? as u64,
                    parent_session: r.get(2)?,
                    child_session: r.get(3)?,
                    excerpt: r.get(4)?,
                    status: r.get(5)?,
                    has_result: r.get(6)?,
                    created_at_ms: r.get(7)?,
                    updated_at_ms: r.get(8)?,
                    profile: r.get(9)?,
                    origin: r.get(10)?,
                    note: r.get(11)?,
                    continues: r.get::<_, Option<i64>>(12)?.map(|c| c as u64),
                })
            })?;
            for task in tasks {
                let task = task?;
                out.tasks.insert(task.child_session.clone(), task);
            }

            let mut stmt = c.prepare(
                "SELECT r.id,r.session,r.request,r.state,r.stop_requested_at,r.outcome,r.started_at,r.ended_at FROM runs r
                 JOIN (SELECT session, MAX(id) AS id FROM runs GROUP BY session) m ON m.id = r.id",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(LastRun {
                    run: RunRow {
                        id: r.get::<_, i64>(0)? as u64,
                        session: r.get(1)?, request: r.get(2)?,
                        state: match r.get::<_, String>(3)?.as_str() {
                            "running" => RunState::Running,
                            "stopping" => RunState::Stopping,
                            _ => RunState::Settled,
                        },
                        stop_requested: r.get::<_, Option<i64>>(4)?.is_some(),
                        outcome: r.get::<_, Option<String>>(5)?.and_then(|o| serde_json::from_str(&o).ok()),
                        started_at_ms: r.get(6)?,
                    },
                    ended_at_ms: r.get(7)?,
                })
            })?;
            for row in rows {
                let row = row?;
                out.last_runs.insert(row.run.session.clone(), row);
            }

            let mut stmt = c.prepare(
                "SELECT p.session, p.id, e.state, e.created_at, e.updated_at, p.outcome FROM external_prompts p
                 JOIN external_runs e
                   ON e.session = p.session AND e.generation = p.generation AND e.op = p.op
                 JOIN (SELECT session, MAX(id) AS id FROM external_prompts GROUP BY session) m
                   ON m.id = p.id",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    LastExternalRun {
                        id: r.get::<_, i64>(1)? as u64,
                        state: r.get(2)?,
                        outcome: r.get::<_, Option<String>>(5)?.map(|s| serde_json::from_str(&s)).transpose().map_err(|e| rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(e)))?,
                        created_at_ms: r.get(3)?,
                        updated_at_ms: r.get(4)?,
                    },
                ))
            })?;
            for row in rows {
                let (session, run) = row?;
                out.last_external.insert(session, run);
            }

            let mut stmt =
                c.prepare("SELECT DISTINCT session FROM requests WHERE state = 'queued'")?;
            for session in stmt.query_map([], |r| r.get::<_, String>(0))? {
                out.queued.insert(session?);
            }

            let mut stmt = c.prepare(
                "SELECT session, MIN(created_at) FROM external_permissions
                 WHERE state = 'pending' AND expires_at > ?1 GROUP BY session",
            )?;
            for row in stmt.query_map([now], |r| Ok((r.get::<_, String>(0)?, r.get(1)?)))? {
                let (session, at) = row?;
                out.approvals.insert(session, at);
            }

            let mut stmt = c.prepare(
                "SELECT session, COUNT(*) FROM jobs
                 WHERE state IN ('starting', 'running', 'cancelling') GROUP BY session",
            )?;
            for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
                let (session, n) = row?;
                out.jobs_active.insert(session, n as u32);
            }

            out.through = c.query_row(
                "SELECT COALESCE(MAX(cursor), 0) FROM event_outbox",
                [],
                |r| r.get::<_, i64>(0),
            )? as u64;
            Ok(out)
        })
    }
}
