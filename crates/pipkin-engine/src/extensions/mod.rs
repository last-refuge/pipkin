//! Session extensions: code that asks the person questions, posts notices and status lines, and
//! drives the session's agent, without adding model tools, prompt sections or providers.
//!
//! This is the native counterpart of the reference's session facets (chord facet plugin
//! packages loaded by `pi server -e`), restricted to what those facets can reach there:
//! `UiRequestHost` (questions, notices, status), `AgentController` (prompt, steer, follow-up,
//! cancel queued, abort, compact, lookup) and `History.page`. See `docs/native-extensions.md`.
//!
//! Two ways to write one:
//! - **In process, in Rust:** implement [`Extension`]; the runtime gives it an
//!   [`ExtensionHost`] on activation.
//! - **Out of process, in any language:** a command speaking the JSON-lines contract of
//!   [`process`], wrapped by [`process::ProcessExtension`].
//!
//! The contract is versioned by [`EXTENSION_API_VERSION`]; the engine refuses an extension that
//! declares another version instead of guessing.

pub mod examples;
pub mod process;
pub mod questions;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

use std::collections::HashSet;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use futures_util::FutureExt;
use pipkin_engine_protocol::{
    Answer, Call, CancelQueuedOutcome, Entry, EntryId, ErrorCode, InputMode, LookupResult,
    NoticeLevel, ProtoError, Reply, SessionId,
};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub use crate::provider::BoxFuture;
pub use questions::{Question, QuestionHost};

/// The extension contract version (native trait and process protocol alike).
pub const EXTENSION_API_VERSION: u32 = 1;

/// Most history entries one page may carry (`HISTORY_MAX_PAGE`).
pub const HISTORY_MAX_PAGE: u32 = 200;

/// A session extension living in the engine process.
pub trait Extension: Send + Sync {
    /// Stable id, unique among one session's extensions.
    fn id(&self) -> &str;

    /// The contract version this extension was written against.
    fn api_version(&self) -> u32 {
        EXTENSION_API_VERSION
    }

    /// Start the extension for one session. The future runs as its own task (like a facet's
    /// `onActivate` work); it may finish at once or keep working (asking questions, waiting on
    /// the agent) until the session deactivates it. Dropping the future must be safe: the
    /// runner aborts it when the session closes.
    fn activate(&self, host: Arc<dyn ExtensionHost>) -> BoxFuture<'static, ()>;

    /// The session is closing. Called before the activation task is awaited (with a grace
    /// period) and then aborted.
    fn deactivate(&self) {}
}

/// A prompt, steer or follow-up the session admitted (`Reply::Admitted`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Admission {
    pub run: Option<u64>,
    pub entry: Option<u64>,
}

/// A page of committed history (`Reply::Page`): entries strictly before the cursor, oldest
/// first.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryPage {
    pub entries: Vec<Entry>,
    pub has_older: bool,
}

/// What an extension can reach: the session's questions and its agent. The runtime implements
/// it (see [`CallHost`]); [`testing::TestHost`] is a test double.
pub trait ExtensionHost: Send + Sync {
    /// The session this host belongs to.
    fn session(&self) -> &str;

    /// Ask the person (see [`QuestionHost::ask`] for every rule).
    fn ask(
        &self,
        question: Question,
        timeout: Option<Duration>,
        cancel: CancellationToken,
    ) -> BoxFuture<'static, Option<Answer>>;

    fn notify(&self, level: NoticeLevel, message: &str);

    /// Set a status line, or clear it with `None`.
    fn set_status(&self, key: &str, text: Option<&str>);

    /// Start a run; refused with [`ErrorCode::Busy`] while one is active. `request_key` makes a
    /// retry idempotent; without one, a fresh key is used.
    fn prompt(
        &self,
        text: String,
        request_key: Option<String>,
    ) -> BoxFuture<'static, Result<Admission, ProtoError>>;

    /// Steer the active run, or start one when idle.
    fn steer(
        &self,
        text: String,
        request_key: Option<String>,
    ) -> BoxFuture<'static, Result<Admission, ProtoError>>;

    /// Queue input for after the active run, or start one when idle.
    fn follow_up(
        &self,
        text: String,
        request_key: Option<String>,
    ) -> BoxFuture<'static, Result<Admission, ProtoError>>;

    /// Withdraw a queued steer or follow-up.
    fn cancel_queued(
        &self,
        entry: u64,
    ) -> BoxFuture<'static, Result<CancelQueuedOutcome, ProtoError>>;

