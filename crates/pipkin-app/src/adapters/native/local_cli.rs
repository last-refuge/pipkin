//! Closed setup protocol, deliberately separate from NativeSession creation and prompt ops.
//! One bounded worker keeps metadata checks off the data loop; Revoke uses the control lane.
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use pipkin_core::local_cli as core;
use pipkin_engine_protocol::{self as proto, Call, ErrorCode, Reply};

use super::{
    CallError, Client, ConversationId, EventKind, LifecycleEvent, Live, Msg, OperationId, Worker,
    lock,
};

const CAPACITY: usize = 8;

/// Only the latest inspection can still race a core cancellation. Replacing this slot
/// does not revoke its returned ticket: it may already own a created conversation.
pub(super) struct Inspection {
    tuple: (ConversationId, u64, OperationId),
    ticket: Option<core::SetupId>,
}

pub(super) struct Pending {
    pub conn: u64,
    pub conversation: ConversationId,
    pub generation: u64,
    pub op: OperationId,
    pub request: core::LocalCliRequest,
}

pub(super) struct Completion {
    pub pending: Pending,
    pub result: Result<Reply, CallError>,
}

pub(super) struct Job {
    pub client: Weak<Client>,
    pub pending: Pending,
}

impl Job {
    pub fn run(self, timeout: Duration) -> Completion {
        let result = match self.client.upgrade() {
            Some(client) => client.call(call(&self.pending.request), timeout),
            None => Err(CallError::NotSent("The setup connection ended.".into())),
        };
        Completion {
            pending: self.pending,
            result,
        }
    }
}

