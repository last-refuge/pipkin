//! The Pi engine adapter: maps Pi's replicated services onto Pipkin's core.
//!
//! One worker thread owns the visible connection. It discovers a trusted local server (or
//! launches and owns one), mirrors the session directory into the catalogue, and opens a
//! conversation by attaching its session and subscribing to its `Transcript` and `Models`
//! services. Detached runs started here are watched on separate read-only connections. Every update
//! is checked against the live subscription, so traffic from an attachment the user has moved
//! away from cannot reach the screen.
//!
//! Prompts go through `AgentController` with the submission's journaled request key, so the
//! engine deduplicates and a lost acknowledgment can be resolved by asking it. A run's
//! settlement is observed by short `lookup` polls, never by a long-lived call: Pi releases an
//! attachment only after its in-flight calls finish, so a call that waited for the whole run
//! would block switching sessions.
//!
//! Steers and follow-ups go through the same controller with their own journaled keys. The
//! engine owns the queue (`pi.inbox`), so the queue shown is whatever the engine reports. One
//! lost acknowledgment is resolved by asking the engine about its key and, if it never saw it,
//! sending the very same key again: the engine admits a key once, so that cannot duplicate.
//!
//! The adapter never replays a *prompt* after a disconnect. It reconnects, resubscribes and
//! refreshes what is shown; anything that mutates stays the user's explicit decision.
//!
//! A conversation imported into the built-in engine is owned by it (the desktop's record, see
//! [`OwnershipCheck`]). Besides the core's own refusal, this adapter refuses at its boundary
//! anything that would change such a conversation, with a definite answer and nothing sent.

pub mod attach;
mod auth;
pub mod engine;
pub mod session;
mod session_changes;
mod subagents;
pub mod transcript;
pub mod workspace;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pi_client::Error;
use pi_client::chord::{Mode, ServiceCall};
use pi_client::client::{Client, ClientEvent, ClientOptions, Subscription};
use pi_client::protocol::RpcTarget;
use pi_client::unix::{self, ServerRoute};
use pipkin_core::{
    Attachment, Backend, BackendEvent, BackendRequest, CancelOutcome, ChangesState, Connection,
    ConversationId, EventKind, ItemId, LifecycleEvent, LifecycleSink, ModelInfo, OperationId,
    QueueId, QueueMode, RequestId, UiAnswer,
    onboarding::{SignInSnapshot, SignInStatus},
};
use serde_json::{Value, json};

use self::engine::{EngineConfig, EngineHost};
use self::session::Session;
use self::workspace::Workspace;

const CALL_TIMEOUT: Duration = Duration::from_secs(15);
const ATTACH_TIMEOUT: Duration = Duration::from_secs(5);
/// How often a conversation with a run in flight is polled for its outcome, as a fallback to
/// the transcript updates that normally trigger the check first.
const RUN_POLL: Duration = Duration::from_millis(400);
const CHANGES_POLL: Duration = Duration::from_secs(2);
/// Messages handled per wakeup before remapping, so a burst becomes one refresh.
const BATCH: usize = 256;
/// Waits before each retry of an attach that failed with `internal_error`; at most this many
/// retries.
const ATTACH_RETRY_DELAYS: [Duration; 4] = [
    Duration::from_millis(150),
    Duration::from_millis(400),
    Duration::from_millis(1000),
    Duration::from_millis(2000),
];
/// A subscription that keeps failing is not worth patching; resync through a reconnect.
const MAX_RESUBSCRIBES: u32 = 3;
/// How long shutdown waits for the worker, and, when it owns the engine, for the engine to stop.
/// Entries asked for per page of older history.
const HISTORY_PAGE: u32 = 50;
/// Pages of history searched, at most, for a tool result that is no longer in the live view.
const TOOL_OUTPUT_PAGES: usize = 20;
const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);
const SHUTDOWN_WAIT_OWNED_ENGINE: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
pub struct PiConfig {
    /// The Pi server profile directory (sockets and `default-server-id`).
    pub directory: PathBuf,
    /// Use this logical server instead of discovering one.
    pub server_id: Option<String>,
    /// Delay between connection attempts while no server is reachable.
    pub retry_delay: Duration,
    /// Launch and own an engine instead of expecting one to be running.
    pub engine: Option<EngineConfig>,
    /// Asks the desktop's record whether the built-in engine owns a conversation, at the moment
    /// a change to it would be sent. `None`: no such record is kept (tests, other tools).
    pub owned_elsewhere: Option<OwnershipCheck>,
}

/// Whether the built-in engine owns a conversation, read fresh each time so an import made by
/// another window is seen too. An error is a reason not to send (fail closed).
#[derive(Clone)]
pub struct OwnershipCheck(pub Arc<dyn Fn(ConversationId) -> Result<bool, String> + Send + Sync>);

impl std::fmt::Debug for OwnershipCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OwnershipCheck")
    }
}

impl PiConfig {
    pub fn new(directory: PathBuf) -> Self {
        PiConfig {
            directory,
            server_id: None,
            retry_delay: Duration::from_secs(3),
            engine: None,
            owned_elsewhere: None,
        }
    }

    /// Own the engine described by `engine`: connect to exactly that profile and server.
    pub fn managed(engine: EngineConfig) -> Self {
        PiConfig {
            directory: engine.server_dir.clone(),
            server_id: Some(engine.server_id.clone()),
            retry_delay: Duration::from_secs(3),
            engine: Some(engine),
            owned_elsewhere: None,
        }
    }
}

