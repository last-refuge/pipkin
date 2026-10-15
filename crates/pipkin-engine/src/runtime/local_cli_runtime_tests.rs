//! Public RPC fixtures: fake executable and fake metadata/model output only.
#![cfg(unix)]
use super::*;
use crate::{
    provider::scripted::Scripted,
    runtime::tests::{Engine, Fixture},
    store::ExternalRunState,
};
use pipkin_engine_protocol::*;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    time::{Duration, Instant},
};

struct Fake {
    fixture: Fixture,
}
impl Fake {
    fn new() -> Self {
        let fixture = Fixture::new();
        let help = policy::REQUIRED_FLAGS
            .iter()
            .map(|f| format!("{f} fixture\n"))
            .collect::<String>();
        let script = format!(
            r##"#!/bin/sh
cd '{project}' || exit 1
metadata=yes
for arg in "$@"; do if [ "$arg" = '--print' ]; then metadata=no; fi; done
if [ "$metadata" = yes ]; then
 printf 'probe\n' >> probes
 while [ -f gate-probe ]; do printf 'waiting\n' > probing; sleep 0.02; done
 case "$1" in
 --version) printf '%s\n' '2.1.299 (Claude Code)';;
 --help) printf '%s' '{help}';;
 *) printf '%s\n' '{{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","email":"fake@example.invalid","orgId":"fixture"}}';;
 esac
 exit 0
fi
previous=''
for arg in "$@"; do
 if [ "$previous" = '--session-id' ] || [ "$previous" = '--resume' ]; then uuid="$arg"; fi
 previous="$arg"
done
printf '%s\n' "$*" >> launches
cat > /dev/null
while [ -f gate-turn ]; do sleep 0.02; done
cat <<EOF
{{"type":"system","subtype":"init","session_id":"$uuid","tools":[]}}
{{"type":"assistant","session_id":"$uuid","message":{{"id":"answer","role":"assistant","content":[{{"type":"text","text":"Synthetic answer"}}]}}}}
{{"type":"result","session_id":"$uuid","subtype":"success","is_error":false,"result":"Synthetic answer"}}
EOF
"##,
            project = fixture.project.display(),
        );
        fs::write(fixture.dir.path().join("fake-cli"), script).unwrap();
        fs::set_permissions(
            fixture.dir.path().join("fake-cli"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        Self { fixture }
    }
    fn engine(&self, provider: &Scripted, enabled: bool) -> Engine {
        let mut config = self.fixture.config();
        config.enable_local_claude_cli = enabled;
        config
            .env
            .vars
            .insert("HOME".into(), self.fixture.dir.path().display().to_string());
        config
            .env
            .vars
            .insert("PATH".into(), "/usr/bin:/bin".into());
        self.fixture.start_config(provider, config)
    }
    fn inspect(&self, engine: &mut Engine, existing: bool) -> String {
        let target = if existing {
            LocalCliSetupTarget::Existing {
                session: "cli".into(),
                model: LocalCliModel::Sonnet,
            }
        } else {
            LocalCliSetupTarget::New {
                project: self.fixture.project.display().to_string(),
                model: LocalCliModel::Sonnet,
            }
        };
        let Reply::LocalCliSetup(view) = engine
            .call(Call::InspectLocalCli {
                mode: pipkin_engine_protocol::LocalCliMode::ChatOnly,
                executable: self
                    .fixture
                    .dir
                    .path()
                    .join("fake-cli")
                    .display()
                    .to_string(),
                target,
            })
            .unwrap()
        else {
            panic!("setup")
        };
        view.setup
    }
    fn approve(&self, engine: &mut Engine, existing: bool) -> String {
        let setup = self.inspect(engine, existing);
        let Reply::LocalCliSetup(view) = engine
            .call(Call::CheckLocalCli {
                setup: setup.clone(),
                trust_executable: true,
                trust_project: true,
            })
            .unwrap()
        else {
            panic!("checked")
        };
        assert_eq!(view.stage, LocalCliSetupStage::MetadataVerified);
        if !existing {
            engine
                .call(Call::CreateLocalCliSession {
                    session: "cli".into(),
                    setup: setup.clone(),
                })
                .unwrap();
        }
        setup
    }
    fn count(&self, name: &str) -> usize {
        fs::read_to_string(self.fixture.project.join(name))
            .unwrap_or_default()
            .lines()
            .count()
    }
    fn gate(&self, name: &str) {
        fs::write(self.fixture.project.join(name), "").unwrap();
    }
    fn release(&self, name: &str) {
        fs::remove_file(self.fixture.project.join(name)).unwrap();
    }
}
fn wait(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "fixture deadline");
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn completed(shared: &Shared, request: &str) {
    wait(|| {
        shared
            .store
            .external_prompt("cli", request)
            .unwrap()
            .is_some_and(|r| r.outcome.is_some())
            && shared.session("cli").unwrap().active_run().is_none()
    });
    assert_eq!(
        shared
            .store
            .external_prompt("cli", request)
            .unwrap()
            .unwrap()
            .state,
        ExternalRunState::Completed
    );
}
fn send_prompt(engine: &mut Engine, key: &str) {
    engine.next += 1;
    let call = Call::Prompt {
        session: "cli".into(),
        request: key.into(),
        text: "fixture".into(),
        images: vec![],
        model: None,
    };
    engine
        .conn
        .send(ClientFrame {
            id: engine.next,
            lane: call.lane(),
            call,
        })
        .unwrap();
}

