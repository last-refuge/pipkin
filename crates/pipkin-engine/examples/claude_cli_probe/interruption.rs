//! Phase-0 pending-approval interruption, not a production permission UI.
//! A private SQLite file is the fixture's broker boundary. It contains no prompts or
//! credentials. Same-user hostile filesystem mutation is NOT contained by this test.

use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::lifecycle::{ApprovalState, Ledger, Phase, Ticket};
use super::{Environment, Options, emit, isolation, permission, process, qualification};

const DATABASE: &str = "lifecycle.sqlite3";
const APPROVAL_TTL_MS: u64 = 15_000;

// One fresh CLI session/run per qualification, not a production conversation identifier.
pub(super) fn ticket(session: &str) -> Ticket {
    Ticket {
        session: session.into(),
        generation: 1,
        run: 1,
    }
}

#[cfg(unix)]
fn now_ms() -> Result<u64, &'static str> {
    // Same nondecreasing clock domain in parent and hook; Linux BOOTTIME includes
    // suspend. Restart recovery revokes all active grants before accepting work.
    #[cfg(target_os = "linux")]
    let clock = libc::CLOCK_BOOTTIME;
    #[cfg(not(target_os = "linux"))]
    let clock = libc::CLOCK_MONOTONIC;
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: initialized writable timespec and a supported local clock identifier.
    if unsafe { libc::clock_gettime(clock, &mut ts) } != 0 || ts.tv_sec < 0 || ts.tv_nsec < 0 {
        return Err("approval clock unavailable");
    }
    u64::try_from(ts.tv_sec)
        .ok()
        .and_then(|s| s.checked_mul(1000))
        .and_then(|ms| ms.checked_add(ts.tv_nsec as u64 / 1_000_000))
        .ok_or("approval clock unavailable")
}

#[cfg(not(unix))]
fn now_ms() -> Result<u64, &'static str> {
    Err("approval clock requires Unix")
}

fn request_id() -> Result<String, &'static str> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|_| "request identity unavailable")?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

fn fingerprint(root: &Path, session: &str) -> Result<String, &'static str> {
    // Canonical expected arguments, not arbitrary CLI JSON ordering or extra fields.
    let value = json!([
        session,
        root,
        "Write",
        root.join(permission::TARGET),
        permission::CONTENT
    ]);
    let bytes = serde_json::to_vec(&value).map_err(|_| "request fingerprint unavailable")?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn settings(
    helper: &Path,
    root: &Path,
    session: &str,
    request: &str,
) -> Result<Value, &'static str> {
    let quoted = |path: &Path| {
        path.to_str()
            .and_then(permission::shell_word)
            .ok_or("invalid helper path")
    };
    let helper = quoted(helper)?;
    let root = quoted(root)?;
    if !super::valid_uuid(session) || !super::valid_uuid(request) {
        return Err("invalid fixture identity");
    }
    Ok(json!({
        "permissions": {"ask": ["Write"]},
        "hooks": {"PermissionRequest": [{"matcher": "Write", "hooks": [{
            "type": "command",
            "command": format!("{helper} fixture-interrupt --root {root} --session-id {session} --request-id {request}"),
            "timeout": 20, "onFailure": "block"
        }]}]}
    }))
}

/// The hook owns no policy: it waits for the parent, then consumes an exact one-shot
/// decision before returning allow. Cancellation cannot undo an already consumed grant.
/// The live interruption experiment NEVER grants; the allow branch has offline controls.
fn wait_for_decision(input: &[u8], root: &Path, session: &str, request: &str) -> Value {
    wait_with_ttl(input, root, session, request, APPROVAL_TTL_MS)
}

