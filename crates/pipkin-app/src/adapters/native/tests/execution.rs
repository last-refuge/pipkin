use super::*;
use crate::adapters::native::execution::{preserve_identity, prompt};
use pipkin_core::{ExecutionProfile, GoalControlAction, QueueId, QueueMode};

fn external_summary(backend: ExecutionBackend, blocked: Option<&str>) -> SessionSummary {
    let mut s = summary("s1");
    s.execution_backend = backend;
    s.local_cli_mode =
        (backend == ExecutionBackend::LocalClaudeCli).then_some(LocalCliMode::ChatOnly);
    s.blocked = blocked.map(str::to_owned);
    s
}

#[test]
fn cli_mode_changes_are_not_upgrades_and_unknown_mode_cannot_prompt() {
    for before in [
        None,
        Some(LocalCliMode::ChatOnly),
        Some(LocalCliMode::RestrictedWrite),
        Some(LocalCliMode::Unsupported),
    ] {
        for after in [
            None,
            Some(LocalCliMode::ChatOnly),
            Some(LocalCliMode::RestrictedWrite),
            Some(LocalCliMode::Unsupported),
        ] {
            let mut previous = external_summary(ExecutionBackend::LocalClaudeCli, None);
            previous.local_cli_mode = before;
            let mut next = previous.clone();
            next.local_cli_mode = after;
            preserve_identity(&previous, &mut next, false);
            if before != after {
                assert_eq!(next.execution_backend, ExecutionBackend::Unsupported);
                assert!(next.blocked.is_some());
                assert!(prompt(&next, false, false).is_err());
            } else if matches!(after, None | Some(LocalCliMode::Unsupported)) {
                assert!(prompt(&next, false, false).is_err());
            }
        }
    }
}

fn external_run(fake: &Fake, backend: ExecutionBackend, blocked: Option<&'static str>) -> Run {
    let mut run = Run::start(fake.host(move |call| match call {
        Call::ListSessions => Some(Some(Ok(Reply::Sessions {
            sessions: vec![external_summary(backend, blocked)],
        }))),
        Call::OpenSession { session, .. } => {
            let mut view = snapshot(session, vec![]);
            view.summary = external_summary(backend, blocked);
            Some(Some(Ok(Reply::Snapshot(Box::new(view)))))
        }
        Call::Prompt { .. } => Some(Some(Ok(Reply::Admitted {
            run: Some(1),
            entry: None,
        }))),
        Call::Stop { .. } => Some(Some(Ok(Reply::Stopping { run: Some(1) }))),
        _ => None,
    }));
    run.ready();
    run.backend.request(BackendRequest::Open {
        conversation: run.conversation(),
        generation: 1,
    });
    run.until("external history open", |r| {
        r.has(|k| matches!(k, EventKind::Opened { .. }))
    });
    run
}

fn barrier(run: &mut Run, op: u64) {
    run.backend.request(BackendRequest::CheckStatus {
        conversation: run.conversation(),
        generation: 1,
        op: OperationId(op),
        request: Some(RequestId(format!("receipt-{op}"))),
    });
    run.until("read-only receipt", |r| {
        r.seen.iter().any(|e| {
            e.op == Some(OperationId(op)) && matches!(e.kind, EventKind::StatusResolved { .. })
        })
    });
}