fn snapshot(engine: &mut Engine) -> SessionSnapshot {
    let Reply::Snapshot(snapshot) = engine
        .call(Call::OpenSession {
            session: "cli".into(),
            window: None,
        })
        .unwrap()
    else {
        panic!("snapshot")
    };
    *snapshot
}

#[test]
fn grant_installation_and_renewal_advance_snapshot_revision_once() {
    for enabled in [false, true] {
        let fake = Fake::new();
        let provider = Scripted::echo("never called");
        let mut engine = fake.engine(&provider, enabled);
        let setup = fake.approve(&mut engine, false);
        let fresh = snapshot(&mut engine);
        assert_eq!(fresh.revision, u64::from(enabled));
        assert_eq!(fresh.summary.blocked.is_none(), enabled);
        engine.call(Call::RevokeLocalCliSetup { setup }).unwrap();
        let blocked = snapshot(&mut engine);
        assert!(blocked.summary.blocked.is_some());
        assert_eq!(blocked.revision, fresh.revision);

        let setup = fake.approve(&mut engine, true);
        let regranted = snapshot(&mut engine);
        assert_eq!(regranted.summary.blocked.is_none(), enabled);
        assert_eq!(regranted.revision, blocked.revision + u64::from(enabled));
        if enabled {
            assert!(regranted.revision > blocked.revision);
        }
        // Rechecking the same ticket renews trust too. No wall-clock sleep:
        // readiness ordering depends on revision, not millisecond timestamps.
        engine
            .call(Call::CheckLocalCli {
                setup,
                trust_executable: true,
                trust_project: true,
            })
            .unwrap();
        assert_eq!(
            snapshot(&mut engine).revision,
            regranted.revision + u64::from(enabled)
        );
        assert_eq!(fake.count("launches"), 0);
        assert!(provider.requests().is_empty());
        if enabled {
            assert!(
                engine
                    .events
                    .iter()
                    .any(|event| matches!(event, Event::SessionsChanged))
            );
            assert!(
                !engine
                    .events
                    .iter()
                    .any(|event| matches!(event, Event::Session { .. }))
            );
        }
    }
}

#[test]
fn readiness_lookups_and_duplicate_creation_never_grant_or_bump() {
    let fake = Fake::new();
    let provider = Scripted::echo("never called");
    let mut engine = fake.engine(&provider, true);
    let setup = fake.approve(&mut engine, false);
    for revoked in [false, true] {
        if revoked {
            engine
                .call(Call::RevokeLocalCliSetup {
                    setup: setup.clone(),
                })
                .unwrap();
        }
        let before = snapshot(&mut engine);
        let probes = fake.count("probes");
        for call in [
            Call::ListSessions,
            Call::LocalCliSessionStatus {
                session: "cli".into(),
            },
            Call::LookupLocalCliCreation {
                session: "cli".into(),
                setup: setup.clone(),
            },
            Call::CreateLocalCliSession {
                session: "cli".into(),
                setup: setup.clone(),
            },
        ] {
            engine.call(call).unwrap();
        }
        let after = snapshot(&mut engine);
        assert_eq!(after.revision, before.revision);
        assert_eq!(after.summary.blocked.is_some(), revoked);
        assert_eq!(fake.count("probes"), probes);
    }
    assert_eq!(fake.count("launches"), 0);
    assert!(provider.requests().is_empty());
}

