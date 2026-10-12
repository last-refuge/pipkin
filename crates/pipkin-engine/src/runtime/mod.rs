//! The engine runtime: one tokio runtime, the store, the catalog/auth layer, per-session
//! execution, and client connections.

mod compaction;
pub mod faults;
mod glue;
mod import;
mod lifecycle;
pub mod models;
mod plugins;
mod removal;
mod run;
mod session;
mod subagents;
mod views;
mod wait;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pipkin_engine_protocol::*;
use tokio::sync::{Semaphore, mpsc};

use crate::auth::Auth;
use crate::config::{EngineConfig, Settings};
use crate::host::Connection;
use crate::provider::http::NetSettings;
use crate::provider::{ApiAdapter, Registry, ToolSpec};
use crate::store::{SessionField, SessionRow, Store};
use crate::tools::{MutationLocks, Tool};
use crate::util::now_ms;

pub use session::{SessionRt, Settled};

pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Shared engine state.
pub struct Shared {
    pub config: EngineConfig,
    pub store: Arc<Store>,
    pub auth: Arc<Auth>,
    /// The engine-wide HTTP client and what it was built from (see [`Shared::refresh_net`]).
    net: Mutex<(NetSettings, reqwest::Client)>,
    pub epoch: u64,
    pub adapters: Registry,
    pub tools: Vec<Arc<dyn Tool>>,
    pub generations: Arc<Semaphore>,
    pub processes: Arc<Semaphore>,
    pub locks: Arc<MutationLocks>,
    publication: Mutex<()>,
    stopping: tokio_util::sync::CancellationToken,
    prompt_waits: Arc<Semaphore>,
    compaction_slots: Arc<Semaphore>,
    removal_slots: Arc<Semaphore>,
    compaction_recovery: tokio::sync::Notify,
    clients: Mutex<HashMap<u64, crate::delivery::Delivery>>,
    cursors: Mutex<HashMap<u64, (u64, u64)>>,
    sessions: Mutex<lifecycle::Sessions>,
    handle: tokio::runtime::Handle,
    me: std::sync::Weak<Shared>,
}

impl Shared {
    /// The engine-wide HTTP client (cheap to clone; shares the connection pool).
    pub fn http(&self) -> reqwest::Client {
        self.net.lock().unwrap_or_else(|e| e.into_inner()).1.clone()
    }

    /// Re-read proxy variables, the extra CA bundle and global network settings
    /// (`httpProxy`, `httpIdleTimeoutMs`); rebuild the client (and the sign-in client) when
    /// they changed. Called before each model request and on `RefreshModels`.
    pub fn refresh_net(&self) -> reqwest::Client {
        self.refresh_net_snapshot().1
    }

    pub fn refresh_net_snapshot(&self) -> (NetSettings, reqwest::Client) {
        let global = Settings::load(&self.config.config_dir, None).0;
        let wanted = NetSettings::new(&self.config.env, &global);
        let mut net = self.net.lock().unwrap_or_else(|e| e.into_inner());
        if net.0 != wanted {
            let client = crate::provider::http::client_with(&wanted);
            self.auth.set_http(client.clone());
            *net = (wanted, client);
        }
        net.clone()
    }

    /// The proxy configuration currently in effect.
    pub fn proxy_config(&self) -> crate::provider::proxy::ProxyConfig {
        self.net
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .0
            .proxy
            .clone()
    }

    /// Publish a committed event to every connected client.
    pub fn publish(&self, event: Event) {
        let mut clients = self.clients.lock().unwrap_or_else(|e| e.into_inner());
        if !clients.is_empty() {
            let event = pipkin_engine_protocol::delivery::SharedEvent::from(event);
            clients.retain(|_, tx| tx.event(self.epoch, event.clone()));
        }
    }

