use super::*;
use std::{cell::RefCell, rc::Rc};

#[gpui::test]
fn normal_model_picker_routes_typed_cli_choice_even_if_catalog_changes(cx: &mut TestAppContext) {
    cx.update(crate::text::init);
    let effects = Rc::new(RefCell::new(Vec::new()));
    let captured = effects.clone();
    let mut s = state();
    s.can_create = true;
    s.dispatch(CoreCommand::AddProject("/selected/project".into()));
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
    cx.run_until_parked();
    effects.borrow_mut().clear();
    root.update_in(cx, |w, window, cx| w.begin_conversation(window, cx));
    cx.run_until_parked();
    assert!(effects.borrow().iter().any(|e| matches!(
        e,
        Effect::Backend(BackendRequest::CreateConversation { .. })
    )));
    root.update_in(cx, |w, window, cx| {
        assert_eq!(w.overlay, Overlay::None);
        // A fresh harness for the other configured connection, without completing a fake native creation.
        w.model.update(cx, |m, _| {
            let mut fresh = state();
            fresh.dispatch(CoreCommand::AddProject("/selected/project".into()));
            fresh.models.push(ModelInfo {
                id: "native/gpt".into(),
                name: "GPT".into(),
                note: "native".into(),
            });
            m.state = fresh;
        });
        effects.borrow_mut().clear();
        w.open_overlay(Overlay::Model, window, cx);
        assert_eq!(w.provider_menu.len(), 4);
        w.overlay_sel = w
            .provider_menu
            .iter()
            .position(|m| m.selection == ProviderModel::ClaudeCli(CliModel::Sonnet))
            .unwrap();
        w.model.update(cx, |m, cx| {
            m.state.models.insert(
                0,
                ModelInfo {
                    id: "native/new".into(),
                    name: "New native model".into(),
                    note: "native".into(),
                },
            );
            cx.notify();
        });
        w.confirm_selection(window, cx);
        assert_eq!(w.state(cx).local_cli.draft.model, CliModel::Sonnet);
        assert_eq!(w.overlay, Overlay::Prefs);
        assert_eq!(
            w.settings_section,
            super::super::super::settings::SettingsSection::Connections
        );
        assert_eq!(w.state(cx).local_cli.draft.project, "/selected/project");
        assert!(w.state(cx).local_cli.guided && !w.local_cli_advanced);
        let revision = w.state(cx).local_cli.input_revision();
        w.open_local_cli(false, window, cx);
        assert_eq!(w.overlay, Overlay::LocalCli);
        assert!(!w.state(cx).local_cli.provider_only);
        assert_eq!(
            w.state(cx).local_cli.input_revision(),
            revision,
            "Continue must not restart discovery or replace the selected conversation flow"
        );
        assert!(w.menu_focus.is_focused(window));
    });
    cx.run_until_parked();
    assert_eq!(
        effects
            .borrow()
            .iter()
            .filter(|e| matches!(
                e,
                Effect::Backend(BackendRequest::LocalCli {
                    request: LocalCliRequest::Discover { .. },
                    ..
                })
            ))
            .count(),
        1
    );
    assert!(effects.borrow().iter().all(|e| matches!(
        e,
        Effect::SavePrefs(_)
            | Effect::Backend(BackendRequest::LocalCli {
                request: LocalCliRequest::Discover { .. },
                ..
            })
    )));
}

