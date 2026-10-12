//! A focused task directory/detail view, separate from workspace diffs. Presentation is
//! prepared on model updates; rendering never parses task prompts or guesses progress.
//!
//! Both the directory and a task's activity are virtualized lists, so only the rows in view
//! are mounted however many tasks or entries are loaded. Paging state (revealed history, the
//! older-activity cursor) is the core's; this view only projects it and keeps the reading
//! position when rows are added above it.
use std::cell::Cell;
use std::hash::{DefaultHasher, Hash, Hasher};

use super::{controls::*, workspace::Workspace};
use crate::theme::ActiveTheme;
use gpui::{
    Context, IntoElement, ListAlignment, ListOffset, ListState, ParentElement, Role, Styled, div,
    list, prelude::*, px,
};
use pipkin_core::{Command, ConversationId, SUBAGENT_HISTORY_PAGE, SubagentInfo, SubagentState};

const OVERDRAW: f32 = 600.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum InspectorTab {
    #[default]
    Changes,
    Subagents,
    Plan,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum TaskStatus {
    Pending,
    Running,
    Waiting,
    Completing,
    Done,
    Failed,
    Aborted,
    Unknown,
}
impl TaskStatus {
    fn from_engine(value: &str) -> Self {
        match value {
            "pending" => Self::Pending,
            "running" => Self::Running,
            "waiting" => Self::Waiting,
            "completing" => Self::Completing,
            "done" => Self::Done,
            "failed" => Self::Failed,
            "aborted" => Self::Aborted,
            _ => Self::Unknown,
        }
    }
    fn active(self) -> bool {
        matches!(
            self,
            Self::Pending | Self::Running | Self::Waiting | Self::Completing
        )
    }
    fn finished(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Aborted)
    }
    fn label(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Running => "Running",
            Self::Waiting => "Waiting",
            Self::Completing => "Completing",
            Self::Done => "Completed",
            Self::Failed => "Failed",
            Self::Aborted => "Stopped",
            Self::Unknown => "Unknown",
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Self::Done => "check",
            Self::Failed => "circle-alert",
            Self::Aborted => "square",
            Self::Running | Self::Completing => "loader-circle",
            _ => "clock",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Hash)]
pub(super) struct TaskRow {
    pub id: u64,
    pub title: String,
    pub task: String,
    pub summary: String,
    status: TaskStatus,
}

/// One row of the virtualized directory, in display order.
#[derive(Clone, Debug, PartialEq, Hash)]
enum DirEntry {
    Empty,
    HistoryToggle { open: bool, finished: usize },
    Idle,
    Task(TaskRow),
    ShowMore { shown: usize, finished: usize },
    Notice(String),
}
impl DirEntry {
    /// Identity across updates (a task keeps its identity while its status changes).
    fn identity(&self) -> u64 {
        let mut h = DefaultHasher::new();
        match self {
            DirEntry::Task(row) => (3u8, row.id).hash(&mut h),
            DirEntry::Empty => 0u8.hash(&mut h),
            DirEntry::HistoryToggle { .. } => 1u8.hash(&mut h),
            DirEntry::Idle => 2u8.hash(&mut h),
            DirEntry::ShowMore { .. } => 4u8.hash(&mut h),
            DirEntry::Notice(_) => 5u8.hash(&mut h),
        }
        h.finish()
    }
    fn content(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.hash(&mut h);
        h.finish()
    }
}

