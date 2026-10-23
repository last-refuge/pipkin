use super::{D, DashboardView, graph_layout};
use crate::model::Model;
use crate::theme::ActiveTheme;
use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext, px, size};
use pipkin_core::{dashboard as d, *};
use std::{cell::RefCell, rc::Rc};

type Captured = Rc<RefCell<Vec<Effect>>>;
fn fixture(
    cx: &mut TestAppContext,
    compact: bool,
    filter: d::ActivityFilter,
) -> (Entity<DashboardView>, VisualTestContext, Captured) {
    cx.update(|cx| {
        crate::text::init(cx);
        cx.set_global(crate::theme::Theme::new(
            Theme::Dark,
            TextSize::Normal,
            true,
        ));
    });
    let effects: Captured = Default::default();
    let captured = effects.clone();
    let window = cx.open_window(
        size(px(if compact { 900. } else { 1600. }), px(1000.)),
        |w, cx| {
            let mut state = AppState::new(
                Bootstrap {
                    projects: vec![],
                    models: vec![],
                    conversations: vec![],
                    now: 0,
                },
                Prefs::default(),
            );
            state.dashboard.open = true;
            state.dashboard.available = true;
            state.dashboard.freshness = d::Freshness::Live;
            state.dashboard.filter = filter;
            state.dashboard.rows = vec![
                graph_layout::tests::row(
                    "one",
                    "/one",
                    d::WorkState::Waiting {
                        reason: d::WaitReason::Question,
                    },
                ),
                graph_layout::tests::row(
                    "two",
                    "/two",
                    d::WorkState::Waiting {
                        reason: d::WaitReason::Question,
                    },
                ),
                graph_layout::tests::row("ready", "/two", d::WorkState::Ready),
            ];
            let model = cx.new(|_| Model::new(state));
            model.update(cx, |m, _| {
                m.set_effect_handler(Box::new(move |e, _| captured.borrow_mut().push(e)))
            });
            DashboardView::new(model, w, cx)
        },
    );
    let root = window.root(cx).unwrap();
    let visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    (root, visual, effects)
}
fn click(cx: &mut VisualTestContext, id: &'static str) {
    let bounds = cx
        .debug_bounds(id)
        .unwrap_or_else(|| panic!("missing control {id}"));
    cx.simulate_click(bounds.center(), Default::default());
    cx.run_until_parked();
}

#[gpui::test]
fn graph_filters_restore_clear_and_query_the_backend(cx: &mut TestAppContext) {
    let (root, mut cx, effects) = fixture(
        cx,
        false,
        d::ActivityFilter {
            text: Some("saved query".into()),
            cwd: Some("/one".into()),
            ..Default::default()
        },
    );
    root.update(&mut cx, |v, cx| {
        assert_eq!(v.search.read(cx).text(), "saved query")
    });
    click(&mut cx, "graph-clear-filters");
    root.update(&mut cx, |v, cx| {
        assert!(v.model.read(cx).state.dashboard.filter.cwd.is_none());
        assert_eq!(v.search.read(cx).text(), "");
    });
    click(&mut cx, "graph-running");
    assert!(effects.borrow().iter().any(|e| matches!(
        e,
        Effect::Backend(BackendRequest::Dashboard(d::Request::Watch {
            group: Some(d::ActivityGroup::Running),
            ..
        }))
    )));
    click(&mut cx, "refresh-workers");
    click(&mut cx, "list-view");
    root.update(&mut cx, |v, _| assert!(v.graph.list));
    click(&mut cx, "graph-view");
    root.update(&mut cx, |v, _| assert!(!v.graph.list));
    assert!(!effects.borrow().iter().any(|e| matches!(
        e,
        Effect::Backend(BackendRequest::Submit { .. } | BackendRequest::UiRespond { .. })
    )));
}

#[gpui::test]
fn compact_attention_opens_the_new_target_and_empty_queue_does_nothing(cx: &mut TestAppContext) {
    let (root, mut cx, _) = fixture(cx, true, Default::default());
    root.update(&mut cx, |v, cx| v.send(D::Select("one".into()), cx));
    cx.run_until_parked();
    click(&mut cx, "narrow-attention");
    root.update(&mut cx, |v, cx| {
        assert_eq!(
            v.model.read(cx).state.dashboard.selected.as_deref(),
            Some("two")
        );
        assert!(v.modal_open);
    });
    click(&mut cx, "close-thread-sheet");
    root.update(&mut cx, |v, cx| {
        v.model.update(cx, |m, cx| {
            for row in &mut m.state.dashboard.rows {
                row.worker.state = d::WorkState::Ready;
            }
            cx.notify();
        })
    });
    cx.run_until_parked();
    click(&mut cx, "narrow-attention");
    root.update(&mut cx, |v, _| assert!(!v.modal_open));
}

