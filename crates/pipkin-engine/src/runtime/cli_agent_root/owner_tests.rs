use super::*;
use crate::runtime::cli_agent_owner::{Error as OwnerError, Registry};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn absolute_root_deadline_cancels_and_reaps_the_fresh_metadata_probe_before_inference() {
    let f = Fixture::new("deadline").await;
    std::fs::write(f.dir.path().join("hold-metadata"), "hold").unwrap();
    assert_eq!(run(f.store.clone(), f.launch).await, Err(Error::Cancelled));
    let pid: libc::pid_t = std::fs::read_to_string(f.dir.path().join("metadata-pid"))
        .unwrap()
        .parse()
        .unwrap();
    assert!(pid > 1);
    // Signal zero only observes the owned fixture pid; it sends no signal.
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
    assert!(!f.registry.has_turn("s"));
    assert!(!f.dir.path().join("launches").exists());
    assert_eq!(
        f.store.cli_agent_turn("s", "r").unwrap().unwrap().state,
        cli_agent_turns::State::Rejected
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn coding_preflight_cannot_mint_a_native_orchestration_grant() {
    let f = Fixture::new("success").await;
    let env: Environment = [
        ("HOME".into(), f.dir.path().as_os_str().into()),
        ("PATH".into(), "/usr/bin:/bin".into()),
    ]
    .into_iter()
    .collect();
    let coding = preflight::check_for_policy(
        Request {
            candidate: Candidate::inspect(&f.dir.path().join("fake-cli"), f.dir.path(), &env)
                .unwrap(),
            trust_executable: true,
            trust_project: true,
            environment: env,
            model: ModelAlias::Sonnet,
        },
        CancellationToken::new(),
        preflight::LaunchPolicy::Coding,
    )
    .await
    .unwrap();
    assert!(matches!(
        f.registry
            .create_approved(&f.store, 1, &coding, "coding", "coding-key"),
        Err(OwnerError::Preflight)
    ));
    assert!(f.store.cli_agent_binding("coding").unwrap().is_none());
    assert!(!f.dir.path().join("launches").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_connection_disconnect_revokes_only_its_independent_native_grant() {
    let f = Fixture::new("success").await;
    let prepared = f.launch.ownership.prepare().await.unwrap();
    tokio::task::spawn_blocking(move || {
        for enabled in [false, true] {
            let fx = crate::runtime::tests::Fixture::new();
            let provider = crate::provider::scripted::Scripted::echo("must not infer");
            let mut config = fx.config();
            config.enable_local_claude_cli = true;
            config.enable_local_claude_cli_orchestration = enabled;
            let e = fx.start_config(&provider, config);
            let shared = e.handle.shared().clone();
            let owner = *shared.clients.lock().unwrap().keys().next().unwrap();
            let created = shared.cli_agents.create_approved(
                &shared.store,
                owner,
                &prepared,
                "owned",
                "owned-key",
            );
            if !enabled {
                assert!(matches!(created, Err(OwnerError::Unavailable)));
                assert!(shared.store.cli_agent_binding("owned").unwrap().is_none());
            } else {
                assert!(created.unwrap().1);
                let turn = shared
                    .cli_agents
                    .begin_turn(&shared.store, owner, "owned")
                    .unwrap();
                drop(e.conn);
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                while !turn.cancellation().is_cancelled() {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "connection did not revoke ownership"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                assert!(shared.cli_agents.has_turn("owned"));
                drop(turn);
                assert!(!shared.cli_agents.has_turn("owned"));
            }
            assert!(provider.requests().is_empty());
            e.handle.shutdown(Duration::from_secs(2));
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconnect_and_create_lookup_never_reconstitute_execution_authority() {
    let f = Fixture::new("success").await;
    let prepared = f.launch.ownership.prepare().await.unwrap();
    let binding = f.launch.ownership.binding().clone();
    f.registry.connect(2);
    assert!(matches!(
        f.registry.begin_turn(&f.store, 2, "s"),
        Err(OwnerError::Unavailable)
    ));
    assert!(
        !f.registry
            .create_approved(&f.store, 2, &prepared, "s", "create")
            .unwrap()
            .1
    );
    assert!(matches!(
        f.registry.begin_turn(&f.store, 2, "s"),
        Err(OwnerError::Unavailable)
    ));
    f.registry.revoke_owner(1);
    f.registry.connect(1);
    assert!(
        f.registry.has_turn("s"),
        "revocation retains cleanup ownership"
    );
    assert!(matches!(
        f.registry.approve_return(&f.store, 1, &prepared, &binding),
        Err(OwnerError::Busy)
    ));
    assert_eq!(run(f.store.clone(), f.launch).await, Err(Error::Cancelled));
    assert!(!f.registry.has_turn("s"));
    assert!(
        !f.registry
            .create_approved(&f.store, 1, &prepared, "s", "create")
            .unwrap()
            .1
    );
    assert!(matches!(
        f.registry.begin_turn(&f.store, 1, "s"),
        Err(OwnerError::Unavailable)
    ));
    f.registry
        .approve_return(&f.store, 1, &prepared, &binding)
        .unwrap();
    let turn = f.registry.begin_turn(&f.store, 1, "s").unwrap();
    assert!(turn.prepare().await.is_ok());
    drop(turn);
    assert!(!f.dir.path().join("launches").exists());
    assert_eq!(
        f.store.cli_agent_turn("s", "r").unwrap().unwrap().state,
        cli_agent_turns::State::Rejected
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_disabled_and_shutdown_owners_cannot_create_or_return() {
    let f = Fixture::new("success").await;
    let prepared = f.launch.ownership.prepare().await.unwrap();
    let off = Registry::new(false);
    off.connect(1);
    assert!(matches!(
        off.create_approved(&f.store, 1, &prepared, "off", "off-key"),
        Err(OwnerError::Unavailable)
    ));
    assert!(f.store.cli_agent_binding("off").unwrap().is_none());
    let closed = Registry::new(true);
    closed.connect(1);
    closed.shutdown();
    closed.connect(1);
    assert!(matches!(
        closed.create_approved(&f.store, 1, &prepared, "closed", "closed-key"),
        Err(OwnerError::Unavailable)
    ));
    assert!(f.store.cli_agent_binding("closed").unwrap().is_none());
    f.registry.shutdown();
    assert!(f.launch.ownership.cancellation().is_cancelled());
    assert!(f.registry.has_turn("s"));
    assert_eq!(run(f.store.clone(), f.launch).await, Err(Error::Cancelled));
    assert!(!f.registry.has_turn("s"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_or_foreign_ownership_cannot_consume_another_root_lease() {
    let mut f = Fixture::new("success").await;
    let prepared = f.launch.ownership.prepare().await.unwrap();
    let binding = f
        .registry
        .create_approved(&f.store, 1, &prepared, "other", "other-key")
        .unwrap()
        .0;
    assert_ne!(
        binding.create.identity.cli_session_id,
        f.launch.ownership.binding().create.identity.cli_session_id
    );
    let other = f.registry.begin_turn(&f.store, 1, "other").unwrap();
    let original = std::mem::replace(&mut f.launch.ownership, other);
    assert_eq!(
        run(f.store.clone(), f.launch).await,
        Err(Error::Preparation)
    );
    assert!(!f.registry.has_turn("other"));
    assert!(
        f.registry.has_turn("s"),
        "foreign rejection must not release the legitimate owner"
    );
    assert_eq!(
        f.store.cli_agent_turn("s", "r").unwrap().unwrap().state,
        cli_agent_turns::State::Admitted
    );
    drop(original);
    assert!(!f.dir.path().join("launches").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_disconnect_session_revocation_and_shutdown_cancel_the_actual_owned_root() {
    for action in 0..5 {
        let f = Fixture::new("stop").await;
        let prepared = f.launch.ownership.prepare().await.unwrap();
        let binding = f.launch.ownership.binding().clone();
        let events = drain(f.events);
        let peer = tokio::spawn(client(f.peer, f.dir.path().into(), false));
        let root = tokio::spawn(run(f.store.clone(), f.launch));
        let ready = tokio::time::timeout(Duration::from_secs(10), async {
            while !f.dir.path().join("acknowledged").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        match action {
            0 => {
                assert!(f.registry.stop("s"));
            }
            1 => f.registry.revoke_owner(1),
            2 => f.registry.revoke_session("s"),
            3 => f.registry.shutdown(),
            _ => f.connection.cancel(),
        }
        assert!(root.await.unwrap().is_err());
        assert!(ready.is_ok(), "native Write did not finish");
        peer.await.unwrap();
        events.await.unwrap();
        assert!(!f.registry.has_turn("s"));
        assert_eq!(
            f.store.cli_agent_turn("s", "r").unwrap().unwrap().state,
            cli_agent_turns::State::Unknown
        );
        assert!(f.store.check_cli_agent_ready(&binding).is_err());
        f.registry.connect(1);
        assert!(
            f.registry
                .approve_return(&f.store, 1, &prepared, &binding)
                .is_err()
        );
        assert!(f.registry.begin_turn(&f.store, 1, "s").is_err());
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("effect")).unwrap(),
            "once"
        );
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("launches")).unwrap(),
            "spawn\n"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_identity_and_cancelled_turn_never_admit_work_or_overwrite_bindings() {
    let f = Fixture::new("success").await;
    let prepared = f.launch.ownership.prepare().await.unwrap();
    assert!(matches!(
        f.registry
            .create_approved(&f.store, 1, &prepared, "s", "changed-key"),
        Err(OwnerError::Identity)
    ));
    assert!(matches!(
        f.registry.begin_turn(&f.store, 1, "s"),
        Err(OwnerError::Busy)
    ));
    f.registry.stop("s");
    assert!(matches!(
        f.launch
            .ownership
            .admit(&f.store, "other-request", 1, "op", "text"),
        Err(OwnerError::Unavailable)
    ));
    assert!(
        f.store
            .cli_agent_turn("s", "other-request")
            .unwrap()
            .is_none()
    );
    assert_eq!(run(f.store.clone(), f.launch).await, Err(Error::Cancelled));
    assert_eq!(
        f.store
            .cli_agent_binding("s")
            .unwrap()
            .unwrap()
            .create
            .request_key,
        "create"
    );
}
