//! Actual debug helper process + authenticated loopback + protocol + journal.
//! No installed CLI, account, provider inference or native tool execution. The one catalog
//! entry is explicitly a no-effect fixture, not a claim of native orchestration parity.
#![cfg(unix)]
use pipkin_engine::{
    execution::claude_cli::{
        orchestration_helper::{CHANNEL_ENV, FLAG},
        orchestration_protocol::{Incoming, PROTOCOL_VERSION, Scope, Session},
    },
    provider::ToolSpec,
    runtime::cli_bridge_channel::{Launch, Server, retire},
    store::{
        Store,
        cli_bridge::{Admission, BridgeSpec, ORCHESTRATION_BACKEND, Settlement},
    },
};
use serde_json::{Value, json};
use std::{process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    time::{Instant, timeout},
};
fn end() -> Instant {
    Instant::now() + Duration::from_secs(10)
}
async fn fixture() -> (Arc<Store>, Server) {
    let store = Arc::new(Store::memory().unwrap());
    let run=store.tx(|tx| {
        tx.execute("INSERT INTO sessions(id,cwd,created_at,updated_at,origin,execution_backend) VALUES('s','/fixture',1,1,'{}',?1)",[ORCHESTRATION_BACKEND])?;
        tx.execute("INSERT INTO runs(session,state,started_at) VALUES('s','running',1)",[])?;Ok(tx.last_insert_rowid() as u64)
    }).unwrap();
    let spec = BridgeSpec {
        id: "bridge".into(),
        scope: Scope {
            session: "s".into(),
            generation: 1,
            operation: "op".into(),
            run,
        },
        owner: "owner".into(),
        binding_hash: [1; 32],
        capability_hash: [0; 32],
    };
    let server = Server::bind(store.clone(), spec, end()).await.unwrap();
    (store, server)
}
fn child(launch: &Launch, home: &std::path::Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_pipkin-engine"));
    command
        .arg(FLAG)
        .env(CHANNEL_ENV, launch.environment_value())
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_relays_mcp_and_committed_fixture_receipts_without_startup_or_tools() {
    let (store, server) = fixture().await;
    let identity = server.identity().clone();
    let launch = server.launch();
    let home = tempfile::tempdir().unwrap();
    let mut child = child(&launch, home.path()).spawn().unwrap();
    let mut host = timeout(Duration::from_secs(5), server.accept())
        .await
        .unwrap()
        .unwrap()
        .into_parts();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut protocol = Session::new(
        identity.scope.clone(),
        &[ToolSpec {
            name: "fixture_no_effect".into(),
            description: "Simulated, not a native tool".into(),
            parameters: json!({"type":"object"}),
            constrained_sampling: None,
        }],
    )
    .unwrap();
    let mut effects = 0;
    let requests = [
        json!({"jsonrpc":"2.0","id":"init","method":"initialize","params":{"protocolVersion":PROTOCOL_VERSION,"capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":"call","method":"tools/call","params":{"name":"fixture_no_effect","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":"call","method":"tools/call","params":{"name":"fixture_no_effect","arguments":{}}}),
    ];
    for request in requests {
        input
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
        let frame = host.reader.read(end()).await.unwrap();
        let response = match protocol.receive(&frame) {
            Incoming::Ignore => continue,
            Incoming::Response(v) => v,
            Incoming::Call {
                key,
                name,
                arguments,
            } => {
                let Admission::New(claim) = store
                    .admit_cli_bridge_call(&host.lease, &key.request_id(), &name, &arguments, 2)
                    .unwrap()
                else {
                    panic!("new fixture call")
                };
                effects += 1;
                let result =
                    json!({"content":[{"type":"text","text":"simulated bridge response"}]});
                assert_eq!(
                    store
                        .settle_cli_bridge_call(&host.lease, &claim, &result, 3)
                        .unwrap(),
                    Settlement::Committed
                );
                protocol.complete(&key, result).unwrap()
            }
            Incoming::Replay(key) => {
                let Admission::Completed(result) = store
                    .admit_cli_bridge_call(
                        &host.lease,
                        &key.request_id(),
                        "fixture_no_effect",
                        &json!({}),
                        4,
                    )
                    .unwrap()
                else {
                    panic!("receipt")
                };
                protocol.replay(&key, result).unwrap()
            }
            other => panic!("{other:?}"),
        };
        host.writer
            .write(&serde_json::to_vec(&response).unwrap(), end())
            .await
            .unwrap();
        let mut line = String::new();
        timeout(Duration::from_secs(5), output.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap(), response);
    }
    assert_eq!(effects, 1);
    drop(input);
    let closed_input: Value =
        serde_json::from_slice(&host.reader.read(end()).await.unwrap()).unwrap();
    assert_eq!(
        closed_input,
        json!({"pipkinNativeBridgeControl":"input-closed-v1"})
    );
    assert!(protocol.is_quiescent());
    assert!(store.close_cli_tool_bridge(&host.lease, 5).unwrap());
    host.writer
        .write(
            &serde_json::to_vec(&json!({"pipkinNativeBridgeControl":"closed-v1"})).unwrap(),
            end(),
        )
        .await
        .unwrap();
    let output = timeout(Duration::from_secs(5), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
    assert_eq!(
        store
            .read(
                |c| Ok(c.query_row("SELECT COUNT(*) FROM tool_calls", [], |r| r
                    .get::<_, i64>(0))?)
            )
            .unwrap(),
        0
    );
}
#[tokio::test]
async fn malformed_configuration_and_extra_arguments_fail_without_output_or_profile_access() {
    let home = tempfile::tempdir().unwrap();
    for extra in [false, true] {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_pipkin-engine"));
        command.arg(FLAG);
        if extra {
            command.arg("unexpected");
        }
        let child = command
            .env(CHANNEL_ENV, "invalid")
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path())
            .env("XDG_DATA_HOME", home.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let result = timeout(Duration::from_secs(5), child.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.status.code(), Some(2));
        assert!(result.stdout.is_empty());
        assert!(result.stderr.is_empty());
    }
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
}
#[tokio::test]
async fn redirected_regular_file_stdout_is_not_written_and_never_claims() {
    let (store, server) = fixture().await;
    let identity = server.identity().clone();
    let launch = server.launch();
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("output");
    std::fs::write(&path, b"sentinel").unwrap();
    let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    let child = child(&launch, home.path()).stdout(file).spawn().unwrap();
    let result = timeout(Duration::from_secs(5), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert_eq!(std::fs::read(path).unwrap(), b"sentinel");
    assert_eq!(
        store
            .read(
                |c| Ok(c.query_row("SELECT state FROM cli_tool_bridges", [], |r| r
                    .get::<_, String>(0))?)
            )
            .unwrap(),
        "prepared"
    );
    drop(server);
    retire(store, identity).await.unwrap();
}
