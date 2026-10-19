use super::*;
fn fixture() -> (Store, Arc<Lease>, Projection) {
    let s = Store::memory().unwrap();
    let b = s
        .create_cli_agent_session(&cli_agent::tests::create("s", "/fixture"), 1)
        .unwrap()
        .0;
    let lease = Arc::new(
        s.admit_cli_agent_turn(
            &Input {
                images: &[],
                mode: pipkin_engine_protocol::InputMode::Prompt,
                session: "s",
                request: "r",
                generation: 1,
                operation: "op",
                binding: b.digest,
                text: "input",
            },
            "owner",
            2,
        )
        .unwrap()
        .lease
        .unwrap(),
    );
    s.prepare_cli_tool_bridge(
        &cli_bridge::BridgeSpec {
            id: "b".into(),
            scope: lease.receipt.scope.clone(),
            owner: "owner".into(),
            binding_hash: [3; 32],
            capability_hash: [4; 32],
        },
        3,
    )
    .unwrap();
    s.begin_cli_agent_launch(&lease, "b", 4).unwrap();
    s.mark_cli_agent_running(&lease, 5).unwrap();
    let p = Projection::new(lease.clone());
    (s, lease, p)
}
fn delta(offset: usize, text: &str) -> Observation {
    Observation::Delta {
        message_id: "m".into(),
        block_index: 0,
        kind: TextKind::Text,
        offset,
        text: text.into(),
    }
}
fn snapshot(text: &str) -> Observation {
    Observation::Snapshot {
        message_id: "m".into(),
        block_index: 0,
        kind: TextKind::Text,
        text: text.into(),
    }
}
fn text(s: &Store) -> String {
    let entries = s.page("s", None, 20).unwrap().0;
    let EntryKind::Message {
        message: Message::Assistant { content, .. },
        ..
    } = &entries.last().unwrap().kind
    else {
        panic!()
    };
    let Block::Text { text, .. } = &content[0] else {
        panic!()
    };
    text.clone()
}
#[test]
fn deltas_are_byte_scoped_and_snapshot_repair_does_not_reapply_chunks() {
    let (s, l, mut p) = fixture();
    let scope = &l.receipt.scope;
    p.apply(&s, scope, vec![Observation::Initialized, delta(0, "é")], 6)
        .unwrap();
    assert_eq!(text(&s), "é");
    assert!(p.apply(&s, scope, vec![delta(1, "bad")], 7).is_err());
    assert_eq!(text(&s), "é");
    p.apply(&s, scope, vec![delta(2, "!"), snapshot("done")], 8)
        .unwrap();
    assert_eq!(text(&s), "done");
    assert_eq!(
        s.read(
            |c| Ok(c.query_row("SELECT COUNT(*) FROM entry_chunks", [], |r| r
                .get::<_, u64>(0))?)
        )
        .unwrap(),
        0
    );
}
#[test]
fn storage_failure_rolls_back_projection_cursor_and_stale_scope_never_changes_it() {
    let (s, l, mut p) = fixture();
    let scope = &l.receipt.scope;
    let mut stale = scope.clone();
    stale.generation += 1;
    assert!(
        p.apply(&s, &stale, vec![Observation::Initialized], 6)
            .is_err()
    );
    s.fail_next_write();
    assert!(
        p.apply(
            &s,
            scope,
            vec![Observation::Initialized, snapshot("lost")],
            7
        )
        .is_err()
    );
    assert_eq!(s.entry_count("s").unwrap(), 1);
    p.apply(
        &s,
        scope,
        vec![Observation::Initialized, snapshot("kept")],
        8,
    )
    .unwrap();
    assert_eq!(text(&s), "kept");
}
#[test]
fn terminal_and_finalized_text_are_not_root_completion_and_stop_retires_observations() {
    let (s, l, mut p) = fixture();
    let scope = &l.receipt.scope;
    p.apply(
        &s,
        scope,
        vec![
            Observation::Initialized,
            snapshot("answer"),
            Observation::Terminal {
                metadata: None,
                outcome: Outcome::Success,
                usage: None,
            },
        ],
        6,
    )
    .unwrap();
    p.finish(&s, 7, false).unwrap();
    assert_eq!(
        s.cli_agent_turn("s", "r").unwrap().unwrap().state,
        State::Running
    );
    assert!(p.apply(&s, scope, vec![snapshot("late")], 8).is_err());
    s.abandon_cli_agent_turn(&l, 9).unwrap();
    assert_eq!(
        s.cli_agent_turn("s", "r").unwrap().unwrap().state,
        State::Unknown
    );
    assert!(s.page("s", None, 20).unwrap().0.iter().all(|e| e.retired));
    assert_eq!(text(&s), "answer");
}
#[test]
fn restart_retains_committed_prefix_and_never_reconstructs_projection_authority() {
    let (s, l, mut p) = fixture();
    p.apply(
        &s,
        &l.receipt.scope,
        vec![Observation::Initialized, delta(0, "prefix")],
        6,
    )
    .unwrap();
    drop(p);
    assert_eq!(s.recover_cli_agent_turns(7).unwrap(), 1);
    assert_eq!(text(&s), "prefix");
    let mut p = Projection::new(l.clone());
    assert!(
        p.apply(&s, &l.receipt.scope, vec![Observation::Initialized], 8)
            .is_err()
    );
    assert!(s.page("s", None, 20).unwrap().0.iter().all(|e| e.retired));
}
#[test]
fn size_and_order_errors_roll_back_the_entire_projection_batch() {
    let (s, l, mut p) = fixture();
    let scope = &l.receipt.scope;
    assert!(
        p.apply(
            &s,
            scope,
            vec![
                Observation::Initialized,
                snapshot(&"x".repeat(MAX_PROJECTED_BYTES + 1))
            ],
            6
        )
        .is_err()
    );
    assert!(
        p.apply(&s, scope, vec![snapshot("uninitialized")], 7)
            .is_err()
    );
    p.apply(
        &s,
        scope,
        vec![Observation::Initialized, snapshot("valid")],
        8,
    )
    .unwrap();
    assert!(
        p.apply(
            &s,
            scope,
            vec![
                Observation::Terminal {
                    metadata: None,
                    outcome: Outcome::Success,
                    usage: None
                },
                snapshot("late")
            ],
            9
        )
        .is_err()
    );
    assert!(p.terminal().is_none());
    assert_eq!(text(&s), "valid");
}

