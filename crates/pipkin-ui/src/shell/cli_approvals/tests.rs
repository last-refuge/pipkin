use super::*;
use gpui::{TestAppContext, VisualTestContext};
use pipkin_core::*;
use std::{cell::RefCell, rc::Rc};

fn fixture() -> (AppState, CliApprovalContext, CliApprovalReview) {
    let mut s = AppState::new(
        Bootstrap {
            projects: vec![Project {
                id: ProjectId(1),
                name: "fixture".into(),
                path: "/captured/project".into(),
            }],
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
    s.selected = Some(ConversationId(1));
    for c in &mut s.conversations {
        c.opened = true;
        c.generation = 1;
        c.run = RunState::Running {
            op: OperationId(100 + c.id.0),
        };
    }
    for id in 1..=2 {
        s.apply_event(BackendEvent {
            conversation: ConversationId(id),
            generation: 1,
            op: None,
            kind: EventKind::ExecutionContext {
                profile: ExecutionProfile::LocalCliRestrictedWrite,
                execution_blocked: None,
            },
        });
    }
    s.set_cli_approvals_available(true);
    let out = s.dispatch(Command::CliApproval(C::Discover(ConversationId(1))));
    let q = out
        .effects
        .into_iter()
        .find_map(|e| match e {
            Effect::Backend(BackendRequest::CliApproval(CliApprovalPortRequest::Discover(q))) => {
                Some(q)
            }
            _ => None,
        })
        .unwrap();
    let run = CliApprovalRun {
        epoch: 1,
        session: "engine-session".into(),
        binding_generation: 9,
        op: "external-op".into(),
        prompt: 41,
    };
    let review = CliApprovalReview {
        id: CliApprovalId {
            run: run.clone(),
            invocation: "ab".repeat(32),
            arguments_sha256: "cd".repeat(32),
        },
        arguments: {
            let mut text = String::from(
                "Write\nfile_path: \"/captured/project/a\"\ncontent: \"[untrusted link](https://example.invalid) <script>not executable</script> ",
            );
            text.push_str(&"x".repeat(CLI_APPROVAL_DISPLAY_BYTES - text.len() - 1));
            text.push('"');
            text
        },
        expires_at_ms: 10000,
    };
    s.apply_event(BackendEvent {
        conversation: q.conversation,
        generation: q.generation,
        op: Some(q.op),
        kind: EventKind::CliApproval(Box::new(CliApprovalEvent::Discovered {
            query: q,
            run: Some(run),
            reviews: vec![review.clone()],
        })),
    });
    let context = s.cli_approval_context(ConversationId(1)).unwrap().clone();
    (s, context, review)
}
fn last_request(
    effects: &Rc<RefCell<Vec<Effect>>>,
    w: &Workspace,
    cx: &mut Context<Workspace>,
) -> CliApprovalRequest {
    let queued: Vec<_> = effects.borrow_mut().drain(..).collect();
    for effect in queued {
        match effect {
            Effect::Backend(BackendRequest::CliApproval(CliApprovalPortRequest::Known(r))) => {
                return r;
            }
            Effect::JournalCliApproval(r) => {
                w.model.update(cx, |m, cx| {
                    m.mutate(|s| s.cli_approval_persisted(&r, Ok(())), cx)
                });
                return last_request(effects, w, cx);
            }
            _ => {}
        }
    }
    panic!("exact approval request")
}
fn finish(req: &CliApprovalRequest, result: CliApprovalResult) -> BackendEvent {
    BackendEvent {
        conversation: req.context.conversation,
        generation: req.context.generation,
        op: Some(req.op),
        kind: EventKind::CliApproval(Box::new(CliApprovalEvent::Response(CliApprovalResponse {
            context: req.context.clone(),
            op: req.op,
            result,
        }))),
    }
}

#[gpui::test]
fn keyboard_review_navigation_scrolls_full_arguments_without_authority(cx: &mut TestAppContext) {
    let (s, _, review) = fixture();
    let effects = Rc::new(RefCell::new(vec![]));
    let captured = effects.clone();
    let window = cx.add_window(|window, cx| {
        let model = cx.new(|_| {
            let mut m = crate::model::Model::new(s);
            m.set_effect_handler(Box::new(move |e, _| captured.borrow_mut().push(e)));
            m
        });
        model.update(cx, |m, cx| m.explore_workspace(cx));
        Workspace::new(model, Default::default(), window, cx)
    });
    let root = window.root(cx).unwrap();
    let cx = &mut VisualTestContext::from_window(window.into(), cx);
    root.update_in(cx, |w, window, cx| w.open_cli_approvals(window, cx));
    cx.run_until_parked();
    effects.borrow_mut().clear();
    root.update_in(cx, |w, window, _| {
        let bounds = w.cli_dialog_scroll.bounds();
        assert!(
            bounds.origin.y + bounds.size.height <= window.viewport_size().height,
            "review dialog must fit vertically: {bounds:?}"
        );
    });
    cx.simulate_keystrokes("ctrl-shift-c");
    cx.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some(review.arguments.as_str())
        )
    });
    let mut reached_arguments = false;
    for _ in 0..30 {
        cx.simulate_keystrokes("tab");
        reached_arguments |= root.update_in(cx, |w, window, cx| {
            assert!(w.menu_focus.contains_focused(window, cx));
            w.cli_review_focus.is_focused(window)
        });
    }
    assert!(
        reached_arguments,
        "full arguments must actually be reachable by Tab, not only direct focus"
    );
    root.update_in(cx, |w, window, cx| window.focus(&w.cli_review_focus, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("pagedown");
    cx.run_until_parked();
    root.update(cx, |w, cx| {
        assert!(w.cli_review_scroll.offset().y < px(0.));
        assert_eq!(w.cli_approval_text.read(cx).text.as_ref(), review.arguments);
    });
    cx.simulate_keystrokes("home enter");
    cx.run_until_parked();
    root.update(cx, |w, _| {
        assert_eq!(w.cli_review_scroll.offset().y, px(0.))
    });
    assert!(
        effects.borrow().is_empty(),
        "navigation must never probe, decide or journal"
    );
    root.update(cx, |w, cx| {
        w.model.update(cx, |m, cx| {
            m.state.set_now(10001);
            cx.notify();
        })
    });
    cx.run_until_parked();
    // Headless GPUI has no platform frame loop. Deliver the two frame callbacks
    // with the changed focus tree rendered between them, as native GPUI does.
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
        window.simulate_next_frame(cx);
    });
    root.update_in(cx, |w, window, cx| {
        assert!(w.cli_approval_text.read(cx).text.is_empty());
        assert!(
            w.menu_focus.is_focused(window),
            "expiry must not strand focus on removed arguments"
        );
    });
    cx.update(|_, cx| {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string("expired sentinel".into()))
    });
    cx.simulate_keystrokes("ctrl-shift-c space enter tab");
    cx.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some("expired sentinel")
        )
    });
    assert!(
        effects.borrow().is_empty(),
        "focus recovery must not perform any action"
    );
}

