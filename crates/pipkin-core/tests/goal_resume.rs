//! Lost goal receipts must offer observation, not a dead Resume button or mutation replay.
use pipkin_core::*;
mod support;

fn state() -> AppState {
    let mut s = AppState::new(
        Bootstrap {
            projects: vec![Project {
                id: ProjectId(1),
                name: "one".into(),
                path: "/one".into(),
            }],
            conversations: vec![(ConversationId(1), ProjectId(1), "one".into(), 0)],
            models: vec![],
            now: 0,
        },
        Prefs::default(),
    );
    s.mode = Mode::Real;
    s.set_connection(Connection::Ready);
    s.dispatch(Command::SelectConversation(ConversationId(1)));
    support::native_context(&mut s);
    s.conversations[0].opened = true;
    apply(&mut s, EventKind::ConversationLifecycleReady(true));
    apply(&mut s, EventKind::GoalSynced { goal: None });
    s
}
fn apply(s: &mut AppState, kind: EventKind) {
    let c = s.current().unwrap();
    let e = BackendEvent {
        conversation: c.id,
        generation: c.generation,
        op: None,
        kind,
    };
    s.apply_event(e);
}
fn report(s: &mut AppState, state: GoalState) {
    apply(
        s,
        EventKind::GoalSynced {
            goal: Some(Goal {
                text: "work".into(),
                paused: (state == GoalState::Paused).then(|| "no status".into()),
                turns: 10,
                native: true,
                id: Some(6),
                state: Some(state),
                criteria_met: 0,
                criteria_total: 0,
                usage_tokens: 0,
                usage_cost_usd: 0.0,
                max_turns: None,
                max_tokens: None,
                max_cost_usd: None,
                audit_required: false,
                audit_verdict: None,
            }),
        },
    );
}
fn pending(s: &mut AppState, start: bool) -> RequestId {
    report(s, GoalState::Paused);
    let out = s.dispatch(if start {
        Command::SetGoal("work".into())
    } else {
        Command::ResumeGoal
    });
    out.effects
        .into_iter()
        .find_map(|e| match e {
            Effect::Backend(BackendRequest::SetGoal { request, .. })
            | Effect::Backend(BackendRequest::GoalControl {
                request: Some(request),
                ..
            }) => Some(request),
            _ => None,
        })
        .expect("start/resume intent")
}
fn refresh(s: &mut AppState) -> u64 {
    let out = s.dispatch(Command::ResumeGoal);
    assert!(
        out.effects.iter().all(|e| matches!(
            e,
            Effect::SavePrefs(_) | Effect::Backend(BackendRequest::Open { .. })
        )),
        "{:?}",
        out.effects
    );
    let generation = out
        .effects
        .iter()
        .find_map(|e| match e {
            Effect::Backend(BackendRequest::Open {
                conversation: ConversationId(1),
                generation,
            }) => Some(*generation),
            _ => None,
        })
        .expect(
            "an unconfirmed goal requests an observation instead of silently dropping the click",
        );
    assert!(
        s.notice
            .as_deref()
            .unwrap()
            .contains("no goal command was resent")
    );
    generation
}
#[test]
fn unconfirmed_start_or_resume_refreshes_then_requires_a_new_explicit_intent() {
    for start in [true, false] {
        let mut s = state();
        let request = pending(&mut s, start);
        s.dispatch(Command::EditDraft("keep this draft".into()));
        let old = s.current().unwrap().generation;
        apply(
            &mut s,
            EventKind::GoalRefused {
                reason: "reply lost".into(),
            },
        );
        assert!(s.current().unwrap().goal_start_unconfirmed());
        let fresh = refresh(&mut s);
        assert!(fresh > old);
        assert!(s.close_conversation_reason(ConversationId(1)).is_some());
        // A late receipt cannot resolve the read's new attachment.
        s.apply_event(BackendEvent {
            conversation: ConversationId(1),
            generation: old,
            op: None,
            kind: EventKind::GoalStartConfirmed {
                request: request.clone(),
            },
        });
        assert!(s.current().unwrap().goal_start_unconfirmed());
        report(&mut s, GoalState::Paused);
        assert!(!s.current().unwrap().goal_start_unconfirmed());
        assert!(
            s.dispatch(Command::ResumeGoal).effects.is_empty(),
            "wait for the complete lifecycle snapshot"
        );
        apply(&mut s, EventKind::ConversationLifecycleReady(true));
        assert_eq!(s.current().unwrap().draft.text, "keep this draft");
        let out = s.dispatch(Command::ResumeGoal);
        assert!(
            matches!(out.effects.as_slice(),[Effect::Backend(BackendRequest::GoalControl {request:Some(new),action:GoalControlAction::Resume,..})] if new!=&request)
        );
    }
}
#[test]
fn failed_goal_status_read_can_be_retried_without_replaying_the_mutation() {
    let mut s = state();
    pending(&mut s, false);
    let first = refresh(&mut s);
    apply(
        &mut s,
        EventKind::OpenFailed {
            message: "offline".into(),
        },
    );
    assert!(s.current().unwrap().goal_start_unconfirmed());
    assert!(
        s.notice
            .as_deref()
            .unwrap()
            .contains("Could not check goal status: offline")
    );
    let retry = refresh(&mut s);
    assert!(retry > first);
    assert!(
        refresh(&mut s) > retry,
        "repeated clicks only refresh observations"
    );
}
#[test]
fn refreshed_active_goal_is_not_resumed_again_and_old_snapshots_cannot_clear_the_fence() {
    let mut s = state();
    pending(&mut s, true);
    let old = s.current().unwrap().generation;
    let current = refresh(&mut s);
    for (conversation, generation) in [(ConversationId(1), old), (ConversationId(2), current)] {
        s.apply_event(BackendEvent {
            conversation,
            generation,
            op: None,
            kind: EventKind::GoalSynced { goal: None },
        });
        assert!(s.current().unwrap().goal_start_unconfirmed());
    }
    report(&mut s, GoalState::Active);
    apply(&mut s, EventKind::ConversationLifecycleReady(true));
    assert!(!s.current().unwrap().goal_start_unconfirmed());
    assert!(s.dispatch(Command::ResumeGoal).effects.is_empty());
    assert_eq!(
        s.current().unwrap().goal.as_ref().unwrap().state,
        Some(GoalState::Active)
    );
}
