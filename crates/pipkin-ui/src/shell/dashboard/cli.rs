use super::super::dashboard_cli::{Action, Request};
use super::*;

impl DashboardView {
    pub(in crate::shell) fn thread_dialog_open(&self) -> bool {
        self.modal_open
    }

    pub(super) fn cli_controls(&self, row: &d::Row, cx: &Context<Self>) -> AnyElement {
        let s = &self.model.read(cx).state;
        let Some(id) = row.conversation else {
            return div().into_any_element();
        };
        let Some(c) = s.conversation(id).filter(|c| {
            c.profile.is_local_cli()
                || matches!(
                    row.worker.execution_backend,
                    d::ExecutionBackend::LocalClaudeCli | d::ExecutionBackend::ClaudeCliAgent
                )
        }) else {
            return div().into_any_element();
        };
        let mut panel = div().px(px(16.)).py(px(10.)).flex().flex_col().gap(px(8.));
        let mut buttons = div().flex().flex_wrap().gap(px(8.));
        for (key, label, action, visible, enabled) in [
            (
                "dashboard-cli-approvals",
                "Review CLI approvals",
                Action::Approvals,
                s.cli_approval_panel_available(id)
                    || matches!(
                        row.worker.state,
                        d::WorkState::Waiting {
                            reason: d::WaitReason::CliApproval
                        }
                    ),
                s.dashboard_attachment_ready(id) || s.dashboard_attach_available(),
            ),
            (
                "dashboard-cli-reconnect",
                "Reconnect Claude Code…",
                Action::Reconnect,
                true,
                !s.owned_elsewhere(id)
                    && !s.conversation_is_closed(id)
                    && self.model.read(cx).local_cli_session(id).is_some()
                    && (s.local_cli_availability().begin
                        || (s.local_cli.guided
                            && s.local_cli.stage != LocalCliStage::Created
                            && matches!(&s.local_cli.target, Some(LocalCliTarget::Existing { conversation, generation, session })
                                if *conversation == id && *generation == c.generation
                                    && self.model.read(cx).local_cli_session(id) == Some(session)))),
            ),
        ] {
            if !visible {
                continue;
            }
            let request = Request {
                session: row.worker.session.clone(),
                generation: s.dashboard.detail.generation,
                revision: row.worker.revision,
                action,
            };
            let view = cx.entity();
            buttons = buttons.child(
                Btn::new(key)
                    .label(label)
                    .tab_stop(true)
                    .disabled(!enabled)
                    .on_click(move |_, cx| view.update(cx, |_, cx| cx.emit(request.clone()))),
            );
        }
        buttons = buttons.child(
            self.button(
                "dashboard-cli-refresh",
                "Refresh thread state",
                D::Attach {
                    session: row.worker.session.clone(),
                    generation: s.dashboard.detail.generation,
                    revision: row.worker.revision,
                },
            )
            .disabled(!s.dashboard_attach_available())
            .hint("Reload this thread’s live connection and requests without sending a prompt"),
        );
        panel = panel.child(buttons);
        if let Some(reason) = &c.execution_blocked {
            panel = panel.child(
                div()
                    .text_color(cx.theme().colors.warning)
                    .child(reason.clone()),
            );
        }
        if let Some(error) = &c.status_error {
            panel = panel.child(
                div()
                    .text_color(cx.theme().colors.danger)
                    .child(error.clone()),
            );
        }
        if s.dashboard.attachment == Some((id, c.generation)) {
            if let Some(error) = &s.dashboard.attachment_error {
                panel = panel.child(
                    div()
                        .text_color(cx.theme().colors.danger)
                        .child(error.clone()),
                );
            } else if !s.dashboard_attachment_ready(id) {
                panel = panel.child("Loading the thread’s live connection and requests…");
            }
        }
        panel.into_any_element()
    }
}
