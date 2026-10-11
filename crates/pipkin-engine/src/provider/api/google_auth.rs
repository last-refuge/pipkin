//! Google Application Default Credentials for Vertex AI.
//!
//! Reference behaviour: `@google/genai` (Node) builds a `google-auth-library` `GoogleAuth` with
//! the `cloud-platform` scope and `keyFilename = GOOGLE_APPLICATION_CREDENTIALS`
//! (`packages/ai/src/api/google-vertex.ts:96-110`, `:357-460`). That library reads the
//! credentials file (the env path, else `~/.config/gcloud/application_default_credentials.json`)
//! and exchanges it for an access token:
//!
//! - `service_account`: an RS256-signed JWT assertion (`iss`, `scope`, `aud`, `iat`, `exp`) is
//!   posted with `grant_type=urn:ietf:params:oauth:grant-type:jwt-bearer`.
//! - `authorized_user` (`gcloud auth application-default login`): a refresh-token grant with the
//!   file's client id/secret.
//!
//! Tokens are cached per credentials file and token endpoint until five minutes before expiry.
//! `quota_project_id` becomes the `x-goog-user-project` header. The token endpoint can be
//! overridden with the `googleTokenUrl` request option (tests point it at a local mock).
//! Not ported: external-account (workload identity pool) and impersonated credential files, and
//! the GCE/Cloud Run metadata-server fallback when no file exists.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::google_shared::opt_str;
use crate::provider::{ProviderRequest, StreamCtx, http};

pub(crate) const CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
pub(crate) const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const JWT_BEARER_GRANT: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";
/// Refresh this long before the token expires (google-auth-library `eagerRefreshThresholdMillis`).
const EAGER_REFRESH: Duration = Duration::from_secs(5 * 60);
const TOKEN_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const NO_CREDENTIALS: &str = "Could not load the default credentials. Browse to https://cloud.google.com/docs/authentication/getting-started for more information.";

/// An access token plus the quota project to bill, if the credentials name one.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AccessToken {
    pub token: String,
    pub quota_project: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
enum CredentialsFile {
    ServiceAccount {
        client_email: String,
        private_key: String,
        quota_project: Option<String>,
    },
    AuthorizedUser {
        client_id: String,
        client_secret: String,
        refresh_token: String,
        quota_project: Option<String>,
    },
}

struct Cached {
    token: AccessToken,
    expires: Instant,
}

fn cache() -> &'static Mutex<HashMap<String, Cached>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Cached>>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

/// The credentials file to use: `GOOGLE_APPLICATION_CREDENTIALS`, else the gcloud ADC file.
pub(crate) fn credentials_path(req: &ProviderRequest, explicit: Option<String>) -> Option<PathBuf> {
    if let Some(path) = explicit.or_else(|| req.env_var("GOOGLE_APPLICATION_CREDENTIALS")) {
        return Some(expand_home(req, &path));
    }
    let home = req.env_var("HOME")?;
    Some(
        Path::new(&home)
            .join(".config")
            .join("gcloud")
            .join("application_default_credentials.json"),
    )
}

fn expand_home(req: &ProviderRequest, path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), req.env_var("HOME")) {
        (Some(rest), Some(home)) => Path::new(&home).join(rest),
        _ => PathBuf::from(path),
    }
}

fn field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn parse_credentials(text: &str) -> Result<CredentialsFile, String> {
    let v: Value = serde_json::from_str(text)
        .map_err(|e| format!("The Google credentials file is not valid JSON: {e}"))?;
    let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
    let missing = |k: &str| format!("The Google {kind} credentials file has no {k}");
    let quota_project = field(&v, "quota_project_id");
    match kind {
        "service_account" => Ok(CredentialsFile::ServiceAccount {
            client_email: field(&v, "client_email").ok_or_else(|| missing("client_email"))?,
            private_key: field(&v, "private_key").ok_or_else(|| missing("private_key"))?,
            quota_project,
        }),
        "authorized_user" => Ok(CredentialsFile::AuthorizedUser {
            client_id: field(&v, "client_id").ok_or_else(|| missing("client_id"))?,
            client_secret: field(&v, "client_secret").ok_or_else(|| missing("client_secret"))?,
            refresh_token: field(&v, "refresh_token").ok_or_else(|| missing("refresh_token"))?,
            quota_project,
        }),
        other => Err(format!(
            "Google credentials of type \"{other}\" are not supported by this engine (use a service account key or `gcloud auth application-default login`)"
        )),
    }
}

