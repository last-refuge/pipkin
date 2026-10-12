//! OAuth for HTTP MCP servers, on the engine's own primitives (loopback callback server, PKCE,
//! the sign-in interaction model), never on the SDK's `auth` feature (which needs its reqwest).
//!
//! Discovery follows the MCP authorization specification: the server's `WWW-Authenticate`
//! header may name its protected-resource metadata (RFC 9728); that names the authorization
//! server, whose metadata (RFC 8414 / OpenID discovery) gives the endpoints. Without a
//! configured client id the engine registers one dynamically (RFC 7591). Tokens are
//! requested for the server's canonical URL as the `resource` (RFC 8707) and stored per
//! server in the engine's private `mcp-auth.json`. Secrets never appear in views or logs.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::auth::oauth::callback::{
    CallbackOptions, CallbackOrManual, CallbackServer, wait_for_callback_or_manual,
};
use crate::auth::oauth::http::{post_form, post_json, send, with_query};
use crate::auth::oauth::pkce::{b64url, pkce, random_bytes};
use crate::auth::oauth::{AuthEvent, FlowCtx, Interaction, ModelAuth, OAuthCredential, OAuthFlow};
use crate::provider::BoxFuture;

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const CLIENT_NAME: &str = "Pipkin";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discovered {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
    /// Scopes the resource asks for, when it says.
    pub scopes: Vec<String>,
    /// The canonical resource identifier sent as `resource`.
    pub resource: String,
}

fn origin(url: &url::Url) -> String {
    let mut o = format!("{}://{}", url.scheme(), url.host_str().unwrap_or(""));
    if let Some(port) = url.port() {
        o.push_str(&format!(":{port}"));
    }
    o
}

/// RFC 8707 canonical form: scheme and host lowercase, no fragment, no trailing slash on an
/// empty path.
pub fn canonical_resource(server_url: &str) -> String {
    let Ok(mut u) = url::Url::parse(server_url) else {
        return server_url.to_owned();
    };
    u.set_fragment(None);
    let mut s = u.to_string();
    if u.path() == "/" && u.query().is_none() {
        s = s.trim_end_matches('/').to_owned();
    }
    s
}

/// Parameters of a `Bearer` challenge: `resource_metadata`, `scope`, `error`.
pub fn challenge_param(header: &str, key: &str) -> Option<String> {
    header.split(',').map(str::trim).find_map(|part| {
        let part = part.strip_prefix("Bearer ").unwrap_or(part).trim();
        let (k, v) = part.split_once('=')?;
        (k.trim() == key).then(|| v.trim().trim_matches('"').to_owned())
    })
}

async fn get_json(
    http: &reqwest::Client,
    url: &str,
    cancel: Option<&CancellationToken>,
) -> Option<Value> {
    let resp = send(
        http.get(url)
            .header("accept", "application/json")
            .header("mcp-protocol-version", "2025-06-18")
            .timeout(DISCOVERY_TIMEOUT),
        cancel,
    )
    .await
    .ok()?;
    if !resp.ok() {
        return None;
    }
    let value = resp.json();
    value.is_object().then_some(value)
}

