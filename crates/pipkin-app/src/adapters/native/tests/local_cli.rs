//! Fake protocol only: no CLI, provider, desktop input, or filesystem probe.
use super::*;
use pipkin_core::local_cli as core;

#[path = "local_cli_retirement.rs"]
mod retirement;

fn welcome() -> Option<Result<Reply, ProtoError>> {
    Some(Ok(Reply::Welcome(Welcome {
        protocol: PROTOCOL_VERSION,
        engine_version: "fake".into(),
        engine_revision: "fake".into(),
        schema: 1,
        epoch: 1,
        capabilities: vec!["localCliSetup".into()],
    })))
}
fn ticket() -> LocalCliSetupView {
    LocalCliSetupView {
        mode: pipkin_engine_protocol::LocalCliMode::ChatOnly,
        setup: "setup-123".into(),
        target: LocalCliSetupTarget::New {
            thinking: None,
            project: "/chosen-project".into(),
            model: LocalCliModel::Opus,
        },
        selected_executable: "/chosen-cli".into(),
        executable: "/resolved/cli".into(),
        project: "/resolved/project".into(),
        qualified_version: "qualified".into(),
        stage: LocalCliSetupStage::AwaitingTrust,
        reason: None,
        execution_enabled: false,
    }
}
fn identity() -> core::LocalCliCreationId {
    core::LocalCliCreationId {
        session: core::EngineSessionId("cli-setup-123".into()),
        setup: core::SetupId("setup-123".into()),
    }
}
fn cli_summary(id: &str) -> SessionSummary {
    let mut s = summary(id);
    s.execution_backend = ExecutionBackend::LocalClaudeCli;
    s.local_cli_mode = Some(LocalCliMode::ChatOnly);
    s.blocked = Some("Execution disabled pending qualification.".into());
    s
}
fn send(run: &Run, op: u64, request: core::LocalCliRequest) {
    run.backend.request(BackendRequest::LocalCli {
        conversation: core::LOCAL_CLI_GLOBAL,
        generation: 17,
        op: OperationId(op),
        request,
    });
}
fn inspect() -> core::LocalCliRequest {
    core::LocalCliRequest::Inspect {
        mode: core::CliMode::ChatOnly,
        executable: "/chosen-cli".into(),
        target: core::LocalCliInspectTarget::New {
            thinking: None,
            project: "/chosen-project".into(),
            model: core::CliModel::Opus,
        },
    }
}
fn event(run: &Run, op: u64) -> Option<&core::LocalCliEvent> {
    run.seen.iter().find_map(|e| {
        if e.conversation != core::LOCAL_CLI_GLOBAL
            || e.generation != 17
            || e.op != Some(OperationId(op))
        {
            return None;
        }
        match &e.kind {
            EventKind::LocalCli(event) => Some(event),
            _ => None,
        }
    })
}

#[test]
fn coding_mode_roundtrips_and_a_different_reply_mode_is_retired_not_adopted() {
    for wrong_mode in [false, true] {
        let fake = Fake::new();
        let mut run = Run::start(fake.host(move |call| match call {
            Call::Hello { .. } => Some(welcome()),
            Call::InspectLocalCli { mode, .. } => {
                assert_eq!(*mode, pipkin_engine_protocol::LocalCliMode::RestrictedWrite);
                let mut t = ticket();
                if !wrong_mode {
                    t.mode = *mode;
                }
                t.execution_enabled = true; // Never an execution grant in the adapter.
                Some(Some(Ok(Reply::LocalCliSetup(t))))
            }
            Call::RevokeLocalCliSetup { .. } => Some(Some(Ok(Reply::Ok))),
            _ => None,
        }));
        let mut request = inspect();
        let core::LocalCliRequest::Inspect { mode, .. } = &mut request else {
            panic!()
        };
        *mode = core::CliMode::RestrictedWrite;
        send(&run, 73, request);
        run.until("mode-bound inspect", |r| event(r, 73).is_some());
        if wrong_mode {
            assert!(matches!(
                event(&run, 73),
                Some(core::LocalCliEvent::Failed {
                    failure: core::LocalCliFailure::Unknown(_),
                    ..
                })
            ));
            run.until("retire mismatched ticket", |_| {
                fake.count(|c| matches!(c, Call::RevokeLocalCliSetup { .. })) == 1
            });
        } else {
            assert!(
                matches!(event(&run, 73), Some(core::LocalCliEvent::Inspected(v)) if v.mode == core::CliMode::RestrictedWrite)
            );
        }
        assert_eq!(
            fake.count(|c| matches!(
                c,
                Call::CheckLocalCli { .. }
                    | Call::CreateLocalCliSession { .. }
                    | Call::Prompt { .. }
            )),
            0
        );
    }
}

