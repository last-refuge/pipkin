//! Process-level fault handling: qualification crash points, and reaping tool processes that
//! outlived an engine that was killed.

/// The environment variable that marks every process a tool call starts (and, by inheritance,
/// its descendants) with `<session>/<call id>`, so a later engine can find what an interrupted
/// call left running.
pub const TOOL_CALL_ENV: &str = "PIPKIN_TOOL_CALL";

/// The marker value for one call.
pub fn tool_call_token(session: &str, call_id: &str) -> String {
    format!("{session}/{call_id}")
}

/// Qualification crash points. When `PIPKIN_ENGINE_CRASH_POINT` names `point`, the engine kills
/// itself with SIGKILL right there: no unwinding, no orderly shutdown, nothing flushed. It lets a
/// test kill the real process between two durable steps that no outside observer can separate.
/// Unset (always, outside qualification), this is one cached environment lookup.
pub fn crash_point(point: &str) {
    static CONFIGURED: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    let configured = CONFIGURED.get_or_init(|| std::env::var("PIPKIN_ENGINE_CRASH_POINT").ok());
    if configured.as_deref() != Some(point) {
        return;
    }
    log::warn!("crash point {point}: killing the engine");
    #[cfg(unix)]
    // SAFETY: signalling our own process.
    unsafe {
        libc::kill(libc::getpid(), libc::SIGKILL);
    }
    std::process::abort();
}

/// Kill every process still carrying the marker of `token` (a tool call whose engine died
/// while it ran). The command's process group cannot be trusted after its leader died, and
/// its pid may since have been reused, so processes are matched by their marker instead.
/// Returns how many were signalled. Linux only; elsewhere nothing is found (the process
/// tree is not inspected).
pub fn kill_tool_processes(token: &str) -> usize {
    #[cfg(target_os = "linux")]
    {
        let needle = format!("{TOOL_CALL_ENV}={token}");
        let me = std::process::id();
        let mut killed = 0;
        // A dying shell may still be forking: repeat until a pass finds nothing.
        for _ in 0..20 {
            let found = marked_processes(needle.as_bytes(), me);
            if found.is_empty() {
                break;
            }
            for pid in found {
                // SAFETY: SIGKILL to a process that carries this call's marker.
                if unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) } == 0 {
                    killed += 1;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        killed
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = token;
        0
    }
}

#[cfg(target_os = "linux")]
fn marked_processes(needle: &[u8], me: u32) -> Vec<u32> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return vec![];
    };
    dir.filter_map(|e| e.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| *pid != me)
        .filter(|pid| {
            // Zombies and other users' processes read as empty or fail: not ours to kill.
            std::fs::read(format!("/proc/{pid}/environ"))
                .is_ok_and(|env| env.split(|b| *b == 0).any(|var| var == needle))
        })
        .collect()
}
