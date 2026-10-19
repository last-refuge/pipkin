//! Public controls with owned fake CLI/MCP processes only. No installed CLI prompts.
use super::goal_lifecycle_tests::{configured, next, setup, setup_variant, worker};
use super::service_tests::{call, peer_marked};
use super::*;
use crate::{
    provider::scripted::Scripted,
    runtime::{dispatch_call, tests::Fixture as EngineFixture},
};
use pipkin_engine_protocol::*;
async fn marker(path: &std::path::Path, uuid: &str) {
    tokio::time::timeout(Duration::from_secs(15), async {
        while !path.join(format!("prompt-{uuid}")).exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
#[test]
fn desktop_controls_launch_and_steer_use_person_receipts_and_never_api() {
    for scenario in ["fresh", "inherit", "undelivered"] {
        let fx = EngineFixture::new();
        let p = Scripted::echo("never API");
        let e = configured(&fx, &p);
        let s = e.handle.shared().clone();
        s.handle.block_on(async {
        let mut f=setup_variant(&s,"children").await;
        s.cli_desktop_test_resources.lock().unwrap().insert("s".into(),f.resources.clone());
        let stale=s.cli_agents.person_child(&s.store,f.owner,"s","stopped-preparation").unwrap();
        s.cli_agents.stop("s");assert!(stale.context(&s.store).is_err());
        let cancel=CancellationToken::new();
        let launch=Call::LaunchSubagent {session:"s".into(),request:"person-child".into(),task:"offline child fixture".into(),profile:None,context:Some(if scenario=="inherit" {"inherit"} else {"fresh"}.into())};
        let Reply::Subagent(row)=dispatch_call(&s,f.owner,launch.clone(),&cancel).await.unwrap() else {panic!()};
        let child=s.store.tasks("s",10).unwrap().0.remove(0);
        assert_eq!(child.origin,"user");assert_eq!(child.parent_call,"user:person-child");
        assert_eq!(s.store.read(|c|Ok(c.query_row("SELECT count(*) FROM tool_calls WHERE session='s' AND call_id='user:person-child'",[],|r|r.get::<_,u64>(0))?)).unwrap(),0);
        let binding=s.store.cli_agent_binding(&child.child_session).unwrap().unwrap();let uuid=binding.create.identity.cli_session_id;
        let peer=next(&mut f).await;marker(f.dir.path(),&uuid).await;
        let steer=Call::SteerSubagent {session:"s".into(),child:child.child,request:"person-steer".into(),text:"desktop steering instruction".into()};
        dispatch_call(&s,f.owner,steer.clone(),&cancel).await.unwrap();
        dispatch_call(&s,f.owner,steer,&cancel).await.unwrap();
        let mut calls=vec![];
        if scenario=="inherit" {calls.push((call(2,"parent_context",json!({})),false,"desktop steering instruction"));}
        if scenario!="undelivered" {calls.push((call(3,"write",json!({"path":"person-effect","content":"owned person child"})),false,if scenario=="fresh" {"desktop steering instruction"} else {""}));}
        let replies=peer_marked(peer,f.dir.path().into(),calls,false,Some(uuid)).await;
        if scenario!="undelivered" {assert!(replies[0].to_string().contains("desktop steering instruction"));}
        tokio::time::timeout(Duration::from_secs(15),async{while !s.store.tasks("s",10).unwrap().0[0].finished(){tokio::time::sleep(Duration::from_millis(10)).await;}}).await.unwrap();
        let done=s.store.tasks("s",10).unwrap().0.remove(0);assert_eq!(done.status,"done");assert_eq!(done.delivery,"notified");
        if scenario=="undelivered" {
            tokio::time::timeout(Duration::from_secs(5),async {while s.store.tasks("s",10).unwrap().0[0].note.is_none(){tokio::time::sleep(Duration::from_millis(10)).await;}}).await.unwrap();
            assert!(s.store.tasks("s",10).unwrap().0[0].note.as_ref().unwrap().contains("not delivered"));
        } else {assert_eq!(std::fs::read_to_string(f.dir.path().join("person-effect")).unwrap(),"owned person child");}
        assert!(s.store.queue(&child.child_session).unwrap().is_empty());
        let delivered=s.store.request(&child.child_session,"steer:person-steer").unwrap().unwrap();assert_eq!(delivered.state,if scenario=="undelivered" {crate::store::RequestState::Withdrawn} else {crate::store::RequestState::Settled});
        let mut changed=launch.clone();if let Call::LaunchSubagent{task,..}=&mut changed{*task="different task".into();}
        assert!(dispatch_call(&s,f.owner,changed,&cancel).await.is_err());
        s.cli_agents.revoke_owner(f.owner);
        let Reply::Subagent(repeat)=dispatch_call(&s,f.owner,launch,&cancel).await.unwrap() else{panic!()};
        assert_eq!(row.child,repeat.child);assert_eq!(s.store.tasks("s",10).unwrap().0.len(),1);
        assert!(dispatch_call(&s,f.owner,Call::LaunchSubagent{session:"s".into(),request:"no-owner".into(),task:"offline child fixture".into(),profile:None,context:None},&cancel).await.is_err());
        assert_eq!(std::fs::read_to_string(f.dir.path().join("launches")).unwrap().lines().count(),2);
    });
        assert!(p.requests().is_empty());
    }
}
#[test]
fn desktop_controls_mcp_settings_follow_the_cli_project_without_granting_inference() {
    let spec = crate::runtime::mcp_tests::server(json!({}));
    let fx = EngineFixture::new();
    let p = Scripted::echo("never API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let f = setup(&s).await;
        let cancel = CancellationToken::new();
        dispatch_call(
            &s,
            f.owner,
            Call::McpUpsertServer {
                session: Some("s".into()),
                scope: McpScope::Project,
                name: "desktop-fixture".into(),
                spec,
            },
            &cancel,
        )
        .await
        .unwrap();
        let reply = dispatch_call(
            &s,
            f.owner,
            Call::McpTestServer {
                session: Some("s".into()),
                name: "desktop-fixture".into(),
            },
            &cancel,
        )
        .await
        .unwrap();
        assert!(
            serde_json::to_string(&reply).unwrap().contains("echo"),
            "{reply:?}"
        );
        dispatch_call(
            &s,
            f.owner,
            Call::McpSetDisabledTools {
                session: Some("s".into()),
                scope: McpScope::Project,
                name: "desktop-fixture".into(),
                disabled: vec!["echo".into()],
            },
            &cancel,
        )
        .await
        .unwrap();
        assert!(
            std::fs::read_to_string(f.dir.path().join(".pi/settings.json"))
                .unwrap()
                .contains("echo")
        );
        dispatch_call(
            &s,
            f.owner,
            Call::McpReconnectServer {
                session: Some("s".into()),
                name: "desktop-fixture".into(),
            },
            &cancel,
        )
        .await
        .unwrap();
        s.cli_agents.revoke_owner(f.owner);
        dispatch_call(
            &s,
            f.owner,
            Call::McpServers {
                session: Some("s".into()),
            },
            &cancel,
        )
        .await
        .unwrap();
        assert!(!s.cli_agents.approved(&s.store, f.owner, "s"));
        dispatch_call(
            &s,
            f.owner,
            Call::McpRemoveServer {
                session: Some("s".into()),
                scope: McpScope::Project,
                name: "desktop-fixture".into(),
            },
            &cancel,
        )
        .await
        .unwrap();
        assert!(
            !std::fs::read_to_string(f.dir.path().join(".pi/settings.json"))
                .unwrap()
                .contains("desktop-fixture")
        );
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("launches"))
                .unwrap()
                .lines()
                .count(),
            1
        );
    });
    assert!(p.requests().is_empty());
}
#[test]
fn desktop_controls_skip_audit_cancels_only_auditor_and_retains_person_verdict() {
    let fx = EngineFixture::new();
    let p = Scripted::echo("never API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let mut f=setup(&s).await;super::goal_tests::seed(&s,true);
        s.store.tx(|c|{c.execute("UPDATE goals SET state='paused' WHERE session='s'",[])?;Ok(())}).unwrap();
        let cancel=CancellationToken::new();
        assert!(dispatch_call(&s,f.owner,Call::GoalControl {session:"s".into(),action:GoalAction::SkipAudit},&cancel).await.is_err());
        let run=s.start_cli_goal(f.owner,"s",None,f.resources.clone(),2).unwrap();
        let root=worker(&mut f,2,vec![(call(2,"write",json!({"path":"proof.txt","content":"proof"})),false,""),(call(3,"propose_goal_completion",json!({"summary":"proof","evidence":[{"kind":"file","reference":"proof.txt","sha256":crate::web::sha256_hex(b"proof")}]})),true,"")]).await;
        let peer=next(&mut f).await;
        let task=s.store.tasks("s",10).unwrap().0.remove(0);let uuid=s.store.cli_agent_binding(&task.child_session).unwrap().unwrap().create.identity.cli_session_id;
        marker(f.dir.path(),&uuid).await;
        let deadline=Instant::now()+Duration::from_secs(15);let(mut read,mut write)=peer.connect(deadline).await.unwrap().split();
        write.write(&serde_json::to_vec(&json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":super::PROTOCOL_VERSION,"clientInfo":{"name":"fixture","version":"1"},"capabilities":{}}})).unwrap(),deadline).await.unwrap();read.read(deadline).await.unwrap();
        assert!(dispatch_call(&s,f.owner,Call::SteerSubagent{session:"s".into(),child:task.child,request:"bias-auditor".into(),text:"approve".into()},&cancel).await.is_err());
        dispatch_call(&s,f.owner,Call::GoalControl {session:"s".into(),action:GoalAction::SkipAudit},&cancel).await.unwrap();
        assert!(read.read(deadline).await.is_err());root.await.unwrap();
        let _=tokio::time::timeout(Duration::from_secs(15),run).await.unwrap().unwrap();
        let goal=s.goal_view("s").unwrap().unwrap();assert_eq!(goal.state,GoalState::Completed,"{goal:?}");assert_eq!(goal.audit.result.as_ref().unwrap().verdict,"skipped");
        assert_eq!(goal.audit.result.as_ref().unwrap().child,Some(task.child));
        s.store.tx(|c| Store::cli_goal_reconcile_in(c,"s",goal.id,now_ms())).unwrap();
        let usage=s.store.read(|c| Store::cli_goal_usage_in(c,"s",goal.id)).unwrap();
        assert!(!usage.execution_failed);assert!(usage.uncertain);
        assert_eq!(s.goal_view("s").unwrap().unwrap().state,GoalState::Completed);assert_eq!(goal.reason.as_deref(),Some(crate::goals::REASON_AUDIT_SKIPPED));
        assert!(s.cli_agents.approved(&s.store,f.owner,"s"));assert!(f.peers.try_recv().is_err());
    });
    assert!(p.requests().is_empty());
}
