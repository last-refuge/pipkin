//! Public dispatch to the real owner/actor, using fake CLI/MCP peers only.
use super::goal_lifecycle_tests::{configured, next, response, setup};
use super::service_tests::peer_marked;
use super::*;
use crate::{
    provider::scripted::Scripted,
    runtime::{dispatch_call, tests::Fixture as EngineFixture},
};
use pipkin_engine_protocol::*;

#[test]
fn desktop_orchestration_routes_prompt_resume_queue_stop_and_snapshot_without_api() {
    let fx = EngineFixture::new();
    let provider = Scripted::echo("never API");
    let e = configured(&fx, &provider);
    let s = e.handle.shared().clone();
    s.handle.block_on(async {
        let mut f = setup(&s).await;
        s.cli_desktop_test_resources
            .lock()
            .unwrap()
            .insert("s".into(), f.resources.clone());
        let cancel = CancellationToken::new();
        let call = Call::Prompt {
            session: "s".into(),
            request: "desktop-prompt".into(),
            text: "queued desktop turn".into(),
            images: vec![ImageInput {mime_type:"image/png".into(),data:"aW1hZ2U=".into()}],
            model: None,
        };
        let reply = dispatch_call(&s, f.owner, call.clone(), &cancel)
            .await
            .unwrap();
        assert!(matches!(reply, Reply::Admitted { run: Some(_), .. }));
        // Admission returns while fake CLI is waiting, not after the turn finishes.
        let peer = next(&mut f).await;
        let marker = format!("{}-2", f.uuid);
        let mut snapshot = s.snapshot("s", 20).unwrap();
        let mut filtered = Reply::Snapshot(Box::new(snapshot.clone()));
        s.filter_local_cli_readiness(f.owner, &mut filtered);
        assert!(matches!(filtered,Reply::Snapshot(v) if v.summary.blocked.is_none()));
        assert!(snapshot.run.is_some());
        assert!(snapshot.subagents.is_some());
        let q = dispatch_call(
            &s,
            f.owner,
            Call::Enqueue {
                session: "s".into(),
                request: "desktop-queue".into(),
                mode: InputMode::FollowUp,
                text: "queued turn".into(),
                images: vec![],
            },
            &cancel,
        )
        .await
        .unwrap();
        let Reply::Admitted {
            entry: Some(entry), ..
        } = q
        else {
            panic!()
        };
        snapshot = s.snapshot("s", 20).unwrap();
        assert_eq!(snapshot.queue.len(), 1);
        assert!(matches!(
            dispatch_call(
                &s,
                f.owner,
                Call::CancelQueued {
                    session: "s".into(),
                    entry
                },
                &cancel
            )
            .await
            .unwrap(),
            Reply::CancelQueued {
                outcome: CancelQueuedOutcome::Cancelled
            }
        ));
        response(f.dir.path(), &marker, &f.uuid, "desktop answer");
        peer_marked(peer, f.dir.path().into(), vec![], false, Some(marker)).await;
        tokio::time::timeout(Duration::from_secs(10), async {
            while s.cli_agents.has_turn("s") {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(matches!(
            dispatch_call(
                &s,
                f.owner,
                Call::Lookup {
                    session: "s".into(),
                    request: "desktop-prompt".into()
                },
                &cancel
            )
            .await
            .unwrap(),
            Reply::Lookup(LookupResult::Known { open: false, .. })
        ));
        // Public goal start acknowledges its durable admission before the scheduled worker.
        let goal = dispatch_call(
            &s,
            f.owner,
            Call::SetGoal {
                session: "s".into(),
                objective: "offline desktop goal".into(),
            },
            &cancel,
        )
        .await
        .unwrap();
        assert!(matches!(goal, Reply::Goal(Some(_))));
        let peer = next(&mut f).await;
        let marker = format!("{}-3", f.uuid);
        response(f.dir.path(), &marker, &f.uuid, "goal needs input");
        peer_marked(
            peer,
            f.dir.path().into(),
            vec![(
                super::service_tests::call(
                    2,
                    "update_goal",
                    json!({"status":"blocked","reason":"Need a decision"}),
                ),
                false,
                "",
            )],
            false,
            Some(marker),
        )
        .await;
        tokio::time::timeout(Duration::from_secs(10), async {
            while s.cli_agents.has_turn("s") {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let snapshot = s.snapshot("s", 20).unwrap();
        assert!(snapshot.goal.is_some());
        assert!(
            dispatch_call(
                &s,
                f.owner,
                Call::GoalControl {
                    session: "s".into(),
                    action: GoalAction::SetLimits {
                        max_turns: Some(3),
                        max_tokens: Some(100),
                        max_cost_usd: Some(1.0)
                    }
                },
                &cancel
            )
            .await
            .is_err()
        );
        dispatch_call(
            &s,
            f.owner,
            Call::GoalControl {
                session: "s".into(),
                action: GoalAction::SetLimits {
                    max_turns: Some(3),
                    max_tokens: Some(100),
                    max_cost_usd: None,
                },
            },
            &cancel,
        )
        .await
        .unwrap();
        let prepared_before_stop = s.cli_agents.input_epoch(f.owner, "s").unwrap();
        dispatch_call(
            &s,
            f.owner,
            Call::Stop {
                session: "s".into(),
            },
            &cancel,
        )
        .await
        .unwrap();
        // Stop during resource preparation cannot let older intent adopt the new epoch.
        assert!(s.enqueue_cli_input_during(
            crate::runtime::cli_agent_inputs::Input {
                owner: f.owner, session: "s".into(), request: "prepared-before-stop".into(),
                generation: 1, operation: "prepared-before-stop".into(),
                kind: crate::store::cli_agent_turns::inputs::Kind::FollowUp,
                text: "queued stale intent".into(), images: vec![],
            }, f.resources.clone(), Some(&prepared_before_stop)
        ).is_err());
        assert!(s.start_cli_goal_notified(f.owner, "s", Some("stale goal".into()),
            f.resources.clone(), 1, None, Some(&prepared_before_stop)).is_err());
        assert!(s.store.queue("s").unwrap().is_empty());
        assert_eq!(s.store.request("s","desktop-prompt").unwrap().unwrap().images.len(),1);
        for rounds in [Some(2), None] {
            dispatch_call(&s,f.owner,Call::GoalControl{session:"s".into(),action:GoalAction::SetAudit{required:true,max_rounds:rounds}},&cancel).await.unwrap();
        }
        assert_eq!(s.goal_view("s").unwrap().unwrap().audit.max_rounds,2);
        // A permanent uncertain receipt must not advertise Ready just because its old
        // connected approval still exists. This models the durable fence after cleanup.
        s.store.tx(|tx| {tx.execute("UPDATE cli_agent_turns SET fenced=1 WHERE session='s' AND request_key='desktop-prompt'",[])?; Ok(())}).unwrap();
        assert!(!s.cli_agents.approved(&s.store,f.owner,"s"));
        s.cli_agents.revoke_owner(f.owner);
        // A lost reply remains readable, but cannot produce a new launch or grant.
        assert!(matches!(
            dispatch_call(&s, f.owner, call, &cancel).await.unwrap(),
            Reply::Admitted { run: Some(_), .. }
        ));
        assert!(
            dispatch_call(
                &s,
                f.owner,
                Call::Prompt {
                    session: "s".into(),
                    request: "no-owner".into(),
                    text: "no".into(),
                    images: vec![],
                    model: None
                },
                &cancel
            )
            .await
            .is_err()
        );
        assert!(s.store.queue("s").unwrap().is_empty());
    });
    assert!(provider.requests().is_empty());
}
