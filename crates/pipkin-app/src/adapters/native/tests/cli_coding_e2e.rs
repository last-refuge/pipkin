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
    let help = policy::REQUIRED_FLAGS.join("\n")
        + "\n--settings\n--setting-sources\n--dangerously-skip-permissions\n--append-system-prompt";
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
    discover_for(h, h.state.selected.unwrap())
}
fn discover_for(h: &mut Harness, id: ConversationId) -> (CliApprovalContext, CliApprovalReview) {
    h.until("captured running prompt", |s| {
        s.conversation(id)
            .is_some_and(|c| matches!(c.run, RunState::Running { .. }))
    });
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
            "no verified review: notices={:?}, target={:?}, discover={}, context={:?}, rows={:?}",
            h.notices,
            h.state.conversation(id).map(|c| (
                c.generation,
                c.opened,
                c.profile,
                &c.execution_blocked,
                &c.run
            )),
            h.state.cli_approval_discovery_available(id),
            h.state.cli_approval_context(id),
            h.state
                .dashboard
                .rows
                .iter()
                .filter(|r| r.conversation == Some(id))
                .map(|r| (&r.worker.state, &r.worker.run))
                .collect::<Vec<_>>()
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

#[test]
fn dashboard_cli_reconnect_send_approval_and_resume_preserve_normal_conversation() {
    use pipkin_core::dashboard::{Action, Command as Dash, Freshness};
    let fx = Fixture::start(|_, _| panic!("native inference forbidden"));
    let executable = install(&fx);
    let mut h = setup(&fx, &executable);
    let cli = h.state.selected.unwrap();
    h.dispatch(Command::SelectProviderModel(
        pipkin_core::ProviderModel::Native(h.state.models[0].id.clone()),
    ));
    h.open_new_conversation(&fx.project);
    h.until("normal conversation ready", |s| {
        s.selected != Some(cli) && s.current().is_some_and(|c| c.opened)
    });
    let normal = h.state.selected.unwrap();
    h.dispatch(Command::EditDraft("keep normal draft".into()));
    h.dispatch(Command::Dashboard(Dash::Open));
    h.dispatch(Command::Dashboard(Dash::Group(Some(
        pipkin_core::dashboard::ActivityGroup::All,
    ))));
    h.until("CLI in dashboard", |s| {
        s.dashboard.freshness == Freshness::Live
            && s.dashboard.rows.iter().any(|r| r.conversation == Some(cli))
    });
    let session = h
        .state
        .dashboard
        .rows
        .iter()
        .find(|r| r.conversation == Some(cli))
        .unwrap()
        .worker
        .session
        .clone();
    h.dispatch(Command::Dashboard(Dash::Select(session.clone())));
    h.until("CLI history", |s| !s.dashboard.detail.loading);
    h.dispatch(Command::LocalCli(Setup::BeginExisting {
        conversation: cli,
        session: pipkin_core::EngineSessionId(session.clone()),
    }));
    h.dispatch(Command::LocalCli(Setup::Edit(LocalCliDraft {
        thinking: None,
        mode: CliMode::RestrictedWrite,
        executable: executable.display().to_string(),
        project: fx.project.display().to_string(),
        model: CliModel::Sonnet,
    })));
    h.dispatch(Command::LocalCli(Setup::Inspect));
    h.until("dashboard reconnect inspection", |s| {
        s.local_cli.stage == LocalCliStage::AwaitingTrust
    });
    h.dispatch(Command::LocalCli(Setup::TrustExecutable(true)));
    h.dispatch(Command::LocalCli(Setup::TrustProject(true)));
    h.dispatch(Command::LocalCli(Setup::Check));
    h.until("dashboard reconnect verified", |s| {
        s.local_cli.stage == LocalCliStage::Verified
    });
    h.dispatch(Command::LocalCli(Setup::ReturnToConversation));
    h.until("secondary CLI attached", |s| {
        s.dashboard_attachment_ready(cli)
            && s.conversation(cli).unwrap().execution_blocked.is_none()
    });
    assert_eq!(h.state.selected, Some(normal));
    assert!(h.state.dashboard.open);
    for turn in 1..=2 {
        h.until("live ready CLI row", |s| {
            s.dashboard.freshness == Freshness::Live
                && s.dashboard.selected_row().is_some_and(|r| {
                    matches!(r.worker.state, pipkin_core::dashboard::WorkState::Ready)
                })
        });
        h.dispatch(Command::Dashboard(Dash::Edit("synthetic prompt".into())));
        let row = h.state.dashboard.selected_row().unwrap();
        h.dispatch(Command::Dashboard(Dash::Act {
            session: session.clone(),
            revision: row.worker.revision,
            generation: h.state.dashboard.detail.generation,
            draft_revision: h.state.dashboard_draft().1,
            action: Action::Send,
        }));
        let (context, review) = discover_for(&mut h, cli);
        assert_eq!(context.conversation, cli);
        act(
            &mut h,
            &context,
            A::Decide {
                id: review.id,
                decision: D::AllowOnce,
            },
        );
        h.until("secondary CLI resumed and completed", |s| {
            s.conversation(cli)
                .is_some_and(|c| matches!(c.run, RunState::Idle))
        });
        assert_eq!(h.state.selected, Some(normal));
        assert!(h.state.dashboard.open);
        assert_eq!(
            h.state.dashboard.selected.as_deref(),
            Some(session.as_str())
        );
        assert_eq!(
            h.state.conversation(normal).unwrap().draft.text,
            "keep normal draft"
        );
        assert_eq!(launch_count(&fx), turn);
    }
    assert_eq!(
        std::fs::read_to_string(fx.project.join("effect")).unwrap(),
        "synthetic effect\n"
    );
    assert!(fx.requests().is_empty());
    h.stop();
}

#[test]
fn dashboard_new_claude_thread_uses_chosen_project_and_model_without_navigation() {
    use pipkin_core::dashboard::{Command as Dash, Freshness};
    let fx = Fixture::start(|_, _| panic!("native inference forbidden"));
    let executable = install(&fx);
    let mut h = Harness::start(&fx, &fx.root.join("dashboard-create.db"));
    h.until("setup capability", |s| {
        s.connection.is_ready() && s.local_cli.available
    });
    h.open_new_conversation(&fx.project);
    h.until("main conversation", |s| {
        s.current().is_some_and(|c| c.opened)
    });
    let main = h.state.selected;
    h.dispatch(Command::EditDraft("preserve main draft".into()));
    let chosen = fx.root.join("chosen-project");
    std::fs::create_dir(&chosen).unwrap();
    h.state.prefs.cli_executable = Some(executable.display().to_string());
    h.dispatch(Command::Dashboard(Dash::Open));
    h.dispatch(Command::LocalCli(Setup::BeginDashboardNew(LocalCliDraft {
        project: chosen.display().to_string(),
        model: CliModel::Opus,
        mode: CliMode::Coding,
        ..Default::default()
    })));
    h.until("new thread inspection", |s| {
        s.local_cli.stage == LocalCliStage::AwaitingTrust
    });
    let ticket = h.state.local_cli.ticket.as_ref().unwrap().clone();
    assert_eq!(ticket.project, chosen.display().to_string());
    h.dispatch(Command::LocalCli(Setup::ConfirmAndConnect {
        setup: ticket.setup,
    }));
    h.until("new dashboard thread attached", |s| {
        s.local_cli.stage == LocalCliStage::Created
            && s.local_cli
                .created
                .as_ref()
                .is_some_and(|c| s.dashboard_attachment_ready(c.conversation))
            && s.dashboard.freshness == Freshness::Live
    });
    let created = h.state.local_cli.created.as_ref().unwrap().clone();
    h.until("new graph node", |s| {
        s.dashboard
            .rows
            .iter()
            .any(|r| r.conversation == Some(created.conversation))
    });
    let row = h
        .state
        .dashboard
        .rows
        .iter()
        .find(|r| r.conversation == Some(created.conversation))
        .unwrap();
    assert_eq!(row.worker.cwd, chosen.display().to_string());
    assert!(h.state.dashboard.open);
    assert_eq!(h.state.selected, main);
    assert_eq!(h.state.current().unwrap().draft.text, "preserve main draft");
    assert!(
        !chosen.join("launches").exists(),
        "Creation must not submit a prompt"
    );
    h.dispatch(Command::LocalCli(Setup::BeginExisting {
        conversation: created.conversation,
        session: created.identity.session,
    }));
    h.dispatch(Command::LocalCli(Setup::SessionStatus));
    h.until("saved CLI configuration", |s| {
        s.local_cli.session_status.is_some()
    });
    let saved = h.state.local_cli.session_status.as_ref().unwrap();
    assert_eq!(saved.model, Some(CliModel::Opus));
    assert_eq!(saved.project, chosen.display().to_string());
    h.stop();
}

#[test]
fn dashboard_new_native_thread_uses_chosen_project_and_model_without_navigation() {
    use pipkin_core::dashboard::{Command as Dash, Creation, Freshness};
    let fx = Fixture::start(|_, _| panic!("creation must never call inference"));
    let mut h = Harness::start(&fx, &fx.root.join("dashboard-native-create.db"));
    h.until("native catalog", |s| {
        s.connection.is_ready() && !s.models.is_empty()
    });
    h.open_new_conversation(&fx.project);
    h.until("main conversation", |s| {
        s.current().is_some_and(|c| c.opened)
    });
    let main = h.state.selected;
    h.dispatch(Command::EditDraft("preserve native main draft".into()));
    let prefs = h.state.prefs.clone();
    let chosen = fx.root.join("chosen-native-project");
    std::fs::create_dir(&chosen).unwrap();
    let model = h.state.models[0].id.clone();
    h.dispatch(Command::Dashboard(Dash::Open));
    h.dispatch(Command::Dashboard(Dash::CreateNative {
        project: chosen.display().to_string(),
        model: model.clone(),
    }));
    h.until("new native thread attached", |s| {
        matches!(s.dashboard.creation, Some(Creation::Created(id)) if s.dashboard_attachment_ready(id))
            && s.dashboard.freshness == Freshness::Live
    });
    let Some(Creation::Created(created)) = h.state.dashboard.creation else {
        panic!("creation receipt missing")
    };
    h.until("new graph node", |s| {
        s.dashboard
            .rows
            .iter()
            .any(|r| r.conversation == Some(created))
    });
    let row = h
        .state
        .dashboard
        .rows
        .iter()
        .find(|r| r.conversation == Some(created))
        .unwrap();
    assert_eq!(row.worker.cwd, chosen.display().to_string());
    assert_eq!(
        h.state.conversation(created).unwrap().provider_model,
        Some(pipkin_core::ProviderModel::Native(model))
    );
    assert!(h.state.dashboard.open);
    assert_eq!(h.state.selected, main);
    assert_eq!(
        h.state.current().unwrap().draft.text,
        "preserve native main draft"
    );
    assert_eq!(h.state.prefs.selected_project, prefs.selected_project);
    assert_eq!(h.state.prefs.provider_model, prefs.provider_model);
    assert_eq!(h.state.prefs.model, prefs.model);
    h.stop();
}