#[gpui::test]
fn find_again_pointer_click_keeps_both_setup_purposes_open(cx: &mut TestAppContext) {
    cx.update(crate::text::init);
    for provider_only in [false, true] {
        let effects = Rc::new(RefCell::new(Vec::new()));
        let captured = effects.clone();
        let mut s = inspected();
        s.local_cli.guided = true;
        s.local_cli.provider_only = provider_only;
        s.local_cli.stage = LocalCliStage::Failed;
        s.local_cli.error = Some(LocalCliFailure::Rejected(
            "Unsupported CLI version; no provider was substituted.".into(),
        ));
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
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        root.update_in(&mut visual, |w, window, cx| {
            w.open_overlay(Overlay::LocalCli, window, cx)
        });
        visual.run_until_parked();
        root.update(&mut visual, |w, cx| {
            w.cli_dialog_scroll
                .set_offset(gpui::point(px(0.), px(-10000.)));
            cx.notify();
        });
        visual.run_until_parked();
        effects.borrow_mut().clear();
        let bounds = visual.debug_bounds("claude-find-again").unwrap();
        visual.update(|window, _| assert!(bounds.bottom() <= window.viewport_size().height));
        visual.simulate_click(bounds.center(), Default::default());
        visual.run_until_parked();
        root.update(&mut visual, |w, cx| {
            assert_eq!(w.overlay, Overlay::LocalCli);
            assert_eq!(w.state(cx).local_cli.provider_only, provider_only);
            assert!(w.state(cx).local_cli.discovering);
            assert!(w.state(cx).local_cli.ticket.is_none());
        });
        assert_eq!(
            effects
                .borrow()
                .iter()
                .filter(|e| matches!(
                    e,
                    Effect::Backend(BackendRequest::LocalCli {
                        request: LocalCliRequest::Discover { .. },
                        ..
                    })
                ))
                .count(),
            1
        );
        visual.simulate_click(gpui::point(px(2.), px(2.)), Default::default());
        visual.run_until_parked();
        assert_eq!(
            root.read_with(&visual, |w, _| w.overlay),
            Overlay::None,
            "outside click still dismisses"
        );
    }
}

#[gpui::test]
fn stale_folder_picker_cannot_replace_newer_setup_or_restore_trust(cx: &mut TestAppContext) {
    cx.update(crate::text::init);
    let mut s = inspected();
    s.local_cli.guided = true;
    let window = cx.add_window(|window, cx| {
        let model = cx.new(|_| crate::model::Model::new(s));
        model.update(cx, |m, cx| m.explore_workspace(cx));
        Workspace::new(model, Default::default(), window, cx)
    });
    let root = window.root(cx).unwrap();
    let cx = &mut VisualTestContext::from_window(window.into(), cx);
    root.update_in(cx, |w, window, cx| {
        w.open_overlay(Overlay::LocalCli, window, cx);
        w.pick_cli_path(false, window, cx);
        let mut next = w.state(cx).local_cli.draft.clone();
        next.project = "/newer/project".into();
        w.dispatch(CoreCommand::LocalCli(C::Configure(next)), cx);
    });
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|options| {
        assert!(options.directories && !options.files && !options.multiple);
        Some(vec!["/stale/project".into()])
    });
    cx.run_until_parked();
    root.update(cx, |w, cx| {
        assert_eq!(w.state(cx).local_cli.draft.project, "/newer/project");
        assert!(!w.state(cx).local_cli.trust_project && !w.state(cx).local_cli.trust_executable);
    });
}

#[gpui::test]
fn guided_confirmation_is_visible_explicit_and_dispatches_only_one_check(cx: &mut TestAppContext) {
    cx.update(crate::text::init);
    let effects = Rc::new(RefCell::new(Vec::new()));
    let captured = effects.clone();
    let mut s = inspected();
    s.local_cli.guided = true;
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
    root.update_in(cx, |w, window, cx| {
        w.open_overlay(Overlay::LocalCli, window, cx)
    });
    cx.run_until_parked();
    effects.borrow_mut().clear();
    cx.simulate_keystrokes("space enter");
    cx.run_until_parked();
    assert!(
        effects.borrow().is_empty(),
        "neutral focus/render cannot approve"
    );
    let bounds = cx
        .debug_bounds("claude-confirm")
        .expect("confirmation is rendered");
    cx.update(|window, _| {
        assert!(
            bounds.bottom() <= window.viewport_size().height,
            "confirmation must fit the default viewport"
        )
    });
    cx.simulate_click(bounds.center(), Default::default());
    cx.run_until_parked();
    root.update(cx, |w, cx| {
        assert!(w.state(cx).local_cli.trust_executable && w.state(cx).local_cli.trust_project);
        assert_eq!(w.state(cx).local_cli.stage, LocalCliStage::Checking);
        assert_eq!(
            w.overlay,
            Overlay::LocalCli,
            "clicking inside setup must not dismiss it"
        );
    });
    assert_eq!(
        effects
            .borrow()
            .iter()
            .filter(|e| matches!(
                e,
                Effect::Backend(BackendRequest::LocalCli {
                    request: LocalCliRequest::Check { .. },
                    ..
                })
            ))
            .count(),
        1
    );
    assert!(effects.borrow().iter().all(|e| matches!(
        e,
        Effect::SavePrefs(_)
            | Effect::Backend(BackendRequest::LocalCli {
                request: LocalCliRequest::Check { .. },
                ..
            })
    )));
}

