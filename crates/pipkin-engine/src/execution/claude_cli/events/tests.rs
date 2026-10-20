//! Synthetic 2.1.296-shaped fixtures only. No CLI, filesystem or network activity.
use super::*;
use serde_json::{Value, json};

const SESSION: &str = "12345678-1234-4234-a234-123456789abc";
const OTHER: &str = "87654321-4321-4321-b432-cba987654321";
const SECRET: &str = "PRIVATE_account_token_stderr_tool_input";

#[path = "coding_results_tests.rs"]
mod coding_results_tests;
#[path = "diagnostics_tests.rs"]
mod diagnostics;

fn decoder() -> Decoder {
    Decoder::new(SESSION, &[]).unwrap()
}
fn envelope(kind: &str, mut body: Value) -> Value {
    body["type"] = json!(kind);
    body["session_id"] = json!(SESSION);
    body
}
fn init(tools: &[&str]) -> Value {
    envelope("system", json!({"subtype":"init","tools":tools}))
}
fn terminal() -> Value {
    envelope(
        "result",
        json!({"subtype":"success","is_error":false,"result":""}),
    )
}
fn stream(body: Value) -> Value {
    envelope(
        "stream_event",
        json!({"event":body,"parent_tool_use_id":null}),
    )
}
fn start(id: &str) -> Value {
    stream(json!({"type":"message_start","message":{"id":id,"role":"assistant","content":[]}}))
}
fn block(index: usize, content: Value) -> Value {
    stream(json!({"type":"content_block_start","index":index,"content_block":content}))
}
fn delta(index: usize, text: &str) -> Value {
    stream(
        json!({"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":text}}),
    )
}
fn stop_block(index: usize) -> Value {
    stream(json!({"type":"content_block_stop","index":index}))
}
fn stop() -> Value {
    stream(json!({"type":"message_stop"}))
}
fn assistant(id: &str, content: Value) -> Value {
    envelope(
        "assistant",
        json!({"message":{"id":id,"role":"assistant","content":content}}),
    )
}
fn text(text: &str) -> Value {
    json!({"type":"text","text":text})
}
fn tool(id: &str, name: &str) -> Value {
    json!({"type":"tool_use","id":id,"name":name,"input":{"private":SECRET}})
}
fn tool_result(id: &str, text: &str) -> Value {
    envelope(
        "user",
        json!({"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":id,"content":text,"is_error":false}]},"tool_use_result":{"raw":SECRET}}),
    )
}
fn ndjson(records: &[Value]) -> Vec<u8> {
    records
        .iter()
        .map(|value| format!("{value}\n"))
        .collect::<String>()
        .into_bytes()
}
fn feed(decoder: &mut Decoder, records: &[Value], events: &mut Vec<Event>) -> Result<(), Error> {
    decoder.feed(&ndjson(records), |event| {
        events.push(event);
        Ok::<_, ()>(())
    })
}
fn run(records: &[Value], allowlist: &[&str]) -> Result<(Summary, Vec<Event>), Error> {
    let mut decoder = Decoder::new(SESSION, allowlist)?;
    let mut events = Vec::new();
    feed(&mut decoder, records, &mut events)?;
    let summary = decoder.finish(|event| {
        events.push(event);
        Ok::<_, ()>(())
    })?;
    Ok((summary, events))
}
fn display(events: &[Event]) -> BTreeMap<(String, usize), String> {
    let mut output = BTreeMap::<(String, usize), String>::new();
    for event in events {
        match event {
            Event::BlockDelta {
                message_id,
                block_index,
                offset,
                text,
                ..
            } => {
                let current = output
                    .entry((message_id.clone(), *block_index))
                    .or_default();
                assert_eq!(current.len(), *offset);
                current.push_str(text);
            }
            Event::BlockSnapshot {
                message_id,
                block_index,
                text,
                ..
            } => {
                output.insert((message_id.clone(), *block_index), text.clone());
            }
            _ => {}
        }
    }
    output
}
fn partial_records() -> Vec<Value> {
    vec![
        init(&[]),
        start("msg_1"),
        block(0, text("")),
        delta(0, "hëllo 🦀"),
        stop_block(0),
        stop(),
        assistant("msg_1", json!([text("hëllo 🦀")])),
        terminal(),
    ]
}

#[test]
fn every_utf8_chunk_boundary_and_bytewise_feed() {
    let records = partial_records();
    let input = ndjson(&records);
    let expected = run(&records, &[]).unwrap();
    for split in 0..=input.len() {
        let mut decoder = decoder();
        let mut events = Vec::new();
        for chunk in [&input[..split], &[], &input[split..]] {
            decoder
                .feed(chunk, |event| {
                    events.push(event);
                    Ok::<_, ()>(())
                })
                .unwrap();
        }
        let summary = decoder.finish(|_| Ok::<_, ()>(())).unwrap();
        assert_eq!((summary, events), expected);
    }
    let mut decoder = decoder();
    let mut events = Vec::new();
    for byte in &input {
        decoder
            .feed(&[*byte], |event| {
                events.push(event);
                Ok::<_, ()>(())
            })
            .unwrap();
    }
    assert_eq!(
        (decoder.finish(|_| Ok::<_, ()>(())).unwrap(), events),
        expected
    );
    assert_eq!(display(&expected.1)[&("msg_1".to_owned(), 0)], "hëllo 🦀");
    assert!(
        !expected
            .1
            .iter()
            .any(|event| matches!(event, Event::BlockSnapshot { .. }))
    );
}

