//! Person-launched CLI children: durable notification-only receipts, never fabricated tool calls.
use super::{Shared, cli_agent_runtime::admission::Start, cli_bridge_driver::Services};
use crate::{
    execution::claude_cli::orchestration_child,
    profiles::ContextMode,
    store::Store,
    tools::SubagentLaunch,
    util::{now_ms, uuid_v4},
};
use pipkin_engine_protocol::*;
use std::{sync::Arc, time::Duration};
use tokio::{sync::oneshot, time::Instant};
fn failed(e: impl std::fmt::Debug) -> ProtoError {
    log::debug!("Desktop child refused: {e:?}");
    #[cfg(test)]
    eprintln!("Desktop child refused: {e:?}");
    ProtoError::new(
        ErrorCode::Conflict,
        "Claude child operation refused; inspect its status before retrying.",
    )
}
impl Shared {
    pub(super) async fn launch_cli_desktop_child(
        self: &Arc<Self>,
        owner: u64,
        session: &str,
        request: &str,
        task: &str,
        profile: &Option<String>,
        context: &Option<String>,
    ) -> Result<Reply, ProtoError> {
        if request.is_empty() || request.len() > 240 || request.chars().any(char::is_control) {
            return Err(failed("invalid request"));
        }
        let context = match context.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(s) => Some(ContextMode::parse(s).ok_or_else(|| failed("invalid context"))?),
        };
        let launch = SubagentLaunch {
            child: None,
            task: task.into(),
            profile: profile
                .as_ref()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty()),
            context,
            wait: false,
            wake: false,
            model: None,
        };
        let call = format!("user:{request}");
        let intent = serde_json::json!({"task":launch.task,"profile":launch.profile,"context":launch.context.map(|v|v.as_str())});
        // Lost acknowledgements read the original receipt, even after approval was revoked.
        if let Some(row) = self
            .store
            .read(|c| Store::task_for_call(c, session, &call))?
        {
            let stored = self
                .store
                .read(|c| Store::cli_native_child_policy_in(c, &row.child_session))?
                .ok_or_else(|| failed("missing policy"))?;
            let snapshot: serde_json::Value = serde_json::from_str(&stored.1).map_err(failed)?;
            if row.origin != "user" || snapshot["cliNative"]["personIntent"] != intent {
                return Err(failed("request changed"));
            }
            return Ok(Reply::Subagent(self.subagent_row(session, row.child)?));
        }
        self.cli_desktop_ready(owner, session)?;
        let authority = self
            .cli_agents
            .person_child(&self.store, owner, session, request)
            .map_err(failed)?;
        let resources = tokio::task::block_in_place(|| self.cli_desktop_resources(session))?;
        let policy = tokio::task::block_in_place(|| {
            let settings = self.settings_for(session);
            let tools = resources.tools.iter().map(|t| t.spec()).collect::<Vec<_>>();
            let capture = |entries: &[_], revision| {
                orchestration_child::capture_person(
                    &authority.scope,
                    &call,
                    authority.binding(),
                    &settings,
                    &launch,
                    &tools,
                    &resources.prompt,
                    entries,
                    revision,
                    now_ms(),
                )
                .map_err(failed)
            };
            let policy = capture(&[], 0)?;
            if !policy.inherits() {
                return Ok(Arc::new(policy));
            }
            let (entries, revision) = authority.context(&self.store).map_err(failed)?;
            capture(&entries, revision).map(Arc::new)
        })?;
        let caller = super::request_cancel::capture();
        let shared = self.clone();
        let tracking = self.external_tasks.token();
        let (send, receive) = oneshot::channel();
        let parent = session.to_owned();
        self.spawn(async move {
            let mut send = Some(send);
            let result: Result<(), ProtoError> = async {
                let deadline = Instant::now() + Duration::from_secs(30 * 60);
                let prepared = authority
                    .prepare(&policy, caller.clone(), deadline)
                    .await
                    .map_err(failed)?;
                let (created, ownership) = tokio::task::block_in_place(|| {
                    let _gate = shared.publication.lock().unwrap_or_else(|e| e.into_inner());
                    let result = authority
                        .create(
                            &shared.store,
                            policy.clone(),
                            resources.clone(),
                            &prepared,
                            &caller,
                        )
                        .map_err(failed)?;
                    drop(_gate);
                    shared.publish_subagents(&parent);
                    Ok::<_, ProtoError>(result)
                })?;
                let Some(ownership) = ownership else {
                    let _ = send.take().unwrap().send(
                        shared
                            .subagent_row(&parent, created.task.child)
                            .map(Reply::Subagent),
                    );
                    return Ok(());
                };
                let child = created.binding.create.session.clone();
                let cancel = ownership.cancellation();
                let mut tools = resources
                    .tools
                    .iter()
                    .filter(|t| policy.record().allows(&t.spec().name))
                    .cloned()
                    .collect::<Vec<_>>();
                if let Some(context) = policy.context_tool() {
                    tools.push(context);
                }
                #[cfg(test)]
                let (peer_send, peer_receive) = oneshot::channel();
                let completion = shared.submit_cli_agent_root(Start {
                    ownership,
                    request: format!("child:{call}"),
                    generation: 1,
                    operation: uuid_v4(),
                    text: launch.task,
                    images: vec![],
                    system: policy.system().into(),
                    assets_parent: resources.image.assets_parent().into(),
                    image: resources.image.clone(),
                    tools,
                    services: Services {
                        child_resources: Some(resources.clone()),
                        shell: resources.shell.clone(),
                        ..Default::default()
                    },
                    deadline,
                    cancel,
                    #[cfg(test)]
                    test_peer: Some(peer_send),
                    #[cfg(test)]
                    test_preparing: None,
                });
                #[cfg(test)]
                if let Some(peers) = resources.peers.clone() {
                    shared.spawn(async move {
                        if let Ok(peer) = peer_receive.await {
                            let _ = peers.send(peer);
                        }
                    });
                }
                // Engine tracking and submission precede acknowledgement. Caller cancellation
                // after this durable handoff cannot strand or replay the committed child.
                let _ = send.take().unwrap().send(
                    shared
                        .subagent_row(&parent, created.task.child)
                        .map(Reply::Subagent),
                );
                let _ = completion.await;
                tokio::task::block_in_place(|| {
                    let _gate = shared.publication.lock().unwrap_or_else(|e| e.into_inner());
                    shared
                        .store
                        .tx(|c| Store::settle_cli_native_child_in(c, &child, now_ms()))?;
                    drop(_gate);
                    shared.publish_subagents(&parent);
                    Ok::<_, ProtoError>(())
                })?;
                Ok(())
            }
            .await;
            if let Some(send) = send {
                let _ = send.send(result.map(|_| Reply::Ok));
            } else if let Err(e) = result {
                log::warn!("Desktop child settlement: {}", e.message);
                shared.publish(Event::ResyncRequired);
            }
            drop(tracking);
        });
        receive.await.map_err(failed)?
    }
}

