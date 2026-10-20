use super::*;
use crate::execution::claude_cli::{coding_policy, events, process};
use crate::store::{ExternalPromptMode, ExternalSettlementEvidence};

fn old_stop(s: &Store) -> ExternalRunKey {
    let mut binding = spec();
    binding.launch_policy = coding_policy::ID.into();
    s.create_cli_session_with_model("cli", "create", &binding, "sonnet", 1)
        .unwrap();
    let key = key("stopped");
    s.tx(|tx| Store::admit_external_prompt_in(tx, "cli", "old", "old instruction", &key.op, 1, 2))
        .unwrap();
    s.tx(|tx| Store::begin_external_launch_in(tx, &key, 3))
        .unwrap();
    s.tx(|tx| Store::begin_external_run_in(tx, &key, 4))
        .unwrap();
    s.tx(|tx| Store::project_external_event_in(tx, &key, 1, &events::Event::Initialized, 5))
        .unwrap();
    s.tx(|tx| {
        Store::project_external_event_in(
            tx,
            &key,
            2,
            &events::Event::ExternalToolObserved {
                message_id: "message".into(),
                block_index: 0,
                tool_id: "unfinished".into(),
                name: "Write".into(),
            },
            5,
        )
    })
    .unwrap();
    s.tx(|tx| Store::stop_external_prompt_in(tx, &key, 6))
        .unwrap();
    let e = ExternalSettlementEvidence {
        input_delivered: true,
        root_reaped: true,
        pipes_drained: true,
        stop_requested: true,
        cancelled_clean_confirmed: true,
        supervisor_stop: Some(process::Stop::Cancelled),
        projection_error: Some(events::Error::Incomplete),
        rejected_record: Some(events::RecordClass::EndOfStream),
        ..Default::default()
    };
    s.tx(|tx| Store::settle_external_prompt_in(tx, &key, &e, 7))
        .unwrap();
    // Reproduce the exact old persisted shape only in this disposable fixture.
    s.tx(|tx| { tx.execute("UPDATE external_prompts SET evidence=json_remove(evidence,'$.continuation_authorized') WHERE session='cli'", [])?; Ok(()) }).unwrap();
    key
}
fn recover(s: &Store) -> CliBinding {
    let expected = s.cli_binding("cli").unwrap().unwrap();
    s.tx(|tx| Store::recover_cli_clean_stop_in(tx, "cli", &expected, "fresh-verified-setup", 10))
        .unwrap()
}
fn audit_count(s: &Store) -> i64 {
    s.read(|c| Ok(c.query_row("SELECT COUNT(*) FROM cli_stop_recoveries", [], |r| r.get(0))?))
        .unwrap()
}

#[test]
fn old_clean_stop_recovery_preserves_receipt_unknown_actions_and_resume_identity_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    let key = old_stop(&s);
    let old = s.external_prompt("cli", "old").unwrap().unwrap();
    let actions = s.external_actions(&key).unwrap();
    let identity = s.cli_binding("cli").unwrap().unwrap().identity;
    assert_eq!(actions[0].status, ExternalActionStatus::Unknown);
    assert!(s.cli_clean_stop_recovery("cli").unwrap().is_some());
    let binding = recover(&s);
    assert!(binding.context_valid);
    assert_eq!(binding.generation, 3);
    assert_eq!(binding.identity, identity);
    assert_eq!(audit_count(&s), 1);
    assert_eq!(s.external_prompt("cli", "old").unwrap().unwrap(), old);
    assert_eq!(s.external_actions(&key).unwrap(), actions);
    assert!(
        s.tx(|tx| Store::recover_cli_clean_stop_in(tx, "cli", &binding, "again", 11))
            .is_err()
    );
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    assert_eq!(s.recover_external_runs(12).unwrap(), 0);
    let duplicate = s
        .tx(|tx| {
            Store::admit_external_prompt_in(tx, "cli", "old", "old instruction", "duplicate", 3, 13)
        })
        .unwrap()
        .0;
    assert!(!duplicate.newly_admitted);
    assert_eq!(duplicate.receipt, old);
    let next = s
        .tx(|tx| {
            Store::admit_external_prompt_in(
                tx,
                "cli",
                "new",
                "inspect existing work",
                "new-op",
                3,
                14,
            )
        })
        .unwrap()
        .0;
    assert!(next.newly_admitted);
    assert_eq!(next.receipt.mode, ExternalPromptMode::Resume);
    assert_eq!(s.external_actions(&key).unwrap(), actions);
    s.integrity_check().unwrap();
}

