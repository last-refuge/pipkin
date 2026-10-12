//! MCP as the runtime sees it: one engine-wide [`McpService`] whose configuration follows the
//! merged settings of the session making a call (a project's `.pi/settings.json` can add or
//! override servers; the project scope is reported on each server), the three compact tools
//! offered to every session (children keep them only when their frozen launch policy
//! allows), the `McpServers` view, the management calls behind the Connections page, the
//! `McpChanged` event and the idle reaper.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pipkin_engine_protocol::*;
use serde_json::{Map, Value};

use super::Shared;
use crate::config::Settings;
use crate::mcp::config::{McpServerConfig, merge_spec, parse_server, valid_name};
use crate::mcp::{AuthProgress, McpConfig, McpError, ServerSnapshot};
use crate::tools::Tool;
use crate::tools::mcp::{McpCallTool, McpDescribeTool, McpSearchTool};

/// How often idle servers are checked.
const REAP_INTERVAL: Duration = Duration::from_secs(1);

fn invalid(message: impl Into<String>) -> ProtoError {
    ProtoError::new(ErrorCode::Invalid, message.into())
}

impl Shared {
    pub fn mcp_tools(&self) -> [Arc<dyn Tool>; 3] {
        [
            Arc::new(McpSearchTool),
            Arc::new(McpDescribeTool),
            Arc::new(McpCallTool),
        ]
    }

    /// The session's working directory, when the session exists.
    fn mcp_cwd(&self, session: Option<&str>) -> Result<Option<PathBuf>, ProtoError> {
        let Some(session) = session else {
            return Ok(None);
        };
        let row = self.store.session(session)?.ok_or_else(|| {
            ProtoError::new(
                ErrorCode::NotFound,
                format!("Session {session} does not exist"),
            )
        })?;
        Ok(Some(PathBuf::from(row.cwd)))
    }

