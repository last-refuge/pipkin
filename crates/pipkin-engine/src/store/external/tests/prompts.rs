use super::*;
use crate::execution::claude_cli::events::{Event as CliEvent, Outcome, TextKind};
#[path = "action_projection.rs"]
mod actions;

fn admit(s: &Store, request: &str, op: &str) -> ExternalPromptReceipt {
    s.tx(|tx| Store::admit_external_prompt_in(tx, "cli", request, "question", op, 1, 2))
        .unwrap()
        .0
        .receipt
}
fn start(s: &Store, k: &ExternalRunKey) {
    s.tx(|tx| Store::begin_external_launch_in(tx, k, 3))
        .unwrap();
    s.tx(|tx| Store::begin_external_run_in(tx, k, 4)).unwrap();
    event(s, k, 1, CliEvent::Initialized);
}
fn event(s: &Store, k: &ExternalRunKey, cursor: u64, event: CliEvent) -> Vec<SessionChange> {
    s.tx(|tx| Store::project_external_event_in(tx, k, cursor, &event, 5))
        .unwrap()
}
fn delta(text: &str, offset: usize) -> CliEvent {
    CliEvent::BlockDelta {
        message_id: "msg".into(),
        block_index: 0,
        kind: TextKind::Text,
        offset,
        text: text.into(),
    }
}
fn success() -> ExternalSettlementEvidence {
    ExternalSettlementEvidence {
        projection_finished: true,
        input_delivered: true,
        root_reaped: true,
        pipes_drained: true,
        exit_code: Some(0),
        ..Default::default()
    }
}
fn terminal(s: &Store, k: &ExternalRunKey, cursor: u64) {
    event(
        s,
        k,
        cursor,
        CliEvent::Terminal {
            metadata: None,
            outcome: Outcome::Success,
            usage: None,
        },
    );
}
fn count(s: &Store, table: &str) -> i64 {
    s.read(|c| Ok(c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?))
        .unwrap()
}

#[test]
fn legacy_completed_ledger_without_prompt_receipt_cannot_authorize_resume_or_fresh_launch() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    cli(&s);
    let legacy = key("legacy");
    running(&s, &legacy);
    s.transition_external_run(
        &legacy,
        ExternalRunState::Running,
        ExternalRunState::Completed,
        5,
    )
    .unwrap();
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    let before = s.session("cli").unwrap().unwrap().revision;
    assert!(matches!(
        s.tx(|tx| Store::admit_external_prompt_in(
            tx,
            "cli",
            "new-request",
            "question",
            "new-op",
            1,
            6
        )),
        Err(StoreError::ExecutionConflict)
    ));
    assert_eq!(count(&s, "external_prompts"), 0);
    assert_eq!(count(&s, "entries"), 0);
    assert_eq!(count(&s, "external_runs"), 1);
    assert_eq!(s.session("cli").unwrap().unwrap().revision, before);
}

#[test]
fn receipt_admission_is_atomic_and_external_only() {
    let s = Store::memory().unwrap();
    cli(&s);
    let rev = s.session("cli").unwrap().unwrap().revision;
    let head = s.catch_up(0, 256).unwrap().head;
    s.fail_next_write();
    assert!(matches!(
        s.tx(|tx| {
            let result =
                Store::admit_external_prompt_in(tx, "cli", "request", "question", "op", 1, 2)?;
            Store::bump(tx, "cli", 2)?;
            Ok(result)
        }),
        Err(StoreError::Injected)
    ));
    for table in [
        "external_prompts",
        "external_runs",
        "entries",
        "requests",
        "runs",
        "tool_calls",
    ] {
        assert_eq!(count(&s, table), 0);
    }
    assert_eq!(s.catch_up(0, 256).unwrap().head, head);
    let (a, changes) = s
        .tx(|tx| Store::admit_external_prompt_in(tx, "cli", "request", "question", "op", 1, 2))
        .unwrap();
    assert!(a.newly_admitted);
    assert_eq!(a.receipt.mode, ExternalPromptMode::Fresh);
    assert_eq!(changes.len(), 2);
    assert!(matches!(
        &changes[0],
        SessionChange::EntryAppended {
            entry: Entry {
                kind: EntryKind::ExternalMessage {
                    role: ExternalRole::User,
                    ..
                },
                ..
            }
        }
    ));
    assert!(
        matches!(&changes[1], SessionChange::RunChanged { run: Some(run) } if run.id == a.receipt.id)
    );
    assert_eq!(
        s.session("cli").unwrap().unwrap().revision,
        rev,
        "_in must not bump"
    );
    for table in ["requests", "runs", "tool_calls", "external_actions"] {
        assert_eq!(count(&s, table), 0);
    }
    s.integrity_check().unwrap();
}

