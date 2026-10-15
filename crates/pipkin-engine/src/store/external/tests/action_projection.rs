use super::*;
fn coding(s: &Store) {
    let mut binding = spec();
    binding.launch_policy = EXTERNAL_PERMISSION_POLICY.into();
    s.create_cli_session("cli", "create", &binding, 1).unwrap();
}
fn observation(index: usize, id: &str) -> CliEvent {
    CliEvent::ExternalToolObserved {
        message_id: "msg".into(),
        block_index: index,
        tool_id: id.into(),
        name: "Write".into(),
    }
}
fn result(id: &str, error: Option<bool>) -> CliEvent {
    CliEvent::ExternalToolResult {
        tool_id: id.into(),
        is_error: error,
        text: "CLI output, not a native tool result".into(),
    }
}
fn text(index: usize, value: &str) -> CliEvent {
    CliEvent::BlockSnapshot {
        message_id: "msg".into(),
        block_index: index,
        kind: TextKind::Text,
        text: value.into(),
    }
}
fn blocks(s: &Store) -> Vec<ExternalBlock> {
    s.read(|c| {
        let id: u64 = c.query_row(
            "SELECT entry FROM external_projection_messages WHERE message_id='msg'",
            [],
            |r| r.get(0),
        )?;
        let entry = Store::entry_in(c, "cli", id)?.unwrap();
        let EntryKind::ExternalMessage { blocks, .. } = entry.kind else {
            panic!()
        };
        Ok(blocks)
    })
    .unwrap()
}
#[test]
fn mixed_text_and_actions_are_receipt_backed_inert_and_not_approval_correlated() {
    let s = Store::memory().unwrap();
    coding(&s);
    let r = admit(&s, "r", "op");
    let k = &r.key;
    start(&s, k);
    event(&s, k, 2, text(0, "before"));
    event(&s, k, 3, observation(1, "call-a"));
    event(&s, k, 4, text(2, "after"));
    let changes = event(&s, k, 5, result("call-a", Some(false)));
    assert!(matches!(
        &changes[..],
        [SessionChange::EntryReplaced { .. }]
    ));
    assert!(matches!(
        &blocks(&s)[..],
        [
            ExternalBlock::Text { .. },
            ExternalBlock::Action {
                state: ExternalActionState::ReportedSuccess,
                ..
            },
            ExternalBlock::Text { .. }
        ]
    ));
    assert_eq!(
        s.external_actions(k).unwrap()[0].status,
        ExternalActionStatus::Completed
    );
    // No guessed approval ID, native run, request, or tool intent is created.
    for table in ["external_permissions", "runs", "requests", "tool_calls"] {
        assert_eq!(count(&s, table), 0);
    }
    assert!(
        s.record_external_action(k, "other", "Write", ExternalActionStatus::Observed, 7)
            .is_err()
    );
    terminal(&s, k, 6);
    let settled = s
        .tx(|tx| Store::settle_external_prompt_in(tx, k, &success(), 8))
        .unwrap()
        .0;
    assert_eq!(settled.receipt.state, ExternalRunState::Completed);
    assert!(
        matches!(settled.receipt.result, Some(PromptResult::Done { text, .. }) if text == "beforeafter")
    );
}
#[test]
fn duplicates_holes_orphans_wrong_tools_and_block_type_changes_roll_back() {
    let s = Store::memory().unwrap();
    coding(&s);
    let r = admit(&s, "r", "op");
    let k = &r.key;
    start(&s, k);
    for invalid in [
        observation(1, "a"),
        result("missing", Some(false)),
        CliEvent::ExternalToolObserved {
            message_id: "msg".into(),
            block_index: 0,
            tool_id: "x".into(),
            name: "Bash".into(),
        },
    ] {
        assert!(
            s.tx(|tx| Store::project_external_event_in(tx, k, 2, &invalid, 5))
                .is_err()
        );
        assert_eq!(count(&s, "external_actions"), 0);
        assert_eq!(count(&s, "external_projection_messages"), 0);
        assert_eq!(s.external_prompt_for_run(k).unwrap().unwrap().cursor, 1);
    }
    event(&s, k, 2, observation(0, "a"));
    for invalid in [
        observation(1, "a"),
        observation(0, "b"),
        text(0, "replace action"),
        delta("change action", 0),
    ] {
        assert!(
            s.tx(|tx| Store::project_external_event_in(tx, k, 3, &invalid, 6))
                .is_err()
        );
    }
    event(&s, k, 3, result("a", Some(true)));
    assert!(
        s.tx(|tx| Store::project_external_event_in(tx, k, 4, &result("a", Some(false)), 7))
            .is_err()
    );
    assert!(matches!(
        &blocks(&s)[0],
        ExternalBlock::Action {
            state: ExternalActionState::ReportedFailure,
            ..
        }
    ));
    assert_eq!(
        s.external_actions(k).unwrap()[0].status,
        ExternalActionStatus::Failed
    );
}
#[test]
fn unknown_or_missing_result_blocks_completion_and_retires_without_replay() {
    for has_unknown_result in [false, true] {
        let s = Store::memory().unwrap();
        coding(&s);
        let r = admit(&s, "r", "op");
        let k = &r.key;
        start(&s, k);
        event(&s, k, 2, observation(0, "a"));
        let cursor = if has_unknown_result {
            event(&s, k, 3, result("a", None));
            4
        } else {
            3
        };
        terminal(&s, k, cursor);
        assert!(
            s.tx(|tx| Store::settle_external_prompt_in(tx, k, &success(), 8))
                .is_err()
        );
        assert_eq!(
            s.external_prompt_for_run(k).unwrap().unwrap().state,
            ExternalRunState::Running
        );
        s.tx(|tx| {
            Store::settle_external_prompt_in(tx, k, &ExternalSettlementEvidence::default(), 9)
        })
        .unwrap();
        assert!(matches!(
            &blocks(&s)[0],
            ExternalBlock::Action {
                state: ExternalActionState::Unknown,
                ..
            }
        ));
        assert_eq!(
            s.external_actions(k).unwrap()[0].status,
            ExternalActionStatus::Unknown
        );
        assert!(!s.cli_binding("cli").unwrap().unwrap().context_valid);
        assert!(
            s.tx(|tx| Store::admit_external_prompt_in(
                tx, "cli", "again", "no retry", "again", 2, 10
            ))
            .is_err()
        );
    }
}
#[test]
fn action_budget_and_transaction_failure_leave_cursor_and_projection_unchanged() {
    let s = Store::memory().unwrap();
    coding(&s);
    let r = admit(&s, "r", "op");
    let k = &r.key;
    start(&s, k);
    let failed = s.tx(|tx| {
        Store::project_external_event_in(tx, k, 2, &observation(0, "a"), 5)?;
        Err::<(), _>(StoreError::ExecutionConflict)
    });
    assert!(failed.is_err());
    for table in [
        "external_actions",
        "external_projection_actions",
        "external_projection_messages",
    ] {
        assert_eq!(count(&s, table), 0);
    }
    event(&s, k, 2, observation(0, "a"));
    let large = CliEvent::ExternalToolResult {
        tool_id: "a".into(),
        is_error: Some(false),
        text: "x".repeat(1024 * 1024),
    };
    assert!(
        s.tx(|tx| Store::project_external_event_in(tx, k, 3, &large, 6))
            .is_err()
    );
    assert_eq!(s.external_prompt_for_run(k).unwrap().unwrap().cursor, 2);
    assert!(matches!(
        &blocks(&s)[0],
        ExternalBlock::Action {
            state: ExternalActionState::Observed,
            ..
        }
    ));
}
#[test]
fn restart_and_removal_preserve_uncertainty_and_clean_indexed_projection() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    coding(&s);
    let r = admit(&s, "r", "op");
    start(&s, &r.key);
    event(&s, &r.key, 2, observation(0, "a"));
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    s.recover_external_runs(10).unwrap();
    assert!(matches!(
        &blocks(&s)[0],
        ExternalBlock::Action {
            state: ExternalActionState::Unknown,
            ..
        }
    ));
    assert_eq!(
        s.external_prompt_for_run(&r.key).unwrap().unwrap().state,
        ExternalRunState::Unknown
    );
    s.integrity_check().unwrap();
    s.begin_removal("cli").unwrap();
    s.finish_removal("cli").unwrap();
    for table in [
        "external_actions",
        "external_projection_actions",
        "external_projection_messages",
    ] {
        assert_eq!(count(&s, table), 0);
    }
    s.integrity_check().unwrap();
}
#[test]
fn action_ids_are_scoped_to_prompt_not_recovered_by_name_or_order() {
    let s = Store::memory().unwrap();
    coding(&s);
    let first = admit(&s, "r1", "op1");
    start(&s, &first.key);
    event(&s, &first.key, 2, observation(0, "same"));
    event(&s, &first.key, 3, result("same", Some(false)));
    terminal(&s, &first.key, 4);
    s.tx(|tx| Store::settle_external_prompt_in(tx, &first.key, &success(), 8))
        .unwrap();
    let second = admit(&s, "r2", "op2");
    assert_eq!(second.mode, ExternalPromptMode::Resume);
    start(&s, &second.key);
    assert!(
        s.tx(|tx| Store::project_external_event_in(
            tx,
            &second.key,
            2,
            &result("same", Some(false)),
            9
        ))
        .is_err()
    );
    event(&s, &second.key, 2, observation(0, "same"));
    assert_eq!(
        s.external_actions(&first.key).unwrap()[0].status,
        ExternalActionStatus::Completed
    );
    assert_eq!(
        s.external_actions(&second.key).unwrap()[0].status,
        ExternalActionStatus::Observed
    );
}

