//! File/URL external identities and external authorized-user credentials → Google STS.
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[path = "google_aws.rs"]
mod aws;
#[path = "google_certificate.rs"]
mod certificate;
#[path = "google_executable.rs"]
mod executable;
use std::time::Duration;
use tokio::io::AsyncReadExt;

use super::{BUDGET, CLOUD_PLATFORM_SCOPE, LIMIT, State, Token, response, response_bytes};
use crate::provider::{ProviderRequest, http};

pub(super) fn execution_context(
    req: &ProviderRequest,
    document: Option<&Value>,
) -> (BTreeMap<String, String>, Duration) {
    let mut doc = document;
    while let Some(value) = doc.filter(|d| d["type"] == "impersonated_service_account") {
        doc = value.get("source_credentials");
    }
    if doc.is_some_and(is_aws) {
        return (aws::environment(req), BUDGET);
    }
    if doc.is_some_and(|d| {
        !is_aws(d)
            && !is_executable(d)
            && !option(option(d, "credential_source"), "certificate").is_null()
    }) {
        return (certificate::environment(req), BUDGET);
    }
    if doc.is_some_and(is_executable) {
        executable::execution_context(req, document)
    } else {
        (BTreeMap::new(), BUDGET)
    }
}

// ADC factory dispatch uses these original keys; aliases apply inside the selected client.
fn selected(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::String(v) => !v.is_empty(),
        Value::Number(v) => v.as_f64().is_some_and(|v| v != 0.0),
        _ => true,
    }
}
fn is_aws(doc: &Value) -> bool {
    selected(&doc["credential_source"]["environment_id"])
}
fn is_executable(doc: &Value) -> bool {
    selected(&doc["credential_source"]["executable"])
}