#[test]
fn request_retry_precedes_binding_and_generation_checks_and_uses_protocol_keys() {
    let s = Store::memory().unwrap();
    cli(&s);
    let request = "λ !?".repeat(51); // 255 bytes, intentionally not an op token.
    assert_eq!(request.len(), 255);
    let first = admit(&s, &request, "op");
    assert_eq!(s.recover_external_runs(10).unwrap(), 1);
    let (a, changes) = s
        .tx(|tx| {
            Store::admit_external_prompt_in(tx, "cli", &request, "question", "not a token", 0, 11)
        })
        .unwrap();
    assert!(!a.newly_admitted);
    assert!(changes.is_empty());
    assert_eq!(a.receipt.id, first.id);
    assert_eq!(a.receipt.state, ExternalRunState::Unknown);
    conflict(
        s.tx(|tx| Store::admit_external_prompt_in(tx, "cli", &request, "changed", "op", 1, 11)),
    );
    conflict(
        s.tx(|tx| Store::admit_external_prompt_in(tx, "cli", "new", "question", "new", 2, 11)),
    );
    let s = Store::memory().unwrap();
    cli(&s);
    assert!(
        s.tx(|tx| Store::admit_external_prompt_in(
            tx,
            "cli",
            &"x".repeat(256),
            "question",
            "op",
            1,
            2
        ))
        .is_ok()
    );
    conflict(s.tx(|tx| {
        Store::admit_external_prompt_in(tx, "cli", &"x".repeat(257), "question", "op", 1, 2)
    }));
}

#[test]
fn launch_cas_raw_bypass_and_stale_projection_are_refused() {
    let s = Store::memory().unwrap();
    cli(&s);
    let r = admit(&s, "r", "op");
    let k = &r.key;
    conflict(s.tx(|tx| Store::begin_external_run_in(tx, k, 3)));
    conflict(s.admit_external_run(k, 3));
    conflict(s.transition_external_run(
        k,
        ExternalRunState::Admitted,
        ExternalRunState::LaunchIntent,
        3,
    ));
    s.tx(|tx| Store::begin_external_launch_in(tx, k, 3))
        .unwrap();
    conflict(s.tx(|tx| Store::begin_external_launch_in(tx, k, 3)));
    s.tx(|tx| Store::begin_external_run_in(tx, k, 4)).unwrap();
    conflict(s.tx(|tx| Store::begin_external_run_in(tx, k, 4)));
    conflict(s.record_external_action(k, "call", "Read", ExternalActionStatus::Observed, 5));
    let mut stale = k.clone();
    stale.generation += 1;
    conflict(s.tx(|tx| Store::project_external_event_in(tx, &stale, 1, &CliEvent::Initialized, 5)));
    stale = key("wrong");
    conflict(s.tx(|tx| Store::project_external_event_in(tx, &stale, 1, &CliEvent::Initialized, 5)));
    conflict(s.tx(|tx| Store::project_external_event_in(tx, k, 2, &CliEvent::Initialized, 5)));
    event(&s, k, 1, CliEvent::Initialized);
    conflict(s.tx(|tx| Store::project_external_event_in(tx, k, 1, &CliEvent::Initialized, 5)));
    conflict(s.tx(|tx| Store::project_external_event_in(tx, k, 2, &delta("oops", 1), 5)));
    assert_eq!(s.external_prompt("cli", "r").unwrap().unwrap().cursor, 1);
}

