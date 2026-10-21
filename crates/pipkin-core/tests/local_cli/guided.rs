use super::*;

#[test]
fn configured_cli_model_connects_without_mode_selection_or_another_approval() {
    let mut s = state();
    s.prefs.cli_provider_configured = true;
    s.prefs.cli_executable = Some("/bin/claude".into());
    s.prefs.cli_mode = CliMode::ChatOnly; // legacy preference cannot select chat for new work
    let discovery = call(cmd(&mut s, LocalCliCommand::BeginGuided));
    assert_eq!(s.local_cli.draft.mode, CliMode::Coding);
    let inspect = call(answer(
        &mut s,
        &discovery,
        LocalCliEvent::Discovered(vec!["/other/claude".into(), "/bin/claude".into()]),
    ));
    let mut ticket = view(&inspect);
    ticket.executable = "/bin/claude".into();
    let check = call(answer(
        &mut s,
        &inspect,
        LocalCliEvent::Inspected(ticket.clone()),
    ));
    assert!(matches!(
        check.request,
        LocalCliRequest::Check {
            trust_executable: true,
            trust_project: true,
            ..
        }
    ));
    ticket.stage = LocalCliMetadataStage::MetadataVerified;
    let out = answer(&mut s, &check, LocalCliEvent::Checked(ticket));
    assert!(
        out.effects
            .iter()
            .any(|effect| matches!(effect, Effect::JournalLocalCliCreation { .. }))
    );
}

#[test]
fn configured_cli_never_executes_a_newly_discovered_different_candidate() {
    let mut s = state();
    s.prefs.cli_provider_configured = true;
    s.prefs.cli_executable = Some("/previous/claude".into());
    let discovery = call(cmd(&mut s, LocalCliCommand::BeginGuided));
    let inspect = call(answer(
        &mut s,
        &discovery,
        LocalCliEvent::Discovered(vec!["/different/claude".into()]),
    ));
    let ticket = view(&inspect);
    no_effect(answer(&mut s, &inspect, LocalCliEvent::Inspected(ticket)));
    assert_eq!(s.local_cli.stage, LocalCliStage::AwaitingTrust);
}

fn guided_review(s: &mut AppState) -> LocalCliSetupView {
    let discovery = call(cmd(s, LocalCliCommand::BeginGuided));
    assert!(
        matches!(discovery.request, LocalCliRequest::Discover { ref project, .. } if project == "/project")
    );
    let inspect = call(answer(
        s,
        &discovery,
        LocalCliEvent::Discovered(vec!["/bin/claude".into()]),
    ));
    let v = view(&inspect);
    no_effect(answer(s, &inspect, LocalCliEvent::Inspected(v.clone())));
    assert!(!s.local_cli.trust_executable && !s.local_cli.trust_project);
    v
}

#[test]
fn provider_configuration_entry_cannot_replace_pending_conversation_setup() {
    let mut s = state();
    let ticket = guided_review(&mut s);
    let revision = s.local_cli.input_revision();
    assert!(s.pending_cli_conversation_setup());
    no_effect(cmd(&mut s, LocalCliCommand::BeginProviderSetup));
    assert!(!s.local_cli.provider_only);
    assert_eq!(s.local_cli.input_revision(), revision);
    assert_eq!(s.local_cli.ticket.as_ref(), Some(&ticket));
    let check = call(cmd(
        &mut s,
        LocalCliCommand::ConfirmAndConnect {
            setup: ticket.setup.clone(),
        },
    ));
    let mut verified = ticket;
    verified.stage = LocalCliMetadataStage::MetadataVerified;
    let out = answer(&mut s, &check, LocalCliEvent::Checked(verified));
    assert!(
        out.effects
            .iter()
            .any(|e| matches!(e, Effect::JournalLocalCliCreation { .. }))
    );
}

