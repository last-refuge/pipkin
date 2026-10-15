use super::*;

#[test]
fn rejected_record_diagnostics_are_closed_and_payload_free() {
    for (kind, expected) in [
        ("message_delta", RecordClass::MessageDelta),
        (SECRET, RecordClass::Unknown),
    ] {
        let mut decoder = decoder();
        let record = stream(json!({"type":kind,"unexpected":SECRET}));
        let error = feed(&mut decoder, &[init(&[]), start("m"), record], &mut vec![]).unwrap_err();
        assert_eq!(decoder.failed_record(), Some(expected));
        let diagnostic = serde_json::to_string(&(error, decoder.failed_record())).unwrap();
        assert!(!diagnostic.contains(SECRET));
        assert_eq!(feed(&mut decoder, &[terminal()], &mut vec![]), Err(error));
        assert_eq!(decoder.failed_record(), Some(expected));
    }
}

#[test]
fn parse_failure_hashes_only_schema_labels_and_never_echoes_values() {
    for (record, kind, label) in [
        (
            stream(
                json!({"type":"message_delta","delta":{"stop_reason":"end_turn",SECRET:SECRET}}),
            ),
            ParseFailureKind::UnknownField,
            Some(SECRET),
        ),
        (
            stream(json!({"type":"message_delta","usage":null})),
            ParseFailureKind::MissingField,
            Some("delta"),
        ),
        (
            stream(json!({"type":"message_delta","delta":SECRET})),
            ParseFailureKind::InvalidType,
            None,
        ),
    ] {
        let mut decoder = decoder();
        assert_eq!(
            feed(&mut decoder, &[init(&[]), start("m"), record], &mut vec![]),
            Err(Error::MalformedRecord)
        );
        let failure = decoder.parse_failure().unwrap();
        assert_eq!(failure.kind, kind);
        assert_eq!(
            failure.field_sha256,
            label.map(|s| Sha256::digest(s.as_bytes()).into())
        );
        assert!(!serde_json::to_string(&failure).unwrap().contains(SECRET));
    }
}

#[test]
fn result_error_flag_omission_is_protocol_default_not_delivery_or_action_proof() {
    // Official Handle tool calls documentation's successful example omits is_error.
    // Null/malformed status is deliberately NOT normalized to success.
    for (field, expected) in [
        (None, Some(false)),
        (Some(json!(false)), Some(false)),
        (Some(json!(true)), Some(true)),
        (Some(Value::Null), None),
    ] {
        let mut result = tool_result("t", "inert reported output");
        let body = result["message"]["content"][0].as_object_mut().unwrap();
        if let Some(field) = field {
            body.insert("is_error".into(), field);
        } else {
            body.remove("is_error");
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
        assert!(events.iter().any(
            |event| matches!(event,Event::ExternalToolResult {is_error,..} if *is_error==expected)
        ));
    }
    let mut malformed = tool_result("t", "inert");
    malformed["message"]["content"][0]["is_error"] = json!("false");
    assert!(
        run(
            &[
                init(&["Write"]),
                assistant("m", json!([tool("t", "Write")])),
                malformed,
                terminal()
            ],
            &["Write"]
        )
        .is_err()
    );
}

fn delta_with_metadata() -> Value {
    // Published Anthropic RawMessageDeltaEvent/BetaRawMessageDeltaEvent schema,
    // fetched 2026-10-15. This is a synthetic schema regression, not a captured CLI frame.
    stream(
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null,"container":null,"stop_details":null},"usage":{"output_tokens":3},"context_management":null,"input_transformations":[]}),
    )
}
fn sequence(delta: Value) -> Vec<Value> {
    vec![
        init(&[]),
        start("m"),
        block(0, text("")),
        super::delta(0, "answer"),
        stop_block(0),
        assistant("m", json!([text("answer")])),
        delta,
        stop(),
        terminal(),
    ]
}
#[test]
fn published_empty_message_delta_metadata_is_inert_not_a_content_mutation() {
    let (summary, events) = run(&sequence(delta_with_metadata()), &[]).unwrap();
    assert_eq!(summary.outcome, Some(Outcome::Success));
    assert_eq!(
        display(&events).values().cloned().collect::<Vec<_>>(),
        vec!["answer"]
    );
}
#[test]
fn known_context_management_is_cli_owned_inert_telemetry() {
    let (_, baseline) = run(&sequence(delta_with_metadata()), &[]).unwrap();
    for edits in [
        json!([]),
        json!([
            {"type":"clear_thinking_20251015","cleared_input_tokens":12,"cleared_thinking_turns":1},
            {"type":"clear_tool_uses_20250919","cleared_input_tokens":30,"cleared_tool_uses":2}
        ]),
    ] {
        let mut record = delta_with_metadata();
        record["event"]["context_management"] = json!({"applied_edits":edits});
        let (summary, events) = run(&sequence(record), &[]).unwrap();
        assert_eq!(summary.outcome, Some(Outcome::Success));
        assert_eq!(events.len(), baseline.len());
        assert_eq!(display(&events), display(&baseline));
    }
    for metadata in [
        json!({}),
        json!([]),
        json!({"applied_edits":[],"content":SECRET}),
        json!({"applied_edits":[{"type":"clear_thinking_20251015","cleared_input_tokens":-1,"cleared_thinking_turns":1}]}),
    ] {
        let mut record = delta_with_metadata();
        record["event"]["context_management"] = metadata;
        assert!(run(&sequence(record), &[]).is_err());
    }
}

#[test]
fn nonempty_unqualified_metadata_and_mutation_fields_remain_rejected() {
    for (field, value) in [
        ("container", json!({"id":SECRET})),
        ("stop_details", json!({"type":SECRET})),
        ("text", json!(SECRET)),
    ] {
        let mut record = delta_with_metadata();
        record["event"]["delta"][field] = value;
        assert!(run(&sequence(record), &[]).is_err());
    }
    for (field, value) in [
        ("context_management", json!({"applied_edits":[SECRET]})),
        ("input_transformations", json!([SECRET])),
        ("content", json!([SECRET])),
    ] {
        let mut record = delta_with_metadata();
        record["event"][field] = value;
        assert!(run(&sequence(record), &[]).is_err());
    }
}
