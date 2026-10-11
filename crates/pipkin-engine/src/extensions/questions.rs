//! The session-scoped question host: questions, notices and status lines an extension sends to
//! whichever presentation is attached.
//!
//! A port of the reference `UiRequests`/`UiRequestHost` pair
//! (`coding-agent/src/experimental/services/ui-requests{,-provider}.ts`). The rules are the
//! reference's, including the refusal texts a presentation shows verbatim:
//!
//! - ids are `q<n>` for questions and `n<n>` for notices, from one shared counter;
//! - a question waits for an answer at most `timeout` (two minutes by default, `0` waits
//!   indefinitely); on timeout an info notice `No answer to "<title>"; it was cancelled.` is
//!   posted and the question resolves `None`;
//! - at most [`UI_MAX_PENDING`] questions are open; a further question is refused at once with a
//!   warning notice and resolves `None`;
//! - at most [`UI_MAX_NOTICES`] notices are kept, oldest dropped first;
//! - the first accepted answer wins; there is no per-client ownership;
//! - cancelling (by id, by the asker's [`CancellationToken`], by dropping the asking future, or
//!   by [`QuestionHost::dispose`]) resolves the question `None`.
//!
//! Length limits are counted in UTF-16 code units, like the reference's JavaScript `slice` and
//! `length`, but a cut never splits a character.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pipkin_engine_protocol::{
    Answer, NoticeLevel, NoticeView, QuestionKind, QuestionOption, QuestionView, UiView,
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

/// How long a question waits by default (`UI_DEFAULT_TIMEOUT_MS`).
pub const UI_DEFAULT_TIMEOUT: Duration = Duration::from_millis(120_000);
/// Most questions open at once (`UI_MAX_PENDING`).
pub const UI_MAX_PENDING: usize = 20;
/// Most notices kept (`UI_MAX_NOTICES`).
pub const UI_MAX_NOTICES: usize = 20;
/// Longest accepted `input` answer (`UI_MAX_INPUT_CHARS`).
pub const UI_MAX_INPUT_CHARS: usize = 10_000;
/// Titles are cut to this length (`UI_MAX_TITLE_CHARS`).
pub const UI_MAX_TITLE_CHARS: usize = 300;
/// Messages under a question title are cut to this length.
pub const UI_MAX_MESSAGE_CHARS: usize = 2_000;
/// Notice texts are cut to this length.
pub const UI_MAX_NOTICE_CHARS: usize = 500;
/// Status keys are cut to this length.
pub const UI_MAX_STATUS_KEY_CHARS: usize = 80;
/// Status texts are cut to this length.
pub const UI_MAX_STATUS_TEXT_CHARS: usize = 300;
/// How much of a title a notice quotes.
const NOTICE_TITLE_CHARS: usize = 80;

pub const REFUSE_CONFIRM: &str = "A confirmation needs yes or no.";
pub const REFUSE_NOT_TEXT: &str = "An answer must be text.";
pub const REFUSE_NOT_A_CHOICE: &str = "That is not one of the choices.";
pub const REFUSE_NOT_OPEN: &str = "That question is no longer open.";

/// Receives every new [`UiView`] after a change (to publish `SessionChange::UiChanged`).
///
/// It is called while the host's state is locked, so changes arrive in order; it must not call
/// back into the same [`QuestionHost`] (forward the view to a channel instead).
pub type UiChangeFn = Arc<dyn Fn(UiView) + Send + Sync>;

/// The clock used for `deadline_ms` (Unix milliseconds). Injectable for tests.
pub type ClockFn = Arc<dyn Fn() -> i64 + Send + Sync>;

/// A question an extension asks the person.
#[derive(Clone, Debug, PartialEq)]
pub struct Question {
    pub kind: QuestionKind,
    pub title: String,
    pub message: Option<String>,
    /// The choices of a `select`.
    pub options: Vec<QuestionOption>,
    pub placeholder: Option<String>,
    pub default_value: Option<String>,
}

impl Question {
    pub fn select(title: impl Into<String>, options: Vec<QuestionOption>) -> Question {
        Question {
            kind: QuestionKind::Select,
            title: title.into(),
            message: None,
            options,
            placeholder: None,
            default_value: None,
        }
    }

