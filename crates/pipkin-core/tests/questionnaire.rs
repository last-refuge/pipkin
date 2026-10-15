//! Questionnaires (ASK-2) in the core: the draft is seeded from the engine's defaults and
//! edited by toggles (single replaces, multiple toggles, unknown ids ignored); it survives a
//! re-ask by the same asker and is dropped for a different one; the submitted answers are
//! built from the draft plus the presentation's texts, blanks dropped; a default alone is
//! submitted only as a selection the person left in place; and a waiting question marks its
//! conversation for attention whether or not it is open.

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
    state.apply_event(event(
        &state,
        EventKind::Opened {
            items: vec![],
            has_older: false,
            changes: vec![],
        },
    ));
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

fn option(id: &str) -> UiFormOption {
    UiFormOption {
        id: id.into(),
        label: id.to_uppercase(),
        description: None,
        preview: None,
    }
}

fn form() -> UiForm {
    UiForm {
        allow_note: true,
        questions: vec![
            UiFormQuestion {
                id: "approach".into(),
                kind: UiFormKind::Single,
                prompt: "Which?".into(),
                description: None,
                preview: None,
                options: vec![option("a"), option("b")],
                allow_other: true,
                allow_note: false,
                default: vec!["a".into()],
                placeholder: None,
            },
            UiFormQuestion {
                id: "scope".into(),
                kind: UiFormKind::Multiple,
                prompt: "Parts?".into(),
                description: None,
                preview: None,
                options: vec![option("x"), option("y"), option("z")],
                allow_other: false,
                allow_note: true,
                default: vec![],
                placeholder: None,
            },
            UiFormQuestion {
                id: "name".into(),
                kind: UiFormKind::Text,
                prompt: "Name?".into(),
                description: None,
                preview: None,
                options: vec![],
                allow_other: false,
                allow_note: false,
                default: vec![],
                placeholder: None,
            },
        ],
    }
}

fn request(id: &str, source: Option<&str>) -> UiRequest {
    UiRequest {
        id: id.into(),
        kind: UiRequestKind::Form,
        title: "Before I start".into(),
        message: None,
        items: vec![],
        placeholder: None,
        default_value: None,
        deadline: None,
        form: Some(form()),
        source: source.map(str::to_owned),
    }
}

fn show(s: &mut AppState, requests: Vec<UiRequest>) {
    s.apply_event(event(
        s,
        EventKind::UiState {
            requests,
            status: vec![],
            notices: vec![],
        },
    ));
}

fn selected(s: &AppState, q: &str) -> Vec<String> {
    s.current().unwrap().form_selected(q).to_vec()
}

fn toggle(s: &mut AppState, q: &str, o: &str) {
    s.dispatch(Command::ToggleFormOption {
        question: q.into(),
        option: o.into(),
    });
}

#[test]
fn the_draft_starts_from_defaults_and_toggles_per_kind() {
    let mut s = state();
    show(&mut s, vec![request("q7", Some("call:c1"))]);
    assert_eq!(selected(&s, "approach"), ["a"]);
    assert!(selected(&s, "scope").is_empty());
    // Single: replace; the same again clears (no answer).
    toggle(&mut s, "approach", "b");
    assert_eq!(selected(&s, "approach"), ["b"]);
    toggle(&mut s, "approach", "b");
    assert!(selected(&s, "approach").is_empty());
    // Multiple: toggle each.
    toggle(&mut s, "scope", "x");
    toggle(&mut s, "scope", "z");
    toggle(&mut s, "scope", "x");
    assert_eq!(selected(&s, "scope"), ["z"]);
    // Unknown question or option: nothing.
    toggle(&mut s, "scope", "nope");
    toggle(&mut s, "colour", "x");
    assert_eq!(selected(&s, "scope"), ["z"]);
    // Only the open questionnaire is a draft.
    assert!(s.current().unwrap().activity() == Some("Needs your answer"));
}

