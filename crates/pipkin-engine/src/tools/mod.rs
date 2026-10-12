//! Coding tools and the shared tool-call machinery.
//!
//! Baseline (reference durable engine): `read`, `write`, `edit`, `bash` (this module) and
//! `subagent` (implemented by the session runtime through [`ToolHost`]). `grep`/`find`/`ls`
//! exist only in the reference's classic agent and are not part of the shipped baseline.
//!
//! Every call goes through [`run`]: argument repair (`prepare_arguments`), schema validation,
//! execution, output bounding (50 KiB / 2000 lines, head or tail), and the `<harness>`
//! diagnostics block appended to the result, matching the reference result text.
//!
//! Streaming output: inside [`run`], [`ToolCtx::output`] (the call's `progress` callback) is the
//! tool's output sink. `run` keeps the bounded window the model will see plus a larger retained
//! copy (up to [`MAX_RETAINED_OUTPUT_BYTES`]), and forwards the current window text (sanitized,
//! the whole window each time) to the session's `progress` callback: the first change after a
//! quiet period at once, then at most every 100 ms and no faster than ~100 KiB/s of new text.

pub mod ask;
pub mod bash;
pub mod diff;
pub mod edit;
mod files;
pub mod goal;
pub mod path;
pub mod plan;
pub mod read;
pub mod schema;
pub mod text;
pub mod web;
pub mod write;

#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pipkin_engine_protocol::Block;
use pipkin_engine_protocol::limits::MAX_RETAINED_OUTPUT_BYTES;
use serde_json::Value;
use tokio::sync::{Notify, Semaphore};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::provider::{BoxFuture, ToolSpec};
use text::{OutputBuffer, bound_output};

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

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warn => "warn",
            Severity::Error => "error",
        }
    }
}

/// A remark attached to a tool result (`[warn] ...` in the `<harness>` block). An empty `code`
/// means the reference left the code out.
#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: String,
    pub message: String,
}

impl Diagnostic {
    pub fn new(severity: Severity, code: impl Into<String>, message: impl Into<String>) -> Self {
        Diagnostic {
            severity,
            code: code.into(),
            message: message.into(),
        }
    }

    /// `{severity, code?, message}` as the reference stores it.
    pub fn to_json(&self) -> Value {
        let mut value = serde_json::json!({ "severity": self.severity.as_str() });
        if !self.code.is_empty() {
            value["code"] = Value::String(self.code.clone());
        }
        value["message"] = Value::String(self.message.clone());
        value
    }
}

