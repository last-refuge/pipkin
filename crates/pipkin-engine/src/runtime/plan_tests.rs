//! Plans through the client contract: the model's `plan_update`/`plan_read`, the client's
//! `UpdatePlan`/`Plan`, the published `PlanChanged`, the prompt's `plan` section, idempotent
//! receipts, conflicts, restart persistence, removal and children's separate plans.

use std::time::Duration;

use pipkin_engine_protocol::*;
use serde_json::json;

use super::tests::{Engine, Fixture};
use crate::provider::ProviderRequest;
use crate::provider::scripted::{Reply as R, Scripted};
use crate::store::Store;

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

/// After a user message, call `tool` with `args`; then answer with the tool result's text.
fn calling(
    tool: &'static str,
    args: serde_json::Value,
) -> impl Fn(&ProviderRequest, usize) -> R + Send + Sync {
    move |req, _| {
        if last_is_user(req) {
            R::Tools(None, vec![(tool.into(), args.clone())])
        } else {
            R::Text(format!(
                "model saw: {}",
                last_tool_result(req).unwrap_or_default()
            ))
        }
    }
}

fn tool_results(
    snap: &SessionSnapshot,
    tool: &str,
) -> Vec<(bool, String, Option<serde_json::Value>)> {
    snap.entries
        .iter()
        .filter_map(|e| match &e.kind {
            EntryKind::Message {
                message:
                    m @ Message::ToolResult {
                        tool_name,
                        is_error,
                        extra,
                        ..
                    },
                ..
            } if tool_name == tool => Some((*is_error, m.text(), extra.get("details").cloned())),
            _ => None,
        })
        .collect()
}

fn plan_of(e: &mut Engine, s: &str) -> PlanView {
    match e.call(Call::Plan {
        session: s.into(),
        include_archived: false,
    }) {
        Ok(Reply::Plan(p)) => p,
        other => panic!("{other:?}"),
    }
}

fn update(
    e: &mut Engine,
    s: &str,
    key: &str,
    ops: serde_json::Value,
    expected: Option<u64>,
) -> Result<PlanReceipt, ProtoError> {
    match e.call(Call::UpdatePlan {
        session: s.into(),
        request: key.into(),
        ops: serde_json::from_value(ops).unwrap(),
        expected_revision: expected,
    }) {
        Ok(Reply::PlanUpdated(r)) => Ok(r),
        Ok(other) => panic!("{other:?}"),
        Err(e) => Err(e),
    }
}

fn plan_changed(events: &[Event], s: &str) -> Vec<PlanView> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Session {
                session,
                change: SessionChange::PlanChanged { plan },
                ..
            } if session == s => Some(plan.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_model_creates_tasks_and_the_next_request_carries_the_plan_section() {
    let fx = Fixture::new();
    let provider = Scripted::new(calling(
        "plan_update",
        json!({"ops": [
            {"op": "create", "ref": "m", "title": "Write migration", "criteria": "schema 11 applies"},
            {"op": "create", "title": "Add tests", "dependsOn": ["m"]},
            {"op": "update", "task": "m", "status": "active"}
        ]}),
    ));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "plan it").unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let snap = e.snapshot(&s);
    let results = tool_results(&snap, "plan_update");
    assert_eq!(results.len(), 1);
    assert!(!results[0].0, "{}", results[0].1);
    assert!(
        results[0]
            .1
            .starts_with("Plan revision 1. 2 open (1 active, 0 blocked, 1 pending)"),
        "{}",
        results[0].1
    );
    assert!(
        results[0].1.contains("Created: #1 (m), #2"),
        "{}",
        results[0].1
    );
    assert!(
        results[0].1.contains("#1 [active] Write migration"),
        "{}",
        results[0].1
    );
    assert!(
        results[0].1.contains("#2 [pending] Add tests (after #1)"),
        "{}",
        results[0].1
    );
    let details = results[0].2.as_ref().unwrap();
    assert_eq!(details["plan"]["version"], 1);
    assert_eq!(details["plan"]["outcome"], "applied");
    assert_eq!(details["plan"]["created"]["m"], 1);
    assert_eq!(details["plan"]["changed"], json!([1, 2]));
    // The snapshot and the published change carry the plan.
    assert_eq!(snap.plan.revision, 1);
    assert_eq!(snap.plan.tasks.len(), 2);
    assert_eq!(snap.plan.tasks[0].status, TaskStatus::Active);
    let changes = plan_changed(&e.events, &s);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0], snap.plan);
    // The request after the tool round carries the summary as a system section; the
    // first request did not.
    let captured = provider.requests();
    let reqs: Vec<_> = captured.iter().map(|r| r.resolve_systems(false)).collect();
    assert_eq!(reqs.len(), 2);
    assert!(!reqs[0].system.contains("Current plan"));
    assert!(
        reqs[1].system.contains("Current plan (revision 1; 2 open"),
        "{}",
        reqs[1].system
    );
    assert!(reqs[1].system.contains("- #1 [active] Write migration"));
    assert!(
        reqs[1]
            .system
            .contains("- #2 [pending] Add tests (after #1)")
    );
    // The tools are offered, after the others, in a stable order.
    let names: Vec<&str> = reqs[0].tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "read",
            "write",
            "edit",
            "bash",
            "subagent",
            "ask_user_question",
            "get_goal",
            "update_goal",
            "propose_goal_completion",
            "plan_update",
            "plan_read",
            "web_search",
            "web_fetch",
            "web_read"
        ]
    );
}

