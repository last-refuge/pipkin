//! The MCP service: supervised servers (stdio processes and Streamable HTTP endpoints),
//! cached catalogs, bounded calls, per-server OAuth sign-in and the management operations
//! behind the native Connections page.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use pipkin_engine_protocol::AuthStatus;
use rmcp::model::{
    CallToolRequestParams, ClientConfig, ContentBlock, ElicitRequestParams, ElicitResult,
    ElicitationAction, GetPromptRequestParams, Implementation, ReadResourceRequestParams,
    ResourceContents,
};
use rmcp::service::RunningService;
use rmcp::transport::child_process::TokioChildProcess;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use rmcp::{ClientHandler, RoleClient, ServiceExt};
use serde_json::{Map, Value};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::config::{
    McpConfig, McpServerConfig, Transport, read_secrets, resolve_one, resolve_refs,
};
use super::http::{EngineHttp, auth_required};
use super::oauth::{McpFlow, discover, register};
use crate::auth::oauth::{
    AuthEvent, CANCELLED, Endpoints, FlowCtx, Interaction, InteractionSink, OAuthFlow, Prompt,
};
use crate::auth::store::{CredentialStore, FileStore, OAuthCredential, StoredCredential};
use crate::config::Env;
use crate::provider::BoxFuture;

/// Backoff after a failed start before another attempt.
pub const START_BACKOFF: Duration = Duration::from_secs(30);
/// Most tools a server may advertise (a bound on cached schema size).
pub const MAX_TOOLS_PER_SERVER: usize = 500;
/// Most results `mcp_search` returns.
pub const MAX_SEARCH_RESULTS: usize = 50;
/// Longest tool description kept in the catalog.
pub const MAX_DESCRIPTION_CHARS: usize = 2_000;
/// Largest input schema kept (serialized bytes).
pub const MAX_SCHEMA_BYTES: usize = 64 * 1024;
/// Tokens are refreshed when they expire within this margin.
const TOKEN_MARGIN_MS: i64 = 60 * 1000;
const REFRESH_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, PartialEq)]
pub struct ToolInfo {
    pub server: String,
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    /// The server's annotations, verbatim; hints only.
    pub annotations: Option<Value>,
    /// Configuration revision of the server the catalog was read from.
    pub revision: String,
}

impl ToolInfo {
    pub fn qualified(&self) -> String {
        format!("{}/{}", self.server, self.name)
    }
}

/// A resource or resource template a server offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceInfo {
    pub server: String,
    /// The URI, or the URI template (`template` is set).
    pub uri: String,
    pub name: String,
    pub description: String,
    pub mime_type: Option<String>,
    pub size: Option<u64>,
    pub template: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptArgumentInfo {
    pub name: String,
    pub description: String,
    pub required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptInfo {
    pub server: String,
    pub name: String,
    pub description: String,
    pub arguments: Vec<PromptArgumentInfo>,
}

/// One part of a read resource: text, or a binary blob reported by size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourcePart {
    pub uri: String,
    pub mime_type: Option<String>,
    pub text: Option<String>,
    /// Decoded size of a blob (its bytes are not kept).
    pub blob_bytes: Option<usize>,
}

/// A rendered prompt: its messages as `(role, text)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptOutcome {
    pub description: Option<String>,
    pub messages: Vec<(String, String)>,
}

/// What the person decided about a server's request for input (elicitation).
#[derive(Clone, Debug, PartialEq)]
pub struct ElicitOutcome {
    /// `accept`, `decline` or `cancel`.
    pub action: String,
    pub content: Option<Value>,
}

/// Presents a server's form to the person of the session whose call triggered it. `cancel`
/// is the triggering call's cancellation: a stopped run withdraws the questions.
pub trait Elicitor: Send + Sync {
    fn elicit(
        &self,
        session: &str,
        server: &str,
        message: &str,
        schema: Value,
        cancel: CancellationToken,
    ) -> BoxFuture<'static, ElicitOutcome>;
}

/// A sign-in prompt as a client sees it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PromptView {
    pub id: String,
    /// `text`, `secret`, `select` or `manual_code`.
    pub kind: String,
    pub message: String,
    pub placeholder: Option<String>,
    pub options: Vec<(String, String)>,
}

/// Progress of a server's sign-in. The URL is volatile presentation state, never logged.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuthProgress {
    pub status: AuthStatus,
    pub message: String,
    pub url: Option<String>,
    pub prompt: Option<PromptView>,
}

/// What a client sees of a server.
#[derive(Clone, Debug, PartialEq)]
pub struct ServerSnapshot {
    pub config: McpServerConfig,
    /// `stopped`, `connecting`, `ready`, `authRequired`, `failed`.
    pub state: String,
    pub error: Option<String>,
    pub catalog: Option<Vec<ToolInfo>>,
    pub resources: Option<Vec<ResourceInfo>>,
    pub prompts: Option<Vec<PromptInfo>>,
    /// What the server declared at initialization: `tools`, `resources`,
    /// `resourcesSubscribe`, `prompts`, `logging`, `completions`.
    pub capabilities: Vec<String>,
    pub in_flight: usize,
    pub auth: AuthProgress,
    /// Saved tokens exist for this server (metadata only).
    pub signed_in: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TestOutcome {
    pub tools: Vec<String>,
    pub server_name: Option<String>,
    pub server_version: Option<String>,
    pub protocol_version: Option<String>,
    pub latency_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpError {
    NotConfigured(String),
    UnknownServer(String),
    Disabled(String),
    StartFailed {
        server: String,
        reason: String,
    },
    AuthRequired {
        server: String,
    },
    UnknownTool {
        server: String,
        tool: String,
    },
    ToolDisabled {
        server: String,
        tool: String,
    },
    /// The server did not declare this feature (resources, prompts).
    Unsupported {
        server: String,
        feature: String,
    },
    InvalidArguments(String),
    Timeout {
        server: String,
        tool: String,
        secs: u64,
    },
    Cancelled,
    Transport {
        server: String,
        reason: String,
    },
}

impl McpError {
    pub fn kind(&self) -> &'static str {
        match self {
            McpError::NotConfigured(_) => "not_configured",
            McpError::UnknownServer(_) => "unknown_server",
            McpError::Disabled(_) => "disabled",
            McpError::StartFailed { .. } => "start_failed",
            McpError::AuthRequired { .. } => "auth_required",
            McpError::UnknownTool { .. } => "unknown_tool",
            McpError::ToolDisabled { .. } => "tool_disabled",
            McpError::Unsupported { .. } => "unsupported",
            McpError::InvalidArguments(_) => "invalid_arguments",
            McpError::Timeout { .. } => "timeout",
            McpError::Cancelled => "cancelled",
            McpError::Transport { .. } => "transport",
        }
    }
}

impl std::fmt::Display for McpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            McpError::NotConfigured(m) => write!(f, "no MCP servers are configured: {m}"),
            McpError::UnknownServer(s) => write!(f, "no MCP server named {s:?} is configured"),
            McpError::Disabled(s) => write!(f, "MCP server {s} is disabled in settings"),
            McpError::StartFailed { server, reason } => {
                write!(f, "MCP server {server} could not start: {reason}")
            }
            McpError::AuthRequired { server } => write!(
                f,
                "MCP server {server} needs you to sign in first (Settings → MCP servers)"
            ),
            McpError::UnknownTool { server, tool } => {
                write!(f, "MCP server {server} has no tool {tool:?}")
            }
            McpError::ToolDisabled { server, tool } => {
                write!(f, "tool {server}/{tool} is switched off in settings")
            }
            McpError::Unsupported { server, feature } => {
                write!(f, "MCP server {server} does not offer {feature}")
            }
            McpError::InvalidArguments(m) => f.write_str(m),
            McpError::Timeout { server, tool, secs } => write!(
                f,
                "{server}/{tool} did not answer within {secs} s; its effect is unknown"
            ),
            McpError::Cancelled => f.write_str("the call was cancelled; its effect is unknown"),
            McpError::Transport { server, reason } => write!(f, "MCP server {server}: {reason}"),
        }
    }
}