#[test]
fn projection_replaces_snapshots_and_settles_once_without_native_rows() {
    let s = Store::memory().unwrap();
    cli(&s);
    let r = admit(&s, "r", "op");
    let k = &r.key;
    start(&s, k);
    let changes = event(&s, k, 2, delta("partial", 0));
    let entry_id = match &changes[0] {
        SessionChange::EntryAppended { entry } => entry.id,
        _ => panic!(),
    };
    event(
        &s,
        k,
        3,
        CliEvent::BlockSnapshot {
            message_id: "msg".into(),
            block_index: 0,
            kind: TextKind::Text,
            text: "answer".into(),
        },
    );
    event(
        &s,
        k,
        4,
        CliEvent::BlockSnapshot {
            message_id: "msg".into(),
            block_index: 1,
            kind: TextKind::Thinking,
            text: "private thought".into(),
        },
    );
    assert_eq!(count(&s, "entries"), 2);
    let entry = s
        .read(|c| Store::entry_in(c, "cli", entry_id))
        .unwrap()
        .unwrap();
    assert!(matches!(
        entry.kind,
        EntryKind::ExternalMessage { partial: true, .. }
    ));
    conflict(s.tx(|tx| Store::project_external_event_in(tx, k, 5, &delta("late", 6), 6)));
    terminal(&s, k, 5);
    let before = s.external_prompt("cli", "r").unwrap().unwrap();
    assert!(before.result.is_none());
    assert_eq!(before.state, ExternalRunState::Running);
    conflict(s.tx(|tx| Store::project_external_event_in(tx, k, 6, &delta("late", 6), 6)));
    let (settled, changes) = s
        .tx(|tx| Store::settle_external_prompt_in(tx, k, &success(), 7))
        .unwrap();
    assert!(settled.newly_settled);
    assert_eq!(settled.receipt.state, ExternalRunState::Completed);
    assert_eq!(
        settled.receipt.result,
        Some(PromptResult::Done {
            text: "answer".into(),
            reason: None
        })
    );
    assert_eq!(
        changes
            .iter()
            .filter(|c| matches!(c, SessionChange::RunChanged { run: None }))
            .count(),
        1
    );
    assert_eq!(
        changes
            .iter()
            .filter(|c| matches!(c, SessionChange::RunSettled { .. }))
            .count(),
        1
    );
    let (again, changes) = s
        .tx(|tx| Store::settle_external_prompt_in(tx, k, &ExternalSettlementEvidence::default(), 8))
        .unwrap();
    assert!(!again.newly_settled);
    assert_eq!(again.receipt, settled.receipt);
    assert!(changes.is_empty());
    assert!(s.cli_binding("cli").unwrap().unwrap().context_valid);
    assert!(
        matches!(s.prompt_result("cli", "r").unwrap(), crate::store::prompt_result::PromptState::Settled(PromptResult::Done { text, .. }) if text == "answer")
    );
    let next = admit(&s, "r2", "op2");
    assert_eq!(next.mode, ExternalPromptMode::Resume);
    assert!(next.id > r.id);
    for table in ["requests", "runs", "tool_calls", "external_actions"] {
        assert_eq!(count(&s, table), 0);
    }
}

