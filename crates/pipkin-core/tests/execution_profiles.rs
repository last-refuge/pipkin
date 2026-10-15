use pipkin_core::*;
mod support;

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
                (A, ProjectId(1), "chat".into(), 1),
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
        None,
        EventKind::Opened {
            items: vec![],
            has_older: true,
            changes: vec![],
        },
    );
    support::native_context(&mut s);
    s.dispatch(Command::EditDraft("explicit text".into()));
    s
}

fn event(s: &mut AppState, op: Option<OperationId>, kind: EventKind) -> Outcome {
    s.apply_event(BackendEvent {
        conversation: A,
        generation: s.conversation(A).unwrap().generation,
        op,
        kind,
    })
}

fn context(s: &mut AppState, profile: ExecutionProfile, blocked: bool) {
    event(
        s,
        None,
        EventKind::ExecutionContext {
            profile,
            execution_blocked: blocked.then(|| "not ready".into()),
        },
    );
}

fn request(out: Outcome) -> RequestId {
    out.effects
        .into_iter()
        .find_map(|e| match e {
            Effect::JournalIntent { request, .. } => Some(request),
            _ => None,
        })
        .expect("explicit journal intent")
}

fn no_execution(out: &Outcome) {
    assert!(
        !out.effects
            .iter()
            .any(|e| matches!(e, Effect::Backend(_) | Effect::JournalIntent { .. })),
        "{out:?}"
    );
}

fn native_only() -> Vec<Command> {
    vec![
        Command::Retry,
        Command::Steer,
        Command::QueueFollowUp,
        Command::RemoveQueued(QueueId(1)),
        Command::SetModel("model".into()),
        Command::SetThinkingLevel("high".into()),
        Command::RefreshModels,
        Command::SetGoal("goal".into()),
        Command::ClearGoal,
        Command::PauseGoal,
        Command::ResumeGoal,
        Command::SetGoalLimits {
            max_turns: Some(1),
            max_tokens: None,
            max_cost_usd: None,
        },
        Command::SetGoalCriteria(vec!["done".into()]),
        Command::SetGoalAudit(true),
        Command::SkipGoalAudit,
        Command::TogglePlanTask(1),
        Command::TogglePlanHistory,
        Command::DismissPlanError,
        Command::EditPlanTask {
            task: 1,
            edit: PlanEdit::Complete,
        },
        Command::SelectSubagent(Some(1)),
        Command::SetSubagentsEnabled(true),
        Command::ToggleSubagentHistory,
        Command::ShowMoreSubagentHistory,
        Command::LoadOlderSubagent,
        Command::StopSubagent(1),
        Command::SteerSubagent {
            child: 1,
            text: "steer".into(),
        },
        Command::LaunchSubagent("task".into()),
        Command::ToggleJobHistory,
        Command::ToggleJob(1),
        Command::RefreshJobLog(1),
        Command::CancelJob(1),
        Command::MarkJobsSeen,
        Command::DismissJobError,
        Command::AnswerUiRequest {
            id: "tool".into(),
            answer: UiAnswer::Confirm(true),
        },
        Command::CancelUiRequest("tool".into()),
        Command::ToggleFormOption {
            question: "q".into(),
            option: "a".into(),
        },
        Command::McpRefresh,
        Command::McpAddServer(McpServerInput {
            scope: McpScope::User,
            name: "server".into(),
            kind: McpKind::Stdio,
            target: "tool".into(),
            values: vec![],
        }),
        Command::McpRemoveServer("server".into()),
        Command::McpTestServer("server".into()),
        Command::McpReconnectServer("server".into()),
        Command::McpSetToolEnabled {
            server: "server".into(),
            tool: "tool".into(),
            enabled: true,
        },
        Command::McpSignIn("server".into()),
        Command::McpAnswerSignIn {
            server: "server".into(),
            prompt: "prompt".into(),
            response: onboarding::SignInAnswer::new("answer".into()),
        },
        Command::McpCancelSignIn("server".into()),
        Command::McpSignOut("server".into()),
        Command::McpPreviewImport("{}".into()),
        Command::McpToggleImportEntry("server".into()),
        Command::McpApplyImport(McpScope::User),
        Command::McpDismissImport,
        Command::McpToggleTools("server".into()),
        Command::McpDismissNotice,
    ]
}

