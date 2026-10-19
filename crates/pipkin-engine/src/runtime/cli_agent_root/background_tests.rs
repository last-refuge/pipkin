//! Background scheduling and lifecycle with owned offline processes and native MCP peers.
use super::goal_lifecycle_tests::configured;
use super::service_tests::{call, peer_marked};
use super::*;
use crate::{
    provider::{BoxFuture, ToolSpec, scripted::Scripted},
    runtime::{cli_agent_inputs::Input, tests::Fixture as EngineFixture},
    tools::{Tool, ToolCtx, ToolError, ToolOutput},
};

fn owner(s: &crate::runtime::Shared) -> u64 {
    *s.clients.lock().unwrap().keys().next().unwrap()
}
async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
async fn idle(s: &crate::runtime::Shared) {
    until(|| s.external_tasks.is_empty()).await;
}
async fn next(
    peers: &mut mpsc::UnboundedReceiver<cli_bridge_channel::Launch>,
) -> cli_bridge_channel::Launch {
    tokio::time::timeout(Duration::from_secs(10), peers.recv())
        .await
        .unwrap()
        .unwrap()
}
#[test]
fn background_wakes_yield_to_user_input_keep_ready_order_and_precede_goals() {
    let fx = EngineFixture::new();
    let p = Scripted::echo("no API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let mut f=Fixture::new_in("job-wake",Some((s.clone(),owner(&s)))).await;
        let uuid=f.launch.ownership.binding().create.identity.cli_session_id.clone();
        let (send,mut peers)=mpsc::unbounded_channel();
        f.launch.services.child_peers=Some(send.clone());
        let mut resources=(*s.capture_cli_child_resources("s",f.launch.assets.system_text(),f.launch.tools.clone(),f.launch.assets.image(),None).unwrap()).clone();
        resources.peers=Some(send);
        let requests=(1..=2).map(|id|(call(id+1,"job_start",json!({"command":format!("while [ ! -f finish-{id} ]; do sleep 0.02; done; printf 'done'"),"wake":true,"timeout":20})),false,if id==1 {"Started job #1"} else {"Started job #2"})).collect();
        let peer=tokio::spawn(peer_marked(f.peer,f.dir.path().into(),requests,false,Some(uuid.clone())));
        assert_eq!(s.spawn_cli_agent_root(f.launch).await.unwrap().unwrap().state,cli_agent_turns::State::Completed); peer.await.unwrap();
        let hold=s.cli_agents.begin_turn(&s.store,owner(&s),"s").unwrap();
        // Complete in reverse launch order and observe each ready ticket before the next.
        std::fs::write(f.dir.path().join("finish-2"),"").unwrap();
        until(||s.cli_agents.ready_wakes("s")==1).await;
        std::fs::write(f.dir.path().join("finish-1"),"").unwrap();
        until(||s.cli_agents.ready_wakes("s")==2).await;
        let queued=s.enqueue_cli_input(Input { images: vec![],owner:owner(&s),session:"s".into(),request:"human-first".into(),generation:2,operation:"human-op".into(),kind:crate::store::cli_agent_turns::inputs::Kind::FollowUp,text:"queued human first".into()},Arc::new(resources)).unwrap();
        super::goal_tests::seed(&s,false);
        let goal=s.cli_agents.goal_owner(&s.store,owner(&s),"s").unwrap();
        {
            let _gate=s.publication.lock().unwrap();
            drop(hold);
            assert!(matches!(goal.begin(&s.store,cli_agent_turns::GoalTurn {view:s.goal_view("s").unwrap().unwrap()}),Err(crate::runtime::cli_agent_owner::Error::Busy)));
        }
        drop(goal);
        for turn in 2..=4 {
            peer_marked(next(&mut peers).await,f.dir.path().into(),vec![],false,Some(format!("{uuid}-{turn}"))).await;
        }
        assert_eq!(queued.completion.unwrap().await.unwrap().unwrap().state,cli_agent_turns::State::Completed);
        idle(&s).await;
        let human=s.store.cli_agent_turn("s","human-first").unwrap().unwrap();
        let second=s.store.cli_agent_turn("s","cli-job-wake:2").unwrap().unwrap();
        let first=s.store.cli_agent_turn("s","cli-job-wake:1").unwrap().unwrap();
        assert!(human.scope.run<second.scope.run && second.scope.run<first.scope.run);
        assert_eq!(s.cli_agents.ready_wakes("s"),0);
        assert_eq!(std::fs::read_to_string(f.dir.path().join(format!("prompt-{uuid}-2"))).unwrap(),"queued human first");
        assert!(s.jobs_view("s").unwrap().jobs.iter().all(|j|j.delivery=="woken"));
        assert!(peers.try_recv().is_err());
    });
    assert!(p.requests().is_empty());
}
#[test]
fn background_job_wake_outlives_launching_turn_deadline() {
    let fx = EngineFixture::new();
    let p = Scripted::echo("no API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let mut f=Fixture::new_in("job-short",Some((s.clone(),owner(&s)))).await;
        let uuid=f.launch.ownership.binding().create.identity.cli_session_id.clone();
        let deadline=f.launch.server.deadline();
        let (send,mut peers)=mpsc::unbounded_channel(); f.launch.services.child_peers=Some(send);
        let peer=tokio::spawn(peer_marked(f.peer,f.dir.path().into(),vec![(call(2,"job_start",json!({"command":"while [ ! -f finish-job ]; do sleep 0.02; done; printf 'late output'","wake":true,"timeout":20})),false,"Started job #1")],false,Some(uuid.clone())));
        assert_eq!(s.spawn_cli_agent_root(f.launch).await.unwrap().unwrap().state,cli_agent_turns::State::Completed); peer.await.unwrap();
        tokio::time::sleep_until(deadline+Duration::from_millis(100)).await;
        assert_eq!(s.job_view("s",1).unwrap().delivery,"pending");
        std::fs::write(f.dir.path().join("finish-job"),"").unwrap();
        peer_marked(next(&mut peers).await,f.dir.path().into(),vec![(call(2,"job_log",json!({"id":1})),false,"late output")],false,Some(format!("{uuid}-2"))).await;
        idle(&s).await;
        assert_eq!(s.job_view("s",1).unwrap().delivery,"woken");
        assert_eq!(s.store.cli_agent_turn("s","cli-job-wake:1").unwrap().unwrap().state,cli_agent_turns::State::Completed);
    });
    assert!(p.requests().is_empty());
}
struct HoldReceipt {
    started: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}
