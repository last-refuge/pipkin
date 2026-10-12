//! AWS Signature Version 4 and the parts of the AWS credential provider chain that work
//! without the AWS SDK.
//!
//! **Signing** follows the published SigV4 algorithm (canonical request, string to sign,
//! derived key) with the SDK's non-S3 conventions: the already-encoded path is URI-encoded
//! again, query pairs are sorted, header values are trimmed and inner whitespace collapsed,
//! `x-amz-content-sha256` is added like the JS SDK does, and unsignable headers (user-agent,
//! authorization, ...) are left out.
//!
//! **Credentials** (reference: AWS SDK v3 `defaultProvider` as configured by
//! `api/bedrock-converse-stream.ts:157-243`), in order:
//! 1. environment keys `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` (+ `AWS_SESSION_TOKEN`) —
//!    skipped when a profile was chosen explicitly for this provider;
//! 2. the shared config/credentials files profile (`AWS_PROFILE` or `default`;
//!    `AWS_SHARED_CREDENTIALS_FILE`, `AWS_CONFIG_FILE`): static keys, `role_arn` with
//!    `source_profile`/`credential_source` (STS `AssumeRole`, SigV4-signed),
//!    `role_arn` + `web_identity_token_file`, and `credential_process`;
//! 3. web identity from `AWS_WEB_IDENTITY_TOKEN_FILE` + `AWS_ROLE_ARN` (STS
//!    `AssumeRoleWithWebIdentity`, unsigned);
//! 4. the ECS/EKS container endpoint (`AWS_CONTAINER_CREDENTIALS_RELATIVE_URI` /
//!    `_FULL_URI`, with `AWS_CONTAINER_AUTHORIZATION_TOKEN(_FILE)`);
//! 5. EC2 instance metadata (IMDSv2 with v1 fallback) unless `AWS_EC2_METADATA_DISABLED`.
//!
//! IAM Identity Center profiles use the AWS CLI token cache, including session-token refresh.
//! MFA role profiles require a callback which neither Pipkin nor the pinned Pi adapter supplies;
//! they fail explicitly. Temporary credentials are cached in-process until five minutes
//! before they expire. STS honours `AWS_ENDPOINT_URL_STS` / `AWS_ENDPOINT_URL`.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use url::Url;

#[path = "aws_sso.rs"]
mod sso;

/// Reads an environment value (request overrides, then the process).
pub type EnvLookup<'a> = &'a (dyn Fn(&str) -> Option<String> + Send + Sync);

#[derive(Clone, PartialEq, Eq)]
pub struct AwsCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
    /// Unix seconds.
    pub expires_at: Option<u64>,
}

impl std::fmt::Debug for AwsCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AwsCredentials({}, [redacted])", self.access_key_id)
    }
}

