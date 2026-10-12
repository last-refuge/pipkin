//! Recoverable context compaction. Selection, responses, retries and settlement are
//! checkpointed. Credentials are resolved anew; summaries and usage commit exactly once.
use super::{Shared, session::SessionRt};
use crate::context::compaction::{self as algo, Threshold};
use crate::store::{
    Store,
    compactions::{CompactionProgress, CompactionScope},
};
use pipkin_engine_protocol::*;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

mod journal;
mod request;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Compacted {
    Done(EntryId),
    Submitted(u64),
    Nothing(String),
}

impl Shared {
    pub(super) fn flush_summary_writes(
        tx: &rusqlite::Connection,
        session: &str,
    ) -> crate::store::Result<Vec<SessionChange>> {
        let mut changes = Vec::new();
        for entry in Store::place_summary_writes(tx, session, crate::util::now_ms())? {
            changes.push(SessionChange::EntryAppended { entry });
            changes.push(SessionChange::BoundaryMoved { has_older: true });
        }
        Ok(changes)
    }

    pub(super) async fn compact_manual(
        self: &Arc<Self>,
        sess: &Arc<SessionRt>,
        instructions: Option<&str>,
        request_cancel: &CancellationToken,
    ) -> Result<Compacted, String> {
        self.compact_key(
            sess,
            None,
            &format!("manual:{}", crate::util::uuid_v4()),
            instructions,
            request_cancel,
        )
        .await
    }

    pub(super) async fn compact_key(
        self: &Arc<Self>,
        sess: &Arc<SessionRt>,
        run: Option<u64>,
        key: &str,
        instructions: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<Compacted, String> {
        let scope = if run.is_some() {
            CompactionScope::Blocking
        } else {
            CompactionScope::Manual
        };
        let id = {
            let _lock = sess.lock.lock().await;
            if cancel.is_cancelled() {
                return Err("Compaction aborted.".into());
            }
            if run.is_none()
                && self
                    .store
                    .read(|c| Store::pending_summaries(c, &sess.id))
                    .map_err(|e| e.to_string())?
                    >= 64
            {
                return Err("The summary queue is full. Wait for a conversation boundary.".into());
            }
            self.commit(sess, |tx| {
                let job = Store::admit_compaction(tx, &sess.id, key, run, scope, instructions)?;
                Ok((job.id, vec![]))
            })
            .map_err(|e| e.message)?
        };
        super::faults::crash_point("compaction_admitted");
        self.execute_compaction(sess, id, cancel).await
    }

    /// Resume owned work even if settings changed while the process was down.
    pub(super) async fn compact_if_needed(
        self: &Arc<Self>,
        sess: &Arc<SessionRt>,
        run: u64,
        cancel: &CancellationToken,
    ) -> bool {
        let pending = self.store.read(|c| Store::pending_run_compaction(c, run));
        match pending {
            Ok(Some(id)) => {
                return matches!(
                    self.execute_compaction(sess, id, cancel).await,
                    Ok(Compacted::Done(_))
                );
            }
            Err(e) => {
                self.storage_failure(sess, &e.to_string());
                return false;
            }
            Ok(None) => {}
        }
        let Ok(prepared) = self.prepare(sess).await else {
            return false;
        };
        let tokens = algo::context_tokens(&prepared.entries)
            + algo::estimate_text_tokens(&prepared.req.system);
        let tail = prepared.entries.last().map_or(0, |e| e.id);
        let threshold = algo::threshold_compaction(
            tokens,
            prepared.model.context_window,
            &prepared.settings.compaction,
        );
        // Release context/credentials before waiting for another operation or a slot.
        drop(prepared);
        match threshold {
            Some(Threshold::Blocking) => matches!(
                self.compact_key(
                    sess,
                    Some(run),
                    &format!("threshold:{run}:{tail}"),
                    None,
                    cancel
                )
                .await,
                Ok(Compacted::Done(_))
            ),
            Some(Threshold::Background) => {
                let _lock = sess.lock.lock().await;
                let admitted = self.commit(sess, |tx| {
                    use rusqlite::OptionalExtension;
                    let pending: Option<u64> = tx.query_row(
                        "SELECT id FROM compactions WHERE session = ?1 AND scope = 'background' AND state != 'settled' LIMIT 1",
                        [&sess.id], |r| r.get(0)).optional()?;
                    if pending.is_none() && Store::pending_summaries(tx, &sess.id)? < 64 {
                        Store::admit_compaction(tx, &sess.id, &format!("background:{tail}"), None,
                            CompactionScope::Background, None)?;
                    }
                    Ok(((), vec![]))
                });
                if admitted.is_ok() {
                    self.compaction_recovery.notify_one();
                }
                false
            }
            None => false,
        }
    }
}

/// One bounded recovery scheduler, shared by startup and explicit reopen. It holds only
/// small identity pages; at most eight recovery futures exist, independently of run permits.
/// A live caller and recovery may race, but the per-session claim admits only one driver.
pub(super) fn start_recovery(shared: &Arc<Shared>) {
    let shared = shared.clone();
    let owner = shared.clone();
    owner.spawn(async move {
        let mut tasks = tokio::task::JoinSet::new();
        loop {
            let mut after = 0;
            loop {
                if shared.stopping.is_cancelled() {
                    return;
                }
                let page = match shared.store.pending_compactions_page(after, 32) {
                    Ok(page) => page,
                    Err(error) => {
                        log::error!("cannot enumerate compactions: {error}");
                        break;
                    }
                };
                if page.is_empty() {
                    break;
                }
                for (id, session) in page {
                    after = id;
                    while tasks.len() >= 8 {
                        tokio::select! {
                            _ = shared.stopping.cancelled() => return,
                            _ = tasks.join_next() => {}
                        }
                    }
                    let Ok(sess) = shared.session(&session) else {
                        continue;
                    };
                    if sess.failure().is_some()
                        || sess.active_compactions.lock().unwrap().contains(&id)
                    {
                        continue;
                    }
                    let shared = shared.clone();
                    tasks.spawn(async move {
                        let token = CancellationToken::new();
                        if let Err(e) = shared.execute_compaction(&sess, id, &token).await {
                            log::debug!("session {}: recovered compaction {id}: {e}", sess.id);
                        }
                    });
                }
            }
            loop {
                tokio::select! {
                    _ = shared.stopping.cancelled() => return,
                    _ = shared.compaction_recovery.notified() => break,
                    _ = tasks.join_next(), if !tasks.is_empty() => {}
                }
            }
        }
    });
}
