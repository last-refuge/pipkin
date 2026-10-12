//! The contract between the Pipkin desktop and its built-in engine.
//!
//! Everything here is plain data: serde types, hard limits and a length-prefixed frame codec.
//! The same types are used by the in-process host (direct channels) and by the private worker
//! transport (inherited pipes), so the topology can change without changing the contract.
//!
//! Versioning: [`PROTOCOL_VERSION`] changes whenever a message changes shape incompatibly. The
//! engine refuses a client with a different protocol in [`api::Call::Hello`]; it never guesses.

pub mod api;
pub mod content;
pub mod frame;
pub mod limits;
pub mod view;

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
pub const PROTOCOL_VERSION: u32 = 20;

pub mod delivery;
