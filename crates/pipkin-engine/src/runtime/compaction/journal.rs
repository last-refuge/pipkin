use super::*;
use crate::provider::{CacheRetention, StreamCtx, errors};
use crate::store::compactions::{
    CompactionJob, CompactionOutcome as Outcome, SummaryRequest, SummaryResponse,
};
use crate::util::now_ms;
use std::sync::atomic::Ordering;
use std::time::Duration;

struct Claim {
    sess: Arc<SessionRt>,
    id: u64,
    cancel: CancellationToken,
    background: bool,
}
impl Drop for Claim {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.sess
            .manual_compactions
            .lock()
            .unwrap()
            .remove(&self.id);
        if self.background {
            self.sess.compacting.store(false, Ordering::SeqCst);
        }
        self.sess
            .active_compactions
            .lock()
            .unwrap()
            .remove(&self.id);
        self.sess.changed.notify_waiters();
    }
}

impl Shared {
    fn compaction_job(&self, sess: &SessionRt, id: u64) -> Result<CompactionJob, String> {
        match self.store.read(|c| Store::compaction_in(c, id)) {
            Ok(Some(job)) if job.session == sess.id => Ok(job),
            Ok(_) => Err("Compaction not found in this session".into()),
            Err(e) => {
                self.storage_failure(sess, &e.to_string());
                Err(e.to_string())
            }
        }
    }

    fn checkpoint(
        &self,
        sess: &SessionRt,
        job: &CompactionJob,
        progress: &CompactionProgress,
    ) -> Result<(), String> {
        self.commit(sess, |tx| {
            Ok((Store::save_compaction(tx, job, progress)?, vec![]))
        })
        .map(|_| ())
        .map_err(|e| e.message)
    }

    /// Also handles cancellation when recovery won the execution claim. Persist first, then
    /// signal the live driver. A cancelled RPC cannot abandon a queued durable operation.
    async fn cancel_compaction(&self, sess: &SessionRt, id: u64) -> Result<(), String> {
        let _lock = sess.lock.lock().await;
        let job = self.compaction_job(sess, id)?;
        self.checkpoint(
            sess,
            &job,
            &CompactionProgress::Terminal {
                result: Outcome::Aborted,
            },
        )?;
        if let Some(token) = sess.manual_compactions.lock().unwrap().get(&id) {
            token.cancel();
        }
        Ok(())
    }

    pub(in crate::runtime) async fn execute_compaction(
        self: &Arc<Self>,
        sess: &Arc<SessionRt>,
        id: u64,
        external: &CancellationToken,
    ) -> Result<Compacted, String> {
        self.require_native_execution(&sess.id)
            .map_err(|e| e.message)?;
        tokio::select! {
            biased;
            _ = sess.removed.cancelled() => Err("Session removal is committed".into()),
            result = self.execute_compaction_live(sess, id, external) => result,
        }
    }

