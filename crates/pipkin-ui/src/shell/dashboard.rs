//! Independent activity page. The history projection uses the ordinary transcript renderer.
use super::controls::{Btn, BtnKind};
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
    model: Entity<Model>,
    projection: Entity<Model>,
    transcript: Entity<TranscriptView>,
    editor: Entity<ComposerEditor>,
    search: Entity<ComposerEditor>,
    scroll: UniformListScrollHandle,
    output: Option<(String, Result<String, String>)>,
    selected: Option<String>,
    draft_key: (Option<String>, u64),
    _subs: Vec<Subscription>,
}
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
            e.set_label("Message to selected worker", cx);
            e.set_placeholder("Message this worker…");
        });
        let search = cx.new(|cx| ComposerEditor::single_line(window, cx));
        search.update(cx, |e, cx| {
            e.set_label("Filter workers", cx);
            e.set_placeholder("Search title, task, or project…");
        });
        let subs = vec![
            cx.observe_in(&model, window, |this, _, _, cx| this.sync(cx)),
            cx.subscribe_in(&editor, window, |this, _, event, window, cx| match event {
                ComposerEvent::Changed => {
                    let text = this.editor.read(cx).text();
                    this.send(D::Edit(text), cx);
                }
                ComposerEvent::Submit => {
                    let action = this.model.read(cx).state.dashboard.selected_row().map(|r| {
                        if r.finished_child() {
                            Control::FollowUp
                        } else if r.worker.kind == d::WorkerKind::Child {
                            Control::Steer
                        } else {
                            Control::Send
                        }
                    });
                    if let Some(action) = action {
                        this.act(action, cx);
                    }
                }
                ComposerEvent::Escape => {
                    this.send(D::Back, cx);
                    this.focus_list(window, cx);
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
            model,
            projection,
            transcript,
            editor,
            search,
            scroll: UniformListScrollHandle::new(),
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
    pub(super) fn focus_input(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.editor.read(cx).focus_handle(cx), cx);
    }
    pub(super) fn focus_history(&self, window: &mut Window, cx: &mut Context<Self>) {
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
        let detail = state.dashboard.detail.clone();
        let selected = state.dashboard.selected.clone();
        let prefs = state.prefs.clone();
        let (text, rev) = state.dashboard_draft();
        let key = (selected.clone(), rev);
        if key != self.draft_key {
            self.draft_key = key;
            self.editor.update(cx, |e, cx| e.set_text(&text, cx));
        }
        let changed = self.selected != selected;
        self.selected = selected;
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
    fn control(&self, id: &'static str, label: &'static str, action: Control, cx: &App) -> Btn {
        let state = &self.model.read(cx).state;
        let Some(row) = state.dashboard.selected_row() else {
            return Btn::new(id).label(label).disabled(true);
        };
        let closed = row.root.is_some_and(|c| state.conversation_is_closed(c));
        let disabled = closed
            || !state.connection.is_ready()
            || state.dashboard.freshness != d::Freshness::Live
            || state.dashboard.detail.error.is_some()
            || state.dashboard.control.is_some();
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
        .disabled(disabled)
        .kind(if action == Control::Stop {
            BtnKind::Danger
        } else {
            BtnKind::Subtle
        })
    }
}
impl Render for DashboardView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let c = theme.colors;
        let state = &self.model.read(cx).state;
        let projects = state.projects.clone();
        let closed_roots = state.prefs.closed_conversations.clone();
        let dash = state.dashboard.clone();
        let selected = dash.selected_row().cloned();
        let narrow = window.viewport_size().width < px(1100.0);
        let mut page = div()
            .id("orchestration-page")
            .key_context("Orchestration")
            .track_focus(&self.focus)
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
            .gap(px(8.));
        page = page.child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .p(px(12.))
                .child(div().text_lg().child("Orchestration"))
                .child(div().flex_1().child(dash.freshness.label()))
                .child(self.button("refresh-workers", "Refresh", D::Refresh))
                .child(self.button("return-chat", "Return to conversation", D::Close)),
        );
        if let Some(counts) = dash.counts {
            page=page.child(div().px(px(16.)).child(format!("{} workers · {} running · {} need attention · {} finished in 24 h · {} background jobs",counts.total,counts.running,dash.attention(),counts.finished_recent,counts.jobs_active)));
        }
        if let Some(error) = dash.error.clone() {
            page = page.child(div().px(px(16.)).text_color(c.danger).child(error));
        }
        if !dash.available {
            return page.child(div().p(px(16.)).child("Activity is unavailable while the engine connects. Cached state cannot authorize actions.")).into_any_element();
        }
        let mut tabs = div().flex().flex_wrap().gap(px(4.)).px(px(12.));
        for (id, label, group) in [
            ("overview", "Overview", None),
            ("running", "Running", Some(d::ActivityGroup::Running)),
            (
                "attention",
                "Needs attention",
                Some(d::ActivityGroup::Attention),
            ),
            ("finished", "Finished", Some(d::ActivityGroup::Finished)),
            ("all", "All", Some(d::ActivityGroup::All)),
        ] {
            tabs = tabs.child(
                self.button(id, label, D::Group(group))
                    .selected(dash.group == group),
            );
        }
        page = page.child(tabs);
        let filter = dash.filter.clone();
        let model = self.model.clone();
        let mut filters = div()
            .flex()
            .flex_wrap()
            .gap(px(4.))
            .px(px(12.))
            .child(div().w(px(240.)).child(self.search.clone()));
        let next_kind = match filter.kind {
            None => Some(d::WorkerKind::Root),
            Some(d::WorkerKind::Root) => Some(d::WorkerKind::Child),
            _ => None,
        };
        let mut f = filter.clone();
        f.kind = next_kind;
        filters = filters.child(self.button(
            "kind-filter",
            format!(
                "Kind: {}",
                match filter.kind {
                    None => "All",
                    Some(d::WorkerKind::Root) => "Conversations",
                    _ => "Children",
                }
            ),
            D::Filter(f),
        ));
        let next_backend = match filter.backend {
            None => Some(d::ExecutionBackend::NativeAgent),
            Some(d::ExecutionBackend::NativeAgent) => Some(d::ExecutionBackend::ClaudeCliAgent),
            Some(d::ExecutionBackend::ClaudeCliAgent) => Some(d::ExecutionBackend::LocalClaudeCli),
            _ => None,
        };
        let mut f = filter.clone();
        f.backend = next_backend;
        filters = filters.child(self.button(
            "backend-filter",
            format!(
                "Backend: {}",
                match filter.backend {
                    None => "All",
                    Some(d::ExecutionBackend::NativeAgent) => "Native",
                    Some(d::ExecutionBackend::ClaudeCliAgent) => "Claude orchestration",
                    Some(d::ExecutionBackend::LocalClaudeCli) => "Local CLI",
                    _ => "Unsupported",
                }
            ),
            D::Filter(f),
        ));
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
                    .map_or("All projects".to_owned(), |p| format!("Project: {p}")),
                D::Filter(f),
            ),
        );
        let mut f = filter.clone();
        f.failed = !f.failed;
        filters = filters.child(
            self.button("failure-filter", "Failures", D::Filter(f))
                .selected(filter.failed),
        );
        let search = self.search.clone();
        filters = filters.child(Btn::new("clear-filters").label("Clear filters").on_click(
            move |_, cx| {
                search.update(cx, |e, cx| e.set_text("", cx));
                model.update(cx, |m, cx| {
                    m.dispatch(Command::Dashboard(D::Filter(Default::default())), cx)
                });
            },
        ));
        page = page.child(filters);
        let mut body = div()
            .flex()
            .flex_1()
            .w_full()
            .min_w_0()
            .min_h_0()
            .gap(px(12.))
            .px(px(12.));
        if !narrow || selected.is_none() {
            let rows = dash.rows.clone();
            let len = rows.len();
            let source = self.model.clone();
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
                            div()
                                .id(("worker", i))
                                .role(Role::Button)
                                .aria_label(format!("{} · {}", row.title(), row.label()))
                                .flex()
                                .flex_col()
                                .h(px(88.))
                                .p(px(10.))
                                .border_b_1()
                                .border_color(c.border)
                                .bg(if selected { c.bg_pane } else { c.bg_app })
                                .cursor_pointer()
                                .on_click(move |_, _, cx| {
                                    model.update(cx, |m, cx| {
                                        m.dispatch(
                                            Command::Dashboard(D::Select(session.clone())),
                                            cx,
                                        )
                                    })
                                })
                                .child(div().truncate().child(
                                    row.title().lines().next().unwrap_or_default().to_owned(),
                                ))
                                .child(div().text_sm().child(format!(
                                    "{} · {}",
                                    if row.worker.kind == d::WorkerKind::Root {
                                        "Conversation"
                                    } else {
                                        "Child"
                                    },
                                    row.label()
                                )))
                                .child(
                                    div()
                                        .text_sm()
                                        .truncate()
                                        .text_color(c.text_muted)
                                        .child(row.worker.cwd.clone()),
                                )
                        })
                        .collect()
                }),
            )
            .track_scroll(&self.scroll)
            .w_full()
            .flex_1();
            let mut col = div().flex().flex_col().min_h_0().min_w_0();
            col = if selected.is_some() {
                col.w(px(320.)).flex_shrink_0()
            } else {
                col.flex_1()
            };
            col = col.child(list);
            if len == 0 {
                col = col.child(div().p(px(16.)).child(if dash.loading {
                    "Loading workers…"
                } else {
                    "No workers match these filters."
                }));
            }
            if dash.next.is_some() {
                col = col.child(
                    self.button("more-workers", "Load more workers", D::More)
                        .disabled(dash.loading),
                );
            }
            body = body.child(col);
        }
        if let Some(row) = selected {
            let mut detail = div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .gap(px(6.));
            detail = detail.child(
                div()
                    .flex()
                    .gap(px(6.))
                    .child(self.button("back-workers", "Back to workers", D::Back))
                    .child(div().flex_1().truncate().child(row.title().to_owned())),
            );
            detail = detail.child(div().text_sm().truncate().child(format!(
                "{} child tasks active · {} background jobs",
                row.worker.children_active, row.worker.jobs_active
            )));
            detail = detail.child(div().text_sm().truncate().child(format!(
                "{} · {} · {}",
                row.label(),
                row.worker.model.as_deref().unwrap_or("Model unavailable"),
                row.worker.cwd.as_str()
            )));
            if let Some(parent) = row.worker.parent.as_ref() {
                detail = detail.child(div().text_sm().truncate().child(format!(
                        "Parent: {parent}{}",
                        row.worker
                            .continues
                            .map_or(String::new(), |c| format!(" · Follows child {c}"))
                    )));
            }
            if let Some(error) = dash.detail.error.as_ref() {
                detail = detail.child(div().text_color(c.danger).child(error.clone()));
            }
            detail = detail.child(self.button(
                "latest-worker-history",
                "Latest history",
                D::Select(row.worker.session.clone()),
            ));
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
                    self.button("older-worker-history", "Load older history", D::Older)
                        .disabled(dash.detail.loading),
                );
            }
            let closed = row.root.is_some_and(|c| closed_roots.contains(&c.0));
            if closed {
                detail=detail.child(div().child("This conversation is closed. Reopen it from Closed conversations to send work."));
            } else {
                detail = detail.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .max_h(px(180.))
                        .min_h(px(60.))
                        .child(self.editor.clone()),
                );
                let mut actions = div().flex().flex_wrap().gap(px(4.));
                if row.worker.kind == d::WorkerKind::Root {
                    for (id, label, action) in [
                        ("send-worker", "Send", Control::Send),
                        ("queue-worker", "Queue", Control::Queue),
                        ("steer-worker", "Steer", Control::Steer),
                    ] {
                        actions = actions.child(self.control(id, label, action, cx));
                    }
                } else if row.finished_child() {
                    actions = actions.child(self.control(
                        "follow-child",
                        "Follow up as new child",
                        Control::FollowUp,
                        cx,
                    ));
                } else {
                    actions = actions.child(self.control(
                        "steer-child",
                        "Steer child",
                        Control::Steer,
                        cx,
                    ));
                }
                if row.worker.run.as_ref().is_some_and(|r| r.phase.is_some())
                    || (!row.finished_child() && row.worker.kind == d::WorkerKind::Child)
                {
                    actions = actions.child(self.control("stop-worker", "Stop", Control::Stop, cx));
                }
                if dash.control.as_ref().is_some_and(|p| p.uncertain) {
                    actions = actions.child(self.button(
                        "check-worker-action",
                        "Check action status",
                        D::Reconcile,
                    ));
                }
                detail = detail.child(actions);
            }
            if let Some(root) = row.root {
                let model = self.model.clone();
                detail = detail.child(
                    Btn::new("open-owner")
                        .label("Open owning conversation / review requests")
                        .on_click(move |_, cx| {
                            model.update(cx, |m, cx| {
                                m.dispatch(Command::Dashboard(D::Close), cx);
                                m.dispatch(Command::SelectConversation(root), cx);
                            })
                        }),
                );
            }
            body = body.child(detail);
        }
        page.child(body).child(div().px(px(16.)).pb(px(8.)).text_sm().text_color(c.text_muted).child("Pipkin conversations and child tasks · CLI-internal workers and agents outside Pipkin are not included. Jobs are related work, not agents.")).into_any_element()
    }
}
