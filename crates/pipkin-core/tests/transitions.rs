use pipkin_core::*;
mod support;

fn state() -> AppState {
    let boot = Bootstrap {
        projects: vec![Project {
            id: ProjectId(1),
            name: "demo".into(),
            path: "/demo".into(),
        }],
        models: vec![
            ModelInfo {
                id: "a".into(),
                name: "A".into(),
                note: String::new(),
            },
            ModelInfo {
                id: "b".into(),
                name: "B".into(),
                note: String::new(),
            },
        ],
        conversations: vec![
            (ConversationId(1), ProjectId(1), "one".into(), 10),
            (ConversationId(2), ProjectId(1), "two".into(), 5),
        ],
        now: 100,
    };
    let mut s = AppState::new(boot, Prefs::default());
    s.prefs.selected_project = Some(ProjectId(1));
    s.dispatch(Command::SelectConversation(ConversationId(1)));
    support::native_context(&mut s);
    s
}

fn ev(s: &AppState, kind: EventKind) -> BackendEvent {
    let c = s.current().unwrap();
    BackendEvent {
        conversation: c.id,
        generation: c.generation,
        op: c.run.op(),
        kind,
    }
}

/// Commit every journal intent in `out`, as the controller does once storage acknowledges.
fn settle(s: &mut AppState, mut out: Outcome) -> Outcome {
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
    settle(s, out)
}

fn apply(s: &mut AppState, event: BackendEvent) -> Outcome {
    let out = s.apply_event(event);
    settle(s, out)
}

fn run_started(s: &mut AppState, text: &str) -> OperationId {
    s.dispatch(Command::EditDraft(text.into()));
    send(s, Command::Submit);
    let op = s.current().unwrap().run.op().unwrap();
    let e = ev(s, EventKind::Accepted);
    s.apply_event(e);
    op
}

#[test]
fn background_completion_is_unseen_until_the_conversation_is_selected() {
    let mut s = state();
    let op = run_started(&mut s, "start");
    let generation = s.current().unwrap().generation;
    s.dispatch(Command::SelectConversation(ConversationId(2)));
    assert_eq!(
        s.conversation(ConversationId(1)).unwrap().activity(),
        Some("Working")
    );
    apply(
        &mut s,
        BackendEvent {
            conversation: ConversationId(1),
            generation,
            op: Some(op),
            kind: EventKind::Completed,
        },
    );
    assert_eq!(
        s.conversation(ConversationId(1)).unwrap().activity(),
        Some("Done!")
    );
    s.dispatch(Command::SelectConversation(ConversationId(1)));
    assert_eq!(s.conversation(ConversationId(1)).unwrap().activity(), None);
    let op = run_started(&mut s, "again");
    let generation = s.current().unwrap().generation;
    apply(
        &mut s,
        BackendEvent {
            conversation: ConversationId(1),
            generation,
            op: Some(op),
            kind: EventKind::Completed,
        },
    );
    assert_eq!(s.conversation(ConversationId(1)).unwrap().activity(), None);
}

#[test]
fn effort_is_per_conversation_engine_authoritative_and_generation_guarded() {
    let mut s = state();
    s.mode = Mode::Real;
    let opened = ev(
        &s,
        EventKind::Opened {
            items: vec![],
            has_older: false,
            changes: vec![],
        },
    );
    apply(&mut s, opened);
    let thinking = ev(
        &s,
        EventKind::ThinkingState {
            level: "low".into(),
            levels: vec!["off".into(), "low".into(), "high".into()],
        },
    );
    apply(&mut s, thinking);
    assert!(
        s.dispatch(Command::SetThinkingLevel("max".into()))
            .effects
            .is_empty()
    );
    let out = s.dispatch(Command::SetThinkingLevel("high".into()));
    assert!(
        matches!(&out.effects[..], [Effect::Backend(BackendRequest::SetThinkingLevel { conversation: ConversationId(1), level, .. })] if level == "high")
    );
    assert_eq!(s.current().unwrap().thinking_level.as_deref(), Some("low"));
    let confirmed = ev(
        &s,
        EventKind::ThinkingState {
            level: "high".into(),
            levels: vec!["off".into(), "low".into(), "high".into()],
        },
    );
    apply(&mut s, confirmed);
    assert_eq!(s.current().unwrap().thinking_level.as_deref(), Some("high"));

    s.dispatch(Command::SelectConversation(ConversationId(2)));
    assert!(s.current().unwrap().thinking_level.is_none());
    let b = ev(
        &s,
        EventKind::Opened {
            items: vec![],
            has_older: false,
            changes: vec![],
        },
    );
    apply(&mut s, b);
    let thinking = ev(
        &s,
        EventKind::ThinkingState {
            level: "off".into(),
            levels: vec!["off".into()],
        },
    );
    apply(&mut s, thinking);
    s.dispatch(Command::SelectConversation(ConversationId(1)));
    let old_generation = s.current().unwrap().generation - 1;
    assert!(
        s.apply_event(BackendEvent {
            conversation: ConversationId(1),
            generation: old_generation,
            op: None,
            kind: EventKind::ThinkingState {
                level: "off".into(),
                levels: vec!["off".into()]
            }
        })
        .notes
        .is_empty()
    );
    assert_eq!(s.current().unwrap().thinking_level.as_deref(), Some("high"));
}

#[test]
fn submit_moves_draft_into_transcript_and_runs() {
    let mut s = state();
    s.dispatch(Command::EditDraft("fix the test".into()));
    let out = send(&mut s, Command::Submit);
    assert!(out.effects.iter().any(|e| matches!(e, Effect::Backend(BackendRequest::Submit { text, .. }) if text == "fix the test")));
    let c = s.current().unwrap();
    assert_eq!(c.draft.text, "");
    assert!(matches!(c.run, RunState::Submitting { .. }));
    assert!(matches!(
        &c.items.last().unwrap().kind,
        ItemKind::User {
            delivery: Delivery::Pending,
            ..
        }
    ));
    let e = ev(&s, EventKind::Accepted);
    s.apply_event(e);
    assert!(matches!(s.current().unwrap().run, RunState::Running { .. }));
}

#[test]
fn cannot_submit_empty_or_twice() {
    let mut s = state();
    assert!(!s.availability().submit);
    run_started(&mut s, "go");
    s.dispatch(Command::EditDraft("again".into()));
    assert!(!s.availability().submit);
    assert!(s.availability().steer && s.availability().queue);
}

#[test]
fn stale_generation_and_op_events_are_dropped() {
    let mut s = state();
    let op = run_started(&mut s, "go");
    let (cid, cgen) = {
        let c = s.current().unwrap();
        (c.id, c.generation)
    };
    let stale_gen = BackendEvent {
        conversation: cid,
        generation: cgen + 5,
        op: Some(op),
        kind: EventKind::Token("x".into()),
    };
    s.apply_event(stale_gen);
    let wrong_op = BackendEvent {
        conversation: cid,
        generation: cgen,
        op: Some(OperationId(999)),
        kind: EventKind::Token("x".into()),
    };
    s.apply_event(wrong_op);
    assert!(
        !s.current()
            .unwrap()
            .items
            .iter()
            .any(|i| matches!(i.kind, ItemKind::Assistant { .. }))
    );
}

