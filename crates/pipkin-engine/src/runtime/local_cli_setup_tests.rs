//! Fake metadata executable only; no installed CLI, live auth, inference or git.
#![cfg(unix)]
use super::*;
use crate::{
    provider::scripted::Scripted,
    runtime::tests::{Engine, Fixture},
};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

struct Fake {
    fixture: Fixture,
    executable: PathBuf,
    marker: PathBuf,
}
impl Fake {
    fn new() -> Self {
        let fixture = Fixture::new();
        let executable = fixture.dir.path().join("fake-cli");
        let marker = fixture.dir.path().join("metadata-invocations");
        let help = policy::REQUIRED_FLAGS
            .iter()
            .map(|f| format!("{f} fixture\n"))
            .collect::<String>()
            + "--settings fixture\n";
        let auth = r#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","email":"fake@example.invalid","orgId":"fake"}"#;
        fs::write(&executable, format!("#!/bin/sh\nprintf 'metadata\\n' >> '{}'\ncase \"$1\" in\n--version) printf '%s\\n' '2.1.299 (Claude Code)';;\n--help) printf '%s' '{help}';;\n*) for arg in \"$@\"; do if [ \"$arg\" = '--print' ]; then exit 99; fi; done; printf '%s\\n' '{auth}';;\nesac\n", marker.display())).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            fixture,
            executable,
            marker,
        }
    }
    fn engine(&self) -> Engine {
        let mut config = self.fixture.config();
        config.env.vars.insert(
            "HOME".into(),
            self.fixture.dir.path().to_str().unwrap().into(),
        );
        config
            .env
            .vars
            .insert("PATH".into(), "/usr/bin:/bin".into());
        self.fixture
            .start_config(&Scripted::echo("must not execute"), config)
    }
    fn inspect(&self, shared: &Shared, owner: u64) -> View {
        view(
            shared
                .inspect_local_cli(
                    owner,
                    self.executable.to_str().unwrap(),
                    Target::New {
                        project: self.fixture.project.to_str().unwrap().into(),
                        model: LocalCliModel::Sonnet,
                    },
                )
                .unwrap(),
        )
    }
    fn count(&self) -> usize {
        fs::read_to_string(&self.marker)
            .unwrap_or_default()
            .lines()
            .count()
    }
}
fn view(reply: Reply) -> View {
    let Reply::LocalCliSetup(view) = reply else {
        panic!("setup view expected")
    };
    assert!(!view.execution_enabled);
    view
}
fn check(shared: &Arc<Shared>, owner: u64, setup: &str) -> View {
    view(
        shared
            .handle
            .block_on(shared.check_local_cli(owner, setup, true, true))
            .unwrap(),
    )
}
fn expire(shared: &Shared, setup: &str) {
    shared
        .local_cli_setups
        .state
        .lock()
        .unwrap()
        .tickets
        .get_mut(setup)
        .unwrap()
        .expires = Instant::now() - Duration::from_secs(1);
}

#[test]
fn setup_failures_expose_only_closed_actionable_login_and_version_categories() {
    for (from, to, expected) in [
        (
            "2.1.299 (Claude Code)",
            "2.1.298 (Claude Code)",
            VERSION_FAILURE,
        ),
        ("\"loggedIn\":true", "\"loggedIn\":false", LOGIN_FAILURE),
        (
            "2.1.299 (Claude Code)",
            "PRIVATE_UNTRUSTED_ERROR_BODY",
            VERSION_FAILURE,
        ),
    ] {
        let fake = Fake::new();
        let script = fs::read_to_string(&fake.executable)
            .unwrap()
            .replace(from, to);
        fs::write(&fake.executable, script).unwrap();
        let engine = fake.engine();
        let inspected = fake.inspect(engine.handle.shared(), 1);
        let checked = check(engine.handle.shared(), 1, &inspected.setup);
        assert_eq!(checked.stage, Stage::Failed);
        assert_eq!(checked.reason.as_deref(), Some(expected));
        assert!(
            !checked
                .reason
                .unwrap()
                .contains("PRIVATE_UNTRUSTED_ERROR_BODY")
        );
    }
}