#[test]
fn rejected_output_keeps_one_inert_diagnostic_across_settlement_and_restart() {
    use crate::execution::claude_cli::{events, process};
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    cli(&s);
    let r = admit(&s, "r", "op");
    start(&s, &r.key);
    event(&s, &r.key, 2, delta("retained prefix", 0));
    let evidence = ExternalSettlementEvidence {
        input_delivered: true,
        root_reaped: true,
        pipes_drained: true,
        supervisor_stop: Some(process::Stop::ObserverFailure),
        projection_error: Some(events::Error::MalformedRecord),
        rejected_record: Some(events::RecordClass::ContentBlockDelta),
        parse_failure: Some(events::ParseFailure {
            kind: events::ParseFailureKind::UnknownField,
            field_sha256: Some([42; 32]),
            message_delta_shapes: None,
        }),
        ..Default::default()
    };
    let (settled, changes) = s
        .tx(|tx| Store::settle_external_prompt_in(tx, &r.key, &evidence, 8))
        .unwrap();
    assert_eq!(settled.receipt.state, ExternalRunState::Unknown);
    let Some(RunOutcome::Interrupted { detail }) = &settled.receipt.outcome else {
        panic!("unknown outcome")
    };
    for expected in [
        "unconfirmed",
        "malformed CLI record",
        "ContentBlockDelta",
        "UnknownField",
        "cannot resume",
    ] {
        assert!(detail.contains(expected), "{detail}");
    }
    assert!(!detail.contains("42")); // not even the rejected label's digest is displayed
    let (entries, _) = s.page("cli", None, 100).unwrap();
    assert_eq!(entries.len(), 3);
    let notice = entries.last().unwrap();
    assert_eq!(notice.run, Some(r.id));
    assert!(
        matches!(&notice.kind, EntryKind::Notice { level: pipkin_engine_protocol::NoticeLevel::Error, text } if text == detail)
    );
    assert_eq!(
        changes
            .iter()
            .filter(|c| matches!(c, SessionChange::EntryAppended { .. }))
            .count(),
        1
    );
    let notice_position = changes
        .iter()
        .position(|c| matches!(c, SessionChange::EntryAppended { .. }))
        .unwrap();
    let settled_position = changes
        .iter()
        .position(|c| matches!(c, SessionChange::RunSettled { .. }))
        .unwrap();
    assert!(notice_position < settled_position);
    let (again, changes) = s
        .tx(|tx| Store::settle_external_prompt_in(tx, &r.key, &success(), 9))
        .unwrap();
    assert!(!again.newly_settled);
    assert_eq!(again.receipt, settled.receipt);
    assert!(changes.is_empty());
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    assert_eq!(s.page("cli", None, 100).unwrap().0, entries);
    assert_eq!(
        s.external_prompt("cli", "r").unwrap().unwrap(),
        settled.receipt
    );
    assert!(!s.cli_binding("cli").unwrap().unwrap().context_valid);
    assert!(
        s.tx(|tx| Store::admit_external_prompt_in(
            tx,
            "cli",
            "new",
            "do not replay",
            "new-op",
            1,
            10
        ))
        .is_err()
    );
    for table in ["requests", "runs", "tool_calls", "external_actions"] {
        assert_eq!(count(&s, table), 0);
    }
}

#[test]
fn display_warnings_and_known_provider_errors_survive_restart_without_replay() {
    for provider_error in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        cli(&s);
        let r = admit(&s, "first", "first-op");
        start(&s, &r.key);
        event(&s, &r.key, 2, delta("answer", 0));
        event(
            &s,
            &r.key,
            3,
            CliEvent::Terminal {
                metadata: None,
                outcome: if provider_error {
                    Outcome::Error
                } else {
                    Outcome::Success
                },
                usage: None,
            },
        );
        let evidence = ExternalSettlementEvidence {
            presentation_incomplete: true,
            exit_code: Some(if provider_error { 1 } else { 0 }),
            ..success()
        };
        let (settled, _) = s
            .tx(|tx| Store::settle_external_prompt_in(tx, &r.key, &evidence, 8))
            .unwrap();
        assert!(s.cli_binding("cli").unwrap().unwrap().context_valid);
        assert_eq!(
            matches!(settled.receipt.outcome, Some(RunOutcome::ModelError { .. })),
            provider_error
        );
        let entries = s.page("cli", None, 100).unwrap().0;
        assert!(matches!(
            &entries.last().unwrap().kind,
            EntryKind::Notice { .. }
        ));
        drop(s);
        let s = Store::open(dir.path()).unwrap();
        assert_eq!(s.page("cli", None, 100).unwrap().0, entries);
        assert_eq!(
            s.external_prompt("cli", "first").unwrap().unwrap(),
            settled.receipt
        );
        let (same, changes) = s
            .tx(|tx| {
                Store::settle_external_prompt_in(
                    tx,
                    &r.key,
                    &ExternalSettlementEvidence::default(),
                    9,
                )
            })
            .unwrap();
        assert!(!same.newly_settled && changes.is_empty());
        assert_eq!(same.receipt, settled.receipt);
        assert_eq!(
            admit(&s, "new-instruction", "new-op").mode,
            ExternalPromptMode::Resume
        );
        for table in ["requests", "runs", "tool_calls"] {
            assert_eq!(count(&s, table), 0);
        }
    }
}