#[gpui::test]
fn dashboard_reconnect_captures_other_thread_and_reopens_without_restarting(
    cx: &mut TestAppContext,
) {
    let mut s = AppState::new(
        Bootstrap {
            projects: vec![],
            models: vec![],
            now: 1,
            conversations: vec![
                (ConversationId(1), ProjectId(1), "normal".into(), 1),
                (ConversationId(2), ProjectId(1), "CLI".into(), 1),
            ],
        },
        Prefs::default(),
    );
    s.mode = Mode::Real;
    s.selected = Some(ConversationId(1));
    s.dashboard.open = true;
    s.dashboard.freshness = dashboard::Freshness::Live;
    s.conversations[0].draft.text = "normal draft".into();
    s.dashboard.rows.push(dashboard::Row {
        worker: crate::shell::dashboard::test_row(
            "cli-session",
            "/project",
            dashboard::WorkState::Blocked {
                reason: "Reconnect required".into(),
            },
        )
        .worker,
        conversation: Some(ConversationId(2)),
        root: Some(ConversationId(2)),
    });
    s.dashboard.rows[0].worker.execution_backend = dashboard::ExecutionBackend::LocalClaudeCli;
    let effects = Rc::new(RefCell::new(vec![]));
    let captured = effects.clone();
    let window = cx.add_window(|window, cx| {
        let model = cx.new(|_| {
            let mut m = crate::model::Model::new(s);
            m.set_effect_handler(Box::new(move |e, _| captured.borrow_mut().push(e)));
            m
        });
        model.update(cx, |m, cx| {
            m.set_local_cli_catalog(
                true,
                [(ConversationId(2), EngineSessionId("cli-session".into()))],
                cx,
            )
        });
        Workspace::new(model, Default::default(), window, cx)
    });
    let root = window.root(cx).unwrap();
    let cx = &mut VisualTestContext::from_window(window.into(), cx);
    root.update_in(cx, |w, window, cx| {
        w.dashboard
            .update(cx, |v, cx| v.open_flyup("cli-session".into(), window, cx))
    });
    cx.run_until_parked();
    effects.borrow_mut().clear();
    let bounds = cx
        .debug_bounds("dashboard-cli-reconnect")
        .expect("reconnect from thread dialog");
    cx.simulate_click(bounds.center(), Default::default());
    cx.run_until_parked();
    root.update(cx, |w, cx| {
        assert_eq!(w.overlay, Overlay::None);
        assert!(w.dashboard_cli_pending.is_some());
        let generation = w
            .state(cx)
            .conversation(ConversationId(2))
            .unwrap()
            .generation;
        w.model.update(cx, |m, cx| {
            m.state.conversations[1].opened = true;
            m.apply_event(
                BackendEvent {
                    conversation: ConversationId(2),
                    generation,
                    op: None,
                    kind: EventKind::ExecutionContext {
                        profile: ExecutionProfile::LocalCliCoding,
                        execution_blocked: Some("Reconnect required".into()),
                    },
                },
                cx,
            );
            m.apply_event(
                BackendEvent {
                    conversation: ConversationId(2),
                    generation,
                    op: None,
                    kind: EventKind::ConversationLifecycleReady(true),
                },
                cx,
            );
        });
    });
    cx.run_until_parked();
    root.update_in(cx, |w, window, cx| {
        assert_eq!(w.overlay, Overlay::LocalCli);
        assert!(matches!(&w.state(cx).local_cli.target, Some(LocalCliTarget::Existing { conversation: ConversationId(2), session, .. }) if session.0 == "cli-session"));
        let revision = w.state(cx).local_cli.input_revision();
        assert!(!w.state(cx).local_cli_availability().session_status);
        w.close_overlay(window, cx);
        effects.borrow_mut().clear();
        w.open_local_cli_for(ConversationId(2), window, cx);
        assert_eq!(w.state(cx).local_cli.input_revision(), revision);
        assert!(effects.borrow().is_empty(), "reopening is not a new request");
        assert_eq!(w.state(cx).selected, Some(ConversationId(1)));
        assert_eq!(w.state(cx).conversations[0].draft.text, "normal draft");
        assert!(w.state(cx).dashboard.open);
        w.close_overlay(window, cx);
        assert!(w.dashboard.read(cx).thread_dialog_open());
    });
}