    async fn execute_compaction_live(
        self: &Arc<Self>,
        sess: &Arc<SessionRt>,
        id: u64,
        external: &CancellationToken,
    ) -> Result<Compacted, String> {
        // Waiting callers retain only an identity. Do not hold a permit or decoded prompt
        // while waiting for another executor of this same operation.
        loop {
            let changed = sess.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.stopping.is_cancelled() {
                return Err("Engine shutting down".into());
            }
            if let Some(error) = sess.failure() {
                return Err(error);
            }
            if sess.active_compactions.lock().unwrap().insert(id) {
                break;
            }
            tokio::select! {
                _ = self.stopping.cancelled() => return Err("Engine shutting down".into()),
                _ = external.cancelled() => {
                    self.cancel_compaction(sess, id).await?;
                    return Err("Compaction aborted.".into());
                }
                _ = &mut changed => {}
            }
        }
        let token = external.child_token();
        let mut claim = Claim {
            sess: sess.clone(),
            id,
            cancel: token.clone(),
            background: false,
        };
        // Register before waiting for capacity so Stop reaches foreground queued work too.
        {
            let _lock = sess.lock.lock().await;
            let scope = self
                .store
                .read(|c| {
                    Ok(
                        c.query_row("SELECT scope FROM compactions WHERE id = ?1", [id], |r| {
                            r.get::<_, String>(0)
                        })?,
                    )
                })
                .map_err(|e| {
                    self.storage_failure(sess, &e.to_string());
                    e.to_string()
                })?;
            claim.background = scope == "background";
            if claim.background {
                sess.compacting.store(true, Ordering::SeqCst);
            } else {
                sess.manual_compactions
                    .lock()
                    .unwrap()
                    .insert(id, token.clone());
            }
        }
        let _permit = tokio::select! {
            _ = self.stopping.cancelled() => return Err("Engine shutting down".into()),
            _ = token.cancelled() => {
                self.cancel_compaction(sess, id).await?;
                return Err("Compaction aborted.".into());
            }
            p = self.compaction_slots.acquire() => p.map_err(|e| e.to_string())?,
        };
        loop {
            if self.stopping.is_cancelled() {
                return Err("Engine shutting down".into());
            }
            if let Some(error) = sess.failure() {
                return Err(error);
            }
            let job = self.compaction_job(sess, id)?;
            if let CompactionProgress::Terminal { result } = &job.progress {
                return match result {
                    Outcome::Done { entry } => {
                        self.publish_context(&sess.id);
                        Ok(Compacted::Done(*entry))
                    }
                    Outcome::Submitted { submission } => {
                        self.next_from_queue(sess).await;
                        Ok(Compacted::Submitted(*submission))
                    }
                    Outcome::Nothing { reason } => Ok(Compacted::Nothing(reason.clone())),
                    Outcome::Failed { error } => Err(error.clone()),
                    Outcome::Aborted => Err("Compaction aborted.".into()),
                };
            }
            // A blocking job can be recovered independently, including after its owner was
            // durably stopped/settled before the recovery scheduler reached it.
            let orphaned = if let Some(run) = job.run {
                self.store
                    .read(|c| {
                        Ok(Store::run_in(c, run)?
                            .is_none_or(|r| r.stop_requested || r.view().is_none()))
                    })
                    .map_err(|e| {
                        self.storage_failure(sess, &e.to_string());
                        e.to_string()
                    })?
            } else {
                false
            };
            if job.stopping || token.is_cancelled() || orphaned {
                self.checkpoint(
                    sess,
                    &job,
                    &CompactionProgress::Terminal {
                        result: Outcome::Aborted,
                    },
                )?;
                continue;
            }
            match &job.progress {
                CompactionProgress::Select => {
                    let selected = tokio::select! {
                        biased;
                        _ = self.stopping.cancelled() => return Err("Engine shutting down".into()),
                        _ = token.cancelled() => continue,
                        result = self.prepare(sess) => result,
                    };
                    let prepared = match selected {
                        Ok(prepared) => prepared,
                        Err(error) => {
                            if error.is_none() && sess.failure().is_none() {
                                self.storage_failure(sess, "Cannot read compaction input");
                            }
                            if sess.failure().is_some() {
                                return Err("Compaction storage failure".into());
                            }
                            let detail = match error {
                                Some(
                                    RunOutcome::NoModel { detail }
                                    | RunOutcome::ModelError { detail },
                                ) => detail,
                                other => format!("Cannot select compaction input: {other:?}"),
                            };
                            self.checkpoint(
                                sess,
                                &job,
                                &CompactionProgress::Terminal {
                                    result: Outcome::Failed { error: detail },
                                },
                            )?;
                            continue;
                        }
                    };
                    let Some(first_kept) = algo::select_cut(
                        &prepared.entries,
                        prepared.settings.compaction.keep_recent_tokens,
                    ) else {
                        self.checkpoint(
                            sess,
                            &job,
                            &CompactionProgress::Terminal {
                                result: Outcome::Nothing {
                                    reason: "Nothing to compact: the conversation is too short."
                                        .into(),
                                },
                            },
                        )?;
                        continue;
                    };
                    let mut req = prepared.request_template;
                    req.system = algo::SUMMARIZATION_SYSTEM_PROMPT.into();
                    req.messages = vec![Message::User {
                        content: vec![Block::text(algo::summary_request(
                            &algo::summary_source(&prepared.entries, first_kept),
                            job.instructions.as_deref(),
                        ))],
                        timestamp: now_ms(),
                        extra: Default::default(),
                    }];
                    req.tools.clear();
                    req.cache_retention = CacheRetention::None;
                    req.max_tokens = Some(algo::summary_max_tokens(
                        prepared.settings.compaction.reserve_tokens,
                        prepared.model.max_tokens,
                    ));
                    let request = SummaryRequest::capture(
                        &req,
                        prepared.entries.last().map_or(0, |e| e.id),
                        first_kept,
                        algo::context_tokens(&prepared.entries),
                    );
                    self.checkpoint(
                        sess,
                        &job,
                        &CompactionProgress::Summarize {
                            request: Box::new(request),
                            attempt: 1,
                            response: None,
                        },
                    )?;
                    super::super::faults::crash_point("compaction_selected");
                }
                CompactionProgress::Summarize {
                    request,
                    attempt,
                    response: None,
                } => {
                    let transport = tokio::select! {
                        biased;
                        _ = self.stopping.cancelled() => return Err("Engine shutting down".into()),
                        _ = token.cancelled() => continue,
                        result = self.summary_request(request) => result,
                    };
                    let (req, adapter, http, net) = match transport {
                        Ok(t) => t,
                        Err(error) => {
                            self.checkpoint(
                                sess,
                                &job,
                                &CompactionProgress::Terminal {
                                    result: Outcome::Failed { error },
                                },
                            )?;
                            continue;
                        }
                    };
                    let message = tokio::select! {
                        biased;
                        _ = self.stopping.cancelled() => return Err("Engine shutting down".into()),
                        _ = token.cancelled() => continue,
                        message = adapter.stream(StreamCtx { net: Some(&net), req: &req, http: &http, events: None, cancel: token.clone() }) => message,
                    };
                    if self.stopping.is_cancelled() {
                        return Err("Engine shutting down".into());
                    }
                    if token.is_cancelled() {
                        continue;
                    }
                    self.checkpoint(
                        sess,
                        &job,
                        &CompactionProgress::Summarize {
                            request: request.clone(),
                            attempt: *attempt,
                            response: Some(Box::new(SummaryResponse {
                                message,
                                received_at: now_ms(),
                            })),
                        },
                    )?;
                    super::super::faults::crash_point("compaction_response_saved");
                }
                CompactionProgress::Summarize {
                    request,
                    attempt,
                    response: Some(response),
                } => {
                    let _lock = sess.lock.lock().await;
                    if token.is_cancelled() {
                        continue;
                    }
                    self.classify_compaction(sess, &job, request, *attempt, response)?;
                    super::super::faults::crash_point("compaction_classified");
                }
                CompactionProgress::Retry {
                    request,
                    attempt,
                    until,
                } => {
                    let wait = Duration::from_millis(until.saturating_sub(now_ms()).max(0) as u64);
                    tokio::select! {
                        biased;
                        _ = self.stopping.cancelled() => return Err("Engine shutting down".into()),
                        _ = token.cancelled() => continue,
                        _ = tokio::time::sleep(wait) => {}
                    }
                    self.checkpoint(
                        sess,
                        &job,
                        &CompactionProgress::Summarize {
                            request: request.clone(),
                            attempt: attempt.saturating_add(1),
                            response: None,
                        },
                    )?;
                }
                CompactionProgress::Terminal { .. } => unreachable!(),
            }
        }
    }