#[test]
fn failed_readiness_commit_revokes_new_grant_and_does_not_report_verified() {
    let fake = Fake::new();
    let provider = Scripted::echo("never called");
    let mut engine = fake.engine(&provider, true);
    fake.approve(&mut engine, false);
    let before = snapshot(&mut engine);
    let setup = fake.inspect(&mut engine, true);
    let shared = engine.handle.shared().clone();
    shared.store.fail_next_write();
    assert!(
        engine
            .call(Call::CheckLocalCli {
                setup: setup.clone(),
                trust_executable: true,
                trust_project: true,
            })
            .is_err()
    );
    assert!(shared.local_cli_setups.current_grant("cli").is_none());
    assert_eq!(
        shared.store.session("cli").unwrap().unwrap().revision,
        before.revision
    );
    let Reply::LocalCliSetup(view) = engine.call(Call::LocalCliSetupStatus { setup }).unwrap()
    else {
        panic!("setup status")
    };
    assert_eq!(view.stage, LocalCliSetupStage::Failed);
    assert!(view.reason.is_some());
    assert_eq!(fake.count("launches"), 0);
    assert!(provider.requests().is_empty());
}

#[test]
fn fresh_creation_with_failed_grant_commit_stays_blocked_on_duplicate() {
    let fake = Fake::new();
    let provider = Scripted::echo("never called");
    let mut engine = fake.engine(&provider, true);
    let setup = fake.inspect(&mut engine, false);
    engine
        .call(Call::CheckLocalCli {
            setup: setup.clone(),
            trust_executable: true,
            trust_project: true,
        })
        .unwrap();
    let shared = engine.handle.shared().clone();
    // Allow durable creation, but reject its subsequent readiness revision bump.
    shared
        .store
        .tx(|tx| {
            tx.execute_batch(
                "CREATE TEMP TRIGGER fail_readiness BEFORE UPDATE OF revision ON sessions
            BEGIN SELECT RAISE(ABORT, 'fixture readiness commit failure'); END;",
            )?;
            Ok(())
        })
        .unwrap();
    assert!(
        engine
            .call(Call::CreateLocalCliSession {
                session: "cli".into(),
                setup: setup.clone(),
            })
            .is_err()
    );
    assert!(shared.local_cli_setups.current_grant("cli").is_none());
    assert_eq!(shared.store.session("cli").unwrap().unwrap().revision, 0);
    let Reply::LocalCliSetup(view) = engine
        .call(Call::LocalCliSetupStatus {
            setup: setup.clone(),
        })
        .unwrap()
    else {
        panic!("setup status")
    };
    assert_eq!(view.stage, LocalCliSetupStage::Failed);
    let Reply::Session { summary } = engine
        .call(Call::CreateLocalCliSession {
            session: "cli".into(),
            setup,
        })
        .unwrap()
    else {
        panic!("durable creation receipt")
    };
    assert!(summary.blocked.is_some());
    assert!(shared.local_cli_setups.current_grant("cli").is_none());
    assert_eq!(shared.store.session("cli").unwrap().unwrap().revision, 0);
    assert_eq!(fake.count("probes"), 3);
    assert_eq!(fake.count("launches"), 0);
    assert!(provider.requests().is_empty());
}

#[test]
fn public_fresh_resume_and_lost_ack_survive_revocation_and_restart_without_probes() {
    let fake = Fake::new();
    let provider = Scripted::echo("never called");
    let mut engine = fake.engine(&provider, true);
    let setup = fake.approve(&mut engine, false);
    let shared = engine.handle.shared().clone();
    assert!(
        shared
            .execution_summary(&shared.store.session("cli").unwrap().unwrap())
            .blocked
            .is_none()
    );
    let first = engine.prompt("cli", "one", "fixture").unwrap();
    completed(&shared, "one");
    assert_eq!(fake.count("probes"), 6);
    engine.prompt("cli", "two", "fixture").unwrap();
    completed(&shared, "two");
    assert_eq!(fake.count("probes"), 9);
    let launches = fs::read_to_string(fake.fixture.project.join("launches")).unwrap();
    assert!(launches.lines().next().unwrap().contains("--session-id"));
    assert!(launches.lines().nth(1).unwrap().contains("--resume"));
    engine.call(Call::RevokeLocalCliSetup { setup }).unwrap();
    let probes = fake.count("probes");
    assert_eq!(
        serde_json::to_value(engine.prompt("cli", "one", "fixture").unwrap()).unwrap(),
        serde_json::to_value(&first).unwrap()
    );
    assert_eq!(
        engine.prompt("cli", "one", "changed").unwrap_err().code,
        ErrorCode::Conflict
    );
    assert_eq!(
        engine.prompt("cli", "three", "fixture").unwrap_err().code,
        ErrorCode::Unsupported
    );
    assert_eq!(fake.count("probes"), probes);
    assert!(provider.requests().is_empty());
    drop(shared);
    drop(engine);
    // Even disabling new execution on restart cannot hide the old receipt.
    let mut engine = fake.engine(&provider, false);
    assert_eq!(
        serde_json::to_value(engine.prompt("cli", "one", "fixture").unwrap()).unwrap(),
        serde_json::to_value(first).unwrap()
    );
    assert_eq!(
        engine.prompt("cli", "new", "fixture").unwrap_err().code,
        ErrorCode::Unsupported
    );
    assert_eq!(fake.count("probes"), probes);
    drop(engine);
    let mut engine = fake.engine(&provider, true);
    fake.approve(&mut engine, true);
    engine.prompt("cli", "after-reapproval", "fixture").unwrap();
    completed(engine.handle.shared(), "after-reapproval");
    assert_eq!(fake.count("launches"), 3);
    assert!(provider.requests().is_empty());
}