#[test]
fn inspect_check_create_maps_exact_identity_and_catalog_not_newest_row() {
    let fake = Fake::new();
    let created = Arc::new(AtomicBool::new(false));
    let saved = created.clone();
    let mut run = Run::start(fake.host(move |call| match call {
        Call::Hello { .. } => Some(welcome()),
        Call::InspectLocalCli {
            executable,
            target,
            mode,
        } => {
            assert_eq!(*mode, pipkin_engine_protocol::LocalCliMode::ChatOnly);
            assert_eq!(executable, "/chosen-cli");
            assert_eq!(target, &ticket().target);
            Some(Some(Ok(Reply::LocalCliSetup(ticket()))))
        }
        Call::CheckLocalCli {
            setup,
            trust_executable,
            trust_project,
        } => {
            assert_eq!(setup, "setup-123");
            assert!(*trust_executable && *trust_project);
            let mut t = ticket();
            t.stage = LocalCliSetupStage::MetadataVerified;
            t.execution_enabled = true; // This metadata flag is not an execution grant.
            Some(Some(Ok(Reply::LocalCliSetup(t))))
        }
        Call::CreateLocalCliSession { session, setup } => {
            assert_eq!(session, "cli-setup-123");
            assert_eq!(setup, "setup-123");
            saved.store(true, Ordering::SeqCst);
            Some(Some(Ok(Reply::Session {
                summary: cli_summary(session),
            })))
        }
        Call::ListSessions if saved.load(Ordering::SeqCst) => {
            let mut unrelated = summary("unrelated-newest");
            unrelated.created_at_ms += 999_999;
            Some(Some(Ok(Reply::Sessions {
                sessions: vec![cli_summary("cli-setup-123"), unrelated],
            })))
        }
        _ => None,
    }));
    run.ready();
    assert!(
        run.lifecycle
            .iter()
            .any(|e| matches!(e, LifecycleEvent::LocalCliSetupAvailable(true)))
    );
    send(&run, 1, inspect());
    run.until("inspection", |r| event(r, 1).is_some());
    let Some(core::LocalCliEvent::Inspected(view)) = event(&run, 1) else {
        panic!("wrong inspect result")
    };
    assert_eq!(view.executable, "/resolved/cli");
    assert_eq!(view.project, "/resolved/project");
    assert_eq!(view.root_candidate, Some(identity().session));
    assert_eq!(
        view.target,
        match inspect() {
            core::LocalCliRequest::Inspect { target, .. } => target,
            _ => unreachable!(),
        }
    );
    assert_eq!(
        fake.count(|c| matches!(c, Call::CheckLocalCli { .. })),
        0,
        "inspection cannot auto-approve"
    );
    send(
        &run,
        2,
        core::LocalCliRequest::Check {
            setup: identity().setup,
            trust_executable: true,
            trust_project: true,
        },
    );
    run.until("metadata verified", |r| matches!(event(r, 2), Some(core::LocalCliEvent::Checked(v)) if v.stage == core::LocalCliMetadataStage::MetadataVerified));
    assert!(
        !run.has(|k| matches!(
            k,
            EventKind::ExecutionContext {
                execution_blocked: None,
                ..
            }
        )),
        "metadata must not unblock execution"
    );
    send(
        &run,
        3,
        core::LocalCliRequest::Create {
            identity: identity(),
        },
    );
    run.until("created", |r| event(r, 3).is_some());
    let Some(core::LocalCliEvent::Created(done)) = event(&run, 3) else {
        panic!("wrong create result")
    };
    assert_eq!(done.identity, identity());
    let expected = super::super::mapping::assign_ids(&[cli_summary("cli-setup-123"), {
        let mut s = summary("unrelated-newest");
        s.created_at_ms += 999_999;
        s
    }])
    .into_iter()
    .find(|(_, s)| s.id == "cli-setup-123")
    .unwrap()
    .0;
    assert_eq!(done.conversation, expected);
    assert_eq!(
        run.backend.local_cli_sessions(),
        vec![(expected, identity().session)]
    );
    assert!(run.lifecycle.iter().any(|e| matches!(e, LifecycleEvent::Catalog(b) if b.conversations.iter().any(|c| c.0 == expected))));
    assert_eq!(
        fake.count(|c| matches!(c, Call::CreateSession { .. } | Call::Prompt { .. })),
        0
    );
    run.backend.shutdown();
}

