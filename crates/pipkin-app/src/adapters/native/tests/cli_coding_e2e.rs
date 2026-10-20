//! Core → native adapter → embedded engine → fake CLI/socket → public approvals.
//! No installed CLI, account request, native inference, or desktop input.
use super::*;
use pipkin_core::{
    CliApprovalAction as A, CliApprovalCommand as C, CliApprovalContext, CliApprovalDecision as D,
    CliApprovalReceiptState as R, CliApprovalReview, CliApprovalStatus as S, CliMode, CliModel,
    LocalCliCommand as Setup, LocalCliDraft, LocalCliStage,
};
use pipkin_engine::execution::claude_cli::{permission_wire as wire, policy};
use std::os::unix::fs::PermissionsExt;

#[path = "cli_approval_recovery_e2e.rs"]
mod recovery;

fn install(fx: &Fixture) -> PathBuf {
    let executable = fx.root.join("fake-coding-cli");
    let image = std::env::current_exe()
        .unwrap()
        .to_str()
        .unwrap()
        .replace('\'', "'\\''");
    let help = policy::REQUIRED_FLAGS.join("\n") + "\n--settings";
    let auth = r#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","email":"fixture@example.invalid","orgId":"fixture"}"#;
    std::fs::write(&executable,format!("#!/bin/sh\ncase \"$1\" in\n--version) printf '%s\\n' '2.1.299 (Claude Code)'; exit ;;\n--help) printf '%s' '{help}'; exit ;;\nesac\nfor arg in \"$@\"; do\nif [ \"$arg\" = '--print' ]; then\nprintf '%s\\0' \"$@\" > fake-args\nprintf 'launch\\n' >> launches\nexport PIPKIN_APP_CODING_FIXTURE=1\nexec '{image}' --exact adapters::native::e2e::cli_coding_e2e::fixture_child --nocapture 3>&1 1>&2\nfi\ndone\nprintf '%s\\n' '{auth}'\n")).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = fx.config();
    assert!(!config.enable_local_claude_cli && !config.enable_local_claude_cli_write);
    config.enable_local_claude_cli = true;
    config.enable_local_claude_cli_write = true;
    config
        .env
        .vars
        .insert("HOME".into(), fx.root.display().to_string());
    config
        .env
        .vars
        .insert("PATH".into(), "/usr/bin:/bin".into());
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
    executable
}
fn setup(fx: &Fixture, executable: &Path) -> Harness {
    let mut h = Harness::start(fx, &fx.root.join("desktop.db"));
    h.until("setup API", |s| {
        s.connection.is_ready() && s.local_cli.available
    });
    h.dispatch(Command::LocalCli(Setup::BeginNew(LocalCliDraft {
        thinking: None,
        mode: CliMode::RestrictedWrite,
        executable: executable.display().to_string(),
        project: fx.project.display().to_string(),
        model: CliModel::Sonnet,
    })));
    h.dispatch(Command::LocalCli(Setup::Inspect));
    h.until("inspection", |s| {
        s.local_cli.stage == LocalCliStage::AwaitingTrust
    });
    h.dispatch(Command::LocalCli(Setup::TrustExecutable(true)));
    h.dispatch(Command::LocalCli(Setup::TrustProject(true)));
    h.dispatch(Command::LocalCli(Setup::Check));
    h.until("metadata", |s| s.local_cli.stage == LocalCliStage::Verified);
    h.dispatch(Command::LocalCli(Setup::Create));
    h.until("owner-scoped coding readiness", |s| {
        s.current().is_some_and(|c| {
            c.opened
                && c.profile == pipkin_core::ExecutionProfile::LocalCliRestrictedWrite
                && c.execution_blocked.is_none()
        })
    });
    assert!(!fx.project.join("launches").exists());
    h
}
fn discover(h: &mut Harness) -> (CliApprovalContext, CliApprovalReview) {
    h.until("captured running prompt", |s| {
        s.current()
            .is_some_and(|c| matches!(c.run, RunState::Running { .. }))
    });
    let id = h.state.selected.unwrap();
    let deadline = Instant::now() + WAIT;
    // Explicit fixture polling to wait for the fake process. No production polling is added.
    loop {
        if h.state.cli_approval_discovery_available(id) {
            h.dispatch(Command::CliApproval(C::Discover(id)));
        }
        h.pump();
        if let Some(context) = h.state.cli_approval_context(id)
            && let Some(review) = h
                .state
                .cli_approvals
                .items(context)
                .iter()
                .find_map(|r| r.review.clone())
        {
            return (context.clone(), review);
        }
        assert!(
            Instant::now() < deadline,
            "no verified review: {:?}",
            h.notices
        );
        thread::sleep(Duration::from_millis(5));
    }
}
fn act(h: &mut Harness, c: &CliApprovalContext, action: A) {
    h.dispatch(Command::CliApproval(C::Act {
        context: c.clone(),
        action: Box::new(action),
    }));
}
fn launch_count(fx: &Fixture) -> usize {
    std::fs::read_to_string(fx.project.join("launches"))
        .unwrap()
        .lines()
        .count()
}

