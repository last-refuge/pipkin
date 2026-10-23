//! The engine-wide activity directory (protocol 42, `docs/orchestration-dashboard-contracts.md`
//! §3–4). Rows are composed from one store read plus the open questions of runtimes that are
//! already live. Nothing here opens, recovers, attaches or materialises a session, runs an
//! extension or probes a CLI.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use pipkin_engine_protocol::limits::{
    MAX_ACTIVITY_PAGE, MAX_ACTIVITY_ROWS, MAX_ACTIVITY_TEXT_CHARS, MAX_OBSERVE_CHILDREN,
    MAX_PAGE_ENTRIES,
};
use pipkin_engine_protocol::*;

use super::Shared;
use super::session::SessionRt;
use crate::store::activity::{ActivitySource, ActivityTask, LastExternalRun, LastRun};
use crate::store::{RunState, SessionRow, Store};
use crate::util::now_ms;

/// How long a settled worker stays in Finished (owner decision, contracts §10).
pub(super) const RECENT_MS: i64 = 24 * 60 * 60 * 1000;

/// One composed worker: its row plus what grouping, ordering and filtering need.
#[derive(Clone)]
struct Worker {
    row: ActivityRow,
    group: Option<ActivityGroup>,
    /// When the worker started needing attention, or its run started, or it settled.
    anchor: i64,
    failed: bool,
    /// Case-folded title, task excerpt and cwd.
    haystack: String,
    /// The task's own row, for children.
    task: Option<(String, u64, String)>,
}

impl Worker {
    fn sort_at(&self, group: ActivityGroup) -> i64 {
        match group {
            ActivityGroup::All => self.row.created_at_ms,
            ActivityGroup::Overview => match self.group {
                Some(ActivityGroup::Attention) => 30_000_000_000_000_000 - self.anchor,
                Some(ActivityGroup::Running) => 20_000_000_000_000_000 + self.anchor,
                _ => 10_000_000_000_000_000 + self.anchor,
            },
            _ => self.anchor,
        }
    }

    fn with_sort_at(&self, group: ActivityGroup) -> ActivityRow {
        let mut row = self.row.clone();
        row.sort_at_ms = self.sort_at(group);
        row
    }
}

/// The composed directory at one instant.
#[derive(Clone)]
struct Directory {
    workers: Arc<Vec<Worker>>,
    through: u64,
}

fn coverage() -> Coverage {
    Coverage {
        roots: true,
        children: true,
        live_attention: true,
        excluded: vec![CoverageGap::CliInternalWorkers, CoverageGap::ExternalAgents],
    }
}

fn invalid(message: impl Into<String>) -> ProtoError {
    ProtoError::new(ErrorCode::Invalid, message)
}

fn group_wire(group: ActivityGroup) -> &'static str {
    match group {
        ActivityGroup::Overview => "overview",
        ActivityGroup::Attention => "attention",
        ActivityGroup::Running => "running",
        ActivityGroup::Finished => "finished",
        ActivityGroup::All => "all",
    }
}

/// Oldest first for Attention (it is a queue), newest first otherwise.
fn ascending(group: ActivityGroup) -> bool {
    group == ActivityGroup::Attention
}

fn encode_cursor(group: ActivityGroup, sort_at: i64, session: &str) -> ActivityCursor {
    ActivityCursor(format!("{}:{sort_at}:{session}", group_wire(group)))
}

fn decode_cursor(
    group: ActivityGroup,
    cursor: &ActivityCursor,
) -> Result<(i64, String), ProtoError> {
    let mut parts = cursor.0.splitn(3, ':');
    let (Some(g), Some(at), Some(session)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(invalid("That activity cursor is not valid"));
    };
    if g != group_wire(group) {
        return Err(invalid("That activity cursor belongs to another group"));
    }
    let at = at
        .parse()
        .map_err(|_| invalid("That activity cursor is not valid"))?;
    Ok((at, session.to_owned()))
}