#[test]
fn old_clean_stop_recovery_refuses_missing_conflicting_or_newer_negative_evidence() {
    for sql in [
        "UPDATE external_prompts SET cursor=0",
        "UPDATE external_prompts SET evidence=NULL",
        "UPDATE external_prompts SET evidence=json_set(evidence,'$.continuation_authorized',json('false'))",
        "UPDATE external_prompts SET evidence=json_set(evidence,'$.root_reaped',json('false'))",
        "UPDATE external_prompts SET evidence=json_set(evidence,'$.pipes_drained',json('false'))",
        "UPDATE external_prompts SET evidence=json_set(evidence,'$.input_delivered',json('false'))",
        "UPDATE external_prompts SET evidence=json_set(evidence,'$.cleanup_timed_out',json('true'))",
        "UPDATE external_prompts SET evidence=json_set(evidence,'$.supervisor_stop','timed_out')",
        "UPDATE external_prompts SET evidence=json_set(evidence,'$.projection_error','Sequence')",
        "UPDATE external_prompts SET evidence=json_set(evidence,'$.cancelled_clean_confirmed',json('false'))",
        "UPDATE external_prompts SET outcome='{}'",
        "UPDATE external_runs SET state='unknown'",
        "UPDATE external_actions SET status='running'",
        "INSERT INTO external_runs VALUES('cli',1,'older-raw','unknown',0,0)",
    ] {
        let s = Store::memory().unwrap();
        old_stop(&s);
        s.tx(|tx| {
            tx.execute_batch(sql)?;
            Ok(())
        })
        .unwrap();
        let binding = s.cli_binding("cli").unwrap().unwrap();
        assert!(
            !s.cli_clean_stop_recovery("cli")
                .is_ok_and(|receipt| receipt.is_some()),
            "{sql}"
        );
        assert!(
            s.tx(|tx| Store::recover_cli_clean_stop_in(tx, "cli", &binding, "new-check", 10))
                .is_err(),
            "{sql}"
        );
        assert_eq!(s.cli_binding("cli").unwrap().unwrap(), binding);
        assert_eq!(audit_count(&s), 0);
    }
}

#[test]
fn old_clean_stop_recovery_is_atomic_cas_guarded_and_cannot_be_an_unrecorded_unfence() {
    let s = Store::memory().unwrap();
    old_stop(&s);
    let binding = s.cli_binding("cli").unwrap().unwrap();
    assert!(
        s.tx(|tx| {
            tx.execute(
                "UPDATE cli_bindings SET context_valid=1,generation=3 WHERE session='cli'",
                [],
            )?;
            Ok(())
        })
        .is_err()
    );
    let mut wrong = binding.clone();
    wrong.identity.account_scope = "different".into();
    assert!(
        s.tx(|tx| Store::recover_cli_clean_stop_in(tx, "cli", &wrong, "check", 10))
            .is_err()
    );
    assert!(
        s.tx(|tx| {
            Store::recover_cli_clean_stop_in(tx, "cli", &binding, "check", 10)?;
            Err::<(), _>(StoreError::Injected)
        })
        .is_err()
    );
    assert_eq!(audit_count(&s), 0);
    assert_eq!(s.cli_binding("cli").unwrap().unwrap(), binding);
    recover(&s);
    assert!(
        s.tx(|tx| {
            tx.execute("UPDATE cli_stop_recoveries SET setup='rewritten'", [])?;
            Ok(())
        })
        .is_err()
    );
    s.begin_removal("cli").unwrap();
    s.finish_removal("cli").unwrap();
    assert_eq!(audit_count(&s), 0);
    s.integrity_check().unwrap();
}

#[test]
fn v33_upgrade_backs_up_and_preserves_locked_receipts_without_automatic_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    old_stop(&s);
    let before = s.external_prompt("cli", "old").unwrap().unwrap();
    s.tx(|tx| {
        schema::remove_v34_for_test(tx)?;
        tx.execute_batch("PRAGMA user_version=33")?;
        Ok(())
    })
    .unwrap();
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    assert_eq!(s.external_prompt("cli", "old").unwrap().unwrap(), before);
    assert!(!s.cli_binding("cli").unwrap().unwrap().context_valid);
    assert_eq!(audit_count(&s), 0);
    let backup = Connection::open(dir.path().join("engine.sqlite.v33.bak")).unwrap();
    assert_eq!(schema::version(&backup).unwrap(), 33);
    recover(&s);
    s.integrity_check().unwrap();
}
