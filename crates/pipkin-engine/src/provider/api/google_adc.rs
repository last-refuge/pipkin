//! ADC discovery and bounded, identity-specific credential refresh.
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use super::{AccessToken, CLOUD_PLATFORM_SCOPE, GOOGLE_TOKEN_URL, NO_CREDENTIALS};
use crate::provider::api::common::opt_str;
use crate::provider::{BoxFuture, StreamCtx, http};

#[path = "google_external.rs"]
mod external;

const LIMIT: usize = 1024 * 1024;
const BUDGET: Duration = Duration::from_secs(30);
const EAGER: Duration = Duration::from_secs(300);
struct Cached {
    token: AccessToken,
    expires: Option<Instant>,
}
#[derive(Default)]
struct State {
    cached: Option<Cached>,
    // Google external authorized-user clients retain rotated refresh tokens in memory.
    refresh_tokens: HashMap<String, String>,
}
type Token = (String, Option<Duration>);
type Slot = Arc<tokio::sync::Mutex<State>>;
fn slot(key: String) -> Result<Slot, String> {
    static CACHE: OnceLock<Mutex<HashMap<String, Slot>>> = OnceLock::new();
    let mut all = CACHE
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(slot) = all.get(&key) {
        return Ok(slot.clone());
    }
    if all.len() >= 64 {
        let idle = all
            .iter()
            .find(|(_, v)| Arc::strong_count(v) == 1)
            .map(|(k, _)| k.clone())
            .ok_or("Too many concurrent Google credential identities")?;
        all.remove(&idle);
    }
    let slot = Arc::new(tokio::sync::Mutex::new(State::default()));
    all.insert(key, slot.clone());
    Ok(slot)
}
fn field(v: &Value, k: &str) -> Result<String, String> {
    super::field(v, k).ok_or_else(|| format!("Google credentials/response missing {k}"))
}
async fn read(path: &Path) -> Result<Option<Value>, String> {
    let file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("Could not read Google credentials file".into()),
    };
    if !file
        .metadata()
        .await
        .map_err(|_| "Cannot inspect Google credentials file")?
        .is_file()
    {
        return Err("Google credentials path is not a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| "Cannot read Google credentials file")?;
    if bytes.len() > LIMIT {
        return Err("Google credentials file is too large".into());
    }
    let value =
        serde_json::from_slice(&bytes).map_err(|_| "Google credentials file is not valid JSON")?;
    Ok(Some(value))
}
async fn response_bytes(mut response: reqwest::Response) -> Result<Vec<u8>, String> {
    let status = response.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| http::transport_error(&e))?
    {
        if bytes.len().saturating_add(chunk.len()) > LIMIT {
            return Err("Google authentication response is too large".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        return Err(format!(
            "Google authentication failed (HTTP {})",
            status.as_u16()
        ));
    }
    Ok(bytes)
}
async fn response(response: reqwest::Response) -> Result<Value, String> {
    serde_json::from_slice(&response_bytes(response).await?)
        .map_err(|_| "Google authentication returned invalid JSON".into())
}
fn oauth_token(v: &Value) -> Result<(String, Duration), String> {
    let token = field(v, "access_token")?;
    let seconds = v["expires_in"]
        .as_u64()
        .filter(|s| *s > 0)
        .ok_or("Google authentication returned invalid expires_in")?;
    let duration = Duration::from_secs(seconds);
    Instant::now()
        .checked_add(duration)
        .ok_or("Google authentication returned excessive token lifetime")?;
    Ok((token, duration))
}
fn universe(doc: &Value) -> &str {
    if let Some(domain) = external::option(doc, "universe_domain")
        .as_str()
        .filter(|s| !s.is_empty())
    {
        domain
    } else if doc["type"] == "impersonated_service_account" {
        universe(&doc["source_credentials"])
    } else {
        "googleapis.com"
    }
}
fn validate(doc: &Value, depth: usize) -> Result<(), String> {
    if depth > 8 {
        return Err("Google credential impersonation nesting is too deep".into());
    }
    if doc["type"] == "impersonated_service_account" {
        let source = doc
            .get("source_credentials")
            .ok_or("Google impersonation has no source_credentials")?;
        if doc.get("universe_domain").is_some() && universe(doc) != universe(source) {
            return Err("Google impersonation universe does not match source credentials".into());
        }
        principal(doc)?;
        validate(source, depth + 1)?;
    } else if matches!(
        doc["type"].as_str(),
        Some("external_account" | "external_account_authorized_user")
    ) {
        external::validate(doc)?;
    } else {
        super::parse_credentials(&doc.to_string())?;
    }
    Ok(())
}
fn principal(doc: &Value) -> Result<String, String> {
    let raw = field(doc, "service_account_impersonation_url")?;
    if raw.len() > 256 {
        return Err("Google impersonation URL is too long".into());
    }
    let last = raw.rsplit('/').next().unwrap_or("");
    let principal = last
        .strip_suffix(":generateAccessToken")
        .or_else(|| last.strip_suffix(":generateIdToken"))
        .filter(|p| !p.is_empty())
        .ok_or("Cannot extract Google impersonation principal")?;
    Ok(principal.to_owned())
}
fn exchange<'a>(
    http: &'a reqwest::Client,
    doc: &'a Value,
    token_url: &'a str,
    depth: usize,
    state: &'a mut State,
) -> BoxFuture<'a, Result<Token, String>> {
    Box::pin(async move {
        if depth > 8 {
            return Err("Google credential impersonation nesting is too deep".into());
        }
        if matches!(
            doc["type"].as_str(),
            Some("external_account" | "external_account_authorized_user")
        ) {
            return external::exchange(http, doc, state).await;
        }
        if doc["type"] != "impersonated_service_account" {
            let parsed = super::parse_credentials(&doc.to_string())?;
            return super::exchange(http, &CancellationToken::new(), token_url, &parsed)
                .await
                .map(|(t, d)| (t, Some(d)));
        }
        let source = &doc["source_credentials"];
        let (source_token, _) = exchange(http, source, token_url, depth + 1, state).await?;
        let principal = principal(doc)?;
        let base = super::field(doc, "endpoint")
            .unwrap_or_else(|| format!("https://iamcredentials.{}", universe(source)));
        let endpoint = format!(
            "{}/v1/projects/-/serviceAccounts/{principal}:generateAccessToken",
            base.trim_end_matches('/')
        );
        let parsed =
            url::Url::parse(&endpoint).map_err(|_| "Invalid Google impersonation endpoint")?;
        if parsed.scheme() != "https"
            && !(parsed.scheme() == "http"
                && matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")))
        {
            return Err("Google impersonation endpoint requires HTTPS (or loopback HTTP)".into());
        }
        let delegates = doc.get("delegates").cloned().unwrap_or(json!([]));
        if !delegates
            .as_array()
            .is_some_and(|a| a.iter().all(Value::is_string))
        {
            return Err("Google impersonation delegates must be strings".into());
        }
        let lifetime = doc.get("lifetime").and_then(Value::as_u64).unwrap_or(3600);
        let body = json!({"delegates":delegates,"scope":[CLOUD_PLATFORM_SCOPE],"lifetime":format!("{lifetime}s")});
        let mut request = http
            .post(endpoint)
            .bearer_auth(source_token)
            .json(&body)
            .timeout(BUDGET);
        if let Some(quota) = super::field(source, "quota_project_id") {
            request = request.header("x-goog-user-project", quota);
        }
        let result = response(
            request
                .send()
                .await
                .map_err(|e| http::transport_error(&e))?,
        )
        .await?;
        let token = field(&result, "accessToken")?;
        let expires = chrono::DateTime::parse_from_rfc3339(&field(&result, "expireTime")?)
            .map_err(|_| "Invalid Google impersonation expireTime")?
            .timestamp();
        let lifetime = u64::try_from(expires)
            .ok()
            .and_then(|v| v.checked_sub(super::unix_now()))
            .filter(|v| *v > 0)
            .ok_or("Google impersonation token is already expired")?;
        Ok((token, Some(Duration::from_secs(lifetime))))
    })
}
fn google_resident(ctx: &StreamCtx<'_>) -> bool {
    if ["CLOUD_RUN_JOB", "FUNCTION_NAME", "K_SERVICE"]
        .iter()
        .any(|name| ctx.req.env_var(name).is_some())
    {
        return true;
    }
    let small_file = |path: &Path| -> Option<String> {
        use std::io::Read;
        let mut text = String::new();
        std::fs::File::open(path)
            .ok()?
            .take(1024)
            .read_to_string(&mut text)
            .ok()?;
        Some(text)
    };
    if Path::new("/sys/class/dmi/id/bios_date").exists()
        && small_file(Path::new("/sys/class/dmi/id/bios_vendor"))
            .is_some_and(|s| s.contains("Google"))
    {
        return true;
    }
    std::fs::read_dir("/sys/class/net")
        .ok()
        .is_some_and(|entries| {
            entries.flatten().any(|entry| {
                small_file(&entry.path().join("address"))
                    .is_some_and(|mac| mac.starts_with("42:01"))
            })
        })
}

fn metadata_url(ctx: &StreamCtx<'_>) -> Result<String, String> {
    if let Some(mode) = ctx.req.env_var("METADATA_SERVER_DETECTION") {
        match mode.trim().to_ascii_lowercase().as_str() {
            "none" => return Err(NO_CREDENTIALS.into()),
            "bios-only" if !google_resident(ctx) => return Err(NO_CREDENTIALS.into()),
            "bios-only" | "assume-present" | "ping-only" => {}
            _ => return Err("Unknown METADATA_SERVER_DETECTION value".into()),
        }
    }
    let host = ctx
        .req
        .env_var("GCE_METADATA_IP")
        .or_else(|| ctx.req.env_var("GCE_METADATA_HOST"))
        .unwrap_or_else(|| "169.254.169.254".into());
    let base = if host.contains("://") {
        host
    } else {
        format!("http://{host}")
    };
    let mut url = url::Url::parse(&base).map_err(|_| "Invalid Google metadata host")?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Invalid Google metadata host".into());
    }
    url.set_path("/computeMetadata/v1/instance/service-accounts/default/token");
    url.query_pairs_mut()
        .append_pair("scopes", CLOUD_PLATFORM_SCOPE);
    Ok(url.to_string())
}
async fn metadata(http: &reqwest::Client, url: &str) -> Result<(String, Duration), String> {
    let response = http
        .get(url)
        .header("Metadata-Flavor", "Google")
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .map_err(|_| NO_CREDENTIALS.to_owned())?;
    if response
        .headers()
        .get("metadata-flavor")
        .and_then(|v| v.to_str().ok())
        != Some("Google")
    {
        return Err("Google metadata response has no valid Metadata-Flavor header".into());
    }
    oauth_token(&self::response(response).await?)
}