#[gpui::test]
fn dashboard_new_picker_captures_project_and_model_without_leaving_graph(cx: &mut TestAppContext) {
    cx.update(crate::text::init);
    let effects = Rc::new(RefCell::new(Vec::new()));
    let captured = effects.clone();
    let mut s = state();
    s.projects = vec![
        Project {
            id: ProjectId(1),
            name: "First".into(),
            path: "/first".into(),
        },
        Project {
            id: ProjectId(2),
            name: "Second".into(),
            path: "/second".into(),
        },
    ];
    s.dashboard.open = true;
    let window = cx.open_window(gpui::size(px(1600.), px(1000.)), |window, cx| {
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
    cx.run_until_parked();
    let bounds = cx.debug_bounds("graph-new-thread").unwrap();
    cx.simulate_click(bounds.center(), Default::default());
    cx.run_until_parked();
    root.update(cx, |w, cx| {
        assert_eq!(w.overlay, Overlay::DashboardNew);
        assert!(w.state(cx).dashboard.open);
    });
    assert!(
        !effects
            .borrow()
            .iter()
            .any(|e| matches!(e, Effect::Backend(BackendRequest::LocalCli { .. })))
    );
    let bounds = cx.debug_bounds("new-cli-project-2").unwrap();
    cx.simulate_click(bounds.center(), Default::default());
    cx.run_until_parked();
    let bounds = cx.debug_bounds("new-cli-opus").unwrap();
    cx.simulate_click(bounds.center(), Default::default());
    cx.run_until_parked();
    let bounds = cx.debug_bounds("new-thread-create").unwrap();
    cx.simulate_click(bounds.center(), Default::default());
    cx.run_until_parked();
    root.update_in(cx, |w, window, cx| {
        assert_eq!(w.overlay, Overlay::LocalCli);
        assert!(w.state(cx).dashboard.open);
        assert_eq!(w.state(cx).local_cli.draft.project, "/second");
        assert_eq!(w.state(cx).local_cli.draft.model, CliModel::Opus);
        let generation = w.state(cx).local_cli.input_revision();
        w.close_overlay(window, cx);
        w.open_dashboard_new(window, cx);
        assert_eq!(w.overlay, Overlay::LocalCli);
        assert_eq!(w.state(cx).local_cli.input_revision(), generation);
    });
    assert_eq!(
        effects
            .borrow()
            .iter()
            .filter(|e| matches!(
                e,
                Effect::Backend(BackendRequest::LocalCli {
                    request: LocalCliRequest::Discover { .. },
                    ..
                })
            ))
            .count(),
        1
    );
    root.update(cx, |w, cx| {
        w.model.update(cx, |m, cx| {
            m.state.local_cli.stage = LocalCliStage::Created;
            m.state.local_cli.created = Some(LocalCliCreated {
                identity: LocalCliCreationId {
                    session: EngineSessionId("new-cli".into()),
                    setup: SetupId("setup".into()),
                },
                conversation: ConversationId(3),
            });
            m.state
                .dashboard
                .rows
                .push(crate::shell::dashboard::test_row(
                    "new-cli",
                    "/second",
                    dashboard::WorkState::Ready,
                ));
            cx.notify();
        });
    });
    cx.run_until_parked();
    root.update(cx, |w, cx| {
        assert_eq!(w.overlay, Overlay::None);
        assert!(w.state(cx).dashboard.open);
        assert_eq!(w.state(cx).dashboard.selected.as_deref(), Some("new-cli"));
        assert!(w.dashboard.read(cx).thread_dialog_open());
    });
}