pub(super) struct Lane {
    tx: Mutex<Option<mpsc::SyncSender<Job>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Lane {
    pub fn new(feedback: mpsc::Sender<Msg>, timeout: Duration) -> Self {
        let (tx, rx) = mpsc::sync_channel::<Job>(CAPACITY);
        let thread = thread::Builder::new()
            .name("native-cli-setup".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    if feedback
                        .send(Msg::LocalCliCompleted(Box::new(job.run(timeout))))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .expect("spawn bounded setup worker");
        Self {
            tx: Mutex::new(Some(tx)),
            thread: Mutex::new(Some(thread)),
        }
    }

    pub fn submit(&self, job: Job) -> Result<(), Box<Pending>> {
        match lock(&self.tx).as_ref() {
            Some(tx) => tx.try_send(job).map_err(|e| match e {
                mpsc::TrySendError::Full(job) | mpsc::TrySendError::Disconnected(job) => {
                    Box::new(job.pending)
                }
            }),
            None => Err(Box::new(job.pending)),
        }
    }

    pub fn close(&self) {
        lock(&self.tx).take();
        let mut thread = lock(&self.thread);
        if thread.as_ref().is_some_and(JoinHandle::is_finished) {
            let _ = thread.take().unwrap().join();
        }
    }
}
impl Drop for Lane {
    fn drop(&mut self) {
        self.close();
    }
}

fn model(model: core::CliModel) -> proto::LocalCliModel {
    match model {
        core::CliModel::Haiku => proto::LocalCliModel::Haiku,
        core::CliModel::Sonnet => proto::LocalCliModel::Sonnet,
        core::CliModel::Opus => proto::LocalCliModel::Opus,
    }
}
fn core_model(model: proto::LocalCliModel) -> core::CliModel {
    match model {
        proto::LocalCliModel::Haiku => core::CliModel::Haiku,
        proto::LocalCliModel::Sonnet => core::CliModel::Sonnet,
        proto::LocalCliModel::Opus => core::CliModel::Opus,
    }
}
fn launch_mode(mode: core::CliMode) -> proto::LocalCliMode {
    match mode {
        core::CliMode::ChatOnly => proto::LocalCliMode::ChatOnly,
        core::CliMode::RestrictedWrite => proto::LocalCliMode::RestrictedWrite,
        core::CliMode::Unsupported => proto::LocalCliMode::Unsupported,
    }
}
fn core_mode(mode: proto::LocalCliMode) -> core::CliMode {
    match mode {
        proto::LocalCliMode::ChatOnly => core::CliMode::ChatOnly,
        proto::LocalCliMode::RestrictedWrite => core::CliMode::RestrictedWrite,
        proto::LocalCliMode::Unsupported => core::CliMode::Unsupported,
    }
}
fn target(target: &core::LocalCliInspectTarget) -> proto::LocalCliSetupTarget {
    match target {
        core::LocalCliInspectTarget::New { project, model: m } => proto::LocalCliSetupTarget::New {
            project: project.clone(),
            model: model(*m),
        },
        core::LocalCliInspectTarget::Existing { session, model: m } => {
            proto::LocalCliSetupTarget::Existing {
                session: session.0.clone(),
                model: model(*m),
            }
        }
    }
}

pub(super) fn call(request: &core::LocalCliRequest) -> Call {
    use core::LocalCliRequest as R;
    match request {
        R::Inspect {
            mode,
            executable,
            target: t,
        } => Call::InspectLocalCli {
            mode: launch_mode(*mode),
            executable: executable.clone(),
            target: target(t),
        },
        R::Check {
            setup,
            trust_executable,
            trust_project,
        } => Call::CheckLocalCli {
            setup: setup.0.clone(),
            trust_executable: *trust_executable,
            trust_project: *trust_project,
        },
        R::Status { setup } => Call::LocalCliSetupStatus {
            setup: setup.0.clone(),
        },
        R::Revoke { setup } => Call::RevokeLocalCliSetup {
            setup: setup.0.clone(),
        },
        R::Create { identity } => Call::CreateLocalCliSession {
            session: identity.session.0.clone(),
            setup: identity.setup.0.clone(),
        },
        R::LookupCreation { identity } => Call::LookupLocalCliCreation {
            session: identity.session.0.clone(),
            setup: identity.setup.0.clone(),
        },
        R::SessionStatus { session } => Call::LocalCliSessionStatus {
            session: session.0.clone(),
        },
    }
}

pub(super) fn failed(phase: core::LocalCliPhase, error: CallError) -> core::LocalCliEvent {
    use core::{LocalCliFailure as F, LocalCliPhase as P};
    let failure = match error {
        CallError::NotSent(reason) => F::Rejected(reason),
        CallError::Lost(reason) => F::Unknown(reason),
        CallError::Refused(e) => match e.code {
            ErrorCode::Conflict => F::Conflict,
            ErrorCode::NotFound
                if matches!(phase, P::Check | P::Status | P::Revoke | P::Create) =>
            {
                F::Expired
            }
            ErrorCode::Cancelled => F::Unknown(e.message),
            _ => F::Rejected(e.message),
        },
    };
    core::LocalCliEvent::Failed { phase, failure }
}

fn valid_setup(setup: &str) -> bool {
    !setup.is_empty()
        && setup.len() <= 124
        && setup
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn view(view: proto::LocalCliSetupView) -> Result<core::LocalCliSetupView, CallError> {
    if !valid_setup(&view.setup)
        || view.executable.is_empty()
        || view.project.is_empty()
        || view.mode == proto::LocalCliMode::Unsupported
    {
        return Err(CallError::Lost(
            "Invalid setup identity or resolved paths.".into(),
        ));
    }
    let (target, root_candidate) = match view.target {
        proto::LocalCliSetupTarget::New { project, model } => (
            core::LocalCliInspectTarget::New {
                project,
                model: core_model(model),
            },
            Some(core::EngineSessionId(format!("cli-{}", view.setup))),
        ),
        proto::LocalCliSetupTarget::Existing { session, model } => (
            core::LocalCliInspectTarget::Existing {
                session: core::EngineSessionId(session),
                model: core_model(model),
            },
            None,
        ),
    };
    Ok(core::LocalCliSetupView {
        setup: core::SetupId(view.setup),
        mode: core_mode(view.mode),
        target,
        root_candidate,
        executable: view.executable,
        project: view.project,
        qualified_version: view.qualified_version,
        stage: match view.stage {
            proto::LocalCliSetupStage::AwaitingTrust => core::LocalCliMetadataStage::AwaitingTrust,
            proto::LocalCliSetupStage::Checking => core::LocalCliMetadataStage::Checking,
            proto::LocalCliSetupStage::MetadataVerified => {
                core::LocalCliMetadataStage::MetadataVerified
            }
            proto::LocalCliSetupStage::Failed => core::LocalCliMetadataStage::Failed,
        },
        expires_at: None,
        reason: view.reason,
    })
}

impl Live {
    pub(super) fn retire_local_cli_inspect(
        &mut self,
        worker: &Worker,
        conversation: ConversationId,
        generation: u64,
        op: OperationId,
    ) {
        let tuple = (conversation, generation, op);
        if self
            .local_cli_inspection
            .as_ref()
            .is_some_and(|i| i.tuple == tuple)
        {
            let inspection = self.local_cli_inspection.take().unwrap();
            if let Some(ticket) = inspection.ticket {
                self.cleanup_inspection(worker.conn, tuple, ticket);
            }
            // A pending completion will see that its tuple is no longer current and
            // clean up its own ticket. Retirement has no receipt or protocol call.
        }
    }

    fn cleanup_inspection(
        &self,
        conn: u64,
        (conversation, generation, op): (ConversationId, u64, OperationId),
        setup: core::SetupId,
    ) {
        self.control.revoke_setup(Job {
            client: Arc::downgrade(&self.client),
            pending: Pending {
                conn,
                conversation,
                generation,
                op,
                request: core::LocalCliRequest::Revoke { setup },
            },
        });
    }

    pub(super) fn local_cli_request(
        &mut self,
        worker: &mut Worker,
        conversation: ConversationId,
        generation: u64,
        op: OperationId,
        request: core::LocalCliRequest,
    ) {
        let pending = Pending {
            conn: worker.conn,
            conversation,
            generation,
            op,
            request,
        };
        if matches!(pending.request, core::LocalCliRequest::Inspect { .. }) {
            self.local_cli_inspection = Some(Inspection {
                tuple: (conversation, generation, op),
                ticket: None,
            });
        }
        let error = if !self.local_cli_supported {
            Some("The engine does not advertise Local CLI setup.")
        } else if matches!(&pending.request, core::LocalCliRequest::Create { identity } | core::LocalCliRequest::LookupCreation { identity } if !valid_setup(&identity.setup.0) || identity.session.0 != format!("cli-{}", identity.setup.0))
        {
            Some("Invalid frozen Local CLI creation identity.")
        } else if !self.local_cli_source_live(worker, &pending) {
            Some("The captured Local CLI conversation is no longer attached.")
        } else {
            None
        };
        if let Some(error) = error {
            self.local_cli_complete(
                worker,
                Completion {
                    pending,
                    result: Err(CallError::NotSent(error.into())),
                },
            );
            return;
        }
        if matches!(&pending.request,
            core::LocalCliRequest::Create { identity } | core::LocalCliRequest::LookupCreation { identity }
                if self.local_cli_inspection.as_ref().and_then(|i| i.ticket.as_ref()) == Some(&identity.setup))
        {
            // Creation (including an unknown outcome) is never Inspect cancellation.
            self.local_cli_inspection = None;
        }
        if let Err(pending) = worker.setup_lane.submit(Job {
            client: Arc::downgrade(&self.client),
            pending,
        }) {
            self.local_cli_complete(
                worker,
                Completion {
                    pending: *pending,
                    result: Err(CallError::NotSent(
                        "The bounded setup lane is busy or closed.".into(),
                    )),
                },
            );
        }
    }

    fn local_cli_source_live(&self, worker: &Worker, p: &Pending) -> bool {
        if matches!(
            p.request,
            core::LocalCliRequest::Revoke { .. } | core::LocalCliRequest::LookupCreation { .. }
        ) {
            return true;
        }
        if p.conversation == core::LOCAL_CLI_GLOBAL {
            return !matches!(
                p.request,
                core::LocalCliRequest::Inspect {
                    target: core::LocalCliInspectTarget::Existing { .. },
                    ..
                } | core::LocalCliRequest::SessionStatus { .. }
            );
        }
        if worker.attachment_generations.get(&p.conversation) != Some(&p.generation) {
            return false;
        }
        let Some(summary) = self
            .session_of(p.conversation)
            .filter(|s| s.execution_backend == proto::ExecutionBackend::LocalClaudeCli)
        else {
            return false;
        };
        match &p.request {
            core::LocalCliRequest::Inspect {
                target: core::LocalCliInspectTarget::Existing { session, .. },
                ..
            }
            | core::LocalCliRequest::SessionStatus { session } => summary.id == session.0,
            core::LocalCliRequest::Inspect { .. } | core::LocalCliRequest::Create { .. } => false,
            _ => true,
        }
    }

    pub(super) fn local_cli_complete(&mut self, worker: &mut Worker, completion: Completion) {
        let Completion { pending: p, result } = completion;
        if p.conn != worker.conn {
            return;
        }
        let tuple = (p.conversation, p.generation, p.op);
        let inspecting = matches!(p.request, core::LocalCliRequest::Inspect { .. });
        let inspection_current = self
            .local_cli_inspection
            .as_ref()
            .is_some_and(|i| i.tuple == tuple);
        let source_live = self.local_cli_source_live(worker, &p);
        if inspecting && !inspection_current {
            if let Ok(Reply::LocalCliSetup(v)) = result {
                self.cleanup_inspection(p.conn, tuple, core::SetupId(v.setup));
            }
            return;
        }
        if inspecting && !source_live {
            if let Ok(Reply::LocalCliSetup(v)) = &result {
                self.cleanup_inspection(p.conn, tuple, core::SetupId(v.setup.clone()));
            }
            self.local_cli_inspection = None;
        }
        let result = if source_live {
            result
        } else {
            Err(CallError::NotSent(
                "The captured Local CLI source generation changed.".into(),
            ))
        };
        let inspected_ticket = match (&p.request, &result) {
            (core::LocalCliRequest::Inspect { .. }, Ok(Reply::LocalCliSetup(v))) => {
                Some(v.setup.clone())
            }
            _ => None,
        };
        let event = self
            .local_cli_reply(worker, &p.request, result)
            .unwrap_or_else(|e| failed(p.request.phase(), e));
        if let Some(setup) = inspected_ticket {
            if matches!(event, core::LocalCliEvent::Inspected(_)) {
                if let Some(inspection) = &mut self.local_cli_inspection {
                    inspection.ticket = Some(core::SetupId(setup));
                }
            } else {
                self.local_cli_inspection = None;
                self.cleanup_inspection(p.conn, tuple, core::SetupId(setup));
            }
        }
        // Check metadata is not a grant. Re-read server readiness even after a lost
        // mutation reply; never submit a prompt or switch to a different conversation.
        if matches!(
            p.request,
            core::LocalCliRequest::Check { .. } | core::LocalCliRequest::Revoke { .. }
        ) {
            if let Err(error) = self.refresh_sessions() {
                log::warn!("could not refresh Local CLI catalog: {error}");
            }
            if self
                .current
                .as_ref()
                .is_some_and(|c| c.conversation == p.conversation && c.generation == p.generation)
            {
                self.open(worker, p.conversation, p.generation, false);
            }
            (worker.sink)(LifecycleEvent::Catalog(self.catalog(worker)));
        }
        worker.emit_op(
            p.conversation,
            p.generation,
            p.op,
            EventKind::LocalCli(event),
        );
    }

    fn local_cli_reply(
        &mut self,
        worker: &Worker,
        request: &core::LocalCliRequest,
        result: Result<Reply, CallError>,
    ) -> Result<core::LocalCliEvent, CallError> {
        use core::{LocalCliEvent as E, LocalCliRequest as R};
        match (request, result?) {
            (
                R::Inspect {
                    mode,
                    executable,
                    target: t,
                },
                Reply::LocalCliSetup(v),
            ) if v.selected_executable == *executable
                && v.mode == launch_mode(*mode)
                && v.target == target(t)
                && v.stage == proto::LocalCliSetupStage::AwaitingTrust =>
            {
                Ok(E::Inspected(view(v)?))
            }
            (R::Check { setup, .. }, Reply::LocalCliSetup(v)) if setup.0 == v.setup => {
                Ok(E::Checked(view(v)?))
            }
            (R::Status { setup }, Reply::LocalCliSetup(v)) if setup.0 == v.setup => {
                Ok(E::Status(view(v)?))
            }
            (R::Revoke { .. }, Reply::Ok) => Ok(E::Revoked),
            (R::Create { identity }, Reply::Session { summary }) => Ok(E::Created(
                self.local_cli_created(worker, identity, summary)?,
            )),
            (R::LookupCreation { identity }, Reply::LocalCliCreation { summary }) => {
                Ok(E::CreationLookup {
                    identity: identity.clone(),
                    found: summary
                        .map(|s| self.local_cli_created(worker, identity, s))
                        .transpose()?,
                })
            }
            (R::SessionStatus { session }, Reply::LocalCliSession(v))
                if session.0 == v.summary.id
                    && v.summary.execution_backend == proto::ExecutionBackend::LocalClaudeCli =>
            {
                Ok(E::SessionStatus(core::LocalCliSessionView {
                    session: session.clone(),
                    mode: core_mode(v.mode),
                    executable: v.executable,
                    project: v.summary.cwd,
                    model: v.model.map(core_model),
                    context_valid: v.context_valid,
                }))
            }
            _ => Err(CallError::Lost(
                "Unexpected Local CLI protocol reply; no retry was attempted.".into(),
            )),
        }
    }

    fn local_cli_created(
        &mut self,
        worker: &Worker,
        identity: &core::LocalCliCreationId,
        summary: proto::SessionSummary,
    ) -> Result<core::LocalCliCreated, CallError> {
        if summary.id != identity.session.0
            || summary.execution_backend != proto::ExecutionBackend::LocalClaudeCli
        {
            return Err(CallError::Refused(proto::ProtoError::new(
                ErrorCode::Conflict,
                "Creation returned a different execution identity.",
            )));
        }
        // Re-read the entire catalog through its collision resolver. Never select the newest row
        // or derive an ID independently of the mapper. Failure here leaves creation uncertain.
        self.refresh_sessions()?;
        let conversation = self
            .conversation_of(&identity.session.0)
            .filter(|id| *id != core::LOCAL_CLI_GLOBAL)
            .ok_or_else(|| {
                CallError::Lost("Created identity is not present in the catalog.".into())
            })?;
        if self
            .session_of(conversation)
            .is_none_or(|s| s.execution_backend != proto::ExecutionBackend::LocalClaudeCli)
        {
            return Err(CallError::Refused(proto::ProtoError::new(
                ErrorCode::Conflict,
                "Catalog execution identity conflicts with creation.",
            )));
        }
        (worker.sink)(LifecycleEvent::Catalog(self.catalog(worker)));
        Ok(core::LocalCliCreated {
            identity: identity.clone(),
            conversation,
        })
    }
}
