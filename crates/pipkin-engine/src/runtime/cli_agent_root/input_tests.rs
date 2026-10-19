//! Owned offline CLI peers only. No installed executable or Claude prompt allowance.
use super::goal_lifecycle_tests::{Ready, configured, next, response, setup};
use super::service_tests::peer_marked;
use super::*;
use crate::{
    provider::scripted::Scripted,
    runtime::{
        cli_agent_inputs::{Enqueued, Input},
        tests::Fixture as EngineFixture,
    },
    store::{RequestState, cli_agent_turns::inputs::Kind},
};
fn enqueue(
    s: &Arc<crate::runtime::Shared>,
    f: &Ready,
    key: &str,
    kind: Kind,
    text: &str,
) -> Enqueued {
    s.enqueue_cli_input(
        Input {
            images: vec![],
            owner: f.owner,
            session: "s".into(),
            request: key.into(),
            generation: 2,
            operation: format!("op-{key}"),
            kind,
            text: text.into(),
        },
        f.resources.clone(),
    )
    .unwrap()
}
async fn finish(q: Enqueued) -> Result<cli_agent_turns::Receipt, String> {
    tokio::time::timeout(Duration::from_secs(20), q.completion.unwrap())
        .await
        .unwrap()
        .unwrap()
}
async fn run(f: &mut Ready, turn: u32, compact: bool, tokens: bool) {
    let peer = next(f).await;
    let marker = format!("{}-{turn}", f.uuid);
    response(f.dir.path(), &marker, &f.uuid, "done");
    let path = f.dir.path().join(format!("answer-{marker}"));
    let mut answer: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    if tokens {
        answer["message"]["usage"] = json!({"input_tokens":10,"cache_read_input_tokens":20,"cache_creation_input_tokens":30,"output_tokens":999});
    }
    let prefix = if compact {
        format!(
            "{}\n",
            json!({"type":"system","subtype":"compact_boundary","session_id":f.uuid,"compact_metadata":{"trigger":"manual","pre_tokens":12345}})
        )
    } else {
        String::new()
    };
    std::fs::write(path, format!("{prefix}{answer}\n")).unwrap();
    // Real helpers start inside the launched CLI. Receiving the test capability can
    // precede admission/preflight, especially with a 2 MiB prompt under workspace load.
    // Do not consume the production handshake deadline before that process exists.
    tokio::time::timeout(Duration::from_secs(15), async {
        while !f.dir.path().join(format!("prompt-{marker}")).exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    peer_marked(peer, f.dir.path().into(), vec![], false, Some(marker)).await;
}
#[test]
fn cli_inputs_prioritize_steering_preserve_fifo_and_withdraw_without_replay() {
    let fx = EngineFixture::new();
    let p = Scripted::echo("never API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let mut f = setup(&s).await;
        let hold = s.cli_agents.begin_turn(&s.store, f.owner, "s").unwrap();
        let a = enqueue(&s, &f, "a", Kind::FollowUp, "queued a");
        let b = enqueue(&s, &f, "b", Kind::Steer, "queued b");
        let c = enqueue(&s, &f, "c", Kind::FollowUp, "queued c");
        let d = enqueue(&s, &f, "d", Kind::FollowUp, "queued d");
        let duplicate = enqueue(&s, &f, "b", Kind::Steer, "queued b");
        assert_eq!(duplicate.entry, b.entry);
        assert!(duplicate.completion.is_none());
        s.cancel_queued("s", c.entry).await.unwrap();
        drop(hold);
        run(&mut f, 2, false, false).await;
        let receipt = finish(b).await.unwrap();
        assert_eq!(
            receipt.mode,
            crate::execution::claude_cli::policy::TurnMode::Resume
        );
        run(&mut f, 3, false, false).await;
        finish(a).await.unwrap();
        run(&mut f, 4, false, false).await;
        finish(d).await.unwrap();
        assert!(finish(c).await.is_err());
        assert!(s.store.queue("s").unwrap().is_empty());
        let read = |n| {
            std::fs::read_to_string(f.dir.path().join(format!("prompt-{}-{n}", f.uuid))).unwrap()
        };
        assert_eq!(read(2), "queued b");
        assert_eq!(read(3), "queued a");
        assert_eq!(read(4), "queued d");
        assert_eq!(
            s.store.request("s", "b").unwrap().unwrap().mode,
            pipkin_engine_protocol::InputMode::Steer
        );
        assert!(
            enqueue(&s, &f, "b", Kind::Steer, "queued b")
                .completion
                .is_none()
        );
        assert!(p.requests().is_empty());
    });
    e.handle.shutdown(Duration::from_secs(5));
}
#[test]
fn cli_compaction_requires_boundary_and_reports_request_context_without_api_estimates() {
    let fx = EngineFixture::new();
    let p = Scripted::echo("never API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let mut f = setup(&s).await;
        let initial = s.context_usage("s").unwrap();
        assert_eq!(initial.tokens, None);
        assert_eq!(initial.compaction, "cli-managed");
        let q = enqueue(&s, &f, "compact", Kind::Compact, "preserve decisions");
        run(&mut f, 2, true, false).await;
        finish(q).await.unwrap();
        let usage = s.context_usage("s").unwrap();
        assert_eq!(usage.tokens, None);
        assert_eq!(usage.compaction, "cli-compacted");
        assert_eq!(usage.capacity, None);
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join(format!("prompt-{}-2", f.uuid))).unwrap(),
            "/compact preserve decisions"
        );
        let q = enqueue(&s, &f, "after", Kind::FollowUp, "queued after compact");
        run(&mut f, 3, false, true).await;
        finish(q).await.unwrap();
        let usage = s.context_usage("s").unwrap();
        assert_eq!(usage.tokens, Some(60));
        assert_eq!(usage.source, "cli-reported-input");
        assert_eq!(usage.remaining, None);
        s.store
            .tx(|c| {
                c.execute(
                    "UPDATE cli_agent_turns SET fenced=1 WHERE session='s' AND request_key='after'",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        let uncertain = s.context_usage("s").unwrap();
        assert_eq!(uncertain.tokens, None);
        assert!(uncertain.stale);
        let count: u64 = s
            .store
            .read(|c| Ok(c.query_row("SELECT COUNT(*) FROM compactions", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(count, 0);
        assert!(p.requests().is_empty());
    });
    e.handle.shutdown(Duration::from_secs(5));
}
#[test]
fn cli_compaction_missing_boundary_fences_success_text_and_does_not_run_followup() {
    let fx = EngineFixture::new();
    let p = Scripted::echo("never API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let mut f = setup(&s).await;
        let q = enqueue(&s, &f, "compact", Kind::Compact, "");
        let after = enqueue(&s, &f, "after", Kind::FollowUp, "queued never");
        run(&mut f, 2, false, false).await;
        assert!(finish(q).await.is_err());
        assert!(finish(after).await.is_err());
        assert_eq!(
            s.store
                .cli_agent_turn("s", "compact")
                .unwrap()
                .unwrap()
                .state,
            cli_agent_turns::State::Unknown
        );
        assert!(s.store.cli_agent_turn("s", "after").unwrap().is_none());
        assert_eq!(
            s.store.request("s", "after").unwrap().unwrap().state,
            RequestState::Withdrawn
        );
        assert_eq!(s.context_usage("s").unwrap().tokens, None);
        assert!(p.requests().is_empty());
    });
    e.handle.shutdown(Duration::from_secs(5));
}
#[test]
fn cli_queued_input_stop_revocation_and_restart_data_never_authorize_delivery() {
    for action in ["stop", "revoke", "recovery", "withdraw"] {
        let fx = EngineFixture::new();
        let p = Scripted::echo("never API");
        let e = configured(&fx, &p);
        let s = e.handle.shared().clone();
        s.handle.block_on(async {
            let f = setup(&s).await;
            let hold = s.cli_agents.begin_turn(&s.store, f.owner, "s").unwrap();
            let q = enqueue(&s, &f, "pending", Kind::Steer, "queued pending");
            match action {
                "stop" => {
                    s.stop_cli_agent_root("s").await.unwrap();
                }
                "revoke" => s.cli_agents.revoke_owner(f.owner),
                "recovery" => {
                    s.store
                        .recover_cli_agent_turns(crate::util::now_ms())
                        .unwrap();
                }
                _ => {
                    s.cancel_queued("s", q.entry).await.unwrap();
                }
            }
            drop(hold);
            assert!(finish(q).await.is_err());
            assert!(s.store.cli_agent_turn("s", "pending").unwrap().is_none());
            assert_eq!(
                s.store.request("s", "pending").unwrap().unwrap().state,
                RequestState::Withdrawn
            );
            assert_eq!(
                std::fs::read_to_string(f.dir.path().join("launches"))
                    .unwrap()
                    .lines()
                    .count(),
                1
            );
            assert!(p.requests().is_empty());
        });
        e.handle.shutdown(Duration::from_secs(5));
    }
}
#[test]
fn cli_input_capacity_wait_is_cancellable_and_duplicate_conflicts_are_read_only() {
    let fx = EngineFixture::new();
    let p = Scripted::echo("never API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let f = setup(&s).await;
        let capacity = s
            .generations
            .clone()
            .acquire_many_owned(s.generations.available_permits() as u32)
            .await
            .unwrap();
        let q = enqueue(&s, &f, "wait", Kind::FollowUp, "queued wait");
        assert!(
            s.enqueue_cli_input(
                Input {
                    images: vec![],
                    owner: f.owner,
                    session: "s".into(),
                    request: "wait".into(),
                    generation: 2,
                    operation: "op-wait".into(),
                    kind: Kind::Steer,
                    text: "queued changed".into()
                },
                f.resources.clone()
            )
            .is_err()
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            while !s.cli_agents.has_turn("s") {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(s.store.cli_agent_turn("s", "wait").unwrap().is_none());
        s.cli_agents.stop("s");
        assert!(finish(q).await.is_err());
        drop(capacity);
        assert!(s.store.cli_agent_turn("s", "wait").unwrap().is_none());
        assert!(p.requests().is_empty());
    });
    e.handle.shutdown(Duration::from_secs(5));
}

#[test]
fn cli_compaction_cannot_execute_native_tools_even_if_sdk_attempts_them() {
    let fx = EngineFixture::new();
    let p = Scripted::echo("never API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let mut f = setup(&s).await;
        let q = enqueue(&s, &f, "compact", Kind::Compact, "");
        let peer = next(&mut f).await;
        peer_marked(
            peer,
            f.dir.path().into(),
            vec![(
                super::service_tests::call(
                    2,
                    "write",
                    json!({"path":"forbidden.txt","content":"should not run"}),
                ),
                true,
                "",
            )],
            true,
            Some(format!("{}-2", f.uuid)),
        )
        .await;
        assert!(finish(q).await.is_err());
        assert!(!f.dir.path().join("forbidden.txt").exists());
        let count: u64 = s
            .store
            .read(|c| Ok(c.query_row("SELECT COUNT(*) FROM tool_calls", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(count, 0);
        assert!(p.requests().is_empty());
    });
    e.handle.shutdown(Duration::from_secs(5));
}

#[test]
fn cli_queue_withdrawal_during_capacity_wait_never_admits_input() {
    let fx = EngineFixture::new();
    let p = Scripted::echo("never API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let f = setup(&s).await;
        let capacity = s
            .generations
            .clone()
            .acquire_many_owned(s.generations.available_permits() as u32)
            .await
            .unwrap();
        let q = enqueue(&s, &f, "withdraw", Kind::FollowUp, "queued withdraw");
        tokio::time::timeout(Duration::from_secs(5), async {
            while !s.cli_agents.has_turn("s") {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(2), s.cancel_queued("s", q.entry))
            .await
            .unwrap()
            .unwrap();
        assert!(finish(q).await.is_err());
        drop(capacity);
        assert!(s.store.cli_agent_turn("s", "withdraw").unwrap().is_none());
        assert!(p.requests().is_empty());
    });
    e.handle.shutdown(Duration::from_secs(5));
}

fn queue_modes(fx: &EngineFixture, steer: &str, follow: &str) {
    std::fs::create_dir_all(&fx.config().config_dir).unwrap();
    std::fs::write(
        fx.config().config_dir.join("settings.json"),
        json!({"steeringMode":steer,"followUpMode":follow}).to_string(),
    )
    .unwrap();
}
#[test]
fn cli_batch_all_keeps_exact_messages_receipts_and_resume_in_one_invocation() {
    let fx = EngineFixture::new();
    queue_modes(&fx, "all", "all");
    let p = Scripted::echo("never API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let mut f = setup(&s).await;
        let hold = s.cli_agents.begin_turn(&s.store, f.owner, "s").unwrap();
        let a = enqueue(&s, &f, "a", Kind::FollowUp, "queued first");
        let text = "queued second\n\"quoted\" \\ snow 雪";
        let b = enqueue(&s, &f, "b", Kind::FollowUp, text);
        let c = enqueue(&s, &f, "c", Kind::FollowUp, "queued withdrawn");
        s.cancel_queued("s", c.entry).await.unwrap();
        drop(hold);
        run(&mut f, 2, false, false).await;
        let ar = finish(a).await.unwrap();
        let br = finish(b).await.unwrap();
        assert_eq!(ar.scope.run, br.scope.run);
        assert!(finish(c).await.is_err());
        let prompt =
            std::fs::read_to_string(f.dir.path().join(format!("prompt-{}-2", f.uuid))).unwrap();
        let messages: Vec<(String, String)> =
            serde_json::from_str(prompt.split_once('\n').unwrap().1).unwrap();
        assert_eq!(
            messages,
            vec![
                ("a".into(), "queued first".into()),
                ("b".into(), text.into())
            ]
        );
        let b = s.store.request("s", "b").unwrap().unwrap();
        assert_eq!(b.state, RequestState::Settled);
        assert_eq!(b.run, Some(ar.scope.run));
        assert_eq!(b.text, text);
        assert!(
            enqueue(&s, &f, "b", Kind::FollowUp, text)
                .completion
                .is_none()
        );
        let q = enqueue(&s, &f, "after", Kind::FollowUp, "queued after batch");
        run(&mut f, 3, false, false).await;
        finish(q).await.unwrap();
        assert!(f.peers.try_recv().is_err());
        assert!(p.requests().is_empty());
    });
    e.handle.shutdown(Duration::from_secs(5));
}
#[test]
fn cli_mid_turn_steering_all_is_atomic_with_tool_receipt_and_replay_is_exact() {
    use super::service_tests::call;
    let fx = EngineFixture::new();
    queue_modes(&fx, "all", "all");
    let p = Scripted::echo("never API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let mut f=setup(&s).await;
        let lead=enqueue(&s,&f,"lead",Kind::FollowUp,"queued active");
        let peer=next(&mut f).await; // Root admission is already committed.
        let a=enqueue(&s,&f,"a",Kind::Steer,"use Rust");
        let b=enqueue(&s,&f,"b",Kind::Steer,"keep tests\n\"all\"");
        let withdrawn=enqueue(&s,&f,"withdrawn",Kind::Steer,"never deliver");
        s.cancel_queued("s",withdrawn.entry).await.unwrap();
        let later=enqueue(&s,&f,"later",Kind::FollowUp,"queued later");
        let marker=format!("{}-2",f.uuid);response(f.dir.path(),&marker,&f.uuid,"done");
        let request=call(2,"get_goal",json!({}));
        let replies=peer_marked(peer,f.dir.path().into(),vec![(request.clone(),true,"use Rust"),(request,true,"keep tests")],false,Some(marker)).await;
        assert_eq!(replies[0],replies[1]);assert!(!replies[0].to_string().contains("never deliver"));
        let root=finish(lead).await.unwrap();
        assert_eq!(finish(a).await.unwrap().scope.run,root.scope.run);
        assert_eq!(finish(b).await.unwrap().scope.run,root.scope.run);
        assert!(finish(withdrawn).await.is_err());
        let count:u64=s.store.read(|c|Ok(c.query_row("SELECT COUNT(*) FROM cli_agent_input_deliveries WHERE run=?1 AND channel='tool'",[root.scope.run],|r|r.get(0))?)).unwrap();
        assert_eq!(count,2);
        run(&mut f,3,false,false).await;finish(later).await.unwrap();assert!(p.requests().is_empty());
    });
    e.handle.shutdown(Duration::from_secs(5));
}
#[test]
fn cli_mid_turn_one_at_a_time_does_not_consume_on_replayed_tool_boundary() {
    use super::service_tests::call;
    let fx = EngineFixture::new();
    let p = Scripted::echo("never API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let mut f = setup(&s).await;
        let lead = enqueue(&s, &f, "lead", Kind::FollowUp, "queued active");
        let peer = next(&mut f).await;
        let a = enqueue(&s, &f, "a", Kind::Steer, "first steer");
        let b = enqueue(&s, &f, "b", Kind::Steer, "second steer");
        let marker = format!("{}-2", f.uuid);
        response(f.dir.path(), &marker, &f.uuid, "done");
        let request = call(2, "get_goal", json!({}));
        let replies = peer_marked(
            peer,
            f.dir.path().into(),
            vec![
                (request.clone(), true, "first steer"),
                (request, true, "first steer"),
                (call(3, "get_goal", json!({})), true, "second steer"),
            ],
            false,
            Some(marker),
        )
        .await;
        assert_eq!(replies[0], replies[1]);
        assert!(!replies[0].to_string().contains("second steer"));
        let root = finish(lead).await.unwrap();
        assert_eq!(finish(a).await.unwrap().scope.run, root.scope.run);
        assert_eq!(finish(b).await.unwrap().scope.run, root.scope.run);
        assert!(f.peers.try_recv().is_err());
        assert!(p.requests().is_empty());
    });
    e.handle.shutdown(Duration::from_secs(5));
}
#[test]
fn cli_delivered_steering_disconnect_settles_unknown_and_never_replays() {
    use super::service_tests::call;
    let fx = EngineFixture::new();
    let p = Scripted::echo("never API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let mut f=setup(&s).await;
        let lead=enqueue(&s,&f,"lead",Kind::FollowUp,"queued active");let peer=next(&mut f).await;
        let steer=enqueue(&s,&f,"steer",Kind::Steer,"change direction");
        let deadline=Instant::now()+Duration::from_secs(15);
        let (mut read,mut write)=peer.connect(deadline).await.unwrap().split();
        for request in [json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":PROTOCOL_VERSION,"clientInfo":{"name":"fixture","version":"1"},"capabilities":{}}}),json!({"jsonrpc":"2.0","method":"notifications/initialized"}),call(2,"get_goal",json!({}))] {
            write.write(&serde_json::to_vec(&request).unwrap(),deadline).await.unwrap();
            if request.get("id").is_some() {let reply=read.read(deadline).await.unwrap();if request["id"]==2 {assert!(String::from_utf8(reply).unwrap().contains("change direction"));}}
        }
        // Lose the connection after durable delivery but before terminal/close evidence.
        drop(read);drop(write);
        assert!(finish(lead).await.is_err());assert!(finish(steer).await.is_err());
        let row=s.store.request("s","steer").unwrap().unwrap();
        assert_eq!(row.state,RequestState::Settled);assert!(matches!(row.outcome,Some(pipkin_engine_protocol::RunOutcome::Interrupted{..})));
        s.store.recover_cli_agent_turns(crate::util::now_ms()).unwrap();
        assert!(enqueue(&s,&f,"steer",Kind::Steer,"change direction").completion.is_none());
        assert!(s.store.queue("s").unwrap().is_empty());assert!(f.peers.try_recv().is_err());assert!(p.requests().is_empty());
    });
    e.handle.shutdown(Duration::from_secs(5));
}

#[test]
fn cli_batch_all_does_not_cross_compaction_or_pull_withdrawn_capacity_waiters() {
    let fx = EngineFixture::new();
    queue_modes(&fx, "all", "all");
    let p = Scripted::echo("never API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let mut f = setup(&s).await;
        let capacity = s
            .generations
            .clone()
            .acquire_many_owned(s.generations.available_permits() as u32)
            .await
            .unwrap();
        let a = enqueue(&s, &f, "a", Kind::FollowUp, "queued first");
        let b = enqueue(&s, &f, "b", Kind::FollowUp, "queued withdrawn at capacity");
        let compact = enqueue(&s, &f, "compact", Kind::Compact, "");
        let after = enqueue(&s, &f, "after", Kind::FollowUp, "queued after compact");
        s.cancel_queued("s", b.entry).await.unwrap();
        assert!(finish(b).await.is_err());
        drop(capacity);
        run(&mut f, 2, false, false).await;
        finish(a).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join(format!("prompt-{}-2", f.uuid))).unwrap(),
            "queued first"
        );
        run(&mut f, 3, true, false).await;
        finish(compact).await.unwrap();
        run(&mut f, 4, false, false).await;
        finish(after).await.unwrap();
        assert!(s.store.queue("s").unwrap().is_empty());
        assert!(p.requests().is_empty());
    });
    e.handle.shutdown(Duration::from_secs(5));
}
#[test]
fn cli_steering_never_adopts_old_approval_or_persisted_intents() {
    use super::service_tests::call;
    use crate::{
        runtime::cli_agent_runtime::admission::Start, store::cli_agent_turns::inputs::Queued,
    };
    let fx = EngineFixture::new();
    queue_modes(&fx, "all", "all");
    let p = Scripted::echo("never API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let mut f = setup(&s).await;
        let hold = s.cli_agents.begin_turn(&s.store, f.owner, "s").unwrap();
        let prepared = hold.prepare().await.unwrap();
        let binding = hold.binding().clone();
        let old = Queued {
            images: vec![],
            session: "s".into(),
            request: "old".into(),
            text: "old approval must not deliver".into(),
            kind: Kind::Steer,
            generation: 2,
            operation: "old-op".into(),
            ticket: "old-ticket".into(),
        };
        // Retain the old live ticket to make revocation/reapproval race deterministic.
        let (old_ticket, _) = s
            .cli_agents
            .queue_input(&s.store, f.owner, old.clone())
            .unwrap();
        drop(hold);
        s.cli_agents.revoke_session("s");
        s.cli_agents
            .approve_return(&s.store, f.owner, &prepared, &binding)
            .unwrap();
        let orphan = Queued {
            request: "orphan".into(),
            text: "persisted data must not deliver".into(),
            ticket: "orphan-ticket".into(),
            ..old.clone()
        };
        s.store
            .queue_cli_input(&binding, &orphan, crate::util::now_ms())
            .unwrap();
        let ownership = s.cli_agents.begin_turn(&s.store, f.owner, "s").unwrap();
        let (send, receive) = tokio::sync::oneshot::channel();
        let root = s.submit_cli_agent_root(Start {
            images: vec![],
            ownership,
            request: "fresh-owner".into(),
            generation: 3,
            operation: "fresh-owner-op".into(),
            text: "queued explicit new owner".into(),
            system: f.resources.root_system.to_string(),
            assets_parent: f.resources.image.assets_parent().into(),
            image: f.resources.image.clone(),
            tools: f.resources.tools.clone(),
            services: crate::runtime::cli_bridge_driver::Services {
                child_resources: Some(f.resources.clone()),
                ..Default::default()
            },
            deadline: Instant::now() + Duration::from_secs(30),
            cancel: CancellationToken::new(),
            test_peer: Some(send),
            test_preparing: None,
        });
        let peer = receive.await.unwrap();
        let valid = enqueue(&s, &f, "valid", Kind::Steer, "current user input");
        let marker = format!("{}-2", f.uuid);
        response(f.dir.path(), &marker, &f.uuid, "done");
        let replies = peer_marked(
            peer,
            f.dir.path().into(),
            vec![(call(2, "get_goal", json!({})), true, "current user input")],
            false,
            Some(marker),
        )
        .await;
        assert!(!replies[0].to_string().contains("must not deliver"));
        assert_eq!(
            root.await.unwrap().unwrap().state,
            cli_agent_turns::State::Completed
        );
        finish(valid).await.unwrap();
        assert_eq!(
            s.store.request("s", "old").unwrap().unwrap().state,
            RequestState::Queued
        );
        assert_eq!(
            s.store.request("s", "orphan").unwrap().unwrap().state,
            RequestState::Queued
        );
        drop(old_ticket);
        s.store
            .recover_cli_agent_turns(crate::util::now_ms())
            .unwrap();
        assert!(s.store.queue("s").unwrap().is_empty());
        assert!(f.peers.try_recv().is_err());
        assert!(p.requests().is_empty());
    });
    e.handle.shutdown(Duration::from_secs(5));
}

