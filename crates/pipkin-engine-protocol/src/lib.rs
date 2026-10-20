//! The contract between the Pipkin desktop and its built-in engine.
//!
//! Everything here is plain data: serde types, hard limits and a length-prefixed frame codec.
//! The same types are used by the in-process host (direct channels) and by the private worker
//! transport (inherited pipes), so the topology can change without changing the contract.
//!
//! Versioning: [`PROTOCOL_VERSION`] changes whenever a message changes shape incompatibly. The
//! engine refuses a client with a different protocol in [`api::Call::Hello`]; it never guesses.

pub mod activity;
pub mod api;
pub mod content;
pub mod frame;
pub mod limits;
pub mod view;

pub use activity::*;
pub use api::*;
pub use content::*;
pub use view::*;

/// Wire protocol version. Bump on any incompatible change to [`api`], [`content`] or [`view`].
// Version 12 carries native-history provenance in snapshots and history pages.
// Version 13 adds plans: `plan` in snapshots, `PlanChanged`, `Plan`/`UpdatePlan` calls.
// Version 14 adds `WebSources` (retained web sources) and the `web` capability.
// Version 15 carries the launch profile on subagent rows.
// Version 16 adds engine-owned goals: `goal` in snapshots, `GoalChanged`, the goal calls.
// Version 17 adds goal criteria, evidence, limits and structured decisions.
// Version 18 adds the verifying state and the goal audit record.
// Version 19 adds `McpServers` (configured MCP servers and their state) and the `mcp` capability.
// Version 20 adds MCP management: server edits, tests, sign-in, import, `McpChanged`.
// Version 21 adds MCP resources, prompts and declared capabilities to the server view.
// Version 22 adds background jobs: `jobs` in snapshots, `JobsChanged`, the job calls.
// Version 23 adds `context` (current context occupancy) to snapshots and `ContextChanged`.
// Version 24 adds subagent controls: `StopSubagent`, `SteerSubagent`, `LaunchSubagent`,
// `SubagentResult`, and the origin/background/delivery/note fields on `SubagentRow`.
// Version 25 adds questionnaires: `QuestionKind::Form`, `QuestionView::form` / `source`,
// `Answer::Form`.
// Version 26 adds engine diagnostics: `Diagnostics`, `ExportDiagnostics`, `DiagnosticsView`.
// Version 27 adds the trace exporter: `ExportHealth`, `ExportChanged`.
// Version 28 adds export configuration: `ExportConfigure`, `ExportSetCredential`,
// `ExportClearCredential`, `ExportPreview`, and the settings fields on `ExportHealthView`.
// Version 29 adds `webSources` to snapshots and `WebSourcesChanged`.
// Version 30 adds goal cost limits (`maxCostUsd`, `usageCostUsd`); 31 adds bounded subagent
// results with artifact references (`SubagentResultView.artifacts`, `resultChars`).
// Version 32 carries executionBackend in session summaries. Old peers must not
// interpret a disabled external-agent conversation as a native provider session.
// Version 33 adds inert external transcript entries, deliberately separate from native messages.
// Version 34 adds explicit, metadata-only local CLI setup and immutable creation calls.
// Version 36 adds inert CLI-reported action blocks, never native tool calls.
// Version 37 captures immutable CLI setup mode. 38 adds owner-scoped external approvals.
// Version 42 adds the store-only activity directory for the orchestration dashboard:
// `Activity`, `ActivityRows`, `ObserveHistory`, `ObserveChildren`, the guarded `StopRun` and
// `FollowUpSubagent`, gated by the capabilities in [`activity`].
pub const PROTOCOL_VERSION: u32 = 42;

mod cli_approvals;
pub use cli_approvals::*;

pub mod delivery;
pub mod execution;
pub use execution::{
    ExecutionBackend, ExternalActionState, ExternalBlock, ExternalRole, LocalCliMode,
    LocalCliModel, LocalCliSessionView, LocalCliSetupStage, LocalCliSetupTarget, LocalCliSetupView,
};