#[test]
#[cfg(target_os = "linux")]
fn public_coding_setup_is_immutable_metadata_and_chat_switch_cannot_enable_it() {
    use pipkin_engine_protocol::Call;
    let fake = Fake::new();
    let script = fs::read_to_string(&fake.executable)
        .unwrap()
        .replace("2.1.299 (Claude Code)", "2.1.301 (Claude Code)");
    fs::write(&fake.executable, script).unwrap();
    let provider = Scripted::echo("must not execute");
    let mut config = fake.fixture.config();
    config.enable_local_claude_cli = true;
    config.env.vars.insert(
        "HOME".into(),
        fake.fixture.dir.path().to_str().unwrap().into(),
    );
    config
        .env
        .vars
        .insert("PATH".into(), "/usr/bin:/bin".into());
    let mut engine = fake.fixture.start_config(&provider, config);
    let inspect = |mode, target| Call::InspectLocalCli {
        mode,
        target,
        executable: fake.executable.to_str().unwrap().into(),
    };
    let target = || Target::New {
        project: fake.fixture.project.to_str().unwrap().into(),
        model: LocalCliModel::Sonnet,
    };
    assert_eq!(
        engine
            .call(inspect(LocalCliMode::Unsupported, target()))
            .unwrap_err()
            .code,
        ErrorCode::Unsupported
    );
    assert_eq!(fake.count(), 0);
    let mut bindings = Vec::new();
    for (id, mode, other) in [
        (
            "coding",
            LocalCliMode::RestrictedWrite,
            LocalCliMode::ChatOnly,
        ),
        (
            "chat",
            LocalCliMode::ChatOnly,
            LocalCliMode::RestrictedWrite,
        ),
    ] {
        let ticket = view(engine.call(inspect(mode, target())).unwrap());
        assert_eq!(ticket.mode, mode);
        let checked = view(
            engine
                .call(Call::CheckLocalCli {
                    setup: ticket.setup.clone(),
                    trust_executable: true,
                    trust_project: true,
                })
                .unwrap(),
        );
        assert_eq!(checked.stage, Stage::MetadataVerified);
        assert_eq!(checked.mode, mode);
        engine
            .call(Call::CreateLocalCliSession {
                session: id.into(),
                setup: ticket.setup.clone(),
            })
            .unwrap();
        let shared = engine.handle.shared().clone();
        let binding = shared.store.cli_binding(id).unwrap().unwrap();
        assert_eq!(binding_mode(&binding.identity.launch_policy), mode);
        assert_eq!(binding.identity.cli_version, "2.1.301");
        let existing = || Target::Existing {
            session: id.into(),
            model: LocalCliModel::Sonnet,
        };
        let before = fake.count();
        assert_eq!(
            engine.call(inspect(other, existing())).unwrap_err().code,
            ErrorCode::Conflict
        );
        assert_eq!(fake.count(), before);
        let reapprove = view(engine.call(inspect(mode, existing())).unwrap());
        let checked = view(
            engine
                .call(Call::CheckLocalCli {
                    setup: reapprove.setup,
                    trust_executable: true,
                    trust_project: true,
                })
                .unwrap(),
        );
        assert_eq!(checked.mode, mode);
        assert_eq!(checked.stage, Stage::MetadataVerified);
        let Reply::LocalCliSession(status) = engine
            .call(Call::LocalCliSessionStatus { session: id.into() })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(status.mode, mode);
        assert_eq!(status.cli_version, "2.1.301");
        assert_eq!(status.summary.local_cli_mode, Some(mode));
        assert_eq!(engine.snapshot(id).summary.local_cli_mode, Some(mode));
        let Reply::Sessions { sessions } = engine.call(Call::ListSessions).unwrap() else {
            panic!()
        };
        assert_eq!(
            sessions.iter().find(|s| s.id == id).unwrap().local_cli_mode,
            Some(mode)
        );
        if mode == LocalCliMode::RestrictedWrite {
            assert!(status.summary.blocked.is_some());
            assert!(shared.local_cli_setups.current_grant(id).is_none());
            let before = fake.count();
            assert!(
                engine
                    .call(Call::Prompt {
                        session: id.into(),
                        request: "never".into(),
                        text: "must not execute".into(),
                        images: vec![],
                        model: None
                    })
                    .is_err()
            );
            assert_eq!(fake.count(), before);
            assert!(shared.store.external_prompt(id, "never").unwrap().is_none());
        } else {
            assert!(status.summary.blocked.is_none());
        }
        // A lost Create ack is a lookup even after the ticket is revoked.
        engine
            .call(Call::RevokeLocalCliSetup {
                setup: ticket.setup.clone(),
            })
            .unwrap();
        engine
            .call(Call::CreateLocalCliSession {
                session: id.into(),
                setup: ticket.setup,
            })
            .unwrap();
        assert_eq!(shared.store.cli_binding(id).unwrap().unwrap(), binding);
        bindings.push(binding);
    }
    assert_ne!(
        bindings[0].identity.cli_session_id,
        bindings[1].identity.cli_session_id
    );
    assert_ne!(
        bindings[0].identity.config_ref,
        bindings[1].identity.config_ref
    );
    assert!(provider.requests().is_empty());
    assert_eq!(fake.count(), 12); // version/help/auth for fresh and existing, two modes
}