#[test]
fn unknown_creation_only_explicit_pure_lookup_and_never_native_fallback() {
    let fake = Fake::new();
    let mut run = Run::start(fake.host(|call| match call {
        Call::Hello { .. } => Some(welcome()),
        Call::CreateLocalCliSession { .. } => Some(None), // lost acknowledgement
        Call::LookupLocalCliCreation { session, setup } => {
            assert_eq!(session, "cli-setup-123");
            assert_eq!(setup, "setup-123");
            Some(Some(Ok(Reply::LocalCliCreation { summary: None })))
        }
        _ => None,
    }));
    run.ready();
    send(
        &run,
        10,
        core::LocalCliRequest::Create {
            identity: identity(),
        },
    );
    run.until("unknown creation", |r| {
        matches!(
            event(r, 10),
            Some(core::LocalCliEvent::Failed {
                phase: core::LocalCliPhase::Create,
                failure: core::LocalCliFailure::Unknown(_)
            })
        )
    });
    assert_eq!(
        fake.count(|c| matches!(c, Call::LookupLocalCliCreation { .. })),
        0
    );
    send(
        &run,
        11,
        core::LocalCliRequest::LookupCreation {
            identity: identity(),
        },
    );
    run.until("pure missing lookup", |r| {
        matches!(
            event(r, 11),
            Some(core::LocalCliEvent::CreationLookup { found: None, .. })
        )
    });
    assert_eq!(
        fake.count(|c| matches!(c, Call::CreateLocalCliSession { .. })),
        1
    );
    assert_eq!(
        fake.count(|c| matches!(
            c,
            Call::CreateSession { .. } | Call::InspectLocalCli { .. } | Call::CheckLocalCli { .. }
        )),
        0
    );
    run.backend.shutdown();
}

#[test]
fn cancel_revoke_crosses_control_lane_while_metadata_check_is_unanswered() {
    let fake = Fake::new();
    let mut run = Run::start(fake.host(|call| match call {
        Call::Hello { .. } => Some(welcome()),
        Call::CheckLocalCli { .. } => Some(None),
        Call::RevokeLocalCliSetup { setup } => {
            assert_eq!(setup, "setup-123");
            Some(Some(Ok(Reply::Ok)))
        }
        _ => None,
    }));
    run.ready();
    send(
        &run,
        20,
        core::LocalCliRequest::Check {
            setup: identity().setup,
            trust_executable: true,
            trust_project: true,
        },
    );
    run.until("check in flight", |_| {
        fake.count(|c| matches!(c, Call::CheckLocalCli { .. })) == 1
    });
    let start = Instant::now();
    send(
        &run,
        21,
        core::LocalCliRequest::Revoke {
            setup: identity().setup,
        },
    );
    run.until("revoke before check timeout", |r| {
        matches!(event(r, 21), Some(core::LocalCliEvent::Revoked))
    });
    assert!(start.elapsed() < Duration::from_millis(400));
    assert!(
        event(&run, 20).is_none(),
        "revoke must complete before the held Check"
    );
    run.backend.shutdown();
}

#[test]
fn status_is_pure_and_expired_is_not_rechecked() {
    let fake = Fake::new();
    let mut run = Run::start(fake.host(|call| match call {
        Call::Hello { .. } => Some(welcome()),
        Call::LocalCliSetupStatus { .. } => {
            Some(Some(Err(ProtoError::new(ErrorCode::NotFound, "expired"))))
        }
        _ => None,
    }));
    run.ready();
    send(
        &run,
        30,
        core::LocalCliRequest::Status {
            setup: identity().setup,
        },
    );
    run.until("expired status", |r| {
        matches!(
            event(r, 30),
            Some(core::LocalCliEvent::Failed {
                phase: core::LocalCliPhase::Status,
                failure: core::LocalCliFailure::Expired
            })
        )
    });
    assert_eq!(
        fake.count(|c| matches!(
            c,
            Call::CheckLocalCli { .. }
                | Call::InspectLocalCli { .. }
                | Call::CreateLocalCliSession { .. }
        )),
        0
    );
    run.backend.shutdown();
}

