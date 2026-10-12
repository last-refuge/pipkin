//! MCP server configuration from settings (user `settings.json`, project `.pi/settings.json`):
//!
//! ```json
//! {
//!   "mcp": {
//!     "callTimeoutSeconds": 120,
//!     "startTimeoutSeconds": 30,
//!     "servers": {
//!       "filesystem": {
//!         "command": "npx",
//!         "args": ["-y", "@modelcontextprotocol/server-filesystem", "/srv/docs"],
//!         "env": { "API_TOKEN": "$DOCS_TOKEN", "OTHER": "secret:docs-other" },
//!         "cwd": "/srv",
//!         "enabled": true,
//!         "idleSeconds": 300,
//!         "keepAlive": false
//!       }
//!     }
//!   }
//! }
//! ```
//!
//! Environment values are literal unless they are `$NAME` (read from the engine's
//! environment) or `secret:<key>` (read from the engine's private `mcp-secrets.json` next to
//! the credential file). Resolved values are passed to the child process only; they never
//! appear in views, logs or tool results. A server's configuration revision is a hash of its
//! launch-relevant fields: when it changes, a running server is stopped and rediscovered.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::config::Settings;

pub const DEFAULT_CALL_TIMEOUT_SECS: u64 = 120;
pub const DEFAULT_START_TIMEOUT_SECS: u64 = 30;
pub const DEFAULT_IDLE_SECS: u64 = 300;
/// Most servers one engine will manage.
pub const MAX_SERVERS: usize = 32;
/// Server names: letters, digits, `-`, `_`, `.`; at most this long.
pub const MAX_NAME_LEN: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnvValue {
    Literal(String),
    /// `$NAME`: the engine's environment.
    FromEnv(String),
    /// `secret:<key>`: the private secrets file.
    FromSecret(String),
}

