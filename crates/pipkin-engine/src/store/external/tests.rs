use super::*;
use crate::store::compactions::CompactionScope;

mod permissions;
mod prompts;
mod review;
mod setup;

fn receipt_count(s: &Store) -> i64 {
    s.read(|c| {
        Ok(
            c.query_row("SELECT COUNT(*) FROM cli_binding_receipts", [], |r| {
                r.get(0)
            })?,
        )
    })
    .unwrap()
}

fn spec() -> CliBindingSpec {
    CliBindingSpec {
        cli_session_id: "12345678-1234-4234-8234-123456789abc".into(),
        project_path: "/project".into(),
        executable_path: "/usr/bin/claude".into(),
        cli_version: "1.2.3".into(),
        config_ref: "config-v1".into(),
        account_scope: "account-a".into(),
        launch_policy: "strict-v1".into(),
    }
}
fn key(op: &str) -> ExternalRunKey {
    ExternalRunKey {
        session: "cli".into(),
        generation: 1,
        op: op.into(),
    }
}
fn cli(s: &Store) {
    s.create_cli_session("cli", "create", &spec(), 1).unwrap();
}
fn running(s: &Store, key: &ExternalRunKey) {
    assert!(s.admit_external_run(key, 2).unwrap().1);
    s.transition_external_run(
        key,
        ExternalRunState::Admitted,
        ExternalRunState::LaunchIntent,
        3,
    )
    .unwrap();
    s.transition_external_run(
        key,
        ExternalRunState::LaunchIntent,
        ExternalRunState::Running,
        4,
    )
    .unwrap();
}
fn conflict<T: std::fmt::Debug>(result: Result<T>) {
    assert!(
        matches!(result, Err(StoreError::ExecutionConflict)),
        "{result:?}"
    );
}
fn drop_v21(c: &Connection) -> Result<()> {
    c.execute_batch(
        "DROP TABLE external_permissions; DROP TABLE external_projection_messages; DROP TABLE external_prompts;
        DROP TABLE external_actions; DROP TABLE external_runs; DROP TABLE cli_bindings;
        DROP TABLE cli_binding_receipts;
        DROP TRIGGER session_execution_immutable; DROP TRIGGER cli_session_shape_insert;
        DROP TRIGGER cli_session_shape_update; ALTER TABLE sessions DROP COLUMN execution_backend;
        DROP TABLE external_projection_actions; PRAGMA user_version=20;",
    )?;
    Ok(())
}

#[test]
fn fresh_identity_reopens_and_native_origin_is_not_backend() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    let native = s
        .create_session(
            "native",
            "/tmp",
            Some("native-key"),
            &Origin::Native,
            None,
            None,
            None,
            1,
        )
        .unwrap()
        .0;
    assert_eq!(native.execution_backend, ExecutionBackend::NativeAgent);
    assert!(s.cli_binding("native").unwrap().is_none());
    // Imported CLI history still defaults to native; origin never selects an executor.
    let imported = Origin::Imported {
        engine: "claude".into(),
        source: "legacy".into(),
    };
    s.create_session("imported", "/tmp", None, &imported, None, None, None, 1)
        .unwrap();
    cli(&s);
    let row = s.session("cli").unwrap().unwrap();
    assert_eq!(row.execution_backend, ExecutionBackend::LocalClaudeCli);
    assert!(row.blocked.is_none());
    assert!(row.summary().blocked.is_some());
    assert!(!row.subagents_enabled);
    assert!(row.model.is_none() && row.parent.is_none() && row.root.is_none());
    assert_eq!(
        s.cli_binding("cli").unwrap().unwrap(),
        CliBinding {
            identity: spec(),
            generation: 1,
            context_valid: true
        }
    );
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    assert_eq!(
        s.session("native").unwrap().unwrap().execution_backend,
        ExecutionBackend::NativeAgent
    );
    assert_eq!(s.cli_binding("cli").unwrap().unwrap().identity, spec());
    let row = s.session("imported").unwrap().unwrap();
    assert_eq!(row.execution_backend, ExecutionBackend::NativeAgent);
    assert_eq!(row.origin, imported);
    assert_eq!(s.root_sessions().unwrap().len(), 3);
    assert_eq!(s.all_sessions().unwrap().len(), 3);
}

