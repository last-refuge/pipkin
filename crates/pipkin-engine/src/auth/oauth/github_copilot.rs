//! GitHub Copilot device flow (reference `auth/oauth/github-copilot.ts`).
//!
//! The stored credential keeps the GitHub access token as `refresh` and the short-lived Copilot
//! token as `access`; refreshing exchanges the GitHub token for a new Copilot token and
//! re-reads the account's model list (`availableModelIds`, used to filter the catalog). The
//! request base URL comes from the Copilot token's `proxy-ep`.

use std::time::Duration;

use base64::Engine;
use serde_json::{Map, Value};

use super::device::{Poll, PollOptions, poll_device};
use super::http::{Resp, form, send};
use super::{AuthEvent, FlowCtx, Interaction, ModelAuth, OAuthCredential, OAuthFlow, Prompt};
use crate::provider::BoxFuture;

const CLIENT_ID_B64: &str = "SXYxLmI1MDdhMDhjODdlY2ZlOTg=";
const COPILOT_HEADERS: [(&str, &str); 4] = [
    ("User-Agent", "GitHubCopilotChat/0.35.0"),
    ("Editor-Version", "vscode/1.107.0"),
    ("Editor-Plugin-Version", "copilot-chat/0.35.0"),
    ("Copilot-Integration-Id", "vscode-chat"),
];
const COPILOT_API_VERSION: &str = "2026-06-01";
const INDIVIDUAL_BASE: &str = "https://api.individual.githubcopilot.com";

pub struct GitHubCopilot;

fn client_id() -> String {
    base64::engine::general_purpose::STANDARD
        .decode(CLIENT_ID_B64)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_default()
}

fn normalize_domain(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    };
    url::Url::parse(&candidate)
        .ok()?
        .host_str()
        .map(str::to_owned)
}

fn github_base(ctx: &FlowCtx, domain: &str) -> String {
    ctx.endpoints
        .github
        .clone()
        .unwrap_or_else(|| format!("https://{domain}"))
}

fn github_api_base(ctx: &FlowCtx, domain: &str) -> String {
    ctx.endpoints
        .github_api
        .clone()
        .unwrap_or_else(|| format!("https://api.{domain}"))
}

/// `proxy-ep=proxy.X` in the token -> `https://api.X`.
fn base_url_from_token(token: &str) -> Option<String> {
    let start = token.find("proxy-ep=")? + "proxy-ep=".len();
    let host = token[start..].split(';').next()?;
    if host.is_empty() {
        return None;
    }
    let host = host
        .strip_prefix("proxy.")
        .map_or(host.to_owned(), |h| format!("api.{h}"));
    Some(format!("https://{host}"))
}

pub(crate) fn copilot_base_url(token: Option<&str>, enterprise: Option<&str>) -> String {
    token
        .and_then(base_url_from_token)
        .or_else(|| enterprise.map(|d| format!("https://copilot-api.{d}")))
        .unwrap_or_else(|| INDIVIDUAL_BASE.into())
}

fn enterprise_domain(credential: &OAuthCredential) -> Option<String> {
    credential
        .extra
        .get("enterpriseUrl")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .and_then(normalize_domain)
}