#[test]
fn decoder_mixed_snapshot_projects_without_inputs_or_native_context() {
    use crate::execution::claude_cli::events::Decoder;
    let s = Store::memory().unwrap();
    coding(&s);
    let r = admit(&s, "r", "op");
    s.tx(|tx| Store::begin_external_launch_in(tx, &r.key, 3))
        .unwrap();
    s.tx(|tx| Store::begin_external_run_in(tx, &r.key, 4))
        .unwrap();
    let session = spec().cli_session_id;
    let records = [
        serde_json::json!({"type":"system","subtype":"init","session_id":session,"tools":["Write"]}),
        serde_json::json!({"type":"assistant","session_id":session,"message":{"id":"msg","role":"assistant","content":[
            {"type":"text","text":"before"},
            {"type":"tool_use","id":"a","name":"Write","input":{"secret":"PRIVATE_INPUT_NEVER_STORED"}},
            {"type":"text","text":"after"}]}}),
        serde_json::json!({"type":"user","session_id":session,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"a","content":"reported output","is_error":false}]}}),
        serde_json::json!({"type":"result","session_id":session,"subtype":"success","is_error":false,"result":"not projected twice"}),
    ];
    let bytes = records.iter().map(|v| format!("{v}\n")).collect::<String>();
    let mut decoder = Decoder::new(&session, &["Write"]).unwrap();
    let mut cursor = 0;
    let mut sink = |event| {
        cursor += 1;
        s.tx(|tx| Store::project_external_event_in(tx, &r.key, cursor, &event, 5))
            .map(|_| ())
    };
    for chunk in bytes.as_bytes().chunks(7) {
        decoder.feed(chunk, &mut sink).unwrap();
    }
    decoder.finish(&mut sink).unwrap();
    let blocks = blocks(&s);
    assert!(matches!(
        &blocks[..],
        [
            ExternalBlock::Text { .. },
            ExternalBlock::Action {
                state: ExternalActionState::ReportedSuccess,
                ..
            },
            ExternalBlock::Text { .. }
        ]
    ));
    assert!(
        !serde_json::to_string(&blocks)
            .unwrap()
            .contains("PRIVATE_INPUT")
    );
    assert_eq!(count(&s, "tool_calls"), 0);
    assert_eq!(count(&s, "external_permissions"), 0);
    let entry = s
        .read(|c| {
            let id = c.query_row("SELECT entry FROM external_projection_messages", [], |r| {
                r.get(0)
            })?;
            Store::entry_in(c, "cli", id)
        })
        .unwrap()
        .unwrap();
    assert!(crate::context::derive::model_messages(&[entry]).is_empty());
}

#[test]
fn schema24_backup_preserves_identity_before_action_support() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    coding(&s);
    s.tx(|tx| {
        tx.execute_batch("DROP TABLE external_projection_actions; PRAGMA user_version=24;")?;
        Ok(())
    })
    .unwrap();
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    assert_eq!(s.read(schema::version).unwrap(), 25);
    let old = Connection::open(dir.path().join("engine.sqlite.v24.bak")).unwrap();
    assert_eq!(schema::version(&old).unwrap(), 24);
    assert!(
        old.prepare("SELECT * FROM external_projection_actions")
            .is_err()
    );
    assert_eq!(
        s.cli_binding("cli")
            .unwrap()
            .unwrap()
            .identity
            .launch_policy,
        EXTERNAL_PERMISSION_POLICY
    );
    s.integrity_check().unwrap();
}