#[test]
fn a_rejected_batch_is_a_tool_error_that_changes_nothing() {
    let fx = Fixture::new();
    let provider = Scripted::new(calling(
        "plan_update",
        json!({"ops": [
            {"op": "create", "title": "ok"},
            {"op": "update", "task": 1, "status": "skipped"}
        ]}),
    ));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "go").unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let snap = e.snapshot(&s);
    let results = tool_results(&snap, "plan_update");
    assert!(results[0].0);
    assert!(
        results[0]
            .1
            .contains("Plan update rejected (op 1): a reason is required to mark task #1 skipped"),
        "{}",
        results[0].1
    );
    let details = results[0].2.as_ref().unwrap();
    assert_eq!(details["plan"]["outcome"], "rejected");
    assert_eq!(details["plan"]["op"], 1);
    assert_eq!(details["plan"]["revision"], 0);
    assert_eq!(snap.plan, PlanView::default());
    assert!(plan_changed(&e.events, &s).is_empty());
    assert!(
        !provider.requests()[1]
            .resolve_systems(false)
            .system
            .contains("Current plan")
    );
}

#[test]
fn plan_read_lists_tasks_or_one_task_in_full() {
    let fx = Fixture::new();
    let provider = Scripted::new(|req: &ProviderRequest, n| {
        if last_is_user(req) {
            R::Tools(
                None,
                vec![(
                    "plan_read".into(),
                    if n == 0 {
                        json!({})
                    } else {
                        json!({"task": 1})
                    },
                )],
            )
        } else {
            R::Text("ok".into())
        }
    });
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    update(
        &mut e,
        &s,
        "k1",
        json!([
            {"op": "create", "title": "Investigate", "detail": "look at the logs", "criteria": "root cause named"},
            {"op": "create", "title": "Fix", "dependsOn": [1]},
            {"op": "update", "task": 1, "status": "completed", "addEvidence": [{"kind": "toolCall", "reference": "call_9", "note": "log excerpt"}]}
        ]),
        None,
    )
    .unwrap();
    e.prompt(&s, "r1", "read").unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    e.prompt(&s, "r2", "read one").unwrap();
    assert_eq!(e.settled("r2"), RunOutcome::Completed);
    let snap = e.snapshot(&s);
    let results = tool_results(&snap, "plan_read");
    assert_eq!(results.len(), 2);
    assert!(
        results[0]
            .1
            .starts_with("Plan revision 1. 1 open (0 active, 0 blocked, 1 pending) · 1 completed"),
        "{}",
        results[0].1
    );
    assert!(
        results[0]
            .1
            .contains("#1 [completed] Investigate\n#2 [pending] Fix (after #1)"),
        "{}",
        results[0].1
    );
    assert!(
        results[1].1.contains("Detail: look at the logs"),
        "{}",
        results[1].1
    );
    assert!(results[1].1.contains("Criteria: root cause named"));
    assert!(results[1].1.contains("- toolCall: call_9 — log excerpt"));
    assert_eq!(results[1].2.as_ref().unwrap()["plan"]["outcome"], "read");
}

