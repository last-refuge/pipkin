//! Offline deterministic lock-order regression. No provider or CLI execution.
use crate::{provider::scripted::Scripted, runtime::tests::Fixture};
use std::time::{Duration, Instant};

#[test]
fn cli_mode_is_present_in_committed_snapshot_before_ephemeral_overlay() {
    use pipkin_engine_protocol::{LocalCliMode, UiView};
    let fixture = Fixture::new();
    let provider = Scripted::echo("must not run");
    let engine = fixture.start(&provider);
    let shared = engine.handle.shared().clone();
    shared
        .store
        .create_cli_session_with_model(
            "cli-mode",
            "create",
            &crate::store::CliBindingSpec {
                cli_session_id: "12345678-1234-4234-8234-123456789abc".into(),
                project_path: "/project".into(),
                executable_path: "/fake/claude".into(),
                cli_version: "2.1.296".into(),
                config_ref: "config".into(),
                account_scope: "account".into(),
                launch_policy: crate::execution::claude_cli::write_policy::ID.into(),
            },
            "sonnet",
            1,
        )
        .unwrap();
    let registry = shared.sessions.lock().unwrap();
    let reader = shared.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let _publication = reader.publication.lock().unwrap();
        let _ = tx.send(reader.snapshot_once("cli-mode", 20, &UiView::default(), 0));
    });
    let result = rx.recv_timeout(Duration::from_secs(2));
    drop(registry);
    if result.is_err() {
        let _ = rx.recv_timeout(Duration::from_secs(2));
    }
    thread.join().unwrap();
    let snapshot = result
        .expect("committed CLI identity acquired session registry")
        .unwrap();
    assert_eq!(
        snapshot.summary.local_cli_mode,
        Some(LocalCliMode::RestrictedWrite)
    );
    assert!(snapshot.summary.blocked.is_some());
    assert!(provider.requests().is_empty());
}

#[test]
fn committed_snapshot_never_takes_session_registry_including_cached_tokens() {
    let fixture = Fixture::new();
    let provider = Scripted::echo("must not run");
    let mut engine = fixture.start(&provider);
    let id = engine.create(&fixture.project, "snapshot-locks");
    let shared = engine.handle.shared().clone();
    let runtime = shared.session(&id).unwrap();
    let ui = runtime.questions.view();
    let tokens = super::Shared::cached_system_tokens(&runtime);
    let registry = shared.sessions.lock().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    // Block the first stored read, proving the reader actually acquired publication.
    shared
        .store
        .read(|_| {
            let reader = shared.clone();
            std::thread::spawn(move || {
                let _ = tx.send(reader.snapshot_from_cache(&id, 20, &ui, tokens));
            });
            let deadline = Instant::now() + Duration::from_secs(2);
            while shared.publication.try_lock().is_ok() {
                assert!(
                    Instant::now() < deadline,
                    "reader never acquired publication"
                );
                std::thread::yield_now();
            }
            Ok(())
        })
        .unwrap();
    // Registry stays held throughout the entire committed phase, including context
    // computation. Any new hidden session() lookup in that phase blocks completion.
    let result = rx.recv_timeout(Duration::from_secs(2));
    drop(registry);
    if result.is_err() {
        // Let a regressed reader unwind before reporting failure.
        let _ = rx.recv_timeout(Duration::from_secs(2));
    }
    result
        .expect("committed snapshot acquired session registry")
        .unwrap();
    assert!(provider.requests().is_empty());
}
