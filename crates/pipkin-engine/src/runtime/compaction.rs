//! Compaction (reference `durable/src/harness/compaction.ts`): summarize the context before a
//! cut so the next request fits.
//!
//! Triggers: a manual `Compact` call; before a request, over `contextWindow - reserveTokens`
//! (blocking: the request waits) or over the background threshold (background: the request
//! proceeds and the summary lands when ready); after a context-overflow error (blocking, once
//! per attempt). The summary request uses the session's model with no tools and no prompt
//! caching. A summary whose cut was already passed by a newer compaction is dropped as stale.
//! Every attempt's usage is recorded on the session's model bucket.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use pipkin_engine_protocol::*;
use serde_json::Map;
use tokio_util::sync::CancellationToken;

use super::Shared;
use super::session::SessionRt;
use crate::context::compaction::{self as algo, Threshold};
use crate::provider::{CacheRetention, StreamCtx, errors};
use crate::store::Store;
use crate::util::now_ms;

/// What a compaction attempt did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Compacted {
    /// A compaction entry was committed.
    Done(EntryId),
    /// Nothing before a valid cut, or the cut was passed by a newer compaction.
    Nothing(String),
}

impl Shared {
    /// Summarize the session's context now. `run` attributes the entry to a run (blocking
    /// compactions inside a run); `None` for manual and background ones.
    pub async fn compact(
        self: &Arc<Self>,
        sess: &Arc<SessionRt>,
        run: Option<u64>,
        instructions: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<Compacted, String> {
        let prepared = match self.prepare(sess).await {
            Ok(p) => p,
            Err(Some(outcome)) => {
                return Err(match outcome {
                    RunOutcome::NoModel { detail } | RunOutcome::ModelError { detail } => detail,
                    other => format!("{other:?}"),
                });
            }
            Err(None) => return Err("The session could not be read.".into()),
        };
        let settings = &prepared.settings;
        let Some(first_kept) =
            algo::select_cut(&prepared.entries, settings.compaction.keep_recent_tokens)
        else {
            return Ok(Compacted::Nothing(
                "Nothing to compact: the conversation is too short.".into(),
            ));
        };
        let source = algo::summary_source(&prepared.entries, first_kept);
        let tokens_before = algo::context_tokens(&prepared.entries);
        let mut req = prepared.req.clone();
        req.system = algo::SUMMARIZATION_SYSTEM_PROMPT.to_owned();
        req.messages = vec![Message::User {
            content: vec![Block::text(algo::summary_request(&source, instructions))],
            timestamp: now_ms(),
            extra: Map::new(),
        }];
        req.tools = vec![];
        req.cache_retention = CacheRetention::None;
        req.max_tokens = Some(algo::summary_max_tokens(
            settings.compaction.reserve_tokens,
            prepared.model.max_tokens,
        ));
        let retry = settings.retry.clone();
        let mut attempt = 0u32;
        let summary = loop {
            attempt += 1;
            let message = prepared
                .adapter
                .stream(StreamCtx {
                    req: &req,
                    http: &prepared.http,
                    events: None,
                    cancel: cancel.clone(),
                })
                .await;
            self.record_usage(sess, &prepared.model.key(), &message);
            match algo::validate_summary(&message) {
                Ok(summary) => break summary,
                Err(error) => {
                    if cancel.is_cancelled() {
                        return Err("Compaction aborted.".into());
                    }
                    if retry.enabled
                        && attempt <= retry.max_retries
                        && errors::is_retryable(&message)
                    {
                        let wait = errors::retry_delay_ms(
                            attempt,
                            retry.base_delay_ms,
                            retry.max_agent_delay_ms,
                        );
                        tokio::select! {
                            _ = cancel.cancelled() => return Err("Compaction aborted.".into()),
                            _ = tokio::time::sleep(Duration::from_millis(wait)) => {}
                        }
                        continue;
                    }
                    return Err(error);
                }
            }
        };
        let session = sess.id.clone();
        self.commit(sess, |tx| {
            // A newer compaction that already keeps less than this one makes it stale.
            if let Some((_, newest_first_kept)) = Store::last_head(tx, &session)?
                && newest_first_kept >= first_kept
            {
                return Ok((
                    Compacted::Nothing(
                        "Compaction summary dropped: the context changed under it.".into(),
                    ),
                    vec![],
                ));
            }
            let entry = Store::append_entry(
                tx,
                &session,
                now_ms(),
                run,
                &EntryKind::Compaction {
                    summary: summary.clone(),
                    first_kept,
                    tokens_before,
                },
            )?;
            let id = entry.id;
            Ok((
                Compacted::Done(id),
                vec![
                    SessionChange::EntryAppended { entry },
                    SessionChange::BoundaryMoved { has_older: true },
                ],
            ))
        })
        .map_err(|e| e.message)
    }

    fn record_usage(&self, sess: &SessionRt, key: &str, message: &Message) {
        let Message::Assistant { usage, .. } = message else {
            return;
        };
        if usage.total_tokens == 0 && usage.cost.total == 0.0 {
            return;
        }
        let session = sess.id.clone();
        let _ = self.commit(sess, |tx| {
            Store::add_usage(tx, &session, "model", key, usage)?;
            Ok(((), vec![]))
        });
    }

    /// Before a request: compact when over a threshold. Returns true when a blocking compaction
    /// committed (the caller re-prepares the request).
    pub(super) async fn compact_if_needed(
        self: &Arc<Self>,
        sess: &Arc<SessionRt>,
        run: u64,
        cancel: &CancellationToken,
    ) -> bool {
        let Ok(prepared) = self.prepare(sess).await else {
            return false;
        };
        let tokens = algo::context_tokens(&prepared.entries)
            + algo::estimate_text_tokens(&prepared.req.system);
        match algo::threshold_compaction(
            tokens,
            prepared.model.context_window,
            &prepared.settings.compaction,
        ) {
            Some(Threshold::Blocking) => {
                matches!(
                    self.compact(sess, Some(run), None, cancel).await,
                    Ok(Compacted::Done(_))
                )
            }
            Some(Threshold::Background) => {
                if !sess.compacting.swap(true, Ordering::SeqCst) {
                    let shared = self.clone();
                    let sess = sess.clone();
                    self.spawn(async move {
                        // Background work survives a Stop of the run (reference behavior).
                        let token = CancellationToken::new();
                        if let Err(e) = shared.compact(&sess, None, None, &token).await {
                            log::warn!("session {}: background compaction failed: {e}", sess.id);
                        }
                        sess.compacting.store(false, Ordering::SeqCst);
                    });
                }
                false
            }
            None => false,
        }
    }
}
