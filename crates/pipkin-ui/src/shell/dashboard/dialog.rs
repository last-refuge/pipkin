//! Centered thread dialog and guarded engine question responses.
use super::presentation::*;
use super::*;

impl DashboardView {
    pub(super) fn current_request(&self, row: &d::Row, cx: &App) -> Option<UiRequest> {
        if !matches!(
            row.worker.state,
            d::WorkState::Waiting {
                reason: d::WaitReason::Question
            }
        ) {
            return None;
        }
        row.conversation
            .and_then(|id| self.model.read(cx).state.conversation(id))
            .and_then(|c| c.ui_requests.first())
            .cloned()
    }
    pub(super) fn submit_reply(&self, cx: &mut Context<Self>) {
        let Some(row) = self.model.read(cx).state.dashboard.selected_row().cloned() else {
            return;
        };
        if requires_request_ui(&row) {
            let Some(q) = self.current_request(&row, cx) else {
                return;
            };
            let answer = if q.kind == UiRequestKind::Input {
                Some(UiAnswer::Text(self.answer_editor.read(cx).text()))
            } else {
                self.request_choice
                    .as_ref()
                    .filter(|(id, _)| id == &q.id)
                    .map(|(_, a)| a.clone())
            };
            if let Some(answer) = answer {
                self.send(
                    D::Answer {
                        session: row.worker.session,
                        revision: row.worker.revision,
                        generation: self.model.read(cx).state.dashboard.detail.generation,
                        request: q.id,
                        answer,
                    },
                    cx,
                );
            }
        } else {
            self.act(
                if row.finished_child() {
                    Control::FollowUp
                } else if row.worker.kind == d::WorkerKind::Child {
                    Control::Steer
                } else {
                    Control::Send
                },
                cx,
            );
        }
    }
    pub(super) fn request_panel(&self, row: &d::Row, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let c = &t.colors;
        let request = self.current_request(row, cx);
        let Some(q) = request.filter(|q| q.kind != UiRequestKind::Form) else {
            let s = &self.model.read(cx).state;
            let approval = matches!(
                row.worker.state,
                d::WorkState::Waiting {
                    reason: d::WaitReason::CliApproval
                }
            );
            return div()
                .p(px(16.))
                .flex().flex_col().gap(px(12.))
                .text_color(c.text_muted)
                .child(if approval { "Review the exact request using CLI approvals above. Closing the dialog leaves the request waiting." }
                    else { "Load this thread’s live requests to answer here." })
                .when(!approval, |panel| panel.child(self.button("dashboard-load-request", "Load thread requests", D::Attach {
                    session: row.worker.session.clone(), generation: s.dashboard.detail.generation, revision: row.worker.revision,
                }).disabled(!s.dashboard_attach_available())))
                .into_any_element();
        };
        let state = &self.model.read(cx).state;
        let conversation = row.conversation.and_then(|id| state.conversation(id));
        let pending = conversation.is_some_and(|c| c.ui_answering.contains(&q.id));
        let disabled = pending
            || state.read_only.is_some()
            || !state.connection.is_ready()
            || state.dashboard.freshness != d::Freshness::Live
            || state.dashboard.detail.error.is_some()
            || !state.dashboard_answer_ready()
            || row
                .root
                .is_some_and(|id| state.conversation_is_closed(id) || state.owned_elsewhere(id));
        let error = conversation.and_then(|c| c.ui_error.clone());
        let mut panel = div().p(px(24.)).flex().flex_col().gap(px(18.)).child(
            div()
                .text_size(px(18. * t.scale))
                .font_weight(FontWeight::SEMIBOLD)
                .child(q.title.clone()),
        );
        if !state.dashboard_attachment_ready(conversation.map_or(ConversationId(0), |c| c.id)) {
            panel = panel.child(
                self.button(
                    "dashboard-load-request",
                    "Load live request",
                    D::Attach {
                        session: row.worker.session.clone(),
                        generation: state.dashboard.detail.generation,
                        revision: row.worker.revision,
                    },
                )
                .disabled(!state.dashboard_attach_available()),
            );
        }
        if let Some(message) = &q.message {
            panel = panel.child(div().text_color(c.text_muted).child(message.clone()));
        }
        let options: Vec<_> = match q.kind {
            UiRequestKind::Select => q
                .items
                .iter()
                .map(|i| {
                    (
                        i.label.clone(),
                        i.description.clone(),
                        UiAnswer::Choice(i.value.clone()),
                    )
                })
                .collect(),
            UiRequestKind::Confirm => vec![
                ("Yes".into(), None, UiAnswer::Confirm(true)),
                ("No".into(), None, UiAnswer::Confirm(false)),
            ],
            _ => vec![],
        };
        let mut choices = div().flex().flex_wrap().gap(px(12.));
        for (i, (label, description, answer)) in options.into_iter().enumerate() {
            let view = cx.entity();
            let key_view = view.clone();
            let request_id = q.id.clone();
            let key_request = request_id.clone();
            let key_answer = answer.clone();
            let selected = self
                .request_choice
                .as_ref()
                .is_some_and(|(id, a)| id == &q.id && a == &answer);
            choices = choices.child(
                div()
                    .id(("question-choice", i))
                    .flex_1()
                    .min_w(px(200.))
                    .p(px(16.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(if selected { c.accent } else { c.border_strong })
                    .bg(if selected { c.accent_bg } else { c.bg_input })
                    .role(Role::Button)
                    .aria_label(format!(
                        "{label}{}",
                        if selected { ", selected" } else { "" }
                    ))
                    .when(!disabled, |d| d.tab_index(0).tab_stop(true))
                    .cursor_pointer()
                    .focus_visible(|s| s.border_color(c.accent))
                    .hover(|s| s.bg(c.bg_hover))
                    .on_click(move |_, _, cx| {
                        if !disabled {
                            view.update(cx, |v, cx| {
                                v.request_choice = Some((request_id.clone(), answer.clone()));
                                cx.notify();
                            });
                        }
                    })
                    .capture_key_down(move |event, w, cx| {
                        if !disabled
                            && !event.keystroke.modifiers.modified()
                            && matches!(event.keystroke.key.as_str(), "enter" | "space")
                        {
                            w.prevent_default();
                            if event.is_held {
                                cx.stop_propagation();
                                return;
                            }
                            key_view.update(cx, |v, cx| {
                                v.request_choice = Some((key_request.clone(), key_answer.clone()));
                                cx.notify();
                            });
                            cx.stop_propagation();
                        }
                    })
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(10.))
                            .child(
                                div()
                                    .mt(px(3.))
                                    .size(px(16.))
                                    .flex_none()
                                    .rounded_full()
                                    .border_1()
                                    .border_color(if selected { c.accent } else { c.text_muted })
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .when(selected, |d| d.child(status_dot(c.accent, 8.))),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(8.))
                                    .child(div().font_weight(FontWeight::MEDIUM).child(label))
                                    .when_some(description, |d, text| {
                                        d.child(
                                            div()
                                                .text_size(t.small_size())
                                                .text_color(c.text_muted)
                                                .child(text),
                                        )
                                    }),
                            ),
                    ),
            );
        }
        if q.kind == UiRequestKind::Input {
            panel = panel
                .child(div().font_weight(FontWeight::MEDIUM).child("Your reply"))
                .child(
                    div()
                        .p(px(14.))
                        .rounded(px(10.))
                        .border_1()
                        .border_color(c.border_strong)
                        .bg(c.bg_input)
                        .min_h(px(88.))
                        .max_h(px(180.))
                        .child(self.answer_editor.clone()),
                );
        } else {
            panel = panel.child(choices);
        }
        if let Some(error) = error {
            panel = panel.child(div().text_color(c.danger).child(error));
        }
        let missing = if q.kind == UiRequestKind::Input {
            self.answer_editor.read(cx).text().trim().is_empty()
        } else {
            self.request_choice
                .as_ref()
                .is_none_or(|(id, _)| id != &q.id)
        };
        let view = cx.entity();
        panel
            .child(
                div().flex().justify_end().child(
                    Btn::new("send-question-answer")
                        .label(if pending { "Sending…" } else { "Send answer" })
                        .icon("arrow-up")
                        .kind(BtnKind::Primary)
                        .disabled(disabled || missing)
                        .tab_stop(true)
                        .on_click(move |_, cx| view.update(cx, |v, cx| v.submit_reply(cx))),
                ),
            )
            .into_any_element()
    }
}