#[test]
fn authoritative_snapshots_replace_partials_and_deduplicate_finals() {
    let final_message = assistant("msg_1", json!([text("authoritative")]));
    let records = vec![
        init(&[]),
        start("msg_1"),
        block(0, text("partial")),
        delta(0, " answer"),
        stop_block(0),
        stop(),
        final_message.clone(),
        final_message,
        terminal(),
    ];
    let (summary, events) = run(&records, &[]).unwrap();
    assert_eq!(display(&events)[&("msg_1".into(), 0)], "authoritative");
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Event::BlockSnapshot { .. }))
            .count(),
        1
    );
    assert_eq!(
        summary.projected_bytes,
        "partial answer".len() + "authoritative".len()
    );
    assert!(!format!("{events:?}").contains("authoritative"));
}

#[test]
fn thinking_and_text_are_separate_indexed_blocks_and_messages() {
    let records = vec![
        init(&[]),
        start("msg_1"),
        block(0, json!({"type":"thinking","thinking":""})),
        stream(
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"考え"}}),
        ),
        stream(
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":SECRET}}),
        ),
        stop_block(0),
        block(1, text("answer")),
        stop_block(1),
        stream(
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":1}}),
        ),
        stop(),
        assistant(
            "msg_1",
            json!([{"type":"thinking","thinking":"考え","signature":SECRET},text("answer")]),
        ),
        assistant("msg_2", json!([text("second")])),
        terminal(),
    ];
    let (_, events) = run(&records, &[]).unwrap();
    assert_eq!(display(&events).len(), 3);
    assert!(events.iter().any(|event| matches!(
        event,
        Event::BlockDelta {
            kind: TextKind::Thinking,
            ..
        }
    )));
    assert!(!format!("{events:?}").contains(SECRET));
}

#[test]
fn snapshot_before_stream_stop_and_missing_preview_stop_keep_execution_separate() {
    let records = vec![
        init(&[]),
        start("m"),
        block(0, text("a")),
        assistant("m", json!([text("a")])),
        stop_block(0),
        stop(),
        terminal(),
    ];
    assert!(run(&records, &[]).is_ok());
    let mut invalid = records;
    invalid.remove(4);
    let (summary, _) = run(&invalid, &[]).unwrap();
    assert!(summary.presentation_incomplete);
    assert_eq!(summary.outcome, Some(Outcome::Success));
    invalid.pop();
    assert_eq!(run(&invalid, &[]).unwrap_err(), Error::Incomplete);
}

#[test]
fn conflicting_finals_late_deltas_and_mismatched_ids_fail() {
    for records in [
        vec![
            init(&[]),
            assistant("m", json!([text("a")])),
            assistant("m", json!([text("b")])),
        ],
        vec![
            init(&[]),
            start("m"),
            block(0, text("a")),
            assistant("m", json!([text("a")])),
            delta(0, "b"),
        ],
        vec![
            init(&[]),
            start("m"),
            assistant("other", json!([text("a")])),
        ],
        vec![
            init(&[]),
            start("m"),
            block(0, text("a")),
            assistant("m", json!([])),
        ],
        vec![
            init(&[]),
            start("m"),
            block(0, text("a")),
            block(0, text("a")),
        ],
        vec![init(&[]), start("m"), block(0, text("a")), delta(1, "b")],
        vec![init(&[]), start("m"), start("m")],
        vec![
            init(&[]),
            start("m"),
            block(0, text("a")),
            stop_block(0),
            stop_block(0),
        ],
    ] {
        assert!(run(&records, &[]).is_err());
    }
}

#[test]
fn constructor_and_all_known_envelopes_require_exact_canonical_identity() {
    for bad in [
        "",
        "not-a-uuid",
        &SESSION.to_uppercase(),
        "12345678_1234-4234-a234-123456789abc",
    ] {
        assert!(matches!(
            Decoder::new(bad, &[]),
            Err(Error::InvalidConfiguration)
        ));
    }
    for record in [
        init(&[]),
        assistant("m", json!([])),
        start("m"),
        tool_result("t", "x"),
        terminal(),
        envelope("system", json!({"subtype":"status"})),
    ] {
        for replacement in [
            None,
            Some(json!(OTHER)),
            Some(Value::Null),
            Some(json!(SESSION.to_uppercase())),
        ] {
            let mut record = record.clone();
            if let Some(value) = replacement {
                record["session_id"] = value;
            } else {
                record.as_object_mut().unwrap().remove("session_id");
            }
            let mut decoder = decoder();
            assert!(feed(&mut decoder, &[record], &mut Vec::new()).is_err());
            assert!(decoder.feed(&[], |_| Ok::<_, ()>(())).is_err());
        }
    }
    let mut unknown = json!({"type":"future", "session_id":OTHER});
    assert_eq!(
        feed(&mut decoder(), &[unknown.clone()], &mut Vec::new()),
        Err(Error::Identity)
    );
    unknown["session_id"] = json!(SESSION);
    assert!(feed(&mut decoder(), &[unknown], &mut Vec::new()).is_ok());
}

