//! MCP as the runtime sees it: one engine-wide [`McpService`] whose configuration follows the
//! merged settings of the session making a call (a project's `.pi/settings.json` can add or
//! override servers; the project scope is reported on each server), the three compact tools
//! offered to every session (children keep them only when their frozen launch policy
//! allows), the `McpServers` view and the idle reaper.

use std::sync::Arc;
use std::time::{Duration, Instant};

use pipkin_engine_protocol::*;
use serde_json::Value;

use super::Shared;
use crate::config::Settings;
use crate::mcp::{McpConfig, config::EnvValue};
use crate::tools::Tool;
use crate::tools::mcp::{McpCallTool, McpDescribeTool, McpSearchTool};

/// How often idle servers are checked.
const REAP_INTERVAL: Duration = Duration::from_secs(1);

impl Shared {
    pub fn mcp_tools(&self) -> [Arc<dyn Tool>; 3] {
        [
            Arc::new(McpSearchTool),
            Arc::new(McpDescribeTool),
            Arc::new(McpCallTool),
        ]
    }

    /// The MCP configuration for a session: merged settings, with each server's scope
    /// (`project` when the project's settings set or changed it).
    pub fn mcp_config_for(&self, session: &str) -> McpConfig {
        let mut merged = self.settings_for(session);
        let user = Settings::load(&self.config.config_dir, None).0;
        let user_servers = user
            .other
            .get("mcp")
            .and_then(|m| m.get("servers"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let mut scopes = serde_json::Map::new();
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
        McpConfig::from_settings(&merged)
    }

    /// Apply the session's configuration to the service before a call. Servers whose
    /// configuration changed (or that disappeared) are stopped in the background.
    pub fn mcp_refresh(&self, session: &str) {
        let config = self.mcp_config_for(session);
        for warning in &config.warnings {
            log::warn!("mcp settings: {warning}");
        }
        for handle in self.mcp.apply_config(config) {
            self.spawn(async move { handle.stop().await });
        }
    }

    pub fn mcp_servers_view(&self, session: &str) -> Result<McpServersView, ProtoError> {
        if self.store.session(session)?.is_none() {
            return Err(ProtoError::new(
                ErrorCode::NotFound,
                format!("Session {session} does not exist"),
            ));
        }
        self.mcp_refresh(session);
        let config = self.mcp.config();
        let servers = self
            .mcp
            .snapshots()
            .into_iter()
            .map(|snap| {
                let (command, env) = config
                    .server(&snap.name)
                    .map(|c| {
                        let mut command = vec![c.command.clone()];
                        command.extend(c.args.iter().cloned());
                        let env = c
                            .env
                            .iter()
                            .map(|(k, v)| match v {
                                EnvValue::Literal(_) => format!("{k}=…"),
                                other => format!("{k}={}", other.display()),
                            })
                            .collect();
                        (command.join(" "), env)
                    })
                    .unwrap_or_default();
                McpServerView {
                    name: snap.name,
                    scope: snap.scope,
                    enabled: snap.enabled,
                    state: snap.state,
                    error: snap.error,
                    tools: snap.tools,
                    revision: snap.revision,
                    command,
                    env,
                    keep_alive: snap.keep_alive,
                    idle_seconds: snap.idle_seconds,
                    in_flight: snap.in_flight,
                }
            })
            .collect();
        Ok(McpServersView {
            servers,
            warnings: config.warnings,
            call_timeout_seconds: config.call_timeout_secs,
        })
    }
}

/// Stop servers that have been idle beyond their allowance (never one with a call in
/// flight) until the engine stops.
pub(super) fn start_reaper(shared: &Arc<Shared>) {
    let weak = Arc::downgrade(shared);
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
