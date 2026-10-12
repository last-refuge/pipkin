use pipkin_core::*;

fn state() -> AppState {
    let mut state = AppState::new(
        Bootstrap {
            projects: vec![Project {
                id: ProjectId(1),
                name: "project".into(),
                path: "/project".into(),
            }],
            models: vec![],
            conversations: vec![
                (ConversationId(1), ProjectId(1), "one".into(), 1),
                (ConversationId(2), ProjectId(1), "two".into(), 2),
            ],
            now: 0,
        },
        Prefs::default(),
    );
    state.dispatch(Command::SelectConversation(ConversationId(1)));
    state
}

#[test]
fn navigation_is_session_scoped_and_restores_each_unsent_draft() {
    let mut s = state();
    s.restore_prompt_history(ConversationId(1), vec!["first".into(), "second".into()]);
    s.restore_prompt_history(ConversationId(2), vec!["other session".into()]);
    s.restore_prompt_history(ConversationId(999), vec!["not a session".into()]);
    s.dispatch(Command::EditDraft("unfinished one".into()));
    let epoch = s.current().unwrap().draft.sync_epoch;
    let result = s.dispatch(Command::PreviousPrompt);
    assert!(result.effects.is_empty(), "recall must never send a prompt");
    assert_eq!(s.current().unwrap().draft.text, "second");
    assert!(s.current().unwrap().draft.sync_epoch > epoch);
    assert_eq!(s.current().unwrap().draft.save, SaveState::Dirty);

    s.dispatch(Command::SelectConversation(ConversationId(2)));
    s.dispatch(Command::EditDraft("unfinished two".into()));
    s.dispatch(Command::PreviousPrompt);
    assert_eq!(s.current().unwrap().draft.text, "other session");
    s.dispatch(Command::NextPrompt);
    assert_eq!(s.current().unwrap().draft.text, "unfinished two");

    s.dispatch(Command::SelectConversation(ConversationId(1)));
    assert_eq!(s.current().unwrap().draft.text, "second");
    s.dispatch(Command::PreviousPrompt);
    assert_eq!(s.current().unwrap().draft.text, "first");
    s.dispatch(Command::NextPrompt);
    assert_eq!(s.current().unwrap().draft.text, "second");
    s.dispatch(Command::NextPrompt);
    assert_eq!(s.current().unwrap().draft.text, "unfinished one");
}

#[test]
fn editing_recalled_text_does_not_edit_history_or_discard_the_new_draft() {
    let mut s = state();
    s.restore_prompt_history(ConversationId(1), vec!["sent".into()]);
    s.dispatch(Command::PreviousPrompt);
    s.dispatch(Command::EditDraft("edited recalled prompt".into()));
    s.dispatch(Command::NextPrompt);
    assert_eq!(s.current().unwrap().draft.text, "edited recalled prompt");
    s.dispatch(Command::PreviousPrompt);
    assert_eq!(s.current().unwrap().draft.text, "sent");
    s.dispatch(Command::NextPrompt);
    assert_eq!(s.current().unwrap().draft.text, "edited recalled prompt");
}

#[test]
fn recall_keeps_draft_attachments_and_does_not_reuse_historical_attachments() {
    let mut s = state();
    s.restore_prompt_history(ConversationId(1), vec!["sent".into()]);
    let attachment = Attachment {
        path: "/project/new.txt".into(),
        name: "new.txt".into(),
        size: Some(12),
        error: None,
    };
    s.dispatch(Command::AddAttachments(vec![attachment.clone()]));
    s.dispatch(Command::PreviousPrompt);
    assert_eq!(s.current().unwrap().draft.attachments, vec![attachment]);
}

#[test]
fn transcript_prompts_are_available_and_empty_history_is_a_noop() {
    let mut s = state();
    assert!(s.dispatch(Command::PreviousPrompt).effects.is_empty());
    assert!(s.current().unwrap().draft.text.is_empty());
    let generation = s.current().unwrap().generation;
    let user = |id, text: &str| TranscriptItem {
        native_only: false,
        id: ItemId(id),
        at: 0,
        kind: ItemKind::User {
            text: text.into(),
            attachments: vec![],
            delivery: Delivery::Sent,
            steer: false,
        },
    };
    s.apply_event(BackendEvent {
        conversation: ConversationId(1),
        generation,
        op: None,
        kind: EventKind::Opened {
            items: vec![user(1, "first"), user(2, "latest")],
            has_older: false,
            changes: vec![],
        },
    });
    s.dispatch(Command::PreviousPrompt);
    assert_eq!(s.current().unwrap().draft.text, "latest");
    s.dispatch(Command::PreviousPrompt);
    assert_eq!(s.current().unwrap().draft.text, "first");
}
