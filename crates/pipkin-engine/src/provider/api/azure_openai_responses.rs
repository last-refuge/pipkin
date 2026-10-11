//! `azure-openai-responses`: the Responses API on Azure OpenAI
//! (reference `api/azure-openai-responses.ts`).
//!
//! Base URL: `azureBaseUrl` option > `AZURE_OPENAI_BASE_URL` > `https://<resource>.openai.azure.com/openai/v1`
//! (resource from `azureResourceName` / `AZURE_OPENAI_RESOURCE_NAME`) > the model's base URL;
//! Azure hosts are normalized to `/openai/v1`. Requests go to `<base>/responses?api-version=<v>`
//! (`azureApiVersion` / `AZURE_OPENAI_API_VERSION`, default `v1`) with an `api-key` header. The
//! deployment name (`azureDeploymentName` / `AZURE_OPENAI_DEPLOYMENT_NAME_MAP` / model id) is
//! sent as `model`.

use std::sync::Arc;

use pipkin_engine_protocol::Message;
use serde_json::{Map, Value, json};

use super::openai_completions::{clamp_prompt_cache_key, sdk_error};
use super::openai_responses::MIN_OUTPUT_TOKENS;
use super::openai_responses_shared::{apply_reasoning, convert_messages, convert_tools, exchange};
use super::transform::{build_headers, option_str, reasoning_effort};
use crate::provider::{ApiAdapter, BoxFuture, ProviderRequest, StreamCtx};

pub const API: &str = "azure-openai-responses";

const TOOL_CALL_PROVIDERS: &[&str] = &[
    "openai",
    "openai-codex",
    "opencode",
    "azure-openai-responses",
];
const DEFAULT_API_VERSION: &str = "v1";

pub struct AzureOpenAIResponses;

pub fn adapter() -> Arc<dyn ApiAdapter> {
    Arc::new(AzureOpenAIResponses)
}

impl ApiAdapter for AzureOpenAIResponses {
    fn api(&self) -> &'static str {
        API
    }

    fn stream<'a>(&'a self, ctx: StreamCtx<'a>) -> BoxFuture<'a, Message> {
        Box::pin(run(ctx))
    }
}

/// `modelId=deployment,...`
fn parse_deployment_map(value: &str) -> Vec<(String, String)> {
    value
        .split(',')
        .filter_map(|entry| {
            let entry = entry.trim();
            let mut parts = entry.splitn(2, '=');
            let model = parts.next()?.trim();
            let deployment = parts.next()?;
            // The reference splits with a limit of two and drops the remainder.
            let deployment = deployment.split('=').next().unwrap_or_default().trim();
            (!model.is_empty() && !deployment.is_empty())
                .then(|| (model.to_owned(), deployment.to_owned()))
        })
        .collect()
}

pub fn deployment_name(req: &ProviderRequest) -> String {
    if let Some(name) = option_str(req, "azureDeploymentName").filter(|s| !s.is_empty()) {
        return name.to_owned();
    }
    req.env_var("AZURE_OPENAI_DEPLOYMENT_NAME_MAP")
        .and_then(|raw| {
            parse_deployment_map(&raw)
                .into_iter()
                .rev()
                .find(|(m, _)| *m == req.model.id)
                .map(|(_, d)| d)
        })
        .unwrap_or_else(|| req.model.id.clone())
}

pub fn normalize_base_url(base: &str) -> Result<String, String> {
    let trimmed = base.trim().trim_end_matches('/');
    let mut url =
        url::Url::parse(trimmed).map_err(|_| format!("Invalid Azure OpenAI base URL: {base}"))?;
    let host = url.host_str().unwrap_or_default().to_owned();
    let azure = host.ends_with(".openai.azure.com")
        || host.ends_with(".cognitiveservices.azure.com")
        || host.ends_with(".ai.azure.com");
    let path = url.path().trim_end_matches('/').to_owned();
    if azure && matches!(path.as_str(), "" | "/" | "/openai" | "/openai/v1/responses") {
        url.set_path("/openai/v1");
        url.set_query(None);
    }
    Ok(url.to_string().trim_end_matches('/').to_owned())
}

/// `(base URL, api version)` (reference `resolveAzureConfig`).
pub fn resolve_config(req: &ProviderRequest) -> Result<(String, String), String> {
    let version = option_str(req, "azureApiVersion")
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| req.env_var("AZURE_OPENAI_API_VERSION"))
        .unwrap_or_else(|| DEFAULT_API_VERSION.to_owned());
    let explicit = option_str(req, "azureBaseUrl")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            req.env_var("AZURE_OPENAI_BASE_URL")
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
        });
    let resource = option_str(req, "azureResourceName")
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| req.env_var("AZURE_OPENAI_RESOURCE_NAME"));
    let base = explicit
        .or_else(|| resource.map(|r| format!("https://{r}.openai.azure.com/openai/v1")))
        .or_else(|| Some(req.base_url().to_owned()).filter(|s| !s.is_empty()))
        .ok_or_else(|| {
            "Azure OpenAI base URL is required. Set AZURE_OPENAI_BASE_URL or AZURE_OPENAI_RESOURCE_NAME, or pass azureBaseUrl, azureResourceName, or model.baseUrl.".to_owned()
        })?;
    Ok((normalize_base_url(&base)?, version))
}