#[test]
fn rediscovery_retires_old_ticket_and_consent_without_changing_setup_purpose() {
    let mut s = state();
    guided_review(&mut s);
    let check = check(&mut s);
    no_effect(answer(
        &mut s,
        &check,
        LocalCliEvent::Failed {
            phase: LocalCliPhase::Check,
            failure: LocalCliFailure::Rejected("unsupported version".into()),
        },
    ));
    assert!(s.local_cli_availability().inspect);
    let revision = s.local_cli.input_revision();
    let mut out = cmd(&mut s, LocalCliCommand::Discover);
    assert!(s.local_cli.discovering && !s.local_cli.provider_only);
    assert!(
        s.local_cli.ticket.is_none() && !s.local_cli.trust_executable && !s.local_cli.trust_project
    );
    assert_ne!(s.local_cli.input_revision(), revision);
    assert!(out.effects.iter().all(|e| matches!(
        e,
        Effect::Backend(BackendRequest::LocalCli {
            request: LocalCliRequest::Discover { .. } | LocalCliRequest::Revoke { .. },
            ..
        })
    )));
    out.effects.retain(|e| {
        matches!(
            e,
            Effect::Backend(BackendRequest::LocalCli {
                request: LocalCliRequest::Discover { .. },
                ..
            })
        )
    });
    let discovery = call(out);
    no_effect(answer(
        &mut s,
        &discovery,
        LocalCliEvent::Discovered(vec![]),
    ));
    assert!(!s.local_cli_availability().trust);
}

#[test]
fn configuring_cli_provider_never_selects_it_or_disables_native_conversations() {
    for success in [false, true] {
        let mut s = state();
        s.selected = Some(ConversationId(1));
        s.conversations[0].opened = true;
        s.apply_event(BackendEvent {
            conversation: ConversationId(1),
            generation: s.conversations[0].generation,
            op: None,
            kind: EventKind::ExecutionContext {
                profile: ExecutionProfile::NativeAgent,
                execution_blocked: None,
            },
        });
        s.dispatch(Command::EditDraft("native draft".into()));
        let discovery = call(cmd(&mut s, LocalCliCommand::BeginProviderSetup));
        let inspect = call(answer(
            &mut s,
            &discovery,
            LocalCliEvent::Discovered(vec!["/bin/claude".into()]),
        ));
        let mut ticket = view(&inspect);
        answer(&mut s, &inspect, LocalCliEvent::Inspected(ticket.clone()));
        let check = call(cmd(
            &mut s,
            LocalCliCommand::ConfirmAndConnect {
                setup: ticket.setup.clone(),
            },
        ));
        let event = if success {
            ticket.stage = LocalCliMetadataStage::MetadataVerified;
            LocalCliEvent::Checked(ticket)
        } else {
            LocalCliEvent::Failed {
                phase: LocalCliPhase::Check,
                failure: LocalCliFailure::Rejected("unsupported version".into()),
            }
        };
        let out = answer(&mut s, &check, event);
        assert!(!out.effects.iter().any(|e| matches!(
            e,
            Effect::Backend(_) | Effect::JournalLocalCliCreation { .. }
        )));
        assert!(s.availability().submit);
        assert!(s.prefs.provider_model.is_none() && s.prefs.pending_provider_models.is_empty());
        assert_eq!(s.prefs.cli_provider_configured, success);
        assert!(!s.local_cli_availability().create);
    }
}

#[test]
fn guided_setup_needs_no_typed_values_but_cannot_probe_before_scoped_confirmation() {
    let mut s = state();
    let mut v = guided_review(&mut s);
    no_effect(cmd(
        &mut s,
        LocalCliCommand::ConfirmAndConnect {
            setup: SetupId("stale".into()),
        },
    ));
    let check = call(cmd(
        &mut s,
        LocalCliCommand::ConfirmAndConnect {
            setup: v.setup.clone(),
        },
    ));
    assert!(matches!(
        check.request,
        LocalCliRequest::Check {
            trust_executable: true,
            trust_project: true,
            ..
        }
    ));
    assert_eq!(s.prefs.cli_executable.as_deref(), Some("/canonical/claude"));
    v.stage = LocalCliMetadataStage::MetadataVerified;
    let checked = LocalCliEvent::Checked(v);
    let out = answer(&mut s, &check, checked.clone());
    assert_eq!(out.effects.len(), 2);
    assert!(matches!(&out.effects[1], Effect::SavePrefs(p) if p.cli_provider_configured));
    let Effect::JournalLocalCliCreation {
        conversation,
        generation,
        op,
        identity,
    } = &out.effects[0]
    else {
        panic!("Create must be durable before dispatch")
    };
    let create =
        call(s.local_cli_creation_persisted(*conversation, *generation, *op, identity, Ok(())));
    assert!(matches!(create.request, LocalCliRequest::Create { .. }));
    no_effect(answer(&mut s, &check, checked));
    let mut restarted = AppState::new(boot(&[1]), s.prefs.clone());
    restarted.mode = Mode::Real;
    restarted.local_cli.available = true;
    assert!(!restarted.local_cli.trust_executable && restarted.local_cli.ticket.is_none());
    let discovery = call(cmd(&mut restarted, LocalCliCommand::BeginGuided));
    assert!(matches!(
        discovery.request,
        LocalCliRequest::Discover { saved: Some(_), .. }
    ));
}

