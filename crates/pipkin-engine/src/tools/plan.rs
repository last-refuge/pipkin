//! `plan_update` and `plan_read`: the model's compact access to the session's plan.
//!
//! `plan_update` applies a batch of operations atomically through [`ToolHost::plan_update`]
//! and reports the tasks it touched plus the counts, not the whole list (the summary section
//! and `plan_read` carry that). It is replay-safe because the engine records the batch's
//! receipt under the call id: a rerun after a restart returns the recorded outcome without
//! applying the batch twice. `plan_read` lists the plan or one task in full.
//!
//! The result contract (`details.plan`, version 1): `{outcome: "applied", revision, created,
//! changed}`, `{outcome: "rejected", code, op?, message, revision}`, or for reads
//! `{outcome: "read", revision, counts}`.

use pipkin_engine_protocol::{Block, PlanOp, PlanView};
use serde_json::{Value, json};

use super::{Tool, ToolCtx, ToolError, ToolOutput};
use crate::plans;
use crate::provider::{BoxFuture, ToolSpec};

pub const PLAN_UPDATE: &str = "plan_update";
pub const PLAN_READ: &str = "plan_read";
pub const RESULT_VERSION: u32 = 1;

pub const UPDATE_DESCRIPTION: &str = "Change the session's plan: a persistent list of tasks with status, dependencies, completion criteria and evidence, shared with the person. Use it to lay out multi-step work and to record progress honestly. A batch applies atomically. Operations: create {ref?, title, detail?, criteria?, depends_on?, parent?, status?, owner?}, update {task, title?, detail?, criteria?, depends_on?, parent?, status?, reason?, owner?, add_evidence?, clear_evidence?}, reorder {task, after?}, archive {task}, archiveFinished. Statuses: pending, active, blocked, completed, skipped, cancelled. A task can be active or completed only when its dependencies are completed or skipped; skipping, cancelling or reopening needs a reason. Attach evidence (tool call ids, commands with exit status, file paths, hashes) when completing. Tasks refer to each other by id (#n) or by the ref of a task created earlier in the same batch.";

pub const READ_DESCRIPTION: &str = "Read the session's plan: every live task on one line, or one task in full (detail, criteria, reason, evidence) when given its id. Pass include_archived to see archived tasks too.";

fn details(fields: Value) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("version".into(), RESULT_VERSION.into());
    if let Value::Object(fields) = fields {
        body.extend(fields);
    }
    json!({ "plan": Value::Object(body) })
}

fn invalid(message: String) -> ToolError {
    ToolError {
        message,
        diagnostics: vec![],
        details: Some(details(json!({ "outcome": "invalid" }))),
    }
}

pub struct PlanUpdateTool;

impl Tool for PlanUpdateTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            constrained_sampling: None,
            name: PLAN_UPDATE.into(),
            description: UPDATE_DESCRIPTION.into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "ops": {
                        "type": "array",
                        "description": "Operations to apply, in order; each has an \"op\" field",
                        "items": {"type": "object"}
                    },
                    "expected_revision": {
                        "type": "integer",
                        "description": "Reject the batch unless the plan is still at this revision"
                    }
                },
                "required": ["ops"]
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
            let ops: Vec<PlanOp> =
                serde_json::from_value(args.get("ops").cloned().unwrap_or(Value::Null))
                    .map_err(|e| invalid(format!("ops: {e}")))?;
            let expected = args.get("expected_revision").and_then(Value::as_u64);
            let Some(host) = &ctx.host else {
                return Err(ToolError::new("No execution host is configured"));
            };
            let outcome = host
                .plan_update(&ctx.call_id, ops, expected)
                .await
                .map_err(ToolError::new)?;
            match outcome {
                Err(rejected) => Err(ToolError {
                    message: match rejected.op {
                        Some(op) => format!("Plan update rejected (op {op}): {}", rejected.message),
                        None => format!("Plan update rejected: {}", rejected.message),
                    },
                    diagnostics: vec![],
                    details: Some(details(json!({
                        "outcome": "rejected",
                        "code": rejected.code,
                        "op": rejected.op,
                        "message": rejected.message,
                        "revision": rejected.revision,
                    }))),
                }),
                Ok(receipt) => {
                    let view = host.plan_read(false).await.map_err(ToolError::new)?;
                    let mut lines = vec![format!(
                        "Plan revision {}. {}",
                        receipt.revision,
                        plans::counts_line(&view.counts)
                    )];
                    if !receipt.created.is_empty() {
                        let mut by_id: Vec<(&String, &u64)> = receipt.created.iter().collect();
                        by_id.sort_by_key(|(_, id)| **id);
                        let created: Vec<String> = by_id
                            .into_iter()
                            .map(|(name, id)| {
                                if name.starts_with('#') {
                                    format!("#{id}")
                                } else {
                                    format!("#{id} ({name})")
                                }
                            })
                            .collect();
                        lines.push(format!("Created: {}", created.join(", ")));
                    }
                    for id in &receipt.changed {
                        if let Some(task) = view.tasks.iter().find(|t| t.id == *id) {
                            lines.push(plans::line(task));
                        } else {
                            lines.push(format!("#{id} [archived]"));
                        }
                    }
                    Ok(ToolOutput {
                        content: Some(vec![Block::text(lines.join("\n"))]),
                        details: Some(details(json!({
                            "outcome": "applied",
                            "revision": receipt.revision,
                            "created": receipt.created,
                            "changed": receipt.changed,
                        }))),
                        ..ToolOutput::default()
                    })
                }
            }
        })
    }
}

pub struct PlanReadTool;

fn render(view: &PlanView, task: Option<u64>) -> Result<String, ToolError> {
    if let Some(id) = task {
        let found = view.tasks.iter().find(|t| t.id == id).ok_or_else(|| {
            invalid(format!(
                "task #{id} does not exist (or is archived; pass include_archived)"
            ))
        })?;
        return Ok(plans::detail(found));
    }
    let mut lines = vec![format!(
        "Plan revision {}. {}",
        view.revision,
        plans::counts_line(&view.counts)
    )];
    if view.tasks.is_empty() {
        lines.push("No tasks.".into());
    }
    for task in &view.tasks {
        lines.push(plans::line(task));
    }
    if view.truncated {
        lines.push("(older archived tasks omitted)".into());
    }
    Ok(lines.join("\n"))
}

impl Tool for PlanReadTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            constrained_sampling: None,
            name: PLAN_READ.into(),
            description: READ_DESCRIPTION.into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": {"type": "integer", "description": "Show this one task in full"},
                    "include_archived": {"type": "boolean", "description": "Also list archived tasks"}
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
            let Some(host) = &ctx.host else {
                return Err(ToolError::new("No execution host is configured"));
            };
            let task = args.get("task").and_then(Value::as_u64);
            let include_archived = args
                .get("include_archived")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || task.is_some();
            let view = host
                .plan_read(include_archived)
                .await
                .map_err(ToolError::new)?;
            let text = render(&view, task)?;
            Ok(ToolOutput {
                content: Some(vec![Block::text(text)]),
                details: Some(details(json!({
                    "outcome": "read",
                    "revision": view.revision,
                    "counts": view.counts,
                }))),
                ..ToolOutput::default()
            })
        })
    }
}
