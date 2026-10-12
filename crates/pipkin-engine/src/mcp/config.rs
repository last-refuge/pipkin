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
//!         "keepAlive": false,
//!         "disabledTools": ["delete_file"]
//!       },
//!       "remote": {
//!         "url": "https://mcp.example.com/mcp",
//!         "headers": { "X-Team": "docs", "Authorization": "$REMOTE_TOKEN" },
//!         "oauth": { "clientId": "$REMOTE_CLIENT_ID", "scopes": ["read"] }
//!       }
//!     }
//!   }
//! }
//! ```
//!
//! A server is either a process over stdio (`command`) or a Streamable HTTP endpoint (`url`).
//! Environment and header values are literal unless they are `$NAME` (read from the engine's
//! environment) or `secret:<key>` (read from the engine's private `mcp-secrets.json` next to
//! the credential file). Resolved values reach the child process or the request only; views
//! show references as written and literals as `…`. A server's configuration revision is a
//! hash of its launch-relevant fields: when it changes, a running server is stopped and
//! rediscovered.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::config::Settings;

pub const DEFAULT_CALL_TIMEOUT_SECS: u64 = 120;
pub const DEFAULT_START_TIMEOUT_SECS: u64 = 30;
pub const DEFAULT_IDLE_SECS: u64 = 300;
/// Most servers one engine will manage.
pub const MAX_SERVERS: usize = 32;
/// Server names: letters, digits, `-`, `_`, `.`; at most this long.
pub const MAX_NAME_LEN: usize = 64;
/// What a view shows for an inline literal, and what an edit sends to keep one.
pub const REDACTED: &str = "…";

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

    /// What a view may show: never the value of a literal.
    pub fn display(&self) -> String {
        match self {
            EnvValue::Literal(_) => REDACTED.to_owned(),
            EnvValue::FromEnv(name) => format!("${name}"),
            EnvValue::FromSecret(key) => format!("secret:{key}"),
        }
    }

    /// As written in settings.
    pub fn raw(&self) -> String {
        match self {
            EnvValue::Literal(v) => v.clone(),
            EnvValue::FromEnv(name) => format!("${name}"),
            EnvValue::FromSecret(key) => format!("secret:{key}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Transport {
    Stdio {
        command: String,
        args: Vec<String>,
        cwd: Option<String>,
        env: Vec<(String, EnvValue)>,
    },
    Http {
        url: String,
        headers: Vec<(String, EnvValue)>,
    },
}

/// OAuth settings for an HTTP server. Without a client id the engine registers one
/// dynamically (RFC 7591) when the server's authorization server allows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OAuthSettings {
    pub client_id: Option<EnvValue>,
    pub client_secret: Option<EnvValue>,
    pub scopes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpServerConfig {
    pub name: String,
    pub transport: Transport,
    pub enabled: bool,
    pub idle_seconds: u64,
    pub keep_alive: bool,
    /// Tools the person switched off; never offered or callable.
    pub disabled_tools: Vec<String>,
    pub oauth: OAuthSettings,
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

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_LEN
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

fn revision_of(transport: &Transport, oauth: &OAuthSettings) -> String {
    let mut bytes: Vec<u8> = Vec::new();
    let mut pairs = |tag: u8, list: &[(String, EnvValue)]| {
        for (k, v) in list {
            bytes.push(tag);
            bytes.extend_from_slice(k.as_bytes());
            bytes.push(tag + 1);
            bytes.extend_from_slice(v.raw().as_bytes());
        }
    };
    match transport {
        Transport::Stdio {
            command,
            args,
            cwd,
            env,
        } => {
            pairs(1, env);
            pairs(
                2,
                &[("command".to_owned(), EnvValue::Literal(command.clone()))],
            );
            for a in args {
                pairs(3, &[("arg".to_owned(), EnvValue::Literal(a.clone()))]);
            }
            pairs(
                4,
                &[(
                    "cwd".to_owned(),
                    EnvValue::Literal(cwd.clone().unwrap_or_default()),
                )],
            );
        }
        Transport::Http { url, headers } => {
            pairs(5, headers);
            pairs(6, &[("url".to_owned(), EnvValue::Literal(url.clone()))]);
        }
    }
    pairs(
        7,
        &[(
            "clientId".to_owned(),
            EnvValue::Literal(
                oauth
                    .client_id
                    .as_ref()
                    .map(EnvValue::raw)
                    .unwrap_or_default(),
            ),
        )],
    );
    pairs(
        8,
        &[(
            "scopes".to_owned(),
            EnvValue::Literal(oauth.scopes.join(" ")),
        )],
    );
    let digest = Sha256::digest(&bytes);
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

fn string_list(name: &str, key: &str, value: Option<&Value>) -> Result<Vec<String>, String> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|a| {
                a.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("mcp.servers.{name}.{key} must be strings"))
            })
            .collect(),
        Some(_) => Err(format!("mcp.servers.{name}.{key} must be a list")),
    }
}

