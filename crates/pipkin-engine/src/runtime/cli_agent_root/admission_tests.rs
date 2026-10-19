//! Owned admission builder through a real engine; fake CLI/SDK/MCP, actual native Write.
use super::*;
use crate::{
    provider::scripted::Scripted,
    runtime::{
        cli_agent_runtime::admission::Start,
        tests::{Engine, Fixture as EngineFixture},
    },
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
async fn start(shared: Arc<crate::runtime::Shared>, variant: &str) -> (tempfile::TempDir, Start) {
    let f = Fixture::new_in(variant, Some((shared.clone(), owner(&shared)))).await;
    // Reuse the metadata-only fixture setup. Its unused original admission is explicitly
    // rejected before this NEW request; no model/peer was started for that setup receipt.
    f.store
        .abandon_cli_agent_turn(&f.launch.root, crate::util::now_ms())
        .unwrap();
    drop(f.launch);
    let ownership = shared
        .cli_agents
        .begin_turn(&shared.store, owner(&shared), "s")
        .unwrap();
    let start = recipe(&f.dir, ownership);
    (f.dir, start)
}
fn recipe(dir: &tempfile::TempDir, ownership: crate::runtime::cli_agent_owner::Turn) -> Start {
    Start {
        images: vec![],
        ownership,
        request: "owned-r".into(),
        generation: 2,
        operation: "owned-op".into(),
        text: "offline root fixture".into(),
        system: "offline system fixture".into(),
        assets_parent: dir.path().to_owned(),
        image: image(),
        tools: crate::tools::builtin_tools()
            .into_iter()
            .filter(|t| t.spec().name == "write")
            .collect(),
        services: Services::default(),
        deadline: Instant::now() + Duration::from_secs(20),
        cancel: CancellationToken::new(),
        test_peer: None,
        test_preparing: None,
    }
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
fn owned_admission_survives_lost_reply_and_duplicate_is_lookup_only() {
    let fx = EngineFixture::new();
    let provider = Scripted::echo("must not infer");
    let e = engine(&fx, &provider);
    let shared = e.handle.shared().clone();
    shared.handle.block_on(async {
        let (dir, mut start) = start(shared.clone(), "success").await;
        assert!(
            shared
                .store
                .cli_agent_turn("s", "owned-r")
                .unwrap()
                .is_none()
        );
        let (send, receive) = tokio::sync::oneshot::channel();
        start.test_peer = Some(send);
        drop(shared.submit_cli_agent_root(start));
        let peer = tokio::time::timeout(Duration::from_secs(10), receive)
            .await
            .unwrap()
            .unwrap();
        client(peer, dir.path().to_owned(), true).await;
        idle(&shared).await;
        let receipt = shared
            .store
            .cli_agent_turn("s", "owned-r")
            .unwrap()
            .unwrap();
        assert_eq!(receipt.state, cli_agent_turns::State::Completed);
        assert_eq!(receipt.scope.generation, 2);
        assert_eq!(receipt.scope.operation, "owned-op");
        let turn = shared
            .cli_agents
            .begin_turn(&shared.store, owner(&shared), "s")
            .unwrap();
        let mut repeated = recipe(&dir, turn);
        let (send, receive) = tokio::sync::oneshot::channel();
        repeated.test_peer = Some(send);
        let busy = shared
            .generations
            .clone()
            .try_acquire_many_owned(shared.generations.available_permits() as u32)
            .unwrap();
        repeated.cancel.cancel();
        repeated.system.clear();
        let lookup = shared
            .submit_cli_agent_root(repeated)
            .await
            .unwrap()
            .unwrap();
        drop(busy);
        assert_eq!(lookup, receipt);
        assert!(receive.await.is_err());
        idle(&shared).await;
        assert_eq!(
            std::fs::read_to_string(dir.path().join("launches")).unwrap(),
            "spawn\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("effect")).unwrap(),
            "once"
        );
        assert!(shared.session("s").unwrap().active_run().is_none());
    });
    assert!(provider.requests().is_empty());
}
#[test]
fn preparation_failures_retire_owned_admission_without_inference_or_capacity_leaks() {
    for invalid_catalog in [false, true] {
        let fx = EngineFixture::new();
        let provider = Scripted::echo("must not infer");
        let e = engine(&fx, &provider);
        let shared = e.handle.shared().clone();
        shared.handle.block_on(async {
            let (dir,mut start)=start(shared.clone(),"success").await;
            if invalid_catalog {start.tools.push(start.tools[0].clone());}
            else {start.assets_parent=dir.path().join("missing-parent");}
            let capacity=shared.generations.available_permits();
            assert!(shared.submit_cli_agent_root(start).await.unwrap().is_err());idle(&shared).await;
            assert_eq!(shared.store.cli_agent_turn("s","owned-r").unwrap().unwrap().state,cli_agent_turns::State::Rejected);
            assert_eq!(shared.generations.available_permits(),capacity);
            assert!(!shared.cli_agents.has_turn("s"));assert!(!dir.path().join("launches").exists());
            let connected:i64=shared.store.read(|c|Ok(c.query_row("SELECT COUNT(*) FROM cli_tool_bridges WHERE state IN ('prepared','connected')",[],|r|r.get(0))?)).unwrap();
            assert_eq!(connected,0);
        });
        assert!(provider.requests().is_empty());
    }
}
#[test]
fn cancelled_locked_handoff_and_busy_capacity_do_not_admit_input() {
    for lock_wait in [false, true] {
        let fx = EngineFixture::new();
        let provider = Scripted::echo("must not infer");
        let e = engine(&fx, &provider);
        let shared = e.handle.shared().clone();
        shared.handle.block_on(async {
            let (dir, start) = start(shared.clone(), "success").await;
            let sess = shared.session("s").unwrap();
            let lock = if lock_wait {
                Some(sess.lock.lock().await)
            } else {
                None
            };
            let permit = if lock_wait {
                None
            } else {
                Some(
                    shared
                        .generations
                        .clone()
                        .try_acquire_many_owned(shared.generations.available_permits() as u32)
                        .unwrap(),
                )
            };
            let cancel = start.cancel.clone();
            let result = shared.submit_cli_agent_root(start);
            if lock_wait {
                cancel.cancel();
            }
            assert!(
                tokio::time::timeout(Duration::from_secs(5), result)
                    .await
                    .unwrap()
                    .unwrap()
                    .is_err()
            );
            idle(&shared).await;
            assert!(
                shared
                    .store
                    .cli_agent_turn("s", "owned-r")
                    .unwrap()
                    .is_none()
            );
            assert!(!shared.cli_agents.has_turn("s"));
            assert!(!dir.path().join("launches").exists());
            drop(lock);
            drop(permit);
        });
    }
}
#[test]
fn unknown_owned_turn_is_lookup_only_and_never_readmitted() {
    let fx = EngineFixture::new();
    let provider = Scripted::echo("must not infer");
    let e = engine(&fx, &provider);
    let shared = e.handle.shared().clone();
    shared.handle.block_on(async {
        let (dir, mut start) = start(shared.clone(), "stop").await;
        let binding = start.ownership.binding().digest;
        let (send, receive) = tokio::sync::oneshot::channel();
        start.test_peer = Some(send);
        let result = shared.submit_cli_agent_root(start);
        let peer = tokio::time::timeout(Duration::from_secs(10), receive)
            .await
            .unwrap()
            .unwrap();
        let peer = tokio::spawn(client(peer, dir.path().to_owned(), false));
        tokio::time::timeout(Duration::from_secs(10), async {
            while !dir.path().join("acknowledged").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        shared.stop_by_backend("s").await.unwrap();
        assert!(result.await.unwrap().is_err());
        peer.await.unwrap();
        idle(&shared).await;
        let input = cli_agent_turns::Input {
            images: &[],
            mode: pipkin_engine_protocol::InputMode::Prompt,
            session: "s",
            request: "owned-r",
            generation: 2,
            operation: "owned-op",
            text: "offline root fixture",
            binding,
        };
        let receipt = shared.store.lookup_cli_agent_turn(&input).unwrap().unwrap();
        assert_eq!(receipt.state, cli_agent_turns::State::Unknown);
        assert_eq!(
            shared.store.lookup_cli_agent_turn(&input).unwrap(),
            Some(receipt)
        );
        assert!(
            shared
                .cli_agents
                .begin_turn(&shared.store, owner(&shared), "s")
                .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("launches")).unwrap(),
            "spawn\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("effect")).unwrap(),
            "once"
        );
    });
    assert!(provider.requests().is_empty());
}
#[test]
fn preparing_root_publishes_in_order_and_stop_targets_original_run() {
    use pipkin_engine_protocol::{EntryKind, Event, Reply, ServerFrame, SessionChange};
    let fx = EngineFixture::new();
    let provider = Scripted::echo("must not infer");
    let e = engine(&fx, &provider);
    let shared = e.handle.shared().clone();
    shared.handle.block_on(async {
        let (dir, mut start) = start(shared.clone(), "success").await;
        let (ready, prepared) = tokio::sync::oneshot::channel();
        let (proceed, wait) = tokio::sync::oneshot::channel();
        start.test_preparing = Some((ready, wait));
        let result = shared.submit_cli_agent_root(start);
        tokio::time::timeout(Duration::from_secs(10), prepared)
            .await
            .unwrap()
            .unwrap();
        let receipt = shared
            .store
            .cli_agent_turn("s", "owned-r")
            .unwrap()
            .unwrap();
        let sess = shared.session("s").unwrap();
        assert_eq!(sess.active_run(), Some(receipt.scope.run));
        assert_eq!(receipt.state, cli_agent_turns::State::Admitted);
        shared
            .commit(&sess, |tx| {
                let entry = Store::append_entry(
                    tx,
                    "s",
                    crate::util::now_ms(),
                    sess.active_run(),
                    &EntryKind::Notice {
                        level: pipkin_engine_protocol::NoticeLevel::Info,
                        text: "during preparation".into(),
                    },
                )?;
                Ok(((), vec![SessionChange::EntryAppended { entry }]))
            })
            .unwrap();
        let mut revision = 0;
        let mut order = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), async {
            while order.len() < 2 {
                while let Ok(ServerFrame::Event { event, .. }) = e.conn.try_recv() {
                    assert!(!matches!(event, Event::ResyncRequired));
                    if let Event::Session {
                        session,
                        revision: next,
                        change,
                    } = event
                    {
                        if session != "s" {
                            continue;
                        }
                        assert!(next >= revision);
                        revision = next;
                        if let SessionChange::EntryAppended { entry } = change {
                            match entry.kind {
                                EntryKind::Message {
                                    request: Some(key), ..
                                } if key == "owned-r" => order.push("input"),
                                EntryKind::Notice { text, .. } if text == "during preparation" => {
                                    order.push("notice")
                                }
                                _ => {}
                            }
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(order, vec!["input", "notice"]);
        assert_eq!(
            shared.stop_by_backend("s").await.unwrap(),
            Reply::Stopping {
                run: Some(receipt.scope.run)
            }
        );
        assert_eq!(
            shared.store.run(receipt.scope.run).unwrap().unwrap().state,
            crate::store::RunState::Stopping
        );
        assert!(!shared.external_tasks.is_empty());
        proceed.send(()).unwrap();
        assert!(result.await.unwrap().is_err());
        idle(&shared).await;
        assert_eq!(
            shared
                .store
                .cli_agent_turn("s", "owned-r")
                .unwrap()
                .unwrap()
                .state,
            cli_agent_turns::State::Rejected
        );
        assert!(sess.active_run().is_none());
        assert!(sess.failure().is_none());
        assert!(!dir.path().join("launches").exists());
        // Stop before launch leaves no uncertain CLI transcript. A NEW explicit input
        // may start fresh after cleanup; the rejected request is never replayed.
        let ownership = shared
            .cli_agents
            .begin_turn(&shared.store, owner(&shared), "s")
            .unwrap();
        let mut next = recipe(&dir, ownership);
        next.request = "explicit-after-stop".into();
        next.operation = "after-stop-op".into();
        let (send, receive) = tokio::sync::oneshot::channel();
        next.test_peer = Some(send);
        let resumed = shared.submit_cli_agent_root(next);
        let peer = receive.await.unwrap();
        client(peer, dir.path().into(), true).await;
        let receipt = resumed.await.unwrap().unwrap();
        assert_eq!(receipt.state, cli_agent_turns::State::Completed);
        assert_eq!(
            receipt.mode,
            crate::execution::claude_cli::policy::TurnMode::Fresh
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("launches"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        assert_eq!(
            shared
                .store
                .cli_agent_turn("s", "owned-r")
                .unwrap()
                .unwrap()
                .state,
            cli_agent_turns::State::Rejected
        );
    });
    assert!(provider.requests().is_empty());
}
#[test]
fn removal_waits_for_owned_preparation_and_keeps_permanent_rejected_receipt() {
    let fx = EngineFixture::new();
    let provider = Scripted::echo("must not infer");
    let e = engine(&fx, &provider);
    let shared = e.handle.shared().clone();
    shared.handle.block_on(async {
        let (dir, mut start) = start(shared.clone(), "success").await;
        let cancel = start.ownership.cancellation();
        let (ready, prepared) = tokio::sync::oneshot::channel();
        let (proceed, wait) = tokio::sync::oneshot::channel();
        start.test_preparing = Some((ready, wait));
        let result = shared.submit_cli_agent_root(start);
        tokio::time::timeout(Duration::from_secs(10), prepared)
            .await
            .unwrap()
            .unwrap();
        let deleting = shared.clone();
        let removal = tokio::spawn(async move { deleting.remove_session("s").await });
        tokio::time::timeout(Duration::from_secs(5), cancel.cancelled())
            .await
            .unwrap();
        assert!(!shared.external_tasks.is_empty());
        proceed.send(()).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(10), removal)
                .await
                .unwrap()
                .unwrap()
                .is_ok()
        );
        assert!(result.await.unwrap().is_err());
        idle(&shared).await;
        assert!(shared.store.session("s").unwrap().is_none());
        assert_eq!(
            shared
                .store
                .cli_agent_turn("s", "owned-r")
                .unwrap()
                .unwrap()
                .state,
            cli_agent_turns::State::Rejected
        );
        assert!(!shared.cli_agents.has_turn("s"));
        assert!(!dir.path().join("launches").exists());
    });
}
#[test]
fn changed_duplicate_cannot_mutate_receipt_or_gain_a_second_launch() {
    let fx = EngineFixture::new();
    let provider = Scripted::echo("must not infer");
    let e = engine(&fx, &provider);
    let shared = e.handle.shared().clone();
    shared.handle.block_on(async {
        let (dir, mut start) = start(shared.clone(), "success").await;
        start.assets_parent = dir.path().join("missing-parent");
        assert!(shared.submit_cli_agent_root(start).await.unwrap().is_err());
        idle(&shared).await;
        let receipt = shared
            .store
            .cli_agent_turn("s", "owned-r")
            .unwrap()
            .unwrap();
        let turn = shared
            .cli_agents
            .begin_turn(&shared.store, owner(&shared), "s")
            .unwrap();
        let mut changed = recipe(&dir, turn);
        changed.text = "different input".into();
        assert!(
            shared
                .submit_cli_agent_root(changed)
                .await
                .unwrap()
                .is_err()
        );
        idle(&shared).await;
        assert_eq!(
            shared
                .store
                .cli_agent_turn("s", "owned-r")
                .unwrap()
                .unwrap(),
            receipt
        );
        assert!(!dir.path().join("launches").exists());
    });
}
