use super::*;
use crate::host::Connection;
use crate::provider::scripted::{Gate, Reply as R, Scripted};
use crate::runtime::tests::{Engine, Fixture};
use pipkin_engine_protocol::*;
use std::time::{Duration, Instant};

fn send(conn: &Connection, id: u64, session: &str, request: &str) {
    conn.send(ClientFrame {
        id,
        lane: Lane::Normal,
        call: Call::WaitForPrompt {
            session: session.into(),
            request: request.into(),
        },
    })
    .unwrap();
}
fn reply(conn: &Connection, id: u64) -> Result<Reply, ProtoError> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match conn
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("wait reply")
        {
            ServerFrame::Reply { id: got, result } if got == id => return result,
            _ => {}
        }
    }
}
fn wait(e: &mut Engine, session: &str, request: &str) -> Result<Reply, ProtoError> {
    e.call(Call::WaitForPrompt {
        session: session.into(),
        request: request.into(),
    })
}
fn done(text: &str) -> Reply {
    Reply::PromptResult(PromptResult::Done {
        text: text.into(),
        reason: None,
    })
}
fn unanswered(reason: &str) -> Reply {
    Reply::PromptResult(PromptResult::Unanswered {
        text: None,
        reason: reason.into(),
    })
}
fn waits(e: &Engine, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while e.handle.shared().prompt_waits.available_permits() != 64 - count {
        assert!(Instant::now() < deadline, "waiter capacity {count}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn waits_for_its_final_answer_and_keeps_it_after_later_turns_and_restart() {
    let fx = Fixture::new();
    let gate = Gate::new();
    let g = gate.clone();
    let provider = Scripted::new(move |_, n| match n {
        0 => R::Tools(
            Some("intermediate text".into()),
            vec![(
                "write".into(),
                serde_json::json!({"path":"wait.txt","content":"once"}),
            )],
        ),
        1 => R::Gated(
            g.clone(),
            Box::new(R::Thinking(
                "private reasoning".into(),
                "final answer".into(),
            )),
        ),
        _ => R::Text("later answer".into()),
    });
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "s");
    e.prompt(&s, "first", "code").unwrap();
    let conn = e.handle.connect("waiter");
    send(&conn, 1, &s, "first");
    waits(&e, 1);
    gate.open();
    assert_eq!(reply(&conn, 1).unwrap(), done("final answer"));
    e.prompt(&s, "second", "later").unwrap();
    assert_eq!(wait(&mut e, &s, "second").unwrap(), done("later answer"));
    assert_eq!(wait(&mut e, &s, "first").unwrap(), done("final answer"));
    drop(conn);
    drop(e);
    let mut e = fx.start(&provider);
    assert_eq!(wait(&mut e, &s, "first").unwrap(), done("final answer"));
    assert_eq!(
        std::fs::read_to_string(fx.project.join("wait.txt")).unwrap(),
        "once"
    );
    assert_eq!(provider.requests().len(), 3);
}

#[test]
fn withdrawn_queued_inputs_and_stop_wake_their_own_waiters() {
    let fx = Fixture::new();
    let mut e = fx.start(&Scripted::new(|_, _| R::Hang));
    let s = e.create(&fx.project, "s");
    e.prompt(&s, "active", "hold").unwrap();
    let queued = e
        .call(Call::Enqueue {
            session: s.clone(),
            request: "queued".into(),
            mode: InputMode::FollowUp,
            text: "later".into(),
            images: vec![],
        })
        .unwrap();
    let Reply::Admitted {
        entry: Some(entry), ..
    } = queued
    else {
        panic!("{queued:?}")
    };
    let a = e.handle.connect("active wait");
    let b = e.handle.connect("queued wait");
    send(&a, 1, &s, "active");
    send(&b, 1, &s, "queued");
    waits(&e, 2);
    e.call(Call::CancelQueued {
        session: s.clone(),
        entry,
    })
    .unwrap();
    assert_eq!(reply(&b, 1).unwrap(), unanswered("aborted"));
    waits(&e, 1);
    e.call(Call::Stop { session: s.clone() }).unwrap();
    assert_eq!(reply(&a, 1).unwrap(), unanswered("aborted"));
    assert_eq!(wait(&mut e, &s, "queued").unwrap(), unanswered("aborted"));
}

#[test]
fn unknown_and_foreign_keys_refuse_and_model_failures_return_reference_reasons() {
    let fx = Fixture::new();
    let mut e = fx.start(&Scripted::new(|_, _| {
        R::Error("401: invalid api key".into())
    }));
    let s = e.create(&fx.project, "s");
    let other = e.create(&fx.project, "other");
    e.prompt(&s, "known", "fail").unwrap();
    assert_eq!(
        wait(&mut e, &s, "known").unwrap(),
        unanswered("model_error")
    );
    for key in ["known", "missing"] {
        let error = wait(&mut e, &other, key).unwrap_err();
        assert_eq!(error.code, ErrorCode::NotFound);
        assert_eq!(error.message, format!("Unknown prompt: {key}"));
    }
    drop(e);
    let fx = Fixture::new();
    let mut config = fx.config();
    config.default_model = None;
    config.extra_models.clear();
    let mut e = fx.start_config(&Scripted::echo("unused"), config);
    let s = e.create(&fx.project, "s");
    e.prompt(&s, "none", "fail").unwrap();
    assert_eq!(wait(&mut e, &s, "none").unwrap(), unanswered("no_model"));
}

#[test]
fn full_wait_capacity_does_not_block_normal_calls_and_disconnect_releases_it() {
    let fx = Fixture::new();
    let provider = Scripted::new(|_, _| R::Hang);
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "s");
    e.prompt(&s, "active", "hold").unwrap();
    let conn = e.handle.connect("many waits");
    for n in 1..=64 {
        send(&conn, n, &s, "active");
    }
    waits(&e, 64);
    // A normal call on the SAME connection must still run with every wait slot occupied.
    conn.send(ClientFrame {
        id: 100,
        lane: Lane::Normal,
        call: Call::Lookup {
            session: s.clone(),
            request: "active".into(),
        },
    })
    .unwrap();
    assert!(matches!(
        reply(&conn, 100),
        Ok(Reply::Lookup(LookupResult::Known { open: true, .. }))
    ));
    assert_eq!(
        wait(&mut e, &s, "active").unwrap_err().code,
        ErrorCode::Busy
    );
    drop(conn);
    waits(&e, 0);
    assert!(matches!(
        e.call(Call::Lookup {
            session: s.clone(),
            request: "active".into()
        })
        .unwrap(),
        Reply::Lookup(LookupResult::Known { open: true, .. })
    ));
    e.call(Call::Stop { session: s.clone() }).unwrap();
    assert_eq!(wait(&mut e, &s, "active").unwrap(), unanswered("aborted"));
}

