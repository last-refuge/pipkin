//! Private SessionRt ownership from admission through cleanup of an approved CLI root.
//! Explicit setup selects this executor; no API inference. Committed root text and native tool
//! cards share the native commit/publication gate; only error retirement requests reconciliation.
#![allow(dead_code)]
use super::{
    Shared,
    cli_agent_root::{self, Error, Launch},
    session::{Active, SessionRt, Settled},
};
use crate::{
    store::{
        RunState, Store, StoreError,
        cli_agent_turns::{Receipt, projection::Update},
    },
    util::now_ms,
};
use pipkin_engine_protocol::{Event, ProtoError, Reply, SessionChange};
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, oneshot};
use tokio_util::{sync::CancellationToken, task::task_tracker::TaskTrackerToken};

// Declaration order matters even when the spawned future is never polled: release the
// original resources/turn before the tracker can report that this task is gone.
pub(super) mod admission;

struct Registered {
    launch: Launch,
    lifetime: Option<Lifetime>,
    _completion: Option<TaskTrackerToken>,
}
struct Lifetime {
    sess: Arc<SessionRt>,
    run: u64,
    cancel: CancellationToken,
    settled: bool,
    _permit: OwnedSemaphorePermit,
}
impl Drop for Lifetime {
    fn drop(&mut self) {
        self.cancel.cancel();
        let mut active = self.sess.active.lock().unwrap_or_else(|e| e.into_inner());
        if active.as_ref().is_some_and(|a| a.run == self.run) {
            *active = None;
            if !self.settled && !self.sess.removed.is_cancelled() {
                *self.sess.failed.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some("Owned CLI root ended without confirmed durable settlement".into());
            }
        }
        self.sess.changed.notify_waiters();
    }
}

impl Shared {
    /// Synchronous handoff: ownership moves into an engine task BEFORE returning a waiter.
    /// Dropping that waiter never drops the actor. The tracker covers preparation, process/
    /// native cleanup, SQL retirement and SessionRt release, including prelaunch refusals.
    #[cfg(test)] // Already-admitted fixture seam; production must use owned submit.
    pub(super) fn spawn_cli_agent_root(
        self: &Arc<Self>,
        launch: Launch,
    ) -> oneshot::Receiver<Result<Receipt, Error>> {
        let registered = Registered {
            launch,
            lifetime: None,
            _completion: Some(self.external_tasks.token()),
        };
        let (send, receive) = oneshot::channel();
        let shared = self.clone();
        self.spawn(async move {
            let result = shared.run_cli_agent_root(registered).await;
            let _ = send.send(result);
        });
        receive
    }

