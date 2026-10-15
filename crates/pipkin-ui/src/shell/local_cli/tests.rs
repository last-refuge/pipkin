use super::*;
use gpui::{AppContext as _, EntityInputHandler, TestAppContext, VisualTestContext};
use pipkin_core::*;

fn state() -> AppState {
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
    s.local_cli.available = true;
    s
}
fn draft() -> LocalCliDraft {
    LocalCliDraft {
        mode: CliMode::ChatOnly,
        executable: "/explicit/claude".into(),
        project: "/explicit/project".into(),
        model: CliModel::Sonnet,
    }
}
fn ticket() -> LocalCliSetupView {
    LocalCliSetupView {
        mode: CliMode::ChatOnly,
        setup: SetupId("ui-test".into()),
        target: LocalCliInspectTarget::New {
            project: draft().project,
            model: CliModel::Sonnet,
        },
        root_candidate: Some(EngineSessionId("cli-ui-test".into())),
        executable: "/captured/claude".into(),
        project: "/captured/project".into(),
        qualified_version: "test metadata".into(),
        stage: LocalCliMetadataStage::AwaitingTrust,
        expires_at: None,
        reason: None,
    }
}
fn inspected() -> AppState {
    let mut s = state();
    s.dispatch(CoreCommand::LocalCli(C::BeginNew(draft())));
    let out = s.dispatch(CoreCommand::LocalCli(C::Inspect));
    reply(&mut s, out, LocalCliEvent::Inspected(ticket()));
    s
}
fn reply(s: &mut AppState, out: Outcome, event: LocalCliEvent) {
    let out = if let Some(Effect::JournalLocalCliCreation {
        conversation,
        generation,
        op,
        identity,
    }) = out.effects.first()
    {
        s.local_cli_creation_persisted(*conversation, *generation, *op, identity, Ok(()))
    } else {
        out
    };
    let (conversation, generation, op) = out
        .effects
        .into_iter()
        .find_map(|e| match e {
            Effect::Backend(BackendRequest::LocalCli {
                conversation,
                generation,
                op,
                ..
            }) => Some((conversation, generation, op)),
            _ => None,
        })
        .expect("explicit setup request");
    s.apply_event(BackendEvent {
        conversation,
        generation,
        op: Some(op),
        kind: EventKind::LocalCli(event),
    });
}

#[test]
fn availability_labels_and_demo_marker_are_honest() {
    let mut s = state();
    assert!(s.local_cli_availability().begin);
    assert!(availability_label(&s).contains("no automatic checks"));
    s.local_cli.available = false;
    assert!(!s.local_cli_availability().begin);
    assert!(availability_label(&s).contains("not advertised"));
    s.local_cli.available = true;
    s.connection = Connection::Offline("test".into());
    assert!(!s.local_cli_availability().begin);
    assert!(availability_label(&s).contains("Connect to the engine"));
    s.connection = Connection::Ready;
    s.read_only = Some("test read-only".into());
    assert!(!s.local_cli_availability().begin);
    assert!(availability_label(&s).contains("read-only"));
    s.read_only = None;
    s.mode = Mode::Demo;
    s.local_cli.available = true;
    assert!(!s.local_cli_availability().begin);
    assert!(availability_label(&s).contains("Demo · simulated agent"));
    let commands = super::super::commands::build(&s, None);
    assert!(
        !commands
            .iter()
            .find(|c| c.title == NEW_LABEL)
            .unwrap()
            .enabled
    );
    assert!(BILLING.contains("unverified") && BILLING.contains("already-enabled extra usage"));
    assert!(LEASE.contains("five minutes after Inspect"));
    assert!(LEASE.contains("stops active work") && LEASE.contains("fresh conversation"));
}

#[test]
fn both_trust_choices_are_explicit_and_edits_invalidate_them() {
    let mut s = inspected();
    assert!(!s.local_cli.trust_executable && !s.local_cli.trust_project);
    assert!(!s.local_cli_availability().check);
    s.dispatch(CoreCommand::LocalCli(C::TrustExecutable(true)));
    assert!(!s.local_cli_availability().check);
    s.dispatch(CoreCommand::LocalCli(C::TrustProject(true)));
    assert!(s.local_cli_availability().check);
    let mut changed = draft();
    changed.model = CliModel::Opus;
    s.dispatch(CoreCommand::LocalCli(C::Edit(changed)));
    assert!(s.local_cli.ticket.is_none());
    assert!(!s.local_cli.trust_executable && !s.local_cli.trust_project);
    assert!(!s.local_cli_availability().check);
}

