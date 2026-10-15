//! Fake wire only. No CLI launch, tools, model request, or raw argument persistence.
use super::*;
use pipkin_core::{
    CliApprovalAction, CliApprovalContext, CliApprovalDecision, CliApprovalEvent, CliApprovalId,
    CliApprovalPortRequest, CliApprovalQuery, CliApprovalRequest, CliApprovalResult,
    CliApprovalRun,
};
fn coding() -> SessionSummary {
    let mut s = summary("s1");
    s.execution_backend = ExecutionBackend::LocalClaudeCli;
    s.local_cli_mode = Some(LocalCliMode::RestrictedWrite);
    s
}
fn identity(prompt: u64) -> LocalCliApprovalRun {
    LocalCliApprovalRun {
        epoch: 1,
        session: "s1".into(),
        generation: 2,
        op: "external-op".into(),
        prompt,
    }
}
fn approval_id(prompt: u64) -> LocalCliApprovalId {
    LocalCliApprovalId {
        run: identity(prompt),
        invocation: "ab".repeat(32),
        arguments_sha256: "cd".repeat(32),
    }
}
fn start(fake: &Fake, reported: Arc<AtomicU64>, lost_decision: bool) -> Run {
    let mut r = Run::start(fake.host(move |call| match call {
        Call::Hello { .. } => Some(Some(Ok(Reply::Welcome(Welcome {
            protocol: PROTOCOL_VERSION,
            engine_version: "fake".into(),
            engine_revision: "fake".into(),
            schema: 25,
            epoch: 1,
            capabilities: vec!["localCliApprovals".into()],
        })))),
        Call::ListSessions => Some(Some(Ok(Reply::Sessions {
            sessions: vec![coding()],
        }))),
        Call::OpenSession { session, .. } => {
            let mut s = snapshot(session, vec![]);
            s.summary = coding();
            Some(Some(Ok(Reply::Snapshot(Box::new(s)))))
        }
        Call::Prompt { .. } => Some(Some(Ok(Reply::Admitted {
            run: Some(41),
            entry: None,
        }))),
        Call::Lookup { .. } => Some(Some(Ok(Reply::Lookup(LookupResult::Known {
            mode: InputMode::Prompt,
            entry: None,
            run: Some(41),
            open: true,
            outcome: None,
        })))),
        Call::LocalCliApprovals { .. } => {
            let prompt = reported.load(Ordering::SeqCst);
            Some(Some(Ok(Reply::LocalCliApprovals {
                run: Some(identity(prompt)),
                reviews: vec![LocalCliApprovalReview {
                    id: approval_id(prompt),
                    arguments: "Write\nfile_path: \"/project/a\"\ncontent: \"private\"".into(),
                    expires_at_ms: i64::MAX,
                }],
            })))
        }
        Call::DecideLocalCliApproval { id, decision } => Some(if lost_decision {
            None
        } else {
            Some(Ok(Reply::LocalCliApproval {
                receipt: Some(LocalCliApprovalReceipt {
                    id: id.clone(),
                    state: if *decision == LocalCliApprovalDecision::AllowOnce {
                        LocalCliApprovalState::Consumed
                    } else {
                        LocalCliApprovalState::Denied
                    },
                    decision: Some(*decision),
                }),
            }))
        }),
        Call::LookupLocalCliApproval { id } => Some(Some(Ok(Reply::LocalCliApproval {
            receipt: Some(LocalCliApprovalReceipt {
                id: id.clone(),
                state: LocalCliApprovalState::Consumed,
                decision: Some(LocalCliApprovalDecision::AllowOnce),
            }),
        }))),
        _ => None,
    }));
    r.ready();
    r.backend.request(BackendRequest::Open {
        conversation: r.conversation(),
        generation: 1,
    });
    r.until("coding snapshot", |r| {
        r.has(|e| matches!(e, EventKind::Opened { .. }))
    });
    r
}
fn submit(r: &mut Run) {
    r.backend.request(BackendRequest::Submit {
        conversation: r.conversation(),
        generation: 1,
        op: OperationId(7),
        request: RequestId("prompt".into()),
        text: "synthetic".into(),
        attachments: vec![],
        model: None,
    });
    r.until("admitted", |r| {
        r.seen
            .iter()
            .any(|e| e.op == Some(OperationId(7)) && matches!(e.kind, EventKind::Accepted))
    });
}
fn query(r: &Run, op: u64) -> CliApprovalQuery {
    CliApprovalQuery {
        conversation: r.conversation(),
        generation: 1,
        run_op: OperationId(7),
        op: OperationId(op),
    }
}
fn send(r: &mut Run, req: CliApprovalPortRequest) -> CliApprovalEvent {
    let (_, _, op) = req.tuple();
    r.backend.request(BackendRequest::CliApproval(req));
    r.until("approval reply", |r| {
        r.seen
            .iter()
            .any(|e| e.op == Some(op) && matches!(e.kind, EventKind::CliApproval(_)))
    });
    r.seen
        .iter()
        .rev()
        .find_map(|e| match &e.kind {
            EventKind::CliApproval(event) if e.op == Some(op) => Some((**event).clone()),
            _ => None,
        })
        .unwrap()
}
fn context(r: &Run) -> CliApprovalContext {
    CliApprovalContext {
        conversation: r.conversation(),
        generation: 1,
        run_op: OperationId(7),
        run: CliApprovalRun {
            epoch: 1,
            session: "s1".into(),
            binding_generation: 2,
            op: "external-op".into(),
            prompt: 41,
        },
    }
}
fn id(c: &CliApprovalContext) -> CliApprovalId {
    CliApprovalId {
        run: c.run.clone(),
        invocation: "ab".repeat(32),
        arguments_sha256: "cd".repeat(32),
    }
}
#[test]
fn discovery_requires_admitted_receipt_and_decisions_lookup_keep_exact_identity() {
    let fake = Fake::new();
    let reported = Arc::new(AtomicU64::new(41));
    let mut r = start(&fake, reported.clone(), false);
    let q = query(&r, 20);
    assert!(matches!(
        send(&mut r, CliApprovalPortRequest::Discover(q)),
        CliApprovalEvent::DiscoveryFailed(_)
    ));
    assert!(
        !fake
            .calls()
            .iter()
            .any(|c| matches!(c, Call::LocalCliApprovals { .. }))
    );
    submit(&mut r);
    reported.store(42, Ordering::SeqCst);
    let q = query(&r, 21);
    assert!(matches!(
        send(&mut r, CliApprovalPortRequest::Discover(q)),
        CliApprovalEvent::DiscoveryFailed(_)
    ));
    reported.store(41, Ordering::SeqCst);
    let q = query(&r, 22);
    let CliApprovalEvent::Discovered {
        run: Some(run),
        reviews,
        ..
    } = send(&mut r, CliApprovalPortRequest::Discover(q))
    else {
        panic!()
    };
    assert_eq!(run.prompt, 41);
    assert_eq!(reviews.len(), 1);
    assert!(reviews[0].arguments.contains("private"));
    let context = context(&r);
    let id = id(&context);
    for (op, decision) in [
        (23, CliApprovalDecision::AllowOnce),
        (24, CliApprovalDecision::Deny),
    ] {
        let response = send(
            &mut r,
            CliApprovalPortRequest::Known(CliApprovalRequest {
                context: context.clone(),
                op: OperationId(op),
                action: CliApprovalAction::Decide {
                    id: id.clone(),
                    decision,
                },
            }),
        );
        assert!(matches!(
            response,
            CliApprovalEvent::Response(pipkin_core::CliApprovalResponse {
                result: CliApprovalResult::Receipt(Some(_)),
                ..
            })
        ));
    }
    let before = fake.calls().len();
    let mut wrong = id.clone();
    wrong.run.session = "other".into();
    let response = send(
        &mut r,
        CliApprovalPortRequest::Known(CliApprovalRequest {
            context: context.clone(),
            op: OperationId(25),
            action: CliApprovalAction::Decide {
                id: wrong,
                decision: CliApprovalDecision::AllowOnce,
            },
        }),
    );
    assert!(matches!(
        response,
        CliApprovalEvent::Response(pipkin_core::CliApprovalResponse {
            result: CliApprovalResult::Failed,
            ..
        })
    ));
    assert_eq!(fake.calls().len(), before);
    let response = send(
        &mut r,
        CliApprovalPortRequest::Known(CliApprovalRequest {
            context,
            op: OperationId(26),
            action: CliApprovalAction::Lookup { id },
        }),
    );
    assert!(matches!(
        response,
        CliApprovalEvent::Response(pipkin_core::CliApprovalResponse {
            result: CliApprovalResult::Receipt(Some(_)),
            ..
        })
    ));
    assert_eq!(
        fake.calls()
            .iter()
            .filter(|c| matches!(c, Call::Prompt { .. }))
            .count(),
        1
    );
    assert_eq!(
        fake.calls()
            .iter()
            .filter(|c| matches!(c, Call::DecideLocalCliApproval { .. }))
            .count(),
        2
    );
}
#[test]
fn uncertain_decision_is_not_repeated_and_read_only_lookup_is_a_different_call() {
    let fake = Fake::new();
    let mut r = start(&fake, Arc::new(AtomicU64::new(41)), true);
    submit(&mut r);
    let context = context(&r);
    let id = id(&context);
    let reply = send(
        &mut r,
        CliApprovalPortRequest::Known(CliApprovalRequest {
            context: context.clone(),
            op: OperationId(30),
            action: CliApprovalAction::Decide {
                id: id.clone(),
                decision: CliApprovalDecision::AllowOnce,
            },
        }),
    );
    assert!(matches!(
        reply,
        CliApprovalEvent::Response(pipkin_core::CliApprovalResponse {
            result: CliApprovalResult::Failed,
            ..
        })
    ));
    send(
        &mut r,
        CliApprovalPortRequest::Known(CliApprovalRequest {
            context,
            op: OperationId(31),
            action: CliApprovalAction::Lookup { id },
        }),
    );
    assert_eq!(
        fake.calls()
            .iter()
            .filter(|c| matches!(c, Call::DecideLocalCliApproval { .. }))
            .count(),
        1
    );
    assert_eq!(
        fake.calls()
            .iter()
            .filter(|c| matches!(c, Call::LookupLocalCliApproval { .. }))
            .count(),
        1
    );
    assert_eq!(
        fake.calls()
            .iter()
            .filter(|c| matches!(c, Call::Prompt { .. }))
            .count(),
        1
    );
}
#[test]
fn replaced_connection_cannot_reuse_captured_prompt_proof_for_discovery() {
    let fake = Fake::new();
    let mut r = start(&fake, Arc::new(AtomicU64::new(41)), false);
    submit(&mut r);
    fake.sever();
    r.until("replacement connection", |_| {
        fake.connects.load(Ordering::SeqCst) >= 2
    });
    r.backend.request(BackendRequest::CheckStatus {
        conversation: r.conversation(),
        generation: 1,
        op: OperationId(7),
        request: Some(RequestId("prompt".into())),
    });
    r.until("old prompt looked up", |r| {
        r.seen.iter().any(|e| {
            e.op == Some(OperationId(7)) && matches!(e.kind, EventKind::StatusResolved { .. })
        })
    });
    let before = fake
        .calls()
        .iter()
        .filter(|c| matches!(c, Call::LocalCliApprovals { .. }))
        .count();
    let q = query(&r, 40);
    assert!(matches!(
        send(&mut r, CliApprovalPortRequest::Discover(q)),
        CliApprovalEvent::DiscoveryFailed(_)
    ));
    assert_eq!(
        fake.calls()
            .iter()
            .filter(|c| matches!(c, Call::LocalCliApprovals { .. }))
            .count(),
        before
    );
}
