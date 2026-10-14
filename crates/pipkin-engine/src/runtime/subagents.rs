//! Durable subagents (reference `experimental/durable/subagent.ts` + Pipkin's subagent overlay).
//!
//! A `subagent` call creates — or, on replay, finds — exactly one child session for its
//! `(parent session, call id)`, committed atomically with the task row. The child receives the
//! task as a prompt under the durable key `subagent:<call id>` (admission is idempotent), runs
//! with the parent's model/thinking and cwd but without the `subagent` tool (children cannot
//! delegate), and its answer (the concatenated text of its final reply) becomes the parent's
//! tool result. The tool is replay-safe: after a restart the call reruns, finds the same child
//! and the same admission, and waits for its settlement. Stopping the parent call stops the
//! child.
//!
//! SUB-2: a person can Stop or Steer one child from the inspector. Both are authorized by the
//! `(root, child)` pair the engine published, recorded durably (the note, the steer key) and
//! settled through the child's own run; a waiting parent is told the child was stopped by the
//! person rather than merely "aborted".
//!
//! SUB-3: `wait: false` makes the call return a launch receipt while the child runs in the
//! background. A detached watcher records the child's settlement on its task row and delivers
//! the completion once: clients see `SubagentsChanged`; a model-origin child launched with
//! `wake` gets one follow-up under the fixed key `subagent:<child>:done` (idempotent across
//! restarts), suppressed when the run that launched it was stopped by the person. Children
//! launched by the person (`LaunchSubagent`) are notification-only. Restart re-attaches
//! watchers to background children and finishes interrupted deliveries.

use std::sync::Arc;
use std::time::Duration;

use pipkin_engine_protocol::limits::{
    MAX_SUBAGENT_ARTIFACTS, SUBAGENT_RESULT_INLINE_CHARS, SUBAGENT_TASK_EXCERPT_CHARS,
};
use pipkin_engine_protocol::*;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::Shared;
use super::session::SessionRt;
use crate::profiles::{self, ContextMode, LaunchRecord};
use crate::provider::{BoxFuture, ToolSpec};
use crate::store::{RequestState, Store, TaskRow};
use crate::tools::{SubagentLaunch, Tool, ToolCtx, ToolError, ToolHost, ToolOutput};
use crate::util::now_ms;

pub const SUBAGENT_DESCRIPTION: &str = "Delegate a self-contained task to a subagent and get its answer back. Give it everything it needs to know: by default it starts without this conversation (context \"fresh\"); context \"inherit\" adds a bounded excerpt of recent messages. Profiles: \"worker\" (default: every tool a child can have), \"reviewer\" (read-only: read, plan_read, web_read), \"scout\" (reads plus web search and fetch; cannot change the workspace), or a profile defined in settings. A child never delegates further or asks the person. Set wait to false to keep working while the child runs in the background: the call returns its child number at once; by default you get a follow-up message when it finishes (wake), and subagent_status, subagent_wait and subagent_result report on it.";

/// The note a person's Stop leaves on the task row (shown to the parent and the inspector).
pub const STOPPED_BY_PERSON: &str = "Stopped by the person";
/// Longest a single `subagent_wait` blocks.
pub const MAX_WAIT_SECS: u64 = 600;
/// Answer text included in a completion follow-up; the rest is read with `subagent_result`.
const WAKE_ANSWER_CHARS: usize = 4096;
/// Largest task a person may launch from the inspector.
const MAX_LAUNCH_TASK_BYTES: usize = 64 * 1024;

pub struct SubagentTool;

impl Tool for SubagentTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            constrained_sampling: None,
            name: "subagent".into(),
            description: SUBAGENT_DESCRIPTION.into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": {"type": "string", "description": "What the subagent should do"},
                    "profile": {"type": "string", "description": "worker (default), reviewer, scout, or a settings-defined profile"},
                    "context": {"type": "string", "description": "fresh (default) or inherit: whether the child sees a bounded excerpt of this conversation"},
                    "wait": {"type": "boolean", "description": "true (default): wait for the answer. false: return at once with the child number and keep working while it runs"},
                    "wake": {"type": "boolean", "description": "Background children only: send a follow-up message when the child finishes (default true); false records the result for subagent_result without interrupting"}
                },
                "required": ["task"]
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
            let task = args.get("task").and_then(Value::as_str).unwrap_or_default();
            let profile = args
                .get("profile")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_owned);
            let context = match args.get("context").and_then(Value::as_str).map(str::trim) {
                None | Some("") => None,
                Some(mode) => Some(crate::profiles::ContextMode::parse(mode).ok_or_else(|| {
                    ToolError::new(format!(
                        "context must be \"fresh\" or \"inherit\", not {mode:?}"
                    ))
                })?),
            };
            let Some(host) = &ctx.host else {
                return Err(ToolError::new("No execution host is configured"));
            };
            let wait = args.get("wait").and_then(Value::as_bool).unwrap_or(true);
            let wake = args.get("wake").and_then(Value::as_bool).unwrap_or(true);
            let launch = SubagentLaunch {
                task: task.to_owned(),
                profile,
                context,
                wait,
                wake,
                model: None,
            };
            match host
                .subagent(&ctx.call_id, launch, ctx.cancel.clone())
                .await
            {
                Ok((text, details)) => Ok(ToolOutput {
                    content: Some(vec![Block::text(text)]),
                    details: Some(details),
                    ..ToolOutput::default()
                }),
                Err(message) => Err(ToolError::new(message)),
            }
        })
    }
}

/// Durable execution services for one session: progress checkpoints and subagent calls.
pub struct Host {
    pub shared: Arc<Shared>,
    pub parent: String,
}

impl ToolHost for Host {
    fn checkpoint_output(
        &self,
        call_id: &str,
        output: &crate::tools::text::BoundedOutput,
    ) -> Result<(), String> {
        let session = self.shared.session(&self.parent).map_err(|e| e.message)?;
        self.shared
            .commit(&session, |tx| {
                Store::save_tool_progress(tx, &self.parent, call_id, output)?;
                Ok(((), vec![]))
            })
            .map_err(|e| e.message)?;
        super::faults::crash_point("after_tool_progress_checkpoint");
        Ok(())
    }

    fn subagent<'a>(
        &'a self,
        call_id: &'a str,
        launch: SubagentLaunch,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<(String, Value), String>> {
        Box::pin(async move {
            self.shared
                .run_subagent(&self.parent, call_id, launch, cancel)
                .await
        })
    }

    fn ask<'a>(
        &'a self,
        _call_id: &'a str,
        question: crate::extensions::questions::Question,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, crate::tools::ask::AskOutcome> {
        Box::pin(async move { self.shared.ask_user(&self.parent, question, cancel).await })
    }

