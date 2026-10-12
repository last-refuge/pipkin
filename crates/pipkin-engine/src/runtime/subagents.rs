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

use std::sync::Arc;

use pipkin_engine_protocol::limits::SUBAGENT_TASK_EXCERPT_CHARS;
use pipkin_engine_protocol::*;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::Shared;
use crate::profiles::{self, ContextMode, LaunchRecord};
use crate::provider::{BoxFuture, ToolSpec};
use crate::store::{Store, TaskRow};
use crate::tools::{SubagentLaunch, Tool, ToolCtx, ToolError, ToolHost, ToolOutput};
use crate::util::now_ms;

pub const SUBAGENT_DESCRIPTION: &str = "Delegate a self-contained task to a subagent and get its answer back. Give it everything it needs to know: by default it starts without this conversation (context \"fresh\"); context \"inherit\" adds a bounded excerpt of recent messages. Profiles: \"worker\" (default: every tool a child can have), \"reviewer\" (read-only: read, plan_read, web_read), \"scout\" (reads plus web search and fetch; cannot change the workspace), or a profile defined in settings. A child never delegates further or asks the person.";

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
                    "context": {"type": "string", "description": "fresh (default) or inherit: whether the child sees a bounded excerpt of this conversation"}
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
            let launch = SubagentLaunch {
                task: task.to_owned(),
                profile,
                context,
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
            Ok(service
                .search(&self.parent, call_id, query, count, cancel)
                .await)
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
            Ok(service.fetch(&self.parent, call_id, url, cancel).await)
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
            Ok(self.shared.mcp.call(qualified, arguments, cancel).await)
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
        let parent_row = self
            .store
            .session(parent)
            .map_err(|e| e.to_string())?
            .ok_or("The parent session no longer exists")?;
        let root = parent_row.root.clone().unwrap_or_else(|| parent.to_owned());
        let child_id = format!("{parent}:{call_id}");
        // An existing child (replay) keeps the policy it was launched with: nothing below
        // is re-resolved for it.
        let existing = self
            .store
            .read(|c| Store::task_for_call(c, parent, call_id))
            .map_err(|e| e.to_string())?;
        let (task_row, created): (TaskRow, bool) = match existing {
            Some(row) => (row, false),
            None => {
                // Resolve the profile, model, context and caps against the parent's settings.
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
                let model = profile.model.clone().or_else(|| parent_row.model.clone());
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
                let outcome: Result<Result<TaskRow, String>, _> = tokio::task::block_in_place(
                    || {
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
                        let (row, _) = Store::create_task(
                            tx, &root, parent, call_id, &child_id, &prompt, now,
                        )?;
                        Store::set_task_launch(tx, &child_id, &launch_json)?;
                        Store::bump(tx, &root, now)?;
                        Ok(Ok(Store::task_by_session(tx, &child_id)?.unwrap_or(row)))
                    })
                    },
                );
                match outcome.map_err(|e| e.to_string())? {
                    Ok(row) => (row, true),
                    Err(refused) => return Err(refused),
                }
            }
        };
        if created {
            self.publish_subagents(&root);
        }
        let task = task_row.task.as_str();
        let child = task_row.child_session.clone();
        let details = json!({"conversationId": child});
        let key = format!("subagent:{call_id}");
        let parent_session = self.session(parent).map_err(|e| e.message)?;
        let sess = self.session(&child).map_err(|e| e.message)?;
        let mut settled = sess.settled.subscribe();
        // Reopening the parent also reopens its existing durable child. After an in-process
        // storage failure, admission alone only finds the old key; it does not resume it.
        if let Err(error) = self.recover_session(&child) {
            if error.code == ErrorCode::Storage {
                self.storage_failure(&parent_session, &error.message);
            }
            return Err(error.message);
        }
        // Idempotent admission: a replay finds the original.
        match self.prompt(&child, &key, task, &[]).await {
            Ok(_) => {}
            Err(e) if e.code == ErrorCode::Busy => {}
            Err(e) => {
                if e.code == ErrorCode::Storage {
                    // Recovery may still be waiting for the preceding child execution to
                    // unwind. Do not turn that unresolved call into a final error result.
                    self.storage_failure(&parent_session, &e.message);
                }
                return Err(format!("Subagent {} failed: {}", task_row.child, e.message));
            }
        }
        self.set_task_status(&root, &child, "running", None);
        // Wait for this key's settlement (it may already be settled).
        let outcome = loop {
            if let Ok(Some(req)) = self.store.request(&child, &key)
                && let Some(outcome) = req.outcome.clone()
                && req.state == crate::store::RequestState::Settled
            {
                break outcome;
            }
            if let Some(why) = sess.failure() {
                // A failed child cannot publish settlement. Keep the parent's replay-safe
                // call open too, so explicit recovery can resume the same child and result.
                self.storage_failure(&parent_session, &format!("subagent storage error: {why}"));
                return Err(format!(
                    "Subagent {} stopped after a storage error",
                    task_row.child
                ));
            }
            tokio::select! {
                _ = cancel.cancelled() => {
                    if sess.removed.is_cancelled() { return Err("The conversation was removed".into()); }
                    if let Err(error) = self.stop(&child).await {
                        // A failed Stop commit cancels execution through storage_failure,
                        // but cannot produce a durable settlement. Wait for execution to
                        // unwind, not for a settlement that storage cannot record. In
                        // particular, do not label this still-open child "aborted".
                        let _ = tokio::time::timeout(std::time::Duration::from_secs(10), async {
                            while sess.active_run().is_some() {
                                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                            }
                        }).await;
                        return Err(format!("Subagent {} could not confirm Stop: {}", task_row.child, error.message));
                    }
                    // Wait briefly for the child to settle as stopped.
                    tokio::select! {
                        _ = sess.removed.cancelled() => return Err("The conversation was removed".into()),
                        _ = tokio::time::timeout(std::time::Duration::from_secs(10), settled.recv()) => {}
                    }
                    self.set_task_status(&root, &child, "aborted", None);
                    return Err(format!("Subagent {} failed: aborted", task_row.child));
                }
                r = settled.recv() => {
                    if r.is_err() {
                        settled = sess.settled.subscribe();
                    }
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
            }
        };
        let status = status_for(&outcome);
        if outcome != RunOutcome::Completed {
            self.set_task_status(&root, &child, status, None);
            return Err(format!(
                "Subagent {} failed: {}",
                task_row.child,
                match &outcome {
                    RunOutcome::Aborted => "aborted".to_owned(),
                    RunOutcome::ModelError { detail }
                    | RunOutcome::NoModel { detail }
                    | RunOutcome::Interrupted { detail }
                    | RunOutcome::Failed { detail } => format!("unanswered ({detail})"),
                    RunOutcome::Completed => unreachable!(),
                }
            ));
        }
        let answer = self.answer_text(&child);
        self.set_task_status(&root, &child, status, Some(&answer));
        Ok((answer, details))
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
            children: tasks
                .into_iter()
                .map(|t| SubagentRow {
                    child: t.child,
                    task_id: t.child,
                    call_id: t.parent_call,
                    task: t.task.chars().take(SUBAGENT_TASK_EXCERPT_CHARS).collect(),
                    status: t.status,
                    session: t.child_session,
                    profile: t
                        .launch
                        .as_deref()
                        .and_then(LaunchRecord::from_json)
                        .map(|l| l.profile),
                })
                .collect(),
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
