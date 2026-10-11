//! Claude Pro/Max OAuth, ported from Pi at d2a311097cbcf669e699479587332ae3988a49d0.
//! Copyright (c) 2025 Mario Zechner. MIT; see packaging/upstream-notices/pi/LICENSE.
//! Browser/loopback and copy-code flows use the existing per-client lifecycle and store.

use std::sync::Arc;

use serde_json::{Map, Value, json};

use super::callback::{
    CallbackOptions, CallbackOrManual, CallbackServer, wait_for_callback_or_manual,
};
use super::http::{post_json, send, with_query};
use super::pkce::pkce;
use super::{AuthEvent, FlowCtx, Interaction, ModelAuth, OAuthCredential, OAuthFlow, Prompt};
use crate::provider::BoxFuture;

const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const SCOPES: &str = "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";
const PORT: u16 = 53692;
const CALLBACK_PATH: &str = "/callback";

pub struct Anthropic;

// Same accepted paste forms as Pi: redirect URL, code#state, query string, or code.
fn parse_input(input: &str) -> (Option<String>, Option<String>) {
    super::openai_codex::parse_authorization_input(input)
}

async fn token_request(
    ctx: &FlowCtx,
    body: Value,
    io: Option<&Interaction>,
) -> Result<OAuthCredential, String> {
    let response = send(
        post_json(
            &ctx.http,
            &format!("{}/v1/oauth/token", ctx.endpoints.anthropic_platform),
            &body,
        )
        .header("accept", "application/json")
        .timeout(std::time::Duration::from_secs(30)),
        io.map(|i| &i.cancel),
    )
    .await
    .map_err(|_| "Anthropic token request failed".to_owned())?;
    // Token endpoints may echo secrets: never retain their body in a provider error.
    if !response.ok() {
        return Err(format!(
            "Anthropic token request failed (HTTP {})",
            response.status
        ));
    }
    let value = response.json();
    let access = super::json_str(&value, "access_token").filter(|s| !s.is_empty());
    let refresh = super::json_str(&value, "refresh_token").filter(|s| !s.is_empty());
    let lifetime = super::positive_number(&value, "expires_in")
        .filter(|s| *s <= (i64::MAX / 1000 - super::now_ms() / 1000) as f64);
    match (access, refresh, lifetime) {
        (Some(a), Some(r), Some(seconds)) => Ok(OAuthCredential {
            access: a.to_owned(),
            refresh: r.to_owned(),
            // Auth::fresh_oauth already applies Pi's five-minute early-refresh margin.
            expires: super::now_ms() + (seconds * 1000.0) as i64,
            extra: Map::new(),
        }),
        _ => Err("Anthropic token response is missing valid token fields".into()),
    }
}

async fn login(ctx: &FlowCtx, io: &Interaction) -> Result<OAuthCredential, String> {
    let method = io
        .prompt(Prompt::select(
            "Select Anthropic login method:",
            &[
                ("browser", "Browser login (default)"),
                ("copy_code", "Copy code login (headless)"),
            ],
        ))
        .await?;
    let copy_code = match method.as_str() {
        "browser" => false,
        "copy_code" => true,
        _ => return Err("Unknown Anthropic login method".into()),
    };
    let proof = pkce();
    let callback = if copy_code {
        None
    } else {
        CallbackServer::<String>::start(
            CallbackOptions {
                provider_name: "Anthropic".into(),
                host: ctx.endpoints.callback_host.clone(),
                port: ctx.endpoints.port(PORT),
                path: CALLBACK_PATH.into(),
                redirect_host: Some("localhost".into()),
                state: Some(proof.verifier.clone()),
                timeout: None,
                validate: None,
            },
            Arc::new(|q| Box::pin(async move { Ok(q.get("code").cloned().unwrap_or_default()) })),
            &io.cancel,
        )
        .await
        .ok()
    };
    let redirect = if copy_code {
        format!("{}/oauth/code/callback", ctx.endpoints.anthropic_platform)
    } else {
        callback
            .as_ref()
            .map(|c| c.redirect_uri.clone())
            .unwrap_or_else(|| {
                format!(
                    "http://localhost:{}{CALLBACK_PATH}",
                    ctx.endpoints.port(PORT)
                )
            })
    };
    let url = with_query(
        &format!("{}/oauth/authorize", ctx.endpoints.anthropic_authorize),
        &[
            ("code", "true"),
            ("client_id", CLIENT_ID),
            ("response_type", "code"),
            ("redirect_uri", &redirect),
            ("scope", SCOPES),
            ("code_challenge", &proof.challenge),
            ("code_challenge_method", "S256"),
            ("state", &proof.verifier),
        ],
    )?;
    io.notify(AuthEvent::AuthUrl { url, instructions: Some(if copy_code {
        "Complete login in your browser, then copy the code Anthropic shows and paste it here."
    } else {
        "Complete login in your browser. If the browser is on another machine, paste the final redirect URL here."
    }.into()) });
    let response = if copy_code {
        CallbackOrManual::Manual(
            io.prompt(Prompt::manual_code(
                "Paste the code Anthropic shows after you sign in:",
                "code#state",
            ))
            .await?,
        )
    } else {
        wait_for_callback_or_manual(
            io,
            callback.as_ref(),
            "Complete login in your browser, or paste the authorization code / redirect URL here:",
            &redirect,
        )
        .await?
    };
    let code = match response {
        CallbackOrManual::Callback(code) => Some(code),
        CallbackOrManual::Manual(text) => {
            let (code, state) = parse_input(&text);
            if state.is_some_and(|s| !s.is_empty() && s != proof.verifier) {
                return Err("OAuth state mismatch".into());
            }
            code
        }
    }
    .filter(|s| !s.is_empty())
    .ok_or("Missing authorization code")?;
    io.notify(AuthEvent::Progress(
        "Exchanging authorization code for tokens...".into(),
    ));
    token_request(
        ctx,
        json!({
            "grant_type": "authorization_code", "client_id": CLIENT_ID,
            "code": code, "state": proof.verifier, "code_verifier": proof.verifier,
            "redirect_uri": redirect,
        }),
        Some(io),
    )
    .await
}

impl OAuthFlow for Anthropic {
    fn name(&self) -> &str {
        "Anthropic (Claude Pro/Max)"
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
            let mut refreshed = token_request(ctx, json!({
                "grant_type": "refresh_token", "client_id": CLIENT_ID, "refresh_token": credential.refresh,
            }), None).await?;
            refreshed.extra = credential.extra.clone();
            Ok(refreshed)
        })
    }
    fn to_auth(&self, credential: &OAuthCredential) -> ModelAuth {
        ModelAuth {
            api_key: Some(credential.access.clone()),
            ..Default::default()
        }
    }
}