#[test]
fn inspection_and_partial_trust_never_run_processes_and_owner_is_enforced() {
    let fake = Fake::new();
    let engine = fake.engine();
    let shared = engine.handle.shared();
    let ticket = fake.inspect(shared, 41);
    assert_eq!(ticket.stage, Stage::AwaitingTrust);
    for (exe, project) in [(false, false), (true, false), (false, true)] {
        assert_eq!(
            shared
                .handle
                .block_on(shared.check_local_cli(41, &ticket.setup, exe, project))
                .unwrap_err()
                .code,
            ErrorCode::Invalid
        );
    }
    assert_eq!(fake.count(), 0);
    assert_eq!(
        shared
            .local_cli_setup_status(42, &ticket.setup)
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        shared
            .revoke_local_cli_setup(42, &ticket.setup)
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        shared
            .handle
            .block_on(shared.check_local_cli(42, &ticket.setup, true, true))
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        shared
            .create_local_cli_session(42, "foreign", &ticket.setup)
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        check(shared, 41, &ticket.setup).stage,
        Stage::MetadataVerified
    );
    assert_eq!(fake.count(), 3);
}

#[test]
fn changed_candidate_is_not_reconstructed_at_check_and_failures_are_sanitized() {
    let fake = Fake::new();
    let engine = fake.engine();
    let shared = engine.handle.shared();
    let ticket = fake.inspect(shared, 1);
    fs::write(&fake.executable, "#!/bin/sh\necho private-raw-stdout\n").unwrap();
    let result = check(shared, 1, &ticket.setup);
    assert_eq!(result.stage, Stage::Failed);
    assert_eq!(result.reason.as_deref(), Some(FAILURE));
    assert_eq!(fake.count(), 0);
    assert!(
        !serde_json::to_string(&result)
            .unwrap()
            .contains("private-raw-stdout")
    );
}

#[test]
fn paths_are_bounded_and_control_characters_rejected_before_inspection() {
    let fake = Fake::new();
    let engine = fake.engine();
    let shared = engine.handle.shared();
    for value in ["x\n".into(), "x\0".into(), "x".repeat(4097)] {
        assert_eq!(
            shared
                .inspect_local_cli(
                    1,
                    &value,
                    Target::New {
                        project: fake.fixture.project.display().to_string(),
                        model: LocalCliModel::Haiku
                    }
                )
                .unwrap_err()
                .code,
            ErrorCode::Invalid
        );
        assert_eq!(
            shared
                .inspect_local_cli(
                    1,
                    fake.executable.to_str().unwrap(),
                    Target::New {
                        project: value,
                        model: LocalCliModel::Haiku
                    }
                )
                .unwrap_err()
                .code,
            ErrorCode::Invalid
        );
    }
    assert_eq!(fake.count(), 0);
}