/// The `<harness>` block appended to a result: one `[severity] message` line per diagnostic.
pub fn render_diagnostics(diagnostics: &[Diagnostic]) -> String {
    let lines: Vec<String> = diagnostics
        .iter()
        .map(|d| format!("[{}] {}", d.severity.as_str(), d.message))
        .collect();
    format!("<harness>\n{}\n</harness>", lines.join("\n"))
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

/// Engine services a tool may need (durable progress, subagents, questions).
pub trait ToolHost: Send + Sync {
    /// Commit the bounded output window before it is published. Stateless hosts may omit it.
    fn checkpoint_output(
        &self,
        _call_id: &str,
        _output: &text::BoundedOutput,
    ) -> Result<(), String> {
        Ok(())
    }

    /// Run (or resume) the subagent child for `call_id` and return its answer text.
    fn subagent<'a>(
        &'a self,
        call_id: &'a str,
        launch: SubagentLaunch,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<(String, Value), String>>;

    /// Ask the person `question` on behalf of the call and wait for what they submit. The
    /// default host has no presentation to ask.
    fn ask<'a>(
        &'a self,
        _call_id: &'a str,
        _question: crate::extensions::questions::Question,
        _cancel: CancellationToken,
    ) -> BoxFuture<'a, ask::AskOutcome> {
        Box::pin(async {
            ask::AskOutcome::Unavailable("no presentation can answer questions".into())
        })
    }

    /// Apply a plan batch under the call's idempotency key.
    fn plan_update<'a>(
        &'a self,
        _call_id: &'a str,
        _ops: Vec<pipkin_engine_protocol::PlanOp>,
        _expected_revision: Option<u64>,
    ) -> BoxFuture<
        'a,
        Result<
            Result<pipkin_engine_protocol::PlanReceipt, pipkin_engine_protocol::PlanRejected>,
            String,
        >,
    > {
        Box::pin(async { Err("No plan service is configured".into()) })
    }

    /// The session's plan.
    fn plan_read<'a>(
        &'a self,
        _include_archived: bool,
    ) -> BoxFuture<'a, Result<pipkin_engine_protocol::PlanView, String>> {
        Box::pin(async { Err("No plan service is configured".into()) })
    }

    /// Search the web and record the results as sources of this call.
    #[allow(clippy::type_complexity)]
    fn web_search<'a>(
        &'a self,
        _call_id: &'a str,
        _query: &'a str,
        _count: usize,
        _cancel: CancellationToken,
    ) -> BoxFuture<
        'a,
        Result<
            Result<(String, Vec<crate::web::SourceRecord>), crate::web::providers::SearchError>,
            String,
        >,
    > {
        Box::pin(async { Err("No web service is configured".into()) })
    }

    /// Fetch a page within the network boundary and retain it as a source of this call.
    #[allow(clippy::type_complexity)]
    fn web_fetch<'a>(
        &'a self,
        _call_id: &'a str,
        _url: &'a str,
        _cancel: CancellationToken,
    ) -> BoxFuture<
        'a,
        Result<
            Result<(crate::web::SourceRecord, crate::web::Fetched), crate::web::FetchError>,
            String,
        >,
    > {
        Box::pin(async { Err("No web service is configured".into()) })
    }

    /// The conversation's current goal, if any.
    fn goal_get<'a>(
        &'a self,
    ) -> BoxFuture<'a, Result<Option<pipkin_engine_protocol::GoalView>, String>> {
        Box::pin(async { Err("No goal service is configured".into()) })
    }

    /// Record a structured progress report; `Err(reason)` when the report is refused.
    #[allow(clippy::type_complexity)]
    fn goal_update<'a>(
        &'a self,
        _call_id: &'a str,
        _request: goal::GoalUpdateRequest,
    ) -> BoxFuture<'a, Result<Result<pipkin_engine_protocol::GoalView, String>, String>> {
        Box::pin(async { Err("No goal service is configured".into()) })
    }

    /// Propose completion; `Err((problems, goal))` when it is not accepted.
    #[allow(clippy::type_complexity)]
    fn goal_propose<'a>(
        &'a self,
        _call_id: &'a str,
        _proposal: goal::GoalProposal,
    ) -> BoxFuture<
        'a,
        Result<
            Result<
                pipkin_engine_protocol::GoalView,
                (Vec<String>, Option<pipkin_engine_protocol::GoalView>),
            >,
            String,
        >,
    > {
        Box::pin(async { Err("No goal service is configured".into()) })
    }

    /// A retained source and its text (None when unknown or expired).
    #[allow(clippy::type_complexity)]
    fn web_source<'a>(
        &'a self,
        _id: u64,
    ) -> BoxFuture<'a, Result<(Option<crate::web::SourceRecord>, Option<String>), String>> {
        Box::pin(async { Err("No web service is configured".into()) })
    }
}

/// What a `subagent` call asks for: the task and, optionally, a profile and context mode.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubagentLaunch {
    pub task: String,
    pub profile: Option<String>,
    pub context: Option<crate::profiles::ContextMode>,
}

/// Serializes writes/edits to the same canonical path across sessions in this process.
#[derive(Default)]
pub struct MutationLocks {
    locks: std::sync::Mutex<std::collections::HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
}

impl MutationLocks {
    pub fn lock_for(&self, path: PathBuf) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.locks.lock().unwrap_or_else(|e| e.into_inner());
        // Forget locks nobody holds or waits for, so the map stays small.
        if locks.len() >= 256 {
            locks.retain(|_, lock| Arc::strong_count(lock) > 1);
        }
        locks.entry(path).or_default().clone()
    }
}

