use super::*;
use crate::{
    execution::claude_cli::{
        orchestration_protocol::PROTOCOL_VERSION,
        orchestration_wire::{Reader, Writer},
    },
    runtime::cli_bridge_channel::Server,
    store::{ToolState, cli_bridge::BridgeSpec},
};
use serde_json::json;
mod close;
use tokio::{
    io::{ReadHalf, WriteHalf},
    net::TcpStream,
};
type Client = (Reader<ReadHalf<TcpStream>>, Writer<WriteHalf<TcpStream>>);
fn end() -> Instant {
    Instant::now() + Duration::from_secs(15)
}
async fn start_raw(
    dir: &std::path::Path,
    tools: Vec<Arc<dyn Tool>>,
    native: bool,
) -> (
    Arc<Store>,
    Client,
    CancellationToken,
    tokio::task::JoinHandle<Result<(), DriverError>>,
) {
    let s = Arc::new(Store::memory().unwrap());
    let binding = s
        .create_cli_agent_session(
            &crate::store::cli_agent::tests::create("s", dir.to_str().unwrap()),
            1,
        )
        .unwrap()
        .0;
    let run = s
        .tx(|tx| {
            tx.execute(
                "INSERT INTO runs(session,state,started_at) VALUES('s','running',1)",
                [],
            )?;
            Ok(tx.last_insert_rowid() as u64)
        })
        .unwrap();
    let spec = BridgeSpec {
        id: "b".into(),
        scope: Scope {
            session: "s".into(),
            generation: 1,
            operation: "op".into(),
            run,
        },
        owner: "owner".into(),
        binding_hash: binding.digest,
        capability_hash: [2; 32],
    };
    let catalog_hash = protocol::Session::new(
        spec.scope.clone(),
        &tools.iter().map(|t| t.spec()).collect::<Vec<_>>(),
    )
    .unwrap()
    .catalog_hash();
    let server = if native {
        Server::bind_native(s.clone(), spec, catalog_hash, end())
            .await
            .unwrap()
    } else {
        Server::bind(s.clone(), spec, end()).await.unwrap()
    };
    let launch = server.launch();
    let (host, client) = tokio::join!(server.accept(), launch.connect(end()));
    let cancel = CancellationToken::new();
    let task = tokio::spawn(serve(
        s.clone(),
        host.unwrap(),
        tools,
        Services::default(),
        cancel.clone(),
        end(),
    ));
    (s, client.unwrap().split(), cancel, task)
}
async fn start(
    dir: &std::path::Path,
    tools: Vec<Arc<dyn Tool>>,
) -> (
    Arc<Store>,
    Client,
    CancellationToken,
    tokio::task::JoinHandle<Result<(), DriverError>>,
) {
    let (s, mut client, cancel, task) = start_raw(dir, tools, true).await;
    send(&mut client.1,json!({"jsonrpc":"2.0","id":"init","method":"initialize","params":{"protocolVersion":PROTOCOL_VERSION,"clientInfo":{"name":"fixture","version":"1"},"capabilities":{}}})).await;
    receive(&mut client.0).await;
    send(
        &mut client.1,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    )
    .await;
    (s, client, cancel, task)
}
async fn send(w: &mut Writer<WriteHalf<TcpStream>>, v: Value) {
    w.write(&serde_json::to_vec(&v).unwrap(), end())
        .await
        .unwrap();
}
async fn receive(r: &mut Reader<ReadHalf<TcpStream>>) -> Value {
    serde_json::from_slice(&r.read(end()).await.unwrap()).unwrap()
}
fn call(id: u64, name: &str, args: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":args}})
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_write_read_and_receipt_replay_use_normal_tool_records_without_native_api() {
    let dir = tempfile::tempdir().unwrap();
    let (s, (mut r, mut w), cancel, task) = start(dir.path(), tools::builtin_tools()).await;
    let write = call(
        1,
        "write",
        json!({"path":"file.txt","content":"native effect"}),
    );
    send(&mut w, write.clone()).await;
    let original = receive(&mut r).await;
    assert_eq!(original["result"]["isError"], false);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        "native effect"
    );
    std::fs::write(dir.path().join("file.txt"), "external edit").unwrap();
    send(&mut w, write).await;
    assert_eq!(receive(&mut r).await, original);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        "external edit"
    );
    send(&mut w, call(2, "read", json!({"path":"file.txt"}))).await;
    assert!(
        receive(&mut r).await["result"]["content"]
            .to_string()
            .contains("external edit")
    );
    assert_eq!(s.entry_count("s").unwrap(), 4);
    assert!(s.read(|c| Store::assert_native_execution(c, "s")).is_err());
    let states = s.read(|c| Store::tools_in_state(c, "s", "done")).unwrap();
    assert_eq!(states.len(), 2);
    assert!(states.iter().all(|t| !t.replay_safe));
    cancel.cancel();
    assert!(task.await.unwrap().is_err());
    assert!(
        states
            .iter()
            .all(|t| s.tool_call("s", &t.call_id).unwrap().unwrap().state == ToolState::Done)
    );
}
#[tokio::test]
async fn schema_errors_are_native_error_results_and_unknown_tools_have_no_intent() {
    let dir = tempfile::tempdir().unwrap();
    let (s, (mut r, mut w), cancel, task) = start(dir.path(), tools::builtin_tools()).await;
    send(&mut w, call(1, "write", json!({}))).await;
    assert_eq!(receive(&mut r).await["result"]["isError"], true);
    send(&mut w, call(2, "invented", json!({}))).await;
    assert!(receive(&mut r).await.get("error").is_some());
    assert_eq!(s.entry_count("s").unwrap(), 2);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    cancel.cancel();
    assert!(task.await.unwrap().is_err());
}
#[tokio::test]
async fn failed_intent_commit_executes_nothing_and_fences_the_lifetime() {
    let dir = tempfile::tempdir().unwrap();
    let (s, (mut r, mut w), _cancel, task) = start(dir.path(), tools::builtin_tools()).await;
    s.fail_next_write();
    send(
        &mut w,
        call(1, "write", json!({"path":"never","content":"no"})),
    )
    .await;
    assert!(r.read(end()).await.is_err());
    assert_eq!(task.await.unwrap(), Err(DriverError::Storage));
    assert!(!dir.path().join("never").exists());
    assert_eq!(s.entry_count("s").unwrap(), 0);
}
struct ChangingCatalog(std::sync::atomic::AtomicUsize);
impl Tool for ChangingCatalog {
    fn spec(&self) -> crate::provider::ToolSpec {
        let mut spec = tools::write::WriteTool.spec();
        if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0 {
            spec.description.push_str(" changed");
        }
        spec
    }
    fn execute<'a>(
        &'a self,
        _: Value,
        _: &'a ToolCtx,
    ) -> crate::provider::BoxFuture<'a, Result<tools::ToolOutput, tools::ToolError>> {
        Box::pin(async { panic!("unapproved catalog must never execute") })
    }
}
#[tokio::test]
async fn actual_catalog_must_match_the_durable_owner_catalog_binding() {
    let dir = tempfile::tempdir().unwrap();
    let (s, (mut r, _w), _cancel, task) = start_raw(
        dir.path(),
        vec![Arc::new(ChangingCatalog(
            std::sync::atomic::AtomicUsize::new(0),
        ))],
        true,
    )
    .await;
    assert_eq!(task.await.unwrap(), Err(DriverError::Setup));
    assert!(r.read(end()).await.is_err());
    assert_eq!(s.entry_count("s").unwrap(), 0);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn protocol_only_connection_has_no_native_dispatch_handoff() {
    let dir = tempfile::tempdir().unwrap();
    let (s, (mut r, _w), _cancel, task) =
        start_raw(dir.path(), tools::builtin_tools(), false).await;
    assert_eq!(task.await.unwrap(), Err(DriverError::Setup));
    assert!(r.read(end()).await.is_err());
    assert_eq!(s.entry_count("s").unwrap(), 0);
}

struct FailAfterWrite {
    store: Arc<std::sync::OnceLock<Arc<Store>>>,
}
impl Tool for FailAfterWrite {
    fn spec(&self) -> crate::provider::ToolSpec {
        tools::write::WriteTool.spec()
    }
    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolCtx,
    ) -> crate::provider::BoxFuture<'a, Result<tools::ToolOutput, tools::ToolError>> {
        Box::pin(async move {
            let result = tools::write::WriteTool.execute(args, ctx).await;
            self.store.get().unwrap().fail_next_write();
            result
        })
    }
}
#[tokio::test]
async fn actual_effect_with_lost_result_commit_stays_unknown_and_is_not_reexecuted() {
    let dir = tempfile::tempdir().unwrap();
    let slot = Arc::new(std::sync::OnceLock::new());
    let (s, (mut r, mut w), _cancel, task) = start(
        dir.path(),
        vec![Arc::new(FailAfterWrite {
            store: slot.clone(),
        })],
    )
    .await;
    assert!(slot.set(s.clone()).is_ok());
    send(
        &mut w,
        call(1, "write", json!({"path":"effect","content":"happened"})),
    )
    .await;
    assert!(r.read(end()).await.is_err());
    assert_eq!(task.await.unwrap(), Err(DriverError::Storage));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("effect")).unwrap(),
        "happened"
    );
    let records = s
        .read(|c| Store::tools_in_state(c, "s", "interrupted"))
        .unwrap();
    assert_eq!(records.len(), 1);
    assert!(records[0].result_entry.is_none());
    assert_eq!(s.entry_count("s").unwrap(), 1);
    assert_eq!(
        s.read(
            |c| Ok(c.query_row("SELECT state FROM cli_bridge_calls", [], |r| r
                .get::<_, String>(0))?)
        )
        .unwrap(),
        "unknown"
    );
}
#[tokio::test]
async fn conflicting_id_cannot_start_a_second_native_effect() {
    let dir = tempfile::tempdir().unwrap();
    let (s, (mut r, mut w), _cancel, task) = start(dir.path(), tools::builtin_tools()).await;
    send(
        &mut w,
        call(1, "write", json!({"path":"first","content":"once"})),
    )
    .await;
    receive(&mut r).await;
    send(
        &mut w,
        call(1, "write", json!({"path":"second","content":"forbidden"})),
    )
    .await;
    assert!(r.read(end()).await.is_err());
    assert_eq!(task.await.unwrap(), Err(DriverError::Protocol));
    assert!(!dir.path().join("second").exists());
    assert_eq!(s.entry_count("s").unwrap(), 2);
}