    /// The MCP configuration for a session: merged settings, with each server's scope
    /// (`project` when the project's settings set or changed it).
    pub fn mcp_config_for(&self, session: Option<&str>) -> Result<McpConfig, ProtoError> {
        let cwd = self.mcp_cwd(session)?;
        let mut merged = Settings::load(&self.config.config_dir, cwd.as_deref()).0;
        let user = Settings::load(&self.config.config_dir, None).0;
        let user_servers = user
            .other
            .get("mcp")
            .and_then(|m| m.get("servers"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let mut scopes = Map::new();
        if let Some(servers) = merged
            .other
            .get("mcp")
            .and_then(|m| m.get("servers"))
            .and_then(Value::as_object)
        {
            for (name, body) in servers {
                let scope = if user_servers.get(name) == Some(body) {
                    "user"
                } else {
                    "project"
                };
                scopes.insert(name.clone(), Value::String(scope.into()));
            }
        }
        merged
            .other
            .insert("mcpScopes".into(), Value::Object(scopes));
        Ok(McpConfig::from_settings(&merged))
    }

    /// Apply the session's configuration to the service before a call. Servers whose
    /// configuration changed (or that disappeared) are stopped in the background.
    pub fn mcp_refresh(&self, session: Option<&str>) {
        let Ok(config) = self.mcp_config_for(session) else {
            return;
        };
        for warning in &config.warnings {
            log::warn!("mcp settings: {warning}");
        }
        for handle in self.mcp.apply_config(config) {
            self.spawn(async move { handle.stop().await });
        }
    }

    pub fn mcp_servers_view(&self, session: Option<&str>) -> Result<McpServersView, ProtoError> {
        let cwd = self.mcp_cwd(session)?;
        self.mcp_refresh(session);
        let config = self.mcp.config();
        let servers = self.mcp.snapshots().into_iter().map(server_view).collect();
        Ok(McpServersView {
            servers,
            warnings: config.warnings,
            call_timeout_seconds: config.call_timeout_secs,
            project_available: cwd.is_some(),
        })
    }

    fn mcp_settings_path(
        &self,
        session: Option<&str>,
        scope: McpScope,
    ) -> Result<PathBuf, ProtoError> {
        match scope {
            McpScope::User => Ok(self.config.config_dir.join("settings.json")),
            McpScope::Project => {
                let cwd = self.mcp_cwd(session)?.ok_or_else(|| {
                    invalid("Project settings need an open session in that project")
                })?;
                Ok(cwd.join(".pi").join("settings.json"))
            }
        }
    }

    /// Edit `mcp.servers` in one settings file, then re-apply the configuration.
    fn mcp_edit_servers(
        &self,
        session: Option<&str>,
        scope: McpScope,
        edit: impl FnOnce(&mut Map<String, Value>) -> Result<(), ProtoError>,
    ) -> Result<(), ProtoError> {
        let path = self.mcp_settings_path(session, scope)?;
        let mut outcome = Ok(());
        let written = Settings::save_at(&path, |value| {
            let root = value.as_object_mut().expect("object");
            let mcp = root
                .entry("mcp")
                .or_insert_with(|| Value::Object(Map::new()));
            if !mcp.is_object() {
                *mcp = Value::Object(Map::new());
            }
            let servers = mcp
                .as_object_mut()
                .expect("object")
                .entry("servers")
                .or_insert_with(|| Value::Object(Map::new()));
            if !servers.is_object() {
                *servers = Value::Object(Map::new());
            }
            outcome = edit(servers.as_object_mut().expect("object"));
        });
        outcome?;
        written.map_err(|e| {
            ProtoError::new(
                ErrorCode::Storage,
                format!("cannot write {}: {e}", path.display()),
            )
        })?;
        self.mcp_refresh(session);
        Ok(())
    }

    fn stored_spec(&self, session: Option<&str>, scope: McpScope, name: &str) -> Option<Value> {
        let path = self.mcp_settings_path(session, scope).ok()?;
        let mut warnings = Vec::new();
        crate::config::read_json(&path, &mut warnings)
            .get("mcp")?
            .get("servers")?
            .get(name)
            .cloned()
    }

    pub fn mcp_upsert(
        &self,
        session: Option<&str>,
        scope: McpScope,
        name: &str,
        spec: Value,
    ) -> Result<(), ProtoError> {
        if !valid_name(name) {
            return Err(invalid(format!(
                "{name:?} is not a valid server name (letters, digits, - _ .)"
            )));
        }
        let merged = merge_spec(self.stored_spec(session, scope, name).as_ref(), &spec);
        parse_server(name, &merged, "user").map_err(invalid)?;
        self.mcp_edit_servers(session, scope, |servers| {
            servers.insert(name.to_owned(), merged);
            Ok(())
        })
    }

    pub fn mcp_remove(
        &self,
        session: Option<&str>,
        scope: McpScope,
        name: &str,
    ) -> Result<(), ProtoError> {
        let mut removed = false;
        self.mcp_edit_servers(session, scope, |servers| {
            removed = servers.remove(name).is_some();
            Ok(())
        })?;
        if !removed {
            return Err(ProtoError::new(
                ErrorCode::NotFound,
                format!("No server named {name} in those settings"),
            ));
        }
        if let Some(handle) = self.mcp.stop(name, true) {
            self.spawn(async move { handle.stop().await });
        }
        Ok(())
    }

    pub fn mcp_set_disabled(
        &self,
        session: Option<&str>,
        scope: McpScope,
        name: &str,
        disabled: Vec<String>,
    ) -> Result<(), ProtoError> {
        self.mcp_edit_servers(session, scope, |servers| {
            let Some(entry) = servers.get_mut(name).and_then(Value::as_object_mut) else {
                return Err(ProtoError::new(
                    ErrorCode::NotFound,
                    format!("No server named {name} in those settings"),
                ));
            };
            if disabled.is_empty() {
                entry.remove("disabledTools");
            } else {
                entry.insert("disabledTools".into(), Value::from(disabled));
            }
            Ok(())
        })
    }

    pub async fn mcp_test(&self, session: Option<&str>, name: &str) -> McpTestView {
        self.mcp_refresh(session);
        match self.mcp.test(name).await {
            Ok(outcome) => McpTestView {
                name: name.to_owned(),
                ok: true,
                message: format!(
                    "Connected to {}{} with {} tool(s) in {} ms",
                    outcome
                        .server_name
                        .clone()
                        .unwrap_or_else(|| name.to_owned()),
                    outcome
                        .server_version
                        .as_ref()
                        .map(|v| format!(" {v}"))
                        .unwrap_or_default(),
                    outcome.tools.len(),
                    outcome.latency_ms
                ),
                tools: outcome.tools,
                server_name: outcome.server_name,
                server_version: outcome.server_version,
                protocol_version: outcome.protocol_version,
                latency_ms: outcome.latency_ms,
            },
            Err(e) => McpTestView {
                name: name.to_owned(),
                ok: false,
                message: e.to_string(),
                ..McpTestView::default()
            },
        }
    }

    pub async fn mcp_reconnect(&self, session: Option<&str>, name: &str) -> Result<(), ProtoError> {
        self.mcp_refresh(session);
        match self.mcp.reconnect(name).await {
            Ok(()) => Ok(()),
            // The state carries the reason; the view is still useful.
            Err(McpError::StartFailed { .. } | McpError::AuthRequired { .. }) => Ok(()),
            Err(e) => Err(invalid(e.to_string())),
        }
    }

    pub fn mcp_auth_start(&self, session: Option<&str>, name: &str) -> Result<(), ProtoError> {
        self.mcp_refresh(session);
        self.mcp
            .auth_start(name)
            .map_err(|e| invalid(e.to_string()))
    }

    pub fn mcp_import_preview(&self, session: Option<&str>, text: &str) -> McpImportView {
        let configured: Vec<String> = self
            .mcp_config_for(session)
            .map(|c| c.servers.into_iter().map(|s| s.name).collect())
            .unwrap_or_default();
        let preview = crate::mcp::import::preview(text);
        McpImportView {
            entries: preview
                .entries
                .into_iter()
                .map(|e| McpImportEntryView {
                    exists: configured.contains(&e.name),
                    valid: !e.spec.is_null() && parse_server(&e.name, &e.spec, "user").is_ok(),
                    name: e.name,
                    kind: e.kind,
                    spec: e.redacted,
                    warnings: e.warnings,
                })
                .collect(),
            warnings: preview.warnings,
        }
    }

    pub fn mcp_import_apply(
        &self,
        session: Option<&str>,
        scope: McpScope,
        text: &str,
        names: &[String],
    ) -> Result<(), ProtoError> {
        let preview = crate::mcp::import::preview(text);
        let mut chosen = Vec::new();
        for name in names {
            let entry = preview
                .entries
                .iter()
                .find(|e| &e.name == name)
                .ok_or_else(|| invalid(format!("{name} is not in the pasted configuration")))?;
            if entry.spec.is_null() {
                return Err(invalid(format!(
                    "{name} cannot be imported: {}",
                    entry.warnings.join("; ")
                )));
            }
            parse_server(name, &entry.spec, "user").map_err(invalid)?;
            chosen.push((name.clone(), entry.spec.clone()));
        }
        if chosen.is_empty() {
            return Err(invalid("Choose at least one server to import"));
        }
        self.mcp_edit_servers(session, scope, |servers| {
            for (name, spec) in chosen {
                servers.insert(name, spec);
            }
            Ok(())
        })
    }
}

fn auth_view(config: &McpServerConfig, snap: &ServerSnapshot) -> McpAuthView {
    let AuthProgress {
        status,
        message,
        url,
        prompt,
    } = snap.auth.clone();
    McpAuthView {
        supported: config.kind() == "http",
        signed_in: snap.signed_in,
        required: snap.state == "authRequired",
        status,
        message,
        url,
        prompt: prompt.map(|p| McpPromptView {
            id: p.id,
            kind: p.kind,
            message: p.message,
            placeholder: p.placeholder,
            options: p.options,
        }),
    }
}

fn server_view(snap: ServerSnapshot) -> McpServerView {
    let config = snap.config.clone();
    let catalog: Vec<McpToolView> = snap
        .catalog
        .as_ref()
        .map(|tools| {
            tools
                .iter()
                .map(|t| McpToolView {
                    name: t.name.clone(),
                    description: t.description.lines().next().unwrap_or("").trim().to_owned(),
                    enabled: config.tool_enabled(&t.name),
                })
                .collect()
        })
        .unwrap_or_default();
    McpServerView {
        auth: auth_view(&config, &snap),
        name: config.name.clone(),
        scope: config.scope.clone(),
        kind: config.kind().into(),
        enabled: config.enabled,
        state: snap.state,
        error: snap.error,
        tools: snap.catalog.as_ref().map(Vec::len),
        catalog,
        disabled_tools: config.disabled_tools.clone(),
        revision: config.revision.clone(),
        command: config.command_line(),
        env: config.values_display(),
        keep_alive: config.keep_alive,
        idle_seconds: config.idle_seconds,
        in_flight: snap.in_flight,
        spec: config.redacted_spec(),
    }
}

/// Wire the service to the engine: its HTTP client, change publication, and the idle reaper
/// (servers idle beyond their allowance stop, never one with a call in flight).
pub(super) fn attach(shared: &Arc<Shared>) {
    let weak = Arc::downgrade(shared);
    shared.mcp.set_http(Arc::new({
        let weak = weak.clone();
        move || weak.upgrade().map(|s| s.refresh_net()).unwrap_or_default()
    }));
    shared.mcp.set_listener(Arc::new({
        let weak = weak.clone();
        move || {
            if let Some(s) = weak.upgrade() {
                s.publish(Event::McpChanged);
            }
        }
    }));
    shared.spawn(async move {
        let mut ticks = tokio::time::interval(REAP_INTERVAL);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticks.tick().await;
            let Some(shared) = weak.upgrade() else { break };
            if shared.stopping.is_cancelled() {
                break;
            }
            for (name, handle) in shared.mcp.reap_idle(Instant::now()) {
                log::info!("mcp server {name}: stopped after idling");
                shared.spawn(async move { handle.stop().await });
            }
        }
    });
}