#[test]
fn features_thinking_and_metadata_project_once_without_recharging_subsets() {
    let (s, l, mut p) = fixture();
    let metadata=crate::execution::claude_cli::usage_metadata::parse(&serde_json::json!({"duration_ms":42,"total_cost_usd":0.25,"modelUsage":{"sonnet":{"inputTokens":100000,"contextWindow":200000}}}).to_string()).unwrap();
    let usage = crate::execution::claude_cli::events::Usage {
        input_tokens: Some(10),
        output_tokens: Some(20),
        cache_read_input_tokens: Some(30),
        cache_creation_input_tokens: Some(40),
        reasoning_tokens: Some(15),
        cache_write_1h: Some(10),
    };
    p.apply(
        &s,
        &l.receipt.scope,
        vec![
            Observation::Initialized,
            Observation::Delta {
                message_id: "thought".into(),
                block_index: 0,
                kind: TextKind::Thinking,
                offset: 0,
                text: "thinking 雪".into(),
            },
            snapshot("answer"),
            Observation::Terminal {
                outcome: Outcome::Success,
                usage: Some(usage),
                metadata: Some(metadata.clone()),
            },
        ],
        6,
    )
    .unwrap();
    s.fail_next_write();
    assert!(p.finish(&s, 7, false).is_err());
    p.finish(&s, 8, false).unwrap();
    assert!(p.finish(&s, 9, false).is_err());
    let entries = s.page("s", None, 20).unwrap().0;
    let mut tokens = 0;
    let mut reports = 0;
    let mut thoughts = 0;
    for entry in entries {
        if let EntryKind::Message {
            message:
                Message::Assistant {
                    content,
                    usage,
                    extra,
                    ..
                },
            partial,
            ..
        } = entry.kind
        {
            assert!(!partial);
            tokens += usage.total_tokens;
            if matches!(&content[0],Block::Thinking{thinking,..} if thinking=="thinking 雪") {
                thoughts += 1;
            }
            if let Some(report) = extra.get("cliUsageMetadata") {
                reports += 1;
                assert_eq!(report, &serde_json::to_value(&metadata).unwrap());
                assert_eq!(usage.reasoning, Some(15));
                assert_eq!(usage.cache_write_1h, Some(10));
                assert_eq!(extra["costUnavailable"], true);
                assert_eq!(usage.cost.total, 0.0);
                assert_eq!(extra["usageUnavailable"], false);
            }
        }
    }
    assert_eq!((tokens, reports, thoughts), (100, 1, 1));
    let encoded = serde_json::to_string(&p.terminal()).unwrap();
    let terminal: Option<ExternalTerminal> = serde_json::from_str(&encoded).unwrap();
    assert_eq!(terminal, p.terminal());
    let old:ExternalTerminal=serde_json::from_value(serde_json::json!({"outcome":"success","usage":{"input_tokens":1,"output_tokens":2,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}})).unwrap();
    assert!(old.metadata.is_none());
    assert!(old.usage.unwrap().reasoning_tokens.is_none());
    // Projection and a reported success still cannot grant execution completion.
    assert_eq!(
        s.cli_agent_turn("s", "r").unwrap().unwrap().state,
        State::Running
    );
}
