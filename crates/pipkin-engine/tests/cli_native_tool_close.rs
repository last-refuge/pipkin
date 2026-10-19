//! Actual debug helper/native Write with owner-controlled close. No installed CLI or inference.
#![cfg(unix)]
use pipkin_engine::{
    execution::claude_cli::{
        orchestration_helper::{CHANNEL_ENV, FLAG},
        orchestration_protocol::{PROTOCOL_VERSION, Scope, Session},
    },
    runtime::{
        cli_bridge_channel::Server,
        cli_bridge_driver::{self, CloseHandle, DriverError, Services},
    },
    store::{Store, cli_agent, cli_agent_turns, cli_bridge::BridgeSpec},
    tools,
};
use serde_json::{Value, json};
use std::{process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    time::{Instant, timeout},
};
use tokio_util::sync::CancellationToken;
fn end() -> Instant {
    Instant::now() + Duration::from_secs(20)
}
struct Fixture {
    dir: tempfile::TempDir,
    store: Arc<Store>,
    scope: Scope,
    root: cli_agent_turns::Lease,
    handle: CloseHandle,
    cancel: CancellationToken,
    child: tokio::process::Child,
    input: tokio::process::ChildStdin,
    output: BufReader<tokio::process::ChildStdout>,
    task: tokio::task::JoinHandle<Result<(), DriverError>>,
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::memory().unwrap());
        let binding = store
            .create_cli_agent_session(
                &cli_agent::Create {
                    session: "s".into(),
                    request_key: "create-s".into(),
                    model: "sonnet".into(),
                    thinking: None,
                    parent: None,
                    identity: pipkin_engine::store::CliBindingSpec {
                        cli_session_id: "11111111-1111-4111-8111-111111111111".into(),
                        project_path: dir.path().to_str().unwrap().into(),
                        executable_path: "/fixture/claude".into(),
                        cli_version: "2.1.301".into(),
                        config_ref: "fixture".into(),
                        account_scope: "fixture".into(),
                        launch_policy: "cli-native-orchestration-v1".into(),
                    },
                },
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
                    text: "offline fixture only",
                },
                "owner",
                2,
            )
            .unwrap()
            .lease
            .unwrap();
        let scope = root.receipt().scope.clone();
        let offered = tools::builtin_tools();
        let catalog = Session::new(
            scope.clone(),
            &offered.iter().map(|t| t.spec()).collect::<Vec<_>>(),
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
        let launch = server.launch();
        store.begin_cli_agent_launch(&root, "b", 3).unwrap();
        store.mark_cli_agent_running(&root, 4).unwrap();
        let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_pipkin-engine"))
            .arg(FLAG)
            .env_clear()
            .env("HOME", dir.path())
            .env("PATH", "/usr/bin:/bin")
            .env(CHANNEL_ENV, launch.environment_value())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let connected = timeout(Duration::from_secs(5), server.accept())
            .await
            .unwrap()
            .unwrap();
        let (handle, request) = cli_bridge_driver::close_channel(scope.clone());
        let cancel = CancellationToken::new();
        let task = tokio::spawn(cli_bridge_driver::serve_controlled(
            store.clone(),
            connected,
            offered,
            Services::default(),
            cancel.clone(),
            end(),
            request,
        ));
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        Self {
            dir,
            store,
            scope,
            root,
            handle,
            cancel,
            child,
            input,
            output,
            task,
        }
    }
    async fn send(&mut self, v: Value) {
        self.input
            .write_all(format!("{v}\n").as_bytes())
            .await
            .unwrap();
    }
    async fn reply(&mut self) -> Value {
        let mut line = String::new();
        timeout(Duration::from_secs(5), self.output.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        serde_json::from_str(&line).unwrap()
    }
    async fn initialize(&mut self) {
        self.send(json!({"jsonrpc":"2.0","id":"init","method":"initialize","params":{"protocolVersion":PROTOCOL_VERSION,"clientInfo":{"name":"offline-fixture","version":"1"},"capabilities":{}}})).await;
        self.reply().await;
        self.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await;
        self.send(json!({"jsonrpc":"2.0","id":"list","method":"tools/list"}))
            .await;
        self.reply().await;
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_helper_cleanly_quiesces_without_printing_fake_results_or_completing_root() {
    let mut f = Fixture::new().await;
    f.initialize().await;
    f.send(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"write","arguments":{"path":"effect","content":"once"}}})).await;
    assert_eq!(f.reply().await["result"]["isError"], false);
    assert_eq!(f.handle.finish(&f.scope).await.unwrap().scope(), &f.scope);
    assert_eq!(f.task.await.unwrap(), Ok(()));
    let mut remaining = String::new();
    timeout(
        Duration::from_secs(5),
        f.output.read_to_string(&mut remaining),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(remaining.is_empty());
    assert!(
        timeout(Duration::from_secs(5), f.child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    let mut stderr = String::new();
    f.child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .await
        .unwrap();
    assert!(stderr.is_empty());
    assert_eq!(
        std::fs::read_to_string(f.dir.path().join("effect")).unwrap(),
        "once"
    );
    assert_eq!(
        f.store.cli_agent_turn("s", "r").unwrap().unwrap().state,
        cli_agent_turns::State::Running
    );
    // No CLI root was launched/reaped by this fixture: closed MCP alone cannot be success.
    f.store.abandon_cli_agent_turn(&f.root, 10).unwrap();
    assert_eq!(
        f.store.cli_agent_turn("s", "r").unwrap().unwrap().state,
        cli_agent_turns::State::Unknown
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_helper_stdin_eof_closes_tools_but_waits_for_owned_model_terminal() {
    for signal in [None, Some(libc::SIGTERM)] {
        assert_client_shutdown(signal).await;
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_helper_sigint_closes_tools_but_waits_for_owned_model_terminal() {
    assert_client_shutdown(Some(libc::SIGINT)).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_helper_sigint_cannot_override_owner_stop() {
    let mut f = Fixture::new().await;
    f.initialize().await;
    f.send(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"write","arguments":{"path":"effect","content":"once"}}})).await;
    assert_eq!(f.reply().await["result"]["isError"], false);
    f.cancel.cancel();
    // Signal only this fixture's owned, unreaped helper. Stop already revoked authority.
    assert_eq!(
        unsafe { libc::kill(f.child.id().unwrap() as i32, libc::SIGINT) },
        0
    );
    assert!(
        timeout(Duration::from_secs(5), f.task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert!(f.handle.finish(&f.scope).await.is_err());
    timeout(Duration::from_secs(5), f.child.wait())
        .await
        .unwrap()
        .unwrap();
    f.store.abandon_cli_agent_turn(&f.root, 10).unwrap();
    assert_eq!(
        f.store.cli_agent_turn("s", "r").unwrap().unwrap().state,
        cli_agent_turns::State::Unknown
    );
    assert_eq!(
        std::fs::read_to_string(f.dir.path().join("effect")).unwrap(),
        "once"
    );
}
async fn assert_client_shutdown(signal: Option<i32>) {
    for terminal in [true, false] {
        let mut f = Fixture::new().await;
        f.initialize().await;
        f.send(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"write","arguments":{"path":"effect","content":"once"}}})).await;
        assert_eq!(f.reply().await["result"]["isError"], false);
        let retained_input = if let Some(signal) = signal {
            assert_eq!(
                unsafe { libc::kill(f.child.id().unwrap() as i32, signal) },
                0
            );
            Some(f.input)
        } else {
            drop(f.input);
            None
        };
        let status = timeout(Duration::from_secs(5), f.child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(
            status.success(),
            "helper shutdown for {signal:?}: {status:?}"
        );
        drop(retained_input);
        let mut output = String::new();
        f.output.read_to_string(&mut output).await.unwrap();
        assert!(output.is_empty());
        assert!(
            !f.task.is_finished(),
            "helper closure is not a model terminal"
        );
        assert_eq!(
            f.store.cli_agent_turn("s", "r").unwrap().unwrap().state,
            cli_agent_turns::State::Running
        );
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("effect")).unwrap(),
            "once"
        );
        if terminal {
            assert_eq!(f.handle.finish(&f.scope).await.unwrap().scope(), &f.scope);
            assert_eq!(f.task.await.unwrap(), Ok(()));
        } else {
            drop(f.handle);
            assert!(
                timeout(Duration::from_secs(5), f.task)
                    .await
                    .unwrap()
                    .unwrap()
                    .is_err()
            );
        }
        f.store.abandon_cli_agent_turn(&f.root, 10).unwrap();
        assert_eq!(
            f.store.cli_agent_turn("s", "r").unwrap().unwrap().state,
            cli_agent_turns::State::Unknown
        );
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn partial_mcp_input_eof_cannot_be_a_clean_close() {
    let mut f = Fixture::new().await;
    f.initialize().await;
    f.input.write_all(b"{\"jsonrpc\":\"2.0\"").await.unwrap();
    drop(f.input);
    assert!(
        !timeout(Duration::from_secs(5), f.child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    assert!(f.task.await.unwrap().is_err());
    assert!(f.handle.finish(&f.scope).await.is_err());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_stdin_cannot_spoof_private_helper_close_acknowledgments() {
    for control in ["quiesced-v1", "input-closed-v1"] {
        let mut f = Fixture::new().await;
        f.initialize().await;
        f.send(json!({"pipkinNativeBridgeControl":control})).await;
        assert!(
            !timeout(Duration::from_secs(5), f.child.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
        assert!(f.task.await.unwrap().is_err());
        assert!(f.handle.finish(&f.scope).await.is_err());
        assert_eq!(f.store.entry_count("s").unwrap(), 1);
        let mut output = String::new();
        f.output.read_to_string(&mut output).await.unwrap();
        assert!(output.is_empty());
    }
}
