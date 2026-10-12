//! OAuth flows for the built-in providers (reference `packages/ai/src/auth/oauth/*`).
//!
//! A flow implements [`OAuthFlow`]: `login` drives an [`Interaction`] (prompts answered by the
//! user through the sign-in lifecycle, notifications for URLs and device codes), `refresh`
//! exchanges the refresh token, `to_auth` derives request auth from a valid credential.
//!
//! Every endpoint is overridable through the engine environment ([`Endpoints::from_env`]) so the
//! flows are tested against a local mock server; production uses the reference endpoints.
//!
//! | Variable | Default | Used by |
//! |---|---|---|
//! | `PI_OAUTH_CALLBACK_HOST` | `127.0.0.1` | loopback host (Codex, ChatGPT, OpenRouter) |
//! | `PIPKIN_OAUTH_CALLBACK_PORT` | flow's fixed port (1455, 1456) | loopback port; `0` = any free port |
//! | `PIPKIN_OAUTH_ANTHROPIC_AUTHORIZE_URL` | `https://claude.ai` | Claude authorization |
//! | `PIPKIN_OAUTH_ANTHROPIC_PLATFORM_URL` | `https://platform.claude.com` | Claude token exchange and copy-code callback |
//! | `PIPKIN_OAUTH_OPENAI_URL` | `https://auth.openai.com` | Codex, Sign in with ChatGPT |
//! | `PIPKIN_OAUTH_GITHUB_URL` | `https://<domain>` | Copilot device + token endpoints |
//! | `PIPKIN_OAUTH_GITHUB_API_URL` | `https://api.<domain>` | Copilot token exchange |
//! | `PIPKIN_OAUTH_COPILOT_API_URL` | from the token's `proxy-ep` | Copilot `/models`, policy |
//! | `PIPKIN_OAUTH_OPENROUTER_URL` | `https://openrouter.ai` | OpenRouter |
//! | `KIMI_CODE_OAUTH_HOST` / `KIMI_OAUTH_HOST` | `https://auth.kimi.com` | Kimi Code |
//! | `PIPKIN_OAUTH_META_AUTH_URL` | `https://auth.meta.com` | Meta device flow |
//! | `PIPKIN_OAUTH_META_API_URL` | `https://api.meta.ai` | Meta key mint |
//! | `PI_RADIUS_GATEWAY` | `https://radius.pi.dev` | built-in Radius provider |
//! | `PIPKIN_OAUTH_XAI_URL` | `https://auth.x.ai` | xAI |
//! | `PIPKIN_OAUTH_POLL_INTERVAL_MS` | server interval | fixed device-code poll interval |
//! | `PIPKIN_OAUTH_RETRY_BASE_MS` | reference delays | base of refresh/429 retry delays |

mod anthropic;
pub mod callback;
pub mod device;
mod github_copilot;
pub mod http;
mod kimi_coding;
mod meta;
mod openai_chatgpt;
mod openai_codex;
mod openrouter;
pub mod pkce;
mod radius;
mod xai;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::config::Env;
use crate::provider::BoxFuture;

pub use crate::auth::store::OAuthCredential;

pub const CANCELLED: &str = "Login cancelled";

/// What a prompt asks for. Wire names follow the reference (`text|secret|select|manual_code`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    Text,
    Secret,
    Select,
    ManualCode,
}

impl PromptKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PromptKind::Text => "text",
            PromptKind::Secret => "secret",
            PromptKind::Select => "select",
            PromptKind::ManualCode => "manual_code",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub kind: PromptKind,
    pub message: String,
    pub placeholder: Option<String>,
    /// `(id, label)` for `select`.
    pub options: Vec<(String, String)>,
}

impl Prompt {
    pub fn text(message: &str, placeholder: Option<&str>) -> Prompt {
        Prompt {
            kind: PromptKind::Text,
            message: message.into(),
            placeholder: placeholder.map(str::to_owned),
            options: vec![],
        }
    }

    pub fn manual_code(message: &str, placeholder: &str) -> Prompt {
        Prompt {
            kind: PromptKind::ManualCode,
            message: message.into(),
            placeholder: Some(placeholder.into()),
            options: vec![],
        }
    }

    pub fn select(message: &str, options: &[(&str, &str)]) -> Prompt {
        Prompt {
            kind: PromptKind::Select,
            message: message.into(),
            placeholder: None,
            options: options
                .iter()
                .map(|(i, l)| (i.to_string(), l.to_string()))
                .collect(),
        }
    }
}

/// Progress a flow reports. URLs and codes are volatile UI state, never logged.
#[derive(Clone, PartialEq, Eq)]
pub enum AuthEvent {
    Info(String),
    AuthUrl {
        url: String,
        instructions: Option<String>,
    },
    DeviceCode {
        user_code: String,
        verification_uri: String,
    },
    Progress(String),
}

impl std::fmt::Debug for AuthEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            AuthEvent::Info(_) => "Info",
            AuthEvent::AuthUrl { .. } => "AuthUrl",
            AuthEvent::DeviceCode { .. } => "DeviceCode",
            AuthEvent::Progress(_) => "Progress",
        })
    }
}