/// DER bytes of a PEM block, and whether it is PKCS#8 (`PRIVATE KEY`) or PKCS#1.
fn pem_to_der(pem: &str) -> Result<(Vec<u8>, bool), String> {
    let pkcs8 = pem.contains("-----BEGIN PRIVATE KEY-----");
    let pkcs1 = pem.contains("-----BEGIN RSA PRIVATE KEY-----");
    if !pkcs8 && !pkcs1 {
        return Err("The service account private key is not a PEM private key".into());
    }
    let body: String = pem
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .flat_map(|l| l.chars().filter(|c| !c.is_whitespace()))
        .collect();
    let der = STANDARD
        .decode(body)
        .map_err(|_| "The service account private key is not valid base64".to_string())?;
    Ok((der, pkcs8))
}

/// An RS256 JSON Web Signature over `header.claims`.
pub(crate) fn sign_jwt_rs256(pem: &str, header: &Value, claims: &Value) -> Result<String, String> {
    use ring::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
    let (der, pkcs8) = pem_to_der(pem)?;
    let key = if pkcs8 {
        RsaKeyPair::from_pkcs8(&der)
    } else {
        RsaKeyPair::from_der(&der)
    }
    .map_err(|e| format!("The service account private key was rejected: {e}"))?;
    let input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header.to_string()),
        URL_SAFE_NO_PAD.encode(claims.to_string())
    );
    let mut sig = vec![0u8; key.public().modulus_len()];
    key.sign(
        &RSA_PKCS1_SHA256,
        &ring::rand::SystemRandom::new(),
        input.as_bytes(),
        &mut sig,
    )
    .map_err(|_| "Could not sign the service account assertion".to_string())?;
    Ok(format!("{input}.{}", URL_SAFE_NO_PAD.encode(sig)))
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The form fields for a token request.
fn grant(file: &CredentialsFile) -> Result<Vec<(&'static str, String)>, String> {
    match file {
        CredentialsFile::ServiceAccount {
            client_email,
            private_key,
            ..
        } => {
            let iat = unix_now();
            let claims = json!({
                "iss": client_email,
                "scope": CLOUD_PLATFORM_SCOPE,
                "aud": GOOGLE_TOKEN_URL,
                "exp": iat + 3600,
                "iat": iat,
            });
            let assertion = sign_jwt_rs256(private_key, &json!({"alg": "RS256"}), &claims)?;
            Ok(vec![
                ("grant_type", JWT_BEARER_GRANT.into()),
                ("assertion", assertion),
            ])
        }
        CredentialsFile::AuthorizedUser {
            client_id,
            client_secret,
            refresh_token,
            ..
        } => Ok(vec![
            ("refresh_token", refresh_token.clone()),
            ("client_id", client_id.clone()),
            ("client_secret", client_secret.clone()),
            ("grant_type", "refresh_token".into()),
        ]),
    }
}

async fn exchange(
    ctx: &StreamCtx<'_>,
    token_url: &str,
    file: &CredentialsFile,
) -> Result<(String, Duration), String> {
    let body = {
        let mut form = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in grant(file)? {
            form.append_pair(k, &v);
        }
        form.finish()
    };
    let send = ctx
        .http
        .post(token_url)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept", "application/json")
        .timeout(TOKEN_REQUEST_TIMEOUT)
        .body(body)
        .send();
    let resp = tokio::select! {
        _ = ctx.cancel.cancelled() => return Err("Request aborted".into()),
        r = send => r.map_err(|e| http::transport_error(&e))?,
    };
    let status = resp.status();
    let text = tokio::select! {
        _ = ctx.cancel.cancelled() => return Err("Request aborted".into()),
        t = resp.text() => t.map_err(|e| http::transport_error(&e))?,
    };
    let parsed: Option<Value> = serde_json::from_str(&text).ok();
    if !status.is_success() {
        if let Some(error) = parsed
            .as_ref()
            .and_then(|v| v.get("error"))
            .and_then(Value::as_str)
        {
            let description = parsed
                .as_ref()
                .and_then(|v| v.get("error_description"))
                .and_then(Value::as_str)
                .unwrap_or("undefined");
            return Err(format!("{}: {error}: {description}", status.as_u16()));
        }
        return Err(http::format_error(Some(status), &text, ""));
    }
    let parsed = parsed.ok_or("The Google token endpoint returned invalid JSON")?;
    let token = parsed
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("The Google token endpoint returned no access_token")?;
    let expires_in = parsed
        .get("expires_in")
        .and_then(Value::as_u64)
        .unwrap_or(3600);
    Ok((token.to_owned(), Duration::from_secs(expires_in)))
}

