use super::*;
fn pending(s: &mut AppState) -> CliApprovalRequest {
    let q = discover(s, 1);
    let r = install(s, &q);
    let context = s.cli_approval_context(ConversationId(1)).unwrap().clone();
    let out = s.dispatch(Command::CliApproval(CliApprovalCommand::Act {
        context,
        action: Box::new(CliApprovalAction::Decide {
            id: r.id,
            decision: CliApprovalDecision::AllowOnce,
        }),
    }));
    assert_eq!(out.effects.len(), 1);
    let Effect::JournalCliApproval(request) = out.effects.into_iter().next().unwrap() else {
        panic!("journal before decision")
    };
    *request
}
#[test]
fn only_matching_live_durable_ack_can_release_one_decision() {
    let mut s = state();
    let r = pending(&mut s);
    let id = CliApprovalRecovery::attempt(&r).unwrap().id;
    result(
        &mut s,
        &CliApprovalPortRequest::Known(r.clone()),
        CliApprovalResult::Receipt(Some(CliApprovalReceipt {
            id: id.clone(),
            state: CliApprovalReceiptState::Consumed,
            decision: Some(CliApprovalDecision::AllowOnce),
        })),
    );
    assert_eq!(
        s.cli_approvals.items(&r.context)[0].status,
        CliApprovalStatus::Submitting
    );
    let mut wrong = r.clone();
    wrong.context.generation += 1;
    assert!(s.cli_approval_persisted(&wrong, Ok(())).effects.is_empty());
    let released = request(s.cli_approval_persisted(&r, Ok(())));
    assert_eq!(released, CliApprovalPortRequest::Known(r.clone()));
    assert!(s.cli_approval_persisted(&r, Ok(())).effects.is_empty());
    let out = event(
        &mut s,
        &released,
        CliApprovalEvent::Response(CliApprovalResponse {
            context: r.context.clone(),
            op: r.op,
            result: CliApprovalResult::Receipt(Some(CliApprovalReceipt {
                id,
                state: CliApprovalReceiptState::Consumed,
                decision: Some(CliApprovalDecision::AllowOnce),
            })),
        }),
    );
    let [Effect::SaveCliApprovalRecovery(saved)] = out.effects.as_slice() else {
        panic!()
    };
    assert_eq!(saved.receipt, Some(CliApprovalReceiptState::Consumed));
    assert!(saved.valid());
    assert!(
        !s.cli_approval_receipt_persisted(saved, Err("disk".into()))
            .notes
            .is_empty()
    );
    assert!(s.notice.as_ref().unwrap().contains("could not be saved"));
    let mut stale = (**saved).clone();
    stale.context.generation += 1;
    assert!(
        s.cli_approval_receipt_persisted(&stale, Err("disk".into()))
            .notes
            .is_empty()
    );
    assert_eq!(
        s.cli_approvals.items(&r.context)[0].status,
        CliApprovalStatus::Recorded(CliApprovalReceiptState::Consumed)
    );
}
#[test]
fn failed_expired_disconnected_readonly_and_retired_ack_never_release_or_rearm() {
    for case in 0..6 {
        let mut s = state();
        let r = pending(&mut s);
        let result = match case {
            0 => Err("disk failure".into()),
            1 => {
                s.set_now(11);
                Ok(())
            }
            2 => {
                s.set_connection(Connection::Reconnecting);
                Ok(())
            }
            3 => {
                s.read_only = Some("storage fault".into());
                Ok(())
            }
            4 => {
                s.cli_approvals.invalidate(&r.context);
                Ok(())
            }
            _ => {
                s.set_cli_approvals_available(false);
                Ok(())
            }
        };
        assert!(s.cli_approval_persisted(&r, result).effects.is_empty());
        let item = &s.cli_approvals.items(&r.context)[0];
        assert_eq!(item.status, CliApprovalStatus::Unknown);
        assert!(item.attempted.is_some());
        assert!(
            !s.cli_approvals
                .availability(&r.context, Some(&item.id))
                .decide
        );
    }
}
#[test]
fn restored_attempts_have_no_plaintext_authority_or_automatic_requests_and_keep_the_bound() {
    let mut original = state();
    let r = pending(&mut original);
    let record = CliApprovalRecovery::attempt(&r).unwrap();
    let mut records = vec![];
    for n in 1..=CLI_APPROVAL_CONTEXTS {
        let mut row = record.clone();
        row.context.conversation = ConversationId(n as u64);
        row.context.run.session = format!("s{n}");
        row.id.run = row.context.run.clone();
        records.push(row);
    }
    let mut restored = state();
    restored.restore_cli_approval_recovery(Ok(records));
    assert!(!restored.cli_approvals.recovery_unavailable());
    assert_eq!(
        restored.cli_approvals.contexts().count(),
        CLI_APPROVAL_CONTEXTS
    );
    for context in restored.cli_approvals.contexts() {
        assert_eq!(context.generation, 0);
        let item = &restored.cli_approvals.items(context)[0];
        assert!(item.review.is_none());
        assert_eq!(item.status, CliApprovalStatus::Unknown);
        assert!(
            !restored
                .cli_approvals
                .availability(context, Some(&item.id))
                .decide
        );
    }
    assert!(!restored.cli_approval_discovery_available(ConversationId(9)));
    assert!(
        restored
            .dispatch(Command::CliApproval(CliApprovalCommand::Reattach(
                ConversationId(1)
            )))
            .effects
            .is_empty()
    );
    let rebound = restored
        .cli_approval_context(ConversationId(1))
        .unwrap()
        .clone();
    assert_eq!(rebound.run, r.context.run);
    let out = act(
        &mut restored,
        &rebound,
        CliApprovalAction::Lookup { id: record.id },
    );
    let CliApprovalPortRequest::Known(lookup) = request(out) else {
        panic!()
    };
    assert!(lookup.op.0 > r.context.run_op.0);
}
#[test]
fn only_resolved_restored_scopes_allow_a_separately_verified_new_run() {
    let mut original = state();
    let attempted = pending(&mut original);
    for resolved in [false, true] {
        let mut record = CliApprovalRecovery::attempt(&attempted).unwrap();
        record.receipt = resolved.then_some(CliApprovalReceiptState::Consumed);
        let mut s = state();
        s.restore_cli_approval_recovery(Ok(vec![record]));
        s.conversations[0].run = RunState::Running {
            op: OperationId(200),
        };
        if !resolved {
            assert!(!s.cli_approval_discovery_available(ConversationId(1)));
            continue;
        }
        let req = discover(&mut s, 1);
        let CliApprovalPortRequest::Discover(q) = &req else {
            panic!()
        };
        let mut next = review(q);
        next.id.run.prompt += 1;
        event(
            &mut s,
            &req,
            CliApprovalEvent::Discovered {
                query: q.clone(),
                run: Some(next.id.run.clone()),
                reviews: vec![next.clone()],
            },
        );
        let context = s.cli_approval_context(ConversationId(1)).unwrap();
        assert_eq!(context.run, next.id.run);
        assert!(s.cli_approval_availability(context, Some(&next.id)).decide);
    }
}

#[test]
fn bad_journal_fails_closed_and_known_receipts_restore_as_non_authoritative_metadata() {
    let mut s = state();
    let r = pending(&mut s);
    let record = CliApprovalRecovery::attempt(&r).unwrap();
    for bad in [
        Err("unreadable".into()),
        Ok(vec![record.clone(), record.clone()]),
    ] {
        let mut restored = state();
        restored.restore_cli_approval_recovery(bad);
        assert!(restored.cli_approvals.recovery_unavailable());
        assert!(!restored.cli_approval_discovery_available(ConversationId(1)));
    }
    let mut known = record;
    known.receipt = Some(CliApprovalReceiptState::Consumed);
    let mut restored = state();
    restored.restore_cli_approval_recovery(Ok(vec![known]));
    let context = restored.cli_approval_context(ConversationId(1)).unwrap();
    assert_eq!(
        restored.cli_approvals.items(context)[0].status,
        CliApprovalStatus::Recorded(CliApprovalReceiptState::Consumed)
    );
    assert!(
        !restored
            .cli_approvals
            .availability(context, Some(&restored.cli_approvals.items(context)[0].id))
            .decide
    );
}
