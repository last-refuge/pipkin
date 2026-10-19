use super::*;

#[test]
fn model_creation_is_atomic_and_retryable_at_each_write_boundary() {
    for boundary in ["sessions", "cli_bindings", "cli_binding_receipts", "commit"] {
        let s = Store::memory().unwrap();
        let head = s.catch_up(0, 256).unwrap().head;
        if boundary == "commit" {
            s.fail_next_write();
        } else {
            s.read(|c| {
                c.execute_batch(&format!(
                    "CREATE TEMP TRIGGER setup_failure AFTER INSERT ON {boundary}
                     BEGIN SELECT RAISE(ABORT, 'setup failure'); END;"
                ))?;
                Ok(())
            })
            .unwrap();
        }
        assert!(
            s.create_cli_session_with_model("cli", "approval", &spec(), "sonnet", 1)
                .is_err(),
            "{boundary}"
        );
        assert!(s.session("cli").unwrap().is_none());
        assert!(s.cli_creation("cli", "approval").unwrap().is_none());
        assert!(s.cli_binding("cli").unwrap().is_none());
        assert_eq!(receipt_count(&s), 0);
        assert_eq!(s.catch_up(0, 256).unwrap().head, head);
        s.read(|c| {
            c.execute_batch("DROP TRIGGER IF EXISTS temp.setup_failure")?;
            Ok(())
        })
        .unwrap();
        let (row, created) = s
            .create_cli_session_with_model("cli", "approval", &spec(), "sonnet", 2)
            .unwrap();
        assert!(created);
        assert_eq!(row.model.as_deref(), Some("sonnet"));
        assert_eq!(row.cwd, spec().project_path);
        assert_eq!(row.execution_backend, ExecutionBackend::LocalClaudeCli);
        assert!(row.parent.is_none() && row.root.is_none() && row.thinking.is_none());
        assert!(!row.subagents_enabled);
        assert_eq!(receipt_count(&s), 1);
        s.integrity_check().unwrap();
    }
}

#[test]
fn model_aliases_reopen_exactly_and_conflicting_payloads_never_rebind() {
    for alias in ["haiku", "sonnet", "opus"] {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        let (first, created) = s
            .create_cli_session_with_model("cli", "approval", &spec(), alias, 1)
            .unwrap();
        assert!(created);
        drop(s);
        let s = Store::open(dir.path()).unwrap();
        let head = s.catch_up(0, 256).unwrap().head;
        assert_eq!(
            s.cli_creation("cli", "approval").unwrap(),
            Some(first.clone())
        );
        assert_eq!(
            s.create_cli_session_with_model("cli", "approval", &spec(), alias, i64::MAX)
                .unwrap(),
            (first.clone(), false)
        );
        for other in ["haiku", "sonnet", "opus"] {
            if other != alias {
                conflict(s.create_cli_session_with_model("cli", "approval", &spec(), other, 2));
            }
        }
        conflict(s.create_cli_session("cli", "approval", &spec(), 2));
        for field in 0..7 {
            let mut changed = spec();
            match field {
                0 => changed.cli_session_id = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee".into(),
                1 => changed.project_path = "/other-project".into(),
                2 => changed.executable_path = "/other/claude".into(),
                3 => changed.cli_version = "2.0".into(),
                4 => changed.config_ref = "other-config".into(),
                5 => changed.account_scope = "other-account".into(),
                _ => changed.launch_policy = "other-policy".into(),
            }
            conflict(s.create_cli_session_with_model("cli", "approval", &changed, alias, 2));
        }
        assert_eq!(s.session("cli").unwrap(), Some(first));
        assert_eq!(s.cli_binding("cli").unwrap().unwrap().identity, spec());
        assert_eq!(receipt_count(&s), 1);
        assert_eq!(s.catch_up(0, 256).unwrap().head, head);
        assert_eq!(s.read(schema::version).unwrap(), schema::SCHEMA_VERSION);
    }
}

#[test]
fn exact_model_creation_remains_readable_after_context_fencing() {
    let s = Store::memory().unwrap();
    s.create_cli_session_with_model("cli", "approval", &spec(), "sonnet", 1)
        .unwrap();
    running(&s, &key("one"));
    assert_eq!(s.recover_external_runs(5).unwrap(), 1);
    assert!(!s.cli_binding("cli").unwrap().unwrap().context_valid);
    let row = s.session("cli").unwrap().unwrap();
    assert_eq!(
        s.cli_creation("cli", "approval").unwrap(),
        Some(row.clone())
    );
    assert_eq!(
        s.create_cli_session_with_model("cli", "approval", &spec(), "sonnet", 6)
            .unwrap(),
        (row, false)
    );
    assert_eq!(receipt_count(&s), 1);
    conflict(s.admit_external_run(&key("two"), 7));
}

