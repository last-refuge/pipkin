//! Explicit desktop routes for the native-tool CLI executor. Ordinary CLI Coding never
//! enters here; read-only receipts cannot reconstruct the original connected approval.
use super::{
    Shared, cli_agent_host::ChildResources, cli_agent_runtime::admission::Start,
    cli_bridge_driver::Services,
};
use crate::{
    context::prompt::SystemPrompt,
    execution::claude_cli::orchestration_policy::HelperImage,
    store::{
        Store,
        cli_agent_turns::{Input, inputs::Kind},
    },
    util::{now_ms, uuid_v4},
};
use pipkin_engine_protocol::*;
use std::{sync::Arc, time::Duration};
use tokio::{sync::oneshot, time::Instant};

fn unavailable() -> ProtoError {
    ProtoError::new(
        ErrorCode::Unsupported,
        "Connect this Claude orchestration conversation first. Changed or uncertain execution requires a separate conversation.",
    )
}
fn failed(_: impl std::fmt::Debug) -> ProtoError {
    ProtoError::new(
        ErrorCode::Conflict,
        "Claude orchestration refused the operation; inspect the conversation status before retrying.",
    )
}
impl Shared {
    pub(super) fn cli_desktop_ready(&self, owner: u64, session: &str) -> Result<(), ProtoError> {
        if !self.local_cli_execution_enabled(LocalCliMode::Orchestration)
            || !self.cli_agents.approved(&self.store, owner, session)
        {
            return Err(unavailable());
        }
        Ok(())
    }
    pub(super) fn cli_desktop_resources(
        &self,
        session: &str,
    ) -> Result<Arc<ChildResources>, ProtoError> {
        #[cfg(test)]
        if let Some(resources) = self
            .cli_desktop_test_resources
            .lock()
            .unwrap()
            .get(session)
            .cloned()
        {
            return Ok(resources);
        }
        let image = self
            .cli_helper
            .get_or_init(|| HelperImage::capture_in(&self.config.data_dir).map_err(str::to_owned))
            .as_ref()
            .map_err(failed)?
            .clone();
        let row = self.store.session(session)?.ok_or_else(unavailable)?;
        let sess = self.session(session)?;
        let tools = self.tools_for(&row);
        let settings = self.settings_for(session);
        self.system_sections(
            &sess,
            &row,
            &tools.iter().map(|t| t.spec()).collect::<Vec<_>>(),
            &settings,
        );
        let prompt = sess
            .prompt_resources
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .captured(&row.cwd)
            .ok_or_else(unavailable)?;
        let system = SystemPrompt::build(&prompt);
        self.capture_cli_child_resources(
            session,
            &system,
            tools,
            image,
            settings.shell_path.map(Into::into),
        )
        .map_err(failed)
    }
    pub(super) async fn prompt_cli_desktop(
        self: &Arc<Self>,
        owner: u64,
        session: &str,
        request: &str,
        text: &str,
        images: &[ImageInput],
        model: Option<&str>,
    ) -> Result<Reply, ProtoError> {
        if model.is_some() {
            return Err(ProtoError::new(
                ErrorCode::Unsupported,
                "Choose a model when creating a separate CLI conversation.",
            ));
        }
        if request.is_empty() || request.len() > limits::MAX_KEY_BYTES {
            return Err(ProtoError::new(ErrorCode::Invalid, "Invalid request key"));
        }
        crate::execution::claude_cli::orchestration_input::validate(text, images)
            .map_err(failed)?;
        let binding = self
            .store
            .cli_agent_binding(session)?
            .ok_or_else(unavailable)?;
        // Stable desktop request identity. Duplicate lookup is allowed even after disconnect.
        let input = Input {
            mode: InputMode::Prompt,
            session,
            request,
            generation: 1,
            operation: request,
            text,
            images,
            binding: binding.digest,
        };
        if let Some(receipt) = self.store.lookup_cli_agent_turn(&input)? {
            return Ok(Reply::Admitted {
                run: Some(receipt.scope.run),
                entry: None,
            });
        }
        self.cli_desktop_ready(owner, session)?;
        let ownership = self
            .cli_agents
            .begin_turn(&self.store, owner, session)
            .map_err(failed)?;
        let resources = tokio::task::block_in_place(|| self.cli_desktop_resources(session))?;
        let (send, receive) = oneshot::channel();
        #[cfg(test)]
        let (peer_send, peer_receive) = oneshot::channel();
        #[cfg(test)]
        let peers = resources.peers.clone();
        let completion = self.submit_cli_agent_root_notified(
            Start {
                ownership,
                request: request.into(),
                generation: 1,
                operation: request.into(),
                text: text.into(),
                images: images.to_vec(),
                system: resources.root_system.to_string(),
                assets_parent: self.config.data_dir.clone(),
                image: resources.image.clone(),
                tools: resources.tools.clone(),
                services: Services {
                    shell: resources.shell.clone(),
                    child_resources: Some(resources),
                    ..Default::default()
                },
                deadline: Instant::now() + Duration::from_secs(30 * 60),
                cancel: super::request_cancel::capture(),
                #[cfg(test)]
                test_peer: Some(peer_send),
                #[cfg(test)]
                test_preparing: None,
            },
            Some(send),
        );
        #[cfg(test)]
        if let Some(peers) = peers {
            self.spawn(async move {
                if let Ok(peer) = peer_receive.await {
                    let _ = peers.send(peer);
                }
            });
        }
        match receive.await {
            Ok(receipt) => Ok(Reply::Admitted {
                run: Some(receipt.scope.run),
                entry: None,
            }),
            Err(_) => {
                let result = completion.await.map_err(failed)?;
                result
                    .map(|r| Reply::Admitted {
                        run: Some(r.scope.run),
                        entry: None,
                    })
                    .map_err(failed)
            }
        }
    }
    pub(super) async fn route_cli_desktop(
        self: &Arc<Self>,
        owner: u64,
        call: &Call,
    ) -> Option<Result<Reply, ProtoError>> {
        let session = match call {
            Call::Enqueue { session, .. }
            | Call::CancelQueued { session, .. }
            | Call::Compact { session, .. }
            | Call::SetGoal { session, .. }
            | Call::GoalControl { session, .. }
            | Call::UpdatePlan { session, .. }
            | Call::JobCancel { session, .. }
            | Call::SetSubagentsEnabled { session, .. }
            | Call::StopSubagent { session, .. }
            | Call::FollowUpSubagent { session, .. }
            | Call::LaunchSubagent { session, .. }
            | Call::SteerSubagent { session, .. } => session,
            _ => return None,
        };
        if !self
            .store
            .session(session)
            .ok()
            .flatten()
            .is_some_and(|r| r.execution_backend == ExecutionBackend::ClaudeCliAgent)
        {
            return None;
        }
        Some(self.cli_desktop_operation(owner, session, call).await)
    }
    async fn cli_desktop_operation(
        self: &Arc<Self>,
        owner: u64,
        session: &str,
        call: &Call,
    ) -> Result<Reply, ProtoError> {
        if let Call::FollowUpSubagent {
            child,
            request,
            text,
            ..
        } = call
        {
            return self
                .launch_cli_desktop_child_inner(
                    owner,
                    session,
                    request,
                    text,
                    &None,
                    &None,
                    Some(*child),
                )
                .await;
        }
        if let Call::LaunchSubagent {
            request,
            task,
            profile,
            context,
            ..
        } = call
        {
            return self
                .launch_cli_desktop_child(owner, session, request, task, profile, context)
                .await;
        }
        self.cli_desktop_ready(owner, session)?;
        match call {
            Call::UpdatePlan {
                request,
                ops,
                expected_revision,
                ..
            } => {
                if request.is_empty() || request.len() > limits::MAX_KEY_BYTES {
                    return Err(failed("request key"));
                }
                match self.update_plan(
                    session,
                    &format!("client:{request}"),
                    ops,
                    *expected_revision,
                )? {
                    Ok(r) => Ok(Reply::PlanUpdated(r)),
                    Err(e) => Err(ProtoError::new(e.code, e.message)),
                }
            }
            Call::JobCancel { id, .. } => self.job_cancel(session, *id).await.map(Reply::Job),
            Call::SetSubagentsEnabled { enabled, .. } => {
                self.store.set_session_field(
                    session,
                    crate::store::SessionField::SubagentsEnabled(*enabled),
                )?;
                self.publish_subagents(session);
                Ok(Reply::Ok)
            }
            Call::SteerSubagent {
                child,
                request,
                text,
                ..
            } => self.steer_cli_desktop_child(owner, session, *child, request, text),
            Call::StopSubagent { child, .. } => {
                let task = self.authorized_child(session, *child)?;
                self.stop_by_backend(&task.child_session).await?;
                Ok(Reply::Subagent(self.subagent_row(session, *child)?))
            }
            Call::Enqueue {
                request,
                mode,
                text,
                images,
                ..
            } => {
                let kind = match mode {
                    InputMode::FollowUp => Kind::FollowUp,
                    InputMode::Steer => Kind::Steer,
                    _ => return Err(failed("Use Prompt")),
                };
                let epoch = self
                    .cli_agents
                    .input_epoch(owner, session)
                    .map_err(failed)?;
                let resources =
                    tokio::task::block_in_place(|| self.cli_desktop_resources(session))?;
                let q = self
                    .enqueue_cli_input_during(
                        super::cli_agent_inputs::Input {
                            owner,
                            session: session.into(),
                            request: request.clone(),
                            generation: 1,
                            operation: request.clone(),
                            kind,
                            text: text.clone(),
                            images: images.clone(),
                        },
                        resources,
                        Some(&epoch),
                    )
                    .map_err(failed)?;
                Ok(Reply::Admitted {
                    run: None,
                    entry: Some(q.entry),
                })
            }
            Call::CancelQueued { entry, .. } => self.cancel_queued(session, *entry).await,
            Call::Compact { instructions, .. } => {
                let epoch = self
                    .cli_agents
                    .input_epoch(owner, session)
                    .map_err(failed)?;
                let resources =
                    tokio::task::block_in_place(|| self.cli_desktop_resources(session))?;
                let request = uuid_v4();
                self.enqueue_cli_input_during(
                    super::cli_agent_inputs::Input {
                        owner,
                        session: session.into(),
                        operation: request.clone(),
                        request,
                        generation: 1,
                        kind: Kind::Compact,
                        text: instructions.clone().unwrap_or_default(),
                        images: vec![],
                    },
                    resources,
                    Some(&epoch),
                )
                .map_err(failed)?;
                Ok(Reply::Ok)
            }
            Call::SetGoal { objective, .. } => {
                self.cli_desktop_goal(owner, session, Some(objective.clone()))
                    .await
            }
            Call::GoalControl {
                action: GoalAction::Resume,
                ..
            } => self.cli_desktop_goal(owner, session, None).await,
            Call::GoalControl {
                action: GoalAction::Revise { objective },
                ..
            } => {
                self.cli_desktop_goal(owner, session, Some(objective.clone()))
                    .await
            }
            Call::GoalControl { action, .. } => {
                let _gate = self.publication.lock().unwrap_or_else(|e| e.into_inner());
                let (goal, revision) = self
                    .cli_agents
                    .authorize_desktop(&self.store, owner, session, || {
                        self.store.tx(|tx| {
                            let mut goal = Store::goal_current(tx, session)?
                                .ok_or(crate::store::StoreError::ExecutionConflict)?;
                            match action {
                                GoalAction::Pause => Store::goal_set_state(
                                    tx,
                                    session,
                                    goal.id,
                                    GoalState::Paused,
                                    Some(crate::goals::REASON_PAUSED_BY_PERSON),
                                    now_ms(),
                                )?,
                                GoalAction::Archive => Store::goal_archive(
                                    tx,
                                    session,
                                    goal.id,
                                    crate::goals::REASON_ARCHIVED,
                                    now_ms(),
                                )?,
                                GoalAction::SetLimits {
                                    max_turns,
                                    max_tokens,
                                    max_cost_usd,
                                } => {
                                    if max_cost_usd.is_some() {
                                        return Err(crate::store::StoreError::ExecutionConflict);
                                    }
                                    goal.limits = GoalLimits {
                                        max_turns: max_turns.filter(|v| *v > 0),
                                        max_tokens: max_tokens.filter(|v| *v > 0),
                                        max_cost_usd: None,
                                    };
                                    Store::goal_write_progress(tx, session, &goal, now_ms())?;
                                }
                                GoalAction::SetCriteria { criteria } => {
                                    goal.criteria = crate::goals::criteria_from_texts(criteria)
                                        .map_err(|_| crate::store::StoreError::ExecutionConflict)?;
                                    goal.stall_turns = 0;
                                    goal.decision = None;
                                    Store::goal_write_progress(tx, session, &goal, now_ms())?;
                                    if goal.state == GoalState::Completed {
                                        Store::goal_set_state(
                                            tx,
                                            session,
                                            goal.id,
                                            GoalState::Paused,
                                            Some("Criteria changed"),
                                            now_ms(),
                                        )?;
                                    }
                                }
                                GoalAction::SkipAudit => {
                                    let waiting = goal.state == GoalState::Verifying
                                        || (goal.state == GoalState::Paused
                                            && goal.audit.result.as_ref().is_some_and(|r| {
                                                matches!(
                                                    r.verdict.as_str(),
                                                    "changes_requested"
                                                        | "inconclusive"
                                                        | "stale"
                                                        | "failed"
                                                )
                                            }));
                                    if !waiting {
                                        return Err(crate::store::StoreError::ExecutionConflict);
                                    }
                                    let result = goal.audit.result.get_or_insert(GoalAuditResult {
                                        round: goal.audit.rounds,
                                        verdict: String::new(),
                                        findings: vec![],
                                        summary: None,
                                        fingerprint: None,
                                        child: None,
                                    });
                                    result.verdict = "skipped".into();
                                    // The pending verdict has no child number until normal
                                    // settlement. Capture the exact frozen audit now so late
                                    // cancellation accounting can identify the skipped child.
                                    if result.child.is_none() {
                                        use rusqlite::OptionalExtension;
                                        result.child = tx.query_row(
                                            "SELECT t.child FROM cli_agent_child_policies p JOIN subagent_tasks t ON t.child_session=p.child AND t.parent_session=p.parent AND t.parent_call=p.parent_call WHERE p.parent=?1 AND json_extract(p.snapshot,'$.cliNative.audit.id')=?2 AND json_extract(p.snapshot,'$.cliNative.audit.audit.rounds')=?3 ORDER BY t.child DESC LIMIT 1",
                                            rusqlite::params![session, goal.id, goal.audit.rounds],
                                            |r| r.get(0),
                                        ).optional()?;
                                    }
                                    Store::goal_write_progress(tx, session, &goal, now_ms())?;
                                    Store::goal_set_state(
                                        tx,
                                        session,
                                        goal.id,
                                        GoalState::Completed,
                                        Some(crate::goals::REASON_AUDIT_SKIPPED),
                                        now_ms(),
                                    )?;
                                }
                                GoalAction::SetAudit {
                                    required,
                                    max_rounds,
                                } => {
                                    goal.audit.required = *required;
                                    goal.audit.max_rounds = max_rounds
                                        .filter(|v| *v > 0)
                                        .unwrap_or(if goal.audit.max_rounds == 0 {
                                            crate::goals::DEFAULT_AUDIT_ROUNDS
                                        } else {
                                            goal.audit.max_rounds
                                        });
                                    Store::goal_write_progress(tx, session, &goal, now_ms())?;
                                }
                                _ => return Err(crate::store::StoreError::ExecutionConflict),
                            }
                            Ok((
                                Store::goal_current(tx, session)?,
                                Store::bump(tx, session, now_ms())?,
                            ))
                        })
                    })
                    .map_err(failed)?;
                if matches!(action, GoalAction::SkipAudit)
                    && let Some(goal) = &goal
                {
                    self.cli_agents.cancel_desktop_auditors(
                        owner,
                        session,
                        goal.id,
                        goal.audit.rounds,
                    );
                }
                self.publish(Event::Session {
                    session: session.into(),
                    revision,
                    change: SessionChange::GoalChanged {
                        goal: goal.clone().map(Box::new),
                    },
                });
                Ok(Reply::Goal(goal))
            }
            _ => Err(unavailable()),
        }
    }
    async fn cli_desktop_goal(
        self: &Arc<Self>,
        owner: u64,
        session: &str,
        objective: Option<String>,
    ) -> Result<Reply, ProtoError> {
        let epoch = self
            .cli_agents
            .input_epoch(owner, session)
            .map_err(failed)?;
        let resources = tokio::task::block_in_place(|| self.cli_desktop_resources(session))?;
        let (send, receive) = oneshot::channel();
        let completion = self
            .start_cli_goal_notified(
                owner,
                session,
                objective,
                resources,
                1,
                Some(send),
                Some(&epoch),
            )
            .map_err(failed)?;
        match receive.await {
            Ok(goal) => Ok(Reply::Goal(Some(goal))),
            Err(_) => completion
                .await
                .map_err(failed)?
                .map(|g| Reply::Goal(Some(g)))
                .map_err(failed),
        }
    }
}
