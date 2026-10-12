//! The built-in engine adapter: maps `pipkin-engine`'s typed contract onto Pipkin's core.
//!
//! One worker thread owns the connection. It starts the engine (an owned worker process, or
//! in-process), says hello, mirrors the session catalogue, and opens a conversation by reading
//! its snapshot and then applying the committed changes the engine publishes for it. Every
//! event carries `(epoch, seq)`; a gap or a new epoch discards what was derived from events and
//! re-reads: the catalogue, the open session's snapshot, and the outcome of every run and queue
//! request in doubt (by `Lookup`). A prompt is never sent again: a lost acknowledgment is an
//! unknown outcome the person resolves by asking the engine.
//!
//! Runs settle from `RunSettled` events, which the engine publishes for every session, so a
//! conversation the person moved away from still settles. Streaming deltas update a per-session
//! mirror; the transcript is remapped and sent at most every `SYNC_INTERVAL`.

pub mod client;
mod control;
mod import;
mod mapping;
mod mirror;
pub mod supervisor;
pub mod transcript;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pipkin_core::onboarding::{SignInProvider, SignInSnapshot, SignInStatus};
use pipkin_core::{
    Attachment, Backend, BackendEvent, BackendRequest, CancelOutcome, ChangesState, Connection,
    ConversationId, EventKind, ItemId, LifecycleEvent, LifecycleSink, ModelInfo, OperationId,
    QueueId, QueueMode, RequestId, UiAnswer,
};
use pipkin_engine_protocol::{
    Call, CancelQueuedOutcome, Entry, ErrorCode, Event, ImageInput, InputMode, LookupResult,
    PROTOCOL_VERSION, ProtoError, Reply, Secret, SessionChange, SessionSummary,
};

use self::client::{CallError, Client, Inbound, SeqCheck, Sequencer};
pub use self::import::{ImportDone, ImportResult, unavailable as import_unavailable};
use self::mirror::{Mirror, Touched};
use self::supervisor::{HostSpec, Supervisor};
use crate::adapters::attach;
use crate::adapters::workspace::{self, Workspace};

const CALL_TIMEOUT: Duration = Duration::from_secs(15);
/// The transcript is remapped at most this often while a reply streams (about 20 per second).
const SYNC_INTERVAL: Duration = Duration::from_millis(50);
const CHANGES_POLL: Duration = Duration::from_secs(2);
const BATCH: usize = 256;
/// Entries per page of older history, and the pages read for file-change evidence on open.
const HISTORY_PAGE: u32 = 50;
const EVIDENCE_PAGES: usize = 8;
const SUBAGENT_PAGE: u32 = 200;
/// A selected child's newest page is re-read at most this often while it is changing.
const CHILD_SYNC_INTERVAL: Duration = Duration::from_millis(250);
const SHUTDOWN_CALL: Duration = Duration::from_secs(2);
const OFFLINE: &str = "Not connected to the built-in engine.";

#[derive(Clone)]
pub struct NativeConfig {
    pub host: HostSpec,
    /// Explicit Pi profile source for discovery and confirmed imports only.
    pub import_agent_dir: Option<String>,
    /// Shown as the location of sessions without a usable working directory.
    pub location: String,
    /// Delay between connection attempts while the engine cannot be reached.
    pub retry_delay: Duration,
    pub call_timeout: Duration,
}

impl NativeConfig {
    pub fn with_import_source(mut self, source: Option<&Path>) -> Self {
        self.import_agent_dir = source.map(|path| path.display().to_string());
        self
    }

    pub fn new(host: HostSpec, location: String) -> NativeConfig {
        NativeConfig {
            host,
            import_agent_dir: None,
            location,
            retry_delay: Duration::from_secs(3),
            call_timeout: CALL_TIMEOUT,
        }
    }
}

enum Msg {
    EngineReady {
        conn: u64,
        pending: std::sync::Arc<Mutex<pipkin_engine_protocol::delivery::EventQueue>>,
    },
    Request(BackendRequest),
    Engine {
        conn: u64,
        inbound: Inbound,
    },
    Changes {
        conversation: ConversationId,
        generation: u64,
        scanned_through: u64,
        workspace: Workspace,
    },
    /// An explicit Pi import step, answered through `done`.
    Import {
        step: pipkin_core::ImportStep,
        done: ImportDone,
    },
    StopFailed {
        conversation: ConversationId,
        generation: u64,
        op: OperationId,
        message: String,
    },
    Shutdown,
}

pub struct NativeBackend {
    config: NativeConfig,
    control: Arc<control::Control>,
    events: crate::event_queue::EventSender,
    tx: Sender<Msg>,
    rx: Mutex<Option<Receiver<Msg>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl NativeBackend {
    pub fn new(events: impl Into<crate::event_queue::EventSender>, config: NativeConfig) -> Self {
        let events = events.into();
        let (tx, rx) = mpsc::channel();
        let control = Arc::new(control::Control::new(tx.clone(), config.call_timeout));
        NativeBackend {
            config,
            control,
            events,
            tx,
            rx: Mutex::new(Some(rx)),
            worker: Mutex::new(None),
        }
    }
}

impl NativeBackend {
    /// Run one step of an explicit Pi import; `done` is called once with the answer (from the
    /// backend's worker thread), also when the engine cannot be reached.
    pub fn import(&self, step: pipkin_core::ImportStep, done: ImportDone) {
        if let Err(mpsc::SendError(Msg::Import { step, done })) =
            self.tx.send(Msg::Import { step, done })
        {
            done(import::unavailable(&step, OFFLINE));
        }
    }
}

impl Backend for NativeBackend {
    fn start(&self, sink: LifecycleSink) {
        let Some(rx) = lock(&self.rx).take() else {
            return;
        };
        let worker = Worker {
            supervisor: Supervisor::new(self.config.host.clone()),
            control: self.control.clone(),
            config: self.config.clone(),
            events: self.events.clone(),
            sink,
            tx: self.tx.clone(),
            rx,
            conn: 0,
            ever_connected: false,
            last_open: None,
            runs: HashMap::new(),
            queue_unknown: Vec::new(),
            last_refresh_warning: None,
            changes_bases: HashMap::new(),
        };
        let handle = thread::Builder::new()
            .name("native-backend".into())
            .spawn(move || worker.run())
            .expect("spawn native backend");
        *lock(&self.worker) = Some(handle);
    }

    fn request(&self, request: BackendRequest) {
        if !self.control.request(&request) {
            self.control.submitting(&request);
            let _ = self.tx.send(Msg::Request(request));
        }
    }

    fn shutdown(&self) {
        // Release a producer waiting for UI admission before asking it to shut down.
        self.events.close();
        self.control.close();
        let _ = self.tx.send(Msg::Shutdown);
        if let Some(handle) = lock(&self.worker).take() {
            // The worker asks the engine to stop, then waits out the supervisor's grace period
            // before killing it; never block exit much beyond that.
            let deadline = Instant::now() + supervisor::STOP_GRACE + SHUTDOWN_CALL * 2;
            while !handle.is_finished() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
        self.control.reap();
    }
}

/// A steer or follow-up in flight to the engine.
#[derive(Clone, Debug)]
struct QueueReq {
    conversation: ConversationId,
    generation: u64,
    request: RequestId,
    mode: QueueMode,
    text: String,
    attachments: Vec<Attachment>,
}

/// A prompt the engine admitted whose outcome has not been seen yet.
#[derive(Clone, Debug)]
struct Run {
    conversation: ConversationId,
    generation: u64,
    request: RequestId,
}

struct Worker {
    config: NativeConfig,
    control: Arc<control::Control>,
    supervisor: Supervisor,
    events: crate::event_queue::EventSender,
    sink: LifecycleSink,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    /// Identifies the current connection; frames from an earlier one are ignored.
    conn: u64,
    ever_connected: bool,
    /// Latest selection and generation; true until its initial Opened was delivered.
    last_open: Option<(ConversationId, u64, bool)>,
    /// Admitted prompts awaiting an outcome. Kept across reconnects and reconciled by asking.
    runs: HashMap<OperationId, Run>,
    /// Queue requests whose acknowledgment was lost.
    queue_unknown: Vec<QueueReq>,
    last_refresh_warning: Option<String>,
    /// The newest entry a clean (empty) change scan covered, per conversation.
    changes_bases: HashMap<ConversationId, u64>,
}

enum Outcome {
    Shutdown,
    Retry,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

impl Worker {
    fn run(mut self) {
        loop {
            match self.connect_and_serve() {
                Outcome::Shutdown => break,
                Outcome::Retry => {
                    if self.wait_to_retry() {
                        break;
                    }
                }
            }
        }
        self.supervisor.stop();
    }

    fn report(&self, connection: Connection) {
        (self.sink)(LifecycleEvent::Connection(connection));
    }

    fn notice(&self, message: String) {
        (self.sink)(LifecycleEvent::Notice(message));
    }

    fn emit(&self, conversation: ConversationId, generation: u64, kind: EventKind) {
        let _ = self.events.send_blocking(BackendEvent {
            conversation,
            generation,
            op: None,
            kind,
        });
    }

    fn emit_op(
        &self,
        conversation: ConversationId,
        generation: u64,
        op: OperationId,
        kind: EventKind,
    ) {
        let _ = self.events.send_blocking(BackendEvent {
            conversation,
            generation,
            op: Some(op),
            kind,
        });
    }

