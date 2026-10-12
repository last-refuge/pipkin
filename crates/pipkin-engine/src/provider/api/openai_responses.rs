//! `openai-responses`: the OpenAI Responses API (reference `api/openai-responses.ts`).
//!
//! Stateless replay (`store: false`): encrypted reasoning is requested whenever reasoning is
//! on and carried back through the thinking block's signature. Prompt caching uses the session
//! id as `prompt_cache_key`, with 24h retention or the explicit cache mode where supported.

use std::sync::Arc;

use pipkin_engine_protocol::Message;
use serde_json::{Map, Value, json};

use super::common::{base_max_tokens, build_headers, opt_str, reasoning_effort};
use super::openai_completions::{
    clamp_prompt_cache_key, client_api_key, copilot_headers, sdk_error,
};
use super::openai_responses_shared::{
    apply_reasoning, apply_service_tier_pricing, convert_messages, convert_tools, exchange,
    service_tier_multiplier,
};
use crate::provider::{ApiAdapter, BoxFuture, CacheRetention, ProviderRequest, StreamCtx};

pub const API: &str = "openai-responses";

const TOOL_CALL_PROVIDERS: &[&str] = &["openai", "openai-codex", "opencode"];
/// OpenAI Responses rejects `max_output_tokens` below 16.
pub const MIN_OUTPUT_TOKENS: u64 = 16;
const CHATGPT_USAGE_URL: &str = "https://chatgpt.com/settings/usage";

pub struct OpenAIResponses;

pub fn adapter() -> Arc<dyn ApiAdapter> {
    Arc::new(OpenAIResponses)
}

impl ApiAdapter for OpenAIResponses {
    fn api(&self) -> &'static str {
        API
    }

    fn stream<'a>(&'a self, ctx: StreamCtx<'a>) -> BoxFuture<'a, Message> {
        Box::pin(run(ctx))
    }
}

/// Resolved `model.compat` for Responses endpoints (reference `getCompat`).
#[derive(Clone, Debug, PartialEq)]
pub struct Compat {
    pub session_affinity_format: String,
    pub supports_long_cache_retention: bool,
    pub supports_strict_mode: bool,
    pub supports_explicit_prompt_cache_mode: bool,
    pub supports_max_output_tokens: bool,
}

impl Compat {
    pub fn resolve(req: &ProviderRequest) -> Compat {
        let m = &req.model;
        let detected = if m.provider == "openrouter" || req.base_url().contains("openrouter.ai") {
            "openrouter"
        } else {
            "openai"
        };
        Compat {
            session_affinity_format: m
                .compat_str("sessionAffinityFormat")
                .unwrap_or(detected)
                .to_owned(),
            supports_long_cache_retention: m
                .compat_bool("supportsLongCacheRetention")
                .unwrap_or(true),
            supports_strict_mode: m.compat_bool("supportsStrictMode").unwrap_or(false),
            supports_explicit_prompt_cache_mode: m
                .compat_bool("supportsExplicitPromptCacheMode")
                .unwrap_or(false),
            supports_max_output_tokens: m.compat_bool("supportsMaxOutputTokens").unwrap_or(true),
        }
    }
}

/// An OpenAI access token that is not an API key ("Sign in with ChatGPT"): those requests
/// must omit cache retention options, `max_output_tokens` and `temperature`.
fn is_chatgpt_sign_in(req: &ProviderRequest) -> bool {
    req.model.provider == "openai"
        && req.base_url() == "https://api.openai.com/v1"
        && req
            .credential
            .secret()
            .is_some_and(|k| !k.starts_with("sk-"))
}

