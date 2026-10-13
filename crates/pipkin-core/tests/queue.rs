//! M3: the engine owns the queue and the run; steer and follow-up are journaled like prompts.

use pipkin_core::*;

fn real() -> AppState {
    let boot = Bootstrap {
        projects: vec![Project {
            id: ProjectId(1),
            name: "p".into(),
            path: "/p".into(),
        }],
        models: vec![ModelInfo {
            id: "a/m".into(),
            name: "M".into(),
            note: String::new(),
        }],
        conversations: vec![(ConversationId(1), ProjectId(1), "one".into(), 10)],
        now: 100,
    };
    let mut s = AppState::new(boot, Prefs::default());
    s.mode = Mode::Real;
    s.prefs.selected_project = Some(ProjectId(1));
    s.dispatch(Command::SelectConversation(ConversationId(1)));
    let opened = event(
        &s,
        EventKind::Opened {
            items: vec![],
            has_older: false,
            changes: vec![],
        },
    );
    s.apply_event(opened);
    s
}

fn event(s: &AppState, kind: EventKind) -> BackendEvent {
    let c = s.current().unwrap();
    BackendEvent {
        conversation: c.id,
        generation: c.generation,
        op: c.run.op(),
        kind,
    }
}

fn commit(s: &mut AppState, mut out: Outcome) -> Outcome {
    let intents: Vec<_> = out
        .effects
        .iter()
        .filter_map(|e| match e {
            Effect::JournalIntent {
                conversation,
                request,
                ..
            } => Some((*conversation, request.clone())),
            _ => None,
        })
        .collect();
    for (conversation, request) in intents {
        out.merge(s.intent_persisted(conversation, &request, Ok(())));
    }
    out
}

fn send(s: &mut AppState, command: Command) -> Outcome {
    let out = s.dispatch(command);
    commit(s, out)
}

fn apply(s: &mut AppState, kind: EventKind) -> Outcome {
    let e = event(s, kind);
    let out = s.apply_event(e);
    commit(s, out)
}

/// A prompt in flight and accepted: the run is `Running`.
fn running(s: &mut AppState) {
    s.dispatch(Command::EditDraft("start".into()));
    send(s, Command::Submit);
    apply(s, EventKind::Accepted);
    assert!(matches!(s.current().unwrap().run, RunState::Running { .. }));
}

fn queue_request(out: &Outcome) -> Option<&BackendRequest> {
    out.effects.iter().find_map(|e| match e {
        Effect::Backend(r @ BackendRequest::Queue { .. }) => Some(r),
        _ => None,
    })
}

fn journal_states(out: &Outcome) -> Vec<JournalState> {
    out.effects
        .iter()
        .filter_map(|e| match e {
            Effect::JournalState { state, .. } => Some(*state),
            _ => None,
        })
        .collect()
}

#[test]
fn a_steer_is_journaled_before_it_is_sent_and_leaves_the_run_alone() {
    let mut s = real();
    running(&mut s);
    s.dispatch(Command::EditDraft("use tabs".into()));
    assert!(s.availability().steer);

    // Nothing is sent until the journal commits.
    let out = s.dispatch(Command::Steer);
    assert!(queue_request(&out).is_none());
    assert!(
        out.effects
            .iter()
            .any(|e| matches!(e, Effect::JournalIntent { text, .. } if text == "use tabs"))
    );
    assert_eq!(s.current().unwrap().draft.text, "use tabs");

    let out = commit(&mut s, out);
    let Some(BackendRequest::Queue {
        mode,
        text,
        request,
        attachments,
        ..
    }) = queue_request(&out)
    else {
        panic!("the commit sends the request");
    };
    assert_eq!(
        (*mode, text.as_str(), attachments.len()),
        (QueueMode::Steer, "use tabs", 0)
    );
    let c = s.current().unwrap();
    assert!(matches!(c.run, RunState::Running { .. }), "{:?}", c.run);
    assert_eq!(c.draft.text, "");
    assert_eq!(c.pending_queue.len(), 1);
    assert_eq!(c.pending_queue[0].request, *request);
    assert_eq!(c.pending_queue[0].state, QueueSend::Sending);
    assert!(
        !c.items
            .iter()
            .any(|i| matches!(&i.kind, ItemKind::User { text, .. } if text == "use tabs")),
        "the engine shows it when it places it"
    );
}

