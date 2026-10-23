//! Independent, bounded orchestration observation. No framework or transport I/O.
use crate::{ConversationId, TranscriptItem};
pub use pipkin_engine_protocol::{
    ActivityCounts, ActivityCursor, ActivityFilter, ActivityGroup, ActivityPage, ActivityRow,
    CompletionKey, Coverage, ExecutionBackend, RunBrief, RunOutcome, WaitReason, WorkState,
    WorkerKind,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_ROWS: usize = 2000;
pub const MAX_HISTORY: usize = 2000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Freshness {
    Live,
    Updating,
    #[default]
    Cached,
    Reconnecting,
    Unavailable,
}
impl Freshness {
    pub fn label(self) -> &'static str {
        match self {
            Self::Live => "Live",
            Self::Updating => "Updating",
            Self::Cached => "Cached",
            Self::Reconnecting => "Reconnecting",
            Self::Unavailable => "Unavailable",
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub worker: ActivityRow,
    pub conversation: Option<ConversationId>,
    pub root: Option<ConversationId>,
}
impl Row {
    pub fn title(&self) -> &str {
        self.worker
            .title
            .as_deref()
            .or(self.worker.task_excerpt.as_deref())
            .unwrap_or("Untitled conversation")
    }
    pub fn label(&self) -> &str {
        match &self.worker.state {
            WorkState::Pending => "Pending",
            WorkState::Queued => "Queued",
            WorkState::Running => "Running",
            WorkState::Waiting { .. } => "Waiting for input",
            WorkState::Stopping => "Stopping",
            WorkState::Completing => "Completing",
            WorkState::Ready => "Ready for next prompt",
            WorkState::Completed => "Completed",
            WorkState::Failed { .. } => "Failed",
            WorkState::Stopped => "Stopped",
            WorkState::Interrupted => "Interrupted",
            WorkState::Blocked { reason } => reason,
            WorkState::Unknown => "Outcome unknown",
        }
    }
    pub fn finished_child(&self) -> bool {
        self.worker.kind == WorkerKind::Child
            && matches!(
                self.worker.state,
                WorkState::Completed | WorkState::Stopped | WorkState::Failed { .. }
            )
    }
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Detail {
    pub session: String,
    pub generation: u64,
    pub items: Vec<TranscriptItem>,
    pub oldest: Option<u64>,
    pub has_older: bool,
    pub loading: bool,
    pub error: Option<String>,
    pub output: Option<(String, Result<String, String>)>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    Send,
    Queue,
    Steer,
    Stop,
    FollowUp,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Open,
    CreateNative {
        project: String,
        model: String,
    },
    DismissCreation,
    Close,
    Refresh,
    Group(Option<ActivityGroup>),
    Filter(ActivityFilter),
    Recency(Recency),
    HideProject(String),
    ShowProject(String),
    HideThread(String),
    ShowThread(String),
    ShowAllHidden,
    Select(String),
    MarkReviewed {
        session: String,
        generation: u64,
        completion: CompletionKey,
    },
    More,
    Older,
    Edit(String),
    /// Explicitly load live requests for this root without changing normal selection.
    Attach {
        session: String,
        generation: u64,
        revision: u64,
    },
    Act {
        session: String,
        generation: u64,
        revision: u64,
        draft_revision: u64,
        action: Action,
    },
    /// Answer a known root-session question without changing the conversation selection.
    Answer {
        session: String,
        generation: u64,
        revision: u64,
        request: String,
        answer: crate::UiAnswer,
    },
    Reconcile,
    Back,
    ToolOutput(String),
}
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    Watch {
        generation: u64,
        closed: Vec<ConversationId>,
        filter: ActivityFilter,
        group: Option<ActivityGroup>,
    },
    Close {
        generation: u64,
    },
    More {
        generation: u64,
        filter: ActivityFilter,
        group: ActivityGroup,
        after: ActivityCursor,
    },
    Detail {
        generation: u64,
        session: String,
        before: Option<u64>,
    },
    Control {
        generation: u64,
        session: String,
        root: String,
        child: Option<u64>,
        run: Option<u64>,
        request: String,
        text: String,
        action: Action,
        lookup: bool,
    },
    ToolOutput {
        generation: u64,
        session: String,
        call_id: String,
    },
    Attach {
        conversation: ConversationId,
        generation: u64,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Available(bool),
    Counts(ActivityCounts),
    ToolOutput {
        generation: u64,
        session: String,
        call_id: String,
        result: Result<String, String>,
    },
    Persisted {
        request: String,
        result: Result<(), String>,
    },
    Directory {
        generation: u64,
        rows: Vec<Row>,
        page: ActivityPage,
        append: bool,
    },
    Detail {
        generation: u64,
        session: String,
        before: Option<u64>,
        items: Vec<TranscriptItem>,
        oldest: Option<u64>,
        has_older: bool,
    },
    Invalidated {
        session: String,
        revision: u64,
    },
    Failed {
        generation: u64,
        session: Option<String>,
        message: String,
    },
    Control {
        generation: u64,
        request: String,
        result: Result<Option<String>, String>,
        uncertain: bool,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingControl {
    pub generation: u64,
    pub session: String,
    pub root: String,
    pub child: Option<u64>,
    pub run: Option<u64>,
    pub request: String,
    pub text: String,
    pub action: Action,
    pub uncertain: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Creation {
    Pending(crate::RequestId),
    Failed(String),
    Created(ConversationId),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Recency {
    #[default]
    Month,
    TwoMonths,
    ThreeMonths,
}
impl Recency {
    pub fn days(self) -> u32 {
        match self {
            Self::Month => 30,
            Self::TwoMonths => 60,
            Self::ThreeMonths => 90,
        }
    }
    pub fn next(self) -> Self {
        match self {
            Self::Month => Self::TwoMonths,
            Self::TwoMonths => Self::ThreeMonths,
            Self::ThreeMonths => Self::Month,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HiddenThread {
    pub title: String,
    pub project: String,
}

#[derive(Clone, Debug, Default)]
pub struct State {
    /// A dashboard-local creation never replaces the main conversation.
    pub creation: Option<Creation>,
    pub recency: Recency,
    pub hidden_projects: std::collections::BTreeSet<String>,
    pub hidden_threads: BTreeMap<String, HiddenThread>,
    pub open: bool,
    pub available: bool,
    pub generation: u64,
    pub filter: ActivityFilter,
    pub group: Option<ActivityGroup>,
    pub rows: Vec<Row>,
    pub counts: Option<ActivityCounts>,
    pub global_counts: Option<ActivityCounts>,
    pub coverage: Option<Coverage>,
    pub through: u64,
    pub next: Option<ActivityCursor>,
    pub selected: Option<String>,
    pub detail: Detail,
    pub freshness: Freshness,
    pub loading: bool,
    pub error: Option<String>,
    pub control: Option<PendingControl>,
    pub child_drafts: BTreeMap<String, (String, u64)>,
    pub seen: BTreeMap<String, u64>,
    pub reviewed_completions: BTreeMap<String, CompletionKey>,
    pub pending_send: Option<(ConversationId, u64, Action)>,
    pub attachment: Option<(ConversationId, u64)>,
    pub attachment_error: Option<String>,
}
impl State {
    pub fn completion_unread(&self, row: &Row) -> bool {
        row.worker.completion_unread(&self.reviewed_completions)
    }
    pub fn selected_row(&self) -> Option<&Row> {
        self.selected
            .as_ref()
            .and_then(|s| self.rows.iter().find(|r| &r.worker.session == s))
    }
    pub fn attention(&self) -> u32 {
        self.counts.map_or(0, |c| c.attention)
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    recency: Recency,
    #[serde(default)]
    hidden_projects: std::collections::BTreeSet<String>,
    #[serde(default)]
    hidden_threads: BTreeMap<String, HiddenThread>,
    filter: ActivityFilter,
    group: Option<ActivityGroup>,
    selected: Option<String>,
    child_drafts: BTreeMap<String, (String, u64)>,
    seen: BTreeMap<String, u64>,
    #[serde(default)]
    reviewed_completions: BTreeMap<String, CompletionKey>,
    control: Option<PendingControl>,
}
impl State {
    pub fn saved(&self) -> String {
        serde_json::to_string(&Saved {
            recency: self.recency,
            hidden_projects: self.hidden_projects.clone(),
            hidden_threads: self.hidden_threads.clone(),
            filter: self.filter.clone(),
            group: self.group,
            selected: self.selected.clone(),
            child_drafts: self.child_drafts.clone(),
            seen: self.seen.clone(),
            reviewed_completions: self.reviewed_completions.clone(),
            control: self.control.clone(),
        })
        .unwrap_or_default()
    }
    pub fn restore(saved: &str) -> Self {
        let Ok(saved) = serde_json::from_str::<Saved>(saved) else {
            return Self::default();
        };
        Self {
            recency: saved.recency,
            hidden_projects: saved.hidden_projects,
            hidden_threads: saved.hidden_threads,
            filter: saved.filter,
            group: saved.group,
            selected: saved.selected,
            child_drafts: saved.child_drafts,
            seen: saved.seen,
            reviewed_completions: saved.reviewed_completions,
            control: saved.control.map(|mut p| {
                p.uncertain = true;
                p
            }),
            ..Default::default()
        }
    }
}
