//! Public RPC route, synthetic CLI only. No installed CLI or native inference.
use super::super::tests::Engine;
use super::*;
use crate::execution::claude_cli::coding_policy;

fn connect(f: &Fixture, provider: &Scripted, tail: &str) -> (Engine, FakeCli) {
    let names = ["Read", "Glob", "Grep", "Edit", "Write", "Bash", "Agent"];
    let mut records =
        vec![json!({"type":"system","subtype":"init","session_id":UUID,"tools":names})];
    for (index, name) in names.iter().enumerate() {
        let id = format!("tool-{index}");
        records.push(json!({"type":"assistant","session_id":UUID,"message":{"id":format!("message-{index}"),"role":"assistant","content":[{"type":"tool_use","id":id,"name":name,"input":{"fixture":true}}]}}));
        if *name == "Agent" {
            records.push(json!({"type":"assistant","session_id":UUID,"parent_tool_use_id":id,"message":{"id":"child","content":[{"type":"text","text":"nested CLI observation"}]}}));
        }
        records.push(json!({"type":"user","session_id":UUID,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":id,"content":"fixture result","is_error":false}]}}));
    }
    records.push(json!({"type":"assistant","session_id":UUID,"message":{"id":"answer","content":[{"type":"text","text":"Synthetic answer"}]}}));
    let cli = FakeCli::with_records(f, tail, true, &records);
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
        assert_eq!(
            done.result,
            Some(PromptResult::Done {
                text: "Synthetic answer".into(),
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
fn stop_after_coding_result_still_cancels_owned_work_and_never_replays() {
    let f = Fixture::new();
    let provider = Scripted::echo("must not run");
    let (mut engine, cli) = connect(&f, &provider, "sleep 30");
    let shared = engine.handle.shared().clone();
    prompt(&mut engine, "stop");
    wait(|| receipt(&shared, "stop").terminal.is_some());
    engine
        .call(Call::Stop {
            session: "cli".into(),
        })
        .unwrap();
    assert_ne!(settled(&shared, "stop").state, ExternalRunState::Completed);
    assert!(
        !shared
            .store
            .cli_binding("cli")
            .unwrap()
            .unwrap()
            .context_valid
    );
    prompt(&mut engine, "stop");
    assert_eq!(cli.launches().len(), 1);
    assert!(provider.requests().is_empty());
}