    /// What became of a request key. Never admits anything.
    fn lookup(&self, request_key: String) -> BoxFuture<'static, Result<LookupResult, ProtoError>>;

    /// Stop the active run (and its queue). Returns the run being stopped, if any.
    fn abort(&self) -> BoxFuture<'static, Result<Option<u64>, ProtoError>>;

    /// Summarize older context now.
    fn compact(&self, instructions: Option<String>) -> BoxFuture<'static, Result<(), ProtoError>>;

    /// Committed entries strictly before `before` (the newest when `None`), at most `limit`
    /// (bounded by [`HISTORY_MAX_PAGE`]).
    fn history_page(
        &self,
        before: Option<EntryId>,
        limit: u32,
    ) -> BoxFuture<'static, Result<HistoryPage, ProtoError>>;
}

/// How [`CallHost`] reaches the session: the engine's own call dispatch.
pub type Dispatch =
    Arc<dyn Fn(Call) -> BoxFuture<'static, Result<Reply, ProtoError>> + Send + Sync>;

/// An [`ExtensionHost`] built from the session's [`QuestionHost`] and the engine's call
/// dispatch, so agent control goes through exactly the same admission path as a client's
/// `Prompt`/`Enqueue`/`Lookup`/`Stop`/`Compact`/`HistoryPage`/`CancelQueued` calls.
pub struct CallHost {
    session: SessionId,
    questions: QuestionHost,
    dispatch: Dispatch,
}

impl CallHost {
    pub fn new(session: SessionId, questions: QuestionHost, dispatch: Dispatch) -> CallHost {
        CallHost {
            session,
            questions,
            dispatch,
        }
    }

    pub fn questions(&self) -> &QuestionHost {
        &self.questions
    }

    fn call<T: Send + 'static>(
        &self,
        call: Call,
        decode: fn(Reply) -> Option<T>,
    ) -> BoxFuture<'static, Result<T, ProtoError>> {
        let reply = (self.dispatch)(call);
        Box::pin(async move {
            let reply = reply.await?;
            decode(reply).ok_or_else(|| {
                ProtoError::new(ErrorCode::Internal, "the engine sent an unexpected reply")
            })
        })
    }

    fn enqueue(
        &self,
        mode: InputMode,
        text: String,
        request_key: Option<String>,
    ) -> BoxFuture<'static, Result<Admission, ProtoError>> {
        self.call(
            Call::Enqueue {
                session: self.session.clone(),
                request: request_key_or_fresh(request_key),
                mode,
                text,
                images: Vec::new(),
            },
            admission,
        )
    }
}

fn request_key_or_fresh(key: Option<String>) -> String {
    match key {
        Some(key) if !key.is_empty() => key,
        _ => format!("ext-{}", crate::util::uuid_v4()),
    }
}

fn admission(reply: Reply) -> Option<Admission> {
    match reply {
        Reply::Admitted { run, entry } => Some(Admission { run, entry }),
        _ => None,
    }
}

impl ExtensionHost for CallHost {
    fn session(&self) -> &str {
        &self.session
    }