/// The request body (reference `buildParams`).
pub fn build_params(req: &ProviderRequest, compat: &Compat) -> Result<Value, String> {
    let model = &req.model;
    let retention = req.cache_retention;
    let omit = is_chatgpt_sign_in(req);
    let mut p = Map::new();
    p.insert("model".into(), json!(model.id));
    p.insert(
        "input".into(),
        Value::Array(convert_messages(
            model,
            Some(&req.system),
            &req.messages,
            TOOL_CALL_PROVIDERS,
        )?),
    );
    p.insert("stream".into(), json!(true));
    p.insert("store".into(), json!(false));
    if retention != CacheRetention::None
        && let Some(key) = clamp_prompt_cache_key(&req.session_id)
    {
        p.insert("prompt_cache_key".into(), json!(key));
    }
    if !omit {
        if retention == CacheRetention::Long
            && compat.supports_long_cache_retention
            && !compat.supports_explicit_prompt_cache_mode
        {
            p.insert("prompt_cache_retention".into(), json!("24h"));
        }
        if compat.supports_explicit_prompt_cache_mode {
            if retention == CacheRetention::None {
                p.insert("prompt_cache_options".into(), json!({"mode": "explicit"}));
            } else if retention == CacheRetention::Long && compat.supports_long_cache_retention {
                p.insert("prompt_cache_options".into(), json!({"ttl": "30m"}));
            }
        }
    }
    // Reference `buildBaseOptions`: the cap is clamped to what fits in the context window.
    let max_tokens = base_max_tokens(req);
    if max_tokens > 0 && compat.supports_max_output_tokens && !omit {
        p.insert(
            "max_output_tokens".into(),
            json!(max_tokens.max(MIN_OUTPUT_TOKENS)),
        );
    }
    if !omit && let Some(t) = req.options.get("temperature").filter(|v| !v.is_null()) {
        p.insert("temperature".into(), t.clone());
    }
    if let Some(tier) = req.options.get("serviceTier").filter(|v| !v.is_null()) {
        p.insert("service_tier".into(), tier.clone());
    }
    if !req.tools.is_empty() {
        p.insert(
            "tools".into(),
            Value::Array(convert_tools(
                &req.tools,
                compat.supports_strict_mode,
                Some(false),
            )),
        );
    }
    if let Some(choice) = req.options.get("toolChoice").filter(|v| !v.is_null()) {
        p.insert("tool_choice".into(), choice.clone());
    }
    apply_reasoning(
        &mut p,
        req,
        reasoning_effort(req),
        model.provider == "github-copilot",
        model.provider == "xai",
    );
    Ok(Value::Object(p))
}

async fn run(ctx: StreamCtx<'_>) -> Message {
    let req = ctx.req;
    let compat = Compat::resolve(req);
    let label = if req.model.provider == "openai" {
        "OpenAI".to_owned()
    } else {
        req.model.provider.clone()
    };
    let prefix = format!("{label} API error");
    let api_key = match client_api_key(req) {
        Ok(k) => k,
        Err(m) => return ctx.failed(ctx.emitter(), m),
    };
    let mut dynamic: Vec<(&str, String)> = Vec::new();
    if req.model.provider == "github-copilot" {
        dynamic.extend(copilot_headers(&req.messages));
    }
    if req.cache_retention != CacheRetention::None && !req.session_id.is_empty() {
        let sid = req.session_id.clone();
        if compat.session_affinity_format == "openrouter" {
            dynamic.push(("x-session-id", sid));
        } else {
            if compat.session_affinity_format == "openai" {
                dynamic.push(("session_id", sid.clone()));
            }
            dynamic.push(("x-client-request-id", sid));
        }
    }
    let headers = match build_headers(
        &[("authorization", format!("Bearer {api_key}"))],
        &req.model.headers,
        &dynamic,
        &req.headers,
    ) {
        Ok(h) => h,
        Err(m) => return ctx.failed(ctx.emitter(), m),
    };
    let body = match build_params(req, &compat) {
        Ok(b) => b,
        Err(m) => return ctx.failed(ctx.emitter(), m),
    };
    let url = format!("{}/responses", req.base_url().trim_end_matches('/'));
    let format_http = |text: &str| sdk_error(text, Some(&prefix));
    let (mut message, response_tier) =
        exchange(&ctx, &url, headers, &body, &format_http, "OpenAI Responses").await;
    if let Message::Assistant {
        usage,
        error_message,
        ..
    } = &mut message
    {
        let tier = response_tier.or_else(|| opt_str(req, "serviceTier").map(str::to_owned));
        apply_service_tier_pricing(
            usage,
            service_tier_multiplier(&req.model.id, tier.as_deref(), true),
        );
        if let Some(text) = error_message
            && text.contains("subscription_sharing_usage_limit_exceeded")
        {
            text.push_str(&format!("\nCheck your ChatGPT usage: {CHATGPT_USAGE_URL}"));
        }
    }
    message
}

