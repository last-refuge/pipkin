//! The application controller: wires storage, the demo backend and the GPUI `Model`.
//!
//! Effects emitted by the core are executed here, never in render. Backend events arrive on an
//! async channel and are applied on the foreground in coalesced batches.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{App, AppContext as _, ClipboardItem, Entity};
use pipkin_core::*;
use pipkin_ui::{DemoControls, Model};

use crate::adapters::native::supervisor::{HostKind, HostSpec, WorkerSpec};
use crate::adapters::native::{NativeBackend, NativeConfig};
use crate::adapters::pi::engine::EngineConfig;
use crate::adapters::pi::{PiBackend, PiConfig};
use crate::adapters::script;
use crate::adapters::{DemoBackend, DemoOptions};
use crate::launch::{self, LaunchConfig};
use crate::platform;
use crate::storage::{
    CacheMeta, DEMO_NAMESPACE, DemoConversation, Loaded, OpenRequest, Startup, Storage, StoredDraft,
};

const CLOCK_REFRESH: Duration = Duration::from_secs(15);
const EVENT_QUEUE: usize = 1024;
const MAX_BATCH: usize = 512;

#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    /// What to do: run the application, or print a report and exit.
    pub action: Action,
    pub mode: Mode,
    pub scenario: String,
    pub data_dir: Option<PathBuf>,
    pub speed: f32,
    /// Real mode: the Pi server profile directory (default `PI_SERVER_DIR` or `~/.pi/server`).
    pub pi_dir: Option<PathBuf>,
    /// Real mode: connect to this logical server instead of discovering one.
    pub pi_server_id: Option<String>,
    /// Real mode: a Pi checkout to launch and own an engine from (the development entry point).
    /// Without it, an engine must already be running.
    pub pi_repo: Option<PathBuf>,
    /// Real mode: the agent directory (credentials, models, sessions) for a launched engine.
    pub pi_agent_dir: Option<PathBuf>,
    /// Real mode: project folders to add at startup (repeatable); the last one is selected.
    pub projects: Vec<PathBuf>,
    /// Real mode: plugin packages the launched engine loads (repeatable).
    pub pi_extensions: Vec<PathBuf>,
    /// The editor command for "open in editor" (otherwise `$VISUAL`, `$EDITOR`, the desktop's).
    pub editor: Option<String>,
    /// The terminal command for "open terminal" (otherwise `$TERMINAL`, the desktop's).
    pub terminal: Option<String>,
    /// Real mode: which engine backs the app. Pi unless `--engine native` is given.
    pub engine: EngineKind,
    /// `--engine native`: where the built-in engine runs (`--engine-host`).
    pub engine_host: HostKind,
}

/// The engine behind real mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EngineKind {
    #[default]
    Pi,
    /// Pipkin's built-in engine (`pipkin-engine`).
    Native,
}

/// What a launch is for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Action {
    #[default]
    Run,
    /// `--version`: print the version and exit.
    Version,
    /// `--diagnose [--probe]`: print an installation report and exit; `probe` also starts a
    /// throwaway engine to check it answers.
    Diagnose { probe: bool },
}

impl Default for Options {
    fn default() -> Self {
        Options {
            action: Action::Run,
            mode: Mode::Real,
            scenario: "normal".into(),
            data_dir: None,
            speed: 1.0,
            pi_dir: None,
            pi_server_id: None,
            pi_repo: None,
            pi_agent_dir: None,
            projects: vec![],
            pi_extensions: vec![],
            editor: None,
            terminal: None,
            engine: EngineKind::Pi,
            engine_host: HostKind::Child,
        }
    }
}

/// A path made absolute against the current directory. The engine is started from the home
/// directory, so a relative `--pi-repo ../pi-fork/pi` would point somewhere else there.
fn absolute(path: String) -> Result<PathBuf, String> {
    std::path::absolute(&path).map_err(|e| format!("{path}: {e}"))
}

impl Options {
    pub fn from_env_args() -> Result<Options, String> {
        Options::parse(std::env::args().skip(1))
    }

