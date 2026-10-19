//! Background completion continuation uses the original connected approval and the owned
//! CLI admission/runtime. No generic enqueue/recovery/API model path is called.
use super::*;
use crate::runtime::{
    cli_agent_owner::{Error as OwnerError, WakeTicket},
    cli_agent_runtime::admission::Start,
    cli_bridge_driver::Services,
};
use crate::store::cli_agent_turns::WakeTarget;
use pipkin_engine_protocol::Event;
use std::time::Duration;
impl Host {
    pub(super) async fn deliver_wake(&self, mut ticket: WakeTicket, target: WakeTarget) {
        let epoch = ticket.cancellation();
        let revoked = ticket.revoked();
        let mut child_run = None;
        let result = async {
            let mut delivery_deadline = None;
            let (ownership, request, text) = loop {
                if epoch.is_cancelled()
                    || revoked.is_cancelled()
                    || self.shared.stopping.is_cancelled()
                    || delivery_deadline.is_some_and(|deadline| Instant::now() >= deadline)
                {
                    return Err("CLI continuation cancelled before admission".into());
                }
                let prepared = tokio::task::block_in_place(|| {
                    // Capture generated text and its completion digest under the same
                    // publication gate. A busy parent does not freeze stale result text;
                    // admission rechecks this final snapshot after capacity/preflight waits.
                    let _gate = self
                        .shared
                        .publication
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    let text = match &target {
                        WakeTarget::Child(child) => {
                            let task = self
                                .shared
                                .store
                                .read(|c| Store::task_by_session(c, child))
                                .map_err(|e| e.to_string())?
                                .ok_or("CLI child disappeared")?;
                            if !task.finished() {
                                return Ok(None);
                            }
                            self.shared.subagent_wake_text(&task)
                        }
                        WakeTarget::Job(id) => {
                            let job = self
                                .shared
                                .store
                                .read(|c| Store::job(c, &self.authority.scope().session, *id))
                                .map_err(|e| e.to_string())?
                                .ok_or("CLI job disappeared")?;
                            if job.active() {
                                return Ok(None);
                            }
                            self.shared.job_wake_text(&job)
                        }
                    };
                    delivery_deadline
                        .get_or_insert_with(|| Instant::now() + Duration::from_secs(30 * 60));
                    match ticket.try_begin(&self.shared.store, target.clone(), text.clone()) {
                        Ok((ownership, request)) => Ok(Some((ownership, request, text))),
                        Err(OwnerError::Busy) => Ok(None),
                        Err(error) => Err(format!("CLI continuation refused: {error:?}")),
                    }
                })?;
                if let Some(prepared) = prepared {
                    break prepared;
                }
                tokio::select! {
                    _=epoch.cancelled()=>{},
                    _=revoked.cancelled()=>{},
                    _=self.shared.stopping.cancelled()=>{},
                    _=tokio::time::sleep(Duration::from_millis(100))=>{},
                }
            };
            #[cfg(test)]
            let (send, receive) = tokio::sync::oneshot::channel();
            let mut system = self.children.root_system.to_string();
            let mut tools = self.children.tools.clone();
            if let Some(policy) = ownership.child_policy() {
                system = policy.system().into();
                tools.retain(|t| policy.record().allows(&t.spec().name));
                if let Some(context) = policy.context_tool() {
                    tools.push(context);
                }
                child_run = Some((ownership.binding().create.session.clone(), request.clone()));
            }
            let start = Start {
                images: vec![],
                ownership,
                request,
                generation: ticket.generation(),
                operation: crate::util::uuid_v4(),
                text,
                system,
                assets_parent: self.children.image.assets_parent().to_owned(),
                image: self.children.image.clone(),
                tools,
                services: Services {
                    child_resources: Some(self.children.clone()),
                    shell: self.children.shell.clone(),
                    #[cfg(test)]
                    child_peers: self.children.peers.clone(),
                    ..Services::default()
                },
                deadline: delivery_deadline.ok_or("Missing completion delivery deadline")?,
                cancel: epoch,
                #[cfg(test)]
                test_peer: Some(send),
                #[cfg(test)]
                test_preparing: None,
            };
            let completion = self.shared.submit_cli_agent_root(start);
            #[cfg(test)]
            if let Some(peers) = self.children.peers.clone() {
                self.shared.spawn(async move {
                    if let Ok(peer) = receive.await {
                        let _ = peers.send(peer);
                    }
                });
            }
            completion
                .await
                .map_err(|_| "CLI continuation reply lost; do not replay".to_string())?
                .map_err(|e| format!("CLI continuation did not finish cleanly: {e:?}"))?;
            Ok::<_, String>(())
        }
        .await;
        if let Err(error) = result {
            log::warn!("{error}");
        }
        // Admission changes wake -> woken atomically with the actual input/run. If no
        // admission happened, only suppress notification; never retry the ticket/effect.
        let settled = tokio::task::block_in_place(|| {
            let _gate = self
                .shared
                .publication
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            // Settle only this admitted child wake after its process/bridge cleanup.
            // A failed pre-admission attempt cannot overwrite an earlier child result.
            if let Some((child, request)) = &child_run {
                let settled = self.shared.store.tx(|c| {
                    let current: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM cli_agent_turns WHERE session=?1 AND request_key=?2 AND run=(SELECT MAX(run) FROM cli_agent_turns WHERE session=?1))",rusqlite::params![child,request],|r|r.get(0))?;
                    if !current { return Ok(None); }
                    Store::settle_cli_native_child_in(c,child,now_ms()).map(Some)
                })?;
                if let Some((_, _, parent, revision)) = settled {
                    match self.shared.subagents_view(&parent) {
                        Ok(subagents) => self.shared.publish(Event::Session {
                            session: parent,
                            revision,
                            change: SessionChange::SubagentsChanged { subagents },
                        }),
                        Err(_) => self.shared.publish(Event::ResyncRequired),
                    }
                }
            }
            let session = &self.authority.scope().session;
            let revision=self.shared.store.tx(|c| {
                let changed=match &target {
                    WakeTarget::Child(child)=>c.execute("UPDATE subagent_tasks SET delivery='suppressed',updated_at=?2 WHERE child_session=?1 AND delivery='wake' AND EXISTS(SELECT 1 FROM cli_agent_turns WHERE session=?1 AND request_key=?3 AND run=(SELECT MAX(run) FROM cli_agent_turns WHERE session=?1))",rusqlite::params![child,now_ms(),format!("child:{}",self.call)])?,
                    WakeTarget::Job(id)=>c.execute("UPDATE jobs SET delivery='suppressed',updated_at=?3 WHERE session=?1 AND id=?2 AND delivery='pending'",rusqlite::params![session,*id as i64,now_ms()])?,
                };
                if changed==0 {return Ok(None);}
                Store::bump(c,session,now_ms()).map(Some)
            })?;
            if let Some(revision) = revision {
                match target {
                    WakeTarget::Child(_) => self.child_view(revision),
                    WakeTarget::Job(_) => self.shared.publish_cli_job_view(session, revision),
                }
            }
            Ok::<_, StoreError>(())
        });
        if settled.is_err() {
            self.shared
                .publish(pipkin_engine_protocol::Event::ResyncRequired);
        }
    }
}