#[test]
fn a_follow_up_that_cannot_be_journaled_is_not_sent_and_keeps_its_text() {
    let mut s = real();
    running(&mut s);
    s.dispatch(Command::EditDraft("then ship".into()));
    let out = s.dispatch(Command::QueueFollowUp);
    let (conversation, request) = out
        .effects
        .iter()
        .find_map(|e| match e {
            Effect::JournalIntent {
                conversation,
                request,
                ..
            } => Some((*conversation, request.clone())),
            _ => None,
        })
        .unwrap();
    let out = s.intent_persisted(conversation, &request, Err("disk full".into()));
    assert!(queue_request(&out).is_none());
    let c = s.current().unwrap();
    assert_eq!(c.draft.text, "then ship");
    assert!(c.pending_queue.is_empty());
    assert!(c.intent_error.as_deref().unwrap().contains("disk full"));
}

#[test]
fn only_one_steer_or_follow_up_is_in_the_journal_at_a_time() {
    let mut s = real();
    running(&mut s);
    s.dispatch(Command::EditDraft("one".into()));
    s.dispatch(Command::QueueFollowUp);
    s.dispatch(Command::EditDraft("two".into()));
    assert!(!s.availability().queue && !s.availability().steer);
}

#[test]
fn the_engines_admission_moves_a_request_into_the_queue_and_finishes_its_journal() {
    let mut s = real();
    running(&mut s);
    s.dispatch(Command::EditDraft("later".into()));
    let out = send(&mut s, Command::QueueFollowUp);
    let Some(BackendRequest::Queue { request, .. }) = queue_request(&out).cloned() else {
        panic!()
    };

    let out = apply(
        &mut s,
        EventKind::QueueAdmitted {
            request: request.clone(),
            entry: QueueId(41),
        },
    );
    assert_eq!(journal_states(&out), [JournalState::Queued]);
    assert!(JournalState::Queued.is_terminal());
    let c = s.current().unwrap();
    assert!(c.pending_queue.is_empty());
    assert_eq!(
        c.queue,
        [QueuedPrompt {
            id: QueueId(41),
            text: "later".into(),
            mode: QueueMode::FollowUp
        }]
    );

    // The engine's own report agrees, and stays the authority.
    let queue = c.queue.clone();
    apply(&mut s, EventKind::EngineState { busy: true, queue });
    assert_eq!(s.current().unwrap().queue.len(), 1);
    // A duplicate admission changes nothing.
    let out = apply(
        &mut s,
        EventKind::QueueAdmitted {
            request,
            entry: QueueId(41),
        },
    );
    assert!(journal_states(&out).is_empty());
    assert_eq!(s.current().unwrap().queue.len(), 1);
}

#[test]
fn a_refused_request_returns_its_text_to_an_empty_draft_and_says_why() {
    let mut s = real();
    running(&mut s);
    s.dispatch(Command::EditDraft("nope".into()));
    let out = send(&mut s, Command::Steer);
    let Some(BackendRequest::Queue { request, .. }) = queue_request(&out).cloned() else {
        panic!()
    };
    let out = apply(
        &mut s,
        EventKind::QueueRefused {
            request,
            reason: "The session is not open on the engine server.".into(),
        },
    );
    assert_eq!(journal_states(&out), [JournalState::Rejected]);
    let c = s.current().unwrap();
    assert!(c.pending_queue.is_empty() && c.queue.is_empty());
    assert_eq!(c.draft.text, "nope");
    assert!(
        c.intent_error
            .as_deref()
            .unwrap()
            .starts_with("Not queued: ")
    );

    // Text typed in the meantime is not overwritten.
    s.dispatch(Command::EditDraft("second try".into()));
    let out = send(&mut s, Command::Steer);
    let Some(BackendRequest::Queue { request, .. }) = queue_request(&out).cloned() else {
        panic!()
    };
    s.dispatch(Command::EditDraft("a newer thought".into()));
    apply(
        &mut s,
        EventKind::QueueRefused {
            request,
            reason: "no".into(),
        },
    );
    assert_eq!(s.current().unwrap().draft.text, "a newer thought");
}

