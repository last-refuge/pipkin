use crate::ids::*;

#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub note: String,
}

/// Committed engine usage for one model or tool bucket. Cost is absent when the engine did not
/// report one; zero is a real reported value, not an unknown sentinel.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageAmount {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cache_write_1h: Option<u64>,
    pub reasoning: Option<u64>,
    pub total_tokens: u64,
    pub cost_usd: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionUsage {
    pub session_id: String,
    pub models: Vec<(String, UsageAmount)>,
    pub tools: Vec<(String, UsageAmount)>,
}

impl SessionUsage {
    pub fn total(&self) -> UsageAmount {
        let mut total = UsageAmount {
            cost_usd: Some(0.0),
            ..UsageAmount::default()
        };
        for (_, amount) in self.models.iter().chain(&self.tools) {
            total.input = total.input.saturating_add(amount.input);
            total.output = total.output.saturating_add(amount.output);
            total.cache_read = total.cache_read.saturating_add(amount.cache_read);
            total.cache_write = total.cache_write.saturating_add(amount.cache_write);
            if let Some(value) = amount.cache_write_1h {
                total.cache_write_1h =
                    Some(total.cache_write_1h.unwrap_or(0).saturating_add(value));
            }
            if let Some(value) = amount.reasoning {
                total.reasoning = Some(total.reasoning.unwrap_or(0).saturating_add(value));
            }
            total.total_tokens = total.total_tokens.saturating_add(amount.total_tokens);
            total.cost_usd = total.cost_usd.zip(amount.cost_usd).map(|(a, b)| a + b);
        }
        total
    }
}

/// A durable child of a subagent tool call in one Pi session, never a top-level conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubagentInfo {
    pub id: u64,
    pub task_id: u64,
    pub call_id: String,
    pub task: String,
    pub status: String,
    /// The engine-reported launch profile, shown as given.
    pub profile: Option<String>,
}

/// Bounded, plain-text child entry prepared outside GPUI rendering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubagentPreview {
    pub native_only: bool,
    pub id: ItemId,
    /// Changes whenever this preview is prepared again, so a view can remeasure only what
    /// changed. Unique within one `SubagentState`.
    pub rev: u64,
    pub speaker: &'static str,
    /// Shared so retained previews are not copied on each page or live update.
    pub text: std::sync::Arc<str>,
    pub clipped: bool,
}

