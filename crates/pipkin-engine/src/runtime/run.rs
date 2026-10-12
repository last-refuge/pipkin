//! The durable run loop.
//!
//! The loop is driven by committed state, not by memory, so a resumed run (after a crash or a
//! storage failure) continues exactly where the durable record says:
//!
//! 1. If the run's newest assistant message has tool calls without results, execute them
//!    (intent committed before any effect; unconfirmed intents become `interrupted` results
//!    unless replay-safe), then apply the post-tools boundary (queued steers join the run).
//! 2. Else, if that assistant message is a final answer, settle the run `Completed` and start
//!    the next run from the queue.
//! 3. Else, request the model. Streamed progress is committed in batches before it is
//!    published; the final message replaces the partial one in one commit with its usage.
//!
//! Stop cancels the token: the provider exchange and tools end, and the run settles `Aborted`
//! (its partial reply finalized as aborted, its inputs retired from future model context).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

mod progress;

use pipkin_engine_protocol::*;
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use super::Shared;
use super::session::{Active, SessionRt, user_entry};
use crate::config::{QueueDelivery, Settings};
use crate::provider::{
    CacheRetention, ProviderRequest, StreamCtx, ThinkingLevel, ToolSpec, errors,
};
use crate::store::{RequestRow, Store, ToolState};
use crate::tools::{self, Tool, ToolCtx};
use crate::util::now_ms;

/// A streamed delta is committed and published within this long of arriving. It leaves room
/// for the durable commit itself inside the 50 ms chunk-to-client budget (plan §7)...
const FLUSH_INTERVAL: Duration = Duration::from_millis(40);
/// ...or once this many bytes of deltas are pending.
const FLUSH_BYTES: usize = 16 * 1024;

/// A request ready to send, with what it was built from.
pub(super) struct Prepared {
    /// Resolved transport/options without messages. Generation derives the model context;
    /// compaction estimates the raw entries or supplies its own summary request instead.
    pub request_template: ProviderRequest,
    pub adapter: Arc<dyn crate::provider::ApiAdapter>,
    pub model: crate::provider::Model,
    /// The session's settings, with compaction's per-model overrides already applied.
    pub settings: Settings,
    pub entries: Vec<Entry>,
    /// The engine HTTP client as of this request (rebuilt when network settings change).
    pub http: reqwest::Client,
    pub net: crate::provider::http::NetSettings,
}

enum Step {
    Tools(Vec<(String, String, Value)>),
    Boundary(u64),
    Final,
    Retry(i64),
    Error(u64, i64, Box<Message>),
    Failed(String),
    Request,
}

impl Shared {
    /// Start executing `run` in the background.
    pub fn spawn_run(self: &Arc<Self>, sess: Arc<SessionRt>, run: u64) {
        let cancel = sess.removed.child_token();
        {
            let mut active = sess.active.lock().unwrap_or_else(|e| e.into_inner());
            if active.as_ref().is_some_and(|a| a.run == run) {
                return;
            }
            *active = Some(Active {
                run,
                cancel: cancel.clone(),
            });
        }
        let shared = self.clone();
        self.spawn(async move {
            let outcome = shared.drive(&sess, run, &cancel).await;
            // Release temporary context buffers before reporting an idle/settled run.
            shared.reclaim_idle_pages();
            {
                let mut active = sess.active.lock().unwrap_or_else(|e| e.into_inner());
                if active.as_ref().is_some_and(|a| a.run == run) {
                    *active = None;
                }
            }
            if let Some(outcome) = outcome {
                let continue_queue = outcome == RunOutcome::Completed;
                if shared.settle(&sess, run, outcome).is_ok() {
                    super::faults::crash_point("after-settlement-before-queue");
                    if continue_queue {
                        shared.next_from_queue(&sess).await;
                    }
                }
            }
        });
    }

