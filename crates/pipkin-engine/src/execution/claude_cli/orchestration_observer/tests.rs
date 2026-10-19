use super::*;
use crate::store::{Store, cli_agent, cli_agent_turns};
use serde_json::{Value, json};
const UUID: &str = "11111111-1111-4111-8111-111111111111";
fn fixture() -> (LaunchContext, Vec<ToolSpec>, [u8; 32]) {
    let s = Store::memory().unwrap();
    let b = s
        .create_cli_agent_session(
            &cli_agent::Create {
                session: "s".into(),
                request_key: "create".into(),
                model: "sonnet".into(),
                thinking: None,
                parent: None,
                identity: crate::store::CliBindingSpec {
                    cli_session_id: UUID.into(),
                    project_path: "/fixture".into(),
                    executable_path: "/fixture/claude".into(),
                    cli_version: "2.1.301".into(),
                    config_ref: "fixture".into(),
                    account_scope: "fixture".into(),
                    launch_policy: "cli-native-orchestration-v1".into(),
                },
            },
            1,
        )
        .unwrap()
        .0;
    let l = s
        .admit_cli_agent_turn(
            &cli_agent_turns::Input {
                images: &[],
                mode: pipkin_engine_protocol::InputMode::Prompt,
                session: "s",
                request: "r",
                generation: 1,
                operation: "op",
                binding: b.digest,
                text: "private prompt",
            },
            "owner",
            2,
        )
        .unwrap()
        .lease
        .unwrap();
    let context = s.cli_agent_launch_context(&l).unwrap();
    let tools = vec![ToolSpec {
        name: "write".into(),
        description: "native Write".into(),
        parameters: json!({"type":"object"}),
        constrained_sampling: None,
    }];
    let hash = Session::new(context.scope().clone(), &tools)
        .unwrap()
        .catalog_hash();
    (context, tools, hash)
}
fn records(values: Vec<Value>) -> Vec<u8> {
    values
        .into_iter()
        .map(|mut v| {
            v["session_id"] = json!(UUID);
            format!("{v}\n")
        })
        .collect::<String>()
        .into_bytes()
}
fn init() -> Value {
    json!({"type":"system","subtype":"init","tools":["mcp__pipkin__write"]})
}
fn terminal() -> Value {
    json!({"type":"result","subtype":"success","is_error":false,"result":"done"})
}
fn ignore(_: &Scope, _: Observation) -> Result<(), ()> {
    Ok(())
}
#[test]
fn exact_catalog_rejects_omissions_builtins_other_servers_and_duplicates() {
    let (context, tools, hash) = fixture();
    for inventory in [
        json!([]),
        json!(["Write"]),
        json!(["mcp__other__write"]),
        json!(["mcp__pipkin__write", "Read"]),
        json!(["mcp__pipkin__write", "mcp__pipkin__write"]),
    ] {
        let mut observer = Observer::new(&context, &tools, hash).unwrap();
        let mut i = init();
        i["tools"] = inventory;
        assert_eq!(
            observer.feed(context.scope(), &records(vec![i]), ignore),
            Err(Error::UnsupportedTool)
        );
        assert_eq!(observer.finish(ignore), Err(Error::UnsupportedTool));
    }
}
#[test]
fn catalog_hash_and_invalid_catalog_are_rejected_before_observation() {
    let (context, tools, hash) = fixture();
    assert!(matches!(
        Observer::new(&context, &tools, [0; 32]),
        Err(Error::InvalidConfiguration)
    ));
    assert!(matches!(
        Observer::new(&context, &[], hash),
        Err(Error::InvalidConfiguration)
    ));
    let duplicate = vec![tools[0].clone(), tools[0].clone()];
    assert!(matches!(
        Observer::new(&context, &duplicate, hash),
        Err(Error::InvalidConfiguration)
    ));
}
#[test]
fn fragmented_native_stdout_emits_only_scoped_display_and_never_duplicate_tool_cards() {
    let (context, tools, hash) = fixture();
    let mut o = Observer::new(&context, &tools, hash).unwrap();
    let scope = context.scope();
    assert_eq!(o.catalog_hash(), hash);
    assert_eq!(o.scope(), scope);
    let bytes = records(vec![
        init(),
        json!({"type":"assistant","message":{"id":"m","role":"assistant","content":[{"type":"tool_use","id":"sdk-tool-id","name":"mcp__pipkin__write","input":{"path":"DO_NOT_EXECUTE"}},{"type":"text","text":"done"}]}}),
        json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"sdk-tool-id","is_error":false,"content":"PRIVATE_NATIVE_TOOL_RESULT"}]}}),
        terminal(),
    ]);
    let mut observed = Vec::new();
    for byte in bytes {
        o.feed(scope, &[byte], |s, event| {
            assert_eq!(s, scope);
            observed.push(event);
            Ok::<_, ()>(())
        })
        .unwrap();
    }
    assert_eq!(o.finish(ignore).unwrap().outcome, Some(Outcome::Success));
    assert_eq!(observed.len(), 4);
    assert!(matches!(
        &observed[1],
        Observation::Context {
            tokens: None,
            compact_pre_tokens: None
        }
    ));
    assert!(matches!(&observed[2],Observation::Snapshot {text,..} if text=="done"));
    assert!(!format!("{observed:?}").contains("done"));
}
#[test]
fn stale_scope_does_not_consume_bytes_but_wrong_uuid_and_sdk_children_fail_closed() {
    let (context, tools, hash) = fixture();
    let mut o = Observer::new(&context, &tools, hash).unwrap();
    let mut stale = context.scope().clone();
    stale.generation += 1;
    assert_eq!(
        o.feed(&stale, &records(vec![init()]), ignore),
        Err(Error::Identity)
    );
    assert_eq!(o.summary().records, 0);
    o.feed(context.scope(), &records(vec![init(), terminal()]), ignore)
        .unwrap();
    assert_eq!(o.finish(ignore).unwrap().outcome, Some(Outcome::Success));
    for extra in [
        json!({"session_id":"22222222-2222-4222-8222-222222222222"}),
        json!({"parent_tool_use_id":"sdk-child"}),
    ] {
        let mut o = Observer::new(&context, &tools, hash).unwrap();
        let mut i = init();
        i["session_id"] = json!(UUID);
        for (k, v) in extra.as_object().unwrap() {
            i[k] = v.clone();
        }
        assert_eq!(
            o.feed(context.scope(), format!("{i}\n").as_bytes(), ignore),
            Err(Error::Identity)
        );
    }
}
#[test]
fn terminal_is_not_eof_success_and_trailing_contradictions_still_fail() {
    let (context, tools, hash) = fixture();
    let mut o = Observer::new(&context, &tools, hash).unwrap();
    o.feed(context.scope(), &records(vec![init()]), ignore)
        .unwrap();
    assert_eq!(o.finish(ignore), Err(Error::Incomplete));
    let mut o = Observer::new(&context, &tools, hash).unwrap();
    o.feed(context.scope(), &records(vec![init(), terminal()]), ignore)
        .unwrap();
    assert_eq!(
        o.feed(context.scope(), &records(vec![terminal()]), ignore),
        Err(Error::Sequence)
    );
    assert_eq!(o.finish(ignore), Err(Error::Sequence));
}
#[test]
fn sink_failure_remains_fatal_and_terminal_without_newline_is_scoped() {
    let (context, tools, hash) = fixture();
    let mut o = Observer::new(&context, &tools, hash).unwrap();
    assert_eq!(
        o.feed(context.scope(), &records(vec![init()]), |_, _| Err(())),
        Err(Error::SinkRejected)
    );
    let mut o = Observer::new(&context, &tools, hash).unwrap();
    let mut bytes = records(vec![init(), terminal()]);
    bytes.pop();
    o.feed(context.scope(), &bytes, ignore).unwrap();
    let mut terminal_seen = false;
    assert_eq!(
        o.finish(|scope, event| {
            assert_eq!(scope, context.scope());
            terminal_seen = matches!(event, Observation::Terminal { .. });
            Ok::<_, ()>(())
        })
        .unwrap()
        .outcome,
        Some(Outcome::Success)
    );
    assert!(terminal_seen);
}

