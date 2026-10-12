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
//! GOAL-2: a turn's outcome is the executor's structured decision (`update_goal`,
//! `propose_goal_completion`), with the final-line marker accepted only as a labelled
//! fallback. Completion needs accepted mechanical evidence for every criterion. Person-set
//! turn and token allowances are checked before each turn, and a goal that continues without
//! any change to its progress signature for `goals.stallTurns` turns pauses with a reason.

use std::sync::Arc;

use pipkin_engine_protocol::*;

use super::Shared;
use crate::goals;
use crate::store::{RequestState, Store};
use crate::tools::goal::{CriterionClaim, GoalProposal, GoalUpdateRequest, parse_claim_status};
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
                // New requirements invalidate every met claim and the stall count.
                let mut view = current.clone();
                goals::mark_stale(&mut view.criteria);
                view.stall_turns = 0;
                view.decision = None;
                self.goal_commit(session, |tx| {
                    Store::goal_revise(tx, session, current.id, &objective, now)?;
                    Store::goal_write_progress(tx, session, &view, now)?;
                    Store::goal_set_turn_marks(tx, session, current.id, None, None)
                })?;
                self.goal_tick(session, None).await;
            }
            GoalAction::Archive => {
                self.goal_commit(session, |tx| {
                    Store::goal_archive(tx, session, current.id, goals::REASON_ARCHIVED, now)
                })?;
            }
            GoalAction::SetLimits {
                max_turns,
                max_tokens,
            } => {
                let mut view = current.clone();
                view.limits = GoalLimits {
                    max_turns: max_turns.filter(|n| *n > 0),
                    max_tokens: max_tokens.filter(|n| *n > 0),
                };
                self.goal_commit(session, |tx| {
                    Store::goal_write_progress(tx, session, &view, now)
                })?;
            }
            GoalAction::SetCriteria { criteria } => {
                let criteria = goals::criteria_from_texts(&criteria)
                    .map_err(|e| ProtoError::new(ErrorCode::Invalid, e))?;
                let mut view = current.clone();
                view.criteria = criteria;
                view.stall_turns = 0;
                if matches!(view.state, GoalState::Completed) {
                    // Completion was judged against other criteria.
                    view.decision = None;
                }
                self.goal_commit(session, |tx| {
                    Store::goal_write_progress(tx, session, &view, now)?;
                    if matches!(current.state, GoalState::Completed) {
                        Store::goal_set_state(
                            tx,
                            session,
                            current.id,
                            GoalState::Paused,
                            Some("Paused: the criteria changed after completion; resume to continue."),
                            now,
                        )?;
                    }
                    Ok(())
                })?;
            }
        }
        self.goal_view(session)
    }

    /// Tokens reported for the whole conversation tree (input, output and cache).
    fn session_tokens(&self, session: &str) -> u64 {
        self.store
            .usage(session)
            .map(|u| {
                u.models
                    .iter()
                    .map(|(_, usage)| {
                        usage.input + usage.output + usage.cache_read + usage.cache_write
                    })
                    .sum()
            })
            .unwrap_or(0)
    }

    /// What the engine knows about a tool call named as evidence.
    fn call_check(&self, session: &str, call_id: &str) -> goals::CallCheck {
        let Ok(Some(row)) = self.store.tool_call(session, call_id) else {
            return goals::CallCheck::Missing;
        };
        match row.state {
            crate::store::ToolState::Intent => goals::CallCheck::Open,
            crate::store::ToolState::Interrupted => goals::CallCheck::Failed,
            crate::store::ToolState::Done => {
                let failed = row
                    .result_entry
                    .and_then(|id| {
                        self.store
                            .read(|c| Store::entry_in(c, session, id))
                            .ok()
                            .flatten()
                    })
                    .is_some_and(|entry| {
                        matches!(
                            &entry.kind,
                            EntryKind::Message {
                                message: Message::ToolResult { is_error: true, .. },
                                ..
                            }
                        )
                    });
                if failed {
                    goals::CallCheck::Failed
                } else {
                    goals::CallCheck::Succeeded
                }
            }
        }
    }

    /// Apply claims to the goal's criteria, checking their evidence.
    fn apply_claims(
        &self,
        session: &str,
        view: &mut GoalView,
        claims: &[CriterionClaim],
        cwd: &std::path::Path,
    ) -> Result<(), String> {
        for claim in claims {
            let Some(at) = view.criteria.iter().position(|c| c.id == claim.id) else {
                return Err(format!("no criterion #{}", claim.id));
            };
            goals::check_evidence_count(view.criteria[at].evidence.len(), claim.evidence.len())?;
            for input in &claim.evidence {
                goals::bounded("evidence reference", &input.reference)?;
                let mut evidence = input.clone().into_evidence();
                goals::check_evidence(&mut evidence, &|id| self.call_check(session, id), cwd);
                view.criteria[at].evidence.push(evidence);
            }
            if let Some(status) = &claim.status {
                let status = parse_claim_status(status).ok_or_else(|| {
                    format!("criterion status must be met, unmet or open, not {status:?}")
                })?;
                goals::judge_criterion(&mut view.criteria[at], status, view.revision);
            }
        }
        Ok(())
    }

    fn active_goal_for_tool(&self, session: &str) -> Result<GoalView, String> {
        let goal = self
            .goal_view(session)
            .map_err(|e| e.message)?
            .ok_or("This conversation has no goal.")?;
        if goal.state != GoalState::Active {
            return Err(format!(
                "The goal is {} ({}); only the person can resume it.",
                goal.state.as_str(),
                goal.reason.as_deref().unwrap_or("no reason recorded")
            ));
        }
        Ok(goal)
    }

    /// `update_goal`: progress note, criterion claims, task links and the turn's decision.
    pub fn goal_update_from_tool(
        &self,
        session: &str,
        request: GoalUpdateRequest,
    ) -> Result<Result<GoalView, String>, String> {
        let mut view = match self.active_goal_for_tool(session) {
            Ok(g) => g,
            Err(reason) => return Ok(Err(reason)),
        };
        let cwd = self
            .store
            .session(session)
            .ok()
            .flatten()
            .map(|r| std::path::PathBuf::from(r.cwd))
            .unwrap_or_default();
        let now = now_ms();
        let applied = (|| -> Result<Option<(GoalState, String)>, String> {
            if let Some(note) = &request.note {
                view.note = Some(goals::bounded("note", note)?);
            }
            if !request.link_tasks.is_empty() {
                let plan = self
                    .store
                    .read(|c| Store::plan_state(c, session))
                    .map_err(|e| e.to_string())?;
                for id in &request.link_tasks {
                    if !plan.tasks.iter().any(|t| t.id == *id) {
                        return Err(format!("no plan task #{id} in this conversation"));
                    }
                    if !view.linked_tasks.contains(id) {
                        view.linked_tasks.push(*id);
                    }
                }
            }
            self.apply_claims(session, &mut view, &request.criteria, &cwd)?;
            let reason = request
                .reason
                .as_deref()
                .map(|r| goals::bounded("reason", r))
                .transpose()?;
            match request.status.as_deref() {
                None => Ok(None),
                Some("continue") => {
                    view.decision = Some(GoalDecision {
                        epoch: view.epoch,
                        kind: "continue".into(),
                        reason: reason.clone(),
                    });
                    Ok(None)
                }
                Some(kind @ ("blocked" | "unreachable")) => {
                    let reason = reason.ok_or_else(|| format!("{kind} needs a reason"))?;
                    view.decision = Some(GoalDecision {
                        epoch: view.epoch,
                        kind: kind.into(),
                        reason: Some(reason.clone()),
                    });
                    let state = if kind == "blocked" {
                        GoalState::Blocked
                    } else {
                        GoalState::Failed
                    };
                    Ok(Some((state, reason)))
                }
                Some(other) => Err(format!(
                    "status must be continue, blocked or unreachable, not {other:?}"
                )),
            }
        })();
        let transition = match applied {
            Ok(t) => t,
            Err(reason) => return Ok(Err(reason)),
        };
        let id = view.id;
        let written = self.goal_commit(session, |tx| {
            Store::goal_write_progress(tx, session, &view, now)?;
            if let Some((state, reason)) = &transition {
                Store::goal_set_state(tx, session, id, *state, Some(reason), now)?;
            }
            Ok(())
        });
        match written {
            Ok(Some(goal)) => Ok(Ok(goal)),
            Ok(None) => Ok(Err("The goal was archived meanwhile.".into())),
            Err(e) => Err(e.message),
        }
    }

    /// `propose_goal_completion`: accepted only with accepted mechanical evidence for every
    /// criterion (or for the objective itself when there are none).
    #[allow(clippy::type_complexity)]
    pub fn goal_propose_from_tool(
        &self,
        session: &str,
        proposal: GoalProposal,
    ) -> Result<Result<GoalView, (Vec<String>, Option<GoalView>)>, String> {
        let mut view = match self.active_goal_for_tool(session) {
            Ok(g) => g,
            Err(reason) => return Ok(Err((vec![reason], None))),
        };
        let summary = match goals::bounded("summary", &proposal.summary) {
            Ok(s) => s,
            Err(e) => return Ok(Err((vec![e], Some(view)))),
        };
        let cwd = self
            .store
            .session(session)
            .ok()
            .flatten()
            .map(|r| std::path::PathBuf::from(r.cwd))
            .unwrap_or_default();
        let now = now_ms();
        let mut claims = proposal.criteria.clone();
        if view.criteria.is_empty() {
            // The objective is the one criterion; the proposal's evidence is for it.
            view.criteria = vec![GoalCriterion {
                id: 1,
                text: format!("Objective: {}", view.objective),
                status: CriterionStatus::Open,
                evidence: Vec::new(),
                judged_at_revision: 0,
            }];
            claims.push(CriterionClaim {
                id: 1,
                status: Some("met".into()),
                evidence: proposal.evidence.clone(),
            });
        }
        // A proposal claims every named criterion met.
        for claim in &mut claims {
            claim.status = Some("met".into());
        }
        // File evidence recorded earlier is re-checked now: a changed file is stale.
        for c in &mut view.criteria {
            for e in &mut c.evidence {
                if e.kind == "file" && e.accepted == Some(true) {
                    goals::check_evidence(e, &|id| self.call_check(session, id), &cwd);
                }
            }
            if c.status == CriterionStatus::Met
                && !c
                    .evidence
                    .iter()
                    .any(|e| e.accepted == Some(true) && !e.judgment)
            {
                c.status = CriterionStatus::Unverified;
            }
        }
        if let Err(reason) = self.apply_claims(session, &mut view, &claims, &cwd) {
            return Ok(Err((vec![reason], Some(view))));
        }
        let unmet = goals::unmet_criteria(&view.criteria);
        let rejected: Vec<String> = view
            .criteria
            .iter()
            .flat_map(|c| {
                c.evidence
                    .iter()
                    .filter(|e| e.accepted == Some(false))
                    .map(move |e| {
                        format!(
                            "#{} {} {}: {}",
                            c.id,
                            e.kind,
                            e.reference,
                            e.verdict.clone().unwrap_or_default()
                        )
                    })
            })
            .collect();
        view.note = Some(summary);
        let id = view.id;
        if unmet.is_empty() {
            view.decision = Some(GoalDecision {
                epoch: view.epoch,
                kind: "met".into(),
                reason: None,
            });
            let written = self.goal_commit(session, |tx| {
                Store::goal_write_progress(tx, session, &view, now)?;
                Store::goal_set_state(
                    tx,
                    session,
                    id,
                    GoalState::Completed,
                    Some(goals::REASON_PROPOSED),
                    now,
                )
            });
            return match written {
                Ok(Some(goal)) => Ok(Ok(goal)),
                Ok(None) => Ok(Err((vec!["The goal was archived meanwhile.".into()], None))),
                Err(e) => Err(e.message),
            };
        }
        // Not accepted: keep the checked evidence and claims so the next attempt builds on them.
        let written = self.goal_commit(session, |tx| {
            Store::goal_write_progress(tx, session, &view, now)
        });
        let goal = match written {
            Ok(g) => g,
            Err(e) => return Err(e.message),
        };
        let mut problems: Vec<String> =
            unmet.into_iter().map(|u| format!("not met: {u}")).collect();
        problems.extend(
            rejected
                .into_iter()
                .map(|r| format!("rejected evidence: {r}")),
        );
        Ok(Err((problems, goal)))
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

    /// Pause the current goal because Stop was requested. Only a goal whose own turn is
    /// open is paused: a replacement set alongside the Stop (the desktop sends both, and Stop
    /// travels on the control lane) has no open turn yet and must not be paused by it.
    pub fn goal_on_stop(&self, session: &str) {
        let Ok(Some(goal)) = self.goal_view(session) else {
            return;
        };
        if goal.state != GoalState::Active {
            return;
        }
        let own_turn_open = goal.last_request.as_deref().is_some_and(|key| {
            self.store
                .request(session, key)
                .ok()
                .flatten()
                .is_some_and(|r| r.state != RequestState::Settled)
        });
        if !own_turn_open {
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
            // A turn that completed, blocked or failed the goal from inside still gets its
            // tokens attributed to the goal.
            if let Some((key, _)) = &settled
                && goal.last_request.as_deref() == Some(key.as_str())
            {
                self.account_turn_tokens(session, &goal);
            }
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
                // Account this turn's tokens before anything else.
                let tokens_now = self.session_tokens(session);
                let (signature_before, tokens_start) = self
                    .store
                    .read(|c| Store::goal_turn_marks(c, session, goal.id))
                    .unwrap_or((None, None));
                let spent = tokens_start.map_or(0, |start| tokens_now.saturating_sub(start));
                // The structured decision of this epoch wins; the marker is the fallback.
                let decision = goal
                    .decision
                    .as_ref()
                    .filter(|d| d.epoch == goal.epoch)
                    .map(|d| d.kind.clone());
                let verdict = match decision.as_deref() {
                    Some("continue") => goals::Verdict::Continue,
                    Some("met") => goals::Verdict::Met,
                    Some("blocked") => goals::Verdict::Blocked,
                    Some("unreachable") => goals::Verdict::Unreachable,
                    _ => goals::verdict(&self.answer_text(session)),
                };
                let structured = decision.is_some();
                let plan_revision = self
                    .store
                    .read(|c| Store::plan_state(c, session))
                    .map(|p| p.revision)
                    .unwrap_or(0);
                let signature = goals::progress_signature(
                    &goal.criteria,
                    goal.note.as_deref(),
                    &goal.linked_tasks,
                    plan_revision,
                );
                let stalled = signature_before.as_deref() == Some(signature.as_str());
                let stall_turns = if verdict == goals::Verdict::Continue && stalled {
                    goal.stall_turns + 1
                } else {
                    0
                };
                let mut progressed = goal.clone();
                progressed.usage_tokens = goal.usage_tokens.saturating_add(spent);
                progressed.stall_turns = stall_turns;
                let _ = self.goal_commit(session, |tx| {
                    Store::goal_write_progress(tx, session, &progressed, now_ms())?;
                    Store::goal_set_turn_marks(tx, session, goal.id, Some(&signature), tokens_start)
                });
                match verdict {
                    goals::Verdict::Continue => {}
                    goals::Verdict::Met => {
                        if structured {
                            // propose_goal_completion already completed it; nothing to do.
                            return;
                        }
                        return pause(GoalState::Completed, goals::REASON_MET);
                    }
                    goals::Verdict::Blocked => {
                        return pause(GoalState::Blocked, goals::REASON_BLOCKED);
                    }
                    goals::Verdict::Unreachable => {
                        return pause(GoalState::Failed, goals::REASON_UNREACHABLE);
                    }
                    goals::Verdict::Ambiguous => {
                        return pause(GoalState::Paused, goals::REASON_NO_STATUS);
                    }
                }
                let threshold = self.goal_stall_threshold(session);
                if stall_turns >= threshold {
                    return pause(
                        GoalState::Paused,
                        &format!(
                            "Paused: no change to the goal's progress in {stall_turns} continued turns; revise the goal or resume to try again."
                        ),
                    );
                }
                if goal.limits.max_turns.is_some_and(|max| goal.turns >= max) {
                    return pause(GoalState::Paused, goals::REASON_TURNS_USED);
                }
                if goal
                    .limits
                    .max_tokens
                    .is_some_and(|max| progressed.usage_tokens >= max)
                {
                    return pause(GoalState::Paused, goals::REASON_TOKENS_USED);
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
        // A resumed or revised goal is also subject to its allowances.
        if goal.limits.max_turns.is_some_and(|max| goal.turns >= max) {
            return pause(GoalState::Paused, goals::REASON_TURNS_USED);
        }
        if goal
            .limits
            .max_tokens
            .is_some_and(|max| goal.usage_tokens >= max)
        {
            return pause(GoalState::Paused, goals::REASON_TOKENS_USED);
        }
        let key = goals::request_key(goal.id, goal.epoch + 1);
        let tokens_start = self.session_tokens(session);
        let scheduled = self.goal_commit(session, |tx| {
            Store::goal_schedule_turn(tx, session, goal.id, goal.epoch, &key, now_ms())?;
            let (signature, _) = Store::goal_turn_marks(tx, session, goal.id)?;
            Store::goal_set_turn_marks(
                tx,
                session,
                goal.id,
                signature.as_deref(),
                Some(tokens_start),
            )?;
            Ok(())
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
        let text = goals::turn_prompt_structured(&goal.objective, &goal.criteria);
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

    /// Add the settled turn's tokens (session total minus the start mark) to the goal.
    fn account_turn_tokens(&self, session: &str, goal: &GoalView) {
        let tokens_now = self.session_tokens(session);
        let (signature, tokens_start) = self
            .store
            .read(|c| Store::goal_turn_marks(c, session, goal.id))
            .unwrap_or((None, None));
        let Some(start) = tokens_start else { return };
        let mut progressed = goal.clone();
        progressed.usage_tokens = goal
            .usage_tokens
            .saturating_add(tokens_now.saturating_sub(start));
        let _ = self.goal_commit(session, |tx| {
            Store::goal_write_progress(tx, session, &progressed, now_ms())?;
            Store::goal_set_turn_marks(tx, session, goal.id, signature.as_deref(), None)
        });
    }

    /// `goals.stallTurns` from settings (default 3, at least 1).
    fn goal_stall_threshold(&self, session: &str) -> u32 {
        self.settings_for(session)
            .other
            .get("goals")
            .and_then(|g| g.get("stallTurns"))
            .and_then(serde_json::Value::as_u64)
            .map(|n| (n as u32).max(1))
            .unwrap_or(goals::DEFAULT_STALL_TURNS)
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
