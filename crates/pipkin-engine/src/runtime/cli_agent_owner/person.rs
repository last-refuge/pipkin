//! Explicit desktop child intent. No parent model run or fabricated native tool call.
use super::*;
use crate::execution::claude_cli::{orchestration_child::Frozen, orchestration_protocol::Scope};
use crate::store::cli_agent_children::Created;
use tokio::time::Instant;

pub(in crate::runtime) struct PersonChild {
    registry: Registry,
    grant: Arc<Grant>,
    epoch: InputEpoch,
    pub scope: Scope,
}
impl Registry {
    pub(in crate::runtime) fn person_child(
        &self,
        store: &Store,
        owner: u64,
        session: &str,
        request: &str,
    ) -> Result<PersonChild, Error> {
        let state = self.state.lock().unwrap();
        self.owner(&state, owner)?;
        let grant = state
            .grants
            .get(session)
            .filter(|g| g.owner == owner && !g.cancel.is_cancelled() && g.child.is_none())
            .cloned()
            .ok_or(Error::Unavailable)?;
        live(store, &grant.binding)?;
        if !state.active.contains_key(session) {
            store
                .check_cli_agent_ready(&grant.binding)
                .map_err(|_| Error::Identity)?;
        }
        let epoch = InputEpoch {
            grant: Arc::downgrade(&grant),
            epoch: grant.continuations.lock().unwrap().child_token(),
        };
        Ok(PersonChild {
            registry: self.clone(),
            grant,
            epoch,
            scope: Scope {
                session: session.into(),
                generation: 1,
                operation: format!("user:{request}"),
                run: 0,
            },
        })
    }
}
impl PersonChild {
    fn check(&self, state: &State) -> Result<(), Error> {
        self.registry.owner(state, self.grant.owner)?;
        let current = state
            .grants
            .get(&self.scope.session)
            .ok_or(Error::Unavailable)?;
        self.epoch.check(current)?;
        if self.grant.cancel.is_cancelled() {
            return Err(Error::Unavailable);
        }
        Ok(())
    }
    pub fn binding(&self) -> &Binding {
        &self.grant.binding
    }
    pub fn context(
        &self,
        store: &Store,
    ) -> Result<(Vec<pipkin_engine_protocol::Entry>, u64), Error> {
        let state = self.registry.state.lock().unwrap();
        self.check(&state)?;
        live(store, &self.grant.binding)?;
        store
            .read(|c| Store::cli_child_context_at(c, &self.scope.session))
            .map_err(|_| Error::Storage)
    }
    pub async fn prepare(
        &self,
        policy: &Frozen,
        caller: CancellationToken,
        deadline: Instant,
    ) -> Result<Prepared, Error> {
        self.check(&self.registry.state.lock().unwrap())?;
        let r = &self.grant.request;
        let cancel = self.grant.cancel.child_token();
        let check = preflight::check_orchestration(
            Request {
                candidate: r.candidate.clone(),
                environment: r.environment.clone(),
                model: policy.model(),
                trust_executable: true,
                trust_project: true,
            },
            cancel.clone(),
        );
        tokio::pin!(check);
        let result = tokio::select! { biased;
            _=caller.cancelled()=>{cancel.cancel();let _=check.await;return Err(Error::Unavailable)},
            _=self.epoch.epoch.cancelled()=>{cancel.cancel();let _=check.await;return Err(Error::Unavailable)},
            _=tokio::time::sleep_until(deadline)=>{cancel.cancel();let _=check.await;return Err(Error::Unavailable)},
            r=&mut check=>r.map_err(|_|Error::Preflight)?,
        };
        self.check(&self.registry.state.lock().unwrap())?;
        Ok(result)
    }
    pub fn create(
        &self,
        store: &Store,
        policy: Arc<Frozen>,
        resources: Arc<crate::runtime::cli_agent_host::ChildResources>,
        prepared: &Prepared,
        caller: &CancellationToken,
    ) -> Result<(Created, Option<Turn>), Error> {
        let request = prepared
            .orchestration_request()
            .map_err(|_| Error::Preflight)?;
        let mut state = self.registry.state.lock().unwrap();
        self.check(&state)?;
        live(store, &self.grant.binding)?;
        if caller.is_cancelled()
            || state.grants.len() >= MAX_GRANTS
            || !policy.matches(&self.scope, &self.grant.binding, policy.tools())
        {
            return Err(Error::Unavailable);
        }
        let session = uuid_v4();
        let uuid = uuid_v4();
        let create = prepared
            .orchestration_create_with_thinking(
                &session,
                &policy.create_key(),
                &uuid,
                Some(self.scope.session.clone()),
                policy.record().thinking.as_deref(),
            )
            .map_err(|_| Error::Preflight)?;
        let mut identity = self.grant.binding.create.identity.clone();
        identity.cli_session_id = uuid.clone();
        if create.identity != identity || create.model != policy.model().as_str() {
            return Err(Error::Identity);
        }
        let created = store
            .tx(|tx| {
                Store::create_cli_person_child_in(
                    tx,
                    &self.scope,
                    &policy,
                    &session,
                    &uuid,
                    now_ms(),
                )
            })
            .map_err(|_| Error::Storage)?;
        if !created.created {
            return Ok((created, None));
        }
        let grant = Arc::new(Grant {
            continuations: Mutex::new(CancellationToken::new()),
            child_parent: Some(Arc::downgrade(&self.grant)),
            child_resources: Some(resources),
            child: Some(policy),
            owner: self.grant.owner,
            binding: created.binding.clone(),
            request,
            cancel: self.grant.cancel.child_token(),
        });
        let id = uuid_v4();
        let cancel = grant.cancel.child_token();
        state.grants.insert(session.clone(), grant.clone());
        state.active.insert(session, (id.clone(), cancel.clone()));
        Ok((
            created,
            Some(Turn {
                queued: None,
                goal: None,
                child_return: None,
                wake: None,
                wake_cancel: None,
                registry: self.registry.clone(),
                grant,
                id,
                cancel,
            }),
        ))
    }
}