#[test]
fn ready_cli_still_refuses_native_only_backend_requests() {
    let fake = Fake::new();
    let mut run = external_run(&fake, ExecutionBackend::LocalClaudeCli, None);
    let conversation = run.conversation();
    barrier(&mut run, 100);
    let before = fake.calls().len();
    let requests = vec![
        BackendRequest::Queue {
            conversation,
            generation: 1,
            request: RequestId("queue".into()),
            mode: QueueMode::FollowUp,
            text: "no".into(),
            attachments: vec![],
        },
        BackendRequest::CancelQueued {
            conversation,
            generation: 1,
            entry: QueueId(1),
        },
        BackendRequest::SetModel {
            conversation,
            generation: 1,
            model: "p/m".into(),
        },
        BackendRequest::SetThinkingLevel {
            conversation,
            generation: 1,
            level: "high".into(),
        },
        BackendRequest::RefreshModels {
            conversation,
            generation: 1,
        },
        BackendRequest::SetGoal {
            conversation,
            generation: 1,
            request: RequestId("goal".into()),
            objective: "no".into(),
        },
        BackendRequest::GoalControl {
            conversation,
            generation: 1,
            request: Some(RequestId("resume".into())),
            action: GoalControlAction::Resume,
        },
        BackendRequest::MigrateGoal {
            conversation,
            generation: 1,
            key: "old".into(),
            text: "no".into(),
            paused: None,
        },
        BackendRequest::SetSubagentsEnabled {
            conversation,
            generation: 1,
            enabled: true,
        },
        BackendRequest::LaunchSubagent {
            conversation,
            generation: 1,
            request: RequestId("child".into()),
            task: "no".into(),
        },
        BackendRequest::CancelJob {
            conversation,
            generation: 1,
            id: 1,
        },
        BackendRequest::UiCancel {
            conversation,
            generation: 1,
            id: "question".into(),
        },
        BackendRequest::McpRefresh {
            conversation: Some(conversation),
        },
        BackendRequest::McpReconnect {
            conversation: Some(conversation),
            name: "server".into(),
        },
        BackendRequest::McpAuthStart {
            conversation: Some(conversation),
            name: "server".into(),
        },
    ];
    for request in requests {
        run.backend.request(request);
    }
    barrier(&mut run, 101);
    assert!(run.has(|k| matches!(k, EventKind::QueueRefused { .. })));
    assert!(run.has(|k| matches!(k, EventKind::GoalRefused { .. })));
    assert!(run.has(|k| matches!(k, EventKind::SubagentControlFailed { .. })));
    assert!(
        fake.calls()[before..]
            .iter()
            .all(|call| matches!(call, Call::Lookup { .. })),
        "native-only calls escaped: {:?}",
        &fake.calls()[before..]
    );
}

#[test]
fn cli_text_requires_server_readiness_and_rejects_images_and_model_overrides() {
    let fake = Fake::new();
    let mut run = external_run(&fake, ExecutionBackend::LocalClaudeCli, None);
    let conversation = run.conversation();
    for (op, model, attachments) in [
        (1, Some("override".into()), vec![]),
        (
            2,
            None,
            vec![pipkin_core::Attachment {
                path: "/must-not-read.png".into(),
                name: "image".into(),
                size: None,
                error: None,
            }],
        ),
    ] {
        let mut request = submit(conversation, op, "no");
        if let BackendRequest::Submit {
            model: m,
            attachments: a,
            ..
        } = &mut request
        {
            *m = model;
            *a = attachments;
        }
        run.backend.request(request);
    }
    barrier(&mut run, 90);
    assert_eq!(fake.count(|c| matches!(c, Call::Prompt { .. })), 0);
    assert_eq!(
        run.seen
            .iter()
            .filter(|e| matches!(e.kind, EventKind::Rejected { .. }))
            .count(),
        2
    );
    // This is only a fake server readiness grant. Production Prompt remains server-gated.
    run.backend.request(submit(conversation, 3, "text"));
    run.until("future text-only admission", |r| {
        r.has(|k| matches!(k, EventKind::Accepted))
    });
    assert_eq!(
        fake.count(|c| matches!(c, Call::Prompt { model: None, images, .. } if images.is_empty())),
        1
    );
    fake.publish(Event::Session {
        session: "s1".into(),
        revision: 5,
        change: SessionChange::SummaryChanged {
            summary: external_summary(ExecutionBackend::LocalClaudeCli, Some("server blocked")),
        },
    });
    run.until("server block", |r| r.has(|k| matches!(k, EventKind::ExecutionContext { execution_blocked: Some(reason), profile: ExecutionProfile::LocalCliChatOnly } if reason == "server blocked")));
    run.backend.request(submit(conversation, 4, "blocked"));
    barrier(&mut run, 91);
    assert_eq!(fake.count(|c| matches!(c, Call::Prompt { .. })), 1);
    // Stop remains available for the already-admitted external run after readiness is lost.
    run.backend.request(BackendRequest::Cancel {
        conversation,
        generation: 1,
        op: OperationId(3),
    });
    run.until("active CLI stop", |_| {
        fake.count(|c| matches!(c, Call::Stop { .. })) == 1
    });
}

