//! Fixed profiles and temporary readiness gates are independent.
use pipkin_core::*;
mod support;

const BLOCKED: &str = "Execution is not enabled. History remains available.";
const A: ConversationId = ConversationId(1);
const B: ConversationId = ConversationId(2);

fn state() -> AppState {
    let mut s = AppState::new(
        Bootstrap {
            projects: vec![Project {
                id: ProjectId(1),
                name: "project".into(),
                path: "/project".into(),
            }],
            models: vec![],
            conversations: vec![
                (A, ProjectId(1), "disabled".into(), 1),
                (B, ProjectId(1), "native".into(), 2),
            ],
            now: 1,
        },
        Prefs::default(),
    );
    s.mode = Mode::Real;
    s.can_create = true;
    s.dispatch(Command::SelectConversation(A));
    event(
        &mut s,
        A,
        EventKind::Opened {
            items: vec![],
            has_older: true,
            changes: vec![],
        },
    );
    support::native_context(&mut s);
    s.dispatch(Command::EditDraft("keep this draft".into()));
    s
}

fn event(s: &mut AppState, id: ConversationId, kind: EventKind) -> Outcome {
    s.apply_event(BackendEvent {
        conversation: id,
        generation: s.conversation(id).unwrap().generation,
        op: None,
        kind,
    })
}

fn block(s: &mut AppState) {
    event(
        s,
        A,
        EventKind::ExecutionContext {
            execution_blocked: Some(BLOCKED.into()),
            profile: ExecutionProfile::NativeAgent,
        },
    );
}

fn no_execution(out: &Outcome) {
    assert!(
        !out.effects
            .iter()
            .any(|e| matches!(e, Effect::Backend(_) | Effect::JournalIntent { .. })),
        "{out:?}"
    );
}

#[test]
fn disabled_context_refuses_execution_intents_and_preserves_drafts_and_reads() {
    let mut s = state();
    block(&mut s);
    assert_eq!(s.read_only_reason(A).as_deref(), Some(BLOCKED));
    assert!(!s.availability().submit);
    assert!(!s.availability().retry);
    assert!(s.availability().load_older);
    assert!(s.availability().new_conversation);
    for command in [
        Command::Submit,
        Command::Retry,
        Command::Steer,
        Command::QueueFollowUp,
        Command::RemoveQueued(QueueId(1)),
        Command::Cancel,
        Command::RetryStop,
        Command::SetModel("other".into()),
        Command::SetThinkingLevel("high".into()),
        Command::SetGoal("automate".into()),
        Command::ClearGoal,
        Command::PauseGoal,
        Command::ResumeGoal,
        Command::SetGoalLimits {
            max_turns: Some(5),
            max_tokens: None,
            max_cost_usd: None,
        },
        Command::SetGoalCriteria(vec!["done".into()]),
        Command::SetGoalAudit(true),
        Command::SkipGoalAudit,
        Command::SetSubagentsEnabled(true),
        Command::LaunchSubagent("work".into()),
        Command::SteerSubagent {
            child: 1,
            text: "work".into(),
        },
        Command::StopSubagent(1),
        Command::CancelJob(1),
        Command::EditPlanTask {
            task: 1,
            edit: PlanEdit::Complete,
        },
        Command::AnswerUiRequest {
            id: "question".into(),
            answer: UiAnswer::Confirm(true),
        },
        Command::CancelUiRequest("question".into()),
        Command::McpTestServer("fixture".into()),
        Command::McpReconnectServer("fixture".into()),
        Command::McpSignIn("fixture".into()),
    ] {
        s.notice = None;
        no_execution(&s.dispatch(command));
        assert_eq!(s.notice.as_deref(), Some(BLOCKED));
        assert_eq!(s.current().unwrap().draft.text, "keep this draft");
    }
    let c = s.conversations.iter_mut().find(|c| c.id == A).unwrap();
    c.run = RunState::Running { op: OperationId(1) };
    assert!(!s.availability().steer);
    assert!(!s.availability().queue);
    assert!(!s.availability().cancel);
    no_execution(&s.dispatch(Command::Steer));
    no_execution(&s.dispatch(Command::QueueFollowUp));
    let out = s.dispatch(Command::LoadOlder);
    assert!(
        out.effects
            .iter()
            .any(|e| matches!(e, Effect::Backend(BackendRequest::LoadOlder { .. })))
    );
    let out = s.dispatch(Command::NewConversation);
    assert!(out.effects.iter().any(|e| matches!(
        e,
        Effect::Backend(BackendRequest::CreateConversation { .. })
    )));
}