#[test]
fn late_event_lands_in_its_own_conversation_not_selected_one() {
    let mut s = state();
    let op = run_started(&mut s, "go");
    s.dispatch(Command::SelectConversation(ConversationId(2)));
    s.apply_event(BackendEvent {
        conversation: ConversationId(1),
        generation: 1,
        op: Some(op),
        kind: EventKind::Token("hi".into()),
    });
    assert!(s.conversation(ConversationId(2)).unwrap().items.is_empty());
    assert_eq!(s.conversation(ConversationId(1)).unwrap().items.len(), 2);
}

#[test]
fn cancel_waits_for_settlement() {
    let mut s = state();
    run_started(&mut s, "go");
    s.dispatch(Command::Cancel);
    assert!(matches!(
        s.current().unwrap().run,
        RunState::Stopping { .. }
    ));
    // Completion racing the cancel must not be treated as settled-by-us; stays until event.
    assert!(!s.availability().cancel);
    let e = ev(&s, EventKind::Cancelled);
    s.apply_event(e);
    assert_eq!(s.current().unwrap().run, RunState::Idle);
}

#[test]
fn failed_stop_can_be_retried_but_clock_and_status_checks_never_settle_it() {
    let mut s = state();
    s.apply_event(ev(
        &s,
        EventKind::Opened {
            items: vec![],
            has_older: false,
            changes: vec![],
        },
    ));
    s.mode = Mode::Real;
    run_started(&mut s, "work");
    s.dispatch(Command::Cancel);
    assert!(!s.availability().cancel && !s.availability().retry_stop);
    s.apply_event(ev(
        &s,
        EventKind::StopFailed {
            message: "abort unavailable".into(),
        },
    ));
    assert!(s.availability().retry_stop);
    assert_eq!(
        s.current().unwrap().stop_error.as_deref(),
        Some("abort unavailable")
    );
    let retried = s.dispatch(Command::RetryStop);
    assert!(matches!(
        retried.effects.as_slice(),
        [Effect::Backend(BackendRequest::Cancel { .. })]
    ));
    assert!(!s.availability().retry_stop);
    assert!(s.dispatch(Command::RetryStop).effects.is_empty());
    s.set_now(100_000);
    assert!(matches!(
        s.current().unwrap().run,
        RunState::Stopping { .. }
    ));
    let check = s.dispatch(Command::CheckStatus);
    assert!(matches!(
        check.effects.as_slice(),
        [Effect::Backend(BackendRequest::CheckStatus { .. })]
    ));
    assert!(!s.availability().check_status);
    assert!(s.dispatch(Command::CheckStatus).effects.is_empty());
    s.apply_event(ev(
        &s,
        EventKind::StatusCheckFailed {
            message: "lookup failed".into(),
        },
    ));
    assert!(s.availability().check_status);
    assert!(matches!(
        s.current().unwrap().run,
        RunState::Stopping { .. }
    ));
    assert!(s.dispatch(Command::Submit).effects.is_empty());
    assert!(s.dispatch(Command::Retry).effects.is_empty());
    s.dispatch(Command::CheckStatus);
    s.apply_event(ev(&s, EventKind::StatusResolved { accepted: true }));
    assert!(matches!(
        s.current().unwrap().run,
        RunState::Stopping { .. }
    ));
    assert!(s.current().unwrap().status_error.is_none());
    let late_error = ev(
        &s,
        EventKind::StopFailed {
            message: "late failure".into(),
        },
    );
    let settled = s.apply_event(ev(&s, EventKind::Completed));
    assert_eq!(journal_states(&settled), vec![JournalState::Completed]);
    assert_eq!(
        s.current().unwrap().run,
        RunState::Idle,
        "completion wins on the engine's word, not stop intent"
    );
    s.apply_event(late_error);
    assert!(s.current().unwrap().stop_error.is_none());
}

#[test]
fn recovery_notice_and_check_failure_are_per_conversation_and_do_not_replay_work() {
    let mut s = state();
    s.apply_event(ev(
        &s,
        EventKind::Opened {
            items: vec![],
            has_older: false,
            changes: vec![],
        },
    ));
    s.mode = Mode::Real;
    run_started(&mut s, "work");
    s.dispatch(Command::Cancel);
    s.dispatch(Command::CheckStatus);
    let settled = ev(&s, EventKind::Cancelled);
    s.set_connection(Connection::Reconnecting);
    assert!(s.current().unwrap().recovery_notice);
    assert!(!s.current().unwrap().status_check_pending);
    assert!(!s.availability().check_status && !s.availability().retry_stop);
    assert!(matches!(
        s.current().unwrap().run,
        RunState::Stopping { .. }
    ));
    let restored = s.set_connection(Connection::Ready);
    assert_eq!(backend_submits(&restored), 0);
    assert!(s.availability().check_status);
    s.dispatch(Command::SelectConversation(ConversationId(2)));
    assert!(!s.current().unwrap().recovery_notice);
    let out = s.apply_event(settled);
    assert_eq!(backend_submits(&out), 0);
    assert_eq!(
        s.conversation(ConversationId(1)).unwrap().run,
        RunState::Idle
    );
    s.dispatch(Command::SelectConversation(ConversationId(1)));
    assert!(
        s.current().unwrap().recovery_notice,
        "settlement does not erase the recovery caveat"
    );
    let dismissed = s.dispatch(Command::DismissRecoveryNotice);
    assert!(dismissed.effects.is_empty());
    assert!(!s.current().unwrap().recovery_notice);
}

#[test]
fn failed_unknown_status_checks_remain_unknown_and_disable_prompt_retry() {
    let mut s = state();
    s.mode = Mode::Real;
    s.restore_unresolved(
        ConversationId(1),
        RequestId("lost".into()),
        "work".into(),
        vec![],
    );
    s.dispatch(Command::CheckStatus);
    assert!(s.current().unwrap().status_check_pending);
    let out = s.apply_event(ev(
        &s,
        EventKind::StatusCheckFailed {
            message: "engine unavailable".into(),
        },
    ));
    assert!(out.effects.is_empty());
    assert!(matches!(
        s.current().unwrap().run,
        RunState::OutcomeUnknown { .. }
    ));
    assert!(s.availability().check_status && !s.availability().retry && !s.availability().submit);
    assert!(s.current().unwrap().recovery_notice);
    assert_eq!(
        s.current().unwrap().status_error.as_deref(),
        Some("engine unavailable")
    );
    s.dispatch(Command::CheckStatus);
    assert!(s.current().unwrap().status_error.is_none());
    s.apply_event(ev(&s, EventKind::StatusResolved { accepted: false }));
    let RunState::Failed { message } = &s.current().unwrap().run else {
        panic!()
    };
    assert!(message.contains("no record"));
    assert!(message.contains("before deciding whether to retry"));
    assert!(!message.contains("never received"));
}

