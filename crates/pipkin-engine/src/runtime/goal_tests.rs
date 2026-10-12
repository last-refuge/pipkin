//! The engine-owned goal runner through the client contract: scheduling from explicit
//! markers only, pause reasons, Stop, restart, replacement, detachment, human-input
//! precedence, migration, control and the published goal.

use std::time::Duration;

use pipkin_engine_protocol::*;

use super::tests::{Engine, Fixture};
use crate::goals;
use crate::provider::ProviderRequest;
use crate::provider::scripted::{Gate, Reply as R, Scripted};

fn user_text(req: &ProviderRequest) -> String {
    req.messages
        .iter()
        .rev()
        .find_map(|m| match m {
            Message::User { .. } => Some(m.text()),
            _ => None,
        })
        .unwrap_or_default()
}

/// Answers goal turns from `replies` in order (the n-th goal turn gets the n-th reply, the
/// rest hang); anything else is echoed.
fn goal_script(replies: Vec<&'static str>) -> impl Fn(&ProviderRequest, usize) -> R + Send + Sync {
    let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    move |req, _| {
        if user_text(req).starts_with(goals::PROMPT_PREFIX) {
            let n = count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            match replies.get(n) {
                Some(text) => R::Text((*text).to_owned()),
                None => R::Hang,
            }
        } else {
            R::Text(format!("echo: {}", user_text(req)))
        }
    }
}

fn goal_of(e: &mut Engine, s: &str) -> Option<GoalView> {
    e.snapshot(s).goal
}

fn set_goal(e: &mut Engine, s: &str, objective: &str) -> Option<GoalView> {
    match e.call(Call::SetGoal {
        session: s.into(),
        objective: objective.into(),
    }) {
        Ok(Reply::Goal(g)) => g,
        other => panic!("{other:?}"),
    }
}

fn control(e: &mut Engine, s: &str, action: GoalAction) -> Result<Option<GoalView>, ProtoError> {
    match e.call(Call::GoalControl {
        session: s.into(),
        action,
    }) {
        Ok(Reply::Goal(g)) => Ok(g),
        Ok(other) => panic!("{other:?}"),
        Err(e) => Err(e),
    }
}

