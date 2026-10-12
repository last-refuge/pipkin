//! Google external-account AWS suppliers and serialized GetCallerIdentity proofs.
use super::{BUDGET, State, checked_url, option, required, response, response_bytes, string};
use crate::provider::api::aws_auth::{AwsCredentials, sha256_hex, signing_key};
use crate::provider::{ProviderRequest, http};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use std::collections::BTreeMap;

pub(super) fn environment(req: &ProviderRequest) -> BTreeMap<String, String> {
    [
        "AWS_REGION",
        "AWS_DEFAULT_REGION",
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
    ]
    .into_iter()
    .filter_map(|k| req.env_var(k).map(|v| (k.into(), v)))
    .collect()
}
pub(super) fn validate(source: &Value) -> Result<(), String> {
    let id = required(source, "environment_id")?;
    let version = id
        .strip_prefix("aws")
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()));
    if version.and_then(|s| s.parse::<u32>().ok()) != Some(1) {
        return Err("Unsupported Google AWS credential_source environment_id".into());
    }
    required(source, "regional_cred_verification_url")?;
    Ok(())
}
async fn text(request: reqwest::RequestBuilder) -> Result<String, String> {
    let bytes = response_bytes(
        request
            .timeout(BUDGET)
            .send()
            .await
            .map_err(|e| http::transport_error(&e))?,
    )
    .await?;
    String::from_utf8(bytes).map_err(|_| "Google AWS metadata is not UTF-8".into())
}
async fn metadata_token(http: &reqwest::Client, source: &Value) -> Result<Option<String>, String> {
    let Some(url) = string(source, "imdsv2_session_token_url") else {
        return Ok(None);
    };
    checked_url(&url)?;
    let token = text(
        http.put(url)
            .header("x-aws-ec2-metadata-token-ttl-seconds", "300"),
    )
    .await?;
    if token.is_empty() {
        return Err("Google AWS metadata session token is empty".into());
    }
    Ok(Some(token))
}
fn metadata_get(
    http: &reqwest::Client,
    url: &str,
    token: Option<&str>,
) -> Result<reqwest::RequestBuilder, String> {
    checked_url(url)?;
    let mut r = http.get(url).timeout(BUDGET);
    if let Some(token) = token {
        r = r.header("x-aws-ec2-metadata-token", token);
    }
    Ok(r)
}
async fn region(
    http: &reqwest::Client,
    source: &Value,
    env: &BTreeMap<String, String>,
) -> Result<String, String> {
    if let Some(region) = env
        .get("AWS_REGION")
        .or_else(|| env.get("AWS_DEFAULT_REGION"))
    {
        return Ok(region.clone());
    }
    let token = metadata_token(http, source).await?;
    let mut zone = text(metadata_get(
        http,
        &required(source, "region_url")?,
        token.as_deref(),
    )?)
    .await?;
    // The SDK removes the availability-zone suffix without trimming the response.
    zone.pop();
    if zone.is_empty() {
        return Err("Google AWS metadata region is empty".into());
    }
    Ok(zone)
}
async fn credentials(
    http: &reqwest::Client,
    source: &Value,
    env: &BTreeMap<String, String>,
) -> Result<AwsCredentials, String> {
    if let (Some(id), Some(secret)) = (
        env.get("AWS_ACCESS_KEY_ID"),
        env.get("AWS_SECRET_ACCESS_KEY"),
    ) {
        return Ok(AwsCredentials::new(
            id,
            secret,
            env.get("AWS_SESSION_TOKEN").map(String::as_str),
        ));
    }
    let token = metadata_token(http, source).await?;
    let url = required(source, "url")?;
    let role = text(metadata_get(http, &url, token.as_deref())?).await?;
    if role.is_empty() {
        return Err("Google AWS metadata role is empty".into());
    }
    let result = response(
        metadata_get(http, &format!("{url}/{role}"), token.as_deref())?
            .send()
            .await
            .map_err(|e| http::transport_error(&e))?,
    )
    .await?;
    Ok(AwsCredentials::new(
        &required(&result, "AccessKeyId")?,
        &required(&result, "SecretAccessKey")?,
        string(&result, "Token").as_deref(),
    ))
}

fn encode_component(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn signed_subject(
    url: &str,
    region: &str,
    audience: &str,
    creds: &AwsCredentials,
    time: chrono::DateTime<chrono::Utc>,
) -> Result<String, String> {
    checked_url(url)?;
    let parsed = url::Url::parse(url).map_err(|_| "Invalid Google AWS verification URL")?;
    let host = &parsed[url::Position::BeforeHost..url::Position::AfterPort];
    let service = host
        .split('.')
        .next()
        .filter(|s| !s.is_empty())
        .ok_or("Google AWS verification URL has no host")?;
    let date = time.format("%Y%m%d").to_string();
    let stamp = time.format("%Y%m%dT%H%M%SZ").to_string();
    let mut headers = BTreeMap::from([("host", host.to_owned()), ("x-amz-date", stamp.clone())]);
    if let Some(token) = &creds.session_token {
        headers.insert(
            "x-amz-security-token",
            token.trim_matches([' ', '\t', '\r', '\n']).into(),
        );
    }
    let keys = headers.keys().copied().collect::<Vec<_>>().join(";");
    let canonical_headers = headers
        .iter()
        .map(|(k, v)| format!("{k}:{v}\n"))
        .collect::<String>();
    // Google's signer uses the URL's existing path/query order, unlike the Bedrock signer.
    let canonical = format!(
        "POST\n{}\n{}\n{canonical_headers}\n{keys}\n{}",
        parsed.path(),
        parsed.query().unwrap_or(""),
        sha256_hex(b"")
    );
    let scope = format!("{date}/{region}/{service}/aws4_request");
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{stamp}\n{scope}\n{}",
        sha256_hex(canonical.as_bytes())
    );
    let key = signing_key(&creds.secret_access_key, &date, region, service);
    let mut mac = Hmac::<Sha256>::new_from_slice(&key).expect("HMAC accepts any key length");
    mac.update(to_sign.as_bytes());
    let signature = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    headers.insert(
        "authorization",
        format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={keys}, Signature={signature}",
            creds.access_key_id
        ),
    );
    // The pinned Google client adds its audience after signing; retain that wire contract.
    headers.insert(
        "x-goog-cloud-target-resource",
        audience.trim_matches([' ', '\t', '\r', '\n']).into(),
    );
    let headers = headers
        .into_iter()
        .map(|(key, value)| json!({"key":key,"value":value}))
        .collect::<Vec<_>>();
    Ok(encode_component(
        &json!({"url":url,"method":"POST","headers":headers}).to_string(),
    ))
}

pub(super) async fn subject(
    http: &reqwest::Client,
    doc: &Value,
    state: &mut State,
    env: &BTreeMap<String, String>,
) -> Result<String, String> {
    let source = option(doc, "credential_source");
    let identity = sha256_hex(doc.to_string().as_bytes());
    let region = if let Some(region) = state.aws_regions.get(&identity) {
        region.clone()
    } else {
        let region = region(http, source, env).await?;
        state.aws_regions.insert(identity, region.clone());
        region
    };
    let creds = credentials(http, source, env).await?;
    let url = required(source, "regional_cred_verification_url")?.replacen("{region}", &region, 1);
    signed_subject(
        &url,
        &region,
        &required(doc, "audience")?,
        &creds,
        std::time::SystemTime::now().into(),
    )
}

#[cfg(test)]
#[path = "google_aws_tests.rs"]
mod tests;
