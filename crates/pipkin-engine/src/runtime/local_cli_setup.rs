//! Connection-scoped metadata approvals and opt-in, ephemeral execution grants.
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use pipkin_engine_protocol::{
    ErrorCode, Event, LocalCliMode, LocalCliModel, LocalCliSetupStage as Stage,
    LocalCliSetupTarget as Target, LocalCliSetupView as View, ProtoError, Reply,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot};
use tokio_util::sync::CancellationToken;

use super::Shared;
use crate::{
    execution::claude_cli::{policy, preflight},
    store::{CliBinding, Store, StoreError},
    util::{now_ms, uuid_v4},
};

const TTL: Duration = Duration::from_secs(300);
const FAILURE: &str = "Local CLI metadata verification failed; inspect and approve again.";
fn error(code: ErrorCode, text: &'static str) -> ProtoError {
    ProtoError::new(code, text)
}
fn missing() -> ProtoError {
    error(ErrorCode::NotFound, "Local CLI setup is unavailable.")
}
fn metadata_error(_: impl Sized) -> ProtoError {
    error(ErrorCode::Invalid, FAILURE)
}
fn storage_error(e: StoreError) -> ProtoError {
    let code = ProtoError::from(e).code;
    error(
        code,
        "Local CLI session metadata is unavailable or conflicts.",
    )
}
fn bounded(value: &str) -> Result<(), ProtoError> {
    if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(error(
            ErrorCode::Invalid,
            "Select a bounded path without control characters.",
        ));
    }
    Ok(())
}
pub(super) fn binding_mode(id: &str) -> LocalCliMode {
    match id {
        policy::POLICY_ID => LocalCliMode::ChatOnly,
        crate::store::EXTERNAL_PERMISSION_POLICY => LocalCliMode::RestrictedWrite,
        _ => LocalCliMode::Unsupported,
    }
}
fn launch_policy(mode: LocalCliMode) -> Result<preflight::LaunchPolicy, ProtoError> {
    match mode {
        LocalCliMode::ChatOnly => Ok(preflight::LaunchPolicy::ChatOnly),
        LocalCliMode::RestrictedWrite => Ok(preflight::LaunchPolicy::RestrictedWrite),
        LocalCliMode::Unsupported => Err(error(
            ErrorCode::Unsupported,
            "Unsupported CLI launch policy.",
        )),
    }
}
fn model(target: &Target) -> policy::ModelAlias {
    match target {
        Target::New { model, .. } | Target::Existing { model, .. } => match model {
            LocalCliModel::Haiku => policy::ModelAlias::Haiku,
            LocalCliModel::Sonnet => policy::ModelAlias::Sonnet,
            LocalCliModel::Opus => policy::ModelAlias::Opus,
        },
    }
}

// Deliberately no Debug/Serialize: the captured environment must never escape.
struct Ticket {
    owner: u64,
    expires: Instant,
    view: View,
    candidate: preflight::Candidate,
    environment: policy::Environment,
    cli_uuid: String,
    saved: Option<CliBinding>,
    cancel: CancellationToken,
    prepared: Option<Arc<preflight::Prepared>>,
    attempt: u64,
}
#[derive(Default)]
struct State {
    tickets: HashMap<String, Ticket>,
    // Registrations outlive revoked/expired tickets until process cleanup completes.
    pending: HashMap<String, u64>,
    closed: bool,
    grants: HashMap<String, Arc<Grant>>,
    turns: HashMap<String, (String, CancellationToken)>,
}