#[test]
fn storage_failure_ends_wait_without_inventing_a_settlement_then_reopen_recovers() {
    let fx = Fixture::new();
    let gate = Gate::new();
    let g = gate.clone();
    let provider =
        Scripted::new(move |_, _| R::Gated(g.clone(), Box::new(R::Text("saved".into()))));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "s");
    e.prompt(&s, "active", "hold").unwrap();
    let conn = e.handle.connect("wait");
    send(&conn, 1, &s, "active");
    waits(&e, 1);
    // Wait for the provider's checkpoint before injecting the next stream write fault.
    let deadline = Instant::now() + Duration::from_secs(5);
    while provider.requests().is_empty() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    e.handle.shared().store.fail_next_write();
    gate.open();
    assert_eq!(reply(&conn, 1).unwrap_err().code, ErrorCode::Storage);
    assert!(matches!(
        e.call(Call::Lookup {
            session: s.clone(),
            request: "active".into()
        })
        .unwrap(),
        Reply::Lookup(LookupResult::Known { open: true, .. })
    ));
    // Engine restart resumes committed work; the same key remains waitable.
    drop(conn);
    drop(e);
    let mut e = fx.start(&provider);
    assert_eq!(wait(&mut e, &s, "active").unwrap(), done("saved"));
}

#[test]
fn settlement_registration_races_never_lose_a_reply() {
    let fx = Fixture::new();
    let mut e = fx.start(&Scripted::echo("fast"));
    let s = e.create(&fx.project, "s");
    for n in 0..32 {
        let key = format!("fast-{n}");
        e.prompt(&s, &key, "x").unwrap();
        assert_eq!(wait(&mut e, &s, &key).unwrap(), done("fast"));
    }
}