#[test]
fn initialization_inventory_is_explicit_and_no_tools_is_the_default_policy() {
    for records in [
        vec![assistant("m", json!([text("x")]))],
        vec![start("m")],
        vec![terminal()],
        vec![init(&["Bash"])],
        vec![envelope("system", json!({"subtype":"init"}))],
        vec![envelope("system", json!({"subtype":"init","tools":null}))],
        vec![init(&[]), assistant("m", json!([tool("t", "Write")]))],
    ] {
        assert!(run(&records, &[]).is_err());
    }
    assert!(run(&[init(&["Write", "Write"]), terminal()], &["Write"]).is_err());
    assert!(run(&[init(&["Write", "Bash"]), terminal()], &["Write"]).is_err());
    assert!(
        run(
            &[init(&[]), assistant("m", json!([tool("t", "Write")]))],
            &["Write"]
        )
        .is_err()
    );
    assert!(run(&[init(&[]), terminal()], &["Write"]).is_ok());
    assert!(Decoder::new(SESSION, &["Write", "Write"]).is_err());
    assert!(Decoder::new(SESSION, &[""]).is_err());
}

#[test]
fn repeated_init_preserves_pending_tools_text_and_one_initialization() {
    for mode in 0..3 {
        let mut d = Decoder::new(SESSION, &["Agent"]).unwrap();
        if mode == 1 {
            d = d.with_cli_tools();
        } else if mode == 2 {
            d = d.with_exact_inventory();
        }
        let mut events = Vec::new();
        let records = [
            init(&["Agent"]),
            assistant("launch", json!([tool("agent-1", "Agent")])),
            start("waiting"),
            block(0, text("Waiting")),
            // A repeated init is telemetry, not a new root run or message boundary.
            init(&["Agent"]),
            delta(0, " for agents"),
            stop_block(0),
            stop(),
            tool_result("agent-1", "background exploration complete"),
            envelope(
                "system",
                json!({"subtype":"task_notification","task_id":"task-1","status":"completed"}),
            ),
            init(&["Agent"]),
            assistant("answer", json!([text("Finished contract")])),
            terminal(),
        ];
        feed(&mut d, &records, &mut events).unwrap();
        let summary = d.finish(|_| Ok::<_, ()>(())).unwrap();
        assert_eq!(summary.outcome, Some(Outcome::Success));
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, Event::Initialized))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, Event::ExternalToolObserved { .. }))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, Event::ExternalToolResult { .. }))
                .count(),
            1
        );
        assert_eq!(
            display(&events)[&("waiting".into(), 0)],
            "Waiting for agents"
        );
        assert_eq!(display(&events)[&("answer".into(), 0)], "Finished contract");
    }
}

#[test]
fn repeated_init_revalidates_identity_shape_inventory_and_terminal_boundary() {
    let mut wrong_session = init(&["Agent"]);
    wrong_session["session_id"] = json!(OTHER);
    let mut mutation = init(&["Agent"]);
    mutation["message"] = json!({"content": [text(SECRET)]});
    for (record, expected) in [
        (wrong_session, Error::Identity),
        (init(&["Agent", "Agent"]), Error::UnsupportedTool),
        (init(&[""]), Error::UnsupportedTool),
        (
            envelope("system", json!({"subtype":"init","tools":null})),
            Error::MalformedRecord,
        ),
        (mutation, Error::UnsupportedContent),
        (init(&vec!["Agent"; MAX_ENTRIES + 1]), Error::Limit),
    ] {
        let mut d = Decoder::new(SESSION, &[]).unwrap().with_cli_tools();
        assert_eq!(
            feed(&mut d, &[init(&["Agent"]), record], &mut Vec::new()),
            Err(expected)
        );
    }
    for exact in [false, true] {
        let mut d = Decoder::new(SESSION, &["Agent", "Write"]).unwrap();
        if exact {
            d = d.with_exact_inventory();
        }
        assert_eq!(
            feed(
                &mut d,
                &[init(&["Agent", "Write"]), init(&["Agent"])],
                &mut Vec::new()
            ),
            Err(Error::UnsupportedTool)
        );
        let mut d = Decoder::new(SESSION, &["Agent"]).unwrap();
        if exact {
            d = d.with_exact_inventory();
        }
        assert_eq!(
            feed(
                &mut d,
                &[init(&["Agent"]), init(&["Agent", "Bash"])],
                &mut Vec::new()
            ),
            Err(Error::UnsupportedTool)
        );
    }
    let mut d = Decoder::new(SESSION, &[]).unwrap().with_cli_tools();
    feed(&mut d, &[init(&[]), terminal(), init(&[])], &mut Vec::new()).unwrap();
    assert_eq!(d.finish(|_| Ok::<_, ()>(())), Err(Error::Incomplete));
    let mut d = Decoder::new(SESSION, &[]).unwrap().with_cli_tools();
    assert_eq!(
        feed(
            &mut d,
            &[
                init(&["Agent"]),
                assistant("launch", json!([tool("pending", "Agent")])),
                init(&["Agent"]),
                terminal()
            ],
            &mut Vec::new()
        ),
        Err(Error::OrphanToolResult)
    );
}