    fn plan_update<'a>(
        &'a self,
        call_id: &'a str,
        ops: Vec<PlanOp>,
        expected_revision: Option<u64>,
    ) -> BoxFuture<'a, Result<Result<PlanReceipt, PlanRejected>, String>> {
        Box::pin(async move {
            self.shared
                .update_plan(
                    &self.parent,
                    &format!("call:{call_id}"),
                    &ops,
                    expected_revision,
                )
                .map_err(|e| e.message)
        })
    }

    fn plan_read<'a>(&'a self, include_archived: bool) -> BoxFuture<'a, Result<PlanView, String>> {
        Box::pin(async move {
            self.shared
                .plan_view(&self.parent, include_archived)
                .map_err(|e| e.message)
        })
    }

    fn web_search<'a>(
        &'a self,
        call_id: &'a str,
        query: &'a str,
        count: usize,
        cancel: CancellationToken,
    ) -> BoxFuture<
        'a,
        Result<
            Result<(String, Vec<crate::web::SourceRecord>), crate::web::providers::SearchError>,
            String,
        >,
    > {
        Box::pin(async move {
            let service = self.shared.web_service(&self.parent);
            let outcome = service
                .search(&self.parent, call_id, query, count, cancel)
                .await;
            if outcome.is_ok() {
                self.shared.publish_web_sources(&self.parent);
            }
            Ok(outcome)
        })
    }

    fn web_fetch<'a>(
        &'a self,
        call_id: &'a str,
        url: &'a str,
        cancel: CancellationToken,
    ) -> BoxFuture<
        'a,
        Result<
            Result<(crate::web::SourceRecord, crate::web::Fetched), crate::web::FetchError>,
            String,
        >,
    > {
        Box::pin(async move {
            let service = self.shared.web_service(&self.parent);
            let outcome = service.fetch(&self.parent, call_id, url, cancel).await;
            if outcome.is_ok() {
                self.shared.publish_web_sources(&self.parent);
            }
            Ok(outcome)
        })
    }

    fn goal_get<'a>(&'a self) -> BoxFuture<'a, Result<Option<GoalView>, String>> {
        Box::pin(async move { self.shared.goal_view(&self.parent).map_err(|e| e.message) })
    }

    fn goal_update<'a>(
        &'a self,
        _call_id: &'a str,
        request: crate::tools::goal::GoalUpdateRequest,
    ) -> BoxFuture<'a, Result<Result<GoalView, String>, String>> {
        Box::pin(async move { self.shared.goal_update_from_tool(&self.parent, request) })
    }

    fn goal_propose<'a>(
        &'a self,
        _call_id: &'a str,
        proposal: crate::tools::goal::GoalProposal,
    ) -> BoxFuture<'a, Result<Result<GoalView, (Vec<String>, Option<GoalView>)>, String>> {
        Box::pin(async move { self.shared.goal_propose_from_tool(&self.parent, proposal) })
    }

    fn mcp_search<'a>(
        &'a self,
        query: &'a str,
        server: Option<&'a str>,
    ) -> BoxFuture<
        'a,
        Result<
            Result<(Vec<crate::mcp::ToolInfo>, Vec<(String, String)>), crate::mcp::McpError>,
            String,
        >,
    > {
        Box::pin(async move {
            self.shared.mcp_refresh(Some(&self.parent));
            Ok(self.shared.mcp.search(query, server).await)
        })
    }

    fn mcp_describe<'a>(
        &'a self,
        qualified: &'a str,
    ) -> BoxFuture<'a, Result<Result<crate::mcp::ToolInfo, crate::mcp::McpError>, String>> {
        Box::pin(async move {
            self.shared.mcp_refresh(Some(&self.parent));
            Ok(self.shared.mcp.describe(qualified).await)
        })
    }

    fn mcp_call<'a>(
        &'a self,
        _call_id: &'a str,
        qualified: &'a str,
        arguments: serde_json::Value,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<Result<crate::mcp::CallOutcome, crate::mcp::McpError>, String>> {
        Box::pin(async move {
            self.shared.mcp_refresh(Some(&self.parent));
            Ok(self
                .shared
                .mcp
                .call(&self.parent, qualified, arguments, cancel)
                .await)
        })
    }

    fn mcp_resources<'a>(
        &'a self,
        server: Option<&'a str>,
    ) -> BoxFuture<
        'a,
        Result<
            Result<(Vec<crate::mcp::ResourceInfo>, Vec<(String, String)>), crate::mcp::McpError>,
            String,
        >,
    > {
        Box::pin(async move {
            self.shared.mcp_refresh(Some(&self.parent));
            Ok(self.shared.mcp_resources(server).await)
        })
    }

    fn mcp_read<'a>(
        &'a self,
        server: &'a str,
        uri: &'a str,
    ) -> BoxFuture<'a, Result<Result<Vec<crate::mcp::ResourcePart>, crate::mcp::McpError>, String>>
    {
        Box::pin(async move {
            self.shared.mcp_refresh(Some(&self.parent));
            Ok(self.shared.mcp.read_resource(server, uri).await)
        })
    }

    fn mcp_prompts<'a>(
        &'a self,
        server: Option<&'a str>,
    ) -> BoxFuture<
        'a,
        Result<
            Result<(Vec<crate::mcp::PromptInfo>, Vec<(String, String)>), crate::mcp::McpError>,
            String,
        >,
    > {
        Box::pin(async move {
            self.shared.mcp_refresh(Some(&self.parent));
            Ok(self.shared.mcp_prompts(server).await)
        })
    }

    fn mcp_prompt<'a>(
        &'a self,
        server: &'a str,
        prompt: &'a str,
        arguments: serde_json::Map<String, Value>,
    ) -> BoxFuture<'a, Result<Result<crate::mcp::PromptOutcome, crate::mcp::McpError>, String>>
    {
        Box::pin(async move {
            self.shared.mcp_refresh(Some(&self.parent));
            Ok(self.shared.mcp.get_prompt(server, prompt, arguments).await)
        })
    }

    fn job_start<'a>(
        &'a self,
        call_id: &'a str,
        spec: crate::runtime::jobs::JobSpec,
    ) -> BoxFuture<'a, Result<JobView, String>> {
        Box::pin(async move {
            let run = self
                .shared
                .store
                .tool_call(&self.parent, call_id)
                .ok()
                .flatten()
                .and_then(|c| c.run);
            self.shared
                .job_start(&self.parent, "model", Some(call_id), run, spec)
                .await
                .map_err(|e| e.message)
        })
    }

    fn jobs<'a>(&'a self) -> BoxFuture<'a, Result<JobsView, String>> {
        Box::pin(async move { self.shared.jobs_view(&self.parent).map_err(|e| e.message) })
    }

    fn job_log<'a>(
        &'a self,
        id: u64,
        offset: u64,
        limit: Option<u64>,
    ) -> BoxFuture<'a, Result<JobLogView, String>> {
        Box::pin(async move {
            self.shared
                .job_log(&self.parent, id, offset, limit)
                .map_err(|e| e.message)
        })
    }

    fn job_wait<'a>(
        &'a self,
        id: u64,
        timeout: std::time::Duration,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<JobView, String>> {
        Box::pin(async move {
            self.shared
                .job_wait(&self.parent, id, timeout, &cancel)
                .await
                .map_err(|e| e.message)
        })
    }

    fn job_cancel<'a>(&'a self, id: u64) -> BoxFuture<'a, Result<JobView, String>> {
        Box::pin(async move {
            self.shared
                .job_cancel(&self.parent, id)
                .await
                .map_err(|e| e.message)
        })
    }

    fn subagent_status<'a>(&'a self) -> BoxFuture<'a, Result<SubagentsView, String>> {
        Box::pin(async move {
            self.shared
                .subagents_view(&self.parent)
                .map_err(|e| e.message)
        })
    }

    fn subagent_result<'a>(
        &'a self,
        child: u64,
        offset: usize,
    ) -> BoxFuture<'a, Result<SubagentResultView, String>> {
        Box::pin(async move {
            self.shared
                .subagent_result(&self.parent, child, offset)
                .map_err(|e| e.message)
        })
    }

    fn subagent_answer<'a>(&'a self, child: u64) -> BoxFuture<'a, Result<Option<String>, String>> {
        Box::pin(async move {
            let task = self
                .shared
                .authorized_child(&self.parent, child)
                .map_err(|e| e.message)?;
            Ok(match task.status.as_str() {
                "done" => task.result,
                _ => None,
            })
        })
    }

    fn subagent_wait<'a>(
        &'a self,
        child: u64,
        timeout: Duration,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<SubagentResultView, String>> {
        Box::pin(async move {
            self.shared
                .subagent_wait(&self.parent, child, timeout, &cancel)
                .await
                .map_err(|e| e.message)
        })
    }

    fn web_source<'a>(
        &'a self,
        id: u64,
    ) -> BoxFuture<'a, Result<(Option<crate::web::SourceRecord>, Option<String>), String>> {
        Box::pin(async move {
            let service = self.shared.web_service(&self.parent);
            let record = service.source(&self.parent, id)?;
            let text = match &record {
                Some(r) if !r.expired => service.text(&self.parent, id)?,
                _ => None,
            };
            Ok((record, text))
        })
    }
}