/// A tool's outcome: the server's content blocks and its own error flag.
#[derive(Clone, Debug, PartialEq)]
pub struct CallOutcome {
    pub content: Vec<pipkin_engine_protocol::Block>,
    pub structured: Option<Value>,
    pub is_error: bool,
    pub tool: ToolInfo,
}

/// Receives catalog-change notifications for one server.
#[derive(Clone)]
struct Handler {
    server: String,
    dirty: Arc<AtomicBool>,
    resources_dirty: Arc<AtomicBool>,
    prompts_dirty: Arc<AtomicBool>,
    /// Sessions with a call in flight on this server (with the call's cancellation), oldest
    /// first; a server's request for input goes to the most recent caller.
    callers: Arc<Mutex<Vec<(String, CancellationToken)>>>,
    elicitor: Option<Arc<dyn Elicitor>>,
}

impl ClientHandler for Handler {
    fn get_info(&self) -> ClientConfig {
        // Advertised honestly: form elicitation when a presentation can answer; no sampling,
        // no roots, no URL elicitation.
        let mut capabilities = rmcp::model::ClientCapabilities::default();
        if self.elicitor.is_some() {
            // Form elicitation without schema validation (the person's answers are typed by
            // the question host, not checked against the schema here).
            capabilities.elicitation = serde_json::from_value(serde_json::json!({"form": {}})).ok();
        }
        ClientConfig::new(
            capabilities,
            Implementation::new("pipkin", env!("CARGO_PKG_VERSION")),
        )
    }

    fn on_tool_list_changed(
        &self,
        _context: rmcp::service::NotificationContext<RoleClient>,
    ) -> impl std::future::Future<Output = ()> + Send + '_ {
        self.dirty.store(true, Ordering::SeqCst);
        std::future::ready(())
    }

    fn on_resource_list_changed(
        &self,
        _context: rmcp::service::NotificationContext<RoleClient>,
    ) -> impl std::future::Future<Output = ()> + Send + '_ {
        self.resources_dirty.store(true, Ordering::SeqCst);
        std::future::ready(())
    }

    fn on_prompt_list_changed(
        &self,
        _context: rmcp::service::NotificationContext<RoleClient>,
    ) -> impl std::future::Future<Output = ()> + Send + '_ {
        self.prompts_dirty.store(true, Ordering::SeqCst);
        std::future::ready(())
    }

    fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        _context: rmcp::service::RequestContext<RoleClient>,
    ) -> impl std::future::Future<Output = Result<ElicitResult, rmcp::ErrorData>> + Send + '_ {
        let caller = self
            .callers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .last()
            .cloned();
        let elicitor = self.elicitor.clone();
        let server = self.server.clone();
        Box::pin(async move {
            let (Some((session, cancel)), Some(elicitor)) = (caller, elicitor) else {
                return Ok(ElicitResult::new(ElicitationAction::Decline));
            };
            match request {
                ElicitRequestParams::FormElicitationParams {
                    message,
                    requested_schema,
                    ..
                } => {
                    let schema = serde_json::to_value(&requested_schema).unwrap_or(Value::Null);
                    let outcome = elicitor
                        .elicit(&session, &server, &message, schema, cancel)
                        .await;
                    let action = match outcome.action.as_str() {
                        "accept" => ElicitationAction::Accept,
                        "cancel" => ElicitationAction::Cancel,
                        _ => ElicitationAction::Decline,
                    };
                    let mut result = ElicitResult::new(action);
                    if let Some(content) = outcome.content {
                        result = result.with_content(content);
                    }
                    Ok(result)
                }
                // URL elicitation is not advertised; a server sending it is declined.
                _ => Ok(ElicitResult::new(ElicitationAction::Decline)),
            }
        })
    }
}

/// Sessions with a call in flight on a server, with each call's cancellation.
type Callers = Arc<Mutex<Vec<(String, CancellationToken)>>>;

type Client = RunningService<RoleClient, Handler>;

/// A running server taken out of the service, to be stopped off any lock.
pub struct ServerHandle(Arc<Client>);

impl ServerHandle {
    /// Send the shutdown and wait for the process; only the last holder can.
    pub async fn stop(self) {
        if let Ok(client) = Arc::try_unwrap(self.0) {
            let _ = client.cancel().await;
        }
    }
}

enum State {
    Stopped,
    Connecting,
    Ready {
        client: Arc<Client>,
        dirty: Arc<AtomicBool>,
        resources_dirty: Arc<AtomicBool>,
        prompts_dirty: Arc<AtomicBool>,
        callers: Arc<Mutex<Vec<(String, CancellationToken)>>>,
    },
    /// The server answered 401; the person must sign in.
    AuthRequired {
        www_authenticate: Option<String>,
    },
    Failed {
        reason: String,
        until: Instant,
    },
}

struct Attempt {
    cancel: CancellationToken,
    pending: Option<(String, oneshot::Sender<String>)>,
}

struct Entry {
    config: McpServerConfig,
    state: State,
    catalog: Option<Vec<ToolInfo>>,
    resources: Option<Vec<ResourceInfo>>,
    prompts: Option<Vec<PromptInfo>>,
    capabilities: Vec<String>,
    last_used: Instant,
    in_flight: usize,
    auth: AuthProgress,
    attempt: Option<Attempt>,
    signed_in: bool,
}

pub type ChangeListener = Arc<dyn Fn() + Send + Sync>;
pub type HttpSource = Arc<dyn Fn() -> reqwest::Client + Send + Sync>;

pub struct McpService {
    me: Weak<McpService>,
    config_dir: PathBuf,
    env: Env,
    config: Mutex<McpConfig>,
    servers: Mutex<HashMap<String, Entry>>,
    /// Serializes starts per server so two first calls cannot spawn two processes.
    starting: tokio::sync::Mutex<()>,
    /// Catalogs persisted across engine runs, by `name:revision`.
    cache_path: PathBuf,
    credentials: Arc<dyn CredentialStore>,
    http: Mutex<Option<HttpSource>>,
    listener: Mutex<Option<ChangeListener>>,
    elicitor: Mutex<Option<Arc<dyn Elicitor>>>,
}

impl McpService {
    pub fn new(config_dir: PathBuf, data_dir: PathBuf, env: Env) -> Arc<McpService> {
        Arc::new_cyclic(|me| McpService {
            me: me.clone(),
            credentials: Arc::new(FileStore::new(config_dir.join("mcp-auth.json"))),
            config_dir,
            env,
            config: Mutex::new(McpConfig::default()),
            servers: Mutex::new(HashMap::new()),
            starting: tokio::sync::Mutex::new(()),
            cache_path: data_dir.join("mcp-catalog.json"),
            http: Mutex::new(None),
            listener: Mutex::new(None),
            elicitor: Mutex::new(None),
        })
    }

    /// Where HTTP transports and sign-in get their client (the engine's, with its policy).
    pub fn set_http(&self, source: HttpSource) {
        *self.http.lock().unwrap_or_else(|e| e.into_inner()) = Some(source);
    }