/// Everything a call needs, owned by the session runtime.
#[derive(Clone)]
pub struct ToolCtx {
    pub cwd: PathBuf,
    pub session: String,
    pub call_id: String,
    pub cancel: CancellationToken,
    /// Live (volatile) output progress for presentation. The session's callback receives the
    /// call's current bounded output window (see the module docs); inside [`run`] the tool's
    /// copy of this field is its output sink, so tools write output with [`ToolCtx::output`].
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

impl ToolCtx {
    /// Append a chunk to the call's streamed output (`api.output`).
    pub fn output(&self, chunk: &str) {
        (self.progress)(chunk)
    }
}

pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    /// Rerun after an interruption only when both the recorded and the current tool say so.
    fn replay_safe(&self) -> bool {
        false
    }
    /// One sequential tool makes its whole round sequential (reference `executionMode`). The
    /// built-in tools run in parallel, as in the reference.
    fn sequential(&self) -> bool {
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

/// A [`ToolSpec`] from the reference's literal JSON schema.
pub(crate) fn spec_from_json(name: &str, description: &str, schema: &str) -> ToolSpec {
    ToolSpec {
        constrained_sampling: None,
        name: name.to_owned(),
        description: description.to_owned(),
        parameters: serde_json::from_str(schema).unwrap_or(Value::Null),
    }
}

/// The built-in coding tools in reference order (`read`, `write`, `edit`, `bash`).
pub fn builtin_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(read::ReadTool),
        Arc::new(write::WriteTool),
        Arc::new(edit::EditTool),
        Arc::new(bash::BashTool),
    ]
}

/// Minimum pause between progress publications.
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// Each publication also buys a pause proportional to the new text it carried.
pub const PROGRESS_BYTES_PER_SECOND: u64 = 100 * 1024;

/// The streamed output of one call: the model window and the engine's retained copy.
struct Stream {
    window: OutputBuffer,
    retained: OutputBuffer,
    dirty: bool,
}

fn retained_limits(retain: Retain) -> OutputLimits {
    OutputLimits {
        max_bytes: MAX_RETAINED_OUTPUT_BYTES,
        max_lines: usize::MAX / 4,
        retain,
    }
}

/// Publish the window to the session's progress callback, throttled.
async fn publish_progress(stream: &Mutex<Stream>, notify: &Notify, ctx: &ToolCtx) {
    let mut next_at = Instant::now();
    let mut written = String::new();
    loop {
        notify.notified().await;
        if next_at > Instant::now() {
            tokio::time::sleep_until(next_at).await;
        }
        let output = {
            let mut s = stream.lock().unwrap_or_else(|e| e.into_inner());
            if !s.dirty {
                continue;
            }
            s.dirty = false;
            s.window.snapshot()
        };
        if let Some(host) = &ctx.host
            && host.checkpoint_output(&ctx.call_id, &output).is_err()
        {
            ctx.cancel.cancel();
            continue;
        }
        let text = output.text;
        let fresh = if text.starts_with(&written) {
            text.len() - written.len()
        } else {
            text.len()
        };
        let started = Instant::now();
        (ctx.progress)(&text);
        written = text;
        let pace = Duration::from_millis(fresh as u64 * 1000 / PROGRESS_BYTES_PER_SECOND);
        next_at = started + PROGRESS_INTERVAL.max(pace);
    }
}

/// The harness's truncation diagnostic; `retain` is unknown when rebuilt after an abort.
fn truncated(dropped_lines: u64, dropped_bytes: u64, retain: Option<Retain>) -> Diagnostic {
    let kept = match retain {
        None => "",
        Some(Retain::Head) => " to its beginning",
        Some(Retain::Tail) => " to its end",
    };
    Diagnostic::new(
        Severity::Warn,
        "truncated",
        format!("Output truncated{kept}: {dropped_lines} lines, {dropped_bytes} bytes dropped"),
    )
}