    /// Run until the run should settle; `None` when it must stay unsettled (storage failure).
    async fn drive(
        self: &Arc<Self>,
        sess: &Arc<SessionRt>,
        run: u64,
        cancel: &CancellationToken,
    ) -> Option<RunOutcome> {
        loop {
            if cancel.is_cancelled() {
                return (sess.failure().is_none()).then_some(RunOutcome::Aborted);
            }
            let step = match self.next_step(&sess.id, run) {
                Ok(s) => s,
                Err(e) => {
                    log::error!("session {}: cannot read run {run}: {e}", sess.id);
                    return None;
                }
            };
            match step {
                Step::Final => return Some(RunOutcome::Completed),
                Step::Tools(calls) => {
                    if let Err(e) = self.tool_round(sess, run, calls, cancel).await {
                        log::error!("session {}: tool round failed: {e}", sess.id);
                        return None;
                    }
                    if cancel.is_cancelled() {
                        continue;
                    }
                    super::faults::crash_point("after_tools_before_boundary");
                }
                Step::Boundary(assistant) => {
                    let mode = self.settings_for(&sess.id).steering_mode;
                    if self.place_queued(sess, run, assistant, mode).is_err() {
                        return None;
                    }
                    super::faults::crash_point("after_tool_boundary");
                }
                Step::Retry(until) => {
                    let remaining = until.saturating_sub(now_ms()).max(0) as u64;
                    tokio::select! {
                        _ = cancel.cancelled() => {}
                        _ = tokio::time::sleep(Duration::from_millis(remaining)) => {}
                    }
                }
                Step::Failed(detail) => return Some(RunOutcome::ModelError { detail }),
                Step::Error(entry, at_ms, message) => {
                    if self
                        .classify_error(sess, run, entry, at_ms, &message, cancel)
                        .await
                        .is_err()
                    {
                        return None;
                    }
                }
                Step::Request if self.compact_if_needed(sess, run, cancel).await => continue,
                Step::Request => {
                    if sess.failure().is_some() || self.stopping.is_cancelled() {
                        return None;
                    }
                    if cancel.is_cancelled() {
                        continue;
                    }
                    match self.request(sess, run, cancel).await {
                        Ok(_) => {}
                        Err(Some(outcome)) => return Some(outcome),
                        Err(None) => return None,
                    }
                }
            }
        }
    }

