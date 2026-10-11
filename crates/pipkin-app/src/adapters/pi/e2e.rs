//! End-to-end tests against a REAL Pi engine with a scripted provider. Opt-in:
//!
//! ```text
//! PIPKIN_PI_REPO=<pi checkout> cargo test -p pipkin-app e2e -- --ignored --nocapture
//! ```
//!
//! The engine, its coding tools, its durable transcript and its session storage are real; only
//! the model's answers are scripted. A harness drives the real core state machine, the real
//! `PiBackend`, and the real SQLite journal. A fault proxy between them can drop the engine's
//! reply to a prompt (or the prompt itself) and sever the connection.

use std::collections::{HashSet, VecDeque};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use pi_client::cbor::{self, Limits};
use pi_client::chord::parse_service_call;
use pi_client::frame::{DEFAULT_MAX_FRAME_LENGTH, FrameDecoder, encode_frame};
use pi_client::protocol::{
    ClientMessage, ServerMessage, parse_client_message, parse_server_message,
};
use pipkin_core::{
    AppState, Backend, BackendEvent, Bootstrap, ChangesState, Command, Connection, ConversationId,
    Delivery, Effect, ItemKind, LifecycleEvent, Mode, Outcome, QueueMode, RequestId, RunState,
    ToolStatus, describe_attachment,
};
use serde_json::{Value, json};

use super::engine::{EngineHost, profile_pids};
use super::testsupport::{Fixture, Gate, Prepared, Reply, is_tool_result_turn, prepare};
use super::{PiBackend, PiConfig};
use crate::controller::Restore;
use crate::storage::Storage;

const NAMESPACE: &str = "pi:e2e";
const WAIT: Duration = Duration::from_secs(90);

/// The model's script: edit a tracked file, create a new one, then say what it did.
fn two_edits(request: &Value, n: usize) -> Reply {
    match n {
        0 => Reply::Tool {
            lead: Some("Updating the notes. ".into()),
            name: "write".into(),
            args: json!({ "path": "notes.txt", "content": "one\ntwo\n" }),
        },
        1 => Reply::Tool {
            lead: None,
            name: "write".into(),
            args: json!({ "path": "hello.txt", "content": "hello from the stub\n" }),
        },
        _ => {
            assert!(
                is_tool_result_turn(request),
                "the final turn shows a tool result"
            );
            Reply::Text("Done: updated notes.txt and created hello.txt.".into())
        }
    }
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Process::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .status()
        .expect("git runs")
        .success();
    assert!(ok, "git {args:?}");
}

/// A project with one committed file, so edits show up as a real diff.
fn init_project(project: &Path) {
    git(project, &["init", "-q", "-b", "main"]);
    std::fs::write(project.join("notes.txt"), "one\n").unwrap();
    git(project, &["add", "-A"]);
    git(project, &["commit", "-q", "-m", "init"]);
}

// --------------------------------------------------------------------------- the harness

/// The application as the controller composes it, minus the window: real state machine, real
/// backend, real SQLite storage, with effects executed the way the controller executes them.
struct Harness {
    state: AppState,
    backend: PiBackend,
    storage: Arc<Storage>,
    restore: Restore,
    events: async_channel::Receiver<BackendEvent>,
    life: Arc<Mutex<VecDeque<LifecycleEvent>>>,
    acks: mpsc::Receiver<(ConversationId, RequestId, Result<(), String>)>,
    ack_tx: mpsc::Sender<(ConversationId, RequestId, Result<(), String>)>,
    notices: Vec<String>,
    /// What the application would have put on the clipboard, saved to a file, or launched.
    copied: Vec<String>,
    saved: Vec<(String, String)>,
    launches: Vec<pipkin_core::Launch>,
}

fn unique(prefix: &str) -> String {
    static N: AtomicUsize = AtomicUsize::new(0);
    format!("{prefix}{}", N.fetch_add(1, Ordering::SeqCst))
}

impl Harness {
    fn start(config: PiConfig, db: &Path) -> Harness {
        Harness::start_in(config, db, NAMESPACE)
    }

    fn start_in(config: PiConfig, db: &Path, namespace: &str) -> Harness {
        let storage = Arc::new(Storage::open(db, namespace).expect("storage"));
        let loaded = storage.load_all().expect("load");
        let mut state = AppState::new(
            Bootstrap {
                projects: vec![],
                models: vec![],
                conversations: vec![],
                now: 1,
            },
            loaded.prefs.clone(),
        );
        state.mode = Mode::Real;
        state.can_create = true;
        state.set_request_prefix(unique("e2e"));
        state.set_connection(Connection::Connecting);
        for path in &loaded.projects {
            state.restore_project(path);
        }
        // Conversations with a saved copy are known before (and without) the engine's list.
        for saved in storage.cached_conversations().expect("saved conversations") {
            state.restore_cached_conversation(
                saved.id,
                &saved.meta.project_path,
                saved.meta.title,
                saved.meta.updated_at,
            );
        }
        let restore = Restore::new(loaded, false);
        let (tx, events) = async_channel::unbounded();
        let backend = PiBackend::new(tx, config);
        let life = Arc::new(Mutex::new(VecDeque::new()));
        let sink = {
            let life = life.clone();
            Box::new(move |e| life.lock().unwrap().push_back(e))
        };
        backend.start(sink);
        let (ack_tx, acks) = mpsc::channel();
        let mut harness = Harness {
            state,
            backend,
            storage,
            restore,
            events,
            life,
            acks,
            ack_tx,
            notices: vec![],
            copied: vec![],
            saved: vec![],
            launches: vec![],
        };
        // As the controller does: conversations known from saved copies are shown at once.
        let mut out = harness.restore.apply(&mut harness.state);
        out.merge(harness.state.select_initial());
        harness.exec(out);
        harness
    }

    fn exec(&mut self, outcome: Outcome) {
        for effect in outcome.effects {
            match effect {
                Effect::Backend(request) => self.backend.request(request),
                Effect::SaveDraft {
                    conversation,
                    text,
                    attachments,
                    rev,
                } => {
                    // Really written, so a restart restores it; waited for, so the test is
                    // deterministic.
                    let (tx, rx) = mpsc::channel();
                    self.storage
                        .save_draft(conversation, text, attachments, rev, move |r| {
                            let _ = tx.send(r);
                        });
                    let result = rx.recv_timeout(Duration::from_secs(10)).unwrap();
                    self.state.draft_saved(conversation, rev, result);
                }
                Effect::SavePrefs(prefs) => self.storage.save_prefs(&prefs),
                Effect::CompleteSetup(prefs) => self.storage.save_prefs(&prefs),
                Effect::SaveGoal {
                    conversation,
                    text,
                    paused,
                } => self.storage.save_goal(conversation, text, paused),
                Effect::SaveProject { path } => self.storage.save_project(path),
                Effect::SaveConversation { .. } => {}
                Effect::SaveCache { conversation } => {
                    if let Some(c) = self.state.conversation(conversation)
                        && let Some(project) =
                            self.state.projects.iter().find(|p| p.id == c.project)
                    {
                        let meta = crate::storage::CacheMeta {
                            title: c.title.clone(),
                            project_path: project.path.clone(),
                            updated_at: c.updated_at,
                        };
                        self.storage.save_cache(
                            conversation,
                            meta,
                            &c.items,
                            c.has_older,
                            self.state.now(),
                        );
                    }
                }
                Effect::LoadCache { conversation } => {
                    if let Ok(Some(copy)) = self.storage.load_cache(conversation) {
                        let out = self.state.apply_cache(
                            conversation,
                            copy.items,
                            copy.has_older,
                            copy.synced_at,
                        );
                        self.exec(out);
                    }
                }
                Effect::SearchHistory { query } => {
                    if let Ok(results) = self.storage.search(&query, 40) {
                        let out = self.state.apply_search_results(results);
                        self.exec(out);
                    }
                }
                Effect::CopyText(text) => self.copied.push(text),
                Effect::SaveText {
                    suggested_name,
                    text,
                } => self.saved.push((suggested_name, text)),
                Effect::Launch(what) => self.launches.push(what),
                Effect::Import(_) => {}
                Effect::JournalIntent {
                    conversation,
                    request,
                    text,
                    attachments,
                    model,
                    ..
                } => {
                    let tx = self.ack_tx.clone();
                    let key = request.clone();
                    self.storage.journal_intent(
                        conversation,
                        request,
                        &text,
                        &attachments,
                        model.as_deref(),
                        move |r| {
                            let _ = tx.send((conversation, key, r));
                        },
                    );
                }
                Effect::JournalState { request, state, .. } => {
                    self.storage.journal_state(request, state)
                }
            }
        }
    }

    fn dispatch(&mut self, command: Command) {
        let outcome = self.state.dispatch(command);
        self.exec(outcome);
    }

    /// Apply everything that has arrived, once.
    fn pump(&mut self) {
        let lifecycle: Vec<LifecycleEvent> = self.life.lock().unwrap().drain(..).collect();
        for event in lifecycle {
            let outcome = match event {
                LifecycleEvent::Connection(c) => self.state.set_connection(c),
                LifecycleEvent::Catalog(boot) => {
                    let mut out = self.state.apply_catalog(boot);
                    out.merge(self.restore.apply(&mut self.state));
                    out.merge(self.state.select_initial());
                    out
                }
                LifecycleEvent::ModelSelected(m) => self.state.set_engine_model(m),
                LifecycleEvent::SignIn(_) | LifecycleEvent::SetupSaved(_) => Outcome::default(),
                LifecycleEvent::Notice(message) => {
                    self.notices.push(message.clone());
                    self.state.set_notice(message)
                }
                LifecycleEvent::StorageIssue(message) => self.state.set_storage_issue(message),
            };
            self.exec(outcome);
        }
        while let Ok(event) = self.events.try_recv() {
            let outcome = self.state.apply_event(event);
            self.exec(outcome);
        }
        while let Ok((conversation, request, result)) = self.acks.try_recv() {
            let outcome = self.state.intent_persisted(conversation, &request, result);
            self.exec(outcome);
        }
    }

