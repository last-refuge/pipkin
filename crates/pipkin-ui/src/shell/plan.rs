//! The plan inspector: the engine's task list for the open session, projected from core
//! state. Rows are prepared on model updates (never parsed during render) and shown in a
//! virtualized list so large plans mount only the rows in view. Open work is listed first;
//! finished work is counted and listed on request. One task can be expanded to its criteria,
//! evidence, reason and the person's edits, which go back through the engine's plan service.
//!
//! Counts are task counts. Nothing here estimates time or goal completion.

use std::cell::Cell;
use std::hash::{DefaultHasher, Hash, Hasher};

use gpui::{
    Context, IntoElement, ListAlignment, ListState, ParentElement, Role, Styled, div, list,
    prelude::*, px,
};
use pipkin_core::{
    Command, ConversationId, PlanCounts, PlanEdit, PlanState, PlanTask, PlanTaskStatus,
};

use super::subagents::sync_list;
use super::{controls::*, workspace::Workspace};
use crate::theme::ActiveTheme;

const OVERDRAW: f32 = 600.0;

/// One row of the virtualized plan list.
#[derive(Clone, Debug, PartialEq, Hash)]
enum PlanEntry {
    Empty,
    Task {
        id: u64,
        expanded: bool,
        content: u64,
    },
    HistoryToggle {
        open: bool,
        finished: usize,
    },
    Notice(String),
}

impl PlanEntry {
    fn identity(&self) -> u64 {
        let mut h = DefaultHasher::new();
        match self {
            PlanEntry::Empty => 0u8.hash(&mut h),
            PlanEntry::Task { id, .. } => (1u8, id).hash(&mut h),
            PlanEntry::HistoryToggle { .. } => 2u8.hash(&mut h),
            PlanEntry::Notice(_) => 3u8.hash(&mut h),
        }
        h.finish()
    }

    fn content(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.hash(&mut h);
        h.finish()
    }
}

fn task_content(task: &PlanTask) -> u64 {
    let mut h = DefaultHasher::new();
    (
        task.revision,
        task.status,
        &task.title,
        task.stale,
        &task.owner,
        &task.depends_on,
    )
        .hash(&mut h);
    h.finish()
}

pub(super) fn status_icon(status: PlanTaskStatus) -> &'static str {
    match status {
        PlanTaskStatus::Completed => "check",
        PlanTaskStatus::Active => "loader-circle",
        PlanTaskStatus::Blocked => "circle-alert",
        PlanTaskStatus::Skipped => "arrow-right",
        PlanTaskStatus::Cancelled => "square",
        PlanTaskStatus::Pending => "clock",
    }
}

/// `3 of 6 tasks done · 1 active, 1 blocked` — task counts, labelled as such.
pub(super) fn counts_line(counts: &PlanCounts) -> String {
    let mut parts = Vec::new();
    if counts.active > 0 {
        parts.push(format!("{} active", counts.active));
    }
    if counts.blocked > 0 {
        parts.push(format!("{} blocked", counts.blocked));
    }
    if counts.pending > 0 {
        parts.push(format!("{} pending", counts.pending));
    }
    let mut line = format!("{} of {} tasks done", counts.finished(), counts.live());
    if !parts.is_empty() {
        line.push_str(" · ");
        line.push_str(&parts.join(", "));
    }
    if counts.archived > 0 {
        line.push_str(&format!(" · {} archived", counts.archived));
    }
    line
}

pub(super) struct PlanRows {
    key: Option<(ConversationId, u64)>,
    /// What was last projected: revision, expanded task, history, error presence.
    projection: Option<(u64, Option<u64>, bool, bool, bool)>,
    entries: Vec<PlanEntry>,
    entry_keys: Vec<(u64, u64)>,
    pub open: usize,
    pub finished: usize,
    pub(super) list: ListState,
    /// Rows rendered in the last frame, for bounded-mount checks.
    pub(super) mounted: Cell<usize>,
}