#[test]
fn changing_mode_retires_trust_and_default_hosting_is_not_an_approval() {
    let mut s = inspected();
    s.dispatch(CoreCommand::LocalCli(C::TrustExecutable(true)));
    s.dispatch(CoreCommand::LocalCli(C::TrustProject(true)));
    let mut changed = draft();
    changed.mode = CliMode::RestrictedWrite;
    let out = s.dispatch(CoreCommand::LocalCli(C::Edit(changed)));
    assert!(out.effects.iter().all(|e| matches!(
        e,
        Effect::Backend(BackendRequest::LocalCli {
            request: LocalCliRequest::Revoke { .. },
            ..
        })
    )));
    assert!(s.local_cli.ticket.is_none());
    assert!(!s.local_cli.trust_executable && !s.local_cli.trust_project);
    let out = s.dispatch(CoreCommand::LocalCli(C::Inspect));
    assert!(out.effects.iter().any(|e| matches!(
        e,
        Effect::Backend(BackendRequest::LocalCli {
            request: LocalCliRequest::Inspect {
                mode: CliMode::RestrictedWrite,
                ..
            },
            ..
        })
    )));
    assert!(
        WRITE_LIMITS.contains("available by default") && WRITE_LIMITS.contains("fresh trust/check")
    );
    assert!(WRITE_LIMITS.contains("Attached engines control their own availability"));
    assert!(
        WRITE_LIMITS.contains("exact approval")
            && WRITE_LIMITS.contains("No Read/Edit/Bash/MCP parity")
    );
    assert!(WRITE_LIMITS.contains("no sandbox") && WRITE_LIMITS.contains("fresh conversation"));
}

#[test]
fn unknown_creation_offers_only_lookup_until_confirmed_absence() {
    let mut s = inspected();
    s.dispatch(CoreCommand::LocalCli(C::TrustExecutable(true)));
    s.dispatch(CoreCommand::LocalCli(C::TrustProject(true)));
    let check = s.dispatch(CoreCommand::LocalCli(C::Check));
    let mut verified = ticket();
    verified.stage = LocalCliMetadataStage::MetadataVerified;
    reply(&mut s, check, LocalCliEvent::Checked(verified));
    let create = s.dispatch(CoreCommand::LocalCli(C::Create));
    let identity = s.local_cli.creation.clone().unwrap();
    reply(
        &mut s,
        create,
        LocalCliEvent::Failed {
            phase: LocalCliPhase::Create,
            failure: LocalCliFailure::Unknown("lost response".into()),
        },
    );
    let a = s.local_cli_availability();
    assert!(a.lookup_creation && !a.repeat_create && !a.create && !a.begin);
    assert!(stage_label(s.local_cli.stage).contains("look up"));
    let lookup = s.dispatch(CoreCommand::LocalCli(C::LookupCreation));
    assert!(
        matches!(&lookup.effects[0], Effect::Backend(BackendRequest::LocalCli { request: LocalCliRequest::LookupCreation { identity: i }, .. }) if i == &identity)
    );
    reply(
        &mut s,
        lookup,
        LocalCliEvent::CreationLookup {
            identity: identity.clone(),
            found: None,
        },
    );
    assert!(s.local_cli_availability().repeat_create);
    let repeated = s.dispatch(CoreCommand::LocalCli(C::RepeatCreate));
    assert!(
        matches!(&repeated.effects[0], Effect::JournalLocalCliCreation { identity: i, .. } if i == &identity)
    );
}

