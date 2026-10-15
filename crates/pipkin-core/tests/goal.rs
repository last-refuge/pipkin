use pipkin_core::*;
mod support;

fn state() -> AppState {
    let mut s = AppState::new(
        Bootstrap {
            projects: vec![Project {
                id: ProjectId(1),
                name: "demo".into(),
                path: "/demo".into(),
            }],
            models: vec![],
            conversations: vec![(ConversationId(1), ProjectId(1), "one".into(), 0)],
            now: 0,
        },
        Prefs::default(),
    );
    support::native_context(&mut s);
    s
}

fn settle(s: &mut AppState, out: Outcome) -> Outcome {
    let intents: Vec<_> = out
        .effects
        .iter()
        .filter_map(|effect| {
            if let Effect::JournalIntent {
                conversation,
                request,
                ..
            } = effect
            {
                Some((*conversation, request.clone()))
            } else {
                None
            }
        })
        .collect();
    let mut all = out;
    for (conversation, request) in intents {
        all.merge(s.intent_persisted(conversation, &request, Ok(())));
    }
    all
}

fn event(s: &AppState, kind: EventKind) -> BackendEvent {
    let c = s.current().unwrap();
    BackendEvent {
        conversation: c.id,
        generation: c.generation,
        op: c.run.op(),
        kind,
    }
}

#[test]
fn goal_continues_only_with_explicit_status_and_stops_when_met() {
    let mut s = state();
    let out = s.dispatch(Command::SetGoal("count to two".into()));
    let first = settle(&mut s, out);
    assert!(first.effects.iter().any(|e| matches!(e, Effect::Backend(BackendRequest::Submit { text, .. }) if text.contains("count to two"))));
    assert_eq!(s.current().unwrap().goal.as_ref().unwrap().turns, 1);
    s.apply_event(event(&s, EventKind::Accepted));
    s.apply_event(event(
        &s,
        EventKind::Token("One.\n[PIPKIN_GOAL_CONTINUE]".into()),
    ));
    let completed = event(&s, EventKind::Completed);
    let out = s.apply_event(completed);
    let next = settle(&mut s, out);
    assert!(
        next.effects
            .iter()
            .any(|e| matches!(e, Effect::Backend(BackendRequest::Submit { .. })))
    );
    s.apply_event(event(&s, EventKind::Accepted));
    s.apply_event(event(
        &s,
        EventKind::Token("Two.\n[PIPKIN_GOAL_MET]".into()),
    ));
    let done = s.apply_event(event(&s, EventKind::Completed));
    assert!(
        !done
            .effects
            .iter()
            .any(|e| matches!(e, Effect::JournalIntent { .. }))
    );
    assert_eq!(
        s.current()
            .unwrap()
            .goal
            .as_ref()
            .unwrap()
            .paused
            .as_deref(),
        Some("Goal met.")
    );
}

#[test]
fn clearing_while_goal_intent_is_uncommitted_never_sends_it() {
    let mut s = state();
    let out = s.dispatch(Command::SetGoal("pending".into()));
    let request = out
        .effects
        .iter()
        .find_map(|effect| match effect {
            Effect::JournalIntent { request, .. } => Some(request.clone()),
            _ => None,
        })
        .unwrap();
    s.dispatch(Command::ClearGoal);
    let acknowledged = s.intent_persisted(ConversationId(1), &request, Ok(()));
    assert!(acknowledged.effects.iter().any(|effect| matches!(
        effect,
        Effect::JournalState {
            state: JournalState::Cancelled,
            ..
        }
    )));
    assert!(
        !acknowledged
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::Backend(BackendRequest::Submit { .. })))
    );
}

#[test]
fn restored_goal_is_visible_but_never_replays_without_an_explicit_update() {
    let mut s = state();
    s.restore_goal(ConversationId(1), "ship v1".into(), None);
    let opened = event(
        &s,
        EventKind::Opened {
            items: vec![],
            has_older: false,
            changes: vec![],
        },
    );
    let out = s.apply_event(opened);
    assert!(
        !out.effects
            .iter()
            .any(|effect| matches!(effect, Effect::JournalIntent { .. }))
    );
    assert!(s.current().unwrap().goal.as_ref().unwrap().paused.is_some());
    let out = s.dispatch(Command::SetGoal("ship v1".into()));
    assert!(
        out.effects
            .iter()
            .any(|effect| matches!(effect, Effect::JournalIntent { .. }))
    );
}

