//! Per-session state and mutations: admission, queue, Stop, settlement and recovery.
//!
//! Every mutation is one short store transaction that also bumps the session revision; its
//! events are published only after the commit. A storage error fails the session closed: the
//! active run is cancelled and further mutations are refused until the session is reopened,
//! which reconciles from what was committed.

use std::sync::{Arc, Mutex};

use pipkin_engine_protocol::limits::{
    MAX_IMAGE_BYTES, MAX_IMAGES, MAX_KEY_BYTES, MAX_PROMPT_BYTES,
};
use pipkin_engine_protocol::*;
use serde_json::Map;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use super::Shared;
use crate::store::{self, Admission, RequestRow, RunState, Store, ToolState};
use crate::util::now_ms;

/// A run executing in this process.
pub struct Active {
    pub run: u64,
    pub cancel: CancellationToken,
}

/// How a run ended, broadcast to in-process waiters (subagent parents).
#[derive(Clone, Debug)]
pub struct Settled {
    pub run: u64,
    pub request: Option<String>,
    pub outcome: RunOutcome,
}

pub struct SessionRt {
    pub id: String,
    pub(super) root_id: String,
    pub(super) removed: CancellationToken,
    /// Selection pinned before this lifetime was acquired. Never changed while active.
    pub(super) extension_profile: Vec<ExtensionSpec>,
    pub(super) profile_pin_pending: std::sync::atomic::AtomicBool,
    /// Completion of the preceding lifetime's extension teardown, if any.
    pub(super) predecessor: Option<Arc<CancellationToken>>,
    /// Serializes this session's admission/stop/settlement sequences.
    pub lock: tokio::sync::Mutex<()>,
    pub active: Mutex<Option<Active>>,
    pub failed: Mutex<Option<String>>,
    pub settled: broadcast::Sender<Settled>,
    pub(super) changed: tokio::sync::Notify,
    /// The system prompt last sent, so it stays byte-stable between turns (prompt caching).
    pub prompt: Mutex<Option<(String, Map<String, serde_json::Value>)>>,
    /// Shared with the root and its children, never with independent roots in the same cwd.
    pub(super) prompt_resources: Arc<Mutex<super::glue::Resources>>,
    /// Questions, status lines and notices from this session's extensions (volatile).
    pub questions: crate::extensions::questions::QuestionHost,
    /// Live foreground compactions, including manual work with no coding run.
    pub(super) manual_compactions: Mutex<std::collections::BTreeMap<u64, CancellationToken>>,
    pub(super) active_compactions: Mutex<std::collections::HashSet<u64>>,
    /// A background compaction is running.
    pub compacting: std::sync::atomic::AtomicBool,
    /// Session extensions, started when the session is first opened.
    pub extensions: tokio::sync::Mutex<Option<crate::extensions::ExtensionRunner>>,
    /// Serializes [`Shared::recover_session`]: startup recovery and a client's open can run
    /// at once, and both would record the same interrupted call.
    pub(super) recovery: Mutex<()>,
}

impl SessionRt {
    pub fn new(id: &str, questions: crate::extensions::questions::QuestionHost) -> SessionRt {
        SessionRt {
            questions,
            recovery: Mutex::new(()),
            extension_profile: Vec::new(),
            profile_pin_pending: std::sync::atomic::AtomicBool::new(false),
            predecessor: None,
            extensions: tokio::sync::Mutex::new(None),
            compacting: std::sync::atomic::AtomicBool::new(false),
            manual_compactions: Mutex::new(Default::default()),
            active_compactions: Mutex::new(Default::default()),
            id: id.to_owned(),
            root_id: id.to_owned(),
            removed: CancellationToken::new(),
            lock: tokio::sync::Mutex::new(()),
            active: Mutex::new(None),
            failed: Mutex::new(None),
            settled: broadcast::channel(64).0,
            changed: tokio::sync::Notify::new(),
            prompt: Mutex::new(None),
            prompt_resources: Arc::default(),
        }
    }

