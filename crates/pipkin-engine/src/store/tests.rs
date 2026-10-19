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
fn usage_optional_subsets_survive_reopen_and_later_legacy_records() {
    let dir = tempfile::tempdir().unwrap();
    {
        let s = Store::open(dir.path()).unwrap();
        session(&s, "s");
        s.tx(|tx| {
            Store::add_usage(
                tx,
                "s",
                "model",
                "p/m",
                &Usage {
                    output: 20,
                    cache_write: 8,
                    reasoning: Some(5),
                    cache_write_1h: Some(3),
                    total_tokens: 28,
                    ..Usage::default()
                },
            )
        })
        .unwrap();
    }
    let s = Store::open(dir.path()).unwrap();
    s.tx(|tx| {
        Store::add_usage(
            tx,
            "s",
            "model",
            "p/m",
            &Usage {
                output: 2,
                total_tokens: 2,
                ..Usage::default()
            },
        )
    })
    .unwrap();
    let u = &s.usage("s").unwrap().models[0].1;
    assert_eq!(u.reasoning, Some(5));
    assert_eq!(u.cache_write_1h, Some(3));
    assert_eq!(u.total_tokens, 30);
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

/// Policy: a damaged engine database is refused at open with an error that names the file,
/// and the file is left byte-for-byte as it was (no WAL switch, no migration, no repair).
#[test]
fn a_damaged_database_is_refused_untouched_and_named() {
    // Garbage where the database should be.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("engine.sqlite");
    std::fs::write(&path, vec![0x5au8; 64 * 1024]).unwrap();
    let before = std::fs::read(&path).unwrap();
    let Err(StoreError::Corrupt(why)) = Store::open(dir.path()) else {
        panic!("garbage must be refused")
    };
    assert!(why.contains(&path.display().to_string()), "{why}");
    assert!(why.contains("left untouched"), "{why}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!dir.path().join("engine.sqlite-wal").exists());

    // A real database with a damaged page in the middle.
    let dir = tempfile::tempdir().unwrap();
    {
        let s = Store::open(dir.path()).unwrap();
        session(&s, "s");
        s.tx(|tx| {
            for i in 0..400 {
                Store::append_entry(tx, "s", i, None, &user(&"words ".repeat(200)))?;
            }
            Ok(())
        })
        .unwrap();
    }
    let path = dir.path().join("engine.sqlite");
    let mut bytes = std::fs::read(&path).unwrap();
    assert!(bytes.len() > 4096 * 40, "{}", bytes.len());
    for b in &mut bytes[4096 * 20..4096 * 22] {
        *b = 0xab;
    }
    std::fs::write(&path, &bytes).unwrap();
    let Err(StoreError::Corrupt(why)) = Store::open(dir.path()) else {
        panic!("a damaged page must be refused")
    };
    assert!(why.contains("engine.sqlite is damaged"), "{why}");
    assert_eq!(std::fs::read(&path).unwrap(), bytes, "left untouched");
    // The refusal is a storage error for clients.
    let proto: ProtoError = StoreError::Corrupt(why).into();
    assert_eq!(proto.code, ErrorCode::Storage);
}

/// Injected faults strike only their phase (from SQLite itself, rolling the whole transaction
/// back), disk-full and read-only refuse real writes, and clearing restores normal service.
#[test]
fn injected_faults_fail_their_phase_and_roll_back_until_cleared() {
    let s = store();
    session(&s, "s");
    let admit = |key: &str, text: &str| {
        s.tx(|tx| Store::admit(tx, "s", key, InputMode::Prompt, text, &[], None, 1))
    };
    s.inject(Fault::Admission).unwrap();
    let err = admit("k1", "x").unwrap_err();
    assert!(err.to_string().contains("injected at admission"), "{err}");
    assert!(s.request("s", "k1").unwrap().is_none(), "rolled back");
    // Other phases still work.
    s.tx(|tx| Store::start_run(tx, "s", None, 1)).unwrap();
    s.clear_faults().unwrap();
    assert!(matches!(admit("k1", "x").unwrap(), Admission::New(_)));

    let run = s.tx(|tx| Store::start_run(tx, "s", None, 2)).unwrap();
    s.inject(Fault::Settlement).unwrap();
    let err = s
        .tx(|tx| Store::settle_run(tx, run, &RunOutcome::Completed, 3))
        .unwrap_err();
    assert!(err.to_string().contains("injected at settlement"), "{err}");
    assert_eq!(s.run(run).unwrap().unwrap().state, RunState::Running);
    s.clear_faults().unwrap();

    // Disk full: a write that needs new pages fails with SQLite's own SQLITE_FULL.
    let dir = tempfile::tempdir().unwrap();
    let disk = Store::open(dir.path()).unwrap();
    session(&disk, "s");
    disk.inject(Fault::DiskFull).unwrap();
    let big = "x".repeat(256 * 1024);
    let err = disk
        .tx(|tx| Store::admit(tx, "s", "big", InputMode::Prompt, &big, &[], None, 1))
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::Sqlite(rusqlite::Error::SqliteFailure(f, _))
            if f.code == rusqlite::ErrorCode::DiskFull),
        "{err:?}"
    );
    assert!(disk.request("s", "big").unwrap().is_none());
    disk.clear_faults().unwrap();
    disk.tx(|tx| Store::admit(tx, "s", "big", InputMode::Prompt, &big, &[], None, 1))
        .unwrap();

    // Read-only: every write is refused.
    disk.inject(Fault::ReadOnly).unwrap();
    let err = disk.tx(|tx| Store::kv_set(tx, "s", "a", "b")).unwrap_err();
    assert!(err.to_string().contains("readonly"), "{err}");
    disk.clear_faults().unwrap();
    disk.tx(|tx| Store::kv_set(tx, "s", "a", "b")).unwrap();
}