#[gpui::test]
fn reapproval_captures_backend_id_and_metadata_never_unlocks_composer(cx: &mut TestAppContext) {
    cx.update(crate::text::init);
    let window = cx.add_window(|window, cx| {
        let mut s = AppState::new(
            Bootstrap {
                projects: vec![],
                models: vec![],
                now: 1,
                conversations: vec![
                    (ConversationId(41), ProjectId(1), "CLI".into(), 1),
                    (ConversationId(42), ProjectId(1), "Other".into(), 1),
                ],
            },
            Prefs::default(),
        );
        s.mode = Mode::Real;
        s.selected = Some(ConversationId(41));
        let c = s
            .conversations
            .iter_mut()
            .find(|c| c.id == ConversationId(41))
            .unwrap();
        let generation = c.generation;
        s.apply_event(BackendEvent {
            conversation: ConversationId(41),
            generation,
            op: None,
            kind: EventKind::ExecutionContext {
                profile: ExecutionProfile::LocalCliChatOnly,
                execution_blocked: Some("Needs authoritative execution approval".into()),
            },
        });
        let model = cx.new(|_| crate::model::Model::new(s));
        Workspace::new(model, Default::default(), window, cx)
    });
    let root = window.root(cx).unwrap();
    let cx = &mut VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    root.update_in(cx, |w, window, cx| {
        assert!(!w.local_cli_entry_enabled(true, cx));
        w.model.update(cx, |m, cx| m.set_local_cli_catalog(true, [(ConversationId(41), EngineSessionId("exact-backend-uuid".into()))], cx));
        assert!(w.local_cli_entry_enabled(true, cx));
        w.open_local_cli(true, window, cx);
        assert!(matches!(&w.state(cx).local_cli.target, Some(LocalCliTarget::Existing { conversation, session, .. }) if *conversation == ConversationId(41) && session.0 == "exact-backend-uuid"));
        w.model.update(cx, |m, cx| {
            m.state.local_cli.stage = LocalCliStage::Verified;
            cx.notify();
        });
    });
    cx.run_until_parked();
    root.update(cx, |w, cx| {
        assert!(w.composer.read(cx).is_disabled(), "metadata is not execution readiness");
        w.model.update(cx, |m, _| m.state.selected = Some(ConversationId(42)));
        let out = w.model.update(cx, |m, _| m.state.dispatch(CoreCommand::LocalCli(C::SessionStatus)));
        assert!(out.effects.iter().any(|e| matches!(e, Effect::Backend(BackendRequest::LocalCli { conversation, request: LocalCliRequest::SessionStatus { session }, .. }) if *conversation == ConversationId(41) && session.0 == "exact-backend-uuid")));
    });
}

#[gpui::test]
fn replacing_catalog_ids_notifies_even_when_setup_availability_is_unchanged(
    cx: &mut TestAppContext,
) {
    let model = cx.new(|_| crate::model::Model::new(state()));
    let count = std::rc::Rc::new(std::cell::Cell::new(0));
    let observed = count.clone();
    let _subscription =
        cx.update(|cx| cx.observe(&model, move |_, _| observed.set(observed.get() + 1)));
    model.update(cx, |m, cx| {
        m.set_local_cli_catalog(
            true,
            [(ConversationId(1), EngineSessionId("first".into()))],
            cx,
        )
    });
    cx.run_until_parked();
    let before = count.get();
    assert!(before > 0);
    model.update(cx, |m, cx| {
        m.set_local_cli_catalog(
            true,
            [(ConversationId(1), EngineSessionId("replacement".into()))],
            cx,
        )
    });
    cx.run_until_parked();
    assert!(count.get() > before);
    assert_eq!(
        model.read_with(cx, |m, _| m
            .local_cli_session(ConversationId(1))
            .unwrap()
            .0
            .clone()),
        "replacement"
    );
}

fn start_creation(s: &mut AppState, name: &str) -> Outcome {
    s.dispatch(CoreCommand::LocalCli(C::BeginNew(draft())));
    let inspect = s.dispatch(CoreCommand::LocalCli(C::Inspect));
    let mut view = ticket();
    view.setup = SetupId(name.into());
    view.root_candidate = Some(EngineSessionId(format!("cli-{name}")));
    reply(s, inspect, LocalCliEvent::Inspected(view.clone()));
    s.dispatch(CoreCommand::LocalCli(C::TrustExecutable(true)));
    s.dispatch(CoreCommand::LocalCli(C::TrustProject(true)));
    let check = s.dispatch(CoreCommand::LocalCli(C::Check));
    view.stage = LocalCliMetadataStage::MetadataVerified;
    reply(s, check, LocalCliEvent::Checked(view));
    s.dispatch(CoreCommand::LocalCli(C::Create))
}

