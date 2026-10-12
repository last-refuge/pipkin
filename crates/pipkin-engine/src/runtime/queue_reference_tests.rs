use super::tests::{Engine, Fixture};
use crate::provider::scripted::{Gate, Reply as R, Scripted};
use crate::store::prompt_result::PromptState;
use pipkin_engine_protocol::*;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

fn project(m: &Message) -> Value {
    let role = match m {
        Message::System(_) => "system",
        Message::User { .. } => "user",
        Message::Assistant { .. } => "assistant",
        Message::ToolResult { .. } => "toolResult",
    };
    let value = serde_json::to_value(m).unwrap();
    let text = value["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|b| b["text"].as_str())
        .collect::<String>();
    json!([
        role,
        text,
        m.tool_calls()
            .iter()
            .map(|(_, name, _)| *name)
            .collect::<Vec<_>>()
    ])
}
fn requests(p: &Scripted) -> Value {
    json!(
        p.requests()
            .iter()
            .map(|r| r
                .messages
                .iter()
                .filter(|m| !matches!(m, Message::System(_)))
                .map(project)
                .collect::<Vec<_>>())
            .collect::<Vec<_>>()
    )
}
fn queue(e: &Engine, s: &str) -> Value {
    json!(e.handle.shared().store.queue(s).unwrap().iter().map(|r| json!({"key":r.key,"mode":match r.mode {InputMode::Steer=>"steer",_=>"followUp"}})).collect::<Vec<_>>())
}
fn idle(e: &Engine, s: &str) {
    let until = Instant::now() + Duration::from_secs(10);
    let mut since = None;
    loop {
        let shared = e.handle.shared();
        if shared.session(s).unwrap().active_run().is_none()
            && shared.store.open_run(s).unwrap().is_none()
        {
            if since.get_or_insert_with(Instant::now).elapsed() > Duration::from_millis(100) {
                return;
            }
        } else {
            since = None;
        }
        assert!(Instant::now() < until, "queue did not become idle");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn admit(e: &mut Engine, s: &str, key: &str, mode: &str) {
    if mode == "reject" {
        e.prompt(s, key, key).unwrap();
    } else {
        e.call(Call::Enqueue {
            session: s.into(),
            request: key.into(),
            mode: if mode == "steer" {
                InputMode::Steer
            } else {
                InputMode::FollowUp
            },
            text: key.into(),
            images: vec![],
        })
        .unwrap();
    }
}

#[test]
fn queue_boundaries_and_failure_recovery_match_executed_pi() {
    let reference: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/native-queue/runtime-reference.json"
    ))
    .unwrap();
    let mut failures = vec![];
    for case in reference["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let fx = Fixture::new();
        std::fs::create_dir_all(&fx.config().config_dir).unwrap();
        std::fs::write(fx.config().config_dir.join("settings.json"),json!({"retry":{"enabled":false},"compaction":{"enabled":false},"steeringMode":case.get("steeringMode").unwrap_or(&json!("one-at-a-time")),"followUpMode":case.get("followUpMode").unwrap_or(&json!("one-at-a-time"))}).to_string()).unwrap();
        let gate = Gate::new();
        let held = gate.clone();
        let first = case["first"].as_str().unwrap().to_owned();
        let provider = Scripted::new(move |req, n| {
            let reply = if n == 0 && first == "error" {
                R::Error("invalid request".into())
            } else if n == 0 && first == "tools" {
                R::Tools(None, vec![("not-installed".into(), json!({}))])
            } else {
                R::Text(format!(
                    "answer:{}",
                    req.messages
                        .iter()
                        .rev()
                        .find(|m| matches!(m, Message::User { .. }))
                        .unwrap()
                        .text()
                ))
            };
            if n == 0 {
                R::Gated(held.clone(), Box::new(reply))
            } else {
                reply
            }
        });
        let mut e = fx.start(&provider);
        let session = e.create(&fx.project, "c");
        e.prompt(&session, "main", "main").unwrap();
        e.until("initial model gate", |_| provider.requests().len() == 1);
        assert_eq!(
            e.prompt(&session, "busy-reject", "busy-reject")
                .unwrap_err()
                .code,
            ErrorCode::Busy
        );
        for q in case["queued"].as_array().unwrap() {
            admit(
                &mut e,
                &session,
                q["key"].as_str().unwrap(),
                q["mode"].as_str().unwrap(),
            );
        }
        if let Some(key) = case["withdraw"].as_str() {
            let entry = e
                .handle
                .shared()
                .store
                .request(&session, key)
                .unwrap()
                .unwrap()
                .queue_entry
                .unwrap();
            e.call(Call::CancelQueued {
                session: session.clone(),
                entry,
            })
            .unwrap();
        }
        assert_eq!(queue(&e, &session), case["before"], "{name}");
        gate.open();
        e.settled("main");
        idle(&e, &session);
        let phase = json!({"queue":queue(&e,&session),"requests":requests(&provider)});
        if phase != case["afterFirst"] {
            failures.push(format!("{name}: first boundary differs: {phase}"));
        }
        if case.get("wake").is_some() {
            // Native must preserve the failed-run queue through a real engine reopen too.
            let before = phase.clone();
            e.handle.shutdown(Duration::from_secs(5));
            drop(e);
            e = fx.start(&provider);
            e.snapshot(&session);
            idle(&e, &session);
            let after = json!({"queue":queue(&e,&session),"requests":requests(&provider)});
            if after != before {
                failures.push(format!("{name}: restart executed failed-run queue"));
            }
            admit(
                &mut e,
                &session,
                case["wake"]["key"].as_str().unwrap(),
                case["wake"]["mode"].as_str().unwrap(),
            );
            // A final boundary can group wake with an older steer. RunSettled names
            // the run's first request; each member has its own durable result.
            let shared = e.handle.shared().clone();
            e.until("wake result", |_| {
                matches!(
                    shared.store.prompt_result(&session, "wake").unwrap(),
                    PromptState::Settled(_)
                )
            });
            idle(&e, &session);
        }
        let history: Vec<_> = e
            .handle
            .shared()
            .store
            .page(&session, None, 1000)
            .unwrap()
            .0
            .iter()
            .filter_map(|entry| match &entry.kind {
                EntryKind::Message { message, .. } if !matches!(message, Message::System(_)) => {
                    Some(project(message))
                }
                _ => None,
            })
            .collect();
        let mut outcomes = json!({});
        for key in case["expected"]["outcomes"].as_object().unwrap().keys() {
            outcomes[key] = match e
                .handle
                .shared()
                .store
                .prompt_result(&session, key)
                .unwrap()
            {
                PromptState::Settled(PromptResult::Done { text, .. }) => {
                    json!({"status":"done","reason":null,"answer":text})
                }
                PromptState::Settled(PromptResult::Unanswered { reason, .. }) => {
                    json!({"status":"unanswered","reason":reason,"answer":null})
                }
                PromptState::Pending => json!({"status":"queued","reason":null,"answer":null}),
                PromptState::Unknown => json!({"status":"unknown"}),
            };
        }
        let actual = json!({"requests":requests(&provider),"queue":queue(&e,&session),"history":history,"outcomes":outcomes});
        if actual != case["expected"] {
            failures.push(format!("{name}: final trace/history/settlements differ"));
            std::fs::write(
                format!("/tmp/pipkin-queue-{name}.json"),
                serde_json::to_string_pretty(&json!({"actual":actual,"expected":case["expected"]}))
                    .unwrap(),
            )
            .unwrap();
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn failed_queue_prompt_preserves_capacity_order_and_idempotency() {
    let fx = Fixture::new();
    std::fs::create_dir_all(&fx.config().config_dir).unwrap();
    std::fs::write(
        fx.config().config_dir.join("settings.json"),
        r#"{"retry":{"enabled":false},"compaction":{"enabled":false}}"#,
    )
    .unwrap();
    let failure = Gate::new();
    let resumed = Gate::new();
    let first = failure.clone();
    let second = resumed.clone();
    let provider = Scripted::new(move |_, n| match n {
        0 => R::Gated(first.clone(), Box::new(R::Error("invalid request".into()))),
        1 => R::Gated(second.clone(), Box::new(R::Text("resumed".into()))),
        _ => R::Text("done".into()),
    });
    let mut e = fx.start(&provider);
    let session = e.create(&fx.project, "c");
    e.prompt(&session, "main", "main").unwrap();
    e.until("first request", |_| provider.requests().len() == 1);
    let capacity = limits::MAX_QUEUE;
    for i in 0..capacity {
        admit(&mut e, &session, &format!("f{i}"), "followUp");
    }
    failure.open();
    assert!(matches!(e.settled("main"), RunOutcome::ModelError { .. }));
    idle(&e, &session);
    assert_eq!(provider.requests().len(), 1);
    assert_eq!(
        e.prompt(&session, "wake", "wake").unwrap_err().code,
        ErrorCode::TooLarge
    );
    assert!(
        e.handle
            .shared()
            .store
            .request(&session, "wake")
            .unwrap()
            .is_none()
    );
    assert_eq!(queue(&e, &session).as_array().unwrap().len(), capacity);

    let withdrawn = e
        .handle
        .shared()
        .store
        .request(&session, "f0")
        .unwrap()
        .unwrap()
        .queue_entry
        .unwrap();
    e.call(Call::CancelQueued {
        session: session.clone(),
        entry: withdrawn,
    })
    .unwrap();
    let admitted = e.prompt(&session, "wake", "wake").unwrap();
    assert!(matches!(
        admitted,
        Reply::Admitted {
            run: None,
            entry: Some(_)
        }
    ));
    e.until("resumed request gate", |_| provider.requests().len() == 2);
    let snapshot = queue(&e, &session);
    assert_eq!(
        snapshot.as_array().unwrap().last().unwrap(),
        &json!({"key":"wake","mode":"followUp"})
    );
    let replay = e.prompt(&session, "wake", "wake").unwrap();
    assert_eq!(
        serde_json::to_value(admitted).unwrap(),
        serde_json::to_value(replay).unwrap()
    );
    assert_eq!(
        e.prompt(&session, "wake", "changed").unwrap_err().code,
        ErrorCode::Conflict
    );
    assert_eq!(
        e.call(Call::Enqueue {
            session: session.clone(),
            request: "wake".into(),
            mode: InputMode::FollowUp,
            text: "wake".into(),
            images: vec![],
        })
        .unwrap_err()
        .code,
        ErrorCode::Conflict
    );
    assert_eq!(queue(&e, &session), snapshot);
    assert_eq!(provider.requests().len(), 2);
    resumed.open();
    e.settled("wake");
    idle(&e, &session);
    let seen: Vec<_> = provider
        .requests()
        .iter()
        .map(|r| {
            r.messages
                .iter()
                .rev()
                .find(|m| matches!(m, Message::User { .. }))
                .unwrap()
                .text()
        })
        .collect();
    let expected: Vec<_> = std::iter::once("main".to_owned())
        .chain((1..capacity).map(|i| format!("f{i}")))
        .chain(std::iter::once("wake".to_owned()))
        .collect();
    assert_eq!(seen, expected);
    assert_eq!(queue(&e, &session), json!([]));
    e.handle.shutdown(Duration::from_secs(5));
    drop(e);
    let mut e = fx.start(&provider);
    e.prompt(&session, "wake", "wake").unwrap();
    idle(&e, &session);
    assert_eq!(provider.requests().len(), capacity + 1);
    assert!(matches!(
        e.handle
            .shared()
            .store
            .prompt_result(&session, "wake")
            .unwrap(),
        PromptState::Settled(PromptResult::Done { .. })
    ));
}