    /// Client-local state has its own sequence; another client's sign-in never creates gaps.
    pub fn publish_to(&self, client: u64, event: Event) {
        if let Some(tx) = self
            .clients
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&client)
        {
            tx.event(self.epoch, event);
        }
    }

    pub fn spawn<F>(&self, future: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.handle.spawn(future);
    }

    /// Tools offered to a session's model: the coding tools, plus `subagent` for root sessions
    /// that have it enabled (children never delegate further).
    pub fn tools_for(&self, row: &SessionRow) -> Vec<Arc<dyn Tool>> {
        let mut tools = self.tools.clone();
        if row.parent.is_none() && row.subagents_enabled {
            tools.push(self.subagent_tool());
        }
        tools
    }

    /// The system prompt, kept byte-identical between turns unless its inputs change.
    fn system_sections(
        &self,
        sess: &SessionRt,
        row: &SessionRow,
        tools: &[ToolSpec],
        settings: &Settings,
    ) -> serde_json::Map<String, serde_json::Value> {
        let names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        let key = format!("{}\u{0}{}", row.cwd, names.join(","));
        let mut cache = sess.prompt.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((k, p)) = cache.as_ref()
            && *k == key
        {
            return p.clone();
        }
        let (prompt, warnings) = sess
            .prompt_resources
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .system_sections(&row.cwd, &self.config.config_dir, &names, &settings.skills);
        for warning in warnings {
            log::warn!("session {}: {warning}", row.id);
        }
        *cache = Some((key, prompt.clone()));
        prompt
    }

    fn model_and_thinking_changed(&self, session: &str) {
        let Ok(Some(row)) = self.store.session(session) else {
            return;
        };
        let catalog = self.auth.catalog();
        let model = row.model.as_deref().and_then(|k| catalog.find(k));
        let thinking = models::thinking_view(model, row.thinking.as_deref());
        self.publish(Event::Session {
            session: session.to_owned(),
            revision: row.revision,
            change: SessionChange::ModelChanged {
                model: row.model.clone(),
            },
        });
        self.publish(Event::Session {
            session: session.to_owned(),
            revision: row.revision,
            change: SessionChange::ThinkingChanged { thinking },
        });
    }
}

pub struct EngineHandle {
    runtime: Mutex<Option<tokio::runtime::Runtime>>,
    shared: Arc<Shared>,
    next_client: AtomicU64,
}

impl EngineHandle {
    pub fn start(config: EngineConfig) -> Result<EngineHandle, String> {
        Self::start_with(config, Registry::builtin(), crate::tools::builtin_tools())
    }

    /// Start with explicit adapters and tools (tests add the scripted provider).
    pub fn start_with(
        config: EngineConfig,
        adapters: Registry,
        tools: Vec<Arc<dyn Tool>>,
    ) -> Result<EngineHandle, String> {
        let store = Store::open(&config.data_dir).map_err(|e| e.to_string())?;
        // No worker exists yet. Complete committed removals before recovering any work.
        for root in store.pending_removals().map_err(|e| e.to_string())? {
            // Reap tool processes orphaned by a crash before deleting their durable ids.
            for (session, call) in store
                .removal_tool_intents(&root)
                .map_err(|e| e.to_string())?
            {
                faults::kill_tool_processes(&faults::tool_call_token(&session, &call));
            }
            store.finish_removal(&root).map_err(|e| e.to_string())?;
        }
        crate::extensions::profiles::initialize(
            &store,
            &config.extensions,
            config.clear_extensions,
        )
        .map_err(|e| e.to_string())?;
        let epoch = store.next_epoch().map_err(|e| e.to_string())?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .max_blocking_threads(64)
            .thread_name("pipkin-engine")
            .enable_all()
            .build()
            .map_err(|e| format!("cannot start the engine runtime: {e}"))?;
        let auth = Arc::new(Auth::new(&config));
        let net = NetSettings::new(&config.env, &Settings::load(&config.config_dir, None).0);
        let http = crate::provider::http::client_with(&net);
        auth.set_http(http.clone());
        let shared = Arc::new_cyclic(|me| Shared {
            me: me.clone(),
            generations: Arc::new(Semaphore::new(config.max_active_generations.max(1))),
            processes: Arc::new(Semaphore::new(config.max_tool_processes.max(1))),
            config,
            store: Arc::new(store),
            auth,
            net: Mutex::new((net, http)),
            epoch,
            adapters,
            tools,
            locks: Arc::default(),
            cursors: Mutex::new(HashMap::new()),
            publication: Mutex::new(()),
            stopping: tokio_util::sync::CancellationToken::new(),
            prompt_waits: Arc::new(Semaphore::new(64)),
            compaction_slots: Arc::new(Semaphore::new(8)),
            removal_slots: Arc::new(Semaphore::new(8)),
            compaction_recovery: tokio::sync::Notify::new(),
            clients: Mutex::new(HashMap::new()),
            sessions: Mutex::new(lifecycle::Sessions::default()),
            handle: runtime.handle().clone(),
        });
        {
            let shared = shared.clone();
            shared.auth.set_listener(Arc::new({
                let shared = Arc::downgrade(&shared);
                move |client, snapshot| {
                    if let Some(s) = shared.upgrade() {
                        s.publish_to(client, Event::AuthChanged(snapshot));
                    }
                }
            }));
        }
        // Resume or settle runs left unsettled by a previous engine process.
        {
            let shared = shared.clone();
            runtime.spawn(async move {
                let sessions = shared.store.pending_sessions().unwrap_or_else(|error| {
                    log::error!("cannot enumerate pending work: {error}");
                    Vec::new()
                });
                for session in sessions {
                    if let Err(e) = shared.recover_session(&session) {
                        log::error!("cannot recover session {session}: {e}");
                        continue;
                    }
                    // A crash can fall after settlement but before the next queued run is
                    // started. Such a session has no open run and still needs scheduling.
                    if let Ok(sess) = shared.session(&session) {
                        shared.next_from_queue(&sess).await;
                    }
                }
            });
        }
        lifecycle::start_reaper(&shared);
        compaction::start_recovery(&shared);
        Ok(EngineHandle {
            runtime: Mutex::new(Some(runtime)),
            shared,
            next_client: AtomicU64::new(1),
        })
    }

    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    /// Install an extra wire adapter before use (tests).
    pub fn adapters(&self) -> &Registry {
        &self.shared.adapters
    }