#[test]
fn a_lost_acknowledgment_is_unknown_not_refused_and_later_resolves() {
    let mut s = real();
    running(&mut s);
    s.dispatch(Command::EditDraft("maybe".into()));
    let out = send(&mut s, Command::QueueFollowUp);
    let Some(BackendRequest::Queue { request, .. }) = queue_request(&out).cloned() else {
        panic!()
    };
    let out = apply(
        &mut s,
        EventKind::QueueAckLost {
            request: request.clone(),
        },
    );
    assert_eq!(journal_states(&out), [JournalState::Unknown]);
    let c = s.current().unwrap();
    assert_eq!(c.pending_queue[0].state, QueueSend::Unknown);
    assert_eq!(
        c.draft.text, "",
        "the text stays pending, it is not handed back"
    );

    apply(
        &mut s,
        EventKind::QueueAdmitted {
            request,
            entry: QueueId(7),
        },
    );
    let c = s.current().unwrap();
    assert!(c.pending_queue.is_empty());
    assert_eq!(c.queue[0].id, QueueId(7));
}

#[test]
fn removing_a_queued_input_asks_the_engine_and_waits_for_its_answer() {
    let mut s = real();
    running(&mut s);
    apply(
        &mut s,
        EventKind::EngineState {
            busy: true,
            queue: vec![
                QueuedPrompt {
                    id: QueueId(5),
                    text: "a".into(),
                    mode: QueueMode::FollowUp,
                },
                QueuedPrompt {
                    id: QueueId(6),
                    text: "b".into(),
                    mode: QueueMode::Steer,
                },
            ],
        },
    );
    let out = s.dispatch(Command::RemoveQueued(QueueId(5)));
    assert!(out.effects.iter().any(|e| matches!(
        e,
        Effect::Backend(BackendRequest::CancelQueued { entry, .. }) if *entry == QueueId(5)
    )));
    assert_eq!(
        s.current().unwrap().queue.len(),
        2,
        "not removed until the engine says"
    );

    apply(
        &mut s,
        EventKind::QueueCancelled {
            entry: QueueId(5),
            outcome: CancelOutcome::Cancelled,
        },
    );
    assert_eq!(s.current().unwrap().queue.len(), 1);

    // One that already started cannot be removed, and the user is told.
    apply(
        &mut s,
        EventKind::QueueCancelled {
            entry: QueueId(6),
            outcome: CancelOutcome::AlreadyConsumed,
        },
    );
    let c = s.current().unwrap();
    assert_eq!(c.queue.len(), 1);
    assert!(
        c.intent_error
            .as_deref()
            .unwrap()
            .contains("already started")
    );

    // An unknown id asks nothing.
    let out = s.dispatch(Command::RemoveQueued(QueueId(99)));
    assert!(out.effects.is_empty());
}

#[test]
fn stopping_withdraws_the_queue_through_the_engines_report() {
    let mut s = real();
    running(&mut s);
    apply(
        &mut s,
        EventKind::EngineState {
            busy: true,
            queue: vec![QueuedPrompt {
                id: QueueId(5),
                text: "a".into(),
                mode: QueueMode::FollowUp,
            }],
        },
    );
    let out = s.dispatch(Command::Cancel);
    assert!(
        out.effects
            .iter()
            .any(|e| matches!(e, Effect::Backend(BackendRequest::Cancel { .. })))
    );
    assert!(matches!(
        s.current().unwrap().run,
        RunState::Stopping { .. }
    ));
    assert_eq!(
        s.current().unwrap().queue.len(),
        1,
        "still there until the engine reports"
    );
    apply(&mut s, EventKind::Cancelled);
    apply(
        &mut s,
        EventKind::EngineState {
            busy: false,
            queue: vec![],
        },
    );
    let c = s.current().unwrap();
    assert_eq!(c.run, RunState::Idle);
    assert!(c.queue.is_empty());
}