impl Shared {
    pub(super) fn steer_cli_desktop_child(
        self: &Arc<Self>,
        owner: u64,
        parent: &str,
        child: u64,
        request: &str,
        text: &str,
    ) -> Result<Reply, ProtoError> {
        if request.is_empty()
            || request.len() > 240
            || text.trim().is_empty()
            || text.len() > 64 * 1024
        {
            return Err(failed("invalid steering"));
        }
        let task = self.authorized_child(parent, child)?;
        let q = crate::store::cli_agent_turns::inputs::Queued {
            session: task.child_session.clone(),
            request: format!("steer:{request}"),
            operation: format!("steer:{request}"),
            generation: 1,
            kind: crate::store::cli_agent_turns::inputs::Kind::Steer,
            text: text.into(),
            images: vec![],
            ticket: uuid_v4(),
        };
        let ticket = {
            let _gate = self.publication.lock().unwrap_or_else(|e| e.into_inner());
            let t = self
                .cli_agents
                .steer_child(&self.store, owner, parent, q)
                .map_err(failed)?;
            self.publish_cli_queue(&task.child_session);
            t
        };
        if let Some(ticket) = ticket {
            let shared = self.clone();
            let parent = parent.to_owned();
            let tracking = self.external_tasks.token();
            self.spawn(async move {
                let cancel=ticket.cancellation();let revoked=ticket.revoked();
                let deadline=Instant::now()+Duration::from_secs(30*60);
                loop {
                    if tokio::task::block_in_place(||ticket.consumed(&shared.store)).unwrap_or(false) {break;}
                    if cancel.is_cancelled() || revoked.is_cancelled() || shared.stopping.is_cancelled() || Instant::now()>=deadline {
                        tokio::task::block_in_place(|| {
                            {let _gate=shared.publication.lock().unwrap_or_else(|e|e.into_inner());
                            if ticket.consumed(&shared.store).unwrap_or(true) {return;}
                            let _=shared.store.withdraw_cli_input(&ticket.queued);
                            let _=shared.store.tx(|tx| {if Store::task_by_session(tx,&task.child_session)?.is_some_and(|t|t.note.is_none()) {Store::set_task_note(tx,&task.child_session,"Steering was not delivered before this child stopped or finished.",now_ms())?;Store::bump(tx,&parent,now_ms())?;}Ok(())});
                            shared.publish_cli_queue(&task.child_session);}
                            shared.publish_subagents(&parent);
                        });break;
                    }
                    tokio::select!{_=cancel.cancelled()=>{},_=revoked.cancelled()=>{},_=shared.stopping.cancelled()=>{},_=tokio::time::sleep(Duration::from_millis(50))=>{}}
                }
                drop(ticket);drop(tracking);
            });
        }
        Ok(Reply::Subagent(self.subagent_row(parent, child)?))
    }
}