fn uncertain_creation(s: &mut AppState, name: &str) -> LocalCliCreationId {
    let create = start_creation(s, name);
    reply(
        s,
        create,
        LocalCliEvent::Failed {
            phase: LocalCliPhase::Create,
            failure: LocalCliFailure::Unknown("lost response".into()),
        },
    );
    s.local_cli.creation.clone().unwrap()
}

fn lookup_absent(s: &mut AppState, identity: &LocalCliCreationId) {
    let lookup = s.dispatch(CoreCommand::LocalCli(C::LookupCreation));
    reply(
        s,
        lookup,
        LocalCliEvent::CreationLookup {
            identity: identity.clone(),
            found: None,
        },
    );
}

#[test]
fn parking_and_read_only_recovery_are_explicit_and_never_create_or_lookup_automatically() {
    let mut s = state();
    let identity = uncertain_creation(&mut s, "parked");
    assert!(!s.local_cli_availability().park_creation);
    lookup_absent(&mut s, &identity);
    assert!(s.local_cli_availability().park_creation);
    let parked = s.dispatch(CoreCommand::LocalCli(C::ParkCreationAndBeginNew(draft())));
    assert!(parked.effects.iter().all(|e| matches!(e,
        Effect::Backend(BackendRequest::LocalCli { request: LocalCliRequest::Revoke { setup }, .. }) if setup == &identity.setup)));
    assert_eq!(
        s.local_cli.parked_creations(),
        std::slice::from_ref(&identity)
    );
    assert!(s.local_cli.creation.is_none() && s.local_cli.ticket.is_none());
    assert!(!s.local_cli.trust_executable && !s.local_cli.trust_project);
    if !parked.effects.is_empty() {
        reply(&mut s, parked, LocalCliEvent::Revoked);
    }
    s.read_only = Some("read-only".into());
    assert!(s.local_cli_availability().recover_creation);
    let recovered = s.dispatch(CoreCommand::LocalCli(C::RecoverCreation(identity.clone())));
    assert!(recovered.effects.is_empty());
    assert_eq!(s.local_cli.creation, Some(identity.clone()));
    let a = s.local_cli_availability();
    assert!(a.lookup_creation && !a.inspect && !a.check && !a.create && !a.repeat_create);
    lookup_absent(&mut s, &identity);
    assert!(!s.local_cli_availability().repeat_create);
    assert!(!s.local_cli_availability().park_creation);
    s.read_only = None;
    assert!(s.local_cli_availability().park_creation);
    s.dispatch(CoreCommand::LocalCli(C::ParkCreationAndBeginNew(draft())));
    assert_eq!(s.local_cli.parked_creations(), &[identity]);
}

#[test]
fn full_recovery_list_preserves_active_pair_and_never_evicts() {
    let mut s = state();
    for index in 0..LOCAL_CLI_RECOVERY_CAPACITY {
        let identity = uncertain_creation(&mut s, &format!("record-{index}"));
        lookup_absent(&mut s, &identity);
        s.dispatch(CoreCommand::LocalCli(C::ParkCreationAndBeginNew(draft())));
    }
    let retained = s.local_cli.parked_creations().to_vec();
    let identity = uncertain_creation(&mut s, "overflow");
    lookup_absent(&mut s, &identity);
    assert!(!s.local_cli_availability().park_creation);
    assert!(
        s.dispatch(CoreCommand::LocalCli(C::ParkCreationAndBeginNew(draft())))
            .effects
            .is_empty()
    );
    assert_eq!(s.local_cli.creation, Some(identity));
    assert_eq!(s.local_cli.parked_creations(), retained);
}