    fn until(&mut self, what: &str, done: impl Fn(&AppState) -> bool) {
        let deadline = Instant::now() + WAIT;
        loop {
            self.pump();
            if done(&self.state) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for: {what}\n  connection: {:?}\n  notices: {:?}\n  run: {:?}",
                self.state.connection,
                self.notices,
                self.state.current().map(|c| c.run.clone()),
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Open `project` as a project, ask for a conversation in it, and wait until it is open.
    fn open_new_conversation(&mut self, project: &Path) {
        self.until("connected", |s| s.connection.is_ready());
        // The engine resolves a session's directory through symlinks, so open the real path.
        let real = std::fs::canonicalize(project).unwrap();
        self.dispatch(Command::AddProject(real.display().to_string()));
        self.dispatch(Command::NewConversation);
        self.until("the new conversation is open", |s| {
            s.current().is_some_and(|c| c.opened)
        });
    }

    /// Keep the application running until the engine has made `n` model requests.
    fn until_asked(&mut self, provider: &super::testsupport::StubProvider, n: usize) {
        self.until("the engine asked the model", |_| {
            provider.requests().len() >= n
        });
    }

    fn send(&mut self, text: &str) {
        self.dispatch(Command::EditDraft(text.into()));
        self.dispatch(Command::Submit);
    }

    fn user_messages(&self) -> Vec<String> {
        self.state
            .current()
            .unwrap()
            .items
            .iter()
            .filter_map(|i| match &i.kind {
                ItemKind::User { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// An orderly stop, as the application's quit does.
    fn stop(self) {
        self.backend.shutdown();
        self.storage.shutdown();
    }
}

fn direct_config(server_dir: &Path, server_id: &str) -> PiConfig {
    let mut config = PiConfig::new(server_dir.to_path_buf());
    config.server_id = Some(server_id.to_owned());
    config.retry_delay = Duration::from_millis(200);
    config
}

// ------------------------------------------------------------------------- the fault proxy

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    None,
    /// Let the prompt reach the engine, then drop its reply and sever the connection.
    DropNextPromptReply,
    /// Sever the connection instead of delivering the prompt to the engine.
    DropNextPromptRequest,
    /// Let the next steer or follow-up reach the engine, then drop its reply and sever.
    DropNextQueueReply,
    /// Sever the connection instead of delivering the next steer or follow-up.
    DropNextQueueRequest,
    /// Lose a stop request before it reaches Pi; the run must not appear settled.
    DropNextStopRequest,
}

type Sever = Arc<dyn Fn() + Send + Sync>;

struct ProxyState {
    fault: Fault,
    prompt_ids: HashSet<String>,
    queue_ids: HashSet<String>,
    fired: usize,
    /// Every live connection, so a test can cut them all at once.
    severs: Vec<Sever>,
}

/// A Unix-socket proxy in front of the engine that understands the wire well enough to find a
/// prompt call and its reply. Everything else is forwarded byte for byte.
struct FaultProxy {
    dir: PathBuf,
    state: Arc<Mutex<ProxyState>>,
    stop: Arc<AtomicBool>,
}

fn is_prompt(call: &Value) -> bool {
    parse_service_call(call)
        .is_ok_and(|c| c.service_id == "pi.agent-controller" && c.member == "prompt")
}

fn is_queue(call: &Value) -> bool {
    parse_service_call(call).is_ok_and(|c| {
        c.service_id == "pi.agent-controller" && matches!(c.member.as_str(), "steer" | "followUp")
    })
}

impl FaultProxy {
    fn start(upstream: PathBuf, dir: PathBuf, server_id: &str) -> FaultProxy {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = dir.join(format!("{server_id}.sock"));
        let listener = UnixListener::bind(&socket).unwrap();
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        let state = Arc::new(Mutex::new(ProxyState {
            fault: Fault::None,
            prompt_ids: HashSet::new(),
            queue_ids: HashSet::new(),
            fired: 0,
            severs: Vec::new(),
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let (s, st) = (state.clone(), stop.clone());
        thread::spawn(move || {
            for client in listener.incoming().flatten() {
                if st.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(engine) = UnixStream::connect(&upstream) else {
                    continue;
                };
                Self::pipe(client, engine, s.clone());
            }
        });
        FaultProxy { dir, state, stop }
    }

    fn arm(&self, fault: Fault) {
        self.state.lock().unwrap().fault = fault;
    }

    fn fired(&self) -> usize {
        self.state.lock().unwrap().fired
    }

    /// Cut every connection through the proxy, as a network failure would.
    fn sever_all(&self) {
        let severs = std::mem::take(&mut self.state.lock().unwrap().severs);
        for sever in severs {
            sever();
        }
    }

    fn pipe(client: UnixStream, engine: UnixStream, state: Arc<Mutex<ProxyState>>) {
        let sever: Sever = {
            let (c, e) = (client.try_clone().unwrap(), engine.try_clone().unwrap());
            Arc::new(move || {
                let _ = c.shutdown(std::net::Shutdown::Both);
                let _ = e.shutdown(std::net::Shutdown::Both);
            })
        };
        state.lock().unwrap().severs.push(sever.clone());
        // client -> engine
        {
            let (mut from, mut to, state, sever) = (
                client.try_clone().unwrap(),
                engine.try_clone().unwrap(),
                state.clone(),
                sever.clone(),
            );
            thread::spawn(move || {
                let mut frames = FrameDecoder::new(DEFAULT_MAX_FRAME_LENGTH);
                let mut buf = [0u8; 16 * 1024];
                loop {
                    let n = match from.read(&mut buf) {
                        Ok(0) | Err(_) => return sever(),
                        Ok(n) => n,
                    };
                    let Ok(payloads) = frames.push(&buf[..n]) else {
                        return sever();
                    };
                    for payload in payloads {
                        if let Ok(value) = cbor::decode(&payload, &Limits::default())
                            && let Ok(ClientMessage::Request { id, call, .. }) =
                                parse_client_message(&value)
                        {
                            let mut s = state.lock().unwrap();
                            if is_prompt(&call) {
                                if s.fault == Fault::DropNextPromptRequest {
                                    s.fault = Fault::None;
                                    s.fired += 1;
                                    drop(s);
                                    return sever();
                                }
                                s.prompt_ids.insert(id);
                            } else if is_queue(&call) {
                                if s.fault == Fault::DropNextQueueRequest {
                                    s.fault = Fault::None;
                                    s.fired += 1;
                                    drop(s);
                                    return sever();
                                }
                                s.queue_ids.insert(id);
                            } else if parse_service_call(&call).is_ok_and(|c| {
                                c.service_id == "pi.agent-controller" && c.member == "abort"
                            }) && s.fault == Fault::DropNextStopRequest
                            {
                                s.fault = Fault::None;
                                s.fired += 1;
                                drop(s);
                                return sever();
                            }
                        }
                        let frame = encode_frame(&payload).unwrap();
                        if to.write_all(&frame).is_err() {
                            return sever();
                        }
                    }
                }
            });
        }
        // engine -> client
        thread::spawn(move || {
            let (mut from, mut to) = (engine, client);
            let mut frames = FrameDecoder::new(DEFAULT_MAX_FRAME_LENGTH);
            let mut buf = [0u8; 16 * 1024];
            loop {
                let n = match from.read(&mut buf) {
                    Ok(0) | Err(_) => return sever(),
                    Ok(n) => n,
                };
                let Ok(payloads) = frames.push(&buf[..n]) else {
                    return sever();
                };
                for payload in payloads {
                    if let Ok(value) = cbor::decode(&payload, &Limits::default())
                        && let Ok(ServerMessage::Response { id, .. }) = parse_server_message(&value)
                    {
                        let mut s = state.lock().unwrap();
                        let drop_reply = (s.fault == Fault::DropNextPromptReply
                            && s.prompt_ids.contains(&id))
                            || (s.fault == Fault::DropNextQueueReply && s.queue_ids.contains(&id));
                        if drop_reply {
                            s.fault = Fault::None;
                            s.fired += 1;
                            drop(s);
                            return sever();
                        }
                    }
                    let frame = encode_frame(&payload).unwrap();
                    if to.write_all(&frame).is_err() {
                        return sever();
                    }
                }
            }
        });
    }
}

impl Drop for FaultProxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = UnixStream::connect(self.dir.join("wake"));
    }
}

// --------------------------------------------------------------------------------- the tests

fn nothing(_: &Value, _: usize) -> Reply {
    Reply::Text("ok".into())
}

fn handshake(dir: &Path, id: &str) -> bool {
    use pi_client::client::ClientOptions;
    pi_client::unix::connect(&dir.join(format!("{id}.sock")), ClientOptions::new(id)).is_ok()
}

#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn the_engine_starts_serves_and_stops_without_leaving_processes() {
    let mut fx = Fixture::start(nothing);
    assert!(handshake(&fx.server_dir, &fx.server_id), "{}", fx.log());
    assert!(!profile_pids(&fx.server_dir, &fx.server_id).is_empty());
    let (dir, id) = (fx.server_dir.clone(), fx.server_id.clone());
    fx.host.stop();
    // Pi's coordinator and workers are detached; every one of them must be gone.
    assert_eq!(
        profile_pids(&dir, &id),
        Vec::<u32>::new(),
        "orphaned engine processes"
    );
    assert!(!handshake(&dir, &id), "nothing should answer after stop");
    fx.host.stop(); // stopping twice is harmless
}

#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn workspace_scan_failure_keeps_the_last_diff_and_retry_recovers_without_a_prompt() {
    let fx = Fixture::start(nothing);
    init_project(&fx.project);
    let mut h = Harness::start(
        direct_config(&fx.server_dir, &fx.server_id),
        &fx.root.join("app.sqlite3"),
    );
    h.open_new_conversation(&fx.project);
    assert!(h.state.current().unwrap().changes.is_empty());
    std::fs::write(fx.project.join("notes.txt"), "one\ntwo\n").unwrap();
    h.dispatch(Command::RefreshChanges);
    h.until("a successful project scan", |s| {
        s.current()
            .is_some_and(|c| c.changes_state == ChangesState::Ready && c.changes.len() == 1)
    });
    let snapshot = h.state.current().unwrap().changes.clone();
    let selection = h.state.current().unwrap().selected_change;
    let moved = fx.root.join("moved-project");
    std::fs::rename(&fx.project, &moved).unwrap();
    h.dispatch(Command::RefreshChanges);
    h.until("the scan reports the missing project", |s| {
        s.current()
            .is_some_and(|c| matches!(c.changes_state, ChangesState::Unavailable(_)))
    });
    assert_eq!(h.state.current().unwrap().changes, snapshot);
    assert_eq!(h.state.current().unwrap().selected_change, selection);
    assert!(h.state.availability().refresh_changes);

    std::fs::rename(&moved, &fx.project).unwrap();
    std::fs::write(fx.project.join("notes.txt"), "one\nnew\n").unwrap();
    h.dispatch(Command::RefreshChanges);
    h.until("retry refreshes the diff", |s| {
        s.current()
            .is_some_and(|c| c.changes_state == ChangesState::Ready && c.changes != snapshot)
    });
    assert_eq!(
        h.state.current().unwrap().changes[0].added,
        snapshot[0].added
    );
    std::fs::rename(fx.project.join(".git"), fx.root.join("saved-git")).unwrap();
    h.dispatch(Command::RefreshChanges);
    h.until("the non-Git folder is explicit", |s| {
        s.current()
            .is_some_and(|c| c.changes_state == ChangesState::NotARepository)
    });
    assert!(h.state.current().unwrap().changes.is_empty());
    assert!(
        fx.provider.requests().is_empty(),
        "scanning must not send a prompt"
    );
}

/// M2's exit criterion, first half: in a temporary project a real engine with a scripted
/// provider executes file-changing tools; the app shows the tool output and the real diff; the
/// same history comes back when it is reopened.
#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn a_real_run_edits_files_shows_output_and_diff_and_reopens_the_same_history() {
    let fx = Fixture::start(two_edits);
    init_project(&fx.project);
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(direct_config(&fx.server_dir, &fx.server_id), &db);
    h.open_new_conversation(&fx.project);

    // The conversation was created in the project's directory, through the real engine.
    let real = std::fs::canonicalize(&fx.project)
        .unwrap()
        .display()
        .to_string();
    let project = h.state.current_project().expect("a project").clone();
    assert_eq!(project.path, real);
    assert_eq!(project.name, "project");
    // The engine, not a local preference, says which model is selected.
    assert_eq!(h.state.prefs.model.as_deref(), Some("stub/scripted"));
    assert!(h.state.models.iter().any(|m| m.id == "stub/scripted"));
    h.until("the engine reports supported effort levels", |s| {
        s.current().is_some_and(|c| !c.thinking_levels.is_empty())
    });
    assert_eq!(h.state.current().unwrap().thinking_levels, ["off"]);

    h.send("please update the notes and add a greeting");
    h.until("the run completes", |s| {
        s.current().is_some_and(|c| c.run == RunState::Idle)
            && s.current().unwrap().items.iter().any(
                |i| matches!(&i.kind, ItemKind::Assistant { text, .. } if text.contains("Done:")),
            )
            && s.current().unwrap().changes.len() == 2
    });

    // The files really changed, in the project.
    assert_eq!(
        std::fs::read_to_string(fx.project.join("notes.txt")).unwrap(),
        "one\ntwo\n"
    );
    assert_eq!(
        std::fs::read_to_string(fx.project.join("hello.txt")).unwrap(),
        "hello from the stub\n"
    );

    // The transcript shows the real exchange: the prompt, both tool calls with their output, the answer.
    let conv = h.state.current().unwrap();
    let tools: Vec<_> = conv
        .items
        .iter()
        .filter_map(|i| {
            if let ItemKind::Tool(t) = &i.kind {
                Some(t)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(tools.len(), 2, "{:?}", conv.items);
    assert!(
        tools
            .iter()
            .all(|t| t.name == "write" && t.status == ToolStatus::Ok),
        "{tools:?}"
    );
    assert!(tools[0].input.contains("notes.txt") && tools[1].input.contains("hello.txt"));
    assert!(
        tools[0].output.contains("Successfully wrote"),
        "{}",
        tools[0].output
    );
    assert_eq!(
        h.user_messages(),
        ["please update the notes and add a greeting"]
    );
    assert!(matches!(
        &conv.items.last().unwrap().kind,
        ItemKind::Assistant { text, streaming: false } if text == "Done: updated notes.txt and created hello.txt."
    ));
    assert!(conv.items.iter().any(|i| matches!(
        &i.kind,
        ItemKind::User {
            delivery: Delivery::Sent,
            ..
        }
    )));

    // The workspace diff is real: a modified tracked file and a new untracked one.
    let names: Vec<&str> = conv.changes.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(names, ["notes.txt", "hello.txt"]);
    assert_eq!((conv.changes[0].added, conv.changes[0].removed), (1, 0));
    assert_eq!((conv.changes[1].added, conv.changes[1].removed), (1, 0));

    // The model was called three times: two tool turns and the final answer.
    let requests = fx.provider.requests();
    assert_eq!(requests.len(), 3);
    assert!(is_tool_result_turn(&requests[1]) && is_tool_result_turn(&requests[2]));

    // Nothing is left unresolved in the journal: the request ran to completion.
    h.storage.shutdown();
    let loaded = Storage::open(&db, NAMESPACE).unwrap().load_all().unwrap();
    assert!(
        loaded.open_requests.is_empty(),
        "{:?}",
        loaded.open_requests
    );
    let items_before = h.state.current().unwrap().items.clone();
    let conversation = h.state.current().unwrap().id;
    h.backend.shutdown();

    // Reopen from scratch: a new application instance against the same engine shows the same history.
    let mut again = Harness::start(direct_config(&fx.server_dir, &fx.server_id), &db);
    again.until("the conversation reopens", |s| {
        s.selected == Some(conversation) && s.current().is_some_and(|c| c.opened)
    });
    let reopened = again.state.current().unwrap();
    assert_eq!(reopened.items.len(), items_before.len());
    assert_eq!(
        again.user_messages(),
        ["please update the notes and add a greeting"]
    );
    assert_eq!(
        reopened.items.iter().map(|i| i.id).collect::<Vec<_>>(),
        items_before.iter().map(|i| i.id).collect::<Vec<_>>(),
        "the same entries keep the same ids across a reopen"
    );
    again.until("the workspace changes load", |s| {
        s.current().unwrap().changes.len() == 2
    });
    // A fresh conversation in the same dirty repository must not start with the
    // previous session's workspace diff; returning to the old one retains its view.
    again.dispatch(Command::NewConversation);
    again.until("new conversation opens", |s| {
        s.selected != Some(conversation) && s.current().is_some_and(|c| c.opened)
    });
    thread::sleep(Duration::from_millis(150));
    assert!(again.state.current().unwrap().changes.is_empty());
    again.dispatch(Command::SelectConversation(conversation));
    again.until("old conversation's changes return", |s| {
        s.current()
            .is_some_and(|c| c.id == conversation && c.changes.len() == 2)
    });
    again.stop();
}

/// Everything shared by the two "drop the acknowledgment" scenarios.
struct Fault2 {
    fx: Fixture,
    proxy: FaultProxy,
    db: PathBuf,
}

fn fault_setup() -> Fault2 {
    let fx = Fixture::start(two_edits);
    init_project(&fx.project);
    let proxy = FaultProxy::start(
        fx.server_dir.join(format!("{}.sock", fx.server_id)),
        fx.root.join("proxy"),
        &fx.server_id,
    );
    let db = fx.root.join("app.sqlite3");
    Fault2 { fx, proxy, db }
}

/// M2's exit criterion, second half, case 1: the engine accepted the prompt and ran it, but the
/// acknowledgment never reached the app, and the app was killed. After relaunch the outcome is
/// unknown, never resent; asking the engine finds the original, so the prompt ran exactly once.
#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn a_dropped_acknowledgment_then_a_relaunch_never_runs_the_prompt_twice() {
    let f = fault_setup();
    let mut h = Harness::start(direct_config(&f.proxy.dir, &f.fx.server_id), &f.db);
    h.open_new_conversation(&f.fx.project);
    let conversation = h.state.current().unwrap().id;

    f.proxy.arm(Fault::DropNextPromptReply);
    h.send("update the notes");
    h.until("the outcome is unknown", |s| {
        matches!(
            s.current().map(|c| &c.run),
            Some(RunState::OutcomeUnknown { .. })
        )
    });
    assert_eq!(f.proxy.fired(), 1);
    // The core refuses to resend on its own: submit is off, retry is off, only a status check.
    let a = h.state.availability();
    assert!(!a.submit && !a.retry && a.check_status);

    // The engine, unaware of any of this, finishes the run it accepted.
    let deadline = Instant::now() + WAIT;
    while f.fx.provider.requests().len() < 3 || !f.fx.project.join("hello.txt").exists() {
        assert!(
            Instant::now() < deadline,
            "the engine never finished the run it accepted"
        );
        thread::sleep(Duration::from_millis(50));
    }

    // Kill the application. The journal already holds the intent; nothing else was saved.
    h.stop();

    // Relaunch against the engine directly. The unresolved request is restored as unknown.
    let mut h2 = Harness::start(direct_config(&f.fx.server_dir, &f.fx.server_id), &f.db);
    h2.until("the conversation reopens as unknown", |s| {
        s.selected == Some(conversation)
            && s.current()
                .is_some_and(|c| c.opened && matches!(c.run, RunState::OutcomeUnknown { .. }))
    });
    assert_eq!(
        f.fx.provider.requests().len(),
        3,
        "relaunching resent nothing"
    );
    assert!(
        !h2.state.availability().submit,
        "no automatic resubmission is offered"
    );

    // The user asks. The engine finds the original submission by its key.
    h2.dispatch(Command::CheckStatus);
    h2.until("the status resolves and the run settles", |s| {
        s.current()
            .is_some_and(|c| c.run == RunState::Idle && c.last_submission.is_some())
    });
    assert_eq!(h2.user_messages(), ["update the notes"], "one prompt, once");
    assert!(
        h2.state.current().unwrap().items.iter().any(|i| matches!(
            &i.kind,
            ItemKind::User {
                delivery: Delivery::Sent,
                ..
            }
        )),
        "the message is shown as delivered"
    );
    assert_eq!(
        f.fx.provider.requests().len(),
        3,
        "the run happened exactly once"
    );
    assert_eq!(
        std::fs::read_to_string(f.fx.project.join("notes.txt")).unwrap(),
        "one\ntwo\n"
    );

    // The journal entry is closed, so a third launch has nothing left to resolve.
    h2.stop();
    let loaded = Storage::open(&f.db, NAMESPACE).unwrap().load_all().unwrap();
    assert!(
        loaded.open_requests.is_empty(),
        "{:?}",
        loaded.open_requests
    );
}

/// Case 2: the prompt never reached the engine (the connection died first), and the app was
/// killed. After relaunch the engine proves it has no such submission, and only then does the
/// user's explicit retry send it, once.
#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn a_lost_prompt_is_proven_unsent_and_only_an_explicit_retry_sends_it() {
    let f = fault_setup();
    let mut h = Harness::start(direct_config(&f.proxy.dir, &f.fx.server_id), &f.db);
    h.open_new_conversation(&f.fx.project);
    let conversation = h.state.current().unwrap().id;

    f.proxy.arm(Fault::DropNextPromptRequest);
    h.send("add a greeting");
    h.until("the outcome is unknown", |s| {
        matches!(
            s.current().map(|c| &c.run),
            Some(RunState::OutcomeUnknown { .. })
        )
    });
    assert_eq!(f.proxy.fired(), 1);
    thread::sleep(Duration::from_millis(500));
    assert_eq!(
        f.fx.provider.requests().len(),
        0,
        "the engine never saw the prompt"
    );
    h.stop();

    let mut h2 = Harness::start(direct_config(&f.fx.server_dir, &f.fx.server_id), &f.db);
    h2.until("reopened as unknown", |s| {
        s.selected == Some(conversation)
            && s.current()
                .is_some_and(|c| c.opened && matches!(c.run, RunState::OutcomeUnknown { .. }))
    });
    h2.dispatch(Command::CheckStatus);
    // The engine has no submission under that key: provably never accepted.
    h2.until("proven not accepted", |s| {
        s.current()
            .is_some_and(|c| matches!(c.run, RunState::Failed { .. }))
    });
    assert!(h2.state.availability().retry, "now a retry is offered");
    assert_eq!(f.fx.provider.requests().len(), 0, "still nothing was run");

    // Only the user's explicit retry sends it, and it runs once.
    h2.dispatch(Command::Retry);
    h2.until("the retried run completes", |s| {
        s.current()
            .is_some_and(|c| c.run == RunState::Idle && c.changes.len() == 2)
    });
    assert_eq!(h2.user_messages(), ["add a greeting"]);
    assert_eq!(f.fx.provider.requests().len(), 3, "exactly one run");
    assert_eq!(
        std::fs::read_to_string(f.fx.project.join("hello.txt")).unwrap(),
        "hello from the stub\n"
    );
    h2.stop();
}

/// Engine lifecycle: the app launches and owns the engine, restarts it promptly when it dies,
/// and stops every process it started on quit.
#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn the_app_owns_the_engine_restarts_it_after_a_crash_and_stops_it_on_quit() {
    let p: Prepared = prepare(nothing);
    let (dir, id) = (p.server_dir.clone(), p.server_id.clone());
    let db = p.root.join("app.sqlite3");
    let mut config = PiConfig::managed(p.config.clone());
    config.retry_delay = Duration::from_millis(200);
    let mut h = Harness::start(config, &db);

    h.until("the app launched the engine and connected", |s| {
        s.connection.is_ready()
    });
    assert!(!profile_pids(&dir, &id).is_empty(), "the engine is running");

    // The engine crashes hard (every process of it, no cleanup); the app notices, restarts it,
    // and reconnects, without waiting out the launcher lock the crash left behind.
    for pid in profile_pids(&dir, &id) {
        // SAFETY: SIGKILL to processes matched by this test's own profile identity.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    }
    let crashed = Instant::now();
    h.until("the app sees the loss", |s| !s.connection.is_ready());
    h.until("the app restarted the engine and reconnected", |s| {
        s.connection.is_ready()
    });
    assert!(
        crashed.elapsed() < Duration::from_secs(20),
        "a restart after a crash took {:?}; the stale launcher lock was not cleared",
        crashed.elapsed()
    );

    // Quit: every process the app started is stopped, none orphaned.
    h.stop();
    assert_eq!(
        profile_pids(&dir, &id),
        Vec::<u32>::new(),
        "orphaned engine processes after quit"
    );
    drop(p.provider);
}

/// A crash loop is capped: an engine that cannot start is retried a few times, then the app stops
/// trying and says why, with the engine's own output, instead of spinning.
#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn an_engine_that_cannot_start_is_retried_a_few_times_then_reported_with_its_output() {
    let mut p: Prepared = prepare(nothing);
    // Pi rejects a malformed server id as it starts, so the engine exits during startup, every
    // time, within moments.
    p.config.server_id = "not-a-valid-server-id".into();
    let (dir, id) = (p.server_dir.clone(), p.config.server_id.clone());
    let db = p.root.join("app.sqlite3");
    let mut config = PiConfig::managed(p.config.clone());
    config.retry_delay = Duration::from_millis(100);
    let mut h = Harness::start(config, &db);

    h.until(
        "the crash loop is reported",
        |s| matches!(&s.connection, Connection::Failed(m) if m.contains("keeps crashing")),
    );
    let Connection::Failed(message) = h.state.connection.clone() else {
        unreachable!()
    };
    assert!(message.contains("3 starts"), "{message}");
    assert!(
        message.to_lowercase().contains("invalid"),
        "the engine's own output is in the diagnosis: {message}"
    );
    // Once capped, no further engine is started.
    thread::sleep(Duration::from_secs(2));
    assert_eq!(
        profile_pids(&dir, &id),
        Vec::<u32>::new(),
        "no engine left running"
    );
    h.stop();
}

/// A user who has an engine running already must be left alone by the app: connecting to it
/// never starts, restarts or stops anything.
#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn an_engine_the_app_did_not_start_is_never_stopped_by_it() {
    let fx = Fixture::start(nothing);
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(direct_config(&fx.server_dir, &fx.server_id), &db);
    h.until("connected", |s| s.connection.is_ready());
    let before = profile_pids(&fx.server_dir, &fx.server_id);
    assert!(!before.is_empty());
    h.stop();
    assert!(
        handshake(&fx.server_dir, &fx.server_id),
        "the engine outlives the app"
    );
    let after = profile_pids(&fx.server_dir, &fx.server_id);
    assert_eq!(
        before, after,
        "no engine process was started or stopped by the app"
    );
    drop(EngineHost::new(fx.host.config().clone())); // a host that never started anything stops nothing
    assert!(handshake(&fx.server_dir, &fx.server_id));
}

// ------------------------------------------------------------------------- M3: run control

fn assistant_texts(s: &AppState) -> Vec<String> {
    s.current()
        .map(|c| {
            c.items
                .iter()
                .filter_map(|i| match &i.kind {
                    ItemKind::Assistant { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn queued(s: &AppState) -> Vec<(QueueMode, String)> {
    s.current()
        .map(|c| c.queue.iter().map(|q| (q.mode, q.text.clone())).collect())
        .unwrap_or_default()
}

fn is_running(s: &AppState) -> bool {
    matches!(s.current().map(|c| &c.run), Some(RunState::Running { .. }))
}

fn is_idle(s: &AppState) -> bool {
    s.current().is_some_and(|c| c.run == RunState::Idle)
}

/// The text of every user message in a model request, oldest first.
fn user_texts(request: &Value) -> Vec<String> {
    request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "user")
        .map(|m| match &m["content"] {
            Value::String(s) => s.clone(),
            Value::Array(parts) => parts
                .iter()
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join(""),
            other => other.to_string(),
        })
        .collect()
}

/// The first model turn is held until the gate opens; later turns answer at once.
fn held_first(
    gate: &Gate,
    first: Reply,
    rest: Vec<Reply>,
) -> impl Fn(&Value, usize) -> Reply + use<> {
    let gate = gate.clone();
    move |_, n| match n {
        0 => Reply::Gated(gate.clone(), Box::new(first.clone())),
        n => rest
            .get(n - 1)
            .cloned()
            .unwrap_or_else(|| Reply::Text(format!("extra turn {n}"))),
    }
}

/// Stopping a run that is in flight: the stop is a request, the run ends when the engine says
/// so, and the conversation takes the next prompt afterwards.
#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn stopping_a_run_waits_for_the_engine_and_the_conversation_takes_the_next_prompt() {
    let gate = Gate::new();
    let fx = Fixture::start(held_first(
        &gate,
        Reply::Text("never delivered".into()),
        vec![Reply::Text("second answer".into())],
    ));
    init_project(&fx.project);
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(direct_config(&fx.server_dir, &fx.server_id), &db);
    h.open_new_conversation(&fx.project);

    h.send("a long task");
    h.until_asked(&fx.provider, 1);
    h.until("the run is in flight", is_running);
    assert!(h.state.availability().cancel);

    h.dispatch(Command::Cancel);
    assert!(matches!(
        h.state.current().unwrap().run,
        RunState::Stopping { .. }
    ));
    assert!(
        !h.state.availability().submit,
        "no new prompt until the engine confirms"
    );
    h.until("the engine confirms the stop", is_idle);
    assert!(
        !assistant_texts(&h.state)
            .iter()
            .any(|t| t.contains("never"))
    );

    // The conversation is usable again, and the stopped prompt is closed in the journal.
    h.send("try again");
    h.until("the next prompt is answered", |s| {
        is_idle(s) && assistant_texts(s).contains(&"second answer".to_owned())
    });
    assert_eq!(h.user_messages(), ["a long task", "try again"]);
    h.storage.shutdown();
    let loaded = Storage::open(&db, NAMESPACE).unwrap().load_all().unwrap();
    assert!(
        loaded.open_requests.is_empty(),
        "{:?}",
        loaded.open_requests
    );
    gate.open();
    h.backend.shutdown();
}

#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn a_lost_stop_request_remains_unsettled_and_can_be_checked_then_retried_without_a_prompt() {
    let gate = Gate::new();
    let fx = Fixture::start(held_first(
        &gate,
        Reply::Text("not delivered".into()),
        vec![],
    ));
    init_project(&fx.project);
    let proxy = FaultProxy::start(
        fx.server_dir.join(format!("{}.sock", fx.server_id)),
        fx.root.join("proxy"),
        &fx.server_id,
    );
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(direct_config(&proxy.dir, &fx.server_id), &db);
    h.open_new_conversation(&fx.project);
    h.send("one long task");
    h.until_asked(&fx.provider, 1);
    h.until("the run is active", is_running);
    proxy.arm(Fault::DropNextStopRequest);
    h.dispatch(Command::Cancel);
    h.until("lost stop is reported after reconnect", |s| {
        proxy.fired() == 1
            && s.connection.is_ready()
            && s.current()
                .is_some_and(|c| c.stop_error.is_some() && c.recovery_notice)
    });
    assert!(matches!(
        h.state.current().unwrap().run,
        RunState::Stopping { .. }
    ));
    assert!(!h.state.availability().submit);
    assert!(h.state.availability().check_status && h.state.availability().retry_stop);
    h.dispatch(Command::CheckStatus);
    h.until("Pi confirms the existing run is still open", |s| {
        !s.current().unwrap().status_check_pending
    });
    assert!(matches!(
        h.state.current().unwrap().run,
        RunState::Stopping { .. }
    ));
    assert!(h.state.current().unwrap().status_error.is_none());
    assert_eq!(fx.provider.requests().len(), 1);
    h.dispatch(Command::RetryStop);
    h.until("Pi confirms the retried stop", is_idle);
    assert_eq!(h.user_messages(), ["one long task"]);
    assert_eq!(
        fx.provider.requests().len(),
        1,
        "neither check nor retry stop resends the prompt"
    );
    assert!(h.state.current().unwrap().stop_error.is_none());
    assert!(h.state.current().unwrap().recovery_notice);
    h.storage.shutdown();
    journal_is_clean(&db);
    gate.open();
    h.backend.shutdown();
}

fn stopped_input_is_not_presented_to_the_next_model_turn(as_goal: bool) {
    let gate = Gate::new();
    let fx = Fixture::start(held_first(
        &gate,
        Reply::Text("old task not completed".into()),
        vec![Reply::Text("1 2 3 ... 100\n[PIPKIN_GOAL_MET]".into())],
    ));
    init_project(&fx.project);
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(direct_config(&fx.server_dir, &fx.server_id), &db);
    h.open_new_conversation(&fx.project);
    h.send("commit and push");
    h.until_asked(&fx.provider, 1);
    h.until("the original request is running", is_running);
    h.dispatch(Command::EditDraft("also publish the old work".into()));
    h.dispatch(Command::QueueFollowUp);
    h.until("the old follow-up is queued", |s| {
        !s.current().unwrap().queue.is_empty()
    });
    h.dispatch(Command::Cancel);
    if as_goal {
        // Entering a new goal while stop confirmation is still pending must neither send it
        // early nor pause it when the previous run's cancellation eventually arrives.
        h.dispatch(Command::SetGoal("count to 100".into()));
    } else {
        h.until("stop is confirmed and old queued work is withdrawn", |s| {
            is_idle(s) && s.current().unwrap().queue.is_empty()
        });
        h.send("count to 100");
    }
    h.until("only the replacement finishes", |s| {
        is_idle(s)
            && assistant_texts(s)
                .iter()
                .any(|text| text.starts_with("1 2 3"))
    });
    let requests = fx.provider.requests();
    assert_eq!(requests.len(), 2, "the withdrawn follow-up must not run");
    let users = user_texts(&requests[1]);
    assert_eq!(
        users.len(),
        1,
        "stopped instructions must not leak into the next model context: {users:?}"
    );
    assert!(users[0].contains("count to 100"));
    assert!(!users[0].contains("commit and push"));
    assert!(!users[0].contains("publish the old work"));
    assert!(
        h.user_messages()
            .iter()
            .any(|text| text == "commit and push"),
        "keep the visible audit trail"
    );
    if as_goal {
        assert_eq!(
            h.state
                .current()
                .unwrap()
                .goal
                .as_ref()
                .unwrap()
                .paused
                .as_deref(),
            Some("Goal met.")
        );
    }
    h.storage.shutdown();
    journal_is_clean(&db);
    gate.open();
    h.backend.shutdown();
}

#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn stopping_a_prompt_flushes_its_instruction_before_a_new_prompt() {
    stopped_input_is_not_presented_to_the_next_model_turn(false);
}

#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn stopping_a_prompt_flushes_its_instruction_before_a_replacement_goal() {
    stopped_input_is_not_presented_to_the_next_model_turn(true);
}

/// A follow-up waits behind the run in the engine's own queue, then runs on its own.
#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn a_follow_up_waits_in_the_engines_queue_then_runs_by_itself() {
    let gate = Gate::new();
    let fx = Fixture::start(held_first(
        &gate,
        Reply::Text("first done".into()),
        vec![Reply::Text("second done".into())],
    ));
    init_project(&fx.project);
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(direct_config(&fx.server_dir, &fx.server_id), &db);
    h.open_new_conversation(&fx.project);

    h.send("first");
    h.until_asked(&fx.provider, 1);
    h.until("running", is_running);
    h.dispatch(Command::EditDraft("second".into()));
    assert!(h.state.availability().queue);
    h.dispatch(Command::QueueFollowUp);
    h.until("the draft moved on, to the queue", |s| {
        s.current().unwrap().draft.text.is_empty()
    });
    h.until("the engine's queue shows it", |s| {
        queued(s) == [(QueueMode::FollowUp, "second".to_owned())]
            && s.current().unwrap().pending_queue.is_empty()
    });
    assert!(is_running(&h.state));

    gate.open();
    h.until("both ran, in order", |s| {
        is_idle(s) && assistant_texts(s) == ["first done".to_owned(), "second done".to_owned()]
    });
    assert!(h.state.current().unwrap().queue.is_empty());
    assert_eq!(h.user_messages(), ["first", "second"]);
    let requests = fx.provider.requests();
    assert_eq!(requests.len(), 2, "one model turn each");
    assert_eq!(user_texts(&requests[1]), ["first", "second"]);

    // Both journal entries are closed: nothing to reconcile on the next launch.
    h.storage.shutdown();
    let loaded = Storage::open(&db, NAMESPACE).unwrap().load_all().unwrap();
    assert!(
        loaded.open_requests.is_empty(),
        "{:?}",
        loaded.open_requests
    );
    h.backend.shutdown();
}

/// Removing a queued input withdraws it from the engine: it never reaches the model.
#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn a_queued_input_removed_before_it_runs_never_reaches_the_model() {
    let gate = Gate::new();
    let fx = Fixture::start(held_first(
        &gate,
        Reply::Text("first done".into()),
        vec![Reply::Text("kept done".into())],
    ));
    init_project(&fx.project);
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(direct_config(&fx.server_dir, &fx.server_id), &db);
    h.open_new_conversation(&fx.project);

    h.send("first");
    h.until_asked(&fx.provider, 1);
    h.until("running", is_running);
    for text in ["queued-drop-xq7", "queued-keep-xq7"] {
        h.dispatch(Command::EditDraft(text.into()));
        h.dispatch(Command::QueueFollowUp);
        let want = text.to_owned();
        h.until("queued", |s| {
            queued(s).iter().any(|(_, t)| *t == want)
                && s.current().unwrap().pending_queue.is_empty()
        });
    }
    assert_eq!(queued(&h.state).len(), 2);

    let second = h.state.current().unwrap().queue[0].id;
    h.dispatch(Command::RemoveQueued(second));
    h.until("the engine dropped it", |s| {
        queued(s) == [(QueueMode::FollowUp, "queued-keep-xq7".to_owned())]
    });

    gate.open();
    h.until("first and the kept input ran", |s| {
        is_idle(s) && assistant_texts(s) == ["first done".to_owned(), "kept done".to_owned()]
    });
    let requests = fx.provider.requests();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert!(
            !request.to_string().contains("queued-drop-xq7"),
            "a removed input must never be sent to the model: {request}"
        );
    }
    assert_eq!(user_texts(&requests[1]), ["first", "queued-keep-xq7"]);
    h.backend.shutdown();
}

