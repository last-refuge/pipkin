//! Installed CLI and network inference are never invoked by these owned fixtures.
use super::child_tests::peer;
use super::*;
use crate::{provider::scripted::Scripted, runtime::tests::Fixture as EngineFixture};

#[test]
fn child_return_across_parent_turns_keeps_policy_and_rejects_lost_owner_or_unknown_resume() {
    for end in ["complete", "reapprove", "stop-resume"] {
        let fx = EngineFixture::new();
        let provider = Scripted::echo("no API fallback");
        let mut config = fx.config();
        config.enable_local_claude_cli = true;
        config.enable_local_claude_cli_orchestration = true;
        let config_dir = config.config_dir.clone();
        let engine = fx.start_config(&provider, config);
        let shared = engine.handle.shared().clone();
        shared.handle.block_on(async {
            let owner=*shared.clients.lock().unwrap().keys().next().unwrap();
            let mut f=Fixture::new_in("children",Some((shared.clone(),owner))).await;
            let binding=f.launch.ownership.binding().clone();
            let prepared=f.launch.ownership.prepare().await.unwrap();
            let uuid=binding.create.identity.cli_session_id.clone();
            let image=f.launch.assets.image();
            let system=f.launch.assets.system_text().to_string();
            let tools=f.launch.tools.clone();
            let (send,mut children)=mpsc::unbounded_channel();
            f.launch.services.child_peers=Some(send.clone());
            let parent=tokio::spawn(peer(f.peer,f.dir.path().into(),uuid.clone(),Some(json!({"task":"offline child fixture","wait":true,"context":"inherit"})),false,false,None));
            let root=shared.spawn_cli_agent_root(f.launch);
            let first=tokio::time::timeout(Duration::from_secs(15),children.recv()).await.unwrap().unwrap();
            let task=shared.store.tasks("s",10).unwrap().0.remove(0);
            let child_uuid=shared.store.cli_agent_binding(&task.child_session).unwrap().unwrap().create.identity.cli_session_id;
            peer(first,f.dir.path().into(),child_uuid.clone(),None,false,false,None).await;
            assert_eq!(root.await.unwrap().unwrap().state,cli_agent_turns::State::Completed);
            parent.await.unwrap();
            // Later profile changes must neither replace nor widen the original child.
            std::fs::create_dir_all(&config_dir).unwrap();
            std::fs::write(config_dir.join("settings.json"),json!({"subagents":{"profiles":{"worker":{"model":"openai/forbidden","instructions":"replacement instructions","tools":["read"]}}}}).to_string()).unwrap();
            if end=="reapprove" {
                shared.cli_agents.revoke_session("s");
                shared.cli_agents.approve_return(&shared.store,owner,&prepared,&binding).unwrap();
            }
            let ownership=shared.cli_agents.begin_turn(&shared.store,owner,"s").unwrap();
            let (peer_send,peer_receive)=tokio::sync::oneshot::channel();
            let next=shared.submit_cli_agent_root(crate::runtime::cli_agent_runtime::admission::Start { images: vec![],
                ownership,request:"parent-next".into(),generation:2,operation:"next-op".into(),
                text:"offline root fixture".into(),system,assets_parent:image.assets_parent().into(),image,tools,
                services:crate::runtime::cli_bridge_driver::Services {child_peers:Some(send),..Default::default()},
                deadline:Instant::now()+Duration::from_secs(30),cancel:CancellationToken::new(),
                test_peer:Some(peer_send),test_preparing:None,
            });
            let parent=tokio::spawn(peer(peer_receive.await.unwrap(),f.dir.path().into(),format!("{uuid}-2"),
                Some(json!({"task":"offline continuation fixture","child":task.child,"wait":true})),false,end!="complete",None));
            if end!="reapprove" {
                let child=tokio::time::timeout(Duration::from_secs(15),children.recv()).await.unwrap().unwrap();
                let (send,ready)=tokio::sync::oneshot::channel();
                let child=tokio::spawn(peer(child,f.dir.path().into(),format!("{child_uuid}-2"),None,end=="stop-resume",false,Some(send)));
                if end=="stop-resume" {ready.await.unwrap();shared.stop_by_backend(&task.child_session).await.unwrap();}
                child.await.unwrap();
            }
            assert_eq!(next.await.unwrap().unwrap().state,cli_agent_turns::State::Completed);
            parent.await.unwrap();
            tokio::time::timeout(Duration::from_secs(10),async {
                while !shared.external_tasks.is_empty() {tokio::time::sleep(Duration::from_millis(5)).await;}
            }).await.unwrap();
            let latest=shared.store.tasks("s",10).unwrap().0.remove(0);
            assert_eq!(latest.status,if end=="stop-resume" {"failed"}else{"done"});
            assert_eq!(std::fs::read_to_string(f.dir.path().join("launches")).unwrap().lines().count(),if end=="reapprove" {3}else{4});
            if end=="complete" {
                assert_eq!(std::fs::read(f.dir.path().join(format!("system-{child_uuid}"))).unwrap(),std::fs::read(f.dir.path().join(format!("system-{child_uuid}-2"))).unwrap());
            }
            if end=="stop-resume" {
                assert!(latest.result.as_ref().unwrap().contains("do not replay"));
                // Recovery must inspect the newest unknown resume, not promote the old
                // completed first turn back into a successful task result.
                shared.store.tx(|c|{c.execute("UPDATE subagent_tasks SET status='running',delivery='notify' WHERE child_session=?1",[&task.child_session])?;Ok(())}).unwrap();
                assert_eq!(shared.store.recover_cli_native_children(now_ms()).unwrap(),1);
                assert_eq!(shared.store.tasks("s",10).unwrap().0[0].status,"failed");
            }
            let restarted=crate::runtime::cli_agent_owner::Registry::new(true);
            restarted.connect(owner);
            assert!(restarted.begin_turn(&shared.store,owner,&task.child_session).is_err());
            assert!(children.try_recv().is_err());
        });
        assert!(provider.requests().is_empty());
    }
}

