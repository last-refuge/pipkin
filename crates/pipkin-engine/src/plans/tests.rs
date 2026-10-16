//! Plan domain rules: atomic batches, stable ids, references, dependencies, reasons,
//! staleness, archival, bounds and rendering.

use pipkin_engine_protocol::limits::{MAX_PLAN_OPS, MAX_PLAN_TASKS, MAX_TASK_TITLE_CHARS};
use pipkin_engine_protocol::{ErrorCode, PlanOp, TaskEvidence, TaskRef, TaskStatus};
use serde_json::json;

use super::{PlanState, apply, counts_line, line, summary, view};

fn op(value: serde_json::Value) -> PlanOp {
    serde_json::from_value(value).unwrap()
}

fn ops(values: &[serde_json::Value]) -> Vec<PlanOp> {
    values.iter().cloned().map(op).collect()
}

fn applied(state: &PlanState, batch: &[serde_json::Value]) -> PlanState {
    apply(state, &ops(batch), None, 1_000)
        .unwrap_or_else(|e| panic!("{e:?}"))
        .state
}

fn rejected(
    state: &PlanState,
    batch: &[serde_json::Value],
) -> pipkin_engine_protocol::PlanRejected {
    apply(state, &ops(batch), None, 1_000).expect_err("rejected")
}

fn task(state: &PlanState, id: u64) -> &pipkin_engine_protocol::PlanTaskView {
    state.tasks.iter().find(|t| t.id == id).unwrap()
}

/// Three tasks: #1, #2 after #1, #3 after #2.
fn chain() -> PlanState {
    applied(
        &PlanState::default(),
        &[
            json!({"op": "create", "ref": "a", "title": "Write migration"}),
            json!({"op": "create", "ref": "b", "title": "Add tests", "dependsOn": ["a"]}),
            json!({"op": "create", "title": "Update docs", "dependsOn": ["b"], "criteria": "docs mention schema 11"}),
        ],
    )
}

fn finished_plan() -> PlanState {
    applied(
        &chain(),
        &[
            json!({"op":"update", "task":1, "status":"completed", "add_evidence":[{"kind":"test", "reference":"retained evidence"}]}),
            json!({"op":"update", "task":2, "status":"completed"}),
            json!({"op":"update", "task":3, "status":"completed"}),
        ],
    )
}

#[test]
fn new_independent_work_archives_finished_plan_atomically_without_reusing_ids() {
    let old = finished_plan();
    let snapshot = old.clone();
    let operations = ops(&[
        json!({"op":"create", "ref":"new", "title":"New work", "status":"active"}),
        json!({"op":"create", "ref":"verify", "title":"Verify", "dependsOn":["new"]}),
        json!({"op":"update", "task":"verify", "detail":"New plan details"}),
    ]);
    let result = apply(&old, &operations, Some(old.revision), 2_000).unwrap();
    assert_eq!(old, snapshot);
    assert_eq!(result.state.next_id, 6);
    assert_eq!(result.receipt.changed, [1, 2, 3, 4, 5]);
    assert_eq!(
        result
            .events
            .iter()
            .filter(|e| e.kind == "archived")
            .count(),
        3
    );
    let current = view(&result.state, false);
    assert_eq!(
        current.tasks.iter().map(|t| t.id).collect::<Vec<_>>(),
        [4, 5]
    );
    assert_eq!(current.counts.completed, 0);
    assert_eq!(current.counts.archived, 3);
    assert_eq!(task(&result.state, 1).evidence, task(&old, 1).evidence);
    assert_eq!(view(&result.state, true).tasks.len(), 5);
    assert!(
        apply(
            &old,
            &ops(&[json!({"op":"create", "title":""})]),
            None,
            2_000
        )
        .is_err()
    );
    assert_eq!(
        old, snapshot,
        "a rejected new plan cannot retire the old one"
    );
    assert!(apply(&old, &operations, Some(old.revision - 1), 2_000).is_err());
}

