//! Controlled Phase 0 experiments, not production execution. Initialization controls
//! use a disposable home with no copied credentials. Opt-in live checks let the CLI
//! use/refresh its own login; the harness never writes real account/settings files.

use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::{
    AuthMethod, Environment, Options, auth_summary, base_spec, emit, isolation, permission,
    process, safety_args,
};

/// Current public PermissionRequest hook route. Safe mode intentionally cannot be used:
/// it disables this explicitly supplied hook too. Restricted mode excludes user/project/
/// local settings, while managed policy still applies. This policy must be qualified
/// independently of the already tested no-tools safe-mode route.
fn hook_policy_args(settings: serde_json::Value) -> Vec<OsString> {
    vec![
        "--mcp-config".into(),
        "{\"mcpServers\":{}}".into(),
        "--settings".into(),
        settings.to_string().into(),
        "--restricted".into(),
        "--strict-mcp-config".into(),
        "--disable-slash-commands".into(),
        "--no-chrome".into(),
    ]
}

pub async fn isolation_probe(
    executable: &Path,
    cancel: CancellationToken,
) -> Result<(), &'static str> {
    let helper = std::env::current_exe().map_err(|_| "cannot resolve qualification helper")?;
    let fixture = isolation::Fixture::new(&helper)?;
    let env = fixture.isolated_env();
    let mut observations = Vec::new();
    // A control must demonstrate that the CLI actually reads the fixture. Merely checking
    // that a marker is absent under a suppression flag would otherwise be a false pass.
    for (label, mut args) in [
        (
            "control",
            vec!["--setting-sources".into(), "user,project,local".into()],
        ),
        ("safe_policy", safety_args()),
        ("hook_policy", hook_policy_args(json!({}))),
    ] {
        fixture.clear_markers()?;
        args.extend([
            "--plugin-dir".into(),
            fixture.plugin.as_os_str().to_owned(),
            "--init-only".into(),
        ]);
        let mut spec = base_spec(executable, &fixture.project, &env, args);
        spec.timeout = Some(Duration::from_secs(25));
        let report = process::run(spec, cancel.clone()).await?;
        let observed = fixture.checked_observed()?;
        // The explicit test plugin is intentionally injected for safe-mode qualification.
        // The hook policy is NOT expected to exclude a plugin explicitly passed by a host.
        let expected = if label == "control" {
            ["user-hook", "project-hook", "local-hook", "plugin-hook"]
                .iter()
                .all(|name| observed.contains(name))
        } else if label == "safe_policy" {
            observed.is_empty()
        } else {
            !observed.iter().any(|name| {
                matches!(
                    *name,
                    "user-hook" | "project-hook" | "local-hook" | "project-mcp"
                )
            })
        };
        let passed = report.clean_exit() && expected;
        emit(&json!({"experiment":"startup_isolation", "case":label,
            "inference_requested":false, "credential_profile":"disposable_no_credentials",
            "observed_markers": observed, "passed":passed, "process": report}))?;
        observations.push((label, passed));
        if !passed {
            return Err("startup isolation control/policy failed; no live capability qualified");
        }
    }
    // mcp list is a public non-inference command. If it never launches the control server,
    // explicitly record the path as unexercised instead of claiming MCP isolation passed.
    let mut control_launched = false;
    let mut mcp_cases_passed = true;
    for (label, mut args) in [
        (
            "control",
            vec!["--setting-sources".into(), "user,project,local".into()],
        ),
        ("safe_policy", safety_args()),
        ("hook_policy", hook_policy_args(json!({}))),
    ] {
        fixture.clear_markers()?;
        args.extend(["mcp".into(), "list".into()]);
        let mut spec = base_spec(executable, &fixture.project, &env, args);
        spec.timeout = Some(Duration::from_secs(15));
        let report = process::run(spec, cancel.clone()).await?;
        let observed = fixture.checked_observed()?;
        let launched = observed.contains(&"project-mcp");
        if label == "control" {
            control_launched = report.clean_exit() && launched;
        }
        let pass = if label == "control" {
            control_launched
        } else {
            suppression_verified(control_launched, launched, report.clean_exit())
        };
        mcp_cases_passed &= pass;
        emit(
            &json!({"experiment":"mcp_discovery_isolation", "case":label,
            "inference_requested":false, "control_exercised":control_launched,
            "observed_markers":observed,"passed":pass,
            "suppression_observed":if label == "control" { None } else { Some(pass) },"process":report}),
        )?;
        if label != "control" && launched {
            return Err("project MCP sentinel launched under an isolation policy");
        }
    }
    emit(
        &json!({"experiment":"isolation_summary","startup_controls_passed":observations.iter().all(|(_, ok)|*ok),
        "project_mcp_control_exercised":control_launched,"mcp_cases_passed":mcp_cases_passed,"print_mode_isolation_verified":false,
        "managed_policy_isolation_verified":false,"billing_verified":false}),
    )?;
    if !mcp_cases_passed {
        return Err("MCP discovery/suppression was not established by clean controlled runs");
    }
    Ok(())
}

