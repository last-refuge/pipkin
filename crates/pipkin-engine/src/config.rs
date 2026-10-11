//! Engine configuration: where state lives, which environment is visible, and the settings
//! the engine honours (`settings.json`, global then project).
//!
//! Native ownership: engine sessions live under `data_dir`; non-secret configuration
//! (`settings.json`, `models.json`, global `AGENTS.md`, `skills/`, `prompts/`) under
//! `config_dir`; credentials in the credential store (`auth.json` in `config_dir`, mode 0600,
//! until a qualified OS secret store replaces it). Nothing here reads the legacy engine's
//! directories; importing them is an explicit, separate action.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Map, Value};

/// Process-visible environment, overridable for tests. With `ambient` false nothing is read
/// from the real process environment (no ambient API keys, no proxies).
#[derive(Clone, Debug, Default)]
pub struct Env {
    pub vars: HashMap<String, String>,
    pub ambient: bool,
}

impl Env {
    pub fn process() -> Env {
        Env {
            vars: HashMap::new(),
            ambient: true,
        }
    }

    pub fn isolated(vars: HashMap<String, String>) -> Env {
        Env {
            vars,
            ambient: false,
        }
    }

    pub fn get(&self, name: &str) -> Option<String> {
        self.vars
            .get(name)
            .cloned()
            .or_else(|| {
                if self.ambient {
                    std::env::var(name).ok()
                } else {
                    None
                }
            })
            .filter(|v| !v.is_empty())
    }
}

#[derive(Clone, Debug)]
pub struct EngineConfig {
    /// Engine database and artifacts.
    pub data_dir: PathBuf,
    /// Non-secret configuration and the credential file.
    pub config_dir: PathBuf,
    pub env: Env,
    /// Initial model for new sessions (`provider/modelId`) when settings name none.
    pub default_model: Option<String>,
    /// Tests: extra wire adapters (e.g. the scripted provider) and their models.
    pub extra_models: Vec<crate::provider::Model>,
    /// Upper bound on concurrently generating sessions.
    pub max_active_generations: usize,
    /// Upper bound on concurrently running tool processes.
    pub max_tool_processes: usize,
}

impl EngineConfig {
    pub fn new(data_dir: impl Into<PathBuf>, config_dir: impl Into<PathBuf>) -> EngineConfig {
        EngineConfig {
            data_dir: data_dir.into(),
            config_dir: config_dir.into(),
            env: Env::process(),
            default_model: None,
            extra_models: vec![],
            max_active_generations: 8,
            max_tool_processes: 16,
        }
    }

    pub fn settings_path(&self) -> PathBuf {
        self.config_dir.join("settings.json")
    }

    pub fn models_path(&self) -> PathBuf {
        self.config_dir.join("models.json")
    }

