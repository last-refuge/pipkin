//! Tool tests. Reference ports are named after the reference test they port
//! (`packages/durable/test/*.test.ts`); the golden JSON files hold outputs captured from the
//! reference implementation itself.

mod ask;
mod bash;
mod edit;
mod output;
mod read;
mod run;
mod schema;
mod write;

use std::path::Path;
use std::sync::{Arc, Mutex};

use pipkin_engine_protocol::Block;
use serde_json::Value;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use super::{MutationLocks, Tool, ToolCtx, ToolError, ToolOutput, ToolResult};

/// A context for `cwd` whose progress callback records what it receives.
pub fn ctx_with(cwd: &Path) -> (ToolCtx, Arc<Mutex<Vec<String>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let ctx = ToolCtx {
        cwd: cwd.to_path_buf(),
        session: "s".into(),
        call_id: "call".into(),
        cancel: CancellationToken::new(),
        progress: Arc::new(move |text: &str| sink.lock().unwrap().push(text.to_owned())),
        env: vec![],
        shell: None,
        locks: Arc::new(MutationLocks::default()),
        processes: Arc::new(Semaphore::new(8)),
        host: None,
    };
    (ctx, seen)
}

pub fn ctx(cwd: &Path) -> ToolCtx {
    ctx_with(cwd).0
}

/// Execute a tool directly (like the reference tests' fake api): returns the outcome and the
/// output chunks it streamed.
pub async fn execute(
    tool: &dyn Tool,
    args: Value,
    ctx: &ToolCtx,
) -> (Result<ToolOutput, ToolError>, String) {
    let chunks = Arc::new(Mutex::new(String::new()));
    let sink = chunks.clone();
    let call = ToolCtx {
        progress: Arc::new(move |text: &str| sink.lock().unwrap().push_str(text)),
        ..ctx.clone()
    };
    let args = tool.prepare_arguments(args);
    let result = tool.execute(args, &call).await;
    let out = chunks.lock().unwrap().clone();
    (result, out)
}

/// Content texts (images as `[image]`) joined with `|`, like the reference's `resultText`.
pub fn result_text(result: &ToolResult) -> String {
    result
        .content
        .iter()
        .map(|b| match b {
            Block::Text { text, .. } => text.clone(),
            Block::Image { .. } => "[image]".into(),
            _ => String::new(),
        })
        .collect::<Vec<_>>()
        .join("|")
}

pub fn output_text(output: &ToolOutput) -> String {
    output
        .content
        .iter()
        .flatten()
        .filter_map(|b| match b {
            Block::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn diagnostic_text(output: &ToolOutput) -> String {
    output
        .diagnostics
        .iter()
        .map(|d| d.message.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn temp() -> tempfile::TempDir {
    tempfile::tempdir().expect("temp dir")
}
