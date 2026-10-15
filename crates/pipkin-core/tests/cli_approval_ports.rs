use pipkin_core::*;
#[path = "cli_approval_ports/recovery.rs"]
mod recovery;
fn state() -> AppState {
    let mut s = AppState::new(
        Bootstrap {
            projects: vec![],
            models: vec![],
            conversations: (1..=9)
                .map(|n| (ConversationId(n), ProjectId(1), format!("{n}"), 1))
                .collect(),
            now: 1,
        },
        Prefs::default(),
    );
    s.mode = Mode::Real;
    for n in 1..=9 {
        let c = &mut s.conversations[n - 1];
        c.generation = 1;
        c.opened = true;
        c.run = RunState::Running {
            op: OperationId(100 + n as u64),
        };
        s.apply_event(BackendEvent {
            conversation: ConversationId(n as u64),
            generation: 1,
            op: None,
            kind: EventKind::ExecutionContext {
                profile: ExecutionProfile::LocalCliRestrictedWrite,
                execution_blocked: None,
            },
        });
    }
    s.set_cli_approvals_available(true);
    s
}
fn request(out: Outcome) -> CliApprovalPortRequest {
    assert_eq!(out.effects.len(), 1);
    match out.effects.into_iter().next().unwrap() {
        Effect::Backend(BackendRequest::CliApproval(r)) => r,
        other => panic!("{other:?}"),
    }
}
fn discover(s: &mut AppState, id: u64) -> CliApprovalPortRequest {
    request(
        s.dispatch(Command::CliApproval(CliApprovalCommand::Discover(
            ConversationId(id),
        ))),
    )
}
fn event(s: &mut AppState, r: &CliApprovalPortRequest, event: CliApprovalEvent) -> Outcome {
    let (conversation, generation, op) = r.tuple();
    s.apply_event(BackendEvent {
        conversation,
        generation,
        op: Some(op),
        kind: EventKind::CliApproval(Box::new(event)),
    })
}
fn review(q: &CliApprovalQuery) -> CliApprovalReview {
    CliApprovalReview {
        id: CliApprovalId {
            run: CliApprovalRun {
                epoch: 1,
                session: format!("s{}", q.conversation),
                binding_generation: 1,
                op: "external-op".into(),
                prompt: 40 + q.conversation.0,
            },
            invocation: "ab".repeat(32),
            arguments_sha256: "cd".repeat(32),
        },
        arguments: "Write\nfile_path: \"/project/a\"\ncontent: \"private\"".into(),
        expires_at_ms: 10000,
    }
}
fn install(s: &mut AppState, req: &CliApprovalPortRequest) -> CliApprovalReview {
    let CliApprovalPortRequest::Discover(q) = req else {
        panic!()
    };
    let r = review(q);
    event(
        s,
        req,
        CliApprovalEvent::Discovered {
            query: q.clone(),
            run: Some(r.id.run.clone()),
            reviews: vec![r.clone()],
        },
    );
    r
}
fn act(s: &mut AppState, context: &CliApprovalContext, action: CliApprovalAction) -> Outcome {
    let out = s.dispatch(Command::CliApproval(CliApprovalCommand::Act {
        context: context.clone(),
        action: Box::new(action),
    }));
    if let Some(Effect::JournalCliApproval(request)) = out.effects.first() {
        s.cli_approval_persisted(request, Ok(()))
    } else {
        out
    }
}
fn result(s: &mut AppState, req: &CliApprovalPortRequest, result: CliApprovalResult) {
    let CliApprovalPortRequest::Known(r) = req else {
        panic!()
    };
    event(
        s,
        req,
        CliApprovalEvent::Response(CliApprovalResponse {
            context: r.context.clone(),
            op: r.op,
            result,
        }),
    );
}
#[test]
fn command_effect_event_flow_is_captured_once_and_uncertainty_never_resends() {
    let mut s = state();
    let req = discover(&mut s, 1);
    s.dispatch(Command::SelectConversation(ConversationId(2)));
    let r = install(&mut s, &req);
    let context = s.cli_approval_context(ConversationId(1)).unwrap().clone();
    assert!(s.cli_approval_availability(&context, Some(&r.id)).decide);
    let allow = CliApprovalAction::Decide {
        id: r.id.clone(),
        decision: CliApprovalDecision::AllowOnce,
    };
    let req = request(act(&mut s, &context, allow.clone()));
    assert_eq!(
        s.cli_approvals.items(&context)[0].status,
        CliApprovalStatus::Submitting
    );
    assert!(act(&mut s, &context, allow.clone()).effects.is_empty());
    event(&mut s, &req, req.clone().failed());
    assert_eq!(
        s.cli_approvals.items(&context)[0].status,
        CliApprovalStatus::Unknown
    );
    let list = discover(&mut s, 1);
    result(
        &mut s,
        &list,
        CliApprovalResult::Listed {
            run: Some(context.run.clone()),
            reviews: vec![r.clone()],
        },
    );
    assert!(s.cli_approvals.items(&context)[0].review.is_none());
    assert!(act(&mut s, &context, allow).effects.is_empty());
    s.conversations[0].run = RunState::Idle;
    let lookup = request(act(
        &mut s,
        &context,
        CliApprovalAction::Lookup { id: r.id.clone() },
    ));
    result(&mut s, &lookup, CliApprovalResult::Receipt(None));
    assert_eq!(
        s.cli_approvals.items(&context)[0].status,
        CliApprovalStatus::Unknown
    );
    let lookup = request(act(
        &mut s,
        &context,
        CliApprovalAction::Lookup { id: r.id.clone() },
    ));
    result(
        &mut s,
        &lookup,
        CliApprovalResult::Receipt(Some(CliApprovalReceipt {
            id: r.id,
            state: CliApprovalReceiptState::Consumed,
            decision: Some(CliApprovalDecision::AllowOnce),
        })),
    );
    assert_eq!(
        s.cli_approvals.items(&context)[0].status,
        CliApprovalStatus::Recorded(CliApprovalReceiptState::Consumed)
    );
    assert!(
        s.conversations
            .iter()
            .all(|c| c.items.is_empty() && c.pending_intent.is_none())
    );
}
#[test]
fn discovery_is_single_flight_exact_scoped_and_can_complete_in_reverse_order() {
    let mut s = state();
    let a = discover(&mut s, 1);
    let b = discover(&mut s, 2);
    assert!(!s.cli_approval_discovery_available(ConversationId(1)));
    let CliApprovalPortRequest::Discover(q) = &a else {
        panic!()
    };
    let r = review(q);
    let mut wrong = q.clone();
    wrong.run_op = OperationId(999);
    event(
        &mut s,
        &a,
        CliApprovalEvent::Discovered {
            query: wrong,
            run: Some(r.id.run.clone()),
            reviews: vec![r],
        },
    );
    assert!(s.cli_approval_context(ConversationId(1)).is_none());
    install(&mut s, &b);
    install(&mut s, &a);
    assert!(s.cli_approval_context(ConversationId(1)).is_some());
    assert!(s.cli_approval_context(ConversationId(2)).is_some());
    let before = format!("{:?}", s.cli_approvals);
    install(&mut s, &a);
    assert_eq!(before, format!("{:?}", s.cli_approvals));
}
#[test]
fn lifecycle_loss_cannot_install_late_reviews_and_chat_is_never_a_review_capability() {
    for profile in [
        ExecutionProfile::NativeAgent,
        ExecutionProfile::LocalCliChatOnly,
        ExecutionProfile::Unsupported,
    ] {
        let mut s = state();
        let req = discover(&mut s, 1);
        s.apply_event(BackendEvent {
            conversation: ConversationId(1),
            generation: 1,
            op: None,
            kind: EventKind::ExecutionContext {
                profile,
                execution_blocked: None,
            },
        });
        install(&mut s, &req);
        assert!(s.cli_approval_context(ConversationId(1)).is_none());
        assert!(!s.cli_approval_discovery_available(ConversationId(1)));
    }
    for reason in 0..4 {
        let mut s = state();
        let req = discover(&mut s, 1);
        match reason {
            0 => {
                s.set_cli_approvals_available(false);
                s.set_cli_approvals_available(true);
            }
            1 => s.conversations[0].generation += 1,
            2 => {
                s.conversations[0].run = RunState::Stopping {
                    op: OperationId(101),
                }
            }
            _ => s.conversations[0].execution_blocked = Some("retired".into()),
        }
        install(&mut s, &req);
        assert!(s.cli_approval_context(ConversationId(1)).is_none());
    }
}
#[test]
fn explicit_reattachment_preserves_attempts_and_only_enables_read_only_recovery() {
    let mut s = state();
    let req = discover(&mut s, 1);
    let r = install(&mut s, &req);
    let old = s.cli_approval_context(ConversationId(1)).unwrap().clone();
    let decision = request(act(
        &mut s,
        &old,
        CliApprovalAction::Decide {
            id: r.id.clone(),
            decision: CliApprovalDecision::Deny,
        },
    ));
    // Wall-clock regression retires authority, but must not prevent explicit receipt
    // reattachment/recovery. Neither operation can restore a decision.
    s.set_now(0);
    s.conversations[0].generation = 2;
    s.dispatch(Command::CliApproval(CliApprovalCommand::Reattach(
        ConversationId(1),
    )));
    event(&mut s, &decision, decision.clone().failed());
    let new = s.cli_approval_context(ConversationId(1)).unwrap().clone();
    assert_eq!(new.generation, 2);
    assert_eq!(
        s.cli_approvals.items(&new)[0].status,
        CliApprovalStatus::Unknown
    );
    assert!(!s.cli_approval_availability(&new, Some(&r.id)).decide);
    assert!(!s.cli_approval_availability(&old, Some(&r.id)).lookup);
    let lookup = request(act(&mut s, &new, CliApprovalAction::Lookup { id: r.id }));
    result(&mut s, &lookup, CliApprovalResult::Receipt(None));
    assert_eq!(
        s.cli_approvals.items(&new)[0].status,
        CliApprovalStatus::Unknown
    );
}
#[test]
fn panel_policy_never_stops_a_different_selection_or_run_or_reattaches_a_live_scope() {
    let mut s = state();
    s.selected = Some(ConversationId(1));
    let req = discover(&mut s, 1);
    let review = install(&mut s, &req);
    let context = s.cli_approval_context(ConversationId(1)).unwrap().clone();
    assert!(s.cli_approval_panel_available(context.conversation));
    assert!(!s.cli_approval_reattach_available(context.conversation));
    assert!(
        s.dispatch(Command::CliApproval(CliApprovalCommand::Reattach(
            context.conversation
        )))
        .effects
        .is_empty()
    );
    assert!(
        s.cli_approval_availability(&context, Some(&review.id))
            .decide
    );
    assert!(matches!(
        s.cli_approval_stop_command(
            context.conversation,
            context.generation,
            Some(context.run_op)
        ),
        Some(Command::Cancel)
    ));
    s.selected = Some(ConversationId(2));
    assert!(
        s.cli_approval_stop_command(
            context.conversation,
            context.generation,
            Some(context.run_op)
        )
        .is_none()
    );
    s.selected = Some(context.conversation);
    assert!(
        s.cli_approval_stop_command(context.conversation, 2, Some(context.run_op))
            .is_none()
    );
    assert!(
        s.cli_approval_stop_command(context.conversation, 1, Some(OperationId(900)))
            .is_none()
    );
    s.set_cli_approvals_available(false);
    assert!(
        matches!(
            s.cli_approval_stop_command(
                context.conversation,
                context.generation,
                Some(context.run_op)
            ),
            Some(Command::Cancel)
        ),
        "Stop is independent of approval API availability"
    );
    s.mode = Mode::Demo;
    assert!(!s.cli_approval_panel_available(context.conversation));
}
#[test]
fn pending_discovery_is_bounded_and_capability_does_not_issue_requests() {
    let mut s = state();
    assert!(s.set_cli_approvals_available(true).effects.is_empty());
    for n in 1..=8 {
        discover(&mut s, n);
    }
    assert!(!s.cli_approval_discovery_available(ConversationId(9)));
    assert!(
        s.dispatch(Command::CliApproval(CliApprovalCommand::Discover(
            ConversationId(9)
        )))
        .effects
        .is_empty()
    );
}