#[test]
fn full_coding_approval_flow_allows_once_and_resumes_without_native_tools_or_inference() {
    let fx = Fixture::start(|_, _| panic!("native inference forbidden"));
    git(&fx.project, &["init", "-q"]);
    std::fs::write(fx.project.join("unrelated.txt"), "not a CLI effect\n").unwrap();
    let executable = install(&fx);
    let mut h = setup(&fx, &executable);
    for turn in 1..=2 {
        h.send("synthetic prompt");
        assert_eq!(
            h.state
                .current()
                .unwrap()
                .pending_intent
                .as_ref()
                .unwrap()
                .model,
            None
        );
        let (context, review) = discover(&mut h);
        assert!(review.arguments.contains("synthetic effect"));
        assert!(
            h.state
                .cli_approval_availability(&context, Some(&review.id))
                .decide
        );
        act(
            &mut h,
            &context,
            A::Decide {
                id: review.id.clone(),
                decision: D::AllowOnce,
            },
        );
        assert!(
            !h.state
                .cli_approval_availability(&context, Some(&review.id))
                .decide
        );
        // An immediate second intent is rejected before another backend effect.
        act(
            &mut h,
            &context,
            A::Decide {
                id: review.id.clone(),
                decision: D::Deny,
            },
        );
        h.until("clean external completion", is_idle);
        assert_eq!(
            std::fs::read_to_string(fx.project.join("effect")).unwrap(),
            "synthetic effect\n"
        );
        act(
            &mut h,
            &context,
            A::Lookup {
                id: review.id.clone(),
            },
        );
        h.until("explicit consumed receipt", |s| {
            s.cli_approvals
                .items(&context)
                .iter()
                .any(|i| i.status == S::Recorded(R::Consumed))
        });
        assert_eq!(
            h.state.cli_approvals.items(&context)[0].attempted,
            Some(D::AllowOnce)
        );
        assert!(
            !h.state
                .cli_approval_availability(&context, Some(&review.id))
                .decide
        );
        assert!(
            h.state
                .current()
                .unwrap()
                .items
                .iter()
                .all(|i| !matches!(i.kind, ItemKind::Tool(_)))
        );
        h.until(
            "captured project Git state, without native edit evidence",
            |s| {
                s.current().is_some_and(|c| {
                    c.changes_state == ChangesState::Ready
                        && c.changes.iter().any(|f| f.path == "effect" && f.added == 1)
                        && c.changes.iter().any(|f| f.path == "unrelated.txt")
                })
            },
        );
        assert_eq!(launch_count(&fx), turn);
        assert!(fx.requests().is_empty());
    }
    assert!(
        std::fs::read(fx.project.join("fake-args"))
            .unwrap()
            .windows(8)
            .any(|w| w == b"--resume")
    );
    git(&fx.project, &["add", "-A"]);
    git(&fx.project, &["commit", "-q", "-m", "synthetic workspace"]);
    h.dispatch(Command::RefreshChanges);
    h.until(
        "committed CLI workspace clears without fabricated tool evidence",
        |s| {
            s.current()
                .is_some_and(|c| c.changes_state == ChangesState::Ready && c.changes.is_empty())
        },
    );
    h.stop();
}

#[test]
fn lost_decision_reply_never_resends_and_recovers_only_through_explicit_lookup() {
    let fx = Fixture::start(|_, _| panic!("native inference forbidden"));
    let executable = install(&fx);
    let mut h = setup(&fx, &executable);
    h.send("synthetic prompt");
    let (context, review) = discover(&mut h);
    fx.faults.drop_approval_reply.store(true, Ordering::SeqCst);
    act(
        &mut h,
        &context,
        A::Decide {
            id: review.id.clone(),
            decision: D::AllowOnce,
        },
    );
    h.until("uncertain decision response", |s| {
        s.cli_approvals
            .items(&context)
            .iter()
            .any(|i| i.status == S::Unknown)
    });
    act(
        &mut h,
        &context,
        A::Decide {
            id: review.id.clone(),
            decision: D::AllowOnce,
        },
    );
    act(
        &mut h,
        &context,
        A::Decide {
            id: review.id.clone(),
            decision: D::Deny,
        },
    );
    h.until("independently completed external run", is_idle);
    assert_eq!(fx.faults.approval_decisions.load(Ordering::SeqCst), 1);
    assert_eq!(launch_count(&fx), 1);
    assert_eq!(
        std::fs::read_to_string(fx.project.join("effect")).unwrap(),
        "synthetic effect\n"
    );
    assert!(
        h.state
            .cli_approvals
            .items(&context)
            .iter()
            .all(|i| i.status == S::Unknown)
    );
    act(
        &mut h,
        &context,
        A::Lookup {
            id: review.id.clone(),
        },
    );
    h.until("read-only consumed recovery", |s| {
        s.cli_approvals
            .items(&context)
            .iter()
            .any(|i| i.status == S::Recorded(R::Consumed))
    });
    assert_eq!(fx.faults.approval_decisions.load(Ordering::SeqCst), 1);
    assert!(
        !h.state
            .cli_approval_availability(&context, Some(&review.id))
            .decide
    );
    assert!(fx.requests().is_empty());
    h.stop();
}

