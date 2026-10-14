use super::*;

const SESSION: &str = "11111111-1111-4111-8111-111111111111";
const REQUEST: &str = "22222222-2222-4222-8222-222222222222";
const CHILD_ROOT: &str = "PIPKIN_CLAUDE_PROBE_HOOK_ROOT";
const CHILD_MODE: &str = "PIPKIN_CLAUDE_PROBE_HOOK_MODE";

fn input(root: &Path) -> Vec<u8> {
    serde_json::to_vec(&json!({"session_id":SESSION,"cwd":root,
        "hook_event_name":"PermissionRequest","tool_name":"Write",
        "tool_input":{"file_path":root.join(permission::TARGET),"content":permission::CONTENT}}))
    .unwrap()
}

fn setup(root: &Path) -> Ledger {
    std::fs::create_dir(root.join("project")).unwrap();
    let mut ledger = Ledger::create(&root.join(DATABASE)).unwrap();
    ledger.admit(&ticket(SESSION)).unwrap();
    ledger
}

fn spec(root: &Path, mode: &str) -> process::Spec {
    let mut spec = super::super::base_spec(
        &std::env::current_exe().unwrap(),
        root,
        &std::env::vars_os().collect(),
        ["--exact", "interruption::tests::hook_child", "--nocapture"]
            .into_iter()
            .map(Into::into)
            .collect(),
    );
    spec.env.insert(CHILD_ROOT.into(), root.as_os_str().into());
    spec.env.insert(CHILD_MODE.into(), mode.into());
    spec.timeout = Duration::from_secs(5);
    spec.grace = Duration::from_millis(50);
    spec
}

/// The synthetic CLI calls the same hook function, then applies exactly the test Write
/// iff the returned public decision is allow. It never contacts a model or executes shell.
#[test]
fn hook_child() {
    let Some(root) = std::env::var_os(CHILD_ROOT) else {
        return;
    };
    let root = std::path::PathBuf::from(root).join("project");
    let mode = std::env::var(CHILD_MODE).unwrap();
    let response = if mode == "expiry" {
        wait_with_ttl(&input(&root), &root, SESSION, REQUEST, 200)
    } else {
        wait_for_decision(&input(&root), &root, SESSION, REQUEST)
    };
    if response["hookSpecificOutput"]["decision"]["behavior"] == "allow" {
        std::fs::write(root.join(permission::TARGET), permission::CONTENT).unwrap();
    } else {
        assert_eq!(response, permission::denial());
    }
}