#[test]
fn a_run_the_engine_reports_is_shown_and_only_the_engine_ends_it() {
    let mut s = real();
    assert_eq!(s.current().unwrap().run, RunState::Idle);
    apply(
        &mut s,
        EventKind::EngineState {
            busy: true,
            queue: vec![],
        },
    );
    let c = s.current().unwrap();
    assert!(matches!(c.run, RunState::Running { .. }));
    assert!(s.availability().cancel, "a run found busy can be stopped");

    // A stop on an adopted run settles from the engine's report.
    s.dispatch(Command::Cancel);
    assert!(matches!(
        s.current().unwrap().run,
        RunState::Stopping { .. }
    ));
    apply(
        &mut s,
        EventKind::EngineState {
            busy: true,
            queue: vec![],
        },
    );
    assert!(matches!(
        s.current().unwrap().run,
        RunState::Stopping { .. }
    ));
    apply(
        &mut s,
        EventKind::EngineState {
            busy: false,
            queue: vec![],
        },
    );
    assert_eq!(s.current().unwrap().run, RunState::Idle);
}

#[test]
fn an_idle_report_does_not_end_a_run_this_window_started() {
    let mut s = real();
    running(&mut s);
    // The prompt is accepted but the engine's view has not caught up (or a stale view arrives).
    apply(
        &mut s,
        EventKind::EngineState {
            busy: false,
            queue: vec![],
        },
    );
    assert!(matches!(s.current().unwrap().run, RunState::Running { .. }));
    // Its own settlement still ends it.
    apply(&mut s, EventKind::Completed);
    assert_eq!(s.current().unwrap().run, RunState::Idle);
}

#[test]
fn a_failed_run_is_not_replaced_by_an_engine_report() {
    let mut s = real();
    running(&mut s);
    apply(
        &mut s,
        EventKind::Failed {
            message: "model error".into(),
        },
    );
    apply(
        &mut s,
        EventKind::EngineState {
            busy: true,
            queue: vec![],
        },
    );
    assert!(matches!(s.current().unwrap().run, RunState::Failed { .. }));
}

#[test]
fn engine_state_reports_are_ignored_by_the_demo() {
    let mut s = real();
    s.mode = Mode::Demo;
    apply(
        &mut s,
        EventKind::EngineState {
            busy: true,
            queue: vec![QueuedPrompt {
                id: QueueId(1),
                text: "x".into(),
                mode: QueueMode::FollowUp,
            }],
        },
    );
    let c = s.current().unwrap();
    assert_eq!(c.run, RunState::Idle);
    assert!(c.queue.is_empty());
}

#[test]
fn a_stale_engine_report_from_another_attachment_is_dropped() {
    let mut s = real();
    let mut e = event(
        &s,
        EventKind::EngineState {
            busy: true,
            queue: vec![],
        },
    );
    e.generation += 1;
    s.apply_event(e);
    assert_eq!(s.current().unwrap().run, RunState::Idle);
}

#[test]
fn a_submission_in_doubt_is_checked_on_opening_and_on_reconnecting() {
    // Restored from the journal before the conversation was opened.
    let mut s = real();
    s.apply_event(event(
        &s,
        EventKind::Opened {
            items: vec![],
            has_older: false,
            changes: vec![],
        },
    ));
    s.restore_unresolved(
        ConversationId(1),
        RequestId("req-9".into()),
        "unsure".into(),
        vec![],
    );
    assert!(matches!(
        s.current().unwrap().run,
        RunState::OutcomeUnknown { .. }
    ));
    let out = apply(
        &mut s,
        EventKind::Opened {
            items: vec![],
            has_older: false,
            changes: vec![],
        },
    );
    assert!(out.effects.iter().any(|e| matches!(
        e,
        Effect::Backend(BackendRequest::CheckStatus { request: Some(r), .. }) if r.0 == "req-9"
    )));

    // A reconnect asks again; nothing is ever resent.
    s.set_connection(Connection::Reconnecting);
    let out = s.set_connection(Connection::Ready);
    assert!(
        out.effects
            .iter()
            .any(|e| matches!(e, Effect::Backend(BackendRequest::CheckStatus { .. })))
    );
    assert!(
        !out.effects
            .iter()
            .any(|e| matches!(e, Effect::Backend(BackendRequest::Submit { .. })))
    );

    // With nothing in doubt, reconnecting asks nothing.
    let mut quiet = real();
    quiet.set_connection(Connection::Reconnecting);
    assert!(quiet.set_connection(Connection::Ready).effects.is_empty());
}

