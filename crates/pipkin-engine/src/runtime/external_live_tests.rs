//! Explicitly authorized installed-CLI qualification, never part of offline test runs.
//! A caller-owned permit file is consumed before a request; rerunning cannot resend it.
use super::*;
use crate::config::Env;
use std::io::Write;

#[test]
#[ignore = "explicit metadata-only diagnostic; never submits inference"]
fn authorized_metadata_diagnostic() {
    let root =
        PathBuf::from(std::env::var("PIPKIN_CLI_LIVE_ROOT").expect("explicit artifact root"));
    let executable =
        PathBuf::from(std::env::var("PIPKIN_CLI_LIVE_EXECUTABLE").expect("explicit executable"));
    assert!(root.is_absolute() && root.is_dir());
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join("metadata-diagnostic-consumed"))
        .expect("one-shot metadata check");
    let environment = std::env::vars_os().collect();
    let candidate = preflight::Candidate::inspect(&executable, &root, &environment).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let checked = rt.block_on(preflight::check_for_policy(
        preflight::Request {
            candidate,
            environment,
            trust_executable: true,
            trust_project: true,
            model: policy::ModelAlias::Sonnet,
        },
        CancellationToken::new(),
        preflight::LaunchPolicy::Coding,
    ));
    println!(
        "metadata diagnostic (closed category): {:?}",
        checked.as_ref().err()
    );
    assert!(checked.is_ok());
}

#[test]
#[ignore = "requires a fresh explicitly authorized live-prompt permit and installed CLI"]
fn authorized_coding_round_trip() {
    let root =
        PathBuf::from(std::env::var("PIPKIN_CLI_LIVE_ROOT").expect("explicit artifact root"));
    let executable = std::env::var("PIPKIN_CLI_LIVE_EXECUTABLE").expect("explicit executable");
    assert!(root.is_absolute() && root.is_dir());
    let mut permit = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(
            root.join(match std::env::var("PIPKIN_CLI_LIVE_SLOT").as_deref() {
                Ok("1") => "prompt-1-consumed",
                Ok("2") => "prompt-2-consumed",
                Ok("3") => "prompt-3-consumed",
                _ => panic!(
                    "explicit authorized slot 1, 2 or 3 required; never retry a consumed slot"
                ),
            }),
        )
        .expect("one-shot permit already consumed");
    permit
        .write_all(
            b"Authorized by user: one fresh disposable-project coding prompt. Never replay.\n",
        )
        .unwrap();
    permit.sync_all().unwrap();
    let f = Fixture::new();
    assert!(
        f.dir.path().starts_with(&root),
        "TMPDIR must be the owned artifact root"
    );
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let marker = format!("qualification-{:?}", std::time::SystemTime::now());
        std::fs::write(f.project.join("probe-input.txt"), &marker).unwrap();
        std::fs::write(f.project.join("AGENTS.md"), "Disposable qualification project. Only read probe-input.txt and write probe-output.txt with the same contents. Do not access files outside this directory. No shell commands, network, agents or other work.\n").unwrap();
        let provider = Scripted::echo("native inference must not run");
        let mut config = f.config();
        config.env = Env::process();
        config.enable_local_claude_cli = true;
        config.enable_local_claude_cli_write = true;
        let mut engine = f.start_config(&provider, config);
        let Reply::LocalCliSetup(view) = engine
            .call(Call::InspectLocalCli {
                mode: LocalCliMode::Coding,
                executable,
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
                session: "qualification".into(),
                setup: view.setup,
            })
            .unwrap();
        let Reply::Admitted { .. } = engine.call(Call::Prompt {
            session: "qualification".into(), request: "authorized-one".into(),
            text: "This is a disposable tool-execution test, not a request to explain commands. Read AGENTS.md and probe-input.txt using your Read tool. Use Write to create probe-output.txt containing exactly the input file contents. Do not use Bash, agents, network, or files outside this directory. Reply briefly when finished.".into(),
            images: vec![], model: None,
        }).unwrap() else { panic!("prompt admission") };
        let shared = engine.handle.shared().clone();
        let deadline = Instant::now() + Duration::from_secs(120);
        let receipt = loop {
            let receipt = shared
                .store
                .external_prompt("qualification", "authorized-one")
                .unwrap()
                .unwrap();
            if matches!(
                receipt.state,
                ExternalRunState::Completed
                    | ExternalRunState::Interrupted
                    | ExternalRunState::Unknown
            ) {
                break receipt;
            }
            if Instant::now() >= deadline {
                engine
                    .call(Call::Stop {
                        session: "qualification".into(),
                    })
                    .unwrap();
                panic!("qualification deadline; stopped without retry");
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        println!(
            "state={:?}; terminal={:?}; evidence={:?}",
            receipt.state, receipt.terminal, receipt.evidence
        );
        let names = shared.store.read(|c| {
            Ok(c.prepare("SELECT tool_name,status FROM external_actions WHERE session='qualification' ORDER BY created_at,call_id")?
                .query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?)
        }).unwrap();
        println!("observed actions={names:?}");
        assert_eq!(receipt.state, ExternalRunState::Completed);
        assert!(names.iter().any(|(n, s)| n == "Read" && s == "completed"));
        assert!(names.iter().any(|(n, s)| n == "Write" && s == "completed"));
        assert_eq!(
            std::fs::read_to_string(f.project.join("probe-output.txt")).unwrap(),
            marker
        );
        assert!(provider.requests().is_empty());
    }));
    println!("retained fixture: {}", f.dir.keep().display());
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}
