//! Dashboard entries reuse captured-target permission and connection dialogs.
mod new_thread;

pub(super) struct NewThread;

use super::workspace::{Overlay, Workspace};
use gpui::{Context, Window};
use pipkin_core::{CliApprovalCommand, Command, dashboard as d};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Approvals,
    Reconnect,
}
#[derive(Clone, Debug)]
pub(super) struct Request {
    pub session: String,
    pub generation: u64,
    pub revision: u64,
    pub action: Action,
}

impl Workspace {
    pub(super) fn on_dashboard_cli(
        &mut self,
        request: &Request,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let s = self.state(cx);
        if !s.dashboard.open || s.dashboard.detail.generation != request.generation {
            return;
        }
        let Some(id) = s
            .dashboard
            .selected_row()
            .filter(|r| {
                r.worker.session == request.session && r.worker.revision == request.revision
            })
            .and_then(|r| r.conversation)
        else {
            return;
        };
        if s.owned_elsewhere(id) || s.conversation_is_closed(id) {
            return;
        }
        let known_cli = s.conversation(id).is_some_and(|c| c.profile.is_local_cli());
        if request.action == Action::Reconnect && known_cli {
            self.open_local_cli_for(id, window, cx);
            return;
        }
        if request.action == Action::Approvals && s.dashboard_attachment_ready(id) {
            self.open_dashboard_cli_approvals(id, window, cx);
            return;
        }
        // The directory identifies the backend for presentation, but only an explicit
        // Open's execution context can authorize setup or permission operations.
        let cli_row = s.dashboard.selected_row().is_some_and(|r| {
            matches!(
                r.worker.execution_backend,
                d::ExecutionBackend::LocalClaudeCli | d::ExecutionBackend::ClaudeCliAgent
            )
        });
        if !(known_cli || cli_row) || !s.dashboard_attach_available() {
            return;
        }
        self.dispatch(
            Command::Dashboard(d::Command::Attach {
                session: request.session.clone(),
                generation: request.generation,
                revision: request.revision,
            }),
            cx,
        );
        self.dashboard_cli_pending = self
            .state(cx)
            .dashboard
            .attachment
            .map(|(id, generation)| (id, generation, request.action));
        self.reconcile_dashboard_cli(window, cx);
    }

    fn open_dashboard_cli_approvals(
        &mut self,
        id: pipkin_core::ConversationId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_cli_approvals_for(id, window, cx);
        // The explicit Review action loads requests, never sends a decision.
        if self.state(cx).cli_approval_discovery_available(id) {
            self.dispatch(Command::CliApproval(CliApprovalCommand::Discover(id)), cx);
        }
    }

    pub(super) fn reconcile_dashboard_cli(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, generation, action)) = self.dashboard_cli_pending else {
            return;
        };
        let s = self.state(cx);
        if !s.dashboard.open
            || !s.connection.is_ready()
            || s.dashboard.selected_row().and_then(|r| r.conversation) != Some(id)
            || s.conversation(id)
                .is_none_or(|c| c.generation != generation)
            || self.overlay != Overlay::None
            || !self.dashboard.read(cx).thread_dialog_open()
            || s.dashboard.attachment_error.is_some()
        {
            self.dashboard_cli_pending = None;
            return;
        }
        if s.dashboard_attachment_ready(id) {
            self.dashboard_cli_pending = None;
            match action {
                Action::Approvals => self.open_dashboard_cli_approvals(id, window, cx),
                Action::Reconnect => self.open_local_cli_for(id, window, cx),
            }
        }
    }
}