impl AwsCredentials {
    pub fn new(id: &str, secret: &str, token: Option<&str>) -> AwsCredentials {
        AwsCredentials {
            access_key_id: id.to_owned(),
            secret_access_key: secret.to_owned(),
            session_token: token.filter(|t| !t.is_empty()).map(str::to_owned),
            expires_at: None,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// SigV4
// ---------------------------------------------------------------------------------------------

type HmacSha256 = Hmac<Sha256>;

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// RFC 3986 encoding of everything but unreserved characters (and `/` when kept).
fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b'/' if keep_slash => out.push('/'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Percent-encode one path segment the way the SDK builds request paths
/// (`extendedEncodeURIComponent`).
pub fn encode_path_segment(s: &str) -> String {
    uri_encode(s, false)
}

/// `(YYYYMMDD, YYYYMMDDTHHMMSSZ)` in UTC.
pub fn amz_dates(time: SystemTime) -> (String, String) {
    let secs = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    let date = format!("{y:04}{m:02}{d:02}");
    let stamp = format!(
        "{date}T{:02}{:02}{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    );
    (date, stamp)
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Parse an ISO-8601 UTC timestamp (`2024-01-02T03:04:05Z`, fractional seconds allowed).
pub fn parse_iso8601(s: &str) -> Option<u64> {
    let s = s.trim();
    let (date, time) = s.split_once('T')?;
    let mut dp = date.split('-');
    let y: i64 = dp.next()?.parse().ok()?;
    let m: u32 = dp.next()?.parse().ok()?;
    let d: u32 = dp.next()?.parse().ok()?;
    let time = time.trim_end_matches('Z');
    let time = time.split(['+']).next()?;
    let mut tp = time.split(':');
    let h: u64 = tp.next()?.parse().ok()?;
    let mi: u64 = tp.next()?.parse().ok()?;
    let sec: f64 = tp.next().unwrap_or("0").parse().ok()?;
    let days = days_from_civil(y, m, d);
    (days >= 0).then(|| days as u64 * 86_400 + h * 3600 + mi * 60 + sec as u64)
}

const UNSIGNABLE: &[&str] = &[
    "authorization",
    "cache-control",
    "connection",
    "expect",
    "from",
    "keep-alive",
    "max-forwards",
    "pragma",
    "referer",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "user-agent",
    "x-amzn-trace-id",
];

/// What to sign.
pub struct SigningParams<'a> {
    pub credentials: &'a AwsCredentials,
    pub region: &'a str,
    pub service: &'a str,
    pub time: SystemTime,
    /// Add `x-amz-content-sha256` (the JS SDK does for every service).
    pub content_sha256_header: bool,
}

fn host_header(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default();
    match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    }
}

fn canonical_query(url: &Url) -> String {
    let mut pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (uri_encode(&k, false), uri_encode(&v, false)))
        .collect();
    pairs.sort();
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// Sign a request in place: adds `host`, `x-amz-date`, `x-amz-security-token` (when the
/// credentials carry one), optionally `x-amz-content-sha256`, then `authorization`. Header
/// names are stored lowercased. Returns the signature (hex).
pub fn sign(
    method: &str,
    url: &Url,
    headers: &mut Vec<(String, String)>,
    body: &[u8],
    params: &SigningParams<'_>,
) -> String {
    for (k, _) in headers.iter_mut() {
        *k = k.to_ascii_lowercase();
    }
    headers.retain(|(k, _)| k != "authorization");
    let (date, stamp) = amz_dates(params.time);
    let payload_hash = sha256_hex(body);
    let mut set = |name: &str, value: String| {
        headers.retain(|(k, _)| k != name);
        headers.push((name.to_owned(), value));
    };
    if !url.host_str().unwrap_or_default().is_empty() {
        set("host", host_header(url));
    }
    set("x-amz-date", stamp.clone());
    if let Some(token) = &params.credentials.session_token {
        set("x-amz-security-token", token.clone());
    }
    if params.content_sha256_header {
        set("x-amz-content-sha256", payload_hash.clone());
    }

    let mut canonical: Vec<(String, String)> = Vec::new();
    for (k, v) in headers.iter() {
        if UNSIGNABLE.contains(&k.as_str()) {
            continue;
        }
        let value = v.split_whitespace().collect::<Vec<_>>().join(" ");
        match canonical.iter_mut().find(|(ck, _)| ck == k) {
            Some((_, existing)) => {
                existing.push(',');
                existing.push_str(&value);
            }
            None => canonical.push((k.clone(), value)),
        }
    }
    canonical.sort_by(|a, b| a.0.cmp(&b.0));
    let signed_headers = canonical
        .iter()
        .map(|(k, _)| k.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let canonical_headers: String = canonical
        .iter()
        .map(|(k, v)| format!("{k}:{v}\n"))
        .collect();
    let path = if url.path().is_empty() {
        "/".to_owned()
    } else {
        uri_encode(url.path(), true)
    };
    let canonical_request = format!(
        "{method}\n{path}\n{}\n{canonical_headers}\n{signed_headers}\n{payload_hash}",
        canonical_query(url)
    );
    let scope = format!("{date}/{}/{}/aws4_request", params.region, params.service);
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{stamp}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );
    let key = signing_key(
        &params.credentials.secret_access_key,
        &date,
        params.region,
        params.service,
    );
    let signature = hex(&hmac(&key, string_to_sign.as_bytes()));
    headers.push((
        "authorization".into(),
        format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
            params.credentials.access_key_id
        ),
    ));
    signature
}

pub fn signing_key(secret: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
    let k_date = hmac(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let k_region = hmac(&k_date, region.as_bytes());
    let k_service = hmac(&k_region, service.as_bytes());
    hmac(&k_service, b"aws4_request")
}

// ---------------------------------------------------------------------------------------------
// Shared config files
// ---------------------------------------------------------------------------------------------

pub type Profiles = HashMap<String, HashMap<String, String>>;

/// Parse an AWS INI file. `config_style` maps `[profile x]` to `x` (and `[default]`).
pub fn parse_ini(text: &str, config_style: bool) -> Profiles {
    let mut out: Profiles = HashMap::new();
    let mut current: Option<String> = None;
    for raw in text.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let name = trimmed[1..trimmed.len() - 1].trim();
            current = if config_style {
                if name == "default" {
                    Some("default".into())
                } else {
                    name.strip_prefix("profile ")
                        .or_else(|| name.strip_prefix("profile\t"))
                        .map(|n| n.trim().to_owned())
                }
            } else {
                Some(name.to_owned())
            };
            continue;
        }
        // Indented lines continue a nested sub-section (e.g. `s3 =`); not needed here.
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let Some(section) = &current else { continue };
        if let Some((k, v)) = trimmed.split_once('=') {
            out.entry(section.clone())
                .or_default()
                .insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
        }
    }
    out
}

fn home(env: EnvLookup<'_>) -> Option<String> {
    env("HOME").or_else(|| env("USERPROFILE"))
}

/// Profiles from the config file, overlaid per key by the credentials file.
pub fn load_profiles(env: EnvLookup<'_>) -> Profiles {
    let base = home(env).map(|h| format!("{h}/.aws"));
    let config_path =
        env("AWS_CONFIG_FILE").or_else(|| base.as_ref().map(|b| format!("{b}/config")));
    let creds_path = env("AWS_SHARED_CREDENTIALS_FILE")
        .or_else(|| base.as_ref().map(|b| format!("{b}/credentials")));
    let read = |p: Option<String>| p.and_then(|p| std::fs::read_to_string(p).ok());
    let mut profiles = read(config_path)
        .map(|t| parse_ini(&t, true))
        .unwrap_or_default();
    if let Some(text) = read(creds_path) {
        for (name, values) in parse_ini(&text, false) {
            profiles.entry(name).or_default().extend(values);
        }
    }
    profiles
}

/// The `region` configured for a profile, if any.
pub fn profile_region(env: EnvLookup<'_>, profile: &str) -> Option<String> {
    load_profiles(env)
        .get(profile)
        .and_then(|p| p.get("region"))
        .filter(|r| !r.is_empty())
        .cloned()
}

// ---------------------------------------------------------------------------------------------
// Provider chain
// ---------------------------------------------------------------------------------------------

/// A provider either has nothing to offer (try the next) or failed definitively.
#[derive(Debug)]
enum ChainError {
    Next(String),
    Fatal(String),
}

type ChainResult = Result<AwsCredentials, ChainError>;

fn cache() -> &'static Mutex<HashMap<String, AwsCredentials>> {
    static CACHE: OnceLock<Mutex<HashMap<String, AwsCredentials>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn cached(key: &str) -> Option<AwsCredentials> {
    let map = cache().lock().unwrap_or_else(|e| e.into_inner());
    map.get(key)
        .filter(|c| c.expires_at.is_some_and(|t| t > now_secs() + 300))
        .cloned()
}

fn remember(key: &str, creds: &AwsCredentials) {
    if creds.expires_at.is_some() {
        cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.to_owned(), creds.clone());
    }
}

/// Inputs of one resolution.
pub struct ChainInput<'a> {
    pub env: EnvLookup<'a>,
    /// A profile chosen for this provider (option or scoped `AWS_PROFILE`): wins over
    /// environment keys.
    pub explicit_profile: Option<String>,
    /// The profile from the process environment, if any.
    pub ambient_profile: Option<String>,
    /// Region for STS calls.
    pub region: String,
    pub http: &'a reqwest::Client,
    pub cancel: &'a CancellationToken,
}