#[cfg(test)]
mod tests {
    use pipkin_engine_protocol::{Block, Delta, Message, StopReason};
    use serde_json::{Map, Value, json};

    use super::*;
    use crate::provider::api::openai_completions::tests::{
        assistant, fragmented, parts, read_tool, request, responses_sse, run_cancelled, run_mock,
        tool_result, user,
    };
    use crate::provider::mock::MockResponse;
    use crate::provider::{Credential, Model, ThinkingLevel};

    fn model() -> Value {
        json!({"id": "gpt-5", "provider": "openai", "api": "openai-responses",
               "baseUrl": "https://api.openai.com/v1", "reasoning": true,
               "input": ["text", "image"], "maxTokens": 32000,
               "cost": {"input": 2.0, "output": 8.0}})
    }

    fn body(req: &ProviderRequest) -> Value {
        build_params(req, &Compat::resolve(req)).unwrap()
    }

    fn reasoning_item() -> Value {
        json!({"type": "reasoning", "id": "rs_1", "summary": [{"type": "summary_text", "text": "Plan A"}],
               "encrypted_content": "ENC"})
    }

    #[test]
    fn full_request_body_with_replay() {
        let mut req = request(model());
        let m = req.model.clone();
        req.system = "Be brief.".into();
        req.thinking = ThinkingLevel::High;
        req.session_id = "sess".into();
        req.max_tokens = Some(8);
        req.tools = vec![read_tool()];
        let mut text_extra = Map::new();
        text_extra.insert(
            "textSignature".into(),
            json!(r#"{"v":1,"id":"msg_abc","phase":"commentary"}"#),
        );
        req.messages = vec![
            user(vec![Block::text("look"), Block::image("image/png", "AAAA")]),
            assistant(
                &m,
                vec![
                    Block::Thinking {
                        thinking: "Plan A".into(),
                        signature: Some(reasoning_item().to_string()),
                        redacted: false,
                        extra: Map::new(),
                    },
                    Block::Text {
                        text: "Reading.".into(),
                        extra: text_extra,
                    },
                    Block::tool_call("call_1|fc_1", "read", json!({"path": "a"})),
                ],
                StopReason::ToolUse,
            ),
            tool_result(
                "call_1|fc_1",
                "read",
                vec![Block::text("img:"), Block::image("image/png", "BBBB")],
            ),
            assistant(&m, vec![Block::text("Done.")], StopReason::Stop),
        ];
        assert_eq!(
            body(&req),
            json!({
                "model": "gpt-5",
                "stream": true,
                "store": false,
                "prompt_cache_key": "sess",
                "max_output_tokens": 16,
                "reasoning": {"effort": "high", "summary": "auto"},
                "include": ["reasoning.encrypted_content"],
                "tools": [{"type": "function", "name": "read", "description": "Read a file",
                           "parameters": {"type": "object", "properties": {"path": {"type": "string"}}}}],
                "input": [
                    {"role": "developer", "content": "Be brief."},
                    {"role": "user", "content": [
                        {"type": "input_text", "text": "look"},
                        {"type": "input_image", "detail": "auto", "image_url": "data:image/png;base64,AAAA"}
                    ]},
                    reasoning_item(),
                    {"type": "message", "role": "assistant", "status": "completed", "id": "msg_abc", "phase": "commentary",
                     "content": [{"type": "output_text", "text": "Reading.", "annotations": []}]},
                    {"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "read", "arguments": "{\"path\":\"a\"}"},
                    {"type": "function_call_output", "call_id": "call_1", "output": [
                        {"type": "input_text", "text": "img:"},
                        {"type": "input_image", "detail": "auto", "image_url": "data:image/png;base64,BBBB"}
                    ]},
                    {"type": "message", "role": "assistant", "status": "completed", "id": "msg_pi_3",
                     "content": [{"type": "output_text", "text": "Done.", "annotations": []}]}
                ]
            })
        );
    }

    #[test]
    fn cache_retention_effort_mapping_and_chatgpt_sign_in() {
        let mut m = model();
        m["thinkingLevelMap"] = json!({"off": "minimal", "low": "low-native"});
        let mut req = request(m);
        req.session_id = "s".into();
        req.cache_retention = CacheRetention::Long;
        req.options.insert("temperature".into(), json!(0.3));
        req.options.insert("serviceTier".into(), json!("flex"));
        let b = body(&req);
        assert_eq!(b["reasoning"], json!({"effort": "minimal"}));
        assert!(b.get("include").is_none());
        assert_eq!(b["prompt_cache_retention"], "24h");
        assert_eq!(b["temperature"], 0.3);
        assert_eq!(b["service_tier"], "flex");
        req.thinking = ThinkingLevel::Low;
        assert_eq!(body(&req)["reasoning"]["effort"], "low-native");

        // Explicit cache mode replaces retention with options.
        req.model
            .compat
            .insert("supportsExplicitPromptCacheMode".into(), json!(true));
        let b = body(&req);
        assert!(b.get("prompt_cache_retention").is_none());
        assert_eq!(b["prompt_cache_options"], json!({"ttl": "30m"}));
        req.cache_retention = CacheRetention::None;
        let b = body(&req);
        assert_eq!(b["prompt_cache_options"], json!({"mode": "explicit"}));
        assert!(b.get("prompt_cache_key").is_none());

        // An access token that is not an API key omits unsupported fields.
        req.credential = Credential::OAuth {
            access: "eyJ-token".into(),
            metadata: Map::new(),
        };
        req.cache_retention = CacheRetention::Long;
        let b = body(&req);
        for k in [
            "prompt_cache_options",
            "prompt_cache_retention",
            "max_output_tokens",
            "temperature",
        ] {
            assert!(b.get(k).is_none(), "{k}");
        }
        assert_eq!(b["prompt_cache_key"], "s");
    }

    #[test]
    fn provider_specific_reasoning_rules() {
        let mut req = request(
            json!({"id": "gpt-5", "provider": "github-copilot", "api": "openai-responses", "reasoning": true}),
        );
        assert!(
            body(&req).get("reasoning").is_none(),
            "copilot never sends an off effort"
        );
        req.options
            .insert("reasoningSummary".into(), json!("detailed"));
        assert_eq!(
            body(&req)["reasoning"],
            json!({"effort": "medium", "summary": "detailed"})
        );

        let req = request(
            json!({"id": "grok-4", "provider": "xai", "api": "openai-responses", "reasoning": true,
                                  "thinkingLevelMap": {"off": null}}),
        );
        let b = body(&req);
        assert!(b.get("reasoning").is_none());
        assert_eq!(b["include"], json!(["reasoning.encrypted_content"]));

        let mut req = request(
            json!({"id": "m", "provider": "openai", "api": "openai-responses", "reasoning": false,
                                      "compat": {"supportsStrictMode": true, "supportsMaxOutputTokens": false}}),
        );
        req.system = "s".into();
        req.tools = vec![read_tool()];
        let b = body(&req);
        assert_eq!(b["input"][0]["role"], "system");
        assert_eq!(b["tools"][0]["strict"], false);
        assert!(b.get("max_output_tokens").is_none());
        assert!(b.get("reasoning").is_none());
    }

    #[test]
    fn foreign_and_cross_model_tool_calls_are_normalized() {
        let mut req = request(model());
        let codex: Model = serde_json::from_value(json!({"id": "gpt-5-codex", "provider": "openai-codex", "api": "openai-codex-responses"})).unwrap();
        let sibling: Model = serde_json::from_value(
            json!({"id": "gpt-4.1", "provider": "openai", "api": "openai-responses"}),
        )
        .unwrap();
        let mut sig_extra = Map::new();
        sig_extra.insert("textSignature".into(), json!("x".repeat(70)));
        req.messages = vec![
            user(vec![Block::text("q")]),
            assistant(
                &codex,
                vec![Block::tool_call("call+1|item/zz", "read", json!({}))],
                StopReason::ToolUse,
            ),
            tool_result("call+1|item/zz", "read", vec![Block::text("r")]),
            assistant(
                &sibling,
                vec![
                    Block::Text {
                        text: "t".into(),
                        extra: sig_extra,
                    },
                    Block::tool_call("call_2|fc_2", "read", json!({})),
                ],
                StopReason::ToolUse,
            ),
            tool_result("call_2|fc_2", "read", vec![]),
        ];
        let b = body(&req);
        let input = b["input"].as_array().unwrap();
        let expected_item = format!("fc_{}", crate::provider::api::common::short_hash("item/zz"));
        assert_eq!(
            input[1],
            json!({"type": "function_call", "id": expected_item, "call_id": "call_1", "name": "read", "arguments": "{}"})
        );
        assert_eq!(input[2]["call_id"], "call_1");
        // Same provider/api, different model: text keeps no signature (cross-model), and the
        // item id is dropped to avoid reasoning pairing validation.
        assert_eq!(input[3]["id"], "msg_pi_3");
        assert_eq!(
            input[4],
            json!({"type": "function_call", "call_id": "call_2", "name": "read", "arguments": "{}"})
        );
        assert_eq!(
            input[5],
            json!({"type": "function_call_output", "call_id": "call_2", "output": "(no tool output)"})
        );
    }

    fn stream_events() -> Vec<Value> {
        vec![
            json!({"type": "response.created", "response": {"id": "resp_1", "status": "in_progress"}}),
            json!({"type": "response.output_item.added", "output_index": 0, "item": {"type": "reasoning", "id": "rs_1", "summary": []}}),
            json!({"type": "response.reasoning_summary_text.delta", "output_index": 0, "delta": "Plan "}),
            json!({"type": "response.reasoning_summary_text.delta", "output_index": 0, "delta": "\u{e9}\u{1f600}"}),
            json!({"type": "response.reasoning_summary_part.done", "output_index": 0}),
            json!({"type": "response.output_item.done", "output_index": 0, "item":
                {"type": "reasoning", "id": "rs_1", "summary": [{"type": "summary_text", "text": "Plan \u{e9}\u{1f600}"}], "encrypted_content": "ENC"}}),
            json!({"type": "response.output_item.added", "output_index": 1, "item": {"type": "message", "id": "msg_1", "role": "assistant", "content": []}}),
            json!({"type": "response.output_text.delta", "output_index": 1, "delta": "Hel"}),
            json!({"type": "response.output_text.delta", "output_index": 1, "delta": "lo"}),
            json!({"type": "response.output_item.done", "output_index": 1, "item":
                {"type": "message", "id": "msg_1", "role": "assistant", "phase": "commentary",
                 "content": [{"type": "output_text", "text": "Hello"}]}}),
            json!({"type": "response.output_item.added", "output_index": 2, "item": {"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "read", "arguments": ""}}),
            json!({"type": "response.function_call_arguments.delta", "output_index": 2, "delta": "{\"path\":"}),
            json!({"type": "response.function_call_arguments.delta", "output_index": 2, "delta": "\"a.rs\"}"}),
            json!({"type": "response.function_call_arguments.done", "output_index": 2, "arguments": "{\"path\":\"a.rs\"}"}),
            json!({"type": "response.output_item.done", "output_index": 2, "item":
                {"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "read", "arguments": "{\"path\":\"a.rs\"}"}}),
            json!({"type": "response.completed", "response": {"id": "resp_1", "status": "completed", "service_tier": "default",
                "usage": {"input_tokens": 1000, "output_tokens": 50, "total_tokens": 1050,
                          "input_tokens_details": {"cached_tokens": 400}, "output_tokens_details": {"reasoning_tokens": 20}}}}),
        ]
    }

    #[tokio::test]
    async fn fragmented_stream_round_trips_through_replay() {
        let sse = responses_sse(&stream_events());
        for step in [1, 9, 4096] {
            let mut req = request(model());
            req.thinking = ThinkingLevel::High;
            req.session_id = "sess".into();
            let run = run_mock(adapter(), req.clone(), "/v1", vec![fragmented(&sse, step)]).await;
            let (content, stop, error, extra) = parts(&run.message);
            assert_eq!(error, None, "step {step}");
            assert_eq!(stop, StopReason::ToolUse);
            assert_eq!(extra["responseId"], "resp_1");
            assert_eq!(extra["rawStopReason"], "completed");
            let Block::Thinking {
                thinking,
                signature,
                ..
            } = &content[0]
            else {
                panic!()
            };
            assert_eq!(
                thinking, "Plan \u{e9}\u{1f600}",
                "the final summary replaces the streamed text"
            );
            let item: Value = serde_json::from_str(signature.as_deref().unwrap()).unwrap();
            assert_eq!(item["encrypted_content"], "ENC");
            let Block::Text {
                text,
                extra: text_extra,
            } = &content[1]
            else {
                panic!()
            };
            assert_eq!(text, "Hello");
            assert_eq!(
                text_extra["textSignature"],
                r#"{"v":1,"id":"msg_1","phase":"commentary"}"#
            );
            assert_eq!(
                content[2],
                Block::tool_call("call_1|fc_1", "read", json!({"path": "a.rs"}))
            );
            let Message::Assistant { usage, .. } = &run.message else {
                panic!()
            };
            assert_eq!(
                (
                    usage.input,
                    usage.output,
                    usage.cache_read,
                    usage.total_tokens
                ),
                (600, 50, 400, 1050)
            );
            assert_eq!(usage.reasoning, Some(20));
            assert!((usage.cost.input - 600.0 * 2.0 / 1e6).abs() < 1e-12);
            assert!(run.deltas.contains(&Delta::Text {
                block: 1,
                text: "Hel".into()
            }));
            assert!(run.deltas.contains(&Delta::ToolCall {
                block: 2,
                id: "call_1|fc_1".into(),
                name: "read".into()
            }));
            let r = &run.requests[0];
            assert_eq!(r.path, "/v1/responses");
            assert_eq!(r.header("authorization"), Some("Bearer sk-test"));
            assert_eq!(r.header("session_id"), Some("sess"));
            assert_eq!(r.header("x-client-request-id"), Some("sess"));
            assert_eq!(r.header("x-session-affinity"), None);

            // Replay into the next request.
            let mut next = req.clone();
            next.messages = vec![
                user(vec![Block::text("q")]),
                run.message.clone(),
                tool_result("call_1|fc_1", "read", vec![Block::text("fn main")]),
            ];
            let b = body(&next);
            let input = b["input"].as_array().unwrap();
            assert_eq!(input[1], item);
            assert_eq!(input[2]["id"], "msg_1");
            assert_eq!(input[2]["phase"], "commentary");
            assert_eq!(
                input[3],
                json!({"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "read", "arguments": "{\"path\":\"a.rs\"}"})
            );
            assert_eq!(
                input[4],
                json!({"type": "function_call_output", "call_id": "call_1", "output": "fn main"})
            );
        }
    }

    #[tokio::test]
    async fn encrypted_reasoning_is_backfilled_from_the_completed_response() {
        let events = vec![
            json!({"type": "response.output_item.added", "output_index": 0, "item": {"type": "reasoning", "id": "rs_9"}}),
            json!({"type": "response.output_item.done", "output_index": 0, "item": {"type": "reasoning", "id": "rs_9", "summary": []}}),
            json!({"type": "response.output_item.added", "output_index": 1, "item": {"type": "message", "id": "m", "phase": "final_answer"}}),
            json!({"type": "response.output_text.delta", "output_index": 1, "delta": "ok"}),
            json!({"type": "response.output_item.done", "output_index": 1, "item": {"type": "message", "id": "m", "content": [{"type": "output_text", "text": "ok"}]}}),
            json!({"type": "response.completed", "response": {"status": "completed",
                "output": [{"type": "reasoning", "id": "rs_9", "encrypted_content": "LATE"}]}}),
        ];
        let run = run_mock(
            adapter(),
            request(model()),
            "",
            vec![fragmented(&responses_sse(&events), 17)],
        )
        .await;
        let (content, stop, _, _) = parts(&run.message);
        assert_eq!(stop, StopReason::Stop);
        let Block::Thinking { signature, .. } = &content[0] else {
            panic!()
        };
        let item: Value = serde_json::from_str(signature.as_deref().unwrap()).unwrap();
        assert_eq!(item["encrypted_content"], "LATE");
    }

    #[tokio::test]
    async fn incomplete_failed_and_error_events() {
        let incomplete = |reason: &str| {
            responses_sse(&[
                json!({"type": "response.output_item.added", "output_index": 0, "item": {"type": "message", "id": "m"}}),
                json!({"type": "response.output_text.delta", "output_index": 0, "delta": "cut"}),
                json!({"type": "response.incomplete", "response": {"status": "incomplete", "incomplete_details": {"reason": reason}}}),
            ])
        };
        let run = run_mock(
            adapter(),
            request(model()),
            "",
            vec![fragmented(&incomplete("max_output_tokens"), 50)],
        )
        .await;
        let (content, stop, _, extra) = parts(&run.message);
        assert_eq!(stop, StopReason::Length);
        assert_eq!(content, &[Block::text("cut")]);
        assert_eq!(extra["rawStopReason"], "incomplete.max_output_tokens");
        let run = run_mock(
            adapter(),
            request(model()),
            "",
            vec![fragmented(&incomplete("content_filter"), 50)],
        )
        .await;
        assert_eq!(
            parts(&run.message).2,
            Some("Response incomplete: content_filter")
        );

        let failed = responses_sse(&[
            json!({"type": "response.failed", "response": {"status": "failed",
            "error": {"code": "server_error", "message": "The server had an error"}}}),
        ]);
        let run = run_mock(
            adapter(),
            request(model()),
            "",
            vec![fragmented(&failed, 50)],
        )
        .await;
        let (_, stop, error, _) = parts(&run.message);
        assert_eq!(stop, StopReason::Error);
        assert_eq!(error, Some("server_error: The server had an error"));
        assert!(crate::provider::errors::is_retryable(&run.message));

        let err = responses_sse(&[
            json!({"type": "error", "code": "rate_limit_exceeded", "message": "Rate limit reached"}),
        ]);
        let run = run_mock(adapter(), request(model()), "", vec![fragmented(&err, 50)]).await;
        assert_eq!(
            parts(&run.message).2,
            Some("Error Code rate_limit_exceeded: Rate limit reached")
        );

        let truncated = responses_sse(&stream_events()[..8]);
        let run = run_mock(
            adapter(),
            request(model()),
            "",
            vec![fragmented(&truncated, 50)],
        )
        .await;
        let (_, stop, error, _) = parts(&run.message);
        assert_eq!(stop, StopReason::Error);
        assert_eq!(
            error,
            Some("OpenAI Responses stream ended before a terminal response event")
        );
        assert!(crate::provider::errors::is_retryable(&run.message));
    }

    #[tokio::test]
    async fn tool_use_with_an_unfinished_call_is_refused() {
        let events = vec![
            json!({"type": "response.output_item.added", "output_index": 0, "item": {"type": "function_call", "id": "fc_1", "call_id": "c1", "name": "read", "arguments": ""}}),
            json!({"type": "response.function_call_arguments.delta", "output_index": 0, "delta": "{\"pa"}),
            json!({"type": "response.completed", "response": {"status": "completed"}}),
        ];
        let run = run_mock(
            adapter(),
            request(model()),
            "",
            vec![fragmented(&responses_sse(&events), 50)],
        )
        .await;
        let (_, stop, error, _) = parts(&run.message);
        assert_eq!(stop, StopReason::Error);
        assert_eq!(
            error,
            Some("OpenAI Responses stream completed with an unfinished tool call: read (c1|fc_1)")
        );
    }

    #[tokio::test]
    async fn http_errors_carry_the_api_prefix_and_usage_hint() {
        let run = run_mock(
            adapter(),
            request(model()),
            "",
            vec![MockResponse::json(
                401,
                &json!({"error": {"message": "Incorrect API key", "code": "invalid_api_key"}}),
            )],
        )
        .await;
        let error = parts(&run.message).2.unwrap().to_owned();
        assert!(error.starts_with("OpenAI API error (401): {"), "{error}");
        let mut m = model();
        m["provider"] = json!("opencode");
        let run = run_mock(
            adapter(),
            request(m),
            "",
            vec![MockResponse::json(
                429,
                &json!({"error": {"message": "subscription_sharing_usage_limit_exceeded"}}),
            )],
        )
        .await;
        let error = parts(&run.message).2.unwrap();
        assert!(error.starts_with("opencode API error (429): "), "{error}");
        assert!(error.ends_with("\nCheck your ChatGPT usage: https://chatgpt.com/settings/usage"));
        assert!(!crate::provider::errors::is_retryable(&run.message));
    }

    #[tokio::test]
    async fn service_tier_pricing_and_openrouter_session_header() {
        let mut events = stream_events();
        events.last_mut().unwrap()["response"]["service_tier"] = json!("flex");
        let mut req = request(
            json!({"id": "gpt-5", "provider": "openrouter", "api": "openai-responses", "reasoning": true,
                                      "cost": {"input": 2.0, "output": 8.0}}),
        );
        req.session_id = "s9".into();
        let run = run_mock(
            adapter(),
            req,
            "",
            vec![fragmented(&responses_sse(&events), 333)],
        )
        .await;
        let Message::Assistant { usage, .. } = &run.message else {
            panic!()
        };
        assert!((usage.cost.input - 600.0 * 2.0 / 1e6 * 0.5).abs() < 1e-12);
        assert!((usage.cost.total - (usage.cost.input + usage.cost.output)).abs() < 1e-12);
        assert_eq!(run.requests[0].header("x-session-id"), Some("s9"));
        assert_eq!(run.requests[0].header("session_id"), None);
    }

    #[tokio::test]
    async fn cancellation_mid_stream_is_aborted() {
        let body = responses_sse(&stream_events()[..8]);
        let message = run_cancelled(adapter(), request(model()), body).await;
        let (content, stop, error, _) = parts(&message);
        assert_eq!(stop, StopReason::Aborted);
        assert_eq!(error, None);
        assert!(matches!(content[0], Block::Thinking { .. }));
    }

    /// Reference `clampMaxTokensToContext` (`simple-options.ts:18-22`) through
    /// `buildBaseOptions`: 10000 window - 1100 estimated - 4096 safety = 4804.
    fn crowd_context(req: &mut ProviderRequest) {
        req.system = "x".repeat(4000);
        req.messages = vec![Message::user_text("y".repeat(400), 1)];
    }

    #[test]
    fn max_output_tokens_are_clamped_to_the_context_window() {
        let mut req = request(json!({"id": "m", "provider": "acme", "api": API,
            "baseUrl": "https://acme.test/v1", "contextWindow": 10000, "maxTokens": 32000}));
        crowd_context(&mut req);
        assert_eq!(body(&req)["max_output_tokens"], json!(4804));
    }
}