#[test]
fn unknown_outcome_never_resends() {
    let mut s = state();
    s.dispatch(Command::EditDraft("risky".into()));
    send(&mut s, Command::Submit);
    let e = ev(&s, EventKind::AckLost);
    s.apply_event(e);
    assert!(matches!(
        s.current().unwrap().run,
        RunState::OutcomeUnknown { .. }
    ));
    s.dispatch(Command::EditDraft("more".into()));
    assert!(!s.availability().submit && !s.availability().retry);
    let out = s.dispatch(Command::Submit);
    assert!(out.effects.is_empty());
    let out = s.dispatch(Command::CheckStatus);
    assert!(matches!(
        out.effects[0],
        Effect::Backend(BackendRequest::CheckStatus { .. })
    ));
    let e = ev(&s, EventKind::StatusResolved { accepted: true });
    s.apply_event(e);
    assert!(matches!(s.current().unwrap().run, RunState::Running { .. }));
}

#[test]
fn rejection_retains_text_and_allows_explicit_retry() {
    let mut s = state();
    s.dispatch(Command::EditDraft("keep me".into()));
    send(&mut s, Command::Submit);
    let e = ev(
        &s,
        EventKind::Rejected {
            reason: "provider unavailable".into(),
        },
    );
    s.apply_event(e);
    let c = s.current().unwrap();
    assert_eq!(c.draft.text, "keep me");
    assert!(matches!(c.run, RunState::Failed { .. }));
    assert!(s.availability().retry);
    let out = send(&mut s, Command::Retry);
    assert!(
        out.effects
            .iter()
            .any(|e| matches!(e, Effect::Backend(BackendRequest::Submit { .. })))
    );
}

#[test]
fn queue_runs_next_after_completion_but_not_after_cancel() {
    let mut s = state();
    run_started(&mut s, "first");
    for t in ["second", "third"] {
        s.dispatch(Command::EditDraft(t.into()));
        s.dispatch(Command::QueueFollowUp);
    }
    assert_eq!(s.current().unwrap().queue.len(), 2);
    let e = ev(&s, EventKind::Completed);
    let out = apply(&mut s, e);
    assert!(out.effects.iter().any(
        |e| matches!(e, Effect::Backend(BackendRequest::Submit { text, .. }) if text == "second")
    ));
    assert_eq!(s.current().unwrap().queue.len(), 1);
    s.dispatch(Command::Cancel);
    let e = ev(&s, EventKind::Cancelled);
    let out = s.apply_event(e);
    assert!(
        !out.effects
            .iter()
            .any(|e| matches!(e, Effect::Backend(BackendRequest::Submit { .. })))
    );
    assert_eq!(s.current().unwrap().queue.len(), 1);
    let q = s.current().unwrap().queue[0].id;
    s.dispatch(Command::RemoveQueued(q));
    assert!(s.current().unwrap().queue.is_empty());
}

#[test]
fn tool_output_is_bounded() {
    let mut s = state();
    run_started(&mut s, "go");
    let e = ev(
        &s,
        EventKind::ToolStarted {
            call: 1,
            name: "bash".into(),
            input: "cargo test".into(),
            call_id: None,
        },
    );
    s.apply_event(e);
    let big = "é".repeat(20_000);
    let e = ev(
        &s,
        EventKind::ToolOutput {
            call: 1,
            chunk: big,
        },
    );
    s.apply_event(e);
    let e = ev(&s, EventKind::ToolFinished { call: 1, ok: false });
    s.apply_event(e);
    let ItemKind::Tool(t) = &s.current().unwrap().items.last().unwrap().kind else {
        panic!()
    };
    assert!(t.truncated && t.output.len() <= TOOL_OUTPUT_PREVIEW_BYTES && t.full_len == 40_000);
    assert_eq!(t.status, ToolStatus::Failed);
}

#[test]
fn drafts_are_per_conversation_and_flushed_on_switch() {
    let mut s = state();
    s.dispatch(Command::EditDraft("draft one".into()));
    let out = s.dispatch(Command::SelectConversation(ConversationId(2)));
    assert!(out.effects.iter().any(|e| matches!(e, Effect::SaveDraft { conversation, text, .. } if *conversation == ConversationId(1) && text == "draft one")));
    assert_eq!(s.current().unwrap().draft.text, "");
    s.dispatch(Command::SelectConversation(ConversationId(1)));
    assert_eq!(s.current().unwrap().draft.text, "draft one");
}

#[test]
fn save_acknowledgement_and_failure() {
    let mut s = state();
    s.dispatch(Command::EditDraft("x".into()));
    let out = s.dispatch(Command::FlushDraft(ConversationId(1)));
    let Effect::SaveDraft { rev, .. } = out.effects[0] else {
        panic!()
    };
    assert_eq!(s.current().unwrap().draft.save, SaveState::Saving);
    // Edit during the write: acknowledged rev is stale, stays Dirty.
    s.dispatch(Command::EditDraft("xy".into()));
    s.draft_saved(ConversationId(1), rev, Ok(()));
    assert_eq!(s.current().unwrap().draft.save, SaveState::Dirty);
    let out = s.dispatch(Command::FlushDraft(ConversationId(1)));
    let Effect::SaveDraft { rev, .. } = out.effects[0] else {
        panic!()
    };
    s.draft_saved(ConversationId(1), rev, Err("disk full".into()));
    let c = s.current().unwrap();
    assert!(matches!(c.draft.save, SaveState::Failed(_)));
    assert_eq!(c.draft.text, "xy");
}

#[test]
fn history_prepend_reports_count_and_shifts_streaming_index() {
    let mut s = state();
    run_started(&mut s, "go");
    let e = ev(&s, EventKind::Token("a".into()));
    s.apply_event(e);
    let page: Vec<_> = (0..5)
        .map(|i| TranscriptItem {
            native_only: false,
            id: ItemId(i),
            at: 0,
            kind: ItemKind::Notice {
                text: "old".into(),
                level: NoticeLevel::Info,
            },
        })
        .collect();
    let (cid, cgen) = {
        let c = s.current().unwrap();
        (c.id, c.generation)
    };
    let out = s.apply_event(BackendEvent {
        conversation: cid,
        generation: cgen,
        op: None,
        kind: EventKind::OlderPage {
            items: page,
            has_older: false,
        },
    });
    assert!(out.notes.contains(&Note::ItemsPrepended(cid, 5)));
    let e = ev(&s, EventKind::Token("b".into()));
    s.apply_event(e);
    let ItemKind::Assistant { text, .. } = &s.current().unwrap().items.last().unwrap().kind else {
        panic!()
    };
    assert_eq!(text, "ab");
}