impl Default for PlanRows {
    fn default() -> Self {
        PlanRows {
            key: None,
            projection: None,
            entries: Vec::new(),
            entry_keys: Vec::new(),
            open: 0,
            finished: 0,
            list: ListState::new(0, ListAlignment::Top, px(OVERDRAW)),
            mounted: Cell::new(0),
        }
    }
}

impl PlanRows {
    pub fn tab_label(&self) -> String {
        if self.open == 0 {
            "Plan".into()
        } else {
            format!("Plan {}", self.open)
        }
    }

    #[cfg(test)]
    fn listed_tasks(&self) -> Vec<(u64, bool)> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                PlanEntry::Task { id, expanded, .. } => Some((*id, *expanded)),
                _ => None,
            })
            .collect()
    }

    pub fn refresh(&mut self, key: Option<(ConversationId, u64)>, plan: Option<&PlanState>) {
        let empty = PlanState::default();
        let plan = plan.unwrap_or(&empty);
        let projection = Some((
            plan.revision,
            plan.expanded,
            plan.history_open,
            plan.error.is_some(),
            plan.pending_edit.is_some(),
        ));
        // Different revisions always re-project; the same revision with the same choices is
        // the same list, but a resync at the same revision is cheap to confirm by content.
        let scope_changed = self.key != key;
        if !scope_changed && self.projection == projection && !plan.tasks.is_empty() {
            let same = plan
                .tasks
                .iter()
                .zip(self.entries.iter().filter(|e| matches!(e, PlanEntry::Task { .. })))
                .all(|(t, e)| matches!(e, PlanEntry::Task { id, content, .. } if *id == t.id && *content == task_content(t)));
            if same {
                return;
            }
        }
        self.key = key;
        self.projection = projection;
        self.open = plan.counts.open() as usize;
        self.finished = plan.counts.finished() as usize;
        let mut entries = Vec::new();
        if plan.tasks.is_empty() {
            entries.push(PlanEntry::Empty);
        }
        for task in plan.tasks.iter().filter(|t| !t.status.finished()) {
            entries.push(PlanEntry::Task {
                id: task.id,
                expanded: plan.expanded == Some(task.id),
                content: task_content(task),
            });
        }
        let finished = plan.tasks.iter().filter(|t| t.status.finished()).count();
        if finished > 0 {
            entries.push(PlanEntry::HistoryToggle {
                open: plan.history_open,
                finished,
            });
            if plan.history_open {
                // Newest completion first, like subagent history.
                for task in plan.tasks.iter().rev().filter(|t| t.status.finished()) {
                    entries.push(PlanEntry::Task {
                        id: task.id,
                        expanded: plan.expanded == Some(task.id),
                        content: task_content(task),
                    });
                }
            }
        }
        if plan.truncated {
            entries.push(PlanEntry::Notice(
                "Some archived tasks are not listed here; ask for the plan to see them.".into(),
            ));
        }
        let keys: Vec<(u64, u64)> = entries
            .iter()
            .map(|e| (e.identity(), e.content()))
            .collect();
        if scope_changed {
            self.list.reset(keys.len());
        } else {
            sync_list(&self.list, &self.entry_keys, &keys, 0);
        }
        self.entries = entries;
        self.entry_keys = keys;
    }
}

impl Workspace {
    pub(super) fn refresh_plan_rows(&mut self, cx: &gpui::App) {
        let model = self.model.read(cx);
        let current = model.state.current();
        self.plan_rows.refresh(
            current.map(|c| (c.id, c.generation)),
            current.map(|c| &c.plan),
        );
    }

