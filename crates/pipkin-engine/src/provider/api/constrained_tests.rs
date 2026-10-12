//! Executed-reference declarations and end-to-end Responses grammar tool exchanges.
use super::common::schema::resolve_grammar;
use super::openai_completions::tests::{fragmented, request, responses_sse, run_mock, tool_result};
use super::openai_responses_shared::{ResponsesStream, convert_request_messages, convert_tools};
use crate::provider::{ApiAdapter, Emitter, ToolSpec};
use pipkin_engine_protocol::{Block, Message, StopReason, SystemMessage, ToolDeclaration};
use serde_json::{Value, json};
use std::sync::Arc;

pub(crate) fn grammar_tool() -> ToolSpec {
    ToolSpec {
        name: "patch".into(),
        description: "Apply a patch".into(),
        parameters: json!({"type":"object","properties":{"patch":{"type":"string"}},"required":["patch"]}),
        constrained_sampling: Some(
            json!({"type":"grammar","variants":{"openai_lark":"start: /.+/s"}}),
        ),
    }
}

#[test]
fn constrained_declarations_match_executed_pinned_reference() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../../fixtures/native-system/constrained-reference.json"
    ))
    .unwrap();
    assert_eq!(
        fixture["reference"],
        "d2a311097cbcf669e699479587332ae3988a49d0"
    );
    for case in fixture["cases"].as_array().unwrap() {
        let declaration: ToolDeclaration = serde_json::from_value(case["tool"].clone()).unwrap();
        let tool = ToolSpec::from(&declaration);
        assert_eq!(ToolDeclaration::from(&tool), declaration);
        let options = &case["options"];
        let mut req = request(
            json!({"id":"test","api":"openai-responses","provider":"openai",
            "compat":{"supportsOpenAIGrammarTools":options["supportsOpenAIGrammarTools"].as_bool().unwrap_or(false)}}),
        );
        req.tools = vec![tool.clone()];
        let strict = if options.get("strict").is_some_and(Value::is_null) {
            None
        } else {
            Some(options["strict"].as_bool().unwrap_or(false))
        };
        let result = convert_tools(
            &req,
            &req.tools,
            options["supportsStrictMode"].as_bool().unwrap_or(true),
            strict,
        );
        if let Some(error) = case["error"].as_str() {
            assert_eq!(result.unwrap_err(), error, "{}", case["name"]);
        } else {
            assert_eq!(json!(result.unwrap()), case["tools"], "{}", case["name"]);
            let grammar = resolve_grammar(&req, &tool).unwrap().map(|g| json!({"format":g.format,"definition":g.definition,"inputProperty":g.input_property}));
            assert_eq!(json!(grammar), case["grammar"], "{}", case["name"]);
        }
    }
}

fn grammar_request(api: &str) -> crate::provider::ProviderRequest {
    let provider = if api == "openai-codex-responses" {
        "openai-codex"
    } else {
        "openai"
    };
    let mut req = request(json!({"id":"gpt-test","api":api,"provider":provider,
        "compat":{"supportsOpenAIGrammarTools":true}}));
    req.tools = vec![grammar_tool()];
    if api == "openai-codex-responses" {
        req.options.insert("transport".into(), json!("sse"));
        req.options.insert("retryBaseDelayMs".into(), json!(1));
        // Synthetic JWT: the adapter needs account metadata, not an external account.
        req.credential = crate::provider::Credential::ApiKey("x.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoidGVzdCJ9fQ.x".into());
    }
    req
}

pub(crate) fn grammar_events(input: &str) -> Vec<Value> {
    vec![
        json!({"type":"response.output_item.added","output_index":0,"item":{"type":"custom_tool_call","name":"patch","id":"ctc_one","call_id":"call_one","input":""}}),
        json!({"type":"response.custom_tool_call_input.delta","output_index":0,"delta":input}),
        json!({"type":"response.custom_tool_call_input.done","output_index":0,"input":input}),
        json!({"type":"response.output_item.done","output_index":0,"item":{"type":"custom_tool_call","name":"patch","id":"ctc_one","call_id":"call_one","input":input}}),
        json!({"type":"response.completed","response":{"status":"completed","output":[]}}),
    ]
}

