//! Calls, replies and events.
//!
//! Every call gets exactly one reply with the same id. Events carry a monotonically increasing
//! `seq` per connection within one engine `epoch`; a client that sees a gap (or a new epoch) discards what it
//! derived from events and re-reads snapshots. Admission, settlement and question answers are
//! durable before they are acknowledged or published, so a resync never loses an outcome: it
//! is found again with `Lookup`. `CatchUp` pages transactionally stored session invalidations
//! by a separate durable cursor; re-read affected state before `AckEvents`. Expired cursors
//! explicitly reset to the current head. Auth/progress remain volatile.

use serde::{Deserialize, Serialize};

use crate::content::ImageInput;
use crate::view::*;

/// A native session extension. Existing TypeScript facet packages require a separate port.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum ExtensionSpec {
    Builtin(String),
    /// Direct argv, without a shell. The host normalizes argv[0] to an absolute executable.
    Process {
        id: String,
        argv: Vec<String>,
    },
}

/// Which lane a frame travels in. Control frames (Stop, Shutdown) are handled ahead of queued
/// normal calls so a slow presentation or a busy session cannot delay cancellation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Lane {
    #[default]
    Normal,
    Control,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientFrame {
    pub id: u64,
    #[serde(default)]
    pub lane: Lane,
    pub call: Call,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "frame", rename_all = "camelCase")]
