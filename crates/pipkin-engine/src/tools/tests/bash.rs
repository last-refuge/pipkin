//! `bash`: ports of `tools.test.ts` "bash", `env-node.test.ts` shell cases and
//! `harness-tools.test.ts` "coding tools", plus pipe draining, process-group cancellation and
//! the spill file.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;

use super::{ctx, ctx_with, execute, result_text, temp};
use crate::tools::bash::{BashTool, resolve_shell};
use crate::tools::{Severity, run};

fn spill_path(diagnostics: &[crate::tools::Diagnostic]) -> Option<PathBuf> {
    diagnostics.iter().find_map(|d| {
        (d.code == "full_output")
            .then(|| PathBuf::from(d.message.strip_prefix("Full output: ").unwrap()))
    })
}

#[tokio::test]
async fn streams_combined_stdout_and_stderr_and_returns_no_content_of_its_own() {
    let dir = temp();
    let (result, output) = execute(
        &BashTool,
        json!({"command": "printf out; printf err >&2"}),
        &ctx(dir.path()),
    )
    .await;
    let out = result.unwrap();
    assert!(output.contains("out") && output.contains("err"));
    assert_eq!(out.content, None);
    assert!(out.diagnostics.is_empty());
}

#[tokio::test]
async fn throws_on_nonzero_exits_and_timeouts_after_streaming_the_output() {
    let dir = temp();
    let c = ctx(dir.path());
    let (failed, output) =
        execute(&BashTool, json!({"command": "printf failed; exit 7"}), &c).await;
    assert_eq!(failed.unwrap_err().message, "Command exited with code 7");
    assert_eq!(output, "failed");
    let started = std::time::Instant::now();
    let (slow, _) = execute(
        &BashTool,
        json!({"command": "sleep 2", "timeout": 0.01}),
        &c,
    )
    .await;
    assert_eq!(
        slow.unwrap_err().message,
        "Command timed out after 0.01 seconds"
    );
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[tokio::test]
async fn rejects_invalid_timeouts_before_spawning() {
    let dir = temp();
    let c = ctx(dir.path());
    for (timeout, message) in [
        (
            json!(0),
            "Invalid timeout: must be a finite number of seconds",
        ),
        (
            json!(-1),
            "Invalid timeout: must be a finite number of seconds",
        ),
        (
            json!(2147484),
            "Invalid timeout: maximum is 2147483.647 seconds",
        ),
    ] {
        let (result, _) = execute(
            &BashTool,
            json!({"command": "touch spawned", "timeout": timeout}),
            &c,
        )
        .await;
        assert_eq!(result.unwrap_err().message, message);
    }
    assert!(!dir.path().join("spawned").exists());
}

#[tokio::test]
async fn reports_a_missing_working_directory_and_a_missing_custom_shell() {
    let dir = temp();
    let gone = dir.path().join("gone");
    let (result, _) = execute(&BashTool, json!({"command": "true"}), &ctx(&gone)).await;
    assert_eq!(
        result.unwrap_err().message,
        format!(
            "Working directory does not exist: {}\nCannot execute bash commands.",
            gone.display()
        )
    );
    let mut c = ctx(dir.path());
    c.shell = Some(dir.path().join("no-shell"));
    let (result, _) = execute(&BashTool, json!({"command": "true"}), &c).await;
    assert_eq!(
        result.unwrap_err().message,
        format!(
            "Custom shell path not found: {}",
            dir.path().join("no-shell").display()
        )
    );
    assert_eq!(resolve_shell(None).unwrap(), PathBuf::from("/bin/bash"));
}

#[tokio::test]
async fn executes_in_cwd_with_env_overrides_and_a_custom_shell() {
    let dir = temp();
    let mut c = ctx(dir.path());
    c.env = vec![("PI_TOOL_TEST".into(), "from-ctx".into())];
    c.shell = Some(PathBuf::from("/bin/sh"));
    let (result, output) = execute(
        &BashTool,
        json!({"command": "printf '%s:%s:%s' \"$PWD\" \"$PI_TOOL_TEST\" \"${HOME:+home}\""}),
        &c,
    )
    .await;
    result.unwrap();
    let canonical = std::fs::canonicalize(dir.path()).unwrap();
    assert_eq!(output, format!("{}:from-ctx:home", canonical.display()));
}

#[cfg(unix)]
#[tokio::test]
async fn maps_signal_killed_processes_to_128_plus_the_signal() {
    let dir = temp();
    let (result, _) = execute(
        &BashTool,
        json!({"command": "kill -9 $$"}),
        &ctx(dir.path()),
    )
    .await;
    assert_eq!(result.unwrap_err().message, "Command exited with code 137");
}

#[tokio::test]
async fn decodes_utf8_split_across_writes() {
    let dir = temp();
    let (result, output) = execute(
        &BashTool,
        json!({"command": "printf '\\xe2'; sleep 0.05; printf '\\x82\\xac\\n'"}),
        &ctx(dir.path()),
    )
    .await;
    result.unwrap();
    assert_eq!(output, "€\n");
}

#[tokio::test]
async fn streams_every_byte_and_spills_complete_output_beyond_the_default_limits() {
    let dir = temp();
    let (result, output) = execute(
        &BashTool,
        json!({"command": "i=1; while [ $i -le 3000 ]; do echo line-$i; i=$((i + 1)); done"}),
        &ctx(dir.path()),
    )
    .await;
    let out = result.unwrap();
    let expected: String = (1..=3000).map(|i| format!("line-{i}\n")).collect();
    assert_eq!(output, expected);
    let path = spill_path(&out.diagnostics).expect("spill reported");
    assert_eq!(out.diagnostics[0].severity, Severity::Info);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let dir_mode = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(dir_mode & 0o777, 0o700);
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(
            name.starts_with("pi-output-") && name.ends_with(".log"),
            "{name}"
        );
    }
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[tokio::test]
async fn does_not_spill_output_within_the_limits() {
    let dir = temp();
    let (result, _) = execute(
        &BashTool,
        json!({"command": "printf small"}),
        &ctx(dir.path()),
    )
    .await;
    assert!(result.unwrap().diagnostics.is_empty());
}

#[tokio::test]
async fn reports_the_spill_of_a_command_that_times_out() {
    let dir = temp();
    let (result, _) = execute(
        &BashTool,
        json!({"command": "i=1; while [ $i -le 2001 ]; do echo line-$i; i=$((i + 1)); done; sleep 5", "timeout": 1}),
        &ctx(dir.path()),
    )
    .await;
    let error = result.unwrap_err();
    assert_eq!(error.message, "Command timed out after 1 seconds");
    let path = spill_path(&error.diagnostics).expect("spill reported");
    let full = std::fs::read_to_string(&path).unwrap();
    assert!(full.starts_with("line-1\nline-2\n"));
    assert!(full.ends_with("line-2000\nline-2001\n"));
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

/// More than 1 MiB on both pipes at once: both are drained concurrently, nothing deadlocks,
/// and every byte arrives.
#[tokio::test]
async fn drains_large_stdout_and_stderr_without_deadlock() {
    let dir = temp();
    let command = "head -c 1500000 /dev/zero | tr '\\0' 'o' & head -c 1500000 /dev/zero | tr '\\0' 'e' >&2; wait";
    let (result, output) = tokio::time::timeout(
        Duration::from_secs(20),
        execute(&BashTool, json!({"command": command}), &ctx(dir.path())),
    )
    .await
    .expect("no deadlock");
    let out = result.unwrap();
    assert_eq!(output.len(), 3_000_000);
    assert_eq!(output.bytes().filter(|b| *b == b'o').count(), 1_500_000);
    assert_eq!(output.bytes().filter(|b| *b == b'e').count(), 1_500_000);
    let path = spill_path(&out.diagnostics).unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 3_000_000);
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

/// Whether `pid` is still a live (not zombie) `sleep`.
#[cfg(target_os = "linux")]
fn alive(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        let Some((head, rest)) = stat.split_once(") ") else {
            return false;
        };
        head.ends_with("(sleep") && !rest.starts_with('Z') && !rest.starts_with('X')
    })
}

