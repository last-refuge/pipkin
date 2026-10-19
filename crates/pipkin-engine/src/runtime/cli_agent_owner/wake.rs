//! One-use continuation of an original connected approval. Stored wake data is not
//! an owner; reconnect/reapproval, Stop and restart never reconstruct this ticket.
use super::*;
use crate::store::{
    cli_agent_turns::{Wake, WakeTarget},
    cli_bridge,
};
pub(super) struct PendingWake {
    id: String,
    grant: std::sync::Weak<Grant>,
    epoch: CancellationToken,
    ready: Option<u64>,
    issued: bool,
}
impl PendingWake {
    fn live(&self, state: &State, session: &str) -> bool {
        !self.issued
            && !self.epoch.is_cancelled()
            && self.grant.upgrade().is_some_and(|g| {
                !g.cancel.is_cancelled()
                    && state
                        .grants
                        .get(session)
                        .is_some_and(|current| Arc::ptr_eq(current, &g))
            })
    }
}
pub(super) fn has_ready(state: &State, session: &str) -> bool {
    state
        .wakes
        .iter()
        .any(|((s, _), w)| s == session && w.ready.is_some() && w.live(state, session))
}
pub(in crate::runtime) struct WakeTicket {
    id: String,
    registry: Registry,
    grant: Arc<Grant>,
    epoch: CancellationToken,
    scope: crate::execution::claude_cli::orchestration_protocol::Scope,
    call: String,
    issued: bool,
    tool: String,
}
impl ServiceAuthority {
    pub fn wake_ticket(
        &self,
        store: &Store,
        lease: &cli_bridge::Lease,
        call: &str,
        tool: &str,
    ) -> Result<WakeTicket, Error> {
        let mut state = self.registry.state.lock().unwrap();
        self.check(&state).map_err(|_| Error::Unavailable)?;
        let key = (self.scope.session.clone(), call.to_owned());
        if self
            .grant
            .child
            .as_ref()
            .is_some_and(|p| tool != "job_start" || p.service() != "subagent")
            || lease.scope() != &self.scope
            || !matches!(tool, "subagent" | "job_start")
        {
            return Err(Error::Identity);
        }
        if state.wakes.contains_key(&key) || state.wakes.len() >= 1024 {
            return Err(Error::Busy);
        }
        store
            .read(|c| Store::assert_cli_native_service_in(c, lease, call, tool))
            .map_err(|_| Error::Identity)?;
        let id = uuid_v4();
        let epoch = self.grant.continuations.lock().unwrap().child_token();
        state.wakes.insert(
            key,
            PendingWake {
                id: id.clone(),
                grant: Arc::downgrade(&self.grant),
                epoch: epoch.clone(),
                ready: None,
                issued: false,
            },
        );
        Ok(WakeTicket {
            id,
            registry: self.registry.clone(),
            grant: self.grant.clone(),
            epoch,
            scope: self.scope.clone(),
            call: call.into(),
            issued: false,
            tool: tool.into(),
        })
    }
}
impl WakeTicket {
    pub fn cancellation(&self) -> CancellationToken {
        self.epoch.clone()
    }
    pub fn revoked(&self) -> CancellationToken {
        self.grant.cancel.clone()
    }
    pub fn generation(&self) -> u64 {
        self.scope.generation
    }
    /// Busy is a read-only wait. Once a Turn is minted, this ticket can never mint another,
    /// including when submission fails before admission or its receiver disappears.
    pub fn try_begin(
        &mut self,
        store: &Store,
        target: WakeTarget,
        text: String,
    ) -> Result<(Turn, String), Error> {
        if target.tool() != self.tool {
            return Err(Error::Identity);
        }
        let mut state = self.registry.state.lock().unwrap();
        self.registry.owner(&state, self.grant.owner)?;
        let session = &self.grant.binding.create.session;
        if self.issued
            || self.epoch.is_cancelled()
            || self.grant.cancel.is_cancelled()
            || !state
                .grants
                .get(session)
                .is_some_and(|g| Arc::ptr_eq(g, &self.grant))
        {
            return Err(Error::Unavailable);
        }
        let key = (session.clone(), self.call.clone());
        if !state.wakes.get(&key).is_some_and(|w| w.id == self.id) {
            return Err(Error::Unavailable);
        }
        if state.wakes.get(&key).is_some_and(|w| w.ready.is_none()) {
            state.wake_sequence = state
                .wake_sequence
                .checked_add(1)
                .ok_or(Error::Unavailable)?;
            let sequence = state.wake_sequence;
            state.wakes.get_mut(&key).ok_or(Error::Unavailable)?.ready = Some(sequence);
        }
        let first = state
            .wakes
            .iter()
            .filter(|((s, _), w)| s == session && w.live(&state, session))
            .filter_map(|(_, w)| w.ready.map(|n| (n, w.id.as_str())))
            .min_by_key(|(n, _)| *n);
        if state.active.contains_key(session)
            || first.is_none_or(|(_, id)| id != self.id)
            || !store.queue(session).map_err(|_| Error::Storage)?.is_empty()
        {
            return Err(Error::Busy);
        }
        if let Some(policy) = &self.grant.child {
            let parent = self
                .grant
                .child_parent
                .as_ref()
                .and_then(std::sync::Weak::upgrade)
                .ok_or(Error::Unavailable)?;
            if parent.cancel.is_cancelled()
                || !state
                    .grants
                    .get(&parent.binding.create.session)
                    .is_some_and(|g| Arc::ptr_eq(g, &parent))
                || policy.service() != "subagent"
            {
                return Err(Error::Unavailable);
            }
            let task = store
                .read(|c| Store::task_by_session(c, session))
                .map_err(|_| Error::Storage)?
                .ok_or(Error::Identity)?;
            // The original child actor must publish its result and consume its parent
            // notification before a job wake can replace the child's latest run.
            if !task.finished() || matches!(task.delivery.as_str(), "wake" | "notify") {
                return Err(Error::Busy);
            }
            if task.status != "done" {
                return Err(Error::Identity);
            }
        } else {
            live(store, &self.grant.binding)?;
        }
        store
            .check_cli_agent_ready(&self.grant.binding)
            .map_err(|_| Error::Identity)?;
        let goals = store
            .read(|c| Store::cli_run_goals_in(c, session, self.scope.run))
            .map_err(|_| Error::Storage)?;
        for (root, id) in goals {
            let goal = store
                .read(|c| Store::goal_current(c, &root))
                .map_err(|_| Error::Storage)?
                .filter(|g| g.id == id && g.state == pipkin_engine_protocol::GoalState::Active)
                .ok_or(Error::Unavailable)?;
            let usage = store
                .read(|c| Store::cli_goal_usage_in(c, &root, goal.id))
                .map_err(|_| Error::Storage)?;
            if usage.active > 0 {
                return Err(Error::Busy);
            }
            store
                .read(|c| Store::cli_goal_allow_in(c, &root, id, true))
                .map_err(|_| Error::Unavailable)?;
        }
        let wake = store
            .read(|c| Wake::capture(c, target, session, &self.call, self.scope.run, text))
            .map_err(|_| Error::Identity)?;
        let request = wake.request().to_owned();
        let id = uuid_v4();
        let cancel = self.grant.cancel.child_token();
        state
            .active
            .insert(session.clone(), (id.clone(), cancel.clone()));
        self.issued = true;
        state.wakes.get_mut(&key).ok_or(Error::Unavailable)?.issued = true;
        Ok((
            Turn {
                queued: None,
                goal: None,
                child_return: None,
                registry: self.registry.clone(),
                grant: self.grant.clone(),
                id,
                cancel,
                wake: Some(wake),
                wake_cancel: Some(self.epoch.clone()),
            },
            request,
        ))
    }
}

impl Drop for WakeTicket {
    fn drop(&mut self) {
        self.epoch.cancel();
        let mut state = self.registry.state.lock().unwrap();
        let key = (self.scope.session.clone(), self.call.clone());
        if state.wakes.get(&key).is_some_and(|w| w.id == self.id) {
            state.wakes.remove(&key);
        }
    }
}

#[cfg(test)]
impl Registry {
    pub(in crate::runtime) fn ready_wakes(&self, session: &str) -> usize {
        let state = self.state.lock().unwrap();
        state
            .wakes
            .iter()
            .filter(|((s, _), w)| s == session && w.ready.is_some() && w.live(&state, session))
            .count()
    }
}