/// Resolve credentials through the chain described in the module docs.
pub async fn resolve_credentials(input: &ChainInput<'_>) -> Result<AwsCredentials, String> {
    let mut reasons = Vec::new();
    if input.explicit_profile.is_none() {
        match from_env(input.env) {
            Ok(c) => return Ok(c),
            Err(ChainError::Next(r)) => reasons.push(r),
            Err(ChainError::Fatal(r)) => return Err(r),
        }
    }
    let profile = input
        .explicit_profile
        .clone()
        .or_else(|| input.ambient_profile.clone())
        .unwrap_or_else(|| "default".into());
    for step in 0..4 {
        if input.cancel.is_cancelled() {
            return Err("Request was aborted".into());
        }
        let result = match step {
            0 => from_profile(input, profile.clone(), Vec::new()).await,
            1 => from_web_identity_env(input).await,
            2 => from_container(input).await,
            _ => from_imds(input).await,
        };
        match result {
            Ok(c) => return Ok(c),
            Err(ChainError::Next(r)) => reasons.push(r),
            Err(ChainError::Fatal(r)) => return Err(r),
        }
    }
    log::debug!("AWS credential chain: {}", reasons.join("; "));
    Err("Could not load credentials from any providers".into())
}

type BoxChain<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = ChainResult> + Send + 'a>>;

fn from_env(env: EnvLookup<'_>) -> ChainResult {
    match (env("AWS_ACCESS_KEY_ID"), env("AWS_SECRET_ACCESS_KEY")) {
        (Some(id), Some(secret)) => Ok(AwsCredentials::new(
            &id,
            &secret,
            env("AWS_SESSION_TOKEN").as_deref(),
        )),
        _ => Err(ChainError::Next(
            "Unable to find environment variable credentials.".into(),
        )),
    }
}

fn from_profile<'a>(input: &'a ChainInput<'a>, name: String, visited: Vec<String>) -> BoxChain<'a> {
    Box::pin(async move {
        let profiles = load_profiles(input.env);
        let Some(p) = profiles.get(&name) else {
            return Err(ChainError::Next(format!(
                "Profile {name} could not be found or parsed in shared credentials file."
            )));
        };
        let get = |k: &str| p.get(k).filter(|v| !v.is_empty()).cloned();
        let static_creds = || match (get("aws_access_key_id"), get("aws_secret_access_key")) {
            (Some(id), Some(secret)) => Some(AwsCredentials::new(
                &id,
                &secret,
                get("aws_session_token").as_deref(),
            )),
            _ => None,
        };
        let role_arn = get("role_arn");
        let source_profile = get("source_profile");
        let credential_source = get("credential_source");
        // Role assumption wins over static keys on the first profile visited.
        if let Some(role) = &role_arn
            && (source_profile.is_some() || credential_source.is_some())
            && (visited.is_empty() || static_creds().is_none())
        {
            if get("mfa_serial").is_some() {
                return Err(ChainError::Fatal(format!(
                    "Profile {name} requires MFA (mfa_serial), which Pipkin cannot prompt for. Use temporary credentials instead."
                )));
            }
            let source = if let Some(src) = &source_profile {
                if visited.contains(src) || (*src == name && !visited.is_empty()) {
                    return Err(ChainError::Fatal(format!(
                        "Detected a cycle in source_profile for profile {name}"
                    )));
                }
                if *src == name {
                    static_creds().ok_or_else(|| {
                        ChainError::Fatal(format!(
                            "Profile {name} uses itself as source_profile but has no static credentials"
                        ))
                    })?
                } else {
                    let mut next = visited.clone();
                    next.push(name.clone());
                    from_profile(input, src.clone(), next)
                        .await
                        .map_err(|e| match e {
                            ChainError::Next(m) | ChainError::Fatal(m) => ChainError::Fatal(m),
                        })?
                }
            } else {
                match credential_source.as_deref() {
                    Some("Environment") => from_env(input.env).map_err(fatal)?,
                    Some("EcsContainer") => from_container(input).await.map_err(fatal)?,
                    Some("Ec2InstanceMetadata") => from_imds(input).await.map_err(fatal)?,
                    other => {
                        return Err(ChainError::Fatal(format!(
                            "Unsupported credential source in profile {name}: {}",
                            other.unwrap_or("")
                        )));
                    }
                }
            };
            let key = format!("assume:{name}:{role}");
            if let Some(c) = cached(&key) {
                return Ok(c);
            }
            let creds = assume_role(input, &source, role, p)
                .await
                .map_err(fatal_str)?;
            remember(&key, &creds);
            return Ok(creds);
        }
        if let Some(c) = static_creds() {
            return Ok(c);
        }
        if let (Some(role), Some(file)) = (&role_arn, get("web_identity_token_file")) {
            let session = get("role_session_name");
            return web_identity(input, role, &file, session.as_deref())
                .await
                .map_err(fatal_str);
        }
        if let Some(command) = get("credential_process") {
            let key = format!("process:{name}");
            if let Some(c) = cached(&key) {
                return Ok(c);
            }
            let creds = credential_process(&command, input.cancel)
                .await
                .map_err(fatal_str)?;
            remember(&key, &creds);
            return Ok(creds);
        }
        if [
            "sso_session",
            "sso_start_url",
            "sso_region",
            "sso_account_id",
            "sso_role_name",
        ]
        .iter()
        .any(|key| get(key).is_some())
        {
            return sso::resolve(input, p).await.map_err(fatal_str);
        }
        Err(ChainError::Next(format!(
            "Profile {name} did not contain credentials"
        )))
    })
}

fn fatal(e: ChainError) -> ChainError {
    match e {
        ChainError::Next(m) | ChainError::Fatal(m) => ChainError::Fatal(m),
    }
}