#[test]
fn preserved_submission_results_match_the_executed_pi_service_projection() {
    let reference: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../fixtures/pi-prompt-waits/results.json"
    ))
    .unwrap();
    let fx = Fixture::new();
    let mut e = fx.start(&Scripted::echo("unused"));
    for case in reference["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let s = e.create(&fx.project, name);
        let operation = case["operationId"].as_str().unwrap_or("1");
        if case.get("record").is_some() {
            let mut record = case["record"].clone();
            record["requestId"] = operation.into();
            e.handle
                .shared()
                .store
                .tx(|tx| {
                    crate::store::Store::admit(
                        tx,
                        &s,
                        operation,
                        InputMode::Prompt,
                        "input",
                        &[],
                        None,
                        1,
                    )?;
                    if let Some(data) = case.get("entry") {
                        let entry = crate::store::Store::append_entry(
                            tx,
                            &s,
                            1,
                            None,
                            &EntryKind::Custom {
                                custom_type: "pi.reference".into(),
                                data: data.clone(),
                            },
                        )?;
                        record["answer"] = entry.id.into();
                    }
                    crate::store::Store::settle_request(
                        tx,
                        &s,
                        operation,
                        &RunOutcome::Completed,
                        1,
                    )?;
                    crate::store::Store::kv_set(
                        tx,
                        &s,
                        "pi.submissions",
                        &serde_json::json!([record]).to_string(),
                    )
                })
                .unwrap();
        }
        match wait(&mut e, &s, operation) {
            Ok(Reply::PromptResult(result)) => assert_eq!(
                serde_json::to_value(result).unwrap(),
                case["result"],
                "{name}"
            ),
            Err(error) => assert_eq!(error.message, case["error"].as_str().unwrap(), "{name}"),
            other => panic!("{name}: {other:?}"),
        }
    }
}

#[test]
fn queued_steer_and_follow_up_wait_for_the_run_that_consumes_them() {
    let fx = Fixture::new();
    let gate = Gate::new();
    let g = gate.clone();
    let provider = Scripted::new(move |_, n| match n {
        0 => R::Gated(
            g.clone(),
            Box::new(R::Tools(
                None,
                vec![(
                    "write".into(),
                    serde_json::json!({"path":"steered.txt","content":"yes"}),
                )],
            )),
        ),
        1 => R::Text("steered answer".into()),
        _ => R::Text("follow up answer".into()),
    });
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "s");
    e.prompt(&s, "prompt", "hold").unwrap();
    for (key, mode) in [("steer", InputMode::Steer), ("follow", InputMode::FollowUp)] {
        e.call(Call::Enqueue {
            session: s.clone(),
            request: key.into(),
            mode,
            text: key.into(),
            images: vec![],
        })
        .unwrap();
    }
    let conn = e.handle.connect("queued wait");
    send(&conn, 1, &s, "steer");
    waits(&e, 1);
    gate.open();
    assert_eq!(reply(&conn, 1).unwrap(), done("steered answer"));
    assert_eq!(wait(&mut e, &s, "prompt").unwrap(), done("steered answer"));
    assert_eq!(
        wait(&mut e, &s, "follow").unwrap(),
        done("follow up answer")
    );
}

#[test]
fn dropping_a_rust_extension_wait_and_engine_shutdown_release_capacity() {
    use crate::extensions::{CallHost, ExtensionHost};
    let fx = Fixture::new();
    let mut e = fx.start(&Scripted::new(|_, _| R::Hang));
    let s = e.create(&fx.project, "s");
    e.snapshot(&s);
    let shared = e.handle.shared();
    let sess = shared.session(&s).unwrap();
    let host = CallHost::new(
        s.clone(),
        sess.questions.clone(),
        shared.extension_dispatch(&sess),
    );
    let admitted = shared
        .handle
        .block_on(host.prompt("hold".into(), None))
        .unwrap();
    assert!(admitted.operation_id.starts_with("ext-"));
    let pending = shared
        .handle
        .spawn(host.wait_for_prompt(admitted.operation_id.clone()));
    waits(&e, 1);
    pending.abort();
    waits(&e, 0);
    let pending = shared
        .handle
        .spawn(host.wait_for_prompt(admitted.operation_id.clone()));
    waits(&e, 1);
    shared.stopping.cancel();
    assert_eq!(
        shared.handle.block_on(pending).unwrap().unwrap_err().code,
        ErrorCode::ShuttingDown
    );
    waits(&e, 0);
    assert!(matches!(
        e.call(Call::Lookup {
            session: s,
            request: admitted.operation_id
        })
        .unwrap(),
        Reply::Lookup(LookupResult::Known { open: true, .. })
    ));
}