#[test]
fn v20_is_backed_up_and_preserves_native_records() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    s.create_session(
        "native",
        "/tmp",
        None,
        &Origin::Native,
        Some("model"),
        None,
        None,
        1,
    )
    .unwrap();
    let imported = Origin::Imported {
        engine: "claude".into(),
        source: "legacy".into(),
    };
    s.create_session("imported", "/tmp", None, &imported, None, None, None, 1)
        .unwrap();
    s.tx(|c| drop_v21(c)).unwrap();
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    assert_eq!(s.read(schema::version).unwrap(), SCHEMA_VERSION);
    let row = s.session("native").unwrap().unwrap();
    assert_eq!(row.execution_backend, ExecutionBackend::NativeAgent);
    assert_eq!(row.model.as_deref(), Some("model"));
    let row = s.session("imported").unwrap().unwrap();
    assert_eq!(row.execution_backend, ExecutionBackend::NativeAgent);
    assert_eq!(row.origin, imported);
    let backup = Connection::open(dir.path().join("engine.sqlite.v20.bak")).unwrap();
    assert_eq!(schema::version(&backup).unwrap(), 20);
    assert_eq!(
        backup
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert!(
        backup
            .prepare("SELECT execution_backend FROM sessions")
            .is_err()
    );
}

#[test]
fn binding_and_keys_are_immutable_and_cross_backend_conflicts() {
    let s = Store::memory().unwrap();
    cli(&s);
    let first = s.session("cli").unwrap().unwrap();
    assert_eq!(
        s.create_cli_session("cli", "create", &spec(), 9).unwrap(),
        (first, false)
    );
    conflict(s.create_cli_session("different", "create", &spec(), 2));
    conflict(s.create_cli_session("cli", "different", &spec(), 2));
    let mut changed = spec();
    changed.account_scope = "other".into();
    conflict(s.create_cli_session("cli", "create", &changed, 2));
    conflict(s.create_cli_session("other", "other", &spec(), 2));
    conflict(s.create_session(
        "other",
        "/tmp",
        Some("create"),
        &Origin::Native,
        None,
        None,
        None,
        2,
    ));
    conflict(s.create_session("cli", "/tmp", None, &Origin::Native, None, None, None, 2));
    s.create_session(
        "native",
        "/tmp",
        Some("native-key"),
        &Origin::Native,
        None,
        None,
        None,
        1,
    )
    .unwrap();
    conflict(s.create_cli_session("native", "native-key", &spec(), 2));
    assert!(
        s.tx(|c| {
            c.execute(
                "UPDATE sessions SET execution_backend='nativeAgent' WHERE id='cli'",
                [],
            )?;
            Ok(())
        })
        .is_err()
    );
    assert!(
        s.tx(|c| {
            c.execute(
                "UPDATE cli_bindings SET cli_version='new' WHERE session='cli'",
                [],
            )?;
            Ok(())
        })
        .is_err()
    );
    assert!(
        s.tx(|c| {
            c.execute("UPDATE sessions SET subagents_enabled=1 WHERE id='cli'", [])?;
            Ok(())
        })
        .is_err()
    );
    assert_eq!(s.cli_binding("cli").unwrap().unwrap().identity, spec());
}