#[test]
fn two_connections_cannot_share_launch_authority_but_can_lookup_receipts() {
    let fake = Fake::new();
    let provider = Scripted::echo("never called");
    let mut engine = fake.engine(&provider, true);
    let setup = fake.approve(&mut engine, false);
    let shared = engine.handle.shared().clone();
    let second = engine.handle.connect("unapproved");
    assert_ne!(second.client_id(), engine.conn.client_id());
    // Keep both actual RPC connections alive: swapping only changes which one
    // the test's call helper uses, not the grant's connection owner.
    let mut other = std::mem::replace(&mut engine.conn, second);
    assert!(matches!(
        engine
            .call(Call::Hello {
                protocol: PROTOCOL_VERSION,
                client: "unapproved".into(),
            })
            .unwrap(),
        Reply::Welcome(_)
    ));
    for call in [
        Call::ListSessions,
        Call::OpenSession {
            session: "cli".into(),
            window: None,
        },
        Call::LocalCliSessionStatus {
            session: "cli".into(),
        },
        Call::LookupLocalCliCreation {
            session: "cli".into(),
            setup: setup.clone(),
        },
        Call::CreateLocalCliSession {
            session: "cli".into(),
            setup: setup.clone(),
        },
    ] {
        let summary = match engine.call(call).unwrap() {
            Reply::Sessions { mut sessions } => sessions.remove(0),
            Reply::Snapshot(snapshot) => snapshot.summary,
            Reply::LocalCliSession(view) => view.summary,
            Reply::LocalCliCreation {
                summary: Some(summary),
            }
            | Reply::Session { summary } => summary,
            _ => panic!("expected read-only session summary"),
        };
        assert!(
            summary.blocked.is_some(),
            "foreign approval must not advertise readiness"
        );
    }
    let probes = fake.count("probes");
    assert_eq!(probes, 3);
    assert_eq!(
        engine
            .prompt("cli", "unauthorized", "fixture")
            .unwrap_err()
            .code,
        ErrorCode::Unsupported
    );
    assert_eq!(fake.count("probes"), probes);
    assert_eq!(fake.count("launches"), 0);
    assert!(
        shared
            .store
            .external_prompt("cli", "unauthorized")
            .unwrap()
            .is_none()
    );
    wait(|| shared.external_tasks.is_empty());

    // Rejection must neither revoke the real owner's grant nor reserve a turn.
    std::mem::swap(&mut engine.conn, &mut other);
    let Reply::Snapshot(snapshot) = engine
        .call(Call::OpenSession {
            session: "cli".into(),
            window: None,
        })
        .unwrap()
    else {
        panic!()
    };
    assert!(snapshot.summary.blocked.is_none());
    let receipt = engine.prompt("cli", "owned", "fixture").unwrap();
    completed(&shared, "owned");
    let probes = fake.count("probes");
    assert_eq!(probes, 6);
    assert_eq!(fake.count("launches"), 1);
    std::mem::swap(&mut engine.conn, &mut other);
    assert_eq!(
        serde_json::to_value(engine.prompt("cli", "owned", "fixture").unwrap()).unwrap(),
        serde_json::to_value(receipt).unwrap()
    );
    assert_eq!(
        engine.prompt("cli", "owned", "changed").unwrap_err().code,
        ErrorCode::Conflict
    );
    assert_eq!(
        engine
            .prompt("cli", "still-unauthorized", "fixture")
            .unwrap_err()
            .code,
        ErrorCode::Unsupported
    );
    assert_eq!(fake.count("probes"), probes);
    assert_eq!(fake.count("launches"), 1);
    assert!(
        shared
            .store
            .external_prompt("cli", "still-unauthorized")
            .unwrap()
            .is_none()
    );
    assert!(provider.requests().is_empty());
}

