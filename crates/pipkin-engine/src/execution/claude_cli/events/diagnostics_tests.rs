//! Presentation compatibility and closed execution boundaries, all synthetic/offline.
use super::*;

#[test]
fn cli_owned_default_inventory_and_large_tool_previews_do_not_require_native_approvals() {
    let mut d = decoder().with_cli_tools();
    let mut events = vec![];
    feed(&mut d, &[init(&["FutureBuiltIn"])], &mut events).unwrap();
    for n in 0..100 {
        let id = format!("t{n}");
        feed(
            &mut d,
            &[assistant(
                &format!("m{n}"),
                json!([tool(&id, "FutureBuiltIn")]),
            )],
            &mut events,
        )
        .unwrap();
        feed(&mut d, &[envelope("user",json!({"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":id,"is_error":false,"content":"x".repeat(100_000)}]}}))], &mut events).unwrap();
    }
    feed(
        &mut d,
        &[assistant("final", json!([text("done")])), terminal()],
        &mut events,
    )
    .unwrap();
    let summary = d.finish(|_| Ok::<_, ()>(())).unwrap();
    assert_eq!(summary.outcome, Some(Outcome::Success));
    assert!(summary.presentation_incomplete);
    assert!(summary.projected_bytes < MAX_PROJECTED_BYTES);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::ExternalToolResult { .. }))
            .count(),
        100
    );
    assert!(
        events
            .iter()
            .all(|e| !matches!(e, Event::ExternalToolResult { text, .. } if text.len() > 8192))
    );
}

#[test]
fn a_nested_cli_result_cannot_complete_the_root_turn() {
    let mut d = decoder().with_cli_tools();
    let mut child = terminal();
    child["parent_tool_use_id"] = json!("agent");
    child["session_id"] = json!(OTHER);
    feed(
        &mut d,
        &[
            init(&["Agent"]),
            assistant("m", json!([tool("agent", "Agent")])),
            child,
        ],
        &mut vec![],
    )
    .unwrap();
    assert_eq!(d.finish(|_| Ok::<_, ()>(())), Err(Error::Incomplete));
}

#[test]
fn correlated_child_telemetry_can_drain_after_root_completion_but_unknown_parents_cannot() {
    let mut d = decoder().with_cli_tools();
    let mut child = terminal();
    child["parent_tool_use_id"] = json!("agent");
    child["session_id"] = json!(OTHER);
    feed(
        &mut d,
        &[
            init(&["Agent"]),
            assistant("m", json!([tool("agent", "Agent")])),
            tool_result("agent", "done"),
            terminal(),
            child.clone(),
        ],
        &mut vec![],
    )
    .unwrap();
    assert_eq!(
        d.finish(|_| Ok::<_, ()>(())).unwrap().outcome,
        Some(Outcome::Success)
    );
    let mut d = decoder().with_cli_tools();
    assert_eq!(
        feed(&mut d, &[init(&["Agent"]), child], &mut vec![]),
        Err(Error::Identity)
    );
}

fn thinking_sequence() -> Vec<Value> {
    let mut first = assistant(
        "m",
        json!([{"type":"thinking","thinking":"Thought π","signature":SECRET}]),
    );
    first["uuid"] = json!("thinking-envelope");
    let mut second = assistant("m", json!([text("answer")]));
    second["uuid"] = json!("text-envelope");
    vec![
        init(&[]),
        start("m"),
        block(0, json!({"type":"thinking","thinking":""})),
        stream(
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Thought π","estimated_tokens":128}}),
        ),
        first,
        stop_block(0),
        block(1, text("")),
        delta(1, "answer"),
        second,
        stop_block(1),
        stop(),
        terminal(),
    ]
}

#[test]
fn documented_per_block_assistant_messages_do_not_finalize_the_whole_message() {
    let records = thinking_sequence();
    let (summary, events) = run(&records, &[]).unwrap();
    assert_eq!(summary.outcome, Some(Outcome::Success));
    assert!(!summary.presentation_incomplete);
    assert_eq!(display(&events)[&("m".into(), 0)], "Thought π");
    assert_eq!(display(&events)[&("m".into(), 1)], "answer");
    let mut decoder = decoder();
    let mut chunks = vec![];
    for byte in ndjson(&records) {
        decoder
            .feed(&[byte], |event| {
                chunks.push(event);
                Ok::<_, ()>(())
            })
            .unwrap();
    }
    assert_eq!(decoder.finish(|_| Ok::<_, ()>(())).unwrap(), summary);
    assert_eq!(display(&chunks), display(&events));
    assert!(!format!("{events:?}").contains(SECRET));
}

