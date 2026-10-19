//! One-use input ticket minted by explicit person intent, bound to the original grant.
use super::*;
use crate::store::{RequestState, cli_agent_turns::inputs::Queued};
pub(in crate::runtime) struct InputTicket {
    registry: Registry,
    grant: Arc<Grant>,
    epoch: CancellationToken,
    pub queued: Queued,
    issued: bool,
}
impl Registry {
    pub(in crate::runtime) fn queue_input(
        &self,
        store: &Store,
        owner: u64,
        q: Queued,
    ) -> Result<(Option<InputTicket>, u64), Error> {
        self.queue_input_during(store, owner, q, None)
    }
    pub(in crate::runtime) fn queue_input_during(
        &self,
        store: &Store,
        owner: u64,
        q: Queued,
        expected: Option<&InputEpoch>,
    ) -> Result<(Option<InputTicket>, u64), Error> {
        let mut state = self.state.lock().unwrap();
        self.owner(&state, owner)?;
        let grant = state
            .grants
            .get(&q.session)
            .filter(|g| g.owner == owner && !g.cancel.is_cancelled() && g.child.is_none())
            .cloned()
            .ok_or(Error::Unavailable)?;
        if let Some(expected) = expected {
            expected.check(&grant)?;
        }
        live(store, &grant.binding)?;
        let epoch = grant.continuations.lock().unwrap().child_token();
        let (row, new) = store
            .queue_cli_input(&grant.binding, &q, now_ms())
            .map_err(|_| Error::Storage)?;
        let entry = row.queue_entry.ok_or(Error::Identity)?;
        if new {
            state.inputs.insert(
                (q.session.clone(), q.request.clone()),
                (q.clone(), epoch.clone(), Arc::downgrade(&grant)),
            );
        }

        Ok((
            new.then(|| InputTicket {
                registry: self.clone(),
                grant,
                epoch,
                queued: q,
                issued: false,
            }),
            entry,
        ))
    }
}
impl Registry {
    pub(in crate::runtime) fn cancel_queued_input(&self, session: &str, request: &str) {
        if let Some((_, cancel, _)) = self
            .state
            .lock()
            .unwrap()
            .inputs
            .get(&(session.into(), request.into()))
        {
            cancel.cancel();
        }
    }
}
impl Drop for InputTicket {
    fn drop(&mut self) {
        self.epoch.cancel();
        let mut state = self.registry.state.lock().unwrap();
        let key = (self.queued.session.clone(), self.queued.request.clone());
        if state
            .inputs
            .get(&key)
            .is_some_and(|(q, _, _)| q.ticket == self.queued.ticket)
        {
            state.inputs.remove(&key);
        }
    }
}

impl InputTicket {
    pub fn cancellation(&self) -> CancellationToken {
        self.epoch.clone()
    }
    pub fn revoked(&self) -> CancellationToken {
        self.grant.cancel.clone()
    }
    pub fn begin(&mut self, store: &Store) -> Result<Turn, Error> {
        let mut state = self.registry.state.lock().unwrap();
        self.registry.owner(&state, self.grant.owner)?;
        if self.issued
            || self.epoch.is_cancelled()
            || self.grant.cancel.is_cancelled()
            || !state
                .grants
                .get(&self.queued.session)
                .is_some_and(|g| Arc::ptr_eq(g, &self.grant))
        {
            return Err(Error::Unavailable);
        }
        let row = store
            .request(&self.queued.session, &self.queued.request)
            .map_err(|_| Error::Storage)?
            .ok_or(Error::Identity)?;
        if row.state != RequestState::Queued {
            return Err(Error::Unavailable);
        }
        if state.active.contains_key(&self.queued.session)
            || !store
                .cli_input_is_next(&self.queued)
                .map_err(|_| Error::Storage)?
        {
            return Err(Error::Busy);
        }
        live(store, &self.grant.binding)?;
        store
            .check_cli_agent_ready(&self.grant.binding)
            .map_err(|_| Error::Identity)?;
        let id = uuid_v4();
        let cancel = self.grant.cancel.child_token();
        state
            .active
            .insert(self.queued.session.clone(), (id.clone(), cancel.clone()));
        // Queued person intent takes precedence over an idle autonomous goal controller.
        if let Some((_, goal)) = state.goals.get(&self.queued.session) {
            goal.cancel();
        }
        self.issued = true;
        Ok(Turn {
            queued: Some(self.queued.clone()),
            goal: None,
            child_return: None,
            wake: None,
            wake_cancel: Some(self.epoch.clone()),
            registry: self.registry.clone(),
            grant: self.grant.clone(),
            id,
            cancel,
        })
    }
}