#[test]
fn hello_without_capability_refuses_setup_without_fallback() {
    let fake = Fake::new();
    let mut run = Run::start(fake.host(|_| None));
    run.ready();
    assert!(
        !run.lifecycle
            .iter()
            .any(|e| matches!(e, LifecycleEvent::LocalCliSetupAvailable(true)))
    );
    send(&run, 40, inspect());
    run.until("unsupported", |r| {
        matches!(
            event(r, 40),
            Some(core::LocalCliEvent::Failed {
                failure: core::LocalCliFailure::Rejected(_),
                ..
            })
        )
    });
    assert_eq!(
        fake.count(|c| matches!(c, Call::InspectLocalCli { .. } | Call::CreateSession { .. })),
        0
    );
    run.backend.shutdown();
}

#[test]
fn stale_existing_generation_and_unknown_profile_cannot_inspect_or_create_native() {
    for backend in [
        ExecutionBackend::LocalClaudeCli,
        ExecutionBackend::Unsupported,
    ] {
        let fake = Fake::new();
        let mut run = Run::start(fake.host(move |call| match call {
            Call::Hello { .. } => Some(welcome()),
            Call::ListSessions => {
                let mut s = summary("s1");
                s.execution_backend = backend;
                Some(Some(Ok(Reply::Sessions { sessions: vec![s] })))
            }
            Call::OpenSession { session, .. } => {
                let mut s = snapshot(session, vec![]);
                s.summary.execution_backend = backend;
                Some(Some(Ok(Reply::Snapshot(Box::new(s)))))
            }
            _ => None,
        }));
        run.ready();
        let conversation = run.conversation();
        run.backend.request(BackendRequest::Open {
            conversation,
            generation: 9,
        });
        run.until("attached", |r| {
            r.has(|k| matches!(k, EventKind::Opened { .. }))
        });
        let generation = if backend == ExecutionBackend::Unsupported {
            9
        } else {
            8
        };
        run.backend.request(BackendRequest::LocalCli {
            conversation,
            generation,
            op: OperationId(50),
            request: core::LocalCliRequest::Inspect {
                mode: core::CliMode::ChatOnly,
                executable: "/chosen-cli".into(),
                target: core::LocalCliInspectTarget::Existing {
                    session: core::EngineSessionId("s1".into()),
                    model: core::CliModel::Sonnet,
                },
            },
        });
        run.until("source refused", |r| {
            r.seen.iter().any(|e| {
                e.conversation == conversation
                    && e.generation == generation
                    && e.op == Some(OperationId(50))
                    && matches!(
                        e.kind,
                        EventKind::LocalCli(core::LocalCliEvent::Failed {
                            failure: core::LocalCliFailure::Rejected(_),
                            ..
                        })
                    )
            })
        });
        assert_eq!(
            fake.count(|c| matches!(c, Call::InspectLocalCli { .. } | Call::CreateSession { .. })),
            0
        );
        run.backend.shutdown();
    }
}

#[test]
fn successful_lookup_resolves_exact_catalog_id_without_probe_or_create() {
    let fake = Fake::new();
    let mut run = Run::start(fake.host(|call| match call {
        Call::Hello { .. } => Some(welcome()),
        Call::LookupLocalCliCreation { session, .. } => Some(Some(Ok(Reply::LocalCliCreation {
            summary: Some(cli_summary(session)),
        }))),
        Call::ListSessions => Some(Some(Ok(Reply::Sessions {
            sessions: vec![summary("unrelated"), cli_summary("cli-setup-123")],
        }))),
        _ => None,
    }));
    run.ready();
    send(
        &run,
        60,
        core::LocalCliRequest::LookupCreation {
            identity: identity(),
        },
    );
    run.until("found lookup", |r| {
        matches!(
            event(r, 60),
            Some(core::LocalCliEvent::CreationLookup { found: Some(_), .. })
        )
    });
    let Some(core::LocalCliEvent::CreationLookup {
        found: Some(found), ..
    }) = event(&run, 60)
    else {
        unreachable!()
    };
    let expected =
        super::super::mapping::assign_ids(&[summary("unrelated"), cli_summary("cli-setup-123")])
            .into_iter()
            .find(|(_, s)| s.id == "cli-setup-123")
            .unwrap()
            .0;
    assert_eq!(found.conversation, expected);
    assert_eq!(found.identity, identity());
    assert_eq!(
        fake.count(|c| matches!(
            c,
            Call::CreateSession { .. }
                | Call::CreateLocalCliSession { .. }
                | Call::InspectLocalCli { .. }
                | Call::CheckLocalCli { .. }
        )),
        0
    );
    run.backend.shutdown();
}

