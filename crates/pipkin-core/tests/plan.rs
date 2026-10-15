//! The plan projection: engine-reported tasks, the person's view choices, and edits that go
//! back through the engine under a request key, with stale answers ignored.

use pipkin_core::*;
mod support;

fn state() -> AppState {
    let mut state = AppState::new(
        Bootstrap {
            projects: vec![Project {
                id: ProjectId(1),
                name: "P".into(),
                path: "/tmp/p".into(),
            }],
            conversations: vec![
                (ConversationId(1), ProjectId(1), "one".into(), 1),
                (ConversationId(2), ProjectId(1), "two".into(), 2),
            ],
            models: vec![],
            now: 1,
        },
        Prefs::default(),
    );
    state.mode = Mode::Real;
    state.set_connection(Connection::Ready);
    state.dispatch(Command::SelectConversation(ConversationId(1)));
    support::native_context(&mut state);
    state
}

fn event(s: &AppState, kind: EventKind) -> BackendEvent {
    let c = s.current().unwrap();
    BackendEvent {
        conversation: c.id,
        generation: c.generation,
        op: None,
        kind,
    }
}

fn task(id: u64, status: PlanTaskStatus) -> PlanTask {
    PlanTask {
        id,
        parent: None,
        title: format!("Task {id}"),
        detail: String::new(),
        status,
        reason: None,
        depends_on: vec![],
        criteria: String::new(),
        evidence: vec![],
        owner: None,
        archived: false,
        stale: false,
        revision: 1,
    }
}

fn synced(tasks: Vec<PlanTask>, revision: u64) -> EventKind {
    let mut counts = PlanCounts::default();
    for t in &tasks {
        match t.status {
            PlanTaskStatus::Pending => counts.pending += 1,
            PlanTaskStatus::Active => counts.active += 1,
            PlanTaskStatus::Blocked => counts.blocked += 1,
            PlanTaskStatus::Completed => counts.completed += 1,
            PlanTaskStatus::Skipped => counts.skipped += 1,
            PlanTaskStatus::Cancelled => counts.cancelled += 1,
        }
    }
    EventKind::PlanSynced {
        revision,
        tasks,
        counts,
        truncated: false,
    }
}

fn opened(s: &mut AppState) {
    s.apply_event(event(
        s,
        EventKind::Opened {
            items: vec![],
            has_older: false,
            changes: vec![],
        },
    ));
}

fn backend_requests(out: &Outcome) -> Vec<&BackendRequest> {
    out.effects
        .iter()
        .filter_map(|e| match e {
            Effect::Backend(r) => Some(r),
            _ => None,
        })
        .collect()
}

#[test]
fn a_plan_is_unavailable_until_an_opened_session_reports_one() {
    let mut s = state();
    assert!(!s.current().unwrap().plan.available);
    // Before the conversation is open, a plan is ignored (it belongs to an attachment).
    s.apply_event(event(&s, synced(vec![task(1, PlanTaskStatus::Active)], 1)));
    assert!(!s.current().unwrap().plan.available);
    opened(&mut s);
    s.apply_event(event(&s, synced(vec![task(1, PlanTaskStatus::Active)], 1)));
    let plan = &s.current().unwrap().plan;
    assert!(plan.available);
    assert_eq!(plan.revision, 1);
    assert_eq!(plan.counts.active, 1);
    assert_eq!(plan.tasks.len(), 1);
    // Reopening resets it: a newer attachment may land on an engine without plans.
    opened(&mut s);
    assert_eq!(s.current().unwrap().plan, PlanState::default());
}

#[test]
fn a_plan_from_another_generation_or_conversation_lands_where_it_belongs() {
    let mut s = state();
    opened(&mut s);
    let mut stale = event(&s, synced(vec![task(1, PlanTaskStatus::Active)], 5));
    stale.generation += 1;
    s.apply_event(stale);
    assert!(
        !s.current().unwrap().plan.available,
        "another generation's plan is dropped"
    );
    // A plan for conversation 2 while 1 is selected goes to 2 (once 2 is open).
    s.dispatch(Command::SelectConversation(ConversationId(2)));
    opened(&mut s);
    let for_two = event(&s, synced(vec![task(9, PlanTaskStatus::Pending)], 1));
    s.dispatch(Command::SelectConversation(ConversationId(1)));
    s.apply_event(for_two);
    assert!(!s.current().unwrap().plan.available);
    s.dispatch(Command::SelectConversation(ConversationId(2)));
    assert_eq!(s.current().unwrap().plan.tasks[0].id, 9);
}

