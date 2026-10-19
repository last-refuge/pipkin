//! Child inference authority is minted only from the original live parent and native call.
use super::*;
use crate::{
    execution::claude_cli::orchestration_child::{Delivery, Frozen},
    provider::ToolSpec,
    store::{cli_agent_children::Created, cli_bridge},
};
use tokio::time::Instant;
impl ServiceAuthority {
    pub fn binding(&self) -> &Binding {
        &self.grant.binding
    }
    pub async fn prepare_child(
        &self,
        policy: &Frozen,
        worker: &CancellationToken,
        deadline: Instant,
    ) -> Result<Prepared, Error> {
        self.authorize(|| Ok(())).map_err(|_| Error::Unavailable)?;
        if self.grant.child.is_some()
            || !policy.matches(&self.scope, &self.grant.binding, policy.tools())
            || worker.is_cancelled()
            || Instant::now() >= deadline
        {
            return Err(Error::Identity);
        }
        let r = &self.grant.request;
        let cancel = self.cancel.child_token();
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
        let result = tokio::select! {biased;
            _=worker.cancelled()=>{cancel.cancel();let _=check.await;return Err(Error::Unavailable);},
            _=tokio::time::sleep_until(deadline)=>{cancel.cancel();let _=check.await;return Err(Error::Unavailable);},
            result=&mut check=>result.map_err(|_|Error::Preflight)?,
        };
        self.authorize(|| Ok(())).map_err(|_| Error::Unavailable)?;
        Ok(result)
    }
    /// Caller holds publication gate and an engine-tracked lifetime through launch/cleanup.
    /// Postcommit cancellation returns the original cancelled Turn, never hides known creation.
    #[allow(clippy::too_many_arguments)]
    pub fn create_child(
        &self,
        store: &Store,
        lease: &cli_bridge::Lease,
        policy: Arc<Frozen>,
        resources: Arc<crate::runtime::cli_agent_host::ChildResources>,
        prepared: &Prepared,
        worker: &CancellationToken,
        deadline: Instant,
    ) -> Result<(Created, Option<Turn>), Error> {
        let request = prepared
            .orchestration_request()
            .map_err(|_| Error::Preflight)?;
        let mut state = self.registry.state.lock().unwrap();
        self.registry.owner(&state, self.grant.owner)?;
        if self.cancel.is_cancelled()
            || worker.is_cancelled()
            || Instant::now() >= deadline
            || !state
                .grants
                .get(&self.scope.session)
                .is_some_and(|g| Arc::ptr_eq(g, &self.grant))
            || !state
                .active
                .get(&self.scope.session)
                .is_some_and(|(id, _)| id == &self.id)
            || self.grant.child.is_some()
            || lease.scope() != &self.scope
            || !policy.matches(&self.scope, &self.grant.binding, policy.tools())
        {
            return Err(Error::Unavailable);
        }
        if state.grants.len() >= MAX_GRANTS {
            return Err(Error::Busy);
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
                if self.cancel.is_cancelled() || worker.is_cancelled() || Instant::now() >= deadline
                {
                    return Err(crate::store::StoreError::ExecutionConflict);
                }
                Store::create_cli_native_child_in(tx, lease, &policy, &session, &uuid, now_ms())
            })
            .map_err(|_| Error::Storage)?;
        if !created.created {
            return Ok((created, None));
        }
        let grant = Arc::new(Grant {
            continuations: Mutex::new(CancellationToken::new()),
            child_parent: Some(Arc::downgrade(&self.grant)),
            child_resources: Some(resources),
            child: Some(policy.clone()),
            owner: self.grant.owner,
            binding: created.binding.clone(),
            request,
            // Retain the original policy/resources for explicit continuation. Individual
            // inline turns still follow parent cancellation; the grant follows approval.
            cancel: self.grant.cancel.child_token(),
        });
        let id = uuid_v4();
        let cancel = grant.cancel.child_token();
        state.grants.insert(session.clone(), grant.clone());
        state.active.insert(session, (id.clone(), cancel.clone()));
        let turn = Turn {
            queued: None,
            goal: None,
            child_return: None,
            wake: None,
            wake_cancel: (policy.delivery() == Delivery::Inline).then(|| self.cancel.clone()),
            registry: self.registry.clone(),
            grant,
            id,
            cancel,
        };
        Ok((created, Some(turn)))
    }
}
pub(super) struct ChildReturn {
    pub parent: ServiceAuthority,
    pub data: crate::store::cli_agent_children::Continuation,
}
impl ServiceAuthority {
    /// Only the original connected parent can continue an idle, clean child. A saved
    /// policy/UUID is never deserialized into a grant after reconnect or restart.
    pub fn continue_child(
        &self,
        store: &Store,
        lease: Arc<cli_bridge::Lease>,
        call: &str,
        launch: &crate::tools::SubagentLaunch,
    ) -> Result<(Created, Turn), Error> {
        if launch.profile.is_some()
            || launch.context.is_some()
            || launch.model.is_some()
            || launch.task.trim().is_empty()
            || launch.task.len() > 64 * 1024
            || launch.task.contains('\0')
            || lease.scope() != &self.scope
            || self.grant.child.is_some()
        {
            return Err(Error::Identity);
        }
        let mut state = self.registry.state.lock().unwrap();
        self.registry.owner(&state, self.grant.owner)?;
        if self.cancel.is_cancelled()
            || !state
                .grants
                .get(&self.scope.session)
                .is_some_and(|g| Arc::ptr_eq(g, &self.grant))
            || !state
                .active
                .get(&self.scope.session)
                .is_some_and(|(id, _)| id == &self.id)
        {
            return Err(Error::Unavailable);
        }
        let task = store
            .read(|c| {
                Store::assert_cli_native_service_in(c, &lease, call, "subagent")?;
                Store::task_by_child(
                    c,
                    &self.scope.session,
                    launch
                        .child
                        .ok_or(crate::store::StoreError::ExecutionConflict)?,
                )?
                .ok_or(crate::store::StoreError::ExecutionConflict)
            })
            .map_err(|_| Error::Identity)?;
        let grant = state
            .grants
            .get(&task.child_session)
            .cloned()
            .ok_or(Error::Unavailable)?;
        let policy = grant.child.as_ref().ok_or(Error::Identity)?;
        if policy.service() != "subagent"
            || grant.cancel.is_cancelled()
            || !grant
                .child_parent
                .as_ref()
                .and_then(std::sync::Weak::upgrade)
                .is_some_and(|p| Arc::ptr_eq(&p, &self.grant))
            || task.status != "done"
            || matches!(task.delivery.as_str(), "wake" | "notify")
            || state.active.contains_key(&task.child_session)
        {
            return Err(Error::Busy);
        }
        store
            .check_cli_agent_ready(&grant.binding)
            .map_err(|_| Error::Identity)?;
        let data = crate::store::cli_agent_children::Continuation {
            lease,
            call: call.into(),
            child: task.child_session.clone(),
            text: launch.task.clone(),
            delivery: if launch.wait {
                "inline"
            } else if launch.wake {
                "wake"
            } else {
                "notify"
            }
            .into(),
            digest: policy.digest(),
        };
        store
            .read(|c| data.validate(c))
            .map_err(|_| Error::Identity)?;
        let id = uuid_v4();
        let cancel = grant.cancel.child_token();
        state
            .active
            .insert(task.child_session.clone(), (id.clone(), cancel.clone()));
        let created = Created {
            binding: grant.binding.clone(),
            task,
            created: false,
            revision: None,
        };
        Ok((
            created,
            Turn {
                queued: None,
                goal: None,
                child_return: Some(ChildReturn {
                    parent: self.clone(),
                    data,
                }),
                wake: None,
                wake_cancel: launch.wait.then(|| self.cancel.clone()),
                registry: self.registry.clone(),
                grant,
                id,
                cancel,
            },
        ))
    }
}
impl Turn {
    pub(in crate::runtime) fn child_resources(
        &self,
    ) -> Option<Arc<crate::runtime::cli_agent_host::ChildResources>> {
        self.grant.child_resources.clone()
    }
    pub fn child_policy_arc(&self) -> Option<Arc<Frozen>> {
        self.grant.child.clone()
    }
    pub fn child_policy(&self) -> Option<&Frozen> {
        self.grant.child.as_deref()
    }
    pub fn validate_inputs(&self, system: &str, tools: &[ToolSpec]) -> Result<(), Error> {
        if let Some(p) = self.child_policy()
            && (p.system() != system || p.tools() != tools)
        {
            return Err(Error::Identity);
        }
        Ok(())
    }
    pub(super) fn live_for_admission(
        &self,
        store: &Store,
        request: &str,
        generation: u64,
        operation: &str,
        text: &str,
    ) -> Result<(), Error> {
        let Some(policy) = self.child_policy() else {
            return live(store, self.binding());
        };
        if self.child_return.is_none() && self.wake.is_none() && policy.task() != text
            || store
                .cli_agent_binding(&self.binding().create.session)
                .map_err(|_| Error::Storage)?
                .as_ref()
                != Some(self.binding())
        {
            return Err(Error::Identity);
        }
        let snapshot = store
            .read(|c| Store::cli_native_child_policy_in(c, &self.binding().create.session))
            .map_err(|_| Error::Storage)?
            .ok_or(Error::Identity)?;
        if snapshot.0 != policy.digest() || snapshot.1 != policy.snapshot() {
            return Err(Error::Identity);
        }
        if let Some(wake) = &self.wake {
            if !matches!(wake.target(), cli_agent_turns::WakeTarget::Job(_))
                || request != wake.request()
            {
                return Err(Error::Identity);
            }
            // Exact generated input and original authenticated job call are revalidated
            // by Wake::validate_in in the admission transaction.
            return Ok(());
        }
        if let Some(returned) = &self.child_return {
            if returned.data.text != text || request != format!("child:{}", returned.data.call) {
                return Err(Error::Identity);
            }
            return Ok(());
        }
        // Initial child only. A saved completed child is not a continuation grant.
        let existing = store
            .lookup_cli_agent_turn(&cli_agent_turns::Input {
                images: &[],
                mode: pipkin_engine_protocol::InputMode::Prompt,
                session: &self.binding().create.session,
                request,
                generation,
                operation,
                binding: self.binding().digest,
                text,
            })
            .map_err(|_| Error::Storage)?;
        let count: i64 = store
            .read(|c| {
                Ok(c.query_row(
                    "SELECT COUNT(*) FROM cli_agent_turns WHERE session=?1",
                    [&self.binding().create.session],
                    |r| r.get(0),
                )?)
            })
            .map_err(|_| Error::Storage)?;
        if existing.is_none() && count != 0 {
            return Err(Error::Identity);
        }
        Ok(())
    }
}