#[test]
fn validation_is_bounded_and_does_not_probe_disk_or_accept_payloads() {
    let s = Store::memory().unwrap();
    for field in 0..7 {
        let mut b = spec();
        match field {
            0 => b.cli_session_id = b.cli_session_id.to_uppercase(),
            1 => b.project_path = "/project/../other".into(),
            2 => b.executable_path = "relative/claude".into(),
            3 => b.config_ref = "/home/user/.credentials".into(),
            4 => b.account_scope = "{\"token\":\"secret\"}".into(),
            5 => b.launch_policy = "x".repeat(129),
            _ => b.cli_version = "v1\nsecret".into(),
        }
        conflict(s.create_cli_session("cli", "create", &b, 1));
    }
    assert!(s.all_sessions().unwrap().is_empty());
    cli(&s); // /project need not exist: storage never probes it.
}

#[test]
fn native_admissions_fail_closed_for_cli_and_unknown() {
    let s = Store::memory().unwrap();
    cli(&s);
    s.tx(|c| { c.execute("INSERT INTO sessions(id,cwd,created_at,updated_at,origin,execution_backend) VALUES('future','/tmp',1,1,'{\"kind\":\"native\"}','futureBackend')", [])?; Ok(()) }).unwrap();
    for id in ["cli", "future"] {
        assert!(matches!(
            s.read(|c| Store::assert_native_execution(c, id)),
            Err(StoreError::UnsupportedExecution)
        ));
        assert!(matches!(
            s.tx(|c| Store::start_run(c, id, None, 1)),
            Err(StoreError::UnsupportedExecution)
        ));
        assert!(matches!(
            s.tx(|c| Store::admit(c, id, "input", InputMode::FollowUp, "text", &[], None, 1)),
            Err(StoreError::UnsupportedExecution)
        ));
        assert!(matches!(
            s.tx(|c| Store::tool_intent(c, id, "call", None, "bash", &Value::Null, false, 1)),
            Err(StoreError::UnsupportedExecution)
        ));
        assert!(matches!(
            s.tx(|c| Store::admit_compaction(
                c,
                id,
                "compact",
                None,
                CompactionScope::Manual,
                None
            )),
            Err(StoreError::UnsupportedExecution)
        ));
        assert!(s.session(id).unwrap().unwrap().summary().blocked.is_some());
    }
    let error: ProtoError = StoreError::UnsupportedExecution.into();
    assert_eq!(error.code, ErrorCode::Unsupported);
}

#[test]
fn ordered_transitions_idempotency_and_completed_only_next_admission() {
    use ExternalRunState::*;
    let s = Store::memory().unwrap();
    cli(&s);
    let k = key("one");
    let (first, fresh) = s.admit_external_run(&k, 2).unwrap();
    assert!(fresh);
    assert_eq!(s.admit_external_run(&k, 3).unwrap(), (first, false));
    conflict(s.admit_external_run(&key("other"), 3));
    conflict(s.transition_external_run(&k, Admitted, Running, 3));
    conflict(s.transition_external_run(&k, Running, Completed, 3));
    s.transition_external_run(&k, Admitted, LaunchIntent, 3)
        .unwrap();
    conflict(s.transition_external_run(&k, Admitted, LaunchIntent, 4));
    s.transition_external_run(&k, LaunchIntent, Running, 4)
        .unwrap();
    conflict(s.transition_external_run(&key("wrong"), Running, Completed, 5));
    let mut stale = k.clone();
    stale.generation = 2;
    conflict(s.transition_external_run(&stale, Running, Completed, 5));
    conflict(s.admit_external_run(&stale, 5));
    s.transition_external_run(&k, Running, Completed, 5)
        .unwrap();
    assert!(!s.admit_external_run(&k, 6).unwrap().1);
    assert!(s.admit_external_run(&key("two"), 6).unwrap().1);
    conflict(s.transition_external_run(&k, Running, Completed, 7));
    assert!(s.cli_binding("cli").unwrap().unwrap().context_valid);
}

