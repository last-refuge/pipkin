//! Explicit, metadata-only assistance. Never infer actions or manufacture model context.
use super::*;
impl AppState {
    pub fn local_cli_diagnostic_scope(&self) -> (ConversationId, u64) {
        self.local_cli.route()
    }
    pub(super) fn local_cli_fresh_hints(&self) -> Option<LocalCliDraft> {
        let LocalCliTarget::Existing {
            conversation,
            generation,
            session,
        } = self.local_cli.target.as_ref()?
        else {
            return None;
        };
        let c = self.conversation(*conversation)?;
        if c.generation != *generation
            || c.run.is_busy()
            || !c.execution_profile(self.mode).is_local_cli()
            || self.owned_elsewhere(*conversation)
        {
            return None;
        }
        let view = self.local_cli.session_status.as_ref()?;
        if &view.session != session
            || view.executable.trim().is_empty()
            || view.project.trim().is_empty()
            || !matches!(
                (c.execution_profile(self.mode), view.mode),
                (crate::ExecutionProfile::LocalCliChatOnly, CliMode::ChatOnly)
                    | (
                        crate::ExecutionProfile::LocalCliRestrictedWrite,
                        CliMode::RestrictedWrite
                    )
            )
        {
            return None;
        }
        Some(LocalCliDraft {
            mode: view.mode,
            executable: view.executable.clone(),
            project: view.project.clone(),
            model: view.model?,
        })
    }
    /// Static guidance only. No raw failure strings, paths, arguments or account metadata.
    pub fn local_cli_recovery_guidance(&self) -> &'static str {
        if self.local_cli.recovery_unavailable || self.cli_approvals.recovery_unavailable() {
            "Recovery storage is unreadable or exceeds its bound. Preserve the database and backups; do not delete records or reset the profile to bypass uncertainty. Decisions/creation stay disabled as applicable."
        } else if self.local_cli.unresolved() {
            "Creation may be uncertain. Use explicit read-only lookup of the saved identity; absence is not final and never triggers automatic recreation."
        } else if self
            .local_cli
            .session_status
            .as_ref()
            .is_some_and(|v| !v.context_valid)
        {
            "This CLI context cannot be resumed safely. Inspect transcript and project files. Once the source run is settled, start a separate fresh setup; it copies only executable/project/model/mode hints, never transcript, arguments, prompts, goals or authority."
        } else {
            match self.local_cli.error {
                Some(LocalCliFailure::Unknown(_)) => {
                    "The setup outcome is unknown. Use explicit status or read-only lookup; never blindly resend a mutation."
                }
                Some(LocalCliFailure::Expired) => {
                    "Approval expired. Inspect again and explicitly renew both trust choices; an old ticket cannot authorize a new request."
                }
                Some(LocalCliFailure::Conflict) => {
                    "The saved creation identity conflicts. Preserve it and use read-only lookup; do not substitute another identity for the same uncertain attempt."
                }
                _ => {
                    "For login failures, use the installed CLI's own sign-in outside Pipkin; never copy credentials into Pipkin. For version/flag failures, verify the supported CLI version in the bridge qualification notes; do not bypass policy flags. Metadata verification alone is not execution readiness."
                }
            }
        }
    }
    /// Called by an explicit command, not render. Only closed types/booleans/counts escape.
    pub fn local_cli_diagnostic_report(&self) -> String {
        let s = &self.local_cli;
        let context = s.session_status.as_ref().map(|v| v.context_valid);
        let blocked = match &s.target {
            Some(LocalCliTarget::Existing { conversation, .. }) => self
                .conversation(*conversation)
                .map(|c| c.execution_blocked.is_some()),
            _ => None,
        };
        let uncertain = self
            .cli_approvals
            .contexts()
            .filter(|c| {
                self.cli_approvals.items(c).iter().any(|i| {
                    matches!(
                        i.status,
                        crate::CliApprovalStatus::Unknown | crate::CliApprovalStatus::Submitting
                    )
                })
            })
            .count();
        format!(
            "Local Claude CLI bridge diagnostic v1 (redacted)\nMode: {:?}\nSetup stage: {:?}\nMetadata API available: {}\nConnection ready: {}\nDesktop writable: {}\nTicket present: {}\nTrust choices: executable={} project={}\nSaved context valid: {:?}\nConversation blocked: {:?}\nCreation recovery unavailable: {}\nParked creation identities: {}\nApproval recovery unavailable: {}\nApproval contexts: {} (maximum {})\nUncertain approval contexts: {}\n\n{}\n\nReceipt consumption is not proof of delivery or action. Project Git state is observation, not attribution. Stop cannot undo effects; after a crash a CLI child/descendant may still exist. Do not resume or replay uncertain work. No billing verification is implied; no credential extraction or API fallback is used.\nNo paths, session IDs, account metadata, raw errors, prompts, argument bodies or transcript text are included.\n",
            s.draft.mode,
            s.stage,
            s.available,
            self.connection.is_ready(),
            self.read_only.is_none(),
            s.ticket.is_some(),
            s.trust_executable,
            s.trust_project,
            context,
            blocked,
            s.recovery_unavailable,
            s.parked_creations.len(),
            self.cli_approvals.recovery_unavailable(),
            self.cli_approvals.contexts().count(),
            crate::CLI_APPROVAL_CONTEXTS,
            uncertain,
            self.local_cli_recovery_guidance()
        )
    }
}
