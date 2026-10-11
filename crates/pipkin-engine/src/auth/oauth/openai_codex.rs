//! OpenAI Codex (ChatGPT Plus/Pro) sign-in, provider `openai-codex`
//! (reference `auth/oauth/openai-codex.ts`): browser PKCE with a loopback callback on port 1455
//! (manual paste when the port is taken), or device-code login.

use std::sync::Arc;

use serde_json::{Map, Value};

use super::callback::{
    CallbackOptions, CallbackOrManual, CallbackServer, wait_for_callback_or_manual,
};
use super::device::{Poll, PollOptions, poll_device};
use super::http::{post_form, post_json, send, with_query};
use super::pkce::{pkce, random_bytes};
use super::{AuthEvent, FlowCtx, Interaction, ModelAuth, OAuthCredential, OAuthFlow, Prompt};
use crate::provider::BoxFuture;

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const SCOPE: &str = "openid profile email offline_access";
const PORT: u16 = 1455;
const CALLBACK_PATH: &str = "/auth/callback";
const DEVICE_TIMEOUT_SECONDS: f64 = 15.0 * 60.0;
const JWT_CLAIM_PATH: &str = "https://api.openai.com/auth";

pub struct OpenAICodex;

struct Token {
    access: String,
    refresh: String,
    expires: i64,
}

/// Accepts a redirect URL, `code#state`, a query string or a raw code.
pub(crate) fn parse_authorization_input(input: &str) -> (Option<String>, Option<String>) {
    let value = input.trim();
    if value.is_empty() {
        return (None, None);
    }
    if let Ok(url) = url::Url::parse(value)
        && url.has_host()
    {
        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        return (q.get("code").cloned(), q.get("state").cloned());
    }
    if let Some((code, state)) = value.split_once('#') {
        return (Some(code.to_owned()), Some(state.to_owned()));
    }
    if value.contains("code=") {
        let q: std::collections::HashMap<_, _> = url::form_urlencoded::parse(value.as_bytes())
            .into_owned()
            .collect();
        return (q.get("code").cloned(), q.get("state").cloned());
    }
    (Some(value.to_owned()), None)
}

