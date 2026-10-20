//! Fixed, version-qualified chat-only launch policy. No shell, arbitrary CLI flags, tools,
//! credential loading or inferred billing promise. Keep private environment values out of Debug.
use super::process::{OutputMode, Spec};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    time::Duration,
};

// Historical live qualification baseline, not an upper compatibility bound.
pub const QUALIFIED_VERSION: &str = "2.1.299";
pub const MINIMUM_SUPPORTED_VERSION: &str = "2.1.299";
pub const VERSION_FAILURE: &str =
    "CLI version is below the supported minimum or malformed; no model request attempted";
pub const POLICY_ID: &str = "safe-restricted-no-tools-v1";
pub const MAX_PROMPT_BYTES: usize = 256 * 1024;
pub const TURN_TIMEOUT: Duration = Duration::from_secs(180);
pub type Environment = BTreeMap<OsString, OsString>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelAlias {
    Haiku,
    Sonnet,
    Opus,
}
impl ModelAlias {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Haiku => "haiku",
            Self::Sonnet => "sonnet",
            Self::Opus => "opus",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnMode {
    Fresh,
    Resume,
}

pub const REQUIRED_FLAGS: &[&str] = &[
    "--mcp-config",
    "--safe-mode",
    "--restricted",
    "--strict-mcp-config",
    "--print",
    "--tools",
    "--disallowedTools",
    "--permission-mode",
    "--permission-prompts",
    "--input-format",
    "--output-format",
    "--verbose",
    "--include-partial-messages",
    "--no-chrome",
    "--disable-slash-commands",
    "--model",
    "--session-id",
    "--resume",
];

/// Detect conflicts BEFORE narrowing the environment: never silently strip a billing override
/// and then advertise subscription routing. Unknown CLI-specific overrides fail closed too.
/// Values of conflict variables are neither read nor included in errors.
pub fn launch_environment(source: &Environment) -> Result<Environment, &'static str> {
    for key in source.keys() {
        let Some(key) = key.to_str() else {
            return Err("non-Unicode environment variable name");
        };
        if key.starts_with("ANTHROPIC_")
            || key.starts_with("CLAUDE_CODE_")
            // Broker bindings may only be injected by the owning run after preflight.
            || key.starts_with("PIPKIN_CLI_PERMISSION_")
            || key.starts_with("PIPKIN_CLI_NATIVE_TOOL_")
            || matches!(
                key,
                "CLAUDECODE"
                    | "CLAUDE_AGENT_SDK_VERSION"
                    | "NODE_OPTIONS"
                    | "BUN_OPTIONS"
                    | "LD_PRELOAD"
                    | "LD_LIBRARY_PATH"
                    | "DYLD_INSERT_LIBRARIES"
                    | "DYLD_LIBRARY_PATH"
                    | "BASH_ENV"
                    | "ENV"
            )
        {
            return Err(
                "CLI routing, nested-session or executable environment overrides must be resolved explicitly",
            );
        }
    }
    let mut env = Environment::new();
    for name in [
        "HOME",
        "PATH",
        "USER",
        "LOGNAME",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "TMPDIR",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
        "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS",
        "SSH_AUTH_SOCK",
        "CLAUDE_CONFIG_DIR",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "NODE_EXTRA_CA_CERTS",
        "REQUESTS_CA_BUNDLE",
        "CURL_CA_BUNDLE",
    ] {
        if let Some(value) = source.get(OsStr::new(name)) {
            env.insert(name.into(), value.clone());
        }
    }
    for name in [
        "HOME",
        "CLAUDE_CONFIG_DIR",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
    ] {
        if let Some(value) = env.get(OsStr::new(name))
            && !Path::new(value).is_absolute()
        {
            return Err("CLI home/configuration directories must be absolute");
        }
    }
    if !env.contains_key(OsStr::new("HOME")) {
        return Err("CLI HOME is unavailable");
    }
    if env
        .get(OsStr::new("PATH"))
        .is_none_or(|path| std::env::split_paths(path).any(|p| !p.is_absolute()))
    {
        return Err("CLI PATH must contain only absolute directories");
    }
    Ok(env)
}

/// Resolve only filesystem metadata, not a login shell or executable. Relative PATH entries
/// are ignored. The returned path is not a trust decision and must still be approved.
pub fn resolve(executable: Option<&Path>, env: &Environment) -> Result<PathBuf, &'static str> {
    let usable = |p: &Path| {
        if !p.is_absolute() {
            return false;
        }
        let Ok(meta) = p.metadata() else {
            return false;
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            meta.is_file() && meta.permissions().mode() & 0o111 != 0
        }
        #[cfg(not(unix))]
        {
            meta.is_file()
        }
    };
    if let Some(path) = executable {
        if !usable(path) {
            return Err("select an absolute executable file");
        }
        return path
            .canonicalize()
            .map_err(|_| "cannot resolve selected executable");
    }
    for dir in std::env::split_paths(
        env.get(OsStr::new("PATH"))
            .ok_or("select the CLI executable; PATH is unavailable")?,
    ) {
        let path = dir.join("claude");
        if usable(&path) {
            return path
                .canonicalize()
                .map_err(|_| "cannot resolve discovered executable");
        }
    }
    Err("Claude CLI was not found in absolute PATH entries")
}