#[test]
fn completion_requires_every_independent_fact_and_persisted_success() {
    for missing in 0..15 {
        let s = Store::memory().unwrap();
        cli(&s);
        let r = admit(&s, "r", "op");
        let k = &r.key;
        start(&s, k);
        event(&s, k, 2, delta("prefix", 0));
        if missing != 0 {
            event(
                &s,
                k,
                3,
                CliEvent::Terminal {
                    metadata: None,
                    outcome: if missing == 10 {
                        Outcome::Error
                    } else {
                        Outcome::Success
                    },
                    usage: None,
                },
            );
        }
        let mut evidence = success();
        match missing {
            1 => evidence.projection_finished = false,
            2 => evidence.input_delivered = false,
            3 => evidence.root_reaped = false,
            4 => evidence.pipes_drained = false,
            5 => evidence.exit_code = Some(1),
            6 => evidence.exit_code = None,
            7 => evidence.stop_requested = true,
            8 => evidence.cleanup_timed_out = true,
            9 => evidence.cancelled_clean_confirmed = true,
            11 => {
                evidence.supervisor_stop =
                    Some(crate::execution::claude_cli::process::Stop::ObserverFailure)
            }
            12 => {
                evidence.projection_error =
                    Some(crate::execution::claude_cli::events::Error::MalformedRecord)
            }
            13 => {
                evidence.rejected_record =
                    Some(crate::execution::claude_cli::events::RecordClass::Unknown)
            }
            14 => {
                evidence.parse_failure = Some(crate::execution::claude_cli::events::ParseFailure {
                    kind: crate::execution::claude_cli::events::ParseFailureKind::Other,
                    field_sha256: None,
                    message_delta_shapes: None,
                })
            }
            _ => {}
        }
        let (settled, _) = s
            .tx(|tx| Store::settle_external_prompt_in(tx, k, &evidence, 8))
            .unwrap();
        assert_eq!(
            settled.receipt.state,
            if missing == 10 {
                ExternalRunState::Interrupted
            } else {
                ExternalRunState::Unknown
            },
            "case {missing}"
        );
        assert_eq!(
            settled.receipt.result,
            Some(PromptResult::Unanswered {
                text: Some("prefix".into()),
                reason: if missing == 10 {
                    "providerError"
                } else {
                    "interrupted"
                }
                .into()
            })
        );
        assert_eq!(
            s.cli_binding("cli").unwrap().unwrap().context_valid,
            missing == 10
        );
        let entries = s
            .read(|c| {
                Ok(c.query_row(
                    "SELECT COUNT(*) FROM entries WHERE retired=1 AND partial=0",
                    [],
                    |r| r.get::<_, i64>(0),
                )?)
            })
            .unwrap();
        assert_eq!(entries, 2);
    }
}

#[test]
fn stop_is_durable_idempotent_in_every_phase_and_never_completes() {
    for phase in 0..3 {
        for clean in [false, true] {
            let s = Store::memory().unwrap();
            cli(&s);
            let r = admit(&s, "r", "op");
            let k = &r.key;
            if phase > 0 {
                s.tx(|tx| Store::begin_external_launch_in(tx, k, 3))
                    .unwrap();
            }
            if phase > 1 {
                s.tx(|tx| Store::begin_external_run_in(tx, k, 4)).unwrap();
                event(&s, k, 1, CliEvent::Initialized);
                terminal(&s, k, 2);
            }
            let (receipt, changes) = s.tx(|tx| Store::stop_external_prompt_in(tx, k, 6)).unwrap();
            assert_eq!(receipt.state, ExternalRunState::Stopping);
            assert_eq!(changes.len(), 1);
            let (_, changes) = s.tx(|tx| Store::stop_external_prompt_in(tx, k, 7)).unwrap();
            assert!(changes.is_empty());
            conflict(s.tx(|tx| Store::begin_external_launch_in(tx, k, 8)));
            conflict(s.tx(|tx| Store::begin_external_run_in(tx, k, 8)));
            let mut evidence = success();
            evidence.cancelled_clean_confirmed = clean;
            let (settled, _) = s
                .tx(|tx| Store::settle_external_prompt_in(tx, k, &evidence, 9))
                .unwrap();
            assert_eq!(
                settled.receipt.state,
                if clean {
                    ExternalRunState::Interrupted
                } else {
                    ExternalRunState::Unknown
                }
            );
            if clean {
                assert_eq!(settled.receipt.outcome, Some(RunOutcome::Aborted));
            }
            assert!(!s.cli_binding("cli").unwrap().unwrap().context_valid);
            let (_, changes) = s
                .tx(|tx| Store::stop_external_prompt_in(tx, k, 10))
                .unwrap();
            assert!(changes.is_empty());
        }
    }
}