#[test]
fn coding_init_inventory_refresh_does_not_forget_previously_observed_tools() {
    let mut d = Decoder::new(SESSION, &[]).unwrap().with_cli_tools();
    let mut events = Vec::new();
    feed(
        &mut d,
        &[
            init(&["Agent"]),
            assistant("launch", json!([tool("agent-1", "Agent")])),
            init(&["Bash"]),
            assistant("work", json!([tool("bash-1", "Bash")])),
            tool_result("bash-1", "command complete"),
            tool_result("agent-1", "background complete"),
            terminal(),
        ],
        &mut events,
    )
    .unwrap();
    assert_eq!(
        d.finish(|_| Ok::<_, ()>(())).unwrap().outcome,
        Some(Outcome::Success)
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::Initialized))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::ExternalToolResult { .. }))
            .count(),
        2
    );
}

#[test]
fn external_tools_observe_once_discard_arguments_and_correlate_results() {
    let result = tool_result("t", "display output");
    let records = vec![
        init(&["Write"]),
        start("m"),
        block(0, tool("t", "Write")),
        stream(
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":SECRET}}),
        ),
        stop_block(0),
        stop(),
        assistant("m", json!([tool("t", "Write")])),
        result.clone(),
        result,
        terminal(),
    ];
    let (summary, events) = run(&records, &["Write"]).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Event::ExternalToolObserved { .. }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Event::ExternalToolResult { .. }))
            .count(),
        1
    );
    assert_eq!(summary.projected_bytes, "display output".len());
    assert!(display(&events).is_empty());
    assert!(!format!("{events:?}").contains(SECRET));
}

#[test]
fn orphan_result_is_rejected_before_display_and_never_replayed() {
    let mut decoder = Decoder::new(SESSION, &["Write"]).unwrap();
    let mut events = Vec::new();
    assert_eq!(
        feed(
            &mut decoder,
            &[init(&["Write"]), tool_result("future", SECRET)],
            &mut events
        ),
        Err(Error::OrphanToolResult)
    );
    assert_eq!(events, vec![Event::Initialized]);
    assert_eq!(
        feed(
            &mut decoder,
            &[assistant("m", json!([tool("future", "Write")]))],
            &mut events
        ),
        Err(Error::OrphanToolResult)
    );
    assert!(decoder.tools.is_empty());
}

#[test]
fn reused_tool_ids_conflicting_results_and_unknown_tools_fail() {
    for trailing in [
        assistant("other", json!([tool("t", "Write")])),
        assistant("other", json!([tool("t2", "Bash")])),
        tool_result("unknown", "x"),
        tool_result("t", "different"),
    ] {
        assert!(
            run(
                &[
                    init(&["Write"]),
                    assistant("m", json!([tool("t", "Write")])),
                    tool_result("t", "first"),
                    trailing,
                    terminal()
                ],
                &["Write"]
            )
            .is_err()
        );
    }
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
    let mut error = terminal();
    error["subtype"] = json!("error_during_execution");
    error["is_error"] = json!(true);
    assert_eq!(
        run(
            &[
                init(&["Write"]),
                assistant("m", json!([tool("t", "Write")])),
                error
            ],
            &["Write"]
        )
        .unwrap()
        .0
        .outcome,
        Some(Outcome::Error)
    );
}

#[test]
fn result_block_arrays_and_explicit_null_error_flag_remain_uncertain() {
    let result = envelope(
        "user",
        json!({"message":{"content":[{"type":"tool_result","tool_use_id":"t","is_error":null,"content":[text("a"),text("b")]}]}}),
    );
    let (_, events) = run(
        &[
            init(&["Read"]),
            assistant("m", json!([tool("t", "Read")])),
            result,
            terminal(),
        ],
        &["Read"],
    )
    .unwrap();
    assert!(events.iter().any(|event| matches!(event, Event::ExternalToolResult { is_error: None, text, .. } if text == "ab")));
}

