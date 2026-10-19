//! Parent -> native subagent tool -> independently supervised CLI child -> native Write.
//! CLI/SDK peers are fixtures; inference is never sent to an installed CLI or API provider.
use super::*;
use crate::{provider::scripted::Scripted, runtime::tests::Fixture as EngineFixture};
pub(super) async fn peer(
    launch: cli_bridge_channel::Launch,
    path: std::path::PathBuf,
    uuid: String,
    parent: Option<Value>,
    stop: bool,
    refused: bool,
    ready: Option<tokio::sync::oneshot::Sender<()>>,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let (mut reader, mut writer) = launch.connect(deadline).await.unwrap().split();
    for request in [
        json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":PROTOCOL_VERSION,"clientInfo":{"name":"fixture","version":"1"},"capabilities":{}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
    ] {
        writer
            .write(&serde_json::to_vec(&request).unwrap(), deadline)
            .await
            .unwrap();
        if request.get("id").is_some() {
            let body = reader.read(deadline).await.unwrap();
            let body: Value = serde_json::from_slice(&body).unwrap();
            if request["method"] == "tools/list"
                && body["result"]["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|t| t["name"] == "parent_context")
                && !path.join("skip-context").exists()
                && !path.join("no-context-tools").exists()
            {
                writer.write(&serde_json::to_vec(&json!({"jsonrpc":"2.0","id":99,"method":"tools/call","params":{"name":"parent_context","arguments":{}}})).unwrap(),deadline).await.unwrap();
                let context: Value =
                    serde_json::from_slice(&reader.read(deadline).await.unwrap()).unwrap();
                assert_eq!(context["result"]["isError"], false);
                std::fs::write(
                    path.join(format!("context-{uuid}")),
                    serde_json::to_vec(&context["result"]).unwrap(),
                )
                .unwrap();
            }
        }
    }
    let arguments = match parent.as_ref() {
        Some(Value::Array(calls)) => calls.clone(),
        Some(call) => vec![call.clone()],
        None if path.join("no-context-tools").exists() => vec![],
        None => vec![Value::Null],
    };
    let is_parent = parent.is_some();
    for (index, mut arguments) in arguments.into_iter().enumerate() {
        let expected_error = arguments
            .get("_refused")
            .and_then(Value::as_bool)
            .unwrap_or(refused);
        if let Some(object) = arguments.as_object_mut() {
            object.remove("_refused");
        }
        let request = if is_parent {
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"subagent","arguments":arguments}})
        } else {
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"write","arguments":{"path":"child-effect","content":"owned child effect"}}})
        };
        let mut request = request;
        request["id"] = json!(index + 2);
        let parent = is_parent;
        writer
            .write(&serde_json::to_vec(&request).unwrap(), deadline)
            .await
            .unwrap();
        let response = reader.read(deadline).await;
        if stop && parent {
            assert!(response.is_err());
            return;
        }
        if !parent && path.join("skip-context").exists() {
            assert!(response.is_err());
            return;
        }
        let response: Value = serde_json::from_slice(&response.unwrap()).unwrap();
        assert_eq!(response["result"]["isError"], expected_error, "{response}");
        if parent && !stop {
            writer
                .write(&serde_json::to_vec(&request).unwrap(), deadline)
                .await
                .unwrap();
            let replay: Value =
                serde_json::from_slice(&reader.read(deadline).await.unwrap()).unwrap();
            assert_eq!(
                response, replay,
                "MCP replay must return the receipt, not run another child"
            );
        }
    }
    if let Some(ready) = ready {
        let _ = ready.send(());
    }
    if stop {
        assert!(reader.read(deadline).await.is_err());
        return;
    }
    std::fs::write(path.join(format!("acknowledged-{uuid}")), "ack").unwrap();
    let request: Value = serde_json::from_slice(&reader.read(deadline).await.unwrap()).unwrap();
    assert!(Control::Quiesce.matches(&request));
    writer
        .write(
            &serde_json::to_vec(&Control::Quiesced.value()).unwrap(),
            deadline,
        )
        .await
        .unwrap();
    let response: Value = serde_json::from_slice(&reader.read(deadline).await.unwrap()).unwrap();
    assert!(Control::Closed.matches(&response));
    std::fs::write(path.join(format!("closed-{uuid}")), "closed").unwrap();
}
fn exercise(stop: bool, refused: bool) {
    let fx = EngineFixture::new();
    let provider = Scripted::echo("no API inference");
    let mut config = fx.config();
    config.enable_local_claude_cli = true;
    config.enable_local_claude_cli_orchestration = true;
    let config_dir = config.config_dir.clone();
    let e = fx.start_config(&provider, config);
    if refused {
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("settings.json"),
            serde_json::to_vec(
                &json!({"subagents":{"profiles":{"worker":{"model":"openai/no-fallback"}}}}),
            )
            .unwrap(),
        )
        .unwrap();
    }
    let shared = e.handle.shared().clone();
    shared.handle.block_on(async {
        let owner = *shared.clients.lock().unwrap().keys().next().unwrap();
        let mut f = Fixture::new_in("children", Some((shared.clone(), owner))).await;
        let parent_uuid = f
            .launch
            .ownership
            .binding()
            .create
            .identity
            .cli_session_id
            .clone();
        let (send, mut children) = tokio::sync::mpsc::unbounded_channel();
        f.launch.services.child_peers = Some(send);
        let parent = tokio::spawn(peer(
            f.peer,
            f.dir.path().into(),
            parent_uuid.clone(),
            Some(json!({"task":"offline child fixture","wait":true,"context":"fresh"})),
            stop,
            refused,
            None,
        ));
        let result = shared.spawn_cli_agent_root(f.launch);
        if refused {
            assert_eq!(
                result.await.unwrap().unwrap().state,
                cli_agent_turns::State::Completed
            );
            parent.await.unwrap();
            assert!(shared.store.tasks("s", 10).unwrap().0.is_empty());
            assert!(!f.dir.path().join("child-effect").exists());
            assert_eq!(
                std::fs::read_to_string(f.dir.path().join("launches"))
                    .unwrap()
                    .lines()
                    .count(),
                1
            );
            return;
        }
        let launch = tokio::time::timeout(Duration::from_secs(15), children.recv())
            .await
            .unwrap()
            .expect("child listener");
        let task = shared.store.tasks("s", 10).unwrap().0.remove(0);
        let binding = shared
            .store
            .cli_agent_binding(&task.child_session)
            .unwrap()
            .unwrap();
        assert_ne!(parent_uuid, binding.create.identity.cli_session_id);
        assert_eq!(binding.create.parent.as_deref(), Some("s"));
        assert_eq!(binding.create.model, "sonnet");
        let (ready, reached) = tokio::sync::oneshot::channel();
        let child = tokio::spawn(peer(
            launch,
            f.dir.path().into(),
            binding.create.identity.cli_session_id.clone(),
            None,
            stop,
            false,
            Some(ready),
        ));
        reached.await.unwrap();
        if stop {
            shared.stop_by_backend("s").await.unwrap();
        }
        let result = tokio::time::timeout(Duration::from_secs(20), result)
            .await
            .unwrap()
            .unwrap();
        if stop {
            assert!(result.is_err());
        } else {
            assert_eq!(result.unwrap().state, cli_agent_turns::State::Completed);
        }
        child.await.unwrap();
        parent.await.unwrap();
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("launches"))
                .unwrap()
                .lines()
                .count(),
            2
        );
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("child-effect")).unwrap(),
            "owned child effect"
        );
        assert!(!f.dir.path().join("not-execution-authority").exists());
        let task = shared.store.tasks("s", 10).unwrap().0.remove(0);
        assert_eq!(task.status, if stop { "failed" } else { "done" });
        assert!(task.result.as_ref().is_some_and(|r| r.contains(if stop {
            "do not replay"
        } else {
            "done"
        })));
        let turn = shared
            .store
            .cli_agent_turn(&task.child_session, &format!("child:{}", task.parent_call))
            .unwrap()
            .unwrap();
        assert_eq!(
            turn.state,
            if stop {
                cli_agent_turns::State::Unknown
            } else {
                cli_agent_turns::State::Completed
            }
        );
        assert!(!shared.cli_agents.has_turn(&task.child_session));
        assert!(
            shared
                .cli_agents
                .begin_turn(&shared.store, owner, &task.child_session)
                .is_err()
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            while !shared.external_tasks.is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    });
    assert!(provider.requests().is_empty());
}
#[test]
fn inline_child_uses_cli_runtime_native_write_and_returns_without_api_requests() {
    exercise(false, false);
}
#[test]
fn api_selected_child_profile_is_refused_without_child_or_api_inference() {
    exercise(false, true);
}
#[test]
fn parent_stop_reaps_inline_child_and_preserves_known_write_without_replay() {
    exercise(true, false);
}

