//! CLI-reported telemetry, never execution authority or a subscription bill.
//! Cumulative model/cost snapshots must not be summed with per-invocation token receipts.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metadata {
    pub invocation: Map<String, Value>,
    pub session_snapshot: Map<String, Value>,
}
impl std::fmt::Debug for Metadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CliUsageMetadata([redacted])")
    }
}
fn count(v: &Value) -> Option<Value> {
    v.as_u64()
        .filter(|v| *v <= i64::MAX as u64)
        .map(Value::from)
}
fn cost(v: &Value) -> Option<Value> {
    v.as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0 && *n <= 1_000_000_000.0)
        .map(|_| v.clone())
}
fn label(v: &Value) -> Option<Value> {
    v.as_str()
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-._:/".contains(&c))
        })
        .map(|s| Value::String(s.into()))
}
fn fields(
    raw: &Value,
    names: &[&str],
    into: &mut Map<String, Value>,
    parse: fn(&Value) -> Option<Value>,
) {
    for name in names {
        if let Some(v) = raw.get(*name).and_then(parse) {
            into.insert((*name).into(), v);
        }
    }
}
pub(crate) fn parse(line: &str) -> Option<Metadata> {
    let raw: Value = serde_json::from_str(line).ok()?;
    let mut result = Metadata::default();
    fields(
        &raw,
        &["duration_ms", "duration_api_ms", "num_turns"],
        &mut result.invocation,
        count,
    );
    // Closed enumerations avoid retaining arbitrary error strings or user content.
    if let Some(
        reason @ ("end_turn" | "max_tokens" | "stop_sequence" | "tool_use" | "pause_turn"
        | "refusal"),
    ) = raw.get("stop_reason").and_then(Value::as_str)
    {
        result
            .invocation
            .insert("stop_reason".into(), reason.into());
    }
    if let Some(usage) = raw.get("usage") {
        let mut details = Map::new();
        for (field, allowed) in [
            ("service_tier", &["standard", "priority", "batch"][..]),
            ("speed", &["standard", "fast"][..]),
        ] {
            if let Some(value) = usage
                .get(field)
                .and_then(Value::as_str)
                .filter(|v| allowed.contains(v))
            {
                details.insert(field.into(), value.into());
            }
        }
        if let Some(server) = usage.get("server_tool_use") {
            let mut counts = Map::new();
            fields(
                server,
                &["web_search_requests", "web_fetch_requests"],
                &mut counts,
                count,
            );
            if !counts.is_empty() {
                details.insert("server_tool_use".into(), counts.into());
            }
        }
        if let Some(cache) = usage.get("cache_creation") {
            let mut counts = Map::new();
            fields(
                cache,
                &["ephemeral_5m_input_tokens", "ephemeral_1h_input_tokens"],
                &mut counts,
                count,
            );
            if !counts.is_empty() {
                details.insert("cache_creation".into(), counts.into());
            }
        }
        if !details.is_empty() {
            result
                .invocation
                .insert("usage_details".into(), details.into());
        }
    }
    fields(
        &raw,
        &["total_cost_usd"],
        &mut result.session_snapshot,
        cost,
    );
    if let Some(models) = raw
        .get("modelUsage")
        .and_then(Value::as_object)
        .filter(|m| m.len() <= 16)
    {
        let mut retained = Map::new();
        for (name, value) in models {
            if label(&Value::String(name.clone())).is_none() {
                continue;
            }
            let mut model = Map::new();
            fields(
                value,
                &[
                    "inputTokens",
                    "outputTokens",
                    "cacheReadInputTokens",
                    "cacheCreationInputTokens",
                    "webSearchRequests",
                    "contextWindow",
                    "maxOutputTokens",
                ],
                &mut model,
                count,
            );
            fields(value, &["costUSD"], &mut model, cost);
            fields(value, &["canonicalModel"], &mut model, label);
            if let Some(
                provider @ ("firstParty"
                | "bedrock"
                | "vertex"
                | "foundry"
                | "anthropicAws"
                | "anthropicGoogleCloud"
                | "mantle"
                | "gateway"),
            ) = value.get("provider").and_then(Value::as_str)
            {
                model.insert("provider".into(), provider.into());
            }
            if !model.is_empty() {
                retained.insert(name.clone(), Value::Object(model));
            }
        }
        if !retained.is_empty() {
            result
                .session_snapshot
                .insert("modelUsage".into(), retained.into());
        }
    }
    (!result.invocation.is_empty() || !result.session_snapshot.is_empty()).then_some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn features_malformed_telemetry_is_unknown_and_cannot_inject_content() {
        let value = json!({"duration_ms": -1,"duration_api_ms": "private", "num_turns":{},"total_cost_usd": -1,"stop_reason":"private text", "modelUsage":{"model":{"inputTokens":-1,"provider":"private","canonicalModel":"private text","costUSD":-1,"secret":"private"}}});
        assert!(parse(&value.to_string()).is_none());
        let value = json!({"duration_ms":12,"total_cost_usd":0,"modelUsage":{"model":{"inputTokens":3,"contextWindow":200000,"secret":"private"}}});
        let parsed = parse(&value.to_string()).unwrap();
        assert_eq!(parsed.invocation["duration_ms"], 12);
        assert_eq!(parsed.session_snapshot["total_cost_usd"], 0);
        assert!(!serde_json::to_string(&parsed).unwrap().contains("private"));
        let mut too_many = json!({});
        for n in 0..17 {
            too_many[format!("model{n}")] = json!({"inputTokens":1});
        }
        assert!(parse(&json!({"modelUsage":too_many}).to_string()).is_none());
    }
}
