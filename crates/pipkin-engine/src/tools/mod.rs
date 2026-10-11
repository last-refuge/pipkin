//! Coding tools and the shared tool-call machinery.
//!
//! Baseline (reference durable engine): `read`, `write`, `edit`, `bash` (this module) and
//! `subagent` (implemented by the session runtime through [`ToolHost`]). `grep`/`find`/`ls`
//! exist only in the reference's classic agent and are not part of the shipped baseline.
//!
//! Every call goes through [`run`]: argument repair (`prepare_arguments`), schema validation,
//! execution, output bounding (50 KiB / 2000 lines, head or tail), and the `<harness>`
//! diagnostics block appended to the result, matching the reference result text.

use std::path::PathBuf;
use std::sync::Arc;

use pipkin_engine_protocol::Block;
use serde_json::Value;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::provider::{BoxFuture, ToolSpec};

/// Which end of oversized output is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Retain {
    Head,
    Tail,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputLimits {
    pub max_bytes: usize,
    pub max_lines: usize,
    pub retain: Retain,
}

impl Default for OutputLimits {
    fn default() -> Self {
        OutputLimits {
            max_bytes: 51_200,
            max_lines: 2_000,
            retain: Retain::Head,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warn,
    Error,
}

/// A remark attached to a tool result (`[warn] ...` in the `<harness>` block).
#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: String,
    pub message: String,
}

/// What a tool's `execute` returns. `content: None` means "use the streamed output".
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolOutput {
    pub content: Option<Vec<Block>>,
    pub is_error: bool,
    pub details: Option<Value>,
    pub diagnostics: Vec<Diagnostic>,
}

/// A failed execution: the message plus whatever output was produced.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolError {
    pub message: String,
    pub diagnostics: Vec<Diagnostic>,
    pub details: Option<Value>,
}

impl ToolError {
    pub fn new(message: impl Into<String>) -> ToolError {
        ToolError {
            message: message.into(),
            diagnostics: vec![],
            details: None,
        }
    }
}

/// The settled result recorded as a `toolResult` message.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    /// Exactly what the model sees (bounded, with the `<harness>` block).
    pub content: Vec<Block>,
    pub is_error: bool,
    pub details: Option<Value>,
    /// The complete output as retained by the engine (may exceed the model-visible bound).
    pub retained: String,
    /// Output bytes the engine did not retain.
    pub dropped_bytes: u64,
}

/// Engine services a tool may need (subagents, questions).
pub trait ToolHost: Send + Sync {
    /// Run (or resume) the subagent child for `call_id` and return its answer text.
    fn subagent<'a>(
        &'a self,
        call_id: &'a str,
        task: &'a str,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<(String, Value), String>>;
}

/// Serializes writes/edits to the same canonical path across sessions in this process.
#[derive(Default)]
pub struct MutationLocks {
    locks: std::sync::Mutex<std::collections::HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
}

impl MutationLocks {
    pub fn lock_for(&self, path: PathBuf) -> Arc<tokio::sync::Mutex<()>> {
        self.locks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(path)
            .or_default()
            .clone()
    }
}

/// Everything a call needs, owned by the session runtime.
#[derive(Clone)]
pub struct ToolCtx {
    pub cwd: PathBuf,
    pub session: String,
    pub call_id: String,
    pub cancel: CancellationToken,
    /// Live (volatile) output progress for presentation.
    pub progress: Arc<dyn Fn(&str) + Send + Sync>,
    /// Extra environment for child processes (e.g. `PI_SESSION_ID`).
    pub env: Vec<(String, String)>,
    /// Shell override from settings (`shellPath`).
    pub shell: Option<PathBuf>,
    pub locks: Arc<MutationLocks>,
    /// Bounds concurrently running tool processes across the engine.
    pub processes: Arc<Semaphore>,
    pub host: Option<Arc<dyn ToolHost>>,
}

pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    /// Rerun after an interruption only when both the recorded and the current tool say so.
    fn replay_safe(&self) -> bool {
        false
    }
    fn limits(&self) -> OutputLimits {
        OutputLimits::default()
    }
    /// Pure argument repair before validation.
    fn prepare_arguments(&self, args: Value) -> Value {
        args
    }
    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolCtx,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>>;
}

/// The built-in coding tools in reference order (`read`, `write`, `edit`, `bash`).
pub fn builtin_tools() -> Vec<Arc<dyn Tool>> {
    vec![]
}

/// Run one call through validation, execution, bounding and diagnostics.
pub async fn run(tool: &dyn Tool, args: Value, ctx: &ToolCtx) -> ToolResult {
    let _ = (tool, args, ctx);
    ToolResult {
        content: vec![Block::text("Tool machinery is not implemented yet")],
        is_error: true,
        details: None,
        retained: String::new(),
        dropped_bytes: 0,
    }
}

/// The result for an unknown or unoffered tool.
pub fn unavailable(name: &str) -> ToolResult {
    let text = format!("Tool {name} is not available");
    ToolResult {
        content: vec![Block::text(format!(
            "<harness>\n[error] {text}\n</harness>"
        ))],
        is_error: true,
        details: None,
        retained: text,
        dropped_bytes: 0,
    }
}

/// The result recorded when a call's effect is unconfirmed after an interruption.
pub fn interrupted(name: &str, partial: &str) -> ToolResult {
    let mut content = Vec::new();
    if !partial.is_empty() {
        content.push(Block::text(partial));
    }
    content.push(Block::text(format!(
        "<harness>\n[error] Tool {name} was interrupted and may have partially run\n</harness>"
    )));
    ToolResult {
        content,
        is_error: true,
        details: None,
        retained: partial.to_owned(),
        dropped_bytes: 0,
    }
}

/// The result recorded when a call is stopped.
pub fn aborted(name: &str, partial: &str) -> ToolResult {
    let mut content = Vec::new();
    if !partial.is_empty() {
        content.push(Block::text(partial));
    }
    content.push(Block::text(format!(
        "<harness>\n[error] Tool {name} was aborted\n</harness>"
    )));
    ToolResult {
        content,
        is_error: true,
        details: None,
        retained: partial.to_owned(),
        dropped_bytes: 0,
    }
}
