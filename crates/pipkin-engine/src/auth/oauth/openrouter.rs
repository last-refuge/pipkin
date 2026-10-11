//! OpenRouter PKCE sign-in (reference `auth/oauth/openrouter.ts`): the code is exchanged for a
//! permanent, user-controlled API key stored as a non-expiring OAuth credential.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use super::callback::{
    CallbackOptions, CallbackOrManual, CallbackServer, wait_for_callback_or_manual,
};
use super::http::{post_json, send, with_query};
use super::pkce::pkce;
use super::{AuthEvent, CANCELLED, FlowCtx, Interaction, ModelAuth, OAuthCredential, OAuthFlow};
use crate::provider::BoxFuture;

const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(30);
/// JavaScript `Number.MAX_SAFE_INTEGER`: never expires.
pub const NEVER: i64 = 9_007_199_254_740_991;

pub struct OpenRouter;

fn parse_input(input: &str) -> Option<String> {
    let value = input.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(url) = url::Url::parse(value)
        && url.has_host()
    {
        return url
            .query_pairs()
            .find(|(k, _)| k == "code")
            .map(|(_, v)| v.into_owned());
    }
    if value.contains("code=") {
        return url::form_urlencoded::parse(value.as_bytes())
            .find(|(k, _)| k == "code")
            .map(|(_, v)| v.into_owned());
    }
    Some(value.to_owned())
}

fn error_detail(body: &Value) -> Option<String> {
    for k in ["error_description", "message", "error"] {
        if let Some(s) = body.get(k).and_then(Value::as_str) {
            return Some(s.to_owned());
        }
    }
    body.pointer("/error/message")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

async fn exchange(
    http: reqwest::Client,
    base: String,
    code: String,
    verifier: String,
    cancel: CancellationToken,
) -> Result<OAuthCredential, String> {
    if cancel.is_cancelled() {
        return Err(CANCELLED.into());
    }
    let rb = post_json(
        &http,
        &format!("{base}/api/v1/auth/keys"),
        &serde_json::json!({"code": code, "code_verifier": verifier, "code_challenge_method": "S256"}),
    )
    .header("accept", "application/json");
    let resp = match tokio::time::timeout(EXCHANGE_TIMEOUT, send(rb, Some(&cancel))).await {
        Err(_) => return Err("OpenRouter OAuth token exchange timed out".into()),
        Ok(r) => r?,
    };
    let body = resp.object();
    if !resp.ok() {
        return Err(format!(
            "OpenRouter OAuth key exchange failed (HTTP {}){}",
            resp.status,
            error_detail(&body)
                .map(|d| format!(": {d}"))
                .unwrap_or_default()
        ));
    }
    if !resp.json().is_object() {
        return Err("OpenRouter OAuth returned invalid JSON".into());
    }
    match body.get("key").and_then(Value::as_str) {
        Some(k) if !k.is_empty() => Ok(OAuthCredential {
            access: k.to_owned(),
            refresh: String::new(),
            expires: NEVER,
            extra: Map::new(),
        }),
        _ => Err("OpenRouter OAuth response carries no \"key\"".into()),
    }
}

async fn login(ctx: &FlowCtx, io: &Interaction) -> Result<OAuthCredential, String> {
    let pkce = pkce();
    let base = ctx.endpoints.openrouter.clone();
    let (http, verifier, cancel, base2) = (
        ctx.http.clone(),
        pkce.verifier.clone(),
        io.cancel.clone(),
        base.clone(),
    );
    // No `state`: the random path keeps stray requests from completing the sign-in.
    let callback = CallbackServer::<OAuthCredential>::start(
        CallbackOptions {
            provider_name: "OpenRouter".into(),
            host: ctx.endpoints.callback_host.clone(),
            port: 0,
            path: format!("/oauth/callback/{}", crate::util::uuid_v4()),
            redirect_host: None,
            state: None,
            timeout: Some(LOGIN_TIMEOUT),
            validate: None,
        },
        Arc::new(move |q| {
            let code = q.get("code").cloned().unwrap_or_default();
            Box::pin(exchange(
                http.clone(),
                base2.clone(),
                code,
                verifier.clone(),
                cancel.clone(),
            ))
        }),
        &io.cancel,
    )
    .await
    .map_err(|e| e.to_string())?;
    let redirect = callback.redirect_uri.clone();
    let url = with_query(
        &format!("{base}/auth"),
        &[
            ("callback_url", &redirect),
            ("code_challenge", &pkce.challenge),
            ("code_challenge_method", "S256"),
        ],
    )?;
    io.notify(AuthEvent::Progress(format!(
        "Listening for OpenRouter OAuth callback on {redirect}"
    )));
    io.notify(AuthEvent::AuthUrl {
        url,
        instructions: Some(
            "Complete sign-in in your browser. If the browser is on another machine, paste the final redirect URL here."
                .into(),
        ),
    });
    let result = wait_for_callback_or_manual(
        io,
        Some(&callback),
        "Complete sign-in in your browser, or paste the authorization code / redirect URL here:",
        &redirect,
    )
    .await?;
    match result {
        CallbackOrManual::Callback(c) => Ok(c),
        CallbackOrManual::Manual(input) => {
            let code = parse_input(&input).ok_or("Missing authorization code")?;
            io.notify(AuthEvent::Progress(
                "Exchanging authorization code for an API key...".into(),
            ));
            exchange(
                ctx.http.clone(),
                base,
                code,
                pkce.verifier,
                io.cancel.clone(),
            )
            .await
        }
    }
}

impl OAuthFlow for OpenRouter {
    fn name(&self) -> &str {
        "OpenRouter OAuth"
    }

    fn login<'a>(
        &'a self,
        ctx: &'a FlowCtx,
        io: &'a Interaction,
    ) -> BoxFuture<'a, Result<OAuthCredential, String>> {
        Box::pin(login(ctx, io))
    }

    /// The key does not expire; refresh is the identity.
    fn refresh<'a>(
        &'a self,
        _ctx: &'a FlowCtx,
        credential: &'a OAuthCredential,
    ) -> BoxFuture<'a, Result<OAuthCredential, String>> {
        Box::pin(async move { Ok(credential.clone()) })
    }

    fn to_auth(&self, credential: &OAuthCredential) -> ModelAuth {
        ModelAuth {
            api_key: Some(credential.access.clone()),
            ..Default::default()
        }
    }
}
