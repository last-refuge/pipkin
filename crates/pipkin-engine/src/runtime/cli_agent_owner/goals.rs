//! An explicit goal start/resume retains the original connected root approval. This
//! capability is never reconstructed by startup, session metadata or a model tool call.
use super::*;
use crate::store::cli_agent_turns::GoalTurn;

pub(in crate::runtime) struct GoalOwner {
    registry: Registry,
    grant: Arc<Grant>,
    epoch: CancellationToken,
    id: String,
}
impl Registry {
    pub(in crate::runtime) fn goal_owner(
        &self,
        store: &Store,
        owner: u64,
        session: &str,
    ) -> Result<GoalOwner, Error> {
        self.goal_owner_during(store, owner, session, None)
    }
    pub(in crate::runtime) fn goal_owner_during(
        &self,
        store: &Store,
        owner: u64,
        session: &str,
        expected: Option<&InputEpoch>,
    ) -> Result<GoalOwner, Error> {
        let mut state = self.state.lock().unwrap();
        self.owner(&state, owner)?;
        let grant = state
            .grants
            .get(session)
            .filter(|g| g.owner == owner && !g.cancel.is_cancelled())
            .cloned()
            .ok_or(Error::Unavailable)?;
        if let Some(expected) = expected {
            expected.check(&grant)?;
        }
        live(store, &grant.binding)?;
        store
            .check_cli_agent_ready(&grant.binding)
            .map_err(|_| Error::Identity)?;
        if state.goals.contains_key(session) {
            return Err(Error::Busy);
        }
        let id = uuid_v4();
        let epoch = grant.continuations.lock().unwrap().child_token();
        state
            .goals
            .insert(session.into(), (id.clone(), epoch.clone()));
        Ok(GoalOwner {
            registry: self.clone(),
            grant,
            epoch,
            id,
        })
    }
}
impl GoalOwner {
    pub fn session(&self) -> &str {
        &self.grant.binding.create.session
    }
    pub fn cancellation(&self) -> CancellationToken {
        self.epoch.clone()
    }
    pub fn revoked(&self) -> CancellationToken {
        self.grant.cancel.clone()
    }
    fn current(&self, state: &State) -> Result<(), Error> {
        self.registry.owner(state, self.grant.owner)?;
        if self.epoch.is_cancelled()
            || self.grant.cancel.is_cancelled()
            || !state
                .grants
                .get(self.session())
                .is_some_and(|g| Arc::ptr_eq(g, &self.grant))
            || !state
                .goals
                .get(self.session())
                .is_some_and(|(id, _)| id == &self.id)
        {
            return Err(Error::Unavailable);
        }
        Ok(())
    }
    pub fn authorize<T>(
        &self,
        f: impl FnOnce() -> crate::store::Result<T>,
    ) -> crate::store::Result<T> {
        let state = self.registry.state.lock().unwrap();
        self.current(&state)
            .map_err(|_| crate::store::StoreError::ExecutionConflict)?;
        f()
    }
    pub fn begin(&self, store: &Store, goal: GoalTurn) -> Result<Turn, Error> {
        let mut state = self.registry.state.lock().unwrap();
        self.current(&state)?;
        if state.active.contains_key(self.session()) || wake::has_ready(&state, self.session()) {
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
            .insert(self.session().into(), (id.clone(), cancel.clone()));
        Ok(Turn {
            queued: None,
            goal: Some(goal),
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
impl Drop for GoalOwner {
    fn drop(&mut self) {
        self.epoch.cancel();
        let mut state = self.registry.state.lock().unwrap();
        if state
            .goals
            .get(self.session())
            .is_some_and(|(id, _)| id == &self.id)
        {
            state.goals.remove(self.session());
        }
    }
}