    async fn run_cli_agent_root(self: Arc<Self>, registered: Registered) -> Result<Receipt, Error> {
        let mut launch = registered.launch;
        let mut lifetime = registered.lifetime;
        let input_published = lifetime.is_some();
        // This check precedes all session/store mutation. A foreign registry cannot adopt
        // this engine's root merely by presenting matching durable scope data.
        if !launch.ownership.belongs_to(&self.cli_agents) || !launch.ownership.matches(&launch.root)
        {
            return Err(Error::Preparation);
        }
        let scope = launch.root.receipt().scope.clone();
        let request = launch.root.receipt().request.clone();
        let cancel = launch.ownership.cancellation();
        let deadline = launch.server.deadline();
        let children = launch.services.child_resources.take().or_else(|| {
            tokio::task::block_in_place(|| {
                self.capture_cli_child_resources(
                    &scope.session,
                    launch.assets.system_text(),
                    launch.tools.clone(),
                    launch.assets.image(),
                    launch.services.shell.clone(),
                )
            })
            .ok()
        });
        #[cfg(test)]
        let children = children.map(|r| {
            let mut r = (*r).clone();
            r.peers = launch.services.child_peers.clone().or(r.peers);
            r.job_start_pause = launch
                .services
                .job_start_pause
                .clone()
                .or(r.job_start_pause);
            Arc::new(r)
        });
        if children.is_none() {
            cancel.cancel();
        }
        // A private caller cannot create a second process/FS mutation capacity pool.
        launch.services.processes = self.processes.clone();
        launch.services.locks = self.locks.clone();
        launch.services.publication = None;
        // Engine-owned roots never inherit an API-backed or arbitrary caller host.
        launch.services.host = None;
        launch.services.host_factory = None;
        launch.services.settle = None;
        let sess = self.session(&scope.session);
        let mut attached = false;
        if let Ok(sess) = sess {
            let lock = tokio::select! { biased;
                _ = cancel.cancelled() => None,
                _ = launch.cancel.cancelled() => None,
                _ = self.stopping.cancelled() => None,
                _ = sess.removed.cancelled() => None,
                _ = tokio::time::sleep_until(deadline) => None,
                lock = sess.lock.lock() => Some(lock),
            };
            if lock.is_some()
                && !cancel.is_cancelled()
                && !launch.cancel.is_cancelled()
                && !self.stopping.is_cancelled()
                && !sess.removed.is_cancelled()
                && sess.failure().is_none()
                && sess.active_run() == lifetime.as_ref().map(|l| l.run)
                && tokio::task::block_in_place(|| self.store.cli_agent_launch_context(&launch.root))
                    .is_ok()
                && let Ok(permit) = if lifetime.is_some() {
                    Ok(None)
                } else {
                    self.generations.clone().try_acquire_owned().map(Some)
                }
            {
                if let Some(permit) = permit {
                    *sess.active.lock().unwrap_or_else(|e| e.into_inner()) = Some(Active {
                        run: scope.run,
                        cancel: cancel.clone(),
                    });
                    lifetime = Some(Lifetime {
                        sess: sess.clone(),
                        run: scope.run,
                        cancel: cancel.clone(),
                        settled: false,
                        _permit: permit,
                    });
                }
                attached = true;
                let current = sess.clone();
                let current_run = scope.run;
                let emitting = sess.clone();
                let shared = self.clone();
                let publisher = super::cli_bridge_driver::Publication::new(
                    self.publication.clone(),
                    scope.clone(),
                    move || {
                        !current.removed.is_cancelled()
                            && current.failure().is_none()
                            && current.active_run() == Some(current_run)
                    },
                    move |update| {
                        emitting.changed.notify_waiters();
                        for change in update.changes {
                            shared.publish(Event::Session {
                                session: update.scope.session.clone(),
                                revision: update.revision,
                                change,
                            });
                        }
                    },
                );
                // Only the legacy private already-admitted seam needs initial publication.
                // The owned builder committed/delivered its input under the same gate.
                if !input_published
                    && publisher
                        .commit(&scope, || {
                            Ok(((), Some(self.cli_agent_initial(&launch.root)?)))
                        })
                        .is_err()
                {
                    cancel.cancel();
                }
                match launch.ownership.services(&launch.root) {
                    Ok(authority) if children.is_some() => {
                        let settling = self.clone();
                        let delivery_authority = authority.clone();
                        let steering = self.settings_for(&scope.session).steering_mode;
                        launch.services.settle = Some(Arc::new(move |lease, claim, result| {
                            delivery_authority.settle_with_steering(
                                &settling.store,
                                lease,
                                claim,
                                result,
                                steering,
                            )
                        }));
                        launch.services.host_factory = Some(super::cli_agent_host::factory(
                            self.clone(),
                            authority,
                            publisher.clone(),
                            children.clone().expect("checked resources"),
                        ))
                    }
                    _ => cancel.cancel(),
                }
                launch.services.publication = Some(publisher);
                sess.changed.notify_waiters();
            }
        }
        if !attached {
            cancel.cancel();
        }
        let (publication, mut updates) = tokio::sync::mpsc::channel(16);
        launch.publication = publication;
        let drain = async {
            while let Some(update) = updates.recv().await {
                if update.scope != scope {
                    cancel.cancel();
                }
                // Attached roots publish directly under the shared commit gate. This lane
                // remains only for standalone coordinator callers, not deferred live deltas.
                if !update.changes.is_empty() {
                    cancel.cancel();
                }
            }
        };
        let root = cli_agent_root::run(self.store.clone(), launch);
        let (result, ()) = tokio::join!(root, drain);
        if let Some(lifetime) = lifetime {
            self.finish_cli_agent_root(lifetime, request, result.is_ok())
                .await;
        } else {
            self.publish(Event::ResyncRequired);
        }
        result
    }

    fn cli_agent_initial(
        &self,
        lease: &crate::store::cli_agent_turns::Lease,
    ) -> Result<Update, StoreError> {
        let scope = lease.receipt().scope.clone();
        let row = self
            .store
            .session(&scope.session)?
            .ok_or(StoreError::ExecutionConflict)?;
        let entry = self
            .store
            .entry(&scope.session, lease.receipt().user_entry)?
            .ok_or(StoreError::ExecutionConflict)?;
        let run = self
            .store
            .run(scope.run)?
            .ok_or(StoreError::ExecutionConflict)?;
        let mut changes = vec![
            SessionChange::EntryAppended { entry },
            SessionChange::RunChanged { run: run.view() },
        ];
        for entry in self.store.cli_batch_entries(scope.run)? {
            changes.insert(changes.len() - 1, SessionChange::EntryAppended { entry });
        }
        if let Some(goal) = self
            .goal_view(&scope.session)
            .map_err(|_| StoreError::ExecutionConflict)?
            && goal.last_request.as_deref() == Some(lease.receipt().request.as_str())
            && crate::goals::is_turn_of(&lease.receipt().request, goal.id)
        {
            changes.push(SessionChange::GoalChanged {
                goal: Some(Box::new(goal)),
            });
        }
        Ok(Update {
            scope,
            revision: row.revision,
            changes,
        })
    }