struct HoldBackgroundReceipt;
impl crate::tools::Tool for HoldBackgroundReceipt {
    fn spec(&self) -> crate::provider::ToolSpec {
        crate::runtime::subagents::SubagentTool.spec()
    }
    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a crate::tools::ToolCtx,
    ) -> crate::provider::BoxFuture<'a, Result<crate::tools::ToolOutput, crate::tools::ToolError>>
    {
        Box::pin(async move {
            crate::runtime::subagents::SubagentTool
                .execute(args, ctx)
                .await?;
            std::future::pending().await
        })
    }
}
#[test]
fn stop_before_background_tool_receipt_cancels_the_unconfirmed_delegation() {
    let fx = EngineFixture::new();
    let provider = Scripted::echo("no API fallback");
    let mut config = fx.config();
    config.enable_local_claude_cli = true;
    config.enable_local_claude_cli_orchestration = true;
    let e = fx.start_config(&provider, config);
    let shared = e.handle.shared().clone();
    shared.handle.block_on(async {
        let owner = *shared.clients.lock().unwrap().keys().next().unwrap();
        let mut f = Fixture::new_in("children", Some((shared.clone(), owner))).await;
        let index = f
            .launch
            .tools
            .iter()
            .position(|t| t.spec().name == "subagent")
            .unwrap();
        f.launch.tools[index] = Arc::new(HoldBackgroundReceipt);
        let uuid = f
            .launch
            .ownership
            .binding()
            .create
            .identity
            .cli_session_id
            .clone();
        let (send, mut children) = tokio::sync::mpsc::unbounded_channel();
        f.launch.services.child_peers = Some(send);
        let parent = tokio::spawn(peer(
            f.peer,
            f.dir.path().into(),
            uuid,
            Some(json!({"task":"offline child fixture","wait":false,"wake":false})),
            true,
            false,
            None,
        ));
        let root = shared.spawn_cli_agent_root(f.launch);
        let launch = tokio::time::timeout(Duration::from_secs(15), children.recv())
            .await
            .unwrap()
            .unwrap();
        let task = shared.store.tasks("s", 10).unwrap().0.remove(0);
        let binding = shared
            .store
            .cli_agent_binding(&task.child_session)
            .unwrap()
            .unwrap();
        let (ready, reached) = tokio::sync::oneshot::channel();
        let child = tokio::spawn(peer(
            launch,
            f.dir.path().into(),
            binding.create.identity.cli_session_id,
            None,
            true,
            false,
            Some(ready),
        ));
        reached.await.unwrap();
        assert!(
            !shared
                .store
                .cli_native_child_handed_off(&task.child_session)
                .unwrap()
        );
        shared.stop_by_backend("s").await.unwrap();
        assert!(root.await.unwrap().is_err());
        parent.await.unwrap();
        child.await.unwrap();
        tokio::time::timeout(Duration::from_secs(8), async {
            while !shared.external_tasks.is_empty() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let task = shared.store.tasks("s", 10).unwrap().0.remove(0);
        assert_eq!(task.status, "failed");
        assert!(task.note.as_ref().unwrap().contains("do not replay"));
        assert_eq!(
            shared
                .store
                .cli_agent_turn(&task.child_session, &format!("child:{}", task.parent_call))
                .unwrap()
                .unwrap()
                .state,
            cli_agent_turns::State::Unknown
        );
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("child-effect")).unwrap(),
            "owned child effect"
        );
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("launches"))
                .unwrap()
                .lines()
                .count(),
            2
        );
    });
    assert!(provider.requests().is_empty());
}
fn parent_views(connection: &crate::host::Connection, revision: &mut u64) -> (Vec<String>, bool) {
    use pipkin_engine_protocol::{Event, ServerFrame, SessionChange};
    let mut phases = Vec::new();
    let mut resync = false;
    while let Ok(frame) = connection.try_recv() {
        match frame {
            ServerFrame::Event {
                event:
                    Event::Session {
                        session,
                        revision: next,
                        change,
                    },
                ..
            } if session == "s" => {
                assert!(next >= *revision);
                *revision = next;
                if let SessionChange::SubagentsChanged { subagents } = change {
                    phases.push(subagents.children[0].status.clone());
                }
            }
            ServerFrame::Event {
                event: Event::ResyncRequired,
                ..
            } => resync = true,
            _ => {}
        }
    }
    (phases, resync)
}
#[test]
fn background_notify_survives_parent_completion_but_not_revoked_child_ownership() {
    for end in ["complete", "child-stop", "revoke", "disconnect", "shutdown"] {
        let fx = EngineFixture::new();
        let provider = Scripted::echo("no API inference");
        let mut config = fx.config();
        config.enable_local_claude_cli = true;
        config.enable_local_claude_cli_orchestration = true;
        let e = fx.start_config(&provider, config);
        let shared = e.handle.shared().clone();
        let mut connection = Some(e.conn);
        shared.handle.block_on(async {
            let owner=*shared.clients.lock().unwrap().keys().next().unwrap();
            let mut f=Fixture::new_in("children",Some((shared.clone(),owner))).await;
            let parent_uuid=f.launch.ownership.binding().create.identity.cli_session_id.clone();
            let (send,mut children)=tokio::sync::mpsc::unbounded_channel();
            f.launch.services.child_peers=Some(send);
            let parent=tokio::spawn(peer(f.peer,f.dir.path().into(),parent_uuid,
                Some(json!({"task":"offline child fixture","wait":false,"wake":false,"context":"fresh"})),false,false,None));
            let root=shared.spawn_cli_agent_root(f.launch);
            let launch=tokio::time::timeout(Duration::from_secs(15),children.recv()).await.unwrap().unwrap();
            // The parent finishes, including its native receipt and worker teardown,
            // BEFORE the child connects its authenticated peer or performs a tool effect.
            assert_eq!(tokio::time::timeout(Duration::from_secs(15),root).await.unwrap().unwrap().unwrap().state,cli_agent_turns::State::Completed);
            parent.await.unwrap();
            let task=shared.store.tasks("s",10).unwrap().0.remove(0);
            assert_eq!(task.status,"running");assert_eq!(task.delivery,"notify");
            // Read this boundary before cancellation: the bounded delivery queue may
            // correctly replace earlier presentation events with ResyncRequired.
            let mut revision=0;
            let (phases,resync)=parent_views(connection.as_ref().unwrap(),&mut revision);
            assert!(!resync,"end={end}");
            assert_eq!(phases,vec!["pending","running"],"end={end}");
            assert!(!f.dir.path().join("child-effect").exists());
            let binding=shared.store.cli_agent_binding(&task.child_session).unwrap().unwrap();
            let (ready,reached)=tokio::sync::oneshot::channel();
            let child=tokio::spawn(peer(launch,f.dir.path().into(),binding.create.identity.cli_session_id,
                None,end!="complete",false,Some(ready)));
            reached.await.unwrap();
            match end {
                "complete"=>{},
                "child-stop"=>{shared.stop_by_backend(&task.child_session).await.unwrap();},
                "revoke"=>shared.cli_agents.revoke_session("s"),
                "disconnect"=>drop(connection.take()),
                "shutdown"=>{shared.cli_agents.shutdown();shared.stopping.cancel();},
                _=>unreachable!(),
            }
            child.await.unwrap();
            tokio::time::timeout(Duration::from_secs(8),async {
                while !shared.external_tasks.is_empty() {tokio::time::sleep(Duration::from_millis(5)).await;}
            }).await.unwrap();
            let final_task=shared.store.tasks("s",10).unwrap().0.remove(0);
            assert_eq!(final_task.status,if end=="complete" {"done"} else {"failed"});
            assert_eq!(final_task.delivery,"notified");
            // Cancellation can legitimately request a resnapshot instead of preserving
            // every queued view frame. Shutdown need not deliver after closing clients.
            if end != "shutdown" && let Some(connection) = connection.as_ref() {
                let (phases,resync)=parent_views(connection,&mut revision);
                if !resync {assert_eq!(phases,vec![final_task.status.as_str()],"end={end}");}
                assert_eq!(shared.subagents_view("s").unwrap().children[0].status,final_task.status);
            }
            assert!(final_task.result.as_ref().unwrap().contains(if end=="complete" {"done"} else {"do not replay"}));
            let read=shared.subagent_result("s",task.child,0).unwrap();
            assert_eq!(read.result,if end=="complete" {final_task.result.clone()} else {None});
            if end!="complete" {assert!(read.note.as_ref().unwrap().contains("do not replay"));}
            let waited=shared.subagent_wait("s",task.child,Duration::ZERO,&tokio_util::sync::CancellationToken::new()).await.unwrap();
            assert_eq!(waited.result,read.result);
            assert_eq!(waited.note,read.note);
            assert!(shared.subagent_result(&task.child_session,task.child,0).is_err());
            assert!(!shared.cli_agents.has_turn(&task.child_session));
            assert!(shared.cli_agents.begin_turn(&shared.store,owner,&task.child_session).is_err());
            assert_eq!(std::fs::read_to_string(f.dir.path().join("child-effect")).unwrap(),"owned child effect");
            assert_eq!(std::fs::read_to_string(f.dir.path().join("launches")).unwrap().lines().count(),2);
            let receipt=shared.store.cli_agent_turn(&task.child_session,&format!("child:{}",task.parent_call)).unwrap().unwrap();
            assert_eq!(receipt.state,if end=="complete" {cli_agent_turns::State::Completed} else {cli_agent_turns::State::Unknown});
            // Simulate only a lost task-view settlement in this owned fixture DB.
            // Recovery reads the actual root receipt; it never relaunches either model.
            shared.store.tx(|c| {c.execute("UPDATE subagent_tasks SET status='pending',result=NULL,delivery='notify' WHERE child_session=?1",[&task.child_session])?;Ok(())}).unwrap();
            assert_eq!(shared.store.recover_cli_native_children(now_ms()).unwrap(),1);
            assert_eq!(shared.store.recover_cli_native_children(now_ms()).unwrap(),0);
            let recovered=shared.store.tasks("s",10).unwrap().0.remove(0);
            assert_eq!(recovered.status,final_task.status);
            assert_eq!(recovered.result,final_task.result);
            assert_eq!(recovered.delivery,"notified");
            assert_eq!(std::fs::read_to_string(f.dir.path().join("launches")).unwrap().lines().count(),2);
        });
        assert!(provider.requests().is_empty());
    }
}
#[test]
fn background_wake_waits_resumes_once_and_obeys_stop_capacity_and_reapproval() {
    for end in [
        "complete",
        "idle-stop",
        "busy-release",
        "busy-stop",
        "reapprove",
        "capacity-release",
        "capacity-stop",
    ] {
        let fx = EngineFixture::new();
        let provider = Scripted::echo("no API fallback");
        let mut config = fx.config();
        config.enable_local_claude_cli = true;
        config.enable_local_claude_cli_orchestration = true;
        let e = fx.start_config(&provider, config);
        let shared = e.handle.shared().clone();
        shared.handle.block_on(async {
            let owner = *shared.clients.lock().unwrap().keys().next().unwrap();
            let mut f = Fixture::new_in("children", Some((shared.clone(), owner))).await;
            let binding=f.launch.ownership.binding().clone();
            let uuid = binding.create.identity.cli_session_id.clone();
            let prepared=f.launch.ownership.prepare().await.unwrap();
            let should_wake=matches!(end,"complete"|"busy-release"|"capacity-release");
            let (send, mut peers) = tokio::sync::mpsc::unbounded_channel();
            f.launch.services.child_peers = Some(send);
            let parent = tokio::spawn(peer(f.peer, f.dir.path().into(), uuid.clone(),
                Some(json!({"task":"offline child fixture","wait":false,"wake":true,"context":"fresh"})),
                false, false, None));
            assert_eq!(shared.spawn_cli_agent_root(f.launch).await.unwrap().unwrap().state,
                cli_agent_turns::State::Completed);
            parent.await.unwrap();
            let child_peer = tokio::time::timeout(Duration::from_secs(15), peers.recv()).await.unwrap().unwrap();
            let task = shared.store.tasks("s", 10).unwrap().0.remove(0);
            let child_uuid = shared.store.cli_agent_binding(&task.child_session).unwrap().unwrap().create.identity.cli_session_id;
            assert_ne!(uuid, child_uuid);
            assert_eq!(task.delivery, "wake");
            let busy=if matches!(end,"busy-release"|"busy-stop"|"reapprove"|"capacity-release"|"capacity-stop") {
                Some(shared.cli_agents.begin_turn(&shared.store,owner,"s").unwrap())
            } else {None};
            if end=="idle-stop" {
                // There is no active parent Turn. Stop must still revoke queued continuations.
                shared.stop_by_backend("s").await.unwrap();
            }
            peer(child_peer, f.dir.path().into(), child_uuid.clone(), None, false, false, None).await;
            if busy.is_some() {
                tokio::time::timeout(Duration::from_secs(5),async {
                    while shared.store.tasks("s",10).unwrap().0[0].status!="done" {tokio::time::sleep(Duration::from_millis(5)).await;}
                }).await.unwrap();
                assert!(shared.store.cli_agent_turn("s",&format!("cli-child-wake:{}",task.child_session)).unwrap().is_none());
            }
            let capacity=if end.starts_with("capacity-") {
                Some(shared.generations.clone().try_acquire_many_owned(shared.generations.available_permits() as u32).unwrap())
            } else {None};
            if end=="busy-stop" {shared.stop_by_backend("s").await.unwrap();}
            if end=="reapprove" {shared.cli_agents.revoke_session("s");}
            drop(busy);
            if end=="reapprove" {shared.cli_agents.approve_return(&shared.store,owner,&prepared,&binding).unwrap();}
            if capacity.is_some() {
                tokio::time::timeout(Duration::from_secs(5),async {
                    while !shared.cli_agents.has_turn("s") {tokio::time::sleep(Duration::from_millis(5)).await;}
                }).await.unwrap();
                assert!(shared.store.cli_agent_turn("s",&format!("cli-child-wake:{}",task.child_session)).unwrap().is_none());
                if end=="capacity-stop" {shared.stop_by_backend("s").await.unwrap();}
            }
            drop(capacity);
            if should_wake {
                let wake_peer = tokio::time::timeout(Duration::from_secs(15), peers.recv()).await.unwrap().expect("root wake listener");
                peer(wake_peer, f.dir.path().into(), format!("{uuid}-2"), None, false, false, None).await;
            }
            tokio::time::timeout(Duration::from_secs(10), async {
                while !shared.external_tasks.is_empty() { tokio::time::sleep(Duration::from_millis(5)).await; }
            }).await.unwrap();
            let settled = shared.store.tasks("s", 10).unwrap().0.remove(0);
            assert_eq!(settled.status, "done");
            assert_eq!(settled.delivery, if should_wake {"woken"} else {"suppressed"});
            let wake_request = format!("cli-child-wake:{}", task.child_session);
            let receipt = shared.store.cli_agent_turn("s", &wake_request).unwrap();
            if !should_wake {
                assert!(receipt.is_none());
            } else {
                let receipt = receipt.unwrap();
                assert_eq!(receipt.state, cli_agent_turns::State::Completed);
                assert_eq!(receipt.mode, crate::execution::claude_cli::policy::TurnMode::Resume);
                let mode: String = shared.store.read(|c| Ok(c.query_row("SELECT mode FROM requests WHERE session='s' AND key=?1", [&wake_request], |r| r.get(0))?)).unwrap();
                assert_eq!(mode, "followUp");
                assert!(!shared.cli_agents.has_turn("s"));
            }
            let identities = std::fs::read_to_string(f.dir.path().join("launch-identities")).unwrap();
            assert_eq!(identities.lines().count(), if should_wake {3} else {2});
            assert!(identities.contains(&format!("--session-id {uuid} 1")));
            assert!(identities.contains(&format!("--session-id {child_uuid} 1")));
            assert_eq!(identities.contains(&format!("--resume {uuid} 2")), should_wake);
            assert!(!f.dir.path().join("not-execution-authority").exists());
            assert_eq!(std::fs::read_to_string(f.dir.path().join("child-effect")).unwrap(), "owned child effect");
            // Recovery is metadata-only, never a second wake or a child relaunch.
            assert_eq!(shared.store.recover_cli_native_children(now_ms()).unwrap(), 0);
            assert!(peers.try_recv().is_err());
        });
        assert!(provider.requests().is_empty());
    }
}
