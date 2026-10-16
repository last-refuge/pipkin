//! Close/Reopen is reversible organization, never an execution or deletion request.
use pipkin_core::*;
mod support;

fn bootstrap() -> Bootstrap {
    Bootstrap {
        projects: vec![
            Project {
                id: ProjectId(1),
                name: "one".into(),
                path: "/one".into(),
            },
            Project {
                id: ProjectId(2),
                name: "two".into(),
                path: "/two".into(),
            },
        ],
        conversations: vec![
            (ConversationId(1), ProjectId(1), "first".into(), 1),
            (ConversationId(2), ProjectId(1), "second".into(), 2),
            (ConversationId(3), ProjectId(2), "other project".into(), 3),
        ],
        models: vec![],
        now: 1,
    }
}
fn state() -> AppState {
    let mut s = AppState::new(bootstrap(), Prefs::default());
    s.mode = Mode::Real;
    s.set_connection(Connection::Ready);
    s.dispatch(Command::SelectConversation(ConversationId(1)));
    support::native_context(&mut s);
    for c in &mut s.conversations {
        c.opened = true;
    }
    for id in 1..=3 {
        event(&mut s, id, EventKind::ConversationLifecycleReady(true));
    }
    s
}
fn close(s: &mut AppState, id: u64) -> Outcome {
    let generation = s.conversation(ConversationId(id)).unwrap().generation;
    s.dispatch(Command::CloseConversation {
        conversation: ConversationId(id),
        generation,
    })
}
fn event(s: &mut AppState, id: u64, kind: EventKind) -> Outcome {
    s.apply_event(BackendEvent {
        conversation: ConversationId(id),
        generation: s.conversation(ConversationId(id)).unwrap().generation,
        op: None,
        kind,
    })
}
fn organization_effects_only(out: &Outcome) {
    assert!(
        out.effects.iter().all(|e| matches!(
            e,
            Effect::SavePrefs(_)
                | Effect::LoadCache { .. }
                | Effect::Backend(BackendRequest::Open { .. })
        )),
        "{:?}",
        out.effects
    );
}
fn goal(state: GoalState) -> Goal {
    Goal {
        text: "goal".into(),
        paused: (state == GoalState::Paused).then(|| "paused".into()),
        turns: 0,
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

#[test]
fn close_and_reopen_preserve_draft_history_and_each_execution_identity() {
    for (profile, model) in [
        (
            ExecutionProfile::NativeAgent,
            ProviderModel::Native("native".into()),
        ),
        (
            ExecutionProfile::LocalCliChatOnly,
            ProviderModel::ClaudeCli(CliModel::Sonnet),
        ),
        (
            ExecutionProfile::LocalCliRestrictedWrite,
            ProviderModel::ClaudeCli(CliModel::Opus),
        ),
    ] {
        let mut s = state();
        let c = &mut s.conversations[0];
        c.profile = profile;
        c.provider_model = Some(model.clone());
        c.draft.text = "unsent draft".into();
        c.draft.rev = 8;
        c.draft.save = SaveState::Saved;
        c.items.push(TranscriptItem {
            native_only: false,
            id: ItemId(1),
            at: 1,
            kind: ItemKind::Assistant {
                text: "history".into(),
                streaming: false,
            },
        });
        let draft = c.draft.clone();
        let items = c.items.clone();
        organization_effects_only(&close(&mut s, 1));
        assert_eq!(s.selected, Some(ConversationId(2)));
        assert_eq!(
            s.visible_conversations()
                .iter()
                .map(|c| c.id)
                .collect::<Vec<_>>(),
            [ConversationId(2)]
        );
        assert_eq!(s.closed_conversations()[0].id, ConversationId(1));
        assert_eq!(s.conversations.len(), 3);
        let c = s.conversation(ConversationId(1)).unwrap();
        assert_eq!(c.draft, draft);
        assert_eq!(c.items, items);
        assert_eq!(c.profile, profile);
        organization_effects_only(&s.dispatch(Command::ReopenConversation(ConversationId(1))));
        assert!(!s.conversation_is_closed(ConversationId(1)));
        let c = s.current().unwrap();
        assert_eq!(c.id, ConversationId(1));
        assert_eq!(c.profile, profile);
        assert_eq!(c.provider_model, Some(model));
        assert_eq!(c.draft, draft);
        assert_eq!(c.items, items);
    }
}

#[test]
fn last_close_stays_blank_across_catalog_restart_and_project_selection() {
    let mut s = state();
    close(&mut s, 2);
    close(&mut s, 1);
    assert_eq!(s.selected, None);
    assert_eq!(s.prefs.selected_conversation, None);
    s.apply_catalog(bootstrap());
    s.select_initial();
    assert_eq!(s.selected, None, "do not jump to another project");
    let mut restarted = AppState::new(bootstrap(), s.prefs.clone());
    restarted.mode = Mode::Real;
    restarted.select_initial();
    assert_eq!(restarted.selected, None);
    restarted.dispatch(Command::SelectProject(ProjectId(1)));
    assert_eq!(restarted.selected, None);
    restarted.dispatch(Command::SelectProject(ProjectId(2)));
    assert_eq!(restarted.selected, Some(ConversationId(3)));
    restarted.dispatch(Command::SelectProject(ProjectId(1)));
    assert_eq!(restarted.prefs.selected_conversation, None);
    restarted.search = "first".into();
    assert!(restarted.visible_conversations().is_empty());
    assert_eq!(restarted.closed_conversations().len(), 1);
    organization_effects_only(&restarted.dispatch(Command::SelectConversation(ConversationId(1))));
    assert!(
        !restarted.conversation_is_closed(ConversationId(1)),
        "explicit search/history navigation reopens"
    );
}

#[test]
fn stale_close_and_busy_unknown_or_unsaved_work_are_rejected_without_effects() {
    for run in [
        RunState::Submitting { op: OperationId(9) },
        RunState::Running { op: OperationId(9) },
        RunState::Stopping { op: OperationId(9) },
        RunState::OutcomeUnknown { op: OperationId(9) },
    ] {
        let mut s = state();
        s.conversations[0].run = run;
        assert!(close(&mut s, 1).effects.is_empty());
        assert!(!s.conversation_is_closed(ConversationId(1)));
    }
    let guards: &[fn(&mut ConversationState)] = &[
        |c| c.opened = false,
        |c| c.status_check_pending = true,
        |c| c.waiting_question = true,
        |c| c.ui_answering.push("pending".into()),
        |c| c.draft.save = SaveState::Dirty,
        |c| c.draft.save = SaveState::Saving,
        |c| c.draft.save = SaveState::Failed("disk".into()),
        |c| c.jobs.active = 1,
        |c| {
            c.subagents.children.push(SubagentInfo {
                status: "running".into(),
                ..Default::default()
            })
        },
        |c| c.subagents.control_pending = Some(RequestId("child".into())),
        |c| {
            c.queue.push(QueuedPrompt {
                source: None,
                id: QueueId(1),
                text: "queued".into(),
                mode: QueueMode::FollowUp,
            })
        },
        |c| {
            c.pending_queue.push(PendingQueued {
                request: RequestId("queue".into()),
                text: "unknown".into(),
                attachments: vec![],
                mode: QueueMode::FollowUp,
                state: QueueSend::Unknown,
            })
        },
        |c| c.goal = Some(goal(GoalState::Active)),
        |c| c.goal = Some(goal(GoalState::Verifying)),
    ];
    for (index, guard) in guards.iter().enumerate() {
        let mut s = state();
        guard(&mut s.conversations[0]);
        assert!(
            s.close_conversation_reason(ConversationId(1)).is_some(),
            "guard {index}"
        );
        assert!(close(&mut s, 1).effects.is_empty());
        assert_eq!(s.selected, Some(ConversationId(1)));
    }
    let mut s = state();
    let stale = s.current().unwrap().generation + 1;
    assert!(
        s.dispatch(Command::CloseConversation {
            conversation: ConversationId(1),
            generation: stale
        })
        .effects
        .is_empty()
    );
    assert!(!s.conversation_is_closed(ConversationId(1)));
    s.set_connection(Connection::Offline("offline".into()));
    assert!(close(&mut s, 1).effects.is_empty());
}

#[test]
fn pending_provider_change_or_cli_creation_is_not_hidden() {
    let mut s = state();
    s.prefs
        .pending_provider_models
        .insert(1, ProviderModel::ClaudeCli(CliModel::Haiku));
    assert!(close(&mut s, 1).effects.is_empty());
    assert_eq!(s.prefs.pending_provider_models.len(), 1);
    organization_effects_only(&close(&mut s, 2));
    s.prefs.pending_provider_models.clear();
    s.local_cli.stage = LocalCliStage::CreateUnknown;
    assert!(close(&mut s, 1).effects.is_empty());
    s.local_cli.stage = LocalCliStage::Editing;
    s.conversations[0].goal = Some(goal(GoalState::Paused));
    organization_effects_only(&close(&mut s, 1));
    assert!(
        s.conversation_is_closed(ConversationId(1)),
        "a settled paused goal is retained"
    );
}

#[test]
fn late_activity_reveals_closed_conversation_without_selecting_or_replaying_it() {
    let mut s = state();
    close(&mut s, 1);
    let id = ConversationId(1);
    let generation = s.conversation(id).unwrap().generation;
    s.apply_event(BackendEvent {
        conversation: id,
        generation: generation + 1,
        op: None,
        kind: EventKind::EngineState {
            busy: true,
            queue: vec![],
        },
    });
    assert!(s.conversation_is_closed(id));
    let out = event(
        &mut s,
        1,
        EventKind::EngineState {
            busy: true,
            queue: vec![],
        },
    );
    assert!(!s.conversation_is_closed(id));
    assert_eq!(s.selected, Some(ConversationId(2)));
    assert!(s.conversation(id).unwrap().run.is_busy());
    assert!(
        out.effects
            .iter()
            .all(|e| matches!(e, Effect::SavePrefs(_)))
    );
}

#[test]
fn a_partial_or_stale_snapshot_cannot_enable_close_and_global_attention_only_reveals() {
    let mut s = state();
    event(&mut s, 1, EventKind::ConversationLifecycleReady(false));
    assert!(close(&mut s, 1).effects.is_empty());
    let generation = s.current().unwrap().generation;
    for (generation, op) in [(generation + 1, None), (generation, Some(OperationId(99)))] {
        s.apply_event(BackendEvent {
            conversation: ConversationId(1),
            generation,
            op,
            kind: EventKind::ConversationLifecycleReady(true),
        });
        assert!(close(&mut s, 1).effects.is_empty());
    }
    event(&mut s, 1, EventKind::ConversationLifecycleReady(true));
    close(&mut s, 1);
    assert!(s.conversation_is_closed(ConversationId(1)));
    let out = s.reveal_conversation_activity(ConversationId(1));
    assert!(!s.conversation_is_closed(ConversationId(1)));
    assert_eq!(s.selected, Some(ConversationId(2)));
    assert!(matches!(
        s.conversation(ConversationId(1)).unwrap().run,
        RunState::Idle
    ));
    assert!(
        out.effects
            .iter()
            .all(|e| matches!(e, Effect::SavePrefs(_)))
    );
    assert!(
        close(&mut s, 1).effects.is_empty(),
        "attention does not prove idle or grant authority"
    );
}

#[test]
fn restored_unknown_work_overrides_closed_metadata_without_a_prompt() {
    let mut s = state();
    close(&mut s, 1);
    s.conversations[0].run = RunState::OutcomeUnknown {
        op: OperationId(99),
    };
    let out = s.select_initial();
    assert!(!s.conversation_is_closed(ConversationId(1)));
    assert_eq!(s.selected, Some(ConversationId(2)));
    assert!(
        out.effects
            .iter()
            .all(|e| matches!(e, Effect::SavePrefs(_)))
    );
}

#[test]
fn goal_start_and_resume_require_exact_receipts_not_old_projections() {
    let mut s = state();
    event(&mut s, 1, EventKind::GoalSynced { goal: None });
    let out = s.dispatch(Command::SetGoal("goal".into()));
    let request = out
        .effects
        .iter()
        .find_map(|e| match e {
            Effect::Backend(BackendRequest::SetGoal { request, .. }) => Some(request.clone()),
            _ => None,
        })
        .unwrap();
    event(&mut s, 1, EventKind::GoalSynced { goal: None });
    event(
        &mut s,
        1,
        EventKind::GoalStartConfirmed {
            request: RequestId("wrong".into()),
        },
    );
    assert!(close(&mut s, 1).effects.is_empty());
    event(
        &mut s,
        1,
        EventKind::GoalSynced {
            goal: Some(goal(GoalState::Active)),
        },
    );
    event(&mut s, 1, EventKind::GoalStartConfirmed { request });
    assert!(close(&mut s, 1).effects.is_empty());
    event(
        &mut s,
        1,
        EventKind::GoalSynced {
            goal: Some(goal(GoalState::Paused)),
        },
    );
    assert!(s.close_conversation_reason(ConversationId(1)).is_none());
    let out = s.dispatch(Command::ResumeGoal);
    let request = out
        .effects
        .iter()
        .find_map(|e| match e {
            Effect::Backend(BackendRequest::GoalControl {
                request: Some(request),
                ..
            }) => Some(request.clone()),
            _ => None,
        })
        .unwrap();
    event(
        &mut s,
        1,
        EventKind::GoalSynced {
            goal: Some(goal(GoalState::Paused)),
        },
    );
    event(
        &mut s,
        1,
        EventKind::GoalRefused {
            reason: "reply lost".into(),
        },
    );
    assert!(
        close(&mut s, 1).effects.is_empty(),
        "unknown start remains visible"
    );
    event(
        &mut s,
        1,
        EventKind::GoalSynced {
            goal: Some(goal(GoalState::Active)),
        },
    );
    event(&mut s, 1, EventKind::GoalStartConfirmed { request });
    assert!(close(&mut s, 1).effects.is_empty());
}
