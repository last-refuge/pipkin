//! Synthetic fake CLI only: real preflight, binding, supervisor, receipts and publication.
#![cfg(unix)]

use std::{
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use pipkin_engine_protocol::*;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::{Shared, external::LaunchTest, tests::Fixture};
use crate::{
    execution::claude_cli::{events, policy, preflight, process},
    provider::scripted::Scripted,
    store::{
        ExternalPromptMode, ExternalPromptReceipt, ExternalRunState, ExternalSettlementEvidence,
    },
};

const UUID: &str = "11111111-1111-4111-8111-111111111111";
#[cfg(target_os = "linux")]
#[path = "external_coding_tests.rs"]
mod coding;

struct FakeCli {
    executable: PathBuf,
    environment: policy::Environment,
    project: PathBuf,
}
impl FakeCli {
    fn new(f: &Fixture, tail: &str, terminal: bool) -> Self {
        let executable = f.dir.path().join("fake-cli");
        let help = policy::REQUIRED_FLAGS
            .iter()
            .map(|flag| format!("{flag} fixture\n"))
            .collect::<String>();
        let records = [
            json!({"type":"system","subtype":"init","session_id":UUID,"tools":[]}),
            json!({"type":"stream_event","session_id":UUID,"event":{"type":"message_start","message":{"id":"answer","role":"assistant","content":[]}}}),
            json!({"type":"stream_event","session_id":UUID,"event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}}),
            json!({"type":"stream_event","session_id":UUID,"event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Synthetic answer"}}}),
            json!({"type":"stream_event","session_id":UUID,"event":{"type":"content_block_stop","index":0}}),
            json!({"type":"stream_event","session_id":UUID,"event":{"type":"message_stop"}}),
            json!({"type":"assistant","session_id":UUID,"message":{"id":"answer","role":"assistant","content":[{"type":"text","text":"Synthetic answer"}]}}),
        ];
        let mut wire = records.iter().map(|r| format!("{r}\n")).collect::<String>();
        if terminal {
            wire.push_str(&format!("{}\n", json!({"type":"result","session_id":UUID,"subtype":"success","is_error":false,"result":"NOT the projected answer"})));
        }
        let auth = r#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","email":"fixture@example.invalid","orgId":"fixture-org"}"#;
        let script = format!(
            "#!/bin/sh\ncase \"$1\" in\n--version) printf '%s\\n' '2.1.299 (Claude Code)'; exit ;;\n--help) printf '%s' '{help}'; exit ;;\nesac\nfor arg in \"$@\"; do\n if [ \"$arg\" = '--print' ]; then\n  printf '%s\\n' \"$*\" >> invocations\n  printf '%s\\n' \"$$\" > root-pid\n  cat > received-prompt\n  printf '%s' '{wire}'\n  {tail}\n  exit 0\n fi\ndone\nprintf '%s\\n' '{auth}'\n"
        );
        std::fs::write(&executable, script).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            executable,
            environment: [
                ("HOME".into(), f.dir.path().as_os_str().to_owned()),
                ("PATH".into(), "/usr/bin:/bin".into()),
            ]
            .into_iter()
            .collect(),
            project: f.project.clone(),
        }
    }
    fn prepared(&self, shared: &Shared) -> preflight::Prepared {
        shared
            .handle
            .block_on(preflight::check(
                preflight::Request {
                    candidate: preflight::Candidate::inspect(
                        &self.executable,
                        &self.project,
                        &self.environment,
                    )
                    .unwrap(),
                    trust_executable: true,
                    trust_project: true,
                    environment: self.environment.clone(),
                    model: policy::ModelAlias::Sonnet,
                },
                CancellationToken::new(),
            ))
            .unwrap()
    }
    fn seed(&self, shared: &Shared) {
        let prepared = self.prepared(shared);
        shared
            .store
            .create_cli_session("cli", "create-cli", &prepared.binding(UUID).unwrap(), 1)
            .unwrap();
    }
    fn submit(&self, shared: &Arc<Shared>, request: &str, text: &str) -> Result<Reply, ProtoError> {
        let prepared = self.prepared(shared);
        shared
            .handle
            .block_on(shared.prompt_external_prepared("cli", request, text, prepared))
    }
    fn launches(&self) -> Vec<String> {
        std::fs::read_to_string(self.project.join("invocations"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

fn wait(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "external fixture deadline");
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn receipt(shared: &Shared, request: &str) -> ExternalPromptReceipt {
    shared
        .store
        .external_prompt("cli", request)
        .unwrap()
        .unwrap()
}
fn settled(shared: &Shared, request: &str) -> ExternalPromptReceipt {
    wait(|| {
        receipt(shared, request).outcome.is_some()
            && shared.session("cli").unwrap().active_run().is_none()
    });
    receipt(shared, request)
}

#[test]
fn streaming_receipt_completes_once_and_duplicates_never_spawn_or_call_native() {
    let f = Fixture::new();
    let provider = Scripted::echo("must not be used");
    let mut engine = f.start(&provider);
    let shared = engine.handle.shared().clone();
    let cli = FakeCli::new(&f, "while [ ! -f release ]; do sleep 0.02; done", true);
    cli.seed(&shared);
    let first = cli.submit(&shared, "p", "text-only prompt").unwrap();
    let run = match first {
        Reply::Admitted { run: Some(run), .. } => run,
        _ => panic!("admission"),
    };
    wait(|| receipt(&shared, "p").terminal.is_some());
    let provisional = receipt(&shared, "p");
    assert_eq!(provisional.state, ExternalRunState::Running);
    assert!(provisional.result.is_none());
    // No unbounded waiting actors: a duplicate works even with zero spare capacity.
    let spare = shared
        .generations
        .clone()
        .try_acquire_many_owned(shared.generations.available_permits() as u32)
        .unwrap();
    assert!(
        matches!(cli.submit(&shared, "p", "text-only prompt"), Ok(Reply::Admitted { run: Some(id), .. }) if id == run)
    );
    assert_eq!(
        cli.submit(&shared, "p", "different text").unwrap_err().code,
        ErrorCode::Conflict
    );
    assert_eq!(
        cli.submit(&shared, "busy", "not queued").unwrap_err().code,
        ErrorCode::Busy
    );
    assert!(
        shared
            .store
            .external_prompt("cli", "busy")
            .unwrap()
            .is_none()
    );
    assert!(shared.session("cli").unwrap().failure().is_none());
    drop(spare);
    std::fs::write(f.project.join("release"), "").unwrap();
    let done = settled(&shared, "p");
    assert_eq!(done.state, ExternalRunState::Completed);
    assert_eq!(done.outcome, Some(RunOutcome::Completed));
    assert_eq!(
        done.result,
        Some(PromptResult::Done {
            text: "Synthetic answer".into(),
            reason: None
        })
    );
    assert!(done.evidence.as_ref().unwrap().projection_finished);
    assert!(
        matches!(cli.submit(&shared, "p", "text-only prompt"), Ok(Reply::Admitted { run: Some(id), .. }) if id == run)
    );
    assert_eq!(cli.launches().len(), 1);
    engine.until("external settlement", |events| {
        events.iter().any(|e| {
            matches!(e,
        Event::Session { change: SessionChange::RunSettled { run: id, .. }, .. } if *id == run)
        })
    });
    assert_eq!(
        engine
            .events
            .iter()
            .filter(|e| matches!(e,
        Event::Session { change: SessionChange::RunSettled { run: id, .. }, .. } if *id == run))
            .count(),
        1
    );
    assert!(engine.events.iter().any(|e| matches!(
        e,
        Event::Session {
            change: SessionChange::EntryAppended {
                entry: Entry {
                    kind: EntryKind::ExternalMessage {
                        role: ExternalRole::Assistant,
                        partial: true,
                        ..
                    },
                    ..
                }
            },
            ..
        }
    )));
    // A second distinct turn is Store-authorized Resume, never inferred by the runtime.
    cli.submit(&shared, "p2", "second turn").unwrap();
    assert_eq!(settled(&shared, "p2").mode, ExternalPromptMode::Resume);
    assert!(cli.launches()[0].contains("--session-id"));
    assert!(cli.launches()[1].contains("--resume"));
    for call in [
        Call::Prompt {
            session: "cli".into(),
            request: "native".into(),
            text: "no".into(),
            images: vec![],
            model: None,
        },
        Call::Stop {
            session: "cli".into(),
        },
    ] {
        assert_eq!(engine.call(call).unwrap_err().code, ErrorCode::Unsupported);
    }
    assert!(provider.requests().is_empty());
    let sess = shared.session("cli").unwrap();
    assert!(sess.extensions.try_lock().unwrap().is_none());
    assert!(sess.extension_profile.is_empty());
}

#[test]
fn terminal_then_timeout_is_unknown_and_duplicate_after_fencing_is_still_a_receipt() {
    let f = Fixture::new();
    let provider = Scripted::echo("unused");
    let engine = f.start(&provider);
    let shared = engine.handle.shared().clone();
    // The root exits zero on TERM. A successful terminal + exit zero still cannot
    // override the supervisor timeout (the child also holds the output pipes open).
    let cli = FakeCli::new(&f, "trap 'exit 0' TERM; sleep 30 & wait", true);
    cli.seed(&shared);
    let prepared = cli.prepared(&shared);
    shared
        .handle
        .block_on(shared.prompt_external_inner(
            "cli",
            "p",
            "timeout",
            prepared,
            Some(LaunchTest {
                timeout: Some(Duration::from_millis(200)),
                ..Default::default()
            }),
        ))
        .unwrap();
    let done = settled(&shared, "p");
    assert!(done.terminal.is_some());
    assert_eq!(done.state, ExternalRunState::Unknown);
    let evidence = done.evidence.unwrap();
    assert_eq!(evidence.exit_code, Some(0));
    assert!(!evidence.projection_finished);
    let binding = shared.store.cli_binding("cli").unwrap().unwrap();
    assert!(!binding.context_valid && binding.generation > done.key.generation);
    assert!(matches!(
        cli.submit(&shared, "p", "timeout"),
        Ok(Reply::Admitted { .. })
    ));
    assert_eq!(cli.launches().len(), 1);
    assert_eq!(
        cli.submit(&shared, "p2", "no retry").unwrap_err().code,
        ErrorCode::Conflict
    );
    assert!(shared.session("cli").unwrap().failure().is_none());
    assert!(provider.requests().is_empty());
}

#[test]
fn stop_commits_intent_then_kills_owned_group_and_fences_context_after_cleanup() {
    let f = Fixture::new();
    let provider = Scripted::echo("unused");
    let engine = f.start(&provider);
    let shared = engine.handle.shared().clone();
    let cli = FakeCli::new(
        &f,
        "(sleep 1; touch escaped-marker) & printf '%s\\n' \"$!\" > child-pid; wait",
        false,
    );
    cli.seed(&shared);
    cli.submit(&shared, "p", "stop me").unwrap();
    wait(|| receipt(&shared, "p").cursor >= 2 && f.project.join("child-pid").exists());
    let pid: i32 = std::fs::read_to_string(f.project.join("root-pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(matches!(
        shared.handle.block_on(shared.stop_external_prompt("cli")),
        Ok(Reply::Stopping { run: Some(_) })
    ));
    let done = settled(&shared, "p");
    assert_eq!(done.state, ExternalRunState::Interrupted);
    assert_eq!(done.outcome, Some(RunOutcome::Aborted));
    let evidence = done.evidence.unwrap();
    assert!(evidence.cancelled_clean_confirmed && evidence.root_reaped && evidence.pipes_drained);
    assert!(
        !shared
            .store
            .cli_binding("cli")
            .unwrap()
            .unwrap()
            .context_valid
    );
    // SAFETY: signal zero only probes the PID recorded by this synthetic executable.
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    std::thread::sleep(Duration::from_millis(1100));
    assert!(!f.project.join("escaped-marker").exists());
    assert!(provider.requests().is_empty());
}

#[test]
fn observer_rejection_after_terminal_never_completes_or_marks_storage_failure() {
    let f = Fixture::new();
    let provider = Scripted::echo("unused");
    let engine = f.start(&provider);
    let shared = engine.handle.shared().clone();
    let cli = FakeCli::new(&f, "printf '%s\\n' 'not-json'", true);
    cli.seed(&shared);
    cli.submit(&shared, "p", "observer rejection").unwrap();
    let done = settled(&shared, "p");
    assert!(done.terminal.is_some());
    assert_eq!(done.state, ExternalRunState::Unknown);
    let evidence = done.evidence.unwrap();
    assert!(!evidence.projection_finished);
    assert_eq!(
        evidence.supervisor_stop,
        Some(process::Stop::ObserverFailure)
    );
    assert_eq!(
        evidence.projection_error,
        Some(events::Error::MalformedRecord)
    );
    assert_eq!(evidence.rejected_record, Some(events::RecordClass::Unknown));
    // Existing schema-25 receipts omit these optional diagnostic facts.
    let mut old = serde_json::to_value(&evidence).unwrap();
    for field in [
        "supervisor_stop",
        "projection_error",
        "rejected_record",
        "parse_failure",
    ] {
        old.as_object_mut().unwrap().remove(field);
    }
    let old: ExternalSettlementEvidence = serde_json::from_value(old).unwrap();
    assert!(
        old.supervisor_stop.is_none()
            && old.projection_error.is_none()
            && old.rejected_record.is_none()
            && old.parse_failure.is_none()
    );
    assert!(shared.session("cli").unwrap().failure().is_none());
    assert!(
        !shared
            .store
            .cli_binding("cli")
            .unwrap()
            .unwrap()
            .context_valid
    );
    assert!(provider.requests().is_empty());
}

#[test]
fn invalid_text_and_native_identity_fail_before_admission() {
    let f = Fixture::new();
    let provider = Scripted::echo("unused");
    let mut engine = f.start(&provider);
    let shared = engine.handle.shared().clone();
    let cli = FakeCli::new(&f, ":", true);
    cli.seed(&shared);
    for text in [
        "\0".to_owned(),
        "x".repeat(policy::MAX_PROMPT_BYTES + 1),
        " ".into(),
    ] {
        assert!(cli.submit(&shared, "invalid", &text).is_err());
        assert!(
            shared
                .store
                .external_prompt("cli", "invalid")
                .unwrap()
                .is_none()
        );
    }
    let native = engine.create(&f.project, "native");
    let prepared = cli.prepared(&shared);
    let error = shared
        .handle
        .block_on(shared.prompt_external_prepared(&native, "p", "not external", prepared))
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Unsupported);
    assert!(shared.session("cli").unwrap().failure().is_none());
    assert!(cli.launches().is_empty());
    assert!(provider.requests().is_empty());
}

#[test]
fn storage_failure_cancels_drains_and_leaves_receipt_unfinished_for_recovery() {
    let f = Fixture::new();
    let provider = Scripted::echo("unused");
    let engine = f.start(&provider);
    let shared = engine.handle.shared().clone();
    let cli = FakeCli::new(&f, "sleep 30", false);
    cli.seed(&shared);
    shared.store.tx(|tx| {
        tx.execute_batch("CREATE TRIGGER fixture_projection_failure BEFORE UPDATE OF cursor ON external_prompts WHEN NEW.cursor=2 BEGIN SELECT RAISE(ABORT, 'fixture projection write denied'); END")?;
        Ok(())
    }).unwrap();
    cli.submit(&shared, "p", "storage failure").unwrap();
    let sess = shared.session("cli").unwrap();
    wait(|| sess.failure().is_some() && sess.active_run().is_none());
    let unfinished = receipt(&shared, "p");
    assert_eq!(unfinished.state, ExternalRunState::Running);
    assert_eq!(unfinished.cursor, 1);
    assert!(unfinished.evidence.is_none() && unfinished.outcome.is_none());
    assert!(
        !shared
            .store
            .cli_binding("cli")
            .unwrap()
            .unwrap()
            .context_valid
    );
    let pid: i32 = std::fs::read_to_string(f.project.join("root-pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // SAFETY: signal zero probes only this fixture's owned root PID.
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert_eq!(
        shared
            .store
            .recover_external_runs(crate::util::now_ms())
            .unwrap(),
        1
    );
    assert_eq!(receipt(&shared, "p").state, ExternalRunState::Unknown);
    assert_eq!(cli.launches().len(), 1);
    assert!(provider.requests().is_empty());
}

#[test]
fn shutdown_waits_for_owned_process_cleanup_projection_and_settlement() {
    let f = Fixture::new();
    let provider = Scripted::echo("unused");
    let engine = f.start(&provider);
    let shared = engine.handle.shared().clone();
    let cli = FakeCli::new(
        &f,
        "trap 'sleep 0.1; touch cleanup-complete; exit 0' TERM; touch ready; while :; do sleep 1; done",
        true,
    );
    cli.seed(&shared);
    cli.submit(&shared, "p", "shutdown").unwrap();
    wait(|| f.project.join("ready").exists() && receipt(&shared, "p").terminal.is_some());
    assert!(!shared.external_tasks.is_empty());
    let pid: i32 = std::fs::read_to_string(f.project.join("root-pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    engine.handle.shutdown(Duration::from_secs(2));
    // No post-shutdown wait: these must be true BEFORE runtime destruction.
    assert!(f.project.join("cleanup-complete").exists());
    assert!(shared.external_tasks.is_empty());
    assert!(shared.session("cli").unwrap().active_run().is_none());
    let done = receipt(&shared, "p");
    assert_eq!(done.state, ExternalRunState::Unknown);
    let evidence = done.evidence.unwrap();
    assert!(evidence.root_reaped && evidence.pipes_drained && !evidence.cleanup_timed_out);
    assert!(!evidence.projection_finished);
    // Global stopping is not durable user Stop intent, even with clean cleanup.
    assert!(!matches!(done.outcome, Some(RunOutcome::Aborted)));
    // SAFETY: only this fake process's recorded PID is probed.
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert!(provider.requests().is_empty());
}

#[test]
fn shutdown_deadline_leaves_unsettled_receipt_for_recovery_not_false_cancel() {
    let f = Fixture::new();
    let provider = Scripted::echo("unused");
    let engine = f.start(&provider);
    let shared = engine.handle.shared().clone();
    let cli = FakeCli::new(&f, "sleep 30", false);
    cli.seed(&shared);
    cli.submit(&shared, "p", "bounded shutdown").unwrap();
    wait(|| receipt(&shared, "p").cursor >= 2);
    let sess = shared.session("cli").unwrap();
    // Delay settlement beyond the shutdown budget, without blocking a runtime worker.
    let lock = shared.handle.block_on(sess.lock.lock());
    let start = Instant::now();
    engine.handle.shutdown(Duration::from_millis(30));
    assert!(start.elapsed() < Duration::from_secs(3));
    let unfinished = receipt(&shared, "p");
    assert!(unfinished.outcome.is_none() && unfinished.evidence.is_none());
    drop(lock);
    assert_eq!(
        shared
            .store
            .recover_external_runs(crate::util::now_ms())
            .unwrap(),
        1
    );
    assert_eq!(receipt(&shared, "p").state, ExternalRunState::Unknown);
    assert_eq!(cli.launches().len(), 1);
    assert!(provider.requests().is_empty());
}

#[test]
fn registered_unpolled_actor_is_drained_and_late_admission_cannot_spawn() {
    let f = Fixture::new();
    let provider = Scripted::echo("unused");
    let engine = f.start(&provider);
    let shared = engine.handle.shared().clone();
    let cli = FakeCli::new(&f, ":", true);
    cli.seed(&shared);
    let prepared = cli.prepared(&shared);
    let late = cli.prepared(&shared);
    let (_release, gate) = tokio::sync::oneshot::channel();
    shared
        .handle
        .block_on(shared.prompt_external_inner(
            "cli",
            "p",
            "not launched",
            prepared,
            Some(LaunchTest {
                gate: Some(gate),
                ..Default::default()
            }),
        ))
        .unwrap();
    assert_eq!(shared.external_tasks.len(), 1);
    shared.stopping.cancel();
    let done = shared.external_shutdown();
    shared.handle.block_on(async {
        tokio::time::timeout(Duration::from_secs(2), done.wait())
            .await
            .unwrap();
        assert_eq!(
            shared
                .prompt_external_prepared("cli", "late", "no launch", late)
                .await
                .unwrap_err()
                .code,
            ErrorCode::Busy
        );
    });
    assert!(shared.external_tasks.is_empty());
    assert!(cli.launches().is_empty());
    assert_eq!(receipt(&shared, "p").state, ExternalRunState::Unknown);
    assert!(
        shared
            .store
            .external_prompt("cli", "late")
            .unwrap()
            .is_none()
    );
    assert!(provider.requests().is_empty());
}

#[test]
fn actor_revalidates_delayed_executable_and_project_candidates_before_spawn() {
    for change in ["unchanged", "executable", "project", "expired"] {
        let f = Fixture::new();
        let provider = Scripted::echo("unused");
        let engine = f.start(&provider);
        let shared = engine.handle.shared().clone();
        let cli = FakeCli::new(&f, ":", true);
        cli.seed(&shared);
        let prepared = cli.prepared(&shared);
        let (release, gate) = tokio::sync::oneshot::channel();
        shared
            .handle
            .block_on(shared.prompt_external_inner(
                "cli",
                "p",
                "delayed candidate",
                prepared,
                Some(LaunchTest {
                    gate: Some(gate),
                    ..Default::default()
                }),
            ))
            .unwrap();
        assert_eq!(receipt(&shared, "p").state, ExternalRunState::Running);
        assert!(cli.launches().is_empty());
        match change {
            "executable" => {
                let replacement = f.dir.path().join("replacement");
                std::fs::copy(&cli.executable, &replacement).unwrap();
                std::fs::rename(replacement, &cli.executable).unwrap();
            }
            "project" => {
                std::fs::rename(&f.project, f.dir.path().join("old-project")).unwrap();
                std::fs::create_dir(&f.project).unwrap();
            }
            // Prepared intentionally has no clock mutation API. Exercise its actual
            // std::time freshness deadline; no installed CLI or inference is involved.
            "expired" => std::thread::sleep(Duration::from_secs(31)),
            _ => std::thread::sleep(Duration::from_millis(50)),
        }
        release.send(()).unwrap();
        let done = settled(&shared, "p");
        if change == "unchanged" {
            assert_eq!(done.state, ExternalRunState::Completed);
            assert_eq!(cli.launches().len(), 1);
        } else {
            assert_eq!(done.state, ExternalRunState::Unknown, "{change}");
            let evidence = done.evidence.unwrap();
            assert!(!evidence.input_delivered && !evidence.cancelled_clean_confirmed);
            assert!(cli.launches().is_empty(), "{change}");
            assert!(!f.dir.path().join("old-project/invocations").exists());
            assert!(
                !shared
                    .store
                    .cli_binding("cli")
                    .unwrap()
                    .unwrap()
                    .context_valid
            );
            assert!(matches!(
                cli.submit(&shared, "p", "delayed candidate"),
                Ok(Reply::Admitted { .. })
            ));
        }
        assert!(shared.session("cli").unwrap().failure().is_none());
        assert!(provider.requests().is_empty());
    }
}

#[test]
fn removal_cleans_process_but_leaves_receipt_for_recovery() {
    let f = Fixture::new();
    let provider = Scripted::echo("unused");
    let engine = f.start(&provider);
    let shared = engine.handle.shared().clone();
    let cli = FakeCli::new(&f, "sleep 30", false);
    cli.seed(&shared);
    cli.submit(&shared, "p", "removed").unwrap();
    wait(|| receipt(&shared, "p").cursor >= 2);
    let sess = shared.session("cli").unwrap();
    sess.removed.cancel();
    wait(|| shared.external_tasks.is_empty());
    assert!(sess.active_run().is_none());
    assert!(receipt(&shared, "p").outcome.is_none());
    assert!(receipt(&shared, "p").evidence.is_none());
    let pid: i32 = std::fs::read_to_string(f.project.join("root-pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // SAFETY: only this fake process's recorded PID is probed.
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert!(provider.requests().is_empty());
}

#[test]
fn eof_exit_zero_without_terminal_never_completes() {
    let f = Fixture::new();
    let provider = Scripted::echo("unused");
    let engine = f.start(&provider);
    let shared = engine.handle.shared().clone();
    let cli = FakeCli::new(&f, ":", false);
    cli.seed(&shared);
    cli.submit(&shared, "p", "no result").unwrap();
    let done = settled(&shared, "p");
    assert_eq!(done.state, ExternalRunState::Unknown);
    assert!(done.terminal.is_none());
    assert!(!done.evidence.unwrap().projection_finished);
    assert!(provider.requests().is_empty());
}
