//! Per-client sign-in lifecycle (reference `coding-agent/experimental/services/provider-auth-provider.ts`).
//!
//! Each client has at most one attempt. Starting a new attempt cancels the previous one;
//! answers must name the current attempt and its current challenge. Provider error text never
//! reaches a snapshot (it may contain URLs or tokens): failures say "Could not complete sign-in.
//! Try again." Snapshots are volatile and never logged.

use std::collections::HashMap;
use std::sync::Arc;

use pipkin_engine_protocol::{
    AuthChallenge, AuthProviderView, AuthSnapshot, AuthStatus, CredentialKind, ErrorCode,
    ProtoError,
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::oauth::{self, AuthEvent, Interaction, InteractionSink, Prompt, PromptKind};
use super::providers::RESERVED_MESSAGE;
use super::store::StoredCredential;
use super::{Inner, lock};
use crate::provider::BoxFuture;

pub const MAX_ANSWER_CHARS: usize = 8192;
pub const MSG_CONNECTING: &str = "Connecting…";
pub const MSG_DONE: &str = "Connected. Choose a model to continue.";
pub const MSG_FAILED: &str = "Could not complete sign-in. Try again.";
pub const MSG_CHECKING: &str = "Checking your saved connection…";
pub const MSG_REUSED: &str = "Connected. Choose a project to continue.";
pub const MSG_REUSE_FAILED: &str = "The saved connection is not ready. Sign in again to continue.";
pub const MSG_REMOVED: &str = "Saved connection removed.";
pub const MSG_EXPIRED: &str = "The sign-in question has expired";
pub const MSG_BAD_OPTION: &str = "The selected option is not available";
pub const MSG_NOT_AVAILABLE: &str = "This provider is not available for sign-in";
pub const MSG_REMOVING: &str = "Wait for connection removal to finish";
const CANCELLED: &str = "Sign-in cancelled";

#[derive(Default)]
pub(crate) struct State {
    clients: HashMap<u64, Client>,
    removing: bool,
}

#[derive(Default)]
struct Client {
    attempt: Option<Attempt>,
    view: View,
}

struct Attempt {
    id: String,
    provider: String,
    cancel: CancellationToken,
    pending: Option<Pending>,
}

struct Pending {
    challenge: AuthChallenge,
    tx: oneshot::Sender<String>,
}

#[derive(Clone, Default)]
struct View {
    attempt: Option<String>,
    provider: Option<String>,
    status: AuthStatus,
    message: String,
    url: Option<String>,
    device_code: Option<String>,
    challenge: Option<AuthChallenge>,
}

impl View {
    fn connecting(attempt: &str, provider: &str, message: &str) -> View {
        View {
            attempt: Some(attempt.to_owned()),
            provider: Some(provider.to_owned()),
            status: AuthStatus::Connecting,
            message: message.to_owned(),
            ..Default::default()
        }
    }

    fn settle(&mut self, status: AuthStatus, message: &str) {
        self.status = status;
        self.message = message.to_owned();
        self.challenge = None;
        self.url = None;
        self.device_code = None;
    }
}

fn err(code: ErrorCode, message: &str) -> ProtoError {
    ProtoError::new(code, message)
}

/// Prompts and notifications of one attempt, published only while it is current.
struct AttemptSink {
    inner: Arc<Inner>,
    client: u64,
    attempt: String,
}

/// Withdraws a pending challenge when its prompt future goes away unanswered (the callback
/// won the race, or the attempt ended).
struct PendingGuard {
    inner: Arc<Inner>,
    client: u64,
    attempt: String,
    challenge: String,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        let withdrawn = self
            .inner
            .update_current(self.client, &self.attempt, |att, view| {
                if att
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.challenge.id == self.challenge)
                {
                    att.pending = None;
                    view.challenge = None;
                    view.status = AuthStatus::Waiting;
                    true
                } else {
                    false
                }
            });
        if withdrawn == Some(true) {
            self.inner.emit(self.client);
        }
    }
}

