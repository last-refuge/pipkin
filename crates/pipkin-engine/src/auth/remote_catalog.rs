//! Persisted public catalog overlays. Reference: remote-catalog-provider.ts and models.ts.
//! Network requests carry no provider credentials; models.json is applied after the overlay.
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use futures_util::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::{Auth, FileStore, lock};
use crate::provider::Model;

const REFRESH_MS: i64 = 4 * 60 * 60 * 1000;
const MAX_BODY: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    models: Vec<Model>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checked_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_modified: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Default)]
struct State {
    generation: u64,
    cancel: CancellationToken,
    stored: BTreeMap<String, Entry>,
    overlays: BTreeMap<String, Vec<Model>>,
    warnings: BTreeMap<String, String>,
}

pub(super) struct Cache {
    file: FileStore,
    generated_at: i64,
    state: Mutex<State>,
}

fn generated_at() -> i64 {
    let manifest: Value = serde_json::from_str(include_str!("../../model-data/manifest.json"))
        .expect("embedded catalog manifest");
    chrono::DateTime::parse_from_rfc3339(
        manifest["generatedAt"]
            .as_str()
            .expect("catalog generation"),
    )
    .expect("valid catalog generation timestamp")
    .timestamp_millis()
}

impl Cache {
    pub(super) fn cancel(&self) {
        lock(&self.state).cancel.cancel();
    }
    pub(super) fn reload(&self) {
        self.begin(&CancellationToken::new());
    }
    pub(super) fn new(path: std::path::PathBuf) -> Self {
        let cache = Self {
            file: FileStore::new(path),
            generated_at: generated_at(),
            state: Mutex::default(),
        };
        cache.restore(&mut lock(&cache.state));
        cache
    }

    fn visible(&self, provider: &str, entry: &Entry) -> Vec<Model> {
        if entry.last_modified.is_none_or(|at| at <= self.generated_at) {
            return vec![];
        }
        entry
            .models
            .iter()
            .filter(|m| m.provider == provider && supported(m.kind.as_deref()))
            .cloned()
            .collect()
    }

    fn restore(&self, state: &mut State) {
        let entries = match self.file.raw_entries() {
            Ok(entries) => entries,
            Err(_) => {
                state.warnings.insert(
                    "cache".into(),
                    "Could not read models-store.json; keeping the usable model catalog.".into(),
                );
                return;
            }
        };
        state.stored.clear();
        state.overlays.clear();
        state.warnings.clear();
        for (provider, value) in entries {
            match serde_json::from_value::<Entry>(value) {
                Ok(entry) => {
                    state
                        .overlays
                        .insert(provider.clone(), self.visible(&provider, &entry));
                    state.stored.insert(provider, entry);
                }
                Err(_) => {
                    state.warnings.insert(
                        provider.clone(),
                        format!("Could not read the cached catalog for {provider}."),
                    );
                }
            }
        }
    }

    fn begin(&self, caller: &CancellationToken) -> Option<(u64, CancellationToken)> {
        let mut state = lock(&self.state);
        if caller.is_cancelled() {
            return None;
        }
        state.cancel.cancel();
        state.cancel = caller.child_token();
        state.generation += 1;
        self.restore(&mut state);
        Some((state.generation, state.cancel.clone()))
    }

    pub(super) fn apply(&self, baseline: Vec<Model>, warnings: &mut Vec<String>) -> Vec<Model> {
        let state = lock(&self.state);
        warnings.extend(state.warnings.values().cloned());
        let providers: std::collections::HashSet<_> =
            baseline.iter().map(|m| m.provider.clone()).collect();
        let mut models = baseline;
        let mut positions: BTreeMap<_, _> = models
            .iter()
            .enumerate()
            .map(|(i, m)| {
                (
                    (
                        m.provider.clone(),
                        m.kind.clone().unwrap_or_else(|| "chat".into()),
                        m.id.clone(),
                    ),
                    i,
                )
            })
            .collect();
        for (provider, overlay) in &state.overlays {
            if provider == "radius" || !providers.contains(provider) {
                continue;
            }
            for model in overlay {
                let key = (
                    provider.clone(),
                    model.kind.clone().unwrap_or_else(|| "chat".into()),
                    model.id.clone(),
                );
                if let Some(index) = positions.get(&key) {
                    models[*index] = model.clone();
                } else {
                    positions.insert(key, models.len());
                    models.push(model.clone());
                }
            }
        }
        models
    }