/// Where Pi keeps its server profile unless told otherwise: `PI_SERVER_DIR` or `~/.pi/server`.
pub fn default_directory() -> PathBuf {
    if let Some(dir) = std::env::var_os("PI_SERVER_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".pi").join("server")
}

enum Msg {
    Request(BackendRequest),
    Client(ClientEvent),
    /// A workspace scan finished on its own thread.
    Changes {
        conversation: ConversationId,
        generation: u64,
        scanned_through: u64,
        workspace: Workspace,
    },
    BackgroundSettled {
        op: OperationId,
        request: RequestId,
        kind: EventKind,
    },
    Shutdown,
}

pub struct PiBackend {
    config: PiConfig,
    events: crate::event_queue::EventSender,
    tx: Sender<Msg>,
    rx: Mutex<Option<Receiver<Msg>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl PiBackend {
    pub fn new(events: impl Into<crate::event_queue::EventSender>, config: PiConfig) -> Self {
        let events = events.into();
        let (tx, rx) = mpsc::channel();
        PiBackend {
            config,
            events,
            tx,
            rx: Mutex::new(Some(rx)),
            worker: Mutex::new(None),
        }
    }
}

impl Backend for PiBackend {
    fn start(&self, sink: LifecycleSink) {
        let Some(rx) = lock(&self.rx).take() else {
            return; // already started
        };
        let worker = Worker {
            engine: self.config.engine.clone().map(EngineHost::new),
            config: self.config.clone(),
            events: self.events.clone(),
            sink,
            tx: self.tx.clone(),
            rx,
            last_open: None,
            ever_connected: false,
            runs: HashMap::new(),
            watchers: HashMap::new(),
            queue_unknown: Vec::new(),
            last_refresh_warning: None,
        };
        let handle = thread::Builder::new()
            .name("pi-backend".into())
            .spawn(move || worker.run())
            .expect("spawn pi backend");
        *lock(&self.worker) = Some(handle);
    }

    fn request(&self, request: BackendRequest) {
        let _ = self.tx.send(Msg::Request(request));
    }

    fn shutdown(&self) {
        let _ = self.tx.send(Msg::Shutdown);
        if let Some(handle) = lock(&self.worker).take() {
            // An engine this adapter owns must be stopped before the process exits, or it would
            // be orphaned; give that longer than a plain disconnect.
            let wait = if self.config.engine.is_some() {
                SHUTDOWN_WAIT_OWNED_ENGINE
            } else {
                SHUTDOWN_WAIT
            };
            let deadline = Instant::now() + wait;
            while !handle.is_finished() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            // A worker stuck in a call is abandoned rather than blocking application exit.
        }
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

/// A prompt the engine accepted whose outcome has not been seen yet.
#[derive(Clone, Debug)]
struct Run {
    conversation: ConversationId,
    generation: u64,
    request: RequestId,
}

struct Worker {
    config: PiConfig,
    engine: Option<EngineHost>,
    events: crate::event_queue::EventSender,
    sink: LifecycleSink,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    /// The conversation most recently opened, refreshed after a reconnect.
    last_open: Option<(ConversationId, u64)>,
    ever_connected: bool,
    /// Accepted prompts awaiting an outcome. Kept across reconnects: they are reconciled by
    /// asking the engine, never by resending.
    runs: HashMap<OperationId, Run>,
    /// Detached-session observers; the main connection remains attached to the selected session.
    watchers: HashMap<OperationId, Arc<AtomicBool>>,
    /// Queue requests whose acknowledgment was lost, resolved when the conversation is attached.
    queue_unknown: Vec<QueueReq>,
    /// The last model-refresh warning shown, so it is said once.
    last_refresh_warning: Option<String>,
}

enum Outcome {
    Shutdown,
    Retry,
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
        for stop in self.watchers.values() {
            stop.store(true, Ordering::Relaxed);
        }
        // Stop only what this adapter started.
        if let Some(engine) = self.engine.as_mut() {
            engine.stop();
        }
    }

    fn now() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64)
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

    fn stop_watching(&mut self, op: OperationId) {
        if let Some(stop) = self.watchers.remove(&op) {
            stop.store(true, Ordering::Relaxed);
        }
    }

    fn background_settled(&mut self, op: OperationId, request: &RequestId, kind: EventKind) {
        let Some(run) = self
            .runs
            .get(&op)
            .filter(|r| &r.request == request)
            .cloned()
        else {
            return;
        };
        self.runs.remove(&op);
        self.stop_watching(op);
        self.emit_op(run.conversation, run.generation, op, kind);
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

    /// Sleep out the retry delay, still answering requests so nothing waits on a dead link.
    /// Returns true on shutdown.
    fn wait_to_retry(&mut self) -> bool {
        let deadline = Instant::now() + self.config.retry_delay;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(remaining) {
                Err(mpsc::RecvTimeoutError::Timeout) => return false,
                Err(mpsc::RecvTimeoutError::Disconnected) | Ok(Msg::Shutdown) => return true,
                Ok(Msg::Request(request)) => {
                    if let Some(request) = self.refuse_owned(request) {
                        self.refuse_offline(request)
                    }
                }
                Ok(Msg::BackgroundSettled { op, request, kind }) => {
                    self.background_settled(op, &request, kind);
                }
                Ok(Msg::Client(_)) | Ok(Msg::Changes { .. }) => {} // from a connection that is gone
            }
        }
    }

    /// Refuse, before anything reaches Pi, a change to a conversation the built-in engine owns:
    /// a prompt is rejected (never an unknown outcome), queued input is refused, an answer is
    /// refused, settings are declined with a notice. Stopping, status checks and reads pass.
    /// Returns the request when it may go ahead.
    fn refuse_owned(&self, request: BackendRequest) -> Option<BackendRequest> {
        let Some(check) = &self.config.owned_elsewhere else {
            return Some(request);
        };
        let conversation = match &request {
            BackendRequest::Submit { conversation, .. }
            | BackendRequest::Steer { conversation, .. }
            | BackendRequest::Queue { conversation, .. }
            | BackendRequest::CancelQueued { conversation, .. }
            | BackendRequest::SetModel { conversation, .. }
            | BackendRequest::SetThinkingLevel { conversation, .. }
            | BackendRequest::SetSubagentsEnabled { conversation, .. }
            | BackendRequest::UiRespond { conversation, .. } => *conversation,
            _ => return Some(request),
        };
        let reason = match (check.0)(conversation) {
            Ok(false) => return Some(request),
            Ok(true) => pipkin_core::NATIVE_OWNED_NOTICE.to_string(),
            Err(e) => format!(
                "Not sent: Pipkin could not confirm which engine owns this conversation ({e})."
            ),
        };
        log::info!("refused a change to a conversation Pi does not own: {reason}");
        match request {
            BackendRequest::Submit {
                conversation,
                generation,
                op,
                ..
            } => self.emit_op(conversation, generation, op, EventKind::Rejected { reason }),
            BackendRequest::Queue {
                conversation,
                generation,
                request,
                ..
            } => self.emit(
                conversation,
                generation,
                EventKind::QueueRefused { request, reason },
            ),
            BackendRequest::UiRespond {
                conversation,
                generation,
                id,
                ..
            } => self.emit(
                conversation,
                generation,
                EventKind::UiRespondRefused { id, reason },
            ),
            _ => self.notice(reason),
        }
        None
    }

    fn refuse_offline(&self, request: BackendRequest) {
        const OFFLINE: &str = "Not connected to the Pi engine.";
        match request {
            BackendRequest::Open {
                conversation,
                generation,
            } => self.emit(
                conversation,
                generation,
                EventKind::OpenFailed {
                    message: OFFLINE.into(),
                },
            ),
            // Nothing was sent, so this is a definite refusal, never an unknown outcome.
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
            // Nothing was sent: the text goes back to the draft.
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
            BackendRequest::SetSubagentsEnabled { .. }
            | BackendRequest::RemoveSignIn(_)
            | BackendRequest::StartSignIn(_)
            | BackendRequest::ReuseSignIn(_)
            | BackendRequest::AnswerSignIn { .. }
            | BackendRequest::CancelSignIn { .. }
            | BackendRequest::CreateConversation { .. }
            | BackendRequest::SetModel { .. }
            | BackendRequest::SetThinkingLevel { .. } => self.notice(OFFLINE.into()),
            // These need the engine; the user can try again.
            BackendRequest::CancelQueued { .. }
            | BackendRequest::RefreshModels { .. }
            | BackendRequest::UiRespond { .. }
            | BackendRequest::UiCancel { .. } => self.notice(OFFLINE.into()),
            BackendRequest::Cancel {
                conversation,
                generation,
                op,
            } => self.emit_op(
                conversation,
                generation,
                op,
                EventKind::StopFailed {
                    message: format!("Could not ask Pi to stop: {OFFLINE}"),
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
            other => log::warn!("not supported while offline: {other:?}"),
        }
    }

    fn resolve(&self) -> Result<ServerRoute, Connection> {
        let dir = &self.config.directory;
        if let Some(id) = &self.config.server_id {
            return Ok(ServerRoute {
                server_id: id.clone(),
                path: dir.join(format!("{id}.sock")),
            });
        }
        let probe = ClientOptions::new("00000000-0000-4000-8000-000000000000");
        let found = unix::discover(dir, &probe).map_err(|e| Connection::Offline(e.to_string()))?;
        let mut routes = found.routes;
        match routes.len() {
            // A server may be running behind permissions we refuse; say so instead of "none".
            0 if !found.untrusted.is_empty() => {
                Err(Connection::Failed(found.untrusted[0].1.to_string()))
            }
            0 if !found.incompatible.is_empty() => Err(Connection::Incompatible(
                found.incompatible[0].1.to_string(),
            )),
            0 => Err(Connection::Offline(format!(
                "no Pi server is running in {}. Start one with the Pi experimental server.",
                dir.display()
            ))),
            1 => Ok(routes.remove(0)),
            _ => {
                // Prefer the server Pi itself would pick.
                let default =
                    std::fs::read_to_string(dir.join("default-server-id")).unwrap_or_default();
                match routes.iter().position(|r| r.server_id == default.trim()) {
                    Some(i) => Ok(routes.remove(i)),
                    None => Err(Connection::Failed(format!(
                        "{} Pi servers are running in {}; choose one with --pi-server-id.",
                        routes.len(),
                        dir.display()
                    ))),
                }
            }
        }
    }

    fn connect_and_serve(&mut self) -> Outcome {
        (self.sink)(LifecycleEvent::SignIn(SignInSnapshot::default()));
        self.report(if self.ever_connected {
            Connection::Reconnecting
        } else {
            Connection::Connecting
        });
        // An engine we own is started (or restarted, within a cap) before connecting.
        if let Some(engine) = self.engine.as_mut()
            && let Err(error) = engine.ensure_running()
        {
            self.report(Connection::Failed(error.to_string()));
            return Outcome::Retry;
        }
        let route = match self.resolve() {
            Ok(route) => route,
            Err(connection) => {
                self.report(connection);
                return Outcome::Retry;
            }
        };
        let (client, events) =
            match unix::connect(&route.path, ClientOptions::new(&route.server_id)) {
                Ok(pair) => pair,
                Err(error) => {
                    self.report(match &error {
                        Error::Untrusted(_) => Connection::Failed(error.to_string()),
                        Error::Server { code, .. } if code == "version" => {
                            Connection::Incompatible(error.to_string())
                        }
                        _ => Connection::Offline(error.to_string()),
                    });
                    return Outcome::Retry;
                }
            };
        // Forward client events into the one queue this thread reads.
        let forward_tx = self.tx.clone();
        thread::spawn(move || {
            while let Ok(event) = events.recv_blocking() {
                if forward_tx.send(Msg::Client(event)).is_err() {
                    break;
                }
            }
        });
        let mut live = match Live::start(&client, &route, &self.config) {
            Ok(live) => live,
            Err(connection) => {
                client.disconnect();
                self.report(connection);
                return Outcome::Retry;
            }
        };
        self.ever_connected = true;
        (self.sink)(LifecycleEvent::Catalog(live.catalog()));
        live.publish_auth(self);
        self.report(Connection::Ready);
        // After a reconnect, refresh what is on screen instead of leaving it stale.
        if let Some((conversation, generation)) = self.last_open {
            live.open(self, conversation, generation, false);
        }
        self.serve(&mut live)
    }

    fn serve(&mut self, live: &mut Live) -> Outcome {
        loop {
            // While a run is in flight in the open conversation, wake regularly to ask how it
            // went; otherwise sleep until something happens.
            let polling =
                live.has_runs(self) || live.current.as_ref().is_some_and(|c| c.workspace_ready);
            let first = if polling {
                match self.rx.recv_timeout(RUN_POLL) {
                    Ok(m) => Some(m),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return Outcome::Shutdown,
                }
            } else {
                match self.rx.recv() {
                    Ok(m) => Some(m),
                    Err(_) => return Outcome::Shutdown,
                }
            };
            let mut batch = Vec::new();
            batch.extend(first);
            while batch.len() < BATCH {
                match self.rx.try_recv() {
                    Ok(m) => batch.push(m),
                    Err(_) => break,
                }
            }
            let mut dirty = Dirty::default();
            for message in batch {
                match message {
                    Msg::Shutdown => {
                        live.client.disconnect();
                        return Outcome::Shutdown;
                    }
                    Msg::Request(request) => {
                        if let Some(request) = self.refuse_owned(request) {
                            live.handle_request(self, request)
                        }
                    }
                    Msg::Client(ClientEvent::Disconnected(error)) => {
                        log::warn!("Pi connection lost: {error}");
                        return Outcome::Retry;
                    }
                    Msg::Client(event) => {
                        if let Some(outcome) = live.handle_event(self, event, &mut dirty) {
                            return outcome;
                        }
                    }
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
                    Msg::BackgroundSettled { op, request, kind } => {
                        self.background_settled(op, &request, kind);
                    }
                }
            }
            live.flush(self, dirty);
            // Settlement is checked after every wakeup that could have changed it: a transcript
            // update, or the poll timer.
            live.poll_runs(self);
            if let Some(current) = &live.current
                && current.workspace_ready
                && Instant::now() >= current.next_changes_scan
            {
                live.schedule_changes(self, current.conversation, current.generation);
            }
        }
    }
}

#[derive(Default)]
struct Dirty {
    auth: bool,
    directory: bool,
    transcript: bool,
    models: bool,
    ui: bool,
    subagents: bool,
    child: bool,
}

struct Current {
    conversation: ConversationId,
    session_id: String,
    generation: u64,
    target: RpcTarget,
    transcript: Subscription,
    models: Option<Subscription>,
    /// Questions extensions ask, if the engine has the service.
    ui: Option<Subscription>,
    /// Child directory and effective enable setting, only on a qualified Pi worker.
    subagents: Option<Subscription>,
    /// The live, read-only keyed transcript selected in this session.
    child: Option<(u64, Subscription)>,
    /// The session's working directory, for workspace changes.
    cwd: Option<String>,
    /// What the last transcript showed, to notice when tools finish or a run ends.
    tools_done: usize,
    busy: bool,
    /// A new, untouched session must not inherit the project's already-dirty diff.
    workspace_ready: bool,
    /// Last scan published for this attachment; unchanged background results do not repaint.
    last_workspace: Option<Workspace>,
    publish_empty_changes: bool,
    next_changes_scan: Instant,
    /// Raw durable entries retain tool edit evidence even after live-view compaction.
    change_entries: std::collections::BTreeMap<u64, Value>,
    /// The oldest entry the live view held last time, to notice a compaction or reset moving it.
    oldest_entry: Option<u64>,
}

struct Live {
    client: Client,
    route: ServerRoute,
    location: String,
    dir: Subscription,
    auth: Option<Subscription>,
    auth_done: Option<String>,
    sessions: Vec<(ConversationId, Session)>,
    index: HashMap<ConversationId, Session>,
    models: Vec<ModelInfo>,
    current: Option<Current>,
    resubscribes: u32,
    /// A workspace scan is running; another is wanted once it ends.
    scanning: bool,
    rescan: Option<(ConversationId, u64)>,
    changes_bases: HashMap<ConversationId, u64>,
}

fn server_target(route: &ServerRoute) -> RpcTarget {
    RpcTarget::Server {
        server_id: route.server_id.clone(),
    }
}

/// A completed engine outcome for a submission, as the core should see it.
fn outcome_event(reason: Option<&str>, detail: Option<&str>) -> EventKind {
    match reason {
        None => EventKind::Completed,
        Some("aborted") => EventKind::Cancelled,
        Some("model_error") => EventKind::Failed {
            message: detail
                .filter(|d| !d.is_empty())
                .map_or_else(|| "The model returned an error.".to_owned(), str::to_owned),
        },
        Some("no_model") => EventKind::Failed {
            message: "No model is selected for this session. Choose one and send again.".into(),
        },
        Some("stale") => EventKind::Failed {
            message: "The message was superseded and never answered.".into(),
        },
        Some(other) => EventKind::Failed {
            message: format!("The run ended without an answer ({other})."),
        },
    }
}

/// The `AgentController` prompt/steer/followUp argument for a prepared message.
fn prompt_args(prepared: attach::Prepared, request: &RequestId) -> Value {
    let images = if prepared.images.is_empty() {
        Value::Null
    } else {
        Value::Array(prepared.images)
    };
    json!({ "message": prepared.message, "images": images, "requestId": request.0 })
}

/// What `AgentController.lookup` said about a request key.
enum Lookup {
    Unknown,
    Known {
        /// Running or queued.
        open: bool,
        /// The engine's id for the submission, which is its queue entry id while queued.
        operation: Option<u64>,
        reason: Option<String>,
        detail: Option<String>,
    },
}

fn parse_lookup(value: &Value) -> Option<Lookup> {
    if value.get("found")?.as_bool()? {
        let status = value.get("status")?.as_str()?;
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
        Some(Lookup::Known {
            open: matches!(status, "queued" | "placed"),
            operation: text("operationId").and_then(|id| id.parse().ok()),
            reason: text("reason"),
            detail: text("detail"),
        })
    } else {
        Some(Lookup::Unknown)
    }
}

/// Poll one detached session on its own connection. Never move the visible client's attachment.
/// The request key identifies the run; a lost connection only triggers a fresh lookup, not a
/// second submission. The worker validates the key again before publishing an outcome.
fn watch_background(
    tx: Sender<Msg>,
    route: ServerRoute,
    session_id: String,
    op: OperationId,
    request: RequestId,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::Relaxed) {
        let result = (|| -> Result<Option<EventKind>, Error> {
            let (client, _events) =
                unix::connect(&route.path, ClientOptions::new(&route.server_id))?;
            let result = (|| -> Result<Option<EventKind>, Error> {
                let attach =
                    ServiceCall::new("pi.session-management", "attach", vec![json!(session_id)]);
                client
                    .request(&server_target(&route), &attach)?
                    .wait_timeout(CALL_TIMEOUT)?;
                let deadline = Instant::now() + ATTACH_TIMEOUT;
                let target = loop {
                    if stop.load(Ordering::Relaxed) {
                        return Ok(None);
                    }
                    if let Some(attachment) = client.attachment()
                        && attachment.session_id == session_id
                    {
                        break attachment.rpc();
                    }
                    if Instant::now() >= deadline {
                        return Err(Error::Timeout);
                    }
                    thread::sleep(Duration::from_millis(10));
                };
                while !stop.load(Ordering::Relaxed) {
                    let answer = client
                        .request(
                            &target,
                            &ServiceCall::new(
                                "pi.agent-controller",
                                "lookup",
                                vec![json!(request.0)],
                            ),
                        )?
                        .wait_timeout(CALL_TIMEOUT)?;
                    match answer.as_ref().and_then(parse_lookup) {
                        Some(Lookup::Known {
                            open: false,
                            reason,
                            detail,
                            ..
                        }) => {
                            return Ok(Some(outcome_event(reason.as_deref(), detail.as_deref())));
                        }
                        Some(Lookup::Unknown) => {
                            return Ok(Some(EventKind::Failed {
                                message: "Pi no longer has a record of this run. Its tool effects cannot be confirmed; inspect the project before sending more work.".into(),
                            }));
                        }
                        _ => thread::sleep(RUN_POLL),
                    }
                }
                Ok(None)
            })();
            client.disconnect();
            result
        })();
        match result {
            Ok(Some(kind)) => {
                let _ = tx.send(Msg::BackgroundSettled {
                    op,
                    request: request.clone(),
                    kind,
                });
                break;
            }
            Ok(None) => break,
            Err(error) => {
                log::debug!("background session lookup will retry: {error}");
                thread::sleep(Duration::from_secs(1));
            }
        }
    }
}

impl Live {
    fn server(&self) -> RpcTarget {
        RpcTarget::Server {
            server_id: self.client.server_id().to_owned(),
        }
    }

    fn start(client: &Client, route: &ServerRoute, config: &PiConfig) -> Result<Live, Connection> {
        let target = server_target(route);
        // The services this milestone needs. A missing one is an incompatible engine, which no
        // amount of retrying fixes, so say exactly what is missing.
        let catalogue = client
            .catalogue(&target, CALL_TIMEOUT)
            .map_err(|e| Connection::Offline(e.to_string()))?;
        for required in ["pi.session-directory", "pi.session-management"] {
            if !catalogue.iter().any(|e| e.service_id == required) {
                return Err(Connection::Incompatible(format!(
                    "the engine does not provide {required}"
                )));
            }
        }
        let dir = client
            .subscribe(
                &target,
                "pi.session-directory",
                Mode::Singleton,
                CALL_TIMEOUT,
            )
            .map_err(|e| Connection::Offline(e.to_string()))?;
        let auth = if catalogue.iter().any(|e| e.service_id == "pi.provider-auth") {
            client
                .subscribe(&target, "pi.provider-auth", Mode::Singleton, CALL_TIMEOUT)
                .ok()
        } else {
            None
        };
        let mut live = Live {
            client: client.clone(),
            route: route.clone(),
            location: display_path(&config.directory),
            dir,
            auth,
            auth_done: None,
            sessions: Vec::new(),
            index: HashMap::new(),
            models: Vec::new(),
            current: None,
            resubscribes: 0,
            scanning: false,
            rescan: None,
            changes_bases: HashMap::new(),
        };
        live.refresh_directory();
        Ok(live)
    }

    fn auth_state(&self) -> Option<SignInSnapshot> {
        let value = self
            .auth
            .as_ref()?
            .read(|r| r.state("state").cloned())
            .flatten()?;
        let state = auth::parse(&value);
        state.available.then_some(state)
    }

    fn publish_auth(&self, worker: &Worker) {
        (worker.sink)(LifecycleEvent::SignIn(
            self.auth_state().unwrap_or_default(),
        ));
    }

    fn refresh_directory(&mut self) {
        let state = self
            .dir
            .read(|r| r.state("state").cloned())
            .flatten()
            .unwrap_or(Value::Null);
        self.sessions = session::assign_ids(&session::parse_directory(&state));
        self.index = session::index(&self.sessions);
    }

    fn catalog(&self) -> pipkin_core::Bootstrap {
        session::catalog(
            &self.sessions,
            self.models.clone(),
            &self.location,
            Worker::now(),
        )
    }

    /// One short call to the open session's `AgentController` (or `Models`) service.
    fn call(
        &self,
        target: &RpcTarget,
        service: &str,
        member: &str,
        args: Vec<Value>,
    ) -> Result<Option<Value>, Error> {
        let pending = self
            .client
            .request(target, &ServiceCall::new(service, member, args))?;
        match pending.wait_timeout(CALL_TIMEOUT) {
            Err(Error::Timeout) => {
                pending.cancel();
                Err(Error::Timeout)
            }
            other => other,
        }
    }

    fn handle_request(&mut self, worker: &mut Worker, request: BackendRequest) {
        match request {
            BackendRequest::RemoveSignIn(provider) => {
                if self.auth.is_none()
                    || self
                        .call(
                            &self.server(),
                            "pi.provider-auth",
                            "remove",
                            vec![json!(provider.pi_id())],
                        )
                        .is_err()
                {
                    worker.notice(
                        "Could not remove the saved connection. Update the engine or try again."
                            .into(),
                    );
                } else if let Some(conversation) = self.current.as_ref().map(|c| c.conversation) {
                    self.refresh_models(worker, conversation);
                }
            }
            BackendRequest::StartSignIn(provider) => {
                if self.auth.is_none() {
                    worker.notice("This Pi engine does not support subscription sign-in. Update the bundled engine and try again.".into());
                } else if self
                    .call(
                        &self.server(),
                        "pi.provider-auth",
                        "start",
                        vec![json!(provider.pi_id())],
                    )
                    .is_err()
                {
                    worker.notice(
                        "Could not start sign-in. Check the engine connection and try again."
                            .into(),
                    );
                }
            }
            BackendRequest::ReuseSignIn(provider) => {
                if self.auth.is_some()
                    && self
                        .call(
                            &self.server(),
                            "pi.provider-auth",
                            "reuse",
                            vec![json!(provider.pi_id())],
                        )
                        .is_err()
                {
                    worker.notice(
                        "Could not check your saved connection. Sign in again to continue.".into(),
                    );
                }
            }
            BackendRequest::AnswerSignIn {
                attempt,
                challenge,
                response,
            } => {
                if self.auth.is_some()
                    && self
                        .call(
                            &self.server(),
                            "pi.provider-auth",
                            "answer",
                            vec![json!(attempt), json!(challenge), json!(response.expose())],
                        )
                        .is_err()
                {
                    worker.notice("The sign-in question expired. Try signing in again.".into());
                }
            }
            BackendRequest::CancelSignIn { attempt } => {
                if self.auth.is_some() {
                    // Cancellation is safe to repeat and a disconnected client is cancelled by Pi.
                    let _ = self.call(
                        &self.server(),
                        "pi.provider-auth",
                        "cancel",
                        vec![json!(attempt)],
                    );
                }
            }
            BackendRequest::Open {
                conversation,
                generation,
            } => {
                worker.last_open = Some((conversation, generation));
                self.open(worker, conversation, generation, true);
            }
            BackendRequest::OpenSubagent {
                conversation,
                generation,
                child,
            } => {
                self.open_subagent(worker, conversation, generation, child);
            }
            // Pi serves one bounded child view; it has no older-page cursor. The core never
            // asks for one (Pi views are unpaged), but an answer must still be honest.
            BackendRequest::LoadOlderSubagent {
                conversation,
                generation,
                child,
                before,
            } => worker.emit(
                conversation,
                generation,
                EventKind::SubagentOlderFailed {
                    child,
                    before,
                    reason: "Earlier subagent activity is not available from Pi.".into(),
                },
            ),
            BackendRequest::SetSubagentsEnabled {
                conversation,
                generation,
                enabled,
            } => {
                if let Some(current) = &self.current
                    && current.conversation == conversation
                    && current.generation == generation
                    && current.subagents.is_some()
                {
                    let target = current.target.clone();
                    if let Err(error) =
                        self.call(&target, "pi.subagents", "setEnabled", vec![json!(enabled)])
                    {
                        worker.notice(format!("Could not change the subagent setting: {error}"));
                    }
                    // Wait for the replicated configuration; a method ack can precede its
                    // publication. Never display the requested value as if it were effective.
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
            BackendRequest::RefreshModels { conversation, .. } => {
                self.refresh_models(worker, conversation)
            }
            BackendRequest::RefreshChanges {
                conversation,
                generation,
            } => {
                if let Some(current) = self.current.as_mut()
                    && current.conversation == conversation
                    && current.generation == generation
                {
                    // An explicit refresh may scan even an untouched session. Automatic scans
                    // still wait until work starts so a fresh conversation keeps its calm state.
                    current.workspace_ready = true;
                    // Only explicit refreshes expose progress. Routine scans keep the current
                    // inspector body mounted, including its empty state.
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
                            "The conversation is no longer attached.".into(),
                        )),
                    );
                }
            }
            BackendRequest::Cancel {
                conversation,
                generation,
                op,
            } => self.cancel(worker, conversation, generation, op),
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
            } => self.set_model(worker, conversation, &model),
            BackendRequest::SetThinkingLevel {
                conversation,
                level,
                ..
            } => self.set_thinking_level(worker, conversation, &level),
            BackendRequest::CreateConversation { cwd, request, .. } => {
                self.create_conversation(worker, &cwd, &request)
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
            } => self.ui_cancel(worker, conversation, id),
            // The demo's steer; real steers arrive as `Queue`.
            BackendRequest::Steer { .. } => {
                worker.notice("This engine steers through the queue.".into())
            }
        }
    }

    /// The open conversation's target, if `conversation` is the one attached.
    fn target_for(&self, conversation: ConversationId) -> Option<RpcTarget> {
        self.current
            .as_ref()
            .filter(|c| c.conversation == conversation)
            .map(|c| c.target.clone())
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
        let reject = |reason: &str| {
            worker.emit_op(
                conversation,
                generation,
                op,
                EventKind::Rejected {
                    reason: reason.into(),
                },
            )
        };
        // Everything below sends nothing, so each is a definite refusal.
        let Some(target) = self.target_for(conversation) else {
            return reject("The session is not open on the Pi server.");
        };
        let prepared = match self.prepare(&text, &attachments) {
            Ok(prepared) => prepared,
            Err(reason) => return reject(&reason),
        };
        let call = ServiceCall::new(
            "pi.agent-controller",
            "prompt",
            vec![prompt_args(prepared, &request)],
        );
        let pending = match self.client.request(&target, &call) {
            Ok(p) => p,
            // The call never left: a definite refusal.
            Err(error) => return reject(&format!("Could not send the message: {error}")),
        };
        let ack_lost = |worker: &Worker| {
            worker.emit_op(conversation, generation, op, EventKind::AckLost);
        };
        match pending.wait_timeout(CALL_TIMEOUT) {
            Ok(Some(reply)) => match reply.get("accepted").and_then(Value::as_bool) {
                Some(true) => {
                    worker.runs.insert(
                        op,
                        Run {
                            conversation,
                            generation,
                            request,
                        },
                    );
                    worker.emit_op(conversation, generation, op, EventKind::Accepted);
                    self.schedule_changes(worker, conversation, generation);
                }
                Some(false) => {
                    let reason = reply["error"]["message"]
                        .as_str()
                        .unwrap_or("Pi did not accept the message.");
                    reject(reason);
                }
                // An answer we cannot read: it may have been accepted.
                None => ack_lost(worker),
            },
            // An internal error may come after the engine admitted the input: unknown.
            Err(Error::Server { code, .. }) if code == "internal_error" => ack_lost(worker),
            // The engine answered with a definite refusal (not attached, unknown service, ...).
            Err(Error::Server { message, .. }) => reject(&message),
            Err(Error::Timeout) => {
                pending.cancel();
                ack_lost(worker);
            }
            // Connection loss or anything else after sending: the outcome is unknown.
            Err(_) | Ok(None) => ack_lost(worker),
        }
    }

    /// Re-check the attachments against their files and build the message Pi takes.
    fn prepare(&self, text: &str, attachments: &[Attachment]) -> Result<attach::Prepared, String> {
        let cwd = self.current.as_ref().and_then(|c| c.cwd.clone());
        attach::prepare(text, attachments, cwd.as_deref().map(Path::new))
    }

    /// Steer the active run or queue a follow-up. Both go to the engine under the journaled
    /// key, so sending one again (after a lost acknowledgment) cannot admit it twice.
    fn queue_request(&mut self, worker: &mut Worker, req: QueueReq) {
        worker.queue_unknown.retain(|u| u.request != req.request);
        let Some(target) = self.target_for(req.conversation) else {
            return worker.queue_refused(&req, "The session is not open on the Pi server.".into());
        };
        let prepared = match self.prepare(&req.text, &req.attachments) {
            Ok(prepared) => prepared,
            Err(reason) => return worker.queue_refused(&req, reason),
        };
        let member = match req.mode {
            QueueMode::Steer => "steer",
            QueueMode::FollowUp => "followUp",
        };
        let call = ServiceCall::new(
            "pi.agent-controller",
            member,
            vec![prompt_args(prepared, &req.request)],
        );
        let pending = match self.client.request(&target, &call) {
            Ok(p) => p,
            // The call never left: a definite refusal.
            Err(error) => {
                return worker.queue_refused(&req, format!("Could not send it: {error}"));
            }
        };
        match pending.wait_timeout(CALL_TIMEOUT) {
            Ok(Some(reply)) => match reply.get("accepted").and_then(Value::as_bool) {
                Some(true) => {
                    let entry = reply
                        .get("entryId")
                        .and_then(Value::as_str)
                        .and_then(|id| id.parse::<u64>().ok());
                    match entry {
                        Some(entry) => {
                            worker.emit(
                                req.conversation,
                                req.generation,
                                EventKind::QueueAdmitted {
                                    request: req.request.clone(),
                                    entry: QueueId(entry),
                                },
                            );
                            self.schedule_changes(worker, req.conversation, req.generation);
                        }
                        // Admitted, but we cannot tell which entry: ask again.
                        None => worker.queue_lost(&req),
                    }
                }
                Some(false) => {
                    let reason = reply["error"]["message"]
                        .as_str()
                        .unwrap_or("Pi did not accept it.")
                        .to_owned();
                    worker.queue_refused(&req, reason);
                }
                None => worker.queue_lost(&req),
            },
            Err(Error::Server { code, .. }) if code == "internal_error" => worker.queue_lost(&req),
            Err(Error::Server { message, .. }) => worker.queue_refused(&req, message),
            Err(Error::Timeout) => {
                pending.cancel();
                worker.queue_lost(&req);
            }
            Err(_) | Ok(None) => worker.queue_lost(&req),
        }
    }

    /// Settle the queue requests whose acknowledgment was lost: ask the engine about each key,
    /// and send the key again if it has no record of it. The engine admits a key once, so a
    /// resend after a request that did arrive just returns the original.
    fn resolve_queue_unknown(&mut self, worker: &mut Worker) {
        let Some(open) = self.current.as_ref().map(|c| c.conversation) else {
            return;
        };
        let pending: Vec<QueueReq> = worker
            .queue_unknown
            .iter()
            .filter(|u| u.conversation == open)
            .cloned()
            .collect();
        for req in pending {
            let Some(target) = self.target_for(req.conversation) else {
                continue;
            };
            let answer = self
                .call(
                    &target,
                    "pi.agent-controller",
                    "lookup",
                    vec![json!(req.request.0)],
                )
                .map(|v| v.as_ref().and_then(parse_lookup));
            match answer {
                Ok(Some(Lookup::Known {
                    operation: Some(entry),
                    ..
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
                Ok(Some(Lookup::Unknown)) => self.queue_request(worker, req),
                // Could not ask, or an answer we cannot read: look again next time.
                _ => {}
            }
        }
    }

    fn cancel_queued(
        &mut self,
        worker: &mut Worker,
        conversation: ConversationId,
        generation: u64,
        entry: QueueId,
    ) {
        let Some(target) = self.target_for(conversation) else {
            return worker
                .notice("Open the conversation first, then remove it from the queue.".into());
        };
        let answer = self.call(
            &target,
            "pi.agent-controller",
            "cancelQueued",
            vec![json!(entry.0.to_string())],
        );
        let outcome = match answer {
            Ok(Some(v)) => match v.get("outcome").and_then(Value::as_str) {
                Some("cancelled") => CancelOutcome::Cancelled,
                Some("already_consumed") => CancelOutcome::AlreadyConsumed,
                Some("not_found") => CancelOutcome::NotFound,
                _ => return worker.notice("Pi gave an answer that could not be read.".into()),
            },
            Ok(None) => return worker.notice("Pi gave no answer; try again.".into()),
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

    fn refresh_models(&mut self, worker: &mut Worker, conversation: ConversationId) {
        let Some(target) = self.target_for(conversation) else {
            return worker.notice("Open a conversation first, then refresh the models.".into());
        };
        if let Err(error) = self.call(&target, "pi.models", "refresh", vec![]) {
            worker.notice(format!("Could not refresh the models: {error}"));
        }
    }

    fn cancel(
        &mut self,
        worker: &mut Worker,
        conversation: ConversationId,
        generation: u64,
        op: OperationId,
    ) {
        let failed = |worker: &Worker, message| {
            worker.emit_op(
                conversation,
                generation,
                op,
                EventKind::StopFailed { message },
            )
        };
        let Some(target) = self.target_for(conversation) else {
            return failed(
                worker,
                "The session is not open, so the run cannot be asked to stop.".into(),
            );
        };
        if let Err(error) = self.call(&target, "pi.agent-controller", "abort", vec![]) {
            // A failed or lost stop request is not settlement. Let the person check/retry the
            // stop, and preserve the engine's actual outcome even if it finishes with an error.
            failed(worker, format!("Could not ask Pi to stop: {error}"));
        }
    }

    fn check_status(
        &mut self,
        worker: &mut Worker,
        conversation: ConversationId,
        generation: u64,
        op: OperationId,
        request: Option<RequestId>,
    ) {
        let failed = |worker: &Worker, message| {
            worker.emit_op(
                conversation,
                generation,
                op,
                EventKind::StatusCheckFailed { message },
            )
        };
        let Some(request) = request else {
            // An adopted run has no local request key. Read the live engine replica rather
            // than inventing a key or a terminal outcome. Busy=false settles adopted runs only.
            if self.current.as_ref().is_some_and(|c| {
                c.conversation == conversation
                    && c.generation == generation
                    && c.transcript
                        .read(|r| r.state("state").is_some())
                        .unwrap_or(false)
            }) {
                self.flush(
                    worker,
                    Dirty {
                        transcript: true,
                        ..Dirty::default()
                    },
                );
            } else {
                failed(worker, "The live conversation is not available. Wait for it to reconnect, then check again.".into());
            }
            return;
        };
        let Some(target) = self.target_for(conversation) else {
            return failed(
                worker,
                "Open the conversation first, then check again.".into(),
            );
        };
        let answer = self
            .call(
                &target,
                "pi.agent-controller",
                "lookup",
                vec![json!(request.0)],
            )
            .map(|v| v.as_ref().and_then(parse_lookup));
        match answer {
            Ok(Some(Lookup::Unknown)) => {
                // A previously accepted run losing its record is not an unadmitted prompt.
                // Never tell the person it had no side effects merely because lookup is empty.
                let kind = if worker.runs.remove(&op).is_some() {
                    worker.stop_watching(op);
                    EventKind::Failed { message: "Pi no longer has a record of this run. Its tool effects cannot be confirmed; inspect the project before sending more work.".into() }
                } else {
                    EventKind::StatusResolved { accepted: false }
                };
                worker.emit_op(conversation, generation, op, kind);
            }
            Ok(Some(Lookup::Known {
                open,
                reason,
                detail,
                ..
            })) => {
                worker.emit_op(
                    conversation,
                    generation,
                    op,
                    EventKind::StatusResolved { accepted: true },
                );
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
                    worker.stop_watching(op);
                    // Put the latest durable reply into core before settlement/goal evaluation;
                    // the lookup response can beat the view's queued dirty notification.
                    self.sync_settled_conversation(worker, conversation);
                    let kind = outcome_event(reason.as_deref(), detail.as_deref());
                    worker.emit_op(conversation, generation, op, kind);
                    self.schedule_changes(worker, conversation, generation);
                }
            }
            Ok(None) => failed(
                worker,
                "Pi gave no usable status. The outcome is still unresolved; try checking again."
                    .into(),
            ),
            Err(error) => failed(
                worker,
                format!("Could not check with Pi: {error}. The outcome is still unresolved."),
            ),
        }
    }

    /// Settlement may refresh its own live replica, never whichever session is now selected.
    fn sync_settled_conversation(&mut self, worker: &mut Worker, conversation: ConversationId) {
        if self
            .current
            .as_ref()
            .is_some_and(|c| c.conversation == conversation)
        {
            self.flush(
                worker,
                Dirty {
                    transcript: true,
                    ..Dirty::default()
                },
            );
        }
    }

    /// Ask the engine how each in-flight prompt of the open conversation went.
    fn poll_runs(&mut self, worker: &mut Worker) {
        let Some(current) = &self.current else { return };
        let (conversation, target) = (current.conversation, current.target.clone());
        let ops: Vec<(OperationId, Run)> = worker
            .runs
            .iter()
            .filter(|(_, r)| r.conversation == conversation)
            .map(|(op, r)| (*op, r.clone()))
            .collect();
        for (op, run) in ops {
            let answer = self
                .call(
                    &target,
                    "pi.agent-controller",
                    "lookup",
                    vec![json!(run.request.0)],
                )
                .map(|v| v.as_ref().and_then(parse_lookup));
            let event = match answer {
                // Still running, or we could not ask: look again next time.
                Ok(Some(Lookup::Known { open: true, .. })) | Err(_) | Ok(None) => continue,
                Ok(Some(Lookup::Known { reason, detail, .. })) => {
                    outcome_event(reason.as_deref(), detail.as_deref())
                }
                Ok(Some(Lookup::Unknown)) => EventKind::Failed {
                    message: "Pi no longer has a record of this run. Its tool effects cannot be confirmed; inspect the project before sending more work.".into(),
                },
            };
            worker.runs.remove(&op);
            worker.stop_watching(op);
            self.sync_settled_conversation(worker, run.conversation);
            worker.emit_op(run.conversation, run.generation, op, event);
            if let Some(current) = self
                .current
                .as_mut()
                .filter(|c| c.conversation == run.conversation && c.generation == run.generation)
            {
                current.workspace_ready = true;
            }
            self.schedule_changes(worker, run.conversation, run.generation);
        }
        self.resolve_queue_unknown(worker);
    }

    fn has_runs(&self, worker: &Worker) -> bool {
        self.current.as_ref().is_some_and(|c| {
            worker
                .runs
                .values()
                .any(|r| r.conversation == c.conversation)
                || worker
                    .queue_unknown
                    .iter()
                    .any(|u| u.conversation == c.conversation)
        })
    }

    /// One short call to the open session's `History` service.
    fn history_page(
        &self,
        target: &RpcTarget,
        before: Option<u64>,
        limit: u32,
    ) -> Result<(Vec<Value>, bool), String> {
        let args = vec![json!({ "before": before, "limit": limit })];
        match self.call(target, "pi.history", "page", args) {
            Ok(Some(page)) => {
                let entries = page
                    .get("entries")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let more = page.get("more").and_then(Value::as_bool).unwrap_or(false);
                Ok((entries, more))
            }
            Ok(None) => Err("the engine gave no history".into()),
            Err(error) => Err(error.to_string()),
        }
    }

    /// Whether the conversation has entries older than the oldest the view holds. An engine
    /// without a history service has none to offer.
    fn has_older(&self, target: &RpcTarget, view: &Value) -> bool {
        let Some(oldest) = transcript::oldest_entry_id(view) else {
            return false;
        };
        self.history_page(target, Some(oldest), 1)
            .is_ok_and(|(entries, _)| !entries.is_empty())
    }

    /// The page of history just older than `before` (the oldest item shown).
    fn load_older(
        &mut self,
        worker: &mut Worker,
        conversation: ConversationId,
        generation: u64,
        before: Option<ItemId>,
    ) {
        let fail = |worker: &Worker, message: String| {
            worker.emit(conversation, generation, EventKind::OlderFailed { message })
        };
        let Some(target) = self.target_for(conversation) else {
            return fail(worker, "The session is not open on the Pi server.".into());
        };
        let Some(entry) = before.and_then(transcript::entry_of_item) else {
            return worker.emit(
                conversation,
                generation,
                EventKind::OlderPage {
                    items: vec![],
                    has_older: false,
                },
            );
        };
        match self.history_page(&target, Some(entry), HISTORY_PAGE) {
            Ok((entries, more)) => {
                if let Some(current) = self.current.as_mut().filter(|current| {
                    current.conversation == conversation && current.generation == generation
                }) {
                    for entry in &entries {
                        if let Some(id) = entry.get("id").and_then(Value::as_u64) {
                            current.change_entries.insert(id, entry.clone());
                        }
                    }
                }
                let mapped = transcript::map_history_page(&entries);
                worker.emit(
                    conversation,
                    generation,
                    EventKind::OlderPage {
                        items: mapped.items,
                        has_older: more,
                    },
                );
                self.schedule_changes(worker, conversation, generation);
            }
            Err(message) => fail(
                worker,
                format!("Could not load earlier messages: {message}"),
            ),
        }
    }

    /// The complete result of a tool call: from what the session holds now, or, for a call from
    /// before a compaction, from its history (a bounded number of pages back).
    fn fetch_tool_output(
        &mut self,
        worker: &mut Worker,
        conversation: ConversationId,
        generation: u64,
        call_id: String,
    ) {
        let unavailable = |worker: &Worker, reason: String| {
            worker.emit(
                conversation,
                generation,
                EventKind::ToolOutputUnavailable {
                    call_id: call_id.clone(),
                    reason,
                },
            )
        };
        let Some(current) = self
            .current
            .as_ref()
            .filter(|c| c.conversation == conversation)
        else {
            return unavailable(worker, "The session is not open on the Pi server.".into());
        };
        let target = current.target.clone();
        let view = current
            .transcript
            .read(|r| r.state("state").cloned())
            .flatten();
        if let Some(text) = view
            .as_ref()
            .and_then(|v| transcript::tool_result_in_view(v, &call_id))
        {
            return worker.emit(
                conversation,
                generation,
                EventKind::ToolOutputFull { call_id, text },
            );
        }
        let mut before = view.as_ref().and_then(transcript::oldest_entry_id);
        for _ in 0..TOOL_OUTPUT_PAGES {
            let Some(oldest) = before else { break };
            match self.history_page(&target, Some(oldest), 200) {
                Ok((entries, more)) => {
                    if let Some(text) = transcript::tool_result_text(&entries, &call_id) {
                        return worker.emit(
                            conversation,
                            generation,
                            EventKind::ToolOutputFull { call_id, text },
                        );
                    }
                    before = entries.last().and_then(|e| e.get("id")?.as_u64());
                    if !more {
                        break;
                    }
                }
                Err(error) => return unavailable(worker, error),
            }
        }
        unavailable(
            worker,
            "the engine does not have this result any more".into(),
        )
    }

    fn ui_respond(
        &mut self,
        worker: &mut Worker,
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
        let Some(target) = self.target_for(conversation) else {
            return refuse(worker, "The session is not open on the Pi server.".into());
        };
        let value = match answer {
            UiAnswer::Choice(v) | UiAnswer::Text(v) => json!(v),
            UiAnswer::Confirm(b) => json!(b),
        };
        match self.call(&target, "pi.ui-requests", "respond", vec![json!(id), value]) {
            Ok(Some(reply)) if reply.get("accepted").and_then(Value::as_bool) == Some(true) => {}
            Ok(Some(reply)) => refuse(
                worker,
                reply
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("The engine did not take the answer.")
                    .to_owned(),
            ),
            Ok(None) => refuse(worker, "The engine gave no answer; try again.".into()),
            Err(error) => refuse(worker, format!("Could not send the answer: {error}")),
        }
    }

    fn ui_cancel(&mut self, worker: &mut Worker, conversation: ConversationId, id: String) {
        let Some(target) = self.target_for(conversation) else {
            return worker.notice("Open the conversation first, then decline the question.".into());
        };
        if let Err(error) = self.call(&target, "pi.ui-requests", "cancel", vec![json!(id)]) {
            worker.notice(format!("Could not decline the question: {error}"));
        }
    }

    fn set_model(&mut self, worker: &mut Worker, conversation: ConversationId, model: &str) {
        let Some(target) = self.target_for(conversation) else {
            return worker.notice("Open the conversation first, then choose a model.".into());
        };
        let Some((provider, model_id)) = session::split_model_id(model) else {
            return worker.notice(format!("Unrecognized model id: {model}"));
        };
        let args = vec![json!({ "provider": provider, "modelId": model_id })];
        // The selection changes only when the engine's replicated state says so.
        if let Err(error) = self.call(&target, "pi.models", "select", args) {
            worker.notice(format!("Could not switch the model: {error}"));
        }
    }

    fn set_thinking_level(
        &mut self,
        worker: &mut Worker,
        conversation: ConversationId,
        level: &str,
    ) {
        let Some(target) = self.target_for(conversation) else {
            return worker.notice("Open the conversation first, then choose effort.".into());
        };
        // The replicated configuration, not this call, confirms the new selection.
        if let Err(error) = self.call(&target, "pi.models", "selectThinking", vec![json!(level)]) {
            worker.notice(format!("Could not change effort: {error}"));
        }
    }

    fn sync_thinking(
        &self,
        worker: &Worker,
        conversation: ConversationId,
        generation: u64,
        target: &RpcTarget,
        state: &Value,
    ) {
        let Some(level) = session::selected_thinking(state) else {
            return;
        };
        match self.call(target, "pi.models", "getThinkingLevels", vec![]) {
            Ok(Some(value)) => {
                let levels = value
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                worker.emit(
                    conversation,
                    generation,
                    EventKind::ThinkingState { level, levels },
                );
            }
            Ok(None) => {}
            Err(error) => log::warn!("could not read thinking levels: {error}"),
        }
    }

    fn create_conversation(&mut self, worker: &mut Worker, cwd: &str, request: &RequestId) {
        // The request key doubles as the session id, so repeating a lost creation is harmless:
        // Pi refuses a duplicate id and that refusal is treated as success.
        let args = vec![json!({ "id": request.0, "cwd": cwd })];
        match self.call(&self.server(), "pi.session-management", "create", args) {
            Ok(_) => {}
            Err(Error::Server { message, .. }) if message.contains("already exists") => {}
            Err(error) => worker.notice(format!("Could not create the conversation: {error}")),
        }
    }

    /// Collect uncommitted Git changes scoped to the selected session's file mutations,
    /// off this thread. A clean result retires committed evidence as the session's new base.
    fn schedule_changes(&mut self, worker: &Worker, conversation: ConversationId, generation: u64) {
        let Some(current) = self
            .current
            .as_ref()
            .filter(|c| c.conversation == conversation && c.generation == generation)
        else {
            return;
        };
        if !current.workspace_ready {
            return;
        }
        let entries: Vec<_> = current.change_entries.values().cloned().collect();
        let scanned_through = current
            .change_entries
            .keys()
            .next_back()
            .copied()
            .unwrap_or(0);
        let after = self.changes_bases.get(&conversation).copied().unwrap_or(0);
        let cwd = current.cwd.clone();
        if self.scanning {
            self.rescan = Some((conversation, generation));
            return;
        }
        self.scanning = true;
        let (conversation, generation) = (current.conversation, current.generation);
        if let Some(current) = self.current.as_mut() {
            current.next_changes_scan = Instant::now() + CHANGES_POLL;
        }
        let tx = worker.tx.clone();
        thread::spawn(move || {
            let workspace = match cwd {
                Some(cwd) => session_changes::collect(Path::new(&cwd), &entries, after),
                None if session_changes::paths(&entries, after).is_empty() => Workspace::Changes {
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
        worker: &Worker,
        conversation: ConversationId,
        generation: u64,
        scanned_through: u64,
        workspace: Workspace,
    ) {
        self.scanning = false;
        // Only the conversation that is still open may show the result.
        let had_files = self.current.as_ref().is_some_and(|current| {
            matches!(&current.last_workspace, Some(Workspace::Changes { files, .. }) if !files.is_empty())
        });
        let changed = self
            .current
            .as_mut()
            .filter(|c| c.conversation == conversation && c.generation == generation)
            .is_some_and(|current| {
                if matches!(&workspace, Workspace::Changes { files, truncated: false } if files.is_empty()) {
                    let base = self.changes_bases.entry(conversation).or_default();
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
                        .is_some_and(|current| std::mem::take(&mut current.publish_empty_changes));
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

    /// Attach a session, subscribe to its services and show its state.
    fn open(
        &mut self,
        worker: &mut Worker,
        conversation: ConversationId,
        generation: u64,
        initial: bool,
    ) {
        let fail = |worker: &Worker, message: String| {
            if initial {
                worker.emit(conversation, generation, EventKind::OpenFailed { message });
            } else {
                log::warn!("could not refresh {conversation:?} after reconnecting: {message}");
            }
        };
        let Some(session) = self.index.get(&conversation).cloned() else {
            return fail(
                worker,
                "This session no longer exists on the Pi server.".into(),
            );
        };
        let session_id = session.session_id.clone();
        if let Some(previous) = &self.current
            && previous.conversation != conversation
        {
            for (op, run) in &worker.runs {
                if run.conversation == previous.conversation && !worker.watchers.contains_key(op) {
                    let stop = Arc::new(AtomicBool::new(false));
                    worker.watchers.insert(*op, stop.clone());
                    let tx = worker.tx.clone();
                    let route = self.route.clone();
                    let session = previous.session_id.clone();
                    let op = *op;
                    let request = run.request.clone();
                    thread::spawn(move || watch_background(tx, route, session, op, request, stop));
                }
            }
        }
        // Drop the previous attachment's subscriptions first; the client retires them anyway.
        self.current = None;
        // A deferred scan belongs to the attachment that requested it, not the new one.
        self.rescan = None;
        let attach = ServiceCall::new("pi.session-management", "attach", vec![json!(session_id)]);
        // Pi's server can answer `internal_error` when a session is re-attached right after a
        // client switched away from it (observed against the real server: the session's worker
        // is still retiring). Attaching only navigates, it changes no session data, so a short
        // bounded retry is safe. Every other failure is final.
        let mut attempt = 0;
        loop {
            let result = self
                .client
                .request(&self.server(), &attach)
                .and_then(|pending| pending.wait_timeout(CALL_TIMEOUT));
            match result {
                Ok(_) => break,
                Err(Error::Server { ref code, .. })
                    if code == "internal_error" && attempt < ATTACH_RETRY_DELAYS.len() =>
                {
                    log::warn!(
                        "attach of {session_id} failed with internal_error; retrying (attempt {})",
                        attempt + 1
                    );
                    thread::sleep(ATTACH_RETRY_DELAYS[attempt]);
                    attempt += 1;
                }
                Err(error) => {
                    return fail(worker, format!("Could not attach the session: {error}"));
                }
            }
        }
        // The route arrives out of band, before or after the response.
        let deadline = Instant::now() + ATTACH_TIMEOUT;
        let target = loop {
            if let Some(a) = self.client.attachment()
                && a.session_id == session_id
            {
                break a.rpc();
            }
            if Instant::now() >= deadline {
                return fail(
                    worker,
                    "The Pi server did not publish the session attachment.".into(),
                );
            }
            thread::sleep(Duration::from_millis(10));
        };
        let transcript =
            match self
                .client
                .subscribe(&target, "pi.transcript", Mode::Singleton, CALL_TIMEOUT)
            {
                Ok(sub) => sub,
                Err(error) => {
                    return fail(worker, format!("Could not read the session: {error}"));
                }
            };
        let models = self
            .client
            .subscribe(&target, "pi.models", Mode::Singleton, CALL_TIMEOUT)
            .ok();
        // Optional: an engine without extension questions simply has none to show.
        let ui = self
            .client
            .subscribe(&target, "pi.ui-requests", Mode::Singleton, CALL_TIMEOUT)
            .ok();
        // Older/external engines are supported: unavailable is not an empty directory.
        let subagents = self
            .client
            .subscribe(&target, "pi.subagents", Mode::Singleton, CALL_TIMEOUT)
            .ok();
        let Some(view) = transcript.read(|r| r.state("state").cloned()).flatten() else {
            return fail(worker, "The session has no transcript.".into());
        };
        let has_older = self.has_older(&target, &view);
        let oldest_entry = transcript::oldest_entry_id(&view);
        let mapped = transcript::map_view(&view);
        let (tools_done, busy) = (tools_done(&mapped), mapped.busy);
        let workspace_ready = tools_done > 0 || has_older;
        let queue = mapped.queue.clone();
        let kind = if initial {
            EventKind::Opened {
                items: mapped.items,
                has_older,
                changes: vec![],
            }
        } else {
            EventKind::Synced {
                items: mapped.items,
            }
        };
        worker.emit(conversation, generation, kind);
        if let Some(usage) = transcript::usage(&view, &session_id) {
            worker.emit(conversation, generation, EventKind::UsageSynced(usage));
        }
        worker.emit(
            conversation,
            generation,
            EventKind::EngineState { busy, queue },
        );
        if let Some(models) = &models {
            let state = models.read(|r| r.state("state").cloned()).flatten();
            self.models = state
                .as_ref()
                .map(session::parse_models)
                .unwrap_or_default();
            (worker.sink)(LifecycleEvent::Catalog(self.catalog()));
            (worker.sink)(LifecycleEvent::ModelSelected(
                state.as_ref().and_then(session::selected_model),
            ));
            if let Some(state) = &state {
                self.sync_thinking(worker, conversation, generation, &target, state);
            }
        }
        self.resubscribes = 0;
        if let Some(ui) = &ui
            && let Some(state) = ui.read(|r| r.state("state").cloned()).flatten()
        {
            let (requests, status, notices) = session::parse_ui_state(&state);
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
        let mut change_entries = std::collections::BTreeMap::new();
        if has_older {
            let mut before = oldest_entry;
            // Bound history retrieval for session file scope. Normal transcript paging
            // remains independent and can reveal additional file paths.
            for _ in 0..8 {
                let Ok((entries, more)) = self.history_page(&target, before, HISTORY_PAGE) else {
                    break;
                };
                let next = entries
                    .iter()
                    .filter_map(|entry| entry.get("id").and_then(Value::as_u64))
                    .min();
                for entry in entries {
                    if let Some(id) = entry.get("id").and_then(Value::as_u64) {
                        change_entries.insert(id, entry);
                    }
                }
                if !more {
                    break;
                }
                if next.is_none() || next == before {
                    break;
                }
                before = next;
            }
        }
        for entry in view
            .get("entries")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(id) = entry.get("id").and_then(Value::as_u64) {
                change_entries.insert(id, entry.clone());
            }
        }
        self.current = Some(Current {
            conversation,
            session_id,
            generation,
            target,
            transcript,
            models,
            ui,
            subagents,
            child: None,
            cwd: session.cwd,
            tools_done,
            busy,
            workspace_ready,
            last_workspace: None,
            publish_empty_changes: false,
            next_changes_scan: Instant::now(),
            change_entries,
            oldest_entry,
        });
        self.refresh_subagents(worker);
        self.schedule_changes(worker, conversation, generation);
        // Prompts accepted earlier may have finished while this conversation was not attached.
        self.poll_runs(worker);
    }

    /// Reconcile the engine-owned session policy and durable directory, including terminal children.
    fn refresh_subagents(&self, worker: &Worker) {
        let Some(current) = &self.current else { return };
        let (conversation, generation) = (current.conversation, current.generation);
        let Some(service) = &current.subagents else {
            // An older engine has no child capability. The core defaults to unavailable;
            // injecting an extra attach event would perturb ordinary transcript ordering.
            return;
        };
        let enabled = service
            .read(|r| r.state("configuration").and_then(subagents::parse_enabled))
            .flatten();
        let result = self.call(&current.target, "pi.subagents", "list", vec![]);
        match (enabled, result) {
            (Some(enabled), Ok(Some(value))) => match subagents::parse_directory(&value) {
                Ok(children) => worker.emit(
                    conversation,
                    generation,
                    EventKind::SubagentsSynced { enabled, children },
                ),
                Err(reason) => worker.emit(
                    conversation,
                    generation,
                    EventKind::SubagentsUnavailable(reason),
                ),
            },
            _ => worker.emit(
                conversation,
                generation,
                EventKind::SubagentsUnavailable(
                    "Could not read this session's subagent state. Reconnect to retry.".into(),
                ),
            ),
        }
    }

    fn open_subagent(
        &mut self,
        worker: &Worker,
        conversation: ConversationId,
        generation: u64,
        child: u64,
    ) {
        let Some(current) = self.current.as_ref().filter(|c| {
            c.conversation == conversation && c.generation == generation && c.subagents.is_some()
        }) else {
            worker.emit(
                conversation,
                generation,
                EventKind::SubagentViewFailed {
                    child,
                    reason: "The session is no longer attached or does not support subagents."
                        .into(),
                },
            );
            return;
        };
        let target = current.target.clone();
        // Retire the previous child's subscription before admitting another child view.
        if let Some(current) = self.current.as_mut() {
            current.child = None;
        }
        let live = self
            .call(&target, "pi.subagents", "open", vec![json!(child)])
            .is_ok();
        if live
            && let Ok(subscription) =
                self.client
                    .subscribe(&target, "pi.subagent-transcript", Mode::Keyed, CALL_TIMEOUT)
        {
            let view = subscription
                .read(|r| {
                    r.instances()
                        .into_iter()
                        .flatten()
                        .find(|address| address.key == child.to_string())
                        .and_then(|address| r.instance_state(&Some(address), "state").cloned())
                })
                .flatten();
            if let Some(view) = view {
                worker.emit(
                    conversation,
                    generation,
                    EventKind::SubagentView {
                        child,
                        items: transcript::map_view(&view).items,
                        live: true,
                    },
                );
                if let Some(current) = self.current.as_mut() {
                    current.child = Some((child, subscription));
                }
                return;
            }
        }
        // A mount limit or older worker can still serve a read-only snapshot. Never call this live.
        match self.call(&target, "pi.subagents", "view", vec![json!(child)]) {
            Ok(Some(view)) => worker.emit(
                conversation,
                generation,
                EventKind::SubagentView {
                    child,
                    items: transcript::map_view(&view).items,
                    live: false,
                },
            ),
            _ => worker.emit(
                conversation,
                generation,
                EventKind::SubagentViewFailed {
                    child,
                    reason: "Could not read the subagent transcript. Try selecting it again."
                        .into(),
                },
            ),
        }
    }

    fn handle_event(
        &mut self,
        worker: &mut Worker,
        event: ClientEvent,
        dirty: &mut Dirty,
    ) -> Option<Outcome> {
        match event {
            ClientEvent::SubscriptionChanged { subscription, .. } => {
                if subscription == self.dir.id() {
                    dirty.directory = true;
                } else if self.auth.as_ref().is_some_and(|a| a.id() == subscription) {
                    dirty.auth = true;
                } else if let Some(current) = &self.current {
                    // Anything not from the live subscriptions is from an attachment we left.
                    if subscription == current.transcript.id() {
                        dirty.transcript = true;
                    } else if current
                        .models
                        .as_ref()
                        .is_some_and(|m| m.id() == subscription)
                    {
                        dirty.models = true;
                    } else if current.ui.as_ref().is_some_and(|u| u.id() == subscription) {
                        dirty.ui = true;
                    } else if current
                        .subagents
                        .as_ref()
                        .is_some_and(|s| s.id() == subscription)
                    {
                        dirty.subagents = true;
                    } else if current
                        .child
                        .as_ref()
                        .is_some_and(|(_, s)| s.id() == subscription)
                    {
                        dirty.child = true;
                    }
                }
            }
            ClientEvent::SubscriptionFailed {
                subscription,
                error,
            } => {
                let is_current = self.current.as_ref().is_some_and(|c| {
                    c.transcript.id() == subscription
                        || c.models.as_ref().is_some_and(|m| m.id() == subscription)
                });
                if let Some(current) = self.current.as_mut()
                    && current
                        .child
                        .as_ref()
                        .is_some_and(|(_, s)| s.id() == subscription)
                {
                    let child = current.child.take().unwrap().0;
                    worker.emit(
                        current.conversation,
                        current.generation,
                        EventKind::SubagentViewFailed {
                            child,
                            reason: format!("The live subagent view disconnected: {error}"),
                        },
                    );
                } else if let Some(current) = self.current.as_mut()
                    && current
                        .subagents
                        .as_ref()
                        .is_some_and(|s| s.id() == subscription)
                {
                    current.subagents = None;
                    worker.emit(
                        current.conversation,
                        current.generation,
                        EventKind::SubagentsUnavailable(
                            "The subagent service disconnected. Reconnect to retry.".into(),
                        ),
                    );
                } else if self.auth.as_ref().is_some_and(|a| a.id() == subscription) {
                    self.auth = None;
                    (worker.sink)(LifecycleEvent::SignIn(SignInSnapshot::default()));
                } else if subscription == self.dir.id() || is_current {
                    log::warn!("subscription {subscription} failed: {error}");
                    self.resubscribes += 1;
                    if self.resubscribes > MAX_RESUBSCRIBES {
                        // Stop patching; a reconnect resynchronizes everything.
                        self.client.disconnect();
                        return Some(Outcome::Retry);
                    }
                    if subscription == self.dir.id() {
                        match self.client.subscribe(
                            &self.server(),
                            "pi.session-directory",
                            Mode::Singleton,
                            CALL_TIMEOUT,
                        ) {
                            Ok(dir) => {
                                self.dir = dir;
                                dirty.directory = true;
                            }
                            Err(_) => {
                                self.client.disconnect();
                                return Some(Outcome::Retry);
                            }
                        }
                    } else if let Some((conversation, generation)) = self
                        .current
                        .as_ref()
                        .map(|c| (c.conversation, c.generation))
                    {
                        self.open(worker, conversation, generation, false);
                    }
                }
            }
            ClientEvent::Attachment(_)
            | ClientEvent::SubscriptionRetired { .. }
            | ClientEvent::Disconnected(_) => {}
        }
        None
    }

    /// Turn the batch's changes into one refresh each.
    fn flush(&mut self, worker: &mut Worker, dirty: Dirty) {
        if dirty.auth {
            self.publish_auth(worker);
            let done = self
                .auth_state()
                .filter(|s| s.status == SignInStatus::Done)
                .and_then(|s| s.attempt);
            if let Some(attempt) = done.filter(|id| self.auth_done.as_ref() != Some(id)) {
                self.auth_done = Some(attempt);
                if let Some(conversation) = self.current.as_ref().map(|c| c.conversation) {
                    self.refresh_models(worker, conversation);
                }
            }
        }
        if dirty.directory {
            self.refresh_directory();
        }
        if dirty.subagents {
            self.refresh_subagents(worker);
        }
        if dirty.child
            && let Some(current) = &self.current
            && let Some((child, subscription)) = &current.child
        {
            let view = subscription
                .read(|r| {
                    r.instances()
                        .into_iter()
                        .flatten()
                        .find(|address| address.key == child.to_string())
                        .and_then(|address| r.instance_state(&Some(address), "state").cloned())
                })
                .flatten();
            if let Some(view) = view {
                worker.emit(
                    current.conversation,
                    current.generation,
                    EventKind::SubagentView {
                        child: *child,
                        items: transcript::map_view(&view).items,
                        live: true,
                    },
                );
            }
        }
        if dirty.models
            && let Some(current) = &self.current
            && let Some(models) = &current.models
            && let Some(state) = models.read(|r| r.state("state").cloned()).flatten()
        {
            self.models = session::parse_models(&state);
            (worker.sink)(LifecycleEvent::ModelSelected(session::selected_model(
                &state,
            )));
            self.sync_thinking(
                worker,
                current.conversation,
                current.generation,
                &current.target,
                &state,
            );
            // A provider the engine could not read (expired login, bad key) is said once.
            let warning = session::refresh_warning(&state);
            if warning != worker.last_refresh_warning {
                if let Some(text) = &warning {
                    worker.notice(text.clone());
                }
                worker.last_refresh_warning = warning;
            }
        }
        if dirty.directory || dirty.models {
            (worker.sink)(LifecycleEvent::Catalog(self.catalog()));
        }
        if dirty.ui
            && let Some(current) = &self.current
            && let Some(ui) = &current.ui
            && let Some(state) = ui.read(|r| r.state("state").cloned()).flatten()
        {
            let (requests, status, notices) = session::parse_ui_state(&state);
            worker.emit(
                current.conversation,
                current.generation,
                EventKind::UiState {
                    requests,
                    status,
                    notices,
                },
            );
        }
        if dirty.transcript {
            let view = self.current.as_ref().and_then(|c| {
                c.transcript
                    .read(|r| r.state("state").cloned())
                    .flatten()
                    .map(|v| (c.conversation, c.generation, c.session_id.clone(), v))
            });
            if let Some((conversation, generation, session_id, view)) = view {
                if let Some(current) = self.current.as_mut() {
                    for entry in view
                        .get("entries")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        if let Some(id) = entry.get("id").and_then(Value::as_u64) {
                            current.change_entries.insert(id, entry.clone());
                        }
                    }
                }
                // A compaction or reset moves where the live view starts; what lies before it
                // is then history to offer.
                let oldest = transcript::oldest_entry_id(&view);
                let moved = self
                    .current
                    .as_ref()
                    .is_some_and(|c| c.oldest_entry != oldest);
                if moved
                    && let Some(target) = self.current.as_mut().map(|c| {
                        c.oldest_entry = oldest;
                        c.target.clone()
                    })
                {
                    let has_older = self.has_older(&target, &view);
                    worker.emit(conversation, generation, EventKind::HasOlder(has_older));
                }
                let mapped = transcript::map_view(&view);
                let (done, busy) = (tools_done(&mapped), mapped.busy);
                let queue = mapped.queue.clone();
                worker.emit(
                    conversation,
                    generation,
                    EventKind::Synced {
                        items: mapped.items,
                    },
                );
                if let Some(usage) = transcript::usage(&view, &session_id) {
                    worker.emit(conversation, generation, EventKind::UsageSynced(usage));
                }
                worker.emit(
                    conversation,
                    generation,
                    EventKind::EngineState { busy, queue },
                );
                // Files change when a tool finishes or a run ends.
                let changed = self
                    .current
                    .as_mut()
                    .map(|c| {
                        let changed = c.tools_done != done || c.busy != busy;
                        c.workspace_ready |= c.tools_done != done || (c.busy && !busy);
                        c.tools_done = done;
                        c.busy = busy;
                        changed
                    })
                    .unwrap_or(false);
                if changed {
                    self.schedule_changes(worker, conversation, generation);
                }
            }
        }
    }
}

/// How many tool calls in the transcript have finished.
fn tools_done(mapped: &transcript::Mapped) -> usize {
    mapped
        .items
        .iter()
        .filter(|item| {
            matches!(&item.kind, pipkin_core::ItemKind::Tool(t) if t.status != pipkin_core::ToolStatus::Running)
        })
        .count()
}

fn display_path(path: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod real_pi;

#[cfg(test)]
mod testsupport;

#[cfg(test)]
mod e2e;