    fn ask(
        &self,
        question: Question,
        timeout: Option<Duration>,
        cancel: CancellationToken,
    ) -> BoxFuture<'static, Option<Answer>> {
        Box::pin(self.questions.ask(question, timeout, cancel))
    }

    fn notify(&self, level: NoticeLevel, message: &str) {
        self.questions.notify(level, message);
    }

    fn set_status(&self, key: &str, text: Option<&str>) {
        self.questions.set_status(key, text);
    }

    fn prompt(
        &self,
        text: String,
        request_key: Option<String>,
    ) -> BoxFuture<'static, Result<Admission, ProtoError>> {
        self.call(
            Call::Prompt {
                session: self.session.clone(),
                request: request_key_or_fresh(request_key),
                text,
                images: Vec::new(),
                model: None,
            },
            admission,
        )
    }

    fn steer(
        &self,
        text: String,
        request_key: Option<String>,
    ) -> BoxFuture<'static, Result<Admission, ProtoError>> {
        self.enqueue(InputMode::Steer, text, request_key)
    }

    fn follow_up(
        &self,
        text: String,
        request_key: Option<String>,
    ) -> BoxFuture<'static, Result<Admission, ProtoError>> {
        self.enqueue(InputMode::FollowUp, text, request_key)
    }

    fn cancel_queued(
        &self,
        entry: u64,
    ) -> BoxFuture<'static, Result<CancelQueuedOutcome, ProtoError>> {
        self.call(
            Call::CancelQueued {
                session: self.session.clone(),
                entry,
            },
            |reply| match reply {
                Reply::CancelQueued { outcome } => Some(outcome),
                _ => None,
            },
        )
    }

    fn lookup(&self, request_key: String) -> BoxFuture<'static, Result<LookupResult, ProtoError>> {
        self.call(
            Call::Lookup {
                session: self.session.clone(),
                request: request_key,
            },
            |reply| match reply {
                Reply::Lookup(result) => Some(result),
                _ => None,
            },
        )
    }

    fn abort(&self) -> BoxFuture<'static, Result<Option<u64>, ProtoError>> {
        self.call(
            Call::Stop {
                session: self.session.clone(),
            },
            |reply| match reply {
                Reply::Stopping { run } => Some(run),
                _ => None,
            },
        )
    }

    fn compact(&self, instructions: Option<String>) -> BoxFuture<'static, Result<(), ProtoError>> {
        self.call(
            Call::Compact {
                session: self.session.clone(),
                instructions,
            },
            |_| Some(()),
        )
    }

    fn history_page(
        &self,
        before: Option<EntryId>,
        limit: u32,
    ) -> BoxFuture<'static, Result<HistoryPage, ProtoError>> {
        self.call(
            Call::HistoryPage {
                session: self.session.clone(),
                before,
                limit: limit.clamp(1, HISTORY_MAX_PAGE),
            },
            |reply| match reply {
                Reply::Page { entries, has_older } => Some(HistoryPage { entries, has_older }),
                _ => None,
            },
        )
    }
}

struct Running {
    extension: Arc<dyn Extension>,
    task: JoinHandle<()>,
}

/// Activates a session's extensions and stops them when the session closes.
///
/// Each activation runs as its own task. A panicking extension is reported once as an error
/// notice and is not restarted. The runner does not own the session's [`QuestionHost`]: the
/// runtime disposes it after [`ExtensionRunner::shutdown`], which cancels whatever is still
/// open.
pub struct ExtensionRunner {
    host: Arc<dyn ExtensionHost>,
    running: Vec<Running>,
    ids: HashSet<String>,
}

impl ExtensionRunner {
    pub fn new(host: Arc<dyn ExtensionHost>) -> ExtensionRunner {
        ExtensionRunner {
            host,
            running: Vec::new(),
            ids: HashSet::new(),
        }
    }

    /// Ids of the extensions started so far.
    pub fn ids(&self) -> Vec<String> {
        self.running
            .iter()
            .map(|r| r.extension.id().to_string())
            .collect()
    }

    /// Activate one extension. Refused (with a warning notice) when its contract version is
    /// not [`EXTENSION_API_VERSION`] or its id is already running. Must be called inside a
    /// tokio runtime.
    pub fn start(&mut self, extension: Arc<dyn Extension>) -> Result<(), String> {
        let id = extension.id().to_string();
        let version = extension.api_version();
        if version != EXTENSION_API_VERSION {
            let message = format!(
                "Extension {id} needs extension API {version}; this engine provides {EXTENSION_API_VERSION}. It was not started."
            );
            self.host.notify(NoticeLevel::Warning, &message);
            return Err(message);
        }
        if !self.ids.insert(id.clone()) {
            let message =
                format!("Extension {id} is already running; the duplicate was not started.");
            self.host.notify(NoticeLevel::Warning, &message);
            return Err(message);
        }
        let activation = extension.activate(self.host.clone());
        let host = self.host.clone();
        let task = tokio::spawn(async move {
            if AssertUnwindSafe(activation).catch_unwind().await.is_err() {
                host.notify(
                    NoticeLevel::Error,
                    &format!("Extension {id} failed and was stopped."),
                );
            }
        });
        self.running.push(Running { extension, task });
        Ok(())
    }

    /// Deactivate every extension, give their tasks `grace` to finish, then abort the rest.
    pub async fn shutdown(self, grace: Duration) {
        for running in &self.running {
            running.extension.deactivate();
        }
        let deadline = tokio::time::Instant::now() + grace;
        for mut running in self.running {
            if tokio::time::timeout_at(deadline, &mut running.task)
                .await
                .is_err()
            {
                running.task.abort();
                let _ = running.task.await;
            }
        }
    }
}

#[cfg(test)]
mod tests;
