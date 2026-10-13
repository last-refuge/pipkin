//! Background job tools: `job_start` returns at once with a stable id (the command keeps
//! running outside the model turn); `job_status`, `job_log` and `job_wait` are pure reads
//! (replay-safe); `job_cancel` is idempotent. `job_start` is not replay-safe: a start
//! interrupted by a restart is recorded as interrupted and never re-sent, and the job row,
//! if the intent was committed, says `unknown`.

use pipkin_engine_protocol::{Block, JobState, JobView};
use serde_json::{Value, json};

use super::{Tool, ToolCtx, ToolError, ToolOutput};
use crate::provider::{BoxFuture, ToolSpec};
use crate::runtime::jobs::{JobSpec, describe_state, format_elapsed};

pub const JOB_START: &str = "job_start";
pub const JOB_STATUS: &str = "job_status";
pub const JOB_LOG: &str = "job_log";
pub const JOB_WAIT: &str = "job_wait";
pub const JOB_CANCEL: &str = "job_cancel";
pub const RESULT_VERSION: u32 = 1;

fn details(fields: Value) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("version".into(), RESULT_VERSION.into());
    if let Value::Object(f) = fields {
        body.extend(f);
    }
    json!({ "job": Value::Object(body) })
}

fn host(ctx: &ToolCtx) -> Result<&std::sync::Arc<dyn super::ToolHost>, ToolError> {
    ctx.host
        .as_ref()
        .ok_or_else(|| ToolError::new("No execution host is configured"))
}

fn id_arg(args: &Value) -> Result<u64, ToolError> {
    args.get("id")
        .and_then(Value::as_u64)
        .filter(|id| *id > 0)
        .ok_or_else(|| ToolError::new("id must be a positive job id"))
}

pub fn summary(job: &JobView, now: i64) -> String {
    let label = job.name.clone().unwrap_or_else(|| job.command.clone());
    let elapsed = match (job.started_at_ms, job.ended_at_ms) {
        (Some(s), Some(e)) if e >= s => format!(", {}", format_elapsed(e - s)),
        (Some(s), None) => format!(", {} so far", format_elapsed(now - s)),
        _ => String::new(),
    };
    let mut line = format!(
        "#{} {label}: {}{elapsed}; log {} bytes{}",
        job.id,
        describe_state(state_name(job.state), job.exit_code),
        job.log_bytes,
        if job.log_truncated {
            " (truncated at the quota)"
        } else {
            ""
        }
    );
    if let Some(reason) = &job.reason
        && job.state != JobState::Failed
    {
        line.push_str(&format!(" — {reason}"));
    }
    line
}

fn state_name(state: JobState) -> &'static str {
    match state {
        JobState::Starting => "starting",
        JobState::Running => "running",
        JobState::Cancelling => "cancelling",
        JobState::Succeeded => "succeeded",
        JobState::Failed => "failed",
        JobState::TimedOut => "timed_out",
        JobState::Cancelled => "cancelled",
        JobState::Interrupted => "interrupted",
        JobState::Unknown => "unknown",
    }
}

fn job_json(job: &JobView) -> Value {
    json!({
        "id": job.id,
        "name": job.name,
        "state": state_name(job.state),
        "exitCode": job.exit_code,
        "logBytes": job.log_bytes,
        "logTruncated": job.log_truncated,
        "wake": job.wake,
        "delivery": job.delivery,
    })
}

pub struct JobStartTool;