/// Map a child run's state to the directory status strings the desktop understands
/// (reference: terminal `completed` -> `done`, `aborted`, else `failed`; live states raw).
fn status_for(outcome: &RunOutcome) -> &'static str {
    match outcome {
        RunOutcome::Completed => "done",
        RunOutcome::Aborted => "aborted",
        _ => "failed",
    }
}

impl Shared {
    pub fn subagent_tool(&self) -> Arc<dyn Tool> {
        Arc::new(SubagentTool)
    }

    /// Tool names a child could be offered (every built-in but the ones never for children).
    pub fn child_offerable_tools(&self) -> Vec<String> {
        self.tools
            .iter()
            .map(|t| t.spec().name)
            .chain(["list_files".to_owned(), "search_files".to_owned()])
            .chain(self.plan_tools().iter().map(|t| t.spec().name))
            .chain(self.web_tools().iter().map(|t| t.spec().name))
            .filter(|n| !profiles::NEVER_FOR_CHILDREN.contains(&n.as_str()))
            .collect()
    }

    /// The frozen launch policy of a child session, if it has one.
    pub fn child_launch(&self, session: &str) -> Option<LaunchRecord> {
        self.store
            .read(|c| Store::task_by_session(c, session))
            .ok()
            .flatten()
            .and_then(|t| t.launch)
            .and_then(|l| LaunchRecord::from_json(&l))
    }

    pub(super) async fn run_subagent(
        self: &Arc<Self>,
        parent: &str,
        call_id: &str,
        launch: SubagentLaunch,
        cancel: CancellationToken,
    ) -> Result<(String, Value), String> {
        let delivery = match (launch.wait, launch.wake) {
            (true, _) => "inline",
            (false, true) => "wake",
            (false, false) => "notify",
        };
        let (task_row, created) = self.create_child(parent, call_id, launch, "model", delivery)?;
        if created {
            self.publish_subagents(&task_row.root);
        }
        let details = json!({"conversationId": task_row.child_session, "child": task_row.child});
        let parent_session = self.session(parent).map_err(|e| e.message)?;
        let sess = self.admit_child(&parent_session, &task_row).await?;
        // The row, not the arguments, says whether this call waits: a replay keeps the
        // policy the child was launched with.
        if task_row.background() {
            self.watch_child(task_row.clone());
            let policy = if task_row.delivery == "wake" {
                "You will get a follow-up message when it finishes; subagent_status and subagent_wait also report on it."
            } else {
                "It will not interrupt you: check subagent_status, wait with subagent_wait, and read its answer with subagent_result."
            };
            return Ok((
                format!(
                    "Subagent #{} started in the background. {policy}",
                    task_row.child
                ),
                details,
            ));
        }
        let outcome = self
            .await_child(&parent_session, &sess, &task_row, cancel)
            .await?;
        match self.record_outcome(&task_row, &outcome) {
            ChildEnd::Answered(answer) => {
                let artifacts = self.child_artifacts(&task_row.child_session);
                let mut details = details;
                details["artifacts"] = json!(artifacts);
                Ok((inline_answer(task_row.child, &answer, &artifacts), details))
            }
            ChildEnd::Unanswered(message) => Err(message),
        }
    }

