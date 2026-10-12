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
use std::time::{Duration, Instant};

use pipkin_engine_protocol::*;
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use super::Shared;
use super::session::{Active, SessionRt, user_entry};
use crate::config::{QueueDelivery, Settings};
use crate::provider::{
    CacheRetention, ProviderRequest, StreamCtx, StreamEvent, ThinkingLevel, ToolSpec, errors,
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
    pub req: ProviderRequest,
    pub adapter: Arc<dyn crate::provider::ApiAdapter>,
    pub model: crate::provider::Model,
    /// The session's settings, with compaction's per-model overrides already applied.
    pub settings: Settings,
    pub entries: Vec<Entry>,
    /// The engine HTTP client as of this request (rebuilt when network settings change).
    pub http: reqwest::Client,
}

enum Step {
    Tools(Vec<(String, String, Value)>),
    Final,
    Retry(i64),
    Error(u64, i64, Box<Message>),
    Failed(String),
    Request,
}

impl Shared {
    /// Start executing `run` in the background.
    pub fn spawn_run(self: &Arc<Self>, sess: Arc<SessionRt>, run: u64) {
        let cancel = CancellationToken::new();
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
            if let Some(outcome) = outcome
                && shared.settle(&sess, run, outcome).is_ok()
            {
                shared.next_from_queue(&sess).await;
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
                    let settings = self.settings_for(&sess.id);
                    if self
                        .place_queued(sess, run, settings.steering_mode, false)
                        .is_err()
                    {
                        return None;
                    }
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
                Step::Request => match self.request(sess, run, cancel).await {
                    Ok(_) => {}
                    Err(Some(outcome)) => return Some(outcome),
                    Err(None) => return None,
                },
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
        if !state.overflow_compacted
            && settings.compaction.enabled
            && errors::is_context_overflow(message, window)
        {
            state.overflow_compacted = true;
            self.commit(sess, |tx| {
                Store::save_generation_state(tx, run, &state)?;
                Ok(((), vec![]))
            })?;
            if let Ok(super::compaction::Compacted::Done(_)) =
                self.compact(sess, Some(run), None, cancel).await
            {
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
        state.retry_until =
            (retry.enabled && state.failures <= retry.max_retries && errors::is_retryable(message))
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
                        if !pending.is_empty()
                            && *stop_reason != StopReason::Aborted
                            && *stop_reason != StopReason::Error
                        {
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
                                        .unwrap_or_else(|| "The model failed.".into()),
                                    _ => unreachable!(),
                                }),
                            });
                        }
                        match stop_reason {
                            StopReason::Stop | StopReason::Length => Ok(Step::Final),
                            StopReason::ToolUse if !has_calls => Ok(Step::Final),
                            // Results recorded or an interrupted partial: ask the model again.
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

    /// One model exchange. `Ok(Some(message))` is the committed final message; `Err(Some)`
    /// settles the run (no model, no credentials); `Err(None)` leaves it unsettled.
    async fn request(
        self: &Arc<Self>,
        sess: &Arc<SessionRt>,
        run: u64,
        cancel: &CancellationToken,
    ) -> Result<Option<Message>, Option<RunOutcome>> {
        let Prepared {
            req,
            adapter,
            model,
            http,
            ..
        } = self.prepare(sess).await?;
        let _permit = tokio::select! {
            p = self.generations.clone().acquire_owned() => p.ok(),
            _ = cancel.cancelled() => return Ok(None),
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();
        let fut = adapter.stream(StreamCtx {
            req: &req,
            http: &http,
            events: Some(tx),
            cancel: cancel.clone(),
        });
        tokio::pin!(fut);
        let mut partial: Option<u64> = None;
        let mut pending: Vec<Delta> = Vec::new();
        let mut pending_bytes = 0usize;
        // When the oldest uncommitted delta arrived: the flush deadline runs from it.
        let mut oldest: Option<Instant> = None;
        let message = loop {
            let wait = oldest.map_or(FLUSH_INTERVAL, |t| {
                FLUSH_INTERVAL.saturating_sub(t.elapsed())
            });
            tokio::select! {
                m = &mut fut => break m,
                Some(StreamEvent::Delta(d)) = rx.recv() => {
                    pending_bytes += match &d {
                        Delta::Text { text, .. } | Delta::Thinking { text, .. } => text.len(),
                        Delta::ToolCall { .. } => 64,
                    };
                    pending.push(d);
                    oldest.get_or_insert_with(Instant::now);
                    if pending_bytes >= FLUSH_BYTES && self.flush(sess, run, &model, &mut partial, &mut pending).is_err() {
                        cancel.cancel();
                    }
                    if pending.is_empty() { pending_bytes = 0; oldest = None; }
                }
                _ = tokio::time::sleep(wait), if !pending.is_empty() => {
                    if self.flush(sess, run, &model, &mut partial, &mut pending).is_err() {
                        cancel.cancel();
                    }
                    pending_bytes = 0;
                    oldest = None;
                }
            }
        };
        if sess.failure().is_some() {
            return Err(None);
        }
        // The final message carries everything; uncommitted deltas need no separate commit.
        let session = sess.id.clone();
        let usage_key = model.key();
        let final_message = message.clone();
        let committed = self.commit(sess, move |tx| {
            let now = now_ms();
            let kind = EntryKind::Message {
                message: final_message.clone(),
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
                    entry: Store::append_entry(tx, &session, now, Some(run), &kind)?,
                },
            };
            if !matches!(
                &final_message,
                Message::Assistant {
                    stop_reason: StopReason::Error,
                    ..
                }
            ) {
                Store::reset_generation_state(tx, run)?;
            }
            let mut changes = vec![change];
            if let Message::Assistant { usage, .. } = &final_message
                && (usage.total_tokens > 0 || usage.cost.total > 0.0)
            {
                Store::add_usage(tx, &session, "model", &usage_key, usage)?;
                changes.push(SessionChange::UsageChanged {
                    usage: usage_in(tx, &session)?,
                });
            }
            Ok(((), changes))
        });
        match committed {
            Ok(()) => {
                super::faults::crash_point("after_generation_result");
                Ok(Some(message))
            }
            Err(_) => Err(None),
        }
    }

    /// Resolve the model, adapter, credential and context for the next request.
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
        // Network settings may have changed since the last request.
        let http = self.refresh_net();
        let proxy = self.proxy_config();
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
        let mut settings = Settings::load(
            &self.config.config_dir,
            Some(std::path::Path::new(&row.cwd)),
        )
        .0;
        settings.compaction = settings.compaction.for_model(&model.key());
        let mut entries = match tokio::task::block_in_place(|| self.store.context_entries(&sess.id))
        {
            Ok(e) => e,
            Err(_) => return Err(None),
        };
        let tools = self.tools_for(&row);
        let specs: Vec<ToolSpec> = tools.iter().map(|t| t.spec()).collect();
        let desired = self.system_sections(sess, &row, &specs, &settings);
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
        let messages = super::glue::model_messages(&entries);
        let system = String::new();
        let thinking = row
            .thinking
            .as_deref()
            .and_then(ThinkingLevel::parse)
            .map(|l| model.clamp_thinking(l))
            .unwrap_or(ThinkingLevel::Off);
        let provider_session = self.provider_session_id(&sess.id);
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
            messages,
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
            timeout: settings
                .idle_timeout()
                .unwrap_or(Duration::from_secs(i32::MAX as u64)),
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
            req,
            adapter,
            model,
            settings,
            entries,
            http,
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
            // A replay-safe subagent call may recover even when new calls are disabled.
            prior
                .as_ref()
                .filter(|p| p.replay_safe && self.replay_allowed(p))
                .map(|_| self.subagent_tool())
        }) else {
            let result = tools::unavailable(name);
            self.record_tool_result(sess, run, call_id, name, &result, ToolState::Done)?;
            return Ok(());
        };
        if let Some(prior) = &prior
            && !(prior.replay_safe && tool.replay_safe() && self.replay_allowed(prior))
        {
            let result = tools::interrupted(name, "");
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

    /// Recovery authority for a replay-safe call: the recorded contract must equal the current
    /// tool's (name, description, parameters, replay).
    pub fn replay_allowed(&self, call: &crate::store::ToolCallRow) -> bool {
        if call.name != "subagent" {
            return false;
        }
        let recorded = self
            .store
            .kv_get(&call.session, &format!("tool_contract:{}", call.call_id))
            .ok()
            .flatten();
        let current =
            serde_json::to_string(&contract_of(&self.subagent_tool())).expect("serializes");
        recorded.as_deref() == Some(current.as_str())
    }

    /// Place queued inputs. Post-tools: steers join the run. Final: the caller starts a new run.
    fn place_queued(
        &self,
        sess: &SessionRt,
        run: u64,
        mode: QueueDelivery,
        follow_ups: bool,
    ) -> Result<(), ProtoError> {
        let session = sess.id.clone();
        self.commit(sess, |tx| {
            let queue = Store::queue_in(tx, &session)?;
            let chosen = choose(&queue, mode, follow_ups, QueueDelivery::OneAtATime);
            if chosen.is_empty() {
                return Ok(((), vec![]));
            }
            let keys: Vec<String> = chosen.iter().map(|r| r.key.clone()).collect();
            Store::consume_queued(tx, &session, &keys, run)?;
            let mut changes = Vec::new();
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

    /// After a run settles: start the next run from queued steers and follow-ups.
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
            let queue = Store::queue_in(tx, &session)?;
            let chosen = choose(
                &queue,
                settings.steering_mode,
                true,
                settings.follow_up_mode,
            );
            let Some(first) = chosen.first() else {
                return Ok((None, vec![]));
            };
            let now = now_ms();
            let run = Store::start_run(tx, &session, Some(&first.key), now)?;
            let keys: Vec<String> = chosen.iter().map(|r| r.key.clone()).collect();
            Store::consume_queued(tx, &session, &keys, run)?;
            let mut changes = Vec::new();
            for row in &chosen {
                let entry = Store::append_entry(tx, &session, now, Some(run), &user_entry(row))?;
                changes.push(SessionChange::EntryAppended { entry });
            }
            changes.push(SessionChange::QueueChanged {
                queue: Store::queue_view(&Store::queue_in(tx, &session)?),
            });
            changes.push(SessionChange::RunChanged {
                run: Store::run_in(tx, run)?.and_then(|r| r.view()),
            });
            Ok((Some(run), changes))
        });
        if let Ok(Some(run)) = started {
            self.spawn_run(sess.clone(), run);
        }
    }

    /// The provider's cache/affinity id for this session (stable across restarts).
    fn provider_session_id(&self, session: &str) -> String {
        if let Ok(Some(id)) = self.store.kv_get(session, "provider_session_id") {
            return id;
        }
        let id = crate::util::uuid_v4();
        let _ = self
            .store
            .tx(|tx| Store::kv_set(tx, session, "provider_session_id", &id));
        self.store
            .kv_get(session, "provider_session_id")
            .ok()
            .flatten()
            .unwrap_or(id)
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

fn usage_in(c: &rusqlite::Connection, session: &str) -> crate::store::Result<UsageView> {
    let mut stmt = c.prepare_cached(
        "SELECT bucket, key, value FROM usage WHERE session = ?1 ORDER BY bucket, key",
    )?;
    let rows = stmt
        .query_map([session], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut view = UsageView::default();
    for (bucket, key, raw) in rows {
        let usage: Usage = serde_json::from_str(&raw).unwrap_or_default();
        if bucket == "tool" {
            view.tools.push((key, usage));
        } else {
            view.models.push((key, usage));
        }
    }
    Ok(view)
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
fn unusable_proxy(
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
