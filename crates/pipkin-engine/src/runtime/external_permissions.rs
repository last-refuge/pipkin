//! Receipt-backed controller, used by the internal actor and owner-scoped RPC facade.
//! The actor reserves this INSTEAD OF its ordinary Running CAS, retains it
//! through settlement, and seals authority before signals. All decisions/fences serialize
//! here; delivery must be bounded, synchronous, nonblocking and non-reentrant.
//! SQLite commits use Shared publication, never native tool tables. Slow persistence
//! cannot authorize late: monotonic lease/request checks run again AFTER commit.
#![allow(dead_code)] // Public coding launch/UI and live qualification remain gated.

use super::{SessionRt, Shared};
use crate::{
    execution::claude_cli::{
        permission::{self, CliScope, Decision, GateLimits, PermissionGate, RunScope, Ticket},
        permission_wire::{Binding, RequestFrame},
    },
    store::{
        EXTERNAL_PERMISSION_POLICY, ExternalPermissionDecision, ExternalPermissionKey,
        ExternalPermissionRequest, ExternalPermissionState, ExternalPromptReceipt, Store,
        StoreError,
    },
    util::now_ms,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

const REVIEW_LIFETIME: Duration = Duration::from_secs(110);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Error {
    Scope,
    Unavailable,
    Request,
    Persistence,
    Decision,
}

/// Engine-captured owner and tokens, never fields from helper/user JSON.
pub(super) struct Lease {
    pub owner: u64,
    pub expires: Instant,
    pub revoked: CancellationToken,
    pub run_cancel: CancellationToken,
}

/// Complete inert display for this connection only. Intentionally no Debug/Serialize.
#[derive(Clone)]
pub(super) struct Review {
    pub ticket: Ticket,
    pub arguments: String,
    pub expires_at_ms: i64,
    pub expires: Instant,
}
struct Pending {
    review: Review,
    request: ExternalPermissionRequest,
    expires: Instant,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Seal {
    pub revocation_confirmed: bool,
    /// A clean CLI exit cannot hide unresolved requests that cleanup just revoked.
    pub unresolved: bool,
}
struct CancelOnUnwind<'a> {
    token: &'a CancellationToken,
    armed: bool,
}
impl Drop for CancelOnUnwind<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.token.cancel();
        }
    }
}

struct State {
    gate: PermissionGate,
    pending: BTreeMap<[u8; 32], Pending>,
    seal: Option<Seal>,
    endpoint_claimed: bool,
}