fn run_brief(last: &LastRun) -> RunBrief {
    let settled = last.run.state == RunState::Settled;
    RunBrief {
        id: last.run.id,
        phase: last.run.view().map(|v| v.phase),
        outcome: if settled {
            last.run.outcome.clone()
        } else {
            None
        },
        started_at_ms: last.run.started_at_ms,
        ended_at_ms: if settled { last.ended_at_ms } else { None },
    }
}

fn external_active(state: &str) -> bool {
    matches!(state, "admitted" | "launchIntent" | "running")
}

fn external_brief(last: &LastExternalRun) -> RunBrief {
    let (phase, outcome) = match last.state.as_str() {
        s if external_active(s) => (Some(RunPhase::Running), None),
        "stopping" => (Some(RunPhase::Stopping), None),
        "completed" => (None, Some(RunOutcome::Completed)),
        "interrupted" => (
            None,
            Some(RunOutcome::Interrupted {
                detail: "The Claude CLI run was interrupted".into(),
            }),
        ),
        _ => (
            None,
            Some(RunOutcome::Interrupted {
                detail: "The Claude CLI run's outcome is unknown".into(),
            }),
        ),
    };
    let outcome = if phase.is_none() {
        last.outcome.clone().or(outcome)
    } else {
        outcome
    };
    let settled = outcome.is_some();
    RunBrief {
        id: last.id,
        phase,
        outcome,
        started_at_ms: last.created_at_ms,
        ended_at_ms: settled.then_some(last.updated_at_ms),
    }
}

/// A settled run's outcome as a root state.
fn settled_state(outcome: Option<&RunOutcome>) -> WorkState {
    match outcome {
        None | Some(RunOutcome::Completed) => WorkState::Ready,
        Some(RunOutcome::Aborted) => WorkState::Stopped,
        Some(RunOutcome::Interrupted { .. }) => WorkState::Interrupted,
        Some(
            RunOutcome::ModelError { detail }
            | RunOutcome::NoModel { detail }
            | RunOutcome::Failed { detail },
        ) => WorkState::Failed {
            detail: Some(detail.clone()),
        },
    }
}

fn group_of(state: &WorkState, settled_at: Option<i64>, _now: i64) -> Option<ActivityGroup> {
    match state {
        WorkState::Waiting {
            reason: WaitReason::Question | WaitReason::CliApproval | WaitReason::Input,
        } => Some(ActivityGroup::Attention),
        WorkState::Pending
        | WorkState::Queued
        | WorkState::Running
        | WorkState::Waiting { .. }
        | WorkState::Stopping
        | WorkState::Completing => Some(ActivityGroup::Running),
        WorkState::Completed
        | WorkState::Failed { .. }
        | WorkState::Stopped
        | WorkState::Interrupted
        | WorkState::Ready => {
            // A root that never ran is ready but has nothing to report as finished.
            settled_at?;
            Some(ActivityGroup::Finished)
        }
        WorkState::Blocked { .. } | WorkState::Unknown => None,
    }
}

fn child_state(
    task: &ActivityTask,
    open: Option<&LastRun>,
    question: bool,
    approval: bool,
) -> WorkState {
    match task.status.as_str() {
        "pending" => WorkState::Pending,
        "running" => {
            if open.is_some_and(|r| r.run.stop_requested || r.run.state == RunState::Stopping) {
                WorkState::Stopping
            } else if approval {
                WorkState::Waiting {
                    reason: WaitReason::CliApproval,
                }
            } else if question {
                WorkState::Waiting {
                    reason: WaitReason::Question,
                }
            } else {
                WorkState::Running
            }
        }
        "waiting" => WorkState::Waiting {
            reason: WaitReason::Input,
        },
        "completing" => WorkState::Completing,
        "done" => WorkState::Completed,
        "failed" => WorkState::Failed {
            detail: task.note.clone(),
        },
        "aborted" => WorkState::Stopped,
        _ => WorkState::Unknown,
    }
}

