//! Headless packaging and bounded stdin, without an engine, CLI or account request.
#![cfg(unix)]
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn packaged_helper_roundtrips_to_a_fake_private_peer_without_startup_or_effects() {
    use pipkin_engine::execution::claude_cli::{
        permission::{CliScope, PermissionRequest},
        permission_wire as wire,
    };
    use std::os::unix::fs::PermissionsExt;
    use tokio::{
        io::AsyncWriteExt,
        net::UnixListener,
        time::{Instant as AsyncInstant, timeout},
    };
    let base = std::path::PathBuf::from(std::env::var_os("HOME").unwrap())
        .canonicalize()
        .unwrap();
    let home = tempfile::Builder::new()
        .prefix(".ph-test-")
        .tempdir_in(base)
        .unwrap();
    std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = home.path().join("s");
    let listener = UnixListener::bind(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let uuid = "11111111-1111-4111-8111-111111111111";
    for valid in [true, false] {
        let nonce = wire::Binding::fresh().unwrap();
        let input = if valid {
            serde_json::json!({
                "session_id":uuid, "cwd":"/fixture/project", "permission_mode":"default",
                "hook_event_name":"PermissionRequest", "tool_name":"Write",
                "tool_input":{"file_path":home.path().join("never-written"), "content":"synthetic"}
            })
            .to_string()
        } else {
            "not JSON".into()
        };
        let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_pipkin-engine"))
            .args(["--claude-permission-hook", "--socket"])
            .arg(&path)
            .env_clear()
            .env("HOME", home.path())
            .env("PIPKIN_CLI_PERMISSION_NONCE", nonce.to_hex())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(input.as_bytes()).await.unwrap();
        stdin.shutdown().await.unwrap();
        drop(stdin);
        let (mut peer, _) = timeout(Duration::from_secs(5), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let frame = wire::read_request(&mut peer, AsyncInstant::now() + Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(frame.run_nonce, nonce);
        assert_eq!(frame.payload, input);
        let parsed = PermissionRequest::parse(
            frame.payload.as_bytes(),
            &CliScope::new(uuid, "/fixture/project").unwrap(),
        );
        assert_eq!(parsed.is_ok(), valid);
        let decision = if valid {
            wire::Decision::AllowOnce
        } else {
            wire::Decision::Deny
        };
        let reply = wire::ReplyFrame::for_request(&frame, decision).unwrap();
        wire::write_reply(
            &mut peer,
            &reply,
            AsyncInstant::now() + Duration::from_secs(2),
        )
        .await
        .unwrap();
        let output = timeout(Duration::from_secs(5), child.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let result = &result["hookSpecificOutput"]["decision"];
        assert_eq!(result["behavior"], if valid { "allow" } else { "deny" });
        assert!(result.get("updatedInput").is_none());
        assert!(result.get("updatedPermissions").is_none());
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 1); // socket only
    }
}

#[test]
fn helper_stdin_deadline_denies_without_starting_an_engine() {
    let home = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_pipkin-engine"))
        .args(["--claude-permission-hook", "--socket"])
        .arg(home.path().join("no-listener"))
        .env_clear()
        .env("HOME", home.path())
        .env("PIPKIN_CLI_PERMISSION_NONCE", "ab".repeat(32))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Keep the writer open and send nothing. Reading stdin must not hang forever.
    let writer = child.stdin.take().unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut expired = false;
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            expired = true;
            child.kill().unwrap();
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    drop(writer);
    assert!(!expired, "helper did not enforce its stdin deadline");
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["hookSpecificOutput"]["decision"]["behavior"], "deny");
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
}
