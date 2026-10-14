//! Real SIGKILL fault cuts against a synthetic executor, never the installed Claude CLI.
//! This tests the fixture ledger, not production engine recovery or CLI history semantics.
#![cfg(unix)]

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::interruption::ticket;
use super::lifecycle::{ApprovalState, Ledger, Phase};

const SESSION: &str = "11111111-1111-4111-8111-111111111111";
const REQUEST: &str = "22222222-2222-4222-8222-222222222222";
const FINGERPRINT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CHILD_MODE: &str = "PIPKIN_CLAUDE_PROBE_CRASH_CUT";
const CHILD_ROOT: &str = "PIPKIN_CLAUDE_PROBE_CRASH_ROOT";

fn append_effect(root: &Path, name: &str) {
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(root.join(name))
        .unwrap();
    file.write_all(b"1\n").unwrap();
    file.sync_all().unwrap();
}

/// Invoked in a separately filtered test process. No CLI executable is involved.
#[test]
fn crash_child() {
    let Some(cut) = std::env::var_os(CHILD_MODE) else {
        return;
    };
    let cut = cut.to_str().unwrap();
    let root = std::path::PathBuf::from(std::env::var_os(CHILD_ROOT).unwrap());
    let t = ticket(SESSION);
    let mut ledger = Ledger::create(&root.join("run.sqlite3")).unwrap();
    ledger.admit(&t).unwrap();
    if cut != "admitted" {
        ledger.launch_intent(&t).unwrap();
        if cut != "launch_intent" {
            ledger.running(&t).unwrap();
            append_effect(&root, "submission-receipts");
            if cut != "submitted" {
                ledger
                    .request(&t, REQUEST, FINGERPRINT, 100, 60_000)
                    .unwrap();
                if cut != "pending" {
                    assert!(ledger.decide(&t, REQUEST, FINGERPRINT, true, 101).unwrap());
                    if cut != "granted" {
                        assert!(ledger.consume(&t, REQUEST, FINGERPRINT, 102).unwrap());
                        if cut != "consumed" {
                            append_effect(&root, "external-effect");
                            if cut == "completed" {
                                // Synthetic valid final + confirmed exit, not a real CLI result.
                                ledger.complete(&t).unwrap();
                            } else {
                                assert_eq!(cut, "effect");
                            }
                        }
                    }
                }
            }
        }
    }
    // Force the parent to kill a process with its DB connection still open.
    append_effect(&root, "checkpoint-ready");
    loop {
        std::thread::sleep(Duration::from_secs(30));
    }
}

struct OwnedChild(std::process::Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn count(root: &Path, name: &str) -> usize {
    match std::fs::read(root.join(name)) {
        Ok(bytes) => {
            assert_eq!(bytes, b"1\n");
            1
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
        Err(e) => panic!("fixture evidence unavailable: {e}"),
    }
}

#[test]
fn killed_host_never_replays_submission_or_external_effect_and_blocks_unsafe_resume() {
    for cut in [
        "admitted",
        "launch_intent",
        "submitted",
        "pending",
        "granted",
        "consumed",
        "effect",
        "completed",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "recovery_tests::crash_child", "--nocapture"])
            .env(CHILD_MODE, cut)
            .env(CHILD_ROOT, root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut child = OwnedChild(child);
        let until = Instant::now() + Duration::from_secs(10);
        while !root.join("checkpoint-ready").exists() {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "child exited before {cut} checkpoint"
            );
            assert!(Instant::now() < until, "{cut} checkpoint timeout");
            std::thread::sleep(Duration::from_millis(5));
        }
        child.0.kill().unwrap();
        let status = child.0.wait().unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(libc::SIGKILL));
        let sent = count(root, "submission-receipts");
        let effects = count(root, "external-effect");
        assert_eq!(
            sent,
            usize::from(!matches!(cut, "admitted" | "launch_intent"))
        );
        assert_eq!(effects, usize::from(matches!(cut, "effect" | "completed")));
        let mut ledger = Ledger::open(&root.join("run.sqlite3")).unwrap();
        let completed = cut == "completed";
        assert_eq!(ledger.recover().unwrap(), usize::from(!completed));
        assert_eq!(ledger.recover().unwrap(), 0);
        let t = ticket(SESSION);
        let snapshot = ledger.snapshot(&t).unwrap();
        assert_eq!(
            snapshot.phase,
            if completed {
                Phase::Completed
            } else {
                Phase::Unknown
            }
        );
        assert_eq!(snapshot.resume_allowed, completed);
        assert_eq!(snapshot.queue_allowed, completed);
        assert_eq!(snapshot.context_valid, completed);
        if matches!(cut, "pending" | "granted") {
            assert_eq!(
                ledger.approval(&t, REQUEST, 103).unwrap(),
                ApprovalState::Revoked
            );
        }
        assert!(!ledger.decide(&t, REQUEST, FINGERPRINT, true, 103).unwrap());
        assert!(!ledger.consume(&t, REQUEST, FINGERPRINT, 103).unwrap());
        assert!(
            ledger.admit(&t).is_err(),
            "same admission must never replay"
        );
        if !completed {
            let mut next = t.clone();
            next.run += 1;
            assert!(
                ledger.admit(&next).is_err(),
                "unsafe session cannot continue"
            );
            assert!(
                ledger.complete(&t).is_err(),
                "late result cannot resolve unknown execution"
            );
        }
        assert_eq!(count(root, "submission-receipts"), sent);
        assert_eq!(count(root, "external-effect"), effects);
    }
}