/// Steering joins the run in flight: the model's next turn in the same run sees it.
#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn a_steer_reaches_the_next_model_turn_of_the_active_run() {
    let gate = Gate::new();
    let fx = Fixture::start(held_first(
        &gate,
        Reply::Tool {
            lead: Some("Working. ".into()),
            name: "write".into(),
            args: json!({ "path": "notes.txt", "content": "one\ntwo\n" }),
        },
        vec![Reply::Text("done, with tabs".into())],
    ));
    init_project(&fx.project);
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(direct_config(&fx.server_dir, &fx.server_id), &db);
    h.open_new_conversation(&fx.project);

    h.send("edit the notes");
    h.until_asked(&fx.provider, 1);
    h.until("running", is_running);
    h.dispatch(Command::EditDraft("use tabs".into()));
    assert!(h.state.availability().steer);
    h.dispatch(Command::Steer);
    h.until("the engine holds the steer", |s| {
        queued(s) == [(QueueMode::Steer, "use tabs".to_owned())]
    });

    gate.open();
    h.until("the run finished", |s| {
        is_idle(s) && assistant_texts(s).iter().any(|t| t.contains("with tabs"))
    });
    let requests = fx.provider.requests();
    assert_eq!(requests.len(), 2, "one run, two model turns: {requests:?}");
    assert_eq!(
        user_texts(&requests[1]),
        ["edit the notes", "use tabs"],
        "the second turn of the same run saw the steer"
    );
    assert_eq!(h.user_messages(), ["edit the notes", "use tabs"]);
    assert!(h.state.current().unwrap().queue.is_empty());
    h.backend.shutdown();
}

