use super::*;
use crate::{execution::claude_cli::orchestration_control::Control, store::cli_agent_turns};
struct Fixture {
    store: Arc<Store>,
    client: Client,
    cancel: CancellationToken,
    handle: CloseHandle,
    scope: Scope,
    root: cli_agent_turns::Lease,
    task: tokio::task::JoinHandle<Result<(), DriverError>>,
}
async fn fixture(dir: &std::path::Path, tools: Vec<Arc<dyn Tool>>) -> Fixture {
    let store = Arc::new(Store::memory().unwrap());
    let binding = store
        .create_cli_agent_session(
            &crate::store::cli_agent::tests::create("s", dir.to_str().unwrap()),
            1,
        )
        .unwrap()
        .0;
    let root = store
        .admit_cli_agent_turn(
            &cli_agent_turns::Input {
                images: &[],
                mode: pipkin_engine_protocol::InputMode::Prompt,
                session: "s",
                request: "r",
                generation: 1,
                operation: "op",
                binding: binding.digest,
                text: "offline root fixture",
            },
            "owner",
            2,
        )
        .unwrap()
        .lease
        .unwrap();
    let scope = root.receipt().scope.clone();
    let catalog = protocol::Session::new(
        scope.clone(),
        &tools.iter().map(|t| t.spec()).collect::<Vec<_>>(),
    )
    .unwrap()
    .catalog_hash();
    let server = Server::bind_native(
        store.clone(),
        BridgeSpec {
            id: "b".into(),
            scope: scope.clone(),
            owner: "owner".into(),
            binding_hash: binding.digest,
            capability_hash: [0; 32],
        },
        catalog,
        end(),
    )
    .await
    .unwrap();
    store.begin_cli_agent_launch(&root, "b", 3).unwrap();
    store.mark_cli_agent_running(&root, 4).unwrap();
    let launch = server.launch();
    let (host, client) = tokio::join!(server.accept(), launch.connect(end()));
    let cancel = CancellationToken::new();
    let (handle, request) = close_channel(scope.clone());
    let task = tokio::spawn(serve_controlled(
        store.clone(),
        host.unwrap(),
        tools,
        Services::default(),
        cancel.clone(),
        end(),
        request,
    ));
    Fixture {
        store,
        client: client.unwrap().split(),
        cancel,
        handle,
        scope,
        root,
        task,
    }
}
async fn ready(f: &mut Fixture) {
    send(&mut f.client.1,json!({"jsonrpc":"2.0","id":"init","method":"initialize","params":{"protocolVersion":PROTOCOL_VERSION,"clientInfo":{"name":"fixture","version":"1"},"capabilities":{}}})).await;
    receive(&mut f.client.0).await;
    send(
        &mut f.client.1,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    )
    .await;
    send(
        &mut f.client.1,
        json!({"jsonrpc":"2.0","id":"catalog","method":"tools/list"}),
    )
    .await;
    receive(&mut f.client.0).await;
}
fn state(s: &Store) -> String {
    s.read(|c| {
        Ok(
            c.query_row("SELECT state FROM cli_tool_bridges WHERE id='b'", [], |r| {
                r.get(0)
            })?,
        )
    })
    .unwrap()
}
#[test]
fn dropping_owner_synchronously_revokes_dispatch_before_receiver_poll() {
    let (handle, mut request) = close_channel(Scope {
        session: "s".into(),
        generation: 1,
        operation: "op".into(),
        run: 1,
    });
    assert!(!request.requested.is_cancelled());
    drop(handle);
    assert!(request.requested.is_cancelled());
    assert!(matches!(
        request.receiver.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Closed)
    ));
}
#[tokio::test]
async fn owned_close_is_scoped_acknowledged_and_not_root_success() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = fixture(dir.path(), tools::builtin_tools()).await;
    ready(&mut f).await;
    send(
        &mut f.client.1,
        call(1, "write", json!({"path":"effect","content":"once"})),
    )
    .await;
    receive(&mut f.client.0).await;
    let mut stale = f.scope.clone();
    stale.operation = "old-op".into();
    assert!(matches!(
        f.handle.finish(&stale).await,
        Err(DriverError::Protocol)
    ));
    assert!(!f.task.is_finished());
    let (closed, ()) = tokio::join!(f.handle.finish(&f.scope), async {
        assert_eq!(receive(&mut f.client.0).await, Control::Quiesce.value());
        send(&mut f.client.1, Control::Quiesced.value()).await;
        assert_eq!(receive(&mut f.client.0).await, Control::Closed.value());
    });
    assert_eq!(closed.unwrap().scope(), &f.scope);
    assert_eq!(f.task.await.unwrap(), Ok(()));
    assert_eq!(state(&f.store), "closed");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("effect")).unwrap(),
        "once"
    );
    assert_eq!(
        f.store.cli_agent_turn("s", "r").unwrap().unwrap().state,
        cli_agent_turns::State::Running
    );
    let evidence = cli_agent_turns::Completion {
        cli_session_id: "11111111-1111-4111-8111-111111111111".into(),
        terminal: crate::store::ExternalTerminal {
            metadata: None,
            outcome: crate::store::ExternalTerminalOutcome::Success,
            usage: None,
        },
        supervisor: Default::default(),
    };
    assert!(
        f.store
            .complete_cli_agent_turn(&f.root, &evidence, 10)
            .is_err()
    );
    assert!(f.handle.finish(&f.scope).await.is_err());
}
#[tokio::test]
async fn early_input_close_refuses_unsettled_work_and_late_stop() {
    for busy in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let started = Arc::new(tokio::sync::Notify::new());
        let mut tools = tools::builtin_tools();
        tools.push(Arc::new(Waiting {
            started: started.clone(),
        }));
        let mut f = fixture(dir.path(), tools).await;
        ready(&mut f).await;
        if busy {
            send(&mut f.client.1, call(1, "waiting", json!({}))).await;
            started.notified().await;
        }
        send(&mut f.client.1, Control::InputClosed.value()).await;
        if busy {
            assert_eq!(f.task.await.unwrap(), Err(DriverError::Busy));
            assert_eq!(state(&f.store), "fenced");
        } else {
            assert_eq!(receive(&mut f.client.0).await, Control::Closed.value());
            assert!(!f.task.is_finished());
            f.cancel.cancel();
            assert_eq!(f.task.await.unwrap(), Err(DriverError::Cancelled));
        }
        assert!(f.handle.finish(&f.scope).await.is_err());
        assert_ne!(
            f.store.cli_agent_turn("s", "r").unwrap().unwrap().state,
            cli_agent_turns::State::Completed
        );
    }
}
#[tokio::test]
async fn input_eof_racing_with_owner_quiescence_has_one_close_ack() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = fixture(dir.path(), tools::builtin_tools()).await;
    ready(&mut f).await;
    let (closed, ()) = tokio::join!(f.handle.finish(&f.scope), async {
        assert_eq!(receive(&mut f.client.0).await, Control::Quiesce.value());
        send(&mut f.client.1, Control::InputClosed.value()).await;
        assert_eq!(receive(&mut f.client.0).await, Control::Closed.value());
    });
    assert_eq!(closed.unwrap().scope(), &f.scope);
    assert_eq!(f.task.await.unwrap(), Ok(()));
    assert_eq!(state(&f.store), "closed");
}
#[tokio::test]
async fn finish_during_native_work_fences_pending_and_never_runs_queued_write() {
    let dir = tempfile::tempdir().unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let mut tools = tools::builtin_tools();
    tools.push(Arc::new(Waiting {
        started: started.clone(),
    }));
    let mut f = fixture(dir.path(), tools).await;
    ready(&mut f).await;
    send(&mut f.client.1, call(1, "waiting", json!({}))).await;
    started.notified().await;
    send(
        &mut f.client.1,
        call(2, "write", json!({"path":"never","content":"no"})),
    )
    .await;
    assert!(matches!(
        f.handle.finish(&f.scope).await,
        Err(DriverError::Busy)
    ));
    assert_eq!(f.task.await.unwrap(), Err(DriverError::Busy));
    assert_eq!(state(&f.store), "fenced");
    assert!(!dir.path().join("never").exists());
    assert_eq!(
        f.store
            .read(|c| Store::tools_in_state(c, "s", "interrupted"))
            .unwrap()
            .len(),
        1
    );
}
#[tokio::test]
async fn initialization_and_explicit_owner_lifetime_are_required() {
    for drop_owner in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut f = fixture(dir.path(), tools::builtin_tools()).await;
        if drop_owner {
            drop(f.handle);
        } else {
            assert!(matches!(
                f.handle.finish(&f.scope).await,
                Err(DriverError::Busy)
            ));
        }
        assert!(f.task.await.unwrap().is_err());
        assert_eq!(state(&f.store), "fenced");
        assert_eq!(f.store.entry_count("s").unwrap(), 1);
    }
}
#[tokio::test]
async fn malformed_ack_or_commit_failure_cannot_close() {
    for bad_ack in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let mut f = fixture(dir.path(), tools::builtin_tools()).await;
        ready(&mut f).await;
        let (closed, ()) = tokio::join!(f.handle.finish(&f.scope), async {
            assert_eq!(receive(&mut f.client.0).await, Control::Quiesce.value());
            if !bad_ack {
                f.store.fail_next_write();
            }
            send(
                &mut f.client.1,
                if bad_ack {
                    json!({"jsonrpc":"2.0","id":99,"method":"ping"})
                } else {
                    Control::Quiesced.value()
                },
            )
            .await;
            assert!(f.client.0.read(end()).await.is_err());
        });
        assert!(closed.is_err());
        assert!(f.task.await.unwrap().is_err());
        assert_eq!(state(&f.store), "fenced");
    }
}
#[tokio::test]
async fn stop_wins_while_waiting_for_helper_quiescence() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = fixture(dir.path(), tools::builtin_tools()).await;
    ready(&mut f).await;
    let (closed, ()) = tokio::join!(f.handle.finish(&f.scope), async {
        assert_eq!(receive(&mut f.client.0).await, Control::Quiesce.value());
        f.store.request_stop("s", 7).unwrap();
        f.cancel.cancel();
    });
    assert!(closed.is_err());
    assert!(f.task.await.unwrap().is_err());
    assert_eq!(state(&f.store), "fenced");
}
#[tokio::test]
async fn missing_helper_ack_is_deadline_bounded_and_not_success() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = fixture(dir.path(), tools::builtin_tools()).await;
    ready(&mut f).await;
    let (closed, ()) = tokio::join!(f.handle.finish(&f.scope), async {
        assert_eq!(receive(&mut f.client.0).await, Control::Quiesce.value());
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(11)).await;
    });
    assert!(closed.is_err());
    assert!(f.task.await.unwrap().is_err());
    assert_eq!(state(&f.store), "fenced");
}
