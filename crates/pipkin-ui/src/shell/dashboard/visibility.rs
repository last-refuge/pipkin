//! Manual dashboard visibility is persisted in the core, independently of search and recency.
use super::*;

impl DashboardView {
    pub(super) fn hide_controls(&self, row: &d::Row, prefix: &str) -> impl IntoElement {
        div().flex().flex_wrap().gap(px(4.))
            .child(self.button(SharedString::from(format!("{prefix}-hide-thread")), "Hide thread", D::HideThread(row.worker.session.clone()))
                .compact().hint("Hide this thread and its child tasks from orchestration. Work continues; restore it from Hidden."))
            .child(self.button(SharedString::from(format!("{prefix}-hide-project")), "Hide project", D::HideProject(row.worker.cwd.clone()))
                .compact().hint("Hide all threads in this project from orchestration. Restore it from Hidden."))
    }

    fn close_hidden(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.hidden_open = false;
        self.focus_list(window, cx);
        cx.notify();
    }

    pub(super) fn hidden_manager(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let c = &t.colors;
        let dash = &self.model.read(cx).state.dashboard;
        let empty = dash.hidden_projects.is_empty() && dash.hidden_threads.is_empty();
        let close = cx.entity();
        let backdrop = close.clone();
        let mut list = div()
            .id("hidden-items-scroll")
            .overflow_y_scroll()
            .max_h(px(
                (f32::from(window.viewport_size().height) - 300.).max(80.)
            ))
            .flex()
            .flex_col()
            .gap(px(12.));
        if !dash.hidden_projects.is_empty() {
            list = list.child(div().font_weight(FontWeight::SEMIBOLD).child("Projects"));
        }
        for path in &dash.hidden_projects {
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().truncate().child(project_name(path)))
                            .child(
                                div()
                                    .text_size(t.small_size())
                                    .text_color(c.text_muted)
                                    .truncate()
                                    .child(path.clone()),
                            ),
                    )
                    .child(self.button(
                        SharedString::from(format!("show-project:{path}")),
                        "Show project",
                        D::ShowProject(path.clone()),
                    )),
            );
        }
        if !dash.hidden_threads.is_empty() {
            list = list.child(div().font_weight(FontWeight::SEMIBOLD).child("Threads"));
        }
        for (session, hidden) in &dash.hidden_threads {
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().line_clamp(2).child(hidden.title.clone()))
                            .child(
                                div()
                                    .text_size(t.small_size())
                                    .text_color(c.text_muted)
                                    .truncate()
                                    .child(hidden.project.clone()),
                            )
                            .when(dash.hidden_projects.contains(&hidden.project), |d| {
                                d.child(
                                    div()
                                        .text_size(t.small_size())
                                        .text_color(c.accent)
                                        .child("Project also hidden — show it to see this thread."),
                                )
                            }),
                    )
                    .child(self.button(
                        SharedString::from(format!("show-thread:{session}")),
                        "Show thread",
                        D::ShowThread(session.clone()),
                    )),
            );
        }
        if empty {
            list = list.child(div().py(px(24.)).text_color(c.text_muted).child(
                "Nothing hidden. Use Hide on a project, or select a thread and choose Hide thread.",
            ));
        }
        let dialog = div().id("hidden-items-dialog").role(Role::Dialog).aria_label("Hidden projects and threads")
            .track_focus(&self.modal_focus).occlude().relative().w_full().max_w(px(680.))
            .p(px(24.)).rounded(px(16.)).border_1().border_color(c.border_strong).bg(c.bg_elevated)
            .text_color(c.text).shadow_lg().flex().flex_col().gap(px(18.))
            .capture_key_down(cx.listener(|v, ev: &KeyDownEvent, w, cx| {
                if ev.keystroke.key == "escape" {
                    v.close_hidden(w, cx);
                    cx.stop_propagation();
                } else {
                    v.navigate(ev, w, cx);
                }
            }))
            .child(div().text_size(px(22.)).font_weight(FontWeight::SEMIBOLD).child("Hidden from orchestration"))
            .child(div().text_color(c.text_muted).child("Hidden work keeps running, but is excluded from this dashboard and its attention counts. Conversations and drafts remain in history."))
            .child(list)
            .child(div().text_size(t.small_size()).text_color(c.text_muted).child("Restored items follow your current search, status and recency filters. Showing a project keeps individually hidden threads hidden."))
            .child(div().flex().items_center().gap(px(12.))
                .child(self.button("show-all-hidden", "Show all hidden", D::ShowAllHidden).disabled(empty))
                .child(div().flex_1())
                .child(Btn::new("close-hidden").label("Done").tab_stop(true).on_click(move |w, cx| close.update(cx, |v, cx| v.close_hidden(w, cx)))));
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .p(px(20.))
            .child(
                div()
                    .id("hidden-items-scrim")
                    .absolute()
                    .inset_0()
                    .bg(c.bg_app.opacity(0.5))
                    .occlude()
                    .on_click(move |_, w, cx| backdrop.update(cx, |v, cx| v.close_hidden(w, cx))),
            )
            .child(dialog)
            .into_any_element()
    }
}
