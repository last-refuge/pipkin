//! CLI actor. Each public mode requires its host opt-in, fresh preflight and live grant.
//! Coding additionally owns a receipt-backed broker/listener. Hosting policy grants no consent.
//! Shared tracks external tasks through cleanup/settlement. Shutdown cancels first, then
//! waits concurrently with other teardown within its bounded deadline. A deadline never
//! synthesizes cancellation evidence: startup recovery owns unfinished receipts.
//!
//! Never calls the native provider, tools, extensions, goals, compaction or queue driver.
#![allow(dead_code)] // Internal prepared entry points are retained for qualification tests.

use std::sync::{Arc, Mutex};

use pipkin_engine_protocol::{ErrorCode, ExecutionBackend, ProtoError, Reply, SessionChange};
use tokio::sync::{OwnedSemaphorePermit, mpsc};
use tokio_util::{
    sync::CancellationToken,
    task::{TaskTracker, task_tracker::TaskTrackerToken},
};

use super::{SessionRt, Settled, Shared, session::Active};
use crate::execution::claude_cli::{events, policy, preflight::Prepared, process};
use crate::store::{
    self, CliBindingSpec, ExternalPromptMode, ExternalPromptReceipt, ExternalRunKey,
    ExternalSettlementEvidence, Store, StoreError,
};
use crate::util::now_ms;

const EVENT_CAPACITY: usize = 128;

struct ObservedDecoder {
    decoder: Option<events::Decoder>,
    error: Option<events::Error>,
    rejected_record: Option<events::RecordClass>,
    parse_failure: Option<events::ParseFailure>,
    presentation_incomplete: bool,
    diagnostic: Option<events::StreamDiagnostic>,
}

#[cfg(test)]
#[derive(Default)]
pub(super) struct LaunchTest {
    pub timeout: Option<std::time::Duration>,
    pub gate: Option<tokio::sync::oneshot::Receiver<()>>,
}

struct Launch {
    prepared: Prepared,
    binding: CliBindingSpec,
    text: String,
    grant: Option<Arc<super::local_cli_setup::Grant>>,
    #[cfg(unix)]
    coding: Option<super::external_coding::Coding>,
    #[cfg(test)]
    test: Option<LaunchTest>,
}

fn unsupported() -> ProtoError {
    ProtoError::new(
        ErrorCode::Unsupported,
        "This operation requires a CLI root conversation.",
    )
}

fn admitted(receipt: &ExternalPromptReceipt) -> Reply {
    Reply::Admitted {
        run: Some(receipt.id),
        entry: None,
    }
}

/// Owns the session lifetime and capacity even if the actor future is dropped. The
/// supervisor future lives INSIDE that actor (never a detached subprocess task), so
/// dropping the actor also drops its owned process group and pipe pumps.
struct Lifetime {
    sess: Arc<SessionRt>,
    run: u64,
    cancel: CancellationToken,
    _permit: OwnedSemaphorePermit,
    // Last field: completion is observed only after lifetime cleanup/capacity release.
    _completion: TaskTrackerToken,
}
impl Lifetime {
    fn clear_active(&self) {
        let mut active = self.sess.active.lock().unwrap_or_else(|e| e.into_inner());
        if active.as_ref().is_some_and(|a| a.run == self.run) {
            *active = None;
        }
        self.sess.changed.notify_waiters();
    }
}
impl Drop for Lifetime {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.clear_active();
    }
}

impl Shared {
    /// Call only after cancelling `stopping`. Registration precedes the stopping
    /// check, so every admitted actor is counted even if it has not been polled.
    /// A late registrant after wait completes observes stopping and cannot launch.
    pub(super) fn external_shutdown(&self) -> TaskTracker {
        debug_assert!(self.stopping.is_cancelled());
        self.external_tasks.close();
        self.external_tasks.clone()
    }