#[test]
fn refreshing_models_needs_an_open_conversation_in_real_mode() {
    let mut s = real();
    assert!(s.availability().refresh_models);
    let out = s.dispatch(Command::RefreshModels);
    assert!(
        out.effects
            .iter()
            .any(|e| matches!(e, Effect::Backend(BackendRequest::RefreshModels { .. })))
    );
    s.mode = Mode::Demo;
    assert!(!s.availability().refresh_models);
    assert!(s.dispatch(Command::RefreshModels).effects.is_empty());
}

#[test]
fn a_failed_draft_save_can_be_retried_and_carries_its_attachments() {
    let mut s = real();
    let file = Attachment {
        path: "/p/a.txt".into(),
        name: "a.txt".into(),
        size: Some(3),
        error: None,
    };
    s.dispatch(Command::AddAttachments(vec![file.clone()]));
    s.dispatch(Command::EditDraft("hi".into()));
    let out = s.dispatch(Command::FlushDraft(ConversationId(1)));
    let Effect::SaveDraft {
        attachments, rev, ..
    } = &out.effects[0]
    else {
        panic!()
    };
    assert_eq!(attachments, std::slice::from_ref(&file));
    s.draft_saved(ConversationId(1), *rev, Err("disk full".into()));
    assert!(s.availability().retry_save);

    let out = s.dispatch(Command::RetrySave);
    assert!(
        out.effects
            .iter()
            .any(|e| matches!(e, Effect::SaveDraft { attachments, .. } if attachments.len() == 1))
    );
    assert!(!s.availability().retry_save);
    // Nothing to retry once saved.
    assert!(s.dispatch(Command::RetrySave).effects.is_empty());
}

#[test]
fn removing_an_attachment_marks_the_draft_for_saving() {
    let mut s = real();
    let file = Attachment {
        path: "/p/a.txt".into(),
        name: "a.txt".into(),
        size: Some(3),
        error: None,
    };
    s.dispatch(Command::AddAttachments(vec![file]));
    let out = s.dispatch(Command::FlushDraft(ConversationId(1)));
    let Effect::SaveDraft { rev, .. } = out.effects[0] else {
        panic!()
    };
    s.draft_saved(ConversationId(1), rev, Ok(()));
    assert_eq!(s.current().unwrap().draft.save, SaveState::Saved);
    s.dispatch(Command::RemoveAttachment(0));
    assert_eq!(s.current().unwrap().draft.save, SaveState::Dirty);
    let out = s.dispatch(Command::FlushDraft(ConversationId(1)));
    assert!(
        out.effects
            .iter()
            .any(|e| matches!(e, Effect::SaveDraft { attachments, .. } if attachments.is_empty()))
    );
}

#[test]
fn a_restored_draft_brings_back_its_attachments_but_never_overwrites_typing() {
    let mut s = real();
    let file = Attachment {
        path: "/p/a.txt".into(),
        name: "a.txt".into(),
        size: Some(3),
        error: Some("File not found".into()),
    };
    s.restore_draft(ConversationId(1), "text".into(), vec![file.clone()]);
    let c = s.current().unwrap();
    assert_eq!(c.draft.text, "text");
    assert_eq!(c.draft.attachments, std::slice::from_ref(&file));
    assert!(
        !s.availability().submit,
        "a flagged attachment blocks sending"
    );

    s.dispatch(Command::RemoveAttachment(0));
    s.dispatch(Command::EditDraft("typed".into()));
    s.restore_draft(ConversationId(1), "old".into(), vec![file]);
    assert_eq!(s.current().unwrap().draft.text, "typed");
    assert!(s.current().unwrap().draft.attachments.is_empty());
}

#[test]
fn the_saved_state_notice_is_kept_until_dismissed() {
    let mut s = real();
    s.set_storage_issue("could not save".into());
    assert_eq!(s.storage_issue.as_deref(), Some("could not save"));
    s.dispatch(Command::DismissStorageIssue);
    assert!(s.storage_issue.is_none());
}

