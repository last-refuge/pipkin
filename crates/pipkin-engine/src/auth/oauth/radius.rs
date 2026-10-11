//! Radius gateway sign-in (reference `auth/oauth/radius.ts`): browser PKCE with a loopback
//! callback on 127.0.0.1:1456 (authorization endpoint discovered from the gateway), or device
//! code. Token endpoints live on the gateway.

use std::sync::Arc;

use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use super::callback::{CallbackOptions, CallbackServer};
use super::device::{Poll, PollOptions, poll_device};
use super::http::{post_form, send, with_query};
use super::pkce::pkce;
use super::{
    AuthEvent, CANCELLED, FlowCtx, Interaction, ModelAuth, OAuthCredential, OAuthFlow, Prompt,
};
use crate::provider::BoxFuture;

const CALLBACK_HOST: &str = "127.0.0.1";
const PORT: u16 = 1456;
const CALLBACK_PATH: &str = "/oauth/callback";
const EXPIRY_SKEW_MS: i64 = 60_000;
const CLIENT_ID: &str = "pi-gateway";
const SCOPE: &str = "gateway offline_access";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

pub struct Radius {
    name: String,
    gateway: String,
}

impl Radius {
    pub fn new(name: String, gateway: String) -> Radius {
        Radius {
            name,
            gateway: crate::auth::models_json::normalize_gateway(&gateway),
        }
    }

    fn url(&self, path: &str) -> Result<String, String> {
        url::Url::parse(&self.gateway)
            .and_then(|u| u.join(path))
            .map(|u| u.to_string())
            .map_err(|e| format!("Invalid Radius gateway {}: {e}", self.gateway))
    }
}

/// A token endpoint error with its OAuth `error` code.
struct OAuthError {
    code: Option<String>,
    message: String,
}

fn response_error(status: u16, text: &str, message: &str) -> OAuthError {
    let (mut code, mut description) = (None, None);
    if !text.is_empty() {
        match serde_json::from_str::<Value>(text) {
            Ok(v) => {
                code = super::json_str(&v, "error").map(str::to_owned);
                description = super::json_str(&v, "error_description").map(str::to_owned);
            }
            Err(_) => description = Some(text.to_owned()),
        }
    }
    let detail = match (&code, &description) {
        (Some(c), Some(d)) => format!("{c}: {d}"),
        (Some(c), None) => c.clone(),
        (None, Some(d)) if !d.is_empty() => d.clone(),
        _ => status.to_string(),
    };
    OAuthError {
        code,
        message: format!("{message}: {detail}"),
    }
}

async fn request_token(
    ctx: &FlowCtx,
    url: &str,
    pairs: &[(&str, &str)],
    cancel: Option<&CancellationToken>,
) -> Result<Result<OAuthCredential, OAuthError>, String> {
    let resp = send(
        post_form(&ctx.http, url, pairs).header("accept", "application/json"),
        cancel,
    )
    .await?;
    if !resp.ok() {
        return Ok(Err(response_error(
            resp.status,
            &resp.text,
            "Radius OAuth token request failed",
        )));
    }
    let json = resp.json();
    let (Some(access), Some(refresh), Some(expires_in)) = (
        super::json_str(&json, "access_token"),
        super::json_str(&json, "refresh_token"),
        json.get("expires_in").and_then(Value::as_f64),
    ) else {
        return Err("Radius OAuth token response is missing required fields".into());
    };
    let mut extra = Map::new();
    if let Some(scope) = super::json_str(&json, "scope") {
        extra.insert("scope".into(), Value::String(scope.to_owned()));
    }
    Ok(Ok(OAuthCredential {
        access: access.to_owned(),
        refresh: refresh.to_owned(),
        expires: super::now_ms() + (expires_in * 1000.0) as i64 - EXPIRY_SKEW_MS,
        extra,
    }))
}

impl Radius {
    async fn login_browser(
        &self,
        ctx: &FlowCtx,
        io: &Interaction,
    ) -> Result<OAuthCredential, String> {
        let resp = send(
            ctx.http
                .get(self.url("/v1/oauth")?)
                .header("accept", "application/json"),
            Some(&io.cancel),
        )
        .await?;
        if !resp.ok() {
            return Err(format!(
                "Could not load Radius OAuth config from {}: {} {}",
                self.gateway, resp.status, resp.text
            ));
        }
        let endpoint = super::json_str(&resp.json(), "authorizationEndpoint")
            .map(str::to_owned)
            .ok_or_else(|| format!("Invalid Radius OAuth config from {}", self.gateway))?;
        let pkce = pkce();
        let state = crate::util::uuid_v4();
        let token_url = self.url("/v1/oauth/token")?;
        let port = ctx.endpoints.port(PORT);
        let redirect_slot: Arc<std::sync::OnceLock<String>> = Arc::default();
        let (http, verifier, cancel, slot) = (
            ctx.clone(),
            pkce.verifier.clone(),
            io.cancel.clone(),
            redirect_slot.clone(),
        );
        let callback = CallbackServer::<OAuthCredential>::start(
            CallbackOptions {
                provider_name: "Radius".into(),
                host: CALLBACK_HOST.into(),
                port,
                path: CALLBACK_PATH.into(),
                redirect_host: None,
                state: Some(state.clone()),
                timeout: None,
                validate: None,
            },
            Arc::new(move |q| {
                let (ctx, verifier, cancel, slot, token_url) = (
                    http.clone(),
                    verifier.clone(),
                    cancel.clone(),
                    slot.clone(),
                    token_url.clone(),
                );
                Box::pin(async move {
                    let code = q.get("code").cloned().unwrap_or_default();
                    let redirect = slot.get().cloned().unwrap_or_default();
                    request_token(
                        &ctx,
                        &token_url,
                        &[
                            ("grant_type", "authorization_code"),
                            ("client_id", CLIENT_ID),
                            ("redirect_uri", &redirect),
                            ("code", &code),
                            ("code_verifier", &verifier),
                        ],
                        Some(&cancel),
                    )
                    .await?
                    .map_err(|e| e.message)
                })
            }),
            &io.cancel,
        )
        .await
        .map_err(|e| e.to_string())?;
        let redirect = callback.redirect_uri.clone();
        let _ = redirect_slot.set(redirect.clone());
        let url = with_query(
            &endpoint,
            &[
                ("response_type", "code"),
                ("client_id", CLIENT_ID),
                ("redirect_uri", &redirect),
                ("scope", SCOPE),
                ("code_challenge", &pkce.challenge),
                ("code_challenge_method", "S256"),
                ("handoff", "url"),
                ("state", &state),
            ],
        )?;
        io.notify(AuthEvent::Progress(format!(
            "Listening for OAuth callback on {redirect}"
        )));
        io.notify(AuthEvent::AuthUrl {
            url,
            instructions: Some("Continue in your browser.".into()),
        });
        match callback.wait().await {
            Some(r) => r,
            None => Err("OAuth callback did not complete.".into()),
        }
    }

