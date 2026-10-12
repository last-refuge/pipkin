//! Adapter behavior against a scripted fake engine: what the backend emits for replies, errors,
//! event gaps and a lost link, without a real engine.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use pipkin_core::onboarding::SignInProvider;
use pipkin_core::{
    Backend, BackendEvent, BackendRequest, Connection, ConversationId, EventKind, LifecycleEvent,
    OperationId, RequestId,
};
use pipkin_engine_protocol::*;

use super::client::testing::ChanSender;
use super::client::{FrameReceiver, Link};
use super::supervisor::HostSpec;
use super::{NativeBackend, NativeConfig};

type Handler = dyn Fn(&Call) -> Option<Result<Reply, ProtoError>> + Send + Sync;

/// A fake engine: answers calls with `handler` (no answer when it returns `None`) and lets the
/// test publish events on the live connection.
struct Fake {
    calls: Arc<Mutex<Vec<Call>>>,
    events: Arc<Mutex<Option<mpsc::Sender<ServerFrame>>>>,
    severed: Arc<Mutex<Arc<AtomicBool>>>,
    connects: Arc<AtomicU64>,
    seq: Arc<AtomicU64>,
}

fn summary(id: &str) -> SessionSummary {
    SessionSummary {
        id: id.into(),
        cwd: "/nonexistent-pipkin-test".into(),
        title: Some("Test".into()),
        created_at_ms: 1_700_000_000_000,
        updated_at_ms: 1_700_000_000_000,
        origin: Origin::Native,
        model: Some("p/m".into()),
        blocked: None,
    }
}

fn snapshot(id: &str, entries: Vec<Entry>) -> SessionSnapshot {
    SessionSnapshot {
        native_history_start: 1,
        summary: summary(id),
        revision: 1,
        entries,
        has_older: false,
        run: None,
        queue: vec![],
        usage: UsageView::default(),
        thinking: ThinkingView {
            level: "medium".into(),
            levels: vec!["off".into(), "medium".into()],
        },
        ui: UiView::default(),
        subagents: None,
        plan: PlanView::default(),
    }
}

/// The answers every test needs; `custom` is asked first.
fn default_reply(call: &Call) -> Option<Result<Reply, ProtoError>> {
    Some(Ok(match call {
        Call::Hello { .. } => Reply::Welcome(Welcome {
            protocol: PROTOCOL_VERSION,
            engine_version: "fake".into(),
            engine_revision: "fake".into(),
            schema: 1,
            epoch: 1,
            capabilities: vec![],
        }),
        Call::ListSessions => Reply::Sessions {
            sessions: vec![summary("s1")],
        },
        Call::Models { .. } => Reply::Models(ModelsView::default()),
        Call::AuthState => Reply::Auth(AuthSnapshot::default()),
        Call::OpenSession { session, .. } => Reply::Snapshot(Box::new(snapshot(session, vec![]))),
        Call::Lookup { .. } => Reply::Lookup(LookupResult::Unknown),
        Call::Shutdown => Reply::Ok,
        _ => Reply::Ok,
    }))
}

/// The client's end of a fake link, which the test can cut.
struct Severable {
    rx: mpsc::Receiver<ServerFrame>,
    cut: Arc<AtomicBool>,
}

impl FrameReceiver for Severable {
    fn recv(&mut self) -> Result<ServerFrame, String> {
        loop {
            if self.cut.load(Ordering::SeqCst) {
                return Err("the fake engine went away".into());
            }
            match self.rx.recv_timeout(Duration::from_millis(5)) {
                Ok(frame) => return Ok(frame),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err("closed".into()),
            }
        }
    }
}

impl Fake {
    fn new() -> Fake {
        Fake {
            calls: Arc::default(),
            events: Arc::default(),
            severed: Arc::default(),
            connects: Arc::default(),
            seq: Arc::new(AtomicU64::new(1)),
        }
    }

