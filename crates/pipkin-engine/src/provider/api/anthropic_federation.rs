//! Anthropic workload identity federation. Port of @anthropic-ai/sdk 0.129.0
//! lib/credentials/{oidc-federation,identity-token,token-cache,types}.ts.
//! Copyright 2023 Anthropic, PBC. MIT; see packaging/upstream-notices/anthropic-sdk/LICENSE.
//! Only Pi's environment/file registration is exposed, not SDK credential profiles.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::sync::watch;
use tokio::time::Instant;

use crate::provider::{ProviderRequest, StreamCtx};

pub(super) const OAUTH_BETA: &str = "oauth-2025-04-20";
const ADVISORY: Duration = Duration::from_secs(120);
const MANDATORY: Duration = Duration::from_secs(30);
const BACKOFF: Duration = Duration::from_secs(5);
const MAX_BODY: usize = 1024 * 1024;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Config {
    base: String,
    rule: String,
    organization: String,
    file: String,
    service_account: Option<String>,
    workspace: Option<String>,
}

impl Config {
    pub(super) fn from_request(req: &ProviderRequest) -> Option<Self> {
        if req.model.provider != "anthropic" {
            return None;
        }
        let value = |name| req.env_var(name).filter(|s| !s.is_empty());
        let base = req.base_url().trim().trim_end_matches('/');
        Some(Self {
            base: if base.is_empty() {
                super::DEFAULT_BASE_URL.into()
            } else {
                base.into()
            },
            rule: value("ANTHROPIC_FEDERATION_RULE_ID")?,
            organization: value("ANTHROPIC_ORGANIZATION_ID")?,
            file: value("ANTHROPIC_IDENTITY_TOKEN_FILE")?,
            service_account: value("ANTHROPIC_SERVICE_ACCOUNT_ID"),
            workspace: value("ANTHROPIC_WORKSPACE_ID"),
        })
    }
}

#[derive(Clone)]
struct Token {
    value: String,
    expires: Instant,
}
type Refresh = watch::Receiver<Option<Result<Token, String>>>;

#[derive(Default)]
struct Data {
    cached: Option<Token>,
    pending: Option<Refresh>,
    advisory_error: Option<Instant>,
    epoch: u64,
}

#[derive(Default)]
struct State {
    data: Mutex<Data>,
}

/// The pinned Pi adapter retains one config/client. Old in-flight requests keep their
/// own state, so switching a workspace cannot publish a token into its replacement.
#[derive(Default)]
pub(super) struct Cache {
    current: Mutex<Option<(Config, Arc<State>)>>,
}

pub(super) struct Lease {
    pub value: String,
    state: Arc<State>,
}

impl Lease {
    pub fn observe(&self, status: reqwest::StatusCode) {
        if status != reqwest::StatusCode::UNAUTHORIZED {
            return;
        }
        let mut data = self.state.data.lock().unwrap_or_else(|e| e.into_inner());
        // A late 401 for an older request must not erase a newer successful refresh.
        if data
            .cached
            .as_ref()
            .is_some_and(|token| token.value == self.value)
        {
            data.cached = None;
            data.pending = None;
            data.epoch += 1;
        }
    }
}

