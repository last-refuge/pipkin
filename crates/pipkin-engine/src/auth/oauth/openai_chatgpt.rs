//! OpenAI "Sign in with ChatGPT" for the Responses API, provider `openai`
//! (reference `auth/oauth/openai-chatgpt.ts`). Dynamic client registration: the callback
//! carries the issued `client_id`, stored with the credential and used for refresh.

use std::sync::Arc;

use serde_json::{Map, Value};

use super::callback::{CallbackOptions, CallbackServer, Query};
use super::http::{post_form, send, with_query};
use super::pkce::{b64url, pkce, random_bytes};
use super::{
    AuthEvent, CANCELLED, FlowCtx, Interaction, ModelAuth, OAuthCredential, OAuthFlow, Prompt,
};
use crate::provider::BoxFuture;

const DYNAMIC_CLIENT_ID: &str = "dynamic_agent_client";
const AGENT_NAME_HINT: &str = "Pi";
const RESOURCE: &str = "https://api.openai.com/v1";
const PORT: u16 = 1455;
const CALLBACK_PATH: &str = "/auth/callback";
const DIRECT_TOKEN_SCOPE: &str = "chatgpt.tokens.use.direct";
const EXPIRY_MARGIN_MS: i64 = 3 * 60 * 1000;

pub struct OpenAIChatGPT;

struct Authorization {
    code: String,
    client_id: String,
}

fn from_query(q: &Query, expected_state: &str) -> Result<Authorization, String> {
    let code = q
        .get("code")
        .filter(|c| !c.is_empty())
        .ok_or("Missing authorization code")?;
    let state = q
        .get("state")
        .filter(|s| !s.is_empty())
        .ok_or("Missing OAuth state")?;
    if state != expected_state {
        return Err("OAuth state mismatch".into());
    }
    let client_id = q
        .get("client_id")
        .map(|c| c.trim())
        .filter(|c| !c.is_empty())
        .ok_or("OpenAI OAuth registration callback did not contain an issued client ID")?;
    Ok(Authorization {
        code: code.clone(),
        client_id: client_id.to_owned(),
    })
}

fn from_manual_input(
    input: &str,
    redirect_uri: &str,
    state: &str,
) -> Result<Authorization, String> {
    let url = url::Url::parse(input.trim())
        .map_err(|_| "Paste the full callback URL from the browser".to_string())?;
    let expected = url::Url::parse(redirect_uri).map_err(|e| e.to_string())?;
    if url.origin() != expected.origin() || url.path() != expected.path() {
        return Err(format!(
            "The pasted callback URL must start with {redirect_uri}"
        ));
    }
    let q: Query = url.query_pairs().into_owned().collect();
    if let Some(e) = q.get("error") {
        return Err(format!("ChatGPT authorization failed: {e}"));
    }
    from_query(&q, state)
}

/// OpenAI identifies each installation by `urn:uuid:<uuid>`.
fn agent_host_id(device_id: Option<&str>) -> Result<String, String> {
    let id = device_id.unwrap_or_default();
    let ok = id.len() == 36
        && id.chars().enumerate().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        });
    if !ok {
        return Err(
            "Sign in with ChatGPT requires a device ID (UUID) for this installation".into(),
        );
    }
    Ok(format!("urn:uuid:{}", id.to_ascii_lowercase()))
}

async fn request_token(
    ctx: &FlowCtx,
    pairs: &[(&str, &str)],
    cancel: Option<&tokio_util::sync::CancellationToken>,
) -> Result<Value, String> {
    let url = format!("{}/api/accounts/oauth/token", ctx.endpoints.openai_auth);
    let resp = send(post_form(&ctx.http, &url, pairs), cancel).await?;
    if !resp.ok() {
        return Err(format!(
            "OpenAI OAuth token request failed ({}): {}",
            resp.status, resp.text
        ));
    }
    match resp.json() {
        v @ Value::Object(_) => Ok(v),
        _ => Err("OpenAI OAuth token response must be an object".into()),
    }
}

fn credential_from_token(token: &Value, client_id: &str) -> Result<OAuthCredential, String> {
    let field = |k: &str| {
        super::json_str(token, k)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
            .ok_or_else(|| format!("OpenAI OAuth token response has invalid {k}"))
    };
    let access = field("access_token")?;
    let refresh = field("refresh_token")?;
    let scope = field("scope")?;
    let expires_in = super::positive_number(token, "expires_in")
        .ok_or("OpenAI OAuth token response has invalid expires_in")?;
    let scopes: Vec<String> = scope.split_whitespace().map(str::to_owned).collect();
    if !scopes.iter().any(|s| s == DIRECT_TOKEN_SCOPE) {
        return Err(format!(
            "OpenAI OAuth grant did not include {DIRECT_TOKEN_SCOPE}"
        ));
    }
    let mut extra = Map::new();
    extra.insert("clientId".into(), Value::String(client_id.to_owned()));
    extra.insert(
        "scopes".into(),
        Value::Array(scopes.into_iter().map(Value::String).collect()),
    );
    Ok(OAuthCredential {
        access,
        refresh,
        expires: super::now_ms() + (expires_in * 1000.0) as i64 - EXPIRY_MARGIN_MS,
        extra,
    })
}

