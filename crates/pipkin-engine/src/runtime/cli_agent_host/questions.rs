//! Exact live question response grants, not permission to run an API/native inference loop.
use super::{Host, Shared};
use crate::{
    extensions::questions::{Question, UI_MAX_PENDING},
    runtime::session::SessionRt,
    tools::ask::AskOutcome,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub(in crate::runtime) struct Questions(Mutex<HashMap<String, Arc<Host>>>);
struct Registration {
    session: Arc<SessionRt>,
    host: Arc<Host>,
    id: Arc<Mutex<Option<String>>>,
}
impl Drop for Registration {
    fn drop(&mut self) {
        if let Some(id) = self.id.lock().unwrap().as_ref() {
            let mut grants = self.session.cli_questions.0.lock().unwrap();
            if grants.get(id).is_some_and(|h| Arc::ptr_eq(h, &self.host)) {
                grants.remove(id);
            }
        }
    }
}
impl Shared {
    pub(in crate::runtime) fn cli_question_allowed(&self, session: &str, question: &str) -> bool {
        // Release sessions/map locks before publication -> authority -> store validation.
        // Do not open or reconstruct a session/host from a saved source/call identifier.
        let sess = self
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .live
            .get(session)
            .map(|s| s.rt.clone());
        let Some(sess) = sess else {
            return false;
        };
        let host = sess.cli_questions.0.lock().unwrap().get(question).cloned();
        host.is_some_and(|h| h.check().is_ok())
    }
}
impl Host {
    pub(super) async fn ask_scoped(
        &self,
        call: &str,
        question: Question,
        cancel: CancellationToken,
    ) -> AskOutcome {
        if let Err(reason) = self.named("ask_user_question", Some(call)) {
            return AskOutcome::Unavailable(reason);
        }
        self.ask_authorized(question, cancel).await
    }

    // Also used by the exact native mcp_call's scoped form presenter, never a session id
    // reconstructed by the global MCP handler. Both routes retain the pending-call proof.
    pub(super) async fn ask_authorized(
        &self,
        mut question: Question,
        cancel: CancellationToken,
    ) -> AskOutcome {
        if let Err(reason) = self.check() {
            return AskOutcome::Unavailable(reason);
        }
        if !self.shared.has_clients() {
            return AskOutcome::Unavailable("no presentation is connected".into());
        }
        let sess = match self.shared.session(&self.authority.scope().session) {
            Ok(sess) => sess,
            Err(e) => return AskOutcome::Unavailable(e.message),
        };
        question.source = Some(format!("call:{}", self.call));
        let registration = Registration {
            session: sess.clone(),
            host: Arc::new(self.clone()),
            id: Arc::default(),
        };
        let slot = registration.id.clone();
        let host = registration.host.clone();
        let session = sess.clone();
        // No publication/registry/storage lock is held while the question callback publishes
        // its volatile UI view. The exact-id grant is installed before that view is delivered.
        let waiting = sess.questions.ask_registered(
            question,
            Some(Duration::ZERO),
            self.cancel.clone(),
            move |id| {
                let mut slot = slot.lock().unwrap();
                let mut grants = session.cli_questions.0.lock().unwrap();
                if grants.len() >= UI_MAX_PENDING || grants.contains_key(id) {
                    return false;
                }
                grants.insert(id.into(), host);
                *slot = Some(id.into());
                true
            },
        );
        // Close the registration race without granting a late answer execution authority.
        // Dropping waiting withdraws exactly its question; dropping registration revokes RPC.
        if self.check().is_err() || cancel.is_cancelled() {
            drop(waiting);
            return AskOutcome::Unanswered;
        }
        let answer = tokio::select! {biased;
            _=self.cancel.cancelled()=>None,
            _=cancel.cancelled()=>None,
            _=self.shared.stopping.cancelled()=>None,
            _=tokio::time::sleep_until(self.deadline)=>None,
            answer=waiting=>answer,
        };
        drop(registration);
        if self.check().is_err() || cancel.is_cancelled() {
            return AskOutcome::Unanswered;
        }
        match answer {
            Some(answer) => AskOutcome::Answered(answer),
            None => AskOutcome::Unanswered,
        }
        // Unlike the API question host, shutdown/restart never re-asks this CLI call.
    }
}