    /// Classify each committed error exactly once. The checkpoint precedes waiting and is
    /// retained while the next request streams, so crashing cannot replenish the retry budget.
    async fn classify_error(
        self: &Arc<Self>,
        sess: &Arc<SessionRt>,
        run: u64,
        entry: u64,
        at_ms: i64,
        message: &Message,
        cancel: &CancellationToken,
    ) -> Result<(), ProtoError> {
        let mut state =
            tokio::task::block_in_place(|| self.store.read(|c| Store::generation_state(c, run)))
                .map_err(ProtoError::from)?;
        let settings = self.settings_for(&sess.id);
        let window = self
            .auth
            .catalog()
            .find(&format!(
                "{}/{}",
                message_provider(message),
                message_model(message)
            ))
            .map_or(0, |m| m.context_window);
        let compaction_key = format!("overflow:{run}:{entry}");
        let admitted = self.store.read(|c| {
            Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM compactions WHERE session = ?1 AND operation_key = ?2)",
                rusqlite::params![sess.id, compaction_key], |r| r.get::<_,bool>(0))?)
        }).map_err(ProtoError::from)?;
        let overflow = errors::is_context_overflow(message, window);
        if !state.overflow_compacted && (admitted || (settings.compaction.enabled && overflow)) {
            let result = self
                .compact_key(sess, Some(run), &compaction_key, None, cancel)
                .await;
            if let Some(error) = sess.failure() {
                return Err(ProtoError::new(ErrorCode::Storage, error));
            }
            if self.stopping.is_cancelled() {
                return Err(ProtoError::new(
                    ErrorCode::Cancelled,
                    "Engine shutting down",
                ));
            }
            state.overflow_compacted = true;
            if let Ok(super::compaction::Compacted::Done(_)) = result {
                state.handled_error = Some(entry);
                state.retry_until = Some(now_ms());
                return self.commit(sess, |tx| {
                    Store::save_generation_state(tx, run, &state)?;
                    Ok(((), vec![]))
                });
            }
        }
        state.failures = state.failures.saturating_add(1);
        state.handled_error = Some(entry);
        let retry = &settings.retry;
        state.retry_until = (!overflow
            && retry.enabled
            && state.failures <= retry.max_retries
            && errors::is_retryable(message))
        .then(|| {
            let delay = errors::retry_delay_ms(
                state.failures,
                retry.base_delay_ms,
                retry.max_agent_delay_ms,
            );
            at_ms.saturating_add(i64::try_from(delay).unwrap_or(i64::MAX))
        });
        self.commit(sess, |tx| {
            Store::save_generation_state(tx, run, &state)?;
            Ok(((), vec![]))
        })?;
        super::faults::crash_point("after_retry_checkpoint");
        Ok(())
    }

    /// What the committed record says to do next.
    fn next_step(&self, session: &str, run: u64) -> crate::store::Result<Step> {
        tokio::task::block_in_place(|| {
            self.store.read(|c| {
                let entries = Store::entries_of_run(c, session, run)?;
                let mut answered: HashSet<String> = HashSet::new();
                for e in &entries {
                    if let EntryKind::Message {
                        message: Message::ToolResult { tool_call_id, .. },
                        ..
                    } = &e.kind
                    {
                        answered.insert(tool_call_id.clone());
                    }
                }
                let last_message = entries.iter().rev().find(|e| {
                    matches!(e.kind, EntryKind::Message { partial: false, .. })
                        && !matches!(
                            &e.kind,
                            EntryKind::Message {
                                message: Message::ToolResult { .. },
                                ..
                            }
                        )
                });
                let Some(last) = last_message else {
                    return Ok(Step::Request);
                };
                match &last.kind {
                    EntryKind::Message {
                        message: message @ Message::Assistant { stop_reason, .. },
                        ..
                    } => {
                        let pending: Vec<(String, String, Value)> = message
                            .tool_calls()
                            .into_iter()
                            .filter(|(id, _, _)| !answered.contains(*id))
                            .map(|(id, name, args)| (id.to_owned(), name.to_owned(), args.clone()))
                            .collect();
                        let has_calls = !message.tool_calls().is_empty();
                        // Only a tool-use response starts a round. Providers can include
                        // tool-shaped content in a final stop/length response; preserve it
                        // in history without granting execution authority.
                        if !pending.is_empty() && *stop_reason == StopReason::ToolUse {
                            return Ok(Step::Tools(pending));
                        }
                        if *stop_reason == StopReason::Error {
                            let state = Store::generation_state(c, run)?;
                            if state.handled_error != Some(last.id) {
                                return Ok(Step::Error(
                                    last.id,
                                    last.at_ms,
                                    Box::new(message.clone()),
                                ));
                            }
                            return Ok(match state.retry_until {
                                Some(until) if until > now_ms() => Step::Retry(until),
                                Some(_) => Step::Request,
                                None => Step::Failed(match message {
                                    Message::Assistant { error_message, .. } => error_message
                                        .clone()
                                        .unwrap_or_else(|| "Model response ended with stop reason error".into()),
                                    _ => unreachable!(),
                                }),
                            });
                        }
                        if *stop_reason == StopReason::ToolUse && has_calls {
                            let placed: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM tool_boundaries WHERE run = ?1 AND assistant = ?2)",
                                rusqlite::params![run, last.id], |r| r.get(0))?;
                            if !placed { return Ok(Step::Boundary(last.id)); }
                        }
                        match stop_reason {
                            StopReason::Stop | StopReason::Length => Ok(Step::Final),
                            StopReason::ToolUse if !has_calls => Ok(Step::Final),
                            // An adapter abort without our cancellation intent is a terminal
                            // model failure, not permission to silently make another request.
                            StopReason::Aborted => Ok(Step::Failed(match message {
                                Message::Assistant { error_message, .. } => error_message
                                    .clone().unwrap_or_else(|| "Model response ended with stop reason aborted".into()),
                                _ => unreachable!(),
                            })),
                            // The tool round's results are recorded: ask the model again.
                            _ => Ok(Step::Request),
                        }
                    }
                    _ => Ok(Step::Request),
                }
            })
        })
    }

    pub fn settings_for(&self, session: &str) -> Settings {
        let cwd = self
            .store
            .session(session)
            .ok()
            .flatten()
            .map(|s| std::path::PathBuf::from(s.cwd));
        Settings::load(&self.config.config_dir, cwd.as_deref()).0
    }

    /// One model exchange. The final message moves into durable publication; the caller
    /// reads the committed state on its next step. `Err(Some)` settles the run; `Err(None)`
    /// leaves it unsettled after a storage failure.
    async fn request(
        self: &Arc<Self>,
        sess: &Arc<SessionRt>,
        run: u64,
        cancel: &CancellationToken,
    ) -> Result<(), Option<RunOutcome>> {
        let Prepared {
            request_template: mut req,
            adapter,
            model,
            http,
            net,
            entries,
            ..
        } = self.prepare(sess).await?;
        req.messages = super::glue::model_messages(&entries);
        drop(entries);
        let _permit = tokio::select! {
            p = self.generations.clone().acquire_owned() => p.ok(),
            _ = cancel.cancelled() => return Ok(()),
        };
        let progress = progress::Progress::new(
            self.clone(),
            sess.clone(),
            run,
            model.clone(),
            cancel.clone(),
        );
        let fut = adapter.stream(StreamCtx {
            net: Some(&net),
            req: &req,
            http: &http,
            events: Some(progress.sink()),
            cancel: cancel.clone(),
        });
        tokio::pin!(fut);
        let message = loop {
            let wait = progress.wait_time();
            tokio::select! {
                m = &mut fut => break m,
                _ = progress.changed.notified() => {},
                _ = tokio::time::sleep(wait.unwrap_or(FLUSH_INTERVAL)), if wait.is_some() => progress.flush(),
            }
        };
        let partial = progress.finish();
        if sess.failure().is_some() {
            return Err(None);
        }
        // The final message carries everything; uncommitted deltas need no separate commit.
        let session = sess.id.clone();
        let usage_key = model.key();
        let is_error = matches!(
            &message,
            Message::Assistant {
                stop_reason: StopReason::Error,
                ..
            }
        );
        let usage = match &message {
            Message::Assistant { usage, .. }
                if usage.total_tokens > 0 || usage.cost.total > 0.0 =>
            {
                Some(usage.clone())
            }
            _ => None,
        };
        let committed = self.commit(sess, move |tx| {
            let now = now_ms();
            let mut message = message;
            Store::disambiguate_tool_ids(tx, &session, &mut message)?;
            let kind = EntryKind::Message {
                message,
                mode: InputMode::Prompt,
                request: None,
                partial: false,
            };
            let change = match partial {
                Some(id) => {
                    let entry = Entry {
                        id,
                        at_ms: now,
                        run: Some(run),
                        retired: false,
                        kind,
                    };
                    Store::replace_entry(tx, &session, &entry)?;
                    SessionChange::EntryReplaced { entry }
                }
                None => SessionChange::EntryAppended {
                    entry: Store::append_entry_owned(tx, &session, now, Some(run), kind)?,
                },
            };
            if !is_error {
                Store::reset_generation_state(tx, run)?;
            }
            let mut changes = vec![change];
            if let Some(usage) = &usage {
                Store::add_usage(tx, &session, "model", &usage_key, usage)?;
                changes.push(SessionChange::UsageChanged {
                    usage: Store::usage_in(tx, &session)?,
                });
            }
            Ok(((), changes))
        });
        match committed {
            Ok(()) => {
                super::faults::crash_point("after_generation_result");
                Ok(())
            }
            Err(_) => Err(None),
        }
    }

    /// Resolve transport/options and the raw context range. Do not construct a second
    /// copy of history until the caller actually needs outbound model messages.
    pub(super) async fn prepare(
        self: &Arc<Self>,
        sess: &Arc<SessionRt>,
    ) -> Result<Prepared, Option<RunOutcome>> {
        let row = match self.store.session(&sess.id) {
            Ok(Some(r)) => r,
            _ => return Err(None),
        };
        let catalog = self.auth.catalog();
        let Some(key) = row.model.clone() else {
            return Err(Some(RunOutcome::NoModel {
                detail: "No model is configured".into(),
            }));
        };
        let Some(model) = catalog.find(&key).cloned() else {
            return Err(Some(RunOutcome::NoModel {
                detail: format!("Model {key} is not available"),
            }));
        };
        let Some(adapter) = self.adapters.get(&model.api) else {
            return Err(Some(RunOutcome::ModelError {
                detail: format!("No adapter for API {} (model {key})", model.api),
            }));
        };
        let mut settings = Settings::load(
            &self.config.config_dir,
            Some(std::path::Path::new(&row.cwd)),
        )
        .0;
        settings.compaction = settings
            .compaction
            .for_model(&model.key())
            .map_err(|detail| Some(RunOutcome::ModelError { detail }))?;
        let timeout = settings
            .idle_timeout()
            .map_err(|detail| Some(RunOutcome::ModelError { detail }))?;
        // Network settings may have changed since the last request.
        let (net, http) = self.refresh_net_snapshot();
        if let Some(detail) = &net.validation_error {
            return Err(Some(RunOutcome::ModelError {
                detail: detail.clone(),
            }));
        }
        let proxy = net.proxy.clone();
        if let Err(detail) = unusable_proxy(&proxy, &model.base_url) {
            return Err(Some(RunOutcome::ModelError { detail }));
        }
        let auth = match self.auth.resolve(&model, &http).await {
            Ok(a) => a,
            Err(e) => {
                return Err(Some(RunOutcome::ModelError {
                    detail: e.message().to_owned(),
                }));
            }
        };
        let mut entries = match tokio::task::block_in_place(|| self.store.context_entries(&sess.id))
        {
            Ok(e) => e,
            Err(_) => return Err(None),
        };
        let tools = self.tools_for(&row);
        let specs: Vec<ToolSpec> = tools.iter().map(|t| t.spec()).collect();
        let mut desired = self.system_sections(sess, &row, &specs, &settings);
        // The plan summary is a positional section: it changes between turns without
        // touching the cached prompt resources, and is patched like any other section.
        if row.parent.is_some()
            && let Some(instructions) = self.child_launch(&sess.id).and_then(|l| l.instructions)
        {
            desired.insert("profile".into(), serde_json::Value::String(instructions));
        }
        if let Some(plan) = self.plan_section(&sess.id) {
            desired.insert("plan".into(), serde_json::Value::String(plan));
        }
        let declarations = specs
            .iter()
            .map(pipkin_engine_protocol::ToolDeclaration::from)
            .collect::<Vec<_>>();
        let now = now_ms();
        let plan = crate::context::system::plan(&entries, &desired, &declarations, now);
        if !plan.messages.is_empty() {
            let appended = self
                .commit(sess, |tx| {
                    let mut changes = Vec::new();
                    Store::retire_entries(tx, &sess.id, &plan.retire)?;
                    for id in &plan.retire {
                        if let Some(entry) = Store::entry_in(tx, &sess.id, *id)? {
                            changes.push(SessionChange::EntryReplaced { entry });
                        }
                    }
                    let mut appended = Vec::new();
                    for system in &plan.messages {
                        let kind = EntryKind::Message {
                            message: Message::System(system.clone()),
                            mode: InputMode::Prompt,
                            request: None,
                            partial: false,
                        };
                        let entry = Store::append_entry(tx, &sess.id, now, None, &kind)?;
                        changes.push(SessionChange::EntryAppended {
                            entry: entry.clone(),
                        });
                        appended.push(entry);
                    }
                    Ok((appended, changes))
                })
                .map_err(|_| None)?;
            for entry in &mut entries {
                if plan.retire.contains(&entry.id) {
                    entry.retired = true;
                }
            }
            entries.extend(appended);
        }
        let system = String::new();
        let thinking = row
            .thinking
            .as_deref()
            .and_then(ThinkingLevel::parse)
            .map(|l| model.clamp_thinking(l))
            .unwrap_or(ThinkingLevel::Off);
        let provider_session = self.provider_session_id(&sess.id).map_err(|error| {
            Some(RunOutcome::ModelError {
                detail: format!("Cannot persist provider session identity: {error}"),
            })
        })?;
        let mut options = Map::new();
        options.insert(
            "maxRetries".into(),
            settings.retry.provider.max_retries.into(),
        );
        options.insert(
            "maxRetryDelayMs".into(),
            settings.retry.provider.max_retry_delay_ms.into(),
        );
        // Provider-specific knobs the reference reads from settings.
        for key in ["transport", "thinkingBudgets", "websocketConnectTimeoutMs"] {
            if let Some(v) = settings.other.get(key) {
                options.insert(key.into(), v.clone());
            }
        }
        let mut req = ProviderRequest {
            model: model.clone(),
            system,
            messages: Vec::new(),
            tools: specs,
            thinking,
            session_id: provider_session,
            max_tokens: None,
            cache_retention: if self.config.env.get("PI_CACHE_RETENTION").as_deref() == Some("long")
            {
                CacheRetention::Long
            } else {
                CacheRetention::Short
            },
            credential: auth.credential,
            headers: auth.headers,
            base_url: auth.base_url,
            timeout: timeout.unwrap_or(Duration::from_secs(i32::MAX as u64)),
            env: auth.env,
            options,
        };
        // An isolated engine shows adapters only its own variables, never the process's.
        if !self.config.env.ambient {
            for (k, v) in &self.config.env.vars {
                req.env.entry(k.clone()).or_insert_with(|| v.clone());
            }
            req.options.insert(
                crate::provider::ISOLATED_ENV.into(),
                serde_json::Value::Bool(true),
            );
        }
        if let Err(e) = crate::provider::api::cloudflare::prepare(&mut req) {
            return Err(Some(RunOutcome::ModelError { detail: e }));
        }
        if let Err(detail) = unusable_proxy(&proxy, req.base_url()) {
            return Err(Some(RunOutcome::ModelError { detail }));
        }
        // Transports outside the shared client (Codex WebSockets) read the proxy from the
        // request env; a credential's own proxy variables win, as in the reference.
        for (key, value) in proxy.env_pairs() {
            if !req.env.contains_key(&key) && !req.env.contains_key(&key.to_ascii_uppercase()) {
                req.env.insert(key, value);
            }
        }
        Ok(Prepared {
            request_template: req,
            adapter,
            model,
            settings,
            entries,
            http,
            net,
        })
    }

    /// Commit pending deltas (creating the partial entry on first use), then publish them.
    fn flush(
        &self,
        sess: &SessionRt,
        run: u64,
        model: &crate::provider::Model,
        partial: &mut Option<u64>,
        pending: &mut Vec<Delta>,
    ) -> Result<(), ProtoError> {
        let deltas = std::mem::take(pending);
        let session = sess.id.clone();
        let existing = *partial;
        let id = self.commit(sess, |tx| {
            let mut changes = Vec::new();
            let id = match existing {
                Some(id) => id,
                None => {
                    let kind = EntryKind::Message {
                        message: Message::Assistant {
                            content: vec![],
                            api: model.api.clone(),
                            provider: model.provider.clone(),
                            model: model.id.clone(),
                            usage: Usage::default(),
                            stop_reason: StopReason::Stop,
                            error_message: None,
                            timestamp: now_ms(),
                            extra: Map::new(),
                        },
                        mode: InputMode::Prompt,
                        request: None,
                        partial: true,
                    };
                    let entry = Store::append_entry(tx, &session, now_ms(), Some(run), &kind)?;
                    let id = entry.id;
                    changes.push(SessionChange::EntryAppended { entry });
                    id
                }
            };
            Store::append_chunks(tx, &session, id, &deltas)?;
            changes.extend(
                deltas
                    .iter()
                    .cloned()
                    .map(|delta| SessionChange::EntryDelta { id, delta }),
            );
            Ok((id, changes))
        })?;
        *partial = Some(id);
        Ok(())
    }

    /// Execute one round of tool calls (in parallel; writes to one path are serialized).
    async fn tool_round(
        self: &Arc<Self>,
        sess: &Arc<SessionRt>,
        run: u64,
        calls: Vec<(String, String, Value)>,
        cancel: &CancellationToken,
    ) -> Result<(), ProtoError> {
        let row = self
            .store
            .session(&sess.id)?
            .ok_or_else(|| ProtoError::new(ErrorCode::NotFound, "session vanished"))?;
        let offered = self.tools_for(&row);
        let find = |name: &str| offered.iter().find(|t| t.spec().name == name).cloned();
        // Reference `generation.ts:558-561`: parallel unless an offered tool in this round is
        // sequential. Calls run in their committed order either way.
        let sequential = calls
            .iter()
            .any(|(_, name, _)| find(name).is_some_and(|t| t.sequential()));
        let mut result = Ok(());
        let mut handles = Vec::new();
        for (call_id, name, args) in calls {
            if sequential && cancel.is_cancelled() {
                // Unstarted calls of a stopped sequential round get an aborted result when
                // the run settles.
                break;
            }
            let shared = self.clone();
            let sess = sess.clone();
            let call_cancel = cancel.child_token();
            let tool = find(&name);
            let cwd = row.cwd.clone();
            let model = row.model.clone();
            let thinking = row.thinking.clone();
            let task = tokio::spawn(async move {
                shared
                    .one_tool(
                        &sess,
                        run,
                        &call_id,
                        &name,
                        args,
                        tool,
                        cwd,
                        model,
                        thinking,
                        call_cancel,
                    )
                    .await
            });
            if sequential {
                Self::joined(task.await, &mut result);
            } else {
                handles.push(task);
            }
        }
        for h in handles {
            Self::joined(h.await, &mut result);
        }
        result
    }

    fn joined(
        outcome: Result<Result<(), ProtoError>, tokio::task::JoinError>,
        result: &mut Result<(), ProtoError>,
    ) {
        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(e)) => *result = Err(e),
            Err(e) => {
                *result = Err(ProtoError::new(
                    ErrorCode::Internal,
                    format!("tool task failed: {e}"),
                ))
            }
        }
    }

    async fn one_tool(
        self: &Arc<Self>,
        sess: &Arc<SessionRt>,
        run: u64,
        call_id: &str,
        name: &str,
        args: Value,
        tool: Option<Arc<dyn Tool>>,
        cwd: String,
        model: Option<String>,
        thinking: Option<String>,
        cancel: CancellationToken,
    ) -> Result<(), ProtoError> {
        let session = sess.id.clone();
        // An intent committed before a restart: the effect may have happened.
        let prior = self.store.tool_call(&session, call_id)?;
        let Some(tool) = tool.or_else(|| {
            // A replay-safe call may recover even when new calls of its tool are no longer
            // offered (subagents disabled); its own tool recovers it, never another.
            prior
                .as_ref()
                .filter(|p| p.replay_safe && self.replay_allowed(p))
                .and_then(|p| self.recoverable_tool(&p.name))
        }) else {
            let result = tools::unavailable(name);
            self.record_tool_result(sess, run, call_id, name, &result, ToolState::Done)?;
            return Ok(());
        };
        if let Some(prior) = &prior
            && !(prior.replay_safe && tool.replay_safe() && self.replay_allowed(prior))
        {
            let output = self
                .store
                .read(|c| Store::tool_progress(c, &session, call_id))?;
            let result = tools::recovered_output(name, &output, false);
            self.record_tool_result(sess, run, call_id, name, &result, ToolState::Interrupted)?;
            return Ok(());
        }
        if prior.is_none() {
            let contract = serde_json::to_string(&contract_of(&tool)).expect("serializes");
            let replay = tool.replay_safe();
            let s = session.clone();
            let args_c = args.clone();
            self.commit(sess, |tx| {
                Store::tool_intent(tx, &s, call_id, Some(run), name, &args_c, replay, now_ms())?;
                Store::kv_set(tx, &s, &format!("tool_contract:{call_id}"), &contract)?;
                Ok(((), vec![]))
            })?;
        }
        let shared = self.clone();
        let progress_session = session.clone();
        let progress_call = call_id.to_owned();
        let ctx = ToolCtx {
            cwd: std::path::PathBuf::from(&cwd),
            session: session.clone(),
            call_id: call_id.to_owned(),
            cancel: cancel.clone(),
            progress: Arc::new(move |text: &str| {
                shared.publish(Event::Session {
                    session: progress_session.clone(),
                    revision: 0,
                    change: SessionChange::ToolProgress {
                        call_id: progress_call.clone(),
                        text: text.to_owned(),
                    },
                });
            }),
            env: tool_env(&session, call_id, model.as_deref(), thinking.as_deref()),
            shell: self
                .settings_for(&session)
                .shell_path
                .map(std::path::PathBuf::from),
            locks: self.locks.clone(),
            processes: self.processes.clone(),
            host: Some(Arc::new(super::subagents::Host {
                shared: self.clone(),
                parent: session.clone(),
            })),
        };
        let result = tools::run(tool.as_ref(), args, &ctx).await;
        self.record_tool_result(sess, run, call_id, name, &result, ToolState::Done)?;
        Ok(())
    }

    /// Recovery authority for a replay-safe call: the tool must be one that recovers its own
    /// calls ([`Shared::recoverable_tool`]) and the recorded contract must equal the current
    /// tool's (name, description, parameters, replay).
    pub fn replay_allowed(&self, call: &crate::store::ToolCallRow) -> bool {
        let Some(tool) = self.recoverable_tool(&call.name) else {
            return false;
        };
        let recorded = self
            .store
            .kv_get(&call.session, &format!("tool_contract:{}", call.call_id))
            .ok()
            .flatten();
        let current = serde_json::to_string(&contract_of(&tool)).expect("serializes");
        recorded.as_deref() == Some(current.as_str())
    }

    /// Place queued inputs. Post-tools: steers join the run. Final: the caller starts a new run.
    fn place_queued(
        &self,
        sess: &SessionRt,
        run: u64,
        assistant: u64,
        mode: QueueDelivery,
    ) -> Result<(), ProtoError> {
        let session = sess.id.clone();
        self.commit(sess, |tx| {
            let queue = Store::queue_in(tx, &session)?;
            if Store::run_in(tx, run)?.is_none_or(|r| r.stop_requested || r.view().is_none()) {
                return Ok(((), vec![]));
            }
            let inserted = tx.execute(
                "INSERT OR IGNORE INTO tool_boundaries(run, assistant) VALUES (?1, ?2)",
                rusqlite::params![run, assistant],
            )?;
            if inserted == 0 {
                return Ok(((), vec![]));
            }
            let mut changes = Self::flush_summary_writes(tx, &session)?;
            let chosen = choose(&queue, mode, false, QueueDelivery::OneAtATime);
            if chosen.is_empty() {
                return Ok(((), changes));
            }
            let keys: Vec<String> = chosen.iter().map(|r| r.key.clone()).collect();
            Store::consume_queued(tx, &session, &keys, run)?;
            let now = now_ms();
            for row in &chosen {
                let entry = Store::append_entry(tx, &session, now, Some(run), &user_entry(row))?;
                changes.push(SessionChange::EntryAppended { entry });
            }
            changes.push(SessionChange::QueueChanged {
                queue: Store::queue_view(&Store::queue_in(tx, &session)?),
            });
            Ok(((), changes))
        })
    }

    /// After successful settlement: start the next run from queued inputs. A failed
    /// run leaves its queue waiting for explicit new input, including across restart.
    pub async fn next_from_queue(self: &Arc<Self>, sess: &Arc<SessionRt>) {
        let _guard = sess.lock.lock().await;
        if sess.active_run().is_some() {
            return;
        }
        let settings = self.settings_for(&sess.id);
        let session = sess.id.clone();
        let started = self.commit(sess, |tx| {
            if Store::open_run_in(tx, &session)?.is_some() {
                return Ok((None, vec![]));
            }
            use rusqlite::OptionalExtension;
            let last: Option<Option<String>> = tx
                .query_row(
                    "SELECT outcome FROM runs WHERE session = ?1 ORDER BY id DESC LIMIT 1",
                    [&session],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(outcome) = last
                && outcome
                    .as_deref()
                    .and_then(|v| serde_json::from_str::<RunOutcome>(v).ok())
                    != Some(RunOutcome::Completed)
            {
                return Ok((None, vec![]));
            }
            Self::start_queued_run(tx, &session, &settings)
        });
        if let Ok(Some(run)) = started {
            self.spawn_run(sess.clone(), run);
        }
    }

    /// Shared final-boundary selection, also used atomically with a new idle admission.
    pub(super) fn start_queued_run(
        tx: &rusqlite::Connection,
        session: &str,
        settings: &Settings,
    ) -> crate::store::Result<(Option<u64>, Vec<SessionChange>)> {
        let queue = Store::queue_in(tx, session)?;
        let chosen = choose(
            &queue,
            settings.steering_mode,
            true,
            settings.follow_up_mode,
        );
        let Some(first) = chosen.first() else {
            return Ok((None, vec![]));
        };
        let mut changes = Self::flush_summary_writes(tx, session)?;
        let now = now_ms();
        let run = Store::start_run(tx, session, Some(&first.key), now)?;
        let keys: Vec<_> = chosen.iter().map(|r| r.key.clone()).collect();
        Store::consume_queued(tx, session, &keys, run)?;
        for row in &chosen {
            let entry = Store::append_entry(tx, session, now, Some(run), &user_entry(row))?;
            changes.push(SessionChange::EntryAppended { entry });
        }
        changes.push(SessionChange::QueueChanged {
            queue: Store::queue_view(&Store::queue_in(tx, session)?),
        });
        changes.push(SessionChange::RunChanged {
            run: Store::run_in(tx, run)?.and_then(|r| r.view()),
        });
        Ok((Some(run), changes))
    }

    /// The provider's cache/affinity id for this session (stable across restarts).
    fn provider_session_id(&self, session: &str) -> crate::store::Result<String> {
        if let Some(id) = self.store.kv_get(session, "provider_session_id")? {
            return Ok(id);
        }
        self.store
            .kv_get_or_insert(session, "provider_session_id", &crate::util::uuid_v4())
    }
}

