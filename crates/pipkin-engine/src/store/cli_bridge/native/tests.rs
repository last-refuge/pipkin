use super::*;
fn fixture() -> (Store, Lease) {
    let s = Store::memory().unwrap();
    let l = fixture_in(&s, "/project");
    (s, l)
}
fn fixture_in(s: &Store, cwd: &str) -> Lease {
    let binding = s
        .create_cli_agent_session(&crate::store::cli_agent::tests::create("s", cwd), 1)
        .unwrap()
        .0;
    let run = s
        .tx(|tx| {
            tx.execute(
                "INSERT INTO runs(session,state,started_at) VALUES('s','running',1)",
                [],
            )?;
            Ok(tx.last_insert_rowid() as u64)
        })
        .unwrap();
    let spec = BridgeSpec {
        id: "b".into(),
        scope: Scope {
            session: "s".into(),
            generation: 1,
            operation: "op".into(),
            run,
        },
        owner: "owner".into(),
        binding_hash: binding.digest,
        capability_hash: [2; 32],
    };
    s.prepare_cli_tool_bridge(&spec, 2).unwrap();
    s.claim_cli_tool_bridge(&spec, 3).unwrap().unwrap()
}
mod durability;

fn claim(s: &Store, l: &Lease) -> Claim {
    let Admission::New(c) = s
        .admit_cli_native_call(l, &json!(1), "write", &json!({"path":"x","content":"y"}), 4)
        .unwrap()
    else {
        panic!()
    };
    c
}
fn result() -> ToolResult {
    ToolResult {
        content: vec![Block::text("actual result")],
        is_error: false,
        details: Some(json!({"private":"details"})),
        retained: "retained output".into(),
        dropped_bytes: 2,
    }
}
fn counts(s: &Store) -> (i64, i64, i64) {
    s.read(|c| Ok(c.query_row("SELECT (SELECT COUNT(*) FROM cli_bridge_calls),(SELECT COUNT(*) FROM tool_calls),(SELECT COUNT(*) FROM entries)",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?)).unwrap()
}
#[test]
fn service_guard_requires_exact_pending_native_join_not_protocol_metadata() {
    let (s, l) = fixture();
    let Admission::New(protocol) = s
        .admit_cli_bridge_call(&l, &json!(99), "plan_update", &json!({}), 4)
        .unwrap()
    else {
        panic!()
    };
    assert!(
        s.read(|tx| Store::assert_cli_native_service_in(tx, &l, protocol.call_id(), "plan_update"))
            .is_err()
    );
    let native = claim(&s, &l);
    assert!(
        s.read(|tx| Store::assert_cli_native_service_in(tx, &l, native.call_id(), "write"))
            .is_ok()
    );
    assert!(
        s.read(|tx| Store::assert_cli_native_service_in(tx, &l, native.call_id(), "plan_update"))
            .is_err()
    );
    assert!(
        s.read(|tx| Store::assert_cli_native_service_in(tx, &l, "sdk-id", "write"))
            .is_err()
    );
    s.settle_cli_native_call(&l, &native, &result(), 5).unwrap();
    assert!(
        s.read(|tx| Store::assert_cli_native_service_in(tx, &l, native.call_id(), "write"))
            .is_err()
    );
}
#[test]
fn published_updates_are_exact_committed_entries_and_replays_emit_nothing() {
    let (s, l) = fixture();
    let args = json!({"path":"x","content":"y"});
    let (admission, update) = s
        .admit_cli_native_call_published(&l, &json!(1), "write", &args, 4)
        .unwrap();
    let Admission::New(claim) = admission else {
        panic!("new claim")
    };
    let verify = |update: Option<Update>| {
        let update = update.unwrap();
        assert_eq!(&update.scope, l.scope());
        assert_eq!(update.revision, s.session("s").unwrap().unwrap().revision);
        assert_eq!(update.changes.len(), 1);
        let SessionChange::EntryAppended { entry } = &update.changes[0] else {
            panic!("append")
        };
        assert_eq!(Some(entry.clone()), s.entry("s", entry.id).unwrap());
    };
    verify(update);
    let (pending, update) = s
        .admit_cli_native_call_published(&l, &json!(1), "write", &args, 5)
        .unwrap();
    assert!(matches!(pending, Admission::Pending));
    assert!(update.is_none());
    let ((settlement, _), update) = s
        .settle_cli_native_call_published(&l, &claim, &result(), 6)
        .unwrap();
    assert_eq!(settlement, Settlement::Committed);
    verify(update);
    let ((settlement, _), update) = s
        .settle_cli_native_call_published(&l, &claim, &result(), 7)
        .unwrap();
    assert_eq!(settlement, Settlement::AlreadyCommitted);
    assert!(update.is_none());
    let (replayed, update) = s
        .admit_cli_native_call_published(&l, &json!(1), "write", &args, 8)
        .unwrap();
    assert!(matches!(replayed, Admission::Completed(_)));
    assert!(update.is_none());
}
#[test]
fn intent_entry_ids_and_result_output_commit_together_and_old_guard_stays_closed() {
    let (s, l) = fixture();
    let c = claim(&s, &l);
    assert_eq!(counts(&s), (1, 1, 1));
    let t = s.tool_call("s", c.call_id()).unwrap().unwrap();
    assert!(!t.replay_safe);
    assert_eq!(t.state, ToolState::Intent);
    assert!(
        s.read(|tx| Store::assert_native_execution(tx, "s"))
            .is_err()
    );
    let result = result();
    assert_eq!(
        s.settle_cli_native_call(&l, &c, &result, 5).unwrap().0,
        Settlement::Committed
    );
    assert_eq!(counts(&s), (1, 1, 2));
    assert_eq!(
        s.tool_output("s", c.call_id()).unwrap().unwrap().text,
        result.retained
    );
    assert_eq!(
        s.cli_native_receipt(&l, &json!(1)).unwrap(),
        mcp_result(&result).unwrap()
    );
    assert_eq!(
        s.settle_cli_native_call(&l, &c, &result, 6).unwrap().0,
        Settlement::AlreadyCommitted
    );
    assert_eq!(counts(&s), (1, 1, 2));
    assert!(matches!(
        s.admit_cli_native_call(
            &l,
            &json!(1),
            "write",
            &json!({"path":"x","content":"y"}),
            7
        )
        .unwrap(),
        Admission::Completed(_)
    ));
    assert_eq!(counts(&s), (1, 1, 2));
}
#[test]
fn faults_rollback_both_journals_and_transcript_without_acknowledgement() {
    let (s, l) = fixture();
    s.fail_next_write();
    assert!(
        s.admit_cli_native_call(&l, &json!(1), "write", &json!({}), 4)
            .is_err()
    );
    assert_eq!(counts(&s), (0, 0, 0));
    let c = claim(&s, &l);
    s.fail_next_write();
    assert!(s.settle_cli_native_call(&l, &c, &result(), 5).is_err());
    assert_eq!(counts(&s), (1, 1, 1));
    assert_eq!(
        s.tool_call("s", c.call_id()).unwrap().unwrap().state,
        ToolState::Intent
    );
    assert!(s.cli_native_receipt(&l, &json!(1)).is_err());
    assert!(s.tool_output("s", c.call_id()).unwrap().is_none());
}
#[test]
fn late_results_after_fence_never_turn_unknown_into_native_success() {
    let (s, l) = fixture();
    let c = claim(&s, &l);
    s.fence_cli_tool_bridge(&l, 5).unwrap();
    let t = s.tool_call("s", c.call_id()).unwrap().unwrap();
    assert_eq!(t.state, ToolState::Interrupted);
    assert!(t.result_entry.is_none());
    assert_eq!(
        s.settle_cli_native_call(&l, &c, &result(), 6).unwrap().0,
        Settlement::Unknown
    );
    assert_eq!(counts(&s), (1, 1, 1));
    assert!(s.tool_output("s", c.call_id()).unwrap().is_none());
}
#[test]
fn native_metadata_or_retained_output_cannot_contradict_committed_receipt() {
    for change in 0..3 {
        let (s, l) = fixture();
        let c = claim(&s, &l);
        let original = result();
        s.settle_cli_native_call(&l, &c, &original, 5).unwrap();
        let mut changed = original.clone();
        match change {
            0 => changed.details = Some(json!({"different":true})),
            1 => changed.retained.push('x'),
            _ => changed.dropped_bytes += 1,
        }
        assert_eq!(
            s.settle_cli_native_call(&l, &c, &changed, 6).unwrap().0,
            Settlement::Fenced
        );
        assert_eq!(counts(&s), (1, 1, 2));
        assert_eq!(
            s.tool_output("s", c.call_id()).unwrap().unwrap().text,
            original.retained
        );
    }
}
#[test]
fn protocol_only_receipts_cannot_be_promoted_into_native_dispatch() {
    let (s, l) = fixture();
    s.admit_cli_bridge_call(&l, &json!(1), "write", &json!({}), 4)
        .unwrap();
    assert!(matches!(
        s.admit_cli_native_call(&l, &json!(1), "write", &json!({}), 5)
            .unwrap(),
        Admission::Fenced
    ));
    assert_eq!(counts(&s), (1, 0, 0));
}
#[test]
fn image_content_is_structured_and_unsupported_or_oversize_output_is_not_silently_flattened() {
    let mut r = result();
    r.content = vec![Block::image("image/png", "base64")];
    assert_eq!(
        mcp_result(&r).unwrap()["content"][0],
        json!({"type":"image","mimeType":"image/png","data":"base64"})
    );
    r.content = vec![Block::tool_call("fake", "write", json!({}))];
    assert!(mcp_result(&r).is_err());
    r.content = vec![Block::text("x".repeat(MAX_RESULT_BYTES))];
    assert!(mcp_result(&r).is_err());
}
#[test]
fn progress_requires_live_joined_scope_and_restart_keeps_interrupted_intents_nonreplayable() {
    let (s, l) = fixture();
    let c = claim(&s, &l);
    let progress = BoundedOutput::default();
    s.cli_native_progress(&l, c.call_id(), &progress).unwrap();
    assert!(s.cli_native_progress(&l, "foreign", &progress).is_err());
    s.recover_cli_tool_bridges(5).unwrap();
    assert!(s.cli_native_progress(&l, c.call_id(), &progress).is_err());
    assert_eq!(
        s.tool_call("s", c.call_id()).unwrap().unwrap().state,
        ToolState::Interrupted
    );
    assert_eq!(
        s.settle_cli_native_call(&l, &c, &result(), 6).unwrap().0,
        Settlement::Unknown
    );
}