#[test]
fn parity_large_sdk_echoes_are_inert_and_do_not_spend_native_display_budget() {
    let (context, tools, hash) = fixture();
    let mut observer = Observer::new(&context, &tools, hash).unwrap();
    let scope = context.scope();
    let text = "display λ ".repeat(180_000);
    assert!(text.len() > events::MAX_PROJECTED_BYTES);
    let bytes = records(vec![
        init(),
        json!({"type":"assistant","message":{"id":"m","role":"assistant","content":[{"type":"tool_use","id":"sdk-large","name":"mcp__pipkin__write","input":{"path":"NEVER_EXECUTE","content":"x".repeat(1024*1024)}},{"type":"text","text":text}]}}),
        json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"sdk-large","is_error":false,"content":"r".repeat(2*1024*1024)}]}}),
        terminal(),
    ]);
    let mut snapshots = Vec::new();
    observer
        .feed(scope, &bytes, |_, event| {
            if let Observation::Snapshot { text, .. } = event {
                snapshots.push(text);
            }
            Ok::<_, ()>(())
        })
        .unwrap();
    assert!(snapshots.contains(&text));
    assert!(!observer.summary().presentation_incomplete);
    assert!(observer.summary().projected_bytes < 3 * 1024 * 1024);
    assert_eq!(
        observer.finish(ignore).unwrap().outcome,
        Some(Outcome::Success)
    );
    // Expanded line allowance still has a hard bound before parsing or observation.
    let mut observer = Observer::new(&context, &tools, hash).unwrap();
    assert_eq!(
        observer.feed(
            scope,
            &vec![b' '; super::super::orchestration_policy::MAX_STREAM_LINE_BYTES + 1],
            ignore
        ),
        Err(Error::Limit)
    );
}

