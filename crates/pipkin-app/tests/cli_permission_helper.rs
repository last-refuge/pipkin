//! Desktop binary helper dispatch must bypass logging, storage and GPUI startup.
//! Only our helper binary runs; no Claude CLI or account request is made.
#![cfg(unix)]
use std::process::{Command, Stdio};

#[test]
fn malformed_helper_invocation_is_fixed_denial_without_desktop_startup() {
    let home = tempfile::tempdir().unwrap();
    for args in [
        vec!["--claude-permission-hook"],
        vec![
            "--claude-permission-hook",
            "--socket",
            "private-canary-relative",
        ],
        vec![
            "--claude-permission-hook",
            "--socket",
            "/nonexistent/socket",
            "--extra",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_pipkin"))
            .args(args)
            .env_clear()
            .env("HOME", home.path())
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let decision = &body["hookSpecificOutput"]["decision"];
        assert_eq!(decision["behavior"], "deny");
        assert_eq!(decision["interrupt"], true);
        assert!(decision.get("updatedInput").is_none());
        assert!(decision.get("updatedPermissions").is_none());
        assert!(!String::from_utf8_lossy(&output.stdout).contains("private-canary"));
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
    }
}