#[derive(Default)]
pub(super) struct Cache {
    value: Option<(std::time::Instant, HashSet<String>, Directory)>,
}
impl Shared {
    /// Sessions whose live runtime has an open question. Runtimes are only looked at, never
    /// created; the sessions lock is released before any question host is read.
    fn live_questions(&self) -> HashSet<String> {
        let live: Vec<(String, Arc<SessionRt>)> = self
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .live
            .iter()
            .map(|(id, m)| (id.clone(), m.rt.clone()))
            .collect();
        live.into_iter()
            .filter(|(_, rt)| !rt.questions.view().questions.is_empty())
            .map(|(id, _)| id)
            .collect()
    }

    fn directory(&self) -> Result<Directory, ProtoError> {
        let now = now_ms();
        let questions = self.live_questions();
        let head = self.store.read(|c| {
            Ok(c.query_row(
                "SELECT COALESCE(MAX(cursor),0) FROM event_outbox",
                [],
                |r| r.get::<_, u64>(0),
            )?)
        })?;
        if let Some((at, previous, dir)) = &self
            .activity_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .value
            && at.elapsed() < std::time::Duration::from_millis(100)
            && dir.through == head
            && previous == &questions
        {
            return Ok(dir.clone());
        }
        let src = self.store.activity_source(now)?;
        let catalog = self.auth.catalog();
        let mut unavailable = HashMap::new();
        let ActivitySource {
            sessions,
            tasks,
            last_runs,
            last_external,
            queued,
            approvals,
            jobs_active,
            through,
        } = &src;
        let mut children_active: HashMap<&str, u32> = HashMap::new();
        for task in tasks.values() {
            if !task.finished() {
                *children_active
                    .entry(task.parent_session.as_str())
                    .or_default() += 1;
            }
        }
        let mut workers = Vec::with_capacity(sessions.len());
        for row in sessions {
            let worker = if row.parent.is_some() {
                // A child session without a task (none exist today) is not a worker.
                let Some(task) = tasks.get(&row.id) else {
                    continue;
                };
                self.child_worker(
                    row,
                    task,
                    last_runs.get(&row.id),
                    &questions,
                    approvals,
                    now,
                )
            } else {
                let summary = self.root_summary(row, &catalog, &mut unavailable);
                root_worker(
                    row,
                    summary,
                    last_runs.get(&row.id),
                    last_external.get(&row.id),
                    queued.contains(&row.id),
                    questions.contains(&row.id),
                    approvals.get(&row.id).copied(),
                    now,
                )
            };
            let mut worker = worker;
            worker.row.children_active = children_active.get(row.id.as_str()).copied().unwrap_or(0);
            worker.row.jobs_active = jobs_active.get(&row.id).copied().unwrap_or(0);
            workers.push(worker);
        }
        let directory = Directory {
            workers: Arc::new(workers),
            through: *through,
        };
        self.activity_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .value = Some((std::time::Instant::now(), questions, directory.clone()));
        Ok(directory)
    }

    fn child_worker(
        &self,
        row: &SessionRow,
        task: &ActivityTask,
        last: Option<&LastRun>,
        questions: &HashSet<String>,
        approvals: &HashMap<String, i64>,
        now: i64,
    ) -> Worker {
        let open = last.filter(|r| r.run.state != RunState::Settled);
        let approval = approvals.get(&row.id).copied();
        let state = child_state(task, open, questions.contains(&row.id), approval.is_some());
        let terminal = task.finished();
        let group = if terminal {
            Some(ActivityGroup::Finished)
        } else {
            group_of(&state, None, now)
        };
        let anchor = match group {
            Some(ActivityGroup::Attention) => approval
                .or(open.map(|r| r.run.started_at_ms))
                .unwrap_or(task.updated_at_ms),
            Some(ActivityGroup::Running) => {
                open.map_or(task.created_at_ms, |r| r.run.started_at_ms)
            }
            _ => task.updated_at_ms,
        };
        let haystack = format!("{}\n{}", task.excerpt, row.cwd).to_lowercase();
        let failed = matches!(state, WorkState::Failed { .. } | WorkState::Interrupted);
        Worker {
            row: ActivityRow {
                session: row.id.clone(),
                revision: row.revision,
                kind: WorkerKind::Child,
                root: task.root.clone(),
                parent: Some(task.parent_session.clone()),
                child: Some(task.child),
                cwd: row.cwd.clone(),
                title: None,
                task_excerpt: Some(task.excerpt.clone()),
                profile: task.profile.clone(),
                origin: Some(task.origin.clone()),
                continues: task.continues,
                execution_backend: row.execution_backend,
                local_cli_mode: None,
                model: row.model.clone(),
                state,
                raw_status: Some(task.status.clone()),
                run: last.map(run_brief),
                children_active: 0,
                jobs_active: 0,
                has_result: task.status == "done" && task.has_result,
                created_at_ms: task.created_at_ms,
                updated_at_ms: row.updated_at_ms.max(task.updated_at_ms),
                sort_at_ms: 0,
            },
            group,
            anchor,
            failed,
            haystack,
            task: Some((task.root.clone(), task.child, task.parent_session.clone())),
        }
    }

