use std::rc::Rc;

use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Role, StatefulInteractiveElement,
    Styled, Window, div, prelude::*, px, uniform_list,
};
use pipkin_core::*;

use super::controls::*;
use super::subagents::InspectorTab;
use super::workspace::{Panel, Workspace};
use crate::theme::ActiveTheme;

impl Workspace {
    fn refresh_diff_rows(&mut self, cx: &gpui::App) {
        let key = {
            let s = self.state(cx);
            s.current().and_then(|c| {
                c.selected_change
                    .and_then(|i| c.changes.get(i).map(|_| (c.id, i, c.changes_revision)))
            })
        };
        if self.diff_rows.key == key {
            return;
        }
        self.diff_rows.key = key;
        self.diff_rows.rows.clear();
        self.diff_rows.widest = 0;
        self.diff_rows.change = None;
        let Some((id, i, ..)) = key else { return };
        let change = self
            .state(cx)
            .conversation(id)
            .and_then(|c| c.changes.get(i))
            .cloned();
        if let Some(f) = change {
            let mut widest = 0usize;
            let mut best = 0usize;
            for (h, hunk) in f.hunks.iter().enumerate() {
                self.diff_rows.rows.push((h as u32, u32::MAX));
                for (l, line) in hunk.lines.iter().enumerate() {
                    let w = line.text.chars().count();
                    if w > widest {
                        widest = w;
                        best = self.diff_rows.rows.len();
                    }
                    self.diff_rows.rows.push((h as u32, l as u32));
                }
            }
            self.diff_rows.widest = best;
            self.diff_rows.change = Some(Rc::new(f));
        }
    }