fn fatal_str(m: String) -> ChainError {
    ChainError::Fatal(m)
}

fn sts_endpoint(input: &ChainInput<'_>) -> String {
    if let Some(url) =
        (input.env)("AWS_ENDPOINT_URL_STS").or_else(|| (input.env)("AWS_ENDPOINT_URL"))
    {
        return url.trim_end_matches('/').to_owned();
    }
    let suffix = if input.region.starts_with("cn-") {
        "amazonaws.com.cn"
    } else {
        "amazonaws.com"
    };
    format!("https://sts.{}.{suffix}", input.region)
}

fn form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", uri_encode(k, false), uri_encode(v, false)))
        .collect::<Vec<_>>()
        .join("&")
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The text of the first `<tag>...</tag>` element.
pub fn xml_tag(body: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(xml_unescape(body[start..end].trim()))
}

fn session_name(env: EnvLookup<'_>, configured: Option<&str>) -> String {
    configured
        .map(str::to_owned)
        .or_else(|| env("AWS_ROLE_SESSION_NAME"))
        .unwrap_or_else(|| format!("pipkin-session-{}", crate::util::now_ms()))
}

async fn send_with_cancel(
    builder: reqwest::RequestBuilder,
    cancel: &CancellationToken,
) -> Result<(reqwest::StatusCode, String), String> {
    let fut = async {
        let resp = builder
            .send()
            .await
            .map_err(|e| crate::provider::http::transport_error(&e))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| crate::provider::http::transport_error(&e))?;
        Ok((status, text))
    };
    tokio::select! {
        _ = cancel.cancelled() => Err("Request was aborted".into()),
        r = fut => r,
    }
}

fn sts_credentials(
    status: reqwest::StatusCode,
    body: &str,
    action: &str,
) -> Result<AwsCredentials, String> {
    if !status.is_success() {
        let code = xml_tag(body, "Code").unwrap_or_else(|| status.as_u16().to_string());
        let message = xml_tag(body, "Message").unwrap_or_else(|| body.trim().to_owned());
        return Err(format!("STS {action} failed: {code}: {message}"));
    }
    let id = xml_tag(body, "AccessKeyId");
    let secret = xml_tag(body, "SecretAccessKey");
    match (id, secret) {
        (Some(id), Some(secret)) => {
            let mut c = AwsCredentials::new(&id, &secret, xml_tag(body, "SessionToken").as_deref());
            c.expires_at = xml_tag(body, "Expiration").and_then(|e| parse_iso8601(&e));
            Ok(c)
        }
        _ => Err(format!("STS {action} returned no credentials")),
    }
}

async fn assume_role(
    input: &ChainInput<'_>,
    source: &AwsCredentials,
    role_arn: &str,
    profile: &HashMap<String, String>,
) -> Result<AwsCredentials, String> {
    let session = session_name(
        input.env,
        profile.get("role_session_name").map(String::as_str),
    );
    let mut pairs = vec![
        ("Action", "AssumeRole"),
        ("Version", "2011-06-15"),
        ("RoleArn", role_arn),
        ("RoleSessionName", session.as_str()),
    ];
    if let Some(id) = profile.get("external_id") {
        pairs.push(("ExternalId", id));
    }
    if let Some(d) = profile.get("duration_seconds") {
        pairs.push(("DurationSeconds", d));
    }
    let body = form(&pairs);
    let endpoint = sts_endpoint(input);
    let url = Url::parse(&format!("{endpoint}/")).map_err(|e| e.to_string())?;
    let mut headers = vec![(
        "content-type".to_owned(),
        "application/x-www-form-urlencoded".to_owned(),
    )];
    sign(
        "POST",
        &url,
        &mut headers,
        body.as_bytes(),
        &SigningParams {
            credentials: source,
            region: &input.region,
            service: "sts",
            time: SystemTime::now(),
            content_sha256_header: false,
        },
    );
    let builder = input
        .http
        .post(url)
        .headers(super::common::header_map(&headers)?)
        .timeout(Duration::from_secs(30))
        .body(body);
    let (status, text) = send_with_cancel(builder, input.cancel).await?;
    sts_credentials(status, &text, "AssumeRole")
}

async fn web_identity(
    input: &ChainInput<'_>,
    role_arn: &str,
    token_file: &str,
    session: Option<&str>,
) -> Result<AwsCredentials, String> {
    let key = format!("webid:{role_arn}:{token_file}");
    if let Some(c) = cached(&key) {
        return Ok(c);
    }
    let token = std::fs::read_to_string(token_file)
        .map_err(|e| format!("Could not read web identity token file {token_file}: {e}"))?;
    let session = session_name(input.env, session);
    let body = form(&[
        ("Action", "AssumeRoleWithWebIdentity"),
        ("Version", "2011-06-15"),
        ("RoleArn", role_arn),
        ("RoleSessionName", &session),
        ("WebIdentityToken", token.trim()),
    ]);
    let builder = input
        .http
        .post(format!("{}/", sts_endpoint(input)))
        .header("content-type", "application/x-www-form-urlencoded")
        .timeout(Duration::from_secs(30))
        .body(body);
    let (status, text) = send_with_cancel(builder, input.cancel).await?;
    let creds = sts_credentials(status, &text, "AssumeRoleWithWebIdentity")?;
    remember(&key, &creds);
    Ok(creds)
}

async fn from_web_identity_env(input: &ChainInput<'_>) -> ChainResult {
    let (Some(file), Some(role)) = (
        (input.env)("AWS_WEB_IDENTITY_TOKEN_FILE"),
        (input.env)("AWS_ROLE_ARN"),
    ) else {
        return Err(ChainError::Next(
            "Web identity configuration not specified".into(),
        ));
    };
    web_identity(input, &role, &file, None)
        .await
        .map_err(ChainError::Fatal)
}

