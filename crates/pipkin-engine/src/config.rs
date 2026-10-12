//! Engine configuration: where state lives, which environment is visible, and the settings
//! the engine honours (`settings.json`, global then project).
//!
//! Native ownership: engine sessions live under `data_dir`; non-secret configuration
//! (`settings.json`, `models.json`, global `AGENTS.md`, `skills/`, `prompts/`) under
//! `config_dir`; credentials in the credential store (`auth.json` in `config_dir`, mode 0600,
//! until a qualified OS secret store replaces it). Nothing here reads the legacy engine's
//! directories; importing them is an explicit, separate action.

use std::collections::{BTreeMap, HashMap};
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
    /// Explicit nonempty native extension default. Empty restores the saved engine default;
    /// PrepareExtensions can explicitly clear it. Root sessions pin their first selection.
    pub extensions: Vec<ExtensionSpec>,
    /// Explicitly clear the sticky default at startup (mutually exclusive with extensions).
    pub clear_extensions: bool,
}

pub use pipkin_engine_protocol::ExtensionSpec;

/// Parse `builtin:<name>` or a path to an executable (its file stem is the id).
pub fn parse_extension(raw: &str) -> Result<ExtensionSpec, String> {
    if let Some(name) = raw.strip_prefix("builtin:") {
        return match name {
            "questions" | "greeting" => Ok(ExtensionSpec::Builtin(name.to_owned())),
            other => Err(format!(
                "unknown built-in extension {other:?} (questions, greeting)"
            )),
        };
    }
    let path = PathBuf::from(raw);
    if !path.is_file() {
        return Err(format!("extension {raw} is not a file"));
    }
    let id = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| raw.to_owned());
    let path =
        std::fs::canonicalize(&path).map_err(|e| format!("cannot resolve extension {raw}: {e}"))?;
    Ok(ExtensionSpec::Process {
        id,
        argv: vec![
            path.to_str()
                .ok_or_else(|| "extension path is not UTF-8".to_owned())?
                .to_owned(),
        ],
    })
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
            extensions: vec![],
            clear_extensions: false,
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
    /// Per-model token settings keyed by exact `provider/modelId` (reference
    /// `CompactionModelOverride`, `settings-manager.ts:23-33`).
    pub model_overrides: BTreeMap<String, CompactionOverride>,
}

/// `compaction.modelOverrides["provider/id"]`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct CompactionOverride {
    pub reserve_tokens: Option<u64>,
    pub keep_recent_tokens: Option<u64>,
}

impl Default for CompactionSettings {
    fn default() -> Self {
        CompactionSettings {
            enabled: true,
            reserve_tokens: 16_384,
            keep_recent_tokens: 20_000,
            background_tokens: 32_768,
            model_overrides: BTreeMap::new(),
        }
    }
}

impl CompactionSettings {
    /// The effective settings for one model: its override, then the ordinary setting, then the
    /// default, per token field (reference `getCompactionTokenSetting`,
    /// `settings-manager.ts:928-960`). Keys match exactly (`provider/modelId`).
    pub fn for_model(&self, key: &str) -> CompactionSettings {
        let mut out = self.clone();
        if let Some(o) = self.model_overrides.get(key) {
            if let Some(v) = o.reserve_tokens {
                out.reserve_tokens = v;
            }
            if let Some(v) = o.keep_recent_tokens {
                out.keep_recent_tokens = v;
            }
        }
        out
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
        // Each file is migrated on its own before the merge, as the reference loads them.
        let mut merged = read_json(&config_dir.join("settings.json"), &mut warnings);
        migrate(&mut merged);
        if let Some(cwd) = cwd {
            let mut project = read_json(&cwd.join(".pi").join("settings.json"), &mut warnings);
            migrate(&mut project);
            deep_merge(&mut merged, project);
        }
        match serde_json::from_value::<Settings>(merged) {
            Ok(s) => (s, warnings),
            Err(e) => {
                warnings.push(format!("settings.json is invalid ({e}); using defaults"));
                (Settings::default(), warnings)
            }
        }
    }

    /// Reference `getHttpIdleTimeoutMs`: `httpIdleTimeoutMs` (default 300 s; `0` or
    /// `"disabled"` disables it, `None`). A value the reference would reject keeps the default.
    pub fn http_idle_timeout(&self) -> Option<std::time::Duration> {
        let ms = self
            .http_idle_timeout_ms
            .as_ref()
            .and_then(parse_timeout_ms)
            .unwrap_or(DEFAULT_HTTP_IDLE_TIMEOUT_MS);
        (ms > 0).then(|| std::time::Duration::from_millis(ms))
    }

