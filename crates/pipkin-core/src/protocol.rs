use crate::ids::*;
use crate::model::*;
use crate::onboarding::{SignInAnswer, SignInProvider};

/// User intents issued by the UI or command palette.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    RemoveSignIn(SignInProvider),
    StartSignIn(SignInProvider),
    ReuseSignIn(SignInProvider),
    AnswerSignIn {
        attempt: String,
        challenge: String,
        response: SignInAnswer,
    },
    CancelSignIn {
        attempt: String,
    },
    SelectProject(ProjectId),
    SelectConversation(ConversationId),
    /// Navigate the selected session's read-only child transcript, or back to its parent.
    SelectSubagent(Option<u64>),
    /// This session's engine setting, not an app-local visibility preference.
    SetSubagentsEnabled(bool),
    /// Expand or collapse the directory's finished-task history (a projection, not deletion).
    ToggleSubagentHistory,
    /// Reveal the next page of finished tasks (paged engines only).
    ShowMoreSubagentHistory,
    /// Read the page of child activity before the oldest shown entry (paged engines only).
    LoadOlderSubagent,
    NewConversation,
    RenameConversation(ConversationId, String),
    SetSearch(String),

    EditDraft(String),
    PreviousPrompt,
    NextPrompt,
    FlushDraft(ConversationId),
    AddAttachments(Vec<Attachment>),
    RemoveAttachment(usize),
    SetModel(String),
    SetThinkingLevel(String),
    /// Add a project folder (by absolute path) so conversations can be created in it.
    AddProject(String),
    /// Dismiss the application-level notice shown after a failed background action.
    DismissNotice,
    /// Dismiss the notice about saved state (a recovery or an unsaved-work warning).
    DismissStorageIssue,
    /// Ask the engine to re-read its provider credentials and model catalogue.
    RefreshModels,
    RefreshChanges,
    /// Save the current draft again after a failed save.
    RetrySave,

    /// Copy the complete output of a tool call, fetching it from the engine when the preview
    /// holds only part of it.
    CopyToolOutput(ItemId),
    /// Save the complete output of a tool call to a file the person chooses.
    SaveToolOutput(ItemId),
    /// Answer a question an extension asked.
    AnswerUiRequest {
        id: String,
        answer: UiAnswer,
    },
    /// Decline to answer a question an extension asked.
    CancelUiRequest(String),
    /// Dismiss the notices extensions have posted.
    DismissUiNotices,
    /// Show where a search hit is: open its conversation and scroll to the message.
    OpenSearchHit(usize),
    /// Go back to the conversation that was open before a search hit was opened.
    ReturnFromSearch,
    /// The message the view was asked to scroll to has been shown.
    ClearScrollTarget,
    /// Open a changed file (by its index in the changes list) in the editor.
    OpenInEditor(usize),
    /// Open a terminal in the project's folder.
    OpenTerminal,

    Submit,
    /// Set or replace the active goal for the selected conversation.
    SetGoal(String),
    /// Remove the active goal and stop scheduling continuations.
    ClearGoal,
    Steer,
    QueueFollowUp,
    RemoveQueued(QueueId),
    Cancel,
    /// Ask the backend what happened to an acknowledgment-less submission.
    CheckStatus,
    /// Repeat a stop request that failed, never a prompt.
    RetryStop,
    DismissRecoveryNotice,
    /// Resubmit after a rejection or failure. Explicit user action only.
    Retry,
    DismissFailure,

    LoadOlder,
    SelectChange(usize),

    SetTheme(Theme),
    SetTextSize(TextSize),
    SetReducedMotion(bool),
    SetNavWidth(f32),
    SetInspectorWidth(f32),
    /// The window was resized to this many logical pixels.
    SetWindowSize(f32, f32),
    SetInspectorOpen(bool),

    /// Ask the engine what a Pi profile holds and offer to import it (built-in engine only).
    ImportFromPi(crate::import::ImportKind),
    /// Import what the current offer lists. Explicit user action only.
    ConfirmImport,
    /// Drop the current offer without importing anything.
    CancelImport,
}