#[test]
fn cli_oversized_mid_turn_steer_stays_queued_then_runs_losslessly() {
    use super::service_tests::call;
    let fx = EngineFixture::new();
    let p = Scripted::echo("never API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let mut f = setup(&s).await;
        let lead = enqueue(&s, &f, "lead", Kind::FollowUp, "queued active");
        let peer = next(&mut f).await;
        let text = format!("queued {}", "x".repeat(96 * 1024));
        let steer = enqueue(&s, &f, "large", Kind::Steer, &text);
        let marker = format!("{}-2", f.uuid);
        response(f.dir.path(), &marker, &f.uuid, "done");
        let replies = peer_marked(
            peer,
            f.dir.path().into(),
            vec![(call(2, "get_goal", json!({})), true, "no goal")],
            false,
            Some(marker),
        )
        .await;
        assert!(!replies[0].to_string().contains("Pipkin user steering"));
        let root = finish(lead).await.unwrap();
        run(&mut f, 3, false, false).await;
        let delivered = finish(steer).await.unwrap();
        assert_ne!(root.scope.run, delivered.scope.run);
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join(format!("prompt-{}-3", f.uuid))).unwrap(),
            text
        );
        assert!(s.store.queue("s").unwrap().is_empty());
        assert!(p.requests().is_empty());
    });
    e.handle.shutdown(Duration::from_secs(5));
}

