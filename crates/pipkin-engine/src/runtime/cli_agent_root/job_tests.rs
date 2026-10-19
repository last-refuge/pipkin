//! CLI native job workflows against the existing supervisor; no installed/model inference.
use super::service_tests::{call, peer};
use super::*;
use crate::{
    provider::{BoxFuture, ToolSpec, scripted::Scripted},
    runtime::tests::{Engine, Fixture as EngineFixture},
    tools::{Tool, ToolCtx, ToolError, ToolHost, ToolOutput},
};
use pipkin_engine_protocol::{Call, Event, ServerFrame, SessionChange};
fn engine(fx: &EngineFixture, p: &Scripted) -> Engine {
    let mut c = fx.config();
    c.enable_local_claude_cli = true;
    c.enable_local_claude_cli_orchestration = true;
    fx.start_config(p, c)
}
fn owner(shared: &crate::runtime::Shared) -> u64 {
    *shared.clients.lock().unwrap().keys().next().unwrap()
}
async fn idle(shared: &crate::runtime::Shared) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !shared.external_tasks.is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
#[test]
fn native_jobs_start_read_wait_cancel_replay_and_recover_without_inference() {
    let fx = EngineFixture::new();
    let provider = Scripted::echo("no API");
    let e = engine(&fx, &provider);
    let shared = e.handle.shared().clone();
    shared.handle.block_on(async {
        let f=Fixture::new_in("jobs",Some((shared.clone(),owner(&shared)))).await;
        let start=call(2,"job_start",json!({"command":"printf 'native job output'; printf 'effect' > job-effect","name":"owned job"}));
        let requests=vec![(start.clone(),false,"Started job #1"),(start,false,"Started job #1"),
            (call(3,"job_wait",json!({"id":1,"timeout":5})),false,"succeeded"),
            (call(4,"job_log",json!({"id":1})),false,"native job output"),
            (call(5,"job_status",json!({})),false,"owned job"),
            (call(7,"job_start",json!({"command":"sleep 60","timeout":10})),false,"Started job #2"),
            (call(8,"job_cancel",json!({"id":2})),false,"cancelled"),
            (call(9,"job_wait",json!({"id":2,"timeout":1})),false,"cancelled"),
            (call(10,"job_log",json!({"id":9999})),true,"No job")];
        let peer=tokio::spawn(peer(f.peer,f.dir.path().into(),requests,false));
        assert_eq!(shared.spawn_cli_agent_root(f.launch).await.unwrap().unwrap().state,cli_agent_turns::State::Completed);
        let responses=peer.await.unwrap();assert_eq!(responses[0],responses[1]);idle(&shared).await;
        let jobs=shared.jobs_view("s").unwrap();assert_eq!(jobs.jobs.len(),2);assert_eq!(jobs.active,0);
        assert_eq!(std::fs::read_to_string(f.dir.path().join("job-effect")).unwrap(),"effect");assert!(!f.dir.path().join("forbidden-wake").exists());
        assert!(jobs.jobs.iter().all(|j|j.delivery=="notified"));
        let mut revisions=0;let mut views=0;
        while let Ok(ServerFrame::Event {event,..})=e.conn.try_recv() {if let Event::Session {revision,change,..}=event {assert!(revision>=revisions);revisions=revision;if matches!(change,SessionChange::JobsChanged {..}){views+=1;}}}
        assert!(views>=4);
        assert!(shared.guard_execution_call(&Call::JobCancel {session:"s".into(),id:1}).is_err());
        // Exercise restart classification with durable native joins, never launch authority.
        let joined=shared.store.read(|c|Store::job(c,"s",1)).unwrap().unwrap();
        assert!(shared.store.read(|c|Store::cli_job_cleanup_owned(c,&joined)).unwrap());
        let unjoined=shared.store.tx(|tx| {
            tx.execute("UPDATE jobs SET state='starting',pid=NULL,engine_epoch=0,delivery='pending' WHERE session='s' AND id=1",[])?;
            Store::job_create(tx,"s",&crate::store::jobs::JobIntent {name:None,command:"touch forbidden-recovery",cwd:f.dir.path().to_str().unwrap(),origin:"model",call_id:Some("sdk-observed-id"),run:joined.run,task:None,timeout_secs:None,wake:false,engine_epoch:0,nonce:"fixture",log_path:""},crate::util::now_ms())
        }).unwrap();
        assert!(!shared.store.read(|c|Store::cli_job_cleanup_owned(c,&unjoined)).unwrap());
        shared.jobs_recover();
        assert_eq!(shared.store.read(|c|Store::job(c,"s",1)).unwrap().unwrap().state,"unknown");
        assert_eq!(shared.store.read(|c|Store::job(c,"s",unjoined.id)).unwrap().unwrap().state,"starting");
        assert!(!f.dir.path().join("forbidden-recovery").exists());
    });
    assert!(provider.requests().is_empty());
}
struct DropWaiter {
    drop_now: std::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    dropped: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    finish: std::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}