#[test]
fn child_return_refuses_overrides_foreign_busy_unknown_and_unread_context() {
    for refusal in [
        "override",
        "foreign",
        "busy",
        "unknown",
        "unread",
        "unread-terminal",
    ] {
        let fx = EngineFixture::new();
        let provider = Scripted::echo("no API fallback");
        let mut config = fx.config();
        config.enable_local_claude_cli = true;
        config.enable_local_claude_cli_orchestration = true;
        let engine = fx.start_config(&provider, config);
        let shared = engine.handle.shared().clone();
        shared.handle.block_on(async {
            let owner=*shared.clients.lock().unwrap().keys().next().unwrap();
            let mut f=Fixture::new_in("children",Some((shared.clone(),owner))).await;
            let uuid=f.launch.ownership.binding().create.identity.cli_session_id.clone();
            let (send,mut children)=mpsc::unbounded_channel();
            f.launch.services.child_peers=Some(send);
            let failed=matches!(refusal,"unknown"|"unread"|"unread-terminal");
            let first=json!({"task":"offline child fixture","wait":refusal!="busy","wake":false,"context":"inherit","_refused":failed});
            let mut next=json!({"task":"offline continuation fixture","child":if refusal=="foreign" {99}else{1},"_refused":true});
            if refusal=="override" {next["profile"]=json!("reviewer");}
            if refusal=="unread" {std::fs::write(f.dir.path().join("skip-context"),"").unwrap();}
            if refusal=="unread-terminal" {std::fs::write(f.dir.path().join("no-context-tools"),"").unwrap();}
            let parent=tokio::spawn(peer(f.peer,f.dir.path().into(),uuid,Some(json!([first,next])),false,false,None));
            let root=shared.spawn_cli_agent_root(f.launch);
            let child=tokio::time::timeout(Duration::from_secs(15),children.recv()).await.unwrap().unwrap();
            let task=shared.store.tasks("s",10).unwrap().0.remove(0);
            let child_uuid=shared.store.cli_agent_binding(&task.child_session).unwrap().unwrap().create.identity.cli_session_id;
            if refusal=="busy" {
                assert_eq!(root.await.unwrap().unwrap().state,cli_agent_turns::State::Completed);
                peer(child,f.dir.path().into(),child_uuid,None,false,false,None).await;
            } else {
                let (send,ready)=tokio::sync::oneshot::channel();
                let child=tokio::spawn(peer(child,f.dir.path().into(),child_uuid,None,refusal=="unknown",false,Some(send)));
                if refusal=="unknown" {ready.await.unwrap();shared.stop_by_backend(&task.child_session).await.unwrap();}
                // An unread terminal is rejected after Quiesce, so Closed may be sent
                // before the coordinator refuses root completion; neither is success.
                child.await.unwrap();
                assert_eq!(root.await.unwrap().unwrap().state,cli_agent_turns::State::Completed);
            }
            parent.await.unwrap();
            tokio::time::timeout(Duration::from_secs(10),async {
                while !shared.external_tasks.is_empty() {tokio::time::sleep(Duration::from_millis(5)).await;}
            }).await.unwrap();
            assert!(children.try_recv().is_err());
            let task=shared.store.tasks("s",10).unwrap().0.remove(0);
            assert_eq!(task.status,if failed {"failed"}else{"done"});
            assert_eq!(std::fs::read_to_string(f.dir.path().join("launches")).unwrap().lines().count(),2);
            let count:i64=shared.store.read(|c|Ok(c.query_row("SELECT COUNT(*) FROM cli_agent_turns WHERE session=?1",[&task.child_session],|r|r.get(0))?)).unwrap();
            assert_eq!(count,1);
            if refusal.starts_with("unread") {assert!(!f.dir.path().join("child-effect").exists());}
            if failed {
                assert_eq!(shared.store.cli_agent_turn(&task.child_session,&format!("child:{}",task.parent_call)).unwrap().unwrap().state,cli_agent_turns::State::Unknown);
            }
            assert_eq!(shared.store.recover_cli_native_children(now_ms()).unwrap(),0);
        });
        assert!(provider.requests().is_empty());
    }
}