    /// One page of the directory (`Call::Activity`).
    pub(super) fn activity(
        &self,
        filter: ActivityFilter,
        group: ActivityGroup,
        after: Option<ActivityCursor>,
        limit: u32,
    ) -> Result<Reply, ProtoError> {
        let text = match filter.text.as_deref().map(str::trim) {
            Some(t) if t.chars().count() > MAX_ACTIVITY_TEXT_CHARS => {
                return Err(ProtoError::new(
                    ErrorCode::TooLarge,
                    format!("Search text is limited to {MAX_ACTIVITY_TEXT_CHARS} characters"),
                ));
            }
            Some(t) if !t.is_empty() => Some(t.to_lowercase()),
            _ => None,
        };
        let after = after.map(|c| decode_cursor(group, &c)).transpose()?;
        let limit = limit.clamp(1, MAX_ACTIVITY_PAGE) as usize;
        let Directory { workers, through } = self.directory()?;
        if filter.excluded_roots.len() > 20_000
            || filter.hidden_sessions.len() > 20_000
            || filter.hidden_projects.len() > 20_000
            || filter
                .inactive_days
                .is_some_and(|days| days == 0 || days > 3650)
        {
            return Err(invalid("Invalid activity visibility policy"));
        }
        let now = now_ms();
        let matches = |w: &Worker| {
            filter.includes_visibility(&w.row, now)
                && filter.cwd.as_ref().is_none_or(|cwd| &w.row.cwd == cwd)
                && filter.kind.is_none_or(|k| w.row.kind == k)
                && filter.backend.is_none_or(|b| w.row.execution_backend == b)
                && (!filter.failed || w.failed)
                && text
                    .as_ref()
                    .is_none_or(|t| w.haystack.contains(t.as_str()))
        };
        let mut counts = ActivityCounts::default();
        let mut selected = Vec::new();
        if filter.seen_failures.len() > 20_000
            || filter
                .reviewed_completions
                .as_ref()
                .is_some_and(|seen| seen.len() > 20_000)
        {
            return Err(invalid("Too many acknowledged results"));
        }
        for w in workers.iter().filter(|w| matches(w)) {
            let unseen = (w.failed
                && filter
                    .seen_failures
                    .get(&w.row.session)
                    .is_none_or(|r| *r < w.row.revision))
                || filter
                    .reviewed_completions
                    .as_ref()
                    .is_some_and(|seen| w.row.completion_unread(seen));
            if unseen && w.group != Some(ActivityGroup::Attention) {
                counts.attention += 1;
            }
            counts.total += 1;
            counts.jobs_active += w.row.jobs_active;
            match w.group {
                Some(ActivityGroup::Attention) => counts.attention += 1,
                Some(ActivityGroup::Running) => counts.running += 1,
                Some(ActivityGroup::Finished) if now - w.anchor <= RECENT_MS => {
                    counts.finished_recent += 1;
                    if w.failed {
                        counts.failed_recent += 1;
                    }
                }
                _ => {}
            }
            if group == ActivityGroup::All
                || w.group == Some(group)
                || (group == ActivityGroup::Attention && unseen)
                || (group == ActivityGroup::Overview
                    && (unseen
                        || matches!(
                            w.group,
                            Some(ActivityGroup::Attention | ActivityGroup::Running)
                        )
                        || (w.group == Some(ActivityGroup::Finished)
                            && now - w.anchor <= RECENT_MS)))
            {
                let sort_at = if group == ActivityGroup::All && filter.inactive_days.is_some() {
                    w.row.last_activity_ms()
                } else if unseen && group == ActivityGroup::Overview {
                    30_000_000_000_000_000 - w.anchor
                } else {
                    w.sort_at(group)
                };
                selected.push((w, sort_at));
            }
        }
        let up = ascending(group);
        selected.sort_by(|a, b| {
            let ka = (a.1, a.0.row.session.as_str());
            let kb = (b.1, b.0.row.session.as_str());
            if up { ka.cmp(&kb) } else { kb.cmp(&ka) }
        });
        if let Some((at, session)) = &after {
            let key = (*at, session.as_str());
            selected.retain(|(w, at)| {
                let k = (*at, w.row.session.as_str());
                if up { k > key } else { k < key }
            });
        }
        let more = selected.len() > limit;
        selected.truncate(limit);
        let next = more
            .then(|| selected.last())
            .flatten()
            .map(|(w, at)| encode_cursor(group, *at, &w.row.session));
        Ok(Reply::Activity(ActivityPage {
            rows: selected
                .into_iter()
                .map(|(w, at)| {
                    let mut row = w.row.clone();
                    row.sort_at_ms = at;
                    row
                })
                .collect(),
            next,
            counts,
            through,
            coverage: coverage(),
        }))
    }