/// Steers per the steering mode; follow-ups (when allowed) per the follow-up mode.
fn choose(
    queue: &[RequestRow],
    steering: QueueDelivery,
    follow_ups: bool,
    follow_mode: QueueDelivery,
) -> Vec<RequestRow> {
    let mut out: Vec<RequestRow> = Vec::new();
    let steers = queue.iter().filter(|r| r.mode == InputMode::Steer);
    match steering {
        QueueDelivery::All => out.extend(steers.cloned()),
        QueueDelivery::OneAtATime => out.extend(steers.take(1).cloned()),
    }
    if follow_ups {
        let follows = queue.iter().filter(|r| r.mode == InputMode::FollowUp);
        match follow_mode {
            QueueDelivery::All => out.extend(follows.cloned()),
            QueueDelivery::OneAtATime => out.extend(follows.take(1).cloned()),
        }
    }
    out.sort_by_key(|r| r.queue_entry);
    out
}

fn contract_of(tool: &Arc<dyn Tool>) -> Value {
    let spec = tool.spec();
    serde_json::json!({
        "name": spec.name,
        "description": spec.description,
        "parameters": spec.parameters,
        "replay": if tool.replay_safe() { "safe" } else { "unsafe" },
    })
}

/// Session details exposed to shell commands (reference `PI_*` variables), plus the call's
/// marker so a restarted engine can reap what an interrupted call left running.
fn tool_env(
    session: &str,
    call_id: &str,
    model: Option<&str>,
    thinking: Option<&str>,
) -> Vec<(String, String)> {
    let mut env = vec![
        ("PI_SESSION_ID".to_owned(), session.to_owned()),
        (
            super::faults::TOOL_CALL_ENV.to_owned(),
            super::faults::tool_call_token(session, call_id),
        ),
    ];
    if let Some((provider, id)) = model.and_then(|m| m.split_once('/')) {
        env.push(("PI_PROVIDER".into(), provider.into()));
        env.push(("PI_MODEL".into(), id.into()));
    }
    if let Some(t) = thinking {
        env.push(("PI_REASONING_LEVEL".into(), t.into()));
    }
    env
}