#[test]
fn unknown_envelopes_are_observations_never_completion_authority() {
    let records = vec![
        json!({"type":"future","result":"success","is_error":false,"account":SECRET}),
        init(&[]),
        envelope("system", json!({"subtype":"future_status"})),
        terminal(),
    ];
    let (summary, events) = run(&records, &[]).unwrap();
    assert_eq!(summary.unknown_records, 2);
    assert_eq!(events.len(), 2);
    for field in ["message", "event", "content", "delta", "tool_use_result"] {
        let mut unknown = json!({"type":"future"});
        unknown[field] = Value::Null;
        let mut decoder = decoder();
        feed(&mut decoder, &[unknown], &mut Vec::new()).unwrap();
        assert!(decoder.summary().presentation_incomplete);
        assert_eq!(decoder.finish(|_| Ok::<_, ()>(())), Err(Error::Incomplete));
    }
    let mut decoder = decoder();
    feed(
        &mut decoder,
        &[json!({"type":"future","result":"success"})],
        &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(decoder.finish(|_| Ok::<_, ()>(())), Err(Error::Incomplete));
}

#[test]
fn unsupported_display_is_a_warning_but_nested_and_tool_streams_fail_closed() {
    for bad in [
        json!({"type":"image","source":SECRET}),
        json!({"type":"future","text":"x"}),
        json!({"type":"redacted_thinking","data":SECRET}),
    ] {
        let (summary, events) =
            run(&[init(&[]), assistant("m", json!([bad])), terminal()], &[]).unwrap();
        assert!(summary.presentation_incomplete);
        assert_eq!(summary.outcome, Some(Outcome::Success));
        assert!(!format!("{events:?}").contains(SECRET));
    }
    for kind in [
        "citations_delta",
        "future_delta",
        "input_json_delta",
        "signature_delta",
        "thinking_delta",
    ] {
        let bad = stream(
            json!({"type":"content_block_delta","index":0,"delta":{"type":kind,"text":"x","thinking":"x","partial_json":"{}","signature":"s"}}),
        );
        let decoded = run(
            &[init(&[]), start("m"), block(0, text("")), bad, terminal()],
            &[],
        );
        if kind == "input_json_delta" {
            assert!(decoded.is_err());
        } else {
            assert!(decoded.unwrap().0.presentation_incomplete);
        }
    }
    let mut nested = start("m");
    nested["parent_tool_use_id"] = json!("parent");
    assert_eq!(run(&[init(&[]), nested], &[]).unwrap_err(), Error::Identity);
    let bad = stream(json!({"type":"message_delta","delta":{"text":"hidden mutation"}}));
    let (_, events) = run(&[init(&[]), start("m"), bad, stop(), terminal()], &[]).unwrap();
    assert!(display(&events).is_empty());
}

#[test]
fn malformed_utf8_duplicate_fields_truncation_and_arrays_are_redacted() {
    for raw in [
        b"{\n".as_slice(),
        b"[]\n",
        b"null\n",
        b"\n",
        b"{}\n",
        b"{\"type\":7}\n",
        b"{\"type\":\"future\",\"type\":\"result\"}\n",
        b"{\"type\":\"future\",\"ignored\":\"\xff\"}\n",
        b"{\"type\":\"future\"} trailing\n",
        b"{\"type\":\"future\"}{}\n",
    ] {
        let mut decoder = decoder();
        assert_eq!(
            decoder.feed(raw, |_| Ok::<_, ()>(())),
            Err(Error::MalformedRecord)
        );
        assert_eq!(
            decoder.feed(&[], |_| Ok::<_, ()>(())),
            Err(Error::MalformedRecord)
        );
        assert_eq!(
            decoder.finish(|_| Ok::<_, ()>(())),
            Err(Error::MalformedRecord)
        );
    }
    let mut decoder = decoder();
    decoder
        .feed(b"{\"type\":\"future\",\"ignored\":\"unfinished", |_| {
            Ok::<_, ()>(())
        })
        .unwrap();
    assert_eq!(
        decoder.finish(|_| Ok::<_, ()>(())),
        Err(Error::MalformedRecord)
    );
    let bad = format!(r#"{{"type":"result","is_error":"{SECRET}"}}"#);
    let mut decoder = super::Decoder::new(SESSION, &[]).unwrap();
    decoder.feed(bad.as_bytes(), |_| Ok::<_, ()>(())).unwrap();
    let error = decoder.finish(|_| Ok::<_, ()>(())).unwrap_err();
    assert!(!format!("{error}: {error:?}").contains(SECRET));
}

#[test]
fn positional_presentation_and_unknown_metadata_cannot_establish_completion() {
    for record in [
        envelope("assistant", json!({"message":["m", "assistant", []]})),
        envelope("user", json!({"message":[null, "user", []]})),
        stream(json!({"type":"message_start","message":["m", "assistant", []]})),
        stream(json!({"type":"ping","delta":{"type":"text_delta","text":"hidden"}})),
    ] {
        assert!(run(&[init(&[]), record], &[]).is_err());
    }
    let delta = stream(json!({"type":"content_block_delta","index":0,
        "delta":{"type":"text_delta","text":"x","future_mutation":"hidden"}}));
    assert!(run(&[init(&[]), start("m"), block(0, text("")), delta], &[]).is_err());
}

#[test]
fn pressure_failure_mid_snapshot_does_not_deliver_remaining_blocks() {
    let mut decoder = decoder();
    feed(&mut decoder, &[init(&[])], &mut Vec::new()).unwrap();
    let mut calls = 0;
    assert_eq!(
        decoder.feed(
            &ndjson(&[assistant("m", json!([text("a"), text("b"), text("c")]))]),
            |_| {
                calls += 1;
                if calls == 2 { Err(()) } else { Ok(()) }
            }
        ),
        Err(Error::SinkRejected)
    );
    assert_eq!(calls, 2);
    assert!(decoder.messages.is_empty());
}

#[test]
fn crlf_and_complete_final_record_without_lf() {
    let input = format!("{}\r\n{}", init(&[]), terminal());
    let mut decoder = decoder();
    let mut events = Vec::new();
    decoder
        .feed(input.as_bytes(), |event| {
            events.push(event);
            Ok::<_, ()>(())
        })
        .unwrap();
    assert_eq!(events.len(), 1);
    let summary = decoder
        .finish(|event| {
            events.push(event);
            Ok::<_, ()>(())
        })
        .unwrap();
    assert_eq!(summary.outcome, Some(Outcome::Success));
    assert_eq!(events.len(), 2);
}

#[test]
fn terminal_summary_does_not_duplicate_existing_text_and_usage_is_optional() {
    let mut result = terminal();
    result["result"] = json!(SECRET);
    result["account"] = json!(SECRET);
    result["stderr"] = json!(SECRET);
    let (_, events) = run(
        &[
            init(&[]),
            assistant("m", json!([text("answer")])),
            result.clone(),
        ],
        &[],
    )
    .unwrap();
    assert_eq!(display(&events)[&("m".into(), 0)], "answer");
    assert!(matches!(
        events.last(),
        Some(Event::Terminal { usage: None, .. })
    ));
    assert!(!format!("{events:?}").contains(SECRET));
    result["usage"] = json!({"input_tokens":0,"output_tokens":12,"account":SECRET});
    let (_, events) = run(&[init(&[]), result], &[]).unwrap();
    assert!(matches!(
        events.last(),
        Some(Event::Terminal {
            usage: Some(Usage {
                input_tokens: Some(0),
                output_tokens: Some(12),
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
                reasoning_tokens: None,
                cache_write_1h: None
            }),
            ..
        })
    ));
    for invalid in [json!(-1), json!(1.5), json!("0"), json!({})] {
        let mut result = terminal();
        result["usage"] = json!({"input_tokens":invalid});
        let (_, events) = run(&[init(&[]), result], &[]).unwrap();
        assert!(matches!(
            events.last(),
            Some(Event::Terminal { usage: None, .. })
        ));
    }
}

#[test]
fn unknown_status_never_succeeds_and_error_status_is_terminal() {
    for (status, is_error, expected) in [
        ("success", false, Outcome::Success),
        ("success", true, Outcome::Error),
        ("error_during_execution", false, Outcome::Error),
        ("error_max_turns", true, Outcome::Error),
        ("error_max_budget_usd", true, Outcome::Error),
        ("error_max_structured_output_retries", true, Outcome::Error),
        ("future", false, Outcome::Unknown),
        ("future", true, Outcome::Unknown),
    ] {
        let result = envelope("result", json!({"subtype":status,"is_error":is_error}));
        assert_eq!(
            run(&[init(&[]), result], &[]).unwrap().0.outcome,
            Some(expected)
        );
    }
    for field in ["subtype", "is_error"] {
        let mut result = terminal();
        result.as_object_mut().unwrap().remove(field);
        assert!(run(&[init(&[]), result], &[]).is_err());
    }
}

#[test]
fn terminal_is_unique_and_eof_or_drop_is_not_success() {
    for records in [vec![], vec![init(&[])], vec![init(&[]), start("m")]] {
        assert_eq!(run(&records, &[]).unwrap_err(), Error::Incomplete);
    }
    for trailing in [terminal(), json!({"type":"future"}), init(&[])] {
        let mut decoder = decoder();
        assert_eq!(
            feed(
                &mut decoder,
                &[init(&[]), terminal(), trailing],
                &mut Vec::new()
            ),
            Err(Error::Sequence)
        );
        assert_eq!(decoder.summary().outcome, None);
        assert_eq!(decoder.finish(|_| Ok::<_, ()>(())), Err(Error::Sequence));
    }
    let mut decoder = decoder();
    let mut events = Vec::new();
    feed(
        &mut decoder,
        &[init(&[]), assistant("m", json!([text("partial")]))],
        &mut events,
    )
    .unwrap();
    drop(decoder);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Terminal { .. }))
    );
}