    fn publish(&self, generation: u64, provider: &str, update: Result<Update, String>) {
        let cancel = {
            let state = lock(&self.state);
            if state.generation != generation || state.cancel.is_cancelled() {
                return;
            }
            state.cancel.clone()
        };
        let update = match update {
            Ok(update) => update,
            Err(error) => {
                let mut state = lock(&self.state);
                if state.generation == generation && !state.cancel.is_cancelled() {
                    state.warnings.insert(provider.into(), error);
                }
                return;
            }
        };
        // Acquire the external file lock BEFORE the publication lock. A busy cache file
        // must not stop cancellation/newer refresh admission. Hold the publication guard
        // from the final current-generation check through atomic persistence and update.
        let (mut state, saved) = if let Some(entry) = &update.persist {
            let mut held = None;
            let saved = self.file.set_raw_if(
                provider,
                serde_json::to_value(entry).expect("catalog entry"),
                &cancel,
                || {
                    let state = lock(&self.state);
                    let current = state.generation == generation && !state.cancel.is_cancelled();
                    held = Some(state);
                    current
                },
            );
            (held.unwrap_or_else(|| lock(&self.state)), saved)
        } else {
            (lock(&self.state), Ok(true))
        };
        if state.generation != generation || state.cancel.is_cancelled() {
            return;
        }
        if saved.is_err() {
            state.warnings.insert(
                provider.into(),
                format!(
                    "Could not save the model catalog for {provider}; keeping its previous catalog."
                ),
            );
            return;
        }
        if let Some(entry) = update.persist {
            state.stored.insert(provider.into(), entry);
        }
        if let Some(models) = update.models {
            state.overlays.insert(provider.into(), models);
        }
        if let Some(error) = update.warning {
            state.warnings.insert(provider.into(), error);
        } else {
            state.warnings.remove(provider);
        }
    }
}

#[derive(Default)]
struct Update {
    persist: Option<Entry>,
    models: Option<Vec<Model>>,
    warning: Option<String>,
}

fn supported(kind: Option<&str>) -> bool {
    kind.is_none_or(|k| matches!(k, "chat" | "image" | "classifier"))
}

fn parse(provider: &str, value: Value) -> Result<Vec<Model>, String> {
    let entries = match value {
        Value::Array(a) => a,
        Value::Object(mut o) => match o.remove("models") {
            Some(Value::Array(a)) => a,
            Some(value) => {
                o.insert("models".into(), value);
                o.into_values().collect()
            }
            None => o.into_values().collect(),
        },
        _ => return Err(format!("Invalid model catalog for {provider}.")),
    };
    entries
        .into_iter()
        .filter(|m| m.is_object() && m.get("id").is_some())
        .filter(|m| {
            m.get("type").is_none()
                || m.get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|k| supported(Some(k)))
        })
        .map(|mut value| {
            value["provider"] = provider.into();
            serde_json::from_value::<Model>(value)
                .map_err(|_| format!("Invalid model metadata for {provider}."))
        })
        .collect()
}