    /// Real mode is the default; `--demo <scenario>` selects the simulated backend.
    /// Parse `--demo <scenario>`, `--data-dir <path>` and `--speed <factor>`
    /// (each also accepted as `--flag=value`).
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Options, String> {
        let mut options = Options::default();
        let mut engine_host = None;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let (flag, inline) = match arg.split_once('=') {
                Some((f, v)) => (f.to_string(), Some(v.to_string())),
                None => (arg, None),
            };
            let mut value = |name: &str| {
                inline
                    .clone()
                    .or_else(|| args.next())
                    .ok_or_else(|| format!("{name} needs a value"))
            };
            match flag.as_str() {
                "--demo" => {
                    let name = value("--demo")?;
                    if !script::scenario_names().contains(&name.as_str()) {
                        return Err(format!(
                            "unknown scenario {name:?}; choose one of: {}",
                            script::scenario_names().join(", ")
                        ));
                    }
                    options.scenario = name;
                    options.mode = Mode::Demo;
                }
                "--version" | "-V" => options.action = Action::Version,
                "--diagnose" => {
                    options.action = Action::Diagnose {
                        probe: matches!(options.action, Action::Diagnose { probe: true }),
                    }
                }
                "--probe" => options.action = Action::Diagnose { probe: true },
                "--data-dir" => options.data_dir = Some(absolute(value("--data-dir")?)?),
                "--pi-dir" => options.pi_dir = Some(absolute(value("--pi-dir")?)?),
                "--pi-repo" => options.pi_repo = Some(absolute(value("--pi-repo")?)?),
                "--project" => options.projects.push(PathBuf::from(value("--project")?)),
                "--pi-extension" => {
                    let path = PathBuf::from(value("--pi-extension")?);
                    options.pi_extensions.push(
                        std::fs::canonicalize(&path)
                            .map_err(|e| format!("--pi-extension {}: {e}", path.display()))?,
                    );
                }
                "--engine" => {
                    options.engine = match value("--engine")?.as_str() {
                        "pi" => EngineKind::Pi,
                        "native" => EngineKind::Native,
                        other => {
                            return Err(format!("unknown engine {other:?}; choose pi or native"));
                        }
                    }
                }
                "--engine-host" => {
                    engine_host = Some(match value("--engine-host")?.as_str() {
                        "child" => HostKind::Child,
                        "inprocess" => HostKind::InProcess,
                        other => {
                            return Err(format!(
                                "unknown engine host {other:?}; choose child or inprocess"
                            ));
                        }
                    })
                }
                "--editor" => options.editor = Some(value("--editor")?),
                "--terminal" => options.terminal = Some(value("--terminal")?),
                "--pi-agent-dir" => {
                    options.pi_agent_dir = Some(absolute(value("--pi-agent-dir")?)?)
                }
                "--pi-server-id" => {
                    let id = value("--pi-server-id")?;
                    if !pi_client::protocol::is_server_id(&id) {
                        return Err(format!(
                            "--pi-server-id must be a lowercase UUIDv4, got {id:?}"
                        ));
                    }
                    options.pi_server_id = Some(id);
                }
                "--speed" => {
                    let raw = value("--speed")?;
                    let speed: f32 = raw
                        .parse()
                        .map_err(|_| format!("invalid --speed {raw:?}"))?;
                    if !speed.is_finite() || speed < 0.0 {
                        return Err(format!("--speed must be a non-negative number, got {raw}"));
                    }
                    options.speed = speed;
                }
                other => return Err(format!("unknown argument {other:?}")),
            }
        }
        if options.engine == EngineKind::Native {
            if options.mode == Mode::Demo {
                return Err("--engine native cannot be combined with --demo".into());
            }
            if options.pi_dir.is_some()
                || options.pi_server_id.is_some()
                || options.pi_repo.is_some()
                || options.pi_agent_dir.is_some()
                || !options.pi_extensions.is_empty()
            {
                return Err(
                    "the --pi-* options apply to the Pi engine, not --engine native".into(),
                );
            }
        }
        if let Some(host) = engine_host {
            if options.engine != EngineKind::Native {
                return Err("--engine-host applies to --engine native".into());
            }
            options.engine_host = host;
        }
        Ok(options)
    }
}

/// Entropy for request IDs so they stay unique across restarts. The core takes it as data.
fn request_prefix(now: i64) -> String {
    use std::hash::{BuildHasher, Hasher};
    let random = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    format!("{now:x}{random:016x}")
}

fn unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Merge consecutive token deltas for the same operation so one paint covers a whole burst.
fn coalesce(batch: Vec<BackendEvent>) -> Vec<BackendEvent> {
    let mut out: Vec<BackendEvent> = Vec::with_capacity(batch.len());
    for event in batch {
        if let (EventKind::Token(next), Some(last)) = (&event.kind, out.last_mut())
            && let EventKind::Token(prev) = &mut last.kind
            && last.conversation == event.conversation
            && last.generation == event.generation
            && last.op == event.op
        {
            prev.push_str(next);
            continue;
        }
        out.push(event);
    }
    out
}

/// The Pi server profile directory this run talks to.
fn profile_dir(options: &Options) -> PathBuf {
    options
        .pi_dir
        .clone()
        .unwrap_or_else(crate::adapters::pi::default_directory)
}

/// Desktop data is namespaced by backend so identities from one never apply to another. A real
/// backend is identified by its profile directory, so two engines never share drafts, journals
/// or selections.
fn namespace(options: &Options) -> String {
    match options.mode {
        Mode::Demo => DEMO_NAMESPACE.to_owned(),
        Mode::Real if options.engine == EngineKind::Native => format!(
            "native:{:x}",
            stable_id(
                &platform::engine_dirs(options.data_dir.as_deref())
                    .data
                    .display()
                    .to_string()
            )
        ),
        Mode::Real => format!(
            "pi:{:x}",
            stable_id(&profile_dir(options).display().to_string())
        ),
    }
}

/// The engine directory to launch and own: `--pi-repo`, else, when no running server was named
/// (`--pi-dir`, `--pi-server-id`), the bundled engine of an installation.
fn engine_to_launch(options: &Options) -> Option<PathBuf> {
    if let Some(repo) = &options.pi_repo {
        return Some(repo.clone());
    }
    if options.pi_dir.is_some() || options.pi_server_id.is_some() {
        return None;
    }
    let candidates = crate::install::engine_search_dirs(
        std::env::current_exe().ok().as_deref(),
        std::env::var_os(crate::install::ENGINE_DIR_ENV).map(PathBuf::from),
    );
    match crate::install::find_engine(&candidates) {
        Ok((dir, manifest)) => {
            log::info!(
                "using the bundled engine {} at {}",
                manifest.version,
                dir.display()
            );
            Some(dir)
        }
        Err(reasons) => {
            log::info!(
                "no bundled engine ({}); looking for a running server",
                reasons.join("; ")
            );
            None
        }
    }
}

