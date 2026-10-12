//! AWS CLI `aws login` cache and signed refresh. This is distinct from SSO.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

use super::{AwsCredentials, ChainInput, home, now_secs, sha256_hex};

const LIMIT: usize = 1024 * 1024;
const BUDGET: Duration = Duration::from_secs(30);
const LOGIN: &str = "Please reauthenticate using aws login with the corresponding profile.";
fn text(v: &Value, k: &str) -> Result<String, String> {
    v[k].as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("AWS login cache/response is missing {k}. {LOGIN}"))
}
fn expiry(v: &Value) -> Result<u64, String> {
    v.as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .and_then(|d| u64::try_from(d.timestamp()).ok())
        .ok_or_else(|| format!("AWS login has invalid expiry. {LOGIN}"))
}
fn credentials(token: &Value) -> Result<AwsCredentials, String> {
    let access = &token["accessToken"];
    text(access, "accountId")?;
    Ok(AwsCredentials {
        access_key_id: text(access, "accessKeyId")?,
        secret_access_key: text(access, "secretAccessKey")?,
        session_token: Some(text(access, "sessionToken")?),
        expires_at: Some(expiry(&access["expiresAt"])?),
    })
}
async fn read(path: &std::path::Path) -> Result<Value, String> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| format!("Cannot read AWS login cache. {LOGIN}"))?;
    if !file
        .metadata()
        .await
        .map_err(|_| "Cannot inspect AWS login cache")?
        .is_file()
    {
        return Err("AWS login cache is not a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| "Cannot read AWS login cache")?;
    if bytes.len() > LIMIT {
        return Err("AWS login cache is too large".into());
    }
    let token: Value = serde_json::from_slice(&bytes)
        .map_err(|_| format!("Invalid AWS login cache JSON. {LOGIN}"))?;
    for k in ["clientId", "refreshToken", "dpopKey"] {
        text(&token, k)?;
    }
    credentials(&token)?;
    Ok(token)
}

fn der(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut output = vec![tag];
    if body.len() < 128 {
        output.push(body.len() as u8);
    } else {
        let bytes = body.len().to_be_bytes();
        let start = bytes.iter().position(|b| *b != 0).unwrap();
        output.push(0x80 | (bytes.len() - start) as u8);
        output.extend_from_slice(&bytes[start..]);
    }
    output.extend_from_slice(body);
    output
}
fn key(pem: &str) -> Result<EcdsaKeyPair, String> {
    let sec1 = pem.starts_with("-----BEGIN EC PRIVATE KEY-----");
    if !sec1 && !pem.starts_with("-----BEGIN PRIVATE KEY-----") {
        return Err("AWS login DPoP key must be a P-256 private key".into());
    }
    let b64: String = pem
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .flat_map(|l| l.chars().filter(|c| !c.is_whitespace()))
        .collect();
    let mut bytes = STANDARD
        .decode(b64)
        .map_err(|_| "Invalid AWS login DPoP key encoding")?;
    if sec1 {
        // Wrap SEC1 in PKCS#8 with the ecPublicKey and prime256v1 OIDs. Ring validates
        // the private/public key pair and curve; no ASN.1 signature conversion is needed.
        let mut body = vec![
            2, 1, 0, 0x30, 0x13, 6, 7, 0x2a, 0x86, 0x48, 0xce, 0x3d, 2, 1, 6, 8, 0x2a, 0x86, 0x48,
            0xce, 0x3d, 3, 1, 7,
        ];
        body.extend(der(4, &bytes));
        bytes = der(0x30, &body);
    }
    EcdsaKeyPair::from_pkcs8(
        &ECDSA_P256_SHA256_FIXED_SIGNING,
        &bytes,
        &ring::rand::SystemRandom::new(),
    )
    .map_err(|_| "Invalid AWS login P-256 DPoP key".into())
}
fn proof(pem: &str, endpoint: &str) -> Result<String, String> {
    let key = key(pem)?;
    let point = key.public_key().as_ref();
    let header = json!({"alg":"ES256","typ":"dpop+jwt","jwk":{"kty":"EC","crv":"P-256","x":URL_SAFE_NO_PAD.encode(&point[1..33]),"y":URL_SAFE_NO_PAD.encode(&point[33..65])}});
    let mut id = [0u8; 16];
    getrandom::getrandom(&mut id).map_err(|_| "Cannot create AWS login proof ID")?;
    id[6] = (id[6] & 0x0f) | 0x40;
    id[8] = (id[8] & 0x3f) | 0x80;
    let h = super::hex(&id);
    let jti = format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    );
    let payload = json!({"jti":jti,"htm":"POST","htu":endpoint,"iat":now_secs()});
    let message = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header.to_string()),
        URL_SAFE_NO_PAD.encode(payload.to_string())
    );
    let sig = key
        .sign(&ring::rand::SystemRandom::new(), message.as_bytes())
        .map_err(|_| "Cannot sign AWS login proof")?;
    Ok(format!(
        "{message}.{}",
        URL_SAFE_NO_PAD.encode(sig.as_ref())
    ))
}

