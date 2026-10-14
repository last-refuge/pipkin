//! Phase-0 scripted permission for one explicitly consented scratch Write only.
//! Not a production approval mechanism, UI prompt, or MCP permission server.
//! The parent owns the private scratch directory and must not reuse its markers.
//! The parent observes `.permission-called`, `.permission-denied`, and
//! `.permission-allowed` (fixed-text regular files); both outcomes may exist after
//! repeated requests. `.permission-grant-used` permanently consumes the one grant.
//! Markers record hook decisions, not proof that Claude executed the Write.

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;

use serde_json::{Value, json};

pub const CONTENT: &str = "PIPKIN_PERMISSION_OK\n";
pub const TARGET: &str = "permission-target.txt";

const MAX_INPUT: usize = 64 * 1024;
const CALLED: &str = ".permission-called";
const MATCHED: &str = ".permission-request-matched";
const DENIED: &str = ".permission-denied";
const ALLOWED: &str = ".permission-allowed";
const GRANT: &str = ".permission-grant-used";
// Intentionally contains no request, session, path, or error details.
const MARKER: &[u8] = b"PIPKIN_PERMISSION_OBSERVED\n";

#[derive(Clone, Copy)]
pub enum Policy {
    Deny,
    AllowOnce,
}

/// POSIX command hook for the Unix-only probe. Invalid command arguments block.
/// `onFailure` requires Claude Code 2.1.295+; it belongs on the hook entry.
pub fn settings(helper: &Path, root: &Path, session: &str, policy: Policy) -> Value {
    let command = (|| {
        if !helper.is_absolute() || !root.is_absolute() {
            return None;
        }
        let helper = shell_word(helper.to_str()?)?;
        let root = shell_word(root.to_str()?)?;
        let session = shell_word(session)?;
        let policy = match policy {
            Policy::Deny => "deny",
            Policy::AllowOnce => "allow-once",
        };
        Some(format!(
            "{helper} fixture-permission --root {root} --session-id {session} --policy {policy}"
        ))
    })()
    .unwrap_or_else(|| "exit 1".into());
    json!({
        "permissions": {"ask": ["Write"]},
        "hooks": {"PermissionRequest": [{
            "matcher": "Write",
            "hooks": [{
                "type": "command",
                "command": command,
                "timeout": 5,
                "onFailure": "block"
            }]
        }]}
    })
}

pub(super) fn shell_word(s: &str) -> Option<String> {
    if s.contains('\0') {
        return None;
    }
    Some(format!("'{}'", s.replace('\'', "'\\''")))
}

/// Also used by the parent's bounded stdin reader on malformed input/read timeout.
pub fn denial() -> Value {
    json!({"hookSpecificOutput": {
        "hookEventName": "PermissionRequest",
        "decision": {
            "behavior": "deny",
            "message": "Phase 0 scratch permission denied.",
            "interrupt": true
        }
    }})
}

/// Decide without modifying tool input or granting any persistent permission rule.
/// Root and its ancestors are parent-owned: this is not a hostile-filesystem sandbox.
fn correlated_request(
    input: &[u8],
    root: &Path,
    session: &str,
) -> Option<(serde_json::Map<String, Value>, std::path::PathBuf)> {
    if input.len() > MAX_INPUT {
        return None;
    }
    let Value::Object(request) = serde_json::from_slice::<Value>(input).ok()? else {
        return None;
    };
    if request.get("session_id").and_then(Value::as_str) != Some(session)
        || request.get("hook_event_name").and_then(Value::as_str) != Some("PermissionRequest")
    {
        return None;
    }
    // Reject a symlink root, missing root, and non-directory root. Resolve cwd only
    // for correlation; never use an untrusted cwd as the destination for markers.
    if !root.is_absolute() || !fs::symlink_metadata(root).is_ok_and(|m| m.file_type().is_dir()) {
        return None;
    }
    let canonical_root = root.canonicalize().ok()?;
    let cwd = request.get("cwd").and_then(Value::as_str)?;
    if !Path::new(cwd).is_absolute()
        || !Path::new(cwd)
            .canonicalize()
            .is_ok_and(|cwd| cwd == canonical_root)
    {
        return None;
    }
    Some((request, canonical_root))
}

