//! The application controller: wires storage, the demo backend and the GPUI `Model`.
//!
//! Effects emitted by the core are executed here, never in render. Backend events arrive on an
//! async channel and are applied on the foreground in coalesced batches.

#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{App, AppContext as _, ClipboardItem, Entity};
use pipkin_core::*;
use pipkin_ui::{DemoControls, Model};

use crate::adapters::native::supervisor::{HostKind, HostSpec, WorkerSpec};
use crate::adapters::native::{ImportResult, NativeBackend, NativeConfig};
use crate::adapters::script;
use crate::adapters::{DemoBackend, DemoOptions};
use crate::launch::{self, LaunchConfig};
use crate::platform;
use crate::storage::{
    CacheMeta, CopyReport, DEMO_NAMESPACE, DemoConversation, Loaded, OpenRequest, Startup, Storage,
    StoredDraft,
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
    /// Legacy agent directory used only as an explicit import source.
    pub pi_agent_dir: Option<PathBuf>,
    /// Real mode: project folders to add at startup (repeatable); the last one is selected.
    pub projects: Vec<PathBuf>,
    /// The editor command for "open in editor" (otherwise `$VISUAL`, `$EDITOR`, the desktop's).
    pub editor: Option<String>,
    /// The terminal command for "open terminal" (otherwise `$TERMINAL`, the desktop's).
    pub terminal: Option<String>,
    /// Real mode uses the built-in engine. `--engine native` remains a CLI alias.
    pub engine: EngineKind,
    /// `--engine native`: where the built-in engine runs (`--engine-host`).
    pub engine_host: HostKind,
    /// Explicit local endpoint of a separately owned native engine.
    pub engine_attach: Option<PathBuf>,
    /// `--engine native`: session extensions (`--engine-extension`, repeatable): an executable
    /// speaking the process extension protocol, or `builtin:<name>`.
    pub engine_extensions: Vec<String>,
    pub engine_clear_extensions: bool,
}

/// The engine behind real mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EngineKind {
    /// Pipkin's built-in engine (`pipkin-engine`).
    #[default]
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
            pi_agent_dir: None,
            projects: vec![],
            editor: None,
            terminal: None,
            engine: EngineKind::Native,
            engine_host: HostKind::Child,
            engine_attach: None,
            engine_extensions: vec![],
            engine_clear_extensions: false,
        }
    }
}