    fn queue_refused(&self, req: &QueueReq, reason: String) {
        self.emit(
            req.conversation,
            req.generation,
            EventKind::QueueRefused {
                request: req.request.clone(),
                reason,
            },
        );
    }

    fn queue_lost(&mut self, req: &QueueReq) {
        self.queue_unknown.push(req.clone());
        self.emit(
            req.conversation,
            req.generation,
            EventKind::QueueAckLost {
                request: req.request.clone(),
            },
        );
    }

    fn wait_to_retry(&mut self) -> bool {
        let deadline = Instant::now() + self.config.retry_delay;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(remaining) {
                Err(mpsc::RecvTimeoutError::Timeout) => return false,
                Err(mpsc::RecvTimeoutError::Disconnected) | Ok(Msg::Shutdown) => return true,
                Ok(Msg::Request(request)) => self.refuse_offline(request),
                Ok(Msg::StopFailed {
                    conversation,
                    generation,
                    op,
                    message,
                }) => self.emit_op(
                    conversation,
                    generation,
                    op,
                    EventKind::StopFailed { message },
                ),
                Ok(Msg::Import { step, done }) => done(import::unavailable(&step, OFFLINE)),
                Ok(Msg::Engine { .. }) | Ok(Msg::EngineReady { .. }) | Ok(Msg::Changes { .. }) => {}
            }
        }
    }

    /// Nothing was sent, so every refusal here is definite.
    fn refuse_offline(&mut self, request: BackendRequest) {
        if let BackendRequest::Submit {
            conversation,
            generation,
            op,
            ..
        } = &request
        {
            self.control.submitted(*conversation, *generation, *op);
        }
        match request {
            BackendRequest::Open {
                conversation,
                generation,
            } => {
                // Keep the latest read-only selection so an offline saved copy can attach
                // when the engine returns. Mutations below are still definitely refused.
                self.last_open = Some((conversation, generation, true));
                self.emit(
                    conversation,
                    generation,
                    EventKind::OpenFailed {
                        message: OFFLINE.into(),
                    },
                );
            }
            BackendRequest::Submit {
                conversation,
                generation,
                op,
                ..
            } => self.emit_op(
                conversation,
                generation,
                op,
                EventKind::Rejected {
                    reason: OFFLINE.into(),
                },
            ),
            BackendRequest::LoadOlder {
                conversation,
                generation,
                ..
            } => self.emit(
                conversation,
                generation,
                EventKind::OlderFailed {
                    message: OFFLINE.into(),
                },
            ),
            BackendRequest::Queue {
                conversation,
                generation,
                request,
                ..
            } => self.emit(
                conversation,
                generation,
                EventKind::QueueRefused {
                    request,
                    reason: OFFLINE.into(),
                },
            ),
            BackendRequest::OpenSubagent {
                conversation,
                generation,
                child,
            } => self.emit(
                conversation,
                generation,
                EventKind::SubagentViewFailed {
                    child,
                    reason: OFFLINE.into(),
                },
            ),
            BackendRequest::LoadOlderSubagent {
                conversation,
                generation,
                child,
                before,
            } => self.emit(
                conversation,
                generation,
                EventKind::SubagentOlderFailed {
                    child,
                    before,
                    reason: OFFLINE.into(),
                },
            ),
            BackendRequest::Cancel {
                conversation,
                generation,
                op,
            } => self.emit_op(
                conversation,
                generation,
                op,
                EventKind::StopFailed {
                    message: format!("Could not ask the engine to stop: {OFFLINE}"),
                },
            ),
            BackendRequest::CheckStatus {
                conversation,
                generation,
                op,
                ..
            } => self.emit_op(
                conversation,
                generation,
                op,
                EventKind::StatusCheckFailed {
                    message: format!("Could not check the run: {OFFLINE}"),
                },
            ),
            BackendRequest::RefreshChanges {
                conversation,
                generation,
            } => self.emit(
                conversation,
                generation,
                EventKind::ChangesScanState(ChangesState::Unavailable(OFFLINE.into())),
            ),
            BackendRequest::FetchToolOutput {
                conversation,
                generation,
                call_id,
            } => self.emit(
                conversation,
                generation,
                EventKind::ToolOutputUnavailable {
                    call_id,
                    reason: OFFLINE.into(),
                },
            ),
            BackendRequest::UiRespond {
                conversation,
                generation,
                id,
                ..
            } => self.emit(
                conversation,
                generation,
                EventKind::UiRespondRefused {
                    id,
                    reason: OFFLINE.into(),
                },
            ),
            BackendRequest::Steer { .. } => {}
            _ => self.notice(OFFLINE.into()),
        }
    }

    fn connect_and_serve(&mut self) -> Outcome {
        (self.sink)(LifecycleEvent::SignIn(SignInSnapshot::default()));
        self.report(if self.ever_connected {
            Connection::Reconnecting
        } else {
            Connection::Connecting
        });
        let link = match self.supervisor.start() {
            Ok(link) => link,
            Err(error) => {
                self.report(Connection::Failed(error));
                return Outcome::Retry;
            }
        };
        self.conn += 1;
        let conn = self.conn;
        let forward = Mutex::new(self.tx.clone());
        let pending = std::sync::Arc::new(Mutex::new(
            pipkin_engine_protocol::delivery::EventQueue::default(),
        ));
        let client = Arc::new(Client::start(link, move |inbound| {
            if let Inbound::Event { epoch, seq, event } = inbound {
                let mut queue = lock(&pending);
                let wake = queue.is_empty();
                queue.push(epoch, seq, *event);
                if wake {
                    let _ = lock(&forward).send(Msg::EngineReady {
                        conn,
                        pending: pending.clone(),
                    });
                }
            } else {
                let _ = lock(&forward).send(Msg::Engine { conn, inbound });
            }
        }));
        let welcome = match client.call(
            Call::Hello {
                protocol: PROTOCOL_VERSION,
                client: format!("pipkin-desktop {}", env!("CARGO_PKG_VERSION")),
            },
            self.config.call_timeout,
        ) {
            Ok(Reply::Welcome(w)) => w,
            Err(CallError::Refused(e)) if e.code == ErrorCode::Incompatible => {
                self.report(Connection::Incompatible(e.message));
                return Outcome::Retry;
            }
            Ok(other) => {
                self.report(Connection::Incompatible(format!(
                    "the engine answered hello with {other:?}"
                )));
                return Outcome::Retry;
            }
            Err(error) => {
                self.report(Connection::Offline(format!(
                    "the engine did not answer: {error}"
                )));
                return Outcome::Retry;
            }
        };
        let mut live = Live {
            client,
            control: self.control.clone(),
            timeout: self.config.call_timeout,
            seq: Sequencer::default(),
            durable: welcome
                .capabilities
                .iter()
                .any(|c| c == "durableInvalidations"),
            cursor: 0,
            needs_resync: false,
            next_catch_up: Instant::now(),
            sessions: Vec::new(),
            index: HashMap::new(),
            models: Vec::new(),
            current: None,
            auth_done: None,
            scanning: false,
            rescan: None,
        };
        live.seq.welcome(welcome.epoch);
        if let Err(error) = live.refresh_sessions() {
            self.report(Connection::Offline(format!(
                "could not read the conversations: {error}"
            )));
            return Outcome::Retry;
        }
        live.refresh_models(self, None);
        self.ever_connected = true;
        (self.sink)(LifecycleEvent::Catalog(live.catalog(self)));
        match live.call(Call::AuthState) {
            Ok(Reply::Auth(snapshot)) => live.publish_auth(self, &snapshot),
            other => log::warn!("could not read sign-in state: {other:?}"),
        }
        self.report(Connection::Ready);
        if let Some((conversation, generation, initial)) = self.last_open
            && live.open(self, conversation, generation, initial)
        {
            self.last_open = Some((conversation, generation, false));
        }
        live.reconcile(self);
        self.serve(&mut live)
    }

