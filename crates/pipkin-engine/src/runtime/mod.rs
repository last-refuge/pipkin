//! The engine runtime: one tokio runtime, the store, the catalog/auth layer, per-session
//! execution, and client connections.

mod compaction;
pub mod faults;
mod glue;
mod import;
pub mod models;
mod run;
mod session;
mod subagents;
mod views;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pipkin_engine_protocol::*;
use tokio::sync::{Semaphore, mpsc};

use crate::auth::Auth;
use crate::config::{EngineConfig, Settings};
use crate::host::Connection;
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
    pub http: reqwest::Client,
    pub epoch: u64,
    pub adapters: Registry,
    pub tools: Vec<Arc<dyn Tool>>,
    pub generations: Arc<Semaphore>,
    pub processes: Arc<Semaphore>,
    pub locks: Arc<MutationLocks>,
    seq: AtomicU64,
    clients: Mutex<HashMap<u64, std::sync::mpsc::Sender<ServerFrame>>>,
    sessions: Mutex<HashMap<String, Arc<SessionRt>>>,
    handle: tokio::runtime::Handle,
    me: std::sync::Weak<Shared>,
}

impl Shared {
    /// Publish a committed event to every connected client.
    pub fn publish(&self, event: Event) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let frame = ServerFrame::Event {
            epoch: self.epoch,
            seq,
            event,
        };
        let mut clients = self.clients.lock().unwrap_or_else(|e| e.into_inner());
        clients.retain(|_, tx| tx.send(frame.clone()).is_ok());
    }

    /// Send to one client only (volatile, client-scoped events like sign-in state).
    pub fn publish_to(&self, client: u64, event: Event) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let frame = ServerFrame::Event {
            epoch: self.epoch,
            seq,
            event,
        };
        if let Some(tx) = self
            .clients
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&client)
        {
            let _ = tx.send(frame);
        }
    }

    pub fn spawn<F>(&self, future: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.handle.spawn(future);
    }

    /// The runtime state of an existing session.
    pub fn session(&self, id: &str) -> Result<Arc<SessionRt>, ProtoError> {
        if let Some(s) = self
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
        {
            return Ok(s.clone());
        }
        if self.store.session(id)?.is_none() {
            return Err(ProtoError::new(
                ErrorCode::NotFound,
                format!("Session {id} does not exist"),
            ));
        }
        let weak = self.me.clone();
        let session = id.to_owned();
        Ok(self
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(id.to_owned())
            .or_insert_with(|| {
                let questions = crate::extensions::questions::QuestionHost::new(Some(Arc::new(
                    move |ui: UiView| {
                        if let Some(shared) = weak.upgrade() {
                            shared.publish(Event::Session {
                                session: session.clone(),
                                revision: 0,
                                change: SessionChange::UiChanged { ui },
                            });
                        }
                    },
                )));
                Arc::new(SessionRt::new(id, questions))
            })
            .clone())
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
    fn system_prompt(
        &self,
        sess: &SessionRt,
        row: &SessionRow,
        tools: &[ToolSpec],
        settings: &Settings,
    ) -> String {
        let names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        let key = format!(
            "{}\u{0}{}\u{0}{}",
            row.cwd,
            names.join(","),
            settings.skills.join(",")
        );
        let mut cache = sess.prompt.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((k, p)) = cache.as_ref()
            && *k == key
        {
            return p.clone();
        }
        let (prompt, warnings) =
            glue::system_prompt(&row.cwd, &self.config.config_dir, &names, &settings.skills);
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
        let epoch = store.next_epoch().map_err(|e| e.to_string())?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .max_blocking_threads(64)
            .thread_name("pipkin-engine")
            .enable_all()
            .build()
            .map_err(|e| format!("cannot start the engine runtime: {e}"))?;
        let auth = Arc::new(Auth::new(&config));
        let shared = Arc::new_cyclic(|me| Shared {
            me: me.clone(),
            generations: Arc::new(Semaphore::new(config.max_active_generations.max(1))),
            processes: Arc::new(Semaphore::new(config.max_tool_processes.max(1))),
            config,
            store: Arc::new(store),
            auth,
            http: crate::provider::http::client(),
            epoch,
            adapters,
            tools,
            locks: Arc::default(),
            seq: AtomicU64::new(0),
            clients: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
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
                let runs = shared.store.unsettled_runs().unwrap_or_default();
                let mut seen = std::collections::HashSet::new();
                for run in runs {
                    if seen.insert(run.session.clone())
                        && let Err(e) = shared.recover_session(&run.session)
                    {
                        log::error!("cannot recover session {}: {e}", run.session);
                    }
                }
            });
        }
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
    pub fn connect(&self, _name: &str) -> Connection {
        let client = self.next_client.fetch_add(1, Ordering::SeqCst);
        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<ClientFrame>();
        let (out_tx, out_rx) = std::sync::mpsc::channel::<ServerFrame>();
        self.shared
            .clients
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(client, out_tx.clone());
        let shared = self.shared.clone();
        if let Some(rt) = self
            .runtime
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            rt.spawn(async move {
                while let Some(frame) = in_rx.recv().await {
                    let shared = shared.clone();
                    let out = out_tx.clone();
                    // Every call runs concurrently; per-session mutations serialize on the
                    // session lock, and Stop takes only that lock (never waits on a run).
                    tokio::spawn(async move {
                        let result = dispatch(&shared, client, frame.call).await;
                        let _ = out.send(ServerFrame::Reply {
                            id: frame.id,
                            result,
                        });
                    });
                }
                shared
                    .clients
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&client);
                shared.auth.client_gone(client);
            });
        }
        Connection {
            tx: in_tx,
            rx: out_rx,
            client,
        }
    }

    /// Orderly stop: extensions are deactivated (their processes ended) and questions
    /// cancelled; runs in flight stay unsettled in the store and resume on the next start.
    pub fn shutdown(&self, timeout: Duration) {
        // Pooled provider connections (Codex WebSockets) belong to this engine's lifetime.
        for adapter in self.shared.adapters.adapters() {
            adapter.close_sessions(None);
        }
        let sessions: Vec<Arc<SessionRt>> = self
            .shared
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
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
    match call {
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
                    "subagents".into(),
                    "questions".into(),
                    "compaction".into(),
                    "toolOutput".into(),
                ],
            }))
        }
        Call::ListSessions => Ok(Reply::Sessions {
            sessions: shared.summaries()?,
        }),
        Call::CreateSession { cwd, request } => shared.create_session(&cwd, &request),
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
        Call::OpenSession { session, window } => {
            shared.recover_session(&session)?;
            shared.ensure_extensions(&session).await;
            Ok(Reply::Snapshot(Box::new(shared.snapshot(
                &session,
                window.unwrap_or(limits::DEFAULT_WINDOW_ENTRIES),
            )?)))
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
        Call::CancelQueued { session, entry } => shared.cancel_queued(&session, entry).await,
        Call::Stop { session } => shared.stop(&session).await,
        Call::SetModel { session, model } => shared.set_model(&session, &model),
        Call::SetThinking { session, level } => shared.set_thinking(&session, &level),
        Call::Models { session } => {
            let catalog = shared.auth.catalog();
            let selected = match &session {
                Some(s) => shared.store.session(s)?.and_then(|r| r.model),
                None => None,
            };
            Ok(Reply::Models(models::models_view(
                &shared.auth,
                &catalog,
                selected.as_deref(),
            )))
        }
        Call::RefreshModels => {
            let shared = shared.clone();
            tokio::task::spawn_blocking(move || {
                shared.auth.reload();
                shared.publish(Event::ModelsChanged);
            })
            .await
            .map_err(|e| ProtoError::new(ErrorCode::Internal, e.to_string()))?;
            Ok(Reply::Ok)
        }
        Call::Compact {
            session,
            instructions,
        } => {
            let sess = shared.session(&session)?;
            let cancel = tokio_util::sync::CancellationToken::new();
            match shared
                .compact(&sess, None, instructions.as_deref(), &cancel)
                .await
            {
                Ok(compaction::Compacted::Done(_)) => Ok(Reply::Ok),
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
        Call::ImportDiscover { agent_dir } => import::discover(shared, agent_dir).await,
        Call::ImportSessions {
            agent_dir,
            sessions,
        } => import::sessions(shared, agent_dir, sessions).await,
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
        self.session(session)?;
        let catalog = self.auth.catalog();
        let model = catalog
            .find(key)
            .ok_or_else(|| ProtoError::new(ErrorCode::NotFound, format!("Unknown model: {key}")))?
            .clone();
        let row = self
            .store
            .session(session)?
            .ok_or_else(|| not_found("Session"))?;
        let current = row
            .thinking
            .as_deref()
            .and_then(crate::provider::ThinkingLevel::parse)
            .unwrap_or(models::DEFAULT_THINKING);
        let thinking = model.clamp_thinking(current);
        self.store
            .set_session_field(session, SessionField::Model(Some(model.key())))?;
        self.store.set_session_field(
            session,
            SessionField::Thinking(Some(thinking.as_str().into())),
        )?;
        // The reference also makes the selection the global default.
        if let Err(e) =
            Settings::save_default_model(&self.config.config_dir, &model.provider, &model.id)
        {
            log::warn!("could not save the default model: {e}");
        }
        self.model_and_thinking_changed(session);
        Ok(Reply::Ok)
    }

    fn set_thinking(&self, session: &str, level: &str) -> Result<Reply, ProtoError> {
        self.session(session)?;
        let row = self
            .store
            .session(session)?
            .ok_or_else(|| not_found("Session"))?;
        let catalog = self.auth.catalog();
        let model = row.model.as_deref().and_then(|k| catalog.find(k));
        let levels = model.map_or(vec![crate::provider::ThinkingLevel::Off], |m| {
            m.thinking_levels()
        });
        let parsed = crate::provider::ThinkingLevel::parse(level)
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
            })?;
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
    /// Start the configured extensions for a root session, once per engine process.
    async fn ensure_extensions(self: &Arc<Self>, session: &str) {
        if self.config.extensions.is_empty() {
            return;
        }
        let Ok(sess) = self.session(session) else {
            return;
        };
        if !matches!(self.store.session(session), Ok(Some(row)) if row.parent.is_none()) {
            return;
        }
        let mut slot = sess.extensions.lock().await;
        if slot.is_some() {
            return;
        }
        let weak = self.me.clone();
        let dispatch: crate::extensions::Dispatch = Arc::new(move |call: Call| {
            let weak = weak.clone();
            match weak.upgrade() {
                // Client 0: extensions act inside the engine, not as a connection.
                Some(shared) => dispatch_boxed(shared, 0, call),
                None => Box::pin(async {
                    Err(ProtoError::new(
                        ErrorCode::ShuttingDown,
                        "the engine is stopping",
                    ))
                }),
            }
        });
        let host = Arc::new(crate::extensions::CallHost::new(
            session.to_owned(),
            sess.questions.clone(),
            dispatch,
        ));
        let mut runner = crate::extensions::ExtensionRunner::new(host);
        for spec in &self.config.extensions {
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
mod tests;
