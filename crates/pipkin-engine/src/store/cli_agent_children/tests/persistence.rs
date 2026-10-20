use super::*;
#[test]
fn immutable_policy_is_removed_with_binding_but_uuid_reservation_survives() {
    let f = Fixture::new();
    let call = f.call(1, "subagent");
    let p = f.policy(&call, fresh()).unwrap();
    f.create(&p, "child", UUID).unwrap();
    for sql in [
        "UPDATE cli_agent_child_policies SET snapshot=snapshot WHERE child='child'",
        "DELETE FROM cli_agent_child_policies WHERE child='child'",
    ] {
        assert!(
            f.s.tx(|tx| {
                tx.execute(sql, [])?;
                Ok(())
            })
            .is_err()
        );
    }
    assert!(
        f.s.read(|tx| Store::cli_native_child_policy_in(tx, "child"))
            .unwrap()
            .is_some()
    );
    f.s.begin_removal("parent").unwrap();
    f.s.finish_removal("parent").unwrap();
    assert_eq!(f.counts(), (0, 2, 0));
    assert!(
        f.s.read(|tx| Store::cli_native_child_policy_in(tx, "child"))
            .unwrap()
            .is_none()
    );
    let mut replacement = cli_agent::tests::create("replacement", "/project");
    replacement.identity.cli_session_id = UUID.into();
    assert!(f.s.create_cli_agent_session(&replacement, 20).is_err());
}
#[test]
fn snapshot_corruption_is_rejected_not_reinterpreted_as_a_policy() {
    let f = Fixture::new();
    let call = f.call(1, "subagent");
    let p = f.policy(&call, fresh()).unwrap();
    f.create(&p, "child", UUID).unwrap();
    f.s.tx(|tx| {tx.execute_batch("DROP TRIGGER cli_agent_child_policy_immutable; UPDATE cli_agent_child_policies SET snapshot='{}';")?;Ok(())}).unwrap();
    assert!(
        f.s.read(|tx| Store::cli_native_child_policy_in(tx, "child"))
            .is_err()
    );
    assert!(f.create(&p, "another", UUID).is_err());
    assert_eq!(f.counts(), (2, 2, 1));
    assert!(f.s.open_run("child").unwrap().is_none());
}
#[test]
fn oversized_corrupt_snapshot_is_refused_before_loading_its_body() {
    let f = Fixture::new();
    let call = f.call(1, "subagent");
    let p = f.policy(&call, fresh()).unwrap();
    f.create(&p, "child", UUID).unwrap();
    f.s.tx(|tx| {
        tx.execute_batch(
            "DROP TRIGGER cli_agent_child_policy_immutable; PRAGMA ignore_check_constraints=ON;",
        )?;
        tx.execute(
            "UPDATE cli_agent_child_policies SET snapshot=?1",
            ["x".repeat(2 * 1024 * 1024 + 1)],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(
        f.s.read(|tx| Store::cli_native_child_policy_in(tx, "child"))
            .is_err()
    );
}
#[test]
fn detached_child_binding_does_not_become_a_task_or_launch_grant() {
    let f = Fixture::new();
    let call = f.call(1, "subagent");
    let p = f.policy(&call, fresh()).unwrap();
    let mut create = f.b.create.clone();
    create.session = "child".into();
    create.request_key = p.create_key();
    create.parent = Some("parent".into());
    create.model = "haiku".into();
    create.identity.cli_session_id = UUID.into();
    f.s.create_cli_agent_session(&create, 8).unwrap();
    assert!(f.create(&p, "child", UUID).is_err());
    assert_eq!(f.counts(), (2, 2, 0));
    assert!(
        f.s.read(|tx| Store::cli_native_child_policy_in(tx, "child"))
            .unwrap()
            .is_none()
    );
    assert!(f.s.open_run("child").unwrap().is_none());
}
#[test]
fn v28_migration_preserves_binding_and_root_receipts_without_child_authority() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    let b = s
        .create_cli_agent_session(&cli_agent::tests::create("parent", "/project"), 1)
        .unwrap()
        .0;
    let admitted = s
        .admit_cli_agent_turn(
            &cli_agent_turns::Input {
                images: &[],
                mode: pipkin_engine_protocol::InputMode::Prompt,
                session: "parent",
                request: "request",
                generation: 1,
                operation: "op",
                binding: b.digest,
                text: "original input",
            },
            "owner",
            2,
        )
        .unwrap()
        .lease
        .unwrap();
    let receipt = admitted.receipt().clone();
    s.tx(|tx| {
        remove_schema_for_test(tx)?;
        crate::store::schema::remove_v33_for_test(tx)?;
        tx.execute_batch("PRAGMA user_version=28")?;
        Ok(())
    })
    .unwrap();
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    assert_eq!(s.read(schema::version).unwrap(), schema::SCHEMA_VERSION);
    assert_eq!(s.cli_agent_binding("parent").unwrap().unwrap(), b);
    assert_eq!(
        s.cli_agent_turn("parent", "request").unwrap().unwrap(),
        receipt
    );
    let count: i64 = s
        .read(|tx| {
            Ok(
                tx.query_row("SELECT COUNT(*) FROM cli_agent_child_policies", [], |r| {
                    r.get(0)
                })?,
            )
        })
        .unwrap();
    assert_eq!(count, 0);
    let backup = Connection::open(dir.path().join("engine.sqlite.v28.bak")).unwrap();
    assert_eq!(schema::version(&backup).unwrap(), 28);
    let count: i64 = backup
        .query_row("SELECT COUNT(*) FROM cli_agent_turns", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn desktop_controls_v31_migration_preserves_identity_and_backs_up_before_person_child_shape() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    let binding = s
        .create_cli_agent_session(&cli_agent::tests::create("parent", "/project"), 1)
        .unwrap()
        .0;
    let old = include_str!("../../cli_agent_children.sql");
    let trigger = &old[old
        .find("CREATE TRIGGER cli_agent_child_policy_shape")
        .unwrap()
        ..old
            .find("CREATE TRIGGER cli_agent_child_policy_immutable")
            .unwrap()];
    s.tx(|c| {
        c.execute_batch("DROP TRIGGER cli_agent_child_policy_shape;")?;
        c.execute_batch(trigger)?;
        crate::store::schema::remove_v33_for_test(c)?;
        c.execute_batch("PRAGMA user_version=31")?;
        Ok(())
    })
    .unwrap();
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    assert_eq!(s.read(schema::version).unwrap(), schema::SCHEMA_VERSION);
    assert_eq!(s.cli_agent_binding("parent").unwrap().unwrap(), binding);
    let backup = Connection::open(dir.path().join("engine.sqlite.v31.bak")).unwrap();
    assert_eq!(schema::version(&backup).unwrap(), 31);
    assert!(s.tasks("parent", 10).unwrap().0.is_empty());
    assert!(s.open_run("parent").unwrap().is_none());
}
