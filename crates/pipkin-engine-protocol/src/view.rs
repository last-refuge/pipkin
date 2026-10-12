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
    Text {
        block: u32,
        text: String,
    },
    Thinking {
        block: u32,
        text: String,
    },
    ToolCall {
        block: u32,
        id: String,
        name: String,
    },
}

/// The settled result of an input operation. Text includes only the answer's text blocks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum PromptResult {
    Done {
        text: String,
        reason: Option<String>,
    },
    Unanswered {
        text: Option<String>,
        reason: String,
    },
}

/// Why a run ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "camelCase")]
pub enum RunOutcome {
    Completed,
    /// Settled after a Stop.
    Aborted,
    ModelError {
        detail: String,
    },
    NoModel {
        detail: String,
    },
    /// The engine stopped while the run was in flight; effects after the last committed
    /// entry are unconfirmed.
    Interrupted {
        detail: String,
    },
    Failed {
        detail: String,
    },
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

/// Committed usage for a conversation and its descendants, grouped by original source.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageView {
    /// By `provider/model`.
    pub models: Vec<(String, Usage)>,
    /// By tool name when a tool reports its own usage. Child model usage stays in `models`.
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
    /// The profile the child was launched with (`worker`, `reviewer`, `scout`, or a
    /// settings-defined name); `None` for children launched before profiles existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
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
    /// First entry id allocated by native; earlier entries are the imported source copy.
    pub native_history_start: EntryId,
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
    /// The session's plan (live tasks). Empty when it has none.
    #[serde(default)]
    pub plan: PlanView,
    /// The session's current (unarchived) goal, when the engine owns goals.
    #[serde(default)]
    pub goal: Option<GoalView>,
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

// ------------------------------------------------------------------------------------------
// Import of a legacy (Pi) profile. Explicit, user-approved calls only; never automatic.

/// Where one legacy session stands with respect to a native import.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ImportSessionStatus {
    Importable,
    AlreadyImported {
        native: SessionId,
    },
    /// A writer owns the session (a running Pi worker) or it changed while it was read.
    Locked {
        #[serde(default)]
        detail: Option<String>,
    },
    /// The native engine cannot represent the session without guessing.
    Blocked {
        reasons: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSessionView {
    /// The legacy session id (also the native id it is imported under).
    pub id: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub created_at_ms: Option<i64>,
    pub status: ImportSessionStatus,
}

/// One saved legacy credential: provider id and kind only, never a value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportCredentialView {
    pub provider: String,
    /// The stored `type` (`api_key`, `oauth`, ...).
    pub kind: String,
    /// Set when this engine will not import it as usable (kept for a later integration).
    #[serde(default)]
    pub reserved: Option<String>,
    /// The native credential store already has an entry for this provider (it is kept).
    #[serde(default)]
    pub native_exists: bool,
}

/// What a legacy profile holds that could be imported. Read-only; values are never included.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub agent_dir: String,
    pub sessions: Vec<ImportSessionView>,
    /// Configuration items present (`models.json`, `settings.json`, `AGENTS.md`, `skills`, ...).
    pub config: Vec<String>,
    pub credentials: Vec<ImportCredentialView>,
    /// `auth.json` exists but could not be read (no contents are included).
    #[serde(default)]
    pub credentials_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub enum ImportSessionOutcome {
    Imported {
        native: SessionId,
        /// Additional native sessions created for subagent and forked conversations.
        children: u32,
        /// Source history entries.
        entries: u64,
        /// Submissions recorded as settled requests.
        requests: u64,
    },
    /// An earlier import is returned; nothing was written.
    AlreadyImported {
        native: SessionId,
    },
    Locked {
        detail: String,
    },
    Blocked {
        reasons: Vec<String>,
    },
    Failed {
        detail: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSessionResult {
    pub source: String,
    pub outcome: ImportSessionOutcome,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedView {
    pub sessions: Vec<ImportSessionResult>,
    /// Where consistent copies of the imported sources were kept, if any were taken.
    #[serde(default)]
    pub backup_dir: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub enum ConfigItemOutcome {
    /// Copied `files` files into the native configuration.
    Copied {
        files: u32,
    },
    /// Some files were copied; `kept` existing native files were left as they were.
    Partial {
        files: u32,
        kept: u32,
    },
    /// Every native file already existed and was kept.
    KeptExisting,
    NotFound,
    Failed {
        detail: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigItemResult {
    pub name: String,
    pub outcome: ConfigItemOutcome,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigImportView {
    pub items: Vec<ConfigItemResult>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub enum CredentialImportOutcome {
    Imported,
    /// The native store already has an entry for this provider; it was not overwritten.
    KeptExisting,
    /// Preserved where it is (the legacy profile) but not imported as usable.
    Reserved {
        reason: String,
    },
    NotFound,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialImportResult {
    pub provider: String,
    pub outcome: CredentialImportOutcome,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialsImportView {
    pub providers: Vec<CredentialImportResult>,
}

#[cfg(test)]
mod import_tests {
    use super::*;
    use crate::api::{Call, Reply};

    fn round_trip<T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(
        value: &T,
    ) -> Value {
        let json = serde_json::to_value(value).unwrap();
        let back: T = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(&back, value);
        json
    }

    #[test]
    fn import_calls_round_trip_with_their_wire_names() {
        let json = round_trip(&Call::ImportDiscover {
            server_dir: None,
            agent_dir: None,
        });
        assert_eq!(
            json,
            serde_json::json!({"call": "importDiscover", "agentDir": null})
        );
        let parsed: Call = serde_json::from_str(r#"{"call": "importDiscover"}"#).unwrap();
        assert_eq!(
            parsed,
            Call::ImportDiscover {
                server_dir: None,
                agent_dir: None
            }
        );
        let json = round_trip(&Call::ImportSessions {
            server_dir: Some("/server".into()),
            agent_dir: Some("/a".into()),
            sessions: vec!["s1".into()],
        });
        assert_eq!(json["agentDir"], "/a");
        assert_eq!(json["serverDir"], "/server");
        assert_eq!(json["sessions"][0], "s1");
        round_trip(&Call::ImportConfig {
            agent_dir: None,
            items: vec!["models.json".into()],
        });
        let json = round_trip(&Call::ImportCredentials {
            agent_dir: None,
            providers: vec!["openrouter".into()],
        });
        assert_eq!(json["call"], "importCredentials");
        assert_eq!(crate::PROTOCOL_VERSION, 20);
        let servers = Call::McpServers {
            session: Some("s1".into()),
        };
        assert_eq!(
            serde_json::to_value(&servers).unwrap()["call"],
            "mcpServers"
        );
        round_trip(&servers);
        let release = Call::ReleaseSession {
            session: "s1".into(),
        };
        assert_eq!(
            serde_json::to_value(&release).unwrap()["call"],
            "releaseSession"
        );
        round_trip(&release);
    }

    fn view(id: &str, status: ImportSessionStatus) -> ImportSessionView {
        ImportSessionView {
            id: id.into(),
            cwd: None,
            created_at_ms: None,
            status,
        }
    }

    #[test]
    fn import_replies_round_trip() {
        let json = round_trip(&Reply::ImportReport(ImportReport {
            agent_dir: "/a".into(),
            sessions: vec![
                view("s1", ImportSessionStatus::Importable),
                view("s2", ImportSessionStatus::Locked { detail: None }),
                view(
                    "s3",
                    ImportSessionStatus::Blocked {
                        reasons: vec!["a fork".into()],
                    },
                ),
                view(
                    "s4",
                    ImportSessionStatus::AlreadyImported {
                        native: "s4".into(),
                    },
                ),
            ],
            config: vec!["models.json".into()],
            credentials: vec![ImportCredentialView {
                provider: "anthropic".into(),
                kind: "oauth".into(),
                reserved: Some("later".into()),
                native_exists: false,
            }],
            credentials_error: None,
        }));
        assert_eq!(json["reply"], "importReport");
        assert_eq!(json["sessions"][2]["status"]["status"], "blocked");
        assert_eq!(json["credentials"][0]["nativeExists"], false);
        round_trip(&Reply::Imported(ImportedView {
            sessions: vec![
                ImportSessionResult {
                    source: "s1".into(),
                    outcome: ImportSessionOutcome::Imported {
                        native: "s1".into(),
                        children: 1,
                        entries: 5,
                        requests: 2,
                    },
                },
                ImportSessionResult {
                    source: "s2".into(),
                    outcome: ImportSessionOutcome::Failed {
                        detail: "io".into(),
                    },
                },
            ],
            backup_dir: Some("/d/imports/x".into()),
        }));
        round_trip(&Reply::ConfigImported(ConfigImportView {
            items: vec![
                ConfigItemResult {
                    name: "skills".into(),
                    outcome: ConfigItemOutcome::Partial { files: 1, kept: 2 },
                },
                ConfigItemResult {
                    name: "AGENTS.md".into(),
                    outcome: ConfigItemOutcome::KeptExisting,
                },
            ],
        }));
        round_trip(&Reply::CredentialsImported(CredentialsImportView {
            providers: vec![
                CredentialImportResult {
                    provider: "openrouter".into(),
                    outcome: CredentialImportOutcome::Imported,
                },
                CredentialImportResult {
                    provider: "anthropic".into(),
                    outcome: CredentialImportOutcome::Reserved {
                        reason: "later".into(),
                    },
                },
            ],
        }));
    }
}

// ------------------------------------------------------------------------------- plans

/// The state of one plan task.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    Pending,
    Active,
    Blocked,
    Completed,
    Skipped,
    Cancelled,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Pending => "pending",
            TaskStatus::Active => "active",
            TaskStatus::Blocked => "blocked",
            TaskStatus::Completed => "completed",
            TaskStatus::Skipped => "skipped",
            TaskStatus::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<TaskStatus> {
        Some(match s {
            "pending" => TaskStatus::Pending,
            "active" => TaskStatus::Active,
            "blocked" => TaskStatus::Blocked,
            "completed" => TaskStatus::Completed,
            "skipped" => TaskStatus::Skipped,
            "cancelled" => TaskStatus::Cancelled,
            _ => return None,
        })
    }

    /// Completed, skipped or cancelled: the task is finished one way or another.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            TaskStatus::Completed | TaskStatus::Skipped | TaskStatus::Cancelled
        )
    }

    /// A dependency in this state no longer holds its dependents back: completed work, or
    /// work explicitly waived by skipping it. Cancelled work never satisfies a dependent.
    pub fn satisfies_dependents(self) -> bool {
        matches!(self, TaskStatus::Completed | TaskStatus::Skipped)
    }

    /// A reason must accompany this status.
    pub fn needs_reason(self) -> bool {
        matches!(self, TaskStatus::Skipped | TaskStatus::Cancelled)
    }
}

/// A piece of evidence attached to a task: what kind of thing it is, a reference to it
/// (a tool call id, a command and its exit status, an artifact path or hash, a workspace
/// revision, or a labelled manual observation) and an optional note.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskEvidence {
    pub kind: String,
    pub reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One task of a plan, as stored and as shown.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanTaskView {
    /// Stable within the plan; never reused.
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<u64>,
    pub title: String,
    #[serde(default)]
    pub detail: String,
    pub status: TaskStatus,
    /// Why it was skipped, cancelled or reopened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<u64>,
    /// What must hold for the task to count as done.
    #[serde(default)]
    pub criteria: String,
    #[serde(default)]
    pub evidence: Vec<TaskEvidence>,
    /// A reference to whatever is doing the work (for example a subagent child or a job).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default)]
    pub archived: bool,
    /// A completion whose basis changed since: a dependency was reopened, or the criteria
    /// were revised after completion. Cleared when the task is completed again.
    #[serde(default)]
    pub stale: bool,
    /// The plan revision of the task's last change.
    pub revision: u64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at_ms: Option<i64>,
}

/// Task counts by status among live (non-archived) tasks, plus the archived count. These are
/// task counts, not a prediction of time or of the outcome.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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
    pub fn live(&self) -> u32 {
        self.pending + self.active + self.blocked + self.completed + self.skipped + self.cancelled
    }

    pub fn open(&self) -> u32 {
        self.pending + self.active + self.blocked
    }
}

/// A session's plan: its tasks in order and its revision.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanView {
    pub revision: u64,
    /// Live tasks in display order; archived tasks too when requested.
    pub tasks: Vec<PlanTaskView>,
    pub counts: PlanCounts,
    /// Some archived tasks were left out of `tasks` to stay within bounds.
    #[serde(default)]
    pub truncated: bool,
}

/// A task named by an operation: an existing task's id, or the `ref` of a task created
/// earlier in the same batch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TaskRef {
    Id(u64),
    New(String),
}

/// One mutation of a plan. A batch is applied atomically: any rejected operation changes
/// nothing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum PlanOp {
    Create {
        /// A name later operations in the same batch can use for this task.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        r#ref: Option<String>,
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        criteria: Option<String>,
        #[serde(default, alias = "depends_on", skip_serializing_if = "Vec::is_empty")]
        depends_on: Vec<TaskRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<TaskRef>,
        /// `pending` unless given; a task is never created finished.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<TaskStatus>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner: Option<String>,
    },
    Update {
        task: TaskRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        criteria: Option<String>,
        /// Replaces the dependency list.
        #[serde(default, alias = "depends_on", skip_serializing_if = "Option::is_none")]
        depends_on: Option<Vec<TaskRef>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<TaskRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<TaskStatus>,
        /// Required when skipping, cancelling or reopening.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner: Option<String>,
        #[serde(default, alias = "add_evidence", skip_serializing_if = "Vec::is_empty")]
        add_evidence: Vec<TaskEvidence>,
        #[serde(
            default,
            alias = "clear_evidence",
            skip_serializing_if = "std::ops::Not::not"
        )]
        clear_evidence: bool,
    },
    /// Move a live task after another live task, or to the front.
    Reorder {
        task: TaskRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<TaskRef>,
    },
    /// Archive a finished task. It stays inspectable and still satisfies dependents.
    Archive { task: TaskRef },
    /// Archive every finished live task.
    ArchiveFinished,
}

/// What an accepted batch did. Recorded with the mutation so a replayed batch returns the
/// same receipt.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanReceipt {
    /// The plan revision after the batch.
    pub revision: u64,
    /// Ids of created tasks by their `ref` (or `#<op index>` when the operation had none).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub created: std::collections::BTreeMap<String, u64>,
    /// Every task the batch created or changed, in id order.
    #[serde(default)]
    pub changed: Vec<u64>,
}

/// Why a batch was rejected. Nothing was changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanRejected {
    /// `conflict` for a revision mismatch, `invalid` otherwise.
    pub code: crate::ErrorCode,
    /// The offending operation's index, when one operation is to blame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<usize>,
    pub message: String,
    /// The plan revision at the time of the rejection.
    pub revision: u64,
}

// --------------------------------------------------------------------------- web sources

/// Which settings file an MCP edit goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum McpScope {
    User,
    Project,
}

