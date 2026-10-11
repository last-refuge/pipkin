use super::*;
use serde_json::json;

fn store() -> Store {
    Store::memory().unwrap()
}

fn session(s: &Store, id: &str) -> SessionRow {
    s.create_session(id, "/tmp", None, &Origin::Native, None, None, None, 1)
        .unwrap()
        .0
}

fn user(text: &str) -> EntryKind {
    EntryKind::Message {
        message: Message::user_text(text, 1),
        mode: InputMode::Prompt,
        request: None,
        partial: false,
    }
}

#[test]
fn create_is_idempotent_under_its_key() {
    let s = store();
    let (a, created) = s
        .create_session("s1", "/p", Some("k"), &Origin::Native, None, None, None, 1)
        .unwrap();
    assert!(created);
    let (b, created) = s
        .create_session("s2", "/q", Some("k"), &Origin::Native, None, None, None, 2)
        .unwrap();
    assert!(!created);
    assert_eq!(a.id, b.id);
    assert_eq!(s.root_sessions().unwrap().len(), 1);
}

#[test]
fn same_key_same_payload_is_the_original_and_changed_payload_conflicts() {
    let s = store();
    session(&s, "s");
    let first = s
        .tx(|tx| Store::admit(tx, "s", "k1", InputMode::Prompt, "hi", &[], None, 1))
        .unwrap();
    assert!(matches!(first, Admission::New(_)));
    let again = s
        .tx(|tx| Store::admit(tx, "s", "k1", InputMode::Prompt, "hi", &[], None, 2))
        .unwrap();
    assert!(matches!(again, Admission::Existing(_)));
    let changed = s
        .tx(|tx| Store::admit(tx, "s", "k1", InputMode::Prompt, "bye", &[], None, 3))
        .unwrap();
    assert_eq!(changed, Admission::Conflict);
    let mode = s
        .tx(|tx| Store::admit(tx, "s", "k1", InputMode::FollowUp, "hi", &[], None, 3))
        .unwrap();
    assert_eq!(mode, Admission::Conflict);
}

#[test]
fn an_injected_failure_rolls_back_and_admits_nothing() {
    let s = store();
    session(&s, "s");
    s.fail_next_write();
    let err = s.tx(|tx| Store::admit(tx, "s", "k", InputMode::Prompt, "x", &[], None, 1));
    assert!(matches!(err, Err(StoreError::Injected)));
    assert!(s.request("s", "k").unwrap().is_none());
}