#[gpui::test]
fn full_inert_review_is_captured_copied_and_never_submitted_by_enter(cx: &mut TestAppContext) {
    let (s, context, review) = fixture();
    let effects = Rc::new(RefCell::new(vec![]));
    let captured = effects.clone();
    let window = cx.add_window(|window, cx| {
        let model = cx.new(|_| {
            let mut m = crate::model::Model::new(s);
            m.set_effect_handler(Box::new(move |e, _| captured.borrow_mut().push(e)));
            m
        });
        Workspace::new(model, Default::default(), window, cx)
    });
    let root = window.root(cx).unwrap();
    let cx = &mut VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    effects.borrow_mut().clear();
    root.update_in(cx, |w, window, cx| {
        w.open_cli_approvals(window, cx);
        w.confirm_selection(window, cx);
        w.open_overlay(Overlay::Prefs, window, cx);
        assert!(w.cli_approval_text.read(cx).text.is_empty());
        assert!(
            w.state(cx)
                .cli_approval_availability(&context, Some(&review.id))
                .decide,
            "hiding presentation does not reset or consume core authority"
        );
        w.open_cli_approvals(window, cx);
    });
    cx.run_until_parked();
    root.update(cx, |w, cx| {
        assert!(
            effects.borrow().is_empty(),
            "opening/rendering/Enter never request or decide"
        );
        assert_eq!(w.cli_approval_text.read(cx).text.as_ref(), review.arguments);
        assert!(w.cli_approval_panel.heading.contains("/captured/project"));
        w.model.update(cx, |m, cx| {
            m.state.selected = Some(ConversationId(2));
            cx.notify();
        });
        w.copy_review(&context, &review.id, cx);
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some(review.arguments.as_str())
        );
        assert!(
            w.state(cx)
                .cli_approval_stop_command(
                    context.conversation,
                    context.generation,
                    Some(context.run_op)
                )
                .is_none()
        );
        w.approval_action(
            context.clone(),
            CliApprovalAction::Decide {
                id: review.id.clone(),
                decision: D::AllowOnce,
            },
            cx,
        );
        let req = last_request(&effects, w, cx);
        assert_eq!(req.context, context);
        assert!(matches!(
            req.action,
            CliApprovalAction::Decide {
                decision: D::AllowOnce,
                ..
            }
        ));
        w.model.update(cx, |m, cx| {
            m.apply_event(finish(&req, CliApprovalResult::Failed), cx)
        });
    });
    cx.run_until_parked();
    root.update(cx, |w, cx| {
        assert!(w.cli_approval_text.read(cx).text.is_empty());
        assert!(w.state(cx).conversations.iter().all(|c| c.items.is_empty()));
    });
}

