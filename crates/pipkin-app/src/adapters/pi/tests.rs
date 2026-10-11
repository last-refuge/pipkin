//! The adapter against the mock Pi server, over real Unix sockets with the real trust checks.

use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pi_client::delta::{Op, Seg};
use pi_client::testing::{ConnHandle, MockPi, SERVER_ID, install_session_management};
use pipkin_core::{
    Backend, BackendEvent, BackendRequest, Bootstrap, ChangesState, Connection, ConversationId,
    EventKind, ItemKind, LifecycleEvent, OperationId, RequestId,
    onboarding::{SignInAnswer, SignInProvider},
};
use serde_json::{Value, json};

use super::{Msg, PiBackend, PiConfig, Workspace};

const WAIT: Duration = Duration::from_secs(10);

fn chmod(path: &std::path::Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for: {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn user_entry(id: u64, text: &str) -> Value {
    json!({ "id": id, "conversationId": 1, "kind": "pi.message",
        "model": [{ "role": "user", "content": text, "timestamp": 1_700_000_000_000i64 }] })
}

fn view(texts: &[&str]) -> Value {
    let entries: Vec<Value> = texts
        .iter()
        .enumerate()
        .map(|(i, t)| user_entry(i as u64 + 1, t))
        .collect();
    json!({ "conversation": { "id": 1 }, "entries": entries, "docs": {} })
}

fn key(s: &str) -> Seg {
    Seg::Key(s.into())
}

struct Env {
    _dir: tempfile::TempDir,
    dir: PathBuf,
    pi: MockPi,
    conns: Arc<Mutex<Vec<ConnHandle>>>,
    backend: PiBackend,
    events: async_channel::Receiver<BackendEvent>,
    lifecycle: Arc<Mutex<Vec<LifecycleEvent>>>,
}

fn sessions_state(sessions: &[(&str, i64)]) -> Value {
    json!({ "revision": 1, "sessions": sessions.iter().map(|(id, at)| json!({
        "serverId": SERVER_ID, "sessionId": id, "createdAt": at,
    })).collect::<Vec<_>>() })
}

/// A mock whose attach handler swaps the transcript to the attached session's view, so each
/// attachment really does show different content.
type Views = Arc<Mutex<Vec<(String, Value)>>>;

fn mock(sessions: &[(&str, i64)], views: Vec<(&str, Value)>) -> MockPi {
    mock_with_views(sessions, views).0
}

/// Like `mock`, but the per-session views stay editable, so a test can change what a session
/// shows between attachments (for example while the connection is down).
fn mock_with_views(sessions: &[(&str, i64)], views: Vec<(&str, Value)>) -> (MockPi, Views) {
    let pi = MockPi::default();
    pi.add_service("pi.session-directory", &[], Some(sessions_state(sessions)));
    pi.add_service(
        "pi.transcript",
        &[],
        Some(json!({ "entries": [], "docs": {} })),
    );
    pi.add_service(
        "pi.models",
        &["select", "refresh"],
        Some(json!({
            "catalog": { "revision": 1, "availableModels": [
                { "provider": "anthropic", "modelId": "sonnet", "name": "Sonnet", "reasoning": true } ] },
            "configuration": { "model": { "provider": "anthropic", "modelId": "sonnet" }, "thinkingLevel": "off" },
        })),
    );
    install_session_management(&pi);
    let views: Views = Arc::new(Mutex::new(
        views.into_iter().map(|(k, v)| (k.to_owned(), v)).collect(),
    ));
    let handler_views = views.clone();
    pi.set_handler("pi.session-management", "attach", move |pi, conn, call| {
        let session_id = call.args[0].as_str().unwrap().to_owned();
        let attachment_id = format!("att-{}", pi.requests().len());
        conn.set_attachment(Some(pi_client::protocol::SessionTarget {
            server_id: SERVER_ID.into(),
            session_id: session_id.clone(),
            attachment_id,
        }));
        let view = handler_views
            .lock()
            .unwrap()
            .iter()
            .find(|(k, _)| *k == session_id)
            .map(|(_, v)| v.clone());
        if let Some(v) = view {
            pi.publish("pi.transcript", vec![Op::Replace(v)]);
        }
        Ok(None)
    });
    (pi, views)
}

fn listen(pi: &MockPi, dir: &std::path::Path, id: &str, conns: &Arc<Mutex<Vec<ConnHandle>>>) {
    let socket = dir.join(format!("{id}.sock"));
    let listener = UnixListener::bind(&socket).unwrap();
    chmod(&socket, 0o600);
    let (pi, conns) = (pi.clone(), conns.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let handle = pi.accept(stream);
            conns.lock().unwrap().push(handle);
        }
    });
}

fn start(pi: MockPi, serve: bool) -> Env {
    start_with_mode(pi, serve, 0o700)
}

fn start_with_mode(pi: MockPi, serve: bool, dir_mode: u32) -> Env {
    start_configured(pi, serve, dir_mode, |_| {})
}

fn start_configured(
    pi: MockPi,
    serve: bool,
    dir_mode: u32,
    tweak: impl FnOnce(&mut PiConfig),
) -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("server");
    std::fs::create_dir(&dir).unwrap();
    chmod(&dir, dir_mode);
    let conns = Arc::new(Mutex::new(Vec::new()));
    if serve {
        listen(&pi, &dir, SERVER_ID, &conns);
    }
    let (tx, events) = async_channel::unbounded();
    let mut config = PiConfig::new(dir.clone());
    config.retry_delay = Duration::from_millis(50);
    tweak(&mut config);
    let backend = PiBackend::new(tx, config);
    let lifecycle = Arc::new(Mutex::new(Vec::new()));
    let sink = {
        let lifecycle = lifecycle.clone();
        Box::new(move |e| lifecycle.lock().unwrap().push(e))
    };
    backend.start(sink);
    Env {
        _dir: tmp,
        dir,
        pi,
        conns,
        backend,
        events,
        lifecycle,
    }
}