#[tokio::test]
async fn grammar_streams_and_replays_across_all_responses_adapters() {
    let input = "*** Begin Patch\n+é😀\\\"\n*** End Patch";
    let adapters: Vec<Arc<dyn ApiAdapter>> = vec![
        super::openai_responses::adapter(),
        super::azure_openai_responses::adapter(),
        super::openai_codex_responses::adapter(),
    ];
    for adapter in adapters {
        let mut req = grammar_request(adapter.api());
        let run = run_mock(
            adapter,
            req.clone(),
            "/v1",
            vec![fragmented(&responses_sse(&grammar_events(input)), 3)],
        )
        .await;
        let Message::Assistant {
            stop_reason,
            error_message,
            ..
        } = &run.message
        else {
            panic!()
        };
        assert_eq!(
            *stop_reason,
            StopReason::ToolUse,
            "{}: {error_message:?}",
            req.model.api
        );
        assert_eq!(run.requests.len(), 1);
        let recorded = &run.requests[0];
        let body = if recorded.header("content-encoding") == Some("zstd") {
            serde_json::from_slice::<Value>(&zstd::decode_all(&recorded.body[..]).unwrap()).unwrap()
        } else {
            recorded.json()
        };
        assert_eq!(body["tools"][0]["type"], "custom");
        assert!(
            matches!(&run.message.content()[0],Block::ToolCall {arguments,..} if *arguments==json!({"patch":input}))
        );
        // Persisted/reloaded call retains the provider id and original argument property.
        let stored = serde_json::to_vec(&run.message).unwrap();
        req.messages = vec![
            serde_json::from_slice(&stored).unwrap(),
            tool_result("call_one|ctc_one", "patch", vec![Block::text("applied")]),
        ];
        let replay = convert_request_messages(&req, false, &["openai", "openai-codex"]).unwrap();
        assert_eq!(
            replay[0],
            json!({"type":"custom_tool_call","id":"ctc_one","call_id":"call_one","name":"patch","input":input})
        );
        assert_eq!(replay[1]["type"], "custom_tool_call_output");
        // Moving to an endpoint without grammar support preserves the executable JSON call.
        req.model
            .compat
            .insert("supportsOpenAIGrammarTools".into(), json!(false));
        let fallback = convert_request_messages(&req, false, &["openai", "openai-codex"]).unwrap();
        assert_eq!(fallback[0]["type"], "function_call");
        assert!(fallback[0].get("id").is_none());
        assert_eq!(
            serde_json::from_str::<Value>(fallback[0]["arguments"].as_str().unwrap()).unwrap(),
            json!({"patch":input})
        );
        assert_eq!(fallback[1]["type"], "function_call_output");
        req.model
            .compat
            .insert("supportsOpenAIGrammarTools".into(), json!(true));
        req.model.id = "another-model".into();
        let cross_model =
            convert_request_messages(&req, false, &["openai", "openai-codex"]).unwrap();
        assert_eq!(cross_model[0]["type"], "custom_tool_call");
        assert!(cross_model[0].get("id").is_none());
        if let Message::Assistant { content, .. } = &mut req.messages[0]
            && let Block::ToolCall { arguments, .. } = &mut content[0]
        {
            *arguments = json!({"input": "wrong property"});
        }
        assert!(
            convert_request_messages(&req, false, &["openai", "openai-codex"])
                .unwrap_err()
                .contains("requires argument \"patch\" to be a string")
        );
    }
}

