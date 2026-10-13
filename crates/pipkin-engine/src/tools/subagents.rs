//! Lookups for background subagents (SUB-3): `subagent_status` lists this session's children,
//! `subagent_result` reads one child's recorded answer, `subagent_wait` blocks (bounded) until
//! a child finishes. All three are pure reads of durable task rows and so replay-safe.

use pipkin_engine_protocol::{Block, SubagentResultView, SubagentRow};
use serde_json::{Value, json};

use super::{Tool, ToolCtx, ToolError, ToolOutput};
use crate::provider::{BoxFuture, ToolSpec};

pub const SUBAGENT_STATUS: &str = "subagent_status";
pub const SUBAGENT_RESULT: &str = "subagent_result";
pub const SUBAGENT_WAIT: &str = "subagent_wait";
pub const RESULT_VERSION: u32 = 1;

fn host(ctx: &ToolCtx) -> Result<&std::sync::Arc<dyn super::ToolHost>, ToolError> {
    ctx.host
        .as_ref()
        .ok_or_else(|| ToolError::new("No execution host is configured"))
}

fn child_arg(args: &Value) -> Result<u64, ToolError> {
    args.get("child")
        .and_then(Value::as_u64)
        .filter(|id| *id > 0)
        .ok_or_else(|| ToolError::new("child must be a positive subagent number"))
}

fn status_word(status: &str) -> &'static str {
    match status {
        "done" => "finished",
        "aborted" => "stopped",
        "failed" => "failed",
        "pending" => "starting",
        _ => "running",
    }
}

pub fn row_line(row: &SubagentRow) -> String {
    let excerpt: String = row
        .task
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(80)
        .collect();
    let mut line = format!("#{} {}: {excerpt}", row.child, status_word(&row.status));
    if row.background {
        line.push_str(" (background");
        if row.origin == "user" {
            line.push_str(", started by the person");
        }
        line.push(')');
    }
    if let Some(note) = &row.note {
        line.push_str(&format!(" — {note}"));
    }
    if row.has_result {
        line.push_str(" — answer available");
    }
    line
}

fn result_text(view: &SubagentResultView) -> String {
    let mut text = format!("Subagent #{} {}.", view.child, status_word(&view.status));
    if let Some(note) = &view.note {
        text.push_str(&format!(" {note}."));
    }
    match (&view.result, view.status.as_str()) {
        (Some(answer), _) if !answer.trim().is_empty() => {
            text.push_str(&format!("\nAnswer:\n{}", answer.trim_end()));
        }
        (_, "done") => text.push_str(" It gave no answer text."),
        (_, "failed") => text.push_str(" No answer was recorded."),
        (_, "aborted") => text.push_str(" No answer will arrive."),
        _ => text.push_str(" No answer yet."),
    }
    text
}

fn details(view: &SubagentResultView) -> Value {
    json!({"subagent": {
        "version": RESULT_VERSION,
        "child": view.child,
        "status": view.status,
        "delivery": view.delivery,
        "origin": view.origin,
        "conversationId": view.session,
    }})
}

pub struct SubagentStatusTool;

impl Tool for SubagentStatusTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            constrained_sampling: None,
            name: SUBAGENT_STATUS.into(),
            description: "List this conversation's subagents (number, state, task) or report one of them. Background children launched with wait false, and children the person started, appear here with whether an answer is available.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "child": {"type": "integer", "description": "One subagent number (optional; default: all)"}
                }
            }),
        }
    }

    fn replay_safe(&self) -> bool {
        true
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolCtx,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let h = host(ctx)?;
            let view = h.subagent_status().await.map_err(ToolError::new)?;
            let rows: Vec<&SubagentRow> = match args.get("child").and_then(Value::as_u64) {
                Some(child) => {
                    let Some(row) = view.children.iter().find(|r| r.child == child) else {
                        return Err(ToolError::new(format!(
                            "No subagent #{child} in this conversation"
                        )));
                    };
                    vec![row]
                }
                None => view.children.iter().collect(),
            };
            let mut text = if rows.is_empty() {
                "No subagents in this conversation.".to_owned()
            } else {
                rows.iter()
                    .map(|r| row_line(r))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            if view.truncated {
                text.push_str("\n(The list is cut at the engine's bound.)");
            }
            Ok(ToolOutput {
                content: Some(vec![Block::text(text)]),
                details: Some(json!({"subagents": {
                    "version": RESULT_VERSION,
                    "children": rows.iter().map(|r| json!({
                        "child": r.child,
                        "status": r.status,
                        "background": r.background,
                        "origin": r.origin,
                        "delivery": r.delivery,
                        "hasResult": r.has_result,
                    })).collect::<Vec<_>>(),
                }})),
                ..ToolOutput::default()
            })
        })
    }
}

pub struct SubagentResultTool;

impl Tool for SubagentResultTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            constrained_sampling: None,
            name: SUBAGENT_RESULT.into(),
            description: "Read a subagent's recorded answer by its number (the full text, also for children that finished in the background).".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "child": {"type": "integer", "description": "The subagent number"}
                },
                "required": ["child"]
            }),
        }
    }

    fn replay_safe(&self) -> bool {
        true
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolCtx,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let child = child_arg(&args)?;
            let view = host(ctx)?
                .subagent_result(child)
                .await
                .map_err(ToolError::new)?;
            Ok(ToolOutput {
                content: Some(vec![Block::text(result_text(&view))]),
                details: Some(details(&view)),
                ..ToolOutput::default()
            })
        })
    }
}

pub struct SubagentWaitTool;

impl Tool for SubagentWaitTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            constrained_sampling: None,
            name: SUBAGENT_WAIT.into(),
            description: "Wait for a background subagent to finish, up to a bounded number of seconds (default 60, at most 600), then report its answer.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "child": {"type": "integer", "description": "The subagent number"},
                    "timeout": {"type": "number", "description": "Seconds to wait (default 60, at most 600)"}
                },
                "required": ["child"]
            }),
        }
    }

    fn replay_safe(&self) -> bool {
        true
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolCtx,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let child = child_arg(&args)?;
            let timeout = args
                .get("timeout")
                .and_then(Value::as_f64)
                .filter(|t| t.is_finite() && *t > 0.0)
                .unwrap_or(60.0)
                .min(crate::runtime::subagents::MAX_WAIT_SECS as f64);
            let view = host(ctx)?
                .subagent_wait(
                    child,
                    std::time::Duration::from_secs_f64(timeout),
                    ctx.cancel.clone(),
                )
                .await
                .map_err(ToolError::new)?;
            let mut text = result_text(&view);
            if !matches!(view.status.as_str(), "done" | "failed" | "aborted") {
                text.push_str(&format!(
                    "\nStill running after {timeout} seconds; wait again or continue with other work."
                ));
            }
            Ok(ToolOutput {
                content: Some(vec![Block::text(text)]),
                details: Some(details(&view)),
                ..ToolOutput::default()
            })
        })
    }
}