#[test]
fn new_tasks_do_not_archive_unfinished_work_or_explicit_continuations() {
    let continuing = applied(&chain(), &[json!({"op":"create", "title":"More work"})]);
    assert!(continuing.tasks.iter().all(|t| !t.archived));
    let old = finished_plan();
    for op in [
        json!({"op":"create", "title":"Dependent continuation", "dependsOn":[1]}),
        json!({"op":"create", "title":"Child continuation", "parent":1}),
    ] {
        let continued = applied(&old, &[op]);
        assert!(continued.tasks.iter().all(|t| !t.archived));
    }
    let reopened = applied(
        &old,
        &[
            json!({"op":"update", "task":3, "status":"pending", "reason":"more work"}),
            json!({"op":"create", "title":"Additional task"}),
        ],
    );
    assert!(reopened.tasks.iter().all(|t| !t.archived));
}

#[test]
fn creation_allocates_stable_ids_and_resolves_refs_within_the_batch() {
    let state = chain();
    assert_eq!(state.revision, 1);
    assert_eq!(state.next_id, 4);
    let ids: Vec<u64> = state.tasks.iter().map(|t| t.id).collect();
    assert_eq!(ids, [1, 2, 3]);
    assert_eq!(task(&state, 2).depends_on, [1]);
    assert_eq!(task(&state, 3).depends_on, [2]);
    assert_eq!(task(&state, 3).criteria, "docs mention schema 11");
    let result = apply(
        &PlanState::default(),
        &ops(&[json!({"op": "create", "ref": "x", "title": "t"})]),
        None,
        5,
    )
    .unwrap();
    assert_eq!(result.receipt.created.get("x"), Some(&1));
    assert_eq!(result.receipt.changed, [1]);
    assert_eq!(result.receipt.revision, 1);
    assert_eq!(result.events.len(), 1);
    assert_eq!(result.events[0].kind, "created");
}

#[test]
fn an_unknown_ref_or_id_rejects_the_whole_batch_and_changes_nothing() {
    let state = chain();
    let r = rejected(
        &state,
        &[
            json!({"op": "update", "task": 1, "status": "active"}),
            json!({"op": "create", "title": "x", "dependsOn": ["nope"]}),
        ],
    );
    assert_eq!(r.op, Some(1));
    assert_eq!(r.code, ErrorCode::Invalid);
    assert!(r.message.contains("ref \"nope\""), "{}", r.message);
    assert_eq!(r.revision, 1);
    let r = rejected(&state, &[json!({"op": "update", "task": 42, "title": "x"})]);
    assert!(r.message.contains("#42 does not exist"));
}

#[test]
fn expected_revision_mismatch_is_a_conflict() {
    let state = chain();
    let r = apply(
        &state,
        &ops(&[json!({"op": "update", "task": 1, "status": "active"})]),
        Some(0),
        1,
    )
    .unwrap_err();
    assert_eq!(r.code, ErrorCode::Conflict);
    assert_eq!(r.op, None);
    assert!(r.message.contains("revision 1, not 0"));
    apply(
        &state,
        &ops(&[json!({"op": "update", "task": 1, "status": "active"})]),
        Some(1),
        1,
    )
    .unwrap();
}

#[test]
fn dependencies_cannot_be_self_missing_or_cyclic() {
    let state = chain();
    let r = rejected(
        &state,
        &[json!({"op": "update", "task": 2, "dependsOn": [2]})],
    );
    assert!(r.message.contains("cannot depend on itself"));
    let r = rejected(
        &state,
        &[json!({"op": "update", "task": 1, "dependsOn": [3]})],
    );
    assert!(r.message.contains("cycle"), "{}", r.message);
    assert_eq!(r.op, None, "cycles are a whole-plan check");
    let r = rejected(
        &state,
        &[json!({"op": "create", "title": "x", "dependsOn": [99]})],
    );
    assert!(r.message.contains("#99 does not exist"));
}

