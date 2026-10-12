use super::tests::Fixture;
use crate::provider::scripted::{Gate, Reply as R, Scripted};
use pipkin_engine_protocol::*;
use serde_json::json;
use std::time::{Duration, Instant};

fn wait(mut condition: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < end, "expected runtime progress");
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn child(req: &crate::provider::ProviderRequest) -> bool {
    req.tools.iter().all(|t| t.name != "subagent")
}

#[test]
fn saturated_parent_generations_release_capacity_for_children_and_resume() {
    let fx = Fixture::new();
    let parents = Gate::new();
    let children = Gate::new();
    let (p, c) = (parents.clone(), children.clone());
    let provider = Scripted::new(move |req, _| {
        if child(req) {
            return R::Gated(c.clone(), Box::new(R::Text("child completed".into())));
        }
        if req
            .messages
            .iter()
            .any(|m| matches!(m, Message::ToolResult { .. }))
        {
            return R::Text("parent completed".into());
        }
        R::Gated(
            p.clone(),
            Box::new(R::Tools(
                None,
                vec![("subagent".into(), json!({"task":"do child work"}))],
            )),
        )
    });
    let mut cfg = fx.config();
    cfg.max_active_generations = 2;
    let mut e = fx.start_config(&provider, cfg);
    for i in 0..3 {
        let session = e.create(&fx.project, &format!("parent-{i}"));
        e.prompt(&session, &format!("run-{i}"), "delegate").unwrap();
    }
    wait(|| provider.requests().len() == 2);
    assert_eq!(e.handle.shared().generations.available_permits(), 0);
    assert!(provider.requests().iter().all(|r| !child(r)));
    parents.open();
    wait(|| provider.requests().iter().filter(|r| child(r)).count() >= 2);
    assert_eq!(e.handle.shared().generations.available_permits(), 0);
    children.open();
    for i in 0..3 {
        assert_eq!(e.settled(&format!("run-{i}")), RunOutcome::Completed);
    }
    assert_eq!(
        provider.requests().len(),
        9,
        "each parent has two generations and each child one"
    );
    wait(|| e.handle.shared().generations.available_permits() == 2);
    e.handle.shutdown(Duration::from_secs(1));
}

#[test]
fn root_usage_includes_running_children_and_survives_replay_and_restart_without_double_counting() {
    let fx = Fixture::new();
    let finished = Gate::new();
    let gate = finished.clone();
    let provider = Scripted::new(move |req, _| {
        let has_result = req
            .messages
            .iter()
            .any(|m| matches!(m, Message::ToolResult { .. }));
        if child(req) {
            if has_result {
                return R::Gated(gate.clone(), Box::new(R::Text("child done".into())));
            }
            return R::Tools(
                None,
                vec![("bash".into(), json!({"command":"printf child-output"}))],
            );
        }
        if has_result {
            return R::Text("parent done".into());
        }
        R::Tools(
            None,
            vec![
                ("subagent".into(), json!({"task":"first child"})),
                ("subagent".into(), json!({"task":"second child"})),
            ],
        )
    });
    let mut e = fx.start(&provider);
    let session = e.create(&fx.project, "usage-root");
    e.prompt(&session, "delegate", "run two children").unwrap();
    wait(|| provider.requests().iter().filter(|r| child(r)).count() == 4);
    let ongoing = e.snapshot(&session);
    assert_eq!(
        ongoing.usage.models[0].1.total_tokens, 45,
        "parent request and both committed child tool requests"
    );
    assert!(e.events.iter().any(|event|matches!(event,Event::Session{session:s,change:SessionChange::UsageChanged{usage},..} if s==&session && usage.models[0].1.total_tokens==45)),"root gets live child usage updates");
    finished.open();
    assert_eq!(e.settled("delegate"), RunOutcome::Completed);
    let expected = e.snapshot(&session).usage;
    assert_eq!(expected.models.len(), 1);
    assert_eq!(expected.models[0].0, "scripted/scripted");
    assert_eq!(
        (
            expected.models[0].1.input,
            expected.models[0].1.output,
            expected.models[0].1.total_tokens
        ),
        (60, 30, 90)
    );
    assert!(
        expected.tools.is_empty(),
        "child model usage keeps its model identity"
    );
    e.prompt(&session, "delegate", "run two children").unwrap();
    assert_eq!(e.snapshot(&session).usage, expected);
    assert_eq!(provider.requests().len(), 6);
    e.handle.shutdown(Duration::from_secs(1));
    drop(e);
    let resumed = Scripted::echo("another root");
    let mut e = fx.start(&resumed);
    assert_eq!(e.snapshot(&session).usage, expected);
    e.prompt(&session, "delegate", "run two children").unwrap();
    assert!(
        resumed.requests().is_empty(),
        "durable replay must not execute a second time"
    );
    let other = e.create(&fx.project, "unrelated");
    e.prompt(&other, "other", "new root").unwrap();
    assert_eq!(e.settled("other"), RunOutcome::Completed);
    assert_eq!(e.snapshot(&other).usage.models[0].1.total_tokens, 15);
    assert_eq!(e.snapshot(&session).usage, expected);
    e.handle.shutdown(Duration::from_secs(1));
}