#[gpui::test]
fn uncertain_decision_survives_close_and_explicit_lookup_never_reenables_it(
    cx: &mut TestAppContext,
) {
    let (s, context, review) = fixture();
    let effects = Rc::new(RefCell::new(vec![]));
    let captured = effects.clone();
    let window = cx.add_window(|window, cx| {
        let model = cx.new(|_| {
            let mut m = crate::model::Model::new(s);
            m.set_effect_handler(Box::new(move |e, _| captured.borrow_mut().push(e)));
            m
        });
        Workspace::new(model, Default::default(), window, cx)
    });
    let root = window.root(cx).unwrap();
    let cx = &mut VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    effects.borrow_mut().clear();
    root.update_in(cx, |w, window, cx| {
        w.open_cli_approvals(window, cx);
        w.approval_action(
            context.clone(),
            CliApprovalAction::Decide {
                id: review.id.clone(),
                decision: D::Deny,
            },
            cx,
        );
        let req = last_request(&effects, w, cx);
        w.close_overlay(window, cx);
        assert!(w.cli_approval_text.read(cx).text.is_empty());
        assert_eq!(
            w.state(cx).cli_approvals.items(&context)[0].attempted,
            Some(D::Deny)
        );
        w.model.update(cx, |m, cx| {
            m.apply_event(finish(&req, CliApprovalResult::Failed), cx)
        });
        w.open_cli_approvals(window, cx);
        assert!(effects.borrow().is_empty());
        w.approval_action(
            context.clone(),
            CliApprovalAction::Decide {
                id: review.id.clone(),
                decision: D::AllowOnce,
            },
            cx,
        );
        assert!(effects.borrow().is_empty());
        w.approval_action(
            context.clone(),
            CliApprovalAction::Lookup {
                id: review.id.clone(),
            },
            cx,
        );
        let lookup = last_request(&effects, w, cx);
        assert!(matches!(lookup.action, CliApprovalAction::Lookup { .. }));
        w.model.update(cx, |m, cx| {
            m.apply_event(
                finish(
                    &lookup,
                    CliApprovalResult::Receipt(Some(CliApprovalReceipt {
                        id: review.id.clone(),
                        state: R::Denied,
                        decision: Some(D::Deny),
                    })),
                ),
                cx,
            )
        });
        assert!(
            !w.state(cx)
                .cli_approval_availability(&context, Some(&review.id))
                .decide
        );
        assert_eq!(
            w.state(cx).cli_approvals.items(&context)[0].status,
            S::Recorded(R::Denied)
        );
    });
    cx.run_until_parked();
}

#[gpui::test]
fn expiry_and_attachment_change_remove_plaintext_and_old_callbacks(cx: &mut TestAppContext) {
    let (s, context, review) = fixture();
    let window = cx.add_window(|window, cx| {
        let model = cx.new(|_| crate::model::Model::new(s));
        Workspace::new(model, Default::default(), window, cx)
    });
    let root = window.root(cx).unwrap();
    let cx = &mut VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    root.update_in(cx, |w, window, cx| {
        w.open_cli_approvals(window, cx);
        w.model.update(cx, |m, cx| {
            m.state.set_now(10001);
            cx.notify();
        });
    });
    cx.run_until_parked();
    root.update(cx, |w, cx| {
        assert!(w.cli_approval_text.read(cx).text.is_empty());
        assert!(
            !w.state(cx)
                .cli_approval_availability(&context, Some(&review.id))
                .decide
        );
        w.model.update(cx, |m, cx| {
            m.state.conversations[0].generation += 1;
            cx.notify();
        });
        assert!(w.panel_context(cx).is_none());
        assert!(!w.panel_refresh_available(cx));
        assert!(
            w.state(cx)
                .cli_approval_reattach_available(context.conversation)
        );
        w.approval_action(
            context.clone(),
            CliApprovalAction::Decide {
                id: review.id.clone(),
                decision: D::AllowOnce,
            },
            cx,
        );
        assert!(
            w.state(cx).cli_approvals.items(&context)[0]
                .attempted
                .is_none()
        );
    });
    cx.run_until_parked();
}