#[test]
fn a_task_is_active_or_completed_only_when_its_dependencies_are_satisfied() {
    let state = chain();
    let r = rejected(
        &state,
        &[json!({"op": "update", "task": 2, "status": "active"})],
    );
    assert!(
        r.message
            .contains("#2 cannot be active while it depends on #1 (pending)"),
        "{}",
        r.message
    );
    // Blocked is always allowed; several tasks may be active at once.
    let state = applied(
        &state,
        &[
            json!({"op": "update", "task": 2, "status": "blocked"}),
            json!({"op": "update", "task": 1, "status": "active"}),
            json!({"op": "create", "title": "Parallel", "status": "active"}),
        ],
    );
    assert_eq!(task(&state, 2).status, TaskStatus::Blocked);
    assert_eq!(view(&state, false).counts.active, 2);
    // Completing #1 unblocks #2; skipping #2 (waived) satisfies #3; cancelling does not.
    let state = applied(
        &state,
        &[
            json!({"op": "update", "task": 1, "status": "completed", "addEvidence": [{"kind": "test", "reference": "cargo test: exit 0"}]}),
            json!({"op": "update", "task": 2, "status": "skipped", "reason": "covered elsewhere"}),
            json!({"op": "update", "task": 3, "status": "active"}),
        ],
    );
    assert_eq!(task(&state, 1).evidence.len(), 1);
    assert!(task(&state, 1).completed_at_ms.is_some());
    let r = rejected(
        &state,
        &[
            json!({"op": "update", "task": 2, "status": "cancelled", "reason": "dropped"}),
            json!({"op": "update", "task": 3, "status": "completed"}),
        ],
    );
    assert!(
        r.message
            .contains("#3 cannot be completed while it depends on #2 (cancelled)"),
        "{}",
        r.message
    );
}

#[test]
fn skipping_cancelling_and_reopening_need_a_reason() {
    let state = chain();
    let r = rejected(
        &state,
        &[json!({"op": "update", "task": 1, "status": "skipped"})],
    );
    assert!(r.message.contains("reason is required"));
    let r = rejected(
        &state,
        &[json!({"op": "update", "task": 1, "status": "cancelled"})],
    );
    assert!(r.message.contains("reason is required"));
    let state = applied(
        &state,
        &[json!({"op": "update", "task": 1, "status": "completed"})],
    );
    let r = rejected(
        &state,
        &[json!({"op": "update", "task": 1, "status": "pending"})],
    );
    assert!(
        r.message.contains("from completed to pending"),
        "{}",
        r.message
    );
    let state = applied(
        &state,
        &[json!({"op": "update", "task": 1, "status": "pending", "reason": "tests failed"})],
    );
    assert_eq!(task(&state, 1).status, TaskStatus::Pending);
    assert_eq!(task(&state, 1).reason.as_deref(), Some("tests failed"));
    assert_eq!(task(&state, 1).completed_at_ms, None);
}

#[test]
fn reopening_marks_completed_dependents_stale_and_recompletion_clears_it() {
    let state = applied(
        &chain(),
        &[
            json!({"op": "update", "task": 1, "status": "completed"}),
            json!({"op": "update", "task": 2, "status": "completed"}),
        ],
    );
    let result = apply(
        &state,
        &ops(&[json!({"op": "update", "task": 1, "status": "active", "reason": "found a bug"})]),
        None,
        2,
    )
    .unwrap();
    let state = result.state;
    assert!(task(&state, 2).stale);
    assert_eq!(
        task(&state, 2).status,
        TaskStatus::Completed,
        "the claim stands, flagged"
    );
    assert!(
        !task(&state, 3).stale,
        "pending work has no claim to invalidate"
    );
    assert!(result.receipt.changed.contains(&2));
    let kinds: Vec<&str> = result.events.iter().map(|e| e.kind).collect();
    assert_eq!(kinds, ["reopened", "stale"]);
    let prior = &result.events[0].data["prior"];
    assert_eq!(prior["status"], "completed");
    assert!(prior["completedAtMs"].is_number());
    // Completing #2 again (after #1 is done again) clears the flag.
    let state = applied(
        &state,
        &[
            json!({"op": "update", "task": 1, "status": "completed"}),
            json!({"op": "update", "task": 2, "status": "pending", "reason": "re-check"}),
            json!({"op": "update", "task": 2, "status": "completed"}),
        ],
    );
    assert!(!task(&state, 2).stale);
    // Revising the criteria of completed work also makes the completion stale.
    let state = applied(
        &state,
        &[json!({"op": "update", "task": 2, "criteria": "also covers rollback"})],
    );
    assert!(task(&state, 2).stale);
}