    /// Re-read named workers (`Call::ActivityRows`).
    pub(super) fn activity_rows(&self, sessions: Vec<SessionId>) -> Result<Reply, ProtoError> {
        if sessions.len() > MAX_ACTIVITY_ROWS {
            return Err(ProtoError::new(
                ErrorCode::TooLarge,
                format!("At most {MAX_ACTIVITY_ROWS} rows can be refetched at once"),
            ));
        }
        let Directory { workers, through } = self.directory()?;
        let by_id: HashMap<&str, &Worker> = workers
            .iter()
            .map(|w| (w.row.session.as_str(), w))
            .collect();
        let mut rows = Vec::new();
        let mut missing = Vec::new();
        let mut seen = HashSet::new();
        for session in sessions {
            if !seen.insert(session.clone()) {
                continue;
            }
            match by_id.get(session.as_str()) {
                Some(w) => {
                    let group = w.group.unwrap_or(ActivityGroup::All);
                    rows.push(w.with_sort_at(group));
                }
                None => missing.push(session),
            }
        }
        Ok(Reply::ActivityRows {
            rows,
            missing,
            through,
        })
    }

    /// A history page straight from the store (`Call::ObserveHistory`), root or child.
    pub(super) fn observe_history(
        &self,
        session: &str,
        before: Option<EntryId>,
        limit: u32,
    ) -> Result<Reply, ProtoError> {
        self.store.session(session)?.ok_or_else(|| {
            ProtoError::new(
                ErrorCode::NotFound,
                format!("Session {session} does not exist"),
            )
        })?;
        self.store
            .read(|c| Store::assert_session_live(c, session))?;
        let (entries, has_older) =
            self.store
                .page(session, before, limit.clamp(1, MAX_PAGE_ENTRIES))?;
        Ok(Reply::Page {
            native_history_start: self.store.native_history_start(session)?,
            entries,
            has_older,
        })
    }