impl Env {
    fn connections(&self) -> Vec<Connection> {
        self.lifecycle
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| {
                if let LifecycleEvent::Connection(c) = e {
                    Some(c.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    fn catalogs(&self) -> Vec<Bootstrap> {
        self.lifecycle
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| {
                if let LifecycleEvent::Catalog(b) = e {
                    Some(b.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    fn wait_ready(&self) {
        wait_until("ready", || self.connections().contains(&Connection::Ready));
    }

    /// The live connection. Discovery probes connect first, so the newest one is the session.
    fn live_connection(&self) -> ConnHandle {
        self.conns
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("a connection")
    }

    fn conversation(&self, index: usize) -> ConversationId {
        self.wait_ready();
        self.catalogs().last().unwrap().conversations[index].0
    }

    /// Content tests skip incidental scan-state reports, which are checked explicitly below.
    fn next_event(&self) -> BackendEvent {
        loop {
            let event = self.next_event_with_scan_state();
            if !matches!(event.kind, EventKind::ChangesScanState(_)) {
                return event;
            }
        }
    }

    /// Engine-state reports accompany every refresh and are checked by their own tests.
    fn next_event_with_scan_state(&self) -> BackendEvent {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Ok(e) = self.events.try_recv() {
                if matches!(e.kind, EventKind::EngineState { .. }) {
                    continue;
                }
                return e;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for a backend event"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn no_event_within(&self, ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
        while let Ok(e) = self.events.try_recv() {
            if !matches!(
                e.kind,
                EventKind::EngineState { .. } | EventKind::ChangesScanState(_)
            ) {
                panic!("unexpected event: {e:?}");
            }
        }
    }

    fn open(&self, conversation: ConversationId, generation: u64) {
        self.backend.request(BackendRequest::Open {
            conversation,
            generation,
        });
    }
}

fn texts(items: &[pipkin_core::TranscriptItem]) -> Vec<String> {
    items
        .iter()
        .filter_map(|i| match &i.kind {
            ItemKind::User { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn connects_then_publishes_the_catalog_and_ready() {
    let env = start(
        mock(
            &[("aaaaaaaa-1", 1_700_000_100), ("bbbbbbbb-2", 1_700_000_200)],
            vec![],
        ),
        true,
    );
    env.wait_ready();
    let connections = env.connections();
    assert_eq!(connections.first(), Some(&Connection::Connecting));
    assert_eq!(connections.last(), Some(&Connection::Ready));
    let catalog = env.catalogs().pop().unwrap();
    assert_eq!(catalog.conversations.len(), 2);
    assert_eq!(catalog.projects.len(), 1);
    assert!(
        catalog
            .conversations
            .iter()
            .any(|c| c.2.starts_with("Session aaaaaaaa"))
    );
    // The catalogue precedes readiness: the UI never shows Ready with nothing to list.
    let order: Vec<&str> = env
        .lifecycle
        .lock()
        .unwrap()
        .iter()
        .map(|e| match e {
            LifecycleEvent::Catalog(_) => "catalog",
            LifecycleEvent::Connection(Connection::Ready) => "ready",
            LifecycleEvent::Connection(_) => "state",
            LifecycleEvent::ModelSelected(_)
            | LifecycleEvent::SignIn(_)
            | LifecycleEvent::SetupSaved(_)
            | LifecycleEvent::Notice(_)
            | LifecycleEvent::StorageIssue(_) => "other",
        })
        .collect();
    let catalog_at = order.iter().position(|e| *e == "catalog").unwrap();
    let ready_at = order.iter().position(|e| *e == "ready").unwrap();
    assert!(catalog_at < ready_at);
}

#[test]
fn subscription_oauth_calls_are_scoped_to_the_server_without_exposing_answers_in_events() {
    let pi = mock(&[], vec![]);
    pi.add_service(
        "pi.provider-auth",
        &["start", "reuse", "answer", "cancel", "remove"],
        Some(json!({
            "credentialsKnown": true, "hasExistingCredentials": false, "existingProviders": [],
            "attempt": null, "provider": null, "status": "idle", "message": "", "url": null,
            "deviceCode": null, "challenge": null,
        })),
    );
    pi.set_handler("pi.provider-auth", "start", |pi, _, call| {
        assert_eq!(call.args[0], "anthropic");
        pi.publish(
            "pi.provider-auth",
            vec![Op::Set(vec![key("attempt")], json!("try-1"))],
        );
        Ok(Some(json!("try-1")))
    });
    pi.set_handler("pi.provider-auth", "reuse", |_, _, call| {
        assert_eq!(call.args[0], "openai-codex");
        Ok(Some(json!("reused-attempt")))
    });
    pi.set_handler("pi.provider-auth", "answer", |_, _, call| {
        assert_eq!(call.args[0], "try-1");
        assert_eq!(call.args[1], "challenge-1");
        assert_eq!(call.args[2], "synthetic-code");
        Ok(None)
    });
    pi.set_handler("pi.provider-auth", "remove", |_, _, call| {
        assert_eq!(call.args, vec![json!("anthropic")]);
        Ok(None)
    });
    pi.set_handler("pi.provider-auth", "cancel", |_, _, _| Ok(None));
    let env = start(pi, true);
    env.wait_ready();
    wait_until("OAuth service subscribed", || {
        env.lifecycle.lock().unwrap().iter().any(|event| {
            matches!(event,
        LifecycleEvent::SignIn(s) if s.available && s.credentials_known)
        })
    });
    env.backend
        .request(BackendRequest::StartSignIn(SignInProvider::Claude));
    wait_until("start RPC", || {
        env.pi
            .requests()
            .iter()
            .any(|(_, r)| r.service_id == "pi.provider-auth" && r.member == "start")
    });
    env.backend
        .request(BackendRequest::ReuseSignIn(SignInProvider::ChatGpt));
    wait_until("reuse RPC", || {
        env.pi
            .requests()
            .iter()
            .any(|(_, r)| r.service_id == "pi.provider-auth" && r.member == "reuse")
    });
    env.backend.request(BackendRequest::AnswerSignIn {
        attempt: "try-1".into(),
        challenge: "challenge-1".into(),
        response: SignInAnswer::new("synthetic-code".into()),
    });
    wait_until("answer RPC", || {
        env.pi
            .requests()
            .iter()
            .any(|(_, r)| r.service_id == "pi.provider-auth" && r.member == "answer")
    });
    env.backend.request(BackendRequest::CancelSignIn {
        attempt: "try-1".into(),
    });
    wait_until("cancel RPC", || {
        env.pi
            .requests()
            .iter()
            .any(|(_, r)| r.service_id == "pi.provider-auth" && r.member == "cancel")
    });
    env.backend
        .request(BackendRequest::RemoveSignIn(SignInProvider::Claude));
    wait_until("remove RPC", || {
        env.pi
            .requests()
            .iter()
            .any(|(_, r)| r.service_id == "pi.provider-auth" && r.member == "remove")
    });
    assert!(!format!("{:?}", env.lifecycle.lock().unwrap().as_slice()).contains("synthetic-code"));
    env.backend.shutdown();
}

#[test]
fn opening_a_session_shows_its_actual_transcript_and_models() {
    let env = start(
        mock(
            &[("s-one", 100)],
            vec![("s-one", view(&["fix the failing test", "and run clippy"]))],
        ),
        true,
    );
    let conv = env.conversation(0);
    env.open(conv, 7);
    let event = env.next_event();
    assert_eq!((event.conversation, event.generation), (conv, 7));
    let EventKind::Opened {
        items, has_older, ..
    } = event.kind
    else {
        panic!("{event:?}")
    };
    assert_eq!(texts(&items), ["fix the failing test", "and run clippy"]);
    assert!(!has_older);
    // The session's model catalogue reaches the core, configured model first.
    wait_until("models", || {
        env.catalogs().last().is_some_and(|c| !c.models.is_empty())
    });
    assert_eq!(
        env.catalogs().last().unwrap().models[0].id,
        "anthropic/sonnet"
    );
}

#[test]
fn usage_ledger_is_reported_on_attach_and_when_it_changes() {
    let mut initial = view(&["first"]);
    initial["docs"]["pi.usage"] = json!({ "models": { "p/m": {
        "input": 10, "output": 2, "cacheRead": 3, "cacheWrite": 0,
        "totalTokens": 15, "cost": { "total": 0.02 }
    } }, "tools": {} });
    let env = start(mock(&[("s-one", 100)], vec![("s-one", initial)]), true);
    let conv = env.conversation(0);
    env.open(conv, 1);
    assert!(matches!(env.next_event().kind, EventKind::Opened { .. }));
    let usage = env.next_event();
    let EventKind::UsageSynced(usage) = usage.kind else {
        panic!("{usage:?}")
    };
    assert_eq!(
        (usage.session_id.as_str(), usage.total().total_tokens),
        ("s-one", 15)
    );
    env.pi.publish(
        "pi.transcript",
        vec![Op::Set(
            vec![
                key("docs"),
                key("pi.usage"),
                key("models"),
                key("p/m"),
                key("output"),
            ],
            json!(5),
        )],
    );
    assert!(matches!(env.next_event().kind, EventKind::Synced { .. }));
    let changed = env.next_event();
    let EventKind::UsageSynced(usage) = changed.kind else {
        panic!("{changed:?}")
    };
    assert_eq!(usage.models[0].1.output, 5);
}

#[test]
fn live_changes_arrive_as_synced_items() {
    let env = start(
        mock(&[("s-one", 100)], vec![("s-one", view(&["first"]))]),
        true,
    );
    let conv = env.conversation(0);
    env.open(conv, 1);
    assert!(matches!(env.next_event().kind, EventKind::Opened { .. }));
    env.pi.publish(
        "pi.transcript",
        vec![Op::Splice {
            path: vec![key("entries")],
            index: 1,
            remove: 0,
            items: vec![user_entry(2, "second")],
        }],
    );
    let event = env.next_event();
    assert_eq!((event.conversation, event.generation), (conv, 1));
    let EventKind::Synced { items } = event.kind else {
        panic!("{event:?}")
    };
    assert_eq!(texts(&items), ["first", "second"]);
}

#[test]
fn a_burst_of_updates_is_coalesced_and_ends_consistent() {
    let env = start(
        mock(&[("s-one", 100)], vec![("s-one", view(&["first"]))]),
        true,
    );
    let conv = env.conversation(0);
    env.open(conv, 1);
    env.next_event();
    for i in 0..60u64 {
        env.pi.publish(
            "pi.transcript",
            vec![Op::Splice {
                path: vec![key("entries")],
                index: (i + 1) as usize,
                remove: 0,
                items: vec![user_entry(i + 2, &format!("m{i}"))],
            }],
        );
    }
    // Whatever the batching, the final synced state has every message.
    let mut last = 0;
    wait_until("converged", || {
        while let Ok(e) = env.events.try_recv() {
            if let EventKind::Synced { items } = e.kind {
                last = items.len();
            }
        }
        last == 61
    });
}

#[test]
fn switching_sessions_never_lets_the_old_attachment_update_the_screen() {
    let env = start(
        mock(
            &[("s-a", 100), ("s-b", 200)],
            vec![("s-a", view(&["from A"])), ("s-b", view(&["from B"]))],
        ),
        true,
    );
    let (a, b) = {
        env.wait_ready();
        let c = env.catalogs().pop().unwrap().conversations;
        let find = |t: &str| c.iter().find(|x| x.2.contains(t)).unwrap().0;
        (find("s-a"), find("s-b"))
    };
    env.open(a, 1);
    let EventKind::Opened { items, .. } = env.next_event().kind else {
        panic!()
    };
    assert_eq!(texts(&items), ["from A"]);
    let conn = env.live_connection();
    let old_ids = conn.subscription_ids("pi.transcript");
    assert_eq!(old_ids.len(), 1);

    env.open(b, 1);
    let event = env.next_event();
    assert_eq!(event.conversation, b);
    let EventKind::Opened { items, .. } = event.kind else {
        panic!("{event:?}")
    };
    assert_eq!(texts(&items), ["from B"]);

    // The adapter released A's subscription before attaching B, so only B's remains.
    let live_ids = conn.subscription_ids("pi.transcript");
    assert_eq!(live_ids.len(), 1);
    assert_ne!(live_ids, old_ids);

    // A frame for A's subscription that was already in flight arrives now. It must not surface.
    conn.send_update(
        &old_ids[0],
        json!({ "type": "state", "member": "state", "sequence": 99, "ops": [["s", ["entries"], []]] }),
    );
    env.no_event_within(300);

    // B keeps updating normally afterwards.
    env.pi.publish(
        "pi.transcript",
        vec![Op::Splice {
            path: vec![key("entries")],
            index: 1,
            remove: 0,
            items: vec![user_entry(9, "B again")],
        }],
    );
    let event = env.next_event();
    assert_eq!(event.conversation, b);
    let EventKind::Synced { items } = event.kind else {
        panic!("{event:?}")
    };
    assert_eq!(texts(&items), ["from B", "B again"]);
}

#[test]
fn opening_an_unknown_conversation_fails_visibly() {
    let env = start(mock(&[("s", 1)], vec![]), true);
    env.wait_ready();
    env.open(ConversationId(424_242), 3);
    let event = env.next_event();
    assert_eq!(event.generation, 3);
    assert!(
        matches!(event.kind, EventKind::OpenFailed { ref message } if message.contains("no longer exists"))
    );
}

#[test]
fn offline_until_a_server_appears() {
    let pi = mock(&[("s", 1)], vec![]);
    let env = start(pi.clone(), false);
    wait_until("offline", || {
        env.connections()
            .iter()
            .any(|c| matches!(c, Connection::Offline(m) if m.contains("no Pi server")))
    });
    // A request while offline is answered, not left hanging.
    env.open(ConversationId(1), 1);
    assert!(matches!(
        env.next_event().kind,
        EventKind::OpenFailed { .. }
    ));
    // The server starts later; the adapter finds it on its own.
    listen(&pi, &env.dir, SERVER_ID, &env.conns);
    env.wait_ready();
    assert_eq!(env.catalogs().last().unwrap().conversations.len(), 1);
}

#[test]
fn an_engine_without_the_required_services_is_incompatible() {
    let pi = MockPi::default();
    pi.add_service("pi.session-directory", &[], Some(sessions_state(&[])));
    // No pi.session-management.
    let env = start(pi, true);
    wait_until("incompatible", || {
        env.connections().iter().any(
            |c| matches!(c, Connection::Incompatible(m) if m.contains("pi.session-management")),
        )
    });
    assert!(!env.connections().contains(&Connection::Ready));
}

#[test]
fn an_untrusted_directory_is_reported_as_such_not_as_no_server() {
    // The server is running, but its directory is open to other users.
    let env = start_with_mode(mock(&[("s", 1)], vec![]), true, 0o755);
    wait_until("failed", || {
        env.connections()
            .iter()
            .any(|c| matches!(c, Connection::Failed(m) if m.contains("accessible to other users")))
    });
    assert!(!env.connections().contains(&Connection::Ready));
    assert!(
        !env.connections()
            .iter()
            .any(|c| matches!(c, Connection::Offline(m) if m.contains("no Pi server")))
    );
}

#[test]
fn losing_the_connection_reconnects_and_refreshes_the_open_conversation() {
    let (pi, views) = mock_with_views(&[("s-a", 100)], vec![("s-a", view(&["before"]))]);
    let env = start(pi, true);
    let conv = env.conversation(0);
    env.open(conv, 5);
    assert!(matches!(env.next_event().kind, EventKind::Opened { .. }));
    let requests_before = env.pi.requests().len();

    // The engine moves on while the connection is down; the next attach shows the new state.
    views.lock().unwrap()[0].1 = view(&["before", "while away"]);
    env.live_connection().close();
    wait_until("reconnecting", || {
        env.connections().contains(&Connection::Reconnecting)
    });
    wait_until("ready again", || {
        env.connections()
            .iter()
            .filter(|c| **c == Connection::Ready)
            .count()
            >= 2
    });

    // The same conversation, same generation, refreshed in place (Synced, not a fresh open).
    let event = env.next_event();
    assert_eq!((event.conversation, event.generation), (conv, 5));
    let EventKind::Synced { items } = event.kind else {
        panic!("{event:?}")
    };
    assert_eq!(texts(&items), ["before", "while away"]);
    // Reconnecting re-attached to read, but replayed nothing that mutates.
    let mutating = env
        .pi
        .requests()
        .iter()
        .skip(requests_before)
        .filter(|(_, c)| c.service_id == "pi.agent-controller")
        .count();
    assert_eq!(mutating, 0);
}

#[test]
fn directory_changes_update_the_catalog() {
    let env = start(mock(&[("s-a", 100)], vec![]), true);
    env.wait_ready();
    let before = env.catalogs().len();
    env.pi.publish(
        "pi.session-directory",
        vec![Op::Splice {
            path: vec![key("sessions")],
            index: 1,
            remove: 0,
            items: vec![json!({ "serverId": SERVER_ID, "sessionId": "s-new", "createdAt": 300 })],
        }],
    );
    wait_until("new session", || {
        env.catalogs()
            .iter()
            .skip(before)
            .any(|c| c.conversations.len() == 2)
    });
}

#[test]
fn malformed_directory_rows_do_not_break_the_catalog() {
    let state = json!({ "revision": 1, "sessions": [ { "sessionId": "ok", "createdAt": 5 }, 7, { "createdAt": 1 }, { "sessionId": "" } ] });
    let pi = mock(&[], vec![]);
    pi.publish("pi.session-directory", vec![Op::Replace(state)]);
    let env = start(pi, true);
    env.wait_ready();
    assert_eq!(env.catalogs().last().unwrap().conversations.len(), 1);
}

#[test]
fn discovery_prefers_the_default_server_when_several_run() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("server");
    std::fs::create_dir(&dir).unwrap();
    chmod(&dir, 0o700);
    let other_id = "00000000-0000-4000-8000-0000000000dd";
    let conns = Arc::new(Mutex::new(Vec::new()));
    let a = mock(&[("from-default", 1)], vec![]);
    let b = MockPi::new(other_id);
    b.add_service(
        "pi.session-directory",
        &[],
        Some(sessions_state(&[("from-other", 1)])),
    );
    install_session_management(&b);
    listen(&a, &dir, SERVER_ID, &conns);
    listen(&b, &dir, other_id, &conns);

    let run = |default: Option<&str>| {
        if let Some(d) = default {
            std::fs::write(dir.join("default-server-id"), d).unwrap();
        }
        let (tx, _events) = async_channel::unbounded();
        let mut config = PiConfig::new(dir.clone());
        config.retry_delay = Duration::from_millis(50);
        let backend = PiBackend::new(tx, config);
        let lifecycle = Arc::new(Mutex::new(Vec::new()));
        let sink = {
            let l = lifecycle.clone();
            Box::new(move |e| l.lock().unwrap().push(e))
        };
        backend.start(sink);
        (backend, lifecycle)
    };

    // Ambiguous without a default: refuse to guess.
    let (backend, lifecycle) = run(None);
    wait_until("ambiguity", || {
        lifecycle.lock().unwrap().iter().any(|e| matches!(e, LifecycleEvent::Connection(Connection::Failed(m)) if m.contains("--pi-server-id")))
    });
    backend.shutdown();

    // With the profile's default id present, that server is chosen.
    let (backend, lifecycle) = run(Some(SERVER_ID));
    wait_until("default chosen", || {
        lifecycle.lock().unwrap().iter().any(|e| matches!(e, LifecycleEvent::Catalog(b) if b.conversations.iter().any(|c| c.2.contains("from-def"))))
    });
    backend.shutdown();
}

#[test]
fn shutdown_returns_promptly_and_later_requests_are_harmless() {
    let env = start(mock(&[("s", 1)], vec![]), true);
    env.wait_ready();
    let started = Instant::now();
    env.backend.shutdown();
    assert!(started.elapsed() < Duration::from_secs(2));
    env.backend.request(BackendRequest::Open {
        conversation: ConversationId(1),
        generation: 1,
    });
    env.no_event_within(100);
}

#[test]
fn attach_retries_a_transient_internal_error_then_succeeds() {
    let pi = mock(&[("s", 1)], vec![("s", view(&["hello"]))]);
    let failures = Arc::new(Mutex::new(2u32));
    let attempts = Arc::new(Mutex::new(0u32));
    let (f, a) = (failures.clone(), attempts.clone());
    // Replace the mock's attach: fail twice with the server's generic error, then behave.
    pi.set_handler("pi.session-management", "attach", move |pi, conn, call| {
        *a.lock().unwrap() += 1;
        let mut left = f.lock().unwrap();
        if *left > 0 {
            *left -= 1;
            return Err(pi_client::protocol::ProtocolError {
                code: "internal_error".into(),
                message: "Internal server error".into(),
            });
        }
        let session_id = call.args[0].as_str().unwrap().to_owned();
        conn.set_attachment(Some(pi_client::protocol::SessionTarget {
            server_id: SERVER_ID.into(),
            session_id,
            attachment_id: "att-retry".into(),
        }));
        pi.publish("pi.transcript", vec![Op::Replace(view(&["hello"]))]);
        Ok(None)
    });
    let env = start(pi, true);
    let conv = env.conversation(0);
    env.open(conv, 1);
    let event = env.next_event();
    let EventKind::Opened { items, .. } = event.kind else {
        panic!("{event:?}")
    };
    assert_eq!(texts(&items), ["hello"]);
    assert_eq!(*attempts.lock().unwrap(), 3, "two failures, then success");
}

#[test]
fn attach_gives_up_after_bounded_retries_and_other_errors_are_final() {
    // A persistent internal_error is retried a bounded number of times, then reported.
    let pi = mock(&[("s", 1)], vec![]);
    let attempts = Arc::new(Mutex::new(0u32));
    let a = attempts.clone();
    pi.set_handler("pi.session-management", "attach", move |_, _, _| {
        *a.lock().unwrap() += 1;
        Err(pi_client::protocol::ProtocolError {
            code: "internal_error".into(),
            message: "x".into(),
        })
    });
    let env = start(pi, true);
    let conv = env.conversation(0);
    env.open(conv, 1);
    let event = env.next_event();
    assert!(
        matches!(event.kind, EventKind::OpenFailed { ref message } if message.contains("Could not attach"))
    );
    assert_eq!(*attempts.lock().unwrap(), 5, "one try plus four retries");

    // A different error code is not retried.
    let pi = mock(&[("s", 1)], vec![]);
    let attempts = Arc::new(Mutex::new(0u32));
    let a = attempts.clone();
    pi.set_handler("pi.session-management", "attach", move |_, _, _| {
        *a.lock().unwrap() += 1;
        Err(pi_client::protocol::ProtocolError {
            code: "session_not_found".into(),
            message: "gone".into(),
        })
    });
    let env = start(pi, true);
    let conv = env.conversation(0);
    env.open(conv, 1);
    assert!(matches!(
        env.next_event().kind,
        EventKind::OpenFailed { .. }
    ));
    assert_eq!(*attempts.lock().unwrap(), 1);
}

#[test]
fn a_server_that_refuses_our_protocol_is_reported_as_incompatible_not_absent() {
    let pi = mock(&[("s", 1)], vec![]);
    pi.reject_handshake("version", "unsupported protocol version");
    let env = start(pi, true);
    wait_until("incompatible", || {
        env.connections()
            .iter()
            .any(|c| matches!(c, Connection::Incompatible(m) if m.contains("version")))
    });
    assert!(!env.connections().contains(&Connection::Ready));
    assert!(
        !env.connections()
            .iter()
            .any(|c| matches!(c, Connection::Offline(m) if m.contains("no Pi server")))
    );
}

// ------------------------------------------------------------------ agent controller flows

use pipkin_core::FileChange;

/// What the mock AgentController, Models and session management were asked, and what the engine
/// "reports" for each request key. Tests edit `statuses` to model the engine finishing a run.
#[derive(Default)]
struct Agent {
    prompts: Vec<Value>,
    aborts: usize,
    selects: Vec<Value>,
    thinking_selections: Vec<Value>,
    creates: Vec<Value>,
    /// requestId -> the JSON `lookup` returns.
    statuses: std::collections::HashMap<String, Value>,
    next_op: u64,
    /// How the next prompt is answered.
    prompt_mode: PromptMode,
    /// Every steer and follow-up received: (member, argument).
    queued: Vec<(String, Value)>,
    /// How the next steer or follow-up is answered (then back to `Accept`).
    queue_answer: QueueAnswer,
    cancelled: Vec<String>,
    /// What `cancelQueued` answers; `cancelled` when empty.
    cancel_outcome: &'static str,
    refreshes: usize,
}

#[derive(Default, Clone)]
enum QueueAnswer {
    #[default]
    Accept,
    Reject(&'static str),
    /// The request reaches the engine, which admits it, but the reply never arrives.
    DropAfterAdmit,
    /// The request never reaches the engine.
    DropBefore,
}

fn queued_status(entry: &str) -> Value {
    json!({ "found": true, "operationId": entry, "status": "queued", "reason": null, "detail": null })
}

#[derive(Default, Clone)]
enum PromptMode {
    #[default]
    Accept,
    Reject(&'static str),
    InternalError,
    DropConnection,
}

fn placed(op: &str) -> Value {
    json!({ "found": true, "operationId": op, "status": "placed", "reason": null, "detail": null })
}

fn settled(op: &str, status: &str, reason: Option<&str>, detail: Option<&str>) -> Value {
    json!({ "found": true, "operationId": op, "status": status, "reason": reason, "detail": detail })
}

/// A mock engine with an AgentController. Each prompt is admitted as `placed`.
fn agent_mock(
    sessions: &[(&str, i64)],
    views: Vec<(&str, Value)>,
) -> (MockPi, Arc<Mutex<Agent>>, Views) {
    let (pi, views) = mock_with_views(sessions, views);
    let agent = Arc::new(Mutex::new(Agent::default()));
    let a = agent.clone();
    pi.set_handler("pi.agent-controller", "prompt", move |_, conn, call| {
        let mut agent = a.lock().unwrap();
        let request = call.args[0].clone();
        agent.prompts.push(request.clone());
        match agent.prompt_mode.clone() {
            PromptMode::Reject(why) => Ok(Some(json!({
                "accepted": false, "operationId": null,
                "error": { "code": "busy", "message": why },
            }))),
            PromptMode::InternalError => Err(pi_client::protocol::ProtocolError {
                code: "internal_error".into(),
                message: "Internal server error".into(),
            }),
            PromptMode::DropConnection => {
                conn.close();
                Ok(None)
            }
            PromptMode::Accept => {
                agent.next_op += 1;
                let op = agent.next_op.to_string();
                if let Some(key) = request["requestId"].as_str() {
                    // A repeated key returns the original submission, as the real engine does.
                    let existing = agent.statuses.get(key).cloned();
                    let operation = existing
                        .as_ref()
                        .and_then(|e| e["operationId"].as_str().map(str::to_owned))
                        .unwrap_or_else(|| op.clone());
                    agent
                        .statuses
                        .entry(key.to_owned())
                        .or_insert_with(|| placed(&op));
                    return Ok(Some(
                        json!({ "accepted": true, "operationId": operation, "error": null }),
                    ));
                }
                Ok(Some(
                    json!({ "accepted": true, "operationId": op, "error": null }),
                ))
            }
        }
    });
    let a = agent.clone();
    pi.set_handler("pi.agent-controller", "lookup", move |_, _, call| {
        let key = call.args[0].as_str().unwrap_or("");
        Ok(Some(
            a.lock()
                .unwrap()
                .statuses
                .get(key)
                .cloned()
                .unwrap_or(json!({ "found": false })),
        ))
    });
    for member in ["steer", "followUp"] {
        let a = agent.clone();
        pi.set_handler("pi.agent-controller", member, move |_, conn, call| {
            let mut agent = a.lock().unwrap();
            let answer = std::mem::take(&mut agent.queue_answer);
            if matches!(answer, QueueAnswer::DropBefore) {
                conn.close();
                return Ok(None);
            }
            let request = call.args[0].clone();
            agent.queued.push((member.to_owned(), request.clone()));
            if let QueueAnswer::Reject(why) = answer {
                return Ok(Some(json!({
                    "accepted": false, "entryId": null,
                    "error": { "code": "operation_failed", "message": why },
                })));
            }
            // A repeated key returns the original submission, as the real engine does.
            let key = request["requestId"].as_str().unwrap_or("").to_owned();
            let entry = match agent.statuses.get(&key) {
                Some(existing) => existing["operationId"].as_str().unwrap_or("0").to_owned(),
                None => {
                    agent.next_op += 1;
                    let entry = agent.next_op.to_string();
                    agent.statuses.insert(key, queued_status(&entry));
                    entry
                }
            };
            if matches!(answer, QueueAnswer::DropAfterAdmit) {
                conn.close();
                return Ok(None);
            }
            Ok(Some(
                json!({ "accepted": true, "entryId": entry, "error": null }),
            ))
        });
    }
    let a = agent.clone();
    pi.set_handler("pi.agent-controller", "cancelQueued", move |_, _, call| {
        let mut agent = a.lock().unwrap();
        let entry = call.args[0].as_str().unwrap_or("").to_owned();
        agent.cancelled.push(entry.clone());
        let outcome = match agent.cancel_outcome {
            "" => "cancelled",
            other => other,
        };
        if outcome == "cancelled" {
            for status in agent.statuses.values_mut() {
                if status["operationId"] == entry.as_str() {
                    *status = settled(&entry, "unanswered", Some("aborted"), None);
                }
            }
        }
        Ok(Some(json!({ "outcome": outcome })))
    });
    let a = agent.clone();
    pi.set_handler("pi.models", "refresh", move |_, _, _| {
        a.lock().unwrap().refreshes += 1;
        Ok(None)
    });
    let a = agent.clone();
    pi.set_handler("pi.agent-controller", "abort", move |_, _, _| {
        let mut agent = a.lock().unwrap();
        agent.aborts += 1;
        for status in agent.statuses.values_mut() {
            if status["status"] == "placed" {
                let op = status["operationId"].as_str().unwrap_or("").to_owned();
                *status = settled(&op, "unanswered", Some("aborted"), None);
            }
        }
        Ok(None)
    });
    pi.set_handler("pi.models", "getThinkingLevels", |_, _, _| {
        Ok(Some(json!(["off", "low", "medium", "high"])))
    });
    let a = agent.clone();
    pi.set_handler("pi.models", "selectThinking", move |pi, _, call| {
        a.lock()
            .unwrap()
            .thinking_selections
            .push(call.args[0].clone());
        pi.publish(
            "pi.models",
            vec![Op::Set(
                vec![key("configuration"), key("thinkingLevel")],
                call.args[0].clone(),
            )],
        );
        Ok(None)
    });
    let a = agent.clone();
    pi.set_handler("pi.models", "select", move |pi, _, call| {
        a.lock().unwrap().selects.push(call.args[0].clone());
        let model = call.args[0].clone();
        pi.publish(
            "pi.models",
            vec![Op::Set(vec![key("configuration"), key("model")], model)],
        );
        Ok(None)
    });
    let a = agent.clone();
    pi.set_handler("pi.session-management", "create", move |pi, _, call| {
        let options = call.args[0].clone();
        let mut agent = a.lock().unwrap();
        let id = options["id"].as_str().unwrap_or("generated").to_owned();
        if agent.creates.iter().any(|c| c["id"] == options["id"]) {
            return Err(pi_client::protocol::ProtocolError {
                code: "internal_error".into(),
                message: format!("Session {id} already exists"),
            });
        }
        agent.creates.push(options.clone());
        let row = json!({ "serverId": SERVER_ID, "sessionId": id, "createdAt": 4_000_000_000_000i64, "cwd": options["cwd"] });
        pi.publish(
            "pi.session-directory",
            vec![Op::Splice { path: vec![key("sessions")], index: 99, remove: 0, items: vec![row.clone()] }],
        );
        Ok(Some(row))
    });
    (pi, agent, views)
}

fn submit(env: &Env, conv: ConversationId, generation: u64, op: u64, request: &str, text: &str) {
    env.backend.request(BackendRequest::Submit {
        conversation: conv,
        generation,
        op: OperationId(op),
        request: RequestId(request.into()),
        text: text.into(),
        attachments: vec![],
        model: None,
    });
}

impl Env {
    /// The next event for `op`, skipping unrelated transcript and workspace updates.
    fn next_for(&self, op: u64) -> BackendEvent {
        loop {
            let event = self.next_event();
            if event.op == Some(OperationId(op)) {
                return event;
            }
        }
    }

    fn open_first(&self) -> ConversationId {
        let conv = self.conversation(0);
        self.open(conv, 1);
        assert!(matches!(self.next_event().kind, EventKind::Opened { .. }));
        conv
    }

    fn wait_notice(&self, needle: &str) {
        wait_until(needle, || {
            self.lifecycle
                .lock()
                .unwrap()
                .iter()
                .any(|e| matches!(e, LifecycleEvent::Notice(m) if m.contains(needle)))
        });
    }
}

#[test]
fn a_prompt_is_sent_with_its_request_key_and_settles_when_the_engine_says_so() {
    let (pi, agent, _) = agent_mock(&[("s", 1)], vec![("s", view(&[]))]);
    let env = start(pi, true);
    let conv = env.open_first();
    submit(&env, conv, 1, 7, "req-1", "fix the bug");
    assert_eq!(env.next_for(7).kind, EventKind::Accepted);
    {
        let agent = agent.lock().unwrap();
        assert_eq!(agent.prompts.len(), 1);
        assert_eq!(agent.prompts[0]["message"], "fix the bug");
        assert_eq!(
            agent.prompts[0]["requestId"], "req-1",
            "the journaled key reaches the engine"
        );
        assert!(agent.prompts[0]["images"].is_null());
    }
    // Still running: nothing settles yet.
    env.no_event_within(600);
    // The engine finishes the run; the adapter notices by asking, not by being told.
    agent
        .lock()
        .unwrap()
        .statuses
        .insert("req-1".into(), settled("1", "done", None, None));
    assert_eq!(env.next_for(7).kind, EventKind::Completed);
}

#[test]
fn a_refused_prompt_is_a_definite_rejection_with_the_engine_s_reason() {
    let (pi, agent, _) = agent_mock(&[("s", 1)], vec![("s", view(&[]))]);
    let env = start(pi, true);
    let conv = env.open_first();
    agent.lock().unwrap().prompt_mode = PromptMode::Reject("Pi is busy with another run");
    submit(&env, conv, 1, 3, "req-busy", "again");
    let EventKind::Rejected { reason } = env.next_for(3).kind else {
        panic!()
    };
    assert!(reason.contains("busy"), "{reason}");
    // A refusal leaves nothing to settle.
    env.no_event_within(600);
}

#[test]
fn an_internal_error_after_sending_is_an_unknown_outcome_not_a_rejection() {
    let (pi, agent, _) = agent_mock(&[("s", 1)], vec![("s", view(&[]))]);
    let env = start(pi, true);
    let conv = env.open_first();
    agent.lock().unwrap().prompt_mode = PromptMode::InternalError;
    submit(&env, conv, 1, 4, "req-ie", "maybe");
    assert_eq!(env.next_for(4).kind, EventKind::AckLost);
}

#[test]
fn losing_the_connection_before_the_acknowledgment_is_an_unknown_outcome() {
    let (pi, agent, _) = agent_mock(&[("s", 1)], vec![("s", view(&[]))]);
    let env = start(pi, true);
    let conv = env.open_first();
    agent.lock().unwrap().prompt_mode = PromptMode::DropConnection;
    submit(&env, conv, 1, 5, "req-drop", "lost");
    assert_eq!(env.next_for(5).kind, EventKind::AckLost);
}

#[test]
fn a_prompt_for_a_session_that_is_not_open_is_refused_and_nothing_is_sent() {
    let (pi, agent, _) = agent_mock(&[("s", 1)], vec![("s", view(&[]))]);
    let env = start(pi, true);
    let conv = env.conversation(0); // listed, never opened
    submit(&env, conv, 1, 6, "req-closed", "hi");
    let EventKind::Rejected { reason } = env.next_for(6).kind else {
        panic!()
    };
    assert!(reason.contains("not open"), "{reason}");
    assert!(agent.lock().unwrap().prompts.is_empty());
}

#[test]
fn an_attachment_that_cannot_be_sent_is_refused_and_nothing_is_sent() {
    let (pi, agent, _) = agent_mock(&[("s", 1)], vec![("s", view(&[]))]);
    let env = start(pi, true);
    let conv = env.open_first();
    env.backend.request(BackendRequest::Submit {
        conversation: conv,
        generation: 1,
        op: OperationId(8),
        request: RequestId("req-att".into()),
        text: "with file".into(),
        attachments: vec![pipkin_core::Attachment {
            path: "/tmp/pipkin-no-such-file/a.txt".into(),
            name: "a.txt".into(),
            size: Some(1),
            error: None,
        }],
        model: None,
    });
    let EventKind::Rejected { reason } = env.next_for(8).kind else {
        panic!()
    };
    assert!(
        reason.contains("a.txt") && reason.contains("no longer there"),
        "{reason}"
    );
    assert!(agent.lock().unwrap().prompts.is_empty());
}

fn attached(dir: &std::path::Path, name: &str, bytes: &[u8]) -> pipkin_core::Attachment {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    pipkin_core::describe_attachment(&path)
}

#[test]
fn attachments_reach_the_engine_as_text_and_images_and_read_back_as_chips() {
    let dir = tempfile::tempdir().unwrap();
    let notes = attached(dir.path(), "notes.txt", b"alpha\nbeta\n");
    let png = attached(
        dir.path(),
        "shot.png",
        &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3, 4],
    );
    let (pi, agent, _) = agent_mock(&[("s", 1)], vec![("s", view(&[]))]);
    let env = start(pi, true);
    let conv = env.open_first();
    env.backend.request(BackendRequest::Submit {
        conversation: conv,
        generation: 1,
        op: OperationId(9),
        request: RequestId("req-files".into()),
        text: "review these".into(),
        attachments: vec![notes.clone(), png],
        model: None,
    });
    assert_eq!(env.next_for(9).kind, EventKind::Accepted);
    let prompt = agent.lock().unwrap().prompts[0].clone();
    let message = prompt["message"].as_str().unwrap();
    assert!(
        message.starts_with("review these\n\n<attached-file "),
        "{message}"
    );
    assert!(message.contains("alpha\nbeta\n"));
    assert_eq!(prompt["requestId"], "req-files");
    let images = prompt["images"].as_array().unwrap();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0]["mimeType"], "image/png");
    // What the engine stores reads back as the typed text plus a chip, not a wall of file text.
    let (text, chips) = super::attach::split_message(message);
    assert_eq!(text, "review these");
    assert_eq!(chips[0].name, "notes.txt");
}

#[test]
fn a_file_that_changed_since_it_was_attached_is_refused_before_anything_is_sent() {
    let dir = tempfile::tempdir().unwrap();
    let notes = attached(dir.path(), "notes.txt", b"one");
    std::fs::write(&notes.path, b"one and two").unwrap();
    let (pi, agent, _) = agent_mock(&[("s", 1)], vec![("s", view(&[]))]);
    let env = start(pi, true);
    let conv = env.open_first();
    env.backend.request(BackendRequest::Submit {
        conversation: conv,
        generation: 1,
        op: OperationId(10),
        request: RequestId("req-changed".into()),
        text: "go".into(),
        attachments: vec![notes],
        model: None,
    });
    let EventKind::Rejected { reason } = env.next_for(10).kind else {
        panic!()
    };
    assert!(reason.contains("changed after you attached it"), "{reason}");
    assert!(agent.lock().unwrap().prompts.is_empty());
}

#[test]
fn check_status_resolves_through_the_engine_by_request_key() {
    let (pi, agent, _) = agent_mock(&[("s", 1)], vec![("s", view(&[]))]);
    let env = start(pi, true);
    let conv = env.open_first();
    let check = |op: u64, key: &str| {
        env.backend.request(BackendRequest::CheckStatus {
            conversation: conv,
            generation: 1,
            op: OperationId(op),
            request: Some(RequestId(key.into())),
        })
    };
    // The engine never saw it: proven not accepted, so a retry is safe.
    check(10, "never-arrived");
    assert_eq!(
        env.next_for(10).kind,
        EventKind::StatusResolved { accepted: false }
    );
    // The engine has it and it is still running: accepted, then settles later.
    agent
        .lock()
        .unwrap()
        .statuses
        .insert("running".into(), placed("9"));
    check(11, "running");
    assert_eq!(
        env.next_for(11).kind,
        EventKind::StatusResolved { accepted: true }
    );
    agent
        .lock()
        .unwrap()
        .statuses
        .insert("running".into(), settled("9", "done", None, None));
    assert_eq!(env.next_for(11).kind, EventKind::Completed);
    // The engine has it and it already finished.
    agent
        .lock()
        .unwrap()
        .statuses
        .insert("finished".into(), settled("8", "done", None, None));
    check(12, "finished");
    assert_eq!(
        env.next_for(12).kind,
        EventKind::StatusResolved { accepted: true }
    );
    assert_eq!(env.next_for(12).kind, EventKind::Completed);
    // No local request key (an adopted run): read the live engine replica, not a made-up key.
    env.backend.request(BackendRequest::CheckStatus {
        conversation: conv,
        generation: 1,
        op: OperationId(13),
        request: None,
    });
    let mut refreshed = false;
    wait_until("the live replica is re-read", || {
        while let Ok(event) = env.events.try_recv() {
            if let EventKind::EngineState { busy, .. } = event.kind {
                assert!(!busy);
                refreshed = true;
            }
        }
        refreshed
    });
}

#[test]
fn failed_stop_and_status_requests_are_scoped_and_do_not_hide_the_engine_outcome() {
    let (pi, agent, _) = agent_mock(&[("s", 1)], vec![("s", view(&[]))]);
    let env = start(pi, true);
    let conv = env.open_first();
    submit(&env, conv, 1, 30, "req-stop-error", "long job");
    assert_eq!(env.next_for(30).kind, EventKind::Accepted);
    let a = agent.clone();
    env.pi
        .set_handler("pi.agent-controller", "abort", move |_, _, _| {
            a.lock().unwrap().aborts += 1;
            Err(pi_client::protocol::ProtocolError {
                code: "busy".into(),
                message: "stop service unavailable".into(),
            })
        });
    let stop = || {
        env.backend.request(BackendRequest::Cancel {
            conversation: conv,
            generation: 1,
            op: OperationId(30),
        })
    };
    stop();
    let failure = env.next_for(30);
    assert_eq!((failure.conversation, failure.generation), (conv, 1));
    let EventKind::StopFailed { message } = failure.kind else {
        panic!("wrong stop result")
    };
    assert!(message.contains("stop service unavailable"));
    env.backend.request(BackendRequest::CheckStatus {
        conversation: conv,
        generation: 1,
        op: OperationId(30),
        request: Some(RequestId("req-stop-error".into())),
    });
    assert_eq!(
        env.next_for(30).kind,
        EventKind::StatusResolved { accepted: true }
    );
    let a = agent.clone();
    env.pi
        .set_handler("pi.agent-controller", "abort", move |_, _, _| {
            a.lock().unwrap().aborts += 1;
            Ok(None) // Stop request acknowledged, but the run has not settled yet.
        });
    stop();
    wait_until("the retried stop request arrives", || {
        agent.lock().unwrap().aborts == 2
    });
    agent.lock().unwrap().statuses.insert(
        "req-stop-error".into(),
        settled(
            "1",
            "unanswered",
            Some("model_error"),
            Some("provider failed while stopping"),
        ),
    );
    assert_eq!(
        env.next_for(30).kind,
        EventKind::Failed {
            message: "provider failed while stopping".into()
        }
    );
    assert_eq!(
        agent.lock().unwrap().prompts.len(),
        1,
        "retrying stop must not resend a prompt"
    );

    env.pi
        .set_handler("pi.agent-controller", "lookup", |_, _, _| {
            Err(pi_client::protocol::ProtocolError {
                code: "internal_error".into(),
                message: "status service unavailable".into(),
            })
        });
    env.backend.request(BackendRequest::CheckStatus {
        conversation: conv,
        generation: 1,
        op: OperationId(31),
        request: Some(RequestId("unknown".into())),
    });
    let EventKind::StatusCheckFailed { message } = env.next_for(31).kind else {
        panic!("wrong check result")
    };
    assert!(message.contains("status service unavailable"));
    assert!(message.contains("still unresolved"));
}

#[test]
fn engine_failures_read_as_failures_with_their_message() {
    let (pi, agent, _) = agent_mock(&[("s", 1)], vec![("s", view(&[]))]);
    let env = start(pi, true);
    let conv = env.open_first();
    submit(&env, conv, 1, 20, "req-err", "go");
    assert_eq!(env.next_for(20).kind, EventKind::Accepted);
    agent.lock().unwrap().statuses.insert(
        "req-err".into(),
        settled(
            "1",
            "unanswered",
            Some("model_error"),
            Some("provider unavailable"),
        ),
    );
    assert_eq!(
        env.next_for(20).kind,
        EventKind::Failed {
            message: "provider unavailable".into()
        }
    );
}

#[test]
fn stopping_a_run_asks_the_engine_and_settles_as_cancelled() {
    let (pi, agent, _) = agent_mock(&[("s", 1)], vec![("s", view(&[]))]);
    let env = start(pi, true);
    let conv = env.open_first();
    submit(&env, conv, 1, 30, "req-stop", "long job");
    assert_eq!(env.next_for(30).kind, EventKind::Accepted);
    env.backend.request(BackendRequest::Cancel {
        conversation: conv,
        generation: 1,
        op: OperationId(30),
    });
    // Stop is only a request: the run settles when the engine reports it.
    assert_eq!(env.next_for(30).kind, EventKind::Cancelled);
    assert_eq!(agent.lock().unwrap().aborts, 1);
}

#[test]
fn a_run_that_finished_while_another_session_was_open_settles_in_background() {
    let (pi, agent, _) = agent_mock(
        &[("s-a", 100), ("s-b", 200)],
        vec![("s-a", view(&[])), ("s-b", view(&[]))],
    );
    let env = start(pi, true);
    env.wait_ready();
    let (a, b) = {
        let c = env.catalogs().pop().unwrap().conversations;
        let find = |t: &str| c.iter().find(|x| x.2.contains(t)).unwrap().0;
        (find("s-a"), find("s-b"))
    };
    env.open(a, 1);
    assert!(matches!(env.next_event().kind, EventKind::Opened { .. }));
    submit(&env, a, 1, 40, "req-away", "start it");
    assert_eq!(env.next_for(40).kind, EventKind::Accepted);
    env.open(b, 1);
    assert!(matches!(
        env.next_for_conversation(b).kind,
        EventKind::Opened { .. }
    ));
    assert!(matches!(
        env.next_for_conversation(b).kind,
        EventKind::ThinkingState { .. }
    ));
    // The separate watcher observes the outcome without moving the visible attachment.
    agent
        .lock()
        .unwrap()
        .statuses
        .insert("req-away".into(), settled("1", "done", None, None));
    let settled = env.next_for(40);
    assert_eq!(settled.conversation, a);
    assert_eq!(settled.kind, EventKind::Completed);
    // A scan started before the switch can complete after the run itself. Neither its
    // contents nor a follow-up scan may be redirected to the visible conversation.
    env.backend
        .tx
        .send(Msg::Changes {
            scanned_through: 0,
            conversation: a,
            generation: 1,
            workspace: Workspace::Changes {
                files: vec![FileChange {
                    path: "only-in-a.txt".into(),
                    added: 1,
                    removed: 0,
                    hunks: vec![],
                }],
                truncated: false,
            },
        })
        .unwrap();
    std::thread::sleep(Duration::from_millis(250));
    while let Ok(event) = env.events.try_recv() {
        assert!(
            !matches!(
                event.kind,
                EventKind::ChangesSynced(_) | EventKind::ChangesScanState(_)
            ),
            "a background completion must not change the visible inspector: {event:?}"
        );
    }
    assert!(
        env.conns
            .lock()
            .unwrap()
            .iter()
            .any(|conn| { conn.attachment().is_some_and(|a| a.session_id == "s-b") })
    );
}

impl Env {
    fn next_for_conversation(&self, conversation: ConversationId) -> BackendEvent {
        loop {
            let event = self.next_event();
            if event.conversation == conversation {
                return event;
            }
        }
    }
}

#[test]
fn choosing_a_model_asks_the_engine_and_the_selection_follows_its_report() {
    let (pi, agent, _) = agent_mock(&[("s", 1)], vec![("s", view(&[]))]);
    let env = start(pi, true);
    let conv = env.open_first();
    // The transcript can arrive before the model subscription is processed.
    wait_until("initial model selected", || {
        env.lifecycle
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, LifecycleEvent::ModelSelected(Some(m)) if m == "anthropic/sonnet"))
    });
    let initial = loop {
        let ev = env.next_event();
        if let EventKind::ThinkingState { level, levels } = ev.kind {
            break (level, levels);
        }
    };
    assert_eq!(
        initial,
        (
            "off".into(),
            vec![
                "off".to_string(),
                "low".into(),
                "medium".into(),
                "high".into()
            ]
        )
    );
    env.backend.request(BackendRequest::SetThinkingLevel {
        conversation: conv,
        generation: 1,
        level: "high".into(),
    });
    let updated = loop {
        let ev = env.next_event();
        if let EventKind::ThinkingState { level, .. } = ev.kind {
            break level;
        }
    };
    assert_eq!(updated, "high");
    assert_eq!(agent.lock().unwrap().thinking_selections, [json!("high")]);
    env.backend.request(BackendRequest::SetModel {
        conversation: conv,
        generation: 1,
        model: "openai/gpt".into(),
    });
    wait_until("select sent", || !agent.lock().unwrap().selects.is_empty());
    assert_eq!(
        agent.lock().unwrap().selects[0],
        json!({ "provider": "openai", "modelId": "gpt" })
    );
    wait_until("engine reported the new model", || {
        env.lifecycle
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, LifecycleEvent::ModelSelected(Some(m)) if m == "openai/gpt"))
    });
    // A malformed id is not sent to the engine.
    env.backend.request(BackendRequest::SetModel {
        conversation: conv,
        generation: 1,
        model: "noslash".into(),
    });
    env.wait_notice("Unrecognized model");
    assert_eq!(agent.lock().unwrap().selects.len(), 1);
}

#[test]
fn creating_a_conversation_names_the_directory_and_is_safe_to_repeat() {
    let (pi, agent, _) = agent_mock(&[("s", 1)], vec![]);
    let env = start(pi, true);
    env.wait_ready();
    let create = || {
        env.backend.request(BackendRequest::CreateConversation {
            project: pipkin_core::project_id_for_path("/work/app"),
            cwd: "/work/app".into(),
            request: RequestId("create-1".into()),
        })
    };
    create();
    wait_until("created", || agent.lock().unwrap().creates.len() == 1);
    assert_eq!(
        agent.lock().unwrap().creates[0],
        json!({ "id": "create-1", "cwd": "/work/app" })
    );
    // The new session arrives through the directory, in the project of its directory.
    wait_until("catalog lists it", || {
        env.catalogs().last().is_some_and(|c| {
            c.projects.iter().any(|p| p.path == "/work/app")
                && c.conversations
                    .iter()
                    .any(|x| x.1 == pipkin_core::project_id_for_path("/work/app"))
        })
    });
    // Repeating the same creation (a retry after a lost reply) is not an error.
    create();
    env.no_event_within(500);
    assert!(
        !env.lifecycle
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, LifecycleEvent::Notice(_))),
        "a duplicate creation must not show an error"
    );
}

#[test]
fn a_failed_creation_is_reported_as_a_notice() {
    let (pi, _agent, _) = agent_mock(&[("s", 1)], vec![]);
    pi.set_handler("pi.session-management", "create", |_, _, _| {
        Err(pi_client::protocol::ProtocolError {
            code: "service_invalid_value".into(),
            message: "Session cwd is not an existing directory: /nope".into(),
        })
    });
    let env = start(pi, true);
    env.wait_ready();
    env.backend.request(BackendRequest::CreateConversation {
        project: pipkin_core::project_id_for_path("/nope"),
        cwd: "/nope".into(),
        request: RequestId("create-bad".into()),
    });
    env.wait_notice("not an existing directory");
}

#[test]
fn session_changes_refresh_ignores_global_git_state_and_does_not_send_engine_input() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&project)
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(project.join("notes.txt"), "one\n").unwrap();

    let (pi, agent, _) = agent_mock(&[], vec![("s", view(&[]))]);
    pi.publish(
        "pi.session-directory",
        vec![Op::Replace(json!({
            "revision": 2, "sessions": [{
                "serverId": SERVER_ID, "sessionId": "s", "createdAt": 5,
                "cwd": project.to_str().unwrap(),
            }],
        }))],
    );
    let env = start(pi, true);
    let conv = env.conversation(0);
    env.open(conv, 7);
    event_where(&env, |kind| matches!(kind, EventKind::ThinkingState { .. }));
    std::thread::sleep(Duration::from_millis(150));
    while let Ok(event) = env.events.try_recv() {
        assert!(
            !matches!(
                event.kind,
                EventKind::ChangesSynced(_) | EventKind::ChangesScanState(_)
            ),
            "an untouched conversation must not automatically scan"
        );
    }

    let refresh = || {
        env.backend.request(BackendRequest::RefreshChanges {
            conversation: conv,
            generation: 7,
        })
    };
    refresh();
    let loading = event_where(&env, |kind| {
        matches!(kind, EventKind::ChangesScanState(ChangesState::Loading))
    });
    assert_eq!(
        (loading.conversation, loading.generation, loading.op),
        (conv, 7, None)
    );
    let success = event_where(&env, |kind| matches!(kind, EventKind::ChangesSynced(_)));
    let EventKind::ChangesSynced(files) = success.kind else {
        unreachable!()
    };
    assert!(
        files.is_empty(),
        "unrelated workspace files must not enter the session inspector"
    );

    // Recorded session evidence does not depend on today's working directory.
    let moved = root.path().join("moved");
    std::fs::rename(&project, &moved).unwrap();
    refresh();
    let recorded = event_where(&env, |kind| matches!(kind, EventKind::ChangesSynced(_)));
    assert!(matches!(recorded.kind, EventKind::ChangesSynced(files) if files.is_empty()));
    assert_eq!((recorded.conversation, recorded.generation), (conv, 7));
    std::fs::rename(&moved, &project).unwrap();
    refresh();
    event_where(&env, |kind| matches!(kind, EventKind::ChangesSynced(_)));

    // Repository configuration cannot contaminate recorded session changes.
    let config_path = project.join(".git/config");
    let original_config = std::fs::read(&config_path).unwrap();
    std::fs::write(&config_path, "[broken\n").unwrap();
    refresh();
    let recorded = event_where(&env, |kind| matches!(kind, EventKind::ChangesSynced(_)));
    assert!(matches!(recorded.kind, EventKind::ChangesSynced(files) if files.is_empty()));
    std::fs::write(&config_path, original_config).unwrap();
    refresh();
    event_where(&env, |kind| matches!(kind, EventKind::ChangesSynced(_)));

    std::fs::rename(project.join(".git"), root.path().join("saved-git")).unwrap();
    refresh();
    let recorded = event_where(&env, |kind| matches!(kind, EventKind::ChangesSynced(_)));
    assert!(matches!(recorded.kind, EventKind::ChangesSynced(files) if files.is_empty()));
    let agent = agent.lock().unwrap();
    assert!(
        agent.prompts.is_empty() && agent.queued.is_empty() && agent.aborts == 0,
        "a scan must not submit, steer or stop"
    );
}

#[test]
fn automatic_empty_scans_are_quiet_but_file_changes_and_explicit_refresh_are_published() {
    let repo = tempfile::tempdir().unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(repo.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .status()
            .unwrap()
            .success()
    );
    let (pi, _, _) = agent_mock(&[], vec![("s", view(&[]))]);
    pi.publish("pi.session-directory", vec![Op::Replace(json!({ "revision": 2, "sessions": [{
        "serverId": SERVER_ID, "sessionId": "s", "createdAt": 5, "cwd": repo.path().to_str().unwrap(),
    }]}))]);
    let env = start(pi, true);
    let conv = env.conversation(0);
    env.open(conv, 1);
    event_where(&env, |kind| matches!(kind, EventKind::ThinkingState { .. }));
    let refresh = || {
        env.backend.request(BackendRequest::RefreshChanges {
            conversation: conv,
            generation: 1,
        })
    };
    refresh();
    event_where(&env, |kind| {
        matches!(kind, EventKind::ChangesScanState(ChangesState::Loading))
    });
    let first = event_where(&env, |kind| matches!(kind, EventKind::ChangesSynced(_)));
    assert!(matches!(first.kind, EventKind::ChangesSynced(ref files) if files.is_empty()));
    let finish_tools = |count: usize| {
        let entries: Vec<_> = (0..count).flat_map(|i| {
            let call_id = format!("c{i}");
            let name = if i == 3 { "write" } else { "read" };
            let arguments = if i == 3 { json!({"path": "new.txt", "content": "hello\n"}) } else { json!({"path": "notes.txt"}) };
            [json!({ "id": 2*i+1, "kind": "pi.assistant", "model": [{
                "role": "assistant", "stopReason": "toolUse", "timestamp": 0,
                "content": [{ "type": "toolCall", "id": call_id, "name": name, "arguments": arguments }],
            }] }), json!({ "id": 2*i+2, "kind": "pi.tool-result", "model": [{
                "role": "toolResult", "toolCallId": call_id, "toolName": name, "isError": false,
                "timestamp": 0, "content": [{ "type": "text", "text": "done" }],
            }] })]
        }).collect();
        env.pi.publish(
            "pi.transcript",
            vec![Op::Replace(
                json!({ "conversation": { "id": 1 }, "docs": {}, "entries": entries }),
            )],
        );
    };
    for count in 1..=3 {
        finish_tools(count);
        event_where(&env, |kind| matches!(kind, EventKind::Synced { .. }));
        std::thread::sleep(Duration::from_millis(250));
        while let Ok(event) = env.events.try_recv() {
            assert!(
                !matches!(
                    event.kind,
                    EventKind::ChangesScanState(_) | EventKind::ChangesSynced(_)
                ),
                "unchanged background scan must not replace the empty inspector: {:?}",
                event.kind
            );
        }
    }
    // A genuine filesystem change must still reach the inspector automatically, without Loading.
    std::fs::write(repo.path().join("new.txt"), "hello\n").unwrap();
    finish_tools(4);
    let changed = loop {
        let event = env.next_event();
        assert!(!matches!(
            event.kind,
            EventKind::ChangesScanState(ChangesState::Loading)
        ));
        if let EventKind::ChangesSynced(files) = event.kind {
            break files;
        }
    };
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].path, "new.txt");
    refresh();
    event_where(&env, |kind| {
        matches!(kind, EventKind::ChangesScanState(ChangesState::Loading))
    });
    let same = event_where(&env, |kind| matches!(kind, EventKind::ChangesSynced(_)));
    assert!(matches!(same.kind, EventKind::ChangesSynced(files) if files == changed));
}

#[test]
fn workspace_changes_follow_the_session_directory_and_update_when_a_tool_finishes() {
    let repo = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(repo.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(
        repo.path().join("notes.txt"),
        "one
",
    )
    .unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "init"]);

