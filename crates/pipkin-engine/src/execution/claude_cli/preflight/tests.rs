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
    use crate::execution::claude_cli::test_support::write_script;
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
            write_script(&exe, script).unwrap();
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
                "2.1.299 (Claude Code)",
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
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn native_preflight_is_distinct_from_coding_and_cannot_use_generic_launch() {
        let f = Fixture::good();
        let extra = super::super::super::coding_policy::FLAGS
            .iter()
            .chain(super::super::super::orchestration_policy::FLAGS)
            .map(|s| format!("{s} fixture"))
            .collect::<Vec<_>>()
            .join("\n");
        let script = std::fs::read_to_string(&f.exe).unwrap().replace(
            "--safe-mode fixture",
            &format!("{extra}\n--safe-mode fixture"),
        );
        write_script(&f.exe, script).unwrap();
        let coding = check_for_policy(f.request(), CancellationToken::new(), LaunchPolicy::Coding)
            .await
            .unwrap();
        let native = check_orchestration(f.request(), CancellationToken::new())
            .await
            .unwrap();
        assert!(coding.orchestration_request().is_err());
        assert!(native.orchestration_request().is_ok());
        let uuid = "11111111-1111-4111-8111-111111111111";
        let c = coding.checked_binding(uuid).unwrap();
        let n = native.checked_binding(uuid).unwrap();
        assert_ne!(c.launch_policy, n.launch_policy);
        assert_ne!(c.config_ref, n.config_ref);
        assert!(native.validate_binding(&c).is_err());
        assert!(coding.validate_binding(&n).is_err());
        assert!(
            coding
                .orchestration_create("s", "create", uuid, None)
                .is_err()
        );
        let captured = native
            .orchestration_create("s", "create", uuid, None)
            .unwrap();
        assert_eq!(captured.model, "sonnet");
        assert_eq!(captured.identity, n);
        assert!(
            native
                .launch_spec(&n, TurnMode::Fresh, "no native channel")
                .is_err()
        );
        assert_eq!(f.calls(), 6);
    }
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn native_hidden_system_file_flag_must_be_accepted_by_metadata_parser() {
        for supported in [true, false] {
            let f = Fixture::good();
            let flags = super::super::super::orchestration_policy::FLAGS.join("\n");
            assert!(!flags.contains("--system-prompt-file"));
            let check = if supported {
                r#"found=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --print|-p) exit 92 ;;
    --system-prompt-file)
      shift
      [ -f "$1" ] || exit 93
      [ "$(cat "$1")" = 'Metadata-only capability check; no inference.' ] || exit 94
      found=1 ;;
  esac
  shift
done
[ "$found" = 1 ] || exit 95"#
            } else {
                "exit 96"
            };
            let script = format!(
                "#!/bin/sh\ncase \"$1\" in\n--version) printf '2.1.301 (Claude Code)\\n' ;;\n--help) printf '%s\\n' '{flags}' ;;\n--mcp-config)\n{check}\nprintf '%s\\n' '{{\"loggedIn\":true,\"authMethod\":\"claude.ai\",\"apiProvider\":\"firstParty\",\"email\":\"fixture@example.invalid\",\"orgId\":\"fixture\"}}' ;;\n*) exit 99 ;;\nesac\n"
            );
            crate::execution::claude_cli::test_support::write_script(&f.exe, script).unwrap();
            let result = check_orchestration(f.request(), CancellationToken::new()).await;
            assert_eq!(result.is_ok(), supported);
            if !supported {
                assert_eq!(
                    result.err(),
                    Some(
                        "CLI auth probe did not settle cleanly; use CLI terminal login if signed out"
                    )
                );
            }
        }
    }
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn native_child_alias_changes_preserve_configuration_but_not_model_identity() {
        let f = Fixture::good();
        let extra = super::super::super::orchestration_policy::FLAGS
            .iter()
            .map(|s| format!("{s} fixture"))
            .collect::<Vec<_>>()
            .join("\n");
        let script = std::fs::read_to_string(&f.exe).unwrap().replace(
            "--safe-mode fixture",
            &format!("{extra}\n--safe-mode fixture"),
        );
        write_script(&f.exe, script).unwrap();
        let parent = check_orchestration(f.request(), CancellationToken::new())
            .await
            .unwrap();
        let mut request = f.request();
        request.model = ModelAlias::Haiku;
        let child = check_orchestration(request, CancellationToken::new())
            .await
            .unwrap();
        let p = parent
            .orchestration_create(
                "parent",
                "p-create",
                "11111111-1111-4111-8111-111111111111",
                None,
            )
            .unwrap();
        let c = child
            .orchestration_create(
                "child",
                "c-create",
                "22222222-2222-4222-8222-222222222222",
                Some("parent".into()),
            )
            .unwrap();
        assert_eq!(p.identity.config_ref, c.identity.config_ref);
        assert_eq!(p.model, "sonnet");
        assert_eq!(c.model, "haiku");
        let store = crate::store::Store::memory().unwrap();
        let p = store.create_cli_agent_session(&p, 1).unwrap().0;
        let c = store.create_cli_agent_session(&c, 2).unwrap().0;
        assert_ne!(p.digest, c.digest);
        assert_eq!(c.root.as_deref(), Some("parent"));
        assert_eq!(f.calls(), 6);
    }
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn native_preflight_requires_its_flags_and_both_trust_decisions() {
        let f = Fixture::good();
        let mut request = f.request();
        request.trust_project = false;
        assert!(
            check_orchestration(request, CancellationToken::new())
                .await
                .is_err()
        );
        assert_eq!(f.calls(), 0);
        let mut request = f.request();
        request.trust_executable = false;
        assert!(
            check_orchestration(request, CancellationToken::new())
                .await
                .is_err()
        );
        assert_eq!(f.calls(), 0);
        assert!(
            check_orchestration(f.request(), CancellationToken::new())
                .await
                .is_err()
        );
        assert_eq!(f.calls(), 2);
    }
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn coding_binding_is_distinct_and_cannot_upgrade_a_chat_context() {
        use crate::execution::claude_cli::{
            permission_helper::HelperLaunch, permission_wire::Binding, write_policy::Hook,
        };
        let f = Fixture::good();
        let script = std::fs::read_to_string(&f.exe).unwrap().replace(
            "--safe-mode fixture",
            "--settings fixture\n--safe-mode fixture",
        );
        write_script(&f.exe, script).unwrap();
        let chat = check(f.request(), CancellationToken::new()).await.unwrap();
        let coding = check_for_policy(
            f.request(),
            CancellationToken::new(),
            LaunchPolicy::RestrictedWrite,
        )
        .await
        .unwrap();
        let uuid = "11111111-1111-4111-8111-111111111111";
        let cb = chat.checked_binding(uuid).unwrap();
        let wb = coding.checked_binding(uuid).unwrap();
        assert_ne!(cb.config_ref, wb.config_ref);
        assert_eq!(cb.launch_policy, policy::POLICY_ID);
        assert_eq!(wb.launch_policy, super::super::super::write_policy::ID);
        assert!(chat.validate_binding(&wb).is_err());
        assert!(coding.validate_binding(&cb).is_err());
        assert!(coding.launch_spec(&wb, TurnMode::Fresh, "prompt").is_err());
        let hook = Hook::capture(
            HelperLaunch::new(Path::new("/private/socket"), Binding::fresh().unwrap()).unwrap(),
        )
        .unwrap();
        assert!(
            chat.launch_write_spec(&cb, TurnMode::Fresh, "prompt", &hook)
                .is_err()
        );
        assert!(
            coding
                .launch_write_spec(&cb, TurnMode::Fresh, "prompt", &hook)
                .is_err()
        );
        assert!(
            coding
                .launch_write_spec(&wb, TurnMode::Fresh, "prompt", &hook)
                .is_ok()
        );
        assert!(
            coding
                .launch_write_spec(&wb, TurnMode::Resume, "prompt", &hook)
                .is_ok()
        );
        assert_eq!(f.calls(), 6); // Only version/help/auth. Constructing Specs never launches.
        let calls = std::fs::read_to_string(f.dir.path().join("invocations")).unwrap();
        let auth = calls.lines().last().unwrap();
        assert!(auth.contains("--settings") && auth.contains("onFailure"));
        assert!(!auth.contains("--safe-mode") && !auth.contains("--print"));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn coding_requires_settings_help_and_consent_before_authentication() {
        let f = Fixture::good();
        let mut req = f.request();
        req.trust_project = false;
        assert!(
            check_for_policy(req, CancellationToken::new(), LaunchPolicy::RestrictedWrite)
                .await
                .is_err()
        );
        assert_eq!(f.calls(), 0);
        assert!(
            check_for_policy(
                f.request(),
                CancellationToken::new(),
                LaunchPolicy::RestrictedWrite
            )
            .await
            .is_err()
        );
        assert_eq!(f.calls(), 2); // Missing --settings stops before auth; no inference.
    }

    #[tokio::test]
    async fn filesystem_revalidation_cannot_return_freshness_after_lease_expiry() {
        let f = Fixture::good();
        let prepared = check(f.request(), CancellationToken::new()).await.unwrap();
        let binding = prepared
            .binding("11111111-1111-4111-8111-111111111111")
            .unwrap();
        let mut clocks = [
            prepared.checked_at + Duration::from_secs(1),
            prepared.checked_at + Duration::from_secs(31),
        ]
        .into_iter();
        assert!(
            prepared
                .validate_binding_with_clock(&binding, || clocks.next().unwrap())
                .is_err()
        );
        assert!(clocks.next().is_none());
        assert_eq!(f.calls(), 3); // Metadata only, no inference or automatic refresh.
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
    async fn old_or_malformed_version_never_runs_auth_or_inference() {
        for version in [
            "2.1.298 (Claude Code)",
            "2.1.301 unknown",
            "999999999999999999999.0.0 (Claude Code)",
        ] {
            let f = Fixture::new(version, "must-not-read");
            assert!(check(f.request(), CancellationToken::new()).await.is_err());
            assert_eq!(f.calls(), 1);
        }
    }

    #[tokio::test]
    async fn newer_version_keeps_actual_identity_and_other_preflight_checks() {
        let auth = r#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","email":"fixture@example.invalid","orgId":"fixture-org"}"#;
        for launch_policy in [LaunchPolicy::ChatOnly, LaunchPolicy::RestrictedWrite] {
            if launch_policy == LaunchPolicy::RestrictedWrite && !cfg!(target_os = "linux") {
                continue;
            }
            let f = Fixture::new("2.1.301 (Claude Code)", auth);
            let script = std::fs::read_to_string(&f.exe).unwrap().replace(
                "--safe-mode fixture",
                "--safe-mode fixture\n--settings fixture",
            );
            write_script(&f.exe, script).unwrap();
            let prepared = check_for_policy(f.request(), CancellationToken::new(), launch_policy)
                .await
                .unwrap();
            let binding = prepared
                .checked_binding("11111111-1111-4111-8111-111111111111")
                .unwrap();
            assert_eq!(binding.cli_version, "2.1.301");
            assert_eq!(f.calls(), 3);
            let mut changed = binding;
            changed.cli_version = policy::QUALIFIED_VERSION.into();
            assert!(
                prepared.validate_binding(&changed).is_err(),
                "new releases cannot silently change an existing session identity"
            );
        }
        let f = Fixture::new("2.1.301 (Claude Code)", auth);
        let script = std::fs::read_to_string(&f.exe)
            .unwrap()
            .replace("--safe-mode fixture", "missing flag");
        write_script(&f.exe, script).unwrap();
        assert!(check(f.request(), CancellationToken::new()).await.is_err());
        assert_eq!(f.calls(), 2, "missing capabilities must stop before auth");
        let f = Fixture::new(
            "2.1.301 (Claude Code)",
            r#"{"loggedIn":false,"authMethod":"none"}"#,
        );
        assert!(check(f.request(), CancellationToken::new()).await.is_err());
        assert_eq!(f.calls(), 3, "a newer version does not bypass login");
    }
    #[tokio::test]
    async fn prepared_metadata_is_bound_and_only_builds_a_no_tools_stdin_request() {
        let f = Fixture::good();
        let mut prepared = check(f.request(), CancellationToken::new()).await.unwrap();
        assert_eq!(f.calls(), 3);
        let binding = prepared
            .binding("11111111-1111-4111-8111-111111111111")
            .unwrap();
        assert_eq!(
            prepared.checked_binding(&binding.cli_session_id).unwrap(),
            binding
        );
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
        assert!(prepared.checked_binding(&binding.cli_session_id).is_err());
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
        write_script(&f.exe, "#!/bin/sh\nexit 99\n").unwrap();
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