#[test]
fn interrupted_and_unknown_permanently_fence_binding() {
    use ExternalRunState::*;
    for terminal in [Interrupted, Unknown] {
        let s = Store::memory().unwrap();
        cli(&s);
        let k = key("one");
        running(&s, &k);
        s.transition_external_run(&k, Running, Stopping, 5).unwrap();
        assert!(s.cli_binding("cli").unwrap().unwrap().context_valid);
        conflict(s.admit_external_run(&key("while-stopping"), 6));
        conflict(s.transition_external_run(&k, Stopping, Completed, 6));
        s.transition_external_run(&k, Stopping, terminal, 6)
            .unwrap();
        let binding = s.cli_binding("cli").unwrap().unwrap();
        assert!(!binding.context_valid);
        assert_eq!(binding.generation, 2);
        conflict(s.admit_external_run(&key("two"), 7));
        let mut newer = key("two");
        newer.generation = 2;
        conflict(s.admit_external_run(&newer, 7));
        conflict(s.transition_external_run(&k, Running, Completed, 7));
        assert!(
            s.session("cli")
                .unwrap()
                .unwrap()
                .summary()
                .blocked
                .is_some()
        );
        assert!(
            s.tx(|c| {
                c.execute(
                    "UPDATE cli_bindings SET context_valid=1 WHERE session='cli'",
                    [],
                )?;
                Ok(())
            })
            .is_err()
        );
    }
}

#[test]
fn concurrent_writers_admit_only_one_active_operation() {
    let s = std::sync::Arc::new(Store::memory().unwrap());
    cli(&s);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let writers: Vec<_> = (0..8)
        .map(|i| {
            let s = s.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                s.admit_external_run(&key(&format!("op-{i}")), 2)
            })
        })
        .collect();
    let successes = writers
        .into_iter()
        .map(|w| usize::from(w.join().unwrap().is_ok()))
        .sum::<usize>();
    assert_eq!(successes, 1);
}

#[test]
fn action_metadata_is_separate_bounded_and_stale_guarded() {
    use ExternalActionStatus::*;
    let s = Store::memory().unwrap();
    cli(&s);
    let k = key("one");
    running(&s, &k);
    s.record_external_action(&k, "call", "Read", Observed, 5)
        .unwrap();
    s.record_external_action(&k, "call", "Read", Running, 6)
        .unwrap();
    conflict(s.record_external_action(&k, "call", "Write", Completed, 7));
    conflict(s.record_external_action(&key("other"), "call", "Read", Completed, 7));
    conflict(s.record_external_action(&k, &"x".repeat(129), "Read", Completed, 7));
    s.record_external_action(&k, "call", "Read", Completed, 7)
        .unwrap();
    let action = s.external_actions(&k).unwrap().pop().unwrap();
    assert_eq!(action.status, Completed);
    assert_eq!(action.provenance, ExecutionBackend::LocalClaudeCli);
    conflict(s.record_external_action(&k, "call", "Read", Running, 8));
    s.transition_external_run(
        &k,
        ExternalRunState::Running,
        ExternalRunState::Completed,
        8,
    )
    .unwrap();
    conflict(s.record_external_action(&k, "late", "Read", Observed, 9));
    assert!(s.tool_call("cli", "call").unwrap().is_none());
    assert!(s.unsettled_runs().unwrap().is_empty());
    assert_eq!(s.entry_count("cli").unwrap(), 0);
}

