//! Explicit desktop setup, independent of prompt/run state and engine protocol crates.
//! See `LOCAL_CLI_API.md` for the adapter contract and uncertain-mutation rules.
use crate::{
    AppState, BackendEvent, BackendRequest, Command, ConversationId, Effect, EventKind, Mode, Note,
    OperationId, Outcome,
};

mod assistance;
mod guided;
mod recovery;

/// Reserved for setup that does not yet have a desktop conversation.
pub const LOCAL_CLI_GLOBAL: ConversationId = ConversationId(0);
/// No eviction: a full recovery list blocks parking another unresolved identity.
pub const LOCAL_CLI_RECOVERY_CAPACITY: usize = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CliMode {
    Orchestration,
    #[default]
    Coding,
    ChatOnly,
    RestrictedWrite,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CliModel {
    Haiku,
    #[default]
    Sonnet,
    Opus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineSessionId(pub String);
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupId(pub String);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalCliDraft {
    pub mode: CliMode,
    pub executable: String,
    pub project: String,
    pub model: CliModel,
    pub thinking: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalCliTarget {
    New,
    Existing {
        conversation: ConversationId,
        generation: u64,
        session: EngineSessionId,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LocalCliStage {
    #[default]
    Editing,
    Inspecting,
    AwaitingTrust,
    Checking,
    CheckUnknown,
    Verified,
    Creating,
    CreateUnknown,
    Reconciling,
    Created,
    Failed,
}

/// Engine metadata stages; MetadataVerified is never an execution grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalCliMetadataStage {
    AwaitingTrust,
    Checking,
    MetadataVerified,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalCliInspectTarget {
    New {
        project: String,
        model: CliModel,
        thinking: Option<String>,
    },
    Existing {
        session: EngineSessionId,
        model: CliModel,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalCliSetupView {
    pub setup: SetupId,
    pub mode: CliMode,
    pub target: LocalCliInspectTarget,
    /// New setup: `cli-` plus the engine's globally unique setup UUID. Frozen at Inspect.
    pub root_candidate: Option<EngineSessionId>,
    pub executable: String,
    pub project: String,
    /// Minimum supported version; the observed executable version belongs to its binding.
    pub qualified_version: String,
    pub stage: LocalCliMetadataStage,
    /// Unix seconds, on AppState's injected clock. None means backend-enforced expiry only.
    pub expires_at: Option<i64>,
    pub reason: Option<String>,
}

/// Stable pair used for every Create and pure lookup, including after ticket invalidation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalCliCreationId {
    pub session: EngineSessionId,
    pub setup: SetupId,
}

/// Adapter must resolve the exact engine ID to its collision-checked desktop catalog ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalCliCreated {
    pub identity: LocalCliCreationId,
    pub conversation: ConversationId,
}

/// Read-only saved-session hints. They cannot check trust boxes or verify a ticket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalCliSessionView {
    pub session: EngineSessionId,
    pub mode: CliMode,
    pub executable: String,
    pub project: String,
    pub model: Option<CliModel>,
    pub context_valid: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalCliCommand {
    BeginNew(LocalCliDraft),
    /// Guided setup only discovers filesystem metadata until explicit confirmation.
    BeginGuided,
    /// Create from an explicit dashboard draft without changing the main provider or project.
    BeginDashboardNew(LocalCliDraft),
    /// Configure the provider without selecting a model or creating a conversation.
    BeginProviderSetup,
    BeginGuidedExisting {
        conversation: ConversationId,
        session: EngineSessionId,
    },
    Discover,
    Configure(LocalCliDraft),
    /// The visible captured ticket, not whatever ticket happens to arrive later.
    ConfirmAndConnect {
        setup: SetupId,
    },
    BeginExisting {
        conversation: ConversationId,
        session: EngineSessionId,
    },
    Edit(LocalCliDraft),
    TrustExecutable(bool),
    TrustProject(bool),
    Inspect,
    Check,
    Status,
    Revoke,
    Create,
    LookupCreation,
    /// Only after an explicit lookup returned None; never a fresh identity.
    RepeatCreate,
    /// Explicitly retain an absent-at-lookup identity and start a separate setup.
    ParkCreationAndBeginNew(LocalCliDraft),
    /// Activate a retained pair for read-only lookup, without restoring approval.
    RecoverCreation(LocalCliCreationId),
    SessionStatus,
    /// Copy only closed, redacted state facts and static recovery guidance.
    CopyDiagnostics {
        conversation: ConversationId,
        generation: u64,
    },
    /// Start a separate setup using captured metadata hints, never source context/history.
    FreshSetup(LocalCliTarget),
    /// Re-open/refresh the captured existing conversation after metadata verification.
    ReturnToConversation,
    Cancel,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalCliRequest {
    Discover {
        saved: Option<String>,
        project: String,
    },
    Inspect {
        mode: CliMode,
        executable: String,
        target: LocalCliInspectTarget,
    },
    Check {
        setup: SetupId,
        trust_executable: bool,
        trust_project: bool,
    },
    Status {
        setup: SetupId,
    },
    Revoke {
        setup: SetupId,
    },
    Create {
        identity: LocalCliCreationId,
    },
    LookupCreation {
        identity: LocalCliCreationId,
    },
    SessionStatus {
        session: EngineSessionId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalCliPhase {
    Discover,
    Inspect,
    Check,
    Status,
    Revoke,
    Create,
    LookupCreation,
    SessionStatus,
}

impl LocalCliRequest {
    pub fn phase(&self) -> LocalCliPhase {
        match self {
            Self::Discover { .. } => LocalCliPhase::Discover,
            Self::Inspect { .. } => LocalCliPhase::Inspect,
            Self::Check { .. } => LocalCliPhase::Check,
            Self::Status { .. } => LocalCliPhase::Status,
            Self::Revoke { .. } => LocalCliPhase::Revoke,
            Self::Create { .. } => LocalCliPhase::Create,
            Self::LookupCreation { .. } => LocalCliPhase::LookupCreation,
            Self::SessionStatus { .. } => LocalCliPhase::SessionStatus,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalCliFailure {
    /// Transport loss/timeout/cancellation: do not infer that a mutation did not happen.
    Unknown(String),
    Rejected(String),
    Expired,
    Conflict,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalCliEvent {
    Discovered(Vec<String>),
    Inspected(LocalCliSetupView),
    Checked(LocalCliSetupView),
    Status(LocalCliSetupView),
    Revoked,
    Created(LocalCliCreated),
    CreationLookup {
        identity: LocalCliCreationId,
        found: Option<LocalCliCreated>,
    },
    SessionStatus(LocalCliSessionView),
    Failed {
        phase: LocalCliPhase,
        failure: LocalCliFailure,
    },
}
impl LocalCliEvent {
    pub fn phase(&self) -> LocalCliPhase {
        match self {
            Self::Discovered(_) => LocalCliPhase::Discover,
            Self::Inspected(_) => LocalCliPhase::Inspect,
            Self::Checked(_) => LocalCliPhase::Check,
            Self::Status(_) => LocalCliPhase::Status,
            Self::Revoked => LocalCliPhase::Revoke,
            Self::Created(_) => LocalCliPhase::Create,
            Self::CreationLookup { .. } => LocalCliPhase::LookupCreation,
            Self::SessionStatus(_) => LocalCliPhase::SessionStatus,
            Self::Failed { phase, .. } => *phase,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Pending {
    persisting: bool,
    conversation: ConversationId,
    generation: u64,
    op: OperationId,
    phase: LocalCliPhase,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalCliState {
    pub guided: bool,
    pub dashboard_creation: bool,
    pub provider_only: bool,
    pub(crate) provider_origin: Option<(ConversationId, crate::ProviderModel)>,
    pub candidates: Vec<String>,
    pub discovering: bool,
    /// One-shot consent for this exact pending Check only. Never serialized.
    connect_after_check: bool,
    /// Adapter capability, false until explicitly advertised. Not a profile/readiness grant.
    pub available: bool,
    /// An unreadable recovery journal must never be treated as an empty journal.
    pub recovery_unavailable: bool,
    pub target: Option<LocalCliTarget>,
    pub draft: LocalCliDraft,
    pub stage: LocalCliStage,
    pub trust_executable: bool,
    pub trust_project: bool,
    pub ticket: Option<LocalCliSetupView>,
    pub session_status: Option<LocalCliSessionView>,
    /// Never forgotten by Cancel, edit, disconnect, or a failed/absent lookup.
    /// Explicit parking moves this identity into `parked_creations`.
    pub creation: Option<LocalCliCreationId>,
    pub created: Option<LocalCliCreated>,
    pub lookup_absent: bool,
    pub error: Option<LocalCliFailure>,
    parked_creations: Vec<LocalCliCreationId>,
    generation: u64,
    next_op: u64,
    pending: Option<Pending>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocalCliAvailability {
    pub begin: bool,
    pub edit: bool,
    pub trust: bool,
    pub inspect: bool,
    pub check: bool,
    pub status: bool,
    pub revoke: bool,
    pub create: bool,
    pub lookup_creation: bool,
    pub repeat_create: bool,
    pub park_creation: bool,
    /// Applies to each identity in `LocalCliState::parked_creations()`.
    pub recover_creation: bool,
    pub session_status: bool,
    pub fresh_setup: bool,
    pub return_to_conversation: bool,
    pub cancel: bool,
}

impl LocalCliState {
    /// Captures asynchronous file-picker/discovery editing lifetime, including Existing targets.
    pub fn input_revision(&self) -> u64 {
        self.generation
    }

    /// Bounded recovery identities. Entries remain here while being recovered,
    /// until an exact completion receipt and its catalog entry have reconciled.
    pub fn parked_creations(&self) -> &[LocalCliCreationId] {
        &self.parked_creations
    }

    pub(crate) fn setup_busy(&self) -> bool {
        self.pending.is_some() || self.unresolved()
    }

    fn unresolved(&self) -> bool {
        self.creation.is_some() && self.stage != LocalCliStage::Created
    }

    pub(crate) fn reconnect_unsettled(&self, id: ConversationId) -> bool {
        matches!(&self.target, Some(LocalCliTarget::Existing { conversation, .. }) if *conversation == id)
            && (self.pending.is_some() || self.stage == LocalCliStage::CheckUnknown)
    }

    fn invalidate(&mut self) {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("setup generation exhausted");
        self.pending = None;
        self.discovering = false;
        self.connect_after_check = false;
        self.ticket = None;
        self.trust_executable = false;
        self.trust_project = false;
        self.lookup_absent = false;
        if self.unresolved() {
            self.stage = if self.created.is_some() {
                LocalCliStage::Reconciling
            } else {
                LocalCliStage::CreateUnknown
            };
        } else if self.stage != LocalCliStage::Created {
            self.stage = LocalCliStage::Editing;
        }
    }

    fn route(&self) -> (ConversationId, u64) {
        match &self.target {
            Some(LocalCliTarget::Existing {
                conversation,
                generation,
                ..
            }) => (*conversation, *generation),
            _ => (LOCAL_CLI_GLOBAL, self.generation),
        }
    }
}

impl AppState {
    fn local_cli_target_live(&self) -> bool {
        self.local_cli_target_allowed(false)
    }

    // Revocation cleans up an exact ticket, not an attachment execution grant.
    // Ownership must still match, but opening that same conversation must not strand it.
    fn local_cli_target_allowed(&self, cleanup: bool) -> bool {
        match &self.local_cli.target {
            Some(LocalCliTarget::New) => true,
            Some(LocalCliTarget::Existing {
                conversation,
                generation,
                ..
            }) => {
                self.conversation(*conversation).is_some_and(|c| {
                    cleanup
                        || (c.generation == *generation
                            && c.execution_profile(self.mode).is_local_cli())
                }) && !self.owned_elsewhere(*conversation)
            }
            None => false,
        }
    }

    /// Shared by dispatch, UI buttons, keyboard actions and palette. No probes or I/O.
    pub fn set_local_cli_available(&mut self, available: bool) -> Outcome {
        if self.local_cli.available == available {
            return Outcome::default();
        }
        self.local_cli.available = available;
        if !available {
            self.local_cli_disconnected();
        }
        Outcome {
            notes: vec![Note::Other],
            ..Outcome::default()
        }
    }

    pub fn local_cli_availability(&self) -> LocalCliAvailability {
        let s = &self.local_cli;
        let connected = s.available && self.mode == Mode::Real && self.connection.is_ready();
        let writable = connected && self.read_only.is_none();
        let live = self.local_cli_target_live();
        let free = s.pending.is_none();
        let editable = !s.unresolved();
        let ticket = s
            .ticket
            .as_ref()
            .is_some_and(|t| t.expires_at.is_none_or(|at| self.now() < at));
        let verified = ticket
            && s.ticket
                .as_ref()
                .is_some_and(|t| t.stage == LocalCliMetadataStage::MetadataVerified);
        LocalCliAvailability {
            begin: writable && editable && !self.pending_native_creation(),
            edit: s.target.is_some() && editable,
            trust: writable && live && free && ticket && s.stage == LocalCliStage::AwaitingTrust,
            inspect: writable
                && s.draft.mode != CliMode::Unsupported
                && live
                && editable
                && (free
                    || s.pending
                        .as_ref()
                        .is_some_and(|p| p.phase == LocalCliPhase::Revoke))
                && matches!(s.stage, LocalCliStage::Editing | LocalCliStage::Failed),
            check: writable
                && live
                && free
                && ticket
                && s.stage == LocalCliStage::AwaitingTrust
                && s.trust_executable
                && s.trust_project,
            status: connected
                && live
                && free
                && ticket
                && matches!(
                    s.stage,
                    LocalCliStage::CheckUnknown | LocalCliStage::Checking
                ),
            revoke: connected && self.local_cli_target_allowed(true) && ticket,
            create: writable
                && !s.provider_only
                && !s.recovery_unavailable
                && live
                && free
                && verified
                && s.stage == LocalCliStage::Verified
                && s.creation.is_none()
                && matches!(s.target, Some(LocalCliTarget::New)),
            lookup_creation: connected
                && live
                && free
                && s.creation.is_some()
                && s.created.is_none()
                && s.stage == LocalCliStage::CreateUnknown,
            repeat_create: writable
                && !s.recovery_unavailable
                && live
                && free
                && verified
                && s.lookup_absent
                && s.stage == LocalCliStage::CreateUnknown
                && s.created.is_none(),
            park_creation: writable
                && live
                && free
                && s.lookup_absent
                && s.stage == LocalCliStage::CreateUnknown
                && s.created.is_none()
                && s.creation.as_ref().is_some_and(|identity| {
                    s.parked_creations.len() < LOCAL_CLI_RECOVERY_CAPACITY
                        || s.parked_creations.contains(identity)
                }),
            recover_creation: connected && free && !s.parked_creations.is_empty(),
            session_status: connected
                && live
                && free
                && editable
                && matches!(s.target, Some(LocalCliTarget::Existing { .. })),
            fresh_setup: writable
                && live
                && free
                && editable
                && self.local_cli_fresh_hints().is_some(),
            return_to_conversation: connected
                && live
                && free
                && ticket
                && s.stage == LocalCliStage::Verified
                && matches!(s.target, Some(LocalCliTarget::Existing { .. })),
            cancel: s.target.is_some(),
        }
    }

    fn local_cli_send(&mut self, request: LocalCliRequest, out: &mut Outcome) {
        let s = &mut self.local_cli;
        s.next_op = s.next_op.checked_add(1).expect("setup operation exhausted");
        let (conversation, generation) = s.route();
        let op = OperationId(s.next_op);
        s.pending = Some(Pending {
            persisting: matches!(&request, LocalCliRequest::Create { .. }),
            conversation,
            generation,
            op,
            phase: request.phase(),
        });
        if let LocalCliRequest::Create { identity } = request {
            out.effects.push(Effect::JournalLocalCliCreation {
                conversation,
                generation,
                op,
                identity,
            });
        } else {
            out.effects.push(Effect::Backend(BackendRequest::LocalCli {
                conversation,
                generation,
                op,
                request,
            }));
        }
    }

    pub(crate) fn dispatch_local_cli(&mut self, command: LocalCliCommand) -> Outcome {
        use LocalCliCommand as C;
        if let Some(out) = self.dispatch_guided_cli(&command) {
            return out;
        }
        let mut out = Outcome::default();
        let a = self.local_cli_availability();
        match command {
            C::BeginNew(draft) if a.begin => {
                self.local_cli.dashboard_creation = false;
                self.local_cli.guided = false;
                self.local_cli.provider_only = false;
                self.local_cli.provider_origin = None;
                self.local_cli.invalidate();
                self.local_cli.stage = LocalCliStage::Editing;
                self.local_cli.target = Some(LocalCliTarget::New);
                self.local_cli.draft = draft;
                self.local_cli.creation = None;
                self.local_cli.created = None;
                self.local_cli.session_status = None;
                self.local_cli.error = None;
            }
            C::BeginExisting {
                conversation,
                session,
            } if a.begin && conversation != LOCAL_CLI_GLOBAL => {
                let Some(c) = self.conversation(conversation) else {
                    return out;
                };
                if !c.execution_profile(self.mode).is_local_cli()
                    || self.owned_elsewhere(conversation)
                {
                    return out;
                }
                let generation = c.generation;
                self.local_cli.dashboard_creation = false;
                self.local_cli.guided = false;
                self.local_cli.provider_only = false;
                self.local_cli.provider_origin = None;
                self.local_cli.invalidate();
                self.local_cli.stage = LocalCliStage::Editing;
                self.local_cli.target = Some(LocalCliTarget::Existing {
                    conversation,
                    generation,
                    session,
                });
                self.local_cli.draft = LocalCliDraft::default();
                self.local_cli.creation = None;
                self.local_cli.created = None;
                self.local_cli.session_status = None;
                self.local_cli.error = None;
            }
            C::CopyDiagnostics {
                conversation,
                generation,
            } if self.mode == Mode::Real
                && self.local_cli.target.is_some()
                && self.local_cli.route() == (conversation, generation) =>
            {
                out.effects
                    .push(Effect::CopyText(self.local_cli_diagnostic_report()));
            }
            C::FreshSetup(target)
                if a.fresh_setup && self.local_cli.target.as_ref() == Some(&target) =>
            {
                let draft = self.local_cli_fresh_hints().unwrap();
                self.local_cli_cancel(&mut out);
                self.local_cli.invalidate();
                self.local_cli.target = Some(LocalCliTarget::New);
                self.local_cli.stage = LocalCliStage::Editing;
                self.local_cli.draft = draft;
                self.local_cli.creation = None;
                self.local_cli.created = None;
                self.local_cli.session_status = None;
                self.local_cli.error = None;
            }
            C::Edit(draft) if a.edit => {
                // Revoke the old approval as well as dropping local trust. This is cleanup,
                // not a probe; a fresh Inspect may supersede its acknowledgement.
                self.local_cli_cancel(&mut out);
                self.local_cli.stage = LocalCliStage::Editing;
                self.local_cli.creation = None;
                self.local_cli.created = None;
                self.local_cli.draft = draft;
                self.local_cli.error = None;
            }
            C::TrustExecutable(value) if a.trust => self.local_cli.trust_executable = value,
            C::TrustProject(value) if a.trust => self.local_cli.trust_project = value,
            C::Inspect if a.inspect => {
                let d = &self.local_cli.draft;
                if d.executable.trim().is_empty() || d.mode == CliMode::Unsupported {
                    return out;
                }
                let target = match &self.local_cli.target {
                    Some(LocalCliTarget::New) if !d.project.trim().is_empty() => {
                        LocalCliInspectTarget::New {
                            project: d.project.clone(),
                            model: d.model,
                            thinking: d.thinking.clone(),
                        }
                    }
                    Some(LocalCliTarget::Existing { session, .. }) => {
                        LocalCliInspectTarget::Existing {
                            session: session.clone(),
                            model: d.model,
                        }
                    }
                    _ => return out,
                };
                let request = LocalCliRequest::Inspect {
                    mode: d.mode,
                    executable: d.executable.clone(),
                    target,
                };
                self.local_cli.stage = LocalCliStage::Inspecting;
                self.local_cli.error = None;
                self.local_cli_send(request, &mut out);
            }
            C::Check if a.check => {
                let setup = self.local_cli.ticket.as_ref().unwrap().setup.clone();
                self.local_cli.stage = LocalCliStage::Checking;
                self.local_cli_send(
                    LocalCliRequest::Check {
                        setup,
                        trust_executable: true,
                        trust_project: true,
                    },
                    &mut out,
                );
            }
            C::Status if a.status => {
                let setup = self.local_cli.ticket.as_ref().unwrap().setup.clone();
                self.local_cli_send(LocalCliRequest::Status { setup }, &mut out);
            }
            C::Create if a.create => {
                let t = self.local_cli.ticket.as_ref().unwrap();
                let identity = LocalCliCreationId {
                    session: t.root_candidate.clone().unwrap(),
                    setup: t.setup.clone(),
                };
                self.local_cli.creation = Some(identity.clone());
                self.local_cli.stage = LocalCliStage::Creating;
                self.local_cli_send(LocalCliRequest::Create { identity }, &mut out);
            }
            C::RepeatCreate if a.repeat_create => {
                let identity = self.local_cli.creation.clone().unwrap();
                self.local_cli.lookup_absent = false;
                self.local_cli.stage = LocalCliStage::Creating;
                self.local_cli_send(LocalCliRequest::Create { identity }, &mut out);
            }
            C::LookupCreation if a.lookup_creation => {
                let identity = self.local_cli.creation.clone().unwrap();
                self.local_cli.lookup_absent = false;
                self.local_cli.stage = LocalCliStage::Reconciling;
                self.local_cli_send(LocalCliRequest::LookupCreation { identity }, &mut out);
            }
            C::ParkCreationAndBeginNew(draft) if a.park_creation => {
                let identity = self.local_cli.creation.clone().unwrap();
                if !self.local_cli.parked_creations.contains(&identity) {
                    self.local_cli.parked_creations.push(identity);
                }
                self.local_cli_cancel(&mut out);
                self.local_cli.stage = LocalCliStage::Editing;
                self.local_cli.target = Some(LocalCliTarget::New);
                self.local_cli.draft = draft;
                self.local_cli.creation = None;
                self.local_cli.created = None;
                self.local_cli.session_status = None;
                self.local_cli.error = None;
            }
            C::RecoverCreation(identity)
                if a.recover_creation && self.local_cli.parked_creations.contains(&identity) =>
            {
                // Swap at capacity rather than forgetting the current unknown identity.
                // Merely selecting another recovery record restores no authority or effects.
                if self.local_cli.unresolved()
                    && let Some(current) = self.local_cli.creation.clone()
                    && current != identity
                    && !self.local_cli.parked_creations.contains(&current)
                {
                    let slot = self
                        .local_cli
                        .parked_creations
                        .iter_mut()
                        .find(|i| **i == identity)
                        .unwrap();
                    *slot = current;
                }
                self.local_cli_cancel(&mut out);
                // Retire cleanup replies too: this is a new global recovery generation,
                // not the previous (possibly Existing) attachment or approval.
                self.local_cli.invalidate();
                self.local_cli.target = Some(LocalCliTarget::New);
                self.local_cli.draft = LocalCliDraft::default();
                self.local_cli.creation = Some(identity);
                self.local_cli.created = None;
                self.local_cli.session_status = None;
                self.local_cli.error = None;
                self.local_cli.stage = LocalCliStage::CreateUnknown;
            }
            C::SessionStatus if a.session_status => {
                let Some(LocalCliTarget::Existing { session, .. }) = &self.local_cli.target else {
                    unreachable!()
                };
                self.local_cli_send(
                    LocalCliRequest::SessionStatus {
                        session: session.clone(),
                    },
                    &mut out,
                );
            }
            C::ReturnToConversation if a.return_to_conversation => {
                let Some(LocalCliTarget::Existing { conversation, .. }) = self.local_cli.target
                else {
                    unreachable!()
                };
                // An authoritative Open refresh, not a local execution-readiness assumption.
                self.local_cli_refresh(conversation, &mut out);
            }
            C::Cancel if a.cancel => self.local_cli_cancel(&mut out),
            C::Revoke if a.revoke => self.local_cli_cancel(&mut out),
            _ => return out,
        }
        out.notes.push(Note::Other);
        out
    }

    pub(crate) fn local_cli_cancel(&mut self, out: &mut Outcome) {
        if let Some(p) = &self.local_cli.pending
            && p.phase == LocalCliPhase::Inspect
        {
            // No ticket is known yet. Let the adapter retire this precise in-flight
            // inspection, including revoking a ticket if its completion races Cancel.
            out.effects
                .push(Effect::Backend(BackendRequest::RetireLocalCliInspect {
                    conversation: p.conversation,
                    generation: p.generation,
                    op: p.op,
                }));
        }
        let ticket = self.local_cli.ticket.as_ref().map(|t| t.setup.clone());
        self.local_cli.invalidate();
        if self.connection.is_ready()
            && self.local_cli.available
            && self.local_cli_target_allowed(true)
            && let Some(setup) = ticket
        {
            self.local_cli_send(LocalCliRequest::Revoke { setup }, out);
        }
    }

    pub(crate) fn local_cli_disconnected(&mut self) {
        self.local_cli.invalidate();
    }

    pub(crate) fn local_cli_expire(&mut self) {
        if self
            .local_cli
            .ticket
            .as_ref()
            .and_then(|t| t.expires_at)
            .is_some_and(|at| self.now() >= at)
        {
            self.local_cli.invalidate();
            self.local_cli.error = Some(LocalCliFailure::Expired);
        }
    }

    fn local_cli_refresh(&mut self, id: ConversationId, out: &mut Outcome) {
        // A dashboard reconnect refreshes the captured thread in place. Selecting it
        // normally would close the graph and replace the user's main conversation.
        if self.dashboard.open {
            out.merge(self.dashboard_attach(id));
            return;
        }
        // Force Open even if already selected; SelectConversation handles generation and prefs.
        if let Some(c) = self.conversations.iter_mut().find(|c| c.id == id) {
            c.opened = false;
        }
        out.merge(self.dispatch(Command::SelectConversation(id)));
    }

    /// Called after both a correlated completion and catalog merges (either arrival order).
    pub(crate) fn local_cli_reconcile_catalog(&mut self, out: &mut Outcome) {
        if self.local_cli.stage != LocalCliStage::Reconciling || !self.connection.is_ready() {
            return;
        }
        let Some(created) = &self.local_cli.created else {
            return;
        };
        if self.conversation(created.conversation).is_none() {
            return;
        }
        let id = created.conversation;
        out.effects.push(Effect::RetireLocalCliCreation {
            identity: created.identity.clone(),
        });
        self.local_cli
            .parked_creations
            .retain(|identity| identity != &created.identity);
        self.local_cli.stage = LocalCliStage::Created;
        let origin = self.local_cli.provider_origin.take();
        let select = self.provider_origin_current(&origin);
        self.finish_provider_origin(origin, out);
        if self.local_cli.dashboard_creation {
            if self.dashboard.open {
                // Show the newly created idle thread even when an attention/search filter
                // excluded it. Keep the graph entity and its viewport alive.
                self.dashboard.group = Some(crate::dashboard::ActivityGroup::All);
                self.dashboard.filter = Default::default();
                out.merge(self.dispatch(Command::Dashboard(crate::dashboard::Command::Refresh)));
                out.merge(self.dashboard_attach(id));
            }
        } else if select {
            self.local_cli_refresh(id, out);
        }
        self.coalesce_provider_prefs(out);
    }

    pub(crate) fn apply_local_cli_event(&mut self, ev: BackendEvent) -> Outcome {
        let mut out = Outcome::default();
        let EventKind::LocalCli(event) = ev.kind else {
            return out;
        };
        let Some(p) = &self.local_cli.pending else {
            return out;
        };
        // This precedes all conversation lookup and prompt run-op matching.
        if p.persisting
            || (ev.conversation, ev.generation, ev.op, event.phase())
                != (p.conversation, p.generation, Some(p.op), p.phase)
            || !self.local_cli_target_allowed(p.phase == LocalCliPhase::Revoke)
            || !self.connection.is_ready()
        {
            return out;
        }
        // Also validate payload identity and phase-specific shape before consuming the op.
        if !self.local_cli_event_valid(&event) {
            return out;
        }
        self.local_cli.pending = None;
        self.local_cli.error = None;
        let checked = matches!(&event, LocalCliEvent::Checked(_));
        let discovered = matches!(&event, LocalCliEvent::Discovered(_));
        let inspected = matches!(&event, LocalCliEvent::Inspected(_));
        let hints = matches!(&event, LocalCliEvent::SessionStatus(_));
        match event {
            LocalCliEvent::Discovered(paths) => {
                self.local_cli.discovering = false;
                self.local_cli.candidates = paths;
                if self.prefs.cli_provider_configured
                    && let Some(saved) = &self.prefs.cli_executable
                    && self.local_cli.candidates.contains(saved)
                {
                    self.local_cli.draft.executable = saved.clone();
                } else if self.local_cli.candidates.len() == 1 {
                    self.local_cli.draft.executable = self.local_cli.candidates[0].clone();
                }
            }
            LocalCliEvent::Inspected(view) => {
                self.local_cli.ticket = Some(view);
                self.local_cli.trust_executable = false;
                self.local_cli.trust_project = false;
                self.local_cli.stage = LocalCliStage::AwaitingTrust;
            }
            LocalCliEvent::Checked(view) | LocalCliEvent::Status(view) => {
                self.local_cli.stage = match view.stage {
                    LocalCliMetadataStage::AwaitingTrust => LocalCliStage::AwaitingTrust,
                    LocalCliMetadataStage::Checking => LocalCliStage::CheckUnknown,
                    LocalCliMetadataStage::MetadataVerified => LocalCliStage::Verified,
                    LocalCliMetadataStage::Failed => LocalCliStage::Failed,
                };
                self.local_cli.ticket = Some(view);
            }
            LocalCliEvent::Revoked => {}
            LocalCliEvent::Created(created) => {
                self.local_cli.created = Some(created);
                self.local_cli.stage = LocalCliStage::Reconciling;
            }
            LocalCliEvent::CreationLookup { found, .. } => {
                self.local_cli.lookup_absent = found.is_none();
                self.local_cli.stage = if found.is_some() {
                    LocalCliStage::Reconciling
                } else {
                    LocalCliStage::CreateUnknown
                };
                self.local_cli.created = found;
            }
            LocalCliEvent::SessionStatus(view) => self.local_cli.session_status = Some(view),
            LocalCliEvent::Failed { phase, failure } => {
                self.local_cli.connect_after_check = false;
                self.local_cli.discovering = false;
                // A failed Create never proves absence; pure lookup is the only next step.
                if self.local_cli.unresolved() {
                    self.local_cli.stage = if self.local_cli.created.is_some() {
                        LocalCliStage::Reconciling
                    } else {
                        LocalCliStage::CreateUnknown
                    };
                } else if matches!(phase, LocalCliPhase::Check | LocalCliPhase::Status)
                    && matches!(failure, LocalCliFailure::Unknown(_))
                {
                    self.local_cli.stage = LocalCliStage::CheckUnknown;
                } else if !matches!(phase, LocalCliPhase::SessionStatus | LocalCliPhase::Revoke) {
                    self.local_cli.stage = LocalCliStage::Failed;
                }
                if matches!(failure, LocalCliFailure::Expired) {
                    self.local_cli.invalidate();
                }
                self.local_cli.error = Some(failure);
            }
        }
        self.local_cli_expire();
        self.advance_guided_cli(discovered, inspected, hints, checked, &mut out);
        self.local_cli_reconcile_catalog(&mut out);
        out.notes.push(Note::Other);
        out
    }

    fn local_cli_event_valid(&self, event: &LocalCliEvent) -> bool {
        let s = &self.local_cli;
        let creation_matches = |c: &LocalCliCreated| {
            s.creation.as_ref() == Some(&c.identity) && c.conversation != LOCAL_CLI_GLOBAL
        };
        match event {
            LocalCliEvent::Discovered(paths) => {
                paths.len() <= 16
                    && paths.iter().all(|p| {
                        p.starts_with('/') && p.len() <= 4096 && !p.chars().any(char::is_control)
                    })
            }
            LocalCliEvent::Inspected(view) => {
                let target = match &s.target {
                    Some(LocalCliTarget::New) => LocalCliInspectTarget::New {
                        project: s.draft.project.clone(),
                        model: s.draft.model,
                        thinking: s.draft.thinking.clone(),
                    },
                    Some(LocalCliTarget::Existing { session, .. }) => {
                        LocalCliInspectTarget::Existing {
                            session: session.clone(),
                            model: s.draft.model,
                        }
                    }
                    None => return false,
                };
                let bounded_id = !view.setup.0.is_empty()
                    && view.setup.0.len() <= 124
                    && view
                        .setup
                        .0
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
                bounded_id
                    && view.mode == s.draft.mode
                    && view.mode != CliMode::Unsupported
                    && view.target == target
                    && view.stage == LocalCliMetadataStage::AwaitingTrust
                    && !view.executable.is_empty()
                    && !view.project.is_empty()
                    && match &s.target {
                        Some(LocalCliTarget::New) => view
                            .root_candidate
                            .as_ref()
                            .is_some_and(|id| id.0 == format!("cli-{}", view.setup.0)),
                        _ => view.root_candidate.is_none(),
                    }
            }
            LocalCliEvent::Checked(view) | LocalCliEvent::Status(view) => {
                s.ticket.as_ref().is_some_and(|t| {
                    t.setup == view.setup
                        && t.mode == view.mode
                        && t.target == view.target
                        && t.root_candidate == view.root_candidate
                        && t.executable == view.executable
                        && t.project == view.project
                        && t.qualified_version == view.qualified_version
                        && t.expires_at == view.expires_at
                })
            }
            LocalCliEvent::Created(c) => creation_matches(c),
            LocalCliEvent::CreationLookup { identity, found } => {
                s.creation.as_ref() == Some(identity) && found.as_ref().is_none_or(creation_matches)
            }
            LocalCliEvent::SessionStatus(view) => {
                matches!(&s.target, Some(LocalCliTarget::Existing { session, .. }) if session == &view.session)
            }
            LocalCliEvent::Revoked | LocalCliEvent::Failed { .. } => true,
        }
    }
}
