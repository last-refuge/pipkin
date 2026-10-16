use super::*;

fn providers() -> AppState {
    let mut s = state();
    s.local_cli.available = true;
    s.models = vec![
        ModelInfo {
            id: "native/gpt".into(),
            name: "GPT".into(),
            note: "Native provider".into(),
        },
        ModelInfo {
            id: "native/claude".into(),
            name: "Claude subscription".into(),
            note: "Native provider".into(),
        },
    ];
    s.prefs.model = Some("native/gpt".into());
    event(
        &mut s,
        None,
        EventKind::ConversationModel(Some(ProviderModel::Native("native/gpt".into()))),
    );
    s
}

#[test]
fn unified_catalog_has_native_models_and_distinct_typed_cli_aliases() {
    let mut s = providers();
    s.models.push(ModelInfo {
        id: "claude-cli/sonnet".into(),
        name: "custom native".into(),
        note: "native".into(),
    });
    let rows = s.provider_models();
    assert_eq!(rows.len(), 6);
    assert!(
        rows.iter()
            .any(|m| m.selection == ProviderModel::Native("claude-cli/sonnet".into()))
    );
    assert!(
        rows.iter()
            .any(|m| m.selection == ProviderModel::ClaudeCli(CliModel::Sonnet))
    );
    s.mode = Mode::Demo;
    assert_eq!(s.provider_models().len(), 3);
}

#[test]
fn failed_or_cancelled_cli_choice_blocks_only_source_and_preserves_its_draft() {
    let mut s = providers();
    s.dispatch(Command::SelectProviderModel(ProviderModel::ClaudeCli(
        CliModel::Opus,
    )));
    assert_eq!(s.local_cli.draft.model, CliModel::Opus);
    s.dispatch(Command::LocalCli(LocalCliCommand::Cancel));
    assert!(!s.availability().submit);
    no_execution(&s.dispatch(Command::Submit));
    assert_eq!(
        s.current_execution_profile(),
        Some(ExecutionProfile::NativeAgent)
    );
    assert_eq!(s.current().unwrap().draft.text, "explicit text");
    s.dispatch(Command::SelectConversation(B));
    let generation = s.conversation(B).unwrap().generation;
    for kind in [
        EventKind::Opened {
            items: vec![],
            has_older: false,
            changes: vec![],
        },
        EventKind::ExecutionContext {
            profile: ExecutionProfile::NativeAgent,
            execution_blocked: None,
        },
        EventKind::ConversationModel(Some(ProviderModel::Native("native/gpt".into()))),
    ] {
        s.apply_event(BackendEvent {
            conversation: B,
            generation,
            op: None,
            kind,
        });
    }
    s.local_cli.available = false;
    assert_eq!(
        s.next_provider_model(),
        Some(ProviderModel::Native("native/gpt".into()))
    );
    assert!(
        s.availability().new_conversation,
        "New on this native conversation does not depend on the other conversation's CLI choice"
    );
    s.dispatch(Command::EditDraft("other conversation".into()));
    assert!(
        s.availability().submit,
        "another provider/conversation stays usable"
    );
    let request = request(s.dispatch(Command::Submit));
    let out = s.intent_persisted(B, &request, Ok(()));
    assert!(out.effects.iter().any(|e| matches!(e, Effect::Backend(BackendRequest::Submit { conversation: B, model: Some(model), .. }) if model == "native/gpt")));
    s.dispatch(Command::SelectConversation(A));
    s.dispatch(Command::KeepConversationProvider);
    assert!(!s.provider_selection_pending(A));
}

#[test]
fn restart_retains_only_scoped_intent_not_global_exclusion_or_authority() {
    let mut before = providers();
    before.dispatch(Command::SelectProviderModel(ProviderModel::ClaudeCli(
        CliModel::Haiku,
    )));
    let mut restarted = providers();
    restarted.prefs = before.prefs.clone();
    assert!(restarted.provider_selection_pending(A));
    assert!(!restarted.provider_selection_pending(B));
    assert!(!restarted.availability().submit);
    assert!(restarted.local_cli.ticket.is_none() && !restarted.local_cli.trust_executable);
    restarted.local_cli.available = false;
    no_execution(&restarted.dispatch(Command::NewConversation));
    assert!(matches!(
        restarted.prefs.provider_model,
        Some(ProviderModel::ClaudeCli(CliModel::Haiku))
    ));
}