#[test]
fn failed_cli_check_keeps_native_composer_blocked_and_never_creates_a_native_chat() {
    let mut s = state();
    s.selected = Some(ConversationId(1));
    s.conversations[0].opened = true;
    s.apply_event(BackendEvent {
        conversation: ConversationId(1),
        generation: s.conversations[0].generation,
        op: None,
        kind: EventKind::ExecutionContext {
            profile: ExecutionProfile::NativeAgent,
            execution_blocked: None,
        },
    });
    s.dispatch(Command::EditDraft("which model are you?".into()));
    assert!(s.availability().submit);
    let ticket = guided_review(&mut s);
    let check = call(cmd(
        &mut s,
        LocalCliCommand::ConfirmAndConnect {
            setup: ticket.setup,
        },
    ));
    no_effect(answer(
        &mut s,
        &check,
        LocalCliEvent::Failed {
            phase: LocalCliPhase::Check,
            failure: LocalCliFailure::Rejected("Unsupported Claude Code version".into()),
        },
    ));
    assert_eq!(s.local_cli.stage, LocalCliStage::Failed);
    assert!(!s.availability().submit);
    no_effect(s.dispatch(Command::Submit));
    assert_eq!(s.current().unwrap().draft.text, "which model are you?");
    assert_eq!(
        s.current_execution_profile(),
        Some(ExecutionProfile::NativeAgent)
    );
    assert_eq!(
        s.prefs.provider_model,
        Some(ProviderModel::ClaudeCli(CliModel::Sonnet))
    );
}

#[test]
fn keeping_source_provider_retires_only_its_inflight_check_continuation() {
    let mut s = state();
    let mut ticket = guided_review(&mut s);
    let check = call(cmd(
        &mut s,
        LocalCliCommand::ConfirmAndConnect {
            setup: ticket.setup.clone(),
        },
    ));
    let out = s.dispatch(Command::KeepConversationProvider);
    assert!(out.effects.iter().any(|e| matches!(
        e,
        Effect::Backend(BackendRequest::LocalCli {
            request: LocalCliRequest::Revoke { .. },
            ..
        })
    )));
    ticket.stage = LocalCliMetadataStage::MetadataVerified;
    no_effect(answer(&mut s, &check, LocalCliEvent::Checked(ticket)));
    assert!(!s.provider_selection_pending(ConversationId(1)));
    assert!(s.local_cli.creation.is_none());
}

#[test]
fn ambiguous_discovery_and_late_picker_generations_cannot_choose_or_grant() {
    let mut s = state();
    let first = call(cmd(&mut s, LocalCliCommand::BeginGuided));
    no_effect(answer(
        &mut s,
        &first,
        LocalCliEvent::Discovered(vec!["/a/claude".into(), "/b/claude".into()]),
    ));
    assert!(s.local_cli.draft.executable.is_empty());
    let old_revision = s.local_cli.input_revision();
    let mut d = s.local_cli.draft.clone();
    d.executable = "/b/claude".into();
    let inspection = call(cmd(&mut s, LocalCliCommand::Configure(d)));
    assert!(s.local_cli.input_revision() > old_revision);
    no_effect(answer(
        &mut s,
        &first,
        LocalCliEvent::Discovered(vec!["/attacker".into()]),
    ));
    assert_eq!(s.local_cli.draft.executable, "/b/claude");
    assert!(matches!(
        inspection.request,
        LocalCliRequest::Inspect { .. }
    ));
    assert!(!s.local_cli.trust_project);
}

#[test]
fn cancellation_and_unknown_check_retire_the_one_shot_continuation() {
    for cancel in [false, true] {
        let mut s = state();
        let mut v = guided_review(&mut s);
        let check = call(cmd(
            &mut s,
            LocalCliCommand::ConfirmAndConnect {
                setup: v.setup.clone(),
            },
        ));
        v.stage = LocalCliMetadataStage::MetadataVerified;
        if cancel {
            cmd(&mut s, LocalCliCommand::Cancel);
            no_effect(answer(&mut s, &check, LocalCliEvent::Checked(v)));
        } else {
            answer(
                &mut s,
                &check,
                LocalCliEvent::Failed {
                    phase: LocalCliPhase::Check,
                    failure: LocalCliFailure::Unknown("lost".into()),
                },
            );
            let status = call(cmd(&mut s, LocalCliCommand::Status));
            no_effect(answer(&mut s, &status, LocalCliEvent::Status(v)));
            assert_eq!(s.local_cli.stage, LocalCliStage::Verified);
        }
        assert!(s.local_cli.creation.is_none());
    }
}