fn json_credentials(v: &Value, token_key: &str) -> Result<AwsCredentials, String> {
    let s = |k: &str| v.get(k).and_then(Value::as_str);
    match (s("AccessKeyId"), s("SecretAccessKey")) {
        (Some(id), Some(secret)) => {
            let mut c = AwsCredentials::new(id, secret, s(token_key));
            c.expires_at = s("Expiration").and_then(parse_iso8601);
            Ok(c)
        }
        _ => Err("credential response is missing AccessKeyId/SecretAccessKey".into()),
    }
}

async fn credential_process(
    command: &str,
    cancel: &CancellationToken,
) -> Result<AwsCredentials, String> {
    let mut cmd = if cfg!(windows) {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/C").arg(command);
        c
    } else {
        let mut c = tokio::process::Command::new("sh");
        c.arg("-c").arg(command);
        c
    };
    cmd.kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = cmd
        .spawn()
        .map_err(|e| format!("credential_process failed to start: {e}"))?;
    let out = tokio::select! {
        _ = cancel.cancelled() => return Err("Request was aborted".into()),
        _ = tokio::time::sleep(Duration::from_secs(60)) => {
            return Err("credential_process timed out".into());
        }
        o = child.wait_with_output() => o.map_err(|e| format!("credential_process failed: {e}"))?,
    };
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "credential_process exited with {}: {}",
            out.status,
            err.trim()
        ));
    }
    let v: Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("credential_process returned invalid JSON: {e}"))?;
    if v.get("Version").and_then(Value::as_u64) != Some(1) {
        return Err("credential_process output must have Version 1".into());
    }
    json_credentials(&v, "SessionToken").map_err(|e| format!("credential_process {e}"))
}

const ECS_HOST: &str = "http://169.254.170.2";

fn allowed_full_uri(url: &Url) -> bool {
    if url.scheme() == "https" {
        return true;
    }
    match url.host() {
        Some(url::Host::Ipv4(ip)) => {
            ip.is_loopback()
                || ip.octets() == [169, 254, 170, 2]
                || ip.octets() == [169, 254, 170, 23]
        }
        Some(url::Host::Ipv6(ip)) => {
            ip.is_loopback()
                || ip.segments()[..4] == [0xfd00, 0x0ec2, 0, 0] && ip.segments()[7] == 0x23
        }
        Some(url::Host::Domain(d)) => d == "localhost",
        None => false,
    }
}

async fn from_container(input: &ChainInput<'_>) -> ChainResult {
    let env = input.env;
    let url = if let Some(rel) = env("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI") {
        format!("{ECS_HOST}{rel}")
    } else if let Some(full) = env("AWS_CONTAINER_CREDENTIALS_FULL_URI") {
        let parsed = Url::parse(&full)
            .map_err(|e| ChainError::Fatal(format!("Invalid container credentials URI: {e}")))?;
        if !allowed_full_uri(&parsed) {
            return Err(ChainError::Fatal(format!(
                "{} is not a valid container metadata service hostname",
                parsed.host_str().unwrap_or_default()
            )));
        }
        full
    } else {
        return Err(ChainError::Next(
            "The container metadata credential provider cannot be used unless AWS_CONTAINER_CREDENTIALS_RELATIVE_URI or AWS_CONTAINER_CREDENTIALS_FULL_URI is set".into(),
        ));
    };
    let key = format!("ecs:{url}");
    if let Some(c) = cached(&key) {
        return Ok(c);
    }
    let auth = match env("AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE") {
        Some(file) => Some(
            std::fs::read_to_string(&file)
                .map_err(|e| ChainError::Fatal(format!("Could not read {file}: {e}")))?
                .trim()
                .to_owned(),
        ),
        None => env("AWS_CONTAINER_AUTHORIZATION_TOKEN"),
    };
    let mut builder = input.http.get(&url).timeout(Duration::from_secs(5));
    if let Some(auth) = auth {
        builder = builder.header("authorization", auth);
    }
    let (status, text) = send_with_cancel(builder, input.cancel)
        .await
        .map_err(ChainError::Fatal)?;
    if !status.is_success() {
        return Err(ChainError::Fatal(format!(
            "Container credentials request failed: {}",
            crate::provider::http::format_error(Some(status), &text, "")
        )));
    }
    let v: Value = serde_json::from_str(&text)
        .map_err(|e| ChainError::Fatal(format!("Invalid container credentials response: {e}")))?;
    let creds =
        json_credentials(&v, "Token").map_err(|e| ChainError::Fatal(format!("Container {e}")))?;
    remember(&key, &creds);
    Ok(creds)
}