#[test]
fn the_draft_survives_a_re_ask_by_the_same_asker_and_not_by_another() {
    let mut s = state();
    show(&mut s, vec![request("q1", Some("call:c1"))]);
    toggle(&mut s, "scope", "y");
    // The engine restarted: a new runtime id, the same call.
    show(&mut s, vec![request("q2", Some("call:c1"))]);
    assert_eq!(selected(&s, "scope"), ["y"]);
    assert_eq!(selected(&s, "approach"), ["a"]);
    // A different asker: defaults again.
    show(&mut s, vec![request("q3", Some("call:c2"))]);
    assert!(selected(&s, "scope").is_empty());
    // Gone: no draft.
    show(&mut s, vec![]);
    assert!(s.current().unwrap().form_draft.is_none());
    assert!(s.current().unwrap().activity().is_none());
}

#[test]
fn answers_are_built_from_the_draft_and_the_texts_with_blanks_dropped() {
    let mut s = state();
    show(&mut s, vec![request("q1", None)]);
    toggle(&mut s, "scope", "x");
    toggle(&mut s, "scope", "z");
    let texts = |q: &str, field: &str| -> Option<String> {
        match (q, field) {
            ("approach", "text") => Some("  ".into()),
            ("scope", "note") => Some(" skip docs ".into()),
            ("name", "text") => Some("Pipkin".into()),
            ("", "note") => Some("".into()),
            _ => None,
        }
    };
    let (id, answers) = s.current().unwrap().form_answers(texts).unwrap();
    assert_eq!(id, "q1");
    assert_eq!(answers.note, None, "a blank note is dropped");
    assert_eq!(
        answers.answers,
        vec![
            UiFormAnswer {
                question: "approach".into(),
                selected: vec!["a".into()],
                text: None,
                note: None,
            },
            UiFormAnswer {
                question: "scope".into(),
                selected: vec!["x".into(), "z".into()],
                text: None,
                note: Some("skip docs".into()),
            },
            UiFormAnswer {
                question: "name".into(),
                selected: vec![],
                text: Some("Pipkin".into()),
                note: None,
            },
        ]
    );
    // Submitting sends exactly that, once.
    let out = s.dispatch(Command::AnswerUiRequest {
        id: id.clone(),
        answer: UiAnswer::Form(answers.clone()),
    });
    assert!(matches!(
        &out.effects[..],
        [Effect::Backend(BackendRequest::UiRespond { id: i, answer: UiAnswer::Form(a), .. })]
            if *i == id && *a == answers
    ));
    assert!(
        s.dispatch(Command::AnswerUiRequest {
            id,
            answer: UiAnswer::Form(answers),
        })
        .effects
        .is_empty()
    );
    // A text question's text is only read for text or "other" questions.
    let (_, answers) = s
        .current()
        .unwrap()
        .form_answers(|_, _| Some("stray".into()))
        .unwrap();
    assert_eq!(answers.answers[1].text, None);
    assert_eq!(answers.answers[0].text.as_deref(), Some("stray"));
}

#[test]
fn a_waiting_question_marks_any_conversation_for_attention() {
    let mut s = state();
    // Reported for a conversation that is not open.
    s.set_question_waiting(ConversationId(2), true);
    assert_eq!(
        s.conversation(ConversationId(2)).unwrap().activity(),
        Some("Needs your answer")
    );
    assert!(s.current().unwrap().activity().is_none());
    s.set_question_waiting(ConversationId(2), false);
    assert!(
        s.conversation(ConversationId(2))
            .unwrap()
            .activity()
            .is_none()
    );
    // The open conversation's own state keeps the same flag.
    show(&mut s, vec![request("q1", None)]);
    assert!(s.current().unwrap().waiting_question);
    show(&mut s, vec![]);
    assert!(!s.current().unwrap().waiting_question);
    // An unknown conversation is ignored.
    assert!(
        s.set_question_waiting(ConversationId(9), true)
            .notes
            .is_empty()
    );
}
