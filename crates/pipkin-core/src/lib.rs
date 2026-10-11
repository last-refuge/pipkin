//! Framework-independent application state, commands and events.
//!
//! Nothing here knows about GPUI, colors, geometry or transport frames.

pub mod attach;
pub mod backend;
pub mod ids;
pub mod import;
pub mod model;
pub mod onboarding;
pub mod prompt_history;
pub mod protocol;
pub mod state;
pub mod subagents;

pub use attach::*;
pub use backend::*;
pub use ids::*;
pub use import::*;
pub use model::*;
pub use protocol::*;
pub use state::*;
pub use subagents::*;