/// A model request whose proxy is configured but unusable (SOCKS, PAC, malformed) fails with
/// the reference message instead of bypassing the proxy (`resolveHttpProxyUrlForTarget`).
pub(super) fn unusable_proxy(
    proxy: &crate::provider::proxy::ProxyConfig,
    base_url: &str,
) -> Result<(), String> {
    match url::Url::parse(base_url) {
        Ok(target) => proxy.proxy_for(&target).map(|_| ()),
        Err(_) => Ok(()),
    }
}

fn message_provider(m: &Message) -> &str {
    match m {
        Message::Assistant { provider, .. } => provider,
        _ => "",
    }
}

fn message_model(m: &Message) -> &str {
    match m {
        Message::Assistant { model, .. } => model,
        _ => "",
    }
}

impl Shared {
    /// Growing context creates short-lived allocations in many size classes. glibc otherwise
    /// retains their free pages in worker arenas. Return them at the run boundary, after the
    /// request future (and its buffers) has been dropped, before publishing settlement.
    fn reclaim_idle_pages(&self) {
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        tokio::task::block_in_place(|| {
            // SAFETY: malloc_trim is thread-safe, takes no pointers, and only releases free pages.
            unsafe {
                libc::malloc_trim(0);
            }
        });
    }
}

#[cfg(test)]
mod queue_reference_tests {
    use super::*;