    fn require_external_root(&self, session: &str) -> Result<(), ProtoError> {
        let row = self
            .store
            .session(session)?
            .ok_or_else(|| ProtoError::new(ErrorCode::NotFound, "Session does not exist"))?;
        if row.execution_backend != ExecutionBackend::LocalClaudeCli
            || row.parent.is_some()
            || row.root.is_some()
        {
            return Err(unsupported());
        }
        Ok(())
    }

    /// Expected admission/projection conflicts are not storage failures. A savepoint
    /// is essential: `_in` projection can reject AFTER writing provisional rows.
    pub(super) fn external_commit<T>(
        &self,
        sess: &SessionRt,
        f: impl FnOnce(&rusqlite::Transaction<'_>) -> store::Result<(T, Vec<SessionChange>)>,
    ) -> Result<T, ProtoError> {
        self.commit(sess, |tx| {
            tx.execute_batch("SAVEPOINT external_runtime")?;
            match f(tx) {
                Ok((value, changes)) => {
                    tx.execute_batch("RELEASE external_runtime")?;
                    Ok((Ok(value), changes))
                }
                Err(e @ (StoreError::ExecutionConflict | StoreError::UnsupportedExecution)) => {
                    tx.execute_batch("ROLLBACK TO external_runtime; RELEASE external_runtime")?;
                    Ok((Err(e), vec![]))
                }
                Err(e) => Err(e),
            }
        })?
        .map_err(Into::into)
    }

    pub(super) async fn prompt_external_prepared(
        self: &Arc<Self>,
        session: &str,
        request: &str,
        text: &str,
        prepared: Prepared,
    ) -> Result<Reply, ProtoError> {
        self.prompt_external_inner(
            session,
            request,
            text,
            prepared,
            #[cfg(test)]
            None,
        )
        .await
    }

    pub(super) async fn prompt_external_inner(
        self: &Arc<Self>,
        session: &str,
        request: &str,
        text: &str,
        prepared: Prepared,
        #[cfg(test)] test: Option<LaunchTest>,
    ) -> Result<Reply, ProtoError> {
        self.prompt_external_authorized(
            session,
            request,
            text,
            prepared,
            None,
            #[cfg(test)]
            test,
        )
        .await
    }

    pub(super) async fn prompt_external_authorized(
        self: &Arc<Self>,
        session: &str,
        request: &str,
        text: &str,
        prepared: Prepared,
        turn: Option<&super::local_cli_setup::Turn>,
        #[cfg(test)] test: Option<LaunchTest>,
    ) -> Result<Reply, ProtoError> {
        self.prompt_external_dispatch(
            session,
            request,
            text,
            prepared,
            turn,
            #[cfg(unix)]
            None,
            #[cfg(test)]
            test,
        )
        .await
    }

    /// Current-grant route: the registry rechecks this exact Turn under its admission
    /// lock. Actor revocation belongs to the persistent grant, never its probe child.
    #[cfg(unix)]
    pub(super) async fn prompt_external_coding_authorized(
        self: &Arc<Self>,
        turn: &super::local_cli_setup::Turn,
        request: &str,
        text: &str,
        prepared: Prepared,
    ) -> Result<Reply, ProtoError> {
        let coding = super::external_coding::Admission {
            owner: turn.owner(),
            // Turn-bound Write authority, independent of the former setup ticket.
            expires: std::time::Instant::now() + policy::TURN_TIMEOUT,
            revoked: turn.grant.cancel.clone(),
            parent: self.config.data_dir.clone(),
            feedback: super::external_coding::Feedback::PullOnly,
        };
        self.prompt_external_dispatch(
            &turn.session,
            request,
            text,
            prepared,
            Some(turn),
            Some(coding),
            #[cfg(test)]
            None,
        )
        .await
    }

    /// Internal offline-qualified path only. No public handler selects it. Admission
    /// fields are engine-captured. Public callers use the current-grant wrapper above.
    #[cfg(unix)]
    pub(super) async fn prompt_external_coding(
        self: &Arc<Self>,
        session: &str,
        request: &str,
        text: &str,
        prepared: Prepared,
        coding: super::external_coding::Admission,
    ) -> Result<Reply, ProtoError> {
        self.prompt_external_dispatch(
            session,
            request,
            text,
            prepared,
            None,
            Some(coding),
            #[cfg(test)]
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)] // One private dispatch preserves the chat-only admission path.
    async fn prompt_external_dispatch(
        self: &Arc<Self>,
        session: &str,
        request: &str,
        text: &str,
        prepared: Prepared,
        turn: Option<&super::local_cli_setup::Turn>,
        #[cfg(unix)] coding: Option<super::external_coding::Admission>,
        #[cfg(test)] test: Option<LaunchTest>,
    ) -> Result<Reply, ProtoError> {
        self.require_external_root(session)?;
        super::session::check_input(request, text, &[])?;
        if text.len() > policy::MAX_PROMPT_BYTES || text.contains('\0') {
            return Err(ProtoError::new(
                ErrorCode::Invalid,
                "CLI text exceeds the input policy.",
            ));
        }
        let sess = self.session(session)?;
        let _lock = sess.lock.lock().await;
        // Resolve a lost acknowledgement before checking capacity, preflight freshness,
        // or context validity. Only the durable receipt decides duplicate identity.
        if let Some(receipt) = self.external_commit(&sess, |tx| {
            Store::external_prompt_in(tx, session, request).map(|receipt| (receipt, vec![]))
        })? {
            let existing = self.external_commit(&sess, |tx| {
                Store::admit_external_prompt_in(
                    tx,
                    session,
                    request,
                    text,
                    &receipt.key.op,
                    receipt.key.generation,
                    now_ms(),
                )
            })?;
            return Ok(admitted(&existing.receipt));
        }
        self.local_cli_setups.with_admission(turn, || {
            // Register BEFORE checking stopping, and retain the token through admission,
            // process cleanup, projection, durable settlement and active-slot release.
            let completion = self.external_tasks.token();
            if self.stopping.is_cancelled() || sess.removed.is_cancelled() {
                return Err(ProtoError::new(ErrorCode::Busy, "Engine is stopping."));
            }
            let permit = self
                .generations
                .clone()
                .try_acquire_owned()
                .map_err(|_| ProtoError::new(ErrorCode::Busy, "Execution capacity is busy."))?;
            if sess.active_run().is_some() {
                return Err(ProtoError::new(ErrorCode::Busy, "Conversation is busy."));
            }
            let binding = self.store.cli_binding(session)?.ok_or_else(unsupported)?;
            if let Some(turn) = turn {
                let row = self.store.session(session)?.ok_or_else(unsupported)?;
                if !self.local_cli_execution_enabled(super::local_cli_setup::binding_mode(
                    &binding.identity.launch_policy,
                )) || binding != turn.grant.binding
                    || row.model.as_deref() != Some(turn.grant.model.as_str())
                {
                    return Err(unsupported());
                }
            }
            // Validate before admission, but do not cache a launch spec across scheduling.
            // The actor repeats validation using the committed Store-authorized mode.
            let wants_coding = prepared.requires_permissions();
            #[cfg(unix)]
            if wants_coding != coding.is_some()
                || coding.as_ref().is_some_and(|c| {
                    c.owner == 0
                        || c.revoked.is_cancelled()
                        || std::time::Instant::now() >= c.expires
                })
            {
                return Err(unsupported());
            }
            #[cfg(not(unix))]
            if wants_coding {
                return Err(unsupported());
            }
            if wants_coding {
                prepared
                    .validate_binding(&binding.identity)
                    .map_err(|_| unsupported())?;
            } else {
                for mode in [policy::TurnMode::Fresh, policy::TurnMode::Resume] {
                    prepared
                        .launch_spec(&binding.identity, mode, text)
                        .map_err(|_| {
                            ProtoError::new(ErrorCode::Conflict, "CLI preflight no longer matches.")
                        })?;
                }
            }
            let tools = prepared.observed_tools();
            let decoder = events::Decoder::new(&binding.identity.cli_session_id, tools)
                .map_err(|_| unsupported())?;
            let decoder = if binding.identity.launch_policy
                == crate::execution::claude_cli::coding_policy::ID
            {
                decoder.with_cli_tools()
            } else {
                decoder
            };
            let op = crate::util::uuid_v4();
            let admission = self.external_commit(&sess, |tx| {
                if super::request_cancel::is_cancelled() {
                    return Ok((
                        Err(ProtoError::new(
                            ErrorCode::Cancelled,
                            "The request was cancelled before admission.",
                        )),
                        vec![],
                    ));
                }
                // Recheck after acquiring the Store transaction as well: time spent
                // waiting for SQLite or validating filesystem identity consumes TTL.
                if turn.is_some_and(|turn| turn.cancel.is_cancelled()) {
                    return Ok((Err(unsupported()), vec![]));
                }
                #[cfg(unix)]
                if coding.as_ref().is_some_and(|c| {
                    c.revoked.is_cancelled() || std::time::Instant::now() >= c.expires
                }) {
                    return Ok((Err(unsupported()), vec![]));
                }
                Store::admit_external_prompt_in(
                    tx,
                    session,
                    request,
                    text,
                    &op,
                    binding.generation,
                    now_ms(),
                )
                .map(|(admission, changes)| (Ok(admission), changes))
            })??;
            if !admission.newly_admitted {
                return Ok(admitted(&admission.receipt));
            }
            let receipt = admission.receipt;
            #[allow(unused_mut)]
            let mut launch = Launch {
                prepared,
                binding: binding.identity,
                text: text.to_owned(),
                grant: turn.map(|turn| turn.grant.clone()),
                #[cfg(unix)]
                coding: None,
                #[cfg(test)]
                test,
            };
            let cancel = self.stopping.child_token();
            *sess.active.lock().unwrap_or_else(|e| e.into_inner()) = Some(Active {
                run: receipt.id,
                cancel: cancel.clone(),
            });
            let lifetime = Lifetime {
                sess: sess.clone(),
                run: receipt.id,
                cancel,
                _permit: permit,
                _completion: completion,
            };
            // Two durable transitions, both BEFORE any subprocess exists. Neither is replayable.
            if let Err(error) = self
                .external_commit(&sess, |tx| {
                    Store::begin_external_launch_in(tx, &receipt.key, now_ms())
                })
                .and_then(|_| {
                    #[cfg(unix)]
                    if let Some(coding) = coding {
                        launch.coding = Some(
                            super::external_coding::Coding::reserve(
                                self,
                                &receipt,
                                lifetime.cancel.clone(),
                                coding,
                            )
                            .map_err(|_| unsupported())?,
                        );
                        return Ok(());
                    }
                    self.external_commit(&sess, |tx| {
                        Store::begin_external_run_in(tx, &receipt.key, now_ms())
                            .map(|(_, changes)| ((), changes))
                    })
                })
            {
                // No process exists. Preserve an explicit uncertain receipt instead
                // of abandoning an active receipt after expected reservation loss.
                let _ = self.external_commit(&sess, |tx| {
                    Store::settle_external_prompt_in(
                        tx,
                        &receipt.key,
                        &ExternalSettlementEvidence::default(),
                        now_ms(),
                    )
                });
                self.fence_failed_external(&sess, &receipt.key);
                return Err(error);
            }
            let reply = admitted(&receipt);
            let shared = self.clone();
            self.spawn(async move {
                shared
                    .drive_external(lifetime, receipt, launch, decoder)
                    .await;
            });
            Ok(reply)
        })
    }

    /// Read-only duplicate lookup still uses Store admission to compare the payload
    /// fingerprint. It deliberately precedes trust, auth probes and capacity checks.
    pub(super) fn external_duplicate(
        &self,
        session: &str,
        request: &str,
        text: &str,
    ) -> Result<Option<Reply>, ProtoError> {
        let sess = self.session(session)?;
        self.external_commit(&sess, |tx| {
            let Some(receipt) = Store::external_prompt_in(tx, session, request)? else {
                return Ok((None, vec![]));
            };
            let (result, changes) = Store::admit_external_prompt_in(
                tx,
                session,
                request,
                text,
                &receipt.key.op,
                receipt.key.generation,
                now_ms(),
            )?;
            Ok((Some(admitted(&result.receipt)), changes))
        })
    }

    pub(super) async fn stop_external_prompt(&self, session: &str) -> Result<Reply, ProtoError> {
        self.require_external_root(session)?;
        let sess = self.session(session)?;
        let _lock = sess.lock.lock().await;
        let active = sess
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|a| (a.run, a.cancel.clone()));
        let Some((run, cancel)) = active else {
            return Ok(Reply::Stopping { run: None });
        };
        let receipt = self
            .store
            .active_external_prompts()?
            .into_iter()
            .find(|r| r.id == run && r.key.session == session)
            .ok_or_else(|| {
                ProtoError::new(ErrorCode::Conflict, "External receipt is not active.")
            })?;
        let stopped = self.external_commit(&sess, |tx| {
            Store::stop_external_prompt_in(tx, &receipt.key, now_ms())
        });
        // Commit intent before cancellation. Even a broken/fenced binding must not
        // keep an owned process running; report the commit error without claiming
        // durable Stop or clean settlement in that exceptional case.
        cancel.cancel();
        stopped?;
        Ok(Reply::Stopping { run: Some(run) })
    }