    pub fn auth_path(&self) -> PathBuf {
        self.config_dir.join("auth.json")
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct CompactionSettings {
    pub enabled: bool,
    pub reserve_tokens: u64,
    pub keep_recent_tokens: u64,
    pub background_tokens: u64,
}

impl Default for CompactionSettings {
    fn default() -> Self {
        CompactionSettings {
            enabled: true,
            reserve_tokens: 16_384,
            keep_recent_tokens: 20_000,
            background_tokens: 32_768,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ProviderRetrySettings {
    pub timeout_ms: Option<u64>,
    pub max_retries: u32,
    pub max_retry_delay_ms: u64,
}

impl Default for ProviderRetrySettings {
    fn default() -> Self {
        ProviderRetrySettings {
            timeout_ms: None,
            max_retries: 0,
            max_retry_delay_ms: 60_000,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct RetrySettings {
    pub enabled: bool,
    pub max_retries: u32,
    pub base_delay_ms: u64,
    pub max_agent_delay_ms: u64,
    pub provider: ProviderRetrySettings,
}

impl Default for RetrySettings {
    fn default() -> Self {
        RetrySettings {
            enabled: true,
            max_retries: 3,
            base_delay_ms: 2_000,
            max_agent_delay_ms: 60_000,
            provider: ProviderRetrySettings::default(),
        }
    }
}

/// `one-at-a-time` (default) or `all`.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
pub enum QueueDelivery {
    #[default]
    #[serde(rename = "one-at-a-time")]
    OneAtATime,
    #[serde(rename = "all")]
    All,
}

/// The settings the reference durable engine honours, plus model defaults.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub default_provider: Option<String>,
    pub default_model: Option<String>,
    pub default_thinking_level: Option<String>,
    pub steering_mode: QueueDelivery,
    pub follow_up_mode: QueueDelivery,
    pub compaction: CompactionSettings,
    pub retry: RetrySettings,
    /// Extra skill paths (resolved against the session cwd).
    pub skills: Vec<String>,
    pub http_proxy: Option<String>,
    /// Milliseconds; `0` disables the idle timeout.
    pub http_idle_timeout_ms: Option<Value>,
    pub shell_path: Option<String>,
    pub enabled_models: Option<Vec<String>>,
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            default_provider: None,
            default_model: None,
            default_thinking_level: None,
            steering_mode: QueueDelivery::OneAtATime,
            follow_up_mode: QueueDelivery::OneAtATime,
            compaction: CompactionSettings::default(),
            retry: RetrySettings::default(),
            skills: vec![],
            http_proxy: None,
            http_idle_timeout_ms: None,
            shell_path: None,
            enabled_models: None,
            other: Map::new(),
        }
    }
}

impl Settings {
    /// Global settings merged with the project's `.pi/settings.json` (objects merge
    /// recursively; arrays and scalars from the project replace). A missing or invalid file is
    /// ignored with a warning; settings never block a session.
    pub fn load(config_dir: &Path, cwd: Option<&Path>) -> (Settings, Vec<String>) {
        let mut warnings = Vec::new();
        let mut merged = read_json(&config_dir.join("settings.json"), &mut warnings);
        if let Some(cwd) = cwd {
            let project = read_json(&cwd.join(".pi").join("settings.json"), &mut warnings);
            deep_merge(&mut merged, project);
        }
        migrate(&mut merged);
        match serde_json::from_value::<Settings>(merged) {
            Ok(s) => (s, warnings),
            Err(e) => {
                warnings.push(format!("settings.json is invalid ({e}); using defaults"));
                (Settings::default(), warnings)
            }
        }
    }

    /// The HTTP idle timeout; `None` means disabled.
    pub fn idle_timeout(&self) -> Option<std::time::Duration> {
        let ms = match &self.http_idle_timeout_ms {
            None => self.retry.provider.timeout_ms.unwrap_or(300_000),
            Some(Value::String(s)) if s == "disabled" => 0,
            Some(v) => v.as_u64().unwrap_or(300_000),
        };
        (ms > 0).then(|| std::time::Duration::from_millis(ms))
    }

    /// Persist a new global default model (the reference engine does this on selection).
    pub fn save_default_model(config_dir: &Path, provider: &str, model: &str) -> std::io::Result<()> {
        let path = config_dir.join("settings.json");
        let mut warnings = vec![];
        let mut value = read_json(&path, &mut warnings);
        if !value.is_object() {
            value = Value::Object(Map::new());
        }
        value["defaultProvider"] = Value::String(provider.into());
        value["defaultModel"] = Value::String(model.into());
        std::fs::create_dir_all(config_dir)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&value)? + "\n")?;
        std::fs::rename(tmp, path)
    }
}

/// Read a JSON (with comments tolerated) object file; absent is `{}`.
pub fn read_json(path: &Path, warnings: &mut Vec<String>) -> Value {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let text = text.trim_start_matches('\u{feff}');
            match serde_json::from_str::<Value>(&strip_json_comments(text)) {
                Ok(v) if v.is_object() => v,
                Ok(_) => {
                    warnings.push(format!("{} is not a JSON object; ignored", path.display()));
                    Value::Object(Map::new())
                }
                Err(e) => {
                    warnings.push(format!("{} is invalid JSON ({e}); ignored", path.display()));
                    Value::Object(Map::new())
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Object(Map::new()),
        Err(e) => {
            warnings.push(format!("cannot read {} ({e}); ignored", path.display()));
            Value::Object(Map::new())
        }
    }
}

/// Remove `//` and `/* */` comments outside strings (JSONC).
pub fn strip_json_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut prev = ' ';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
            }
            _ => out.push(c),
        }
    }
    out
}

pub fn deep_merge(base: &mut Value, over: Value) {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                match b.get_mut(&k) {
                    Some(existing) if existing.is_object() && v.is_object() => {
                        deep_merge(existing, v)
                    }
                    _ => {
                        if !v.is_null() {
                            b.insert(k, v);
                        }
                    }
                }
            }
        }
        (b, o) => *b = o,
    }
}

/// The reference engine's load-time settings migrations.
fn migrate(v: &mut Value) {
    let Some(obj) = v.as_object_mut() else { return };
    if let Some(q) = obj.remove("queueMode") {
        obj.entry("steeringMode").or_insert(q);
    }
    if let Some(delay) = obj
        .get_mut("retry")
        .and_then(Value::as_object_mut)
        .and_then(|r| r.remove("maxDelayMs"))
    {
        let retry = obj
            .get_mut("retry")
            .and_then(Value::as_object_mut)
            .expect("checked above");
        let provider = retry
            .entry("provider")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(p) = provider.as_object_mut() {
            p.entry("maxRetryDelayMs").or_insert(delay);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_settings_override_global_and_defaults_apply() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("cfg");
        let cwd = dir.path().join("proj");
        std::fs::create_dir_all(cwd.join(".pi")).unwrap();
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::write(
            cfg.join("settings.json"),
            r#"{ // comment
              "compaction": {"reserveTokens": 1000, "keepRecentTokens": 5}, "queueMode": "all",
              "retry": {"maxDelayMs": 5}}"#,
        )
        .unwrap();
        std::fs::write(
            cwd.join(".pi/settings.json"),
            r#"{"compaction": {"keepRecentTokens": 7}}"#,
        )
        .unwrap();
        let (s, w) = Settings::load(&cfg, Some(&cwd));
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(s.compaction.reserve_tokens, 1000);
        assert_eq!(s.compaction.keep_recent_tokens, 7);
        assert!(s.compaction.enabled);
        assert_eq!(s.steering_mode, QueueDelivery::All);
        assert_eq!(s.retry.provider.max_retry_delay_ms, 5);
        assert_eq!(s.retry.max_retries, 3);
        assert_eq!(s.idle_timeout(), Some(std::time::Duration::from_secs(300)));
    }

    #[test]
    fn invalid_files_warn_and_fall_back() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("settings.json"), "{nope").unwrap();
        let (s, w) = Settings::load(dir.path(), None);
        assert_eq!(s, Settings::default());
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn isolated_env_hides_the_process_environment() {
        let env = Env::isolated(HashMap::from([("A".into(), "1".into())]));
        assert_eq!(env.get("A").as_deref(), Some("1"));
        assert_eq!(env.get("PATH"), None);
    }
}
