//! Execution policy lives in the core; readiness never grants native capabilities.
use crate::{
    AppState, Command, ConversationId, ConversationState, ExecutionProfile, IntentOrigin, Mode,
    Note, Outcome,
};

/// Pure conversation capabilities for UI affordances; independent of temporary readiness.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExecutionCapabilities {
    pub model_settings: bool,
    pub attachments: bool,
}

impl ConversationState {
    pub(crate) fn execution_profile(&self, mode: Mode) -> ExecutionProfile {
        if mode == Mode::Real && !self.execution_context_known {
            ExecutionProfile::Unsupported
        } else {
            self.profile
        }
    }
}

impl ExecutionProfile {
    pub fn is_local_cli(self) -> bool {
        matches!(self, Self::LocalCliChatOnly | Self::LocalCliRestrictedWrite)
    }

    pub(crate) fn allows_command(self, command: &Command) -> bool {
        if self == Self::NativeAgent {
            return true;
        }
        match command {
            Command::Submit | Command::Cancel | Command::RetryStop => self.is_local_cli(),
            // Local editing/navigation, historical reads, app configuration and creation of
            // a fresh conversation do not execute work in this conversation.
            Command::LocalCli(_)
            | Command::SelectProviderModel(_)
            | Command::KeepConversationProvider
            | Command::SelectProject(_)
            | Command::SelectConversation(_)
            | Command::CloseConversation { .. }
            | Command::ReopenConversation(_)
            | Command::NewConversation
            | Command::RenameConversation(..)
            | Command::SetSearch(_)
            | Command::EditDraft(_)
            | Command::PreviousPrompt
            | Command::NextPrompt
            | Command::FlushDraft(_)
            | Command::RemoveAttachment(_)
            | Command::AddProject(_)
            | Command::DismissNotice
            | Command::DismissStorageIssue
            | Command::RefreshChanges
            | Command::RetrySave
            | Command::CopyToolOutput(_)
            | Command::SaveToolOutput(_)
            | Command::DismissUiNotices
            | Command::OpenSearchHit(_)
            | Command::ReturnFromSearch
            | Command::ClearScrollTarget
            | Command::OpenInEditor(_)
            | Command::OpenTerminal
            | Command::CheckStatus
            | Command::DismissRecoveryNotice
            | Command::DismissFailure
            | Command::LoadOlder
            | Command::SelectChange(_)
            | Command::SetTheme(_)
            | Command::SetTextSize(_)
            | Command::SetReducedMotion(_)
            | Command::SetNavWidth(_)
            | Command::SetInspectorWidth(_)
            | Command::SetWindowSize(..)
            | Command::SetInspectorOpen(_)
            | Command::RemoveSignIn(_)
            | Command::StartSignIn(_)
            | Command::ReuseSignIn(_)
            | Command::AnswerSignIn { .. }
            | Command::CancelSignIn { .. }
            | Command::RefreshDiagnostics
            | Command::ExportDiagnostics
            | Command::ExportConfigure(_)
            | Command::ExportDisable
            | Command::ExportSetCredential { .. }
            | Command::ExportClearCredential
            | Command::ExportPreview(_)
            | Command::ExportDismissNotice
            | Command::ImportFromPi(_)
            | Command::ConfirmImport
            | Command::CancelImport
            | Command::ReleaseToPi
            | Command::ConfirmRelease
            | Command::CancelRelease => true,
            // Fail closed for native features, including future commands.
            _ => false,
        }
    }

    pub(crate) fn allows_intent(
        self,
        origin: IntentOrigin,
        text: &str,
        attachments: usize,
    ) -> bool {
        match self {
            Self::NativeAgent => true,
            Self::LocalCliChatOnly | Self::LocalCliRestrictedWrite => {
                origin == IntentOrigin::Draft && !text.trim().is_empty() && attachments == 0
            }
            Self::Unsupported => false,
        }
    }
}

impl AppState {
    /// Authoritative conversation identity; unresolved real contexts are unsupported, not native.
    pub fn current_execution_profile(&self) -> Option<ExecutionProfile> {
        self.current().map(|c| c.execution_profile(self.mode))
    }

    /// Use for model/thinking controls and attachment affordances, not UI-side profile rules.
    /// Unknown real contexts fail closed; Demo retains its native simulated capabilities.
    pub fn execution_capabilities(&self) -> ExecutionCapabilities {
        let native = self
            .current()
            .is_some_and(|c| c.execution_profile(self.mode) == ExecutionProfile::NativeAgent);
        ExecutionCapabilities {
            model_settings: native,
            attachments: native,
        }
    }

    pub(crate) fn refuse_if_profile_disallows(
        &mut self,
        id: ConversationId,
        allowed: bool,
        out: &mut Outcome,
    ) -> bool {
        if allowed {
            return false;
        }
        self.notice = Some(
            self.conversation(id)
                .and_then(|c| c.execution_blocked.clone())
                .unwrap_or_else(|| "This execution profile does not support that action.".into()),
        );
        out.notes.push(Note::Other);
        true
    }

    pub(crate) fn refuse_intent(
        &mut self,
        id: ConversationId,
        origin: IntentOrigin,
        text: &str,
        attachments: usize,
        out: &mut Outcome,
    ) -> bool {
        if self.provider_selection_pending(id) {
            self.notice = Some("Not sent: this conversation has an unfinished provider change. Complete it or explicitly keep the original provider; no fallback was used.".into());
            out.notes.push(Note::Other);
            return true;
        }
        let allowed = self.conversation(id).is_some_and(|c| {
            c.execution_profile(self.mode)
                .allows_intent(origin, text, attachments)
                && (c.execution_profile(self.mode) == ExecutionProfile::NativeAgent
                    || (self.connection.is_ready()
                        && self.read_only.is_none()
                        && (self.mode == crate::Mode::Demo || c.opened)
                        && matches!(
                            c.run,
                            crate::RunState::Idle | crate::RunState::Failed { .. }
                        )))
        });
        self.refuse_if_profile_disallows(id, allowed, out)
            || self.refuse_if_execution_blocked(id, out)
            || self.refuse_if_owned(id, out)
    }
}