    /// Who presents servers' requests for input; without one, form elicitation is not
    /// advertised and every request is declined.
    pub fn set_elicitor(&self, elicitor: Arc<dyn Elicitor>) {
        *self.elicitor.lock().unwrap_or_else(|e| e.into_inner()) = Some(elicitor);
    }

    /// Called after any state a client may show changes.
    pub fn set_listener(&self, listener: ChangeListener) {
        *self.listener.lock().unwrap_or_else(|e| e.into_inner()) = Some(listener);
    }

    fn changed(&self) {
        let listener = self
            .listener
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(l) = listener {
            l();
        }
    }

    fn http(&self) -> reqwest::Client {
        let source = self.http.lock().unwrap_or_else(|e| e.into_inner()).clone();
        source.map(|s| s()).unwrap_or_default()
    }

    fn has_credential(&self, name: &str) -> bool {
        matches!(
            self.credentials.read(name),
            Ok(Some(StoredCredential::OAuth(_)))
        )
    }

    /// Apply (merged) settings. Servers whose revision changed, or that disappeared, are
    /// stopped; their catalogs are forgotten. Tool enablement follows without a restart.
    pub fn apply_config(&self, config: McpConfig) -> Vec<ServerHandle> {
        let mut to_stop = Vec::new();
        let mut changed = false;
        {
            let mut servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
            let names: Vec<String> = servers.keys().cloned().collect();
            for name in names {
                let keep = config
                    .server(&name)
                    .is_some_and(|c| c.revision == servers[&name].config.revision);
                if !keep {
                    changed = true;
                    if let Some(entry) = servers.remove(&name) {
                        if let Some(attempt) = entry.attempt {
                            attempt.cancel.cancel();
                        }
                        if let State::Ready { client, .. } = entry.state {
                            to_stop.push(ServerHandle(client));
                        }
                    }
                }
            }
            for server in &config.servers {
                if !servers.contains_key(&server.name) {
                    changed = true;
                    servers.insert(
                        server.name.clone(),
                        Entry {
                            config: server.clone(),
                            state: State::Stopped,
                            catalog: self.cached_catalog(server),
                            resources: None,
                            prompts: None,
                            capabilities: Vec::new(),
                            last_used: Instant::now(),
                            in_flight: 0,
                            auth: AuthProgress::default(),
                            attempt: None,
                            signed_in: self.has_credential(&server.name),
                        },
                    );
                } else if let Some(entry) = servers.get_mut(&server.name) {
                    if entry.config != *server {
                        changed = true;
                    }
                    entry.config = server.clone();
                }
            }
        }
        *self.config.lock().unwrap_or_else(|e| e.into_inner()) = config;
        if changed {
            self.changed();
        }
        to_stop
    }