impl Cache {
    pub async fn get(&self, config: Config, ctx: &StreamCtx<'_>) -> Result<Lease, String> {
        if ctx.cancel.is_cancelled() {
            return Err("Request aborted".into());
        }
        let state = {
            let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
            if current.as_ref().is_none_or(|(key, _)| key != &config) {
                *current = Some((config.clone(), Arc::default()));
            }
            current.as_ref().unwrap().1.clone()
        };
        let mut pending = {
            let mut data = state.data.lock().unwrap_or_else(|e| e.into_inner());
            let cached = data.cached.clone();
            let remaining = cached
                .as_ref()
                .map(|t| t.expires.saturating_duration_since(Instant::now()));
            if remaining.is_some_and(|left| left > ADVISORY) {
                return Ok(Lease {
                    value: cached.unwrap().value,
                    state: state.clone(),
                });
            }
            let advisory = remaining.is_some_and(|left| left > MANDATORY);
            if data.pending.is_none()
                && (!advisory || data.advisory_error.is_none_or(|at| at.elapsed() >= BACKOFF))
            {
                let (tx, rx) = watch::channel(None);
                data.pending = Some(rx);
                let epoch = data.epoch;
                let shared = state.clone();
                let http = ctx.http.clone();
                let budget = if ctx.req.timeout.is_zero() {
                    MANDATORY
                } else {
                    ctx.req.timeout.min(MANDATORY)
                };
                // The refresh belongs to the cache, not its first caller. Canceling a
                // caller stops its wait; concurrent/advisory users still get the result.
                tokio::spawn(async move {
                    let result = match tokio::time::timeout(budget, exchange(&config, &http)).await
                    {
                        Ok(result) => result,
                        Err(_) => Err("Anthropic federation token exchange timed out".into()),
                    };
                    let mut data = shared.data.lock().unwrap_or_else(|e| e.into_inner());
                    if data.epoch == epoch {
                        data.pending = None;
                        match &result {
                            Ok(token) => {
                                data.cached = Some(token.clone());
                                data.advisory_error = None;
                            }
                            Err(error) if advisory => {
                                data.advisory_error = Some(Instant::now());
                                log::warn!("Anthropic federation advisory refresh failed: {error}");
                            }
                            _ => {}
                        }
                    }
                    let _ = tx.send(Some(result));
                });
            }
            if advisory {
                return Ok(Lease {
                    value: cached.unwrap().value,
                    state: state.clone(),
                });
            }
            data.pending
                .as_ref()
                .expect("mandatory refresh started")
                .clone()
        };
        loop {
            if let Some(result) = pending.borrow_and_update().clone() {
                return result.map(|token| Lease {
                    value: token.value,
                    state,
                });
            }
            tokio::select! {
                _ = ctx.cancel.cancelled() => return Err("Request aborted".into()),
                changed = pending.changed() => if changed.is_err() {
                    return Err("Anthropic federation refresh stopped".into());
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "anthropic_federation_tests.rs"]
mod tests;

fn secure_endpoint(base: &str) -> Result<(), String> {
    let url = url::Url::parse(base).map_err(|_| "Invalid Anthropic federation token endpoint")?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if url.scheme() == "https" || (url.scheme() == "http" && loopback) {
        return Ok(());
    }
    Err("Refusing to send credential over non-HTTPS token endpoint".into())
}

fn safe_error(body: &[u8], assertion: &str) -> String {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return "non-JSON error body omitted".into();
    };
    let mut safe = serde_json::Map::new();
    for key in ["error", "error_description", "error_uri"] {
        if let Some(text) = value.get(key).and_then(Value::as_str) {
            let mut text = text.replace(assertion, "[redacted]");
            for secret in [
                "access_token",
                "refresh_token",
                "assertion",
                "identity_token",
            ] {
                if let Some(secret) = value
                    .get(secret)
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty())
                {
                    text = text.replace(secret, "[redacted]");
                }
            }
            safe.insert(key.into(), Value::String(text.chars().take(2000).collect()));
        }
    }
    Value::Object(safe).to_string()
}

async fn exchange(config: &Config, http: &reqwest::Client) -> Result<Token, String> {
    secure_endpoint(&config.base)?;
    if !tokio::fs::metadata(&config.file)
        .await
        .map_err(|e| format!("Failed to read identity token file: {e}"))?
        .is_file()
    {
        return Err("Identity token source must be a regular file".into());
    }
    let file = tokio::fs::File::open(&config.file)
        .await
        .map_err(|e| format!("Failed to read identity token file: {e}"))?;
    let mut bytes = Vec::new();
    file.take((MAX_BODY + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| format!("Failed to read identity token file: {e}"))?;
    if bytes.len() > MAX_BODY {
        return Err("Identity token file exceeds 1 MiB".into());
    }
    let text = String::from_utf8_lossy(&bytes);
    let assertion = text.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    if assertion.is_empty() {
        return Err("Identity token file is empty".into());
    }
    if assertion.encode_utf16().count() > 16 * 1024 {
        return Err("Identity token exceeds the 16 KiB assertion limit".into());
    }
    let mut body = json!({"grant_type":"urn:ietf:params:oauth:grant-type:jwt-bearer",
        "assertion":assertion,"federation_rule_id":config.rule,"organization_id":config.organization});
    if let Some(value) = &config.service_account {
        body["service_account_id"] = json!(value);
    }
    if let Some(value) = &config.workspace {
        body["workspace_id"] = json!(value);
    }
    let mut response = http
        .post(format!("{}/v1/oauth/token", config.base))
        .header(
            "anthropic-beta",
            format!("{OAUTH_BETA},oidc-federation-2026-04-01"),
        )
        .header(
            "user-agent",
            concat!("pipkin/", env!("CARGO_PKG_VERSION"), " oidcFederation"),
        )
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            format!(
                "Failed to reach Anthropic federation token endpoint: {}",
                e.without_url()
            )
        })?;
    let status = response.status();
    let request_id = response
        .headers()
        .get("request-id")
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            format!(
                " (request-id {})",
                v.replace(assertion, "[redacted]")
                    .chars()
                    .take(200)
                    .collect::<String>()
            )
        })
        .unwrap_or_default();
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Could not read Anthropic federation token response")?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_BODY {
            return Err("Anthropic federation token response exceeds 1 MiB".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        let hint = if status == reqwest::StatusCode::UNAUTHORIZED && config.workspace.is_none() {
            " Check the federation rule; set ANTHROPIC_WORKSPACE_ID if it covers multiple workspaces."
        } else {
            ""
        };
        return Err(format!(
            "Token exchange failed with status {}{request_id}: {}{hint}",
            status.as_u16(),
            safe_error(&bytes, assertion)
        ));
    }
    let data: Value =
        serde_json::from_slice(&bytes).map_err(|_| "Token endpoint returned non-JSON response")?;
    let value = data
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("Token endpoint response missing access_token")?;
    if data
        .get("token_type")
        .and_then(Value::as_str)
        .is_some_and(|kind| !kind.is_empty() && !kind.eq_ignore_ascii_case("bearer"))
    {
        return Err("Token endpoint response has unsupported token_type (want Bearer)".into());
    }
    let expires = data
        .get("expires_in")
        .and_then(|v| v.as_f64().or_else(|| v.as_str()?.trim().parse().ok()))
        .filter(|v| v.is_finite())
        .ok_or("Token endpoint response missing finite expires_in")?;
    let lifetime = Duration::try_from_secs_f64(expires.max(0.0))
        .map_err(|_| "Token expiry is out of range")?;
    let expires = Instant::now()
        .checked_add(lifetime)
        .ok_or("Token expiry is out of range")?;
    Ok(Token {
        value: value.into(),
        expires,
    })
}