    async fn drive_external(
        &self,
        lifetime: Lifetime,
        receipt: ExternalPromptReceipt,
        launch: Launch,
        decoder: events::Decoder,
    ) {
        let sess = &lifetime.sess;
        let cancel = &lifetime.cancel;
        #[cfg(any(test, unix))]
        let mut launch = launch;
        #[cfg(test)]
        if let Some(gate) = launch.test.as_mut().and_then(|test| test.gate.take()) {
            tokio::select! {
                _ = gate => {},
                _ = cancel.cancelled() => {},
                _ = sess.removed.cancelled() => {},
            }
        }
        if launch
            .grant
            .as_ref()
            .is_some_and(|g| g.cancel.is_cancelled())
        {
            // Persist Stop before cancellation; a failed commit fences launch too.
            let _ = self.stop_external_prompt(&receipt.key.session).await;
            cancel.cancel();
        }
        #[cfg(unix)]
        let bound = if let Some(coding) = launch.coding.as_mut() {
            match coding.bind().await {
                Ok(bound) => Some(bound),
                Err(_) => {
                    self.finish_external(&lifetime, receipt, ExternalSettlementEvidence::default())
                        .await;
                    return;
                }
            }
        } else {
            None
        };
        // No await between this revalidation and polling the owned supervisor below.
        // A rejected/expired candidate has durable launch intent but no trustworthy
        // process evidence: settle Unknown, never Completed or cleanly Cancelled.
        let spec = if cancel.is_cancelled() || sess.removed.is_cancelled() {
            Err("cancelled before launch")
        } else {
            let mode = match receipt.mode {
                ExternalPromptMode::Fresh => policy::TurnMode::Fresh,
                ExternalPromptMode::Resume => policy::TurnMode::Resume,
            };
            #[cfg(unix)]
            let spec = if let Some(bound) = &bound {
                launch
                    .prepared
                    .launch_write_spec(&launch.binding, mode, &launch.text, &bound.hook)
            } else {
                launch
                    .prepared
                    .launch_spec(&launch.binding, mode, &launch.text)
            };
            #[cfg(not(unix))]
            let spec = launch
                .prepared
                .launch_spec(&launch.binding, mode, &launch.text);
            #[cfg(unix)]
            let spec = if launch
                .coding
                .as_ref()
                .is_some_and(|c| !c.broker.permits_launch())
            {
                Err("coding lease retired before spawn")
            } else {
                spec
            };
            spec
        };
        let Ok(spec) = spec else {
            #[cfg(unix)]
            {
                if let Some(coding) = &launch.coding {
                    coding.broker.fence();
                }
                drop(bound);
            }
            self.finish_external(&lifetime, receipt, ExternalSettlementEvidence::default())
                .await;
            return;
        };
        #[cfg(test)]
        let spec = {
            let mut spec = spec;
            if let Some(timeout) = launch.test.and_then(|test| test.timeout) {
                spec.timeout = timeout;
            }
            spec
        };
        let decoder = Arc::new(Mutex::new(ObservedDecoder {
            decoder: Some(decoder),
            error: None,
            rejected_record: None,
            parse_failure: None,
            presentation_incomplete: false,
            diagnostic: None,
        }));
        let (tx, mut rx) = mpsc::channel(EVENT_CAPACITY);
        let observed_decoder = decoder.clone();
        let observed_tx = tx.clone();
        let observer: process::AsyncStdoutObserver = Box::new(move |bytes| {
            let observed_decoder = observed_decoder.clone();
            let observed_tx = observed_tx.clone();
            Box::pin(async move {
                // One bounded pipe chunk plus decoder's bounded line/projected-byte budget.
                // Release the decoder lock before applying asynchronous backpressure.
                let (events, failed) = {
                    let mut observed = observed_decoder
                        .try_lock()
                        .map_err(|_| "projection unavailable")?;
                    let inner = observed.decoder.as_mut().ok_or("projection unavailable")?;
                    let mut events = Vec::new();
                    let result = inner.feed(&bytes, |event| {
                        events.push(event);
                        Ok::<_, ()>(())
                    });
                    let warning = inner.summary().presentation_incomplete;
                    let diagnostic = inner.diagnostic();
                    if let Err(error) = result {
                        let record = inner.failed_record();
                        let failure = inner.parse_failure();
                        observed.rejected_record = record;
                        observed.parse_failure = failure;
                        observed.error = Some(error);
                    }
                    observed.presentation_incomplete |= warning;
                    observed.diagnostic = diagnostic;
                    (events, result.is_err())
                };
                for event in events {
                    observed_tx
                        .send(event)
                        .await
                        .map_err(|_| "projection unavailable")?;
                }
                if failed {
                    Err("projection rejected")
                } else {
                    Ok(())
                }
            })
        });
        #[cfg(unix)]
        let before_signal = launch.coding.as_ref().map(|coding| {
            let broker = coding.broker.clone();
            // The supervisor also signals descendants after a normal root exit.
            // Seal authority here, but do not turn its cleanup into user cancellation.
            Box::new(move || broker.seal().revocation_confirmed) as process::BeforeSignal
        });
        #[cfg(not(unix))]
        let before_signal = None;
        let supervise = async {
            let supervisor =
                process::run_observed_async(spec, cancel.clone(), before_signal, observer);
            tokio::pin!(supervisor);
            let revoked = async {
                #[cfg(unix)]
                if let Some(coding) = &launch.coding {
                    coding.broker.consent_retired().await;
                    return;
                }
                if let Some(grant) = &launch.grant {
                    grant.cancel.cancelled().await;
                } else {
                    std::future::pending::<()>().await;
                }
            };
            let report = tokio::select! {
                biased;
                _ = revoked => {
                    let _ = self.stop_external_prompt(&receipt.key.session).await;
                    // On storage failure commit already fences this lifetime.
                    cancel.cancel();
                    supervisor.await
                }
                report = &mut supervisor => report,
                _ = sess.removed.cancelled() => {
                    cancel.cancel();
                    supervisor.await
                }
            };
            // No observer remains active. EOF itself is not evidence of success.
            let mut final_events = Vec::new();
            let mut finished = decoder.try_lock().ok().is_some_and(|mut observed| {
                let Some(inner) = observed.decoder.take() else {
                    return false;
                };
                match inner.finish(|event| {
                    final_events.push(event);
                    Ok::<_, ()>(())
                }) {
                    Ok(summary) => {
                        observed.presentation_incomplete |= summary.presentation_incomplete;
                        true
                    }
                    Err(error) => {
                        observed.error.get_or_insert(error);
                        observed
                            .rejected_record
                            .get_or_insert(events::RecordClass::EndOfStream);
                        false
                    }
                }
            });
            for event in final_events {
                if tx.send(event).await.is_err() {
                    finished = false;
                    break;
                }
            }
            drop(tx);
            (report, finished)
        };
        let consume = async {
            let mut cursor = receipt.cursor;
            let mut projection_ok = true;
            let mut action_results_known = true;
            let mut batch = Vec::with_capacity(EVENT_CAPACITY);
            while rx.recv_many(&mut batch, EVENT_CAPACITY).await != 0 {
                action_results_known &= !batch.iter().any(|event| {
                    matches!(
                        event,
                        events::Event::ExternalToolResult { is_error: None, .. }
                    )
                });
                if projection_ok {
                    let _lock = sess.lock.lock().await;
                    let next = cursor + batch.len() as u64;
                    match self.external_commit(sess, |tx| {
                        let mut changes: Vec<SessionChange> = Vec::new();
                        let mut entries = std::collections::HashMap::<u64, usize>::new();
                        for (offset, event) in batch.iter().enumerate() {
                            for change in Store::project_external_event_in(
                                tx,
                                &receipt.key,
                                cursor + offset as u64 + 1,
                                event,
                                now_ms(),
                            )? {
                                if let SessionChange::EntryAppended { entry }
                                | SessionChange::EntryReplaced { entry } = &change
                                {
                                    if let Some(position) = entries.get(&entry.id)
                                        && let SessionChange::EntryAppended { entry: prior }
                                        | SessionChange::EntryReplaced { entry: prior } =
                                            &mut changes[*position]
                                    {
                                        *prior = entry.clone();
                                        continue;
                                    }
                                    entries.insert(entry.id, changes.len());
                                }
                                changes.push(change);
                            }
                        }
                        Ok(((), changes))
                    }) {
                        Ok(()) => cursor = next,
                        Err(_) => {
                            projection_ok = false;
                            cancel.cancel();
                        }
                    }
                }
                batch.clear(); // Drain after failure; never retry a delta.
            }
            projection_ok && action_results_known
        };
        let owned_supervise = async {
            #[cfg(unix)]
            if let Some(bound) = bound {
                let coding = launch.coding.as_ref().expect("bound coding resources");
                let listener = bound.server.run_optional(bound.reviews);
                tokio::pin!(listener);
                tokio::pin!(supervise);
                let (result, listener_finished) = tokio::select! {
                    biased;
                    result = &mut supervise => (result, false),
                    _ = &mut listener => { coding.broker.fence(); (supervise.await, true) },
                };
                let proof = coding.seal();
                coding.broker.fence();
                if !listener_finished {
                    listener.await;
                }
                return (result, Some(proof));
            }
            (
                supervise.await,
                None::<(super::external_permissions::Seal, bool)>,
            )
        };
        // Both supervisor and listener are owned here; neither is detached.
        let (((report, finished), proof), projected) = tokio::join!(owned_supervise, consume);
        let mut evidence = report
            .as_ref()
            .map(|report| evidence(report, finished && projected))
            .unwrap_or_default();
        if let Ok(observed) = decoder.try_lock() {
            evidence.projection_error = observed.error;
            evidence.rejected_record = observed.rejected_record;
            evidence.parse_failure = observed.parse_failure;
            evidence.presentation_incomplete = observed.presentation_incomplete;
            evidence.stream_diagnostic = observed.diagnostic;
        }
        if let Some((seal, completion_eligible)) = proof {
            evidence.projection_finished &= completion_eligible
                && report
                    .as_ref()
                    .is_ok_and(|r| r.approval_revocation_confirmed == Some(true));
            evidence.cancelled_clean_confirmed &= seal.revocation_confirmed
                && report
                    .as_ref()
                    .is_ok_and(|r| r.approval_revocation_confirmed == Some(true));
        }
        self.finish_external(&lifetime, receipt, evidence).await;
    }