#[test]
fn sink_failure_stops_current_chunk_poisoning_decoder_without_retry_or_drop() {
    let records = vec![
        init(&[]),
        assistant("m1", json!([text("one")])),
        assistant("m2", json!([text("two")])),
        terminal(),
    ];
    for reject_at in 0..4 {
        let mut decoder = decoder();
        let mut calls = 0;
        let error = decoder.feed(&ndjson(&records), |_| {
            calls += 1;
            if calls == reject_at + 1 {
                Err(SECRET)
            } else {
                Ok(())
            }
        });
        assert_eq!(error, Err(Error::SinkRejected));
        assert_eq!(calls, reject_at + 1);
        assert_eq!(
            decoder.feed(&[], |_| -> Result<(), ()> { panic!("retried") }),
            Err(Error::SinkRejected)
        );
        assert!(decoder.messages.is_empty() && decoder.tools.is_empty() && decoder.line.is_empty());
        assert_eq!(decoder.summary().outcome, None);
        assert_eq!(
            decoder.finish(|_| -> Result<(), ()> { panic!("retried") }),
            Err(Error::SinkRejected)
        );
    }
    let mut decoder = decoder();
    feed(&mut decoder, &[init(&[])], &mut Vec::new()).unwrap();
    decoder
        .feed(terminal().to_string().as_bytes(), |_| Ok::<_, ()>(()))
        .unwrap();
    assert_eq!(decoder.finish(|_| Err(SECRET)), Err(Error::SinkRejected));
}