async fn from_imds(input: &ChainInput<'_>) -> ChainResult {
    let env = input.env;
    if env("AWS_EC2_METADATA_DISABLED").is_some_and(|v| v.eq_ignore_ascii_case("true")) {
        return Err(ChainError::Next(
            "EC2 Instance Metadata Service access disabled".into(),
        ));
    }
    let base =
        env("AWS_EC2_METADATA_SERVICE_ENDPOINT").unwrap_or_else(|| "http://169.254.169.254".into());
    let base = base.trim_end_matches('/').to_owned();
    if let Some(c) = cached("imds") {
        return Ok(c);
    }
    let timeout = Duration::from_secs(1);
    let unavailable =
        |m: String| ChainError::Next(format!("EC2 instance metadata unavailable: {m}"));
    let token = match send_with_cancel(
        input
            .http
            .put(format!("{base}/latest/api/token"))
            .header("x-aws-ec2-metadata-token-ttl-seconds", "21600")
            .timeout(timeout),
        input.cancel,
    )
    .await
    {
        Ok((s, t)) if s.is_success() => Some(t),
        Ok((s, _)) if s.as_u16() == 403 => {
            return Err(ChainError::Fatal(
                "EC2 instance metadata token request was refused (403)".into(),
            ));
        }
        Ok(_) if env("AWS_EC2_METADATA_V1_DISABLED").is_some_and(|v| v == "true") => {
            return Err(unavailable("IMDSv1 fallback disabled".into()));
        }
        Ok(_) => None,
        Err(e) => return Err(unavailable(e)),
    };
    let get = |path: String| {
        let mut b = input.http.get(format!("{base}{path}")).timeout(timeout);
        if let Some(t) = &token {
            b = b.header("x-aws-ec2-metadata-token", t.trim());
        }
        b
    };
    let (status, roles) = send_with_cancel(
        get("/latest/meta-data/iam/security-credentials/".into()),
        input.cancel,
    )
    .await
    .map_err(unavailable)?;
    let role = roles.lines().next().unwrap_or("").trim().to_owned();
    if !status.is_success() || role.is_empty() {
        return Err(unavailable("no instance role".into()));
    }
    let (status, text) = send_with_cancel(
        get(format!("/latest/meta-data/iam/security-credentials/{role}")),
        input.cancel,
    )
    .await
    .map_err(unavailable)?;
    if !status.is_success() {
        return Err(unavailable(format!("status {}", status.as_u16())));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| unavailable(e.to_string()))?;
    let creds = json_credentials(&v, "Token").map_err(unavailable)?;
    remember("imds", &creds);
    Ok(creds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::mock::{MockResponse, MockServer};

    fn vector_creds() -> AwsCredentials {
        AwsCredentials::new(
            "AKIDEXAMPLE",
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            None,
        )
    }

    fn vector_time() -> SystemTime {
        // 2015-08-30T12:36:00Z, the date of the AWS SigV4 test suite.
        UNIX_EPOCH + Duration::from_secs(1_440_938_160)
    }

    fn vector_sign(
        method: &str,
        url: &str,
        extra: &[(&str, &str)],
        service: &str,
    ) -> (String, Vec<(String, String)>) {
        let url = Url::parse(url).unwrap();
        let mut headers: Vec<(String, String)> = extra
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let creds = vector_creds();
        let sig = sign(
            method,
            &url,
            &mut headers,
            b"",
            &SigningParams {
                credentials: &creds,
                region: "us-east-1",
                service,
                time: vector_time(),
                content_sha256_header: false,
            },
        );
        (sig, headers)
    }

    #[test]
    fn dates_are_formatted_in_utc() {
        assert_eq!(
            amz_dates(vector_time()),
            ("20150830".to_owned(), "20150830T123600Z".to_owned())
        );
        assert_eq!(parse_iso8601("2015-08-30T12:36:00Z"), Some(1_440_938_160));
        assert_eq!(
            parse_iso8601("2015-08-30T12:36:00.123Z"),
            Some(1_440_938_160)
        );
        assert_eq!(
            amz_dates(UNIX_EPOCH + Duration::from_secs(951_782_400)).0,
            "20000229"
        );
    }

    /// AWS SigV4 test suite: `get-vanilla` and `post-vanilla`.
    #[test]
    fn sigv4_test_suite_vanilla_vectors() {
        let (sig, headers) = vector_sign("GET", "https://example.amazonaws.com/", &[], "service");
        assert_eq!(
            sig,
            "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
        let auth = headers.iter().find(|(k, _)| k == "authorization").unwrap();
        assert_eq!(
            auth.1,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
        let (sig, _) = vector_sign("POST", "https://example.amazonaws.com/", &[], "service");
        assert_eq!(
            sig,
            "5da7c1a2acd57cee7505fc6676e4e544621c30862966e37dddb68e92efbe5d6b"
        );
    }

    /// The worked example from the AWS SigV4 documentation (IAM `ListUsers`).
    #[test]
    fn sigv4_documentation_example() {
        assert_eq!(
            hex(&signing_key(
                "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
                "20150830",
                "us-east-1",
                "iam"
            )),
            "c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9"
        );
        let (sig, _) = vector_sign(
            "GET",
            "https://iam.amazonaws.com/?Action=ListUsers&Version=2010-05-08",
            &[(
                "Content-Type",
                "application/x-www-form-urlencoded; charset=utf-8",
            )],
            "iam",
        );
        assert_eq!(
            sig,
            "5d672d79c15b13162d9279b0855cfba6789a8edb4c82c400e06b5924a6f2b5d7"
        );
    }

    #[test]
    fn non_s3_paths_are_encoded_twice_and_tokens_signed() {
        let model = encode_path_segment("anthropic.claude-3-5-sonnet-20240620-v1:0");
        assert_eq!(model, "anthropic.claude-3-5-sonnet-20240620-v1%3A0");
        let url = Url::parse(&format!(
            "https://bedrock-runtime.us-east-1.amazonaws.com/model/{model}/converse-stream"
        ))
        .unwrap();
        let mut headers = vec![("user-agent".to_owned(), "x".to_owned())];
        let mut creds = vector_creds();
        creds.session_token = Some("TOKEN".into());
        sign(
            "POST",
            &url,
            &mut headers,
            b"{}",
            &SigningParams {
                credentials: &creds,
                region: "us-east-1",
                service: "bedrock",
                time: vector_time(),
                content_sha256_header: true,
            },
        );
        let auth = &headers
            .iter()
            .find(|(k, _)| k == "authorization")
            .unwrap()
            .1;
        assert!(
            auth.contains(
                "SignedHeaders=host;x-amz-content-sha256;x-amz-date;x-amz-security-token,"
            )
        );
        assert!(
            headers
                .iter()
                .any(|(k, v)| k == "x-amz-security-token" && v == "TOKEN")
        );
        assert_eq!(
            uri_encode(url.path(), true),
            "/model/anthropic.claude-3-5-sonnet-20240620-v1%253A0/converse-stream"
        );
    }

    #[test]
    fn ini_parsing_handles_config_and_credentials_styles() {
        let config = "[default]\nregion = us-west-2\n[profile dev]\nregion=eu-west-1\ns3 =\n  max = 1\nrole_arn = arn:aws:iam::1:role/r\n# c\n[sso-session x]\nsso_region=y\n";
        let p = parse_ini(config, true);
        assert_eq!(p["default"]["region"], "us-west-2");
        assert_eq!(p["dev"]["region"], "eu-west-1");
        assert!(!p["dev"].contains_key("max"));
        assert!(!p.contains_key("sso-session x"));
        let creds = parse_ini("[dev]\naws_access_key_id = A\n", false);
        assert_eq!(creds["dev"]["aws_access_key_id"], "A");
    }

    struct Fixture {
        dir: tempfile::TempDir,
        vars: HashMap<String, String>,
    }

    impl Fixture {
        fn new(config: &str, credentials: &str) -> Fixture {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("config"), config).unwrap();
            std::fs::write(dir.path().join("credentials"), credentials).unwrap();
            let mut vars = HashMap::new();
            vars.insert(
                "AWS_CONFIG_FILE".into(),
                dir.path().join("config").display().to_string(),
            );
            vars.insert(
                "AWS_SHARED_CREDENTIALS_FILE".into(),
                dir.path().join("credentials").display().to_string(),
            );
            vars.insert("HOME".into(), dir.path().display().to_string());
            vars.insert("AWS_EC2_METADATA_DISABLED".into(), "true".into());
            Fixture { dir, vars }
        }

        fn set(&mut self, k: &str, v: &str) {
            self.vars.insert(k.into(), v.into());
        }

        async fn resolve(
            &self,
            explicit: Option<&str>,
            ambient: Option<&str>,
        ) -> Result<AwsCredentials, String> {
            let vars = self.vars.clone();
            let lookup = move |k: &str| vars.get(k).cloned().filter(|v| !v.is_empty());
            let http = crate::provider::http::client();
            let cancel = CancellationToken::new();
            resolve_credentials(&ChainInput {
                env: &lookup,
                explicit_profile: explicit.map(str::to_owned),
                ambient_profile: ambient.map(str::to_owned),
                region: "us-east-1".into(),
                http: &http,
                cancel: &cancel,
            })
            .await
        }
    }

    #[tokio::test]
    async fn env_keys_profiles_and_precedence() {
        let mut f = Fixture::new(
            "[profile dev]\nregion = eu-west-1\n",
            "[default]\naws_access_key_id = DEF\naws_secret_access_key = defsecret\n[dev]\naws_access_key_id = DEV\naws_secret_access_key = devsecret\naws_session_token = devtoken\n",
        );
        // No env keys: the default profile.
        assert_eq!(f.resolve(None, None).await.unwrap().access_key_id, "DEF");
        // Ambient profile.
        let dev = f.resolve(None, Some("dev")).await.unwrap();
        assert_eq!(
            (dev.access_key_id.as_str(), dev.session_token.as_deref()),
            ("DEV", Some("devtoken"))
        );
        f.set("AWS_ACCESS_KEY_ID", "ENV");
        f.set("AWS_SECRET_ACCESS_KEY", "envsecret");
        // Environment keys beat an ambient profile ...
        assert_eq!(
            f.resolve(None, Some("dev")).await.unwrap().access_key_id,
            "ENV"
        );
        // ... but an explicitly chosen profile beats environment keys.
        assert_eq!(
            f.resolve(Some("dev"), None).await.unwrap().access_key_id,
            "DEV"
        );
        // A missing profile falls through to nothing.
        let err = f.resolve(Some("nope"), None).await.unwrap_err();
        assert_eq!(err, "Could not load credentials from any providers");
        let lookup = |k: &str| f.vars.get(k).cloned();
        assert_eq!(profile_region(&lookup, "dev").as_deref(), Some("eu-west-1"));
        drop(f.dir);
    }

    #[tokio::test]
    async fn incomplete_sso_and_unsupported_mfa_profiles_fail_clearly() {
        let f = Fixture::new(
            "[profile sso]\nsso_session = corp\n[profile mfa]\nrole_arn = arn:aws:iam::1:role/r\nsource_profile = base\nmfa_serial = arn:mfa\n",
            "[base]\naws_access_key_id = B\naws_secret_access_key = b\n",
        );
        let err = f.resolve(Some("sso"), None).await.unwrap_err();
        assert!(err.contains("IAM Identity Center"), "{err}");
        let err = f.resolve(Some("mfa"), None).await.unwrap_err();
        assert!(err.contains("MFA"), "{err}");
    }

    const STS_OK: &str = "<AssumeRoleResponse><AssumeRoleResult><Credentials><AccessKeyId>ASIAROLE</AccessKeyId><SecretAccessKey>rolesecret</SecretAccessKey><SessionToken>roletoken&amp;x</SessionToken><Expiration>2099-01-01T00:00:00Z</Expiration></Credentials></AssumeRoleResult></AssumeRoleResponse>";

    #[tokio::test]
    async fn assume_role_profiles_sign_an_sts_request() {
        let server = MockServer::start(|_, _| {
            let mut r = MockResponse::json(200, &serde_json::json!({}));
            r.headers = vec![("content-type".into(), "text/xml".into())];
            r.chunks = vec![STS_OK.as_bytes().to_vec()];
            r
        })
        .await;
        let mut f = Fixture::new(
            "[profile role]\nrole_arn = arn:aws:iam::123:role/test\nsource_profile = base\nrole_session_name = s1\n",
            "[base]\naws_access_key_id = BASE\naws_secret_access_key = basesecret\n",
        );
        f.set("AWS_ENDPOINT_URL_STS", &server.url);
        let c = f.resolve(Some("role"), None).await.unwrap();
        assert_eq!(c.access_key_id, "ASIAROLE");
        assert_eq!(c.session_token.as_deref(), Some("roletoken&x"));
        assert_eq!(c.expires_at, parse_iso8601("2099-01-01T00:00:00Z"));
        let req = &server.requests()[0];
        assert_eq!(req.method, "POST");
        let body = String::from_utf8(req.body.clone()).unwrap();
        assert_eq!(
            body,
            "Action=AssumeRole&Version=2011-06-15&RoleArn=arn%3Aaws%3Aiam%3A%3A123%3Arole%2Ftest&RoleSessionName=s1"
        );
        let auth = req.header("authorization").unwrap();
        assert!(
            auth.starts_with("AWS4-HMAC-SHA256 Credential=BASE/"),
            "{auth}"
        );
        assert!(auth.contains("/us-east-1/sts/aws4_request"));
        // Cached until near expiry: a second resolve makes no request.
        f.resolve(Some("role"), None).await.unwrap();
        assert_eq!(server.requests().len(), 1);
    }

    #[tokio::test]
    async fn web_identity_env_calls_sts_unsigned() {
        let server = MockServer::start(|_, _| {
            let mut r = MockResponse::json(200, &serde_json::json!({}));
            r.chunks = vec![
                STS_OK
                    .replace("AssumeRole", "AssumeRoleWithWebIdentity")
                    .into_bytes(),
            ];
            r
        })
        .await;
        let mut f = Fixture::new("", "");
        let token = f.dir.path().join("token");
        std::fs::write(&token, "jwt-token\n").unwrap();
        f.set("AWS_WEB_IDENTITY_TOKEN_FILE", &token.display().to_string());
        f.set("AWS_ROLE_ARN", "arn:aws:iam::1:role/webid-unique");
        f.set("AWS_ROLE_SESSION_NAME", "sess");
        f.set("AWS_ENDPOINT_URL_STS", &server.url);
        let c = f.resolve(None, None).await.unwrap();
        assert_eq!(c.access_key_id, "ASIAROLE");
        let req = &server.requests()[0];
        assert!(req.header("authorization").is_none());
        let body = String::from_utf8(req.body.clone()).unwrap();
        assert!(body.contains("Action=AssumeRoleWithWebIdentity"));
        assert!(
            body.ends_with("&RoleSessionName=sess&WebIdentityToken=jwt-token"),
            "{body}"
        );
    }

    #[tokio::test]
    async fn sts_errors_are_reported() {
        let server = MockServer::start(|_, _| {
            let mut r = MockResponse::json(403, &serde_json::json!({}));
            r.chunks = vec![b"<ErrorResponse><Error><Code>AccessDenied</Code><Message>no &amp; no</Message></Error></ErrorResponse>".to_vec()];
            r
        })
        .await;
        let mut f = Fixture::new(
            "[profile role]\nrole_arn = arn:aws:iam::123:role/denied\nsource_profile = base\n",
            "[base]\naws_access_key_id = BASE\naws_secret_access_key = basesecret\n",
        );
        f.set("AWS_ENDPOINT_URL_STS", &server.url);
        let err = f.resolve(Some("role"), None).await.unwrap_err();
        assert_eq!(err, "STS AssumeRole failed: AccessDenied: no & no");
    }

    #[tokio::test]
    async fn container_endpoint_with_authorization_token() {
        let server = MockServer::start(|req, _| {
            assert_eq!(req.header("authorization"), Some("Bearer ecs-token"));
            MockResponse::json(
                200,
                &serde_json::json!({"AccessKeyId": "ECSKEY", "SecretAccessKey": "s", "Token": "t",
                    "Expiration": "2099-01-01T00:00:00Z"}),
            )
        })
        .await;
        let mut f = Fixture::new("", "");
        f.set(
            "AWS_CONTAINER_CREDENTIALS_FULL_URI",
            &format!("{}/creds/unique-1", server.url),
        );
        let token = f.dir.path().join("ecs-token");
        std::fs::write(&token, "Bearer ecs-token\n").unwrap();
        f.set(
            "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE",
            &token.display().to_string(),
        );
        let c = f.resolve(None, None).await.unwrap();
        assert_eq!(
            (c.access_key_id.as_str(), c.session_token.as_deref()),
            ("ECSKEY", Some("t"))
        );
        assert_eq!(server.requests()[0].path, "/creds/unique-1");

        let mut bad = Fixture::new("", "");
        bad.set(
            "AWS_CONTAINER_CREDENTIALS_FULL_URI",
            "http://evil.example.com/creds",
        );
        assert!(
            bad.resolve(None, None)
                .await
                .unwrap_err()
                .contains("not a valid container")
        );
    }

    #[tokio::test]
    async fn imds_v2_flow() {
        let server = MockServer::start(|req, _| match (req.method.as_str(), req.path.as_str()) {
            ("PUT", "/latest/api/token") => {
                let mut r = MockResponse::json(200, &serde_json::json!({}));
                r.chunks = vec![b"imds-token".to_vec()];
                r
            }
            ("GET", "/latest/meta-data/iam/security-credentials/") => {
                assert_eq!(req.header("x-aws-ec2-metadata-token"), Some("imds-token"));
                let mut r = MockResponse::json(200, &serde_json::json!({}));
                r.chunks = vec![b"my-role\n".to_vec()];
                r
            }
            _ => MockResponse::json(
                200,
                &serde_json::json!({"Code": "Success", "AccessKeyId": "IMDSKEY", "SecretAccessKey": "s",
                    "Token": "t", "Expiration": "2099-01-01T00:00:00Z"}),
            ),
        })
        .await;
        let mut f = Fixture::new("", "");
        f.vars.remove("AWS_EC2_METADATA_DISABLED");
        f.set("AWS_EC2_METADATA_SERVICE_ENDPOINT", &server.url);
        // The IMDS cache key is global; clear it so this test is independent.
        cache().lock().unwrap().remove("imds");
        let c = f.resolve(None, None).await.unwrap();
        assert_eq!(c.access_key_id, "IMDSKEY");
        assert_eq!(
            server.requests()[2].path,
            "/latest/meta-data/iam/security-credentials/my-role"
        );
        cache().lock().unwrap().remove("imds");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn credential_process_profiles() {
        let f = Fixture::new(
            "[profile proc]\ncredential_process = printf '{\"Version\":1,\"AccessKeyId\":\"PROC\",\"SecretAccessKey\":\"s\",\"SessionToken\":\"t\"}'\n[profile broken]\ncredential_process = sh -c 'echo nope >&2; exit 3'\n",
            "",
        );
        let c = f.resolve(Some("proc"), None).await.unwrap();
        assert_eq!(
            (c.access_key_id.as_str(), c.session_token.as_deref()),
            ("PROC", Some("t"))
        );
        let err = f.resolve(Some("broken"), None).await.unwrap_err();
        assert!(err.contains("exited") && err.contains("nope"), "{err}");
    }
}