    async fn finish_external(
        &self,
        lifetime: &Lifetime,
        receipt: ExternalPromptReceipt,
        evidence: ExternalSettlementEvidence,
    ) {
        let sess = &lifetime.sess;
        let _lock = sess.lock.lock().await;
        if sess.failure().is_some() || sess.removed.is_cancelled() {
            self.fence_failed_external(sess, &receipt.key);
            lifetime.clear_active();
            return; // Leave receipt unfinished for startup recovery; no false settlement.
        }
        let settlement = self.external_commit(sess, |tx| {
            Store::settle_external_prompt_in(tx, &receipt.key, &evidence, now_ms())
        });
        // Still under the session lock, after the owned process has drained/reaped.
        lifetime.clear_active();
        match settlement {
            Ok(settlement) if settlement.newly_settled => {
                if let Some(outcome) = settlement.receipt.outcome {
                    let _ = sess.settled.send(Settled {
                        run: receipt.id,
                        request: Some(receipt.request),
                        outcome,
                    });
                }
                sess.changed.notify_waiters();
                // Deliberately no native goal completion, queue continuation or extensions.
            }
            Ok(_) => {}
            Err(_) => self.fence_failed_external(sess, &receipt.key),
        }
    }

    /// Best effort only, after cleanup. Do not settle on a broken store. The generation
    /// CAS fences unsafe reuse while preserving the unfinished receipt for recovery.
    fn fence_failed_external(&self, sess: &SessionRt, key: &ExternalRunKey) {
        if sess.failure().is_none() {
            return;
        }
        let _ = tokio::task::block_in_place(|| {
            self.store.tx(|tx| {
            tx.execute(
                "UPDATE cli_bindings SET context_valid=0,generation=generation+1 WHERE session=?1 AND generation=?2 AND context_valid=1",
                rusqlite::params![key.session, key.generation],
            )?;
            Ok(())
        })
        });
    }
}