#[gpui::test]
fn map_controls_fit_expand_and_refresh_dialog_history(cx: &mut TestAppContext) {
    let (root, mut cx, effects) = fixture(cx, false, Default::default());
    click(&mut cx, "zoom-in");
    root.update(&mut cx, |v, _| assert!(v.graph.zoom.is_some()));
    root.update(&mut cx, |v, cx| {
        v.graph.zoom = Some(0.2);
        cx.notify();
    });
    cx.run_until_parked();
    click(&mut cx, "zoom-out");
    root.update(&mut cx, |v, _| {
        assert!(
            v.graph.zoom.unwrap() < 0.2,
            "zoom out must not enlarge a densely fitted map"
        )
    });
    click(&mut cx, "rail-map");
    root.update(&mut cx, |v, _| {
        assert!(v.graph.zoom.is_none());
        assert_eq!(v.graph.pan, (0., 0.));
    });
    click(&mut cx, "collapse-projects");
    root.update(&mut cx, |v, _| {
        assert!(
            v.graph
                .projects
                .iter()
                .all(|p| v.graph.collapsed.contains(&p.path))
        );
        v.graph.collapsed.insert("/previous-filter".into());
    });
    click(&mut cx, "collapse-projects");
    root.update(&mut cx, |v, _| assert!(v.graph.collapsed.is_empty()));
    root.update_in(&mut cx, |v, w, cx| v.open_flyup("ready".into(), w, cx));
    cx.run_until_parked();
    effects.borrow_mut().clear();
    click(&mut cx, "dialog-latest-history");
    assert!(effects.borrow().iter().any(|e| matches!(e, Effect::Backend(BackendRequest::Dashboard(d::Request::Detail { session, before: None, .. })) if session == "ready")));
}

#[gpui::test]
fn send_control_targets_the_selected_thread_without_changing_the_main_conversation(
    cx: &mut TestAppContext,
) {
    let (root, mut cx, effects) = fixture(cx, false, Default::default());
    root.update(&mut cx, |v, cx| {
        v.model.update(cx, |m, cx| {
            let target = ConversationId(7);
            let normal = ConversationId(8);
            m.state.conversations = vec![
                ConversationState::new(target, ProjectId(1), "Target".into(), 0),
                ConversationState::new(normal, ProjectId(1), "Normal".into(), 0),
            ];
            m.state.selected = Some(normal);
            m.state.conversations[1].draft.text = "keep my draft".into();
            let row = &mut m.state.dashboard.rows[2];
            row.conversation = Some(target);
            row.root = Some(target);
            cx.notify();
        })
    });
    cx.run_until_parked();
    root.update_in(&mut cx, |v, w, cx| v.open_flyup("ready".into(), w, cx));
    cx.run_until_parked();
    cx.simulate_input("message for target");
    cx.run_until_parked();
    effects.borrow_mut().clear();
    click(&mut cx, "send-worker");
    assert!(effects.borrow().iter().any(|e| matches!(
        e,
        Effect::Backend(BackendRequest::Dashboard(d::Request::Attach {
            conversation: ConversationId(7),
            ..
        }))
    )));
    root.update(&mut cx, |v, cx| {
        let state = &v.model.read(cx).state;
        assert_eq!(state.selected, Some(ConversationId(8)));
        assert_eq!(
            state.conversation(ConversationId(8)).unwrap().draft.text,
            "keep my draft"
        );
        assert_eq!(
            state.conversation(ConversationId(7)).unwrap().draft.text,
            "message for target"
        );
    });
}

#[gpui::test]
fn graph_recency_control_cycles_saved_policy_without_execution(cx: &mut TestAppContext) {
    let (root, mut cx, effects) = fixture(cx, false, Default::default());
    for days in [60, 90, 30] {
        click(&mut cx, "graph-recency");
        root.update(&mut cx, |v, cx| {
            assert_eq!(v.model.read(cx).state.dashboard.recency.days(), days)
        });
        assert!(effects.borrow().iter().any(|e| matches!(e,
            Effect::Backend(BackendRequest::Dashboard(d::Request::Watch { filter, .. })) if filter.inactive_days == Some(days))));
    }
    assert!(!effects.borrow().iter().any(|e| matches!(
        e,
        Effect::Backend(BackendRequest::Submit { .. } | BackendRequest::CreateConversation { .. })
    )));
}