impl InteractionSink for AttemptSink {
    fn prompt(&self, prompt: Prompt) -> BoxFuture<'static, Result<String, String>> {
        let inner = self.inner.clone();
        let client = self.client;
        let attempt = self.attempt.clone();
        Box::pin(async move {
            let (tx, rx) = oneshot::channel();
            let challenge = AuthChallenge {
                id: crate::util::uuid_v4(),
                kind: prompt.kind.as_str().to_owned(),
                message: prompt.message,
                placeholder: prompt.placeholder,
                options: if prompt.kind == PromptKind::Select {
                    prompt.options
                } else {
                    vec![]
                },
            };
            let challenge_id = challenge.id.clone();
            let published = inner.update_current(client, &attempt, |att, view| {
                att.pending = Some(Pending {
                    challenge: challenge.clone(),
                    tx,
                });
                view.status = AuthStatus::Prompt;
                view.challenge = Some(challenge);
            });
            if published.is_none() {
                return Err(CANCELLED.into());
            }
            inner.emit(client);
            let _guard = PendingGuard {
                inner: inner.clone(),
                client,
                attempt,
                challenge: challenge_id,
            };
            rx.await.map_err(|_| CANCELLED.to_string())
        })
    }

    fn notify(&self, event: AuthEvent) {
        let changed =
            self.inner
                .update_current(self.client, &self.attempt, |_, view| match event {
                    AuthEvent::AuthUrl { url, instructions } => {
                        view.status = AuthStatus::Waiting;
                        view.url = Some(url);
                        view.message =
                            instructions.unwrap_or_else(|| "Continue in your browser.".into());
                    }
                    AuthEvent::DeviceCode {
                        user_code,
                        verification_uri,
                    } => {
                        view.status = AuthStatus::Waiting;
                        view.url = Some(verification_uri);
                        view.device_code = Some(user_code);
                        view.message = "Enter this code on the provider's sign-in page.".into();
                    }
                    AuthEvent::Info(m) | AuthEvent::Progress(m) => view.message = m,
                });
        if changed.is_some() {
            self.inner.emit(self.client);
        }
    }
}

impl Inner {
    /// Run `f` on the client's current attempt if it is `attempt`.
    fn update_current<R>(
        &self,
        client: u64,
        attempt: &str,
        f: impl FnOnce(&mut Attempt, &mut View) -> R,
    ) -> Option<R> {
        let mut state = lock(&self.signin);
        let c = state.clients.get_mut(&client)?;
        let att = c.attempt.as_mut().filter(|a| a.id == attempt)?;
        Some(f(att, &mut c.view))
    }

    pub(super) fn emit(&self, client: u64) {
        let listener = lock(&self.listener).clone();
        if let Some(l) = listener {
            l(client, self.snapshot(client));
        }
    }

    fn emit_all(&self) {
        let clients: Vec<u64> = lock(&self.signin).clients.keys().copied().collect();
        for c in clients {
            self.emit(c);
        }
    }

    pub(super) fn cancel_all_attempts(&self) {
        let mut state = lock(&self.signin);
        for c in state.clients.values_mut() {
            if let Some(a) = c.attempt.take() {
                a.cancel.cancel();
            }
        }
    }

    /// Sign-in-capable providers (metadata only).
    fn provider_views(&self) -> (Vec<AuthProviderView>, bool) {
        let (stored, failed) = match self.store.list() {
            Ok(list) => (list, false),
            Err(_) => (vec![], true),
        };
        let has = |id: &str| stored.iter().any(|i| i.provider == id);
        let mut views = vec![AuthProviderView {
            id: "anthropic".into(),
            name: "Anthropic (Claude subscription)".into(),
            kind: CredentialKind::OAuth,
            available: false,
            unavailable_reason: Some(RESERVED_MESSAGE.into()),
            has_credentials: has("anthropic"),
        }];
        for (id, name) in oauth::BUILTIN_OAUTH_PROVIDERS {
            views.push(AuthProviderView {
                id: (*id).into(),
                name: (*name).into(),
                kind: CredentialKind::OAuth,
                available: true,
                unavailable_reason: None,
                has_credentials: has(id),
            });
        }
        let models = self.models_config();
        for (id, p) in &models.providers {
            if !p.oauth_radius || views.iter().any(|v| &v.id == id) || p.radius_gateway().is_none()
            {
                continue;
            }
            views.push(AuthProviderView {
                id: id.clone(),
                name: p.name.clone().unwrap_or_else(|| id.clone()),
                kind: CredentialKind::OAuth,
                available: true,
                unavailable_reason: None,
                has_credentials: has(id),
            });
        }
        (views, failed)
    }

    pub(super) fn snapshot(&self, client: u64) -> AuthSnapshot {
        let (providers, failed) = self.provider_views();
        let view = lock(&self.signin)
            .clients
            .entry(client)
            .or_default()
            .view
            .clone();
        AuthSnapshot {
            providers,
            credential_lookup_failed: failed,
            attempt: view.attempt,
            provider: view.provider,
            status: view.status,
            message: view.message,
            url: view.url,
            device_code: view.device_code,
            challenge: view.challenge,
        }
    }