impl Tool for HoldReceipt {
    fn spec(&self) -> ToolSpec {
        crate::tools::jobs::JobStartTool.spec()
    }
    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolCtx,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let output = crate::tools::jobs::JobStartTool.execute(args, ctx).await?;
            self.started
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .send(())
                .unwrap();
            ctx.cancel.cancelled().await;
            Ok(output)
        })
    }
}
#[test]
fn background_spawn_without_committed_tool_receipt_is_cancelled_and_never_wakes() {
    for wake in [false, true] {
        let fx = EngineFixture::new();
        let p = Scripted::echo("no API");
        let e = configured(&fx, &p);
        let s = e.handle.shared().clone();
        s.handle.block_on(async {
            let mut f=Fixture::new_in("job-wake",Some((s.clone(),owner(&s)))).await;
            let uuid=f.launch.ownership.binding().create.identity.cli_session_id.clone();
            let (send,mut peers)=mpsc::unbounded_channel(); f.launch.services.child_peers=Some(send);
            let (started,ready)=tokio::sync::oneshot::channel();
            let index=f.launch.tools.iter().position(|t|t.spec().name=="job_start").unwrap();
            f.launch.tools[index]=Arc::new(HoldReceipt {started:std::sync::Mutex::new(Some(started))});
            let peer=tokio::spawn(peer_marked(f.peer,f.dir.path().into(),vec![(call(2,"job_start",json!({"command":"printf started > started-job; sleep 60","wake":wake,"timeout":20})),false,"")],true,Some(uuid.clone())));
            let result=s.spawn_cli_agent_root(f.launch);
            tokio::time::timeout(Duration::from_secs(10),ready).await.unwrap().unwrap();
            until(||f.dir.path().join("started-job").exists()).await;
            s.stop_by_backend("s").await.unwrap();
            assert!(result.await.unwrap().is_err()); peer.await.unwrap(); idle(&s).await;
            assert_eq!(s.job_view("s",1).unwrap().state,pipkin_engine_protocol::JobState::Cancelled);
            assert!(s.store.cli_agent_turn("s","cli-job-wake:1").unwrap().is_none());
            assert!(peers.try_recv().is_err()); assert_eq!(s.cli_agents.ready_wakes("s"),0);
        });
        assert!(p.requests().is_empty());
    }
}
#[test]
fn background_child_job_resumes_frozen_child_and_updates_parent_or_suppresses_after_stop() {
    for end in ["complete", "parent-wake", "stop", "revoke"] {
        let fx = EngineFixture::new();
        let p = Scripted::echo("no API");
        let e = configured(&fx, &p);
        let s = e.handle.shared().clone();
        s.handle.block_on(async {
            let mut f=Fixture::new_in("background",Some((s.clone(),owner(&s)))).await;
            let uuid=f.launch.ownership.binding().create.identity.cli_session_id.clone();
            let (send,mut peers)=mpsc::unbounded_channel(); f.launch.services.child_peers=Some(send);
            let parent=tokio::spawn(peer_marked(f.peer,f.dir.path().into(),vec![(call(2,"subagent",json!({"task":"offline child fixture","wait":false,"wake":end=="parent-wake","context":"inherit"})),false,"starting in the background")],false,Some(uuid.clone())));
            let root=s.spawn_cli_agent_root(f.launch);
            let child_peer=next(&mut peers).await;
            let task=s.store.read(|c|Store::task_by_child(c,"s",1)).unwrap().unwrap();
            let child=task.child_session;
            let binding=s.store.cli_agent_binding(&child).unwrap().unwrap();
            let cuuid=binding.create.identity.cli_session_id;
            peer_marked(child_peer,f.dir.path().into(),vec![(call(2,"parent_context",json!({})),false,"offline root fixture"),(call(3,"job_start",json!({"command":"while [ ! -f finish-child-job ]; do sleep 0.02; done; printf 'child job output'","wake":true,"timeout":20})),false,"Started job #1")],false,Some(cuuid.clone())).await;
            assert_eq!(root.await.unwrap().unwrap().state,cli_agent_turns::State::Completed); parent.await.unwrap();
            until(||s.store.read(|c|Store::task_by_session(c,&child)).unwrap().unwrap().status=="done").await;
            match end {"stop"=>{s.stop_by_backend(&child).await.unwrap();},"revoke"=>s.cli_agents.revoke_session("s"),_=>{}}
            std::fs::write(f.dir.path().join("finish-child-job"),"").unwrap();
            if end=="parent-wake" {
                peer_marked(next(&mut peers).await,f.dir.path().into(),vec![],false,Some(format!("{uuid}-2"))).await;
            }
            if matches!(end,"complete"|"parent-wake") {
                let resumed=next(&mut peers).await;
                assert_eq!(s.store.read(|c|Store::task_by_session(c,&child)).unwrap().unwrap().status,"running");
                peer_marked(resumed,f.dir.path().into(),vec![(call(2,"parent_context",json!({})),false,"offline root fixture"),(call(3,"job_log",json!({"id":1})),false,"child job output")],false,Some(format!("{cuuid}-2"))).await;
            }
            idle(&s).await;
            let task=s.store.read(|c|Store::task_by_session(c,&child)).unwrap().unwrap();
            assert_eq!(task.status,"done"); assert_eq!(task.delivery,if end=="parent-wake" {"woken"} else {"notified"});
            let turn=s.store.cli_agent_turn(&child,"cli-job-wake:1").unwrap();
            if matches!(end,"complete"|"parent-wake") {
                let turn=turn.unwrap(); assert_eq!(turn.state,cli_agent_turns::State::Completed); assert_eq!(turn.mode,crate::execution::claude_cli::policy::TurnMode::Resume);
                assert_eq!(std::fs::read(f.dir.path().join(format!("system-{cuuid}"))).unwrap(),std::fs::read(f.dir.path().join(format!("system-{cuuid}-2"))).unwrap());
                assert_eq!(s.job_view(&child,1).unwrap().delivery,"woken");
            } else {assert!(turn.is_none()); assert_eq!(s.job_view(&child,1).unwrap().delivery,"suppressed");}
            assert!(peers.try_recv().is_err());
        });
        assert!(p.requests().is_empty());
    }
}
