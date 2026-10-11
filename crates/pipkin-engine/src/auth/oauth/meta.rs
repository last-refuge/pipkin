//! Meta Model API device flow (reference `auth/oauth/meta.ts`). The identity token (stored as
//! `refresh`) mints a day-long API key (stored as `access`); "refresh" re-mints the key and a
//! 401/403 means the session is dead.

use std::time::Duration;

use serde_json::{Map, Value};

use super::device::{Poll, PollOptions, poll_device};
use super::http::{post_form, send};
use super::{AuthEvent, CANCELLED, FlowCtx, Interaction, ModelAuth, OAuthCredential, OAuthFlow};
use crate::provider::BoxFuture;

const CLIENT_ID: &str = "1031625952748946";
const KEY_LIFETIME_MS: i64 = 24 * 60 * 60 * 1000;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Meta;

fn detail(json: &Value) -> String {
    for k in ["error_description", "detail", "message", "error"] {
        if let Some(s) = json.get(k).and_then(Value::as_str).map(str::trim)
            && !s.is_empty()
        {
            return format!(": {s}");
        }
    }
    String::new()
}

async fn mint(
    ctx: &FlowCtx,
    identity: &str,
    cancel: Option<&tokio_util::sync::CancellationToken>,
) -> Result<OAuthCredential, String> {
    let resp = send(
        ctx.http
            .post(format!("{}/muse-code/key", ctx.endpoints.meta_api))
            .header("Accept", "application/json")
            .header("Authorization", format!("Bearer {identity}"))
            .header("Content-Type", "application/json")
            .header("x-api-version", "1.0.0")
            .body("{}")
            .timeout(REQUEST_TIMEOUT),
        cancel,
    )
    .await?;
    let json = resp.object();
    if resp.status == 401 || resp.status == 403 {
        return Err(format!(
            "Meta session expired (status {}). Run `/login meta` to sign in again.{}",
            resp.status,
            detail(&json)
        ));
    }
    if !resp.ok() {
        return Err(format!(
            "Meta API key mint failed with status {}{}",
            resp.status,
            detail(&json)
        ));
    }
    match super::json_str(&json, "api_key").filter(|k| !k.is_empty()) {
        Some(key) => Ok(OAuthCredential {
            access: key.to_owned(),
            refresh: identity.to_owned(),
            expires: super::now_ms() + KEY_LIFETIME_MS,
            extra: Map::new(),
        }),
        None => Err(format!(
            "Meta did not issue an API key.{}",
            super::trusted_http_url(super::json_str(&json, "action_url"))
                .map(|u| format!(" Complete setup at {u}"))
                .unwrap_or_default()
        )),
    }
}

async fn login(ctx: &FlowCtx, io: &Interaction) -> Result<OAuthCredential, String> {
    let base = &ctx.endpoints.meta_auth;
    let resp = send(
        post_form(
            &ctx.http,
            &format!("{base}/oidc/device/authorization/"),
            &[("client_id", CLIENT_ID)],
        )
        .timeout(REQUEST_TIMEOUT),
        Some(&io.cancel),
    )
    .await?;
    let json = resp.object();
    if !resp.ok() {
        return Err(format!(
            "Meta device authorization failed with status {}{}",
            resp.status,
            detail(&json)
        ));
    }
    let verification = super::trusted_http_url(super::json_str(&json, "verification_uri_complete"))
        .or_else(|| super::trusted_http_url(super::json_str(&json, "verification_uri")));
    let (Some(device_code), Some(user_code), Some(verification)) = (
        super::json_str(&json, "device_code").filter(|s| !s.is_empty()),
        super::json_str(&json, "user_code").filter(|s| !s.is_empty()),
        verification,
    ) else {
        return Err("Invalid Meta device authorization response".into());
    };
    io.notify(AuthEvent::DeviceCode {
        user_code: user_code.to_owned(),
        verification_uri: verification,
    });
    let token_url = format!("{base}/oidc/device/token/");
    let identity = poll_device(
        PollOptions {
            interval_seconds: super::positive_number(&json, "interval"),
            expires_in_seconds: super::positive_number(&json, "expires_in"),
            wait_before_first_poll: true,
            fixed_interval: ctx.endpoints.poll_interval,
        },
        &io.cancel,
        || async {
            let resp = send(
                post_form(
                    &ctx.http,
                    &token_url,
                    &[
                        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                        ("device_code", device_code),
                        ("client_id", CLIENT_ID),
                    ],
                )
                .timeout(REQUEST_TIMEOUT),
                Some(&io.cancel),
            )
            .await?;
            let json = resp.object();
            if resp.ok()
                && let Some(t) = super::json_str(&json, "access_token").filter(|t| !t.is_empty())
            {
                return Ok(Poll::Complete(t.to_owned()));
            }
            Ok(match super::json_str(&json, "error") {
                Some("authorization_pending") => Poll::Pending,
                Some("slow_down") => Poll::SlowDown(super::positive_number(&json, "interval")),
                Some("access_denied") => Poll::Failed("Meta login was denied.".into()),
                Some("expired_token") => {
                    Poll::Failed("Meta device authorization expired. Please restart login.".into())
                }
                _ => Poll::Failed(format!(
                    "Meta device token request failed with status {}{}",
                    resp.status,
                    detail(&json)
                )),
            })
        },
    )
    .await;
    let identity = match identity {
        Err(_) if io.cancel.is_cancelled() => return Err(CANCELLED.into()),
        r => r?,
    };
    io.notify(AuthEvent::Progress(
        "Enabling Meta Model API access...".into(),
    ));
    mint(ctx, &identity, Some(&io.cancel)).await
}

impl OAuthFlow for Meta {
    fn name(&self) -> &str {
        "Meta (Muse subscription)"
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
        Box::pin(mint(ctx, &credential.refresh, None))
    }

    fn to_auth(&self, credential: &OAuthCredential) -> ModelAuth {
        ModelAuth {
            api_key: Some(credential.access.clone()),
            ..Default::default()
        }
    }
}
