//! The engine-owned goal runner (plan §9, GOAL-1). One owner per session: when the engine
//! advertises `goals`, the desktop only sends intent and mirrors state.
//!
//! The invariants kept from the desktop runner:
//!
//! - scheduling intent is committed (epoch, turns, request key) before the turn is sent;
//! - a turn continues only when the executor's final line explicitly says so; an ambiguous
//!   reply pauses the goal, and nothing is ever blindly resent;
//! - a settlement advances the goal only when it is the goal's own latest turn settling;
//! - Stop pauses the goal that exists at that instant; a goal set afterwards is a replacement;
//! - an engine restart pauses every active goal until the person resumes;
//! - no continuation is scheduled while no client is attached to the session (the desktop's
//!   "no goal turn after the window is gone"), and a queued human message pauses the goal
//!   rather than racing it.
//!
//! Completion is the executor's report, labelled as such; GOAL-2 adds evidence and audit.

use std::sync::Arc;

use pipkin_engine_protocol::*;

use super::Shared;
use crate::goals;
use crate::store::{RequestState, Store};
use crate::util::now_ms;

impl Shared {
    pub fn goal_view(&self, session: &str) -> Result<Option<GoalView>, ProtoError> {
        if self.store.session(session)?.is_none() {
            return Err(ProtoError::new(
                ErrorCode::NotFound,
                format!("Session {session} does not exist"),
            ));
        }
        Ok(self.store.read(|c| Store::goal_current(c, session))?)
    }

    pub fn goal_history(&self, session: &str) -> Result<Vec<GoalView>, ProtoError> {
        if self.store.session(session)?.is_none() {
            return Err(ProtoError::new(
                ErrorCode::NotFound,
                format!("Session {session} does not exist"),
            ));
        }
        Ok(self.store.read(|c| Store::goal_history(c, session, 200))?)
    }

    /// Commit a goal change and publish `GoalChanged` with it.
    fn goal_commit(
        &self,
        session: &str,
        f: impl FnOnce(&rusqlite::Transaction) -> crate::store::Result<()>,
    ) -> Result<Option<GoalView>, ProtoError> {
        let sess = self.session(session)?;
        self.commit(&sess, |tx| {
            f(tx)?;
            let goal = Store::goal_current(tx, session)?;
            Ok((goal.clone(), vec![SessionChange::GoalChanged { goal }]))
        })
    }

    fn check_objective(objective: &str) -> Result<String, ProtoError> {
        let objective = objective.trim();
        if objective.is_empty() {
            return Err(ProtoError::new(ErrorCode::Invalid, "the goal is empty"));
        }
        if objective.len() > limits::MAX_PROMPT_BYTES / 4 {
            return Err(ProtoError::new(ErrorCode::TooLarge, "the goal is too long"));
        }
        Ok(objective.to_owned())
    }

    /// Set or replace the goal and start it if the session is idle.
    pub async fn set_goal(
        self: &Arc<Self>,
        session: &str,
        objective: &str,
    ) -> Result<Option<GoalView>, ProtoError> {
        let objective = Self::check_objective(objective)?;
        let row = self.store.session(session)?.ok_or_else(|| {
            ProtoError::new(
                ErrorCode::NotFound,
                format!("Session {session} does not exist"),
            )
        })?;
        if row.parent.is_some() {
            return Err(ProtoError::new(
                ErrorCode::Unsupported,
                "a subagent cannot own a goal",
            ));
        }
        self.goal_commit(session, |tx| {
            Store::goal_insert(
                tx,
                session,
                &objective,
                GoalState::Active,
                None,
                "user",
                goals::REASON_REPLACED,
                now_ms(),
            )?;
            Ok(())
        })?;
        self.goal_tick(session, None).await;
        self.goal_view(session)
    }