    /// Create (or, on replay, find) the child session and task row for `(parent, call_id)`.
    /// Resolves the profile, model, context and caps against the parent's settings for a new
    /// child; an existing child keeps the policy it was launched with.
    fn create_child(
        self: &Arc<Self>,
        parent: &str,
        call_id: &str,
        launch: SubagentLaunch,
        origin: &str,
        delivery: &str,
    ) -> Result<(TaskRow, bool), String> {
        self.require_native_execution(parent)
            .map_err(|e| e.message)?;
        let parent_row = self
            .store
            .session(parent)
            .map_err(|e| e.to_string())?
            .ok_or("The parent session no longer exists")?;
        let root = parent_row.root.clone().unwrap_or_else(|| parent.to_owned());
        let child_id = format!("{parent}:{call_id}");
        let existing = self
            .store
            .read(|c| Store::task_for_call(c, parent, call_id))
            .map_err(|e| e.to_string())?;
        if let Some(row) = existing {
            return Ok((row, false));
        }
        let settings = self.settings_for(parent);
        let offerable = self.child_offerable_tools();
        let profiles = profiles::profiles_from_settings(&settings, &offerable)
            .map_err(|e| format!("Subagent profiles are misconfigured: {e}"))?;
        let caps = profiles::caps_from_settings(&settings);
        let name = launch
            .profile
            .clone()
            .unwrap_or_else(|| profiles::DEFAULT_PROFILE.to_owned());
        let profile = profiles.iter().find(|p| p.name == name).ok_or_else(|| {
            format!(
                "Unknown subagent profile {name:?} (available: {})",
                profiles
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
        let model = launch
            .model
            .clone()
            .or_else(|| profile.model.clone())
            .or_else(|| parent_row.model.clone());
        if let Some(key) = &model {
            let catalog = self.auth.catalog();
            let found = catalog.find(key).cloned();
            match found {
                None => {
                    return Err(format!(
                        "Model {key} (profile {name:?}) is not available; no other model is substituted"
                    ));
                }
                Some(m) => {
                    if let Err(crate::auth::AuthError::Unavailable(why)) =
                        self.auth.availability(&m)
                    {
                        return Err(format!(
                            "Model {key} (profile {name:?}) is not available: {why}"
                        ));
                    }
                }
            }
        }
        let thinking = profile
            .thinking
            .clone()
            .or_else(|| parent_row.thinking.clone());
        if let Some(level) = &thinking
            && crate::provider::ThinkingLevel::parse(level).is_none()
        {
            return Err(format!(
                "Thinking level {level:?} (profile {name:?}) is not valid"
            ));
        }
        let context = launch.context.unwrap_or(profile.context);
        let (excerpt, omitted, parent_revision) = if context == ContextMode::Inherit {
            let (entries, _) = self.store.page(parent, None, 50).unwrap_or_default();
            let (excerpt, omitted) = profiles::context_excerpt(&entries);
            (excerpt, omitted, Some(parent_row.revision))
        } else {
            (Vec::new(), 0, None)
        };
        let prompt = profiles::compose_prompt(
            &launch.task,
            &excerpt,
            omitted,
            parent_revision.unwrap_or(0),
        );
        let record = LaunchRecord {
            version: profiles::PROFILE_VERSION,
            profile: profile.name.clone(),
            instructions: profile.instructions.clone(),
            tools: match &profile.tools {
                profiles::ToolPolicy::All => None,
                profiles::ToolPolicy::Only(list) => Some(list.clone()),
            },
            model: model.clone(),
            thinking: thinking.clone(),
            context,
            parent_revision,
            context_messages: excerpt.len(),
            context_omitted: omitted,
            created_at_ms: now_ms(),
        };
        let launch_json = record.to_json();
        // Create the child and its task mapping atomically, under the caps.
        let outcome: Result<Result<TaskRow, String>, _> = tokio::task::block_in_place(|| {
            self.store.tx(|tx| {
                Store::assert_session_live(tx, parent)?;
                if let Some(existing) = Store::task_for_call(tx, parent, call_id)? {
                    return Ok(Ok(existing));
                }
                let (live, total) = Store::child_counts(tx, &root)?;
                if total >= caps.max_total {
                    return Ok(Err(format!(
                        "This conversation already has {total} subagents (the limit is {}); no more can be started",
                        caps.max_total
                    )));
                }
                if live >= caps.max_concurrent {
                    return Ok(Err(format!(
                        "{live} subagent(s) are already running (the limit is {}); wait for one to finish",
                        caps.max_concurrent
                    )));
                }
                let now = now_ms();
                tx.execute(
                    "INSERT INTO sessions(id, cwd, title, created_at, updated_at, origin, model,
                        thinking, subagents_enabled, parent, root, create_key, revision)
                     VALUES (?1, ?2, NULL, ?3, ?3, ?4, ?5, ?6, 0, ?7, ?8, NULL, 0)",
                    rusqlite::params![
                        child_id,
                        parent_row.cwd,
                        now,
                        serde_json::to_string(&Origin::Native).expect("serializes"),
                        model,
                        thinking,
                        parent,
                        root
                    ],
                )?;
                let (row, _) = Store::create_task_with(
                    tx, &root, parent, call_id, &child_id, &prompt, origin, delivery, now,
                )?;
                Store::set_task_launch(tx, &child_id, &launch_json)?;
                Store::bump(tx, &root, now)?;
                Ok(Ok(Store::task_by_session(tx, &child_id)?.unwrap_or(row)))
            })
        });
        match outcome.map_err(|e| e.to_string())? {
            Ok(row) => Ok((row, true)),
            Err(refused) => Err(refused),
        }
    }

    /// Reopen the child and admit its task prompt (idempotent: a replay finds the original),
    /// marking the task running.
    async fn admit_child(
        self: &Arc<Self>,
        parent_session: &Arc<SessionRt>,
        task_row: &TaskRow,
    ) -> Result<Arc<SessionRt>, String> {
        let child = task_row.child_session.as_str();
        let key = format!("subagent:{}", task_row.parent_call);
        let sess = self.session(child).map_err(|e| e.message)?;
        // Reopening the parent also reopens its existing durable child. After an in-process
        // storage failure, admission alone only finds the old key; it does not resume it.
        if let Err(error) = self.recover_session(child) {
            if error.code == ErrorCode::Storage {
                self.storage_failure(parent_session, &error.message);
            }
            return Err(error.message);
        }
        match self.prompt(child, &key, &task_row.task, &[]).await {
            Ok(_) => {}
            Err(e) if e.code == ErrorCode::Busy => {}
            Err(e) => {
                if e.code == ErrorCode::Storage {
                    // Recovery may still be waiting for the preceding child execution to
                    // unwind. Do not turn that unresolved call into a final error result.
                    self.storage_failure(parent_session, &e.message);
                }
                return Err(format!("Subagent {} failed: {}", task_row.child, e.message));
            }
        }
        if !task_row.finished() {
            self.set_task_status(&task_row.root, child, "running", None);
        }
        Ok(sess)
    }

    /// Wait for the child's task key to settle (it may already be settled), stopping the
    /// child when the parent call is cancelled.
    async fn await_child(
        self: &Arc<Self>,
        parent_session: &Arc<SessionRt>,
        sess: &Arc<SessionRt>,
        task_row: &TaskRow,
        cancel: CancellationToken,
    ) -> Result<RunOutcome, String> {
        let root = task_row.root.as_str();
        let child = task_row.child_session.as_str();
        let key = format!("subagent:{}", task_row.parent_call);
        let mut settled = sess.settled.subscribe();
        loop {
            if let Some(outcome) = self.child_outcome(child, &key) {
                return Ok(outcome);
            }
            if let Some(why) = sess.failure() {
                // A failed child cannot publish settlement. Keep the parent's replay-safe
                // call open too, so explicit recovery can resume the same child and result.
                self.storage_failure(parent_session, &format!("subagent storage error: {why}"));
                return Err(format!(
                    "Subagent {} stopped after a storage error",
                    task_row.child
                ));
            }
            tokio::select! {
                _ = cancel.cancelled() => {
                    if sess.removed.is_cancelled() { return Err("The conversation was removed".into()); }
                    if let Err(error) = self.stop(child).await {
                        // A failed Stop commit cancels execution through storage_failure,
                        // but cannot produce a durable settlement. Wait for execution to
                        // unwind, not for a settlement that storage cannot record. In
                        // particular, do not label this still-open child "aborted".
                        let _ = tokio::time::timeout(Duration::from_secs(10), async {
                            while sess.active_run().is_some() {
                                tokio::time::sleep(Duration::from_millis(10)).await;
                            }
                        }).await;
                        return Err(format!("Subagent {} could not confirm Stop: {}", task_row.child, error.message));
                    }
                    // Wait briefly for the child to settle as stopped.
                    tokio::select! {
                        _ = sess.removed.cancelled() => return Err("The conversation was removed".into()),
                        _ = tokio::time::timeout(Duration::from_secs(10), settled.recv()) => {}
                    }
                    self.set_task_status(root, child, "aborted", None);
                    return Err(format!("Subagent {} failed: aborted", task_row.child));
                }
                r = settled.recv() => {
                    if r.is_err() {
                        settled = sess.settled.subscribe();
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(500)) => {}
            }
        }
    }

    /// The settled (or withdrawn: stopped before it ran) outcome of the child's task key.
    fn child_outcome(&self, child: &str, key: &str) -> Option<RunOutcome> {
        let req = self.store.request(child, key).ok().flatten()?;
        match req.state {
            RequestState::Settled | RequestState::Withdrawn => req.outcome,
            _ => None,
        }
    }

    /// Record how the child ended on its task row (once: a row already finished keeps its
    /// status) and say what the parent should hear.
    fn record_outcome(&self, task_row: &TaskRow, outcome: &RunOutcome) -> ChildEnd {
        let root = task_row.root.as_str();
        let child = task_row.child_session.as_str();
        let current = self
            .store
            .read(|c| Store::task_by_session(c, child))
            .ok()
            .flatten()
            .unwrap_or_else(|| task_row.clone());
        let n = task_row.child;
        if *outcome == RunOutcome::Completed {
            let answer = match current.finished() {
                true => current.result.clone().unwrap_or_default(),
                false => {
                    let answer = self.answer_text(child);
                    self.set_task_status(root, child, "done", Some(&answer));
                    answer
                }
            };
            return ChildEnd::Answered(answer);
        }
        let status = status_for(outcome);
        if !current.finished() {
            self.set_task_status(root, child, status, None);
        }
        let message = match outcome {
            RunOutcome::Aborted => match &current.note {
                Some(note) => format!(
                    "Subagent {n} was stopped before it answered ({note}). Its result will not arrive; do not start it again unless asked."
                ),
                None => format!("Subagent {n} failed: aborted"),
            },
            RunOutcome::ModelError { detail }
            | RunOutcome::NoModel { detail }
            | RunOutcome::Interrupted { detail }
            | RunOutcome::Failed { detail } => {
                format!("Subagent {n} failed: unanswered ({detail})")
            }
            RunOutcome::Completed => unreachable!(),
        };
        ChildEnd::Unanswered(message)
    }

    // ------------------------------------------------------------------ background children

    /// Attach (once) a watcher that records the child's settlement and delivers it.
    fn watch_child(self: &Arc<Self>, task: TaskRow) {
        {
            let mut watchers = self
                .subagent_watchers
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if !watchers.insert(task.child_session.clone()) {
                return;
            }
        }
        let shared = self.clone();
        self.spawn(async move {
            shared.watch_child_run(&task).await;
            shared
                .subagent_watchers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&task.child_session);
        });
    }

    async fn watch_child_run(self: &Arc<Self>, task: &TaskRow) {
        let child = task.child_session.as_str();
        let key = format!("subagent:{}", task.parent_call);
        let Ok(sess) = self.session(child) else {
            return;
        };
        if !task.finished() {
            // After a restart the child's run is resumed by startup recovery; this is the
            // same idempotent step, in case the watcher gets there first.
            if let Err(e) = self.recover_session(child) {
                log::warn!(
                    "subagent {child}: cannot recover for watching: {}",
                    e.message
                );
                return;
            }
            let mut settled = sess.settled.subscribe();
            let outcome = loop {
                if let Some(outcome) = self.child_outcome(child, &key) {
                    break outcome;
                }
                if sess.failure().is_some() {
                    // Storage failed; explicit recovery re-attaches a watcher later.
                    return;
                }
                tokio::select! {
                    _ = sess.removed.cancelled() => return,
                    _ = self.stopping.cancelled() => return,
                    r = settled.recv() => {
                        if r.is_err() {
                            settled = sess.settled.subscribe();
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                }
            };
            let _ = self.record_outcome(task, &outcome);
        }
        self.subagent_deliver(child).await;
    }

    /// Deliver a finished background child's completion once: clients already saw
    /// `SubagentsChanged`; a model-origin child launched with `wake` gets one follow-up unless
    /// the run that launched it was stopped by the person (`suppressed`).
    async fn subagent_deliver(self: &Arc<Self>, child_session: &str) {
        let Ok(Some(task)) = self
            .store
            .read(|c| Store::task_by_session(c, child_session))
        else {
            return;
        };
        if !task.finished() || !task.delivery_pending() {
            return;
        }
        let stopped = task.origin == "model"
            && self
                .store
                .tool_call(&task.parent_session, &task.parent_call)
                .ok()
                .flatten()
                .and_then(|c| c.run)
                .and_then(|run| {
                    self.store
                        .read(|c| Store::run_outcome(c, run))
                        .ok()
                        .flatten()
                })
                .is_some_and(|outcome| outcome.contains("aborted"));
        let delivery = if task.delivery != "wake" || task.origin != "model" {
            "notified"
        } else if stopped {
            "suppressed"
        } else {
            "woken"
        };
        if delivery == "woken" {
            let key = format!("subagent:{}:done", task.child);
            let text = self.subagent_wake_text(&task);
            match self
                .enqueue(&task.parent_session, &key, InputMode::FollowUp, &text, &[])
                .await
            {
                Ok(_) => {}
                Err(e) if e.code == ErrorCode::Conflict => {}
                Err(e) => {
                    log::warn!(
                        "subagent {}/{}: completion could not wake the model: {}",
                        task.root,
                        task.child,
                        e.message
                    );
                    self.set_task_delivery(&task, "notified");
                    return;
                }
            }
        }
        self.set_task_delivery(&task, delivery);
    }

    fn set_task_delivery(&self, task: &TaskRow, delivery: &str) {
        let _ = tokio::task::block_in_place(|| {
            self.store.tx(|tx| {
                Store::set_task_delivery(tx, &task.child_session, delivery, now_ms())?;
                Store::bump(tx, &task.root, now_ms())
            })
        });
        self.publish_subagents(&task.root);
    }

    fn subagent_wake_text(&self, task: &TaskRow) -> String {
        let excerpt: String = task
            .task
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .take(80)
            .collect();
        let mut text = match task.status.as_str() {
            "done" => format!("Background subagent #{} ({excerpt}) finished.", task.child),
            "aborted" => format!(
                "Background subagent #{} ({excerpt}) was stopped{}.",
                task.child,
                match &task.note {
                    Some(note) => format!(" ({note})"),
                    None => String::new(),
                }
            ),
            _ => format!(
                "Background subagent #{} ({excerpt}) failed without an answer.",
                task.child
            ),
        };
        if let Some(answer) = task.result.as_deref().filter(|a| !a.trim().is_empty()) {
            let shown: String = answer.chars().take(WAKE_ANSWER_CHARS).collect();
            let cut = answer.chars().count() > WAKE_ANSWER_CHARS;
            text.push_str(&format!(
                "\nAnswer{}:\n{}",
                if cut { " (beginning)" } else { "" },
                shown.trim_end()
            ));
            if cut {
                text.push_str(&format!(
                    "\nUse subagent_result with child {} for the full answer.",
                    task.child
                ));
            }
        }
        text.push_str(&artifact_lines(&self.child_artifacts(&task.child_session)));
        text
    }

    /// What a child produced beyond its answer: the files its settled `write` and `edit`
    /// calls touched (resolved against its cwd, in first-touched order) and the web sources
    /// it retained, bounded by [`MAX_SUBAGENT_ARTIFACTS`].
    pub(super) fn child_artifacts(&self, child_session: &str) -> Vec<SubagentArtifact> {
        let mut out = Vec::new();
        let cwd = self
            .store
            .session(child_session)
            .ok()
            .flatten()
            .map(|r| std::path::PathBuf::from(r.cwd))
            .unwrap_or_default();
        let calls = self
            .store
            .read(|c| Store::tool_calls_done_named(c, child_session, &["write", "edit"]))
            .unwrap_or_default();
        for call in calls {
            let Some(path) = call.arguments.get("path").and_then(Value::as_str) else {
                continue;
            };
            // A call whose result was an error changed nothing.
            let failed = call.result_entry.is_some_and(|id| {
                matches!(
                    self.store
                        .entry(child_session, id)
                        .ok()
                        .flatten()
                        .map(|e| e.kind),
                    Some(EntryKind::Message {
                        message: Message::ToolResult { is_error: true, .. },
                        ..
                    })
                )
            });
            if failed {
                continue;
            }
            let reference = cwd.join(path).display().to_string();
            if out
                .iter()
                .any(|a: &SubagentArtifact| a.kind == "file" && a.reference == reference)
            {
                continue;
            }
            out.push(SubagentArtifact {
                kind: "file".into(),
                reference,
                label: None,
            });
            if out.len() >= MAX_SUBAGENT_ARTIFACTS {
                return out;
            }
        }
        for source in self.web_sources_bounded(child_session).unwrap_or_default() {
            if out.len() >= MAX_SUBAGENT_ARTIFACTS {
                break;
            }
            out.push(SubagentArtifact {
                kind: "source".into(),
                reference: source.final_url,
                label: source.title,
            });
        }
        out
    }

    /// Startup: re-attach watchers to background children and finish cut-off deliveries.
    pub(super) fn subagents_recover(self: &Arc<Self>) {
        let Ok(rows) = self.store.tasks_background() else {
            return;
        };
        for task in rows {
            if self.require_native_execution(&task.parent_session).is_ok()
                && self.require_native_execution(&task.child_session).is_ok()
            {
                self.watch_child(task);
            }
        }
    }

    // ---------------------------------------------------------------------- person controls

    /// The task row for `(root, child)`, refusing a number from any other session.
    fn authorized_child(&self, root: &str, child: u64) -> Result<TaskRow, ProtoError> {
        self.store
            .session(root)?
            .ok_or_else(|| ProtoError::new(ErrorCode::NotFound, "Unknown session"))?;
        self.store
            .read(|c| Store::task_by_child(c, root, child))?
            .ok_or_else(|| ProtoError::new(ErrorCode::NotFound, "Not a subagent of this session"))
    }

    fn subagent_row(&self, root: &str, child: u64) -> Result<SubagentRow, ProtoError> {
        Ok(row_view(&self.authorized_child(root, child)?))
    }

    /// Stop one child on the person's behalf. The note is durable before the child's own
    /// Stop; the task settles through the child's run (its waiting parent or watcher
    /// records `aborted`), except for a child with nothing running, which is settled here.
    pub async fn stop_subagent(
        self: &Arc<Self>,
        root: &str,
        child: u64,
        _request: &str,
    ) -> Result<SubagentRow, ProtoError> {
        let task = self.authorized_child(root, child)?;
        if task.finished() {
            return Ok(row_view(&task));
        }
        let session = task.child_session.clone();
        self.store.tx(|tx| {
            Store::set_task_note(tx, &session, STOPPED_BY_PERSON, now_ms())?;
            Store::bump(tx, root, now_ms())
        })?;
        self.publish_subagents(root);
        match self.stop(&session).await? {
            Reply::Stopping { run: None } => {
                self.set_task_status(root, &session, "aborted", None);
                let shared = self.clone();
                self.spawn(async move { shared.subagent_deliver(&session).await });
            }
            _ => {
                // Settlement is recorded by whoever waits on the child. Nobody may be
                // (an inline child whose parent has not replayed yet): make sure of it.
                let shared = self.clone();
                self.spawn(async move { shared.settle_stopped_child(&session).await });
            }
        }
        self.subagent_row(root, child)
    }

    /// After a person's Stop: once the child's key settles, record it if nobody else did.
    async fn settle_stopped_child(self: &Arc<Self>, child_session: &str) {
        let Ok(Some(task)) = self
            .store
            .read(|c| Store::task_by_session(c, child_session))
        else {
            return;
        };
        let key = format!("subagent:{}", task.parent_call);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            if let Ok(Some(now)) = self
                .store
                .read(|c| Store::task_by_session(c, child_session))
                && now.finished()
            {
                break;
            }
            if let Some(outcome) = self.child_outcome(child_session, &key) {
                let _ = self.record_outcome(&task, &outcome);
                break;
            }
            if tokio::time::Instant::now() >= deadline || self.stopping.is_cancelled() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if task.background() {
            self.subagent_deliver(child_session).await;
        }
    }

    /// Give a running child a steering message under the durable key `steer:<request>`.
    pub async fn steer_subagent(
        self: &Arc<Self>,
        root: &str,
        child: u64,
        request: &str,
        text: &str,
    ) -> Result<SubagentRow, ProtoError> {
        let task = self.authorized_child(root, child)?;
        if task.finished() {
            return Err(ProtoError::new(
                ErrorCode::Invalid,
                format!("Subagent {child} has already finished"),
            ));
        }
        if task.note.is_some() {
            return Err(ProtoError::new(
                ErrorCode::Invalid,
                format!("Subagent {child} is being stopped"),
            ));
        }
        if text.trim().is_empty() {
            return Err(ProtoError::new(
                ErrorCode::Invalid,
                "The steering message is empty",
            ));
        }
        self.enqueue(
            &task.child_session,
            &format!("steer:{request}"),
            InputMode::Steer,
            text,
            &[],
        )
        .await?;
        self.subagent_row(root, child)
    }

    /// Launch a background child on the person's behalf: notification-only, idempotent per
    /// `request` (the child's call id is `user:<request>`).
    pub async fn launch_subagent(
        self: &Arc<Self>,
        session: &str,
        request: &str,
        task: String,
        profile: Option<String>,
        context: Option<String>,
    ) -> Result<SubagentRow, ProtoError> {
        let invalid = |m: String| ProtoError::new(ErrorCode::Invalid, m);
        if request.is_empty() || request.len() > 256 {
            return Err(invalid("The request key is missing or too long.".into()));
        }
        if task.trim().is_empty() {
            return Err(invalid("The task is empty.".into()));
        }
        if task.len() > MAX_LAUNCH_TASK_BYTES {
            return Err(ProtoError::new(
                ErrorCode::TooLarge,
                "The task is too long for a subagent.",
            ));
        }
        let context = match context.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(mode) => Some(ContextMode::parse(mode).ok_or_else(|| {
                invalid(format!(
                    "context must be \"fresh\" or \"inherit\", not {mode:?}"
                ))
            })?),
        };
        let parent_session = self.session(session)?;
        let launch = SubagentLaunch {
            task,
            profile: profile
                .map(|p| p.trim().to_owned())
                .filter(|p| !p.is_empty()),
            context,
            wait: false,
            wake: false,
            model: None,
        };
        let call_id = format!("user:{request}");
        let (row, created) = self
            .create_child(session, &call_id, launch, "user", "notify")
            .map_err(invalid)?;
        if created {
            self.publish_subagents(&row.root);
        }
        self.admit_child(&parent_session, &row)
            .await
            .map_err(invalid)?;
        self.watch_child(row.clone());
        self.subagent_row(&row.root, row.child)
    }

    pub fn subagent_result(
        &self,
        root: &str,
        child: u64,
        offset: usize,
    ) -> Result<SubagentResultView, ProtoError> {
        let task = self.authorized_child(root, child)?;
        let mut view = result_view(&task);
        if let Some(answer) = view.result.take() {
            let (piece, total, more) =
                crate::web::slice_chars(&answer, offset, SUBAGENT_RESULT_INLINE_CHARS);
            view.result = Some(piece);
            view.result_chars = total as u64;
            view.result_offset = offset.min(total) as u64;
            view.result_truncated = more;
        }
        view.artifacts = self.child_artifacts(&task.child_session);
        Ok(view)
    }

    /// Wait (bounded) for a child to finish, then its result; a still-running child is
    /// reported as it is at the deadline or on cancellation.
    pub async fn subagent_wait(
        &self,
        root: &str,
        child: u64,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<SubagentResultView, ProtoError> {
        let deadline =
            tokio::time::Instant::now() + timeout.min(Duration::from_secs(MAX_WAIT_SECS));
        loop {
            let task = self.authorized_child(root, child)?;
            if task.finished() {
                return self.subagent_result(root, child, 0);
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(250)) => {}
                _ = tokio::time::sleep_until(deadline) => return Ok(result_view(&task)),
                _ = cancel.cancelled() => return Ok(result_view(&task)),
            }
        }
    }

    /// Concatenated text blocks of the child's final assistant reply.
    pub(super) fn answer_text(&self, child: &str) -> String {
        let (entries, _) = self.store.page(child, None, 50).unwrap_or_default();
        entries
            .iter()
            .rev()
            .find_map(|e| match &e.kind {
                EntryKind::Message {
                    message: m @ Message::Assistant { .. },
                    partial: false,
                    ..
                } => Some(m.text()),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// Stop the children of `run`'s subagent calls that have no result yet. Used when the
    /// parent will never wait for them again (its Stop was settled without the run executing
    /// here), so a child must not run on unobserved.
    pub(super) fn abort_orphaned_children(
        self: &Arc<Self>,
        parent: &str,
        run: u64,
    ) -> Result<(), ProtoError> {
        let calls = tokio::task::block_in_place(|| {
            self.store
                .read(|c| Store::tools_in_state(c, parent, "intent"))
        })?;
        let mut failure = None;
        for call in calls
            .iter()
            .filter(|c| c.run == Some(run) && c.name == "subagent")
        {
            if let Err(error) = self.abort_child(parent, &call.call_id) {
                // Still cancel the other children; none may continue unobserved merely
                // because an earlier child's Stop could not be persisted.
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    /// Stop the child of one subagent call (if it has one and it is still running) and mark
    /// its task aborted. The child's own Stop is durable before its run is settled.
    pub(super) fn abort_child(
        self: &Arc<Self>,
        parent: &str,
        call_id: &str,
    ) -> Result<(), ProtoError> {
        let Some(task) = self
            .store
            .read(|c| Store::task_for_call(c, parent, call_id))?
        else {
            return Ok(());
        };
        let child = task.child_session.clone();
        let sess = self.session(&child)?;
        let stopped = self.store.request_stop(&child, now_ms());
        if stopped.is_ok() {
            for token in sess
                .manual_compactions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .values()
            {
                token.cancel();
            }
            self.compaction_recovery.notify_one();
        }
        match stopped {
            Ok(Some((run, _))) => {
                let active = sess
                    .active
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_ref()
                    .filter(|a| a.run == run)
                    .map(|a| a.cancel.clone());
                match active {
                    Some(cancel) => cancel.cancel(),
                    None => {
                        // The Stop is durable now. Recovery can clear a prior storage
                        // failure and settle this stopped run without restarting tools.
                        self.recover_session(&child)?;
                    }
                }
            }
            Ok(None) => {}
            Err(e) => {
                // Failed persistence still fails execution closed; it cannot justify an
                // aborted task label or let the parent retire the unresolved call.
                self.storage_failure(&sess, &e.to_string());
                if let Ok(parent_session) = self.session(parent) {
                    self.storage_failure(&parent_session, &e.to_string());
                }
                return Err(e.into());
            }
        }
        if !matches!(task.status.as_str(), "done" | "aborted" | "failed") {
            self.store.tx(|tx| {
                Store::set_task_status(tx, &child, "aborted", None, now_ms())?;
                Store::bump(tx, &task.root, now_ms())
            })?;
            self.publish_subagents(&task.root);
        }
        Ok(())
    }

    fn set_task_status(&self, root: &str, child: &str, status: &str, result: Option<&str>) {
        let _ = tokio::task::block_in_place(|| {
            self.store.tx(|tx| {
                Store::set_task_status(tx, child, status, result, now_ms())?;
                Store::bump(tx, root, now_ms())
            })
        });
        self.publish_subagents(root);
    }

    pub fn subagents_view(&self, root: &str) -> Result<SubagentsView, ProtoError> {
        let row = self
            .store
            .session(root)?
            .ok_or_else(|| ProtoError::new(ErrorCode::NotFound, "Unknown session"))?;
        let (tasks, truncated) = self
            .store
            .tasks(root, limits_rows())
            .map_err(ProtoError::from)?;
        Ok(SubagentsView {
            enabled: row.subagents_enabled,
            children: tasks.iter().map(row_view).collect(),
            truncated,
        })
    }

    pub fn publish_subagents(&self, root: &str) {
        let _publication = self.publication.lock().unwrap_or_else(|e| e.into_inner());
        // Revision first: never label an older view with a newer revision. A concurrent write
        // will publish again; publishers serialize and re-read after taking this lock.
        let revision = self
            .store
            .session(root)
            .ok()
            .flatten()
            .map_or(0, |s| s.revision);
        if let Ok(view) = self.subagents_view(root) {
            self.publish(Event::Session {
                session: root.to_owned(),
                revision,
                change: SessionChange::SubagentsChanged { subagents: view },
            });
        }
    }

    /// A child's history page, after checking it belongs to `root` (ownership guard).
    pub fn subagent_history(
        &self,
        root: &str,
        child: u64,
        before: Option<u64>,
        limit: u32,
    ) -> Result<Reply, ProtoError> {
        let task = self
            .store
            .read(|c| Store::task_by_child(c, root, child))?
            .ok_or_else(|| {
                ProtoError::new(ErrorCode::NotFound, "Not a subagent of this session")
            })?;
        let (entries, has_older) = self.store.page(
            &task.child_session,
            before,
            limit.clamp(1, pipkin_engine_protocol::limits::MAX_PAGE_ENTRIES),
        )?;
        Ok(Reply::Page {
            entries,
            has_older,
            native_history_start: self.store.native_history_start(&task.child_session)?,
        })
    }
}

fn limits_rows() -> u32 {
    pipkin_engine_protocol::limits::MAX_SUBAGENT_ROWS
}

/// What the parent hears when its child ends.
enum ChildEnd {
    Answered(String),
    Unanswered(String),
}

fn excerpt(task: &str) -> String {
    task.chars().take(SUBAGENT_TASK_EXCERPT_CHARS).collect()
}

pub(super) fn row_view(t: &TaskRow) -> SubagentRow {
    SubagentRow {
        child: t.child,
        task_id: t.child,
        call_id: t.parent_call.clone(),
        task: excerpt(&t.task),
        status: t.status.clone(),
        session: t.child_session.clone(),
        profile: t
            .launch
            .as_deref()
            .and_then(LaunchRecord::from_json)
            .map(|l| l.profile),
        origin: t.origin.clone(),
        background: t.background(),
        delivery: t.delivery.clone(),
        note: t.note.clone(),
        has_result: t.status == "done" && t.result.is_some(),
    }
}

/// The row as a result view with the whole answer; `Shared::subagent_result` slices it
/// and attaches artifacts.
fn result_view(t: &TaskRow) -> SubagentResultView {
    let result = match t.status.as_str() {
        "done" => t.result.clone(),
        _ => None,
    };
    SubagentResultView {
        child: t.child,
        status: t.status.clone(),
        task: excerpt(&t.task),
        result_chars: result.as_ref().map_or(0, |r| r.chars().count() as u64),
        result,
        note: t.note.clone(),
        origin: t.origin.clone(),
        delivery: t.delivery.clone(),
        session: t.child_session.clone(),
        result_offset: 0,
        result_truncated: false,
        artifacts: vec![],
    }
}

/// The answer a waiting parent receives inline: at most [`SUBAGENT_RESULT_INLINE_CHARS`],
/// with the rest reachable by offset, and the child's artifacts listed after it.
fn inline_answer(child: u64, answer: &str, artifacts: &[SubagentArtifact]) -> String {
    let (piece, total, more) = crate::web::slice_chars(answer, 0, SUBAGENT_RESULT_INLINE_CHARS);
    let mut text = piece;
    if more {
        let shown = text.chars().count();
        text.push_str(&format!(
            "\n[Answer continues: {} of {total} characters shown; read the rest with subagent_result child {child} offset {shown}.]",
            shown
        ));
    }
    text.push_str(&artifact_lines(artifacts));
    text
}

/// "Artifacts:" lines for a result (empty when there are none).
pub(crate) fn artifact_lines(artifacts: &[SubagentArtifact]) -> String {
    if artifacts.is_empty() {
        return String::new();
    }
    let mut text = String::from("\nArtifacts:");
    for a in artifacts {
        text.push_str(&format!("\n- {}: {}", a.kind, a.reference));
        if let Some(label) = &a.label {
            text.push_str(&format!(" ({label})"));
        }
    }
    text
}
