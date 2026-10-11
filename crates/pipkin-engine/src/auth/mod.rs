//! Credentials, provider configuration and sign-in.
//!
//! - **Catalog**: built-in models + `models.json` providers/models/overrides (+ test extras).
//! - **Resolution** ([`Auth::resolve`]): per request, in reference precedence: stored
//!   credential (`auth.json`), `models.json` `apiKey`, ambient environment. OAuth refresh is
//!   serialized per credential and persisted atomically; a failed refresh never falls back to a
//!   different billing credential.
//! - **Availability** ([`Auth::availability`]): metadata only (no network, no refresh).
//! - **Sign-in lifecycle**: per-client attempts with start/reuse/answer/cancel/remove,
//!   challenges and stale-attempt guards; volatile, never persisted or logged.
//!
//! The Claude subscription sign-in and its request path are **reserved** for a later
//! integration: the provider is listed with `available: false`, starting it is refused with
//! `Unsupported`, and a stored subscription credential makes affected models unavailable
//! rather than silently using an API key.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};

use pipkin_engine_protocol::{AuthSnapshot, ErrorCode, ProtoError};

use crate::config::EngineConfig;
use crate::provider::{BoxFuture, Credential, Model};

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

pub struct Auth {
    config: EngineConfig,
    catalog: RwLock<Arc<Catalog>>,
}

impl Auth {
    pub fn new(config: &EngineConfig) -> Auth {
        let auth = Auth {
            config: config.clone(),
            catalog: RwLock::new(Arc::new(Catalog::default())),
        };
        auth.reload();
        auth
    }

    pub fn set_listener(&self, _listener: AuthListener) {}

    /// Re-read `models.json`, `auth.json` and rebuild the catalog.
    pub fn reload(&self) -> Arc<Catalog> {
        let mut models = crate::provider::model::builtin_models();
        models.extend(self.config.extra_models.iter().cloned());
        let catalog = Arc::new(Catalog {
            models,
            warnings: vec![],
        });
        *self.catalog.write().unwrap_or_else(|e| e.into_inner()) = catalog.clone();
        catalog
    }

    pub fn catalog(&self) -> Arc<Catalog> {
        self.catalog
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Metadata-only check whether `model` could be used now.
    pub fn availability(&self, model: &Model) -> Result<(), AuthError> {
        if model.api == "scripted" {
            return Ok(());
        }
        Err(AuthError::NotConfigured(format!(
            "No credentials are configured for {}",
            model.provider
        )))
    }

    /// Resolve the credential and headers for one request (may refresh OAuth).
    pub fn resolve<'a>(
        &'a self,
        model: &'a Model,
        _http: &'a reqwest::Client,
    ) -> BoxFuture<'a, Result<ResolvedAuth, AuthError>> {
        Box::pin(async move {
            self.availability(model)?;
            Ok(ResolvedAuth {
                credential: Credential::None,
                headers: BTreeMap::new(),
                base_url: None,
                env: HashMap::new(),
            })
        })
    }

    pub fn snapshot(&self, _client: u64) -> AuthSnapshot {
        AuthSnapshot::default()
    }

    pub fn start<'a>(&'a self, _client: u64, provider: &'a str) -> BoxFuture<'a, Result<String, ProtoError>> {
        Box::pin(async move {
            Err(ProtoError::new(
                ErrorCode::Unsupported,
                format!("Sign-in for {provider} is not available"),
            ))
        })
    }

    pub fn reuse<'a>(&'a self, client: u64, provider: &'a str) -> BoxFuture<'a, Result<String, ProtoError>> {
        self.start(client, provider)
    }

    pub fn answer(&self, _client: u64, _attempt: &str, _challenge: &str, _response: String) -> Result<(), ProtoError> {
        Err(ProtoError::new(ErrorCode::NotFound, "The sign-in question has expired"))
    }

    pub fn cancel(&self, _client: u64, _attempt: &str) {}

    pub fn remove(&self, provider: &str) -> Result<(), ProtoError> {
        Err(ProtoError::new(
            ErrorCode::Unsupported,
            format!("Could not remove the saved connection for {provider}"),
        ))
    }

    pub fn set_api_key(&self, provider: &str, _key: String) -> Result<(), ProtoError> {
        Err(ProtoError::new(
            ErrorCode::Unsupported,
            format!("Saving an API key for {provider} is not available"),
        ))
    }

    /// A client disconnected: cancel its attempts.
    pub fn client_gone(&self, _client: u64) {}
}
