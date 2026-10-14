use super::*;

#[test]
fn account_identity_is_hashed_without_retaining_status_payload_or_claiming_billing() {
    let status = br#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","email":"fixture@example.invalid","orgId":"fixture-org","unknown":"private-do-not-retain"}"#;
    let marker = account_scope(status).unwrap();
    assert_eq!(marker.len(), 64);
    assert!(!marker.contains("fixture") && !marker.contains("private"));
    let changed = String::from_utf8(status.to_vec())
        .unwrap()
        .replace("fixture-org", "another-org");
    assert_ne!(marker, account_scope(changed.as_bytes()).unwrap());
    for bad in [
        r#"{"loggedIn":false,"authMethod":"none"}"#,
        r#"{"loggedIn":true,"authMethod":"api_key","email":"secret"}"#,
        r#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"future","email":"secret","orgId":"o"}"#,
        r#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty"}"#,
    ] {
        assert!(
            !account_scope(bad.as_bytes())
                .unwrap_err()
                .contains("secret")
        );
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::{ffi::OsStr, os::unix::fs::PermissionsExt};
    struct Fixture {
        dir: tempfile::TempDir,
        exe: PathBuf,
        project: PathBuf,
        env: Environment,
    }
    impl Fixture {
        fn new(version: &str, auth: &str) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let exe = dir.path().join("fake-cli");
            let project = dir.path().join("project");
            std::fs::create_dir(&project).unwrap();
            let help = policy::REQUIRED_FLAGS
                .iter()
                .map(|flag| format!("{flag} fixture"))
                .collect::<Vec<_>>()
                .join("\n");
            let log = dir.path().join("invocations");
            let script = format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\ncase \"$1\" in\n--version) printf '%s\\n' '{}' ;;\n--help) printf '%s\\n' '{}' ;;\n--mcp-config) printf '%s\\n' '{}' ;;\n*) exit 99 ;;\nesac\n",
                log.display(),
                version,
                help,
                auth
            );
            std::fs::write(&exe, script).unwrap();
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
            let env = [
                ("HOME".into(), dir.path().as_os_str().to_owned()),
                ("PATH".into(), "/usr/bin:/bin".into()),
            ]
            .into_iter()
            .collect();
            Self {
                dir,
                exe,
                project,
                env,
            }
        }
        fn good() -> Self {
            Self::new(
                "2.1.296 (Claude Code)",
                r#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","email":"fixture@example.invalid","orgId":"fixture-org"}"#,
            )
        }
        fn request(&self) -> Request {
            Request {
                candidate: Candidate::inspect(&self.exe, &self.project, &self.env).unwrap(),
                trust_executable: true,
                trust_project: true,
                environment: self.env.clone(),
                model: ModelAlias::Sonnet,
            }
        }
        fn calls(&self) -> usize {
            std::fs::read_to_string(self.dir.path().join("invocations"))
                .unwrap_or_default()
                .lines()
                .count()
        }
    }
    #[tokio::test]
    async fn consent_and_environment_fail_before_any_spawn() {
        let f = Fixture::good();
        let mut request = f.request();
        request.trust_executable = false;
        assert!(check(request, CancellationToken::new()).await.is_err());
        let mut request = f.request();
        request.trust_project = false;
        assert!(check(request, CancellationToken::new()).await.is_err());
        let mut request = f.request();
        request
            .environment
            .insert("ANTHROPIC_API_KEY".into(), "must-not-forward".into());
        assert!(check(request, CancellationToken::new()).await.is_err());
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(check(f.request(), cancel).await.is_err());
        assert_eq!(f.calls(), 0);
    }
    #[tokio::test]
    async fn unqualified_version_never_runs_auth_or_inference() {
        let f = Fixture::new("9.9.9 (Claude Code)", "must-not-read");
        assert!(check(f.request(), CancellationToken::new()).await.is_err());
        assert_eq!(f.calls(), 1);
    }
    #[tokio::test]
    async fn prepared_metadata_is_bound_and_only_builds_a_no_tools_stdin_request() {
        let f = Fixture::good();
        let mut prepared = check(f.request(), CancellationToken::new()).await.unwrap();
        assert_eq!(f.calls(), 3);
        let binding = prepared
            .binding("11111111-1111-4111-8111-111111111111")
            .unwrap();
        assert_eq!(binding.cli_version, policy::QUALIFIED_VERSION);
        assert_eq!(binding.launch_policy, policy::POLICY_ID);
        let spec = prepared
            .launch_spec(&binding, TurnMode::Fresh, "do not execute this test prompt")
            .unwrap();
        assert_eq!(
            spec.env.get(OsStr::new("HOME")),
            f.env.get(OsStr::new("HOME"))
        );
        assert_eq!(f.calls(), 3); // Building a spec never spawns the inference process.
        let mut changed = binding.clone();
        changed.account_scope = "different-account".into();
        assert!(
            prepared
                .launch_spec(&changed, TurnMode::Resume, "x")
                .is_err()
        );
        prepared.checked_at = Instant::now() - Duration::from_secs(31);
        assert!(
            prepared
                .launch_spec(&binding, TurnMode::Resume, "x")
                .is_err()
        );
        assert_eq!(f.calls(), 3);
    }
    #[tokio::test]
    async fn changed_version_manager_symlink_or_executable_revokes_candidate_and_prepared_metadata()
    {
        let f = Fixture::good();
        let link = f.dir.path().join("selected-cli");
        std::os::unix::fs::symlink(&f.exe, &link).unwrap();
        let mut request = f.request();
        request.candidate = Candidate::inspect(&link, &f.project, &f.env).unwrap();
        let prepared = check(request, CancellationToken::new()).await.unwrap();
        let binding = prepared
            .binding("11111111-1111-4111-8111-111111111111")
            .unwrap();
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink("/bin/sh", &link).unwrap();
        assert!(
            prepared
                .launch_spec(&binding, TurnMode::Fresh, "x")
                .is_err()
        );
        let request = f.request();
        std::fs::write(&f.exe, "#!/bin/sh\nexit 99\n").unwrap();
        assert!(check(request, CancellationToken::new()).await.is_err());
        assert_eq!(f.calls(), 3);
    }
    #[tokio::test]
    async fn changed_configuration_directory_symlink_requires_new_preflight() {
        let mut f = Fixture::good();
        let a = f.dir.path().join("profile-a");
        let b = f.dir.path().join("profile-b");
        let link = f.dir.path().join("profile-link");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        std::os::unix::fs::symlink(&a, &link).unwrap();
        f.env
            .insert("CLAUDE_CONFIG_DIR".into(), link.clone().into_os_string());
        let prepared = check(f.request(), CancellationToken::new()).await.unwrap();
        let binding = prepared
            .binding("11111111-1111-4111-8111-111111111111")
            .unwrap();
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&b, &link).unwrap();
        assert!(
            prepared
                .launch_spec(&binding, TurnMode::Resume, "x")
                .is_err()
        );
        assert_eq!(f.calls(), 3);
    }
    #[tokio::test]
    async fn account_change_is_not_silently_rebound_and_project_content_changes_are_allowed() {
        let f = Fixture::good();
        let prepared = check(f.request(), CancellationToken::new()).await.unwrap();
        let binding = prepared
            .binding("11111111-1111-4111-8111-111111111111")
            .unwrap();
        std::fs::write(f.project.join("normal-file"), "normal project editing").unwrap();
        assert!(
            prepared
                .launch_spec(&binding, TurnMode::Resume, "x")
                .is_ok()
        );
        let mut changed = binding.clone();
        changed.account_scope = policy::marker(["another-account"]);
        assert!(
            prepared
                .launch_spec(&changed, TurnMode::Resume, "x")
                .is_err()
        );
        std::fs::rename(&f.project, f.dir.path().join("old-project")).unwrap();
        std::fs::create_dir(&f.project).unwrap();
        assert!(
            prepared
                .launch_spec(&binding, TurnMode::Fresh, "x")
                .is_err()
        );
    }
}