    fn host(
        &self,
        custom: impl Fn(&Call) -> Option<Option<Result<Reply, ProtoError>>> + Send + Sync + 'static,
    ) -> HostSpec {
        let calls = self.calls.clone();
        let events = self.events.clone();
        let connects = self.connects.clone();
        let severed = self.severed.clone();
        let handler: Arc<Handler> =
            Arc::new(move |call| custom(call).unwrap_or_else(|| default_reply(call)));
        HostSpec::Fake(Arc::new(move || {
            connects.fetch_add(1, Ordering::SeqCst);
            let (to_engine, from_client) = mpsc::channel::<ClientFrame>();
            let (to_client, from_engine) = mpsc::channel::<ServerFrame>();
            *events.lock().unwrap() = Some(to_client.clone());
            let cut = Arc::new(AtomicBool::new(false));
            *severed.lock().unwrap() = cut.clone();
            let calls = calls.clone();
            let handler = handler.clone();
            thread::spawn(move || {
                while let Ok(frame) = from_client.recv() {
                    calls.lock().unwrap().push(frame.call.clone());
                    if let Some(result) = handler(&frame.call)
                        && to_client
                            .send(ServerFrame::Reply {
                                id: frame.id,
                                result,
                            })
                            .is_err()
                    {
                        break;
                    }
                }
            });
            Ok(Link {
                sender: Box::new(ChanSender(to_engine)),
                receiver: Box::new(Severable {
                    rx: from_engine,
                    cut,
                }),
            })
        }))
    }

    /// Publish an event with the next sequence number (or a chosen one).
    fn publish(&self, event: Event) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst);
        self.publish_at(seq, event);
    }

    fn publish_at(&self, seq: u64, event: Event) {
        if let Some(tx) = self.events.lock().unwrap().as_ref() {
            let _ = tx.send(ServerFrame::Event {
                epoch: 1,
                seq,
                event,
            });
        }
    }

    /// Drop the live connection, as a crashed engine would.
    fn sever(&self) {
        self.events.lock().unwrap().take();
        self.severed.lock().unwrap().store(true, Ordering::SeqCst);
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    fn count(&self, pred: impl Fn(&Call) -> bool) -> usize {
        self.calls().iter().filter(|c| pred(c)).count()
    }
}

struct Run {
    backend: NativeBackend,
    events: async_channel::Receiver<BackendEvent>,
    life: Arc<Mutex<VecDeque<LifecycleEvent>>>,
    seen: Vec<BackendEvent>,
    lifecycle: Vec<LifecycleEvent>,
}

impl Run {
    fn start(host: HostSpec) -> Run {
        let mut config = NativeConfig::new(host, "/engine".into());
        config.retry_delay = Duration::from_millis(20);
        config.call_timeout = Duration::from_millis(500);
        let (tx, events) = async_channel::unbounded();
        let backend = NativeBackend::new(tx, config);
        let life = Arc::new(Mutex::new(VecDeque::new()));
        let sink = {
            let life = life.clone();
            Box::new(move |e| life.lock().unwrap().push_back(e))
        };
        backend.start(sink);
        Run {
            backend,
            events,
            life,
            seen: vec![],
            lifecycle: vec![],
        }
    }

    fn pump(&mut self) {
        while let Ok(e) = self.events.try_recv() {
            self.seen.push(e);
        }
        self.lifecycle.extend(self.life.lock().unwrap().drain(..));
    }