#[test]
fn full_coding_deny_and_stop_retire_reviews_without_effects_or_retry() {
    for deny in [false, true] {
        let fx = Fixture::start(|_, _| panic!("native inference forbidden"));
        let executable = install(&fx);
        let mut h = setup(&fx, &executable);
        h.send("synthetic prompt");
        let (context, review) = discover(&mut h);
        if deny {
            act(
                &mut h,
                &context,
                A::Decide {
                    id: review.id.clone(),
                    decision: D::Deny,
                },
            );
        } else {
            h.dispatch(Command::Cancel);
        }
        h.until("coding scope retired", |s| {
            s.current().is_some_and(|c| {
                c.execution_blocked.is_some() == deny
                    && !matches!(
                        c.run,
                        RunState::Submitting { .. }
                            | RunState::Running { .. }
                            | RunState::Stopping { .. }
                    )
            })
        });
        assert!(
            !h.state
                .cli_approval_availability(&context, Some(&review.id))
                .decide
        );
        act(
            &mut h,
            &context,
            A::Decide {
                id: review.id.clone(),
                decision: D::AllowOnce,
            },
        );
        h.pump();
        assert!(!fx.project.join("effect").exists());
        assert_eq!(launch_count(&fx), 1);
        assert!(!h.state.availability().retry);
        assert!(fx.requests().is_empty());
        h.stop();
    }
}

#[test]
fn fixture_child() {
    if std::env::var_os("PIPKIN_APP_CODING_FIXTURE").is_none() {
        return;
    }
    use std::{
        io::{Read, Write},
        os::fd::FromRawFd,
    };
    let mut output = unsafe { std::fs::File::from_raw_fd(3) };
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).unwrap();
    assert_eq!(input, "synthetic prompt\n");
    let raw = std::fs::read("fake-args").unwrap();
    let argv: Vec<_> = raw
        .split(|b| *b == 0)
        .filter(|b| !b.is_empty())
        .map(|b| std::str::from_utf8(b).unwrap())
        .collect();
    let uuid = argv[argv
        .iter()
        .position(|a| matches!(*a, "--session-id" | "--resume"))
        .unwrap()
        + 1];
    let settings: serde_json::Value =
        serde_json::from_str(argv[argv.iter().position(|a| *a == "--settings").unwrap() + 1])
            .unwrap();
    let command = settings["hooks"]["PermissionRequest"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    let socket = command
        .split(" '--socket' ")
        .nth(1)
        .unwrap()
        .strip_prefix('\'')
        .unwrap()
        .strip_suffix('\'')
        .unwrap();
    let cwd = std::env::current_dir().unwrap();
    writeln!(
        output,
        "{}",
        serde_json::json!({"type":"system","subtype":"init","session_id":uuid,"tools":["Write"]})
    )
    .unwrap();
    writeln!(output,"{}",serde_json::json!({"type":"assistant","session_id":uuid,"message":{"id":"answer","role":"assistant","content":[{"type":"text","text":"Synthetic external answer"},{"type":"tool_use","id":"write-a","name":"Write","input":{"never":"native arguments"}}]}})).unwrap();
    let payload=serde_json::json!({"session_id":uuid,"cwd":cwd,"permission_mode":"default","hook_event_name":"PermissionRequest","tool_name":"Write","tool_input":{"file_path":cwd.join("effect"),"content":"synthetic effect\n"}}).to_string();
    let frame = wire::RequestFrame::new(
        wire::Binding::from_hex(&std::env::var("PIPKIN_CLI_PERMISSION_NONCE").unwrap()).unwrap(),
        payload,
    )
    .unwrap();
    let reply = (|| {
        // Fixture-only framing; production helper/decoder qualification is separate.
        let mut peer = std::os::unix::net::UnixStream::connect(socket).ok()?;
        peer.set_write_timeout(Some(Duration::from_secs(2))).ok()?;
        peer.set_read_timeout(Some(Duration::from_secs(20))).ok()?;
        let packet = serde_json::to_vec(&frame).ok()?;
        peer.write_all(&(packet.len() as u32).to_be_bytes()).ok()?;
        peer.write_all(&packet).ok()?;
        let mut header = [0; 4];
        peer.read_exact(&mut header).ok()?;
        let length = u32::from_be_bytes(header) as usize;
        if length > 1024 {
            return None;
        }
        let mut packet = vec![0; length];
        peer.read_exact(&mut packet).ok()?;
        let reply = serde_json::from_slice::<wire::ReplyFrame>(&packet).ok()?;
        frame.decision_from(&reply).ok()
    })();
    if reply == Some(wire::Decision::AllowOnce) {
        std::fs::write("effect", "synthetic effect\n").unwrap();
        writeln!(output,"{}",serde_json::json!({"type":"user","session_id":uuid,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"write-a","content":"synthetic effect","is_error":false}]}})).unwrap();
        writeln!(output,"{}",serde_json::json!({"type":"result","session_id":uuid,"subtype":"success","is_error":false,"result":"not answer text"})).unwrap();
    }
    std::process::exit(0);
}