/// What the application can do right now. UI controls and the command palette share this.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Availability {
    pub new_conversation: bool,
    pub submit: bool,
    pub steer: bool,
    pub queue: bool,
    pub cancel: bool,
    pub refresh_models: bool,
    pub refresh_changes: bool,
    pub retry_save: bool,
    pub open_in_editor: bool,
    pub open_terminal: bool,
    pub check_status: bool,
    pub retry_stop: bool,
    pub retry: bool,
    pub load_older: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BackendRequest {
    RemoveSignIn(SignInProvider),
    /// V1 sign-in can only request Pi's allowlisted subscription OAuth methods.
    StartSignIn(SignInProvider),
    ReuseSignIn(SignInProvider),
    AnswerSignIn {
        attempt: String,
        challenge: String,
        response: SignInAnswer,
    },
    CancelSignIn {
        attempt: String,
    },
    Open {
        conversation: ConversationId,
        generation: u64,
    },
    OpenSubagent {
        conversation: ConversationId,
        generation: u64,
        child: u64,
    },
    SetSubagentsEnabled {
        conversation: ConversationId,
        generation: u64,
        enabled: bool,
    },
    /// The bounded page of `child`'s history strictly before the item `before`. Answered by
    /// `SubagentOlderPage` or `SubagentOlderFailed` naming the same child and cursor.
    LoadOlderSubagent {
        conversation: ConversationId,
        generation: u64,
        child: u64,
        before: ItemId,
    },
    LoadOlder {
        conversation: ConversationId,
        generation: u64,
        before: Option<ItemId>,
    },
    Submit {
        conversation: ConversationId,
        generation: u64,
        op: OperationId,
        /// Durable idempotency key, already journaled before this request is sent.
        request: RequestId,
        text: String,
        attachments: Vec<Attachment>,
        model: Option<String>,
    },
    Steer {
        conversation: ConversationId,
        generation: u64,
        op: OperationId,
        text: String,
    },
    /// Steer the active run or queue a follow-up (real mode). `request` is the journaled key, so
    /// the engine admits it once however often it is sent.
    Queue {
        conversation: ConversationId,
        generation: u64,
        request: RequestId,
        mode: QueueMode,
        text: String,
        attachments: Vec<Attachment>,
    },
    /// Withdraw an input the engine still holds in its queue.
    CancelQueued {
        conversation: ConversationId,
        generation: u64,
        entry: QueueId,
    },
    /// Re-read the engine's credentials and model catalogue.
    RefreshModels {
        conversation: ConversationId,
        generation: u64,
    },
    /// Re-scan the open conversation's project without sending any input to the engine.
    RefreshChanges {
        conversation: ConversationId,
        generation: u64,
    },
    /// Fetch the complete result of a tool call from the engine.
    FetchToolOutput {
        conversation: ConversationId,
        generation: u64,
        call_id: String,
    },
    /// Answer a question an extension asked.
    UiRespond {
        conversation: ConversationId,
        generation: u64,
        id: String,
        answer: UiAnswer,
    },
    /// Withdraw a question an extension asked.
    UiCancel {
        conversation: ConversationId,
        generation: u64,
        id: String,
    },
    Cancel {
        conversation: ConversationId,
        generation: u64,
        op: OperationId,
    },
    CheckStatus {
        conversation: ConversationId,
        generation: u64,
        op: OperationId,
        /// The journaled key of the submission in doubt, for backends that can look it up.
        request: Option<RequestId>,
    },
    /// Ask the backend to create a conversation whose agent works in `cwd`.
    CreateConversation {
        project: ProjectId,
        cwd: String,
        request: RequestId,
    },
    /// Ask the backend to use `model` for this conversation. The backend, not this request, is
    /// authoritative: the selection changes only when it reports it.
    SetModel {
        conversation: ConversationId,
        generation: u64,
        model: String,
    },
    SetThinkingLevel {
        conversation: ConversationId,
        generation: u64,
        level: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct BackendEvent {
    pub conversation: ConversationId,
    pub generation: u64,
    pub op: Option<OperationId>,
    pub kind: EventKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EventKind {
    /// Initial window of history for a conversation.
    Opened {
        items: Vec<TranscriptItem>,
        has_older: bool,
        changes: Vec<FileChange>,
    },
    OlderPage {
        items: Vec<TranscriptItem>,
        has_older: bool,
    },
    /// The backend's current view of an open conversation, replacing what is shown. Used for
    /// replicated state that changes after the initial window (live output, other writers).
    /// Ignored until the conversation has been opened.
    Synced {
        items: Vec<TranscriptItem>,
    },
    /// Current session's authoritative child directory and effective tool setting.
    SubagentsSynced {
        enabled: bool,
        children: Vec<SubagentInfo>,
    },
    /// No supported child service, or a read failed; never show this as an empty list.
    SubagentsUnavailable(String),
    /// A selected child view, tagged so a late response cannot replace another child.
    SubagentView {
        child: u64,
        items: Vec<TranscriptItem>,
        live: bool,
    },
    SubagentViewFailed {
        child: u64,
        reason: String,
    },
    /// A paging engine's directory (native): like `SubagentsSynced`, and marks the session's
    /// directory and child history as pageable. `truncated`: the engine cut it at its bound.
    SubagentDirectory {
        enabled: bool,
        children: Vec<SubagentInfo>,
        truncated: bool,
    },
    /// The newest bounded page of the selected child's history (oldest first; item ids grow
    /// with history order). Older pages already shown are kept when this page overlaps them.
    SubagentPage {
        child: u64,
        items: Vec<TranscriptItem>,
        live: bool,
        has_older: bool,
    },
    /// The page strictly before `before` (oldest first), answering `LoadOlderSubagent`.
    SubagentOlderPage {
        child: u64,
        before: ItemId,
        items: Vec<TranscriptItem>,
        has_older: bool,
    },
    SubagentOlderFailed {
        child: u64,
        before: ItemId,
        reason: String,
    },
    /// The engine's authoritative per-session ledger, independent of the displayed history.
    UsageSynced(SessionUsage),
    /// Opening the conversation failed. It stays unopened, so selecting it again retries.
    OpenFailed {
        message: String,
    },
    Accepted,
    /// The submission was refused before any work began.
    Rejected {
        reason: String,
    },
    /// The connection dropped before acknowledgment.
    AckLost,
    /// A stop request failed; this is not proof that the run has stopped.
    StopFailed {
        message: String,
    },
    /// A read-only status check failed; the existing outcome remains unresolved.
    StatusCheckFailed {
        message: String,
    },
    /// Answer to `CheckStatus`.
    StatusResolved {
        accepted: bool,
    },
    Token(String),
    ToolStarted {
        call: u32,
        name: String,
        input: String,
    },
    ToolOutput {
        call: u32,
        chunk: String,
    },
    ToolFinished {
        call: u32,
        ok: bool,
    },
    ChangesReported(Vec<FileChange>),
    /// Changes belonging to this conversation. Replaces its displayed file evidence.
    ChangesSynced(Vec<FileChange>),
    /// Collection progress, coverage, or failure; failure preserves the last successful result.
    ChangesScanState(ChangesState),
    /// Pi's authoritative, per-conversation thinking setting and supported levels.
    ThinkingState {
        level: String,
        levels: Vec<String>,
    },
    Completed,
    Failed {
        message: String,
    },
    /// The engine confirms a cancel has settled.
    Cancelled,
    SteerAccepted,
    /// The engine holds the queue request `request` as `entry`.
    QueueAdmitted {
        request: RequestId,
        entry: QueueId,
    },
    /// The engine definitely did not take the queue request.
    QueueRefused {
        request: RequestId,
        reason: String,
    },
    /// The connection dropped before the engine answered a queue request.
    QueueAckLost {
        request: RequestId,
    },
    /// Answer to `CancelQueued`.
    QueueCancelled {
        entry: QueueId,
        outcome: CancelOutcome,
    },
    /// Whether the conversation has history before what is shown, after the shown part changed
    /// (a compaction or reset moved where the live view starts).
    HasOlder(bool),
    /// Loading older history failed; asking again retries.
    OlderFailed {
        message: String,
    },
    /// The complete output of a tool call.
    ToolOutputFull {
        call_id: String,
        text: String,
    },
    /// The complete output could not be had.
    ToolOutputUnavailable {
        call_id: String,
        reason: String,
    },
    /// The questions extensions are waiting on, their status lines and notices.
    UiState {
        requests: Vec<UiRequest>,
        status: Vec<(String, String)>,
        notices: Vec<UiNotice>,
    },
    /// The engine did not take an answer; the question stays open.
    UiRespondRefused {
        id: String,
        reason: String,
    },
    /// The engine's own view of the run and its queue. Not tied to any operation: it is how a
    /// run started elsewhere, or before this window opened, becomes visible and how it ends.
    EngineState {
        busy: bool,
        queue: Vec<QueuedPrompt>,
    },
}

/// Work the controller executes on the core's behalf.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Backend(BackendRequest),
    SaveDraft {
        conversation: ConversationId,
        text: String,
        attachments: Vec<Attachment>,
        rev: u64,
    },
    SavePrefs(Prefs),
    /// Acknowledged write. Never show completion until the writer commits this preference.
    CompleteSetup(Prefs),
    SaveGoal {
        conversation: ConversationId,
        text: Option<String>,
        paused: Option<String>,
    },
    SaveConversation {
        conversation: ConversationId,
    },
    /// Remember a project folder the user opened.
    SaveProject {
        path: String,
    },
    /// Keep a copy of the conversation's transcript for offline reading and search.
    SaveCache {
        conversation: ConversationId,
    },
    /// Show the saved copy of a conversation while the engine's own state is fetched.
    LoadCache {
        conversation: ConversationId,
    },
    /// Search saved history. The controller answers with `AppState::apply_search_results`.
    SearchHistory {
        query: String,
    },
    CopyText(String),
    /// Save text to a file the person chooses.
    SaveText {
        suggested_name: String,
        text: String,
    },
    Launch(Launch),
    /// Durably record a submission's immutable payload. The controller must report the commit
    /// with `AppState::intent_persisted`; until then nothing is sent and the draft is kept.
    JournalIntent {
        conversation: ConversationId,
        request: RequestId,
        text: String,
        attachments: Vec<Attachment>,
        model: Option<String>,
        origin: IntentOrigin,
    },
    JournalState {
        conversation: ConversationId,
        request: RequestId,
        state: JournalState,
    },
    /// One step of an explicit Pi import. The controller answers with
    /// `AppState::import_discovered` or `AppState::import_finished`.
    Import(crate::import::ImportStep),
}

/// Lifecycle of a journaled request after its intent is recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JournalState {
    /// The acknowledgment was lost; whether the engine accepted it is unknown.
    Unknown,
    Accepted,
    Rejected,
    Completed,
    Failed,
    Cancelled,
    /// The engine admitted a steer or follow-up; its queue owns it from here.
    Queued,
    /// Replaced by a newer request for the same conversation during recovery.
    Superseded,
}

impl JournalState {
    /// No further reconciliation is needed once a request reaches a terminal state.
    pub fn is_terminal(self) -> bool {
        !matches!(self, JournalState::Unknown | JournalState::Accepted)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            JournalState::Unknown => "unknown",
            JournalState::Accepted => "accepted",
            JournalState::Rejected => "rejected",
            JournalState::Completed => "completed",
            JournalState::Failed => "failed",
            JournalState::Cancelled => "cancelled",
            JournalState::Queued => "queued",
            JournalState::Superseded => "superseded",
        }
    }
}

/// Fine-grained change notices so views can splice instead of rebuilding.
#[derive(Clone, Debug, PartialEq)]
pub enum Note {
    ItemsReset(ConversationId),
    ItemsPrepended(ConversationId, usize),
    ItemsAppended(ConversationId, usize),
    ItemChanged(ConversationId, usize),
    ConversationsChanged,
    SelectionChanged,
    Other,
}

#[derive(Debug, Default)]
pub struct Outcome {
    pub effects: Vec<Effect>,
    pub notes: Vec<Note>,
}

impl Outcome {
    pub fn merge(&mut self, other: Outcome) {
        self.effects.extend(other.effects);
        self.notes.extend(other.notes);
    }
}
