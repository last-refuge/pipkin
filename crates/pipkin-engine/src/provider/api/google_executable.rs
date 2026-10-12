//! Explicitly enabled Google ADC credential helper processes (no shell expansion).
use super::{BUDGET, LIMIT, option, required, string};
use crate::provider::{ISOLATED_ENV, ProviderRequest};
use serde_json::Value;
use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;

const ALLOW: &str = "GOOGLE_EXTERNAL_ACCOUNT_ALLOW_EXECUTABLES";

fn timeout(config: &Value) -> Result<Duration, String> {
    let millis = match option(config, "timeout_millis") {
        Value::Null => 30_000,
        value => value.as_u64().ok_or("Invalid Google executable timeout")?,
    };
    if !(5_000..=120_000).contains(&millis) {
        return Err("Google executable timeout must be between 5000 and 120000 ms".into());
    }
    Ok(Duration::from_millis(millis))
}

// Match the SDK's double-quote grouping, including literal single quotes and backslashes.
// This is deliberately not shell parsing: $, pipes, substitutions and redirects are arguments.
fn command(text: &str) -> Result<Vec<String>, String> {
    static PARTS: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let parts: Vec<_> = PARTS
        .get_or_init(|| regex::Regex::new(r#"(?:[^\s"]+|"[^"]*")+"#).unwrap())
        .find_iter(text)
        .map(|m| {
            let s = m.as_str();
            s.strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or(s)
                .to_owned()
        })
        .collect();
    if parts.first().is_none_or(String::is_empty) {
        return Err("Google executable command is empty".into());
    }
    Ok(parts)
}

pub(super) fn validate(config: &Value) -> Result<(), String> {
    command(&required(config, "command")?)?;
    timeout(config)?;
    Ok(())
}

pub(in super::super) fn execution_context(
    req: &ProviderRequest,
    document: Option<&Value>,
) -> (BTreeMap<String, String>, Duration) {
    let mut doc = document;
    while let Some(value) = doc.filter(|d| d["type"] == "impersonated_service_account") {
        doc = value.get("source_credentials");
    }
    let config = doc.map(|d| option(option(d, "credential_source"), "executable"));
    if let Some(config) = config.filter(|c| !c.is_null()) {
        // Helpers may use arbitrary environment variables. Snapshot only their environment,
        // never the model request/history, and include it in the identity cache key.
        let mut env = BTreeMap::new();
        if req.options.get(ISOLATED_ENV) != Some(&Value::Bool(true)) {
            env.extend(
                std::env::vars_os()
                    .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))),
            );
        }
        env.extend(req.env.iter().map(|(k, v)| (k.clone(), v.clone())));
        // The configured helper budget is additional to the bounded network exchange.
        return (env, BUDGET + timeout(config).unwrap_or(BUDGET));
    }
    (BTreeMap::new(), BUDGET)
}

struct Reply {
    version: u64,
    token: Option<String>,
    expiration: Option<u64>,
}
impl Reply {
    fn parse(bytes: &[u8]) -> Result<Self, String> {
        let v: Value =
            serde_json::from_slice(bytes).map_err(|_| "Google executable returned invalid JSON")?;
        let version = v["version"]
            .as_u64()
            .filter(|v| *v > 0)
            .ok_or("Invalid Google executable response version")?;
        let success = v["success"]
            .as_bool()
            .ok_or("Missing Google executable success flag")?;
        let token = if success {
            let key = match v["token_type"].as_str() {
                Some(
                    "urn:ietf:params:oauth:token-type:jwt"
                    | "urn:ietf:params:oauth:token-type:id_token",
                ) => "id_token",
                Some("urn:ietf:params:oauth:token-type:saml2") => "saml_response",
                _ => return Err("Invalid Google executable token type".into()),
            };
            Some(required(&v, key)?)
        } else {
            required(&v, "code")?;
            required(&v, "message")?;
            None
        };
        let expiration = if success && !v["expiration_time"].is_null() {
            Some(
                v["expiration_time"]
                    .as_u64()
                    .ok_or("Invalid Google executable expiration")?,
            )
        } else {
            None
        };
        Ok(Self {
            version,
            token,
            expiration,
        })
    }
    fn expired(&self) -> bool {
        let rounded_now = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            + 500)
            / 1000;
        self.expiration.is_some_and(|e| u128::from(e) < rounded_now)
    }
}

async fn cached(path: &str) -> Result<Option<Reply>, String> {
    // Missing/unresolvable and non-file paths are cache misses in the SDK.
    let Ok(meta) = tokio::fs::metadata(path).await else {
        return Ok(None);
    };
    if !meta.is_file() {
        return Ok(None);
    }
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| "Cannot read Google executable output file")?;
    let mut bytes = Vec::new();
    file.take(LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| "Cannot read Google executable output file")?;
    if bytes.len() > LIMIT {
        return Err("Google executable output file is too large".into());
    }
    if bytes.is_empty() {
        return Ok(None);
    }
    let reply = Reply::parse(&bytes)?;
    Ok((reply.token.is_some() && !reply.expired()).then_some(reply))
}