/// Bound the text of result content: when the joined text exceeds the limits, the text items
/// are replaced by one bounded item at the position of the first (head) or last (tail) text
/// item; other content is kept. Returns the dropped lines and bytes.
fn bound_content(content: Vec<Block>, limits: OutputLimits) -> (Vec<Block>, u64, u64) {
    let joined: String = content
        .iter()
        .filter_map(|b| match b {
            Block::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let bounded = bound_output(&joined, limits);
    if bounded.dropped_bytes == 0 {
        return (content, 0, 0);
    }
    let text_positions: Vec<usize> = content
        .iter()
        .enumerate()
        .filter(|(_, b)| matches!(b, Block::Text { .. }))
        .map(|(i, _)| i)
        .collect();
    let keep = match limits.retain {
        Retain::Head => text_positions.first().copied(),
        Retain::Tail => text_positions.last().copied(),
    };
    let mut result = Vec::with_capacity(content.len());
    let mut replacement = Some(bounded.text);
    for (i, block) in content.into_iter().enumerate() {
        match block {
            Block::Text { extra, .. } if Some(i) == keep => result.push(Block::Text {
                text: replacement.take().unwrap_or_default(),
                extra,
            }),
            Block::Text { .. } => {}
            other => result.push(other),
        }
    }
    (
        result,
        bounded.dropped_lines as u64,
        bounded.dropped_bytes as u64,
    )
}

fn joined_text(content: &[Block]) -> String {
    content
        .iter()
        .filter_map(|b| match b {
            Block::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// Settle a result: the content plus the rendered diagnostics block; `retained` is the full
/// output text the engine kept, or (when there was none) the diagnostics' messages.
fn settle(
    mut content: Vec<Block>,
    is_error: bool,
    details: Option<Value>,
    diagnostics: &[Diagnostic],
    retained: Option<(String, u64)>,
) -> ToolResult {
    let (mut retained, mut dropped_bytes) = retained.unwrap_or_else(|| (joined_text(&content), 0));
    if retained.len() > MAX_RETAINED_OUTPUT_BYTES {
        let kept = crate::util::clip(&retained, MAX_RETAINED_OUTPUT_BYTES).len();
        dropped_bytes += (retained.len() - kept) as u64;
        retained.truncate(kept);
    }
    if retained.is_empty() {
        retained = diagnostics
            .iter()
            .map(|d| d.message.as_str())
            .collect::<Vec<_>>()
            .join("\n");
    }
    if !diagnostics.is_empty() {
        content.push(Block::text(render_diagnostics(diagnostics)));
    }
    ToolResult {
        content,
        is_error,
        details,
        retained,
        dropped_bytes,
    }
}

/// An error result the harness writes itself: no content and one `error` diagnostic.
fn harness_error(code: &str, message: String) -> ToolResult {
    settle(
        vec![],
        true,
        None,
        &[Diagnostic::new(Severity::Error, code, message)],
        None,
    )
}

/// Run one call through validation, execution, bounding and diagnostics.
pub async fn run(tool: &dyn Tool, args: Value, ctx: &ToolCtx) -> ToolResult {
    let spec = tool.spec();
    let prepared = tool.prepare_arguments(args);
    let args = match schema::validate(&spec.name, &spec.parameters, &prepared) {
        Ok(args) => args,
        Err(message) => return harness_error("invalid_arguments", message),
    };
    if ctx.cancel.is_cancelled() {
        return aborted(&spec.name, "");
    }
    let limits = tool.limits();
    let stream = Arc::new(Mutex::new(Stream {
        window: OutputBuffer::new(limits),
        retained: OutputBuffer::new(retained_limits(limits.retain)),
        dirty: false,
    }));
    let notify = Arc::new(Notify::new());
    let sink = {
        let stream = stream.clone();
        let notify = notify.clone();
        move |chunk: &str| {
            if chunk.is_empty() {
                return;
            }
            let mut s = stream.lock().unwrap_or_else(|e| e.into_inner());
            s.window.push(chunk);
            s.retained.push(chunk);
            s.dirty = true;
            drop(s);
            notify.notify_one();
        }
    };
    let call_ctx = ToolCtx {
        progress: Arc::new(sink),
        ..ctx.clone()
    };
    let outcome = {
        let execution = tool.execute(args, &call_ctx);
        let publisher = publish_progress(&stream, &notify, ctx);
        tokio::pin!(execution, publisher);
        tokio::select! {
            biased;
            outcome = &mut execution => outcome,
            () = &mut publisher => unreachable!("progress publishing never ends"),
        }
    };
    let (window, full) = {
        let mut s = stream.lock().unwrap_or_else(|e| e.into_inner());
        (s.window.snapshot(), s.retained.snapshot())
    };
    let streamed = || -> Vec<Block> {
        if window.text.is_empty() {
            vec![]
        } else {
            vec![Block::text(window.text.clone())]
        }
    };
    let streamed_retained = || Some((full.text.clone(), full.dropped_bytes));

    match outcome {
        Ok(output) => {
            let mut diagnostics = output.diagnostics;
            let (content, retained) = match output.content {
                Some(content) => {
                    let text = joined_text(&content);
                    (content, Some((text, 0)))
                }
                None => {
                    if window.dropped_bytes > 0 {
                        diagnostics.push(truncated(
                            window.dropped_lines,
                            window.dropped_bytes,
                            Some(limits.retain),
                        ));
                    }
                    (streamed(), streamed_retained())
                }
            };
            let (content, lines, bytes) = bound_content(content, limits);
            if bytes > 0 {
                diagnostics.push(truncated(lines, bytes, Some(limits.retain)));
            }
            settle(
                content,
                output.is_error,
                output.details,
                &diagnostics,
                retained,
            )
        }
        Err(error) if ctx.cancel.is_cancelled() => {
            // The abort path: the partial output, what the tool reported, then the abort.
            let mut diagnostics = error.diagnostics;
            if window.dropped_bytes > 0 {
                diagnostics.push(truncated(window.dropped_lines, window.dropped_bytes, None));
            }
            diagnostics.push(Diagnostic::new(
                Severity::Error,
                "aborted",
                format!("Tool {} was aborted", spec.name),
            ));
            settle(
                streamed(),
                true,
                error.details,
                &diagnostics,
                streamed_retained(),
            )
        }
        Err(error) => {
            let mut diagnostics = error.diagnostics;
            diagnostics.push(Diagnostic::new(
                Severity::Error,
                "tool_error",
                error.message,
            ));
            if window.dropped_bytes > 0 {
                diagnostics.push(truncated(
                    window.dropped_lines,
                    window.dropped_bytes,
                    Some(limits.retain),
                ));
            }
            let (content, lines, bytes) = bound_content(streamed(), limits);
            if bytes > 0 {
                diagnostics.push(truncated(lines, bytes, Some(limits.retain)));
            }
            settle(
                content,
                true,
                error.details,
                &diagnostics,
                streamed_retained(),
            )
        }
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

/// Rebuild a result from the last durable model-visible window and its truncation counts.
pub fn recovered_output(name: &str, output: &text::BoundedOutput, aborted: bool) -> ToolResult {
    let mut result = if aborted {
        self::aborted(name, &output.text)
    } else {
        interrupted(name, &output.text)
    };
    if output.dropped_bytes > 0 {
        let error = if aborted {
            format!("Tool {name} was aborted")
        } else {
            format!("Tool {name} was interrupted and may have partially run")
        };
        *result.content.last_mut().expect("interruption diagnostic") =
            Block::text(render_diagnostics(&[
                truncated(output.dropped_lines, output.dropped_bytes, None),
                Diagnostic::new(
                    Severity::Error,
                    if aborted { "aborted" } else { "interrupted" },
                    error,
                ),
            ]));
    }
    result.dropped_bytes = output.dropped_bytes;
    result
}