/// The engine to launch and own, for `--pi-repo`. Its server identity is the one given, else the
/// profile's `default-server-id` (the identity Pi itself would use), else a fresh one that is
/// remembered there so the next launch is the same logical server.
fn managed_engine(options: &Options, pi_repo: &Path) -> Result<EngineConfig, String> {
    let server_dir = profile_dir(options);
    let id_file = server_dir.join("default-server-id");
    let server_id = match &options.pi_server_id {
        Some(id) => id.clone(),
        None => match std::fs::read_to_string(&id_file) {
            Ok(text) if pi_client::protocol::is_server_id(text.trim()) => text.trim().to_owned(),
            _ => {
                let id = platform::fresh_server_id()
                    .map_err(|e| format!("cannot create a server id: {e}"))?;
                std::fs::create_dir_all(&server_dir)
                    .and_then(|_| std::fs::write(&id_file, &id))
                    .map_err(|e| {
                        format!(
                            "cannot remember the server id in {}: {e}",
                            id_file.display()
                        )
                    })?;
                id
            }
        },
    };
    let cwd = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| PathBuf::from("/"));
    Ok(EngineConfig {
        pi_repo: pi_repo.to_path_buf(),
        log_path: platform::data_dir(options.data_dir.as_deref()).join("engine.log"),
        server_dir,
        server_id,
        agent_dir: options.pi_agent_dir.clone(),
        cwd,
        model: None,
        env: vec![],
        extensions: options.pi_extensions.clone(),
    })
}

/// Open the desktop database, and say when it is not the one the user expects: a damaged file
/// that was set aside, or no usable database at all. In the second case Pipkin keeps working
/// against a throwaway database so a prompt can still be journaled, and says plainly that
/// nothing survives the session.
fn open_storage(options: &Options) -> (Arc<Storage>, Option<String>) {
    let dir = platform::data_dir(options.data_dir.as_deref());
    let ns = namespace(options);
    match Storage::open_recovering(&platform::db_path(&dir), &ns) {
        Ok((storage, Startup::Clean)) => (Arc::new(storage), None),
        Ok((storage, Startup::Recovered(message))) => {
            log::error!("{message}");
            (Arc::new(storage), Some(message))
        }
        Err(e) => {
            let fallback =
                std::env::temp_dir().join(format!("pipkin-fallback-{}", std::process::id()));
            log::error!(
                "cannot open storage in {}: {e}; using {}",
                dir.display(),
                fallback.display()
            );
            let storage = Storage::open(&platform::db_path(&fallback), &ns)
                .expect("a throwaway database in the temporary directory");
            let message = format!(
                "Pipkin cannot use its saved data ({e}). Your drafts and unfinished sends will \
                 not survive closing the app, and anything saved earlier is not shown. Copy \
                 anything you need to keep."
            );
            (Arc::new(storage), Some(message))
        }
    }
}

/// Stored state waiting for its conversation to appear in a catalog. Real sessions come from
/// the engine, possibly after startup or a reconnect, so each catalog restores whatever now has
/// a home and keeps the rest for later.
pub(crate) struct Restore {
    /// Demo-only conversations kept in the desktop database.
    conversations: Vec<DemoConversation>,
    drafts: Vec<StoredDraft>,
    requests: Vec<OpenRequest>,
    prompt_histories: Vec<(ConversationId, Vec<String>)>,
    goals: Vec<(ConversationId, String, Option<String>)>,
}

impl Restore {
    pub(crate) fn new(loaded: Loaded, include_demo_conversations: bool) -> Restore {
        Restore {
            conversations: if include_demo_conversations {
                loaded.conversations
            } else {
                vec![]
            },
            drafts: loaded.drafts,
            requests: loaded.open_requests,
            prompt_histories: loaded.prompt_histories,
            goals: loaded.goals,
        }
    }

    fn restore_conversations(&mut self, state: &mut AppState) {
        for c in self.conversations.drain(..) {
            state.restore_conversation(c.id, c.project, c.title, c.updated_at);
        }
        // Keep histories for real sessions whose catalog has not arrived yet.
        self.prompt_histories.retain(|(id, prompts)| {
            if state.conversation(*id).is_none() {
                return true;
            }
            state.restore_prompt_history(*id, prompts.clone());
            false
        });
    }

    pub(crate) fn apply(&mut self, state: &mut AppState) -> Outcome {
        let mut out = Outcome::default();
        self.restore_conversations(state);
        self.drafts.retain(|d| {
            if state.conversation(d.conversation).is_none() {
                return true;
            }
            // The files may have moved or changed since the draft was saved.
            let attachments = d.attachments.iter().map(revalidate_attachment).collect();
            state.restore_draft(d.conversation, d.text.clone(), attachments);
            false
        });
        self.goals.retain(|(id, text, paused)| {
            if state.conversation(*id).is_none() {
                return true;
            }
            state.restore_goal(*id, text.clone(), paused.clone());
            false
        });
        // Never resend: an unresolved request becomes "outcome unknown" for the user to resolve.
        self.requests.retain(|r| {
            if state.conversation(r.conversation).is_none() {
                return true;
            }
            out.merge(state.restore_unresolved(
                r.conversation,
                r.request.clone(),
                r.text.clone(),
                r.attachments.clone(),
            ));
            false
        });
        out
    }
}

