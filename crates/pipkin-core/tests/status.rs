//! The status line: derived from the open conversation's state, with empty segments left
//! out, counts from engine state, and context from the engine's estimate only.

use pipkin_core::*;

fn state() -> AppState {
    let mut state = AppState::new(
        Bootstrap {
            projects: vec![Project {
                id: ProjectId(1),
                name: "P".into(),
                path: "/tmp/p".into(),
            }],
            conversations: vec![(ConversationId(1), ProjectId(1), "one".into(), 1)],
            models: vec![ModelInfo {
                id: "p/m".into(),
                name: "Model M".into(),
                note: String::new(),
            }],
            now: 1,
        },
        Prefs::default(),
    );
    state.mode = Mode::Real;
    state.set_connection(Connection::Ready);
    state.dispatch(Command::SelectConversation(ConversationId(1)));
    let c = state.current().unwrap();
    let ev = BackendEvent {
        conversation: c.id,
        generation: c.generation,
        op: None,
        kind: EventKind::Opened {
            items: vec![],
            has_older: false,
            changes: vec![],
        },
    };
    state.apply_event(ev);
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

#[test]
fn an_idle_conversation_shows_only_the_model() {
    let state = state();
    let line = state.status_line().unwrap();
    assert_eq!(line.model.as_deref(), Some("Model M"));
    assert_eq!(line.effort, None);
    assert_eq!(line.phase, StatusPhase::Idle);
    assert_eq!(
        (
            line.queued,
            line.active_jobs,
            line.active_children,
            line.open_questions
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(line.context, None);
    assert_eq!(line.total_tokens, None);
    assert_eq!(line.accessible(), "Idle, model Model M");
}

#[test]
fn counts_context_and_usage_come_from_engine_state() {
    let mut state = state();
    let ev = event(
        &state,
        EventKind::JobsSynced {
            jobs: JobsSync {
                jobs: vec![],
                active: 2,
                truncated: false,
            },
        },
    );
    state.apply_event(ev);
    let ev = event(
        &state,
        EventKind::SubagentsSynced {
            enabled: true,
            children: vec![
                SubagentInfo {
                    id: 1,
                    task_id: 1,
                    call_id: "c1".into(),
                    task: "a".into(),
                    status: "running".into(),
                    profile: None,
                    ..SubagentInfo::default()
                },
                SubagentInfo {
                    id: 2,
                    task_id: 2,
                    call_id: "c2".into(),
                    task: "b".into(),
                    status: "done".into(),
                    profile: None,
                    ..SubagentInfo::default()
                },
            ],
        },
    );
    state.apply_event(ev);
    let ev = event(
        &state,
        EventKind::ContextSynced {
            context: ContextUsage {
                tokens: Some(50_000),
                source: ContextSource::Estimated,
                capacity: Some(200_000),
                reserve: Some(16_000),
                remaining: Some(134_000),
                model: Some("p/m".into()),
                stale: false,
                compaction: "idle".into(),
                revision: 9,
            },
        },
    );
    state.apply_event(ev);
    let ev = event(
        &state,
        EventKind::UsageSynced(SessionUsage {
            session_id: "s".into(),
            models: vec![(
                "p/m".into(),
                UsageAmount {
                    input: 100,
                    output: 20,
                    total_tokens: 120,
                    cost_usd: Some(0.5),
                    ..Default::default()
                },
            )],
            tools: vec![],
        }),
    );
    state.apply_event(ev);
    let line = state.status_line().unwrap();
    assert_eq!(line.active_jobs, 2);
    assert_eq!(line.active_children, 1, "finished children do not count");
    let context = line.context.as_ref().unwrap();
    assert_eq!(context.percent(), Some(25));
    assert!(context.known());
    assert_eq!(line.total_tokens, Some(120));
    assert_eq!(line.cost_usd, Some(0.5));
    assert_eq!(
        line.accessible(),
        "Idle, model Model M, 2 jobs running, 1 subagents running, context 25 percent used (estimated), 120 session tokens"
    );
    // Context percent never comes from cumulative usage: no capacity, no percent.
    let ev = event(
        &state,
        EventKind::ContextSynced {
            context: ContextUsage {
                tokens: Some(70),
                source: ContextSource::Reported,
                capacity: None,
                ..Default::default()
            },
        },
    );
    state.apply_event(ev);
    let line = state.status_line().unwrap();
    assert_eq!(line.context.as_ref().unwrap().percent(), None);
    assert!(line.accessible().contains("context 70 tokens (reported)"));
    // Unavailable context is not known and not spoken.
    let ev = event(
        &state,
        EventKind::ContextSynced {
            context: ContextUsage::default(),
        },
    );
    state.apply_event(ev);
    let line = state.status_line().unwrap();
    assert!(!line.context.as_ref().unwrap().known());
    assert!(!line.accessible().contains("context"));
}
