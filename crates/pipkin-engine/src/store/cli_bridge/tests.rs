use super::*;
use serde_json::json;

mod durability;
mod limits;

// Protocol-only unbound rows: reserved backend remains Unsupported and cannot dispatch tools.
fn fixture(s: &Store, sid: &str, backend: &str) -> BridgeSpec {
    fixture_with_parent(s, sid, backend, None)
}
fn fixture_with_parent(s: &Store, sid: &str, backend: &str, parent: Option<&str>) -> BridgeSpec {
    let run = s.tx(|tx| {
        tx.execute("INSERT INTO sessions(id,cwd,created_at,updated_at,origin,execution_backend,subagents_enabled,parent,root)
            VALUES(?1,'/project',1,1,'{}',?2,0,?3,?3)", params![sid,backend,parent])?;
        tx.execute("INSERT INTO runs(session,state,started_at) VALUES(?1,'running',1)", [sid])?;
        Ok(tx.last_insert_rowid() as u64)
    }).unwrap();
    BridgeSpec {
        id: format!("bridge-{sid}"),
        scope: Scope {
            session: sid.into(),
            generation: 1,
            operation: "op".into(),
            run,
        },
        owner: "owner".into(),
        binding_hash: [1; 32],
        capability_hash: [2; 32],
    }
}
fn connected(s: &Store, sid: &str) -> (BridgeSpec, Lease) {
    let spec = fixture(s, sid, ORCHESTRATION_BACKEND);
    assert!(s.prepare_cli_tool_bridge(&spec, 2).unwrap());
    let lease = s.claim_cli_tool_bridge(&spec, 3).unwrap().unwrap();
    (spec, lease)
}
fn admit(s: &Store, lease: &Lease, id: Value) -> Claim {
    match s
        .admit_cli_bridge_call(
            lease,
            &id,
            "read",
            &json!({"path":"secret-project-file"}),
            4,
        )
        .unwrap()
    {
        Admission::New(c) => c,
        other => panic!("expected new, got {other:?}"),
    }
}
fn state(s: &Store, spec: &BridgeSpec) -> String {
    s.read(|c| Ok(stored(c, &spec.id)?.unwrap().1)).unwrap()
}
fn receipt(s: &Store, id: &str) -> (String, Option<String>) {
    s.read(|c| {
        Ok(c.query_row(
            "SELECT state,result FROM cli_bridge_calls WHERE call_id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?)
    })
    .unwrap()
}
#[test]
fn reserved_backend_cannot_enable_native_api_and_old_backends_cannot_mint() {
    let s = Store::memory().unwrap();
    for backend in ["nativeAgent", "localClaudeCli", "futureOther"] {
        let spec = fixture(&s, backend, backend);
        assert!(matches!(
            s.prepare_cli_tool_bridge(&spec, 2),
            Err(StoreError::UnsupportedExecution)
        ));
    }
    let (spec, _) = connected(&s, "new");
    assert_eq!(
        s.session("new").unwrap().unwrap().execution_backend,
        ExecutionBackend::ClaudeCliAgent
    );
    assert!(matches!(
        s.read(|c| Store::assert_native_execution(c, &spec.scope.session)),
        Err(StoreError::UnsupportedExecution)
    ));
}
#[test]
fn immutable_identity_single_connection_and_wrong_proof_cannot_revoke() {
    let s = Store::memory().unwrap();
    let (spec, lease) = connected(&s, "s");
    assert!(!s.prepare_cli_tool_bridge(&spec, 4).unwrap());
    assert!(s.claim_cli_tool_bridge(&spec, 4).unwrap().is_none());
    for field in 0..8 {
        let mut changed = spec.clone();
        match field {
            0 => changed.owner.push('x'),
            1 => changed.scope.session.push('x'),
            2 => changed.scope.generation += 1,
            3 => changed.scope.operation.push('x'),
            4 => changed.scope.run += 1,
            5 => changed.binding_hash[0] += 1,
            6 => changed.capability_hash[0] += 1,
            _ => changed.id.push('x'),
        }
        assert!(s.prepare_cli_tool_bridge(&changed, 4).is_err());
        assert!(s.claim_cli_tool_bridge(&changed, 4).is_err());
    }
    assert_eq!(state(&s, &spec), "connected");
    admit(&s, &lease, json!(1));
}
#[test]
fn duplicate_and_lost_admission_response_never_issue_another_claim() {
    let s = Store::memory().unwrap();
    let (_, l) = connected(&s, "s");
    let c = admit(&s, &l, json!(1)); // Pretend the owner lost this acknowledgement.
    assert!(matches!(
        s.admit_cli_bridge_call(
            &l,
            &json!(1),
            "read",
            &json!({"path":"secret-project-file"}),
            5
        )
        .unwrap(),
        Admission::Pending
    ));
    let text_id = admit(&s, &l, json!("1"));
    assert_ne!(text_id.call_id(), c.call_id());
}
#[test]
fn canonical_arguments_and_results_replay_committed_receipts_only() {
    let s = Store::memory().unwrap();
    let (_, l) = connected(&s, "s");
    let Admission::New(c) = s
        .admit_cli_bridge_call(
            &l,
            &json!("a"),
            "tool",
            &json!({"a":1,"z":{"c":3,"b":2}}),
            4,
        )
        .unwrap()
    else {
        panic!()
    };
    assert!(matches!(
        s.admit_cli_bridge_call(
            &l,
            &json!("a"),
            "tool",
            &json!({"z":{"b":2,"c":3},"a":1}),
            5
        )
        .unwrap(),
        Admission::Pending
    ));
    let result = json!({"content":[{"text":"actual result", "type":"text"}],"isError":false});
    assert_eq!(
        s.settle_cli_bridge_call(&l, &c, &result, 6).unwrap(),
        Settlement::Committed
    );
    assert_eq!(
        s.settle_cli_bridge_call(&l, &c, &result, 7).unwrap(),
        Settlement::AlreadyCommitted
    );
    match s
        .admit_cli_bridge_call(
            &l,
            &json!("a"),
            "tool",
            &json!({"a":1,"z":{"c":3,"b":2}}),
            8,
        )
        .unwrap()
    {
        Admission::Completed(v) => assert_eq!(v, result),
        _ => panic!(),
    }
}
#[test]
fn conflicting_request_fence_commits_and_late_success_never_upgrades_unknown() {
    let s = Store::memory().unwrap();
    let (spec, l) = connected(&s, "s");
    let c = admit(&s, &l, json!(1));
    assert!(matches!(
        s.admit_cli_bridge_call(&l, &json!(1), "write", &json!({}), 5)
            .unwrap(),
        Admission::Fenced
    ));
    assert_eq!(state(&s, &spec), "fenced");
    assert_eq!(receipt(&s, c.call_id()), ("unknown".into(), None));
    assert_eq!(
        s.settle_cli_bridge_call(&l, &c, &json!({"content":[]}), 6)
            .unwrap(),
        Settlement::Unknown
    );
    assert!(s.claim_cli_tool_bridge(&spec, 7).unwrap().is_none());
}
#[test]
fn contradictory_result_preserves_original_and_fences_other_work() {
    let s = Store::memory().unwrap();
    let (spec, l) = connected(&s, "s");
    let c = admit(&s, &l, json!(1));
    let other = admit(&s, &l, json!(2));
    let result = json!({"content":[{"type":"text","text":"original"}]});
    assert_eq!(
        s.settle_cli_bridge_call(&l, &c, &result, 5).unwrap(),
        Settlement::Committed
    );
    let original = receipt(&s, c.call_id());
    assert_eq!(
        s.settle_cli_bridge_call(&l, &c, &json!({"content":[]}), 6)
            .unwrap(),
        Settlement::Fenced
    );
    assert_eq!(receipt(&s, c.call_id()), original);
    assert_eq!(receipt(&s, other.call_id()).0, "unknown");
    assert_eq!(state(&s, &spec), "fenced");
}
#[test]
fn foreign_claim_and_invalid_scope_do_not_touch_owner() {
    let s = Store::memory().unwrap();
    let (spec, l) = connected(&s, "s");
    let c = admit(&s, &l, json!(1));
    let (_, foreign) = connected(&s, "other");
    assert!(
        s.settle_cli_bridge_call(&foreign, &c, &json!({}), 5)
            .is_err()
    );
    assert_eq!(state(&s, &spec), "connected");
    for id in [
        Value::Null,
        json!(1.5),
        json!(u64::MAX),
        json!(""),
        json!("\n"),
        json!({}),
    ] {
        assert!(
            s.admit_cli_bridge_call(&l, &id, "tool", &json!({}), 5)
                .is_err()
        );
    }
    for generation in [0, u64::MAX] {
        let mut invalid = spec.clone();
        invalid.scope.generation = generation;
        assert!(s.prepare_cli_tool_bridge(&invalid, 5).is_err());
    }
}
#[test]
fn stop_and_ancestor_policy_are_checked_independently() {
    let s = Store::memory().unwrap();
    let (spec, l) = connected(&s, "s");
    let c = admit(&s, &l, json!(1));
    s.tx(|tx| {
        tx.execute(
            "UPDATE runs SET stop_requested_at=5 WHERE id=?1",
            [spec.scope.run as i64],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        s.settle_cli_bridge_call(&l, &c, &json!({}), 6).unwrap(),
        Settlement::Fenced
    );
    fixture(&s, "parent", "nativeAgent");
    let child = fixture_with_parent(&s, "child", ORCHESTRATION_BACKEND, Some("parent"));
    assert!(matches!(
        s.prepare_cli_tool_bridge(&child, 7),
        Err(StoreError::UnsupportedExecution)
    ));
}
#[test]
fn transaction_failure_never_returns_a_claim_or_a_committed_result() {
    let s = Store::memory().unwrap();
    let (_, l) = connected(&s, "s");
    s.fail_next_write();
    assert!(matches!(
        s.admit_cli_bridge_call(
            &l,
            &json!(1),
            "read",
            &json!({"path":"secret-project-file"}),
            4
        ),
        Err(StoreError::Injected)
    ));
    let c = admit(&s, &l, json!(1));
    s.fail_next_write();
    assert!(matches!(
        s.settle_cli_bridge_call(&l, &c, &json!({}), 5),
        Err(StoreError::Injected)
    ));
    assert_eq!(receipt(&s, c.call_id()).0, "intent");
    s.fail_next_write();
    assert!(s.fence_cli_tool_bridge(&l, 6).is_err());
    assert_eq!(receipt(&s, c.call_id()).0, "intent");
    s.fence_cli_tool_bridge(&l, 7).unwrap();
    assert_eq!(receipt(&s, c.call_id()).0, "unknown");
}
#[test]
fn restart_retires_prepared_and_connected_but_preserves_committed_results() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    let (spec, l) = connected(&s, "s");
    let c = admit(&s, &l, json!(1));
    let done = admit(&s, &l, json!(2));
    s.settle_cli_bridge_call(&l, &done, &json!({"content":[]}), 5)
        .unwrap();
    let prepared = fixture(&s, "prepared", ORCHESTRATION_BACKEND);
    s.prepare_cli_tool_bridge(&prepared, 5).unwrap();
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    assert_eq!(s.recover_cli_tool_bridges(10).unwrap(), 2);
    assert_eq!(s.recover_cli_tool_bridges(11).unwrap(), 0);
    assert_eq!(receipt(&s, c.call_id()).0, "unknown");
    assert_eq!(receipt(&s, done.call_id()).0, "completed");
    assert!(s.claim_cli_tool_bridge(&spec, 12).unwrap().is_none());
    assert!(s.claim_cli_tool_bridge(&prepared, 12).unwrap().is_none());
    assert_eq!(
        s.settle_cli_bridge_call(&l, &c, &json!({}), 12).unwrap(),
        Settlement::Unknown
    );
}
#[test]
fn removal_retires_authority_and_payloads_but_permanently_reserves_identity() {
    let s = Store::memory().unwrap();
    let (spec, l) = connected(&s, "s");
    let c = admit(&s, &l, json!(1));
    s.begin_removal("s").unwrap();
    assert_eq!(state(&s, &spec), "fenced");
    assert_eq!(receipt(&s, c.call_id()).0, "unknown");
    assert!(s.claim_cli_tool_bridge(&spec, 6).unwrap().is_none());
    s.finish_removal("s").unwrap();
    assert_eq!(
        s.read(|tx| Ok(
            tx.query_row("SELECT COUNT(*) FROM cli_bridge_calls", [], |r| r
                .get::<_, i64>(0))?
        ))
        .unwrap(),
        0
    );
    assert_eq!(state(&s, &spec), "fenced");
    assert!(!s.prepare_cli_tool_bridge(&spec, 7).unwrap());
    assert!(
        s.tx(|tx| {
            tx.execute("DELETE FROM cli_tool_bridges", [])?;
            Ok(())
        })
        .is_err()
    );
}
#[test]
fn clean_close_requires_all_results_and_never_reconnects() {
    let s = Store::memory().unwrap();
    let (spec, l) = connected(&s, "s");
    let c = admit(&s, &l, json!(1));
    s.settle_cli_bridge_call(&l, &c, &json!({}), 5).unwrap();
    assert!(s.close_cli_tool_bridge(&l, 6).unwrap());
    assert!(s.close_cli_tool_bridge(&l, 7).unwrap());
    assert_eq!(state(&s, &spec), "closed");
    assert!(s.claim_cli_tool_bridge(&spec, 8).unwrap().is_none());
    let (other, l) = connected(&s, "other");
    admit(&s, &l, json!(1));
    assert!(!s.close_cli_tool_bridge(&l, 6).unwrap());
    assert_eq!(state(&s, &other), "fenced");
}
#[test]
fn sql_guards_prevent_rebinding_and_receipt_rewrites() {
    let s = Store::memory().unwrap();
    let (_, l) = connected(&s, "s");
    let c = admit(&s, &l, json!(1));
    for sql in [
        "UPDATE cli_tool_bridges SET owner='other'",
        "UPDATE cli_tool_bridges SET state='prepared'",
        "UPDATE cli_tool_bridges SET binding_hash=zeroblob(32)",
        "UPDATE cli_bridge_calls SET name='write'",
        "UPDATE cli_bridge_calls SET rpc_key='2'",
    ] {
        assert!(
            s.tx(|tx| {
                tx.execute(sql, [])?;
                Ok(())
            })
            .is_err(),
            "{sql}"
        );
    }
    s.fence_cli_tool_bridge(&l, 5).unwrap();
    assert!(
        s.tx(|tx| {
            tx.execute(
                "UPDATE cli_bridge_calls SET state='completed',result='{}'",
                [],
            )?;
            Ok(())
        })
        .is_err()
    );
    assert_eq!(receipt(&s, c.call_id()).0, "unknown");
}
#[test]
fn pending_limit_and_aggregate_result_budget_fence_durably() {
    let s = Store::memory().unwrap();
    let (spec, l) = connected(&s, "s");
    for i in 0..MAX_PENDING {
        admit(&s, &l, json!(i));
    }
    assert!(matches!(
        s.admit_cli_bridge_call(&l, &json!(MAX_PENDING), "read", &json!({}), 5)
            .unwrap(),
        Admission::Fenced
    ));
    assert_eq!(state(&s, &spec), "fenced");
    let (spec, l) = connected(&s, "bytes");
    let payload = json!({"text":"x".repeat(MAX_RESULT_BYTES-100)});
    let mut fenced = false;
    for i in 0..40 {
        let c = admit(&s, &l, json!(i));
        if s.settle_cli_bridge_call(&l, &c, &payload, 6).unwrap() == Settlement::Fenced {
            fenced = true;
            break;
        }
    }
    assert!(fenced);
    assert_eq!(state(&s, &spec), "fenced");
    assert!(s.read(|tx| journal_bytes(tx, &spec.id)).unwrap() <= MAX_JOURNAL_BYTES);
}
#[test]
fn diagnostics_redact_capabilities_arguments_and_results() {
    let s = Store::memory().unwrap();
    let (spec, l) = connected(&s, "private-session");
    let c = admit(&s, &l, json!(1));
    let output = format!(
        "{spec:?} {l:?} {c:?} {:?}",
        Admission::Completed(json!({"secret":"private-result"}))
    );
    for private in [
        "private-session",
        "secret-project-file",
        "private-result",
        "owner",
    ] {
        assert!(!output.contains(private));
    }
}