impl EnvValue {
    pub fn parse(raw: &str) -> EnvValue {
        if let Some(name) = raw.strip_prefix('$')
            && !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return EnvValue::FromEnv(name.to_owned());
        }
        if let Some(key) = raw.strip_prefix("secret:")
            && !key.trim().is_empty()
        {
            return EnvValue::FromSecret(key.trim().to_owned());
        }
        EnvValue::Literal(raw.to_owned())
    }

    /// What a view may show: never the value of a reference.
    pub fn display(&self) -> String {
        match self {
            EnvValue::Literal(v) => v.clone(),
            EnvValue::FromEnv(name) => format!("${name}"),
            EnvValue::FromSecret(key) => format!("secret:{key}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, EnvValue)>,
    pub cwd: Option<String>,
    pub enabled: bool,
    pub idle_seconds: u64,
    pub keep_alive: bool,
    /// `user` or `project`.
    pub scope: String,
    /// Hash of the launch-relevant fields.
    pub revision: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpConfig {
    pub servers: Vec<McpServerConfig>,
    pub call_timeout_secs: u64,
    pub start_timeout_secs: u64,
    pub warnings: Vec<String>,
}

impl Default for McpConfig {
    fn default() -> Self {
        McpConfig {
            servers: Vec::new(),
            call_timeout_secs: DEFAULT_CALL_TIMEOUT_SECS,
            start_timeout_secs: DEFAULT_START_TIMEOUT_SECS,
            warnings: Vec::new(),
        }
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_LEN
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

fn revision_of(
    command: &str,
    args: &[String],
    env: &[(String, EnvValue)],
    cwd: Option<&str>,
) -> String {
    let mut h = Sha256::new();
    h.update(command.as_bytes());
    for a in args {
        h.update([0]);
        h.update(a.as_bytes());
    }
    for (k, v) in env {
        h.update([1]);
        h.update(k.as_bytes());
        h.update([2]);
        h.update(v.display().as_bytes());
    }
    h.update([3]);
    h.update(cwd.unwrap_or("").as_bytes());
    let digest = h.finalize();
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Parse one server entry. The scope comes from where the merged settings say it was set;
/// settings are merged by key, so a project entry replaces a user entry of the same name.
fn parse_server(name: &str, body: &Value, scope: &str) -> Result<McpServerConfig, String> {
    if !valid_name(name) {
        return Err(format!(
            "mcp.servers: {name:?} is not a valid server name (letters, digits, - _ ., at most {MAX_NAME_LEN})"
        ));
    }
    let Some(body) = body.as_object() else {
        return Err(format!("mcp.servers.{name} must be an object"));
    };
    let command = body
        .get("command")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .ok_or_else(|| format!("mcp.servers.{name}.command is required"))?
        .to_owned();
    let args = match body.get("args") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|a| {
                a.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("mcp.servers.{name}.args must be strings"))
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(format!("mcp.servers.{name}.args must be a list")),
    };
    let env = match body.get("env") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Object(map)) => {
            let mut env = Vec::new();
            for (k, v) in map {
                let Some(v) = v.as_str() else {
                    return Err(format!("mcp.servers.{name}.env.{k} must be a string"));
                };
                env.push((k.clone(), EnvValue::parse(v)));
            }
            env
        }
        Some(_) => return Err(format!("mcp.servers.{name}.env must be an object")),
    };
    let cwd = body
        .get("cwd")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_owned);
    let enabled = body.get("enabled").and_then(Value::as_bool).unwrap_or(true);
    let idle_seconds = body
        .get("idleSeconds")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_IDLE_SECS)
        .max(1);
    let keep_alive = body
        .get("keepAlive")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let revision = revision_of(&command, &args, &env, cwd.as_deref());
    Ok(McpServerConfig {
        name: name.to_owned(),
        command,
        args,
        env,
        cwd,
        enabled,
        idle_seconds,
        keep_alive,
        scope: scope.to_owned(),
        revision,
    })
}

impl McpConfig {
    /// From merged settings. Invalid entries become warnings and are left out; the rest
    /// still work.
    pub fn from_settings(settings: &Settings) -> McpConfig {
        let mut config = McpConfig::default();
        let Some(section) = settings.other.get("mcp") else {
            return config;
        };
        let num = |key: &str, default: u64| {
            section
                .get(key)
                .and_then(Value::as_u64)
                .filter(|n| *n > 0)
                .unwrap_or(default)
        };
        config.call_timeout_secs = num("callTimeoutSeconds", DEFAULT_CALL_TIMEOUT_SECS);
        config.start_timeout_secs = num("startTimeoutSeconds", DEFAULT_START_TIMEOUT_SECS);
        let Some(servers) = section.get("servers") else {
            return config;
        };
        let Some(servers) = servers.as_object() else {
            config
                .warnings
                .push("mcp.servers must be an object of server entries".into());
            return config;
        };
        let scopes: BTreeMap<String, String> = settings
            .other
            .get("mcpScopes")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                    .collect()
            })
            .unwrap_or_default();
        for (name, body) in servers {
            if config.servers.len() >= MAX_SERVERS {
                config.warnings.push(format!(
                    "mcp.servers: at most {MAX_SERVERS} servers are managed; {name} was left out"
                ));
                continue;
            }
            let scope = scopes.get(name).map(String::as_str).unwrap_or("user");
            match parse_server(name, body, scope) {
                Ok(server) => config.servers.push(server),
                Err(warning) => config.warnings.push(warning),
            }
        }
        config
    }

    pub fn server(&self, name: &str) -> Option<&McpServerConfig> {
        self.servers.iter().find(|s| s.name == name)
    }
}

/// The private secrets file: `{"key": "value"}`, read at launch time only.
pub fn read_secrets(config_dir: &Path) -> BTreeMap<String, String> {
    let path = config_dir.join("mcp-secrets.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return BTreeMap::new();
    };
    serde_json::from_str::<BTreeMap<String, String>>(&text).unwrap_or_default()
}