fn ref_map(
    name: &str,
    key: &str,
    value: Option<&Value>,
) -> Result<Vec<(String, EnvValue)>, String> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Object(map)) => {
            let mut out = Vec::new();
            for (k, v) in map {
                let Some(v) = v.as_str() else {
                    return Err(format!("mcp.servers.{name}.{key}.{k} must be a string"));
                };
                out.push((k.clone(), EnvValue::parse(v)));
            }
            Ok(out)
        }
        Some(_) => Err(format!("mcp.servers.{name}.{key} must be an object")),
    }
}

/// Parse one server entry as it appears in settings. The scope comes from where the merged
/// settings say it was set.
pub fn parse_server(name: &str, body: &Value, scope: &str) -> Result<McpServerConfig, String> {
    if !valid_name(name) {
        return Err(format!(
            "mcp.servers: {name:?} is not a valid server name (letters, digits, - _ ., at most {MAX_NAME_LEN})"
        ));
    }
    let Some(body) = body.as_object() else {
        return Err(format!("mcp.servers.{name} must be an object"));
    };
    let text = |key: &str| {
        body.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(str::to_owned)
    };
    let command = text("command");
    let url = text("url");
    let transport = match (command, url) {
        (Some(_), Some(_)) => {
            return Err(format!(
                "mcp.servers.{name}: give either command (stdio) or url (HTTP), not both"
            ));
        }
        (Some(command), None) => Transport::Stdio {
            command,
            args: string_list(name, "args", body.get("args"))?,
            cwd: text("cwd"),
            env: ref_map(name, "env", body.get("env"))?,
        },
        (None, Some(url)) => {
            let parsed = url::Url::parse(&url)
                .map_err(|e| format!("mcp.servers.{name}.url is not a valid URL ({e})"))?;
            if !matches!(parsed.scheme(), "http" | "https") {
                return Err(format!("mcp.servers.{name}.url must be http or https"));
            }
            if parsed.host_str().is_none() {
                return Err(format!("mcp.servers.{name}.url has no host"));
            }
            Transport::Http {
                url,
                headers: ref_map(name, "headers", body.get("headers"))?,
            }
        }
        (None, None) => {
            return Err(format!(
                "mcp.servers.{name}: command (stdio) or url (HTTP) is required"
            ));
        }
    };
    let oauth = match body.get("oauth") {
        None | Some(Value::Null) => OAuthSettings::default(),
        Some(Value::Object(o)) => OAuthSettings {
            client_id: o
                .get("clientId")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(EnvValue::parse),
            client_secret: o
                .get("clientSecret")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(EnvValue::parse),
            scopes: string_list(name, "oauth.scopes", o.get("scopes"))?,
        },
        Some(_) => return Err(format!("mcp.servers.{name}.oauth must be an object")),
    };
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
    let disabled_tools = string_list(name, "disabledTools", body.get("disabledTools"))?;
    let revision = revision_of(&transport, &oauth);
    Ok(McpServerConfig {
        name: name.to_owned(),
        transport,
        enabled,
        idle_seconds,
        keep_alive,
        disabled_tools,
        oauth,
        scope: scope.to_owned(),
        revision,
    })
}

