use super::*;

#[gpui::test]
fn close_and_closed_reopen_controls_keep_the_draft_and_do_not_hide_busy_work(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(crate::text::init);
    let mut state = AppState::new(
        Bootstrap {
            projects: vec![Project {
                id: ProjectId(1),
                name: "Project".into(),
                path: "/project".into(),
            }],
            conversations: vec![(
                ConversationId(1),
                ProjectId(1),
                "Keep this conversation".into(),
                1,
            )],
            models: vec![],
            now: 1,
        },
        Prefs::default(),
    );
    state.selected = Some(ConversationId(1));
    let c = &mut state.conversations[0];
    c.opened = true;
    c.run = RunState::Running { op: OperationId(1) };
    c.draft.text = "keep my unsent draft".into();
    c.draft.save = SaveState::Saved;
    let window = cx.open_window(gpui::size(px(1280.0), px(900.0)), |window, cx| {
        let model = cx.new(|_| crate::model::Model::new(state));
        model.update(cx, |m, cx| m.explore_workspace(cx));
        Workspace::new(model, Default::default(), window, cx)
    });
    let root = window.root(cx).unwrap();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let close = visual.debug_bounds("close-conversation").unwrap();
    assert_eq!(
        close.size.width, close.size.height,
        "Close is an icon-only square control"
    );
    visual.simulate_click(close.center(), Default::default());
    visual.run_until_parked();
    root.update(&mut visual, |w, cx| {
        assert!(!w.state(cx).conversation_is_closed(ConversationId(1)));
        w.model.update(cx, |m, cx| {
            m.state.conversations[0].run = RunState::Idle;
            cx.notify();
        });
    });
    visual.run_until_parked();
    let close = visual.debug_bounds("close-conversation").unwrap();
    visual.simulate_click(close.center(), Default::default());
    visual.run_until_parked();
    root.update(&mut visual, |w, cx| {
        assert_eq!(w.state(cx).selected, None);
        assert!(w.state(cx).conversation_is_closed(ConversationId(1)));
        assert_eq!(
            w.state(cx).conversations[0].draft.text,
            "keep my unsent draft"
        );
    });
    assert!(visual.debug_bounds("close-conversation").is_none());
    let closed = visual.debug_bounds("toggle-closed-conversations").unwrap();
    visual.simulate_click(closed.center(), Default::default());
    visual.run_until_parked();
    let reopen = visual.debug_bounds("reopen-conversation-1").unwrap();
    visual.simulate_click(reopen.center(), Default::default());
    visual.run_until_parked();
    root.update(&mut visual, |w, cx| {
        assert_eq!(w.state(cx).selected, Some(ConversationId(1)));
        assert!(!w.state(cx).conversation_is_closed(ConversationId(1)));
        assert_eq!(
            w.state(cx).current().unwrap().draft.text,
            "keep my unsent draft"
        );
        assert_eq!(w.composer.read(cx).text(), "keep my unsent draft");
    });
    assert!(visual.debug_bounds("close-conversation").is_some());
}
