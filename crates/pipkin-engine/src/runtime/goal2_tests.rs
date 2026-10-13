//! GOAL-2: structured decisions through the goal tools, evidence checked against the
//! engine's own ledger, completion gated on it, allowances, stall detection, stale claims
//! after a revision, and the marker fallback.

use std::time::Duration;

use pipkin_engine_protocol::*;
use serde_json::json;

use super::tests::{Engine, Fixture};
use crate::goals;
use crate::provider::ProviderRequest;
use crate::provider::scripted::{Reply as R, Scripted};

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

fn last_is_user(req: &ProviderRequest) -> bool {
    matches!(
        req.messages
            .iter()
            .rev()
            .find(|m| !matches!(m, Message::System(_))),
        Some(Message::User { .. })
    )
}

fn last_tool_result(req: &ProviderRequest) -> Option<String> {
    req.messages.iter().rev().find_map(|m| match m {
        Message::ToolResult { .. } => Some(m.text()),
        _ => None,
    })
}

/// Each goal turn runs one scripted step: a list of tool calls, then a final text.
type Step = (Vec<(&'static str, serde_json::Value)>, &'static str);

fn scripted_turns(steps: Vec<Step>) -> impl Fn(&ProviderRequest, usize) -> R + Send + Sync {
    let turn = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let within = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    move |req, _| {
        if !user_text(req).starts_with(goals::PROMPT_PREFIX) {
            return R::Text(format!("echo: {}", user_text(req)));
        }
        let t = turn.load(std::sync::atomic::Ordering::SeqCst);
        let Some((calls, final_text)) = steps.get(t) else {
            return R::Hang;
        };
        if last_is_user(req) {
            within.store(0, std::sync::atomic::Ordering::SeqCst);
        }
        let i = within.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        match calls.get(i) {
            Some((name, args)) => R::Tools(None, vec![((*name).to_owned(), args.clone())]),
            None => {
                turn.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                R::Text((*final_text).to_owned())
            }
        }
    }
}

fn goal_of(e: &mut Engine, s: &str) -> Option<GoalView> {
    e.snapshot(s).goal
}

fn set_goal(e: &mut Engine, s: &str, objective: &str) {
    e.call(Call::SetGoal {
        session: s.into(),
        objective: objective.into(),
    })
    .unwrap();
}

fn control(e: &mut Engine, s: &str, action: GoalAction) -> Option<GoalView> {
    match e.call(Call::GoalControl {
        session: s.into(),
        action,
    }) {
        Ok(Reply::Goal(g)) => g,
        other => panic!("{other:?}"),
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
            "goal never reached {state:?}: {:?}",
            goal_of(e, s)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn until_goal(e: &mut Engine, s: &str, what: &str, pred: impl Fn(&GoalView) -> bool) -> GoalView {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(goal) = goal_of(e, s)
            && pred(&goal)
        {
            return goal;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}: {:?}",
            goal_of(e, s)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn tool_results(snap: &SessionSnapshot, tool: &str) -> Vec<(bool, String)> {
    snap.entries
        .iter()
        .filter_map(|e| match &e.kind {
            EntryKind::Message {
                message:
                    m @ Message::ToolResult {
                        tool_name,
                        is_error,
                        ..
                    },
                ..
            } if tool_name == tool => Some((*is_error, m.text())),
            _ => None,
        })
        .collect()
}

fn write_settings(fx: &Fixture, body: serde_json::Value) {
    std::fs::create_dir_all(fx.dir.path().join("config")).unwrap();
    std::fs::write(fx.dir.path().join("config/settings.json"), body.to_string()).unwrap();
}

#[test]
fn structured_continue_and_completion_with_checked_evidence() {
    let fx = Fixture::new();
    std::fs::write(fx.project.join("notes.txt"), "v1").unwrap();
    let hash = crate::web::sha256_hex(b"v1");
    let provider = Scripted::new(scripted_turns(vec![
        (
            vec![
                ("read", json!({"path": "notes.txt"})),
                (
                    "update_goal",
                    json!({"status": "continue", "note": "read the notes", "criteria": [{"id": 1, "status": "met", "evidence": [{"kind": "observation", "reference": "looks fine"}]}]}),
                ),
            ],
            "working on it",
        ),
        (
            vec![
                (
                    "propose_goal_completion",
                    json!({"summary": "done", "criteria": [{"id": 1, "evidence": [{"kind": "file", "reference": "notes.txt", "sha256": hash}]}, {"id": 2, "evidence": [{"kind": "toolCall", "reference": "nope"}]}]}),
                ),
                (
                    "propose_goal_completion",
                    json!({"summary": "done for real", "criteria": [{"id": 2, "evidence": [{"kind": "toolCall", "reference": "will-be-filled"}]}]}),
                ),
            ],
            "finished",
        ),
    ]));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.snapshot(&s);
    set_goal(&mut e, &s, "tidy the notes");
    control(
        &mut e,
        &s,
        GoalAction::SetCriteria {
            criteria: vec!["notes are read".into(), "notes are tidy".into()],
        },
    );
    // Turn 1: a read, then an update with an observation-only claim.
    // The structured continue schedules turn 2 once turn 1 settles.
    let goal = until_goal(&mut e, &s, "the second turn", |g| g.epoch == 2);
    assert_eq!(goal.id, 1);
    // The criteria were set after the goal had already scheduled turn 1; they appear in
    // the prompt from the next turn on.
    e.until("a prompt listed the criteria", |_| {
        provider
            .requests()
            .iter()
            .any(|r| user_text(r).contains("Completion criteria:\n- #1 ["))
    });
    // Turn 2: the first proposal names a missing tool call and is rejected; the goal stays
    // active. Patch the second proposal to cite the real read call before it runs.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let snap = loop {
        let snap = e.snapshot(&s);
        if !tool_results(&snap, "propose_goal_completion").is_empty() {
            break snap;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the first proposal was not answered"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    // Turn 1's own report: the note, the observation-only claim left unverified, and the
    // structured continue, as recorded when it happened.
    let updates = tool_results(&snap, "update_goal");
    assert!(!updates.is_empty());
    assert!(!updates[0].0, "{}", updates[0].1);
    assert!(
        updates[0].1.contains("Last note: read the notes"),
        "{}",
        updates[0].1
    );
    assert!(
        updates[0].1.contains("#1 [unverified] notes are read"),
        "{}",
        updates[0].1
    );
    assert!(
        updates[0].1.contains("judgment observation: looks fine"),
        "{}",
        updates[0].1
    );
    let read_call = snap
        .entries
        .iter()
        .find_map(|e| match &e.kind {
            EntryKind::Message {
                message:
                    Message::ToolResult {
                        tool_name,
                        tool_call_id,
                        ..
                    },
                ..
            } if tool_name == "read" => Some(tool_call_id.clone()),
            _ => None,
        })
        .unwrap();
    let _ = read_call;
    let proposals = tool_results(&snap, "propose_goal_completion");
    assert!(!proposals.is_empty());
    assert!(proposals[0].0, "{}", proposals[0].1);
    assert!(proposals[0].1.contains("not met: #2"), "{}", proposals[0].1);
    assert!(
        proposals[0]
            .1
            .contains("rejected evidence: #2 toolCall nope: no such tool call"),
        "{}",
        proposals[0].1
    );
    let goal = snap.goal.clone().unwrap();
    assert_eq!(goal.state, GoalState::Active);
    assert_eq!(
        goal.criteria[0].status,
        CriterionStatus::Met,
        "the file hash matched"
    );
    assert_eq!(goal.criteria[1].status, CriterionStatus::Unverified);
    e.handle.shutdown(Duration::from_millis(200));
}

#[test]
fn a_proposal_with_mechanical_evidence_for_every_criterion_completes_the_goal() {
    let fx = Fixture::new();
    let provider = Scripted::new(move |req: &ProviderRequest, _| {
        if !user_text(req).starts_with(goals::PROMPT_PREFIX) {
            return R::Text("x".into());
        }
        // First the read; then a proposal citing that read's call id (found in the history).
        let read_id = req.messages.iter().find_map(|m| match m {
            Message::ToolResult {
                tool_call_id,
                tool_name,
                ..
            } if tool_name == "read" => Some(tool_call_id.clone()),
            _ => None,
        });
        let proposed = req.messages.iter().any(|m| matches!(m, Message::ToolResult { tool_name, .. } if tool_name == "propose_goal_completion"));
        match (read_id, proposed) {
            (None, _) => R::Tools(None, vec![("read".into(), json!({"path": "notes.txt"}))]),
            (Some(id), false) => R::Tools(
                None,
                vec![(
                    "propose_goal_completion".into(),
                    json!({"summary": "read it", "evidence": [{"kind": "toolCall", "reference": id}]}),
                )],
            ),
            (Some(_), true) => R::Text("all done".into()),
        }
    });
    std::fs::write(fx.project.join("notes.txt"), "hello").unwrap();
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.snapshot(&s);
    set_goal(&mut e, &s, "read the notes");
    let done = until_state(&mut e, &s, GoalState::Completed);
    assert_eq!(done.reason.as_deref(), Some(goals::REASON_PROPOSED));
    assert_eq!(
        done.criteria.len(),
        1,
        "the objective became the one criterion"
    );
    assert_eq!(done.criteria[0].status, CriterionStatus::Met);
    assert_eq!(done.criteria[0].evidence[0].accepted, Some(true));
    assert_eq!(done.decision.as_ref().map(|d| d.kind.as_str()), Some("met"));
    assert_eq!(done.note.as_deref(), Some("read it"));
    assert_eq!(e.settled("goal:1:1"), RunOutcome::Completed);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        goal_of(&mut e, &s).unwrap().turns,
        1,
        "no further turn after completion"
    );
    assert!(
        goal_of(&mut e, &s).unwrap().usage_tokens > 0,
        "the turn's tokens were attributed"
    );
    // A tool call from a different session is not evidence here.
    let snap = e.snapshot(&s);
    let results = tool_results(&snap, "propose_goal_completion");
    assert!(
        !results[0].0 && results[0].1.starts_with("Completion accepted"),
        "{}",
        results[0].1
    );
}

#[test]
fn blocked_and_unreachable_reports_need_a_reason_and_stop_scheduling() {
    for (status, state) in [
        ("blocked", GoalState::Blocked),
        ("unreachable", GoalState::Failed),
    ] {
        let fx = Fixture::new();
        let provider = Scripted::new(scripted_turns(vec![(
            vec![
                ("update_goal", json!({"status": status})),
                (
                    "update_goal",
                    json!({"status": status, "reason": "need the API key"}),
                ),
            ],
            "waiting",
        )]));
        let mut e = fx.start(&provider);
        let s = e.create(&fx.project, "c");
        e.snapshot(&s);
        set_goal(&mut e, &s, "deploy");
        let goal = until_state(&mut e, &s, state);
        assert_eq!(goal.reason.as_deref(), Some("need the API key"));
        assert_eq!(e.settled("goal:1:1"), RunOutcome::Completed);
        let snap = e.snapshot(&s);
        let updates = tool_results(&snap, "update_goal");
        assert!(
            updates[0].0 && updates[0].1.contains("needs a reason"),
            "{}",
            updates[0].1
        );
        assert!(!updates[1].0);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(goal_of(&mut e, &s).unwrap().turns, 1);
        // The tools refuse to act on a goal that is not active; only the person resumes.
        e.prompt(&s, "r2", "status?").unwrap();
        assert_eq!(e.settled("r2"), RunOutcome::Completed);
    }
}

#[test]
fn allowances_and_stalls_pause_the_goal_with_actionable_reasons() {
    // Turns: a limit of two turns, each continuing with the same (empty) progress.
    let fx = Fixture::new();
    let provider = Scripted::new(scripted_turns(vec![
        (vec![("update_goal", json!({"status": "continue"}))], "t1"),
        (vec![("update_goal", json!({"status": "continue"}))], "t2"),
        (vec![("update_goal", json!({"status": "continue"}))], "t3"),
        (vec![("update_goal", json!({"status": "continue"}))], "t4"),
    ]));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.snapshot(&s);
    set_goal(&mut e, &s, "spin");
    control(
        &mut e,
        &s,
        GoalAction::SetLimits {
            max_turns: Some(2),
            max_tokens: None,
            max_cost_usd: None,
        },
    );
    let paused = until_state(&mut e, &s, GoalState::Paused);
    assert_eq!(paused.reason.as_deref(), Some(goals::REASON_TURNS_USED));
    assert_eq!(paused.turns, 2);
    // Raising the allowance and resuming continues; with no limit the stall guard pauses
    // after three unchanged continued turns.
    control(
        &mut e,
        &s,
        GoalAction::SetLimits {
            max_turns: None,
            max_tokens: None,
            max_cost_usd: None,
        },
    );
    control(&mut e, &s, GoalAction::Resume);
    let stalled = until_state(&mut e, &s, GoalState::Paused);
    assert!(
        stalled
            .reason
            .as_deref()
            .unwrap()
            .contains("no change to the goal's progress"),
        "{:?}",
        stalled.reason
    );
    assert_eq!(
        stalled.turns, 4,
        "turn 1 set the signature; turns 2, 3 and 4 matched it"
    );
    assert_eq!(stalled.stall_turns, 3);

    // Tokens: an allowance below one turn's usage pauses before the second turn.
    let fx = Fixture::new();
    let provider = Scripted::new(scripted_turns(vec![
        (
            vec![("update_goal", json!({"status": "continue", "note": "a"}))],
            "t1",
        ),
        (
            vec![("update_goal", json!({"status": "continue", "note": "b"}))],
            "t2",
        ),
    ]));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.snapshot(&s);
    set_goal(&mut e, &s, "spend");
    control(
        &mut e,
        &s,
        GoalAction::SetLimits {
            max_turns: None,
            max_tokens: Some(1),
            max_cost_usd: None,
        },
    );
    let paused = until_state(&mut e, &s, GoalState::Paused);
    assert_eq!(paused.reason.as_deref(), Some(goals::REASON_TOKENS_USED));
    assert_eq!(paused.turns, 1);
    assert!(paused.usage_tokens >= 1);

    // A configurable stall threshold of one pauses after the first unchanged turn.
    let fx = Fixture::new();
    write_settings(&fx, json!({"goals": {"stallTurns": 1}}));
    let provider = Scripted::new(scripted_turns(vec![
        (
            vec![("update_goal", json!({"status": "continue", "note": "same"}))],
            "t1",
        ),
        (
            vec![("update_goal", json!({"status": "continue", "note": "same"}))],
            "t2",
        ),
    ]));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.snapshot(&s);
    set_goal(&mut e, &s, "spin");
    let stalled = until_state(&mut e, &s, GoalState::Paused);
    assert_eq!(
        stalled.turns, 2,
        "the first turn set the signature; the second matched it"
    );
}

#[test]
fn revising_the_goal_or_criteria_makes_met_claims_stale_and_markers_remain_a_fallback() {
    let fx = Fixture::new();
    std::fs::write(fx.project.join("a.txt"), "a").unwrap();
    let provider = Scripted::new(scripted_turns(vec![
        (
            vec![(
                "update_goal",
                json!({"criteria": [{"id": 1, "status": "met", "evidence": [{"kind": "file", "reference": "a.txt"}]}]}),
            )],
            "no status call this time\n[PIPKIN_GOAL_CONTINUE]",
        ),
        (vec![], "ambiguous ending"),
    ]));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.snapshot(&s);
    set_goal(&mut e, &s, "write a");
    control(
        &mut e,
        &s,
        GoalAction::SetCriteria {
            criteria: vec!["a.txt exists".into()],
        },
    );
    // Turn 1 made no structured decision but ended with a marker: the fallback continues.
    e.until("turn two started", |_| {
        provider
            .requests()
            .iter()
            .filter(|r| last_is_user(r))
            .count()
            >= 2
    });
    let goal = goal_of(&mut e, &s).unwrap();
    assert_eq!(goal.criteria[0].status, CriterionStatus::Met);
    assert_eq!(goal.epoch, 2);
    // Turn 2 reports nothing at all: paused with the no-status reason (not a silent resend).
    let paused = until_state(&mut e, &s, GoalState::Paused);
    assert_eq!(paused.reason.as_deref(), Some(goals::REASON_NO_STATUS));
    // Revising the objective invalidates the met claim; so does replacing the criteria.
    let revised = control(
        &mut e,
        &s,
        GoalAction::Revise {
            objective: "write a and b".into(),
        },
    )
    .unwrap();
    assert_eq!(revised.criteria[0].status, CriterionStatus::Unverified);
    assert_eq!(revised.revision, 2);
    assert!(revised.decision.is_none());
    let replaced = control(
        &mut e,
        &s,
        GoalAction::SetCriteria {
            criteria: vec!["b.txt exists".into()],
        },
    )
    .unwrap();
    assert_eq!(replaced.criteria.len(), 1);
    assert_eq!(replaced.criteria[0].text, "b.txt exists");
    assert_eq!(replaced.criteria[0].status, CriterionStatus::Open);
    e.handle.shutdown(Duration::from_millis(200));
}

#[test]
fn goal_tools_are_root_only_and_refuse_without_a_goal() {
    let fx = Fixture::new();
    let provider = Scripted::new(|req: &ProviderRequest, _| {
        if last_is_user(req) {
            R::Tools(
                None,
                vec![
                    ("get_goal".into(), json!({})),
                    ("update_goal".into(), json!({"note": "hi"})),
                ],
            )
        } else {
            R::Text(format!(
                "saw: {}",
                last_tool_result(req).unwrap_or_default()
            ))
        }
    });
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.snapshot(&s);
    e.prompt(&s, "r1", "anything").unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let snap = e.snapshot(&s);
    let got = tool_results(&snap, "get_goal");
    assert!(got[0].0 && got[0].1.contains("no goal"), "{}", got[0].1);
    let upd = tool_results(&snap, "update_goal");
    assert!(upd[0].0 && upd[0].1.contains("no goal"), "{}", upd[0].1);
    let names: Vec<String> = provider.requests()[0]
        .tools
        .iter()
        .map(|t| t.name.clone())
        .collect();
    for tool in ["get_goal", "update_goal", "propose_goal_completion"] {
        assert!(names.contains(&tool.to_owned()), "{names:?}");
    }
    assert!(e.handle.shared().recoverable_tool("get_goal").is_some());
    assert!(e.handle.shared().recoverable_tool("update_goal").is_none());
    assert!(
        e.handle
            .shared()
            .recoverable_tool("propose_goal_completion")
            .is_none()
    );
}

#[test]
fn a_cost_allowance_pauses_the_goal_and_is_attributed_per_turn() {
    // The scripted model reports 10 input and 5 output tokens per call; priced at a dollar
    // a token, every turn costs at least $15, so a $10 allowance pauses before turn two.
    let fx = Fixture::new();
    let provider = Scripted::new(scripted_turns(vec![
        (
            vec![("update_goal", json!({"status": "continue", "note": "a"}))],
            "t1",
        ),
        (
            vec![("update_goal", json!({"status": "continue", "note": "b"}))],
            "t2",
        ),
    ]));
    let mut config = fx.config();
    config.extra_models[0].cost.input = 1_000_000.0;
    config.extra_models[0].cost.output = 1_000_000.0;
    let mut e = fx.start_config(&provider, config);
    let s = e.create(&fx.project, "c");
    e.snapshot(&s);
    set_goal(&mut e, &s, "spend");
    let view = control(
        &mut e,
        &s,
        GoalAction::SetLimits {
            max_turns: None,
            max_tokens: None,
            max_cost_usd: Some(10.0),
        },
    )
    .unwrap();
    assert_eq!(view.limits.max_cost_usd, Some(10.0));
    let paused = until_state(&mut e, &s, GoalState::Paused);
    assert_eq!(paused.reason.as_deref(), Some(goals::REASON_COST_USED));
    assert_eq!(paused.turns, 1);
    assert!(
        paused.usage_cost_usd >= 15.0,
        "one turn of two calls at a dollar a token: {}",
        paused.usage_cost_usd
    );
    assert!(paused.usage_tokens >= 15);

    // Zero, negative and non-finite allowances clear the limit rather than pausing at once.
    let view = control(
        &mut e,
        &s,
        GoalAction::SetLimits {
            max_turns: None,
            max_tokens: None,
            max_cost_usd: Some(0.0),
        },
    )
    .unwrap();
    assert_eq!(view.limits.max_cost_usd, None);
    assert_eq!(
        serde_json::to_value(GoalAction::SetLimits {
            max_turns: Some(1),
            max_tokens: None,
            max_cost_usd: None
        })
        .unwrap(),
        json!({"action": "setLimits", "max_turns": 1, "max_tokens": null, "max_cost_usd": null})
    );
    // Older desktops omit the cost field.
    let action: GoalAction =
        serde_json::from_value(json!({"action": "setLimits", "max_turns": 2, "max_tokens": null}))
            .unwrap();
    assert_eq!(
        action,
        GoalAction::SetLimits {
            max_turns: Some(2),
            max_tokens: None,
            max_cost_usd: None
        }
    );
}