#[test]
fn stale_or_wrong_session_summaries_cannot_clear_a_block_and_unknown_fails_closed() {
    let fake = Fake::new();
    let mut run = external_run(&fake, ExecutionBackend::LocalClaudeCli, Some("blocked"));
    let conversation = run.conversation();
    fake.publish(Event::Session {
        session: "s1".into(),
        revision: 10,
        change: SessionChange::SummaryChanged {
            summary: external_summary(ExecutionBackend::LocalClaudeCli, Some("new block")),
        },
    });
    run.until("new block", |r| r.has(|k| matches!(k, EventKind::ExecutionContext { execution_blocked: Some(reason), .. } if reason == "new block")));
    for (session, revision) in [("s1", 9), ("s1", 10), ("s1", 0), ("wrong-session", 11)] {
        fake.publish(Event::Session {
            session: session.into(),
            revision,
            change: SessionChange::SummaryChanged {
                summary: external_summary(ExecutionBackend::LocalClaudeCli, None),
            },
        });
    }
    // An ordered notice is a barrier for the event stream.
    fake.publish(Event::Notice {
        message: "summary barrier".into(),
    });
    run.until("summary barrier", |r| {
        r.lifecycle
            .iter()
            .any(|e| matches!(e, LifecycleEvent::Notice(s) if s == "summary barrier"))
    });
    run.backend.request(submit(conversation, 1, "blocked"));
    barrier(&mut run, 80);
    assert_eq!(fake.count(|c| matches!(c, Call::Prompt { .. })), 0);
    assert!(!run.has(|k| matches!(
        k,
        EventKind::ExecutionContext {
            execution_blocked: None,
            ..
        }
    )));
    run.backend.request(BackendRequest::Open {
        conversation,
        generation: 2,
    });
    run.until("stale snapshot refused", |r| {
        r.has(|k| matches!(k, EventKind::OpenFailed { message } if message.contains("stale")))
    });
    assert!(!run.has(|k| matches!(
        k,
        EventKind::ExecutionContext {
            execution_blocked: None,
            ..
        }
    )));
    // Immutable identity changes cannot grant native execution.
    fake.publish(Event::Session {
        session: "s1".into(),
        revision: 12,
        change: SessionChange::SummaryChanged {
            summary: summary("s1"),
        },
    });
    run.until("unsupported identity", |r| {
        r.has(|k| {
            matches!(
                k,
                EventKind::ExecutionContext {
                    profile: ExecutionProfile::Unsupported,
                    execution_blocked: Some(_),
                    ..
                }
            )
        })
    });
    run.backend.request(submit(conversation, 2, "unknown"));
    run.backend.request(BackendRequest::Cancel {
        conversation,
        generation: 1,
        op: OperationId(2),
    });
    run.until("unknown stop refusal", |r| {
        r.has(|k| matches!(k, EventKind::StopFailed { .. }))
    });
    barrier(&mut run, 81);
    assert_eq!(
        fake.count(|c| matches!(c, Call::Prompt { .. } | Call::Stop { .. })),
        0
    );
}

