//! Independent activity page. The history projection uses the ordinary transcript renderer.
mod cli;
#[cfg(test)]
mod controls_tests;
mod dialog;
mod graph;
mod graph_layout;
#[cfg(test)]
pub(in crate::shell) use graph_layout::tests::row as test_row;
mod presentation;

use super::controls::{Btn, BtnKind, chip, icon};
use crate::{
    model::Model,
    text::{ComposerEditor, ComposerEvent},
    theme::ActiveTheme,
    transcript::TranscriptView,
};
use gpui::{prelude::*, uniform_list, *};
use pipkin_core::{
    dashboard::{self as d, Action as Control, Command as D},
    *,
};

pub(super) struct DashboardView {
    focus: FocusHandle,
    modal_focus: FocusHandle,
    modal_open: bool,
    modal_history: bool,
    request_choice: Option<(String, UiAnswer)>,
    graph: graph_layout::GraphState,
    model: Entity<Model>,
    projection: Entity<Model>,
    transcript: Entity<TranscriptView>,
    editor: Entity<ComposerEditor>,
    answer_editor: Entity<ComposerEditor>,
    answer_key: Option<(String, String)>,
    answer_drafts: std::collections::BTreeMap<(String, String), String>,
    search: Entity<ComposerEditor>,
    scroll: UniformListScrollHandle,
    filters_open: bool,
    about_open: bool,
    details_open: bool,
    output: Option<(String, Result<String, String>)>,
    selected: Option<String>,
    draft_key: (Option<String>, u64),
    _subs: Vec<Subscription>,
}
impl EventEmitter<super::dashboard_cli::NewThread> for DashboardView {}
impl EventEmitter<super::dashboard_cli::Request> for DashboardView {}
impl DashboardView {
    pub fn new(model: Entity<Model>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = AppState::new(
            Bootstrap {
                projects: vec![],
                models: vec![],
                conversations: vec![],
                now: 0,
            },
            Prefs::default(),
        );
        let projection = cx.new(|_| Model::new(state));
        let source = model.clone();
        projection.update(cx, |m, _| {
            m.set_effect_handler(Box::new(move |effect, cx| match effect {
                Effect::Backend(BackendRequest::LoadOlder { .. }) => {
                    source.update(cx, |m, cx| m.dispatch(Command::Dashboard(D::Older), cx));
                }
                Effect::Backend(BackendRequest::FetchToolOutput { call_id, .. }) => source
                    .update(cx, |m, cx| {
                        m.dispatch(Command::Dashboard(D::ToolOutput(call_id)), cx)
                    }),
                Effect::CopyText(_) | Effect::SaveText { .. } | Effect::Launch(_) => {
                    source.update(cx, |m, cx| {
                        m.mutate(
                            |_| Outcome {
                                effects: vec![effect],
                                notes: vec![],
                            },
                            cx,
                        )
                    })
                }
                _ => {}
            }))
        });
        let transcript = cx.new(|cx| TranscriptView::new(projection.clone(), window, cx));
        let editor = cx.new(|cx| ComposerEditor::new(window, cx));
        editor.update(cx, |e, cx| {
            e.set_label("Message to selected task", cx);
            e.set_placeholder("Message this task…");
        });
        let answer_editor = cx.new(|cx| ComposerEditor::new(window, cx));
        answer_editor.update(cx, |e, cx| {
            e.set_label("Answer to selected question", cx);
            e.set_placeholder("Your answer…");
        });
        let search = cx.new(|cx| ComposerEditor::single_line(window, cx));
        search.update(cx, |e, cx| {
            e.set_label("Filter workers", cx);
            e.set_placeholder("Find a thread…");
        });
        let subs = vec![
            cx.subscribe_in(
                &answer_editor,
                window,
                |this, _, event, window, cx| match event {
                    ComposerEvent::Changed => {
                        if let Some(key) = &this.answer_key {
                            this.answer_drafts
                                .insert(key.clone(), this.answer_editor.read(cx).text());
                        }
                        cx.notify();
                    }
                    ComposerEvent::Submit => this.submit_reply(cx),
                    ComposerEvent::Escape => this.close_flyup(window, cx),
                    _ => {}
                },
            ),
            cx.observe_in(&model, window, |this, _, _, cx| this.sync(cx)),
            cx.subscribe_in(&editor, window, |this, _, event, window, cx| match event {
                ComposerEvent::Changed => {
                    let text = this.editor.read(cx).text();
                    this.send(D::Edit(text), cx);
                }
                ComposerEvent::Submit => {
                    this.submit_reply(cx);
                }
                ComposerEvent::Escape => {
                    if this.modal_open {
                        this.close_flyup(window, cx);
                    } else {
                        this.send(D::Back, cx);
                        this.focus_list(window, cx);
                    }
                }
                _ => {}
            }),
            cx.subscribe(&search, |this, _, event, cx| {
                if matches!(event, ComposerEvent::Changed) {
                    let text = this.search.read(cx).text();
                    let mut filter = this.model.read(cx).state.dashboard.filter.clone();
                    filter.text = (!text.trim().is_empty()).then_some(text);
                    this.send(D::Filter(filter), cx);
                }
            }),
        ];
        let mut view = Self {
            focus: cx.focus_handle(),
            modal_focus: cx.focus_handle(),
            modal_open: false,
            modal_history: false,
            request_choice: None,
            graph: graph_layout::GraphState::default(),
            model,
            projection,
            transcript,
            editor,
            answer_editor,
            answer_key: None,
            answer_drafts: Default::default(),
            search,
            scroll: UniformListScrollHandle::new(),
            filters_open: false,
            about_open: false,
            details_open: false,
            output: None,
            selected: None,
            draft_key: (None, 0),
            _subs: subs,
        };
        view.sync(cx);
        view
    }
    pub(super) fn focus_list(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
    }
    fn navigate(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = event.keystroke.modifiers;
        if event.keystroke.key != "tab" || m.control || m.alt || m.platform {
            return;
        }
        // A composing editor must retain its marked range, including when Tab is pressed.
        if self.editor.read(cx).is_composing()
            || self.answer_editor.read(cx).is_composing()
            || self.search.read(cx).is_composing()
        {
            cx.stop_propagation();
            return;
        }
        // This page owns its own tab sequence; never land on the conversation behind it.
        for _ in 0..512 {
            if m.shift {
                window.focus_prev(cx);
            } else {
                window.focus_next(cx);
            }
            if if self.modal_open {
                self.modal_focus.contains_focused(window, cx)
            } else {
                self.focus.contains_focused(window, cx)
            } {
                window.prevent_default();
                cx.stop_propagation();
                return;
            }
        }
        self.focus_list(window, cx);
        cx.stop_propagation();
    }
    fn step(&self, forward: bool, cx: &mut Context<Self>) {
        let d = &self.model.read(cx).state.dashboard;
        let at = d
            .selected
            .as_ref()
            .and_then(|s| d.rows.iter().position(|r| &r.worker.session == s));
        let at = match (at, forward) {
            (Some(i), true) => (i + 1).min(d.rows.len().saturating_sub(1)),
            (Some(i), false) => i.saturating_sub(1),
            _ => 0,
        };
        if let Some(row) = d.rows.get(at) {
            self.send(D::Select(row.worker.session.clone()), cx);
        }
    }
    pub(super) fn focus_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.graph.list && !self.modal_open {
            if let Some(id) = self.selected.clone() {
                self.open_flyup(id, window, cx);
            }
            return;
        }
        let question = self
            .model
            .read(cx)
            .state
            .dashboard
            .selected_row()
            .filter(|row| requires_request_ui(row))
            .and_then(|row| self.current_request(row, cx));
        if question.is_some_and(|q| q.kind == UiRequestKind::Input) {
            window.focus(&self.answer_editor.read(cx).focus_handle(cx), cx);
        } else if self
            .model
            .read(cx)
            .state
            .dashboard
            .selected_row()
            .is_some_and(requires_request_ui)
        {
            window.focus(
                if self.graph.list {
                    &self.focus
                } else {
                    &self.modal_focus
                },
                cx,
            );
        } else {
            window.focus(&self.editor.read(cx).focus_handle(cx), cx);
        }
    }
    pub(super) fn focus_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.graph.list && !self.modal_open {
            if let Some(id) = self.selected.clone() {
                self.open_flyup(id, window, cx);
            } else {
                return;
            }
        }
        self.modal_history = true;
        cx.notify();
        window.focus(&self.transcript.read(cx).focus_handle(cx), cx);
    }
    fn send(&self, command: D, cx: &mut Context<Self>) {
        self.model
            .update(cx, |m, cx| m.dispatch(Command::Dashboard(command), cx));
    }
    pub(super) fn act(&self, action: Control, cx: &mut Context<Self>) {
        let state = &self.model.read(cx).state;
        let Some(row) = state.dashboard.selected_row() else {
            return;
        };
        if requires_request_ui(row) {
            return;
        }
        self.send(
            D::Act {
                session: row.worker.session.clone(),
                generation: state.dashboard.detail.generation,
                revision: row.worker.revision,
                draft_revision: state.dashboard_draft().1,
                action,
            },
            cx,
        );
    }
    fn sync(&mut self, cx: &mut Context<Self>) {
        let state = &self.model.read(cx).state;
        self.graph.sync(&state.dashboard);
        let dashboard_open = state.dashboard.open;
        let detail = state.dashboard.detail.clone();
        let selected = state.dashboard.selected.clone();
        let prefs = state.prefs.clone();
        let search_text = state.dashboard.filter.text.clone().unwrap_or_default();
        let answer_key = state.dashboard.selected_row().and_then(|r| {
            r.conversation
                .and_then(|id| state.conversation(id))
                .and_then(|c| c.ui_requests.first())
                .map(|q| (r.worker.session.clone(), q.id.clone()))
        });
        let (text, rev) = state.dashboard_draft();
        if self.search.read(cx).text() != search_text {
            self.search.update(cx, |e, cx| e.set_text(&search_text, cx));
        }
        let key = (selected.clone(), rev);
        if key != self.draft_key {
            self.draft_key = key;
            self.editor.update(cx, |e, cx| e.set_text(&text, cx));
        }
        if answer_key != self.answer_key {
            self.answer_key = answer_key;
            let answer = self
                .answer_key
                .as_ref()
                .and_then(|k| self.answer_drafts.get(k))
                .cloned()
                .unwrap_or_default();
            self.answer_editor
                .update(cx, |e, cx| e.set_text(&answer, cx));
        }
        let changed = self.selected != selected;
        if changed {
            self.modal_history = false;
        }
        self.selected = selected;
        if self.selected.is_none() || !dashboard_open {
            self.modal_open = false;
        }
        let output = detail.output.clone();
        let generation = detail.generation;
        self.projection.update(cx, |m, cx| {
            m.mutate(
                |s| {
                    s.prefs = prefs;
                    let id = ConversationId(1);
                    if changed || s.conversations.is_empty() {
                        s.conversations = vec![ConversationState::new(
                            id,
                            ProjectId(1),
                            "Worker history".into(),
                            0,
                        )];
                        s.selected = Some(id);
                    }
                    let c = &mut s.conversations[0];
                    let different = c.items != detail.items;
                    c.generation = detail.generation;
                    c.items = detail.items;
                    c.opened = true;
                    c.has_older = detail.has_older;
                    c.loading_older = detail.loading;
                    let mut out = Outcome::note(Note::Other);
                    if changed {
                        out.notes.push(Note::SelectionChanged);
                    }
                    if different {
                        out.notes.push(Note::ItemsReset(id));
                    }
                    out
                },
                cx,
            )
        });
        if changed {
            self.request_choice = None;
            if let Some(id) = &self.selected {
                self.graph.reveal(id);
            }
            self.details_open = false;
            self.output = None;
        }
        if output != self.output {
            self.output = output.clone();
            if let Some((call_id, result)) = output {
                self.projection.update(cx, |m, cx| {
                    m.apply_event(
                        BackendEvent {
                            conversation: ConversationId(1),
                            generation,
                            op: None,
                            kind: match result {
                                Ok(text) => EventKind::ToolOutputFull { call_id, text },
                                Err(reason) => EventKind::ToolOutputUnavailable { call_id, reason },
                            },
                        },
                        cx,
                    )
                });
            }
        }
        cx.notify();
    }
    fn button(&self, id: &'static str, label: impl Into<SharedString>, command: D) -> Btn {
        let model = self.model.clone();
        Btn::new(id)
            .label(label)
            .tab_stop(true)
            .on_click(move |_, cx| {
                model.update(cx, |m, cx| {
                    m.dispatch(Command::Dashboard(command.clone()), cx)
                })
            })
    }
    fn icon_button(
        &self,
        id: &'static str,
        symbol: &'static str,
        label: &'static str,
        command: D,
    ) -> Btn {
        let model = self.model.clone();
        Btn::new(id)
            .icon(symbol)
            .aria(label)
            .hint(label)
            .tab_stop(true)
            .on_click(move |_, cx| {
                model.update(cx, |m, cx| {
                    m.dispatch(Command::Dashboard(command.clone()), cx)
                })
            })
    }
    fn control(&self, id: &'static str, label: &'static str, action: Control, cx: &App) -> Btn {
        let state = &self.model.read(cx).state;
        let Some(row) = state.dashboard.selected_row() else {
            return Btn::new(id).label(label).disabled(true);
        };
        let unavailable = state.dashboard_action_unavailable(action);
        self.button(
            id,
            label,
            D::Act {
                session: row.worker.session.clone(),
                generation: state.dashboard.detail.generation,
                revision: row.worker.revision,
                draft_revision: state.dashboard_draft().1,
                action,
            },
        )
        .disabled(unavailable.is_some())
        .hint(unavailable.unwrap_or(label))
        .kind(if action == Control::Stop {
            BtnKind::Danger
        } else {
            BtnKind::Subtle
        })
    }
}
fn requires_request_ui(row: &d::Row) -> bool {
    matches!(
        row.worker.state,
        d::WorkState::Waiting {
            reason: d::WaitReason::Question | d::WaitReason::CliApproval
        }
    )
}

