//! `get_goal`, `update_goal` and `propose_goal_completion`: the executor's structured
//! reporting on the engine-owned goal (GOAL-2). The tools cannot create, resume or extend a
//! goal: that is the person's intent, sent from the desktop.
//!
//! `details.goal` (version 1) carries `{outcome: "read" | "updated" | "completed" |
//! "rejected" | "error", ...}`.

use pipkin_engine_protocol::{Block, CriterionStatus, GoalEvidence, GoalView};
use serde_json::{Value, json};

use super::{Tool, ToolCtx, ToolError, ToolOutput};
use crate::provider::{BoxFuture, ToolSpec};

pub const GET_GOAL: &str = "get_goal";
pub const UPDATE_GOAL: &str = "update_goal";
pub const PROPOSE_GOAL_COMPLETION: &str = "propose_goal_completion";
pub const RESULT_VERSION: u32 = 1;

pub const GET_DESCRIPTION: &str = "Read the conversation's current goal: objective, state, turns and allowances, completion criteria with their status and evidence, and your last note.";
pub const UPDATE_DESCRIPTION: &str = "Report progress on the goal. status: \"continue\" (more to do now), \"blocked\" (the person's input is needed; give the reason) or \"unreachable\" (give the reason). note: a short progress note. criteria: claims per criterion id with status met/unmet/open and evidence items {kind: toolCall|command|test|file|observation, reference, note?, sha256?}; the engine checks tool calls against its ledger and files against their hash, and counts observations as judgment only. link_tasks: plan task ids this goal covers. This never completes the goal: use propose_goal_completion for that.";
pub const PROPOSE_DESCRIPTION: &str = "Propose that the goal is met. Give a summary and, per criterion, evidence the engine can check (tool call ids, commands you ran, files with their sha256). It is accepted only when every criterion has accepted mechanical evidence; otherwise the unmet criteria and rejected evidence are returned and the goal stays active. Do not claim what was not done.";

/// One claim about a criterion.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub struct CriterionClaim {
    pub id: u64,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub evidence: Vec<EvidenceInput>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub struct EvidenceInput {
    pub kind: String,
    pub reference: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
}

impl EvidenceInput {
    pub fn into_evidence(self) -> GoalEvidence {
        GoalEvidence {
            kind: self.kind,
            reference: self.reference,
            note: self.note,
            sha256: self.sha256,
            accepted: None,
            verdict: None,
            judgment: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub struct GoalUpdateRequest {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub criteria: Vec<CriterionClaim>,
    #[serde(default)]
    pub link_tasks: Vec<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub struct GoalProposal {
    pub summary: String,
    #[serde(default)]
    pub criteria: Vec<CriterionClaim>,
    /// Evidence for the objective itself when the goal has no criteria.
    #[serde(default)]
    pub evidence: Vec<EvidenceInput>,
}

fn details(fields: Value) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("version".into(), RESULT_VERSION.into());
    if let Value::Object(fields) = fields {
        body.extend(fields);
    }
    json!({ "goal": Value::Object(body) })
}

fn error(kind: &str, message: String) -> ToolError {
    ToolError {
        message,
        diagnostics: vec![],
        details: Some(details(json!({ "outcome": "error", "kind": kind }))),
    }
}

/// The goal as the model reads it.
pub fn render(goal: &GoalView) -> String {
    let mut lines = vec![format!(
        "Goal #{} (revision {}): {}",
        goal.id, goal.revision, goal.objective
    )];
    let mut status = format!("State: {}", goal.state.as_str());
    if let Some(reason) = &goal.reason {
        status.push_str(&format!(" ({reason})"));
    }
    lines.push(status);
    let mut budget = format!("Turns: {}", goal.turns);
    if let Some(max) = goal.limits.max_turns {
        budget.push_str(&format!(" of {max}"));
    }
    budget.push_str(&format!(" · tokens used: {}", goal.usage_tokens));
    if let Some(max) = goal.limits.max_tokens {
        budget.push_str(&format!(" of {max}"));
    }
    lines.push(budget);
    if goal.criteria.is_empty() {
        lines.push(
            "Criteria: none set (propose completion with evidence for the objective itself)."
                .into(),
        );
    } else {
        lines.push("Criteria:".into());
        for c in &goal.criteria {
            lines.push(format!("- #{} [{}] {}", c.id, c.status.as_str(), c.text));
            for e in &c.evidence {
                let mark = match (e.accepted, e.judgment) {
                    (Some(true), true) => "judgment",
                    (Some(true), false) => "accepted",
                    (Some(false), _) => "rejected",
                    (None, _) => "unchecked",
                };
                lines.push(format!(
                    "    · {} {}: {}{}",
                    mark,
                    e.kind,
                    e.reference,
                    e.verdict
                        .as_deref()
                        .map(|v| format!(" — {v}"))
                        .unwrap_or_default()
                ));
            }
        }
    }
    if let Some(note) = &goal.note {
        lines.push(format!("Last note: {note}"));
    }
    if !goal.linked_tasks.is_empty() {
        let ids: Vec<String> = goal.linked_tasks.iter().map(|t| format!("#{t}")).collect();
        lines.push(format!("Linked plan tasks: {}", ids.join(", ")));
    }
    if goal.stall_turns > 0 {
        lines.push(format!(
            "No change to progress in the last {} continued turn(s).",
            goal.stall_turns
        ));
    }
    lines.join("\n")
}

fn goal_json(goal: &GoalView) -> Value {
    json!({
        "id": goal.id,
        "revision": goal.revision,
        "state": goal.state,
        "turns": goal.turns,
        "epoch": goal.epoch,
        "criteria": goal.criteria.iter().map(|c| json!({
            "id": c.id, "status": c.status, "accepted": c.evidence.iter().filter(|e| e.accepted == Some(true) && !e.judgment).count()
        })).collect::<Vec<_>>(),
        "limits": goal.limits,
        "usageTokens": goal.usage_tokens,
    })
}

pub struct GetGoalTool;

impl Tool for GetGoalTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            constrained_sampling: None,
            name: GET_GOAL.into(),
            description: GET_DESCRIPTION.into(),
            parameters: json!({"type": "object", "properties": {}}),
        }
    }

