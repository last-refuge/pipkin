use super::*;
use crate::execution::claude_cli::events::{Event, Outcome};
use crate::store::external::permissions::*;

mod removal;

fn fixture(s: &Store, policy: &str) -> ExternalPermissionRequest {
    let epoch = s.next_epoch().unwrap();
    let mut binding = spec();
    binding.launch_policy = policy.into();
    s.create_cli_session_with_model("cli", "create", &binding, "sonnet", 1)
        .unwrap();
    let receipt = s
        .tx(|tx| Store::admit_external_prompt_in(tx, "cli", "request", "question", "op", 1, 2))
        .unwrap()
        .0
        .receipt;
    s.tx(|tx| Store::begin_external_launch_in(tx, &receipt.key, 3))
        .unwrap();
    s.tx(|tx| Store::begin_external_run_in(tx, &receipt.key, 4))
        .unwrap();
    ExternalPermissionRequest {
        key: ExternalPermissionKey {
            engine_epoch: epoch,
            owner: "connection-1".into(),
            run: receipt.key,
            prompt: receipt.id,
            invocation_nonce: "host-invocation-1".into(),
        },
        arguments_sha256: "a".repeat(64),
        tool_name: "Write".into(),
        policy_id: EXTERNAL_PERMISSION_POLICY.into(),
        expires_at_ms: 100,
    }
}
fn request(s: &Store, r: &ExternalPermissionRequest) -> ExternalPermissionReceipt {
    s.tx(|tx| Store::request_external_permission_in(tx, r, 5))
        .unwrap()
}
fn decide(
    s: &Store,
    r: &ExternalPermissionRequest,
    d: ExternalPermissionDecision,
) -> ExternalPermissionReceipt {
    s.tx(|tx| Store::decide_external_permission_in(tx, r, d, 6))
        .unwrap()
}
fn consume(s: &Store, r: &ExternalPermissionRequest) -> ExternalPermissionConsumption {
    s.tx(|tx| Store::consume_external_permission_in(tx, r, 7))
        .unwrap()
}
fn read(s: &Store, r: &ExternalPermissionRequest) -> ExternalPermissionReceipt {
    s.read(|c| Store::external_permission_in(c, &r.key))
        .unwrap()
        .unwrap()
}

#[test]
fn permission_debug_redacts_invocation_and_digest_through_receipt_wrappers() {
    let s = Store::memory().unwrap();
    let r = fixture(&s, EXTERNAL_PERMISSION_POLICY);
    assert_eq!(format!("{:?}", r.key), "ExternalPermissionKey([redacted])");
    assert_eq!(format!("{r:?}"), "ExternalPermissionRequest([redacted])");
    let receipt = request(&s, &r);
    let consumption = ExternalPermissionConsumption {
        receipt,
        newly_consumed: false,
    };
    let debug = format!("{consumption:?}");
    assert!(!debug.contains(&r.key.invocation_nonce));
    assert!(!debug.contains(&r.arguments_sha256));
}

#[test]
fn decisions_and_consumption_are_idempotent_not_repeat_authorization() {
    let s = Store::memory().unwrap();
    let r = fixture(&s, EXTERNAL_PERMISSION_POLICY);
    let pending = request(&s, &r);
    assert_eq!(request(&s, &r), pending);
    conflict(s.tx(|tx| Store::consume_external_permission_in(tx, &r, 5)));
    let granted = decide(&s, &r, ExternalPermissionDecision::AllowOnce);
    assert_eq!(granted.state, ExternalPermissionState::Granted);
    assert_eq!(
        decide(&s, &r, ExternalPermissionDecision::AllowOnce),
        granted
    );
    conflict(s.tx(|tx| {
        Store::decide_external_permission_in(tx, &r, ExternalPermissionDecision::Deny, 6)
    }));
    assert!(consume(&s, &r).newly_consumed);
    assert!(!consume(&s, &r).newly_consumed);
    assert_eq!(
        decide(&s, &r, ExternalPermissionDecision::AllowOnce).state,
        ExternalPermissionState::Consumed
    );
    let stopped = s
        .tx(|tx| Store::stop_external_prompt_in(tx, &r.key.run, 8))
        .unwrap()
        .0;
    assert_eq!(stopped.state, ExternalRunState::Stopping);
    assert!(!consume(&s, &r).newly_consumed);
    assert_eq!(s.external_actions(&r.key.run).unwrap(), vec![]);
    assert_eq!(
        s.external_prompt_by_id(r.key.prompt)
            .unwrap()
            .unwrap()
            .outcome,
        None
    );
    for table in ["runs", "requests", "tool_calls"] {
        assert_eq!(
            s.read(|c| Ok(
                c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                    .get::<_, i64>(0))?
            ))
            .unwrap(),
            0
        );
    }
}

