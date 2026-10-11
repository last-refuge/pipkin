//! Credentials, provider configuration and sign-in.
//!
//! - **Catalog** ([`Auth::reload`]): built-in models + `models.json` providers/models/overrides
//!   (+ test extras). Configuration problems are warnings, never failures. A stored GitHub
//!   Copilot credential's `availableModelIds` filters that provider's models.
//! - **Resolution** ([`Auth::resolve`]): per request, in reference precedence: stored
//!   credential (`auth.json`), `models.json` `apiKey`, ambient environment ([`EngineConfig::env`]
//!   only, so tests are isolated). Then `models.json` provider headers, `authHeader`, and
//!   per-model headers. OAuth refresh is serialized per credential (in-process mutex + the
//!   store's file lock), bounded by 15 s, persisted atomically, and a failed refresh is an error
//!   that never falls back to a different credential.
//! - **Availability** ([`Auth::availability`]): metadata only (no network, no refresh, no
//!   command execution; a configured `!command` counts as available).
//! - **Sign-in lifecycle** (`start`/`reuse`/`answer`/`cancel`/`remove`/`snapshot`): per-client
//!   attempts with challenges and stale-attempt guards; volatile, never persisted or logged.
//!
//! The Claude subscription sign-in and its request path are **reserved** for a later
//! integration: the provider is listed with `available: false`, starting it is refused with
//! `Unsupported`, and a stored subscription credential makes affected models unavailable
//! rather than silently using an API key. Ordinary Anthropic API keys work normally.
//!
//! Storage: `auth.json` via [`store::FileStore`] (0600 file, 0700 dir, atomic writes, `flock`).
//! An OS secret store (Secret Service, Keychain) is not yet qualified; it would implement
//! [`store::CredentialStore`].

pub mod config_value;
pub mod models_json;
pub mod oauth;
pub mod providers;
mod signin;
pub mod store;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use pipkin_engine_protocol::{AuthSnapshot, ProtoError};
use serde_json::Value;

use crate::config::EngineConfig;
use crate::provider::{BoxFuture, Credential, Model};

use models_json::{ModelsConfig, ProviderConfig};
use oauth::{Endpoints, FlowCtx, OAuthFlow};
use providers::{KeyAuth, KeyInput, RESERVED_MESSAGE};
use store::{CredentialStore, FileStore, OAuthCredential, StoredCredential};

/// Why a model cannot be used right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthError {
    /// No credential or configuration for this provider.
    NotConfigured(String),
    /// The credential kind or provider path is not implemented in this engine (reserved).
    Unavailable(String),
    /// Configured, but resolving failed (expired and refresh failed, command failed, ...).
    Failed(String),
}

impl AuthError {
    pub fn message(&self) -> &str {
        match self {
            AuthError::NotConfigured(m) | AuthError::Unavailable(m) | AuthError::Failed(m) => m,
        }
    }
}

/// Everything resolved for one request.
#[derive(Clone, Debug)]
pub struct ResolvedAuth {
    pub credential: Credential,
    pub headers: BTreeMap<String, Option<String>>,
    pub base_url: Option<String>,
    pub env: HashMap<String, String>,
}

/// The effective model catalog.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub models: Vec<Model>,
    /// Problems found while loading configuration (said once by clients).
    pub warnings: Vec<String>,
}

impl Catalog {
    pub fn find(&self, key: &str) -> Option<&Model> {
        let (provider, id) = key.split_once('/')?;
        self.models
            .iter()
            .find(|m| m.provider == provider && m.id == id)
    }
}

/// Called with `(client, snapshot)` when a client's sign-in state changes.
pub type AuthListener = Arc<dyn Fn(u64, AuthSnapshot) + Send + Sync>;

/// OAuth tokens are refreshed when they expire within this margin (reference: 5 minutes).
pub const OAUTH_VALIDITY_MARGIN_MS: i64 = 5 * 60 * 1000;
/// Bound on one refresh call (reference: 15 s).
pub const OAUTH_REFRESH_TIMEOUT: Duration = Duration::from_secs(15);

pub struct Auth {
    inner: Arc<Inner>,
}