#[test]
fn archival_is_separate_from_status_and_archived_tasks_are_read_only() {
    let state = chain();
    let r = rejected(&state, &[json!({"op": "archive", "task": 1})]);
    assert!(
        r.message
            .contains("cannot be archived until it is finished")
    );
    let state = applied(
        &state,
        &[
            json!({"op": "update", "task": 1, "status": "completed"}),
            json!({"op": "archive", "task": 1}),
        ],
    );
    assert!(task(&state, 1).archived);
    assert_eq!(view(&state, false).tasks.len(), 2);
    assert_eq!(view(&state, true).tasks.len(), 3);
    assert_eq!(view(&state, false).counts.archived, 1);
    let r = rejected(&state, &[json!({"op": "update", "task": 1, "title": "x"})]);
    assert!(r.message.contains("#1 is archived"));
    let r = rejected(&state, &[json!({"op": "reorder", "task": 1})]);
    assert!(r.message.contains("#1 is archived"));
    // An archived dependency still satisfies its dependents; ids are never reused.
    let state = applied(
        &state,
        &[
            json!({"op": "update", "task": 2, "status": "active"}),
            json!({"op": "create", "title": "Fresh"}),
        ],
    );
    assert_eq!(state.tasks.last().unwrap().id, 4);
    let state = applied(
        &state,
        &[
            json!({"op": "update", "task": 2, "status": "completed"}),
            json!({"op": "update", "task": 4, "status": "cancelled", "reason": "not needed"}),
            json!({"op": "archiveFinished"}),
        ],
    );
    let archived: Vec<u64> = state
        .tasks
        .iter()
        .filter(|t| t.archived)
        .map(|t| t.id)
        .collect();
    assert_eq!(archived, [1, 2, 4]);
}

#[test]
fn reorder_moves_among_live_tasks() {
    let state = chain();
    let state = applied(&state, &[json!({"op": "reorder", "task": 3})]);
    let order: Vec<u64> = state.tasks.iter().map(|t| t.id).collect();
    assert_eq!(order, [3, 1, 2]);
    let state = applied(&state, &[json!({"op": "reorder", "task": 3, "after": 2})]);
    let order: Vec<u64> = state.tasks.iter().map(|t| t.id).collect();
    assert_eq!(order, [1, 2, 3]);
    let r = rejected(&state, &[json!({"op": "reorder", "task": 2, "after": 2})]);
    assert!(r.message.contains("after itself"));
    let r = rejected(&state, &[json!({"op": "reorder", "task": 2, "after": 77})]);
    assert!(r.message.contains("#77 does not exist"));
    assert_eq!(
        state.tasks.iter().map(|t| t.id).collect::<Vec<_>>(),
        [1, 2, 3],
        "a rejected reorder restores the order"
    );
}

#[test]
fn parents_must_exist_and_not_form_a_cycle() {
    let state = applied(
        &PlanState::default(),
        &[
            json!({"op": "create", "ref": "p", "title": "Parent"}),
            json!({"op": "create", "title": "Child", "parent": "p"}),
        ],
    );
    assert_eq!(task(&state, 2).parent, Some(1));
    let r = rejected(&state, &[json!({"op": "update", "task": 1, "parent": 2})]);
    assert!(r.message.contains("cycle"), "{}", r.message);
    let r = rejected(&state, &[json!({"op": "update", "task": 1, "parent": 1})]);
    assert!(r.message.contains("own parent"));
}