#[cfg(all(unix, not(target_os = "linux")))]
fn alive(pid: i32) -> bool {
    let output = std::process::Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "stat=", "-o", "comm="])
        .output()
        .expect("query the owned sleep process");
    if !output.status.success() {
        return false;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut fields = text.split_whitespace();
    let state = fields.next().unwrap_or_default();
    let command = fields.next().unwrap_or_default();
    !state.starts_with('Z')
        && !state.starts_with('X')
        && std::path::Path::new(command)
            .file_name()
            .is_some_and(|n| n == "sleep")
}

/// Cancellation kills the whole process group: a backgrounded grandchild `sleep` is gone, and
/// the call settles promptly as an aborted result with its partial output.
#[cfg(unix)]
#[tokio::test]
async fn cancellation_kills_the_process_group_including_descendants() {
    let dir = temp();
    let (c, _) = ctx_with(dir.path());
    let pid_file = dir.path().join("pid");
    let command = format!(
        "sleep 300 & echo $! > {}; echo started; wait",
        pid_file.display()
    );
    let call = tokio::spawn({
        let c = c.clone();
        async move { run(&BashTool, json!({"command": command}), &c).await }
    });
    let mut pid = None;
    for _ in 0..200 {
        if let Ok(text) = std::fs::read_to_string(&pid_file)
            && let Ok(p) = text.trim().parse::<i32>()
        {
            pid = Some(p);
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let pid = pid.expect("grandchild started");
    assert!(alive(pid));
    tokio::time::sleep(Duration::from_millis(50)).await;
    c.cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(5), call)
        .await
        .expect("settles promptly")
        .unwrap();
    assert!(result.is_error);
    assert_eq!(
        result_text(&result),
        "started\n|<harness>\n[error] Tool bash was aborted\n</harness>"
    );
    for _ in 0..100 {
        if !alive(pid) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        !alive(pid),
        "grandchild sleep {pid} survived: {:?}",
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
    );
}

/// A command that exits while a descendant keeps stdout open settles after the stdio grace.
#[tokio::test]
async fn settles_after_the_stdio_grace_when_a_descendant_holds_the_pipes() {
    let dir = temp();
    let started = std::time::Instant::now();
    let (result, output) = execute(
        &BashTool,
        json!({"command": "echo first; (sleep 3; echo late) &"}),
        &ctx(dir.path()),
    )
    .await;
    result.unwrap();
    assert_eq!(output, "first\n");
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn holds_a_process_permit_while_running() {
    let dir = temp();
    let mut c = ctx(dir.path());
    c.processes = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
    let held = c.processes.clone().acquire_owned().await.unwrap();
    let call = tokio::spawn({
        let c = c.clone();
        async move {
            execute(&BashTool, json!({"command": "touch ran"}), &c)
                .await
                .0
        }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!dir.path().join("ran").exists());
    drop(held);
    call.await.unwrap().unwrap();
    assert!(dir.path().join("ran").exists());
    assert_eq!(c.processes.available_permits(), 1);
}

// `harness-tools.test.ts` "coding tools"

#[tokio::test]
async fn answers_a_failing_command_with_its_retained_tail_and_diagnostics_in_order() {
    let dir = temp();
    let command = "i=1; while [ $i -le 3000 ]; do echo line-$i; i=$((i + 1)); done; exit 7";
    let result = run(&BashTool, json!({"command": command}), &ctx(dir.path())).await;
    assert!(result.is_error);
    let text = result_text(&result);
    assert!(text.starts_with("line-1001\n"), "{}", &text[..40]);
    assert!(text.contains("line-3000\n|<harness>\n[info] Full output: "));
    assert!(text.contains(
        "\n[error] Command exited with code 7\n[warn] Output truncated to its end: 1000 lines, "
    ));
    let expected: String = (1..=3000).map(|i| format!("line-{i}\n")).collect();
    assert_eq!(result.retained, expected);
    assert_eq!(result.dropped_bytes, 0);
    let path = text
        .split("[info] Full output: ")
        .nth(1)
        .and_then(|rest| rest.lines().next())
        .unwrap();
    std::fs::remove_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
}

#[tokio::test]
async fn reads_edits_and_runs_a_command_in_one_session() {
    let dir = temp();
    std::fs::write(dir.path().join("notes.txt"), "hello world\n").unwrap();
    let c = ctx(dir.path());
    let tools = crate::tools::builtin_tools();
    let read = run(tools[0].as_ref(), json!({"path": "notes.txt"}), &c).await;
    let edit = run(
        tools[2].as_ref(),
        json!({"path": "notes.txt", "edits": [{"oldText": "world", "newText": "durable"}]}),
        &c,
    )
    .await;
    let bash = run(tools[3].as_ref(), json!({"command": "cat notes.txt"}), &c).await;
    assert_eq!(
        [&read, &edit, &bash].map(|r| (r.is_error, result_text(r))),
        [
            (false, "hello world\n".to_owned()),
            (
                false,
                "Successfully replaced 1 block(s) in notes.txt.".to_owned()
            ),
            (false, "hello durable\n".to_owned()),
        ]
    );
}

#[tokio::test]
async fn publishes_throttled_live_progress_of_the_tail_window() {
    let dir = temp();
    let (c, seen) = ctx_with(dir.path());
    // One line, a pause well past the 100 ms minimum interval, then a burst of four lines
    // microseconds apart: the first line is published at once and on its own; of the
    // burst, the first chunk to arrive goes out at once (the interval has long passed) and
    // the rest wait for the interval and are published together. The command stays alive
    // after the burst: a publication still waiting for the interval when the call settles
    // is dropped (the result carries the whole output anyway).
    let result = run(
        &BashTool,
        json!({"command": "echo tick-1; sleep 0.3; echo tick-2; echo tick-3; echo tick-4; echo tick-5; sleep 0.3"}),
        &c,
    )
    .await;
    assert_eq!(
        result_text(&result),
        "tick-1\ntick-2\ntick-3\ntick-4\ntick-5\n"
    );
    let seen = seen.lock().unwrap().clone();
    assert!(!seen.is_empty());
    assert_eq!(seen[0], "tick-1\n");
    // Each publication is the whole window so far, never more than the final output.
    for window in &seen {
        assert!("tick-1\ntick-2\ntick-3\ntick-4\ntick-5\n".starts_with(window.as_str()));
    }
    // The burst took one or two publications (one when it arrived as a single chunk),
    // never one per line: the throttle coalesced it.
    assert!((2..=3).contains(&seen.len()), "{seen:?}");
}