#[test]
fn default_flag_and_unsupported_payloads_never_probe_or_call_native() {
    let fake = Fake::new();
    let provider = Scripted::echo("never called");
    let mut engine = fake.engine(&provider, false);
    fake.approve(&mut engine, false);
    assert_eq!(
        engine.prompt("cli", "off", "fixture").unwrap_err().code,
        ErrorCode::Unsupported
    );
    assert_eq!(fake.count("probes"), 3);
    drop(engine);
    let mut engine = fake.engine(&provider, true);
    fake.approve(&mut engine, true);
    let probes = fake.count("probes");
    for (images, model) in [
        (vec![], Some("sonnet".into())),
        (
            vec![ImageInput {
                mime_type: "image/png".into(),
                data: "fixture".into(),
            }],
            None,
        ),
    ] {
        assert_eq!(
            engine
                .call(Call::Prompt {
                    session: "cli".into(),
                    request: "reject".into(),
                    text: "fixture".into(),
                    images,
                    model
                })
                .unwrap_err()
                .code,
            ErrorCode::Unsupported
        );
    }
    for call in [
        Call::SetModel {
            session: "cli".into(),
            model: "scripted/scripted".into(),
        },
        Call::SetGoal {
            session: "cli".into(),
            objective: "no automation".into(),
        },
    ] {
        assert_eq!(engine.call(call).unwrap_err().code, ErrorCode::Unsupported);
    }
    assert_eq!(fake.count("probes"), probes);
    assert_eq!(fake.count("launches"), 0);
    assert!(provider.requests().is_empty());
}

#[test]
fn stop_and_revoke_during_owned_preflight_cannot_admit_or_launch_later() {
    for revoke in [false, true] {
        let fake = Fake::new();
        let provider = Scripted::echo("never called");
        let mut engine = fake.engine(&provider, true);
        let setup = fake.approve(&mut engine, false);
        fake.gate("gate-probe");
        send_prompt(&mut engine, "pending");
        wait(|| fake.count("probing") > 0);
        assert!(!engine.handle.shared().external_tasks.is_empty());
        if revoke {
            engine.call(Call::RevokeLocalCliSetup { setup }).unwrap();
        } else {
            assert!(matches!(
                engine
                    .call(Call::Stop {
                        session: "cli".into()
                    })
                    .unwrap(),
                Reply::Stopping { run: None }
            ));
        }
        wait(|| engine.handle.shared().external_tasks.is_empty());
        fake.release("gate-probe");
        assert!(
            engine
                .handle
                .shared()
                .store
                .external_prompt("cli", "pending")
                .unwrap()
                .is_none()
        );
        assert_eq!(fake.count("launches"), 0);
        assert!(provider.requests().is_empty());
    }
}

#[test]
fn changed_captured_candidate_rejects_without_reinspection_or_native_fallback() {
    let fake = Fake::new();
    let provider = Scripted::echo("never called");
    let mut engine = fake.engine(&provider, true);
    fake.approve(&mut engine, false);
    fs::set_permissions(
        fake.fixture.dir.path().join("fake-cli"),
        fs::Permissions::from_mode(0o500),
    )
    .unwrap();
    assert!(engine.prompt("cli", "stale", "fixture").is_err());
    assert_eq!(fake.count("probes"), 3);
    assert_eq!(fake.count("launches"), 0);
    assert!(provider.requests().is_empty());
}