    fn replay_safe(&self) -> bool {
        true
    }

    fn execute<'a>(
        &'a self,
        _args: Value,
        ctx: &'a ToolCtx,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let Some(host) = &ctx.host else {
                return Err(ToolError::new("No execution host is configured"));
            };
            let Some(goal) = host.goal_get().await.map_err(ToolError::new)? else {
                return Err(error("no_goal", "This conversation has no goal.".into()));
            };
            Ok(ToolOutput {
                content: Some(vec![Block::text(render(&goal))]),
                details: Some(details(
                    json!({ "outcome": "read", "goal": goal_json(&goal) }),
                )),
                ..ToolOutput::default()
            })
        })
    }
}

fn criteria_schema() -> Value {
    json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "id": {"type": "integer"},
                "status": {"type": "string", "description": "met, unmet or open"},
                "evidence": {"type": "array", "items": {"type": "object", "properties": {
                    "kind": {"type": "string", "description": "toolCall, command, test, file or observation"},
                    "reference": {"type": "string", "description": "A tool call id, a path relative to the project, or the observation"},
                    "note": {"type": "string"},
                    "sha256": {"type": "string", "description": "For file: the content hash this claim is about"}
                }, "required": ["kind", "reference"]}}
            },
            "required": ["id"]
        }
    })
}

pub struct UpdateGoalTool;

impl Tool for UpdateGoalTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            constrained_sampling: None,
            name: UPDATE_GOAL.into(),
            description: UPDATE_DESCRIPTION.into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "status": {"type": "string", "description": "continue, blocked or unreachable"},
                    "reason": {"type": "string", "description": "Why blocked or unreachable"},
                    "note": {"type": "string", "description": "A short progress note"},
                    "criteria": criteria_schema(),
                    "link_tasks": {"type": "array", "items": {"type": "integer"}}
                }
            }),
        }
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolCtx,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let request: GoalUpdateRequest = serde_json::from_value(args)
                .map_err(|e| error("invalid", format!("update_goal: {e}")))?;
            let Some(host) = &ctx.host else {
                return Err(ToolError::new("No execution host is configured"));
            };
            match host
                .goal_update(&ctx.call_id, request)
                .await
                .map_err(ToolError::new)?
            {
                Err(reason) => Err(error("rejected", reason)),
                Ok(goal) => Ok(ToolOutput {
                    content: Some(vec![Block::text(format!("Recorded.\n{}", render(&goal)))]),
                    details: Some(details(
                        json!({ "outcome": "updated", "goal": goal_json(&goal) }),
                    )),
                    ..ToolOutput::default()
                }),
            }
        })
    }
}

pub struct ProposeGoalCompletionTool;

impl Tool for ProposeGoalCompletionTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            constrained_sampling: None,
            name: PROPOSE_GOAL_COMPLETION.into(),
            description: PROPOSE_DESCRIPTION.into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "summary": {"type": "string", "description": "What was done and how it meets the goal"},
                    "criteria": criteria_schema(),
                    "evidence": {"type": "array", "description": "Evidence for the objective when the goal has no criteria", "items": {"type": "object", "properties": {
                        "kind": {"type": "string"}, "reference": {"type": "string"}, "note": {"type": "string"}, "sha256": {"type": "string"}
                    }, "required": ["kind", "reference"]}}
                },
                "required": ["summary"]
            }),
        }
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolCtx,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let proposal: GoalProposal = serde_json::from_value(args)
                .map_err(|e| error("invalid", format!("propose_goal_completion: {e}")))?;
            let Some(host) = &ctx.host else {
                return Err(ToolError::new("No execution host is configured"));
            };
            match host
                .goal_propose(&ctx.call_id, proposal)
                .await
                .map_err(ToolError::new)?
            {
                Ok(goal) => Ok(ToolOutput {
                    content: Some(vec![Block::text(format!(
                        "Completion accepted: every criterion has accepted evidence. The goal is marked completed (not audited).\n{}",
                        render(&goal)
                    ))]),
                    details: Some(details(
                        json!({ "outcome": "completed", "goal": goal_json(&goal) }),
                    )),
                    ..ToolOutput::default()
                }),
                Err((problems, goal)) => {
                    let mut lines =
                        vec!["Completion not accepted; the goal stays active.".to_owned()];
                    for p in &problems {
                        lines.push(format!("- {p}"));
                    }
                    if let Some(goal) = &goal {
                        lines.push(render(goal));
                    }
                    Err(ToolError {
                        message: lines.join("\n"),
                        diagnostics: vec![],
                        details: Some(details(json!({
                            "outcome": "rejected",
                            "problems": problems,
                            "goal": goal.as_ref().map(goal_json),
                        }))),
                    })
                }
            }
        })
    }
}

/// A claim status as the model writes it.
pub fn parse_claim_status(s: &str) -> Option<CriterionStatus> {
    match s {
        "met" => Some(CriterionStatus::Met),
        "unmet" => Some(CriterionStatus::Unmet),
        "open" => Some(CriterionStatus::Open),
        _ => None,
    }
}