/// Pi is authoritative for this session's setting and child directory. `None` means the
/// attached engine has no subagent service; it is not an empty child list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SubagentState {
    pub enabled: Option<bool>,
    pub children: Vec<SubagentInfo>,
    pub selected: Option<u64>,
    pub items: Vec<TranscriptItem>,
    pub previews: Vec<SubagentPreview>,
    pub error: Option<String>,
    pub live: bool,
    /// One in-flight read per selected child; cleared on detach and failure.
    pub loading: bool,
    /// Avoid remounting a successfully read but empty child on each activity update.
    pub viewed: Option<u64>,
    /// The engine pages this directory and child history (native engine). Pi's directory and
    /// bounded previews are unpaged; this stays false for them.
    pub paged: bool,
    /// The engine cut the published directory at its bound; later children are not listed.
    pub truncated: bool,
    /// Finished (history) rows revealed in the directory; 0 means history is collapsed.
    pub history: usize,
    /// The selected child has entries older than `items` (paged engines only).
    pub has_older: bool,
    /// Cursor of the one in-flight older-activity page: its answer must name it.
    pub loading_older: Option<ItemId>,
    pub older_error: Option<String>,
    /// Source of `SubagentPreview::rev`.
    pub preview_rev: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Attachment {
    pub path: String,
    pub name: String,
    pub size: Option<u64>,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    Sent,
    /// Submitted, waiting for acceptance.
    Pending,
    /// Acknowledgment lost; the engine may or may not have the prompt.
    Unknown,
    Rejected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolStatus {
    Running,
    Ok,
    Failed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub call_ref: Option<(OperationId, u32)>,
    /// The engine's id for the call, which names its complete result.
    pub call_id: Option<String>,
    pub name: String,
    pub input: String,
    pub output: String,
    /// Output was cut to the preview bound; `full_len` is the original size in bytes.
    pub truncated: bool,
    pub full_len: usize,
    pub status: ToolStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeLevel {
    Info,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ItemKind {
    User {
        text: String,
        attachments: Vec<Attachment>,
        delivery: Delivery,
        steer: bool,
    },
    Assistant {
        text: String,
        streaming: bool,
    },
    Tool(ToolCall),
    Notice {
        text: String,
        level: NoticeLevel,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct TranscriptItem {
    /// This item contains history that exists only in the built-in engine.
    pub native_only: bool,
    pub id: ItemId,
    /// Unix seconds.
    pub at: i64,
    pub kind: ItemKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffKind {
    Context,
    Add,
    Remove,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileChange {
    pub path: String,
    pub added: u32,
    pub removed: u32,
    pub hunks: Vec<Hunk>,
}

/// Collection status and coverage of a conversation's displayed change evidence.
/// A failed/in-flight collection preserves the last successful result, not a claim of no changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ChangesState {
    #[default]
    Unscanned,
    Loading,
    Ready,
    NotARepository,
    Unavailable(String),
}

/// How queued input joins a run: steering lands at the next boundary inside the run, a
/// follow-up waits for the run to finish.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QueueMode {
    Steer,
    #[default]
    FollowUp,
}

/// Input waiting behind a run. In real mode the engine owns the queue and `id` is its entry
/// id; in the demo the id is local.
#[derive(Clone, Debug, PartialEq)]
pub struct QueuedPrompt {
    pub id: QueueId,
    pub text: String,
    pub mode: QueueMode,
}

/// Whether the engine has confirmed it holds a queue request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueSend {
    /// Sent; no answer yet.
    Sending,
    /// The acknowledgment was lost. Never resent; the engine is asked about its key instead.
    Unknown,
}

/// A steer or follow-up that is journaled and sent but not yet confirmed by the engine. Shown
/// beside the engine's queue so the user's text never disappears between pressing the key and
/// the engine reporting it.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingQueued {
    pub request: RequestId,
    pub text: String,
    pub attachments: Vec<Attachment>,
    pub mode: QueueMode,
    pub state: QueueSend,
}

/// What withdrawing a queued input did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelOutcome {
    Cancelled,
    /// A run already took it.
    AlreadyConsumed,
    NotFound,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RunState {
    Idle,
    /// Submitted; waiting for the engine to accept.
    Submitting {
        op: OperationId,
    },
    Running {
        op: OperationId,
    },
    /// Cancel requested; stays here until the engine confirms settlement.
    Stopping {
        op: OperationId,
    },
    /// Acknowledgment was lost. Never resent automatically.
    OutcomeUnknown {
        op: OperationId,
    },
    Failed {
        message: String,
    },
}

impl RunState {
    pub fn op(&self) -> Option<OperationId> {
        match self {
            RunState::Submitting { op }
            | RunState::Running { op }
            | RunState::Stopping { op }
            | RunState::OutcomeUnknown { op } => Some(*op),
            _ => None,
        }
    }

    pub fn is_busy(&self) -> bool {
        self.op().is_some()
    }

    pub fn label(&self) -> &'static str {
        match self {
            RunState::Idle => "Idle",
            RunState::Submitting { .. } => "Sending",
            RunState::Running { .. } => "Working",
            RunState::Stopping { .. } => "Stopping",
            RunState::OutcomeUnknown { .. } => "Outcome unknown",
            RunState::Failed { .. } => "Failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SaveState {
    Clean,
    Dirty,
    Saving,
    Saved,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    pub text: String,
    pub attachments: Vec<Attachment>,
    pub rev: u64,
    /// Bumped whenever the core (not the editor) changes the text, so the editor resyncs.
    pub sync_epoch: u64,
    pub save: SaveState,
}

impl Default for Draft {
    fn default() -> Self {
        Draft {
            text: String::new(),
            attachments: Vec::new(),
            rev: 0,
            sync_epoch: 0,
            save: SaveState::Clean,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextSize {
    Small,
    Normal,
    Large,
}

impl TextSize {
    pub fn scale(self) -> f32 {
        match self {
            TextSize::Small => 0.92,
            TextSize::Normal => 1.0,
            TextSize::Large => 1.2,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Prefs {
    /// Set only after the user enters a verified, usable first conversation.
    pub setup_completed: bool,
    pub theme: Theme,
    pub text_size: TextSize,
    pub reduced_motion: bool,
    pub nav_width: f32,
    pub inspector_width: f32,
    pub inspector_open: bool,
    /// The window's size when last resized (not maximized or fullscreen), to open at again.
    pub window_size: Option<(f32, f32)>,
    pub selected_project: Option<ProjectId>,
    pub selected_conversation: Option<ConversationId>,
    pub model: Option<String>,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            setup_completed: false,
            theme: Theme::Dark,
            text_size: TextSize::Normal,
            reduced_motion: false,
            nav_width: 240.0,
            inspector_width: 400.0,
            inspector_open: true,
            window_size: None,
            selected_project: None,
            selected_conversation: None,
            model: None,
        }
    }
}

/// Which backend the application was composed with. Real mode must never show simulated data
/// or report simulated success.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Demo,
    Real,
}

/// Engine connection lifecycle, independent of per-conversation run and save states.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Connection {
    Connecting,
    #[default]
    Ready,
    Reconnecting,
    Offline(String),
    Incompatible(String),
    Failed(String),
}

impl Connection {
    pub fn is_ready(&self) -> bool {
        matches!(self, Connection::Ready)
    }

    /// Short user-facing label; `None` when nothing needs saying.
    pub fn banner(&self) -> Option<String> {
        match self {
            Connection::Ready => None,
            Connection::Connecting => Some("Connecting to the built-in engine…".into()),
            Connection::Reconnecting => Some("Connection lost. Reconnecting…".into()),
            Connection::Offline(why) => Some(format!("Offline: {why}")),
            Connection::Incompatible(why) => Some(format!("Incompatible engine: {why}")),
            Connection::Failed(why) => Some(format!("Engine failed: {why}")),
        }
    }
}

/// Where a submission came from, which decides what happens once its intent is durable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntentOrigin {
    /// The composer draft. It is cleared only after the intent is durable.
    Draft,
    /// Explicit retry of a rejected or failed submission.
    Retry,
    /// The next client-side queued prompt (demo only; the engine owns the queue in real mode).
    Queue,
    /// Automatically scheduled turn for the active goal.
    Goal,
    /// Steering input for the active run (real mode).
    Steer,
    /// Input queued after the active run (real mode).
    FollowUp,
}

/// A submission whose intent is being made durable. Nothing is sent and the draft is untouched
/// until the journal acknowledges the commit.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingIntent {
    pub request: RequestId,
    pub origin: IntentOrigin,
    pub text: String,
    pub attachments: Vec<Attachment>,
    pub model: Option<String>,
}

/// What kind of answer a question from an extension takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiRequestKind {
    Select,
    Confirm,
    Input,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UiRequestItem {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

/// A question an extension asked the person, waiting for an answer. The engine owns it; its `id`
/// is what an answer or a cancel names.
#[derive(Clone, Debug, PartialEq)]
pub struct UiRequest {
    pub id: String,
    pub kind: UiRequestKind,
    pub title: String,
    pub message: Option<String>,
    pub items: Vec<UiRequestItem>,
    pub placeholder: Option<String>,
    pub default_value: Option<String>,
    /// Epoch milliseconds after which the engine cancels the question.
    pub deadline: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiNoticeLevel {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UiNotice {
    pub id: String,
    pub level: UiNoticeLevel,
    pub message: String,
}

/// An answer to a question.
#[derive(Clone, Debug, PartialEq)]
pub enum UiAnswer {
    Choice(String),
    Confirm(bool),
    Text(String),
}

/// A message found by searching saved history.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit {
    pub conversation: ConversationId,
    pub item: ItemId,
    /// A short run of text around the match, with the match itself between `\u{2}` and `\u{3}`.
    pub snippet: String,
    pub at: i64,
}

/// The outcome of a search of saved history, with how much it covered.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchResults {
    pub query: String,
    pub hits: Vec<SearchHit>,
    /// Conversations whose saved copy was searched.
    pub conversations_searched: usize,
    /// Messages in those copies.
    pub messages_searched: usize,
    /// The search stopped at its result limit.
    pub truncated: bool,
}

/// What to do with the complete output of a tool call once it arrives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputUse {
    Copy,
    Save,
}

/// Something the application starts outside itself.
#[derive(Clone, Debug, PartialEq)]
pub enum Launch {
    /// Open a file in the person's editor. `root` is the project it must lie inside.
    Editor { path: String, root: String },
    /// Open a terminal in a directory.
    Terminal { cwd: String },
}

/// The most characters of a prompt used as a conversation's name.
pub const TITLE_CHARS: usize = 44;

/// A conversation name from a prompt: its first non-empty line, spaces collapsed, cut to
/// `TITLE_CHARS` characters with an ellipsis when it was longer.
pub fn prompt_title(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= TITLE_CHARS {
        return collapsed;
    }
    let cut: String = collapsed.chars().take(TITLE_CHARS).collect();
    format!("{}\u{2026}", cut.trim_end())
}

// ------------------------------------------------------------------------------- plans

/// The state of one plan task, as the engine reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlanTaskStatus {
    Pending,
    Active,
    Blocked,
    Completed,
    Skipped,
    Cancelled,
}

impl PlanTaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            PlanTaskStatus::Pending => "pending",
            PlanTaskStatus::Active => "active",
            PlanTaskStatus::Blocked => "blocked",
            PlanTaskStatus::Completed => "completed",
            PlanTaskStatus::Skipped => "skipped",
            PlanTaskStatus::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<PlanTaskStatus> {
        Some(match s {
            "pending" => PlanTaskStatus::Pending,
            "active" => PlanTaskStatus::Active,
            "blocked" => PlanTaskStatus::Blocked,
            "completed" => PlanTaskStatus::Completed,
            "skipped" => PlanTaskStatus::Skipped,
            "cancelled" => PlanTaskStatus::Cancelled,
            _ => return None,
        })
    }

    /// Completed, skipped or cancelled: finished one way or another.
    pub fn finished(self) -> bool {
        matches!(
            self,
            PlanTaskStatus::Completed | PlanTaskStatus::Skipped | PlanTaskStatus::Cancelled
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            PlanTaskStatus::Pending => "Pending",
            PlanTaskStatus::Active => "Active",
            PlanTaskStatus::Blocked => "Blocked",
            PlanTaskStatus::Completed => "Completed",
            PlanTaskStatus::Skipped => "Skipped",
            PlanTaskStatus::Cancelled => "Cancelled",
        }
    }
}

/// Evidence attached to a task by whoever completed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanEvidence {
    pub kind: String,
    pub reference: String,
    pub note: Option<String>,
}

/// One task of the engine's plan. The engine owns it; the UI only projects it.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanTask {
    pub id: u64,
    pub parent: Option<u64>,
    pub title: String,
    pub detail: String,
    pub status: PlanTaskStatus,
    pub reason: Option<String>,
    pub depends_on: Vec<u64>,
    pub criteria: String,
    pub evidence: Vec<PlanEvidence>,
    pub owner: Option<String>,
    pub archived: bool,
    /// A completion whose basis changed since (a dependency reopened, criteria revised).
    pub stale: bool,
    pub revision: u64,
}