#[gpui::test]
fn reattachment_after_clock_failure_only_prepares_explicit_read_only_lookup(
    cx: &mut TestAppContext,
) {
    let (s, context, review) = fixture();
    let effects = Rc::new(RefCell::new(vec![]));
    let captured = effects.clone();
    let window = cx.add_window(|window, cx| {
        let model = cx.new(|_| {
            let mut m = crate::model::Model::new(s);
            m.set_effect_handler(Box::new(move |e, _| captured.borrow_mut().push(e)));
            m
        });
        Workspace::new(model, Default::default(), window, cx)
    });
    let root = window.root(cx).unwrap();
    let cx = &mut VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    effects.borrow_mut().clear();
    root.update_in(cx, |w, window, cx| {
        w.open_cli_approvals(window, cx);
        w.approval_action(
            context.clone(),
            CliApprovalAction::Decide {
                id: review.id.clone(),
                decision: D::AllowOnce,
            },
            cx,
        );
        let req = last_request(&effects, w, cx);
        w.model.update(cx, |m, cx| {
            m.apply_event(finish(&req, CliApprovalResult::Failed), cx);
            m.state.set_now(0);
            m.state.conversations[0].generation += 1;
            cx.notify();
        });
        assert!(w.panel_reattach_available(cx));
        w.reattach_receipts(w.cli_approval_panel.target.unwrap(), cx);
        assert!(
            effects.borrow().is_empty(),
            "reattach never looks up or resends"
        );
        let rebound = w.panel_context(cx).unwrap();
        assert_eq!(rebound.generation, 2);
        assert_eq!(
            w.state(cx).cli_approvals.items(&rebound)[0].attempted,
            Some(D::AllowOnce)
        );
        assert!(w.cli_approval_text.read(cx).text.is_empty());
        assert!(
            !w.state(cx)
                .cli_approval_availability(&rebound, Some(&review.id))
                .decide
        );
        w.approval_action(
            rebound.clone(),
            CliApprovalAction::Lookup {
                id: review.id.clone(),
            },
            cx,
        );
        let lookup = last_request(&effects, w, cx);
        assert_eq!(lookup.context, rebound);
        assert!(matches!(lookup.action, CliApprovalAction::Lookup { .. }));
    });
    cx.run_until_parked();
}

#[gpui::test]
fn restored_uncertainty_is_inert_and_reopening_cannot_restore_argument_text_or_decisions(
    cx: &mut TestAppContext,
) {
    let (mut s, context, review) = fixture();
    s.cli_approvals = Default::default();
    s.restore_cli_approval_recovery(Ok(vec![CliApprovalRecovery {
        context: context.clone(),
        id: review.id.clone(),
        decision: D::AllowOnce,
        receipt: None,
    }]));
    let effects = Rc::new(RefCell::new(vec![]));
    let captured = effects.clone();
    let window = cx.add_window(|window, cx| {
        let model = cx.new(|_| {
            let mut m = crate::model::Model::new(s);
            m.set_effect_handler(Box::new(move |e, _| captured.borrow_mut().push(e)));
            m
        });
        Workspace::new(model, Default::default(), window, cx)
    });
    let root = window.root(cx).unwrap();
    let cx = &mut VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    effects.borrow_mut().clear();
    root.update_in(cx, |w, window, cx| {
        w.open_cli_approvals(window, cx);
        assert!(w.cli_approval_text.read(cx).text.is_empty());
        assert!(effects.borrow().is_empty());
        assert!(w.panel_reattach_available(cx));
        w.reattach_receipts(w.cli_approval_panel.target.unwrap(), cx);
        let rebound = w.panel_context(cx).unwrap();
        for decision in [D::AllowOnce, D::Deny] {
            w.approval_action(
                rebound.clone(),
                CliApprovalAction::Decide {
                    id: review.id.clone(),
                    decision,
                },
                cx,
            );
        }
        w.close_overlay(window, cx);
        w.open_cli_approvals(window, cx);
        assert!(effects.borrow().is_empty());
        assert!(w.cli_approval_text.read(cx).text.is_empty());
        assert_eq!(
            w.state(cx).cli_approvals.items(&rebound)[0].status,
            S::Unknown
        );
    });
    cx.run_until_parked();
}