struct Waiting {
    started: Arc<tokio::sync::Notify>,
}
impl Tool for Waiting {
    fn sequential(&self) -> bool {
        true
    }
    fn spec(&self) -> crate::provider::ToolSpec {
        crate::provider::ToolSpec {
            name: "waiting".into(),
            description: "cooperative fixture".into(),
            parameters: json!({"type":"object"}),
            constrained_sampling: None,
        }
    }
    fn execute<'a>(
        &'a self,
        _: Value,
        ctx: &'a ToolCtx,
    ) -> crate::provider::BoxFuture<'a, Result<tools::ToolOutput, tools::ToolError>> {
        Box::pin(async move {
            ctx.output("committed progress");
            self.started.notify_one();
            ctx.cancel.cancelled().await;
            Err(tools::ToolError::new("cancelled fixture"))
        })
    }
}
#[tokio::test]
async fn stop_cancels_active_native_worker_and_drops_queued_effects_without_fake_success() {
    let dir = tempfile::tempdir().unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let mut tools = tools::builtin_tools();
    tools.push(Arc::new(Waiting {
        started: started.clone(),
    }));
    let (s, (mut r, mut w), cancel, task) = start(dir.path(), tools).await;
    send(&mut w, call(1, "waiting", json!({}))).await;
    started.notified().await;
    send(
        &mut w,
        call(2, "write", json!({"path":"never","content":"no"})),
    )
    .await;
    cancel.cancel();
    assert!(task.await.unwrap().is_err());
    assert!(r.read(end()).await.is_err());
    assert!(!dir.path().join("never").exists());
    assert_eq!(
        s.read(|c| Store::tools_in_state(c, "s", "interrupted"))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(s.entry_count("s").unwrap(), 1);
}
#[tokio::test]
async fn channel_disconnect_cancels_worker_and_never_promotes_its_late_result() {
    let dir = tempfile::tempdir().unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let (s, (r, w), _cancel, task) = start(
        dir.path(),
        vec![Arc::new(Waiting {
            started: started.clone(),
        })],
    )
    .await;
    let mut w = w;
    send(&mut w, call(1, "waiting", json!({}))).await;
    started.notified().await;
    drop(r);
    drop(w);
    assert!(task.await.unwrap().is_err());
    assert_eq!(
        s.read(|c| Store::tools_in_state(c, "s", "interrupted"))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(s.entry_count("s").unwrap(), 1);
}

mod parity;