#[test]
fn replacement_goal_waits_for_old_stop_and_is_not_paused_by_its_confirmation() {
    let mut s = state();
    let old = s.dispatch(Command::SetGoal("commit and push".into()));
    settle(&mut s, old);
    s.apply_event(event(&s, EventKind::Accepted));
    s.dispatch(Command::Cancel);
    assert_eq!(
        s.current()
            .unwrap()
            .goal
            .as_ref()
            .unwrap()
            .paused
            .as_deref(),
        Some("Stop requested.")
    );
    let stop = event(&s, EventKind::Cancelled);
    let replacement = s.dispatch(Command::SetGoal("count to 100".into()));
    assert!(
        !replacement
            .effects
            .iter()
            .any(|e| matches!(e, Effect::JournalIntent { .. }))
    );
    let confirmed = s.apply_event(stop);
    let next = settle(&mut s, confirmed);
    let texts: Vec<_> = next
        .effects
        .iter()
        .filter_map(|e| match e {
            Effect::Backend(BackendRequest::Submit { text, .. }) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(texts.len(), 1);
    assert!(texts[0].contains("count to 100"));
    assert!(!texts[0].contains("commit and push"));
    assert!(s.current().unwrap().goal.as_ref().unwrap().paused.is_none());
    assert_eq!(s.current().unwrap().goal.as_ref().unwrap().turns, 1);
}

#[test]
fn stop_pauses_the_current_goal_even_when_completion_races_confirmation() {
    let mut s = state();
    let old = s.dispatch(Command::SetGoal("commit and push".into()));
    settle(&mut s, old);
    s.apply_event(event(&s, EventKind::Accepted));
    s.dispatch(Command::Cancel);
    s.apply_event(event(
        &s,
        EventKind::Token("continue\n[PIPKIN_GOAL_CONTINUE]".into()),
    ));
    let finished = s.apply_event(event(&s, EventKind::Completed));
    assert!(
        !finished
            .effects
            .iter()
            .any(|e| matches!(e, Effect::JournalIntent { .. }))
    );
    assert!(s.current().unwrap().goal.as_ref().unwrap().paused.is_some());
}

#[test]
fn missing_marker_pauses_and_clear_does_not_submit_again() {
    let mut s = state();
    let out = s.dispatch(Command::SetGoal("test".into()));
    settle(&mut s, out);
    s.apply_event(event(&s, EventKind::Accepted));
    s.apply_event(event(&s, EventKind::Token("ambiguous".into())));
    let completed = s.apply_event(event(&s, EventKind::Completed));
    assert!(
        !completed
            .effects
            .iter()
            .any(|e| matches!(e, Effect::JournalIntent { .. }))
    );
    assert!(s.current().unwrap().goal.as_ref().unwrap().paused.is_some());
    s.dispatch(Command::ClearGoal);
    assert!(s.current().unwrap().goal.is_none());
}

// ------------------------------------------------------------------ engine-owned goals

fn opened(s: &mut AppState) {
    let ev = event(
        s,
        EventKind::Opened {
            items: vec![],
            has_older: false,
            changes: vec![],
        },
    );
    s.apply_event(ev);
}

fn unscoped(s: &AppState, kind: EventKind) -> BackendEvent {
    BackendEvent {
        op: None,
        ..event(s, kind)
    }
}

fn native_goal(text: &str, state: GoalState, reason: Option<&str>) -> Goal {
    Goal {
        text: text.into(),
        paused: reason.map(str::to_owned),
        turns: 1,
        native: true,
        id: Some(1),
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
    }
}

fn backend(out: &Outcome) -> Vec<&BackendRequest> {
    out.effects
        .iter()
        .filter_map(|e| match e {
            Effect::Backend(r) => Some(r),
            _ => None,
        })
        .collect()
}

#[test]
fn an_engine_that_owns_goals_disables_the_desktop_runner_and_takes_intent() {
    let mut s = state();
    s.mode = Mode::Real;
    s.set_connection(Connection::Ready);
    opened(&mut s);
    s.apply_event(unscoped(&s, EventKind::GoalSynced { goal: None }));
    assert!(s.current().unwrap().goal_native);
    assert!(s.current().unwrap().goal.is_none());
    // Setting a goal sends intent; nothing is journaled or submitted locally.
    let out = s.dispatch(Command::SetGoal("count to two".into()));
    assert!(
        matches!(backend(&out).as_slice(), [BackendRequest::SetGoal { objective, .. }] if objective == "count to two")
    );
    assert!(
        !out.effects
            .iter()
            .any(|e| matches!(e, Effect::JournalIntent { .. } | Effect::SaveGoal { .. }))
    );
    assert!(
        s.current().unwrap().goal.is_none(),
        "the engine's report shows it"
    );
    s.apply_event(unscoped(
        &s,
        EventKind::GoalSynced {
            goal: Some(native_goal("count to two", GoalState::Active, None)),
        },
    ));
    let goal = s.current().unwrap().goal.clone().unwrap();
    assert!(goal.native && goal.paused.is_none());
    // Pause and clear are engine controls; Stop does not pause locally.
    let out = s.dispatch(Command::PauseGoal);
    assert!(matches!(
        backend(&out).as_slice(),
        [BackendRequest::GoalControl {
            action: GoalControlAction::Pause,
            ..
        }]
    ));
    assert!(s.current().unwrap().goal.as_ref().unwrap().paused.is_none());
    let out = s.dispatch(Command::ResumeGoal);
    assert!(backend(&out).is_empty(), "an active goal is not resumed");
    s.apply_event(unscoped(
        &s,
        EventKind::GoalSynced {
            goal: Some(native_goal(
                "count to two",
                GoalState::Paused,
                Some("Paused by you."),
            )),
        },
    ));
    let out = s.dispatch(Command::ResumeGoal);
    assert!(matches!(
        backend(&out).as_slice(),
        [BackendRequest::GoalControl {
            action: GoalControlAction::Resume,
            ..
        }]
    ));
    let out = s.dispatch(Command::ClearGoal);
    assert!(matches!(
        backend(&out).as_slice(),
        [BackendRequest::GoalControl {
            action: GoalControlAction::Archive,
            ..
        }]
    ));
    assert!(
        s.current().unwrap().goal.is_some(),
        "until the engine reports"
    );
    s.apply_event(unscoped(&s, EventKind::GoalSynced { goal: None }));
    assert!(s.current().unwrap().goal.is_none());
    // A completed turn never makes the desktop schedule anything.
    let out = s.apply_event(event(&s, EventKind::Completed));
    assert!(
        !out.effects
            .iter()
            .any(|e| matches!(e, Effect::JournalIntent { .. }))
    );
    // Reopening forgets the engine's goal until it reports again.
    opened(&mut s);
    assert!(!s.current().unwrap().goal_native);
}

#[test]
fn a_desktop_kept_goal_is_offered_to_the_engine_once_and_retired_when_it_reports() {
    let mut s = state();
    s.mode = Mode::Real;
    s.set_connection(Connection::Ready);
    s.restore_goal(
        ConversationId(1),
        "ship v1".into(),
        Some("Goal met.".into()),
    );
    opened(&mut s);
    assert!(
        s.current()
            .unwrap()
            .goal
            .as_ref()
            .is_some_and(|g| !g.native)
    );
    let out = s.apply_event(unscoped(&s, EventKind::GoalSynced { goal: None }));
    let requests = backend(&out);
    let [
        BackendRequest::MigrateGoal {
            key, text, paused, ..
        },
    ] = requests.as_slice()
    else {
        panic!("{requests:?}");
    };
    assert!(key.starts_with("desktop-"));
    assert_eq!(text, "ship v1");
    assert_eq!(paused.as_deref(), Some("Goal met."));
    assert!(
        s.current().unwrap().goal.is_some(),
        "kept visible until the engine answers"
    );
    // A second empty report while waiting changes nothing and asks nothing again.
    let out = s.apply_event(unscoped(&s, EventKind::GoalSynced { goal: None }));
    assert!(backend(&out).is_empty());
    assert!(s.current().unwrap().goal.is_some());
    // The engine reports the migrated goal: the desktop record is retired.
    let out = s.apply_event(unscoped(
        &s,
        EventKind::GoalSynced {
            goal: Some(native_goal("ship v1", GoalState::Paused, Some("Goal met."))),
        },
    ));
    assert!(
        out.effects
            .iter()
            .any(|e| matches!(e, Effect::SaveGoal { text: None, .. }))
    );
    let goal = s.current().unwrap().goal.clone().unwrap();
    assert!(goal.native);
    assert_eq!(goal.paused.as_deref(), Some("Goal met."));
    // Without an engine report (legacy Pi), the desktop runner still works as before.
    let mut legacy = state();
    legacy.restore_goal(ConversationId(1), "ship v1".into(), None);
    let out = legacy.dispatch(Command::ResumeGoal);
    assert!(
        out.effects
            .iter()
            .any(|e| matches!(e, Effect::JournalIntent { .. }))
    );
    assert!(
        legacy
            .current()
            .unwrap()
            .goal
            .as_ref()
            .unwrap()
            .paused
            .is_none()
    );
    let out = legacy.dispatch(Command::PauseGoal);
    assert!(out.effects.iter().any(|e| matches!(
        e,
        Effect::SaveGoal {
            paused: Some(_),
            ..
        }
    )));
}