    pub fn confirm(title: impl Into<String>) -> Question {
        Question {
            kind: QuestionKind::Confirm,
            title: title.into(),
            message: None,
            options: Vec::new(),
            placeholder: None,
            default_value: None,
        }
    }

    pub fn input(title: impl Into<String>) -> Question {
        Question {
            kind: QuestionKind::Input,
            title: title.into(),
            message: None,
            options: Vec::new(),
            placeholder: None,
            default_value: None,
        }
    }

    pub fn with_message(mut self, message: impl Into<String>) -> Question {
        self.message = Some(message.into());
        self
    }

    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Question {
        self.placeholder = Some(placeholder.into());
        self
    }

    pub fn with_default(mut self, value: impl Into<String>) -> Question {
        self.default_value = Some(value.into());
        self
    }
}

/// A choice of a `select` question.
pub fn option(value: &str, label: &str, description: Option<&str>) -> QuestionOption {
    QuestionOption {
        value: value.to_string(),
        label: label.to_string(),
        description: description.map(str::to_string),
    }
}

/// The first `max` UTF-16 code units of `text`, never splitting a character.
pub fn clip_utf16(text: &str, max: usize) -> &str {
    let mut units = 0;
    for (at, ch) in text.char_indices() {
        units += ch.len_utf16();
        if units > max {
            return &text[..at];
        }
    }
    text
}

struct Pending {
    view: QuestionView,
    settle: oneshot::Sender<Option<Answer>>,
}

#[derive(Default)]
struct State {
    counter: u64,
    pending: Vec<Pending>,
    status: Vec<(String, String)>,
    notices: VecDeque<NoticeView>,
    disposed: bool,
}

impl State {
    fn view(&self) -> UiView {
        UiView {
            questions: self.pending.iter().map(|p| p.view.clone()).collect(),
            status: self.status.clone(),
            notices: self.notices.iter().cloned().collect(),
        }
    }

    fn notify(&mut self, level: NoticeLevel, message: &str) {
        self.counter += 1;
        self.notices.push_back(NoticeView {
            id: format!("n{}", self.counter),
            level,
            message: clip_utf16(message, UI_MAX_NOTICE_CHARS).to_string(),
        });
        while self.notices.len() > UI_MAX_NOTICES {
            self.notices.pop_front();
        }
    }

    fn take(&mut self, id: &str) -> Option<Pending> {
        let at = self.pending.iter().position(|p| p.view.id == id)?;
        Some(self.pending.remove(at))
    }
}

struct Shared {
    state: Mutex<State>,
    on_change: Option<UiChangeFn>,
    clock: ClockFn,
}

impl Shared {
    /// Run `edit` under the lock and publish the new view when it reports a change.
    fn change<R>(&self, edit: impl FnOnce(&mut State) -> (R, bool)) -> R {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let (result, changed) = edit(&mut state);
        if changed && let Some(on_change) = &self.on_change {
            on_change(state.view());
        }
        result
    }
}

/// The question host of one session. Cheap to clone; clones share state.
#[derive(Clone)]
pub struct QuestionHost {
    shared: Arc<Shared>,
}

impl Default for QuestionHost {
    fn default() -> Self {
        QuestionHost::new(None)
    }
}

impl std::fmt::Debug for QuestionHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuestionHost").finish_non_exhaustive()
    }
}

impl QuestionHost {
    pub fn new(on_change: Option<UiChangeFn>) -> QuestionHost {
        QuestionHost::with_clock(on_change, Arc::new(crate::util::now_ms))
    }

    pub fn with_clock(on_change: Option<UiChangeFn>, clock: ClockFn) -> QuestionHost {
        QuestionHost {
            shared: Arc::new(Shared {
                state: Mutex::new(State::default()),
                on_change,
                clock,
            }),
        }
    }

    /// The current questions, status lines and notices.
    pub fn view(&self) -> UiView {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .view()
    }