fn wait_with_ttl(input: &[u8], root: &Path, session: &str, request: &str, ttl_ms: u64) -> Value {
    let decide = || -> Result<Value, &'static str> {
        if !permission::matches_request(input, root, session) {
            return Err("request did not match scratch fixture");
        }
        let path = root
            .parent()
            .ok_or("fixture parent missing")?
            .join(DATABASE);
        let mut ledger = Ledger::open(&path)?;
        let ticket = ticket(session);
        let fingerprint = fingerprint(root, session)?;
        ledger.request(&ticket, request, &fingerprint, now_ms()?, ttl_ms)?;
        // Independent bounded wait even if no host remains to answer.
        let deadline = std::time::Instant::now() + Duration::from_millis(ttl_ms);
        while std::time::Instant::now() < deadline {
            match ledger.approval(&ticket, request, now_ms()?)? {
                ApprovalState::Pending => std::thread::sleep(Duration::from_millis(10)),
                ApprovalState::Granted => {
                    if ledger.consume(&ticket, request, &fingerprint, now_ms()?)? {
                        return Ok(permission::decide(
                            input,
                            root,
                            session,
                            permission::Policy::AllowOnce,
                        ));
                    }
                    break;
                }
                _ => break,
            }
        }
        Ok(permission::denial())
    };
    decide().unwrap_or_else(|_| permission::denial())
}

pub(super) fn helper(args: &[OsString]) -> Result<(), &'static str> {
    let mut root = None;
    let mut session = None;
    let mut request = None;
    let mut args = args[1..].iter();
    while let Some(flag) = args.next() {
        let value = args.next().ok_or("missing interruption-helper value")?;
        match flag.to_str() {
            Some("--root") if root.is_none() => root = Some(std::path::PathBuf::from(value)),
            Some("--session-id") if session.is_none() => session = value.to_str(),
            Some("--request-id") if request.is_none() => request = value.to_str(),
            _ => return Err("invalid interruption-helper argument"),
        }
    }
    let root = root.ok_or("missing scratch root")?;
    let session = session
        .filter(|v| super::valid_uuid(v))
        .ok_or("invalid session identity")?;
    let request = request
        .filter(|v| super::valid_uuid(v))
        .ok_or("invalid request identity")?;
    let response = match qualification::bounded_hook_input() {
        Ok(input) => wait_for_decision(&input, &root, session, request),
        Err(_) => permission::denial(),
    };
    emit(&response)
}

struct InterruptedRun {
    report: process::Report,
    pending_observed: bool,
    late_approval_rejected: bool,
    externally_cancelled: bool,
}

/// Commit launch intent before spawning. Keep supervising even if ledger I/O fails;
/// never publish cancellation just because a token was set or a DB row was changed.
async fn drive(
    spec: process::Spec,
    ledger: &mut Ledger,
    path: &Path,
    ticket: &Ticket,
    request: &str,
    fingerprint: &str,
    cancel: CancellationToken,
) -> Result<InterruptedRun, &'static str> {
    let mut fence_ledger = Ledger::open(path)?;
    let fence_ticket = ticket.clone();
    ledger.launch_intent(ticket)?;
    // Running means execution is now permitted/may have begun, NOT remote acknowledgement.
    ledger.running(ticket)?;
    // Do not inherit cancellation: only this owner may release cancellation after
    // revocation. The supervisor fence covers its independent failure/cleanup paths.
    let stop = CancellationToken::new();
    let child = process::run_fenced(
        spec,
        stop.clone(),
        Some(Box::new(move || {
            fence_ledger.request_stop(&fence_ticket).is_ok()
        })),
    );
    tokio::pin!(child);
    let mut poll = tokio::time::interval(Duration::from_millis(10));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut pending_observed = false;
    let mut late_approval_rejected = false;
    let mut externally_cancelled = false;
    let mut stopping = false;
    let mut monitor_error = None;
    let result = loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled(), if !stopping => {
                externally_cancelled = true;
                stopping = true;
                if let Err(error) = ledger.request_stop(ticket) { monitor_error = Some(error); }
                stop.cancel();
            }
            result = &mut child => break result,
            _ = poll.tick(), if !stopping => {
                let mut observe = || -> Result<bool, &'static str> {
                    // Missing is distinct from storage failure: errors cancel and fail qualification.
                    Ok(ledger.find_approval(ticket, request, now_ms()?)? == Some(ApprovalState::Pending))
                };
                match observe() {
                    Ok(false) => {},
                    Ok(true) => {
                        pending_observed = true;
                        stopping = true;
                        let mut revoke = || -> Result<bool, &'static str> {
                            ledger.request_stop(ticket)?;
                            let now = now_ms()?;
                            Ok(!ledger.decide(ticket, request, fingerprint, true, now)?
                                && !ledger.consume(ticket, request, fingerprint, now)?)
                        };
                        match revoke() {
                            Ok(rejected) => late_approval_rejected = rejected,
                            Err(error) => monitor_error = Some(error),
                        }
                        stop.cancel();
                    }
                    Err(error) => { monitor_error = Some(error); stopping = true; stop.cancel(); }
                }
            }
        }
    };
    let report = match result {
        Ok(report) => report,
        Err(error) => {
            ledger.unknown(ticket)?;
            return Err(error);
        }
    };
    if monitor_error.is_none()
        && stopping
        && report.stop == Some(process::Stop::Cancelled)
        && report.root_reaped
        && report.pipes_drained
        && !report.cleanup_timed_out
        && report.approval_revocation_confirmed == Some(true)
    {
        ledger.interrupted(ticket)?;
    } else {
        ledger.unknown(ticket)?;
    }
    if let Some(error) = monitor_error {
        return Err(error);
    }
    Ok(InterruptedRun {
        report,
        pending_observed,
        late_approval_rejected,
        externally_cancelled,
    })
}