    let (pi, _agent, _) = agent_mock(&[], vec![("s", view(&[]))]);
    pi.publish(
        "pi.session-directory",
        vec![Op::Replace(json!({ "revision": 2, "sessions": [{
            "serverId": SERVER_ID, "sessionId": "s", "createdAt": 5,
            "cwd": repo.path().to_str().unwrap(),
        }]}))],
    );
    let env = start(pi, true);
    let conv = env.conversation(0);
    env.open(conv, 1);
    let changes = |env: &Env| -> Vec<FileChange> {
        loop {
            if let EventKind::ChangesSynced(files) = env.next_event().kind {
                return files;
            }
        }
    };
    // A fresh session starts with a fresh inspector. Its first workspace scan waits
    // for work in this session rather than showing pre-existing project changes.
    assert!(matches!(env.next_event().kind, EventKind::Opened { .. }));
    assert!(matches!(
        env.next_event().kind,
        EventKind::ThinkingState { .. }
    ));
    env.no_event_within(150);

    // A tool edits a tracked file and creates a new one; the transcript shows a finished tool.
    std::fs::write(
        repo.path().join("notes.txt"),
        "one
two
",
    )
    .unwrap();
    std::fs::write(
        repo.path().join("new.txt"),
        "hello
",
    )
    .unwrap();
    let call = json!({ "role": "assistant", "stopReason": "toolUse", "timestamp": 0,
        "content": [
            { "type": "toolCall", "id": "c1", "name": "edit", "arguments": {"path": "notes.txt", "edits": [{"oldText": "one", "newText": "one\ntwo"}]} },
            { "type": "toolCall", "id": "c2", "name": "write", "arguments": {"path": "new.txt", "content": "hello\n"} },
        ] });
    let result = json!({ "role": "toolResult", "toolCallId": "c1", "toolName": "edit", "isError": false,
        "timestamp": 0, "content": [{ "type": "text", "text": "Successfully replaced" }],
        "details": {"patch": "--- a/notes.txt\n+++ b/notes.txt\n@@ -1 +1,2 @@\n one\n+two\n"} });
    let write_result =
        json!({"role": "toolResult", "toolCallId": "c2", "toolName": "write", "isError": false});
    env.pi.publish(
        "pi.transcript",
        vec![Op::Replace(
            json!({ "conversation": { "id": 1 }, "docs": {}, "entries": [
                { "id": 1, "kind": "pi.assistant", "model": [call] },
                { "id": 2, "kind": "pi.tool-result", "model": [result, write_result] },
            ]}),
        )],
    );
    let files = changes(&env);
    let names: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(names, ["notes.txt", "new.txt"], "{files:?}");
    assert_eq!((files[0].added, files[0].removed), (1, 0));
    assert!(
        files[1].hunks[0]
            .lines
            .iter()
            .all(|l| l.kind == pipkin_core::DiffKind::Add)
    );
    // A transcript change that finishes no tool and ends no run does not rescan.
    let _ = ItemKind::Notice {
        text: String::new(),
        level: pipkin_core::NoticeLevel::Info,
    };
}

