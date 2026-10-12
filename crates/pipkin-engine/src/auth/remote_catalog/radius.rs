//! Private gateway catalogs have no public-catalog Last-Modified freshness requirement.
use super::*;
use crate::auth::{Inner, StoredCredential, models_json::ModelsConfig};

pub(in crate::auth) fn gateways(config: &ModelsConfig, default: &str) -> BTreeMap<String, String> {
    let mut gateways = BTreeMap::from([("radius".into(), default.into())]);
    for (id, provider) in &config.providers {
        if provider.oauth_radius
            && let Some(gateway) = provider.radius_gateway()
        {
            gateways.insert(id.clone(), gateway);
        }
    }
    gateways
}

pub(in crate::auth) fn overlays(
    inner: &Inner,
    config: &ModelsConfig,
) -> BTreeMap<String, Vec<Model>> {
    gateways(config, &inner.endpoints.radius_gateway)
        .into_keys()
        .map(|id| {
            let cached = lock(&inner.remote.state).stored.get(&id).map(|entry| {
                entry
                    .models
                    .iter()
                    .filter(|m| m.provider == id && m.api == "pi-messages")
                    .cloned()
                    .collect()
            });
            let models = cached.unwrap_or_else(|| legacy(inner, &id));
            (id, models)
        })
        .collect()
}

fn legacy(inner: &Inner, provider: &str) -> Vec<Model> {
    let Ok(Some(StoredCredential::OAuth(credential))) = inner.store.read(provider) else {
        return vec![];
    };
    credential
        .extra
        .get("gatewayConfig")
        .and_then(|value| parse(provider, value).ok())
        .unwrap_or_default()
}

pub(super) fn migrate_legacy(inner: &Inner, generation: u64) {
    let config = ModelsConfig::load(&inner.config.models_path());
    for provider in gateways(&config, &inner.endpoints.radius_gateway).into_keys() {
        if lock(&inner.remote.state).stored.contains_key(&provider) {
            continue;
        }
        let models = legacy(inner, &provider);
        if !models.is_empty() {
            inner
                .remote
                .publish(generation, &provider, Ok(update(models)));
        }
    }
}

fn update(models: Vec<Model>) -> Update {
    Update {
        persist: Some(Entry {
            models,
            checked_at: Some(crate::util::now_ms()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

// Reference radius-config.ts sanitizes the envelope and filters malformed entries.
fn parse(provider: &str, value: &Value) -> Result<Vec<Model>, String> {
    let Some(base) = value.get("baseUrl").and_then(Value::as_str) else {
        return Err(format!("Invalid Radius config for {provider}."));
    };
    let Some(list) = value.get("models").and_then(Value::as_array) else {
        return Err(format!("Invalid Radius config for {provider}."));
    };
    Ok(list
        .iter()
        .filter_map(|value| {
            let mut model = value.as_object()?.clone();
            if !(model.get("id").is_some_and(Value::is_string)
                && model.get("name").is_some_and(Value::is_string)
                && model.get("reasoning").is_some_and(Value::is_boolean)
                && model.get("input").is_some_and(Value::is_array)
                && model.get("cost").is_some_and(Value::is_object)
                && model.get("contextWindow").is_some_and(Value::is_number)
                && model.get("maxTokens").is_some_and(Value::is_number))
            {
                return None;
            }
            model.insert("api".into(), "pi-messages".into());
            model.insert("provider".into(), provider.into());
            model.insert("baseUrl".into(), base.into());
            serde_json::from_value(Value::Object(model)).ok()
        })
        .collect())
}

pub(super) async fn fetch(
    http: &reqwest::Client,
    gateway: &str,
    provider: &str,
    key: Option<&str>,
    cancel: &CancellationToken,
) -> Result<Update, String> {
    let mut url = url::Url::parse(gateway).map_err(|_| "Invalid Radius gateway URL.".to_owned())?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Radius gateway URL must not contain credentials.".into());
    }
    url.set_path("/v1/config");
    url.set_query(None);
    url.set_fragment(None);
    let mut request = http
        .get(url)
        .header("accept", "application/json")
        .timeout(Duration::from_secs(4));
    if let Some(key) = key {
        request = request.bearer_auth(key);
    }
    let work = async {
        let response = request
            .send()
            .await
            .map_err(|_| format!("Could not refresh Radius catalog for {provider}."))?;
        if !response.status().is_success() {
            // Keep remote error bodies out of diagnostics; they may echo credentials.
            return Err(format!(
                "Radius catalog request failed for {provider}: {}",
                response.status().as_u16()
            ));
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|_| format!("Could not read Radius catalog for {provider}."))?;
            if chunk.len() > MAX_BODY.saturating_sub(bytes.len()) {
                return Err(format!(
                    "Radius catalog for {provider} exceeds the 8 MiB limit."
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        let value = serde_json::from_slice(&bytes)
            .map_err(|_| format!("Invalid Radius catalog for {provider}."))?;
        Ok(update(parse(provider, &value)?))
    };
    tokio::select! {
        _ = cancel.cancelled() => Err("Model refresh cancelled.".into()),
        result = work => result,
    }
}

#[cfg(test)]
mod tests;