/// Closing the app mid-run and opening it again picks the run up where it is: the unanswered
/// prompt is looked up (never resent), the engine's queue is shown, and the run finishes.
#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn a_run_and_its_queue_are_picked_up_again_after_the_app_restarts() {
    let gate = Gate::new();
    let fx = Fixture::start(held_first(
        &gate,
        Reply::Text("first done".into()),
        vec![Reply::Text("second done".into())],
    ));
    init_project(&fx.project);
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(direct_config(&fx.server_dir, &fx.server_id), &db);
    h.open_new_conversation(&fx.project);
    let conversation = h.state.current().unwrap().id;

    h.send("first");
    h.until_asked(&fx.provider, 1);
    h.until("running", is_running);
    h.dispatch(Command::EditDraft("second".into()));
    h.dispatch(Command::QueueFollowUp);
    h.until("queued", |s| {
        queued(s) == [(QueueMode::FollowUp, "second".to_owned())]
    });
    h.stop(); // the window closes; the engine and its run carry on

    let mut h2 = Harness::start(direct_config(&fx.server_dir, &fx.server_id), &db);
    // The prompt's journal entry was still open, so it comes back in doubt, is looked up
    // automatically, and is found running.
    h2.until("the run is found running again", |s| {
        s.selected == Some(conversation) && is_running(s) && !queued(s).is_empty()
    });
    assert_eq!(
        queued(&h2.state),
        [(QueueMode::FollowUp, "second".to_owned())],
        "the queue is the engine's, shown after a restart"
    );
    assert_eq!(fx.provider.requests().len(), 1, "nothing was resent");
    assert_eq!(h2.user_messages(), ["first"], "one prompt, once");

    gate.open();
    h2.until("everything finishes", |s| {
        is_idle(s) && assistant_texts(s) == ["first done".to_owned(), "second done".to_owned()]
    });
    assert_eq!(fx.provider.requests().len(), 2);
    assert_eq!(h2.user_messages(), ["first", "second"]);
    h2.stop();
    let loaded = Storage::open(&db, NAMESPACE).unwrap().load_all().unwrap();
    assert!(
        loaded.open_requests.is_empty(),
        "{:?}",
        loaded.open_requests
    );
}