#[test]
fn recovery_is_atomic_never_replays_and_preserves_completed() {
    use ExternalRunState::*;
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    let states = [Admitted, LaunchIntent, Running, Stopping, Completed];
    let mut keys = vec![];
    for (i, state) in states.into_iter().enumerate() {
        let mut b = spec();
        b.cli_session_id = format!("12345678-1234-4234-8234-{i:012x}");
        let id = format!("cli-{i}");
        s.create_cli_session(&id, &id, &b, 1).unwrap();
        let k = ExternalRunKey {
            session: id,
            generation: 1,
            op: "op".into(),
        };
        s.admit_external_run(&k, 2).unwrap();
        if state != Admitted {
            s.transition_external_run(&k, Admitted, LaunchIntent, 3)
                .unwrap();
        }
        if !matches!(state, Admitted | LaunchIntent) {
            s.transition_external_run(&k, LaunchIntent, Running, 4)
                .unwrap();
        }
        if matches!(state, Stopping | Completed) {
            s.transition_external_run(&k, Running, state, 5).unwrap();
        }
        keys.push(k);
    }
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    let before = s.catch_up(0, 256).unwrap().head;
    s.fail_next_write();
    assert!(matches!(
        s.recover_external_runs(10),
        Err(StoreError::Injected)
    ));
    assert_eq!(s.catch_up(0, 256).unwrap().head, before);
    assert_eq!(s.external_run(&keys[0]).unwrap().unwrap().state, Admitted);
    assert_eq!(s.recover_external_runs(10).unwrap(), 4);
    assert_eq!(s.recover_external_runs(11).unwrap(), 0);
    for k in &keys[..4] {
        assert_eq!(s.external_run(k).unwrap().unwrap().state, Unknown);
        assert!(!s.cli_binding(&k.session).unwrap().unwrap().context_valid);
        assert!(s.queue(&k.session).unwrap().is_empty());
    }
    assert_eq!(s.external_run(&keys[4]).unwrap().unwrap().state, Completed);
    assert!(
        s.cli_binding(&keys[4].session)
            .unwrap()
            .unwrap()
            .context_valid
    );
    assert!(s.unsettled_runs().unwrap().is_empty());
    assert_eq!(
        s.read(
            |c| Ok(c.query_row("SELECT COUNT(*) FROM tool_calls", [], |r| r
                .get::<_, i64>(0))?)
        )
        .unwrap(),
        0
    );
}

#[test]
fn failures_roll_back_identity_ledger_revision_and_outbox_then_removal_keeps_receipt() {
    use ExternalRunState::*;
    let s = Store::memory().unwrap();
    s.fail_next_write();
    assert!(matches!(
        s.create_cli_session("cli", "create", &spec(), 1),
        Err(StoreError::Injected)
    ));
    assert!(s.cli_binding("cli").unwrap().is_none());
    assert!(s.session("cli").unwrap().is_none());
    assert_eq!(receipt_count(&s), 0);
    assert_eq!(s.catch_up(0, 256).unwrap().head, 0);
    cli(&s);
    let k = key("one");
    let before = s.session("cli").unwrap().unwrap().revision;
    let head = s.catch_up(0, 256).unwrap().head;
    s.fail_next_write();
    assert!(matches!(
        s.admit_external_run(&k, 2),
        Err(StoreError::Injected)
    ));
    assert!(s.external_run(&k).unwrap().is_none());
    assert_eq!(s.session("cli").unwrap().unwrap().revision, before);
    assert_eq!(s.catch_up(0, 256).unwrap().head, head);
    running(&s, &k);
    s.record_external_action(&k, "call", "Read", ExternalActionStatus::Running, 5)
        .unwrap();
    s.fail_next_write();
    assert!(matches!(
        s.transition_external_run(&k, Running, Unknown, 6),
        Err(StoreError::Injected)
    ));
    assert_eq!(s.external_run(&k).unwrap().unwrap().state, Running);
    assert!(s.cli_binding("cli").unwrap().unwrap().context_valid);
    s.begin_removal("cli").unwrap();
    assert!(matches!(
        s.record_external_action(&k, "call", "Read", ExternalActionStatus::Completed, 6),
        Err(StoreError::Removed)
    ));
    s.finish_removal("cli").unwrap();
    assert_eq!(receipt_count(&s), 1);
    assert!(s.external_run(&k).unwrap().is_none());
    assert!(s.external_actions(&k).unwrap().is_empty());
    assert!(s.cli_binding("cli").unwrap().is_none());
    assert!(matches!(
        s.create_cli_session("cli", "create", &spec(), 7),
        Err(StoreError::Removed)
    ));
    s.integrity_check().unwrap();
}