/// One configured MCP server as a client sees it. `command` is the executable and its
/// arguments, or the URL; `env` lists variable or header names with their reference
/// (`$NAME`, `secret:key`) or `…` for an inline literal, never a resolved value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerView {
    pub name: String,
    /// `user` or `project`.
    pub scope: String,
    /// `stdio` or `http`.
    pub kind: String,
    pub enabled: bool,
    /// `stopped`, `connecting`, `ready`, `authRequired` or `failed`.
    pub state: String,
    pub error: Option<String>,
    /// Number of tools in the cached catalog, when it has been read.
    pub tools: Option<usize>,
    /// The cached catalog (names and first lines), with enablement.
    #[serde(default)]
    pub catalog: Vec<McpToolView>,
    #[serde(default)]
    pub disabled_tools: Vec<String>,
    /// Hash of the launch-relevant configuration.
    pub revision: String,
    pub command: String,
    pub env: Vec<String>,
    pub keep_alive: bool,
    pub idle_seconds: u64,
    pub in_flight: usize,
    pub auth: McpAuthView,
    /// The settings entry with literals redacted; what an edit starts from.
    #[serde(default)]
    pub spec: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolView {
    pub name: String,
    pub description: String,
    pub enabled: bool,
}

/// Sign-in state of an HTTP server. The URL is volatile presentation state.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpAuthView {
    /// HTTP servers can sign in; stdio servers cannot.
    pub supported: bool,
    /// Saved tokens exist (metadata only).
    pub signed_in: bool,
    /// The server refused the last connection for lack of a token.
    pub required: bool,
    pub status: AuthStatus,
    pub message: String,
    pub url: Option<String>,
    pub prompt: Option<McpPromptView>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpPromptView {
    pub id: String,
    /// `text`, `secret`, `select` or `manual_code`.
    pub kind: String,
    pub message: String,
    pub placeholder: Option<String>,
    #[serde(default)]
    pub options: Vec<(String, String)>,
}

