//! Public RPC route, synthetic CLI only. No installed CLI or native inference.
use super::super::tests::Engine;
use super::*;
use crate::execution::claude_cli::coding_policy;

fn connect(f: &Fixture, provider: &Scripted, tail: &str) -> (Engine, FakeCli) {
    connect_results(f, provider, tail, &[false, false, false])
}

fn connect_results(
    f: &Fixture,
    provider: &Scripted,
    tail: &str,
    errors: &[bool],
) -> (Engine, FakeCli) {
    let names = ["Read", "Glob", "Grep", "Edit", "Write", "Bash", "Agent"];
    let mut records =
        vec![json!({"type":"system","subtype":"init","session_id":UUID,"tools":&names[..6]})];
    for (index, name) in names.iter().enumerate() {
        let id = format!("tool-{index}");
        // CLI-owned Agent is surfaced lazily, absent from the startup inventory.
        if *name == "Agent" {
            for event in [
                json!({"type":"message_start","message":{"id":format!("message-{index}"),"role":"assistant","content":[]}}),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":id,"name":name,"input":{}}}),
                json!({"type":"content_block_stop","index":0}),
                json!({"type":"message_stop"}),
            ] {
                records.push(json!({"type":"stream_event","session_id":UUID,"event":event}));
            }
        }
        records.push(json!({"type":"assistant","session_id":UUID,"message":{"id":format!("message-{index}"),"role":"assistant","content":[{"type":"tool_use","id":id,"name":name,"input":{"fixture":true}}]}}));
        if *name == "Agent" {
            records.push(json!({"type":"assistant","session_id":UUID,"parent_tool_use_id":id,"message":{"id":"child","content":[{"type":"text","text":"nested CLI observation"}]}}));
        }
        records.push(json!({"type":"user","session_id":UUID,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":id,"content":"fixture result","is_error":false}]}}));
    }
    // One CLI process can continue internally after waiting for background work,
    // emitting another root init before its sole terminal result.
    for task in ["explore-engine", "explore-ui"] {
        records.push(json!({"type":"system","subtype":"task_notification","session_id":UUID,"task_id":task,"status":"completed"}));
        records.push(json!({"type":"system","subtype":"init","session_id":UUID,"tools":names}));
        records.push(json!({"type":"assistant","session_id":UUID,"message":{"id":task,"content":[{"type":"text","text":"Background exploration received"}]}}));
    }
    records.push(json!({"type":"assistant","session_id":UUID,"message":{"id":"answer","content":[{"type":"text","text":"Synthetic answer"}]}}));
    // Installed Coding CLI can hold per-turn results until background work ends,
    // then flush all of them after the last assistant message.
    for (index, error) in errors.iter().enumerate() {
        records.push(json!({"type":"result","session_id":UUID,"uuid":format!("result-{index}"),"result_index":index,"subtype":if *error {"error_during_execution"} else {"success"},"is_error":error,"result":"Synthetic answer","usage":{"input_tokens":index+1,"output_tokens":10}}));
    }
    let cli = FakeCli::with_records(f, tail, false, &records);
    let script = std::fs::read_to_string(&cli.executable).unwrap();
    let flags = coding_policy::FLAGS
        .iter()
        .map(|flag| format!("{flag} fixture\n"))
        .collect::<String>();
    let script = script.replacen("--help) printf '%s' '", &format!("--help) printf '%s' '{flags}"), 1)
        .replacen("for arg in \"$@\"; do", "session=''; previous=''; for value in \"$@\"; do case \"$previous\" in --session-id|--resume) session=\"$value\";; esac; previous=\"$value\"; done\nfor arg in \"$@\"; do", 1)
        .replacen("  cat > received-prompt", "  case \"$*\" in *--dangerously-skip-permissions*) :;; *) exit 91;; esac\n  case \"$*\" in *--permission-prompt-tool*|*--restricted*|*--safe-mode*) exit 92;; esac\n  printf 'changed by fake CLI\\n' > cli-owned-change\n  cat > received-prompt", 1)
        ;
    // Substitute only synthetic session placeholders on stdout, not identity checks in Pipkin.
    let script = script.replace(
        &format!("\n  {tail}\n  exit 0"),
        &format!(" | sed \"s/{UUID}/$session/g\"\n  {tail}\n  exit 0"),
    );
    std::fs::write(&cli.executable, script).unwrap();
    let mut config = f.config();
    config.enable_local_claude_cli = true;
    config.enable_local_claude_cli_write = true;
    config
        .env
        .vars
        .insert("HOME".into(), f.dir.path().display().to_string());
    config
        .env
        .vars
        .insert("PATH".into(), "/usr/bin:/bin".into());
    let mut engine = f.start_config(provider, config);
    let Reply::LocalCliSetup(view) = engine
        .call(Call::InspectLocalCli {
            mode: LocalCliMode::Coding,
            executable: cli.executable.display().to_string(),
            target: LocalCliSetupTarget::New {
                thinking: None,
                project: f.project.display().to_string(),
                model: LocalCliModel::Sonnet,
            },
        })
        .unwrap()
    else {
        panic!("setup")
    };
    engine
        .call(Call::CheckLocalCli {
            setup: view.setup.clone(),
            trust_executable: true,
            trust_project: true,
        })
        .unwrap();
    engine
        .call(Call::CreateLocalCliSession {
            session: "cli".into(),
            setup: view.setup,
        })
        .unwrap();
    (engine, cli)
}
fn prompt(engine: &mut Engine, request: &str) {
    engine
        .call(Call::Prompt {
            session: "cli".into(),
            request: request.into(),
            text: "synthetic new instruction".into(),
            images: vec![],
            model: None,
        })
        .unwrap();
}

#[test]
fn ordinary_coding_executes_cli_tools_fresh_and_resumed_without_approval_or_native_fallback() {
    let f = Fixture::new();
    let provider = Scripted::echo("must not run");
    let (mut engine, cli) = connect(&f, &provider, ":");
    let shared = engine.handle.shared().clone();
    for (request, mode) in [
        ("one", ExternalPromptMode::Fresh),
        ("two", ExternalPromptMode::Resume),
    ] {
        prompt(&mut engine, request);
        let done = settled(&shared, request);
        assert_eq!(done.state, ExternalRunState::Completed, "{done:?}");
        assert_eq!(done.mode, mode);
        let evidence = done.evidence.as_ref().unwrap();
        assert!(evidence.projection_finished);
        assert_eq!(evidence.supervisor_stop, None);
        assert_eq!(evidence.projection_error, None);
        let usage = done.terminal.as_ref().unwrap().usage.as_ref().unwrap();
        assert_eq!(usage.input_tokens, Some(6));
        assert_eq!(usage.output_tokens, Some(30));
        assert!(
            shared
                .store
                .cli_binding("cli")
                .unwrap()
                .unwrap()
                .context_valid
        );
        assert_eq!(
            done.result,
            Some(PromptResult::Done {
                // Continuation progress survives alongside the final answer.
                text:
                    "Background exploration receivedBackground exploration receivedSynthetic answer"
                        .into(),
                reason: None
            })
        );
        prompt(&mut engine, request); // receipt lookup, no second execution
    }
    assert_eq!(cli.launches().len(), 2);
    assert!(cli.launches()[1].contains("--resume"));
    assert_eq!(
        std::fs::read_to_string(f.project.join("cli-owned-change")).unwrap(),
        "changed by fake CLI\n"
    );
    shared
        .store
        .read(|c| {
            let actions: u64 = c.query_row(
                "SELECT count(*) FROM external_actions WHERE session='cli' AND status='completed'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(actions, 14);
            let approvals: u64 =
                c.query_row("SELECT count(*) FROM external_permissions", [], |r| {
                    r.get(0)
                })?;
            assert_eq!(approvals, 0);
            Ok(())
        })
        .unwrap();
    assert!(provider.requests().is_empty());
}

#[test]
fn clean_stop_allows_a_new_instruction_on_the_same_coding_connection_without_replay() {
    let f = Fixture::new();
    let provider = Scripted::echo("must not run");
    let (mut engine, cli) = connect(
        &f,
        &provider,
        "if [ ! -f result-written ]; then touch result-written; sleep 30; fi",
    );
    let shared = engine.handle.shared().clone();
    let binding = shared.store.cli_binding("cli").unwrap().unwrap();
    prompt(&mut engine, "stop");
    wait(|| f.project.join("result-written").exists());
    assert!(receipt(&shared, "stop").terminal.is_none());
    engine
        .call(Call::Stop {
            session: "cli".into(),
        })
        .unwrap();
    let stopped = settled(&shared, "stop");
    assert_eq!(stopped.outcome, Some(RunOutcome::Aborted));
    assert_eq!(shared.store.cli_binding("cli").unwrap().unwrap(), binding);
    // Retrying delivery of the old request only returns its settled receipt.
    prompt(&mut engine, "stop");
    assert_eq!(cli.launches().len(), 1);
    engine
        .call(Call::Prompt {
            session: "cli".into(),
            request: "continue".into(),
            text: "Inspect existing changes and continue with the next unfinished step.".into(),
            images: vec![],
            model: None,
        })
        .unwrap();
    let continued = settled(&shared, "continue");
    assert_eq!(
        continued.state,
        ExternalRunState::Completed,
        "{continued:?}"
    );
    assert_eq!(continued.mode, ExternalPromptMode::Resume);
    assert_ne!(continued.id, stopped.id);
    let launches = cli.launches();
    assert_eq!(launches.len(), 2);
    assert!(launches[1].contains(&format!("--resume {}", binding.identity.cli_session_id)));
    assert_eq!(
        std::fs::read_to_string(f.project.join("received-prompt")).unwrap(),
        "Inspect existing changes and continue with the next unfinished step.\n"
    );
    prompt(&mut engine, "stop");
    assert_eq!(cli.launches().len(), 2);
    assert_eq!(receipt(&shared, "stop"), stopped);
    assert!(provider.requests().is_empty());
}

#[test]
fn a_failed_held_coding_turn_is_not_hidden_by_a_later_success() {
    let f = Fixture::new();
    let provider = Scripted::echo("must not run");
    let (mut engine, cli) = connect_results(&f, &provider, ":", &[true, false]);
    let shared = engine.handle.shared().clone();
    prompt(&mut engine, "failure");
    let done = settled(&shared, "failure");
    assert_ne!(done.state, ExternalRunState::Completed);
    assert_eq!(
        done.terminal.unwrap().outcome,
        crate::store::ExternalTerminalOutcome::Error
    );
    assert_eq!(done.evidence.unwrap().supervisor_stop, None);
    prompt(&mut engine, "failure");
    assert_eq!(cli.launches().len(), 1);
    assert!(provider.requests().is_empty());
}

#[test]
fn old_clean_stop_recovery_requires_fresh_matching_and_unrevoked_setup() {
    let f = Fixture::new();
    let provider = Scripted::echo("must not run");
    let (mut engine, cli) = connect(
        &f,
        &provider,
        "if [ ! -f result-written ]; then touch result-written; sleep 30; fi",
    );
    let shared = engine.handle.shared().clone();
    prompt(&mut engine, "stop");
    wait(|| f.project.join("result-written").exists());
    engine
        .call(Call::Stop {
            session: "cli".into(),
        })
        .unwrap();
    assert_eq!(settled(&shared, "stop").outcome, Some(RunOutcome::Aborted));
    shared.store.tx(|tx| {
        tx.execute("UPDATE external_prompts SET evidence=json_remove(evidence,'$.continuation_authorized') WHERE session='cli'", [])?;
        tx.execute("UPDATE cli_bindings SET context_valid=0,generation=generation+1 WHERE session='cli'", [])?;
        Ok(())
    }).unwrap();
    let stopped = receipt(&shared, "stop");
    for case in ["alias", "trust", "revoked", "matching"] {
        let Reply::LocalCliSetup(view) = engine
            .call(Call::InspectLocalCli {
                mode: LocalCliMode::Coding,
                executable: cli.executable.display().to_string(),
                target: LocalCliSetupTarget::Existing {
                    session: "cli".into(),
                    model: if case == "alias" {
                        LocalCliModel::Opus
                    } else {
                        LocalCliModel::Sonnet
                    },
                },
            })
            .unwrap()
        else {
            panic!("inspection");
        };
        // Inspection alone must never restore a binding or launch work.
        assert!(
            !shared
                .store
                .cli_binding("cli")
                .unwrap()
                .unwrap()
                .context_valid
        );
        if case == "revoked" {
            engine
                .call(Call::RevokeLocalCliSetup {
                    setup: view.setup.clone(),
                })
                .unwrap();
        }
        let result = engine.call(Call::CheckLocalCli {
            setup: view.setup,
            trust_executable: case != "trust",
            trust_project: true,
        });
        if case == "matching" {
            assert!(
                matches!(result, Ok(Reply::LocalCliSetup(view)) if view.stage == LocalCliSetupStage::MetadataVerified)
            );
        }
        assert_eq!(
            shared
                .store
                .cli_binding("cli")
                .unwrap()
                .unwrap()
                .context_valid,
            case == "matching"
        );
        assert_eq!(
            cli.launches().len(),
            1,
            "metadata/recovery must not run inference"
        );
        assert_eq!(receipt(&shared, "stop"), stopped);
    }
    prompt(&mut engine, "stop");
    assert_eq!(
        cli.launches().len(),
        1,
        "old request remains a settled receipt"
    );
    assert!(provider.requests().is_empty());
}