async fn fetch(
    client: &reqwest::Client,
    base: &str,
    provider: &str,
    mut stored: Option<Entry>,
    generated: i64,
    cancel: &CancellationToken,
    timeout: Duration,
) -> Result<Update, String> {
    let now = crate::util::now_ms();
    if stored.as_ref().is_some_and(|e| {
        e.last_modified.is_some()
            && e.checked_at
                .is_some_and(|at| now.saturating_sub(at) < REFRESH_MS)
    }) {
        return Ok(Update::default());
    }
    let mut url = url::Url::parse(base).map_err(|_| "Invalid catalog service URL.".to_owned())?;
    url.set_path(&format!("/api/models/providers/{provider}"));
    url.set_query(Some("types=chat,image,classifier"));
    // Reject embedded URL credentials rather than accidentally sending them as Basic auth.
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Catalog service URL must not contain credentials.".into());
    }
    let etag = stored
        .as_ref()
        .filter(|e| !e.models.is_empty())
        .and_then(|e| e.etag.as_deref());
    let mut response = None;
    for attempt in 0..3 {
        let mut request = client
            .get(url.clone())
            .header("accept", "application/json")
            .header("user-agent", concat!("pipkin/", env!("CARGO_PKG_VERSION")))
            .timeout(timeout);
        if let Some(etag) = etag {
            request = request.header("if-none-match", etag);
        }
        let got = tokio::select! {
            _ = cancel.cancelled() => return Err("Model refresh cancelled.".into()),
            got = request.send() => got,
        };
        match got {
            Ok(r)
                if attempt < 2
                    && matches!(r.status().as_u16(), 408 | 425 | 429 | 500 | 502 | 503 | 504) =>
            {
                continue;
            }
            Ok(r) => {
                response = Some(r);
                break;
            }
            Err(_) if attempt < 2 => continue,
            Err(_) => {
                return Err(format!(
                    "Could not refresh the model catalog for {provider}."
                ));
            }
        }
    }
    let response = response.expect("three attempts always produce a result");
    let status = response.status();
    let checked_at = Some(crate::util::now_ms());
    if status.as_u16() == 304
        && let Some(entry) = stored.take()
    {
        return Ok(Update {
            persist: Some(Entry {
                checked_at,
                ..entry
            }),
            ..Default::default()
        });
    }
    if matches!(status.as_u16(), 404 | 501) {
        return Ok(Update {
            persist: Some(Entry {
                checked_at,
                last_modified: Some(0),
                etag: None,
                ..stored.unwrap_or_default()
            }),
            ..Default::default()
        });
    }
    if !status.is_success() {
        return Ok(Update {
            persist: Some(Entry {
                checked_at,
                ..stored.unwrap_or_default()
            }),
            warning: Some(format!(
                "Model catalog request failed for {provider}: {}",
                status.as_u16()
            )),
            ..Default::default()
        });
    }
    let etag = response
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let last_modified = Some(
        response
            .headers()
            .get("last-modified")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| chrono::DateTime::parse_from_rfc2822(s).ok())
            .map_or(0, |d| d.timestamp_millis()),
    );
    let body = async {
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|_| format!("Could not read the model catalog for {provider}."))?;
            if chunk.len() > MAX_BODY.saturating_sub(bytes.len()) {
                return Err(format!(
                    "Model catalog for {provider} exceeds the 8 MiB limit."
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| format!("Invalid model catalog for {provider}."))
    };
    let value = tokio::select! {
        _ = cancel.cancelled() => return Err("Model refresh cancelled.".into()),
        value = body => value?,
    };
    let models = parse(provider, value)?;
    let visible = if last_modified.is_some_and(|at| at > generated) {
        models.clone()
    } else {
        vec![]
    };
    Ok(Update {
        persist: Some(Entry {
            models,
            checked_at,
            last_modified,
            etag,
            extra: Default::default(),
        }),
        models: Some(visible),
        warning: None,
    })
}

impl Auth {
    pub(crate) fn cancel_model_refresh(&self) {
        self.inner.remote.cancel();
    }
    /// Restore then refresh public metadata for configured built-ins. New calls supersede
    /// older ones; cancellation never discards the last usable catalog.
    pub async fn refresh_models(&self, caller: &CancellationToken) {
        if caller.is_cancelled() {
            return;
        }
        let inner = self.inner.clone();
        let token = caller.clone();
        let Ok(Some((generation, cancel))) = tokio::task::spawn_blocking(move || {
            let started = inner.remote.begin(&token);
            inner.reload();
            started
        })
        .await
        else {
            return;
        };
        if self.inner.config.env.contains("PI_OFFLINE") || cancel.is_cancelled() {
            return;
        }
        let catalog = self.catalog();
        let mut providers = BTreeMap::new();
        for builtin in crate::provider::model::builtin_models() {
            if builtin.provider == "radius" {
                continue;
            }
            let model = catalog.find(&builtin.key()).unwrap_or(&builtin);
            if self.availability(model).is_ok() {
                providers
                    .entry(model.provider.clone())
                    .or_insert_with(|| model.clone());
            }
        }
        let http = self
            .inner
            .http
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        stream::iter(providers).for_each_concurrent(8, |(provider,model)| {
            let cancel = cancel.clone();
            let http = http.clone();
            async move {
                let stored = lock(&self.inner.remote.state).stored.get(&provider).cloned();
                let work = async {
                    // Auth resolution can refresh an expired token or evaluate a configured
                    // key. The returned credentials never enter the public catalog request.
                    self.resolve(&model,&http).await.map_err(|_| format!("Could not resolve credentials for {provider} while refreshing models."))?;
                    fetch(&http,&self.inner.config.catalog_base_url,&provider,stored,self.inner.remote.generated_at,&cancel,Duration::from_secs(4)).await
                };
                let update = tokio::select! { _ = cancel.cancelled() => return, update = work => update };
                let inner = self.inner.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    inner.remote.publish(generation,&provider,update);
                    inner.reload();
                }).await;
            }
        }).await;
    }
}

#[cfg(test)]
mod tests;
