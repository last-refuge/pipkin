//! Seeded private goals exercise reporting/evidence, not CLI goal turn scheduling.
use super::service_tests::{call, peer};
use super::*;
use crate::{
    provider::{BoxFuture, ToolSpec, scripted::Scripted},
    runtime::tests::{Engine, Fixture as EngineFixture},
    tools::{Tool, ToolCtx, ToolError, ToolHost, ToolOutput},
};
use pipkin_engine_protocol::{Call, Event, GoalState, ServerFrame, SessionChange};
fn engine(fx: &EngineFixture, p: &Scripted) -> Engine {
    let mut c = fx.config();
    c.enable_local_claude_cli = true;
    c.enable_local_claude_cli_orchestration = true;
    fx.start_config(p, c)
}
fn owner(shared: &crate::runtime::Shared) -> u64 {
    *shared.clients.lock().unwrap().keys().next().unwrap()
}
pub(super) fn seed(shared: &crate::runtime::Shared, audit: bool) {
    shared
        .store
        .tx(|tx| {
            let mut goal = Store::goal_insert(
                tx,
                "s",
                "native reporting goal",
                GoalState::Active,
                None,
                "fixture",
                "replaced",
                crate::util::now_ms(),
            )?;
            goal.audit.required = audit;
            Store::goal_write_progress(tx, "s", &goal, crate::util::now_ms())?;
            tx.execute(
                "UPDATE goals SET last_request='r' WHERE session='s' AND id=?1",
                [goal.id as i64],
            )?;
            Ok(())
        })
        .unwrap();
}
type Captured = Arc<std::sync::Mutex<Option<(Arc<dyn ToolHost>, String)>>>;
struct GuardUpdate(Captured);
impl Tool for GuardUpdate {
    fn spec(&self) -> ToolSpec {
        crate::tools::goal::UpdateGoalTool.spec()
    }
    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolCtx,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let host = ctx.host.as_ref().unwrap();
            let request = serde_json::from_value(args.clone()).unwrap();
            assert!(host.goal_update("sdk-observed-id", request).await.is_err());
            assert!(
                host.goal_propose(
                    &ctx.call_id,
                    serde_json::from_value(json!({"summary":"forged cross-service"})).unwrap()
                )
                .await
                .is_err()
            );
            *self.0.lock().unwrap() = Some((host.clone(), ctx.call_id.clone()));
            crate::tools::goal::UpdateGoalTool.execute(args, ctx).await
        })
    }
}
#[test]
fn native_goal_reports_use_actual_evidence_and_replay_without_a_second_goal_effect() {
    let fx = EngineFixture::new();
    let provider = Scripted::echo("no API");
    let e = engine(&fx, &provider);
    let shared = e.handle.shared().clone();
    shared.handle.block_on(async {
        let mut f=Fixture::new_in("goals",Some((shared.clone(),owner(&shared)))).await;seed(&shared,false);
        let captured:Captured=Arc::default();let index=f.launch.tools.iter().position(|t|t.spec().name=="update_goal").unwrap();f.launch.tools[index]=Arc::new(GuardUpdate(captured.clone()));
        let update=call(4,"update_goal",json!({"status":"continue","note":"native report"}));
        let complete=call(6,"propose_goal_completion",json!({"summary":"actual file evidence","criteria":[{"id":1,"evidence":[{"kind":"file","reference":"proof.txt","sha256":crate::web::sha256_hex(b"native proof")}]}]}));
        let requests=vec![
            (call(2,"write",json!({"path":"proof.txt","content":"native proof"})),false,""),
            (call(3,"get_goal",json!({})),false,"native reporting goal"),
            (update.clone(),false,"native report"),(update,false,"native report"),
            (call(5,"propose_goal_completion",json!({"summary":"unsupported assertion","evidence":[{"kind":"observation","reference":"claimed done"}]})),true,"not accepted"),
            (complete.clone(),false,"Completion accepted"),(complete,false,"Completion accepted")];
        let peer=tokio::spawn(peer(f.peer,f.dir.path().into(),requests,false));assert_eq!(shared.spawn_cli_agent_root(f.launch).await.unwrap().unwrap().state,cli_agent_turns::State::Completed);
        let responses=peer.await.unwrap();assert_eq!(responses[2],responses[3]);assert_eq!(responses[5],responses[6]);
        let goal=shared.goal_view("s").unwrap().unwrap();assert_eq!(goal.state,GoalState::Completed);assert_eq!(goal.criteria.len(),1);assert!(goal.criteria[0].evidence.iter().any(|e|e.kind=="file" && e.accepted==Some(true)));
        let (expired,id)=captured.lock().unwrap().take().unwrap();assert!(expired.goal_update(&id,serde_json::from_value(json!({"note":"late"})).unwrap()).await.is_err());assert_eq!(shared.goal_view("s").unwrap().unwrap(),goal);
        let mut changes=0;let mut revision=0;while let Ok(ServerFrame::Event {event,..})=e.conn.try_recv(){if let Event::Session {revision:next,change,..}=event {assert!(next>=revision);revision=next;if matches!(change,SessionChange::GoalChanged {..}){changes+=1;}}}assert_eq!(changes,3);
        assert!(shared.guard_execution_call(&Call::SetGoal {session:"s".into(),objective:"must not start API".into()}).is_err());
        assert!(shared.store.tasks("s",10).unwrap().0.is_empty());
    });
    assert!(provider.requests().is_empty());
}
#[test]
fn native_goal_audit_requires_mechanical_evidence_before_launch() {
    let fx = EngineFixture::new();
    let provider = Scripted::echo("no API auditor");
    let e = engine(&fx, &provider);
    let shared = e.handle.shared().clone();
    shared.handle.block_on(async {
        let f=Fixture::new_in("goals",Some((shared.clone(),owner(&shared)))).await;seed(&shared,true);
        let requests=vec![(call(2,"propose_goal_completion",json!({"summary":"would audit","evidence":[{"kind":"observation","reference":"test"}]})),true,"not accepted")];
        let peer=tokio::spawn(peer(f.peer,f.dir.path().into(),requests,false));assert_eq!(shared.spawn_cli_agent_root(f.launch).await.unwrap().unwrap().state,cli_agent_turns::State::Completed);peer.await.unwrap();
        assert_eq!(shared.goal_view("s").unwrap().unwrap().state,GoalState::Active);assert!(shared.store.tasks("s",10).unwrap().0.is_empty());
    });
    assert!(provider.requests().is_empty());
}