impl DashboardView {
    pub(super) fn flyup(
        &self,
        row: d::Row,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = cx.theme().clone();
        let c = &t.colors;
        let decision = requires_request_ui(&row) && !self.modal_history;
        let available = (f32::from(window.viewport_size().height) - 48.).max(120.);
        let (label, _, color) = status(&row.worker.state, c);
        let close = cx.entity();
        let backdrop = close.clone();
        let toggle = close.clone();
        let keep = close.clone();
        let state = &self.model.read(cx).state;
        let uncertain_control = state
            .dashboard
            .control
            .as_ref()
            .is_some_and(|p| p.uncertain);
        let mut dialog = div()
            .id("thread-sheet")
            .track_focus(&self.modal_focus)
            .role(Role::Dialog)
            .aria_label(format!("Reply to {}", row.title()))
            .occlude()
            .relative()
            .w_full()
            .max_w(px(if decision { 780. } else { 960. }))
            .max_h(px(available))
            .when(!decision, |d| d.h(px(available.min(820.))))
            .rounded(px(16.))
            .border_1()
            .border_color(c.border_strong)
            .bg(c.bg_elevated)
            .shadow_lg()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .p(px(24.))
                    .border_b_1()
                    .border_color(c.border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(icon("folder", px(15.), c.text_muted))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(t.small_size())
                                    .text_color(c.text_muted)
                                    .child(format!(
                                        "Connected workspace / {}",
                                        project_name(&row.worker.cwd)
                                    )),
                            )
                            .child(
                                Btn::new("close-thread-sheet")
                                    .icon("x")
                                    .aria("Close thread dialog")
                                    .tab_stop(true)
                                    .on_click(move |w, cx| {
                                        close.update(cx, |v, cx| v.close_flyup(w, cx))
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .flex_wrap()
                            .gap(px(12.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(24. * t.scale))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .line_clamp(2)
                                    .child(row.title().to_owned()),
                            )
                            .child(chip(label, color, color.opacity(0.10), cx)),
                    )
                    .child(
                        div()
                            .text_size(t.small_size())
                            .text_color(c.text_muted)
                            .child(format!(
                                "{} · Updated {}",
                                model_name(&row, state),
                                updated_age(&row, state.now())
                            )),
                    ),
            );
        dialog = dialog.child(self.cli_controls(&row, cx));
        if decision {
            let dash = &state.dashboard;
            dialog = dialog.child(
                div()
                    .id("decision-body")
                    .min_h_0()
                    .max_h(px((available - 240.).max(80.)))
                    .overflow_y_scroll()
                    .when_some(
                        dash.error.clone().or(dash.detail.error.clone()),
                        |d, error| {
                            d.child(
                                div()
                                    .px(px(24.))
                                    .pt(px(12.))
                                    .text_color(c.danger)
                                    .child(error),
                            )
                        },
                    )
                    .child(self.request_panel(&row, cx)),
            );
        } else {
            dialog = dialog.child(self.thread_detail(row.clone(), false, cx));
        }
        let mut footer = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .px(px(20.))
            .py(px(12.))
            .border_t_1()
            .border_color(c.border);
        if uncertain_control {
            footer = footer.child(self.button(
                "dialog-check-action",
                "Check action status",
                D::Reconcile,
            ));
        }
        if !decision {
            footer = footer.child(self.icon_button(
                "dialog-latest-history",
                "refresh-cw",
                "Refresh latest history",
                D::Select(row.worker.session.clone()),
            ));
        }
        if requires_request_ui(&row) {
            footer = footer.child(
                Btn::new("dialog-history")
                    .label(if self.modal_history {
                        "Back to decision"
                    } else {
                        "View conversation"
                    })
                    .icon("message-square")
                    .tab_stop(true)
                    .on_click(move |_, cx| {
                        toggle.update(cx, |v, cx| {
                            v.modal_history = !v.modal_history;
                            cx.notify();
                        })
                    }),
            );
        }
        if let Some(button) = self.open_owner_button(&row, "dialog-open-owner", "Open full thread")
        {
            footer = footer.child(button);
        }
        footer = footer.child(div().flex_1()).child(
            Btn::new("keep-waiting")
                .label("Close")
                .tab_stop(true)
                .on_click(move |w, cx| keep.update(cx, |v, cx| v.close_flyup(w, cx))),
        );
        dialog = dialog.child(footer);
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .p(px(20.))
            .child(
                div()
                    .id("thread-sheet-scrim")
                    .absolute()
                    .inset_0()
                    .bg(c.bg_app.opacity(0.50))
                    .occlude()
                    .on_click(move |_, w, cx| backdrop.update(cx, |v, cx| v.close_flyup(w, cx))),
            )
            .child(dialog)
            .into_any_element()
    }
}