fn str_of(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Find the authorization server and its endpoints for `server_url`. `www_authenticate` is
/// the server's challenge when one was seen (it may name the metadata URL and scopes).
pub async fn discover(
    http: &reqwest::Client,
    server_url: &str,
    www_authenticate: Option<&str>,
    cancel: Option<&CancellationToken>,
) -> Result<Discovered, String> {
    let parsed = url::Url::parse(server_url).map_err(|e| format!("server URL: {e}"))?;
    let server_origin = origin(&parsed);
    let path = parsed.path().trim_end_matches('/');
    // Protected resource metadata (RFC 9728).
    let mut candidates = Vec::new();
    if let Some(h) = www_authenticate
        && let Some(url) = challenge_param(h, "resource_metadata")
    {
        candidates.push(url);
    }
    if !path.is_empty() {
        candidates.push(format!(
            "{server_origin}/.well-known/oauth-protected-resource{path}"
        ));
    }
    candidates.push(format!(
        "{server_origin}/.well-known/oauth-protected-resource"
    ));
    let mut issuer = None;
    let mut resource_scopes = Vec::new();
    let mut resource = canonical_resource(server_url);
    for url in candidates {
        if let Some(meta) = get_json(http, &url, cancel).await {
            if let Some(servers) = meta.get("authorization_servers").and_then(Value::as_array)
                && let Some(first) = servers.iter().find_map(|s| s.as_str())
            {
                issuer = Some(first.trim_end_matches('/').to_owned());
            }
            if let Some(scopes) = meta.get("scopes_supported").and_then(Value::as_array) {
                resource_scopes = scopes
                    .iter()
                    .filter_map(|s| s.as_str().map(str::to_owned))
                    .collect();
            }
            if let Some(r) = str_of(&meta, "resource") {
                resource = r;
            }
            break;
        }
    }
    let issuer = issuer.unwrap_or_else(|| server_origin.clone());
    // Authorization server metadata (RFC 8414, then OpenID discovery), else the defaults.
    let issuer_url =
        url::Url::parse(&issuer).map_err(|e| format!("authorization server URL: {e}"))?;
    let issuer_origin = origin(&issuer_url);
    let issuer_path = issuer_url.path().trim_end_matches('/').to_owned();
    let mut meta_urls = Vec::new();
    if !issuer_path.is_empty() {
        meta_urls.push(format!(
            "{issuer_origin}/.well-known/oauth-authorization-server{issuer_path}"
        ));
    }
    meta_urls.push(format!(
        "{issuer_origin}/.well-known/oauth-authorization-server"
    ));
    if !issuer_path.is_empty() {
        meta_urls.push(format!(
            "{issuer_origin}/.well-known/openid-configuration{issuer_path}"
        ));
        meta_urls.push(format!("{issuer}/.well-known/openid-configuration"));
    } else {
        meta_urls.push(format!("{issuer_origin}/.well-known/openid-configuration"));
    }
    let mut found = None;
    for url in meta_urls {
        if let Some(meta) = get_json(http, &url, cancel).await
            && let (Some(a), Some(t)) = (
                str_of(&meta, "authorization_endpoint"),
                str_of(&meta, "token_endpoint"),
            )
        {
            found = Some((a, t, str_of(&meta, "registration_endpoint")));
            break;
        }
    }
    let (authorization_endpoint, token_endpoint, registration_endpoint) = found.unwrap_or((
        format!("{issuer}/authorize"),
        format!("{issuer}/token"),
        Some(format!("{issuer}/register")),
    ));
    let mut scopes = www_authenticate
        .and_then(|h| challenge_param(h, "scope"))
        .map(|s| s.split_whitespace().map(str::to_owned).collect::<Vec<_>>())
        .unwrap_or_default();
    if scopes.is_empty() {
        scopes = resource_scopes;
    }
    Ok(Discovered {
        issuer,
        authorization_endpoint,
        token_endpoint,
        registration_endpoint,
        scopes,
        resource,
    })
}

/// Dynamic client registration (RFC 7591). Returns the client id and secret, if any.
pub async fn register(
    http: &reqwest::Client,
    endpoint: &str,
    redirect_uri: &str,
    cancel: Option<&CancellationToken>,
) -> Result<(String, Option<String>), String> {
    let body = json!({
        "client_name": CLIENT_NAME,
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
        "application_type": "native",
    });
    let resp = send(
        post_json(http, endpoint, &body)
            .header("accept", "application/json")
            .timeout(DISCOVERY_TIMEOUT),
        cancel,
    )
    .await
    .map_err(|e| format!("client registration failed: {e}"))?;
    if !resp.ok() {
        return Err(format!(
            "client registration was refused (HTTP {})",
            resp.status
        ));
    }
    let value = resp.json();
    let id = str_of(&value, "client_id").ok_or("client registration returned no client_id")?;
    Ok((id, str_of(&value, "client_secret")))
}

/// One HTTP server's sign-in: PKCE authorization code through the loopback callback server.
pub struct McpFlow {
    pub server: String,
    pub discovered: Discovered,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub scopes: Vec<String>,
    /// A redirect URI the client was registered with; the login listens on its port again.
    pub redirect_uri: Option<String>,
}

fn credential_from(
    value: &Value,
    flow_extra: Map<String, Value>,
    previous_refresh: Option<&str>,
) -> Result<OAuthCredential, String> {
    let access = str_of(value, "access_token").ok_or("the token response has no access_token")?;
    let refresh = str_of(value, "refresh_token")
        .or_else(|| previous_refresh.map(str::to_owned))
        .unwrap_or_default();
    let lifetime = value
        .get("expires_in")
        .and_then(Value::as_f64)
        .filter(|s| *s > 0.0)
        .unwrap_or(3600.0);
    let expires = crate::util::now_ms() + (lifetime * 1000.0) as i64;
    let mut extra = flow_extra;
    if let Some(scope) = str_of(value, "scope") {
        extra.insert("scope".into(), Value::String(scope));
    }
    Ok(OAuthCredential {
        access,
        refresh,
        expires,
        extra,
    })
}

impl McpFlow {
    fn extra(&self) -> Map<String, Value> {
        let mut extra = Map::new();
        extra.insert(
            "tokenEndpoint".into(),
            json!(self.discovered.token_endpoint),
        );
        extra.insert("clientId".into(), json!(self.client_id));
        if let Some(secret) = &self.client_secret {
            extra.insert("clientSecret".into(), json!(secret));
        }
        extra.insert("resource".into(), json!(self.discovered.resource));
        extra.insert("issuer".into(), json!(self.discovered.issuer));
        extra
    }

    /// From a stored credential (for refresh): the endpoints travel with the tokens.
    pub fn from_credential(server: &str, credential: &OAuthCredential) -> Option<McpFlow> {
        let token_endpoint = str_of(&Value::Object(credential.extra.clone()), "tokenEndpoint")?;
        let client_id = str_of(&Value::Object(credential.extra.clone()), "clientId")?;
        let resource = str_of(&Value::Object(credential.extra.clone()), "resource")?;
        let issuer = str_of(&Value::Object(credential.extra.clone()), "issuer").unwrap_or_default();
        Some(McpFlow {
            server: server.to_owned(),
            discovered: Discovered {
                issuer,
                authorization_endpoint: String::new(),
                token_endpoint,
                registration_endpoint: None,
                scopes: Vec::new(),
                resource,
            },
            client_id,
            client_secret: str_of(&Value::Object(credential.extra.clone()), "clientSecret"),
            scopes: Vec::new(),
            redirect_uri: None,
        })
    }

    async fn exchange(
        &self,
        http: reqwest::Client,
        code: String,
        verifier: String,
        redirect_uri: String,
        cancel: CancellationToken,
    ) -> Result<OAuthCredential, String> {
        let mut pairs: Vec<(&str, &str)> = vec![
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &redirect_uri),
            ("client_id", &self.client_id),
            ("code_verifier", &verifier),
            ("resource", &self.discovered.resource),
        ];
        if let Some(secret) = &self.client_secret {
            pairs.push(("client_secret", secret));
        }
        let resp = send(
            post_form(&http, &self.discovered.token_endpoint, &pairs)
                .header("accept", "application/json")
                .timeout(Duration::from_secs(30)),
            Some(&cancel),
        )
        .await
        .map_err(|_| "the token request failed".to_owned())?;
        // Token endpoints may echo secrets: never retain their body in an error.
        if !resp.ok() {
            return Err(format!("the token request failed (HTTP {})", resp.status));
        }
        credential_from(&resp.json(), self.extra(), None)
    }
}