/// An access token for the credentials file at `path`, from the cache or a fresh exchange.
pub(crate) async fn access_token(ctx: &StreamCtx<'_>, path: &Path) -> Result<AccessToken, String> {
    let token_url = opt_str(ctx.req, "googleTokenUrl").unwrap_or(GOOGLE_TOKEN_URL);
    let key = format!("{}\n{token_url}", path.display());
    let mut cache = tokio::select! {
        _ = ctx.cancel.cancelled() => return Err("Request aborted".into()),
        guard = cache().lock() => guard,
    };
    if let Some(cached) = cache.get(&key)
        && Instant::now() + EAGER_REFRESH < cached.expires
    {
        return Ok(cached.token.clone());
    }
    let text = match tokio::fs::read_to_string(path).await {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(NO_CREDENTIALS.into()),
        Err(e) => {
            return Err(format!(
                "Could not read Google credentials at {}: {e}",
                path.display()
            ));
        }
    };
    let file = parse_credentials(&text)?;
    let started = Instant::now();
    let (token, lifetime) = exchange(ctx, token_url, &file).await?;
    let quota_project = match &file {
        CredentialsFile::ServiceAccount { quota_project, .. }
        | CredentialsFile::AuthorizedUser { quota_project, .. } => quota_project.clone(),
    };
    let token = AccessToken {
        token,
        quota_project,
    };
    cache.insert(
        key,
        Cached {
            token: token.clone(),
            expires: started + lifetime,
        },
    );
    Ok(token)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A throwaway 2048-bit RSA key generated for these tests only. It authorizes nothing.
    pub(crate) const TEST_PRIVATE_KEY: &str = r"-----BEGIN PRIVATE KEY-----
MIIEugIBADANBgkqhkiG9w0BAQEFAASCBKQwggSgAgEAAoIBAQCpp2vpU88r8ILJ
oqDDnuxFXH92V6l/CSVyANmVGJSSh0wBzl0z+9YQ6q1OI0HhkuWZQV7ufHJwCwXG
TN+yMWysbSv6N6ENDDOdB35TMDJS8r0NIcM8EEZlcjs2Xuh9HW43jQoRwDhhDlX/
9DJdll3cM/S84moKAMwHreTHV0mdl8d4tnwDj4VpV6KX49hps0Sl1uWw5ojwLjQH
8hnDbHMvelGqR8g061PQq3DYalBuru9OLuAnubgL/0JVH5cSlemwK6q+1QAPMW/p
Hdb+UC69qTfwbAebQHVaJ43LiSbnLSSZX4C+pURpwbJKJnAD2OYHs4d5p6Pj3JvH
qOrU1EjhAgMBAAECgf9KHK6HaeqWx0+wcIV073fyKkuEPHH5jNqkHWWaPE1VtW+p
WlJc4ybLPQ7Nk5lexqKGJd3SpjVXvM5I/Pa7toZCzMOUCw56egS8vwlFo0hYnrGi
NZ2UH4f2c8W/erZqnEYJqRM9Azud1SVnKsYrHATkrSMSCyvyMrxD893SjHnG3aZa
vfRsmsRlgQqW4Atxx5XK5QEFnmnRBnOTQeu9xP5wF5sTEqn8MmI4ENcL94ouGCHe
bZGRjly3sVmrUYj/v6TcmfNgXeJN7FB629JUfrcucL61xzMpOqV20i5Eaddu62Zg
sVfD4iOjQz05Vc5PVIW0NKo8Jg0U91/Ed5ly9HkCgYEA0e5qCd7wHfkbF1eCduWA
j8SrbozS+oz/mPNeEgm9GovVMmq7gsaluR9DNbaIhPGu7im4jyQRJ9jd+bTNomFz
RvGa4j4KTb9O/l6n/sD0r3YNxojjAwFRgVX9SrEJuvWfY6hqNDIPRF8wv/ltHvGa
xelUmWXPz5jqYDKYY60vkL0CgYEAzuJLvOwFmEVEUQvr8oaB/rquyuKOLBpzz4AR
8NY+pBZJE2vG3GE8v4L6N9KWDkyfJXij3Ebm6NlY1JQbT+lVDO+Z/NJ5N9V6BEQ+
+oalygkP+aToOQJARr7NmkUDLbmivNPAJPeoJobI7iY0erTXHiILrVrZ2FfgyJdV
40sAFPUCgYA+dcwQ8cbJdDNuR3gmvfP5Uo+bDcU8DLWDoW0sur8L5ezh2kW+DDKE
mNubYJrPy0S70af61sVP8MNipzvDg3ET+5CUndbc+P2cyzb6ohhrGBnTJyk73KmO
bLI3BeOaP7hTo2kt7HMEaboMdD7WRRZCqkCkf1lML/gT9RnIFmAflQKBgHJFbn4Y
g/Kad+SoSBg6kX0I0llAEDVuGv/HBqHXRNxPbueqRToe/3Xsu5av27BkRBdnbmLS
SuLcgoNQl1FpWU/+3DhYbRoY8P+ATqUEUHiqzscefiJXCvfIljoa2mmcOrY/48bH
gHpfQobAVYZdcuCy/24uW1tzFrbVWFxmZdItAoGAJXllmf/WFkzeZdZPcOdjqyQj
IlzXjWfN2xsk0TD13WCfriGP9dLb0aACyFsk3JIViCvoGLPWaq2kSWBADchsgJf9
tHy6rqf4xEM7GxUd/l3ygT1niAwipd5OEe12/MSsuUFW4PuEIq5I7ZwKMZlNjAnF
l/x3gGXWTx+4v9qsb2o=
-----END PRIVATE KEY-----
";

    #[test]
    fn rs256_signatures_verify_with_the_public_key() {
        use ring::signature::{KeyPair, RSA_PKCS1_2048_8192_SHA256, RsaKeyPair, UnparsedPublicKey};
        let jwt = sign_jwt_rs256(
            TEST_PRIVATE_KEY,
            &json!({"alg": "RS256"}),
            &json!({"iss": "a@b", "aud": "x"}),
        )
        .unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(
            URL_SAFE_NO_PAD.decode(parts[0]).unwrap(),
            br#"{"alg":"RS256"}"#
        );
        let (der, _) = pem_to_der(TEST_PRIVATE_KEY).unwrap();
        let key = RsaKeyPair::from_pkcs8(&der).unwrap();
        let public = UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, key.public_key().as_ref());
        let signed = format!("{}.{}", parts[0], parts[1]);
        public
            .verify(
                signed.as_bytes(),
                &URL_SAFE_NO_PAD.decode(parts[2]).unwrap(),
            )
            .unwrap();
        assert!(sign_jwt_rs256("not a key", &json!({}), &json!({})).is_err());
    }

    #[test]
    fn credential_files_parse_and_reject_unsupported_types() {
        let sa = json!({"type": "service_account", "client_email": "svc@p.iam.gserviceaccount.com",
            "private_key": TEST_PRIVATE_KEY, "quota_project_id": "billing"});
        assert!(matches!(
            parse_credentials(&sa.to_string()).unwrap(),
            CredentialsFile::ServiceAccount { quota_project: Some(q), .. } if q == "billing"
        ));
        let user = json!({"type": "authorized_user", "client_id": "c", "client_secret": "s",
            "refresh_token": "r"});
        assert!(matches!(
            parse_credentials(&user.to_string()).unwrap(),
            CredentialsFile::AuthorizedUser { .. }
        ));
        let ext = json!({"type": "external_account"});
        assert!(
            parse_credentials(&ext.to_string())
                .unwrap_err()
                .contains("external_account")
        );
    }
}
