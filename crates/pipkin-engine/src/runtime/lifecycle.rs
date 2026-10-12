//! Attachment demand and idle session retirement. The registry lock makes acquiring a
//! session Arc and removing its last registry-owned Arc mutually exclusive. Every executing
//! run, compaction and session operation holds an Arc; extension hosts hold only a Weak and
//! acquire demand through this registry before dispatch, so an old host cannot resurrect a
//! retired lifetime. History and unsettled work remain in the store.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pipkin_engine_protocol::{Call, ErrorCode, Event, ProtoError, SessionChange, UiView};
use tokio_util::sync::CancellationToken;

use super::{SessionRt, Shared, dispatch_boxed};

const INITIAL_GRACE: Duration = Duration::from_secs(10);
const DISCONNECT_GRACE: Duration = Duration::from_secs(30);
const EXTENSION_GRACE: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(super) struct Sessions {
    pub live: HashMap<String, Managed>,
    retiring: HashMap<String, Arc<CancellationToken>>,
}

pub(super) struct Managed {
    pub rt: Arc<SessionRt>,
    clients: HashSet<u64>,
    retire_after: Instant,
}

impl Shared {
    /// The runtime state of an existing session. Retaining this Arc holds retirement.
    pub fn session(&self, id: &str) -> Result<Arc<SessionRt>, ProtoError> {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = sessions.live.get(id) {
            return Ok(entry.rt.clone());
        }
        if self.store.session(id)?.is_none() {
            return Err(ProtoError::new(
                ErrorCode::NotFound,
                format!("Session {id} does not exist"),
            ));
        }
        let shared = self.me.clone();
        let session = id.to_owned();
        let predecessor = sessions.retiring.get(id).cloned();
        let rt = Arc::new_cyclic(|weak: &std::sync::Weak<SessionRt>| {
            let weak = weak.clone();
            let questions = crate::extensions::questions::QuestionHost::scoped(Some(Arc::new(
                move |ui: UiView| {
                    if let Some(shared) = shared.upgrade() {
                        // Serialize against retirement/reopen, including callbacks from a
                        // disposed question host or an extension finishing deactivation.
                        let sessions = shared.sessions.lock().unwrap_or_else(|e| e.into_inner());
                        if sessions.live.get(&session).is_some_and(|entry| {
                            std::ptr::eq(Arc::as_ptr(&entry.rt), weak.as_ptr())
                        }) {
                            shared.publish(Event::Session {
                                session: session.clone(),
                                revision: 0,
                                change: SessionChange::UiChanged { ui },
                            });
                        }
                    }
                },
            )));
            let mut rt = SessionRt::new(id, questions);
            rt.predecessor = predecessor;
            rt
        });
        sessions.live.insert(
            id.to_owned(),
            Managed {
                rt: rt.clone(),
                clients: HashSet::new(),
                retire_after: Instant::now() + INITIAL_GRACE,
            },
        );
        Ok(rt)
    }

    pub(super) fn attach_session(
        &self,
        client: u64,
        id: &str,
    ) -> Result<Arc<SessionRt>, ProtoError> {
        let rt = self.session(id)?;
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        // Our Arc prevents removal between lookup and attachment.
        let entry = sessions.live.get_mut(id).expect("session demand retained");
        entry.clients.insert(client);
        entry.retire_after = Instant::now();
        Ok(rt)
    }

    pub(super) fn release_session(&self, client: u64, id: &str) {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = sessions.live.get_mut(id)
            && entry.clients.remove(&client)
            && entry.clients.is_empty()
        {
            entry.retire_after = Instant::now();
        }
    }

    pub(super) fn release_client(&self, client: u64) {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        for entry in sessions.live.values_mut() {
            if entry.clients.remove(&client) && entry.clients.is_empty() {
                // Preserve warm resources across a short client reconnect. A new client
                // can adopt the same session with OpenSession during this interval.
                entry.retire_after = Instant::now() + DISCONNECT_GRACE;
            }
        }
    }

    pub(super) fn extension_dispatch(&self, sess: &Arc<SessionRt>) -> crate::extensions::Dispatch {
        let shared = self.me.clone();
        let session = sess.id.clone();
        let weak = Arc::downgrade(sess);
        Arc::new(move |call: Call| {
            let demand = shared.upgrade().and_then(|shared| {
                let rt = shared
                    .sessions
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .live
                    .get(&session)
                    .filter(|entry| std::ptr::eq(Arc::as_ptr(&entry.rt), weak.as_ptr()))
                    .map(|entry| entry.rt.clone());
                rt.map(|rt| (shared, rt))
            });
            Box::pin(async move {
                let Some((shared, _demand)) = demand else {
                    return Err(ProtoError::new(
                        ErrorCode::ShuttingDown,
                        "the session is closed",
                    ));
                };
                dispatch_boxed(shared, 0, call).await
            })
        })
    }

    pub(super) fn retire_idle(&self, now: Instant) {
        let mut retired = Vec::new();
        {
            let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            let ids: Vec<_> = sessions
                .live
                .iter()
                .filter(|(_, entry)| {
                    entry.clients.is_empty()
                        && entry.retire_after <= now
                        && Arc::strong_count(&entry.rt) == 1
                    // Preserve fail-closed state until explicit recovery on OpenSession.
                    && entry.rt.failure().is_none()
                        && entry
                            .rt
                            .predecessor
                            .as_ref()
                            .is_none_or(|p| p.is_cancelled())
                })
                .map(|(id, _)| id.clone())
                .collect();
            for id in ids {
                // The provider affinity id is deliberately distinct from the store id.
                // If the store cannot be read, retain resources and retry on the next tick.
                let Ok(provider_id) = self.store.kv_get(&id, "provider_session_id") else {
                    continue;
                };
                let entry = sessions
                    .live
                    .remove(&id)
                    .expect("selected under registry lock");
                // Close pooled transport before allowing a new lifetime to use this id.
                if let Some(provider_id) = provider_id {
                    for adapter in self.adapters.adapters() {
                        adapter.retire_session(&provider_id);
                    }
                }
                let done = Arc::new(CancellationToken::new());
                sessions.retiring.insert(id, done.clone());
                retired.push((entry.rt, done));
            }
        }
        for (sess, done) in retired {
            let shared = self.me.clone();
            self.spawn(async move {
                sess.questions.dispose();
                if let Some(runner) = sess.extensions.lock().await.take() {
                    runner.shutdown(EXTENSION_GRACE).await;
                }
                if let Some(shared) = shared.upgrade() {
                    let mut sessions = shared.sessions.lock().unwrap_or_else(|e| e.into_inner());
                    if sessions
                        .retiring
                        .get(&sess.id)
                        .is_some_and(|p| Arc::ptr_eq(p, &done))
                    {
                        sessions.retiring.remove(&sess.id);
                    }
                    done.cancel();
                } else {
                    done.cancel();
                }
            });
        }
    }
}

pub(super) fn start_reaper(shared: &Arc<Shared>) {
    let weak = Arc::downgrade(shared);
    shared.spawn(async move {
        let mut ticks = tokio::time::interval(Duration::from_millis(100));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticks.tick().await;
            let Some(shared) = weak.upgrade() else { break };
            shared.retire_idle(Instant::now());
        }
    });
}

#[cfg(test)]
mod tests;