    pub fn config(&self) -> McpConfig {
        self.config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn snapshots(&self) -> Vec<ServerSnapshot> {
        let servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<ServerSnapshot> = servers
            .values()
            .map(|e| {
                let (state, error) = match &e.state {
                    State::Stopped => ("stopped", None),
                    State::Connecting => ("connecting", None),
                    State::Ready { .. } => ("ready", None),
                    State::AuthRequired { .. } => ("authRequired", None),
                    State::Failed { reason, .. } => ("failed", Some(reason.clone())),
                };
                ServerSnapshot {
                    config: e.config.clone(),
                    state: state.into(),
                    error,
                    catalog: e.catalog.clone(),
                    resources: e.resources.clone(),
                    prompts: e.prompts.clone(),
                    capabilities: e.capabilities.clone(),
                    in_flight: e.in_flight,
                    auth: e.auth.clone(),
                    signed_in: e.signed_in,
                }
            })
            .collect();
        out.sort_by(|a, b| a.config.name.cmp(&b.config.name));
        out
    }

    fn cached_catalog(&self, server: &McpServerConfig) -> Option<Vec<ToolInfo>> {
        let text = std::fs::read_to_string(&self.cache_path).ok()?;
        let cache: BTreeMap<String, Vec<Value>> = serde_json::from_str(&text).ok()?;
        let key = format!("{}:{}", server.name, server.revision);
        let items = cache.get(&key)?;
        Some(
            items
                .iter()
                .filter_map(|v| {
                    Some(ToolInfo {
                        server: server.name.clone(),
                        name: v.get("name")?.as_str()?.to_owned(),
                        description: v.get("description")?.as_str()?.to_owned(),
                        input_schema: v.get("inputSchema")?.clone(),
                        annotations: v.get("annotations").cloned(),
                        revision: server.revision.clone(),
                    })
                })
                .collect(),
        )
    }

    fn persist_catalog(&self, server: &McpServerConfig, tools: &[ToolInfo]) {
        let mut cache: BTreeMap<String, Vec<Value>> = std::fs::read_to_string(&self.cache_path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        // Keep only the current revision of this server.
        cache.retain(|k, _| !k.starts_with(&format!("{}:", server.name)));
        cache.insert(
            format!("{}:{}", server.name, server.revision),
            tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "inputSchema": t.input_schema,
                        "annotations": t.annotations,
                    })
                })
                .collect(),
        );
        if let Ok(text) = serde_json::to_string(&cache) {
            let tmp = self.cache_path.with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, &self.cache_path);
            }
        }
    }

    fn server_config(&self, name: &str) -> Result<McpServerConfig, McpError> {
        let config = self.config.lock().unwrap_or_else(|e| e.into_inner());
        if config.servers.is_empty() {
            return Err(McpError::NotConfigured(
                "add servers under mcp.servers in settings.json".into(),
            ));
        }
        let server = config
            .server(name)
            .cloned()
            .ok_or_else(|| McpError::UnknownServer(name.to_owned()))?;
        if !server.enabled {
            return Err(McpError::Disabled(name.to_owned()));
        }
        Ok(server)
    }

    /// A bearer token for an HTTP server from its saved credential, refreshed when it is
    /// about to expire. `None` when nothing is saved.
    async fn bearer_for(&self, server: &McpServerConfig) -> Result<Option<String>, String> {
        let name = server.name.clone();
        let store = self.credentials.clone();
        let read = tokio::task::spawn_blocking(move || store.read(&name))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("saved credentials could not be read: {e}"))?;
        let Some(StoredCredential::OAuth(credential)) = read else {
            return Ok(None);
        };
        if credential.expires - crate::util::now_ms() > TOKEN_MARGIN_MS {
            return Ok(Some(format!("Bearer {}", credential.access)));
        }
        let Some(flow) = McpFlow::from_credential(&server.name, &credential) else {
            return Err("the saved sign-in is incomplete; sign in again".into());
        };
        let ctx = self.flow_ctx();
        let refreshed = tokio::time::timeout(REFRESH_TIMEOUT, flow.refresh(&ctx, &credential))
            .await
            .map_err(|_| "the token refresh timed out; sign in again".to_owned())?
            .map_err(|e| format!("{e}; sign in again"))?;
        self.save_credential(&server.name, refreshed.clone())
            .await?;
        Ok(Some(format!("Bearer {}", refreshed.access)))
    }

    async fn save_credential(&self, name: &str, credential: OAuthCredential) -> Result<(), String> {
        let store = self.credentials.clone();
        let name = name.to_owned();
        tokio::task::spawn_blocking(move || {
            let mut txn = store.begin()?;
            txn.set(&name, StoredCredential::OAuth(credential));
            txn.commit()
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("the sign-in could not be saved: {e}"))
    }

    fn flow_ctx(&self) -> FlowCtx {
        FlowCtx {
            http: self.http(),
            endpoints: Endpoints::from_env(&self.env),
            device_id: None,
            copilot_model_ids: Vec::new(),
        }
    }

    /// The ready client for `name`, starting the server when needed.
    async fn ensure_started(&self, name: &str) -> Result<Arc<Client>, McpError> {
        let server = self.server_config(name)?;
        // Fast path.
        {
            let mut servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = servers.get_mut(name) {
                match &entry.state {
                    State::Ready { client, .. } => {
                        entry.last_used = Instant::now();
                        return Ok(client.clone());
                    }
                    State::Failed { reason, until } if Instant::now() < *until => {
                        return Err(McpError::StartFailed {
                            server: name.to_owned(),
                            reason: format!("{reason} (retrying after a backoff)"),
                        });
                    }
                    State::AuthRequired { .. } => {
                        return Err(McpError::AuthRequired {
                            server: name.to_owned(),
                        });
                    }
                    _ => {}
                }
            }
        }
        let _start = self.starting.lock().await;
        // Someone else may have started it while we waited.
        {
            let servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(Entry {
                state: State::Ready { client, .. },
                ..
            }) = servers.get(name)
            {
                return Ok(client.clone());
            }
        }
        self.set_state(name, State::Connecting);
        let start_timeout = Duration::from_secs(self.config().start_timeout_secs);
        let dirty = Arc::new(AtomicBool::new(false));
        let resources_dirty = Arc::new(AtomicBool::new(false));
        let prompts_dirty = Arc::new(AtomicBool::new(false));
        let callers = Arc::new(Mutex::new(Vec::new()));
        let handler = Handler {
            server: name.to_owned(),
            dirty: dirty.clone(),
            resources_dirty: resources_dirty.clone(),
            prompts_dirty: prompts_dirty.clone(),
            callers: callers.clone(),
            elicitor: self
                .elicitor
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        };
        let served = match &server.transport {
            Transport::Stdio {
                command: exe,
                args,
                cwd,
                env,
            } => {
                let secrets = read_secrets(&self.config_dir);
                let env = match resolve_refs(name, "env", env, &|k| self.env.get(k), &secrets) {
                    Ok(env) => env,
                    Err(reason) => return Err(self.fail(name, reason)),
                };
                let mut command = tokio::process::Command::new(exe);
                command.args(args);
                command.env_clear();
                // A minimal, explicit environment: PATH and HOME from the engine, then the
                // server's.
                for inherited in ["PATH", "HOME", "LANG", "TMPDIR", "SHELL", "USER"] {
                    if let Some(v) = self.env.get(inherited) {
                        command.env(inherited, v);
                    }
                }
                for (k, v) in env {
                    command.env(k, v);
                }
                if let Some(cwd) = cwd {
                    command.current_dir(cwd);
                }
                let transport = match TokioChildProcess::builder(command)
                    .stderr(std::process::Stdio::null())
                    .spawn()
                {
                    Ok((transport, _)) => transport,
                    Err(e) => return Err(self.fail(name, format!("cannot spawn {exe}: {e}"))),
                };
                tokio::time::timeout(start_timeout, handler.serve(transport))
                    .await
                    .map(|r| r.map_err(|e| (auth_required(&e), e.to_string())))
            }
            Transport::Http { url, headers } => {
                let secrets = read_secrets(&self.config_dir);
                let headers =
                    match resolve_refs(name, "headers", headers, &|k| self.env.get(k), &secrets) {
                        Ok(h) => h,
                        Err(reason) => return Err(self.fail(name, reason)),
                    };
                let mut custom = HashMap::new();
                for (k, v) in headers {
                    match (
                        reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                        reqwest::header::HeaderValue::from_str(&v),
                    ) {
                        (Ok(name), Ok(value)) => {
                            custom.insert(name, value);
                        }
                        _ => return Err(self.fail(name, format!("header {k} is not valid"))),
                    }
                }
                let bearer = match self.bearer_for(&server).await {
                    Ok(b) => b,
                    Err(reason) => {
                        self.set_state(
                            name,
                            State::AuthRequired {
                                www_authenticate: None,
                            },
                        );
                        self.set_auth(name, |a| {
                            a.status = AuthStatus::Failed;
                            a.message = reason;
                        });
                        return Err(McpError::AuthRequired {
                            server: name.to_owned(),
                        });
                    }
                };
                let mut config = StreamableHttpClientTransportConfig::with_uri(url.clone());
                config.auth_header = bearer;
                config.custom_headers = custom;
                let transport = StreamableHttpClientTransport::with_client(
                    EngineHttp {
                        client: self.http(),
                    },
                    config,
                );
                tokio::time::timeout(start_timeout, handler.serve(transport))
                    .await
                    .map(|r| r.map_err(|e| (auth_required(&e), e.to_string())))
            }
        };
        let client = match served {
            Ok(Ok(client)) => Arc::new(client),
            Ok(Err((Some(www_authenticate), _))) => {
                self.set_state(
                    name,
                    State::AuthRequired {
                        www_authenticate: Some(www_authenticate),
                    },
                );
                return Err(McpError::AuthRequired {
                    server: name.to_owned(),
                });
            }
            Ok(Err((None, e))) => {
                return Err(self.fail(name, format!("initialization failed: {e}")));
            }
            Err(_) => {
                return Err(self.fail(
                    name,
                    format!(
                        "did not finish initializing within {} s",
                        start_timeout.as_secs()
                    ),
                ));
            }
        };
        let capabilities = client
            .peer_info()
            .map(|info| {
                let caps = &info.capabilities;
                let mut list = Vec::new();
                if caps.tools.is_some() {
                    list.push("tools".to_owned());
                }
                if let Some(r) = &caps.resources {
                    list.push("resources".to_owned());
                    if r.subscribe == Some(true) {
                        list.push("resourcesSubscribe".to_owned());
                    }
                }
                if caps.prompts.is_some() {
                    list.push("prompts".to_owned());
                }
                if caps.logging.is_some() {
                    list.push("logging".to_owned());
                }
                if caps.completions.is_some() {
                    list.push("completions".to_owned());
                }
                list
            })
            .unwrap_or_default();
        {
            let mut servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = servers.get_mut(name) {
                entry.capabilities = capabilities;
            }
        }
        self.set_state(
            name,
            State::Ready {
                client: client.clone(),
                dirty,
                resources_dirty,
                prompts_dirty,
                callers,
            },
        );
        Ok(client)
    }

    fn set_state(&self, name: &str, state: State) {
        {
            let mut servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = servers.get_mut(name) {
                entry.state = state;
                entry.last_used = Instant::now();
            }
        }
        self.changed();
    }

    fn set_auth(&self, name: &str, edit: impl FnOnce(&mut AuthProgress)) {
        {
            let mut servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = servers.get_mut(name) {
                edit(&mut entry.auth);
                entry.signed_in = self.has_credential(name);
            }
        }
        self.changed();
    }

    fn fail(&self, name: &str, reason: String) -> McpError {
        log::warn!("mcp server {name}: {reason}");
        self.set_state(
            name,
            State::Failed {
                reason: reason.clone(),
                until: Instant::now() + START_BACKOFF,
            },
        );
        McpError::StartFailed {
            server: name.to_owned(),
            reason,
        }
    }

    /// The server's tools: cached unless the server reported a change or has never been
    /// listed under this revision. Disabled tools are included (callers filter).
    pub async fn catalog(&self, name: &str) -> Result<Vec<ToolInfo>, McpError> {
        let (cached, dirty) = {
            let servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
            let entry = servers.get(name);
            let dirty = entry.is_some_and(|e| match &e.state {
                State::Ready { dirty, .. } => dirty.swap(false, Ordering::SeqCst),
                _ => false,
            });
            (entry.and_then(|e| e.catalog.clone()), dirty)
        };
        if let Some(tools) = cached
            && !dirty
        {
            return Ok(tools);
        }
        let server = self.server_config(name)?;
        let client = self.ensure_started(name).await?;
        self.list_into_catalog(&server, &client).await
    }

    async fn list_into_catalog(
        &self,
        server: &McpServerConfig,
        client: &Client,
    ) -> Result<Vec<ToolInfo>, McpError> {
        let name = server.name.as_str();
        let listed = client
            .list_all_tools()
            .await
            .map_err(|e| self.transport_error(name, &e))?;
        let mut tools: Vec<ToolInfo> = listed
            .into_iter()
            .take(MAX_TOOLS_PER_SERVER)
            .map(|t| {
                let mut schema = Value::Object((*t.input_schema).clone());
                if serde_json::to_string(&schema).map(|s| s.len()).unwrap_or(0) > MAX_SCHEMA_BYTES {
                    schema = serde_json::json!({"type": "object", "description": "schema too large to cache; see the server's documentation"});
                }
                ToolInfo {
                    server: name.to_owned(),
                    name: t.name.to_string(),
                    description: t
                        .description
                        .as_deref()
                        .unwrap_or("")
                        .chars()
                        .take(MAX_DESCRIPTION_CHARS)
                        .collect(),
                    input_schema: schema,
                    annotations: t.annotations.and_then(|a| serde_json::to_value(a).ok()),
                    revision: server.revision.clone(),
                }
            })
            .collect();
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        // Duplicate names within one server are a server bug; keep the first.
        tools.dedup_by(|a, b| a.name == b.name);
        self.persist_catalog(server, &tools);
        {
            let mut servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = servers.get_mut(name) {
                entry.catalog = Some(tools.clone());
            }
        }
        self.changed();
        Ok(tools)
    }

    fn transport_error(&self, name: &str, error: &rmcp::service::ServiceError) -> McpError {
        // A broken transport means the process is gone or unusable: stop tracking it.
        let dropped = {
            let mut servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
            servers.get_mut(name).and_then(|entry| {
                if matches!(entry.state, State::Ready { .. }) {
                    let old = std::mem::replace(&mut entry.state, State::Stopped);
                    if let State::Ready { client, .. } = old {
                        return Some(client);
                    }
                }
                None
            })
        };
        if let Some(client) = dropped {
            tokio::spawn(async move { ServerHandle(client).stop().await });
        }
        if let Some(www_authenticate) = auth_required(error) {
            self.set_state(
                name,
                State::AuthRequired {
                    www_authenticate: Some(www_authenticate),
                },
            );
            return McpError::AuthRequired {
                server: name.to_owned(),
            };
        }
        self.changed();
        McpError::Transport {
            server: name.to_owned(),
            reason: error.to_string(),
        }
    }

    /// Search tool names and descriptions across enabled servers (or one server), starting
    /// servers that have no cached catalog yet. Start failures are reported per server.
    /// Disabled tools are left out.
    pub async fn search(
        &self,
        query: &str,
        server: Option<&str>,
    ) -> Result<(Vec<ToolInfo>, Vec<(String, String)>), McpError> {
        let config = self.config();
        if config.servers.is_empty() {
            return Err(McpError::NotConfigured(
                "add servers under mcp.servers in settings.json".into(),
            ));
        }
        let names: Vec<String> = match server {
            Some(name) => {
                self.server_config(name)?;
                vec![name.to_owned()]
            }
            None => config
                .servers
                .iter()
                .filter(|s| s.enabled)
                .map(|s| s.name.clone())
                .collect(),
        };
        let needle = query.trim().to_lowercase();
        let mut found = Vec::new();
        let mut problems = Vec::new();
        for name in names {
            let enabled = config
                .server(&name)
                .map(|s| s.disabled_tools.clone())
                .unwrap_or_default();
            match self.catalog(&name).await {
                Ok(tools) => {
                    for t in tools {
                        if enabled.contains(&t.name) {
                            continue;
                        }
                        if needle.is_empty()
                            || t.name.to_lowercase().contains(&needle)
                            || t.description.to_lowercase().contains(&needle)
                        {
                            found.push(t);
                        }
                    }
                }
                Err(e) => problems.push((name, e.to_string())),
            }
        }
        found.truncate(MAX_SEARCH_RESULTS);
        Ok((found, problems))
    }

    /// One tool by `server/name`; a disabled tool is refused.
    pub async fn describe(&self, qualified: &str) -> Result<ToolInfo, McpError> {
        let (server, tool) = split_qualified(qualified)?;
        let config = self.server_config(&server)?;
        let tools = self.catalog(&server).await?;
        let info =
            tools
                .into_iter()
                .find(|t| t.name == tool)
                .ok_or_else(|| McpError::UnknownTool {
                    server: server.clone(),
                    tool: tool.clone(),
                })?;
        if !config.tool_enabled(&tool) {
            return Err(McpError::ToolDisabled { server, tool });
        }
        Ok(info)
    }

    /// Call a tool: arguments validated against the cached schema first, then one request
    /// with the configured timeout, cancellable. A lost reply is reported as unknown effect.
    pub async fn call(
        &self,
        session: &str,
        qualified: &str,
        arguments: Value,
        cancel: CancellationToken,
    ) -> Result<CallOutcome, McpError> {
        let tool = self.describe(qualified).await?;
        let arguments = match arguments {
            Value::Null => Value::Object(Map::new()),
            other => other,
        };
        let validated =
            crate::tools::schema::validate(&tool.qualified(), &tool.input_schema, &arguments)
                .map_err(McpError::InvalidArguments)?;
        let Value::Object(args) = validated else {
            return Err(McpError::InvalidArguments(
                "arguments must be an object".into(),
            ));
        };
        let client = self.ensure_started(&tool.server).await?;
        let timeout_secs = self.config().call_timeout_secs;
        self.adjust_in_flight(&tool.server, 1);
        let callers = self.callers_of(&tool.server);
        if let Some(c) = &callers {
            c.lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((session.to_owned(), cancel.clone()));
        }
        let request = CallToolRequestParams::new(tool.name.clone()).with_arguments(args);
        let call = client.call_tool(request);
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(McpError::Cancelled),
            r = tokio::time::timeout(Duration::from_secs(timeout_secs), call) => match r {
                Ok(Ok(result)) => Ok(result),
                Ok(Err(e)) => Err(self.transport_error(&tool.server, &e)),
                Err(_) => Err(McpError::Timeout { server: tool.server.clone(), tool: tool.name.clone(), secs: timeout_secs }),
            },
        };
        self.adjust_in_flight(&tool.server, -1);
        if let Some(c) = &callers {
            let mut c = c.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(pos) = c.iter().rposition(|(s, _)| s == session) {
                c.remove(pos);
            }
        }
        let result = result?;
        let content = result
            .content
            .into_iter()
            .map(|block| match block {
                ContentBlock::Text(text) => pipkin_engine_protocol::Block::text(text.text.clone()),
                ContentBlock::Image(image) => pipkin_engine_protocol::Block::text(format!(
                    "[image {} ({} bytes of base64)]",
                    image.mime_type,
                    image.data.len()
                )),
                other => pipkin_engine_protocol::Block::text(
                    serde_json::to_string(&other).unwrap_or_else(|_| "[content]".into()),
                ),
            })
            .collect();
        Ok(CallOutcome {
            content,
            structured: result.structured_content,
            is_error: result.is_error.unwrap_or(false),
            tool,
        })
    }

    fn callers_of(&self, name: &str) -> Option<Callers> {
        let servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
        match servers.get(name).map(|e| &e.state) {
            Some(State::Ready { callers, .. }) => Some(callers.clone()),
            _ => None,
        }
    }

    fn capability_check(&self, name: &str, feature: &str) -> Result<(), McpError> {
        let servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
        let entry = servers
            .get(name)
            .ok_or_else(|| McpError::UnknownServer(name.to_owned()))?;
        if entry.capabilities.iter().any(|c| c == feature) {
            Ok(())
        } else {
            Err(McpError::Unsupported {
                server: name.to_owned(),
                feature: feature.to_owned(),
            })
        }
    }

    /// The server's resources and resource templates, cached until it reports a change.
    pub async fn resources(&self, name: &str) -> Result<Vec<ResourceInfo>, McpError> {
        let (cached, dirty) = {
            let servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
            let entry = servers.get(name);
            let dirty = entry.is_some_and(|e| match &e.state {
                State::Ready {
                    resources_dirty, ..
                } => resources_dirty.swap(false, Ordering::SeqCst),
                _ => false,
            });
            (entry.and_then(|e| e.resources.clone()), dirty)
        };
        if let Some(list) = cached
            && !dirty
        {
            return Ok(list);
        }
        self.server_config(name)?;
        let client = self.ensure_started(name).await?;
        self.capability_check(name, "resources")?;
        let listed = client
            .list_all_resources()
            .await
            .map_err(|e| self.transport_error(name, &e))?;
        let templates = client
            .list_all_resource_templates()
            .await
            .map_err(|e| self.transport_error(name, &e))?;
        let cut = |s: Option<String>| -> String {
            s.unwrap_or_default()
                .chars()
                .take(MAX_DESCRIPTION_CHARS)
                .collect()
        };
        let mut list: Vec<ResourceInfo> = listed
            .into_iter()
            .take(MAX_TOOLS_PER_SERVER)
            .map(|r| ResourceInfo {
                server: name.to_owned(),
                uri: r.uri,
                name: r.title.unwrap_or(r.name),
                description: cut(r.description),
                mime_type: r.mime_type,
                size: r.size,
                template: false,
            })
            .collect();
        list.extend(
            templates
                .into_iter()
                .take(MAX_TOOLS_PER_SERVER)
                .map(|t| ResourceInfo {
                    server: name.to_owned(),
                    uri: t.uri_template,
                    name: t.title.unwrap_or(t.name),
                    description: cut(t.description),
                    mime_type: t.mime_type,
                    size: None,
                    template: true,
                }),
        );
        {
            let mut servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = servers.get_mut(name) {
                entry.resources = Some(list.clone());
            }
        }
        self.changed();
        Ok(list)
    }

    /// Read one resource. Text parts are kept; blobs are reported by decoded size only.
    pub async fn read_resource(
        &self,
        name: &str,
        uri: &str,
    ) -> Result<Vec<ResourcePart>, McpError> {
        self.server_config(name)?;
        let client = self.ensure_started(name).await?;
        self.capability_check(name, "resources")?;
        let timeout_secs = self.config().call_timeout_secs;
        let read = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            client.read_resource(ReadResourceRequestParams::new(uri)),
        )
        .await
        .map_err(|_| McpError::Timeout {
            server: name.to_owned(),
            tool: format!("read {uri}"),
            secs: timeout_secs,
        })?
        .map_err(|e| match &e {
            rmcp::service::ServiceError::McpError(m) => {
                McpError::InvalidArguments(format!("{name} could not read {uri}: {}", m.message))
            }
            other => self.transport_error(name, other),
        })?;
        Ok(read
            .contents
            .into_iter()
            .map(|part| match part {
                ResourceContents::TextResourceContents {
                    uri,
                    mime_type,
                    text,
                    ..
                } => ResourcePart {
                    uri,
                    mime_type,
                    text: Some(text),
                    blob_bytes: None,
                },
                ResourceContents::BlobResourceContents {
                    uri,
                    mime_type,
                    blob,
                    ..
                } => ResourcePart {
                    uri,
                    mime_type,
                    text: None,
                    blob_bytes: Some(blob.len() * 3 / 4),
                },
                _ => ResourcePart {
                    uri: uri.to_owned(),
                    mime_type: None,
                    text: Some("[content of a kind this engine does not show]".into()),
                    blob_bytes: None,
                },
            })
            .collect())
    }

    /// The server's prompt templates, cached until it reports a change.
    pub async fn prompts(&self, name: &str) -> Result<Vec<PromptInfo>, McpError> {
        let (cached, dirty) = {
            let servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
            let entry = servers.get(name);
            let dirty = entry.is_some_and(|e| match &e.state {
                State::Ready { prompts_dirty, .. } => prompts_dirty.swap(false, Ordering::SeqCst),
                _ => false,
            });
            (entry.and_then(|e| e.prompts.clone()), dirty)
        };
        if let Some(list) = cached
            && !dirty
        {
            return Ok(list);
        }
        self.server_config(name)?;
        let client = self.ensure_started(name).await?;
        self.capability_check(name, "prompts")?;
        let listed = client
            .list_all_prompts()
            .await
            .map_err(|e| self.transport_error(name, &e))?;
        let list: Vec<PromptInfo> = listed
            .into_iter()
            .take(MAX_TOOLS_PER_SERVER)
            .map(|p| PromptInfo {
                server: name.to_owned(),
                name: p.name,
                description: p
                    .description
                    .unwrap_or_default()
                    .chars()
                    .take(MAX_DESCRIPTION_CHARS)
                    .collect(),
                arguments: p
                    .arguments
                    .unwrap_or_default()
                    .into_iter()
                    .map(|a| PromptArgumentInfo {
                        name: a.name,
                        description: a.description.unwrap_or_default(),
                        required: a.required.unwrap_or(false),
                    })
                    .collect(),
            })
            .collect();
        {
            let mut servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = servers.get_mut(name) {
                entry.prompts = Some(list.clone());
            }
        }
        self.changed();
        Ok(list)
    }

    /// Render a prompt template with arguments.
    pub async fn get_prompt(
        &self,
        name: &str,
        prompt: &str,
        arguments: Map<String, Value>,
    ) -> Result<PromptOutcome, McpError> {
        let known = self.prompts(name).await?;
        let info =
            known
                .iter()
                .find(|p| p.name == prompt)
                .ok_or_else(|| McpError::UnknownTool {
                    server: name.to_owned(),
                    tool: prompt.to_owned(),
                })?;
        if let Some(missing) = info
            .arguments
            .iter()
            .find(|a| a.required && !arguments.contains_key(&a.name))
        {
            return Err(McpError::InvalidArguments(format!(
                "prompt {name}/{prompt} needs the argument {:?}",
                missing.name
            )));
        }
        let client = self.ensure_started(name).await?;
        let timeout_secs = self.config().call_timeout_secs;
        let request = GetPromptRequestParams::new(prompt).with_arguments(arguments);
        let got = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            client.get_prompt(request),
        )
        .await
        .map_err(|_| McpError::Timeout {
            server: name.to_owned(),
            tool: format!("prompt {prompt}"),
            secs: timeout_secs,
        })?
        .map_err(|e| match &e {
            rmcp::service::ServiceError::McpError(m) => McpError::InvalidArguments(format!(
                "{name} could not render {prompt}: {}",
                m.message
            )),
            other => self.transport_error(name, other),
        })?;
        Ok(PromptOutcome {
            description: got.description,
            messages: got
                .messages
                .into_iter()
                .map(|m| {
                    let role = match m.role {
                        rmcp::model::Role::User => "user",
                        rmcp::model::Role::Assistant => "assistant",
                    }
                    .to_owned();
                    let text = match m.content {
                        ContentBlock::Text(t) => t.text.clone(),
                        ContentBlock::Image(i) => {
                            format!("[image {} ({} bytes of base64)]", i.mime_type, i.data.len())
                        }
                        other => {
                            serde_json::to_string(&other).unwrap_or_else(|_| "[content]".into())
                        }
                    };
                    (role, text)
                })
                .collect(),
        })
    }

    fn adjust_in_flight(&self, name: &str, delta: i64) {
        let mut servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = servers.get_mut(name) {
            entry.in_flight = (entry.in_flight as i64 + delta).max(0) as usize;
            entry.last_used = Instant::now();
        }
    }

    /// Stop servers idle beyond their `idleSeconds` with nothing in flight (unless kept
    /// alive). Returns the clients to cancel, which the caller awaits off any lock.
    pub fn reap_idle(&self, now: Instant) -> Vec<(String, ServerHandle)> {
        let mut reaped = Vec::new();
        {
            let mut servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
            for (name, entry) in servers.iter_mut() {
                if entry.config.keep_alive || entry.in_flight > 0 {
                    continue;
                }
                let idle = now.saturating_duration_since(entry.last_used)
                    >= Duration::from_secs(entry.config.idle_seconds);
                if idle && matches!(entry.state, State::Ready { .. }) {
                    let old = std::mem::replace(&mut entry.state, State::Stopped);
                    if let State::Ready { client, .. } = old {
                        reaped.push((name.clone(), ServerHandle(client)));
                    }
                }
            }
        }
        if !reaped.is_empty() {
            self.changed();
        }
        reaped
    }

    /// Stop one server now. A failed or auth-required state is kept unless `reset`.
    pub fn stop(&self, name: &str, reset: bool) -> Option<ServerHandle> {
        let handle = {
            let mut servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
            let entry = servers.get_mut(name)?;
            let keep = !reset
                && matches!(
                    entry.state,
                    State::Failed { .. } | State::AuthRequired { .. }
                );
            if keep {
                return None;
            }
            let old = std::mem::replace(&mut entry.state, State::Stopped);
            match old {
                State::Ready { client, .. } => Some(ServerHandle(client)),
                _ => None,
            }
        };
        self.changed();
        handle
    }

    /// Every running client, for shutdown.
    pub fn drain(&self) -> Vec<ServerHandle> {
        let mut servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
        servers
            .values_mut()
            .filter_map(|entry| {
                if let Some(attempt) = entry.attempt.take() {
                    attempt.cancel.cancel();
                }
                let old = std::mem::replace(&mut entry.state, State::Stopped);
                match old {
                    State::Ready { client, .. } => Some(ServerHandle(client)),
                    _ => None,
                }
            })
            .collect()
    }

    /// Connect afresh and list the tools: the Connections page's "Test". A failed or
    /// auth-required state is cleared first so the result reflects now.
    pub async fn test(&self, name: &str) -> Result<TestOutcome, McpError> {
        if let Some(handle) = self.stop(name, true) {
            handle.stop().await;
        }
        let server = self.server_config(name)?;
        let started = Instant::now();
        let client = self.ensure_started(name).await?;
        let tools = self.list_into_catalog(&server, &client).await?;
        let info = client.peer_info();
        let implementation = info.as_ref().and_then(|i| i.server_info.clone());
        Ok(TestOutcome {
            tools: tools.into_iter().map(|t| t.name).collect(),
            server_name: implementation.as_ref().map(|i| i.name.clone()),
            server_version: implementation.as_ref().map(|i| i.version.clone()),
            protocol_version: info.as_ref().map(|i| i.protocol_version.to_string()),
            latency_ms: started.elapsed().as_millis() as u64,
        })
    }

    /// Stop and start again, clearing backoff.
    pub async fn reconnect(&self, name: &str) -> Result<(), McpError> {
        if let Some(handle) = self.stop(name, true) {
            handle.stop().await;
        }
        self.ensure_started(name).await.map(|_| ())
    }

    // ------------------------------------------------------------------ sign-in

    pub fn auth_cancel(&self, name: &str) {
        let attempt = {
            let mut servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
            servers.get_mut(name).and_then(|e| e.attempt.take())
        };
        if let Some(a) = attempt {
            a.cancel.cancel();
            self.set_auth(name, |p| {
                p.status = AuthStatus::Failed;
                p.message = "Sign-in cancelled".into();
                p.url = None;
                p.prompt = None;
            });
        }
    }

    /// Answer the sign-in's open prompt (a pasted code or redirect URL).
    pub fn auth_answer(&self, name: &str, prompt_id: &str, response: String) -> Result<(), String> {
        let sender = {
            let mut servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
            let entry = servers.get_mut(name).ok_or("no such server")?;
            let attempt = entry.attempt.as_mut().ok_or("no sign-in is in progress")?;
            match attempt.pending.take() {
                Some((id, tx)) if id == prompt_id => tx,
                Some(other) => {
                    attempt.pending = Some(other);
                    return Err("that prompt is no longer open".into());
                }
                None => return Err("that prompt is no longer open".into()),
            }
        };
        sender
            .send(response)
            .map_err(|_| "that prompt is no longer open".to_owned())?;
        self.set_auth(name, |p| {
            p.status = AuthStatus::Waiting;
            p.prompt = None;
            p.message = "Finishing sign-in…".into();
        });
        Ok(())
    }

    /// Forget the saved tokens and stop the server; the next use needs a new sign-in.
    pub async fn auth_revoke(&self, name: &str) -> Result<(), String> {
        self.auth_cancel(name);
        if let Some(handle) = self.stop(name, true) {
            handle.stop().await;
        }
        let store = self.credentials.clone();
        let n = name.to_owned();
        tokio::task::spawn_blocking(move || {
            let mut txn = store.begin()?;
            txn.remove(&n);
            txn.commit()
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("the saved sign-in could not be removed: {e}"))?;
        self.set_auth(name, |p| *p = AuthProgress::default());
        Ok(())
    }

    /// Start a sign-in for an HTTP server. Progress is read from snapshots; the result is
    /// a saved credential and a server ready to start on next use.
    pub fn auth_start(&self, name: &str) -> Result<(), McpError> {
        let server = self.server_config(name)?;
        let Transport::Http { url, headers } = server.transport.clone() else {
            return Err(McpError::InvalidArguments(format!(
                "{name} runs over stdio and has no sign-in"
            )));
        };
        self.auth_cancel(name);
        let cancel = CancellationToken::new();
        let www_authenticate = {
            let mut servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
            let entry = servers
                .get_mut(name)
                .ok_or_else(|| McpError::UnknownServer(name.to_owned()))?;
            entry.attempt = Some(Attempt {
                cancel: cancel.clone(),
                pending: None,
            });
            match &entry.state {
                State::AuthRequired { www_authenticate } => www_authenticate.clone(),
                _ => None,
            }
        };
        let stop = self.stop(name, true);
        self.set_auth(name, |p| {
            *p = AuthProgress {
                status: AuthStatus::Connecting,
                message: "Finding the authorization server…".into(),
                url: None,
                prompt: None,
            }
        });
        let Some(me) = self.me.upgrade() else {
            return Err(McpError::Cancelled);
        };
        let name = name.to_owned();
        tokio::spawn(async move {
            if let Some(handle) = stop {
                handle.stop().await;
            }
            let result = me
                .run_sign_in(
                    &name,
                    &server,
                    &url,
                    &headers,
                    www_authenticate,
                    cancel.clone(),
                )
                .await;
            {
                let mut servers = me.servers.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(entry) = servers.get_mut(&name) {
                    entry.attempt = None;
                }
            }
            match result {
                Ok(()) if !cancel.is_cancelled() => {
                    me.set_state(&name, State::Stopped);
                    me.set_auth(&name, |p| {
                        *p = AuthProgress {
                            status: AuthStatus::Done,
                            message: "Signed in".into(),
                            url: None,
                            prompt: None,
                        }
                    });
                }
                Ok(()) => {}
                // A cancellation already set its own message.
                Err(_) if cancel.is_cancelled() => {}
                Err(reason) => {
                    // The reason is user-facing and never carries tokens.
                    log::info!("mcp sign-in for {name} did not complete");
                    me.set_auth(&name, |p| {
                        p.status = AuthStatus::Failed;
                        p.message = reason;
                        p.url = None;
                        p.prompt = None;
                    });
                }
            }
        });
        Ok(())
    }

    async fn run_sign_in(
        self: &Arc<Self>,
        name: &str,
        server: &McpServerConfig,
        url: &str,
        headers: &[(String, super::config::EnvValue)],
        www_authenticate: Option<String>,
        cancel: CancellationToken,
    ) -> Result<(), String> {
        let http = self.http();
        let secrets = read_secrets(&self.config_dir);
        let www_authenticate = match www_authenticate {
            Some(h) => Some(h),
            None => {
                // Ask the server what it wants.
                let resolved =
                    resolve_refs(name, "headers", headers, &|k| self.env.get(k), &secrets)?;
                probe_challenge(&http, url, &resolved, &cancel).await
            }
        };
        let discovered = discover(&http, url, www_authenticate.as_deref(), Some(&cancel)).await?;
        if cancel.is_cancelled() {
            return Err(CANCELLED.into());
        }
        // The client: configured, or registered for this redirect URI.
        let endpoints = Endpoints::from_env(&self.env);
        let port = free_port(&endpoints.callback_host)?;
        let redirect_uri = format!(
            "http://{}:{port}/mcp/callback/{}",
            endpoints.callback_host,
            crate::util::uuid_v4()
        );
        let (client_id, client_secret) = match &server.oauth.client_id {
            Some(id) => (
                resolve_one(name, "oauth.clientId", id, &|k| self.env.get(k), &secrets)?,
                server
                    .oauth
                    .client_secret
                    .as_ref()
                    .map(|s| {
                        resolve_one(
                            name,
                            "oauth.clientSecret",
                            s,
                            &|k| self.env.get(k),
                            &secrets,
                        )
                    })
                    .transpose()?,
            ),
            None => {
                let endpoint = discovered.registration_endpoint.clone().ok_or_else(|| {
                    "the authorization server does not register clients; set oauth.clientId in settings"
                        .to_owned()
                })?;
                self.set_auth(name, |p| {
                    p.message = "Registering this app with the authorization server…".into()
                });
                register(&http, &endpoint, &redirect_uri, Some(&cancel)).await?
            }
        };
        let flow = McpFlow {
            server: name.to_owned(),
            discovered,
            client_id,
            client_secret,
            scopes: server.oauth.scopes.clone(),
            redirect_uri: Some(redirect_uri),
        };
        let ctx = FlowCtx {
            http,
            endpoints,
            device_id: None,
            copilot_model_ids: Vec::new(),
        };
        let io = Interaction::new(
            Arc::new(AttemptSink {
                service: Arc::downgrade(self),
                server: name.to_owned(),
            }),
            cancel.clone(),
        );
        self.set_auth(name, |p| {
            p.status = AuthStatus::Waiting;
            p.message = "Waiting for the browser…".into();
        });
        let credential = tokio::select! {
            r = flow.login(&ctx, &io) => r,
            _ = cancel.cancelled() => Err(CANCELLED.into()),
        }?;
        drop(io);
        self.save_credential(name, credential).await
    }
}

