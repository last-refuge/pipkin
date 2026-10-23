//! Closing is desktop organization, not engine deletion, cancellation or a new conversation.
use crate::*;

pub const MAX_CLOSED_CONVERSATIONS: usize = 10_000;

impl AppState {
    pub fn conversation_is_closed(&self, id: ConversationId) -> bool {
        self.prefs.closed_conversations.contains(&id.0)
    }

    /// Closed rows stay searchable/reopenable within their project, newest first.
    pub fn closed_conversations(&self) -> Vec<&ConversationState> {
        let project = self.current_project().map(|p| p.id);
        let query = self.search.trim().to_lowercase();
        let mut rows: Vec<_> = self
            .conversations
            .iter()
            .filter(|c| {
                Some(c.project) == project
                    && self.conversation_is_closed(c.id)
                    && (query.is_empty() || c.title.to_lowercase().contains(&query))
            })
            .collect();
        rows.sort_by_key(|c| std::cmp::Reverse(c.updated_at));
        rows
    }

    pub(crate) fn unsettled_conversation_reason(
        &self,
        c: &ConversationState,
    ) -> Option<&'static str> {
        if matches!(c.run, RunState::OutcomeUnknown { .. }) || c.status_check_pending {
            return Some("Check the unresolved work status before closing this conversation.");
        }
        if c.goal_start_pending.is_some() {
            return Some(
                "Wait for the goal start/resume receipt, or reopen to refresh its status before closing.",
            );
        }
        if c.run.is_busy() || c.pending_intent.is_some() {
            return Some("Stop active work and wait for it to settle before closing.");
        }
        if !c.queue.is_empty() || !c.pending_queue.is_empty() {
            return Some("Resolve queued follow-ups before closing this conversation.");
        }
        if c.waiting_question || !c.ui_requests.is_empty() || !c.ui_answering.is_empty() {
            return Some("Resolve the pending question before closing this conversation.");
        }
        if c.subagents.children.iter().any(SubagentInfo::active)
            || c.subagents.control_pending.is_some()
            || (c.profile == ExecutionProfile::NativeAgent && c.subagents.error.is_some())
        {
            return Some("Stop active subagents and wait for them to settle before closing.");
        }
        if c.jobs.active > 0
            || c.jobs.error.is_some()
            || c.jobs
                .jobs
                .iter()
                .any(|j| j.state.active() || j.state == crate::jobs::JobState::Unknown)
        {
            return Some("Resolve active or unknown background jobs before closing.");
        }
        if c.goal.as_ref().is_some_and(|g| {
            if g.native {
                matches!(
                    g.state,
                    None | Some(GoalState::Active | GoalState::Verifying)
                )
            } else {
                g.paused.is_none()
            }
        }) {
            return Some("Pause or finish the active goal before closing this conversation.");
        }
        if self.prefs.pending_provider_models.contains_key(&c.id.0)
            || self.prefs.pending_provider_models.contains_key(&0)
        {
            return Some(
                "Finish the provider change or keep this conversation's provider before closing.",
            );
        }
        if self
            .cli_approvals
            .contexts()
            .filter(|context| context.conversation == c.id)
            .any(|context| {
                self.cli_approvals.items(context).iter().any(|item| {
                    matches!(
                        item.status,
                        crate::cli_approvals::CliApprovalStatus::Reviewable
                            | crate::cli_approvals::CliApprovalStatus::Submitting
                            | crate::cli_approvals::CliApprovalStatus::Unknown
                    )
                })
            })
        {
            return Some("Resolve pending or unknown CLI approvals before closing.");
        }
        None
    }

    pub fn close_conversation_reason(&self, id: ConversationId) -> Option<&'static str> {
        let Some(c) = self.conversation(id) else {
            return Some("Conversation is unavailable.");
        };
        if self.conversation_is_closed(id) {
            return Some("This conversation is already closed.");
        }
        if self.prefs.closed_conversations.len() >= MAX_CLOSED_CONVERSATIONS {
            return Some("The Closed list is full; reopen a conversation before closing another.");
        }
        if !c.opened
            || (self.mode == Mode::Real
                && (!self.connection.is_ready()
                    || !c.execution_context_known
                    || !c.lifecycle_ready
                    || c.stale_reason.is_some()
                    || c.cached_only))
        {
            return Some("Connect and load the current conversation state before closing.");
        }
        if let Some(reason) = self.unsettled_conversation_reason(c) {
            return Some(reason);
        }
        if !matches!(c.draft.save, SaveState::Clean | SaveState::Saved)
            || self.storage_issue.is_some()
        {
            return Some("Wait for the draft to save and resolve storage errors before closing.");
        }
        if self.pending_native_creation()
            || matches!(
                self.local_cli.stage,
                LocalCliStage::Creating | LocalCliStage::CreateUnknown | LocalCliStage::Reconciling
            )
            || (matches!(self.local_cli.target, Some(LocalCliTarget::Existing { conversation, .. }) if conversation == id)
                && matches!(
                    self.local_cli.stage,
                    LocalCliStage::Inspecting
                        | LocalCliStage::Checking
                        | LocalCliStage::CheckUnknown
                ))
        {
            return Some(
                "Resolve the pending setup or conversation creation outcome before closing.",
            );
        }
        None
    }

    /// A global backend attention hint may restore visibility, but cannot select a chat,
    /// authorize execution, or decide the outcome of an operation. A fresh view is required
    /// before closing it again.
    pub fn reveal_conversation_activity(&mut self, id: ConversationId) -> Outcome {
        let mut out = Outcome::default();
        if self.prefs.closed_conversations.remove(&id.0) {
            if let Some(c) = self.conversations.iter_mut().find(|c| c.id == id) {
                c.lifecycle_ready = false;
            }
            self.notice = Some(
                "A closed conversation has new activity and was moved back to Conversations."
                    .into(),
            );
            out.effects.push(Effect::SavePrefs(self.prefs.clone()));
            out.notes.push(Note::ConversationsChanged);
            out.notes.push(Note::Other);
            if self.dashboard.open {
                out.merge(self.dispatch_dashboard(crate::dashboard::Command::Refresh));
            }
        }
        out
    }

    /// Activity arriving after Close must not remain hidden. This only restores visibility,
    /// never changes selection, cancels work, sends a prompt or repeats an unknown operation.
    pub(crate) fn reveal_unsettled_conversation(&mut self, id: ConversationId, out: &mut Outcome) {
        if self.conversation_is_closed(id)
            && self
                .conversation(id)
                .is_some_and(|c| self.unsettled_conversation_reason(c).is_some())
        {
            self.prefs.closed_conversations.remove(&id.0);
            out.effects.push(Effect::SavePrefs(self.prefs.clone()));
            out.notes.push(Note::ConversationsChanged);
            self.coalesce_provider_prefs(out);
            if self.dashboard.open {
                out.merge(self.dispatch_dashboard(crate::dashboard::Command::Refresh));
            }
        }
    }
}