#[test]
fn search_filters_and_new_conversation_selects() {
    let mut s = state();
    s.dispatch(Command::SetSearch("TWO".into()));
    assert_eq!(s.visible_conversations().len(), 1);
    s.dispatch(Command::SetSearch("zzz".into()));
    assert!(s.visible_conversations().is_empty());
    s.dispatch(Command::SetSearch(String::new()));
    s.dispatch(Command::NewConversation);
    assert_eq!(s.current().unwrap().title, "New conversation");
    s.dispatch(Command::RenameConversation(
        s.selected.unwrap(),
        "  Renamed ".into(),
    ));
    assert_eq!(s.current().unwrap().title, "Renamed");
}

#[test]
fn unsupported_real_rename_never_saves_a_local_title_across_failure_switch_and_reopen() {
    let mut s = state();
    s.mode = Mode::Real;
    for connection in [
        Connection::Ready,
        Connection::Reconnecting,
        Connection::Offline("engine unavailable".into()),
        Connection::Failed("engine failed".into()),
        Connection::Incompatible("protocol mismatch".into()),
    ] {
        s.set_connection(connection);
        for selected in [ConversationId(1), ConversationId(2)] {
            s.dispatch(Command::SelectConversation(selected));
            for target in [ConversationId(1), ConversationId(2), ConversationId(999)] {
                let out = s.dispatch(Command::RenameConversation(
                    target,
                    "local-only title".into(),
                ));
                assert!(
                    out.effects.is_empty(),
                    "must not save metadata or request a rename"
                );
                assert!(
                    out.notes.is_empty(),
                    "must not announce a successful rename"
                );
                assert_eq!(s.conversation(ConversationId(1)).unwrap().title, "one");
                assert_eq!(s.conversation(ConversationId(2)).unwrap().title, "two");
                assert_eq!(s.selected, Some(selected));
            }
        }
    }
    s.set_connection(Connection::Ready);
    s.apply_catalog(catalog());
    // Fresh core initialization from the engine catalog models restart/reopen. There was no
    // SaveConversation effect that could persist the rejected local title in the app cache.
    let mut reopened = AppState::new(catalog(), s.prefs.clone());
    reopened.mode = Mode::Real;
    reopened.select_initial();
    assert_eq!(reopened.selected, Some(ConversationId(2)));
    assert_eq!(
        reopened.conversation(ConversationId(1)).unwrap().title,
        "one"
    );
    assert_eq!(reopened.current().unwrap().title, "two");

    // These bootstrap labels are not evidence of a server rename contract. Real display titles
    // are derived from replicated prompts; the server currently has no session-title operation.
}

#[test]
fn nothing_reaches_the_backend_until_connected() {
    let mut s = state();
    s.dispatch(Command::EditDraft("hello".into()));
    assert!(s.availability().submit);
    for c in [
        Connection::Connecting,
        Connection::Reconnecting,
        Connection::Offline("x".into()),
        Connection::Incompatible("x".into()),
        Connection::Failed("x".into()),
    ] {
        s.set_connection(c);
        assert_eq!(s.availability(), Availability::default());
        assert!(s.dispatch(Command::Submit).effects.is_empty());
    }
    s.set_connection(Connection::Ready);
    assert!(s.availability().submit);
}

#[test]
fn real_mode_does_not_invent_conversations() {
    let mut s = state();
    s.mode = Mode::Real;
    let before = s.conversations.len();
    let out = s.dispatch(Command::NewConversation);
    assert!(out.effects.is_empty() && out.notes.is_empty());
    assert_eq!(s.conversations.len(), before);
}

fn empty_state(prefs: Prefs) -> AppState {
    AppState::new(
        Bootstrap {
            projects: vec![],
            models: vec![],
            conversations: vec![],
            now: 1,
        },
        prefs,
    )
}

fn catalog() -> Bootstrap {
    Bootstrap {
        projects: vec![Project {
            id: ProjectId(1),
            name: "p".into(),
            path: "/p".into(),
        }],
        models: vec![ModelInfo {
            id: "m".into(),
            name: "M".into(),
            note: String::new(),
        }],
        conversations: vec![
            (ConversationId(1), ProjectId(1), "one".into(), 10),
            (ConversationId(2), ProjectId(1), "two".into(), 5),
        ],
        now: 50,
    }
}

#[test]
fn catalog_arrival_selects_and_opens_first_conversation() {
    let mut s = empty_state(Prefs::default());
    assert!(s.select_initial().effects.is_empty());
    s.apply_catalog(catalog());
    assert_eq!(s.prefs.model.as_deref(), Some("m"));
    let out = s.select_initial();
    assert_eq!(s.selected, Some(ConversationId(1)));
    assert!(out.effects.iter().any(|e| matches!(
        e,
        Effect::Backend(BackendRequest::Open { conversation, .. }) if *conversation == ConversationId(1)
    )));
}

