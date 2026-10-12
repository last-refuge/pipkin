//! Reconstitute a pinned summary request using current credentials and network settings.
use super::*;
use crate::provider::{ApiAdapter, CacheRetention, ProviderRequest, ThinkingLevel};
use crate::store::compactions::SummaryRequest;
use std::time::Duration;

impl Shared {
    pub(super) async fn summary_request(
        &self,
        saved: &SummaryRequest,
    ) -> Result<(ProviderRequest, Arc<dyn ApiAdapter>, reqwest::Client), String> {
        let model = self
            .auth
            .catalog()
            .find(&saved.model)
            .cloned()
            .ok_or_else(|| format!("Model {} is not available", saved.model))?;
        let adapter = self
            .adapters
            .get(&model.api)
            .ok_or_else(|| format!("No adapter for API {}", model.api))?;
        let http = self.refresh_net();
        let proxy = self.proxy_config();
        super::super::run::unusable_proxy(&proxy, &model.base_url)?;
        let auth = self
            .auth
            .resolve(&model, &http)
            .await
            .map_err(|e| e.message().to_owned())?;
        let mut req = ProviderRequest {
            model,
            system: algo::SUMMARIZATION_SYSTEM_PROMPT.into(),
            messages: saved.messages.clone(),
            tools: vec![],
            thinking: ThinkingLevel::parse(&saved.thinking)
                .ok_or("Invalid saved thinking level")?,
            session_id: saved.provider_session.clone(),
            max_tokens: Some(saved.max_tokens),
            cache_retention: CacheRetention::None,
            credential: auth.credential,
            headers: auth.headers,
            base_url: auth.base_url,
            env: auth.env,
            timeout: Duration::from_millis(saved.timeout_ms),
            options: saved.stream_options.clone(),
        };
        if !self.config.env.ambient {
            for (k, v) in &self.config.env.vars {
                req.env.entry(k.clone()).or_insert_with(|| v.clone());
            }
            req.options
                .insert(crate::provider::ISOLATED_ENV.into(), true.into());
        }
        crate::provider::api::cloudflare::prepare(&mut req)?;
        super::super::run::unusable_proxy(&proxy, req.base_url())?;
        for (key, value) in proxy.env_pairs() {
            if !req.env.contains_key(&key) && !req.env.contains_key(&key.to_ascii_uppercase()) {
                req.env.insert(key, value);
            }
        }
        Ok((req, adapter, http))
    }
}