#[test]
fn starting_another_guided_setup_revokes_unfinished_confirmation_and_ignores_its_check() {
    let mut s = state();
    let mut v = guided_review(&mut s);
    let check = call(cmd(
        &mut s,
        LocalCliCommand::ConfirmAndConnect {
            setup: v.setup.clone(),
        },
    ));
    let next = cmd(&mut s, LocalCliCommand::BeginGuided);
    assert!(next.effects.iter().any(|e| matches!(e, Effect::Backend(BackendRequest::LocalCli { request: LocalCliRequest::Revoke { setup }, .. }) if setup == &v.setup)));
    assert!(next.effects.iter().any(|e| matches!(
        e,
        Effect::Backend(BackendRequest::LocalCli {
            request: LocalCliRequest::Discover { .. },
            ..
        })
    )));
    v.stage = LocalCliMetadataStage::MetadataVerified;
    no_effect(answer(&mut s, &check, LocalCliEvent::Checked(v)));
    assert!(s.local_cli.creation.is_none() && !s.local_cli.trust_project);
}

#[test]
fn reconnect_loads_hints_but_waits_for_confirmation_and_returns_only_to_captured_session() {
    let mut s = state();
    existing(&mut s);
    let hints = call(cmd(
        &mut s,
        LocalCliCommand::BeginGuidedExisting {
            conversation: ConversationId(1),
            session: EngineSessionId("saved-engine-session".into()),
        },
    ));
    let inspect = call(answer(
        &mut s,
        &hints,
        LocalCliEvent::SessionStatus(LocalCliSessionView {
            session: EngineSessionId("saved-engine-session".into()),
            mode: CliMode::ChatOnly,
            executable: "/saved/claude".into(),
            project: "/project".into(),
            model: Some(CliModel::Opus),
            context_valid: true,
        }),
    ));
    assert!(!s.local_cli.trust_executable);
    let mut v = view(&inspect);
    no_effect(answer(
        &mut s,
        &inspect,
        LocalCliEvent::Inspected(v.clone()),
    ));
    let check = call(cmd(
        &mut s,
        LocalCliCommand::ConfirmAndConnect {
            setup: v.setup.clone(),
        },
    ));
    s.selected = Some(ConversationId(2));
    v.stage = LocalCliMetadataStage::MetadataVerified;
    let out = answer(&mut s, &check, LocalCliEvent::Checked(v));
    assert!(out.effects.iter().any(|e| matches!(
        e,
        Effect::Backend(BackendRequest::Open {
            conversation: ConversationId(1),
            ..
        })
    )));
    assert!(
        !out.effects
            .iter()
            .any(|e| matches!(e, Effect::JournalLocalCliCreation { .. }))
    );
    assert_eq!(s.local_cli.stage, LocalCliStage::Created);
}

#[test]
fn unsupported_saved_model_does_not_fall_back_to_a_default_or_probe() {
    let mut s = state();
    existing(&mut s);
    let hints = call(cmd(
        &mut s,
        LocalCliCommand::BeginGuidedExisting {
            conversation: ConversationId(1),
            session: EngineSessionId("saved-engine-session".into()),
        },
    ));
    no_effect(answer(
        &mut s,
        &hints,
        LocalCliEvent::SessionStatus(LocalCliSessionView {
            session: EngineSessionId("saved-engine-session".into()),
            mode: CliMode::ChatOnly,
            executable: "/saved/claude".into(),
            project: "/project".into(),
            model: None,
            context_valid: true,
        }),
    ));
    assert_eq!(s.local_cli.stage, LocalCliStage::Failed);
    assert!(s.local_cli.error.is_some() && s.local_cli.ticket.is_none());
    assert!(!s.local_cli.trust_executable && s.local_cli.draft.executable.is_empty());
}