pub(super) async fn probe(
    executable: &Path,
    env: &Environment,
    options: &Options,
    cancel: CancellationToken,
) -> Result<(), &'static str> {
    let helper = std::env::current_exe().map_err(|_| "cannot resolve qualification helper")?;
    let fixture = isolation::Fixture::new(&helper)?;
    let root = &fixture.project;
    let session = options.session.as_deref().ok_or("fresh session required")?;
    let request = request_id()?;
    let fingerprint = fingerprint(root, session)?;
    let ticket = ticket(session);
    let path = fixture.root().join(DATABASE);
    let mut ledger = Ledger::create(&path)?;
    ledger.admit(&ticket)?;
    let settings = settings(&helper, root, session, &request)?;
    let spec =
        qualification::write_spec(executable, env, options, &fixture, settings, cancel.clone())
            .await?;
    let outcome = drive(
        spec,
        &mut ledger,
        &path,
        &ticket,
        &request,
        &fingerprint,
        cancel,
    )
    .await?;
    drop(ledger);
    let mut recovered = Ledger::open(&path)?;
    let recovered_active_runs = recovered.recover()?;
    let snapshot = recovered.snapshot(&ticket)?;
    let continuation_blocked = recovered
        .admit(&Ticket {
            run: 2,
            ..ticket.clone()
        })
        .is_err();
    let late_completion_blocked = recovered.complete(&ticket).is_err();
    let target_exists = qualification::path_exists(&root.join(permission::TARGET))?;
    let observed = permission::observations(root)?;
    let markers = fixture.checked_observed()?;
    let inventory_correlated = outcome.report.stream.as_ref().is_some_and(|s| {
        s.init_seen
            && s.session_id.as_deref() == Some(session)
            && s.write_tool_available
            && s.unexpected_write_fixture_tools == Some(0)
    });
    let passed = outcome.pending_observed
        && outcome.late_approval_rejected
        && !outcome.externally_cancelled
        && outcome.report.stop == Some(process::Stop::Cancelled)
        && outcome.report.root_reaped
        && outcome.report.pipes_drained
        && !outcome.report.cleanup_timed_out
        && outcome.report.approval_revocation_confirmed == Some(true)
        && inventory_correlated
        && snapshot.phase == Phase::Interrupted
        && !snapshot.context_valid
        && !snapshot.resume_allowed
        && !snapshot.queue_allowed
        && continuation_blocked
        && late_completion_blocked
        && recovered_active_runs == 0
        && !target_exists
        && !observed.allowed
        && !observed.grant_used
        && markers.is_empty();
    emit(
        &json!({"experiment":"pending_approval_interruption", "passed":passed,
        "pending_observed":outcome.pending_observed,"late_approval_rejected":outcome.late_approval_rejected,
        "target_exists":target_exists,"grant_used":observed.grant_used,
        "inventory_correlated":inventory_correlated,"unsafe_continuation_blocked":continuation_blocked,
        "late_completion_blocked":late_completion_blocked,
        "reopened_state":snapshot,"unexpected_customization_markers":markers,
        "billing_verified":false,"production_permission_ui_verified":false,
        "process":outcome.report}),
    )?;
    if !passed {
        return Err("interruption not qualified; no automatic retry or resume");
    }
    Ok(())
}

#[cfg(all(test, unix))]
#[path = "interruption_tests.rs"]
mod tests;
