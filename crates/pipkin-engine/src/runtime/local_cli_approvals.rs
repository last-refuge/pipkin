//! Owner-scoped RPC facade for external approvals. No launch, replay or native tool authority.
use super::Shared;
use crate::store::{
    EXTERNAL_PERMISSION_POLICY, ExternalPermissionDecision as D, ExternalPermissionKey,
    ExternalPermissionReceipt, ExternalPermissionState as S, ExternalRunKey, Store,
};
use pipkin_engine_protocol::{
    ErrorCode, LocalCliApprovalDecision as Decision, LocalCliApprovalId as Id,
    LocalCliApprovalReceipt as Receipt, LocalCliApprovalState as State, ProtoError, Reply,
};
#[cfg(unix)]
use {
    super::external_permission_server::Control,
    crate::execution::claude_cli::permission::{self, Ticket},
    pipkin_engine_protocol::{LocalCliApprovalReview as Review, LocalCliApprovalRun as Run},
    std::{
        collections::{HashMap, HashSet},
        sync::{Arc, Mutex, Weak},
    },
};

#[derive(Clone, Default)]
pub(super) struct Registry {
    #[cfg(unix)]
    inner: Arc<Mutex<Entries>>,
}
#[cfg(unix)]
#[derive(Default)]
struct Entries {
    closed: bool,
    owners: HashSet<u64>,
    runs: HashMap<String, (u64, Weak<Control>)>,
}
#[cfg(unix)]
pub(super) struct Registration {
    registry: Registry,
    session: String,
    control: Weak<Control>,
}
#[cfg(unix)]
impl Drop for Registration {
    fn drop(&mut self) {
        let mut state = self
            .registry
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if state
            .runs
            .get(&self.session)
            .is_some_and(|(_, c)| c.ptr_eq(&self.control))
        {
            state.runs.remove(&self.session);
        }
    }
}
fn unavailable() -> ProtoError {
    ProtoError::new(
        ErrorCode::NotFound,
        "CLI approval is unavailable for this connection.",
    )
}
#[cfg(unix)]
fn uncertain() -> ProtoError {
    ProtoError::new(
        ErrorCode::Cancelled,
        "CLI decision outcome may be unknown. Use read-only lookup; never resend an allow.",
    )
}
impl Registry {
    pub fn connect(&self, owner: u64) {
        #[cfg(unix)]
        {
            let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if !state.closed {
                state.owners.insert(owner);
            }
        }
        #[cfg(not(unix))]
        let _ = owner;
    }
    #[cfg(unix)]
    pub fn install(&self, control: &Arc<Control>) -> Result<Registration, ProtoError> {
        if !control.live() {
            return Err(unavailable());
        }
        let scope = control.scope();
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        state.runs.retain(|_, (_, c)| c.strong_count() != 0);
        if state.closed
            || !state.owners.contains(&scope.owner)
            || state.runs.len() >= 64
            || state.runs.contains_key(&scope.key.session)
        {
            return Err(unavailable());
        }
        let weak = Arc::downgrade(control);
        state
            .runs
            .insert(scope.key.session.clone(), (scope.owner, weak.clone()));
        Ok(Registration {
            registry: self.clone(),
            session: scope.key.session.clone(),
            control: weak,
        })
    }
    #[cfg(unix)]
    fn get(&self, owner: u64, session: &str) -> Option<Arc<Control>> {
        let state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed || !state.owners.contains(&owner) {
            return None;
        }
        state
            .runs
            .get(session)
            .filter(|(o, _)| *o == owner)
            .and_then(|(_, c)| c.upgrade())
    }
    pub fn revoke_owner(&self, owner: u64) {
        #[cfg(unix)]
        {
            let controls = {
                let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                state.owners.remove(&owner);
                let mut controls = Vec::new();
                state.runs.retain(|_, (o, c)| {
                    if *o != owner {
                        return true;
                    }
                    if let Some(c) = c.upgrade() {
                        c.cancel();
                        controls.push(c);
                    }
                    false
                });
                controls
            };
            // No persistence here: the tracked actor/listener seals durably before
            // signals. Cancellation is already visible before registry unlock.
            drop(controls);
        }
        #[cfg(not(unix))]
        let _ = owner;
    }
    pub fn shutdown(&self) {
        #[cfg(unix)]
        {
            let controls = {
                let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                state.closed = true;
                state.owners.clear();
                state
                    .runs
                    .drain()
                    .filter_map(|(_, (_, c))| c.upgrade())
                    .inspect(|c| c.cancel())
                    .collect::<Vec<_>>()
            };
            // Preserve the engine shutdown deadline: do not synchronously wait
            // for broker/publication locks here. Owned cleanup performs the seal.
            drop(controls);
        }
    }
}
#[cfg(unix)]
fn run(control: &Control) -> Run {
    let s = control.scope();
    Run {
        epoch: s.epoch,
        session: s.key.session.clone(),
        generation: s.key.generation,
        op: s.key.op.clone(),
        prompt: control.prompt(),
    }
}
#[cfg(unix)]
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
#[cfg(unix)]
fn id(control: &Control, ticket: &Ticket) -> Id {
    Id {
        run: run(control),
        invocation: hex(&ticket.nonce),
        arguments_sha256: hex(&ticket.fingerprint),
    }
}
fn decision(value: Decision) -> D {
    match value {
        Decision::AllowOnce => D::AllowOnce,
        Decision::Deny => D::Deny,
    }
}
fn receipt(id: Id, r: ExternalPermissionReceipt) -> Receipt {
    Receipt {
        id,
        state: match r.state {
            S::Pending => State::Pending,
            S::Granted => State::Granted,
            S::Consumed => State::Consumed,
            S::Denied => State::Denied,
            S::Expired => State::Expired,
            S::Revoked => State::Revoked,
        },
        decision: r.decision.map(|d| match d {
            D::AllowOnce => Decision::AllowOnce,
            D::Deny => Decision::Deny,
        }),
    }
}
fn valid_id(id: &Id, epoch: u64, owner: u64) -> bool {
    let token = |s: &str| {
        !s.is_empty()
            && s.len() <= 128
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
    };
    let hash = |s: &str| {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    owner != 0
        && id.run.epoch == epoch
        && epoch != 0
        && id.run.prompt > 0
        && id.run.prompt <= i64::MAX as u64
        && id.run.generation > 0
        && id.run.generation <= i64::MAX as u64
        && token(&id.run.session)
        && token(&id.run.op)
        && hash(&id.invocation)
        && hash(&id.arguments_sha256)
}
impl Shared {
    fn cli_approval_receipt(
        &self,
        owner: u64,
        id: &Id,
    ) -> Result<Option<ExternalPermissionReceipt>, ProtoError> {
        if !valid_id(id, self.epoch, owner) {
            return Err(unavailable());
        }
        let key = ExternalPermissionKey {
            engine_epoch: self.epoch,
            owner: owner.to_string(),
            run: ExternalRunKey {
                session: id.run.session.clone(),
                generation: id.run.generation,
                op: id.run.op.clone(),
            },
            prompt: id.run.prompt,
            invocation_nonce: id.invocation.clone(),
        };
        let r = self
            .store
            .read(|c| Store::external_permission_in(c, &key))
            .map_err(|_| unavailable())?;
        if r.as_ref().is_some_and(|r| {
            r.request.arguments_sha256 != id.arguments_sha256
                || r.request.policy_id != EXTERNAL_PERMISSION_POLICY
                || r.request.tool_name != "Write"
        }) {
            return Err(unavailable());
        }
        Ok(r)
    }
    pub(super) fn lookup_local_cli_approval(
        &self,
        owner: u64,
        id: Id,
    ) -> Result<Reply, ProtoError> {
        Ok(Reply::LocalCliApproval {
            receipt: self
                .cli_approval_receipt(owner, &id)?
                .map(|r| receipt(id, r)),
        })
    }
    pub(super) fn local_cli_approvals(
        &self,
        owner: u64,
        session: &str,
    ) -> Result<Reply, ProtoError> {
        if session.is_empty() || session.len() > 128 {
            return Err(unavailable());
        }
        #[cfg(unix)]
        {
            if let Some(control) = self.local_cli_approvals.get(owner, session) {
                let reviews = control
                    .reviews(owner)
                    .map_err(|_| unavailable())?
                    .into_iter()
                    .map(|r| Review {
                        id: id(&control, &r.ticket),
                        arguments: r.arguments,
                        expires_at_ms: r.expires_at_ms,
                    })
                    .collect();
                return Ok(Reply::LocalCliApprovals {
                    run: Some(run(&control)),
                    reviews,
                });
            }
        }
        #[cfg(not(unix))]
        let _ = (owner, session);
        Ok(Reply::LocalCliApprovals {
            run: None,
            reviews: vec![],
        })
    }
    pub(super) fn decide_local_cli_approval(
        &self,
        owner: u64,
        requested: Id,
        choice: Decision,
    ) -> Result<Reply, ProtoError> {
        let prior = self
            .cli_approval_receipt(owner, &requested)?
            .ok_or_else(unavailable)?;
        if matches!(prior.state, S::Consumed | S::Denied)
            && prior.decision == Some(decision(choice))
        {
            // Duplicate acknowledgement only, never delivery (even if the original reply was lost).
            return Ok(Reply::LocalCliApproval {
                receipt: Some(receipt(requested, prior)),
            });
        }
        if prior.state != S::Pending {
            return Err(unavailable());
        }
        #[cfg(unix)]
        {
            let control = self
                .local_cli_approvals
                .get(owner, &requested.run.session)
                .ok_or_else(unavailable)?;
            let review = control
                .reviews(owner)
                .map_err(|_| unavailable())?
                .into_iter()
                .find(|r| id(&control, &r.ticket) == requested)
                .ok_or_else(unavailable)?;
            let choice = match choice {
                Decision::AllowOnce => permission::Decision::AllowOnce,
                Decision::Deny => permission::Decision::Deny,
            };
            control
                .decide(owner, &review.ticket, choice)
                .map_err(|_| uncertain())?;
            self.lookup_local_cli_approval(owner, requested)
        }
        #[cfg(not(unix))]
        Err(unavailable())
    }
}
