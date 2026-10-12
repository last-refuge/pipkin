//! End-to-end workflows: the real core state machine, the real `NativeBackend`, the real SQLite
//! journal, and the real engine running in-process with a scripted provider (only the model's
//! answers are scripted). Ported from the Pi suite (`adapters/pi/e2e.rs`); these run by default.
//!
//! The engine outlives each harness (as a Pi server outlives a window): a harness connects to
//! it through a fault link that can drop a prompt's reply, or the prompt itself, and the engine
//! can be restarted under a connected harness to exercise recovery.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use pipkin_core::{
    AppState, Backend, BackendEvent, Bootstrap, ChangesState, Command, Connection, ConversationId,
    Delivery, Effect, ImportFlow, ImportKind, ItemKind, LifecycleEvent, Mode, Outcome, QueueMode,
    RequestId, RunState, describe_attachment,
};
use pipkin_engine::config::{EngineConfig, Env};
use pipkin_engine::provider::ProviderRequest;
use pipkin_engine::provider::scripted::{self, Gate, Reply, Scripted};
use pipkin_engine::runtime::{EngineHandle, registry_with};
use pipkin_engine_protocol::{Block, Call, ClientFrame, Message, ServerFrame};
use serde_json::json;

use super::client::{FrameReceiver, FrameSender, Link};
use super::supervisor::HostSpec;
use super::{ImportResult, NativeBackend, NativeConfig};
use crate::controller::{Restore, copy_imported};
use crate::storage::Storage;

const NAMESPACE: &str = "native:e2e";
/// The desktop namespace Pi conversations had before they were imported.
const PI_NAMESPACE: &str = "pi:e2e";
const WAIT: Duration = Duration::from_secs(30);

// ------------------------------------------------------------------------------- the engine

/// What the fault link does to the next prompt, steer or follow-up.
#[derive(Default)]
struct Faults {
    drop_events: AtomicBool,
    refuse_lookups: AtomicUsize,
    drop_prompt_reply: AtomicBool,
    drop_prompt: AtomicBool,
    /// Let the next steer or follow-up reach the engine, then drop its reply and sever.
    drop_queue_reply: AtomicBool,
    /// Sever the link instead of delivering the next steer or follow-up.
    drop_queue: AtomicBool,
    fired: AtomicUsize,
    /// Reply ids to swallow.
    swallow: Mutex<HashSet<u64>>,
    /// Reply ids that sever the link when they arrive (instead of being delivered).
    sever_on: Mutex<HashSet<u64>>,
}

struct FaultSender {
    inner: pipkin_engine::host::Sender,
    faults: Arc<Faults>,
    /// This link was cut (shared with its receiver).
    severed: Arc<AtomicBool>,
}

impl FrameSender for FaultSender {
    fn send(&mut self, frame: ClientFrame) -> Result<(), String> {
        if self.severed.load(Ordering::SeqCst) {
            return Err("the link was cut".into());
        }
        if matches!(frame.call, Call::Prompt { .. }) {
            if self.faults.drop_prompt.swap(false, Ordering::SeqCst) {
                self.faults.fired.fetch_add(1, Ordering::SeqCst);
                return Ok(()); // "sent", and lost on the way
            }
            if self.faults.drop_prompt_reply.swap(false, Ordering::SeqCst) {
                self.faults.fired.fetch_add(1, Ordering::SeqCst);
                self.faults.swallow.lock().unwrap().insert(frame.id);
            }
        }
        if matches!(frame.call, Call::Enqueue { .. }) {
            if self.faults.drop_queue.swap(false, Ordering::SeqCst) {
                self.faults.fired.fetch_add(1, Ordering::SeqCst);
                self.severed.store(true, Ordering::SeqCst);
                return Ok(()); // written, and the connection died with it
            }
            if self.faults.drop_queue_reply.swap(false, Ordering::SeqCst) {
                self.faults.fired.fetch_add(1, Ordering::SeqCst);
                self.faults.sever_on.lock().unwrap().insert(frame.id);
            }
        }
        self.inner.send(frame)
    }
}

struct FaultReceiver {
    inner: pipkin_engine::delivery::Receiver,
    faults: Arc<Faults>,
    severed: Arc<AtomicBool>,
}

impl FrameReceiver for FaultReceiver {
    fn recv(&mut self) -> Result<ServerFrame, String> {
        loop {
            if self.severed.load(Ordering::SeqCst) {
                return Err("the link was cut".into());
            }
            let frame = match self.inner.recv_timeout(Duration::from_millis(20)) {
                Ok(frame) => frame,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("the engine stopped".to_owned());
                }
            };
            if matches!(&frame, ServerFrame::Event { .. })
                && self.faults.drop_events.load(Ordering::SeqCst)
            {
                self.faults.fired.fetch_add(1, Ordering::SeqCst);
                continue;
            }
            if let ServerFrame::Reply {
                id,
                result: Ok(pipkin_engine_protocol::Reply::Lookup(_)),
            } = &frame
                && self
                    .faults
                    .refuse_lookups
                    .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                    .is_ok()
            {
                return Ok(ServerFrame::Reply {
                    id: *id,
                    result: Err(pipkin_engine_protocol::ProtoError::new(
                        pipkin_engine_protocol::ErrorCode::Storage,
                        "transient lookup failure",
                    )),
                });
            }
            if let ServerFrame::Reply { id, .. } = &frame {
                if self.faults.swallow.lock().unwrap().remove(id) {
                    continue;
                }
                if self.faults.sever_on.lock().unwrap().remove(id) {
                    self.severed.store(true, Ordering::SeqCst);
                    return Err("the link was cut".into());
                }
            }
            return Ok(frame);
        }
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    project: PathBuf,
    provider: Scripted,
    engine: Arc<Mutex<Option<Arc<EngineHandle>>>>,
    faults: Arc<Faults>,
    /// No built-in scripted model: models come only from the engine's `models.json`.
    bare: bool,
}

impl Fixture {
    fn start(script: impl Fn(&ProviderRequest, usize) -> Reply + Send + Sync + 'static) -> Fixture {
        Fixture::start_with(script, false)
    }

    fn start_with(
        script: impl Fn(&ProviderRequest, usize) -> Reply + Send + Sync + 'static,
        bare: bool,
    ) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let project = root.join("project");
        std::fs::create_dir_all(&project).unwrap();
        let fx = Fixture {
            _dir: dir,
            root,
            project,
            provider: Scripted::new(script),
            engine: Arc::new(Mutex::new(None)),
            faults: Arc::new(Faults::default()),
            bare,
        };
        fx.restart_engine();
        fx
    }

    fn config(&self) -> EngineConfig {
        let mut c = EngineConfig::new(self.root.join("engine"), self.root.join("engine-config"));
        // The Pi profile an import reads (the engine's default agent directory).
        c.env = Env::isolated(
            [
                (
                    "PI_CODING_AGENT_DIR".to_owned(),
                    self.pi_agent().display().to_string(),
                ),
                (
                    "PI_SERVER_DIR".to_owned(),
                    self.root.join("pi-server").display().to_string(),
                ),
            ]
            .into(),
        );
        if !self.bare {
            c.extra_models = vec![scripted::model()];
            c.default_model = Some("scripted/scripted".into());
        }
        c
    }

    /// The running engine (tests reach into its store to inject faults).
    fn engine(&self) -> Arc<EngineHandle> {
        self.engine
            .lock()
            .unwrap()
            .clone()
            .expect("the engine runs")
    }

    /// Stop the engine (if running) and start it again on the same data: connected harnesses
    /// see their connection end and reconnect.
    fn restart_engine(&self) {
        // Hold the slot across the restart: a reconnect in the gap waits for the new engine
        // instead of failing (which would count against the supervisor's restart cap).
        let mut slot = self.engine.lock().unwrap();
        if let Some(old) = slot.take() {
            old.shutdown(Duration::from_secs(5));
        }
        let handle = EngineHandle::start_with(
            self.config(),
            registry_with(Arc::new(self.provider.clone())),
            pipkin_engine::tools::builtin_tools(),
        )
        .expect("engine starts");
        *slot = Some(Arc::new(handle));
    }

    fn host(&self) -> HostSpec {
        let engine = self.engine.clone();
        let faults = self.faults.clone();
        HostSpec::Fake(Arc::new(move || {
            let handle = engine
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| "the engine is not running".to_owned())?;
            let (sender, receiver) = handle.connect("e2e").split();
            let severed = Arc::new(AtomicBool::new(false));
            Ok(Link {
                sender: Box::new(FaultSender {
                    inner: sender,
                    faults: faults.clone(),
                    severed: severed.clone(),
                }),
                receiver: Box::new(FaultReceiver {
                    inner: receiver,
                    faults: faults.clone(),
                    severed,
                }),
            })
        }))
    }

    fn native(&self, call_timeout: Duration) -> NativeConfig {
        let mut config =
            NativeConfig::new(self.host(), self.root.join("engine").display().to_string());
        config.retry_delay = Duration::from_millis(50);
        config.call_timeout = call_timeout;
        config
    }

    fn requests(&self) -> Vec<ProviderRequest> {
        self.provider.requests()
    }

    fn pi_agent(&self) -> PathBuf {
        self.root.join("pi-agent")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(engine) = self.engine.lock().unwrap().take() {
            engine.shutdown(Duration::from_secs(5));
        }
    }
}

// ------------------------------------------------------------------------------ the harness

/// The application as the controller composes it, minus the window.
struct Harness {
    state: AppState,
    backend: NativeBackend,
    storage: Arc<Storage>,
    restore: Restore,
    events: crate::event_queue::EventReceiver,
    life: Arc<Mutex<VecDeque<LifecycleEvent>>>,
    acks: mpsc::Receiver<(ConversationId, RequestId, Result<(), String>)>,
    ack_tx: mpsc::Sender<(ConversationId, RequestId, Result<(), String>)>,
    notices: Vec<String>,
    copied: Vec<String>,
    imports: Arc<Mutex<VecDeque<ImportResult>>>,
}