    /// Caller holds publication; accounting never mints inference authority.
    pub(in crate::runtime) fn reconcile_cli_goal_usage(&self, session: &str, run: u64) {
        match self
            .store
            .tx(|c| Store::cli_goal_reconcile_run_in(c, session, run, crate::util::now_ms()))
        {
            Ok(updates) => {
                for (session, goal, revision) in updates {
                    self.publish(Event::Session {
                        session,
                        revision,
                        change: SessionChange::GoalChanged {
                            goal: Some(Box::new(goal)),
                        },
                    });
                }
            }
            Err(_) => self.publish(Event::ResyncRequired),
        }
    }
    async fn finish_cli_agent_root(&self, mut lifetime: Lifetime, request: String, success: bool) {
        let sess = lifetime.sess.clone();
        let session = sess.id.clone();
        let run = lifetime.run;
        let _lock = sess.lock.lock().await;
        let _publication = self.publication.lock().unwrap_or_else(|e| e.into_inner());
        self.reconcile_cli_goal_usage(&session, run);
        let revision = self
            .store
            .session(&session)
            .ok()
            .flatten()
            .map(|r| r.revision);
        let settled = self.store.run(run).ok().flatten().filter(|r| {
            r.session == session
                && r.request.as_deref() == Some(request.as_str())
                && r.state == RunState::Settled
                && r.outcome.is_some()
        });
        lifetime.settled = settled.is_some();
        drop(lifetime);
        if !sess.removed.is_cancelled() {
            // Already under publication: use the CLI store view directly, without
            // acquiring the session registry or publishing the generic volatile revision 0.
            if let Ok(context) = self.store.cli_context_usage(&session) {
                self.publish(Event::Session {
                    session: session.clone(),
                    revision: context.revision,
                    change: SessionChange::ContextChanged { context },
                });
            }
            self.publish_cli_queue(&session);
            if let Some(outcome) = settled.and_then(|r| r.outcome) {
                let _ = sess.settled.send(Settled {
                    run,
                    request: Some(request.clone()),
                    outcome: outcome.clone(),
                });
                if success && let Some(revision) = revision {
                    self.publish(Event::Session {
                        session: session.clone(),
                        revision,
                        change: SessionChange::RunChanged { run: None },
                    });
                    self.publish(Event::Session {
                        session: session.clone(),
                        revision,
                        change: SessionChange::RunSettled {
                            run,
                            request: Some(request),
                            outcome,
                        },
                    });
                } else {
                    // Error retirement changes existing user/display entries. Reconcile
                    // once; never rebuild the transcript on every streamed token.
                    self.publish(Event::ResyncRequired);
                }
            } else {
                self.publish(Event::ResyncRequired);
            }
        }
    }

    pub(super) async fn stop_cli_agent_root(
        self: &Arc<Self>,
        session: &str,
    ) -> Result<Reply, ProtoError> {
        // Signal before waiting for session/publication/storage locks. No native recovery,
        // goal advancement or queued prompt execution is borrowed from the API backend.
        self.cli_agents.stop(session);
        let sess = self.session(session)?;
        let _lock = sess.lock.lock().await;
        let active = sess
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|a| {
                a.cancel.cancel();
                a.run
            });
        let run=self.commit(&sess,|tx| {
            let Some(run)=crate::store::Store::open_run_in(tx,session)? else {return Ok((None,vec![]));};
            if active!=Some(run.id) {return Ok((None,vec![]));}
            tx.execute("UPDATE runs SET state='stopping',stop_requested_at=COALESCE(stop_requested_at,?2) WHERE id=?1",
                rusqlite::params![run.id as i64,now_ms()])?;
            let mut changes=vec![SessionChange::RunChanged {run:crate::store::Store::run_in(tx,run.id)?.and_then(|r|r.view())}];
            if let Some(goal)=crate::store::Store::goal_current(tx,session)?
                && goal.state==pipkin_engine_protocol::GoalState::Active
                && goal.last_request.is_some() && goal.last_request==run.request {
                crate::store::Store::goal_set_state(tx,session,goal.id,pipkin_engine_protocol::GoalState::Paused,Some(crate::goals::REASON_STOP),now_ms())?;
                changes.push(SessionChange::GoalChanged {goal:crate::store::Store::goal_current(tx,session)?.map(Box::new)});
            }
            Ok((Some(run.id),changes))
        })?;
        Ok(Reply::Stopping { run })
    }
}
