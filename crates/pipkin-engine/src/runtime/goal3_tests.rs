//! GOAL-3: the independent completion audit. An accepted proposal on an audit-required goal
//! goes to a read-only auditor child; approval completes, requested changes feed the next
//! turn and are bounded, anything else pauses, a changed workspace makes approval stale,
//! Stop and restart never approve, and skipping is recorded as unreviewed.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use pipkin_engine_protocol::*;
use serde_json::json;

use super::tests::{Engine, Fixture};
use crate::goals;
use crate::profiles::AUDITOR_TOOLS;
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

fn first_user_text(req: &ProviderRequest) -> String {
    req.messages
        .iter()
        .find_map(|m| match m {
            Message::User { .. } => Some(m.text()),
            _ => None,
        })
        .unwrap_or_default()
}

fn is_auditor(req: &ProviderRequest) -> bool {
    first_user_text(req).starts_with("Audit round")
}

/// The executor proposes completion with file evidence on every goal turn; the auditor
/// answers from `auditor` (called with the audit round number).
fn audit_script(
    hash: String,
    auditor: impl Fn(u32) -> R + Send + Sync + 'static,
) -> impl Fn(&ProviderRequest, usize) -> R + Send + Sync {
    let proposed = Arc::new(Mutex::new(std::collections::HashSet::<String>::new()));
    move |req, _| {
        if is_auditor(req) {
            let round: u32 = first_user_text(req)
                .trim_start_matches("Audit round ")
                .split(':')
                .next()
                .and_then(|n| n.trim().parse().ok())
                .unwrap_or(0);
            return auditor(round);
        }
        let text = user_text(req);
        if !text.starts_with(goals::PROMPT_PREFIX) {
            return R::Text("x".into());
        }
        // One proposal per goal turn (keyed by the turn prompt text, which carries the
        // findings and so differs per round), then a final text.
        let key = req
            .messages
            .iter()
            .filter(|m| matches!(m, Message::User { .. }))
            .count()
            .to_string();
        let mut seen = proposed.lock().unwrap();
        if seen.insert(key) {
            R::Tools(
                None,
                vec![(
                    "propose_goal_completion".into(),
                    json!({"summary": "a.txt is written", "evidence": [{"kind": "file", "reference": "a.txt", "sha256": hash}]}),
                )],
            )
        } else {
            R::Text("proposed".into())
        }
    }
}

fn verdict(verdict: &str, ok: bool, note: &str) -> R {
    R::Text(format!(
        "I checked a.txt.\n{}",
        json!({"verdict": verdict, "findings": [{"criterion": 1, "ok": ok, "note": note}]})
    ))
}