#[test]
fn changing_sessions_keeps_uncommitted_changes_scoped_to_session_files() {
    let repo = tempfile::tempdir().unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(repo.path())
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(repo.path().join("a.txt"), "session A\n").unwrap();
    std::fs::write(repo.path().join("b.txt"), "session B\n").unwrap();
    std::fs::write(
        repo.path().join("unrelated.txt"),
        "unrelated workspace edit",
    )
    .unwrap();
    let recorded = |path: &str| {
        json!({ "conversation": {"id": 1}, "docs": {}, "entries": [
            {"id": 1, "kind": "pi.assistant", "model": [{"role": "assistant", "content": [
                {"type": "toolCall", "id": "c", "name": "write", "arguments": {"path": path, "content": "written"}}
            ]}]},
            {"id": 2, "kind": "pi.tool-result", "model": [{"role": "toolResult", "toolCallId": "c", "toolName": "write", "isError": false}]}
        ]})
    };
    let (pi, _, _) = agent_mock(
        &[],
        vec![("a", recorded("a.txt")), ("b", recorded("b.txt"))],
    );
    pi.publish("pi.session-directory", vec![Op::Replace(json!({"revision": 2, "sessions": [
        {"serverId": SERVER_ID, "sessionId": "a", "createdAt": 10, "cwd": repo.path().to_str().unwrap()},
        {"serverId": SERVER_ID, "sessionId": "b", "createdAt": 5, "cwd": repo.path().to_str().unwrap()},
    ]}))]);
    let env = start(pi, true);
    env.wait_ready();
    let a = ConversationId(pipkin_core::stable_id("a"));
    let b = ConversationId(pipkin_core::stable_id("b"));
    for (conversation, generation, expected) in [
        (a, 1, "session A"),
        (b, 1, "session B"),
        (a, 2, "session A"),
    ] {
        env.open(conversation, generation);
        let event = loop {
            let event = env.next_event();
            if event.conversation == conversation
                && event.generation == generation
                && matches!(event.kind, EventKind::ChangesSynced(_))
            {
                break event;
            }
        };
        let EventKind::ChangesSynced(files) = event.kind else {
            unreachable!()
        };
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].path,
            if conversation == a { "a.txt" } else { "b.txt" }
        );
        assert_eq!(files[0].hunks[0].lines[0].text, expected);
    }
    // An external commit, with no transcript update, clears the selected session automatically.
    let git = |args: &[&str]| {
        assert!(
            std::process::Command::new("git")
                .args([
                    "-c",
                    "user.name=test",
                    "-c",
                    "user.email=test@example.com",
                    "-c",
                    "commit.gpgsign=false"
                ])
                .args(args)
                .current_dir(repo.path())
                .status()
                .unwrap()
                .success()
        );
    };
    git(&["add", "a.txt"]);
    git(&["commit", "-q", "-m", "session A"]);
    let cleared = event_where(
        &env,
        |kind| matches!(kind, EventKind::ChangesSynced(files) if files.is_empty()),
    );
    assert_eq!((cleared.conversation, cleared.generation), (a, 2));
    // Old committed calls cannot claim a later edit to the same file as new session work.
    std::fs::write(repo.path().join("a.txt"), "unrelated later edit\n").unwrap();
    env.open(b, 2);
    event_where(
        &env,
        |kind| matches!(kind, EventKind::ChangesSynced(files) if !files.is_empty()),
    );
    env.open(a, 3);
    event_where(&env, |kind| matches!(kind, EventKind::ThinkingState { .. }));
    env.backend.request(BackendRequest::RefreshChanges {
        conversation: a,
        generation: 3,
    });
    let refreshed = event_where(&env, |kind| matches!(kind, EventKind::ChangesSynced(_)));
    assert!(matches!(refreshed.kind, EventKind::ChangesSynced(files) if files.is_empty()));
}

