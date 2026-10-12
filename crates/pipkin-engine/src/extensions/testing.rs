//! Test doubles for extensions: a host with a real [`QuestionHost`] and a recording agent, and
//! a responder that answers questions as they appear.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pipkin_engine_protocol::{
    Answer, Call, CancelQueuedOutcome, InputMode, LookupResult, NoticeLevel, ProtoError,
    QuestionView, Reply, UiView,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{
    Admission, BoxFuture, CallHost, Dispatch, ExtensionHost, HistoryPage, Question, QuestionHost,
};

#[derive(Default)]
struct Agent {
    calls: Vec<Call>,
    next: u64,
    keys: HashMap<String, (InputMode, u64)>,
    failures: VecDeque<ProtoError>,
}

impl Agent {
    fn reply(&mut self, call: Call) -> Result<Reply, ProtoError> {
        self.calls.push(call.clone());
        if let Some(error) = self.failures.pop_front() {
            return Err(error);
        }
        Ok(match call {
            Call::Prompt { request, .. } => {
                self.next += 1;
                self.keys.insert(request, (InputMode::Prompt, self.next));
                Reply::Admitted {
                    run: Some(self.next),
                    entry: None,
                }
            }
            Call::Enqueue { request, mode, .. } => {
                self.next += 1;
                self.keys.insert(request, (mode, self.next));
                Reply::Admitted {
                    run: None,
                    entry: Some(self.next),
                }
            }
            Call::Lookup { request, .. } => Reply::Lookup(match self.keys.get(&request) {
                None => LookupResult::Unknown,
                Some((mode, n)) => LookupResult::Known {
                    mode: *mode,
                    entry: (*mode != InputMode::Prompt).then_some(*n),
                    run: (*mode == InputMode::Prompt).then_some(*n),
                    open: true,
                    outcome: None,
                },
            }),
            Call::CancelQueued { .. } => Reply::CancelQueued {
                outcome: CancelQueuedOutcome::NotFound,
            },
            Call::Stop { .. } => Reply::Stopping { run: None },
            Call::HistoryPage { .. } => Reply::Page {
                native_history_start: 1,
                entries: Vec::new(),
                has_older: false,
            },
            _ => Reply::Ok,
        })
    }
}

/// An [`ExtensionHost`] for tests: real questions, a recording agent that admits everything
/// (prompts get run ids, steers and follow-ups get entry ids, starting at 1), and a channel of
/// every [`UiView`] change.
pub struct TestHost {
    inner: CallHost,
    agent: Arc<Mutex<Agent>>,
}

impl TestHost {
    pub fn new(session: &str) -> (Arc<TestHost>, mpsc::UnboundedReceiver<UiView>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let questions = QuestionHost::new(Some(Arc::new(move |view| {
            let _ = tx.send(view);
        })));
        let agent = Arc::new(Mutex::new(Agent::default()));
        let dispatch_agent = agent.clone();
        let dispatch: Dispatch = Arc::new(move |call| {
            let reply = dispatch_agent
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .reply(call);
            Box::pin(async move { reply })
        });
        let host = TestHost {
            inner: CallHost::new(session.to_string(), questions, dispatch),
            agent,
        };
        (Arc::new(host), rx)
    }

    pub fn questions(&self) -> &QuestionHost {
        self.inner.questions()
    }

    /// Every agent call so far, in order.
    pub fn calls(&self) -> Vec<Call> {
        self.agent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .calls
            .clone()
    }

    /// The next agent call fails with `error`.
    pub fn fail_next(&self, error: ProtoError) {
        self.agent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .failures
            .push_back(error);
    }

    /// The notice texts currently kept, oldest first.
    pub fn notices(&self) -> Vec<String> {
        self.questions()
            .view()
            .notices
            .into_iter()
            .map(|n| n.message)
            .collect()
    }
}

impl ExtensionHost for TestHost {
    fn session(&self) -> &str {
        self.inner.session()
    }

    fn ask(
        &self,
        question: Question,
        timeout: Option<Duration>,
        cancel: CancellationToken,
    ) -> BoxFuture<'static, Option<Answer>> {
        self.inner.ask(question, timeout, cancel)
    }

    fn notify(&self, level: NoticeLevel, message: &str) {
        self.inner.notify(level, message)
    }

    fn set_status(&self, key: &str, text: Option<&str>) {
        self.inner.set_status(key, text)
    }

    fn prompt(
        &self,
        text: String,
        request_key: Option<String>,
    ) -> BoxFuture<'static, Result<Admission, ProtoError>> {
        self.inner.prompt(text, request_key)
    }

    fn steer(
        &self,
        text: String,
        request_key: Option<String>,
    ) -> BoxFuture<'static, Result<Admission, ProtoError>> {
        self.inner.steer(text, request_key)
    }

    fn follow_up(
        &self,
        text: String,
        request_key: Option<String>,
    ) -> BoxFuture<'static, Result<Admission, ProtoError>> {
        self.inner.follow_up(text, request_key)
    }

    fn cancel_queued(
        &self,
        entry: u64,
    ) -> BoxFuture<'static, Result<CancelQueuedOutcome, ProtoError>> {
        self.inner.cancel_queued(entry)
    }

    fn lookup(&self, request_key: String) -> BoxFuture<'static, Result<LookupResult, ProtoError>> {
        self.inner.lookup(request_key)
    }

    fn wait_for_prompt(
        &self,
        operation_id: String,
    ) -> BoxFuture<'static, Result<pipkin_engine_protocol::PromptResult, ProtoError>> {
        self.inner.wait_for_prompt(operation_id)
    }

    fn abort(&self) -> BoxFuture<'static, Result<Option<u64>, ProtoError>> {
        self.inner.abort()
    }

    fn compact(&self, instructions: Option<String>) -> BoxFuture<'static, Result<(), ProtoError>> {
        self.inner.compact(instructions)
    }

    fn history_page(
        &self,
        before: Option<u64>,
        limit: u32,
    ) -> BoxFuture<'static, Result<HistoryPage, ProtoError>> {
        self.inner.history_page(before, limit)
    }
}

/// Answer each new question with `answer(question)` until the channel closes or `stop`
/// is cancelled. Returns the questions seen, in order.
pub async fn respond_until(
    questions: QuestionHost,
    mut views: mpsc::UnboundedReceiver<UiView>,
    stop: CancellationToken,
    answer: impl Fn(&QuestionView) -> Option<Answer>,
) -> Vec<QuestionView> {
    let mut seen = HashSet::new();
    let mut asked = Vec::new();
    loop {
        let view = tokio::select! {
            _ = stop.cancelled() => break,
            view = views.recv() => match view { Some(view) => view, None => break },
        };
        for question in view.questions {
            if seen.insert(question.id.clone()) {
                if let Some(reply) = answer(&question) {
                    questions
                        .respond(&question.id, reply)
                        .expect("the answer is accepted");
                }
                asked.push(question);
            }
        }
    }
    asked
}