#[test]
fn projection_limits_tool_refusal_and_rollback_preserve_cursor() {
    let s = Store::memory().unwrap();
    cli(&s);
    let r = admit(&s, "r", "op");
    let k = &r.key;
    start(&s, k);
    s.fail_next_write();
    assert!(matches!(
        s.tx(|tx| Store::project_external_event_in(tx, k, 2, &delta("discard", 0), 5)),
        Err(StoreError::Injected)
    ));
    assert_eq!(count(&s, "external_projection_messages"), 0);
    assert_eq!(s.external_prompt("cli", "r").unwrap().unwrap().cursor, 1);
    for event in [
        CliEvent::ExternalToolObserved {
            message_id: "msg".into(),
            block_index: 0,
            tool_id: "tool".into(),
            name: "Read".into(),
        },
        CliEvent::ExternalToolResult {
            tool_id: "tool".into(),
            is_error: Some(false),
            text: "result".into(),
        },
    ] {
        assert!(matches!(
            s.tx(|tx| Store::project_external_event_in(tx, k, 2, &event, 5)),
            Err(StoreError::UnsupportedExecution)
        ));
    }
    conflict(s.tx(|tx| {
        Store::project_external_event_in(tx, k, 2, &delta(&"x".repeat(1024 * 1024 + 1), 0), 5)
    }));
    for i in 0..256 {
        event(
            &s,
            k,
            i + 2,
            CliEvent::BlockSnapshot {
                message_id: "msg".into(),
                block_index: i as usize,
                kind: TextKind::Text,
                text: "a".into(),
            },
        );
    }
    conflict(s.tx(|tx| {
        Store::project_external_event_in(
            tx,
            k,
            258,
            &CliEvent::BlockSnapshot {
                message_id: "msg".into(),
                block_index: 256,
                kind: TextKind::Text,
                text: "a".into(),
            },
            5,
        )
    }));
    assert_eq!(s.external_prompt("cli", "r").unwrap().unwrap().cursor, 257);
    s.integrity_check().unwrap();
}

#[test]
fn recovery_unknown_is_atomic_persistent_and_does_not_replay() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    cli(&s);
    let r = admit(&s, "r", "op");
    start(&s, &r.key);
    event(&s, &r.key, 2, delta("prefix", 0));
    terminal(&s, &r.key, 3);
    let rev = s.session("cli").unwrap().unwrap().revision;
    s.fail_next_write();
    assert!(matches!(
        s.recover_external_runs(10),
        Err(StoreError::Injected)
    ));
    assert_eq!(
        s.external_prompt("cli", "r").unwrap().unwrap().state,
        ExternalRunState::Running
    );
    assert!(s.cli_binding("cli").unwrap().unwrap().context_valid);
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    assert_eq!(s.active_external_prompts().unwrap().len(), 1);
    assert_eq!(s.recover_external_runs(11).unwrap(), 1);
    assert_eq!(s.recover_external_runs(12).unwrap(), 0);
    assert_eq!(s.session("cli").unwrap().unwrap().revision, rev + 1);
    assert!(s.active_external_prompts().unwrap().is_empty());
    let receipt = s.external_prompt("cli", "r").unwrap().unwrap();
    assert_eq!(receipt.state, ExternalRunState::Unknown);
    assert_eq!(
        receipt.result,
        Some(PromptResult::Unanswered {
            text: Some("prefix".into()),
            reason: "interrupted".into()
        })
    );
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    assert_eq!(s.external_prompt("cli", "r").unwrap(), Some(receipt));
    assert_eq!(s.recover_external_runs(13).unwrap(), 0);
}

#[test]
fn settlement_and_stop_rollback_leave_no_false_finality() {
    let s = Store::memory().unwrap();
    cli(&s);
    let r = admit(&s, "r", "op");
    start(&s, &r.key);
    event(&s, &r.key, 2, delta("answer", 0));
    terminal(&s, &r.key, 3);
    s.fail_next_write();
    assert!(matches!(
        s.tx(|tx| Store::stop_external_prompt_in(tx, &r.key, 7)),
        Err(StoreError::Injected)
    ));
    assert_eq!(
        s.external_prompt("cli", "r").unwrap().unwrap().state,
        ExternalRunState::Running
    );
    s.fail_next_write();
    assert!(matches!(
        s.tx(|tx| Store::settle_external_prompt_in(
            tx,
            &r.key,
            &ExternalSettlementEvidence::default(),
            8
        )),
        Err(StoreError::Injected)
    ));
    let receipt = s.external_prompt("cli", "r").unwrap().unwrap();
    assert!(receipt.result.is_none());
    assert!(receipt.evidence.is_none());
    assert!(s.cli_binding("cli").unwrap().unwrap().context_valid);
    assert_eq!(
        s.read(|c| Ok(c
            .query_row("SELECT COUNT(*) FROM entries WHERE partial=1", [], |r| r
                .get::<_, i64>(0))?))
            .unwrap(),
        1
    );
}