/// A run found busy that this window did not start (its prompt was already settled in the
/// journal) is still shown and can be stopped.
#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn a_run_started_elsewhere_is_shown_and_can_be_stopped() {
    let gate = Gate::new();
    let fx = Fixture::start(held_first(&gate, Reply::Text("never".into()), vec![]));
    init_project(&fx.project);
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(direct_config(&fx.server_dir, &fx.server_id), &db);
    h.open_new_conversation(&fx.project);
    let conversation = h.state.current().unwrap().id;
    h.send("started here");
    h.until_asked(&fx.provider, 1);
    h.until("running", is_running);
    h.stop();

    // Forget the journal entry, as if another client had started the run.
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute("UPDATE request_journal SET state = 'completed'", [])
            .unwrap();
    }
    let mut h2 = Harness::start(direct_config(&fx.server_dir, &fx.server_id), &db);
    h2.until("the busy run is shown", |s| {
        s.selected == Some(conversation) && is_running(s)
    });
    assert!(h2.state.availability().cancel);
    h2.dispatch(Command::Cancel);
    h2.until("it stops, on the engine's word", is_idle);
    gate.open();
    h2.stop();
}

/// Moving between conversations keeps each one's run: an engine attached to one session at a
/// time still shows the right state when you come back, even when the run ended while away.
#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn switching_conversations_keeps_each_ones_run_and_settles_what_ended_while_away() {
    let gate = Gate::new();
    let fx = Fixture::start(held_first(
        &gate,
        Reply::Text("a is done".into()),
        vec![Reply::Text("b is done".into())],
    ));
    init_project(&fx.project);
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(direct_config(&fx.server_dir, &fx.server_id), &db);
    h.open_new_conversation(&fx.project);
    let a = h.state.current().unwrap().id;
    h.send("work in A");
    h.until_asked(&fx.provider, 1);
    h.until("A is running", is_running);

    // A second conversation in the same project; the first keeps running behind it.
    h.dispatch(Command::NewConversation);
    h.until("B is open", |s| {
        s.selected.is_some_and(|id| id != a) && s.current().is_some_and(|c| c.opened)
    });
    let b = h.state.selected.unwrap();
    assert!(is_idle(&h.state), "B has no run");
    assert!(matches!(
        h.state.conversation(a).unwrap().run,
        RunState::Running { .. }
    ));
    // B works while A is held.
    h.send("work in B");
    h.until("B is answered", |s| {
        is_idle(s) && assistant_texts(s).contains(&"b is done".to_owned())
    });

    h.until("B's initial workspace scan is complete", |s| {
        s.current().unwrap().changes_state == ChangesState::Ready
    });
    // Drain B's coalesced initial scans before simulating A's final filesystem effect.
    for _ in 0..5 {
        thread::sleep(Duration::from_millis(100));
        h.pump();
    }
    let before = h.state.current().unwrap();
    let (b_changes, b_revision, b_selected) = (
        before.changes.clone(),
        before.changes_revision,
        before.selected_change,
    );
    std::fs::write(
        fx.project.join("only-in-a.txt"),
        "effect of the background task\n",
    )
    .unwrap();
    // A finishes while the user is looking at B. Its completion must not trigger a
    // workspace refresh for B, even though these conversations share a project.
    gate.open();
    h.until_asked(&fx.provider, 2);
    h.until("A settles in the background", |s| {
        matches!(s.conversation(a).unwrap().run, RunState::Idle)
    });
    for _ in 0..5 {
        thread::sleep(Duration::from_millis(100));
        h.pump();
    }
    assert_eq!(h.state.selected, Some(b));
    let current = h.state.current().unwrap();
    assert_eq!(current.changes, b_changes);
    assert_eq!(current.changes_revision, b_revision);
    assert_eq!(current.selected_change, b_selected);
    h.dispatch(Command::SelectConversation(a));
    h.until("A is attached again and settled", |s| {
        s.selected == Some(a)
            && s.current().is_some_and(|c| c.opened)
            && is_idle(s)
            && assistant_texts(s).contains(&"a is done".to_owned())
    });
    assert_eq!(h.user_messages(), ["work in A"]);
    // And B is still its own.
    h.dispatch(Command::SelectConversation(b));
    h.until("B shows its own history", |s| {
        s.selected == Some(b)
            && s.current().is_some_and(|c| c.opened)
            && assistant_texts(s).contains(&"b is done".to_owned())
    });
    assert_eq!(h.user_messages(), ["work in B"]);
    h.backend.shutdown();
}

// ------------------------------------------------------------------------------ M3: attachments

/// A valid 1x1 PNG: Pi decodes images before storing them, so the bytes must be real.
const TINY_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn attachments_reach_the_model_as_text_and_image_and_come_back_as_chips() {
    use base64::Engine as _;
    let fx = Fixture::start(|_, _| Reply::Text("I read them.".into()));
    init_project(&fx.project);
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(direct_config(&fx.server_dir, &fx.server_id), &db);
    h.open_new_conversation(&fx.project);

    let outside = fx.root.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let notes = outside.join("spec.md");
    std::fs::write(&notes, "# Spec\nUse tabs.\n").unwrap();
    let image = outside.join("shot.png");
    std::fs::write(
        &image,
        base64::engine::general_purpose::STANDARD
            .decode(TINY_PNG)
            .unwrap(),
    )
    .unwrap();
    h.dispatch(Command::AddAttachments(vec![
        describe_attachment(&notes),
        describe_attachment(&image),
    ]));
    h.dispatch(Command::EditDraft("review these".into()));
    h.dispatch(Command::Submit);
    h.until("answered", |s| {
        is_idle(s) && assistant_texts(s).contains(&"I read them.".to_owned())
    });

    // The model was given the file's text and the image.
    let requests = fx.provider.requests();
    let body = requests[0].to_string();
    assert!(
        body.contains("review these") && body.contains("# Spec\\nUse tabs."),
        "{body}"
    );
    assert!(body.contains("data:image/png;base64,"), "{body}");

    // The transcript shows what the person typed and two chips, not the file's text.
    let c = h.state.current().unwrap();
    let ItemKind::User {
        text, attachments, ..
    } = &c
        .items
        .iter()
        .find(|i| matches!(i.kind, ItemKind::User { .. }))
        .unwrap()
        .kind
    else {
        unreachable!()
    };
    assert_eq!(text, "review these");
    let names: Vec<_> = attachments.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["spec.md", "Image"]);
    assert!(c.draft.attachments.is_empty(), "the draft was cleared");
    h.backend.shutdown();
}

#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn a_draft_with_attachments_survives_a_restart_and_a_missing_file_blocks_sending() {
    let fx = Fixture::start(|_, _| Reply::Text("ok".into()));
    init_project(&fx.project);
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(direct_config(&fx.server_dir, &fx.server_id), &db);
    h.open_new_conversation(&fx.project);
    let conversation = h.state.current().unwrap().id;

    let keep = fx.root.join("keep.txt");
    let gone = fx.root.join("gone.txt");
    std::fs::write(&keep, "kept").unwrap();
    std::fs::write(&gone, "will vanish").unwrap();
    h.dispatch(Command::AddAttachments(vec![
        describe_attachment(&keep),
        describe_attachment(&gone),
    ]));
    h.dispatch(Command::EditDraft("unsent thought".into()));
    h.dispatch(Command::FlushDraft(conversation));
    h.stop();

    std::fs::remove_file(&gone).unwrap();
    let mut h2 = Harness::start(direct_config(&fx.server_dir, &fx.server_id), &db);
    h2.until("the draft is back", |s| {
        s.selected == Some(conversation)
            && s.current().is_some_and(|c| {
                c.opened && c.draft.text == "unsent thought" && c.draft.attachments.len() == 2
            })
    });
    let c = h2.state.current().unwrap();
    assert!(c.draft.attachments[0].error.is_none());
    assert_eq!(
        c.draft.attachments[1].error.as_deref(),
        Some("File not found"),
        "a file that went away is flagged on reopen"
    );
    assert!(
        !h2.state.availability().submit,
        "a flagged attachment blocks sending until it is removed"
    );
    h2.dispatch(Command::RemoveAttachment(1));
    assert!(h2.state.availability().submit);
    h2.dispatch(Command::Submit);
    h2.until("sent and answered", |s| {
        is_idle(s) && assistant_texts(s).contains(&"ok".to_owned())
    });
    assert!(fx.provider.requests()[0].to_string().contains("kept"));
    h2.backend.shutdown();
}

// ------------------------------------------------------------------------ M3: the fault matrix

/// A held run behind a fault proxy: the engine, the app's database, and the project.
struct Held {
    fx: Fixture,
    proxy: FaultProxy,
    gate: Gate,
    db: PathBuf,
}

fn held_setup(rest: Vec<Reply>) -> Held {
    let gate = Gate::new();
    let fx = Fixture::start(held_first(&gate, Reply::Text("first done".into()), rest));
    init_project(&fx.project);
    let proxy = FaultProxy::start(
        fx.server_dir.join(format!("{}.sock", fx.server_id)),
        fx.root.join("proxy"),
        &fx.server_id,
    );
    let db = fx.root.join("app.sqlite3");
    Held {
        fx,
        proxy,
        gate,
        db,
    }
}

fn journal_is_clean(db: &Path) {
    if std::env::var_os("PIPKIN_DEBUG_JOURNAL").is_some() {
        let conn = rusqlite::Connection::open(db).unwrap();
        let mut stmt = conn
            .prepare("SELECT request_id, state, payload FROM request_journal ORDER BY rowid")
            .unwrap();
        for row in stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .unwrap()
        {
            eprintln!("journal: {:?}", row.unwrap());
        }
    }
    let loaded = Storage::open(db, NAMESPACE).unwrap().load_all().unwrap();
    assert!(
        loaded.open_requests.is_empty(),
        "unresolved journal entries: {:?}",
        loaded.open_requests
    );
}

/// The engine admitted a follow-up but the acknowledgment never arrived. The app reconnects,
/// asks the engine about the key, finds it, and shows it once: no duplicate, no resend.
#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn a_follow_up_whose_acknowledgment_is_lost_is_admitted_exactly_once() {
    let f = held_setup(vec![Reply::Text("second done".into())]);
    let mut h = Harness::start(direct_config(&f.proxy.dir, &f.fx.server_id), &f.db);
    h.open_new_conversation(&f.fx.project);
    h.send("first");
    h.until_asked(&f.fx.provider, 1);
    h.until("running", is_running);

    f.proxy.arm(Fault::DropNextQueueReply);
    h.dispatch(Command::EditDraft("second".into()));
    h.dispatch(Command::QueueFollowUp);
    h.until("the acknowledgment was lost", |_| f.proxy.fired() == 1);
    h.until("the app resolves it by asking the engine", |s| {
        s.connection.is_ready()
            && queued(s) == [(QueueMode::FollowUp, "second".to_owned())]
            && s.current().unwrap().pending_queue.is_empty()
    });
    assert!(is_running(&h.state));

    f.gate.open();
    h.until("both ran, once", |s| {
        is_idle(s) && assistant_texts(s) == ["first done".to_owned(), "second done".to_owned()]
    });
    assert_eq!(h.user_messages(), ["first", "second"]);
    assert_eq!(f.fx.provider.requests().len(), 2);
    h.storage.shutdown();
    journal_is_clean(&f.db);
    h.backend.shutdown();
}

