//! A committed removal owns its cleanup independently of the caller's connection.
use super::*;

impl Shared {
    pub(super) async fn remove_session(self: &Arc<Self>, root: &str) -> Result<Reply, ProtoError> {
        let work = {
            let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            let _publication = self.publication.lock().unwrap_or_else(|e| e.into_inner());
            if self.store.removal_state(root)? == Some(true) {
                return Ok(Reply::Ok);
            }
            let row = self
                .store
                .session(root)?
                .ok_or_else(|| not_found("Session"))?;
            if row.parent.is_some() {
                return Err(ProtoError::new(
                    ErrorCode::Invalid,
                    "Remove the root conversation, not an individual child",
                ));
            }
            if !sessions.removing.contains(root) {
                let permit = self
                    .removal_slots
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| {
                        ProtoError::new(
                            ErrorCode::Busy,
                            "Too many removals are still stopping; try again later",
                        )
                    })?;
                let members = self.store.begin_removal(root)?;
                sessions.removing.insert(root.to_owned());
                let group: Vec<_> = sessions
                    .live
                    .values()
                    .filter(|m| m.rt.root_id == root)
                    .map(|m| m.rt.clone())
                    .collect();
                for sess in &group {
                    sess.removed.cancel();
                    sess.changed.notify_waiters();
                }
                Some((group, permit, members))
            } else {
                None
            }
        };
        if let Some((group, permit, members)) = work {
            for session in &members {
                self.cli_agents.revoke_session(session);
            }
            super::faults::crash_point("removal_intent");
            let shared = self.clone();
            let root = root.to_owned();
            self.spawn(async move {
                let _permit = permit;
                shared.finish_session_removal(root, group, members).await;
            });
        }
        // A slow/uncooperative operation must not hold a client RPC indefinitely. Cleanup
        // continues; the durable receipt is safe to query by repeating the same root id.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if self.store.removal_state(root)? == Some(true) {
                return Ok(Reply::Ok);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(ProtoError::new(
                    ErrorCode::Busy,
                    "Removal is committed and still stopping session work; retry this session id to check completion",
                ));
            }
            tokio::select! {
                _ = self.stopping.cancelled() => return Err(ProtoError::new(ErrorCode::ShuttingDown, "Removal will finish on the next engine start")),
                _ = tokio::time::sleep(Duration::from_millis(10)) => {}
            }
        }
    }

    async fn finish_session_removal(
        self: Arc<Self>,
        root: String,
        group: Vec<Arc<SessionRt>>,
        members: Vec<String>,
    ) {
        for sess in &group {
            sess.questions.dispose();
            if let Some(runner) = sess.extensions.lock().await.take() {
                runner.shutdown(Duration::from_secs(2)).await;
            }
            self.jobs_remove_session(&sess.id).await;
        }
        let mut last_error = None;
        loop {
            if self.stopping.is_cancelled() {
                return;
            }
            let finished = {
                let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
                // Registry plus this cleanup job are the only permitted owners. Requests,
                // tools, runs, compactions and waiters each retain their own demand Arc.
                let idle = members.iter().all(|id| {
                    !self.cli_agents.has_turn(id)
                        && sessions
                            .retiring
                            .get(id)
                            .is_none_or(|done| done.is_cancelled())
                }) && group.iter().all(|s| {
                    let registered = sessions
                        .live
                        .get(&s.id)
                        .is_some_and(|m| Arc::ptr_eq(&m.rt, s));
                    Arc::strong_count(s) <= if registered { 2 } else { 1 }
                });
                if !idle {
                    false
                } else {
                    let _publication = self.publication.lock().unwrap_or_else(|e| e.into_inner());
                    match self.store.finish_removal(&root) {
                        Ok(affinities) => {
                            super::faults::crash_point("removal_deleted");
                            for sess in &group {
                                sessions.live.remove(&sess.id);
                                sessions.retiring.remove(&sess.id);
                            }
                            for affinity in affinities {
                                for adapter in self.adapters.adapters() {
                                    adapter.retire_session(&affinity);
                                }
                            }
                            sessions.prompt_resources.remove(&root);
                            sessions.removing.remove(&root);
                            self.publish(Event::SessionsChanged);
                            true
                        }
                        Err(error) => {
                            let message = error.to_string();
                            if last_error.as_deref() != Some(message.as_str()) {
                                log::error!("session removal {root} remains pending: {error}");
                                last_error = Some(message);
                            }
                            false
                        }
                    }
                }
            };
            if finished {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

#[cfg(test)]
mod tests;