#[test]
fn a_v1_database_is_backed_up_before_adding_durable_retry_state() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("engine.sqlite");
    {
        let s = Store::open(tmp.path()).unwrap();
        s.tx(|tx| cli_agent::remove_schema_for_test(tx)).unwrap();
        session(&s, "saved");
        s.tx(|tx| {
            Store::admit(
                tx,
                "saved",
                "pending",
                InputMode::Prompt,
                "keep me",
                &[],
                None,
                1,
            )?;
            let run = Store::start_run(tx, "saved", Some("pending"), 1)?;
            Store::append_entry(tx, "saved", 1, Some(run), &user("keep me"))?;
            Ok(())
        })
        .unwrap();
        // Reconstruct the actual v1 schema, retaining a nonempty history/catalog.
        s.tx(|tx| {
            tx.execute_batch("DROP TABLE diagnostics; DROP TABLE jobs; DROP TABLE goal_events; DROP TABLE goals; ALTER TABLE subagent_tasks DROP COLUMN note; ALTER TABLE subagent_tasks DROP COLUMN delivery; ALTER TABLE subagent_tasks DROP COLUMN origin; ALTER TABLE subagent_tasks DROP COLUMN launch; DROP TABLE web_sources; DROP TABLE plan_receipts; DROP TABLE plan_events; DROP TABLE plan_tasks; DROP TABLE plans; DROP TABLE tool_call_ids; DROP TRIGGER event_session_delete; DROP TRIGGER removed_session_insert; DROP TABLE session_removals; DROP TRIGGER event_session_insert; DROP TRIGGER event_session_revision; DROP TRIGGER event_retention; DROP TABLE event_outbox; DROP TABLE external_permissions; DROP TABLE generation_state; DROP TABLE summary_writes; DROP TABLE tool_boundaries; DROP TABLE compactions; DROP TABLE external_projection_messages; DROP TABLE external_prompts; DROP TABLE external_actions; DROP TABLE external_runs; DROP TABLE cli_bindings; DROP TABLE cli_binding_receipts; DROP TRIGGER session_execution_immutable; DROP TRIGGER cli_session_shape_insert; DROP TRIGGER cli_session_shape_update; DROP TABLE cli_bridge_calls; DROP TABLE cli_tool_bridges; ALTER TABLE sessions DROP COLUMN execution_backend; DROP TABLE external_projection_actions; PRAGMA user_version = 1;")?;
            Ok(())
        })
        .unwrap();
    }
    let upgraded = Store::open(tmp.path()).unwrap();
    assert!(upgraded.session("saved").unwrap().is_some());
    assert_eq!(
        upgraded.request("saved", "pending").unwrap().unwrap().text,
        "keep me"
    );
    assert!(upgraded.open_run("saved").unwrap().is_some());
    assert_eq!(
        upgraded
            .read(|c| Store::entries_of_run(c, "saved", 1))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(upgraded.read(schema::version).unwrap(), SCHEMA_VERSION);
    assert!(
        upgraded
            .read(|c| Store::generation_state(c, 1))
            .unwrap()
            .handled_error
            .is_none()
    );
    let backup = Connection::open_with_flags(
        path.with_extension("sqlite.v1.bak"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(schema::version(&backup).unwrap(), 1);
    assert_eq!(
        backup
            .query_row("SELECT id FROM sessions", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "saved"
    );
    assert!(backup.prepare("SELECT * FROM generation_state").is_err());
    assert_eq!(
        backup
            .query_row("SELECT text FROM requests", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "keep me"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path.with_extension("sqlite.v1.bak"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn outbox_invalidation_is_atomic_with_state_and_rolls_back_on_failure() {
    let s = store();
    session(&s, "s");
    let cursor = s.catch_up(0, 256).unwrap().head;
    s.fail_next_write();
    assert!(
        s.set_session_field("s", SessionField::Title(Some("lost".into())))
            .is_err()
    );
    assert_eq!(s.catch_up(cursor, 256).unwrap().head, cursor);
    assert!(s.session("s").unwrap().unwrap().title.is_none());
    s.set_session_field("s", SessionField::Title(Some("saved".into())))
        .unwrap();
    let page = s.catch_up(cursor, 256).unwrap();
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.events[0].session, "s");
    assert_eq!(
        page.events[0].revision,
        s.session("s").unwrap().unwrap().revision
    );
    assert_eq!(page.through, cursor + 1);
}

#[test]
fn outbox_is_bounded_paged_and_survives_reopen_with_explicit_reset() {
    let tmp = tempfile::tempdir().unwrap();
    let s = Store::open(tmp.path()).unwrap();
    session(&s, "s");
    // One transaction exercises retention without 5000 fsyncs.
    s.tx(|tx| {
        for i in 0..5000 {
            Store::bump(tx, "s", i)?;
        }
        Ok(())
    })
    .unwrap();
    let old = s.catch_up(0, 256).unwrap();
    assert!(old.reset);
    assert_eq!(old.head, 5001);
    let page = s.catch_up(5001 - 4096, 9000).unwrap();
    assert!(!page.reset);
    assert_eq!(page.events.len(), 256);
    assert_eq!(page.events[0].cursor, 906);
    assert!(
        page.events
            .windows(2)
            .all(|w| w[1].cursor == w[0].cursor + 1)
    );
    assert_eq!(
        s.read(
            |c| Ok(c.query_row("SELECT COUNT(*) FROM event_outbox", [], |r| r
                .get::<_, i64>(0))?)
        )
        .unwrap(),
        4096
    );
    drop(s);
    let s = Store::open(tmp.path()).unwrap();
    assert_eq!(s.catch_up(5001 - 4096, 9000).unwrap(), page);
    assert!(s.catch_up(u64::MAX, 1).unwrap().reset);
    assert!(s.catch_up(5001, 1).unwrap().events.is_empty());
}

#[test]
fn a_v2_database_is_backed_up_before_adding_the_outbox() {
    let tmp = tempfile::tempdir().unwrap();
    {
        let s = Store::open(tmp.path()).unwrap();
        s.tx(|tx| cli_agent::remove_schema_for_test(tx)).unwrap();
        session(&s, "existing");
        s.tx(|tx| {
            tx.execute_batch("DROP TABLE diagnostics; DROP TABLE jobs; DROP TABLE goal_events; DROP TABLE goals; ALTER TABLE subagent_tasks DROP COLUMN note; ALTER TABLE subagent_tasks DROP COLUMN delivery; ALTER TABLE subagent_tasks DROP COLUMN origin; ALTER TABLE subagent_tasks DROP COLUMN launch; DROP TABLE web_sources; DROP TABLE plan_receipts; DROP TABLE plan_events; DROP TABLE plan_tasks; DROP TABLE plans; DROP TABLE tool_call_ids; DROP TRIGGER event_session_delete; DROP TRIGGER removed_session_insert; DROP TABLE session_removals; DROP TRIGGER event_session_insert; DROP TRIGGER event_session_revision; DROP TRIGGER event_retention; DROP TABLE event_outbox; DROP TABLE external_permissions; DROP TABLE summary_writes; DROP TABLE tool_boundaries; DROP TABLE compactions; DROP TABLE external_projection_messages; DROP TABLE external_prompts; DROP TABLE external_actions; DROP TABLE external_runs; DROP TABLE cli_bindings; DROP TABLE cli_binding_receipts; DROP TRIGGER session_execution_immutable; DROP TRIGGER cli_session_shape_insert; DROP TRIGGER cli_session_shape_update; DROP TABLE cli_bridge_calls; DROP TABLE cli_tool_bridges; ALTER TABLE sessions DROP COLUMN execution_backend; DROP TABLE external_projection_actions; PRAGMA user_version = 2;")?;
            Ok(())
        }).unwrap();
    }
    let s = Store::open(tmp.path()).unwrap();
    assert!(s.session("existing").unwrap().is_some());
    s.set_session_field("existing", SessionField::Title(Some("updated".into())))
        .unwrap();
    assert_eq!(s.catch_up(0, 10).unwrap().events.len(), 1);
    let backup = Connection::open_with_flags(
        tmp.path().join("engine.sqlite.v2.bak"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(schema::version(&backup).unwrap(), 2);
    assert_eq!(
        backup
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn a_v3_database_is_backed_up_before_positional_system_bodies_are_written() {
    let dir = tempfile::tempdir().unwrap();
    {
        let s = Store::open(dir.path()).unwrap();
        s.tx(|tx| cli_agent::remove_schema_for_test(tx)).unwrap();
        session(&s, "old");
        s.tx(|tx| {
            tx.execute_batch("DROP TABLE diagnostics; DROP TABLE jobs; DROP TABLE goal_events; DROP TABLE goals; ALTER TABLE subagent_tasks DROP COLUMN note; ALTER TABLE subagent_tasks DROP COLUMN delivery; ALTER TABLE subagent_tasks DROP COLUMN origin; ALTER TABLE subagent_tasks DROP COLUMN launch; DROP TABLE web_sources; DROP TABLE plan_receipts; DROP TABLE plan_events; DROP TABLE plan_tasks; DROP TABLE plans; DROP TABLE tool_call_ids; DROP TRIGGER event_session_delete; DROP TRIGGER removed_session_insert; DROP TABLE session_removals; DROP TABLE summary_writes; DROP TABLE tool_boundaries; DROP TABLE compactions; DROP TABLE external_projection_messages; DROP TABLE external_prompts; DROP TABLE external_actions; DROP TABLE external_runs; DROP TABLE cli_bindings; DROP TABLE cli_binding_receipts; DROP TRIGGER session_execution_immutable; DROP TRIGGER cli_session_shape_insert; DROP TRIGGER cli_session_shape_update; DROP TABLE cli_bridge_calls; DROP TABLE cli_tool_bridges; ALTER TABLE sessions DROP COLUMN execution_backend; DROP TABLE external_permissions; DROP TABLE external_projection_actions; PRAGMA user_version = 3")?;
            Ok(())
        })
        .unwrap();
    }
    let s = Store::open(dir.path()).unwrap();
    assert_eq!(s.read(schema::version).unwrap(), SCHEMA_VERSION);
    let backup = Connection::open_with_flags(
        dir.path().join("engine.sqlite.v3.bak"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(schema::version(&backup).unwrap(), 3);
    assert_eq!(
        backup
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get::<_, u64>(0))
            .unwrap(),
        1
    );
    assert!(s.session("old").unwrap().is_some());
}

#[cfg(unix)]
#[test]
fn closing_a_store_releases_ownership_even_when_a_child_retains_its_descriptor() {
    let dir = tempfile::tempdir().unwrap();
    let original = Store::open(dir.path()).unwrap();
    // dup and fork retain the same open-file description used by flock. Keep that
    // descriptor alive to reproduce the interval between another thread's fork and exec.
    let inherited = original._lock.as_ref().unwrap()._file.try_clone().unwrap();
    drop(original);
    let reopened = Store::open(dir.path()).unwrap();
    drop(inherited);
    assert!(
        matches!(Store::open(dir.path()), Err(StoreError::Locked(_))),
        "dropping the old child descriptor must not unlock the new store"
    );
    drop(reopened);
    Store::open(dir.path()).unwrap();
}

#[test]
fn a_v4_database_is_backed_up_before_pinned_extension_profiles_are_written() {
    let dir = tempfile::tempdir().unwrap();
    {
        let s = Store::open(dir.path()).unwrap();
        s.tx(|tx| cli_agent::remove_schema_for_test(tx)).unwrap();
        session(&s, "existing");
        s.tx(|tx| {
            Store::kv_set(tx, "existing", "opaque-state", "retained bytes")?;
            tx.execute_batch("DROP TABLE diagnostics; DROP TABLE jobs; DROP TABLE goal_events; DROP TABLE goals; ALTER TABLE subagent_tasks DROP COLUMN note; ALTER TABLE subagent_tasks DROP COLUMN delivery; ALTER TABLE subagent_tasks DROP COLUMN origin; ALTER TABLE subagent_tasks DROP COLUMN launch; DROP TABLE web_sources; DROP TABLE plan_receipts; DROP TABLE plan_events; DROP TABLE plan_tasks; DROP TABLE plans; DROP TABLE tool_call_ids; DROP TRIGGER event_session_delete; DROP TRIGGER removed_session_insert; DROP TABLE session_removals; DROP TABLE summary_writes; DROP TABLE tool_boundaries; DROP TABLE compactions; DROP TABLE external_projection_messages; DROP TABLE external_prompts; DROP TABLE external_actions; DROP TABLE external_runs; DROP TABLE cli_bindings; DROP TABLE cli_binding_receipts; DROP TRIGGER session_execution_immutable; DROP TRIGGER cli_session_shape_insert; DROP TRIGGER cli_session_shape_update; DROP TABLE cli_bridge_calls; DROP TABLE cli_tool_bridges; ALTER TABLE sessions DROP COLUMN execution_backend; DROP TABLE external_permissions; DROP TABLE external_projection_actions; PRAGMA user_version = 4")?;
            Ok(())
        })
        .unwrap();
    }
    let s = Store::open(dir.path()).unwrap();
    assert_eq!(s.read(schema::version).unwrap(), SCHEMA_VERSION);
    assert_eq!(
        s.kv_get("existing", "opaque-state").unwrap().as_deref(),
        Some("retained bytes")
    );
    crate::extensions::profiles::pin(&s, "existing").unwrap();
    let backup = Connection::open_with_flags(
        dir.path().join("engine.sqlite.v4.bak"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(schema::version(&backup).unwrap(), 4);
    assert_eq!(
        backup
            .query_row("SELECT value FROM kv WHERE key = 'opaque-state'", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "retained bytes"
    );
    assert_eq!(
        backup
            .query_row(
                "SELECT COUNT(*) FROM kv WHERE key = ?1",
                [crate::extensions::profiles::KEY],
                |r| r.get::<_, u64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn a_v8_database_is_backed_up_before_removal_receipts_and_delete_events() {
    let dir = tempfile::tempdir().unwrap();
    {
        let s = Store::open(dir.path()).unwrap();
        s.tx(|tx| cli_agent::remove_schema_for_test(tx)).unwrap();
        session(&s, "existing");
        s.tx(|tx| {
            Store::kv_set(tx, "existing", "opaque-state", "retained bytes")?;
            tx.execute_batch("DROP TABLE diagnostics; DROP TABLE jobs; DROP TABLE goal_events; DROP TABLE goals; ALTER TABLE subagent_tasks DROP COLUMN note; ALTER TABLE subagent_tasks DROP COLUMN delivery; ALTER TABLE subagent_tasks DROP COLUMN origin; ALTER TABLE subagent_tasks DROP COLUMN launch; DROP TABLE web_sources; DROP TABLE plan_receipts; DROP TABLE plan_events; DROP TABLE plan_tasks; DROP TABLE plans; DROP TABLE tool_call_ids; DROP TRIGGER event_session_delete; DROP TRIGGER removed_session_insert; DROP TABLE session_removals; DROP TABLE external_projection_messages; DROP TABLE external_prompts; DROP TABLE external_actions; DROP TABLE external_runs; DROP TABLE cli_bindings; DROP TABLE cli_binding_receipts; DROP TRIGGER session_execution_immutable; DROP TRIGGER cli_session_shape_insert; DROP TRIGGER cli_session_shape_update; DROP TABLE cli_bridge_calls; DROP TABLE cli_tool_bridges; ALTER TABLE sessions DROP COLUMN execution_backend; DROP TABLE external_permissions; DROP TABLE external_projection_actions; PRAGMA user_version = 8")?;
            Ok(())
        }).unwrap();
    }
    let s = Store::open(dir.path()).unwrap();
    assert_eq!(s.read(schema::version).unwrap(), SCHEMA_VERSION);
    assert_eq!(
        s.kv_get("existing", "opaque-state").unwrap().as_deref(),
        Some("retained bytes")
    );
    s.begin_removal("existing").unwrap();
    s.finish_removal("existing").unwrap();
    assert!(s.session("existing").unwrap().is_none());
    let backup = Connection::open_with_flags(
        dir.path().join("engine.sqlite.v8.bak"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(schema::version(&backup).unwrap(), 8);
    assert_eq!(
        backup
            .query_row(
                "SELECT value FROM kv WHERE session='existing' AND key='opaque-state'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "retained bytes"
    );
    assert_eq!(
        s.catch_up(0, 10).unwrap().events.last().unwrap().session,
        "existing"
    );
    s.integrity_check().unwrap();
}

#[test]
fn v9_tool_history_is_backed_up_and_preserved_before_invocation_aliases() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("engine.sqlite");
    let original;
    {
        let s = Store::open(tmp.path()).unwrap();
        s.tx(|tx| cli_agent::remove_schema_for_test(tx)).unwrap();
        session(&s, "s");
        s.tx(|tx| {
            Store::append_entry(tx, "s", 1, None, &user("saved history"))?;
            Store::tool_intent(tx, "s", "provider-id", None, "bash", &json!({}), false, 1)?;
            Store::tool_result(
                tx,
                "s",
                "provider-id",
                ToolState::Done,
                1,
                "saved output",
                0,
                2,
            )?;
            Store::append_entry(
                tx,
                "s",
                2,
                None,
                &EntryKind::Message {
                    message: Message::Assistant {
                        content: vec![Block::tool_call("unexecuted", "bash", json!({}))],
                        api: "x".into(),
                        provider: "x".into(),
                        model: "x".into(),
                        usage: Usage::default(),
                        stop_reason: StopReason::ToolUse,
                        error_message: None,
                        timestamp: 2,
                        extra: Default::default(),
                    },
                    mode: InputMode::Prompt,
                    request: None,
                    partial: false,
                },
            )?;
            // Reconstruct V9 before reopening; its call index is populated by the migration.
            tx.execute_batch("DROP TABLE diagnostics; DROP TABLE jobs; DROP TABLE goal_events; DROP TABLE goals; ALTER TABLE subagent_tasks DROP COLUMN note; ALTER TABLE subagent_tasks DROP COLUMN delivery; ALTER TABLE subagent_tasks DROP COLUMN origin; ALTER TABLE subagent_tasks DROP COLUMN launch; DROP TABLE web_sources; DROP TABLE plan_receipts; DROP TABLE plan_events; DROP TABLE plan_tasks; DROP TABLE plans; DROP TABLE tool_call_ids; DROP TABLE external_projection_messages; DROP TABLE external_prompts; DROP TABLE external_actions; DROP TABLE external_runs; DROP TABLE cli_bindings; DROP TABLE cli_binding_receipts; DROP TRIGGER session_execution_immutable; DROP TRIGGER cli_session_shape_insert; DROP TRIGGER cli_session_shape_update; DROP TABLE cli_bridge_calls; DROP TABLE cli_tool_bridges; ALTER TABLE sessions DROP COLUMN execution_backend; DROP TABLE external_permissions; DROP TABLE external_projection_actions; PRAGMA user_version=9")?;
            Ok(())
        })
        .unwrap();
        original = s.read(|c| Store::entry_in(c, "s", 1)).unwrap();
    }
    let s = Store::open(tmp.path()).unwrap();
    assert_eq!(s.read(schema::version).unwrap(), schema::SCHEMA_VERSION);
    assert_eq!(s.read(|c| Store::entry_in(c, "s", 1)).unwrap(), original);
    assert_eq!(
        s.tool_output("s", "provider-id").unwrap().unwrap().text,
        "saved output"
    );
    // Even an imported/unexecuted historical call reserves its id, without an intent row.
    assert!(s.tool_call("s", "unexecuted").unwrap().is_none());
    let mut later = match s
        .read(|c| Store::entry_in(c, "s", 2))
        .unwrap()
        .unwrap()
        .kind
    {
        EntryKind::Message { message, .. } => message,
        _ => unreachable!(),
    };
    s.tx(|tx| Store::disambiguate_tool_ids(tx, "s", &mut later))
        .unwrap();
    let (id, _, _) = later.tool_calls()[0];
    assert_ne!(id, "unexecuted");
    let backup = Connection::open_with_flags(
        path.with_extension("sqlite.v9.bak"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(schema::version(&backup).unwrap(), 9);
    assert_eq!(Store::entry_in(&backup, "s", 1).unwrap(), original);
    assert_eq!(
        backup
            .query_row(
                "SELECT text FROM tool_outputs WHERE session='s' AND call_id='provider-id'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "saved output"
    );
}