// Never persisted or serialized; Prepared is deliberately absent.
pub(super) struct Grant {
    setup: String,
    owner: u64,
    pub binding: CliBinding,
    candidate: preflight::Candidate,
    environment: policy::Environment,
    pub model: policy::ModelAlias,
    pub policy: preflight::LaunchPolicy,
    pub expires: Instant,
    pub cancel: CancellationToken,
}
pub(super) struct Turn {
    registry: Registry,
    pub session: String,
    id: String,
    pub grant: Arc<Grant>,
    pub cancel: CancellationToken,
}
impl Turn {
    pub fn owner(&self) -> u64 {
        self.grant.owner
    }
    pub fn request(&self) -> preflight::Request {
        preflight::Request {
            candidate: self.grant.candidate.clone(),
            environment: self.grant.environment.clone(),
            model: self.grant.model,
            trust_executable: true,
            trust_project: true,
        }
    }
}
impl Drop for Turn {
    fn drop(&mut self) {
        self.cancel.cancel();
        let mut state = self.registry.state.lock().unwrap();
        if state
            .turns
            .get(&self.session)
            .is_some_and(|(id, _)| id == &self.id)
        {
            state.turns.remove(&self.session);
        }
    }
}
impl State {
    fn prune(&mut self) {
        self.grants.retain(|_, grant| {
            if Instant::now() >= grant.expires || grant.cancel.is_cancelled() {
                grant.cancel.cancel();
                false
            } else {
                true
            }
        });
        self.tickets.retain(|_, ticket| {
            if Instant::now() >= ticket.expires {
                ticket.cancel.cancel();
                false
            } else {
                true
            }
        });
    }
    fn ticket(&mut self, owner: u64, setup: &str) -> Result<&mut Ticket, ProtoError> {
        self.prune();
        self.tickets
            .get_mut(setup)
            .filter(|t| t.owner == owner)
            .ok_or_else(missing)
    }
    fn capacity(&self, owner: u64) -> Result<(), ProtoError> {
        if self.closed {
            return Err(error(ErrorCode::ShuttingDown, "Engine is shutting down."));
        }
        let detached = self
            .pending
            .iter()
            .filter(|(id, _)| !self.tickets.contains_key(*id));
        let total = self.tickets.len() + detached.clone().count();
        let owned = self.tickets.values().filter(|t| t.owner == owner).count()
            + detached.filter(|(_, o)| **o == owner).count();
        if total >= 32 || owned >= 4 {
            return Err(error(ErrorCode::Busy, "Local CLI setup capacity reached."));
        }
        Ok(())
    }
}
#[derive(Clone)]
pub(super) struct Registry {
    state: Arc<Mutex<State>>,
    checks: Arc<Semaphore>,
}
impl Default for Registry {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(State::default())),
            checks: Arc::new(Semaphore::new(2)),
        }
    }
}
impl Registry {
    fn grant(
        &self,
        owner: u64,
        setup: &str,
        session: &str,
        binding: CliBinding,
    ) -> Result<Arc<Grant>, ProtoError> {
        let mut state = self.state.lock().unwrap();
        let ticket = state.ticket(owner, setup)?;
        if ticket.view.stage != Stage::MetadataVerified
            || ticket.cancel.is_cancelled()
            || !matches!(
                ticket.view.mode,
                LocalCliMode::ChatOnly | LocalCliMode::RestrictedWrite
            )
            || binding_mode(&binding.identity.launch_policy) != ticket.view.mode
        {
            return Err(missing());
        }
        let grant = Arc::new(Grant {
            setup: setup.into(),
            owner,
            binding,
            candidate: ticket.candidate.clone(),
            environment: ticket.environment.clone(),
            model: model(&ticket.view.target),
            policy: launch_policy(ticket.view.mode)?,
            expires: ticket.expires,
            cancel: ticket.cancel.child_token(),
        });
        if let Some(old) = state.grants.insert(session.into(), grant.clone()) {
            old.cancel.cancel();
        }
        Ok(grant)
    }
    fn fail_grant(&self, owner: u64, setup: &str, session: &str, grant: Option<&Arc<Grant>>) {
        let mut state = self.state.lock().unwrap();
        if let Some(grant) = grant {
            grant.cancel.cancel();
            if state
                .grants
                .get(session)
                .is_some_and(|g| Arc::ptr_eq(g, grant))
            {
                state.grants.remove(session);
            }
        }
        if let Some(ticket) = state.tickets.get_mut(setup)
            && ticket.owner == owner
            && ticket.view.stage == Stage::MetadataVerified
        {
            ticket.view.stage = Stage::Failed;
            ticket.view.reason = Some(FAILURE.into());
            ticket.prepared = None;
        }
    }
    pub(super) fn current_grant(&self, session: &str) -> Option<Arc<Grant>> {
        let state = self.state.lock().unwrap();
        state
            .grants
            .get(session)
            .filter(|grant| {
                !state.closed && !grant.cancel.is_cancelled() && Instant::now() < grant.expires
            })
            .cloned()
    }
    pub(super) fn owns_grant(&self, owner: u64, session: &str) -> bool {
        self.current_grant(session)
            .is_some_and(|grant| grant.owner == owner)
    }
    pub(super) fn begin_turn(&self, owner: u64, session: &str) -> Result<Turn, ProtoError> {
        let mut state = self.state.lock().unwrap();
        state.prune();
        if state.closed {
            return Err(missing());
        }
        // Check connection authority under the same lock as registration, before
        // exposing busy state or reserving any preflight/launch work.
        let grant = state
            .grants
            .get(session)
            .filter(|grant| grant.owner == owner)
            .cloned()
            .ok_or_else(missing)?;
        if state.turns.contains_key(session) || state.turns.len() >= 32 {
            return Err(error(ErrorCode::Busy, "Local CLI preflight is busy."));
        }
        let id = uuid_v4();
        let cancel = grant.cancel.child_token();
        state
            .turns
            .insert(session.into(), (id.clone(), cancel.clone()));
        Ok(Turn {
            registry: self.clone(),
            session: session.into(),
            id,
            grant,
            cancel,
        })
    }
    pub(super) fn stop_pending(&self, session: &str) -> bool {
        let state = self.state.lock().unwrap();
        if let Some((_, cancel)) = state.turns.get(session) {
            cancel.cancel();
            true
        } else {
            false
        }
    }
    // Revocation/Stop and durable admission share this short synchronous lease.
    // No await or subprocess work is permitted inside the closure.
    pub(super) fn with_admission<T>(
        &self,
        turn: Option<&Turn>,
        f: impl FnOnce() -> Result<T, ProtoError>,
    ) -> Result<T, ProtoError> {
        let state = self.state.lock().unwrap();
        if let Some(turn) = turn
            && (state.closed
                || turn.cancel.is_cancelled()
                || Instant::now() >= turn.grant.expires
                || !state
                    .grants
                    .get(&turn.session)
                    .is_some_and(|g| Arc::ptr_eq(g, &turn.grant)))
        {
            return Err(missing());
        }
        f()
    }
    pub(super) fn revoke_owner(&self, owner: u64) {
        let mut state = self.state.lock().unwrap();
        state.grants.retain(|_, g| {
            if g.owner == owner {
                g.cancel.cancel();
                false
            } else {
                true
            }
        });
        state.tickets.retain(|_, t| {
            if t.owner == owner {
                t.cancel.cancel();
                false
            } else {
                true
            }
        });
    }
    pub(super) fn shutdown(&self) {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        for (_, grant) in state.grants.drain() {
            grant.cancel.cancel();
        }
        for (_, cancel) in state.turns.values() {
            cancel.cancel();
        }
        for (_, ticket) in state.tickets.drain() {
            ticket.cancel.cancel();
        }
    }
    fn insert(&self, ticket: Ticket) -> Result<View, ProtoError> {
        let mut state = self.state.lock().unwrap();
        state.prune();
        state.capacity(ticket.owner)?;
        let view = ticket.view.clone();
        state.tickets.insert(view.setup.clone(), ticket);
        Ok(view)
    }
    fn status(&self, owner: u64, setup: &str) -> Result<View, ProtoError> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .ticket(owner, setup)?
            .view
            .clone())
    }
    fn revoke(&self, owner: u64, setup: &str) -> Result<(), ProtoError> {
        let mut state = self.state.lock().unwrap();
        state.ticket(owner, setup)?.cancel.cancel();
        state.tickets.remove(setup);
        state.grants.retain(|_, g| {
            if g.setup == setup {
                g.cancel.cancel();
                false
            } else {
                true
            }
        });
        Ok(())
    }
    fn begin(
        &self,
        owner: u64,
        setup: &str,
        executable: bool,
        project: bool,
    ) -> Result<Work, ProtoError> {
        let mut state = self.state.lock().unwrap();
        let ticket = state.ticket(owner, setup)?;
        if !executable || !project {
            return Err(error(
                ErrorCode::Invalid,
                "Executable and project trust are both required.",
            ));
        }
        if ticket.view.stage == Stage::Checking {
            return Err(error(
                ErrorCode::Busy,
                "Local CLI metadata check is already running.",
            ));
        }
        // A cancelled check still owns its registration/permit while its child is reaped.
        if self.checks.available_permits() == 0 {
            return Err(error(
                ErrorCode::Busy,
                "Local CLI metadata checks are busy.",
            ));
        }
        if state.pending.contains_key(setup) {
            return Err(error(
                ErrorCode::Busy,
                "Local CLI metadata check is still cleaning up.",
            ));
        }
        // Rechecking replaces trust, rather than keeping an older grant alive if
        // the new verification fails or is cancelled.
        state.grants.retain(|_, grant| {
            if grant.setup == setup {
                grant.cancel.cancel();
                false
            } else {
                true
            }
        });
        let permit = self
            .checks
            .clone()
            .try_acquire_owned()
            .map_err(|_| error(ErrorCode::Busy, "Local CLI metadata checks are busy."))?;
        let ticket = state.ticket(owner, setup)?;
        if ticket.cancel.is_cancelled() {
            return Err(missing());
        }
        ticket.attempt += 1;
        ticket.view.stage = Stage::Checking;
        ticket.view.reason = None;
        ticket.prepared = None;
        let cancel = ticket.cancel.child_token();
        let work = Work {
            request: preflight::Request {
                candidate: ticket.candidate.clone(),
                environment: ticket.environment.clone(),
                model: model(&ticket.view.target),
                trust_executable: true,
                trust_project: true,
            },
            policy: launch_policy(ticket.view.mode)?,
            uuid: ticket.cli_uuid.clone(),
            saved: ticket.saved.clone(),
            target: ticket.view.target.clone(),
            cancel: cancel.clone(),
            deadline: ticket.expires,
            guard: CheckGuard {
                registry: self.clone(),
                owner,
                setup: setup.into(),
                cancel,
                attempt: ticket.attempt,
                cleanup: true,
            },
            _permit: permit,
        };
        state.pending.insert(setup.into(), owner);
        Ok(work)
    }
    fn finish(
        &self,
        owner: u64,
        setup: &str,
        attempt: u64,
        cancel: &CancellationToken,
        result: Result<preflight::Prepared, ProtoError>,
    ) -> Result<View, ProtoError> {
        let mut state = self.state.lock().unwrap();
        let ticket = state.ticket(owner, setup)?;
        if cancel.is_cancelled()
            || ticket.cancel.is_cancelled()
            || ticket.attempt != attempt
            || ticket.view.stage != Stage::Checking
        {
            return Err(missing());
        }
        match result {
            Ok(prepared) => {
                // Existing-session verification is display-only, not a future turn cache.
                ticket.prepared =
                    matches!(ticket.view.target, Target::New { .. }).then(|| Arc::new(prepared));
                ticket.view.stage = Stage::MetadataVerified;
            }
            Err(_) => {
                ticket.view.stage = Stage::Failed;
                ticket.view.reason = Some(FAILURE.into());
            }
        }
        Ok(ticket.view.clone())
    }
}
struct Work {
    request: preflight::Request,
    policy: preflight::LaunchPolicy,
    uuid: String,
    saved: Option<CliBinding>,
    target: Target,
    cancel: CancellationToken,
    guard: CheckGuard,
    deadline: Instant,
    _permit: OwnedSemaphorePermit,
}
// The caller guard cancels on RPC drop. The worker guard additionally releases registration
// only AFTER preflight has reaped its process. Neither can leave a ticket Checking forever.
struct CheckGuard {
    registry: Registry,
    owner: u64,
    setup: String,
    cancel: CancellationToken,
    attempt: u64,
    cleanup: bool,
}
impl Drop for CheckGuard {
    fn drop(&mut self) {
        self.cancel.cancel();
        let mut state = self.registry.state.lock().unwrap();
        if let Some(ticket) = state.tickets.get_mut(&self.setup)
            && ticket.owner == self.owner
            && ticket.attempt == self.attempt
            && ticket.view.stage == Stage::Checking
        {
            ticket.view.stage = Stage::Failed;
            ticket.view.reason = Some(FAILURE.into());
        }
        if self.cleanup {
            state.pending.remove(&self.setup);
        }
    }
}