/// Keeps each conversation's saved copy current without writing on every streamed token: a copy
/// is written once the conversation has been quiet for a moment, and once more on quit.
struct CacheSaver {
    storage: Arc<Storage>,
    dirty: std::cell::RefCell<std::collections::HashSet<ConversationId>>,
    scheduled: std::cell::Cell<bool>,
}

/// How long a conversation is quiet before its copy is written.
const CACHE_QUIET: Duration = Duration::from_millis(1500);

impl CacheSaver {
    fn new(storage: Arc<Storage>) -> Self {
        CacheSaver {
            storage,
            dirty: Default::default(),
            scheduled: Default::default(),
        }
    }

    fn request(self: &Rc<Self>, conversation: ConversationId, cx: &mut gpui::Context<Model>) {
        self.dirty.borrow_mut().insert(conversation);
        if self.scheduled.replace(true) {
            return;
        }
        let this = self.clone();
        cx.spawn(async move |model, cx| {
            cx.background_executor().timer(CACHE_QUIET).await;
            this.scheduled.set(false);
            this.write_dirty(&model, cx);
        })
        .detach();
    }

    fn write_dirty(&self, model: &gpui::WeakEntity<Model>, cx: &mut impl gpui::AppContext) {
        let ids: Vec<ConversationId> = self.dirty.borrow_mut().drain().collect();
        for id in ids {
            let copy = model
                .read_with(cx, |m, _| {
                    let c = m.state.conversation(id)?;
                    let project_path = m
                        .state
                        .projects
                        .iter()
                        .find(|p| p.id == c.project)?
                        .path
                        .clone();
                    c.opened.then(|| {
                        let from = c.items.len().saturating_sub(crate::cache::MAX_ITEMS);
                        (
                            c.items[from..].to_vec(),
                            c.has_older,
                            m.state.now(),
                            CacheMeta {
                                title: c.title.clone(),
                                project_path,
                                updated_at: c.updated_at,
                            },
                        )
                    })
                })
                .ok()
                .flatten();
            if let Some((items, has_older, now, meta)) = copy {
                self.storage.save_cache(id, meta, &items, has_older, now);
            }
        }
    }

    fn flush_now(&self, model: &gpui::WeakEntity<Model>, cx: &mut App) {
        self.write_dirty(model, cx);
    }
}

/// Searches saved history as the search box changes, dropping a search that a newer one replaced.
struct SearchRunner {
    storage: Arc<Storage>,
    latest: std::cell::Cell<u64>,
}

/// How long typing pauses before the saved history is searched.
const SEARCH_QUIET: Duration = Duration::from_millis(150);

impl SearchRunner {
    fn new(storage: Arc<Storage>) -> Self {
        SearchRunner {
            storage,
            latest: Default::default(),
        }
    }

    fn request(self: &Rc<Self>, query: String, cx: &mut gpui::Context<Model>) {
        let seq = self.latest.get() + 1;
        self.latest.set(seq);
        let this = self.clone();
        cx.spawn(async move |model, cx| {
            cx.background_executor().timer(SEARCH_QUIET).await;
            if this.latest.get() != seq {
                return;
            }
            let storage = this.storage.clone();
            let q = query.clone();
            let found = cx
                .background_executor()
                .spawn(async move { storage.search(&q, crate::storage::SEARCH_LIMIT) })
                .await;
            let results = match found {
                Ok(results) => results,
                Err(e) => {
                    log::warn!("search failed: {e}");
                    return;
                }
            };
            if this.latest.get() == seq {
                model
                    .update(cx, |m, cx| {
                        m.mutate(|s| s.apply_search_results(results), cx)
                    })
                    .ok();
            }
        })
        .detach();
    }
}