/// The configured servers plus configuration warnings (invalid entries left out).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServersView {
    pub servers: Vec<McpServerView>,
    pub warnings: Vec<String>,
    pub call_timeout_seconds: u64,
    /// The session's project can hold its own entries (a session was given).
    #[serde(default)]
    pub project_available: bool,
}

/// The outcome of connecting afresh.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpTestView {
    pub name: String,
    pub ok: bool,
    pub message: String,
    #[serde(default)]
    pub tools: Vec<String>,
    pub server_name: Option<String>,
    pub server_version: Option<String>,
    pub protocol_version: Option<String>,
    pub latency_ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpImportEntryView {
    pub name: String,
    pub kind: String,
    /// The converted entry with literals redacted.
    pub spec: serde_json::Value,
    pub warnings: Vec<String>,
    /// A server of this name is already configured (applying replaces it).
    pub exists: bool,
    /// The entry is valid and can be applied.
    pub valid: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpImportView {
    pub entries: Vec<McpImportEntryView>,
    pub warnings: Vec<String>,
}

/// A retained web source: a search result or a fetched page. Content is read through the
/// `web_read` tool; this view carries provenance only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSourceView {
    /// `s<n>`, stable within the session.
    pub id: String,
    /// `search` or `fetch`.
    pub kind: String,
    pub provider: String,
    pub requested_url: String,
    pub final_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// As the provider or page supplied it, verbatim; never derived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
    pub retrieved_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    pub bytes: u64,
    pub text_chars: u64,
    /// The content is no longer retained; the record remains for citation.
    pub expired: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
}