    /// Ask a question and wait for its answer.
    ///
    /// `timeout`: `None` uses [`UI_DEFAULT_TIMEOUT`]; `Some(Duration::ZERO)` waits indefinitely.
    /// Resolves `None` when the question is refused (too many open, a `select` without
    /// choices, a disposed host), times out, is cancelled (by id, by `cancel`, or by dropping
    /// this future), or the host is disposed. An answer always matches the question's kind:
    /// `Choice` for `select`, `Confirm` for `confirm`, `Text` for `input`.
    pub fn ask(
        &self,
        question: Question,
        timeout: Option<Duration>,
        cancel: CancellationToken,
    ) -> impl Future<Output = Option<Answer>> + Send + 'static {
        // Registered now, like the reference: the question is visible before the first poll.
        let timeout = timeout.unwrap_or(UI_DEFAULT_TIMEOUT);
        let expires = tokio::time::Instant::now().checked_add(timeout);
        let shared = self.shared.clone();
        let opened = open(&shared, question, timeout).map(|(id, settled)| {
            let guard = AskGuard {
                shared: shared.clone(),
                id: id.clone(),
                armed: true,
            };
            (id, settled, guard)
        });
        async move {
            let (id, mut settled, mut guard) = opened?;
            let expiry = async {
                match expires {
                    Some(at) if !timeout.is_zero() => tokio::time::sleep_until(at).await,
                    // Zero waits indefinitely; so does a timeout beyond the clock's range.
                    _ => std::future::pending::<()>().await,
                }
            };
            let timed_out = tokio::select! {
                biased;
                answer = &mut settled => {
                    guard.armed = false;
                    return answer.ok().flatten();
                }
                _ = cancel.cancelled() => false,
                _ = expiry => true,
            };
            guard.armed = false;
            let closed = shared.change(|state| match state.take(&id) {
                Some(entry) => {
                    if timed_out {
                        let title = clip_utf16(&entry.view.title, NOTICE_TITLE_CHARS);
                        let notice = format!("No answer to \"{title}\"; it was cancelled.");
                        state.notify(NoticeLevel::Info, &notice);
                    }
                    (true, true)
                }
                None => (false, false),
            });
            if closed {
                None
            } else {
                // A responder settled it between the wakeup and the lock: its answer wins.
                settled.await.ok().flatten()
            }
        }
    }

    /// Post a notice.
    pub fn notify(&self, level: NoticeLevel, message: &str) {
        self.shared.change(|state| {
            state.notify(level, message);
            ((), true)
        });
    }

    /// Set a status line, or clear it with `None`.
    pub fn set_status(&self, key: &str, text: Option<&str>) {
        self.shared.change(|state| match text {
            // The reference deletes by the key as given (it is only cut when set).
            None => match state.status.iter().position(|(k, _)| k == key) {
                Some(at) => {
                    state.status.remove(at);
                    ((), true)
                }
                None => ((), false),
            },
            Some(text) => {
                let key = clip_utf16(key, UI_MAX_STATUS_KEY_CHARS).to_string();
                let text = clip_utf16(text, UI_MAX_STATUS_TEXT_CHARS).to_string();
                match state.status.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, current)) if *current == text => ((), false),
                    Some((_, current)) => {
                        *current = text;
                        ((), true)
                    }
                    None => {
                        state.status.push((key, text));
                        ((), true)
                    }
                }
            }
        });
    }

    /// Answer an open question. A refusal leaves the question open and says why.
    pub fn respond(&self, id: &str, answer: Answer) -> Result<(), String> {
        self.shared.change(|state| {
            let Some(at) = state.pending.iter().position(|p| p.view.id == id) else {
                return (Err(REFUSE_NOT_OPEN.to_string()), false);
            };
            let view = &state.pending[at].view;
            let answer = match (view.kind, answer) {
                (QuestionKind::Confirm, Answer::Confirm { value }) => Answer::Confirm { value },
                (QuestionKind::Confirm, _) => return (Err(REFUSE_CONFIRM.to_string()), false),
                (_, Answer::Confirm { .. }) => return (Err(REFUSE_NOT_TEXT.to_string()), false),
                (QuestionKind::Select, Answer::Choice { value } | Answer::Text { value }) => {
                    if !view.options.iter().any(|o| o.value == value) {
                        return (Err(REFUSE_NOT_A_CHOICE.to_string()), false);
                    }
                    Answer::Choice { value }
                }
                (QuestionKind::Input, Answer::Choice { value } | Answer::Text { value }) => {
                    if value.encode_utf16().count() > UI_MAX_INPUT_CHARS {
                        let reason =
                            format!("An answer can be at most {UI_MAX_INPUT_CHARS} characters.");
                        return (Err(reason), false);
                    }
                    Answer::Text { value }
                }
            };
            let entry = state.pending.remove(at);
            let _ = entry.settle.send(Some(answer));
            (Ok(()), true)
        })
    }

    /// Withdraw an open question; it resolves `None`. False when it is not open.
    pub fn cancel(&self, id: &str) -> bool {
        self.shared.change(|state| match state.take(id) {
            Some(entry) => {
                let _ = entry.settle.send(None);
                (true, true)
            }
            None => (false, false),
        })
    }

    /// Remove every notice (the presentation's "dismiss notices").
    pub fn dismiss_notices(&self) {
        self.shared.change(|state| {
            let changed = !state.notices.is_empty();
            state.notices.clear();
            ((), changed)
        });
    }

    /// Cancel every open question; post `reason` as an info notice unless it is empty.
    pub fn cancel_all(&self, reason: &str) {
        self.shared.change(|state| {
            let changed = !state.pending.is_empty() || !reason.is_empty();
            for entry in state.pending.drain(..) {
                let _ = entry.settle.send(None);
            }
            if !reason.is_empty() {
                state.notify(NoticeLevel::Info, reason);
            }
            ((), changed)
        });
    }

    /// The session is closing: cancel every open question and refuse new ones.
    pub fn dispose(&self) {
        self.shared.change(|state| {
            state.disposed = true;
            let changed = !state.pending.is_empty();
            for entry in state.pending.drain(..) {
                let _ = entry.settle.send(None);
            }
            ((), changed)
        });
    }
}