// -------------------------------------------------------------------- steer, queue, stop

fn queue(env: &Env, conv: ConversationId, request: &str, mode: pipkin_core::QueueMode, text: &str) {
    env.backend.request(BackendRequest::Queue {
        conversation: conv,
        generation: 1,
        request: RequestId(request.into()),
        mode,
        text: text.into(),
        attachments: vec![],
    });
}

/// The next queue-related event for `request`, skipping everything else.
fn queue_event(env: &Env, request: &str) -> EventKind {
    loop {
        let event = env.next_event();
        match &event.kind {
            EventKind::QueueAdmitted { request: r, .. }
            | EventKind::QueueRefused { request: r, .. }
            | EventKind::QueueAckLost { request: r }
                if r.0 == request =>
            {
                return event.kind;
            }
            _ => {}
        }
    }
}

#[test]
fn steers_and_follow_ups_go_through_the_controller_under_their_journaled_keys() {
    use pipkin_core::QueueMode::{FollowUp, Steer};
    let (pi, agent, _) = agent_mock(&[("s", 1)], vec![("s", view(&[]))]);
    let env = start(pi, true);
    let conv = env.open_first();
    submit(&env, conv, 1, 50, "req-run", "long job");
    assert_eq!(env.next_for(50).kind, EventKind::Accepted);

    queue(&env, conv, "req-steer", Steer, "use tabs");
    let EventKind::QueueAdmitted { request, entry } = queue_event(&env, "req-steer") else {
        panic!()
    };
    assert_eq!(
        (request.0.as_str(), entry),
        ("req-steer", pipkin_core::QueueId(2))
    );
    queue(&env, conv, "req-next", FollowUp, "then ship");
    assert!(matches!(
        queue_event(&env, "req-next"),
        EventKind::QueueAdmitted { .. }
    ));

    let agent = agent.lock().unwrap();
    let members: Vec<_> = agent.queued.iter().map(|(m, _)| m.as_str()).collect();
    assert_eq!(members, ["steer", "followUp"]);
    assert_eq!(agent.queued[0].1["message"], "use tabs");
    assert_eq!(agent.queued[0].1["requestId"], "req-steer");
    assert_eq!(agent.queued[1].1["requestId"], "req-next");
    assert_eq!(agent.prompts.len(), 1, "queueing is not a second prompt");
}

