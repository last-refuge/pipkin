use super::*;

#[test]
fn connection_grant_is_only_returned_after_commit() {
    let s = Store::memory().unwrap();
    let spec = fixture(&s, "s", ORCHESTRATION_BACKEND);
    s.fail_next_write();
    assert!(matches!(
        s.prepare_cli_tool_bridge(&spec, 2),
        Err(StoreError::Injected)
    ));
    assert!(s.read(|c| stored(c, &spec.id)).unwrap().is_none());
    s.prepare_cli_tool_bridge(&spec, 3).unwrap();
    s.fail_next_write();
    assert!(matches!(
        s.claim_cli_tool_bridge(&spec, 4),
        Err(StoreError::Injected)
    ));
    assert_eq!(state(&s, &spec), "prepared");
    let _lost_ack = s.claim_cli_tool_bridge(&spec, 5).unwrap().unwrap();
    assert!(s.claim_cli_tool_bridge(&spec, 6).unwrap().is_none());
}

fn legacy_rows(c: &Connection) -> Result<Vec<Vec<Vec<rusqlite::types::Value>>>> {
    [
        "sessions",
        "cli_bindings",
        "cli_binding_receipts",
        "external_runs",
    ]
    .iter()
    .map(|table| {
        let mut q = c.prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))?;
        Ok(q.query_map([], |r| {
            (0..r.as_ref().column_count())
                .map(|i| r.get(i))
                .collect::<rusqlite::Result<Vec<_>>>()
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
    })
    .collect()
}
#[test]
fn migration_from_v25_preserves_native_cli_and_unknown_identity_receipts() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    s.create_session(
        "native",
        "/project",
        Some("native-create"),
        &Origin::default(),
        Some("native-model"),
        None,
        None,
        1,
    )
    .unwrap();
    let binding = CliBindingSpec {
        cli_session_id: "12345678-1234-4234-8234-123456789abc".into(),
        project_path: "/project".into(),
        executable_path: "/usr/bin/claude".into(),
        cli_version: "2.1.301".into(),
        config_ref: "config".into(),
        account_scope: "login".into(),
        launch_policy: "cli-unattended-coding-v1".into(),
    };
    s.create_cli_session_with_model("cli", "cli-create", &binding, "sonnet", 1)
        .unwrap();
    let key = ExternalRunKey {
        session: "cli".into(),
        generation: 1,
        op: "unknown".into(),
    };
    s.admit_external_run(&key, 2).unwrap();
    s.transition_external_run(
        &key,
        ExternalRunState::Admitted,
        ExternalRunState::LaunchIntent,
        3,
    )
    .unwrap();
    s.recover_external_runs(4).unwrap();
    let before = s.read(legacy_rows).unwrap();
    s.tx(|tx| {
        crate::store::cli_agent::remove_schema_for_test(tx)?;
        tx.execute_batch(
            "DROP TABLE cli_bridge_calls; DROP TABLE cli_tool_bridges; PRAGMA user_version=25;",
        )?;
        Ok(())
    })
    .unwrap();
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    assert_eq!(s.read(schema::version).unwrap(), SCHEMA_VERSION);
    assert_eq!(s.read(legacy_rows).unwrap(), before);
    let backup = Connection::open(dir.path().join("engine.sqlite.v25.bak")).unwrap();
    assert_eq!(schema::version(&backup).unwrap(), 25);
    assert_eq!(legacy_rows(&backup).unwrap(), before);
    assert_eq!(s.recover_cli_tool_bridges(5).unwrap(), 0);
    assert_eq!(s.read(legacy_rows).unwrap(), before);
    s.integrity_check().unwrap();
}

// Only the parent test below sets these private variables. This child never runs a CLI,
// model or native tool: the one owned-file write is a deliberately simulated effect.
#[test]
fn crash_cut_child() {
    let Ok(dir) = std::env::var("PIPKIN_TEST_BRIDGE_CRASH_DIR") else {
        return;
    };
    assert_eq!(
        std::env::var("PIPKIN_TEST_BRIDGE_CRASH_MARKER").unwrap(),
        "ledger-fixture-v1"
    );
    let cut = std::env::var("PIPKIN_TEST_BRIDGE_CRASH_CUT").unwrap();
    let dir = std::path::Path::new(&dir);
    let s = Store::open(dir).unwrap();
    let spec = fixture(&s, "s", ORCHESTRATION_BACKEND);
    s.prepare_cli_tool_bridge(&spec, 2).unwrap();
    if cut != "prepared" {
        let l = s.claim_cli_tool_bridge(&spec, 3).unwrap().unwrap();
        if cut != "connected" {
            let c = admit(&s, &l, json!(1));
            if cut != "intent" {
                std::fs::write(dir.join("simulated-effect"), b"exactly once").unwrap();
                if cut == "completed" {
                    s.settle_cli_bridge_call(&l, &c, &json!({"content":[]}), 5)
                        .unwrap();
                }
            }
        }
    }
    // Exit bypasses Rust drops/SQLite close, exercising committed WAL recovery.
    std::process::exit(0);
}
#[test]
fn process_exit_cuts_never_restore_dispatch_authority_or_erase_known_results() {
    for cut in ["prepared", "connected", "intent", "effect", "completed"] {
        let dir = tempfile::tempdir().unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "store::cli_bridge::tests::durability::crash_cut_child",
                "--nocapture",
            ])
            .env("PIPKIN_TEST_BRIDGE_CRASH_DIR", dir.path())
            .env("PIPKIN_TEST_BRIDGE_CRASH_MARKER", "ledger-fixture-v1")
            .env("PIPKIN_TEST_BRIDGE_CRASH_CUT", cut)
            .status()
            .unwrap();
        assert!(status.success(), "{cut}");
        let s = Store::open(dir.path()).unwrap();
        let spec = s.read(|c| Ok(stored(c, "bridge-s")?.unwrap().0)).unwrap();
        assert_eq!(s.recover_cli_tool_bridges(10).unwrap(), 1);
        assert_eq!(state(&s, &spec), "fenced");
        assert!(s.claim_cli_tool_bridge(&spec, 11).unwrap().is_none());
        assert!(!s.prepare_cli_tool_bridge(&spec, 11).unwrap());
        let calls = s
            .read(|c| {
                Ok(c.prepare("SELECT state FROM cli_bridge_calls")?
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?)
            })
            .unwrap();
        assert_eq!(
            calls,
            match cut {
                "prepared" | "connected" => vec![],
                "completed" => vec!["completed"],
                _ => vec!["unknown"],
            }
        );
        if matches!(cut, "effect" | "completed") {
            assert_eq!(
                std::fs::read(dir.path().join("simulated-effect")).unwrap(),
                b"exactly once"
            );
        } else {
            assert!(!dir.path().join("simulated-effect").exists());
        }
        assert_eq!(s.recover_cli_tool_bridges(12).unwrap(), 0);
        s.integrity_check().unwrap();
    }
}