#[test]
fn catalog_honors_remembered_selection_and_keeps_local_state() {
    let prefs = Prefs {
        selected_conversation: Some(ConversationId(2)),
        ..Prefs::default()
    };
    let mut s = empty_state(prefs);
    s.apply_catalog(catalog());
    s.restore_draft(ConversationId(2), "kept".into(), vec![]);
    s.apply_catalog(catalog());
    assert_eq!(s.conversations.len(), 2);
    s.select_initial();
    assert_eq!(s.selected, Some(ConversationId(2)));
    assert_eq!(s.current().unwrap().draft.text, "kept");
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

fn backend_submits(out: &Outcome) -> usize {
    out.effects
        .iter()
        .filter(|e| matches!(e, Effect::Backend(BackendRequest::Submit { .. })))
        .count()
}

fn intent_request(out: &Outcome) -> RequestId {
    out.effects
        .iter()
        .find_map(|e| match e {
            Effect::JournalIntent { request, .. } => Some(request.clone()),
            _ => None,
        })
        .expect("a journal intent")
}

#[test]
fn nothing_is_sent_or_cleared_until_the_intent_is_durable() {
    let mut s = state();
    s.dispatch(Command::EditDraft("fix it".into()));
    let out = s.dispatch(Command::Submit);
    assert_eq!(backend_submits(&out), 0);
    let request = intent_request(&out);
    let c = s.current().unwrap();
    assert_eq!(c.draft.text, "fix it", "draft is kept until the commit");
    assert_eq!(c.run, RunState::Idle);
    assert!(c.items.is_empty());
    let a = s.availability();
    assert!(!a.submit && !a.retry, "no second submission while waiting");
    assert!(s.dispatch(Command::Submit).effects.is_empty());

    let out = s.intent_persisted(ConversationId(1), &request, Ok(()));
    assert_eq!(backend_submits(&out), 1);
    assert!(out.effects.iter().any(|e| matches!(
        e,
        Effect::Backend(BackendRequest::Submit { request: r, .. }) if *r == request
    )));
    let c = s.current().unwrap();
    assert_eq!(c.draft.text, "");
    assert!(matches!(c.run, RunState::Submitting { .. }));
}

#[test]
fn failed_journal_write_keeps_the_draft_and_sends_nothing() {
    let mut s = state();
    s.dispatch(Command::EditDraft("precious".into()));
    let first = intent_request(&s.dispatch(Command::Submit));
    let out = s.intent_persisted(ConversationId(1), &first, Err("disk full".into()));
    assert_eq!(backend_submits(&out), 0);
    let c = s.current().unwrap();
    assert_eq!(c.draft.text, "precious");
    assert_eq!(c.run, RunState::Idle);
    assert!(c.intent_error.as_ref().unwrap().contains("disk full"));
    assert!(s.availability().submit, "the user can try again");

    // A late duplicate acknowledgment for the failed request must do nothing.
    assert!(
        s.intent_persisted(ConversationId(1), &first, Ok(()))
            .effects
            .is_empty()
    );

    let second = intent_request(&s.dispatch(Command::Submit));
    assert_ne!(first, second, "each attempt has its own request id");
    assert!(s.current().unwrap().intent_error.is_none());
    s.dispatch(Command::DismissFailure);
}

#[test]
fn text_typed_after_pressing_send_is_not_lost() {
    let mut s = state();
    s.dispatch(Command::EditDraft("first".into()));
    let request = intent_request(&s.dispatch(Command::Submit));
    s.dispatch(Command::EditDraft("first and more".into()));
    let out = s.intent_persisted(ConversationId(1), &request, Ok(()));
    assert_eq!(backend_submits(&out), 1);
    assert_eq!(s.current().unwrap().draft.text, "first and more");
}

#[test]
fn acknowledgment_for_another_request_is_ignored() {
    let mut s = state();
    s.dispatch(Command::EditDraft("x".into()));
    s.dispatch(Command::Submit);
    let out = s.intent_persisted(ConversationId(1), &RequestId("other".into()), Ok(()));
    assert!(out.effects.is_empty() && out.notes.is_empty());
    assert!(s.current().unwrap().pending_intent.is_some());
}

#[test]
fn request_ids_are_unique_and_prefixed() {
    let mut s = state();
    s.set_request_prefix("abc".into());
    s.dispatch(Command::EditDraft("a".into()));
    let a = intent_request(&send_raw(&mut s));
    s.intent_persisted(ConversationId(1), &a, Err("x".into()));
    let b = intent_request(&send_raw(&mut s));
    assert!(a.0.starts_with("abc-") && b.0.starts_with("abc-"));
    assert_ne!(a, b);
}

fn send_raw(s: &mut AppState) -> Outcome {
    s.dispatch(Command::Submit)
}

#[test]
fn journal_follows_the_request_to_a_terminal_state_once() {
    let mut s = state();
    s.dispatch(Command::EditDraft("go".into()));
    send(&mut s, Command::Submit);
    let accepted = ev(&s, EventKind::Accepted);
    assert_eq!(
        journal_states(&s.apply_event(accepted)),
        vec![JournalState::Accepted]
    );
    let done = ev(&s, EventKind::Completed);
    assert_eq!(
        journal_states(&s.apply_event(done)),
        vec![JournalState::Completed]
    );
    // The request is settled; nothing can rewrite it.
    let again = BackendEvent {
        conversation: ConversationId(1),
        generation: s.current().unwrap().generation,
        op: None,
        kind: EventKind::Completed,
    };
    assert!(journal_states(&s.apply_event(again)).is_empty());
}

#[test]
fn lost_ack_and_rejection_are_journaled() {
    let mut s = state();
    s.dispatch(Command::EditDraft("go".into()));
    send(&mut s, Command::Submit);
    let lost = ev(&s, EventKind::AckLost);
    assert_eq!(
        journal_states(&s.apply_event(lost)),
        vec![JournalState::Unknown]
    );
    let resolved = ev(&s, EventKind::StatusResolved { accepted: false });
    assert_eq!(
        journal_states(&s.apply_event(resolved)),
        vec![JournalState::Rejected]
    );
}

#[test]
fn recovered_unresolved_request_is_unknown_and_never_resent() {
    let mut s = state();
    let r1 = RequestId("old-1".into());
    let out = s.restore_unresolved(ConversationId(1), r1.clone(), "half sent".into(), vec![]);
    assert_eq!(backend_submits(&out), 0);
    assert!(out.effects.is_empty());
    let c = s.current().unwrap();
    assert!(matches!(c.run, RunState::OutcomeUnknown { .. }));
    assert_eq!(c.last_submission.as_ref().unwrap().0, "half sent");
    let a = s.availability();
    assert!(a.check_status && !a.submit);

    // A newer unresolved request for the same conversation supersedes the older one.
    let out = s.restore_unresolved(
        ConversationId(1),
        RequestId("old-2".into()),
        "newer".into(),
        vec![],
    );
    assert!(out.effects.iter().any(|e| matches!(
        e,
        Effect::JournalState { request, state: JournalState::Superseded, .. } if *request == r1
    )));

    // Resolving as not accepted closes the newest request and offers an explicit retry.
    let resolved = ev(&s, EventKind::StatusResolved { accepted: false });
    assert_eq!(
        journal_states(&s.apply_event(resolved)),
        vec![JournalState::Rejected]
    );
    assert!(s.availability().retry);
}

#[test]
fn failed_queue_journal_write_puts_the_prompt_back() {
    let mut s = state();
    run_started(&mut s, "first");
    s.dispatch(Command::EditDraft("second".into()));
    s.dispatch(Command::QueueFollowUp);
    let done = ev(&s, EventKind::Completed);
    let out = s.apply_event(done);
    let request = intent_request(&out);
    assert!(s.current().unwrap().queue.is_empty());
    s.intent_persisted(ConversationId(1), &request, Err("busy".into()));
    let c = s.current().unwrap();
    assert_eq!(c.queue.len(), 1);
    assert_eq!(c.queue[0].text, "second");
}

#[test]
fn synced_replaces_items_only_once_opened() {
    let mut s = state();
    let item = |id: u64, text: &str| TranscriptItem {
        native_only: false,
        id: ItemId(id),
        at: 0,
        kind: ItemKind::Assistant {
            text: text.into(),
            streaming: false,
        },
    };
    let ev = |s: &AppState, items| {
        let c = s.current().unwrap();
        BackendEvent {
            conversation: c.id,
            generation: c.generation,
            op: None,
            kind: EventKind::Synced { items },
        }
    };
    // Not opened yet: ignored.
    let e = ev(&s, vec![item(1, "early")]);
    assert!(s.apply_event(e).notes.is_empty());
    assert!(s.current().unwrap().items.is_empty());

    let c = s.current().unwrap();
    let open = BackendEvent {
        conversation: c.id,
        generation: c.generation,
        op: None,
        kind: EventKind::Opened {
            items: vec![item(1, "one")],
            has_older: false,
            changes: vec![],
        },
    };
    s.apply_event(open);
    let e = ev(&s, vec![item(1, "one"), item(2, "two")]);
    let out = s.apply_event(e);
    assert_eq!(s.current().unwrap().items.len(), 2);
    assert!(out.notes.iter().any(|n| matches!(n, Note::ItemsReset(_))));

    // A stale generation never lands.
    let mut stale = ev(&s, vec![item(9, "stale")]);
    stale.generation += 1;
    s.apply_event(stale);
    assert_eq!(s.current().unwrap().items.len(), 2);
}

#[test]
fn output_preview_is_bounded_on_a_character_boundary() {
    let (short, cut) = preview_output("hi");
    assert_eq!((short.as_str(), cut), ("hi", false));
    let long = "\u{e9}".repeat(TOOL_OUTPUT_PREVIEW_BYTES); // two bytes per char
    let (preview, cut) = preview_output(&long);
    assert!(cut && preview.len() <= TOOL_OUTPUT_PREVIEW_BYTES);
    assert!(preview.chars().all(|c| c == '\u{e9}'));
}

#[test]
fn a_failed_open_is_visible_and_retryable() {
    let mut s = state();
    let c = s.current().unwrap();
    let (conv, generation) = (c.id, c.generation);
    let failed = BackendEvent {
        conversation: conv,
        generation,
        op: None,
        kind: EventKind::OpenFailed {
            message: "engine unreachable".into(),
        },
    };
    s.apply_event(failed);
    let c = s.current().unwrap();
    assert!(!c.opened);
    assert!(
        matches!(&c.items[0].kind, ItemKind::Notice { text, level: NoticeLevel::Error } if text == "engine unreachable")
    );
    // Selecting it again (after leaving) asks the backend to open it once more.
    s.dispatch(Command::SelectConversation(ConversationId(2)));
    let out = s.dispatch(Command::SelectConversation(conv));
    assert!(out.effects.iter().any(|e| matches!(e, Effect::Backend(BackendRequest::Open { conversation, .. }) if *conversation == conv)));
    // Once open, a later failure does not clobber the real transcript.
    let c = s.current().unwrap();
    let open = BackendEvent {
        conversation: conv,
        generation: c.generation,
        op: None,
        kind: EventKind::Opened {
            items: vec![],
            has_older: false,
            changes: vec![],
        },
    };
    s.apply_event(open);
    let c = s.current().unwrap();
    let late = BackendEvent {
        conversation: conv,
        generation: c.generation,
        op: None,
        kind: EventKind::OpenFailed {
            message: "late".into(),
        },
    };
    s.apply_event(late);
    assert!(s.current().unwrap().items.is_empty());
}

#[test]
fn new_conversation_is_offered_only_where_it_can_work() {
    let mut s = state();
    assert!(s.availability().new_conversation, "demo with a project");
    // Also available before any conversation is selected.
    s.selected = None;
    assert!(s.availability().new_conversation);
    // Not while disconnected.
    s.set_connection(Connection::Offline("x".into()));
    assert!(!s.availability().new_conversation);
    s.set_connection(Connection::Ready);
    // Real sessions are created by the engine, which cannot be asked yet.
    s.mode = Mode::Real;
    assert!(!s.availability().new_conversation);
    // No project, nothing to create in.
    let mut empty = empty_state(Prefs::default());
    assert!(!empty.availability().new_conversation);
    assert!(empty.dispatch(Command::NewConversation).effects.is_empty());
}

#[test]
fn a_read_only_build_offers_no_way_to_send() {
    let mut s = state();
    s.read_only = Some("cannot run prompts yet".into());
    s.dispatch(Command::EditDraft("hello".into()));
    let a = s.availability();
    assert!(!a.submit && !a.steer && !a.queue && !a.retry);
    assert!(s.dispatch(Command::Submit).effects.is_empty());
    assert!(
        s.current().unwrap().pending_intent.is_none(),
        "nothing was journaled"
    );
    // Reading still works: stopping and paging are not send actions.
    s.read_only = None;
    assert!(s.availability().submit);
}

fn real_state() -> AppState {
    let mut s = empty_state(Prefs::default());
    s.mode = Mode::Real;
    s.can_create = true;
    s
}

#[test]
fn project_ids_names_and_paths_are_derived_from_the_folder() {
    assert_eq!(
        project_id_for_path("/work/app"),
        project_id_for_path("/work/app")
    );
    assert_ne!(
        project_id_for_path("/work/app"),
        project_id_for_path("/work/other")
    );
    assert_eq!(project_name_for_path("/work/app"), "app");
    assert_eq!(project_name_for_path("/work/app/"), "app");
    assert_eq!(project_name_for_path("/"), "/");
}

#[test]
fn opening_a_project_adds_selects_and_remembers_it() {
    let mut s = real_state();
    let out = s.dispatch(Command::AddProject("/work/app/".into()));
    assert!(
        out.effects
            .iter()
            .any(|e| matches!(e, Effect::SaveProject { path } if path == "/work/app"))
    );
    assert_eq!(s.projects.len(), 1);
    assert_eq!(s.projects[0].name, "app");
    assert_eq!(s.current_project().unwrap().path, "/work/app");
    // Adding it again changes nothing and saves nothing new.
    let again = s.dispatch(Command::AddProject("/work/app".into()));
    assert!(
        !again
            .effects
            .iter()
            .any(|e| matches!(e, Effect::SaveProject { .. }))
    );
    assert_eq!(s.projects.len(), 1);
    // A relative path is refused with a visible notice.
    s.dispatch(Command::AddProject("relative".into()));
    assert!(s.notice.as_deref().unwrap().contains("absolute"));
    s.dispatch(Command::DismissNotice);
    assert!(s.notice.is_none());
}

#[test]
fn bookmarked_projects_survive_the_backend_catalog() {
    let mut s = real_state();
    s.restore_project("/work/mine");
    s.apply_catalog(Bootstrap {
        projects: vec![Project {
            id: ProjectId(5),
            name: "theirs".into(),
            path: "/work/theirs".into(),
        }],
        models: vec![],
        conversations: vec![],
        now: 1,
    });
    let names: Vec<&str> = s.projects.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["theirs", "mine"]);
    // A catalog that already knows the folder does not duplicate it.
    s.apply_catalog(Bootstrap {
        projects: vec![Project {
            id: project_id_for_path("/work/mine"),
            name: "mine".into(),
            path: "/work/mine".into(),
        }],
        models: vec![],
        conversations: vec![],
        now: 2,
    });
    assert_eq!(s.projects.len(), 1);
}