fn goal_of(e: &mut Engine, s: &str) -> Option<GoalView> {
    e.snapshot(s).goal
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

fn start_audited_goal(fx: &Fixture, e: &mut Engine, s: &str) {
    e.snapshot(s);
    e.call(Call::SetGoal {
        session: s.into(),
        objective: "write a.txt".into(),
    })
    .unwrap();
    // The goal is set before the audit flag so the first turn is already scheduled; the
    // proposal happens in that turn and reads the flag at proposal time.
    control(
        e,
        s,
        GoalAction::SetAudit {
            required: true,
            max_rounds: None,
        },
    )
    .unwrap();
    let _ = fx;
}

fn subagents(e: &mut Engine, s: &str) -> SubagentsView {
    match e.call(Call::Subagents { session: s.into() }) {
        Ok(Reply::Subagents(v)) => v,
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_approved_audit_completes_the_goal_through_a_read_only_auditor_child() {
    let fx = Fixture::new();
    std::fs::write(fx.project.join("a.txt"), "done").unwrap();
    let hash = crate::web::sha256_hex(b"done");
    let provider = Scripted::new(audit_script(hash, |_| {
        verdict("approved", true, "a.txt says done")
    }));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    start_audited_goal(&fx, &mut e, &s);
    let done = until_state(&mut e, &s, GoalState::Completed);
    assert!(
        done.reason
            .as_deref()
            .unwrap()
            .starts_with("Audit approved (round 1)"),
        "{:?}",
        done.reason
    );
    let result = done.audit.result.clone().unwrap();
    assert_eq!(result.verdict, "approved");
    assert_eq!(result.round, 1);
    assert!(result.child.is_some());
    assert_eq!(result.findings[0].note.as_deref(), Some("a.txt says done"));
    assert!(
        done.usage_tokens > 0,
        "the audit's tokens count against the goal"
    );
    // The auditor ran as a child with the auditor profile and only inspection tools.
    let auditor = provider
        .requests()
        .into_iter()
        .find(is_auditor)
        .expect("the auditor ran");
    let names: Vec<String> = auditor.tools.iter().map(|t| t.name.clone()).collect();
    assert_eq!(names, AUDITOR_TOOLS);
    assert!(
        auditor
            .resolve_systems(false)
            .system
            .contains("You are auditing another agent's claim")
    );
    assert!(first_user_text(&auditor).contains("engine-accepted file a.txt"));
    let view = subagents(&mut e, &s);
    assert_eq!(view.children.len(), 1);
    assert_eq!(view.children[0].profile.as_deref(), Some("auditor"));
    assert_eq!(view.children[0].status, "done");
    // The verifying state was published on the way.
    e.until("verifying was published", |ev| {
        ev.iter().any(|e| matches!(e, Event::Session { change: SessionChange::GoalChanged { goal: Some(g) }, .. } if g.state == GoalState::Verifying))
    });
}

#[test]
fn requested_changes_feed_the_next_turn_and_are_bounded_then_skipping_is_unreviewed() {
    let fx = Fixture::new();
    std::fs::write(fx.project.join("a.txt"), "done").unwrap();
    let hash = crate::web::sha256_hex(b"done");
    let provider = Scripted::new(audit_script(hash, |round| {
        verdict(
            "changes_requested",
            false,
            &format!("round {round}: not convinced"),
        )
    }));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    start_audited_goal(&fx, &mut e, &s);
    let paused = until_state(&mut e, &s, GoalState::Paused);
    assert!(
        paused
            .reason
            .as_deref()
            .unwrap()
            .contains("requested changes 2 time(s) (the limit is 2)"),
        "{:?}",
        paused.reason
    );
    assert_eq!(paused.audit.rounds, 2);
    assert_eq!(
        paused.audit.result.as_ref().unwrap().verdict,
        "changes_requested"
    );
    assert_eq!(paused.turns, 2, "one repair turn ran between the audits");
    let executor_turns: Vec<ProviderRequest> = provider
        .requests()
        .into_iter()
        .filter(|r| !is_auditor(r) && user_text(r).starts_with(goals::PROMPT_PREFIX))
        .collect();
    let second = executor_turns
        .iter()
        .find(|r| user_text(r).contains("did not approve"))
        .expect("the repair turn carried the findings");
    assert!(user_text(second).contains("criterion #1: not met — round 1: not convinced"));
    assert_eq!(
        subagents(&mut e, &s).children.len(),
        2,
        "one auditor child per round"
    );
    // Skipping completes it, labelled unreviewed; resume is then refused.
    let skipped = control(&mut e, &s, GoalAction::SkipAudit).unwrap().unwrap();
    assert_eq!(skipped.state, GoalState::Completed);
    assert_eq!(skipped.reason.as_deref(), Some(goals::REASON_AUDIT_SKIPPED));
    assert_eq!(skipped.audit.result.as_ref().unwrap().verdict, "skipped");
    assert!(control(&mut e, &s, GoalAction::SkipAudit).is_err());
}

#[test]
fn an_inconclusive_or_malformed_verdict_pauses_and_never_approves() {
    for reply in ["looks fine to me", "{\"verdict\": \"maybe\"}"] {
        let fx = Fixture::new();
        std::fs::write(fx.project.join("a.txt"), "done").unwrap();
        let hash = crate::web::sha256_hex(b"done");
        let text = reply.to_owned();
        let provider = Scripted::new(audit_script(hash, move |_| R::Text(text.clone())));
        let mut e = fx.start(&provider);
        let s = e.create(&fx.project, "c");
        start_audited_goal(&fx, &mut e, &s);
        let paused = until_state(&mut e, &s, GoalState::Paused);
        assert!(
            paused.reason.as_deref().unwrap().contains("inconclusive"),
            "{reply}: {:?}",
            paused.reason
        );
        assert_eq!(
            paused.audit.result.as_ref().unwrap().verdict,
            "inconclusive"
        );
        assert!(
            paused
                .audit
                .result
                .as_ref()
                .unwrap()
                .summary
                .as_deref()
                .unwrap()
                .contains("no verdict object")
        );
    }
}

#[test]
fn a_workspace_change_during_the_audit_makes_approval_stale() {
    let fx = Fixture::new();
    std::fs::write(fx.project.join("a.txt"), "done").unwrap();
    let hash = crate::web::sha256_hex(b"done");
    let gate = Gate::new();
    let g = gate.clone();
    let provider = Scripted::new(audit_script(hash, move |_| {
        R::Gated(g.clone(), Box::new(verdict("approved", true, "fine")))
    }));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    start_audited_goal(&fx, &mut e, &s);
    until_state(&mut e, &s, GoalState::Verifying);
    e.until("the auditor is thinking", |_| {
        provider.requests().iter().any(is_auditor)
    });
    std::fs::write(fx.project.join("a.txt"), "changed meanwhile").unwrap();
    gate.open();
    let paused = until_state(&mut e, &s, GoalState::Paused);
    assert_eq!(paused.reason.as_deref(), Some(goals::REASON_AUDIT_STALE));
    assert_eq!(paused.audit.result.as_ref().unwrap().verdict, "stale");
}

#[test]
fn stop_and_restart_end_an_audit_without_approval() {
    let fx = Fixture::new();
    std::fs::write(fx.project.join("a.txt"), "done").unwrap();
    let hash = crate::web::sha256_hex(b"done");
    let gate = Gate::new();
    let g = gate.clone();
    let provider = Scripted::new(audit_script(hash.clone(), move |_| {
        R::Gated(g.clone(), Box::new(verdict("approved", true, "fine")))
    }));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    start_audited_goal(&fx, &mut e, &s);
    until_state(&mut e, &s, GoalState::Verifying);
    e.until("the auditor is thinking", |_| {
        provider.requests().iter().any(is_auditor)
    });
    e.call(Call::Stop { session: s.clone() }).unwrap();
    let paused = until_state(&mut e, &s, GoalState::Paused);
    assert_eq!(paused.reason.as_deref(), Some(goals::REASON_AUDIT_STOPPED));
    assert_eq!(paused.audit.result.as_ref().unwrap().verdict, "stopped");
    gate.open();
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(
        goal_of(&mut e, &s).unwrap().state,
        GoalState::Paused,
        "a late approval changes nothing"
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while subagents(&mut e, &s).children[0].status != "aborted" {
        assert!(
            std::time::Instant::now() < deadline,
            "the auditor child was not stopped"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        control(&mut e, &s, GoalAction::SkipAudit).is_err(),
        "a stopped audit is not skippable"
    );

    // Restart during an audit: paused with the restart reason, the pending audit is
    // stopped on resume, and a fresh turn starts.
    let fx = Fixture::new();
    std::fs::write(fx.project.join("a.txt"), "done").unwrap();
    let s;
    {
        let gate = Gate::new();
        let g = gate.clone();
        let provider = Scripted::new(audit_script(hash.clone(), move |_| {
            R::Gated(g.clone(), Box::new(verdict("approved", true, "fine")))
        }));
        let mut e = fx.start(&provider);
        s = e.create(&fx.project, "c");
        start_audited_goal(&fx, &mut e, &s);
        until_state(&mut e, &s, GoalState::Verifying);
        e.until("the auditor is thinking", |_| {
            provider.requests().iter().any(is_auditor)
        });
        e.handle.shutdown(Duration::from_millis(200));
    }
    let provider = Scripted::new(audit_script(hash, |_| verdict("approved", true, "fine")));
    let mut e = fx.start(&provider);
    let paused = goal_of(&mut e, &s).unwrap();
    assert_eq!(paused.state, GoalState::Paused);
    assert_eq!(paused.reason.as_deref(), Some(goals::REASON_RESTART));
    assert_eq!(paused.audit.result.as_ref().unwrap().verdict, "pending");
    control(&mut e, &s, GoalAction::Resume).unwrap();
    let resumed = goal_of(&mut e, &s).unwrap();
    assert_ne!(resumed.audit.result.as_ref().unwrap().verdict, "pending");
    // The resumed goal proposes again and this time the audit approves.
    let done = until_state(&mut e, &s, GoalState::Completed);
    assert_eq!(done.audit.rounds, 2);
    assert!(
        done.reason
            .as_deref()
            .unwrap()
            .starts_with("Audit approved (round 2)")
    );
}

#[test]
fn a_pinned_auditor_model_replaces_the_profiles_choice_and_an_unknown_one_pauses() {
    // `goals.auditorModel` names the model every audit runs on; the executor keeps its own.
    let fx = Fixture::new();
    std::fs::write(fx.project.join("a.txt"), "done").unwrap();
    let hash = crate::web::sha256_hex(b"done");
    let provider = Scripted::new(audit_script(hash.clone(), |_| {
        verdict("approved", true, "a.txt says done")
    }));
    let mut config = fx.config();
    let mut reviewer = crate::provider::scripted::model();
    reviewer.id = "reviewer".into();
    config.extra_models.push(reviewer);
    std::fs::create_dir_all(fx.dir.path().join("config")).unwrap();
    std::fs::write(
        fx.dir.path().join("config/settings.json"),
        json!({"goals": {"auditorModel": "scripted/reviewer"}}).to_string(),
    )
    .unwrap();
    let mut e = fx.start_config(&provider, config.clone());
    let s = e.create(&fx.project, "c");
    start_audited_goal(&fx, &mut e, &s);
    let done = until_state(&mut e, &s, GoalState::Completed);
    assert_eq!(done.audit.result.as_ref().unwrap().verdict, "approved");
    assert!(
        done.usage_cost_usd >= 0.0,
        "cost is accounted (zero for an unpriced model)"
    );
    let requests = provider.requests();
    let auditor = requests
        .iter()
        .find(|r| is_auditor(r))
        .expect("the auditor ran");
    assert_eq!(auditor.model.id, "reviewer");
    let executor = requests
        .iter()
        .find(|r| !is_auditor(r))
        .expect("the executor ran");
    assert_eq!(executor.model.id, "scripted");
    e.handle.shutdown(Duration::from_secs(1));
    drop(e);

    // A pinned model that is not in the catalog never silently falls back: the audit cannot
    // start and the goal pauses for review with the reason.
    std::fs::write(
        fx.dir.path().join("config/settings.json"),
        json!({"goals": {"auditorModel": "scripted/missing"}}).to_string(),
    )
    .unwrap();
    let provider = Scripted::new(audit_script(hash, |_| {
        verdict("approved", true, "a.txt says done")
    }));
    let mut e = fx.start_config(&provider, config);
    let s = e.create(&fx.project, "c2");
    start_audited_goal(&fx, &mut e, &s);
    let paused = until_state(&mut e, &s, GoalState::Paused);
    assert!(
        paused
            .reason
            .as_deref()
            .unwrap()
            .contains("scripted/missing"),
        "{:?}",
        paused.reason
    );
    assert!(provider.requests().iter().all(|r| !is_auditor(r)));
}