#[test]
fn line_bound_is_exact_and_applies_across_chunks() {
    let prefix = r#"{"type":"future","ignored":""#;
    let line = format!(
        "{prefix}{}\"}}",
        "x".repeat(MAX_LINE_BYTES - prefix.len() - 2)
    );
    assert_eq!(line.len(), MAX_LINE_BYTES);
    let mut decoder = decoder();
    decoder.feed(line.as_bytes(), |_| Ok::<_, ()>(())).unwrap();
    assert!(decoder.line.capacity() <= MAX_LINE_BYTES);
    decoder.feed(b"\n", |_| Ok::<_, ()>(())).unwrap();
    assert_eq!(decoder.summary().unknown_records, 1);
    let mut decoder = super::Decoder::new(SESSION, &[]).unwrap();
    decoder
        .feed(&vec![b' '; MAX_LINE_BYTES], |_| Ok::<_, ()>(()))
        .unwrap();
    assert_eq!(decoder.feed(b" ", |_| Ok::<_, ()>(())), Err(Error::Limit));
    assert!(decoder.line.is_empty());
}

#[test]
fn cumulative_display_bound_includes_replacements_and_tool_results() {
    let mut decoder = decoder();
    let mut bytes = 0;
    let mut sink = |event| {
        match event {
            Event::BlockDelta { text, .. }
            | Event::BlockSnapshot { text, .. }
            | Event::ExternalToolResult { text, .. } => bytes += text.len(),
            _ => {}
        }
        Ok::<_, ()>(())
    };
    decoder
        .feed(
            &ndjson(&[init(&[]), start("m"), block(0, text(""))]),
            &mut sink,
        )
        .unwrap();
    let part = "x".repeat(MAX_PROJECTED_BYTES / 8);
    for _ in 0..8 {
        decoder
            .feed(&ndjson(&[delta(0, &part)]), &mut sink)
            .unwrap();
    }
    assert_eq!(decoder.summary().projected_bytes, MAX_PROJECTED_BYTES);
    assert_eq!(
        decoder.feed(&ndjson(&[delta(0, "x")]), &mut sink),
        Err(Error::Limit)
    );
    assert_eq!(bytes, MAX_PROJECTED_BYTES);

    let mut decoder = super::Decoder::new(SESSION, &["Read"]).unwrap();
    feed(
        &mut decoder,
        &[init(&["Read"]), assistant("m", json!([tool("t", "Read")]))],
        &mut Vec::new(),
    )
    .unwrap();
    decoder.summary.projected_bytes = MAX_PROJECTED_BYTES; // exact remaining-budget edge
    assert_eq!(
        feed(&mut decoder, &[tool_result("t", "x")], &mut Vec::new()),
        Err(Error::Limit)
    );

    let mut decoder = super::Decoder::new(SESSION, &[]).unwrap();
    feed(
        &mut decoder,
        &[
            init(&[]),
            start("m"),
            block(0, text("a")),
            stop_block(0),
            stop(),
        ],
        &mut Vec::new(),
    )
    .unwrap();
    decoder.summary.projected_bytes = MAX_PROJECTED_BYTES;
    assert_eq!(
        feed(
            &mut decoder,
            &[assistant("m", json!([text("b")]))],
            &mut Vec::new()
        ),
        Err(Error::Limit)
    );
}

#[test]
fn ids_message_count_and_total_block_count_are_bounded() {
    for id in [
        "".to_owned(),
        "x".repeat(MAX_ID_BYTES + 1),
        "space id".to_owned(),
        "é".to_owned(),
    ] {
        assert_eq!(
            run(&[init(&[]), assistant(&id, json!([]))], &[]).unwrap_err(),
            Error::Identity
        );
        assert_eq!(
            run(
                &[init(&["Read"]), assistant("m", json!([tool(&id, "Read")]))],
                &["Read"]
            )
            .unwrap_err(),
            Error::Identity
        );
    }
    assert!(
        run(
            &[
                init(&[]),
                assistant(&"x".repeat(MAX_ID_BYTES), json!([])),
                terminal()
            ],
            &[]
        )
        .is_ok()
    );
    let mut decoder = decoder();
    feed(&mut decoder, &[init(&[])], &mut Vec::new()).unwrap();
    for n in 0..MAX_ENTRIES {
        feed(
            &mut decoder,
            &[assistant(&format!("m{n}"), json!([]))],
            &mut Vec::new(),
        )
        .unwrap();
    }
    assert_eq!(decoder.messages.len(), MAX_ENTRIES);
    assert_eq!(
        feed(
            &mut decoder,
            &[assistant("overflow", json!([]))],
            &mut Vec::new()
        ),
        Err(Error::Limit)
    );

    let mut decoder = super::Decoder::new(SESSION, &[]).unwrap();
    feed(&mut decoder, &[init(&[])], &mut Vec::new()).unwrap();
    for message in 0..MAX_ENTRIES / 256 {
        feed(
            &mut decoder,
            &[assistant(
                &format!("blocks{message}"),
                json!(vec![text(""); 256]),
            )],
            &mut Vec::new(),
        )
        .unwrap();
    }
    let mut events = Vec::new();
    assert_eq!(
        feed(
            &mut decoder,
            &[assistant("overflow", json!([text("")]))],
            &mut events
        ),
        Err(Error::Limit)
    );
    assert!(events.is_empty());
}