#[test]
fn deny_expire_and_revoke_are_closed_and_receipts_never_regrant() {
    for terminal in ["deny", "expire-pending", "expire-granted", "revoke"] {
        let s = Store::memory().unwrap();
        let r = fixture(&s, EXTERNAL_PERMISSION_POLICY);
        request(&s, &r);
        let state = match terminal {
            "deny" => {
                decide(&s, &r, ExternalPermissionDecision::Deny);
                ExternalPermissionState::Denied
            }
            "expire-pending" => {
                let expired = s
                    .tx(|tx| {
                        Store::decide_external_permission_in(
                            tx,
                            &r,
                            ExternalPermissionDecision::AllowOnce,
                            100,
                        )
                    })
                    .unwrap();
                assert_eq!(expired.decision, None);
                ExternalPermissionState::Expired
            }
            "expire-granted" => {
                decide(&s, &r, ExternalPermissionDecision::AllowOnce);
                let expired = s
                    .tx(|tx| Store::consume_external_permission_in(tx, &r, 100))
                    .unwrap();
                assert!(!expired.newly_consumed);
                ExternalPermissionState::Expired
            }
            _ => {
                decide(&s, &r, ExternalPermissionDecision::AllowOnce);
                s.tx(|tx| Store::revoke_external_permission_in(tx, &r.key, 7))
                    .unwrap();
                ExternalPermissionState::Revoked
            }
        };
        assert_eq!(read(&s, &r).state, state);
        assert_eq!(request(&s, &r).state, state);
        conflict(s.tx(|tx| Store::consume_external_permission_in(tx, &r, 101)));
        // An explicit expiry/revoke is also idempotent, without changing another terminal state.
        s.tx(|tx| Store::expire_external_permission_in(tx, &r.key, 102))
            .unwrap();
        s.tx(|tx| Store::revoke_external_permission_in(tx, &r.key, 103))
            .unwrap();
        assert_eq!(read(&s, &r).state, state);
    }
}

#[test]
fn foreign_scope_and_changed_payload_are_rejected_at_every_authority_boundary() {
    let s = Store::memory().unwrap();
    let r = fixture(&s, EXTERNAL_PERMISSION_POLICY);
    request(&s, &r);
    decide(&s, &r, ExternalPermissionDecision::AllowOnce);
    let mut variants = vec![];
    for field in 0..12 {
        let mut bad = r.clone();
        match field {
            0 => bad.key.engine_epoch += 1,
            1 => bad.key.owner = "connection-2".into(),
            2 => bad.key.run.session = "native".into(),
            3 => bad.key.run.generation += 1,
            4 => bad.key.run.op = "other".into(),
            5 => bad.key.prompt += 1,
            6 => bad.key.invocation_nonce = "another-invocation".into(),
            7 => bad.arguments_sha256 = "b".repeat(64),
            8 => bad.tool_name = "Bash".into(),
            9 => bad.policy_id = "safe-restricted-no-tools-v1".into(),
            10 => bad.expires_at_ms += 1,
            _ => bad.arguments_sha256 = "A".repeat(64),
        }
        variants.push(bad);
    }
    for bad in variants {
        conflict(s.tx(|tx| {
            Store::decide_external_permission_in(tx, &bad, ExternalPermissionDecision::AllowOnce, 7)
        }));
        conflict(s.tx(|tx| Store::consume_external_permission_in(tx, &bad, 7)));
    }
    assert_eq!(read(&s, &r).state, ExternalPermissionState::Granted);
    let mut changed = r.clone();
    changed.arguments_sha256 = "c".repeat(64);
    conflict(s.tx(|tx| Store::request_external_permission_in(tx, &changed, 8)));
    let mut foreign = r.key.clone();
    foreign.owner = "foreign".into();
    assert!(
        s.read(|c| Store::external_permission_in(c, &foreign))
            .unwrap()
            .is_none()
    );
    assert!(
        s.read(|c| Store::external_permissions_in(c, &foreign, 0, 256))
            .unwrap()
            .is_empty()
    );
    conflict(s.tx(|tx| Store::revoke_external_permission_in(tx, &foreign, 8)));
    conflict(s.tx(|tx| Store::expire_external_permission_in(tx, &foreign, 100)));
    assert!(consume(&s, &r).newly_consumed);
}