    pub(super) fn render_inspector(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let agents = self.inspector_tab == InspectorTab::Subagents;
        let plan = self.inspector_tab == InspectorTab::Plan;
        let jobs = self.inspector_tab == InspectorTab::Jobs;
        let jobs_available = self.state(cx).current().is_some_and(|c| c.jobs.available);
        if !agents && !plan {
            self.refresh_diff_rows(cx);
        }
        let t = cx.theme().clone();
        let c = &t.colors;
        let demo = self.state(cx).mode == Mode::Demo;
        let cli_workspace = self
            .state(cx)
            .current()
            .is_some_and(|c| c.profile.is_local_cli());
        let (changes, selected, scan_state) = {
            let s = self.state(cx);
            match s.current() {
                Some(cv) => (
                    cv.changes
                        .iter()
                        .map(|f| (f.path.clone(), f.added, f.removed, change_letter(f)))
                        .collect::<Vec<_>>(),
                    cv.selected_change,
                    cv.changes_state.clone(),
                ),
                None => (vec![], None, ChangesState::Unscanned),
            }
        };
        let this = cx.entity();
        let temp = self.temp_panel == Some(Panel::Inspector);
        let avail = self.state(cx).availability();
        let launch_buttons = (!demo && !agents).then(|| {
            let (editor_this, terminal_this, refresh_this) =
                (this.clone(), this.clone(), this.clone());
            div()
                .flex()
                .items_center()
                .gap(px(2.0))
                .child(
                    Btn::new("refresh-changes")
                        .icon("refresh-cw")
                        .aria(if cli_workspace {
                            "Refresh this project's Git state (not CLI action attribution)"
                        } else {
                            "Refresh this session's uncommitted changes"
                        })
                        .disabled(!avail.refresh_changes)
                        .on_click(move |_, cx| {
                            refresh_this.update(cx, |t, cx| t.dispatch(Command::RefreshChanges, cx))
                        }),
                )
                .child(
                    Btn::new("open-in-editor")
                        .icon("file-text")
                        .aria("Open the selected file in your editor")
                        .disabled(!avail.open_in_editor)
                        .on_click(move |_, cx| {
                            if let Some(i) = selected {
                                editor_this
                                    .update(cx, |t, cx| t.dispatch(Command::OpenInEditor(i), cx));
                            }
                        }),
                )
                .child(
                    Btn::new("open-terminal")
                        .icon("terminal")
                        .aria("Open a terminal in the project folder")
                        .disabled(!avail.open_terminal)
                        .on_click(move |_, cx| {
                            terminal_this.update(cx, |t, cx| t.dispatch(Command::OpenTerminal, cx))
                        }),
                )
        });

        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.0))
            .h(px(68.0 * t.scale.max(1.0)))
            .px(px(24.0))
            .border_b_1()
            .border_color(c.border)
            .child(
                div()
                    .id("inspector-title")
                    .role(Role::Heading)
                    .aria_label(if agents {
                        "Subagent tasks"
                    } else if demo {
                        "Workspace changes, demo"
                    } else {
                        "Session changes"
                    })
                    .flex_1()
                    .truncate()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(if agents {
                        "Subagents"
                    } else if plan {
                        "Plan"
                    } else if demo {
                        "Workspace changes · Demo"
                    } else {
                        "Session changes"
                    }),
            )
            .children(launch_buttons)
            .when(temp, |d| {
                let this = this.clone();
                d.child(
                    Btn::new("close-inspector")
                        .icon("x")
                        .aria("Close inspector")
                        .on_click(move |window, cx| {
                            this.update(cx, |t, cx| t.close_panel(window, cx))
                        }),
                )
            });

        let change_tab = this.clone();
        let agent_tab = this.clone();
        let plan_tab = this.clone();
        let jobs_tab = this.clone();
        let plan_available = self.state(cx).current().is_some_and(|c| c.plan.available);
        let tabs = div()
            .id("inspector-tabs")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.0))
            .px(px(12.0))
            .py(px(8.0))
            .border_b_1()
            .border_color(c.border)
            .child(
                Btn::new("inspector-changes-tab")
                    .label("Changes")
                    .selected(!agents && !plan && !jobs)
                    .aria(if cli_workspace {
                        "Show this project's Git state (not CLI action attribution)"
                    } else {
                        "Show this session's uncommitted changes"
                    })
                    .on_click(move |_, cx| {
                        change_tab.update(cx, |w, cx| {
                            w.inspector_tab = InspectorTab::Changes;
                            cx.notify();
                        });
                    }),
            )
            .child(
                Btn::new("inspector-subagents-tab")
                    .label(self.subagent_rows.tab_label())
                    .selected(agents)
                    .aria(format!(
                        "Show subagent tasks, {} active, {} total",
                        self.subagent_rows.active, self.subagent_rows.total
                    ))
                    .on_click(move |_, cx| {
                        agent_tab.update(cx, |w, cx| {
                            w.inspector_tab = InspectorTab::Subagents;
                            cx.notify();
                        });
                    }),
            )
            .when(jobs_available, |d| {
                d.child(
                    Btn::new("inspector-jobs-tab")
                        .label(self.job_rows.tab_label())
                        .selected(jobs)
                        .aria(format!(
                            "Show background jobs, {} active, {} finished{}",
                            self.job_rows.active,
                            self.job_rows.finished,
                            if self.job_rows.unseen > 0 {
                                format!(", {} finished since you looked", self.job_rows.unseen)
                            } else {
                                String::new()
                            }
                        ))
                        .on_click(move |_, cx| {
                            jobs_tab.update(cx, |w, cx| {
                                w.inspector_tab = InspectorTab::Jobs;
                                w.dispatch(Command::MarkJobsSeen, cx);
                                cx.notify();
                            });
                        }),
                )
            })
            .when(plan_available, |d| {
                d.child(
                    Btn::new("inspector-plan-tab")
                        .label(self.plan_rows.tab_label())
                        .selected(plan)
                        .aria(format!(
                            "Show the plan, {} open, {} finished",
                            self.plan_rows.open, self.plan_rows.finished
                        ))
                        .on_click(move |_, cx| {
                            plan_tab.update(cx, |w, cx| {
                                w.inspector_tab = InspectorTab::Plan;
                                cx.notify();
                            });
                        }),
                )
            });
        if jobs {
            return div()
                .id("inspector")
                .role(Role::Complementary)
                .aria_label("Jobs inspector")
                .flex()
                .flex_col()
                .size_full()
                .min_h_0()
                .bg(c.bg_changes)
                .child(header)
                .child(tabs)
                .child(self.render_jobs_panel(cx))
                .into_any_element();
        }
        if plan {
            return div()
                .id("inspector")
                .role(Role::Complementary)
                .aria_label("Plan inspector")
                .flex()
                .flex_col()
                .size_full()
                .min_h_0()
                .bg(c.bg_changes)
                .child(header)
                .child(tabs)
                .child(self.render_plan_panel(cx))
                .into_any_element();
        }
        if agents {
            return div()
                .id("inspector")
                .role(Role::Complementary)
                .aria_label("Subagents inspector")
                .flex()
                .flex_col()
                .size_full()
                .min_h_0()
                .bg(c.bg_changes)
                .child(header)
                .child(tabs)
                .child(self.render_subagent_panel(cx))
                .into_any_element();
        }
        let status = changes_status(&scan_state, !changes.is_empty());
        let banner = (!demo)
            .then_some(status.as_ref())
            .flatten()
            .map(|(title, detail)| {
                let this = this.clone();
                let failed = matches!(scan_state, ChangesState::Unavailable(_));
                div()
                    .id("changes-scan-status")
                    .role(if failed { Role::Alert } else { Role::Status })
                    .aria_label(format!("{title}. {detail}"))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .p(px(12.0))
                    .border_b_1()
                    .border_color(c.border)
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(if failed { c.warning } else { c.text_muted })
                            .child(*title),
                    )
                    .child(
                        div()
                            .id("changes-scan-detail")
                            .text_size(t.small_size())
                            .text_color(c.text_muted)
                            .max_h(px(100.0 * t.scale))
                            .overflow_y_scroll()
                            .child(detail.clone()),
                    )
                    .when(failed, |d| {
                        d.child(
                            Btn::new("retry-changes")
                                .label("Retry scan")
                                .aria("Retry workspace changes scan")
                                .disabled(!avail.refresh_changes)
                                .on_click(move |_, cx| {
                                    this.update(cx, |t, cx| t.dispatch(Command::RefreshChanges, cx))
                                }),
                        )
                    })
            });
        let body: gpui::AnyElement =
            if changes.is_empty() && !demo && status.is_some() {
                // Progress, failure and a non-Git folder are not a successful empty scan.
                div().flex_1().into_any_element()
            } else if changes.is_empty() {
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(8.0))
                    .px(px(24.0))
                    .child(
                        gpui::img(crate::assets::MASCOT_WAITING)
                            .w(px(150.0 * t.scale.max(1.0)))
                            .h(px(188.0 * t.scale.max(1.0)))
                            .object_fit(gpui::ObjectFit::Contain),
                    )
                    .child(div().text_color(c.text_muted).child(if demo {
                        "No changes yet"
                    } else {
                        "No uncommitted changes"
                    }))
                    .child(
                        div()
                            .text_size(t.small_size())
                            .text_color(c.text_faint)
                            .child(if demo {
                                "Simulated file edits appear here with their diffs."
                            } else {
                                "Changes clear after you commit."
                            }),
                    )
                    .into_any_element()
            } else {
                let count = changes.len();
                let list =
                    div()
                        .id("changed-files")
                        .role(Role::List)
                        .aria_label("Changed files")
                        .flex()
                        .flex_col()
                        .max_h(px(180.0 * t.scale))
                        .overflow_y_scroll()
                        .track_scroll(&self.changes_scroll)
                        .children(changes.iter().enumerate().map(
                            |(i, (path, add, rem, letter))| {
                                let this = this.clone();
                                let sel = selected == Some(i);
                                let badge = match letter {
                                    'A' => c.success,
                                    'D' => c.danger,
                                    _ => c.accent_fill,
                                };
                                menu_row(("change", i), sel, cx)
                                    .role(Role::ListItem)
                                    .aria_label(format!("{path}, {add} added, {rem} removed"))
                                    .aria_selected(sel)
                                    .rounded(px(0.0))
                                    .on_click(move |_, _, cx| {
                                        this.update(cx, |t, cx| {
                                            t.dispatch(Command::SelectChange(i), cx)
                                        })
                                    })
                                    .child(
                                        div()
                                            .flex()
                                            .flex_none()
                                            .items_center()
                                            .justify_center()
                                            .size(px(18.0 * t.scale.max(1.0)))
                                            .rounded(px(3.0))
                                            .bg(badge)
                                            .text_color(c.accent_text)
                                            .text_size(t.small_size())
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .child(letter.to_string()),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_ellipsis_start()
                                            .font_family(t.mono_font())
                                            .text_size(t.small_size())
                                            .child(path.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .font_family(t.mono_font())
                                            .text_size(t.small_size())
                                            .text_color(c.diff_add_text)
                                            .child(format!("+{add}")),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .font_family(t.mono_font())
                                            .text_size(t.small_size())
                                            .text_color(c.diff_remove_text)
                                            .child(format!("\u{2212}{rem}")),
                                    )
                            },
                        ));
                let files_card = div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .overflow_hidden()
                    .rounded(px(7.0))
                    .border_1()
                    .border_color(c.border_strong)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .px(px(12.0))
                            .py(px(8.0))
                            .border_b_1()
                            .border_color(c.border_strong)
                            .text_size(t.small_size())
                            .text_color(c.text_muted)
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("Files changed"),
                            )
                            .child(format!("{count} file{}", if count == 1 { "" } else { "s" })),
                    )
                    .child(list);
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .gap(px(10.0))
                    .p(px(12.0))
                    .child(self.render_diff(cx))
                    .child(files_card)
                    .into_any_element()
            };

        div()
            .id("inspector")
            .role(Role::Complementary)
            .aria_label("Changes inspector")
            .flex()
            .flex_col()
            .size_full()
            .bg(c.bg_changes)
            .child(header)
            .child(tabs)
            .children(banner)
            .when(!demo && !agents && !plan, |panel| {
                panel.child(
                    div()
                        .px(px(12.0))
                        .py(px(6.0))
                        .text_size(t.small_size())
                        .text_color(c.text_faint)
                        .child(changes_provenance(cli_workspace)),
                )
            })
            .child(body)
            .into_any_element()
    }

    fn render_diff(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let Some(change) = self.diff_rows.change.clone() else {
            return div().flex_1().into_any_element();
        };
        let rows: Rc<Vec<(u32, u32)>> = Rc::new(self.diff_rows.rows.clone());
        let count = rows.len();
        let lh = t.code_line_height();
        let (add_bg, rem_bg, hunk_bg) = (
            t.colors.diff_add_bg,
            t.colors.diff_remove_bg,
            t.colors.diff_hunk_bg,
        );
        let (add_fg, rem_fg, faint, text, muted) = (
            t.colors.diff_add_text,
            t.colors.diff_remove_text,
            t.colors.text_faint,
            t.colors.text,
            t.colors.text_muted,
        );
        let mono = t.mono_font();
        let size = t.code_size();
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.0))
            .px(px(10.0))
            .h(px(38.0))
            .border_b_1()
            .border_color(t.colors.border_strong)
            .child(icon("chevron-right", px(14.0), t.colors.accent))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(t.mono_font())
                    .text_size(t.small_size())
                    .child(change.path.clone()),
            )
            .child(
                div()
                    .text_size(t.small_size())
                    .text_color(add_fg)
                    .child(format!("+{}", change.added)),
            )
            .child(
                div()
                    .text_size(t.small_size())
                    .text_color(rem_fg)
                    .child(format!("−{}", change.removed)),
            );
        let list = uniform_list("diff-rows", count, {
            let change = change.clone();
            move |range, _window, _cx| {
                range
                    .map(|ix| {
                        let (h, l) = rows[ix];
                        let hunk = &change.hunks[h as usize];
                        if l == u32::MAX {
                            return div()
                                .flex()
                                .h(lh)
                                .min_w_full()
                                .bg(hunk_bg)
                                .px(px(8.0))
                                .text_color(muted)
                                .font_family(mono.clone())
                                .text_size(size)
                                .whitespace_nowrap()
                                .child(hunk.header.clone())
                                .into_any_element();
                        }
                        let line = &hunk.lines[l as usize];
                        let (bg, fg, sign) = match line.kind {
                            DiffKind::Add => (Some(add_bg), add_fg, "+"),
                            DiffKind::Remove => (Some(rem_bg), rem_fg, "-"),
                            DiffKind::Context => (None, text, " "),
                        };
                        let num = |n: Option<u32>| {
                            div()
                                .w(px(44.0))
                                .flex_none()
                                .text_color(faint)
                                .text_right()
                                .pr(px(6.0))
                                .child(n.map(|n| n.to_string()).unwrap_or_default())
                        };
                        div()
                            .flex()
                            .h(lh)
                            .min_w_full()
                            .when_some(bg, |d, b| d.bg(b))
                            .font_family(mono.clone())
                            .text_size(size)
                            .whitespace_nowrap()
                            .child(num(line.old_no))
                            .child(num(line.new_no))
                            .child(div().w(px(16.0)).flex_none().text_color(fg).child(sign))
                            .child(div().text_color(fg).pr(px(12.0)).child(line.text.clone()))
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }
        })
        .with_horizontal_sizing_behavior(gpui::ListHorizontalSizingBehavior::Unconstrained)
        .with_width_from_item(Some(self.diff_rows.widest))
        .track_scroll(&self.diff_scroll)
        .size_full();
        div()
            .id("diff")
            .role(Role::Document)
            .aria_label(format!("Diff for {}", change.path))
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .rounded(px(7.0))
            .border_1()
            .border_color(t.colors.border_strong)
            .bg(t.colors.code_bg)
            .child(header)
            .child(div().flex_1().min_h_0().child(list))
            .into_any_element()
    }
}