fn unique(prefix: &str) -> String {
    static N: AtomicUsize = AtomicUsize::new(0);
    format!(
        "{prefix}{}x{}",
        N.fetch_add(1, Ordering::SeqCst),
        std::process::id()
    )
}

impl Harness {
    fn start(fx: &Fixture, db: &Path) -> Harness {
        Harness::start_with(fx.native(Duration::from_secs(10)), db)
    }

    fn start_with(config: NativeConfig, db: &Path) -> Harness {
        Self::start_with_limits(
            config,
            db,
            crate::event_queue::NATIVE_EVENTS,
            crate::event_queue::NATIVE_BYTES,
        )
    }

    fn start_with_limits(config: NativeConfig, db: &Path, count: usize, bytes: usize) -> Harness {
        let storage = Arc::new(Storage::open(db, NAMESPACE).expect("storage"));
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
        for saved in storage.cached_conversations().expect("saved conversations") {
            state.restore_cached_conversation(
                saved.id,
                &saved.meta.project_path,
                saved.meta.title,
                saved.meta.updated_at,
            );
        }
        let restore = Restore::new(loaded, false);
        let (tx, events) = crate::event_queue::bounded(count, bytes);
        let backend = NativeBackend::new(tx, config);
        let life = Arc::new(Mutex::new(VecDeque::new()));
        let sink = {
            let life = life.clone();
            Box::new(move |e| life.lock().unwrap().push_back(e))
        };
        backend.start(sink);
        let (ack_tx, acks) = mpsc::channel();
        let mut h = Harness {
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
            imports: Arc::default(),
        };
        let mut out = h.restore.apply(&mut h.state);
        out.merge(h.state.select_initial());
        h.exec(out);
        h
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
                    let (tx, rx) = mpsc::channel();
                    self.storage
                        .save_draft(conversation, text, attachments, rev, move |r| {
                            let _ = tx.send(r);
                        });
                    let result = rx.recv_timeout(Duration::from_secs(10)).unwrap();
                    self.state.draft_saved(conversation, rev, result);
                }
                Effect::SavePrefs(prefs) | Effect::CompleteSetup(prefs) => {
                    self.storage.save_prefs(&prefs)
                }
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
                Effect::SaveText { .. } | Effect::Launch(_) => {}
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
                Effect::ReleaseOwnership { conversation } => {
                    let result = self.storage.release_ownership(conversation).map(|_| ());
                    let out = self.state.ownership_released(conversation, result);
                    self.exec(out);
                }
                // As the controller does: the backend answers on its own thread.
                Effect::Import(step) => {
                    let imports = self.imports.clone();
                    self.backend.import(
                        step,
                        Box::new(move |r| imports.lock().unwrap().push_back(r)),
                    );
                }
            }
        }
    }

    fn dispatch(&mut self, command: Command) {
        let outcome = self.state.dispatch(command);
        self.exec(outcome);
    }

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
        while let Ok(mut lease) = self.events.try_recv() {
            let outcome = self.state.apply_event(lease.take());
            self.exec(outcome);
        }
        while let Ok((conversation, request, result)) = self.acks.try_recv() {
            let outcome = self.state.intent_persisted(conversation, &request, result);
            self.exec(outcome);
        }
        let imports: Vec<ImportResult> = self.imports.lock().unwrap().drain(..).collect();
        for result in imports {
            let outcome = match result {
                ImportResult::Offer(offer) => self.state.import_discovered(offer),
                ImportResult::Finished {
                    message,
                    imported,
                    existing,
                } => {
                    let (report, loaded) =
                        copy_imported(&self.storage, PI_NAMESPACE, &imported, &existing, 1_700)
                            .expect("the desktop data is copied");
                    self.restore.add_imported(loaded, &imported);
                    let mut out = self.restore.apply(&mut self.state);
                    out.merge(
                        self.state
                            .import_finished(format!("{message}\ncopied: {report:?}")),
                    );
                    out
                }
            };
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
                "timed out waiting for: {what}\n  connection: {:?}\n  notices: {:?}\n  run: {:?}\n  items: {:?}",
                self.state.connection,
                self.notices,
                self.state.current().map(|c| c.run.clone()),
                self.state.current().map(|c| c.items.clone()),
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn open_new_conversation(&mut self, project: &Path) {
        self.until("connected", |s| s.connection.is_ready());
        let real = std::fs::canonicalize(project).unwrap();
        self.dispatch(Command::AddProject(real.display().to_string()));
        self.dispatch(Command::NewConversation);
        self.until("the new conversation is open", |s| {
            s.current().is_some_and(|c| c.opened)
        });
    }

    fn until_asked(&mut self, fx: &Fixture, n: usize) {
        self.until("the engine asked the model", |_| fx.requests().len() >= n);
    }

    fn send(&mut self, text: &str) {
        self.dispatch(Command::EditDraft(text.into()));
        self.dispatch(Command::Submit);
    }

    fn user_messages(&self) -> Vec<String> {
        user_messages(&self.state)
    }

    fn stop(self) {
        self.backend.shutdown();
        self.storage.shutdown();
    }
}

