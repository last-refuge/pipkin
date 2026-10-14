use super::*;

#[test]
fn removal_reopen_never_reuses_uuid_even_completed_or_never_launched() {
    use ExternalRunState::*;
    for terminal in [Some(Unknown), Some(Interrupted), Some(Completed), None] {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        cli(&s);
        if let Some(terminal) = terminal {
            let k = key("one");
            running(&s, &k);
            s.transition_external_run(&k, Running, terminal, 5).unwrap();
        }
        // The exact live creation request remains idempotent, even after context fencing.
        assert!(!s.create_cli_session("cli", "create", &spec(), 6).unwrap().1);
        assert_eq!(receipt_count(&s), 1);
        s.begin_removal("cli").unwrap();
        s.finish_removal("cli").unwrap();
        drop(s);

        let s = Store::open(dir.path()).unwrap();
        assert!(s.cli_binding("cli").unwrap().is_none());
        assert_eq!(receipt_count(&s), 1);
        let head = s.catch_up(0, 256).unwrap().head;
        conflict(s.create_cli_session("replacement", "replacement-key", &spec(), 7));
        assert!(s.session("replacement").unwrap().is_none());
        assert_eq!(s.catch_up(0, 256).unwrap().head, head);
        assert_eq!(receipt_count(&s), 1);
        let owner: String = s
            .read(|c| {
                Ok(c.query_row(
                    "SELECT session FROM cli_binding_receipts WHERE cli_session_id=?1",
                    [&spec().cli_session_id],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(owner, "cli");

        // A genuinely new context is allowed; no launch occurs in this test.
        let mut fresh = spec();
        fresh.cli_session_id = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee".into();
        assert!(
            s.create_cli_session("replacement", "replacement-key", &fresh, 8)
                .unwrap()
                .1
        );
        let k = ExternalRunKey {
            session: "replacement".into(),
            generation: 1,
            op: "fresh".into(),
        };
        assert!(s.admit_external_run(&k, 9).unwrap().1);
        assert_eq!(receipt_count(&s), 2);
        s.integrity_check().unwrap();
    }
}

#[test]
fn receipts_are_minimal_immutable_and_binding_insert_is_atomic() {
    let s = Store::memory().unwrap();
    cli(&s);
    let columns = s
        .read(|c| {
            Ok(c.prepare("PRAGMA table_info(cli_binding_receipts)")?
                .query_map([], |r| r.get::<_, String>(1))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .unwrap();
    assert_eq!(columns, ["cli_session_id", "session"]);
    for sql in [
        "UPDATE cli_binding_receipts SET session='other'",
        "UPDATE cli_binding_receipts SET cli_session_id='aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee'",
        "DELETE FROM cli_binding_receipts",
        "INSERT OR REPLACE INTO cli_binding_receipts SELECT * FROM cli_binding_receipts",
        "INSERT INTO cli_binding_receipts VALUES('aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee','unbound')",
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
    // Even direct binding insertion cannot bypass canonical UUID shape or reuse receipts.
    s.begin_removal("cli").unwrap();
    s.finish_removal("cli").unwrap();
    for uuid in [
        spec().cli_session_id,
        "AAAAAAAA-BBBB-4CCC-8DDD-EEEEEEEEEEEE".into(),
        "aaaaaaaaabbbb-4ccc-8ddd-eeeeeeeeeee".into(),
        "not-a-uuid".into(),
    ] {
        assert!(s.tx(|c| {
            c.execute("INSERT INTO sessions(id,cwd,created_at,updated_at,origin,subagents_enabled,create_key,execution_backend)
                VALUES('replacement','/project',1,1,'{\"kind\":\"native\"}',0,'new','localClaudeCli')", [])?;
            c.execute("INSERT INTO cli_bindings(session,cli_session_id,project_path,executable_path,cli_version,config_ref,account_scope,launch_policy)
                VALUES('replacement',?1,'/project','/usr/bin/claude','1','config','account','strict')", [&uuid])?;
            Ok(())
        }).is_err(), "{uuid}");
        assert!(s.session("replacement").unwrap().is_none());
        assert_eq!(receipt_count(&s), 1);
    }
    s.integrity_check().unwrap();
}

#[test]
fn completion_waits_for_settled_metadata_and_failures_preserve_all_state() {
    use ExternalActionStatus::*;
    for (pending, settled) in [(Observed, Completed), (Running, Failed)] {
        let s = Store::memory().unwrap();
        cli(&s);
        let k = key("one");
        running(&s, &k);
        s.record_external_action(&k, "call", "Read", pending, 5)
            .unwrap();
        let run = s.external_run(&k).unwrap();
        let session = s.session("cli").unwrap();
        let binding = s.cli_binding("cli").unwrap();
        let actions = s.external_actions(&k).unwrap();
        let head = s.catch_up(0, 256).unwrap().head;
        conflict(s.transition_external_run(
            &k,
            ExternalRunState::Running,
            ExternalRunState::Completed,
            6,
        ));
        // The database guard agrees with the API and cannot strand pending metadata either.
        assert!(
            s.tx(|c| {
                c.execute(
                    "UPDATE external_runs SET state='completed' WHERE session='cli'",
                    [],
                )?;
                Ok(())
            })
            .is_err()
        );
        assert_eq!(s.external_run(&k).unwrap(), run);
        assert_eq!(s.session("cli").unwrap(), session);
        assert_eq!(s.cli_binding("cli").unwrap(), binding);
        assert_eq!(s.external_actions(&k).unwrap(), actions);
        assert_eq!(s.catch_up(0, 256).unwrap().head, head);
        conflict(s.admit_external_run(&key("two"), 6));

        s.record_external_action(&k, "call", "Read", settled, 7)
            .unwrap();
        let session = s.session("cli").unwrap();
        let head = s.catch_up(0, 256).unwrap().head;
        s.fail_next_write();
        assert!(matches!(
            s.transition_external_run(
                &k,
                ExternalRunState::Running,
                ExternalRunState::Completed,
                8
            ),
            Err(StoreError::Injected)
        ));
        assert_eq!(s.external_run(&k).unwrap(), run);
        assert_eq!(s.session("cli").unwrap(), session);
        assert_eq!(s.cli_binding("cli").unwrap(), binding);
        assert_eq!(s.catch_up(0, 256).unwrap().head, head);
        assert_eq!(s.external_actions(&k).unwrap()[0].status, settled);
        s.transition_external_run(
            &k,
            ExternalRunState::Running,
            ExternalRunState::Completed,
            9,
        )
        .unwrap();
        assert!(s.admit_external_run(&key("two"), 10).unwrap().1);
    }
}

#[test]
fn unknown_action_blocks_completion_until_run_is_fenced() {
    let s = Store::memory().unwrap();
    cli(&s);
    let k = key("one");
    running(&s, &k);
    s.record_external_action(&k, "call", "Write", ExternalActionStatus::Running, 5)
        .unwrap();
    s.record_external_action(&k, "call", "Write", ExternalActionStatus::Unknown, 6)
        .unwrap();
    let run = s.external_run(&k).unwrap();
    let session = s.session("cli").unwrap();
    let head = s.catch_up(0, 256).unwrap().head;
    conflict(s.transition_external_run(
        &k,
        ExternalRunState::Running,
        ExternalRunState::Completed,
        7,
    ));
    assert_eq!(s.external_run(&k).unwrap(), run);
    assert_eq!(s.session("cli").unwrap(), session);
    assert_eq!(s.catch_up(0, 256).unwrap().head, head);
    assert!(s.cli_binding("cli").unwrap().unwrap().context_valid);
    conflict(s.admit_external_run(&key("two"), 8));
    conflict(s.record_external_action(&k, "call", "Write", ExternalActionStatus::Completed, 8));
    s.transition_external_run(&k, ExternalRunState::Running, ExternalRunState::Unknown, 9)
        .unwrap();
    assert!(!s.cli_binding("cli").unwrap().unwrap().context_valid);
    assert_eq!(
        s.external_actions(&k).unwrap()[0].status,
        ExternalActionStatus::Unknown
    );
}