#[test]
fn saved_native_model_is_not_journaled_or_sent_for_cli_chat() {
    for profile in [
        ExecutionProfile::NativeAgent,
        ExecutionProfile::LocalCliChatOnly,
        ExecutionProfile::LocalCliRestrictedWrite,
    ] {
        let mut s = state();
        s.prefs.model = Some("saved/native-model".into());
        context(&mut s, profile, false);
        assert!(s.availability().submit);
        let expected =
            (profile == ExecutionProfile::NativeAgent).then_some("saved/native-model".to_string());
        let out = s.dispatch(Command::Submit);
        assert!(out.effects.iter().any(|e| matches!(
            e, Effect::JournalIntent { model, .. } if model == &expected
        )));
        assert_eq!(
            s.current().unwrap().pending_intent.as_ref().unwrap().model,
            expected
        );
        let request = request(out);
        let out = s.intent_persisted(A, &request, Ok(()));
        assert!(out.effects.iter().any(|e| matches!(
            e, Effect::Backend(BackendRequest::Submit { model, .. }) if model == &expected
        )));
        assert_eq!(s.prefs.model.as_deref(), Some("saved/native-model"));
    }
}

#[test]
fn cli_ready_text_submit_and_stop_retry_after_readiness_loss() {
    cli_ready_text_and_stop(ExecutionProfile::LocalCliChatOnly);
    cli_ready_text_and_stop(ExecutionProfile::LocalCliRestrictedWrite);
}
fn cli_ready_text_and_stop(profile: ExecutionProfile) {
    let mut s = state();
    context(&mut s, profile, false);
    assert!(s.availability().submit);
    let request = request(s.dispatch(Command::Submit));
    let out = s.intent_persisted(A, &request, Ok(()));
    let op = out
        .effects
        .iter()
        .find_map(|e| match e {
            Effect::Backend(BackendRequest::Submit {
                op,
                text,
                attachments,
                ..
            }) => {
                assert_eq!(text, "explicit text");
                assert!(attachments.is_empty());
                Some(*op)
            }
            _ => None,
        })
        .unwrap();
    context(&mut s, profile, true);
    assert_eq!(s.current().unwrap().run, RunState::Submitting { op });
    event(&mut s, Some(op), EventKind::Accepted);
    assert_eq!(s.current().unwrap().run, RunState::Running { op });
    assert!(s.availability().cancel);
    assert!(s.availability().check_status);
    let out = s.dispatch(Command::Cancel);
    assert!(out.effects.iter().any(
        |e| matches!(e, Effect::Backend(BackendRequest::Cancel { op: got, .. }) if *got == op)
    ));
    event(
        &mut s,
        Some(op),
        EventKind::StopFailed {
            message: "try stop again".into(),
        },
    );
    assert!(s.availability().retry_stop);
    let out = s.dispatch(Command::RetryStop);
    assert!(out.effects.iter().any(
        |e| matches!(e, Effect::Backend(BackendRequest::Cancel { op: got, .. }) if *got == op)
    ));
    event(&mut s, Some(op), EventKind::Cancelled);
    assert_eq!(s.current().unwrap().run, RunState::Idle);
    assert!(!s.availability().submit);
}

#[test]
fn cached_unopened_conversation_needs_authoritative_context_not_history() {
    let mut s = AppState::new(
        Bootstrap {
            projects: vec![],
            models: vec![],
            conversations: vec![],
            now: 1,
        },
        Prefs::default(),
    );
    s.mode = Mode::Real;
    s.restore_cached_conversation(A, "/project", "cached external".into(), 1);
    s.selected = Some(A);
    s.apply_cache(
        A,
        vec![TranscriptItem {
            native_only: false,
            id: ItemId(1),
            at: 1,
            kind: ItemKind::User {
                text: "cached".into(),
                attachments: vec![],
                delivery: Delivery::Sent,
                steer: false,
            },
        }],
        true,
        1,
    );
    assert!(s.current().unwrap().cached_only);
    assert!(!s.current().unwrap().opened);
    for opened in [false, true] {
        if opened {
            s.dispatch(Command::SelectConversation(A));
            event(
                &mut s,
                None,
                EventKind::Opened {
                    items: vec![],
                    has_older: true,
                    changes: vec![],
                },
            );
        }
        assert_eq!(s.execution_capabilities(), ExecutionCapabilities::default());
        for cmd in native_only() {
            no_execution(&s.dispatch(cmd));
        }
        no_execution(&s.dispatch(Command::Submit));
    }
    // Stale or operation-scoped context cannot grant capabilities either.
    let generation = s.current().unwrap().generation;
    let kind = EventKind::ExecutionContext {
        profile: ExecutionProfile::NativeAgent,
        execution_blocked: None,
    };
    s.apply_event(BackendEvent {
        conversation: A,
        generation: generation + 1,
        op: None,
        kind: kind.clone(),
    });
    event(&mut s, Some(OperationId(123)), kind);
    assert_eq!(s.execution_capabilities(), ExecutionCapabilities::default());
    context(&mut s, ExecutionProfile::LocalCliChatOnly, false);
    for cmd in native_only() {
        no_execution(&s.dispatch(cmd));
    }
    assert_eq!(s.execution_capabilities(), ExecutionCapabilities::default());
    context(&mut s, ExecutionProfile::NativeAgent, false);
    assert!(s.execution_capabilities().model_settings);
    assert!(s.execution_capabilities().attachments);
    for cmd in [
        Command::McpTestServer("server".into()),
        Command::McpReconnectServer("server".into()),
    ] {
        assert!(
            s.dispatch(cmd)
                .effects
                .iter()
                .any(|e| matches!(e, Effect::Backend(_)))
        );
    }
}