/// The follow-up never reached the engine. The same key is sent again, and it is admitted once.
#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn a_follow_up_that_never_reached_the_engine_is_sent_again_under_its_key_once() {
    let f = held_setup(vec![Reply::Text("second done".into())]);
    let mut h = Harness::start(direct_config(&f.proxy.dir, &f.fx.server_id), &f.db);
    h.open_new_conversation(&f.fx.project);
    h.send("first");
    h.until_asked(&f.fx.provider, 1);
    h.until("running", is_running);

    f.proxy.arm(Fault::DropNextQueueRequest);
    h.dispatch(Command::EditDraft("second".into()));
    h.dispatch(Command::QueueFollowUp);
    h.until("the request was cut", |_| f.proxy.fired() == 1);
    h.until("it was sent again and admitted", |s| {
        s.connection.is_ready()
            && queued(s) == [(QueueMode::FollowUp, "second".to_owned())]
            && s.current().unwrap().pending_queue.is_empty()
    });

    f.gate.open();
    h.until("both ran, once", |s| {
        is_idle(s) && assistant_texts(s) == ["first done".to_owned(), "second done".to_owned()]
    });
    assert_eq!(h.user_messages(), ["first", "second"]);
    assert_eq!(f.fx.provider.requests().len(), 2);
    h.storage.shutdown();
    journal_is_clean(&f.db);
    h.backend.shutdown();
}

/// The acknowledgment is lost and the app is killed before it can ask. After relaunch the
/// journal's open entry is looked up, never resent, and the follow-up still runs exactly once.
#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn a_lost_follow_up_acknowledgment_then_a_kill_and_relaunch_runs_it_once() {
    let f = held_setup(vec![Reply::Text("second done".into())]);
    let mut config = direct_config(&f.proxy.dir, &f.fx.server_id);
    config.retry_delay = Duration::from_secs(30); // the app is killed before it reconnects
    let mut h = Harness::start(config, &f.db);
    h.open_new_conversation(&f.fx.project);
    let conversation = h.state.current().unwrap().id;
    h.send("first");
    h.until_asked(&f.fx.provider, 1);
    h.until("running", is_running);

    f.proxy.arm(Fault::DropNextQueueReply);
    h.dispatch(Command::EditDraft("second".into()));
    h.dispatch(Command::QueueFollowUp);
    h.until("the acknowledgment was lost", |s| {
        f.proxy.fired() == 1
            && s.current()
                .unwrap()
                .pending_queue
                .iter()
                .any(|p| p.state == pipkin_core::QueueSend::Unknown)
    });
    h.stop();

    let mut h2 = Harness::start(direct_config(&f.fx.server_dir, &f.fx.server_id), &f.db);
    h2.until("the engine's side of it is found", |s| {
        s.selected == Some(conversation)
            && is_running(s)
            && queued(s) == [(QueueMode::FollowUp, "second".to_owned())]
    });
    assert_eq!(f.fx.provider.requests().len(), 1, "nothing was resent");

    f.gate.open();
    h2.until("both ran, once", |s| {
        is_idle(s) && assistant_texts(s) == ["first done".to_owned(), "second done".to_owned()]
    });
    assert_eq!(h2.user_messages(), ["first", "second"]);
    assert_eq!(f.fx.provider.requests().len(), 2);
    h2.stop();
    journal_is_clean(&f.db);
}

/// The link to the engine drops while a run is in flight and the engine carries on: the app
/// reconnects, keeps showing the run and its queue, and the run settles on the engine's word.
#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn losing_the_connection_mid_run_reconnects_and_the_run_still_settles() {
    let f = held_setup(vec![Reply::Text("second done".into())]);
    let mut h = Harness::start(direct_config(&f.proxy.dir, &f.fx.server_id), &f.db);
    h.open_new_conversation(&f.fx.project);
    h.send("first");
    h.until_asked(&f.fx.provider, 1);
    h.until("running", is_running);
    h.dispatch(Command::EditDraft("second".into()));
    h.dispatch(Command::QueueFollowUp);
    h.until("queued", |s| {
        queued(s) == [(QueueMode::FollowUp, "second".to_owned())]
    });

    f.proxy.sever_all();
    h.until("the loss is noticed", |s| !s.connection.is_ready());
    // While the engine is unreachable nothing can be sent, and nothing is claimed.
    assert!(!h.state.availability().submit && !h.state.availability().cancel);
    h.until("reconnected", |s| s.connection.is_ready());
    assert!(is_running(&h.state), "the run is still shown");
    assert_eq!(queued(&h.state).len(), 1, "and so is its queue");

    f.gate.open();
    h.until("everything finished", |s| {
        is_idle(s) && assistant_texts(s) == ["first done".to_owned(), "second done".to_owned()]
    });
    assert_eq!(f.fx.provider.requests().len(), 2, "no turn was repeated");
    assert_eq!(h.user_messages(), ["first", "second"]);
    h.storage.shutdown();
    journal_is_clean(&f.db);
    h.backend.shutdown();
}

/// Storage refuses the journal write (disk full, say): nothing is sent, the text stays in the
/// composer, and once storage works again the same text sends, once.
#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn a_prompt_that_cannot_be_journaled_is_not_sent_and_sends_once_storage_recovers() {
    let fx = Fixture::start(|_, _| Reply::Text("ok".into()));
    init_project(&fx.project);
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(direct_config(&fx.server_dir, &fx.server_id), &db);
    h.open_new_conversation(&fx.project);

    h.storage.set_write_failure(true);
    h.send("important words");
    h.until("the refusal is shown", |s| {
        s.current().unwrap().intent_error.is_some()
    });
    let c = h.state.current().unwrap();
    assert_eq!(c.draft.text, "important words", "nothing was lost");
    assert!(is_idle(&h.state));
    thread::sleep(Duration::from_millis(500));
    assert_eq!(fx.provider.requests().len(), 0, "nothing reached the model");
    assert!(h.user_messages().is_empty());

    h.storage.set_write_failure(false);
    h.dispatch(Command::DismissFailure);
    h.dispatch(Command::Submit);
    h.until("it sends once", |s| {
        is_idle(s) && assistant_texts(s) == ["ok".to_owned()]
    });
    assert_eq!(h.user_messages(), ["important words"]);
    assert_eq!(fx.provider.requests().len(), 1);
    h.backend.shutdown();
}

/// The engine process dies hard while a run is in flight. The app restarts it and reconnects;
/// the engine's own durable recovery resumes the interrupted run. The app resends nothing: the
/// prompt exists once, in the transcript, in the model's context, and in the journal.
#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn an_engine_crash_mid_run_is_recovered_by_the_engine_and_the_prompt_is_not_duplicated() {
    let gate = Gate::new();
    let g = gate.clone();
    let p: Prepared = prepare(move |_, n| match n {
        0 => Reply::Gated(g.clone(), Box::new(Reply::Text("never delivered".into()))),
        n => Reply::Text(format!("recovered answer {n}")),
    });
    init_project(&p.project);
    let (dir, id) = (p.server_dir.clone(), p.server_id.clone());
    let db = p.root.join("app.sqlite3");
    let mut config = PiConfig::managed(p.config.clone());
    config.retry_delay = Duration::from_millis(200);
    let mut h = Harness::start(config, &db);
    h.open_new_conversation(&p.project);
    h.send("first");
    h.until_asked(&p.provider, 1);
    h.until("running", is_running);

    for pid in profile_pids(&dir, &id) {
        // SAFETY: SIGKILL to processes matched by this test's own profile identity.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    }
    h.until("the app sees the loss", |s| !s.connection.is_ready());
    assert!(
        h.state.current().unwrap().recovery_notice,
        "interrupted work needs the tool-recovery caveat"
    );
    assert!(
        !h.state.availability().submit,
        "nothing can be sent while the engine is down"
    );
    h.until("the app restarted the engine and reconnected", |s| {
        s.connection.is_ready()
    });
    h.until("the engine resumed the run and finished it", |s| {
        is_idle(s)
            && assistant_texts(s)
                .iter()
                .any(|t| t.starts_with("recovered answer"))
    });

    assert_eq!(h.user_messages(), ["first"], "one prompt, once");
    for request in p.provider.requests() {
        assert_eq!(user_texts(&request), ["first"], "never a second copy");
    }
    h.storage.shutdown();
    journal_is_clean(&db);
    gate.open();
    h.backend.shutdown();
    drop(p.provider);
}

/// Credentials are Pi's: the app shows "no model is ready" when the engine has none, and a
/// refresh picks up a provider that was configured after the engine started.
#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn a_provider_configured_after_start_appears_after_a_refresh_and_can_then_be_used() {
    let mut p: Prepared = prepare(|_, _| Reply::Text("hello from the new provider".into()));
    init_project(&p.project);
    // The engine starts with no provider at all.
    let agent_dir = p.config.agent_dir.clone().unwrap();
    let configured = std::fs::read_to_string(agent_dir.join("models.json")).unwrap();
    std::fs::write(agent_dir.join("models.json"), r#"{"providers":{}}"#).unwrap();
    p.config.model = None;
    let db = p.root.join("app.sqlite3");
    let mut h = Harness::start(PiConfig::managed(p.config.clone()), &db);
    h.open_new_conversation(&p.project);

    assert!(h.state.models.is_empty(), "{:?}", h.state.models);
    assert!(
        h.state.availability().refresh_models,
        "a refresh is offered when there is no model"
    );

    // The user adds the provider (Pi's own configuration), then refreshes.
    std::fs::write(agent_dir.join("models.json"), configured).unwrap();
    h.dispatch(Command::RefreshModels);
    h.until("the provider's model appears", |s| {
        s.models.iter().any(|m| m.id == "stub/scripted")
    });

    h.dispatch(Command::SetModel("stub/scripted".into()));
    h.until("the engine reports the selection", |s| {
        s.prefs.model.as_deref() == Some("stub/scripted")
    });
    h.send("hello");
    h.until("it answers", |s| {
        is_idle(s) && assistant_texts(s) == ["hello from the new provider".to_owned()]
    });
    h.stop();
    drop(p.provider);
}

#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn choosing_effort_is_confirmed_by_pi_and_restored_per_conversation() {
    let p = prepare(|_, _| Reply::Text("okay".into()));
    init_project(&p.project);
    let agent_dir = p.config.agent_dir.as_ref().unwrap();
    let path = agent_dir.join("models.json");
    let mut config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config["providers"]["stub"]["models"][0]["reasoning"] = json!(true);
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    let db = p.root.join("app.sqlite3");
    let mut h = Harness::start(PiConfig::managed(p.config.clone()), &db);
    h.open_new_conversation(&p.project);
    h.until("reasoning levels load", |s| {
        s.current()
            .is_some_and(|c| c.thinking_levels.iter().any(|l| l == "high"))
    });
    let first = h.state.current().unwrap().id;
    h.dispatch(Command::SetThinkingLevel("high".into()));
    h.until("Pi confirms high effort", |s| {
        s.current()
            .is_some_and(|c| c.thinking_level.as_deref() == Some("high"))
    });
    h.dispatch(Command::NewConversation);
    h.until("new conversation opens", |s| {
        s.current()
            .is_some_and(|c| c.id != first && c.opened && c.thinking_level.is_some())
    });
    h.dispatch(Command::SelectConversation(first));
    h.until("first conversation restores its effort", |s| {
        s.current().is_some_and(|c| {
            c.id == first && c.opened && c.thinking_level.as_deref() == Some("high")
        })
    });
    h.stop();
}

// ------------------------------------------------------------------------------ M4

/// A second client attached to a session, to do what Pipkin does not offer (compacting) or to
/// look at the engine's services directly.
struct RawSession {
    client: pi_client::client::Client,
    target: pi_client::protocol::RpcTarget,
    _events: async_channel::Receiver<pi_client::client::ClientEvent>,
}

impl RawSession {
    /// Attach the only session an engine holds, as a second client.
    fn attach(fx: &Fixture) -> RawSession {
        use pi_client::chord::ServiceCall;
        use pi_client::client::ClientOptions;
        let sessions = fx.root.join("agent/experimental/sessions");
        let session_id = std::fs::read_dir(&sessions)
            .unwrap_or_else(|e| panic!("{}: {e}", sessions.display()))
            .flatten()
            .find(|e| e.path().join("meta.json").exists())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .expect("a session on disk");
        let (client, events) = pi_client::unix::connect(
            &fx.server_dir.join(format!("{}.sock", fx.server_id)),
            ClientOptions::new(&fx.server_id),
        )
        .expect("handshake");
        let server = pi_client::protocol::RpcTarget::Server {
            server_id: fx.server_id.clone(),
        };
        client
            .request(
                &server,
                &ServiceCall::new("pi.session-management", "attach", vec![json!(session_id)]),
            )
            .and_then(|p| p.wait_timeout(Duration::from_secs(30)))
            .expect("attach");
        let deadline = Instant::now() + Duration::from_secs(10);
        let target = loop {
            if let Some(a) = client.attachment()
                && a.session_id == session_id
            {
                break a.rpc();
            }
            assert!(Instant::now() < deadline, "no attachment route");
            thread::sleep(Duration::from_millis(10));
        };
        RawSession {
            client,
            target,
            _events: events,
        }
    }

