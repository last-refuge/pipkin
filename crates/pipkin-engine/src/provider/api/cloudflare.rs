//! Cloudflare Workers AI and AI Gateway: not API kinds of their own, but a request
//! preparation step in front of the wrapped adapters (`openai-completions`,
//! `openai-responses`, `anthropic-messages`).
//!
//! Reference: `providers/cloudflare-stream.ts` (`resolveCloudflareModel`),
//! `providers/cloudflare-auth.ts` and `api/cloudflare.ts`.
//!
//! **Call [`prepare`] from the auth/dispatch layer after the credential, headers and env are
//! resolved and before the adapter runs.** It:
//! 1. fills `{CLOUDFLARE_ACCOUNT_ID}` / `{CLOUDFLARE_GATEWAY_ID}` in the base URL from the
//!    request env (credential env overrides, then the process), writing the result to
//!    `ProviderRequest::base_url`; a placeholder without a value is an error naming the
//!    variable (the reference would send the literal placeholder and fail remotely);
//! 2. for `cloudflare-ai-gateway`, moves the API key into `cf-aig-authorization: Bearer <key>`
//!    and deletes the adapters' own `Authorization` / `x-api-key` headers (the credential
//!    becomes [`Credential::None`]). A user-configured `cf-aig-authorization` header wins, as
//!    request headers win over auth headers in the reference.
//!
//! Workers AI keeps the API key as the normal `Authorization: Bearer` credential.

use crate::provider::{Credential, ProviderRequest};

pub const ACCOUNT_ID: &str = "CLOUDFLARE_ACCOUNT_ID";
pub const GATEWAY_ID: &str = "CLOUDFLARE_GATEWAY_ID";
pub const API_KEY: &str = "CLOUDFLARE_API_KEY";
pub const AI_GATEWAY_PROVIDER: &str = "cloudflare-ai-gateway";
pub const WORKERS_AI_PROVIDER: &str = "cloudflare-workers-ai";

/// Workers AI OpenAI-compatible endpoint template.
pub const WORKERS_AI_BASE_URL: &str =
    "https://api.cloudflare.com/client/v4/accounts/{CLOUDFLARE_ACCOUNT_ID}/ai/v1";
/// AI Gateway unified (OpenAI-compatible) endpoint template.
pub const AI_GATEWAY_COMPAT_BASE_URL: &str =
    "https://gateway.ai.cloudflare.com/v1/{CLOUDFLARE_ACCOUNT_ID}/{CLOUDFLARE_GATEWAY_ID}/compat";
/// AI Gateway OpenAI passthrough template.
pub const AI_GATEWAY_OPENAI_BASE_URL: &str =
    "https://gateway.ai.cloudflare.com/v1/{CLOUDFLARE_ACCOUNT_ID}/{CLOUDFLARE_GATEWAY_ID}/openai";
/// AI Gateway Anthropic passthrough template.
pub const AI_GATEWAY_ANTHROPIC_BASE_URL: &str = "https://gateway.ai.cloudflare.com/v1/{CLOUDFLARE_ACCOUNT_ID}/{CLOUDFLARE_GATEWAY_ID}/anthropic";

/// Whether this request belongs to a Cloudflare provider or carries a Cloudflare template.
pub fn applies(req: &ProviderRequest) -> bool {
    matches!(
        req.model.provider.as_str(),
        AI_GATEWAY_PROVIDER | WORKERS_AI_PROVIDER
    ) || has_placeholders(req.base_url())
}

fn has_placeholders(url: &str) -> bool {
    url.contains(&format!("{{{ACCOUNT_ID}}}")) || url.contains(&format!("{{{GATEWAY_ID}}}"))
}

/// Replace the placeholders that have values; unknown ones stay as they are.
pub fn fill_placeholders(url: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = url.to_owned();
    for name in [ACCOUNT_ID, GATEWAY_ID] {
        let placeholder = format!("{{{name}}}");
        if out.contains(&placeholder)
            && let Some(value) = lookup(name)
        {
            out = out.replace(&placeholder, &value);
        }
    }
    out
}

