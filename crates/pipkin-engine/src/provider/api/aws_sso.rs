//! AWS CLI SSO cache → OIDC refresh (session profiles) → role credentials.
//! Wire/cache contracts are compared with the SDK pinned by Pi; no SDK runtime is needed.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

use super::{AwsCredentials, ChainInput, hex, home, now_secs, parse_ini, sha256_hex};

const LOGIN: &str = "Run aws sso login with the corresponding profile to refresh the SSO session.";
const LIMIT: u64 = 1024 * 1024;
const BUDGET: Duration = Duration::from_secs(30);

#[derive(Clone)]
struct Config {
    path: PathBuf,
    refreshable: bool,
    account: String,
    role: String,
    portal: String,
    oidc: String,
}

fn value(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn expiry(v: &Value) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(v.as_str()?)
        .ok()?
        .timestamp()
        .try_into()
        .ok()
}

fn token_path(home: &str, id: &str) -> PathBuf {
    let digest = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, id.as_bytes());
    PathBuf::from(home)
        .join(".aws/sso/cache")
        .join(format!("{}.json", hex(digest.as_ref())))
}

fn endpoint(input: &ChainInput<'_>, service: &str, default: String) -> Result<String, String> {
    let url = (input.env)(&format!("AWS_ENDPOINT_URL_{service}"))
        .or_else(|| (input.env)("AWS_ENDPOINT_URL"))
        .unwrap_or(default);
    let parsed = url::Url::parse(&url).map_err(|_| "Invalid IAM Identity Center endpoint")?;
    let loopback = matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if parsed.scheme() != "https" && !(parsed.scheme() == "http" && loopback) {
        return Err(
            "IAM Identity Center endpoints require HTTPS (HTTP is allowed on loopback)".into(),
        );
    }
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err("Invalid IAM Identity Center endpoint components".into());
    }
    Ok(url.trim_end_matches('/').to_owned())
}

fn config(input: &ChainInput<'_>, profile: &HashMap<String, String>) -> Result<Config, String> {
    let required = |p: &HashMap<String, String>, key: &str| {
        p.get(key)
            .filter(|s| !s.is_empty())
            .cloned()
            .ok_or_else(|| format!("IAM Identity Center profile is missing {key}. {LOGIN}"))
    };
    let home =
        home(input.env).ok_or("IAM Identity Center token cache needs HOME or USERPROFILE")?;
    let session = profile.get("sso_session").filter(|s| !s.is_empty());
    let (region, id) = if let Some(session) = session {
        let path = (input.env)("AWS_CONFIG_FILE").unwrap_or_else(|| format!("{home}/.aws/config"));
        let text = std::fs::read_to_string(path).map_err(|_| {
            format!("IAM Identity Center session configuration cannot be read. {LOGIN}")
        })?;
        // Keep sso-session sections separate from profiles; they must never become AWS_PROFILE candidates.
        let sections = parse_ini(&text, false);
        let section = sections
            .get(&format!("sso-session {session}"))
            .ok_or_else(|| format!("IAM Identity Center session {session} is missing. {LOGIN}"))?;
        required(section, "sso_start_url")?;
        (required(section, "sso_region")?, session.clone())
    } else {
        (
            required(profile, "sso_region")?,
            required(profile, "sso_start_url")?,
        )
    };
    if !region
        .bytes()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    {
        return Err("Invalid IAM Identity Center region".into());
    }
    let suffix = if region.starts_with("cn-") {
        "amazonaws.com.cn"
    } else {
        "amazonaws.com"
    };
    Ok(Config {
        path: token_path(&home, &id),
        refreshable: session.is_some(),
        account: required(profile, "sso_account_id")?,
        role: required(profile, "sso_role_name")?,
        portal: endpoint(
            input,
            "SSO",
            format!("https://portal.sso.{region}.{suffix}"),
        )?,
        oidc: endpoint(input, "SSO_OIDC", format!("https://oidc.{region}.{suffix}"))?,
    })
}

#[derive(Default)]
struct State {
    last_refresh: Option<Instant>,
    credentials: HashMap<String, AwsCredentials>,
}