#[test]
fn queue_entries_are_ordered_withdrawable_and_bounded() {
    let s = store();
    session(&s, "s");
    for i in 0..3 {
        s.tx(|tx| {
            Store::admit(
                tx,
                "s",
                &format!("q{i}"),
                InputMode::FollowUp,
                "t",
                &[],
                None,
                i,
            )
        })
        .unwrap();
    }
    let q = s.queue("s").unwrap();
    assert_eq!(
        q.iter().map(|r| r.queue_entry.unwrap()).collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert_eq!(
        s.cancel_queued("s", 2).unwrap().0,
        CancelQueuedOutcome::Cancelled
    );
    assert_eq!(
        s.cancel_queued("s", 2).unwrap().0,
        CancelQueuedOutcome::NotFound
    );
    assert_eq!(
        s.cancel_queued("s", 9).unwrap().0,
        CancelQueuedOutcome::NotFound
    );
    s.tx(|tx| {
        let run = Store::start_run(tx, "s", None, 1)?;
        Store::consume_queued(tx, "s", &["q0".into()], run)
    })
    .unwrap();
    assert_eq!(
        s.cancel_queued("s", 1).unwrap().0,
        CancelQueuedOutcome::AlreadyConsumed
    );
    for i in 0..MAX_QUEUE {
        let r = s
            .tx(|tx| {
                Store::admit(
                    tx,
                    "s",
                    &format!("f{i}"),
                    InputMode::Steer,
                    "t",
                    &[],
                    None,
                    1,
                )
            })
            .unwrap();
        if i + 1 >= MAX_QUEUE {
            assert_eq!(r, Admission::Full, "at {i}");
        }
    }
}

#[test]
fn streaming_chunks_are_appended_and_materialized_then_dropped_on_finalize() {
    let s = store();
    session(&s, "s");
    let partial = EntryKind::Message {
        message: Message::Assistant {
            content: vec![],
            api: "a".into(),
            provider: "p".into(),
            model: "m".into(),
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 1,
            extra: Default::default(),
        },
        mode: InputMode::Prompt,
        request: None,
        partial: true,
    };
    let entry = s
        .tx(|tx| Store::append_entry(tx, "s", 1, Some(1), &partial))
        .unwrap();
    s.tx(|tx| {
        Store::append_chunks(
            tx,
            "s",
            entry.id,
            &[
                Delta::Thinking {
                    block: 0,
                    text: "hm".into(),
                },
                Delta::Text {
                    block: 1,
                    text: "hel".into(),
                },
                Delta::Text {
                    block: 1,
                    text: "lo".into(),
                },
            ],
        )
    })
    .unwrap();
    let got = s.entry("s", entry.id).unwrap().unwrap();
    let EntryKind::Message { message, .. } = &got.kind else {
        panic!()
    };
    assert_eq!(message.text(), "hello");
    assert!(
        s.context_entries("s").unwrap().is_empty(),
        "partial is not context"
    );
    let mut done = got.clone();
    if let EntryKind::Message { partial, .. } = &mut done.kind {
        *partial = false;
    }
    s.tx(|tx| Store::replace_entry(tx, "s", &done)).unwrap();
    assert_eq!(s.context_entries("s").unwrap().len(), 1);
    let chunks: i64 = s
        .read(|c| Ok(c.query_row("SELECT COUNT(*) FROM entry_chunks", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(chunks, 0);
}

#[test]
fn context_starts_at_the_newest_head_marker_and_pages_reach_everything() {
    let s = store();
    session(&s, "s");
    let ids: Vec<u64> = (0..5)
        .map(|i| {
            s.tx(|tx| Store::append_entry(tx, "s", i, None, &user(&format!("u{i}"))))
                .unwrap()
                .id
        })
        .collect();
    s.tx(|tx| {
        Store::append_entry(
            tx,
            "s",
            9,
            None,
            &EntryKind::Compaction {
                summary: "sum".into(),
                first_kept: ids[3],
                tokens_before: 10,
            },
        )
    })
    .unwrap();
    let ctx = s.context_entries("s").unwrap();
    assert!(matches!(ctx[0].kind, EntryKind::Compaction { .. }));
    assert_eq!(ctx[1..].iter().map(|e| e.id).collect::<Vec<_>>(), &ids[3..]);
    let (window, older) = s.window("s", 100).unwrap();
    assert!(older);
    assert!(matches!(window[0].kind, EntryKind::Compaction { .. }));
    let (page, more) = s.page("s", Some(window[0].id), 2).unwrap();
    assert_eq!(page.iter().map(|e| e.id).collect::<Vec<_>>(), &ids[3..5]);
    assert!(more);
    s.tx(|tx| Store::append_entry(tx, "s", 10, None, &EntryKind::Reset { handoff: None }))
        .unwrap();
    let after = s
        .tx(|tx| Store::append_entry(tx, "s", 11, None, &user("new")))
        .unwrap();
    let ctx = s.context_entries("s").unwrap();
    assert!(matches!(ctx[0].kind, EntryKind::Reset { .. }));
    assert_eq!(ctx[1].id, after.id);
    assert_eq!(ctx.len(), 2);
}

#[test]
fn stop_intent_is_durable_and_settlement_closes_the_run() {
    let s = store();
    session(&s, "s");
    let run = s
        .tx(|tx| {
            Store::admit(tx, "s", "k", InputMode::Prompt, "x", &[], None, 1)?;
            Store::start_run(tx, "s", Some("k"), 1)
        })
        .unwrap();
    assert_eq!(s.request_stop("s", 2).unwrap().unwrap().0, run);
    let row = s.run(run).unwrap().unwrap();
    assert!(row.stop_requested);
    assert_eq!(row.view().unwrap().phase, RunPhase::Stopping);
    s.tx(|tx| {
        Store::settle_run(tx, run, &RunOutcome::Aborted, 3)?;
        Store::settle_request(tx, "s", "k", &RunOutcome::Aborted, 3)
    })
    .unwrap();
    assert!(s.open_run("s").unwrap().is_none());
    assert!(s.request_stop("s", 4).unwrap().is_none());
    assert_eq!(
        s.request("s", "k").unwrap().unwrap().lookup(),
        LookupResult::Known {
            mode: InputMode::Prompt,
            entry: None,
            run: Some(run),
            open: false,
            outcome: Some(RunOutcome::Aborted)
        }
    );
}

#[test]
fn tool_output_is_retained_with_honest_drop_counts() {
    let s = store();
    session(&s, "s");
    s.tx(|tx| {
        Store::tool_intent(
            tx,
            "s",
            "c1",
            None,
            "bash",
            &json!({"command": "x"}),
            false,
            1,
        )?;
        Store::tool_result(tx, "s", "c1", ToolState::Done, 5, "out", 7, 2)
    })
    .unwrap();
    let out = s.tool_output("s", "c1").unwrap().unwrap();
    assert_eq!(
        (out.text.as_str(), out.retained_bytes, out.dropped_bytes),
        ("out", 3, 7)
    );
    assert_eq!(
        s.tool_call("s", "c1").unwrap().unwrap().state,
        ToolState::Done
    );
}

#[test]
fn a_child_task_is_created_once_per_parent_call_and_delivered_once() {
    let s = store();
    session(&s, "root");
    s.create_session(
        "child",
        "/tmp",
        None,
        &Origin::Native,
        None,
        None,
        Some(("root", "root")),
        1,
    )
    .unwrap();
    let (t, created) = s
        .tx(|tx| Store::create_task(tx, "root", "root", "call", "child", "do it", 1))
        .unwrap();
    assert!(created);
    let (again, created) = s
        .tx(|tx| Store::create_task(tx, "root", "root", "call", "other", "do it", 1))
        .unwrap();
    assert!(!created);
    assert_eq!(t, again);
    assert!(s.tx(|tx| Store::mark_task_delivered(tx, "child")).unwrap());
    assert!(!s.tx(|tx| Store::mark_task_delivered(tx, "child")).unwrap());
    assert!(s.root_sessions().unwrap().iter().all(|r| r.id != "child"));
}

#[test]
fn usage_accumulates_per_bucket() {
    let s = store();
    session(&s, "s");
    let u = Usage {
        input: 1,
        output: 2,
        total_tokens: 3,
        ..Usage::default()
    };
    s.tx(|tx| {
        Store::add_usage(tx, "s", "model", "p/m", &u)?;
        Store::add_usage(tx, "s", "model", "p/m", &u)?;
        Store::add_usage(tx, "s", "tool", "subagent", &u)
    })
    .unwrap();
    let v = s.usage("s").unwrap();
    assert_eq!(v.models[0].1.total_tokens, 6);
    assert_eq!(v.tools[0].0, "subagent");
}

#[test]
fn a_newer_schema_is_refused_untouched_and_the_lock_is_exclusive() {
    let dir = tempfile::tempdir().unwrap();
    {
        let s = Store::open(dir.path()).unwrap();
        assert!(matches!(
            Store::open(dir.path()),
            Err(StoreError::Locked(_))
        ));
        s.integrity_check().unwrap();
    }
    let path = dir.path().join("engine.sqlite");
    {
        let c = Connection::open(&path).unwrap();
        c.execute_batch("PRAGMA user_version = 99").unwrap();
    }
    let before = std::fs::read(&path).unwrap();
    assert!(matches!(
        Store::open(dir.path()),
        Err(StoreError::NewerSchema(99))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn acknowledged_writes_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    {
        let s = Store::open(dir.path()).unwrap();
        session(&s, "s");
        s.tx(|tx| Store::admit(tx, "s", "k", InputMode::Prompt, "x", &[], None, 1))
            .unwrap();
    }
    let s = Store::open(dir.path()).unwrap();
    assert!(s.request("s", "k").unwrap().is_some());
    let backup = dir.path().join("copy.sqlite");
    s.backup_to(&backup).unwrap();
    let c = Connection::open(&backup).unwrap();
    let n: i64 = c
        .query_row("SELECT COUNT(*) FROM requests", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
}