    fn until(&mut self, what: &str, done: impl Fn(&Run) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.pump();
            if done(self) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}: {:#?}\n{:#?}",
                self.seen,
                self.lifecycle
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn ready(&mut self) {
        self.until("ready", |r| {
            r.lifecycle
                .iter()
                .any(|e| matches!(e, LifecycleEvent::Connection(Connection::Ready)))
        });
    }

    fn conversation(&self) -> ConversationId {
        self.lifecycle
            .iter()
            .rev()
            .find_map(|e| match e {
                LifecycleEvent::Catalog(b) => b.conversations.first().map(|c| c.0),
                _ => None,
            })
            .expect("a catalog")
    }

    fn kinds(&self) -> Vec<&EventKind> {
        self.seen.iter().map(|e| &e.kind).collect()
    }

    fn has(&self, pred: impl Fn(&EventKind) -> bool) -> bool {
        self.seen.iter().any(|e| pred(&e.kind))
    }
}

fn submit(conversation: ConversationId, op: u64, key: &str) -> BackendRequest {
    BackendRequest::Submit {
        conversation,
        generation: 1,
        op: OperationId(op),
        request: RequestId(key.into()),
        text: "hello".into(),
        attachments: vec![],
        model: None,
    }
}

#[test]
fn the_catalog_models_and_sign_in_arrive_before_ready() {
    let fake = Fake::new();
    let mut run = Run::start(fake.host(|call| match call {
        Call::AuthState => Some(Some(Ok(Reply::Auth(AuthSnapshot {
            providers: vec![
                AuthProviderView {
                    id: "anthropic".into(),
                    name: "Anthropic".into(),
                    kind: CredentialKind::OAuth,
                    available: false,
                    unavailable_reason: Some("Reserved for a later integration.".into()),
                    has_credentials: false,
                },
                AuthProviderView {
                    id: "openai-codex".into(),
                    name: "ChatGPT".into(),
                    kind: CredentialKind::OAuth,
                    available: true,
                    unavailable_reason: None,
                    has_credentials: true,
                },
            ],
            ..AuthSnapshot::default()
        })))),
        Call::AuthStart { provider } if provider == "anthropic" => {
            Some(Some(Err(ProtoError::new(
                ErrorCode::Unsupported,
                "Claude subscription sign-in is reserved: not implemented.",
            ))))
        }
        _ => None,
    }));
    run.ready();
    let sign_in = run
        .lifecycle
        .iter()
        .rev()
        .find_map(|e| match e {
            LifecycleEvent::SignIn(s) if s.available => Some(s.clone()),
            _ => None,
        })
        .expect("a sign-in snapshot");
    assert_eq!(
        sign_in.unavailable_reason(SignInProvider::Claude),
        Some("Reserved for a later integration.")
    );
    assert_eq!(sign_in.existing_providers, vec![SignInProvider::ChatGpt]);
    // Asking anyway is refused plainly: a notice, never a fake success or another provider.
    run.backend
        .request(BackendRequest::StartSignIn(SignInProvider::Claude));
    run.until("the refusal is said", |r| {
        r.lifecycle
            .iter()
            .any(|e| matches!(e, LifecycleEvent::Notice(m) if m.contains("reserved")))
    });
    assert_eq!(
        fake.count(|c| matches!(c, Call::AuthStart { provider } if provider != "anthropic")),
        0
    );
    let boot = run
        .lifecycle
        .iter()
        .find_map(|e| match e {
            LifecycleEvent::Catalog(b) => Some(b.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(boot.conversations[0].2, "Test");
    run.backend.shutdown();
}

#[test]
fn refusals_are_definite_and_other_failures_are_unknown_outcomes() {
    let fake = Fake::new();
    let mut run = Run::start(fake.host(|call| match call {
        Call::Prompt { request, .. } if request == "busy" => Some(Some(Err(ProtoError::new(
            ErrorCode::Busy,
            "A run is in progress.",
        )))),
        Call::Prompt { request, .. } if request == "internal" => {
            Some(Some(Err(ProtoError::new(ErrorCode::Internal, "oops"))))
        }
        // No answer at all: a timeout after sending.
        Call::Prompt { request, .. } if request == "silent" => Some(None),
        Call::Prompt { .. } => Some(Some(Ok(Reply::Admitted {
            run: Some(1),
            entry: None,
        }))),
        _ => None,
    }));
    run.ready();
    let c = run.conversation();
    run.backend.request(submit(c, 1, "busy"));
    run.backend.request(submit(c, 2, "internal"));
    run.backend.request(submit(c, 3, "silent"));
    run.backend.request(submit(c, 4, "ok"));
    run.until("four answers", |r| r.seen.len() >= 4);
    let by_op = |op: u64| {
        run.seen
            .iter()
            .find(|e| e.op == Some(OperationId(op)))
            .map(|e| e.kind.clone())
            .unwrap()
    };
    assert_eq!(
        by_op(1),
        EventKind::Rejected {
            reason: "A run is in progress.".into()
        }
    );
    assert_eq!(by_op(2), EventKind::AckLost);
    assert_eq!(by_op(3), EventKind::AckLost);
    assert_eq!(by_op(4), EventKind::Accepted);
    // The engine's settlement of the admitted prompt reaches its operation.
    fake.publish(Event::Session {
        session: "s1".into(),
        revision: 5,
        change: SessionChange::RunSettled {
            run: 1,
            request: Some("ok".into()),
            outcome: RunOutcome::ModelError {
                detail: "429 rate limited".into(),
            },
        },
    });
    run.until("settled", |r| {
        r.seen.iter().any(|e| {
            e.op == Some(OperationId(4))
                && matches!(&e.kind, EventKind::Failed { message } if message == "429 rate limited")
        })
    });
    assert_eq!(
        fake.count(|c| matches!(c, Call::Prompt { .. })),
        4,
        "never resent"
    );
    run.backend.shutdown();
}

#[test]
fn an_event_gap_rereads_the_catalog_the_open_session_and_runs_in_doubt() {
    let fake = Fake::new();
    let mut run = Run::start(fake.host(|call| match call {
        Call::Prompt { .. } => Some(Some(Ok(Reply::Admitted {
            run: Some(1),
            entry: None,
        }))),
        Call::Lookup { request, .. } if request == "r1" => {
            Some(Some(Ok(Reply::Lookup(LookupResult::Known {
                mode: InputMode::Prompt,
                entry: None,
                run: Some(1),
                open: false,
                outcome: Some(RunOutcome::Completed),
            }))))
        }
        _ => None,
    }));
    run.ready();
    let c = run.conversation();
    run.backend.request(BackendRequest::Open {
        conversation: c,
        generation: 1,
    });
    run.until("opened", |r| {
        r.has(|k| matches!(k, EventKind::Opened { .. }))
    });
    run.backend.request(submit(c, 7, "r1"));
    run.until("accepted", |r| r.has(|k| *k == EventKind::Accepted));
    fake.publish(Event::SessionsChanged);
    let lists = fake.count(|c| matches!(c, Call::ListSessions));
    let opens = fake.count(|c| matches!(c, Call::OpenSession { .. }));
    run.until("the in-order event is applied", |_| {
        fake.count(|c| matches!(c, Call::ListSessions)) > lists
    });
    // Skip a sequence number: the settlement event was missed.
    fake.publish_at(
        fake.seq.fetch_add(5, Ordering::SeqCst) + 5,
        Event::ModelsChanged,
    );
    run.until("the run in doubt is resolved by asking", |r| {
        r.seen
            .iter()
            .any(|e| e.op == Some(OperationId(7)) && e.kind == EventKind::Completed)
    });
    assert!(
        fake.count(|c| matches!(c, Call::OpenSession { .. })) > opens,
        "re-read"
    );
    assert!(fake.count(|c| matches!(c, Call::Lookup { request, .. } if request == "r1")) >= 1);
    assert_eq!(
        fake.count(|c| matches!(c, Call::Prompt { .. })),
        1,
        "never resent"
    );
    run.backend.shutdown();
}

#[test]
fn a_lost_engine_is_reported_and_the_adapter_reconnects_and_reopens() {
    let fake = Fake::new();
    let mut run = Run::start(fake.host(|_| None));
    run.ready();
    let c = run.conversation();
    run.backend.request(BackendRequest::Open {
        conversation: c,
        generation: 1,
    });
    run.until("opened", |r| {
        r.has(|k| matches!(k, EventKind::Opened { .. }))
    });
    fake.sever();
    run.until("reconnecting, then ready again", |r| {
        r.lifecycle
            .iter()
            .any(|e| matches!(e, LifecycleEvent::Connection(Connection::Reconnecting)))
            && fake.connects.load(Ordering::SeqCst) >= 2
            && r.has(|k| matches!(k, EventKind::Synced { .. }))
    });
    // The reopened conversation is refreshed in place, not reset.
    assert_eq!(
        run.kinds()
            .iter()
            .filter(|k| matches!(k, EventKind::Opened { .. }))
            .count(),
        1
    );
    run.backend.shutdown();
}

#[test]
fn an_incompatible_engine_is_said_plainly() {
    let fake = Fake::new();
    let mut run = Run::start(fake.host(|call| match call {
        Call::Hello { .. } => Some(Some(Err(ProtoError::new(
            ErrorCode::Incompatible,
            "the engine speaks protocol 2, the client 1",
        )))),
        _ => None,
    }));
    run.until("incompatible", |r| {
        r.lifecycle.iter().any(|e| {
            matches!(e, LifecycleEvent::Connection(Connection::Incompatible(m)) if m.contains("protocol 2"))
        })
    });
    run.backend.shutdown();
}

#[test]
fn a_failed_stop_is_not_settlement_and_a_stop_settles_only_on_the_event() {
    let fake = Fake::new();
    let refuse = Arc::new(AtomicBool::new(true));
    let r2 = refuse.clone();
    let mut run = Run::start(fake.host(move |call| match call {
        Call::Prompt { .. } => Some(Some(Ok(Reply::Admitted {
            run: Some(1),
            entry: None,
        }))),
        Call::Stop { .. } if r2.load(Ordering::SeqCst) => {
            Some(Some(Err(ProtoError::new(ErrorCode::Storage, "disk full"))))
        }
        Call::Stop { .. } => Some(Some(Ok(Reply::Stopping { run: Some(1) }))),
        _ => None,
    }));
    run.ready();
    let c = run.conversation();
    run.backend.request(submit(c, 3, "r"));
    run.until("accepted", |r| r.has(|k| *k == EventKind::Accepted));
    let cancel = BackendRequest::Cancel {
        conversation: c,
        generation: 1,
        op: OperationId(3),
    };
    run.backend.request(cancel.clone());
    run.until("stop failed", |r| {
        r.has(|k| matches!(k, EventKind::StopFailed { .. }))
    });
    refuse.store(false, Ordering::SeqCst);
    run.backend.request(cancel);
    thread::sleep(Duration::from_millis(100));
    run.pump();
    assert!(
        !run.has(|k| *k == EventKind::Cancelled),
        "Stopping is not settlement"
    );
    fake.publish(Event::Session {
        session: "s1".into(),
        revision: 3,
        change: SessionChange::RunSettled {
            run: 1,
            request: Some("r".into()),
            outcome: RunOutcome::Aborted,
        },
    });
    run.until("cancelled", |r| {
        r.seen
            .iter()
            .any(|e| e.op == Some(OperationId(3)) && e.kind == EventKind::Cancelled)
    });
    run.backend.shutdown();
}

fn assistant(id: u64, partial: bool, text: &str) -> Entry {
    Entry {
        id,
        at_ms: 0,
        run: Some(1),
        retired: false,
        kind: EntryKind::Message {
            message: Message::Assistant {
                content: vec![Block::text(text)],
                api: String::new(),
                provider: String::new(),
                model: String::new(),
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 0,
                extra: Default::default(),
            },
            mode: InputMode::Prompt,
            request: None,
            partial,
        },
    }
}

#[test]
fn streaming_is_coalesced_and_the_final_text_precedes_settlement() {
    let fake = Fake::new();
    let mut run = Run::start(fake.host(|call| match call {
        Call::Prompt { .. } => Some(Some(Ok(Reply::Admitted {
            run: Some(1),
            entry: None,
        }))),
        _ => None,
    }));
    run.ready();
    let c = run.conversation();
    run.backend.request(BackendRequest::Open {
        conversation: c,
        generation: 1,
    });
    run.until("opened", |r| {
        r.has(|k| matches!(k, EventKind::Opened { .. }))
    });
    run.backend.request(submit(c, 9, "r9"));
    run.until("accepted", |r| r.has(|k| *k == EventKind::Accepted));
    let mut rev = 2;
    let mut next = |change| {
        rev += 1;
        Event::Session {
            session: "s1".into(),
            revision: rev,
            change,
        }
    };
    fake.publish(next(SessionChange::EntryAppended {
        entry: assistant(2, true, ""),
    }));
    let started = Instant::now();
    for _ in 0..300 {
        fake.publish(next(SessionChange::EntryDelta {
            id: 2,
            delta: Delta::Text {
                block: 0,
                text: "x".into(),
            },
        }));
        thread::sleep(Duration::from_micros(500));
    }
    fake.publish(next(SessionChange::EntryReplaced {
        entry: assistant(2, false, "final answer"),
    }));
    fake.publish(next(SessionChange::RunSettled {
        run: 1,
        request: Some("r9".into()),
        outcome: RunOutcome::Completed,
    }));
    run.until("settled", |r| r.has(|k| *k == EventKind::Completed));
    let elapsed = started.elapsed();
    let syncs: Vec<usize> = run
        .seen
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.kind, EventKind::Synced { .. }))
        .map(|(i, _)| i)
        .collect();
    let budget = (elapsed.as_millis() / 50) as usize + 3;
    assert!(
        syncs.len() <= budget,
        "{} transcript updates in {elapsed:?} (budget {budget})",
        syncs.len()
    );
    let done = run
        .seen
        .iter()
        .position(|e| e.kind == EventKind::Completed)
        .unwrap();
    let last_sync = *syncs
        .iter()
        .rev()
        .find(|i| **i < done)
        .expect("a sync before settlement");
    let EventKind::Synced { items } = &run.seen[last_sync].kind else {
        unreachable!()
    };
    assert!(
        items.iter().any(|i| matches!(&i.kind, pipkin_core::ItemKind::Assistant { text, .. } if text == "final answer")),
        "the final text reached the core before its settlement"
    );
    run.backend.shutdown();
}

#[test]
fn offline_requests_are_refused_without_sending_anything() {
    let mut config = NativeConfig::new(
        HostSpec::Fake(Arc::new(|| Err("no engine here".to_owned()))),
        "/engine".into(),
    );
    config.retry_delay = Duration::from_secs(5);
    let (tx, events) = async_channel::unbounded();
    let backend = NativeBackend::new(tx, config);
    let life = Arc::new(Mutex::new(VecDeque::new()));
    let sink = {
        let life = life.clone();
        Box::new(move |e| life.lock().unwrap().push_back(e))
    };
    backend.start(sink);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !life
        .lock()
        .unwrap()
        .iter()
        .any(|e| matches!(e, LifecycleEvent::Connection(Connection::Failed(m)) if m.contains("no engine here")))
    {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    backend.request(submit(ConversationId(1), 1, "k"));
    let event = events.recv_blocking().unwrap();
    assert!(matches!(event.kind, EventKind::Rejected { .. }));
    backend.shutdown();
}

fn child_entries(range: std::ops::Range<u64>) -> Vec<Entry> {
    range
        .map(|id| assistant(id, false, &format!("child step {id}")))
        .collect()
}

#[test]
fn child_history_pages_by_cursor_and_the_selected_child_refreshes_live() {
    let fake = Fake::new();
    let mut run = Run::start(fake.host(|call| match call {
        Call::OpenSession { session, .. } => {
            let mut snap = snapshot(session, vec![]);
            snap.subagents = Some(SubagentsView {
                enabled: true,
                children: vec![SubagentRow {
                    child: 3,
                    task_id: 3,
                    call_id: "call-3".into(),
                    task: "Survey the crate.".into(),
                    // The engine records a finished task as `done`.
                    status: "done".into(),
                    session: "s1:call-3".into(),
                    profile: None,
                }],
                truncated: true,
            });
            Some(Some(Ok(Reply::Snapshot(Box::new(snap)))))
        }
        Call::SubagentHistory {
            child: 3, before, ..
        } => Some(Some(match before {
            None => Ok(Reply::Page {
                native_history_start: 1,
                entries: child_entries(300..500),
                has_older: true,
            }),
            Some(300) => Ok(Reply::Page {
                native_history_start: 1,
                entries: vec![Entry {
                    id: 299,
                    at_ms: 1,
                    run: None,
                    retired: false,
                    kind: EntryKind::Message {
                        message: Message::System(SystemMessage::default()),
                        mode: InputMode::Prompt,
                        request: None,
                        partial: false,
                    },
                }],
                has_older: true,
            }),
            Some(299) => Ok(Reply::Page {
                native_history_start: 1,
                entries: child_entries(100..299),
                has_older: true,
            }),
            Some(_) => Err(ProtoError::new(ErrorCode::NotFound, "gone")),
        })),
        _ => None,
    }));
    run.ready();
    let c = run.conversation();
    run.backend.request(BackendRequest::Open {
        conversation: c,
        generation: 1,
    });
    run.until("a pageable directory", |r| {
        r.has(|k| {
            matches!(k, EventKind::SubagentDirectory { truncated: true, children, .. }
                if children.len() == 1 && children[0].status == "done")
        })
    });
    run.backend.request(BackendRequest::OpenSubagent {
        conversation: c,
        generation: 1,
        child: 3,
    });
    run.until("the newest child page", |r| {
        r.has(|k| {
            matches!(k, EventKind::SubagentPage { child: 3, items, has_older: true, live: true }
                if items.len() == 200)
        })
    });
    let before = run
        .seen
        .iter()
        .find_map(|e| match &e.kind {
            EventKind::SubagentPage { items, .. } => items.first().map(|i| i.id),
            _ => None,
        })
        .unwrap();
    run.backend.request(BackendRequest::LoadOlderSubagent {
        conversation: c,
        generation: 1,
        child: 3,
        before,
    });
    run.until("the older page, naming its cursor", |r| {
        r.has(|k| {
            matches!(k, EventKind::SubagentOlderPage { child: 3, before: b, items, has_older: true }
                if *b == before && items.len() == 199)
        })
    });
    assert!(fake.calls().contains(&Call::SubagentHistory {
        session: "s1".into(),
        child: 3,
        before: Some(300),
        limit: 200,
    }));
    assert!(
        fake.calls().contains(&Call::SubagentHistory {
            session: "s1".into(),
            child: 3,
            before: Some(299),
            limit: 200,
        }),
        "hidden system-only pages advance the raw cursor"
    );
    // A refused page is reported against the same cursor; nothing is invented.
    let oldest = run
        .seen
        .iter()
        .find_map(|e| match &e.kind {
            EventKind::SubagentOlderPage { items, .. } => items.first().map(|i| i.id),
            _ => None,
        })
        .unwrap();
    run.backend.request(BackendRequest::LoadOlderSubagent {
        conversation: c,
        generation: 1,
        child: 3,
        before: oldest,
    });
    run.until("a failed older page", |r| {
        r.has(|k| matches!(k, EventKind::SubagentOlderFailed { child: 3, before: b, .. } if *b == oldest))
    });
    // A stale attachment is refused without a call.
    let calls = fake.count(|c| matches!(c, Call::SubagentHistory { .. }));
    run.backend.request(BackendRequest::LoadOlderSubagent {
        conversation: c,
        generation: 7,
        child: 3,
        before,
    });
    run.until("the stale request is refused", |r| {
        r.seen
            .iter()
            .any(|e| e.generation == 7 && matches!(e.kind, EventKind::SubagentOlderFailed { .. }))
    });
    assert_eq!(
        fake.count(|c| matches!(c, Call::SubagentHistory { .. })),
        calls
    );
    // Commits in an unrelated session do not re-read; the selected child's do (coalesced).
    let pages = |r: &Run| {
        r.seen
            .iter()
            .filter(|e| matches!(e.kind, EventKind::SubagentPage { .. }))
            .count()
    };
    let mut rev = 10;
    for session in ["elsewhere", "s1:call-3"] {
        for id in 0..20 {
            rev += 1;
            fake.publish(Event::Session {
                session: session.into(),
                revision: rev,
                change: SessionChange::EntryAppended {
                    entry: assistant(600 + id, false, "more"),
                },
            });
        }
        if session == "elsewhere" {
            thread::sleep(Duration::from_millis(400));
            run.pump();
            assert_eq!(pages(&run), 1, "an unrelated session refreshed the child");
        }
    }
    run.until("a live refresh of the newest page", |r| pages(r) >= 2);
    thread::sleep(Duration::from_millis(400));
    run.pump();
    assert!(pages(&run) <= 3, "{} refreshes for 20 commits", pages(&run));
    run.backend.shutdown();
}