/// Share refresh for every account/role using one session token. Evict idle states at a
/// fixed bound; never evict an in-flight owner and accidentally refresh its token twice.
fn state(path: &std::path::Path) -> Result<Arc<tokio::sync::Mutex<State>>, String> {
    type States = HashMap<PathBuf, Arc<tokio::sync::Mutex<State>>>;
    static STATES: OnceLock<Mutex<States>> = OnceLock::new();
    let mut map = STATES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(state) = map.get(path) {
        return Ok(state.clone());
    }
    if map.len() >= 64 {
        if let Some(key) = map
            .iter()
            .find(|(_, state)| Arc::strong_count(state) == 1)
            .map(|(key, _)| key.clone())
        {
            map.remove(&key);
        } else {
            return Err("Too many concurrent IAM Identity Center sessions".into());
        }
    }
    let state = Arc::new(tokio::sync::Mutex::new(State::default()));
    map.insert(path.to_owned(), state.clone());
    Ok(state)
}

pub(super) async fn resolve(
    input: &ChainInput<'_>,
    profile: &HashMap<String, String>,
) -> Result<AwsCredentials, String> {
    if input.cancel.is_cancelled() {
        return Err("Request was aborted".into());
    }
    let config = config(input, profile)?;
    let state = state(&config.path)?;
    let http = input.http.clone();
    // The owned exchange completes and persists rotating credentials even if one caller stops.
    // Both queueing and network/file work have a total budget.
    let mut task = tokio::spawn(async move {
        tokio::time::timeout(BUDGET, async {
            let mut state = state.lock().await;
            credentials(&http, &config, &mut state).await
        })
        .await
        .map_err(|_| "IAM Identity Center credential exchange timed out".to_owned())?
    });
    tokio::select! {
        _ = input.cancel.cancelled() => Err("Request was aborted".into()),
        result = &mut task => result.map_err(|_| "IAM Identity Center credential task failed".to_owned())?,
    }
}

async fn read_token(path: &std::path::Path) -> Result<Value, String> {
    let mut file = tokio::fs::File::open(path).await.map_err(|_| {
        format!("IAM Identity Center token cache is missing or unreadable. {LOGIN}")
    })?;
    if !file
        .metadata()
        .await
        .map_err(|_| "Cannot inspect IAM Identity Center token cache")?
        .is_file()
    {
        return Err("IAM Identity Center token cache is not a regular file".into());
    }
    let mut body = Vec::new();
    (&mut file)
        .take(LIMIT + 1)
        .read_to_end(&mut body)
        .await
        .map_err(|_| "Cannot read IAM Identity Center token cache")?;
    if body.len() as u64 > LIMIT {
        return Err("IAM Identity Center token cache is too large".into());
    }
    let token: Value = serde_json::from_slice(&body)
        .map_err(|_| format!("IAM Identity Center token cache is invalid JSON. {LOGIN}"))?;
    if !token.is_object() {
        return Err(format!(
            "IAM Identity Center token cache is not an object. {LOGIN}"
        ));
    }
    Ok(token)
}

async fn send(builder: reqwest::RequestBuilder) -> Result<Value, String> {
    let mut response = builder
        .timeout(BUDGET)
        .send()
        .await
        .map_err(|e| crate::provider::http::transport_error(&e))?;
    let status = response.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| crate::provider::http::transport_error(&e))?
    {
        if bytes.len().saturating_add(chunk.len()) > LIMIT as usize {
            return Err("IAM Identity Center response is too large".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        // Do not echo a token endpoint's response, which may contain submitted secrets.
        return Err(format!(
            "IAM Identity Center request failed (HTTP {}). {LOGIN}",
            status.as_u16()
        ));
    }
    serde_json::from_slice(&bytes).map_err(|_| "IAM Identity Center returned invalid JSON".into())
}

async fn persist(path: PathBuf, old: Value, new: Value) {
    // Reuse the existing private, locked, atomic JSON writer. Only replace a token still
    // matching the one refreshed; another profile login's values must not be overwritten.
    let result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        use crate::auth::store::{CredentialStore, FileStore, StoredCredential};
        let store = FileStore::new(path).with_lock_wait(Duration::from_secs(1));
        let mut txn = store
            .begin()
            .map_err(|_| "Cannot lock IAM Identity Center token cache")?;
        for key in ["accessToken", "refreshToken", "expiresAt"] {
            let expected = old
                .get(key)
                .map(|v| StoredCredential::Unsupported(v.clone()));
            if txn.get(key) != expected {
                return Ok(());
            }
        }
        for key in ["accessToken", "expiresAt", "refreshToken"] {
            if let Some(value) = new.get(key) {
                txn.set(key, StoredCredential::Unsupported(value.clone()));
            } else {
                txn.remove(key);
            }
        }
        txn.commit()
            .map_err(|_| "Cannot save IAM Identity Center token cache".to_owned())
    })
    .await;
    if !matches!(result, Ok(Ok(()))) {
        log::warn!("IAM Identity Center token refreshed, but its cache could not be saved");
    }
}

