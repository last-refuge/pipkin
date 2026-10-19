//! Actual debug engine helper and actual native coding tools, in an owned temporary project.
//! No installed Claude CLI, account, provider inference, normal GUI or orchestration creator.
#![cfg(unix)]
use pipkin_engine::{
    execution::claude_cli::{
        orchestration_helper::{CHANNEL_ENV, FLAG},
        orchestration_protocol::{PROTOCOL_VERSION, Scope, Session},
    },
    runtime::{
        cli_bridge_channel::Server,
        cli_bridge_driver::{Services, serve},
    },
    store::{Store, cli_bridge::BridgeSpec},
    tools,
};
use serde_json::{Value, json};
use std::{process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    time::{Instant, timeout},
};
use tokio_util::sync::CancellationToken;
fn end() -> Instant {
    Instant::now() + Duration::from_secs(20)
}
fn call(id: u64, name: &str, args: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":args}})
}
async fn send(input: &mut tokio::process::ChildStdin, value: Value) {
    input
        .write_all(format!("{value}\n").as_bytes())
        .await
        .unwrap();
}
async fn reply(output: &mut BufReader<tokio::process::ChildStdout>) -> Value {
    let mut line = String::new();
    timeout(Duration::from_secs(10), output.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    serde_json::from_str(&line).unwrap()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_helper_executes_native_read_write_edit_bash_and_stop_reaps_owned_process() {
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::memory().unwrap());
    let binding = store
        .create_cli_agent_session(
            &pipkin_engine::store::cli_agent::Create {
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
    let run = store
        .tx(|tx| {
            tx.execute(
                "INSERT INTO runs(session,state,started_at) VALUES('s','running',1)",
                [],
            )?;
            Ok(tx.last_insert_rowid() as u64)
        })
        .unwrap();
    let spec = BridgeSpec {
        id: "native-driver-fixture".into(),
        scope: Scope {
            session: "s".into(),
            generation: 1,
            operation: "op".into(),
            run,
        },
        owner: "owner".into(),
        binding_hash: binding.digest,
        capability_hash: [0; 32],
    };
    let catalog_hash = Session::new(
        spec.scope.clone(),
        &tools::builtin_tools()
            .iter()
            .map(|t| t.spec())
            .collect::<Vec<_>>(),
    )
    .unwrap()
    .catalog_hash();
    let server = Server::bind_native(store.clone(), spec, catalog_hash, end())
        .await
        .unwrap();
    let launch = server.launch();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_pipkin-engine"))
        .arg(FLAG)
        .env(CHANNEL_ENV, launch.environment_value())
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path())
        .env("XDG_DATA_HOME", home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let connected = server.accept().await.unwrap();
    let cancel = CancellationToken::new();
    let driver = tokio::spawn(serve(
        store.clone(),
        connected,
        tools::builtin_tools(),
        Services::default(),
        cancel.clone(),
        end(),
    ));
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    send(&mut input,json!({"jsonrpc":"2.0","id":"init","method":"initialize","params":{"protocolVersion":PROTOCOL_VERSION,"clientInfo":{"name":"native-fixture","version":"1"},"capabilities":{}}})).await;
    assert!(reply(&mut output).await.get("result").is_some());
    send(
        &mut input,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    )
    .await;
    let write = call(1, "write", json!({"path":"file.txt","content":"one"}));
    send(&mut input, write.clone()).await;
    let original = reply(&mut output).await;
    assert_eq!(original["result"]["isError"], false);
    std::fs::write(dir.path().join("file.txt"), "outside edit").unwrap();
    send(&mut input, write).await;
    assert_eq!(reply(&mut output).await, original);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        "outside edit"
    );
    send(
        &mut input,
        call(
            2,
            "edit",
            json!({"path":"file.txt","edits":[{"oldText":"outside edit","newText":"two"}]}),
        ),
    )
    .await;
    assert_eq!(reply(&mut output).await["result"]["isError"], false);
    send(&mut input, call(3, "read", json!({"path":"file.txt"}))).await;
    assert!(
        reply(&mut output).await["result"]["content"]
            .to_string()
            .contains("two")
    );
    send(
        &mut input,
        call(
            4,
            "bash",
            json!({"command":"printf '%s/%s' \"$PI_PROVIDER\" \"$PI_MODEL\""}),
        ),
    )
    .await;
    assert!(
        reply(&mut output).await["result"]["content"]
            .to_string()
            .contains("claude-cli/sonnet")
    );
    assert_eq!(
        store
            .read(|c| Store::tools_in_state(c, "s", "done"))
            .unwrap()
            .len(),
        4
    );
    assert_eq!(store.entry_count("s").unwrap(), 8);
    // A real native bash process, not CLI inference. The marker and PID are owned by this test.
    send(
        &mut input,
        call(
            5,
            "bash",
            json!({"command":"printf '%s' \"$$\" > started; exec sleep 30"}),
        ),
    )
    .await;
    let pid: i32 = timeout(Duration::from_secs(5), async {
        loop {
            if let Some(pid) = std::fs::read_to_string(dir.path().join("started"))
                .ok()
                .and_then(|v| v.parse::<i32>().ok())
                .filter(|p| *p > 1)
            {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    assert!(
        timeout(Duration::from_secs(8), driver)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    // SAFETY: signal 0 observes existence only; it sends no signal to any process.
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    let unknown = store
        .read(|c| Store::tools_in_state(c, "s", "interrupted"))
        .unwrap();
    assert_eq!(unknown.len(), 1);
    assert!(unknown[0].result_entry.is_none());
    assert_eq!(store.entry_count("s").unwrap(), 9);
    drop(input);
    let result = timeout(Duration::from_secs(5), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(result.stderr.is_empty());
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
}