    fn serve(&mut self, live: &mut Live) -> Outcome {
        loop {
            let wait = live
                .next_wakeup()
                .into_iter()
                .chain(live.durable.then_some(live.next_catch_up))
                .min()
                .map(|at| at.saturating_duration_since(Instant::now()));
            let first = match wait {
                Some(wait) => match self.rx.recv_timeout(wait) {
                    Ok(m) => Some(m),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return Outcome::Shutdown,
                },
                None => match self.rx.recv() {
                    Ok(m) => Some(m),
                    Err(_) => return Outcome::Shutdown,
                },
            };
            let mut batch: Vec<Msg> = first.into_iter().collect();
            while batch.len() < BATCH {
                match self.rx.try_recv() {
                    Ok(m) => batch.push(m),
                    Err(_) => break,
                }
            }
            for message in batch {
                match message {
                    Msg::Shutdown => {
                        if self.supervisor.owns_engine() {
                            let _ = live.client.call(Call::Shutdown, SHUTDOWN_CALL);
                        }
                        live.client.close();
                        self.supervisor.stop();
                        live.client.join_reader(SHUTDOWN_CALL);
                        return Outcome::Shutdown;
                    }
                    Msg::Request(request) => live.handle_request(self, request),
                    Msg::StopFailed {
                        conversation,
                        generation,
                        op,
                        message,
                    } => self.emit_op(
                        conversation,
                        generation,
                        op,
                        EventKind::StopFailed { message },
                    ),
                    Msg::Import { step, done } => live.import(self, step, done),
                    Msg::EngineReady { conn, .. } if conn != self.conn => {}
                    Msg::EngineReady { pending, .. } => {
                        // Drain under the lock, then release it before doing any engine calls.
                        let frames: Vec<_> = {
                            let mut q = lock(&pending);
                            std::iter::from_fn(|| q.pop()).collect()
                        };
                        for frame in frames {
                            if let pipkin_engine_protocol::ServerFrame::Event {
                                epoch,
                                seq,
                                event,
                            } = frame
                            {
                                live.observe_event(self, epoch, seq, event);
                            }
                        }
                    }
                    Msg::Engine { conn, .. } if conn != self.conn => {}
                    Msg::Engine {
                        inbound: Inbound::Closed(reason),
                        ..
                    } => {
                        log::warn!("the engine connection ended: {reason}");
                        if !self.supervisor.alive() {
                            log::warn!("the engine is no longer running; restarting it");
                        }
                        live.client.close();
                        return Outcome::Retry;
                    }
                    Msg::Engine {
                        inbound: Inbound::Event { epoch, seq, event },
                        ..
                    } => live.observe_event(self, epoch, seq, *event),
                    Msg::Changes {
                        conversation,
                        generation,
                        scanned_through,
                        workspace,
                    } => live.changes_ready(
                        self,
                        conversation,
                        generation,
                        scanned_through,
                        workspace,
                    ),
                }
            }
            if live.durable && Instant::now() >= live.next_catch_up {
                live.catch_up(self, false);
            }
            live.flush_sync(self, false);
            live.flush_child(self);
            if let Some(current) = &live.current
                && current.workspace_ready
                && Instant::now() >= current.next_changes_scan
            {
                let (c, g) = (current.conversation, current.generation);
                live.schedule_changes(self, c, g);
            }
        }
    }
}

struct Current {
    conversation: ConversationId,
    generation: u64,
    session: String,
    cwd: Option<String>,
    mirror: Mirror,
    /// The mirror changed since the transcript was last sent.
    dirty: bool,
    last_sync: Instant,
    /// The child transcript selected in this session.
    child: Option<u64>,
    /// The selected child's session committed something since its newest page was last read.
    child_dirty: bool,
    child_sync: Instant,
    tools_done: usize,
    workspace_ready: bool,
    last_workspace: Option<Workspace>,
    publish_empty_changes: bool,
    next_changes_scan: Instant,
    /// Tool call and result entries, including older history, for the session's file scope.
    evidence: BTreeMap<u64, Entry>,
}

struct Live {
    client: Arc<Client>,
    control: Arc<control::Control>,
    timeout: Duration,
    seq: Sequencer,
    durable: bool,
    cursor: u64,
    needs_resync: bool,
    next_catch_up: Instant,
    sessions: Vec<(ConversationId, SessionSummary)>,
    index: HashMap<ConversationId, SessionSummary>,
    models: Vec<ModelInfo>,
    current: Option<Current>,
    auth_done: Option<String>,
    scanning: bool,
    rescan: Option<(ConversationId, u64)>,
}

impl Drop for Live {
    fn drop(&mut self) {
        self.control.clear(&self.client);
        self.client.close();
    }
}

/// How many tool calls in the mapped transcript have finished.
fn tools_done(items: &[pipkin_core::TranscriptItem]) -> usize {
    items
        .iter()
        .filter(|i| matches!(&i.kind, pipkin_core::ItemKind::Tool(t) if t.status != pipkin_core::ToolStatus::Running))
        .count()
}

fn images(prepared: &attach::Prepared) -> Vec<ImageInput> {
    prepared
        .images
        .iter()
        .filter_map(|v| {
            Some(ImageInput {
                mime_type: v.get("mimeType")?.as_str()?.to_owned(),
                data: v.get("data")?.as_str()?.to_owned(),
            })
        })
        .collect()
}

const NO_RECORD: &str = "The engine no longer has a record of this run. Its tool effects cannot be confirmed; inspect the project before sending more work.";

impl Live {
    fn call(&self, call: Call) -> Result<Reply, CallError> {
        self.client.call(call, self.timeout)
    }

    fn refresh_sessions(&mut self) -> Result<(), CallError> {
        match self.call(Call::ListSessions)? {
            Reply::Sessions { sessions } => {
                self.sessions = mapping::assign_ids(&sessions);
                self.index = self.sessions.iter().cloned().collect();
                self.control.bind(&self.client, &self.sessions);
                Ok(())
            }
            other => Err(CallError::Lost(format!("unexpected reply {other:?}"))),
        }
    }

    fn catalog(&self, worker: &Worker) -> pipkin_core::Bootstrap {
        mapping::catalog(
            &self.sessions,
            self.models.clone(),
            &worker.config.location,
            now_secs(),
        )
    }

    fn conversation_of(&self, session: &str) -> Option<ConversationId> {
        self.sessions
            .iter()
            .find(|(_, s)| s.id == session)
            .map(|(id, _)| *id)
    }

    fn session_of(&self, conversation: ConversationId) -> Option<&SessionSummary> {
        self.index.get(&conversation)
    }

    /// Re-read the models (for `session`, or the open one) and say which is selected.
    fn refresh_models(&mut self, worker: &mut Worker, session: Option<String>) {
        let session = session.or_else(|| self.current.as_ref().map(|c| c.session.clone()));
        match self.call(Call::Models {
            session: session.clone(),
        }) {
            Ok(Reply::Models(view)) => {
                self.models = mapping::models(&view);
                if session.is_some() {
                    (worker.sink)(LifecycleEvent::ModelSelected(view.selected.clone()));
                }
                let warning = mapping::refresh_warning(&view);
                if warning != worker.last_refresh_warning {
                    if let Some(text) = &warning {
                        worker.notice(text.clone());
                    }
                    worker.last_refresh_warning = warning;
                }
            }
            other => log::warn!("could not read the models: {other:?}"),
        }
    }

    fn publish_auth(
        &mut self,
        worker: &mut Worker,
        snapshot: &pipkin_engine_protocol::AuthSnapshot,
    ) {
        let state = mapping::sign_in(snapshot);
        let done = (state.status == SignInStatus::Done)
            .then(|| state.attempt.clone())
            .flatten();
        (worker.sink)(LifecycleEvent::SignIn(state));
        if let Some(attempt) = done.filter(|a| self.auth_done.as_ref() != Some(a)) {
            self.auth_done = Some(attempt);
            self.refresh_models(worker, None);
            (worker.sink)(LifecycleEvent::Catalog(self.catalog(worker)));
        }
    }

    /// When the loop must wake without a message: to send a coalesced transcript update, or to
    /// rescan the project.
    fn next_wakeup(&self) -> Option<Instant> {
        let current = self.current.as_ref()?;
        let sync = current.dirty.then_some(current.last_sync + SYNC_INTERVAL);
        let scan = current.workspace_ready.then_some(current.next_changes_scan);
        let child = current
            .child_dirty
            .then_some(current.child_sync + CHILD_SYNC_INTERVAL);
        [sync, scan, child].into_iter().flatten().min()
    }

    /// Send the open conversation's transcript if it changed, at most every `SYNC_INTERVAL`
    /// unless `force`.
    fn flush_sync(&mut self, worker: &Worker, force: bool) {
        let Some(current) = self.current.as_mut() else {
            return;
        };
        if !current.dirty || (!force && current.last_sync.elapsed() < SYNC_INTERVAL) {
            return;
        }
        current.dirty = false;
        current.last_sync = Instant::now();
        let mapped = transcript::map_entries_with_origin(
            &current.mirror.entries,
            &current.mirror.progress,
            Some(current.mirror.native_history_start),
        );
        let done = tools_done(&mapped.items);
        let finished_tool = done != current.tools_done;
        current.tools_done = done;
        let (conversation, generation) = (current.conversation, current.generation);
        worker.emit(
            conversation,
            generation,
            EventKind::Synced {
                items: mapped.items,
            },
        );
        if finished_tool {
            current.workspace_ready = true;
            self.schedule_changes(worker, conversation, generation);
        }
    }

    fn emit_engine_state(&self, worker: &Worker) {
        if let Some(c) = &self.current {
            worker.emit(
                c.conversation,
                c.generation,
                EventKind::EngineState {
                    busy: c.mirror.busy(),
                    queue: transcript::queue(&c.mirror.queue),
                },
            );
        }
    }

    fn handle_event(&mut self, worker: &mut Worker, event: Event) {
        match event {
            Event::ResyncRequired => self.catch_up(worker, true),
            Event::SessionsChanged => {
                self.resync(worker);
            }
            Event::ModelsChanged => {
                self.refresh_models(worker, None);
                (worker.sink)(LifecycleEvent::Catalog(self.catalog(worker)));
            }
            Event::AuthChanged(snapshot) => self.publish_auth(worker, &snapshot),
            Event::Notice { message } => worker.notice(message),
            Event::Session {
                session,
                revision,
                change,
            } => self.session_change(worker, session, revision, change),
        }
    }

