//! CLI job admission uses the existing supervisor, never an inference runner. The startup
//! actor survives a dropped tool waiter and owns cleanup until the process is supervised.
use super::*;
use crate::runtime::jobs::JobSpec;
use pipkin_engine_protocol::{ErrorCode, JobView, ProtoError};

#[cfg(test)]
pub(in crate::runtime) type StartPause = std::sync::Mutex<
    Option<(
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
    )>,
>;

pub(in crate::runtime) struct JobAuthority(Host);
impl JobAuthority {
    #[cfg(test)]
    pub(in crate::runtime) async fn before_spawn(&self) {
        let pause = self
            .0
            .children
            .job_start_pause
            .as_ref()
            .and_then(|p| p.lock().unwrap().take());
        if let Some((entered, resume)) = pause {
            let _ = entered.send(());
            tokio::select! {biased;
                _=self.0.cancel.cancelled()=>{},
                _=self.0.shared.stopping.cancelled()=>{},
                _=tokio::time::sleep_until(self.0.deadline)=>{},
                _=resume=>{},
            }
        }
    }
    pub(in crate::runtime) fn session(&self) -> &str {
        &self.0.authority.scope().session
    }
    pub(in crate::runtime) fn call(&self) -> &str {
        &self.0.call
    }
    pub(in crate::runtime) fn run(&self) -> u64 {
        self.0.authority.scope().run
    }
    pub(in crate::runtime) fn check(&self) -> Result<(), ProtoError> {
        if self.0.shared.stopping.is_cancelled() {
            return Err(ProtoError::new(ErrorCode::Invalid, "Engine stopping"));
        }
        self.0
            .named("job_start", Some(self.call()))
            .and_then(|()| self.0.check())
            .map_err(|e| ProtoError::new(ErrorCode::Invalid, e))
    }
    pub(in crate::runtime) fn commit<T>(
        &self,
        f: impl FnOnce(&rusqlite::Transaction) -> crate::store::Result<(T, Vec<SessionChange>)>,
    ) -> Result<T, ProtoError> {
        self.0
            .transaction(|tx| {
                let (result, changes) = f(tx)?;
                let scope = self.0.authority.scope();
                let revision = Store::bump(tx, &scope.session, now_ms())?;
                Ok((
                    result,
                    Some(Update {
                        scope: scope.clone(),
                        revision,
                        changes,
                    }),
                ))
            })
            .map_err(|e| ProtoError::new(ErrorCode::Invalid, e))
    }
    pub(in crate::runtime) fn spawn(
        &self,
        command: &mut tokio::process::Command,
    ) -> Result<std::io::Result<tokio::process::Child>, ProtoError> {
        self.0
            .publication
            .commit(self.0.authority.scope(), || {
                self.0.authority.authorize(|| {
                    self.0.shared.store.read(|c| self.0.live(c))?;
                    if self.0.shared.stopping.is_cancelled() {
                        return Err(StoreError::ExecutionConflict);
                    }
                    // The job's intent has already committed. No await or transferable permission
                    // between the pending-call check and starting this owned process.
                    Ok((command.spawn(), None))
                })
            })
            .map_err(|e| ProtoError::new(ErrorCode::Invalid, e.to_string()))
    }
}
impl Host {
    /// Startup receipt must reach the durable native call before this job can detach.
    /// Read receipt before cancellation: a confirmed handoff survives normal parent teardown.
    async fn wait_job_handoff(&self) -> bool {
        loop {
            let state = tokio::task::block_in_place(|| {
                self.shared.store.cli_job_handoff(
                    &self.authority.scope().session,
                    &self.call,
                    self.authority.scope().run,
                )
            });
            match state {
                Ok(Some(confirmed)) => return confirmed,
                Err(_) => return false,
                Ok(None) => {}
            }
            if self.cancel.is_cancelled()
                || self.shared.stopping.is_cancelled()
                || Instant::now() >= self.deadline
            {
                // Recheck after cancellation to avoid racing the native receipt commit.
                return tokio::task::block_in_place(|| {
                    self.shared.store.cli_job_handoff(
                        &self.authority.scope().session,
                        &self.call,
                        self.authority.scope().run,
                    )
                })
                .ok()
                .flatten()
                    == Some(true);
            }
            tokio::select! {biased;
                _=self.cancel.cancelled()=>{},_=self.shared.stopping.cancelled()=>{},
                _=tokio::time::sleep_until(self.deadline)=>{},
                _=tokio::time::sleep(std::time::Duration::from_millis(25))=>{},
            }
        }
    }
    pub(super) fn start_job(
        &self,
        spec: JobSpec,
    ) -> tokio::sync::oneshot::Receiver<Result<JobView, String>> {
        let authority = JobAuthority(self.clone());
        let tracking = self.shared.external_tasks.token();
        let (send, receive) = tokio::sync::oneshot::channel();
        let shared = self.shared.clone();
        self.shared.spawn(async move {
            let wake = if spec.wake {
                match tokio::task::block_in_place(|| authority.0.authority.wake_ticket(&shared.store,&authority.0.lease,authority.call(),"job_start")) {
                    Ok(ticket)=>Some(ticket),
                    Err(error)=>{ let _=send.send(Err(format!("CLI job wake ownership refused: {error:?}"))); return; },
                }
            } else {None};
            let result=shared.job_start_cli(&authority,spec).await;
            let result=match result {
                Ok(view) if authority.check().is_err() => {
                    let _=shared.job_cancel(authority.session(),view.id).await;
                    Err("CLI job startup lost its live scope; inspect the retained job, do not replay".into())
                }
                other=>other.map_err(|e|e.message),
            };
            // Send the startup receipt before waiting for completion. A dropped waiter
            // leaves this tracked actor responsible for suppression/cleanup, never replay.
            let id=result.as_ref().ok().map(|view|view.id);
            let _=send.send(result);
            // Even startup failures can leave a process intent that needs cleanup.
            let id=id.or_else(||shared.store.read(|c| {
                use rusqlite::OptionalExtension;
                Ok(c.query_row("SELECT id FROM jobs WHERE session=?1 AND call_id=?2",rusqlite::params![authority.session(),authority.call()],|r|r.get::<_,u64>(0)).optional()?)
            }).ok().flatten());
            if let Some(id)=id {
                if !authority.0.wait_job_handoff().await {
                    let _=shared.job_cancel(authority.session(),id).await;
                    if let Some(wake)=&wake {wake.cancellation().cancel();}
                }
                if let Some(wake)=wake {authority.0.deliver_wake(wake,crate::store::cli_agent_turns::WakeTarget::Job(id)).await;}
            }
            drop(authority);
            drop(tracking);
        });
        receive
    }
}

impl Shared {
    /// Caller holds the publication gate; the revision is from the delivery transaction.
    pub(in crate::runtime) fn publish_cli_job_view(&self, session: &str, revision: u64) {
        match self.jobs_view(session) {
            Ok(jobs) => self.publish(pipkin_engine_protocol::Event::Session {
                session: session.into(),
                revision,
                change: SessionChange::JobsChanged { jobs },
            }),
            Err(_) => self.publish(pipkin_engine_protocol::Event::ResyncRequired),
        }
    }
}