    fn classify_compaction(
        &self,
        sess: &SessionRt,
        job: &CompactionJob,
        request: &SummaryRequest,
        attempt: u32,
        response: &SummaryResponse,
    ) -> Result<(), String> {
        // Like the reference, retry policy is live; only attempt/deadline and the
        // selected provider request are pinned by the operation.
        let retry = self.settings_for(&sess.id).retry;
        self.commit(sess, |tx| {
            // Check revision before ANY side effect. Stop or a cancelled waiter may have
            // settled the job after its response was read. All changes below share this tx.
            let current = Store::compaction_in(tx, job.id)?.ok_or_else(|| {
                crate::store::StoreError::Corrupt("Compaction disappeared".into())
            })?;
            if current.revision != job.revision || current.stopping {
                return Ok(((), vec![]));
            }
            if let Some(run) = job.run
                && Store::run_in(tx, run)?.is_none_or(|r| r.stop_requested || r.view().is_none())
            {
                Store::save_compaction(
                    tx,
                    job,
                    &CompactionProgress::Terminal {
                        result: Outcome::Aborted,
                    },
                )?;
                return Ok(((), vec![]));
            }
            let mut changes = vec![];
            let progress = match algo::validate_summary(&response.message) {
                Ok(summary) => {
                    let kind = EntryKind::Compaction {
                        summary,
                        first_kept: request.first_kept,
                        tokens_before: request.tokens_before,
                    };
                    let result = if job.scope == CompactionScope::Blocking {
                        let entry = Store::append_entry(tx, &sess.id, now_ms(), job.run, &kind)?;
                        let id = entry.id;
                        changes.extend([
                            SessionChange::EntryAppended { entry },
                            SessionChange::BoundaryMoved { has_older: true },
                        ]);
                        Outcome::Done { entry: id }
                    } else {
                        let submission = Store::admit_summary_write(tx, &sess.id, job.id, &kind)?;
                        if Store::open_run_in(tx, &sess.id)?.is_none() {
                            changes.extend(Self::flush_summary_writes(tx, &sess.id)?);
                        }
                        Outcome::Submitted { submission }
                    };
                    CompactionProgress::Terminal { result }
                }
                Err(error) => {
                    if retry.enabled
                        && attempt <= retry.max_retries
                        && errors::is_retryable(&response.message)
                    {
                        let delay = errors::retry_delay_ms(
                            attempt,
                            retry.base_delay_ms,
                            retry.max_agent_delay_ms,
                        );
                        CompactionProgress::Retry {
                            request: Box::new(request.clone()),
                            attempt,
                            until: response
                                .received_at
                                .saturating_add(i64::try_from(delay).unwrap_or(i64::MAX)),
                        }
                    } else {
                        CompactionProgress::Terminal {
                            result: Outcome::Failed { error },
                        }
                    }
                }
            };
            if !Store::save_compaction(tx, job, &progress)? {
                // Cannot happen under the single transaction writer, but roll back entry
                // placement rather than silently double-account if that invariant changes.
                return Err(crate::store::StoreError::Corrupt(
                    "Compaction revision changed inside transaction".into(),
                ));
            }
            if let Message::Assistant { usage, .. } = &response.message {
                Store::add_usage(tx, &sess.id, "model", &request.model, usage)?;
                changes.push(SessionChange::UsageChanged {
                    usage: Store::usage_in(tx, &sess.id)?,
                });
            }
            Ok(((), changes))
        })
        .map_err(|e| e.message)
    }
}
