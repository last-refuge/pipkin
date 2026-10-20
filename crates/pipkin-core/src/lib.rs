//! Framework-independent application state, commands and events.
//!
//! Nothing here knows about GPUI, colors, geometry or transport frames.

pub mod attach;
pub mod backend;
pub mod cli_approval_port;
pub mod cli_approvals;
mod conversations;
pub mod dashboard;
pub mod diagnostics;
mod execution;
pub mod ids;
pub mod import;
pub mod jobs;
pub mod local_cli;
pub mod mcp;
pub mod model;
pub mod onboarding;
pub mod ownership;
pub mod prompt_history;
pub mod protocol;
mod providers;
pub mod state;
pub mod status;
pub mod subagents;

pub use attach::*;
pub use backend::*;
pub use cli_approval_port::*;
pub use cli_approvals::*;
pub use conversations::MAX_CLOSED_CONVERSATIONS;
pub use execution::ExecutionCapabilities;
pub use ids::*;
pub use import::*;
pub use jobs::*;
pub use local_cli::*;
pub use mcp::*;
pub use model::*;
pub use ownership::*;
pub use protocol::*;
pub use providers::*;
pub use state::*;
pub use status::*;
pub use subagents::*;
