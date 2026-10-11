//! Read models the engine publishes: catalog rows, durable entries, run/queue state, models,
//! sign-in state, extension questions and subagents. These are projections of committed
//! state; nothing here is a command.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::content::{Message, Usage};

/// Engine session id (stable, opaque, unique across engines and imports).
pub type SessionId = String;
/// Durable entry id, unique and increasing within a session.
pub type EntryId = u64;

/// Where a session's history came from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Origin {
    #[default]
    Native,
    /// Imported from a legacy engine profile; `source` is the legacy session id.
    Imported { engine: String, source: String },
}

/// A top-level conversation in the catalog. Child (subagent) sessions are not listed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: SessionId,
    pub cwd: String,
    #[serde(default)]
    pub title: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    #[serde(default)]
    pub origin: Origin,
    /// Committed provider/model selection, `provider/modelId`.
    #[serde(default)]
    pub model: Option<String>,
    /// The session cannot run because its provider is unavailable in this engine (for example,
    /// a reserved provider). History stays readable; nothing is executed or rerouted.
    #[serde(default)]
    pub blocked: Option<String>,
}

/// How an input joined the conversation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InputMode {
    #[default]
    Prompt,
    Steer,
    FollowUp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NoticeLevel {
    Info,
    Warning,
    Error,
}

/// What a durable entry records.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum EntryKind {
    /// A model-context message. `partial` is true while an assistant message streams; its
    /// committed prefix is shown, its final form replaces it.
    Message {
        message: Message,
        #[serde(default)]
        mode: InputMode,
        /// The request key that admitted a user input.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        partial: bool,
    },
    /// Everything before `first_kept` is summarized by `summary` in the model context.
    Compaction {
        summary: String,
        first_kept: EntryId,
        tokens_before: u64,
    },
    /// History before this entry leaves the model context. `handoff` (if any) starts the new
    /// context.
    Reset {
        #[serde(default)]
        handoff: Option<Message>,
    },
    Notice {
        level: NoticeLevel,
        text: String,
    },
    ModelChange {
        model: String,
    },
    ThinkingChange {
        level: String,
    },
    /// Extension state or an imported payload this engine does not interpret. Preserved
    /// verbatim with its provenance; never sent to a model.
    Custom {
        custom_type: String,
        data: Value,
    },
}

/// A committed entry of a session's history.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: EntryId,
    pub at_ms: i64,
    #[serde(default)]
    pub run: Option<u64>,
    /// Excluded from future model context (a stopped instruction and its partial reply).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub retired: bool,
    pub kind: EntryKind,
}

/// An incremental change to a streaming entry, published only after it is committed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Delta {
    Text { block: u32, text: String },
    Thinking { block: u32, text: String },
    ToolCall { block: u32, id: String, name: String },
}