/// Bring a list's item count in line with new `(identity, revision)` keys without losing the
/// reading position: unchanged rows keep their measurements, same-identity rows are remeasured,
/// and only the differing middle is spliced. Rows inserted above the scroll top shift it, so
/// the rows being read stay in place. `offset` is the number of fixed rows before the keys.
pub(super) fn sync_list(list: &ListState, old: &[(u64, u64)], new: &[(u64, u64)], offset: usize) {
    let p = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let s = old[p..]
        .iter()
        .rev()
        .zip(new[p..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let (old_mid, new_mid) = (&old[p..old.len() - s], &new[p..new.len() - s]);
    let q = old_mid
        .iter()
        .zip(new_mid)
        .take_while(|(a, b)| a.0 == b.0)
        .count();
    let start = offset + p;
    if q > 0 {
        list.remeasure_items(start..start + q);
    }
    let (o, n) = (old_mid.len() - q, new_mid.len() - q);
    if o > 0 || n > 0 {
        let top = list.logical_scroll_top();
        list.splice(start + q..start + q + o, n);
        // Rows were added in front of everything shown while the header row was at the top
        // (e.g. "Load earlier activity" was just used): keep the first row that was being read
        // at the top instead of revealing the new rows above it.
        if o == 0 && p == 0 && q == 0 && s == old.len() && !old.is_empty() && top.item_ix < offset {
            list.scroll_to(ListOffset {
                item_ix: offset + n,
                offset_in_item: px(0.),
            });
        }
    }
}

pub(super) struct SubagentRows {
    key: Option<(ConversationId, u64)>,
    source: Vec<SubagentInfo>,
    /// Revealed history, paging mode and truncation as last projected.
    projection: (usize, bool, bool),
    pub rows: Vec<TaskRow>,
    pub finished_rows: Vec<TaskRow>,
    pub selected_row: Option<TaskRow>,
    pub total: usize,
    pub active: usize,
    pub completed: usize,
    pub attention: usize,
    entries: Vec<DirEntry>,
    entry_keys: Vec<(u64, u64)>,
    pub(super) directory: ListState,
    activity_key: Option<(ConversationId, u64, u64)>,
    activity_keys: Vec<(u64, u64)>,
    pub(super) activity: ListState,
    /// Rows rendered in the last frame (both lists), for bounded-mount checks.
    pub(super) mounted: Cell<usize>,
    /// Task ids of directory rows rendered since the last reset (tests and diagnostics).
    pub(super) mounted_tasks: std::cell::RefCell<Vec<u64>>,
}

impl Default for SubagentRows {
    fn default() -> Self {
        SubagentRows {
            key: None,
            source: Vec::new(),
            projection: (0, false, false),
            rows: Vec::new(),
            finished_rows: Vec::new(),
            selected_row: None,
            total: 0,
            active: 0,
            completed: 0,
            attention: 0,
            entries: Vec::new(),
            entry_keys: Vec::new(),
            directory: ListState::new(0, ListAlignment::Top, px(OVERDRAW)),
            activity_key: None,
            activity_keys: Vec::new(),
            activity: ListState::new(1, ListAlignment::Top, px(OVERDRAW)),
            mounted: Cell::new(0),
            mounted_tasks: Default::default(),
        }
    }
}

impl SubagentRows {
    pub fn tab_label(&self) -> String {
        if self.active == 0 {
            "Subagents".into()
        } else {
            format!("Subagents {}", self.active)
        }
    }
    fn finished_count(&self) -> usize {
        self.completed + self.attention
    }
    #[cfg(test)]
    fn visible_rows(&self) -> impl Iterator<Item = &TaskRow> {
        self.entries.iter().filter_map(|e| match e {
            DirEntry::Task(row) => Some(row),
            _ => None,
        })
    }

    pub fn refresh(&mut self, key: Option<(ConversationId, u64)>, state: Option<&SubagentState>) {
        let empty = SubagentState::default();
        let state = state.unwrap_or(&empty);
        self.refresh_activity(key, state);
        let children = state.children.as_slice();
        let selected = state.selected;
        let projection = (state.history_shown(), state.paged, state.truncated);
        if self.key == key
            && self.source == children
            && self.projection == projection
            && self.selected_row.as_ref().map(|r| r.id) == selected
        {
            return;
        }
        self.selected_row = selected
            .and_then(|id| children.iter().find(|c| c.id == id))
            .map(task_row);
        if self.key == key && self.source == children && self.projection == projection {
            return;
        }
        let scope_changed = self.key != key;
        self.key = key;
        self.source = children.to_vec();
        self.projection = projection;
        self.total = children.len();
        self.active = 0;
        self.completed = 0;
        self.attention = 0;
        for child in children {
            match TaskStatus::from_engine(&child.status) {
                status if status.active() => self.active += 1,
                TaskStatus::Done => self.completed += 1,
                TaskStatus::Failed | TaskStatus::Aborted => self.attention += 1,
                _ => {}
            }
        }
        let (history, paged, truncated) = projection;
        // Keep current work separate from collapsed history. Unknown statuses remain visible
        // rather than being mistaken for a terminal outcome. Unpaged (Pi) directories keep a
        // 100-row bound per group; paged directories list all current work (virtualized) and
        // reveal history a page at a time, newest first.
        self.rows = children
            .iter()
            .filter(|child| !TaskStatus::from_engine(&child.status).finished())
            .take(if paged { usize::MAX } else { 100 })
            .map(task_row)
            .collect();
        self.finished_rows = children
            .iter()
            .rev()
            .filter(|child| TaskStatus::from_engine(&child.status).finished())
            .take(history)
            .map(task_row)
            .collect();
        let finished = self.finished_count();
        let mut entries = Vec::new();
        if self.total == 0 {
            entries.push(DirEntry::Empty);
        }
        if finished > 0 {
            entries.push(DirEntry::HistoryToggle {
                open: history > 0,
                finished,
            });
            if self.rows.is_empty() {
                entries.push(DirEntry::Idle);
            }
        }
        entries.extend(self.rows.iter().cloned().map(DirEntry::Task));
        entries.extend(self.finished_rows.iter().cloned().map(DirEntry::Task));
        if paged && history > 0 && finished > history {
            entries.push(DirEntry::ShowMore {
                shown: self.finished_rows.len(),
                finished,
            });
        }
        let current = self.total - finished;
        if paged {
            if truncated {
                entries.push(DirEntry::Notice(
                    "The engine's task directory reached its size limit; later tasks are not listed here."
                        .into(),
                ));
            }
        } else if current > 100 || (history > 0 && finished > 100) {
            entries.push(DirEntry::Notice(format!(
                "Showing up to 100 current tasks and 100 finished tasks when history is open ({current} current, {finished} finished)."
            )));
        }
        let keys: Vec<(u64, u64)> = entries
            .iter()
            .map(|e| (e.identity(), e.content()))
            .collect();
        if scope_changed {
            self.directory.reset(keys.len());
        } else {
            sync_list(&self.directory, &self.entry_keys, &keys, 0);
        }
        self.entries = entries;
        self.entry_keys = keys;
    }

    /// The activity list: row 0 is the header (task excerpt, paging control), then one row per
    /// prepared preview. Selecting another task starts it at the top.
    fn refresh_activity(&mut self, key: Option<(ConversationId, u64)>, state: &SubagentState) {
        let activity_key = key.zip(state.selected).map(|((c, g), child)| (c, g, child));
        let keys: Vec<(u64, u64)> = state.previews.iter().map(|p| (p.id.0, p.rev)).collect();
        if self.activity_key != activity_key {
            self.activity_key = activity_key;
            self.activity.reset(keys.len() + 1);
        } else {
            sync_list(&self.activity, &self.activity_keys, &keys, 1);
            // The header shows loading/error/paging state, which changes without a splice.
            self.activity.remeasure_items(0..1);
        }
        self.activity_keys = keys;
    }
}

fn task_row(child: &SubagentInfo) -> TaskRow {
    let description = if child.task.starts_with("You are ") {
        child
            .task
            .split_once('.')
            .map_or(child.task.as_str(), |(_, rest)| rest.trim())
    } else {
        &child.task
    };
    let mut summary: String = description.chars().take(120).collect();
    if description.chars().count() > 120 {
        summary.push('…');
    }
    let title = task_title(&child.task, child.id);
    if summary.trim_end_matches('.') == title {
        summary.clear();
    }
    TaskRow {
        id: child.id,
        title,
        task: child.task.clone(),
        summary,
        status: TaskStatus::from_engine(&child.status),
    }
}

// This is a UI-derived label from an engine task excerpt, never a claimed agent identity.
fn task_title(task: &str, id: u64) -> String {
    let first = task.trim().split(['\n', '.']).next().unwrap_or("").trim();
    let first = first.strip_prefix("You are ").unwrap_or(first);
    if !first.is_empty() && first.chars().count() <= 56 {
        first.to_owned()
    } else {
        format!("Subagent {id}")
    }
}

fn view_state(
    loading: bool,
    live: bool,
    failed: bool,
    viewed: bool,
    retained: bool,
) -> &'static str {
    if loading {
        "Loading activity…"
    } else if failed && retained {
        "Disconnected · retained preview"
    } else if failed {
        "Activity unavailable"
    } else if live {
        "Live activity"
    } else if viewed {
        "Snapshot"
    } else {
        "Activity unavailable"
    }
}

impl Workspace {
    pub(super) fn refresh_subagent_rows(&mut self, cx: &gpui::App) {
        let model = self.model.read(cx);
        let current = model.state.current();
        self.subagent_rows.refresh(
            current.map(|c| (c.id, c.generation)),
            current.map(|c| &c.subagents),
        );
    }

    fn task_status(&self, status: TaskStatus, cx: &gpui::App) -> impl IntoElement {
        let t = cx.theme();
        let c = &t.colors;
        let fg = match status {
            TaskStatus::Done => c.success,
            TaskStatus::Failed => c.danger,
            status if status.active() => c.warning,
            _ => c.text_muted,
        };
        div()
            .flex()
            .items_center()
            .gap(px(5.0))
            .flex_none()
            .text_size(t.small_size())
            .text_color(fg)
            .child(icon(status.icon(), px(14.0 * t.scale.max(1.0)), fg))
            .child(status.label())
    }

    fn render_directory_entry(&mut self, ix: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(entry) = self.subagent_rows.entries.get(ix).cloned() else {
            return div().into_any_element();
        };
        let rows = &self.subagent_rows;
        rows.mounted.set(rows.mounted.get() + 1);
        let t = cx.theme().clone();
        let c = &t.colors;
        let this = cx.entity();
        let pad = |d: gpui::Div| d.px(px(8.0));
        match entry {
            DirEntry::Empty => pad(div())
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
                                .child("No delegated tasks yet"),
                        )
                        .child(div().text_size(t.small_size()).text_color(c.text_muted).child(
                            "When Pi delegates work, follow each task here. Ask it to use subagents for independent work.",
                        )),
                )
                .into_any_element(),
            DirEntry::HistoryToggle { open, finished } => pad(div())
                .pt(px(8.0))
                .child(
                    Btn::new("subagents-finished-toggle")
                        .icon(if open { "chevron-down" } else { "chevron-right" })
                        .label(format!(
                            "{} finished tasks ({finished})",
                            if open { "Hide" } else { "Show" }
                        ))
                        .aria(format!(
                            "{} finished subagent history",
                            if open { "Hide" } else { "Show" }
                        ))
                        .on_click(move |_, cx| {
                            this.update(cx, |w, cx| {
                                w.dispatch(Command::ToggleSubagentHistory, cx)
                            });
                        }),
                )
                .into_any_element(),
            DirEntry::Idle => pad(div())
                .child(
                    div()
                        .id("subagents-idle")
                        .role(Role::Status)
                        .p(px(12.0))
                        .text_color(c.text_muted)
                        .child("No active tasks. Finished work is kept in history."),
                )
                .into_any_element(),
            DirEntry::ShowMore { shown, finished } => {
                let next = (finished - shown).min(SUBAGENT_HISTORY_PAGE);
                pad(div())
                    .py(px(8.0))
                    .child(
                        Btn::new("subagents-show-more")
                            .icon("chevron-down")
                            .label(format!(
                                "Show {next} more finished tasks ({shown} of {finished} shown)"
                            ))
                            .aria(format!(
                                "Show more finished subagent tasks, {shown} of {finished} shown"
                            ))
                            .on_click(move |_, cx| {
                                this.update(cx, |w, cx| {
                                    w.dispatch(Command::ShowMoreSubagentHistory, cx)
                                });
                            }),
                    )
                    .into_any_element()
            }
            DirEntry::Notice(text) => pad(div())
                .pb(px(8.0))
                .child(
                    div()
                        .id("subagents-bound-notice")
                        .role(Role::Status)
                        .p(px(12.0))
                        .text_size(t.small_size())
                        .text_color(c.warning)
                        .child(text),
                )
                .into_any_element(),
            DirEntry::Task(row) => {
                rows.mounted_tasks.borrow_mut().push(row.id);
                self.render_task_row(row, cx)
            }
        }
    }

    fn render_task_row(&self, row: TaskRow, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = cx.theme().clone();
        let c = &t.colors;
        let id = row.id;
        let click = cx.entity();
        let key = cx.entity();
        div()
            .px(px(8.0))
            .child(
                menu_row(("subagent", id as usize), false, cx)
                    .role(Role::Button)
                    .aria_label(format!(
                        "{}, {}. Open activity. {}",
                        row.title,
                        row.status.label(),
                        row.task
                    ))
                    .flex_col()
                    .items_start()
                    .gap(px(8.0))
                    .py(px(12.0))
                    .w_full()
                    .min_w_0()
                    .on_click(move |_, _, cx| {
                        click.update(cx, |w, cx| {
                            w.dispatch(Command::SelectSubagent(Some(id)), cx);
                        });
                    })
                    .on_key_down(move |event, _, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            key.update(cx, |w, cx| {
                                w.dispatch(Command::SelectSubagent(Some(id)), cx);
                            });
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
                                    .flex_1()
                                    .min_w_0()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(row.title.clone()),
                            )
                            .child(icon("chevron-right", px(14.0), c.text_muted)),
                    )
                    .child(self.task_status(row.status, cx))
                    .when(!row.summary.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(t.small_size())
                                .text_color(c.text_muted)
                                .min_w_0()
                                .child(row.summary.clone()),
                        )
                    })
                    .child(separator(cx)),
            )
            .into_any_element()
    }

    /// Row 0: task excerpt, error and the older-activity control; then one row per preview.
    fn render_activity_entry(&mut self, ix: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let rows = &self.subagent_rows;
        rows.mounted.set(rows.mounted.get() + 1);
        let t = cx.theme().clone();
        let c = &t.colors;
        let model = self.model.read(cx);
        let Some(state) = model.state.current().map(|v| &v.subagents) else {
            return div().into_any_element();
        };
        if ix > 0 {
            let Some(entry) = state.previews.get(ix - 1).cloned() else {
                return div().into_any_element();
            };
            let tool = entry.speaker == "Tool";
            return div()
                .px(px(16.0))
                .pb(px(16.0))
                .child(
                    div()
                        .id(("subagent-entry", entry.id.0 as usize))
                        .role(Role::ListItem)
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .child(
                            div()
                                .text_size(t.small_size())
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(c.text_muted)
                                .child(entry.speaker)
                                .when(entry.native_only, |d| d.child(" · Native-only history")),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .text_size(t.small_size())
                                .when(tool, |d| d.font_family(t.mono_font()))
                                .child(gpui::SharedString::from(entry.text.clone())),
                        )
                        .when(entry.clipped, |d| {
                            d.child(
                                div()
                                    .text_size(t.small_size())
                                    .text_color(c.text_muted)
                                    .child("Preview shortened"),
                            )
                        }),
                )
                .into_any_element();
        }
        let mut header = div().p(px(16.0)).flex().flex_col().gap(px(16.0)).min_w_0();
        if let Some(row) = &rows.selected_row {
            header = header.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .text_size(t.small_size())
                    .child(div().text_color(c.text_muted).child("Task excerpt"))
                    .child(row.task.clone()),
            );
        }
        if let Some(error) = &state.error {
            header = header.child(
                div()
                    .id("subagent-error")
                    .role(Role::Alert)
                    .text_color(c.warning)
                    .child(error.clone()),
            );
        }
        if state.paged && !state.previews.is_empty() {
            if state.loading_older.is_some() {
                header = header.child(
                    div()
                        .id("subagent-older-loading")
                        .role(Role::Status)
                        .text_size(t.small_size())
                        .text_color(c.text_muted)
                        .child("Loading earlier activity…"),
                );
            } else if state.has_older {
                let this = cx.entity();
                header = header.child(
                    Btn::new("subagent-load-older")
                        .icon("chevron-up")
                        .label("Load earlier activity")
                        .aria("Load earlier subagent activity")
                        .on_click(move |_, cx| {
                            this.update(cx, |w, cx| w.dispatch(Command::LoadOlderSubagent, cx));
                        }),
                );
            } else {
                header = header.child(
                    div()
                        .id("subagent-activity-start")
                        .role(Role::Status)
                        .text_size(t.small_size())
                        .text_color(c.text_muted)
                        .child("Beginning of this task’s activity"),
                );
            }
            if let Some(error) = &state.older_error {
                header = header.child(
                    div()
                        .id("subagent-older-error")
                        .role(Role::Alert)
                        .text_size(t.small_size())
                        .text_color(c.warning)
                        .child(error.clone()),
                );
            }
        }
        if state.previews.is_empty() {
            header = header.child(
                div()
                    .id("subagent-empty-activity")
                    .role(Role::Status)
                    .text_color(c.text_muted)
                    .child(if state.loading {
                        "Reading this task’s activity…"
                    } else {
                        "No transcript entries available yet."
                    }),
            );
        }
        header.into_any_element()
    }

    pub(super) fn render_subagent_panel(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        self.subagent_rows.mounted.set(0);
        self.subagent_rows.mounted_tasks.borrow_mut().clear();
        let t = cx.theme().clone();
        let c = &t.colors;
        let model = self.model.read(cx);
        let state = model.state.current().map(|v| &v.subagents);
        let this = cx.entity();
        let mut panel = div()
            .id("subagents-inspector")
            .role(Role::Region)
            .aria_label("Subagent tasks")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0();
        let Some(state) = state else {
            return panel
                .child(
                    div()
                        .p(px(20.0))
                        .text_color(c.text_muted)
                        .child("Open a conversation to follow its subagents."),
                )
                .into_any_element();
        };
        if state.enabled.is_none() {
            return panel
                .child(div().p(px(20.0)).text_color(c.text_muted).child(
                    state.error.clone().unwrap_or_else(|| {
                        "This engine does not provide subagent activity.".into()
                    }),
                ))
                .into_any_element();
        }
        let summary = if self.subagent_rows.attention > 0 {
            format!(
                "{} active · {} completed · {} stopped or failed",
                self.subagent_rows.active,
                self.subagent_rows.completed,
                self.subagent_rows.attention
            )
        } else {
            format!(
                "{} active · {} completed",
                self.subagent_rows.active, self.subagent_rows.completed
            )
        };
        panel = panel.child(
            div()
                .id("subagents-summary")
                .role(Role::Status)
                .aria_label(summary.clone())
                .flex_none()
                .px(px(16.0))
                .py(px(12.0))
                .border_b_1()
                .border_color(c.border)
                .text_size(t.small_size())
                .text_color(c.text_muted)
                .child(summary),
        );
        if let Some(selected) = state.selected {
            // Selection remains engine-owned. A filtered/bounded directory must not replace it.
            let row = self.subagent_rows.selected_row.as_ref();
            let title = row.map_or_else(|| format!("Subagent {selected}"), |row| row.title.clone());
            let mut header = div()
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(10.0))
                .p(px(16.0))
                .border_b_1()
                .border_color(c.border)
                .child(
                    Btn::new("subagent-return")
                        .icon("arrow-left")
                        .label("All tasks")
                        .aria("Back to all subagent tasks")
                        .on_click({
                            let this = this.clone();
                            move |_, cx| {
                                this.update(cx, |w, cx| {
                                    w.dispatch(Command::SelectSubagent(None), cx)
                                });
                            }
                        }),
                )
                .child(
                    div()
                        .id("subagent-task-title")
                        .role(Role::Heading)
                        .aria_label(title.clone())
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(title),
                );
            if let Some(row) = row {
                header = header.child(self.task_status(row.status, cx));
            }
            header = header.child(
                div()
                    .id("subagent-view-state")
                    .role(Role::Status)
                    .text_size(t.small_size())
                    .text_color(c.text_muted)
                    .child(view_state(
                        state.loading,
                        state.live,
                        state.error.is_some(),
                        state.viewed == Some(selected),
                        !state.previews.is_empty(),
                    )),
            );
            panel = panel.child(header).child(
                div()
                    .id("subagent-transcript")
                    .role(Role::List)
                    .aria_label("Read-only subagent activity")
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(
                        list(
                            self.subagent_rows.activity.clone(),
                            cx.processor(|this, ix: usize, _window, cx| {
                                this.render_activity_entry(ix, cx)
                            }),
                        )
                        .size_full(),
                    ),
            );
        } else {
            if let Some(error) = &state.error {
                panel = panel.child(
                    div()
                        .id("subagent-directory-error")
                        .role(Role::Alert)
                        .p(px(16.0))
                        .text_color(c.warning)
                        .child(error.clone()),
                );
            }
            panel = panel.child(
                div()
                    .id("subagent-directory")
                    .role(Role::List)
                    .aria_label("Subagent tasks; select a task to inspect activity")
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(
                        list(
                            self.subagent_rows.directory.clone(),
                            cx.processor(|this, ix: usize, _window, cx| {
                                this.render_directory_entry(ix, cx)
                            }),
                        )
                        .size_full(),
                    ),
            );
        }
        panel
            .child(
                div()
                    .id("subagents-read-only")
                    .flex_none()
                    .px(px(16.0))
                    .py(px(12.0))
                    .border_t_1()
                    .border_color(c.border)
                    .text_size(t.small_size())
                    .text_color(c.text_muted)
                    .child(if state.selected.is_some() && state.paged {
                        "Read-only activity · earlier pages load on request. Messages go to the main conversation."
                    } else if state.selected.is_some() {
                        "Recent activity preview · read only. Messages go to the main conversation."
                    } else if state.enabled == Some(false) {
                        "New calls off in Preferences. Existing tasks continue."
                    } else {
                        "Select a task to follow its activity. New calls allowed."
                    }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests;
