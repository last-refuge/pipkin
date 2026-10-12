use super::cancel_tests::{replies, send, until};
use super::tests::{Engine, Fixture};
use crate::context::compaction::SUMMARIZATION_SYSTEM_PROMPT;
use crate::provider::scripted::{Gate, Reply as R, Scripted};
use pipkin_engine_protocol::*;
use std::sync::atomic::Ordering;

fn settings(fx: &Fixture, enabled: bool) {
    std::fs::create_dir_all(fx.dir.path().join("config")).unwrap();
    std::fs::write(
        fx.dir.path().join("config/settings.json"),
        serde_json::json!({
            "compaction": {"enabled": enabled, "keepRecentTokens": 5,
                "reserveTokens": 1000, "backgroundTokens": 199000}
        })
        .to_string(),
    )
    .unwrap();
}
fn seed(fx: &Fixture, e: &mut Engine) -> String {
    let session = e.create(&fx.project, "s");
    for i in 0..4 {
        let key = format!("seed{i}");
        e.prompt(&session, &key, "earlier conversation with several words")
            .unwrap();
        assert!(matches!(e.settled(&key), RunOutcome::Completed));
    }
    session
}

#[test]
fn stop_cancels_all_manual_compactions_without_a_running_turn_and_allows_a_new_one() {
    let fx = Fixture::new();
    settings(&fx, false);
    let gate = Gate::new();
    let g = gate.clone();
    let provider = Scripted::new(move |req, _| {
        if req.system == SUMMARIZATION_SYSTEM_PROMPT {
            R::Gated(
                g.clone(),
                Box::new(R::Text("summary of previous work".into())),
            )
        } else {
            R::Text("answer".into())
        }
    });
    let mut e = fx.start(&provider);
    let session = seed(&fx, &mut e);
    let before = e.handle.shared().store.context_entries(&session).unwrap();
    let conn = e.handle.connect("manual");
    for id in 1..=2 {
        send(
            &conn,
            id,
            Call::Compact {
                session: session.clone(),
                instructions: None,
            },
        );
    }
    until(|| {
        provider
            .requests()
            .iter()
            .filter(|r| r.system == SUMMARIZATION_SYSTEM_PROMPT)
            .count()
            == 2
    });
    assert_eq!(
        e.call(Call::Stop {
            session: session.clone()
        }),
        Ok(Reply::Stopping { run: None })
    );
    for result in replies(&conn, 2).values() {
        assert_eq!(result.as_ref().unwrap_err().message, "Compaction aborted.");
    }
    let sess = e.handle.shared().session(&session).unwrap();
    assert!(sess.manual_compactions.lock().unwrap().is_empty());
    assert_eq!(
        e.handle.shared().store.context_entries(&session).unwrap(),
        before
    );
    gate.open();
    assert_eq!(
        e.call(Call::Compact {
            session: session.clone(),
            instructions: None
        }),
        Ok(Reply::Ok)
    );
    assert!(
        e.handle
            .shared()
            .store
            .context_entries(&session)
            .unwrap()
            .iter()
            .any(|e| matches!(e.kind, EntryKind::Compaction { .. }))
    );
}

#[test]
fn background_compaction_survives_stop_and_does_not_restore_the_aborted_instruction() {
    let fx = Fixture::new();
    settings(&fx, false);
    let gate = Gate::new();
    let g = gate.clone();
    let provider = Scripted::new(move |req, _| {
        if req.system == SUMMARIZATION_SYSTEM_PROMPT {
            R::Gated(
                g.clone(),
                Box::new(R::Text("summary of previous work".into())),
            )
        } else if req.messages.iter().any(|m| m.text() == "stop this turn") {
            R::Hang
        } else {
            R::Text("answer".into())
        }
    });
    let mut e = fx.start(&provider);
    let session = seed(&fx, &mut e);
    settings(&fx, true);
    e.prompt(&session, "stopped", "stop this turn").unwrap();
    until(|| {
        let requests = provider.requests();
        requests
            .iter()
            .any(|r| r.system == SUMMARIZATION_SYSTEM_PROMPT)
            && requests
                .iter()
                .any(|r| r.messages.iter().any(|m| m.text() == "stop this turn"))
    });
    e.call(Call::Stop {
        session: session.clone(),
    })
    .unwrap();
    assert!(matches!(e.settled("stopped"), RunOutcome::Aborted));
    let sess = e.handle.shared().session(&session).unwrap();
    assert!(sess.compacting.load(Ordering::SeqCst));
    gate.open();
    until(|| !sess.compacting.load(Ordering::SeqCst));
    let entries = e.handle.shared().store.context_entries(&session).unwrap();
    assert!(
        entries
            .iter()
            .any(|e| matches!(e.kind, EntryKind::Compaction { .. }))
    );
    let summary_request = provider
        .requests()
        .into_iter()
        .find(|r| r.system == SUMMARIZATION_SYSTEM_PROMPT)
        .unwrap();
    assert!(
        !summary_request
            .messages
            .iter()
            .any(|m| m.text().contains("stop this turn"))
    );
    settings(&fx, false);
    e.prompt(&session, "next", "continue safely").unwrap();
    assert!(matches!(e.settled("next"), RunOutcome::Completed));
    let next = provider.requests().pop().unwrap();
    assert!(
        !next
            .messages
            .iter()
            .any(|m| m.text().contains("stop this turn"))
    );
}