/// Prepare a Cloudflare request in place (see the module docs). A no-op for other providers
/// whose base URL has no Cloudflare placeholder.
pub fn prepare(req: &mut ProviderRequest) -> Result<(), String> {
    if !applies(req) {
        return Ok(());
    }
    let filled = fill_placeholders(req.base_url(), |name| req.env_var(name));
    for name in [ACCOUNT_ID, GATEWAY_ID] {
        if filled.contains(&format!("{{{name}}}")) {
            return Err(format!(
                "{} is not configured: set {name} (or sign in again) to use {}",
                if name == ACCOUNT_ID {
                    "The Cloudflare account id"
                } else {
                    "The Cloudflare AI Gateway id"
                },
                req.model.provider
            ));
        }
    }
    if filled != req.base_url() {
        req.base_url = Some(filled);
    }
    if req.model.provider == AI_GATEWAY_PROVIDER {
        let user_header = req
            .headers
            .keys()
            .any(|k| k.eq_ignore_ascii_case("cf-aig-authorization"));
        if let Some(key) = req.credential.secret().map(str::to_owned)
            && !user_header
        {
            req.headers
                .insert("cf-aig-authorization".into(), Some(format!("Bearer {key}")));
        }
        for name in ["authorization", "x-api-key"] {
            if !req.headers.keys().any(|k| k.eq_ignore_ascii_case(name)) {
                req.headers.insert(name.into(), None);
            }
        }
        req.credential = Credential::None;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{CacheRetention, Model, ThinkingLevel};
    use std::collections::{BTreeMap, HashMap};
    use std::time::Duration;

    fn request(provider: &str, base: &str, env: &[(&str, &str)]) -> ProviderRequest {
        let model: Model = serde_json::from_value(serde_json::json!({
            "id": "m", "api": "openai-completions", "provider": provider, "baseUrl": base
        }))
        .unwrap();
        ProviderRequest {
            model,
            system: String::new(),
            messages: vec![],
            tools: vec![],
            thinking: ThinkingLevel::Off,
            session_id: String::new(),
            max_tokens: None,
            cache_retention: CacheRetention::Short,
            credential: Credential::ApiKey("cf-key".into()),
            headers: BTreeMap::new(),
            base_url: None,
            timeout: Duration::from_secs(1),
            // Empty overrides mask any process values so the test is hermetic.
            env: [(ACCOUNT_ID, ""), (GATEWAY_ID, "")]
                .into_iter()
                .chain(env.iter().copied())
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect::<HashMap<_, _>>(),
            options: Default::default(),
        }
    }

    #[test]
    fn gateway_fills_the_template_and_moves_the_key() {
        let mut req = request(
            AI_GATEWAY_PROVIDER,
            AI_GATEWAY_ANTHROPIC_BASE_URL,
            &[(ACCOUNT_ID, "acct"), (GATEWAY_ID, "gw")],
        );
        prepare(&mut req).unwrap();
        assert_eq!(
            req.base_url.as_deref(),
            Some("https://gateway.ai.cloudflare.com/v1/acct/gw/anthropic")
        );
        assert_eq!(req.credential, Credential::None);
        assert_eq!(
            req.headers.get("cf-aig-authorization"),
            Some(&Some("Bearer cf-key".into()))
        );
        assert_eq!(req.headers.get("authorization"), Some(&None));
        assert_eq!(req.headers.get("x-api-key"), Some(&None));
    }

    #[test]
    fn workers_ai_keeps_the_bearer_credential() {
        let mut req = request(
            WORKERS_AI_PROVIDER,
            WORKERS_AI_BASE_URL,
            &[(ACCOUNT_ID, "acct")],
        );
        prepare(&mut req).unwrap();
        assert_eq!(
            req.base_url.as_deref(),
            Some("https://api.cloudflare.com/client/v4/accounts/acct/ai/v1")
        );
        assert_eq!(req.credential, Credential::ApiKey("cf-key".into()));
        assert!(req.headers.is_empty());
    }

    #[test]
    fn missing_ids_and_user_headers() {
        let mut req = request(
            AI_GATEWAY_PROVIDER,
            AI_GATEWAY_COMPAT_BASE_URL,
            &[(ACCOUNT_ID, "acct")],
        );
        let err = prepare(&mut req).unwrap_err();
        assert!(err.contains(GATEWAY_ID), "{err}");

        let mut req = request(
            AI_GATEWAY_PROVIDER,
            AI_GATEWAY_OPENAI_BASE_URL,
            &[(ACCOUNT_ID, "a"), (GATEWAY_ID, "g")],
        );
        req.headers
            .insert("CF-AIG-Authorization".into(), Some("Bearer mine".into()));
        prepare(&mut req).unwrap();
        assert!(!req.headers.contains_key("cf-aig-authorization"));
        assert_eq!(
            req.headers.get("CF-AIG-Authorization"),
            Some(&Some("Bearer mine".into()))
        );

        // Other providers are untouched.
        let mut other = request("openai", "https://api.openai.com/v1", &[]);
        prepare(&mut other).unwrap();
        assert_eq!(other.base_url, None);
        assert_eq!(other.credential, Credential::ApiKey("cf-key".into()));
    }
}
