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
    settings(&fx, false);
    let initial = Scripted::new(|req, _| {
        if req.system == SUMMARIZATION_SYSTEM_PROMPT {
            R::Hang
        } else {
            R::Text("seed answer".into())
        }
    });
    let mut e = fx.start(&initial);
    let session = seed(&fx, &mut e);
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
        e.call(Call::Stop {
            session: session.clone()
        }),
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
    use crate::store::compactions::{CompactionOutcome, CompactionProgress};
    assert!(
        manual.stopping
            || matches!(
                manual.progress,
                CompactionProgress::Terminal {
                    result: CompactionOutcome::Aborted
                }
            )
    );
    assert!(!background.stopping);
    drop(store);
    let resumed = Scripted::echo("recovered background summary");
    let e = fx.start(&resumed);
    let store = &e.handle.shared().store;
    until(|| store.pending_compaction_ids().unwrap().is_empty());
    let manual = store
        .read(|c| Store::compaction_in(c, manual.id))
        .unwrap()
        .unwrap();
    let background = store
        .read(|c| Store::compaction_in(c, background.id))
        .unwrap()
        .unwrap();
    assert!(matches!(
        manual.progress,
        CompactionProgress::Terminal {
            result: CompactionOutcome::Aborted
        }
    ));
    assert!(matches!(
        background.progress,
        CompactionProgress::Terminal {
            result: CompactionOutcome::Done { .. }
        }
    ));
    assert_eq!(
        resumed.requests().len(),
        1,
        "only background resumes, without OpenSession"
    );
    assert_eq!(store.usage(&session).unwrap().models[0].1.total_tokens, 75);
}

#[test]
fn recovered_compactions_are_bounded_and_stop_settles_jobs_waiting_for_capacity() {
    use crate::store::{Store, compactions::CompactionScope};
    let fx = Fixture::new();
    settings(&fx, false);
    let mut e = fx.start(&Scripted::echo("seed answer"));
    let session = seed(&fx, &mut e);
    e.handle.shutdown(std::time::Duration::from_secs(1));
    drop(e);
    let store = Store::open(&fx.config().data_dir).unwrap();
    store
        .tx(|tx| {
            for i in 0..18 {
                Store::admit_compaction(
                    tx,
                    &session,
                    &format!("manual-{i}"),
                    None,
                    CompactionScope::Manual,
                    None,
                )?;
            }
            Ok(())
        })
        .unwrap();
    drop(store);
    let provider = Scripted::new(|_, _| R::Hang);
    let mut e = fx.start(&provider);
    until(|| provider.requests().len() == 8);
    assert_eq!(e.handle.shared().compaction_slots.available_permits(), 0);
    e.call(Call::Stop {
        session: session.clone(),
    })
    .unwrap();
    until(|| {
        e.handle
            .shared()
            .store
            .pending_compaction_ids()
            .unwrap()
            .is_empty()
    });
    assert_eq!(
        provider.requests().len(),
        8,
        "pending stopped jobs never contact the provider"
    );
    assert_eq!(
        e.handle.shared().store.usage(&session).unwrap().models[0]
            .1
            .total_tokens,
        60
    );
    until(|| e.handle.shared().compaction_slots.available_permits() == 8);
}

