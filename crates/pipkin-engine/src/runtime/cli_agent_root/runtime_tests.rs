//! Real engine lifetime, fake SDK/MCP peer, actual native Write; no installed inference.
use super::*;
use crate::provider::scripted::Scripted;
use crate::runtime::tests::{Engine, Fixture as EngineFixture};
use pipkin_engine_protocol::{
    Block, Delta, EntryKind, Event, Message, Reply, ServerFrame, SessionChange,
};
fn engine(fx: &EngineFixture, provider: &Scripted) -> Engine {
    let mut config = fx.config();
    config.enable_local_claude_cli = true;
    config.enable_local_claude_cli_orchestration = true;
    fx.start_config(provider, config)
}
fn owner(shared: &crate::runtime::Shared) -> u64 {
    *shared.clients.lock().unwrap().keys().next().unwrap()
}
async fn acknowledged(path: &std::path::Path) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !path.join("acknowledged").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
async fn idle(shared: &crate::runtime::Shared) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !shared.external_tasks.is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
#[test]
fn live_text_and_native_receipts_arrive_in_commit_order_without_snapshot_churn() {
    let fx = EngineFixture::new();
    let provider = Scripted::echo("must not infer");
    let e = engine(&fx, &provider);
    let shared = e.handle.shared().clone();
    shared.handle.block_on(async {
        let f=Fixture::new_in("stream",Some((shared.clone(),owner(&shared)))).await;
        std::fs::write(f.dir.path().join("wait-first-delta"),"wait").unwrap();
        let peer=tokio::spawn(client(f.peer,f.dir.path().to_owned(),true));
        let result=shared.spawn_cli_agent_root(f.launch);
        let mut entries=std::collections::BTreeMap::<u64,pipkin_engine_protocol::Entry>::new();
        let mut order=Vec::new();let mut revision=0;let mut settled=false;
        tokio::time::timeout(Duration::from_secs(15),async {
            while !settled {
                while let Ok(frame)=e.conn.try_recv() {
                    let ServerFrame::Event {event,..}=frame else {continue;};
                    assert!(!matches!(event,Event::ResyncRequired),"successful streaming must not request repeated snapshots");
                    let Event::Session {session,revision:next,change}=event else {continue;};
                    if session!="s" {continue;}
                    assert!(next>=revision,"out-of-order committed revision");revision=next;
                    match change {
                        SessionChange::EntryAppended {entry}|SessionChange::EntryReplaced {entry}=> {
                            if let EntryKind::Message {message,..}=&entry.kind {
                                match message {
                                    Message::Assistant {api,content,..} if api=="claude-cli-tools"=> {
                                        assert!(content.iter().all(|b|!matches!(b,Block::ToolCall {id,..} if id=="sdk-tool-id")));
                                        order.push("call");
                                    },
                                    Message::ToolResult {..}=>order.push("result"),
                                    _=>{},
                                }
                            }
                            if matches!(&entry.kind,EntryKind::Notice {text,..} if text=="interleaved native commit") {order.push("native commit");}
                            entries.insert(entry.id,entry);
                        },
                        SessionChange::EntryDelta {id,delta:Delta::Text {block,text}}=> {
                            if text=="do" {
                                assert!(!f.dir.path().join("effect").exists());
                                // Exercise an ordinary native writer between the SDK delta
                                // and authenticated tool intent, using the SAME publication gate.
                                let sess=shared.session("s").unwrap();
                                shared.commit(&sess,|tx| {
                                    let entry=Store::append_entry(tx,"s",crate::util::now_ms(),sess.active_run(),
                                        &EntryKind::Notice {level:pipkin_engine_protocol::NoticeLevel::Info,text:"interleaved native commit".into()})?;
                                    Ok(((),vec![SessionChange::EntryAppended {entry}]))
                                }).unwrap();
                                // The peer may admit Write only AFTER the live text is seen.
                                std::fs::write(f.dir.path().join("delta-observed"),"seen").unwrap();
                                order.push("first delta");
                            } else if text=="ne" {order.push("second delta");}
                            let entry=entries.get_mut(&id).expect("append precedes delta");
                            let EntryKind::Message {message:Message::Assistant {content,..},..}=&mut entry.kind else {panic!("assistant")};
                            let Block::Text {text:current,..}=&mut content[block as usize] else {panic!("text")};
                            current.push_str(&text);
                        },
                        SessionChange::RunSettled {..}=>{settled=true;order.push("settled");},
                        _=>{},
                    }
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }).await.unwrap();
        assert!(result.await.unwrap().is_ok());peer.await.unwrap();idle(&shared).await;
        assert_eq!(order,vec!["first delta","native commit","call","result","second delta","settled"]);
        assert_eq!(entries.len(),5,"one user, one display cell, native call/result and interleaved notice");
        for (id,entry) in entries {assert_eq!(Some(entry),shared.store.entry("s",id).unwrap());}
        assert!(provider.requests().is_empty());
    });
}
#[test]
fn dropped_waiter_does_not_drop_owned_root_and_settlement_follows_cleanup() {
    let fx = EngineFixture::new();
    let provider = Scripted::echo("must not infer");
    let e = engine(&fx, &provider);
    let shared = e.handle.shared().clone();
    shared.handle.block_on(async {
        let f = Fixture::new_in("success", Some((shared.clone(), owner(&shared)))).await;
        let sess = shared.session("s").unwrap();
        let mut settled = sess.settled.subscribe();
        let peer = tokio::spawn(client(f.peer, f.dir.path().to_owned(), true));
        let receipt = shared.spawn_cli_agent_root(f.launch);
        drop(receipt);
        let outcome = tokio::time::timeout(Duration::from_secs(15), settled.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.request.as_deref(), Some("r"));
        peer.await.unwrap();
        idle(&shared).await;
        assert_eq!(
            shared
                .store
                .cli_agent_turn("s", "r")
                .unwrap()
                .unwrap()
                .state,
            cli_agent_turns::State::Completed
        );
        assert!(sess.active_run().is_none());
        assert!(sess.failure().is_none());
        assert!(!shared.cli_agents.has_turn("s"));
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("effect")).unwrap(),
            "once"
        );
    });
    assert!(provider.requests().is_empty());
}
#[test]
fn backend_stop_signals_owned_root_and_preserves_known_effect_and_unknown_receipt() {
    let fx = EngineFixture::new();
    let provider = Scripted::echo("must not infer");
    let e = engine(&fx, &provider);
    let shared = e.handle.shared().clone();
    shared.handle.block_on(async {
        let f = Fixture::new_in("stop", Some((shared.clone(), owner(&shared)))).await;
        let run = f.launch.root.receipt().scope.run;
        let peer = tokio::spawn(client(f.peer, f.dir.path().to_owned(), false));
        let result = shared.spawn_cli_agent_root(f.launch);
        acknowledged(f.dir.path()).await;
        assert_eq!(shared.session("s").unwrap().active_run(), Some(run));
        assert!(!shared.external_tasks.is_empty());
        assert!(matches!(
            shared.stop_by_backend("s").await.unwrap(),
            Reply::Stopping { .. }
        ));
        assert!(
            tokio::time::timeout(Duration::from_secs(10), result)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        peer.await.unwrap();
        idle(&shared).await;
        assert_eq!(
            shared
                .store
                .cli_agent_turn("s", "r")
                .unwrap()
                .unwrap()
                .state,
            cli_agent_turns::State::Unknown
        );
        assert!(shared.session("s").unwrap().active_run().is_none());
        assert!(shared.session("s").unwrap().failure().is_none());
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("launches")).unwrap(),
            "spawn\n"
        );
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("effect")).unwrap(),
            "once"
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
#[test]
fn capacity_refusal_retires_admission_without_spawning_or_leaking_a_slot() {
    let fx = EngineFixture::new();
    let provider = Scripted::echo("must not infer");
    let e = engine(&fx, &provider);
    let shared = e.handle.shared().clone();
    shared.handle.block_on(async {
        let f = Fixture::new_in("success", Some((shared.clone(), owner(&shared)))).await;
        let permits = shared
            .generations
            .clone()
            .try_acquire_many_owned(shared.generations.available_permits() as u32)
            .unwrap();
        let result = shared.spawn_cli_agent_root(f.launch).await.unwrap();
        assert!(result.is_err());
        idle(&shared).await;
        assert!(!f.dir.path().join("launches").exists());
        assert_eq!(
            shared
                .store
                .cli_agent_turn("s", "r")
                .unwrap()
                .unwrap()
                .state,
            cli_agent_turns::State::Rejected
        );
        assert!(!shared.cli_agents.has_turn("s"));
        assert!(shared.session("s").unwrap().active_run().is_none());
        drop(permits);
    });
    assert!(provider.requests().is_empty());
}
#[test]
fn foreign_registry_cannot_attach_or_mutate_this_engine() {
    let fx = EngineFixture::new();
    let provider = Scripted::echo("must not infer");
    let e = engine(&fx, &provider);
    let shared = e.handle.shared().clone();
    shared.handle.block_on(async {
        let f = Fixture::new("success").await;
        assert_eq!(
            shared.spawn_cli_agent_root(f.launch).await.unwrap(),
            Err(Error::Preparation)
        );
        idle(&shared).await;
        assert!(shared.store.session("s").unwrap().is_none());
        assert_eq!(
            f.store.cli_agent_turn("s", "r").unwrap().unwrap().state,
            cli_agent_turns::State::Admitted
        );
        assert!(!f.dir.path().join("launches").exists());
    });
}
#[test]
fn engine_shutdown_waits_for_owned_root_retirement() {
    let fx = EngineFixture::new();
    let provider = Scripted::echo("must not infer");
    let e = engine(&fx, &provider);
    let shared = e.handle.shared().clone();
    let (dir, mut result, peer) = shared.handle.block_on(async {
        let f = Fixture::new_in("stop", Some((shared.clone(), owner(&shared)))).await;
        let peer = tokio::spawn(client(f.peer, f.dir.path().to_owned(), false));
        let result = shared.spawn_cli_agent_root(f.launch);
        acknowledged(f.dir.path()).await;
        (f.dir, result, peer)
    });
    e.handle.shutdown(Duration::from_secs(8));
    assert!(shared.external_tasks.is_empty());
    assert!(peer.is_finished());
    assert!(result.try_recv().unwrap().is_err());
    assert_eq!(
        shared
            .store
            .cli_agent_turn("s", "r")
            .unwrap()
            .unwrap()
            .state,
        cli_agent_turns::State::Unknown
    );
    assert!(shared.session("s").unwrap().active_run().is_none());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("effect")).unwrap(),
        "once"
    );
    assert!(provider.requests().is_empty());
}
#[test]
fn cancelled_handoff_does_not_wait_for_the_session_admission_lock() {
    let fx = EngineFixture::new();
    let provider = Scripted::echo("must not infer");
    let e = engine(&fx, &provider);
    let shared = e.handle.shared().clone();
    shared.handle.block_on(async {
        let f = Fixture::new_in("success", Some((shared.clone(), owner(&shared)))).await;
        let sess = shared.session("s").unwrap();
        let hold = sess.lock.lock().await;
        let cancel = f.launch.cancel.clone();
        let result = shared.spawn_cli_agent_root(f.launch);
        tokio::task::yield_now().await;
        cancel.cancel();
        assert!(
            tokio::time::timeout(Duration::from_secs(5), result)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        idle(&shared).await;
        assert!(!f.dir.path().join("launches").exists());
        assert_eq!(
            shared
                .store
                .cli_agent_turn("s", "r")
                .unwrap()
                .unwrap()
                .state,
            cli_agent_turns::State::Rejected
        );
        assert!(sess.active_run().is_none());
        drop(hold);
    });
}
#[test]
fn actual_connection_drop_cancels_tracked_root_without_waiter_ownership() {
    let fx = EngineFixture::new();
    let provider = Scripted::echo("must not infer");
    let e = engine(&fx, &provider);
    let shared = e.handle.shared().clone();
    shared.handle.block_on(async {
        let f = Fixture::new_in("stop", Some((shared.clone(), owner(&shared)))).await;
        let peer = tokio::spawn(client(f.peer, f.dir.path().to_owned(), false));
        drop(shared.spawn_cli_agent_root(f.launch));
        acknowledged(f.dir.path()).await;
        drop(e.conn);
        peer.await.unwrap();
        idle(&shared).await;
        assert_eq!(
            shared
                .store
                .cli_agent_turn("s", "r")
                .unwrap()
                .unwrap()
                .state,
            cli_agent_turns::State::Unknown
        );
        assert!(shared.session("s").unwrap().active_run().is_none());
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("effect")).unwrap(),
            "once"
        );
    });
    assert!(provider.requests().is_empty());
}
#[test]
fn removal_waits_for_private_actor_and_keeps_permanent_receipt() {
    let fx = EngineFixture::new();
    let provider = Scripted::echo("must not infer");
    let e = engine(&fx, &provider);
    let shared = e.handle.shared().clone();
    shared.handle.block_on(async {
        let f = Fixture::new_in("stop", Some((shared.clone(), owner(&shared)))).await;
        let peer = tokio::spawn(client(f.peer, f.dir.path().to_owned(), false));
        let result = shared.spawn_cli_agent_root(f.launch);
        acknowledged(f.dir.path()).await;
        assert_eq!(shared.remove_session("s").await.unwrap(), Reply::Ok);
        assert!(result.await.unwrap().is_err());
        peer.await.unwrap();
        idle(&shared).await;
        assert!(!shared.cli_agents.has_turn("s"));
        assert!(shared.store.session("s").unwrap().is_none());
        assert_eq!(
            shared
                .store
                .cli_agent_turn("s", "r")
                .unwrap()
                .unwrap()
                .state,
            cli_agent_turns::State::Unknown
        );
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("effect")).unwrap(),
            "once"
        );
    });
}