#[test]
fn configuration_edits_never_reuse_a_previously_confirmed_ticket() {
    let mut s = state();
    let v = guided_review(&mut s);
    let check = call(cmd(
        &mut s,
        LocalCliCommand::ConfirmAndConnect {
            setup: v.setup.clone(),
        },
    ));
    let mut d = s.local_cli.draft.clone();
    d.model = CliModel::Opus;
    cmd(&mut s, LocalCliCommand::Configure(d));
    let mut verified = v;
    verified.stage = LocalCliMetadataStage::MetadataVerified;
    no_effect(answer(&mut s, &check, LocalCliEvent::Checked(verified)));
    assert!(!s.local_cli.trust_executable && s.local_cli.creation.is_none());
}

#[test]
fn dashboard_creation_keeps_main_context_and_attaches_exact_created_thread() {
    for catalog_first in [false, true] {
        let mut s = state();
        s.selected = Some(ConversationId(1));
        s.conversations[0].draft.text = "keep main draft".into();
        s.dashboard.open = true;
        s.dashboard.filter.text = Some("old search".into());
        let provider = s.prefs.provider_model.clone();
        let project = s.prefs.selected_project;
        let discovery = call(cmd(
            &mut s,
            LocalCliCommand::BeginDashboardNew(LocalCliDraft {
                project: "/another-project".into(),
                model: CliModel::Opus,
                mode: CliMode::Coding,
                ..Default::default()
            }),
        ));
        assert!(
            matches!(&discovery.request, LocalCliRequest::Discover { project, .. } if project == "/another-project")
        );
        let inspect = call(answer(
            &mut s,
            &discovery,
            LocalCliEvent::Discovered(vec!["/bin/claude".into()]),
        ));
        assert!(
            matches!(&inspect.request, LocalCliRequest::Inspect { target: LocalCliInspectTarget::New { project, model: CliModel::Opus, .. }, .. } if project == "/another-project")
        );
        let mut ticket = view(&inspect);
        ticket.project = "/another-project".into();
        answer(&mut s, &inspect, LocalCliEvent::Inspected(ticket.clone()));
        let check = call(cmd(
            &mut s,
            LocalCliCommand::ConfirmAndConnect {
                setup: ticket.setup.clone(),
            },
        ));
        ticket.stage = LocalCliMetadataStage::MetadataVerified;
        let checked = answer(&mut s, &check, LocalCliEvent::Checked(ticket));
        let Effect::JournalLocalCliCreation {
            conversation,
            generation,
            op,
            identity,
        } = checked
            .effects
            .iter()
            .find(|e| matches!(e, Effect::JournalLocalCliCreation { .. }))
            .unwrap()
        else {
            panic!()
        };
        let create =
            call(s.local_cli_creation_persisted(*conversation, *generation, *op, identity, Ok(())));
        let LocalCliRequest::Create { identity } = &create.request else {
            panic!()
        };
        if catalog_first {
            s.apply_catalog(boot(&[1, 2, 3]));
        }
        let mut result = answer(
            &mut s,
            &create,
            LocalCliEvent::Created(LocalCliCreated {
                identity: identity.clone(),
                conversation: ConversationId(3),
            }),
        );
        if !catalog_first {
            result = s.apply_catalog(boot(&[1, 2, 3]));
        }
        assert_eq!(s.local_cli.stage, LocalCliStage::Created);
        assert!(s.dashboard.open);
        assert_eq!(s.selected, Some(ConversationId(1)));
        assert_eq!(s.conversations[0].draft.text, "keep main draft");
        assert_eq!(s.prefs.provider_model, provider);
        assert_eq!(s.prefs.selected_project, project);
        assert_eq!(s.dashboard.group, Some(dashboard::ActivityGroup::All));
        assert!(s.dashboard.filter.text.is_none());
        assert!(result.effects.iter().any(|e| matches!(
            e,
            Effect::Backend(BackendRequest::Dashboard(dashboard::Request::Attach {
                conversation: ConversationId(3),
                ..
            }))
        )));
    }
}

#[test]
fn dashboard_creation_rejects_invalid_entry() {
    let mut s = state();
    no_effect(cmd(&mut s, LocalCliCommand::BeginDashboardNew(draft())));
    s.dashboard.open = true;
    let mut invalid = draft();
    invalid.project = "relative".into();
    no_effect(cmd(&mut s, LocalCliCommand::BeginDashboardNew(invalid)));
    s.read_only = Some("test".into());
    no_effect(cmd(&mut s, LocalCliCommand::BeginDashboardNew(draft())));
}