    fn session_change(
        &mut self,
        worker: &mut Worker,
        session: String,
        revision: u64,
        change: SessionChange,
    ) {
        self.mark_child(&session, &change);
        match change {
            SessionChange::RunSettled {
                request, outcome, ..
            } => {
                let is_current = self.current.as_ref().is_some_and(|c| c.session == session);
                if is_current {
                    // The final reply reaches the core before its settlement.
                    self.flush_sync(worker, true);
                }
                let Some(request) = request else { return };
                let found = worker
                    .runs
                    .iter()
                    .find(|(_, r)| r.request.0 == request)
                    .map(|(op, r)| (*op, r.clone()));
                if let Some((op, run)) = found {
                    worker.runs.remove(&op);
                    worker.emit_op(
                        run.conversation,
                        run.generation,
                        op,
                        mapping::outcome(Some(&outcome)),
                    );
                    self.after_run(worker, run.conversation, run.generation);
                } else if is_current {
                    self.after_run_current(worker);
                }
            }
            SessionChange::QueueAdmitted { request, entry } => {
                if let Some(at) = worker
                    .queue_unknown
                    .iter()
                    .position(|u| u.request.0 == request)
                {
                    let req = worker.queue_unknown.remove(at);
                    worker.emit(
                        req.conversation,
                        req.generation,
                        EventKind::QueueAdmitted {
                            request: req.request,
                            entry: QueueId(entry),
                        },
                    );
                }
            }
            SessionChange::SummaryChanged { summary } => {
                if let Some(id) = self.conversation_of(&summary.id) {
                    self.index.insert(id, summary.clone());
                    if let Some(row) = self.sessions.iter_mut().find(|(i, _)| *i == id) {
                        row.1 = summary;
                    }
                    (worker.sink)(LifecycleEvent::Catalog(self.catalog(worker)));
                }
            }
            change => {
                let Some(current) = self.current.as_mut().filter(|c| c.session == session) else {
                    return;
                };
                if let SessionChange::EntryAppended { entry }
                | SessionChange::EntryReplaced { entry } = &change
                    && mapping::is_change_evidence(entry)
                {
                    current.evidence.insert(entry.id, entry.clone());
                }
                let (conversation, generation) = (current.conversation, current.generation);
                let touched = current.mirror.apply(revision, change);
                if touched == Touched::Transcript {
                    current.dirty = true;
                }
                self.touched(worker, conversation, generation, touched);
            }
        }
    }

    /// Report what one applied change touched in the open session.
    fn touched(
        &mut self,
        worker: &mut Worker,
        conversation: ConversationId,
        generation: u64,
        touched: Touched,
    ) {
        let Some(current) = self.current.as_ref() else {
            return;
        };
        match touched {
            Touched::Nothing | Touched::Transcript => {}
            Touched::Engine => {
                // The transcript first, so a run's end never outruns its content.
                self.flush_sync(worker, true);
                self.emit_engine_state(worker);
            }
            Touched::Usage => {
                let usage = transcript::usage(&current.mirror.usage, &current.session);
                worker.emit(conversation, generation, EventKind::UsageSynced(usage));
            }
            Touched::Thinking => {
                let t = &current.mirror.thinking;
                worker.emit(
                    conversation,
                    generation,
                    EventKind::ThinkingState {
                        level: t.level.clone(),
                        levels: t.levels.clone(),
                    },
                );
            }
            Touched::Ui => {
                let (requests, status, notices) = mapping::ui_state(&current.mirror.ui);
                worker.emit(
                    conversation,
                    generation,
                    EventKind::UiState {
                        requests,
                        status,
                        notices,
                    },
                );
            }
            Touched::Subagents => self.emit_subagents(worker),
            Touched::Plan => self.emit_plan(worker),
            Touched::Child(child) => {
                if current.child == Some(child) {
                    self.load_child(worker, conversation, generation, child);
                }
            }
            Touched::HasOlder(has_older) => {
                worker.emit(conversation, generation, EventKind::HasOlder(has_older))
            }
            Touched::Model(model) => (worker.sink)(LifecycleEvent::ModelSelected(model)),
            Touched::Lost => {
                self.resync(worker);
            }
        }
    }

    /// Files may have changed when a run ends.
    fn after_run(&mut self, worker: &Worker, conversation: ConversationId, generation: u64) {
        if self
            .current
            .as_ref()
            .is_some_and(|c| c.conversation == conversation && c.generation == generation)
        {
            self.after_run_current(worker);
        }
    }

    fn after_run_current(&mut self, worker: &Worker) {
        if let Some(c) = self.current.as_mut() {
            c.workspace_ready = true;
            let (conversation, generation) = (c.conversation, c.generation);
            self.schedule_changes(worker, conversation, generation);
        }
    }

    /// The engine's plan for the open session (live tasks), as the core's projection.
    fn emit_plan(&self, worker: &Worker) {
        let Some(c) = &self.current else { return };
        let (revision, tasks, counts, truncated) = mapping::plan(&c.mirror.plan);
        worker.emit(
            c.conversation,
            c.generation,
            EventKind::PlanSynced {
                revision,
                tasks,
                counts,
                truncated,
            },
        );
    }

    fn emit_subagents(&self, worker: &Worker) {
        let Some(c) = &self.current else { return };
        // No subagent support for this session: say nothing (the core defaults to unavailable).
        let Some(view) = &c.mirror.subagents else {
            return;
        };
        // The native engine pages child history, so its directory is marked pageable.
        let kind = match mapping::subagents(view) {
            Ok(children) => EventKind::SubagentDirectory {
                enabled: view.enabled,
                children,
                truncated: view.truncated,
            },
            Err(reason) => EventKind::SubagentsUnavailable(reason),
        };
        worker.emit(c.conversation, c.generation, kind);
    }

    /// A committed change in the selected child's own session makes its newest page stale.
    /// The page is re-read coalesced (`CHILD_SYNC_INTERVAL`), never per streamed token.
    fn mark_child(&mut self, session: &str, change: &SessionChange) {
        let Some(current) = self.current.as_mut() else {
            return;
        };
        let Some(child) = current.child else { return };
        let affects_history = matches!(
            change,
            SessionChange::EntryAppended { .. }
                | SessionChange::EntryReplaced { .. }
                | SessionChange::EntryDelta { .. }
                | SessionChange::RunSettled { .. }
                | SessionChange::BoundaryMoved { .. }
        );
        if affects_history
            && current.session != session
            && current.mirror.subagents.as_ref().is_some_and(|view| {
                view.children
                    .iter()
                    .any(|row| row.child == child && row.session == session)
            })
        {
            current.child_dirty = true;
        }
    }

    /// Re-read the selected child's newest page if it changed and the interval has passed.
    fn flush_child(&mut self, worker: &Worker) {
        let Some(current) = self.current.as_mut() else {
            return;
        };
        if !current.child_dirty || current.child_sync.elapsed() < CHILD_SYNC_INTERVAL {
            return;
        }
        current.child_dirty = false;
        let (conversation, generation) = (current.conversation, current.generation);
        if let Some(child) = current.child {
            self.load_child(worker, conversation, generation, child);
        }
    }

    /// Re-read everything derived from events: the catalogue, the open session, and the outcome
    /// of every run and queue request in doubt.
    fn resync(&mut self, worker: &mut Worker) -> bool {
        if self.refresh_sessions().is_err() {
            return false;
        }
        // A durable removal invalidates the catalog even if its live event was missed.
        // Drop local attachment/run state so an absent conversation cannot keep catch-up
        // or a cancelled operation pending forever.
        let removed_runs: Vec<_> = worker
            .runs
            .iter()
            .filter(|(_, run)| !self.index.contains_key(&run.conversation))
            .map(|(op, run)| (*op, run.clone()))
            .collect();
        for (op, run) in removed_runs {
            worker.runs.remove(&op);
            worker.emit_op(
                run.conversation,
                run.generation,
                op,
                EventKind::Failed {
                    message: "This conversation was removed from the engine.".into(),
                },
            );
        }
        worker
            .queue_unknown
            .retain(|req| self.index.contains_key(&req.conversation));
        if self
            .current
            .as_ref()
            .is_some_and(|c| !self.index.contains_key(&c.conversation))
        {
            self.current = None;
        }
        (worker.sink)(LifecycleEvent::Catalog(self.catalog(worker)));
        if let Some((conversation, generation, child)) = self
            .current
            .as_ref()
            .map(|c| (c.conversation, c.generation, c.child))
        {
            if !self.open(worker, conversation, generation, false) {
                return false;
            }
            if let Some(child) = child {
                if let Some(c) = self.current.as_mut() {
                    c.child = Some(child);
                }
                self.load_child(worker, conversation, generation, child);
            }
        }
        self.reconcile(worker);
        true
    }

    fn observe_event(&mut self, worker: &mut Worker, epoch: u64, seq: u64, event: Event) {
        match self.seq.observe(epoch, seq) {
            SeqCheck::InOrder => self.handle_event(worker, event),
            SeqCheck::Gap | SeqCheck::NewEpoch => self.catch_up(worker, true),
        }
    }