#[test]
fn coming_back_to_a_conversation_attaches_it_again_in_real_mode_only() {
    let boot = Bootstrap {
        projects: vec![Project {
            id: ProjectId(1),
            name: "p".into(),
            path: "/p".into(),
        }],
        models: vec![],
        conversations: vec![
            (ConversationId(1), ProjectId(1), "one".into(), 10),
            (ConversationId(2), ProjectId(1), "two".into(), 5),
        ],
        now: 100,
    };
    for (mode, reattaches) in [(Mode::Real, true), (Mode::Demo, false)] {
        let mut s = AppState::new(boot.clone(), Prefs::default());
        s.mode = mode;
        s.prefs.selected_project = Some(ProjectId(1));
        let opens = |out: &Outcome| {
            out.effects
                .iter()
                .filter(|e| matches!(e, Effect::Backend(BackendRequest::Open { .. })))
                .count()
        };
        for id in [1, 2] {
            let out = s.dispatch(Command::SelectConversation(ConversationId(id)));
            assert_eq!(opens(&out), 1);
            let e = BackendEvent {
                conversation: ConversationId(id),
                generation: s.current().unwrap().generation,
                op: None,
                kind: EventKind::Opened {
                    items: vec![],
                    has_older: false,
                    changes: vec![],
                },
            };
            s.apply_event(e);
        }
        let before = s.conversation(ConversationId(1)).unwrap().generation;
        let out = s.dispatch(Command::SelectConversation(ConversationId(1)));
        assert_eq!(opens(&out), usize::from(reattaches), "{mode:?}");
        let after = s.conversation(ConversationId(1)).unwrap().generation;
        assert_eq!(after, before + u64::from(reattaches));
        // Selecting what is already selected attaches nothing.
        let out = s.dispatch(Command::SelectConversation(ConversationId(1)));
        assert_eq!(opens(&out), 0);
    }
}

#[test]
fn a_settlement_survives_a_reattach_but_stale_content_does_not() {
    let mut s = real();
    running(&mut s);
    let op = s.current().unwrap().run.op().unwrap();
    // The conversation is attached again (the generation moves on) while the run is still going.
    s.dispatch(Command::SelectConversation(ConversationId(1)));
    let old = s.current().unwrap().generation;
    s.conversations
        .iter_mut()
        .find(|c| c.id == ConversationId(1))
        .unwrap()
        .generation = old + 1;

    // Stale content is refused...
    s.apply_event(BackendEvent {
        conversation: ConversationId(1),
        generation: old,
        op: None,
        kind: EventKind::EngineState {
            busy: true,
            queue: vec![QueuedPrompt {
                id: QueueId(1),
                text: "stale".into(),
                mode: QueueMode::FollowUp,
            }],
        },
    });
    assert!(s.current().unwrap().queue.is_empty());
    // ...but what became of the run, which the operation identifies, is not.
    s.apply_event(BackendEvent {
        conversation: ConversationId(1),
        generation: old,
        op: Some(op),
        kind: EventKind::Completed,
    });
    assert_eq!(s.current().unwrap().run, RunState::Idle);
}

#[test]
fn a_real_run_completing_never_starts_what_the_engine_still_lists_as_queued() {
    let mut s = real();
    running(&mut s);
    // The engine's report of its queue can still show an input at the moment the prompt's
    // completion arrives; the engine will place it itself.
    apply(
        &mut s,
        EventKind::EngineState {
            busy: true,
            queue: vec![QueuedPrompt {
                id: QueueId(5),
                text: "later".into(),
                mode: QueueMode::FollowUp,
            }],
        },
    );
    let out = apply(&mut s, EventKind::Completed);
    assert!(
        !out.effects.iter().any(|e| matches!(
            e,
            Effect::JournalIntent { .. } | Effect::Backend(BackendRequest::Submit { .. })
        )),
        "nothing is submitted a second time: {:?}",
        out.effects
    );
    let c = s.current().unwrap();
    assert!(c.pending_intent.is_none());
    assert_eq!(c.queue.len(), 1, "the engine's queue is left to the engine");
}
