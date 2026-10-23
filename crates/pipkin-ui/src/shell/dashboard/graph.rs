//! Spatial dashboard and quick conversation sheet. All controls route through the
//! same revision/generation-guarded commands as the list; opening a node is read-only.
use super::{graph_layout::*, presentation::*, *};

impl Render for DashboardView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = if self.graph.list {
            self.render_list(window, cx).into_any_element()
        } else {
            self.render_graph(window, cx).into_any_element()
        };
        div()
            .relative()
            .size_full()
            .child(page)
            .when(self.hidden_open, |d| {
                d.child(self.hidden_manager(window, cx))
            })
    }
}
impl DashboardView {
    pub(super) fn close_flyup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.modal_open = false;
        self.focus_list(window, cx);
        cx.notify();
    }
    pub(in crate::shell) fn open_flyup(
        &mut self,
        session: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_node(session, cx);
        self.modal_open = true;
        self.modal_history = false;
        let input_question = self
            .model
            .read(cx)
            .state
            .dashboard
            .selected_row()
            .and_then(|r| self.current_request(r, cx))
            .is_some_and(|q| q.kind == UiRequestKind::Input);
        if input_question {
            window.focus(&self.answer_editor.read(cx).focus_handle(cx), cx);
        } else if self
            .model
            .read(cx)
            .state
            .dashboard
            .selected_row()
            .is_some_and(requires_request_ui)
        {
            window.focus(&self.modal_focus, cx);
        } else {
            self.focus_input(window, cx);
        }
        cx.notify();
    }
    fn select_node(&mut self, session: String, cx: &mut Context<Self>) {
        self.graph.reveal(&session);
        if self.model.read(cx).state.dashboard.selected.as_ref() != Some(&session) {
            self.send(D::Select(session), cx);
        }
        cx.notify();
    }
    fn step_attention(&mut self, forward: bool, cx: &mut Context<Self>) -> Option<String> {
        let ids = &self.graph.attention;
        if ids.is_empty() {
            return None;
        }
        let index = self
            .model
            .read(cx)
            .state
            .dashboard
            .selected
            .as_ref()
            .and_then(|s| ids.iter().position(|id| id == s));
        let index = match index {
            Some(i) if forward => (i + 1) % ids.len(),
            Some(i) => (i + ids.len() - 1) % ids.len(),
            None => 0,
        };
        let id = ids[index].clone();
        self.select_node(id.clone(), cx);
        Some(id)
    }
    pub(super) fn view_switch(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut bar = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .p(px(3.))
            .rounded(px(8.))
            .border_1()
            .border_color(cx.theme().colors.border);
        for (id, name, symbol, list) in [
            ("graph-view", "Graph", "git-branch", false),
            ("list-view", "List", "list", true),
        ] {
            let view = cx.entity();
            bar = bar.child(
                Btn::new(id)
                    .label(name)
                    .icon(symbol)
                    .selected(self.graph.list == list)
                    .tab_stop(true)
                    .on_click(move |_, cx| {
                        view.update(cx, |v, cx| {
                            v.graph.list = list;
                            cx.notify();
                        })
                    }),
            );
        }
        let view = cx.entity();
        let dash = &self.model.read(cx).state.dashboard;
        bar.child(
            Btn::new("manage-hidden")
                .label(format!(
                    "Hidden ({})",
                    dash.hidden_projects.len() + dash.hidden_threads.len()
                ))
                .hint("Manage projects and threads hidden from orchestration")
                .tab_stop(true)
                .on_click(move |w, cx| {
                    view.update(cx, |v, cx| {
                        v.modal_open = false;
                        v.hidden_open = true;
                        w.focus(&v.modal_focus, cx);
                        cx.notify();
                    })
                }),
        )
    }
    fn render_graph(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let c = &t.colors;
        let dash = self.model.read(cx).state.dashboard.clone();
        let demo = self.model.read(cx).state.mode == Mode::Demo;
        let counts = dash.counts.unwrap_or_default();
        let selected = dash.selected_row().cloned();
        let view = cx.entity();
        let narrow = window.viewport_size().width < px(1120.);
        let mut page = div()
            .id("orchestration-page")
            .key_context("Orchestration")
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(Self::navigate))
            .on_action(
                cx.listener(|this, _: &super::super::actions::DashboardNext, _, cx| {
                    if !this.modal_open {
                        this.step(true, cx);
                    }
                }),
            )
            .on_action(cx.listener(
                |this, _: &super::super::actions::DashboardPrevious, _, cx| {
                    if !this.modal_open {
                        this.step(false, cx);
                    }
                },
            ))
            .on_action(cx.listener(
                |this, _: &super::super::actions::CloseOverlay, window, cx| {
                    if this.modal_open {
                        this.close_flyup(window, cx);
                        cx.stop_propagation();
                    }
                },
            ))
            .role(Role::Main)
            .aria_label("Orchestration workspace graph")
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .bg(c.bg_app)
            .pl(px(60.))
            .text_color(c.text)
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(12.))
                    .px(px(20.))
                    .py(px(12.))
                    .child(
                        div()
                            .text_size(px(26. * t.scale))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Orchestration"),
                    )
                    .when(demo, |d| {
                        d.child(chip("Demo · simulated agent", c.text_muted, c.bg_pane, cx))
                    })
                    .child(div().flex_1())
                    .when(!narrow, |d| {
                        d.child(
                            div()
                                .text_size(t.small_size())
                                .text_color(c.text_muted)
                                .child(format!(
                                    "{} projects · {} loaded threads",
                                    self.graph.projects.len(),
                                    dash.rows.len()
                                )),
                        )
                    })
                    .child(chip(
                        dash.freshness.label(),
                        if dash.freshness == d::Freshness::Live {
                            c.success
                        } else {
                            c.warning
                        },
                        c.bg_pane,
                        cx,
                    ))
                    // Keep this first focusable control for the existing keyboard contract.
                    .child(
                        Btn::new("dashboard-coverage")
                            .icon("circle-help")
                            .aria("About this view")
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
                    .when(!narrow, |d| {
                        d.child(
                            div()
                                .w(px(260.))
                                .px(px(10.))
                                .py(px(8.))
                                .rounded(px(8.))
                                .border_1()
                                .border_color(c.border)
                                .bg(c.bg_input)
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(icon("search", px(16.), c.text_faint))
                                .child(div().flex_1().min_w_0().child(self.search.clone())),
                        )
                    })
                    .child(
                        Btn::new("graph-new-thread")
                            .label("New thread")
                            .icon("plus")
                            .kind(BtnKind::Primary)
                            .tab_stop(true)
                            .on_click({
                                let this = cx.entity();
                                move |_, cx| {
                                    this.update(cx, |_, cx| {
                                        cx.emit(super::super::dashboard_cli::NewThread)
                                    })
                                }
                            }),
                    ),
            );
        let mut filters = div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap(px(8.))
            .px(px(20.))
            .pb(px(16.));
        if narrow {
            filters = filters.child(
                div()
                    .w(px(200.))
                    .p(px(8.))
                    .bg(c.bg_input)
                    .rounded(px(8.))
                    .child(self.search.clone()),
            );
        }
        filters = filters.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(10.))
                .text_size(t.small_size())
                .text_color(c.text_muted)
                .child(icon("monitor", px(16.), c.text_muted))
                .child("Connected workspace"),
        );
        for (id, label, group, color) in [
            (
                "graph-overview",
                "Overview".to_owned(),
                d::ActivityGroup::Overview,
                c.text_muted,
            ),
            (
                "graph-all",
                "All threads".to_owned(),
                d::ActivityGroup::All,
                c.text_muted,
            ),
            (
                "graph-attention",
                format!("Needs you  {}", counts.attention),
                d::ActivityGroup::Attention,
                c.accent,
            ),
            (
                "graph-running",
                format!("Running  {}", counts.running),
                d::ActivityGroup::Running,
                c.activity,
            ),
            (
                "graph-finished",
                "Finished".into(),
                d::ActivityGroup::Finished,
                c.success,
            ),
        ] {
            let view = cx.entity();
            filters = filters.child(
                div()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c.border)
                    .flex()
                    .items_center()
                    .pl(px(10.))
                    .gap(px(4.))
                    .child(status_dot(color, 7.))
                    .child(
                        Btn::new(id)
                            .label(label)
                            .selected(dash.group.unwrap_or(d::ActivityGroup::Overview) == group)
                            .tab_stop(true)
                            .on_click(move |_, cx| {
                                view.update(cx, |v, cx| {
                                    v.graph.attention_only = false;
                                    v.send(D::Group(Some(group)), cx);
                                })
                            }),
                    ),
            );
        }
        filters = filters.child(self.button(
            "graph-recency",
            format!("Recent · {} days", dash.recency.days()),
            D::Recency(dash.recency.next()),
        ).hint("Hide closed conversations and dormant threads. Active work and pending input stay visible. Click to choose 30, 60 or 90 days. Older conversations remain in history."));
        if dash.filter.text.is_some()
            || dash.filter.cwd.is_some()
            || dash.filter.kind.is_some()
            || dash.filter.backend.is_some()
            || dash.filter.failed
        {
            filters = filters
                .child(
                    div()
                        .text_size(t.small_size())
                        .text_color(c.text_muted)
                        .child("Filtered results"),
                )
                .child(
                    self.button(
                        "graph-clear-filters",
                        "Clear filters",
                        D::Filter(Default::default()),
                    )
                    .compact(),
                );
        }
        page = page.child(filters);
        if self.about_open {
            page = page.child(div().mx(px(20.)).mb(px(8.)).p(px(12.)).rounded(px(8.)).bg(c.bg_pane)
                .text_size(t.small_size()).text_color(c.text_muted)
                .child("Shows the connected engine’s loaded conversations and child tasks, grouped by working directory. Counts come from the engine; collapsed badges describe loaded threads. CLI-internal workers and external agents are not visible. Double-click a thread, or focus it and press Enter, to open its conversation sheet."));
        }
        if let Some(error) = &dash.error {
            page = page.child(
                div()
                    .px(px(20.))
                    .py(px(8.))
                    .text_color(c.danger)
                    .child(error.clone()),
            );
        }
        let view = cx.entity();
        let mut map = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .m(px(10.))
            .mr(px(0.))
            .rounded(px(12.))
            .border_1()
            .border_color(c.border)
            .bg(c.bg_app)
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(8.))
                    .p(px(12.))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Workspace map"),
                    )
                    .child(div().flex_1())
                    .child(self.view_switch(cx))
                    .child(
                        Btn::new("collapse-projects")
                            .label(
                                if self
                                    .graph
                                    .projects
                                    .iter()
                                    .all(|p| self.graph.collapsed.contains(&p.path))
                                    && !self.graph.projects.is_empty()
                                {
                                    "Expand projects"
                                } else {
                                    "Collapse projects"
                                },
                            )
                            .compact()
                            .tab_stop(true)
                            .on_click(move |_, cx| {
                                view.update(cx, |v, cx| {
                                    if v.graph
                                        .projects
                                        .iter()
                                        .all(|p| v.graph.collapsed.contains(&p.path))
                                    {
                                        v.graph.collapsed.clear();
                                    } else {
                                        v.graph.collapsed = v
                                            .graph
                                            .projects
                                            .iter()
                                            .map(|p| p.path.clone())
                                            .collect();
                                    }
                                    cx.notify();
                                })
                            }),
                    ),
            );
        map = map.child(self.graph_canvas(window, cx));
        let zoom = format!("{}%", (self.graph.rendered_zoom * 100.).round() as u32);
        let visible: usize = self
            .graph
            .projects
            .iter()
            .map(|p| self.graph.visible_rows(p).len())
            .sum();
        let mut footer = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .p(px(8.))
            .border_t_1()
            .border_color(c.border);
        for (id, label, factor) in [
            ("zoom-out", "−", 0.8),
            ("zoom-in", "+", 1.25),
            ("zoom-fit", "Fit workspace", 0.),
        ] {
            let view = cx.entity();
            footer = footer.child(Btn::new(id).label(label).compact().tab_stop(true).on_click(
                move |_, cx| {
                    view.update(cx, |v, cx| {
                        if factor == 0. {
                            v.graph.zoom = None;
                            v.graph.pan = (0., 0.);
                            v.graph.locate = None;
                        } else {
                            v.graph.zoom = Some(
                                (v.graph.zoom.unwrap_or(v.graph.rendered_zoom) * factor)
                                    .clamp(f32::EPSILON, 1.6),
                            );
                        }
                        cx.notify();
                    })
                },
            ));
        }
        footer = footer
            .child(
                div()
                    .text_size(t.small_size())
                    .text_color(c.text_muted)
                    .child(zoom),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_size(t.small_size())
                    .text_color(c.text_faint)
                    .child(format!(
                        "{visible} visible · {} grouped",
                        dash.rows.len().saturating_sub(visible)
                    )),
            );
        if dash.next.is_some() {
            footer = footer.child(
                self.button("graph-more", "Load more threads", D::More)
                    .disabled(dash.loading)
                    .compact(),
            );
        }
        map = map.child(footer);
        let mut body = div().flex().flex_1().min_h_0().min_w_0().child(map);
        if !narrow {
            body = body.child(self.attention_panel(selected.clone(), cx));
        } else {
            let view = cx.entity();
            body = body.child(
                div().w(px(42.)).flex_shrink_0().child(
                    Btn::new("narrow-attention")
                        .icon("circle-alert")
                        .aria("Next thread needing attention")
                        .disabled(self.graph.attention.is_empty())
                        .tab_stop(true)
                        .on_click(move |window, cx| {
                            view.update(cx, |v, cx| {
                                if let Some(id) = v.step_attention(true, cx) {
                                    v.open_flyup(id, window, cx);
                                }
                            })
                        }),
                ),
            );
        }
        page = page.child(body).child(self.navigation_rail(cx));
        if self.modal_open
            && let Some(row) = selected
        {
            page = page.child(deferred(self.flyup(row, window, cx)).with_priority(8));
        }
        page.into_any_element()
    }

    fn graph_canvas(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let dash = self.model.read(cx).state.dashboard.clone();
        if self.graph.projects.is_empty() {
            return div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(12.))
                .text_color(c.text_muted)
                .child(icon("git-branch", px(32.), c.text_faint))
                .child(if dash.loading {
                    "Loading workspace activity…"
                } else if !dash.available {
                    "Activity directory is not available on this connection."
                } else {
                    "No threads in this scope"
                })
                .child(self.button(
                    "empty-all",
                    "Show all threads",
                    D::Group(Some(d::ActivityGroup::All)),
                ))
                .into_any_element();
        }
        let width = f32::from(window.viewport_size().width);
        let viewport = self.graph.viewport.unwrap_or((
            (width - if width >= 1120. { 452. } else { 124. }).max(1.),
            (f32::from(window.viewport_size().height) - 235.).max(1.),
        ));
        let (available_w, available_h) = viewport;
        let groups: Vec<_> = self
            .graph
            .projects
            .iter()
            .map(|p| {
                let visible = self.graph.visible_rows(p).len();
                (
                    visible,
                    !self.graph.collapsed.contains(&p.path) && p.rows.len() > visible,
                )
            })
            .collect();
        let layout = GraphLayout::fit(&groups, viewport);
        let z = self.graph.zoom.unwrap_or(layout.zoom);
        self.graph.rendered_zoom = z;
        let base = layout.offset;
        if let Some(session) = self.graph.locate.take()
            && let Some((project_index, project)) = self
                .graph
                .projects
                .iter()
                .enumerate()
                .find(|(_, p)| p.rows.iter().any(|r| r.worker.session == session))
        {
            let index = self
                .graph
                .visible_rows(project)
                .iter()
                .position(|r| r.worker.session == session)
                .unwrap_or(0);
            let origin = layout.origins[project_index];
            let visible = self.graph.visible_rows(project);
            let display_origin = if visible.len() == 1 && project.rows.len() == 1 {
                (origin.0 + 132., origin.1)
            } else {
                origin
            };
            let (x, y) = GraphState::thread_position(display_origin, index);
            let left = x * z + base.0 + self.graph.pan.0;
            let top = y * z + base.1 + self.graph.pan.1;
            if left < 0. || left + CARD_W * z > available_w {
                self.graph.pan.0 = available_w / 2. - (x + CARD_W / 2.) * z - base.0;
            }
            if top < 0. || top + CARD_H * z > available_h {
                self.graph.pan.1 = available_h / 2. - (y + CARD_H / 2.) * z - base.1;
            }
        }
        let pan = (base.0 + self.graph.pan.0, base.1 + self.graph.pan.1);
        let coords = move |x: f32, y: f32| (px(x * z + pan.0), px(y * z + pan.1));
        let mut links = Vec::new();
        let mut nodes: Vec<AnyElement> = Vec::new();
        let server = layout.server;
        let (sx, sy) = coords(server.0, server.1);
        let color = if dash.freshness == d::Freshness::Live {
            c.activity
        } else {
            c.warning
        };
        nodes.push(
            div()
                .absolute()
                .left(sx)
                .top(sy)
                .w(px(300. * z))
                .h(px(82. * z))
                .p(px(12. * z))
                .rounded(px(10.))
                .border_1()
                .border_color(c.border_strong)
                .bg(c.bg_card)
                .flex()
                .items_center()
                .gap(px(10. * z))
                .child(icon("monitor", px(30. * z), c.text_muted))
                .child(status_dot(color, 7. * z))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(5. * z))
                        .text_size(px(15. * z))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Connected workspace"),
                        )
                        .child(div().text_size(px(12. * z)).text_color(c.text_muted).child(
                            format!(
                                "{} · {} projects · {} loaded",
                                dash.freshness.label(),
                                self.graph.projects.len(),
                                dash.rows.len()
                            ),
                        )),
                )
                .into_any_element(),
        );
        let state = &self.model.read(cx).state;
        for (project_index, p) in self.graph.projects.iter().enumerate() {
            let origin = layout.origins[project_index];
            let (ox, oy) = origin;
            let highlighted = p
                .rows
                .iter()
                .any(|r| Some(&r.worker.session) == dash.selected.as_ref());
            let entry = ox + (GROUP_W - 24.) / 2.;
            if oy > layout.origins[0].1 {
                links.push(((server.0 + 150., server.1 + 82.), (12., 124.), highlighted));
                links.push(((12., 124.), (12., oy - 24.), highlighted));
                links.push(((12., oy - 24.), (entry, oy), highlighted));
            } else {
                links.push(((server.0 + 150., server.1 + 82.), (entry, oy), highlighted));
            }
            let collapsed = self.graph.collapsed.contains(&p.path);
            let visible = self.graph.visible_rows(p);
            let remaining = p.rows.len().saturating_sub(visible.len());
            let single = visible.len() == 1 && remaining == 0;
            let group_width = if single { CARD_W + 24. } else { GROUP_W };
            let display_origin = if single { (ox + 132., oy) } else { origin };
            let (x, y) = coords(display_origin.0 - 12., oy);
            let group_height = if collapsed {
                PROJECT_H + 8.
            } else {
                THREAD_Y
                    + visible.len().div_ceil(2) as f32 * ROW_PITCH
                    + if remaining > 0 { 36. } else { 0. }
            };
            if f32::from(y) > available_h + 200.
                || f32::from(y) + group_height * z < -200.
                || f32::from(x) > available_w + 300.
                || f32::from(x) + GROUP_W * z < -300.
            {
                continue;
            }
            nodes.push(
                div()
                    .absolute()
                    .left(x)
                    .top(y)
                    .w(px(group_width * z))
                    .h(px(group_height * z))
                    .rounded(px(12.))
                    .border_1()
                    .border_color(if highlighted {
                        c.border_strong
                    } else {
                        c.border
                    })
                    .bg(c.bg_pane)
                    .into_any_element(),
            );
            let path = p.path.clone();
            let key_path = path.clone();
            let view = cx.entity();
            let key_view = view.clone();
            nodes.push(
                div()
                    .id(SharedString::from(format!("project:{}", p.path)))
                    .absolute()
                    .left(x)
                    .top(y)
                    .w(px((group_width - 52.) * z))
                    .h(px(PROJECT_H * z))
                    .px(px(16. * z))
                    .rounded(px(10.))
                    .role(Role::Button)
                    .aria_label(format!(
                        "{}: {} threads, {} need attention. {}",
                        p.path,
                        p.rows.len(),
                        p.attention,
                        if collapsed { "Expand" } else { "Collapse" }
                    ))
                    .tab_index(0)
                    .tab_stop(true)
                    .cursor_pointer()
                    .hover(|s| s.bg(c.bg_hover))
                    .flex()
                    .items_center()
                    .gap(px(10. * z))
                    .on_click(move |_, _, cx| {
                        view.update(cx, |v, cx| {
                            if !v.graph.collapsed.remove(&path) {
                                v.graph.collapsed.insert(path.clone());
                            }
                            cx.notify();
                        })
                    })
                    .capture_key_down(move |ev, w, cx| {
                        if !ev.keystroke.modifiers.modified()
                            && matches!(ev.keystroke.key.as_str(), "enter" | "space")
                        {
                            w.prevent_default();
                            if ev.is_held {
                                cx.stop_propagation();
                                return;
                            }
                            key_view.update(cx, |v, cx| {
                                if !v.graph.collapsed.remove(&key_path) {
                                    v.graph.collapsed.insert(key_path.clone());
                                }
                                cx.notify();
                            });
                            cx.stop_propagation();
                        }
                    })
                    .child(icon(
                        "folder",
                        px(18. * z),
                        if highlighted { c.accent } else { c.text_muted },
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(15. * z))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(p.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(12. * z))
                            .text_color(c.text_muted)
                            .child(format!(
                                "{} {}",
                                p.rows.len(),
                                if p.rows.len() == 1 {
                                    "thread"
                                } else {
                                    "threads"
                                }
                            )),
                    )
                    .when(p.attention > 0, |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(5. * z))
                                .text_size(px(12. * z))
                                .text_color(c.accent)
                                .child(status_dot(c.accent, 6. * z))
                                .child(p.attention.to_string()),
                        )
                    })
                    .child(icon(
                        if collapsed {
                            "chevron-right"
                        } else {
                            "chevron-down"
                        },
                        px(14. * z),
                        c.text_muted,
                    ))
                    .into_any_element(),
            );
            nodes.push(div().absolute()
                .left(x + px((group_width - 52.) * z)).top(y + px(10. * z))
                .child(self.button(SharedString::from(format!("hide-project:{}", p.path)), "Hide", D::HideProject(p.path.clone()))
                    .compact().hint("Hide this project from orchestration. Work continues; restore it from Hidden."))
                .into_any_element());
            for (i, row) in visible.iter().enumerate() {
                let pos = GraphState::thread_position(display_origin, i);
                let (x, y) = coords(pos.0, pos.1);
                let selected = dash.selected.as_ref() == Some(&row.worker.session);
                let attention = needs_attention(row, &dash);
                let (label, symbol, status_color) = row_status(row, &dash, &c);
                let view = cx.entity();
                let key_view = view.clone();
                let session = row.worker.session.clone();
                let key_session = session.clone();
                nodes.push(
                    div()
                        .id(SharedString::from(format!("thread:{session}")))
                        .absolute()
                        .left(x)
                        .top(y)
                        .w(px(CARD_W * z))
                        .h(px(CARD_H * z))
                        .p(px(12. * z))
                        .rounded(px(9.))
                        .border_1()
                        .border_color(if attention {
                            c.accent.opacity(0.8)
                        } else if selected {
                            c.border_strong
                        } else {
                            c.border
                        })
                        .bg(if selected { c.bg_selected } else { c.bg_card })
                        .hover(|s| s.bg(c.bg_selected))
                        .role(Role::Button)
                        .aria_label(format!("{}. {}. Enter to open thread.", row.title(), label))
                        .tab_index(0)
                        .tab_stop(true)
                        .cursor_pointer()
                        .flex()
                        .flex_col()
                        .gap(px(7. * z))
                        .on_click(move |event, w, cx| {
                            view.update(cx, |v, cx| {
                                if event.click_count() >= 2 {
                                    v.open_flyup(session.clone(), w, cx)
                                } else {
                                    v.select_node(session.clone(), cx)
                                }
                            })
                        })
                        .capture_key_down(move |ev, w, cx| {
                            if ev.keystroke.key == "enter" && !ev.keystroke.modifiers.modified() {
                                w.prevent_default();
                                if ev.is_held {
                                    cx.stop_propagation();
                                    return;
                                }
                                key_view
                                    .update(cx, |v, cx| v.open_flyup(key_session.clone(), w, cx));
                                cx.stop_propagation();
                            }
                        })
                        .child(
                            div()
                                .text_size(px(14. * z))
                                .font_weight(FontWeight::MEDIUM)
                                .line_clamp(2)
                                .child(row.title().to_owned()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(12. * z))
                                .text_color(c.text_muted)
                                .child(activity(row, state)),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(5. * z))
                                .text_size(px(11. * z))
                                .child(icon(symbol, px(12. * z), status_color))
                                .child(div().text_color(status_color).child(label))
                                .child(div().flex_1())
                                .child(
                                    div()
                                        .min_w_0()
                                        .max_w(px(90. * z))
                                        .truncate()
                                        .text_color(c.text_faint)
                                        .child(model_name(row, state)),
                                )
                                .child(div().text_color(c.text_faint).child("·"))
                                .child(
                                    div()
                                        .text_color(c.text_faint)
                                        .child(updated_age(row, state.now())),
                                ),
                        )
                        .into_any_element(),
                );
            }
            if !collapsed && remaining > 0 {
                let pos = GraphState::thread_position(origin, visible.len().div_ceil(2) * 2);
                let (x, y) = coords(ox + (502. - 180.) / 2., pos.1);
                let view = cx.entity();
                let key_view = view.clone();
                let path = p.path.clone();
                let key_path = path.clone();
                let disabled = self.graph.attention_only;
                nodes.push(
                    div()
                        .id(SharedString::from(format!("more:{}", p.path)))
                        .absolute()
                        .left(x)
                        .top(y)
                        .w(px(180. * z))
                        .h(px(28. * z))
                        .rounded_full()
                        .border_1()
                        .border_color(c.border)
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(12. * z))
                        .text_color(c.text_muted)
                        .role(Role::Button)
                        .aria_label(format!("Show {remaining} more threads in {}", p.name))
                        .when(!disabled, |d| d.tab_index(0).tab_stop(true))
                        .cursor_pointer()
                        .hover(|s| s.bg(c.bg_hover))
                        .on_click(move |_, _, cx| {
                            if !disabled {
                                view.update(cx, |v, cx| {
                                    *v.graph.limits.entry(path.clone()).or_insert(4) += 4;
                                    cx.notify();
                                });
                            }
                        })
                        .capture_key_down(move |ev, w, cx| {
                            if !disabled
                                && !ev.keystroke.modifiers.modified()
                                && matches!(ev.keystroke.key.as_str(), "enter" | "space")
                            {
                                w.prevent_default();
                                if ev.is_held {
                                    cx.stop_propagation();
                                    return;
                                }
                                key_view.update(cx, |v, cx| {
                                    *v.graph.limits.entry(key_path.clone()).or_insert(4) += 4;
                                    cx.notify();
                                });
                                cx.stop_propagation();
                            }
                        })
                        .child(if disabled {
                            format!("{remaining} other threads")
                        } else {
                            format!("+{remaining} threads")
                        })
                        .into_any_element(),
                );
            }
        }
        let colors = c.clone();
        let measure = cx.entity().downgrade();
        let background = canvas(
            move |bounds, _, cx| {
                let viewport = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                // Measure the graph itself: wrapping headers and the attention panel
                // must not be counted as available space. Notify only when it changes.
                if viewport.0 > 0. && viewport.1 > 0. {
                    cx.defer(move |cx| {
                        let _ = measure.update(cx, |v, cx| {
                            if v.graph.viewport != Some(viewport) {
                                v.graph.viewport = Some(viewport);
                                cx.notify();
                            }
                        });
                    });
                }
            },
            move |bounds, _, window, _| {
                let mut grid = PathBuilder::stroke(px(1.));
                for x in (0..f32::from(bounds.size.width) as usize).step_by(22) {
                    for y in (0..f32::from(bounds.size.height) as usize).step_by(22) {
                        let p = bounds.origin + point(px(x as f32), px(y as f32));
                        grid.move_to(p);
                        grid.line_to(p + point(px(1.), px(0.)));
                    }
                }
                if let Ok(path) = grid.build() {
                    window.paint_path(path, colors.border.opacity(0.45));
                }
                for (from, to, active) in &links {
                    let a = bounds.origin + point(px(from.0 * z + pan.0), px(from.1 * z + pan.1));
                    let b = bounds.origin + point(px(to.0 * z + pan.0), px(to.1 * z + pan.1));
                    let middle = (a.y + b.y) / 2.;
                    let mut path = PathBuilder::stroke(px(if *active { 2. } else { 1. }));
                    path.move_to(a);
                    path.cubic_bezier_to(b, point(a.x, middle), point(b.x, middle));
                    if let Ok(path) = path.build() {
                        window.paint_path(
                            path,
                            if *active {
                                colors.accent
                            } else {
                                colors.text_faint.opacity(0.35)
                            },
                        );
                    }
                }
            },
        )
        .absolute()
        .size_full();
        div()
            .id("workspace-graph-canvas")
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .on_scroll_wheel(cx.listener(|v, ev: &ScrollWheelEvent, _, cx| {
                let delta = ev.delta.pixel_delta(px(24.));
                v.graph.pan.0 += f32::from(delta.x);
                v.graph.pan.1 += f32::from(delta.y);
                cx.notify();
                cx.stop_propagation();
            }))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|v, ev: &MouseDownEvent, _, cx| {
                    v.graph.dragging = Some((f32::from(ev.position.x), f32::from(ev.position.y)));
                    cx.stop_propagation();
                }),
            )
            .on_mouse_move(cx.listener(|v, ev: &MouseMoveEvent, _, cx| {
                if ev.pressed_button != Some(MouseButton::Middle) {
                    v.graph.dragging = None;
                    return;
                }
                if let Some((x, y)) = v
                    .graph
                    .dragging
                    .replace((f32::from(ev.position.x), f32::from(ev.position.y)))
                {
                    v.graph.pan.0 += f32::from(ev.position.x) - x;
                    v.graph.pan.1 += f32::from(ev.position.y) - y;
                    cx.notify();
                }
            }))
            .child(background)
            .children(nodes)
            .into_any_element()
    }

    fn attention_panel(&self, selected: Option<d::Row>, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let c = &t.colors;
        let state = &self.model.read(cx).state;
        let dash = &state.dashboard;
        let count = self.graph.attention.len();
        let index = dash
            .selected
            .as_ref()
            .and_then(|id| self.graph.attention.iter().position(|s| s == id));
        let mut header = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(20.))
            .border_b_1()
            .border_color(c.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Needs your attention"),
                    )
                    .child(chip(
                        count.to_string(),
                        if count > 0 { c.accent } else { c.text_muted },
                        c.accent_bg,
                        cx,
                    )),
            );
        if count > 0 {
            let mut pager = div().flex().items_center().gap(px(6.)).child(
                div()
                    .flex_1()
                    .text_size(t.small_size())
                    .text_color(c.text_muted)
                    .child(index.map_or(format!("{count} threads need you"), |i| {
                        format!("{} of {count}", i + 1)
                    })),
            );
            for (id, symbol, forward) in [
                ("previous-attention", "chevron-left", false),
                ("next-attention", "chevron-right", true),
            ] {
                let view = cx.entity();
                pager = pager.child(
                    Btn::new(id)
                        .icon(symbol)
                        .aria(if forward {
                            "Next attention item"
                        } else {
                            "Previous attention item"
                        })
                        .tab_stop(true)
                        .on_click(move |_, cx| {
                            view.update(cx, |v, cx| {
                                v.step_attention(forward, cx);
                            })
                        }),
                );
            }
            header = header.child(pager);
        }
        let mut content = div()
            .id("graph-inspector-scroll")
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .p(px(20.))
            .flex()
            .flex_col()
            .gap(px(20.));
        if let Some(row) = selected {
            let (label, symbol, color) = row_status(&row, dash, c);
            let question = self.current_request(&row, cx);
            let decision = requires_request_ui(&row);
            let view = cx.entity();
            let id = row.worker.session.clone();
            content =
                content
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .text_size(t.small_size())
                            .text_color(c.text_muted)
                            .child(icon("folder", px(14.), c.text_muted))
                            .child(div().truncate().child(project_name(&row.worker.cwd))),
                    )
                    .child(
                        div()
                            .text_size(px(21. * t.scale))
                            .font_weight(FontWeight::SEMIBOLD)
                            .line_clamp(3)
                            .child(row.title().to_owned()),
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
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(12.))
                            .py(px(18.))
                            .border_t_1()
                            .border_b_1()
                            .border_color(c.border)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .text_color(color)
                                    .child(icon(symbol, px(22.), color))
                                    .child(
                                        div().font_weight(FontWeight::SEMIBOLD).child(
                                            if decision { "Decision needed" } else { label },
                                        ),
                                    ),
                            )
                            .child(div().text_color(c.text_muted).child(activity(&row, state)))
                            .when_some(question.and_then(|q| q.message), |d, message| {
                                d.child(div().line_clamp(5).text_color(c.text_muted).child(message))
                            }),
                    )
                    .child(
                        Btn::new("quick-reply")
                            .label(if decision {
                                "Reply to thread"
                            } else if dash.completion_unread(&row) {
                                "Review result"
                            } else {
                                "Open thread"
                            })
                            .icon("message-square")
                            .kind(BtnKind::Primary)
                            .tab_stop(true)
                            .on_click(move |w, cx| {
                                view.update(cx, |v, cx| v.open_flyup(id.clone(), w, cx))
                            }),
                    );
            content = content.child(self.hide_controls(&row, "inspector"));
            if let Some(button) = self.review_button(&row, "inspector-mark-reviewed", cx) {
                content = content.child(button);
            }
            if let Some(button) =
                self.open_owner_button(&row, "inspector-full-thread", "Open full conversation")
            {
                content = content.child(button);
            }
            if row.worker.children_active > 0 || row.worker.jobs_active > 0 {
                content = content.child(
                    div()
                        .text_size(t.small_size())
                        .text_color(c.text_muted)
                        .child(format!(
                            "{} active child tasks · {} background jobs",
                            row.worker.children_active, row.worker.jobs_active
                        )),
                );
            }
        } else {
            content=content.child(div().flex().flex_col().gap(px(12.)).py(px(16.))
                .child(icon(if count==0 {"circle-check"}else{"message-square"},px(28.),if count==0 {c.success}else{c.accent}))
                .child(div().text_size(px(20.)).font_weight(FontWeight::MEDIUM).child(if count==0 {"You're all caught up"}else{"Choose a thread"}))
                .child(div().text_color(c.text_muted).child(if count==0 {"No loaded threads need input or review. Select a thread on the map to see its activity."}else{"Select an item below to review what it needs."})));
        }
        let others: Vec<_> = self
            .graph
            .attention
            .iter()
            .filter(|id| dash.selected.as_ref() != Some(id))
            .collect();
        if !others.is_empty() {
            content = content.child(
                div()
                    .pt(px(8.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Up next"),
            );
        }
        for id in others {
            if let Some(row) = dash.rows.iter().find(|r| &r.worker.session == id) {
                let view = cx.entity();
                let key_view = view.clone();
                let id = id.clone();
                let key_id = id.clone();
                let (_, _, color) = row_status(row, dash, c);
                content = content.child(
                    div()
                        .id(SharedString::from(format!("attention:{id}")))
                        .p(px(12.))
                        .rounded(px(10.))
                        .border_1()
                        .border_color(c.border)
                        .bg(c.bg_card)
                        .role(Role::Button)
                        .aria_label(format!("Review {}", row.title()))
                        .tab_index(0)
                        .tab_stop(true)
                        .cursor_pointer()
                        .hover(|s| s.bg(c.bg_selected))
                        .flex()
                        .items_start()
                        .gap(px(10.))
                        .on_click(move |_, _, cx| {
                            view.update(cx, |v, cx| v.select_node(id.clone(), cx))
                        })
                        .capture_key_down(move |ev, w, cx| {
                            if !ev.keystroke.modifiers.modified()
                                && matches!(ev.keystroke.key.as_str(), "enter" | "space")
                            {
                                w.prevent_default();
                                if ev.is_held {
                                    cx.stop_propagation();
                                    return;
                                }
                                key_view.update(cx, |v, cx| v.select_node(key_id.clone(), cx));
                                cx.stop_propagation();
                            }
                        })
                        .child(div().pt(px(5.)).child(status_dot(color, 8.)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .line_clamp(2)
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(row.title().to_owned()),
                                )
                                .child(
                                    div()
                                        .text_size(t.small_size())
                                        .text_color(c.text_muted)
                                        .child(format!(
                                            "{} · Updated {}",
                                            project_name(&row.worker.cwd),
                                            updated_age(row, state.now())
                                        )),
                                ),
                        )
                        .child(icon("chevron-right", px(14.), c.text_faint)),
                );
            }
        }
        div()
            .w(px(360.))
            .flex_shrink_0()
            .m(px(10.))
            .rounded(px(12.))
            .border_1()
            .border_color(c.border)
            .bg(c.bg_surface)
            .flex()
            .flex_col()
            .min_h_0()
            .overflow_hidden()
            .child(header)
            .child(content)
            .into_any_element()
    }
}