/// The request body (reference `buildParams`).
pub fn build_params(req: &ProviderRequest) -> Value {
    let model = &req.model;
    let strict = model.compat_bool("supportsStrictMode").unwrap_or(true);
    let mut p = Map::new();
    p.insert("model".into(), json!(deployment_name(req)));
    p.insert(
        "input".into(),
        Value::Array(convert_messages(req, TOOL_CALL_PROVIDERS)),
    );
    p.insert("stream".into(), json!(true));
    p.insert("store".into(), json!(false));
    if let Some(key) = clamp_prompt_cache_key(&req.session_id) {
        p.insert("prompt_cache_key".into(), json!(key));
    }
    let max_tokens = req.max_tokens.unwrap_or(model.max_tokens);
    if max_tokens > 0 {
        p.insert(
            "max_output_tokens".into(),
            json!(max_tokens.max(MIN_OUTPUT_TOKENS)),
        );
    }
    if let Some(t) = req.options.get("temperature").filter(|v| !v.is_null()) {
        p.insert("temperature".into(), t.clone());
    }
    if !req.tools.is_empty() {
        p.insert(
            "tools".into(),
            Value::Array(convert_tools(&req.tools, strict)),
        );
    }
    if let Some(choice) = req.options.get("toolChoice").filter(|v| !v.is_null()) {
        p.insert("tool_choice".into(), choice.clone());
    }
    apply_reasoning(&mut p, req, reasoning_effort(req), false, false);
    Value::Object(p)
}

async fn run(ctx: StreamCtx<'_>) -> Message {
    let req = ctx.req;
    let Some(api_key) = req.credential.secret().filter(|s| !s.is_empty()) else {
        return ctx.failed(
            ctx.emitter(),
            format!("No API key for provider: {}", req.model.provider),
        );
    };
    let (base, version) = match resolve_config(req) {
        Ok(c) => c,
        Err(m) => return ctx.failed(ctx.emitter(), m),
    };
    let mut url = match url::Url::parse(&format!("{base}/responses")) {
        Ok(u) => u,
        Err(_) => {
            return ctx.failed(
                ctx.emitter(),
                format!("Invalid Azure OpenAI base URL: {base}"),
            );
        }
    };
    url.query_pairs_mut().append_pair("api-version", &version);
    let headers = match build_headers(
        &[("api-key", api_key.to_owned())],
        &req.model.headers,
        &[],
        &req.headers,
    ) {
        Ok(h) => h,
        Err(m) => return ctx.failed(ctx.emitter(), m),
    };
    let body = build_params(req);
    let format_http = |text: &str| sdk_error(text, Some("Azure OpenAI API error"));
    let (message, _) = exchange(
        &ctx,
        url.as_str(),
        headers,
        &body,
        &format_http,
        "Azure OpenAI Responses",
    )
    .await;
    message
}

#[cfg(test)]
mod tests {
    use pipkin_engine_protocol::{Block, StopReason};
    use serde_json::{Value, json};

    use super::*;
    use crate::provider::api::openai_completions::tests::{
        fragmented, parts, read_tool, request, responses_sse, run_mock,
    };
    use crate::provider::mock::MockResponse;
    use crate::provider::{CacheRetention, Credential, ThinkingLevel};

    fn model() -> Value {
        json!({"id": "gpt-5", "provider": "azure-openai-responses", "api": "azure-openai-responses",
               "baseUrl": "", "reasoning": true, "maxTokens": 1000})
    }

    #[test]
    fn base_url_resolution_and_normalization() {
        assert_eq!(
            normalize_base_url("https://res.openai.azure.com/").unwrap(),
            "https://res.openai.azure.com/openai/v1"
        );
        assert_eq!(
            normalize_base_url(
                "https://res.cognitiveservices.azure.com/openai/v1/responses?api-version=x"
            )
            .unwrap(),
            "https://res.cognitiveservices.azure.com/openai/v1"
        );
        assert_eq!(
            normalize_base_url("https://proxy.example.com/custom/").unwrap(),
            "https://proxy.example.com/custom"
        );
        assert!(normalize_base_url("not a url").is_err());

        let mut req = request(model());
        assert!(
            resolve_config(&req)
                .unwrap_err()
                .starts_with("Azure OpenAI base URL is required")
        );
        req.env
            .insert("AZURE_OPENAI_RESOURCE_NAME".into(), "myres".into());
        req.env.insert(
            "AZURE_OPENAI_API_VERSION".into(),
            "2025-04-01-preview".into(),
        );
        assert_eq!(
            resolve_config(&req).unwrap(),
            (
                "https://myres.openai.azure.com/openai/v1".into(),
                "2025-04-01-preview".into()
            )
        );
        req.env.insert(
            "AZURE_OPENAI_BASE_URL".into(),
            " https://other.openai.azure.com/openai ".into(),
        );
        assert_eq!(
            resolve_config(&req).unwrap().0,
            "https://other.openai.azure.com/openai/v1"
        );
        req.options
            .insert("azureBaseUrl".into(), json!("https://opt.ai.azure.com"));
        req.options.insert("azureApiVersion".into(), json!("v2"));
        assert_eq!(
            resolve_config(&req).unwrap(),
            ("https://opt.ai.azure.com/openai/v1".into(), "v2".into())
        );
    }