#[test]
fn a_new_conversation_is_requested_from_the_backend_and_selected_when_it_arrives() {
    let mut s = real_state();
    s.dispatch(Command::AddProject("/work/app".into()));
    assert!(s.availability().new_conversation);
    let out = s.dispatch(Command::NewConversation);
    let request = out.effects.iter().find_map(|e| match e {
        Effect::Backend(BackendRequest::CreateConversation {
            cwd,
            project,
            request,
            ..
        }) => {
            assert_eq!(cwd, "/work/app");
            assert_eq!(*project, project_id_for_path("/work/app"));
            Some(request.clone())
        }
        _ => None,
    });
    assert!(request.is_some());
    // While it is pending, another request is not offered (no duplicate creation).
    assert!(!s.availability().new_conversation);
    assert!(s.dispatch(Command::NewConversation).effects.is_empty());

    // Only the exact creation receipt plus catalog can select the new session.
    s.native_conversation_created(request.as_ref().unwrap(), ConversationId(11));
    let project = project_id_for_path("/work/app");
    let out = s.apply_catalog(Bootstrap {
        projects: vec![],
        models: vec![],
        conversations: vec![
            (ConversationId(10), project, "old".into(), 5),
            (ConversationId(11), project, "new".into(), 9),
        ],
        now: 10,
    });
    assert_eq!(s.selected, Some(ConversationId(11)));
    assert!(out.effects.iter().any(|e| matches!(e, Effect::Backend(BackendRequest::Open { conversation, .. }) if *conversation == ConversationId(11))));
    assert!(
        s.availability().new_conversation,
        "creation is available again"
    );
}