pub(crate) struct Inner {
    config: EngineConfig,
    store: Arc<dyn CredentialStore>,
    endpoints: Endpoints,
    http: reqwest::Client,
    catalog: RwLock<Arc<Catalog>>,
    models: RwLock<Arc<ModelsConfig>>,
    copilot_ids: Vec<String>,
    refresh_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    refresh_timeout: Mutex<Duration>,
    signin: Mutex<signin::State>,
    listener: Mutex<Option<AuthListener>>,
}

impl Drop for Auth {
    fn drop(&mut self) {
        self.inner.cancel_all_attempts();
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn not_configured(provider: &str) -> AuthError {
    AuthError::NotConfigured(format!("No credentials are configured for {provider}"))
}

fn unsupported_saved(provider: &str) -> AuthError {
    AuthError::Unavailable(format!(
        "The saved credential for {provider} is not supported by this engine"
    ))
}

fn store_failed(e: impl std::fmt::Display) -> AuthError {
    AuthError::Failed(format!("Saved credentials could not be read: {e}"))
}

/// The result of the credential layer before configured headers.
struct BaseAuth {
    credential: Credential,
    headers: BTreeMap<String, Option<String>>,
    env: BTreeMap<String, String>,
    base_url: Option<String>,
    /// The credential's own `env` (header templates see it first).
    cred_env: BTreeMap<String, String>,
}

impl Auth {
    pub fn new(config: &EngineConfig) -> Auth {
        Auth::with_store(config, Arc::new(FileStore::new(config.auth_path())))
    }

    /// Use another credential store (a future OS secret store, or tests).
    pub fn with_store(config: &EngineConfig, store: Arc<dyn CredentialStore>) -> Auth {
        let http = if config.env.ambient {
            crate::provider::http::client()
        } else {
            reqwest::Client::builder()
                .no_proxy()
                .connect_timeout(Duration::from_secs(30))
                .build()
                .expect("the HTTP client configuration is valid")
        };
        let copilot_ids = crate::provider::model::builtin_models()
            .into_iter()
            .filter(|m| m.provider == "github-copilot")
            .map(|m| m.id)
            .collect();
        let auth = Auth {
            inner: Arc::new(Inner {
                endpoints: Endpoints::from_env(&config.env),
                config: config.clone(),
                store,
                http,
                catalog: RwLock::new(Arc::new(Catalog::default())),
                models: RwLock::new(Arc::new(ModelsConfig::default())),
                copilot_ids,
                refresh_locks: Mutex::default(),
                refresh_timeout: Mutex::new(OAUTH_REFRESH_TIMEOUT),
                signin: Mutex::default(),
                listener: Mutex::new(None),
            }),
        };
        auth.reload();
        auth
    }

    pub fn set_listener(&self, listener: AuthListener) {
        *lock(&self.inner.listener) = Some(listener);
    }

    /// Bound on one OAuth refresh call (default 15 s; tests shorten it).
    pub fn set_refresh_timeout(&self, timeout: Duration) {
        *lock(&self.inner.refresh_timeout) = timeout;
    }

    /// Re-read `models.json`, `auth.json` and rebuild the catalog.
    pub fn reload(&self) -> Arc<Catalog> {
        self.inner.reload()
    }

    pub fn catalog(&self) -> Arc<Catalog> {
        self.inner.catalog()
    }

    /// The `models.json` layer currently in effect.
    pub fn models_config(&self) -> Arc<ModelsConfig> {
        self.inner.models_config()
    }

    /// Metadata-only check whether `model` could be used now.
    pub fn availability(&self, model: &Model) -> Result<(), AuthError> {
        self.inner.availability(model)
    }

    /// Resolve the credential and headers for one request (may refresh OAuth).
    pub fn resolve<'a>(
        &'a self,
        model: &'a Model,
        http: &'a reqwest::Client,
    ) -> BoxFuture<'a, Result<ResolvedAuth, AuthError>> {
        Box::pin(self.inner.resolve(model, http))
    }

    pub fn snapshot(&self, client: u64) -> AuthSnapshot {
        self.inner.snapshot(client)
    }

