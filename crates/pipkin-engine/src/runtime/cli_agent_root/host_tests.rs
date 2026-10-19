//! Real native plan service via independently authenticated per-call host; fake CLI/MCP.
use super::*;
use crate::{
    provider::{BoxFuture, ToolSpec, scripted::Scripted},
    runtime::tests::{Engine, Fixture as EngineFixture},
    tools::{Tool, ToolCtx, ToolError, ToolHost, ToolOutput, plan::PlanUpdateTool},
};
use pipkin_engine_protocol::{Event, ServerFrame, SessionChange};
use tokio::sync::oneshot;
type Captured = (Arc<dyn ToolHost>, String, CancellationToken);
struct Capture {
    send: std::sync::Mutex<Option<oneshot::Sender<Captured>>>,
    proceed: std::sync::Mutex<Option<oneshot::Receiver<()>>>,
    after_effect: bool,
}
impl Capture {
    async fn pause(&self, host: &Arc<dyn ToolHost>, call: &str, cancel: &CancellationToken) {
        if let Some(send) = self.send.lock().unwrap().take() {
            let _ = send.send((host.clone(), call.into(), cancel.clone()));
        }
        let proceed = self.proceed.lock().unwrap().take();
        if let Some(proceed) = proceed {
            let _ = proceed.await;
        }
    }
}
struct CaptureRead;
impl Tool for CaptureRead {
    fn spec(&self) -> ToolSpec {
        crate::tools::plan::PlanReadTool.spec()
    }
    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolCtx,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            assert!(
                ctx.host
                    .as_ref()
                    .unwrap()
                    .plan_update(&ctx.call_id, Vec::new(), None)
                    .await
                    .is_err()
            );
            crate::tools::plan::PlanReadTool.execute(args, ctx).await
        })
    }
}
impl Tool for Capture {
    fn spec(&self) -> ToolSpec {
        PlanUpdateTool.spec()
    }
    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolCtx,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let host = ctx.host.as_ref().unwrap();
            // Even the actual worker host cannot be used as another service/call.
            assert!(host.goal_get().await.is_err());
            assert!(host.plan_read(false).await.is_ok());
            assert!(
                host.plan_update("sdk-printed-id", Vec::new(), None)
                    .await
                    .is_err()
            );
            if self.after_effect {
                let result = PlanUpdateTool.execute(args, ctx).await;
                self.pause(host, &ctx.call_id, &ctx.cancel).await;
                result
            } else {
                self.pause(host, &ctx.call_id, &ctx.cancel).await;
                PlanUpdateTool.execute(args, ctx).await
            }
        })
    }
}
fn engine(fx: &EngineFixture, provider: &Scripted) -> Engine {
    let mut config = fx.config();
    config.enable_local_claude_cli = true;
    config.enable_local_claude_cli_orchestration = true;
    fx.start_config(provider, config)
}
async fn plan_peer(peer: cli_bridge_channel::Launch, path: std::path::PathBuf) {
    let deadline = Instant::now() + Duration::from_secs(20);
    let (mut reader, mut writer) = peer.connect(deadline).await.unwrap().split();
    let update = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"plan_update","arguments":{"ops":[{"op":"create","title":"CLI-owned plan","status":"active"}]}}});
    let mut original = None;
    for request in [
        json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":PROTOCOL_VERSION,"clientInfo":{"name":"fixture","version":"1"},"capabilities":{}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        update.clone(),
        update,
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"plan_read","arguments":{}}}),
    ] {
        writer
            .write(&serde_json::to_vec(&request).unwrap(), deadline)
            .await
            .unwrap();
        if request.get("id").is_some() {
            let reply: Value =
                serde_json::from_slice(&reader.read(deadline).await.unwrap()).unwrap();
            assert!(reply.get("error").is_none(), "{reply}");
            if request["method"] == "tools/call" {
                assert_eq!(reply["result"]["isError"], false, "{reply}");
            }
            if request["id"] == 2 {
                if let Some(old) = &original {
                    assert_eq!(old, &reply);
                } else {
                    original = Some(reply);
                }
            } else if request["id"] == 3 {
                assert!(reply.to_string().contains("CLI-owned plan"));
            }
        }
    }
    std::fs::write(path.join("acknowledged"), "ack").unwrap();
    let request: Value = serde_json::from_slice(&reader.read(deadline).await.unwrap()).unwrap();
    assert!(Control::Quiesce.matches(&request));
    writer
        .write(
            &serde_json::to_vec(&Control::Quiesced.value()).unwrap(),
            deadline,
        )
        .await
        .unwrap();
    let reply: Value = serde_json::from_slice(&reader.read(deadline).await.unwrap()).unwrap();
    assert!(Control::Closed.matches(&reply));
    std::fs::write(path.join("closed"), "closed").unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn service_authority_requires_original_lease_and_expires_on_revoke_or_drop() {
    let f = Fixture::new("success").await;
    let foreign = Fixture::new("success").await;
    assert!(f.launch.ownership.services(&foreign.launch.root).is_err());
    let authority = f.launch.ownership.services(&f.launch.root).unwrap();
    assert!(authority.authorize(|| Ok(())).is_ok());
    f.registry.stop("s");
    assert!(authority.authorize(|| Ok(())).is_err());
    let other = foreign
        .launch
        .ownership
        .services(&foreign.launch.root)
        .unwrap();
    drop(foreign.launch);
    assert!(other.authorize(|| Ok(())).is_err());
}
#[test]
fn stopping_inflight_service_does_not_replay_or_erase_effects() {
    for (after_effect, call_only) in [(false, false), (true, false), (false, true), (true, true)] {
        let fx = EngineFixture::new();
        let provider = Scripted::echo("no API inference");
        let e = engine(&fx, &provider);
        let shared = e.handle.shared().clone();
        shared.handle.block_on(async {
        let owner=*shared.clients.lock().unwrap().keys().next().unwrap();
        let mut f=Fixture::new_in("plan",Some((shared.clone(),owner))).await;
        let (send,captured)=oneshot::channel();let (proceed,wait)=oneshot::channel();
        let index=f.launch.tools.iter().position(|t|t.spec().name=="plan_update").unwrap();
        f.launch.tools[index]=Arc::new(Capture {send:std::sync::Mutex::new(Some(send)),proceed:std::sync::Mutex::new(Some(wait)),after_effect});
        let peer=tokio::spawn(async move {
            let deadline=Instant::now()+Duration::from_secs(20);
            let (mut reader,mut writer)=f.peer.connect(deadline).await.unwrap().split();
            for request in [json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":PROTOCOL_VERSION,"clientInfo":{"name":"fixture","version":"1"},"capabilities":{}}}),json!({"jsonrpc":"2.0","method":"notifications/initialized"}),json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})] {
                writer.write(&serde_json::to_vec(&request).unwrap(),deadline).await.unwrap();
                if request.get("id").is_some() {reader.read(deadline).await.unwrap();}
            }
            let request=json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"plan_update","arguments":{"ops":[{"op":"create","title":"must not commit","status":"active"}]}}});
            writer.write(&serde_json::to_vec(&request).unwrap(),deadline).await.unwrap();
            assert!(reader.read(deadline).await.is_err());
        });
        let authority=f.launch.ownership.services(&f.launch.root).unwrap();
        let result=shared.spawn_cli_agent_root(f.launch);
        let (host,call,call_cancel)=tokio::time::timeout(Duration::from_secs(10),captured).await.unwrap().unwrap();
        if call_only {call_cancel.cancel();assert!(authority.authorize(||Ok(())).is_ok());}
        else {shared.stop_by_backend("s").await.unwrap();}
        assert!(host.plan_update(&call,Vec::new(),None).await.is_err());let _=proceed.send(());
        assert!(result.await.unwrap().is_err());peer.await.unwrap();
        assert_eq!(shared.plan_view("s",false).unwrap().revision,u64::from(after_effect));
        assert_eq!(shared.store.read(|c|Ok(c.query_row("SELECT COUNT(*) FROM plan_receipts",[],|r|r.get::<_,u64>(0))?)).unwrap(),u64::from(after_effect));
        assert_eq!(shared.store.cli_agent_turn("s","r").unwrap().unwrap().state,cli_agent_turns::State::Unknown);
    });
        assert!(provider.requests().is_empty());
    }
}
#[test]
fn native_plan_host_commits_once_in_order_and_cannot_outlive_turn() {
    let fx = EngineFixture::new();
    let provider = Scripted::echo("no API inference");
    let e = engine(&fx, &provider);
    let shared = e.handle.shared().clone();
    shared.handle.block_on(async {
        let owner = *shared.clients.lock().unwrap().keys().next().unwrap();
        let mut f = Fixture::new_in("plan", Some((shared.clone(), owner))).await;
        // Root services are engine-owned; a caller cannot smuggle in an API host/factory.
        f.launch.services.host = Some(Arc::new(crate::runtime::subagents::Host {
            shared: shared.clone(),
            parent: "s".into(),
        }));
        f.launch.services.host_factory = Some(Arc::new(|_, _, _, _, _| {
            panic!("caller factory must be replaced")
        }));
        let (send, captured) = oneshot::channel();
        let index = f
            .launch
            .tools
            .iter()
            .position(|t| t.spec().name == "plan_update")
            .unwrap();
        f.launch.tools[index] = Arc::new(Capture {
            send: std::sync::Mutex::new(Some(send)),
            proceed: std::sync::Mutex::new(None),
            after_effect: false,
        });
        let index = f
            .launch
            .tools
            .iter()
            .position(|t| t.spec().name == "plan_read")
            .unwrap();
        f.launch.tools[index] = Arc::new(CaptureRead);
        let peer = tokio::spawn(plan_peer(f.peer, f.dir.path().into()));
        let result = shared
            .spawn_cli_agent_root(f.launch)
            .await
            .unwrap()
            .unwrap();
        peer.await.unwrap();
        assert_eq!(result.state, cli_agent_turns::State::Completed);
        let (host, call, _) = captured.await.unwrap();
        assert!(host.plan_update(&call, Vec::new(), None).await.is_err());
        let plan = shared.plan_view("s", false).unwrap();
        assert_eq!(plan.revision, 1);
        assert_eq!(plan.tasks.len(), 1);
        let counts = shared
            .store
            .read(|c| {
                Ok((
                    c.query_row("SELECT COUNT(*) FROM plan_receipts", [], |r| {
                        r.get::<_, u64>(0)
                    })?,
                    c.query_row(
                        "SELECT COUNT(*) FROM tool_calls WHERE state='done'",
                        [],
                        |r| r.get::<_, u64>(0),
                    )?,
                ))
            })
            .unwrap();
        assert_eq!(counts, (1, 2));
        assert!(!f.dir.path().join("not-execution-authority").exists());
        let mut revision = 0;
        let mut changes = 0;
        let mut settled = false;
        tokio::time::timeout(Duration::from_secs(5), async {
            while !settled {
                while let Ok(frame) = e.conn.try_recv() {
                    let ServerFrame::Event { event, .. } = frame else {
                        continue;
                    };
                    assert!(!matches!(event, Event::ResyncRequired));
                    if let Event::Session {
                        session,
                        revision: next,
                        change,
                    } = event
                    {
                        if session != "s" {
                            continue;
                        }
                        assert!(next >= revision);
                        revision = next;
                        match change {
                            SessionChange::PlanChanged { .. } => changes += 1,
                            SessionChange::RunSettled { .. } => settled = true,
                            _ => {}
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(changes, 1);
    });
    assert!(provider.requests().is_empty());
}