#[test]
fn client_updates_are_idempotent_by_key_and_conflict_on_stale_revisions() {
    let fx = Fixture::new();
    let provider = Scripted::echo("x");
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    let first = update(
        &mut e,
        &s,
        "k1",
        json!([{"op": "create", "ref": "a", "title": "One"}]),
        Some(0),
    )
    .unwrap();
    assert_eq!(first.revision, 1);
    assert_eq!(first.created.get("a"), Some(&1));
    // Same key: the recorded receipt, no second task.
    let again = update(
        &mut e,
        &s,
        "k1",
        json!([{"op": "create", "ref": "a", "title": "One"}]),
        Some(0),
    )
    .unwrap();
    assert_eq!(again, first);
    assert_eq!(plan_of(&mut e, &s).tasks.len(), 1);
    assert_eq!(plan_changed(&e.events, &s).len(), 1, "one change published");
    // A stale expected revision is a conflict and changes nothing.
    let err = update(
        &mut e,
        &s,
        "k2",
        json!([{"op": "create", "title": "Two"}]),
        Some(0),
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict);
    assert!(err.message.contains("revision 1, not 0"));
    assert_eq!(plan_of(&mut e, &s).tasks.len(), 1);
    // The rejection is recorded under its key too: retrying the same key does not apply it
    // even once the revision would allow it.
    let err = update(
        &mut e,
        &s,
        "k2",
        json!([{"op": "create", "title": "Two"}]),
        None,
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict);
    // An invalid batch is `invalid`; an empty key is refused before anything is recorded.
    let err = update(
        &mut e,
        &s,
        "k3",
        json!([{"op": "update", "task": 9, "title": "x"}]),
        None,
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::Invalid);
    assert!(err.message.contains("#9 does not exist"));
    let err = update(
        &mut e,
        &s,
        "",
        json!([{"op": "create", "title": "x"}]),
        None,
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::Invalid);
    // A client key and a tool call id never collide.
    let store = e.handle.shared().store.clone();
    let recorded = store
        .read(|c| Store::plan_receipt(c, &s, "client:k1"))
        .unwrap();
    assert!(recorded.is_some_and(|r| r.is_ok()));
    assert!(
        store
            .read(|c| Store::plan_receipt(c, &s, "call:k1"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn the_store_replays_a_batch_under_its_key_without_applying_it_twice() {
    let fx = Fixture::new();
    let provider = Scripted::echo("x");
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    let store = e.handle.shared().store.clone();
    let ops: Vec<PlanOp> =
        serde_json::from_value(json!([{"op": "create", "title": "One"}])).unwrap();
    let (first, applied) = store
        .tx(|tx| Store::apply_plan(tx, &s, "call:c1", &ops, None, 1))
        .unwrap();
    assert!(applied);
    let (second, applied) = store
        .tx(|tx| Store::apply_plan(tx, &s, "call:c1", &ops, None, 2))
        .unwrap();
    assert!(!applied, "a replayed call returns its receipt");
    assert_eq!(first, second);
    let state = store.read(|c| Store::plan_state(c, &s)).unwrap();
    assert_eq!(state.tasks.len(), 1);
    assert_eq!(state.revision, 1);
    let events = store.read(|c| Store::plan_events(c, &s, 10)).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["kind"], "created");
    // The recovery registry knows both plan tools, so an interrupted plan_update reruns
    // (and returns its receipt) instead of being recorded as interrupted.
    assert!(e.handle.shared().recoverable_tool("plan_update").is_some());
    assert!(e.handle.shared().recoverable_tool("plan_read").is_some());
    assert!(e.handle.shared().recoverable_tool("bash").is_none());
}

#[test]
fn the_plan_survives_a_restart_and_compaction_does_not_touch_it() {
    let fx = Fixture::new();
    let s;
    {
        let provider = Scripted::echo("x");
        let mut e = fx.start(&provider);
        s = e.create(&fx.project, "c");
        update(
            &mut e,
            &s,
            "k1",
            json!([
                {"op": "create", "title": "A"},
                {"op": "create", "title": "B", "dependsOn": [1]},
                {"op": "update", "task": 1, "status": "completed"},
                {"op": "update", "task": 2, "status": "active"}
            ]),
            None,
        )
        .unwrap();
        e.prompt(&s, "r1", "hello").unwrap();
        assert_eq!(e.settled("r1"), RunOutcome::Completed);
        e.handle.shutdown(Duration::from_millis(200));
    }
    let provider = Scripted::echo("y");
    let mut e = fx.start(&provider);
    let snap = e.snapshot(&s);
    assert_eq!(snap.plan.revision, 1);
    let titles: Vec<(u64, TaskStatus)> = snap.plan.tasks.iter().map(|t| (t.id, t.status)).collect();
    assert_eq!(
        titles,
        [(1, TaskStatus::Completed), (2, TaskStatus::Active)]
    );
    // Ids continue after a restart; nothing is reused.
    let r = update(
        &mut e,
        &s,
        "k2",
        json!([{"op": "create", "title": "C"}]),
        Some(1),
    )
    .unwrap();
    assert_eq!(r.created.get("#0"), Some(&3));
    e.prompt(&s, "r2", "again").unwrap();
    assert_eq!(e.settled("r2"), RunOutcome::Completed);
    assert!(
        provider.requests()[0]
            .resolve_systems(false)
            .system
            .contains("Current plan (revision 2")
    );
}

#[test]
fn a_child_session_has_its_own_plan() {
    let fx = Fixture::new();
    let provider = Scripted::new(|req: &ProviderRequest, _| {
        let is_child = req
            .messages
            .iter()
            .any(|m| matches!(m, Message::User { .. }) && m.text() == "survey");
        if is_child {
            if last_is_user(req) {
                R::Tools(
                    None,
                    vec![(
                        "plan_update".into(),
                        json!({"ops": [{"op": "create", "title": "child task"}]}),
                    )],
                )
            } else {
                R::Text("child done".into())
            }
        } else if last_is_user(req) {
            R::Tools(None, vec![("subagent".into(), json!({"task": "survey"}))])
        } else {
            R::Text("parent done".into())
        }
    });
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "delegate").unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    assert!(
        e.snapshot(&s).plan.tasks.is_empty(),
        "the child's plan is not the parent's"
    );
    let children = match e.call(Call::Subagents { session: s.clone() }) {
        Ok(Reply::Subagents(v)) => v.children,
        other => panic!("{other:?}"),
    };
    assert_eq!(children.len(), 1);
    let child_plan = plan_of(&mut e, &children[0].session);
    assert_eq!(child_plan.tasks.len(), 1);
    assert_eq!(child_plan.tasks[0].title, "child task");
    let child_req = provider
        .requests()
        .into_iter()
        .find(|r| r.messages.iter().any(|m| m.text() == "survey"))
        .unwrap();
    assert!(
        child_req.tools.iter().any(|t| t.name == "plan_update"),
        "children plan their own work"
    );
    assert!(
        child_req
            .tools
            .iter()
            .all(|t| t.name != "ask_user_question")
    );
}

#[test]
fn removing_a_session_removes_its_plan_rows() {
    let fx = Fixture::new();
    let provider = Scripted::echo("x");
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    update(
        &mut e,
        &s,
        "k1",
        json!([{"op": "create", "title": "A"}]),
        None,
    )
    .unwrap();
    e.call(Call::RemoveSession { session: s.clone() }).unwrap();
    let store = e.handle.shared().store.clone();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let rows: i64 = store
            .read(|c| {
                Ok(c.query_row(
                    "SELECT (SELECT COUNT(*) FROM plan_tasks WHERE session = ?1) + (SELECT COUNT(*) FROM plans WHERE session = ?1) + (SELECT COUNT(*) FROM plan_receipts WHERE session = ?1) + (SELECT COUNT(*) FROM plan_events WHERE session = ?1)",
                    [&s],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        if rows == 0 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "plan rows remained: {rows}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        e.call(Call::Plan {
            session: s.clone(),
            include_archived: false
        })
        .is_err()
    );
}

#[test]
fn the_capability_is_advertised_with_the_current_schema() {
    let fx = Fixture::new();
    let provider = Scripted::echo("x");
    let mut e = fx.start(&provider);
    let welcome = match e.call(Call::Hello {
        protocol: PROTOCOL_VERSION,
        client: "t".into(),
    }) {
        Ok(Reply::Welcome(w)) => w,
        other => panic!("{other:?}"),
    };
    assert!(welcome.capabilities.iter().any(|c| c == "plans"));
    assert_eq!(welcome.schema, crate::store::SCHEMA_VERSION);
}