/// Completion of delivery/transport, not inferred execution success. A handled CLI
/// error may exit 1; Store separately requires matching terminal outcome/exit evidence.
/// Every supervisor stop (even with exit 0) still vetoes completion.
fn evidence(report: &process::Report, decoder_finished: bool) -> ExternalSettlementEvidence {
    ExternalSettlementEvidence {
        projection_finished: decoder_finished
            && matches!(report.exit_code, Some(0 | 1))
            && report.stop.is_none()
            && report.stdin_delivered
            && report.root_reaped
            && report.pipes_drained
            && !report.cleanup_timed_out
            && report.approval_revocation_confirmed != Some(false),
        input_delivered: report.stdin_delivered,
        root_reaped: report.root_reaped,
        pipes_drained: report.pipes_drained,
        exit_code: report.exit_code,
        stop_requested: report.stop == Some(process::Stop::Cancelled),
        cleanup_timed_out: report.cleanup_timed_out,
        cancelled_clean_confirmed: report.stop == Some(process::Stop::Cancelled)
            && report.root_reaped
            && report.pipes_drained
            && !report.cleanup_timed_out
            && report.approval_revocation_confirmed != Some(false),
        supervisor_stop: report.stop,
        projection_error: None,
        rejected_record: None,
        parse_failure: None,
        presentation_incomplete: false,
        stream_diagnostic: None,
    }
}