#[test]
fn execution_capabilities_are_pure_and_demo_retains_native_affordances() {
    let mut s = state();
    for profile in [
        ExecutionProfile::NativeAgent,
        ExecutionProfile::LocalCliChatOnly,
        ExecutionProfile::LocalCliRestrictedWrite,
        ExecutionProfile::Unsupported,
    ] {
        context(&mut s, profile, true);
        let before = format!("{:?}", s.current());
        let expected = profile == ExecutionProfile::NativeAgent;
        assert_eq!(
            s.execution_capabilities(),
            ExecutionCapabilities {
                model_settings: expected,
                attachments: expected
            }
        );
        assert_eq!(format!("{:?}", s.current()), before);
    }
    let demo = AppState::new(
        Bootstrap {
            projects: vec![],
            models: vec![],
            conversations: vec![(A, ProjectId(1), "demo".into(), 1)],
            now: 1,
        },
        Prefs::default(),
    );
    assert!(demo.execution_capabilities().model_settings);
    assert!(demo.execution_capabilities().attachments);
}

#[test]
fn restricted_profiles_deny_every_native_only_intent_ready_or_unready() {
    for profile in [
        ExecutionProfile::LocalCliChatOnly,
        ExecutionProfile::LocalCliRestrictedWrite,
        ExecutionProfile::Unsupported,
    ] {
        for blocked in [false, true] {
            for run in [
                RunState::Idle,
                RunState::Submitting { op: OperationId(9) },
                RunState::Running { op: OperationId(9) },
                RunState::Stopping { op: OperationId(9) },
            ] {
                let mut s = state();
                context(&mut s, profile, blocked);
                let c = s.conversations.iter_mut().find(|c| c.id == A).unwrap();
                c.run = run.clone();
                c.plan.available = true;
                c.stop_error = Some("retry".into());
                c.last_submission = Some(("last".into(), vec![]));
                assert!(!s.can_edit_plan());
                let a = s.availability();
                assert!(!a.queue && !a.steer && !a.retry && !a.refresh_models);
                for cmd in native_only() {
                    s.notice = None;
                    let out = s.dispatch(cmd.clone());
                    assert!(
                        out.effects.is_empty(),
                        "{profile:?} {blocked} {cmd:?}: {out:?}"
                    );
                    assert!(s.notice.is_some(), "not denied: {cmd:?}");
                    assert_eq!(s.current().unwrap().draft.text, "explicit text");
                    assert_eq!(s.current().unwrap().run, run);
                }
                if profile == ExecutionProfile::Unsupported {
                    assert!(s.read_only_reason(A).is_some());
                    assert!(!a.submit && !a.cancel && !a.retry_stop);
                    for cmd in [Command::Submit, Command::Cancel, Command::RetryStop] {
                        no_execution(&s.dispatch(cmd));
                    }
                }
            }
        }
    }
}

