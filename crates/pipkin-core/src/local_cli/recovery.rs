//! Durable creation facts are not setup tickets, trust, execution grants, or replay work.
use super::*;

impl LocalCliCreationId {
    pub fn valid_recovery_identity(&self) -> bool {
        let valid = |s: &str, max| {
            !s.is_empty()
                && s.len() <= max
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        };
        valid(&self.session.0, 128) && valid(&self.setup.0, 124)
    }
}

impl AppState {
    /// Startup-only, no effects. Restore at most eight parked identities and one current
    /// unknown identity. Never restore a ticket, checked trust box, pending request or decision.
    pub fn restore_local_cli_creations(
        &mut self,
        records: Result<Vec<LocalCliCreationId>, String>,
    ) {
        let valid = records.as_ref().is_ok_and(|ids| {
            ids.len() <= LOCAL_CLI_RECOVERY_CAPACITY + 1
                && ids.iter().enumerate().all(|(i, id)| {
                    id.valid_recovery_identity()
                        && !ids[..i].iter().any(|old| old.session == id.session)
                })
        });
        if !valid
            || self.local_cli.target.is_some()
            || self.local_cli.pending.is_some()
            || self.local_cli.creation.is_some()
            || !self.local_cli.parked_creations.is_empty()
        {
            self.local_cli.recovery_unavailable = true;
            self.local_cli.error = Some(LocalCliFailure::Unknown("Stored CLI creation recovery could not be loaded. New creation is disabled; do not repeat an uncertain creation.".into()));
            return;
        }
        let mut records = records.unwrap();
        if records.is_empty() {
            return;
        }
        self.local_cli.invalidate();
        self.local_cli.creation = records.pop();
        self.local_cli.parked_creations = records;
        self.local_cli.target = Some(LocalCliTarget::New);
        self.local_cli.stage = LocalCliStage::CreateUnknown;
    }

    /// Only a matching successful durable acknowledgment may release a Create RPC.
    /// Closing, expiry, disconnect or attachment replacement permanently retire this callback.
    pub fn local_cli_creation_persisted(
        &mut self,
        conversation: ConversationId,
        generation: u64,
        op: OperationId,
        identity: &LocalCliCreationId,
        result: Result<(), String>,
    ) -> Outcome {
        let mut out = Outcome::default();
        self.local_cli_expire();
        let Some(p) = self.local_cli.pending.as_ref() else {
            return out;
        };
        if !p.persisting
            || (p.conversation, p.generation, p.op, p.phase)
                != (conversation, generation, op, LocalCliPhase::Create)
            || self.local_cli.creation.as_ref() != Some(identity)
        {
            return out;
        }
        self.local_cli.pending.as_mut().unwrap().persisting = false;
        let permitted = self.mode == Mode::Real
            && self.read_only.is_none()
            && self.connection.is_ready()
            && self.local_cli.available
            && !self.local_cli.recovery_unavailable
            && self.local_cli_target_allowed(false)
            && matches!(self.local_cli.target, Some(LocalCliTarget::New))
            && self.local_cli.trust_executable
            && self.local_cli.trust_project
            && self.local_cli.ticket.as_ref().is_some_and(|ticket| {
                ticket.setup == identity.setup
                    && ticket.root_candidate.as_ref() == Some(&identity.session)
                    && ticket.mode == self.local_cli.draft.mode
                    && matches!(&ticket.target, LocalCliInspectTarget::New { project, model, thinking }
                        if project == &self.local_cli.draft.project && *model == self.local_cli.draft.model && thinking == &self.local_cli.draft.thinking)
                    && ticket.stage == LocalCliMetadataStage::MetadataVerified
                    && ticket.expires_at.is_none_or(|t| self.now() < t)
            });
        if result.is_err() || !permitted || !identity.valid_recovery_identity() {
            self.local_cli.pending = None;
            self.local_cli.stage = LocalCliStage::CreateUnknown;
            self.local_cli.lookup_absent = false;
            self.local_cli.error = Some(LocalCliFailure::Unknown("Creation was not released: its recovery identity could not be saved or its setup scope retired. Use explicit read-only lookup; do not assume absence.".into()));
            out.notes.push(Note::Other);
            return out;
        }
        out.effects.push(Effect::Backend(BackendRequest::LocalCli {
            conversation,
            generation,
            op,
            request: LocalCliRequest::Create {
                identity: identity.clone(),
            },
        }));
        out.notes.push(Note::Other);
        out
    }
}