#[test]
fn malformed_reservation_and_conflicting_creation_fail_closed() {
    let fake = Fake::new();
    let mut run = Run::start(fake.host(|call| match call {
        Call::Hello { .. } => Some(welcome()),
        Call::InspectLocalCli { .. } => {
            let mut t = ticket();
            t.setup = "../not-a-uuid".into();
            Some(Some(Ok(Reply::LocalCliSetup(t))))
        }
        Call::CreateLocalCliSession { .. } => Some(Some(Ok(Reply::Session {
            summary: cli_summary("different-root"),
        }))),
        _ => None,
    }));
    run.ready();
    send(&run, 70, inspect());
    run.until("invalid setup rejected", |r| {
        matches!(event(r, 70), Some(core::LocalCliEvent::Failed { .. }))
    });
    let mut invalid = identity();
    invalid.session.0 = "native-root".into();
    send(
        &run,
        71,
        core::LocalCliRequest::Create { identity: invalid },
    );
    run.until("invalid reservation refused", |r| {
        matches!(
            event(r, 71),
            Some(core::LocalCliEvent::Failed {
                failure: core::LocalCliFailure::Rejected(_),
                ..
            })
        )
    });
    assert_eq!(
        fake.count(|c| matches!(c, Call::CreateLocalCliSession { .. })),
        0
    );
    send(
        &run,
        72,
        core::LocalCliRequest::Create {
            identity: identity(),
        },
    );
    run.until("conflict", |r| {
        matches!(
            event(r, 72),
            Some(core::LocalCliEvent::Failed {
                failure: core::LocalCliFailure::Conflict,
                ..
            })
        )
    });
    assert_eq!(fake.count(|c| matches!(c, Call::CreateSession { .. })), 0);
    run.backend.shutdown();
}

#[test]
fn session_status_is_readonly_canonical_hints_for_captured_session() {
    let fake = Fake::new();
    let mut run = Run::start(fake.host(|call| match call {
        Call::Hello { .. } => Some(welcome()),
        Call::ListSessions => Some(Some(Ok(Reply::Sessions {
            sessions: vec![cli_summary("s1"), cli_summary("other-selected")],
        }))),
        Call::OpenSession { session, .. } => {
            let mut s = snapshot(session, vec![]);
            s.summary = cli_summary(session);
            Some(Some(Ok(Reply::Snapshot(Box::new(s)))))
        }
        Call::LocalCliSessionStatus { session } => {
            Some(Some(Ok(Reply::LocalCliSession(LocalCliSessionView {
                mode: pipkin_engine_protocol::LocalCliMode::ChatOnly,
                summary: cli_summary(session),
                executable: "/saved/canonical/cli".into(),
                cli_version: "qualified".into(),
                model: Some(LocalCliModel::Haiku),
                context_valid: false,
            }))))
        }
        _ => None,
    }));
    run.ready();
    let ids = run.backend.local_cli_sessions();
    let conversation = ids.iter().find(|(_, s)| s.0 == "s1").unwrap().0;
    let other = ids.iter().find(|(_, s)| s.0 == "other-selected").unwrap().0;
    run.backend.request(BackendRequest::Open {
        conversation,
        generation: 7,
    });
    run.until("attached", |r| {
        r.has(|k| matches!(k, EventKind::Opened { .. }))
    });
    run.backend.request(BackendRequest::Open {
        conversation: other,
        generation: 8,
    });
    run.until("selection moved away", |r| {
        r.seen
            .iter()
            .any(|e| e.conversation == other && matches!(e.kind, EventKind::Opened { .. }))
    });
    run.backend.request(BackendRequest::LocalCli {
        conversation,
        generation: 7,
        op: OperationId(80),
        request: core::LocalCliRequest::SessionStatus {
            session: core::EngineSessionId("s1".into()),
        },
    });
    run.until("saved hints", |r| r.seen.iter().any(|e| e.conversation == conversation && e.generation == 7 && e.op == Some(OperationId(80)) && matches!(&e.kind, EventKind::LocalCli(core::LocalCliEvent::SessionStatus(v)) if v.session.0 == "s1" && v.executable == "/saved/canonical/cli" && v.model == Some(core::CliModel::Haiku) && !v.context_valid)));
    assert_eq!(
        fake.count(|c| matches!(
            c,
            Call::InspectLocalCli { .. } | Call::CheckLocalCli { .. } | Call::CreateSession { .. }
        )),
        0
    );
    assert!(!run.has(|k| matches!(
        k,
        EventKind::ExecutionContext {
            execution_blocked: None,
            ..
        }
    )));
    run.backend.shutdown();
}