fn suppression_verified(control_launched: bool, launched: bool, clean_exit: bool) -> bool {
    control_launched && !launched && clean_exit
}

pub async fn permission_probe(
    executable: &Path,
    env: &Environment,
    options: &Options,
    cancel: CancellationToken,
) -> Result<(), &'static str> {
    let helper = std::env::current_exe().map_err(|_| "cannot resolve qualification helper")?;
    let fixture = isolation::Fixture::new(&helper)?;
    let root = &fixture.project;
    let session = options
        .session
        .as_deref()
        .ok_or("permission test requires session UUID")?;
    let policy = if options.mode == "permission-allow" {
        permission::Policy::AllowOnce
    } else {
        permission::Policy::Deny
    };
    let settings = permission::settings(&helper, root, session, policy);
    let spec = write_spec(executable, env, options, &fixture, settings, cancel.clone()).await?;
    let report = process::run(spec, cancel).await?;
    verify_permission(&fixture, session, policy, options, report)
}

/// Same authenticated, Write-only launch contract for permission and interruption checks.
pub(super) async fn write_spec(
    executable: &Path,
    env: &Environment,
    options: &Options,
    fixture: &isolation::Fixture,
    settings: serde_json::Value,
    cancel: CancellationToken,
) -> Result<process::Spec, &'static str> {
    let root = &fixture.project;
    let session = options.session.as_deref().ok_or("fresh session required")?;
    let base_args = hook_policy_args(settings);
    // Preserve the real CLI-owned account only here, after explicit usage consent. Never
    // copy credentials into the disposable HOME used by the non-inference controls.
    let mut auth_args = base_args.clone();
    auth_args.extend(["auth", "status", "--json"].into_iter().map(Into::into));
    let auth = process::run(base_spec(executable, root, env, auth_args), cancel.clone()).await?;
    if !auth.clean_exit() {
        emit(&auth)?;
        return Err("permission-route auth preflight failed; no model request attempted");
    }
    let account = auth_summary(&auth.captured)?;
    emit(&account)?;
    if account.method != AuthMethod::ClaudeAccount {
        return Err("permission test requires CLI-owned Claude-account login; no fallback");
    }
    if !fixture.checked_observed()?.is_empty() {
        return Err("fixture customization executed during preflight; refusing inference");
    }
    let mut args = base_args;
    args.extend(
        [
            "--print",
            "--tools",
            "Write",
            "--disallowedTools",
            "mcp__*",
            "--permission-mode",
            "default",
            "--permission-prompts",
            "none",
            "--input-format",
            "text",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--model",
            &options.model,
            "--session-id",
            session,
        ]
        .into_iter()
        .map(Into::into),
    );
    let mut spec = base_spec(executable, root, env, args);
    spec.mode = process::OutputMode::Stream;
    spec.timeout = Some(Duration::from_secs(60));
    spec.max_stdout = 16 * 1024 * 1024;
    let target = root.join(permission::TARGET);
    spec.input = format!(
        "Make exactly one Write tool call to create {} with exactly the text PIPKIN_PERMISSION_OK followed by a newline. Do not make any other tool call or retry if denied. Then finish your response.\n",
        target.display(),
    ).into_bytes();
    Ok(spec)
}

