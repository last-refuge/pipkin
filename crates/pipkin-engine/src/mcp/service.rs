//! The MCP service: supervised server processes, cached catalogs, bounded calls.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rmcp::model::{CallToolRequestParams, ClientConfig, ContentBlock, Implementation};
use rmcp::service::RunningService;
use rmcp::transport::child_process::TokioChildProcess;
use rmcp::{ClientHandler, RoleClient, ServiceExt};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use super::config::{McpConfig, McpServerConfig, read_secrets, resolve_env};

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

/// What a client sees of a server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerSnapshot {
    pub name: String,
    pub scope: String,
    pub enabled: bool,
    /// `stopped`, `connecting`, `ready`, `failed`.
    pub state: String,
    pub error: Option<String>,
    pub tools: Option<usize>,
    pub revision: String,
    pub keep_alive: bool,
    pub idle_seconds: u64,
    pub in_flight: usize,
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
    UnknownTool {
        server: String,
        tool: String,
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
            McpError::UnknownTool { .. } => "unknown_tool",
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
            McpError::UnknownTool { server, tool } => {
                write!(f, "MCP server {server} has no tool {tool:?}")
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
    dirty: Arc<AtomicBool>,
}

impl ClientHandler for Handler {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::new(
            rmcp::model::ClientCapabilities::default(),
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
}

/// How the service reads the engine's environment (never the process environment directly).
pub type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

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
    },
    Failed {
        reason: String,
        until: Instant,
    },
}

struct Entry {
    config: McpServerConfig,
    state: State,
    catalog: Option<Vec<ToolInfo>>,
    last_used: Instant,
    in_flight: usize,
}

pub struct McpService {
    config_dir: PathBuf,
    env: EnvLookup,
    config: Mutex<McpConfig>,
    servers: Mutex<HashMap<String, Entry>>,
    /// Serializes starts per server so two first calls cannot spawn two processes.
    starting: tokio::sync::Mutex<()>,
    /// Catalogs persisted across engine runs, by `name:revision`.
    cache_path: PathBuf,
}

impl McpService {
    pub fn new(config_dir: PathBuf, data_dir: PathBuf, env: EnvLookup) -> McpService {
        McpService {
            config_dir,
            env,
            config: Mutex::new(McpConfig::default()),
            servers: Mutex::new(HashMap::new()),
            starting: tokio::sync::Mutex::new(()),
            cache_path: data_dir.join("mcp-catalog.json"),
        }
    }