    /// A new client connection.
    pub fn connect(&self, name: &str) -> Connection {
        self.connect_with_ownership(name, true)
    }

    /// An authenticated external host connection cannot request engine shutdown.
    pub fn connect_external(&self, name: &str) -> Connection {
        self.connect_with_ownership(name, false)
    }

    fn connect_with_ownership(&self, _name: &str, owns_engine: bool) -> Connection {
        let client = self.next_client.fetch_add(1, Ordering::SeqCst);
        let (in_tx, mut in_rx) = mpsc::channel::<ClientFrame>(64);
        let (control, mut controls) = mpsc::channel::<ClientFrame>(8);
        let (out_tx, out_rx) = crate::delivery::channel();
        self.shared
            .clients
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(client, out_tx.clone());
        let shared = self.shared.clone();
        let lifetime = tokio_util::sync::CancellationToken::new();
        let sender_lifetime = Arc::new(crate::host::ClientLifetime(lifetime.clone()));
        let requests = Arc::new(crate::host::requests::Requests::default());
        let pending_requests = requests.clone();
        if let Some(rt) = self
            .runtime
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            rt.spawn(async move {
                let mut tasks = tokio::task::JoinSet::new();
                let mut disconnected = false;
                let normal_slots = Arc::new(Semaphore::new(16));
                let control_slots = Arc::new(Semaphore::new(4));
                let wait_slots = Arc::new(Semaphore::new(64));
                loop {
                    // Reserve a task slot before dequeuing, keeping queued and active calls bounded.
                    // Control has independent capacity and is polled first.
                    let (frame, permit) = tokio::select! {
                        biased;
                        _ = lifetime.cancelled(), if !disconnected => {
                            disconnected = true;
                            shared.auth.client_gone(client);
                            out_tx.close();
                            continue;
                        }
                        Some(_) = tasks.join_next(), if !tasks.is_empty() => continue,
                        v = async {
                            let p = control_slots.clone().acquire_owned().await.ok()?;
                            Some((controls.recv().await?, p))
                        }, if !controls.is_closed() || !controls.is_empty() => match v { Some(v) => v, None => continue },
                        v = async {
                            let p = normal_slots.clone().acquire_owned().await.ok()?;
                            Some((in_rx.recv().await?, p))
                        }, if !in_rx.is_closed() || !in_rx.is_empty() => match v { Some(v) => v, None => continue },
                        else => break,
                    };
                    let request = pending_requests.guard(frame.id);
                    let pending = pending_requests.clone();
                    let shared = shared.clone();
                    let out = out_tx.clone();
                    let lifetime = lifetime.clone();
                    let wait_slots = wait_slots.clone();
                    tasks.spawn(async move {
                        // Waiters have their own bounded admission; they must not occupy all
                        // normal slots and prevent the prompt/queue actions they await.
                        let waiting = matches!(&frame.call, Call::WaitForPrompt { .. });
                        // Retain this connection's wait slot through reply delivery too, so
                        // a client that stops reading cannot accumulate completed wait tasks.
                        let _wait_permit = if waiting { wait_slots.try_acquire_owned().ok() } else { None };
                        let wait_full = waiting && _wait_permit.is_none();
                        let _permit = if _wait_permit.is_some() { drop(permit); None } else { Some(permit) };
                        let signing_in = matches!(&frame.call,
                            Call::AuthStart { .. } | Call::AuthReuse { .. } | Call::AuthAnswer { .. });
                        // Only drop futures whose suspension points are cancellation-safe.
                        // Blocking imports/refreshes must finish while retaining this slot;
                        // otherwise repeated cancellations could spawn unbounded detached work.
                        let interruptible = matches!(&frame.call,
                            Call::Prompt { .. } | Call::Enqueue { .. } | Call::CancelQueued { .. }
                            | Call::Stop { .. } | Call::WaitForPrompt { .. });
                        let result = tokio::select! {
                            biased;
                            _ = request.cancel.cancelled(), if interruptible => {
                                Err(ProtoError::new(ErrorCode::Cancelled, "the request was cancelled; committed work is unchanged"))
                            }
                            _ = lifetime.cancelled(), if signing_in || waiting => {
                                shared.auth.client_gone(client);
                                Err(ProtoError::new(ErrorCode::ShuttingDown, "the client disconnected"))
                            }
                            result = async {
                                if request.cancel.is_cancelled() {
                                    return Err(ProtoError::new(ErrorCode::Cancelled, "the request was cancelled"));
                                }
                                if wait_full {
                                    return Err(ProtoError::new(ErrorCode::Busy, "Too many prompt waits on this connection"));
                                }
                                if !owns_engine && matches!(frame.call, Call::Shutdown) {
                                    return Err(ProtoError::new(ErrorCode::Invalid, "Only the server owner can shut down an external engine"));
                                }
                                if let Call::CancelRequest { request } = frame.call {
                                    pending.cancel(request);
                                    return Ok(Reply::Ok);
                                }
                                let result = dispatch_with_cancel(&shared, client, frame.call, &request.cancel).await;
                                if request.cancel.is_cancelled() {
                                    Err(ProtoError::new(ErrorCode::Cancelled, "the request was cancelled; committed work is unchanged"))
                                } else {
                                    result
                                }
                            } => result,
                        };
                        drop(request);
                        out.reply(ServerFrame::Reply { id: frame.id, result }).await;
                    });
                }
                // Drain accepted calls before dropping their attachment demand. Replies no
                // longer hold capacity when a disconnected client stopped reading.
                out_tx.close();
                while tasks.join_next().await.is_some() {}
                shared.release_client(client);
                shared.clients.lock().unwrap_or_else(|e| e.into_inner()).remove(&client);
                shared.cursors.lock().unwrap_or_else(|e| e.into_inner()).remove(&client);
                shared.auth.client_gone(client);
                out_tx.close();
            });
        }
        Connection {
            tx: in_tx,
            control,
            rx: out_rx,
            client,
            lifetime: sender_lifetime,
            requests,
        }
    }

    /// Orderly stop: extensions are deactivated (their processes ended) and questions
    /// cancelled; runs in flight stay unsettled in the store and resume on the next start.
    pub fn shutdown(&self, timeout: Duration) {
        self.shared.stopping.cancel();
        self.shared.auth.cancel_model_refresh();
        for delivery in self
            .shared
            .clients
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
        {
            delivery.close();
        }
        // Pooled provider connections (Codex WebSockets) belong to this engine's lifetime.
        for adapter in self.shared.adapters.adapters() {
            adapter.close_sessions(None);
        }
        let sessions: Vec<Arc<SessionRt>> = self
            .shared
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .live
            .values()
            .map(|entry| entry.rt.clone())
            .collect();
        if let Some(rt) = self
            .runtime
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            let grace = timeout.min(Duration::from_secs(2));
            let done = rt.spawn(async move {
                for sess in sessions {
                    sess.questions.dispose();
                    if let Some(runner) = sess.extensions.lock().await.take() {
                        runner.shutdown(grace).await;
                    }
                }
            });
            let deadline = std::time::Instant::now() + grace + Duration::from_millis(500);
            while !done.is_finished() && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        if let Some(rt) = self
            .runtime
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            rt.shutdown_timeout(timeout);
        }
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        self.shutdown(Duration::from_secs(5));
    }
}