// ---------------------------------------------------------------------------------- goals

/// The state of a session's goal. `completed` and `failed` come from the executor's own
/// report (a status marker) and are not independently verified in this version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GoalState {
    /// Scheduling continuations.
    Active,
    /// A completion proposal is being audited by an independent child; no continuation.
    Verifying,
    /// Not scheduling; the reason says why (Stop, the person, a restart, an ambiguous reply).
    Paused,
    /// The executor reported it needs the person's input.
    Blocked,
    /// The executor reported the goal met.
    Completed,
    /// The executor reported the goal unreachable, or a turn failed.
    Failed,
    /// Archived or replaced by the person.
    Cancelled,
}

impl GoalState {
    pub fn as_str(self) -> &'static str {
        match self {
            GoalState::Active => "active",
            GoalState::Verifying => "verifying",
            GoalState::Paused => "paused",
            GoalState::Blocked => "blocked",
            GoalState::Completed => "completed",
            GoalState::Failed => "failed",
            GoalState::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<GoalState> {
        Some(match s {
            "active" => GoalState::Active,
            "verifying" => GoalState::Verifying,
            "paused" => GoalState::Paused,
            "blocked" => GoalState::Blocked,
            "completed" => GoalState::Completed,
            "failed" => GoalState::Failed,
            "cancelled" => GoalState::Cancelled,
            _ => return None,
        })
    }

    /// Resume makes sense from these states.
    pub fn resumable(self) -> bool {
        matches!(self, GoalState::Paused | GoalState::Blocked)
    }
}

/// The engine-owned goal of a session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalView {
    /// Stable within the session; never reused.
    pub id: u64,
    pub objective: String,
    /// Bumped by every revision of the objective.
    pub revision: u64,
    pub state: GoalState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Goal turns started so far (persisted, unlike the desktop runner's counter).
    pub turns: u64,
    /// Bumped per scheduled continuation; a request key names its epoch.
    pub epoch: u64,
    /// `user`, or `migrated:desktop` for a goal imported from the desktop's records.
    pub provenance: String,
    /// The request key of the latest goal turn, if one was started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_request: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    /// Completion criteria with their evidence.
    #[serde(default)]
    pub criteria: Vec<GoalCriterion>,
    #[serde(default)]
    pub limits: GoalLimits,
    /// Tokens (input, output and cache) reported for this goal's turns so far.
    #[serde(default)]
    pub usage_tokens: u64,
    /// Consecutive continued turns with no change to the progress signature.
    #[serde(default)]
    pub stall_turns: u32,
    /// The executor's latest progress note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Plan task ids the executor linked to this goal.
    #[serde(default)]
    pub linked_tasks: Vec<u64>,
    /// The structured decision of the latest turn, if the executor made one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<GoalDecision>,
    /// The audit policy and the latest audit.
    #[serde(default)]
    pub audit: GoalAudit,
}

/// One criterion's finding from the auditor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditFinding {
    pub criterion: u64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The outcome of one audit round.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalAuditResult {
    pub round: u32,
    /// `pending`, `approved`, `changes_requested`, `inconclusive`, `stale`, `skipped`,
    /// `stopped`, `failed`.
    pub verdict: String,
    #[serde(default)]
    pub findings: Vec<AuditFinding>,
    /// The auditor's own words, bounded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Fingerprint of the file evidence and plan the proposal stood on when the audit began.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    /// The auditor child (subagent directory number), when one ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child: Option<u64>,
}

/// Audit policy and history on a goal.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalAudit {
    /// An accepted proposal must pass an independent audit before completion.
    #[serde(default)]
    pub required: bool,
    /// Audit rounds run so far on the current objective revision.
    #[serde(default)]
    pub rounds: u32,
    /// Rounds allowed before the goal pauses for the person (default 2).
    #[serde(default)]
    pub max_rounds: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<GoalAuditResult>,
}

