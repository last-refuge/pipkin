//! Offline goal lifecycle: owned executable and authenticated native MCP only. No installed
//! Claude executable, inference account, credentials, network or prompt allowance is used.
use super::service_tests::{call, peer_marked};
use super::*;
use crate::{
    provider::scripted::Scripted,
    runtime::{cli_agent_host::ChildResources, tests::Fixture as EngineFixture},
};
use pipkin_engine_protocol::{GoalState, GoalView};
use tokio::sync::oneshot;
pub(super) struct Ready {
    pub(super) dir: tempfile::TempDir,
    pub(super) owner: u64,
    pub(super) uuid: String,
    pub(super) resources: Arc<ChildResources>,
    pub(super) peers: mpsc::UnboundedReceiver<cli_bridge_channel::Launch>,
}
pub(super) fn response(path: &std::path::Path, marker: &str, uuid: &str, text: &str) {
    std::fs::write(path.join(format!("answer-{marker}")),format!("{}\n",json!({"type":"assistant","session_id":uuid,"message":{"id":format!("message-{marker}"),"role":"assistant","content":[{"type":"text","text":text}]}}))).unwrap();
    let usage = if path.join("missing-usage").exists() {
        json!({"output_tokens":7})
    } else {
        json!({"input_tokens":0,"output_tokens":7,"cache_read_input_tokens":0,"cache_creation_input_tokens":0})
    };
    std::fs::write(path.join(format!("terminal-{marker}")),format!("{}\n",json!({"type":"result","session_id":uuid,"subtype":"success","is_error":false,"result":text,"usage":usage}))).unwrap();
}
pub(super) async fn setup(shared: &Arc<crate::runtime::Shared>) -> Ready {
    setup_variant(shared, "goal-runner").await
}
pub(super) async fn setup_variant(shared: &Arc<crate::runtime::Shared>, variant: &str) -> Ready {
    let owner = *shared.clients.lock().unwrap().keys().next().unwrap();
    let f = Fixture::new_in(variant, Some((shared.clone(), owner))).await;
    let uuid = f
        .launch
        .ownership
        .binding()
        .create
        .identity
        .cli_session_id
        .clone();
    let resources = shared
        .capture_cli_child_resources(
            "s",
            f.launch.assets.system_text(),
            f.launch.tools.clone(),
            f.launch.assets.image(),
            None,
        )
        .unwrap();
    let (send, peers) = mpsc::unbounded_channel();
    let mut resources = (*resources).clone();
    resources.peers = Some(send);
    response(f.dir.path(), &uuid, &uuid, "ready");
    let peer_path = f.dir.path().to_path_buf();
    let peer_uuid = uuid.clone();
    let peer = tokio::spawn(async move {
        // A real helper starts inside the launched CLI. Do not start its short
        // authentication deadline while root metadata/preflight is still running.
        tokio::time::timeout(Duration::from_secs(15), async {
            while !peer_path.join("launches").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        peer_marked(f.peer, peer_path, vec![], false, Some(peer_uuid)).await
    });
    assert_eq!(
        shared
            .spawn_cli_agent_root(f.launch)
            .await
            .unwrap()
            .unwrap()
            .state,
        cli_agent_turns::State::Completed
    );
    peer.await.unwrap();
    Ready {
        dir: f.dir,
        owner,
        uuid,
        resources: Arc::new(resources),
        peers,
    }
}
pub(super) async fn next(ready: &mut Ready) -> cli_bridge_channel::Launch {
    tokio::time::timeout(Duration::from_secs(15), ready.peers.recv())
        .await
        .unwrap()
        .unwrap()
}
pub(super) async fn worker(
    ready: &mut Ready,
    turn: u32,
    calls: Vec<(Value, bool, &'static str)>,
) -> tokio::task::JoinHandle<Vec<Value>> {
    let peer = next(ready).await;
    let marker = format!("{}-{turn}", ready.uuid);
    response(ready.dir.path(), &marker, &ready.uuid, "worker finished");
    tokio::spawn(peer_marked(
        peer,
        ready.dir.path().into(),
        calls,
        false,
        Some(marker),
    ))
}
fn propose(id: u64) -> Value {
    call(
        id,
        "propose_goal_completion",
        json!({"summary":"written proof","evidence":[{"kind":"file","reference":"proof.txt","sha256":crate::web::sha256_hex(b"proof")}]}),
    )
}
fn write() -> (Value, bool, &'static str) {
    (
        call(2, "write", json!({"path":"proof.txt","content":"proof"})),
        false,
        "",
    )
}
async fn finish(result: oneshot::Receiver<Result<GoalView, String>>) -> GoalView {
    tokio::time::timeout(Duration::from_secs(20), result)
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}
pub(super) fn configured(fx: &EngineFixture, p: &Scripted) -> crate::runtime::tests::Engine {
    let mut c = fx.config();
    c.enable_local_claude_cli = true;
    c.enable_local_claude_cli_orchestration = true;
    fx.start_config(p, c)
}
#[test]
fn goal_start_schedules_same_uuid_continuations_and_explicit_resume_with_evidence() {
    let fx = EngineFixture::new();
    let p = Scripted::echo("must not call API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let mut f = setup(&s).await;
        let run = s
            .start_cli_goal(
                f.owner,
                "s",
                Some("offline goal".into()),
                f.resources.clone(),
                2,
            )
            .unwrap();
        assert!(
            s.start_cli_goal(f.owner, "s", None, f.resources.clone(), 2)
                .is_err()
        );
        worker(
            &mut f,
            2,
            vec![(
                call(
                    2,
                    "update_goal",
                    json!({"status":"continue","note":"first progress"}),
                ),
                false,
                "",
            )],
        )
        .await
        .await
        .unwrap();
        worker(
            &mut f,
            3,
            vec![(
                call(
                    2,
                    "update_goal",
                    json!({"status":"blocked","reason":"need decision"}),
                ),
                false,
                "",
            )],
        )
        .await
        .await
        .unwrap();
        let g = finish(run).await;
        assert_eq!(g.state, GoalState::Blocked);
        assert_eq!(g.turns, 2);
        assert_eq!(g.usage_tokens, 14);
        let run = s
            .start_cli_goal(f.owner, "s", None, f.resources.clone(), 3)
            .unwrap();
        worker(
            &mut f,
            4,
            vec![write(), (propose(3), false, "Completion accepted")],
        )
        .await
        .await
        .unwrap();
        let g = finish(run).await;
        assert_eq!(g.state, GoalState::Completed);
        assert_eq!(g.turns, 3);
        assert_eq!(g.usage_tokens, 21);
        assert!(
            g.criteria[0]
                .evidence
                .iter()
                .any(|e| e.accepted == Some(true))
        );
        let identities = std::fs::read_to_string(f.dir.path().join("launch-identities")).unwrap();
        assert_eq!(identities.lines().count(), 4);
        assert!(
            identities
                .lines()
                .skip(1)
                .all(|l| l.starts_with(&format!("--resume {}", f.uuid)))
        );
        assert!(f.peers.try_recv().is_err());
        let restarted = crate::runtime::cli_agent_owner::Registry::new(true);
        restarted.connect(f.owner);
        assert!(restarted.goal_owner(&s.store, f.owner, "s").is_err());
    });
    assert!(p.requests().is_empty());
}
#[test]
fn goal_auditors_are_fresh_read_only_cli_children_and_all_failure_verdicts_pause() {
    for scenario in [
        "approved",
        "changes_requested",
        "inconclusive",
        "malformed",
        "stale",
        "api-model",
        "no-inspection",
    ] {
        let fx = EngineFixture::new();
        let p = Scripted::echo("must not audit via API");
        let e = configured(&fx, &p);
        let s = e.handle.shared().clone();
        s.handle.block_on(async {
            let mut f=setup(&s).await;
            super::goal_tests::seed(&s,true);
            s.store.tx(|c|{c.execute("UPDATE goals SET state='paused' WHERE session='s'",[])?;Ok(())}).unwrap();
            if scenario=="api-model" {std::fs::create_dir_all(fx.config().config_dir).unwrap();std::fs::write(fx.config().config_dir.join("settings.json"),json!({"goals":{"auditorModel":"openai/forbidden"}}).to_string()).unwrap();}
            let run=s.start_cli_goal(f.owner,"s",None,f.resources.clone(),2).unwrap();
            let mut calls=vec![write(),(propose(3),false,"")];
            if scenario=="approved" {calls.push((propose(3),false,""));}
            let root=worker(&mut f,2,calls).await;
            if scenario!="api-model" {
                let peer=next(&mut f).await;
                let task=s.store.tasks("s",10).unwrap().0.remove(0);
                let binding=s.store.cli_agent_binding(&task.child_session).unwrap().unwrap();
                let uuid=binding.create.identity.cli_session_id;
                assert_ne!(uuid,f.uuid);
                let (_,policy)=s.store.read(|c|Store::cli_native_child_policy_in(c,&task.child_session)).unwrap().unwrap();let policy:Value=serde_json::from_str(&policy).unwrap();
                assert_eq!(policy["context"],"fresh");assert_eq!(policy["cliNative"]["service"],"propose_goal_completion");assert_eq!(policy["tools"],json!(["read"]));
                let answer=if scenario=="malformed" {"not a verdict".into()} else {json!({"verdict":if matches!(scenario,"stale"|"no-inspection") {"approved"} else {scenario},"findings":[{"criterion":1,"ok":scenario=="approved","note":"read proof.txt"}]}).to_string()};
                response(f.dir.path(),&uuid,&uuid,&answer);
                if scenario=="stale" {std::fs::write(f.dir.path().join("proof.txt"),"changed").unwrap();}
                peer_marked(peer,f.dir.path().into(),if scenario=="no-inspection" {vec![]} else {vec![(call(2,"read",json!({"path":"proof.txt"})),false,"")]},false,Some(uuid)).await;
            }
            root.await.unwrap();
            if scenario=="changes_requested" {worker(&mut f,3,vec![(call(2,"update_goal",json!({"status":"blocked","reason":"review audit findings"})),false,"")]).await.await.unwrap();}
            let g=finish(run).await;
            assert_eq!(g.state,if scenario=="approved" {GoalState::Completed} else if scenario=="changes_requested" {GoalState::Blocked} else {GoalState::Paused},"{scenario}: {g:?}");
            let verdict=g.audit.result.as_ref().unwrap().verdict.as_str();assert_eq!(verdict,match scenario {"api-model"|"no-inspection"=>"failed","malformed"=>"inconclusive",other=>other});
            assert_eq!(g.usage_tokens,if matches!(scenario,"api-model"|"no-inspection") {7} else if scenario=="changes_requested" {21} else {14},"auditor must be charged once");
            assert_eq!(g.audit.rounds,1);assert!(f.peers.try_recv().is_err());
        });
        assert!(p.requests().is_empty());
    }
}
#[test]
fn goal_allowances_stall_ambiguous_reply_and_capacity_stop_never_replay() {
    for scenario in [
        "turns",
        "tokens",
        "stall",
        "ambiguous",
        "capacity-stop",
        "missing-usage",
        "cost",
    ] {
        let fx = EngineFixture::new();
        let p = Scripted::echo("no API");
        let e = configured(&fx, &p);
        let s = e.handle.shared().clone();
        s.handle.block_on(async {
            let mut f = setup(&s).await;
            super::goal_tests::seed(&s, false);
            s.store
                .tx(|c| {
                    let mut goal = Store::goal_current(c, "s")?.unwrap();
                    if scenario == "turns" {
                        goal.limits.max_turns = Some(1);
                    }
                    if matches!(scenario, "tokens" | "missing-usage") {
                        goal.limits.max_tokens = Some(7);
                    }
                    if scenario == "cost" {
                        goal.limits.max_cost_usd = Some(1.0);
                    }
                    Store::goal_write_progress(c, "s", &goal, now_ms())?;
                    Store::goal_set_state(c, "s", goal.id, GoalState::Paused, None, now_ms())
                })
                .unwrap();
            if scenario == "missing-usage" {
                std::fs::write(f.dir.path().join("missing-usage"), "").unwrap();
            }
            let permit = if scenario == "capacity-stop" {
                Some(
                    s.generations
                        .clone()
                        .acquire_many_owned(s.generations.available_permits() as u32)
                        .await
                        .unwrap(),
                )
            } else {
                None
            };
            let run = s
                .start_cli_goal(f.owner, "s", None, f.resources.clone(), 2)
                .unwrap();
            if scenario == "capacity-stop" {
                tokio::time::timeout(Duration::from_secs(5), async {
                    while !s.cli_agents.has_turn("s") {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                })
                .await
                .unwrap();
                assert_eq!(s.goal_view("s").unwrap().unwrap().turns, 0);
                s.cli_agents.stop("s");
                drop(permit);
            } else if scenario != "cost" {
                let turns = if scenario == "stall" { 3 } else { 1 };
                for turn in 2..2 + turns {
                    let calls = if scenario == "ambiguous" {
                        vec![]
                    } else {
                        vec![(
                            call(2, "update_goal", json!({"status":"continue"})),
                            false,
                            "",
                        )]
                    };
                    worker(&mut f, turn, calls).await.await.unwrap();
                }
            }
            let g = finish(run).await;
            assert_eq!(g.state, GoalState::Paused, "{scenario}");
            assert_eq!(
                g.turns,
                match scenario {
                    "stall" => 3,
                    "capacity-stop" | "cost" => 0,
                    _ => 1,
                }
            );
            assert!(f.peers.try_recv().is_err());
        });
        assert!(p.requests().is_empty());
    }
}

#[test]
fn goal_admission_is_atomic_stale_guarded_and_idempotent() {
    let fx = EngineFixture::new();
    let p = Scripted::echo("no API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let f = setup(&s).await;
        let g = s
            .store
            .tx(|tx| {
                Store::goal_insert(
                    tx,
                    "s",
                    "atomic goal",
                    GoalState::Active,
                    None,
                    "fixture",
                    "replaced",
                    now_ms(),
                )
            })
            .unwrap();
        let owner = s.cli_agents.goal_owner(&s.store, f.owner, "s").unwrap();
        let turn = owner
            .begin(
                &s.store,
                crate::store::cli_agent_turns::GoalTurn { view: g.clone() },
            )
            .unwrap();
        s.store
            .tx(|tx| {
                let mut changed = g.clone();
                changed.note = Some("person changed goal".into());
                Store::goal_write_progress(tx, "s", &changed, now_ms())
            })
            .unwrap();
        let request = crate::goals::request_key(g.id, 1);
        let text = crate::goals::turn_prompt_structured(&g.objective, &g.criteria);
        assert!(
            turn.admit(&s.store, &request, 2, "atomic-op", &text)
                .is_err()
        );
        drop(turn);
        let current = s.goal_view("s").unwrap().unwrap();
        assert_eq!(current.turns, 0);
        assert!(s.store.cli_agent_turn("s", &request).unwrap().is_none());
        let turn = owner
            .begin(
                &s.store,
                crate::store::cli_agent_turns::GoalTurn { view: current },
            )
            .unwrap();
        let admitted = turn
            .admit(&s.store, &request, 2, "atomic-op", &text)
            .unwrap();
        assert!(admitted.lease.is_some());
        let repeat = turn
            .admit(&s.store, &request, 2, "atomic-op", &text)
            .unwrap();
        assert!(repeat.lease.is_none());
        assert_eq!(repeat.receipt, admitted.receipt);
        assert_eq!(s.goal_view("s").unwrap().unwrap().turns, 1);
        s.store
            .abandon_cli_agent_turn(admitted.lease.as_ref().unwrap(), now_ms())
            .unwrap();
        drop(turn);
        drop(owner);
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
fn goal_lost_approval_before_admission_and_unknown_running_turn_cannot_resume() {
    for scenario in ["revoked", "stopped"] {
        let fx = EngineFixture::new();
        let p = Scripted::echo("no API");
        let e = configured(&fx, &p);
        let s = e.handle.shared().clone();
        s.handle.block_on(async {
            let mut f = setup(&s).await;
            let permit = if scenario == "revoked" {
                Some(
                    s.generations
                        .clone()
                        .acquire_many_owned(s.generations.available_permits() as u32)
                        .await
                        .unwrap(),
                )
            } else {
                None
            };
            let run = s
                .start_cli_goal(
                    f.owner,
                    "s",
                    Some("offline cancellation".into()),
                    f.resources.clone(),
                    2,
                )
                .unwrap();
            if scenario == "revoked" {
                tokio::time::timeout(Duration::from_secs(5), async {
                    while !s.cli_agents.has_turn("s") {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                })
                .await
                .unwrap();
                s.cli_agents.revoke_session("s");
                drop(permit);
            } else {
                let launch = next(&mut f).await;
                // Keep the process before any SDK terminal. The native peer confirms its
                // setup, then Stop must join it without treating channel EOF as success.
                let (ready, received) = oneshot::channel();
                let peer = tokio::spawn(super::child_tests::peer(
                    launch,
                    f.dir.path().into(),
                    format!("{}-2", f.uuid),
                    None,
                    true,
                    false,
                    Some(ready),
                ));
                received.await.unwrap();
                s.stop_by_backend("s").await.unwrap();
                peer.await.unwrap();
            }
            let g = finish(run).await;
            assert_eq!(g.state, GoalState::Paused);
            assert_eq!(g.turns, if scenario == "revoked" { 0 } else { 1 });
            assert!(
                s.start_cli_goal(f.owner, "s", None, f.resources.clone(), 3)
                    .is_err()
            );
            assert!(f.peers.try_recv().is_err());
        });
        assert!(p.requests().is_empty());
    }
}

#[test]
fn audit_changes_schedule_repair_and_a_second_independent_round() {
    let fx = EngineFixture::new();
    let p = Scripted::echo("no API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let mut f=setup(&s).await;super::goal_tests::seed(&s,true);
        s.store.tx(|c|{c.execute("UPDATE goals SET state='paused' WHERE session='s'",[])?;Ok(())}).unwrap();
        let run=s.start_cli_goal(f.owner,"s",None,f.resources.clone(),2).unwrap();
        let mut uuids=std::collections::HashSet::new();
        for round in 1..=2 {
            let root=worker(&mut f,round+1,vec![write(),(propose(3),false,"")]).await;
            let peer=next(&mut f).await;
            let task=s.store.tasks("s",10).unwrap().0.into_iter().max_by_key(|t|t.child).unwrap();
            let uuid=s.store.cli_agent_binding(&task.child_session).unwrap().unwrap().create.identity.cli_session_id;
            assert!(uuids.insert(uuid.clone()));assert_ne!(uuid,f.uuid);
            response(f.dir.path(),&uuid,&uuid,&json!({"verdict":if round==1 {"changes_requested"}else{"approved"},"findings":[{"criterion":1,"ok":round==2,"note":"inspect the proof again"}]}).to_string());
            peer_marked(peer,f.dir.path().into(),vec![(call(2,"read",json!({"path":"proof.txt"})),false,"")],false,Some(uuid)).await;
            root.await.unwrap();
        }
        let g=finish(run).await;assert_eq!(g.state,GoalState::Completed);assert_eq!(g.audit.rounds,2);assert_eq!(g.turns,2);assert_eq!(g.usage_tokens,28);
        let request=s.store.request("s",g.last_request.as_deref().unwrap()).unwrap().unwrap();assert!(request.text.contains("inspect the proof again"));
        assert_eq!(s.store.tasks("s",10).unwrap().0.len(),2);assert!(f.peers.try_recv().is_err());
    });
    assert!(p.requests().is_empty());
}

#[test]
fn stop_during_independent_audit_joins_the_child_and_cannot_complete_the_goal() {
    let fx = EngineFixture::new();
    let p = Scripted::echo("no API");
    let e = configured(&fx, &p);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let mut f=setup(&s).await;super::goal_tests::seed(&s,true);
        s.store.tx(|c|{c.execute("UPDATE goals SET state='paused' WHERE session='s'",[])?;Ok(())}).unwrap();
        std::fs::write(f.dir.path().join("proof.txt"),"proof").unwrap();
        let run=s.start_cli_goal(f.owner,"s",None,f.resources.clone(),2).unwrap();
        let launch=next(&mut f).await;
        let root=tokio::spawn(peer_marked(launch,f.dir.path().into(),vec![(propose(2),false,"")],true,Some(format!("{}-2",f.uuid))));
        let launch=next(&mut f).await;
        let deadline=Instant::now()+Duration::from_secs(15);
        let (mut read,mut write)=launch.connect(deadline).await.unwrap().split();
        for request in [json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":PROTOCOL_VERSION,"clientInfo":{"name":"fixture","version":"1"},"capabilities":{}}}),json!({"jsonrpc":"2.0","method":"notifications/initialized"}),call(2,"read",json!({"path":"proof.txt"}))] {
            write.write(&serde_json::to_vec(&request).unwrap(),deadline).await.unwrap();
            if request.get("id").is_some() {read.read(deadline).await.unwrap();}
        }
        s.stop_by_backend("s").await.unwrap();assert!(read.read(deadline).await.is_err());root.await.unwrap();
        let g=finish(run).await;assert_eq!(g.state,GoalState::Paused);assert_eq!(g.audit.result.unwrap().verdict,"stopped");
        tokio::time::timeout(Duration::from_secs(10),async {while !s.external_tasks.is_empty() {tokio::time::sleep(Duration::from_millis(10)).await;}}).await.unwrap();
        assert_eq!(s.store.tasks("s",10).unwrap().0[0].status,"failed");assert!(f.peers.try_recv().is_err());
    });
    assert!(p.requests().is_empty());
}

#[test]
fn goal_marker_fallback_is_scoped_to_its_turn_and_never_self_certifies_completion() {
    for marker in [
        crate::goals::MARK_CONTINUE,
        crate::goals::MARK_BLOCKED,
        crate::goals::MARK_UNREACHABLE,
        crate::goals::MARK_MET,
    ] {
        let fx = EngineFixture::new();
        let p = Scripted::echo("no API");
        let e = configured(&fx, &p);
        let s = e.handle.shared().clone();
        s.handle.block_on(async {
            let mut f = setup(&s).await;
            super::goal_tests::seed(&s, false);
            s.store
                .tx(|c| {
                    let mut goal = Store::goal_current(c, "s")?.unwrap();
                    goal.limits.max_turns = Some(1);
                    Store::goal_write_progress(c, "s", &goal, now_ms())?;
                    Store::goal_set_state(c, "s", goal.id, GoalState::Paused, None, now_ms())
                })
                .unwrap();
            let run = s
                .start_cli_goal(f.owner, "s", None, f.resources.clone(), 2)
                .unwrap();
            let peer = next(&mut f).await;
            let name = format!("{}-2", f.uuid);
            response(f.dir.path(), &name, &f.uuid, marker);
            peer_marked(peer, f.dir.path().into(), vec![], false, Some(name)).await;
            let g = finish(run).await;
            assert_eq!(g.turns, 1);
            assert_eq!(
                g.state,
                match marker {
                    crate::goals::MARK_BLOCKED => GoalState::Blocked,
                    crate::goals::MARK_UNREACHABLE => GoalState::Failed,
                    _ => GoalState::Paused,
                }
            );
            if marker == crate::goals::MARK_CONTINUE {
                s.store
                    .tx(|c| {
                        let mut goal = Store::goal_current(c, "s")?.unwrap();
                        goal.limits.max_turns = Some(3);
                        Store::goal_write_progress(c, "s", &goal, now_ms())
                    })
                    .unwrap();
                let run = s
                    .start_cli_goal(f.owner, "s", None, f.resources.clone(), 3)
                    .unwrap();
                let peer = next(&mut f).await;
                let name = format!("{}-3", f.uuid);
                response(f.dir.path(), &name, &f.uuid, "");
                peer_marked(peer, f.dir.path().into(), vec![], false, Some(name)).await;
                let g = finish(run).await;
                assert_eq!(g.turns, 2);
                assert_eq!(g.state, GoalState::Paused);
                assert_eq!(g.reason.as_deref(), Some(crate::goals::REASON_NO_STATUS));
            }
            assert!(f.peers.try_recv().is_err());
        });
        assert!(p.requests().is_empty());
    }
}
