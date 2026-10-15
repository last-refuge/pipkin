use super::*;
fn hints(s: &mut AppState) {
    existing(s);
    let req = call(cmd(s, LocalCliCommand::SessionStatus));
    answer(
        s,
        &req,
        LocalCliEvent::SessionStatus(LocalCliSessionView {
            session: EngineSessionId("saved-engine-session".into()),
            mode: CliMode::ChatOnly,
            executable: "/private/executable".into(),
            project: "/private/project".into(),
            model: Some(CliModel::Opus),
            context_valid: false,
        }),
    );
}
#[test]
fn fresh_setup_copies_only_hints_with_no_probe_create_prompt_or_context_transfer() {
    let mut s = state();
    hints(&mut s);
    s.conversations[0].draft.text = "source prompt must not transfer".into();
    let conversations = format!("{:?}", s.conversations);
    let target = s.local_cli.target.clone().unwrap();
    assert!(s.local_cli_availability().fresh_setup);
    no_effect(cmd(&mut s, LocalCliCommand::FreshSetup(target)));
    assert_eq!(format!("{:?}", s.conversations), conversations);
    assert_eq!(s.local_cli.target, Some(LocalCliTarget::New));
    assert_eq!(s.local_cli.stage, LocalCliStage::Editing);
    assert_eq!(
        s.local_cli.draft,
        LocalCliDraft {
            mode: CliMode::ChatOnly,
            executable: "/private/executable".into(),
            project: "/private/project".into(),
            model: CliModel::Opus
        }
    );
    assert!(s.local_cli.ticket.is_none() && s.local_cli.creation.is_none());
    assert!(!s.local_cli.trust_project && !s.local_cli.trust_executable);
    assert!(!s.local_cli_availability().create);
    assert!(s.local_cli_availability().inspect);
}
#[test]
fn fresh_handoff_rejects_stale_active_unknown_readonly_and_malformed_sources() {
    for case in 0..7 {
        let mut s = state();
        hints(&mut s);
        let target = s.local_cli.target.clone().unwrap();
        match case {
            0 => s.conversations[0].generation += 1,
            1 => {
                s.conversations[0].run = RunState::Running {
                    op: OperationId(500),
                }
            }
            2 => {
                s.conversations[0].run = RunState::OutcomeUnknown {
                    op: OperationId(500),
                }
            }
            3 => s.read_only = Some("read only".into()),
            4 => {
                s.local_cli.session_status.as_mut().unwrap().session =
                    EngineSessionId("other".into())
            }
            5 => s.local_cli.session_status.as_mut().unwrap().mode = CliMode::RestrictedWrite,
            _ => s.local_cli.session_status.as_mut().unwrap().model = None,
        }
        let before = s.local_cli.clone();
        no_effect(cmd(&mut s, LocalCliCommand::FreshSetup(target)));
        assert_eq!(s.local_cli, before);
    }
}
#[test]
fn redacted_diagnostics_are_explicit_scoped_and_contain_no_untrusted_text() {
    let mut s = state();
    hints(&mut s);
    s.local_cli.error = Some(LocalCliFailure::Unknown(
        "SECRET_ERROR account@example.invalid".into(),
    ));
    s.conversations[0].draft.text = "SECRET_PROMPT".into();
    let (conversation, generation) = s.local_cli_diagnostic_scope();
    no_effect(cmd(
        &mut s,
        LocalCliCommand::CopyDiagnostics {
            conversation,
            generation: generation + 1,
        },
    ));
    let out = cmd(
        &mut s,
        LocalCliCommand::CopyDiagnostics {
            conversation,
            generation,
        },
    );
    let [Effect::CopyText(text)] = out.effects.as_slice() else {
        panic!()
    };
    for forbidden in [
        "SECRET_ERROR",
        "SECRET_PROMPT",
        "account@example.invalid",
        "/private/",
        "saved-engine-session",
    ] {
        assert!(!text.contains(forbidden));
    }
    assert!(text.contains("cannot be resumed safely"));
    assert!(text.contains("not proof of delivery"));
    assert_eq!(s.local_cli.stage, LocalCliStage::Editing);
    assert!(s.local_cli.ticket.is_none());
}