#[test]
fn repeated_envelope_ids_deduplicate_but_identical_distinct_blocks_remain_distinct() {
    let mut a = assistant("m", json!([text("same")]));
    a["uuid"] = json!("a");
    let mut b = a.clone();
    b["uuid"] = json!("b");
    let (_, events) = run(&[init(&[]), a.clone(), a, b, terminal()], &[]).unwrap();
    assert_eq!(
        display(&events).values().cloned().collect::<Vec<_>>(),
        vec!["same", "same"]
    );
}

#[test]
fn optional_telemetry_is_ignored_without_schema_or_usage_inference() {
    let (baseline, events) = run(&thinking_sequence(), &[]).unwrap();
    for metadata in [
        Value::Null,
        json!(0),
        json!(u64::MAX),
        json!(-1),
        json!(1.5),
        json!(SECRET),
        json!({"future":SECRET}),
    ] {
        let mut records = thinking_sequence();
        records[3]["event"]["delta"]["estimated_tokens"] = metadata.clone();
        records[3]["event"]["delta"]["future_metadata"] = metadata.clone();
        records[3]["event"]["future_metadata"] = metadata;
        let (summary, current) = run(&records, &[]).unwrap();
        assert_eq!(summary, baseline);
        assert_eq!(display(&current), display(&events));
    }
}

#[test]
fn message_metadata_and_unknown_progress_do_not_mutate_content() {
    let mut records = thinking_sequence();
    records.insert(10,stream(json!({"type":"message_delta","delta":{"container":{"id":SECRET},"stop_details":{},"future":SECRET},"context_management":{"new_type":SECRET},"input_transformations":[SECRET]})));
    records.insert(
        10,
        stream(json!({"type":"future_progress","metadata":SECRET})),
    );
    let (summary, events) = run(&records, &[]).unwrap();
    assert!(!summary.presentation_incomplete);
    assert_eq!(display(&events)[&("m".into(), 1)], "answer");
    assert!(!format!("{events:?}").contains(SECRET));
}

#[test]
fn malformed_display_is_reconciled_from_final_result_not_execution_failure() {
    let mut records = thinking_sequence();
    records[7]["event"]["delta"]["text"] = json!({"private":SECRET});
    records.last_mut().unwrap()["result"] = json!("answer");
    let (summary, events) = run(&records, &[]).unwrap();
    assert_eq!(summary.outcome, Some(Outcome::Success));
    assert!(summary.presentation_incomplete);
    assert_eq!(display(&events)[&("m".into(), 1)], "answer");
    assert!(!format!("{events:?}").contains(SECRET));
}

#[test]
fn display_warning_without_terminal_never_establishes_success() {
    let mut records = thinking_sequence();
    records[7]["event"]["delta"]["text"] = Value::Null;
    records.pop();
    assert_eq!(run(&records, &[]).unwrap_err(), Error::Incomplete);
}

#[test]
fn successful_terminal_survives_trailing_telemetry_but_not_conflicting_execution() {
    let mut records = thinking_sequence();
    records.push(envelope(
        "system",
        json!({"subtype":"prompt_suggestion","suggestion":SECRET}),
    ));
    assert_eq!(
        run(&records, &[]).unwrap().0.outcome,
        Some(Outcome::Success)
    );
    records.push(terminal());
    assert_eq!(run(&records, &[]).unwrap_err(), Error::Sequence);
    records.pop();
    records.last_mut().unwrap()["session_id"] = json!(OTHER);
    assert_eq!(run(&records, &[]).unwrap_err(), Error::Identity);
}

#[test]
fn result_error_flag_omission_is_protocol_default_not_action_authority() {
    for (flag, expected) in [
        (None, Some(false)),
        (Some(json!(false)), Some(false)),
        (Some(json!(true)), Some(true)),
        (Some(Value::Null), None),
    ] {
        let mut result = tool_result("t", "inert output");
        if let Some(flag) = flag {
            result["message"]["content"][0]["is_error"] = flag;
        } else {
            result["message"]["content"][0]
                .as_object_mut()
                .unwrap()
                .remove("is_error");
        }
        let (_, events) = run(
            &[
                init(&["Write"]),
                assistant("m", json!([tool("t", "Write")])),
                result,
                terminal(),
            ],
            &["Write"],
        )
        .unwrap();
        assert!(
            events
                .iter()
                .any(|e| matches!(e,Event::ExternalToolResult{is_error,..} if *is_error==expected))
        );
    }
}