#[test]
fn cli_requires_ready_text_without_attachments_and_keeps_safe_actions() {
    let mut s = state();
    context(&mut s, ExecutionProfile::LocalCliChatOnly, true);
    no_execution(&s.dispatch(Command::Submit));
    assert!(s.availability().load_older && s.availability().new_conversation);
    s.dispatch(Command::EditDraft("new draft".into()));
    assert_eq!(s.current().unwrap().draft.text, "new draft");
    assert!(
        s.dispatch(Command::LoadOlder)
            .effects
            .iter()
            .any(|e| matches!(e, Effect::Backend(BackendRequest::LoadOlder { .. })))
    );
    assert!(
        s.dispatch(Command::NewConversation)
            .effects
            .iter()
            .any(|e| matches!(
                e,
                Effect::Backend(BackendRequest::CreateConversation { .. })
            ))
    );
    context(&mut s, ExecutionProfile::LocalCliChatOnly, false);
    s.dispatch(Command::EditDraft("   ".into()));
    assert!(!s.availability().submit);
    no_execution(&s.dispatch(Command::Submit));
    s.dispatch(Command::EditDraft("text".into()));
    let attachment = Attachment {
        path: "/image.png".into(),
        name: "image.png".into(),
        size: Some(1),
        error: None,
    };
    no_execution(&s.dispatch(Command::AddAttachments(vec![attachment.clone()])));
    assert!(s.current().unwrap().draft.attachments.is_empty());
    // Previously saved attachments must still block sending, and remain removable.
    s.dispatch(Command::EditDraft(String::new()));
    s.restore_draft(A, "text".into(), vec![attachment]);
    assert!(!s.availability().submit);
    no_execution(&s.dispatch(Command::Submit));
    s.dispatch(Command::RemoveAttachment(0));
    assert!(s.availability().submit);
    assert_eq!(
        s.conversation(B).unwrap().profile,
        ExecutionProfile::NativeAgent
    );
}

#[test]
fn delayed_intents_recheck_profile_origin_readiness_and_own_conversation() {
    for origin in [
        IntentOrigin::Draft,
        IntentOrigin::Retry,
        IntentOrigin::Queue,
        IntentOrigin::Goal,
        IntentOrigin::Steer,
        IntentOrigin::FollowUp,
    ] {
        for profile in [
            ExecutionProfile::LocalCliChatOnly,
            ExecutionProfile::LocalCliRestrictedWrite,
            ExecutionProfile::Unsupported,
        ] {
            for blocked in [false, true] {
                let mut s = state();
                let request = request(s.dispatch(Command::Submit));
                s.conversations
                    .iter_mut()
                    .find(|c| c.id == A)
                    .unwrap()
                    .pending_intent
                    .as_mut()
                    .unwrap()
                    .origin = origin;
                context(&mut s, profile, blocked);
                // Readiness/profile updates must not erase an in-flight journal intent.
                assert!(s.conversation(A).unwrap().pending_intent.is_some());
                s.dispatch(Command::SelectConversation(B));
                let out = s.intent_persisted(A, &request, Ok(()));
                if origin == IntentOrigin::Draft && profile.is_local_cli() && !blocked {
                    assert!(out.effects.iter().any(|e| matches!(
                        e,
                        Effect::Backend(BackendRequest::Submit {
                            conversation: A,
                            ..
                        })
                    )));
                } else {
                    no_execution(&out);
                    assert!(out.effects.iter().any(|e| matches!(
                        e,
                        Effect::JournalState {
                            state: JournalState::Cancelled,
                            ..
                        }
                    )));
                    assert_eq!(s.conversation(A).unwrap().draft.text, "explicit text");
                }
            }
        }
    }
}

#[test]
fn restricted_profiles_never_migrate_goals_or_drain_queues() {
    for profile in [
        ExecutionProfile::LocalCliChatOnly,
        ExecutionProfile::LocalCliRestrictedWrite,
        ExecutionProfile::Unsupported,
    ] {
        for blocked in [false, true] {
            let mut s = state();
            context(&mut s, profile, blocked);
            s.restore_goal(A, "legacy".into(), None);
            no_execution(&event(&mut s, None, EventKind::GoalSynced { goal: None }));
            assert!(s.current().unwrap().goal.is_some());
            s.mode = Mode::Demo;
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
            no_execution(&event(&mut s, Some(OperationId(42)), EventKind::Completed));
            assert_eq!(s.current().unwrap().queue.len(), 1);
            s.conversations
                .iter_mut()
                .find(|c| c.id == A)
                .unwrap()
                .queue
                .clear();
            no_execution(&event(
                &mut s,
                None,
                EventKind::Opened {
                    items: vec![],
                    has_older: false,
                    changes: vec![],
                },
            ));
        }
    }
}