#[test]
fn ancestor_usage_invalidation_failure_rolls_back_child_spend_and_recovers_once() {
    let fx = Fixture::new();
    let gate = Gate::new();
    let held = gate.clone();
    let provider = Scripted::new(move |req, _| {
        if child(req) {
            return R::Gated(held.clone(), Box::new(R::Text("child billed once".into())));
        }
        if req
            .messages
            .iter()
            .any(|m| matches!(m, Message::ToolResult { .. }))
        {
            return R::Text("parent done".into());
        }
        R::Tools(
            None,
            vec![("subagent".into(), json!({"task":"usage rollback"}))],
        )
    });
    let mut e = fx.start(&provider);
    let root = e.create(&fx.project, "failure-root");
    e.prompt(&root, "rollback", "delegate").unwrap();
    wait(|| provider.requests().iter().any(child));
    let child_id: String = e
        .handle
        .shared()
        .store
        .read(|c| {
            Ok(c.query_row(
                "SELECT child_session FROM subagent_tasks WHERE root = ?1",
                [&root],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    e.handle.shared().store.read(|c| {
        c.execute_batch(&format!("CREATE TEMP TRIGGER fail_ancestor_usage BEFORE UPDATE OF revision ON sessions WHEN NEW.id = '{}' BEGIN SELECT RAISE(ABORT, 'ancestor usage invalidation failure'); END;",root.replace('\'',"''")))?;
        Ok(())
    }).unwrap();
    gate.open();
    wait(|| {
        e.handle
            .shared()
            .session(&child_id)
            .unwrap()
            .failure()
            .is_some()
    });
    assert!(
        e.handle
            .shared()
            .store
            .usage(&child_id)
            .unwrap()
            .models
            .is_empty()
    );
    assert_eq!(
        e.handle.shared().store.usage(&root).unwrap().models[0]
            .1
            .total_tokens,
        15
    );
    e.handle.shutdown(Duration::from_secs(1));
    drop(e);
    let resumed = Scripted::new(|req, _| {
        R::Text(
            if child(req) {
                "child recovered"
            } else {
                "parent recovered"
            }
            .into(),
        )
    });
    let mut e = fx.start(&resumed);
    assert_eq!(e.settled("rollback"), RunOutcome::Completed);
    assert_eq!(e.snapshot(&root).usage.models[0].1.total_tokens, 45);
    assert_eq!(
        e.handle.shared().store.usage(&child_id).unwrap().models[0]
            .1
            .total_tokens,
        15
    );
    assert_eq!(resumed.requests().len(), 2);
    e.handle.shutdown(Duration::from_secs(1));
}
