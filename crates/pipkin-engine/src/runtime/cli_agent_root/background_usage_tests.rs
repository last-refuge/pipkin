//! Goal/background accounting against owned CLI and authenticated native MCP fixtures only.
use super::goal_lifecycle_tests::{Ready, configured, next, response, setup_variant, worker};
use super::service_tests::{call, peer_marked};
use super::*;
use crate::{provider::scripted::Scripted, runtime::tests::Fixture as EngineFixture};
use pipkin_engine_protocol::{GoalState, GoalView};
async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(12), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
fn seed(s: &crate::runtime::Shared, turns: Option<u64>, tokens: Option<u64>) {
    super::goal_tests::seed(s, false);
    s.store
        .tx(|c| {
            let mut g = Store::goal_current(c, "s")?.unwrap();
            g.limits.max_turns = turns;
            g.limits.max_tokens = tokens;
            Store::goal_write_progress(c, "s", &g, now_ms())?;
            Store::goal_set_state(c, "s", g.id, GoalState::Paused, None, now_ms())
        })
        .unwrap();
}
async fn finish(run: tokio::sync::oneshot::Receiver<Result<GoalView, String>>) -> GoalView {
    tokio::time::timeout(Duration::from_secs(15), run)
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}
fn job(id: u64, file: &str) -> (Value, bool, &'static str) {
    (
        call(
            id,
            "job_start",
            json!({"command":format!("while [ ! -f {file} ]; do sleep 0.02; done; printf ready"),"wake":true,"timeout":30}),
        ),
        false,
        "Started job #1",
    )
}
fn continue_goal(id: u64) -> (Value, bool, &'static str) {
    (
        call(id, "update_goal", json!({"status":"continue"})),
        false,
        "",
    )
}
fn set_usage(f: &Ready, marker: &str, usage: Value) {
    let path = f.dir.path().join(format!("terminal-{marker}"));
    let mut v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    v["usage"] = usage;
    std::fs::write(path, format!("{v}\n")).unwrap();
}
#[test]
fn background_usage_job_wake_counts_all_dimensions_once_and_uses_turn_allowance() {
    let fx = EngineFixture::new();
    let p = Scripted::echo("no API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let mut f=setup_variant(&s,"goal-background").await;seed(&s,Some(2),Some(100));
        let run=s.start_cli_goal(f.owner,"s",None,f.resources.clone(),2).unwrap();
        worker(&mut f,2,vec![job(2,"finish-job"),continue_goal(3)]).await.await.unwrap();
        until(||s.goal_view("s").unwrap().unwrap().usage_tokens==7).await;
        assert!(f.peers.try_recv().is_err());
        std::fs::write(f.dir.path().join("finish-job"),"").unwrap();
        let peer=next(&mut f).await;let marker=format!("{}-3",f.uuid);response(f.dir.path(),&marker,&f.uuid,"wake done");
        set_usage(&f,&marker,json!({"input_tokens":2,"output_tokens":3,"cache_read_input_tokens":5,"cache_creation_input_tokens":7}));
        peer_marked(peer,f.dir.path().into(),vec![],false,Some(marker)).await;
        let g=finish(run).await;assert_eq!(g.state,GoalState::Paused);assert_eq!(g.turns,2);assert_eq!(g.usage_tokens,24);assert!(g.reason.unwrap().to_lowercase().contains("turn"));
        let wake=s.store.cli_agent_turn("s","cli-job-wake:1").unwrap().unwrap();
        let first=s.store.read(|c|Store::cli_goal_usage_in(c,"s",g.id)).unwrap();assert!(first.complete && !first.waiting);assert_eq!(first.known_tokens,24);
        assert!(s.store.tx(|c|Store::cli_goal_reconcile_run_in(c,"s",wake.scope.run,now_ms())).unwrap().is_empty());
        assert_eq!(s.goal_view("s").unwrap().unwrap().usage_tokens,24);
        assert_eq!(std::fs::read_to_string(f.dir.path().join("launch-identities")).unwrap().lines().count(),3);
    });
    assert!(p.requests().is_empty());
}
#[test]
fn background_usage_child_parent_wake_and_child_job_wake_share_original_goal() {
    let fx = EngineFixture::new();
    let p = Scripted::echo("no API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let mut f=setup_variant(&s,"goal-background").await;seed(&s,Some(3),Some(100));
        let run=s.start_cli_goal(f.owner,"s",None,f.resources.clone(),2).unwrap();
        let parent=worker(&mut f,2,vec![(call(2,"subagent",json!({"task":"offline child fixture","wait":false,"wake":true,"context":"fresh"})),false,"starting in the background"),(call(3,"propose_goal_completion",json!({"summary":"premature","evidence":[]})),true,"unsettled"),continue_goal(4)]).await;
        let peer=next(&mut f).await;
        let child=s.store.read(|c|Store::task_by_child(c,"s",1)).unwrap().unwrap().child_session;
        let uuid=s.store.cli_agent_binding(&child).unwrap().unwrap().create.identity.cli_session_id;
        response(f.dir.path(),&uuid,&uuid,"child done");
        peer_marked(peer,f.dir.path().into(),vec![job(2,"finish-job")],false,Some(uuid.clone())).await;parent.await.unwrap();
        worker(&mut f,3,vec![]).await.await.unwrap();
        until(||s.goal_view("s").unwrap().unwrap().usage_tokens==21).await;
        std::fs::write(f.dir.path().join("finish-job"),"").unwrap();
        let peer=next(&mut f).await;let marker=format!("{uuid}-2");response(f.dir.path(),&marker,&uuid,"child wake done");
        set_usage(&f,&marker,json!({"input_tokens":2,"output_tokens":3,"cache_read_input_tokens":5,"cache_creation_input_tokens":7}));
        peer_marked(peer,f.dir.path().into(),vec![],false,Some(marker)).await;
        let g=finish(run).await;assert_eq!(g.state,GoalState::Paused);assert_eq!(g.turns,3);assert_eq!(g.usage_tokens,38);
        until(||s.external_tasks.is_empty()).await;
        assert!(f.peers.try_recv().is_err());
        assert_eq!(s.store.read(|c|Store::cli_goal_usage_in(c,"s",g.id)).unwrap().known_tokens,38);
    });
    assert!(p.requests().is_empty());
}
#[test]
fn background_usage_exhausted_unknown_and_dollar_allowances_suppress_new_wakes() {
    for case in ["turns", "tokens", "missing", "cost"] {
        let fx = EngineFixture::new();
        let p = Scripted::echo("no API");
        let e = configured(&fx, &p);
        let s = e.handle.shared().clone();
        s.handle.block_on(async {
            let mut f = setup_variant(&s, "goal-background").await;
            seed(
                &s,
                if case == "turns" { Some(1) } else { None },
                if case == "tokens" { Some(7) } else { Some(100) },
            );
            if case == "missing" {
                std::fs::write(f.dir.path().join("missing-usage"), "").unwrap();
            }
            let run = s
                .start_cli_goal(f.owner, "s", None, f.resources.clone(), 2)
                .unwrap();
            worker(&mut f, 2, vec![job(2, "finish-job"), continue_goal(3)])
                .await
                .await
                .unwrap();
            until(|| s.goal_view("s").unwrap().unwrap().usage_tokens == 7).await;
            if case == "cost" {
                s.store
                    .tx(|c| {
                        let mut g = Store::goal_current(c, "s")?.unwrap();
                        g.limits.max_cost_usd = Some(1.0);
                        Store::goal_write_progress(c, "s", &g, now_ms())
                    })
                    .unwrap();
            }
            let g = finish(run).await;
            assert_eq!(g.state, GoalState::Paused);
            assert_eq!(g.turns, 1);
            std::fs::write(f.dir.path().join("finish-job"), "").unwrap();
            until(|| s.external_tasks.is_empty()).await;
            assert_eq!(s.job_view("s", 1).unwrap().delivery, "suppressed");
            assert!(
                s.store
                    .cli_agent_turn("s", "cli-job-wake:1")
                    .unwrap()
                    .is_none()
            );
            assert!(f.peers.try_recv().is_err());
        });
        assert!(p.requests().is_empty());
    }
}
#[test]
fn background_usage_late_child_receipt_charges_archived_goal_not_replacement() {
    let fx = EngineFixture::new();
    let p = Scripted::echo("no API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let mut f=setup_variant(&s,"goal-background").await;seed(&s,None,None);
        let run=s.start_cli_goal(f.owner,"s",None,f.resources.clone(),2).unwrap();
        let parent=worker(&mut f,2,vec![(call(2,"subagent",json!({"task":"offline child fixture","wait":false,"wake":false,"context":"fresh"})),false,"starting in the background"),continue_goal(3)]).await;
        let peer=next(&mut f).await;let child=s.store.read(|c|Store::task_by_child(c,"s",1)).unwrap().unwrap().child_session;
        let uuid=s.store.cli_agent_binding(&child).unwrap().unwrap().create.identity.cli_session_id;
        response(f.dir.path(),&uuid,&uuid,"late child");
        let running=tokio::spawn(peer_marked(peer,f.dir.path().into(),vec![(call(2,"job_start",json!({"command":"while [ ! -f finish-child ]; do sleep 0.02; done","timeout":30,"wake":false})),false,"Started job #1"),(call(3,"job_wait",json!({"id":1,"timeout":30})),false,"succeeded")],false,Some(uuid)));
        until(||s.job_view(&child,1).is_ok()).await;
        parent.await.unwrap();until(||!s.cli_agents.has_turn("s")).await;s.cli_agents.stop("s");let old=finish(run).await;assert_eq!(old.state,GoalState::Paused);
        let replacement=s.store.tx(|c|Store::goal_insert(c,"s","replacement",GoalState::Paused,None,"user","replace",now_ms())).unwrap();
        std::fs::write(f.dir.path().join("finish-child"),"").unwrap();running.await.unwrap();
        until(||s.external_tasks.is_empty()).await;
        let old=s.store.read(|c|Store::goal_by_id(c,"s",old.id)).unwrap().unwrap();assert_eq!(old.usage_tokens,14);
        let new=s.goal_view("s").unwrap().unwrap();assert_eq!(new.id,replacement.id);assert_eq!(new.usage_tokens,0);assert_eq!(new.turns,0);assert!(f.peers.try_recv().is_err());
    });
    assert!(p.requests().is_empty());
}
#[test]
fn background_usage_capacity_wait_rechecks_reduced_allowance_before_admission() {
    for limit in ["tokens", "turns"] {
        let fx = EngineFixture::new();
        let p = Scripted::echo("no API");
        let e = configured(&fx, &p);
        let s = e.handle.shared().clone();
        s.handle.block_on(async {
            let mut f = setup_variant(&s, "goal-background").await;
            seed(&s, Some(2), Some(100));
            let run = s
                .start_cli_goal(f.owner, "s", None, f.resources.clone(), 2)
                .unwrap();
            worker(&mut f, 2, vec![job(2, "finish-job"), continue_goal(3)])
                .await
                .await
                .unwrap();
            until(|| {
                s.goal_view("s").unwrap().unwrap().usage_tokens == 7 && !s.cli_agents.has_turn("s")
            })
            .await;
            let permit = s
                .generations
                .clone()
                // Registry retirement precedes lifetime cleanup. Wait for every
                // permit, including the parent's still-held cleanup permit.
                .acquire_many_owned(s.config.max_active_generations.max(1) as u32)
                .await
                .unwrap();
            std::fs::write(f.dir.path().join("finish-job"), "").unwrap();
            until(|| s.cli_agents.has_turn("s")).await;
            assert!(
                s.store
                    .cli_agent_turn("s", "cli-job-wake:1")
                    .unwrap()
                    .is_none()
            );
            s.store
                .tx(|c| {
                    let mut g = Store::goal_current(c, "s")?.unwrap();
                    if limit == "tokens" {
                        g.limits.max_tokens = Some(7);
                    } else {
                        g.limits.max_turns = Some(1);
                    }
                    Store::goal_write_progress(c, "s", &g, now_ms())
                })
                .unwrap();
            drop(permit);
            let goal = finish(run).await;
            assert_eq!(goal.state, GoalState::Paused);
            until(|| s.external_tasks.is_empty()).await;
            assert!(
                s.store
                    .cli_agent_turn("s", "cli-job-wake:1")
                    .unwrap()
                    .is_none()
            );
            assert!(f.peers.try_recv().is_err());
            assert_eq!(
                std::fs::read_to_string(f.dir.path().join("launch-identities"))
                    .unwrap()
                    .lines()
                    .count(),
                2
            );
        });
        assert!(p.requests().is_empty());
    }
}
struct HoldCompletion {
    ready: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: std::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}
