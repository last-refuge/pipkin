//! The Jobs inspector: the engine's background jobs for the open session, projected from
//! core state into a virtualized list. Active jobs first; finished ones are counted and
//! listed on request. One job can be expanded to its command, directory, outcome and a
//! bounded log tail fetched through the engine, with Stop for a running job. Links point at
//! the plan task and the tool call that started it. Nothing here starts a job.

use std::cell::Cell;
use std::hash::{DefaultHasher, Hash, Hasher};

use gpui::{
    Context, IntoElement, ListAlignment, ListState, ParentElement, Role, Styled, div, list,
    prelude::*, px,
};
use pipkin_core::{Command, ConversationId, Job, JobOrigin, JobState, JobsState};

use super::subagents::sync_list;
use super::{controls::*, workspace::Workspace};
use crate::theme::ActiveTheme;

const OVERDRAW: f32 = 600.0;

#[derive(Clone, Debug, PartialEq, Hash)]
enum JobEntry {
    Empty,
    Job {
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

impl JobEntry {
    fn identity(&self) -> u64 {
        let mut h = DefaultHasher::new();
        match self {
            JobEntry::Empty => 0u8.hash(&mut h),
            JobEntry::Job { id, .. } => (1u8, id).hash(&mut h),
            JobEntry::HistoryToggle { .. } => 2u8.hash(&mut h),
            JobEntry::Notice(_) => 3u8.hash(&mut h),
        }
        h.finish()
    }

    fn content(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.hash(&mut h);
        h.finish()
    }
}

fn job_content(job: &Job, log: Option<u64>) -> u64 {
    let mut h = DefaultHasher::new();
    (
        job.state,
        job.exit_code,
        job.log_bytes,
        job.log_truncated,
        &job.reason,
        &job.delivery,
        job.ended_at_ms,
        log,
    )
        .hash(&mut h);
    h.finish()
}

fn state_icon(state: JobState) -> &'static str {
    match state {
        JobState::Succeeded => "check",
        JobState::Starting | JobState::Running | JobState::Cancelling => "loader-circle",
        JobState::Failed | JobState::TimedOut => "circle-alert",
        JobState::Cancelled => "square",
        JobState::Interrupted | JobState::Unknown => "triangle-alert",
    }
}

pub(super) fn elapsed_text(ms: i64) -> String {
    let secs = ms / 1000;
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub(super) struct JobRows {
    key: Option<(ConversationId, u64)>,
    entries: Vec<JobEntry>,
    entry_keys: Vec<(u64, u64)>,
    pub active: usize,
    pub finished: usize,
    pub unseen: usize,
    pub(super) list: ListState,
    pub(super) mounted: Cell<usize>,
}

impl Default for JobRows {
    fn default() -> Self {
        JobRows {
            key: None,
            entries: Vec::new(),
            entry_keys: Vec::new(),
            active: 0,
            finished: 0,
            unseen: 0,
            list: ListState::new(0, ListAlignment::Top, px(OVERDRAW)),
            mounted: Cell::new(0),
        }
    }
}

impl JobRows {
    pub fn tab_label(&self) -> String {
        match (self.active, self.unseen) {
            (0, 0) => "Jobs".into(),
            (a, 0) => format!("Jobs {a}"),
            (0, u) => format!("Jobs ·{u}"),
            (a, u) => format!("Jobs {a} ·{u}"),
        }
    }

    #[cfg(test)]
    fn listed(&self) -> Vec<(u64, bool)> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                JobEntry::Job { id, expanded, .. } => Some((*id, *expanded)),
                _ => None,
            })
            .collect()
    }

    pub fn refresh(&mut self, key: Option<(ConversationId, u64)>, jobs: Option<&JobsState>) {
        let empty = JobsState::default();
        let jobs = jobs.unwrap_or(&empty);
        let scope_changed = self.key != key;
        self.key = key;
        self.active = jobs.active;
        self.finished = jobs.finished();
        self.unseen = jobs.unseen;
        let log_mark = jobs.log.as_ref().map(|l| {
            let mut h = DefaultHasher::new();
            (l.id, l.offset, l.next_offset, &l.text).hash(&mut h);
            h.finish()
        });
        let mut entries = Vec::new();
        if jobs.jobs.is_empty() {
            entries.push(JobEntry::Empty);
        }
        for job in jobs.jobs.iter().filter(|j| j.state.active()) {
            let expanded = jobs.expanded == Some(job.id);
            entries.push(JobEntry::Job {
                id: job.id,
                expanded,
                content: job_content(job, expanded.then_some(log_mark).flatten()),
            });
        }
        if self.finished > 0 {
            entries.push(JobEntry::HistoryToggle {
                open: jobs.history_open,
                finished: self.finished,
            });
            if jobs.history_open {
                for job in jobs.jobs.iter().filter(|j| !j.state.active()) {
                    let expanded = jobs.expanded == Some(job.id);
                    entries.push(JobEntry::Job {
                        id: job.id,
                        expanded,
                        content: job_content(job, expanded.then_some(log_mark).flatten()),
                    });
                }
                if jobs.truncated {
                    entries.push(JobEntry::Notice(
                        "Older finished jobs are not listed here.".into(),
                    ));
                }
            }
        }
        if let Some(error) = &jobs.error {
            entries.push(JobEntry::Notice(error.clone()));
        }
        let keys: Vec<(u64, u64)> = entries
            .iter()
            .map(|e| (e.identity(), e.content()))
            .collect();
        if keys == self.entry_keys && !scope_changed {
            return;
        }
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
    pub(super) fn refresh_job_rows(&mut self, cx: &gpui::App) {
        let model = self.model.read(cx);
        let current = model.state.current();
        self.job_rows.refresh(
            current.map(|c| (c.id, c.generation)),
            current.map(|c| &c.jobs),
        );
    }

    fn render_job_entry(&mut self, ix: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(entry) = self.job_rows.entries.get(ix).cloned() else {
            return div().into_any_element();
        };
        self.job_rows.mounted.set(self.job_rows.mounted.get() + 1);
        let t = cx.theme().clone();
        let c = t.colors.clone();
        let this = cx.entity();
        match entry {
            JobEntry::Empty => div()
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
                                .child("No background jobs"),
                        )
                        .child(div().text_size(t.small_size()).text_color(c.text_muted).child(
                            "When Pipkin starts a build, test run or server in the background, it appears here with its state and log.",
                        )),
                )
                .into_any_element(),
            JobEntry::HistoryToggle { open, finished } => div()
                .px(px(8.0))
                .pt(px(8.0))
                .child(
                    Btn::new("jobs-finished-toggle")
                        .icon(if open { "chevron-down" } else { "chevron-right" })
                        .label(format!(
                            "{} finished jobs ({finished})",
                            if open { "Hide" } else { "Show" }
                        ))
                        .aria(format!(
                            "{} finished jobs, {finished}",
                            if open { "Hide" } else { "Show" }
                        ))
                        .on_click(move |_, cx| {
                            this.update(cx, |w, cx| w.dispatch(Command::ToggleJobHistory, cx));
                        }),
                )
                .into_any_element(),
            JobEntry::Notice(text) => div()
                .px(px(16.0))
                .py(px(12.0))
                .text_size(t.small_size())
                .text_color(c.warning)
                .child(text)
                .into_any_element(),
            JobEntry::Job { id, expanded, .. } => {
                let model = self.model.read(cx);
                let Some(current) = model.state.current() else {
                    return div().into_any_element();
                };
                let Some(job) = current.jobs.job(id).cloned() else {
                    return div().into_any_element();
                };
                let log = current.jobs.log.clone().filter(|l| l.id == id);
                let pending = current.jobs.pending_log == Some(id);
                let real = model.state.mode == pipkin_core::Mode::Real;
                let now = now_ms();
                let (fg, bg) = match job.state {
                    JobState::Succeeded => (c.success, c.bg_active),
                    JobState::Starting | JobState::Running | JobState::Cancelling => {
                        (c.warning, c.bg_active)
                    }
                    JobState::Failed | JobState::TimedOut => (c.danger, c.danger_bg),
                    _ => (c.text_muted, c.bg_active),
                };
                let mut header = div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.0))
                    .child(icon(state_icon(job.state), px(14.0 * t.scale.max(1.0)), fg))
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .font_family(t.mono_font())
                            .text_size(t.small_size())
                            .child(format!("#{} {}", job.id, job.label())),
                    )
                    .child(chip(job.state.label(), fg, bg, cx))
                    .child(chip(
                        match job.origin {
                            JobOrigin::Model => "started by Pipkin",
                            JobOrigin::User => "started by you",
                        },
                        c.text_muted,
                        c.bg_active,
                        cx,
                    ));
                if let Some(ms) = job.elapsed_ms(now) {
                    header = header.child(chip(elapsed_text(ms), c.text_muted, c.bg_active, cx));
                }
                if let Some(code) = job.exit_code
                    && code != 0
                {
                    header = header.child(chip(format!("exit {code}"), c.danger, c.danger_bg, cx));
                }
                if job.wake {
                    header = header.child(chip(
                        match job.delivery.as_str() {
                            "woken" => "woke Pipkin",
                            "suppressed" => "wake suppressed",
                            _ => "wakes Pipkin",
                        },
                        c.text_faint,
                        c.bg_active,
                        cx,
                    ));
                }
                if let Some(task) = job.task {
                    header = header.child(chip(format!("plan task #{task}"), c.accent, c.bg_active, cx));
                }
                let toggle = this.clone();
                let mut body = div()
                    .id(("job-row", id as usize))
                    .role(Role::ListItem)
                    .aria_label(format!("Job {} {}, {}", job.id, job.label(), job.state.label()))
                    .px(px(16.0))
                    .py(px(10.0))
                    .border_b_1()
                    .border_color(c.border)
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        toggle.update(cx, |w, cx| w.dispatch(Command::ToggleJob(id), cx));
                    })
                    .child(header);
                if !expanded {
                    return body.into_any_element();
                }
                let mut detail = div().flex().flex_col().gap(px(6.0)).mt(px(8.0));
                detail = detail.child(
                    div()
                        .text_size(t.small_size())
                        .font_family(t.mono_font())
                        .text_color(c.text)
                        .child(job.command.clone()),
                );
                detail = detail.child(
                    div()
                        .text_size(t.small_size())
                        .text_color(c.text_muted)
                        .child(format!("in {}", job.cwd)),
                );
                if let Some(reason) = &job.reason {
                    detail = detail.child(
                        div()
                            .text_size(t.small_size())
                            .text_color(if job.state == JobState::Succeeded { c.text_muted } else { c.danger })
                            .child(reason.clone()),
                    );
                }
                if let Some(call) = &job.call_id {
                    detail = detail.child(
                        div()
                            .text_size(t.small_size())
                            .text_color(c.text_faint)
                            .child(format!("tool call {}", call.chars().take(16).collect::<String>())),
                    );
                }
                let log_title = match &log {
                    Some(l) => format!(
                        "Log tail: bytes {}–{} of {}{}",
                        l.offset,
                        l.next_offset,
                        l.total_bytes,
                        if l.truncated { " (truncated at the quota)" } else { "" }
                    ),
                    None if pending => "Loading the log…".into(),
                    None => format!("Log: {} bytes", job.log_bytes),
                };
                detail = detail.child(
                    div()
                        .text_size(t.small_size())
                        .text_color(c.text_muted)
                        .child(log_title),
                );
                if let Some(l) = &log
                    && !l.text.is_empty()
                {
                    detail = detail.child(
                        div()
                            .id(("job-log", id as usize))
                            .max_h(px(220.0 * t.scale))
                            .overflow_y_scroll()
                            .p(px(8.0))
                            .rounded(px(6.0))
                            .bg(c.code_bg)
                            .font_family(t.mono_font())
                            .text_size(t.small_size())
                            .text_color(c.text)
                            .child(l.text.clone()),
                    );
                }
                let mut actions = div().flex().flex_wrap().gap(px(4.0)).mt(px(4.0));
                if job.state.active() {
                    let stop = this.clone();
                    actions = actions.child(
                        Btn::new(("job-stop", id as usize))
                            .label("Stop")
                            .kind(BtnKind::Subtle)
                            .compact()
                            .disabled(!real || job.state == JobState::Cancelling)
                            .aria(format!("Stop job {}", job.id))
                            .on_click(move |_, cx| {
                                stop.update(cx, |w, cx| w.dispatch(Command::CancelJob(id), cx));
                            }),
                    );
                }
                let refresh = this.clone();
                actions = actions.child(
                    Btn::new(("job-refresh", id as usize))
                        .label("Refresh log")
                        .kind(BtnKind::Ghost)
                        .compact()
                        .disabled(!real || pending)
                        .on_click(move |_, cx| {
                            refresh.update(cx, |w, cx| w.dispatch(Command::RefreshJobLog(id), cx));
                        }),
                );
                body = body.child(detail.child(actions));
                body.into_any_element()
            }
        }
    }

    pub(super) fn render_jobs_panel(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        self.job_rows.mounted.set(0);
        let t = cx.theme().clone();
        let c = &t.colors;
        let model = self.model.read(cx);
        let jobs = model.state.current().map(|v| &v.jobs);
        let panel = div()
            .id("jobs-inspector")
            .role(Role::Region)
            .aria_label("Jobs")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0();
        let Some(jobs) = jobs else {
            return panel
                .child(
                    div()
                        .p(px(20.0))
                        .text_color(c.text_muted)
                        .child("Open a conversation to see its jobs."),
                )
                .into_any_element();
        };
        if !jobs.available {
            return panel
                .child(
                    div()
                        .p(px(20.0))
                        .text_color(c.text_muted)
                        .child("This engine does not run background jobs."),
                )
                .into_any_element();
        }
        let summary = format!("{} active · {} finished", jobs.active, jobs.finished());
        panel
            .child(
                div()
                    .id("jobs-summary")
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
            )
            .child(
                list(
                    self.job_rows.list.clone(),
                    cx.processor(|this, ix: usize, _window, cx| this.render_job_entry(ix, cx)),
                )
                .size_full(),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipkin_core::{JobsState, JobsSync};

    fn job(id: u64, state: JobState) -> Job {
        Job {
            id,
            name: None,
            command: format!("cmd {id}"),
            cwd: "/p".into(),
            origin: JobOrigin::Model,
            state,
            exit_code: None,
            reason: None,
            call_id: None,
            task: None,
            wake: false,
            delivery: "pending".into(),
            log_bytes: 0,
            log_truncated: false,
            created_at_ms: 0,
            started_at_ms: Some(0),
            ended_at_ms: None,
        }
    }

    #[test]
    fn rows_list_active_first_and_finished_on_request() {
        let mut state = JobsState::default();
        state.sync(JobsSync {
            jobs: vec![job(2, JobState::Running), job(1, JobState::Succeeded)],
            active: 1,
            truncated: false,
        });
        let mut rows = JobRows::default();
        rows.refresh(Some((ConversationId(1), 1)), Some(&state));
        assert_eq!(rows.listed(), [(2, false)]);
        assert_eq!(rows.tab_label(), "Jobs 1");
        state.history_open = true;
        state.expanded = Some(1);
        rows.refresh(Some((ConversationId(1), 1)), Some(&state));
        assert_eq!(rows.listed(), [(2, false), (1, true)]);
        state.sync(JobsSync {
            jobs: vec![job(2, JobState::Failed), job(1, JobState::Succeeded)],
            active: 0,
            truncated: true,
        });
        rows.refresh(Some((ConversationId(1), 1)), Some(&state));
        assert_eq!(rows.tab_label(), "Jobs ·1");
        assert!(
            rows.entries
                .iter()
                .any(|e| matches!(e, JobEntry::Notice(_)))
        );
        assert_eq!(elapsed_text(65_000), "1m 5s");
    }
}