/// Where prompts and notifications go (the sign-in lifecycle, or a test script).
pub trait InteractionSink: Send + Sync {
    /// Resolves with the answer. Dropping the future withdraws the prompt.
    fn prompt(&self, prompt: Prompt) -> BoxFuture<'static, Result<String, String>>;
    fn notify(&self, event: AuthEvent);
}

/// A login's interaction channel plus its cancellation.
#[derive(Clone)]
pub struct Interaction {
    pub cancel: CancellationToken,
    pub sink: Arc<dyn InteractionSink>,
}

impl Interaction {
    pub fn new(sink: Arc<dyn InteractionSink>, cancel: CancellationToken) -> Interaction {
        Interaction { cancel, sink }
    }

    pub fn check(&self) -> Result<(), String> {
        if self.cancel.is_cancelled() {
            Err(CANCELLED.into())
        } else {
            Ok(())
        }
    }

    pub async fn prompt(&self, prompt: Prompt) -> Result<String, String> {
        self.prompt_until(prompt, &CancellationToken::new()).await
    }

    /// A prompt that is withdrawn when `withdraw` fires (e.g. the callback won the race).
    pub async fn prompt_until(
        &self,
        prompt: Prompt,
        withdraw: &CancellationToken,
    ) -> Result<String, String> {
        self.check()?;
        let fut = self.sink.prompt(prompt);
        tokio::select! {
            r = fut => r,
            _ = self.cancel.cancelled() => Err(CANCELLED.into()),
            _ = withdraw.cancelled() => Err(CANCELLED.into()),
        }
    }

    pub fn notify(&self, event: AuthEvent) {
        if !self.cancel.is_cancelled() {
            self.sink.notify(event);
        }
    }
}

/// Request auth derived from a credential.
#[derive(Clone, Default, PartialEq)]
pub struct ModelAuth {
    pub api_key: Option<String>,
    /// `None` deletes a default header.
    pub headers: BTreeMap<String, Option<String>>,
    pub base_url: Option<String>,
}

impl std::fmt::Debug for ModelAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelAuth")
            .field("api_key", &self.api_key.as_ref().map(|_| "[redacted]"))
            .field("headers", &self.headers.keys().collect::<Vec<_>>())
            .field("base_url", &self.base_url)
            .finish()
    }
}

/// Endpoint and timing configuration (see the module table).
#[derive(Clone, Debug)]
pub struct Endpoints {
    pub callback_host: String,
    pub callback_port: Option<u16>,
    pub anthropic_authorize: String,
    pub anthropic_platform: String,
    pub openai_auth: String,
    pub github: Option<String>,
    pub github_api: Option<String>,
    pub copilot_api: Option<String>,
    pub openrouter: String,
    pub kimi: String,
    pub meta_auth: String,
    pub meta_api: String,
    pub radius_gateway: String,
    pub xai: String,
    pub poll_interval: Option<Duration>,
    pub retry_base: Option<Duration>,
}

fn trim_url(s: String) -> String {
    s.trim_end_matches('/').to_owned()
}

impl Endpoints {
    pub fn from_env(env: &Env) -> Endpoints {
        let get = |k: &str| env.get(k).map(trim_url);
        let ms = |k: &str| {
            env.get(k)
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map(Duration::from_millis)
        };
        Endpoints {
            callback_host: env
                .get("PI_OAUTH_CALLBACK_HOST")
                .unwrap_or_else(|| "127.0.0.1".into()),
            callback_port: env
                .get("PIPKIN_OAUTH_CALLBACK_PORT")
                .and_then(|p| p.trim().parse().ok()),
            anthropic_authorize: get("PIPKIN_OAUTH_ANTHROPIC_AUTHORIZE_URL")
                .unwrap_or_else(|| "https://claude.ai".into()),
            anthropic_platform: get("PIPKIN_OAUTH_ANTHROPIC_PLATFORM_URL")
                .unwrap_or_else(|| "https://platform.claude.com".into()),
            openai_auth: get("PIPKIN_OAUTH_OPENAI_URL")
                .unwrap_or_else(|| "https://auth.openai.com".into()),
            github: get("PIPKIN_OAUTH_GITHUB_URL"),
            github_api: get("PIPKIN_OAUTH_GITHUB_API_URL"),
            copilot_api: get("PIPKIN_OAUTH_COPILOT_API_URL"),
            openrouter: get("PIPKIN_OAUTH_OPENROUTER_URL")
                .unwrap_or_else(|| "https://openrouter.ai".into()),
            kimi: get("KIMI_CODE_OAUTH_HOST")
                .or_else(|| get("KIMI_OAUTH_HOST"))
                .unwrap_or_else(|| "https://auth.kimi.com".into()),
            meta_auth: get("PIPKIN_OAUTH_META_AUTH_URL")
                .unwrap_or_else(|| "https://auth.meta.com".into()),
            meta_api: get("PIPKIN_OAUTH_META_API_URL")
                .unwrap_or_else(|| "https://api.meta.ai".into()),
            radius_gateway: crate::auth::models_json::normalize_gateway(
                &env.get("PI_RADIUS_GATEWAY")
                    .unwrap_or_else(|| crate::auth::models_json::DEFAULT_RADIUS_GATEWAY.into()),
            ),
            xai: get("PIPKIN_OAUTH_XAI_URL").unwrap_or_else(|| "https://auth.x.ai".into()),
            poll_interval: ms("PIPKIN_OAUTH_POLL_INTERVAL_MS"),
            retry_base: ms("PIPKIN_OAUTH_RETRY_BASE_MS"),
        }
    }