/// `dispatch` behind a concrete boxed type, so in-engine callers (extensions) can call back
/// into it without a recursive future type.
fn dispatch_boxed(
    shared: Arc<Shared>,
    client: u64,
    call: Call,
) -> crate::provider::BoxFuture<'static, Result<Reply, ProtoError>> {
    Box::pin(async move { dispatch(&shared, client, call).await })
}

fn not_found(what: &str) -> ProtoError {
    ProtoError::new(ErrorCode::NotFound, format!("{what} does not exist"))
}

async fn dispatch(shared: &Arc<Shared>, client: u64, call: Call) -> Result<Reply, ProtoError> {
    dispatch_with_cancel(
        shared,
        client,
        call,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
}

async fn dispatch_with_cancel(
    shared: &Arc<Shared>,
    client: u64,
    call: Call,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<Reply, ProtoError> {
    match call {
        Call::CancelRequest { .. } => Err(ProtoError::new(
            ErrorCode::Invalid,
            "Request cancellation requires a client connection",
        )),
        Call::Hello { protocol, .. } => {
            if protocol != PROTOCOL_VERSION {
                return Err(ProtoError::new(
                    ErrorCode::Incompatible,
                    format!("the engine speaks protocol {PROTOCOL_VERSION}, the client {protocol}"),
                ));
            }
            Ok(Reply::Welcome(Welcome {
                protocol: PROTOCOL_VERSION,
                engine_version: ENGINE_VERSION.into(),
                engine_revision: option_env!("PIPKIN_REVISION").unwrap_or("unknown").into(),
                schema: crate::store::SCHEMA_VERSION,
                epoch: shared.epoch,
                capabilities: vec![
                    "durableInvalidations".into(),
                    "sessionAttachments".into(),
                    "extensionProfiles".into(),
                    "waitForPrompt".into(),
                    "cancelRequest".into(),
                    "cycleThinking".into(),
                    "removeSession".into(),
                    "subagents".into(),
                    "questions".into(),
                    "compaction".into(),
                    "toolOutput".into(),
                ],
            }))
        }
        Call::CatchUp { after, limit } => {
            let page = shared.store.catch_up(after, limit)?;
            let mut cursors = shared.cursors.lock().unwrap_or_else(|e| e.into_inner());
            let cursor = cursors.entry(client).or_default();
            cursor.1 = cursor.1.max(page.through);
            Ok(Reply::Events(page))
        }
        Call::AckEvents { through } => {
            let mut cursors = shared.cursors.lock().unwrap_or_else(|e| e.into_inner());
            let cursor = cursors.entry(client).or_default();
            if through > cursor.1 || through < cursor.0 {
                return Err(ProtoError::new(
                    ErrorCode::Invalid,
                    "Cursor was not delivered or moves backwards",
                ));
            }
            cursor.0 = through;
            Ok(Reply::Ok)
        }
        Call::PrepareExtensions {
            session,
            extensions,
        } => shared.prepare_extensions(session, extensions),
        Call::ListSessions => Ok(Reply::Sessions {
            sessions: shared.summaries()?,
        }),
        Call::CreateSession { cwd, request } => shared.create_session(&cwd, &request),
        Call::RemoveSession { session } => shared.remove_session(&session).await,
        Call::RenameSession { session, title } => {
            shared.session(&session)?;
            let title = title.trim().to_owned();
            shared.store.set_session_field(
                &session,
                SessionField::Title((!title.is_empty()).then_some(title)),
            )?;
            shared.publish(Event::SessionsChanged);
            Ok(Reply::Ok)
        }
        Call::ReleaseSession { session } => {
            shared.release_session(client, &session);
            Ok(Reply::Ok)
        }
        Call::OpenSession { session, window } => {
            let _demand = shared.session(&session)?;
            shared.recover_session(&session)?;
            shared.compaction_recovery.notify_one();
            shared.ensure_extensions(&session).await;
            let snapshot =
                shared.snapshot(&session, window.unwrap_or(limits::DEFAULT_WINDOW_ENTRIES))?;
            // Attach only after a successful open; a refused snapshot must not leak demand.
            shared.attach_session(client, &session)?;
            Ok(Reply::Snapshot(Box::new(snapshot)))
        }
        Call::HistoryPage {
            session,
            before,
            limit,
        } => {
            shared.session(&session)?;
            let (entries, has_older) =
                shared
                    .store
                    .page(&session, before, limit.clamp(1, limits::MAX_PAGE_ENTRIES))?;
            Ok(Reply::Page { entries, has_older })
        }
        Call::Prompt {
            session,
            request,
            text,
            images,
            ..
        } => shared.prompt(&session, &request, &text, &images).await,
        Call::Enqueue {
            session,
            request,
            mode,
            text,
            images,
        } => {
            shared
                .enqueue(&session, &request, mode, &text, &images)
                .await
        }
        Call::Lookup { session, request } => {
            shared.session(&session)?;
            Ok(Reply::Lookup(
                match shared.store.request(&session, &request)? {
                    Some(row) => row.lookup(),
                    None => LookupResult::Unknown,
                },
            ))
        }
        Call::WaitForPrompt { session, request } => shared
            .wait_for_prompt(&session, &request)
            .await
            .map(Reply::PromptResult),
        Call::CancelQueued { session, entry } => shared.cancel_queued(&session, entry).await,
        Call::Stop { session } => shared.stop(&session).await,
        Call::SetModel { session, model } => shared.set_model(&session, &model),
        Call::SetThinking { session, level } => shared.set_thinking(&session, Some(&level)),
        Call::CycleThinking { session } => shared.set_thinking(&session, None),
        Call::Models { session } => {
            let catalog = shared.auth.catalog();
            let row = match &session {
                Some(s) => shared.store.session(s)?,
                None => None,
            };
            let settings = Settings::load(
                &shared.config.config_dir,
                row.as_ref().map(|r| std::path::Path::new(r.cwd.as_str())),
            )
            .0;
            let selected = row.and_then(|r| r.model);
            Ok(Reply::Models(models::models_view(
                &shared.auth,
                &catalog,
                &settings,
                selected.as_deref(),
            )))
        }
        Call::RefreshModels => {
            let refresh = shared.clone();
            tokio::task::spawn_blocking(move || {
                // Network settings (proxy, idle timeout) are re-read too.
                refresh.refresh_net();
            })
            .await
            .map_err(|e| ProtoError::new(ErrorCode::Internal, e.to_string()))?;
            shared.auth.refresh_models(cancel).await;
            shared.publish(Event::ModelsChanged);
            Ok(Reply::Ok)
        }
        Call::Compact {
            session,
            instructions,
        } => {
            let sess = shared.session(&session)?;
            match shared
                .compact_manual(&sess, instructions.as_deref(), cancel)
                .await
            {
                Ok(compaction::Compacted::Done(_) | compaction::Compacted::Submitted(_)) => {
                    Ok(Reply::Ok)
                }
                Ok(compaction::Compacted::Nothing(why)) => {
                    Err(ProtoError::new(ErrorCode::Invalid, why))
                }
                Err(e) => Err(ProtoError::new(ErrorCode::Internal, e)),
            }
        }
        Call::ToolOutput { session, call_id } => {
            shared.session(&session)?;
            match shared.store.tool_output(&session, &call_id)? {
                Some(view) => Ok(Reply::ToolOutput(view)),
                None => Err(not_found("That tool output")),
            }
        }
        Call::AnswerQuestion {
            session,
            question,
            answer,
        } => {
            let sess = shared.session(&session)?;
            sess.questions
                .respond(&question, answer)
                .map_err(|reason| ProtoError::new(ErrorCode::Invalid, reason))?;
            Ok(Reply::Ok)
        }
        Call::CancelQuestion { session, question } => {
            let sess = shared.session(&session)?;
            if sess.questions.cancel(&question) {
                Ok(Reply::Ok)
            } else {
                Err(ProtoError::new(
                    ErrorCode::NotFound,
                    "That question is no longer open.",
                ))
            }
        }
        Call::DismissNotices { session } => {
            shared.session(&session)?.questions.dismiss_notices();
            Ok(Reply::Ok)
        }
        Call::Subagents { session } => Ok(Reply::Subagents(shared.subagents_view(&session)?)),
        Call::SubagentHistory {
            session,
            child,
            before,
            limit,
        } => shared.subagent_history(&session, child, before, limit),
        Call::SetSubagentsEnabled { session, enabled } => {
            shared.session(&session)?;
            shared
                .store
                .set_session_field(&session, SessionField::SubagentsEnabled(enabled))?;
            shared.publish_subagents(&session);
            Ok(Reply::Ok)
        }
        Call::AuthState => Ok(Reply::Auth(shared.auth.snapshot(client))),
        Call::AuthStart { provider } => {
            shared.auth.start(client, &provider).await?;
            Ok(Reply::Auth(shared.auth.snapshot(client)))
        }
        Call::AuthReuse { provider } => {
            shared.auth.reuse(client, &provider).await?;
            Ok(Reply::Auth(shared.auth.snapshot(client)))
        }
        Call::AuthAnswer {
            attempt,
            challenge,
            response,
        } => {
            shared
                .auth
                .answer(client, &attempt, &challenge, response.0)?;
            Ok(Reply::Ok)
        }
        Call::AuthCancel { attempt } => {
            shared.auth.cancel(client, &attempt);
            Ok(Reply::Ok)
        }
        Call::AuthRemove { provider } => {
            shared.auth.remove(&provider)?;
            shared.auth.reload();
            shared.publish(Event::ModelsChanged);
            Ok(Reply::Ok)
        }
        Call::SetApiKey { provider, key } => {
            shared.auth.set_api_key(&provider, key.0)?;
            shared.auth.reload();
            shared.publish(Event::ModelsChanged);
            Ok(Reply::Ok)
        }
        Call::ImportDiscover {
            server_dir,
            agent_dir,
        } => import::discover(shared, agent_dir, server_dir).await,
        Call::ImportSessions {
            server_dir,
            agent_dir,
            sessions,
        } => import::sessions(shared, agent_dir, server_dir, sessions).await,
        Call::ImportConfig { agent_dir, items } => import::config(shared, agent_dir, items).await,
        Call::ImportCredentials {
            agent_dir,
            providers,
        } => import::credentials(shared, agent_dir, providers).await,
        Call::Shutdown => Ok(Reply::Ok),
    }
}

impl Shared {
    fn summaries(&self) -> Result<Vec<SessionSummary>, ProtoError> {
        let catalog = self.auth.catalog();
        Ok(self
            .store
            .root_sessions()?
            .iter()
            .map(|row| {
                let mut summary = row.summary();
                if summary.blocked.is_none()
                    && let Some(key) = &row.model
                    && let Some(model) = catalog.find(key)
                    && let Err(crate::auth::AuthError::Unavailable(why)) =
                        self.auth.availability(model)
                {
                    summary.blocked = Some(why);
                }
                summary
            })
            .collect())
    }

    fn create_session(&self, cwd: &str, request: &str) -> Result<Reply, ProtoError> {
        let path = std::path::Path::new(cwd);
        if !path.is_absolute() {
            return Err(ProtoError::new(
                ErrorCode::Invalid,
                "Session cwd must be an absolute path",
            ));
        }
        let real = std::fs::canonicalize(path)
            .ok()
            .filter(|p| p.is_dir())
            .ok_or_else(|| {
                ProtoError::new(
                    ErrorCode::Invalid,
                    "Session cwd is not an existing directory",
                )
            })?;
        let settings = Settings::load(&self.config.config_dir, Some(&real)).0;
        let catalog = self.auth.catalog();
        let (model, thinking) = models::initial(
            &self.auth,
            &catalog,
            &settings,
            self.config.default_model.as_deref(),
        );
        let id = crate::util::uuid_v4();
        let (row, created) = self.store.create_session(
            &id,
            &real.display().to_string(),
            Some(request),
            &Origin::Native,
            model.as_deref(),
            Some(thinking.as_str()),
            None,
            now_ms(),
        )?;
        if created {
            self.publish(Event::SessionsChanged);
        }
        Ok(Reply::Session {
            summary: row.summary(),
        })
    }

    fn set_model(&self, session: &str, key: &str) -> Result<Reply, ProtoError> {
        let _session = self.session(session)?;
        let catalog = self.auth.catalog();
        let model = catalog
            .find(key)
            .ok_or_else(|| ProtoError::new(ErrorCode::NotFound, format!("Unknown model: {key}")))?
            .clone();
        let settings = self.settings_for(session);
        // Model selection, explicit thinking and cycling share one configuration order.
        let publication = self.publication.lock().unwrap_or_else(|e| e.into_inner());
        let row = self
            .store
            .session(session)?
            .ok_or_else(|| not_found("Session"))?;
        let current = row
            .thinking
            .as_deref()
            .and_then(crate::provider::ThinkingLevel::parse)
            .unwrap_or(models::DEFAULT_THINKING);
        // Reference `_getThinkingLevelForModelSwitch`: the model's own default, then the
        // global default, then the session's current level.
        let thinking = models::thinking_for(&settings, &model, None, current);
        self.store.set_session_field(
            session,
            SessionField::ModelAndThinking {
                model: model.key(),
                thinking: thinking.as_str().into(),
            },
        )?;
        self.model_and_thinking_changed(session);
        drop(publication);
        // The reference also makes the selection the global default, and adds it to a
        // non-empty `enabledModels` scope it is not already in.
        if let Err(e) =
            Settings::save_default_model(&self.config.config_dir, &model.provider, &model.id)
        {
            log::warn!("could not save the default model: {e}");
        }
        if let Some(scope) = models::scope(&self.auth, &catalog, &settings)
            && !scope.models.is_empty()
            && !scope.models.iter().any(|(m, _)| m.key() == model.key())
            && let Some(patterns) = &settings.enabled_models
            && !patterns
                .iter()
                .any(|p| p.to_lowercase() == model.key().to_lowercase())
        {
            let mut patterns = patterns.clone();
            patterns.push(model.key());
            if let Err(e) = Settings::save_enabled_models(&self.config.config_dir, &patterns) {
                log::warn!("could not save the enabled models: {e}");
            }
        }
        Ok(Reply::Ok)
    }

    fn set_thinking(&self, session: &str, level: Option<&str>) -> Result<Reply, ProtoError> {
        let _session = self.session(session)?;
        let _publication = self.publication.lock().unwrap_or_else(|e| e.into_inner());
        let row = self
            .store
            .session(session)?
            .ok_or_else(|| not_found("Session"))?;
        let catalog = self.auth.catalog();
        let model = row.model.as_deref().and_then(|k| catalog.find(k));
        let levels = model.map_or(vec![crate::provider::ThinkingLevel::Off], |m| {
            m.thinking_levels()
        });
        let parsed = if let Some(level) = level {
            crate::provider::ThinkingLevel::parse(level)
                .filter(|l| levels.contains(l))
                .ok_or_else(|| {
                    ProtoError::new(
                        ErrorCode::Invalid,
                        format!(
                            "Thinking level {level} is unavailable; choose one of: {}",
                            levels
                                .iter()
                                .map(|l| l.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    )
                })?
        } else {
            let next = levels
                .iter()
                .position(|l| l.as_str() == row.thinking.as_deref().unwrap_or("off"))
                .map_or(0, |i| (i + 1) % levels.len());
            levels
                .get(next)
                .copied()
                .unwrap_or(crate::provider::ThinkingLevel::Off)
        };
        self.store.set_session_field(
            session,
            SessionField::Thinking(Some(parsed.as_str().into())),
        )?;
        self.model_and_thinking_changed(session);
        Ok(Reply::Thinking(models::thinking_view(
            model,
            Some(parsed.as_str()),
        )))
    }
}

impl Shared {
    /// Start configured extensions once per active session lifetime.
    async fn ensure_extensions(self: &Arc<Self>, session: &str) {
        let Ok(sess) = self.session(session) else {
            return;
        };
        if !matches!(self.store.session(session), Ok(Some(row)) if row.parent.is_none()) {
            return;
        }
        // A new lifetime must not activate extensions until the old generation exited.
        if let Some(previous) = &sess.predecessor {
            previous.cancelled().await;
        }
        let mut slot = sess.extensions.lock().await;
        if slot.is_some() {
            return;
        }
        let dispatch = self.extension_dispatch(&sess);
        let host = Arc::new(crate::extensions::CallHost::new(
            session.to_owned(),
            sess.questions.clone(),
            dispatch,
        ));
        let mut runner = crate::extensions::ExtensionRunner::new(host);
        for spec in &sess.extension_profile {
            let extension: Arc<dyn crate::extensions::Extension> = match spec {
                crate::config::ExtensionSpec::Builtin(name) if name == "questions" => {
                    Arc::new(crate::extensions::examples::QuestionsExample)
                }
                crate::config::ExtensionSpec::Builtin(_) => {
                    Arc::new(crate::extensions::examples::GreetingExample::new())
                }
                crate::config::ExtensionSpec::Process { id, argv } => {
                    Arc::new(crate::extensions::process::ProcessExtension::new(
                        crate::extensions::process::ProcessConfig::new(id.clone(), argv.clone()),
                    ))
                }
            };
            if let Err(e) = runner.start(extension) {
                log::warn!("session {session}: {e}");
            }
        }
        *slot = Some(runner);
    }
}

/// An adapter registry with the scripted provider added (tests and the headless harness).
pub fn registry_with(extra: Arc<dyn ApiAdapter>) -> Registry {
    let mut r = Registry::builtin();
    r.install(extra);
    r
}

#[cfg(test)]
mod fault_tests;
#[cfg(test)]
mod settings_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod cancel_tests;
#[cfg(test)]
mod compaction_tests;

#[cfg(test)]
mod subagent_accounting_tests;