/// A free loopback port for the registered redirect URI (bound, read, released).
fn free_port(host: &str) -> Result<u16, String> {
    let listener = std::net::TcpListener::bind((host, 0))
        .map_err(|e| format!("cannot listen for the sign-in callback: {e}"))?;
    listener
        .local_addr()
        .map(|a| a.port())
        .map_err(|e| e.to_string())
}

/// POST an `initialize` without a token and return the challenge if the server wants one.
async fn probe_challenge(
    http: &reqwest::Client,
    url: &str,
    headers: &[(String, String)],
    cancel: &CancellationToken,
) -> Option<String> {
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "pipkin", "version": env!("CARGO_PKG_VERSION")}}
    });
    let mut req = http
        .post(url)
        .header("accept", "application/json, text/event-stream")
        .json(&body)
        .timeout(Duration::from_secs(15));
    for (k, v) in headers {
        req = req.header(k, v);
    }
    let resp = tokio::select! {
        r = req.send() => r.ok()?,
        _ = cancel.cancelled() => return None,
    };
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Some(
            resp.headers()
                .get(reqwest::header::WWW_AUTHENTICATE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_owned(),
        );
    }
    None
}

/// Routes a sign-in's prompts and progress into the server's snapshot.
struct AttemptSink {
    service: Weak<McpService>,
    server: String,
}