    fn call(&self, service: &str, member: &str, args: Vec<Value>) -> Option<Value> {
        use pi_client::chord::ServiceCall;
        self.client
            .request(&self.target, &ServiceCall::new(service, member, args))
            .and_then(|p| p.wait_timeout(Duration::from_secs(30)))
            .unwrap_or_else(|e| panic!("{service}.{member}: {e}"))
    }
}

/// History before a compaction: the live view starts after it, and the earlier messages are
/// paged in from the engine, each exactly once and in order.
#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn history_before_a_compaction_is_paged_in_once_and_in_order() {
    let fx = Fixture::start(|_, n| Reply::Text(format!("answer {n}")));
    init_project(&fx.project);
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(direct_config(&fx.server_dir, &fx.server_id), &db);
    h.open_new_conversation(&fx.project);
    // Long enough that the engine has something before the recent context to summarize.
    let filler = "lorem ipsum dolor sit amet ".repeat(4000);
    for n in 1..=4 {
        h.send(&format!("question {n}\n{filler}"));
        h.until("answered", |s| {
            is_idle(s) && assistant_texts(s).len() >= n as usize
        });
    }
    assert!(
        !h.state.current().unwrap().has_older,
        "nothing is hidden before a compaction"
    );

    // Another client asks the engine to compact; the live view then starts after the summary.
    let raw = RawSession::attach(&fx);
    let reply = raw
        .call(
            "pi.agent-controller",
            "compact",
            vec![json!({ "customInstructions": null })],
        )
        .expect("compact answered");
    assert_eq!(reply["accepted"], true, "{reply}");
    h.until("the live view is only what follows the compaction", |s| {
        s.current().is_some_and(|c| {
            c.has_older
                && !c.items.iter().any(|i| {
                    matches!(&i.kind, ItemKind::User { text, .. } if text.starts_with("question 1\n"))
                })
        })
    });

    // Page back to the beginning.
    let mut pages = 0;
    while h.state.current().unwrap().has_older {
        h.dispatch(Command::LoadOlder);
        h.until("a page arrives", |s| {
            s.current().is_some_and(|c| !c.loading_older)
        });
        pages += 1;
        assert!(pages < 20, "history never ended");
    }
    let c = h.state.current().unwrap();
    assert!(c.older_error.is_none());
    let asked: Vec<String> = h
        .user_messages()
        .iter()
        .filter(|t| t.starts_with("question"))
        .map(|t| t.lines().next().unwrap().to_owned())
        .collect();
    assert_eq!(
        asked,
        ["question 1", "question 2", "question 3", "question 4"],
        "each once, in the order they were asked"
    );
    let ids: Vec<u64> = c.items.iter().map(|i| i.id.0).collect();
    assert!(
        ids.windows(2).all(|w| w[0] < w[1]),
        "ids strictly increase: {ids:?}"
    );
    h.backend.shutdown();
}

#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn the_complete_output_of_a_long_tool_result_is_copied_and_saved_not_the_preview() {
    let fx = Fixture::start(|request, n| match n {
        0 => Reply::Tool {
            lead: None,
            name: "bash".into(),
            args: json!({ "command": "seq 1 3000" }),
        },
        _ => {
            assert!(is_tool_result_turn(request));
            Reply::Text("counted".into())
        }
    });
    init_project(&fx.project);
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(direct_config(&fx.server_dir, &fx.server_id), &db);
    h.open_new_conversation(&fx.project);
    h.send("count to three thousand");
    h.until("counted", |s| {
        is_idle(s) && assistant_texts(s).contains(&"counted".to_owned())
    });
    let (item, preview, full_len) = {
        let c = h.state.current().unwrap();
        let (item, tool) = c
            .items
            .iter()
            .find_map(|i| match &i.kind {
                ItemKind::Tool(t) => Some((i.id, t.clone())),
                _ => None,
            })
            .expect("a tool call");
        assert!(
            tool.truncated,
            "the preview is cut: {} of {}",
            tool.output.len(),
            tool.full_len
        );
        (item, tool.output.clone(), tool.full_len)
    };

    h.dispatch(Command::CopyToolOutput(item));
    let deadline = Instant::now() + WAIT;
    while h.copied.is_empty() {
        h.pump();
        assert!(
            Instant::now() < deadline,
            "nothing was copied; notices: {:?}",
            h.notices
        );
        thread::sleep(Duration::from_millis(10));
    }
    let copied = h.copied[0].clone();
    assert!(
        copied.len() > preview.len(),
        "{} vs preview {}",
        copied.len(),
        preview.len()
    );
    assert_eq!(copied.len(), full_len, "exactly what the engine holds");
    // The preview is the start of the output; the complete text goes on to the last line.
    assert!(copied.contains("3000"), "the whole count, to the end");
    assert!(
        copied.starts_with(&preview),
        "and it begins with the preview"
    );

    h.dispatch(Command::SaveToolOutput(item));
    let deadline = Instant::now() + WAIT;
    while h.saved.is_empty() {
        h.pump();
        assert!(Instant::now() < deadline, "nothing was saved");
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(h.saved[0].0, "bash-output.txt");
    assert_eq!(h.saved[0].1, copied);
    h.backend.shutdown();
}

#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn a_changed_file_opens_in_the_editor_and_a_terminal_in_the_project_from_a_real_run() {
    let fx = Fixture::start(two_edits);
    init_project(&fx.project);
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(direct_config(&fx.server_dir, &fx.server_id), &db);
    h.open_new_conversation(&fx.project);
    h.send("edit things");
    h.until("done with its changes", |s| {
        is_idle(s) && s.current().unwrap().changes.len() == 2
    });
    let real = std::fs::canonicalize(&fx.project)
        .unwrap()
        .display()
        .to_string();
    h.dispatch(Command::OpenInEditor(0));
    h.dispatch(Command::OpenTerminal);
    assert_eq!(
        h.launches,
        [
            pipkin_core::Launch::Editor {
                path: format!("{real}/notes.txt"),
                root: real.clone()
            },
            pipkin_core::Launch::Terminal { cwd: real.clone() },
        ]
    );
    // What the application would run for it is valid for the real files.
    let config = crate::launch::LaunchConfig {
        editor: Some("code --wait".into()),
        terminal: Some("foot".into()),
    };
    let plan = crate::launch::plan(&h.launches[0], &config, "").unwrap();
    assert_eq!(plan.argv, ["code", "--wait", &format!("{real}/notes.txt")]);
    let plan = crate::launch::plan(&h.launches[1], &config, "").unwrap();
    assert_eq!(
        (plan.argv, plan.cwd),
        (vec!["foot".to_owned()], PathBuf::from(&real))
    );
    h.backend.shutdown();
}

/// Saved copies: after a run, the conversation can be read, searched and its search results
/// opened, with the engine gone; and the engine's own state replaces the copy when it is back.
#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn a_saved_conversation_is_readable_and_searchable_without_the_engine_and_the_engine_takes_over_again()
 {
    let fx =
        Fixture::start(|_, n| Reply::Text(format!("the answer mentions marmalade number {n}")));
    init_project(&fx.project);
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(direct_config(&fx.server_dir, &fx.server_id), &db);
    h.open_new_conversation(&fx.project);
    h.send("tell me about preserves");
    h.until("answered", |s| {
        is_idle(s) && assistant_texts(s).iter().any(|t| t.contains("marmalade"))
    });
    let conversation = h.state.current().unwrap().id;
    let live_items = h.state.current().unwrap().items.len();
    h.stop();

    // No engine at all: a profile directory that holds no server.
    let nowhere = fx.root.join("nowhere");
    std::fs::create_dir_all(&nowhere).unwrap();
    std::fs::set_permissions(
        &nowhere,
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    let mut offline = Harness::start(direct_config(&nowhere, &fx.server_id), &db);
    offline.until("the saved conversation is shown", |s| {
        s.selected == Some(conversation)
            && s.current()
                .is_some_and(|c| c.cached_at.is_some() && !c.items.is_empty())
    });
    let c = offline.state.current().unwrap();
    assert!(!c.opened && c.cached_only);
    assert_eq!(
        c.items.len(),
        live_items,
        "the same messages that were on screen"
    );
    assert!(!offline.state.connection.is_ready());
    assert!(
        !offline.state.availability().submit,
        "nothing can be sent from a saved copy"
    );
    assert_eq!(offline.user_messages(), ["tell me about preserves"]);
    offline.until("the reason is shown", |s| {
        s.current().is_some_and(|c| c.stale_reason.is_some())
    });

    // Drafts stay editable offline.
    offline.dispatch(Command::EditDraft("a thought while away".into()));
    assert_eq!(
        offline.state.current().unwrap().draft.text,
        "a thought while away"
    );

    // Saved messages can be searched, and a result opens its message.
    offline.dispatch(Command::SetSearch("marmalade".into()));
    offline.until("the search finds the answer", |s| {
        s.history_search.query == "marmalade" && !s.history_search.hits.is_empty()
    });
    let r = &offline.state.history_search;
    assert_eq!(
        (r.hits[0].conversation, r.conversations_searched),
        (conversation, 1)
    );
    assert!(r.hits[0].snippet.contains('\u{2}') && r.hits[0].snippet.contains("marmalade"));
    offline.dispatch(Command::OpenSearchHit(0));
    assert_eq!(offline.state.scroll_target.map(|t| t.0), Some(conversation));
    assert!(offline.state.search.is_empty());
    // Quitting flushes the draft, as the application does.
    offline.dispatch(Command::FlushDraft(conversation));
    offline.stop();

    // The engine is back: its own state replaces the saved copy, and sending works again.
    let mut online = Harness::start(direct_config(&fx.server_dir, &fx.server_id), &db);
    online.until("the live conversation replaces the copy", |s| {
        s.selected == Some(conversation)
            && s.current()
                .is_some_and(|c| c.opened && c.cached_at.is_none() && !c.cached_only)
    });
    assert_eq!(online.state.current().unwrap().items.len(), live_items);
    assert_eq!(
        online.state.current().unwrap().draft.text,
        "a thought while away"
    );
    assert!(online.state.availability().submit);
    online.backend.shutdown();
}

/// The engine's extension-question service is real: a client sees its (empty) state and a
/// stale answer is refused with the engine's own reason.
#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn the_engines_extension_question_service_is_there_and_refuses_an_answer_to_nothing() {
    let fx = Fixture::start(nothing);
    init_project(&fx.project);
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(direct_config(&fx.server_dir, &fx.server_id), &db);
    h.open_new_conversation(&fx.project);
    let raw = RawSession::attach(&fx);
    let ui = raw
        .client
        .subscribe(
            &raw.target,
            "pi.ui-requests",
            pi_client::chord::Mode::Singleton,
            Duration::from_secs(30),
        )
        .expect("the engine serves pi.ui-requests");
    let state = ui
        .read(|r| r.state("state").cloned())
        .flatten()
        .expect("state");
    assert_eq!(state["requests"], json!([]));
    assert_eq!(state["status"], json!({}));
    let reply = raw
        .call("pi.ui-requests", "respond", vec![json!("q99"), json!("x")])
        .expect("a reply");
    assert_eq!(reply["accepted"], false);
    assert!(
        reply["reason"].as_str().unwrap().contains("no longer open"),
        "{reply}"
    );

    // Through Pipkin: the same refusal reaches the person's conversation.
    let conv = h.state.current().unwrap();
    assert!(conv.ui_requests.is_empty());
    h.backend.request(pipkin_core::BackendRequest::UiRespond {
        conversation: conv.id,
        generation: conv.generation,
        id: "q99".into(),
        answer: pipkin_core::UiAnswer::Choice("x".into()),
    });
    let deadline = Instant::now() + WAIT;
    while h.state.current().unwrap().ui_error.is_none() {
        h.pump();
        assert!(Instant::now() < deadline, "no refusal arrived");
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        h.state
            .current()
            .unwrap()
            .ui_error
            .as_deref()
            .unwrap()
            .contains("no longer open")
    );
    h.backend.shutdown();
}

fn questions_plugin() -> PathBuf {
    super::testsupport::pi_repo()
        .join("packages/coding-agent/examples/plugins/pi-example-questions")
}

fn first_question(s: &AppState) -> Option<pipkin_core::UiRequest> {
    s.current().and_then(|c| c.ui_requests.first().cloned())
}

fn answer(h: &mut Harness, id: &str, answer: pipkin_core::UiAnswer) {
    h.dispatch(Command::AnswerUiRequest {
        id: id.into(),
        answer,
    });
}

