//! Same-alias legacy routing through the desktop core, adapter, storage and engine.
use super::*;
use pipkin_core::{CliMode, ExecutionProfile, ProviderModel};

fn select_coding(fx: &Fixture, executable: &Path) -> (Harness, String) {
    let db = fx.root.join("selection-desktop.db");
    let storage = Storage::open(&db, NAMESPACE).unwrap();
    storage.save_prefs(&pipkin_core::Prefs {
        model: Some("scripted/scripted".into()),
        cli_executable: Some(executable.display().to_string()),
        cli_provider_configured: true,
        cli_mode: CliMode::ChatOnly,
        cli_model: CliModel::Sonnet,
        provider_model: Some(ProviderModel::ClaudeCli(CliModel::Sonnet)),
        ..Default::default()
    });
    storage.shutdown();
    let mut h = Harness::start(fx, &db);
    h.until("CLI catalog", |s| {
        s.connection.is_ready() && s.local_cli.available
    });
    h.dispatch(Command::AddProject(fx.project.display().to_string()));
    // Manufacture a historical binding using the legacy API, without sending a prompt.
    h.dispatch(Command::LocalCli(C::BeginNew(LocalCliDraft {
        thinking: None,
        mode: CliMode::ChatOnly,
        executable: executable.display().to_string(),
        project: fx.project.display().to_string(),
        model: CliModel::Sonnet,
    })));
    h.dispatch(Command::LocalCli(C::Inspect));
    h.until("legacy inspection", |s| {
        s.local_cli.stage == LocalCliStage::AwaitingTrust
    });
    h.dispatch(Command::LocalCli(C::TrustExecutable(true)));
    h.dispatch(Command::LocalCli(C::TrustProject(true)));
    h.dispatch(Command::LocalCli(C::Check));
    h.until("legacy metadata", |s| {
        s.local_cli.stage == LocalCliStage::Verified
    });
    h.dispatch(Command::LocalCli(C::Create));
    h.until("legacy attachment", |s| {
        s.local_cli.stage == LocalCliStage::Created
            && s.current()
                .is_some_and(|c| c.opened && c.profile == ExecutionProfile::LocalCliChatOnly)
    });
    let legacy = h.state.local_cli.created.clone().unwrap();
    let engine = fx.engine();
    let old_binding = engine
        .shared()
        .store
        .cli_binding(&legacy.identity.session.0)
        .unwrap()
        .unwrap();
    h.dispatch(Command::EditDraft(
        "unsent legacy draft must not be forwarded".into(),
    ));
    h.dispatch(Command::FlushDraft(legacy.conversation));
    assert!(
        h.state
            .selected_provider_label()
            .unwrap()
            .contains("legacy chat, no tools")
    );

    // The exact alias that used to short-circuit. No new Connect/Check confirmations.
    h.dispatch(Command::SelectProviderModel(ProviderModel::ClaudeCli(
        CliModel::Sonnet,
    )));
    h.until("same alias selects a ready coding conversation", |s| {
        s.local_cli.stage == LocalCliStage::Created
            && s.selected != Some(legacy.conversation)
            && s.current().is_some_and(|c| {
                c.opened
                    && c.profile == ExecutionProfile::LocalCliCoding
                    && c.execution_blocked.is_none()
            })
    });
    let coding = h.state.local_cli.created.clone().unwrap();
    assert_ne!(coding.identity.session, legacy.identity.session);
    assert_eq!(h.state.current().unwrap().draft.text, "");
    assert_eq!(
        h.state
            .conversation(legacy.conversation)
            .unwrap()
            .draft
            .text,
        "unsent legacy draft must not be forwarded"
    );
    assert_eq!(
        engine
            .shared()
            .store
            .cli_binding(&legacy.identity.session.0)
            .unwrap()
            .unwrap(),
        old_binding
    );
    assert_eq!(
        engine
            .shared()
            .store
            .cli_binding(&coding.identity.session.0)
            .unwrap()
            .unwrap()
            .identity
            .launch_policy,
        "cli-unattended-coding-v1"
    );
    assert_eq!(
        h.state.selected_provider_label().as_deref(),
        Some("Claude Code CLI · Sonnet")
    );
    assert!(fx.requests().is_empty());
    (h, coding.identity.session.0)
}