#[test]
fn reopen_reclassifies_a_saved_response_after_storage_failure_without_network_or_duplicate_usage() {
    use crate::store::{Store, compactions::*};
    let fx = Fixture::new();
    settings(&fx, false);
    let provider = Scripted::echo("seed answer");
    let mut e = fx.start(&provider);
    let session = seed(&fx, &mut e);
    let shared = e.handle.shared().clone();
    let entries = shared.store.context_entries(&session).unwrap();
    let first_kept = crate::context::compaction::select_cut(&entries, 5).unwrap();
    let request = SummaryRequest::capture(
        &provider.requests()[0],
        entries.last().unwrap().id,
        first_kept,
        100,
    );
    let _job = shared
        .store
        .tx(|tx| {
            let job = Store::admit_compaction(
                tx,
                &session,
                "saved-response",
                None,
                CompactionScope::Manual,
                None,
            )?;
            Store::save_compaction(
                tx,
                &job,
                &CompactionProgress::Summarize {
                    request: Box::new(request),
                    attempt: 1,
                    response: Some(Box::new(SummaryResponse {
                        message: Message::Assistant {
                            content: vec![Block::text("saved durable summary")],
                            api: "scripted".into(),
                            provider: "scripted".into(),
                            model: "scripted".into(),
                            usage: Usage {
                                input: 10,
                                output: 5,
                                total_tokens: 15,
                                ..Default::default()
                            },
                            stop_reason: StopReason::Stop,
                            error_message: None,
                            timestamp: 77,
                            extra: Default::default(),
                        },
                        received_at: 77,
                    })),
                },
            )?;
            Ok(job)
        })
        .unwrap();
    let sess = shared.session(&session).unwrap();
    shared.store.fail_next_write();
    shared.compaction_recovery.notify_one();
    until(|| sess.failure().is_some() && sess.active_compactions.lock().unwrap().is_empty());
    assert_eq!(
        shared.store.usage(&session).unwrap().models[0]
            .1
            .total_tokens,
        60
    );
    assert!(
        !shared
            .store
            .context_entries(&session)
            .unwrap()
            .iter()
            .any(|e| matches!(e.kind, EntryKind::Compaction { .. }))
    );
    e.snapshot(&session); // Reopen clears failure and wakes recovery even with no coding run.
    until(|| shared.store.pending_compaction_ids().unwrap().is_empty());
    assert_eq!(
        shared.store.usage(&session).unwrap().models[0]
            .1
            .total_tokens,
        75
    );
    assert_eq!(
        provider.requests().len(),
        4,
        "the saved summary never contacts the provider"
    );
    assert_eq!(
        shared
            .store
            .context_entries(&session)
            .unwrap()
            .iter()
            .filter(|e| matches!(e.kind, EntryKind::Compaction { .. }))
            .count(),
        1
    );
    e.snapshot(&session);
    assert_eq!(
        shared.store.usage(&session).unwrap().models[0]
            .1
            .total_tokens,
        75
    );
}