#[test]
fn owned_active_stop_ignores_readiness_loss_and_revoke_persists_stop() {
    for revoke in [false, true] {
        let fake = Fake::new();
        let provider = Scripted::echo("never called");
        let mut engine = fake.engine(&provider, true);
        let setup = fake.approve(&mut engine, false);
        fake.gate("gate-turn");
        engine.prompt("cli", "active", "fixture").unwrap();
        wait(|| fake.count("launches") == 1);
        if revoke {
            engine.call(Call::RevokeLocalCliSetup { setup }).unwrap();
        } else {
            engine
                .handle
                .shared()
                .store
                .tx(|tx| {
                    // Model identity is immutable. Simulate loss of readiness, not an
                    // impossible rebinding that the database correctly rejects.
                    tx.execute(
                        "UPDATE sessions SET blocked='fixture readiness lost' WHERE id='cli'",
                        [],
                    )?;
                    Ok(())
                })
                .unwrap();
            assert!(matches!(
                engine
                    .call(Call::Stop {
                        session: "cli".into()
                    })
                    .unwrap(),
                Reply::Stopping { run: Some(_) }
            ));
        }
        wait(|| engine.handle.shared().external_tasks.is_empty());
        let receipt = engine
            .handle
            .shared()
            .store
            .external_prompt("cli", "active")
            .unwrap()
            .unwrap();
        assert!(receipt.outcome.is_some());
        assert_ne!(receipt.state, ExternalRunState::Completed);
        assert!(provider.requests().is_empty());
    }
}

// Also exercise the helper independently of the parent-owned RPC wiring.
#[test]
fn dispatcher_helper_admits_fresh_and_resume_but_replays_revoked_receipts_without_probes() {
    let fake = Fake::new();
    let provider = Scripted::echo("never called");
    let mut engine = fake.engine(&provider, true);
    let setup = fake.approve(&mut engine, false);
    let shared = engine.handle.shared().clone();
    let owner = engine.conn.client_id();
    for key in ["one", "two"] {
        shared
            .handle
            .block_on(shared.prompt_by_backend(owner, "cli", key, "fixture", &[], None))
            .unwrap();
        completed(&shared, key);
    }
    engine.call(Call::RevokeLocalCliSetup { setup }).unwrap();
    let _capacity = shared
        .generations
        .clone()
        .try_acquire_many_owned(shared.generations.available_permits() as u32)
        .unwrap();
    let probes = fake.count("probes");
    assert!(matches!(
        shared
            .handle
            .block_on(shared.prompt_by_backend(owner, "cli", "one", "fixture", &[], None))
            .unwrap(),
        Reply::Admitted { .. }
    ));
    assert_eq!(
        shared
            .handle
            .block_on(shared.prompt_by_backend(owner, "cli", "new", "fixture", &[], None))
            .unwrap_err()
            .code,
        ErrorCode::Unsupported
    );
    assert_eq!(fake.count("probes"), probes);
    assert_eq!(fake.count("launches"), 2);
    assert!(provider.requests().is_empty());
}

#[test]
fn dispatcher_helper_stop_serializes_with_pending_admission_and_tracks_cleanup() {
    let fake = Fake::new();
    let provider = Scripted::echo("never called");
    let mut engine = fake.engine(&provider, true);
    fake.approve(&mut engine, false);
    fake.gate("gate-probe");
    let shared = engine.handle.shared().clone();
    let prompt_shared = shared.clone();
    let owner = engine.conn.client_id();
    let prompt = shared.handle.spawn(async move {
        prompt_shared
            .prompt_by_backend(owner, "cli", "pending", "fixture", &[], None)
            .await
    });
    wait(|| fake.count("probing") > 0);
    shared
        .handle
        .block_on(shared.stop_by_backend("cli"))
        .unwrap();
    assert!(shared.handle.block_on(prompt).unwrap().is_err());
    wait(|| shared.external_tasks.is_empty());
    fake.release("gate-probe");
    assert!(
        shared
            .store
            .external_prompt("cli", "pending")
            .unwrap()
            .is_none()
    );
    assert_eq!(fake.count("launches"), 0);
}

#[test]
fn shutdown_waits_for_owned_preflight_cleanup() {
    let fake = Fake::new();
    let provider = Scripted::echo("never called");
    let mut engine = fake.engine(&provider, true);
    fake.approve(&mut engine, false);
    fake.gate("gate-probe");
    send_prompt(&mut engine, "shutdown");
    wait(|| fake.count("probing") > 0);
    let shared = engine.handle.shared().clone();
    shared.stopping.cancel();
    shared.local_cli_setups.shutdown();
    shared.handle.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), shared.external_shutdown().wait())
            .await
            .unwrap();
    });
    assert!(shared.external_tasks.is_empty());
    assert!(
        shared
            .store
            .external_prompt("cli", "shutdown")
            .unwrap()
            .is_none()
    );
    assert_eq!(fake.count("launches"), 0);
}