#[test]
fn widget_keys_change_with_review_and_attachment_identity() {
    let (_, context, review) = fixture();
    let key = review_key("allow", &context, &review.id);
    let mut other = context.clone();
    other.generation += 1;
    assert_ne!(key, review_key("allow", &other, &review.id));
    let mut id = review.id.clone();
    id.invocation = "ef".repeat(32);
    assert_ne!(key, review_key("allow", &context, &id));
    id = review.id.clone();
    id.arguments_sha256 = "ef".repeat(32);
    assert_ne!(key, review_key("allow", &context, &id));
    assert_ne!(key, review_key("deny", &context, &review.id));
}

#[test]
fn palette_and_receipt_labels_do_not_claim_execution() {
    let (mut s, _, _) = fixture();
    assert!(
        super::super::commands::build(&s, None)
            .iter()
            .any(|c| c.title == LABEL && c.enabled)
    );
    s.mode = Mode::Demo;
    assert!(
        !super::super::commands::build(&s, None)
            .iter()
            .any(|c| c.title == LABEL && c.enabled)
    );
    assert!(status(S::Recorded(R::Consumed)).contains("NOT proof"));
    assert!(WARNING.contains("not a sandbox"));
}

#[gpui::test]
fn dashboard_cli_review_targets_graph_thread_and_returns_to_its_dialog(cx: &mut TestAppContext) {
    let (mut s, context, review) = fixture();
    s.selected = Some(ConversationId(2));
    s.conversations[1].draft.text = "other thread draft".into();
    s.dashboard.open = true;
    s.dashboard.available = true;
    s.dashboard.freshness = dashboard::Freshness::Live;
    s.dashboard.attachment = Some((ConversationId(1), 1));
    s.apply_event(BackendEvent {
        conversation: ConversationId(1),
        generation: 1,
        op: None,
        kind: EventKind::ConversationLifecycleReady(true),
    });
    let worker = crate::shell::dashboard::test_row(
        "engine-session",
        "/captured/project",
        dashboard::WorkState::Waiting {
            reason: dashboard::WaitReason::CliApproval,
        },
    )
    .worker;
    s.dashboard.rows.push(dashboard::Row {
        worker,
        conversation: Some(ConversationId(1)),
        root: Some(ConversationId(1)),
    });
    let effects = Rc::new(RefCell::new(vec![]));
    let captured = effects.clone();
    let window = cx.add_window(|window, cx| {
        let model = cx.new(|_| {
            let mut m = crate::model::Model::new(s);
            m.set_effect_handler(Box::new(move |e, _| captured.borrow_mut().push(e)));
            m
        });
        Workspace::new(model, Default::default(), window, cx)
    });
    let root = window.root(cx).unwrap();
    let cx = &mut VisualTestContext::from_window(window.into(), cx);
    root.update_in(cx, |w, window, cx| {
        w.dashboard.update(cx, |v, cx| {
            v.open_flyup("engine-session".into(), window, cx)
        })
    });
    cx.run_until_parked();
    effects.borrow_mut().clear();
    let bounds = cx
        .debug_bounds("dashboard-cli-approvals")
        .expect("dashboard review entry");
    cx.simulate_click(bounds.center(), Default::default());
    cx.run_until_parked();
    root.update_in(cx, |w, window, cx| {
        assert_eq!(w.overlay, Overlay::CliApprovals);
        assert_eq!(
            w.cli_approval_panel.target.unwrap().conversation,
            ConversationId(1)
        );
        let req = last_request(&effects, w, cx);
        assert_eq!(req.context, context);
        assert_eq!(req.action, CliApprovalAction::List);
        w.model.update(cx, |m, cx| {
            m.apply_event(
                finish(
                    &req,
                    CliApprovalResult::Listed {
                        run: Some(context.run.clone()),
                        reviews: vec![review.clone()],
                    },
                ),
                cx,
            )
        });
        w.reconcile_cli_approval_text(cx);
        w.approval_action(
            context.clone(),
            CliApprovalAction::Decide {
                id: review.id.clone(),
                decision: D::AllowOnce,
            },
            cx,
        );
        let decision = last_request(&effects, w, cx);
        assert_eq!(decision.context, context);
        assert!(matches!(
            decision.action,
            CliApprovalAction::Decide {
                decision: D::AllowOnce,
                ..
            }
        ));
        w.close_overlay(window, cx);
        assert!(w.state(cx).dashboard.open);
        assert!(w.dashboard.read(cx).thread_dialog_open());
        assert_eq!(w.state(cx).selected, Some(ConversationId(2)));
        assert_eq!(
            w.state(cx).conversations[1].draft.text,
            "other thread draft"
        );
        assert_eq!(
            w.state(cx).dashboard.selected.as_deref(),
            Some("engine-session")
        );
    });
}
