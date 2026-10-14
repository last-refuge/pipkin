use super::*;

fn environment() -> Environment {
    [
        ("HOME", "/fixture/home"),
        ("PATH", "/usr/bin:/bin"),
        ("HTTPS_PROXY", "https://private-proxy.invalid"),
        ("NODE_EXTRA_CA_CERTS", "/fixture/ca.pem"),
        ("UNRELATED_SECRET", "do-not-forward"),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v.into()))
    .collect()
}

#[test]
fn rejects_conflicts_by_presence_before_filtering_without_echoing_values() {
    for key in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_FUTURE_OPTION",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDE_CODE_UNKNOWN",
        "CLAUDECODE",
        "NODE_OPTIONS",
        "LD_PRELOAD",
        "BASH_ENV",
    ] {
        for value in ["", "secret-must-not-appear"] {
            let mut env = environment();
            env.insert(key.into(), value.into());
            let error = launch_environment(&env).unwrap_err();
            assert!(!error.contains("secret-must-not-appear"));
            assert!(error.contains("explicitly"));
        }
    }
}

#[test]
fn preserves_cli_owned_login_location_proxy_ca_and_path_but_not_unrelated_secrets() {
    let mut source = environment();
    source.insert("CLAUDE_CONFIG_DIR".into(), "/fixture/profile".into());
    let env = launch_environment(&source).unwrap();
    for key in [
        "HOME",
        "PATH",
        "HTTPS_PROXY",
        "NODE_EXTRA_CA_CERTS",
        "CLAUDE_CONFIG_DIR",
    ] {
        assert_eq!(env.get(OsStr::new(key)), source.get(OsStr::new(key)));
    }
    assert!(!env.contains_key(OsStr::new("UNRELATED_SECRET")));
    source.insert("HOME".into(), "relative-home".into());
    assert!(launch_environment(&source).is_err());
    source.remove(OsStr::new("HOME"));
    assert!(launch_environment(&source).is_err());
}

#[test]
fn fixed_no_tools_policy_uses_stdin_and_exact_session_selection_not_shell_arguments() {
    let uuid = "11111111-1111-4111-8111-111111111111";
    let prompt = "--dangerously-skip-permissions\n'; touch must-not-exist; #";
    for mode in [TurnMode::Fresh, TurnMode::Resume] {
        let spec = turn_spec(
            Path::new("/fixture/claude"),
            Path::new("/fixture/project"),
            &environment(),
            uuid,
            mode,
            ModelAlias::Sonnet,
            prompt,
        )
        .unwrap();
        let args: Vec<_> = spec.args.iter().map(|s| s.to_str().unwrap()).collect();
        assert_eq!(spec.input, format!("{prompt}\n").as_bytes());
        assert!(!args.contains(&prompt));
        assert!(args.windows(2).any(|p| p == ["--tools", ""]));
        assert!(args.contains(&"--safe-mode") && args.contains(&"--restricted"));
        assert!(
            !args.contains(&"--bare")
                && !args.contains(&"--dangerously-skip-permissions")
                && !args.contains(&"--continue")
        );
        let expected = if mode == TurnMode::Fresh {
            "--session-id"
        } else {
            "--resume"
        };
        assert_eq!(&args[args.len() - 2..], [expected, uuid]);
    }
    for bad in [
        "../session",
        "LATEST",
        "11111111-1111-4111-8111-11111111111Z",
    ] {
        assert!(!valid_uuid(bad));
    }
    for bad in [
        "".to_string(),
        "\0".into(),
        "x".repeat(MAX_PROMPT_BYTES + 1),
    ] {
        assert!(
            turn_spec(
                Path::new("/fixture/claude"),
                Path::new("/fixture/project"),
                &environment(),
                uuid,
                TurnMode::Fresh,
                ModelAlias::Sonnet,
                &bad
            )
            .is_err()
        );
    }
}

#[test]
fn version_and_help_are_exact_and_marker_parts_are_unambiguous() {
    assert!(validate_version(b"2.1.296 (Claude Code)\n").is_ok());
    for bytes in [
        b"2.1.297 (Claude Code)".as_slice(),
        b"2.1.296 attacker",
        b"\xff",
    ] {
        assert!(validate_version(bytes).is_err());
    }
    let help = REQUIRED_FLAGS
        .iter()
        .map(|f| format!("  {f} description\n"))
        .collect::<String>();
    assert!(validate_help(help.as_bytes()).is_ok());
    assert!(
        validate_help(
            help.replace(
                "  --safe-mode description",
                "text says --safe-mode is supported"
            )
            .as_bytes()
        )
        .is_err()
    );
    assert_ne!(marker(["ab", "c"]), marker(["a", "bc"]));
}

#[cfg(unix)]
#[test]
fn executable_resolution_never_uses_relative_path_or_a_login_shell() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("claude");
    std::fs::write(&exe, b"#!/bin/sh\nexit 99\n").unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut env = environment();
    env.insert(
        "PATH".into(),
        std::env::join_paths([Path::new("."), dir.path()]).unwrap(),
    );
    assert_eq!(resolve(None, &env).unwrap(), exe.canonicalize().unwrap());
    assert!(resolve(Some(Path::new("./claude")), &env).is_err());
}
