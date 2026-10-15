use super::*;

fn receipts(s: &Store, base: &ExternalPermissionRequest) -> [ExternalPermissionReceipt; 3] {
    std::array::from_fn(|i| {
        let mut r = base.clone();
        r.key.invocation_nonce = format!("removal-{i}");
        request(s, &r);
        if i > 0 {
            decide(s, &r, ExternalPermissionDecision::AllowOnce);
        }
        if i == 2 {
            assert!(consume(s, &r).newly_consumed);
        }
        read(s, &r)
    })
}

fn assert_unchanged(s: &Store, before: &[ExternalPermissionReceipt; 3]) {
    for receipt in before {
        assert_eq!(read(s, &receipt.request), *receipt);
    }
}

fn assert_retired(s: &Store, before: &[ExternalPermissionReceipt; 3]) {
    let requested_at: i64 = s
        .read(|c| {
            Ok(c.query_row(
                "SELECT requested_at FROM session_removals WHERE root='cli'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    for (i, receipt) in before.iter().enumerate() {
        let mut expected = receipt.clone();
        if i < 2 {
            expected.state = ExternalPermissionState::Revoked;
            expected.updated_at_ms = expected.updated_at_ms.max(requested_at);
        }
        assert_eq!(read(s, &receipt.request), expected);
    }
    assert_eq!(s.removal_state("cli").unwrap(), Some(false));
    assert!(matches!(
        s.read(|c| Store::assert_session_live(c, "cli")),
        Err(StoreError::Removed)
    ));
}

#[test]
fn removal_intent_revokes_unspent_permissions_not_consumed_or_unrelated_receipts() {
    let s = Store::memory().unwrap();
    let r = fixture(&s, EXTERNAL_PERMISSION_POLICY);
    let before = receipts(&s, &r);

    let mut binding = spec();
    binding.launch_policy = EXTERNAL_PERMISSION_POLICY.into();
    binding.cli_session_id = "87654321-1234-4234-8234-123456789abc".into();
    s.create_cli_session("other", "other-create", &binding, 1)
        .unwrap();
    let prompt = s
        .tx(|tx| Store::admit_external_prompt_in(tx, "other", "request", "question", "op", 1, 2))
        .unwrap()
        .0
        .receipt;
    s.tx(|tx| Store::begin_external_launch_in(tx, &prompt.key, 3))
        .unwrap();
    s.tx(|tx| Store::begin_external_run_in(tx, &prompt.key, 4))
        .unwrap();
    let mut other = r.clone();
    other.key.run = prompt.key;
    other.key.prompt = prompt.id;
    let unrelated = receipts(&s, &other);

    assert_eq!(s.begin_removal("cli").unwrap(), vec!["cli"]);
    assert_retired(&s, &before);
    assert_unchanged(&s, &unrelated);
    // Revocation does not claim the external run or any already-authorized effect stopped.
    assert_eq!(
        s.external_run(&r.key.run).unwrap().unwrap().state,
        ExternalRunState::Running
    );
    for receipt in &before[..2] {
        conflict(s.tx(|tx| Store::consume_external_permission_in(tx, &receipt.request, 8)));
    }
    assert!(!consume(&s, &before[2].request).newly_consumed);

    // A duplicate must preserve the same revocation receipt, timestamp and decisions.
    assert_eq!(s.begin_removal("cli").unwrap(), vec!["cli"]);
    assert_retired(&s, &before);
    assert_unchanged(&s, &unrelated);
    s.integrity_check().unwrap();
}

#[test]
fn removal_intent_and_permission_revocation_roll_back_together() {
    let s = Store::memory().unwrap();
    let r = fixture(&s, EXTERNAL_PERMISSION_POLICY);
    let before = receipts(&s, &r);
    s.fail_next_write();
    assert!(matches!(s.begin_removal("cli"), Err(StoreError::Injected)));
    assert_eq!(s.removal_state("cli").unwrap(), None);
    assert_unchanged(&s, &before);
    s.read(|c| Store::assert_session_live(c, "cli")).unwrap();

    // Fail within the revocation statement, after the intent has been inserted.
    s.tx(|tx| {
        tx.execute_batch(
            "CREATE TRIGGER fail_removal_revocation BEFORE UPDATE OF state ON external_permissions
             WHEN NEW.state='revoked'
             BEGIN SELECT RAISE(ABORT,'injected revocation failure'); END;",
        )?;
        Ok(())
    })
    .unwrap();
    assert!(s.begin_removal("cli").is_err());
    assert_eq!(s.removal_state("cli").unwrap(), None);
    assert!(s.pending_removals().unwrap().is_empty());
    assert_unchanged(&s, &before);
    s.read(|c| Store::assert_session_live(c, "cli")).unwrap();
    s.tx(|tx| {
        tx.execute_batch("DROP TRIGGER fail_removal_revocation")?;
        Ok(())
    })
    .unwrap();
    s.begin_removal("cli").unwrap();
    assert_retired(&s, &before);
}

#[test]
fn failed_cleanup_and_reopen_do_not_restore_revoked_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    let r = fixture(&s, EXTERNAL_PERMISSION_POLICY);
    let before = receipts(&s, &r);
    s.begin_removal("cli").unwrap();
    s.fail_next_write();
    assert!(matches!(s.finish_removal("cli"), Err(StoreError::Injected)));
    assert_retired(&s, &before);
    // Also exercise a real SQL failure after cleanup's permission cascade has run.
    s.tx(|tx| {
        tx.execute_batch(
            "CREATE TRIGGER fail_removal_cleanup BEFORE DELETE ON sessions
             BEGIN SELECT RAISE(ABORT,'injected cleanup failure'); END;",
        )?;
        Ok(())
    })
    .unwrap();
    assert!(s.finish_removal("cli").is_err());
    assert_retired(&s, &before);
    drop(s);

    let s = Store::open(dir.path()).unwrap();
    assert_retired(&s, &before);
    assert_eq!(s.pending_removals().unwrap(), vec!["cli"]);
    assert_eq!(s.begin_removal("cli").unwrap(), vec!["cli"]);
    assert_retired(&s, &before);
    s.tx(|tx| {
        tx.execute_batch("DROP TRIGGER fail_removal_cleanup")?;
        Ok(())
    })
    .unwrap();
    s.finish_removal("cli").unwrap();
    assert_eq!(s.removal_state("cli").unwrap(), Some(true));
    assert!(s.begin_removal("cli").unwrap().is_empty());
    for receipt in before {
        assert!(
            s.read(|c| Store::external_permission_in(c, &receipt.request.key))
                .unwrap()
                .is_none()
        );
    }
    s.integrity_check().unwrap();
}