#[test]
fn snapshot_gate_is_conversation_and_generation_scoped_and_not_cleared_by_history() {
    let mut s = state();
    block(&mut s);
    let generation = s.current().unwrap().generation;
    s.dispatch(Command::SelectConversation(B));
    event(
        &mut s,
        B,
        EventKind::ExecutionContext {
            execution_blocked: None,
            profile: ExecutionProfile::NativeAgent,
        },
    );
    event(
        &mut s,
        B,
        EventKind::Opened {
            items: vec![],
            has_older: false,
            changes: vec![],
        },
    );
    s.dispatch(Command::EditDraft("native still works".into()));
    assert!(s.availability().submit);
    assert_eq!(s.read_only_reason(A).as_deref(), Some(BLOCKED));
    assert_eq!(s.read_only_reason(B), None);
    // A delayed update for A must neither affect B nor bypass A's gate.
    for (event_generation, op) in [(generation - 1, None), (generation, Some(OperationId(900)))] {
        s.apply_event(BackendEvent {
            conversation: A,
            generation: event_generation,
            op,
            kind: EventKind::ExecutionContext {
                execution_blocked: None,
                profile: ExecutionProfile::Unsupported,
            },
        });
        assert_eq!(s.read_only_reason(A).as_deref(), Some(BLOCKED));
        assert_eq!(s.read_only_reason(B), None);
    }
    for kind in [
        EventKind::Opened {
            items: vec![],
            has_older: true,
            changes: vec![],
        },
        EventKind::Synced { items: vec![] },
        EventKind::OlderPage {
            items: vec![],
            has_older: false,
        },
        EventKind::OpenFailed {
            message: "offline".into(),
        },
    ] {
        event(&mut s, A, kind);
        assert_eq!(s.read_only_reason(A).as_deref(), Some(BLOCKED));
    }
    s.dispatch(Command::SelectConversation(A));
    s.apply_event(BackendEvent {
        conversation: A,
        generation,
        op: None,
        kind: EventKind::ExecutionContext {
            execution_blocked: None,
            profile: ExecutionProfile::Unsupported,
        },
    });
    assert!(!s.availability().submit);
    // Only the new attachment's authoritative identity can remove the gate.
    event(
        &mut s,
        A,
        EventKind::ExecutionContext {
            execution_blocked: None,
            profile: ExecutionProfile::NativeAgent,
        },
    );
    assert!(s.availability().submit);
}

#[test]
fn journal_ack_checks_its_conversation_even_after_selection_changes() {
    let mut s = state();
    let out = s.dispatch(Command::Submit);
    let request = out
        .effects
        .iter()
        .find_map(|e| match e {
            Effect::JournalIntent { request, .. } => Some(request.clone()),
            _ => None,
        })
        .unwrap();
    block(&mut s);
    s.dispatch(Command::SelectConversation(B));
    let out = s.intent_persisted(A, &request, Ok(()));
    no_execution(&out);
    assert!(out.effects.iter().any(|e| matches!(
        e,
        Effect::JournalState {
            state: JournalState::Cancelled,
            ..
        }
    )));
    assert_eq!(s.conversation(A).unwrap().draft.text, "keep this draft");
    assert!(s.conversation(A).unwrap().pending_intent.is_none());
    assert_eq!(s.read_only_reason(B), None);
}

#[test]
fn disabled_goal_does_not_migrate_or_automatically_continue() {
    let mut s = state();
    s.restore_goal(A, "legacy goal".into(), None);
    block(&mut s);
    no_execution(&event(&mut s, A, EventKind::GoalSynced { goal: None }));

    // Also cover the desktop runner and demo queue's event-driven path.
    s.mode = Mode::Demo;
    s.restore_goal(A, "work".into(), None);
    let c = s.conversations.iter_mut().find(|c| c.id == A).unwrap();
    c.goal_native = false;
    c.goal.as_mut().unwrap().paused = None;
    c.run = RunState::Running {
        op: OperationId(42),
    };
    c.queue.push(QueuedPrompt {
        id: QueueId(1),
        text: "queued".into(),
        mode: QueueMode::FollowUp,
        source: None,
    });
    let generation = c.generation;
    let out = s.apply_event(BackendEvent {
        conversation: A,
        generation,
        op: Some(OperationId(42)),
        kind: EventKind::Completed,
    });
    no_execution(&out);
    assert_eq!(s.conversation(A).unwrap().queue.len(), 1);
}