fn verify_permission(
    fixture: &isolation::Fixture,
    session: &str,
    policy: permission::Policy,
    options: &Options,
    report: process::Report,
) -> Result<(), &'static str> {
    let root = &fixture.project;
    let target = root.join(permission::TARGET);
    let permission::Observations {
        called,
        request_matched,
        denied,
        allowed,
        grant_used,
    } = permission::observations(root)?;
    let target_exists = path_exists(&target)?;
    // Bounded file verification: a mismatch or symlink is never accepted as a granted edit.
    let content_matches = exact_content(&target);
    let markers = fixture.checked_observed()?;
    let correlated = report
        .stream
        .as_ref()
        .is_some_and(|s| s.init_seen && s.session_id.as_deref() == Some(session));
    let inventory_verified = report
        .stream
        .as_ref()
        .is_some_and(|s| s.write_tool_available && s.unexpected_write_fixture_tools == Some(0));
    let process_settled = report.stop.is_none() && matches!(report.exit_code, Some(0 | 1));
    let decision_verified = match policy {
        permission::Policy::Deny => {
            called && request_matched && denied && !allowed && !grant_used && !target_exists
        }
        permission::Policy::AllowOnce => {
            called
                && request_matched
                && allowed
                && grant_used
                && !denied
                && content_matches
                && report.clean_exit()
                && report.stream.as_ref().is_some_and(|s| s.result_success)
        }
    };
    let passed = correlated
        && inventory_verified
        && process_settled
        && decision_verified
        && markers.is_empty();
    emit(
        &json!({"experiment":"permission_request_hook","case":options.mode,
        "decision_observed":called,"exact_request_matched":request_matched,"denied":denied,"allowed":allowed,"grant_used":grant_used,
        "target_exists":target_exists,"exact_content_matches":content_matches,
        "unexpected_customization_markers":markers,"write_only_inventory_verified":inventory_verified,"passed":passed,
        "billing_verified":false,"production_permission_ui_verified":false,"process":report}),
    )?;
    if !passed {
        return Err("permission qualification failed or uncertain; do not automatically retry");
    }
    Ok(())
}

pub(super) fn path_exists(path: &Path) -> Result<bool, &'static str> {
    match path.symlink_metadata() {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err("target state uncertain; cannot establish absence"),
    }
}

fn exact_content(path: &Path) -> bool {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let Ok(file) = options.open(path) else {
        return false;
    };
    if !file
        .metadata()
        .is_ok_and(|m| m.is_file() && m.len() == permission::CONTENT.len() as u64)
    {
        return false;
    }
    let mut bytes = Vec::new();
    file.take(permission::CONTENT.len() as u64 + 1)
        .read_to_end(&mut bytes)
        .is_ok()
        && bytes == permission::CONTENT.as_bytes()
}

/// Helpers are callable only as explicit standalone example subcommands. Read failures
/// produce a denial, never an exit-2 fallthrough which would leave PermissionRequest open.
pub fn helper(args: &[OsString]) -> Result<(), &'static str> {
    let mut root = None;
    let mut name = None;
    let mut session = None;
    let mut policy = None;
    let mut iter = args[1..].iter();
    while let Some(flag) = iter.next() {
        let value = iter.next().ok_or("missing fixture-helper argument")?;
        match flag.to_str() {
            Some("--root") if root.is_none() => root = Some(std::path::PathBuf::from(value)),
            Some("--name") if name.is_none() => name = value.to_str(),
            Some("--session-id") if session.is_none() => session = value.to_str(),
            Some("--policy") if policy.is_none() => policy = value.to_str(),
            _ => return Err("invalid fixture-helper arguments"),
        }
    }
    let root = root.ok_or("fixture root required")?;
    if args[0] == "fixture-mark" {
        if session.is_some() || policy.is_some() {
            return Err("unexpected marker arguments");
        }
        isolation::mark(&root, name.ok_or("fixture name required")?)
    } else {
        if name.is_some() {
            return Err("unexpected permission arguments");
        }
        let session = session
            .filter(|s| super::valid_uuid(s))
            .ok_or("fixture session UUID required")?;
        let policy = match policy {
            Some("deny") => permission::Policy::Deny,
            Some("allow-once") => permission::Policy::AllowOnce,
            _ => return Err("fixture permission policy required"),
        };
        let response = match bounded_hook_input() {
            Ok(input) => permission::decide(&input, &root, session, policy),
            Err(_) => permission::denial(),
        };
        emit(&response)
    }
}

