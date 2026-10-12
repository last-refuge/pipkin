use super::*;
use crate::provider::scripted::{Gate, Reply as R, Scripted};
use crate::runtime::tests::Fixture;
use serde_json::json;

fn call(conn: &crate::host::Connection, call: Call) -> Result<Reply, ProtoError> {
    conn.send(ClientFrame {
        id: 1,
        lane: call.lane(),
        call,
    })
    .unwrap();
    loop {
        match conn.recv_timeout(Duration::from_secs(15)).unwrap() {
            ServerFrame::Reply { id: 1, result } => return result,
            ServerFrame::Event { .. } => {}
            other => panic!("{other:?}"),
        }
    }
}

fn wait(mut predicate: impl FnMut() -> bool) {
    let until = std::time::Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(
            std::time::Instant::now() < until,
            "condition did not become true"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn removal_deletes_a_root_and_children_but_preserves_project_other_sessions_and_backups() {
    let fx = Fixture::new();
    let provider = Scripted::new(|_, n| {
        if n == 0 {
            R::Tools(
                None,
                vec![
                    (
                        "write".into(),
                        json!({"path":"keep.txt","content":"project effect"}),
                    ),
                    ("subagent".into(), json!({"task":"inspect"})),
                ],
            )
        } else {
            R::Text("done".into())
        }
    });
    let mut e = fx.start(&provider);
    let root = e.create(&fx.project, "root-key");
    let other = e.create(&fx.project, "other-key");
    e.snapshot(&root);
    e.prompt(&root, "work", "delegate and write").unwrap();
    assert_eq!(e.settled("work"), RunOutcome::Completed);
    let child = e.handle.shared().store.tasks(&root, 10).unwrap().0[0]
        .child_session
        .clone();
    assert_eq!(
        e.call(Call::RemoveSession {
            session: child.clone()
        })
        .unwrap_err()
        .code,
        ErrorCode::Invalid
    );
    let backup = fx.dir.path().join("preserved.sqlite");
    e.handle.shared().store.backup_to(&backup).unwrap();
    let backup_bytes = std::fs::read(&backup).unwrap();
    let cursor = e.handle.shared().store.catch_up(0, 256).unwrap().head;
    assert_eq!(
        e.call(Call::RemoveSession {
            session: root.clone()
        })
        .unwrap(),
        Reply::Ok
    );
    for id in [&root, &child] {
        assert!(e.handle.shared().store.session(id).unwrap().is_none());
        assert_eq!(
            e.call(Call::OpenSession {
                session: id.clone(),
                window: None
            })
            .unwrap_err()
            .code,
            ErrorCode::NotFound
        );
        assert!(
            !e.handle
                .shared()
                .sessions
                .lock()
                .unwrap()
                .live
                .contains_key(id)
        );
    }
    let changes = e.handle.shared().store.catch_up(cursor, 256).unwrap();
    assert!(changes.events.iter().any(|event| event.session == root));
    assert!(changes.events.iter().any(|event| event.session == child));
    assert_eq!(
        std::fs::read(fx.project.join("keep.txt")).unwrap(),
        b"project effect"
    );
    assert_eq!(std::fs::read(&backup).unwrap(), backup_bytes);
    assert_eq!(
        e.call(Call::RemoveSession {
            session: root.clone()
        })
        .unwrap(),
        Reply::Ok
    );
    assert_eq!(
        e.call(Call::CreateSession {
            cwd: fx.project.display().to_string(),
            request: "root-key".into()
        })
        .unwrap_err()
        .code,
        ErrorCode::NotFound
    );
    e.prompt(&other, "other-work", "still usable").unwrap();
    assert_eq!(e.settled("other-work"), RunOutcome::Completed);
    e.handle.shared().store.integrity_check().unwrap();
    e.handle.shutdown(Duration::from_secs(1));
    drop(e);
    let mut e = fx.start(&Scripted::echo("after restart"));
    assert_eq!(
        e.call(Call::RemoveSession {
            session: root.clone()
        })
        .unwrap(),
        Reply::Ok
    );
    let Reply::Sessions { sessions } = e.call(Call::ListSessions).unwrap() else {
        panic!()
    };
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, other);
}

#[test]
fn removal_cancels_a_live_child_queue_and_waiter_and_repeated_clients_join_cleanup() {
    let fx = Fixture::new();
    let gate = Gate::new();
    let g = gate.clone();
    let provider = Scripted::new(move |req, _| {
        if req.tools.iter().any(|t| t.name == "subagent") {
            R::Tools(None, vec![("subagent".into(), json!({"task":"wait"}))])
        } else {
            R::Gated(g.clone(), Box::new(R::Text("late".into())))
        }
    });
    let mut e = fx.start(&provider);
    let root = e.create(&fx.project, "root");
    e.prompt(&root, "work", "delegate").unwrap();
    wait(|| provider.requests().len() == 2);
    e.call(Call::Enqueue {
        session: root.clone(),
        request: "queued".into(),
        mode: InputMode::FollowUp,
        text: "must not run".into(),
        images: vec![],
    })
    .unwrap();
    let waiter = e.handle.connect("waiter");
    waiter
        .send(ClientFrame {
            id: 1,
            lane: Lane::Normal,
            call: Call::WaitForPrompt {
                session: root.clone(),
                request: "queued".into(),
            },
        })
        .unwrap();
    let clients: Vec<_> = (0..6).map(|_| e.handle.connect("remover")).collect();
    std::thread::scope(|scope| {
        for client in clients {
            let root = &root;
            scope.spawn(move || {
                assert_eq!(
                    call(
                        &client,
                        Call::RemoveSession {
                            session: root.clone()
                        }
                    )
                    .unwrap(),
                    Reply::Ok
                )
            });
        }
    });
    loop {
        match waiter.recv_timeout(Duration::from_secs(2)).unwrap() {
            ServerFrame::Reply {
                id: 1,
                result: Err(e),
            } => {
                assert_eq!(e.code, ErrorCode::NotFound);
                break;
            }
            ServerFrame::Event { .. } => {}
            other => panic!("{other:?}"),
        }
    }
    gate.open();
    assert_eq!(provider.requests().len(), 2);
    assert!(e.handle.shared().store.all_sessions().unwrap().is_empty());
}

#[test]
fn a_failed_removal_intent_keeps_the_live_run_and_its_history() {
    let fx = Fixture::new();
    let gate = Gate::new();
    let g = gate.clone();
    let provider = Scripted::new(move |_, _| R::Gated(g.clone(), Box::new(R::Text("kept".into()))));
    let mut e = fx.start(&provider);
    let root = e.create(&fx.project, "root");
    e.prompt(&root, "work", "wait").unwrap();
    wait(|| provider.requests().len() == 1);
    e.handle.shared().store.fail_next_write();
    assert_eq!(
        e.call(Call::RemoveSession {
            session: root.clone()
        })
        .unwrap_err()
        .code,
        ErrorCode::Storage
    );
    assert_eq!(e.handle.shared().store.removal_state(&root).unwrap(), None);
    gate.open();
    assert_eq!(e.settled("work"), RunOutcome::Completed);
    assert!(!e.snapshot(&root).entries.is_empty());
}

#[test]
fn committed_removal_outlives_its_caller_and_restart_never_resumes_the_deleted_run() {
    let fx = Fixture::new();
    let gate = Gate::new();
    let g = gate.clone();
    let provider = Scripted::new(move |_, _| R::Gated(g.clone(), Box::new(R::Text("late".into()))));
    let mut e = fx.start(&provider);
    let root = e.create(&fx.project, "root");
    e.prompt(&root, "work", "wait").unwrap();
    wait(|| provider.requests().len() == 1);
    let demand = e.handle.shared().session(&root).unwrap();
    let remover = e.handle.connect("disappears");
    remover
        .send(ClientFrame {
            id: 1,
            lane: Lane::Normal,
            call: Call::RemoveSession {
                session: root.clone(),
            },
        })
        .unwrap();
    wait(|| e.handle.shared().store.removal_state(&root).unwrap() == Some(false));
    drop(remover);
    assert_eq!(
        e.prompt(&root, "new", "blocked").unwrap_err().code,
        ErrorCode::NotFound
    );
    e.handle.shutdown(Duration::from_millis(100));
    drop(demand);
    drop(e);
    let restarted = Scripted::echo("must not be called");
    let mut e = fx.start(&restarted);
    assert_eq!(
        e.call(Call::RemoveSession { session: root }).unwrap(),
        Reply::Ok
    );
    assert!(e.handle.shared().store.all_sessions().unwrap().is_empty());
    assert!(restarted.requests().is_empty());
}

#[test]
fn cleanup_write_failure_rolls_back_all_records_and_invalidation() {
    let fx = Fixture::new();
    let mut e = fx.start(&Scripted::echo("done"));
    let root = e.create(&fx.project, "root");
    e.prompt(&root, "work", "saved").unwrap();
    e.settled("work");
    let store = &e.handle.shared().store;
    store.begin_removal(&root).unwrap();
    let before = store.session(&root).unwrap();
    let cursor = store.catch_up(0, 256).unwrap().head;
    store.fail_next_write();
    assert!(store.finish_removal(&root).is_err());
    assert_eq!(store.session(&root).unwrap(), before);
    assert_eq!(store.removal_state(&root).unwrap(), Some(false));
    assert_eq!(store.catch_up(cursor, 256).unwrap().head, cursor);
    store.integrity_check().unwrap();
    store.finish_removal(&root).unwrap();
    assert!(store.session(&root).unwrap().is_none());
    assert_eq!(store.catch_up(cursor, 256).unwrap().events.len(), 1);
}

#[test]
fn removal_capacity_refuses_before_intent_and_recovers_after_demands_drop() {
    let fx = Fixture::new();
    let mut e = fx.start(&Scripted::echo("unused"));
    let roots: Vec<_> = (0..9)
        .map(|n| e.create(&fx.project, &format!("root{n}")))
        .collect();
    let demands: Vec<_> = roots
        .iter()
        .map(|id| e.handle.shared().session(id).unwrap())
        .collect();
    let clients: Vec<_> = (0..8).map(|_| e.handle.connect("remover")).collect();
    for (client, root) in clients.iter().zip(&roots) {
        client
            .send(ClientFrame {
                id: 1,
                lane: Lane::Normal,
                call: Call::RemoveSession {
                    session: root.clone(),
                },
            })
            .unwrap();
    }
    wait(|| e.handle.shared().store.pending_removals().unwrap().len() == 8);
    assert_eq!(
        e.call(Call::RemoveSession {
            session: roots[8].clone()
        })
        .unwrap_err()
        .code,
        ErrorCode::Busy
    );
    assert_eq!(
        e.handle.shared().store.removal_state(&roots[8]).unwrap(),
        None
    );
    assert!(e.handle.shared().session(&roots[8]).is_ok());
    drop(demands);
    for root in &roots[..8] {
        wait(|| e.handle.shared().store.removal_state(root).unwrap() == Some(true));
    }
    assert_eq!(
        e.call(Call::RemoveSession {
            session: roots[8].clone()
        })
        .unwrap(),
        Reply::Ok
    );
    assert!(e.handle.shared().store.all_sessions().unwrap().is_empty());
}

#[test]
fn removal_cancels_concurrent_compactions_and_drops_their_durable_jobs() {
    use crate::context::compaction::SUMMARIZATION_SYSTEM_PROMPT;
    let fx = Fixture::new();
    std::fs::create_dir_all(fx.dir.path().join("config")).unwrap();
    std::fs::write(
        fx.dir.path().join("config/settings.json"),
        r#"{"compaction":{"enabled":false,"keepRecentTokens":5,"reserveTokens":1000}}"#,
    )
    .unwrap();
    let gate = Gate::new();
    let g = gate.clone();
    let provider = Scripted::new(move |req, _| {
        if req.system == SUMMARIZATION_SYSTEM_PROMPT {
            R::Gated(g.clone(), Box::new(R::Text("late summary".into())))
        } else {
            R::Text("answer".into())
        }
    });
    let mut e = fx.start(&provider);
    let root = e.create(&fx.project, "root");
    for n in 0..4 {
        let key = format!("seed{n}");
        e.prompt(&root, &key, "earlier conversation with several words")
            .unwrap();
        e.settled(&key);
    }
    let clients: Vec<_> = (0..2).map(|_| e.handle.connect("compact")).collect();
    for client in &clients {
        client
            .send(ClientFrame {
                id: 1,
                lane: Lane::Normal,
                call: Call::Compact {
                    session: root.clone(),
                    instructions: None,
                },
            })
            .unwrap();
    }
    e.handle
        .shared()
        .store
        .tx(|tx| {
            Store::admit_compaction(
                tx,
                &root,
                "background-removal",
                None,
                crate::store::compactions::CompactionScope::Background,
                None,
            )
        })
        .unwrap();
    e.handle.shared().compaction_recovery.notify_one();
    wait(|| {
        provider
            .requests()
            .iter()
            .filter(|r| r.system == SUMMARIZATION_SYSTEM_PROMPT)
            .count()
            == 3
    });
    assert_eq!(
        e.call(Call::RemoveSession {
            session: root.clone()
        })
        .unwrap(),
        Reply::Ok
    );
    for client in clients {
        loop {
            match client.recv_timeout(Duration::from_secs(2)).unwrap() {
                ServerFrame::Reply {
                    result: Err(error), ..
                } => {
                    assert!(error.message.contains("removal"), "{error:?}");
                    break;
                }
                ServerFrame::Event { .. } => {}
                frame => panic!("{frame:?}"),
            }
        }
    }
    gate.open();
    let jobs: u64 = e
        .handle
        .shared()
        .store
        .read(|c| Ok(c.query_row("SELECT COUNT(*) FROM compactions", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(jobs, 0);
    assert!(e.handle.shared().store.session(&root).unwrap().is_none());
}