#[test]
fn parity_post_stop_explicit_input_uses_new_epoch_without_replaying_old_queue() {
    for capacity_wait in [false, true] {
        let fx = EngineFixture::new();
        let p = Scripted::echo("never API");
        let e = configured(&fx, &p);
        let s = e.handle.shared().clone();
        s.handle.block_on(async {
            let mut f = setup(&s).await;
            let capacity = if capacity_wait {
                Some(
                    s.generations
                        .clone()
                        .acquire_many_owned(s.config.max_active_generations.max(1) as u32)
                        .await
                        .unwrap(),
                )
            } else {
                None
            };
            let hold = if capacity_wait {
                None
            } else {
                Some(s.cli_agents.begin_turn(&s.store, f.owner, "s").unwrap())
            };
            let old = enqueue(
                &s,
                &f,
                "old-before-stop",
                Kind::FollowUp,
                "queued old before stop",
            );
            tokio::time::timeout(Duration::from_secs(5), async {
                while !s.cli_agents.has_turn("s") {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            s.stop_cli_agent_root("s").await.unwrap();
            drop(hold);
            assert!(finish(old).await.is_err());
            drop(capacity);
            assert_eq!(
                s.store
                    .request("s", "old-before-stop")
                    .unwrap()
                    .unwrap()
                    .state,
                RequestState::Withdrawn
            );
            let fresh = enqueue(
                &s,
                &f,
                "explicit-after-stop",
                Kind::FollowUp,
                "queued explicit after stop",
            );
            run(&mut f, 2, false, false).await;
            let receipt = finish(fresh).await.unwrap();
            assert_eq!(receipt.state, cli_agent_turns::State::Completed);
            assert_eq!(
                receipt.mode,
                crate::execution::claude_cli::policy::TurnMode::Resume
            );
            assert!(
                s.store
                    .cli_agent_turn("s", "old-before-stop")
                    .unwrap()
                    .is_none()
            );
            let delivered =
                std::fs::read_to_string(f.dir.path().join(format!("prompt-{}-2", f.uuid))).unwrap();
            assert!(delivered.contains("queued explicit after stop"));
            assert!(!delivered.contains("queued old before stop"));
            assert_eq!(
                std::fs::read_to_string(f.dir.path().join("launches"))
                    .unwrap()
                    .lines()
                    .count(),
                2
            );
        });
        assert!(p.requests().is_empty());
        e.handle.shutdown(Duration::from_secs(5));
    }
}

struct ConcurrentWrite {
    entered: mpsc::UnboundedSender<()>,
    release: Arc<tokio::sync::Semaphore>,
}
impl crate::tools::Tool for ConcurrentWrite {
    fn spec(&self) -> crate::provider::ToolSpec {
        crate::tools::write::WriteTool.spec()
    }
    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a crate::tools::ToolCtx,
    ) -> crate::provider::BoxFuture<'a, Result<crate::tools::ToolOutput, crate::tools::ToolError>>
    {
        Box::pin(async move {
            self.entered.send(()).unwrap();
            tokio::select! {_=ctx.cancel.cancelled()=>return Err(crate::tools::ToolError::new("stopped")),p=self.release.acquire()=>p.unwrap().forget()}
            crate::tools::write::WriteTool.execute(args, ctx).await
        })
    }
}
#[test]
fn parity_parallel_native_commits_deliver_steering_once_and_close_cleanly() {
    let fx = EngineFixture::new();
    let p = Scripted::echo("never API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let mut f=setup(&s).await;let (entered,mut arrivals)=mpsc::unbounded_channel();let release=Arc::new(tokio::sync::Semaphore::new(0));
        let mut resources=(*f.resources).clone();let index=resources.tools.iter().position(|t|t.spec().name=="write").unwrap();resources.tools[index]=Arc::new(ConcurrentWrite {entered,release:release.clone()});f.resources=Arc::new(resources);
        let q=enqueue(&s,&f,"concurrent",Kind::FollowUp,"queued concurrent fixture");let peer=next(&mut f).await;
        let marker=format!("{}-2",f.uuid);response(f.dir.path(),&marker,&f.uuid,"done");
        let deadline=Instant::now()+Duration::from_secs(15);let (mut r,mut w)=peer.connect(deadline).await.unwrap().split();
        for request in [json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":PROTOCOL_VERSION,"clientInfo":{"name":"fixture","version":"1"},"capabilities":{}}}),json!({"jsonrpc":"2.0","method":"notifications/initialized"})] {
            w.write(&serde_json::to_vec(&request).unwrap(),deadline).await.unwrap();if request.get("id").is_some(){r.read(deadline).await.unwrap();}
        }
        for id in [1,2] {w.write(&serde_json::to_vec(&super::service_tests::call(id,"write",json!({"path":format!("effect-{id}"),"content":"once"}))).unwrap(),deadline).await.unwrap();}
        for _ in 0..2 {tokio::time::timeout(Duration::from_secs(5),arrivals.recv()).await.unwrap().unwrap();}
        let steering=enqueue(&s,&f,"parallel-steer",Kind::Steer,"steer parallel once");release.add_permits(2);
        let mut delivered=0;for _ in 0..2 {let result:Value=serde_json::from_slice(&r.read(deadline).await.unwrap()).unwrap();assert_eq!(result["result"]["isError"],false);delivered+=usize::from(result.to_string().contains("steer parallel once"));}
        assert_eq!(delivered,1);
        std::fs::write(f.dir.path().join(format!("acknowledged-{marker}")),"").unwrap();
        let control:Value=serde_json::from_slice(&r.read(deadline).await.unwrap()).unwrap();assert!(Control::Quiesce.matches(&control));w.write(&serde_json::to_vec(&Control::Quiesced.value()).unwrap(),deadline).await.unwrap();
        let control:Value=serde_json::from_slice(&r.read(deadline).await.unwrap()).unwrap();assert!(Control::Closed.matches(&control));std::fs::write(f.dir.path().join(format!("closed-{marker}")),"").unwrap();
        assert_eq!(finish(q).await.unwrap().state,cli_agent_turns::State::Completed);assert_eq!(finish(steering).await.unwrap().state,cli_agent_turns::State::Completed);
        for id in [1,2] {assert_eq!(std::fs::read_to_string(f.dir.path().join(format!("effect-{id}"))).unwrap(),"once");}
        assert!(s.store.queue("s").unwrap().is_empty());assert!(f.peers.try_recv().is_err());
    });
    assert!(p.requests().is_empty());
}

#[test]
fn parity_native_sized_prompt_arrives_exactly_and_oversize_is_refused_before_admission() {
    let fx = EngineFixture::new();
    let p = Scripted::echo("never API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let mut f = setup(&s).await;
        let max = pipkin_engine_protocol::limits::MAX_PROMPT_BYTES;
        let text = format!("queued {}x", "λ".repeat((max - 8) / 2));
        assert_eq!(text.len(), max);
        assert!(
            s.enqueue_cli_input(
                Input { images: vec![],
                    owner: f.owner,
                    session: "s".into(),
                    request: "oversize".into(),
                    generation: 2,
                    operation: "too-large".into(),
                    kind: Kind::FollowUp,
                    text: format!("{text}x")
                },
                f.resources.clone()
            )
            .is_err()
        );
        assert!(s.store.request("s", "oversize").unwrap().is_none());
        let q = enqueue(&s, &f, "large", Kind::FollowUp, &text);
        let mut completed = q.completion.unwrap();
        tokio::select! {
            result = &mut completed => panic!("large input failed before fixture completion: {result:?}"),
            () = run(&mut f, 2, false, false) => {},
        }
        assert_eq!(completed.await.unwrap().unwrap().state, cli_agent_turns::State::Completed);
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join(format!("prompt-{}-2", f.uuid))).unwrap(),
            text
        );
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("launches"))
                .unwrap()
                .lines()
                .count(),
            2
        );
    });
    assert!(p.requests().is_empty());
}

