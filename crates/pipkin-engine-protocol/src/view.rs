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
        /// Native child sessions created for subagent conversations.
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
        assert_eq!(crate::PROTOCOL_VERSION, 10);
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