#[test]
fn ticket_and_process_limits_do_not_queue_and_revoked_workers_keep_registration() {
    let fake = Fake::new();
    let engine = fake.engine();
    let shared = engine.handle.shared();
    let tickets = (0..4).map(|_| fake.inspect(shared, 1)).collect::<Vec<_>>();
    assert_eq!(
        shared
            .inspect_local_cli(
                1,
                fake.executable.to_str().unwrap(),
                tickets[0].target.clone()
            )
            .unwrap_err()
            .code,
        ErrorCode::Busy
    );
    let registry = &shared.local_cli_setups;
    let a = registry.begin(1, &tickets[0].setup, true, true).unwrap();
    let b = registry.begin(1, &tickets[1].setup, true, true).unwrap();
    assert!(matches!(
        registry.begin(1, &tickets[2].setup, true, true),
        Err(ProtoError {
            code: ErrorCode::Busy,
            ..
        })
    ));
    registry.revoke(1, &tickets[0].setup).unwrap();
    assert!(a.cancel.is_cancelled());
    assert!(matches!(
        registry.begin(1, &tickets[2].setup, true, true),
        Err(ProtoError {
            code: ErrorCode::Busy,
            ..
        })
    ));
    assert_eq!(
        shared
            .inspect_local_cli(
                1,
                fake.executable.to_str().unwrap(),
                tickets[0].target.clone()
            )
            .unwrap_err()
            .code,
        ErrorCode::Busy
    );
    drop(a);
    drop(b);
    fake.inspect(shared, 1);
    for owner in 2..=8 {
        for _ in 0..4 {
            fake.inspect(shared, owner);
        }
    }
    assert_eq!(
        shared
            .inspect_local_cli(
                9,
                fake.executable.to_str().unwrap(),
                tickets[0].target.clone()
            )
            .unwrap_err()
            .code,
        ErrorCode::Busy
    );
    registry.revoke_owner(1);
    fake.inspect(shared, 9);
    registry.shutdown();
    assert!(registry.state.lock().unwrap().tickets.is_empty());
    assert_eq!(
        shared
            .inspect_local_cli(
                9,
                fake.executable.to_str().unwrap(),
                tickets[0].target.clone()
            )
            .unwrap_err()
            .code,
        ErrorCode::ShuttingDown
    );
    assert_eq!(fake.count(), 0);
}

#[test]
fn late_success_after_revoke_expiry_disconnect_or_shutdown_cannot_publish_verified() {
    for reason in 0..4 {
        let fake = Fake::new();
        let engine = fake.engine();
        let shared = engine.handle.shared();
        let ticket = fake.inspect(shared, 1);
        let work = shared
            .local_cli_setups
            .begin(1, &ticket.setup, true, true)
            .unwrap();
        let Work {
            request,
            cancel,
            guard,
            _permit,
            ..
        } = work;
        let prepared = shared
            .handle
            .block_on(preflight::check(request, cancel.clone()))
            .unwrap();
        match reason {
            0 => shared.local_cli_setups.revoke(1, &ticket.setup).unwrap(),
            1 => expire(shared, &ticket.setup),
            2 => shared.local_cli_setups.revoke_owner(1),
            _ => shared.local_cli_setups.shutdown(),
        }
        assert!(
            shared
                .local_cli_setups
                .finish(1, &ticket.setup, guard.attempt, &cancel, Ok(prepared))
                .is_err()
        );
        assert!(shared.local_cli_setup_status(1, &ticket.setup).is_err());
        drop(guard);
        drop(_permit);
        assert!(
            shared
                .local_cli_setups
                .state
                .lock()
                .unwrap()
                .pending
                .is_empty()
        );
    }
}