/// A path made absolute against the current directory. The engine is started from the home
/// directory, so relative data/import paths must be resolved before that change.
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
        let mut explicit_engine = false;
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
                "--project" => options.projects.push(PathBuf::from(value("--project")?)),
                "--pi-dir" | "--pi-repo" | "--pi-server-id" | "--pi-extension" => {
                    return Err(format!(
                        "{flag} belonged to the retired Pi runtime; use the built-in engine, --engine-attach for a native server, or --pi-agent-dir for migration"
                    ));
                }
                "--engine" => {
                    explicit_engine = true;
                    options.engine = match value("--engine")?.as_str() {
                        "pi" => return Err("The Pi runtime has been retired; Pipkin uses its built-in engine. Use Import from Pi to bring over existing conversations.".into()),
                        "native" => EngineKind::Native,
                        other => {
                            return Err(format!("unknown engine {other:?}; choose native"));
                        }
                    }
                }
                "--engine-clear-extensions" => {
                    options.engine_clear_extensions = true;
                }
                "--engine-extension" => {
                    let raw = value("--engine-extension")?;
                    let spec = if raw.starts_with("builtin:") {
                        raw
                    } else {
                        std::fs::canonicalize(&raw)
                            .map_err(|e| format!("--engine-extension {raw}: {e}"))?
                            .display()
                            .to_string()
                    };
                    pipkin_engine::config::parse_extension(&spec)
                        .map_err(|e| format!("--engine-extension: {e}"))?;
                    options.engine_extensions.push(spec);
                }
                "--engine-attach" => {
                    let path = absolute(value("--engine-attach")?)?;
                    options.engine_attach = Some(path.canonicalize().unwrap_or(path));
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
        if explicit_engine && options.mode == Mode::Demo {
            return Err("--engine native cannot be combined with --demo".into());
        }
        if options.engine_clear_extensions && !options.engine_extensions.is_empty() {
            return Err("Cannot select extensions and clear the default together".into());
        }
        if options.engine_attach.is_some()
            && (engine_host.is_some()
                || !options.engine_extensions.is_empty()
                || options.engine_clear_extensions)
        {
            return Err("an attached engine owns its configuration; omit --engine-host, --engine-extension and --engine-clear-extensions".into());
        }
        if let Some(host) = engine_host {
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

/// Legacy profile location used only to find the desktop namespace for migration.
fn profile_dir(_options: &Options) -> PathBuf {
    std::env::var_os("PI_SERVER_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(".pi/server")
        })
}

/// Desktop data is namespaced by backend so identities from one never apply to another. A real
/// backend is identified by its profile directory, so two engines never share drafts, journals
/// or selections.
fn namespace(options: &Options) -> String {
    match options.mode {
        Mode::Demo => DEMO_NAMESPACE.to_owned(),
        Mode::Real if options.engine_attach.is_some() => {
            format!(
                "native-attach:{:x}",
                stable_id(
                    &options
                        .engine_attach
                        .as_ref()
                        .unwrap()
                        .display()
                        .to_string()
                )
            )
        }
        Mode::Real => format!(
            "native:{:x}",
            stable_id(
                &platform::engine_dirs(options.data_dir.as_deref())
                    .data
                    .display()
                    .to_string()
            )
        ),
    }
}

/// The desktop namespace of the Pi engine for this profile: where conversations imported into
/// the built-in engine had their drafts, journal and saved copies.
fn pi_namespace(options: &Options) -> String {
    format!(
        "pi:{:x}",
        stable_id(&profile_dir(options).display().to_string())
    )
}

/// After an import into the built-in engine: copy (never move) the desktop data of the imported
/// conversations from the Pi namespace and, in the same transaction, record that the built-in
/// engine now owns the ones the engine reported imported now (`imported`; never blocked or
/// locked sessions, which are in neither list). Then read the stored state back so the newly
/// imported ones can be restored ([`Restore::add_imported`]) as their conversations appear.
/// Conversations imported earlier (`existing`) are copied too (nothing the target already has
/// is replaced) but are not restored over what is shown, and their ownership is not recorded
/// again: one released back to Pi stays released. Blocking: run it off the foreground.
pub(crate) fn copy_imported(
    storage: &Storage,
    pi_namespace: &str,
    imported: &[ConversationId],
    existing: &[ConversationId],
    imported_at: i64,
) -> Result<(CopyReport, Loaded), String> {
    let report = if imported.is_empty() && existing.is_empty() {
        CopyReport::default()
    } else {
        storage.import_conversations_from(pi_namespace, imported, existing, imported_at)?
    };
    Ok((report, storage.load_all()?))
}

/// The notice after an import: the engine's account plus what came along from the desktop and
/// what that means for Pi.
fn import_notice(message: String, imported: usize, copied: Result<CopyReport, String>) -> String {
    match copied {
        Ok(r) => {
            let mut notice = message;
            if r.drafts + r.journal + r.caches + r.goals > 0 {
                notice.push_str("\nDrafts, prompt history and saved copies came along from Pi.");
            }
            if imported > 0 {
                notice.push_str(
                    "\nThe imported conversations now continue in the built-in engine; \
                     the original Pi profile remains unchanged for backup or an older Pipkin installation.",
                );
            }
            notice
        }
        Err(e) => format!(
            "{message}\nTheir drafts and saved copies could not be brought over, and they were \
             not marked as owned by the built-in engine: {e}"
        ),
    }
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

    /// Add the stored state of conversations that just came from an import (only theirs, so
    /// nothing shown elsewhere is replaced).
    pub(crate) fn add_imported(&mut self, loaded: Loaded, ids: &[ConversationId]) {
        let wanted = |id: &ConversationId| ids.contains(id);
        self.drafts.extend(
            loaded
                .drafts
                .into_iter()
                .filter(|d| wanted(&d.conversation)),
        );
        self.requests.extend(
            loaded
                .open_requests
                .into_iter()
                .filter(|r| wanted(&r.conversation)),
        );
        self.prompt_histories.extend(
            loaded
                .prompt_histories
                .into_iter()
                .filter(|(id, _)| wanted(id)),
        );
        self.goals
            .extend(loaded.goals.into_iter().filter(|(id, _, _)| wanted(id)));
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
    let (event_tx, event_rx) = if options.mode == Mode::Real {
        crate::event_queue::bounded(
            crate::event_queue::NATIVE_EVENTS,
            crate::event_queue::NATIVE_BYTES,
        )
    } else {
        crate::event_queue::plain(EVENT_QUEUE)
    };
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
    let support_report = if options.mode == Mode::Real {
        crate::diagnostics::prepare_native()
    } else {
        crate::diagnostics::prepare_demo()
    };
    // The built-in engine also imports Pi profiles (explicitly, never at startup).
    let mut native_backend: Option<Arc<NativeBackend>> = None;
    let backend: Arc<dyn Backend> = match &demo {
        Some(demo) => demo.clone(),
        None => {
            let dirs = platform::engine_dirs(options.data_dir.as_deref());
            let log_path = crate::adapters::native::engine_log_path(&platform::data_dir(
                options.data_dir.as_deref(),
            ));
            let host = if let Some(endpoint) = &options.engine_attach {
                HostSpec::External(endpoint.clone())
            } else {
                match options.engine_host {
                    HostKind::Child => match WorkerSpec::engine(
                        &dirs.data,
                        &dirs.config,
                        &options.engine_extensions,
                        options.engine_clear_extensions,
                        &log_path,
                    ) {
                        Ok(spec) => HostSpec::Child(spec),
                        Err(error) => {
                            eprintln!("pipkin: {error}");
                            std::process::exit(2);
                        }
                    },
                    HostKind::InProcess => HostSpec::InProcess {
                        data_dir: dirs.data.clone(),
                        config_dir: dirs.config.clone(),
                        extensions: options.engine_extensions.clone(),
                        clear_extensions: options.engine_clear_extensions,
                    },
                }
            };
            let location = options
                .engine_attach
                .as_ref()
                .unwrap_or(&dirs.data)
                .display()
                .to_string();
            let native = Arc::new(NativeBackend::new(
                event_tx,
                NativeConfig::new(host, location)
                    .with_import_source(options.pi_agent_dir.as_deref()),
            ));
            native_backend = Some(native.clone());
            native
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
        state.import_available = native_backend.is_some();
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
        explicit_external_server: options.engine_attach.is_some(),
    }
    .surface();
    let restore = Rc::new(std::cell::RefCell::new(Restore::new(
        loaded,
        options.mode == Mode::Demo,
    )));
    restore.borrow_mut().restore_conversations(&mut state);

    let model = cx.new(|_| {
        let mut model = Model::new(state);
        model.entry = entry;
        model
    });
    let (life_tx, life_rx) = async_channel::unbounded::<LifecycleEvent>();
    let (ack_tx, ack_rx) = async_channel::unbounded::<(ConversationId, u64, Result<(), String>)>();
    let (intent_tx, intent_rx) =
        async_channel::unbounded::<(ConversationId, RequestId, Result<(), String>)>();
    let (import_tx, import_rx) = async_channel::unbounded::<ImportResult>();

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
            let native_backend = native_backend.clone();
            m.set_effect_handler(Box::new(move |effect, cx| match effect {
                Effect::Backend(request) => backend.request(request),
                Effect::Import(step) => {
                    let tx = import_tx.clone();
                    let done: crate::adapters::native::ImportDone = Box::new(move |result| {
                        let _ = tx.send_blocking(result);
                    });
                    match &native_backend {
                        Some(native) => native.import(step, done),
                        None => done(crate::adapters::native::import_unavailable(
                            &step,
                            "this engine cannot import a Pi profile",
                        )),
                    }
                }
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
                Effect::ReleaseOwnership { conversation } => {
                    let storage = storage.clone();
                    cx.spawn(async move |this, cx| {
                        let result = cx
                            .background_executor()
                            .spawn(async move { storage.release_ownership(conversation) })
                            .await
                            .map(|_| ());
                        this.update(cx, |m, cx| {
                            m.mutate(|s| s.ownership_released(conversation, result), cx)
                        })
                        .ok();
                    })
                    .detach();
                }
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
        let restore = restore.clone();
        model.update(cx, |m, cx| {
            m.mutate(
                |state| {
                    let mut out = restore.borrow_mut().apply(state);
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
                let events = coalesce(batch.iter_mut().map(|lease| lease.take()).collect());
                let alive = weak.update(cx, |m, cx| {
                    for event in events {
                        m.apply_event(event, cx);
                    }
                });
                // Release admission only after the batch has reached the core.
                drop(batch);
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
        let restore = restore.clone();
        cx.spawn(async move |cx| {
            while let Ok(event) = life_rx.recv().await {
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
                                out.merge(restore.borrow_mut().apply(state));
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
                    LifecycleEvent::Mcp(snapshot) => {
                        m.mutate(|state| state.apply_mcp(snapshot), cx)
                    }
                    LifecycleEvent::McpTest(result) => {
                        m.mutate(|state| state.mcp_test_result(result), cx)
                    }
                    LifecycleEvent::McpImport(preview) => {
                        m.mutate(|state| state.mcp_import_previewed(preview), cx)
                    }
                    LifecycleEvent::McpFailed(message) => {
                        m.mutate(|state| state.mcp_failed(message), cx)
                    }
                    LifecycleEvent::QuestionWaiting {
                        conversation,
                        waiting,
                    } => m.mutate(
                        |state| state.set_question_waiting(conversation, waiting),
                        cx,
                    ),
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }
    // Pi import answers: an offer to confirm, or the end of an import, after which the imported
    // conversations' desktop data is copied over from the Pi namespace.
    {
        let weak = model.downgrade();
        let storage = storage.clone();
        let restore = restore.clone();
        let pi_namespace = pi_namespace(&options);
        cx.spawn(async move |cx| {
            while let Ok(result) = import_rx.recv().await {
                let alive = match result {
                    ImportResult::Offer(offer) => weak.update(cx, |m, cx| {
                        m.mutate(|state| state.import_discovered(offer), cx)
                    }),
                    ImportResult::Finished {
                        message,
                        imported,
                        existing,
                    } => {
                        let copy = {
                            let storage = storage.clone();
                            let pi_namespace = pi_namespace.clone();
                            let imported = imported.clone();
                            cx.background_executor()
                                .spawn(async move {
                                    copy_imported(
                                        &storage,
                                        &pi_namespace,
                                        &imported,
                                        &existing,
                                        unix_secs(),
                                    )
                                })
                                .await
                        };
                        weak.update(cx, |m, cx| {
                            let copied = copy.map(|(report, loaded)| {
                                restore.borrow_mut().add_imported(loaded, &imported);
                                report
                            });
                            let notice = import_notice(message, imported.len(), copied);
                            m.mutate(
                                |state| {
                                    let mut out = restore.borrow_mut().apply(state);
                                    out.merge(state.import_finished(notice));
                                    out
                                },
                                cx,
                            )
                        })
                    }
                };
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
    fn native_is_default_and_explicit_alias_preserves_options() {
        let default = parse(&[]).unwrap();
        assert_eq!(default, Options::default());
        assert_eq!(default.engine, EngineKind::Native);
        assert_eq!(default.engine_host, HostKind::Child);
        assert_eq!(parse(&["--engine", "native"]).unwrap(), default);
        assert!(parse(&["--engine", "pi"]).unwrap_err().contains("retired"));
        for flag in ["--pi-repo", "--pi-dir", "--pi-server-id", "--pi-extension"] {
            assert!(parse(&[flag, "/old"]).unwrap_err().contains("retired"));
        }
        assert!(parse(&["--engine", "other"]).is_err());
        assert_eq!(
            parse(&["--engine-host", "inprocess"]).unwrap().engine_host,
            HostKind::InProcess
        );
        assert!(parse(&["--engine-host", "cloud"]).is_err());
    }

    #[test]
    fn demo_remains_explicit_and_simulated() {
        let demo = parse(&["--demo", "followup", "--speed=0.5", "--data-dir=/tmp/demo"]).unwrap();
        assert_eq!(demo.mode, Mode::Demo);
        assert_eq!(demo.scenario, "followup");
        assert_eq!(demo.speed, 0.5);
        assert_eq!(namespace(&demo), DEMO_NAMESPACE);
        assert!(parse(&["--engine", "native", "--demo", "normal"]).is_err());
        assert!(parse(&["--demo", "nope"]).is_err());
        assert!(parse(&["--speed", "-1"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
    }

    #[test]
    fn native_profiles_and_external_hosts_keep_separate_desktop_namespaces() {
        let a = parse(&["--data-dir", "/tmp/a"]).unwrap();
        let b = parse(&["--data-dir", "/tmp/b"]).unwrap();
        let attached = parse(&["--engine-attach", "/endpoint-a"]).unwrap();
        let other = parse(&["--engine-attach", "/endpoint-b"]).unwrap();
        assert_eq!(
            namespace(&a),
            format!("native:{:x}", stable_id("/tmp/a/engine"))
        );
        assert_ne!(namespace(&a), namespace(&b));
        assert_ne!(namespace(&attached), namespace(&other));
        assert_ne!(namespace(&a), namespace(&attached));
        assert_eq!(
            pi_namespace(&a),
            format!("pi:{:x}", stable_id(&profile_dir(&a).display().to_string()))
        );
        assert_ne!(pi_namespace(&a), namespace(&a));
        for flag in ["--engine-host", "--engine-extension"] {
            let value = if flag == "--engine-host" {
                "child"
            } else {
                "builtin:questions"
            };
            assert!(parse(&["--engine-attach", "/endpoint", flag, value]).is_err());
        }
        assert!(parse(&["--engine-attach", "/endpoint", "--engine-clear-extensions"]).is_err());
    }

    #[test]
    fn migration_source_projects_and_native_extensions_keep_their_contracts() {
        for attached in [false, true] {
            let mut args = vec![
                "--pi-agent-dir",
                "agent",
                "--data-dir",
                "data",
                "--project",
                "/a",
                "--project=/b",
            ];
            if attached {
                args.extend(["--engine-attach", "/endpoint"]);
            }
            let options = parse(&args).unwrap();
            assert_eq!(
                options.pi_agent_dir,
                Some(std::env::current_dir().unwrap().join("agent"))
            );
            assert!(options.data_dir.as_ref().unwrap().is_absolute());
            assert_eq!(options.projects, [PathBuf::from("/a"), PathBuf::from("/b")]);
            let config =
                NativeConfig::new(HostSpec::External("/endpoint".into()), "native-data".into())
                    .with_import_source(options.pi_agent_dir.as_deref());
            assert_eq!(
                config.import_agent_dir.as_deref(),
                options.pi_agent_dir.as_deref().and_then(Path::to_str)
            );
        }
        assert_eq!(
            parse(&["--engine-extension", "builtin:questions"])
                .unwrap()
                .engine_extensions,
            ["builtin:questions"]
        );
        assert!(
            parse(&["--engine-clear-extensions"])
                .unwrap()
                .engine_clear_extensions
        );
        assert!(
            parse(&[
                "--engine-clear-extensions",
                "--engine-extension",
                "builtin:questions"
            ])
            .is_err()
        );
        assert!(parse(&["--engine-extension", "builtin:nope"]).is_err());
        assert!(parse(&["--project"]).is_err());
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