fn user_messages(s: &AppState) -> Vec<String> {
    s.current()
        .map(|c| {
            c.items
                .iter()
                .filter_map(|i| match &i.kind {
                    ItemKind::User { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

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
fn user_texts(request: &ProviderRequest) -> Vec<String> {
    request
        .messages
        .iter()
        .filter(|m| matches!(m, Message::User { .. }))
        .map(|m| m.text())
        .collect()
}

fn held_first(
    gate: &Gate,
    first: Reply,
    rest: Vec<Reply>,
) -> impl Fn(&ProviderRequest, usize) -> Reply + Send + Sync + 'static {
    let gate = gate.clone();
    move |_, n| match n {
        0 => Reply::Gated(gate.clone(), Box::new(first.clone())),
        n => rest
            .get(n - 1)
            .cloned()
            .unwrap_or_else(|| Reply::Text(format!("extra turn {n}"))),
    }
}

fn open_requests(db: &Path) -> usize {
    Storage::open(db, NAMESPACE)
        .unwrap()
        .load_all()
        .unwrap()
        .open_requests
        .len()
}

// -------------------------------------------------------------------------------- workflows

#[test]
fn a_prompt_streams_settles_and_reopens_with_the_same_history() {
    let fx = Fixture::start(|_, _| Reply::Text("hello there, friend".into()));
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(&fx, &db);
    h.open_new_conversation(&fx.project);
    let conversation = h.state.current().unwrap().id;
    h.send("say hi");
    h.until("answered", |s| {
        is_idle(s) && assistant_texts(s) == ["hello there, friend".to_owned()]
    });
    assert_eq!(h.user_messages(), ["say hi"]);
    let c = h.state.current().unwrap();
    assert!(
        matches!(
            &c.items[0].kind,
            ItemKind::User {
                delivery: Delivery::Sent,
                ..
            }
        ),
        "{:?}",
        c.items
    );
    let usage = c.usage.as_ref().expect("the engine's ledger");
    assert!(usage.models.iter().any(|(k, _)| k == "scripted/scripted"));
    let ids: Vec<_> = c.items.iter().map(|i| i.id).collect();
    assert_eq!(fx.requests().len(), 1);
    h.stop();
    assert_eq!(open_requests(&db), 0, "the journal entry closed");

    let mut h2 = Harness::start(&fx, &db);
    h2.until("the same conversation reopens", |s| {
        s.selected == Some(conversation) && s.current().is_some_and(|c| c.opened)
    });
    assert_eq!(h2.user_messages(), ["say hi"]);
    let again: Vec<_> = h2
        .state
        .current()
        .unwrap()
        .items
        .iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(again, ids, "item ids are stable across reopen");
    assert_eq!(fx.requests().len(), 1, "reopening sends nothing");
    h2.stop();
}

#[test]
fn a_dropped_acknowledgment_is_resolved_by_asking_and_never_runs_twice() {
    let fx = Fixture::start(|_, _| Reply::Text("done once".into()));
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start_with(fx.native(Duration::from_millis(700)), &db);
    h.open_new_conversation(&fx.project);
    fx.faults.drop_prompt_reply.store(true, Ordering::SeqCst);
    h.send("do it once");
    h.until("the outcome is unknown", |s| {
        matches!(s.current().unwrap().run, RunState::OutcomeUnknown { .. })
    });
    assert_eq!(fx.faults.fired.load(Ordering::SeqCst), 1);
    assert!(h.state.availability().check_status);
    assert!(!h.state.availability().retry, "never resent by default");
    h.dispatch(Command::CheckStatus);
    h.until("the engine says it ran", |s| {
        is_idle(s) && assistant_texts(s) == ["done once".to_owned()]
    });
    assert_eq!(fx.requests().len(), 1, "the prompt ran exactly once");
    assert_eq!(h.user_messages(), ["do it once"]);
    h.stop();
    assert_eq!(open_requests(&db), 0);
}

#[test]
fn a_lost_prompt_is_proven_unsent_and_only_an_explicit_retry_sends_it() {
    let fx = Fixture::start(|_, _| Reply::Text("finally".into()));
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start_with(fx.native(Duration::from_millis(700)), &db);
    h.open_new_conversation(&fx.project);
    fx.faults.drop_prompt.store(true, Ordering::SeqCst);
    h.send("maybe sent");
    h.until("the outcome is unknown", |s| {
        matches!(s.current().unwrap().run, RunState::OutcomeUnknown { .. })
    });
    h.dispatch(Command::CheckStatus);
    h.until("proven never admitted", |s| {
        matches!(s.current().unwrap().run, RunState::Failed { .. })
    });
    assert!(fx.requests().is_empty());
    assert!(h.state.availability().retry);
    h.dispatch(Command::Retry);
    h.until("the retry ran", |s| {
        is_idle(s) && assistant_texts(s) == ["finally".to_owned()]
    });
    assert_eq!(fx.requests().len(), 1);
    h.stop();
}

fn stopped_input_is_not_presented_to_the_next_model_turn(as_goal: bool) {
    let gate = Gate::new();
    let fx = Fixture::start(held_first(
        &gate,
        Reply::Text("old task not completed".into()),
        vec![Reply::Text("1 2 3 ... 100\n[PIPKIN_GOAL_MET]".into())],
    ));
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(&fx, &db);
    h.open_new_conversation(&fx.project);
    h.send("commit and push");
    h.until_asked(&fx, 1);
    h.until("the original request is running", is_running);
    h.dispatch(Command::EditDraft("also publish the old work".into()));
    h.dispatch(Command::QueueFollowUp);
    h.until("the old follow-up is queued", |s| {
        !s.current().unwrap().queue.is_empty()
    });
    h.dispatch(Command::Cancel);
    if as_goal {
        h.dispatch(Command::SetGoal("count to 100".into()));
    } else {
        h.until("stop is confirmed and old queued work is withdrawn", |s| {
            is_idle(s) && s.current().unwrap().queue.is_empty()
        });
        h.send("count to 100");
    }
    h.until("only the replacement finishes", |s| {
        is_idle(s) && assistant_texts(s).iter().any(|t| t.starts_with("1 2 3"))
    });
    let requests = fx.requests();
    assert_eq!(requests.len(), 2, "the withdrawn follow-up must not run");
    let users = user_texts(&requests[1]);
    assert_eq!(
        users.len(),
        1,
        "stopped instructions must not leak: {users:?}"
    );
    assert!(users[0].contains("count to 100"));
    assert!(
        h.user_messages().iter().any(|t| t == "commit and push"),
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
    gate.open();
    h.stop();
    assert_eq!(open_requests(&db), 0);
}

#[test]
fn stopping_a_prompt_flushes_its_instruction_before_a_new_prompt() {
    stopped_input_is_not_presented_to_the_next_model_turn(false);
}

#[test]
fn stopping_a_prompt_flushes_its_instruction_before_a_replacement_goal() {
    stopped_input_is_not_presented_to_the_next_model_turn(true);
}

#[test]
fn a_follow_up_waits_in_the_engines_queue_then_runs_by_itself() {
    let gate = Gate::new();
    let fx = Fixture::start(held_first(
        &gate,
        Reply::Text("first done".into()),
        vec![Reply::Text("second done".into())],
    ));
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(&fx, &db);
    h.open_new_conversation(&fx.project);
    h.send("first");
    h.until_asked(&fx, 1);
    h.until("running", is_running);
    h.dispatch(Command::EditDraft("second".into()));
    assert!(h.state.availability().queue);
    h.dispatch(Command::QueueFollowUp);
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
    let requests = fx.requests();
    assert_eq!(requests.len(), 2, "one model turn each");
    assert_eq!(user_texts(&requests[1]), ["first", "second"]);
    h.stop();
    assert_eq!(open_requests(&db), 0);
}

#[test]
fn a_queued_input_removed_before_it_runs_never_reaches_the_model() {
    let gate = Gate::new();
    let fx = Fixture::start(held_first(
        &gate,
        Reply::Text("first done".into()),
        vec![Reply::Text("kept done".into())],
    ));
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(&fx, &db);
    h.open_new_conversation(&fx.project);
    h.send("first");
    h.until_asked(&fx, 1);
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
    let first = h.state.current().unwrap().queue[0].id;
    h.dispatch(Command::RemoveQueued(first));
    h.until("the engine dropped it", |s| {
        queued(s) == [(QueueMode::FollowUp, "queued-keep-xq7".to_owned())]
    });
    gate.open();
    h.until("first and the kept input ran", |s| {
        is_idle(s) && assistant_texts(s) == ["first done".to_owned(), "kept done".to_owned()]
    });
    let requests = fx.requests();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert!(
            !user_texts(request)
                .iter()
                .any(|t| t.contains("queued-drop-xq7")),
            "a removed input must never be sent to the model"
        );
    }
    assert_eq!(user_texts(&requests[1]), ["first", "queued-keep-xq7"]);
    h.stop();
}

/// Steering joins the run in flight: the model's next turn in the same run sees it. The tool
/// call here is answered "not available" (no built-in tools yet), which still makes a second
/// model turn in the same run.
#[test]
fn a_steer_reaches_the_next_model_turn_of_the_active_run() {
    let gate = Gate::new();
    let fx = Fixture::start(held_first(
        &gate,
        Reply::Tools(
            Some("Working. ".into()),
            vec![(
                "bash".into(),
                serde_json::json!({ "command": "seq 1 20000" }),
            )],
        ),
        vec![Reply::Text("done, with tabs".into())],
    ));
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(&fx, &db);
    h.open_new_conversation(&fx.project);
    h.send("edit the notes");
    h.until_asked(&fx, 1);
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
    let requests = fx.requests();
    assert_eq!(requests.len(), 2, "one run, two model turns");
    assert_eq!(
        user_texts(&requests[1]),
        ["edit the notes", "use tabs"],
        "the second turn of the same run saw the steer"
    );
    assert_eq!(h.user_messages(), ["edit the notes", "use tabs"]);
    assert!(h.state.current().unwrap().queue.is_empty());
    h.stop();
    assert_eq!(open_requests(&db), 0);
}

#[test]
fn a_run_and_its_queue_are_picked_up_again_after_the_app_restarts() {
    let gate = Gate::new();
    let fx = Fixture::start(held_first(
        &gate,
        Reply::Text("first done".into()),
        vec![Reply::Text("second done".into())],
    ));
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(&fx, &db);
    h.open_new_conversation(&fx.project);
    let conversation = h.state.current().unwrap().id;
    h.send("first");
    h.until_asked(&fx, 1);
    h.until("running", is_running);
    h.dispatch(Command::EditDraft("second".into()));
    h.dispatch(Command::QueueFollowUp);
    h.until("queued", |s| {
        queued(s) == [(QueueMode::FollowUp, "second".to_owned())]
    });
    h.stop(); // the window closes; the engine and its run carry on

    let mut h2 = Harness::start(&fx, &db);
    h2.until("the run is found running again", |s| {
        s.selected == Some(conversation) && is_running(s) && !queued(s).is_empty()
    });
    assert_eq!(
        queued(&h2.state),
        [(QueueMode::FollowUp, "second".to_owned())]
    );
    assert_eq!(fx.requests().len(), 1, "nothing was resent");
    assert_eq!(h2.user_messages(), ["first"]);
    gate.open();
    h2.until("everything finishes", |s| {
        is_idle(s) && assistant_texts(s) == ["first done".to_owned(), "second done".to_owned()]
    });
    assert_eq!(fx.requests().len(), 2);
    assert_eq!(h2.user_messages(), ["first", "second"]);
    h2.stop();
    assert_eq!(open_requests(&db), 0);
}

/// The engine restarts under a connected window: the window reconnects, the engine resumes the
/// run from its durable state (asking the model again for the same turn), and the prompt is
/// neither resent by the app nor duplicated in the history.
#[test]
fn an_engine_restart_mid_run_reconnects_and_the_resumed_run_settles() {
    let fx = Fixture::start(|_, n| match n {
        0 => Reply::Hang,
        _ => Reply::Text("after restart".into()),
    });
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(&fx, &db);
    h.open_new_conversation(&fx.project);
    h.send("long job");
    h.until_asked(&fx, 1);
    h.until("running", is_running);
    fx.restart_engine();
    h.until("the resumed run settles, on the engine's word", |s| {
        s.connection.is_ready()
            && is_idle(s)
            && assistant_texts(s).contains(&"after restart".to_owned())
    });
    assert_eq!(h.user_messages(), ["long job"], "one prompt, once");
    let requests = fx.requests();
    assert_eq!(
        requests.len(),
        2,
        "the engine re-asked the model for the same turn"
    );
    assert_eq!(user_texts(&requests[1]), ["long job"]);
    h.stop();
    assert_eq!(open_requests(&db), 0);
}

#[test]
fn a_run_started_elsewhere_is_shown_and_can_be_stopped() {
    let fx = Fixture::start(|_, _| Reply::Hang);
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(&fx, &db);
    h.open_new_conversation(&fx.project);
    let conversation = h.state.current().unwrap().id;
    h.send("started here");
    h.until_asked(&fx, 1);
    h.until("running", is_running);
    h.stop();
    // Forget the journal entry, as if another client had started the run.
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute("UPDATE request_journal SET state = 'completed'", [])
            .unwrap();
    }
    let mut h2 = Harness::start(&fx, &db);
    h2.until("the busy run is shown", |s| {
        s.selected == Some(conversation) && is_running(s)
    });
    assert!(h2.state.availability().cancel);
    h2.dispatch(Command::Cancel);
    h2.until("it stops, on the engine's word", is_idle);
    h2.stop();
}

#[test]
fn switching_conversations_keeps_each_ones_run_and_settles_what_ended_while_away() {
    let gate = Gate::new();
    let fx = Fixture::start(held_first(
        &gate,
        Reply::Text("a is done".into()),
        vec![Reply::Text("b is done".into())],
    ));
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(&fx, &db);
    h.open_new_conversation(&fx.project);
    let a = h.state.current().unwrap().id;
    h.send("work in A");
    h.until_asked(&fx, 1);
    h.until("A is running", is_running);
    h.dispatch(Command::NewConversation);
    h.until("B is open", |s| {
        s.selected.is_some_and(|id| id != a) && s.current().is_some_and(|c| c.opened)
    });
    let b = h.state.selected.unwrap();
    assert!(is_idle(&h.state), "B has no run");
    h.send("work in B");
    h.until("B is answered", |s| {
        is_idle(s) && assistant_texts(s).contains(&"b is done".to_owned())
    });
    gate.open();
    h.until("A settles in the background", |s| {
        matches!(s.conversation(a).unwrap().run, RunState::Idle)
    });
    assert_eq!(h.state.selected, Some(b));
    assert_eq!(h.user_messages(), ["work in B"]);
    h.dispatch(Command::SelectConversation(a));
    h.until("A is open again and settled", |s| {
        s.selected == Some(a)
            && s.current().is_some_and(|c| c.opened)
            && is_idle(s)
            && assistant_texts(s).contains(&"a is done".to_owned())
    });
    assert_eq!(h.user_messages(), ["work in A"]);
    h.dispatch(Command::SelectConversation(b));
    h.until("B shows its own history", |s| {
        s.selected == Some(b)
            && s.current().is_some_and(|c| c.opened)
            && assistant_texts(s).contains(&"b is done".to_owned())
    });
    assert_eq!(h.user_messages(), ["work in B"]);
    h.stop();
    assert_eq!(open_requests(&db), 0);
}

/// A valid 1x1 PNG.
const TINY_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

#[test]
fn attachments_reach_the_model_as_text_and_image_and_come_back_as_chips() {
    use base64::Engine as _;
    let fx = Fixture::start(|_, _| Reply::Text("I read them.".into()));
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(&fx, &db);
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
    let requests = fx.requests();
    let user = requests[0]
        .messages
        .iter()
        .find(|m| matches!(m, Message::User { .. }))
        .unwrap();
    assert!(user.text().contains("review these") && user.text().contains("# Spec\nUse tabs."));
    assert!(
        user.content()
            .iter()
            .any(|b| matches!(b, Block::Image { mime_type, .. } if mime_type == "image/png")),
        "the image reached the model"
    );
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
    assert!(c.draft.attachments.is_empty());
    h.stop();
}

#[test]
fn effort_is_the_engines_and_is_restored_per_conversation() {
    let fx = Fixture::start(|_, _| Reply::Text("ok".into()));
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(&fx, &db);
    h.open_new_conversation(&fx.project);
    let a = h.state.current().unwrap().id;
    h.until("the engine reports effort levels", |s| {
        !s.current().unwrap().thinking_levels.is_empty()
    });
    let levels = h.state.current().unwrap().thinking_levels.clone();
    assert!(levels.contains(&"high".to_owned()), "{levels:?}");
    h.dispatch(Command::SetThinkingLevel("high".into()));
    h.until("A uses high effort", |s| {
        s.current().unwrap().thinking_level.as_deref() == Some("high")
    });
    h.dispatch(Command::NewConversation);
    h.until("B is open", |s| {
        s.selected.is_some_and(|id| id != a) && s.current().is_some_and(|c| c.opened)
    });
    h.until("B reports its own effort", |s| {
        s.current().unwrap().thinking_level.is_some()
    });
    h.dispatch(Command::SetThinkingLevel("low".into()));
    h.until("B uses low effort", |s| {
        s.current().unwrap().thinking_level.as_deref() == Some("low")
    });
    h.dispatch(Command::SelectConversation(a));
    h.until("A's effort is restored from the engine", |s| {
        s.selected == Some(a)
            && s.current().is_some_and(|c| c.opened)
            && s.current().unwrap().thinking_level.as_deref() == Some("high")
    });
    h.send("think hard");
    h.until("answered", |s| is_idle(s) && !assistant_texts(s).is_empty());
    assert_eq!(
        fx.requests()[0].thinking.as_str(),
        "high",
        "the model was asked with A's effort"
    );
    h.stop();
}

#[test]
fn the_complete_tool_output_is_copied_from_the_engine() {
    let fx = Fixture::start(|_, n| match n {
        0 => Reply::Tools(
            Some("Running it. ".into()),
            vec![(
                "bash".into(),
                serde_json::json!({ "command": "seq 1 20000" }),
            )],
        ),
        _ => Reply::Text("done".into()),
    });
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(&fx, &db);
    h.open_new_conversation(&fx.project);
    h.send("list files");
    h.until("answered", |s| {
        is_idle(s) && assistant_texts(s).contains(&"done".to_owned())
    });
    let tool = h
        .state
        .current()
        .unwrap()
        .items
        .iter()
        .find(|i| matches!(i.kind, ItemKind::Tool(_)))
        .expect("the tool call is shown")
        .clone();
    let ItemKind::Tool(call) = &tool.kind else {
        unreachable!()
    };
    assert_eq!(call.name, "bash");
    assert_ne!(
        call.status,
        pipkin_core::ToolStatus::Running,
        "paired with its result"
    );
    h.dispatch(Command::CopyToolOutput(tool.id));
    let deadline = Instant::now() + WAIT;
    while h.copied.is_empty() && Instant::now() < deadline {
        h.pump();
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(h.copied.len(), 1, "notices: {:?}", h.notices);
    assert!(!h.copied[0].is_empty());
    // The engine retained the complete output, beyond the model-visible 50 KiB window.
    assert!(
        h.copied[0].starts_with("1\n2\n3\n"),
        "{:?}",
        &h.copied[0][..20]
    );
    assert!(h.copied[0].trim_end().ends_with("20000"));
    // The preview is the model-visible result (bash keeps the tail window plus the harness
    // remarks); the copy is the engine's retained complete output, so it holds more.
    assert!(h.copied[0].len() > call.output.len());
    h.stop();
}

#[test]
fn older_history_pages_in_and_stays_through_later_updates() {
    let fx = Fixture::start(|_, n| Reply::Text(format!("answer {n}")));
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(&fx, &db);
    h.open_new_conversation(&fx.project);
    let conversation = h.state.current().unwrap().id;
    // More than the engine's default window of 200 entries.
    for n in 0..105 {
        h.send(&format!("prompt {n}"));
        let want = format!("answer {n}");
        h.until("answered", |s| {
            is_idle(s) && assistant_texts(s).last() == Some(&want)
        });
    }
    h.stop();
    let mut h2 = Harness::start(&fx, &db);
    h2.until("reopened with a window", |s| {
        s.selected == Some(conversation) && s.current().is_some_and(|c| c.opened)
    });
    let shown = h2.user_messages().len();
    assert!(shown < 105, "only a window is shown first ({shown})");
    assert!(h2.state.current().unwrap().has_older);
    while h2.state.current().unwrap().has_older {
        let before = h2.user_messages().len();
        h2.dispatch(Command::LoadOlder);
        h2.until("an older page arrives", |s| {
            user_messages(s).len() > before || !s.current().unwrap().has_older
        });
    }
    assert_eq!(h2.user_messages().len(), 105);
    assert_eq!(h2.user_messages()[0], "prompt 0");
    // A new turn does not drop what was paged in.
    h2.send("one more");
    h2.until("answered", |s| {
        is_idle(s) && assistant_texts(s).last() == Some(&"answer 105".to_owned())
    });
    assert_eq!(h2.user_messages().len(), 106);
    let ids: Vec<_> = h2
        .state
        .current()
        .unwrap()
        .items
        .iter()
        .map(|i| i.id.0)
        .collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len(), "unique item ids");
    h2.stop();
}

/// Needs the engine's built-in `write` tool (the tools worker); until then the tool call is
/// answered "not available" and nothing changes on disk.
#[test]
fn a_session_edit_shows_in_changes_until_it_is_committed() {
    let fx = Fixture::start(|_, n| match n {
        0 => Reply::Tools(
            None,
            vec![(
                "write".into(),
                serde_json::json!({ "path": "notes.txt", "content": "one\ntwo\n" }),
            )],
        ),
        _ => Reply::Text("Done.".into()),
    });
    git(&fx.project, &["init", "-q", "-b", "main"]);
    std::fs::write(fx.project.join("notes.txt"), "one\n").unwrap();
    git(&fx.project, &["add", "-A"]);
    git(&fx.project, &["commit", "-q", "-m", "init"]);
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(&fx, &db);
    h.open_new_conversation(&fx.project);
    h.send("update the notes");
    h.until("the edit shows in Changes", |s| {
        is_idle(s)
            && s.current()
                .unwrap()
                .changes
                .iter()
                .any(|f| f.path == "notes.txt")
    });
    git(&fx.project, &["commit", "-qam", "session"]);
    h.dispatch(Command::RefreshChanges);
    h.until("committed work leaves Changes", |s| {
        s.current().unwrap().changes.is_empty()
    });
    h.stop();
}

fn init_project(project: &Path) {
    git(project, &["init", "-q", "-b", "main"]);
    std::fs::write(project.join("README.md"), "# Fixture project\n").unwrap();
    git(project, &["add", "-A"]);
    git(project, &["commit", "-q", "-m", "init"]);
}

/// E2E-02. Changes are scoped to the files the session edited, so the edit is the session's
/// own `write` (the Pi original wrote the file from outside).
#[test]
fn workspace_scan_failure_keeps_the_last_diff_and_retry_recovers_without_a_prompt() {
    let fx = Fixture::start(|_, n| match n {
        0 => Reply::Tools(
            None,
            vec![(
                "write".into(),
                json!({ "path": "notes.txt", "content": "one\ntwo\n" }),
            )],
        ),
        _ => Reply::Text("Written.".into()),
    });
    init_project(&fx.project);
    let mut h = Harness::start(&fx, &fx.root.join("app.sqlite3"));
    h.open_new_conversation(&fx.project);
    assert!(h.state.current().unwrap().changes.is_empty());
    h.send("write the notes");
    h.until("a successful project scan", |s| {
        is_idle(s)
            && s.current()
                .is_some_and(|c| c.changes_state == ChangesState::Ready && c.changes.len() == 1)
    });
    let asked = fx.requests().len();
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
    assert_eq!(
        fx.requests().len(),
        asked,
        "scanning must not send a prompt"
    );
    h.stop();
}

/// A held first run with one follow-up answer after it.
fn held_queue_fixture(gate: &Gate) -> Fixture {
    Fixture::start(held_first(
        gate,
        Reply::Text("first done".into()),
        vec![Reply::Text("second done".into())],
    ))
}

fn queue_second_while_first_runs(h: &mut Harness, fx: &Fixture) {
    h.open_new_conversation(&fx.project);
    h.send("first");
    h.until_asked(fx, 1);
    h.until("running", is_running);
}

/// E2E-21: the engine admitted a follow-up but the acknowledgment never arrived (and the link
/// dropped). The app reconnects, asks the engine about the key, and shows it once.
#[test]
fn a_follow_up_whose_acknowledgment_is_lost_is_admitted_exactly_once() {
    let gate = Gate::new();
    let fx = held_queue_fixture(&gate);
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(&fx, &db);
    queue_second_while_first_runs(&mut h, &fx);
    fx.faults.drop_queue_reply.store(true, Ordering::SeqCst);
    h.dispatch(Command::EditDraft("second".into()));
    h.dispatch(Command::QueueFollowUp);
    h.until("the acknowledgment was lost", |_| {
        fx.faults.fired.load(Ordering::SeqCst) == 1
    });
    h.until("the app resolves it by asking the engine", |s| {
        s.connection.is_ready()
            && queued(s) == [(QueueMode::FollowUp, "second".to_owned())]
            && s.current().unwrap().pending_queue.is_empty()
    });
    assert!(is_running(&h.state));
    gate.open();
    h.until("both ran, once", |s| {
        is_idle(s) && assistant_texts(s) == ["first done".to_owned(), "second done".to_owned()]
    });
    assert_eq!(h.user_messages(), ["first", "second"]);
    assert_eq!(fx.requests().len(), 2);
    assert_eq!(user_texts(&fx.requests()[1]), ["first", "second"]);
    h.stop();
    assert_eq!(open_requests(&db), 0);
}

/// E2E-22: the follow-up never reached the engine (the link died with it). The same key is
/// sent again after reconnecting, and it is admitted once.
#[test]
fn a_follow_up_that_never_reached_the_engine_is_sent_again_under_its_key_once() {
    let gate = Gate::new();
    let fx = held_queue_fixture(&gate);
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(&fx, &db);
    queue_second_while_first_runs(&mut h, &fx);
    fx.faults.drop_queue.store(true, Ordering::SeqCst);
    h.dispatch(Command::EditDraft("second".into()));
    h.dispatch(Command::QueueFollowUp);
    h.until("the request was cut", |_| {
        fx.faults.fired.load(Ordering::SeqCst) == 1
    });
    h.until("it was sent again and admitted", |s| {
        s.connection.is_ready()
            && queued(s) == [(QueueMode::FollowUp, "second".to_owned())]
            && s.current().unwrap().pending_queue.is_empty()
    });
    gate.open();
    h.until("both ran, once", |s| {
        is_idle(s) && assistant_texts(s) == ["first done".to_owned(), "second done".to_owned()]
    });
    assert_eq!(h.user_messages(), ["first", "second"]);
    assert_eq!(fx.requests().len(), 2);
    h.stop();
    assert_eq!(open_requests(&db), 0);
}

/// E2E-23: the acknowledgment is lost and the app is closed before it can ask. After relaunch
/// the engine's side of it is found, nothing is resent, and the follow-up runs exactly once.
#[test]
fn a_lost_follow_up_acknowledgment_then_a_kill_and_relaunch_runs_it_once() {
    let gate = Gate::new();
    let fx = held_queue_fixture(&gate);
    let db = fx.root.join("app.sqlite3");
    let mut config = fx.native(Duration::from_secs(10));
    config.retry_delay = Duration::from_secs(30); // closed before it reconnects
    let mut h = Harness::start_with(config, &db);
    queue_second_while_first_runs(&mut h, &fx);
    let conversation = h.state.current().unwrap().id;
    fx.faults.drop_queue_reply.store(true, Ordering::SeqCst);
    h.dispatch(Command::EditDraft("second".into()));
    h.dispatch(Command::QueueFollowUp);
    h.until("the acknowledgment was lost", |s| {
        fx.faults.fired.load(Ordering::SeqCst) == 1
            && s.current()
                .unwrap()
                .pending_queue
                .iter()
                .any(|p| p.state == pipkin_core::QueueSend::Unknown)
    });
    h.stop();

    let mut h2 = Harness::start(&fx, &db);
    h2.until("the engine's side of it is found", |s| {
        s.selected == Some(conversation)
            && is_running(s)
            && queued(s) == [(QueueMode::FollowUp, "second".to_owned())]
    });
    assert_eq!(fx.requests().len(), 1, "nothing was resent");
    gate.open();
    h2.until("both ran, once", |s| {
        is_idle(s) && assistant_texts(s) == ["first done".to_owned(), "second done".to_owned()]
    });
    assert_eq!(h2.user_messages(), ["first", "second"]);
    assert_eq!(fx.requests().len(), 2);
    h2.stop();
    assert_eq!(open_requests(&db), 0);
}

/// E2E-27: the engine starts with no model; a provider added to its `models.json` afterwards
/// appears after a refresh and can then be used.
#[test]
fn a_provider_configured_after_start_appears_after_a_refresh_and_can_then_be_used() {
    let fx = Fixture::start_with(
        |_, _| Reply::Text("hello from the new provider".into()),
        true,
    );
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(&fx, &db);
    h.open_new_conversation(&fx.project);
    assert!(h.state.models.is_empty(), "{:?}", h.state.models);
    assert!(
        h.state.availability().refresh_models,
        "a refresh is offered when there is no model"
    );
    // The user adds a provider to the engine's own configuration, then refreshes. (Its API is
    // the scripted adapter, so no network is involved.)
    let config = fx.root.join("engine-config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("models.json"),
        json!({"providers": {"stub": {
            "baseUrl": "http://127.0.0.1:9/v1",
            "api": "scripted",
            "apiKey": "stub",
            "models": [{"id": "scripted", "name": "Scripted stub", "input": ["text"]}]
        }}})
        .to_string(),
    )
    .unwrap();
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
    assert_eq!(fx.requests().len(), 1);
    assert_eq!(fx.requests()[0].model.provider, "stub");
    h.stop();
}

/// DUR-DISKFULL at the desktop: the engine cannot record a run's settlement. The window is
/// told (a notice), the run is not shown as finished, and reopening reconciles it from what
/// the engine committed, without asking the model again.
#[test]
fn an_engine_storage_failure_is_shown_and_reopening_settles_the_run() {
    let fx = Fixture::start(|_, _| Reply::Text("saved answer".into()));
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(&fx, &db);
    h.open_new_conversation(&fx.project);
    let conversation = h.state.current().unwrap().id;
    fx.engine()
        .shared()
        .store
        .inject(pipkin_engine::store::Fault::Settlement)
        .unwrap();
    h.send("finish this");
    h.until("the storage failure is shown", |s| {
        s.notice
            .as_deref()
            .is_some_and(|n| n.contains("storage error"))
    });
    thread::sleep(Duration::from_millis(300));
    h.pump();
    assert!(is_running(&h.state), "no invented outcome");
    fx.engine().shared().store.clear_faults().unwrap();
    h.stop();

    let mut h2 = Harness::start(&fx, &db);
    h2.until("reopening settles it", |s| {
        s.selected == Some(conversation)
            && is_idle(s)
            && assistant_texts(s) == ["saved answer".to_owned()]
    });
    assert_eq!(h2.user_messages(), ["finish this"]);
    assert_eq!(fx.requests().len(), 1, "the model was not asked again");
    h2.stop();
    assert_eq!(open_requests(&db), 0);
}

fn git(dir: &Path, args: &[&str]) {
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
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .status()
        .expect("git runs")
        .success();
    assert!(ok, "git {args:?}");
}

// ----------------------------------------------------------------------------- Pi import

fn pi_fixture(id: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/pi-sessions")
        .join(id)
}

/// Copy a reference session into the fixture's Pi agent directory.
fn add_pi_session(fx: &Fixture, id: &str) -> bool {
    let source = pi_fixture(id);
    if !source.join("session.sqlite").is_file() {
        return false;
    }
    let dest = fx.pi_agent().join("experimental/sessions").join(id);
    std::fs::create_dir_all(&dest).unwrap();
    for file in ["meta.json", "session.sqlite"] {
        std::fs::copy(source.join(file), dest.join(file)).unwrap();
    }
    true
}

fn offer(s: &AppState) -> Option<&pipkin_core::ImportOffer> {
    match &s.import {
        ImportFlow::Offered(offer) => Some(offer),
        _ => None,
    }
}

#[test]
fn pi_conversations_are_imported_on_confirmation_with_their_desktop_data() {
    let fx = Fixture::start(|_, _| Reply::Text("Continued natively.".into()));
    if !add_pi_session(&fx, "a-text-multiturn") || !add_pi_session(&fx, "b-tools-diff") {
        eprintln!("no reference fixtures; skipping");
        return;
    }
    // One in use by a running Pi, one never opened.
    assert!(add_pi_session(&fx, "c-stop-then-prompt"));
    let sessions = fx.pi_agent().join("experimental/sessions");
    std::fs::create_dir(sessions.join("c-stop-then-prompt.lock")).unwrap();
    std::fs::create_dir_all(sessions.join("never-opened")).unwrap();
    std::fs::write(
        sessions.join("never-opened/meta.json"),
        r#"{"createdAt": 1, "cwd": "/x"}"#,
    )
    .unwrap();

    // What the Pi desktop kept for the first conversation: a draft and two sent prompts.
    let db = fx.root.join("app.sqlite3");
    let a = ConversationId(pipkin_core::stable_id("a-text-multiturn"));
    {
        let pi = Storage::open(&db, PI_NAMESPACE).unwrap();
        let (tx, rx) = mpsc::channel();
        pi.save_draft(a, "an unsent pi draft".into(), vec![], 4, move |r| {
            tx.send(r).unwrap();
        });
        rx.recv_timeout(WAIT).unwrap().unwrap();
        for (key, text) in [
            ("p1", "Hello, who are you?"),
            ("p2", "And a third, please."),
        ] {
            let (tx, rx) = mpsc::channel();
            pi.journal_intent(a, RequestId(key.into()), text, &[], None, move |r| {
                tx.send(r).unwrap();
            });
            rx.recv_timeout(WAIT).unwrap().unwrap();
            pi.journal_state(RequestId(key.into()), pipkin_core::JournalState::Completed);
        }
        pi.shutdown();
    }

    let mut h = Harness::start(&fx, &db);
    h.until("connected", |s| s.connection.is_ready());
    h.state.import_available = true;
    assert!(
        h.state.conversations.is_empty(),
        "nothing is imported at startup"
    );

    h.dispatch(Command::ImportFromPi(ImportKind::Conversations));
    h.until("the offer", |s| offer(s).is_some());
    let o = offer(&h.state).unwrap().clone();
    assert_eq!(o.sessions, vec!["a-text-multiturn", "b-tools-diff"]);
    let notice = h.state.notice.clone().unwrap();
    for part in [
        "2 Pi conversations from",
        "In use by a running Pi",
        "c-stop-then-p",
        "Cannot be imported: never-opened",
        "keep working with --engine pi",
        "Confirm: import",
    ] {
        assert!(notice.contains(part), "{part}: {notice}");
    }
    assert!(h.state.conversations.is_empty(), "an offer imports nothing");

    h.dispatch(Command::ConfirmImport);
    h.until("the import finished", |s| s.import == ImportFlow::Idle);
    let notice = h.state.notice.clone().unwrap();
    assert!(
        notice.contains("Imported 2 conversations from Pi."),
        "{notice}"
    );
    assert!(notice.contains("not imported: c-stop-then-p"), "{notice}");
    assert!(notice.contains("drafts: 1"), "{notice}");
    h.until("both conversations are listed", |s| {
        s.conversations.len() == 2 && s.conversation(a).is_some()
    });
    assert_eq!(
        h.state.conversation(a).unwrap().draft.text,
        "an unsent pi draft"
    );
    let stored = Storage::open(&db, NAMESPACE).unwrap().load_all().unwrap();
    assert!(stored.drafts.iter().any(|d| d.conversation == a));
    assert_eq!(
        stored.prompt_histories,
        vec![(
            a,
            vec!["Hello, who are you?".into(), "And a third, please.".into()]
        )]
    );
    // The Pi namespace keeps its own copy.
    let pi = Storage::open(&db, PI_NAMESPACE)
        .unwrap()
        .load_all()
        .unwrap();
    assert_eq!(pi.drafts.len(), 1);

    // The imported conversation opens with its whole history and continues natively.
    h.dispatch(Command::SelectConversation(a));
    h.until("the imported conversation is open", |s| {
        s.current().is_some_and(|c| c.opened)
    });
    assert_eq!(
        h.user_messages(),
        [
            "Hello, who are you?",
            "Tell me a second thing.",
            "And a third, please."
        ]
    );
    h.dispatch(Command::SetModel("scripted/scripted".into()));
    h.send("Please continue.");
    h.until("answered natively", |s| {
        is_idle(s) && assistant_texts(s).last().map(String::as_str) == Some("Continued natively.")
    });
    let request = fx.requests().last().cloned().unwrap();
    assert_eq!(
        user_texts(&request),
        [
            "Hello, who are you?",
            "Tell me a second thing.",
            "And a third, please.",
            "Please continue."
        ]
    );

    // Again: nothing left to import.
    h.dispatch(Command::ImportFromPi(ImportKind::Conversations));
    h.until("the answer", |s| s.import == ImportFlow::Idle);
    let notice = h.state.notice.clone().unwrap();
    assert!(
        notice.contains("2 conversations were imported earlier"),
        "{notice}"
    );
    assert!(offer(&h.state).is_none());
    h.stop();
}

#[test]
fn pi_credentials_are_imported_only_after_their_own_confirmation() {
    let fx = Fixture::start(|_, _| Reply::Text("x".into()));
    std::fs::create_dir_all(fx.pi_agent()).unwrap();
    std::fs::write(
        fx.pi_agent().join("auth.json"),
        r#"{"openrouter": {"type": "api_key", "key": "sk-or-SECRET"},
            "anthropic": {"type": "oauth", "access": "sub-SECRET", "refresh": "r", "expires": 1}}"#,
    )
    .unwrap();
    std::fs::write(fx.pi_agent().join("models.json"), r#"{"providers": {}}"#).unwrap();
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(&fx, &db);
    h.until("connected", |s| s.connection.is_ready());
    h.state.import_available = true;

    h.dispatch(Command::ImportFromPi(ImportKind::Credentials));
    h.until("the offer", |s| offer(s).is_some());
    let o = offer(&h.state).unwrap().clone();
    assert_eq!(o.providers, vec!["openrouter", "anthropic"]);
    assert_eq!(o.config, vec!["models.json"]);
    let notice = h.state.notice.clone().unwrap();
    assert!(!notice.contains("SECRET"));
    assert!(!notice.contains("later integration"), "{notice}");
    let native_auth = fx.root.join("engine-config/auth.json");
    assert!(!native_auth.exists(), "an offer writes nothing");

    h.dispatch(Command::CancelImport);
    assert_eq!(h.state.import, ImportFlow::Idle);
    assert!(!native_auth.exists());

    h.dispatch(Command::ImportFromPi(ImportKind::Credentials));
    h.until("the offer", |s| offer(s).is_some());
    h.dispatch(Command::ConfirmImport);
    h.until("the import finished", |s| s.import == ImportFlow::Idle);
    let notice = h.state.notice.clone().unwrap();
    assert!(
        notice.contains("Imported saved credentials for openrouter, anthropic."),
        "{notice}"
    );
    assert!(notice.contains("Copied models.json."), "{notice}");
    assert!(!notice.contains("SECRET"));
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&native_auth).unwrap()).unwrap();
    assert_eq!(saved["openrouter"]["key"], "sk-or-SECRET");
    assert_eq!(saved["anthropic"]["type"], "oauth");
    assert_eq!(saved["anthropic"]["access"], "sub-SECRET");
    assert!(fx.root.join("engine-config/models.json").is_file());
    h.stop();
}

/// Tool results in a model request (how far a scripted child has got).
fn tool_results(request: &ProviderRequest) -> usize {
    request
        .messages
        .iter()
        .filter(|m| matches!(m, Message::ToolResult { .. }))
        .count()
}

const LONG_ROUNDS: usize = 25;
const CALLS_PER_ROUND: usize = 10;

#[test]
fn a_parent_delegates_and_the_inspector_pages_through_its_subagents() {
    let gate = Gate::new();
    let held = gate.clone();
    let fx = Fixture::start(move |request, _| {
        let parent = request.tools.iter().any(|t| t.name == "subagent");
        let task = user_texts(request).first().cloned().unwrap_or_default();
        if parent {
            return if tool_results(request) == 0 {
                Reply::Tools(
                    Some("Delegating.".into()),
                    vec![
                        (
                            "subagent".into(),
                            json!({"task": "Short survey A. Report one line."}),
                        ),
                        (
                            "subagent".into(),
                            json!({"task": "Short survey B. Report one line."}),
                        ),
                        (
                            "subagent".into(),
                            json!({"task": "LONG probe. Call probe many times."}),
                        ),
                    ],
                )
            } else {
                Reply::Text("All subagents reported.".into())
            };
        }
        if !task.starts_with("LONG") {
            return Reply::Text(format!("short child done: {task}"));
        }
        let done = tool_results(request);
        if done < LONG_ROUNDS * CALLS_PER_ROUND {
            Reply::Tools(
                None,
                (0..CALLS_PER_ROUND)
                    .map(|i| ("probe".to_owned(), json!({"n": done + i})))
                    .collect(),
            )
        } else {
            Reply::Gated(
                held.clone(),
                Box::new(Reply::Text("long child done".into())),
            )
        }
    });
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(&fx, &db);
    h.open_new_conversation(&fx.project);
    h.send("Split this work across subagents.");
    // The long child is held at its final answer: still running, history already long.
    h.until("the long child reached its last turn", |_| {
        fx.requests().iter().any(|r| {
            user_texts(r).first().is_some_and(|t| t.starts_with("LONG"))
                && tool_results(r) == LONG_ROUNDS * CALLS_PER_ROUND
        })
    });
    h.until("three children, two finished", |s| {
        let sub = &s.current().unwrap().subagents;
        sub.paged
            && sub.children.len() == 3
            && sub.finished_count() == 2
            && sub.children.iter().any(|c| c.status == "running")
    });
    // History is a projection; the finished pair is revealed by its toggle (one page).
    h.dispatch(Command::ToggleSubagentHistory);
    let sub = &h.state.current().unwrap().subagents;
    assert_eq!(
        (sub.history_shown(), sub.can_show_more_history()),
        (2, false)
    );
    let long = sub
        .children
        .iter()
        .find(|c| c.task.starts_with("LONG"))
        .unwrap()
        .id;
    h.dispatch(Command::SelectSubagent(Some(long)));
    h.until("the newest page of the long child", |s| {
        let sub = &s.current().unwrap().subagents;
        sub.viewed == Some(long) && sub.has_older
    });
    let newest = h.state.current().unwrap().subagents.items.len();
    // Page back to the start, one bounded request at a time.
    let mut pages = 0;
    while h.state.current().unwrap().subagents.can_load_older() {
        let shown = h.state.current().unwrap().subagents.items.len();
        h.dispatch(Command::LoadOlderSubagent);
        h.until("an older child page", |s| {
            let sub = &s.current().unwrap().subagents;
            sub.loading_older.is_none() && (sub.items.len() > shown || !sub.has_older)
        });
        pages += 1;
        assert!(pages < 10, "paging did not converge");
    }
    let sub = &h.state.current().unwrap().subagents;
    assert!(pages >= 1 && sub.items.len() > newest);
    assert!(sub.older_error.is_none());
    let ids: Vec<u64> = sub.items.iter().map(|i| i.id.0).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted, ids, "ordered, without duplicates");
    assert_eq!(sub.previews.iter().map(|p| p.id.0).collect::<Vec<_>>(), ids);
    // The child's task prompt is the first entry, and every probe call is reachable once.
    assert!(matches!(&sub.items[0].kind, ItemKind::User { text, .. } if text.starts_with("LONG")));
    let probes = sub
        .items
        .iter()
        .filter(|i| matches!(&i.kind, ItemKind::Tool(t) if t.name == "probe"))
        .count();
    assert_eq!(probes, LONG_ROUNDS * CALLS_PER_ROUND);
    let first = ids[0];
    // Live: the child answers; only its newest page is re-read, the paged-in start stays.
    gate.open();
    h.until("the long child's answer arrives live", |s| {
        s.current()
            .unwrap()
            .subagents
            .previews
            .iter()
            .any(|p| &*p.text == "long child done")
    });
    let sub = &h.state.current().unwrap().subagents;
    assert_eq!(sub.items[0].id.0, first);
    assert_eq!(sub.selected, Some(long));
    h.until("the parent settles", |s| {
        is_idle(s)
            && assistant_texts(s)
                .last()
                .is_some_and(|t| t == "All subagents reported.")
    });
    h.until("every child finished", |s| {
        s.current().unwrap().subagents.finished_count() == 3
    });
    h.stop();
}

/// The first event the Pi adapter emits for `op`, within the wait.
fn pi_answer(events: &async_channel::Receiver<BackendEvent>, op: u64) -> pipkin_core::EventKind {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Ok(event) = events.try_recv()
            && event.op == Some(pipkin_core::OperationId(op))
        {
            return event.kind;
        }
        assert!(Instant::now() < deadline, "no answer for op {op}");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn an_imported_conversation_is_read_only_with_pi_until_it_is_released() {
    use crate::adapters::pi::{PiBackend, PiConfig};
    use pipkin_core::{ConversationOwner, EventKind, NATIVE_OWNED_NOTICE, ReleaseFlow};

    let fx = Fixture::start(|_, _| Reply::Text("Continued natively.".into()));
    if !add_pi_session(&fx, "a-text-multiturn") {
        eprintln!("no reference fixtures; skipping");
        return;
    }
    // In use by a running Pi: reported, not imported, so never owned.
    assert!(add_pi_session(&fx, "c-stop-then-prompt"));
    let sessions = fx.pi_agent().join("experimental/sessions");
    std::fs::create_dir(sessions.join("c-stop-then-prompt.lock")).unwrap();
    let a = ConversationId(pipkin_core::stable_id("a-text-multiturn"));
    let c = ConversationId(pipkin_core::stable_id("c-stop-then-prompt"));
    let db = fx.root.join("app.sqlite3");
    {
        let pi = Storage::open(&db, PI_NAMESPACE).unwrap();
        let (tx, rx) = mpsc::channel();
        pi.save_draft(a, "an unsent pi draft".into(), vec![], 4, move |r| {
            tx.send(r).unwrap();
        });
        rx.recv_timeout(WAIT).unwrap().unwrap();
        pi.shutdown();
    }

    // Import into the built-in engine (in-process): ownership lands with the desktop data.
    let mut h = Harness::start(&fx, &db);
    h.until("connected", |s| s.connection.is_ready());
    h.state.import_available = true;
    h.dispatch(Command::ImportFromPi(ImportKind::Conversations));
    h.until("the offer", |s| matches!(s.import, ImportFlow::Offered(_)));
    h.dispatch(Command::ConfirmImport);
    h.until("the import finished", |s| s.import == ImportFlow::Idle);
    let notice = h.state.notice.clone().unwrap();
    assert!(notice.contains("Imported 1 conversation"), "{notice}");
    let owned = h.storage.native_owned().unwrap();
    assert_eq!(owned.len(), 1, "{owned:?}");
    assert_eq!(owned[0].conversation, a);
    assert_eq!(
        (owned[0].engine.as_str(), owned[0].source_namespace.as_str()),
        (NAMESPACE, PI_NAMESPACE)
    );
    assert_eq!(
        h.storage.native_owner(c).unwrap(),
        None,
        "locked: not owned"
    );
    // The built-in engine ignores the record: the conversation stays writable natively.
    h.until("listed natively", |s| s.conversation(a).is_some());
    h.dispatch(Command::SelectConversation(a));
    h.until("open natively", |s| s.current().is_some_and(|c| c.opened));
    assert_eq!(h.state.read_only_reason(a), None);
    h.dispatch(Command::SetModel("scripted/scripted".into()));
    h.send("Please continue.");
    h.until("answered natively", |s| {
        is_idle(s) && assistant_texts(s).last().map(String::as_str) == Some("Continued natively.")
    });
    h.stop();
    // The Pi source was not written.
    assert!(sessions.join("a-text-multiturn/session.sqlite").is_file());

    // Later, with --engine pi: ownership is restored at startup from the desktop database.
    let pi_storage = Arc::new(Storage::open(&db, PI_NAMESPACE).unwrap());
    let project = pipkin_core::Project {
        id: pipkin_core::ProjectId(1),
        name: "p".into(),
        path: "/work/p".into(),
    };
    let models = vec![pipkin_core::ModelInfo {
        id: "anthropic/sonnet".into(),
        name: "Sonnet".into(),
        note: String::new(),
    }];
    let mut pi = AppState::new(
        Bootstrap {
            projects: vec![],
            models: vec![],
            conversations: vec![],
            now: 1,
        },
        pipkin_core::Prefs::default(),
    );
    pi.mode = Mode::Real;
    crate::controller::restore_ownership(&pi_storage, &mut pi);
    assert!(matches!(
        pi.owner(a),
        Some(ConversationOwner::NativeEngine { imported_at }) if *imported_at > 0
    ));
    // The Pi engine (a stub's catalog and answers) lists both sessions.
    pi.apply_catalog(Bootstrap {
        projects: vec![project.clone()],
        models: models.clone(),
        conversations: vec![
            (a, project.id, "a".into(), 2),
            (c, project.id, "c".into(), 1),
        ],
        now: 2,
    });
    let open = |pi: &mut AppState, id: ConversationId| {
        pi.dispatch(Command::SelectConversation(id));
        let generation = pi.current().unwrap().generation;
        pi.apply_event(BackendEvent {
            conversation: id,
            generation,
            op: None,
            kind: EventKind::Opened {
                items: vec![],
                has_older: false,
                changes: vec![],
            },
        });
    };
    open(&mut pi, a);
    pi.dispatch(Command::EditDraft("continue in Pi".into()));
    assert!(!pi.availability().submit);
    let out = pi.dispatch(Command::Submit);
    assert!(
        !out.effects.iter().any(|e| matches!(
            e,
            Effect::JournalIntent { .. }
                | Effect::Backend(pipkin_core::BackendRequest::Submit { .. })
        )),
        "{:?}",
        out.effects
    );
    assert_eq!(pi.notice.as_deref(), Some(NATIVE_OWNED_NOTICE));
    assert_eq!(pi.read_only_reason(a).as_deref(), Some(NATIVE_OWNED_NOTICE));
    // The locked session was never imported: Pi still writes it.
    open(&mut pi, c);
    pi.dispatch(Command::EditDraft("still Pi's".into()));
    assert!(pi.availability().submit);

    // The Pi adapter refuses at its boundary too (no Pi running: the ownership reason comes
    // first, a definite rejection, not "not connected" and never an unknown outcome).
    let empty = fx.root.join("no-pi-server");
    std::fs::create_dir_all(&empty).unwrap();
    std::fs::set_permissions(&empty, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    let (tx, events) = async_channel::unbounded();
    let mut config = PiConfig::new(empty);
    config.retry_delay = Duration::from_millis(50);
    config.owned_elsewhere = Some(crate::controller::ownership_check(&pi_storage));
    let backend = PiBackend::new(tx, config);
    backend.start(Box::new(|_| {}));
    let submit = |conversation, op| {
        backend.request(pipkin_core::BackendRequest::Submit {
            conversation,
            generation: 1,
            op: pipkin_core::OperationId(op),
            request: RequestId(format!("r{op}")),
            text: "x".into(),
            attachments: vec![],
            model: None,
        })
    };
    submit(a, 1);
    assert_eq!(
        pi_answer(&events, 1),
        EventKind::Rejected {
            reason: NATIVE_OWNED_NOTICE.into()
        }
    );

    // Released explicitly: only the desktop record goes, and Pi may write it again.
    open(&mut pi, a);
    pi.dispatch(Command::ReleaseToPi);
    assert!(matches!(pi.release, ReleaseFlow::Confirming(id) if id == a));
    let out = pi.dispatch(Command::ConfirmRelease);
    assert_eq!(
        out.effects,
        vec![Effect::ReleaseOwnership { conversation: a }]
    );
    let removed = pi_storage.release_ownership(a).map(|_| ());
    pi.ownership_released(a, removed);
    assert_eq!(pi_storage.native_owner(a).unwrap(), None);
    assert!(
        pi.availability().submit,
        "the draft can be sent in Pi again"
    );
    submit(a, 2);
    assert_eq!(
        pi_answer(&events, 2),
        EventKind::Rejected {
            reason: "Not connected to the Pi engine.".into()
        },
        "past the ownership check: only the missing server refuses it now"
    );
    backend.shutdown();
    // Neither engine's data was touched by releasing: the native copy is still there.
    let native = Storage::open(&db, NAMESPACE).unwrap();
    assert!(
        native
            .load_all()
            .unwrap()
            .prompt_histories
            .iter()
            .any(|(id, prompts)| *id == a && prompts.iter().any(|p| p == "Please continue.")),
        "the native-only history stays with the built-in engine"
    );
    assert!(sessions.join("a-text-multiturn/session.sqlite").is_file());
}

#[test]
fn a_plugin_selection_added_after_the_import_offer_never_transfers_ownership() {
    let fx = Fixture::start(|_, _| Reply::Text("unused".into()));
    assert!(add_pi_session(&fx, "a-text-multiturn"));
    let id = ConversationId(pipkin_core::stable_id("a-text-multiturn"));
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(&fx, &db);
    h.until("connected", |s| s.connection.is_ready());
    h.state.import_available = true;
    h.dispatch(Command::ImportFromPi(ImportKind::Conversations));
    h.until("offer", |s| offer(s).is_some());
    assert_eq!(offer(&h.state).unwrap().sessions, ["a-text-multiturn"]);
    let server = fx.root.join("pi-server");
    std::fs::create_dir_all(&server).unwrap();
    let profile = server.join("plugin-packages-custom.json");
    let content = r#"{"version":1,"packagePaths":["/plugins/questions"]}"#;
    std::fs::write(&profile, content).unwrap();
    h.dispatch(Command::ConfirmImport);
    h.until("blocked import finished", |s| s.import == ImportFlow::Idle);
    let notice = h.state.notice.as_deref().unwrap();
    assert!(
        notice.contains("plugin profile") && notice.contains("keep this conversation on Pi"),
        "{notice}"
    );
    assert!(h.state.conversations.is_empty());
    assert_eq!(h.storage.native_owner(id).unwrap(), None);
    assert!(
        fx.engine()
            .shared()
            .store
            .imported("pi", "a-text-multiturn")
            .unwrap()
            .is_none()
    );
    assert_eq!(std::fs::read_to_string(profile).unwrap(), content);
    h.stop();
}

#[test]
fn durable_catch_up_recovers_a_missing_terminal_event_without_later_live_traffic() {
    let fx = Fixture::start(|_, _| Reply::Text("saved without delivery".into()));
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start(&fx, &db);
    h.open_new_conversation(&fx.project);
    fx.faults.drop_events.store(true, Ordering::SeqCst);
    // Saved settlement must remain recoverable after its invalidation is acknowledged,
    // even when the first reconciliation attempts fail and there is no later event.
    fx.faults.refuse_lookups.store(3, Ordering::SeqCst);
    h.send("do this once");
    h.until(
        "durable catch-up recovers history and settles the operation",
        |s| is_idle(s) && assistant_texts(s) == ["saved without delivery".to_owned()],
    );
    assert!(fx.faults.fired.load(Ordering::SeqCst) > 0);
    assert_eq!(fx.requests().len(), 1);
    h.stop();
    assert_eq!(open_requests(&db), 0);
}

#[cfg(unix)]
#[test]
fn external_desktops_share_runs_and_reconnect_without_owning_the_server() {
    let fx = Fixture::start(|_, n| {
        if n == 0 {
            Reply::Hang
        } else {
            Reply::Text("continued".into())
        }
    });
    let endpoint = fx.root.join("attach");
    let server = pipkin_engine::endpoint::Server::start(fx.engine(), &endpoint).unwrap();
    let config = || {
        let mut c = NativeConfig::new(
            HostSpec::External(endpoint.clone()),
            endpoint.display().to_string(),
        );
        c.retry_delay = Duration::from_millis(50);
        c
    };
    let mut a = Harness::start_with(config(), &fx.root.join("a.sqlite3"));
    a.open_new_conversation(&fx.project);
    let conversation = a.state.selected.unwrap();
    a.send("started by A");
    a.until_asked(&fx, 1);
    a.until("A running", is_running);
    let mut b = Harness::start_with(config(), &fx.root.join("b.sqlite3"));
    b.until("shared conversation in B", |s| {
        s.conversation(conversation).is_some()
    });
    b.dispatch(Command::SelectConversation(conversation));
    b.until("B observes A's run", is_running);
    a.stop();
    assert!(b.state.availability().cancel);
    b.dispatch(Command::Cancel);
    b.until("B stops the shared run", is_idle);
    b.send("continue after A quit");
    b.until("B receives answer", |s| {
        is_idle(s) && assistant_texts(s).contains(&"continued".into())
    });
    drop(server);
    fx.restart_engine();
    let _server = pipkin_engine::endpoint::Server::start(fx.engine(), &endpoint).unwrap();
    b.until("B reconnects to the owner's restarted server", |s| {
        s.connection == Connection::Ready && s.current().is_some_and(|c| c.opened)
    });
    b.send("after restart");
    b.until_asked(&fx, 3);
    b.until("post-restart turn settles", is_idle);
    assert_eq!(
        b.user_messages(),
        ["started by A", "continue after A quit", "after restart"]
    );
    b.stop();
    let probe = crate::install::diagnose_native_attachment(&endpoint, true);
    assert!(!probe.1, "{}", probe.0);
    assert!(probe.0.contains("Ownership: external"));
}

fn wait_without_ui(what: &str, ready: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "timed out without consuming UI events: {what}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn pressure_fixture() -> (Fixture, Harness, String, crate::event_queue::Lease) {
    let text = "large history 🦀\n".repeat(8192);
    let reply = text.clone();
    let fx = Fixture::start(move |_, n| {
        if n == 0 {
            Reply::Thinking("thought".into(), reply.clone())
        } else {
            Reply::Hang
        }
    });
    let db = fx.root.join("app.sqlite3");
    let mut h = Harness::start_with_limits(fx.native(Duration::from_secs(2)), &db, 128, 1024);
    h.open_new_conversation(&fx.project);
    h.send("make history");
    h.until("large answer committed", |s| {
        is_idle(s) && assistant_texts(s) == [text.clone()]
    });
    h.send("wait for Stop");
    h.until_asked(&fx, 2);
    h.until("running", is_running);
    let c = h.state.current().unwrap();
    h.backend.request(pipkin_core::BackendRequest::Open {
        conversation: c.id,
        generation: c.generation,
    });
    // Drain setup traffic only until an actual large history event reaches a UI batch.
    // Retain its lease without applying it, exactly as a paused controller batch would.
    // Looking only at queue occupancy races small events still arriving after Running.
    let deadline = Instant::now() + Duration::from_secs(10);
    let held = loop {
        if let Ok(mut lease) = h.events.try_recv() {
            if lease.bytes() > 1024 {
                break lease;
            }
            let out = h.state.apply_event(lease.take());
            h.exec(out);
        }
        assert!(
            Instant::now() < deadline,
            "large history never reached the UI batch"
        );
        thread::sleep(Duration::from_millis(1));
    };
    wait_without_ui("byte pressure", || {
        let (count, bytes, waiting) = h.backend.events.stats();
        count == 1 && bytes > 1024 && waiting == 1
    });
    (fx, h, text, held)
}

#[test]
fn stop_crosses_desktop_byte_pressure_and_settlement_recovers_after_reconnect() {
    for restart in [false, true] {
        let (fx, mut h, text, mut held) = pressure_fixture();
        let db = fx.root.join("app.sqlite3");
        let engine = fx.engine();
        let session = engine.shared().store.root_sessions().unwrap()[0].id.clone();
        let key = h.storage.load_all().unwrap().open_requests[0]
            .request
            .0
            .clone();
        h.dispatch(Command::Cancel);
        wait_without_ui("durable aborted result before UI delivery", || {
            matches!(
                engine
                    .shared()
                    .store
                    .request(&session, &key)
                    .unwrap()
                    .unwrap()
                    .lookup(),
                pipkin_engine_protocol::LookupResult::Known {
                    open: false,
                    outcome: Some(pipkin_engine_protocol::RunOutcome::Aborted),
                    ..
                }
            )
        });
        assert!(
            matches!(h.state.current().unwrap().run, RunState::Stopping { .. }),
            "UI has not consumed settlement"
        );
        assert_eq!(h.backend.events.stats().0, 1);
        if restart {
            drop(engine);
            fx.restart_engine();
        }
        let out = h.state.apply_event(held.take());
        h.exec(out);
        drop(held);
        h.until("settled after draining", is_idle);
        assert_eq!(assistant_texts(&h.state), [text]);
        assert_eq!(h.user_messages(), ["make history", "wait for Stop"]);
        assert_eq!(fx.requests().len(), 2, "no prompt replay during recovery");
        h.stop();
        assert_eq!(open_requests(&db), 0, "terminal journal state persisted");
    }
}

#[test]
fn shutdown_releases_a_native_worker_waiting_for_desktop_bytes() {
    let (_fx, h, _, _held) = pressure_fixture();
    let started = Instant::now();
    h.backend.shutdown();
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "shutdown waited for UI consumption"
    );
    wait_without_ui("producer released", || h.backend.events.stats().2 == 0);
    h.storage.shutdown();
}