/// Task counts among live tasks, plus archived. Counts of tasks, never a prediction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlanCounts {
    pub pending: u32,
    pub active: u32,
    pub blocked: u32,
    pub completed: u32,
    pub skipped: u32,
    pub cancelled: u32,
    pub archived: u32,
}

impl PlanCounts {
    pub fn open(&self) -> u32 {
        self.pending + self.active + self.blocked
    }

    pub fn finished(&self) -> u32 {
        self.completed + self.skipped + self.cancelled
    }

    pub fn live(&self) -> u32 {
        self.open() + self.finished()
    }
}

/// An edit the person can make to a task from the plan view. Each maps to one status change
/// with a fixed, honest reason; the engine validates it like any model edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanEdit {
    Complete,
    Reopen,
    Skip,
    Cancel,
}

impl PlanEdit {
    /// The status and reason this edit asks the engine for.
    pub fn status_and_reason(self) -> (PlanTaskStatus, Option<&'static str>) {
        match self {
            PlanEdit::Complete => (PlanTaskStatus::Completed, None),
            PlanEdit::Reopen => (
                PlanTaskStatus::Pending,
                Some("Reopened by the person from the plan view"),
            ),
            PlanEdit::Skip => (
                PlanTaskStatus::Skipped,
                Some("Skipped by the person from the plan view"),
            ),
            PlanEdit::Cancel => (
                PlanTaskStatus::Cancelled,
                Some("Cancelled by the person from the plan view"),
            ),
        }
    }