#[cfg(unix)]
pub(super) fn bounded_hook_input() -> Result<Vec<u8>, &'static str> {
    read_input(0, Duration::from_secs(2))
}

#[cfg(unix)]
fn read_input(input_fd: i32, timeout: Duration) -> Result<Vec<u8>, &'static str> {
    let until = std::time::Instant::now() + timeout;
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let remaining = until.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err("hook input timeout");
        }
        let mut fd = libc::pollfd {
            fd: input_fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: initialized pollfd for our stdin, bounded timeout; no competing reader.
        let ready = unsafe { libc::poll(&mut fd, 1, remaining.as_millis().min(100) as i32) };
        if ready < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err("hook input failure");
        }
        if ready == 0 {
            continue;
        }
        // SAFETY: writable chunk with its actual length; poll signaled input/EOF/error.
        let n = unsafe { libc::read(input_fd, chunk.as_mut_ptr().cast(), chunk.len()) };
        if n < 0 {
            return Err("hook input failure");
        }
        if n == 0 {
            return Ok(bytes);
        }
        if bytes.len() + n as usize > 64 * 1024 {
            return Err("hook input limit");
        }
        bytes.extend_from_slice(&chunk[..n as usize]);
    }
}

#[cfg(not(unix))]
pub(super) fn bounded_hook_input() -> Result<Vec<u8>, &'static str> {
    Err("permission fixture currently supports Unix only")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn hook_reader_bounds_input_and_waiting_without_retaining_errors() {
        use std::io::Write;
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        fn pipes() -> (OwnedFd, std::fs::File) {
            let mut fds = [0; 2];
            // SAFETY: writable two-fd array; we take ownership only on success.
            assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
            unsafe {
                (
                    OwnedFd::from_raw_fd(fds[0]),
                    std::fs::File::from_raw_fd(fds[1]),
                )
            }
        }
        let (read, mut write) = pipes();
        write.write_all(b"{\"test\":true}").unwrap();
        drop(write);
        assert_eq!(
            read_input(read.as_raw_fd(), Duration::from_secs(1)).unwrap(),
            b"{\"test\":true}"
        );
        let (read, _held_open) = pipes();
        assert_eq!(
            read_input(read.as_raw_fd(), Duration::from_millis(10)),
            Err("hook input timeout")
        );
        let (read, mut write) = pipes();
        let writer = std::thread::spawn(move || {
            let _ = write.write_all(&vec![b'x'; 65537]);
        });
        assert_eq!(
            read_input(read.as_raw_fd(), Duration::from_secs(2)),
            Err("hook input limit")
        );
        drop(read);
        writer.join().unwrap();
    }

    #[test]
    fn failed_controls_or_invocations_cannot_prove_suppression() {
        assert!(suppression_verified(true, false, true));
        assert!(!suppression_verified(true, false, false));
        assert!(!suppression_verified(false, false, true));
        assert!(!suppression_verified(true, true, true));
    }

    #[test]
    fn target_absence_requires_not_found_not_other_metadata_errors() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(path_exists(&dir.path().join("missing")), Ok(false));
        let file = dir.path().join("file");
        std::fs::write(&file, b"existing").unwrap();
        assert_eq!(path_exists(&file), Ok(true));
        assert!(path_exists(&file.join("child")).is_err());
    }

    #[test]
    fn hook_route_does_not_silently_reuse_safe_or_bare_mode() {
        let args = hook_policy_args(json!({"hooks":{}}));
        for flag in [
            "--bare",
            "--safe-mode",
            "--dangerously-skip-permissions",
            "--allowedTools",
        ] {
            assert!(!args.contains(&OsString::from(flag)));
        }
        assert!(args.contains(&"--restricted".into()));
        assert!(args.contains(&"--strict-mcp-config".into()));
    }
}