#[test]
fn invalid_aliases_and_legacy_model_upgrades_are_rejected() {
    let s = Store::memory().unwrap();
    for alias in [
        "",
        "Sonnet",
        "sonnet ",
        "claude-sonnet-4-5",
        "native/model",
        "opus\n",
    ] {
        conflict(s.create_cli_session_with_model("cli", "approval", &spec(), alias, 1));
    }
    assert!(s.all_sessions().unwrap().is_empty());
    assert_eq!(receipt_count(&s), 0);
    cli(&s);
    conflict(s.create_cli_session_with_model("cli", "create", &spec(), "sonnet", 2));
    assert!(
        s.cli_creation("cli", "create")
            .unwrap()
            .unwrap()
            .model
            .is_none()
    );
    assert!(!s.create_cli_session("cli", "create", &spec(), 2).unwrap().1);
}

#[test]
fn creation_lookup_is_read_only_and_conflicts_on_id_key_or_backend() {
    let s = Store::memory().unwrap();
    let first = s
        .create_cli_session_with_model("cli", "approval", &spec(), "opus", 1)
        .unwrap()
        .0;
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
    let head = s.catch_up(0, 256).unwrap().head;
    s.inject(Fault::ReadOnly).unwrap();
    assert_eq!(
        s.cli_creation("cli", "approval").unwrap(),
        Some(first.clone())
    );
    assert!(s.cli_creation("new", "new-key").unwrap().is_none());
    for (id, key) in [
        ("other", "approval"),
        ("cli", "other-key"),
        ("native", "native-key"),
        ("native", "new-key"),
        ("new", "native-key"),
        ("", "approval"),
        ("cli", ""),
    ] {
        conflict(s.cli_creation(id, key));
    }
    s.clear_faults().unwrap();
    for (id, key) in [
        ("native", "native-key"),
        ("native", "new-key"),
        ("new", "native-key"),
        ("other", "approval"),
        ("cli", "other-key"),
    ] {
        conflict(s.create_cli_session_with_model(id, key, &spec(), "opus", 2));
    }
    // Reads do not even consume the pending write fault.
    s.fail_next_write();
    assert_eq!(
        s.cli_creation("cli", "approval").unwrap(),
        Some(first.clone())
    );
    assert!(matches!(
        s.set_session_field("cli", SessionField::Title(Some("lost".into()))),
        Err(StoreError::Injected)
    ));
    assert_eq!(s.session("cli").unwrap(), Some(first));
    assert_eq!(s.catch_up(0, 256).unwrap().head, head);
}

#[test]
fn model_creation_tombstones_and_uuid_receipts_survive_removal_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    s.create_cli_session_with_model("cli", "approval", &spec(), "haiku", 1)
        .unwrap();
    s.begin_removal("cli").unwrap();
    for (id, key) in [("cli", "approval"), ("cli", "new-key"), ("new", "approval")] {
        assert!(matches!(s.cli_creation(id, key), Err(StoreError::Removed)));
        assert!(matches!(
            s.create_cli_session_with_model(id, key, &spec(), "haiku", 2),
            Err(StoreError::Removed)
        ));
    }
    s.finish_removal("cli").unwrap();
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    let head = s.catch_up(0, 256).unwrap().head;
    for (id, key) in [("cli", "approval"), ("cli", "new-key"), ("new", "approval")] {
        assert!(matches!(s.cli_creation(id, key), Err(StoreError::Removed)));
        assert!(matches!(
            s.create_cli_session_with_model(id, key, &spec(), "haiku", 3),
            Err(StoreError::Removed)
        ));
    }
    assert!(s.cli_creation("new", "new-key").unwrap().is_none());
    conflict(s.create_cli_session_with_model("new", "new-key", &spec(), "haiku", 3));
    assert!(s.session("new").unwrap().is_none());
    assert_eq!(receipt_count(&s), 1);
    assert_eq!(s.catch_up(0, 256).unwrap().head, head);
    let mut fresh = spec();
    fresh.cli_session_id = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee".into();
    assert!(
        s.create_cli_session_with_model("new", "new-key", &fresh, "haiku", 4)
            .unwrap()
            .1
    );
    assert_eq!(receipt_count(&s), 2);
}