#[tokio::test]
async fn real_owned_child_is_stopped_at_pending_approval_and_late_allow_cannot_write() {
    let dir = tempfile::tempdir().unwrap();
    let mut ledger = setup(dir.path());
    let root = dir.path().join("project");
    let fp = fingerprint(&root, SESSION).unwrap();
    let outcome = drive(
        spec(dir.path(), "pending"),
        &mut ledger,
        &dir.path().join(DATABASE),
        &ticket(SESSION),
        REQUEST,
        &fp,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(
        outcome.pending_observed && outcome.late_approval_rejected && !outcome.externally_cancelled
    );
    assert_eq!(outcome.report.stop, Some(process::Stop::Cancelled));
    assert_eq!(outcome.report.approval_revocation_confirmed, Some(true));
    assert!(
        outcome.report.root_reaped
            && outcome.report.pipes_drained
            && !outcome.report.cleanup_timed_out
    );
    assert_eq!(
        ledger.snapshot(&ticket(SESSION)).unwrap().phase,
        Phase::Interrupted
    );
    assert_eq!(
        ledger
            .approval(&ticket(SESSION), REQUEST, now_ms().unwrap())
            .unwrap(),
        ApprovalState::Revoked
    );
    assert!(!qualification::path_exists(&root.join(permission::TARGET)).unwrap());
    drop(ledger);
    let mut recovered = Ledger::open(&dir.path().join(DATABASE)).unwrap();
    assert_eq!(recovered.recover().unwrap(), 0);
    let snapshot = recovered.snapshot(&ticket(SESSION)).unwrap();
    assert!(!snapshot.context_valid && !snapshot.resume_allowed && !snapshot.queue_allowed);
}

#[tokio::test]
async fn external_cancel_during_failed_cleanup_cannot_reclassify_failure_as_interrupted() {
    for invalid_stream in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut ledger = setup(dir.path());
        let path = dir.path().join(DATABASE);
        let t = ticket(SESSION);
        let mut spec = super::super::base_spec(
            Path::new("/bin/sh"),
            dir.path(),
            &std::env::vars_os().collect(),
            [
                "-c",
                if invalid_stream {
                    "printf 'bad-json\\n'; sleep 30"
                } else {
                    "sleep 30"
                },
            ]
            .into_iter()
            .map(Into::into)
            .collect(),
        );
        spec.timeout = Duration::from_millis(100);
        spec.grace = Duration::from_millis(500);
        if invalid_stream {
            spec.mode = process::OutputMode::Stream;
        }
        let cancel = CancellationToken::new();
        let late_cancel = cancel.clone();
        let observer = Ledger::open(&path).unwrap();
        let observed_ticket = t.clone();
        let canceller = tokio::spawn(async move {
            tokio::time::timeout(Duration::from_secs(3), async move {
                while observer.snapshot(&observed_ticket).unwrap().phase != Phase::Stopping {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            late_cancel.cancel();
        });
        let outcome = drive(
            spec,
            &mut ledger,
            &path,
            &t,
            REQUEST,
            &fingerprint(&dir.path().join("project"), SESSION).unwrap(),
            cancel,
        )
        .await
        .unwrap();
        canceller.await.unwrap();
        assert!(outcome.externally_cancelled);
        assert_eq!(
            outcome.report.stop,
            Some(if invalid_stream {
                process::Stop::InvalidStream
            } else {
                process::Stop::TimedOut
            })
        );
        assert_eq!(outcome.report.approval_revocation_confirmed, Some(true));
        assert_eq!(ledger.snapshot(&t).unwrap().phase, Phase::Unknown);
    }
}

#[tokio::test]
async fn explicit_grant_positive_control_reaches_hook_and_writes_exactly_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut ledger = setup(dir.path());
    let t = ticket(SESSION);
    ledger.launch_intent(&t).unwrap();
    ledger.running(&t).unwrap();
    let root = dir.path().join("project");
    let fp = fingerprint(&root, SESSION).unwrap();
    let child = process::run(spec(dir.path(), "grant"), CancellationToken::new());
    tokio::pin!(child);
    let mut poll = tokio::time::interval(Duration::from_millis(5));
    let mut granted = false;
    let report = loop {
        tokio::select! {
            result = &mut child => break result.unwrap(),
            _ = poll.tick(), if !granted => {
                if matches!(ledger.approval(&t, REQUEST, now_ms().unwrap()), Ok(ApprovalState::Pending)) {
                    granted = ledger.decide(&t, REQUEST, &fp, true, now_ms().unwrap()).unwrap();
                }
            }
        }
    };
    assert!(granted && report.clean_exit(), "{report:?}");
    assert_eq!(
        std::fs::read(root.join(permission::TARGET)).unwrap(),
        permission::CONTENT.as_bytes()
    );
    assert_eq!(
        ledger.approval(&t, REQUEST, now_ms().unwrap()).unwrap(),
        ApprovalState::Consumed
    );
    assert!(!ledger.consume(&t, REQUEST, &fp, now_ms().unwrap()).unwrap());
    // A successful permission decision alone does not complete the CLI run.
    assert!(!ledger.snapshot(&t).unwrap().resume_allowed);
}

#[tokio::test]
async fn absent_host_expires_hook_and_late_approval_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut ledger = setup(dir.path());
    let t = ticket(SESSION);
    ledger.launch_intent(&t).unwrap();
    ledger.running(&t).unwrap();
    drop(ledger); // No host connection or decision remains; only the hook opens the DB.
    let report = process::run(spec(dir.path(), "expiry"), CancellationToken::new())
        .await
        .unwrap();
    assert!(report.clean_exit(), "{report:?}");
    let root = dir.path().join("project");
    assert!(!qualification::path_exists(&root.join(permission::TARGET)).unwrap());
    let mut ledger = Ledger::open(&dir.path().join(DATABASE)).unwrap();
    assert_eq!(
        ledger.approval(&t, REQUEST, now_ms().unwrap()).unwrap(),
        ApprovalState::Expired
    );
    assert!(
        !ledger
            .decide(
                &t,
                REQUEST,
                &fingerprint(&root, SESSION).unwrap(),
                true,
                now_ms().unwrap()
            )
            .unwrap()
    );
    assert_eq!(ledger.recover().unwrap(), 1);
    assert_eq!(ledger.snapshot(&t).unwrap().phase, Phase::Unknown);
}

#[test]
fn malformed_or_mismatched_input_cannot_enqueue_a_permission() {
    let dir = tempfile::tempdir().unwrap();
    let mut ledger = setup(dir.path());
    let t = ticket(SESSION);
    ledger.launch_intent(&t).unwrap();
    ledger.running(&t).unwrap();
    let root = dir.path().join("project");
    let mut wrong: Value = serde_json::from_slice(&input(&root)).unwrap();
    wrong["tool_input"]["content"] = json!("not authorized");
    for bytes in [
        b"invalid json".to_vec(),
        serde_json::to_vec(&wrong).unwrap(),
    ] {
        assert_eq!(
            wait_for_decision(&bytes, &root, SESSION, REQUEST),
            permission::denial()
        );
    }
    assert!(ledger.approval(&t, REQUEST, now_ms().unwrap()).is_err());
    assert!(!qualification::path_exists(&root.join(permission::TARGET)).unwrap());
}

#[test]
fn hook_settings_fail_closed_without_safe_mode_or_permission_updates() {
    let settings = settings(
        Path::new("/helper's path"),
        Path::new("/scratch's project"),
        SESSION,
        REQUEST,
    )
    .unwrap();
    assert_eq!(settings["permissions"]["ask"], json!(["Write"]));
    let hook = &settings["hooks"]["PermissionRequest"][0]["hooks"][0];
    assert_eq!(hook["timeout"], 20);
    assert_eq!(hook["onFailure"], "block");
    assert!(hook["command"].as_str().unwrap().contains("'\\''"));
    assert!(!settings.to_string().contains("updatedPermissions"));
    assert!(
        super::settings(
            Path::new("/helper"),
            Path::new("/root"),
            "--bad-session",
            REQUEST
        )
        .is_err()
    );
}
