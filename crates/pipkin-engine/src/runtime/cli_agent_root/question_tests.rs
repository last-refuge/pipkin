//! Existing question presentation/RPC with fake CLI/MCP and real scoped native question tool.
use super::*;
use crate::{
    provider::scripted::Scripted,
    runtime::tests::{Engine, Fixture as EngineFixture},
};
use pipkin_engine_protocol::{Answer, Call, Event, QuestionView, Reply, SessionChange};
fn engine(fx: &EngineFixture, p: &Scripted) -> Engine {
    let mut config = fx.config();
    config.enable_local_claude_cli = true;
    config.enable_local_claude_cli_orchestration = true;
    fx.start_config(p, config)
}
fn owner(shared: &crate::runtime::Shared) -> u64 {
    *shared.clients.lock().unwrap().keys().next().unwrap()
}
async fn open(shared: &crate::runtime::Shared) -> QuestionView {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(q) = shared
                .session("s")
                .unwrap()
                .questions
                .view()
                .questions
                .into_iter()
                .next()
            {
                return q;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}
async fn peer(peer: cli_bridge_channel::Launch, path: std::path::PathBuf, answered: bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    let (mut read, mut write) = peer.connect(deadline).await.unwrap().split();
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
    let request = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"ask_user_question","arguments":{"question":"Choose test response","options":[{"label":"one"},{"label":"two"}]}}});
    let bytes = serde_json::to_vec(&request).unwrap();
    write.write(&bytes, deadline).await.unwrap();
    if !answered {
        assert!(read.read(deadline).await.is_err());
        return;
    }
    let original: Value = serde_json::from_slice(&read.read(deadline).await.unwrap()).unwrap();
    assert_eq!(original["result"]["isError"], false, "{original}");
    assert!(
        original.to_string().contains("two") || original.to_string().contains("did not answer")
    );
    write.write(&bytes, deadline).await.unwrap();
    let duplicate: Value = serde_json::from_slice(&read.read(deadline).await.unwrap()).unwrap();
    assert_eq!(original, duplicate);
    std::fs::write(path.join("acknowledged"), "ack").unwrap();
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
    std::fs::write(path.join("closed"), "closed").unwrap();
}
#[test]
fn exact_live_question_is_answered_or_declined_by_existing_rpc_without_enabling_api_execution() {
    for decline in [false, true] {
        let fx = EngineFixture::new();
        let provider = Scripted::echo("no fallback");
        let mut e = engine(&fx, &provider);
        let shared = e.handle.shared().clone();
        shared.handle.block_on(async {
        let f=Fixture::new_in("question",Some((shared.clone(),owner(&shared)))).await;
        let peer=tokio::spawn(peer(f.peer,f.dir.path().into(),true));
        let result=shared.spawn_cli_agent_root(f.launch);
        let q=open(&shared).await;
        assert!(shared.cli_question_allowed("s",&q.id));assert!(!shared.cli_question_allowed("foreign",&q.id));
        let sess=shared.session("s").unwrap();
        // Even a question copying the visible source does not get an exact-id grant.
        let forged=sess.questions.ask(crate::extensions::questions::Question::input("forged").with_source(q.source.clone().unwrap()),Some(Duration::ZERO),CancellationToken::new());
        let forged_id=sess.questions.view().questions.into_iter().find(|x|x.id!=q.id).unwrap().id;
        let rejected=tokio::task::block_in_place(||e.call(Call::AnswerQuestion {session:"s".into(),question:forged_id,answer:Answer::Text {value:"no".into()}}));
        assert!(rejected.is_err());drop(forged);
        assert!(shared.guard_execution_call(&Call::UpdatePlan {session:"s".into(),request:"forbidden".into(),ops:Vec::new(),expected_revision:None}).is_err());
        let response=if decline {Call::CancelQuestion {session:"s".into(),question:q.id.clone()}} else {Call::AnswerQuestion {session:"s".into(),question:q.id.clone(),answer:Answer::Choice {value:"o2".into()}}};
        assert_eq!(tokio::task::block_in_place(||e.call(response)).unwrap(),Reply::Ok);
        assert_eq!(result.await.unwrap().unwrap().state,cli_agent_turns::State::Completed);peer.await.unwrap();
        assert!(sess.questions.view().questions.is_empty());assert!(!shared.cli_question_allowed("s",&q.id));
        assert!(tokio::task::block_in_place(||e.call(Call::AnswerQuestion {session:"s".into(),question:q.id.clone(),answer:Answer::Choice {value:"o1".into()}})).is_err());
        assert!(e.events.iter().any(|event|matches!(event,Event::Session {change:SessionChange::UiChanged {ui},..} if ui.questions.iter().any(|x|x.id==q.id))));
        let count=shared.store.read(|c|Ok(c.query_row("SELECT COUNT(*) FROM tool_calls WHERE name='ask_user_question' AND state='done'",[],|r|r.get::<_,u64>(0))?)).unwrap();assert_eq!(count,1);
        let expected=if decline {"did not answer"} else {"two"};
        assert!(shared.store.page("s",None,20).unwrap().0.iter().any(|entry|matches!(&entry.kind,pipkin_engine_protocol::EntryKind::Message {message:pipkin_engine_protocol::Message::ToolResult {content,..},..} if content.iter().any(|b| matches!(b,pipkin_engine_protocol::Block::Text {text,..} if text.contains(expected))))));
    });
        assert!(provider.requests().is_empty());
    }
}
#[test]
fn stop_or_removal_withdraws_question_and_never_reasks_unknown_call() {
    for remove in [false, true] {
        let fx = EngineFixture::new();
        let provider = Scripted::echo("no fallback");
        let e = engine(&fx, &provider);
        let shared = e.handle.shared().clone();
        shared.handle.block_on(async {
            let f = Fixture::new_in("question", Some((shared.clone(), owner(&shared)))).await;
            let peer = tokio::spawn(peer(f.peer, f.dir.path().into(), false));
            let result = shared.spawn_cli_agent_root(f.launch);
            let q = open(&shared).await;
            let questions = shared.session("s").unwrap().questions.clone();
            if remove {
                shared.remove_session("s").await.unwrap();
            } else {
                shared.stop_by_backend("s").await.unwrap();
            }
            assert!(result.await.unwrap().is_err());
            peer.await.unwrap();
            assert!(questions.view().questions.is_empty());
            assert!(!shared.cli_question_allowed("s", &q.id));
            assert_eq!(
                shared
                    .store
                    .cli_agent_turn("s", "r")
                    .unwrap()
                    .unwrap()
                    .state,
                cli_agent_turns::State::Unknown
            );
            assert!(
                shared
                    .cli_agents
                    .begin_turn(&shared.store, owner(&shared), "s")
                    .is_err()
            );
        });
        assert!(provider.requests().is_empty());
    }
}
#[test]
fn shutdown_retires_question_instead_of_borrowing_api_reask_recovery() {
    let fx = EngineFixture::new();
    let provider = Scripted::echo("no fallback");
    let e = engine(&fx, &provider);
    let shared = e.handle.shared().clone();
    let (_dir, mut result, peer, q, questions) = shared.handle.block_on(async {
        let f = Fixture::new_in("question", Some((shared.clone(), owner(&shared)))).await;
        let peer = tokio::spawn(peer(f.peer, f.dir.path().into(), false));
        let result = shared.spawn_cli_agent_root(f.launch);
        let q = open(&shared).await;
        let questions = shared.session("s").unwrap().questions.clone();
        (f.dir, result, peer, q, questions)
    });
    e.handle.shutdown(Duration::from_secs(8));
    assert!(shared.external_tasks.is_empty());
    assert!(peer.is_finished());
    assert!(result.try_recv().unwrap().is_err());
    assert!(questions.view().questions.is_empty());
    assert!(!shared.cli_question_allowed("s", &q.id));
    assert_eq!(
        shared
            .store
            .cli_agent_turn("s", "r")
            .unwrap()
            .unwrap()
            .state,
        cli_agent_turns::State::Unknown
    );
    assert!(provider.requests().is_empty());
}
#[test]
fn connection_loss_withdraws_question_without_restart_or_person_refusal() {
    let fx = EngineFixture::new();
    let provider = Scripted::echo("no fallback");
    let e = engine(&fx, &provider);
    let shared = e.handle.shared().clone();
    shared.handle.block_on(async {
        let f = Fixture::new_in("question", Some((shared.clone(), owner(&shared)))).await;
        let peer = tokio::spawn(peer(f.peer, f.dir.path().into(), false));
        let result = shared.spawn_cli_agent_root(f.launch);
        let q = open(&shared).await;
        let questions = shared.session("s").unwrap().questions.clone();
        drop(e.conn);
        assert!(result.await.unwrap().is_err());
        peer.await.unwrap();
        assert!(questions.view().questions.is_empty());
        assert!(!shared.cli_question_allowed("s", &q.id));
        assert_eq!(
            shared
                .store
                .cli_agent_turn("s", "r")
                .unwrap()
                .unwrap()
                .state,
            cli_agent_turns::State::Unknown
        );
        let done = shared
            .store
            .read(|c| {
                Ok(c.query_row(
                    "SELECT COUNT(*) FROM tool_calls WHERE state='done'",
                    [],
                    |r| r.get::<_, u64>(0),
                )?)
            })
            .unwrap();
        assert_eq!(done, 0);
    });
    assert!(provider.requests().is_empty());
}