    pub fn active_run(&self) -> Option<u64> {
        self.active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|a| a.run)
    }

    pub fn failure(&self) -> Option<String> {
        self.failed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

fn invalid(message: impl Into<String>) -> ProtoError {
    ProtoError::new(ErrorCode::Invalid, message)
}

/// Validate an input before anything is recorded.
pub fn check_input(key: &str, text: &str, images: &[ImageInput]) -> Result<(), ProtoError> {
    if key.is_empty() || key.len() > MAX_KEY_BYTES {
        return Err(invalid("The request key is missing or too long."));
    }
    if text.len() > MAX_PROMPT_BYTES {
        return Err(ProtoError::new(
            ErrorCode::TooLarge,
            format!(
                "The message is larger than {} MB.",
                MAX_PROMPT_BYTES / 1024 / 1024
            ),
        ));
    }
    if text.trim().is_empty() && images.is_empty() {
        return Err(invalid("The message is empty."));
    }
    if images.len() > MAX_IMAGES {
        return Err(ProtoError::new(
            ErrorCode::TooLarge,
            format!("At most {MAX_IMAGES} images can be sent at once."),
        ));
    }
    for image in images {
        if !image.mime_type.starts_with("image/") {
            return Err(invalid(format!(
                "{} is not an image type.",
                image.mime_type
            )));
        }
        if image.data.len() / 4 * 3 > MAX_IMAGE_BYTES {
            return Err(ProtoError::new(
                ErrorCode::TooLarge,
                format!(
                    "An image is larger than {} MB.",
                    MAX_IMAGE_BYTES / 1024 / 1024
                ),
            ));
        }
    }
    Ok(())
}

/// The user message for an admitted input.
pub fn user_entry(row: &RequestRow) -> EntryKind {
    let mut content = vec![];
    if !row.text.is_empty() || row.images.is_empty() {
        content.push(Block::text(&row.text));
    }
    for image in &row.images {
        content.push(Block::image(&image.mime_type, &image.data));
    }
    EntryKind::Message {
        message: Message::User {
            content,
            timestamp: now_ms(),
            extra: Map::new(),
        },
        mode: row.mode,
        request: Some(row.key.clone()),
        partial: false,
    }
}

impl Shared {
    /// Commit `f` for `session`, bump its revision when it reports changes, then publish them.
    pub fn commit<T>(
        &self,
        sess: &SessionRt,
        f: impl FnOnce(&rusqlite::Transaction) -> store::Result<(T, Vec<SessionChange>)>,
    ) -> Result<T, ProtoError> {
        if let Some(why) = sess.failure() {
            return Err(ProtoError::new(
                ErrorCode::Storage,
                format!(
                    "This session stopped after a storage error ({why}). Reopen it to continue."
                ),
            ));
        }
        // Keep transaction order and live publication order identical across concurrent runs.
        let _publication = self.publication.lock().unwrap_or_else(|e| e.into_inner());
        if sess.removed.is_cancelled() {
            return Err(crate::store::StoreError::Removed.into());
        }
        let id = sess.id.clone();
        let result = tokio::task::block_in_place(|| {
            self.store.tx(|tx| {
                Store::assert_session_live(tx, &id)?;
                let (value, changes) = f(tx)?;
                let revision = if changes.is_empty() {
                    0
                } else {
                    Store::bump(tx, &id, now_ms())?
                };
                let mut ancestor_usage = Vec::new();
                if changes
                    .iter()
                    .any(|change| matches!(change, SessionChange::UsageChanged { .. }))
                {
                    for ancestor in Store::usage_ancestors(tx, &id)? {
                        let usage = Store::usage_in(tx, &ancestor)?;
                        let revision = Store::bump(tx, &ancestor, now_ms())?;
                        ancestor_usage.push((ancestor, revision, usage));
                    }
                }
                Ok((value, changes, revision, ancestor_usage))
            })
        });
        match result {
            Ok((value, changes, revision, ancestor_usage)) => {
                super::faults::crash_point("after_state_commit_before_delivery");
                sess.changed.notify_waiters();
                for change in changes {
                    self.publish(Event::Session {
                        session: id.clone(),
                        revision,
                        change,
                    });
                }
                for (session, revision, usage) in ancestor_usage {
                    self.publish(Event::Session {
                        session,
                        revision,
                        change: SessionChange::UsageChanged { usage },
                    });
                }
                Ok(value)
            }
            Err(store::StoreError::Removed) => Err(store::StoreError::Removed.into()),
            Err(e) => {
                self.storage_failure(sess, &e.to_string());
                Err(e.into())
            }
        }
    }

    pub(super) fn storage_failure(&self, sess: &SessionRt, why: &str) {
        let id = &sess.id;
        log::error!("session {id}: storage failure: {why}");
        let first = sess
            .failed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace(why.to_owned())
            .is_none();
        sess.changed.notify_waiters();
        if first {
            self.publish(Event::Notice { message: format!("Conversation {id} stopped after a storage error ({why}). What was saved before it is kept; reopen the conversation to continue.") });
        }
        for cancel in sess
            .manual_compactions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
        {
            cancel.cancel();
        }
        if let Some(active) = sess
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            active.cancel.cancel();
        }
    }

    /// Admit a prompt. Busy sessions refuse a new prompt (it is not recorded).
    pub async fn prompt(
        self: &Arc<Self>,
        session: &str,
        key: &str,
        text: &str,
        images: &[ImageInput],
    ) -> Result<Reply, ProtoError> {
        check_input(key, text, images)?;
        let sess = self.session(session)?;
        let _guard = sess.lock.lock().await;
        struct Out {
            start: Option<u64>,
            run: Option<u64>,
            entry: Option<u64>,
        }
        let settings = self.settings_for(session);
        let out = self.commit(&sess, |tx| {
            if let Some(existing) = Store::request_in(tx, session, key)? {
                if existing.fingerprint != store::fingerprint(InputMode::Prompt, text, images) {
                    return Ok((Err(ErrorCode::Conflict), vec![]));
                }
                return Ok((
                    Ok(Out {
                        start: None,
                        run: existing.run,
                        entry: existing.queue_entry,
                    }),
                    vec![],
                ));
            }
            if Store::open_run_in(tx, session)?.is_some() {
                return Ok((Err(ErrorCode::Busy), vec![]));
            }
            let now = now_ms();
            let waiting: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM requests WHERE session = ?1 AND state = 'queued')",
                [session],
                |r| r.get(0),
            )?;
            let admission = if waiting {
                Store::admit_queued_prompt(tx, session, key, text, images, now)?
            } else {
                Store::admit(tx, session, key, InputMode::Prompt, text, images, None, now)?
            };
            let row = match admission {
                Admission::New(row) => row,
                Admission::Full => return Ok((Err(ErrorCode::TooLarge), vec![])),
                _ => return Ok((Err(ErrorCode::Conflict), vec![])),
            };
            if waiting {
                let (start, changes) = Self::start_queued_run(tx, session, &settings)?;
                let admitted = Store::request_in(tx, session, key)?.expect("just admitted");
                return Ok((
                    Ok(Out {
                        start,
                        run: admitted.run,
                        entry: admitted.queue_entry,
                    }),
                    changes,
                ));
            }
            let mut changes = Self::flush_summary_writes(tx, session)?;
            let run = Store::start_run(tx, session, Some(key), now)?;
            Self::diag_run_start(tx, session, run, &row, 1, now)?;
            let entry = Store::append_entry(tx, session, now, Some(run), &user_entry(&row))?;
            let view = Store::run_in(tx, run)?.and_then(|r| r.view());
            changes.extend([
                SessionChange::EntryAppended { entry },
                SessionChange::RunChanged { run: view },
            ]);
            Ok((
                Ok(Out {
                    start: Some(run),
                    run: Some(run),
                    entry: None,
                }),
                changes,
            ))
        })?;
        match out {
            Ok(out) => {
                if let Some(run) = out.start {
                    super::faults::crash_point("after-admission");
                    self.spawn_run(sess.clone(), run);
                }
                Ok(Reply::Admitted {
                    run: out.run,
                    entry: out.entry,
                })
            }
            Err(ErrorCode::Busy) => Err(ProtoError::new(
                ErrorCode::Busy,
                format!("Conversation {session} is busy"),
            )),
            Err(ErrorCode::TooLarge) => Err(ProtoError::new(
                ErrorCode::TooLarge,
                "The queue is full. Withdraw a queued input before submitting another message.",
            )),
            Err(code) => Err(ProtoError::new(
                code,
                "This request key was already used for a different message.",
            )),
        }
    }

    /// Admit a steer or follow-up. When idle it starts a run at once (reference behavior).
    pub async fn enqueue(
        self: &Arc<Self>,
        session: &str,
        key: &str,
        mode: InputMode,
        text: &str,
        images: &[ImageInput],
    ) -> Result<Reply, ProtoError> {
        if mode == InputMode::Prompt {
            return Err(invalid("Use Prompt for a new prompt."));
        }
        check_input(key, text, images)?;
        let sess = self.session(session)?;
        let _guard = sess.lock.lock().await;
        let settings = self.settings_for(session);
        let result = self.commit(&sess, |tx| {
            let now = now_ms();
            let row = match Store::admit(tx, session, key, mode, text, images, None, now)? {
                Admission::New(row) => row,
                Admission::Existing(row) => {
                    return Ok((Ok((row.queue_entry, None, row.run)), vec![]));
                }
                Admission::Conflict => return Ok((Err(ErrorCode::Conflict), vec![])),
                Admission::Full => return Ok((Err(ErrorCode::TooLarge), vec![])),
            };
            if Store::open_run_in(tx, session)?.is_some() {
                let queue = Store::queue_view(&Store::queue_in(tx, session)?);
                return Ok((
                    Ok((row.queue_entry, None, None)),
                    vec![SessionChange::QueueChanged { queue }],
                ));
            }
            // Idle admission resumes waiting work through the same final boundary as
            // ordinary completion; it cannot jump ahead of older inputs.
            let (started, changes) = Self::start_queued_run(tx, session, &settings)?;
            let admitted = Store::request_in(tx, session, key)?.expect("just admitted");
            Ok((Ok((row.queue_entry, started, admitted.run)), changes))
        })?;
        match result {
            Ok((entry, started, admitted)) => {
                if let Some(run) = started {
                    self.spawn_run(sess.clone(), run);
                }
                Ok(Reply::Admitted {
                    run: admitted,
                    entry,
                })
            }
            Err(ErrorCode::TooLarge) => Err(ProtoError::new(
                ErrorCode::TooLarge,
                "The queue is full. Wait for the current work to finish.",
            )),
            Err(code) => Err(ProtoError::new(
                code,
                "This request key was already used for a different message.",
            )),
        }
    }

    pub async fn cancel_queued(&self, session: &str, entry: u64) -> Result<Reply, ProtoError> {
        let sess = self.session(session)?;
        let _guard = sess.lock.lock().await;
        let outcome = self.commit(&sess, |tx| {
            let row: Option<RequestRow> = Store::queue_in(tx, session)?
                .into_iter()
                .find(|r| r.queue_entry == Some(entry));
            let outcome = match row {
                Some(row) => {
                    tx.execute(
                        "UPDATE requests SET state = 'withdrawn', outcome = ?3
                         WHERE session = ?1 AND key = ?2",
                        rusqlite::params![
                            session,
                            row.key,
                            serde_json::to_string(&RunOutcome::Aborted).expect("serializes")
                        ],
                    )?;
                    CancelQueuedOutcome::Cancelled
                }
                None => {
                    let consumed: bool = tx.query_row(
                        "SELECT COUNT(*) FROM requests WHERE session = ?1 AND queue_entry = ?2
                         AND state IN ('running', 'settled')",
                        rusqlite::params![session, entry as i64],
                        |r| r.get::<_, i64>(0),
                    )? > 0;
                    if consumed {
                        CancelQueuedOutcome::AlreadyConsumed
                    } else {
                        CancelQueuedOutcome::NotFound
                    }
                }
            };
            let changes = if outcome == CancelQueuedOutcome::Cancelled {
                vec![SessionChange::QueueChanged {
                    queue: Store::queue_view(&Store::queue_in(tx, session)?),
                }]
            } else {
                vec![]
            };
            Ok((outcome, changes))
        })?;
        Ok(Reply::CancelQueued { outcome })
    }

    /// Persist Stop intent and withdraw queued inputs, then cancel the active run. Settlement
    /// is published by the run itself (or here, when no run is executing in this process).
    pub async fn stop(self: &Arc<Self>, session: &str) -> Result<Reply, ProtoError> {
        // The goal that exists at this instant pauses; one set afterwards is a replacement.
        self.goal_on_stop(session);
        let sess = self.session(session)?;
        let _guard = sess.lock.lock().await;
        let run = self.commit(&sess, |tx| {
            Store::stop_compactions(tx, session)?;
            let Some(run) = Store::open_run_in(tx, session)? else {
                return Ok((None, vec![]));
            };
            let now = now_ms();
            tx.execute(
                "UPDATE runs SET state = 'stopping',
                    stop_requested_at = COALESCE(stop_requested_at, ?2) WHERE id = ?1",
                rusqlite::params![run.id as i64, now],
            )?;
            let queued = Store::queue_in(tx, session)?;
            for row in &queued {
                tx.execute(
                    "UPDATE requests SET state = 'withdrawn', outcome = ?3, settled_at = ?4
                     WHERE session = ?1 AND key = ?2",
                    rusqlite::params![
                        session,
                        row.key,
                        serde_json::to_string(&RunOutcome::Aborted).expect("serializes"),
                        now
                    ],
                )?;
            }
            let mut changes = vec![SessionChange::RunChanged {
                run: Store::run_in(tx, run.id)?.and_then(|r| r.view()),
            }];
            if !queued.is_empty() {
                changes.push(SessionChange::QueueChanged { queue: vec![] });
            }
            Ok((Some(run.id), changes))
        })?;
        // This also applies when the coding run has finished or never existed. Keep the
        // session lock through signaling so a newer manual request starts after this Stop.
        for cancel in sess
            .manual_compactions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
        {
            cancel.cancel();
        }
        let Some(run) = run else {
            return Ok(Reply::Stopping { run: None });
        };
        super::faults::crash_point("after-stop-intent");
        let active = sess
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .filter(|a| a.run == run)
            .map(|a| a.cancel.clone());
        match active {
            Some(cancel) => cancel.cancel(),
            // Nothing executes it here (for example, a run left over from before a restart
            // that has not been resumed): settle it now, with any child it delegated to.
            None => {
                self.abort_orphaned_children(session, run)?;
                self.settle(&sess, run, RunOutcome::Aborted)?;
            }
        }
        Ok(Reply::Stopping { run: Some(run) })
    }

    /// Settle a run: finalize its partial message, retire its inputs after a Stop, record the
    /// outcome on the run and on every request it took. One commit.
    pub fn settle(
        &self,
        sess: &SessionRt,
        run: u64,
        outcome: RunOutcome,
    ) -> Result<(), ProtoError> {
        let session = sess.id.clone();
        let request = self.commit(sess, |tx| {
            let Some(row) = Store::run_in(tx, run)? else {
                return Ok((None, vec![]));
            };
            if row.state == RunState::Settled {
                return Ok((None, vec![]));
            }
            let now = now_ms();
            let mut changes = Vec::new();
            for mut entry in Store::entries_of_run(tx, &session, run)? {
                let mut changed = false;
                if let EntryKind::Message {
                    message: Message::Assistant { stop_reason, .. },
                    partial,
                    ..
                } = &mut entry.kind
                    && *partial
                {
                    *partial = false;
                    *stop_reason = StopReason::Aborted;
                    changed = true;
                }
                if outcome == RunOutcome::Aborted
                    && !entry.retired
                    && matches!(
                        &entry.kind,
                        EntryKind::Message {
                            message: Message::User { .. },
                            ..
                        }
                    )
                {
                    entry.retired = true;
                    changed = true;
                }
                if changed {
                    Store::replace_entry(tx, &session, &entry)?;
                    changes.push(SessionChange::EntryReplaced { entry });
                }
            }
            // Calls the stopped run never started get an aborted result, so the history stays
            // a valid tool exchange (reference generation abort handler).
            if outcome == RunOutcome::Aborted {
                let entries = Store::entries_of_run(tx, &session, run)?;
                let answered: std::collections::HashSet<String> = entries
                    .iter()
                    .filter_map(|e| match &e.kind {
                        EntryKind::Message {
                            message: Message::ToolResult { tool_call_id, .. },
                            ..
                        } => Some(tool_call_id.clone()),
                        _ => None,
                    })
                    .collect();
                let last = entries.iter().rev().find_map(|e| match &e.kind {
                    EntryKind::Message {
                        message: m @ Message::Assistant { stop_reason, .. },
                        partial: false,
                        ..
                    } if *stop_reason == StopReason::ToolUse => Some(m.clone()),
                    _ => None,
                });
                for (id, name, args) in last.as_ref().map(|m| m.tool_calls()).unwrap_or_default() {
                    if answered.contains(id) {
                        continue;
                    }
                    let output = Store::tool_progress(tx, &session, id)?;
                    let result = crate::tools::recovered_output(name, &output, true);
                    let kind = EntryKind::Message {
                        message: Message::ToolResult {
                            tool_call_id: id.to_owned(),
                            tool_name: name.to_owned(),
                            content: result.content.clone(),
                            is_error: true,
                            timestamp: now,
                            extra: Map::new(),
                        },
                        mode: InputMode::Prompt,
                        request: None,
                        partial: false,
                    };
                    let entry = Store::append_entry(tx, &session, now, Some(run), &kind)?;
                    Store::tool_intent(tx, &session, id, Some(run), name, args, false, now)?;
                    Store::tool_result(
                        tx,
                        &session,
                        id,
                        ToolState::Done,
                        entry.id,
                        &result.retained,
                        result.dropped_bytes,
                        now,
                    )?;
                    changes.push(SessionChange::EntryAppended { entry });
                }
            }
            if outcome == RunOutcome::Completed {
                changes.extend(Self::flush_summary_writes(tx, &session)?);
            }
            Store::settle_run(tx, run, &outcome, now)?;
            for req in Store::requests_of_run(tx, run)? {
                Store::settle_request(tx, &session, &req.key, &outcome, now)?;
            }
            {
                use rusqlite::OptionalExtension;
                let stop_at: Option<i64> = tx
                    .query_row(
                        "SELECT stop_requested_at FROM runs WHERE id = ?1",
                        [run as i64],
                        |r| r.get(0),
                    )
                    .optional()?
                    .flatten();
                let (name, detail) = super::diagnostics::outcome_name(&outcome);
                // The run's start record (queue wait, parent and call for a child) is
                // copied so the end record stands on its own for an exporter.
                let started: Option<String> = tx
                    .query_row(
                        "SELECT record FROM diagnostics WHERE session = ?1 AND run = ?2
                         AND kind = 'run.start' ORDER BY seq LIMIT 1",
                        rusqlite::params![session, run as i64],
                        |r| r.get(0),
                    )
                    .optional()?;
                let started: serde_json::Value = started
                    .and_then(|t| serde_json::from_str(&t).ok())
                    .unwrap_or(serde_json::Value::Null);
                Store::diag_append(
                    tx,
                    &session,
                    now,
                    "run.end",
                    Some(run),
                    row.request.as_deref(),
                    None,
                    &serde_json::json!({
                        "outcome": name,
                        "detail": detail,
                        "durationMs": now.saturating_sub(row.started_at_ms).max(0),
                        "stopRequested": row.stop_requested,
                        "stopLatencyMs": stop_at.map(|t| now.saturating_sub(t).max(0)),
                        "queueWaitMs": started.get("queueWaitMs").cloned().unwrap_or(serde_json::Value::Null),
                        "parent": started.get("parent").cloned().unwrap_or(serde_json::Value::Null),
                        "parentCall": started.get("parentCall").cloned().unwrap_or(serde_json::Value::Null),
                    }),
                )?;
            }
            changes.push(SessionChange::RunSettled {
                run,
                request: row.request.clone(),
                outcome: outcome.clone(),
            });
            changes.push(SessionChange::RunChanged { run: None });
            Ok((Some(row.request), changes))
        })?;
        self.publish_context(&session);
        self.export.notify.notify_one();
        if let Some(request) = request {
            self.goal_after_settlement(&session, request.clone(), outcome.clone());
            let _ = sess.settled.send(Settled {
                run,
                request,
                outcome,
            });
        }
        Ok(())
    }

    /// Reconcile a session after a restart or a storage failure: settle runs whose Stop was
    /// durable, record interrupted (unconfirmed) tool effects, and resume the rest.
    pub fn recover_session(self: &Arc<Self>, session: &str) -> Result<(), ProtoError> {
        let sess = self.session(session)?;
        let gate = sess.clone();
        let _one_at_a_time = gate.recovery.lock().unwrap_or_else(|e| e.into_inner());
        if sess
            .profile_pin_pending
            .load(std::sync::atomic::Ordering::Acquire)
        {
            if let Err(error) = crate::extensions::profiles::set_session(
                &self.store,
                session,
                &sess.extension_profile,
            ) {
                self.storage_failure(&sess, &error.message);
                return Err(error);
            }
            sess.profile_pin_pending
                .store(false, std::sync::atomic::Ordering::Release);
        }
        if sess.active_run().is_some() {
            // The failed execution is still unwinding. Clearing its error now lets it
            // mistake storage-induced cancellation for a user Stop and settle the run.
            return Ok(());
        }
        *sess.failed.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let Some(run) = self.store.open_run(session).map_err(ProtoError::from)? else {
            return Ok(());
        };
        if run.stop_requested {
            let intents = self
                .store
                .read(|c| Store::tools_in_state(c, session, "intent"))?;
            for call in intents.iter().filter(|c| c.run == Some(run.id)) {
                super::faults::kill_tool_processes(&super::faults::tool_call_token(
                    session,
                    &call.call_id,
                ));
            }
            self.abort_orphaned_children(session, run.id)?;
            return self.settle(&sess, run.id, RunOutcome::Aborted);
        }
        // Tool intents without a result: unconfirmed effects unless replay-safe.
        let intents = tokio::task::block_in_place(|| {
            self.store
                .read(|c| Store::tools_in_state(c, session, "intent"))
        })
        .map_err(ProtoError::from)?;
        for call in intents.into_iter().filter(|c| c.run == Some(run.id)) {
            if call.replay_safe && self.replay_allowed(&call) {
                continue;
            }
            // Never rerun it, and do not let what it left running finish unobserved either.
            let reaped = super::faults::kill_tool_processes(&super::faults::tool_call_token(
                session,
                &call.call_id,
            ));
            if reaped > 0 {
                log::warn!(
                    "session {session}: killed {reaped} process(es) left by interrupted call {}",
                    call.call_id
                );
            }
            if call.name == "subagent" {
                self.abort_child(session, &call.call_id)?;
            }
            let output = self
                .store
                .read(|c| Store::tool_progress(c, session, &call.call_id))?;
            let result = crate::tools::recovered_output(&call.name, &output, false);
            self.record_tool_result(
                &sess,
                run.id,
                &call.call_id,
                &call.name,
                &result,
                ToolState::Interrupted,
            )?;
        }
        self.diag(
            session,
            "run.resumed",
            Some(run.id),
            run.request.as_deref(),
            None,
            serde_json::json!({ "reason": "restart" }),
        );
        self.spawn_run(sess, run.id);
        Ok(())
    }

    /// Commit a tool's result entry and its retained output.
    pub fn record_tool_result(
        &self,
        sess: &SessionRt,
        run: u64,
        call_id: &str,
        name: &str,
        result: &crate::tools::ToolResult,
        state: ToolState,
    ) -> Result<Entry, ProtoError> {
        let session = sess.id.clone();
        self.commit(sess, |tx| {
            let now = now_ms();
            let mut extra = Map::new();
            if let Some(details) = &result.details {
                extra.insert("details".into(), details.clone());
            }
            let kind = EntryKind::Message {
                message: Message::ToolResult {
                    tool_call_id: call_id.to_owned(),
                    tool_name: name.to_owned(),
                    content: result.content.clone(),
                    is_error: result.is_error,
                    timestamp: now,
                    extra,
                },
                mode: InputMode::Prompt,
                request: None,
                partial: false,
            };
            let entry = Store::append_entry(tx, &session, now, Some(run), &kind)?;
            Store::tool_intent(
                tx,
                &session,
                call_id,
                Some(run),
                name,
                &serde_json::Value::Null,
                false,
                now,
            )?;
            Store::tool_result(
                tx,
                &session,
                call_id,
                state,
                entry.id,
                &result.retained,
                result.dropped_bytes,
                now,
            )?;
            Ok((entry.clone(), vec![SessionChange::EntryAppended { entry }]))
        })
    }
}
