//! Pi provider sign-in state is volatile; never include browser links, codes or errors in logs.
use pipkin_core::onboarding::{SignInChallenge, SignInProvider, SignInSnapshot, SignInStatus};
use serde_json::Value;

pub fn parse(value: &Value) -> SignInSnapshot {
    let status = match value.get("status").and_then(Value::as_str) {
        Some("idle") => SignInStatus::Idle,
        Some("connecting") => SignInStatus::Connecting,
        Some("waiting") => SignInStatus::Waiting,
        Some("prompt") => SignInStatus::Prompt,
        Some("done") => SignInStatus::Done,
        Some("failed") => SignInStatus::Failed,
        _ => return SignInSnapshot::default(),
    };
    let text = |object: &Value, key: &str, max: usize| {
        object
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| s.len() <= max)
            .map(str::to_owned)
    };
    let url = text(value, "url", 8192).filter(|s| s.starts_with("https://"));
    let challenge = value.get("challenge").and_then(|c| {
        let kind = text(c, "type", 32)?;
        if !["text", "secret", "select", "manual_code"].contains(&kind.as_str()) {
            return None;
        }
        let options = c
            .get("options")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .take(32)
                    .filter_map(|o| Some((text(o, "id", 128)?, text(o, "label", 256)?)))
                    .collect()
            })
            .unwrap_or_default();
        Some(SignInChallenge {
            id: text(c, "id", 128)?,
            kind,
            message: text(c, "message", 2048)?,
            placeholder: text(c, "placeholder", 256),
            options,
        })
    });
    SignInSnapshot {
        available: true,
        credentials_known: value
            .get("credentialsKnown")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        credential_lookup_failed: value
            .get("credentialLookupFailed")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        has_existing_credentials: value
            .get("hasExistingCredentials")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        existing_providers: value
            .get("existingProviders")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(Value::as_str)
                    .filter_map(|id| match id {
                        "anthropic" => Some(SignInProvider::Claude),
                        "openai-codex" => Some(SignInProvider::ChatGpt),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        attempt: text(value, "attempt", 128),
        provider: match value.get("provider").and_then(Value::as_str) {
            Some("anthropic") => Some(SignInProvider::Claude),
            Some("openai-codex") => Some(SignInProvider::ChatGpt),
            _ => None,
        },
        status,
        message: text(value, "message", 4096).unwrap_or_default(),
        url,
        device_code: text(value, "deviceCode", 128),
        challenge,
        // Pi offers every allowlisted provider.
        unavailable: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_allowlisted_providers_and_web_links_are_displayed() {
        let v = json!({"attempt":"a", "provider":"openai", "status":"waiting", "url":"javascript:alert(1)", "deviceCode":"123"});
        let s = parse(&v);
        assert!(s.available);
        assert_eq!(s.provider, None);
        assert_eq!(s.url, None);
        assert_eq!(s.device_code.as_deref(), Some("123"));
    }

    #[test]
    fn debug_does_not_expose_a_browser_link_or_device_code() {
        let s = parse(
            &json!({"status":"prompt", "provider":"anthropic", "url":"https://example.test/private", "deviceCode":"secret123", "challenge":{"id":"x", "type":"manual_code", "message":"Paste the redirect"}}),
        );
        assert_eq!(s.provider, Some(SignInProvider::Claude));
        let debug = format!("{s:?}");
        assert!(!debug.contains("private"));
        assert!(!debug.contains("secret123"));
    }
}