#[test]
fn changing_provider_during_journaling_cancels_unsent_intent_without_losing_text() {
    let mut s = providers();
    let request = request(s.dispatch(Command::Submit));
    s.dispatch(Command::SelectProviderModel(ProviderModel::ClaudeCli(
        CliModel::Sonnet,
    )));
    let out = s.intent_persisted(A, &request, Ok(()));
    no_execution(&out);
    assert!(out.effects.iter().any(|e| matches!(
        e,
        Effect::JournalState {
            state: JournalState::Cancelled,
            ..
        }
    )));
    assert_eq!(s.current().unwrap().draft.text, "explicit text");
}

#[test]
fn selection_persists_one_snapshot_with_its_source_guard_not_intermediate_keep_state() {
    let mut s = providers();
    s.dispatch(Command::SelectProviderModel(ProviderModel::ClaudeCli(
        CliModel::Sonnet,
    )));
    let out = s.dispatch(Command::SelectProviderModel(ProviderModel::Native(
        "native/claude".into(),
    )));
    let prefs: Vec<_> = out
        .effects
        .iter()
        .filter_map(|e| {
            if let Effect::SavePrefs(p) = e {
                Some(p)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(prefs.len(), 1);
    assert_eq!(
        prefs[0].pending_provider_models.get(&A.0),
        Some(&ProviderModel::Native("native/claude".into()))
    );
}

#[test]
fn ordinary_new_with_same_model_does_not_cancel_a_source_prompt_waiting_for_journal() {
    let mut s = providers();
    let req = request(s.dispatch(Command::Submit));
    s.dispatch(Command::NewConversation);
    assert!(!s.provider_selection_pending(A));
    let out = s.intent_persisted(A, &req, Ok(()));
    assert!(out.effects.iter().any(|e| matches!(e, Effect::Backend(BackendRequest::Submit { conversation: A, model: Some(model), .. }) if model == "native/gpt")));
}

#[test]
fn model_ack_or_explicit_keep_cannot_revive_an_old_journaled_prompt() {
    for keep in [false, true] {
        let mut s = providers();
        let req = request(s.dispatch(Command::Submit));
        s.dispatch(Command::SelectProviderModel(ProviderModel::Native(
            "native/claude".into(),
        )));
        if keep {
            s.dispatch(Command::KeepConversationProvider);
        } else {
            event(
                &mut s,
                None,
                EventKind::ConversationModel(Some(ProviderModel::Native("native/claude".into()))),
            );
        }
        assert!(!s.provider_selection_pending(A));
        let out = s.intent_persisted(A, &req, Ok(()));
        no_execution(&out);
        assert!(out.effects.iter().any(|e| matches!(
            e,
            Effect::JournalState {
                state: JournalState::Cancelled,
                ..
            }
        )));
        assert_eq!(s.current().unwrap().draft.text, "explicit text");
    }
}

#[test]
fn native_model_choice_waits_for_scoped_authoritative_ack_not_a_local_label() {
    let mut s = providers();
    s.dispatch(Command::SelectProviderModel(ProviderModel::Native(
        "native/claude".into(),
    )));
    assert!(!s.availability().submit);
    let current = s.conversation(A).unwrap().generation;
    s.apply_event(BackendEvent {
        conversation: A,
        generation: current + 1,
        op: None,
        kind: EventKind::ConversationModel(Some(ProviderModel::Native("native/claude".into()))),
    });
    assert!(!s.availability().submit);
    event(
        &mut s,
        Some(OperationId(123)),
        EventKind::ConversationModel(Some(ProviderModel::Native("native/claude".into()))),
    );
    assert!(!s.availability().submit);
    event(
        &mut s,
        None,
        EventKind::ConversationModel(Some(ProviderModel::Native("native/claude".into()))),
    );
    assert!(s.availability().submit);
    let req = request(s.dispatch(Command::Submit));
    let out = s.intent_persisted(A, &req, Ok(()));
    assert!(out.effects.iter().any(|e| matches!(e, Effect::Backend(BackendRequest::Submit { model: Some(model), .. }) if model == "native/claude")));
}

#[test]
fn cli_to_native_creates_exact_new_native_session_and_does_not_mutate_cli() {
    let mut s = providers();
    context(&mut s, ExecutionProfile::LocalCliChatOnly, false);
    event(
        &mut s,
        None,
        EventKind::ConversationModel(Some(ProviderModel::ClaudeCli(CliModel::Opus))),
    );
    let out = s.dispatch(Command::SelectProviderModel(ProviderModel::Native(
        "native/gpt".into(),
    )));
    let req = out
        .effects
        .into_iter()
        .find_map(|e| match e {
            Effect::Backend(BackendRequest::CreateConversation {
                model: Some(model),
                request,
                ..
            }) => {
                assert_eq!(model, "native/gpt");
                Some(request)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(
        s.current_execution_profile(),
        Some(ExecutionProfile::LocalCliChatOnly)
    );
    let c = ConversationId(3);
    let d = ConversationId(4);
    s.apply_catalog(Bootstrap {
        projects: s.projects.clone(),
        models: s.models.clone(),
        conversations: vec![
            (A, ProjectId(1), "cli".into(), 1),
            (B, ProjectId(1), "native".into(), 2),
            (c, ProjectId(1), "created".into(), 3),
            (d, ProjectId(1), "unrelated newer session".into(), 99),
        ],
        now: 4,
    });
    assert_eq!(
        s.selected,
        Some(A),
        "catalog arrival is not a creation receipt"
    );
    s.native_conversation_created(&RequestId("stale".into()), d);
    assert_eq!(s.selected, Some(A));
    let out = s.native_conversation_created(&req, c);
    let prefs: Vec<_> = out
        .effects
        .iter()
        .filter_map(|e| {
            if let Effect::SavePrefs(p) = e {
                Some(p)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(prefs.len(), 1);
    assert_eq!(prefs[0].selected_conversation, Some(c));
    assert!(!prefs[0].pending_provider_models.contains_key(&A.0));
    assert_eq!(s.selected, Some(c));
    assert!(!s.provider_selection_pending(A));
    assert_eq!(s.conversation(A).unwrap().draft.text, "explicit text");
}

#[test]
fn keeping_source_provider_prevents_late_native_creation_from_stealing_selection() {
    let mut s = providers();
    context(&mut s, ExecutionProfile::LocalCliChatOnly, false);
    event(
        &mut s,
        None,
        EventKind::ConversationModel(Some(ProviderModel::ClaudeCli(CliModel::Sonnet))),
    );
    let out = s.dispatch(Command::SelectProviderModel(ProviderModel::Native(
        "native/gpt".into(),
    )));
    let req = out
        .effects
        .into_iter()
        .find_map(|e| match e {
            Effect::Backend(BackendRequest::CreateConversation { request, .. }) => Some(request),
            _ => None,
        })
        .unwrap();
    s.dispatch(Command::KeepConversationProvider);
    s.native_conversation_created(&req, B);
    assert_eq!(s.selected, Some(A));
    assert_eq!(
        s.next_provider_model(),
        Some(ProviderModel::ClaudeCli(CliModel::Sonnet))
    );
}

#[test]
fn blank_workspace_provider_intent_does_not_auto_select_an_unrelated_catalog_row() {
    for choice in [
        ProviderModel::ClaudeCli(CliModel::Sonnet),
        ProviderModel::Native("native/gpt".into()),
    ] {
        let mut s = providers();
        s.selected = None;
        s.prefs.selected_conversation = None;
        s.prefs.provider_model = Some(choice);
        no_execution(&s.select_initial());
        assert!(s.selected.is_none());
        s.prefs.selected_conversation = Some(ConversationId(999));
        no_execution(&s.select_initial());
        assert!(
            s.selected.is_none(),
            "missing saved target is not permission to choose another session"
        );
        // Explicit navigation is still available; this is not a global provider mode.
        s.dispatch(Command::SelectConversation(B));
        assert_eq!(s.selected, Some(B));
    }
}

#[test]
fn native_default_without_model_identity_does_not_inherit_another_chats_cli_choice() {
    let mut s = providers();
    event(&mut s, None, EventKind::ConversationModel(None));
    s.prefs.model = None;
    s.prefs.provider_model = Some(ProviderModel::ClaudeCli(CliModel::Opus));
    assert_eq!(s.next_provider_model(), None);
    let out = s.dispatch(Command::NewConversation);
    assert!(out.effects.iter().any(|e| matches!(
        e,
        Effect::Backend(BackendRequest::CreateConversation { model: None, .. })
    )));
}

#[test]
fn stale_or_unavailable_catalog_choice_is_not_silently_replaced() {
    let mut s = providers();
    s.local_cli.available = false;
    let out = s.dispatch(Command::SelectProviderModel(ProviderModel::ClaudeCli(
        CliModel::Sonnet,
    )));
    no_execution(&out);
    assert!(!s.availability().submit);
    assert_eq!(
        s.current_execution_profile(),
        Some(ExecutionProfile::NativeAgent)
    );
}