/// The code from a pasted redirect URL, `code=...` query, or a bare code; and the state.
fn parse_manual(input: &str) -> (Option<String>, Option<String>) {
    let input = input.trim();
    let query = input
        .split_once('?')
        .map(|(_, q)| q)
        .or_else(|| input.contains('=').then_some(input));
    if let Some(q) = query {
        let q = q.split('#').next().unwrap_or(q);
        let mut code = None;
        let mut state = None;
        for (k, v) in url::form_urlencoded::parse(q.as_bytes()) {
            match &*k {
                "code" => code = Some(v.into_owned()),
                "state" => state = Some(v.into_owned()),
                _ => {}
            }
        }
        return (code, state);
    }
    (Some(input.to_owned()).filter(|s| !s.is_empty()), None)
}

impl OAuthFlow for McpFlow {
    fn name(&self) -> &str {
        &self.server
    }

    fn login<'a>(
        &'a self,
        ctx: &'a FlowCtx,
        io: &'a Interaction,
    ) -> BoxFuture<'a, Result<OAuthCredential, String>> {
        Box::pin(async move {
            let pk = pkce();
            let state = b64url(&random_bytes(16));
            let (http, verifier, cancel) =
                (ctx.http.clone(), pk.verifier.clone(), io.cancel.clone());
            // A registered redirect URI pins the port and path; otherwise any free port.
            let (port, path) = self
                .redirect_uri
                .as_deref()
                .and_then(|r| url::Url::parse(r).ok())
                .map(|u| (u.port().unwrap_or(0), u.path().to_owned()))
                .unwrap_or_else(|| (0, format!("/mcp/callback/{}", crate::util::uuid_v4())));
            let this: &McpFlow = self;
            let flow = Arc::new(ThinFlow {
                discovered: this.discovered.clone(),
                client_id: this.client_id.clone(),
                client_secret: this.client_secret.clone(),
                server: this.server.clone(),
                redirect: std::sync::Mutex::new(None),
            });
            let callback = CallbackServer::<OAuthCredential>::start(
                CallbackOptions {
                    provider_name: self.server.clone(),
                    host: ctx.endpoints.callback_host.clone(),
                    port,
                    path,
                    redirect_host: None,
                    state: Some(state.clone()),
                    timeout: Some(LOGIN_TIMEOUT),
                    validate: None,
                },
                {
                    let flow = flow.clone();
                    let http = http.clone();
                    let verifier = verifier.clone();
                    let cancel = cancel.clone();
                    Arc::new(move |q| {
                        let code = q.get("code").cloned().unwrap_or_default();
                        let flow = flow.clone();
                        let http = http.clone();
                        let verifier = verifier.clone();
                        let cancel = cancel.clone();
                        Box::pin(async move {
                            // The redirect URI the browser used is the server's own.
                            flow.exchange(http, code, verifier, cancel).await
                        })
                    })
                },
                &io.cancel,
            )
            .await
            .map_err(|e| format!("cannot listen for the sign-in callback: {e}"))?;
            let redirect = callback.redirect_uri.clone();
            flow.redirect
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .replace(redirect.clone());
            let scope = if self.scopes.is_empty() {
                self.discovered.scopes.join(" ")
            } else {
                self.scopes.join(" ")
            };
            let mut params: Vec<(&str, &str)> = vec![
                ("response_type", "code"),
                ("client_id", &self.client_id),
                ("redirect_uri", &redirect),
                ("code_challenge", &pk.challenge),
                ("code_challenge_method", "S256"),
                ("state", &state),
                ("resource", &self.discovered.resource),
            ];
            if !scope.is_empty() {
                params.push(("scope", &scope));
            }
            let url = with_query(&self.discovered.authorization_endpoint, &params)?;
            io.notify(AuthEvent::Progress(format!(
                "Listening for the sign-in callback on {redirect}"
            )));
            io.notify(AuthEvent::AuthUrl {
                url,
                instructions: Some(format!(
                    "Sign in to {} in your browser. If the browser is on another machine, paste the final redirect URL here.",
                    self.server
                )),
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
                    let (code, got_state) = parse_manual(&input);
                    let code = code.ok_or("Missing authorization code")?;
                    if let Some(s) = got_state
                        && s != state
                    {
                        return Err("The pasted redirect belongs to another sign-in".into());
                    }
                    io.notify(AuthEvent::Progress(
                        "Exchanging the authorization code for tokens...".into(),
                    ));
                    self.exchange(http, code, verifier, redirect, cancel).await
                }
            }
        })
    }

    fn refresh<'a>(
        &'a self,
        ctx: &'a FlowCtx,
        credential: &'a OAuthCredential,
    ) -> BoxFuture<'a, Result<OAuthCredential, String>> {
        Box::pin(async move {
            if credential.refresh.is_empty() {
                return Err("no refresh token".into());
            }
            let mut pairs: Vec<(&str, &str)> = vec![
                ("grant_type", "refresh_token"),
                ("refresh_token", &credential.refresh),
                ("client_id", &self.client_id),
                ("resource", &self.discovered.resource),
            ];
            if let Some(secret) = &self.client_secret {
                pairs.push(("client_secret", secret));
            }
            let resp = send(
                post_form(&ctx.http, &self.discovered.token_endpoint, &pairs)
                    .header("accept", "application/json")
                    .timeout(Duration::from_secs(30)),
                None,
            )
            .await
            .map_err(|_| "the token refresh failed".to_owned())?;
            if !resp.ok() {
                return Err(format!("the token refresh failed (HTTP {})", resp.status));
            }
            let mut extra = credential.extra.clone();
            extra.extend(self.extra());
            credential_from(&resp.json(), extra, Some(&credential.refresh))
        })
    }

    fn to_auth(&self, credential: &OAuthCredential) -> ModelAuth {
        ModelAuth {
            api_key: Some(credential.access.clone()),
            ..Default::default()
        }
    }
}