    /// Replace the client's attempt with a new one; `None` while a removal runs.
    fn begin_attempt(
        &self,
        client: u64,
        provider: &str,
        message: &str,
    ) -> Result<(String, CancellationToken), ProtoError> {
        let mut state = lock(&self.signin);
        if state.removing {
            return Err(err(ErrorCode::Busy, MSG_REMOVING));
        }
        let c = state.clients.entry(client).or_default();
        if let Some(old) = c.attempt.take() {
            old.cancel.cancel();
        }
        let id = crate::util::uuid_v4();
        let cancel = CancellationToken::new();
        c.attempt = Some(Attempt {
            id: id.clone(),
            provider: provider.to_owned(),
            cancel: cancel.clone(),
            pending: None,
        });
        c.view = View::connecting(&id, provider, message);
        Ok((id, cancel))
    }

    fn settle(&self, client: u64, attempt: &str, status: AuthStatus, message: &str) {
        let changed = self.update_current(client, attempt, |att, view| {
            att.pending = None;
            view.settle(status, message);
        });
        if changed.is_some() {
            self.emit(client);
        }
    }

    pub(super) fn start(
        self: &Arc<Self>,
        client: u64,
        provider: &str,
    ) -> Result<String, ProtoError> {
        if provider == "anthropic" {
            return Err(err(ErrorCode::Unsupported, RESERVED_MESSAGE));
        }
        let flow = self
            .flow(provider)
            .ok_or_else(|| err(ErrorCode::Unsupported, MSG_NOT_AVAILABLE))?;
        let device_id = if provider == "openai" {
            Some(
                self.device_id()
                    .map_err(|_| err(ErrorCode::Internal, "Sign-in is not available"))?,
            )
        } else {
            None
        };
        let (id, cancel) = self.begin_attempt(client, provider, MSG_CONNECTING)?;
        self.emit(client);
        let inner = self.clone();
        let attempt = id.clone();
        let provider = provider.to_owned();
        tokio::spawn(async move {
            let http = inner.http.read().unwrap_or_else(|e| e.into_inner()).clone();
            let ctx = inner.flow_ctx(http, device_id);
            let io = Interaction::new(
                Arc::new(AttemptSink {
                    inner: inner.clone(),
                    client,
                    attempt: attempt.clone(),
                }),
                cancel.clone(),
            );
            let result = tokio::select! {
                r = flow.login(&ctx, &io) => r,
                _ = cancel.cancelled() => Err(oauth::CANCELLED.into()),
            };
            drop(io);
            let outcome = match result {
                Ok(credential) if !cancel.is_cancelled() => {
                    let store = inner.store.clone();
                    let p = provider.clone();
                    let saved = tokio::task::spawn_blocking(move || {
                        let mut txn = store.begin()?;
                        txn.set(&p, StoredCredential::OAuth(credential));
                        txn.commit()
                    })
                    .await;
                    matches!(saved, Ok(Ok(())))
                }
                _ => false,
            };
            if outcome {
                inner.reload();
                inner.settle(client, &attempt, AuthStatus::Done, MSG_DONE);
                inner.emit_all();
            } else {
                // The reason stays out of snapshots and logs (it may carry URLs or tokens).
                log::info!("sign-in for {provider} did not complete");
                inner.settle(client, &attempt, AuthStatus::Failed, MSG_FAILED);
            }
        });
        Ok(id)
    }

    pub(super) fn reuse(&self, client: u64, provider: &str) -> Result<String, ProtoError> {
        if provider == "anthropic" {
            return Err(err(ErrorCode::Unsupported, RESERVED_MESSAGE));
        }
        if self.flow(provider).is_none() {
            return Err(err(ErrorCode::Unsupported, MSG_NOT_AVAILABLE));
        }
        let (id, _) = self.begin_attempt(client, provider, MSG_CHECKING)?;
        self.emit(client);
        // No network: a saved OAuth credential, and at least one usable model.
        let saved = matches!(
            self.store.read(provider),
            Ok(Some(StoredCredential::OAuth(_)))
        );
        let ready = saved
            && self
                .catalog()
                .models
                .iter()
                .any(|m| m.provider == provider && self.availability(m).is_ok());
        if ready {
            self.settle(client, &id, AuthStatus::Done, MSG_REUSED);
        } else {
            self.settle(client, &id, AuthStatus::Failed, MSG_REUSE_FAILED);
        }
        Ok(id)
    }