#[test]
fn same_alias_legacy_selection_connects_coding_without_forwarding_or_native_fallback() {
    let fx = Fixture::start(|_, _| panic!("native inference must not run"));
    let fake = FakeShell::install_with_coding(&fx, true);
    let (mut h, _) = select_coding(&fx, &fake.executable);
    fake.assert_counts(6, 0);
    h.send("explicit synthetic coding instruction");
    h.until("fresh coding answer", |s| {
        is_idle(s) && assistant_texts(s) == ["Synthetic external fresh answer"]
    });
    fake.assert_counts(9, 1);
    assert!(fx.requests().is_empty());
    h.stop();
}

#[test]
#[ignore = "two explicitly authorized real CLI prompts; direct test binary, not cargo's loader environment"]
fn authorized_live_same_alias_selection_fresh_and_resume() {
    use pipkin_engine::store::{ExternalPromptMode, ExternalRunState};
    use std::io::Write;
    let root = PathBuf::from(std::env::var("PIPKIN_CLI_LIVE_ROOT").expect("explicit owned root"));
    let executable =
        PathBuf::from(std::env::var("PIPKIN_CLI_LIVE_EXECUTABLE").expect("explicit executable"));
    assert!(root.is_absolute() && root.is_dir() && executable.is_absolute());
    assert!(
        !root.join("app-fresh-consumed").exists(),
        "never rerun a consumed live test"
    );
    assert!(!root.join("app-resume-consumed").exists());
    let mut fx = Fixture::start(|_, _| panic!("native inference must not run"));
    assert!(
        fx.root.starts_with(&root),
        "TMPDIR must be the owned artifact root"
    );
    fx._dir.disable_cleanup(true);
    println!("retained app fixture: {}", fx.root.display());
    std::fs::write(fx.project.join("AGENTS.md"), "Disposable qualification project. Only read the named probe input and write the named probe output with exactly the same contents. Do not access files outside this directory. No shell commands, network, agents or other work.\n").unwrap();
    let mut config = fx.config();
    config.env = Env::process();
    config.enable_local_claude_cli = true;
    config.enable_local_claude_cli_write = true;
    {
        let mut slot = fx.engine.lock().unwrap();
        slot.take().unwrap().shutdown(Duration::from_secs(5));
        *slot = Some(Arc::new(
            EngineHandle::start_with(
                config,
                registry_with(Arc::new(fx.provider.clone())),
                pipkin_engine::tools::builtin_tools(),
            )
            .unwrap(),
        ));
    }
    let (mut h, session) = select_coding(&fx, &executable);
    let engine = fx.engine();
    let initial_binding = engine
        .shared()
        .store
        .cli_binding(&session)
        .unwrap()
        .unwrap();
    for (turn, permit_name, mode) in [
        (1, "app-fresh-consumed", ExternalPromptMode::Fresh),
        (2, "app-resume-consumed", ExternalPromptMode::Resume),
    ] {
        let mut permit = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join(permit_name))
            .expect("one-shot prompt permit");
        permit.write_all(b"One of the two remaining user-authorized disposable-project prompts. Never replay.\n").unwrap();
        permit.sync_all().unwrap();
        let marker = format!("app-turn-{turn}-{:?}", std::time::SystemTime::now());
        std::fs::write(fx.project.join(format!("probe-input-{turn}.txt")), &marker).unwrap();
        h.send(&format!("Disposable tool-execution test. Read AGENTS.md and probe-input-{turn}.txt with Read. Use Write to create probe-output-{turn}.txt containing exactly that input file's contents. Do not use Bash, agents, network, or files outside this directory. Reply briefly when done."));
        let request = h
            .state
            .current()
            .unwrap()
            .pending_intent
            .as_ref()
            .expect("journaled prompt")
            .request
            .0
            .clone();
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            h.pump();
            if let Some(receipt) = engine
                .shared()
                .store
                .external_prompt(&session, &request)
                .unwrap()
                && matches!(
                    receipt.state,
                    ExternalRunState::Completed
                        | ExternalRunState::Interrupted
                        | ExternalRunState::Unknown
                )
            {
                println!(
                    "turn={turn}; state={:?}; mode={:?}; evidence={:?}",
                    receipt.state, receipt.mode, receipt.evidence
                );
                assert_eq!(receipt.state, ExternalRunState::Completed);
                assert_eq!(receipt.mode, mode);
                let evidence = receipt.evidence.unwrap();
                assert!(
                    evidence.input_delivered
                        && evidence.projection_finished
                        && evidence.root_reaped
                        && evidence.pipes_drained
                );
                assert!(!evidence.presentation_incomplete);
                break;
            }
            if Instant::now() >= deadline {
                h.dispatch(Command::Cancel);
                panic!("live deadline: Stop requested, no retry");
            }
            thread::sleep(Duration::from_millis(20));
        }
        h.until("desktop settles the real CLI turn", |s| {
            is_idle(s) && s.current().unwrap().pending_intent.is_none()
        });
        assert_eq!(
            std::fs::read_to_string(fx.project.join(format!("probe-output-{turn}.txt"))).unwrap(),
            marker
        );
        assert!(fx.requests().is_empty());
    }
    assert_eq!(
        engine
            .shared()
            .store
            .cli_binding(&session)
            .unwrap()
            .unwrap()
            .identity,
        initial_binding.identity
    );
    let (reads, writes, approvals) = engine.shared().store.read(|c| {
        Ok((c.query_row("SELECT count(*) FROM external_actions WHERE session=? AND tool_name='Read' AND status='completed'", [&session], |r| r.get::<_,u64>(0))?,
            c.query_row("SELECT count(*) FROM external_actions WHERE session=? AND tool_name='Write' AND status='completed'", [&session], |r| r.get::<_,u64>(0))?,
            c.query_row("SELECT count(*) FROM external_permissions", [], |r| r.get::<_,u64>(0))?))
    }).unwrap();
    println!(
        "completed reads={reads}; writes={writes}; approval rows={approvals}; native requests={}",
        fx.requests().len()
    );
    assert!(reads >= 2 && writes >= 2);
    assert_eq!(approvals, 0);
    assert_eq!(h.user_messages().len(), 2);
    h.stop();
    assert_eq!(open_requests(&fx.root.join("selection-desktop.db")), 0);
}