pub(super) struct Broker {
    shared: Weak<Shared>,
    // Retain demand only while authority is open. Read-only Control handles must
    // not keep an otherwise-idle removed session alive indefinitely after seal.
    session: Mutex<Option<Arc<SessionRt>>>,
    removed: CancellationToken,
    scope: RunScope,
    prompt: u64,
    project: std::path::PathBuf,
    run_nonce: Binding,
    lease: Lease,
    origin: Instant,
    state: Mutex<State>,
    #[cfg(test)]
    after_commit: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Broker {
    /// Durable exclusive reservation is the existing strict LaunchIntent -> Running
    /// CAS. A second controller, including after restart, cannot reserve this run.
    /// Caller already captured executable/project trust; this does not grant it.
    pub fn reserve(
        shared: &Arc<Shared>,
        receipt: &ExternalPromptReceipt,
        lease: Lease,
    ) -> Result<Arc<Self>, Error> {
        if lease.owner == 0
            || lease.expires <= Instant::now()
            || lease.revoked.is_cancelled()
            || lease.run_cancel.is_cancelled()
            || shared.stopping.is_cancelled()
        {
            return Err(Error::Unavailable);
        }
        let session = shared
            .session(&receipt.key.session)
            .map_err(|_| Error::Scope)?;
        let binding = shared
            .store
            .cli_binding(&receipt.key.session)
            .map_err(|_| Error::Persistence)?
            .ok_or(Error::Scope)?;
        if binding.generation != receipt.key.generation
            || binding.identity.launch_policy != EXTERNAL_PERMISSION_POLICY
        {
            return Err(Error::Scope);
        }
        let cli = CliScope::new(
            &binding.identity.cli_session_id,
            &binding.identity.project_path,
        )
        .map_err(|_| Error::Scope)?;
        let scope = RunScope {
            epoch: shared.epoch,
            owner: lease.owner,
            key: receipt.key.clone(),
            run_id: receipt.id.to_string(),
            cli,
        };
        let run_nonce = Binding::fresh().map_err(|_| Error::Unavailable)?;
        let gate = PermissionGate::new(scope.clone(), GateLimits::default(), 0, |_| {
            shared
                .external_commit(&session, |tx| {
                    let current = Store::external_prompt_for_run_in(tx, &receipt.key)?
                        .ok_or(StoreError::ExecutionConflict)?;
                    if current.id != receipt.id {
                        return Err(StoreError::ExecutionConflict);
                    }
                    Store::begin_external_permission_broker_in(
                        tx,
                        &ExternalPermissionKey {
                            engine_epoch: scope.epoch,
                            owner: scope.owner.to_string(),
                            run: scope.key.clone(),
                            prompt: receipt.id,
                            invocation_nonce: run_nonce.to_hex(),
                        },
                        now_ms(),
                    )
                })
                .map(|_| ())
        })
        .map_err(|_| Error::Persistence)?;
        let broker = Arc::new(Self {
            shared: Arc::downgrade(shared),
            removed: session.removed.clone(),
            session: Mutex::new(Some(session)),
            scope,
            prompt: receipt.id,
            project: binding.identity.project_path.into(),
            run_nonce,
            lease,
            origin: Instant::now(),
            state: Mutex::new(State {
                gate,
                pending: BTreeMap::new(),
                seal: None,
                endpoint_claimed: false,
            }),
            #[cfg(test)]
            after_commit: Mutex::new(None),
        });
        if !broker.live() {
            broker.fence();
            return Err(Error::Unavailable);
        }
        Ok(broker)
    }

    /// One listener per reserved run. Failed binds burn this claim; neither a
    /// retry nor a second transport can reconstruct an execution capability.
    pub fn claim_endpoint(&self) -> Result<(), Error> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !self.live() || state.gate.is_fenced() {
            self.fence_locked(&mut state);
            return Err(Error::Unavailable);
        }
        if state.endpoint_claimed {
            return Err(Error::Unavailable);
        }
        state.endpoint_claimed = true;
        Ok(())
    }

    #[cfg(test)]
    pub fn cancel_after_commit_for_test(&self) {
        let token = self.lease.run_cancel.clone();
        *self.after_commit.lock().unwrap() = Some(Box::new(move || token.cancel()));
    }
    pub fn scope(&self) -> &RunScope {
        &self.scope
    }
    pub fn prompt(&self) -> u64 {
        self.prompt
    }
    /// Nonblocking retirement before the registry unlocks; durable seal follows.
    pub fn cancel(&self) {
        self.lease.run_cancel.cancel();
    }

    pub fn permits_launch(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.live() && !state.gate.is_fenced()
    }