    /// Every child of `session` as directory rows, paged by child number
    /// (`Call::ObserveChildren`). For a root that is all its descendants; for a child, the
    /// children it launched itself.
    pub(super) fn observe_children(
        &self,
        session: &str,
        after: Option<u64>,
        limit: u32,
    ) -> Result<Reply, ProtoError> {
        let row = self.store.session(session)?.ok_or_else(|| {
            ProtoError::new(
                ErrorCode::NotFound,
                format!("Session {session} does not exist"),
            )
        })?;
        self.store
            .read(|c| Store::assert_session_live(c, session))?;
        let root = row.parent.is_none();
        let limit = limit.clamp(1, MAX_OBSERVE_CHILDREN) as usize;
        let Directory { workers, through } = self.directory()?;
        let mut children: Vec<(u64, &Worker)> = workers
            .iter()
            .filter_map(|w| {
                let (task_root, child, parent) = w.task.as_ref()?;
                let mine = if root {
                    task_root == session
                } else {
                    parent == session
                };
                (mine && after.is_none_or(|a| *child > a)).then_some((*child, w))
            })
            .collect();
        children.sort_by_key(|(child, _)| *child);
        let more = children.len() > limit;
        children.truncate(limit);
        let next = more.then(|| children.last().map(|(c, _)| *c)).flatten();
        Ok(Reply::ObservedChildren {
            rows: children
                .into_iter()
                .map(|(_, w)| {
                    let group = w.group.unwrap_or(ActivityGroup::All);
                    w.with_sort_at(group)
                })
                .collect(),
            next,
            through,
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn root_worker(
    row: &SessionRow,
    summary: SessionSummary,
    last: Option<&LastRun>,
    external: Option<&LastExternalRun>,
    queued: bool,
    question: bool,
    approval: Option<i64>,
    now: i64,
) -> Worker {
    let external_root = row.execution_backend == ExecutionBackend::LocalClaudeCli;
    let brief = if external_root {
        external.map(external_brief)
    } else {
        last.map(run_brief)
    };
    let open = brief.as_ref().filter(|b| b.phase.is_some());
    let waiting = |fallback: WorkState| {
        if approval.is_some() {
            WorkState::Waiting {
                reason: WaitReason::CliApproval,
            }
        } else if question {
            WorkState::Waiting {
                reason: WaitReason::Question,
            }
        } else {
            fallback
        }
    };
    let state = match open.and_then(|b| b.phase) {
        Some(RunPhase::Stopping) => WorkState::Stopping,
        Some(RunPhase::Running) => waiting(WorkState::Running),
        None if question || approval.is_some() => waiting(WorkState::Ready),
        None if queued => WorkState::Queued,
        None => match &summary.blocked {
            Some(reason) => WorkState::Blocked {
                reason: reason.clone(),
            },
            None => settled_state(brief.as_ref().and_then(|b| b.outcome.as_ref())),
        },
    };
    let settled_at = brief.as_ref().and_then(|b| b.ended_at_ms);
    let group = group_of(&state, settled_at, now);
    let anchor = match group {
        Some(ActivityGroup::Attention) => approval
            .or(open.map(|b| b.started_at_ms))
            .unwrap_or(row.updated_at_ms),
        Some(ActivityGroup::Running) => open.map_or(row.updated_at_ms, |b| b.started_at_ms),
        _ => settled_at.unwrap_or(row.created_at_ms),
    };
    let failed = matches!(state, WorkState::Failed { .. } | WorkState::Interrupted);
    let haystack = format!(
        "{}\n{}",
        summary.title.as_deref().unwrap_or_default(),
        row.cwd
    )
    .to_lowercase();
    Worker {
        row: ActivityRow {
            session: row.id.clone(),
            revision: row.revision,
            kind: WorkerKind::Root,
            root: row.id.clone(),
            parent: None,
            child: None,
            cwd: row.cwd.clone(),
            title: summary.title,
            task_excerpt: None,
            profile: None,
            origin: None,
            continues: None,
            execution_backend: row.execution_backend,
            local_cli_mode: summary.local_cli_mode,
            model: row.model.clone(),
            state,
            raw_status: None,
            run: brief,
            children_active: 0,
            jobs_active: 0,
            has_result: false,
            created_at_ms: row.created_at_ms,
            updated_at_ms: row.updated_at_ms,
            sort_at_ms: 0,
        },
        group,
        anchor,
        failed,
        haystack,
        task: None,
    }
}

#[cfg(test)]
#[path = "activity_tests.rs"]
mod tests;