/// The SDK accepts original snake_case keys first, with camelCase aliases.
pub(super) fn option<'a>(v: &'a Value, key: &str) -> &'a Value {
    if let Some(value) = v.get(key).filter(|v| !v.is_null()) {
        return value;
    }
    let mut camel = String::new();
    let mut upper = false;
    for c in key.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            camel.extend(c.to_uppercase());
            upper = false;
        } else {
            camel.push(c);
        }
    }
    v.get(camel).unwrap_or(&Value::Null)
}
pub(super) fn string(v: &Value, key: &str) -> Option<String> {
    option(v, key)
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
fn required(v: &Value, key: &str) -> Result<String, String> {
    string(v, key).ok_or_else(|| format!("Google external credentials missing {key}"))
}
fn format(source: &Value) -> Result<(&str, Option<String>), String> {
    let format = option(source, "format");
    let kind = option(format, "type")
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or("text");
    match kind {
        "text" => Ok((kind, None)),
        "json" => Ok((kind, Some(required(format, "subject_token_field_name")?))),
        _ => Err("Invalid Google external credential_source format".into()),
    }
}
pub(super) fn validate(doc: &Value) -> Result<(), String> {
    if doc["type"] == "external_account_authorized_user" {
        required(doc, "client_id")?;
        required(doc, "refresh_token")?;
        return Ok(());
    }
    let audience = required(doc, "audience")?;
    required(doc, "subject_token_type")?;
    if string(doc, "workforce_pool_user_project").is_some() {
        static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        if !PATTERN
            .get_or_init(|| {
                regex::Regex::new(
                    r"//iam\.googleapis\.com/locations/[^/]+/workforcePools/[^/]+/providers/.+",
                )
                .unwrap()
            })
            .is_match(&audience)
        {
            return Err("workforce_pool_user_project requires a Google workforce audience".into());
        }
    }
    let source = option(doc, "credential_source");
    if is_aws(doc) {
        return aws::validate(source);
    }
    if is_executable(doc) {
        return executable::validate(&doc["credential_source"]["executable"]);
    }
    if !option(source, "certificate").is_null() {
        if string(source, "file").is_some() || string(source, "url").is_some() {
            return Err("Google certificate source cannot also specify a file or URL".into());
        }
        format(source)?;
        return certificate::validate(option(source, "certificate"));
    }
    if string(source, "file").is_some() == string(source, "url").is_some() {
        return Err("Google credential_source needs exactly one file or URL".into());
    }
    format(source)?;
    Ok(())
}
fn endpoint(doc: &Value, user: bool) -> Result<String, String> {
    let endpoint = string(doc, "token_url").unwrap_or_else(|| {
        format!(
            "https://sts.{}/v1/{}",
            super::universe(doc),
            if user { "oauthtoken" } else { "token" }
        )
    });
    checked_url(&endpoint)?;
    Ok(endpoint)
}
fn checked_url(value: &str) -> Result<(), String> {
    let url = url::Url::parse(value).map_err(|_| "Invalid Google external credential URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(
            "Google external credential URL must be HTTP(S) without embedded credentials".into(),
        );
    }
    Ok(())
}
async fn subject(http: &reqwest::Client, source: &Value) -> Result<String, String> {
    let bytes = if let Some(file) = string(source, "file") {
        let file = tokio::fs::File::open(file)
            .await
            .map_err(|_| "Cannot read Google external subject-token file")?;
        if !file
            .metadata()
            .await
            .map_err(|_| "Cannot inspect Google external subject-token file")?
            .is_file()
        {
            return Err("Google external subject token is not a regular file".into());
        }
        let mut bytes = Vec::new();
        file.take(LIMIT as u64 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| "Cannot read Google external subject token")?;
        if bytes.len() > LIMIT {
            return Err("Google external subject token is too large".into());
        }
        bytes
    } else {
        let url = required(source, "url")?;
        checked_url(&url)?;
        let mut request = http.get(url).timeout(BUDGET);
        if let Some(headers) = option(source, "headers").as_object() {
            for (k, v) in headers {
                request = request.header(
                    k,
                    v.as_str()
                        .ok_or("Google subject-token headers must be strings")?,
                );
            }
        }
        response_bytes(
            request
                .send()
                .await
                .map_err(|e| http::transport_error(&e))?,
        )
        .await?
    };
    let (kind, field) = format(source)?;
    let token = if kind == "text" {
        String::from_utf8(bytes).map_err(|_| "Google subject token is not UTF-8")?
    } else {
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| "Google subject token is not valid JSON")?;
        value
            .get(field.as_deref().unwrap())
            .and_then(Value::as_str)
            .ok_or("Google subject-token JSON has no configured string field")?
            .to_owned()
    };
    if token.is_empty() {
        return Err("Google external subject token is empty".into());
    }
    Ok(token)
}
fn access(value: &Value, expiry_required: bool) -> Result<Token, String> {
    let token = super::field(value, "access_token")?;
    let expiry = match value.get("expires_in") {
        None if !expiry_required => None,
        Some(Value::Null) if !expiry_required => None,
        Some(v) if v.as_u64() == Some(0) && !expiry_required => None,
        Some(v) => Some(Duration::from_secs(
            v.as_u64()
                .filter(|s| *s > 0)
                .ok_or("Invalid Google STS token lifetime")?,
        )),
        _ => return Err("Google STS response has no token lifetime".into()),
    };
    if let Some(lifetime) = expiry {
        std::time::Instant::now()
            .checked_add(lifetime)
            .ok_or("Excessive Google STS token lifetime")?;
    }
    Ok((token, expiry))
}
fn authenticated(builder: reqwest::RequestBuilder, doc: &Value) -> reqwest::RequestBuilder {
    if let Some(client) = string(doc, "client_id") {
        builder.basic_auth(
            client,
            Some(string(doc, "client_secret").unwrap_or_default()),
        )
    } else {
        builder
    }
}