    /// The loopback port: the override, else the flow's fixed port.
    pub fn port(&self, fixed: u16) -> u16 {
        self.callback_port.unwrap_or(fixed)
    }

    /// A reference retry delay, scaled by the configured base (tests).
    pub fn retry_delay(&self, reference: Duration, attempt_factor: u32) -> Duration {
        match self.retry_base {
            Some(base) => base * attempt_factor,
            None => reference,
        }
    }
}

/// Everything a flow needs besides the interaction.
#[derive(Clone)]
pub struct FlowCtx {
    pub http: reqwest::Client,
    pub endpoints: Endpoints,
    /// Stable installation id (UUID) for flows that identify the agent host.
    pub device_id: Option<String>,
    /// Built-in GitHub Copilot model ids (policy enablement only applies to these).
    pub copilot_model_ids: Vec<String>,
}

/// One provider's OAuth implementation.
pub trait OAuthFlow: Send + Sync {
    /// Display name, e.g. "GitHub Copilot".
    fn name(&self) -> &str;
    fn login<'a>(
        &'a self,
        ctx: &'a FlowCtx,
        io: &'a Interaction,
    ) -> BoxFuture<'a, Result<OAuthCredential, String>>;
    /// Network call; the caller bounds it with a timeout and runs it under the store lock.
    fn refresh<'a>(
        &'a self,
        ctx: &'a FlowCtx,
        credential: &'a OAuthCredential,
    ) -> BoxFuture<'a, Result<OAuthCredential, String>>;
    /// Side-effect free.
    fn to_auth(&self, credential: &OAuthCredential) -> ModelAuth;
}

/// The built-in OAuth providers in display order, with names.
pub const BUILTIN_OAUTH_PROVIDERS: &[(&str, &str)] = &[
    ("anthropic", "Anthropic (Claude Pro/Max)"),
    ("openai-codex", "OpenAI (ChatGPT Plus/Pro)"),
    ("openai", "OpenAI (ChatGPT subscription)"),
    ("github-copilot", "GitHub Copilot"),
    ("openrouter", "OpenRouter"),
    ("kimi-coding", "Kimi Code (subscription)"),
    ("meta", "Meta (Muse subscription)"),
    ("radius", "Radius"),
    ("xai", "xAI (Grok/X subscription)"),
];

/// The flow for `provider`: built-ins, or a Radius gateway (`radius`, or a `models.json`
/// provider with `"oauth": "radius"` and `gateway`).
pub fn flow_for(
    provider: &str,
    endpoints: &Endpoints,
    radius: Option<(String, String)>,
) -> Option<Arc<dyn OAuthFlow>> {
    if let Some((name, gateway)) = radius {
        return Some(Arc::new(radius::Radius::new(name, gateway)));
    }
    Some(match provider {
        "anthropic" => Arc::new(anthropic::Anthropic),
        "openai-codex" => Arc::new(openai_codex::OpenAICodex),
        "openai" => Arc::new(openai_chatgpt::OpenAIChatGPT),
        "github-copilot" => Arc::new(github_copilot::GitHubCopilot),
        "openrouter" => Arc::new(openrouter::OpenRouter),
        "kimi-coding" => Arc::new(kimi_coding::KimiCoding),
        "meta" => Arc::new(meta::Meta),
        "xai" => Arc::new(xai::Xai),
        "radius" => Arc::new(radius::Radius::new(
            "Radius".into(),
            endpoints.radius_gateway.clone(),
        )),
        _ => return None,
    })
}

/// GitHub Copilot model filter from a stored credential's `availableModelIds`.
pub fn copilot_available_models(credential: &OAuthCredential) -> Option<Vec<String>> {
    github_copilot::available_model_ids(credential)
}

pub(crate) fn now_ms() -> i64 {
    crate::util::now_ms()
}

/// Small helpers shared by flow modules.
pub(crate) fn json_str<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(serde_json::Value::as_str)
}

pub(crate) fn positive_number(v: &serde_json::Value, key: &str) -> Option<f64> {
    v.get(key)
        .and_then(serde_json::Value::as_f64)
        .filter(|n| n.is_finite() && *n > 0.0)
}

/// Only http(s) URLs may be shown to the user as links to open.
pub(crate) fn trusted_http_url(value: Option<&str>) -> Option<String> {
    let url = url::Url::parse(value?).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.to_string())
}