pub fn valid_uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
            }
        })
}

pub fn validate_version(bytes: &[u8]) -> Result<(), &'static str> {
    supported_version(bytes).map(|_| ())
}

/// Accept stable numeric releases at or above the minimum, retaining the actual observed
/// version for immutable bindings. Capability/login checks remain independently required.
pub fn supported_version(bytes: &[u8]) -> Result<String, &'static str> {
    fn numeric(version: &str) -> Option<(u64, u64, u64)> {
        let mut parts = version.split('.');
        let mut component = || {
            let part = parts.next()?;
            if part.is_empty()
                || (part.len() > 1 && part.starts_with('0'))
                || !part.bytes().all(|b| b.is_ascii_digit())
            {
                return None;
            }
            part.parse::<u64>().ok()
        };
        let parsed = (component()?, component()?, component()?);
        if parts.next().is_some() {
            return None;
        }
        Some(parsed)
    }
    if bytes.len() > 128 {
        return Err(VERSION_FAILURE);
    }
    let reported = std::str::from_utf8(bytes)
        .ok()
        .map(str::trim)
        .and_then(|s| s.strip_suffix(" (Claude Code)"))
        .ok_or(VERSION_FAILURE)?;
    let version = numeric(reported).ok_or(VERSION_FAILURE)?;
    let minimum = numeric(MINIMUM_SUPPORTED_VERSION).expect("valid minimum CLI version");
    if version < minimum {
        return Err(VERSION_FAILURE);
    }
    Ok(reported.to_owned())
}
pub fn validate_help(bytes: &[u8]) -> Result<(), &'static str> {
    require_flags(bytes, REQUIRED_FLAGS)
}
pub(super) fn require_flags(bytes: &[u8], flags: &[&str]) -> Result<(), &'static str> {
    let help = std::str::from_utf8(bytes).map_err(|_| "invalid CLI help response")?;
    if flags.iter().all(|flag| {
        help.lines().any(|line| {
            line.trim_start().starts_with('-')
                && line
                    .split_whitespace()
                    .any(|token| token.trim_end_matches(',') == *flag)
        })
    }) {
        Ok(())
    } else {
        Err("required CLI safety flags are missing")
    }
}

pub(super) fn safety_args() -> Vec<OsString> {
    [
        "--mcp-config",
        "{\"mcpServers\":{}}",
        "--safe-mode",
        "--restricted",
        "--strict-mcp-config",
    ]
    .into_iter()
    .map(Into::into)
    .collect()
}

pub(super) fn metadata_spec(
    executable: &Path,
    cwd: &Path,
    env: &Environment,
    args: Vec<OsString>,
) -> Spec {
    Spec {
        executable: executable.into(),
        cwd: cwd.into(),
        env: env.clone(),
        args,
        input: vec![],
        mode: OutputMode::Capture,
        timeout: Some(Duration::from_secs(10)),
        grace: Duration::from_millis(300),
        max_stdout: 128 * 1024,
        max_stderr: 128 * 1024,
    }
}

pub(super) fn turn_spec(
    executable: &Path,
    cwd: &Path,
    env: &Environment,
    uuid: &str,
    mode: TurnMode,
    model: ModelAlias,
    prompt: &str,
) -> Result<Spec, &'static str> {
    if !valid_uuid(uuid) {
        return Err("invalid explicit CLI session identifier");
    }
    if prompt.trim().is_empty() || prompt.len() > MAX_PROMPT_BYTES || prompt.contains('\0') {
        return Err("CLI prompt is empty, oversized or contains NUL");
    }
    let mut args = safety_args();
    args.extend(
        [
            "--print",
            "--tools",
            "",
            "--disallowedTools",
            "mcp__*",
            "--permission-mode",
            "dontAsk",
            "--permission-prompts",
            "none",
            "--input-format",
            "text",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--no-chrome",
            "--disable-slash-commands",
            "--model",
            model.as_str(),
            if mode == TurnMode::Resume {
                "--resume"
            } else {
                "--session-id"
            },
            uuid,
        ]
        .into_iter()
        .map(OsString::from),
    );
    let mut spec = metadata_spec(executable, cwd, env, args);
    spec.input = prompt.as_bytes().to_vec();
    spec.input.push(b'\n');
    spec.mode = OutputMode::Stream;
    spec.timeout = Some(TURN_TIMEOUT);
    spec.max_stdout = 16 * 1024 * 1024;
    Ok(spec)
}

/// A nonsecret compatibility marker, not an authorization or filesystem integrity proof.
/// Length-prefixing prevents ambiguous concatenations; private values are never returned.
pub(super) fn marker(parts: impl IntoIterator<Item = impl AsRef<[u8]>>) -> String {
    let mut hash = Sha256::new();
    for part in parts {
        let b = part.as_ref();
        hash.update((b.len() as u64).to_be_bytes());
        hash.update(b);
    }
    format!("{:x}", hash.finalize())
}

#[cfg(test)]
mod tests;
