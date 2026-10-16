use super::*;

#[test]
fn lifecycle_ready_follows_all_work_projections_in_the_attachment_snapshot() {
    let fake = Fake::new();
    let mut run = Run::start(fake.host(|_| None));
    run.ready();
    let conversation = run.conversation();
    run.backend.request(BackendRequest::Open {
        conversation,
        generation: 7,
    });
    run.until("complete lifecycle snapshot", |r| {
        r.has(|e| matches!(e, EventKind::ConversationLifecycleReady(true)))
    });
    let ready = run
        .seen
        .iter()
        .position(|e| matches!(e.kind, EventKind::ConversationLifecycleReady(true)))
        .unwrap();
    assert!(
        run.seen[..ready]
            .iter()
            .any(|e| matches!(e.kind, EventKind::ConversationLifecycleReady(false)))
    );
    for projection in [0, 1, 2, 3, 4] {
        assert!(run.seen[..ready].iter().any(|e| matches!(
            (projection, &e.kind),
            (0, EventKind::Opened { .. })
                | (1, EventKind::EngineState { .. })
                | (2, EventKind::UiState { .. })
                | (3, EventKind::GoalSynced { .. })
                | (4, EventKind::JobsSynced { .. })
        )));
    }
    assert_eq!(run.seen[ready].conversation, conversation);
    assert_eq!(run.seen[ready].generation, 7);
    assert_eq!(run.seen[ready].op, None);
    run.backend.shutdown();
}

#[test]
fn unselected_background_work_emits_visibility_only_without_opening_or_running_a_session() {
    let fake = Fake::new();
    let mut run = Run::start(fake.host(|_| None));
    run.ready();
    let conversation = run.conversation();
    fake.publish(Event::Session {
        session: "s1".into(),
        revision: 2,
        change: SessionChange::JobsChanged {
            jobs: JobsView {
                active: 1,
                ..Default::default()
            },
        },
    });
    run.until("unselected work visibility", |r| {
        r.lifecycle
            .iter()
            .any(|e| matches!(e, LifecycleEvent::ConversationAttention(id) if *id == conversation))
    });
    assert!(!fake.calls().iter().any(|call| matches!(
        call,
        Call::OpenSession { .. }
            | Call::Prompt { .. }
            | Call::Enqueue { .. }
            | Call::RemoveSession { .. }
    )));
    assert!(!run.has(|e| matches!(e, EventKind::Opened { .. } | EventKind::EngineState { .. })));
    run.backend.shutdown();
}