#[gpui::test]
fn unread_completion_survives_opening_and_explicit_review_clears_the_graph(
    cx: &mut TestAppContext,
) {
    let (root, mut cx, effects) = fixture(cx, false, Default::default());
    root.update(&mut cx, |v, cx| {
        v.model.update(cx, |m, cx| {
            let row = m
                .state
                .dashboard
                .rows
                .iter_mut()
                .find(|r| r.worker.session == "ready")
                .unwrap();
            row.worker.run = Some(d::RunBrief {
                id: 1,
                phase: None,
                outcome: Some(d::RunOutcome::Completed),
                started_at_ms: 0,
                ended_at_ms: Some(1),
            });
            cx.notify();
        });
    });
    cx.run_until_parked();
    root.update_in(&mut cx, |v, w, cx| {
        assert!(v.graph.attention.contains(&"ready".into()));
        assert_eq!(
            v.graph
                .projects
                .iter()
                .find(|p| p.path == "/two")
                .unwrap()
                .attention,
            2
        );
        v.open_flyup("ready".into(), w, cx);
    });
    cx.run_until_parked();
    root.update(&mut cx, |v, cx| {
        let dash = &v.model.read(cx).state.dashboard;
        assert!(dash.completion_unread(dash.selected_row().unwrap()));
    });
    click(&mut cx, "dialog-mark-reviewed");
    root.update(&mut cx, |v, cx| {
        let dash = &v.model.read(cx).state.dashboard;
        assert!(!dash.completion_unread(dash.selected_row().unwrap()));
        assert!(!v.graph.attention.contains(&"ready".into()));
        assert!(v.modal_open, "review keeps the graph and dialog in place");
        assert_eq!(
            super::presentation::row_status(dash.selected_row().unwrap(), dash, &cx.theme().colors)
                .0,
            "Awaiting next steps"
        );
    });
    assert!(
        effects
            .borrow()
            .iter()
            .any(|e| matches!(e, Effect::SavePrefs(_)))
    );
    assert!(!effects.borrow().iter().any(|e| matches!(
        e,
        Effect::Backend(BackendRequest::Submit { .. } | BackendRequest::UiRespond { .. })
    )));
}

#[gpui::test]
fn manual_hiding_controls_restore_projects_and_threads_in_graph_and_list(cx: &mut TestAppContext) {
    let (root, mut cx, effects) = fixture(cx, false, Default::default());
    click(&mut cx, "hide-project:/one");
    root.update(&mut cx, |v, cx| {
        assert!(
            v.model
                .read(cx)
                .state
                .dashboard
                .hidden_projects
                .contains("/one")
        );
        assert!(!v.graph.attention.contains(&"one".into()));
    });
    root.update_in(&mut cx, |v, w, cx| v.open_flyup("two".into(), w, cx));
    cx.run_until_parked();
    click(&mut cx, "dialog-hide-thread");
    root.update(&mut cx, |v, cx| {
        assert!(!v.modal_open);
        assert!(
            v.model
                .read(cx)
                .state
                .dashboard
                .hidden_threads
                .contains_key("two")
        );
    });
    click(&mut cx, "hide-project:/two");
    root.update(&mut cx, |v, cx| {
        assert!(v.model.read(cx).state.dashboard.rows.is_empty())
    });
    click(&mut cx, "manage-hidden");
    root.update(&mut cx, |v, _| {
        assert!(v.hidden_open, "manager did not open")
    });
    assert!(
        cx.debug_bounds("close-hidden").is_some(),
        "manager controls not painted"
    );
    click(&mut cx, "show-project:/one");
    root.update(&mut cx, |v, cx| {
        let d = &v.model.read(cx).state.dashboard;
        assert!(!d.hidden_projects.contains("/one"));
        assert!(d.hidden_projects.contains("/two"));
        assert!(d.hidden_threads.contains_key("two"));
    });
    click(&mut cx, "show-thread:two");
    click(&mut cx, "show-all-hidden");
    root.update(&mut cx, |v, cx| {
        assert!(v.model.read(cx).state.dashboard.hidden_projects.is_empty())
    });
    click(&mut cx, "close-hidden");
    click(&mut cx, "list-view");
    click(&mut cx, "manage-hidden");
    root.update(&mut cx, |v, cx| {
        assert!(v.hidden_open);
        assert!(v.model.read(cx).state.dashboard.hidden_threads.is_empty());
    });
    cx.simulate_keystrokes("tab shift-tab escape");
    cx.run_until_parked();
    root.update(&mut cx, |v, _| assert!(!v.hidden_open));
    assert!(!effects.borrow().iter().any(|e| matches!(
        e,
        Effect::Backend(BackendRequest::Submit { .. } | BackendRequest::UiRespond { .. })
    )));
    assert!(effects.borrow().iter().any(|e| matches!(e, Effect::Backend(BackendRequest::Dashboard(d::Request::Watch { filter, .. })) if filter.hidden_sessions.contains("two"))));
}