#[test]
fn bounds_are_enforced() {
    let r = rejected(&PlanState::default(), &[]);
    assert!(r.message.contains("no operations"));
    let many: Vec<serde_json::Value> = (0..=MAX_PLAN_OPS)
        .map(|i| json!({"op": "create", "title": format!("t{i}")}))
        .collect();
    let r = rejected(&PlanState::default(), &many);
    assert!(r.message.contains("at most 100 operations"));
    let r = rejected(
        &PlanState::default(),
        &[json!({"op": "create", "title": "   "})],
    );
    assert!(r.message.contains("title must not be empty"));
    let r = rejected(
        &PlanState::default(),
        &[json!({"op": "create", "title": "t".repeat(MAX_TASK_TITLE_CHARS + 1)})],
    );
    assert!(r.message.contains("at most 200 characters"));
    let r = rejected(
        &PlanState::default(),
        &[json!({"op": "create", "title": "t", "status": "completed"})],
    );
    assert!(r.message.contains("cannot be created completed"));
    let r = rejected(
        &PlanState::default(),
        &[
            json!({"op": "create", "title": "t"}),
            json!({"op": "update", "task": 1, "addEvidence": [{"kind": "", "reference": "x"}]}),
        ],
    );
    assert!(r.message.contains("needs a kind and a reference"));
    // The live-task bound counts after the batch; archived tasks do not count.
    let mut state = PlanState::default();
    for chunk in (0..MAX_PLAN_TASKS as u64)
        .collect::<Vec<_>>()
        .chunks(MAX_PLAN_OPS)
    {
        let batch: Vec<_> = chunk
            .iter()
            .map(|i| json!({"op": "create", "title": format!("t{i}")}))
            .collect();
        state = applied(&state, &batch);
    }
    let r = rejected(&state, &[json!({"op": "create", "title": "one too many"})]);
    assert!(r.message.contains("at most 500 live tasks"));
    let state = applied(
        &state,
        &[
            json!({"op": "update", "task": 1, "status": "completed"}),
            json!({"op": "archive", "task": 1}),
            json!({"op": "create", "title": "fits now"}),
        ],
    );
    assert_eq!(
        state.tasks.iter().filter(|t| !t.archived).count(),
        MAX_PLAN_TASKS
    );
}

#[test]
fn evidence_is_typed_and_bounded() {
    let state = applied(
        &PlanState::default(),
        &[json!({"op": "create", "title": "t"})],
    );
    let state = applied(
        &state,
        &[json!({"op": "update", "task": 1, "addEvidence": [
            {"kind": "toolCall", "reference": "call_1"},
            {"kind": "command", "reference": "cargo test (exit 0)", "note": "882 passed"}
        ]})],
    );
    assert_eq!(
        task(&state, 1).evidence[1],
        TaskEvidence {
            kind: "command".into(),
            reference: "cargo test (exit 0)".into(),
            note: Some("882 passed".into())
        }
    );
    let state = applied(
        &state,
        &[json!({"op": "update", "task": 1, "clearEvidence": true})],
    );
    assert!(task(&state, 1).evidence.is_empty());
    let too_many: Vec<_> = (0..21)
        .map(|i| json!({"kind": "k", "reference": format!("r{i}")}))
        .collect();
    let r = rejected(
        &state,
        &[json!({"op": "update", "task": 1, "addEvidence": too_many})],
    );
    assert!(r.message.contains("at most 20 evidence"));
}

#[test]
fn refs_round_trip_as_ids_or_names() {
    assert_eq!(
        serde_json::from_value::<TaskRef>(json!(3)).unwrap(),
        TaskRef::Id(3)
    );
    assert_eq!(
        serde_json::from_value::<TaskRef>(json!("a")).unwrap(),
        TaskRef::New("a".into())
    );
    let unit: PlanOp = serde_json::from_value(json!({"op": "archiveFinished"})).unwrap();
    assert_eq!(unit, PlanOp::ArchiveFinished);
}

#[test]
fn rendering_is_compact_and_counts_are_task_counts() {
    let state = applied(
        &chain(),
        &[
            json!({"op": "update", "task": 1, "status": "completed"}),
            json!({"op": "update", "task": 2, "status": "active", "owner": "subagent:1"}),
        ],
    );
    let v = view(&state, false);
    assert_eq!(
        line(&v.tasks[1]),
        "#2 [active] Add tests (after #1) [subagent:1]"
    );
    assert_eq!(
        counts_line(&v.counts),
        "2 open (1 active, 0 blocked, 1 pending) · 1 completed, 0 skipped, 0 cancelled"
    );
    let s = summary(&v).unwrap();
    assert!(s.starts_with("Current plan (revision 2; 2 open"), "{s}");
    assert!(s.contains("- #2 [active] Add tests (after #1) [subagent:1]"));
    assert!(s.contains("- #3 [pending] Update docs (after #2)"));
    assert!(
        !s.contains("#1 [completed]"),
        "finished work is counted, not listed"
    );
    assert_eq!(summary(&view(&PlanState::default(), false)), None);
    let state = applied(
        &state,
        &[
            json!({"op": "update", "task": 2, "status": "completed"}),
            json!({"op": "update", "task": 1, "status": "pending", "reason": "redo"}),
        ],
    );
    assert!(
        summary(&view(&state, false))
            .unwrap()
            .contains("1 completed task(s) are stale")
    );
}