#[test]
fn expanding_and_history_are_view_choices_that_survive_resyncs() {
    let mut s = state();
    opened(&mut s);
    s.apply_event(event(
        &s,
        synced(
            vec![
                task(1, PlanTaskStatus::Completed),
                task(2, PlanTaskStatus::Active),
            ],
            1,
        ),
    ));
    s.dispatch(Command::TogglePlanTask(2));
    s.dispatch(Command::TogglePlanHistory);
    let plan = &s.current().unwrap().plan;
    assert_eq!(plan.expanded, Some(2));
    assert!(plan.history_open);
    s.dispatch(Command::TogglePlanTask(77));
    assert_eq!(
        s.current().unwrap().plan.expanded,
        Some(2),
        "unknown tasks are ignored"
    );
    s.apply_event(event(
        &s,
        synced(
            vec![
                task(1, PlanTaskStatus::Completed),
                task(2, PlanTaskStatus::Completed),
            ],
            2,
        ),
    ));
    let plan = &s.current().unwrap().plan;
    assert_eq!(plan.expanded, Some(2));
    assert!(plan.history_open);
    // A task that disappears collapses the expansion.
    s.apply_event(event(
        &s,
        synced(vec![task(1, PlanTaskStatus::Completed)], 3),
    ));
    assert_eq!(s.current().unwrap().plan.expanded, None);
    s.dispatch(Command::TogglePlanTask(1));
    s.dispatch(Command::TogglePlanTask(1));
    assert_eq!(
        s.current().unwrap().plan.expanded,
        None,
        "toggling twice collapses"
    );
}

#[test]
fn an_edit_goes_to_the_engine_under_a_request_key_conditional_on_the_revision() {
    let mut s = state();
    opened(&mut s);
    assert!(!s.can_edit_plan(), "no plan reported yet");
    s.apply_event(event(
        &s,
        synced(
            vec![
                task(1, PlanTaskStatus::Active),
                task(2, PlanTaskStatus::Completed),
            ],
            4,
        ),
    ));
    assert!(s.can_edit_plan());
    // An edit the task does not offer is refused locally without a request.
    let out = s.dispatch(Command::EditPlanTask {
        task: 2,
        edit: PlanEdit::Complete,
    });
    assert!(backend_requests(&out).is_empty());
    let out = s.dispatch(Command::EditPlanTask {
        task: 1,
        edit: PlanEdit::Skip,
    });
    let requests = backend_requests(&out);
    let [
        BackendRequest::UpdatePlan {
            conversation,
            request,
            task: t,
            status,
            reason,
            expected_revision,
            ..
        },
    ] = requests.as_slice()
    else {
        panic!("{requests:?}");
    };
    assert_eq!(*conversation, ConversationId(1));
    assert_eq!(*t, 1);
    assert_eq!(*status, PlanTaskStatus::Skipped);
    assert_eq!(
        reason.as_deref(),
        Some("Skipped by the person from the plan view")
    );
    assert_eq!(*expected_revision, 4);
    let request = request.clone();
    assert_eq!(
        s.current().unwrap().plan.pending_edit,
        Some(request.clone())
    );
    assert!(!s.can_edit_plan(), "one edit at a time");
    // A second edit while one is pending sends nothing.
    let out = s.dispatch(Command::EditPlanTask {
        task: 2,
        edit: PlanEdit::Reopen,
    });
    assert!(backend_requests(&out).is_empty());
    // An answer for another request is ignored; the matching one clears the wait.
    s.apply_event(event(
        &s,
        EventKind::PlanEditApplied {
            request: RequestId("other".into()),
        },
    ));
    assert!(s.current().unwrap().plan.pending_edit.is_some());
    s.apply_event(event(
        &s,
        EventKind::PlanEditApplied {
            request: request.clone(),
        },
    ));
    assert!(s.current().unwrap().plan.pending_edit.is_none());
    assert!(s.can_edit_plan());
    // A refusal keeps its reason until dismissed.
    let out = s.dispatch(Command::EditPlanTask {
        task: 2,
        edit: PlanEdit::Reopen,
    });
    let [BackendRequest::UpdatePlan { request, .. }] = backend_requests(&out).as_slice() else {
        panic!()
    };
    let request = request.clone();
    s.apply_event(event(
        &s,
        EventKind::PlanEditFailed {
            request,
            reason: "the plan is at revision 5, not 4".into(),
        },
    ));
    let plan = &s.current().unwrap().plan;
    assert!(plan.pending_edit.is_none());
    assert_eq!(
        plan.error.as_deref(),
        Some("the plan is at revision 5, not 4")
    );
    s.dispatch(Command::DismissPlanError);
    assert_eq!(s.current().unwrap().plan.error, None);
}

#[test]
fn edits_are_offered_per_status_and_carry_honest_reasons() {
    assert_eq!(
        PlanEdit::offered(PlanTaskStatus::Active),
        [PlanEdit::Complete, PlanEdit::Skip, PlanEdit::Cancel]
    );
    assert_eq!(
        PlanEdit::offered(PlanTaskStatus::Completed),
        [PlanEdit::Reopen]
    );
    assert_eq!(
        PlanEdit::offered(PlanTaskStatus::Cancelled),
        [PlanEdit::Reopen]
    );
    let (status, reason) = PlanEdit::Complete.status_and_reason();
    assert_eq!((status, reason), (PlanTaskStatus::Completed, None));
    let (status, reason) = PlanEdit::Reopen.status_and_reason();
    assert_eq!(status, PlanTaskStatus::Pending);
    assert!(reason.unwrap().contains("by the person"));
    let counts = PlanCounts {
        pending: 1,
        active: 2,
        blocked: 0,
        completed: 3,
        skipped: 1,
        cancelled: 0,
        archived: 4,
    };
    assert_eq!((counts.open(), counts.finished(), counts.live()), (3, 4, 7));
}