#[test]
fn revoke_remains_available_after_created_or_uncertain_create_and_keeps_exact_identity() {
    for confirmed in [false, true] {
        let mut s = state();
        let create = start_creation(&mut s, "revoke-me");
        let identity = s.local_cli.creation.clone().unwrap();
        if confirmed {
            let mut catalog = Bootstrap {
                projects: vec![],
                models: vec![],
                conversations: vec![(ConversationId(9), ProjectId(1), "exact".into(), 1)],
                now: 1,
            };
            s.apply_catalog(catalog.clone());
            reply(
                &mut s,
                create,
                LocalCliEvent::Created(LocalCliCreated {
                    identity: identity.clone(),
                    conversation: ConversationId(9),
                }),
            );
            // Unrelated history must not replace the captured setup identity.
            catalog
                .conversations
                .push((ConversationId(10), ProjectId(1), "unrelated".into(), 1));
            s.apply_catalog(catalog);
            assert_eq!(s.local_cli.stage, LocalCliStage::Created);
        } else {
            reply(
                &mut s,
                create,
                LocalCliEvent::Failed {
                    phase: LocalCliPhase::Create,
                    failure: LocalCliFailure::Unknown("lost".into()),
                },
            );
        }
        assert!(s.local_cli_availability().revoke);
        let revoke = s.dispatch(CoreCommand::LocalCli(C::Revoke));
        assert!(revoke.effects.iter().any(|e| matches!(e,
            Effect::Backend(BackendRequest::LocalCli { request: LocalCliRequest::Revoke { setup }, .. }) if setup == &identity.setup)));
        assert_eq!(s.local_cli.creation, Some(identity));
    }
}

#[gpui::test]
fn core_capabilities_gate_picker_shortcuts_attachments_and_cli_labels(cx: &mut TestAppContext) {
    cx.update(crate::text::init);
    let window = cx.add_window(|window, cx| {
        let mut s = state();
        s.apply_catalog(Bootstrap {
            projects: vec![],
            models: vec![ModelInfo {
                id: "native-model".into(),
                name: "Native chosen model".into(),
                note: String::new(),
            }],
            conversations: vec![(ConversationId(1), ProjectId(1), "chat".into(), 1)],
            now: 1,
        });
        s.selected = Some(ConversationId(1));
        s.prefs.model = Some("native-model".into());
        assert!(!s.execution_capabilities().model_settings);
        assert!(!s.execution_capabilities().attachments);
        assert_eq!(
            restricted_model_label(&s),
            Some("Model settings unsupported")
        );
        let model = cx.new(|_| crate::model::Model::new(s));
        Workspace::new(model, Default::default(), window, cx)
    });
    let root = window.root(cx).unwrap();
    let cx = &mut VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    for profile in [
        ExecutionProfile::NativeAgent,
        ExecutionProfile::LocalCliChatOnly,
        ExecutionProfile::LocalCliRestrictedWrite,
        ExecutionProfile::Unsupported,
    ] {
        root.update_in(cx, |w, window, cx| {
            w.model.update(cx, |m, cx| {
                let c = m.state.current().unwrap();
                m.apply_event(
                    BackendEvent {
                        conversation: c.id,
                        generation: c.generation,
                        op: None,
                        kind: EventKind::ExecutionContext {
                            profile,
                            execution_blocked: None,
                        },
                    },
                    cx,
                );
            });
            let caps = w.state(cx).execution_capabilities();
            assert_eq!(
                caps.model_settings,
                profile == ExecutionProfile::NativeAgent
            );
            assert_eq!(caps.attachments, profile == ExecutionProfile::NativeAgent);
            let expected = match profile {
                ExecutionProfile::NativeAgent => None,
                ExecutionProfile::LocalCliChatOnly | ExecutionProfile::LocalCliRestrictedWrite => {
                    Some("Claude CLI · fixed model")
                }
                ExecutionProfile::Unsupported => Some("Model settings unsupported"),
            };
            assert_eq!(restricted_model_label(w.state(cx)), expected);
            w.close_overlay(window, cx);
            for overlay in [Overlay::Model, Overlay::ModelSettings, Overlay::Effort] {
                w.open_overlay(overlay, window, cx);
                assert_eq!(
                    w.overlay,
                    if caps.model_settings {
                        overlay
                    } else {
                        Overlay::None
                    }
                );
                w.close_overlay(window, cx);
            }
            if !caps.attachments {
                // These must return before any native picker or filesystem access.
                w.attach_files(cx);
                w.attach_paths(vec!["/must-not-be-read".into()], cx);
                assert!(w.state(cx).current().unwrap().draft.attachments.is_empty());
            }
            let commands = super::super::commands::build(w.state(cx), None);
            assert_eq!(
                commands
                    .iter()
                    .find(|c| c.title == "Attach files…")
                    .unwrap()
                    .enabled,
                caps.attachments
            );
            assert_eq!(
                commands
                    .iter()
                    .find(|c| c.title == "Choose model…")
                    .unwrap()
                    .enabled,
                caps.model_settings
            );
            assert_eq!(w.state(cx).prefs.model.as_deref(), Some("native-model"));
        });
        cx.run_until_parked();
    }
}