impl Shared {
    fn install_local_cli_grant(
        &self,
        owner: u64,
        setup: &str,
        session: &str,
        binding: CliBinding,
    ) -> Result<(), ProtoError> {
        if !self.local_cli_execution_enabled(binding_mode(&binding.identity.launch_policy)) {
            return Err(missing());
        }
        let mut installed = None;
        let result = (|| {
            // Capture the runtime before taking the trust registry. Never acquire
            // the session registry or publication while holding the trust registry.
            let sess = self.session(session)?;
            installed = Some(
                self.local_cli_setups
                    .grant(owner, setup, session, binding)?,
            );
            self.commit(&sess, |tx| {
                Store::bump(tx, session, now_ms())?;
                // No serialized Ready=true: grants belong to one connection. Empty
                // changes also prevent commit from bumping the revision a second time.
                Ok(((), Vec::new()))
            })?;
            Ok(())
        })();
        if result.is_err() {
            self.local_cli_setups
                .fail_grant(owner, setup, session, installed.as_ref());
        } else {
            self.publish(Event::SessionsChanged);
        }
        result
    }

    #[cfg(test)]
    pub(super) fn inspect_local_cli(
        &self,
        client: u64,
        executable: &str,
        target: Target,
    ) -> Result<Reply, ProtoError> {
        self.inspect_local_cli_mode(client, executable, target, LocalCliMode::ChatOnly)
    }