#[test]
fn features_usage_metadata_is_bounded_optional_and_never_double_counted() {
    let (context, tools, hash) = fixture();
    let mut observer = Observer::new(&context, &tools, hash).unwrap();
    let mut result = terminal();
    result["usage"] = json!({"input_tokens":12,"output_tokens":20,"cache_read_input_tokens":3,"cache_creation_input_tokens":10,"cache_creation":{"ephemeral_1h_input_tokens":4},"output_tokens_details":{"thinking_tokens":15},"account":"private"});
    result["duration_ms"] = json!(123);
    result["duration_api_ms"] = json!(100);
    result["num_turns"] = json!(2);
    result["total_cost_usd"] = json!(0.125);
    result["stop_reason"] = json!("end_turn");
    result["modelUsage"] = json!({"claude-sonnet-fixture":{"inputTokens":1000,"contextWindow":200000,"maxOutputTokens":64000,"costUSD":0.125,"provider":"firstParty","secret":"private"}});
    let bytes = records(vec![init(), result]);
    let mut terminal = None;
    for byte in bytes {
        observer
            .feed(context.scope(), &[byte], |_, event| {
                if let Observation::Terminal {
                    usage, metadata, ..
                } = event
                {
                    terminal = Some((usage, metadata));
                }
                Ok::<_, ()>(())
            })
            .unwrap();
    }
    observer.finish(ignore).unwrap();
    let (usage, metadata) = terminal.unwrap();
    let usage = usage.unwrap();
    assert_eq!(usage.input_tokens, Some(12)); // not the cumulative 1000
    assert_eq!(usage.reasoning_tokens, Some(15));
    assert_eq!(usage.cache_write_1h, Some(4));
    let metadata = metadata.unwrap();
    assert_eq!(metadata.invocation["num_turns"], 2);
    assert_eq!(
        metadata.session_snapshot["modelUsage"]["claude-sonnet-fixture"]["inputTokens"],
        1000
    );
    assert!(
        !serde_json::to_string(&metadata)
            .unwrap()
            .contains("private")
    );
    assert!(!format!("{metadata:?}").contains("sonnet"));
}