/// The exchange needs the redirect URI the callback server chose, which is only known after
/// it starts; this shares it with the completion closure.
struct ThinFlow {
    discovered: Discovered,
    client_id: String,
    client_secret: Option<String>,
    server: String,
    redirect: std::sync::Mutex<Option<String>>,
}

impl ThinFlow {
    async fn exchange(
        &self,
        http: reqwest::Client,
        code: String,
        verifier: String,
        cancel: CancellationToken,
    ) -> Result<OAuthCredential, String> {
        let redirect = self
            .redirect
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or_default();
        let flow = McpFlow {
            server: self.server.clone(),
            discovered: self.discovered.clone(),
            client_id: self.client_id.clone(),
            client_secret: self.client_secret.clone(),
            scopes: Vec::new(),
            redirect_uri: None,
        };
        flow.exchange(http, code, verifier, redirect, cancel).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_params_resources_and_manual_input_parse() {
        let h = r#"Bearer realm="mcp", resource_metadata="https://h/.well-known/oauth-protected-resource/mcp", scope="read write""#;
        assert_eq!(
            challenge_param(h, "resource_metadata").as_deref(),
            Some("https://h/.well-known/oauth-protected-resource/mcp")
        );
        assert_eq!(challenge_param(h, "scope").as_deref(), Some("read write"));
        assert_eq!(challenge_param(h, "nope"), None);
        assert_eq!(
            canonical_resource("https://H.example/"),
            "https://h.example"
        );
        assert_eq!(
            canonical_resource("https://h.example/mcp#x"),
            "https://h.example/mcp"
        );
        assert_eq!(
            parse_manual("http://127.0.0.1:5/cb?code=abc&state=s#frag"),
            (Some("abc".into()), Some("s".into()))
        );
        assert_eq!(parse_manual("code=xyz"), (Some("xyz".into()), None));
        assert_eq!(parse_manual("  plain  "), (Some("plain".into()), None));
        assert_eq!(parse_manual(""), (None, None));
        let cred = credential_from(
            &json!({"access_token": "a", "refresh_token": "r", "expires_in": 60, "scope": "read"}),
            Map::new(),
            None,
        )
        .unwrap();
        assert_eq!((cred.access.as_str(), cred.refresh.as_str()), ("a", "r"));
        assert!(cred.expires > crate::util::now_ms() + 50_000);
        assert_eq!(cred.extra["scope"], "read");
        let kept = credential_from(&json!({"access_token": "b"}), Map::new(), Some("old")).unwrap();
        assert_eq!(kept.refresh, "old");
        assert!(credential_from(&json!({}), Map::new(), None).is_err());
    }
}