    pub(super) fn answer(
        &self,
        client: u64,
        attempt: &str,
        challenge: &str,
        response: String,
    ) -> Result<(), ProtoError> {
        let expired = || err(ErrorCode::NotFound, MSG_EXPIRED);
        {
            let mut state = lock(&self.signin);
            let c = state.clients.get_mut(&client).ok_or_else(expired)?;
            let att = c
                .attempt
                .as_mut()
                .filter(|a| a.id == attempt)
                .ok_or_else(expired)?;
            let pending = att
                .pending
                .as_ref()
                .filter(|p| p.challenge.id == challenge)
                .ok_or_else(expired)?;
            if response.chars().count() > MAX_ANSWER_CHARS {
                return Err(expired());
            }
            if pending.challenge.kind == PromptKind::Select.as_str()
                && !pending
                    .challenge
                    .options
                    .iter()
                    .any(|(id, _)| *id == response)
            {
                return Err(err(ErrorCode::Invalid, MSG_BAD_OPTION));
            }
            let pending = att.pending.take().ok_or_else(expired)?;
            c.view.challenge = None;
            c.view.status = AuthStatus::Waiting;
            if pending.tx.send(response).is_err() {
                return Err(expired());
            }
        }
        self.emit(client);
        Ok(())
    }

    pub(super) fn cancel(&self, client: u64, attempt: &str) {
        let cancelled = {
            let mut state = lock(&self.signin);
            match state.clients.get_mut(&client) {
                Some(c) if c.attempt.as_ref().is_some_and(|a| a.id == attempt) => {
                    if let Some(a) = c.attempt.take() {
                        a.cancel.cancel();
                    }
                    c.view = View::default();
                    true
                }
                _ => false,
            }
        };
        if cancelled {
            self.emit(client);
        }
    }

    pub(super) fn client_gone(&self, client: u64) {
        if let Some(c) = lock(&self.signin).clients.remove(&client)
            && let Some(a) = c.attempt
        {
            a.cancel.cancel();
        }
    }

    pub(super) fn remove(&self, provider: &str) -> Result<(), ProtoError> {
        {
            let mut state = lock(&self.signin);
            if state.removing {
                return Err(err(ErrorCode::Busy, MSG_REMOVING));
            }
            state.removing = true;
            for c in state.clients.values_mut() {
                if c.attempt.as_ref().is_some_and(|a| a.provider == provider) {
                    if let Some(a) = c.attempt.take() {
                        a.cancel.cancel();
                    }
                    c.view = View {
                        message: MSG_REMOVED.into(),
                        ..Default::default()
                    };
                }
            }
        }
        let result = self.store.begin().and_then(|mut txn| {
            txn.remove(provider);
            txn.commit()
        });
        lock(&self.signin).removing = false;
        self.reload();
        self.emit_all();
        result.map_err(|_| {
            err(
                ErrorCode::Internal,
                "Could not remove the saved connection. Try again.",
            )
        })
    }

    pub(super) fn set_api_key(&self, provider: &str, key: String) -> Result<(), ProtoError> {
        if provider.trim().is_empty() {
            return Err(err(ErrorCode::Invalid, "A provider is required"));
        }
        if key.trim().is_empty() {
            return Err(err(ErrorCode::Invalid, "The API key is empty"));
        }
        if key.len() > 16 * 1024 {
            return Err(err(ErrorCode::TooLarge, "The API key is too long"));
        }
        let saved = (|| -> Result<Result<(), ProtoError>, super::store::StoreError> {
            let mut txn = self.store.begin()?;
            let (env, extra) = match txn.get(provider) {
                Some(StoredCredential::OAuth(_)) if provider == "anthropic" => {
                    // The reserved subscription credential is never replaced implicitly.
                    return Ok(Err(err(
                        ErrorCode::Conflict,
                        "A saved Claude subscription sign-in exists for anthropic; remove it before saving an API key.",
                    )));
                }
                Some(StoredCredential::ApiKey { env, extra, .. }) => (env, extra),
                _ => Default::default(),
            };
            txn.set(
                provider,
                StoredCredential::ApiKey {
                    key: Some(key),
                    env,
                    extra,
                },
            );
            txn.commit()?;
            Ok(Ok(()))
        })();
        match saved {
            Ok(Ok(())) => {
                self.reload();
                self.emit_all();
                Ok(())
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Err(err(
                ErrorCode::Internal,
                "Could not save the API key. Try again.",
            )),
        }
    }
}