#[test]
fn grammar_stream_rejects_rewrites_and_preserves_interrupted_input() {
    let req = grammar_request("openai-responses");
    let events = grammar_events("first");
    let mut emitter = Emitter::new(None);
    let mut state = ResponsesStream::for_request(&req).unwrap();
    state.handle(&mut emitter, &events[0]).unwrap();
    state.handle(&mut emitter, &events[1]).unwrap();
    assert!(state.handle(&mut emitter,&json!({"type":"response.custom_tool_call_input.done","output_index":0,"input":"replaced"})).unwrap_err().contains("non-monotonically"));
    state.settle(&mut emitter);
    assert!(
        matches!(&emitter.content()[0],Block::ToolCall {arguments,..} if *arguments==json!({"patch":"first"}))
    );
    let mut emitter = Emitter::new(None);
    let mut state = ResponsesStream::for_request(&req).unwrap();
    for e in &events[..3] {
        state.handle(&mut emitter, e).unwrap();
    }
    assert!(state.handle(&mut emitter,&json!({"type":"response.custom_tool_call_input.delta","output_index":0,"delta":"more"})).unwrap_err().contains("after it was closed"));
}

#[test]
fn positional_grammar_anchors_preserve_declarations_and_input_properties() {
    let mut req = grammar_request("openai-responses");
    req.model
        .compat
        .insert("supportsMidConvoSystemMessages".into(), json!(true));
    req.model
        .compat
        .insert("supportsAdditionalTools".into(), json!(true));
    req.messages = vec![Message::System(SystemMessage {
        tools_added: vec![ToolDeclaration::from(&grammar_tool())],
        ..Default::default()
    })];
    // A later grammar tool is not in the initial top-level declarations.
    req.messages.insert(
        0,
        Message::System(SystemMessage {
            tools_added: vec![ToolDeclaration::from(
                &super::openai_completions::tests::read_tool(),
            )],
            ..Default::default()
        }),
    );
    let resolved = req.resolve_systems(true);
    let input = convert_request_messages(&resolved, true, &["openai"]).unwrap();
    assert_eq!(input[0]["tools"][0]["type"], "custom");
    assert_eq!(
        super::openai_responses_shared::grammar_properties(&resolved).unwrap()["patch"],
        "patch"
    );
    req.model
        .compat
        .insert("supportsAdditionalTools".into(), json!(false));
    req.model
        .compat
        .insert("supportsToolSearch".into(), json!(true));
    let resolved = req.resolve_systems(true);
    let input = convert_request_messages(&resolved, true, &["openai"]).unwrap();
    assert_eq!(input[1]["tools"][0]["type"], "custom");
    assert_eq!(input[1]["tools"][0]["defer_loading"], true);
}

#[test]
fn strict_tool_metadata_reaches_completions_mistral_and_google() {
    let mut req = grammar_request("openai-completions");
    req.tools[0].constrained_sampling = Some(json!({"type":"json_schema","strict":"require"}));
    req.model
        .compat
        .insert("supportsStrictMode".into(), json!(true));
    let compat = super::openai_completions::Compat::resolve(&req.model, req.base_url());
    let body = super::openai_completions::build_params(&req, &compat).unwrap();
    assert_eq!(body["tools"][0]["function"]["strict"], true);
    assert_eq!(
        body["tools"][0]["function"]["parameters"]["additionalProperties"],
        false
    );
    let body = super::mistral_conversations::build_payload(&req).unwrap();
    assert_eq!(body["tools"][0]["function"]["strict"], true);
    let google = super::google_shared::convert_tools(&req, true)
        .unwrap()
        .unwrap();
    assert!(google.to_string().contains("additionalProperties"));
    req.model
        .compat
        .insert("supportsStrictMode".into(), json!(false));
    let compat = super::openai_completions::Compat::resolve(&req.model, req.base_url());
    assert!(
        super::openai_completions::build_params(&req, &compat)
            .unwrap_err()
            .contains("strict tools are unsupported")
    );
}