impl Tool for DropWaiter {
    fn spec(&self) -> ToolSpec {
        crate::tools::jobs::JobStartTool.spec()
    }
    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolCtx,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let host: &Arc<dyn ToolHost> = ctx.host.as_ref().unwrap();
            assert!(
                host.job_start(
                    "printed-sdk-id",
                    crate::runtime::jobs::JobSpec {
                        command: "touch forbidden-forged".into(),
                        name: None,
                        cwd: None,
                        timeout_secs: None,
                        wake: false,
                        task: None
                    }
                )
                .await
                .is_err()
            );
            assert!(host.job_cancel(1).await.is_err());
            let drop_now = self.drop_now.lock().unwrap().take().unwrap();
            tokio::select! {
                _=crate::tools::jobs::JobStartTool.execute(args,ctx)=>panic!("job returned while startup is paused"),
                result=drop_now=>{result.unwrap();},
            }
            let _ = self.dropped.lock().unwrap().take().unwrap().send(());
            let waiting = self.finish.lock().unwrap().take().unwrap();
            let _ = waiting.await;
            Ok(ToolOutput {
                content: Some(vec![pipkin_engine_protocol::Block::text(
                    "waiter deliberately dropped",
                )]),
                ..ToolOutput::default()
            })
        })
    }
}
#[test]
fn startup_survives_dropped_waiter_and_stop_before_spawn_leaves_no_process() {
    for stop in [false, true] {
        let fx = EngineFixture::new();
        let provider = Scripted::echo("no API");
        let e = engine(&fx, &provider);
        let shared = e.handle.shared().clone();
        shared.handle.block_on(async {
            let mut f = Fixture::new_in("jobs", Some((shared.clone(), owner(&shared)))).await;
            if stop {
                super::goal_tests::seed(&shared, false);
            }
            let (entered, ready) = tokio::sync::oneshot::channel();
            let (resume, go) = tokio::sync::oneshot::channel();
            f.launch.services.job_start_pause =
                Some(Arc::new(std::sync::Mutex::new(Some((entered, go)))));
            let (drop_now, when) = tokio::sync::oneshot::channel();
            let (dropped, gone) = tokio::sync::oneshot::channel();
            let (finish, finished) = tokio::sync::oneshot::channel();
            if !stop {
                let index = f
                    .launch
                    .tools
                    .iter()
                    .position(|t| t.spec().name == "job_start")
                    .unwrap();
                f.launch.tools[index] = Arc::new(DropWaiter {
                    drop_now: std::sync::Mutex::new(Some(when)),
                    dropped: std::sync::Mutex::new(Some(dropped)),
                    finish: std::sync::Mutex::new(Some(finished)),
                });
            }
            let request = call(
                2,
                "job_start",
                json!({"command":"printf 'actor effect' > actor-effect"}),
            );
            let peer = tokio::spawn(peer(
                f.peer,
                f.dir.path().into(),
                vec![(request, false, "waiter deliberately dropped")],
                stop,
            ));
            let result = shared.spawn_cli_agent_root(f.launch);
            tokio::time::timeout(Duration::from_secs(5), ready)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                shared
                    .store
                    .read(|c| Store::job(c, "s", 1))
                    .unwrap()
                    .unwrap()
                    .state,
                "starting"
            );
            if stop {
                shared.stop_by_backend("s").await.unwrap();
                assert!(result.await.unwrap().is_err());
            } else {
                drop_now.send(()).unwrap();
                gone.await.unwrap();
                assert!(!shared.external_tasks.is_empty());
                let _ = resume.send(());
                tokio::time::timeout(Duration::from_secs(5), async {
                    loop {
                        let job = shared
                            .store
                            .read(|c| Store::job(c, "s", 1))
                            .unwrap()
                            .unwrap();
                        if !job.active() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                })
                .await
                .unwrap();
                let _ = finish.send(());
                assert_eq!(
                    result.await.unwrap().unwrap().state,
                    cli_agent_turns::State::Completed
                );
            }
            peer.await.unwrap();
            idle(&shared).await;
            let job = shared
                .store
                .read(|c| Store::job(c, "s", 1))
                .unwrap()
                .unwrap();
            assert_eq!(job.state, if stop { "cancelled" } else { "succeeded" });
            if stop {
                assert_eq!(
                    shared.goal_view("s").unwrap().unwrap().state,
                    pipkin_engine_protocol::GoalState::Paused
                );
            }
            assert_eq!(f.dir.path().join("actor-effect").exists(), !stop);
            assert!(!f.dir.path().join("forbidden-forged").exists());
            assert_eq!(shared.jobs_view("s").unwrap().jobs.len(), 1);
        });
        assert!(provider.requests().is_empty());
    }
}