#[test]
fn native_default_keeps_real_queue_and_demo_auto_drain() {
    assert_eq!(ExecutionProfile::default(), ExecutionProfile::NativeAgent);
    let mut s = state();
    s.conversations.iter_mut().find(|c| c.id == A).unwrap().run =
        RunState::Running { op: OperationId(9) };
    assert!(s.availability().queue && s.availability().steer);
    let request = request(s.dispatch(Command::QueueFollowUp));
    assert!(
        s.intent_persisted(A, &request, Ok(()))
            .effects
            .iter()
            .any(|e| matches!(
                e,
                Effect::Backend(BackendRequest::Queue {
                    mode: QueueMode::FollowUp,
                    ..
                })
            ))
    );
    let mut s = state();
    s.mode = Mode::Demo;
    s.conversations.iter_mut().find(|c| c.id == A).unwrap().run =
        RunState::Running { op: OperationId(9) };
    s.dispatch(Command::QueueFollowUp);
    assert_eq!(s.current().unwrap().queue.len(), 1);
    let out = event(&mut s, Some(OperationId(9)), EventKind::Completed);
    assert!(out.effects.iter().any(|e| matches!(
        e,
        Effect::JournalIntent {
            origin: IntentOrigin::Queue,
            ..
        }
    )));
    assert!(s.current().unwrap().queue.is_empty());
}

#[test]
fn stale_context_cannot_change_profile_readiness_or_live_operation() {
    let mut s = state();
    context(&mut s, ExecutionProfile::LocalCliChatOnly, true);
    let generation = s.current().unwrap().generation;
    let op = OperationId(42);
    s.conversations.iter_mut().find(|c| c.id == A).unwrap().run = RunState::Running { op };
    for (generation, event_op) in [
        (generation - 1, None),
        (generation, Some(OperationId(99))),
        (generation, Some(op)),
    ] {
        let out = s.apply_event(BackendEvent {
            conversation: A,
            generation,
            op: event_op,
            kind: EventKind::ExecutionContext {
                profile: ExecutionProfile::NativeAgent,
                execution_blocked: None,
            },
        });
        assert!(out.effects.is_empty() && out.notes.is_empty());
        let c = s.current().unwrap();
        assert_eq!(c.profile, ExecutionProfile::LocalCliChatOnly);
        assert_eq!(c.execution_blocked.as_deref(), Some("not ready"));
        assert_eq!(c.run, RunState::Running { op });
    }
    context(&mut s, ExecutionProfile::LocalCliChatOnly, false);
    assert_eq!(
        s.current().unwrap().profile,
        ExecutionProfile::LocalCliChatOnly
    );
    assert_eq!(s.current().unwrap().run, RunState::Running { op });
    assert!(!s.availability().queue);
}

#[test]
fn restricted_snapshots_cannot_trigger_native_background_reads() {
    for profile in [
        ExecutionProfile::LocalCliChatOnly,
        ExecutionProfile::LocalCliRestrictedWrite,
        ExecutionProfile::Unsupported,
    ] {
        let mut s = state();
        context(&mut s, profile, false);
        let c = s.conversations.iter_mut().find(|c| c.id == A).unwrap();
        c.subagents.selected = Some(1);
        c.jobs.expanded = Some(1);
        no_execution(&event(
            &mut s,
            None,
            EventKind::SubagentsSynced {
                enabled: true,
                children: vec![SubagentInfo {
                    id: 1,
                    ..Default::default()
                }],
            },
        ));
        no_execution(&event(
            &mut s,
            None,
            EventKind::JobsSynced {
                jobs: JobsSync {
                    jobs: vec![Job {
                        id: 1,
                        name: None,
                        command: "job".into(),
                        cwd: "/project".into(),
                        origin: JobOrigin::User,
                        state: JobState::Running,
                        exit_code: None,
                        reason: None,
                        call_id: None,
                        task: None,
                        wake: true,
                        delivery: "pending".into(),
                        log_bytes: 10,
                        log_truncated: false,
                        created_at_ms: 0,
                        started_at_ms: Some(0),
                        ended_at_ms: None,
                    }],
                    active: 1,
                    truncated: false,
                },
            },
        ));
        assert!(!s.current().unwrap().subagents.loading);
        assert!(s.current().unwrap().jobs.pending_log.is_none());
    }
}

#[test]
fn failed_delayed_native_queue_intent_cannot_recreate_a_cli_queue() {
    let mut s = state();
    let request = request(s.dispatch(Command::Submit));
    s.conversations
        .iter_mut()
        .find(|c| c.id == A)
        .unwrap()
        .pending_intent
        .as_mut()
        .unwrap()
        .origin = IntentOrigin::Queue;
    context(&mut s, ExecutionProfile::LocalCliChatOnly, false);
    no_execution(&s.intent_persisted(A, &request, Err("journal unavailable".into())));
    assert!(s.current().unwrap().queue.is_empty());
    assert_eq!(s.current().unwrap().draft.text, "explicit text");
}