/// Why a run ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "camelCase")]
pub enum RunOutcome {
    Completed,
    /// Settled after a Stop.
    Aborted,
    ModelError { detail: String },
    NoModel { detail: String },
    /// The engine stopped while the run was in flight; effects after the last committed
    /// entry are unconfirmed.
    Interrupted { detail: String },
    Failed { detail: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunPhase {
    Running,
    /// Stop intent is durable; settlement has not been confirmed yet.
    Stopping,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunView {
    pub id: u64,
    pub request: Option<String>,
    pub phase: RunPhase,
    pub started_at_ms: i64,
}

/// A steer or follow-up the engine holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueEntryView {
    pub entry: u64,
    pub request: String,
    pub mode: InputMode,
    pub text: String,
    pub images: u32,
}

/// The committed per-session usage ledger.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageView {
    /// By `provider/model`.
    pub models: Vec<(String, Usage)>,
    /// By tool name (subagent work is attributed to its tool).
    pub tools: Vec<(String, Usage)>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingView {
    pub level: String,
    pub levels: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelView {
    /// `provider/modelId`.
    pub key: String,
    pub name: String,
    pub provider: String,
    pub api: String,
    pub reasoning: bool,
    pub images: bool,
    pub context_window: u64,
    pub max_tokens: u64,
    /// Credentials or configuration exist for this provider.
    pub available: bool,
    /// Why the model cannot be used (missing key, reserved provider, ...).
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsView {
    pub models: Vec<ModelView>,
    pub selected: Option<String>,
    /// Provider configuration problems found on the last refresh (said once by the client).
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QuestionKind {
    Select,
    Confirm,
    Input,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionOption {
    pub value: String,
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionView {
    pub id: String,
    pub kind: QuestionKind,
    pub title: String,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub options: Vec<QuestionOption>,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub default_value: Option<String>,
    /// Unix milliseconds after which the engine cancels the question.
    #[serde(default)]
    pub deadline_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoticeView {
    pub id: String,
    pub level: NoticeLevel,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiView {
    pub questions: Vec<QuestionView>,
    pub status: Vec<(String, String)>,
    pub notices: Vec<NoticeView>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Answer {
    Choice { value: String },
    Confirm { value: bool },
    Text { value: String },
}

/// One durable child task of a session's subagent tool calls.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentRow {
    /// Stable child number within the root session.
    pub child: u64,
    pub task_id: u64,
    pub call_id: String,
    /// Bounded task excerpt.
    pub task: String,
    /// `pending | running | waiting | completing | completed | failed | stopped`.
    pub status: String,
    pub session: SessionId,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentsView {
    pub enabled: bool,
    pub children: Vec<SubagentRow>,
    /// More rows exist than were returned.
    pub truncated: bool,
}

/// Everything needed to show an opened session, read atomically at one revision.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshot {
    pub summary: SessionSummary,
    pub revision: u64,
    /// The newest window of history, oldest first, starting at the live context boundary at
    /// the latest (a compaction moves it).
    pub entries: Vec<Entry>,
    pub has_older: bool,
    pub run: Option<RunView>,
    pub queue: Vec<QueueEntryView>,
    pub usage: UsageView,
    pub thinking: ThinkingView,
    pub ui: UiView,
    pub subagents: Option<SubagentsView>,
}

/// Credential kinds are distinct identities: an API key never stands in for a subscription.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CredentialKind {
    ApiKey,
    OAuth,
    /// Ambient platform credentials (AWS profile, Google ADC); never copied by the engine.
    Ambient,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthProviderView {
    /// Provider id, e.g. `openai-codex`.
    pub id: String,
    pub name: String,
    pub kind: CredentialKind,
    /// The native engine implements this sign-in.
    pub available: bool,
    /// Why it is not available (for example, reserved for a later integration).
    #[serde(default)]
    pub unavailable_reason: Option<String>,
    /// Saved credentials exist (metadata only; values never leave the engine).
    pub has_credentials: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AuthStatus {
    #[default]
    Idle,
    Connecting,
    Waiting,
    Prompt,
    Done,
    Failed,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthChallenge {
    pub id: String,
    /// `code | text | select | confirm`.
    pub kind: String,
    pub message: String,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub options: Vec<(String, String)>,
}

impl std::fmt::Debug for AuthChallenge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthChallenge")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// Volatile per-client sign-in state. Never persisted, never logged (Debug is redacted).
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthSnapshot {
    pub providers: Vec<AuthProviderView>,
    pub credential_lookup_failed: bool,
    pub attempt: Option<String>,
    pub provider: Option<String>,
    pub status: AuthStatus,
    pub message: String,
    pub url: Option<String>,
    pub device_code: Option<String>,
    pub challenge: Option<AuthChallenge>,
}

impl std::fmt::Debug for AuthSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthSnapshot")
            .field("provider", &self.provider)
            .field("status", &self.status)
            .field("providers", &self.providers.len())
            .finish_non_exhaustive()
    }
}