    pub fn start<'a>(
        &'a self,
        client: u64,
        provider: &'a str,
    ) -> BoxFuture<'a, Result<String, ProtoError>> {
        Box::pin(async move { Inner::start(&self.inner, client, provider) })
    }

    pub fn reuse<'a>(
        &'a self,
        client: u64,
        provider: &'a str,
    ) -> BoxFuture<'a, Result<String, ProtoError>> {
        Box::pin(async move { self.inner.reuse(client, provider) })
    }

    pub fn answer(
        &self,
        client: u64,
        attempt: &str,
        challenge: &str,
        response: String,
    ) -> Result<(), ProtoError> {
        self.inner.answer(client, attempt, challenge, response)
    }

    pub fn cancel(&self, client: u64, attempt: &str) {
        self.inner.cancel(client, attempt)
    }

    pub fn remove(&self, provider: &str) -> Result<(), ProtoError> {
        self.inner.remove(provider)
    }

    pub fn set_api_key(&self, provider: &str, key: String) -> Result<(), ProtoError> {
        self.inner.set_api_key(provider, key)
    }

    /// A client disconnected: cancel its attempts.
    pub fn client_gone(&self, client: u64) {
        self.inner.client_gone(client)
    }
}

impl Inner {
    fn catalog(&self) -> Arc<Catalog> {
        self.catalog
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn models_config(&self) -> Arc<ModelsConfig> {
        self.models
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn reload(&self) -> Arc<Catalog> {
        let mut warnings = Vec::new();
        let config = ModelsConfig::load(&self.config.models_path());
        if let Some(e) = &config.error {
            warnings.push(e.clone());
        }
        let builtin = crate::provider::model::builtin_models();
        let mut models = models_json::compose(builtin, &config, &mut warnings);

        // Credential-dependent catalog: Copilot's account model list, Radius gateway catalogs
        // restored from a stored credential.
        match self.store.read("github-copilot") {
            Ok(Some(StoredCredential::OAuth(c))) => {
                if let Some(ids) = oauth::copilot_available_models(&c) {
                    models.retain(|m| m.provider != "github-copilot" || ids.contains(&m.id));
                }
            }
            Ok(_) => {}
            Err(e) => warnings.push(format!("Saved credentials could not be read: {e}")),
        }
        let radius_ids = std::iter::once("radius").chain(
            config
                .providers
                .iter()
                .filter(|(id, p)| p.oauth_radius && id.as_str() != "radius")
                .map(|(id, _)| id.as_str()),
        );
        for id in radius_ids {
            if let Ok(Some(StoredCredential::OAuth(c))) = self.store.read(id) {
                merge_radius_gateway_models(&mut models, id, &c);
            }
        }

        models.extend(self.config.extra_models.iter().cloned());
        let catalog = Arc::new(Catalog { models, warnings });
        *self.models.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(config);
        *self.catalog.write().unwrap_or_else(|e| e.into_inner()) = catalog.clone();
        catalog
    }

    /// The OAuth flow for `provider` (never the reserved Claude subscription).
    fn flow(&self, provider: &str) -> Option<Arc<dyn OAuthFlow>> {
        if provider == "anthropic" {
            return None;
        }
        let models = self.models_config();
        let radius = models
            .provider(provider)
            .filter(|p| p.oauth_radius)
            .and_then(|p| {
                Some((
                    p.name.clone().unwrap_or_else(|| provider.to_owned()),
                    p.radius_gateway()?,
                ))
            });
        oauth::flow_for(provider, &self.endpoints, radius)
    }

    fn flow_ctx(&self, http: reqwest::Client, device_id: Option<String>) -> FlowCtx {
        FlowCtx {
            http,
            endpoints: self.endpoints.clone(),
            device_id,
            copilot_model_ids: self.copilot_ids.clone(),
        }
    }

    fn availability(&self, model: &Model) -> Result<(), AuthError> {
        if model.api == "scripted" {
            return Ok(());
        }
        let provider = model.provider.as_str();
        let models = self.models_config();
        let pc = models.provider(provider);
        let env = &self.config.env;
        match self.store.read(provider).map_err(store_failed)? {
            Some(StoredCredential::OAuth(_)) => {
                if provider == "anthropic" {
                    return Err(AuthError::Unavailable(RESERVED_MESSAGE.into()));
                }
                match self.flow(provider) {
                    Some(_) => Ok(()),
                    None => Err(AuthError::Unavailable(format!(
                        "The saved sign-in for {provider} is not supported by this engine"
                    ))),
                }
            }
            Some(StoredCredential::Unsupported(_)) => Err(unsupported_saved(provider)),
            Some(StoredCredential::ApiKey {
                key, env: cred_env, ..
            }) => {
                // A command counts as configured; a template must resolve.
                let key = key.filter(|k| config_value::is_configured(k, Some(&cred_env), env));
                let input = KeyInput {
                    key: key.as_deref(),
                    env: Some(&cred_env),
                };
                key_auth_ok(provider, providers::resolve_key_auth(provider, &input, env))
            }
            None => match pc.and_then(|p| p.api_key.as_deref()) {
                Some(raw) => {
                    if config_value::is_configured(raw, None, env) {
                        Ok(())
                    } else {
                        Err(not_configured(provider))
                    }
                }
                None => key_auth_ok(
                    provider,
                    providers::resolve_key_auth(provider, &KeyInput::default(), env),
                ),
            },
        }
    }

    async fn resolve(
        &self,
        model: &Model,
        http: &reqwest::Client,
    ) -> Result<ResolvedAuth, AuthError> {
        if model.api == "scripted" {
            return Ok(ResolvedAuth {
                credential: Credential::None,
                headers: BTreeMap::new(),
                base_url: None,
                env: HashMap::new(),
            });
        }
        let provider = model.provider.as_str();
        let models = self.models_config();
        let pc = models.provider(provider);
        let env = &self.config.env;
        let base = match self.store.read(provider).map_err(store_failed)? {
            Some(StoredCredential::OAuth(stored)) => {
                if provider == "anthropic" {
                    return Err(AuthError::Unavailable(RESERVED_MESSAGE.into()));
                }
                let flow = self.flow(provider).ok_or_else(|| {
                    AuthError::Unavailable(format!(
                        "The saved sign-in for {provider} is not supported by this engine"
                    ))
                })?;
                let credential = self
                    .fresh_oauth(provider, flow.clone(), stored, http)
                    .await?;
                let auth = flow.to_auth(&credential);
                let cred_env: BTreeMap<String, String> = credential
                    .extra
                    .get("env")
                    .and_then(Value::as_object)
                    .map(|m| {
                        m.iter()
                            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                            .collect()
                    })
                    .unwrap_or_default();
                let metadata: serde_json::Map<String, Value> = credential
                    .extra
                    .iter()
                    .filter(|(k, _)| !matches!(k.as_str(), "env" | "gatewayConfig"))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                BaseAuth {
                    credential: match auth.api_key {
                        Some(access) => Credential::OAuth { access, metadata },
                        None => Credential::None,
                    },
                    headers: auth.headers,
                    env: BTreeMap::new(),
                    base_url: auth.base_url,
                    cred_env,
                }
            }
            Some(StoredCredential::Unsupported(_)) => return Err(unsupported_saved(provider)),
            Some(StoredCredential::ApiKey {
                key, env: cred_env, ..
            }) => {
                let resolved = match key {
                    Some(k) => config_value::resolve(&k, Some(&cred_env), env, true).await,
                    None => None,
                };
                let input = KeyInput {
                    key: resolved.as_deref(),
                    env: Some(&cred_env),
                };
                key_base(
                    provider,
                    providers::resolve_key_auth(provider, &input, env),
                    cred_env,
                )?
            }
            None => match pc.and_then(|p| p.api_key.as_deref()) {
                Some(raw) => {
                    let key = config_value::resolve_or_err(
                        raw,
                        &format!("API key for provider \"{provider}\""),
                        None,
                        env,
                        true,
                    )
                    .await
                    .map_err(AuthError::Failed)?;
                    let input = KeyInput {
                        key: Some(&key),
                        env: None,
                    };
                    key_base(
                        provider,
                        providers::resolve_key_auth(provider, &input, env),
                        BTreeMap::new(),
                    )?
                }
                None => key_base(
                    provider,
                    providers::resolve_key_auth(provider, &KeyInput::default(), env),
                    BTreeMap::new(),
                )?,
            },
        };
        self.finish(model, pc, base).await
    }

    /// Configured headers over the credential layer (reference `withConfiguredAuth` and
    /// `resolveConfiguredModelHeaders`): provider headers, `authHeader`, per-model headers.
    async fn finish(
        &self,
        model: &Model,
        pc: Option<&ProviderConfig>,
        base: BaseAuth,
    ) -> Result<ResolvedAuth, AuthError> {
        let mut headers = base.headers;
        let mut header_env = base.cred_env.clone();
        for (k, v) in &base.env {
            header_env.insert(k.clone(), v.clone());
        }
        let provider = model.provider.as_str();
        if let Some(pc) = pc {
            for (name, raw) in pc.headers.iter().flatten() {
                let v = config_value::resolve_or_err(
                    raw,
                    &format!("provider \"{provider}\" header \"{name}\""),
                    Some(&header_env),
                    &self.config.env,
                    false,
                )
                .await
                .map_err(AuthError::Failed)?;
                headers.insert(name.clone(), Some(v));
            }
            if pc.auth_header == Some(true) {
                let key = base
                    .credential
                    .secret()
                    .ok_or_else(|| {
                        AuthError::Failed("authHeader requires a resolved API key".into())
                    })?
                    .to_owned();
                headers.insert("Authorization".into(), Some(format!("Bearer {key}")));
            }
            for (name, raw) in pc.model_headers(&model.id) {
                let v = config_value::resolve_or_err(
                    &raw,
                    &format!("model \"{provider}/{}\" header \"{name}\"", model.id),
                    Some(&base.env),
                    &self.config.env,
                    false,
                )
                .await
                .map_err(AuthError::Failed)?;
                headers.insert(name, Some(v));
            }
        }
        Ok(ResolvedAuth {
            credential: base.credential,
            headers,
            base_url: base.base_url,
            env: base.env.into_iter().collect(),
        })
    }

    fn refresh_lock(&self, provider: &str) -> Arc<tokio::sync::Mutex<()>> {
        lock(&self.refresh_locks)
            .entry(provider.to_owned())
            .or_default()
            .clone()
    }

    /// A valid OAuth credential, refreshing (once, under the locks) when it expires soon.
    async fn fresh_oauth(
        &self,
        provider: &str,
        flow: Arc<dyn OAuthFlow>,
        stored: OAuthCredential,
        http: &reqwest::Client,
    ) -> Result<OAuthCredential, AuthError> {
        let expires_soon =
            |c: &OAuthCredential| crate::util::now_ms() + OAUTH_VALIDITY_MARGIN_MS >= c.expires;
        if !expires_soon(&stored) {
            return Ok(stored);
        }
        let refresh_lock = self.refresh_lock(provider);
        let store = self.store.clone();
        let ctx = self.flow_ctx(http.clone(), None);
        let timeout = *lock(&self.refresh_timeout);
        let provider_owned = provider.to_owned();
        // The refresh runs in its own task: a caller that stops waiting cannot discard a
        // rotated refresh token before it is persisted.
        let task = tokio::spawn(async move {
            let provider = provider_owned;
            let _in_process = refresh_lock.lock().await;
            let mut txn = tokio::task::spawn_blocking(move || store.begin())
                .await
                .map_err(|e| AuthError::Failed(e.to_string()))?
                .map_err(store_failed)?;
            match txn.get(&provider) {
                Some(StoredCredential::OAuth(current)) if expires_soon(&current) => {
                    let next =
                        match tokio::time::timeout(timeout, flow.refresh(&ctx, &current)).await {
                            Err(_) => {
                                return Err(AuthError::Failed(format!(
                                    "OAuth refresh failed for {provider}: timed out"
                                )));
                            }
                            Ok(Err(e)) => {
                                return Err(AuthError::Failed(format!(
                                    "OAuth refresh failed for {provider}: {e}"
                                )));
                            }
                            Ok(Ok(next)) => next,
                        };
                    txn.set(&provider, StoredCredential::OAuth(next.clone()));
                    txn.commit().map_err(|e| {
                        AuthError::Failed(format!(
                            "OAuth refresh for {provider} could not be saved: {e}"
                        ))
                    })?;
                    Ok((next, true))
                }
                // Another request or process refreshed it meanwhile.
                Some(StoredCredential::OAuth(current)) => Ok((current, false)),
                // Signed out meanwhile.
                _ => Err(not_configured(&provider)),
            }
        });
        let (credential, refreshed) = task.await.map_err(|e| {
            AuthError::Failed(format!("OAuth refresh failed for {provider}: {e}"))
        })??;
        if refreshed {
            // Credential-dependent catalog data (Copilot's model list) may have changed.
            self.reload();
        }
        Ok(credential)
    }

    /// A stable installation id (UUID), kept in `settings.json` as `deviceId` (reference
    /// `SettingsManager.getOrCreateDeviceId`).
    fn device_id(&self) -> Result<String, String> {
        let path = self.config.settings_path();
        let mut warnings = Vec::new();
        let mut value = crate::config::read_json(&path, &mut warnings);
        if let Some(id) = value.get("deviceId").and_then(Value::as_str)
            && !id.is_empty()
        {
            return Ok(id.to_owned());
        }
        if !warnings.is_empty() {
            return Err(warnings.join("; "));
        }
        let id = crate::util::uuid_v4();
        value["deviceId"] = Value::String(id.clone());
        std::fs::create_dir_all(&self.config.config_dir).map_err(|e| e.to_string())?;
        let tmp = path.with_extension(format!("json.{}.tmp", crate::util::random_hex(4)));
        let text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())? + "\n";
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
        Ok(id)
    }
}

fn key_auth_ok(provider: &str, auth: KeyAuth) -> Result<(), AuthError> {
    match auth {
        KeyAuth::Resolved(_) => Ok(()),
        KeyAuth::NotConfigured => Err(not_configured(provider)),
        KeyAuth::Reserved => Err(AuthError::Unavailable(RESERVED_MESSAGE.into())),
    }
}

fn key_base(
    provider: &str,
    auth: KeyAuth,
    cred_env: BTreeMap<String, String>,
) -> Result<BaseAuth, AuthError> {
    match auth {
        KeyAuth::Resolved(r) => Ok(BaseAuth {
            credential: r.credential,
            headers: r.headers,
            env: r.env,
            base_url: None,
            cred_env,
        }),
        KeyAuth::NotConfigured => Err(not_configured(provider)),
        KeyAuth::Reserved => Err(AuthError::Unavailable(RESERVED_MESSAGE.into())),
    }
}

/// Models from a Radius credential's legacy `gatewayConfig` (reference
/// `providers/radius-config.ts` `getRadiusModels`), upserted by id.
fn merge_radius_gateway_models(
    models: &mut Vec<Model>,
    provider: &str,
    credential: &OAuthCredential,
) {
    let Some(config) = credential
        .extra
        .get("gatewayConfig")
        .and_then(Value::as_object)
    else {
        return;
    };
    let (Some(base_url), Some(list)) = (
        config.get("baseUrl").and_then(Value::as_str),
        config.get("models").and_then(Value::as_array),
    ) else {
        return;
    };
    for m in list {
        let Some(o) = m.as_object() else { continue };
        let valid = o.get("id").is_some_and(Value::is_string)
            && o.get("name").is_some_and(Value::is_string)
            && o.get("reasoning").is_some_and(Value::is_boolean)
            && o.get("input").is_some_and(Value::is_array)
            && o.get("cost").is_some_and(Value::is_object)
            && o.get("contextWindow").is_some_and(Value::is_number)
            && o.get("maxTokens").is_some_and(Value::is_number);
        if !valid {
            continue;
        }
        let mut o = o.clone();
        o.insert("api".into(), Value::String("pi-messages".into()));
        o.insert("provider".into(), Value::String(provider.to_owned()));
        o.insert("baseUrl".into(), Value::String(base_url.to_owned()));
        let Ok(model) = serde_json::from_value::<Model>(Value::Object(o)) else {
            continue;
        };
        match models
            .iter()
            .position(|x| x.provider == provider && x.id == model.id)
        {
            Some(i) => models[i] = model,
            None => models.push(model),
        }
    }
}