    #[test]
    fn user_selection_matches_executed_reference_boundaries() {
        let reference: Value = serde_json::from_str(include_str!(
            "../../../../fixtures/native-queue/inbox-reference.json"
        ))
        .unwrap();
        for case in reference["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            if name == "stop" {
                continue;
            }
            let queue: Vec<_> = case["items"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| item["mode"] != "write")
                .map(|item| RequestRow {
                    session: "s".into(),
                    key: item["id"].to_string(),
                    fingerprint: String::new(),
                    mode: if item["mode"] == "steer" {
                        InputMode::Steer
                    } else {
                        InputMode::FollowUp
                    },
                    state: crate::store::RequestState::Queued,
                    queue_entry: item["id"].as_u64(),
                    run: None,
                    outcome: None,
                    text: item["content"][0]["text"].as_str().unwrap().into(),
                    images: vec![],
                    model: None,
                    created_at_ms: 0,
                })
                .collect();
            let mode = if name == "final-all" {
                QueueDelivery::All
            } else {
                QueueDelivery::OneAtATime
            };
            let chosen = choose(&queue, mode, name.starts_with("final"), mode);
            let actual: Vec<_> = chosen.iter().map(|r| r.text.as_str()).collect();
            let expected: Vec<_> = case["placed"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|e| e["user"].as_str())
                .collect();
            assert_eq!(actual, expected, "{name}");
        }
    }
}