fn exact_write(request: &serde_json::Map<String, Value>, root: &Path) -> bool {
    let target = root.join(TARGET);
    request.get("tool_name").and_then(Value::as_str) == Some("Write")
        && request
            .get("tool_input")
            .and_then(Value::as_object)
            .is_some_and(|tool| {
                tool.len() == 2
                    && target.to_str().is_some_and(|target| {
                        tool.get("file_path").and_then(Value::as_str) == Some(target)
                    })
                    && tool.get("content").and_then(Value::as_str) == Some(CONTENT)
            })
}

/// Read-only validation shared with the durable pending-approval fixture. No audit/grant effects.
pub(super) fn matches_request(input: &[u8], root: &Path, session: &str) -> bool {
    correlated_request(input, root, session).is_some_and(|(request, _)| exact_write(&request, root))
}

pub fn decide(input: &[u8], root: &Path, session: &str, policy: Policy) -> Value {
    let Some((request, canonical_root)) = correlated_request(input, root, session) else {
        return denial();
    };
    let audit_root = canonical_root.as_path();
    // Only correlated requests can leave observations. Existing audit markers
    // must be safe regular files containing exactly our fixed text.
    if observe(audit_root, CALLED).is_err() {
        return deny_observed(audit_root);
    }
    if !exact_write(&request, root) {
        return deny_observed(audit_root);
    }
    // A rejected malformed/different request must not qualify the intended Write test.
    if observe(audit_root, MATCHED).is_err() || matches!(policy, Policy::Deny) {
        return deny_observed(audit_root);
    }
    // Inspect even the unused denial marker: a symlink/foreign marker must never
    // coexist with an allow. NotFound alone proves absence (including dangling links).
    if !matches!(marker_exists(audit_root, DENIED), Ok(false))
        || !matches!(marker_exists(audit_root, ALLOWED), Ok(false))
        || !absent(&audit_root.join(TARGET))
        || create_marker(audit_root, GRANT).is_err()
    {
        return deny_observed(audit_root);
    }
    // The exclusive grant is never removed, even if subsequent I/O fails. This
    // also serializes competing hook processes; no failed attempt is retried.
    if !absent(&audit_root.join(TARGET)) || create_marker(audit_root, ALLOWED).is_err() {
        return deny_observed(audit_root);
    }
    json!({"hookSpecificOutput": {
        "hookEventName": "PermissionRequest",
        "decision": {"behavior": "allow"}
    }})
}

pub struct Observations {
    pub called: bool,
    pub request_matched: bool,
    pub denied: bool,
    pub allowed: bool,
    pub grant_used: bool,
}

/// Parent-side evidence must validate marker contents, not merely follow is_file().
pub fn observations(root: &Path) -> Result<Observations, &'static str> {
    Ok(Observations {
        called: marker_exists(root, CALLED).map_err(|_| "invalid permission evidence")?,
        request_matched: marker_exists(root, MATCHED).map_err(|_| "invalid permission evidence")?,
        denied: marker_exists(root, DENIED).map_err(|_| "invalid permission evidence")?,
        allowed: marker_exists(root, ALLOWED).map_err(|_| "invalid permission evidence")?,
        grant_used: marker_exists(root, GRANT).map_err(|_| "invalid permission evidence")?,
    })
}

fn absent(path: &Path) -> bool {
    matches!(fs::symlink_metadata(path), Err(e) if e.kind() == io::ErrorKind::NotFound)
}

fn deny_observed(root: &Path) -> Value {
    // An audit failure never changes a denial into an allow. Do not overwrite or
    // remove an unsafe marker, and do not expose the OS error or request.
    let _ = observe(root, DENIED);
    denial()
}

fn create_marker(root: &Path, name: &str) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // create_new rejects every existing entry, including dangling symlinks.
    let mut file = options.open(root.join(name))?;
    file.write_all(MARKER)?;
    file.sync_all()
}

fn observe(root: &Path, name: &str) -> io::Result<()> {
    match create_marker(root, name) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            if marker_exists(root, name)? {
                Ok(())
            } else {
                Err(io::ErrorKind::NotFound.into())
            }
        }
        Err(e) => Err(e),
    }
}