#[test]
fn clean_stop_keeps_coding_composer_ready_and_new_text_resumes_same_conversation() {
    let fx = Fixture::start(|_, _| panic!("native inference must not run"));
    let fake = FakeShell::install_with_coding(&fx, true);
    let mut script = std::fs::read_to_string(&fake.executable).unwrap();
    script.push_str("\nif [ ! -f stopped-once ]; then touch stopped-once; sleep 30; fi\n");
    std::fs::write(&fake.executable, script).unwrap();
    let (mut h, session) = select_coding(&fx, &fake.executable);
    let conversation = h.state.selected;
    h.send("first instruction");
    h.until("first CLI output", |s| {
        matches!(s.current().unwrap().run, RunState::Running { .. })
            && assistant_texts(s) == ["Synthetic external fresh answer"]
            && fx.project.join("stopped-once").exists()
    });
    h.dispatch(Command::Cancel);
    h.until("clean Stop returns to ready composer", |s| {
        is_idle(s) && s.current().unwrap().execution_blocked.is_none()
    });
    let engine = fx.engine();
    assert!(
        engine
            .shared()
            .store
            .cli_binding(&session)
            .unwrap()
            .unwrap()
            .context_valid
    );
    assert_eq!(FakeShell::lines(&fake.launches).len(), 1);
    assert!(!h.state.availability().retry);
    h.dispatch(Command::EditDraft(
        "inspect existing work and continue".into(),
    ));
    assert!(h.state.availability().submit);
    h.dispatch(Command::Submit);
    h.until("new instruction completes in original conversation", |s| {
        is_idle(s)
            && assistant_texts(s)
                .iter()
                .any(|text| text == "Synthetic external resume answer")
    });
    assert_eq!(h.state.selected, conversation);
    assert_eq!(
        h.user_messages(),
        ["first instruction", "inspect existing work and continue"]
    );
    let launches = FakeShell::lines(&fake.launches);
    assert_eq!(launches.len(), 2);
    let first = launches[0].strip_prefix("fresh ").unwrap();
    assert_eq!(launches[1], format!("resume {first}"));
    assert!(fx.requests().is_empty());
    h.stop();
}