impl Tool for JobStartTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            constrained_sampling: None,
            name: JOB_START.into(),
            description: "Start a command as a background job and return at once with its id. Use it for builds, test runs, dev servers and anything that takes a while: the conversation continues while it runs. Read progress with job_log, wait with job_wait, stop with job_cancel. Set wake to true to be told when it finishes.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "Shell command to run"},
                    "name": {"type": "string", "description": "Short label for the job"},
                    "cwd": {"type": "string", "description": "Working directory (default: the session's)"},
                    "timeout": {"type": "number", "description": "Seconds before the job is stopped (optional)"},
                    "wake": {"type": "boolean", "description": "Send a follow-up message when the job finishes (default false)"},
                    "task": {"type": "integer", "description": "Plan task id this job works on (optional)"}
                },
                "required": ["command"]
            }),
        }
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolCtx,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let spec = JobSpec {
                command: args
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                name: args.get("name").and_then(Value::as_str).map(str::to_owned),
                cwd: args.get("cwd").and_then(Value::as_str).map(str::to_owned),
                timeout_secs: args.get("timeout").and_then(Value::as_f64),
                wake: args.get("wake").and_then(Value::as_bool).unwrap_or(false),
                task: args.get("task").and_then(Value::as_u64),
            };
            let job = host(ctx)?
                .job_start(&ctx.call_id, spec)
                .await
                .map_err(ToolError::new)?;
            let text = format!(
                "Started job #{} ({}) in {}. It keeps running in the background{}. Check it with job_status or job_log (id {}), wait with job_wait, stop with job_cancel.",
                job.id,
                job.name.clone().unwrap_or_else(|| job.command.clone()),
                job.cwd,
                if job.wake {
                    "; you will get a follow-up when it finishes"
                } else {
                    ""
                },
                job.id
            );
            Ok(ToolOutput {
                content: Some(vec![Block::text(text)]),
                details: Some(details(
                    json!({"outcome": "started", "job": job_json(&job)}),
                )),
                ..ToolOutput::default()
            })
        })
    }
}

pub struct JobStatusTool;

impl Tool for JobStatusTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            constrained_sampling: None,
            name: JOB_STATUS.into(),
            description: "The state of one background job (by id) or of every job in this session."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {"id": {"type": "integer", "description": "A job id; omit for all jobs"}}
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
            let now = crate::util::now_ms();
            let jobs = host(ctx)?.jobs().await.map_err(ToolError::new)?;
            let chosen: Vec<&JobView> = match args.get("id").and_then(Value::as_u64) {
                Some(id) => {
                    let job =
                        jobs.jobs.iter().find(|j| j.id == id).ok_or_else(|| {
                            ToolError::new(format!("No job #{id} in this session"))
                        })?;
                    vec![job]
                }
                None => jobs.jobs.iter().collect(),
            };
            let mut lines: Vec<String> = chosen.iter().map(|j| summary(j, now)).collect();
            if lines.is_empty() {
                lines.push("No jobs in this session.".into());
            }
            if jobs.truncated {
                lines.push("(older finished jobs are not listed)".into());
            }
            Ok(ToolOutput {
                content: Some(vec![Block::text(lines.join("\n"))]),
                details: Some(details(json!({
                    "outcome": "status",
                    "jobs": chosen.iter().map(|j| job_json(j)).collect::<Vec<_>>(),
                }))),
                ..ToolOutput::default()
            })
        })
    }
}

pub struct JobLogTool;