/// A person's control over the current goal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum GoalAction {
    Pause,
    Resume,
    Revise {
        objective: String,
    },
    /// Keep the record in history; no further continuation.
    Archive,
    /// Set or clear allowances (`None` clears).
    SetLimits {
        #[serde(default)]
        max_turns: Option<u64>,
        #[serde(default)]
        max_tokens: Option<u64>,
    },
    /// Replace the completion criteria (statuses start open; prior evidence is kept in the
    /// event log).
    SetCriteria {
        criteria: Vec<String>,
    },
    /// Require (or stop requiring) an independent audit of completion proposals.
    SetAudit {
        required: bool,
        #[serde(default)]
        max_rounds: Option<u32>,
    },
    /// Complete a goal that is verifying or waiting on audit findings without the audit; the
    /// record says "unreviewed", never "approved".
    SkipAudit,
}

/// The status of one completion criterion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CriterionStatus {
    /// Not yet claimed.
    Open,
    /// Claimed met, with at least one accepted evidence item.
    Met,
    /// Reported as not met.
    Unmet,
    /// Claimed but no evidence was accepted, or the basis changed since (revision, file).
    Unverified,
}

impl CriterionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            CriterionStatus::Open => "open",
            CriterionStatus::Met => "met",
            CriterionStatus::Unmet => "unmet",
            CriterionStatus::Unverified => "unverified",
        }
    }
}

/// Evidence attached to a criterion. `accepted` is the engine's deterministic check where
/// one exists (a tool call that exists and did not fail, a file whose hash matches);
/// `judgment` marks an observation the engine cannot check, which is never counted as
/// mechanical proof.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalEvidence {
    /// `toolCall`, `command`, `file`, `observation`.
    pub kind: String,
    /// A tool call id, a path relative to the session's cwd, or the observation text.
    pub reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// For `file`: the content hash the claim is about; checked again on proposal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub judgment: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalCriterion {
    pub id: u64,
    pub text: String,
    pub status: CriterionStatus,
    #[serde(default)]
    pub evidence: Vec<GoalEvidence>,
    /// The goal revision the status was last judged against.
    #[serde(default)]
    pub judged_at_revision: u64,
}

/// Person-set allowances. The engine checks them before scheduling a turn, so the last turn
/// may still overshoot a token allowance by one turn's usage; it never invents them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
}

/// The executor's structured report for a turn (`update_goal` / `propose_goal_completion`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalDecision {
    /// The goal epoch (turn) it was made in.
    pub epoch: u64,
    /// `continue`, `met`, `blocked`, `unreachable`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}