/// The one-letter kind shown beside a changed file: A for a new file, D for a removed one,
/// M for anything else. Inferred from the diff, which carries no explicit status.
fn changes_provenance(cli_workspace: bool) -> &'static str {
    if cli_workspace {
        "Project Git state, not proof of CLI actions. Executable Git filters are disabled."
    } else {
        "Session-associated Git state. Executable Git filters are disabled."
    }
}

fn changes_status(state: &ChangesState, has_changes: bool) -> Option<(&'static str, String)> {
    let previous = if has_changes {
        "Showing the last successful scan. These changes may be out of date."
    } else {
        "No current diff is available."
    };
    match state {
        ChangesState::Unavailable(reason) => Some((
            "Changes unavailable",
            format!("{reason}\n{previous} Fix the problem, then retry the scan."),
        )),
        ChangesState::Loading => Some(("Refreshing changes…", previous.into())),
        ChangesState::NotARepository => Some((
            "Not a Git repository",
            "Workspace diffs require a Git repository. You can still work with Pipkin in this folder."
                .into(),
        )),
        ChangesState::Unscanned if has_changes => {
            Some(("Changes awaiting refresh", previous.into()))
        }
        ChangesState::Unscanned | ChangesState::Ready => None,
    }
}

fn change_letter(f: &FileChange) -> char {
    let lines = || f.hunks.iter().flat_map(|h| h.lines.iter());
    let any = lines().next().is_some();
    if any && lines().all(|l| l.kind == DiffKind::Add) && f.removed == 0 {
        'A'
    } else if any && lines().all(|l| l.kind == DiffKind::Remove) && f.added == 0 {
        'D'
    } else {
        'M'
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_changes_are_project_observations_not_action_receipts() {
        assert!(changes_provenance(true).contains("not proof of CLI actions"));
        assert!(changes_provenance(false).contains("Session-associated"));
        for cli in [false, true] {
            assert!(changes_provenance(cli).contains("Executable Git filters are disabled"));
        }
    }

    fn line(kind: DiffKind) -> DiffLine {
        DiffLine {
            kind,
            old_no: None,
            new_no: None,
            text: String::new(),
        }
    }

    fn change(added: u32, removed: u32, kinds: &[DiffKind]) -> FileChange {
        FileChange {
            path: "a.rs".into(),
            added,
            removed,
            hunks: vec![Hunk {
                header: String::new(),
                lines: kinds.iter().map(|k| line(*k)).collect(),
            }],
        }
    }

    #[test]
    fn scan_copy_distinguishes_failure_progress_non_git_and_empty_success() {
        assert!(changes_status(&ChangesState::Ready, false).is_none());
        assert!(changes_status(&ChangesState::Unscanned, false).is_none());
        let (title, detail) =
            changes_status(&ChangesState::Unavailable("git missing".into()), true).unwrap();
        assert_eq!(title, "Changes unavailable");
        assert!(detail.contains("git missing"));
        assert!(detail.contains("out of date"));
        assert!(detail.contains("retry"));
        let (_, detail) = changes_status(
            &ChangesState::Unavailable("permission denied".into()),
            false,
        )
        .unwrap();
        assert!(detail.contains("No current diff"));
        assert!(!detail.contains("Showing the last"));
        assert_eq!(
            changes_status(&ChangesState::Loading, false).unwrap().0,
            "Refreshing changes…"
        );
        assert_eq!(
            changes_status(&ChangesState::NotARepository, false)
                .unwrap()
                .0,
            "Not a Git repository"
        );
    }

    #[test]
    fn a_file_is_new_removed_or_modified_by_what_its_diff_holds() {
        use DiffKind::*;
        assert_eq!(change_letter(&change(2, 0, &[Add, Add])), 'A');
        assert_eq!(change_letter(&change(0, 2, &[Remove, Remove])), 'D');
        assert_eq!(change_letter(&change(1, 1, &[Context, Remove, Add])), 'M');
        assert_eq!(
            change_letter(&change(0, 0, &[])),
            'M',
            "an empty diff is not called new"
        );
    }
}