// A canceled/expired owner must also terminate descendants holding output pipes open.
// On timeout the child has not been reaped, so the group ID cannot be reused.
struct ProcessGuard(Option<u32>);
impl Drop for ProcessGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(id) = self.0 {
            // SAFETY: only the process group created below for this credential helper.
            unsafe {
                libc::kill(-(id as libc::pid_t), libc::SIGKILL);
            }
        }
    }
}

async fn run(config: &Value, env: BTreeMap<String, String>) -> Result<Reply, String> {
    let args = command(&required(config, "command")?)?;
    let mut cmd = tokio::process::Command::new(&args[0]);
    cmd.args(&args[1..])
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd
        .spawn()
        .map_err(|_| "Google executable could not start")?;
    let mut guard = ProcessGuard(child.id());
    let mut stdout = child
        .stdout
        .take()
        .ok_or("Google executable has no stdout")?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or("Google executable has no stderr")?;
    let work = async {
        let mut bytes = Vec::new();
        let (mut out, mut err) = (true, true);
        let (mut a, mut b) = ([0u8; 8192], [0u8; 8192]);
        while out || err {
            let chunk = tokio::select! {
                r = stdout.read(&mut a), if out => {
                    let n = r.map_err(|_| "Cannot read Google executable output")?;
                    out = n != 0; &a[..n]
                }
                r = stderr.read(&mut b), if err => {
                    let n = r.map_err(|_| "Cannot read Google executable output")?;
                    err = n != 0; &b[..n]
                }
            };
            if bytes.len().saturating_add(chunk.len()) > LIMIT {
                return Err("Google executable output is too large".to_owned());
            }
            bytes.extend_from_slice(chunk);
        }
        let status = child.wait().await;
        guard.0 = None;
        let status = status.map_err(|_| "Cannot wait for Google executable")?;
        if !status.success() {
            return Err("Google executable exited unsuccessfully".into());
        }
        Reply::parse(&bytes)
    };
    tokio::time::timeout(timeout(config)?, work)
        .await
        .map_err(|_| "Google executable timed out".to_owned())?
}

pub(super) async fn subject(
    doc: &Value,
    environment: &BTreeMap<String, String>,
) -> Result<String, String> {
    if environment.get(ALLOW).map(String::as_str) != Some("1") {
        return Err(format!("Google executable credentials require {ALLOW}=1"));
    }
    let config = option(option(doc, "credential_source"), "executable");
    let output = string(config, "output_file");
    let cached = match &output {
        Some(path) => cached(path).await?,
        None => None,
    };
    let reply = match cached {
        Some(reply) => reply,
        None => {
            let mut env = environment.clone();
            env.insert(
                "GOOGLE_EXTERNAL_ACCOUNT_AUDIENCE".into(),
                required(doc, "audience")?,
            );
            env.insert(
                "GOOGLE_EXTERNAL_ACCOUNT_TOKEN_TYPE".into(),
                required(doc, "subject_token_type")?,
            );
            env.insert("GOOGLE_EXTERNAL_ACCOUNT_INTERACTIVE".into(), "0".into());
            if let Some(path) = &output {
                env.insert("GOOGLE_EXTERNAL_ACCOUNT_OUTPUT_FILE".into(), path.clone());
            }
            if let Some(url) = string(doc, "service_account_impersonation_url") {
                if url.len() > 256 {
                    return Err("Google executable impersonation URL is too long".into());
                }
                let principal = url
                    .split_once("/serviceAccounts/")
                    .and_then(|(_, v)| v.strip_suffix(":generateAccessToken"))
                    .filter(|v| !v.contains(':'));
                if let Some(email) = principal.filter(|s| !s.is_empty()) {
                    env.insert(
                        "GOOGLE_EXTERNAL_ACCOUNT_IMPERSONATED_EMAIL".into(),
                        email.into(),
                    );
                }
            }
            run(config, env).await?
        }
    };
    if reply.version > 1 {
        return Err("Unsupported Google executable response version".into());
    }
    if reply.token.is_none() {
        return Err("Google executable reported an authentication failure".into());
    }
    if output.is_some() && reply.expiration.is_none_or(|e| e == 0) {
        return Err("Google executable output-file response requires expiration_time".into());
    }
    if reply.expired() {
        return Err("Google executable response is expired".into());
    }
    Ok(reply.token.unwrap())
}

#[cfg(all(test, unix))]
#[path = "google_executable_tests.rs"]
mod tests;