#[test]
fn job_completion_resumes_original_cli_once_and_stop_or_revocation_suppresses_wake() {
    for end in ["complete", "idle-stop", "revoke"] {
        let fx = EngineFixture::new();
        let provider = Scripted::echo("no API fallback");
        let e = engine(&fx, &provider);
        let shared = e.handle.shared().clone();
        shared.handle.block_on(async {
            let mut f=Fixture::new_in("job-wake",Some((shared.clone(),owner(&shared)))).await;
            let uuid=f.launch.ownership.binding().create.identity.cli_session_id.clone();
            let (send,mut peers)=tokio::sync::mpsc::unbounded_channel();
            f.launch.services.child_peers=Some(send);
            let request=call(2,"job_start",json!({"command":"while [ ! -f finish-job ]; do sleep 0.02; done; printf 'completed once' > job-effect; printf 'job output'","wake":true,"timeout":20}));
            let parent=tokio::spawn(super::service_tests::peer_marked(f.peer,f.dir.path().into(),vec![(request.clone(),false,"Started job #1"),(request,false,"Started job #1")],false,Some(uuid.clone())));
            assert_eq!(shared.spawn_cli_agent_root(f.launch).await.unwrap().unwrap().state,cli_agent_turns::State::Completed);
            let replies=parent.await.unwrap();assert_eq!(replies[0],replies[1]);
            assert_eq!(shared.jobs_view("s").unwrap().jobs.len(),1);
            match end {
                "idle-stop"=>{shared.stop_by_backend("s").await.unwrap();},
                "revoke"=>shared.cli_agents.revoke_session("s"),
                _=>{},
            }
            std::fs::write(f.dir.path().join("finish-job"),"finish").unwrap();
            let view=shared.job_wait("s",1,Duration::from_secs(5),&CancellationToken::new()).await.unwrap();
            assert_eq!(view.state,pipkin_engine_protocol::JobState::Succeeded);
            if end=="complete" {
                let resumed=tokio::time::timeout(Duration::from_secs(10),peers.recv()).await.unwrap().unwrap();
                super::service_tests::peer_marked(resumed,f.dir.path().into(),vec![(call(2,"job_log",json!({"id":1})),false,"job output")],false,Some(format!("{uuid}-2"))).await;
            }
            idle(&shared).await;
            let view=shared.job_view("s",1).unwrap();
            assert_eq!(view.delivery,if end=="complete" {"woken"} else {"suppressed"});
            let receipt=shared.store.cli_agent_turn("s","cli-job-wake:1").unwrap();
            if end=="complete" {
                let receipt=receipt.unwrap();
                assert_eq!(receipt.state,cli_agent_turns::State::Completed);
                assert_eq!(receipt.mode,crate::execution::claude_cli::policy::TurnMode::Resume);
            } else {assert!(receipt.is_none());}
            assert_eq!(std::fs::read_to_string(f.dir.path().join("job-effect")).unwrap(),"completed once");
            let identities=std::fs::read_to_string(f.dir.path().join("launch-identities")).unwrap();
            assert_eq!(identities.lines().count(),if end=="complete" {2} else {1});
            assert_eq!(identities.contains(&format!("--resume {uuid} 2")),end=="complete");
            // Simulate delivery lost at restart. Recovery suppresses rather than reconstructing a ticket.
            shared.store.tx(|c| {c.execute("UPDATE jobs SET delivery='pending' WHERE session='s' AND id=1",[])?;Ok(())}).unwrap();
            shared.jobs_recover();
            tokio::time::timeout(Duration::from_secs(5),async {
                while shared.job_view("s",1).unwrap().delivery=="pending" {tokio::time::sleep(Duration::from_millis(5)).await;}
            }).await.unwrap();
            assert_eq!(shared.job_view("s",1).unwrap().delivery,"suppressed");
            assert!(peers.try_recv().is_err());
        });
        assert!(provider.requests().is_empty());
    }
}