/// Select only still-live, original connected tickets. Persisted rows alone never qualify.
/// Homogeneous batches keep compaction isolated and steering ahead of follow-ups.
pub(super) fn select(
    store: &Store,
    state: &State,
    session: &str,
    kind: crate::store::cli_agent_turns::inputs::Kind,
    mode: crate::config::QueueDelivery,
    leader: Option<&Queued>,
) -> Result<Vec<Queued>, Error> {
    use crate::{config::QueueDelivery, store::cli_agent_turns::inputs::Kind};
    if leader.is_some_and(|q| !q.images.is_empty())
        || kind == Kind::Compact
        || (leader.is_some() && mode == QueueDelivery::OneAtATime)
    {
        return Ok(vec![]);
    }
    let mut bytes = leader.map_or(0, |q| q.text.len());
    let mut selected = Vec::new();
    for row in store.queue(session).map_err(|_| Error::Storage)? {
        if leader.is_some_and(|q| q.request == row.key) {
            continue;
        }
        let Some((q, cancel, original)) = state.inputs.get(&(session.into(), row.key)) else {
            continue;
        };
        if q.kind == Kind::Compact && kind == Kind::FollowUp {
            break;
        }
        if q.kind != kind {
            continue;
        }
        if !original.upgrade().is_some_and(|g| {
            !g.cancel.is_cancelled()
                && state
                    .grants
                    .get(session)
                    .is_some_and(|current| Arc::ptr_eq(current, &g))
        }) {
            continue;
        }
        if cancel.is_cancelled() {
            continue;
        }
        // Keep attachment messages intact in their own CLI turn. At tool boundaries
        // select one image message, which is delivered as structured MCP content.
        if !q.images.is_empty() {
            if leader.is_none() && selected.is_empty() {
                selected.push(q.clone());
            }
            break;
        }
        // Escaping can expand by six. Leave room for IDs, envelope and actual tool output.
        if bytes.saturating_add(q.text.len()) > 96 * 1024 {
            break;
        }
        bytes += q.text.len();
        selected.push(q.clone());
        if mode == QueueDelivery::OneAtATime {
            break;
        }
    }
    Ok(selected)
}
impl InputTicket {
    pub fn delivered(&self, store: &Store) -> Result<Option<cli_agent_turns::Receipt>, Error> {
        let state = self.registry.state.lock().unwrap();
        // Wait only for the delivering Turn, never an unrelated later continuation.
        let Some((receipt, owner)) = store
            .cli_input_delivery(&self.queued)
            .map_err(|_| Error::Storage)?
        else {
            return Ok(None);
        };
        if state
            .active
            .get(&self.queued.session)
            .is_some_and(|(id, _)| id == &owner)
        {
            return Ok(None);
        }
        Ok(Some(receipt))
    }

    pub fn consumed(&self, store: &Store) -> Result<bool, Error> {
        store
            .cli_input_delivery(&self.queued)
            .map(|r| r.is_some())
            .map_err(|_| Error::Storage)
    }
}
impl ServiceAuthority {
    pub(in crate::runtime) fn settle_with_steering(
        &self,
        store: &Store,
        lease: &crate::store::cli_bridge::Lease,
        claim: &crate::store::cli_bridge::Claim,
        result: &crate::tools::ToolResult,
        mode: crate::config::QueueDelivery,
    ) -> crate::store::Result<(
        (crate::store::cli_bridge::Settlement, serde_json::Value),
        Option<crate::store::cli_agent_turns::projection::Update>,
    )> {
        let s = self.registry.state.lock().unwrap();
        self.check(&s)?;
        if lease.scope() != self.scope() {
            return Err(crate::store::StoreError::ExecutionConflict);
        }
        let chosen = select(
            store,
            &s,
            &self.scope.session,
            crate::store::cli_agent_turns::inputs::Kind::Steer,
            mode,
            None,
        )
        .map_err(|_| crate::store::StoreError::ExecutionConflict)?;
        store.settle_cli_native_call_with_inputs(lease, claim, result, &chosen, now_ms())
    }
}

impl Registry {
    /// Person steering is attached to this exact active child turn. It cannot start a
    /// completed child or recreate its grant, and auditors never accept steering.
    pub(in crate::runtime) fn steer_child(
        &self,
        store: &Store,
        owner: u64,
        parent: &str,
        q: Queued,
    ) -> Result<Option<InputTicket>, Error> {
        let mut state = self.state.lock().unwrap();
        self.owner(&state, owner)?;
        let root = state
            .grants
            .get(parent)
            .filter(|g| g.owner == owner && !g.cancel.is_cancelled() && g.child.is_none())
            .cloned()
            .ok_or(Error::Unavailable)?;
        live(store, &root.binding)?;
        let grant = state
            .grants
            .get(&q.session)
            .filter(|g| g.owner == owner && !g.cancel.is_cancelled())
            .cloned()
            .ok_or(Error::Unavailable)?;
        if !grant
            .child_parent
            .as_ref()
            .and_then(std::sync::Weak::upgrade)
            .is_some_and(|p| Arc::ptr_eq(&p, &root))
            || grant.child.as_ref().is_none_or(|p| p.audit().is_some())
            || q.kind != crate::store::cli_agent_turns::inputs::Kind::Steer
            || !q.images.is_empty()
        {
            return Err(Error::Identity);
        }
        let task = store
            .read(|c| Store::task_by_session(c, &q.session))
            .map_err(|_| Error::Storage)?
            .ok_or(Error::Identity)?;
        if task.parent_session != parent || task.finished() || task.note.is_some() {
            return Err(Error::Unavailable);
        }
        let epoch = state
            .active
            .get(&q.session)
            .filter(|(_, c)| !c.is_cancelled())
            .ok_or(Error::Unavailable)?
            .1
            .child_token();
        let (_, new) = store
            .queue_cli_input(&grant.binding, &q, now_ms())
            .map_err(|_| Error::Storage)?;
        if !new {
            return Ok(None);
        }
        state.inputs.insert(
            (q.session.clone(), q.request.clone()),
            (q.clone(), epoch.clone(), Arc::downgrade(&grant)),
        );
        Ok(Some(InputTicket {
            registry: self.clone(),
            grant,
            epoch,
            queued: q,
            issued: false,
        }))
    }
}