fn endpoint(input: &ChainInput<'_>, profile: &HashMap<String, String>) -> Result<String, String> {
    let region = profile.get("region").unwrap_or(&input.region);
    if region.is_empty()
        || !region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err("Invalid AWS login region".into());
    }
    let suffix = if region.starts_with("cn-") {
        "amazonaws.cn"
    } else if region.starts_with("us-gov-") {
        "amazonaws-us-gov.com"
    } else if region.starts_with("us-isob-") {
        "sc2shome.sgov.gov"
    } else if region.starts_with("us-iso-") {
        "c2shome.ic.gov"
    } else if region.starts_with("eu-isoe-") {
        "csphome.adc-e.uk"
    } else if region.starts_with("us-isof-") {
        "csphome.hci.ic.gov"
    } else if region.starts_with("eusc-") {
        "amazonaws-eusc.eu"
    } else {
        "aws.amazon.com"
    };
    let raw = (input.env)("AWS_ENDPOINT_URL_SIGNIN")
        .or_else(|| (input.env)("AWS_ENDPOINT_URL"))
        .unwrap_or_else(|| format!("https://{region}.signin.{suffix}"));
    let url = url::Url::parse(&raw).map_err(|_| "Invalid AWS login endpoint")?;
    if (url.scheme() != "https"
        && !(url.scheme() == "http"
            && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("AWS login endpoint requires HTTPS (or loopback HTTP) and no credentials/query/fragment".into());
    }
    // Sign the same normalized endpoint that reqwest sends (host casing and default
    // ports included), rather than the user's raw spelling of it.
    Ok(format!("{}/v1/token", url.as_str().trim_end_matches('/')))
}

fn owner(path: &std::path::Path) -> Result<Arc<tokio::sync::Mutex<()>>, String> {
    type Owners = HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>;
    static OWNERS: OnceLock<Mutex<Owners>> = OnceLock::new();
    let mut all = OWNERS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(owner) = all.get(path) {
        return Ok(owner.clone());
    }
    if all.len() >= 64 {
        let idle = all
            .iter()
            .find(|(_, v)| Arc::strong_count(v) == 1)
            .map(|(k, _)| k.clone())
            .ok_or("Too many concurrent AWS login sessions")?;
        all.remove(&idle);
    }
    let owner = Arc::new(tokio::sync::Mutex::new(()));
    all.insert(path.to_owned(), owner.clone());
    Ok(owner)
}

async fn save(path: PathBuf, old: Value, new: Value) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        use crate::auth::store::{CredentialStore, FileStore, StoredCredential};
        let store = FileStore::new(path).with_lock_wait(Duration::from_secs(1));
        let mut tx = store.begin().map_err(|_| "Cannot lock AWS login cache")?;
        for k in ["accessToken", "refreshToken", "clientId", "dpopKey"] {
            if tx.get(k) != old.get(k).map(|v| StoredCredential::Unsupported(v.clone())) {
                return Ok(());
            }
        }
        for k in ["accessToken", "refreshToken"] {
            tx.set(k, StoredCredential::Unsupported(new[k].clone()));
        }
        tx.commit()
            .map_err(|_| "Cannot save AWS login cache".to_owned())
    })
    .await
    .map_err(|_| "AWS login cache writer failed".to_owned())?
}