// Compact labels live in the presentation layer; the engine's full reason remains available.
fn status(state: &d::WorkState, c: &crate::theme::Colors) -> (&'static str, &'static str, Hsla) {
    match state {
        d::WorkState::Running => ("Running", "loader-circle", c.activity),
        d::WorkState::Pending | d::WorkState::Queued => ("Queued", "clock", c.text_muted),
        d::WorkState::Waiting {
            reason: d::WaitReason::Question | d::WaitReason::CliApproval,
        } => ("Needs you", "circle-alert", c.accent),
        d::WorkState::Waiting { .. } => ("Waiting", "clock", c.text_muted),
        d::WorkState::Stopping => ("Stopping", "square", c.warning),
        d::WorkState::Completing => ("Finishing", "loader-circle", c.activity),
        d::WorkState::Ready => ("Ready", "clock", c.text_muted),
        d::WorkState::Completed => ("Done", "check", c.success),
        d::WorkState::Failed { .. } => ("Failed", "circle-alert", c.danger),
        d::WorkState::Stopped => ("Stopped", "square", c.text_muted),
        d::WorkState::Interrupted => ("Interrupted", "triangle-alert", c.warning),
        d::WorkState::Blocked { .. } => ("Blocked", "circle-alert", c.warning),
        d::WorkState::Unknown => ("Unknown", "circle-help", c.warning),
    }
}
fn project_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(path)
        .to_owned()
}
impl DashboardView {
    fn render_list(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let c = theme.colors.clone();
        let state = &self.model.read(cx).state;
        let projects = state.projects.clone();
        let dash = state.dashboard.clone();
        let selected = dash.selected_row().cloned();
        let narrow = window.viewport_size().width < px(1100.0);
        let show_directory = !narrow || selected.is_none();
        let live_color = if dash.freshness == d::Freshness::Live {
            c.success
        } else {
            c.warning
        };
        let view = cx.entity();
        let mut page = div()
            .id("orchestration-page")
            .key_context("Orchestration")
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(Self::navigate))
            .on_action(
                cx.listener(|this, _: &super::actions::DashboardNext, _, cx| this.step(true, cx)),
            )
            .on_action(
                cx.listener(|this, _: &super::actions::DashboardPrevious, _, cx| {
                    this.step(false, cx)
                }),
            )
            .role(Role::Main)
            .aria_label("Orchestration")
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .bg(c.bg_app)
            .text_color(c.text)
            .child(self.view_switch(cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(20.))
                    .py(px(14.))
                    .child(icon("panel-right", px(20.), c.accent))
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Orchestration"),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(theme.small_size())
                            .text_color(c.text_muted)
                            .child(div().size(px(6.)).rounded_full().bg(live_color))
                            .child(dash.freshness.label()),
                    )
                    .child(
                        Btn::new("dashboard-coverage")
                            .icon("circle-help")
                            .aria("About this view")
                            .hint("About this view")
                            .tab_stop(true)
                            .selected(self.about_open)
                            .on_click(move |_, cx| {
                                view.update(cx, |v, cx| {
                                    v.about_open = !v.about_open;
                                    cx.notify();
                                })
                            }),
                    )
                    .child(self.icon_button(
                        "refresh-workers",
                        "refresh-cw",
                        "Refresh tasks",
                        D::Refresh,
                    ))
                    .child(self.icon_button(
                        "return-chat",
                        "x",
                        "Return to conversation · Ctrl+Shift+A",
                        D::Close,
                    )),
            );
        if self.about_open {
            let counts = dash.counts.unwrap_or_default();
            page = page.child(div().mx(px(20.)).mb(px(12.)).p(px(12.)).rounded(px(8.)).bg(c.bg_pane)
                .flex().flex_col().gap(px(6.)).text_size(theme.small_size()).text_color(c.text_muted)
                .child(format!("{} tasks · {} finished in 24 h · {} background jobs", counts.total, counts.finished_recent, counts.jobs_active))
                .child("Pipkin conversations and child tasks. CLI-internal workers and agents outside Pipkin are not included. Jobs are related work, not agents."));
        }
        if let Some(error) = dash.error.clone() {
            page = page.child(
                div()
                    .mx(px(20.))
                    .mb(px(8.))
                    .p(px(10.))
                    .rounded(px(8.))
                    .bg(c.danger_bg)
                    .text_color(c.danger)
                    .child(error),
            );
        }
        if !dash.available {
            return page
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(12.))
                        .child(icon("refresh-cw", px(28.), c.text_muted))
                        .child("Connecting to your workspace…"),
                )
                .into_any_element();
        }
        if show_directory {
            let counts = dash.counts.unwrap_or_default();
            let mut tabs = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(4.))
                .px(px(20.))
                .pb(px(12.));
            for (id, label, group, count) in [
                ("overview", "Overview", None, 0),
                (
                    "running",
                    "Working",
                    Some(d::ActivityGroup::Running),
                    counts.running,
                ),
                (
                    "attention",
                    "Needs you",
                    Some(d::ActivityGroup::Attention),
                    dash.attention(),
                ),
                ("finished", "Finished", Some(d::ActivityGroup::Finished), 0),
                ("all", "All", Some(d::ActivityGroup::All), 0),
            ] {
                let label = if count > 0 {
                    format!("{label}  {count}")
                } else {
                    label.into()
                };
                tabs = tabs.child(
                    self.button(id, label, D::Group(group))
                        .selected(dash.group == group),
                );
            }
            page = page.child(tabs);
        }
        let mut body = div()
            .flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .border_t_1()
            .border_color(c.border);
        if show_directory {
            let filter = dash.filter.clone();
            let active_filters = usize::from(filter.kind.is_some())
                + usize::from(filter.backend.is_some())
                + usize::from(filter.cwd.is_some())
                + usize::from(filter.failed);
            let view = cx.entity();
            let mut rail = div()
                .flex()
                .flex_col()
                .min_h_0()
                .min_w_0()
                .when(selected.is_some(), |d| {
                    d.w(px(300.))
                        .flex_shrink_0()
                        .border_r_1()
                        .border_color(c.border)
                })
                .when(selected.is_none(), |d| d.flex_1())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .p(px(12.))
                        .child(div().flex_1().min_w_0().child(self.search.clone()))
                        .child(
                            Btn::new("toggle-worker-filters")
                                .icon("settings")
                                .aria("Filter tasks")
                                .hint("Filter tasks")
                                .selected(self.filters_open || active_filters > 0)
                                .tab_stop(true)
                                .on_click(move |_, cx| {
                                    view.update(cx, |v, cx| {
                                        v.filters_open = !v.filters_open;
                                        cx.notify();
                                    })
                                }),
                        ),
                );
            if self.filters_open {
                let mut filters = div().flex().flex_wrap().gap(px(4.)).px(px(12.)).pb(px(12.));
                let mut f = filter.clone();
                f.kind = match filter.kind {
                    None => Some(d::WorkerKind::Root),
                    Some(d::WorkerKind::Root) => Some(d::WorkerKind::Child),
                    _ => None,
                };
                filters = filters.child(
                    self.button(
                        "kind-filter",
                        match filter.kind {
                            None => "All tasks",
                            Some(d::WorkerKind::Root) => "Conversations",
                            _ => "Children",
                        },
                        D::Filter(f),
                    )
                    .compact()
                    .hint("Cycle task type"),
                );
                let mut f = filter.clone();
                f.backend = match filter.backend {
                    None => Some(d::ExecutionBackend::NativeAgent),
                    Some(d::ExecutionBackend::NativeAgent) => {
                        Some(d::ExecutionBackend::ClaudeCliAgent)
                    }
                    Some(d::ExecutionBackend::ClaudeCliAgent) => {
                        Some(d::ExecutionBackend::LocalClaudeCli)
                    }
                    _ => None,
                };
                filters = filters.child(
                    self.button(
                        "backend-filter",
                        match filter.backend {
                            None => "All engines",
                            Some(d::ExecutionBackend::NativeAgent) => "Native",
                            Some(d::ExecutionBackend::ClaudeCliAgent) => "Claude orchestration",
                            Some(d::ExecutionBackend::LocalClaudeCli) => "Local CLI",
                            _ => "Unsupported",
                        },
                        D::Filter(f),
                    )
                    .compact()
                    .hint("Cycle engine"),
                );
                let mut paths: Vec<_> = projects.iter().map(|p| p.path.clone()).collect();
                paths.sort();
                paths.dedup();
                let next = filter
                    .cwd
                    .as_ref()
                    .and_then(|p| paths.iter().position(|x| x == p))
                    .map_or(0, |i| i + 1);
                let mut f = filter.clone();
                f.cwd = paths.get(next).cloned();
                filters = filters.child(
                    self.button(
                        "project-filter",
                        filter
                            .cwd
                            .as_ref()
                            .map_or("All projects".into(), |p| project_name(p)),
                        D::Filter(f),
                    )
                    .compact()
                    .hint(filter.cwd.clone().unwrap_or("Cycle project".into())),
                );
                let mut f = filter.clone();
                f.failed = !f.failed;
                filters = filters.child(
                    self.button("failure-filter", "Failures", D::Filter(f))
                        .compact()
                        .selected(filter.failed),
                );
                rail = rail.child(filters);
            }
            // Keep active filters visible even when their controls are folded away.
            if active_filters > 0 || filter.text.is_some() {
                let search = self.search.clone();
                let model = self.model.clone();
                rail = rail.child(
                    div()
                        .flex()
                        .items_center()
                        .px(px(12.))
                        .pb(px(8.))
                        .child(
                            div()
                                .flex_1()
                                .text_size(theme.small_size())
                                .text_color(c.text_muted)
                                .child("Filtered results"),
                        )
                        .child(
                            Btn::new("clear-filters")
                                .label("Clear")
                                .compact()
                                .tab_stop(true)
                                .on_click(move |_, cx| {
                                    search.update(cx, |e, cx| e.set_text("", cx));
                                    model.update(cx, |m, cx| {
                                        m.dispatch(
                                            Command::Dashboard(D::Filter(Default::default())),
                                            cx,
                                        )
                                    });
                                }),
                        ),
                );
            }
            let rows = dash.rows.clone();
            let len = rows.len();
            let source = self.model.clone();
            let colors = c.clone();
            let small_size = theme.small_size();
            let row_height = px(76. * theme.scale.max(1.));
            let list = uniform_list(
                "worker-list",
                len,
                cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
                    range
                        .map(|i| {
                            let row = &rows[i];
                            let session = row.worker.session.clone();
                            let selected =
                                source.read(cx).state.dashboard.selected.as_ref() == Some(&session);
                            let model = source.clone();
                            let c = &colors;
                            let (label, symbol, color) = status(&row.worker.state, c);
                            div()
                                .id(("worker", i))
                                .role(Role::Button)
                                .aria_label(format!(
                                    "{} · {} · {} · {}",
                                    if row.worker.kind == d::WorkerKind::Child {
                                        "Child"
                                    } else {
                                        "Conversation"
                                    },
                                    row.title(),
                                    row.label(),
                                    row.worker.cwd
                                ))
                                .on_click(move |_, _, cx| {
                                    model.update(cx, |m, cx| {
                                        m.dispatch(
                                            Command::Dashboard(D::Select(session.clone())),
                                            cx,
                                        )
                                    })
                                })
                                .h(row_height)
                                .px(px(8.))
                                .py(px(3.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .size_full()
                                        .gap(px(10.))
                                        .px(px(12.))
                                        .rounded(px(9.))
                                        .bg(if selected {
                                            c.bg_selected
                                        } else {
                                            gpui::transparent_black()
                                        })
                                        .border_l_2()
                                        .border_color(if selected {
                                            c.accent
                                        } else {
                                            gpui::transparent_black()
                                        })
                                        .hover(|s| s.bg(c.bg_hover))
                                        .cursor_pointer()
                                        .child(icon(symbol, px(16.), color))
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .flex_1()
                                                .min_w_0()
                                                .gap(px(5.))
                                                .child(
                                                    div()
                                                        .truncate()
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .child(
                                                            row.title()
                                                                .lines()
                                                                .next()
                                                                .unwrap_or_default()
                                                                .to_owned(),
                                                        ),
                                                )
                                                .child(
                                                    div()
                                                        .flex()
                                                        .items_center()
                                                        .gap(px(6.))
                                                        .text_size(small_size)
                                                        .text_color(c.text_muted)
                                                        .when(
                                                            row.worker.kind == d::WorkerKind::Child,
                                                            |d| {
                                                                d.child(icon(
                                                                    "zap",
                                                                    px(12.),
                                                                    c.text_faint,
                                                                ))
                                                            },
                                                        )
                                                        .child(
                                                            div().truncate().child(project_name(
                                                                &row.worker.cwd,
                                                            )),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_color(c.text_faint)
                                                                .child("·"),
                                                        )
                                                        .child(
                                                            div()
                                                                .flex_shrink_0()
                                                                .text_color(color)
                                                                .child(label),
                                                        ),
                                                ),
                                        ),
                                )
                        })
                        .collect()
                }),
            )
            .track_scroll(&self.scroll)
            .w_full()
            .flex_1();
            if len == 0 {
                rail = rail.child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(12.))
                        .p(px(24.))
                        .child(icon(
                            if dash.loading {
                                "loader-circle"
                            } else {
                                "circle-check"
                            },
                            px(28.),
                            c.text_faint,
                        ))
                        .child(div().text_color(c.text_muted).child(if dash.loading {
                            "Loading…"
                        } else if filter.text.is_some() || active_filters > 0 {
                            "No matching tasks"
                        } else {
                            "All clear"
                        })),
                );
            } else {
                rail = rail.child(list);
            }
            if dash.next.is_some() {
                rail = rail.child(
                    self.button("more-workers", "Load more", D::More)
                        .disabled(dash.loading),
                );
            }
            body = body.child(rail);
        }
        if let Some(row) = selected {
            body = body.child(self.thread_detail(row, narrow, cx));
        }
        page.child(body).into_any_element()
    }
    fn thread_detail(&self, row: d::Row, narrow: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let c = theme.colors.clone();
        let state = &self.model.read(cx).state;
        let dash = state.dashboard.clone();
        let closed_roots = state.prefs.closed_conversations.clone();
        let (label, symbol, color) = status(&row.worker.state, &c);
        let view = cx.entity();
        let mut header = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(16.))
            .py(px(12.))
            .when(narrow, |d| {
                d.child(self.icon_button("back-workers", "panel-left", "Back to tasks", D::Back))
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(4.))
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(row.title().to_owned()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(theme.small_size())
                            .text_color(c.text_muted)
                            .child(icon("folder", px(12.), c.text_faint))
                            .child(div().truncate().child(project_name(&row.worker.cwd))),
                    ),
            )
            .child(chip(label, color, color.opacity(0.10), cx).child(icon(symbol, px(12.), color)))
            .child(self.icon_button(
                "latest-worker-history",
                "arrow-down",
                "Latest history",
                D::Select(row.worker.session.clone()),
            ))
            .child(
                Btn::new("worker-details")
                    .icon("ellipsis")
                    .aria("Task details")
                    .hint("Task details")
                    .selected(self.details_open)
                    .tab_stop(true)
                    .on_click(move |_, cx| {
                        view.update(cx, |v, cx| {
                            v.details_open = !v.details_open;
                            cx.notify();
                        })
                    }),
            );
        if let Some(root) = row.root {
            let model = self.model.clone();
            header = header.child(
                Btn::new("open-owner")
                    .icon("arrow-right")
                    .aria("Open conversation and requests")
                    .hint("Open conversation and requests")
                    .tab_stop(true)
                    .on_click(move |_, cx| {
                        model.update(cx, |m, cx| {
                            m.dispatch(Command::Dashboard(D::Close), cx);
                            m.dispatch(Command::SelectConversation(root), cx);
                        })
                    }),
            );
        }
        let mut detail = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .bg(if self.modal_open {
                c.bg_elevated
            } else {
                c.bg_surface
            })
            .when(!self.modal_open, |d| d.child(header));
        if self.details_open {
            let mut info = div()
                .mx(px(16.))
                .mb(px(8.))
                .p(px(12.))
                .rounded(px(8.))
                .bg(c.bg_pane)
                .flex()
                .flex_col()
                .gap(px(6.))
                .text_size(theme.small_size())
                .text_color(c.text_muted)
                .child(row.worker.cwd.clone())
                .child(
                    row.worker
                        .model
                        .clone()
                        .unwrap_or("Model unavailable".into()),
                )
                .child(row.label().to_owned());
            if let Some(parent) = &row.worker.parent {
                let name = dash
                    .rows
                    .iter()
                    .find(|r| &r.worker.session == parent)
                    .map(|r| r.title())
                    .unwrap_or("Owning conversation");
                info = info.child(format!("Child of {name}"));
            }
            if let Some(child) = row.worker.continues {
                info = info.child(format!("Follow-up to child {child}"));
            }
            if row.worker.children_active > 0 {
                info = info.child(format!("{} active child tasks", row.worker.children_active));
            }
            if row.worker.jobs_active > 0 {
                info = info.child(format!("{} background jobs", row.worker.jobs_active));
            }
            detail = detail.child(info);
        }
        // An actionable reason must remain visible without opening metadata.
        if let d::WorkState::Blocked { reason } = &row.worker.state {
            detail = detail.child(
                div()
                    .px(px(16.))
                    .pb(px(8.))
                    .text_color(c.warning)
                    .child(reason.clone()),
            );
        }
        if let Some(error) = &dash.error {
            detail = detail.child(div().px(px(16.)).text_color(c.danger).child(error.clone()));
        }
        if let Some(error) = &dash.detail.error {
            detail = detail.child(div().px(px(16.)).text_color(c.danger).child(error.clone()));
        }
        if let Some(error) = row
            .conversation
            .and_then(|id| state.conversation(id))
            .and_then(|c| c.intent_error.as_ref())
        {
            detail = detail.child(div().px(px(16.)).text_color(c.danger).child(error.clone()));
        }
        detail = detail.child(
            div()
                .flex_1()
                .w_full()
                .min_w_0()
                .min_h_0()
                .child(self.transcript.clone()),
        );
        if dash.detail.has_older {
            detail = detail.child(
                div().flex().justify_center().gap(px(8.)).child(
                    self.button("older-worker-history", "Earlier", D::Older)
                        .compact()
                        .disabled(dash.detail.loading),
                ),
            );
        }
        let closed = row.root.is_some_and(|c| closed_roots.contains(&c.0));
        if requires_request_ui(&row) {
            detail = detail.child(self.request_panel(&row, cx));
        } else if closed {
            detail = detail.child(
                div()
                    .p(px(16.))
                    .text_size(theme.small_size())
                    .text_color(c.text_muted)
                    .child("Reopen this conversation to continue."),
            );
        } else {
            let mut actions = div().flex().items_center().flex_wrap().gap(px(6.));
            if row.worker.run.as_ref().is_some_and(|r| r.phase.is_some())
                || (!row.finished_child() && row.worker.kind == d::WorkerKind::Child)
            {
                actions = actions.child(
                    self.control("stop-worker", "Stop", Control::Stop, cx)
                        .icon("square"),
                );
            }
            if dash.control.as_ref().is_some_and(|p| p.uncertain) {
                actions = actions.child(
                    self.button("check-worker-action", "Check status", D::Reconcile)
                        .icon("refresh-cw"),
                );
            }
            actions = actions.child(div().flex_1());
            if row.worker.kind == d::WorkerKind::Root {
                actions = actions
                    .child(
                        self.control("queue-worker", "Queue", Control::Queue, cx)
                            .kind(BtnKind::Ghost)
                            .hint("Queue a message for this conversation"),
                    )
                    .child(
                        self.control("steer-worker", "Steer", Control::Steer, cx)
                            .kind(BtnKind::Ghost)
                            .hint("Guide the current turn"),
                    )
                    .child(
                        self.control("send-worker", "Send", Control::Send, cx)
                            .kind(BtnKind::Primary)
                            .icon("arrow-up"),
                    );
            } else if row.finished_child() {
                actions = actions.child(
                    self.control("follow-child", "Follow up", Control::FollowUp, cx)
                        .kind(BtnKind::Primary)
                        .icon("arrow-up")
                        .hint("Start a linked child with the recorded context"),
                );
            } else {
                actions = actions.child(
                    self.control("steer-child", "Steer", Control::Steer, cx)
                        .kind(BtnKind::Primary)
                        .icon("arrow-up"),
                );
            }
            detail = detail.child(
                div()
                    .m(px(16.))
                    .mt(px(8.))
                    .p(px(12.))
                    .rounded(px(12.))
                    .border_1()
                    .border_color(c.border_strong)
                    .bg(c.bg_input)
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .max_h(px(160.))
                            .min_h(px(48.))
                            .child(self.editor.clone()),
                    )
                    .child(actions),
            );
        }
        detail.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::DashboardView;
    use crate::model::Model;
    use gpui::{AppContext as _, TestAppContext, VisualTestContext};
    use pipkin_core::{AppState, Bootstrap, Prefs, TextSize, Theme};
    use std::{cell::RefCell, rc::Rc};

    #[gpui::test]
    fn dashboard_keyboard_reaches_disclosures_without_dispatching_work(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::text::init(cx);
            cx.set_global(crate::theme::Theme::new(
                Theme::Dark,
                TextSize::Normal,
                true,
            ));
        });
        let effects = Rc::new(RefCell::new(Vec::new()));
        let captured = effects.clone();
        let window = cx.add_window(|window, cx| {
            let mut state = AppState::new(
                Bootstrap {
                    projects: vec![],
                    models: vec![],
                    conversations: vec![],
                    now: 0,
                },
                Prefs::default(),
            );
            state.dashboard.open = true;
            state.dashboard.available = true;
            let model = cx.new(|_| Model::new(state));
            model.update(cx, |m, _| {
                m.set_effect_handler(Box::new(move |e, _| captured.borrow_mut().push(e)))
            });
            DashboardView::new(model, window, cx)
        });
        let root = window.root(cx).unwrap();
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        root.update_in(cx, |v, w, cx| v.focus_list(w, cx));
        cx.run_until_parked();
        cx.simulate_keystrokes("tab enter");
        root.update(cx, |v, _| {
            assert!(v.about_open, "first control opens the coverage disclosure")
        });
        cx.simulate_keystrokes("enter");
        root.update(cx, |v, _| assert!(!v.about_open));
        for _ in 0..40 {
            cx.simulate_keystrokes("tab");
            root.update_in(cx, |v, w, cx| assert!(v.focus.contains_focused(w, cx)));
        }
        cx.simulate_keystrokes("shift-tab");
        root.update_in(cx, |v, w, cx| assert!(v.focus.contains_focused(w, cx)));
        assert!(
            effects.borrow().is_empty(),
            "navigation must never dispatch work"
        );
    }
    #[gpui::test]
    fn flyup_traps_focus_and_keeps_target_drafts_on_dismissal(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::text::init(cx);
            cx.set_global(crate::theme::Theme::new(
                Theme::Dark,
                TextSize::Normal,
                true,
            ));
        });
        let effects = Rc::new(RefCell::new(Vec::new()));
        let captured = effects.clone();
        let window = cx.add_window(|window, cx| {
            let mut state = AppState::new(
                Bootstrap {
                    projects: vec![],
                    models: vec![],
                    conversations: vec![],
                    now: 0,
                },
                Prefs::default(),
            );
            state.dashboard.open = true;
            state.dashboard.available = true;
            state.dashboard.rows = vec![
                super::graph_layout::tests::row(
                    "one",
                    "/app",
                    pipkin_core::dashboard::WorkState::Ready,
                ),
                super::graph_layout::tests::row(
                    "two",
                    "/other",
                    pipkin_core::dashboard::WorkState::Ready,
                ),
            ];
            let model = cx.new(|_| Model::new(state));
            model.update(cx, |m, _| {
                m.set_effect_handler(Box::new(move |e, _| captured.borrow_mut().push(e)))
            });
            DashboardView::new(model, window, cx)
        });
        let root = window.root(cx).unwrap();
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        root.update_in(cx, |v, w, cx| v.open_flyup("one".into(), w, cx));
        cx.run_until_parked();
        cx.simulate_input("keep this draft");
        for _ in 0..25 {
            cx.simulate_keystrokes("tab");
            root.update_in(cx, |v, w, cx| {
                assert!(v.modal_focus.contains_focused(w, cx))
            });
        }
        root.update_in(cx, |v, w, cx| {
            v.close_flyup(w, cx);
            assert_eq!(
                v.model.read(cx).state.dashboard_draft().0,
                "keep this draft"
            );
            v.open_flyup("two".into(), w, cx);
        });
        cx.run_until_parked();
        root.update(cx, |v, cx| assert_eq!(v.editor.read(cx).text(), ""));
        root.update_in(cx, |v, w, cx| v.close_flyup(w, cx));
        root.update_in(cx, |v, w, cx| v.open_flyup("one".into(), w, cx));
        cx.run_until_parked();
        root.update(cx, |v, cx| {
            assert_eq!(v.editor.read(cx).text(), "keep this draft")
        });
        assert!(!effects.borrow().iter().any(|e| matches!(
            e,
            pipkin_core::Effect::Backend(
                pipkin_core::BackendRequest::Submit { .. }
                    | pipkin_core::BackendRequest::UiRespond { .. }
            )
        )));
    }
    #[gpui::test]
    fn decision_options_are_keyboard_selectable_without_submitting_or_losing_context(
        cx: &mut TestAppContext,
    ) {
        use pipkin_core::*;
        cx.update(|cx| {
            crate::text::init(cx);
            cx.set_global(crate::theme::Theme::new(
                Theme::Dark,
                TextSize::Normal,
                true,
            ));
        });
        let effects = Rc::new(RefCell::new(Vec::new()));
        let captured = effects.clone();
        let window = cx.add_window(|w, cx| {
            let mut state = AppState::new(
                Bootstrap {
                    projects: vec![],
                    models: vec![],
                    conversations: vec![],
                    now: 0,
                },
                Prefs::default(),
            );
            state.dashboard.open = true;
            state.dashboard.available = true;
            state.dashboard.freshness = dashboard::Freshness::Live;
            let mut conversation =
                ConversationState::new(ConversationId(1), ProjectId(1), "Retry policy".into(), 0);
            conversation.opened = true;
            conversation.ui_requests.push(UiRequest {
                id: "retry".into(),
                kind: UiRequestKind::Select,
                title: "Which policy?".into(),
                message: None,
                items: vec![UiRequestItem {
                    value: "backoff".into(),
                    label: "Backoff".into(),
                    description: Some("With jitter".into()),
                }],
                placeholder: None,
                default_value: None,
                deadline: None,
                form: None,
                source: None,
            });
            let generation = conversation.generation;
            state.dashboard.attachment = Some((conversation.id, generation));
            state.conversations.push(conversation);
            state.apply_event(BackendEvent {
                conversation: ConversationId(1),
                generation,
                op: None,
                kind: EventKind::ConversationLifecycleReady(true),
            });
            let mut row = super::graph_layout::tests::row(
                "one",
                "/app",
                dashboard::WorkState::Waiting {
                    reason: dashboard::WaitReason::Question,
                },
            );
            row.conversation = Some(ConversationId(1));
            state.dashboard.rows = vec![
                row,
                super::graph_layout::tests::row("two", "/other", dashboard::WorkState::Ready),
            ];
            let model = cx.new(|_| Model::new(state));
            model.update(cx, |m, _| {
                m.set_effect_handler(Box::new(move |e, _| captured.borrow_mut().push(e)))
            });
            DashboardView::new(model, w, cx)
        });
        let root = window.root(cx).unwrap();
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        root.update_in(cx, |v, w, cx| v.open_flyup("one".into(), w, cx));
        cx.run_until_parked();
        cx.simulate_keystrokes("tab tab enter");
        root.update(cx, |v, _| {
            assert_eq!(
                v.request_choice,
                Some(("retry".into(), UiAnswer::Choice("backoff".into())))
            );
            assert!(!v.modal_history);
        });
        // Skip Send and open the history disclosure. The chosen answer stays a draft.
        cx.simulate_keystrokes("tab tab enter");
        root.update(cx, |v, _| {
            assert!(v.modal_history);
            assert!(v.request_choice.is_some());
        });
        assert!(
            !effects
                .borrow()
                .iter()
                .any(|e| matches!(e, Effect::Backend(BackendRequest::UiRespond { .. })))
        );
        root.update_in(cx, |v, w, cx| v.open_flyup("two".into(), w, cx));
        root.update(cx, |v, _| {
            assert!(v.request_choice.is_none());
            assert!(!v.modal_history);
        });
    }
}