fn attached(
    s: &Arc<crate::runtime::Shared>,
    f: &Ready,
    key: &str,
    kind: Kind,
    text: &str,
    images: Vec<pipkin_engine_protocol::ImageInput>,
) -> Result<Enqueued, String> {
    s.enqueue_cli_input(
        Input {
            owner: f.owner,
            session: "s".into(),
            request: key.into(),
            generation: 2,
            operation: format!("op-{key}"),
            kind,
            text: text.into(),
            images,
        },
        f.resources.clone(),
    )
}
fn fixture_image() -> pipkin_engine_protocol::ImageInput {
    pipkin_engine_protocol::ImageInput {
        mime_type: "image/png".into(),
        data: "aW1hZ2U=".into(),
    }
}
#[test]
fn features_images_and_effort_survive_resume_queue_boundaries_and_duplicate_lookup() {
    let fx = EngineFixture::new();
    queue_modes(&fx, "all", "all");
    let p = Scripted::echo("never API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let mut f=super::goal_lifecycle_tests::setup_variant(&s,"goal-thinking").await;
        assert_eq!(s.store.cli_agent_binding("s").unwrap().unwrap().create.thinking.as_deref(),Some("high"));
        let hold=s.cli_agents.begin_turn(&s.store,f.owner,"s").unwrap();
        let a=enqueue(&s,&f,"before-image",Kind::FollowUp,"queued before image");
        let image=fixture_image();
        let b=attached(&s,&f,"image",Kind::FollowUp,"",vec![image.clone()]).unwrap();
        assert!(attached(&s,&f,"image",Kind::FollowUp,"",vec![image.clone()]).unwrap().completion.is_none());
        let mut changed=image.clone();changed.data="Y2hhbmdlZA==".into();
        assert!(attached(&s,&f,"image",Kind::FollowUp,"",vec![changed]).is_err());
        assert!(attached(&s,&f,"bad-compact",Kind::Compact,"",vec![image.clone()]).is_err());
        let c=enqueue(&s,&f,"after-image",Kind::FollowUp,"queued after image");
        drop(hold);
        run(&mut f,2,false,false).await;let ar=finish(a).await.unwrap();
        run(&mut f,3,false,false).await;let br=finish(b).await.unwrap();
        run(&mut f,4,false,false).await;let cr=finish(c).await.unwrap();
        assert_ne!(ar.scope.run,br.scope.run);assert_ne!(br.scope.run,cr.scope.run);
        let body:Value=serde_json::from_slice(&std::fs::read(f.dir.path().join(format!("prompt-{}-3",f.uuid))).unwrap()).unwrap();
        assert_eq!(body["message"]["content"],json!([{"type":"image","source":{"type":"base64","media_type":"image/png","data":image.data}}]));
        let options=std::fs::read_to_string(f.dir.path().join("launch-options")).unwrap();
        assert_eq!(options.lines().collect::<Vec<_>>(),vec!["high text","high text","high stream-json","high text"]);
        let row=s.store.request("s","image").unwrap().unwrap();assert_eq!(row.images,vec![image.clone()]);
        let entry=s.store.page("s",None,50).unwrap().0.into_iter().find(|e|e.id==br.user_entry).unwrap();
        assert!(matches!(entry.kind,pipkin_engine_protocol::EntryKind::Message{message:pipkin_engine_protocol::Message::User{content,..},..} if content==vec![pipkin_engine_protocol::Block::image("image/png",&image.data)]));
        assert!(s.store.queue("s").unwrap().is_empty());assert!(p.requests().is_empty());
    });
    e.handle.shutdown(Duration::from_secs(5));
}

