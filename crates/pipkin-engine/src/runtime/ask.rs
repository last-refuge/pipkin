//! The session side of `ask_user_question`: a model question goes to the session's question
//! host, where it is shown by the attached presentation and settled by `AnswerQuestion` /
//! `CancelQuestion` like an extension's question.
//!
//! Availability is the engine having a connected client: question views are published to
//! every client, and a client that reopens the session receives the open question in its
//! snapshot, so a question asked while another conversation is selected is still shown when
//! the person returns. With no client at all the call fails at once with a typed
//! `unavailable` outcome instead of waiting for nobody. A question waits indefinitely (there
//! is no model-chosen timeout); Stop withdraws it. When the engine is shutting down, the
//! host's disposal must not be recorded as the person's refusal: the call stays unsettled so
//! that the restarted engine asks again (the tool is replay-safe).

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::Shared;
use crate::extensions::questions::Question;
use crate::tools::Tool;
use crate::tools::ask::{AskOutcome, AskUserQuestionTool};

impl Shared {
    pub fn ask_tool(&self) -> Arc<dyn Tool> {
        Arc::new(AskUserQuestionTool)
    }

    /// Whether any client is connected to receive and answer questions.
    pub fn has_clients(&self) -> bool {
        !self
            .clients
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
    }

    pub(super) async fn ask_user(
        self: &Arc<Self>,
        session: &str,
        question: Question,
        cancel: CancellationToken,
    ) -> AskOutcome {
        if !self.has_clients() {
            return AskOutcome::Unavailable("no presentation is connected".into());
        }
        let sess = match self.session(session) {
            Ok(sess) => sess,
            Err(error) => return AskOutcome::Unavailable(error.message),
        };
        let answer = sess
            .questions
            .ask(question, Some(Duration::ZERO), cancel.clone())
            .await;
        match answer {
            Some(answer) => AskOutcome::Answered(answer),
            None if self.stopping.is_cancelled() && !cancel.is_cancelled() => {
                // Disposal at shutdown withdrew it. Never settle: the intent stays durable and
                // the question is asked again after the restart.
                std::future::pending().await
            }
            None => AskOutcome::Unanswered,
        }
    }
}
