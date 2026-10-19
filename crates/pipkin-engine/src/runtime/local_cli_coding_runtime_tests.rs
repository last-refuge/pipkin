//! Public setup → current grant → coding actor → public approval API. Fake executable only.
use super::*;
use crate::runtime::tests::Engine;

fn start(f: &Fixture, provider: &Scripted, master: bool, write: bool) -> Engine {
    let mut config = f.config();
    config.enable_local_claude_cli = master;
    config.enable_local_claude_cli_write = write;
    config
        .env
        .vars
        .insert("HOME".into(), f.dir.path().display().to_string());
    config
        .env
        .vars
        .insert("PATH".into(), "/usr/bin:/bin".into());
    f.start_config(provider, config)
}
fn approve(engine: &mut Engine, cli: &FakeCli, existing: bool) -> String {
    let target = if existing {
        LocalCliSetupTarget::Existing {
            session: "cli".into(),
            model: LocalCliModel::Sonnet,
        }
    } else {
        LocalCliSetupTarget::New {
            thinking: None,
            project: cli.project.display().to_string(),
            model: LocalCliModel::Sonnet,
        }
    };
    let Reply::LocalCliSetup(view) = engine
        .call(Call::InspectLocalCli {
            mode: LocalCliMode::RestrictedWrite,
            executable: cli.executable.display().to_string(),
            target,
        })
        .unwrap()
    else {
        panic!("inspect")
    };
    let setup = view.setup;
    let Reply::LocalCliSetup(view) = engine
        .call(Call::CheckLocalCli {
            setup: setup.clone(),
            trust_executable: true,
            trust_project: true,
        })
        .unwrap()
    else {
        panic!("check")
    };
    assert_eq!(view.stage, LocalCliSetupStage::MetadataVerified);
    setup
}
fn create(engine: &mut Engine, setup: &str) -> SessionSummary {
    let Reply::Session { summary } = engine
        .call(Call::CreateLocalCliSession {
            session: "cli".into(),
            setup: setup.into(),
        })
        .unwrap()
    else {
        panic!("create")
    };
    summary
}
fn pending(engine: &mut Engine, prompt: u64) -> LocalCliApprovalReview {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let Reply::LocalCliApprovals { run, mut reviews } = engine
            .call(Call::LocalCliApprovals {
                session: "cli".into(),
            })
            .unwrap()
        else {
            panic!("view")
        };
        if let Some(review) = reviews.pop() {
            assert_eq!(run.as_ref().unwrap().prompt, prompt);
            assert_eq!(review.id.run, run.unwrap());
            return review;
        }
        assert!(Instant::now() < deadline, "pending public approval");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn submit(engine: &mut Engine, request: &str) -> u64 {
    let Reply::Admitted {
        run: Some(prompt), ..
    } = engine.prompt("cli", request, "synthetic prompt").unwrap()
    else {
        panic!("admission")
    };
    prompt
}
fn launches(f: &Fixture) -> usize {
    std::fs::read_to_string(f.project.join("launches"))
        .unwrap_or_default()
        .lines()
        .count()
}

#[test]
fn coding_requires_both_explicit_host_gates_and_metadata_is_not_a_grant() {
    for (master, write) in [(false, false), (false, true), (true, false), (true, true)] {
        let f = Fixture::new();
        let provider = Scripted::echo("never");
        let mut engine = start(&f, &provider, master, write);
        let shared = engine.handle.shared().clone();
        let cli = fake(&f, "normal");
        let setup = approve(&mut engine, &cli, false);
        let summary = create(&mut engine, &setup);
        assert_eq!(summary.local_cli_mode, Some(LocalCliMode::RestrictedWrite));
        assert_eq!(summary.blocked.is_none(), master && write);
        assert_eq!(
            shared.local_cli_setups.current_grant("cli").is_some(),
            master && write
        );
        assert_eq!(launches(&f), 0, "metadata never launches inference");
        if !(master && write) {
            assert_eq!(
                engine
                    .prompt("cli", "no-grant", "synthetic prompt")
                    .unwrap_err()
                    .code,
                ErrorCode::Unsupported
            );
            assert!(
                shared
                    .store
                    .external_prompt("cli", "no-grant")
                    .unwrap()
                    .is_none()
            );
        }
        no_native(&shared, &provider);
    }
}

#[test]
fn cancelled_request_scope_cannot_admit_coding_or_fence_the_unused_context() {
    let f = Fixture::new();
    let provider = Scripted::echo("never");
    let mut engine = start(&f, &provider, true, true);
    let shared = engine.handle.shared().clone();
    let cli = fake(&f, "normal");
    let setup = approve(&mut engine, &cli, false);
    create(&mut engine, &setup);
    let cancel = CancellationToken::new();
    cancel.cancel();
    // Deliberately bypass the dispatcher's outer select/initial check. The durable
    // admission gate itself must see cancellation, even if metadata completed in a gap.
    let result = shared
        .handle
        .block_on(crate::runtime::request_cancel::scope(
            &cancel,
            shared.prompt_by_backend(
                engine.conn.client_id(),
                "cli",
                "cancel-gap",
                "synthetic prompt",
                &[],
                None,
            ),
        ));
    assert_eq!(result.unwrap_err().code, ErrorCode::Cancelled);
    assert!(
        shared
            .store
            .external_prompt("cli", "cancel-gap")
            .unwrap()
            .is_none()
    );
    assert!(
        shared
            .store
            .cli_binding("cli")
            .unwrap()
            .unwrap()
            .context_valid
    );
    assert!(
        !shared
            .local_cli_setups
            .current_grant("cli")
            .unwrap()
            .cancel
            .is_cancelled()
    );
    assert_eq!(launches(&f), 0);
    no_native(&shared, &provider);
}

#[test]
fn public_coding_turn_outlives_probe_child_and_resumes_only_with_clean_evidence() {
    let f = Fixture::new();
    let provider = Scripted::echo("never");
    let mut engine = start(&f, &provider, true, true);
    let shared = engine.handle.shared().clone();
    let cli = fake(&f, "normal");
    let setup = approve(&mut engine, &cli, false);
    assert!(create(&mut engine, &setup).blocked.is_none());
    let grant = shared.local_cli_setups.current_grant("cli").unwrap();
    for (i, request) in ["first", "second"].into_iter().enumerate() {
        let prompt = submit(&mut engine, request);
        // Caller/Turn has returned. Its child cancellation MUST NOT retire actor authority.
        assert!(!grant.cancel.is_cancelled());
        assert!(!shared.local_cli_setups.stop_pending("cli"));
        let review = pending(&mut engine, prompt);
        assert_eq!(
            submit(&mut engine, request),
            prompt,
            "duplicate is read-only even while pending"
        );
        assert_eq!(launches(&f), i + 1);
        let Reply::LocalCliApproval {
            receipt: Some(receipt),
        } = engine
            .call(Call::DecideLocalCliApproval {
                id: review.id.clone(),
                decision: LocalCliApprovalDecision::AllowOnce,
            })
            .unwrap()
        else {
            panic!("receipt")
        };
        assert_eq!(receipt.state, LocalCliApprovalState::Consumed);
        assert_eq!(settled(&shared, request).state, ExternalRunState::Completed);
        assert!(
            std::fs::read(f.project.join("fake-args"))
                .unwrap()
                .windows(8)
                .any(|w| w == b"--resume")
                == (i == 1)
        );
        assert!(matches!(
            engine
                .call(Call::LookupLocalCliApproval { id: review.id })
                .unwrap(),
            Reply::LocalCliApproval { receipt: Some(_) }
        ));
    }
    engine.call(Call::RevokeLocalCliSetup { setup }).unwrap();
    assert!(grant.cancel.is_cancelled());
    assert_eq!(
        submit(&mut engine, "first"),
        shared
            .store
            .external_prompt("cli", "first")
            .unwrap()
            .unwrap()
            .id
    );
    assert!(engine.prompt("cli", "third", "synthetic prompt").is_err());
    assert_eq!(launches(&f), 2);
    no_native(&shared, &provider);
}

#[test]
fn owner_reapproval_is_required_and_old_grant_cannot_launch_or_approve() {
    let f = Fixture::new();
    let provider = Scripted::echo("never");
    let mut engine = start(&f, &provider, true, true);
    let shared = engine.handle.shared().clone();
    let cli = fake(&f, "normal");
    let setup = approve(&mut engine, &cli, false);
    create(&mut engine, &setup);
    let old = shared.local_cli_setups.current_grant("cli").unwrap();
    let foreign = engine.handle.connect("foreign");
    let request = Call::Prompt {
        session: "cli".into(),
        request: "foreign".into(),
        text: "synthetic prompt".into(),
        images: vec![],
        model: None,
    };
    assert_eq!(
        rpc(&foreign, request).unwrap_err().code,
        ErrorCode::Unsupported
    );
    assert_eq!(launches(&f), 0);
    engine.conn = foreign;
    let Reply::LocalCliSession(view) = engine
        .call(Call::LocalCliSessionStatus {
            session: "cli".into(),
        })
        .unwrap()
    else {
        panic!("status")
    };
    assert!(view.summary.blocked.is_some());
    let setup = approve(&mut engine, &cli, true);
    assert!(old.cancel.is_cancelled());
    let prompt = submit(&mut engine, "new-owner");
    let review = pending(&mut engine, prompt);
    engine.call(Call::RevokeLocalCliSetup { setup }).unwrap();
    assert_eq!(
        settled(&shared, "new-owner").state,
        ExternalRunState::Interrupted
    );
    assert!(
        engine
            .call(Call::DecideLocalCliApproval {
                id: review.id,
                decision: LocalCliApprovalDecision::AllowOnce
            })
            .is_err()
    );
    assert!(!f.project.join("effect").exists());
    assert_eq!(launches(&f), 1);
    no_native(&shared, &provider);
}