impl InteractionSink for AttemptSink {
    fn prompt(&self, prompt: Prompt) -> BoxFuture<'static, Result<String, String>> {
        let (tx, rx) = oneshot::channel();
        let id = crate::util::uuid_v4();
        if let Some(service) = self.service.upgrade() {
            {
                let mut servers = service.servers.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(attempt) = servers
                    .get_mut(&self.server)
                    .and_then(|e| e.attempt.as_mut())
                {
                    attempt.pending = Some((id.clone(), tx));
                }
            }
            service.set_auth(&self.server, |p| {
                p.status = AuthStatus::Prompt;
                p.prompt = Some(PromptView {
                    id,
                    kind: prompt.kind.as_str().to_owned(),
                    message: prompt.message.clone(),
                    placeholder: prompt.placeholder.clone(),
                    options: prompt.options.clone(),
                });
            });
        }
        Box::pin(async move { rx.await.map_err(|_| CANCELLED.to_owned()) })
    }

    fn notify(&self, event: AuthEvent) {
        let Some(service) = self.service.upgrade() else {
            return;
        };
        service.set_auth(&self.server, |p| match event {
            AuthEvent::AuthUrl { url, instructions } => {
                p.status = AuthStatus::Waiting;
                p.url = Some(url);
                if let Some(i) = instructions {
                    p.message = i;
                }
            }
            AuthEvent::DeviceCode {
                user_code,
                verification_uri,
            } => {
                p.status = AuthStatus::Waiting;
                p.url = Some(verification_uri);
                p.message = format!("Enter the code {user_code}");
            }
            AuthEvent::Info(m) | AuthEvent::Progress(m) => p.message = m,
        });
    }
}

/// `server/tool` into its parts.
pub fn split_qualified(qualified: &str) -> Result<(String, String), McpError> {
    match qualified.trim().split_once('/') {
        Some((server, tool)) if !server.is_empty() && !tool.is_empty() => {
            Ok((server.to_owned(), tool.to_owned()))
        }
        _ => Err(McpError::InvalidArguments(format!(
            "tool must be written as server/tool, not {qualified:?}"
        ))),
    }
}