#[test]
fn a_failed_creation_shows_a_notice_and_allows_trying_again() {
    let mut s = real_state();
    s.dispatch(Command::AddProject("/work/app".into()));
    s.dispatch(Command::NewConversation);
    assert!(!s.availability().new_conversation);
    s.set_notice("Session cwd is not an existing directory: /work/app".into());
    assert!(
        s.notice
            .as_deref()
            .unwrap()
            .contains("not an existing directory")
    );
    assert!(s.availability().new_conversation);
}

#[test]
fn creation_needs_the_backend_to_support_it() {
    let mut s = real_state();
    s.can_create = false;
    s.dispatch(Command::AddProject("/work/app".into()));
    assert!(!s.availability().new_conversation);
    assert!(s.dispatch(Command::NewConversation).effects.is_empty());
}

#[test]
fn in_real_mode_the_engine_decides_the_model() {
    let mut s = real_state();
    s.dispatch(Command::AddProject("/work/app".into()));
    let project = project_id_for_path("/work/app");
    s.apply_catalog(Bootstrap {
        projects: vec![],
        models: vec![
            ModelInfo {
                id: "stub/a".into(),
                name: "A".into(),
                note: String::new(),
            },
            ModelInfo {
                id: "stub/b".into(),
                name: "B".into(),
                note: String::new(),
            },
        ],
        conversations: vec![(ConversationId(1), project, "c".into(), 1)],
        now: 1,
    });
    s.dispatch(Command::SelectConversation(ConversationId(1)));
    // Not opened yet: nothing to ask.
    assert!(
        s.dispatch(Command::SetModel("stub/b".into()))
            .effects
            .is_empty()
    );
    let c = s.current().unwrap();
    s.apply_event(BackendEvent {
        conversation: ConversationId(1),
        generation: c.generation,
        op: None,
        kind: EventKind::Opened {
            items: vec![],
            has_older: false,
            changes: vec![],
        },
    });
    support::native_context(&mut s);
    let out = s.dispatch(Command::SetModel("stub/b".into()));
    assert!(out.effects.iter().any(|e| matches!(e, Effect::Backend(BackendRequest::SetModel { model, .. }) if model == "stub/b")));
    // The request alone changes nothing; the engine's report does.
    assert_ne!(s.prefs.model.as_deref(), Some("stub/b"));
    s.set_engine_model(Some("stub/b".into()));
    assert_eq!(s.prefs.model.as_deref(), Some("stub/b"));
    // An unknown model is not even requested.
    assert!(
        s.dispatch(Command::SetModel("nope".into()))
            .effects
            .is_empty()
    );
    // A report of "no model" does not erase a known selection.
    s.set_engine_model(None);
    assert_eq!(s.prefs.model.as_deref(), Some("stub/b"));
}

#[test]
fn workspace_changes_sync_independently_of_any_run() {
    let mut s = state();
    let c = s.current().unwrap();
    let (conv, generation) = (c.id, c.generation);
    let ev = |kind| BackendEvent {
        conversation: conv,
        generation,
        op: None,
        kind,
    };
    let change = |path: &str| FileChange {
        path: path.into(),
        added: 1,
        removed: 0,
        hunks: vec![],
    };
    // Ignored until the conversation is open.
    s.apply_event(ev(EventKind::ChangesSynced(vec![change("early.rs")])));
    assert!(s.current().unwrap().changes.is_empty());
    s.apply_event(ev(EventKind::Opened {
        items: vec![],
        has_older: false,
        changes: vec![],
    }));
    s.apply_event(ev(EventKind::ChangesSynced(vec![
        change("a.rs"),
        change("b.rs"),
    ])));
    assert_eq!(s.current().unwrap().changes.len(), 2);
    assert_eq!(s.current().unwrap().selected_change, Some(0));
    s.apply_event(ev(EventKind::ChangesSynced(vec![])));
    assert!(s.current().unwrap().changes.is_empty());
    assert_eq!(s.current().unwrap().selected_change, None);
}

fn scan_event(s: &AppState, kind: EventKind) -> BackendEvent {
    BackendEvent {
        op: None,
        ..ev(s, kind)
    }
}

fn scanned_change(path: &str) -> FileChange {
    FileChange {
        path: path.into(),
        added: 1,
        removed: 0,
        hunks: vec![],
    }
}

#[test]
fn unchanged_scans_preserve_diff_revision_selection_and_emit_no_change_note() {
    for files in [vec![], vec![scanned_change("a.rs")]] {
        let mut s = state();
        s.mode = Mode::Real;
        s.apply_event(scan_event(
            &s,
            EventKind::Opened {
                items: vec![],
                has_older: false,
                changes: vec![],
            },
        ));
        s.apply_event(scan_event(&s, EventKind::ChangesSynced(files.clone())));
        let rev = s.current().unwrap().changes_revision;
        let selected = s.current().unwrap().selected_change;
        for _ in 0..3 {
            let out = s.apply_event(scan_event(&s, EventKind::ChangesSynced(files.clone())));
            assert!(out.notes.is_empty());
            assert_eq!(s.current().unwrap().changes_revision, rev);
            assert_eq!(s.current().unwrap().selected_change, selected);
        }
        s.dispatch(Command::RefreshChanges);
        let out = s.apply_event(scan_event(&s, EventKind::ChangesSynced(files.clone())));
        assert!(
            !out.notes.is_empty(),
            "explicit refresh must settle its progress state"
        );
        assert_eq!(s.current().unwrap().changes_state, ChangesState::Ready);
        assert_eq!(s.current().unwrap().changes_revision, rev);
        assert_eq!(s.current().unwrap().selected_change, selected);
        assert!(s.availability().refresh_changes);
        let failure = ChangesState::Unavailable("git failed".into());
        s.apply_event(scan_event(&s, EventKind::ChangesScanState(failure.clone())));
        assert!(
            s.apply_event(scan_event(&s, EventKind::ChangesScanState(failure)))
                .notes
                .is_empty()
        );
        assert_eq!(s.current().unwrap().changes, files);
        s.apply_event(scan_event(
            &s,
            EventKind::ChangesScanState(ChangesState::NotARepository),
        ));
        let cleared_rev = s.current().unwrap().changes_revision;
        assert!(
            s.apply_event(scan_event(
                &s,
                EventKind::ChangesScanState(ChangesState::NotARepository)
            ))
            .notes
            .is_empty()
        );
        assert_eq!(s.current().unwrap().changes_revision, cleared_rev);
    }
}