enum RefreshError {
    Denied(String),
    Other(String),
}
impl From<String> for RefreshError {
    fn from(e: String) -> Self {
        Self::Other(e)
    }
}
async fn refresh(
    http: &reqwest::Client,
    path: &std::path::Path,
    endpoint: &str,
    old: &Value,
) -> Result<AwsCredentials, RefreshError> {
    let dpop = proof(&text(old, "dpopKey")?, endpoint)?;
    let body = json!({
        "clientId": text(old, "clientId")?,
        "grantType": "refresh_token",
        "refreshToken": text(old, "refreshToken")?,
    });
    let mut response = http
        .post(endpoint)
        .header("DPoP", dpop)
        .json(&body)
        .timeout(BUDGET)
        .send()
        .await
        .map_err(|e| crate::provider::http::transport_error(&e))?;
    let status = response.status();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_owned();
    // Once denial is observed, malformed or oversized error text cannot turn it
    // into a transient failure that permits using the old credentials.
    let response_error = |message: String| {
        if status.as_u16() == 403 || error_type == "AccessDeniedException" {
            RefreshError::Denied(format!("AWS login refresh was denied. {LOGIN}"))
        } else {
            RefreshError::Other(message)
        }
    };
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| response_error(crate::provider::http::transport_error(&e)))?
    {
        if bytes.len().saturating_add(chunk.len()) > LIMIT {
            return Err(response_error("AWS login response is too large".into()));
        }
        bytes.extend_from_slice(&chunk);
    }
    let body: Value = serde_json::from_slice(&bytes)
        .map_err(|_| response_error("Invalid AWS login response JSON".into()))?;
    if !status.is_success() {
        let body_type = body["__type"]
            .as_str()
            .unwrap_or("")
            .rsplit('#')
            .next()
            .unwrap_or("");
        if status.as_u16() == 403
            || error_type == "AccessDeniedException"
            || body_type == "AccessDeniedException"
        {
            let hint = match body["error"].as_str() {
                Some("TOKEN_EXPIRED") => "AWS login session expired.",
                Some("USER_CREDENTIALS_CHANGED") => "AWS login credentials changed.",
                Some("INSUFFICIENT_PERMISSIONS") => {
                    "AWS login lacks permission for CreateOAuth2Token."
                }
                _ => "AWS login refresh was denied.",
            };
            return Err(RefreshError::Denied(format!("{hint} {LOGIN}")));
        }
        return Err(RefreshError::Other(format!(
            "AWS login refresh failed (HTTP {}). {LOGIN}",
            status.as_u16()
        )));
    }
    let mut updated = old.clone();
    for k in ["accessKeyId", "secretAccessKey", "sessionToken"] {
        updated["accessToken"][k] = json!(text(&body["accessToken"], k)?);
    }
    updated["refreshToken"] = json!(text(&body, "refreshToken")?);
    let seconds = match body.get("expiresIn") {
        None => 900,
        Some(v) => v
            .as_u64()
            .filter(|s| *s > 0)
            .ok_or_else(|| "Invalid AWS login expiresIn".to_owned())?,
    };
    let expires = now_secs()
        .checked_add(seconds)
        .and_then(|v| i64::try_from(v).ok())
        .and_then(|v| chrono::DateTime::from_timestamp(v, 0))
        .ok_or_else(|| "Invalid AWS login expiry".to_owned())?;
    updated["accessToken"]["expiresAt"] =
        json!(expires.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
    save(path.to_owned(), old.clone(), updated.clone()).await?;
    credentials(&updated).map_err(Into::into)
}

pub(super) async fn resolve(
    input: &ChainInput<'_>,
    profile: &HashMap<String, String>,
) -> Result<AwsCredentials, String> {
    if input.cancel.is_cancelled() {
        return Err("Request was aborted".into());
    }
    let session = profile
        .get("login_session")
        .filter(|s| !s.is_empty())
        .ok_or("Missing AWS login_session")?;
    let directory = (input.env)("AWS_LOGIN_CACHE_DIRECTORY")
        .map(PathBuf::from)
        .or_else(|| home(input.env).map(|h| PathBuf::from(h).join(".aws/login/cache")))
        .ok_or("AWS login cache needs HOME or AWS_LOGIN_CACHE_DIRECTORY")?;
    let path = directory.join(format!("{}.json", sha256_hex(session.as_bytes())));
    let endpoint = endpoint(input, profile)?;
    let owner = owner(&path)?;
    let http = input.http.clone();
    let mut task = tokio::spawn(async move {
        tokio::time::timeout(BUDGET, async {
            let _guard = owner.lock().await;
            let token = read(&path).await?;
            let existing = credentials(&token)?;
            if existing.expires_at.unwrap() > now_secs().saturating_add(300) {
                return Ok(existing);
            }
            match refresh(&http, &path, &endpoint, &token).await {
                Ok(c) => Ok(c),
                Err(RefreshError::Denied(e)) => Err(e),
                Err(RefreshError::Other(_)) if existing.expires_at.unwrap() > now_secs() => {
                    Ok(existing)
                }
                Err(RefreshError::Other(e)) => Err(format!("{e} {LOGIN}")),
            }
        })
        .await
        .map_err(|_| "AWS login exchange timed out".to_owned())?
    });
    tokio::select! { _=input.cancel.cancelled()=>Err("Request was aborted".into()), result=&mut task=>result.map_err(|_|"AWS login task failed".to_owned())? }
}

#[cfg(test)]
#[path = "aws_login_tests.rs"]
mod tests;
