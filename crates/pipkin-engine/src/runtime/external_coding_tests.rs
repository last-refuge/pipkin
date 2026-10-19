//! Fake CLI subprocess + real actor, socket, gate and SQLite. No Claude/account call.
use super::*;
use crate::execution::claude_cli::{permission::Decision, permission_wire as wire};
use crate::runtime::{
    external_coding::{Admission, Feedback},
    external_permission_server::Control,
    external_permissions::Review,
};
use tokio::sync::{mpsc, oneshot};

#[path = "external_coding_crash_tests.rs"]
mod crash;
#[path = "local_cli_coding_runtime_tests.rs"]
mod public_route;

fn fake(f: &Fixture, behavior: &str) -> FakeCli {
    let cli = FakeCli::new(f, "", false);
    let help = policy::REQUIRED_FLAGS
        .iter()
        .chain(["--settings"].iter())
        .map(|flag| format!("{flag} fixture\n"))
        .collect::<String>();
    let image = std::env::current_exe().unwrap();
    let image = image.to_str().unwrap().replace('\'', "'\\''");
    let auth = r#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","email":"fixture@example.invalid","orgId":"fixture-org"}"#;
    let script = format!(
        "#!/bin/sh\ncase \"$1\" in\n--version) printf '%s\\n' '2.1.299 (Claude Code)'; exit ;;\n--help) printf '%s' '{help}'; exit ;;\nesac\nfor arg in \"$@\"; do\nif [ \"$arg\" = '--print' ]; then\nprintf '%s\\0' \"$@\" > fake-args\nprintf 'launch\\n' >> launches\nexport PIPKIN_CODING_FIXTURE=1\nexec '{image}' --exact runtime::external_tests::coding::coding_fixture_child --nocapture 3>&1 1>&2\nfi\ndone\nprintf '%s\\n' '{auth}'\n"
    );
    crate::execution::claude_cli::test_support::write_script(&cli.executable, script).unwrap();
    std::fs::write(f.project.join("behavior"), behavior).unwrap();
    cli
}
fn prepared(cli: &FakeCli, shared: &Shared) -> preflight::Prepared {
    shared
        .handle
        .block_on(preflight::check_for_policy(
            preflight::Request {
                candidate: preflight::Candidate::inspect(
                    &cli.executable,
                    &cli.project,
                    &cli.environment,
                )
                .unwrap(),
                trust_executable: true,
                trust_project: true,
                environment: cli.environment.clone(),
                model: policy::ModelAlias::Sonnet,
            },
            CancellationToken::new(),
            preflight::LaunchPolicy::RestrictedWrite,
        ))
        .unwrap()
}
fn seed(cli: &FakeCli, shared: &Shared) {
    // Synthetic internal actor fixtures use owner 7, independently of public setup grants.
    shared.local_cli_approvals.connect(7);
    shared
        .store
        .create_cli_session(
            "cli",
            "create-cli",
            &prepared(cli, shared).checked_binding(UUID).unwrap(),
            1,
        )
        .unwrap();
}
fn submit(
    cli: &FakeCli,
    shared: &Arc<Shared>,
    parent: PathBuf,
    request: &str,
    revoked: CancellationToken,
) -> (Arc<Control>, mpsc::Receiver<Review>) {
    submit_owner(cli, shared, parent, request, revoked, 7)
}
fn submit_owner(
    cli: &FakeCli,
    shared: &Arc<Shared>,
    parent: PathBuf,
    request: &str,
    revoked: CancellationToken,
    owner: u64,
) -> (Arc<Control>, mpsc::Receiver<Review>) {
    let (control, receiver) = oneshot::channel();
    let (reviews, review_rx) = mpsc::channel(16);
    let prepared = prepared(cli, shared);
    shared.handle.block_on(async {
        shared
            .prompt_external_coding(
                "cli",
                request,
                "synthetic prompt",
                prepared,
                Admission {
                    owner,
                    expires: Instant::now() + Duration::from_secs(30),
                    revoked,
                    parent,
                    feedback: Feedback::Observe { reviews, control },
                },
            )
            .await
            .unwrap();
        (
            tokio::time::timeout(Duration::from_secs(5), receiver)
                .await
                .unwrap()
                .unwrap(),
            review_rx,
        )
    })
}
fn review(shared: &Shared, receiver: &mut mpsc::Receiver<Review>) -> Review {
    shared.handle.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), receiver.recv())
            .await
            .unwrap()
            .unwrap()
    })
}
fn no_native(shared: &Shared, provider: &Scripted) {
    assert!(provider.requests().is_empty());
    shared
        .store
        .read(|c| {
            for table in ["runs", "requests", "tool_calls"] {
                assert_eq!(
                    c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                        .get::<_, u64>(0))?,
                    0
                );
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn coding_fixture_child() {
    if std::env::var_os("PIPKIN_CODING_FIXTURE").is_none() {
        return;
    }
    use std::{
        io::{Read, Write},
        os::fd::FromRawFd,
    };
    // The shell wrapper preserves the supervisor's stdout in fd3. Test-harness
    // banners go only to bounded stderr. No Python or external helper dependency.
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
    let behavior = std::fs::read_to_string("behavior").unwrap();
    std::fs::write("fixture-pid", std::process::id().to_string()).unwrap();
    writeln!(
        output,
        "{}",
        json!({"type":"system","subtype":"init","session_id":uuid,"tools":["Write"]})
    )
    .unwrap();
    writeln!(output,"{}",json!({"type":"assistant","session_id":uuid,"message":{"id":"answer","role":"assistant","content":[
        {"type":"text","text":"before"}, {"type":"tool_use","id":"write-a","name":"Write","input":{"must":"not become native arguments"}}, {"type":"text","text":"after"}]}})).unwrap();
    let payload=json!({"session_id":uuid,"cwd":cwd,"permission_mode":"default","hook_event_name":"PermissionRequest","tool_name":"Write","tool_input":{"file_path":cwd.join("effect"),"content":"synthetic effect\n"}}).to_string();
    let frame = wire::RequestFrame::new(
        wire::Binding::from_hex(&std::env::var("PIPKIN_CLI_PERMISSION_NONCE").unwrap()).unwrap(),
        payload,
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let decision = runtime.block_on(async {
        let mut peer = tokio::net::UnixStream::connect(socket).await.unwrap();
        wire::write_request(
            &mut peer,
            &frame,
            tokio::time::Instant::now() + Duration::from_secs(2),
        )
        .await
        .unwrap();
        if behavior == "disconnect" {
            // The post-review disconnect test must not race actor shutdown against review
            // delivery. Immediate disconnect is tested separately and may produce no review.
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            while !std::path::Path::new("disconnect-after-review").exists() {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "review barrier timed out"
                );
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            return None;
        }
        if behavior == "disconnect-immediate" {
            return None;
        }
        wire::read_reply(
            &mut peer,
            &frame,
            tokio::time::Instant::now() + Duration::from_secs(20),
        )
        .await
        .ok()
    });
    if decision == Some(wire::Decision::AllowOnce) {
        if behavior == "crash-after-allow" {
            std::fs::write("allow-paused", "yes").unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            while !std::path::Path::new("release-after-crash").exists() {
                if std::time::Instant::now() >= deadline
                    || !std::path::Path::new("behavior").exists()
                {
                    std::process::exit(82);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        std::fs::write("effect", "synthetic effect\n").unwrap();
        let is_error = if behavior == "unknown-result" {
            serde_json::Value::Null
        } else {
            json!(false)
        };
        let mut result = json!({"type":"user","session_id":uuid,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"write-a","content":"reported effect","is_error":is_error}]}});
        if behavior == "omit-error" {
            result["message"]["content"][0]
                .as_object_mut()
                .unwrap()
                .remove("is_error");
        }
        writeln!(output, "{result}").unwrap();
        writeln!(output,"{}",json!({"type":"result","session_id":uuid,"subtype":"success","is_error":false,"result":"not answer text"})).unwrap();
    }
    std::process::exit(if behavior == "fail-exit" { 7 } else { 0 });
}

#[test]
fn coding_fresh_resume_and_duplicates_use_real_broker_without_native_execution() {
    let f = Fixture::new();
    let provider = Scripted::echo("never");
    let _engine = f.start(&provider);
    let shared = _engine.handle.shared().clone();
    let cli = fake(&f, "normal");
    seed(&cli, &shared);
    for (request, mode) in [
        ("first", ExternalPromptMode::Fresh),
        ("second", ExternalPromptMode::Resume),
    ] {
        let (control, mut reviews) = submit(
            &cli,
            &shared,
            f.dir.path().into(),
            request,
            CancellationToken::new(),
        );
        let r = review(&shared, &mut reviews);
        assert!(control.decide(8, &r.ticket, Decision::AllowOnce).is_err());
        control.decide(7, &r.ticket, Decision::AllowOnce).unwrap();
        let done = settled(&shared, request);
        assert_eq!(done.mode, mode);
        assert_eq!(done.state, ExternalRunState::Completed);
        assert!(matches!(done.result,Some(PromptResult::Done { text,.. }) if text=="beforeafter"));
        assert!(control.decide(7, &r.ticket, Decision::AllowOnce).is_err());
        assert_eq!(
            control.lookup(7, &r.ticket).unwrap().unwrap().state,
            crate::store::ExternalPermissionState::Consumed
        );
    }
    assert_eq!(
        std::fs::read_to_string(f.project.join("effect")).unwrap(),
        "synthetic effect\n"
    );
    // Read-only duplicate path precedes missing readiness and does not create a listener.
    assert!(
        shared
            .external_duplicate("cli", "second", "synthetic prompt")
            .unwrap()
            .is_some()
    );
    assert_eq!(
        std::fs::read_to_string(f.project.join("launches"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    assert!(
        std::fs::read(f.project.join("fake-args"))
            .unwrap()
            .windows(8)
            .any(|w| w == b"--resume")
    );
    no_native(&shared, &provider);
}

#[test]
fn coding_protocol_default_success_flag_resolves_reported_action_without_native_execution() {
    let f = Fixture::new();
    let provider = Scripted::echo("forbidden native inference");
    let engine = f.start(&provider);
    let shared = engine.handle.shared().clone();
    let cli = fake(&f, "omit-error");
    seed(&cli, &shared);
    let (control, mut reviews) = submit(
        &cli,
        &shared,
        f.dir.path().into(),
        "omitted-flag",
        CancellationToken::new(),
    );
    let review = review(&shared, &mut reviews);
    control
        .decide(7, &review.ticket, Decision::AllowOnce)
        .unwrap();
    let receipt = settled(&shared, "omitted-flag");
    assert_eq!(receipt.state, ExternalRunState::Completed);
    assert!(receipt.evidence.unwrap().projection_finished);
    assert_eq!(
        control.lookup(7, &review.ticket).unwrap().unwrap().state,
        crate::store::ExternalPermissionState::Consumed
    );
    assert_eq!(
        std::fs::read(f.project.join("effect")).unwrap(),
        b"synthetic effect\n"
    );
    no_native(&shared, &provider);
}

#[test]
fn coding_stop_deny_and_revoke_fence_before_clean_cancellation() {
    for mode in ["stop", "deny", "revoke"] {
        let f = Fixture::new();
        let provider = Scripted::echo("never");
        let _engine = f.start(&provider);
        let shared = _engine.handle.shared().clone();
        let cli = fake(&f, "normal");
        seed(&cli, &shared);
        let revoked = CancellationToken::new();
        let (control, mut reviews) =
            submit(&cli, &shared, f.dir.path().into(), "p", revoked.clone());
        let r = review(&shared, &mut reviews);
        match mode {
            "stop" => {
                shared
                    .handle
                    .block_on(shared.stop_external_prompt("cli"))
                    .unwrap();
            }
            "deny" => {
                control.decide(7, &r.ticket, Decision::Deny).unwrap();
            }
            _ => revoked.cancel(),
        }
        let done = settled(&shared, "p");
        assert_eq!(
            done.state,
            if mode == "deny" {
                ExternalRunState::Unknown
            } else {
                ExternalRunState::Interrupted
            }
        );
        assert!(done.evidence.as_ref().unwrap().cancelled_clean_confirmed);
        assert!(!f.project.join("effect").exists());
        assert!(
            !shared
                .store
                .cli_binding("cli")
                .unwrap()
                .unwrap()
                .context_valid
        );
        assert!(control.decide(7, &r.ticket, Decision::AllowOnce).is_err());
        no_native(&shared, &provider);
    }
}

fn rpc(connection: &crate::host::Connection, call: Call) -> Result<Reply, ProtoError> {
    connection
        .send(ClientFrame {
            id: 900,
            lane: call.lane(),
            call,
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let ServerFrame::Reply { id: 900, result } = connection
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap()
        {
            return result;
        }
    }
}

#[test]
fn pull_only_actor_owns_control_without_observers_or_background_drainers() {
    for mode in ["allow", "deny", "stop", "revoke"] {
        let f = Fixture::new();
        let provider = Scripted::echo("never");
        let mut engine = f.start(&provider);
        let shared = engine.handle.shared().clone();
        let cli = fake(&f, "normal");
        seed(&cli, &shared);
        let revoked = CancellationToken::new();
        let admitted = shared
            .handle
            .block_on(shared.prompt_external_coding(
                "cli",
                "p",
                "synthetic prompt",
                prepared(&cli, &shared),
                Admission {
                    owner: engine.conn.client,
                    expires: Instant::now() + Duration::from_secs(30),
                    revoked: revoked.clone(),
                    parent: f.dir.path().into(),
                    feedback: Feedback::PullOnly,
                },
            ))
            .unwrap();
        let Reply::Admitted {
            run: Some(prompt), ..
        } = admitted
        else {
            panic!("receipt required")
        };
        // Explicit test polling only: production/frontend performs no automatic discovery.
        let deadline = Instant::now() + Duration::from_secs(5);
        let review = loop {
            let Reply::LocalCliApprovals { run, mut reviews } = engine
                .call(Call::LocalCliApprovals {
                    session: "cli".into(),
                })
                .unwrap()
            else {
                panic!("approval view")
            };
            if let Some(review) = reviews.pop() {
                assert_eq!(run.as_ref().unwrap().prompt, prompt);
                break review;
            }
            assert!(
                Instant::now() < deadline,
                "owned pull-only review was not available"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        match mode {
            "allow" | "deny" => {
                let decision = if mode == "allow" {
                    LocalCliApprovalDecision::AllowOnce
                } else {
                    LocalCliApprovalDecision::Deny
                };
                assert!(matches!(
                    engine
                        .call(Call::DecideLocalCliApproval {
                            id: review.id.clone(),
                            decision
                        })
                        .unwrap(),
                    Reply::LocalCliApproval { receipt: Some(_) }
                ));
            }
            "stop" => {
                engine
                    .call(Call::Stop {
                        session: "cli".into(),
                    })
                    .unwrap();
            }
            _ => revoked.cancel(),
        }
        let done = settled(&shared, "p");
        assert_eq!(
            done.state,
            match mode {
                "allow" => ExternalRunState::Completed,
                "deny" => ExternalRunState::Unknown,
                _ => ExternalRunState::Interrupted,
            }
        );
        assert_eq!(f.project.join("effect").exists(), mode == "allow");
        assert!(
            matches!(engine.call(Call::LocalCliApprovals { session: "cli".into() }).unwrap(), Reply::LocalCliApprovals { run: None, reviews } if reviews.is_empty())
        );
        assert!(matches!(
            engine
                .call(Call::LookupLocalCliApproval { id: review.id })
                .unwrap(),
            Reply::LocalCliApproval { receipt: Some(_) }
        ));
        wait(|| shared.external_tasks.is_empty());
        no_native(&shared, &provider);
    }
}

#[test]
fn public_approval_rpc_is_owner_scoped_exact_and_duplicate_delivery_is_impossible() {
    let f = Fixture::new();
    let provider = Scripted::echo("never");
    let mut engine = f.start(&provider);
    let shared = engine.handle.shared().clone();
    let cli = fake(&f, "normal");
    seed(&cli, &shared);
    let owner = engine.conn.client;
    let (control, mut reviews) = submit_owner(
        &cli,
        &shared,
        f.dir.path().into(),
        "p",
        CancellationToken::new(),
        owner,
    );
    let original = review(&shared, &mut reviews);
    let Reply::LocalCliApprovals {
        run: Some(run),
        reviews,
    } = engine
        .call(Call::LocalCliApprovals {
            session: "cli".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].arguments, original.arguments);
    assert_eq!(reviews[0].id.run, run);
    let id = reviews[0].id.clone();
    let json = serde_json::to_string(&reviews).unwrap();
    assert!(!json.contains(UUID));
    assert!(!format!("{reviews:?}").contains(&original.arguments));
    assert!(shared.local_cli_approvals.install(&control).is_err()); // Cannot replace a live registration.
    let foreign = engine.handle.connect("foreign");
    assert!(
        matches!(rpc(&foreign, Call::LocalCliApprovals { session: "cli".into() }).unwrap(), Reply::LocalCliApprovals { run: None, reviews } if reviews.is_empty())
    );
    assert!(matches!(
        rpc(&foreign, Call::LookupLocalCliApproval { id: id.clone() }).unwrap(),
        Reply::LocalCliApproval { receipt: None }
    ));
    assert!(
        rpc(
            &foreign,
            Call::DecideLocalCliApproval {
                id: id.clone(),
                decision: LocalCliApprovalDecision::AllowOnce
            }
        )
        .is_err()
    );
    for field in 0..7 {
        let mut wrong = id.clone();
        match field {
            0 => wrong.run.epoch += 1,
            1 => wrong.run.generation += 1,
            2 => wrong.run.op.push('x'),
            3 => wrong.run.prompt += 1,
            4 => wrong.invocation = "00".repeat(32),
            5 => wrong.arguments_sha256 = "00".repeat(32),
            _ => wrong.run.session.push('x'),
        }
        assert!(
            engine
                .call(Call::DecideLocalCliApproval {
                    id: wrong,
                    decision: LocalCliApprovalDecision::AllowOnce
                })
                .is_err()
        );
    }
    assert!(!f.project.join("effect").exists());
    assert!(
        matches!(engine.call(Call::LookupLocalCliApproval { id: id.clone() }).unwrap(), Reply::LocalCliApproval { receipt: Some(r) } if r.state == LocalCliApprovalState::Pending)
    );
    assert!(
        matches!(engine.call(Call::DecideLocalCliApproval { id: id.clone(), decision: LocalCliApprovalDecision::AllowOnce }).unwrap(), Reply::LocalCliApproval { receipt: Some(r) } if r.state == LocalCliApprovalState::Consumed)
    );
    assert_eq!(settled(&shared, "p").state, ExternalRunState::Completed);
    assert!(f.project.join("effect").exists());
    wait(|| {
        matches!(
            shared.local_cli_approvals(owner, "cli").unwrap(),
            Reply::LocalCliApprovals { run: None, .. }
        )
    });
    // Lookup/duplicate acknowledgement work after the actor/registry has retired; neither can send bytes.
    assert!(
        matches!(engine.call(Call::LookupLocalCliApproval { id: id.clone() }).unwrap(), Reply::LocalCliApproval { receipt: Some(r) } if r.state == LocalCliApprovalState::Consumed)
    );
    assert!(
        matches!(engine.call(Call::DecideLocalCliApproval { id: id.clone(), decision: LocalCliApprovalDecision::AllowOnce }).unwrap(), Reply::LocalCliApproval { receipt: Some(r) } if r.state == LocalCliApprovalState::Consumed)
    );
    assert!(
        engine
            .call(Call::DecideLocalCliApproval {
                id,
                decision: LocalCliApprovalDecision::Deny
            })
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(f.project.join("launches"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    no_native(&shared, &provider);
}

#[test]
fn public_deny_or_owner_disconnect_retires_without_allow_and_cannot_reinstall() {
    for disconnect in [false, true] {
        let f = Fixture::new();
        let provider = Scripted::echo("never");
        let mut engine = f.start(&provider);
        let shared = engine.handle.shared().clone();
        let cli = fake(&f, "normal");
        seed(&cli, &shared);
        let owner = engine.conn.client;
        let (control, mut reviews) = submit_owner(
            &cli,
            &shared,
            f.dir.path().into(),
            "p",
            CancellationToken::new(),
            owner,
        );
        review(&shared, &mut reviews);
        let Reply::LocalCliApprovals { reviews, .. } = engine
            .call(Call::LocalCliApprovals {
                session: "cli".into(),
            })
            .unwrap()
        else {
            panic!()
        };
        let id = reviews[0].id.clone();
        if disconnect {
            let replacement = engine.handle.connect("replacement");
            drop(std::mem::replace(&mut engine.conn, replacement));
        } else {
            assert!(
                matches!(engine.call(Call::DecideLocalCliApproval { id: id.clone(), decision: LocalCliApprovalDecision::Deny }).unwrap(), Reply::LocalCliApproval { receipt: Some(r) } if r.state == LocalCliApprovalState::Denied)
            );
        }
        assert_eq!(settled(&shared, "p").state, ExternalRunState::Unknown);
        if disconnect {
            assert!(shared.local_cli_approvals.install(&control).is_err());
            assert!(matches!(
                engine.call(Call::LookupLocalCliApproval { id }).unwrap(),
                Reply::LocalCliApproval { receipt: None }
            ));
        } else {
            assert!(
                matches!(engine.call(Call::LookupLocalCliApproval { id }).unwrap(), Reply::LocalCliApproval { receipt: Some(r) } if r.state == LocalCliApprovalState::Denied)
            );
        }
        assert!(!f.project.join("effect").exists());
        no_native(&shared, &provider);
    }
}

#[test]
fn public_decision_after_commit_failure_is_uncertain_and_lookup_never_releases_again() {
    let f = Fixture::new();
    let provider = Scripted::echo("never");
    let mut engine = f.start(&provider);
    let shared = engine.handle.shared().clone();
    let cli = fake(&f, "normal");
    seed(&cli, &shared);
    let owner = engine.conn.client;
    let (control, mut reviews) = submit_owner(
        &cli,
        &shared,
        f.dir.path().into(),
        "p",
        CancellationToken::new(),
        owner,
    );
    review(&shared, &mut reviews);
    let Reply::LocalCliApprovals { reviews, .. } = engine
        .call(Call::LocalCliApprovals {
            session: "cli".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    let id = reviews[0].id.clone();
    control.cancel_after_commit_for_test();
    assert_eq!(
        engine
            .call(Call::DecideLocalCliApproval {
                id: id.clone(),
                decision: LocalCliApprovalDecision::AllowOnce
            })
            .unwrap_err()
            .code,
        ErrorCode::Cancelled
    );
    assert_eq!(settled(&shared, "p").state, ExternalRunState::Unknown);
    assert!(
        matches!(engine.call(Call::LookupLocalCliApproval { id: id.clone() }).unwrap(), Reply::LocalCliApproval { receipt: Some(r) } if r.state == LocalCliApprovalState::Consumed)
    );
    assert!(
        matches!(engine.call(Call::DecideLocalCliApproval { id, decision: LocalCliApprovalDecision::AllowOnce }).unwrap(), Reply::LocalCliApproval { receipt: Some(r) } if r.state == LocalCliApprovalState::Consumed)
    );
    assert!(!f.project.join("effect").exists());
    assert_eq!(
        std::fs::read_to_string(f.project.join("launches"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    no_native(&shared, &provider);
}

#[test]
fn approval_registry_retirement_does_not_wait_for_publication_before_owned_cleanup() {
    for shutdown in [false, true] {
        let f = Fixture::new();
        let provider = Scripted::echo("never");
        let engine = f.start(&provider);
        let shared = engine.handle.shared().clone();
        let cli = fake(&f, "normal");
        seed(&cli, &shared);
        let owner = engine.conn.client;
        let (control, mut reviews) = submit_owner(
            &cli,
            &shared,
            f.dir.path().into(),
            "p",
            CancellationToken::new(),
            owner,
        );
        review(&shared, &mut reviews);
        let publication = shared.publication.lock().unwrap();
        let registry = shared.local_cli_approvals.clone();
        let (done, received) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            if shutdown {
                registry.shutdown();
            } else {
                registry.revoke_owner(owner);
            }
            done.send(()).unwrap();
        });
        let completed = received.recv_timeout(Duration::from_secs(1));
        drop(publication);
        worker.join().unwrap();
        assert!(
            completed.is_ok(),
            "registry retirement cannot synchronously wait for persistence"
        );
        assert_eq!(settled(&shared, "p").state, ExternalRunState::Unknown);
        assert!(shared.local_cli_approvals.install(&control).is_err());
        assert!(!f.project.join("effect").exists());
        no_native(&shared, &provider);
    }
}

#[test]
fn coding_cannot_enter_chat_route_or_use_expired_consent() {
    let f = Fixture::new();
    let provider = Scripted::echo("never");
    let _engine = f.start(&provider);
    let shared = _engine.handle.shared().clone();
    let cli = fake(&f, "normal");
    seed(&cli, &shared);
    let p = prepared(&cli, &shared);
    assert!(
        shared
            .handle
            .block_on(shared.prompt_external_prepared("cli", "p", "synthetic prompt", p))
            .is_err()
    );
    let (control, _receiver) = oneshot::channel();
    let (reviews, _rx) = mpsc::channel(16);
    let admission = Admission {
        owner: 7,
        expires: Instant::now(),
        revoked: CancellationToken::new(),
        parent: f.dir.path().into(),
        feedback: Feedback::Observe { reviews, control },
    };
    let p = prepared(&cli, &shared);
    assert!(
        shared
            .handle
            .block_on(shared.prompt_external_coding(
                "cli",
                "expired",
                "synthetic prompt",
                p,
                admission
            ))
            .is_err()
    );
    assert!(shared.store.external_prompt("cli", "p").unwrap().is_none());
    assert!(
        shared
            .store
            .external_prompt("cli", "expired")
            .unwrap()
            .is_none()
    );
    assert!(!f.project.join("launches").exists());
    no_native(&shared, &provider);
}

#[test]
fn coding_removal_finishes_even_when_a_read_only_control_is_retained() {
    let f = Fixture::new();
    let provider = Scripted::echo("never");
    let _engine = f.start(&provider);
    let shared = _engine.handle.shared().clone();
    let cli = fake(&f, "normal");
    seed(&cli, &shared);
    let (control, mut reviews) = submit(
        &cli,
        &shared,
        f.dir.path().into(),
        "p",
        CancellationToken::new(),
    );
    let r = review(&shared, &mut reviews);
    shared.handle.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), shared.remove_session("cli"))
            .await
            .unwrap()
            .unwrap();
    });
    assert!(shared.store.session("cli").unwrap().is_none());
    assert!(control.decide(7, &r.ticket, Decision::AllowOnce).is_err());
    assert!(!f.project.join("effect").exists());
    assert!(shared.external_tasks.is_empty());
    no_native(&shared, &provider);
}

#[test]
fn coding_setup_loss_never_launches_or_falls_back() {
    for drop_control in [false, true] {
        let f = Fixture::new();
        let provider = Scripted::echo("never");
        let _engine = f.start(&provider);
        let shared = _engine.handle.shared().clone();
        let cli = fake(&f, "normal");
        seed(&cli, &shared);
        let (control, receiver) = oneshot::channel();
        let (reviews, _rx) = mpsc::channel(16);
        let mut receiver = Some(receiver);
        if drop_control {
            drop(receiver.take());
        }
        let admission = Admission {
            owner: 7,
            expires: Instant::now() + Duration::from_secs(30),
            revoked: CancellationToken::new(),
            parent: if drop_control {
                f.dir.path().into()
            } else {
                f.project.clone()
            },
            feedback: Feedback::Observe { reviews, control },
        };
        let prepared = prepared(&cli, &shared);
        shared
            .handle
            .block_on(shared.prompt_external_coding(
                "cli",
                "p",
                "synthetic prompt",
                prepared,
                admission,
            ))
            .unwrap();
        assert_eq!(settled(&shared, "p").state, ExternalRunState::Unknown);
        if let Some(receiver) = receiver {
            assert!(shared.handle.block_on(receiver).is_err());
        }
        assert!(!f.project.join("launches").exists());
        no_native(&shared, &provider);
    }
}

#[test]
fn coding_disconnect_is_unknown_and_shutdown_tracks_pending_cleanup() {
    for shutdown in [false, true] {
        let f = Fixture::new();
        let provider = Scripted::echo("never");
        let engine = f.start(&provider);
        let shared = engine.handle.shared().clone();
        let cli = fake(&f, if shutdown { "normal" } else { "disconnect" });
        seed(&cli, &shared);
        let (control, mut reviews) = submit(
            &cli,
            &shared,
            f.dir.path().into(),
            "p",
            CancellationToken::new(),
        );
        let r = review(&shared, &mut reviews);
        if !shutdown {
            std::fs::write(f.project.join("disconnect-after-review"), b"now").unwrap();
        }
        if shutdown {
            engine.handle.shutdown(Duration::from_secs(3));
            assert!(shared.external_tasks.is_empty());
        }
        assert_eq!(settled(&shared, "p").state, ExternalRunState::Unknown);
        assert!(control.decide(7, &r.ticket, Decision::AllowOnce).is_err());
        assert!(!f.project.join("effect").exists());
        wait(|| shared.external_tasks.is_empty());
        no_native(&shared, &provider);
    }
}

#[test]
fn coding_immediate_disconnect_is_unknown_whether_or_not_review_was_delivered() {
    let f = Fixture::new();
    let provider = Scripted::echo("never");
    let _engine = f.start(&provider);
    let shared = _engine.handle.shared().clone();
    let cli = fake(&f, "disconnect-immediate");
    seed(&cli, &shared);
    let (control, mut reviews) = submit(
        &cli,
        &shared,
        f.dir.path().into(),
        "p",
        CancellationToken::new(),
    );
    assert_eq!(settled(&shared, "p").state, ExternalRunState::Unknown);
    if let Ok(r) = reviews.try_recv() {
        assert!(control.decide(7, &r.ticket, Decision::AllowOnce).is_err());
    }
    assert!(!f.project.join("effect").exists());
    wait(|| shared.external_tasks.is_empty());
    no_native(&shared, &provider);
}

#[test]
fn coding_unknown_action_or_unclean_exit_settles_unknown_not_stuck_running() {
    for behavior in ["unknown-result", "fail-exit"] {
        let f = Fixture::new();
        let provider = Scripted::echo("never");
        let _engine = f.start(&provider);
        let shared = _engine.handle.shared().clone();
        let cli = fake(&f, behavior);
        seed(&cli, &shared);
        let (control, mut reviews) = submit(
            &cli,
            &shared,
            f.dir.path().into(),
            "p",
            CancellationToken::new(),
        );
        let r = review(&shared, &mut reviews);
        control.decide(7, &r.ticket, Decision::AllowOnce).unwrap();
        assert_eq!(settled(&shared, "p").state, ExternalRunState::Unknown);
        assert!(f.project.join("effect").exists()); // Uncertainty does not undo effects.
        assert!(
            !shared
                .store
                .cli_binding("cli")
                .unwrap()
                .unwrap()
                .context_valid
        );
        no_native(&shared, &provider);
    }
}