fn marker_exists(root: &Path, name: &str) -> io::Result<bool> {
    let path = root.join(name);
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Ok(m) if m.file_type().is_file() => {}
        _ => return Err(io::ErrorKind::PermissionDenied.into()),
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // No following a raced symlink or blocking on a raced FIFO.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    // This fixture runs on Unix; do not silently weaken existing-marker checks.
    if !cfg!(unix) {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let mut file = options.open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() != MARKER.len() as u64 {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() != 1 {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
    }
    let mut text = [0; MARKER.len()];
    file.read_exact(&mut text)?;
    if text != MARKER {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: &str = "11111111-1111-4111-8111-111111111111";

    fn request(root: &Path) -> Value {
        json!({
            "session_id": SESSION,
            "cwd": root,
            "hook_event_name": "PermissionRequest",
            "tool_name": "Write",
            "tool_input": {"file_path": root.join(TARGET), "content": CONTENT}
        })
    }

    fn run(value: &Value, root: &Path, policy: Policy) -> Value {
        decide(&serde_json::to_vec(value).unwrap(), root, SESSION, policy)
    }

    fn allow() -> Value {
        json!({"hookSpecificOutput": {
            "hookEventName": "PermissionRequest", "decision": {"behavior": "allow"}
        }})
    }

    fn assert_marker(root: &Path, name: &str) {
        assert_eq!(fs::read(root.join(name)).unwrap(), MARKER);
    }

    #[test]
    fn settings_force_write_ask_and_block_at_exact_hook_entry_level() {
        for (policy, flag) in [(Policy::Deny, "deny"), (Policy::AllowOnce, "allow-once")] {
            let settings = settings(
                Path::new("/tmp/helper's file"),
                Path::new("/tmp/scratch $(false)"),
                SESSION,
                policy,
            );
            assert_eq!(
                settings,
                json!({
                    "permissions": {"ask": ["Write"]},
                    "hooks": {"PermissionRequest": [{"matcher": "Write", "hooks": [{
                        "type": "command",
                        "command": format!("'/tmp/helper'\\''s file' fixture-permission --root '/tmp/scratch $(false)' --session-id '{SESSION}' --policy {flag}"),
                        "timeout": 5, "onFailure": "block"
                    }]}]}
                })
            );
        }
        assert_eq!(shell_word("a\0b"), None);
        assert_eq!(
            settings(
                Path::new("relative"),
                Path::new("/tmp"),
                SESSION,
                Policy::Deny
            )["hooks"]["PermissionRequest"][0]["hooks"][0]["command"],
            "exit 1"
        );
    }

    #[test]
    fn a_prior_denial_prevents_a_later_allow_and_evidence_is_validated() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let input = request(root);
        assert_eq!(run(&input, root, Policy::Deny), denial());
        assert_eq!(run(&input, root, Policy::AllowOnce), denial());
        let markers = observations(root).unwrap();
        assert!(markers.called && markers.denied && !markers.allowed && !markers.grant_used);
        fs::write(root.join(ALLOWED), b"forged").unwrap();
        assert!(observations(root).is_err());
    }

    #[test]
    fn a_denied_mismatched_write_does_not_qualify_the_intended_request() {
        let dir = tempfile::tempdir().unwrap();
        let mut input = request(dir.path());
        input["tool_input"]["content"] = json!("wrong text");
        assert_eq!(run(&input, dir.path(), Policy::Deny), denial());
        let observed = observations(dir.path()).unwrap();
        assert!(observed.called && observed.denied);
        assert!(!observed.request_matched);
        assert_eq!(
            run(&request(dir.path()), dir.path(), Policy::Deny),
            denial()
        );
        assert!(observations(dir.path()).unwrap().request_matched);
    }

    #[test]
    fn standalone_denial_has_exact_documented_shape() {
        assert_eq!(
            denial(),
            json!({"hookSpecificOutput": {
                "hookEventName": "PermissionRequest",
                "decision": {
                    "behavior": "deny",
                    "message": "Phase 0 scratch permission denied.",
                    "interrupt": true
                }
            }})
        );
    }

    #[test]
    fn exact_deny_and_allow_once_without_tool_use_id_or_mutation() {
        for policy in [Policy::Deny, Policy::AllowOnce] {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let input = request(root);
            let original = input.clone();
            let output = run(&input, root, policy);
            assert_eq!(input, original);
            assert_marker(root, CALLED);
            match policy {
                Policy::Deny => {
                    assert_eq!(output, denial());
                    assert_marker(root, DENIED);
                    assert!(absent(&root.join(GRANT)));
                    assert!(absent(&root.join(ALLOWED)));
                }
                Policy::AllowOnce => {
                    assert_eq!(output, allow());
                    assert_marker(root, GRANT);
                    assert_marker(root, ALLOWED);
                    assert!(absent(&root.join(DENIED)));
                    assert_eq!(run(&input, root, policy), denial());
                    assert_marker(root, DENIED);
                    // Removing an audit marker cannot reset the exclusive grant.
                    fs::remove_file(root.join(ALLOWED)).unwrap();
                    assert_eq!(run(&input, root, policy), denial());
                }
            }
            assert!(absent(&root.join(TARGET)));
        }
    }

    #[test]
    fn uncorrelated_requests_leave_no_observations() {
        let other = tempfile::tempdir().unwrap();
        for (field, value) in [
            ("session_id", json!("wrong-session")),
            ("session_id", Value::Null),
            ("cwd", json!(other.path())),
            ("cwd", json!("relative")),
            ("cwd", json!(other.path().join("missing"))),
            ("hook_event_name", json!("PreToolUse")),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut input = request(dir.path());
            input[field] = value;
            assert_eq!(run(&input, dir.path(), Policy::AllowOnce), denial());
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
        }
    }

    #[test]
    fn wrong_tool_path_content_types_and_extra_input_fields_deny() {
        for case in 0..13 {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let mut input = request(root);
            match case {
                0 => input["tool_name"] = json!("Edit"),
                1 => input["tool_name"] = json!("mcp__fixture__Write"),
                2 => input["tool_input"]["file_path"] = json!(TARGET),
                3 => input["tool_input"]["file_path"] = json!(root.join("other.txt")),
                4 => {
                    input["tool_input"]["file_path"] =
                        json!(format!("{}/./{TARGET}", root.display()))
                }
                5 => input["tool_input"]["content"] = json!("wrong-content"),
                6 => input["tool_input"]["content"] = json!(false),
                7 => input["tool_input"]["extra"] = json!(true),
                8 => input["tool_input"]["updatedPermissions"] = json!([]),
                9 => input["tool_input"] = json!([]),
                10 => {
                    input["tool_input"]
                        .as_object_mut()
                        .unwrap()
                        .remove("content");
                }
                11 => {
                    input.as_object_mut().unwrap().remove("tool_input");
                }
                _ => input["tool_input"]["file_path"] = Value::Null,
            }
            assert_eq!(run(&input, root, Policy::AllowOnce), denial());
            assert_marker(root, CALLED);
            assert_marker(root, DENIED);
            assert!(absent(&root.join(GRANT)));
            assert!(absent(&root.join(ALLOWED)));
        }
    }

    #[test]
    fn malformed_nonobject_and_oversized_input_deny_without_markers() {
        let dir = tempfile::tempdir().unwrap();
        for bytes in [
            b"".as_slice(),
            b"{",
            b"[]",
            b"null",
            b"true",
            b"\xff",
            b"{} trailing",
        ] {
            assert_eq!(
                decide(bytes, dir.path(), SESSION, Policy::AllowOnce),
                denial()
            );
        }
        let mut bytes = serde_json::to_vec(&request(dir.path())).unwrap();
        bytes.resize(MAX_INPUT + 1, b' ');
        assert_eq!(
            decide(&bytes, dir.path(), SESSION, Policy::AllowOnce),
            denial()
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
        bytes.truncate(MAX_INPUT);
        assert_eq!(
            decide(&bytes, dir.path(), SESSION, Policy::AllowOnce),
            allow()
        );
    }

    #[test]
    fn existing_target_and_unsafe_marker_entries_fail_closed() {
        for name in [TARGET, CALLED, DENIED, ALLOWED, GRANT] {
            for directory in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join(name);
                if directory {
                    fs::create_dir(&path).unwrap();
                } else {
                    fs::write(&path, b"foreign marker").unwrap();
                }
                assert_eq!(
                    run(&request(dir.path()), dir.path(), Policy::AllowOnce),
                    denial()
                );
                assert!(absent(&dir.path().join(ALLOWED)) || name == ALLOWED);
                if !directory {
                    assert_eq!(fs::read(path).unwrap(), b"foreign marker");
                }
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        assert_eq!(
            run(&request(&missing), &missing, Policy::AllowOnce),
            denial()
        );
        assert!(absent(&missing));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_including_dangling_links_are_never_followed() {
        use std::os::unix::fs::symlink;
        for name in [TARGET, CALLED, DENIED, ALLOWED, GRANT] {
            for dangling in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let outside = tempfile::tempdir().unwrap();
                let destination = outside.path().join("sentinel");
                if !dangling {
                    fs::write(&destination, b"untouched").unwrap();
                }
                symlink(&destination, dir.path().join(name)).unwrap();
                assert_eq!(
                    run(&request(dir.path()), dir.path(), Policy::AllowOnce),
                    denial()
                );
                if dangling {
                    assert!(absent(&destination));
                } else {
                    assert_eq!(fs::read(&destination).unwrap(), b"untouched");
                }
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let alias_dir = tempfile::tempdir().unwrap();
        let alias = alias_dir.path().join("root");
        symlink(dir.path(), &alias).unwrap();
        assert_eq!(run(&request(&alias), &alias, Policy::AllowOnce), denial());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
        // cwd correlation is canonical, unlike the exact tool file_path check.
        let mut input = request(dir.path());
        input["cwd"] = json!(alias);
        assert_eq!(run(&input, dir.path(), Policy::AllowOnce), allow());
    }

    #[cfg(unix)]
    #[test]
    fn hardlinked_audit_marker_is_unsafe_even_with_correct_text() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let sentinel = outside.path().join("sentinel");
        fs::write(&sentinel, MARKER).unwrap();
        fs::hard_link(&sentinel, dir.path().join(CALLED)).unwrap();
        assert_eq!(
            run(&request(dir.path()), dir.path(), Policy::AllowOnce),
            denial()
        );
        assert_eq!(fs::read(sentinel).unwrap(), MARKER);
    }

    #[test]
    fn concurrent_requests_can_allow_at_most_once() {
        let dir = tempfile::tempdir().unwrap();
        let input = serde_json::to_vec(&request(dir.path())).unwrap();
        let barrier = std::sync::Barrier::new(8);
        let allowed = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        decide(&input, dir.path(), SESSION, Policy::AllowOnce) == allow()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| usize::from(h.join().unwrap()))
                .sum::<usize>()
        });
        assert!(allowed <= 1);
    }

    #[test]
    fn outputs_and_observations_are_redacted_and_never_update_input_or_permissions() {
        for policy in [Policy::Deny, Policy::AllowOnce] {
            let dir = tempfile::tempdir().unwrap();
            let mut input = request(dir.path());
            // Common/optional hook fields are allowed but never retained or echoed.
            input["transcript_path"] = json!("SECRET_TRANSCRIPT_PATH");
            input["permission_suggestions"] = json!([{"secret": "SECRET_RULE"}]);
            let output = run(&input, dir.path(), policy).to_string();
            for forbidden in [
                "SECRET",
                SESSION,
                "updatedInput",
                "updatedPermissions",
                "tool_use_id",
            ] {
                assert!(!output.contains(forbidden));
            }
            assert!(!output.contains(dir.path().to_str().unwrap()));
            input["tool_input"]["content"] = json!("SECRET_UNTRUSTED_CONTENT");
            assert_eq!(run(&input, dir.path(), policy), denial());
            for entry in fs::read_dir(dir.path()).unwrap() {
                assert_eq!(fs::read(entry.unwrap().path()).unwrap(), MARKER);
            }
        }
    }
}