impl Tool for JobLogTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            constrained_sampling: None,
            name: JOB_LOG.into(),
            description: "Read a background job's log (stdout and stderr, in arrival order) from a byte offset. Returns a bounded slice and the offset to continue from; use a large offset or omit it with tail=true to read the newest output.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {"type": "integer", "description": "The job id"},
                    "offset": {"type": "integer", "description": "Byte offset to start from (default 0)"},
                    "limit": {"type": "integer", "description": "Bytes to return (default 8192, at most 65536)"},
                    "tail": {"type": "boolean", "description": "Read the last `limit` bytes instead of from `offset`"}
                },
                "required": ["id"]
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
            let id = id_arg(&args)?;
            let limit = args.get("limit").and_then(Value::as_u64);
            let tail = args.get("tail").and_then(Value::as_bool).unwrap_or(false);
            let h = host(ctx)?;
            let offset = if tail {
                let total = h
                    .job_log(id, 0, Some(1))
                    .await
                    .map_err(ToolError::new)?
                    .total_bytes;
                total.saturating_sub(limit.unwrap_or(crate::runtime::jobs::DEFAULT_LOG_SLICE))
            } else {
                args.get("offset").and_then(Value::as_u64).unwrap_or(0)
            };
            let slice = h.job_log(id, offset, limit).await.map_err(ToolError::new)?;
            let mut header = format!(
                "Job #{} log, bytes {}–{} of {}{}{}",
                id,
                slice.offset,
                slice.next_offset,
                slice.total_bytes,
                if slice.truncated {
                    " (truncated at the quota)"
                } else {
                    ""
                },
                if slice.done {
                    " (finished)"
                } else {
                    " (still running)"
                }
            );
            if slice.next_offset < slice.total_bytes {
                header.push_str(&format!(" — continue with offset {}", slice.next_offset));
            }
            let text = if slice.text.is_empty() {
                format!("{header}\n(no output in this range)")
            } else {
                format!("{header}\n{}", slice.text)
            };
            Ok(ToolOutput {
                content: Some(vec![Block::text(text)]),
                details: Some(details(json!({
                    "outcome": "log",
                    "id": id,
                    "offset": slice.offset,
                    "nextOffset": slice.next_offset,
                    "totalBytes": slice.total_bytes,
                    "truncated": slice.truncated,
                    "done": slice.done,
                }))),
                ..ToolOutput::default()
            })
        })
    }
}

pub struct JobWaitTool;

impl Tool for JobWaitTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            constrained_sampling: None,
            name: JOB_WAIT.into(),
            description: "Wait for a background job to finish, up to a bounded number of seconds (default 60, at most 600), then report its state and the end of its log.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {"type": "integer", "description": "The job id"},
                    "timeout": {"type": "number", "description": "Seconds to wait (default 60, at most 600)"}
                },
                "required": ["id"]
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
            let id = id_arg(&args)?;
            let timeout = args
                .get("timeout")
                .and_then(Value::as_f64)
                .filter(|t| t.is_finite() && *t > 0.0)
                .unwrap_or(60.0)
                .min(crate::runtime::jobs::MAX_WAIT_SECS as f64);
            let h = host(ctx)?;
            let job = h
                .job_wait(
                    id,
                    std::time::Duration::from_secs_f64(timeout),
                    ctx.cancel.clone(),
                )
                .await
                .map_err(ToolError::new)?;
            let now = crate::util::now_ms();
            let mut text = summary(&job, now);
            if job.state.active() {
                text.push_str(&format!(
                    "\nStill running after {} seconds; wait again or read job_log.",
                    timeout
                ));
            } else {
                let tail_from = job.log_bytes.saturating_sub(4096);
                if let Ok(slice) = h.job_log(id, tail_from, Some(4096)).await
                    && !slice.text.trim().is_empty()
                {
                    text.push_str(&format!(
                        "\nLast output{}:\n{}",
                        if tail_from > 0 { " (tail)" } else { "" },
                        slice.text.trim_end()
                    ));
                }
            }
            Ok(ToolOutput {
                content: Some(vec![Block::text(text)]),
                details: Some(details(
                    json!({"outcome": "wait", "job": job_json(&job), "finished": !job.state.active()}),
                )),
                ..ToolOutput::default()
            })
        })
    }
}

pub struct JobCancelTool;

impl Tool for JobCancelTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            constrained_sampling: None,
            name: JOB_CANCEL.into(),
            description: "Stop a background job (its whole process tree). Safe to repeat; a finished job is reported as it ended.".into(),
            parameters: json!({
                "type": "object",
                "properties": {"id": {"type": "integer", "description": "The job id"}},
                "required": ["id"]
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
            let id = id_arg(&args)?;
            let job = host(ctx)?.job_cancel(id).await.map_err(ToolError::new)?;
            let now = crate::util::now_ms();
            Ok(ToolOutput {
                content: Some(vec![Block::text(summary(&job, now))]),
                details: Some(details(json!({"outcome": "cancel", "job": job_json(&job)}))),
                ..ToolOutput::default()
            })
        })
    }
}
