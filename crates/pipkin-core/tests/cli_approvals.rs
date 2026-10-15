use pipkin_core::*;

fn context(n: u64) -> CliApprovalContext {
    CliApprovalContext {
        conversation: ConversationId(n),
        generation: 1,
        run_op: OperationId(10),
        run: CliApprovalRun {
            epoch: 1,
            session: format!("cli-{n}"),
            binding_generation: 1,
            op: "external-op".into(),
            prompt: 1,
        },
    }
}
fn review(c: &CliApprovalContext, n: usize) -> CliApprovalReview {
    CliApprovalReview {
        id: CliApprovalId {
            run: c.run.clone(),
            invocation: format!("{n:064x}"),
            arguments_sha256: "ab".repeat(32),
        },
        arguments: "Write\nfile_path: \"/project/file\"\ncontent: \"private content\"".into(),
        expires_at_ms: 10000,
    }
}
struct Harness {
    state: CliApprovals,
    context: CliApprovalContext,
    seq: u64,
}
impl Harness {
    fn new() -> Self {
        let mut state = CliApprovals::default();
        let context = context(1);
        state.tick(1000);
        assert!(state.bind(context.clone()));
        Self {
            state,
            context,
            seq: 100,
        }
    }
    fn request(&mut self, action: CliApprovalAction) -> CliApprovalRequest {
        self.seq += 1;
        self.state
            .request(&self.context, OperationId(self.seq), action)
            .unwrap()
    }
    fn response(
        &mut self,
        req: &CliApprovalRequest,
        result: CliApprovalResult,
    ) -> CliApprovalApply {
        self.state.apply(CliApprovalResponse {
            context: req.context.clone(),
            op: req.op,
            result,
        })
    }
    fn list(&mut self, reviews: Vec<CliApprovalReview>) -> CliApprovalApply {
        let req = self.request(CliApprovalAction::List);
        self.response(
            &req,
            CliApprovalResult::Listed {
                run: Some(self.context.run.clone()),
                reviews,
            },
        )
    }
    fn ready(&mut self) -> CliApprovalReview {
        let r = review(&self.context, 1);
        assert_eq!(self.list(vec![r.clone()]), CliApprovalApply::Applied);
        r
    }
    fn choose(&mut self, id: &CliApprovalId) -> CliApprovalRequest {
        self.request(CliApprovalAction::Decide {
            id: id.clone(),
            decision: CliApprovalDecision::AllowOnce,
        })
    }
    fn lookup(&mut self, id: &CliApprovalId, receipt: Option<CliApprovalReceipt>) {
        let req = self.request(CliApprovalAction::Lookup { id: id.clone() });
        assert_eq!(
            self.response(&req, CliApprovalResult::Receipt(receipt)),
            CliApprovalApply::Applied
        );
    }
}
fn receipt(r: &CliApprovalReview, state: CliApprovalReceiptState) -> CliApprovalReceipt {
    CliApprovalReceipt {
        id: r.id.clone(),
        state,
        decision: match state {
            CliApprovalReceiptState::Pending
            | CliApprovalReceiptState::Expired
            | CliApprovalReceiptState::Revoked => None,
            CliApprovalReceiptState::Denied => Some(CliApprovalDecision::Deny),
            _ => Some(CliApprovalDecision::AllowOnce),
        },
    }
}
#[test]
fn uncertain_decision_never_becomes_another_allow_from_refresh_or_lookup() {
    let mut h = Harness::new();
    let r = h.ready();
    let req = h.choose(&r.id);
    assert_eq!(
        h.state.items(&h.context)[0].status,
        CliApprovalStatus::Submitting
    );
    assert!(!h.state.availability(&h.context, Some(&r.id)).decide);
    assert!(
        h.state
            .request(&h.context, OperationId(999), req.action.clone())
            .is_none()
    );
    h.response(&req, CliApprovalResult::Failed);
    for result in [
        None,
        Some(receipt(&r, CliApprovalReceiptState::Pending)),
        Some(receipt(&r, CliApprovalReceiptState::Granted)),
    ] {
        h.lookup(&r.id, result);
        assert_eq!(
            h.state.items(&h.context)[0].status,
            CliApprovalStatus::Unknown
        );
        assert!(!h.state.availability(&h.context, Some(&r.id)).decide);
        assert_eq!(h.list(vec![r.clone()]), CliApprovalApply::Applied);
        assert!(h.state.items(&h.context)[0].review.is_none());
    }
    h.lookup(&r.id, Some(receipt(&r, CliApprovalReceiptState::Consumed)));
    assert_eq!(
        h.state.items(&h.context)[0].status,
        CliApprovalStatus::Recorded(CliApprovalReceiptState::Consumed)
    );
    h.lookup(&r.id, None);
    assert_eq!(
        h.state.items(&h.context)[0].status,
        CliApprovalStatus::Recorded(CliApprovalReceiptState::Consumed)
    );
    assert!(!h.state.availability(&h.context, Some(&r.id)).decide);
}
#[test]
fn deny_is_single_attempt_and_conflicting_or_invalid_receipts_fail_closed() {
    for state in [
        CliApprovalReceiptState::Denied,
        CliApprovalReceiptState::Consumed,
        CliApprovalReceiptState::Granted,
    ] {
        let mut h = Harness::new();
        let r = h.ready();
        let req = h.request(CliApprovalAction::Decide {
            id: r.id.clone(),
            decision: CliApprovalDecision::Deny,
        });
        let applied = h.response(&req, CliApprovalResult::Receipt(Some(receipt(&r, state))));
        if state == CliApprovalReceiptState::Denied {
            assert_eq!(applied, CliApprovalApply::Applied);
            assert_eq!(
                h.state.items(&h.context)[0].status,
                CliApprovalStatus::Recorded(state)
            );
        } else {
            assert_eq!(applied, CliApprovalApply::Invalid);
            assert_eq!(
                h.state.items(&h.context)[0].status,
                CliApprovalStatus::Unknown
            );
        }
        assert!(!h.state.availability(&h.context, Some(&r.id)).decide);
        assert!(h.state.availability(&h.context, Some(&r.id)).lookup);
    }
}
#[test]
fn soft_retirement_preserves_read_only_work_but_attachment_invalidation_drops_it() {
    let mut h = Harness::new();
    let r = h.ready();
    let decision = h.choose(&r.id);
    h.state.retire(&h.context);
    assert_eq!(
        h.response(
            &decision,
            CliApprovalResult::Receipt(Some(receipt(&r, CliApprovalReceiptState::Consumed)))
        ),
        CliApprovalApply::Ignored
    );
    let lookup = h.request(CliApprovalAction::Lookup { id: r.id.clone() });
    assert!(!h.state.retire(&h.context));
    assert_eq!(
        h.response(
            &lookup,
            CliApprovalResult::Receipt(Some(receipt(&r, CliApprovalReceiptState::Consumed)))
        ),
        CliApprovalApply::Applied
    );
    let lookup = h.request(CliApprovalAction::Lookup { id: r.id.clone() });
    assert!(h.state.invalidate(&h.context));
    assert_eq!(
        h.response(&lookup, CliApprovalResult::Receipt(None)),
        CliApprovalApply::Ignored
    );
    assert!(h.state.availability(&h.context, Some(&r.id)).lookup);
}
#[test]
fn clock_failure_does_not_starve_read_only_recovery_or_allow_new_bindings() {
    let mut h = Harness::new();
    let r = h.ready();
    h.choose(&r.id);
    h.state.tick(999);
    let lookup = h.request(CliApprovalAction::Lookup { id: r.id.clone() });
    h.state.tick(998);
    assert_eq!(
        h.response(
            &lookup,
            CliApprovalResult::Receipt(Some(receipt(&r, CliApprovalReceiptState::Consumed)))
        ),
        CliApprovalApply::Applied
    );
    assert!(!h.state.bind(context(2)));
    assert!(!h.state.availability(&h.context, Some(&r.id)).decide);
}
#[test]
fn every_context_dimension_and_operation_is_checked_before_consuming_reply() {
    let mut h = Harness::new();
    let req = h.request(CliApprovalAction::List);
    let result = CliApprovalResult::Listed {
        run: Some(h.context.run.clone()),
        reviews: vec![review(&h.context, 1)],
    };
    for field in 0..9 {
        let mut event = CliApprovalResponse {
            context: h.context.clone(),
            op: req.op,
            result: result.clone(),
        };
        match field {
            0 => event.context.conversation = ConversationId(2),
            1 => event.context.generation += 1,
            2 => event.context.run_op = OperationId(11),
            3 => event.op = OperationId(12),
            4 => event.context.run.epoch += 1,
            5 => event.context.run.session.push('x'),
            6 => event.context.run.binding_generation += 1,
            7 => event.context.run.op.push('x'),
            _ => event.context.run.prompt += 1,
        }
        assert_eq!(h.state.apply(event), CliApprovalApply::Ignored);
        assert!(h.state.items(&h.context).is_empty());
    }
    assert_eq!(h.response(&req, result.clone()), CliApprovalApply::Applied);
    assert_eq!(h.response(&req, result), CliApprovalApply::Ignored);
    assert!(
        h.state
            .request(&h.context, req.op, CliApprovalAction::List)
            .is_none()
    );
}
#[test]
fn invalid_complete_lists_never_partially_install_or_truncate() {
    for bad in 0..8 {
        let mut h = Harness::new();
        let old = h.ready();
        let mut changed = old.clone();
        match bad {
            0 => changed.arguments.push('\u{202e}'),
            1 => changed.arguments.push('\r'),
            2 => changed.arguments = "a".repeat(CLI_APPROVAL_DISPLAY_BYTES + 1),
            3 => changed.id.arguments_sha256 = "cd".repeat(32),
            4 => changed.expires_at_ms += 1,
            5 => changed.arguments = "changed content with same asserted digest".into(),
            6 => changed.id.run.prompt += 1,
            _ => changed.id.invocation = "invalid".into(),
        }
        assert_eq!(
            h.list(vec![review(&h.context, 2), changed]),
            CliApprovalApply::Invalid
        );
        assert_eq!(h.state.items(&h.context).len(), 1);
        assert!(h.state.items(&h.context)[0].review.is_none());
        assert!(!h.state.availability(&h.context, Some(&old.id)).decide);
    }
    let mut h = Harness::new();
    let r = review(&h.context, 1);
    assert_eq!(h.list(vec![r.clone(), r]), CliApprovalApply::Invalid);
    assert!(h.state.items(&h.context).is_empty());
}
#[test]
fn wrong_receipt_cannot_resolve_a_decision_and_retirement_preserves_uncertainty() {
    let mut h = Harness::new();
    let r = h.ready();
    let req = h.choose(&r.id);
    let mut wrong = receipt(&r, CliApprovalReceiptState::Consumed);
    wrong.id.invocation = "ef".repeat(32);
    assert_eq!(
        h.response(&req, CliApprovalResult::Receipt(Some(wrong))),
        CliApprovalApply::Invalid
    );
    assert_eq!(
        h.state.items(&h.context)[0].status,
        CliApprovalStatus::Unknown
    );
    assert!(h.state.availability(&h.context, Some(&r.id)).lookup);
    h.lookup(&r.id, Some(receipt(&r, CliApprovalReceiptState::Consumed)));
    assert!(!h.state.availability(&h.context, None).list);
}
#[test]
fn disappearance_expiry_and_clock_regression_cannot_revive_reviews() {
    let mut h = Harness::new();
    let r = h.ready();
    assert_eq!(h.list(vec![]), CliApprovalApply::Applied);
    h.list(vec![r.clone()]);
    assert!(!h.state.availability(&h.context, Some(&r.id)).decide);
    let next = review(&h.context, 2);
    h.list(vec![next.clone()]);
    h.state.tick(10000);
    assert!(!h.state.availability(&h.context, Some(&next.id)).decide);
    assert!(h.state.items(&h.context).iter().all(|r| r.review.is_none()));
    h.state.tick(9999);
    assert!(h.state.clock_failed());
    assert!(!h.state.availability(&h.context, None).list);
    assert!(!h.state.bind(context(2)));
}
#[test]
fn reattachment_is_read_only_and_new_runs_cannot_discard_unknown_decisions() {
    let mut h = Harness::new();
    let r = h.ready();
    let req = h.choose(&r.id);
    let mut reattached = h.context.clone();
    reattached.generation += 1;
    assert!(h.state.bind(reattached.clone()));
    assert_eq!(
        h.response(
            &req,
            CliApprovalResult::Receipt(Some(receipt(&r, CliApprovalReceiptState::Consumed)))
        ),
        CliApprovalApply::Ignored
    );
    h.context = reattached;
    assert!(!h.state.availability(&h.context, Some(&r.id)).decide);
    let mut newer = h.context.clone();
    newer.run.prompt += 1;
    newer.run.op = "next-op".into();
    newer.run_op = OperationId(20);
    assert!(!h.state.bind(newer.clone()));
    h.lookup(&r.id, Some(receipt(&r, CliApprovalReceiptState::Consumed)));
    assert!(h.state.bind(newer));
    assert!(!h.state.bind(h.context.clone()));
}
#[test]
fn capacities_fail_closed_without_eviction_and_resolved_sequential_runs_reuse_a_slot() {
    let mut h = Harness::new();
    for chunk in 0..8 {
        let reviews = (0..16)
            .map(|n| review(&h.context, chunk * 16 + n))
            .collect();
        assert_eq!(h.list(reviews), CliApprovalApply::Applied);
    }
    assert_eq!(h.state.items(&h.context).len(), CLI_APPROVAL_REQUESTS);
    assert_eq!(
        h.list(vec![review(&h.context, 128)]),
        CliApprovalApply::Invalid
    );
    assert_eq!(h.state.items(&h.context).len(), CLI_APPROVAL_REQUESTS);
    for n in 2..=CLI_APPROVAL_CONTEXTS as u64 {
        assert!(h.state.bind(context(n)));
    }
    assert!(!h.state.bind(context(9)));
    assert_eq!(h.state.contexts().count(), CLI_APPROVAL_CONTEXTS);
    let mut h = Harness::new();
    for n in 2..300 {
        let previous = h.context.clone();
        h.context.run.prompt = n;
        h.context.run.op = format!("op-{n}");
        h.context.run_op = OperationId(10 + n);
        assert!(h.state.bind(h.context.clone()));
        assert!(!h.state.bind(previous));
        assert_eq!(h.state.contexts().count(), 1);
    }
}
#[test]
fn exact_limit_full_display_is_retained_and_debug_is_redacted() {
    let mut h = Harness::new();
    let mut r = review(&h.context, 1);
    r.arguments = "z".repeat(CLI_APPROVAL_DISPLAY_BYTES);
    assert_eq!(h.list(vec![r.clone()]), CliApprovalApply::Applied);
    assert_eq!(
        h.state.items(&h.context)[0]
            .review
            .as_ref()
            .unwrap()
            .arguments,
        r.arguments
    );
    assert!(!format!("{:?}", h.state).contains(&r.arguments));
    assert!(!format!("{:?}", h.state).contains(&r.id.invocation));
    let mut h = Harness::new();
    assert_eq!(
        h.list((0..17).map(|n| review(&h.context, n)).collect()),
        CliApprovalApply::Invalid
    );
    assert!(h.state.items(&h.context).is_empty());
}
#[test]
fn app_state_retires_on_scope_loss_not_selection_and_disconnect_never_restores_allow() {
    for reason in 0..6 {
        let mut s = AppState::new(
            Bootstrap {
                projects: vec![],
                models: vec![],
                conversations: vec![
                    (ConversationId(1), ProjectId(1), "A".into(), 1),
                    (ConversationId(2), ProjectId(1), "B".into(), 1),
                ],
                now: 1,
            },
            Prefs::default(),
        );
        s.mode = Mode::Real;
        let mut h = Harness::new();
        let c = &mut s.conversations[0];
        c.generation = 1;
        c.opened = true;
        c.run = RunState::Running {
            op: h.context.run_op,
        };
        s.apply_event(BackendEvent {
            conversation: ConversationId(1),
            generation: 1,
            op: None,
            kind: EventKind::ExecutionContext {
                profile: ExecutionProfile::LocalCliRestrictedWrite,
                execution_blocked: None,
            },
        });
        let r = h.ready();
        let context = h.context.clone();
        s.cli_approvals = h.state;
        s.dispatch(Command::SelectConversation(ConversationId(2)));
        assert!(s.cli_approvals.availability(&context, Some(&r.id)).decide);
        match reason {
            0 => {
                s.set_connection(Connection::Offline("lost".into()));
                s.set_connection(Connection::Ready);
            }
            1 => s.conversations[0].generation += 1,
            2 => s.conversations[0].run = RunState::Stopping { op: context.run_op },
            3 => {
                s.apply_event(BackendEvent {
                    conversation: ConversationId(1),
                    generation: 1,
                    op: None,
                    kind: EventKind::ExecutionContext {
                        profile: ExecutionProfile::LocalCliRestrictedWrite,
                        execution_blocked: Some("revoked".into()),
                    },
                });
            }
            4 => s.read_only = Some("read only".into()),
            _ => {
                s.apply_event(BackendEvent {
                    conversation: ConversationId(1),
                    generation: 1,
                    op: None,
                    kind: EventKind::ExecutionContext {
                        profile: ExecutionProfile::LocalCliChatOnly,
                        execution_blocked: None,
                    },
                });
            }
        }
        s.dispatch(Command::SelectConversation(ConversationId(2)));
        assert!(!s.cli_approvals.availability(&context, Some(&r.id)).decide);
        assert!(s.cli_approvals.items(&context)[0].review.is_none());
    }
}
