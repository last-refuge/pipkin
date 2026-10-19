//! Fresh and continued inline/background CLI child workflows. The engine owns preparation through result/cleanup;
//! dropping the tool's waiter never transfers work to an API runner or drops the actor.
use super::*;
use crate::{
    context::prompt::PromptInput,
    execution::claude_cli::{orchestration_child, orchestration_policy::HelperImage},
    runtime::{
        cli_agent_root::Error, cli_agent_runtime::admission::Start, cli_bridge_driver::Services,
    },
    tools::Tool,
};
use pipkin_engine_protocol::Event;
use std::path::PathBuf;
use tokio::sync::oneshot;
#[derive(Clone)]
pub(in crate::runtime) struct ChildResources {
    pub prompt: Arc<PromptInput>,
    pub root_system: Arc<str>,
    pub tools: Vec<Arc<dyn Tool>>,
    pub image: Arc<HelperImage>,
    pub shell: Option<PathBuf>,
    #[cfg(test)]
    pub job_start_pause: Option<Arc<super::StartPause>>,
    #[cfg(test)]
    pub peers:
        Option<tokio::sync::mpsc::UnboundedSender<crate::runtime::cli_bridge_channel::Launch>>,
}
impl Shared {
    pub(in crate::runtime) fn capture_cli_child_resources(
        &self,
        session: &str,
        system: &str,
        tools: Vec<Arc<dyn Tool>>,
        image: Arc<HelperImage>,
        shell: Option<PathBuf>,
    ) -> Result<Arc<ChildResources>, Error> {
        let sess = self.session(session).map_err(|_| Error::Preparation)?;
        let row = self
            .store
            .session(session)
            .map_err(|_| Error::Storage)?
            .ok_or(Error::Preparation)?;
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
            .ok_or(Error::Preparation)?;
        Ok(Arc::new(ChildResources {
            prompt: Arc::new(prompt),
            root_system: Arc::from(system),
            tools,
            image,
            shell,
            #[cfg(test)]
            peers: None,
            #[cfg(test)]
            job_start_pause: None,
        }))
    }
}
struct ChildWork {
    host: Host,
    launch: SubagentLaunch,
    caller: CancellationToken,
    _tracking: tokio_util::task::task_tracker::TaskTrackerToken,
}
impl Host {
    pub(super) fn start_child(
        &self,
        launch: SubagentLaunch,
        caller: CancellationToken,
    ) -> oneshot::Receiver<Result<(String, Value), String>> {
        let work = ChildWork {
            host: self.clone(),
            launch,
            caller,
            _tracking: self.shared.external_tasks.token(),
        };
        let (send, receive) = oneshot::channel();
        self.shared.spawn(async move {
            let ChildWork {
                host,
                launch,
                caller,
                _tracking,
            } = work;
            let mut reply = Some(send);
            let result = host.run_child(launch, caller, &mut reply).await;
            drop(host);
            if let Some(reply) = reply {
                let _ = reply.send(result);
            } else if let Err(error) = result {
                log::warn!("Background CLI child did not finish cleanly: {error}");
            }
            drop(_tracking);
        });
        receive
    }
    pub(super) fn child_view(&self, revision: u64) {
        match self.shared.subagents_view(&self.authority.scope().session) {
            Ok(subagents) => self.shared.publish(Event::Session {
                session: self.authority.scope().session.clone(),
                revision,
                change: SessionChange::SubagentsChanged { subagents },
            }),
            Err(_) => self.shared.publish(Event::ResyncRequired),
        }
    }
    async fn run_child(
        &self,
        launch: SubagentLaunch,
        caller: CancellationToken,
        reply: &mut Option<oneshot::Sender<Result<(String, Value), String>>>,
    ) -> Result<(String, Value), String> {
        let background = !launch.wait;
        tokio::task::block_in_place(|| self.check())?;
        let cancel = self.cancel.child_token();
        if caller.is_cancelled() {
            return Err("CLI child cancelled before admission".into());
        }
        let wake = if !launch.wait && launch.wake {
            Some(
                tokio::task::block_in_place(|| {
                    self.authority.wake_ticket(
                        &self.shared.store,
                        &self.lease,
                        &self.call,
                        "subagent",
                    )
                })
                .map_err(|e| format!("CLI wake ownership refused: {e:?}"))?,
            )
        } else {
            None
        };
        let continuing = launch.child.is_some();
        let (created, ownership, policy, resources) = if continuing {
            let (created, ownership) = tokio::task::block_in_place(|| {
                let _gate = self
                    .shared
                    .publication
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                self.authority.continue_child(
                    &self.shared.store,
                    self.lease.clone(),
                    &self.call,
                    &launch,
                )
            })
            .map_err(|e| format!("CLI child continuation refused: {e:?}"))?;
            let policy = ownership
                .child_policy_arc()
                .ok_or("Missing original child policy")?;
            let resources = ownership
                .child_resources()
                .ok_or("Missing original child resources")?;
            (created, ownership, policy, resources)
        } else {
            let policy = Arc::new(tokio::task::block_in_place(|| {
                let settings = self.shared.settings_for(&self.authority.scope().session);
                let tools = self
                    .children
                    .tools
                    .iter()
                    .map(|t| t.spec())
                    .collect::<Vec<_>>();
                let captured_at = now_ms();
                let capture = |entries: &[_], revision| {
                    if self.name == "propose_goal_completion" {
                        let goal = self
                            .shared
                            .goal_view(&self.authority.scope().session)
                            .map_err(|e| e.message)?
                            .ok_or("Goal disappeared")?;
                        return orchestration_child::capture_auditor(
                            self.authority.scope(),
                            &self.call,
                            self.authority.binding(),
                            &settings,
                            &launch,
                            &tools,
                            &self.children.prompt,
                            captured_at,
                            &goal,
                        )
                        .map_err(|e| format!("CLI auditor policy refused: {e:?}"));
                    }
                    orchestration_child::capture(
                        self.authority.scope(),
                        &self.call,
                        self.authority.binding(),
                        &settings,
                        &launch,
                        &tools,
                        &self.children.prompt,
                        entries,
                        revision,
                        captured_at,
                    )
                    .map_err(|e| format!("CLI child policy refused: {e:?}"))
                };
                let policy = capture(&[], 0)?;
                if !policy.inherits() {
                    return Ok(policy);
                }
                self.transaction(|c| {
                    let (entries, revision) =
                        Store::cli_child_context_in(c, &self.lease, &self.call)?;
                    let policy =
                        capture(&entries, revision).map_err(|_| StoreError::ExecutionConflict)?;
                    Ok((policy, None))
                })
            })?);
            let checked = self
                .authority
                .prepare_child(&policy, &cancel, self.deadline);
            tokio::pin!(checked);
            let prepared = tokio::select! {biased;
                _=caller.cancelled()=>{cancel.cancel();let _=checked.await;return Err("CLI child cancelled before creation".into());},
                result=&mut checked=>result.map_err(|e|format!("CLI child preflight refused: {e:?}"))?,
            };
            let (created, ownership) = tokio::task::block_in_place(|| {
                let _gate = self
                    .shared
                    .publication
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let result = self
                    .authority
                    .create_child(
                        &self.shared.store,
                        &self.lease,
                        policy.clone(),
                        self.children.clone(),
                        &prepared,
                        &cancel,
                        self.deadline,
                    )
                    .map_err(|e| format!("CLI child creation refused: {e:?}"))?;
                if let Some(revision) = result.0.revision {
                    self.child_view(revision);
                }
                Ok::<_, String>(result)
            })?;
            let ownership = ownership
                .ok_or("Child already exists; metadata does not authorize a second launch")?;
            (created, ownership, policy.clone(), self.children.clone())
        };
        let text = launch.task.clone();
        let child = created.binding.create.session.clone();
        let child_cancel = ownership.cancellation();
        let mut tools: Vec<Arc<dyn Tool>> = resources
            .tools
            .iter()
            .filter(|t| policy.record().allows(&t.spec().name))
            .cloned()
            .collect();
        if let Some(context) = policy.context_tool() {
            tools.push(context);
        }
        let services = Services {
            child_resources: Some(resources.clone()),
            shell: resources.shell.clone(),
            #[cfg(test)]
            child_peers: resources.peers.clone(),
            ..Services::default()
        };
        #[cfg(test)]
        let (peer_send, peer_receive) = oneshot::channel();
        let start = Start {
            images: vec![],
            ownership,
            request: format!("child:{}", self.call),
            generation: 1,
            operation: crate::util::uuid_v4(),
            text,
            system: policy.system().into(),
            assets_parent: resources.image.assets_parent().to_owned(),
            image: resources.image.clone(),
            tools,
            services,
            deadline: self.deadline,
            cancel: if background {
                child_cancel.clone()
            } else {
                cancel.clone()
            },
            #[cfg(test)]
            test_peer: Some(peer_send),
            #[cfg(test)]
            test_preparing: None,
        };
        let mut result = self.shared.submit_cli_agent_root(start);
        // Creation is the durable, authorized delegation effect. Submission has taken
        // engine ownership synchronously; this does NOT claim a process has started.
        // A dropped waiter cannot authorize another launch. Parent cancellation before
        // a durable successful tool receipt still cancels this unconfirmed delegation.
        if background && let Some(reply) = reply.take() {
            let _ = reply.send(Ok((
                format!("Subagent #{} is starting in the background. {} Use subagent_status, subagent_wait and subagent_result.", created.task.child, if wake.is_some() {"A CLI follow-up is requested when it finishes, unless stopped or ownership is lost."} else {"It will not interrupt you."}),
                serde_json::json!({"conversationId":child,"child":created.task.child}),
            )));
        }
        #[cfg(test)]
        if let Some(peers) = resources.peers.clone() {
            self.shared.spawn(async move {
                if let Ok(peer) = peer_receive.await {
                    let _ = peers.send(peer);
                }
            });
        }
        let cancel_unconfirmed = || {
            let handed_off = background
                && tokio::task::block_in_place(|| {
                    self.shared
                        .store
                        .cli_native_child_call_handed_off(
                            &self.authority.scope().session,
                            &self.call,
                        )
                        .unwrap_or(false)
                });
            if !handed_off {
                child_cancel.cancel();
            }
        };
        let joined = tokio::select! {biased;
            _=caller.cancelled()=>{cancel_unconfirmed();result.await},
            _=cancel.cancelled()=>{cancel_unconfirmed();result.await},
            _=tokio::time::sleep_until(self.deadline)=>{child_cancel.cancel();result.await},
            result=&mut result=>result,
        };
        if continuing
            && tokio::task::block_in_place(|| {
                self.shared
                    .store
                    .cli_agent_turn(&child, &format!("child:{}", self.call))
            })
            .map_err(|e| e.to_string())?
            .is_none()
        {
            return Err(format!(
                "CLI child continuation was not admitted: {joined:?}"
            ));
        }
        let (confirmed, answer) = tokio::task::block_in_place(|| {
            let _gate = self
                .shared
                .publication
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let (confirmed, answer, _, revision) = self
                .shared
                .store
                .tx(|tx| Store::settle_cli_native_child_in(tx, &child, now_ms()))
                .map_err(|e| e.to_string())?;
            self.child_view(revision);
            Ok::<_, String>((confirmed, answer))
        })?;
        if let Some(wake) = wake {
            self.deliver_wake(
                wake,
                crate::store::cli_agent_turns::WakeTarget::Child(child.clone()),
            )
            .await;
        }
        if !confirmed {
            return Err(answer);
        }
        let artifacts = tokio::task::block_in_place(|| self.shared.child_artifacts(&child));
        Ok((
            answer,
            serde_json::json!({"conversationId":child,"child":created.task.child,"artifacts":artifacts}),
        ))
    }
}