/// An extension in the engine asks three questions; Pipkin shows each, refuses a wrong answer
/// with the engine's own reason, and the extension continues with the right ones.
#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn an_extension_in_the_engine_asks_three_questions_and_the_answers_reach_it() {
    use pipkin_core::{UiAnswer, UiRequestKind};
    let plugin = questions_plugin();
    let fx = Fixture::start_with(nothing, |c| c.extensions = vec![plugin]);
    init_project(&fx.project);
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(direct_config(&fx.server_dir, &fx.server_id), &db);
    h.open_new_conversation(&fx.project);

    h.until("the first question arrives", |s| {
        first_question(s).is_some()
    });
    let q = first_question(&h.state).unwrap();
    assert_eq!(
        (q.kind, q.title.as_str()),
        (UiRequestKind::Select, "Pick a flavor")
    );
    assert_eq!(
        q.items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(),
        ["Mint", "Vanilla", "Pepper"]
    );
    assert_eq!(q.items[0].description.as_deref(), Some("cool and sharp"));
    assert!(q.deadline.is_some(), "the engine says when it gives up");

    // A choice that is not on offer is refused by the engine, and the question stays.
    answer(&mut h, &q.id, UiAnswer::Choice("lemon".into()));
    h.until("the refusal arrives", |s| {
        s.current().unwrap().ui_error.is_some()
    });
    assert!(
        h.state
            .current()
            .unwrap()
            .ui_error
            .as_deref()
            .unwrap()
            .contains("not one of the choices")
    );
    assert_eq!(first_question(&h.state).unwrap().id, q.id);
    assert!(h.state.current().unwrap().ui_answering.is_empty());

    answer(&mut h, &q.id, UiAnswer::Choice("mint".into()));
    h.until("the second question replaces it", |s| {
        first_question(s).is_some_and(|q| q.title == "Keep going?")
    });
    let q2 = first_question(&h.state).unwrap();
    assert_eq!(q2.kind, UiRequestKind::Confirm);
    answer(&mut h, &q2.id, UiAnswer::Confirm(true));
    h.until("the third question", |s| {
        first_question(s).is_some_and(|q| q.kind == UiRequestKind::Input)
    });
    let q3 = first_question(&h.state).unwrap();
    assert_eq!(q3.default_value.as_deref(), Some("pipsqueak"));
    assert_eq!(q3.placeholder.as_deref(), Some("a name"));
    answer(&mut h, &q3.id, UiAnswer::Text("Ada".into()));

    h.until("the extension has heard every answer", |s| {
        let c = s.current().unwrap();
        c.ui_requests.is_empty()
            && c.ui_notices.iter().any(|n| n.message == "Called Ada.")
            && c.ui_status
                .iter()
                .any(|(k, v)| k == "example-questions" && v == "flavor mint")
    });
    let messages: Vec<String> = h
        .state
        .current()
        .unwrap()
        .ui_notices
        .iter()
        .map(|n| n.message.clone())
        .collect();
    assert_eq!(
        messages,
        ["You picked mint.", "Keep going: true.", "Called Ada."],
        "in the order the extension reported them"
    );
    h.dispatch(Command::DismissUiNotices);
    assert!(h.state.current().unwrap().ui_notices.is_empty());
    h.backend.shutdown();
}

/// A question the person declines ends for the extension too; one left open when the link drops
/// is not shown as answerable while the engine is unreachable, and is there again after reconnecting.
#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn declining_ends_the_question_and_a_lost_connection_does_not_leave_it_answerable() {
    use pipkin_core::UiAnswer;
    let plugin = questions_plugin();
    let gate = Gate::new();
    let fx = Fixture::start_with(held_first(&gate, Reply::Text("x".into()), vec![]), |c| {
        c.extensions = vec![plugin]
    });
    init_project(&fx.project);
    let proxy = FaultProxy::start(
        fx.server_dir.join(format!("{}.sock", fx.server_id)),
        fx.root.join("proxy"),
        &fx.server_id,
    );
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(direct_config(&proxy.dir, &fx.server_id), &db);
    h.open_new_conversation(&fx.project);
    h.until("a question", |s| first_question(s).is_some());
    let q = first_question(&h.state).unwrap();

    proxy.sever_all();
    h.until("the loss is noticed", |s| !s.connection.is_ready());
    assert!(
        h.state.current().unwrap().ui_requests.is_empty(),
        "nothing is offered to answer while the engine cannot be reached"
    );
    assert!(
        h.state
            .dispatch(Command::AnswerUiRequest {
                id: q.id.clone(),
                answer: UiAnswer::Choice("mint".into())
            })
            .effects
            .is_empty()
    );
    h.until("reconnected and the question is back", |s| {
        s.connection.is_ready() && first_question(s).is_some_and(|b| b.id == q.id)
    });

    // Declining: the extension is told there was no answer and goes on to its next question.
    h.dispatch(Command::CancelUiRequest(q.id.clone()));
    h.until("the next question", |s| {
        first_question(s).is_some_and(|n| n.title == "Keep going?")
    });
    assert!(
        h.state
            .current()
            .unwrap()
            .ui_notices
            .iter()
            .any(|n| n.message == "No flavor was picked.")
    );
    gate.open();
    h.backend.shutdown();
}

/// Not a test: builds a profile that the real application can be pointed at for looking at it,
/// with what a few hours of use would leave: saved conversations, a long tool result, answered
/// extension questions. Run it with `PIPKIN_SEED_ROOT=/tmp/pk-seed` (a short path) and
/// `PIPKIN_PI_REPO`; it prints how to start the application against the result.
#[test]
#[ignore = "builds a profile for looking at the application; needs PIPKIN_SEED_ROOT and PIPKIN_PI_REPO"]
fn seed_a_profile_to_look_at() {
    use pipkin_core::{UiAnswer, UiRequestKind};
    // Only when asked: running the whole suite must not build a profile.
    let Ok(root) = std::env::var("PIPKIN_SEED_ROOT") else {
        eprintln!("PIPKIN_SEED_ROOT is not set; nothing to build");
        return;
    };
    let root = PathBuf::from(root);
    let _ = std::fs::remove_dir_all(&root);
    let (server, agent, data, project) = (
        root.join("server"),
        root.join("agent"),
        root.join("data"),
        root.join("project"),
    );
    for d in [&server, &agent, &data, &project] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::set_permissions(&server, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    init_project(&project);
    let provider = super::testsupport::StubProvider::start(|request, _| {
        let last = request["messages"]
            .as_array()
            .and_then(|m| m.last())
            .cloned()
            .unwrap_or(Value::Null);
        if last["role"] == "tool" {
            return Reply::Text(
                "Counted all the way to 3000; the full output is in the tool call above.".into(),
            );
        }
        let text = last["content"].to_string();
        if text.contains("count") {
            Reply::Tool {
                lead: Some("Counting. ".into()),
                name: "bash".into(),
                args: json!({ "command": "seq 1 3000" }),
            }
        } else {
            Reply::Text("Here is a marmalade recipe: simmer the fruit with sugar until it sets, then jar it while hot.".into())
        }
    });
    std::fs::write(
        agent.join("models.json"),
        json!({ "providers": { "stub": {
            "baseUrl": provider.base_url(), "api": "openai-completions", "apiKey": "stub",
            "models": [{ "id": "scripted", "name": "Scripted stub", "input": ["text", "image"] }],
        }}})
        .to_string(),
    )
    .unwrap();
    let server_id = std::fs::read_to_string("/proc/sys/kernel/random/uuid")
        .unwrap()
        .trim()
        .to_owned();
    std::fs::write(server.join("default-server-id"), &server_id).unwrap();
    let config = super::engine::EngineConfig {
        pi_repo: super::testsupport::pi_repo(),
        server_dir: server.clone(),
        server_id: server_id.clone(),
        agent_dir: Some(agent.clone()),
        cwd: root.clone(),
        model: Some(("stub".into(), "scripted".into())),
        log_path: root.join("engine.log"),
        env: vec![("PI_OFFLINE".into(), "1".into())],
        extensions: vec![questions_plugin()],
    };
    let mut host = EngineHost::new(config);
    host.ensure_running().expect("the engine starts");
    let namespace = format!(
        "pi:{:x}",
        pipkin_core::stable_id(&server.display().to_string())
    );
    let db = data.join("pipkin.sqlite3");
    let mut h = Harness::start_in(direct_config(&server, &server_id), &db, &namespace);
    h.open_new_conversation(&project);
    // The extension's questions, answered.
    h.until("a question", |s| first_question(s).is_some());
    for _ in 0..3 {
        let q = first_question(&h.state).unwrap();
        let a = match q.kind {
            UiRequestKind::Select => UiAnswer::Choice("mint".into()),
            UiRequestKind::Confirm => UiAnswer::Confirm(true),
            UiRequestKind::Input => UiAnswer::Text("Ada".into()),
        };
        answer(&mut h, &q.id, a);
        h.until("the question is taken", |s| {
            first_question(s).is_none_or(|n| n.id != q.id)
        });
    }
    h.send("tell me about marmalade");
    h.until("answered", |s| {
        is_idle(s) && assistant_texts(s).iter().any(|t| t.contains("marmalade"))
    });
    h.send("count the numbers up to three thousand");
    h.until("counted", |s| {
        is_idle(s) && assistant_texts(s).iter().any(|t| t.starts_with("Counted"))
    });
    // Let the saved copy be written, then leave as the application does.
    thread::sleep(Duration::from_millis(500));
    h.stop();
    host.stop();
    drop(provider);
    println!("SEED_ROOT={}", root.display());
    println!("SEED_SERVER_ID={server_id}");
    println!("SEED_NAMESPACE={namespace}");
}

/// Resident memory in KiB of the given processes, from `/proc`.
fn rss_kib(pids: &[u32]) -> u64 {
    pids.iter()
        .filter_map(|pid| std::fs::read_to_string(format!("/proc/{pid}/statm")).ok())
        .filter_map(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
        .map(|pages| pages * 4)
        .sum()
}

fn open_fds() -> usize {
    std::fs::read_dir("/proc/self/fd").map_or(0, Iterator::count)
}

/// A soak: many prompts through one conversation of a real engine, watching that memory and open
/// files stop growing. `PIPKIN_SOAK_ROUNDS` (default 40) sets the length; a long run is the same
/// test with a bigger number. Not a leak proof, a tripwire for steady growth per prompt.
#[test]
#[ignore = "needs a real Pi engine: set PIPKIN_PI_REPO"]
fn many_prompts_in_one_conversation_do_not_grow_memory_or_open_files() {
    let rounds: usize = std::env::var("PIPKIN_SOAK_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(40);
    let fx = Fixture::start(|_, n| Reply::Text(format!("answer {n}: {}", "words ".repeat(40))));
    init_project(&fx.project);
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(direct_config(&fx.server_dir, &fx.server_id), &db);
    h.open_new_conversation(&fx.project);

    let warmup = (rounds / 4).max(3).min(rounds);
    let (mut engine_base, mut app_base, mut fds_base) = (0, 0, 0);
    let (mut engine_peak, mut app_peak, mut fds_peak) = (0, 0, 0);
    for round in 1..=rounds {
        h.send(&format!("prompt {round}"));
        h.until("the answer arrives", |s| {
            is_idle(s) && assistant_texts(s).len() >= round
        });
        fx.provider.forget_requests();
        let engine = rss_kib(&profile_pids(&fx.server_dir, &fx.server_id));
        let app = rss_kib(&[std::process::id()]);
        let fds = open_fds();
        if round == warmup {
            (engine_base, app_base, fds_base) = (engine, app, fds);
        }
        if round > warmup {
            engine_peak = engine_peak.max(engine);
            app_peak = app_peak.max(app);
            fds_peak = fds_peak.max(fds);
        }
    }
    println!(
        "soak {rounds} prompts: engine {engine_base} -> {engine_peak} KiB, app {app_base} -> {app_peak} KiB, fds {fds_base} -> {fds_peak}"
    );
    assert_eq!(h.user_messages().len(), rounds);
    // Generous bounds: transcripts legitimately grow, so allow a little per prompt, but not a
    // multiple of the baseline or a file handle per prompt.
    let per_prompt = 600; // KiB
    let allowed = (rounds - warmup) as u64 * per_prompt + 64 * 1024;
    assert!(
        engine_peak <= engine_base + allowed,
        "engine memory grew {engine_base} -> {engine_peak} KiB over {rounds} prompts"
    );
    assert!(
        app_peak <= app_base + allowed,
        "app memory grew {app_base} -> {app_peak} KiB over {rounds} prompts"
    );
    assert!(
        fds_peak <= fds_base + 8,
        "open files grew {fds_base} -> {fds_peak}"
    );
    h.storage.shutdown();
    h.backend.shutdown();
}
