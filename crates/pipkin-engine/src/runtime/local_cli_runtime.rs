//! Separately opted-in CLI modes. Metadata is never reused as a turn preflight.
use std::sync::Arc;

use pipkin_engine_protocol::{
    ErrorCode, ExecutionBackend, ImageInput, ProtoError, Reply, SessionSummary,
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::runtime::Shared;
use crate::{
    execution::claude_cli::{policy, preflight},
    store::SessionRow,
};

fn unavailable() -> ProtoError {
    ProtoError::new(
        ErrorCode::Unsupported,
        "Claude Code execution is disabled or its connection is unavailable. Reconnect the configured provider; changed saved identities require a new conversation.",
    )
}
struct CancelOnDrop(CancellationToken);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl Shared {
    pub(super) fn local_cli_execution_enabled(
        &self,
        mode: pipkin_engine_protocol::LocalCliMode,
    ) -> bool {
        use pipkin_engine_protocol::LocalCliMode;
        self.config.enable_local_claude_cli
            && match mode {
                LocalCliMode::Orchestration => {
                    cfg!(target_os = "linux") && self.config.enable_local_claude_cli_orchestration
                }
                LocalCliMode::Coding => self.config.enable_local_claude_cli_write,
                LocalCliMode::ChatOnly => true,
                LocalCliMode::RestrictedWrite => {
                    cfg!(target_os = "linux") && self.config.enable_local_claude_cli_write
                }
                LocalCliMode::Unsupported => false,
            }
    }
    /// `owner` is the dispatching connection's engine-assigned client id.
    pub async fn prompt_by_backend(
        self: &Arc<Self>,
        owner: u64,
        session: &str,
        request: &str,
        text: &str,
        images: &[ImageInput],
        model: Option<&str>,
    ) -> Result<Reply, ProtoError> {
        let row = self
            .store
            .session(session)?
            .ok_or_else(|| ProtoError::new(ErrorCode::NotFound, "Session does not exist"))?;
        if row.execution_backend == ExecutionBackend::ClaudeCliAgent {
            return self
                .prompt_cli_desktop(owner, session, request, text, images, model)
                .await;
        }
        if row.execution_backend == ExecutionBackend::NativeAgent {
            // Preserve the native route, including its existing handling of model.
            return self.prompt(session, request, text, images).await;
        }
        if row.execution_backend != ExecutionBackend::LocalClaudeCli
            || row.parent.is_some()
            || row.root.is_some()
        {
            return Err(unavailable());
        }
        if !images.is_empty() || model.is_some() {
            return Err(ProtoError::new(
                ErrorCode::Unsupported,
                "CLI execution does not support images or per-turn model overrides.",
            ));
        }
        crate::runtime::session::check_input(request, text, images)?;
        if text.len() > policy::MAX_PROMPT_BYTES || text.contains('\0') {
            return Err(ProtoError::new(
                ErrorCode::Invalid,
                "CLI text exceeds the input policy.",
            ));
        }
        // Receipt replay is a pure lookup, not authority to launch another turn:
        // it remains available to other connections and when execution is off.
        if let Some(reply) = self.external_duplicate(session, request, text)? {
            return Ok(reply);
        }
        // Disabling new execution must not hide an already committed receipt.
        if !self.config.enable_local_claude_cli {
            return Err(unavailable());
        }
        // Registration happens before the first await; Stop cannot miss a queued preflight.
        let completion = self.external_tasks.token();
        if self.stopping.is_cancelled() {
            return Err(unavailable());
        }
        let turn = self
            .local_cli_setups
            .begin_turn(owner, session)
            .map_err(|e| {
                if e.code == ErrorCode::Busy {
                    e
                } else {
                    unavailable()
                }
            })?;
        if !self.local_cli_binding_valid(&row, &turn.grant) {
            return Err(unavailable());
        }
        let caller = CancelOnDrop(turn.cancel.clone());
        let shared = self.clone();
        let request = request.to_owned();
        let text = text.to_owned();
        let (tx, rx) = oneshot::channel();
        // This owned task includes admission, so explicitly retain the RPC's cancellation
        // scope. The actor spawned after commit deliberately does not inherit it.
        let request_cancel = super::request_cancel::capture();
        self.spawn(super::request_cancel::scope_owned(
            request_cancel,
            async move {
                let _completion = completion;
                let check = preflight::check_for_policy(
                    turn.request(),
                    turn.cancel.clone(),
                    turn.grant.policy,
                );
                // Preflight retains its own bounded probes and the revocable turn token.
                let checked = check.await;
                let result = match checked {
                    Ok(prepared) => {
                        #[cfg(unix)]
                        if prepared.requires_permissions() {
                            let result = shared
                                .prompt_external_coding_authorized(&turn, &request, &text, prepared)
                                .await;
                            drop(turn);
                            let _ = tx.send(result);
                            return;
                        }
                        shared
                            .prompt_external_authorized(
                                &turn.session,
                                &request,
                                &text,
                                prepared,
                                Some(&turn),
                                #[cfg(test)]
                                None,
                            )
                            .await
                    }
                    Err(_) => Err(unavailable()),
                };
                // Owned registration and tracker survive caller cancellation through probe cleanup.
                drop(turn);
                let _ = tx.send(result);
            },
        ));
        let result = rx.await.map_err(|_| unavailable())?;
        drop(caller);
        result
    }

    pub async fn stop_by_backend(self: &Arc<Self>, session: &str) -> Result<Reply, ProtoError> {
        if self.cli_agents.owns_session(session) {
            return self.stop_cli_agent_root(session).await;
        }
        let row = self
            .store
            .session(session)?
            .ok_or_else(|| ProtoError::new(ErrorCode::NotFound, "Session does not exist"))?;
        if row.execution_backend == ExecutionBackend::NativeAgent {
            return self.stop(session).await;
        }
        if row.execution_backend != ExecutionBackend::LocalClaudeCli
            || row.parent.is_some()
            || row.root.is_some()
        {
            return Err(unavailable());
        }
        let pending = self.local_cli_setups.stop_pending(session);
        let sess = self.session(session)?;
        if !pending && sess.active_run().is_none() && !self.config.enable_local_claude_cli {
            return Err(unavailable());
        }
        self.stop_external_prompt(session).await
    }

    fn local_cli_binding_valid(
        &self,
        row: &SessionRow,
        grant: &crate::runtime::local_cli_setup::Grant,
    ) -> bool {
        row.execution_backend == ExecutionBackend::LocalClaudeCli
            && row.parent.is_none()
            && row.root.is_none()
            && row.blocked.is_none()
            && grant.binding.context_valid
            && self.local_cli_execution_enabled(super::local_cli_setup::binding_mode(
                &grant.binding.identity.launch_policy,
            ))
            && grant.binding.identity.launch_policy == grant.policy.id()
            && row.model.as_deref() == Some(grant.model.as_str())
            && self.store.cli_binding(&row.id).ok().flatten().as_ref() == Some(&grant.binding)
    }

    /// Immutable stored identity only. Safe under publication: no trust/session registry locks.
    pub(super) fn local_cli_summary_mode(
        &self,
        row: &SessionRow,
    ) -> Option<pipkin_engine_protocol::LocalCliMode> {
        if row.execution_backend == ExecutionBackend::ClaudeCliAgent {
            return Some(pipkin_engine_protocol::LocalCliMode::Orchestration);
        }
        (row.execution_backend == pipkin_engine_protocol::ExecutionBackend::LocalClaudeCli).then(
            || {
                self.store
                    .cli_binding(&row.id)
                    .ok()
                    .flatten()
                    .map(|binding| {
                        super::local_cli_setup::binding_mode(&binding.identity.launch_policy)
                    })
                    .unwrap_or(pipkin_engine_protocol::LocalCliMode::Unsupported)
            },
        )
    }

    /// Status only: no filesystem inspection, auth probe, subprocess, or new session lifetime.
    pub fn execution_summary(&self, row: &SessionRow) -> SessionSummary {
        let mut summary = row.summary();
        summary.local_cli_mode = self.local_cli_summary_mode(row);
        if row.execution_backend == ExecutionBackend::ClaudeCliAgent {
            summary.blocked = row
                .blocked
                .clone()
                .or_else(|| Some("Connect Claude orchestration to continue.".into()));
            return summary;
        }
        if summary
            .local_cli_mode
            .is_some_and(|mode| self.local_cli_execution_enabled(mode))
            && let Some(grant) = self.local_cli_setups.current_grant(&row.id)
            && self.local_cli_binding_valid(row, &grant)
            && !self
                .sessions
                .lock()
                .unwrap()
                .live
                .get(&row.id)
                .is_some_and(|s| s.rt.failure().is_some())
        {
            summary.blocked = None;
        }
        summary
    }
}

#[cfg(test)]
#[path = "local_cli_runtime_tests.rs"]
mod tests;