    fn plan_status(&self, status: PlanTaskStatus, stale: bool, cx: &gpui::App) -> impl IntoElement {
        let t = cx.theme();
        let c = &t.colors;
        let fg = match status {
            PlanTaskStatus::Completed if stale => c.warning,
            PlanTaskStatus::Completed => c.success,
            PlanTaskStatus::Active => c.warning,
            PlanTaskStatus::Blocked | PlanTaskStatus::Cancelled => c.danger,
            _ => c.text_muted,
        };
        div()
            .flex()
            .items_center()
            .gap(px(5.0))
            .flex_none()
            .text_size(t.small_size())
            .text_color(fg)
            .child(icon(status_icon(status), px(14.0 * t.scale.max(1.0)), fg))
            .child(if stale {
                "Completed · stale"
            } else {
                status.label()
            })
    }

    fn render_plan_entry(&mut self, ix: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(entry) = self.plan_rows.entries.get(ix).cloned() else {
            return div().into_any_element();
        };
        self.plan_rows.mounted.set(self.plan_rows.mounted.get() + 1);
        let t = cx.theme().clone();
        let c = &t.colors;
        let this = cx.entity();
        match entry {
            PlanEntry::Empty => div()
                .px(px(8.0))
                .pt(px(8.0))
                .child(
                    div()
                        .p(px(12.0))
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child("No plan yet"),
                        )
                        .child(div().text_size(t.small_size()).text_color(c.text_muted).child(
                            "When Pipkin lays out multi-step work, its tasks appear here with their status, dependencies and evidence.",
                        )),
                )
                .into_any_element(),
            PlanEntry::HistoryToggle { open, finished } => div()
                .px(px(8.0))
                .pt(px(8.0))
                .child(
                    Btn::new("plan-finished-toggle")
                        .icon(if open { "chevron-down" } else { "chevron-right" })
                        .label(format!(
                            "{} finished tasks ({finished})",
                            if open { "Hide" } else { "Show" }
                        ))
                        .aria(format!(
                            "{} finished plan tasks, {finished}",
                            if open { "Hide" } else { "Show" }
                        ))
                        .on_click(move |_, cx| {
                            this.update(cx, |w, cx| w.dispatch(Command::TogglePlanHistory, cx));
                        }),
                )
                .into_any_element(),
            PlanEntry::Notice(text) => div()
                .px(px(16.0))
                .py(px(12.0))
                .text_size(t.small_size())
                .text_color(c.text_muted)
                .child(text)
                .into_any_element(),
            PlanEntry::Task { id, expanded, .. } => {
                let model = self.model.read(cx);
                let Some(task) = model
                    .state
                    .current()
                    .and_then(|cv| cv.plan.task(id))
                    .cloned()
                else {
                    return div().into_any_element();
                };
                let can_edit = model.state.can_edit_plan();
                let error = model
                    .state
                    .current()
                    .and_then(|cv| cv.plan.error.clone())
                    .filter(|_| expanded);
                self.render_plan_task(task, expanded, can_edit, error, cx)
            }
        }
    }

    fn render_plan_task(
        &self,
        task: PlanTask,
        expanded: bool,
        can_edit: bool,
        error: Option<String>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let t = cx.theme().clone();
        let c = &t.colors;
        let id = task.id;
        let click = cx.entity();
        let key = cx.entity();
        let mut meta = Vec::new();
        if !task.depends_on.is_empty() {
            let deps: Vec<String> = task.depends_on.iter().map(|d| format!("#{d}")).collect();
            meta.push(format!("after {}", deps.join(", ")));
        }
        if let Some(parent) = task.parent {
            meta.push(format!("under #{parent}"));
        }
        if let Some(owner) = &task.owner {
            meta.push(owner.clone());
        }
        let aria = format!(
            "Task {}: {}, {}{}. {}",
            task.id,
            task.title,
            task.status.label(),
            if task.stale { ", stale" } else { "" },
            if expanded { "Collapse" } else { "Expand" }
        );
        let mut row = menu_row(("plan-task", id as usize), expanded, cx)
            .role(Role::Button)
            .aria_label(aria)
            .flex_col()
            .items_start()
            .gap(px(6.0))
            .py(px(10.0))
            .w_full()
            .min_w_0()
            .on_click(move |_, _, cx| {
                click.update(cx, |w, cx| w.dispatch(Command::TogglePlanTask(id), cx));
            })
            .on_key_down(move |event, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    key.update(cx, |w, cx| w.dispatch(Command::TogglePlanTask(id), cx));
                    cx.stop_propagation();
                }
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .w_full()
                    .min_w_0()
                    .child(
                        div()
                            .flex_none()
                            .text_size(t.small_size())
                            .text_color(c.text_muted)
                            .child(format!("#{id}")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(task.title.clone()),
                    )
                    .child(icon(
                        if expanded {
                            "chevron-down"
                        } else {
                            "chevron-right"
                        },
                        px(14.0),
                        c.text_muted,
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(self.plan_status(task.status, task.stale, cx))
                    .when(!meta.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(t.small_size())
                                .text_color(c.text_muted)
                                .child(meta.join(" · ")),
                        )
                    }),
            );
        if expanded {
            let small = |label: &'static str, text: String| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .text_size(t.small_size())
                    .child(div().text_color(c.text_muted).child(label))
                    .child(text)
            };
            let mut detail = div().flex().flex_col().gap(px(8.0)).w_full().min_w_0();
            if !task.detail.is_empty() {
                detail = detail.child(small("Detail", task.detail.clone()));
            }
            if !task.criteria.is_empty() {
                detail = detail.child(small("Done when", task.criteria.clone()));
            }
            if let Some(reason) = &task.reason {
                detail = detail.child(small("Reason", reason.clone()));
            }
            if task.stale {
                detail = detail.child(div().text_size(t.small_size()).text_color(c.warning).child(
                    "This completion's basis changed since it was recorded; it needs re-checking.",
                ));
            }
            if task.evidence.is_empty() {
                if task.status == PlanTaskStatus::Completed {
                    detail = detail.child(
                        div()
                            .text_size(t.small_size())
                            .text_color(c.text_muted)
                            .child("No evidence recorded"),
                    );
                }
            } else {
                let mut list = div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .text_size(t.small_size())
                    .child(div().text_color(c.text_muted).child("Evidence"));
                for e in &task.evidence {
                    let line = match &e.note {
                        Some(note) => format!("{}: {} — {note}", e.kind, e.reference),
                        None => format!("{}: {}", e.kind, e.reference),
                    };
                    list = list.child(div().font_family(t.mono_font()).child(line));
                }
                detail = detail.child(list);
            }
            let mut actions = div().flex().flex_wrap().gap(px(6.0));
            for edit in PlanEdit::offered(task.status) {
                let edit = *edit;
                let this = cx.entity();
                actions = actions.child(
                    Btn::new(gpui::SharedString::from(format!(
                        "plan-{}-{id}",
                        edit.label()
                    )))
                    .label(edit.label())
                    .compact()
                    .kind(if edit == PlanEdit::Cancel {
                        BtnKind::Danger
                    } else {
                        BtnKind::Subtle
                    })
                    .disabled(!can_edit)
                    .aria(format!("{} task {id}", edit.label()))
                    .on_click(move |_, cx| {
                        this.update(cx, |w, cx| {
                            w.dispatch(Command::EditPlanTask { task: id, edit }, cx)
                        });
                    }),
                );
            }
            detail = detail.child(actions);
            if let Some(error) = error {
                let this = cx.entity();
                detail = detail.child(
                    div()
                        .id(("plan-error", id as usize))
                        .role(Role::Alert)
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .text_size(t.small_size())
                        .text_color(c.warning)
                        .child(div().flex_1().min_w_0().child(error))
                        .child(
                            Btn::new("plan-error-dismiss")
                                .icon("x")
                                .compact()
                                .aria("Dismiss plan error")
                                .on_click(move |_, cx| {
                                    this.update(cx, |w, cx| {
                                        w.dispatch(Command::DismissPlanError, cx)
                                    })
                                }),
                        ),
                );
            }
            row = row.child(detail);
        }
        div()
            .px(px(8.0))
            .child(row.child(separator(cx)))
            .into_any_element()
    }

    pub(super) fn render_plan_panel(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        self.plan_rows.mounted.set(0);
        let t = cx.theme().clone();
        let c = &t.colors;
        let model = self.model.read(cx);
        let plan = model.state.current().map(|v| &v.plan);
        let this = cx.entity();
        let panel = div()
            .id("plan-inspector")
            .role(Role::Region)
            .aria_label("Plan")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0();
        let Some(plan) = plan else {
            return panel
                .child(
                    div()
                        .p(px(20.0))
                        .text_color(c.text_muted)
                        .child("Open a conversation to see its plan."),
                )
                .into_any_element();
        };
        if !plan.available {
            return panel
                .child(
                    div()
                        .p(px(20.0))
                        .text_color(c.text_muted)
                        .child("This engine does not provide a plan."),
                )
                .into_any_element();
        }
        let summary = counts_line(&plan.counts);
        let pending = plan.pending_edit.is_some();
        let _ = this;
        panel
            .child(
                div()
                    .id("plan-summary")
                    .role(Role::Status)
                    .aria_label(summary.clone())
                    .flex_none()
                    .px(px(16.0))
                    .py(px(12.0))
                    .border_b_1()
                    .border_color(c.border)
                    .text_size(t.small_size())
                    .text_color(c.text_muted)
                    .child(summary)
                    .when(pending, |d| d.child(" · applying your change…")),
            )
            .child(
                list(
                    self.plan_rows.list.clone(),
                    cx.processor(|this, ix: usize, _window, cx| this.render_plan_entry(ix, cx)),
                )
                .size_full(),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: u64, status: PlanTaskStatus, deps: &[u64]) -> PlanTask {
        PlanTask {
            id,
            parent: None,
            title: format!("Task {id}"),
            detail: String::new(),
            status,
            reason: None,
            depends_on: deps.to_vec(),
            criteria: String::new(),
            evidence: vec![],
            owner: None,
            archived: false,
            stale: false,
            revision: 1,
        }
    }

    fn plan(tasks: Vec<PlanTask>) -> PlanState {
        let mut counts = PlanCounts::default();
        for t in &tasks {
            match t.status {
                PlanTaskStatus::Pending => counts.pending += 1,
                PlanTaskStatus::Active => counts.active += 1,
                PlanTaskStatus::Blocked => counts.blocked += 1,
                PlanTaskStatus::Completed => counts.completed += 1,
                PlanTaskStatus::Skipped => counts.skipped += 1,
                PlanTaskStatus::Cancelled => counts.cancelled += 1,
            }
        }
        PlanState {
            available: true,
            revision: 1,
            tasks,
            counts,
            ..PlanState::default()
        }
    }

    #[gpui::test]
    fn banner_can_be_closed_without_deleting_tasks_and_new_work_gets_a_new_banner(
        cx: &mut gpui::TestAppContext,
    ) {
        use pipkin_core::{
            AppState, BackendEvent, Bootstrap, EventKind, Prefs, Project, ProjectId,
        };
        cx.update(crate::text::init);
        let mut state = AppState::new(
            Bootstrap {
                projects: vec![Project {
                    id: ProjectId(1),
                    name: "Project".into(),
                    path: "/project".into(),
                }],
                conversations: vec![(ConversationId(1), ProjectId(1), "Chat".into(), 1)],
                models: vec![],
                now: 1,
            },
            Prefs::default(),
        );
        state.selected = Some(ConversationId(1));
        state.conversations[0].opened = true;
        state.conversations[0].plan = plan(vec![task(1, PlanTaskStatus::Active, &[])]);
        let window = cx.add_window(|window, cx| {
            let model = cx.new(|_| crate::model::Model::new(state));
            model.update(cx, |m, cx| m.explore_workspace(cx));
            Workspace::new(model, Default::default(), window, cx)
        });
        let root = window.root(cx).unwrap();
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        assert!(visual.debug_bounds("plan-banner").is_some());
        let close = visual.debug_bounds("dismiss-plan-banner").unwrap();
        visual.simulate_click(close.center(), Default::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("plan-banner").is_none());
        root.update(&mut visual, |w, cx| {
            assert_eq!(w.state(cx).current().unwrap().plan.tasks.len(), 1);
            assert_eq!(
                w.state(cx).current().unwrap().plan.tasks[0].status,
                PlanTaskStatus::Active
            );
            w.dispatch(Command::TogglePlanTask(1), cx);
            assert_eq!(w.state(cx).current().unwrap().plan.expanded, Some(1));
        });
        for (revision, status, visible) in [
            (2, PlanTaskStatus::Active, true),
            (3, PlanTaskStatus::Completed, false),
        ] {
            let p = plan(vec![task(2, status, &[])]);
            root.update(&mut visual, |w, cx| {
                let c = w.state(cx).current().unwrap();
                let event = BackendEvent {
                    conversation: c.id,
                    generation: c.generation,
                    op: None,
                    kind: EventKind::PlanSynced {
                        revision,
                        tasks: p.tasks,
                        counts: p.counts,
                        truncated: false,
                    },
                };
                w.model.update(cx, |m, cx| m.apply_event(event, cx));
            });
            visual.run_until_parked();
            assert_eq!(visual.debug_bounds("plan-banner").is_some(), visible);
        }
    }

    #[test]
    fn open_work_is_listed_and_finished_work_is_counted_until_opened() {
        let key = Some((ConversationId(1), 1));
        let mut rows = PlanRows::default();
        let p = plan(vec![
            task(1, PlanTaskStatus::Completed, &[]),
            task(2, PlanTaskStatus::Active, &[1]),
            task(3, PlanTaskStatus::Skipped, &[]),
            task(4, PlanTaskStatus::Pending, &[2]),
        ]);
        rows.refresh(key, Some(&p));
        assert_eq!(rows.listed_tasks(), [(2, false), (4, false)]);
        assert_eq!(rows.tab_label(), "Plan 2");
        assert!(matches!(
            rows.entries.last(),
            Some(PlanEntry::HistoryToggle {
                open: false,
                finished: 2
            })
        ));
        assert_eq!(
            counts_line(&p.counts),
            "2 of 4 tasks done · 1 active, 1 pending"
        );
        let mut opened = p.clone();
        opened.history_open = true;
        opened.expanded = Some(1);
        rows.refresh(key, Some(&opened));
        assert_eq!(
            rows.listed_tasks(),
            [(2, false), (4, false), (3, false), (1, true)],
            "finished tasks newest first, the expanded one marked"
        );
        assert_eq!(rows.list.item_count(), rows.entries.len());
    }

    #[test]
    fn an_empty_or_unavailable_plan_has_one_row_and_a_plain_label() {
        let mut rows = PlanRows::default();
        rows.refresh(Some((ConversationId(1), 1)), Some(&PlanState::default()));
        assert!(matches!(rows.entries.as_slice(), [PlanEntry::Empty]));
        assert_eq!(rows.tab_label(), "Plan");
        rows.refresh(None, None);
        assert!(matches!(rows.entries.as_slice(), [PlanEntry::Empty]));
    }

    #[test]
    fn a_status_change_at_a_new_revision_reprojects_in_place() {
        let key = Some((ConversationId(1), 1));
        let mut rows = PlanRows::default();
        rows.refresh(key, Some(&plan(vec![task(1, PlanTaskStatus::Active, &[])])));
        let mut next = plan(vec![task(1, PlanTaskStatus::Completed, &[])]);
        next.revision = 2;
        rows.refresh(key, Some(&next));
        assert!(rows.listed_tasks().is_empty());
        assert_eq!(rows.finished, 1);
        assert!(matches!(
            rows.entries.as_slice(),
            [PlanEntry::HistoryToggle {
                open: false,
                finished: 1
            }]
        ));
    }
}