#[test]
fn superseded_inspection_ticket_is_revoked_best_effort_without_waiting() {
    let fake = Fake::new();
    let inspections = AtomicU64::new(0);
    let mut run = Run::start(fake.host(move |call| match call {
        Call::Hello { .. } => Some(welcome()),
        Call::InspectLocalCli { .. } => {
            let first = inspections.fetch_add(1, Ordering::SeqCst) == 0;
            let mut t = ticket();
            if first {
                thread::sleep(Duration::from_millis(120));
            } else {
                t.setup = "setup-456".into();
            }
            Some(Some(Ok(Reply::LocalCliSetup(t))))
        }
        _ => None,
    }));
    run.ready();
    send(&run, 90, inspect());
    run.until("first inspection pending", |_| {
        fake.count(|c| matches!(c, Call::InspectLocalCli { .. })) == 1
    });
    send(&run, 91, inspect());
    run.until("current inspection", |r| matches!(event(r, 91), Some(core::LocalCliEvent::Inspected(v)) if v.setup.0 == "setup-456"));
    run.until("stale cleanup", |_| {
        fake.count(|c| matches!(c, Call::RevokeLocalCliSetup { setup } if setup == "setup-123"))
            == 1
    });
    assert!(!matches!(
        event(&run, 90),
        Some(core::LocalCliEvent::Inspected(_))
    ));
    assert_eq!(
        fake.count(|c| matches!(c, Call::RevokeLocalCliSetup { setup } if setup == "setup-456")),
        0
    );
    run.backend.shutdown();
}

#[test]
fn disconnect_invalidates_setup_and_does_not_retry_or_probe_unknown_creation() {
    let fake = Fake::new();
    let mut run = Run::start(fake.host(|call| match call {
        Call::Hello { .. } => Some(welcome()),
        Call::CreateLocalCliSession { .. } => Some(None),
        _ => None,
    }));
    run.ready();
    let before = run.lifecycle.len();
    send(
        &run,
        100,
        core::LocalCliRequest::Create {
            identity: identity(),
        },
    );
    run.until("create in flight", |_| {
        fake.count(|c| matches!(c, Call::CreateLocalCliSession { .. })) == 1
    });
    fake.sever();
    run.until("setup invalidated", |r| {
        r.lifecycle[before..]
            .iter()
            .any(|e| matches!(e, LifecycleEvent::LocalCliSetupAvailable(false)))
    });
    run.until("reconnected", |r| {
        r.lifecycle[before..]
            .iter()
            .any(|e| matches!(e, LifecycleEvent::Connection(Connection::Ready)))
    });
    assert_eq!(
        fake.count(|c| matches!(c, Call::CreateLocalCliSession { .. })),
        1
    );
    assert_eq!(
        fake.count(|c| matches!(
            c,
            Call::LookupLocalCliCreation { .. }
                | Call::InspectLocalCli { .. }
                | Call::CheckLocalCli { .. }
                | Call::CreateSession { .. }
        )),
        0
    );
    run.backend.shutdown();
}

#[test]
fn transport_failure_classification_is_closed() {
    use super::super::{CallError, local_cli::failed};
    use core::{LocalCliEvent as E, LocalCliFailure as F, LocalCliPhase as P};
    assert!(matches!(
        failed(P::Create, CallError::NotSent("unsent".into())),
        E::Failed {
            failure: F::Rejected(_),
            ..
        }
    ));
    assert!(matches!(
        failed(P::Create, CallError::Lost("lost".into())),
        E::Failed {
            failure: F::Unknown(_),
            ..
        }
    ));
    assert!(matches!(
        failed(
            P::Create,
            CallError::Refused(ProtoError::new(ErrorCode::Conflict, "collision"))
        ),
        E::Failed {
            failure: F::Conflict,
            ..
        }
    ));
    assert!(matches!(
        failed(
            P::Check,
            CallError::Refused(ProtoError::new(ErrorCode::Invalid, "refused"))
        ),
        E::Failed {
            failure: F::Rejected(_),
            ..
        }
    ));
}