#[test]
fn features_image_steering_is_structured_atomic_and_replayed_exactly() {
    use super::service_tests::call;
    let fx = EngineFixture::new();
    let p = Scripted::echo("never API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let mut f = setup(&s).await;
        let lead = enqueue(&s, &f, "lead-image", Kind::FollowUp, "queued active");
        let peer = next(&mut f).await;
        let image = fixture_image();
        let q = attached(
            &s,
            &f,
            "steer-image",
            Kind::Steer,
            "use this image",
            vec![image.clone()],
        )
        .unwrap();
        let marker = format!("{}-2", f.uuid);
        response(f.dir.path(), &marker, &f.uuid, "done");
        let request = call(2, "get_goal", json!({}));
        let replies = peer_marked(
            peer,
            f.dir.path().into(),
            vec![
                (request.clone(), true, "use this image"),
                (request, true, "use this image"),
            ],
            false,
            Some(marker),
        )
        .await;
        assert_eq!(replies[0], replies[1]);
        assert!(
            replies[0]["result"]["content"]
                .as_array()
                .unwrap()
                .iter()
                .any(|b| *b == json!({"type":"image","mimeType":"image/png","data":image.data}))
        );
        let root = finish(lead).await.unwrap();
        assert_eq!(finish(q).await.unwrap().scope.run, root.scope.run);
        assert_eq!(
            s.store.request("s", "steer-image").unwrap().unwrap().images,
            vec![image]
        );
        assert!(s.store.queue("s").unwrap().is_empty());
        assert!(f.peers.try_recv().is_err());
        assert!(p.requests().is_empty());
    });
    e.handle.shutdown(Duration::from_secs(5));
}