#[test]
fn sending_a_queue_key_again_does_not_admit_it_twice() {
    use pipkin_core::QueueMode::FollowUp;
    let (pi, agent, _) = agent_mock(&[("s", 1)], vec![("s", view(&[]))]);
    let env = start(pi, true);
    let conv = env.open_first();
    queue(&env, conv, "req-q", FollowUp, "once");
    let first = queue_event(&env, "req-q");
    queue(&env, conv, "req-q", FollowUp, "once");
    let second = queue_event(&env, "req-q");
    assert_eq!(first, second, "the same entry comes back");
    assert_eq!(
        agent.lock().unwrap().statuses.len(),
        1,
        "the engine holds one submission for the key"
    );
}

#[test]
fn a_refused_queue_request_is_definite() {
    use pipkin_core::QueueMode::Steer;
    let (pi, agent, _) = agent_mock(&[("s", 1)], vec![("s", view(&[]))]);
    let env = start(pi, true);
    let conv = env.open_first();
    agent.lock().unwrap().queue_answer = QueueAnswer::Reject("nothing is running");
    queue(&env, conv, "req-no", Steer, "x");
    let EventKind::QueueRefused { reason, .. } = queue_event(&env, "req-no") else {
        panic!()
    };
    assert!(reason.contains("nothing is running"), "{reason}");
}

#[test]
fn a_queue_request_for_a_session_that_is_not_open_is_refused_and_nothing_is_sent() {
    use pipkin_core::QueueMode::Steer;
    let (pi, agent, _) = agent_mock(&[("s", 1)], vec![("s", view(&[]))]);
    let env = start(pi, true);
    let conv = env.conversation(0); // listed, never opened
    queue(&env, conv, "req-closed", Steer, "x");
    let EventKind::QueueRefused { reason, .. } = queue_event(&env, "req-closed") else {
        panic!()
    };
    assert!(reason.contains("not open"), "{reason}");
    assert!(agent.lock().unwrap().queued.is_empty());
}

#[test]
fn a_lost_acknowledgment_that_did_arrive_resolves_by_lookup_without_resending() {
    use pipkin_core::QueueMode::FollowUp;
    let (pi, agent, _) = agent_mock(&[("s", 1)], vec![("s", view(&[]))]);
    let env = start(pi, true);
    let conv = env.open_first();
    agent.lock().unwrap().queue_answer = QueueAnswer::DropAfterAdmit;
    queue(&env, conv, "req-lost", FollowUp, "maybe");
    assert!(matches!(
        queue_event(&env, "req-lost"),
        EventKind::QueueAckLost { .. }
    ));
    // The adapter reconnects and asks the engine about the key; it finds the entry.
    let EventKind::QueueAdmitted { entry, .. } = queue_event(&env, "req-lost") else {
        panic!()
    };
    assert_eq!(entry, pipkin_core::QueueId(1));
    assert_eq!(
        agent.lock().unwrap().queued.len(),
        1,
        "the request was not sent a second time"
    );
}

#[test]
fn a_lost_request_the_engine_never_saw_is_sent_again_under_the_same_key() {
    use pipkin_core::QueueMode::FollowUp;
    let (pi, agent, _) = agent_mock(&[("s", 1)], vec![("s", view(&[]))]);
    let env = start(pi, true);
    let conv = env.open_first();
    agent.lock().unwrap().queue_answer = QueueAnswer::DropBefore;
    queue(&env, conv, "req-gone", FollowUp, "still wanted");
    assert!(matches!(
        queue_event(&env, "req-gone"),
        EventKind::QueueAckLost { .. }
    ));
    assert!(matches!(
        queue_event(&env, "req-gone"),
        EventKind::QueueAdmitted { .. }
    ));
    let agent = agent.lock().unwrap();
    assert_eq!(agent.queued.len(), 1, "it arrived exactly once");
    assert_eq!(agent.queued[0].1["requestId"], "req-gone");
    assert_eq!(agent.queued[0].1["message"], "still wanted");
}

#[test]
fn removing_a_queued_input_reports_what_the_engine_did() {
    let (pi, agent, _) = agent_mock(&[("s", 1)], vec![("s", view(&[]))]);
    let env = start(pi, true);
    let conv = env.open_first();
    for (outcome, expected) in [
        ("cancelled", pipkin_core::CancelOutcome::Cancelled),
        (
            "already_consumed",
            pipkin_core::CancelOutcome::AlreadyConsumed,
        ),
        ("not_found", pipkin_core::CancelOutcome::NotFound),
    ] {
        agent.lock().unwrap().cancel_outcome = outcome;
        env.backend.request(BackendRequest::CancelQueued {
            conversation: conv,
            generation: 1,
            entry: pipkin_core::QueueId(12),
        });
        loop {
            let event = env.next_event();
            if let EventKind::QueueCancelled { entry, outcome } = event.kind {
                assert_eq!((entry, outcome), (pipkin_core::QueueId(12), expected));
                break;
            }
        }
    }
    assert_eq!(agent.lock().unwrap().cancelled, ["12", "12", "12"]);
}