#[test]
fn inherited_child_continues_same_uuid_and_frozen_context_inline_notify_and_wake() {
    for delivery in ["inline", "notify", "wake"] {
        let fx = EngineFixture::new();
        let provider = Scripted::echo("no API fallback");
        let mut config = fx.config();
        config.enable_local_claude_cli = true;
        config.enable_local_claude_cli_orchestration = true;
        let engine = fx.start_config(&provider, config);
        let shared = engine.handle.shared().clone();
        shared.handle.block_on(async {
            let owner=*shared.clients.lock().unwrap().keys().next().unwrap();
            let mut f=Fixture::new_in("children",Some((shared.clone(),owner))).await;
            let uuid=f.launch.ownership.binding().create.identity.cli_session_id.clone();
            let (send,mut children)=mpsc::unbounded_channel();
            f.launch.services.child_peers=Some(send);
            let parent=tokio::spawn(peer(f.peer,f.dir.path().into(),uuid.clone(),Some(json!([
                {"task":"offline child fixture","wait":true,"context":"inherit"},
                {"task":"offline continuation fixture","child":1,"wait":delivery=="inline","wake":delivery=="wake"}
            ])),false,false,None));
            let root=shared.spawn_cli_agent_root(f.launch);
            let first=tokio::time::timeout(Duration::from_secs(15),children.recv()).await.unwrap().unwrap();
            let task=shared.store.tasks("s",10).unwrap().0.remove(0);
            let binding=shared.store.cli_agent_binding(&task.child_session).unwrap().unwrap();
            let child_uuid=binding.create.identity.cli_session_id.clone();
            peer(first,f.dir.path().into(),child_uuid.clone(),None,false,false,None).await;
            let second=tokio::time::timeout(Duration::from_secs(15),children.recv()).await.unwrap().unwrap();
            peer(second,f.dir.path().into(),format!("{child_uuid}-2"),None,false,false,None).await;
            assert_eq!(root.await.unwrap().unwrap().state,cli_agent_turns::State::Completed);
            parent.await.unwrap();
            if delivery=="wake" {
                let resumed=tokio::time::timeout(Duration::from_secs(15),children.recv()).await.unwrap().unwrap();
                peer(resumed,f.dir.path().into(),format!("{uuid}-2"),None,false,false,None).await;
            }
            tokio::time::timeout(Duration::from_secs(10),async {
                while !shared.external_tasks.is_empty() {tokio::time::sleep(Duration::from_millis(5)).await;}
            }).await.unwrap();
            let tasks=shared.store.tasks("s",10).unwrap().0;
            assert_eq!(tasks.len(),1);
            assert_eq!(tasks[0].status,"done");
            assert_eq!(tasks[0].delivery,match delivery {"inline"=>"inline","notify"=>"notified",_=>"woken"});
            assert_eq!(shared.store.cli_agent_binding(&task.child_session).unwrap().unwrap(),binding);
            let rows:Vec<(String,String)>=shared.store.read(|c|Ok(c.prepare("SELECT mode,state FROM cli_agent_turns WHERE session=?1 ORDER BY run")?.query_map([&task.child_session],|r|Ok((r.get(0)?,r.get(1)?)))?.collect::<rusqlite::Result<_>>()?)).unwrap();
            assert_eq!(rows,vec![("fresh".into(),"completed".into()),("resume".into(),"completed".into())]);
            let read=|name:&str|std::fs::read(f.dir.path().join(name)).unwrap();
            assert_eq!(read(&format!("context-{child_uuid}")),read(&format!("context-{child_uuid}-2")));
            assert_eq!(read(&format!("system-{child_uuid}")),read(&format!("system-{child_uuid}-2")));
            assert_eq!(read(&format!("prompt-{child_uuid}")),b"offline child fixture");
            assert_eq!(read(&format!("prompt-{child_uuid}-2")),b"offline continuation fixture");
            let context:Value=serde_json::from_slice(&read(&format!("context-{child_uuid}"))).unwrap();
            let snapshot:Value=serde_json::from_str(context["content"][0]["text"].as_str().unwrap()).unwrap();
            assert_eq!(snapshot["messages"][0]["role"],"user");
            assert_eq!(snapshot["messages"][0]["text"],"offline root fixture");
            assert!(snapshot["parentRevision"].as_u64().unwrap()>0);
            let identities=String::from_utf8(read("launch-identities")).unwrap();
            assert!(identities.contains(&format!("--session-id {child_uuid} 1")));
            assert!(identities.contains(&format!("--resume {child_uuid} 2")));
            assert_eq!(identities.lines().count(),if delivery=="wake" {4}else{3});
            assert_eq!(shared.store.recover_cli_native_children(now_ms()).unwrap(),0);
            assert!(!f.dir.path().join("not-execution-authority").exists());
        });
        assert!(provider.requests().is_empty());
    }
}
