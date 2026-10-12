//! MCP (plan §4, MCP-1): one native service that supervises configured servers over stdio
//! through the official `rmcp` SDK, discovers their tools lazily and keeps the catalog
//! cached, and executes calls with bounded timeouts. The model reaches it through three
//! compact tools (`mcp_search`, `mcp_describe`, `mcp_call`), so the catalog offered to the
//! model never grows with the number of installed servers.
//!
//! Lifecycle: a server starts on first use, stays ready while calls are in flight, and is
//! stopped by the idle reaper after `idleSeconds` without use unless `keepAlive` is set. A
//! failed start is remembered with its reason and retried only after a backoff. A changed
//! configuration revision stops the running server; the next use starts the new one.
//!
//! Uncertain outcomes stay uncertain: `mcp_call` is not replay-safe, so an interrupted call
//! is recorded as interrupted and never re-sent by the engine; the model may call again.
//! Tool annotations are carried as hints and never treated as proof of idempotency.

pub mod config;
pub mod http;
pub mod import;
pub mod oauth;
pub mod service;

pub use config::{McpConfig, McpServerConfig};
pub use service::{
    AuthProgress, CallOutcome, McpError, McpService, PromptView, ServerHandle, ServerSnapshot,
    TestOutcome, ToolInfo,
};