fn until_state(e: &mut Engine, s: &str, state: GoalState) -> GoalView {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(goal) = goal_of(e, s)
            && goal.state == state
        {
            return goal;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "goal never reached {state:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn goal_changes(events: &[Event], s: &str) -> Vec<Option<GoalView>> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Session {
                session,
                change: SessionChange::GoalChanged { goal },
                ..
            } if session == s => Some(goal.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_goal_continues_only_on_explicit_markers_and_completes_on_met() {
    let fx = Fixture::new();
    let provider = Scripted::new(goal_script(vec![
        "One.\n[PIPKIN_GOAL_CONTINUE]",
        "Two.\n[PIPKIN_GOAL_MET]",
    ]));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.snapshot(&s);
    e.snapshot(&s);
    let goal = set_goal(&mut e, &s, "count to two").unwrap();
    assert_eq!(goal.state, GoalState::Active);
    assert_eq!(goal.provenance, "user");
    let done = until_state(&mut e, &s, GoalState::Completed);
    assert_eq!(done.turns, 2);
    assert_eq!(done.epoch, 2);
    assert_eq!(done.last_request.as_deref(), Some("goal:1:2"));
    assert_eq!(done.reason.as_deref(), Some(goals::REASON_MET));
    let requests = provider.requests();
    assert_eq!(requests.len(), 2);
    assert!(user_text(&requests[0]).starts_with("Pursue this ongoing goal: count to two\n\n"));
    assert_eq!(e.settled("goal:1:1"), RunOutcome::Completed);
    // Published as it went: active, scheduled turns, then completed.
    e.until("goal changes", |ev| goal_changes(ev, &s).len() >= 4);
    let states: Vec<GoalState> = goal_changes(&e.events, &s)
        .into_iter()
        .map(|g| g.unwrap().state)
        .collect();
    assert_eq!(*states.last().unwrap(), GoalState::Completed);
    assert!(
        states
            .iter()
            .all(|st| matches!(st, GoalState::Active | GoalState::Completed))
    );
    // No history yet (the goal is current, not archived); resume is refused for a completed goal.
    assert!(
        matches!(e.call(Call::GoalHistory { session: s.clone() }), Ok(Reply::Goals(v)) if v.is_empty())
    );
    let err = control(&mut e, &s, GoalAction::Resume).unwrap_err();
    assert_eq!(err.code, ErrorCode::Invalid);
}

#[test]
fn blocked_unreachable_and_ambiguous_replies_stop_scheduling_with_their_reason() {
    for (reply, state, reason) in [
        (
            "need you\n[PIPKIN_GOAL_BLOCKED]",
            GoalState::Blocked,
            goals::REASON_BLOCKED,
        ),
        (
            "nope\n[PIPKIN_GOAL_UNREACHABLE]",
            GoalState::Failed,
            goals::REASON_UNREACHABLE,
        ),
        ("hmm, maybe", GoalState::Paused, goals::REASON_AMBIGUOUS),
        (
            "[PIPKIN_GOAL_MET] and then some",
            GoalState::Paused,
            goals::REASON_AMBIGUOUS,
        ),
    ] {
        let fx = Fixture::new();
        let provider = Scripted::new(goal_script(vec![reply, "Two.\n[PIPKIN_GOAL_MET]"]));
        let mut e = fx.start(&provider);
        let s = e.create(&fx.project, "c");
        e.snapshot(&s);
        e.snapshot(&s);
        set_goal(&mut e, &s, "try");
        let goal = until_state(&mut e, &s, state);
        assert_eq!(goal.reason.as_deref(), Some(reason), "{reply}");
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(provider.requests().len(), 1, "{reply}: nothing was resent");
        if state.resumable() {
            // Resume schedules a fresh epoch; the next reply completes it.
            control(&mut e, &s, GoalAction::Resume).unwrap();
            let done = until_state(&mut e, &s, GoalState::Completed);
            assert_eq!(done.epoch, 2);
            assert_eq!(provider.requests().len(), 2);
        }
    }
}

#[test]
fn stop_pauses_the_goal_that_exists_and_a_replacement_is_not_paused_by_it() {
    let fx = Fixture::new();
    let gate = Gate::new();
    let g = gate.clone();
    let provider = Scripted::new(move |req, _| {
        let text = user_text(req);
        if text.contains("commit and push") {
            R::Gated(
                g.clone(),
                Box::new(R::Text("partial\n[PIPKIN_GOAL_CONTINUE]".into())),
            )
        } else {
            R::Text("1 2 3 ... 100\n[PIPKIN_GOAL_MET]".into())
        }
    });
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.snapshot(&s);
    e.snapshot(&s);
    set_goal(&mut e, &s, "commit and push");
    e.until("the goal turn is running", |_| {
        !provider.requests().is_empty()
    });
    e.call(Call::Stop { session: s.clone() }).unwrap();
    let paused = goal_of(&mut e, &s).unwrap();
    assert_eq!(paused.state, GoalState::Paused);
    assert_eq!(paused.reason.as_deref(), Some(goals::REASON_STOP));
    assert_eq!(e.settled("goal:1:1"), RunOutcome::Aborted);
    gate.open();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        goal_of(&mut e, &s).unwrap().state,
        GoalState::Paused,
        "a stopped goal does not continue"
    );
    // A replacement goal archives the old one and runs on its own.
    let replacement = set_goal(&mut e, &s, "count to 100").unwrap();
    assert_eq!(replacement.id, 2);
    let done = until_state(&mut e, &s, GoalState::Completed);
    assert_eq!(done.id, 2);
    assert_eq!(done.turns, 1);
    let history = match e.call(Call::GoalHistory { session: s.clone() }) {
        Ok(Reply::Goals(v)) => v,
        other => panic!("{other:?}"),
    };
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].id, 1);
    assert_eq!(
        history[0].state,
        GoalState::Paused,
        "its own state is kept in history"
    );
    assert_eq!(provider.requests().len(), 2);
}