    fn catch_up(&mut self, worker: &mut Worker, force: bool) {
        // A transient read failure must not consume a reset request, especially for volatile
        // auth state which intentionally has no durable invalidation to trigger a later retry.
        let force = force || self.needs_resync;
        self.needs_resync = force;
        self.next_catch_up = Instant::now() + Duration::from_secs(1);
        if !self.durable {
            self.resync(worker);
            return;
        }
        // One bounded page per pass keeps Stop responsive even with a large backlog.
        let Ok(Reply::Events(page)) = self.call(Call::CatchUp {
            after: self.cursor,
            limit: 256,
        }) else {
            return;
        };
        if (force || page.reset || !page.events.is_empty()) && !self.resync(worker) {
            return;
        }
        if force {
            let Ok(Reply::Auth(auth)) = self.call(Call::AuthState) else {
                return;
            };
            self.publish_auth(worker, &auth);
            self.refresh_models(worker, None);
        }
        self.needs_resync = false;
        // A Lookup refusal leaves an operation in doubt. Poll it again even if its durable
        // invalidation has already been acknowledged and no further live event will arrive.
        self.reconcile(worker);
        if self
            .call(Call::AckEvents {
                through: page.through,
            })
            .is_ok()
        {
            self.cursor = page.through;
            if self.cursor < page.head {
                self.next_catch_up = Instant::now();
            }
        }
    }

    /// Ask the engine about every admitted run and every queue request in doubt.
    fn reconcile(&mut self, worker: &mut Worker) {
        let runs: Vec<(OperationId, Run)> =
            worker.runs.iter().map(|(op, r)| (*op, r.clone())).collect();
        for (op, run) in runs {
            let Some(session) = self.session_of(run.conversation).map(|s| s.id.clone()) else {
                continue;
            };
            let kind = match self.call(Call::Lookup {
                session,
                request: run.request.0.clone(),
            }) {
                Ok(Reply::Lookup(LookupResult::Known {
                    open: false,
                    outcome,
                    ..
                })) => mapping::outcome(outcome.as_ref()),
                Ok(Reply::Lookup(LookupResult::Unknown)) => EventKind::Failed {
                    message: NO_RECORD.into(),
                },
                // Still running, or we could not ask: its settlement event or a later check
                // will decide.
                _ => continue,
            };
            worker.runs.remove(&op);
            if self
                .current
                .as_ref()
                .is_some_and(|c| c.conversation == run.conversation)
            {
                self.flush_sync(worker, true);
            }
            worker.emit_op(run.conversation, run.generation, op, kind);
            self.after_run(worker, run.conversation, run.generation);
        }
        let pending = worker.queue_unknown.clone();
        for req in pending {
            self.resolve_queue_unknown(worker, req);
        }
    }

    fn resolve_queue_unknown(&mut self, worker: &mut Worker, req: QueueReq) {
        let Some(session) = self.session_of(req.conversation).map(|s| s.id.clone()) else {
            return;
        };
        match self.call(Call::Lookup {
            session,
            request: req.request.0.clone(),
        }) {
            Ok(Reply::Lookup(LookupResult::Known {
                entry: Some(entry), ..
            })) => {
                worker.queue_unknown.retain(|u| u.request != req.request);
                worker.emit(
                    req.conversation,
                    req.generation,
                    EventKind::QueueAdmitted {
                        request: req.request.clone(),
                        entry: QueueId(entry),
                    },
                );
            }
            // Never admitted: the same key again cannot admit it twice.
            Ok(Reply::Lookup(LookupResult::Unknown)) => self.queue_request(worker, req),
            _ => {}
        }
    }

    fn handle_request(&mut self, worker: &mut Worker, request: BackendRequest) {
        match request {
            BackendRequest::RemoveSignIn(provider) => {
                match self.call(Call::AuthRemove {
                    provider: provider.pi_id().into(),
                }) {
                    Ok(_) => {
                        self.refresh_models(worker, None);
                        (worker.sink)(LifecycleEvent::Catalog(self.catalog(worker)));
                    }
                    Err(error) => {
                        worker.notice(format!("Could not remove the saved connection: {error}"))
                    }
                }
            }
            BackendRequest::StartSignIn(provider) => self.auth_call(
                worker,
                Call::AuthStart {
                    provider: provider.pi_id().into(),
                },
                provider,
                "Could not start sign-in",
            ),
            BackendRequest::ReuseSignIn(provider) => self.auth_call(
                worker,
                Call::AuthReuse {
                    provider: provider.pi_id().into(),
                },
                provider,
                "Could not check your saved connection. Sign in again to continue",
            ),
            BackendRequest::AnswerSignIn {
                attempt,
                challenge,
                response,
            } => {
                if let Err(error) = self.call(Call::AuthAnswer {
                    attempt,
                    challenge,
                    response: Secret(response.expose().to_owned()),
                }) {
                    log::warn!("sign-in answer refused: {}", error_kind(&error));
                    worker.notice("The sign-in question expired. Try signing in again.".into());
                }
            }
            BackendRequest::CancelSignIn { attempt } => {
                // Safe to repeat; a closed connection cancels it anyway.
                let _ = self.call(Call::AuthCancel { attempt });
            }
            BackendRequest::Open {
                conversation,
                generation,
            } => {
                let opened = self.open(worker, conversation, generation, true);
                worker.last_open = Some((conversation, generation, !opened));
            }
            BackendRequest::OpenSubagent {
                conversation,
                generation,
                child,
            } => {
                let open = self
                    .current
                    .as_mut()
                    .filter(|c| c.conversation == conversation && c.generation == generation)
                    .map(|c| c.child = Some(child))
                    .is_some();
                match open {
                    true => self.load_child(worker, conversation, generation, child),
                    false => worker.emit(
                        conversation,
                        generation,
                        EventKind::SubagentViewFailed {
                            child,
                            reason: "The session is no longer open.".into(),
                        },
                    ),
                }
            }
            BackendRequest::LoadOlderSubagent {
                conversation,
                generation,
                child,
                before,
            } => self.load_older_child(worker, conversation, generation, child, before),
            BackendRequest::UpdatePlan {
                conversation,
                generation,
                request,
                task,
                status,
                reason,
                expected_revision,
            } => {
                let Some(session) = self.session_of(conversation).map(|s| s.id.clone()) else {
                    worker.emit(
                        conversation,
                        generation,
                        EventKind::PlanEditFailed {
                            request,
                            reason: "This conversation is not known to the engine.".into(),
                        },
                    );
                    return;
                };
                let op = pipkin_engine_protocol::PlanOp::Update {
                    task: pipkin_engine_protocol::TaskRef::Id(task),
                    title: None,
                    detail: None,
                    criteria: None,
                    depends_on: None,
                    parent: None,
                    status: Some(mapping::task_status_to_engine(status)),
                    reason,
                    owner: None,
                    add_evidence: Vec::new(),
                    clear_evidence: false,
                };
                // The published plan (PlanChanged), not this reply, shows the new state.
                let kind = match self.call(Call::UpdatePlan {
                    session,
                    request: request.0.clone(),
                    ops: vec![op],
                    expected_revision: Some(expected_revision),
                }) {
                    Ok(_) => EventKind::PlanEditApplied { request },
                    Err(error) => EventKind::PlanEditFailed {
                        request,
                        reason: error.to_string(),
                    },
                };
                worker.emit(conversation, generation, kind);
            }
            BackendRequest::SetSubagentsEnabled {
                conversation,
                enabled,
                ..
            } => {
                let Some(session) = self.session_of(conversation).map(|s| s.id.clone()) else {
                    return worker.notice("This conversation is not known to the engine.".into());
                };
                // The published directory, not this call, shows the effective setting.
                if let Err(error) = self.call(Call::SetSubagentsEnabled { session, enabled }) {
                    worker.notice(format!("Could not change the subagent setting: {error}"));
                }
            }
            BackendRequest::Submit {
                conversation,
                generation,
                op,
                request,
                text,
                attachments,
                ..
            } => self.submit(
                worker,
                conversation,
                generation,
                op,
                request,
                text,
                attachments,
            ),
            BackendRequest::Queue {
                conversation,
                generation,
                request,
                mode,
                text,
                attachments,
            } => self.queue_request(
                worker,
                QueueReq {
                    conversation,
                    generation,
                    request,
                    mode,
                    text,
                    attachments,
                },
            ),
            BackendRequest::CancelQueued {
                conversation,
                generation,
                entry,
            } => self.cancel_queued(worker, conversation, generation, entry),
            BackendRequest::RefreshModels { .. } => {
                if let Err(error) = self.call(Call::RefreshModels) {
                    worker.notice(format!("Could not refresh the models: {error}"));
                }
                self.refresh_models(worker, None);
                (worker.sink)(LifecycleEvent::Catalog(self.catalog(worker)));
            }
            BackendRequest::RefreshChanges {
                conversation,
                generation,
            } => {
                if let Some(current) = self
                    .current
                    .as_mut()
                    .filter(|c| c.conversation == conversation && c.generation == generation)
                {
                    current.workspace_ready = true;
                    current.last_workspace = None;
                    current.publish_empty_changes = true;
                    worker.emit(
                        conversation,
                        generation,
                        EventKind::ChangesScanState(ChangesState::Loading),
                    );
                    self.schedule_changes(worker, conversation, generation);
                } else {
                    worker.emit(
                        conversation,
                        generation,
                        EventKind::ChangesScanState(ChangesState::Unavailable(
                            "The conversation is no longer open.".into(),
                        )),
                    );
                }
            }
            request @ BackendRequest::Cancel { .. } => {
                worker.control.request(&request);
            }
            BackendRequest::CheckStatus {
                conversation,
                generation,
                op,
                request,
            } => self.check_status(worker, conversation, generation, op, request),
            BackendRequest::SetModel {
                conversation,
                model,
                ..
            } => {
                let Some(session) = self.session_of(conversation).map(|s| s.id.clone()) else {
                    return worker
                        .notice("Open the conversation first, then choose a model.".into());
                };
                // The engine's ModelChanged event, not this call, changes the selection.
                if let Err(error) = self.call(Call::SetModel { session, model }) {
                    worker.notice(format!("Could not switch the model: {error}"));
                }
            }
            BackendRequest::SetThinkingLevel {
                conversation,
                generation,
                level,
            } => {
                let Some(session) = self.session_of(conversation).map(|s| s.id.clone()) else {
                    return worker
                        .notice("Open the conversation first, then choose effort.".into());
                };
                match self.call(Call::SetThinking { session, level }) {
                    Ok(Reply::Thinking(view)) => worker.emit(
                        conversation,
                        generation,
                        EventKind::ThinkingState {
                            level: view.level,
                            levels: view.levels,
                        },
                    ),
                    Ok(_) => {}
                    Err(error) => worker.notice(format!("Could not change effort: {error}")),
                }
            }
            BackendRequest::CreateConversation { cwd, request, .. } => {
                match self.call(Call::CreateSession {
                    cwd,
                    request: request.0,
                }) {
                    Ok(_) => {
                        if self.refresh_sessions().is_ok() {
                            (worker.sink)(LifecycleEvent::Catalog(self.catalog(worker)));
                        }
                    }
                    Err(error) => {
                        worker.notice(format!("Could not create the conversation: {error}"))
                    }
                }
            }
            BackendRequest::LoadOlder {
                conversation,
                generation,
                before,
            } => self.load_older(worker, conversation, generation, before),
            BackendRequest::FetchToolOutput {
                conversation,
                generation,
                call_id,
            } => self.fetch_tool_output(worker, conversation, generation, call_id),
            BackendRequest::UiRespond {
                conversation,
                generation,
                id,
                answer,
            } => self.ui_respond(worker, conversation, generation, id, answer),
            BackendRequest::UiCancel {
                conversation, id, ..
            } => {
                let Some(session) = self.session_of(conversation).map(|s| s.id.clone()) else {
                    return;
                };
                if let Err(error) = self.call(Call::CancelQuestion {
                    session,
                    question: id,
                }) {
                    worker.notice(format!("Could not decline the question: {error}"));
                }
            }
            BackendRequest::Steer { .. } => {
                worker.notice("This engine steers through the queue.".into())
            }
        }
    }

