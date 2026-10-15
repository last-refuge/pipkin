use super::*;

#[test]
fn offline_retirement_is_silent_and_never_replayed() {
    let mut run = Run::start(HostSpec::Fake(Arc::new(|| Err("offline test".into()))));
    retire(&run, core::LOCAL_CLI_GLOBAL, 17, 140);
    fence(&mut run, 141);
    assert!(event(&run, 140).is_none());
    assert!(
        !run.lifecycle
            .iter()
            .any(|e| matches!(e, LifecycleEvent::Notice(_)))
    );
    run.backend.shutdown();
}

#[test]
fn check_and_revoke_refresh_authoritative_source_without_metadata_grants_or_send() {
    for checking in [true, false] {
        let fake = Fake::new();
        let changed = AtomicBool::new(false);
        let mut run = Run::start(fake.host(move |call| match call {
            Call::Hello { .. } => Some(welcome()),
            // Intentionally stale/unblocked catalog: only the snapshot is authoritative.
            Call::ListSessions => {
                let mut s = cli_summary("s1");
                s.blocked = None;
                Some(Some(Ok(Reply::Sessions { sessions: vec![s] })))
            }
            Call::OpenSession { session, .. } => {
                let mut s = snapshot(session, vec![]);
                s.summary = cli_summary(session);
                s.revision = if changed.load(Ordering::SeqCst) { 2 } else { 1 };
                if !checking && !changed.load(Ordering::SeqCst) {
                    s.summary.blocked = None;
                }
                Some(Some(Ok(Reply::Snapshot(Box::new(s)))))
            }
            Call::CheckLocalCli { .. } => {
                changed.store(true, Ordering::SeqCst);
                let mut t = ticket();
                t.stage = LocalCliSetupStage::MetadataVerified;
                t.execution_enabled = true;
                Some(Some(Ok(Reply::LocalCliSetup(t))))
            }
            Call::RevokeLocalCliSetup { .. } => {
                changed.store(true, Ordering::SeqCst);
                Some(Some(Ok(Reply::Ok)))
            }
            _ => None,
        }));
        run.ready();
        let conversation = run.conversation();
        run.backend.request(BackendRequest::Open {
            conversation,
            generation: 7,
        });
        run.until("initial source snapshot", |r| {
            r.has(|e| matches!(e, EventKind::Opened { .. }))
        });
        let before = run.seen.len();
        let catalogs = fake.count(|c| matches!(c, Call::ListSessions));
        run.backend.request(BackendRequest::LocalCli {
            conversation,
            generation: 7,
            op: OperationId(150),
            request: if checking {
                core::LocalCliRequest::Check {
                    setup: identity().setup,
                    trust_executable: true,
                    trust_project: true,
                }
            } else {
                core::LocalCliRequest::Revoke {
                    setup: identity().setup,
                }
            },
        });
        run.until("setup completion after refresh", |r| {
            r.seen.iter().any(|e| e.op == Some(OperationId(150)))
        });
        assert_eq!(fake.count(|c| matches!(c, Call::OpenSession { .. })), 2);
        assert!(fake.count(|c| matches!(c, Call::ListSessions)) > catalogs);
        assert!(
            run.seen[before..]
                .iter()
                .any(|e| e.conversation == conversation
                    && e.generation == 7
                    && matches!(
                        &e.kind,
                        EventKind::ExecutionContext {
                            execution_blocked: Some(_),
                            ..
                        }
                    ))
        );
        assert!(!run.seen[before..].iter().any(|e| matches!(
            &e.kind,
            EventKind::ExecutionContext {
                execution_blocked: None,
                ..
            }
        )));
        assert_eq!(
            fake.count(|c| matches!(
                c,
                Call::Prompt { .. }
                    | Call::CreateSession { .. }
                    | Call::CreateLocalCliSession { .. }
            )),
            0
        );
        run.backend.shutdown();
    }
}

fn retire(run: &Run, conversation: ConversationId, generation: u64, op: u64) {
    run.backend.request(BackendRequest::RetireLocalCliInspect {
        conversation,
        generation,
        op: OperationId(op),
    });
}

// FIFO data-worker fence without a protocol call, even while Inspect is blocked.
fn fence(run: &mut Run, op: u64) {
    let mut invalid = identity();
    invalid.session.0 = "invalid".into();
    send(run, op, core::LocalCliRequest::Create { identity: invalid });
    run.until("data-worker fence", |r| event(r, op).is_some());
}