#[test]
fn request_requires_running_coding_receipt_exact_binding_and_valid_metadata() {
    let s = Store::memory().unwrap();
    let r = fixture(&s, "safe-restricted-no-tools-v1");
    conflict(s.tx(|tx| Store::request_external_permission_in(tx, &r, 5)));
    let s = Store::memory().unwrap();
    let r = fixture(&s, EXTERNAL_PERMISSION_POLICY);
    for field in 0..8 {
        let mut bad = r.clone();
        match field {
            0 => bad.arguments_sha256 = "preview".into(),
            1 => bad.key.owner = "x".repeat(129),
            2 => bad.key.invocation_nonce = "credentials\n".into(),
            3 => bad.expires_at_ms = 5,
            4 => bad.key.prompt += 1,
            5 => bad.key.run.generation += 1,
            6 => bad.tool_name = "Read".into(),
            _ => bad.policy_id = "other".into(),
        }
        assert!(
            s.tx(|tx| Store::request_external_permission_in(tx, &bad, 5))
                .is_err()
        );
    }
    s.create_session(
        "native",
        "/project",
        None,
        &Origin::Native,
        None,
        None,
        None,
        1,
    )
    .unwrap();
    let mut native = r.clone();
    native.key.run.session = "native".into();
    assert!(
        s.tx(|tx| Store::request_external_permission_in(tx, &native, 5))
            .is_err()
    );
    s.tx(|tx| {
        tx.execute(
            "UPDATE cli_bindings SET context_valid=0,generation=generation+1 WHERE session='cli'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    conflict(s.tx(|tx| Store::request_external_permission_in(tx, &r, 5)));
    let s = Store::memory().unwrap();
    let mut binding = spec();
    binding.launch_policy = EXTERNAL_PERMISSION_POLICY.into();
    s.next_epoch().unwrap();
    s.create_cli_session("cli", "create", &binding, 1).unwrap();
    running(&s, &r.key.run);
    conflict(s.tx(|tx| Store::request_external_permission_in(tx, &r, 5)));
}

#[test]
fn failed_commits_and_transaction_rollbacks_never_release_authority() {
    let s = Store::memory().unwrap();
    let r = fixture(&s, EXTERNAL_PERMISSION_POLICY);
    s.fail_next_write();
    assert!(matches!(
        s.tx(|tx| Store::request_external_permission_in(tx, &r, 5)),
        Err(StoreError::Injected)
    ));
    assert!(
        s.read(|c| Store::external_permission_in(c, &r.key))
            .unwrap()
            .is_none()
    );
    request(&s, &r);
    s.fail_next_write();
    assert!(matches!(
        s.tx(|tx| Store::decide_external_permission_in(
            tx,
            &r,
            ExternalPermissionDecision::AllowOnce,
            6
        )),
        Err(StoreError::Injected)
    ));
    assert_eq!(read(&s, &r).state, ExternalPermissionState::Pending);
    decide(&s, &r, ExternalPermissionDecision::AllowOnce);
    s.fail_next_write();
    assert!(matches!(
        s.tx(|tx| Store::consume_external_permission_in(tx, &r, 7)),
        Err(StoreError::Injected)
    ));
    assert_eq!(read(&s, &r).state, ExternalPermissionState::Granted);
    // SQLite itself refuses the durable consumption write, not just the wrapper's commit.
    s.tx(|tx| { tx.execute_batch("CREATE TRIGGER fail_permission BEFORE UPDATE ON external_permissions BEGIN SELECT RAISE(ABORT,'test failure'); END;")?; Ok(()) }).unwrap();
    assert!(
        s.tx(|tx| Store::consume_external_permission_in(tx, &r, 7))
            .is_err()
    );
    assert_eq!(read(&s, &r).state, ExternalPermissionState::Granted);
    s.tx(|tx| {
        tx.execute_batch("DROP TRIGGER fail_permission")?;
        Ok(())
    })
    .unwrap();
    assert!(consume(&s, &r).newly_consumed);
}

#[test]
fn stop_settlement_recovery_and_epoch_rotation_atomically_revoke_unspent_authority() {
    for stage in ["pending", "granted"] {
        for ending in ["stop", "settle", "recover", "epoch"] {
            let s = Store::memory().unwrap();
            let r = fixture(&s, EXTERNAL_PERMISSION_POLICY);
            request(&s, &r);
            if stage == "granted" {
                decide(&s, &r, ExternalPermissionDecision::AllowOnce);
            }
            let end = || -> Result<()> {
                match ending {
                    "stop" => {
                        s.tx(|tx| Store::stop_external_prompt_in(tx, &r.key.run, 8))?;
                    }
                    "settle" => {
                        s.tx(|tx| {
                            Store::settle_external_prompt_in(
                                tx,
                                &r.key.run,
                                &ExternalSettlementEvidence::default(),
                                8,
                            )
                        })?;
                    }
                    "recover" => {
                        s.recover_external_runs(8)?;
                    }
                    _ => {
                        s.next_epoch()?;
                    }
                }
                Ok(())
            };
            s.fail_next_write();
            assert!(matches!(end(), Err(StoreError::Injected)));
            assert_eq!(
                read(&s, &r).state,
                if stage == "pending" {
                    ExternalPermissionState::Pending
                } else {
                    ExternalPermissionState::Granted
                }
            );
            assert_eq!(
                s.external_run(&r.key.run).unwrap().unwrap().state,
                ExternalRunState::Running
            );
            end().unwrap();
            assert_eq!(
                s.read(|c| Ok(
                    c.query_row("SELECT state FROM external_permissions", [], |r| r
                        .get::<_, String>(0))?
                ))
                .unwrap(),
                "revoked"
            );
            conflict(s.tx(|tx| Store::consume_external_permission_in(tx, &r, 9)));
            if ending == "epoch" {
                conflict(s.read(|c| Store::external_permission_in(c, &r.key)));
            }
        }
    }
}

#[test]
fn completed_prompt_refuses_unresolved_authorizations_but_consumed_is_not_completion() {
    for stage in ["pending", "granted", "consumed"] {
        let s = Store::memory().unwrap();
        let r = fixture(&s, EXTERNAL_PERMISSION_POLICY);
        request(&s, &r);
        if stage != "pending" {
            decide(&s, &r, ExternalPermissionDecision::AllowOnce);
        }
        if stage == "consumed" {
            consume(&s, &r);
        }
        assert_eq!(
            s.external_run(&r.key.run).unwrap().unwrap().state,
            ExternalRunState::Running
        );
        s.tx(|tx| Store::project_external_event_in(tx, &r.key.run, 1, &Event::Initialized, 8))
            .unwrap();
        s.tx(|tx| {
            Store::project_external_event_in(
                tx,
                &r.key.run,
                2,
                &Event::Terminal {
                    metadata: None,
                    outcome: Outcome::Success,
                    usage: None,
                },
                8,
            )
        })
        .unwrap();
        let evidence = ExternalSettlementEvidence {
            projection_finished: true,
            input_delivered: true,
            root_reaped: true,
            pipes_drained: true,
            exit_code: Some(0),
            ..Default::default()
        };
        let settle = || s.tx(|tx| Store::settle_external_prompt_in(tx, &r.key.run, &evidence, 9));
        if stage != "consumed" {
            conflict(settle());
            assert_eq!(
                s.external_run(&r.key.run).unwrap().unwrap().state,
                ExternalRunState::Running
            );
            s.tx(|tx| Store::revoke_external_permission_in(tx, &r.key, 9))
                .unwrap();
        }
        assert_eq!(
            settle().unwrap().0.receipt.state,
            ExternalRunState::Completed
        );
        // Raw ledger and action APIs remain unable to bypass receipt ownership.
        conflict(s.transition_external_run(
            &r.key.run,
            ExternalRunState::Running,
            ExternalRunState::Completed,
            10,
        ));
        conflict(s.record_external_action(
            &r.key.run,
            "call",
            "Write",
            ExternalActionStatus::Completed,
            10,
        ));
    }
}

#[test]
fn concurrent_consumers_release_at_most_one_allow() {
    let s = std::sync::Arc::new(Store::memory().unwrap());
    let r = fixture(&s, EXTERNAL_PERMISSION_POLICY);
    request(&s, &r);
    decide(&s, &r, ExternalPermissionDecision::AllowOnce);
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let s = s.clone();
            let r = r.clone();
            std::thread::spawn(move || consume(&s, &r).newly_consumed)
        })
        .collect();
    assert_eq!(
        handles
            .into_iter()
            .filter_map(|h| h.join().unwrap().then_some(()))
            .count(),
        1
    );
}

#[test]
fn request_budget_includes_closed_receipts_and_exact_retries_still_work() {
    let s = Store::memory().unwrap();
    let r = fixture(&s, EXTERNAL_PERMISSION_POLICY);
    request(&s, &r);
    decide(&s, &r, ExternalPermissionDecision::Deny);
    s.tx(|tx| {
        for i in 1..4096 {
            let mut other = r.clone();
            other.key.invocation_nonce = format!("host-{i}");
            Store::request_external_permission_in(tx, &other, 5)?;
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(request(&s, &r).state, ExternalPermissionState::Denied);
    let mut overflow = r.clone();
    overflow.key.invocation_nonce = "overflow".into();
    conflict(s.tx(|tx| Store::request_external_permission_in(tx, &overflow, 5)));
}

#[test]
fn immutable_sql_scope_and_bounded_reads() {
    let s = Store::memory().unwrap();
    let r = fixture(&s, EXTERNAL_PERMISSION_POLICY);
    request(&s, &r);
    for assignment in [
        "owner='other'",
        "engine_epoch=2",
        "prompt=2",
        "arguments_sha256=printf('%064d',0)",
        "expires_at=200",
        "state='consumed'",
    ] {
        assert!(
            s.tx(|tx| {
                tx.execute(&format!("UPDATE external_permissions SET {assignment}"), [])?;
                Ok(())
            })
            .is_err()
        );
    }
    for i in 1..260 {
        let mut other = r.clone();
        other.key.invocation_nonce = format!("nonce-{i}");
        request(&s, &other);
    }
    let first = s
        .read(|c| Store::external_permissions_in(c, &r.key, 0, 256))
        .unwrap();
    assert_eq!(first.len(), 256);
    let last = s
        .read(|c| Store::external_permissions_in(c, &r.key, first.last().unwrap().id, 256))
        .unwrap();
    assert_eq!(last.len(), 4);
    conflict(s.read(|c| Store::external_permissions_in(c, &r.key, 0, 257)));
}

#[test]
fn schema23_backup_migration_preserves_external_identity_and_refuses_newer_schema() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    let r = fixture(&s, EXTERNAL_PERMISSION_POLICY);
    s.tx(|tx| {
        crate::store::cli_agent::remove_schema_for_test(tx)?;
        crate::store::schema::remove_v33_for_test(tx)?;
        tx.execute_batch("DROP TABLE cli_bridge_calls; DROP TABLE cli_tool_bridges; DROP TABLE external_projection_actions; DROP TABLE external_permissions; PRAGMA user_version=23;")?;
        Ok(())
    })
    .unwrap();
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    assert_eq!(s.read(schema::version).unwrap(), SCHEMA_VERSION);
    let backup = Connection::open(dir.path().join("engine.sqlite.v23.bak")).unwrap();
    assert_eq!(schema::version(&backup).unwrap(), 23);
    assert!(
        backup
            .prepare("SELECT * FROM external_permissions")
            .is_err()
    );
    assert_eq!(
        s.session("cli").unwrap().unwrap().model.as_deref(),
        Some("sonnet")
    );
    assert_eq!(
        s.cli_binding("cli")
            .unwrap()
            .unwrap()
            .identity
            .cli_session_id,
        spec().cli_session_id
    );
    request(&s, &r);
    s.integrity_check().unwrap();
    s.tx(|tx| {
        tx.execute_batch(&format!("PRAGMA user_version={}", SCHEMA_VERSION + 1))?;
        Ok(())
    })
    .unwrap();
    drop(s);
    assert!(matches!(
        Store::open(dir.path()),
        Err(StoreError::NewerSchema(v)) if v == SCHEMA_VERSION + 1
    ));
}

// A child exits without running Rust/SQLite destructors at each durable boundary. No CLI,
// account, network or external action is invoked. Parent recovery must never replay authority.
#[test]
fn permission_crash_cut_child() {
    let Ok(dir) = std::env::var("PIPKIN_PERMISSION_CRASH_DIR") else {
        return;
    };
    let cut = std::env::var("PIPKIN_PERMISSION_CRASH_CUT").unwrap();
    let s = Store::open(std::path::Path::new(&dir)).unwrap();
    let r = fixture(&s, EXTERNAL_PERMISSION_POLICY);
    request(&s, &r);
    if cut != "pending" {
        decide(&s, &r, ExternalPermissionDecision::AllowOnce);
    }
    if cut == "consumed" {
        consume(&s, &r);
    }
    std::process::exit(0);
}

#[test]
fn crash_cuts_preserve_receipts_and_restart_never_resurrects_authority() {
    for cut in ["pending", "granted", "consumed"] {
        let dir = tempfile::tempdir().unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "store::external::tests::permissions::permission_crash_cut_child",
                "--nocapture",
            ])
            .env("PIPKIN_PERMISSION_CRASH_DIR", dir.path())
            .env("PIPKIN_PERMISSION_CRASH_CUT", cut)
            .status()
            .unwrap();
        assert!(status.success());
        let s = Store::open(dir.path()).unwrap();
        let receipt = s
            .read(|c| {
                Ok(c.query_row("SELECT * FROM external_permissions", [], |r| {
                    r.get::<_, String>("state")
                })?)
            })
            .unwrap();
        assert_eq!(receipt, cut);
        s.recover_external_runs(10).unwrap();
        let key = ExternalPermissionKey {
            engine_epoch: 1,
            owner: "connection-1".into(),
            run: key("op"),
            prompt: 1,
            invocation_nonce: "host-invocation-1".into(),
        };
        let receipt = s
            .read(|c| Store::external_permission_in(c, &key))
            .unwrap()
            .unwrap();
        assert_eq!(
            receipt.state,
            if cut == "consumed" {
                ExternalPermissionState::Consumed
            } else {
                ExternalPermissionState::Revoked
            }
        );
        if cut == "consumed" {
            assert!(
                !s.tx(|tx| Store::consume_external_permission_in(tx, &receipt.request, 11))
                    .unwrap()
                    .newly_consumed
            );
        } else {
            conflict(s.tx(|tx| Store::consume_external_permission_in(tx, &receipt.request, 11)));
        }
        assert_eq!(s.recover_external_runs(12).unwrap(), 0);
        s.next_epoch().unwrap();
        conflict(s.tx(|tx| Store::consume_external_permission_in(tx, &receipt.request, 13)));
        s.integrity_check().unwrap();
    }
}