async fn login(ctx: &FlowCtx, io: &Interaction) -> Result<OAuthCredential, String> {
    let host_id = agent_host_id(ctx.device_id.as_deref())?;
    let pkce = pkce();
    let state = b64url(&random_bytes(32));
    let nonce = b64url(&random_bytes(32));
    let port = ctx.endpoints.port(PORT);
    let validate_state = state.clone();
    let callback = CallbackServer::<Query>::start(
        CallbackOptions {
            provider_name: "ChatGPT".into(),
            host: ctx.endpoints.callback_host.clone(),
            port,
            path: CALLBACK_PATH.into(),
            redirect_host: Some("127.0.0.1".into()),
            state: None,
            timeout: None,
            validate: Some(Arc::new(move |q| {
                from_query(q, &validate_state).map(|_| ())
            })),
        },
        Arc::new(|q| Box::pin(async move { Ok(q) })),
        &io.cancel,
    )
    .await
    .map_err(|e| {
        if e.kind() == std::io::ErrorKind::AddrInUse {
            format!(
                "Port {port} is in use, probably by an unfinished login in another pi session or by the Codex CLI. Cancel that login and try again."
            )
        } else {
            e.to_string()
        }
    })?;
    let redirect_uri = callback.redirect_uri.clone();
    let url = with_query(
        &format!("{}/api/accounts/authorize", ctx.endpoints.openai_auth),
        &[
            ("client_id", DYNAMIC_CLIENT_ID),
            ("agent_name_hint", AGENT_NAME_HINT),
            ("ext_agent_host_id", &host_id),
            ("response_type", "code"),
            ("redirect_uri", &redirect_uri),
            ("resource", RESOURCE),
            (
                "scope",
                &format!(
                    "openid profile email offline_access resource.invoke {DIRECT_TOKEN_SCOPE}"
                ),
            ),
            ("state", &state),
            ("code_challenge", &pkce.challenge),
            ("code_challenge_method", "S256"),
            ("nonce", &nonce),
        ],
    )?;
    io.notify(AuthEvent::AuthUrl {
        url,
        instructions: Some(
            "Complete sign-in in your browser. If the callback does not complete, paste the final redirect URL here."
                .into(),
        ),
    });
    let withdraw = tokio_util::sync::CancellationToken::new();
    let manual = io.prompt_until(
        Prompt::manual_code(
            "Complete login in your browser, or paste the final redirect URL here:",
            &redirect_uri,
        ),
        &withdraw,
    );
    let result = tokio::select! {
        r = callback.wait() => match r {
            Some(Ok(q)) => from_query(&q, &state),
            Some(Err(e)) => Err(e),
            None => Err(CANCELLED.to_string()),
        },
        m = manual => m.and_then(|input| from_manual_input(&input, &redirect_uri, &state)),
    };
    withdraw.cancel();
    drop(callback);
    let result = match result {
        Err(_) if io.cancel.is_cancelled() => return Err(CANCELLED.into()),
        r => r?,
    };
    io.notify(AuthEvent::Progress(
        "Exchanging authorization code for tokens...".into(),
    ));
    let token = request_token(
        ctx,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", &result.client_id),
            ("code", &result.code),
            ("code_verifier", &pkce.verifier),
            ("redirect_uri", &redirect_uri),
            ("resource", RESOURCE),
        ],
        Some(&io.cancel),
    )
    .await?;
    if super::json_str(&token, "id_token").is_none_or(|t| t.trim().is_empty()) {
        return Err("OpenAI OAuth token response did not contain an ID token".into());
    }
    credential_from_token(&token, &result.client_id)
}

impl OAuthFlow for OpenAIChatGPT {
    fn name(&self) -> &str {
        "OpenAI (ChatGPT subscription)"
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
            let client_id = credential
                .extra
                .get("clientId")
                .and_then(Value::as_str)
                .filter(|c| !c.trim().is_empty())
                .ok_or("Stored OpenAI OAuth credential does not contain an issued client ID; reconnect ChatGPT")?
                .to_owned();
            let token = request_token(
                ctx,
                &[
                    ("grant_type", "refresh_token"),
                    ("client_id", &client_id),
                    ("refresh_token", &credential.refresh),
                    ("resource", RESOURCE),
                ],
                None,
            )
            .await?;
            credential_from_token(&token, &client_id)
        })
    }

    fn to_auth(&self, credential: &OAuthCredential) -> ModelAuth {
        ModelAuth {
            api_key: Some(credential.access.clone()),
            ..Default::default()
        }
    }
}
