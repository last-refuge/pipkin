use super::*;
use crate::{
    execution::claude_cli::preflight::{self, Candidate, Request},
    runtime::cli_bridge_channel::Server,
    store::{Store, cli_agent, cli_bridge::BridgeSpec},
};
use std::os::unix::fs::PermissionsExt;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
struct Fixture {
    root: tempfile::TempDir,
    binding: Binding,
    prepared: preflight::Prepared,
    assets: Assets,
    env: Environment,
    context: crate::store::cli_agent_turns::LaunchContext,
    _server: Server,
}
impl Fixture {
    async fn new() -> Self {
        Self::with_input(None, "private user prompt", &[]).await
    }
    async fn with_input(
        thinking: Option<&str>,
        text: &str,
        images: &[pipkin_engine_protocol::ImageInput],
    ) -> Self {
        let root = tempfile::tempdir().unwrap();
        let exe = root.path().join("fake-cli");
        let calls = root.path().join("calls");
        let flags = FLAGS.join("\n");
        crate::execution::claude_cli::test_support::write_script(&exe,format!("#!/bin/sh\nprintf 'metadata\\n' >> '{}'\ncase \"$1\" in\n--version) printf '2.1.301 (Claude Code)\\n';;\n--help) printf '%s\\n' '{}';;\n--mcp-config) printf '%s\\n' '{{\"loggedIn\":true,\"authMethod\":\"claude.ai\",\"apiProvider\":\"firstParty\",\"email\":\"fixture@example.invalid\",\"orgId\":\"fixture\"}}';;\n*) exit 99;;\nesac\n",calls.display(),flags)).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
        let env: Environment = [
            ("HOME".into(), root.path().as_os_str().into()),
            ("PATH".into(), "/usr/bin:/bin".into()),
        ]
        .into_iter()
        .collect();
        let prepared = preflight::check_orchestration(
            Request {
                candidate: Candidate::inspect(&exe, root.path(), &env).unwrap(),
                trust_executable: true,
                trust_project: true,
                environment: env.clone(),
                model: ModelAlias::Sonnet,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
        let create = prepared
            .orchestration_create_with_thinking(
                "s",
                "create",
                "11111111-1111-4111-8111-111111111111",
                None,
                thinking,
            )
            .unwrap();
        let store = Arc::new(Store::memory().unwrap());
        let binding = store.create_cli_agent_session(&create, 1).unwrap().0;
        let lease = store
            .admit_cli_agent_turn(
                &crate::store::cli_agent_turns::Input {
                    images,
                    mode: pipkin_engine_protocol::InputMode::Prompt,
                    session: "s",
                    request: "r",
                    generation: 1,
                    operation: "op",
                    binding: binding.digest,
                    text,
                },
                "owner",
                2,
            )
            .unwrap()
            .lease
            .unwrap();
        let context = store.cli_agent_launch_context(&lease).unwrap();
        let run = context.scope().run;
        let spec = BridgeSpec {
            id: "b".into(),
            scope: super::super::orchestration_protocol::Scope {
                session: "s".into(),
                generation: 1,
                operation: "op".into(),
                run,
            },
            owner: "owner".into(),
            binding_hash: binding.digest,
            capability_hash: [0; 32],
        };
        let server = Server::bind_native(
            store,
            spec,
            [3; 32],
            Instant::now() + Duration::from_secs(30),
        )
        .await
        .unwrap();
        // Test-only source copy; this owned fixture is NEVER executed as a helper/model.
        let image = HelperImage::copy_in(root.path(), File::open(&exe).unwrap()).unwrap();
        let assets = Assets::new(
            root.path(),
            image,
            server.native_launch().unwrap(),
            "private native system snapshot",
        )
        .unwrap();
        Self {
            root,
            binding,
            prepared,
            assets,
            env,
            context,
            _server: server,
        }
    }
}
#[tokio::test]
async fn admitted_launch_spec_uses_only_stored_prompt_mode_and_binding() {
    let f = Fixture::new().await;
    let spec = f
        .prepared
        .launch_admitted_orchestration_spec(&f.context, &f.assets)
        .unwrap();
    assert_eq!(spec.input, b"private user prompt\n");
    assert!(spec.args.iter().any(|a| a == "--session-id"));
    assert!(!spec.args.iter().any(|a| a == "--resume"));
    assert_eq!(
        spec.cwd.to_str().unwrap(),
        f.context.binding().create.identity.project_path
    );
    assert!(!format!("{:?}", f.context).contains("private user prompt"));
    // Only three owned-fixture metadata probes ran, never a model launch.
    assert_eq!(
        std::fs::read_to_string(f.root.path().join("calls"))
            .unwrap()
            .lines()
            .count(),
        3
    );
}
#[tokio::test]
async fn native_policy_has_one_mcp_executor_and_no_secrets_or_prompts_in_argv() {
    let f = Fixture::new().await;
    for mode in [TurnMode::Fresh, TurnMode::Resume] {
        let spec = f
            .prepared
            .launch_orchestration_spec(
                &f.binding,
                &f._server.identity().scope,
                mode,
                "private user prompt",
                &f.assets,
            )
            .unwrap();
        let args = spec
            .args
            .iter()
            .map(|s| s.to_str().unwrap())
            .collect::<Vec<_>>();
        for pair in [
            ["--tools", ""],
            ["--allowedTools", "mcp__pipkin__*"],
            ["--permission-prompts", "none"],
            ["--setting-sources", ""],
        ] {
            assert!(args.windows(2).any(|p| p == pair));
        }
        assert!(args.contains(&"--strict-mcp-config"));
        assert!(args.contains(&"--disable-slash-commands"));
        assert!(args.contains(&if mode == TurnMode::Fresh {
            "--session-id"
        } else {
            "--resume"
        }));
        assert!(!args.join(" ").contains("private"));
        assert!(!args.join(" ").contains("secret"));
        assert!(!spec.env.contains_key(std::ffi::OsStr::new(
            super::super::orchestration_helper::CHANNEL_ENV
        )));
        assert_eq!(spec.input, b"private user prompt\n");
        assert!(
            spec.timeout
                .is_some_and(|timeout| timeout <= Duration::from_secs(30))
        );
    }
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&f.assets.mcp).unwrap()).unwrap();
    assert_eq!(config["mcpServers"].as_object().unwrap().len(), 1);
    assert_eq!(
        config["mcpServers"]["pipkin"]["args"],
        serde_json::json!([super::super::orchestration_helper::FLAG])
    );
    assert_eq!(
        std::fs::read_to_string(f.root.path().join("calls"))
            .unwrap()
            .lines()
            .count(),
        3
    );
    assert!(
        f.prepared
            .launch_spec(&f.binding.create.identity, TurnMode::Fresh, "no channel")
            .is_err()
    );
}
#[tokio::test]
async fn assets_are_private_owned_and_cleaned_without_touching_source_or_other_files() {
    let f = Fixture::new().await;
    let mcp = f.assets.mcp.clone();
    let system = f.assets.system.clone();
    let helper = f.assets.image.path.clone();
    for (p, mode) in [(&mcp, 0o600), (&system, 0o600), (&helper, 0o700)] {
        assert_eq!(
            std::fs::metadata(p).unwrap().permissions().mode() & 0o777,
            mode
        );
    }
    assert_eq!(
        std::fs::metadata(f.assets.directory.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let original = std::fs::read(f.root.path().join("fake-cli")).unwrap();
    drop(f.assets);
    assert!(!mcp.exists() && !system.exists() && !helper.exists());
    assert_eq!(
        std::fs::read(f.root.path().join("fake-cli")).unwrap(),
        original
    );
}
#[tokio::test]
async fn detached_binding_uuid_model_and_snapshot_changes_fail_closed() {
    let f = Fixture::new().await;
    for change in 0..4 {
        let mut b = f.binding.clone();
        match change {
            0 => b.create.identity.cli_session_id = "22222222-2222-4222-8222-222222222222".into(),
            1 => b.create.model = "haiku".into(),
            2 => b.create.session = "other".into(),
            _ => b.create.thinking = Some("high".into()),
        }
        assert!(
            f.prepared
                .launch_orchestration_spec(
                    &b,
                    &f._server.identity().scope,
                    TurnMode::Fresh,
                    "prompt",
                    &f.assets
                )
                .is_err()
        );
    }
    std::fs::write(&f.assets.system, "replaced snapshot with different length").unwrap();
    assert!(
        f.prepared
            .launch_orchestration_spec(
                &f.binding,
                &f._server.identity().scope,
                TurnMode::Fresh,
                "prompt",
                &f.assets
            )
            .is_err()
    );
}
#[tokio::test]
async fn shared_native_configuration_does_not_allow_a_preflight_model_to_rebind_a_session() {
    let f = Fixture::new().await;
    let other = preflight::check_orchestration(
        Request {
            candidate: Candidate::inspect(
                Path::new(&f.binding.create.identity.executable_path),
                f.root.path(),
                &f.env,
            )
            .unwrap(),
            trust_executable: true,
            trust_project: true,
            environment: f.env.clone(),
            model: ModelAlias::Haiku,
        },
        CancellationToken::new(),
    )
    .await
    .unwrap();
    // Configuration legitimately matches across child aliases, but launch model identity does not.
    other.validate_binding(&f.binding.create.identity).unwrap();
    assert!(
        other
            .launch_orchestration_spec(
                &f.binding,
                &f._server.identity().scope,
                TurnMode::Fresh,
                "prompt",
                &f.assets
            )
            .is_err()
    );
}
#[tokio::test]
async fn ambient_channel_override_cannot_be_silently_inherited() {
    let f = Fixture::new().await;
    let mut env = f.env.clone();
    env.insert(
        super::super::orchestration_helper::CHANNEL_ENV.into(),
        "foreign-channel".into(),
    );
    assert!(policy::launch_environment(&env).is_err());
}
#[tokio::test]
async fn invalid_system_snapshots_or_replaced_helper_do_not_construct_a_launch() {
    let f = Fixture::new().await;
    for text in [String::new(), "x\0y".into(), "x".repeat(MAX_SYSTEM + 1)] {
        assert!(
            Assets::new(
                f.root.path(),
                f.assets.image.clone(),
                f._server.native_launch().unwrap(),
                &text
            )
            .is_err()
        );
    }
    std::fs::write(&f.assets.image.path, "changed image").unwrap();
    assert!(f.assets.validate(&f.binding).is_err());
}
#[tokio::test]
async fn expired_channel_cannot_generate_private_launch_assets() {
    let f = Fixture::new().await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(31)).await;
    assert!(f.assets.validate(&f.binding).is_err());
    assert!(f._server.native_launch().is_err());
}
#[tokio::test]
async fn old_assets_cannot_cross_a_new_generation_operation_or_run() {
    let f = Fixture::new().await;
    for change in 0..3 {
        let mut scope = f._server.identity().scope.clone();
        match change {
            0 => scope.generation += 1,
            1 => scope.operation.push('x'),
            _ => scope.run += 1,
        }
        assert!(
            f.prepared
                .launch_orchestration_spec(
                    &f.binding,
                    &scope,
                    TurnMode::Resume,
                    "prompt",
                    &f.assets
                )
                .is_err()
        );
    }
}
#[tokio::test]
async fn widened_private_directory_permissions_reject_launch() {
    let f = Fixture::new().await;
    std::fs::set_permissions(
        f.assets.directory.path(),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert!(f.assets.validate(&f.binding).is_err());
}
#[test]
fn copied_image_is_independent_of_source_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    std::fs::write(&source, b"captured image fixture").unwrap();
    let image = HelperImage::copy_in(dir.path(), File::open(&source).unwrap()).unwrap();
    std::fs::write(&source, b"different source").unwrap();
    image.validate().unwrap();
    assert_eq!(
        std::fs::read(&image.path).unwrap(),
        b"captured image fixture"
    );
}
#[test]
fn copied_image_probe_child() {
    if std::env::var("PIPKIN_TEST_COPIED_IMAGE").ok().as_deref() != Some("owned-image-v1") {
        return;
    }
    assert_eq!(
        std::env::current_exe().unwrap().file_name().unwrap(),
        "helper"
    );
    std::process::exit(0);
}
#[test]
fn captured_running_image_executes_from_its_private_copy_with_clean_environment() {
    let dir = tempfile::tempdir().unwrap();
    let image = HelperImage::capture_in(dir.path()).unwrap();
    let status = std::process::Command::new(&image.path)
        .env_clear()
        .env("HOME", dir.path())
        .env("PATH", "/usr/bin:/bin")
        .env("PIPKIN_TEST_COPIED_IMAGE", "owned-image-v1")
        .args([
            "--exact",
            "execution::claude_cli::orchestration_policy::tests::copied_image_probe_child",
            "--nocapture",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let path = image.path.clone();
    drop(image);
    assert!(!path.exists());
}
#[test]
fn metadata_digest_detects_edits_without_storage_or_model_requests() {
    let s = Store::memory().unwrap();
    let b = s
        .create_cli_agent_session(&cli_agent::tests::create("s", "/project"), 1)
        .unwrap()
        .0;
    assert_eq!(b.verified_digest(), Some(b.digest));
    let mut edited = b;
    edited.root = Some("foreign".into());
    assert!(edited.verified_digest().is_none());
}

#[tokio::test]
async fn features_effort_and_images_use_approved_identity_and_stdin_only() {
    use pipkin_engine_protocol::ImageInput;
    let images = vec![ImageInput {
        mime_type: "image/png".into(),
        data: "aW1hZ2U=".into(),
    }];
    for effort in ["low", "medium", "high", "xhigh", "max"] {
        let f = Fixture::with_input(Some(effort), "", &images).await;
        let spec = f
            .prepared
            .launch_admitted_orchestration_spec(&f.context, &f.assets)
            .unwrap();
        let args = spec
            .args
            .iter()
            .map(|s| s.to_str().unwrap())
            .collect::<Vec<_>>();
        assert!(args.windows(2).any(|p| p == ["--effort", effort]));
        assert!(
            args.windows(2)
                .any(|p| p == ["--input-format", "stream-json"])
        );
        assert!(!args.join(" ").contains(&images[0].data));
        let body: serde_json::Value = serde_json::from_slice(&spec.input).unwrap();
        assert_eq!(
            body["message"]["content"][0]["source"]["data"],
            images[0].data
        );
        assert_eq!(f.context.images(), images);
        assert!(spec.input.ends_with(b"\n"));
    }
}