fn account_id(access: &str) -> Option<String> {
    super::pkce::jwt_payload(access)?
        .get(JWT_CLAIM_PATH)?
        .get("chatgpt_account_id")?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn credential_from_token(t: Token) -> Result<OAuthCredential, String> {
    let account = account_id(&t.access).ok_or("Failed to extract accountId from token")?;
    let mut extra = Map::new();
    extra.insert("accountId".into(), Value::String(account));
    Ok(OAuthCredential {
        access: t.access,
        refresh: t.refresh,
        expires: t.expires,
        extra,
    })
}

async fn token_request(
    ctx: &FlowCtx,
    pairs: &[(&str, &str)],
    operation: &str,
    io: Option<&Interaction>,
) -> Result<Token, String> {
    let url = format!("{}/oauth/token", ctx.endpoints.openai_auth);
    let resp = send(post_form(&ctx.http, &url, pairs), io.map(|i| &i.cancel))
        .await
        .map_err(|e| {
            if operation == "refresh" {
                format!("OpenAI Codex token refresh error: {e}")
            } else {
                e
            }
        })?;
    if !resp.ok() {
        return Err(format!(
            "OpenAI Codex token {operation} failed ({}): {}",
            resp.status, resp.text
        ));
    }
    let json = resp.json();
    let access = super::json_str(&json, "access_token");
    let refresh = super::json_str(&json, "refresh_token");
    let expires_in = json.get("expires_in").and_then(Value::as_f64);
    match (access, refresh, expires_in) {
        (Some(a), Some(r), Some(e)) if !a.is_empty() && !r.is_empty() => Ok(Token {
            access: a.to_owned(),
            refresh: r.to_owned(),
            expires: super::now_ms() + (e * 1000.0) as i64,
        }),
        _ => Err(format!(
            "OpenAI Codex token {operation} response missing fields"
        )),
    }
}

async fn exchange(
    ctx: &FlowCtx,
    io: &Interaction,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<OAuthCredential, String> {
    credential_from_token(
        token_request(
            ctx,
            &[
                ("grant_type", "authorization_code"),
                ("client_id", CLIENT_ID),
                ("code", code),
                ("code_verifier", verifier),
                ("redirect_uri", redirect_uri),
            ],
            "exchange",
            Some(io),
        )
        .await?,
    )
}

async fn login_browser(ctx: &FlowCtx, io: &Interaction) -> Result<OAuthCredential, String> {
    let pkce = pkce();
    let state: String = random_bytes(16)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let port = ctx.endpoints.port(PORT);
    // Port 1455 is shared with the Codex CLI; when it is taken, fall back to the pasted URL.
    let callback = CallbackServer::<String>::start(
        CallbackOptions {
            provider_name: "OpenAI".into(),
            host: ctx.endpoints.callback_host.clone(),
            port,
            path: CALLBACK_PATH.into(),
            redirect_host: Some("localhost".into()),
            state: Some(state.clone()),
            timeout: None,
            validate: None,
        },
        Arc::new(|q| Box::pin(async move { Ok(q.get("code").cloned().unwrap_or_default()) })),
        &io.cancel,
    )
    .await
    .ok();
    let redirect_uri = callback
        .as_ref()
        .map(|c| c.redirect_uri.clone())
        .unwrap_or_else(|| format!("http://localhost:{port}{CALLBACK_PATH}"));
    let url = with_query(
        &format!("{}/oauth/authorize", ctx.endpoints.openai_auth),
        &[
            ("response_type", "code"),
            ("client_id", CLIENT_ID),
            ("redirect_uri", &redirect_uri),
            ("scope", SCOPE),
            ("code_challenge", &pkce.challenge),
            ("code_challenge_method", "S256"),
            ("state", &state),
            ("id_token_add_organizations", "true"),
            ("codex_cli_simplified_flow", "true"),
            ("originator", "pi"),
        ],
    )?;
    io.notify(AuthEvent::AuthUrl {
        url,
        instructions: Some("A browser window should open. Complete login to finish.".into()),
    });
    let result = wait_for_callback_or_manual(
        io,
        callback.as_ref(),
        "Complete login in your browser, or paste the authorization code / redirect URL here:",
        &redirect_uri,
    )
    .await?;
    let code = match result {
        CallbackOrManual::Callback(code) => Some(code),
        CallbackOrManual::Manual(input) => {
            let (code, got_state) = parse_authorization_input(&input);
            if got_state.is_some_and(|s| s != state) {
                return Err("State mismatch".into());
            }
            code
        }
    };
    let code = code
        .filter(|c| !c.is_empty())
        .ok_or("Missing authorization code")?;
    exchange(ctx, io, &code, &pkce.verifier, &redirect_uri).await
}

async fn login_device(ctx: &FlowCtx, io: &Interaction) -> Result<OAuthCredential, String> {
    let base = &ctx.endpoints.openai_auth;
    let resp = send(
        post_json(
            &ctx.http,
            &format!("{base}/api/accounts/deviceauth/usercode"),
            &serde_json::json!({"client_id": CLIENT_ID}),
        ),
        Some(&io.cancel),
    )
    .await?;
    if !resp.ok() {
        if resp.status == 404 {
            return Err("OpenAI Codex device code login is not enabled for this server. Use browser login or verify the server URL.".into());
        }
        return Err(format!(
            "OpenAI Codex device code request failed with status {}{}",
            resp.status,
            if resp.text.is_empty() {
                String::new()
            } else {
                format!(": {}", resp.text)
            }
        ));
    }
    let json = resp.json();
    let interval = match json.get("interval") {
        Some(Value::String(s)) => s.trim().parse::<f64>().ok(),
        Some(v) => v.as_f64(),
        None => None,
    };
    let (Some(device_auth_id), Some(user_code), Some(interval)) = (
        super::json_str(&json, "device_auth_id").filter(|s| !s.is_empty()),
        super::json_str(&json, "user_code").filter(|s| !s.is_empty()),
        interval.filter(|i| i.is_finite() && *i >= 0.0),
    ) else {
        return Err("Invalid OpenAI Codex device code response".into());
    };
    io.notify(AuthEvent::DeviceCode {
        user_code: user_code.to_owned(),
        verification_uri: format!("{base}/codex/device"),
    });
    let token_url = format!("{base}/api/accounts/deviceauth/token");
    let body = serde_json::json!({"device_auth_id": device_auth_id, "user_code": user_code});
    let (code, verifier) = poll_device(
        PollOptions {
            interval_seconds: Some(interval),
            expires_in_seconds: Some(DEVICE_TIMEOUT_SECONDS),
            wait_before_first_poll: false,
            fixed_interval: ctx.endpoints.poll_interval,
        },
        &io.cancel,
        || async {
            let resp = send(post_json(&ctx.http, &token_url, &body), Some(&io.cancel)).await?;
            if resp.ok() {
                let json = resp.json();
                return Ok(
                    match (
                        super::json_str(&json, "authorization_code"),
                        super::json_str(&json, "code_verifier"),
                    ) {
                        (Some(c), Some(v)) if !c.is_empty() && !v.is_empty() => {
                            Poll::Complete((c.to_owned(), v.to_owned()))
                        }
                        _ => Poll::Failed("Invalid OpenAI Codex device auth token response".into()),
                    },
                );
            }
            if resp.status == 403 || resp.status == 404 {
                return Ok(Poll::Pending);
            }
            let json = resp.json();
            let code = match json.get("error") {
                Some(Value::Object(o)) => o.get("code").and_then(Value::as_str),
                Some(Value::String(s)) => Some(s.as_str()),
                _ => None,
            };
            Ok(match code {
                Some("deviceauth_authorization_pending") => Poll::Pending,
                Some("slow_down") => Poll::SlowDown(None),
                _ => Poll::Failed(format!(
                    "OpenAI Codex device auth failed with status {}{}",
                    resp.status,
                    if resp.text.is_empty() {
                        String::new()
                    } else {
                        format!(": {}", resp.text)
                    }
                )),
            })
        },
    )
    .await?;
    exchange(
        ctx,
        io,
        &code,
        &verifier,
        &format!("{base}/deviceauth/callback"),
    )
    .await
}

impl OAuthFlow for OpenAICodex {
    fn name(&self) -> &str {
        "OpenAI (ChatGPT Plus/Pro)"
    }

    fn login<'a>(
        &'a self,
        ctx: &'a FlowCtx,
        io: &'a Interaction,
    ) -> BoxFuture<'a, Result<OAuthCredential, String>> {
        Box::pin(async move {
            let method = io
                .prompt(Prompt::select(
                    "Select OpenAI Codex login method:",
                    &[
                        ("browser", "Browser login (default)"),
                        ("device_code", "Device code login (headless)"),
                    ],
                ))
                .await?;
            match method.as_str() {
                "device_code" => login_device(ctx, io).await,
                "browser" => login_browser(ctx, io).await,
                other => Err(format!("Unknown OpenAI Codex login method: {other}")),
            }
        })
    }

    fn refresh<'a>(
        &'a self,
        ctx: &'a FlowCtx,
        credential: &'a OAuthCredential,
    ) -> BoxFuture<'a, Result<OAuthCredential, String>> {
        Box::pin(async move {
            credential_from_token(
                token_request(
                    ctx,
                    &[
                        ("grant_type", "refresh_token"),
                        ("refresh_token", &credential.refresh),
                        ("client_id", CLIENT_ID),
                    ],
                    "refresh",
                    None,
                )
                .await?,
            )
        })
    }

    fn to_auth(&self, credential: &OAuthCredential) -> ModelAuth {
        ModelAuth {
            api_key: Some(credential.access.clone()),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_input_forms() {
        let p = parse_authorization_input;
        assert_eq!(
            p("http://localhost:1455/auth/callback?code=c1&state=s1"),
            (Some("c1".into()), Some("s1".into()))
        );
        assert_eq!(p("c2#s2"), (Some("c2".into()), Some("s2".into())));
        assert_eq!(
            p("code=c3&state=s3"),
            (Some("c3".into()), Some("s3".into()))
        );
        assert_eq!(p("  rawcode "), (Some("rawcode".into()), None));
        assert_eq!(p(""), (None, None));
    }
}
