//! xAI device flow (reference `auth/oauth/xai.ts`).

use serde_json::{Map, Value};

use super::device::{Poll, PollOptions, poll_device};
use super::http::{Resp, post_form, send};
use super::{AuthEvent, FlowCtx, Interaction, ModelAuth, OAuthCredential, OAuthFlow};
use crate::provider::BoxFuture;

const CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const SCOPE: &str = "openid profile email offline_access grok-cli:access api:access";
const REFRESH_SKEW_MS: i64 = 5 * 60 * 1000;
const DEFAULT_LIFETIME_SECONDS: f64 = 3600.0;

pub struct Xai;

fn required(body: &Value, field: &str) -> Result<String, String> {
    super::json_str(body, field)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("Invalid xAI OAuth response field: {field}"))
}

fn verification(raw: &str) -> Result<String, String> {
    match url::Url::parse(raw) {
        Ok(u) if u.scheme() == "https" => Ok(u.to_string()),
        _ => Err("Untrusted verification URI in xAI OAuth response".into()),
    }
}

fn body(resp: &Resp) -> Result<Value, String> {
    match resp.json() {
        v @ Value::Object(_) => Ok(v),
        Value::Null if serde_json::from_str::<Value>(&resp.text).is_err() => Err(format!(
            "xAI OAuth returned invalid JSON (HTTP {})",
            resp.status
        )),
        _ => Ok(Value::Object(Map::new())),
    }
}

fn failure(action: &str, status: u16, body: &Value) -> String {
    let detail: Vec<&str> = [
        super::json_str(body, "error"),
        super::json_str(body, "error_description"),
    ]
    .into_iter()
    .flatten()
    .collect();
    format!(
        "xAI OAuth {action} failed (HTTP {status}){}",
        if detail.is_empty() {
            String::new()
        } else {
            format!(": {}", detail.join(": "))
        }
    )
}

fn credential(body: &Value, previous_refresh: Option<&str>) -> Result<OAuthCredential, String> {
    let access = required(body, "access_token")?;
    let refresh = match (body.get("refresh_token"), previous_refresh) {
        (None, Some(prev)) if !prev.is_empty() => prev.to_owned(),
        _ => required(body, "refresh_token")?,
    };
    let lifetime = match body.get("expires_in") {
        None => DEFAULT_LIFETIME_SECONDS,
        Some(_) => super::positive_number(body, "expires_in")
            .ok_or("Invalid xAI OAuth response field: expires_in")?,
    };
    Ok(OAuthCredential {
        access,
        refresh,
        expires: super::now_ms() + (lifetime * 1000.0) as i64 - REFRESH_SKEW_MS,
        extra: Map::new(),
    })
}

async fn login(ctx: &FlowCtx, io: &Interaction) -> Result<OAuthCredential, String> {
    let base = &ctx.endpoints.xai;
    let resp = send(
        post_form(
            &ctx.http,
            &format!("{base}/oauth2/device/code"),
            &[
                ("client_id", CLIENT_ID),
                ("scope", SCOPE),
                ("referrer", "pi"),
            ],
        ),
        Some(&io.cancel),
    )
    .await?;
    let b = body(&resp)?;
    if !resp.ok() {
        return Err(failure("device authorization", resp.status, &b));
    }
    let device_code = required(&b, "device_code")?;
    let user_code = required(&b, "user_code")?;
    let uri = verification(&required(&b, "verification_uri")?)?;
    let complete = match super::json_str(&b, "verification_uri_complete").filter(|s| !s.is_empty())
    {
        Some(c) => Some(verification(c)?),
        None => None,
    };
    let expires_in = super::positive_number(&b, "expires_in")
        .ok_or("Invalid xAI OAuth response field: expires_in")?;
    io.notify(AuthEvent::DeviceCode {
        user_code,
        verification_uri: complete.unwrap_or(uri),
    });
    let token_url = format!("{base}/oauth2/token");
    poll_device(
        PollOptions {
            interval_seconds: super::positive_number(&b, "interval"),
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
                        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                        ("client_id", CLIENT_ID),
                        ("device_code", &device_code),
                    ],
                ),
                Some(&io.cancel),
            )
            .await?;
            let b = body(&resp)?;
            if resp.ok() {
                return credential(&b, None).map(Poll::Complete);
            }
            Ok(match super::json_str(&b, "error") {
                Some("authorization_pending") => Poll::Pending,
                Some("slow_down") => Poll::SlowDown(b.get("interval").and_then(Value::as_f64)),
                Some("access_denied" | "authorization_denied") => {
                    Poll::Failed("xAI device authorization was denied".into())
                }
                Some("expired_token") => Poll::Failed("xAI device code expired".into()),
                _ => Poll::Failed(failure("device token polling", resp.status, &b)),
            })
        },
    )
    .await
}

impl OAuthFlow for Xai {
    fn name(&self) -> &str {
        "xAI (Grok/X subscription)"
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
        cred: &'a OAuthCredential,
    ) -> BoxFuture<'a, Result<OAuthCredential, String>> {
        Box::pin(async move {
            let resp = send(
                post_form(
                    &ctx.http,
                    &format!("{}/oauth2/token", ctx.endpoints.xai),
                    &[
                        ("grant_type", "refresh_token"),
                        ("client_id", CLIENT_ID),
                        ("refresh_token", &cred.refresh),
                    ],
                ),
                None,
            )
            .await?;
            let b = body(&resp)?;
            if !resp.ok() {
                return Err(failure("token refresh", resp.status, &b));
            }
            credential(&b, Some(&cred.refresh))
        })
    }

    fn to_auth(&self, credential: &OAuthCredential) -> ModelAuth {
        ModelAuth {
            api_key: Some(credential.access.clone()),
            ..Default::default()
        }
    }
}