    fn auth_call(&mut self, worker: &mut Worker, call: Call, provider: SignInProvider, what: &str) {
        match self.call(call) {
            Ok(Reply::Auth(snapshot)) => self.publish_auth(worker, &snapshot),
            Ok(_) => {}
            Err(CallError::Refused(e)) if e.code == ErrorCode::Unsupported => {
                // A reserved provider: say so; never fall back to anything else.
                worker.notice(format!("{what}: {}", e.message));
                log::info!("sign-in to {} is unavailable", provider.pi_id());
            }
            Err(error) => worker.notice(format!("{what}: {error}")),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn submit(
        &mut self,
        worker: &mut Worker,
        conversation: ConversationId,
        generation: u64,
        op: OperationId,
        request: RequestId,
        text: String,
        attachments: Vec<Attachment>,
    ) {
        let reject = |worker: &Worker, reason: String| {
            self.control.submitted(conversation, generation, op);
            worker.emit_op(conversation, generation, op, EventKind::Rejected { reason })
        };
        // Everything up to the send is a definite refusal: nothing left this process.
        let Some(summary) = self.session_of(conversation).cloned() else {
            return reject(
                worker,
                "This conversation is not known to the engine.".into(),
            );
        };
        let prepared = match attach::prepare(&text, &attachments, Some(Path::new(&summary.cwd))) {
            Ok(p) => p,
            Err(reason) => return reject(worker, reason),
        };
        let images = images(&prepared);
        // A Stop already crossing the wire finishes before a later local prompt is sent.
        // Stop workers also discard queued requests superseded by that new prompt.
        let order = self.control.order(conversation);
        let sending = order.as_ref().map(|o| lock(&o.sending));
        let result = self.call(Call::Prompt {
            session: summary.id.clone(),
            request: request.0.clone(),
            text: prepared.message,
            images,
            // The engine owns the session's model selection.
            model: None,
        });
        drop(sending);
        self.control.submitted(conversation, generation, op);
        match result {
            Ok(Reply::Admitted { .. }) => {
                worker.runs.insert(
                    op,
                    Run {
                        conversation,
                        generation,
                        request,
                    },
                );
                worker.emit_op(conversation, generation, op, EventKind::Accepted);
            }
            Err(CallError::Refused(e)) if mapping::definite_refusal(e.code) => {
                reject(worker, e.message)
            }
            Err(CallError::NotSent(reason)) => {
                reject(worker, format!("Could not send the message: {reason}"))
            }
            // Sent, and no answer we can trust: the outcome is unknown. Never resend.
            Ok(_) | Err(CallError::Refused(_)) | Err(CallError::Lost(_)) => {
                worker.emit_op(conversation, generation, op, EventKind::AckLost)
            }
        }
    }

    fn queue_request(&mut self, worker: &mut Worker, req: QueueReq) {
        worker.queue_unknown.retain(|u| u.request != req.request);
        let Some(summary) = self.session_of(req.conversation).cloned() else {
            return worker
                .queue_refused(&req, "This conversation is not known to the engine.".into());
        };
        let prepared =
            match attach::prepare(&req.text, &req.attachments, Some(Path::new(&summary.cwd))) {
                Ok(p) => p,
                Err(reason) => return worker.queue_refused(&req, reason),
            };
        let images = images(&prepared);
        let result = self.call(Call::Enqueue {
            session: summary.id,
            request: req.request.0.clone(),
            mode: match req.mode {
                QueueMode::Steer => InputMode::Steer,
                QueueMode::FollowUp => InputMode::FollowUp,
            },
            text: prepared.message,
            images,
        });
        match result {
            Ok(Reply::Admitted {
                entry: Some(entry), ..
            }) => worker.emit(
                req.conversation,
                req.generation,
                EventKind::QueueAdmitted {
                    request: req.request.clone(),
                    entry: QueueId(entry),
                },
            ),
            Err(CallError::Refused(e)) if mapping::definite_refusal(e.code) => {
                worker.queue_refused(&req, e.message)
            }
            Err(CallError::NotSent(reason)) => {
                worker.queue_refused(&req, format!("Could not send it: {reason}"))
            }
            // Admitted without an entry we can name, or no trustworthy answer: ask later.
            Ok(_) | Err(CallError::Refused(_)) | Err(CallError::Lost(_)) => worker.queue_lost(&req),
        }
    }

    fn cancel_queued(
        &mut self,
        worker: &mut Worker,
        conversation: ConversationId,
        generation: u64,
        entry: QueueId,
    ) {
        let Some(session) = self.session_of(conversation).map(|s| s.id.clone()) else {
            return worker
                .notice("Open the conversation first, then remove it from the queue.".into());
        };
        let outcome = match self.call(Call::CancelQueued {
            session,
            entry: entry.0,
        }) {
            Ok(Reply::CancelQueued { outcome }) => match outcome {
                CancelQueuedOutcome::Cancelled => CancelOutcome::Cancelled,
                CancelQueuedOutcome::AlreadyConsumed => CancelOutcome::AlreadyConsumed,
                CancelQueuedOutcome::NotFound => CancelOutcome::NotFound,
            },
            Ok(_) => {
                return worker.notice("The engine gave an answer that could not be read.".into());
            }
            Err(error) => {
                return worker.notice(format!("Could not remove it from the queue: {error}"));
            }
        };
        worker.emit(
            conversation,
            generation,
            EventKind::QueueCancelled { entry, outcome },
        );
    }

    fn check_status(
        &mut self,
        worker: &mut Worker,
        conversation: ConversationId,
        generation: u64,
        op: OperationId,
        request: Option<RequestId>,
    ) {
        let failed = |worker: &Worker, message: String| {
            worker.emit_op(
                conversation,
                generation,
                op,
                EventKind::StatusCheckFailed { message },
            )
        };
        let Some(request) = request else {
            // A run this window did not start has no key: re-read the session; its run state
            // (not an invented outcome) settles it.
            if self
                .current
                .as_ref()
                .is_some_and(|c| c.conversation == conversation && c.generation == generation)
            {
                self.open(worker, conversation, generation, false);
            } else {
                failed(
                    worker,
                    "The conversation is not open. Open it, then check again.".into(),
                );
            }
            return;
        };
        let Some(session) = self.session_of(conversation).map(|s| s.id.clone()) else {
            return failed(
                worker,
                "This conversation is not known to the engine.".into(),
            );
        };
        match self.call(Call::Lookup {
            session,
            request: request.0.clone(),
        }) {
            Ok(Reply::Lookup(LookupResult::Unknown)) => {
                // A run that was admitted and then lost its record is not an unsent prompt.
                let kind = if worker.runs.remove(&op).is_some() {
                    EventKind::Failed {
                        message: NO_RECORD.into(),
                    }
                } else {
                    EventKind::StatusResolved { accepted: false }
                };
                worker.emit_op(conversation, generation, op, kind);
            }
            Ok(Reply::Lookup(LookupResult::Known { open, outcome, .. })) => {
                worker.emit_op(conversation, generation, op, EventKind::StatusResolved { accepted: true });
                if open {
                    worker.runs.insert(
                        op,
                        Run {
                            conversation,
                            generation,
                            request,
                        },
                    );
                } else {
                    worker.runs.remove(&op);
                    if self.current.as_ref().is_some_and(|c| c.conversation == conversation) {
                        self.flush_sync(worker, true);
                    }
                    worker.emit_op(conversation, generation, op, mapping::outcome(outcome.as_ref()));
                    self.after_run(worker, conversation, generation);
                }
            }
            Ok(_) => failed(
                worker,
                "The engine gave no usable status. The outcome is still unresolved; try checking again.".into(),
            ),
            Err(error) => failed(
                worker,
                format!("Could not check with the engine: {error}. The outcome is still unresolved."),
            ),
        }
    }

    /// Skip pages containing only hidden system metadata, keeping the raw engine cursor.
    /// Otherwise a child could appear empty or request the same invisible page forever.
    fn child_history_page(
        &mut self,
        session: String,
        child: u64,
        mut before: Option<u64>,
    ) -> Result<Reply, CallError> {
        loop {
            let reply = self.call(Call::SubagentHistory {
                session: session.clone(),
                child,
                before,
                limit: SUBAGENT_PAGE,
            })?;
            let Reply::Page {
                entries,
                has_older,
                native_history_start,
            } = &reply
            else {
                return Ok(reply);
            };
            if !has_older
                || !transcript::map_entries_with_origin(
                    entries,
                    &HashMap::new(),
                    Some(*native_history_start),
                )
                .items
                .is_empty()
            {
                return Ok(reply);
            }
            let Some(next) = entries
                .first()
                .map(|e| e.id)
                .filter(|id| before.is_none_or(|b| *id < b))
            else {
                return Err(CallError::Refused(ProtoError::new(
                    ErrorCode::Invalid,
                    "The child history cursor did not advance.",
                )));
            };
            before = Some(next);
        }
    }

    fn load_child(
        &mut self,
        worker: &Worker,
        conversation: ConversationId,
        generation: u64,
        child: u64,
    ) {
        let Some(session) = self.current.as_ref().map(|c| c.session.clone()) else {
            return;
        };
        if let Some(current) = self.current.as_mut() {
            current.child_dirty = false;
            current.child_sync = Instant::now();
        }
        match self.child_history_page(session, child, None) {
            Ok(Reply::Page {
                entries,
                has_older,
                native_history_start,
            }) => worker.emit(
                conversation,
                generation,
                EventKind::SubagentPage {
                    child,
                    items: transcript::map_entries_with_origin(
                        &entries,
                        &HashMap::new(),
                        Some(native_history_start),
                    )
                    .items,
                    live: true,
                    has_older,
                },
            ),
            other => worker.emit(
                conversation,
                generation,
                EventKind::SubagentViewFailed {
                    child,
                    reason: match other {
                        Err(e) => format!("Could not read the subagent transcript: {e}"),
                        Ok(_) => {
                            "Could not read the subagent transcript. Try selecting it again.".into()
                        }
                    },
                },
            ),
        }
    }

    /// The bounded page of a child's history before `before`, ownership-checked by the
    /// engine. Every answer names the child and cursor so the core can drop stale ones.
    fn load_older_child(
        &mut self,
        worker: &Worker,
        conversation: ConversationId,
        generation: u64,
        child: u64,
        before: ItemId,
    ) {
        let fail = |reason: String| {
            worker.emit(
                conversation,
                generation,
                EventKind::SubagentOlderFailed {
                    child,
                    before,
                    reason,
                },
            )
        };
        let Some(session) = self
            .current
            .as_ref()
            .filter(|c| c.conversation == conversation && c.generation == generation)
            .map(|c| c.session.clone())
        else {
            return fail("The session is no longer open.".into());
        };
        let Some(entry) = transcript::entry_of_item(before) else {
            return fail("Earlier activity cannot be located for this entry.".into());
        };
        match self.child_history_page(session, child, Some(entry)) {
            Ok(Reply::Page {
                entries,
                has_older,
                native_history_start,
            }) => worker.emit(
                conversation,
                generation,
                EventKind::SubagentOlderPage {
                    child,
                    before,
                    items: transcript::map_entries_with_origin(
                        &entries,
                        &HashMap::new(),
                        Some(native_history_start),
                    )
                    .items,
                    has_older,
                },
            ),
            Ok(_) => fail("Could not load earlier subagent activity.".into()),
            Err(error) => fail(format!("Could not load earlier subagent activity: {error}")),
        }
    }

    fn load_older(
        &mut self,
        worker: &Worker,
        conversation: ConversationId,
        generation: u64,
        before: Option<ItemId>,
    ) {
        let fail = |worker: &Worker, message: String| {
            worker.emit(conversation, generation, EventKind::OlderFailed { message })
        };
        let Some(current) = self
            .current
            .as_ref()
            .filter(|c| c.conversation == conversation && c.generation == generation)
        else {
            return fail(worker, "The conversation is not open.".into());
        };
        let Some(entry) = before
            .and_then(transcript::entry_of_item)
            .or(current.mirror.oldest())
        else {
            return worker.emit(
                conversation,
                generation,
                EventKind::OlderPage {
                    items: vec![],
                    has_older: false,
                },
            );
        };
        let session = current.session.clone();
        match self.call(Call::HistoryPage {
            session,
            before: Some(entry),
            limit: HISTORY_PAGE,
        }) {
            Ok(Reply::Page {
                entries,
                has_older,
                native_history_start,
            }) => {
                let items = transcript::map_entries_with_origin(
                    &entries,
                    &HashMap::new(),
                    Some(native_history_start),
                )
                .items;
                if let Some(current) = self.current.as_mut() {
                    for e in entries.iter().filter(|e| mapping::is_change_evidence(e)) {
                        current.evidence.insert(e.id, e.clone());
                    }
                    current.mirror.prepend(entries, has_older);
                }
                worker.emit(
                    conversation,
                    generation,
                    EventKind::OlderPage { items, has_older },
                );
                self.schedule_changes(worker, conversation, generation);
            }
            Ok(_) => fail(worker, "Could not load earlier messages.".into()),
            Err(error) => fail(worker, format!("Could not load earlier messages: {error}")),
        }
    }

    fn fetch_tool_output(
        &mut self,
        worker: &Worker,
        conversation: ConversationId,
        generation: u64,
        call_id: String,
    ) {
        let Some(session) = self.session_of(conversation).map(|s| s.id.clone()) else {
            return worker.emit(
                conversation,
                generation,
                EventKind::ToolOutputUnavailable {
                    call_id,
                    reason: "This conversation is not known to the engine.".into(),
                },
            );
        };
        let kind = match self.call(Call::ToolOutput {
            session,
            call_id: call_id.clone(),
        }) {
            Ok(Reply::ToolOutput(view)) => {
                let mut text = view.text;
                if view.dropped_bytes > 0 {
                    text.push_str(&format!(
                        "\n[{} more bytes of output were not kept by the engine]",
                        view.dropped_bytes
                    ));
                }
                EventKind::ToolOutputFull { call_id, text }
            }
            other => {
                // An older result may still be in the history this window holds.
                let held = self
                    .current
                    .as_ref()
                    .and_then(|c| transcript::tool_result_text(&c.mirror.entries, &call_id));
                match (held, other) {
                    (Some(text), _) => EventKind::ToolOutputFull { call_id, text },
                    (None, Err(e)) => EventKind::ToolOutputUnavailable {
                        call_id,
                        reason: e.to_string(),
                    },
                    (None, Ok(_)) => EventKind::ToolOutputUnavailable {
                        call_id,
                        reason: "the engine does not have this result any more".into(),
                    },
                }
            }
        };
        worker.emit(conversation, generation, kind);
    }

    fn ui_respond(
        &mut self,
        worker: &Worker,
        conversation: ConversationId,
        generation: u64,
        id: String,
        answer: UiAnswer,
    ) {
        let refuse = |worker: &Worker, reason: String| {
            worker.emit(
                conversation,
                generation,
                EventKind::UiRespondRefused {
                    id: id.clone(),
                    reason,
                },
            )
        };
        let Some(session) = self.session_of(conversation).map(|s| s.id.clone()) else {
            return refuse(
                worker,
                "This conversation is not known to the engine.".into(),
            );
        };
        match self.call(Call::AnswerQuestion {
            session,
            question: id.clone(),
            answer: mapping::answer(answer),
        }) {
            Ok(_) => {}
            Err(error) => refuse(worker, error.to_string()),
        }
    }

    /// Read a session's snapshot and show it.
    fn open(
        &mut self,
        worker: &mut Worker,
        conversation: ConversationId,
        generation: u64,
        initial: bool,
    ) -> bool {
        let fail = |worker: &Worker, message: String| {
            if initial {
                worker.emit(conversation, generation, EventKind::OpenFailed { message });
            } else {
                log::warn!("could not refresh {conversation:?}: {message}");
            }
            false
        };
        let Some(summary) = self.session_of(conversation).cloned() else {
            return fail(
                worker,
                "This conversation no longer exists in the engine.".into(),
            );
        };
        // A deferred scan belongs to the session that asked for it.
        if self
            .current
            .as_ref()
            .is_some_and(|c| c.conversation != conversation)
        {
            self.rescan = None;
        }
        let mut snapshot = match self.call(Call::OpenSession {
            session: summary.id.clone(),
            window: None,
        }) {
            Ok(Reply::Snapshot(s)) => *s,
            Ok(other) => return fail(worker, format!("unexpected reply {other:?}")),
            Err(error) => return fail(worker, format!("Could not read the conversation: {error}")),
        };
        // Catch-up must not collapse history the person already paged in. Refresh the same
        // range from the store, including any changed retirement/result state, before replacing it.
        let oldest = (!initial)
            .then(|| {
                self.current
                    .as_ref()
                    .filter(|c| c.conversation == conversation)
                    .and_then(|c| c.mirror.oldest())
            })
            .flatten();
        if let Some(oldest) = oldest {
            while snapshot.has_older && snapshot.entries.first().is_some_and(|e| e.id > oldest) {
                let before = snapshot.entries.first().map(|e| e.id);
                match self.call(Call::HistoryPage {
                    session: summary.id.clone(),
                    before,
                    limit: HISTORY_PAGE,
                }) {
                    Ok(Reply::Page {
                        entries, has_older, ..
                    }) if !entries.is_empty() && entries.first().map(|e| e.id) < before => {
                        snapshot.has_older = has_older;
                        snapshot.entries.splice(0..0, entries);
                    }
                    _ => return fail(worker, "Could not refresh the loaded history".into()),
                }
            }
        }
        if let Some(old) = self.current.as_ref().filter(|c| c.session != summary.id) {
            // Attachment release never cancels a background run. The engine keeps its
            // runtime until active work and every other client's demand have ended.
            if let Err(error) = self.call(Call::ReleaseSession {
                session: old.session.clone(),
            }) {
                log::warn!("could not release conversation attachment: {error}");
            }
        }
        let previous = self
            .current
            .take()
            .filter(|c| c.conversation == conversation);
        let mirror = Mirror::new(snapshot);
        let mapped = transcript::map_entries_with_origin(
            &mirror.entries,
            &mirror.progress,
            Some(mirror.native_history_start),
        );
        let done = tools_done(&mapped.items);
        let has_older = mirror.has_older;
        worker.emit(
            conversation,
            generation,
            if initial {
                EventKind::Opened {
                    items: mapped.items,
                    has_older,
                    changes: vec![],
                }
            } else {
                EventKind::Synced {
                    items: mapped.items,
                }
            },
        );
        if !initial {
            worker.emit(conversation, generation, EventKind::HasOlder(has_older));
        }
        worker.emit(
            conversation,
            generation,
            EventKind::UsageSynced(transcript::usage(&mirror.usage, &summary.id)),
        );
        worker.emit(
            conversation,
            generation,
            EventKind::EngineState {
                busy: mirror.busy(),
                queue: transcript::queue(&mirror.queue),
            },
        );
        if !mirror.thinking.level.is_empty() {
            worker.emit(
                conversation,
                generation,
                EventKind::ThinkingState {
                    level: mirror.thinking.level.clone(),
                    levels: mirror.thinking.levels.clone(),
                },
            );
        }
        let (requests, status, notices) = mapping::ui_state(&mirror.ui);
        worker.emit(
            conversation,
            generation,
            EventKind::UiState {
                requests,
                status,
                notices,
            },
        );
        // File-change evidence: the window, plus a bounded reach into older history.
        let mut evidence: BTreeMap<u64, Entry> = previous.map(|p| p.evidence).unwrap_or_default();
        for e in mirror
            .entries
            .iter()
            .filter(|e| mapping::is_change_evidence(e))
        {
            evidence.insert(e.id, e.clone());
        }
        if has_older {
            let mut before = mirror.oldest();
            for _ in 0..EVIDENCE_PAGES {
                let Some(b) = before else { break };
                let Ok(Reply::Page {
                    entries, has_older, ..
                }) = self.call(Call::HistoryPage {
                    session: summary.id.clone(),
                    before: Some(b),
                    limit: HISTORY_PAGE,
                })
                else {
                    break;
                };
                let next = entries.first().map(|e| e.id);
                for e in entries.into_iter().filter(mapping::is_change_evidence) {
                    evidence.insert(e.id, e);
                }
                if !has_older || next.is_none() || next == before {
                    break;
                }
                before = next;
            }
        }
        let cwd = Some(summary.cwd.clone()).filter(|c| c.starts_with('/'));
        self.current = Some(Current {
            conversation,
            generation,
            session: summary.id.clone(),
            cwd,
            mirror,
            dirty: false,
            last_sync: Instant::now(),
            child: None,
            child_dirty: false,
            child_sync: Instant::now(),
            tools_done: done,
            workspace_ready: done > 0 || has_older,
            last_workspace: None,
            publish_empty_changes: false,
            next_changes_scan: Instant::now(),
            evidence,
        });
        self.refresh_models(worker, Some(summary.id.clone()));
        (worker.sink)(LifecycleEvent::Catalog(self.catalog(worker)));
        self.emit_subagents(worker);
        self.emit_plan(worker);
        self.schedule_changes(worker, conversation, generation);
        true
    }

    /// Collect uncommitted Git changes scoped to the session's file mutations, off this thread.
    fn schedule_changes(&mut self, worker: &Worker, conversation: ConversationId, generation: u64) {
        let Some(current) = self
            .current
            .as_mut()
            .filter(|c| c.conversation == conversation && c.generation == generation)
        else {
            return;
        };
        if !current.workspace_ready {
            return;
        }
        if self.scanning {
            self.rescan = Some((conversation, generation));
            return;
        }
        current.next_changes_scan = Instant::now() + CHANGES_POLL;
        let after = worker
            .changes_bases
            .get(&conversation)
            .copied()
            .unwrap_or(0);
        let paths = mapping::changed_paths(current.evidence.values(), after);
        let scanned_through = current.evidence.keys().next_back().copied().unwrap_or(0);
        let cwd = current.cwd.clone();
        self.scanning = true;
        let tx = worker.tx.clone();
        thread::spawn(move || {
            let workspace = match cwd {
                Some(cwd) => workspace::collect_scoped(Path::new(&cwd), &paths),
                None if paths.is_empty() => Workspace::Changes {
                    files: vec![],
                    truncated: false,
                },
                None => {
                    Workspace::Unavailable("The engine did not report a project folder.".into())
                }
            };
            let _ = tx.send(Msg::Changes {
                conversation,
                generation,
                scanned_through,
                workspace,
            });
        });
    }

    fn changes_ready(
        &mut self,
        worker: &mut Worker,
        conversation: ConversationId,
        generation: u64,
        scanned_through: u64,
        workspace: Workspace,
    ) {
        self.scanning = false;
        let had_files = self.current.as_ref().is_some_and(|c| {
            matches!(&c.last_workspace, Some(Workspace::Changes { files, .. }) if !files.is_empty())
        });
        let changed = self
            .current
            .as_mut()
            .filter(|c| c.conversation == conversation && c.generation == generation)
            .is_some_and(|current| {
                // A clean scan retires everything it covered: committed work does not return.
                if matches!(&workspace, Workspace::Changes { files, truncated: false } if files.is_empty()) {
                    let base = worker.changes_bases.entry(conversation).or_default();
                    *base = (*base).max(scanned_through);
                }
                if current.last_workspace.as_ref() == Some(&workspace) {
                    return false;
                }
                current.last_workspace = Some(workspace.clone());
                true
            });
        if changed {
            let kind = match workspace {
                Workspace::Changes { files, .. } => {
                    let explicit = self
                        .current
                        .as_mut()
                        .is_some_and(|c| std::mem::take(&mut c.publish_empty_changes));
                    if explicit || had_files || !files.is_empty() {
                        worker.emit(conversation, generation, EventKind::ChangesSynced(files));
                    }
                    EventKind::ChangesScanState(ChangesState::Ready)
                }
                Workspace::NotARepository => {
                    EventKind::ChangesScanState(ChangesState::NotARepository)
                }
                Workspace::Unavailable(reason) => {
                    log::warn!("workspace changes unavailable: {reason}");
                    EventKind::ChangesScanState(ChangesState::Unavailable(reason))
                }
            };
            worker.emit(conversation, generation, kind);
        }
        if let Some((conversation, generation)) = self.rescan.take() {
            self.schedule_changes(worker, conversation, generation);
        }
    }
}

/// A short, secret-free description of a failed call, for logs.
fn error_kind(error: &CallError) -> &'static str {
    match error {
        CallError::NotSent(_) => "not sent",
        CallError::Refused(ProtoError { .. }) => "refused",
        CallError::Lost(_) => "lost",
    }
}

/// Where the engine worker's stderr is appended, for a desktop data directory.
pub fn engine_log_path(desktop_data_dir: &Path) -> PathBuf {
    desktop_data_dir.join("engine.log")
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) mod e2e;