#[test]
fn failed_scans_preserve_the_last_diff_and_retry_is_read_only_and_single_flight() {
    let mut s = state();
    s.mode = Mode::Real;
    assert!(!s.availability().refresh_changes);
    s.apply_event(scan_event(
        &s,
        EventKind::ChangesScanState(ChangesState::Unavailable("early".into())),
    ));
    assert_eq!(s.current().unwrap().changes_state, ChangesState::Unscanned);
    s.apply_event(scan_event(
        &s,
        EventKind::Opened {
            items: vec![],
            has_older: false,
            changes: vec![],
        },
    ));
    s.apply_event(scan_event(
        &s,
        EventKind::ChangesSynced(vec![scanned_change("a.rs")]),
    ));
    let rev = s.current().unwrap().changes_revision;
    s.apply_event(scan_event(
        &s,
        EventKind::ChangesScanState(ChangesState::Unavailable("git missing".into())),
    ));
    assert_eq!(s.current().unwrap().changes[0].path, "a.rs");
    assert_eq!(s.current().unwrap().selected_change, Some(0));
    assert_eq!(s.current().unwrap().changes_revision, rev);
    assert!(s.availability().refresh_changes);
    // Refresh remains available in a read-only build, but cannot overlap its own request.
    s.read_only = Some("read-only build".into());
    let out = s.dispatch(Command::RefreshChanges);
    assert!(matches!(
        out.effects.as_slice(),
        [Effect::Backend(BackendRequest::RefreshChanges { .. })]
    ));
    assert_eq!(s.current().unwrap().changes_state, ChangesState::Loading);
    assert!(!s.availability().refresh_changes);
    assert!(s.dispatch(Command::RefreshChanges).effects.is_empty());
    s.set_connection(Connection::Reconnecting);
    assert!(matches!(
        s.current().unwrap().changes_state,
        ChangesState::Unavailable(_)
    ));
    assert!(!s.availability().refresh_changes);
    assert!(s.dispatch(Command::RefreshChanges).effects.is_empty());
    s.set_connection(Connection::Ready);
    assert!(s.availability().refresh_changes);
    s.dispatch(Command::RefreshChanges);
    s.apply_event(scan_event(&s, EventKind::ChangesSynced(vec![])));
    assert_eq!(s.current().unwrap().changes_state, ChangesState::Ready);
    assert!(s.current().unwrap().changes.is_empty());
    assert_eq!(s.current().unwrap().selected_change, None);
    assert!(s.current().unwrap().changes_revision > rev);
}

#[test]
fn scan_state_is_conversation_scoped_and_stale_generations_are_ignored() {
    let mut s = state();
    s.mode = Mode::Real;
    let opened = || EventKind::Opened {
        items: vec![],
        has_older: false,
        changes: vec![],
    };
    s.apply_event(scan_event(&s, opened()));
    s.apply_event(scan_event(
        &s,
        EventKind::ChangesSynced(vec![scanned_change("project-a.rs")]),
    ));
    let late = scan_event(
        &s,
        EventKind::ChangesScanState(ChangesState::Unavailable("old scan".into())),
    );
    s.projects.push(Project {
        id: ProjectId(2),
        name: "other".into(),
        path: "/other".into(),
    });
    s.conversations[1].project = ProjectId(2);
    s.dispatch(Command::SelectConversation(ConversationId(2)));
    s.apply_event(scan_event(&s, opened()));
    s.apply_event(scan_event(
        &s,
        EventKind::ChangesSynced(vec![scanned_change("project-b.rs")]),
    ));
    s.apply_event(late.clone());
    assert_eq!(s.current().unwrap().changes_state, ChangesState::Ready);
    assert_eq!(s.current().unwrap().changes[0].path, "project-b.rs");
    s.dispatch(Command::SelectConversation(ConversationId(1)));
    assert_eq!(s.current().unwrap().changes_state, ChangesState::Unscanned);
    s.apply_event(scan_event(&s, opened()));
    // Reattaching Pi's empty initial diff must preserve this conversation's stale snapshot.
    assert_eq!(s.current().unwrap().changes[0].path, "project-a.rs");
    s.apply_event(late);
    assert_eq!(s.current().unwrap().changes_state, ChangesState::Unscanned);
    s.apply_event(scan_event(
        &s,
        EventKind::ChangesScanState(ChangesState::NotARepository),
    ));
    assert!(s.current().unwrap().changes.is_empty());
    assert_eq!(s.current().unwrap().selected_change, None);
    assert_eq!(
        s.current().unwrap().changes_state,
        ChangesState::NotARepository
    );
}

#[test]
fn equal_size_diff_updates_still_invalidate_cached_rows() {
    let mut s = state();
    s.apply_event(ev(
        &s,
        EventKind::Opened {
            items: vec![],
            has_older: false,
            changes: vec![],
        },
    ));
    let change = |text: &str| FileChange {
        hunks: vec![Hunk {
            header: "@@".into(),
            lines: vec![DiffLine {
                kind: DiffKind::Add,
                old_no: None,
                new_no: Some(1),
                text: text.into(),
            }],
        }],
        ..scanned_change("same.rs")
    };
    s.apply_event(scan_event(
        &s,
        EventKind::ChangesSynced(vec![change("first")]),
    ));
    let revision = s.current().unwrap().changes_revision;
    s.apply_event(scan_event(
        &s,
        EventKind::ChangesSynced(vec![change("second")]),
    ));
    assert!(s.current().unwrap().changes_revision > revision);
    assert_eq!(
        s.current().unwrap().changes[0].hunks[0].lines[0].text,
        "second"
    );
    s.apply_event(scan_event(
        &s,
        EventKind::ChangesSynced(vec![scanned_change("new-first.rs"), change("third")]),
    ));
    assert_eq!(
        s.current().unwrap().selected_change,
        Some(1),
        "preserve the selected path, not its old index"
    );
    s.mode = Mode::Demo;
    assert!(!s.availability().refresh_changes);
    assert!(s.dispatch(Command::RefreshChanges).effects.is_empty());
}

#[test]
fn check_status_carries_the_journaled_request_key() {
    let mut s = state();
    s.dispatch(Command::EditDraft("risky".into()));
    let out = send(&mut s, Command::Submit);
    let request = out.effects.iter().find_map(|e| match e {
        Effect::Backend(BackendRequest::Submit { request, .. }) => Some(request.clone()),
        _ => None,
    });
    let lost = ev(&s, EventKind::AckLost);
    s.apply_event(lost);
    let out = s.dispatch(Command::CheckStatus);
    assert!(out.effects.iter().any(|e| matches!(
        e,
        Effect::Backend(BackendRequest::CheckStatus { request: r, .. }) if *r == request
    )));
}