#[test]
fn cli_model_thinking_and_subagent_mutations_fail_but_titles_and_native_work() {
    for alias in [None, Some("sonnet")] {
        let s = Store::memory().unwrap();
        if let Some(alias) = alias {
            s.create_cli_session_with_model("cli", "create", &spec(), alias, 1)
                .unwrap();
        } else {
            cli(&s);
        }
        s.create_session("native", "/tmp", None, &Origin::Native, None, None, None, 1)
            .unwrap();
        let first = s.session("cli").unwrap();
        let head = s.catch_up(0, 256).unwrap().head;
        for field in [
            SessionField::Model(None),
            SessionField::Model(Some("sonnet".into())),
            SessionField::Thinking(None),
            SessionField::Thinking(Some("high".into())),
            SessionField::ModelAndThinking {
                model: "opus".into(),
                thinking: "high".into(),
            },
            SessionField::SubagentsEnabled(false),
            SessionField::SubagentsEnabled(true),
        ] {
            assert!(matches!(
                s.set_session_field("cli", field),
                Err(StoreError::UnsupportedExecution)
            ));
        }
        assert_eq!(s.session("cli").unwrap(), first);
        assert_eq!(s.catch_up(0, 256).unwrap().head, head);
        for model in [None, Some("haiku"), Some("invalid")] {
            if model == alias {
                continue;
            }
            assert!(
                s.tx(|c| {
                    c.execute("UPDATE sessions SET model=?1 WHERE id='cli'", [model])?;
                    Ok(())
                })
                .is_err()
            );
        }
        s.set_session_field("cli", SessionField::Title(Some("My CLI".into())))
            .unwrap();
        assert_eq!(
            s.session("cli").unwrap().unwrap().title.as_deref(),
            Some("My CLI")
        );
        assert_eq!(s.session("cli").unwrap().unwrap().model.as_deref(), alias);
        for field in [
            SessionField::Model(Some("native/model".into())),
            SessionField::Thinking(Some("high".into())),
            SessionField::ModelAndThinking {
                model: "another/model".into(),
                thinking: "low".into(),
            },
            SessionField::SubagentsEnabled(false),
            SessionField::SubagentsEnabled(true),
        ] {
            s.set_session_field("native", field).unwrap();
        }
        let native = s.session("native").unwrap().unwrap();
        assert_eq!(native.model.as_deref(), Some("another/model"));
        assert_eq!(native.thinking.as_deref(), Some("low"));
        assert!(native.subagents_enabled);
    }
}

#[test]
fn schema22_model_guards_migrate_with_backup_and_no_table_changes() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    cli(&s);
    let tables = |c: &Connection| -> Result<Vec<String>> {
        Ok(
            c.prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")?
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        )
    };
    let before = s.read(tables).unwrap();
    // Reinstall the original v21/v22 guards to exercise an existing on-disk store.
    let original = include_str!("../../external.sql");
    let start = original
        .find("CREATE TRIGGER cli_session_shape_insert")
        .unwrap();
    let end = original.find("-- Permanent, nonsecret UUID").unwrap();
    s.tx(|c| {
        crate::store::cli_agent::remove_schema_for_test(c)?;
        c.execute_batch(
            "DROP TABLE cli_bridge_calls; DROP TABLE cli_tool_bridges; DROP TABLE external_permissions; DROP TRIGGER cli_session_shape_insert; DROP TRIGGER cli_session_shape_update; DROP TABLE external_projection_actions; PRAGMA user_version=22;",
        )?;
        c.execute_batch(&original[start..end])?;
        Ok(())
    })
    .unwrap();
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    assert_eq!(s.read(schema::version).unwrap(), schema::SCHEMA_VERSION);
    let backup = Connection::open(dir.path().join("engine.sqlite.v22.bak")).unwrap();
    assert_eq!(schema::version(&backup).unwrap(), 22);
    assert_eq!(s.read(tables).unwrap(), before);
    assert!(
        s.cli_creation("cli", "create")
            .unwrap()
            .unwrap()
            .model
            .is_none()
    );
    let mut fresh = spec();
    fresh.cli_session_id = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee".into();
    s.create_cli_session_with_model("new", "new-key", &fresh, "opus", 2)
        .unwrap();
    s.integrity_check().unwrap();
}