async fn access_token(
    http: &reqwest::Client,
    config: &Config,
    state: &mut State,
    old: Value,
) -> Result<String, String> {
    let existing = value(&old, "accessToken")
        .ok_or_else(|| format!("IAM Identity Center cache has no accessToken. {LOGIN}"))?;
    let expires = expiry(&old["expiresAt"])
        .ok_or_else(|| format!("IAM Identity Center cache has invalid expiresAt. {LOGIN}"))?;
    let now = now_secs();
    if expires > now.saturating_add(300) || !config.refreshable {
        return if expires > now {
            Ok(existing)
        } else {
            Err(format!("IAM Identity Center token has expired. {LOGIN}"))
        };
    }
    if state
        .last_refresh
        .is_some_and(|t| t.elapsed() < Duration::from_secs(30))
    {
        return if expires > now {
            Ok(existing)
        } else {
            Err(format!("IAM Identity Center token has expired. {LOGIN}"))
        };
    }
    let mut body = json!({"grantType":"refresh_token"});
    for key in ["clientId", "clientSecret", "refreshToken"] {
        body[key] = Value::String(value(&old, key).ok_or_else(|| {
            format!("IAM Identity Center cache has no {key}; cannot refresh. {LOGIN}")
        })?);
    }
    state.last_refresh = Some(Instant::now());
    let refreshed = async {
        let response = send(http.post(format!("{}/token", config.oidc)).json(&body)).await?;
        let access =
            value(&response, "accessToken").ok_or("IAM Identity Center returned no accessToken")?;
        let lifetime = response["expiresIn"]
            .as_u64()
            .filter(|t| *t > 0)
            .ok_or("IAM Identity Center returned invalid expiresIn")?;
        let end = now_secs()
            .checked_add(lifetime)
            .and_then(|t| i64::try_from(t).ok())
            .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
            .ok_or("IAM Identity Center returned invalid token expiry")?;
        let mut new = old.clone();
        new["accessToken"] = json!(access);
        new["expiresAt"] = json!(end.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
        match value(&response, "refreshToken") {
            Some(refresh) => new["refreshToken"] = json!(refresh),
            None => {
                new.as_object_mut().unwrap().remove("refreshToken");
            }
        }
        persist(config.path.clone(), old.clone(), new).await;
        Ok::<_, String>(access)
    }
    .await;
    match refreshed {
        Ok(token) => Ok(token),
        Err(_) if expires > now_secs() => Ok(existing),
        Err(_) => Err(format!(
            "IAM Identity Center token has expired and could not refresh. {LOGIN}"
        )),
    }
}

async fn credentials(
    http: &reqwest::Client,
    config: &Config,
    state: &mut State,
) -> Result<AwsCredentials, String> {
    let token = access_token(http, config, state, read_token(&config.path).await?).await?;
    let cache_key = sha256_hex(
        json!([config.portal, config.account, config.role, token])
            .to_string()
            .as_bytes(),
    );
    if let Some(cached) = state.credentials.get(&cache_key).filter(|c| {
        c.expires_at
            .is_some_and(|e| e > now_secs().saturating_add(300))
    }) {
        return Ok(cached.clone());
    }
    let response = send(
        http.get(format!("{}/federation/credentials", config.portal))
            .query(&[("account_id", &config.account), ("role_name", &config.role)])
            .header("x-amz-sso_bearer_token", &token),
    )
    .await?;
    let role = &response["roleCredentials"];
    let bad = || "IAM Identity Center returned invalid role credentials".to_owned();
    let access = value(role, "accessKeyId").ok_or_else(bad)?;
    let secret = value(role, "secretAccessKey").ok_or_else(bad)?;
    let session = value(role, "sessionToken").ok_or_else(bad)?;
    let expiration = role["expiration"]
        .as_u64()
        .map(|ms| ms / 1000)
        .filter(|s| *s > now_secs())
        .ok_or_else(bad)?;
    let credentials = AwsCredentials {
        access_key_id: access,
        secret_access_key: secret,
        session_token: Some(session),
        expires_at: Some(expiration),
    };
    if state.credentials.len() >= 64 {
        state.credentials.clear();
    }
    state.credentials.insert(cache_key, credentials.clone());
    Ok(credentials)
}

#[cfg(test)]
#[path = "aws_sso_tests.rs"]
mod tests;