#[test]
fn a_restart_pauses_the_goal_until_the_person_resumes() {
    let fx = Fixture::new();
    let s;
    {
        let provider = Scripted::new(goal_script(vec![]));
        let mut e = fx.start(&provider);
        s = e.create(&fx.project, "c");
        e.snapshot(&s);
        set_goal(&mut e, &s, "keep going");
        e.until("the turn started", |_| !provider.requests().is_empty());
        e.handle.shutdown(Duration::from_millis(200));
    }
    let provider = Scripted::new(goal_script(vec![
        "more\n[PIPKIN_GOAL_CONTINUE]",
        "done\n[PIPKIN_GOAL_MET]",
    ]));
    let mut e = fx.start(&provider);
    let goal = goal_of(&mut e, &s).unwrap();
    assert_eq!(goal.state, GoalState::Paused);
    assert_eq!(goal.reason.as_deref(), Some(goals::REASON_RESTART));
    // The interrupted turn itself recovers and answers CONTINUE, but a paused goal does
    // not schedule another.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(Reply::Lookup(LookupResult::Known {
            open: false,
            outcome: Some(_),
            ..
        })) = e.call(Call::Lookup {
            session: s.clone(),
            request: "goal:1:1".into(),
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the turn did not settle"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(goal_of(&mut e, &s).unwrap().state, GoalState::Paused);
    assert_eq!(provider.requests().len(), 1);
    control(&mut e, &s, GoalAction::Resume).unwrap();
    let done = until_state(&mut e, &s, GoalState::Completed);
    assert_eq!(done.epoch, 2);
    assert_eq!(provider.requests().len(), 2);
}

#[test]
fn no_continuation_is_scheduled_while_no_client_is_attached() {
    let fx = Fixture::new();
    let gate = Gate::new();
    let g = gate.clone();
    let provider = Scripted::new(move |req, _| {
        if user_text(req).starts_with(goals::PROMPT_PREFIX) {
            R::Gated(
                g.clone(),
                Box::new(R::Text("more\n[PIPKIN_GOAL_CONTINUE]".into())),
            )
        } else {
            R::Text("x".into())
        }
    });
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.snapshot(&s);
    e.snapshot(&s);
    set_goal(&mut e, &s, "work");
    e.until("the turn started", |_| !provider.requests().is_empty());
    let Engine { handle, conn, .. } = e;
    drop(conn);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while handle.shared().session_attached(&s) {
        assert!(
            std::time::Instant::now() < deadline,
            "client never released the session"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    gate.open();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let goal = handle.shared().goal_view(&s).unwrap().unwrap();
        if goal.state == GoalState::Paused {
            assert_eq!(goal.reason.as_deref(), Some(goals::REASON_DETACHED));
            break;
        }
        assert!(std::time::Instant::now() < deadline, "goal did not pause");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(provider.requests().len(), 1, "no turn for an empty room");
}

#[test]
fn a_queued_human_message_takes_precedence_and_pauses_the_goal() {
    let fx = Fixture::new();
    let gate = Gate::new();
    let g = gate.clone();
    let provider = Scripted::new(move |req, _| {
        if user_text(req).starts_with(goals::PROMPT_PREFIX) {
            R::Gated(
                g.clone(),
                Box::new(R::Text("more\n[PIPKIN_GOAL_CONTINUE]".into())),
            )
        } else {
            R::Text(format!("echo: {}", user_text(req)))
        }
    });
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.snapshot(&s);
    e.snapshot(&s);
    set_goal(&mut e, &s, "work");
    e.until("the turn started", |_| !provider.requests().is_empty());
    e.call(Call::Enqueue {
        session: s.clone(),
        request: "fu".into(),
        mode: InputMode::FollowUp,
        text: "actually, stop and tell me".into(),
        images: vec![],
    })
    .unwrap();
    gate.open();
    assert_eq!(e.settled("fu"), RunOutcome::Completed);
    let goal = until_state(&mut e, &s, GoalState::Paused);
    assert_eq!(goal.reason.as_deref(), Some(goals::REASON_USER_INPUT));
    let texts: Vec<String> = provider.requests().iter().map(user_text).collect();
    assert_eq!(texts.len(), 2, "{texts:?}");
    assert!(texts[1].contains("actually, stop"));
}

#[test]
fn migration_imports_a_desktop_goal_as_paused_exactly_once_and_never_replaces() {
    let fx = Fixture::new();
    let provider = Scripted::new(goal_script(vec!["done\n[PIPKIN_GOAL_MET]"]));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.snapshot(&s);
    e.snapshot(&s);
    let migrate = |e: &mut Engine, key: &str, text: &str| match e.call(Call::MigrateGoal {
        session: s.clone(),
        key: key.into(),
        objective: text.into(),
        paused: Some("Goal met.".into()),
    }) {
        Ok(Reply::Goal(g)) => g,
        other => panic!("{other:?}"),
    };
    let imported = migrate(&mut e, "desktop-1", "ship v1").unwrap();
    assert_eq!(imported.state, GoalState::Paused);
    assert_eq!(imported.reason.as_deref(), Some("Goal met."));
    assert_eq!(imported.provenance, "migrated:desktop");
    assert_eq!(imported.turns, 0);
    std::thread::sleep(Duration::from_millis(100));
    assert!(provider.requests().is_empty(), "migration never schedules");
    // The same key, and a different key while a goal exists, both leave it alone.
    assert_eq!(migrate(&mut e, "desktop-1", "ship v1").unwrap().id, 1);
    assert_eq!(
        migrate(&mut e, "desktop-2", "something else")
            .unwrap()
            .objective,
        "ship v1"
    );
    assert!(
        matches!(e.call(Call::GoalHistory { session: s.clone() }), Ok(Reply::Goals(v)) if v.is_empty())
    );
    // Resuming a migrated goal runs it like any other.
    control(&mut e, &s, GoalAction::Resume).unwrap();
    let done = until_state(&mut e, &s, GoalState::Completed);
    assert_eq!(done.provenance, "migrated:desktop");
}

#[test]
fn the_person_can_pause_resume_revise_and_archive() {
    let fx = Fixture::new();
    let gate = Gate::new();
    let g = gate.clone();
    let provider = Scripted::new(move |req, _| {
        if user_text(req).starts_with(goals::PROMPT_PREFIX) {
            R::Gated(
                g.clone(),
                Box::new(R::Text("more\n[PIPKIN_GOAL_CONTINUE]".into())),
            )
        } else {
            R::Text("x".into())
        }
    });
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.snapshot(&s);
    e.snapshot(&s);
    assert_eq!(
        control(&mut e, &s, GoalAction::Pause).unwrap_err().code,
        ErrorCode::NotFound
    );
    set_goal(&mut e, &s, "first");
    e.until("the turn started", |_| !provider.requests().is_empty());
    let paused = control(&mut e, &s, GoalAction::Pause).unwrap().unwrap();
    assert_eq!(
        paused.reason.as_deref(),
        Some(goals::REASON_PAUSED_BY_PERSON)
    );
    gate.open();
    assert_eq!(e.settled("goal:1:1"), RunOutcome::Completed);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        goal_of(&mut e, &s).unwrap().state,
        GoalState::Paused,
        "paused before the turn settled: no continuation"
    );
    assert_eq!(provider.requests().len(), 1);
    let revised = control(
        &mut e,
        &s,
        GoalAction::Revise {
            objective: "second".into(),
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(revised.objective, "second");
    assert_eq!(revised.revision, 2);
    assert_eq!(revised.state, GoalState::Active);
    e.until("the revised turn started", |_| {
        provider.requests().len() == 2
    });
    assert!(user_text(&provider.requests()[1]).contains("Pursue this ongoing goal: second"));
    let archived = control(&mut e, &s, GoalAction::Archive).unwrap();
    assert!(archived.is_none());
    assert!(goal_of(&mut e, &s).is_none());
    let history = match e.call(Call::GoalHistory { session: s.clone() }) {
        Ok(Reply::Goals(v)) => v,
        other => panic!("{other:?}"),
    };
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].state, GoalState::Cancelled);
    assert_eq!(history[0].reason.as_deref(), Some(goals::REASON_ARCHIVED));
    let welcome = match e.call(Call::Hello {
        protocol: PROTOCOL_VERSION,
        client: "t".into(),
    }) {
        Ok(Reply::Welcome(w)) => w,
        other => panic!("{other:?}"),
    };
    assert!(welcome.capabilities.iter().any(|c| c == "goals"));
    let err = e
        .call(Call::SetGoal {
            session: s.clone(),
            objective: "   ".into(),
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Invalid);
}