    /// The provider request timeout (reference durable `stream.timeoutMs`,
    /// `harness-setup.ts:28-31`): `retry.provider.timeoutMs`, else the HTTP idle timeout. `None`
    /// means disabled.
    pub fn idle_timeout(&self) -> Option<std::time::Duration> {
        match self.retry.provider.timeout_ms {
            Some(ms) => (ms > 0).then(|| std::time::Duration::from_millis(ms)),
            None => self.http_idle_timeout(),
        }
    }

    /// The per-model default thinking level (`modelThinkingLevels["provider/id"]`).
    pub fn model_thinking_level(&self, key: &str) -> Option<crate::provider::ThinkingLevel> {
        self.other
            .get("modelThinkingLevels")
            .and_then(|v| v.get(key))
            .and_then(Value::as_str)
            .and_then(crate::provider::ThinkingLevel::parse)
    }

    /// `defaultThinkingLevel`, when valid.
    pub fn default_thinking(&self) -> Option<crate::provider::ThinkingLevel> {
        self.default_thinking_level
            .as_deref()
            .and_then(crate::provider::ThinkingLevel::parse)
    }

    /// Persist a new global default model (the reference engine does this on selection).
    pub fn save_default_model(
        config_dir: &Path,
        provider: &str,
        model: &str,
    ) -> std::io::Result<()> {
        Settings::save_global(config_dir, |value| {
            value["defaultProvider"] = Value::String(provider.into());
            value["defaultModel"] = Value::String(model.into());
        })
    }

    /// Persist the global `enabledModels` patterns (reference `setEnabledModels`).
    pub fn save_enabled_models(config_dir: &Path, patterns: &[String]) -> std::io::Result<()> {
        Settings::save_global(config_dir, |value| {
            value["enabledModels"] = Value::from(patterns.to_vec());
        })
    }

    fn save_global(config_dir: &Path, edit: impl FnOnce(&mut Value)) -> std::io::Result<()> {
        let path = config_dir.join("settings.json");
        let mut warnings = vec![];
        let mut value = read_json(&path, &mut warnings);
        if !value.is_object() {
            value = Value::Object(Map::new());
        }
        edit(&mut value);
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

/// Reference `DEFAULT_HTTP_IDLE_TIMEOUT_MS`.
pub const DEFAULT_HTTP_IDLE_TIMEOUT_MS: u64 = 300_000;

/// Reference `parseHttpIdleTimeoutMs`: a non-negative finite number (floored), a numeric
/// string, or `"disabled"` (any case, trimmed) for `0`.
pub fn parse_timeout_ms(value: &Value) -> Option<u64> {
    let number = |f: f64| (f.is_finite() && f >= 0.0).then(|| f.floor() as u64);
    match value {
        Value::String(s) => {
            let t = s.trim();
            if t.eq_ignore_ascii_case("disabled") {
                Some(0)
            } else if t.is_empty() {
                None
            } else {
                t.parse::<f64>().ok().and_then(number)
            }
        }
        Value::Number(n) => n.as_f64().and_then(number),
        _ => None,
    }
}

/// The reference engine's load-time settings migrations (`settings-manager.ts:504-563`).
fn migrate(v: &mut Value) {
    let Some(obj) = v.as_object_mut() else { return };
    if obj.contains_key("queueMode") && !obj.contains_key("steeringMode") {
        let q = obj.remove("queueMode").expect("checked");
        obj.insert("steeringMode".into(), q);
    }
    // Legacy `websockets` boolean -> `transport`.
    if !obj.contains_key("transport")
        && let Some(Value::Bool(ws)) = obj.get("websockets").cloned()
    {
        obj.insert(
            "transport".into(),
            Value::String(if ws { "websocket" } else { "sse" }.into()),
        );
        obj.remove("websockets");
    }
    // Old `skills: {enableSkillCommands, customDirectories}` object -> `skills` array.
    if let Some(Value::Object(skills)) = obj.get("skills").cloned() {
        if let Some(enable) = skills.get("enableSkillCommands")
            && !obj.contains_key("enableSkillCommands")
        {
            obj.insert("enableSkillCommands".into(), enable.clone());
        }
        match skills.get("customDirectories") {
            Some(Value::Array(dirs)) if !dirs.is_empty() => {
                obj.insert("skills".into(), Value::Array(dirs.clone()));
            }
            _ => {
                obj.remove("skills");
            }
        }
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
    use serde_json::json;

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

    fn load_global(json: &str) -> Settings {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("settings.json"), json).unwrap();
        let (s, w) = Settings::load(dir.path(), None);
        assert!(w.is_empty(), "{w:?}");
        s
    }

    #[test]
    fn legacy_websockets_and_skills_object_are_migrated() {
        let s = load_global(
            r#"{"websockets": true, "skills": {"enableSkillCommands": false,
                "customDirectories": ["/a", "b"]}}"#,
        );
        assert_eq!(s.other.get("transport"), Some(&json!("websocket")));
        assert!(!s.other.contains_key("websockets"));
        assert_eq!(s.skills, vec!["/a".to_owned(), "b".to_owned()]);
        assert_eq!(s.other.get("enableSkillCommands"), Some(&json!(false)));

        let s = load_global(r#"{"websockets": false, "skills": {"customDirectories": []}}"#);
        assert_eq!(s.other.get("transport"), Some(&json!("sse")));
        assert!(s.skills.is_empty());

        // An explicit `transport` wins and the legacy key is left alone; same for queueMode.
        let s = load_global(
            r#"{"websockets": true, "transport": "sse", "queueMode": "all",
                "steeringMode": "one-at-a-time"}"#,
        );
        assert_eq!(s.other.get("transport"), Some(&json!("sse")));
        assert_eq!(s.other.get("websockets"), Some(&json!(true)));
        assert_eq!(s.steering_mode, QueueDelivery::OneAtATime);
    }

