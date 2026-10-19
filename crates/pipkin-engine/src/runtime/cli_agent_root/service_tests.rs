//! Real native service effects through fake CLI/SDK peers. Only owned local fixture servers.
use super::*;
use crate::{
    provider::{BoxFuture, ToolSpec},
    tools::{Tool, ToolCtx, ToolError, ToolHost, ToolOutput},
};
use crate::{
    provider::{
        mock::{MockResponse, MockServer},
        scripted::Scripted,
    },
    runtime::{
        mcp_tests::{fixture_bin, write_settings},
        tests::{Engine, Fixture as EngineFixture},
    },
};
use pipkin_engine_protocol::{Answer, Call, Reply};
type Progress = Arc<dyn Fn(&str) + Send + Sync>;
type Captured = Arc<std::sync::Mutex<Option<(Arc<dyn ToolHost>, String, Progress)>>>;
struct GuardMcp(Captured);
impl Tool for GuardMcp {
    fn spec(&self) -> ToolSpec {
        crate::tools::mcp::McpCallTool.spec()
    }
    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolCtx,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let host = ctx.host.as_ref().unwrap();
            assert!(
                host.mcp_call(
                    "printed-sdk-id",
                    "fx/counter_increment",
                    args.clone(),
                    ctx.cancel.clone()
                )
                .await
                .is_err()
            );
            assert!(host.plan_read(false).await.is_err());
            assert!(
                host.web_fetch(
                    &ctx.call_id,
                    "http://127.0.0.1:1/forbidden",
                    ctx.cancel.clone()
                )
                .await
                .is_err()
            );
            (ctx.progress)(&"native MCP progress λ ".repeat(3000));
            *self.0.lock().unwrap() =
                Some((host.clone(), ctx.call_id.clone(), ctx.progress.clone()));
            crate::tools::mcp::McpCallTool.execute(args, ctx).await
        })
    }
}
fn engine(fx: &EngineFixture, p: &Scripted) -> Engine {
    let mut config = fx.config();
    config.enable_local_claude_cli = true;
    config.enable_local_claude_cli_orchestration = true;
    config.web_allow_private_hosts = true;
    fx.start_config(p, config)
}
fn owner(shared: &crate::runtime::Shared) -> u64 {
    *shared.clients.lock().unwrap().keys().next().unwrap()
}
pub(super) fn call(id: u64, name: &str, args: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":args}})
}
pub(super) async fn peer(
    launch: cli_bridge_channel::Launch,
    path: std::path::PathBuf,
    calls: Vec<(Value, bool, &'static str)>,
    stop: bool,
) -> Vec<Value> {
    peer_marked(launch, path, calls, stop, None).await
}
pub(super) async fn peer_marked(
    launch: cli_bridge_channel::Launch,
    path: std::path::PathBuf,
    calls: Vec<(Value, bool, &'static str)>,
    stop: bool,
    marker: Option<String>,
) -> Vec<Value> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let (mut read, mut write) = launch.connect(deadline).await.unwrap().split();
    for request in [
        json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":PROTOCOL_VERSION,"clientInfo":{"name":"fixture","version":"1"},"capabilities":{}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
    ] {
        write
            .write(&serde_json::to_vec(&request).unwrap(), deadline)
            .await
            .unwrap();
        if request.get("id").is_some() {
            read.read(deadline).await.unwrap();
        }
    }
    let mut responses = Vec::new();
    for (request, error, text) in calls {
        write
            .write(&serde_json::to_vec(&request).unwrap(), deadline)
            .await
            .unwrap();
        let response = read.read(deadline).await;
        if stop {
            assert!(response.is_err());
            return responses;
        }
        let response: Value = serde_json::from_slice(&response.unwrap()).unwrap();
        assert_eq!(response["result"]["isError"], error, "{response}");
        assert!(response.to_string().contains(text), "{response}");
        responses.push(response);
    }
    std::fs::write(
        path.join(
            marker
                .as_ref()
                .map_or("acknowledged".into(), |m| format!("acknowledged-{m}")),
        ),
        "ack",
    )
    .unwrap();
    let control: Value = serde_json::from_slice(&read.read(deadline).await.unwrap()).unwrap();
    assert!(Control::Quiesce.matches(&control));
    write
        .write(
            &serde_json::to_vec(&Control::Quiesced.value()).unwrap(),
            deadline,
        )
        .await
        .unwrap();
    let control: Value = serde_json::from_slice(&read.read(deadline).await.unwrap()).unwrap();
    assert!(Control::Closed.matches(&control));
    std::fs::write(
        path.join(
            marker
                .as_ref()
                .map_or("closed".into(), |m| format!("closed-{m}")),
        ),
        "closed",
    )
    .unwrap();
    responses
}
fn configure(f: &Fixture, binary: &str, extra: Value) {
    std::fs::create_dir_all(f.dir.path().join(".pi")).unwrap();
    let mut settings = json!({"mcp":{"servers":{"fx":{"command":binary}}}});
    settings
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    std::fs::write(f.dir.path().join(".pi/settings.json"), settings.to_string()).unwrap();
}
#[test]
fn native_mcp_web_and_child_directory_services_use_real_scoped_results_without_api() {
    let binary = fixture_bin();
    let fx = EngineFixture::new();
    // A stale global config must not substitute for the CLI project's configured server.
    write_settings(
        &fx,
        json!({"mcp":{"servers":{"decoy":{"command":"/does/not/exist","enabled":false}}}}),
    );
    let provider = Scripted::echo("no API");
    let e = engine(&fx, &provider);
    let shared = e.handle.shared().clone();
    shared.handle.block_on(async {
        let mock=MockServer::start(|req,_| {
            let search=req.path.starts_with("/search");
            MockResponse {status:200,headers:vec![("content-type".into(),if search {"application/json"} else {"text/html"}.into())],chunks:vec![if search {br#"{"results":[{"url":"https://example.org/owned","title":"owned search","content":"source excerpt"}]}"#.to_vec()} else {b"<html><title>owned page</title><body>native retained text</body></html>".to_vec()}],pause:Duration::ZERO,hang:false}
        }).await;
        let mut f=Fixture::new_in("services",Some((shared.clone(),owner(&shared)))).await;
        f.launch.services.progress = Arc::new(|_,_,_| panic!("caller progress must be replaced by owned per-call progress"));
        let captured: Captured = Arc::default();
        for tool in &mut f.launch.tools {
            if tool.spec().name == "mcp_call" {*tool = Arc::new(GuardMcp(captured.clone()));}
        }
        configure(&f,&binary,json!({"web":{"search":{"provider":"searxng","url":mock.url}}}));
        let mutation=call(4,"mcp_call",json!({"tool":"fx/counter_increment","arguments":{"path":f.dir.path().join("counter"),"hang_after":false}}));
        let fetch=call(10,"web_fetch",json!({"url":format!("{}/page",mock.url)}));
        let requests=vec![
            (call(2,"mcp_search",json!({"query":"echo","server":"fx"})),false,"fx/echo"),
            (call(3,"mcp_describe",json!({"tool":"fx/echo"})),false,"Echo"),
            (mutation.clone(),false,"1"),(mutation,false,"1"),
            (call(5,"mcp_resources",json!({"server":"fx"})),false,"fixture://readme"),
            (call(6,"mcp_read",json!({"server":"fx","uri":"fixture://readme"})),false,"Line one"),
            (call(7,"mcp_prompts",json!({"server":"fx"})),false,"greet"),
            (call(8,"mcp_prompt",json!({"prompt":"fx/greet"})),false,"Hello there"),
            (call(9,"web_search",json!({"query":"owned"})),false,"owned search"),
            (fetch.clone(),false,"native retained text"),(fetch,false,"native retained text"),
            (call(11,"web_read",json!({"source":"s2"})),false,"native retained text"),
            (call(12,"subagent_status",json!({})),false,""),
            (call(13,"subagent_result",json!({"child":1})),true,"Not a subagent"),
            (call(14,"subagent_wait",json!({"child":1,"timeout":1})),true,"Not a subagent")];
        let peer=tokio::spawn(peer(f.peer,f.dir.path().into(),requests,false));let result=shared.spawn_cli_agent_root(f.launch);
        assert_eq!(result.await.unwrap().unwrap().state,cli_agent_turns::State::Completed);let responses=peer.await.unwrap();
        assert_eq!(responses[2],responses[3]);assert_eq!(responses[9],responses[10]);
        assert_eq!(std::fs::read_to_string(f.dir.path().join("counter")).unwrap(),"1");
        let (expired, call, progress) = captured.lock().unwrap().take().unwrap();
        progress("late progress must not be published");
        assert!(expired.mcp_call(&call, "fx/counter_increment", json!({"path":f.dir.path().join("counter"),"hang_after":false}), CancellationToken::new()).await.is_err());
        assert_eq!(std::fs::read_to_string(f.dir.path().join("counter")).unwrap(),"1");
        assert_eq!(mock.requests().len(),2);
        let mut progress_count = 0;
        let mut revision = 0;
        while let Ok(frame) = e.conn.try_recv() {
            if let pipkin_engine_protocol::ServerFrame::Event {event:pipkin_engine_protocol::Event::Session {revision:next, change, ..}, ..} = frame {
                if let pipkin_engine_protocol::SessionChange::ToolProgress {call_id, text} = change {
                    assert_eq!(next,0);assert_eq!(call_id,call);assert!(text.starts_with("native MCP progress"));assert!(text.len()<=32768);progress_count+=1;
                } else {assert!(next>=revision);revision=next;}
            }
        }
        assert_eq!(progress_count,1);
        assert_eq!(shared.store.read(|c|Store::web_sources(c,"s")).unwrap().len(),2);
        assert!(shared.mcp.snapshots().iter().all(|s|s.in_flight==0));
        // Explicit desktop configuration is now supported for root CLI conversations;
        // it still grants no native API execution authority.
        assert!(shared.guard_execution_call(&Call::McpServers {session:Some("s".into())}).is_ok());
        assert!(shared.require_native_execution("s").is_err());
    });
    assert!(provider.requests().is_empty());
}
#[test]
fn native_mcp_forms_use_exact_question_grants_and_stop_revokes_them() {
    let binary = fixture_bin();
    for stop in [false, true] {
        let fx = EngineFixture::new();
        let provider = Scripted::echo("no API");
        let mut e = engine(&fx, &provider);
        let shared = e.handle.shared().clone();
        shared.handle.block_on(async {
            let f = Fixture::new_in("services", Some((shared.clone(), owner(&shared)))).await;
            configure(&f, &binary, json!({}));
            let request = call(
                2,
                "mcp_call",
                json!({"tool":"fx/ask_person","arguments":{"mode":"empty"}}),
            );
            let peer = tokio::spawn(peer(
                f.peer,
                f.dir.path().into(),
                vec![
                    (request.clone(), false, "accept"),
                    (request, false, "accept"),
                ],
                stop,
            ));
            let result = shared.spawn_cli_agent_root(f.launch);
            let q = tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if let Some(q) = shared
                        .session("s")
                        .unwrap()
                        .questions
                        .view()
                        .questions
                        .first()
                        .cloned()
                    {
                        break q;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            assert!(shared.cli_question_allowed("s", &q.id));
            assert!(!shared.cli_question_allowed("foreign", &q.id));
            if stop {
                shared.stop_by_backend("s").await.unwrap();
                assert!(result.await.unwrap().is_err());
            } else {
                assert_eq!(
                    tokio::task::block_in_place(|| e.call(Call::AnswerQuestion {
                        session: "s".into(),
                        question: q.id.clone(),
                        answer: Answer::Confirm { value: true }
                    }))
                    .unwrap(),
                    Reply::Ok
                );
                assert_eq!(
                    result.await.unwrap().unwrap().state,
                    cli_agent_turns::State::Completed
                );
            }
            let responses = peer.await.unwrap();
            if !stop {
                assert_eq!(responses[0], responses[1]);
            }
            assert!(!shared.cli_question_allowed("s", &q.id));
            assert!(
                shared
                    .session("s")
                    .unwrap()
                    .questions
                    .view()
                    .questions
                    .is_empty()
            );
            assert!(shared.mcp.snapshots().iter().all(|s| s.in_flight == 0));
        });
        assert!(provider.requests().is_empty());
    }
}
#[test]
fn stop_after_mcp_effect_does_not_replay_and_releases_in_flight_registration() {
    let binary = fixture_bin();
    let fx = EngineFixture::new();
    let provider = Scripted::echo("no API");
    let e = engine(&fx, &provider);
    let shared = e.handle.shared().clone();
    shared.handle.block_on(async {
        let f = Fixture::new_in("services", Some((shared.clone(), owner(&shared)))).await;
        configure(&f, &binary, json!({}));
        let path = f.dir.path().join("counter");
        let request = call(
            2,
            "mcp_call",
            json!({"tool":"fx/counter_increment","arguments":{"path":path,"hang_after":true}}),
        );
        let peer = tokio::spawn(peer(
            f.peer,
            f.dir.path().into(),
            vec![(request, false, "")],
            true,
        ));
        let result = shared.spawn_cli_agent_root(f.launch);
        tokio::time::timeout(Duration::from_secs(10), async {
            while !path.exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        shared.stop_by_backend("s").await.unwrap();
        assert!(result.await.unwrap().is_err());
        peer.await.unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "1");
        assert!(shared.mcp.snapshots().iter().all(|s| s.in_flight == 0));
        assert_eq!(
            shared
                .store
                .cli_agent_turn("s", "r")
                .unwrap()
                .unwrap()
                .state,
            cli_agent_turns::State::Unknown
        );
    });
    assert!(provider.requests().is_empty());
}