/// Resolve a server's environment for launch. A missing reference is an error naming the
/// reference, never its value.
pub fn resolve_env(
    server: &McpServerConfig,
    env: &dyn Fn(&str) -> Option<String>,
    secrets: &BTreeMap<String, String>,
) -> Result<Vec<(String, String)>, String> {
    server
        .env
        .iter()
        .map(|(k, v)| {
            let value = match v {
                EnvValue::Literal(s) => s.clone(),
                EnvValue::FromEnv(name) => env(name).ok_or_else(|| {
                    format!(
                        "mcp.servers.{}.env.{k}: environment variable {name} is not set",
                        server.name
                    )
                })?,
                EnvValue::FromSecret(key) => secrets.get(key).cloned().ok_or_else(|| {
                    format!(
                        "mcp.servers.{}.env.{k}: secret {key:?} is not in mcp-secrets.json",
                        server.name
                    )
                })?,
            };
            Ok((k.clone(), value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn settings_with(mcp: Value) -> Settings {
        let mut s = Settings::load(Path::new("/nonexistent-config-dir"), None).0;
        s.other.insert("mcp".into(), mcp);
        s
    }

    #[test]
    fn servers_parse_with_references_scopes_and_revisions() {
        let s = settings_with(json!({
            "callTimeoutSeconds": 5,
            "servers": {
                "fs": {"command": "npx", "args": ["-y", "srv"], "env": {"TOKEN": "$TOK", "K": "secret:k", "P": "plain"}, "idleSeconds": 10, "keepAlive": true},
                "bad name!": {"command": "x"},
                "noexec": {},
                "disabled": {"command": "x", "enabled": false}
            }
        }));
        let c = McpConfig::from_settings(&s);
        assert_eq!(c.call_timeout_secs, 5);
        assert_eq!(c.start_timeout_secs, DEFAULT_START_TIMEOUT_SECS);
        assert_eq!(c.servers.len(), 2);
        let fs = c.server("fs").unwrap();
        assert_eq!(fs.args, ["-y", "srv"]);
        assert_eq!(fs.env[0], ("TOKEN".into(), EnvValue::FromEnv("TOK".into())));
        assert_eq!(fs.env[1], ("K".into(), EnvValue::FromSecret("k".into())));
        assert_eq!(fs.env[2], ("P".into(), EnvValue::Literal("plain".into())));
        assert!(fs.keep_alive && fs.idle_seconds == 10 && fs.enabled);
        assert_eq!(fs.scope, "user");
        assert!(!c.server("disabled").unwrap().enabled);
        assert_eq!(c.warnings.len(), 2, "{:?}", c.warnings);
        assert!(
            c.warnings
                .iter()
                .any(|w| w.contains("not a valid server name"))
        );
        assert!(
            c.warnings
                .iter()
                .any(|w| w.contains("noexec.command is required"))
        );
        // The revision follows launch-relevant fields only.
        let again = McpConfig::from_settings(&settings_with(
            json!({"servers": {"fs": {"command": "npx", "args": ["-y", "srv"], "env": {"TOKEN": "$TOK", "K": "secret:k", "P": "plain"}, "idleSeconds": 99}}}),
        ));
        assert_eq!(again.server("fs").unwrap().revision, fs.revision);
        let changed = McpConfig::from_settings(&settings_with(
            json!({"servers": {"fs": {"command": "npx", "args": ["-y", "other"]}}}),
        ));
        assert_ne!(changed.server("fs").unwrap().revision, fs.revision);
    }

    #[test]
    fn environment_resolves_references_without_leaking_values() {
        let s = settings_with(
            json!({"servers": {"a": {"command": "x", "env": {"T": "$TOK", "S": "secret:k", "L": "lit"}}}}),
        );
        let c = McpConfig::from_settings(&s);
        let server = c.server("a").unwrap();
        let mut secrets = BTreeMap::new();
        secrets.insert("k".to_owned(), "sekrit".to_owned());
        let resolved = resolve_env(
            server,
            &|n| (n == "TOK").then(|| "tokval".to_owned()),
            &secrets,
        )
        .unwrap();
        assert_eq!(
            resolved,
            vec![
                ("T".into(), "tokval".into()),
                ("S".into(), "sekrit".into()),
                ("L".into(), "lit".into())
            ]
        );
        let err = resolve_env(server, &|_| None, &secrets).unwrap_err();
        assert!(err.contains("environment variable TOK is not set") && !err.contains("tokval"));
        let err = resolve_env(server, &|_| Some("v".into()), &BTreeMap::new()).unwrap_err();
        assert!(err.contains("secret \"k\" is not in mcp-secrets.json"));
        assert_eq!(server.env[0].1.display(), "$TOK");
        assert_eq!(server.env[1].1.display(), "secret:k");
        let dir = tempfile::tempdir().unwrap();
        assert!(read_secrets(dir.path()).is_empty());
        std::fs::write(dir.path().join("mcp-secrets.json"), "{\"k\": \"v\"}").unwrap();
        assert_eq!(
            read_secrets(dir.path()).get("k").map(String::as_str),
            Some("v")
        );
    }
}