#[test]
fn dropped_rpc_marks_failed_but_keeps_permit_until_worker_cleanup() {
    let fake = Fake::new();
    let engine = fake.engine();
    let shared = engine.handle.shared();
    let ticket = fake.inspect(shared, 1);
    let work = shared
        .local_cli_setups
        .begin(1, &ticket.setup, true, true)
        .unwrap();
    let caller = CheckGuard {
        registry: shared.local_cli_setups.clone(),
        owner: 1,
        setup: ticket.setup.clone(),
        cancel: work.cancel.clone(),
        attempt: work.guard.attempt,
        cleanup: false,
    };
    drop(caller);
    assert!(work.cancel.is_cancelled());
    assert_eq!(
        view(shared.local_cli_setup_status(1, &ticket.setup).unwrap()).stage,
        Stage::Failed
    );
    assert_eq!(shared.local_cli_setups.checks.available_permits(), 1);
    assert!(matches!(
        shared.local_cli_setups.begin(1, &ticket.setup, true, true),
        Err(ProtoError {
            code: ErrorCode::Busy,
            ..
        })
    ));
    drop(work);
    assert_eq!(shared.local_cli_setups.checks.available_permits(), 2);
    assert_eq!(
        check(shared, 1, &ticket.setup).stage,
        Stage::MetadataVerified
    );
}

fn wait(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "metadata cleanup deadline");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn actual_rpc_drop_and_disconnect_cancel_running_metadata_and_reap_before_release() {
    for disconnect in [false, true] {
        let fake = Fake::new();
        fs::write(
            &fake.executable,
            format!(
                "#!/bin/sh\nprintf 'started\\n' >> '{}'\nwhile :; do /bin/sleep 0.05; done\n",
                fake.marker.display(),
            ),
        )
        .unwrap();
        let engine = fake.engine();
        let shared = engine.handle.shared();
        let ticket = fake.inspect(shared, 1);
        let runner = shared.clone();
        let setup = ticket.setup.clone();
        let rpc = shared
            .handle
            .spawn(async move { runner.check_local_cli(1, &setup, true, true).await });
        wait(|| fake.count() > 0);
        assert_eq!(shared.local_cli_setups.checks.available_permits(), 1);
        if disconnect {
            shared.local_cli_setups.revoke_owner(1);
        } else {
            rpc.abort();
        }
        wait(|| shared.local_cli_setups.checks.available_permits() == 2);
        assert!(
            shared
                .local_cli_setups
                .state
                .lock()
                .unwrap()
                .pending
                .is_empty()
        );
        if disconnect {
            assert!(shared.local_cli_setup_status(1, &ticket.setup).is_err());
        } else {
            assert_eq!(
                view(shared.local_cli_setup_status(1, &ticket.setup).unwrap()).stage,
                Stage::Failed
            );
        }
        assert_eq!(fake.count(), 1);
    }
}

#[test]
fn wire_cancel_and_disconnect_revoke_inflight_checks_without_late_verification() {
    use pipkin_engine_protocol::{Call, ClientFrame};
    for disconnect in [false, true] {
        let fake = Fake::new();
        fs::write(
            &fake.executable,
            format!(
                "#!/bin/sh\nprintf 'started\\n' >> '{}'\nwhile :; do /bin/sleep 0.05; done\n",
                fake.marker.display()
            ),
        )
        .unwrap();
        let mut engine = fake.engine();
        let shared = engine.handle.shared().clone();
        let ticket = view(
            engine
                .call(Call::InspectLocalCli {
                    mode: LocalCliMode::ChatOnly,
                    executable: fake.executable.to_str().unwrap().into(),
                    target: Target::New {
                        project: fake.fixture.project.to_str().unwrap().into(),
                        model: LocalCliModel::Sonnet,
                    },
                })
                .unwrap(),
        );
        let call = Call::CheckLocalCli {
            setup: ticket.setup.clone(),
            trust_executable: true,
            trust_project: true,
        };
        engine
            .conn
            .send(ClientFrame {
                id: 100,
                lane: call.lane(),
                call,
            })
            .unwrap();
        wait(|| fake.count() > 0);
        if disconnect {
            let replacement = engine.handle.connect("replacement");
            drop(std::mem::replace(&mut engine.conn, replacement));
            wait(|| {
                !shared
                    .local_cli_setups
                    .state
                    .lock()
                    .unwrap()
                    .tickets
                    .contains_key(&ticket.setup)
            });
        } else {
            assert!(matches!(
                engine.call(Call::CancelRequest { request: 100 }).unwrap(),
                Reply::Ok
            ));
        }
        wait(|| shared.local_cli_setups.checks.available_permits() == 2);
        if !disconnect {
            assert_eq!(
                view(
                    engine
                        .call(Call::LocalCliSetupStatus {
                            setup: ticket.setup
                        })
                        .unwrap()
                )
                .stage,
                Stage::Failed
            );
        }
        assert_eq!(
            fake.count(),
            1,
            "cancellation must prevent help/auth probes too"
        );
    }
}