pub(super) async fn exchange(
    http: &reqwest::Client,
    doc: &Value,
    state: &mut State,
    environment: &BTreeMap<String, String>,
    net: &http::NetSettings,
) -> Result<Token, String> {
    if doc["type"] == "external_account_authorized_user" {
        let key = crate::provider::api::aws_auth::sha256_hex(doc.to_string().as_bytes());
        let refresh = state
            .refresh_tokens
            .get(&key)
            .cloned()
            .unwrap_or(required(doc, "refresh_token")?);
        let request = authenticated(http.post(endpoint(doc, true)?), doc)
            .form(&[("grant_type", "refresh_token"), ("refresh_token", &refresh)])
            .timeout(BUDGET);
        let body = response(
            request
                .send()
                .await
                .map_err(|e| http::transport_error(&e))?,
        )
        .await?;
        let token = access(&body, true)?;
        if let Some(refresh) = super::super::field(&body, "refresh_token") {
            state.refresh_tokens.insert(key, refresh);
        }
        return Ok(token);
    }
    let source = option(doc, "credential_source");
    let certificate =
        if !is_aws(doc) && !is_executable(doc) && !option(source, "certificate").is_null() {
            Some(certificate::prepare(option(source, "certificate"), environment, net).await?)
        } else {
            None
        };
    state.auth_http = certificate.as_ref().map(|c| c.http.clone());
    let http = certificate.as_ref().map(|c| &c.http).unwrap_or(http);
    let subject = if is_aws(doc) {
        aws::subject(http, doc, state, environment).await?
    } else if is_executable(doc) {
        executable::subject(doc, environment).await?
    } else if let Some(certificate) = &certificate {
        certificate.subject.clone()
    } else {
        subject(http, source).await?
    };
    let mut fields = vec![
        (
            "grant_type",
            "urn:ietf:params:oauth:grant-type:token-exchange".to_owned(),
        ),
        ("audience", required(doc, "audience")?),
        ("scope", CLOUD_PLATFORM_SCOPE.to_owned()),
        (
            "requested_token_type",
            "urn:ietf:params:oauth:token-type:access_token".to_owned(),
        ),
        ("subject_token", subject),
        ("subject_token_type", required(doc, "subject_token_type")?),
    ];
    if string(doc, "client_id").is_none()
        && let Some(project) = string(doc, "workforce_pool_user_project")
    {
        fields.push(("options", json!({"userProject":project}).to_string()));
    }
    let request = authenticated(http.post(endpoint(doc, false)?), doc)
        .form(&fields)
        .timeout(BUDGET);
    let sts = response(
        request
            .send()
            .await
            .map_err(|e| http::transport_error(&e))?,
    )
    .await?;
    let token = access(&sts, false)?;
    let Some(impersonation) = string(doc, "service_account_impersonation_url") else {
        return Ok(token);
    };
    checked_url(&impersonation)?;
    let lifetime = option(
        option(doc, "service_account_impersonation"),
        "token_lifetime_seconds",
    )
    .as_u64()
    .filter(|s| *s > 0)
    .unwrap_or(3600);
    let body = json!({"scope":[CLOUD_PLATFORM_SCOPE],"lifetime":format!("{lifetime}s")});
    let result = response(
        http.post(impersonation)
            .bearer_auth(token.0)
            .json(&body)
            .timeout(BUDGET)
            .send()
            .await
            .map_err(|e| http::transport_error(&e))?,
    )
    .await?;
    let token = super::field(&result, "accessToken")?;
    let end = chrono::DateTime::parse_from_rfc3339(&super::field(&result, "expireTime")?)
        .map_err(|_| "Invalid Google external impersonation expiry")?
        .timestamp();
    let seconds = u64::try_from(end)
        .ok()
        .and_then(|s| s.checked_sub(super::super::unix_now()))
        .filter(|s| *s > 0)
        .ok_or("Google external impersonation token is expired")?;
    Ok((token, Some(Duration::from_secs(seconds))))
}

#[cfg(test)]
#[path = "google_external_tests.rs"]
mod tests;