#[test]
fn final_text_reconciliation_never_duplicates_complete_blocks_or_overwrites_prior_messages() {
    let bad = stream(
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":42}}),
    );
    let mut result = terminal();
    result["result"] = json!("firstsecond");
    let (_, events) = run(
        &[
            init(&[]),
            start("m"),
            bad,
            assistant("m", json!([text("first")])),
            assistant("m", json!([text("second")])),
            stop(),
            result,
        ],
        &[],
    )
    .unwrap();
    assert_eq!(
        display(&events).values().cloned().collect::<Vec<_>>(),
        vec!["first", "second"]
    );

    let mut result = terminal();
    result["result"] = json!("new final answer");
    let (_, events) = run(
        &[
            init(&[]),
            assistant("prior", json!([text("prior complete answer")])),
            assistant("new", json!([{"type":"text","text":42}])),
            result,
        ],
        &[],
    )
    .unwrap();
    assert_eq!(
        display(&events)[&("prior".into(), 0)],
        "prior complete answer"
    );
    assert!(
        display(&events)
            .values()
            .any(|text| text == "new final answer")
    );
}

#[test]
fn final_text_replaces_the_whole_unreliable_message_preview_without_repeated_prefixes() {
    let mut result = terminal();
    result["result"] = json!("firstsecond");
    let (_, events) = run(&[init(&[]), start("m"), block(0,text("first")), stop_block(0), block(1,text("sec")),
        stream(json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":42}})), stop(), result], &[]).unwrap();
    assert_eq!(
        display(&events).values().cloned().collect::<String>(),
        "firstsecond"
    );
}

#[test]
fn optional_final_text_shape_is_not_terminal_authority() {
    let mut result = terminal();
    result["result"] = json!({"private":SECRET});
    let (summary, events) = run(&[init(&[]), result.clone()], &[]).unwrap();
    assert_eq!(summary.outcome, Some(Outcome::Success));
    assert!(summary.presentation_incomplete);
    assert!(!format!("{events:?}").contains(SECRET));
    let (summary, events) = run(
        &[init(&[]), assistant("m", json!([text("answer")])), result],
        &[],
    )
    .unwrap();
    assert!(!summary.presentation_incomplete);
    assert_eq!(display(&events)[&("m".into(), 0)], "answer");
}

#[test]
fn malformed_tool_observations_cannot_hide_behind_display_recovery() {
    for block in [
        json!({"type":"tool_use","id":"t","name":42,"input":{}}),
        json!({"type":"server_tool_use","id":"t","name":"hidden","input":{}}),
    ] {
        assert!(
            run(
                &[init(&["Write"]), assistant("m", json!([block])), terminal()],
                &["Write"]
            )
            .is_err()
        );
    }
}

#[test]
fn recoverable_diagnostics_retain_only_bounded_sequence_facts() {
    let mut records = thinking_sequence();
    records[3]["event"]["delta"]["thinking"] = json!({"private":SECRET});
    let mut decoder = decoder();
    feed(&mut decoder, &records, &mut vec![]).unwrap();
    let diagnostic = decoder.diagnostic().unwrap();
    assert_eq!(diagnostic.record, RecordClass::ContentBlockDelta);
    assert_eq!(diagnostic.block_index, Some(0));
    assert_eq!(diagnostic.known_blocks, 1);
    assert!(diagnostic.active_message);
    assert_eq!(diagnostic.block_finalized, Some(false));
    assert!(!serde_json::to_string(&diagnostic).unwrap().contains(SECRET));
}

#[test]
fn tool_identity_and_unresolved_actions_remain_hard_failures() {
    assert_eq!(
        run(
            &[
                init(&["Write"]),
                assistant("m", json!([tool("t", "Write")])),
                terminal()
            ],
            &["Write"]
        )
        .unwrap_err(),
        Error::OrphanToolResult
    );
    let mut records = thinking_sequence();
    records[7]["session_id"] = json!(OTHER);
    assert_eq!(run(&records, &[]).unwrap_err(), Error::Identity);
}