    #[test]
    fn project_files_are_migrated_before_the_merge() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("proj");
        std::fs::create_dir_all(cwd.join(".pi")).unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"steeringMode": "one-at-a-time"}"#,
        )
        .unwrap();
        std::fs::write(cwd.join(".pi/settings.json"), r#"{"queueMode": "all"}"#).unwrap();
        let (s, _) = Settings::load(dir.path(), Some(&cwd));
        assert_eq!(s.steering_mode, QueueDelivery::All);
    }

    #[test]
    fn compaction_model_overrides_apply_per_field_by_exact_key() {
        let s = load_global(
            r#"{"compaction": {"reserveTokens": 1000, "modelOverrides": {
                "p/big": {"reserveTokens": 50000, "keepRecentTokens": 70000},
                "p/half": {"keepRecentTokens": 3}}}}"#,
        );
        let big = s.compaction.for_model("p/big");
        assert_eq!(
            (big.reserve_tokens, big.keep_recent_tokens),
            (50_000, 70_000)
        );
        let half = s.compaction.for_model("p/half");
        assert_eq!((half.reserve_tokens, half.keep_recent_tokens), (1000, 3));
        let other = s.compaction.for_model("P/BIG");
        assert_eq!(
            (other.reserve_tokens, other.keep_recent_tokens),
            (1000, 20_000)
        );
    }

    #[test]
    fn http_idle_timeout_parses_like_the_reference_and_the_provider_timeout_wins() {
        let secs = |s: &Settings| s.http_idle_timeout().map(|d| d.as_millis());
        assert_eq!(secs(&load_global("{}")), Some(300_000));
        assert_eq!(secs(&load_global(r#"{"httpIdleTimeoutMs": 0}"#)), None);
        assert_eq!(
            secs(&load_global(r#"{"httpIdleTimeoutMs": " Disabled "}"#)),
            None
        );
        assert_eq!(
            secs(&load_global(r#"{"httpIdleTimeoutMs": "120000"}"#)),
            Some(120_000)
        );
        assert_eq!(
            secs(&load_global(r#"{"httpIdleTimeoutMs": 1500.9}"#)),
            Some(1500)
        );
        assert_eq!(
            secs(&load_global(r#"{"httpIdleTimeoutMs": -5}"#)),
            Some(300_000)
        );
        let s =
            load_global(r#"{"httpIdleTimeoutMs": 0, "retry": {"provider": {"timeoutMs": 7000}}}"#);
        assert_eq!(s.idle_timeout(), Some(std::time::Duration::from_secs(7)));
        assert_eq!(s.http_idle_timeout(), None);
        let s = load_global(r#"{"httpIdleTimeoutMs": 60000}"#);
        assert_eq!(s.idle_timeout(), Some(std::time::Duration::from_secs(60)));
        let s = load_global(r#"{"httpIdleTimeoutMs": "disabled"}"#);
        assert_eq!(s.idle_timeout(), None);
    }

    #[test]
    fn isolated_env_hides_the_process_environment() {
        let env = Env::isolated(HashMap::from([("A".into(), "1".into())]));
        assert_eq!(env.get("A").as_deref(), Some("1"));
        assert_eq!(env.get("PATH"), None);
    }
}