    pub async fn goal_control(
        self: &Arc<Self>,
        session: &str,
        action: GoalAction,
    ) -> Result<Option<GoalView>, ProtoError> {
        let Some(current) = self.goal_view(session)? else {
            return Err(ProtoError::new(
                ErrorCode::NotFound,
                "This conversation has no goal",
            ));
        };
        let now = now_ms();
        match action {
            GoalAction::Pause => {
                if current.state == GoalState::Active {
                    self.goal_commit(session, |tx| {
                        Store::goal_set_state(
                            tx,
                            session,
                            current.id,
                            GoalState::Paused,
                            Some(goals::REASON_PAUSED_BY_PERSON),
                            now,
                        )
                    })?;
                }
            }
            GoalAction::Resume => {
                if !current.state.resumable() {
                    return Err(ProtoError::new(
                        ErrorCode::Invalid,
                        format!(
                            "a {} goal cannot be resumed; revise it instead",
                            current.state.as_str()
                        ),
                    ));
                }
                self.goal_commit(session, |tx| {
                    Store::goal_set_state(tx, session, current.id, GoalState::Active, None, now)
                })?;
                self.goal_tick(session, None).await;
            }
            GoalAction::Revise { objective } => {
                let objective = Self::check_objective(&objective)?;
                self.goal_commit(session, |tx| {
                    Store::goal_revise(tx, session, current.id, &objective, now)
                })?;
                self.goal_tick(session, None).await;
            }
            GoalAction::Archive => {
                self.goal_commit(session, |tx| {
                    Store::goal_archive(tx, session, current.id, goals::REASON_ARCHIVED, now)
                })?;
            }
        }
        self.goal_view(session)
    }

    /// Import a desktop-kept goal as paused, once per key. An engine goal that already
    /// exists stands; the key is recorded either way so the desktop can retire its record.
    pub fn migrate_goal(
        &self,
        session: &str,
        key: &str,
        objective: &str,
        paused: Option<&str>,
    ) -> Result<Option<GoalView>, ProtoError> {
        if key.is_empty() || key.len() > limits::MAX_KEY_BYTES {
            return Err(ProtoError::new(
                ErrorCode::Invalid,
                "migration key must be 1 to 256 bytes",
            ));
        }
        let objective = Self::check_objective(objective)?;
        let seen = self
            .store
            .read(|c| Store::goal_migration_seen(c, session, key))?;
        if seen {
            return self.goal_view(session);
        }
        let now = now_ms();
        let reason = paused
            .map(str::to_owned)
            .filter(|p| !p.trim().is_empty())
            .unwrap_or_else(|| goals::REASON_MIGRATED.to_owned());
        self.goal_commit(session, |tx| {
            if Store::goal_current(tx, session)?.is_none() {
                Store::goal_insert(
                    tx,
                    session,
                    &objective,
                    GoalState::Paused,
                    Some(&reason),
                    "migrated:desktop",
                    goals::REASON_REPLACED,
                    now,
                )?;
            }
            Store::goal_migration_record(tx, session, key, now)
        })?;
        self.goal_view(session)
    }

    /// Pause the current goal because Stop was requested (the goal that exists now, not a
    /// replacement set afterwards).
    pub fn goal_on_stop(&self, session: &str) {
        let Ok(Some(goal)) = self.goal_view(session) else {
            return;
        };
        if goal.state != GoalState::Active {
            return;
        }
        let _ = self.goal_commit(session, |tx| {
            Store::goal_set_state(
                tx,
                session,
                goal.id,
                GoalState::Paused,
                Some(goals::REASON_STOP),
                now_ms(),
            )
        });
    }

    /// Whether any client is attached to the session right now.
    pub fn session_attached(&self, session: &str) -> bool {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .live
            .get(session)
            .is_some_and(|m| m.attached())
    }