    /// Apply (merged) settings. Servers whose revision changed, or that disappeared, are
    /// stopped; their catalogs are forgotten.
    pub fn apply_config(&self, config: McpConfig) -> Vec<ServerHandle> {
        let mut to_stop = Vec::new();
        let mut servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
        let names: Vec<String> = servers.keys().cloned().collect();
        for name in names {
            let keep = config
                .server(&name)
                .is_some_and(|c| c.revision == servers[&name].config.revision);
            if !keep
                && let Some(entry) = servers.remove(&name)
                && let State::Ready { client, .. } = entry.state
            {
                to_stop.push(ServerHandle(client));
            }
        }
        for server in &config.servers {
            servers.entry(server.name.clone()).or_insert_with(|| Entry {
                config: server.clone(),
                state: State::Stopped,
                catalog: self.cached_catalog(server),
                last_used: Instant::now(),
                in_flight: 0,
            });
            if let Some(entry) = servers.get_mut(&server.name) {
                entry.config = server.clone();
            }
        }
        *self.config.lock().unwrap_or_else(|e| e.into_inner()) = config;
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
                    State::Failed { reason, .. } => ("failed", Some(reason.clone())),
                };
                ServerSnapshot {
                    name: e.config.name.clone(),
                    scope: e.config.scope.clone(),
                    enabled: e.config.enabled,
                    state: state.into(),
                    error,
                    tools: e.catalog.as_ref().map(Vec::len),
                    revision: e.config.revision.clone(),
                    keep_alive: e.config.keep_alive,
                    idle_seconds: e.config.idle_seconds,
                    in_flight: e.in_flight,
                }
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
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
        let secrets = read_secrets(&self.config_dir);
        let env = match resolve_env(&server, &|k| (self.env)(k), &secrets) {
            Ok(env) => env,
            Err(reason) => {
                self.fail(name, reason.clone());
                return Err(McpError::StartFailed {
                    server: name.to_owned(),
                    reason,
                });
            }
        };
        let start_timeout = Duration::from_secs(self.config().start_timeout_secs);
        let mut command = tokio::process::Command::new(&server.command);
        command.args(&server.args);
        command.env_clear();
        // A minimal, explicit environment: PATH and HOME from the engine, then the server's.
        for inherited in ["PATH", "HOME", "LANG", "TMPDIR", "SHELL", "USER"] {
            if let Some(v) = (self.env)(inherited) {
                command.env(inherited, v);
            }
        }
        for (k, v) in env {
            command.env(k, v);
        }
        if let Some(cwd) = &server.cwd {
            command.current_dir(cwd);
        }
        let dirty = Arc::new(AtomicBool::new(false));
        let handler = Handler {
            dirty: dirty.clone(),
        };
        let transport = match TokioChildProcess::builder(command)
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok((transport, _)) => transport,
            Err(e) => {
                let reason = format!("cannot spawn {}: {e}", server.command);
                self.fail(name, reason.clone());
                return Err(McpError::StartFailed {
                    server: name.to_owned(),
                    reason,
                });
            }
        };
        let served = tokio::time::timeout(start_timeout, handler.serve(transport)).await;
        let client = match served {
            Ok(Ok(client)) => Arc::new(client),
            Ok(Err(e)) => {
                let reason = format!("initialization failed: {e}");
                self.fail(name, reason.clone());
                return Err(McpError::StartFailed {
                    server: name.to_owned(),
                    reason,
                });
            }
            Err(_) => {
                let reason = format!(
                    "did not finish initializing within {} s",
                    start_timeout.as_secs()
                );
                self.fail(name, reason.clone());
                return Err(McpError::StartFailed {
                    server: name.to_owned(),
                    reason,
                });
            }
        };
        self.set_state(
            name,
            State::Ready {
                client: client.clone(),
                dirty,
            },
        );
        Ok(client)
    }

    fn set_state(&self, name: &str, state: State) {
        let mut servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = servers.get_mut(name) {
            entry.state = state;
            entry.last_used = Instant::now();
        }
    }

    fn fail(&self, name: &str, reason: String) {
        log::warn!("mcp server {name}: {reason}");
        self.set_state(
            name,
            State::Failed {
                reason,
                until: Instant::now() + START_BACKOFF,
            },
        );
    }

    /// The server's tools: cached unless the server reported a change or has never been
    /// listed under this revision.
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
        let listed = client
            .list_all_tools()
            .await
            .map_err(|e| self.transport_error(name, e.to_string()))?;
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
        self.persist_catalog(&server, &tools);
        let mut servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = servers.get_mut(name) {
            entry.catalog = Some(tools.clone());
        }
        Ok(tools)
    }

    fn transport_error(&self, name: &str, reason: String) -> McpError {
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
            tokio::spawn(async move {
                if let Ok(client) = Arc::try_unwrap(client) {
                    let _ = client.cancel().await;
                }
            });
        }
        McpError::Transport {
            server: name.to_owned(),
            reason,
        }
    }

    /// Search tool names and descriptions across enabled servers (or one server), starting
    /// servers that have no cached catalog yet. Start failures are reported per server.
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
            match self.catalog(&name).await {
                Ok(tools) => {
                    for t in tools {
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

    /// One tool by `server/name`.
    pub async fn describe(&self, qualified: &str) -> Result<ToolInfo, McpError> {
        let (server, tool) = split_qualified(qualified)?;
        let tools = self.catalog(&server).await?;
        tools
            .into_iter()
            .find(|t| t.name == tool)
            .ok_or(McpError::UnknownTool { server, tool })
    }

    /// Call a tool: arguments validated against the cached schema first, then one request
    /// with the configured timeout, cancellable. A lost reply is reported as unknown effect.
    pub async fn call(
        &self,
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
        let request = CallToolRequestParams::new(tool.name.clone()).with_arguments(args);
        let call = client.call_tool(request);
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(McpError::Cancelled),
            r = tokio::time::timeout(Duration::from_secs(timeout_secs), call) => match r {
                Ok(Ok(result)) => Ok(result),
                Ok(Err(e)) => Err(self.transport_error(&tool.server, e.to_string())),
                Err(_) => Err(McpError::Timeout { server: tool.server.clone(), tool: tool.name.clone(), secs: timeout_secs }),
            },
        };
        self.adjust_in_flight(&tool.server, -1);
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
        let mut servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
        let mut reaped = Vec::new();
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
        reaped
    }

    /// Stop one server now (settings page, reconnect).
    pub fn stop(&self, name: &str) -> Option<ServerHandle> {
        let mut servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
        let entry = servers.get_mut(name)?;
        let old = std::mem::replace(&mut entry.state, State::Stopped);
        match old {
            State::Ready { client, .. } => Some(ServerHandle(client)),
            _ => None,
        }
    }

    /// Every running client, for shutdown.
    pub fn drain(&self) -> Vec<ServerHandle> {
        let mut servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
        servers
            .values_mut()
            .filter_map(|entry| {
                let old = std::mem::replace(&mut entry.state, State::Stopped);
                match old {
                    State::Ready { client, .. } => Some(ServerHandle(client)),
                    _ => None,
                }
            })
            .collect()
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
