use super::*;
use serde_json::json;
mod goal_usage;
mod launch_context;
mod wake;
fn setup(s: &Store) -> cli_agent::Binding {
    s.create_cli_agent_session(&cli_agent::tests::create("s", "/project"), 1)
        .unwrap()
        .0
}
fn input<'a>(b: &cli_agent::Binding, key: &'a str) -> Input<'a> {
    Input {
        images: &[],
        mode: pipkin_engine_protocol::InputMode::Prompt,
        session: "s",
        request: key,
        generation: 1,
        operation: key,
        binding: b.digest,
        text: "private root instruction",
    }
}
fn admit(s: &Store, b: &cli_agent::Binding, key: &str) -> Lease {
    s.admit_cli_agent_turn(&input(b, key), "owner", 2)
        .unwrap()
        .lease
        .unwrap()
}
fn bridge(s: &Store, l: &Lease) -> cli_bridge::BridgeSpec {
    let spec = cli_bridge::BridgeSpec {
        id: format!("b-{}", l.receipt.scope.run),
        scope: l.receipt.scope.clone(),
        owner: "owner".into(),
        binding_hash: [3; 32],
        capability_hash: [4; 32],
    };
    assert!(s.prepare_cli_tool_bridge(&spec, 3).unwrap());
    spec
}
fn evidence() -> Completion {
    Completion {
        cli_session_id: "11111111-1111-4111-8111-111111111111".into(),
        terminal: ExternalTerminal {
            metadata: None,
            outcome: ExternalTerminalOutcome::Success,
            usage: None,
        },
        supervisor: ExternalSettlementEvidence {
            projection_finished: true,
            input_delivered: true,
            root_reaped: true,
            pipes_drained: true,
            exit_code: Some(0),
            ..Default::default()
        },
    }
}
fn launch(s: &Store, l: &Lease) -> cli_bridge::Lease {
    let spec = bridge(s, l);
    s.begin_cli_agent_launch(l, &spec.id, 4).unwrap();
    s.mark_cli_agent_running(l, 5).unwrap();
    s.claim_cli_tool_bridge(&spec, 6).unwrap().unwrap()
}
fn counts(s: &Store) -> (i64, i64, i64, i64) {
    s.read(|c|Ok(c.query_row("SELECT (SELECT COUNT(*) FROM requests),(SELECT COUNT(*) FROM runs),(SELECT COUNT(*) FROM entries),(SELECT COUNT(*) FROM cli_agent_turns)",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?)).unwrap()
}
#[test]
fn atomic_admission_retains_user_role_and_duplicate_never_reissues_owner() {
    let s = Store::memory().unwrap();
    let b = setup(&s);
    let l = admit(&s, &b, "r");
    assert_eq!(l.receipt.mode, TurnMode::Fresh);
    assert_eq!(counts(&s), (1, 1, 1, 1));
    let repeated = s
        .admit_cli_agent_turn(&input(&b, "r"), "new-owner", 3)
        .unwrap();
    assert!(repeated.lease.is_none());
    assert_eq!(repeated.receipt, l.receipt);
    assert_eq!(counts(&s), (1, 1, 1, 1));
    let entry = s.page("s", None, 10).unwrap().0.remove(0);
    assert!(matches!(
        entry.kind,
        EntryKind::Message {
            message: Message::User { .. },
            ..
        }
    ));
    assert!(
        s.admit_cli_agent_turn(&input(&b, "second"), "owner", 4)
            .is_err()
    );
    assert!(s.tx(|c| Store::start_run(c, "s", None, 4)).is_err());
}
#[test]
fn admission_and_launch_commit_faults_do_not_mint_authority() {
    let s = Store::memory().unwrap();
    let b = setup(&s);
    s.fail_next_write();
    assert!(s.admit_cli_agent_turn(&input(&b, "r"), "owner", 2).is_err());
    assert_eq!(counts(&s), (0, 0, 0, 0));
    let l = admit(&s, &b, "r");
    let spec = bridge(&s, &l);
    s.fail_next_write();
    assert!(s.begin_cli_agent_launch(&l, &spec.id, 4).is_err());
    assert_eq!(
        s.cli_agent_turn("s", "r").unwrap().unwrap().state,
        State::Admitted
    );
    s.begin_cli_agent_launch(&l, &spec.id, 5).unwrap();
    assert!(s.begin_cli_agent_launch(&l, &spec.id, 6).is_err());
    s.mark_cli_agent_running(&l, 7).unwrap();
    assert!(s.mark_cli_agent_running(&l, 8).is_err());
}
#[test]
fn changed_fingerprint_binding_owner_and_scope_cannot_rebind_or_launch() {
    let s = Store::memory().unwrap();
    let b = setup(&s);
    let l = admit(&s, &b, "r");
    let spec = bridge(&s, &l);
    for kind in 0..4 {
        let mut i = input(&b, "r");
        match kind {
            0 => i.text = "different",
            1 => i.generation = 2,
            2 => i.operation = "different",
            _ => i.binding = [9; 32],
        };
        assert!(s.admit_cli_agent_turn(&i, "owner", 4).is_err());
    }
    let forged = Lease {
        receipt: l.receipt.clone(),
        owner: "foreign".into(),
        binding: b.digest,
    };
    assert!(s.begin_cli_agent_launch(&forged, &spec.id, 4).is_err());
    assert!(s.begin_cli_agent_launch(&l, "missing", 4).is_err());
}
#[test]
fn stop_before_launch_retires_instruction_without_poisoning_unused_uuid() {
    let s = Store::memory().unwrap();
    let b = setup(&s);
    let l = admit(&s, &b, "r");
    let spec = bridge(&s, &l);
    s.request_stop("s", 4).unwrap();
    assert!(s.begin_cli_agent_launch(&l, &spec.id, 5).is_err());
    s.abandon_cli_agent_turn(&l, 6).unwrap();
    assert_eq!(
        s.cli_agent_turn("s", "r").unwrap().unwrap().state,
        State::Rejected
    );
    assert!(s.page("s", None, 10).unwrap().0[0].retired);
    let next = admit(&s, &b, "next");
    assert_eq!(next.receipt.mode, TurnMode::Fresh);
}
#[test]
fn launch_uncertainty_is_permanent_and_cannot_be_promoted_by_late_success() {
    let s = Store::memory().unwrap();
    let b = setup(&s);
    let l = admit(&s, &b, "r");
    let channel = launch(&s, &l);
    s.request_stop("s", 7).unwrap();
    assert!(s.complete_cli_agent_turn(&l, &evidence(), 8).is_err());
    s.abandon_cli_agent_turn(&l, 9).unwrap();
    assert_eq!(
        s.cli_agent_turn("s", "r").unwrap().unwrap().state,
        State::Unknown
    );
    assert!(s.cli_native_context(&channel).is_err());
    assert!(s.complete_cli_agent_turn(&l, &evidence(), 10).is_err());
    assert!(
        s.admit_cli_agent_turn(&input(&b, "next"), "owner", 10)
            .is_err()
    );
    assert!(
        s.admit_cli_agent_turn(&input(&b, "r"), "owner", 10)
            .unwrap()
            .lease
            .is_none()
    );
}
#[test]
fn success_needs_independent_facts_clean_bridge_and_is_idempotent_not_replayable() {
    let s = Store::memory().unwrap();
    let b = setup(&s);
    let l = admit(&s, &b, "r");
    let channel = launch(&s, &l);
    assert!(s.complete_cli_agent_turn(&l, &evidence(), 7).is_err());
    assert!(s.close_cli_tool_bridge(&channel, 8).unwrap());
    for kind in 0..10 {
        let mut e = evidence();
        match kind {
            0 => e.supervisor.input_delivered = false,
            1 => e.supervisor.root_reaped = false,
            2 => e.supervisor.pipes_drained = false,
            3 => e.supervisor.projection_finished = false,
            4 => e.supervisor.exit_code = Some(1),
            5 => e.supervisor.stop_requested = true,
            6 => e.supervisor.cleanup_timed_out = true,
            7 => e.terminal.outcome = ExternalTerminalOutcome::Error,
            8 => e.supervisor.cancelled_clean_confirmed = true,
            _ => e.cli_session_id = "foreign".into(),
        };
        assert!(s.complete_cli_agent_turn(&l, &e, 9).is_err());
    }
    s.fail_next_write();
    assert!(s.complete_cli_agent_turn(&l, &evidence(), 9).is_err());
    assert_eq!(
        s.cli_agent_turn("s", "r").unwrap().unwrap().state,
        State::Running
    );
    assert_eq!(
        s.complete_cli_agent_turn(&l, &evidence(), 10).unwrap(),
        Settlement::Committed
    );
    assert_eq!(
        s.complete_cli_agent_turn(&l, &evidence(), 11).unwrap(),
        Settlement::AlreadyCommitted
    );
    let next = admit(&s, &b, "next");
    assert_eq!(next.receipt.mode, TurnMode::Resume);
}
#[test]
fn conflicting_completion_preserves_evidence_but_fences_context_and_active_successor() {
    let s = Store::memory().unwrap();
    let b = setup(&s);
    let l = admit(&s, &b, "r");
    let channel = launch(&s, &l);
    s.close_cli_tool_bridge(&channel, 7).unwrap();
    s.complete_cli_agent_turn(&l, &evidence(), 8).unwrap();
    let next = admit(&s, &b, "next");
    let next_channel = launch(&s, &next);
    let mut changed = evidence();
    changed.supervisor.exit_code = Some(1);
    assert_eq!(
        s.complete_cli_agent_turn(&l, &changed, 10).unwrap(),
        Settlement::Conflict
    );
    let receipt = s.cli_agent_turn("s", "r").unwrap().unwrap();
    assert_eq!(receipt.state, State::Completed);
    assert!(receipt.fenced);
    let saved: String = s
        .read(|c| {
            Ok(c.query_row(
                "SELECT evidence FROM cli_agent_turns WHERE request_key='r'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(saved, json(&evidence()));
    assert_eq!(
        s.cli_agent_turn("s", "next").unwrap().unwrap().state,
        State::Unknown
    );
    assert!(s.cli_native_context(&next_channel).is_err());
    assert!(
        s.admit_cli_agent_turn(&input(&b, "third"), "owner", 11)
            .is_err()
    );
    assert!(
        s.tx(|c| {
            c.execute_batch("UPDATE cli_agent_turns SET fenced=0")?;
            Ok(())
        })
        .is_err()
    );
}
#[test]
fn protocol_only_receipts_cannot_establish_native_root_completion() {
    let s = Store::memory().unwrap();
    let b = setup(&s);
    let l = admit(&s, &b, "r");
    let channel = launch(&s, &l);
    let cli_bridge::Admission::New(call) = s
        .admit_cli_bridge_call(&channel, &json!(1), "write", &json!({}), 7)
        .unwrap()
    else {
        panic!()
    };
    s.settle_cli_bridge_call(&channel, &call, &json!({"content":[]}), 8)
        .unwrap();
    s.close_cli_tool_bridge(&channel, 9).unwrap();
    assert!(s.complete_cli_agent_turn(&l, &evidence(), 10).is_err());
}
#[test]
fn journaled_root_cannot_dispatch_before_launch_or_complete_with_unknown_native_effect() {
    let s = Store::memory().unwrap();
    let b = setup(&s);
    let l = admit(&s, &b, "r");
    let spec = bridge(&s, &l);
    let early = s.claim_cli_tool_bridge(&spec, 4).unwrap().unwrap();
    assert!(s.cli_native_context(&early).is_err());
    assert!(
        s.admit_cli_native_call(&early, &json!(1), "write", &json!({}), 4)
            .is_err()
    );
    assert!(s.begin_cli_agent_launch(&l, &spec.id, 5).is_err());
    s.abandon_cli_agent_turn(&l, 6).unwrap();
    let l = admit(&s, &b, "next");
    let channel = launch(&s, &l);
    assert!(matches!(
        s.admit_cli_native_call(&channel, &json!(1), "write", &json!({}), 7)
            .unwrap(),
        cli_bridge::Admission::New(_)
    ));
    assert!(!s.close_cli_tool_bridge(&channel, 8).unwrap());
    assert!(s.complete_cli_agent_turn(&l, &evidence(), 9).is_err());
    s.abandon_cli_agent_turn(&l, 10).unwrap();
    assert_eq!(
        s.cli_agent_turn("s", "next").unwrap().unwrap().state,
        State::Unknown
    );
}
#[test]
fn removal_retains_root_receipt_but_deletes_prompt_payload_and_revokes_owner() {
    let s = Store::memory().unwrap();
    let b = setup(&s);
    let l = admit(&s, &b, "r");
    let _channel = launch(&s, &l);
    s.begin_removal("s").unwrap();
    assert_eq!(
        s.cli_agent_turn("s", "r").unwrap().unwrap().state,
        State::Unknown
    );
    s.finish_removal("s").unwrap();
    assert_eq!(counts(&s), (0, 0, 0, 1));
    s.abandon_cli_agent_turn(&l, 10).unwrap();
    assert!(
        s.admit_cli_agent_turn(&input(&b, "r"), "owner", 11)
            .unwrap()
            .lease
            .is_none()
    );
    assert!(
        s.admit_cli_agent_turn(&input(&b, "new"), "owner", 11)
            .is_err()
    );
}
#[test]
fn sql_cannot_rebind_delete_or_reopen_root_receipts() {
    let s = Store::memory().unwrap();
    let b = setup(&s);
    let l = admit(&s, &b, "r");
    s.abandon_cli_agent_turn(&l, 3).unwrap();
    for sql in [
        "DELETE FROM cli_agent_turns",
        "UPDATE cli_agent_turns SET state='admitted'",
        "UPDATE cli_agent_turns SET owner='other'",
        "UPDATE cli_agent_turns SET generation=2",
        "UPDATE cli_agent_turns SET user_entry=123",
    ] {
        assert!(
            s.tx(|c| {
                c.execute_batch(sql)?;
                Ok(())
            })
            .is_err(),
            "{sql}"
        );
    }
}
#[test]
fn concurrent_duplicate_admission_has_exactly_one_owner() {
    let s = std::sync::Arc::new(Store::memory().unwrap());
    let b = setup(&s);
    let threads = (0..16)
        .map(|_| {
            let s = s.clone();
            let b = b.clone();
            std::thread::spawn(move || {
                s.admit_cli_agent_turn(&input(&b, "r"), "owner", 2)
                    .unwrap()
                    .lease
                    .is_some()
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        threads
            .into_iter()
            .map(|t| usize::from(t.join().unwrap()))
            .sum::<usize>(),
        1
    );
    assert_eq!(counts(&s), (1, 1, 1, 1));
}
#[test]
fn prior_unjournaled_history_cannot_be_adopted_as_fresh_context() {
    let s = Store::memory().unwrap();
    let b = setup(&s);
    s.tx(|c| {
        c.execute(
            "INSERT INTO runs(session,state,started_at,ended_at) VALUES('s','settled',1,2)",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(s.admit_cli_agent_turn(&input(&b, "r"), "owner", 3).is_err());
}
#[test]
fn v27_backup_preserves_binding_and_migration_does_not_infer_context_history() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    let b = setup(&s);
    s.tx(|c| {
        remove_schema_for_test(c)?;
        c.execute_batch("PRAGMA user_version=27;")?;
        Ok(())
    })
    .unwrap();
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    assert_eq!(s.read(schema::version).unwrap(), schema::SCHEMA_VERSION);
    assert_eq!(s.cli_agent_binding("s").unwrap().unwrap(), b);
    let backup = Connection::open(dir.path().join("engine.sqlite.v27.bak")).unwrap();
    assert_eq!(schema::version(&backup).unwrap(), 27);
    assert_eq!(s.recover_cli_agent_turns(2).unwrap(), 0);
    assert_eq!(admit(&s, &b, "r").receipt.mode, TurnMode::Fresh);
}
mod durability;

#[test]
fn cli_input_batch_admission_rolls_back_all_members_on_invalid_ticket_and_retires_together() {
    use inputs::{Kind, Queued};
    let s = Store::memory().unwrap();
    let b = setup(&s);
    let make = |key: &str| Queued {
        images: vec![],
        session: "s".into(),
        request: key.into(),
        text: format!("queued {key}"),
        kind: Kind::FollowUp,
        generation: 1,
        operation: key.into(),
        ticket: format!("ticket-{key}"),
    };
    let a = make("a");
    let member = make("member");
    s.queue_cli_input(&b, &a, 2).unwrap();
    s.queue_cli_input(&b, &member, 2).unwrap();
    let i = Input {
        images: &[],
        mode: InputMode::FollowUp,
        session: "s",
        request: &a.request,
        generation: a.generation,
        operation: &a.operation,
        binding: b.digest,
        text: &a.text,
    };
    let before = counts(&s);
    let mut bad = member.clone();
    bad.ticket = "wrong-ticket".into();
    assert!(
        s.admit_cli_agent_turn_with_batch(&i, "owner", None, None, None, Some(&a), &[bad], 3)
            .is_err()
    );
    assert_eq!(counts(&s), before);
    assert_eq!(s.queue("s").unwrap().len(), 2);
    s.fail_next_write();
    assert!(
        s.admit_cli_agent_turn_with_batch(
            &i,
            "owner",
            None,
            None,
            None,
            Some(&a),
            std::slice::from_ref(&member),
            3
        )
        .is_err()
    );
    assert_eq!(counts(&s), before);
    let admission = s
        .admit_cli_agent_turn_with_batch(
            &i,
            "owner",
            None,
            None,
            None,
            Some(&a),
            std::slice::from_ref(&member),
            3,
        )
        .unwrap();
    let lease = admission.lease.unwrap();
    let context = s.cli_agent_launch_context(&lease).unwrap();
    assert!(context.prompt.contains("queued a"));
    assert!(context.prompt.contains("queued member"));
    let before = counts(&s);
    assert!(
        s.admit_cli_agent_turn_with_batch(&i, "other-owner", None, None, None, Some(&a), &[], 4)
            .unwrap()
            .lease
            .is_none()
    );
    assert_eq!(counts(&s), before);
    assert!(
        s.tx(|c| {
            c.execute(
                "UPDATE cli_agent_input_deliveries SET text='replacement'",
                [],
            )?;
            Ok(())
        })
        .is_err()
    );
    s.abandon_cli_agent_turn(&lease, 4).unwrap();
    assert_eq!(
        s.request("s", "member").unwrap().unwrap().state,
        RequestState::Settled
    );
    assert_eq!(
        s.request("s", "member").unwrap().unwrap().outcome,
        Some(RunOutcome::Aborted)
    );
    assert!(s.page("s", None, 10).unwrap().0.iter().all(|e| e.retired));
    assert!(s.queue("s").unwrap().is_empty());
}

#[test]
fn features_image_identity_duplicates_tampering_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.db");
    let s = Store::open(&path).unwrap();
    let b = setup(&s);
    let images = vec![ImageInput {
        mime_type: "image/png".into(),
        data: "aW1hZ2U=".into(),
    }];
    let mut i = input(&b, "image");
    i.text = "";
    i.images = &images;
    let l = s
        .admit_cli_agent_turn(&i, "owner", 2)
        .unwrap()
        .lease
        .unwrap();
    assert!(
        s.admit_cli_agent_turn(&i, "owner", 3)
            .unwrap()
            .lease
            .is_none()
    );
    assert_eq!(s.cli_agent_launch_context(&l).unwrap().images(), images);
    assert!(
        s.lookup_cli_agent_turn(&Input { images: &[], ..i })
            .is_err()
    );
    let changed = vec![ImageInput {
        data: "Y2hhbmdlZA==".into(),
        ..images[0].clone()
    }];
    assert!(
        s.lookup_cli_agent_turn(&Input {
            images: &changed,
            ..i
        })
        .is_err()
    );
    drop(s);
    let s = Store::open(&path).unwrap();
    assert_eq!(s.cli_agent_launch_context(&l).unwrap().images(), images);
    s.tx(|c| {
        c.execute(
            "UPDATE requests SET images=?1 WHERE session='s'",
            [json(&changed)],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(s.cli_agent_launch_context(&l).is_err());
}

#[test]
fn features_malformed_queued_image_data_is_not_treated_as_empty() {
    for raw in ["[{}]", "null", "{}", "[null]"] {
        let s = Store::memory().unwrap();
        let b = setup(&s);
        let q = inputs::Queued {
            session: "s".into(),
            request: "q".into(),
            text: "queued message".into(),
            images: vec![],
            kind: inputs::Kind::FollowUp,
            generation: 1,
            operation: "q".into(),
            ticket: "ticket-q".into(),
        };
        s.queue_cli_input(&b, &q, 2).unwrap();
        s.tx(|c| {
            c.execute(
                "UPDATE requests SET images=?1 WHERE session='s' AND key='q'",
                [raw],
            )?;
            Ok(())
        })
        .unwrap();
        assert!(s.queue_cli_input(&b, &q, 3).is_err());
        let i = Input {
            session: "s",
            request: "q",
            mode: InputMode::FollowUp,
            generation: 1,
            operation: "q",
            text: &q.text,
            images: &[],
            binding: b.digest,
        };
        assert!(
            s.admit_cli_agent_turn_with_input(&i, "owner", None, None, None, Some(&q), 4)
                .is_err()
        );
        assert_eq!(counts(&s), (1, 0, 0, 0));
    }
}