    pub(super) fn inspect_local_cli_mode(
        &self,
        client: u64,
        executable: &str,
        target: Target,
        mode: LocalCliMode,
    ) -> Result<Reply, ProtoError> {
        launch_policy(mode)?;
        bounded(executable)?;
        {
            let mut state = self.local_cli_setups.state.lock().unwrap();
            state.prune();
            state.capacity(client)?;
        }
        if self.stopping.is_cancelled() {
            return Err(error(ErrorCode::ShuttingDown, "Engine is shutting down."));
        }
        let saved = match &target {
            Target::New { project, .. } => {
                bounded(project)?;
                None
            }
            Target::Existing { session, .. } => {
                bounded(session)?;
                let row = self
                    .store
                    .session(session)
                    .map_err(storage_error)?
                    .ok_or_else(missing)?;
                let binding = self
                    .store
                    .cli_binding(session)
                    .map_err(storage_error)?
                    .ok_or_else(missing)?;
                if !binding.context_valid
                    || row.parent.is_some()
                    || row.root.is_some()
                    || executable != binding.identity.executable_path
                    || binding_mode(&binding.identity.launch_policy) != mode
                {
                    return Err(error(
                        ErrorCode::Conflict,
                        "Existing CLI binding is unavailable or does not match.",
                    ));
                }
                Some(binding)
            }
        };
        let project = match &target {
            Target::New { project, .. } => project.as_str(),
            Target::Existing { .. } => &saved.as_ref().unwrap().identity.project_path,
        };
        bounded(project)?;
        let mut environment: policy::Environment = if self.config.env.ambient {
            std::env::vars_os().collect()
        } else {
            Default::default()
        };
        environment.extend(
            self.config
                .env
                .vars
                .iter()
                .map(|(k, v)| (k.into(), v.into())),
        );
        let candidate =
            preflight::Candidate::inspect(Path::new(executable), Path::new(project), &environment)
                .map_err(metadata_error)?;
        let resolved_executable = candidate
            .executable()
            .to_str()
            .ok_or_else(|| metadata_error(()))?
            .to_owned();
        let resolved_project = candidate
            .project()
            .to_str()
            .ok_or_else(|| metadata_error(()))?
            .to_owned();
        bounded(&resolved_executable)?;
        bounded(&resolved_project)?;
        if let Some(saved) = &saved
            && (saved.identity.executable_path != resolved_executable
                || saved.identity.project_path != resolved_project)
        {
            return Err(error(
                ErrorCode::Conflict,
                "Existing CLI paths do not match.",
            ));
        }
        let cli_uuid = saved
            .as_ref()
            .map(|b| b.identity.cli_session_id.clone())
            .unwrap_or_else(uuid_v4);
        let ticket = Ticket {
            owner: client,
            expires: Instant::now() + TTL,
            view: View {
                setup: uuid_v4(),
                mode,
                target,
                selected_executable: executable.into(),
                executable: resolved_executable,
                project: resolved_project,
                qualified_version: policy::QUALIFIED_VERSION.into(),
                stage: Stage::AwaitingTrust,
                reason: None,
                execution_enabled: false,
            },
            candidate,
            environment,
            cli_uuid,
            saved,
            cancel: self.stopping.child_token(),
            prepared: None,
            attempt: 0,
        };
        Ok(Reply::LocalCliSetup(self.local_cli_setups.insert(ticket)?))
    }
    pub(super) async fn check_local_cli(
        self: &Arc<Self>,
        client: u64,
        setup: &str,
        trust_executable: bool,
        trust_project: bool,
    ) -> Result<Reply, ProtoError> {
        // Register before observing shutdown. The owned check must drain/reap even if
        // its RPC is cancelled; shutdown waits for it alongside inference actors.
        let completion = self.external_tasks.token();
        if self.stopping.is_cancelled() {
            return Err(error(ErrorCode::ShuttingDown, "Engine is shutting down."));
        }
        let work = self
            .local_cli_setups
            .begin(client, setup, trust_executable, trust_project)?;
        let caller = CheckGuard {
            registry: self.local_cli_setups.clone(),
            owner: client,
            setup: setup.into(),
            cancel: work.cancel.clone(),
            attempt: work.guard.attempt,
            cleanup: false,
        };
        let shared = self.clone();
        let setup = setup.to_owned();
        let (tx, rx) = oneshot::channel();
        self.spawn(async move {
            let _completion = completion;
            let Work {
                request,
                policy,
                uuid,
                saved,
                target,
                cancel,
                guard,
                deadline,
                _permit,
            } = work;
            let check = preflight::check_for_policy(request, cancel.clone(), policy);
            tokio::pin!(check);
            let checked = tokio::select! {
                result = &mut check => result,
                _ = tokio::time::sleep_until(deadline.into()) => {
                    cancel.cancel();
                    check.await
                }
            };
            let result = match checked {
                Ok(prepared) => (|| {
                    let binding = prepared.checked_binding(&uuid).map_err(metadata_error)?;
                    if let Some(saved) = saved {
                        let alias = model(&target);
                        let Target::Existing { session, .. } = target else {
                            return Err(metadata_error(()));
                        };
                        let current = shared
                            .store
                            .cli_binding(&session)
                            .map_err(storage_error)?
                            .ok_or_else(missing)?;
                        let row = shared
                            .store
                            .session(&session)
                            .map_err(storage_error)?
                            .ok_or_else(missing)?;
                        if !current.context_valid
                            || current != saved
                            || binding != saved.identity
                            || row.model.as_deref() != Some(alias.as_str())
                        {
                            return Err(metadata_error(()));
                        }
                    }
                    Ok(prepared)
                })(),
                Err(_) => Err(metadata_error(())),
            };
            let reply = shared
                .local_cli_setups
                .finish(client, &setup, guard.attempt, &cancel, result)
                .and_then(|view| {
                    if shared.local_cli_execution_enabled(view.mode)
                        && view.stage == Stage::MetadataVerified
                        && let Target::Existing { session, .. } = &view.target
                    {
                        let binding = shared
                            .store
                            .cli_binding(session)
                            .map_err(storage_error)?
                            .ok_or_else(missing)?;
                        shared.install_local_cli_grant(client, &setup, session, binding)?;
                    }
                    Ok(Reply::LocalCliSetup(view))
                });
            drop(guard);
            drop(_permit);
            let _ = tx.send(reply);
        });
        let result = rx.await.map_err(|_| metadata_error(()))?;
        drop(caller);
        result
    }
    pub(super) fn local_cli_setup_status(
        &self,
        client: u64,
        setup: &str,
    ) -> Result<Reply, ProtoError> {
        Ok(Reply::LocalCliSetup(
            self.local_cli_setups.status(client, setup)?,
        ))
    }
    pub(super) fn revoke_local_cli_setup(
        &self,
        client: u64,
        setup: &str,
    ) -> Result<Reply, ProtoError> {
        self.local_cli_setups.revoke(client, setup)?;
        Ok(Reply::Ok)
    }
    pub(super) fn create_local_cli_session(
        &self,
        client: u64,
        session: &str,
        setup: &str,
    ) -> Result<Reply, ProtoError> {
        // Lost acknowledgements survive ticket expiry, disconnect and engine restart.
        if let Some(row) = self
            .store
            .cli_creation(session, setup)
            .map_err(storage_error)?
        {
            return Ok(Reply::Session {
                summary: self.execution_summary(&row),
            });
        }
        let (prepared, uuid, alias) = {
            let mut state = self.local_cli_setups.state.lock().unwrap();
            let ticket = state.ticket(client, setup)?;
            if !matches!(ticket.view.target, Target::New { .. })
                || ticket.view.stage != Stage::MetadataVerified
                || ticket.cancel.is_cancelled()
            {
                return Err(metadata_error(()));
            }
            (
                ticket.prepared.clone().ok_or_else(|| metadata_error(()))?,
                ticket.cli_uuid.clone(),
                model(&ticket.view.target),
            )
        };
        // Revalidation performs filesystem work outside the registry lock.
        let binding = prepared.checked_binding(&uuid).map_err(metadata_error)?;
        let (row, created) = {
            // Serialize durable creation with acknowledged revocation/shutdown. Filesystem
            // checks and subprocesses remain outside this short commit critical section.
            let mut state = self.local_cli_setups.state.lock().unwrap();
            let ticket = state.ticket(client, setup)?;
            if ticket.cancel.is_cancelled()
                || ticket.view.stage != Stage::MetadataVerified
                || !ticket
                    .prepared
                    .as_ref()
                    .is_some_and(|p| Arc::ptr_eq(p, &prepared))
            {
                return Err(missing());
            }
            let result = self
                .store
                .create_cli_session_with_model(session, setup, &binding, alias.as_str(), now_ms())
                .map_err(storage_error)?;
            ticket.prepared = None;
            result
        };
        if created && self.local_cli_execution_enabled(binding_mode(&binding.launch_policy)) {
            let binding = self
                .store
                .cli_binding(session)
                .map_err(storage_error)?
                .ok_or_else(missing)?;
            self.install_local_cli_grant(client, setup, session, binding)?;
        } else if created {
            self.publish(Event::SessionsChanged);
        }
        Ok(Reply::Session {
            summary: self.execution_summary(&row),
        })
    }
}

#[cfg(test)]
#[path = "local_cli_setup_tests.rs"]
mod tests;