    /// Normal completion needs a live lease after durable retirement too. A
    /// previous failure fence cannot be relabelled clean by later cleanup.
    pub fn completion_seal(&self) -> (Seal, bool) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        // Supervisor cleanup may already have sealed authority without cancelling
        // the run. A failure fence additionally cancels it and cannot become clean.
        let seal = self.seal_locked(&mut state);
        let eligible = self.live() && seal.revocation_confirmed && !seal.unresolved;
        (seal, eligible)
    }

    pub fn project(&self) -> &std::path::Path {
        &self.project
    }

    pub fn tracker(&self) -> Result<tokio_util::task::TaskTracker, Error> {
        self.shared
            .upgrade()
            .map(|s| s.external_tasks.clone())
            .ok_or(Error::Unavailable)
    }

    /// Transport retirement observes all captured lifetime tokens, never current selection.
    /// Consent retirement is distinct from a protocol/delivery failure cancelling
    /// the run. Only the former may establish an intentional Stop receipt.
    /// Stop retires this run's approvals, not the connection's consent. Keep that
    /// distinction after the broker seals the run and its cancellation token fires.
    pub fn consent_active(&self) -> bool {
        !self.lease.revoked.is_cancelled() && Instant::now() < self.lease.expires
    }

    pub async fn consent_retired(&self) {
        tokio::select! {
            _ = self.lease.revoked.cancelled() => {},
            _ = tokio::time::sleep_until(self.lease.expires.into()) => {},
        }
    }

    pub async fn retired(&self) {
        let Some(stopping) = self.shared.upgrade().map(|s| s.stopping.clone()) else {
            return;
        };
        tokio::select! {
            _ = self.lease.revoked.cancelled() => {},
            _ = self.lease.run_cancel.cancelled() => {},
            _ = self.removed.cancelled() => {},
            _ = stopping.cancelled() => {},
            _ = tokio::time::sleep_until(self.lease.expires.into()) => {},
        }
    }

    pub fn run_nonce(&self) -> Binding {
        self.run_nonce
    }
    fn tick(&self) -> u64 {
        self.origin
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }
    fn session_rt(&self) -> Result<Arc<SessionRt>, Error> {
        self.session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or(Error::Unavailable)
    }
    fn live(&self) -> bool {
        !self.lease.revoked.is_cancelled()
            && !self.lease.run_cancel.is_cancelled()
            && !self.removed.is_cancelled()
            && Instant::now() < self.lease.expires
            && self
                .shared
                .upgrade()
                .is_some_and(|s| !s.stopping.is_cancelled())
    }
    fn request(&self, ticket: &Ticket, expires_at_ms: i64) -> ExternalPermissionRequest {
        ExternalPermissionRequest {
            key: ExternalPermissionKey {
                engine_epoch: self.scope.epoch,
                owner: self.scope.owner.to_string(),
                run: self.scope.key.clone(),
                prompt: self.prompt,
                invocation_nonce: ticket.nonce.iter().map(|b| format!("{b:02x}")).collect(),
            },
            arguments_sha256: ticket
                .fingerprint
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
            tool_name: "Write".into(),
            policy_id: EXTERNAL_PERMISSION_POLICY.into(),
            expires_at_ms,
        }
    }

    /// Validate before durable Pending, then expose the complete review only after commit.
    /// No CLI tool-use ID is fabricated. Duplicate/mismatched invocations never re-admit.
    pub fn admit(&self, frame: &RequestFrame) -> Result<Review, Error> {
        frame.validate().map_err(|_| Error::Request)?;
        if frame.run_nonce != self.run_nonce {
            return Err(Error::Scope);
        }
        let parsed =
            permission::PermissionRequest::parse(frame.payload.as_bytes(), &self.scope.cli)
                .map_err(|_| Error::Request)?;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !self.live() || state.gate.is_fenced() {
            self.fence_locked(&mut state);
            return Err(Error::Unavailable);
        }
        let expires = self.lease.expires.min(Instant::now() + REVIEW_LIFETIME);
        let deadline = expires
            .duration_since(self.origin)
            .as_millis()
            .try_into()
            .map_err(|_| Error::Unavailable)?;
        let review = state
            .gate
            .admit(
                &self.scope,
                *frame.invocation.as_bytes(),
                parsed,
                self.tick(),
                deadline,
            )
            .map_err(|_| Error::Request)?;
        let expires_at_ms = now_ms().saturating_add(
            expires
                .saturating_duration_since(Instant::now())
                .as_millis() as i64,
        );
        let request = self.request(&review.ticket, expires_at_ms);
        let result = self
            .shared
            .upgrade()
            .ok_or(Error::Unavailable)
            .and_then(|shared| {
                let session = self.session_rt()?;
                shared
                    .external_commit(&session, |tx| {
                        // Existing rows are never another admission, even if they look Pending.
                        if Store::external_permission_in(tx, &request.key)?.is_some() {
                            return Err(StoreError::ExecutionConflict);
                        }
                        Store::request_external_permission_in(tx, &request, now_ms())
                            .map(|r| (r, vec![]))
                    })
                    .map_err(|_| Error::Persistence)
            });
        if result.is_err() || !self.live() || Instant::now() >= expires {
            self.fence_locked(&mut state);
            return Err(result.err().unwrap_or(Error::Unavailable));
        }
        let review = Review {
            arguments: review.argument_display().to_owned(),
            ticket: review.ticket,
            expires_at_ms,
            expires,
        };
        state.pending.insert(
            review.ticket.nonce,
            Pending {
                review: review.clone(),
                request,
                expires,
            },
        );
        Ok(review)
    }

    /// Owner-only ephemeral view; no DB access and no reconstruction after restart.
    pub fn reviews(&self, owner: u64) -> Result<Vec<Review>, Error> {
        if owner != self.scope.owner {
            return Err(Error::Scope);
        }
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !self.live() || state.gate.is_fenced() {
            return Ok(vec![]);
        }
        Ok(state
            .pending
            .values()
            .filter(|p| Instant::now() < p.expires)
            .map(|p| p.review.clone())
            .collect())
    }

    /// Lost acknowledgements use this read-only receipt lookup, never decide again.
    /// It remains available after fencing/expiry and cannot reconstruct a grant.
    pub fn lookup(
        &self,
        owner: u64,
        ticket: &Ticket,
    ) -> Result<Option<crate::store::ExternalPermissionReceipt>, Error> {
        if owner != self.scope.owner || ticket.scope != self.scope {
            return Err(Error::Scope);
        }
        let expected = self.request(ticket, 0);
        let shared = self.shared.upgrade().ok_or(Error::Unavailable)?;
        let receipt = shared
            .store
            .read(|c| Store::external_permission_in(c, &expected.key))
            .map_err(|_| Error::Persistence)?;
        if receipt
            .as_ref()
            .is_some_and(|r| r.request.arguments_sha256 != expected.arguments_sha256)
        {
            return Err(Error::Scope);
        }
        Ok(receipt)
    }

    /// The transport callback gets only the fixed CLI response bytes. A private wire
    /// adapter must encode the SAME decision and bound it to this exact invocation.
    /// Never enqueue an allow for later delivery, await, retry, or re-enter this broker.
    pub fn decide<E>(
        &self,
        owner: u64,
        ticket: &Ticket,
        decision: Decision,
        deliver: impl FnOnce(&[u8]) -> Result<(), E>,
    ) -> Result<(), Error> {
        if owner != self.scope.owner || ticket.scope != self.scope {
            return Err(Error::Scope);
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !self.live() || state.gate.is_fenced() {
            self.fence_locked(&mut state);
            return Err(Error::Unavailable);
        }
        let pending = state.pending.get(&ticket.nonce).ok_or(Error::Decision)?;
        if pending.review.ticket != *ticket {
            return Err(Error::Scope);
        }
        let request = pending.request.clone();
        let expires = pending.expires;
        if Instant::now() >= expires {
            self.fence_locked(&mut state);
            return Err(Error::Unavailable);
        }
        // A callback panic still cancels the owned run; BeforeSignal must call fence
        // before cleanup. The pure gate itself remains permanently fenced on unwind.
        let mut unwind = CancelOnUnwind {
            token: &self.lease.run_cancel,
            armed: true,
        };
        let result = state.gate.decide(
            ticket,
            decision,
            self.tick(),
            |_| {
                let shared = self.shared.upgrade().ok_or(Error::Unavailable)?;
                let session = self.session_rt()?;
                shared
                    .external_commit(&session, |tx| {
                        let prior = Store::external_permission_in(tx, &request.key)?
                            .ok_or(StoreError::ExecutionConflict)?;
                        if prior.state != ExternalPermissionState::Pending
                            || prior.decision.is_some()
                        {
                            return Err(StoreError::ExecutionConflict);
                        }
                        let d = match decision {
                            Decision::AllowOnce => ExternalPermissionDecision::AllowOnce,
                            Decision::Deny => ExternalPermissionDecision::Deny,
                        };
                        let receipt =
                            Store::decide_external_permission_in(tx, &request, d, now_ms())?;
                        match decision {
                            Decision::AllowOnce
                                if !Store::consume_external_permission_in(
                                    tx,
                                    &request,
                                    now_ms(),
                                )?
                                .newly_consumed =>
                            {
                                return Err(StoreError::ExecutionConflict);
                            }
                            Decision::Deny if receipt.state != ExternalPermissionState::Denied => {
                                return Err(StoreError::ExecutionConflict);
                            }
                            _ => {}
                        }
                        Ok(((), vec![]))
                    })
                    .map_err(|_| Error::Persistence)?;
                #[cfg(test)]
                if let Some(hook) = self.after_commit.lock().unwrap().take() {
                    hook();
                }
                // A late successful commit remains consumed/uncertain, not usable authority.
                if !self.live() || Instant::now() >= expires {
                    return Err(Error::Unavailable);
                }
                Ok(())
            },
            |bytes| {
                if !self.live() || Instant::now() >= expires {
                    return Err(());
                }
                deliver(bytes).map_err(|_| ())
            },
        );
        if result.is_err() {
            self.fence_locked(&mut state);
            return Err(Error::Decision);
        }
        unwind.armed = false;
        state.pending.remove(&ticket.nonce);
        // Fixed denial interrupts the CLI. Retire other requests under the SAME lock.
        if decision == Decision::Deny {
            self.fence_locked(&mut state);
        }
        Ok(())
    }

    /// Close authority without signaling. Capture unresolved evidence BEFORE cleanup;
    /// callers must not upgrade this cleanup to proof of clean CLI completion.
    pub fn seal(&self) -> Seal {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.seal_locked(&mut state)
    }
    fn seal_locked(&self, state: &mut State) -> Seal {
        if let Some(seal) = state.seal {
            return seal;
        }
        state.gate.fence();
        let unresolved = !state.pending.is_empty();
        let session = self.session_rt().ok();
        let result = self
            .shared
            .upgrade()
            .ok_or(Error::Unavailable)
            .and_then(|shared| {
                // Cleanup-only metadata transaction: still usable after removal or
                // a session storage fence. Ordinary Shared::commit remains guarded.
                let _publication = shared.publication.lock().unwrap_or_else(|e| e.into_inner());
                let result = tokio::task::block_in_place(|| {
                    shared.store.tx(|tx| {
                        Store::revoke_external_prompt_permissions_in(
                            tx,
                            &self.scope.key,
                            self.prompt,
                            now_ms(),
                        )
                    })
                });
                if let Err(error) = &result
                    && !matches!(error, StoreError::ExecutionConflict | StoreError::Removed)
                    && let Some(session) = &session
                {
                    shared.storage_failure(session, &error.to_string());
                }
                result.map_err(|_| Error::Persistence)
            });
        let seal = Seal {
            revocation_confirmed: result.is_ok(),
            unresolved: unresolved || result.is_err() || result.as_ref().is_ok_and(|n| *n != 0),
        };
        state.pending.clear();
        state.seal = Some(seal);
        self.session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        seal
    }
    fn fence_locked(&self, state: &mut State) -> bool {
        let confirmed = self.seal_locked(state).revocation_confirmed;
        // Persistence failure still permits cleanup; it cannot prove clean cancellation.
        self.lease.run_cancel.cancel();
        confirmed
    }
    pub fn fence(&self) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.fence_locked(&mut state)
    }
}
impl Drop for Broker {
    fn drop(&mut self) {
        self.fence();
    }
}

#[cfg(test)]
#[path = "external_permissions_tests.rs"]
mod tests;