/// Register a question; `None` when it is refused.
fn open(
    shared: &Arc<Shared>,
    question: Question,
    timeout: Duration,
) -> Option<(String, oneshot::Receiver<Option<Answer>>)> {
    if question.kind == QuestionKind::Select && question.options.is_empty() {
        return None;
    }
    shared.change(|state| {
        if state.disposed {
            return (None, false);
        }
        if state.pending.len() >= UI_MAX_PENDING {
            let title = clip_utf16(&question.title, NOTICE_TITLE_CHARS);
            let notice = format!("Too many open questions; \"{title}\" was not shown.");
            state.notify(NoticeLevel::Warning, &notice);
            return (None, true);
        }
        state.counter += 1;
        let id = format!("q{}", state.counter);
        let created = (shared.clock)();
        let deadline_ms = (!timeout.is_zero()).then(|| {
            created.saturating_add(i64::try_from(timeout.as_millis()).unwrap_or(i64::MAX))
        });
        let options = if question.kind == QuestionKind::Select {
            question.options
        } else {
            Vec::new()
        };
        let view = QuestionView {
            id: id.clone(),
            kind: question.kind,
            title: clip_utf16(&question.title, UI_MAX_TITLE_CHARS).to_string(),
            message: question
                .message
                .map(|m| clip_utf16(&m, UI_MAX_MESSAGE_CHARS).to_string()),
            options,
            placeholder: question.placeholder,
            default_value: question.default_value,
            deadline_ms,
        };
        let (settle, settled) = oneshot::channel();
        state.pending.push(Pending { view, settle });
        (Some((id, settled)), true)
    })
}

/// Withdraws the question when the asking future is dropped before it settled.
struct AskGuard {
    shared: Arc<Shared>,
    id: String,
    armed: bool,
}

impl Drop for AskGuard {
    fn drop(&mut self) {
        if self.armed {
            let id = std::mem::take(&mut self.id);
            self.shared.change(|state| ((), state.take(&id).is_some()));
        }
    }
}

#[cfg(test)]
mod tests;