    async fn login_device(
        &self,
        ctx: &FlowCtx,
        io: &Interaction,
    ) -> Result<OAuthCredential, String> {
        let resp = send(
            post_form(
                &ctx.http,
                &self.url("/v1/oauth/device")?,
                &[("client_id", CLIENT_ID), ("scope", SCOPE)],
            )
            .header("accept", "application/json"),
            Some(&io.cancel),
        )
        .await?;
        if !resp.ok() {
            return Err(response_error(
                resp.status,
                &resp.text,
                "Radius OAuth device authorization failed",
            )
            .message);
        }
        let json = resp.json();
        let (Some(device_code), Some(user_code), Some(verification), Some(expires_in)) = (
            super::json_str(&json, "device_code").filter(|s| !s.is_empty()),
            super::json_str(&json, "user_code").filter(|s| !s.is_empty()),
            super::json_str(&json, "verification_uri").filter(|s| !s.is_empty()),
            super::positive_number(&json, "expires_in"),
        ) else {
            return Err(
                "Radius OAuth device authorization response is missing required fields".into(),
            );
        };
        io.notify(AuthEvent::DeviceCode {
            user_code: user_code.to_owned(),
            verification_uri: verification.to_owned(),
        });
        let token_url = self.url("/v1/oauth/token")?;
        poll_device(
            PollOptions {
                interval_seconds: json.get("interval").and_then(Value::as_f64),
                expires_in_seconds: Some(expires_in),
                wait_before_first_poll: false,
                fixed_interval: ctx.endpoints.poll_interval,
            },
            &io.cancel,
            || async {
                let r = request_token(
                    ctx,
                    &token_url,
                    &[
                        ("grant_type", DEVICE_GRANT),
                        ("client_id", CLIENT_ID),
                        ("device_code", device_code),
                    ],
                    Some(&io.cancel),
                )
                .await?;
                Ok(match r {
                    Ok(c) => Poll::Complete(c),
                    Err(e) => match e.code.as_deref() {
                        Some("authorization_pending") => Poll::Pending,
                        Some("slow_down") => Poll::SlowDown(None),
                        Some("expired_token") => {
                            Poll::Failed("Device authorization expired.".into())
                        }
                        Some("access_denied") => {
                            Poll::Failed("Device authorization was denied.".into())
                        }
                        _ => return Err(e.message),
                    },
                })
            },
        )
        .await
    }
}

impl OAuthFlow for Radius {
    fn name(&self) -> &str {
        &self.name
    }

    fn login<'a>(
        &'a self,
        ctx: &'a FlowCtx,
        io: &'a Interaction,
    ) -> BoxFuture<'a, Result<OAuthCredential, String>> {
        Box::pin(async move {
            let method = io
                .prompt(Prompt::select(
                    &format!("Sign in to {}:", self.name),
                    &[
                        ("browser", "Sign in with browser (recommended)"),
                        (
                            "device-code",
                            "Sign in with device code (when signing in from another device)",
                        ),
                    ],
                ))
                .await?;
            let r = match method.as_str() {
                "device-code" => self.login_device(ctx, io).await,
                "browser" => self.login_browser(ctx, io).await,
                other => Err(format!("Unknown {} sign-in method: {other}", self.name)),
            };
            match r {
                Err(_) if io.cancel.is_cancelled() => Err(CANCELLED.into()),
                r => r,
            }
        })
    }

    fn refresh<'a>(
        &'a self,
        ctx: &'a FlowCtx,
        credential: &'a OAuthCredential,
    ) -> BoxFuture<'a, Result<OAuthCredential, String>> {
        Box::pin(async move {
            request_token(
                ctx,
                &self.url("/v1/oauth/token")?,
                &[
                    ("grant_type", "refresh_token"),
                    ("client_id", CLIENT_ID),
                    ("refresh_token", &credential.refresh),
                ],
                None,
            )
            .await?
            .map_err(|e| e.message)
        })
    }

    fn to_auth(&self, credential: &OAuthCredential) -> ModelAuth {
        ModelAuth {
            api_key: Some(credential.access.clone()),
            ..Default::default()
        }
    }
}