    #[test]
    fn deployment_names() {
        let mut req = request(model());
        assert_eq!(deployment_name(&req), "gpt-5");
        req.env.insert(
            "AZURE_OPENAI_DEPLOYMENT_NAME_MAP".into(),
            " gpt-4o=dep-a , gpt-5=dep-b=x,bad, gpt-5 = dep-c".into(),
        );
        assert_eq!(deployment_name(&req), "dep-c");
        req.options
            .insert("azureDeploymentName".into(), json!("explicit"));
        assert_eq!(deployment_name(&req), "explicit");
    }

    #[test]
    fn request_body_defaults() {
        let mut req = request(model());
        req.session_id = "sess".into();
        req.cache_retention = CacheRetention::None;
        req.tools = vec![read_tool()];
        req.thinking = ThinkingLevel::Medium;
        let b = build_params(&req);
        assert_eq!(b["model"], "gpt-5");
        assert_eq!(
            b["prompt_cache_key"], "sess",
            "azure always sends the session cache key"
        );
        assert!(b.get("prompt_cache_retention").is_none());
        assert_eq!(b["max_output_tokens"], 1000);
        assert_eq!(
            b["tools"][0]["strict"], false,
            "strict is sent by default on Azure"
        );
        assert_eq!(
            b["reasoning"],
            json!({"effort": "medium", "summary": "auto"})
        );
        assert_eq!(b["include"], json!(["reasoning.encrypted_content"]));
        req.thinking = ThinkingLevel::Off;
        assert_eq!(build_params(&req)["reasoning"], json!({"effort": "none"}));
    }

    #[tokio::test]
    async fn exchange_uses_api_key_header_version_query_and_deployment() {
        let events = vec![
            json!({"type": "response.output_item.added", "output_index": 0, "item": {"type": "message", "id": "m1"}}),
            json!({"type": "response.output_text.delta", "output_index": 0, "delta": "hi"}),
            json!({"type": "response.output_item.done", "output_index": 0, "item": {"type": "message", "id": "m1", "content": [{"type": "output_text", "text": "hi"}]}}),
            json!({"type": "response.completed", "response": {"id": "r", "status": "completed",
                "usage": {"input_tokens": 10, "output_tokens": 2, "total_tokens": 12}}}),
        ];
        let mut req = request(model());
        req.credential = Credential::ApiKey("azure-key".into());
        req.options
            .insert("azureApiVersion".into(), json!("2025-03-01-preview"));
        req.env.insert(
            "AZURE_OPENAI_DEPLOYMENT_NAME_MAP".into(),
            "gpt-5=prod-gpt5".into(),
        );
        let run = run_mock(
            adapter(),
            req,
            "/openai/v1",
            vec![fragmented(&responses_sse(&events), 21)],
        )
        .await;
        let (content, stop, error, _) = parts(&run.message);
        assert_eq!(error, None);
        assert_eq!(stop, StopReason::Stop);
        assert_eq!(content[0].as_text(), Some("hi"));
        assert!(
            matches!(&content[0], Block::Text { extra, .. } if extra["textSignature"] == r#"{"v":1,"id":"m1"}"#)
        );
        let r = &run.requests[0];
        assert_eq!(
            r.path,
            "/openai/v1/responses?api-version=2025-03-01-preview"
        );
        assert_eq!(r.header("api-key"), Some("azure-key"));
        assert_eq!(r.header("authorization"), None);
        assert_eq!(r.json()["model"], "prod-gpt5");
    }

    #[tokio::test]
    async fn errors_use_the_azure_prefix_and_need_a_key() {
        let run = run_mock(adapter(), request(model()), "", vec![MockResponse::json(404, &json!({"error": {"code": "DeploymentNotFound", "message": "The API deployment for this resource does not exist."}}))]).await;
        let error = parts(&run.message).2.unwrap();
        assert!(
            error.starts_with("Azure OpenAI API error (404): {"),
            "{error}"
        );
        let mut req = request(model());
        req.credential = Credential::None;
        let run = run_mock(
            adapter(),
            req,
            "",
            vec![MockResponse::json(200, &json!({}))],
        )
        .await;
        assert_eq!(
            parts(&run.message).2,
            Some("No API key for provider: azure-openai-responses")
        );
        assert!(run.requests.is_empty());
    }
}
