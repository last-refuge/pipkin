//! A conversation the built-in engine owns after an import is read-only with Pi: every path that
//! would change it is refused before anything is sent, stopping stays possible, and releasing it
//! back is explicit and confirmed.

use pipkin_core::*;

const ONE: ConversationId = ConversationId(1);
const TWO: ConversationId = ConversationId(2);

fn real() -> AppState {
    let mut s = AppState::new(
        Bootstrap {
            projects: vec![Project {
                id: ProjectId(1),
                name: "p".into(),
                path: "/work/p".into(),
            }],
            models: vec![
                ModelInfo {
                    id: "a/m".into(),
                    name: "M".into(),
                    note: String::new(),
                },
                ModelInfo {
                    id: "a/n".into(),
                    name: "N".into(),
                    note: String::new(),
                },
            ],
            conversations: vec![
                (ONE, ProjectId(1), "owned".into(), 2),
                (TWO, ProjectId(1), "free".into(), 1),
            ],
            now: 1000,
        },
        Prefs::default(),
    );
    s.mode = Mode::Real;
    s.prefs.selected_project = Some(ProjectId(1));
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

fn apply(s: &mut AppState, kind: EventKind) -> Outcome {
    let e = event(s, kind);
    s.apply_event(e)
}

fn open(s: &mut AppState, id: ConversationId) {
    s.dispatch(Command::SelectConversation(id));
    apply(
        s,
        EventKind::Opened {
            items: vec![],
            has_older: false,
            changes: vec![],
        },
    );
    // The engine reports the thinking levels it supports.
    apply(
        s,
        EventKind::ThinkingState {
            level: "off".into(),
            levels: vec!["off".into(), "high".into()],
        },
    );
}

fn owned(s: &mut AppState) {
    s.restore_owner(ONE, ConversationOwner::NativeEngine { imported_at: 900 });
}

/// What would leave the window: backend requests and journaled submissions.
fn outgoing(out: &Outcome) -> Vec<&Effect> {
    out.effects
        .iter()
        .filter(|e| {
            matches!(
                e,
                Effect::Backend(BackendRequest::Submit { .. })
                    | Effect::Backend(BackendRequest::Steer { .. })
                    | Effect::Backend(BackendRequest::Queue { .. })
                    | Effect::Backend(BackendRequest::CancelQueued { .. })
                    | Effect::Backend(BackendRequest::SetModel { .. })
                    | Effect::Backend(BackendRequest::SetThinkingLevel { .. })
                    | Effect::Backend(BackendRequest::SetSubagentsEnabled { .. })
                    | Effect::Backend(BackendRequest::UiRespond { .. })
                    | Effect::JournalIntent { .. }
                    | Effect::SaveGoal { .. }
            )
        })
        .collect()
}

fn type_draft(s: &mut AppState, text: &str) {
    s.dispatch(Command::EditDraft(text.into()));
}

fn journaled(out: &Outcome) -> Vec<(ConversationId, RequestId)> {
    out.effects
        .iter()
        .filter_map(|e| match e {
            Effect::JournalIntent {
                conversation,
                request,
                ..
            } => Some((*conversation, request.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn an_owned_conversation_reads_but_offers_no_way_to_send() {
    let mut s = real();
    owned(&mut s);
    open(&mut s, ONE);
    type_draft(&mut s, "please continue");
    let a = s.availability();
    assert!(!a.submit && !a.steer && !a.queue && !a.retry, "{a:?}");
    assert!(a.load_older || !s.current().unwrap().has_older);
    assert_eq!(
        s.read_only_reason(ONE).as_deref(),
        Some(NATIVE_OWNED_NOTICE),
        "the notice names the built-in engine and the import"
    );
    assert!(NATIVE_OWNED_NOTICE.contains("Pipkin --engine native"));
    assert!(NATIVE_OWNED_NOTICE.contains("history as of the import"));

    // Another conversation is unaffected.
    open(&mut s, TWO);
    type_draft(&mut s, "hello");
    assert!(s.availability().submit);
    assert_eq!(s.read_only_reason(TWO), None);
}

/// The state in which `command` would send something: idle with a draft (submit, goal, model,
/// thinking), a rejected prompt (retry), or a run in flight with a queued entry, a subagent
/// setting and an extension question (steer, queue, the rest).
fn ready_for(command: &Command, owner: bool) -> AppState {
    let mut s = real();
    open(&mut s, ONE);
    type_draft(&mut s, "please continue");
    match command {
        Command::Retry => {
            let (conversation, request) = journaled(&s.dispatch(Command::Submit)).remove(0);
            s.intent_persisted(conversation, &request, Ok(()));
            apply(
                &mut s,
                EventKind::Rejected {
                    reason: "busy".into(),
                },
            );
        }
        Command::Steer
        | Command::QueueFollowUp
        | Command::RemoveQueued(_)
        | Command::SetSubagentsEnabled(_)
        | Command::AnswerUiRequest { .. } => {
            // A run the engine reports (started before the import was recorded, or elsewhere).
            apply(
                &mut s,
                EventKind::EngineState {
                    busy: true,
                    queue: vec![QueuedPrompt {
                        source: None,
                        id: QueueId(5),
                        text: "queued".into(),
                        mode: QueueMode::FollowUp,
                    }],
                },
            );
            apply(
                &mut s,
                EventKind::SubagentsSynced {
                    enabled: false,
                    children: vec![],
                },
            );
            apply(
                &mut s,
                EventKind::UiState {
                    requests: vec![UiRequest {
                        id: "q1".into(),
                        kind: UiRequestKind::Confirm,
                        title: "Proceed?".into(),
                        message: None,
                        items: vec![],
                        placeholder: None,
                        default_value: None,
                        deadline: None,
                        form: None,
                        source: None,
                    }],
                    status: vec![],
                    notices: vec![],
                },
            );
        }
        _ => {}
    }
    if owner {
        owned(&mut s);
    }
    s
}

#[test]
fn every_change_to_an_owned_conversation_is_refused_before_anything_is_sent() {
    for command in [
        Command::Submit,
        Command::Steer,
        Command::QueueFollowUp,
        Command::Retry,
        Command::SetGoal("ship it".into()),
        Command::SetModel("a/n".into()),
        Command::SetThinkingLevel("high".into()),
        Command::SetSubagentsEnabled(true),
        Command::RemoveQueued(QueueId(5)),
        Command::AnswerUiRequest {
            id: "q1".into(),
            answer: UiAnswer::Confirm(true),
        },
    ] {
        // Not vacuous: nobody else owning it, the command does send.
        let mut free = ready_for(&command, false);
        let out = free.dispatch(command.clone());
        assert!(!outgoing(&out).is_empty(), "{command:?} sends when free");

        let mut s = ready_for(&command, true);
        let out = s.dispatch(command.clone());
        assert!(
            outgoing(&out).is_empty(),
            "{command:?} must send nothing: {:?}",
            out.effects
        );
        assert_eq!(
            s.notice.as_deref(),
            Some(NATIVE_OWNED_NOTICE),
            "{command:?} says why"
        );
        assert_eq!(s.current().unwrap().draft.text, "please continue");
        assert!(s.current().unwrap().goal.is_none(), "no goal was set");
    }
}

#[test]
fn stopping_a_run_in_flight_stays_possible() {
    let mut s = ready_for(&Command::Steer, true);
    assert!(matches!(s.current().unwrap().run, RunState::Running { .. }));
    let a = s.availability();
    assert!(!a.steer && !a.queue, "{a:?}");
    assert!(a.cancel && a.check_status, "{a:?}");
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
}

#[test]
fn the_same_commands_still_work_in_a_conversation_nobody_else_owns() {
    let mut s = real();
    owned(&mut s);
    open(&mut s, TWO);
    type_draft(&mut s, "hello");
    assert_eq!(journaled(&s.dispatch(Command::Submit)).len(), 1);
    assert_eq!(
        outgoing(&s.dispatch(Command::SetModel("a/n".into()))).len(),
        1
    );
    assert_eq!(s.notice, None);
}

#[test]
fn a_retry_or_a_goal_turn_is_not_sent_once_the_conversation_is_owned() {
    let mut s = real();
    open(&mut s, ONE);
    // A prompt rejected before the ownership was known: retry is not offered afterwards.
    type_draft(&mut s, "first");
    let out = s.dispatch(Command::Submit);
    let (conversation, request) = journaled(&out).remove(0);
    s.intent_persisted(conversation, &request, Ok(()));
    apply(
        &mut s,
        EventKind::Rejected {
            reason: "busy".into(),
        },
    );
    assert!(s.availability().retry);
    owned(&mut s);
    assert!(!s.availability().retry);
    assert!(outgoing(&s.dispatch(Command::Retry)).is_empty());

    // A submission whose journal commit lands after the ownership was recorded is settled as
    // never sent.
    let mut s = real();
    open(&mut s, ONE);
    type_draft(&mut s, "second");
    let (conversation, request) = journaled(&s.dispatch(Command::Submit)).remove(0);
    owned(&mut s);
    let out = s.intent_persisted(conversation, &request, Ok(()));
    assert!(
        !out.effects
            .iter()
            .any(|e| matches!(e, Effect::Backend(BackendRequest::Submit { .. })))
    );
    assert!(out.effects.iter().any(|e| matches!(
        e,
        Effect::JournalState {
            state: JournalState::Cancelled,
            ..
        }
    )));
    assert_eq!(
        s.current().unwrap().draft.text,
        "second",
        "the text is kept"
    );

    // A goal running when the ownership is recorded schedules no further turn.
    let mut s = real();
    open(&mut s, ONE);
    let (conversation, request) =
        journaled(&s.dispatch(Command::SetGoal("finish the port".into()))).remove(0);
    s.intent_persisted(conversation, &request, Ok(()));
    apply(&mut s, EventKind::Accepted);
    apply(
        &mut s,
        EventKind::Token("More to do.\n[PIPKIN_GOAL_CONTINUE]".into()),
    );
    owned(&mut s);
    let out = apply(&mut s, EventKind::Completed);
    assert!(
        journaled(&out).is_empty(),
        "no continuation: {:?}",
        out.effects
    );
}

#[test]
fn releasing_is_explicit_confirmed_and_only_then_makes_the_conversation_writable() {
    let mut s = real();
    open(&mut s, TWO);
    assert!(
        !s.can_release(),
        "only an owned conversation can be released"
    );
    assert!(s.dispatch(Command::ReleaseToPi).effects.is_empty());

    owned(&mut s);
    open(&mut s, ONE);
    assert!(s.can_release());
    assert!(
        s.dispatch(Command::ConfirmRelease).effects.is_empty(),
        "nothing without asking first"
    );
    let out = s.dispatch(Command::ReleaseToPi);
    assert!(out.effects.is_empty(), "asking changes nothing");
    assert_eq!(s.release, ReleaseFlow::Confirming(ONE));
    let notice = s.notice.clone().unwrap();
    assert!(
        notice.contains("after the import is not in the legacy copy"),
        "{notice}"
    );
    assert!(notice.contains(RELEASE_CONFIRM_TITLE), "{notice}");

    // Keeping it changes nothing.
    assert!(s.dispatch(Command::CancelRelease).effects.is_empty());
    assert_eq!(s.release, ReleaseFlow::Idle);
    assert!(s.owned_elsewhere(ONE));

    // Confirming removes only the record, for the conversation that was asked about.
    s.dispatch(Command::ReleaseToPi);
    s.dispatch(Command::SelectConversation(TWO));
    let out = s.dispatch(Command::ConfirmRelease);
    assert_eq!(
        out.effects,
        vec![Effect::ReleaseOwnership { conversation: ONE }]
    );
    assert_eq!(s.release, ReleaseFlow::Releasing(ONE));
    assert!(
        s.dispatch(Command::ConfirmRelease).effects.is_empty(),
        "once"
    );
    assert!(
        s.owned_elsewhere(ONE),
        "still owned until the record is gone"
    );

    // A failed removal keeps it read-only and says so.
    s.ownership_released(ONE, Err("disk full".into()));
    assert!(s.owned_elsewhere(ONE));
    assert!(s.notice.as_deref().unwrap().contains("disk full"));

    s.dispatch(Command::SelectConversation(ONE));
    s.dispatch(Command::ReleaseToPi);
    s.dispatch(Command::ConfirmRelease);
    s.ownership_released(ONE, Ok(()));
    assert!(!s.owned_elsewhere(ONE));
    assert_eq!(s.read_only_reason(ONE), None);
    assert!(!s.can_release());
    type_draft(&mut s, "continue in the legacy engine");
    assert!(s.availability().submit);
    assert_eq!(journaled(&s.dispatch(Command::Submit)).len(), 1);
}

#[test]
fn ownership_applies_to_a_conversation_that_appears_after_it_was_restored() {
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
    s.restore_owner(ONE, ConversationOwner::NativeEngine { imported_at: 1 });
    s.restore_cached_conversation(ONE, "/work/p", "saved".into(), 1);
    assert!(s.owned_elsewhere(ONE));
    s.apply_catalog(Bootstrap {
        projects: vec![Project {
            id: ProjectId(1),
            name: "p".into(),
            path: "/work/p".into(),
        }],
        models: vec![],
        conversations: vec![(ONE, project_id_for_path("/work/p"), "listed".into(), 2)],
        now: 2,
    });
    open(&mut s, ONE);
    type_draft(&mut s, "x");
    assert!(!s.availability().submit);
    assert_eq!(
        s.read_only_reason(ONE).as_deref(),
        Some(NATIVE_OWNED_NOTICE)
    );
}