impl McpServerConfig {
    pub fn kind(&self) -> &'static str {
        match self.transport {
            Transport::Stdio { .. } => "stdio",
            Transport::Http { .. } => "http",
        }
    }

    pub fn url(&self) -> Option<&str> {
        match &self.transport {
            Transport::Http { url, .. } => Some(url),
            Transport::Stdio { .. } => None,
        }
    }

    /// The executable and its arguments, or the URL.
    pub fn command_line(&self) -> String {
        match &self.transport {
            Transport::Stdio { command, args, .. } => {
                let mut line = vec![command.clone()];
                line.extend(args.iter().cloned());
                line.join(" ")
            }
            Transport::Http { url, .. } => url.clone(),
        }
    }

    /// `NAME=$REF` / `NAME=secret:key` / `NAME=…` for environment (stdio) or headers (HTTP).
    pub fn values_display(&self) -> Vec<String> {
        let list = match &self.transport {
            Transport::Stdio { env, .. } => env,
            Transport::Http { headers, .. } => headers,
        };
        list.iter()
            .map(|(k, v)| format!("{k}={}", v.display()))
            .collect()
    }

    pub fn tool_enabled(&self, tool: &str) -> bool {
        !self.disabled_tools.iter().any(|t| t == tool)
    }

    /// The settings entry with every literal value redacted (what a client may edit).
    pub fn redacted_spec(&self) -> Value {
        let refs = |list: &[(String, EnvValue)]| -> Value {
            Value::Object(
                list.iter()
                    .map(|(k, v)| (k.clone(), Value::String(v.display())))
                    .collect(),
            )
        };
        let mut spec = Map::new();
        match &self.transport {
            Transport::Stdio {
                command,
                args,
                cwd,
                env,
            } => {
                spec.insert("command".into(), json!(command));
                if !args.is_empty() {
                    spec.insert("args".into(), json!(args));
                }
                if let Some(cwd) = cwd {
                    spec.insert("cwd".into(), json!(cwd));
                }
                if !env.is_empty() {
                    spec.insert("env".into(), refs(env));
                }
            }
            Transport::Http { url, headers } => {
                spec.insert("url".into(), json!(url));
                if !headers.is_empty() {
                    spec.insert("headers".into(), refs(headers));
                }
            }
        }
        if !self.enabled {
            spec.insert("enabled".into(), json!(false));
        }
        if self.idle_seconds != DEFAULT_IDLE_SECS {
            spec.insert("idleSeconds".into(), json!(self.idle_seconds));
        }
        if self.keep_alive {
            spec.insert("keepAlive".into(), json!(true));
        }
        if !self.disabled_tools.is_empty() {
            spec.insert("disabledTools".into(), json!(self.disabled_tools));
        }
        let mut oauth = Map::new();
        if let Some(id) = &self.oauth.client_id {
            oauth.insert("clientId".into(), json!(id.display()));
        }
        if let Some(secret) = &self.oauth.client_secret {
            oauth.insert("clientSecret".into(), json!(secret.display()));
        }
        if !self.oauth.scopes.is_empty() {
            oauth.insert("scopes".into(), json!(self.oauth.scopes));
        }
        if !oauth.is_empty() {
            spec.insert("oauth".into(), Value::Object(oauth));
        }
        Value::Object(spec)
    }
}