impl crate::tools::Tool for HoldCompletion {
    fn spec(&self) -> crate::provider::ToolSpec {
        crate::tools::goal::ProposeGoalCompletionTool.spec()
    }
    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a crate::tools::ToolCtx,
    ) -> crate::provider::BoxFuture<'a, Result<crate::tools::ToolOutput, crate::tools::ToolError>>
    {
        Box::pin(async move {
            let output = crate::tools::goal::ProposeGoalCompletionTool
                .execute(args, ctx)
                .await?;
            self.ready.lock().unwrap().take().unwrap().send(()).unwrap();
            let wait = self.release.lock().unwrap().take().unwrap();
            let _ = wait.await;
            Ok(output)
        })
    }
}
#[test]
fn background_usage_completion_waits_for_wake_terminal_receipt() {
    let fx = EngineFixture::new();
    let p = Scripted::echo("no API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let mut f=setup_variant(&s,"goal-background").await;seed(&s,Some(2),Some(100));
        let (ready,waiting)=tokio::sync::oneshot::channel();let (release,released)=tokio::sync::oneshot::channel();
        let mut resources=(*f.resources).clone();let index=resources.tools.iter().position(|t|t.spec().name=="propose_goal_completion").unwrap();
        resources.tools[index]=Arc::new(HoldCompletion {ready:std::sync::Mutex::new(Some(ready)),release:std::sync::Mutex::new(Some(released))});f.resources=Arc::new(resources);
        let mut run=s.start_cli_goal(f.owner,"s",None,f.resources.clone(),2).unwrap();
        worker(&mut f,2,vec![job(2,"finish-job"),continue_goal(3)]).await.await.unwrap();
        until(||s.goal_view("s").unwrap().unwrap().usage_tokens==7).await;
        std::fs::write(f.dir.path().join("finish-job"),"").unwrap();
        let wake=worker(&mut f,3,vec![(call(2,"write",json!({"path":"proof.txt","content":"proof"})),false,""),(call(3,"propose_goal_completion",json!({"summary":"proof","evidence":[{"kind":"file","reference":"proof.txt","sha256":crate::web::sha256_hex(b"proof")}]})),false,"Completion accepted")]).await;
        tokio::time::timeout(Duration::from_secs(10),waiting).await.unwrap().unwrap();
        assert_eq!(s.goal_view("s").unwrap().unwrap().state,GoalState::Completed);
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(matches!(run.try_recv(),Err(tokio::sync::oneshot::error::TryRecvError::Empty)));
        release.send(()).unwrap();wake.await.unwrap();let goal=finish(run).await;
        assert_eq!(goal.state,GoalState::Completed);assert_eq!(goal.usage_tokens,14);assert_eq!(goal.turns,2);
    });
    assert!(p.requests().is_empty());
}
