//! Shared dashboard presentation. Labels reflect loaded data, never inferred execution.
use super::*;

pub(super) fn model_name(row: &d::Row, state: &AppState) -> String {
    let Some(id) = row.worker.model.as_deref() else {
        return "Model unavailable".into();
    };
    state
        .models
        .iter()
        .find(|m| m.id == id)
        .map(|m| m.name.clone())
        .unwrap_or_else(|| id.rsplit('/').next().unwrap_or(id).to_owned())
}

pub(super) fn updated_age(row: &d::Row, now: i64) -> String {
    if row.worker.updated_at_ms <= 0 {
        return "—".into();
    }
    super::super::nav::relative_time(now, row.worker.updated_at_ms / 1000)
}

pub(super) fn activity(row: &d::Row, state: &AppState) -> String {
    if state.dashboard.completion_unread(row) {
        return "The prompt finished. Review the result or send next steps.".into();
    }
    if row.worker.completion_key().is_some() {
        return "Reviewed · waiting for next steps".into();
    }
    if requires_request_ui(row) {
        if let Some(q) = row
            .conversation
            .and_then(|id| state.conversation(id))
            .and_then(|c| c.ui_requests.first())
        {
            return q.title.clone();
        }
        return "Open thread dialog to review the request".into();
    }
    match &row.worker.state {
        d::WorkState::Failed {
            detail: Some(detail),
        } => detail.clone(),
        d::WorkState::Running if row.worker.children_active > 0 => {
            format!("{} child tasks in progress", row.worker.children_active)
        }
        d::WorkState::Running if row.worker.jobs_active > 0 => {
            format!("{} background jobs in progress", row.worker.jobs_active)
        }
        d::WorkState::Running => "Turn in progress".into(),
        d::WorkState::Completed => "Ready to review the result".into(),
        _ => row.label().to_owned(),
    }
}

pub(super) fn row_status(
    row: &d::Row,
    dash: &d::State,
    c: &crate::theme::Colors,
) -> (&'static str, &'static str, Hsla) {
    if dash.completion_unread(row) {
        ("Finished · unread", "circle-check", c.accent)
    } else if row.worker.completion_key().is_some() {
        ("Awaiting next steps", "check", c.text_muted)
    } else {
        status(&row.worker.state, c)
    }
}

pub(super) fn status_dot(color: Hsla, size: f32) -> Div {
    div().size(px(size)).flex_none().rounded_full().bg(color)
}

impl DashboardView {
    pub(super) fn review_button(&self, row: &d::Row, id: &'static str, cx: &App) -> Option<Btn> {
        let dash = &self.model.read(cx).state.dashboard;
        if !dash.completion_unread(row) {
            return None;
        }
        Some(
            self.button(
                id,
                "Mark reviewed",
                D::MarkReviewed {
                    session: row.worker.session.clone(),
                    generation: dash.detail.generation,
                    completion: row.worker.completion_key()?,
                },
            )
            .icon("check")
            .hint("Clear this result's unread highlight; the thread stays ready for next steps."),
        )
    }

    pub(super) fn open_owner_button(
        &self,
        row: &d::Row,
        id: &'static str,
        label: &'static str,
    ) -> Option<Btn> {
        let root = row.root?;
        let model = self.model.clone();
        Some(
            Btn::new(id)
                .label(if row.worker.kind == d::WorkerKind::Child {
                    "Open owning conversation"
                } else {
                    label
                })
                .icon("arrow-right")
                .tab_stop(true)
                .on_click(move |_, cx| {
                    model.update(cx, |m, cx| {
                        m.dispatch(Command::Dashboard(D::Close), cx);
                        m.dispatch(Command::SelectConversation(root), cx);
                    });
                }),
        )
    }
}

impl DashboardView {
    pub(super) fn navigation_rail(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = &cx.theme().colors;
        let view = cx.entity();
        div()
            .absolute()
            .left_0()
            .top_0()
            .bottom_0()
            .w(px(60.))
            .border_r_1()
            .border_color(c.border)
            .bg(c.bg_pane)
            .flex()
            .flex_col()
            .items_center()
            .gap(px(18.))
            .py(px(18.))
            .child(
                div()
                    .text_size(px(30.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(c.accent)
                    .child("P"),
            )
            .child(
                Btn::new("rail-map")
                    .icon("git-branch")
                    .aria("Workspace map")
                    .hint("Workspace map")
                    .selected(true)
                    .tab_stop(true)
                    .on_click(move |_, cx| {
                        view.update(cx, |v, cx| {
                            v.graph.list = false;
                            v.graph.zoom = None;
                            v.graph.pan = (0., 0.);
                            v.graph.locate = None;
                            cx.notify();
                        })
                    }),
            )
            .child(
                self.button("rail-chat", "", D::Close)
                    .icon("message-square")
                    .aria("Return to conversation")
                    .hint("Conversations"),
            )
            .child(
                Btn::new("rail-commands")
                    .icon("command")
                    .aria("Commands")
                    .hint("Commands")
                    .tab_stop(true)
                    .on_click(|w, cx| {
                        w.dispatch_action(Box::new(super::super::actions::OpenPalette), cx)
                    }),
            )
            .child(div().flex_1())
            .child(
                Btn::new("rail-settings")
                    .icon("settings")
                    .aria("Settings")
                    .hint("Settings")
                    .tab_stop(true)
                    .on_click(|w, cx| {
                        w.dispatch_action(Box::new(super::super::actions::OpenPreferences), cx)
                    }),
            )
            .into_any_element()
    }
}