    /// Which edits a task in `status` offers.
    pub fn offered(status: PlanTaskStatus) -> &'static [PlanEdit] {
        if status.finished() {
            &[PlanEdit::Reopen]
        } else {
            &[PlanEdit::Complete, PlanEdit::Skip, PlanEdit::Cancel]
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PlanEdit::Complete => "Mark completed",
            PlanEdit::Reopen => "Reopen",
            PlanEdit::Skip => "Skip",
            PlanEdit::Cancel => "Cancel task",
        }
    }
}

/// The session's plan as last reported by the engine. `available` is false until an engine
/// that provides plans has reported one (an empty plan is reported as available with no
/// tasks); Pi and the demo's ordinary scenarios never set it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlanState {
    pub available: bool,
    pub revision: u64,
    /// Live tasks in the engine's display order.
    pub tasks: Vec<PlanTask>,
    pub counts: PlanCounts,
    /// The engine left out some archived tasks to stay within bounds.
    pub truncated: bool,
    /// The one task shown in full (criteria, evidence, actions).
    pub expanded: Option<u64>,
    /// Finished tasks are listed (a projection; nothing is deleted when collapsed).
    pub history_open: bool,
    /// One edit in flight; its answer must name this request.
    pub pending_edit: Option<RequestId>,
    /// Why the last edit was refused.
    pub error: Option<String>,
}

impl PlanState {
    pub fn task(&self, id: u64) -> Option<&PlanTask> {
        self.tasks.iter().find(|t| t.id == id)
    }

    /// Reset everything the engine reports; keep what the person chose to look at.
    pub(crate) fn sync(
        &mut self,
        revision: u64,
        tasks: Vec<PlanTask>,
        counts: PlanCounts,
        truncated: bool,
    ) {
        self.available = true;
        self.revision = revision;
        self.tasks = tasks;
        self.counts = counts;
        self.truncated = truncated;
        if self
            .expanded
            .is_some_and(|id| !self.tasks.iter().any(|t| t.id == id))
        {
            self.expanded = None;
        }
    }
}