#[test]
fn engine_shutdown_waits_for_owned_metadata_root_reaping() {
    let fake = Fake::new();
    let pid_file = fake.marker.with_extension("pid");
    fs::write(&fake.executable, format!(
        "#!/bin/sh\ntrap '' TERM\nprintf '%s' \"$$\" > '{}'\nwhile :; do /bin/sleep 0.05; done\n", pid_file.display()
    )).unwrap();
    let engine = fake.engine();
    let shared = engine.handle.shared().clone();
    let ticket = fake.inspect(&shared, 1);
    let runner = shared.clone();
    let _rpc = shared
        .handle
        .spawn(async move { runner.check_local_cli(1, &ticket.setup, true, true).await });
    wait(|| fs::read_to_string(&pid_file).is_ok_and(|text| text.parse::<i32>().is_ok()));
    let pid: i32 = fs::read_to_string(pid_file).unwrap().parse().unwrap();
    assert_eq!(
        unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) },
        0
    );
    engine.handle.shutdown(Duration::from_secs(5));
    assert!(shared.external_tasks.is_empty());
    assert_eq!(shared.local_cli_setups.checks.available_permits(), 2);
    assert_eq!(
        unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) },
        -1
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
}

#[test]
fn create_revalidates_metadata_and_never_changes_the_inspected_cli_uuid() {
    let fake = Fake::new();
    let engine = fake.engine();
    let shared = engine.handle.shared();
    let ticket = fake.inspect(shared, 1);
    let uuid = shared.local_cli_setups.state.lock().unwrap().tickets[&ticket.setup]
        .cli_uuid
        .clone();
    assert!(policy::valid_uuid(&uuid));
    assert_ne!(uuid, ticket.setup);
    check(shared, 1, &ticket.setup);
    check(shared, 1, &ticket.setup);
    assert_eq!(
        shared.local_cli_setups.state.lock().unwrap().tickets[&ticket.setup].cli_uuid,
        uuid
    );
    fs::write(&fake.executable, "#!/bin/sh\nexit 0\n").unwrap();
    assert!(
        shared
            .create_local_cli_session(1, "cli", &ticket.setup)
            .is_err()
    );
    assert!(shared.store.session("cli").unwrap().is_none());
    assert_eq!(fake.count(), 6);
}