#[test]
fn prompt_removal_keeps_permanent_uuid_receipt() {
    let s = Store::memory().unwrap();
    cli(&s);
    let r = admit(&s, "r", "op");
    start(&s, &r.key);
    event(&s, &r.key, 2, delta("prefix", 0));
    s.begin_removal("cli").unwrap();
    s.fail_next_write();
    assert!(matches!(s.finish_removal("cli"), Err(StoreError::Injected)));
    assert!(s.external_prompt("cli", "r").unwrap().is_some());
    s.finish_removal("cli").unwrap();
    for table in [
        "external_prompts",
        "external_projection_messages",
        "external_runs",
        "cli_bindings",
        "entries",
    ] {
        assert_eq!(count(&s, table), 0);
    }
    assert_eq!(receipt_count(&s), 1);
    conflict(s.create_cli_session("new", "new-create", &spec(), 9));
    s.integrity_check().unwrap();
}

#[test]
fn recovery_closes_every_prompt_phase_including_already_fenced_bindings() {
    for phase in 0..4 {
        for fenced in [false, true] {
            let s = Store::memory().unwrap();
            cli(&s);
            let receipt = admit(&s, "r", "op");
            let k = &receipt.key;
            if phase >= 1 {
                s.tx(|tx| Store::begin_external_launch_in(tx, k, 3))
                    .unwrap();
            }
            if phase >= 2 {
                s.tx(|tx| Store::begin_external_run_in(tx, k, 4)).unwrap();
            }
            if phase >= 3 {
                s.tx(|tx| Store::stop_external_prompt_in(tx, k, 5)).unwrap();
            }
            if fenced {
                s.tx(|tx| {
                    tx.execute("UPDATE cli_bindings SET context_valid=0,generation=generation+1 WHERE session='cli'", [])?;
                    Ok(())
                }).unwrap();
            }
            assert_eq!(s.recover_external_runs(10).unwrap(), 1);
            assert_eq!(s.recover_external_runs(11).unwrap(), 0);
            let recovered = s.external_prompt_by_id(receipt.id).unwrap().unwrap();
            assert_eq!(recovered.state, ExternalRunState::Unknown);
            assert!(recovered.result.is_some());
            assert_eq!(s.cli_binding("cli").unwrap().unwrap().generation, 2);
        }
    }
}

#[test]
fn v21_backup_preserves_raw_ledger_and_migration_adds_only_prompt_tables() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    cli(&s);
    running(&s, &key("raw"));
    s.tx(|tx| { crate::store::cli_agent::remove_schema_for_test(tx)?; tx.execute_batch("DROP TABLE cli_bridge_calls; DROP TABLE cli_tool_bridges; DROP TABLE external_permissions; DROP TABLE external_projection_messages; DROP TABLE external_prompts; DROP TABLE external_projection_actions; PRAGMA user_version=21;")?; Ok(()) }).unwrap();
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    assert_eq!(s.read(schema::version).unwrap(), schema::SCHEMA_VERSION);
    assert_eq!(
        s.external_run(&key("raw")).unwrap().unwrap().state,
        ExternalRunState::Running
    );
    assert_eq!(count(&s, "external_prompts"), 0);
    let backup = Connection::open(dir.path().join("engine.sqlite.v21.bak")).unwrap();
    assert_eq!(schema::version(&backup).unwrap(), 21);
    assert!(backup.prepare("SELECT * FROM external_prompts").is_err());
    assert_eq!(
        backup
            .query_row("SELECT state FROM external_runs", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "running"
    );
    assert_eq!(s.recover_external_runs(10).unwrap(), 1);
    s.integrity_check().unwrap();
}
