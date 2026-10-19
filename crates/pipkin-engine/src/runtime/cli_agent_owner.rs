//! Independent, revocable native-orchestration ownership used by explicit desktop setup.
//! Prepared metadata alone, Coding grants, saved bindings and duplicate creates grant nothing.
//! No model invocation, API resolver or credential access occurs in this registry.
#![allow(dead_code)] // Includes private driver/qualification seams.
use crate::{
    execution::claude_cli::preflight::{self, Prepared, Request},
    store::{
        Store,
        cli_agent::Binding,
        cli_agent_turns::{self, Lease},
    },
    util::{now_ms, uuid_v4},
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

mod children;
mod goals;
mod inputs;
mod person;
pub(super) use goals::GoalOwner;
pub(super) use inputs::InputTicket;
mod wake;
pub(super) use wake::WakeTicket;

const MAX_OWNERS: usize = 64;
const MAX_GRANTS: usize = 128;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Error {
    Unavailable,
    Identity,
    Busy,
    Storage,
    Preflight,
}
struct Grant {
    child_parent: Option<std::sync::Weak<Grant>>,
    child_resources: Option<Arc<super::cli_agent_host::ChildResources>>,
    continuations: Mutex<CancellationToken>,
    child: Option<Arc<crate::execution::claude_cli::orchestration_child::Frozen>>,
    owner: u64,
    binding: Binding,
    request: Request,
    cancel: CancellationToken,
}
/// A captured preparation boundary, not authority. Stop or replacement invalidates it.
pub(super) struct InputEpoch {
    grant: std::sync::Weak<Grant>,
    epoch: CancellationToken,
}
impl InputEpoch {
    fn check(&self, grant: &Arc<Grant>) -> Result<(), Error> {
        if self.epoch.is_cancelled()
            || !self
                .grant
                .upgrade()
                .is_some_and(|original| Arc::ptr_eq(&original, grant))
        {
            Err(Error::Unavailable)
        } else {
            Ok(())
        }
    }
}
#[derive(Default)]
struct State {
    wakes: HashMap<(String, String), wake::PendingWake>,
    wake_sequence: u64,
    inputs: HashMap<
        (String, String),
        (
            cli_agent_turns::inputs::Queued,
            CancellationToken,
            std::sync::Weak<Grant>,
        ),
    >,
    goals: HashMap<String, (String, CancellationToken)>,
    closed: bool,
    owners: HashMap<u64, CancellationToken>,
    grants: HashMap<String, Arc<Grant>>,
    // Revocation never releases this reservation: cleanup must finish and drop the Turn.
    active: HashMap<String, (String, CancellationToken)>,
}
#[derive(Clone)]
pub(crate) struct Registry {
    enabled: bool,
    shutting_down: CancellationToken,
    state: Arc<Mutex<State>>,
}
impl Registry {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            shutting_down: CancellationToken::new(),
            state: Arc::default(),
        }
    }
    #[cfg(test)]
    pub fn connect(&self, owner: u64) {
        self.connect_with_lifetime(owner, CancellationToken::new());
    }
    /// Connection cancellation propagates to grants without waiting for a registry/DB lock.
    pub fn connect_with_lifetime(&self, owner: u64, lifetime: CancellationToken) {
        let mut s = self.state.lock().unwrap();
        if self.enabled
            && !s.closed
            && !self.shutting_down.is_cancelled()
            && !lifetime.is_cancelled()
            && s.owners.len() < MAX_OWNERS
        {
            s.owners
                .entry(owner)
                .or_insert_with(|| lifetime.child_token());
        }
    }
    fn owner<'a>(&self, s: &'a State, owner: u64) -> Result<&'a CancellationToken, Error> {
        if !self.enabled || s.closed || self.shutting_down.is_cancelled() {
            return Err(Error::Unavailable);
        }
        s.owners
            .get(&owner)
            .filter(|c| !c.is_cancelled())
            .ok_or(Error::Unavailable)
    }
    /// Called ONLY for an explicit native-orchestration approval, after its own metadata
    /// check. The original successful create installs authority; exact repeats are lookup
    /// only, even after reconnect. UUIDs are minted here, never accepted from model output.
    pub fn create_approved(
        &self,
        store: &Store,
        owner: u64,
        prepared: &Prepared,
        session: &str,
        key: &str,
    ) -> Result<(Binding, bool), Error> {
        self.create_approved_with_thinking(store, owner, prepared, session, key, None)
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn create_approved_with_thinking(
        &self,
        store: &Store,
        owner: u64,
        prepared: &Prepared,
        session: &str,
        key: &str,
        thinking: Option<&str>,
    ) -> Result<(Binding, bool), Error> {
        let request = prepared
            .orchestration_request()
            .map_err(|_| Error::Preflight)?;
        let mut s = self.state.lock().unwrap();
        let owner_cancel = self.owner(&s, owner)?.clone();
        if let Some(existing) = store
            .cli_agent_binding(session)
            .map_err(|_| Error::Storage)?
        {
            let expected = prepared
                .orchestration_create_with_thinking(
                    session,
                    key,
                    &existing.create.identity.cli_session_id,
                    None,
                    thinking,
                )
                .map_err(|_| Error::Preflight)?;
            if expected != existing.create {
                return Err(Error::Identity);
            }
            return Ok((existing, false));
        }
        if s.grants.len() >= MAX_GRANTS {
            return Err(Error::Busy);
        }
        let create = prepared
            .orchestration_create_with_thinking(session, key, &uuid_v4(), None, thinking)
            .map_err(|_| Error::Preflight)?;
        let (binding, created) = store
            .create_cli_agent_session(&create, now_ms())
            .map_err(|_| Error::Storage)?;
        if !created {
            return Ok((binding, false));
        }
        // A committed binding can outlive a lost/cancelled reply, but never becomes a
        // reconnect grant. A caller in this case must perform metadata lookup only.
        self.owner(&s, owner)?;
        let grant = Arc::new(Grant {
            continuations: Mutex::new(CancellationToken::new()),
            child_parent: None,
            child_resources: None,
            child: None,
            owner,
            binding: binding.clone(),
            request,
            cancel: owner_cancel.child_token(),
        });
        s.grants.insert(session.into(), grant);
        Ok((binding, true))
    }
    /// Explicit reapproval is distinct from lookup. No identity/model/policy transition is
    /// performed; changed or fenced identities require another conversation, never fallback.
    pub fn approve_return(
        &self,
        store: &Store,
        owner: u64,
        prepared: &Prepared,
        expected: &Binding,
    ) -> Result<(), Error> {
        let request = prepared
            .orchestration_request()
            .map_err(|_| Error::Preflight)?;
        let mut s = self.state.lock().unwrap();
        let owner_cancel = self.owner(&s, owner)?.clone();
        let session = &expected.create.session;
        if s.active.contains_key(session) {
            return Err(Error::Busy);
        }
        if s.grants.get(session).is_some_and(|g| g.owner != owner) {
            return Err(Error::Unavailable);
        }
        if !s.grants.contains_key(session) && s.grants.len() >= MAX_GRANTS {
            return Err(Error::Busy);
        }
        live(store, expected)?;
        store
            .check_cli_agent_ready(expected)
            .map_err(|_| Error::Identity)?;
        let create = prepared
            .orchestration_create_with_thinking(
                session,
                &expected.create.request_key,
                &expected.create.identity.cli_session_id,
                None,
                expected.create.thinking.as_deref(),
            )
            .map_err(|_| Error::Preflight)?;
        if create != expected.create {
            return Err(Error::Identity);
        }
        self.owner(&s, owner)?;
        if let Some(old) = s.grants.remove(session) {
            old.cancel.cancel();
        }
        s.grants.retain(|_, g| !g.cancel.is_cancelled());
        s.grants.insert(
            session.clone(),
            Arc::new(Grant {
                continuations: Mutex::new(CancellationToken::new()),
                child_parent: None,
                child_resources: None,
                child: None,
                owner,
                binding: expected.clone(),
                request,
                cancel: owner_cancel.child_token(),
            }),
        );
        Ok(())
    }
    pub fn begin_turn(&self, store: &Store, owner: u64, session: &str) -> Result<Turn, Error> {
        let mut s = self.state.lock().unwrap();
        self.owner(&s, owner)?;
        let grant = s
            .grants
            .get(session)
            .filter(|g| g.owner == owner && !g.cancel.is_cancelled())
            .cloned()
            .ok_or(Error::Unavailable)?;
        if s.active.contains_key(session)
            || store
                .open_run(session)
                .map_err(|_| Error::Storage)?
                .is_some()
        {
            return Err(Error::Busy);
        }
        live(store, &grant.binding)?;
        store
            .check_cli_agent_ready(&grant.binding)
            .map_err(|_| Error::Identity)?;
        let id = uuid_v4();
        let cancel = grant.cancel.child_token();
        s.active
            .insert(session.into(), (id.clone(), cancel.clone()));
        if let Some((_, goal)) = s.goals.get(session) {
            goal.cancel();
        }
        Ok(Turn {
            queued: None,
            goal: None,
            child_return: None,
            wake: None,
            wake_cancel: None,
            registry: self.clone(),
            grant,
            id,
            cancel,
        })
    }
    pub(super) fn cancel_desktop_auditors(&self, owner: u64, session: &str, goal: u64, round: u32) {
        let state = self.state.lock().unwrap();
        let Some(parent) = state.grants.get(session).filter(|g| g.owner == owner) else {
            return;
        };
        for grant in state.grants.values() {
            if grant
                .child_parent
                .as_ref()
                .and_then(std::sync::Weak::upgrade)
                .is_some_and(|p| Arc::ptr_eq(&p, parent))
                && grant
                    .child
                    .as_ref()
                    .and_then(|p| p.audit())
                    .is_some_and(|g| g.id == goal && g.audit.rounds == round)
            {
                grant.cancel.cancel();
            }
        }
    }
    pub(super) fn input_epoch(&self, owner: u64, session: &str) -> Result<InputEpoch, Error> {
        let s = self.state.lock().unwrap();
        self.owner(&s, owner)?;
        let g = s
            .grants
            .get(session)
            .filter(|g| g.owner == owner && !g.cancel.is_cancelled())
            .ok_or(Error::Unavailable)?;
        Ok(InputEpoch {
            grant: Arc::downgrade(g),
            epoch: g.continuations.lock().unwrap().child_token(),
        })
    }
    pub fn approval_cancellation(&self, owner: u64, session: &str) -> Option<CancellationToken> {
        let s = self.state.lock().unwrap();
        s.grants
            .get(session)
            .filter(|g| g.owner == owner)
            .map(|g| g.cancel.clone())
    }
    /// Readiness only; never reconstructs authority or occupies a turn.
    pub fn approved(&self, store: &Store, owner: u64, session: &str) -> bool {
        let s = self.state.lock().unwrap();
        self.owner(&s, owner).is_ok()
            && s.grants.get(session).is_some_and(|g| {
                g.owner == owner
                    && !g.cancel.is_cancelled()
                    && live(store, &g.binding).is_ok()
                    && (s.active.contains_key(session)
                        || store.check_cli_agent_ready(&g.binding).is_ok())
            })
    }
    pub fn authorize_desktop<T>(
        &self,
        store: &Store,
        owner: u64,
        session: &str,
        f: impl FnOnce() -> crate::store::Result<T>,
    ) -> crate::store::Result<T> {
        let s = self.state.lock().unwrap();
        self.owner(&s, owner)
            .map_err(|_| crate::store::StoreError::ExecutionConflict)?;
        let g = s
            .grants
            .get(session)
            .filter(|g| g.owner == owner && !g.cancel.is_cancelled())
            .ok_or(crate::store::StoreError::ExecutionConflict)?;
        live(store, &g.binding).map_err(|_| crate::store::StoreError::ExecutionConflict)?;
        f()
    }
    pub fn has_turn(&self, session: &str) -> bool {
        self.state.lock().unwrap().active.contains_key(session)
    }
    /// An idle connected root can still own background continuation tickets.
    /// Stop must reach their epoch even after the foreground Turn has retired.
    pub fn owns_session(&self, session: &str) -> bool {
        let state = self.state.lock().unwrap();
        state.grants.contains_key(session) || state.active.contains_key(session)
    }
    pub fn stop(&self, session: &str) -> bool {
        let s = self.state.lock().unwrap();
        if let Some(grant) = s.grants.get(session) {
            let mut epoch = grant.continuations.lock().unwrap();
            epoch.cancel();
            *epoch = CancellationToken::new();
        }
        if let Some((_, cancel)) = s.active.get(session) {
            cancel.cancel();
            true
        } else {
            false
        }
    }
    pub fn revoke_session(&self, session: &str) {
        let mut s = self.state.lock().unwrap();
        if let Some(g) = s.grants.remove(session) {
            g.cancel.cancel();
        }
        if let Some((_, cancel)) = s.active.get(session) {
            cancel.cancel();
        }
        s.grants.retain(|_, g| !g.cancel.is_cancelled());
    }
    pub fn revoke_owner(&self, owner: u64) {
        let mut s = self.state.lock().unwrap();
        if let Some(c) = s.owners.remove(&owner) {
            c.cancel();
        }
        s.grants.retain(|_, g| {
            if g.owner == owner {
                g.cancel.cancel();
                false
            } else {
                true
            }
        });
    }
    pub fn shutdown(&self) {
        self.shutting_down.cancel();
        let mut s = self.state.lock().unwrap();
        s.closed = true;
        for (_, cancel) in s.owners.drain() {
            cancel.cancel();
        }
        for (_, g) in s.grants.drain() {
            g.cancel.cancel();
        }
        for (_, cancel) in s.active.values() {
            cancel.cancel();
        }
    }
}
fn live(store: &Store, binding: &Binding) -> Result<(), Error> {
    if binding.verified_digest() != Some(binding.digest)
        || binding.create.parent.is_some()
        || binding.root.is_some()
    {
        return Err(Error::Identity);
    }
    let session = &binding.create.session;
    if store
        .cli_agent_binding(session)
        .map_err(|_| Error::Storage)?
        .as_ref()
        != Some(binding)
        || !store
            .session(session)
            .map_err(|_| Error::Storage)?
            .is_some_and(|r| r.blocked.is_none())
    {
        return Err(Error::Identity);
    }
    Ok(())
}
/// Single active lifetime. Noncloneable/nonserializable; Drop cancels but does no SQLite I/O.
/// A dropped future may leave durable admission uncertain until owner/restart recovery.
pub(crate) struct Turn {
    queued: Option<cli_agent_turns::inputs::Queued>,
    goal: Option<cli_agent_turns::GoalTurn>,
    child_return: Option<children::ChildReturn>,
    wake: Option<crate::store::cli_agent_turns::Wake>,
    wake_cancel: Option<CancellationToken>,
    registry: Registry,
    grant: Arc<Grant>,
    id: String,
    cancel: CancellationToken,
}
impl Turn {
    pub fn input_mode(&self) -> pipkin_engine_protocol::InputMode {
        if let Some(q) = &self.queued {
            q.kind.mode()
        } else if self.wake.is_some() || self.goal.is_some() {
            pipkin_engine_protocol::InputMode::FollowUp
        } else {
            pipkin_engine_protocol::InputMode::Prompt
        }
    }
    pub fn owner_key(&self) -> &str {
        &self.id
    }
    pub fn binding(&self) -> &Binding {
        &self.grant.binding
    }
    pub fn cancellation(&self) -> CancellationToken {
        self.cancel.clone()
    }
    pub fn shutdown_token(&self) -> CancellationToken {
        self.registry.shutting_down.clone()
    }
    fn current(&self, s: &State) -> Result<(), Error> {
        self.registry.owner(s, self.grant.owner)?;
        if let Some(returned) = &self.child_return
            && !s
                .grants
                .get(&returned.parent.scope.session)
                .is_some_and(|g| Arc::ptr_eq(g, &returned.parent.grant))
        {
            return Err(Error::Unavailable);
        }
        if self.cancel.is_cancelled()
            || self
                .wake_cancel
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
            || !s
                .grants
                .get(&self.grant.binding.create.session)
                .is_some_and(|g| Arc::ptr_eq(g, &self.grant))
            || !s
                .active
                .get(&self.grant.binding.create.session)
                .is_some_and(|(id, _)| id == &self.id)
        {
            return Err(Error::Unavailable);
        }
        Ok(())
    }
    pub async fn prepare(&self) -> Result<Prepared, Error> {
        self.current(&self.registry.state.lock().unwrap())?;
        let r = &self.grant.request;
        let checked = preflight::check_orchestration(
            Request {
                candidate: r.candidate.clone(),
                environment: r.environment.clone(),
                model: r.model,
                trust_executable: true,
                trust_project: true,
            },
            self.cancel.clone(),
        )
        .await
        .map_err(|_| Error::Preflight)?;
        self.current(&self.registry.state.lock().unwrap())?;
        let b = self.binding();
        if checked
            .orchestration_create_with_thinking(
                &b.create.session,
                &b.create.request_key,
                &b.create.identity.cli_session_id,
                b.create.parent.clone(),
                b.create.thinking.as_deref(),
            )
            .map_err(|_| Error::Preflight)?
            != b.create
        {
            return Err(Error::Identity);
        }
        Ok(checked)
    }
    pub fn admit(
        &self,
        store: &Store,
        request: &str,
        generation: u64,
        operation: &str,
        text: &str,
    ) -> Result<cli_agent_turns::Admission, Error> {
        self.admit_batched(
            store,
            request,
            generation,
            operation,
            text,
            &[],
            crate::config::QueueDelivery::OneAtATime,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn admit_batched(
        &self,
        store: &Store,
        request: &str,
        generation: u64,
        operation: &str,
        text: &str,
        images: &[pipkin_engine_protocol::ImageInput],
        delivery: crate::config::QueueDelivery,
    ) -> Result<cli_agent_turns::Admission, Error> {
        let s = self.registry.state.lock().unwrap();
        self.current(&s)?;
        self.live_for_admission(store, request, generation, operation, text)?;
        if !images.is_empty()
            && (self.grant.child.is_some() || self.wake.is_some() || self.goal.is_some())
        {
            return Err(Error::Identity);
        }
        let batch = if let Some(q) = &self.queued {
            inputs::select(store, &s, &q.session, q.kind, delivery, Some(q))?
        } else {
            Vec::new()
        };
        store
            .admit_cli_agent_turn_with_batch(
                &cli_agent_turns::Input {
                    mode: self.input_mode(),
                    session: &self.binding().create.session,
                    request,
                    generation,
                    operation,
                    binding: self.binding().digest,
                    text,
                    images,
                },
                &self.id,
                self.wake.as_ref(),
                self.child_return.as_ref().map(|r| &r.data),
                self.goal.as_ref(),
                self.queued.as_ref(),
                &batch,
                now_ms(),
            )
            .map_err(|_| Error::Storage)
    }
    /// Structural proof also survives revocation so the original owner can retire its own
    /// admission. Cancellation independently vetoes launch and completion in the coordinator.
    pub fn belongs_to(&self, registry: &Registry) -> bool {
        Arc::ptr_eq(&self.registry.state, &registry.state)
    }
    pub(super) fn services(&self, lease: &Lease) -> Result<ServiceAuthority, Error> {
        self.current(&self.registry.state.lock().unwrap())?;
        if !self.matches(lease) {
            return Err(Error::Identity);
        }
        Ok(ServiceAuthority {
            registry: self.registry.clone(),
            grant: self.grant.clone(),
            id: self.id.clone(),
            cancel: self.cancel.clone(),
            scope: lease.receipt().scope.clone(),
        })
    }
    pub fn matches(&self, lease: &Lease) -> bool {
        lease.receipt().scope.session == self.binding().create.session
            && lease.matches_owner_binding(&self.id, &self.binding().digest)
    }
}
/// Delegated services only, never a Turn/launch grant. Captures original in-memory identity;
/// cannot be made from a binding, transcript, receipt or a caller-supplied scope.
#[derive(Clone)]
pub(super) struct ServiceAuthority {
    registry: Registry,
    grant: Arc<Grant>,
    id: String,
    cancel: CancellationToken,
    scope: crate::execution::claude_cli::orchestration_protocol::Scope,
}
impl ServiceAuthority {
    pub fn scope(&self) -> &crate::execution::claude_cli::orchestration_protocol::Scope {
        &self.scope
    }
    /// Lock order: publication -> registry -> storage. No await, no publication lock or
    /// registry re-entry inside f. Revocation signals cancellation independently of this lock.
    pub fn authorize<T>(
        &self,
        f: impl FnOnce() -> crate::store::Result<T>,
    ) -> crate::store::Result<T> {
        let s = self.registry.state.lock().unwrap();
        self.check(&s)?;
        f()
    }
    fn check(&self, s: &State) -> crate::store::Result<()> {
        if self.registry.owner(s, self.grant.owner).is_err()
            || self.cancel.is_cancelled()
            || !s
                .grants
                .get(&self.scope.session)
                .is_some_and(|g| Arc::ptr_eq(g, &self.grant))
            || !s
                .active
                .get(&self.scope.session)
                .is_some_and(|(id, _)| id == &self.id)
        {
            return Err(crate::store::StoreError::ExecutionConflict);
        }
        Ok(())
    }
}
impl Drop for Turn {
    fn drop(&mut self) {
        self.cancel.cancel();
        let mut s = self.registry.state.lock().unwrap();
        let session = &self.binding().create.session;
        if s.active.get(session).is_some_and(|(id, _)| id == &self.id) {
            s.active.remove(session);
        }
        if self.grant.child.is_some()
            && self.grant.cancel.is_cancelled()
            && s.grants
                .get(session)
                .is_some_and(|g| Arc::ptr_eq(g, &self.grant))
        {
            s.grants.remove(session);
        }
    }
}