#[cfg(unix)]
#[test]
fn a_process_extension_can_cancel_its_wait_without_cancelling_the_input() {
    let fx = Fixture::new();
    let mut config = fx.config();
    config.extensions = vec![ExtensionSpec::Process { id:"wait-test".into(), argv:vec![
        "/bin/sh".into(), "-c".into(), r#"
IFS= read -r line
printf '%s\n' '{"type":"hello","api":1}'
IFS= read -r line
printf '%s\n' '{"id":1,"method":"prompt","params":{"text":"hold","requestKey":"from-process"}}'
IFS= read -r line
case "$line" in *'"operationId":"from-process"'*) ;; *) exit 2;; esac
printf '%s\n' '{"id":2,"method":"waitForPrompt","params":{"operationId":"from-process"}}' '{"type":"cancel","id":2}'
IFS= read -r line
case "$line" in *'"code":"cancelled"'*) ;; *) exit 3;; esac
printf '%s\n' '{"id":3,"method":"notify","params":{"level":"info","message":"wait cancelled only"}}'
IFS= read -r line
printf '%s\n' '{"id":4,"method":"waitForPrompt","params":{"operationId":"from-process"}}'
IFS= read -r line
case "$line" in *'"reason":"aborted"'*) ;; *) exit 4;; esac
printf '%s\n' '{"id":5,"method":"notify","params":{"level":"info","message":"input settled"}}'
IFS= read -r line
"#.into()] }];
    let mut e = fx.start_config(&Scripted::new(|_, _| R::Hang), config);
    let s = e.create(&fx.project, "s");
    e.snapshot(&s);
    e.until("process cancelled its wait", |events| {
        events.iter().any(|event| {
            matches!(event,
        Event::Session { change: SessionChange::UiChanged { ui }, .. }
        if ui.notices.iter().any(|n| n.message == "wait cancelled only"))
        })
    });
    assert!(matches!(
        e.call(Call::Lookup {
            session: s.clone(),
            request: "from-process".into()
        })
        .unwrap(),
        Reply::Lookup(LookupResult::Known { open: true, .. })
    ));
    e.call(Call::Stop { session: s }).unwrap();
    e.until("process sees settlement", |events| {
        events.iter().any(|event| {
            matches!(event,
        Event::Session { change: SessionChange::UiChanged { ui }, .. }
        if ui.notices.iter().any(|n| n.message == "input settled"))
        })
    });
}

#[test]
fn unread_wait_replies_backpressure_admission_and_disconnect_drains_tasks() {
    let fx = Fixture::new();
    let mut e = fx.start(&Scripted::echo("unused"));
    let s = e.create(&fx.project, "s");
    let conn = e.handle.connect("unread replies");
    let client = conn.client_id();
    let deadline = Instant::now() + Duration::from_millis(300);
    let mut accepted = 0;
    while Instant::now() < deadline {
        let frame = ClientFrame {
            id: accepted + 1,
            lane: Lane::Normal,
            call: Call::WaitForPrompt {
                session: s.clone(),
                request: "unknown".into(),
            },
        };
        if conn.send(frame).is_ok() {
            accepted += 1;
            // 64 queued calls, 16 normal tasks, 64 wait/reply tasks, 16 buffered replies.
            assert!(accepted <= 160, "unread replies admitted {accepted} calls");
        } else {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    assert!(accepted > 64, "engine must have processed requests");
    assert!(matches!(
        e.call(Call::ListSessions),
        Ok(Reply::Sessions { .. })
    ));
    drop(conn);
    let deadline = Instant::now() + Duration::from_secs(5);
    while e
        .handle
        .shared()
        .clients
        .lock()
        .unwrap()
        .contains_key(&client)
    {
        assert!(Instant::now() < deadline, "disconnected wait tasks drained");
        std::thread::sleep(Duration::from_millis(2));
    }
    waits(&e, 0);
}

#[test]
fn restart_schedules_a_queue_left_between_settlement_and_the_next_run() {
    let fx = Fixture::new();
    let provider = Scripted::echo("answer");
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "s");
    e.prompt(&s, "first", "first").unwrap();
    assert_eq!(wait(&mut e, &s, "first").unwrap(), done("answer"));
    drop(e);
    // Reconstruct the committed crash boundary while no engine can schedule it: the
    // previous run is settled, one input remains queued, and there is no open run.
    {
        let store = crate::store::Store::open(&fx.config().data_dir).unwrap();
        store
            .tx(|tx| {
                crate::store::Store::admit(
                    tx,
                    &s,
                    "after-crash",
                    InputMode::FollowUp,
                    "later",
                    &[],
                    None,
                    1,
                )
            })
            .unwrap();
        assert!(store.open_run(&s).unwrap().is_none());
    }
    let mut e = fx.start(&provider);
    // Recovery makes progress before any client opens the session or asks to wait.
    let deadline = Instant::now() + Duration::from_secs(5);
    while provider.requests().len() != 2 {
        assert!(
            Instant::now() < deadline,
            "queued recovery starts without an open run"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(wait(&mut e, &s, "after-crash").unwrap(), done("answer"));
    drop(e);
    let mut e = fx.start(&provider);
    assert_eq!(wait(&mut e, &s, "after-crash").unwrap(), done("answer"));
    assert_eq!(provider.requests().len(), 2);
}