pub(crate) async fn resolve(
    ctx: &StreamCtx<'_>,
    explicit: Option<String>,
) -> Result<AccessToken, String> {
    if ctx.cancel.is_cancelled() {
        return Err("Request aborted".into());
    }
    let explicit = explicit
        .or_else(|| ctx.req.env_var("GOOGLE_APPLICATION_CREDENTIALS"))
        .or_else(|| ctx.req.env_var("google_application_credentials"));
    let path = super::credentials_path(ctx.req, explicit.clone());
    let document = if let Some(path) = &path {
        tokio::select! {
         _=ctx.cancel.cancelled()=>return Err("Request aborted".into()),
         result=tokio::time::timeout(BUDGET,read(path))=>result.map_err(|_|"Google credentials read timed out")??,
        }
    } else {
        None
    };
    if document.is_none() && explicit.is_some() {
        return Err(NO_CREDENTIALS.into());
    }
    let metadata = if document.is_none() {
        Some(metadata_url(ctx)?)
    } else {
        None
    };
    if let Some(doc) = &document {
        validate(doc, 0)?;
    }
    let quota = ctx.req.env_var("GOOGLE_CLOUD_QUOTA_PROJECT").or_else(|| {
        document
            .as_ref()
            .and_then(|d| external::string(d, "quota_project_id"))
    });
    let token_url = opt_str(ctx.req, "googleTokenUrl")
        .unwrap_or(GOOGLE_TOKEN_URL)
        .to_owned();
    let key = crate::provider::api::aws_auth::sha256_hex(
        json!([path, document, metadata, quota, token_url])
            .to_string()
            .as_bytes(),
    );
    let slot = slot(key)?;
    let http = ctx.http.clone();
    let mut owner = tokio::spawn(async move {
        tokio::time::timeout(BUDGET, async {
            let mut state = slot.lock().await;
            if let Some(c) = state.cached.as_ref().filter(|c| {
                c.expires
                    .is_none_or(|expires| Instant::now() + EAGER < expires)
            }) {
                return Ok(c.token.clone());
            }
            let started = Instant::now();
            let (token, lifetime) = match document {
                Some(doc) => exchange(&http, &doc, &token_url, 0, &mut state).await?,
                None => {
                    let (t, d) = self::metadata(&http, metadata.as_deref().unwrap()).await?;
                    (t, Some(d))
                }
            };
            let expires = lifetime
                .map(|duration| {
                    started
                        .checked_add(duration)
                        .ok_or("Google authentication returned excessive token lifetime")
                })
                .transpose()?;
            let token = AccessToken {
                token,
                quota_project: quota,
            };
            state.cached = Some(Cached {
                token: token.clone(),
                expires,
            });
            Ok(token)
        })
        .await
        .map_err(|_| "Google credential exchange timed out".to_owned())?
    });
    tokio::select! {_=ctx.cancel.cancelled()=>Err("Request aborted".into()),result=&mut owner=>result.map_err(|_|"Google credential task failed".to_owned())?}
}

#[cfg(test)]
#[path = "google_adc_tests.rs"]
mod tests;
