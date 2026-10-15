use crate::ids::ConversationId;
use crate::model::Connection;
use crate::onboarding::SignInSnapshot;
use crate::protocol::BackendRequest;
use crate::state::Bootstrap;

/// Connection and catalog updates, separate from per-conversation `BackendEvent`s. These are
/// authoritative state and must never be dropped, so the sink behind them is unbounded.
#[derive(Clone, Debug, PartialEq)]
pub enum LifecycleEvent {
    Connection(Connection),
    /// Explicit service capability, not conversation trust or execution readiness.
    LocalCliSetupAvailable(bool),
    CliApprovalsAvailable(bool),
    /// Projects, models and conversation summaries known to the backend.
    Catalog(Bootstrap),
    /// The model the engine reports as selected (`provider/modelId`), which is authoritative.
    ModelSelected(Option<String>),
    /// Volatile, per-client OAuth state. The URL and challenge must never be persisted or logged.
    SignIn(SignInSnapshot),
    SetupSaved(Result<(), String>),
    /// A background action failed and there is no conversation to attach the failure to.
    Notice(String),
    /// Saved state could not be read or written as usual (a failed background write, say).
    StorageIssue(String),
    /// The MCP servers and their state (engine-owned; secrets never included).
    Mcp(crate::mcp::McpSnapshot),
    McpTest(crate::mcp::McpTestResult),
    McpImport(crate::mcp::McpImportPreview),
    /// A management action was refused; shown on the page, not as an app notice.
    McpFailed(String),
    /// A conversation (open or not) has, or no longer has, a question waiting for the
    /// person. Not tied to an attachment generation.
    QuestionWaiting {
        conversation: ConversationId,
        waiting: bool,
    },
    /// The engine's trace exporter state (OBS-2).
    ExportHealth(crate::diagnostics::ExportHealth),
    /// An example export payload at a content level (OBS-3).
    ExportPreview {
        content: String,
        payload: String,
    },
    /// A trace export settings action was refused; shown on the page.
    ExportFailed(String),
}

pub type LifecycleSink = Box<dyn Fn(LifecycleEvent) + Send + Sync>;

/// The narrow backend port. Initialization is asynchronous: `start` returns immediately and the
/// backend reports progress through the sink. Per-conversation events flow back through a
/// channel supplied at construction, never through return values, so slow or lost responses are
/// representable.
pub trait Backend: Send + Sync {
    /// Begin connecting. Must not block; report `Connecting`/`Ready`/`Offline`/... and the
    /// catalog through `sink`, from any thread.
    fn start(&self, sink: LifecycleSink);
    /// Fire a request. Responses arrive as `BackendEvent`s on the adapter's channel.
    fn request(&self, request: BackendRequest);
    /// Orderly stop, called once at application exit. Must not block indefinitely.
    fn shutdown(&self) {}
}