#[test]
fn blocked_cli_stop_requires_committed_activity_and_ignores_stale_activity() {
    let fake = Fake::new();
    let mut run = external_run(&fake, ExecutionBackend::LocalClaudeCli, Some("blocked"));
    let conversation = run.conversation();
    run.backend.request(BackendRequest::Cancel {
        conversation,
        generation: 1,
        op: OperationId(1),
    });
    run.until("idle CLI stop refused", |r| {
        r.has(|k| matches!(k, EventKind::StopFailed { .. }))
    });
    assert_eq!(fake.count(|c| matches!(c, Call::Stop { .. })), 0);
    fake.publish(Event::Session {
        session: "s1".into(),
        revision: 5,
        change: SessionChange::RunChanged {
            run: Some(RunView {
                id: 1,
                request: Some("external".into()),
                phase: RunPhase::Running,
                started_at_ms: 0,
            }),
        },
    });
    fake.publish(Event::Session {
        session: "s1".into(),
        revision: 4,
        change: SessionChange::RunChanged { run: None },
    });
    fake.publish(Event::Notice {
        message: "activity barrier".into(),
    });
    run.until("activity barrier", |r| {
        r.lifecycle
            .iter()
            .any(|e| matches!(e, LifecycleEvent::Notice(s) if s == "activity barrier"))
    });
    run.backend.request(BackendRequest::Cancel {
        conversation,
        generation: 1,
        op: OperationId(2),
    });
    run.until("active CLI stop allowed", |_| {
        fake.count(|c| matches!(c, Call::Stop { .. })) == 1
    });
}

#[test]
fn call_guard_keeps_compaction_and_all_native_surfaces_off_but_receipts_readable() {
    let cli = external_summary(ExecutionBackend::LocalClaudeCli, None);
    let session = "s1".to_owned();
    let calls = vec![
        Call::Compact {
            session: session.clone(),
            instructions: None,
        },
        Call::CycleThinking {
            session: session.clone(),
        },
        Call::PrepareExtensions {
            session: Some(session.clone()),
            extensions: Some(vec![]),
        },
        Call::UpdatePlan {
            session: session.clone(),
            request: "plan".into(),
            ops: vec![],
            expected_revision: None,
        },
        Call::JobStart {
            session: session.clone(),
            request: "job".into(),
            command: "no".into(),
            name: None,
            cwd: None,
            timeout_secs: None,
        },
        Call::StopSubagent {
            session: session.clone(),
            child: 1,
            request: "stop".into(),
        },
        Call::SteerSubagent {
            session: session.clone(),
            child: 1,
            request: "steer".into(),
            text: "no".into(),
        },
        Call::McpAuthAnswer {
            name: "server".into(),
            prompt: "question".into(),
            response: Secret("no".into()),
        },
    ];
    for call in calls {
        assert!(
            super::super::execution::guard(&call, Some(&session), |_| Some(&cli)).is_err(),
            "{call:?}"
        );
        assert!(super::super::execution::guard(&call, Some(&session), |_| None).is_err());
        let native = summary("s1");
        assert!(super::super::execution::guard(&call, Some(&session), |_| Some(&native)).is_ok());
    }
    for call in [
        Call::Lookup {
            session: session.clone(),
            request: "external".into(),
        },
        Call::WaitForPrompt {
            session: session.clone(),
            request: "external".into(),
        },
        Call::HistoryPage {
            session,
            before: None,
            limit: 50,
        },
    ] {
        assert!(super::super::execution::guard(&call, None, |_| None).is_ok());
    }
    let unknown = external_summary(ExecutionBackend::Unsupported, None);
    assert!(super::super::execution::prompt(&unknown, false, false).is_err());
    assert!(!super::super::execution::stop(&unknown, true));
    let blocked = external_summary(ExecutionBackend::LocalClaudeCli, Some("blocked"));
    assert!(!super::super::execution::stop(&blocked, false));
    assert!(super::super::execution::stop(&blocked, true));
}
