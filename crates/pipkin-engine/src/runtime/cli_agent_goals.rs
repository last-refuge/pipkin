//! Private CLI-only goal controller. Explicit person intent creates an in-memory owner;
//! scheduling data never grants execution. No generic prompt/goal_tick/API runner is used.
#![allow(dead_code)] // Includes private qualification seams.
use super::{
    Shared,
    cli_agent_host::ChildResources,
    cli_agent_owner::{Error as OwnerError, GoalOwner},
    cli_agent_runtime::admission::Start,
    cli_bridge_driver::Services,
};
use crate::{
    goals,
    store::{
        Store, StoreError,
        cli_agent_turns::{GoalTurn, State},
    },
    util::now_ms,
};
use pipkin_engine_protocol::{Event, GoalState, GoalView, SessionChange};
use std::{sync::Arc, time::Duration};
use tokio::{sync::oneshot, time::Instant};

impl Shared {
    /// `Some(objective)` starts/replaces; `None` explicitly resumes the current goal. The
    /// caller must supply resources captured for this exact connected root approval.
    pub(super) fn start_cli_goal(
        self: &Arc<Self>,
        owner: u64,
        session: &str,
        objective: Option<String>,
        resources: Arc<ChildResources>,
        generation: u64,
    ) -> Result<oneshot::Receiver<Result<GoalView, String>>, String> {
        self.start_cli_goal_notified(owner, session, objective, resources, generation, None, None)
    }
    #[allow(clippy::too_many_arguments)] // Admission acknowledgement and preparation fence are independent.
    pub(super) fn start_cli_goal_notified(
        self: &Arc<Self>,
        owner: u64,
        session: &str,
        objective: Option<String>,
        resources: Arc<ChildResources>,
        generation: u64,
        admitted: Option<oneshot::Sender<GoalView>>,
        expected: Option<&super::cli_agent_owner::InputEpoch>,
    ) -> Result<oneshot::Receiver<Result<GoalView, String>>, String> {
        if generation == 0 {
            return Err("Missing generation".into());
        }
        if let Some(text) = &objective
            && (text.trim().is_empty()
                || text.len() > pipkin_engine_protocol::limits::MAX_PROMPT_BYTES / 4
                || text.contains('\0'))
        {
            return Err("Invalid goal objective".into());
        }
        let owner = self
            .cli_agents
            .goal_owner_during(&self.store, owner, session, expected)
            .map_err(|e| format!("Goal approval unavailable: {e:?}"))?;
        let row = self
            .store
            .session(session)
            .map_err(|e| e.to_string())?
            .ok_or("Session disappeared")?;
        if resources.prompt.cwd != std::path::Path::new(&row.cwd) {
            return Err("Goal resources do not match the approved project".into());
        }
        let tracking = self.external_tasks.token();
        let (send, receive) = oneshot::channel();
        let shared = self.clone();
        self.spawn(async move {
            let result = shared
                .run_cli_goal(&owner, objective, resources, generation, admitted)
                .await;
            drop(owner);
            let _ = send.send(result);
            drop(tracking);
        });
        Ok(receive)
    }
    fn cli_goal_write(
        &self,
        owner: &GoalOwner,
        expected: Option<&GoalView>,
        f: impl FnOnce(&rusqlite::Transaction) -> crate::store::Result<()>,
    ) -> Result<GoalView, String> {
        let _gate = self.publication.lock().unwrap_or_else(|e| e.into_inner());
        let (goal, revision) = owner
            .authorize(|| {
                self.store.tx(|tx| {
                    if let Some(expected) = expected
                        && Store::goal_current(tx, owner.session())?.as_ref() != Some(expected)
                    {
                        return Err(StoreError::ExecutionConflict);
                    }
                    f(tx)?;
                    Ok((
                        Store::goal_current(tx, owner.session())?
                            .ok_or(StoreError::ExecutionConflict)?,
                        Store::bump(tx, owner.session(), now_ms())?,
                    ))
                })
            })
            .map_err(|e| e.to_string())?;
        self.publish(Event::Session {
            session: owner.session().into(),
            revision,
            change: SessionChange::GoalChanged {
                goal: Some(Box::new(goal.clone())),
            },
        });
        Ok(goal)
    }
    /// Cleanup may pause its own still-current goal after revocation, but cannot activate,
    /// complete, account twice, or touch a replacement. No new owner is minted here.
    fn cli_goal_pause(
        &self,
        session: &str,
        expected: &GoalView,
        reason: &str,
    ) -> Result<GoalView, String> {
        let _gate = self.publication.lock().unwrap_or_else(|e| e.into_inner());
        let (goal, revision) = self
            .store
            .tx(|tx| {
                let current =
                    Store::goal_current(tx, session)?.ok_or(StoreError::ExecutionConflict)?;
                if current.id != expected.id
                    || current.revision != expected.revision
                    || current.epoch != expected.epoch
                    || (current.state == GoalState::Completed
                        && current
                            .audit
                            .result
                            .as_ref()
                            .is_some_and(|r| r.verdict == "skipped"))
                {
                    return Ok((current, None));
                }
                if matches!(
                    current.state,
                    GoalState::Active | GoalState::Verifying | GoalState::Completed
                ) {
                    let mut view = current.clone();
                    if let Some(result) = view.audit.result.as_mut()
                        && result.verdict == "pending"
                    {
                        result.verdict = "stopped".into();
                    }
                    Store::goal_write_progress(tx, session, &view, now_ms())?;
                    Store::goal_set_state(
                        tx,
                        session,
                        current.id,
                        GoalState::Paused,
                        Some(reason),
                        now_ms(),
                    )?;
                    Ok((
                        Store::goal_current(tx, session)?.ok_or(StoreError::ExecutionConflict)?,
                        Some(Store::bump(tx, session, now_ms())?),
                    ))
                } else {
                    Ok((current, None))
                }
            })
            .map_err(|e| e.to_string())?;
        if let Some(revision) = revision {
            self.publish(Event::Session {
                session: session.into(),
                revision,
                change: SessionChange::GoalChanged {
                    goal: Some(Box::new(goal.clone())),
                },
            });
        }
        Ok(goal)
    }
    async fn run_cli_goal(
        self: &Arc<Self>,
        owner: &GoalOwner,
        objective: Option<String>,
        resources: Arc<ChildResources>,
        generation: u64,
        admitted: Option<oneshot::Sender<GoalView>>,
    ) -> Result<GoalView, String> {
        let session = owner.session();
        let mut goal = tokio::task::block_in_place(|| {
            self.cli_goal_write(owner, None, |tx| {
                if let Some(objective) = objective {
                    Store::goal_insert(
                        tx,
                        session,
                        objective.trim(),
                        GoalState::Active,
                        None,
                        "user",
                        goals::REASON_REPLACED,
                        now_ms(),
                    )?;
                } else {
                    let goal =
                        Store::goal_current(tx, session)?.ok_or(StoreError::ExecutionConflict)?;
                    if !goal.state.resumable() {
                        return Err(StoreError::ExecutionConflict);
                    }
                    Store::goal_set_state(tx, session, goal.id, GoalState::Active, None, now_ms())?;
                }
                Ok(())
            })
        })?;
        if let Some(send) = admitted {
            let _ = send.send(goal.clone());
        }
        let result = self
            .cli_goal_loop(owner, &mut goal, resources, generation)
            .await;
        match result {
            Ok(goal) => Ok(goal),
            Err(reason) => tokio::task::block_in_place(|| {
                self.cli_goal_pause(session, &goal, &format!("Paused: {reason}"))
            }),
        }
    }
    async fn cli_goal_loop(
        self: &Arc<Self>,
        owner: &GoalOwner,
        goal: &mut GoalView,
        resources: Arc<ChildResources>,
        generation: u64,
    ) -> Result<GoalView, String> {
        let session = owner.session();
        let cancel = owner.cancellation();
        let revoked = owner.revoked();
        loop {
            if cancel.is_cancelled() || revoked.is_cancelled() || self.stopping.is_cancelled() {
                return Err("goal stopped or original approval lost; resume explicitly".into());
            }
            // Receipt commitment can precede runtime cleanup/publication. Reconcile
            // and read one transaction so neither allowance nor completion sees an
            // old goal total paired with a newer, already-terminal receipt.
            let (current, usage) = tokio::task::block_in_place(|| {
                let _gate = self.publication.lock().unwrap_or_else(|e| e.into_inner());
                let (current, usage, update) = self.store.tx(|c| {
                    let current =
                        Store::goal_current(c, session)?.ok_or(StoreError::ExecutionConflict)?;
                    let update = Store::cli_goal_reconcile_in(c, session, current.id, now_ms())?;
                    Ok((
                        Store::goal_current(c, session)?.ok_or(StoreError::ExecutionConflict)?,
                        Store::cli_goal_usage_in(c, session, current.id)?,
                        update,
                    ))
                })?;
                if let Some((session, goal, revision)) = update {
                    self.publish(Event::Session {
                        session,
                        revision,
                        change: SessionChange::GoalChanged {
                            goal: Some(Box::new(goal)),
                        },
                    });
                }
                Ok::<_, StoreError>((current, usage))
            })
            .map_err(|e| e.to_string())?;
            if current.id != goal.id {
                return Err("goal replaced".into());
            }
            *goal = current;
            if matches!(goal.state, GoalState::Active | GoalState::Completed)
                && (usage.active > 0 || goal.state == GoalState::Completed && usage.waiting)
            {
                tokio::select! {_=cancel.cancelled()=>{},_=revoked.cancelled()=>{},_=self.stopping.cancelled()=>{},_=tokio::time::sleep(Duration::from_millis(50))=>{}}
                continue;
            }
            if goal.state == GoalState::Completed && usage.execution_failed {
                return Err("background CLI execution did not finish cleanly; inspect retained effects, do not replay".into());
            }
            if goal.state != GoalState::Active {
                return Ok(goal.clone());
            }
            if let Some(reason) = allowance(goal) {
                return Err(reason.into());
            }
            if goal.limits.max_tokens.is_some() && usage.uncertain {
                return Err(
                    "CLI token usage is incomplete; a token-limited goal cannot continue safely"
                        .into(),
                );
            }
            // Let owned background inference/notifications finish and account before
            // scheduling another worker. Sleeping creates no authority or replay path.
            if usage.waiting {
                tokio::select! {_=cancel.cancelled()=>{},_=revoked.cancelled()=>{},_=self.stopping.cancelled()=>{},_=tokio::time::sleep(Duration::from_millis(50))=>{}}
                continue;
            }
            if !tokio::task::block_in_place(|| self.store.queue(session))
                .map_err(|e| e.to_string())?
                .is_empty()
            {
                return Err(goals::REASON_USER_INPUT.into());
            }
            let ownership = match tokio::task::block_in_place(|| {
                owner.begin(&self.store, GoalTurn { view: goal.clone() })
            }) {
                Ok(turn) => turn,
                Err(OwnerError::Busy) => {
                    tokio::select! {_=cancel.cancelled()=>{},_=revoked.cancelled()=>{},_=self.stopping.cancelled()=>{},_=tokio::time::sleep(Duration::from_millis(50))=>{}}
                    continue;
                }
                Err(e) => return Err(format!("CLI goal admission refused: {e:?}; do not replay")),
            };
            let request = goals::request_key(goal.id, goal.epoch + 1);
            let mut text = goals::turn_prompt_structured(&goal.objective, &goal.criteria);
            if let Some(findings) = goals::findings_section(goal) {
                text.push_str(&findings);
            }
            #[cfg(test)]
            let (peer_send, peer_receive) = oneshot::channel();
            let receipt = self.submit_cli_agent_root(Start {
                images: vec![],
                ownership,
                request: request.clone(),
                generation,
                operation: crate::util::uuid_v4(),
                text,
                system: resources.root_system.to_string(),
                assets_parent: resources.image.assets_parent().into(),
                image: resources.image.clone(),
                tools: resources.tools.clone(),
                services: Services {
                    child_resources: Some(resources.clone()),
                    shell: resources.shell.clone(),
                    #[cfg(test)]
                    child_peers: resources.peers.clone(),
                    ..Services::default()
                },
                deadline: Instant::now() + Duration::from_secs(30 * 60),
                cancel: cancel.clone(),
                #[cfg(test)]
                test_peer: Some(peer_send),
                #[cfg(test)]
                test_preparing: None,
            });
            #[cfg(test)]
            if let Some(peers) = resources.peers.clone() {
                self.spawn(async move {
                    if let Ok(peer) = peer_receive.await {
                        let _ = peers.send(peer);
                    }
                });
            }
            let receipt = receipt.await;
            // Refresh the actual scheduled epoch even on failure, so cleanup only pauses
            // our own admitted turn. A replaced/revised goal is never settled by this actor.
            let current = tokio::task::block_in_place(|| self.goal_view(session))
                .map_err(|e| e.message)?
                .ok_or("Goal archived")?;
            if current.id != goal.id || current.last_request.as_deref() != Some(request.as_str()) {
                return Err("goal changed before turn admission".into());
            }
            *goal = current;
            if !matches!(receipt,Ok(Ok(ref r)) if r.state==State::Completed && !r.fenced) {
                return Err(
                    "CLI goal turn did not finish cleanly; inspect retained effects, do not replay"
                        .into(),
                );
            }
            *goal = tokio::task::block_in_place(|| self.cli_goal_settle(owner, goal))?;
            if goal.state != GoalState::Active {
                return Ok(goal.clone());
            }
        }
    }
    fn cli_goal_settle(&self, owner: &GoalOwner, goal: &GoalView) -> Result<GoalView, String> {
        let session = owner.session();
        let (before, _, _) = self
            .store
            .read(|c| Store::goal_turn_marks(c, session, goal.id))
            .map_err(|e| e.to_string())?;
        let revision = self
            .store
            .read(|c| Store::plan_state(c, session))
            .map_err(|e| e.to_string())?
            .revision;
        let signature = goals::progress_signature(
            &goal.criteria,
            goal.note.as_deref(),
            &goal.linked_tasks,
            revision,
        );
        let decision = goal.decision.as_ref().filter(|d| d.epoch == goal.epoch);
        let verdict = match decision.map(|d| d.kind.as_str()) {
            Some("continue") => goals::Verdict::Continue,
            Some("blocked") => goals::Verdict::Blocked,
            Some("unreachable") => goals::Verdict::Unreachable,
            Some("met") => goals::Verdict::Met,
            _ => goals::verdict(
                &self
                    .store
                    .cli_goal_answer(
                        session,
                        goal.last_request.as_deref().ok_or("Missing goal turn")?,
                    )
                    .map_err(|e| e.to_string())?,
            ),
        };
        let continuing = verdict == goals::Verdict::Continue;
        let stalled = if continuing && before.as_deref() == Some(&signature) {
            goal.stall_turns.saturating_add(1)
        } else {
            0
        };
        let usage = self
            .store
            .read(|c| Store::cli_goal_usage_in(c, session, goal.id))
            .map_err(|e| e.to_string())?;

        let threshold = self.goal_stall_threshold(session);
        self.cli_goal_write(owner, Some(goal), |tx| {
            if usage.uncertain {
                const NOTE:&str="CLI token usage is incomplete; the token count is a known minimum.";
                let mut view=goal.clone();
                if !view.note.as_deref().is_some_and(|n|n.contains(NOTE)) {
                    let note=view.note.as_deref().unwrap_or_default().chars().take(2000).collect::<String>();
                    view.note=Some(format!("{note}\n{NOTE}"));
                    Store::goal_write_progress(tx,session,&view,now_ms())?;
                }
            }
            Store::goal_account(
                tx,
                session,
                goal.id,
                0,
                0.0,
                Some(stalled),
                now_ms(),
            )?;
            Store::goal_set_turn_marks(tx, session, goal.id, Some(&signature), None, None)?;
            if goal.state == GoalState::Active {
                let transition=if usage.uncertain && goal.limits.max_tokens.is_some() {
                    Some((GoalState::Paused,"Paused: CLI token usage is incomplete; review the allowance before resuming."))
                } else {match verdict {
                    goals::Verdict::Continue if stalled>=threshold=>Some((GoalState::Paused,"Paused: no change to goal progress; revise or resume.")),
                    goals::Verdict::Continue=>None,
                    goals::Verdict::Blocked=>Some((GoalState::Blocked,"Blocked by the executor's final-line fallback; input needed.")),
                    goals::Verdict::Unreachable=>Some((GoalState::Failed,"Reported unreachable by the executor's final-line fallback.")),
                    goals::Verdict::Met=>Some((GoalState::Paused,"Paused: completion needs a native evidence proposal and any required independent audit.")),
                    goals::Verdict::Ambiguous=>Some((GoalState::Paused,goals::REASON_NO_STATUS)),
                }};
                if let Some((state,reason))=transition {Store::goal_set_state(tx,session,goal.id,state,Some(reason),now_ms())?;}
            }
            Ok(())
        })
    }
}
fn allowance(goal: &GoalView) -> Option<&'static str> {
    if goal.limits.max_cost_usd.is_some() {
        return Some(
            "CLI dollar usage is unavailable; change the dollar allowance explicitly before starting.",
        );
    }
    if goal.limits.max_turns.is_some_and(|m| goal.turns >= m) {
        Some(goals::REASON_TURNS_USED)
    } else if goal
        .limits
        .max_tokens
        .is_some_and(|m| goal.usage_tokens >= m)
    {
        Some(goals::REASON_TOKENS_USED)
    } else {
        None
    }
}