#[test]
fn old_clean_stop_recovery_reconnects_without_replay_then_resumes_in_the_same_composer() {
    let fx = Fixture::start(|_, _| panic!("native inference must not run"));
    let fake = FakeShell::install_with_coding(&fx, true);
    let mut script = std::fs::read_to_string(&fake.executable).unwrap();
    script.push_str("\nif [ ! -f stopped-once ]; then touch stopped-once; sleep 30; fi\n");
    std::fs::write(&fake.executable, script).unwrap();
    let (mut h, session) = select_coding(&fx, &fake.executable);
    let conversation = h.state.selected.unwrap();
    h.send("first instruction");
    h.until("fixture has produced output", |s| {
        matches!(s.current().unwrap().run, RunState::Running { .. })
            && assistant_texts(s) == ["Synthetic external fresh answer"]
            && fx.project.join("stopped-once").exists()
    });
    h.dispatch(Command::Cancel);
    h.until("first turn stopped", is_idle);
    h.dispatch(Command::LocalCli(C::Revoke));
    h.until("connection revoked", |s| {
        s.current().unwrap().execution_blocked.is_some()
    });
    let engine = fx.engine();
    let store = &engine.shared().store;
    let identity = store.cli_binding(&session).unwrap().unwrap().identity;
    // Reproduce the old build's exact durable state only in this disposable fixture.
    store.tx(|tx| {
        tx.execute("UPDATE external_prompts SET evidence=json_remove(evidence,'$.continuation_authorized') WHERE session=?1", [&session])?;
        tx.execute("UPDATE cli_bindings SET context_valid=0,generation=generation+1 WHERE session=?1", [&session])?;
        Ok(())
    }).unwrap();
    let stopped = store.cli_clean_stop_recovery(&session).unwrap().unwrap();
    let probes_before = FakeShell::lines(&fake.metadata).len();
    let generation = h.state.current().unwrap().generation;
    h.dispatch(Command::LocalCli(C::BeginGuidedExisting {
        conversation,
        session: pipkin_core::local_cli::EngineSessionId(session.clone()),
    }));
    h.until("fresh reconnect recovers old clean Stop", |s| {
        s.local_cli.stage == LocalCliStage::Created
            && s.current().is_some_and(|c| {
                c.id == conversation
                    && c.opened
                    && c.generation > generation
                    && c.execution_blocked.is_none()
            })
    });
    let binding = store.cli_binding(&session).unwrap().unwrap();
    assert_eq!(binding.identity, identity);
    assert_eq!(binding.generation, 3);
    assert!(binding.context_valid);
    assert_eq!(FakeShell::lines(&fake.metadata).len(), probes_before + 3);
    assert_eq!(
        FakeShell::lines(&fake.launches).len(),
        1,
        "recovery must not launch a prompt"
    );
    assert_eq!(
        store
            .external_prompt(&session, &stopped.request)
            .unwrap()
            .unwrap(),
        stopped
    );
    h.dispatch(Command::EditDraft(
        "inspect existing work and continue".into(),
    ));
    assert!(h.state.availability().submit, "{:?}", h.state.current());
    h.dispatch(Command::Submit);
    h.until("new instruction completes in original conversation", |s| {
        is_idle(s)
            && assistant_texts(s)
                .iter()
                .any(|text| text == "Synthetic external resume answer")
    });
    assert_eq!(h.state.selected, Some(conversation));
    assert_eq!(
        h.user_messages(),
        ["first instruction", "inspect existing work and continue"]
    );
    let launches = FakeShell::lines(&fake.launches);
    assert_eq!(launches.len(), 2);
    assert_eq!(
        launches[1],
        format!("resume {}", launches[0].strip_prefix("fresh ").unwrap())
    );
    assert_eq!(
        store
            .external_prompt(&session, &stopped.request)
            .unwrap()
            .unwrap(),
        stopped
    );
    assert!(fx.requests().is_empty());
    h.stop();
}
