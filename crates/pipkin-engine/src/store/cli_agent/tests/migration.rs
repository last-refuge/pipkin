use super::*;
fn snapshot(c: &Connection) -> Result<Vec<Vec<Vec<rusqlite::types::Value>>>> {
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
fn v26_backup_preserves_unknown_receipts_and_reserves_removed_legacy_uuids() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    let live = legacy(create("live", "/project").identity);
    s.create_cli_session_with_model("live", "live-create", &live, "sonnet", 1)
        .unwrap();
    let key = ExternalRunKey {
        session: "live".into(),
        generation: 1,
        op: "op".into(),
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
    let mut removed = live.clone();
    removed.cli_session_id = "22222222-2222-4222-8222-222222222222".into();
    s.create_cli_session_with_model("removed", "removed-create", &removed, "opus", 1)
        .unwrap();
    s.begin_removal("removed").unwrap();
    s.finish_removal("removed").unwrap();
    let before = s.read(snapshot).unwrap();
    s.tx(|tx| {
        remove_schema_for_test(tx)?;
        crate::store::schema::remove_v33_for_test(tx)?;
        tx.execute_batch("PRAGMA user_version=26;")?;
        Ok(())
    })
    .unwrap();
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    assert_eq!(s.read(schema::version).unwrap(), schema::SCHEMA_VERSION);
    assert_eq!(s.read(snapshot).unwrap(), before);
    let backup = Connection::open(dir.path().join("engine.sqlite.v26.bak")).unwrap();
    assert_eq!(schema::version(&backup).unwrap(), 26);
    assert_eq!(snapshot(&backup).unwrap(), before);
    assert_eq!(counts(&s), (1, 0, 2));
    for old in [live, removed] {
        let mut agent = create("agent", "/project");
        agent.identity.cli_session_id = old.cli_session_id;
        assert!(s.create_cli_agent_session(&agent, 5).is_err());
    }
    s.integrity_check().unwrap();
}
#[test]
fn acknowledged_binding_survives_reopen_and_reading_it_never_claims_a_bridge() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    let spec = create("agent", "/project");
    let original = s.create_cli_agent_session(&spec, 1).unwrap().0;
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    assert_eq!(
        s.create_cli_agent_session(&spec, 2).unwrap(),
        (original, false)
    );
    assert_eq!(
        s.read(|c| Ok(
            c.query_row("SELECT COUNT(*) FROM cli_tool_bridges", [], |r| r
                .get::<_, i64>(0))?
        ))
        .unwrap(),
        0
    );
    assert_eq!(
        s.read(|c| Ok(c.query_row("SELECT COUNT(*) FROM runs", [], |r| r.get::<_, i64>(0))?))
            .unwrap(),
        0
    );
}
