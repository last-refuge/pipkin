use super::*;
#[test]
fn restart_preserves_unknown_and_known_attempts_without_owner_upgrade_or_resend() {
    for resolve in [false, true] {
        let fx = Fixture::start(|_, _| panic!("native inference forbidden"));
        let executable = install(&fx);
        let mut h = setup(&fx, &executable);
        h.send("synthetic prompt");
        let (context, review) = discover(&mut h);
        fx.faults.drop_approval_reply.store(true, Ordering::SeqCst);
        act(
            &mut h,
            &context,
            A::Decide {
                id: review.id.clone(),
                decision: D::AllowOnce,
            },
        );
        h.until("uncertain decision", |s| {
            s.cli_approvals
                .items(&context)
                .iter()
                .any(|i| i.status == S::Unknown)
        });
        h.until("external run settled independently", is_idle);
        if resolve {
            act(
                &mut h,
                &context,
                A::Lookup {
                    id: review.id.clone(),
                },
            );
            h.until("original owner receipt recovered", |s| {
                s.cli_approvals
                    .items(&context)
                    .iter()
                    .any(|i| i.status == S::Recorded(R::Consumed))
            });
        }
        let storage = h.storage.clone();
        h.until("durable recovery matches observed knowledge", |_| {
            storage
                .load_cli_approvals()
                .unwrap()
                .iter()
                .any(|r| r.id == review.id && r.receipt == resolve.then_some(R::Consumed))
        });
        assert_eq!(fx.faults.approval_decisions.load(Ordering::SeqCst), 1);
        let lookups = fx.faults.approval_lookups.load(Ordering::SeqCst);
        h.stop();
        fx.restart_engine();
        let mut restored = Harness::start(&fx, &fx.root.join("desktop.db"));
        restored.until("read-only reopened coding conversation", |s| {
            s.connection.is_ready()
                && s.current().is_some_and(|c| {
                    c.opened && c.profile == pipkin_core::ExecutionProfile::LocalCliRestrictedWrite
                })
        });
        let saved = restored
            .state
            .cli_approval_context(context.conversation)
            .unwrap()
            .clone();
        assert_eq!(saved.run, context.run);
        let expected = if resolve {
            S::Recorded(R::Consumed)
        } else {
            S::Unknown
        };
        assert_eq!(
            restored.state.cli_approvals.items(&saved)[0].status,
            expected
        );
        assert!(
            restored.state.cli_approvals.items(&saved)[0]
                .review
                .is_none()
        );
        assert_eq!(
            fx.faults.approval_lookups.load(Ordering::SeqCst),
            lookups,
            "no automatic lookup"
        );
        restored.dispatch(Command::CliApproval(C::Reattach(context.conversation)));
        let rebound = restored
            .state
            .cli_approval_context(context.conversation)
            .unwrap()
            .clone();
        assert!(
            restored
                .state
                .cli_approval_availability(&rebound, Some(&review.id))
                .lookup
        );
        act(
            &mut restored,
            &rebound,
            A::Lookup {
                id: review.id.clone(),
            },
        );
        restored.until("lookup fails closed without original owner proof", |s| {
            s.cli_approval_availability(&rebound, Some(&review.id))
                .lookup
        });
        assert_eq!(
            fx.faults.approval_lookups.load(Ordering::SeqCst),
            lookups,
            "no cross-owner archived lookup"
        );
        assert_eq!(
            restored.state.cli_approvals.items(&rebound)[0].status,
            expected
        );
        for decision in [D::AllowOnce, D::Deny] {
            act(
                &mut restored,
                &rebound,
                A::Decide {
                    id: review.id.clone(),
                    decision,
                },
            );
        }
        assert_eq!(fx.faults.approval_decisions.load(Ordering::SeqCst), 1);
        assert_eq!(launch_count(&fx), 1);
        assert!(fx.requests().is_empty());
        restored.stop();
    }
}
#[test]
fn failed_attempt_journal_sends_no_decision_and_stop_still_works() {
    let fx = Fixture::start(|_, _| panic!("native inference forbidden"));
    let executable = install(&fx);
    let mut h = setup(&fx, &executable);
    h.send("synthetic prompt");
    let (context, review) = discover(&mut h);
    h.storage.set_write_failure(true);
    act(
        &mut h,
        &context,
        A::Decide {
            id: review.id.clone(),
            decision: D::AllowOnce,
        },
    );
    assert_eq!(h.state.cli_approvals.items(&context)[0].status, S::Unknown);
    assert_eq!(fx.faults.approval_decisions.load(Ordering::SeqCst), 0);
    assert!(h.storage.load_cli_approvals().unwrap().is_empty());
    h.storage.set_write_failure(false);
    act(
        &mut h,
        &context,
        A::Decide {
            id: review.id,
            decision: D::AllowOnce,
        },
    );
    assert_eq!(fx.faults.approval_decisions.load(Ordering::SeqCst), 0);
    h.dispatch(Command::Cancel);
    h.until("stop after failed journal", is_idle);
    assert!(!fx.project.join("effect").exists());
    assert_eq!(launch_count(&fx), 1);
    assert!(fx.requests().is_empty());
    h.stop();
}
