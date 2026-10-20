use super::*;

fn result(id: &str, failed: bool, tokens: u64) -> Value {
    envelope(
        "result",
        json!({"uuid":id,"subtype":if failed {"error_during_execution"} else {"success"},"is_error":failed,"result":"final answer","usage":{"input_tokens":tokens,"output_tokens":tokens}}),
    )
}
fn finish(d: Decoder, events: &mut Vec<Event>) -> Result<Summary, Error> {
    d.finish(|e| {
        events.push(e);
        Ok::<_, ()>(())
    })
}

#[test]
fn held_coding_results_settle_once_at_eof_count_each_turn_and_preserve_final_text() {
    let records = [
        init(&[]),
        assistant("a", json!([text("final answer")])),
        result("r1", false, 2),
        result("r2", false, 3),
        result("r2", false, 3),
        result("r3", false, 5),
    ];
    // Exercise arbitrary pipe fragmentation, including several results in one chunk.
    for chunk in [1, 17, usize::MAX] {
        let mut d = decoder().with_cli_tools();
        let mut events = vec![];
        for part in ndjson(&records).chunks(chunk) {
            d.feed(part, |e| {
                events.push(e);
                Ok::<_, ()>(())
            })
            .unwrap();
        }
        assert!(!events.iter().any(|e| matches!(e, Event::Terminal { .. })));
        assert_eq!(
            finish(d, &mut events).unwrap().outcome,
            Some(Outcome::Success)
        );
        let terminal: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, Event::Terminal { .. }))
            .collect();
        assert_eq!(terminal.len(), 1);
        assert!(matches!(
            terminal[0],
            Event::Terminal {
                outcome: Outcome::Success,
                usage: Some(Usage {
                    input_tokens: Some(10),
                    output_tokens: Some(10),
                    ..
                }),
                ..
            }
        ));
        assert_eq!(
            display(&events).values().cloned().collect::<Vec<_>>(),
            vec!["final answer"]
        );
    }
}

#[test]
fn coding_continuation_needs_its_own_result_and_preserves_failure_and_unknown_usage() {
    for failed in [false, true] {
        let mut d = decoder().with_cli_tools();
        let mut out = vec![];
        let records = [
            init(&[]),
            result("first", failed, 2),
            init(&[]),
            assistant("next", json!([text("next answer")])),
        ];
        feed(&mut d, &records, &mut out).unwrap();
        // Redelivery of the first result cannot settle new work.
        feed(&mut d, &[result("first", failed, 2)], &mut out).unwrap();
        assert_eq!(finish(d, &mut out), Err(Error::Incomplete));
        let mut d = decoder().with_cli_tools();
        let mut out = vec![];
        feed(&mut d, &records, &mut out).unwrap();
        let mut last = result("last", false, 3);
        last.as_object_mut().unwrap().remove("usage");
        feed(&mut d, &[last], &mut out).unwrap();
        let summary = finish(d, &mut out).unwrap();
        assert_eq!(
            summary.outcome,
            Some(if failed {
                Outcome::Error
            } else {
                Outcome::Success
            })
        );
        assert!(matches!(
            out.last(),
            Some(Event::Terminal { usage: None, .. })
        ));
    }
    let mut d = decoder().with_cli_tools();
    let mut out = vec![];
    let mut future = result("future", false, 1);
    future["subtype"] = json!("future_status");
    feed(
        &mut d,
        &[init(&[]), future, result("success", false, 1)],
        &mut out,
    )
    .unwrap();
    assert_eq!(finish(d, &mut out).unwrap().outcome, Some(Outcome::Unknown));
}

#[test]
fn coding_result_identity_contradictions_bounds_and_sink_errors_remain_failures() {
    for (last, error) in [
        (result("first", true, 1), Error::Sequence),
        (result("first", false, 2), Error::Sequence),
        (result("", false, 1), Error::Identity),
        (terminal(), Error::Identity),
        (
            {
                let mut r = result("other", false, 1);
                r["session_id"] = json!(OTHER);
                r
            },
            Error::Identity,
        ),
        (
            {
                let mut r = result("other", false, 1);
                r["message"] = json!({});
                r
            },
            Error::UnsupportedContent,
        ),
    ] {
        let mut d = decoder().with_cli_tools();
        assert_eq!(
            feed(
                &mut d,
                &[init(&[]), result("first", false, 1), last],
                &mut vec![]
            ),
            Err(error)
        );
    }
    let mut d = decoder().with_cli_tools();
    assert_eq!(
        feed(
            &mut d,
            &[
                init(&[]),
                result("first", false, i64::MAX as u64),
                result("last", false, 1)
            ],
            &mut vec![]
        ),
        Err(Error::Limit)
    );
    let mut d = decoder().with_cli_tools();
    feed(&mut d, &[init(&[]), result("first", false, 1)], &mut vec![]).unwrap();
    assert_eq!(d.finish(|_| Err::<(), _>(())), Err(Error::SinkRejected));
    for exact in [false, true] {
        let mut d = decoder();
        if exact {
            d = d.with_exact_inventory();
        }
        assert_eq!(
            feed(
                &mut d,
                &[init(&[]), result("a", false, 1), result("b", false, 1)],
                &mut vec![]
            ),
            Err(Error::Sequence)
        );
    }
}

#[test]
fn old_result_redelivery_does_not_end_an_active_tool_turn_and_final_line_needs_no_newline() {
    let mut d = decoder().with_cli_tools();
    let mut out = vec![];
    feed(
        &mut d,
        &[
            init(&["Bash"]),
            result("early", false, 2),
            init(&["Bash"]),
            start("working"),
            block(0, tool("call", "Bash")),
            result("early", false, 2),
            stop_block(0),
            stop(),
            tool_result("call", "done"),
        ],
        &mut out,
    )
    .unwrap();
    assert!(d.summary().outcome.is_none());
    // Finish must consume the last complete JSON record even without a trailing LF.
    d.feed(result("last", false, 3).to_string().as_bytes(), |e| {
        out.push(e);
        Ok::<_, ()>(())
    })
    .unwrap();
    assert!(!out.iter().any(|e| matches!(e, Event::Terminal { .. })));
    assert_eq!(finish(d, &mut out).unwrap().outcome, Some(Outcome::Success));
    assert!(matches!(
        out.last(),
        Some(Event::Terminal {
            usage: Some(Usage {
                input_tokens: Some(5),
                ..
            }),
            ..
        })
    ));
}

#[test]
fn distinct_coding_results_are_bounded() {
    let mut d = decoder().with_cli_tools();
    feed(&mut d, &[init(&[])], &mut vec![]).unwrap();
    for i in 0..MAX_ENTRIES {
        feed(
            &mut d,
            &[result(&format!("result-{i}"), false, 0)],
            &mut vec![],
        )
        .unwrap();
    }
    assert_eq!(
        feed(&mut d, &[result("overflow", false, 0)], &mut vec![]),
        Err(Error::Limit)
    );
}
