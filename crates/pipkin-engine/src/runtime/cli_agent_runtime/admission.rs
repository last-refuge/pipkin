//! Engine-owned admission/preparation. No RPC caller receives or reconstructs a root lease.
use super::*;
use crate::{
    execution::claude_cli::{
        orchestration_policy::{Assets, HelperImage},
        orchestration_protocol::Session,
    },
    runtime::{cli_agent_owner::Turn, cli_bridge_channel::Server, cli_bridge_driver::Services},
    store::cli_bridge::BridgeSpec,
    tools::Tool,
};
use std::path::PathBuf;
use tokio::time::Instant;

pub(in crate::runtime) struct Start {
    pub ownership: Turn,
    pub request: String,
    pub generation: u64,
    pub operation: String,
    pub text: String,
    pub images: Vec<pipkin_engine_protocol::ImageInput>,
    pub system: String,
    pub assets_parent: PathBuf,
    pub image: Arc<HelperImage>,
    pub tools: Vec<Arc<dyn Tool>>,
    pub services: Services,
    pub deadline: Instant,
    pub cancel: CancellationToken,
    #[cfg(test)]
    pub test_peer: Option<oneshot::Sender<crate::runtime::cli_bridge_channel::Launch>>,
    #[cfg(test)]
    pub test_preparing: Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>,
}
// Field order keeps resources/ownership ahead of tracker completion even before first poll.
struct Submission {
    start: Start,
    admitted: Option<oneshot::Sender<Receipt>>,
    _completion: TaskTrackerToken,
}
impl Shared {
    pub(in crate::runtime) fn submit_cli_agent_root(
        self: &Arc<Self>,
        start: Start,
    ) -> oneshot::Receiver<Result<Receipt, Error>> {
        self.submit_cli_agent_root_notified(start, None)
    }
    pub(in crate::runtime) fn submit_cli_agent_root_notified(
        self: &Arc<Self>,
        start: Start,
        admitted: Option<oneshot::Sender<Receipt>>,
    ) -> oneshot::Receiver<Result<Receipt, Error>> {
        let submission = Submission {
            start,
            admitted,
            _completion: self.external_tasks.token(),
        };
        let (send, receive) = oneshot::channel();
        let shared = self.clone();
        self.spawn(async move {
            let result = shared.admit_cli_agent_root(submission).await;
            let _ = send.send(result);
        });
        receive
    }
    async fn admit_cli_agent_root(
        self: Arc<Self>,
        submission: Submission,
    ) -> Result<Receipt, Error> {
        let mut start = submission.start;
        let desktop_admission = submission.admitted.is_some();
        let mut admitted = submission.admitted;
        if !start.ownership.belongs_to(&self.cli_agents)
            || start.text.len()
                > crate::execution::claude_cli::orchestration_policy::MAX_PROMPT_BYTES
            || start.request.len() > 1024
            || start.operation.len() > 1024
        {
            return Err(Error::Preparation);
        }
        let session = start.ownership.binding().create.session.clone();
        // Lost replies resolve from exact durable identity, before capacity, freshness,
        // cancellation or launch assets. A receipt can never mint a second lease.
        if let Some(receipt) = tokio::task::block_in_place(|| {
            self.store
                .lookup_cli_agent_turn(&crate::store::cli_agent_turns::Input {
                    mode: start.ownership.input_mode(),
                    session: &session,
                    request: &start.request,
                    generation: start.generation,
                    operation: &start.operation,
                    text: &start.text,
                    images: &start.images,
                    binding: start.ownership.binding().digest,
                })
        })
        .map_err(|_| Error::Storage)?
        {
            if let Some(send) = admitted.take() {
                let _ = send.send(receipt.clone());
            }
            return Ok(receipt);
        }
        if start.system.len() > 1024 * 1024
            || start.system.trim().is_empty()
            || start.system.contains('\0')
            || !start.assets_parent.is_absolute()
        {
            return Err(Error::Preparation);
        }
        start
            .ownership
            .validate_inputs(
                &start.system,
                &start.tools.iter().map(|t| t.spec()).collect::<Vec<_>>(),
            )
            .map_err(|_| Error::Preparation)?;
        if start.ownership.child_policy().is_some() && start.services.child_resources.is_none() {
            return Err(Error::Preparation);
        }
        let sess = self.session(&session).map_err(|_| Error::Preparation)?;
        let cancel = start.ownership.cancellation();
        let deadline = start
            .deadline
            .min(Instant::now() + std::time::Duration::from_secs(30 * 60));
        let lock = tokio::select! {biased;
            _=cancel.cancelled()=>return Err(Error::Cancelled),
            _=start.cancel.cancelled()=>return Err(Error::Cancelled),
            _=self.stopping.cancelled()=>return Err(Error::Cancelled),
            _=sess.removed.cancelled()=>return Err(Error::Cancelled),
            _=tokio::time::sleep_until(deadline)=>return Err(Error::Cancelled),
            lock=sess.lock.lock()=>lock,
        };
        if self.stopping.is_cancelled()
            || sess.removed.is_cancelled()
            || cancel.is_cancelled()
            || start.cancel.is_cancelled()
            || sess.failure().is_some()
            || sess.active_run().is_some()
        {
            return Err(Error::Cancelled);
        }
        // Capacity is reserved BEFORE committing input and retained through runtime cleanup.
        let permit = if start.ownership.input_mode() != pipkin_engine_protocol::InputMode::Prompt {
            // A completion already owns a connected continuation ticket. Wait without
            // committing input while other conversations use capacity. Stop/revocation
            // ends this wait; restart cannot reconstruct it from saved completion data.
            tokio::select! {biased;
                _=cancel.cancelled()=>return Err(Error::Cancelled),
                _=start.cancel.cancelled()=>return Err(Error::Cancelled),
                _=self.stopping.cancelled()=>return Err(Error::Cancelled),
                _=sess.removed.cancelled()=>return Err(Error::Cancelled),
                _=tokio::time::sleep_until(deadline)=>return Err(Error::Cancelled),
                permit=self.generations.clone().acquire_owned()=>permit.map_err(|_|Error::Preparation)?,
            }
        } else {
            self.generations
                .clone()
                .try_acquire_owned()
                .map_err(|_| Error::Preparation)?
        };
        let settings = self.settings_for(&session);
        let delivery = if start.ownership.input_mode() == pipkin_engine_protocol::InputMode::Steer {
            settings.steering_mode
        } else {
            settings.follow_up_mode
        };
        let (admission, lifetime, published) = tokio::task::block_in_place(|| {
            let _publication = self.publication.lock().unwrap_or_else(|e| e.into_inner());
            if cancel.is_cancelled()
                || start.cancel.is_cancelled()
                || self.stopping.is_cancelled()
                || sess.removed.is_cancelled()
                || Instant::now() >= deadline
            {
                return Err(Error::Cancelled);
            }
            let admission = start
                .ownership
                .admit_batched(
                    &self.store,
                    &start.request,
                    start.generation,
                    &start.operation,
                    &start.text,
                    &start.images,
                    delivery,
                )
                .map_err(|_| Error::Storage)?;
            if let Some((parent, revision)) = admission.parent_task_revision.as_ref() {
                match self.subagents_view(parent) {
                    Ok(subagents) => self.publish(Event::Session {
                        session: parent.clone(),
                        revision: *revision,
                        change: SessionChange::SubagentsChanged { subagents },
                    }),
                    Err(_) => self.publish(Event::ResyncRequired),
                }
            }
            if let Some((session, revision)) = admission.job_revision.as_ref() {
                self.publish_cli_job_view(session, *revision);
            }
            self.publish_cli_queue(&session);
            self.reconcile_cli_goal_usage(&session, admission.receipt.scope.run);
            let (lifetime, published) = if let Some(root) = admission.lease.as_ref() {
                let run = root.receipt().scope.run;
                *sess.active.lock().unwrap_or_else(|e| e.into_inner()) = Some(Active {
                    run,
                    cancel: cancel.clone(),
                });
                let lifetime = Lifetime {
                    sess: sess.clone(),
                    run,
                    cancel: cancel.clone(),
                    settled: false,
                    _permit: permit,
                };
                let published = if let Ok(update) = self.cli_agent_initial(root) {
                    sess.changed.notify_waiters();
                    for change in update.changes {
                        self.publish(Event::Session {
                            session: session.clone(),
                            revision: update.revision,
                            change,
                        });
                    }
                    true
                } else {
                    false
                };
                (Some(lifetime), published)
            } else {
                (None, true)
            };
            Ok((admission, lifetime, published))
        })?;
        drop(lock);
        if desktop_admission {
            start.cancel = CancellationToken::new();
        }
        if let Some(send) = admitted.take() {
            let _ = send.send(admission.receipt.clone());
        }
        let Some(root) = admission.lease else {
            return Ok(admission.receipt);
        }; // Lookup, never launch.
        let lifetime = lifetime.expect("admitted root has its original SessionRt lifetime");
        #[cfg(test)]
        if let Some((ready, proceed)) = start.test_preparing {
            let _ = ready.send(());
            let _ = proceed.await;
        }
        // From this point every normal failure retires the original admission. Never select
        // cancellation against bind/asset writing: wait for their owned IO to finish first.
        let prepared = async {
            if !published {
                return Err(Error::Storage);
            }
            if cancel.is_cancelled()
                || start.cancel.is_cancelled()
                || self.stopping.is_cancelled()
                || Instant::now() >= deadline
            {
                return Err(Error::Cancelled);
            }
            if start.services.child_resources.is_none() {
                let resources = tokio::task::block_in_place(|| {
                    self.capture_cli_child_resources(
                        &session,
                        &start.system,
                        start.tools.clone(),
                        start.image.clone(),
                        start.services.shell.clone(),
                    )
                })?;
                start.services.child_resources = Some(resources);
            }
            let catalog = Session::new(
                root.receipt().scope.clone(),
                &start.tools.iter().map(|t| t.spec()).collect::<Vec<_>>(),
            )
            .map_err(|_| Error::Preparation)?
            .catalog_hash();
            let server = Server::bind_native(
                self.store.clone(),
                BridgeSpec {
                    id: crate::util::uuid_v4(),
                    scope: root.receipt().scope.clone(),
                    owner: start.ownership.owner_key().into(),
                    binding_hash: start.ownership.binding().digest,
                    capability_hash: [0; 32],
                },
                catalog,
                deadline,
            )
            .await
            .map_err(|_| Error::Channel)?;
            let native = server.native_launch().map_err(|_| Error::Preparation)?;
            let parent = start.assets_parent;
            let image = start.image;
            let system = start.system;
            let assets =
                tokio::task::spawn_blocking(move || Assets::new(&parent, image, native, &system))
                    .await
                    .map_err(|_| Error::Preparation)?
                    .map_err(|_| Error::Preparation)?;
            #[cfg(test)]
            if let Some(peer) = start.test_peer {
                let _ = peer.send(server.launch());
            }
            Ok((server, assets))
        }
        .await;
        let (server, assets) = match prepared {
            Ok(resources) => resources,
            Err(error) => {
                cancel.cancel();
                let store = self.store.clone();
                let retired = tokio::task::spawn_blocking(move || {
                    store.abandon_cli_agent_turn(&root, now_ms())
                })
                .await;
                self.finish_cli_agent_root(lifetime, start.request, false)
                    .await;
                if !matches!(retired, Ok(Ok(()))) {
                    self.storage_failure(&sess, "Native CLI preparation retirement failed");
                    return Err(Error::Storage);
                }
                return Err(error);
            }
        };
        let (publication, _unused) = tokio::sync::mpsc::channel(1);
        let launch = Launch {
            ownership: start.ownership,
            root,
            server,
            assets,
            tools: start.tools,
            services: start.services,
            cancel: start.cancel,
            publication,
        };
        self.run_cli_agent_root(Registered {
            launch,
            lifetime: Some(lifetime),
            // The outer Submission retains tracking through this call AND its own cleanup.
            _completion: None,
        })
        .await
    }
}