pub fn start(cx: &mut App, options: Options) -> Entity<Model> {
    let (storage, mut storage_issue) = open_storage(&options);
    let loaded = storage.load_all().unwrap_or_else(|e| {
        log::error!("cannot read stored state: {e}");
        storage_issue.get_or_insert(format!(
            "Pipkin could not read its saved data ({e}), so earlier drafts and unfinished \
             sends are not shown."
        ));
        crate::storage::Loaded {
            prefs: Prefs::default(),
            drafts: vec![],
            conversations: vec![],
            open_requests: vec![],
            prompt_histories: vec![],
            goals: vec![],
            projects: vec![],
        }
    });

    let now = unix_secs();
    let (event_tx, event_rx) = async_channel::bounded::<BackendEvent>(EVENT_QUEUE);
    let demo = (options.mode == Mode::Demo).then(|| {
        Arc::new(DemoBackend::new(
            event_tx.clone(),
            DemoOptions {
                speed: options.speed,
                scenario: options.scenario.clone(),
                // Fixtures are relative to the start of today (UTC), so recency reads naturally.
                base_time: now - now % 86_400,
                ..DemoOptions::default()
            },
        ))
    });
    let launch_source = (options.mode == Mode::Real && options.engine == EngineKind::Pi)
        .then(|| engine_to_launch(&options))
        .flatten();
    let support_report = crate::diagnostics::prepare(options.mode, launch_source.as_deref());
    let backend: Arc<dyn Backend> = match &demo {
        Some(demo) => demo.clone(),
        None if options.engine == EngineKind::Native => {
            let dirs = platform::engine_dirs(options.data_dir.as_deref());
            let log_path = crate::adapters::native::engine_log_path(&platform::data_dir(
                options.data_dir.as_deref(),
            ));
            let host = match options.engine_host {
                HostKind::Child => match WorkerSpec::engine(&dirs.data, &dirs.config, &log_path) {
                    Ok(spec) => HostSpec::Child(spec),
                    Err(error) => {
                        eprintln!("pipkin: {error}");
                        std::process::exit(2);
                    }
                },
                HostKind::InProcess => HostSpec::InProcess {
                    data_dir: dirs.data.clone(),
                    config_dir: dirs.config.clone(),
                },
            };
            Arc::new(NativeBackend::new(
                event_tx,
                NativeConfig::new(host, dirs.data.display().to_string()),
            ))
        }
        None => {
            let config = match launch_source {
                Some(repo) => match managed_engine(&options, &repo) {
                    Ok(engine) => PiConfig::managed(engine),
                    Err(error) => {
                        // Say why the engine cannot be launched; do not quietly fall back.
                        eprintln!("pipkin: cannot manage a Pi engine: {error}");
                        std::process::exit(2);
                    }
                },
                None => {
                    let mut config = PiConfig::new(profile_dir(&options));
                    config.server_id = options.pi_server_id.clone();
                    config
                }
            };
            Arc::new(PiBackend::new(event_tx, config))
        }
    };

    // State starts empty and `Connecting`; the backend's catalog arrives asynchronously.
    let mut state = AppState::new(
        Bootstrap {
            projects: vec![],
            models: vec![],
            conversations: vec![],
            now,
        },
        loaded.prefs.clone(),
    );
    state.mode = options.mode;
    if let Some(issue) = storage_issue {
        state.set_storage_issue(issue);
    }
    state.set_now(now);
    state.set_request_prefix(request_prefix(now));
    state.set_connection(Connection::Connecting);
    if options.mode == Mode::Real {
        state.can_create = true;
        // Folders the user opened are known before the engine answers, so the project list
        // is usable while connecting.
        for path in &loaded.projects {
            state.restore_project(path);
        }
        // Folders named on the command line: resolved like the folder picker's, remembered, and
        // the last one is where new conversations go.
        for path in &options.projects {
            match std::fs::canonicalize(path) {
                Ok(real) if real.is_dir() => {
                    let real = real.display().to_string();
                    state.restore_project(&real);
                    storage.save_project(real.clone());
                    state.prefs.selected_project = Some(project_id_for_path(&real));
                }
                _ => eprintln!(
                    "pipkin: --project {} is not a folder; ignoring it",
                    path.display()
                ),
            }
        }
    }
    // Conversations with a saved copy can be read before the engine answers, and without it.
    if options.mode == Mode::Real {
        match storage.cached_conversations() {
            Ok(list) => {
                for saved in list {
                    state.restore_cached_conversation(
                        saved.id,
                        &saved.meta.project_path,
                        saved.meta.title,
                        saved.meta.updated_at,
                    );
                }
            }
            Err(e) => log::warn!("cannot read the saved conversations: {e}"),
        }
    }
    // Stored demo conversations must not leak into real mode. Drafts and unresolved requests
    // are namespaced per backend, so they are restored in either mode, as their conversations
    // appear. A real backend reports its own connection state through `start`.
    let entry = onboarding::EntryFacts {
        setup_completed: loaded.prefs.setup_completed,
        has_saved_work: !loaded.conversations.is_empty()
            || !loaded.drafts.is_empty()
            || !loaded.open_requests.is_empty()
            || !loaded.goals.is_empty()
            || !loaded.projects.is_empty(),
        demo: options.mode == Mode::Demo,
        explicit_external_server: options.pi_server_id.is_some()
            || options.pi_dir.is_some()
            || options.pi_repo.is_some(),
    }
    .surface();
    let mut restore = Restore::new(loaded, options.mode == Mode::Demo);
    restore.restore_conversations(&mut state);

    let model = cx.new(|_| {
        let mut model = Model::new(state);
        model.entry = entry;
        model
    });
    let (life_tx, life_rx) = async_channel::unbounded::<LifecycleEvent>();
    let (ack_tx, ack_rx) = async_channel::unbounded::<(ConversationId, u64, Result<(), String>)>();
    let (intent_tx, intent_rx) =
        async_channel::unbounded::<(ConversationId, RequestId, Result<(), String>)>();

    let cache_saver = Rc::new(CacheSaver::new(storage.clone()));
    let search_runner = Rc::new(SearchRunner::new(storage.clone()));
    let launch_config = LaunchConfig::from_env(options.editor.clone(), options.terminal.clone());
    {
        let backend = backend.clone();
        let storage = storage.clone();
        let (cache_saver, search_runner) = (cache_saver.clone(), search_runner.clone());
        let setup_tx = life_tx.clone();
        model.update(cx, |m, _| {
            m.set_support_report(support_report);
            m.set_effect_handler(Box::new(move |effect, cx| match effect {
                Effect::Backend(request) => backend.request(request),
                Effect::SaveDraft {
                    conversation,
                    text,
                    attachments,
                    rev,
                } => {
                    let ack_tx = ack_tx.clone();
                    storage.save_draft(conversation, text, attachments, rev, move |result| {
                        let _ = ack_tx.send_blocking((conversation, rev, result));
                    });
                }
                Effect::JournalIntent {
                    conversation,
                    request,
                    text,
                    attachments,
                    model,
                    origin,
                } => {
                    let intent_tx = intent_tx.clone();
                    let key = request.clone();
                    storage.journal_intent_with_origin(
                        conversation,
                        PendingIntent {
                            request,
                            origin,
                            text,
                            attachments,
                            model,
                        },
                        move |result| {
                            let _ = intent_tx.send_blocking((conversation, key, result));
                        },
                    );
                }
                Effect::JournalState { request, state, .. } => {
                    storage.journal_state(request, state)
                }
                Effect::SaveCache { conversation } => cache_saver.request(conversation, cx),
                Effect::LoadCache { conversation } => {
                    let storage = storage.clone();
                    cx.spawn(async move |this, cx| {
                        let loaded = cx
                            .background_executor()
                            .spawn(async move { storage.load_cache(conversation) })
                            .await;
                        match loaded {
                            Ok(Some(copy)) => {
                                this.update(cx, |m, cx| {
                                    m.mutate(
                                        |s| {
                                            s.apply_cache(
                                                conversation,
                                                copy.items,
                                                copy.has_older,
                                                copy.synced_at,
                                            )
                                        },
                                        cx,
                                    )
                                })
                                .ok();
                            }
                            Ok(None) => {}
                            Err(e) => log::warn!("cannot read the saved copy: {e}"),
                        }
                    })
                    .detach();
                }
                Effect::SearchHistory { query } => search_runner.request(query, cx),
                Effect::CopyText(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                Effect::SaveText {
                    suggested_name,
                    text,
                } => {
                    let start = std::env::var_os("HOME")
                        .map(PathBuf::from)
                        .filter(|p| p.is_dir())
                        .unwrap_or_else(|| PathBuf::from("/"));
                    let chosen = cx.prompt_for_new_path(&start, Some(&suggested_name));
                    cx.spawn(async move |this, cx| {
                        let Ok(Ok(Some(path))) = chosen.await else {
                            return;
                        };
                        let written = cx
                            .background_executor()
                            .spawn(async move { std::fs::write(&path, text).map(|_| path) })
                            .await;
                        if let Err(e) = written {
                            this.update(cx, |m, cx| {
                                m.mutate(
                                    |s| s.set_notice(format!("Could not save the file: {e}")),
                                    cx,
                                )
                            })
                            .ok();
                        }
                    })
                    .detach();
                }
                Effect::Launch(what) => {
                    let config = launch_config.clone();
                    cx.spawn(async move |this, cx| {
                        let result = cx
                            .background_executor()
                            .spawn(async move { launch::run(&what, &config) })
                            .await;
                        if let Err(e) = result {
                            this.update(cx, |m, cx| {
                                m.mutate(|s| s.set_notice(format!("Could not open it: {e}")), cx)
                            })
                            .ok();
                        }
                    })
                    .detach();
                }
                Effect::SavePrefs(prefs) => storage.save_prefs(&prefs),
                Effect::CompleteSetup(prefs) => {
                    let tx = setup_tx.clone();
                    storage.complete_setup(prefs, move |result| {
                        let _ = tx.send_blocking(LifecycleEvent::SetupSaved(result));
                    });
                }
                Effect::SaveGoal {
                    conversation,
                    text,
                    paused,
                } => storage.save_goal(conversation, text, paused),
                Effect::SaveProject { path } => storage.save_project(path),
                Effect::SaveConversation { conversation } => {
                    // The model is mid-update here; read the conversation once it settles.
                    let storage = storage.clone();
                    cx.spawn(async move |this, cx| {
                        let snapshot = this
                            .read_with(cx, |m, _| {
                                m.state
                                    .conversation(conversation)
                                    .map(|c| DemoConversation {
                                        id: c.id,
                                        project: c.project,
                                        title: c.title.clone(),
                                        updated_at: c.updated_at,
                                    })
                            })
                            .ok()
                            .flatten();
                        if let Some(snapshot) = snapshot {
                            storage.save_conversation(snapshot);
                        }
                    })
                    .detach();
                }
            }));
        });
    }

    // Conversations known from their saved copies are readable at once: restore their drafts
    // and open the remembered one, which shows its saved copy while the engine connects (or
    // when it never does).
    if options.mode == Mode::Real {
        let restore = &mut restore;
        model.update(cx, |m, cx| {
            m.mutate(
                |state| {
                    let mut out = restore.apply(state);
                    out.merge(state.select_initial());
                    out
                },
                cx,
            )
        });
    }

    // Backend events: drain everything queued per wakeup and apply as one batch.
    {
        let weak = model.downgrade();
        cx.spawn(async move |cx| {
            while let Ok(first) = event_rx.recv().await {
                let mut batch = vec![first];
                while batch.len() < MAX_BATCH {
                    match event_rx.try_recv() {
                        Ok(event) => batch.push(event),
                        Err(_) => break,
                    }
                }
                let batch = coalesce(batch);
                let alive = weak.update(cx, |m, cx| {
                    for event in batch {
                        m.apply_event(event, cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    // Draft save acknowledgments (delivered by the storage writer after commit).
    {
        let weak = model.downgrade();
        cx.spawn(async move |cx| {
            while let Ok(first) = ack_rx.recv().await {
                let mut batch = vec![first];
                while let Ok(next) = ack_rx.try_recv() {
                    batch.push(next);
                }
                let alive = weak.update(cx, |m, cx| {
                    for (conversation, rev, result) in batch {
                        m.draft_saved(conversation, rev, result, cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    // Journal commit acknowledgments: only now may a submission be sent.
    {
        let weak = model.downgrade();
        cx.spawn(async move |cx| {
            while let Ok((conversation, request, result)) = intent_rx.recv().await {
                let alive = weak.update(cx, |m, cx| {
                    m.mutate(
                        |state| state.intent_persisted(conversation, &request, result),
                        cx,
                    );
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    // Wall clock for the core, refreshed periodically (the core never reads a clock itself).
    {
        let weak = model.downgrade();
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(CLOCK_REFRESH).await;
                let now = unix_secs();
                if weak.update(cx, |m, _| m.state.set_now(now)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    // Lifecycle updates: connection state and the catalog. The sink never drops an update.
    {
        let tx = life_tx.clone();
        storage.set_error_sink(Arc::new(move |message| {
            let _ = tx.send_blocking(LifecycleEvent::StorageIssue(message));
        }));
    }
    {
        let weak = model.downgrade();
        cx.spawn(async move |cx| {
            while let Ok(event) = life_rx.recv().await {
                let restore = &mut restore;
                let alive = weak.update(cx, |m, cx| match event {
                    LifecycleEvent::Connection(c) => m.set_connection(c, cx),
                    LifecycleEvent::Catalog(boot) => {
                        if m.entry == onboarding::EntrySurface::Welcome
                            && !m.setup.started()
                            && !boot.conversations.is_empty()
                        {
                            m.entry = onboarding::EntrySurface::Workspace;
                        }
                        m.mutate(
                            |state| {
                                let mut out = state.apply_catalog(boot);
                                out.merge(restore.apply(state));
                                out.merge(state.select_initial());
                                out
                            },
                            cx,
                        );
                    }
                    LifecycleEvent::ModelSelected(model) => m.engine_model_selected(model, cx),
                    LifecycleEvent::SignIn(snapshot) => m.set_sign_in(snapshot, cx),
                    LifecycleEvent::SetupSaved(result) => m.setup_saved(result, cx),
                    LifecycleEvent::Notice(message) => {
                        m.mutate(|state| state.set_notice(message), cx)
                    }
                    LifecycleEvent::StorageIssue(message) => {
                        m.mutate(|state| state.set_storage_issue(message), cx)
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }
    backend.start(Box::new(move |event| {
        let _ = life_tx.send_blocking(event);
    }));

    if let Some(demo) = demo {
        cx.set_global(DemoControls {
            scenarios: script::SCENARIOS.iter().map(|s| (s.0, s.1)).collect(),
            current_scenario: Rc::new({
                let backend = demo.clone();
                move || backend.scenario()
            }),
            set_scenario: Rc::new({
                let backend = demo.clone();
                move |name| {
                    backend.set_scenario(name);
                }
            }),
            save_failure: Rc::new({
                let storage = storage.clone();
                move || storage.write_failure()
            }),
            set_save_failure: Rc::new({
                let storage = storage.clone();
                move |fail| storage.set_write_failure(fail)
            }),
        });
    }

    // Orderly exit: flush every dirty draft, then drain and join the storage writer.
    {
        let weak = model.downgrade();
        let storage = storage.clone();
        let backend = backend.clone();
        let cache_saver = cache_saver.clone();
        cx.on_app_quit(move |cx| {
            cache_saver.flush_now(&weak, cx);
            let _ = weak.update(cx, |m, cx| {
                let dirty: Vec<ConversationId> = m
                    .state
                    .conversations
                    .iter()
                    .filter(|c| c.draft.save == SaveState::Dirty)
                    .map(|c| c.id)
                    .collect();
                for id in dirty {
                    m.dispatch(Command::FlushDraft(id), cx);
                }
            });
            let storage = storage.clone();
            let backend = backend.clone();
            let background = cx.background_executor().clone();
            async move {
                background
                    .spawn(async move {
                        // Stopping an engine this app launched can take a few seconds; it must
                        // not freeze the window, and it must finish before the process exits.
                        backend.shutdown();
                        storage.shutdown();
                    })
                    .await;
            }
        })
        .detach();
    }

    model
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Options, String> {
        Options::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn managed_engine_remembers_a_portable_id_and_honors_explicit_identity() {
        let profile = tempfile::tempdir().unwrap();
        let options = Options {
            pi_dir: Some(profile.path().join("server")),
            data_dir: Some(profile.path().join("app")),
            ..Options::default()
        };
        let first = managed_engine(&options, profile.path()).unwrap();
        assert!(pi_client::protocol::is_server_id(&first.server_id));
        assert_eq!(
            managed_engine(&options, profile.path()).unwrap().server_id,
            first.server_id
        );
        let explicit = platform::fresh_server_id().unwrap();
        let options = Options {
            pi_server_id: Some(explicit.clone()),
            ..options
        };
        assert_eq!(
            managed_engine(&options, profile.path()).unwrap().server_id,
            explicit
        );
        assert_eq!(
            std::fs::read_to_string(first.server_dir.join("default-server-id")).unwrap(),
            first.server_id
        );
    }

    #[test]
    fn defaults_and_flags() {
        assert_eq!(parse(&[]).unwrap(), Options::default());
        assert_eq!(Options::default().mode, Mode::Real);
        let o = parse(&["--demo", "followup", "--data-dir=/tmp/x", "--speed", "0.5"]).unwrap();
        assert_eq!(o.scenario, "followup");
        assert_eq!(o.mode, Mode::Demo);
        assert_eq!(o.data_dir, Some(PathBuf::from("/tmp/x")));
        assert_eq!(o.speed, 0.5);
    }

    #[test]
    fn relative_paths_become_absolute_because_the_engine_starts_elsewhere() {
        let here = std::env::current_dir().unwrap();
        let o = parse(&[
            "--pi-repo=../pi-fork/pi",
            "--pi-agent-dir",
            "agent",
            "--data-dir=d",
            "--pi-dir=s",
        ])
        .unwrap();
        for path in [&o.pi_repo, &o.pi_agent_dir, &o.data_dir, &o.pi_dir] {
            assert!(path.as_ref().unwrap().is_absolute(), "{path:?}");
        }
        assert_eq!(o.pi_agent_dir, Some(here.join("agent")));
    }

    #[test]
    fn real_mode_flags() {
        let o = parse(&[
            "--pi-repo=/pi",
            "--pi-dir",
            "/srv",
            "--pi-agent-dir=/agent",
            "--project",
            "/work/a",
            "--project=/work/b",
        ])
        .unwrap();
        assert_eq!(o.mode, Mode::Real);
        assert_eq!(o.pi_repo, Some(PathBuf::from("/pi")));
        assert_eq!(o.pi_dir, Some(PathBuf::from("/srv")));
        assert_eq!(o.pi_agent_dir, Some(PathBuf::from("/agent")));
        assert_eq!(
            o.projects,
            [PathBuf::from("/work/a"), PathBuf::from("/work/b")]
        );
        assert!(parse(&["--pi-server-id", "nope"]).is_err());
        assert!(parse(&["--project"]).is_err());
    }

    #[test]
    fn the_namespace_names_the_backend_and_the_profile() {
        let demo = parse(&["--demo", "normal"]).unwrap();
        assert_eq!(namespace(&demo), DEMO_NAMESPACE);
        let a = parse(&["--pi-dir", "/srv/a"]).unwrap();
        let b = parse(&["--pi-dir", "/srv/b"]).unwrap();
        assert!(namespace(&a).starts_with("pi:"));
        assert_ne!(
            namespace(&a),
            namespace(&b),
            "two profiles never share drafts or journals"
        );
        assert_eq!(
            namespace(&a),
            namespace(&parse(&["--pi-dir", "/srv/a"]).unwrap())
        );
    }

    #[test]
    fn the_engine_is_pi_unless_native_is_chosen_explicitly() {
        let o = parse(&[]).unwrap();
        assert_eq!((o.engine, o.engine_host), (EngineKind::Pi, HostKind::Child));
        assert_eq!(parse(&["--engine", "pi"]).unwrap(), Options::default());
        let n = parse(&["--engine=native"]).unwrap();
        assert_eq!(
            (n.mode, n.engine, n.engine_host),
            (Mode::Real, EngineKind::Native, HostKind::Child)
        );
        let i = parse(&["--engine-host", "inprocess", "--engine", "native"]).unwrap();
        assert_eq!(i.engine_host, HostKind::InProcess);
        assert_eq!(
            parse(&["--engine", "native", "--engine-host=child"])
                .unwrap()
                .engine_host,
            HostKind::Child
        );
        assert!(
            parse(&["--engine", "zed"])
                .unwrap_err()
                .contains("unknown engine")
        );
        assert!(parse(&["--engine"]).is_err());
        assert!(
            parse(&["--engine-host", "inprocess"])
                .unwrap_err()
                .contains("--engine native")
        );
        assert!(parse(&["--engine", "native", "--engine-host", "cloud"]).is_err());
        assert!(parse(&["--engine", "native", "--demo", "normal"]).is_err());
        assert!(parse(&["--engine", "native", "--pi-dir", "/srv"]).is_err());
        assert!(parse(&["--pi-repo=/pi", "--engine", "native"]).is_err());
    }

    #[test]
    fn the_native_namespace_names_the_engine_data_directory() {
        let a = parse(&["--engine", "native", "--data-dir", "/tmp/a"]).unwrap();
        let b = parse(&["--engine", "native", "--data-dir", "/tmp/b"]).unwrap();
        let pi = parse(&["--data-dir", "/tmp/a"]).unwrap();
        assert_eq!(
            namespace(&a),
            format!("native:{:x}", stable_id("/tmp/a/engine"))
        );
        assert_ne!(namespace(&a), namespace(&b));
        assert!(namespace(&pi).starts_with("pi:"));
        assert_ne!(
            namespace(&a),
            namespace(&pi),
            "the engines never share desktop data"
        );
    }

    #[test]
    fn rejects_bad_input() {
        assert!(
            parse(&["--demo", "nope"])
                .unwrap_err()
                .contains("unknown scenario")
        );
        assert!(parse(&["--speed", "-1"]).is_err());
        assert!(parse(&["--speed"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
    }

    #[test]
    fn coalesce_merges_only_matching_tokens() {
        let ev = |op: u64, kind| BackendEvent {
            conversation: ConversationId(1),
            generation: 1,
            op: Some(OperationId(op)),
            kind,
        };
        let batch = vec![
            ev(1, EventKind::Token("a ".into())),
            ev(1, EventKind::Token("b ".into())),
            ev(2, EventKind::Token("c".into())),
            ev(2, EventKind::Completed),
        ];
        let out = coalesce(batch);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].kind, EventKind::Token("a b ".into()));
    }
}