#[test]
fn the_engines_queue_and_busy_flag_are_reported_with_every_refresh() {
    let (pi, _agent, _) = agent_mock(&[("s", 1)], vec![("s", view(&[]))]);
    let env = start(pi, true);
    let conv = env.conversation(0);
    env.open(conv, 1);
    let state = |env: &Env| loop {
        let event = env.events.recv_blocking().unwrap();
        if let EventKind::EngineState { busy, queue } = event.kind {
            return (busy, queue);
        }
    };
    assert_eq!(state(&env), (false, vec![]));
    env.pi.publish(
        "pi.transcript",
        vec![
            Op::Set(
                vec![key("docs"), key("pi.live")],
                json!({ "run": { "taskId": 9, "inputs": [8] } }),
            ),
            Op::Set(
                vec![key("docs"), key("pi.inbox")],
                json!({ "items": [
                    { "id": 21, "mode": "steer", "content": "use tabs" },
                    { "id": 22, "mode": "followUp", "content": "then ship" },
                ]}),
            ),
        ],
    );
    let (busy, queue) = state(&env);
    assert!(busy);
    assert_eq!(
        queue
            .iter()
            .map(|q| (q.id.0, q.text.as_str()))
            .collect::<Vec<_>>(),
        [(21, "use tabs"), (22, "then ship")]
    );
    // The run ends and the engine drains the queue.
    env.pi.publish(
        "pi.transcript",
        vec![
            Op::Set(vec![key("docs"), key("pi.live")], json!({})),
            Op::Set(vec![key("docs"), key("pi.inbox")], json!({ "items": [] })),
        ],
    );
    let (busy, queue) = state(&env);
    assert!(!busy && queue.is_empty());
}

#[test]
fn refreshing_models_asks_the_engine_and_says_when_a_provider_could_not_be_read() {
    let (pi, agent, _) = agent_mock(&[("s", 1)], vec![("s", view(&[]))]);
    let env = start(pi, true);
    let conv = env.open_first();
    env.backend.request(BackendRequest::RefreshModels {
        conversation: conv,
        generation: 1,
    });
    wait_until("refresh sent", || agent.lock().unwrap().refreshes == 1);
    env.pi.publish(
        "pi.models",
        vec![Op::Set(
            vec![key("refresh")],
            json!({ "status": "warning", "errors": { "anthropic": "401 invalid x-api-key" } }),
        )],
    );
    env.wait_notice("anthropic: 401 invalid x-api-key");
    env.wait_notice("sign-in or API key");
}

#[test]
fn queue_requests_while_offline_are_refused_not_lost() {
    // Nothing listens: the adapter is offline, and answers instead of queueing silently.
    let dir = tempfile::tempdir().unwrap();
    let (tx, rx) = async_channel::unbounded();
    let mut config = PiConfig::new(dir.path().to_path_buf());
    config.server_id = Some(SERVER_ID.into());
    config.retry_delay = Duration::from_millis(50);
    let backend = PiBackend::new(tx, config);
    backend.start(Box::new(|_| {}));
    backend.request(BackendRequest::Queue {
        conversation: ConversationId(1),
        generation: 1,
        request: RequestId("req-off".into()),
        mode: pipkin_core::QueueMode::Steer,
        text: "x".into(),
        attachments: vec![],
    });
    let event = rx.recv_blocking().unwrap();
    assert!(matches!(
        event.kind,
        EventKind::QueueRefused { ref reason, .. } if reason.contains("Not connected")
    ));
    backend.shutdown();
}

// ------------------------------------------------------- M4: history, tool output, questions

use pipkin_core::{ItemId, UiAnswer, UiNoticeLevel, UiRequestKind};

/// The next event satisfying `wanted`; anything else before it is skipped.
fn event_where(env: &Env, wanted: impl Fn(&EventKind) -> bool) -> BackendEvent {
    loop {
        let event = env.next_event_with_scan_state();
        if wanted(&event.kind) {
            return event;
        }
    }
}

fn tool_result_entry(id: u64, call: &str, text: &str) -> Value {
    json!({ "id": id, "conversationId": 1, "kind": "pi.message", "model": [{
        "role": "toolResult", "toolCallId": call, "toolName": "bash", "isError": false,
        "content": [{ "type": "text", "text": text }], "timestamp": 1_700_000_000_000i64 }] })
}

type HistoryCalls = Arc<Mutex<Vec<(Option<u64>, u64)>>>;

/// A mock engine whose `pi.history` serves `all` (every entry the conversation ever had, in any
/// order) the way the real service does: newest first, strictly older than `before`.
fn history_mock(sessions: &[(&str, i64)], view: Value, all: Vec<Value>) -> (MockPi, HistoryCalls) {
    let pi = mock(sessions, vec![(sessions[0].0, view)]);
    pi.add_service("pi.history", &["page"], None);
    let calls: HistoryCalls = Arc::new(Mutex::new(Vec::new()));
    let log = calls.clone();
    pi.set_handler("pi.history", "page", move |_, _, call| {
        let request = &call.args[0];
        let before = request["before"].as_u64();
        let limit = request["limit"].as_u64().unwrap_or(50) as usize;
        log.lock().unwrap().push((before, limit as u64));
        let mut older: Vec<Value> = all
            .iter()
            .filter(|e| before.is_none_or(|b| e["id"].as_u64().unwrap_or(0) < b))
            .cloned()
            .collect();
        older.sort_by_key(|e| std::cmp::Reverse(e["id"].as_u64().unwrap_or(0)));
        let more = older.len() > limit;
        older.truncate(limit);
        Ok(Some(json!({ "entries": older, "more": more })))
    });
    (pi, calls)
}

fn view_of(entries: Vec<Value>) -> Value {
    json!({ "conversation": { "id": 1 }, "entries": entries, "docs": {} })
}

#[test]
fn session_changes_recover_file_scope_from_before_compaction() {
    let repo = tempfile::tempdir().unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(repo.path())
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(repo.path().join("old.rs"), "after\n").unwrap();
    let all = vec![
        json!({"id": 1, "kind": "pi.assistant", "model": [{"role": "assistant", "content": [
            {"type": "toolCall", "id": "c", "name": "edit", "arguments": {"path": "old.rs", "oldText": "before", "newText": "after"}}
        ]}]}),
        json!({"id": 2, "kind": "pi.tool-result", "model": [{"role": "toolResult", "toolCallId": "c", "toolName": "edit", "isError": false}]}),
        user_entry(3, "continue after compaction"),
    ];
    let (pi, _) = history_mock(&[("s", 1)], view_of(vec![all[2].clone()]), all);
    pi.publish("pi.session-directory", vec![Op::Replace(json!({"revision": 2, "sessions": [{"serverId": SERVER_ID, "sessionId": "s", "createdAt": 1, "cwd": repo.path().to_str().unwrap()}]}))]);
    let env = start(pi, true);
    let conv = env.conversation(0);
    env.open(conv, 1);
    let event = event_where(&env, |kind| matches!(kind, EventKind::ChangesSynced(_)));
    let EventKind::ChangesSynced(files) = event.kind else {
        unreachable!()
    };
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "old.rs");
    assert_eq!(files[0].hunks[0].lines[0].text, "after");
}

#[test]
fn a_conversation_with_history_before_its_view_offers_to_load_it_and_pages_back_without_gaps() {
    let all: Vec<Value> = (1..=125)
        .map(|n| user_entry(n, &format!("message {n}")))
        .collect();
    // The live view holds only the newest five entries (the rest is before a compaction).
    let (pi, calls) = history_mock(&[("s", 1)], view_of(all[120..].to_vec()), all);
    let env = start(pi, true);
    let conv = env.conversation(0);
    env.open(conv, 1);
    let EventKind::Opened {
        items, has_older, ..
    } = event_where(&env, |k| matches!(k, EventKind::Opened { .. })).kind
    else {
        unreachable!()
    };
    assert!(has_older, "entries 1..=120 are older than the view");
    assert_eq!(items.len(), 5);
    assert_eq!(
        calls.lock().unwrap()[0],
        (Some(121), 1),
        "asked cheaply: is there anything before the oldest?"
    );

    let mut oldest = items[0].id;
    let mut collected: Vec<u64> = Vec::new();
    loop {
        env.backend.request(BackendRequest::LoadOlder {
            conversation: conv,
            generation: 1,
            before: Some(oldest),
        });
        let EventKind::OlderPage { items, has_older } =
            event_where(&env, |k| matches!(k, EventKind::OlderPage { .. })).kind
        else {
            unreachable!()
        };
        assert!(items.len() <= 50);
        // Each page is oldest first and ends just before what was already shown.
        assert!(items.windows(2).all(|w| w[0].id < w[1].id));
        assert!(items.last().is_none_or(|last| last.id < oldest));
        collected.splice(0..0, items.iter().map(|i| i.id.0 / 1024));
        if let Some(first) = items.first() {
            oldest = first.id;
        }
        if !has_older {
            break;
        }
    }
    assert_eq!(
        collected,
        (1..=120).collect::<Vec<_>>(),
        "every older entry once, in order"
    );
}

#[test]
fn a_conversation_with_nothing_before_its_view_has_no_older_history_and_an_old_engine_has_none_to_offer()
 {
    let all: Vec<Value> = (1..=3).map(|n| user_entry(n, "m")).collect();
    let (pi, _) = history_mock(&[("s", 1)], view_of(all.clone()), all);
    let env = start(pi, true);
    let conv = env.conversation(0);
    env.open(conv, 1);
    let EventKind::Opened { has_older, .. } =
        event_where(&env, |k| matches!(k, EventKind::Opened { .. })).kind
    else {
        unreachable!()
    };
    assert!(!has_older);

    // An engine that does not have the service at all: the same, and no error.
    let env = start(mock(&[("s", 1)], vec![("s", view(&["a", "b"]))]), true);
    let conv = env.conversation(0);
    env.open(conv, 1);
    let EventKind::Opened { has_older, .. } =
        event_where(&env, |k| matches!(k, EventKind::Opened { .. })).kind
    else {
        unreachable!()
    };
    assert!(!has_older);
    env.backend.request(BackendRequest::LoadOlder {
        conversation: conv,
        generation: 1,
        before: Some(ItemId(1024)),
    });
    let EventKind::OlderFailed { message } =
        event_where(&env, |k| matches!(k, EventKind::OlderFailed { .. })).kind
    else {
        unreachable!()
    };
    assert!(!message.is_empty());
}

#[test]
fn a_page_that_cannot_be_loaded_says_so_and_a_closed_session_cannot_page() {
    let all: Vec<Value> = (1..=10).map(|n| user_entry(n, "m")).collect();
    let (pi, _) = history_mock(&[("s", 1)], view_of(all[8..].to_vec()), all);
    pi.set_handler("pi.history", "page", |_, _, _| {
        Err(pi_client::protocol::ProtocolError {
            code: "internal_error".into(),
            message: "boom".into(),
        })
    });
    let env = start(pi, true);
    let conv = env.conversation(0);
    // Not opened yet: refused locally.
    env.backend.request(BackendRequest::LoadOlder {
        conversation: conv,
        generation: 1,
        before: Some(ItemId(9 * 1024)),
    });
    let EventKind::OlderFailed { message } =
        event_where(&env, |k| matches!(k, EventKind::OlderFailed { .. })).kind
    else {
        unreachable!()
    };
    assert!(message.contains("not open"), "{message}");
    env.open(conv, 1);
    event_where(&env, |k| matches!(k, EventKind::Opened { .. }));
    env.backend.request(BackendRequest::LoadOlder {
        conversation: conv,
        generation: 1,
        before: Some(ItemId(9 * 1024)),
    });
    let EventKind::OlderFailed { message } =
        event_where(&env, |k| matches!(k, EventKind::OlderFailed { .. })).kind
    else {
        unreachable!()
    };
    assert!(message.contains("boom"), "{message}");
    // An item that is not from an entry has nothing before it.
    env.backend.request(BackendRequest::LoadOlder {
        conversation: conv,
        generation: 1,
        before: None,
    });
    let EventKind::OlderPage { items, has_older } =
        event_where(&env, |k| matches!(k, EventKind::OlderPage { .. })).kind
    else {
        unreachable!()
    };
    assert!(items.is_empty() && !has_older);
}

#[test]
fn the_complete_result_of_a_tool_call_comes_from_the_live_view_or_from_history() {
    let big = "line of output\n".repeat(2000); // 30 KB, well past the preview
    let in_view = tool_result_entry(5, "call_view", &big);
    let before_compaction = tool_result_entry(2, "call_old", &"older output\n".repeat(1500));
    let all = vec![
        before_compaction,
        user_entry(3, "x"),
        user_entry(4, "y"),
        in_view.clone(),
    ];
    let (pi, calls) = history_mock(&[("s", 1)], view_of(vec![user_entry(4, "y"), in_view]), all);
    let env = start(pi, true);
    let conv = env.conversation(0);
    env.open(conv, 1);
    event_where(&env, |k| matches!(k, EventKind::Opened { .. }));
    let fetch = |call: &str| {
        env.backend.request(BackendRequest::FetchToolOutput {
            conversation: conv,
            generation: 1,
            call_id: call.into(),
        });
    };
    fetch("call_view");
    let EventKind::ToolOutputFull { call_id, text } = event_where(&env, |k| {
        matches!(
            k,
            EventKind::ToolOutputFull { .. } | EventKind::ToolOutputUnavailable { .. }
        )
    })
    .kind
    else {
        panic!("expected the output")
    };
    assert_eq!((call_id.as_str(), text.len()), ("call_view", big.len()));
    let history_calls_before = calls.lock().unwrap().len();

    // One from before the view's start is found by paging history.
    fetch("call_old");
    let EventKind::ToolOutputFull { text, .. } = event_where(&env, |k| {
        matches!(
            k,
            EventKind::ToolOutputFull { .. } | EventKind::ToolOutputUnavailable { .. }
        )
    })
    .kind
    else {
        panic!("expected the older output")
    };
    assert_eq!(text, "older output\n".repeat(1500));
    assert!(calls.lock().unwrap().len() > history_calls_before);

    // One that is nowhere is reported, not invented.
    fetch("call_nowhere");
    let EventKind::ToolOutputUnavailable { call_id, reason } = event_where(&env, |k| {
        matches!(
            k,
            EventKind::ToolOutputFull { .. } | EventKind::ToolOutputUnavailable { .. }
        )
    })
    .kind
    else {
        panic!("expected a refusal")
    };
    assert_eq!(call_id, "call_nowhere");
    assert!(reason.contains("does not have"), "{reason}");

    // Asking about a conversation that is not open fails the same way.
    env.backend.request(BackendRequest::FetchToolOutput {
        conversation: ConversationId(42),
        generation: 1,
        call_id: "x".into(),
    });
    event_where(&env, |k| {
        matches!(k, EventKind::ToolOutputUnavailable { .. })
    });
}