pub enum ServerFrame {
    Reply {
        id: u64,
        result: Result<Reply, ProtoError>,
    },
    Event {
        epoch: u64,
        seq: u64,
        event: Event,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    /// Protocol or schema mismatch; no amount of retrying helps.
    Incompatible,
    NotFound,
    /// Same request key with a different payload.
    Conflict,
    Invalid,
    TooLarge,
    /// The session is busy in a way that refuses this call (for example, a second prompt).
    Busy,
    /// A capability the engine does not provide (for example, a reserved provider).
    Unsupported,
    /// The session's writer failed closed after a storage error; reopen to reconcile.
    Storage,
    /// The engine is shutting down.
    ShuttingDown,
    /// The caller cancelled this request; committed work is not rolled back.
    Cancelled,
    Internal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtoError {
    pub code: ErrorCode,
    pub message: String,
}

impl ProtoError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> ProtoError {
        ProtoError {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ProtoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for ProtoError {}

/// A sign-in answer. Debug never prints it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Secret(pub String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[redacted]")
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "call", rename_all = "camelCase")]
pub enum Call {
    /// Cancel a queued/in-flight RPC on this connection, not an admitted coding run.
    /// A cancellation reply does not establish whether a mutation already committed.
    CancelRequest {
        request: u64,
    },
    Hello {
        protocol: u32,
        client: String,
    },
    /// Read bounded durable invalidations after a cursor. Re-read affected snapshots before
    /// acknowledging `through`; a reset means the retained range no longer covers `after`.
    CatchUp {
        after: u64,
        limit: u32,
    },
    AckEvents {
        through: u64,
    },
    /// Read/pin or explicitly replace the native extension selection. None session targets
    /// the engine default; omitted extensions restore the saved selection. [] is explicit.
    PrepareExtensions {
        session: Option<SessionId>,
        extensions: Option<Vec<ExtensionSpec>>,
    },
    ListSessions,
    /// Idempotent under `request`: the same key returns the same session.
    CreateSession {
        cwd: String,
        request: String,
    },
    RenameSession {
        session: SessionId,
        title: String,
    },
    /// Stop and remove a root and its children. The committed intent survives caller
    /// cancellation/restart. Repeating the same removed id succeeds without recreating it.
    RemoveSession {
        session: SessionId,
    },
    /// Attach this client to the session (idempotent until ReleaseSession/disconnect).
    OpenSession {
        session: SessionId,
        #[serde(default)]
        window: Option<u32>,
    },
    /// Release this client's attachment without cancelling runs or deleting history.
    /// Idempotent, including sessions that are no longer loaded.
    ReleaseSession {
        session: SessionId,
    },
    /// Entries strictly before `before` (or the newest when absent), oldest first.
    HistoryPage {
        session: SessionId,
        before: Option<EntryId>,
        limit: u32,
    },
    /// Durable admission under the journaled key. Same key and payload returns the original
    /// admission; same key with a different payload is a `Conflict`.
    Prompt {
        session: SessionId,
        request: String,
        text: String,
        #[serde(default)]
        images: Vec<ImageInput>,
        #[serde(default)]
        model: Option<String>,
    },
    /// A steer or follow-up for the active run (or the next one), under its journaled key.
    Enqueue {
        session: SessionId,
        request: String,
        mode: InputMode,
        text: String,
        #[serde(default)]
        images: Vec<ImageInput>,
    },
    /// What became of a key. Read-only; never sends anything to a model.
    Lookup {
        session: SessionId,
        request: String,
    },
    /// Wait for a durable prompt/steer/follow-up key without admitting or cancelling work.
    WaitForPrompt {
        session: SessionId,
        request: String,
    },
    CancelQueued {
        session: SessionId,
        entry: u64,
    },
    /// Persist Stop intent, then cancel provider, tools and pending retries. Settlement is
    /// published as a `RunSettled` event; the reply only confirms the intent is durable.
    Stop {
        session: SessionId,
    },
    SetModel {
        session: SessionId,
        model: String,
    },
    SetThinking {
        session: SessionId,
        level: String,
    },
    /// Select the next supported level, wrapping to the first; an unknown current level
    /// starts at the first. Serialized with model and explicit thinking changes.
    CycleThinking {
        session: SessionId,
    },
    Models {
        #[serde(default)]
        session: Option<SessionId>,
    },
    /// Re-read credentials, configuration and the catalog.
    RefreshModels,
    /// Summarize older context now.
    Compact {
        session: SessionId,
        #[serde(default)]
        instructions: Option<String>,
    },
    ToolOutput {
        session: SessionId,
        call_id: String,
    },
    AnswerQuestion {
        session: SessionId,
        question: String,
        answer: Answer,
    },
    CancelQuestion {
        session: SessionId,
        question: String,
    },
    DismissNotices {
        session: SessionId,
    },
    Subagents {
        session: SessionId,
    },
    /// The session's plan; `include_archived` adds archived tasks (bounded).
    Plan {
        session: SessionId,
        #[serde(default)]
        include_archived: bool,
    },
    /// Apply a batch of plan operations atomically. `request` is the client's idempotency
    /// key: the same key returns the recorded receipt (or rejection) without re-applying.
    UpdatePlan {
        session: SessionId,
        request: String,
        ops: Vec<PlanOp>,
        /// Reject the batch unless the plan is at this revision.
        #[serde(default)]
        expected_revision: Option<u64>,
    },
    /// The session's retained web sources (search results and fetched pages), oldest first.
    WebSources {
        session: SessionId,
    },
    /// The MCP servers configured for the session's project (user settings merged with the
    /// project's; user settings alone without a session) and their state. Secret values
    /// never appear.
    McpServers {
        #[serde(default)]
        session: Option<SessionId>,
    },
    /// Add or replace a server entry in the user or the session's project settings. `spec`
    /// is the settings entry; a value of `…` keeps the stored literal.
    McpUpsertServer {
        #[serde(default)]
        session: Option<SessionId>,
        scope: McpScope,
        name: String,
        spec: serde_json::Value,
    },
    McpRemoveServer {
        #[serde(default)]
        session: Option<SessionId>,
        scope: McpScope,
        name: String,
    },
    /// Tools switched off for a server (never offered or callable).
    McpSetDisabledTools {
        #[serde(default)]
        session: Option<SessionId>,
        scope: McpScope,
        name: String,
        disabled: Vec<String>,
    },
    /// Connect afresh and list the tools.
    McpTestServer {
        #[serde(default)]
        session: Option<SessionId>,
        name: String,
    },
    McpReconnectServer {
        #[serde(default)]
        session: Option<SessionId>,
        name: String,
    },
    /// Begin an OAuth sign-in for an HTTP server; progress arrives through `McpChanged`.
    McpAuthStart {
        #[serde(default)]
        session: Option<SessionId>,
        name: String,
    },
    McpAuthAnswer {
        name: String,
        prompt: String,
        response: Secret,
    },
    McpAuthCancel {
        name: String,
    },
    /// Forget the saved tokens.
    McpAuthRevoke {
        #[serde(default)]
        session: Option<SessionId>,
        name: String,
    },
    /// Convert pasted `mcpServers` configuration without writing anything.
    McpImportPreview {
        #[serde(default)]
        session: Option<SessionId>,
        text: String,
    },
    /// Write the named entries of a previewed import.
    McpImportApply {
        #[serde(default)]
        session: Option<SessionId>,
        scope: McpScope,
        text: String,
        names: Vec<String>,
    },
    /// Set (or replace) the session's goal; the engine schedules its turns.
    SetGoal {
        session: SessionId,
        objective: String,
    },
    /// Pause, resume, revise or archive the current goal.
    GoalControl {
        session: SessionId,
        action: GoalAction,
    },
    /// Import a goal the desktop kept in its own records, as paused, exactly once per `key`.
    /// When the engine already has a goal, that goal stands and the key is still recorded.
    MigrateGoal {
        session: SessionId,
        key: String,
        objective: String,
        #[serde(default)]
        paused: Option<String>,
    },
    /// The session's goal history (archived goals, newest first).
    GoalHistory {
        session: SessionId,
    },
    SubagentHistory {
        session: SessionId,
        child: u64,
        before: Option<EntryId>,
        limit: u32,
    },
    SetSubagentsEnabled {
        session: SessionId,
        enabled: bool,
    },
    AuthState,
    AuthStart {
        provider: String,
    },
    AuthReuse {
        provider: String,
    },
    AuthAnswer {
        attempt: String,
        challenge: String,
        response: Secret,
    },
    AuthCancel {
        attempt: String,
    },
    AuthRemove {
        provider: String,
    },
    /// Store an API key for a provider (explicit user action; never inferred).
    SetApiKey {
        provider: String,
        key: Secret,
    },
    /// List what a legacy (Pi) profile holds: sessions with their import status, configuration
    /// items and credential metadata. Read-only. `agent_dir` defaults to
    /// `$PI_CODING_AGENT_DIR`, else `~/.pi/agent`.
    ImportDiscover {
        /// Pi coordination directory; defaults to PI_SERVER_DIR, else ~/.pi/server.
        #[serde(default, rename = "serverDir", skip_serializing_if = "Option::is_none")]
        server_dir: Option<String>,
        #[serde(default, rename = "agentDir")]
        agent_dir: Option<String>,
    },
    /// Import the named legacy sessions (explicit user action). Idempotent per session; the
    /// legacy source is never written.
    ImportSessions {
        /// The same coordination directory inspected during discovery.
        #[serde(default, rename = "serverDir", skip_serializing_if = "Option::is_none")]
        server_dir: Option<String>,
        #[serde(default, rename = "agentDir")]
        agent_dir: Option<String>,
        sessions: Vec<String>,
    },
    /// Copy the named configuration items (`models.json`, `settings.json`, `AGENTS.md`,
    /// `skills`, ...) into the native configuration, never overwriting a native file.
    ImportConfig {
        #[serde(default, rename = "agentDir")]
        agent_dir: Option<String>,
        items: Vec<String>,
    },
    /// Merge the named providers' saved credentials into the native credential store (explicit
    /// consent). Existing native entries are never overwritten. Supported subscription
    /// credentials retain their OAuth kind and metadata.
    ImportCredentials {
        #[serde(default, rename = "agentDir")]
        agent_dir: Option<String>,
        providers: Vec<String>,
    },
    Shutdown,
}

impl Call {
    /// Calls that must not wait behind normal traffic.
    pub fn lane(&self) -> Lane {
        match self {
            Call::Stop { .. } | Call::Shutdown | Call::CancelRequest { .. } => Lane::Control,
            _ => Lane::Normal,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CancelQueuedOutcome {
    Cancelled,
    AlreadyConsumed,
    NotFound,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum LookupResult {
    /// The engine never admitted this key.
    Unknown,
    Known {
        mode: InputMode,
        /// Queue entry for steers/follow-ups.
        #[serde(default)]
        entry: Option<u64>,
        run: Option<u64>,
        /// Still queued or running.
        open: bool,
        #[serde(default)]
        outcome: Option<RunOutcome>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Welcome {
    pub protocol: u32,
    pub engine_version: String,
    pub engine_revision: String,
    pub schema: u32,
    pub epoch: u64,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolOutputView {
    pub text: String,
    pub retained_bytes: u64,
    /// Bytes the tool produced beyond the retention limit (not available anywhere).
    pub dropped_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "camelCase")]
pub enum Reply {
    PromptResult(PromptResult),
    Welcome(Welcome),
    Events(EventPage),
    ExtensionProfile {
        session: Option<SessionId>,
        extensions: Vec<ExtensionSpec>,
    },
    Sessions {
        sessions: Vec<SessionSummary>,
    },
    Session {
        summary: SessionSummary,
    },
    Snapshot(Box<SessionSnapshot>),
    Page {
        /// Same stable provenance boundary as the session snapshot, including child pages.
        native_history_start: u64,
        entries: Vec<Entry>,
        has_older: bool,
    },
    Admitted {
        run: Option<u64>,
        entry: Option<u64>,
    },
    Lookup(LookupResult),
    CancelQueued {
        outcome: CancelQueuedOutcome,
    },
    Stopping {
        run: Option<u64>,
    },
    Models(ModelsView),
    ToolOutput(ToolOutputView),
    Subagents(SubagentsView),
    Plan(PlanView),
    PlanUpdated(PlanReceipt),
    WebSources(Vec<WebSourceView>),
    McpServers(McpServersView),
    McpTest(McpTestView),
    McpImport(McpImportView),
    Goal(Option<GoalView>),
    Goals(Vec<GoalView>),
    Auth(AuthSnapshot),
    Thinking(ThinkingView),
    ImportReport(ImportReport),
    Imported(ImportedView),
    ConfigImported(ConfigImportView),
    CredentialsImported(CredentialsImportView),
    Ok,
}

/// A committed change in one session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "camelCase")]
pub enum SessionChange {
    EntryAppended {
        entry: Entry,
    },
    /// A committed entry changed (a streaming message finalized, or entries retired).
    EntryReplaced {
        entry: Entry,
    },
    EntryDelta {
        id: EntryId,
        delta: Delta,
    },
    /// Volatile, coalesced tool output progress; the result arrives as an entry.
    ToolProgress {
        call_id: String,
        text: String,
    },
    RunChanged {
        run: Option<RunView>,
    },
    RunSettled {
        run: u64,
        request: Option<String>,
        outcome: RunOutcome,
    },
    QueueChanged {
        queue: Vec<QueueEntryView>,
    },
    /// A queued input was admitted under `request`.
    QueueAdmitted {
        request: String,
        entry: u64,
    },
    UsageChanged {
        usage: UsageView,
    },
    ModelChanged {
        model: Option<String>,
    },
    ThinkingChanged {
        thinking: ThinkingView,
    },
    UiChanged {
        ui: UiView,
    },
    SubagentsChanged {
        subagents: SubagentsView,
    },
    /// The plan was changed (by the model or a client); live tasks only.
    PlanChanged {
        plan: PlanView,
    },
    /// The current goal changed (set, scheduled, paused, completed, archived).
    GoalChanged {
        goal: Option<Box<GoalView>>,
    },
    /// A child session's history changed (the selected child's viewer re-reads its page).
    ChildChanged {
        child: u64,
    },
    /// The live context boundary moved (compaction); older history is now pageable.
    BoundaryMoved {
        has_older: bool,
    },
    SummaryChanged {
        summary: SessionSummary,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "camelCase")]
pub enum Event {
    /// Live presentation was coalesced. Catch up from the durable cursor and refresh volatile state.
    ResyncRequired,
    SessionsChanged,
    Session {
        session: SessionId,
        revision: u64,
        change: SessionChange,
    },
    ModelsChanged,
    /// Sign-in state for this client changed. Volatile; never in the durable outbox.
    AuthChanged(AuthSnapshot),
    /// MCP server state (connection, sign-in, catalog) changed; read `McpServers` again.
    McpChanged,
    Notice {
        message: String,
    },
}

/// Ordered, transactionally recorded snapshot invalidations, independent of live event epochs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DurableEvent {
    pub cursor: u64,
    pub session: String,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventPage {
    pub events: Vec<DurableEvent>,
    pub through: u64,
    pub head: u64,
    pub reset: bool,
}