#[test]
fn retired_pending_inspect_cleans_only_its_returned_ticket_without_receipt() {
    let fake = Fake::new();
    let (release, wait) = mpsc::channel();
    let wait = Mutex::new(wait);
    let inspections = AtomicU64::new(0);
    let mut run = Run::start(fake.host(move |call| match call {
        Call::Hello { .. } => Some(welcome()),
        Call::InspectLocalCli { .. } => {
            let mut t = ticket();
            if inspections.fetch_add(1, Ordering::SeqCst) == 0 {
                wait.lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
            } else {
                t.setup = "newer-ticket".into();
            }
            Some(Some(Ok(Reply::LocalCliSetup(t))))
        }
        _ => None,
    }));
    run.ready();
    send(&run, 110, inspect());
    run.until("pending inspect", |_| {
        fake.count(|c| matches!(c, Call::InspectLocalCli { .. })) == 1
    });
    let before = fake.calls().len();
    retire(&run, ConversationId(123), 17, 110);
    retire(&run, core::LOCAL_CLI_GLOBAL, 18, 110);
    retire(&run, core::LOCAL_CLI_GLOBAL, 17, 109);
    fence(&mut run, 109);
    retire(&run, core::LOCAL_CLI_GLOBAL, 17, 110);
    fence(&mut run, 111);
    assert_eq!(
        fake.calls().len(),
        before,
        "no ticket means no protocol cleanup call"
    );
    send(&run, 112, inspect());
    fence(&mut run, 113);
    release.send(()).unwrap();
    run.until("newer inspect reply", |r| {
        matches!(event(r, 112), Some(core::LocalCliEvent::Inspected(_)))
    });
    run.until("retired ticket cleanup", |_| {
        fake.count(|c| matches!(c, Call::RevokeLocalCliSetup { setup } if setup == "setup-123"))
            == 1
    });
    retire(&run, core::LOCAL_CLI_GLOBAL, 17, 110);
    fence(&mut run, 114);
    assert!(event(&run, 110).is_none(), "retirement has no core receipt");
    assert_eq!(
        fake.count(|c| matches!(c, Call::RevokeLocalCliSetup { .. })),
        1
    );
    run.backend.shutdown();
}

#[test]
fn reply_before_retirement_retains_one_ticket_and_requires_the_exact_tuple() {
    let fake = Fake::new();
    let mut run = Run::start(fake.host(|call| match call {
        Call::Hello { .. } => Some(welcome()),
        Call::InspectLocalCli { .. } => Some(Some(Ok(Reply::LocalCliSetup(ticket())))),
        _ => None,
    }));
    run.ready();
    send(&run, 120, inspect());
    run.until("inspect reply", |r| event(r, 120).is_some());
    let before = fake.calls().len();
    retire(&run, ConversationId(123), 17, 120);
    retire(&run, core::LOCAL_CLI_GLOBAL, 18, 120);
    retire(&run, core::LOCAL_CLI_GLOBAL, 17, 121);
    fence(&mut run, 122);
    assert_eq!(fake.calls().len(), before);
    retire(&run, core::LOCAL_CLI_GLOBAL, 17, 120);
    run.until("returned ticket cleanup", |_| {
        fake.count(|c| matches!(c, Call::RevokeLocalCliSetup { setup } if setup == "setup-123"))
            == 1
    });
    retire(&run, core::LOCAL_CLI_GLOBAL, 17, 120);
    fence(&mut run, 123);
    assert_eq!(
        fake.count(|c| matches!(c, Call::RevokeLocalCliSetup { .. })),
        1
    );
    assert_eq!(
        run.seen
            .iter()
            .filter(|e| e.op == Some(OperationId(120)))
            .count(),
        1
    );
    run.backend.shutdown();
}

#[test]
fn created_active_conversation_is_not_cancelled_by_new_setup_or_old_retirement() {
    let fake = Fake::new();
    let inspections = AtomicU64::new(0);
    let mut run = Run::start(fake.host(move |call| match call {
        Call::Hello { .. } => Some(welcome()),
        Call::InspectLocalCli { .. } => {
            let mut t = ticket();
            if inspections.fetch_add(1, Ordering::SeqCst) > 0 {
                t.setup = "newer-ticket".into();
            }
            Some(Some(Ok(Reply::LocalCliSetup(t))))
        }
        Call::CreateLocalCliSession { session, .. } => Some(Some(Ok(Reply::Session {
            summary: cli_summary(session),
        }))),
        Call::ListSessions => Some(Some(Ok(Reply::Sessions {
            sessions: vec![cli_summary("cli-setup-123")],
        }))),
        Call::OpenSession { session, .. } => {
            let mut s = snapshot(session, vec![]);
            s.summary = cli_summary(session);
            s.summary.blocked = None;
            s.run = Some(RunView {
                id: 1,
                request: None,
                phase: RunPhase::Running,
                started_at_ms: 0,
            });
            Some(Some(Ok(Reply::Snapshot(Box::new(s)))))
        }
        _ => None,
    }));
    run.ready();
    send(&run, 130, inspect());
    run.until("inspect reply", |r| event(r, 130).is_some());
    send(
        &run,
        131,
        core::LocalCliRequest::Create {
            identity: identity(),
        },
    );
    run.until("created", |r| {
        matches!(event(r, 131), Some(core::LocalCliEvent::Created(_)))
    });
    run.backend.request(BackendRequest::Open {
        conversation: run.conversation(),
        generation: 1,
    });
    run.until("active created conversation", |r| {
        r.has(|e| matches!(e, EventKind::EngineState { busy: true, .. }))
    });
    retire(&run, core::LOCAL_CLI_GLOBAL, 17, 130);
    fence(&mut run, 132);
    send(&run, 133, inspect());
    run.until("new setup", |r| event(r, 133).is_some());
    retire(&run, core::LOCAL_CLI_GLOBAL, 17, 130);
    fence(&mut run, 134);
    assert_eq!(
        fake.count(|c| matches!(
            c,
            Call::RevokeLocalCliSetup { .. } | Call::Stop { .. } | Call::Prompt { .. }
        )),
        0
    );
    run.backend.shutdown();
}