pub(crate) fn available_model_ids(credential: &OAuthCredential) -> Option<Vec<String>> {
    let ids = credential.extra.get("availableModelIds")?.as_array()?;
    ids.iter()
        .map(|v| v.as_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()
}

fn with_copilot_headers(mut rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    for (k, v) in COPILOT_HEADERS {
        rb = rb.header(k, v);
    }
    rb
}

fn http_error(resp: &Resp) -> String {
    let reason = reqwest::StatusCode::from_u16(resp.status)
        .ok()
        .and_then(|s| s.canonical_reason())
        .unwrap_or("");
    format!("{} {reason}: {}", resp.status, resp.text)
}

struct Catalog {
    available: Vec<String>,
    policy: Vec<String>,
}

fn parse_catalog(
    raw: &Value,
    allow_policy_fallback: bool,
    known: &[String],
) -> Result<Catalog, String> {
    let data = raw
        .get("data")
        .and_then(Value::as_array)
        .ok_or("Invalid Copilot models response")?;
    struct Entry {
        id: String,
        picker: bool,
        policy: Option<String>,
    }
    let models: Vec<Entry> = data
        .iter()
        .filter_map(|item| {
            let id = item.get("id")?.as_str()?.to_owned();
            if item
                .pointer("/capabilities/supports/tool_calls")
                .and_then(Value::as_bool)
                == Some(false)
            {
                return None;
            }
            Some(Entry {
                id,
                picker: item.get("model_picker_enabled").and_then(Value::as_bool) == Some(true),
                policy: item
                    .pointer("/policy/state")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
        })
        .collect();
    let picker: Vec<String> = models
        .iter()
        .filter(|m| m.picker && m.policy.as_deref() != Some("disabled"))
        .map(|m| m.id.clone())
        .collect();
    let use_fallback = allow_policy_fallback && picker.is_empty();
    let available = if !picker.is_empty() || !allow_policy_fallback {
        picker
    } else {
        models
            .iter()
            .filter(|m| m.policy.as_deref() == Some("enabled"))
            .map(|m| m.id.clone())
            .collect()
    };
    let policy = models
        .iter()
        .filter(|m| {
            m.policy.as_deref() == Some("unconfigured")
                && known.contains(&m.id)
                && (m.picker || use_fallback)
        })
        .map(|m| m.id.clone())
        .collect();
    Ok(Catalog { available, policy })
}

/// Reference `fetchWithRateLimitRetry`: 5 s per attempt, retry 429 with `retry-after` or
/// `500 * 2^n` ms while within the budget.
async fn send_with_rate_limit_retry(
    ctx: &FlowCtx,
    build: impl Fn() -> reqwest::RequestBuilder,
    max_retries: u32,
    budget: Duration,
    cancel: Option<&tokio_util::sync::CancellationToken>,
) -> Result<Resp, String> {
    let deadline = tokio::time::Instant::now() + budget;
    let mut retry = 0;
    loop {
        let resp = send(build().timeout(Duration::from_secs(5)), cancel).await?;
        if resp.status != 429 || retry >= max_retries {
            return Ok(resp);
        }
        let mut delay = ctx
            .endpoints
            .retry_delay(Duration::from_millis(500 * 2u64.pow(retry)), 1 << retry);
        if let Some(ra) = resp.header("retry-after") {
            match ra.trim().parse::<f64>() {
                Ok(secs) => delay = Duration::from_secs_f64(secs.max(0.0)),
                Err(_) => return Ok(resp),
            }
        }
        if max_retries == 0 || tokio::time::Instant::now() + delay >= deadline {
            return Ok(resp);
        }
        tokio::time::sleep(delay).await;
        retry += 1;
    }
}

async fn fetch_models(
    ctx: &FlowCtx,
    copilot_token: &str,
    enterprise: Option<&str>,
    retries: u32,
    cancel: Option<&tokio_util::sync::CancellationToken>,
) -> Result<Catalog, String> {
    let real_base = copilot_base_url(Some(copilot_token), enterprise);
    let allow_fallback = real_base == INDIVIDUAL_BASE;
    let base = ctx.endpoints.copilot_api.clone().unwrap_or(real_base);
    let url = format!("{base}/models");
    let resp = send_with_rate_limit_retry(
        ctx,
        || {
            with_copilot_headers(
                ctx.http
                    .get(&url)
                    .header("Accept", "application/json")
                    .header("Authorization", format!("Bearer {copilot_token}")),
            )
            .header("X-GitHub-Api-Version", COPILOT_API_VERSION)
        },
        retries,
        Duration::from_secs(5),
        cancel,
    )
    .await?;
    if !resp.ok() {
        return Err(http_error(&resp));
    }
    parse_catalog(&resp.json(), allow_fallback, &ctx.copilot_model_ids)
}

async fn copilot_token(
    ctx: &FlowCtx,
    github_token: &str,
    enterprise: Option<&str>,
    cancel: Option<&tokio_util::sync::CancellationToken>,
) -> Result<OAuthCredential, String> {
    let domain = enterprise.unwrap_or("github.com");
    let url = format!("{}/copilot_internal/v2/token", github_api_base(ctx, domain));
    let resp = send(
        with_copilot_headers(
            ctx.http
                .get(&url)
                .header("Accept", "application/json")
                .header("Authorization", format!("Bearer {github_token}")),
        ),
        cancel,
    )
    .await?;
    if !resp.ok() {
        return Err(http_error(&resp));
    }
    let json = resp.json();
    let (Some(token), Some(expires_at)) = (
        super::json_str(&json, "token"),
        json.get("expires_at").and_then(Value::as_f64),
    ) else {
        return Err("Invalid Copilot token response fields".into());
    };
    let mut extra = Map::new();
    if let Some(e) = enterprise {
        extra.insert("enterpriseUrl".into(), Value::String(e.to_owned()));
    }
    Ok(OAuthCredential {
        access: token.to_owned(),
        refresh: github_token.to_owned(),
        expires: (expires_at * 1000.0) as i64 - 5 * 60 * 1000,
        extra,
    })
}

async fn enable_models(
    ctx: &FlowCtx,
    token: &str,
    ids: &[String],
    enterprise: Option<&str>,
    io: &Interaction,
) -> Result<Vec<String>, String> {
    let base = ctx
        .endpoints
        .copilot_api
        .clone()
        .unwrap_or_else(|| copilot_base_url(Some(token), enterprise));
    let mut enabled = Vec::new();
    for id in ids {
        let url = format!("{base}/models/{id}/policy");
        let resp = send_with_rate_limit_retry(
            ctx,
            || {
                with_copilot_headers(
                    ctx.http
                        .post(&url)
                        .header("Content-Type", "application/json")
                        .header("Authorization", format!("Bearer {token}")),
                )
                .header("openai-intent", "chat-policy")
                .header("x-interaction-type", "chat-policy")
                .body(r#"{"state":"enabled"}"#)
            },
            2,
            Duration::from_secs(5),
            Some(&io.cancel),
        )
        .await;
        match resp {
            Err(e) if io.cancel.is_cancelled() => return Err(e),
            Err(_) => continue,
            // Exhausted rate limiting stops the batch (best effort).
            Ok(r) if r.status == 429 => break,
            Ok(r) if r.ok() => enabled.push(id.clone()),
            Ok(_) => {}
        }
    }
    Ok(enabled)
}

async fn login(ctx: &FlowCtx, io: &Interaction) -> Result<OAuthCredential, String> {
    let input = io
        .prompt(Prompt::text(
            "GitHub Enterprise URL/domain (blank for github.com)",
            Some("company.ghe.com"),
        ))
        .await?;
    io.check()?;
    let enterprise = normalize_domain(&input);
    if !input.trim().is_empty() && enterprise.is_none() {
        return Err("Invalid GitHub Enterprise URL/domain".into());
    }
    let domain = enterprise.clone().unwrap_or_else(|| "github.com".into());
    let base = github_base(ctx, &domain);
    let cid = client_id();
    let resp = send(
        ctx.http
            .post(format!("{base}/login/device/code"))
            .header("Accept", "application/json")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("User-Agent", "GitHubCopilotChat/0.35.0")
            .body(form(&[("client_id", &cid), ("scope", "read:user")])),
        Some(&io.cancel),
    )
    .await?;
    if !resp.ok() {
        return Err(http_error(&resp));
    }
    let json = resp.json();
    let device_code = super::json_str(&json, "device_code");
    let user_code = super::json_str(&json, "user_code");
    let verification = super::json_str(&json, "verification_uri");
    let interval = json.get("interval");
    let expires_in = json.get("expires_in").and_then(Value::as_f64);
    let (Some(device_code), Some(user_code), Some(verification), Some(expires_in)) =
        (device_code, user_code, verification, expires_in)
    else {
        return Err("Invalid device code response fields".into());
    };
    if interval.is_some_and(|i| !i.is_number()) {
        return Err("Invalid device code response fields".into());
    }
    let verification = super::trusted_http_url(Some(verification))
        .ok_or("Untrusted verification_uri in device code response")?;
    io.notify(AuthEvent::DeviceCode {
        user_code: user_code.to_owned(),
        verification_uri: verification,
    });
    let token_url = format!("{base}/login/oauth/access_token");
    let github_token = poll_device(
        PollOptions {
            interval_seconds: interval.and_then(Value::as_f64),
            expires_in_seconds: Some(expires_in),
            wait_before_first_poll: true,
            fixed_interval: ctx.endpoints.poll_interval,
        },
        &io.cancel,
        || async {
            let resp = send(
                ctx.http
                    .post(&token_url)
                    .header("Accept", "application/json")
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .header("User-Agent", "GitHubCopilotChat/0.35.0")
                    .body(form(&[
                        ("client_id", &cid),
                        ("device_code", device_code),
                        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ])),
                Some(&io.cancel),
            )
            .await?;
            if !resp.ok() {
                return Err(http_error(&resp));
            }
            let raw = resp.json();
            if let Some(t) = super::json_str(&raw, "access_token") {
                return Ok(Poll::Complete(t.to_owned()));
            }
            match super::json_str(&raw, "error") {
                Some("authorization_pending") => Ok(Poll::Pending),
                Some("slow_down") => {
                    Ok(Poll::SlowDown(raw.get("interval").and_then(Value::as_f64)))
                }
                Some(e) => Ok(Poll::Failed(format!(
                    "Device flow failed: {e}{}",
                    super::json_str(&raw, "error_description")
                        .map(|d| format!(": {d}"))
                        .unwrap_or_default()
                ))),
                None => Ok(Poll::Failed("Invalid device token response".into())),
            }
        },
    )
    .await?;
    let mut credential =
        copilot_token(ctx, &github_token, enterprise.as_deref(), Some(&io.cancel)).await?;
    let catalog = fetch_models(
        ctx,
        &credential.access,
        enterprise.as_deref(),
        2,
        Some(&io.cancel),
    )
    .await?;
    let mut enabled = Vec::new();
    if !catalog.policy.is_empty() {
        io.notify(AuthEvent::Progress("Enabling models...".into()));
        enabled = enable_models(
            ctx,
            &credential.access,
            &catalog.policy,
            enterprise.as_deref(),
            io,
        )
        .await?;
    }
    let mut ids = catalog.available;
    for id in enabled {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    credential.extra.insert(
        "availableModelIds".into(),
        Value::Array(ids.into_iter().map(Value::String).collect()),
    );
    Ok(credential)
}

impl OAuthFlow for GitHubCopilot {
    fn name(&self) -> &str {
        "GitHub Copilot"
    }

    fn login<'a>(
        &'a self,
        ctx: &'a FlowCtx,
        io: &'a Interaction,
    ) -> BoxFuture<'a, Result<OAuthCredential, String>> {
        Box::pin(login(ctx, io))
    }

    fn refresh<'a>(
        &'a self,
        ctx: &'a FlowCtx,
        credential: &'a OAuthCredential,
    ) -> BoxFuture<'a, Result<OAuthCredential, String>> {
        Box::pin(async move {
            let enterprise = enterprise_domain(credential);
            let mut next =
                copilot_token(ctx, &credential.refresh, enterprise.as_deref(), None).await?;
            let catalog = fetch_models(ctx, &next.access, enterprise.as_deref(), 0, None).await?;
            next.extra.insert(
                "availableModelIds".into(),
                Value::Array(catalog.available.into_iter().map(Value::String).collect()),
            );
            Ok(next)
        })
    }

    fn to_auth(&self, credential: &OAuthCredential) -> ModelAuth {
        ModelAuth {
            api_key: Some(credential.access.clone()),
            headers: Default::default(),
            base_url: Some(copilot_base_url(
                Some(&credential.access),
                enterprise_domain(credential).as_deref(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_and_domains() {
        assert_eq!(
            copilot_base_url(
                Some("tid=1;exp=2;proxy-ep=proxy.business.githubcopilot.com;x=y"),
                None
            ),
            "https://api.business.githubcopilot.com"
        );
        assert_eq!(
            copilot_base_url(Some("no-proxy"), Some("corp.ghe.com")),
            "https://copilot-api.corp.ghe.com"
        );
        assert_eq!(copilot_base_url(None, None), INDIVIDUAL_BASE);
        assert_eq!(
            normalize_domain(" https://corp.ghe.com/x ").as_deref(),
            Some("corp.ghe.com")
        );
        assert_eq!(
            normalize_domain("corp.ghe.com").as_deref(),
            Some("corp.ghe.com")
        );
        assert_eq!(normalize_domain(""), None);
        assert_eq!(client_id().len(), 20);
    }

    #[test]
    fn model_catalog_rules() {
        let raw = serde_json::json!({"data": [
            {"id": "a", "model_picker_enabled": true},
            {"id": "b", "model_picker_enabled": true, "policy": {"state": "disabled"}},
            {"id": "c", "model_picker_enabled": true, "policy": {"state": "unconfigured"}},
            {"id": "d", "model_picker_enabled": true, "capabilities": {"supports": {"tool_calls": false}}},
            {"id": "e", "policy": {"state": "enabled"}}
        ]});
        let known = vec!["c".to_string()];
        let c = parse_catalog(&raw, true, &known).unwrap();
        assert_eq!(c.available, vec!["a", "c"]);
        assert_eq!(c.policy, vec!["c"]);
        let raw = serde_json::json!({"data": [{"id": "e", "policy": {"state": "enabled"}}]});
        assert_eq!(
            parse_catalog(&raw, true, &known).unwrap().available,
            vec!["e"]
        );
        assert!(
            parse_catalog(&raw, false, &known)
                .unwrap()
                .available
                .is_empty()
        );
        assert!(parse_catalog(&serde_json::json!({}), true, &known).is_err());
    }
}