#[test]
fn stop_after_completion_preserves_the_answer_and_its_context() {
    let fx = Fixture::new();
    settings(&fx, false);
    let provider = Scripted::echo("finished answer");
    let mut e = fx.start(&provider);
    let session = e.create(&fx.project, "s");
    e.prompt(&session, "done", "completed instruction").unwrap();
    assert!(matches!(e.settled("done"), RunOutcome::Completed));
    let before = e.handle.shared().store.context_entries(&session).unwrap();
    for _ in 0..3 {
        assert_eq!(
            e.call(Call::Stop {
                session: session.clone()
            }),
            Ok(Reply::Stopping { run: None })
        );
    }
    assert_eq!(
        e.handle.shared().store.context_entries(&session).unwrap(),
        before
    );
    assert_eq!(
        e.handle
            .shared()
            .store
            .request(&session, "done")
            .unwrap()
            .unwrap()
            .outcome,
        Some(RunOutcome::Completed)
    );
    e.prompt(&session, "next", "next instruction").unwrap();
    e.settled("next");
    let next = provider.requests().pop().unwrap();
    assert!(
        next.messages
            .iter()
            .any(|m| m.text() == "completed instruction")
    );
    assert!(next.messages.iter().any(|m| m.text() == "finished answer"));
}

#[cfg(unix)]
#[test]
fn stop_records_aborted_results_for_unstarted_sequential_tools_without_running_them() {
    let fx = Fixture::new();
    settings(&fx, false);
    let started = fx.project.join("started");
    let marker = started.clone();
    let provider = Scripted::new(move |_, _| {
        R::Tools(
            None,
            vec![
                (
                    "bash".into(),
                    serde_json::json!({"command": format!("printf ready > '{}'; sleep 30", marker.display())}),
                ),
                (
                    "write".into(),
                    serde_json::json!({"path":"must-not-exist","content":"unstarted"}),
                ),
            ],
        )
    });
    // Built-ins normally run in parallel. Exercise the supported sequential contract
    // with the real bash implementation wrapped in an explicit sequential declaration.
    struct Sequential(std::sync::Arc<dyn crate::tools::Tool>);
    impl crate::tools::Tool for Sequential {
        fn spec(&self) -> crate::provider::ToolSpec {
            self.0.spec()
        }
        fn sequential(&self) -> bool {
            true
        }
        fn execute<'a>(
            &'a self,
            args: serde_json::Value,
            ctx: &'a crate::tools::ToolCtx,
        ) -> crate::provider::BoxFuture<'a, Result<crate::tools::ToolOutput, crate::tools::ToolError>>
        {
            self.0.execute(args, ctx)
        }
    }
    let mut tools = crate::tools::builtin_tools();
    let index = tools.iter().position(|t| t.spec().name == "bash").unwrap();
    tools[index] = std::sync::Arc::new(Sequential(tools[index].clone()));
    let handle = super::EngineHandle::start_with(
        fx.config(),
        super::registry_with(std::sync::Arc::new(provider.clone())),
        tools,
    )
    .unwrap();
    let conn = handle.connect("sequential-stop");
    let mut e = Engine {
        handle,
        conn,
        next: 0,
        events: vec![],
    };
    let session = e.create(&fx.project, "s");
    e.prompt(&session, "stopped", "run sequential tools")
        .unwrap();
    until(|| started.is_file());
    e.call(Call::Stop {
        session: session.clone(),
    })
    .unwrap();
    assert!(matches!(e.settled("stopped"), RunOutcome::Aborted));
    let snapshot = e.snapshot(&session);
    let results: Vec<_> = snapshot
        .entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            EntryKind::Message {
                message:
                    message @ Message::ToolResult {
                        tool_name,
                        is_error,
                        ..
                    },
                ..
            } => Some((tool_name.as_str(), *is_error, message.text())),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].0, "bash");
    assert_eq!(results[1].0, "write");
    assert!(
        results
            .iter()
            .all(|(_, error, text)| *error && text.contains("was aborted"))
    );
    assert!(!fx.project.join("must-not-exist").exists());
    assert_eq!(provider.requests().len(), 1);
}

#[test]
fn stop_persists_foreground_compaction_intent_even_without_an_open_coding_run() {
    use crate::store::{Store, compactions::CompactionScope};
    let fx = Fixture::new();
    let mut e = fx.start(&Scripted::echo("unused"));
    let session = e.create(&fx.project, "s");
    let (manual, background) = e
        .handle
        .shared()
        .store
        .tx(|tx| {
            Ok((
                Store::admit_compaction(
                    tx,
                    &session,
                    "manual",
                    None,
                    CompactionScope::Manual,
                    None,
                )?,
                Store::admit_compaction(
                    tx,
                    &session,
                    "background",
                    None,
                    CompactionScope::Background,
                    None,
                )?,
            ))
        })
        .unwrap();
    assert_eq!(
        e.call(Call::Stop { session }),
        Ok(Reply::Stopping { run: None })
    );
    e.handle.shutdown(std::time::Duration::from_secs(1));
    drop(e);
    let store = Store::open(&fx.config().data_dir).unwrap();
    let manual = store
        .read(|c| Store::compaction_in(c, manual.id))
        .unwrap()
        .unwrap();
    let background = store
        .read(|c| Store::compaction_in(c, background.id))
        .unwrap()
        .unwrap();
    assert!(manual.stopping);
    assert!(!background.stopping);
    assert_eq!(store.pending_compaction_ids().unwrap().len(), 2);
}