#[gpui::test]
fn cli_trust_keyboard_press_and_release_toggle_exactly_once(cx: &mut TestAppContext) {
    cx.update(crate::text::init);
    let window = cx.add_window(|window, cx| {
        let model = cx.new(|_| crate::model::Model::new(inspected()));
        model.update(cx, |m, cx| m.explore_workspace(cx));
        Workspace::new(model, Default::default(), window, cx)
    });
    let root = window.root(cx).unwrap();
    let cx = &mut VisualTestContext::from_window(window.into(), cx);
    root.update_in(cx, |w, window, cx| {
        w.open_overlay(Overlay::LocalCli, window, cx)
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("tab tab tab tab tab tab tab tab space");
    cx.run_until_parked();
    root.update(cx, |w, cx| assert!(w.state(cx).local_cli.trust_executable));
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        window.dispatch_event(
            gpui::PlatformInput::KeyUp(gpui::KeyUpEvent {
                keystroke: gpui::Keystroke::parse("space").unwrap(),
            }),
            cx,
        );
    });
    cx.run_until_parked();
    root.update(cx, |w, cx| {
        assert!(
            w.state(cx).local_cli.trust_executable,
            "native key-up must not toggle trust back off"
        )
    });
}

#[gpui::test]
fn cli_request_keeps_a_neutral_focus_anchor_after_its_button_disables(cx: &mut TestAppContext) {
    use std::{cell::RefCell, rc::Rc};
    cx.update(crate::text::init);
    let effects = Rc::new(RefCell::new(vec![]));
    let captured = effects.clone();
    let mut s = state();
    s.dispatch(CoreCommand::LocalCli(C::BeginNew(draft())));
    let window = cx.add_window(|window, cx| {
        let model = cx.new(|_| {
            let mut m = crate::model::Model::new(s);
            m.set_effect_handler(Box::new(move |e, _| captured.borrow_mut().push(e)));
            m
        });
        model.update(cx, |m, cx| m.explore_workspace(cx));
        Workspace::new(model, Default::default(), window, cx)
    });
    let root = window.root(cx).unwrap();
    let cx = &mut VisualTestContext::from_window(window.into(), cx);
    root.update_in(cx, |w, window, cx| {
        w.open_overlay(Overlay::LocalCli, window, cx)
    });
    cx.run_until_parked();
    effects.borrow_mut().clear();
    // Executable -> project -> two modes -> three aliases -> diagnostics -> Inspect.
    cx.simulate_keystrokes("tab tab tab tab tab tab tab tab space");
    cx.run_until_parked();
    root.update_in(cx, |w, window, cx| {
        assert!(!w.state(cx).local_cli_availability().inspect);
        assert!(
            w.menu_focus.is_focused(window),
            "a disabled initiating button must not strand focus"
        );
        w.model.update(cx, |m, cx| {
            reply(
                &mut m.state,
                Outcome {
                    effects: std::mem::take(&mut *effects.borrow_mut()),
                    ..Default::default()
                },
                LocalCliEvent::Inspected(ticket()),
            );
            cx.notify();
        });
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("space enter pagedown");
    cx.run_until_parked();
    root.update_in(cx, |w, window, cx| {
        assert!(w.menu_focus.is_focused(window));
        assert!(w.cli_dialog_scroll.offset().y < px(0.));
        assert!(w.state(cx).local_cli_availability().trust);
        assert!(!w.state(cx).local_cli.trust_executable && !w.state(cx).local_cli.trust_project);
    });
    assert!(
        effects.borrow().is_empty(),
        "neutral anchor must never repeat a request or grant trust"
    );
    cx.simulate_keystrokes("tab");
    root.update_in(cx, |w, window, cx| {
        assert!(w.menu_focus.contains_focused(window, cx))
    });
}

#[gpui::test]
fn cli_modal_tab_moves_between_editors_and_never_escapes_or_acts(cx: &mut TestAppContext) {
    cx.update(crate::text::init);
    let window = cx.add_window(|window, cx| {
        let model = cx.new(|_| crate::model::Model::new(inspected()));
        model.update(cx, |m, cx| m.explore_workspace(cx));
        Workspace::new(model, Default::default(), window, cx)
    });
    let root = window.root(cx).unwrap();
    let cx = &mut VisualTestContext::from_window(window.into(), cx);
    root.update_in(cx, |w, window, cx| {
        w.open_overlay(Overlay::LocalCli, window, cx)
    });
    cx.run_until_parked();
    root.update_in(cx, |w, window, cx| {
        assert!(
            w.local_cli_executable
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        )
    });
    cx.simulate_keystrokes("tab");
    root.update_in(cx, |w, window, cx| {
        assert!(
            w.local_cli_project
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        );
        assert!(!w.state(cx).local_cli.trust_executable);
        assert!(!w.state(cx).local_cli.trust_project);
    });
    for _ in 0..40 {
        cx.simulate_keystrokes("tab");
        root.update_in(cx, |w, window, cx| {
            assert!(w.menu_focus.contains_focused(window, cx))
        });
    }
    root.update_in(cx, |w, window, cx| {
        window.focus(&w.local_cli_executable.read(cx).focus_handle(cx), cx)
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("shift-tab");
    root.update_in(cx, |w, window, cx| {
        assert!(w.menu_focus.contains_focused(window, cx));
        assert!(!w.state(cx).local_cli.trust_executable && !w.state(cx).local_cli.trust_project);
        window.focus(&w.local_cli_executable.read(cx).focus_handle(cx), cx);
    });
    let editor = root.read_with(cx, |w, _| w.local_cli_executable.clone());
    editor.update_in(cx, |e, window, cx| {
        e.replace_and_mark_text_in_range(None, "候補", None, window, cx);
        assert!(e.is_composing());
    });
    cx.run_until_parked();
    assert!(editor.read_with(cx, |e, _| e.is_composing()));
    cx.simulate_keystrokes("tab");
    root.update_in(cx, |w, window, cx| {
        assert!(
            w.local_cli_executable
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        );
        assert!(w.local_cli_executable.read(cx).is_composing());
    });
}

// Headless GPUI entity/input-handler tests, not real native input or IME verification.
#[gpui::test]
fn editors_reconcile_silently_user_edit_invalidates_and_close_preserves_core(
    cx: &mut TestAppContext,
) {
    cx.update(crate::text::init);
    let window = cx.add_window(|window, cx| {
        let mut s = inspected();
        s.dispatch(CoreCommand::LocalCli(C::TrustExecutable(true)));
        s.dispatch(CoreCommand::LocalCli(C::TrustProject(true)));
        let model = cx.new(|_| crate::model::Model::new(s));
        Workspace::new(model, Default::default(), window, cx)
    });
    let root = window.root(cx).unwrap();
    let cx = &mut VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    root.update(cx, |w, cx| {
        w.reconcile_local_cli_inputs(cx);
        assert!(w.state(cx).local_cli.trust_executable);
        assert!(w.state(cx).local_cli.trust_project);
        assert_eq!(w.local_cli_executable.read(cx).text(), draft().executable);
    });
    let editor = root.read_with(cx, |w, _| w.local_cli_executable.clone());
    editor.update_in(cx, |e, window, cx| {
        e.replace_text_in_range(None, "-edited", window, cx)
    });
    cx.run_until_parked();
    root.update_in(cx, |w, window, cx| {
        assert!(w.state(cx).local_cli.draft.executable.ends_with("-edited"));
        assert!(!w.state(cx).local_cli.trust_executable);
        assert!(!w.state(cx).local_cli.trust_project);
        assert!(w.state(cx).local_cli.ticket.is_none());
        // Unknown mutation remains owned by core even when the dialog is dismissed.
        w.model.update(cx, |m, _| {
            m.state.local_cli.creation = Some(LocalCliCreationId {
                session: EngineSessionId("captured".into()),
                setup: SetupId("original".into()),
            });
            m.state.local_cli.stage = LocalCliStage::CreateUnknown;
        });
        w.open_overlay(Overlay::LocalCli, window, cx);
        let before = w.state(cx).local_cli.clone();
        w.close_overlay(window, cx);
        assert_eq!(w.state(cx).local_cli, before);
        assert_eq!(w.overlay, Overlay::None);
    });
}