#[test]
fn large_chunks_have_no_internal_event_queue_and_counters_are_checked() {
    let unknown = b"{\"type\":\"future\"}\n";
    let mut decoder = decoder();
    decoder
        .feed(&unknown.repeat(MAX_ENTRIES * 2), |_| -> Result<(), ()> {
            panic!("unexpected event")
        })
        .unwrap();
    assert_eq!(decoder.summary().unknown_records, (MAX_ENTRIES * 2) as u64);
    assert!(decoder.messages.is_empty() && decoder.line.is_empty());
    decoder.summary.records = u64::MAX;
    assert_eq!(
        decoder.feed(unknown, |_| Ok::<_, ()>(())),
        Err(Error::Limit)
    );
}

#[test]
fn native_context_telemetry_is_bounded_scoped_and_never_terminal_usage() {
    let mut d = decoder().with_exact_inventory();
    let mut out = vec![];
    feed(&mut d,&[init(&[]),envelope("assistant",json!({"message":{"id":"a","role":"assistant","content":[],"usage":{"input_tokens":10,"cache_read_input_tokens":20,"cache_creation_input_tokens":30,"output_tokens":999}}})),envelope("system",json!({"subtype":"compact_boundary","compact_metadata":{"trigger":"auto","pre_tokens":1234}})),terminal()],&mut out).unwrap();
    assert!(out.contains(&Event::Context {
        tokens: Some(60),
        compact_pre_tokens: None
    }));
    assert!(out.contains(&Event::Context {
        tokens: None,
        compact_pre_tokens: Some(1234)
    }));
    let n = out.len();
    feed(&mut d,&[envelope("system",json!({"subtype":"compact_boundary","compact_metadata":{"trigger":"manual","pre_tokens":9999}}))],&mut out).unwrap();
    assert_eq!(out.len(), n, "trailing telemetry cannot prove compaction");
    for metadata in [
        json!({"trigger":"manual","pre_tokens":-1}),
        json!({"trigger":"manual","pre_tokens":u64::MAX}),
        json!({"trigger":"made-up","pre_tokens":20}),
        json!({"trigger":"manual"}),
    ] {
        let mut d = decoder().with_exact_inventory();
        let mut out = vec![];
        assert!(
            feed(
                &mut d,
                &[
                    init(&[]),
                    envelope(
                        "system",
                        json!({"subtype":"compact_boundary","compact_metadata":metadata})
                    )
                ],
                &mut out
            )
            .is_err()
        );
    }
    let boundary = envelope(
        "system",
        json!({"subtype":"compact_boundary","compact_metadata":{"trigger":"manual","pre_tokens":10}}),
    );
    assert_eq!(
        feed(
            &mut decoder().with_exact_inventory(),
            std::slice::from_ref(&boundary),
            &mut vec![]
        ),
        Err(Error::MissingInit)
    );
    let mut foreign = boundary;
    foreign["session_id"] = json!(OTHER);
    assert_eq!(
        feed(
            &mut decoder().with_exact_inventory(),
            &[init(&[]), foreign],
            &mut vec![]
        ),
        Err(Error::Identity)
    );
}

#[test]
fn features_invalid_usage_subsets_do_not_erase_valid_counts_or_inflate_totals() {
    for (reasoning, cache) in [
        (json!(21), json!(11)),
        (json!("private"), json!(-1)),
        (Value::Null, Value::Null),
    ] {
        let mut result = terminal();
        result["usage"] = json!({"input_tokens":-1,"output_tokens":20,"cache_creation_input_tokens":10,"output_tokens_details":{"thinking_tokens":reasoning},"cache_creation":{"ephemeral_1h_input_tokens":cache}});
        let (_, events) = run(&[init(&[]), result], &[]).unwrap();
        let Some(Event::Terminal { usage: Some(u), .. }) = events.last() else {
            panic!("usage preserved")
        };
        assert_eq!(u.input_tokens, None);
        assert_eq!(u.output_tokens, Some(20));
        assert_eq!(u.cache_creation_input_tokens, Some(10));
        assert_eq!(u.reasoning_tokens, None);
        assert_eq!(u.cache_write_1h, None);
    }
}
