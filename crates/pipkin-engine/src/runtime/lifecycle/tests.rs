use super::*;
use crate::host::Connection;
use crate::provider::scripted::{Gate, Reply as R, Scripted};
use crate::provider::{ApiAdapter, BoxFuture, StreamCtx};
use crate::runtime::tests::Fixture;
use pipkin_engine_protocol::*;
use std::sync::Mutex;

fn call(conn: &Connection, call: Call) -> Result<Reply, ProtoError> {
    conn.send(ClientFrame {
        id: 991,
        lane: call.lane(),
        call,
    })
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let ServerFrame::Reply { id: 991, result } = conn
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap()
        {
            return result;
        }
    }
}
fn wait(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out: {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn live(shared: &Shared, id: &str) -> bool {
    shared.sessions.lock().unwrap().live.contains_key(id)
}
fn reap(shared: &Shared) {
    shared.retire_idle(Instant::now() + Duration::from_secs(60));
}

struct Closing(Arc<Mutex<Vec<String>>>);
impl ApiAdapter for Closing {
    fn api(&self) -> &'static str {
        "closing-test"
    }
    fn stream<'a>(&'a self, _: StreamCtx<'a>) -> BoxFuture<'a, Message> {
        unreachable!()
    }
    fn close_sessions(&self, id: Option<&str>) {
        if let Some(id) = id {
            self.0.lock().unwrap().push(id.to_owned());
        }
    }
}

#[test]
fn prompt_resources_follow_the_live_root_group_and_are_freed_after_retirement() {
    let fx = Fixture::new();
    let instructions = fx.project.join("AGENTS.md");
    std::fs::write(&instructions, "Original lifetime instruction.").unwrap();
    let provider = Scripted::new(|req, _| {
        if req.tools.iter().any(|tool| tool.name == "subagent")
            && matches!(req.messages.iter().rev().find(|m| !matches!(m, Message::System(_))), Some(m @ Message::User { .. }) if m.text() == "delegate")
        {
            R::Tools(
                None,
                vec![("subagent".into(), serde_json::json!({"task":"inspect"}))],
            )
        } else {
            R::Text("done".into())
        }
    });
    let mut e = fx.start(&provider);
    let root = e.create(&fx.project, "root");
    e.prompt(&root, "delegate", "delegate").unwrap();
    assert_eq!(e.settled("delegate"), RunOutcome::Completed);
    let child_id = e.handle.shared.store.tasks(&root, 1).unwrap().0[0]
        .child_session
        .clone();
    let child = e.handle.shared.session(&child_id).unwrap();
    let resource_lifetime = Arc::downgrade(&child.prompt_resources);
    std::fs::write(&instructions, "New lifetime instruction.").unwrap();

    // Retiring the root alone must not change what its still-live child sees.
    wait("root retires while child retains the group", || {
        reap(&e.handle.shared);
        !live(&e.handle.shared, &root)
    });
    assert!(live(&e.handle.shared, &child_id));
    let reopened = e.handle.shared.session(&root).unwrap();
    assert!(Arc::ptr_eq(
        &reopened.prompt_resources,
        &child.prompt_resources
    ));
    drop(reopened);
    e.prompt(&root, "warm", "continue").unwrap();
    assert_eq!(e.settled("warm"), RunOutcome::Completed);
    let requests = provider.requests();
    let warm = requests.last().unwrap().resolve_systems(false);
    assert!(warm.system.contains("Original lifetime instruction."));
    assert!(!warm.system.contains("New lifetime instruction."));

    drop(child);
    wait("entire root group retires", || {
        reap(&e.handle.shared);
        !live(&e.handle.shared, &root) && !live(&e.handle.shared, &child_id)
    });
    assert!(
        e.handle
            .shared
            .sessions
            .lock()
            .unwrap()
            .prompt_resources
            .is_empty()
    );
    e.prompt(&root, "cold", "new lifetime").unwrap();
    assert_eq!(e.settled("cold"), RunOutcome::Completed);
    let requests = provider.requests();
    let cold = requests.last().unwrap().resolve_systems(false);
    assert!(cold.system.contains("New lifetime instruction."));
    assert!(!cold.system.contains("Original lifetime instruction."));
    wait("retired resource allocations are released", || {
        resource_lifetime.upgrade().is_none()
    });
}

#[test]
fn attachments_are_client_owned_and_retirement_closes_the_provider_affinity_id() {
    let fx = Fixture::new();
    let provider = Scripted::echo("done");
    let mut e = fx.start(&provider);
    let closed = Arc::new(Mutex::new(Vec::new()));
    // Install before use through a fresh registry, rather than mutate a running engine.
    let mut registry = super::super::registry_with(Arc::new(provider.clone()));
    registry.install(Arc::new(Closing(closed.clone())));
    let h = super::super::EngineHandle::start_with(
        {
            let mut c = fx.config();
            c.data_dir = fx.dir.path().join("second-data");
            c
        },
        registry,
        crate::tools::builtin_tools(),
    )
    .unwrap();
    let a = h.connect("a");
    let b = h.connect("b");
    let id = match call(
        &a,
        Call::CreateSession {
            cwd: fx.project.display().to_string(),
            request: "c".into(),
        },
    )
    .unwrap()
    {
        Reply::Session { summary } => summary.id,
        other => panic!("{other:?}"),
    };
    h.shared
        .store
        .tx(|tx| crate::store::Store::kv_set(tx, &id, "provider_session_id", "provider-affinity"))
        .unwrap();
    for c in [&a, &a, &b] {
        call(
            c,
            Call::OpenSession {
                session: id.clone(),
                window: None,
            },
        )
        .unwrap();
    }
    call(
        &a,
        Call::ReleaseSession {
            session: id.clone(),
        },
    )
    .unwrap();
    call(
        &a,
        Call::ReleaseSession {
            session: id.clone(),
        },
    )
    .unwrap();
    reap(&h.shared);
    assert!(live(&h.shared, &id));
    assert!(closed.lock().unwrap().is_empty());
    call(
        &b,
        Call::ReleaseSession {
            session: id.clone(),
        },
    )
    .unwrap();
    wait("last attachment retires", || {
        reap(&h.shared);
        !live(&h.shared, &id)
    });
    assert_eq!(*closed.lock().unwrap(), ["provider-affinity"]);
    let snapshot = call(
        &a,
        Call::OpenSession {
            session: id.clone(),
            window: None,
        },
    )
    .unwrap();
    assert!(matches!(snapshot, Reply::Snapshot(_)));
    assert!(live(&h.shared, &id));
    assert_eq!(
        h.shared
            .store
            .kv_get(&id, "provider_session_id")
            .unwrap()
            .as_deref(),
        Some("provider-affinity")
    );
    // Protocol behavior is independently checked through the actual engine.
    assert_eq!(
        e.call(Call::Hello {
            protocol: PROTOCOL_VERSION - 1,
            client: "old".into()
        })
        .unwrap_err()
        .code,
        ErrorCode::Incompatible
    );
}

#[test]
fn detach_keeps_the_active_run_and_follow_up_until_both_settle() {
    let fx = Fixture::new();
    let first = Gate::new();
    let second = Gate::new();
    let (g1, g2) = (first.clone(), second.clone());
    let provider = Scripted::new(move |_, n| {
        R::Gated(
            if n == 0 { g1.clone() } else { g2.clone() },
            Box::new(R::Text("done".into())),
        )
    });
    let mut e = fx.start(&provider);
    let id = e.create(&fx.project, "c");
    e.snapshot(&id);
    e.prompt(&id, "first", "work").unwrap();
    e.call(Call::Enqueue {
        session: id.clone(),
        request: "second".into(),
        mode: InputMode::FollowUp,
        text: "more work".into(),
        images: vec![],
    })
    .unwrap();
    e.call(Call::ReleaseSession {
        session: id.clone(),
    })
    .unwrap();
    wait("provider started", || !provider.requests().is_empty());
    reap(&e.handle.shared);
    assert!(live(&e.handle.shared, &id));
    first.open();
    e.settled("first");
    wait("follow-up started", || provider.requests().len() == 2);
    reap(&e.handle.shared);
    assert!(live(&e.handle.shared, &id));
    second.open();
    e.settled("second");
    wait("settled session retires", || {
        reap(&e.handle.shared);
        !live(&e.handle.shared, &id)
    });
    let s = e.snapshot(&id);
    assert!(s.entries.iter().any(
        |e| matches!(&e.kind, EntryKind::Message { message, .. } if message.text()=="more work")
    ));
    assert_eq!(provider.requests().len(), 2);
}

#[test]
fn disconnect_grace_can_be_readopted_and_cloned_senders_hold_the_client() {
    let fx = Fixture::new();
    let mut e = fx.start(&Scripted::echo("done"));
    let id = e.create(&fx.project, "c");
    let conn = e.handle.connect("split");
    let client = conn.client_id();
    call(
        &conn,
        Call::OpenSession {
            session: id.clone(),
            window: None,
        },
    )
    .unwrap();
    let (sender, receiver) = conn.split();
    let clone = sender.clone();
    drop(sender);
    reap(&e.handle.shared);
    assert!(
        e.handle.shared.sessions.lock().unwrap().live[&id]
            .clients
            .contains(&client)
    );
    drop(clone);
    wait("client detached", || {
        !e.handle.shared.sessions.lock().unwrap().live[&id]
            .clients
            .contains(&client)
    });
    // Use the actual wall clock here: disconnect has a warm-reconnect grace.
    e.handle.shared.retire_idle(Instant::now());
    assert!(live(&e.handle.shared, &id));
    e.snapshot(&id);
    reap(&e.handle.shared);
    assert!(live(&e.handle.shared, &id));
    e.call(Call::ReleaseSession {
        session: id.clone(),
    })
    .unwrap();
    wait("readopted attachment released", || {
        reap(&e.handle.shared);
        !live(&e.handle.shared, &id)
    });
    drop(receiver);
}

#[test]
fn full_reply_queue_does_not_block_disconnect_or_leave_late_open_attachments() {
    let fx = Fixture::new();
    let mut e = fx.start(&Scripted::echo("done"));
    let id = e.create(&fx.project, "c");
    let conn = e.handle.connect("slow");
    let client = conn.client_id();
    let (sender, receiver) = conn.split();
    for n in 0..64 {
        sender
            .send(ClientFrame {
                id: n,
                lane: Lane::Normal,
                call: Call::OpenSession {
                    session: id.clone(),
                    window: Some(1),
                },
            })
            .unwrap();
    }
    drop(sender); // Receiver deliberately stays alive and never drains the bounded replies.
    wait("accepted calls finish and detach", || {
        !e.handle
            .shared
            .clients
            .lock()
            .unwrap()
            .contains_key(&client)
    });
    assert!(
        e.handle
            .shared
            .sessions
            .lock()
            .unwrap()
            .live
            .values()
            .all(|e| !e.clients.contains(&client))
    );
    wait("no late OpenSession recreates ownership", || {
        reap(&e.handle.shared);
        !live(&e.handle.shared, &id)
    });
    drop(receiver);
}

#[test]
fn initial_grace_and_pending_extension_calls_hold_retirement() {
    let fx = Fixture::new();
    let mut e = fx.start(&Scripted::echo("done"));
    let id = e.create(&fx.project, "c");
    let rt = e.handle.shared.session(&id).unwrap();
    let dispatch = e.handle.shared.extension_dispatch(&rt);
    let weak = Arc::downgrade(&rt);
    drop(rt);
    e.handle.shared.retire_idle(Instant::now());
    assert!(weak.upgrade().is_some(), "initial demand grace retained");
    // Obtaining a future already holds demand, before its first poll.
    let pending = dispatch(Call::HistoryPage {
        session: id.clone(),
        before: None,
        limit: 10,
    });
    reap(&e.handle.shared);
    assert!(weak.upgrade().is_some());
    drop(pending);
    wait("unused session retires", || {
        reap(&e.handle.shared);
        !live(&e.handle.shared, &id)
    });
    e.snapshot(&id);
    let stale = e
        .handle
        .shared
        .handle
        .block_on(dispatch(Call::Prompt {
            session: id.clone(),
            request: "stale".into(),
            text: "must not execute".into(),
            images: vec![],
            model: None,
        }))
        .unwrap_err();
    assert_eq!(stale.code, ErrorCode::ShuttingDown);
    assert!(
        e.handle
            .shared
            .store
            .request(&id, "stale")
            .unwrap()
            .is_none()
    );
}

struct SlowExtension {
    deactivated: Arc<std::sync::atomic::AtomicBool>,
    finish: CancellationToken,
}
impl crate::extensions::Extension for SlowExtension {
    fn id(&self) -> &str {
        "slow-close"
    }
    fn activate(&self, _: Arc<dyn crate::extensions::ExtensionHost>) -> BoxFuture<'static, ()> {
        let finish = self.finish.clone();
        Box::pin(async move { finish.cancelled().await })
    }
    fn deactivate(&self) {
        self.deactivated
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[test]
fn reopening_waits_for_extension_teardown_and_rejects_stale_ui_and_calls() {
    let fx = Fixture::new();
    let mut config = fx.config();
    config.extensions = vec![crate::config::ExtensionSpec::Builtin("questions".into())];
    let mut e = fx.start_config(&Scripted::echo("done"), config);
    let id = e.create(&fx.project, "c");
    let rt = e.handle.shared.session(&id).unwrap();
    let old_questions = rt.questions.clone();
    let dispatch = e.handle.shared.extension_dispatch(&rt);
    let host = Arc::new(crate::extensions::CallHost::new(
        id.clone(),
        old_questions.clone(),
        dispatch.clone(),
    ));
    let deactivated = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let finish = CancellationToken::new();
    e.handle.shared.handle.block_on(async {
        let mut runner = crate::extensions::ExtensionRunner::new(host);
        runner
            .start(Arc::new(SlowExtension {
                deactivated: deactivated.clone(),
                finish: finish.clone(),
            }))
            .unwrap();
        *rt.extensions.lock().await = Some(runner);
    });
    drop(rt);
    reap(&e.handle.shared);
    wait("deactivation begins", || {
        deactivated.load(std::sync::atomic::Ordering::SeqCst)
    });
    let c = e.handle.connect("reopen");
    c.send(ClientFrame {
        id: 1,
        lane: Lane::Normal,
        call: Call::OpenSession {
            session: id.clone(),
            window: None,
        },
    })
    .unwrap();
    wait("new generation waiting", || {
        let sessions = e.handle.shared.sessions.lock().unwrap();
        sessions
            .live
            .get(&id)
            .is_some_and(|entry| Arc::strong_count(&entry.rt) > 1)
    });
    {
        let rt = e.handle.shared.session(&id).unwrap();
        assert!(!rt.predecessor.as_ref().unwrap().is_cancelled());
        assert!(rt.extensions.try_lock().unwrap().is_none());
    }
    old_questions.notify(NoticeLevel::Error, "stale notice");
    old_questions.set_status("stale", Some("must not appear"));
    let err = e
        .handle
        .shared
        .handle
        .block_on(dispatch(Call::HistoryPage {
            session: id.clone(),
            before: None,
            limit: 1,
        }))
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::ShuttingDown);
    while let Ok(frame) = c.try_recv() {
        assert!(
            !matches!(frame, ServerFrame::Reply { .. }),
            "Open must wait for old extensions to exit"
        );
        assert!(
            !matches!(
                frame,
                ServerFrame::Event {
                    event: Event::Session {
                        change: SessionChange::UiChanged { .. },
                        ..
                    },
                    ..
                }
            ),
            "old question host must not publish into new generation"
        );
    }
    finish.cancel();
    wait("new extensions activate", || {
        let rt = e.handle.shared.session(&id).unwrap();
        !rt.questions.view().questions.is_empty()
    });
    let ui = e.snapshot(&id).ui;
    assert_eq!(
        ui.questions.len(),
        1,
        "fresh question host accepts a new question"
    );
    assert!(ui.notices.iter().all(|n| !n.message.contains("stale")));
}

#[test]
fn detached_manual_compaction_finishes_before_retirement() {
    let fx = Fixture::new();
    std::fs::create_dir_all(fx.dir.path().join("config")).unwrap();
    std::fs::write(
        fx.dir.path().join("config/settings.json"),
        r#"{"compaction":{"keepRecentTokens":5}}"#,
    )
    .unwrap();
    let gate = Gate::new();
    let g = gate.clone();
    let provider = Scripted::new(move |req, _| {
        if req.tools.is_empty() {
            R::Gated(g.clone(), Box::new(R::Text("summary".into())))
        } else {
            R::Text("a longer response with some content".into())
        }
    });
    let mut e = fx.start(&provider);
    let id = e.create(&fx.project, "c");
    for n in 0..4 {
        let key = format!("r{n}");
        e.prompt(&id, &key, "content for a conversation turn")
            .unwrap();
        e.settled(&key);
    }
    let compact = e.handle.connect("compact");
    compact
        .send(ClientFrame {
            id: 1,
            lane: Lane::Normal,
            call: Call::Compact {
                session: id.clone(),
                instructions: None,
            },
        })
        .unwrap();
    wait("summary exchange started", || {
        provider.requests().iter().any(|r| r.tools.is_empty())
    });
    reap(&e.handle.shared);
    assert!(
        live(&e.handle.shared, &id),
        "manual compaction holds runtime without an attachment"
    );
    drop(compact);
    gate.open();
    wait("compaction commits and retires", || {
        reap(&e.handle.shared);
        !live(&e.handle.shared, &id)
    });
    let s = e.snapshot(&id);
    assert!(
        s.entries
            .iter()
            .any(|e| matches!(e.kind, EntryKind::Compaction { .. }))
    );
}

#[test]
fn an_answer_from_a_retired_lifetime_cannot_answer_its_replacement() {
    let fx = Fixture::new();
    let mut config = fx.config();
    config.extensions = vec![crate::config::ExtensionSpec::Builtin("questions".into())];
    let mut e = fx.start_config(&Scripted::echo("done"), config);
    let id = e.create(&fx.project, "c");
    e.snapshot(&id);
    wait("first question", || {
        !e.handle
            .shared
            .session(&id)
            .unwrap()
            .questions
            .view()
            .questions
            .is_empty()
    });
    let before = e.snapshot(&id).ui.questions[0].clone();
    e.call(Call::ReleaseSession {
        session: id.clone(),
    })
    .unwrap();
    wait("first lifetime retired", || {
        reap(&e.handle.shared);
        !live(&e.handle.shared, &id)
    });
    e.snapshot(&id);
    wait("replacement question", || {
        !e.handle
            .shared
            .session(&id)
            .unwrap()
            .questions
            .view()
            .questions
            .is_empty()
    });
    let after = e.snapshot(&id).ui.questions[0].clone();
    assert_ne!(before.id, after.id);
    assert!(
        e.call(Call::AnswerQuestion {
            session: id.clone(),
            question: before.id,
            answer: Answer::Choice {
                value: before.options[0].value.clone()
            }
        })
        .is_err()
    );
    assert_eq!(e.snapshot(&id).ui.questions[0].id, after.id);
    e.call(Call::AnswerQuestion {
        session: id,
        question: after.id,
        answer: Answer::Choice {
            value: after.options[0].value.clone(),
        },
    })
    .unwrap();
}

#[test]
fn retirement_does_not_clear_a_storage_failure_before_explicit_reopen() {
    let fx = Fixture::new();
    let mut e = fx.start(&Scripted::echo("done"));
    let id = e.create(&fx.project, "c");
    e.snapshot(&id);
    *e.handle.shared.session(&id).unwrap().failed.lock().unwrap() = Some("storage failed".into());
    e.call(Call::ReleaseSession {
        session: id.clone(),
    })
    .unwrap();
    reap(&e.handle.shared);
    assert_eq!(
        e.handle.shared.session(&id).unwrap().failure().as_deref(),
        Some("storage failed")
    );
    e.snapshot(&id);
    assert!(e.handle.shared.session(&id).unwrap().failure().is_none());
    e.call(Call::ReleaseSession {
        session: id.clone(),
    })
    .unwrap();
    wait("recovered session retires", || {
        reap(&e.handle.shared);
        !live(&e.handle.shared, &id)
    });
}

#[test]
fn idle_release_is_retired_by_the_running_engine_without_manual_sweeps() {
    let fx = Fixture::new();
    let mut e = fx.start(&Scripted::echo("done"));
    let id = e.create(&fx.project, "c");
    e.prompt(&id, "r", "persist this").unwrap();
    e.settled("r");
    e.snapshot(&id);
    e.call(Call::ReleaseSession {
        session: id.clone(),
    })
    .unwrap();
    wait("automatic retirement", || !live(&e.handle.shared, &id));
    assert!(e.snapshot(&id).entries.iter().any(|e| matches!(&e.kind, EntryKind::Message { message, .. } if message.text() == "persist this")));
}

#[test]
fn concurrent_first_users_share_one_durable_provider_identity() {
    let fx = Fixture::new();
    let mut e = fx.start(&Scripted::echo("done"));
    let id = e.create(&fx.project, "c");
    let barrier = std::sync::Barrier::new(16);
    let values = std::thread::scope(|scope| {
        let tasks: Vec<_> = (0..16)
            .map(|n| {
                let (id, barrier, store) = (&id, &barrier, &e.handle.shared.store);
                scope.spawn(move || {
                    barrier.wait();
                    store
                        .kv_get_or_insert(id, "provider_session_id", &format!("candidate-{n}"))
                        .unwrap()
                })
            })
            .collect();
        tasks
            .into_iter()
            .map(|t| t.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert!(values.iter().all(|v| v == &values[0]));
    assert_eq!(
        e.handle
            .shared
            .store
            .kv_get(&id, "provider_session_id")
            .unwrap()
            .as_ref(),
        Some(&values[0])
    );
}
