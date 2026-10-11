//! Kimi Code device flow (reference `auth/oauth/kimi-coding.ts`). The access token is sent as
//! an `Authorization: Bearer` header, not as an API key.

use std::time::Duration;

use serde_json::{Map, Value};

use super::device::{Poll, PollOptions, poll_device};
use super::http::{post_form, send};
use super::{AuthEvent, FlowCtx, Interaction, ModelAuth, OAuthCredential, OAuthFlow};
use crate::provider::BoxFuture;

const CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";
const DEVICE_TIMEOUT_SECONDS: f64 = 15.0 * 60.0;
const DEFAULT_INTERVAL_SECONDS: f64 = 5.0;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const REFRESH_MAX_RETRIES: u32 = 3;

pub struct KimiCoding;

fn parse_token(json: &Value, operation: &str) -> Result<OAuthCredential, String> {
    let access = super::json_str(json, "access_token").filter(|s| !s.is_empty());
    let refresh = super::json_str(json, "refresh_token").filter(|s| !s.is_empty());
    let expires_in = super::positive_number(json, "expires_in");
    match (access, refresh, expires_in) {
        (Some(a), Some(r), Some(e)) => Ok(OAuthCredential {
            access: a.to_owned(),
            refresh: r.to_owned(),
            expires: super::now_ms() + (e * 1000.0) as i64,
            extra: Map::new(),
        }),
        _ => Err(format!(
            "Kimi Code token {operation} response missing fields"
        )),
    }
}

fn suffix(text: &str) -> String {
    if text.is_empty() {
        String::new()
    } else {
        format!(": {text}")
    }
}

async fn login(ctx: &FlowCtx, io: &Interaction) -> Result<OAuthCredential, String> {
    let host = &ctx.endpoints.kimi;
    let resp = send(
        post_form(
            &ctx.http,
            &format!("{host}/api/oauth/device_authorization"),
            &[("client_id", CLIENT_ID)],
        )
        .timeout(REQUEST_TIMEOUT),
        Some(&io.cancel),
    )
    .await?;
    if !resp.ok() {
        return Err(format!(
            "Kimi Code device authorization failed with status {}{}",
            resp.status,
            suffix(&resp.text)
        ));
    }
    let json = resp.json();
    let device_code = super::json_str(&json, "device_code");
    let user_code = super::json_str(&json, "user_code");
    let verification = super::trusted_http_url(super::json_str(&json, "verification_uri"));
    let complete = super::trusted_http_url(super::json_str(&json, "verification_uri_complete"));
    let (Some(device_code), Some(user_code), Some(_), Some(complete)) =
        (device_code, user_code, verification, complete)
    else {
        return Err("Invalid Kimi Code device authorization response".into());
    };
    let interval = super::positive_number(&json, "interval").unwrap_or(DEFAULT_INTERVAL_SECONDS);
    let expires_in = super::positive_number(&json, "expires_in").unwrap_or(DEVICE_TIMEOUT_SECONDS);
    io.notify(AuthEvent::DeviceCode {
        user_code: user_code.to_owned(),
        verification_uri: complete,
    });
    let token_url = format!("{host}/api/oauth/token");
    poll_device(
        PollOptions {
            interval_seconds: Some(interval),
            expires_in_seconds: Some(expires_in),
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
                        ("client_id", CLIENT_ID),
                        ("device_code", device_code),
                        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ],
                )
                .timeout(REQUEST_TIMEOUT),
                Some(&io.cancel),
            )
            .await?;
            if resp.status >= 500 {
                return Ok(Poll::Failed(format!(
                    "Kimi Code device token request failed with status {}{}",
                    resp.status,
                    suffix(&resp.text)
                )));
            }
            let json = resp.json();
            if resp.ok() && super::json_str(&json, "access_token").is_some() {
                return Ok(match parse_token(&json, "poll") {
                    Ok(c) => Poll::Complete(c),
                    Err(e) => Poll::Failed(e),
                });
            }
            let error = super::json_str(&json, "error");
            Ok(match error {
                Some("authorization_pending") => Poll::Pending,
                Some("slow_down") => Poll::SlowDown(super::positive_number(&json, "interval")),
                Some("expired_token") => Poll::Failed(
                    "Kimi Code device authorization expired. Please restart login.".into(),
                ),
                Some("access_denied") => Poll::Failed("Kimi Code login was denied.".into()),
                other => Poll::Failed(format!(
                    "Kimi Code device token request failed (status {}){}",
                    resp.status,
                    other
                        .map(|e| format!(
                            ": {e}{}",
                            super::json_str(&json, "error_description")
                                .map(|d| format!(": {d}"))
                                .unwrap_or_default()
                        ))
                        .unwrap_or_default()
                )),
            })
        },
    )
    .await
}

async fn refresh(ctx: &FlowCtx, refresh_token: &str) -> Result<OAuthCredential, String> {
    let url = format!("{}/api/oauth/token", ctx.endpoints.kimi);
    let mut last_error: Option<String> = None;
    for attempt in 0..=REFRESH_MAX_RETRIES {
        if attempt > 0 {
            let reference = Duration::from_millis(1000 * 2u64.pow(attempt - 1));
            tokio::time::sleep(ctx.endpoints.retry_delay(reference, 1 << (attempt - 1))).await;
        }
        let resp = match send(
            post_form(
                &ctx.http,
                &url,
                &[
                    ("client_id", CLIENT_ID),
                    ("grant_type", "refresh_token"),
                    ("refresh_token", refresh_token),
                ],
            )
            .timeout(REQUEST_TIMEOUT),
            None,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                last_error = Some(e);
                continue;
            }
        };
        let json = resp.json();
        if resp.ok() {
            return parse_token(&json, "refresh");
        }
        if resp.status == 401
            || resp.status == 403
            || super::json_str(&json, "error") == Some("invalid_grant")
        {
            return Err(format!(
                "Kimi Code token refresh unauthorized (status {}){}",
                resp.status,
                super::json_str(&json, "error_description")
                    .map(|d| format!(": {d}"))
                    .unwrap_or_default()
            ));
        }
        if (resp.status == 429 || resp.status >= 500) && attempt < REFRESH_MAX_RETRIES {
            last_error = Some(format!(
                "Kimi Code token refresh failed with status {}",
                resp.status
            ));
            continue;
        }
        return Err(format!(
            "Kimi Code token refresh failed with status {}",
            resp.status
        ));
    }
    Err(last_error.unwrap_or_else(|| "Kimi Code token refresh failed".into()))
}

impl OAuthFlow for KimiCoding {
    fn name(&self) -> &str {
        "Kimi Code (subscription)"
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
        Box::pin(refresh(ctx, &credential.refresh))
    }

    fn to_auth(&self, credential: &OAuthCredential) -> ModelAuth {
        ModelAuth {
            api_key: None,
            headers: [(
                "Authorization".to_owned(),
                Some(format!("Bearer {}", credential.access)),
            )]
            .into(),
            base_url: None,
        }
    }
}