#[test]
fn features_resumed_usage_snapshots_persist_without_summing_session_totals() {
    let fx = EngineFixture::new();
    let p = Scripted::echo("never API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let mut f=setup(&s).await;
        for turn in [2,3] {
            let key=format!("metadata-{turn}");
            let q=enqueue(&s,&f,&key,Kind::FollowUp,"queued usage fixture");let peer=next(&mut f).await;
            let marker=format!("{}-{turn}",f.uuid);response(f.dir.path(),&marker,&f.uuid,"done");
            let path=f.dir.path().join(format!("terminal-{marker}"));
            let mut result:Value=serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            result["usage"]=json!({"input_tokens":3,"output_tokens":10,"cache_read_input_tokens":2,"cache_creation_input_tokens":5,"output_tokens_details":{"thinking_tokens":4},"cache_creation":{"ephemeral_1h_input_tokens":2},"speed":"standard","service_tier":"standard"});
            result["total_cost_usd"]=json!(0.25);
            result["duration_ms"]=json!(50);
            result["modelUsage"]=json!({"sonnet":{"inputTokens":100000,"outputTokens":100000,"contextWindow":200000}});
            std::fs::write(path,format!("{result}\n")).unwrap();
            peer_marked(peer,f.dir.path().into(),vec![],false,Some(marker)).await;
            let receipt=finish(q).await.unwrap();
            assert_eq!(receipt.mode,crate::execution::claude_cli::policy::TurnMode::Resume);
            let raw:String=s.store.read(|c|Ok(c.query_row("SELECT evidence FROM cli_agent_turns WHERE session='s' AND request_key=?1",[&key],|r|r.get(0))?)).unwrap();
            let saved:cli_agent_turns::Completion=serde_json::from_str(&raw).unwrap();
            let usage=saved.terminal.usage.unwrap();assert_eq!(usage.output_tokens,Some(10));assert_eq!(usage.reasoning_tokens,Some(4));
            let metadata=saved.terminal.metadata.unwrap();assert_eq!(metadata.session_snapshot["total_cost_usd"],0.25);
            assert_eq!(metadata.invocation["usage_details"]["speed"],"standard");
            assert!(enqueue(&s,&f,&key,Kind::FollowUp,"queued usage fixture").completion.is_none());
        }
        let mut total=0;
        for entry in s.store.page("s",None,50).unwrap().0 {
            if let pipkin_engine_protocol::EntryKind::Message{message:pipkin_engine_protocol::Message::Assistant{usage,..},..}=entry.kind {total+=usage.total_tokens;}
        }
        assert_eq!(total,47); // initial 7 + two invocations of 20; cumulative 200000 is not charged.
        assert!(p.requests().is_empty());
    });
    e.handle.shutdown(Duration::from_secs(5));
}