    /// Advance the goal after a settlement (`settled` is the request key and outcome), or
    /// after a set/resume/revise (`None`).
    pub async fn goal_tick(self: &Arc<Self>, session: &str, settled: Option<(String, RunOutcome)>) {
        let Ok(Some(goal)) = self.goal_view(session) else {
            return;
        };
        if goal.state != GoalState::Active {
            return;
        }
        let pause = |state: GoalState, reason: &str| {
            let _ = self.goal_commit(session, |tx| {
                Store::goal_set_state(tx, session, goal.id, state, Some(reason), now_ms())
            });
        };
        match (&goal.last_request, &settled) {
            (Some(own), Some((key, outcome))) if own == key => {
                match outcome {
                    RunOutcome::Aborted => return pause(GoalState::Paused, goals::REASON_STOP),
                    RunOutcome::Completed => {}
                    RunOutcome::ModelError { detail }
                    | RunOutcome::NoModel { detail }
                    | RunOutcome::Interrupted { detail }
                    | RunOutcome::Failed { detail } => {
                        return pause(
                            GoalState::Paused,
                            &format!("Paused: the goal turn failed ({detail})."),
                        );
                    }
                }
                match goals::verdict(&self.answer_text(session)) {
                    goals::Verdict::Continue => {}
                    goals::Verdict::Met => return pause(GoalState::Completed, goals::REASON_MET),
                    goals::Verdict::Blocked => {
                        return pause(GoalState::Blocked, goals::REASON_BLOCKED);
                    }
                    goals::Verdict::Unreachable => {
                        return pause(GoalState::Failed, goals::REASON_UNREACHABLE);
                    }
                    goals::Verdict::Ambiguous => {
                        return pause(GoalState::Paused, goals::REASON_AMBIGUOUS);
                    }
                }
            }
            // Another input settled: the goal only moves on its own turns, except that a
            // never-started goal starts once the session is idle.
            (Some(_), Some(_)) => return,
            // A tick without a settlement (set, resume, revise): start a turn only when the
            // latest one is settled; otherwise it will advance when that turn settles.
            (Some(own), None) => {
                let open = self
                    .store
                    .request(session, own)
                    .ok()
                    .flatten()
                    .is_some_and(|r| r.state != RequestState::Settled);
                if open {
                    return;
                }
            }
            (None, _) => {}
        }
        // Start the next turn.
        if !self.session_attached(session) {
            return pause(GoalState::Paused, goals::REASON_DETACHED);
        }
        let busy = self.store.open_run(session).ok().flatten().is_some()
            || self
                .store
                .queue(session)
                .map(|q| !q.is_empty())
                .unwrap_or(false);
        if busy {
            if settled.is_some() {
                // The person's input takes precedence over the goal's next turn.
                return pause(GoalState::Paused, goals::REASON_USER_INPUT);
            }
            // Set/resumed while a run is active: it starts when that run settles.
            return;
        }
        let key = goals::request_key(goal.id, goal.epoch + 1);
        let scheduled = self.goal_commit(session, |tx| {
            Store::goal_schedule_turn(tx, session, goal.id, goal.epoch, &key, now_ms()).map(|_| ())
        });
        // The commit publishes whatever the goal is now; only the tick that actually advanced
        // the epoch sends the turn.
        let advanced = scheduled.ok().flatten().is_some_and(|g| {
            g.id == goal.id
                && g.epoch == goal.epoch + 1
                && g.last_request.as_deref() == Some(key.as_str())
        });
        if !advanced {
            return;
        }
        let text = goals::turn_prompt(&goal.objective);
        match self.prompt(session, &key, &text, &[]).await {
            Ok(_) => {}
            Err(e) if e.code == ErrorCode::Busy => {
                pause(GoalState::Paused, goals::REASON_USER_INPUT)
            }
            Err(e) => pause(
                GoalState::Paused,
                &format!("Paused: could not start the next turn ({}).", e.message),
            ),
        }
    }

    /// Hook for run settlement: advance the session's goal off the settling task.
    pub(super) fn goal_after_settlement(
        &self,
        session: &str,
        request: Option<String>,
        outcome: RunOutcome,
    ) {
        let Some(request) = request else { return };
        let Some(shared) = self.me.upgrade() else {
            return;
        };
        let session = session.to_owned();
        self.spawn(async move {
            shared.goal_tick(&session, Some((request, outcome))).await;
        });
    }
}