#[test]
fn durable_create_retries_after_expiry_and_restart_without_probes_or_activation() {
    let fake = Fake::new();
    let mut engine = fake.engine();
    let setup;
    {
        let shared = engine.handle.shared();
        let ticket = fake.inspect(shared, 1);
        setup = ticket.setup;
        assert_eq!(check(shared, 1, &setup).stage, Stage::MetadataVerified);
        let first = shared.create_local_cli_session(1, "cli", &setup).unwrap();
        let row = shared.store.session("cli").unwrap().unwrap();
        assert_eq!(row.model.as_deref(), Some("sonnet"));
        assert_eq!(
            row.execution_backend,
            pipkin_engine_protocol::ExecutionBackend::LocalClaudeCli
        );
        expire(shared, &setup);
        assert_eq!(
            shared.create_local_cli_session(1, "cli", &setup).unwrap(),
            first
        );
        assert_eq!(fake.count(), 3);
        assert!(shared.create_local_cli_session(1, "other", &setup).is_err());
    }
    use pipkin_engine_protocol::Call;
    assert_eq!(
        engine
            .call(Call::Prompt {
                session: "cli".into(),
                request: "never".into(),
                text: "never".into(),
                images: vec![],
                model: None
            })
            .unwrap_err()
            .code,
        ErrorCode::Unsupported
    );
    assert_eq!(
        engine
            .call(Call::Stop {
                session: "cli".into()
            })
            .unwrap_err()
            .code,
        ErrorCode::Unsupported
    );
    assert!(matches!(
        engine
            .call(Call::HistoryPage {
                session: "cli".into(),
                before: None,
                limit: 20
            })
            .unwrap(),
        Reply::Page { .. }
    ));
    drop(engine);
    let engine = fake.engine();
    fs::remove_file(&fake.executable).unwrap();
    let shared = engine.handle.shared();
    assert!(
        shared
            .local_cli_setups
            .state
            .lock()
            .unwrap()
            .tickets
            .is_empty()
    );
    assert!(matches!(
        shared.create_local_cli_session(22, "cli", &setup).unwrap(),
        Reply::Session { .. }
    ));
    assert_eq!(fake.count(), 3);
}

#[test]
fn existing_metadata_requires_exact_saved_identity_and_model_without_rebinding() {
    let fake = Fake::new();
    let engine = fake.engine();
    let shared = engine.handle.shared();
    let ticket = fake.inspect(shared, 1);
    check(shared, 1, &ticket.setup);
    shared
        .create_local_cli_session(1, "cli", &ticket.setup)
        .unwrap();
    let saved = shared.store.cli_binding("cli").unwrap().unwrap();
    for (alias, stage) in [
        (LocalCliModel::Sonnet, Stage::MetadataVerified),
        (LocalCliModel::Opus, Stage::Failed),
    ] {
        let ticket = view(
            shared
                .inspect_local_cli(
                    2,
                    fake.executable.to_str().unwrap(),
                    Target::Existing {
                        session: "cli".into(),
                        model: alias,
                    },
                )
                .unwrap(),
        );
        assert_eq!(check(shared, 2, &ticket.setup).stage, stage);
        assert!(
            shared
                .create_local_cli_session(2, "other", &ticket.setup)
                .is_err()
        );
        assert_eq!(shared.store.cli_binding("cli").unwrap().unwrap(), saved);
    }
    // Invalid durable context cannot be repaired or rebound by metadata setup.
    let key = crate::store::ExternalRunKey {
        session: "cli".into(),
        generation: saved.generation,
        op: "synthetic-interruption".into(),
    };
    shared.store.admit_external_run(&key, 1).unwrap();
    shared.store.recover_external_runs(2).unwrap();
    assert_eq!(
        shared
            .inspect_local_cli(
                3,
                fake.executable.to_str().unwrap(),
                Target::Existing {
                    session: "cli".into(),
                    model: LocalCliModel::Sonnet,
                }
            )
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert!(
        !shared
            .store
            .cli_binding("cli")
            .unwrap()
            .unwrap()
            .context_valid
    );
    // Lost creation replies resolve without reactivating the invalid context.
    shared
        .create_local_cli_session(1, "cli", &ticket.setup)
        .unwrap();
    assert!(
        !shared
            .store
            .cli_binding("cli")
            .unwrap()
            .unwrap()
            .context_valid
    );
    // The runtime has no native-history projection or external launch authority here.
    let source = include_str!("local_cli_setup.rs");
    for forbidden in [
        "prompt_external",
        "launch_spec(",
        "admit_external",
        "append_entry",
        "set_context_valid",
    ] {
        assert!(!source.contains(forbidden));
    }
}