/// An edited entry on top of the stored one: a value of [`REDACTED`] keeps what is stored.
/// Everything else replaces the stored entry (the edit is the whole spec).
pub fn merge_spec(existing: Option<&Value>, incoming: &Value) -> Value {
    let mut out = incoming.clone();
    let Some(existing) = existing.and_then(Value::as_object) else {
        return out;
    };
    for key in ["env", "headers"] {
        if let Some(Value::Object(map)) = out.get_mut(key) {
            for (k, v) in map.iter_mut() {
                if v.as_str() == Some(REDACTED) {
                    *v = existing
                        .get(key)
                        .and_then(|e| e.get(k))
                        .cloned()
                        .unwrap_or(Value::String(String::new()));
                }
            }
        }
    }
    if let Some(Value::Object(oauth)) = out.get_mut("oauth") {
        for k in ["clientId", "clientSecret"] {
            if oauth.get(k).and_then(Value::as_str) == Some(REDACTED) {
                let kept = existing
                    .get("oauth")
                    .and_then(|o| o.get(k))
                    .cloned()
                    .unwrap_or(Value::String(String::new()));
                oauth.insert(k.to_owned(), kept);
            }
        }
    }
    out
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

/// Resolve references for launch. A missing reference is an error naming the reference,
/// never its value.
pub fn resolve_refs(
    server: &str,
    section: &str,
    list: &[(String, EnvValue)],
    env: &dyn Fn(&str) -> Option<String>,
    secrets: &BTreeMap<String, String>,
) -> Result<Vec<(String, String)>, String> {
    list.iter()
        .map(|(k, v)| {
            Ok((
                k.clone(),
                resolve_one(server, &format!("{section}.{k}"), v, env, secrets)?,
            ))
        })
        .collect()
}

pub fn resolve_one(
    server: &str,
    field: &str,
    value: &EnvValue,
    env: &dyn Fn(&str) -> Option<String>,
    secrets: &BTreeMap<String, String>,
) -> Result<String, String> {
    match value {
        EnvValue::Literal(s) => Ok(s.clone()),
        EnvValue::FromEnv(name) => env(name).ok_or_else(|| {
            format!("mcp.servers.{server}.{field}: environment variable {name} is not set")
        }),
        EnvValue::FromSecret(key) => secrets.get(key).cloned().ok_or_else(|| {
            format!("mcp.servers.{server}.{field}: secret {key:?} is not in mcp-secrets.json")
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                "fs": {"command": "npx", "args": ["-y", "srv"], "env": {"TOKEN": "$TOK", "K": "secret:k", "P": "plain"}, "idleSeconds": 10, "keepAlive": true, "disabledTools": ["rm"]},
                "web": {"url": "https://mcp.example.com/mcp", "headers": {"Authorization": "$T"}, "oauth": {"clientId": "abc", "scopes": ["read"]}},
                "bad name!": {"command": "x"},
                "noexec": {},
                "both": {"command": "x", "url": "https://x"},
                "badurl": {"url": "ftp://x"},
                "disabled": {"command": "x", "enabled": false}
            }
        }));
        let c = McpConfig::from_settings(&s);
        assert_eq!(c.call_timeout_secs, 5);
        assert_eq!(c.start_timeout_secs, DEFAULT_START_TIMEOUT_SECS);
        assert_eq!(c.servers.len(), 3);
        let fs = c.server("fs").unwrap();
        let Transport::Stdio { args, env, .. } = &fs.transport else {
            panic!("stdio");
        };
        assert_eq!(args, &["-y", "srv"]);
        assert_eq!(env[0], ("TOKEN".into(), EnvValue::FromEnv("TOK".into())));
        assert_eq!(env[1], ("K".into(), EnvValue::FromSecret("k".into())));
        assert_eq!(env[2], ("P".into(), EnvValue::Literal("plain".into())));
        assert!(fs.keep_alive && fs.idle_seconds == 10 && fs.enabled);
        assert_eq!(fs.disabled_tools, ["rm"]);
        assert!(!fs.tool_enabled("rm") && fs.tool_enabled("ls"));
        assert_eq!(fs.scope, "user");
        assert_eq!(fs.kind(), "stdio");
        assert_eq!(fs.values_display(), ["TOKEN=$TOK", "K=secret:k", "P=…"]);
        let web = c.server("web").unwrap();
        assert_eq!(web.kind(), "http");
        assert_eq!(web.url(), Some("https://mcp.example.com/mcp"));
        assert_eq!(web.oauth.client_id, Some(EnvValue::Literal("abc".into())));
        assert_eq!(web.oauth.scopes, ["read"]);
        assert_eq!(
            web.redacted_spec(),
            json!({"url": "https://mcp.example.com/mcp", "headers": {"Authorization": "$T"}, "oauth": {"clientId": "…", "scopes": ["read"]}})
        );
        assert!(!c.server("disabled").unwrap().enabled);
        assert_eq!(c.warnings.len(), 4, "{:?}", c.warnings);
        assert!(
            c.warnings
                .iter()
                .any(|w| w.contains("not a valid server name"))
        );
        assert!(
            c.warnings
                .iter()
                .any(|w| w.contains("command (stdio) or url (HTTP) is required"))
        );
        assert!(c.warnings.iter().any(|w| w.contains("not both")));
        assert!(
            c.warnings
                .iter()
                .any(|w| w.contains("must be http or https"))
        );
        // The revision follows launch-relevant fields only.
        let again = McpConfig::from_settings(&settings_with(
            json!({"servers": {"fs": {"command": "npx", "args": ["-y", "srv"], "env": {"TOKEN": "$TOK", "K": "secret:k", "P": "plain"}, "idleSeconds": 99, "disabledTools": []}}}),
        ));
        assert_eq!(again.server("fs").unwrap().revision, fs.revision);
        let changed = McpConfig::from_settings(&settings_with(
            json!({"servers": {"fs": {"command": "npx", "args": ["-y", "other"]}}}),
        ));
        assert_ne!(changed.server("fs").unwrap().revision, fs.revision);
    }

    #[test]
    fn references_resolve_without_leaking_and_edits_keep_redacted_literals() {
        let s = settings_with(
            json!({"servers": {"a": {"command": "x", "env": {"T": "$TOK", "S": "secret:k", "L": "lit"}}}}),
        );
        let c = McpConfig::from_settings(&s);
        let server = c.server("a").unwrap();
        let Transport::Stdio { env, .. } = &server.transport else {
            panic!()
        };
        let mut secrets = BTreeMap::new();
        secrets.insert("k".to_owned(), "sekrit".to_owned());
        let resolved = resolve_refs(
            "a",
            "env",
            env,
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
        let err = resolve_refs("a", "env", env, &|_| None, &secrets).unwrap_err();
        assert!(err.contains("environment variable TOK is not set") && !err.contains("tokval"));
        let err =
            resolve_refs("a", "env", env, &|_| Some("v".into()), &BTreeMap::new()).unwrap_err();
        assert!(err.contains("secret \"k\" is not in mcp-secrets.json"));
        let dir = tempfile::tempdir().unwrap();
        assert!(read_secrets(dir.path()).is_empty());
        std::fs::write(dir.path().join("mcp-secrets.json"), "{\"k\": \"v\"}").unwrap();
        assert_eq!(
            read_secrets(dir.path()).get("k").map(String::as_str),
            Some("v")
        );
        let stored = json!({"command": "x", "env": {"L": "lit", "T": "$TOK"}, "oauth": {"clientSecret": "s3"}});
        let edited = json!({"command": "y", "env": {"L": "…", "T": "$OTHER", "N": "new"}, "oauth": {"clientSecret": "…", "clientId": "id"}});
        assert_eq!(
            merge_spec(Some(&stored), &edited),
            json!({"command": "y", "env": {"L": "lit", "T": "$OTHER", "N": "new"}, "oauth": {"clientSecret": "s3", "clientId": "id"}})
        );
        assert_eq!(
            merge_spec(None, &json!({"env": {"L": "…"}})),
            json!({"env": {"L": "…"}})
        );
    }
}