#[test]
fn blocking_threshold_recovers_before_coding_even_when_compaction_is_disabled_after_restart() {
    let fx = Fixture::new();
    settings(&fx, false);
    let initial = Scripted::new(|req, _| {
        if req.system == SUMMARIZATION_SYSTEM_PROMPT {
            R::Hang
        } else {
            R::Text("seed answer".into())
        }
    });
    let mut e = fx.start(&initial);
    let session = seed(&fx, &mut e);
    std::fs::write(
        fx.dir.path().join("config/settings.json"),
        serde_json::json!({
            "compaction":{"enabled":true,"keepRecentTokens":5,"reserveTokens":199999}
        })
        .to_string(),
    )
    .unwrap();
    e.prompt(&session, "threshold", "new coding instruction")
        .unwrap();
    until(|| {
        initial
            .requests()
            .iter()
            .any(|r| r.system == SUMMARIZATION_SYSTEM_PROMPT)
    });
    assert_eq!(initial.requests().len(), 5, "coding waits for the summary");
    e.handle.shutdown(std::time::Duration::from_secs(1));
    drop(e);
    settings(&fx, false);
    let resumed = Scripted::new(|req, _| {
        if req.system == SUMMARIZATION_SYSTEM_PROMPT {
            R::Text("recovered threshold summary".into())
        } else {
            R::Text("completed after recovery".into())
        }
    });
    let mut e = fx.start(&resumed);
    e.call(Call::OpenSession {
        session: session.clone(),
        window: None,
    })
    .unwrap();
    assert_eq!(e.settled("threshold"), RunOutcome::Completed);
    let requests = resumed.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].system, SUMMARIZATION_SYSTEM_PROMPT);
    assert!(
        requests[1]
            .messages
            .iter()
            .any(|m| m.text().contains("recovered threshold summary"))
    );
    assert!(
        requests[1]
            .messages
            .iter()
            .any(|m| m.text() == "new coding instruction")
    );
    assert!(
        e.handle
            .shared()
            .store
            .pending_compaction_ids()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_late_summary_cannot_replace_a_newer_head_but_its_usage_is_counted_once() {
    let fx = Fixture::new();
    settings(&fx, false);
    let first = Gate::new();
    let second = Gate::new();
    let gates = [first.clone(), second.clone()];
    let summaries = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = summaries.clone();
    let provider = Scripted::new(move |req, _| {
        if req.system == SUMMARIZATION_SYSTEM_PROMPT {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            R::Gated(gates[n].clone(), Box::new(R::Text(format!("summary {n}"))))
        } else {
            R::Text("seed answer".into())
        }
    });
    let mut e = fx.start(&provider);
    let session = seed(&fx, &mut e);
    let conn = e.handle.connect("summaries");
    send(
        &conn,
        1,
        Call::Compact {
            session: session.clone(),
            instructions: None,
        },
    );
    until(|| summaries.load(Ordering::SeqCst) == 1);
    send(
        &conn,
        2,
        Call::Compact {
            session: session.clone(),
            instructions: None,
        },
    );
    until(|| summaries.load(Ordering::SeqCst) == 2);
    second.open();
    until(|| {
        e.handle.shared().store.context_entries(&session).unwrap().iter().any(|e| matches!(&e.kind, EntryKind::Compaction { summary, .. } if summary == "summary 1"))
    });
    first.open();
    let results = replies(&conn, 2);
    assert_eq!(results[&2], Ok(Reply::Ok));
    assert!(
        results[&1]
            .as_ref()
            .unwrap_err()
            .message
            .contains("context changed")
    );
    let entries = e.handle.shared().store.context_entries(&session).unwrap();
    assert_eq!(
        entries
            .iter()
            .filter(|e| matches!(e.kind, EntryKind::Compaction { .. }))
            .count(),
        1
    );
    assert_eq!(
        e.handle.shared().store.usage(&session).unwrap().models[0]
            .1
            .total_tokens,
        90
    );
}

#[test]
fn orphaned_child_abort_reaches_foreground_compaction_without_an_open_child_run() {
    use crate::store::Store;
    let fx = Fixture::new();
    settings(&fx, false);
    let provider = Scripted::new(|req, _| {
        if req.system == SUMMARIZATION_SYSTEM_PROMPT {
            R::Hang
        } else {
            R::Text("seed answer".into())
        }
    });
    let mut e = fx.start(&provider);
    let child = seed(&fx, &mut e);
    let parent = e.create(&fx.project, "parent");
    let shared = e.handle.shared().clone();
    shared
        .store
        .tx(|tx| {
            Store::create_task(
                tx,
                &parent,
                &parent,
                "orphan",
                &child,
                "child work",
                crate::util::now_ms(),
            )?;
            Ok(())
        })
        .unwrap();
    let conn = e.handle.connect("child-compaction");
    send(
        &conn,
        1,
        Call::Compact {
            session: child.clone(),
            instructions: None,
        },
    );
    until(|| {
        provider
            .requests()
            .iter()
            .any(|r| r.system == SUMMARIZATION_SYSTEM_PROMPT)
    });
    let aborter = shared.clone();
    shared.spawn(async move {
        aborter.abort_child(&parent, "orphan");
    });
    let results = replies(&conn, 1);
    assert_eq!(
        results[&1].as_ref().unwrap_err().message,
        "Compaction aborted."
    );
    until(|| shared.store.pending_compaction_ids().unwrap().is_empty());
    assert!(
        !shared
            .store
            .context_entries(&child)
            .unwrap()
            .iter()
            .any(|e| matches!(e.kind, EntryKind::Compaction { .. }))
    );
}

#[test]
fn compaction_retry_policy_changes_apply_without_resetting_the_attempt() {
    let fx = Fixture::new();
    settings(&fx, false);
    std::fs::write(
        fx.dir.path().join("config/settings.json"),
        serde_json::json!({
            "compaction":{"enabled":false,"keepRecentTokens":5},
            "retry":{"maxRetries":3,"baseDelayMs":1}
        })
        .to_string(),
    )
    .unwrap();
    let gate = Gate::new();
    let held = gate.clone();
    let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let attempts = count.clone();
    let provider = Scripted::new(move |req, _| {
        if req.system != SUMMARIZATION_SYSTEM_PROMPT {
            return R::Text("seed answer".into());
        }
        let error = R::Error("503: overloaded".into());
        if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            error
        } else {
            R::Gated(held.clone(), Box::new(error))
        }
    });
    let mut e = fx.start(&provider);
    let session = seed(&fx, &mut e);
    let conn = e.handle.connect("retry-policy");
    send(
        &conn,
        1,
        Call::Compact {
            session,
            instructions: None,
        },
    );
    until(|| count.load(Ordering::SeqCst) == 2);
    std::fs::write(
        fx.dir.path().join("config/settings.json"),
        serde_json::json!({"retry":{"enabled":false}}).to_string(),
    )
    .unwrap();
    gate.open();
    assert!(replies(&conn, 1)[&1].is_err());
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert!(
        e.handle
            .shared()
            .store
            .pending_compaction_ids()
            .unwrap()
            .is_empty()
    );
}