fn ui_request(id: &str, kind: &str) -> Value {
    json!({ "id": id, "kind": kind, "title": format!("Question {id}"), "message": null,
        "items": [{ "value": "a", "label": "Choice A", "description": null }],
        "placeholder": null, "defaultValue": null, "createdAt": 1, "deadline": 99 })
}

type UiCalls = Arc<Mutex<Vec<(String, Value)>>>;

fn ui_mock() -> (MockPi, UiCalls) {
    let pi = mock(&[("s", 1)], vec![("s", view(&[]))]);
    pi.add_service(
        "pi.ui-requests",
        &["respond", "cancel"],
        Some(json!({ "requests": [], "status": {}, "notices": [] })),
    );
    let calls: UiCalls = Arc::new(Mutex::new(Vec::new()));
    let log = calls.clone();
    pi.set_handler("pi.ui-requests", "respond", move |pi, _, call| {
        let (id, value) = (
            call.args[0].as_str().unwrap_or("").to_owned(),
            call.args[1].clone(),
        );
        log.lock()
            .unwrap()
            .push((format!("respond {id}"), value.clone()));
        if id == "q1" && value == json!("a") {
            pi.publish(
                "pi.ui-requests",
                vec![Op::Set(vec![key("requests")], json!([]))],
            );
            Ok(Some(json!({ "accepted": true, "reason": null })))
        } else {
            Ok(Some(
                json!({ "accepted": false, "reason": "That is not one of the choices." }),
            ))
        }
    });
    let log = calls.clone();
    pi.set_handler("pi.ui-requests", "cancel", move |_, _, call| {
        log.lock().unwrap().push((
            format!("cancel {}", call.args[0].as_str().unwrap_or("")),
            Value::Null,
        ));
        Ok(Some(json!({ "cancelled": true })))
    });
    (pi, calls)
}

#[test]
fn questions_from_extensions_are_shown_answered_and_cancelled_through_the_engine() {
    let (pi, calls) = ui_mock();
    let env = start(pi, true);
    let conv = env.conversation(0);
    env.open(conv, 1);
    let ui_state = |env: &Env| {
        let EventKind::UiState {
            requests,
            status,
            notices,
        } = event_where(env, |k| matches!(k, EventKind::UiState { .. })).kind
        else {
            unreachable!()
        };
        (requests, status, notices)
    };
    assert_eq!(ui_state(&env), (vec![], vec![], vec![]), "reported at open");

    env.pi.publish(
        "pi.ui-requests",
        vec![
            Op::Set(
                vec![key("requests")],
                json!([ui_request("q1", "select"), ui_request("q2", "wheel")]),
            ),
            Op::Set(
                vec![key("status")],
                json!({ "lint": "clean", "build": "compiling" }),
            ),
            Op::Set(
                vec![key("notices")],
                json!([{ "id": "n1", "level": "warning", "message": "heads up", "at": 1 }]),
            ),
        ],
    );
    let (requests, status, notices) = ui_state(&env);
    assert_eq!(
        requests.len(),
        1,
        "the unknown kind is not shown as a question"
    );
    assert_eq!(
        (requests[0].id.as_str(), requests[0].kind),
        ("q1", UiRequestKind::Select)
    );
    assert_eq!(requests[0].items[0].label, "Choice A");
    assert_eq!(requests[0].deadline, Some(99));
    assert_eq!(
        status,
        [
            ("build".to_owned(), "compiling".to_owned()),
            ("lint".to_owned(), "clean".to_owned())
        ]
    );
    // The question this build cannot show is said so, plainly, before it times out.
    assert!(
        notices
            .iter()
            .any(|n| n.message.contains("cannot show") && n.level == UiNoticeLevel::Warning)
    );
    assert!(notices.iter().any(|n| n.message == "heads up"));

    // A wrong answer comes back refused with the engine's reason; the question stays.
    env.backend.request(BackendRequest::UiRespond {
        conversation: conv,
        generation: 1,
        id: "q1".into(),
        answer: UiAnswer::Choice("zzz".into()),
    });
    let EventKind::UiRespondRefused { id, reason } =
        event_where(&env, |k| matches!(k, EventKind::UiRespondRefused { .. })).kind
    else {
        unreachable!()
    };
    assert_eq!(
        (id.as_str(), reason.as_str()),
        ("q1", "That is not one of the choices.")
    );
    // A good one is taken, and the engine's state then says the question is over.
    env.backend.request(BackendRequest::UiRespond {
        conversation: conv,
        generation: 1,
        id: "q1".into(),
        answer: UiAnswer::Choice("a".into()),
    });
    let (requests, ..) = ui_state(&env);
    assert!(requests.is_empty());
    env.backend.request(BackendRequest::UiCancel {
        conversation: conv,
        generation: 1,
        id: "q9".into(),
    });
    wait_until("cancel sent", || {
        calls.lock().unwrap().iter().any(|(c, _)| c == "cancel q9")
    });
    let log = calls.lock().unwrap();
    assert_eq!(
        log[0],
        ("respond q1".to_owned(), json!("zzz")),
        "the answer goes as the plain value"
    );
}

#[test]
fn an_engine_without_extension_questions_just_has_none() {
    let env = start(mock(&[("s", 1)], vec![("s", view(&["a"]))]), true);
    let conv = env.conversation(0);
    env.open(conv, 1);
    event_where(&env, |k| matches!(k, EventKind::Opened { .. }));
    env.backend.request(BackendRequest::UiRespond {
        conversation: conv,
        generation: 1,
        id: "q1".into(),
        answer: UiAnswer::Confirm(true),
    });
    let EventKind::UiRespondRefused { reason, .. } =
        event_where(&env, |k| matches!(k, EventKind::UiRespondRefused { .. })).kind
    else {
        unreachable!()
    };
    assert!(!reason.is_empty());
}

#[test]
fn a_ten_thousand_message_history_pages_in_completely_with_each_page_bounded() {
    let all: Vec<Value> = (1..=10_000)
        .map(|n| user_entry(n, &format!("message {n}")))
        .collect();
    let (pi, calls) = history_mock(&[("s", 1)], view_of(all[9_995..].to_vec()), all);
    let env = start(pi, true);
    let conv = env.conversation(0);
    env.open(conv, 1);
    let EventKind::Opened {
        items, has_older, ..
    } = event_where(&env, |k| matches!(k, EventKind::Opened { .. })).kind
    else {
        unreachable!()
    };
    assert!(has_older);
    let mut oldest = items[0].id;
    let mut seen = std::collections::HashSet::new();
    let mut pages = 0;
    let mut largest = 0;
    loop {
        env.backend.request(BackendRequest::LoadOlder {
            conversation: conv,
            generation: 1,
            before: Some(oldest),
        });
        let EventKind::OlderPage { items, has_older } =
            event_where(&env, |k| matches!(k, EventKind::OlderPage { .. })).kind
        else {
            unreachable!()
        };
        largest = largest.max(items.len());
        for item in &items {
            assert!(seen.insert(item.id), "item {:?} arrived twice", item.id);
        }
        pages += 1;
        if let Some(first) = items.first() {
            oldest = first.id;
        }
        if !has_older {
            break;
        }
    }
    assert_eq!(seen.len(), 9_995, "every older message, once");
    assert_eq!(pages, 9_995_usize.div_ceil(50));
    assert!(largest <= 50, "a page carried {largest} items");
    let asked = calls.lock().unwrap();
    assert!(
        asked.iter().all(|(_, limit)| *limit <= 50),
        "no request for more than a page"
    );
}

// ------------------------------------------------------------ ownership by the built-in engine

#[test]
fn changes_to_a_conversation_the_built_in_engine_owns_are_refused_before_reaching_pi() {
    use std::sync::atomic::{AtomicU8, Ordering};
    let (pi, agent, _) = agent_mock(&[("s", 1)], vec![("s", view(&[]))]);
    // 0: Pi owns it; 1: the built-in engine owns it; 2: the record cannot be read.
    let owner = Arc::new(AtomicU8::new(1));
    let check = {
        let owner = owner.clone();
        super::OwnershipCheck(Arc::new(move |_| match owner.load(Ordering::SeqCst) {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err("database is locked".into()),
        }))
    };
    let env = start_configured(pi, true, 0o700, |config| {
        config.owned_elsewhere = Some(check)
    });
    let conv = env.open_first();
    assert_eq!(conv, ConversationId(pipkin_core::stable_id("s")));
    let to_pi_before = env
        .pi
        .requests()
        .iter()
        .filter(|(_, c)| c.service_id == "pi.agent-controller")
        .count();

    // A prompt gets a definite rejection with the reason: never an unknown outcome.
    submit(&env, conv, 1, 7, "req-1", "continue here");
    assert_eq!(
        env.next_for(7).kind,
        EventKind::Rejected {
            reason: pipkin_core::NATIVE_OWNED_NOTICE.into()
        }
    );
    queue(&env, conv, "q-1", pipkin_core::QueueMode::Steer, "steer");
    assert_eq!(
        queue_event(&env, "q-1"),
        EventKind::QueueRefused {
            request: RequestId("q-1".into()),
            reason: pipkin_core::NATIVE_OWNED_NOTICE.into()
        }
    );
    env.backend.request(BackendRequest::UiRespond {
        conversation: conv,
        generation: 1,
        id: "q1".into(),
        answer: pipkin_core::UiAnswer::Confirm(true),
    });
    let refused = loop {
        let event = env.next_event();
        if let EventKind::UiRespondRefused { id, reason } = event.kind {
            break (id, reason);
        }
    };
    assert_eq!(
        refused,
        ("q1".into(), pipkin_core::NATIVE_OWNED_NOTICE.into())
    );
    for request in [
        BackendRequest::SetModel {
            conversation: conv,
            generation: 1,
            model: "openai/gpt".into(),
        },
        BackendRequest::SetThinkingLevel {
            conversation: conv,
            generation: 1,
            level: "high".into(),
        },
        BackendRequest::SetSubagentsEnabled {
            conversation: conv,
            generation: 1,
            enabled: true,
        },
        BackendRequest::CancelQueued {
            conversation: conv,
            generation: 1,
            entry: pipkin_core::QueueId(3),
        },
    ] {
        env.lifecycle.lock().unwrap().clear();
        env.backend.request(request);
        env.wait_notice("continues in the built-in engine");
    }
    // An unreadable record is a reason not to send, said as such.
    owner.store(2, Ordering::SeqCst);
    submit(&env, conv, 1, 8, "req-2", "continue here");
    let EventKind::Rejected { reason } = env.next_for(8).kind else {
        panic!("a definite rejection")
    };
    assert!(
        reason.contains("could not confirm which engine owns"),
        "{reason}"
    );

    // Nothing reached Pi's agent controller.
    {
        let agent = agent.lock().unwrap();
        assert!(agent.prompts.is_empty() && agent.queued.is_empty());
        assert!(agent.selects.is_empty() && agent.thinking_selections.is_empty());
        assert!(agent.cancelled.is_empty());
    }
    assert_eq!(
        env.pi
            .requests()
            .iter()
            .filter(|(_, c)| c.service_id == "pi.agent-controller")
            .count(),
        to_pi_before
    );

    // Stopping is safe: it still goes to Pi while the built-in engine owns the conversation.
    owner.store(1, Ordering::SeqCst);
    env.backend.request(BackendRequest::Cancel {
        conversation: conv,
        generation: 1,
        op: OperationId(9),
    });
    wait_until("the stop reached Pi", || {
        env.pi
            .requests()
            .iter()
            .filter(|(_, c)| c.service_id == "pi.agent-controller")
            .count()
            > to_pi_before
    });

    // Released back to Pi: the same prompt goes through.
    owner.store(0, Ordering::SeqCst);
    submit(&env, conv, 1, 10, "req-3", "continue in Pi");
    assert_eq!(env.next_for(10).kind, EventKind::Accepted);
    assert_eq!(agent.lock().unwrap().prompts.len(), 1);
}

#[test]
fn an_owned_conversation_is_refused_with_its_reason_even_while_pi_is_unreachable() {
    let env = start_configured(MockPi::default(), false, 0o700, |config| {
        config.owned_elsewhere = Some(super::OwnershipCheck(Arc::new(|_| Ok(true))))
    });
    submit(&env, ConversationId(4), 2, 3, "req-1", "x");
    let event = env.next_for(3);
    assert_eq!(
        event.kind,
        EventKind::Rejected {
            reason: pipkin_core::NATIVE_OWNED_NOTICE.into()
        },
        "the ownership reason, not \"not connected\""
    );
}
