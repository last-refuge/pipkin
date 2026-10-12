//! Qualification of fault and recovery paths (plan §9): storage faults at each durable phase,
//! the subagent recovery contract, Stop around subagents, and the whole-turn retry policy.
//! Real process kills are in `tests/crash_points.rs`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pipkin_engine_protocol::*;
use serde_json::json;

use super::tests::{Engine, Fixture, texts};
use crate::provider::ProviderRequest;
use crate::provider::scripted::{Reply as R, Scripted};
use crate::store::{Fault, Store};

fn user_texts(req: &ProviderRequest) -> Vec<String> {
    req.messages
        .iter()
        .filter(|m| matches!(m, Message::User { .. }))
        .map(|m| m.text())
        .collect()
}

fn lookup(e: &mut Engine, s: &str, key: &str) -> LookupResult {
    match e.call(Call::Lookup {
        session: s.into(),
        request: key.into(),
    }) {
        Ok(Reply::Lookup(l)) => l,
        other => panic!("{other:?}"),
    }
}

/// Wait until `key` is settled, asking (events before a connect are not replayed).
fn settled_by_lookup(e: &mut Engine, s: &str, key: &str) -> RunOutcome {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let LookupResult::Known {
            open: false,
            outcome: Some(outcome),
            ..
        } = lookup(e, s, key)
        {
            return outcome;
        }
        assert!(Instant::now() < deadline, "{key} did not settle");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Reopen the session until `key` settles: the first reopen may come while the failed run is
/// still winding down in this process, and then there is nothing to resume yet.
fn reopen_until_settled(e: &mut Engine, s: &str, key: &str) -> RunOutcome {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        e.snapshot(s);
        if let LookupResult::Known {
            open: false,
            outcome: Some(outcome),
            ..
        } = lookup(e, s, key)
        {
            return outcome;
        }
        assert!(
            Instant::now() < deadline,
            "{key} did not settle after reopening"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn storage_notice(events: &[Event]) -> Option<String> {
    events.iter().find_map(|e| match e {
        Event::Notice { message } if message.contains("storage error") => Some(message.clone()),
        _ => None,
    })
}

fn settled_event(events: &[Event], key: &str) -> bool {
    events.iter().any(|e| {
        matches!(e, Event::Session { change: SessionChange::RunSettled { request: Some(r), .. }, .. } if r == key)
    })
}

fn entry_delta(events: &[Event]) -> bool {
    events.iter().any(|e| {
        matches!(
            e,
            Event::Session {
                change: SessionChange::EntryDelta { .. },
                ..
            }
        )
    })
}

fn history(e: &mut Engine, s: &str) -> Vec<Entry> {
    match e.call(Call::HistoryPage {
        session: s.into(),
        before: None,
        limit: 500,
    }) {
        Ok(Reply::Page { entries, .. }) => entries,
        other => panic!("{other:?}"),
    }
}

// ----------------------------------------------------------------------------- storage faults

#[test]
fn disk_full_or_read_only_at_admission_fails_closed_and_admits_nothing() {
    for fault in [Fault::DiskFull, Fault::ReadOnly] {
        let fx = Fixture::new();
        let provider = Scripted::echo("ok");
        let mut e = fx.start(&provider);
        let s = e.create(&fx.project, "c");
        e.handle.shared().store.inject(fault).unwrap();
        // Large enough that it needs new database pages.
        let big = "lorem ipsum ".repeat(30_000);
        let err = e.prompt(&s, "r1", &big).unwrap_err();
        assert_eq!(err.code, ErrorCode::Storage, "{fault:?}: {err:?}");
        assert!(
            storage_notice(&e.events).is_some(),
            "{fault:?}: the client is told"
        );
        // Failed closed: even a write that would fit is refused until reopened.
        let again = e.prompt(&s, "r2", "small").unwrap_err();
        assert_eq!(again.code, ErrorCode::Storage);
        assert!(again.message.contains("Reopen it"), "{}", again.message);
        assert_eq!(lookup(&mut e, &s, "r1"), LookupResult::Unknown);
        assert_eq!(lookup(&mut e, &s, "r2"), LookupResult::Unknown);
        e.handle.shared().store.clear_faults().unwrap();
        e.snapshot(&s);
        e.prompt(&s, "r3", "now it fits").unwrap();
        assert_eq!(e.settled("r3"), RunOutcome::Completed);
        assert_eq!(provider.requests().len(), 1, "{fault:?}");
        assert_eq!(user_texts(&provider.requests()[0]), ["now it fits"]);
    }
}

/// A fault while a reply streams (committing deltas, finalizing the message, or the whole
/// database turning read-only): the session fails closed with nothing claimed, the client is
/// told, and reopening resumes the turn from what was committed, asking the model again.
fn a_fault_mid_turn_fails_closed_and_reopening_resumes(fault: Fault) {
    let fx = Fixture::new();
    let first = "partial progress that keeps streaming for quite a while";
    let provider = Scripted::new(move |_, n| {
        R::Text(if n == 0 {
            first.into()
        } else {
            "resumed answer".into()
        })
    })
    .paced(Duration::from_millis(25));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "stream something").unwrap();
    e.until("committed progress", entry_delta);
    e.handle.shared().store.inject(fault).unwrap();
    e.until("the storage failure is reported", |ev| {
        storage_notice(ev).is_some()
    });
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !settled_event(&e.events, "r1"),
        "{fault:?}: no fake settlement"
    );
    let refused = e.prompt(&s, "r2", "more").unwrap_err();
    assert_eq!(refused.code, ErrorCode::Storage, "{fault:?}");
    assert!(
        matches!(
            lookup(&mut e, &s, "r1"),
            LookupResult::Known { open: true, .. }
        ),
        "{fault:?}: still open"
    );
    e.handle.shared().store.clear_faults().unwrap();
    assert_eq!(
        reopen_until_settled(&mut e, &s, "r1"),
        RunOutcome::Completed,
        "{fault:?}"
    );
    let snap = e.snapshot(&s);
    let all = texts(&snap);
    let users: Vec<_> = all.iter().filter(|t| t.0 == "user").collect();
    assert_eq!(users.len(), 1, "{fault:?}: {all:?}");
    let assistants: Vec<_> = all.iter().filter(|t| t.0 == "assistant").collect();
    assert_eq!(assistants.last().unwrap().1, "resumed answer", "{fault:?}");
    // What was committed of the interrupted reply is kept (finalized as aborted), never more.
    assert!(assistants.len() <= 2, "{fault:?}: {all:?}");
    if assistants.len() == 2 {
        assert!(first.starts_with(&assistants[0].1), "{fault:?}: {all:?}");
    }
    let requests = provider.requests();
    assert_eq!(requests.len(), 2, "{fault:?}: the turn is asked once more");
    assert_eq!(user_texts(&requests[1]), ["stream something"], "{fault:?}");
    assert!(
        !requests[1]
            .messages
            .iter()
            .any(|m| matches!(m, Message::Assistant { .. })),
        "{fault:?}: the unfinished reply is not presented as context"
    );
}

#[test]
fn a_fault_committing_streamed_progress_fails_closed_and_reopening_resumes() {
    a_fault_mid_turn_fails_closed_and_reopening_resumes(Fault::ChunkFlush);
}

#[test]
fn a_fault_finalizing_the_reply_fails_closed_and_reopening_resumes() {
    a_fault_mid_turn_fails_closed_and_reopening_resumes(Fault::FinalCommit);
}

#[test]
fn a_database_turning_read_only_mid_run_fails_closed_and_reopening_resumes() {
    a_fault_mid_turn_fails_closed_and_reopening_resumes(Fault::ReadOnly);
}

#[test]
fn a_fault_recording_a_tool_result_never_reruns_the_command() {
    let fx = Fixture::new();
    let ran = fx.dir.path().join("ran.txt");
    let command = format!("echo once >> {}", ran.display());
    let provider = Scripted::new(move |_, n| match n {
        0 => R::Tools(
            None,
            vec![("bash".into(), json!({ "command": command.clone() }))],
        ),
        _ => R::Text("after".into()),
    });
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.handle.shared().store.inject(Fault::ToolResult).unwrap();
    e.prompt(&s, "r1", "run it").unwrap();
    e.until("the storage failure is reported", |ev| {
        storage_notice(ev).is_some()
    });
    assert_eq!(std::fs::read_to_string(&ran).unwrap(), "once\n");
    assert!(matches!(
        lookup(&mut e, &s, "r1"),
        LookupResult::Known { open: true, .. }
    ));
    e.handle.shared().store.clear_faults().unwrap();
    assert_eq!(
        reopen_until_settled(&mut e, &s, "r1"),
        RunOutcome::Completed
    );
    assert_eq!(
        std::fs::read_to_string(&ran).unwrap(),
        "once\n",
        "the command ran once"
    );
    let snap = e.snapshot(&s);
    let tool = texts(&snap).into_iter().find(|t| t.0 == "tool").unwrap().1;
    assert!(
        tool.contains("Tool bash was interrupted and may have partially run"),
        "{tool}"
    );
    let requests = provider.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].messages.iter().any(|m| matches!(m,
        Message::ToolResult { is_error: true, .. } if m.text().contains("was interrupted"))));
}

#[test]
fn tool_checkpoint_faults_keep_the_last_commit_and_do_not_publish_failed_output() {
    for (fault, after_gate) in [
        (Fault::ReadOnly, "printf 'uncommitted-suffix'; sleep 30"),
        (Fault::ToolResult, ":"),
    ] {
        let fx = Fixture::new();
        let gate = fx.project.join("release-tool");
        let ran = fx.project.join("ran-once");
        let command = format!(
            "echo once >> {}; printf 'saved-prefix'; while [ ! -e {} ]; do sleep 0.02; done; {after_gate}",
            ran.display(),
            gate.display(),
        );
        let provider = Scripted::new(move |_, n| {
            if n == 0 {
                R::Tools(None, vec![("bash".into(), json!({"command": command}))])
            } else {
                R::Text("recovered".into())
            }
        });
        let mut e = fx.start(&provider);
        let s = e.create(&fx.project, "c");
        e.prompt(&s, "r1", "run it").unwrap();
        e.until("first durable output", |events| {
            events.iter().any(|event| {
                matches!(event,
                    Event::Session { change: SessionChange::ToolProgress { text, .. }, .. }
                        if text == "saved-prefix"
                )
            })
        });
        let call_id = e
            .events
            .iter()
            .find_map(|event| match event {
                Event::Session {
                    change: SessionChange::ToolProgress { call_id, .. },
                    ..
                } => Some(call_id.clone()),
                _ => None,
            })
            .unwrap();
        e.handle.shared().store.inject(fault).unwrap();
        std::fs::write(&gate, "go").unwrap();
        e.until("storage fault", |events| storage_notice(events).is_some());
        assert!(!settled_event(&e.events, "r1"));
        assert!(!e.events.iter().any(|event| matches!(event,
            Event::Session { change: SessionChange::ToolProgress { text, .. }, .. }
                if text.contains("uncommitted-suffix")
        )));
        let checkpoint = e
            .handle
            .shared()
            .store
            .read(|c| Store::tool_progress(c, &s, &call_id))
            .unwrap();
        assert_eq!(
            checkpoint.text, "saved-prefix",
            "{fault:?}: old checkpoint survives rollback"
        );
        assert!(
            e.handle
                .shared()
                .store
                .tool_output(&s, &call_id)
                .unwrap()
                .is_none()
        );
        e.handle.shared().store.clear_faults().unwrap();
        assert_eq!(
            reopen_until_settled(&mut e, &s, "r1"),
            RunOutcome::Completed
        );
        let result = tool_results(&e.snapshot(&s));
        assert_eq!(result.len(), 1);
        assert!(result[0].2.contains("saved-prefix"));
        assert!(result[0].2.contains("was interrupted"));
        assert!(!result[0].2.contains("uncommitted-suffix"));
        assert_eq!(std::fs::read_to_string(&ran).unwrap(), "once\n");
        assert_eq!(provider.requests().len(), 2);
        assert!(
            e.handle
                .shared()
                .store
                .kv_get(&s, &format!("tool_progress:{call_id}"))
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn a_fault_settling_the_run_keeps_the_answer_and_reopening_settles_without_asking_again() {
    let fx = Fixture::new();
    let provider = Scripted::echo("all done");
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.handle.shared().store.inject(Fault::Settlement).unwrap();
    e.prompt(&s, "r1", "finish").unwrap();
    e.until("the storage failure is reported", |ev| {
        storage_notice(ev).is_some()
    });
    assert!(!settled_event(&e.events, "r1"));
    // The answer was committed before the settlement failed (read without reopening).
    let answered = history(&mut e, &s).iter().any(|entry| {
        matches!(&entry.kind, EntryKind::Message { message: m @ Message::Assistant { .. }, partial: false, .. } if m.text() == "all done")
    });
    assert!(answered);
    assert!(matches!(
        lookup(&mut e, &s, "r1"),
        LookupResult::Known { open: true, .. }
    ));
    e.handle.shared().store.clear_faults().unwrap();
    assert_eq!(
        reopen_until_settled(&mut e, &s, "r1"),
        RunOutcome::Completed
    );
    assert_eq!(provider.requests().len(), 1, "the model is not asked again");
    let snap = e.snapshot(&s);
    assert_eq!(
        texts(&snap),
        vec![
            ("user".into(), "finish".into(), false),
            ("assistant".into(), "all done".into(), false)
        ]
    );
}

// ------------------------------------------------------------------------ subagent recovery

/// The parent asks for one subagent call per user prompt (call ids are per request), then
/// reports what came back; a child answers per `child`.
fn delegating(
    child: impl Fn(&ProviderRequest) -> R + Send + Sync + 'static,
) -> impl Fn(&ProviderRequest, usize) -> R + Send + Sync + 'static {
    move |req, _| {
        let is_child = !req
            .messages
            .iter()
            .filter(|m| matches!(m, Message::User { .. }))
            .any(|m| ["delegate", "new", "stop it", "late stop"].contains(&m.text().as_str()));
        if is_child {
            return child(req);
        }
        let last_is_user = matches!(
            req.messages
                .iter()
                .rev()
                .find(|m| !matches!(m, Message::System(_))),
            Some(Message::User { .. })
        );
        if last_is_user {
            R::Tools(
                None,
                vec![("subagent".into(), json!({"task": "a careful survey"}))],
            )
        } else {
            let results: String = req
                .messages
                .iter()
                .filter(|m| matches!(m, Message::ToolResult { .. }))
                .map(|m| m.text())
                .collect::<Vec<_>>()
                .join(" | ");
            R::Text(format!("parent got: {results}"))
        }
    }
}

fn child_requests(p: &Scripted) -> usize {
    p.requests()
        .iter()
        .filter(|r| user_texts(r) == ["a careful survey"])
        .count()
}

fn subagents(e: &mut Engine, s: &str) -> SubagentsView {
    match e.call(Call::Subagents { session: s.into() }) {
        Ok(Reply::Subagents(v)) => v,
        other => panic!("{other:?}"),
    }
}

fn tool_results(snap: &SessionSnapshot) -> Vec<(String, bool, String)> {
    snap.entries
        .iter()
        .filter_map(|e| match &e.kind {
            EntryKind::Message {
                message:
                    m @ Message::ToolResult {
                        tool_name,
                        is_error,
                        ..
                    },
                ..
            } => Some((tool_name.clone(), *is_error, m.text())),
            _ => None,
        })
        .collect()
}

/// Port of the subagent overlay test "resumes an already-running subagent after disabling new
/// calls and restarting" (packaging/pi-subagents.patch, harness-tools-recovery.test.ts): the
/// in-flight replay-safe call recovers although new calls are disabled, and a new call from a
/// later turn is refused with `Tool subagent is not available`.
#[test]
fn a_running_subagent_resumes_after_subagents_were_disabled_and_a_restart() {
    let fx = Fixture::new();
    let s;
    {
        let hang = Scripted::new(delegating(|_| R::Hang));
        let mut e = fx.start(&hang);
        s = e.create(&fx.project, "c");
        e.prompt(&s, "r1", "delegate").unwrap();
        e.until("the child asked its model", |_| child_requests(&hang) == 1);
        e.call(Call::SetSubagentsEnabled {
            session: s.clone(),
            enabled: false,
        })
        .unwrap();
        assert!(!subagents(&mut e, &s).enabled);
        e.handle.shutdown(Duration::from_millis(100));
    }
    let answer = Scripted::new(delegating(|_| R::Text("child finished".into())));
    let mut e = fx.start(&answer);
    assert_eq!(settled_by_lookup(&mut e, &s, "r1"), RunOutcome::Completed);
    // The recovered call reran (waiting on the same child), and the result is not an error.
    assert_eq!(child_requests(&answer), 1, "the same child continued");
    let snap = e.snapshot(&s);
    assert_eq!(
        tool_results(&snap),
        vec![("subagent".into(), false, "child finished".into())]
    );
    assert_eq!(subagents(&mut e, &s).children.len(), 1);
    // The parent's next request no longer offers the tool...
    let resumed = answer.requests().into_iter().last().unwrap();
    assert!(resumed.tools.iter().all(|t| t.name != "subagent"));
    // ...and a new call from a later turn has no recovery authority: it is refused.
    e.prompt(&s, "r2", "new").unwrap();
    assert_eq!(e.settled("r2"), RunOutcome::Completed);
    let snap = e.snapshot(&s);
    let results = tool_results(&snap);
    assert_eq!(results.len(), 2);
    assert!(results[1].1, "{results:?}");
    assert!(
        results[1].2.contains("Tool subagent is not available"),
        "{results:?}"
    );
    assert_eq!(
        subagents(&mut e, &s).children.len(),
        1,
        "no second child was made"
    );
    assert_eq!(child_requests(&answer), 1);
}

/// Port of "does not give a removed or replaced subagent the old call's recovery authority":
/// when the call's recorded contract (name, description, parameters, replay policy) differs
/// from the current tool's, or is missing, the restarted engine records the call interrupted,
/// never reruns it, and stops its child instead of letting it run on unobserved. The native
/// subagent tool is built in, so the change is made to the recorded side of the comparison.
#[test]
fn a_changed_subagent_contract_gives_no_recovery_authority() {
    for change in ["removed", "description", "parameters", "policy"] {
        let fx = Fixture::new();
        let s;
        {
            let hang = Scripted::new(delegating(|_| R::Hang));
            let mut e = fx.start(&hang);
            s = e.create(&fx.project, "c");
            e.prompt(&s, "r1", "delegate").unwrap();
            e.until("the child asked its model", |_| child_requests(&hang) == 1);
            e.handle.shutdown(Duration::from_millis(100));
        }
        {
            let store = Store::open(&fx.config().data_dir).unwrap();
            let call = store
                .read(|c| Store::tools_in_state(c, &s, "intent"))
                .unwrap()
                .into_iter()
                .find(|c| c.name == "subagent")
                .unwrap();
            let key = format!("tool_contract:{}", call.call_id);
            let mut contract: serde_json::Value =
                serde_json::from_str(&store.kv_get(&s, &key).unwrap().unwrap()).unwrap();
            match change {
                "description" => contract["description"] = json!("different registration"),
                "parameters" => contract["parameters"]["required"] = json!([]),
                "policy" => contract["replay"] = json!("unsafe"),
                _ => {}
            }
            store
                .tx(|tx| {
                    if change == "removed" {
                        tx.execute(
                            "DELETE FROM kv WHERE session = ?1 AND key = ?2",
                            rusqlite::params![s, key],
                        )?;
                        Ok(())
                    } else {
                        Store::kv_set(tx, &s, &key, &contract.to_string())
                    }
                })
                .unwrap();
        }
        let answer = Scripted::new(delegating(|_| R::Text("must not run".into())));
        let mut e = fx.start(&answer);
        assert_eq!(
            settled_by_lookup(&mut e, &s, "r1"),
            RunOutcome::Completed,
            "{change}"
        );
        let snap = e.snapshot(&s);
        let results = tool_results(&snap);
        assert_eq!(results.len(), 1, "{change}: {results:?}");
        assert!(results[0].1, "{change}: {results:?}");
        assert!(
            results[0]
                .2
                .contains("Tool subagent was interrupted and may have partially run"),
            "{change}: {results:?}"
        );
        // The child was stopped, not resumed: its model is never asked again.
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(child_requests(&answer), 0, "{change}");
        let view = subagents(&mut e, &s);
        assert_eq!(view.children.len(), 1, "{change}");
        assert_eq!(view.children[0].status, "aborted", "{change}");
        let child = view.children[0].session.clone();
        assert_eq!(
            settled_by_lookup(
                &mut e,
                &child,
                &format!("subagent:{}", view.children[0].call_id)
            ),
            RunOutcome::Aborted,
            "{change}"
        );
    }
}

#[test]
fn stopping_the_parent_stops_its_running_child() {
    let fx = Fixture::new();
    let provider = Scripted::new(delegating(|_| R::Hang));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "stop it").unwrap();
    e.until("the child asked its model", |_| {
        child_requests(&provider) == 1
    });
    let started = Instant::now();
    e.call(Call::Stop { session: s.clone() }).unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Aborted);
    assert!(started.elapsed() < Duration::from_secs(5));
    let view = subagents(&mut e, &s);
    assert_eq!(view.children.len(), 1);
    assert_eq!(view.children[0].status, "aborted");
    let child_key = format!("subagent:{}", view.children[0].call_id);
    assert_eq!(
        settled_by_lookup(&mut e, &view.children[0].session, &child_key),
        RunOutcome::Aborted
    );
    let snap = e.snapshot(&s);
    let results = tool_results(&snap);
    assert_eq!(results.len(), 1, "{results:?}");
    assert!(
        results[0].1 && results[0].2.contains("aborted"),
        "{results:?}"
    );
    assert_eq!(child_requests(&provider), 1);
}

#[cfg(unix)]
#[test]
fn orphaned_child_stop_storage_failure_cancels_execution_without_claiming_settlement() {
    let fx = Fixture::new();
    let provider = Scripted::new(|_, _| {
        R::Tools(
            None,
            vec![(
                "bash".into(),
                json!({
                    "command":"printf 'started\\n' >> orphan-effects; sleep 30; printf 'finished\\n' >> orphan-effects"
                }),
            )],
        )
    });
    let mut e = fx.start(&provider);
    let parent = e.create(&fx.project, "parent");
    let child = e.create(&fx.project, "child");
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
    e.prompt(&child, "child-work", "run the command").unwrap();
    let effects = fx.project.join("orphan-effects");
    e.until("orphan tool started", |_| effects.exists());
    let child_rt = shared.session(&child).unwrap();
    shared.store.inject(Fault::ReadOnly).unwrap();
    let aborter = shared.clone();
    let parent_id = parent.clone();
    let (sent, received) = std::sync::mpsc::channel();
    shared.spawn(async move {
        sent.send(aborter.abort_child(&parent_id, "orphan"))
            .unwrap();
    });
    assert_eq!(
        received
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap_err()
            .code,
        ErrorCode::Storage
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while child_rt.active_run().is_some() {
        assert!(
            Instant::now() < deadline,
            "orphaned child kept executing after Stop storage failure"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(child_rt.failure().is_some());
    assert!(shared.session(&parent).unwrap().failure().is_some());
    assert!(matches!(
        lookup(&mut e, &child, "child-work"),
        LookupResult::Known { open: true, .. }
    ));
    let task = shared
        .store
        .read(|c| Store::task_for_call(c, &parent, "orphan"))
        .unwrap()
        .unwrap();
    assert_ne!(task.status, "aborted");
    assert_eq!(std::fs::read_to_string(&effects).unwrap(), "started\n");
    shared.store.clear_faults().unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(provider.requests().len(), 1);
    let aborter = shared.clone();
    let (sent, received) = std::sync::mpsc::channel();
    shared.spawn(async move {
        sent.send(aborter.abort_child(&parent, "orphan")).unwrap();
    });
    received
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert_eq!(
        settled_by_lookup(&mut e, &child, "child-work"),
        RunOutcome::Aborted
    );
    assert_eq!(provider.requests().len(), 1);
    assert_eq!(std::fs::read_to_string(&effects).unwrap(), "started\n");
}

#[cfg(unix)]
#[test]
fn a_parent_storage_failure_cancels_child_tools_and_reopens_without_repeating_effects() {
    for fault in [Fault::ReadOnly, Fault::Admission] {
        let fx = Fixture::new();
        let provider = Scripted::new(delegating(|req| {
            if req
                .messages
                .iter()
                .any(|m| matches!(m, Message::ToolResult { .. }))
            {
                R::Text("child recovered its interrupted command".into())
            } else {
                R::Tools(
                    None,
                    vec![(
                        "bash".into(),
                        json!({
                            "command": "printf 'started\\n' >> child-effects; sleep 30; printf 'finished\\n' >> child-effects"
                        }),
                    )],
                )
            }
        }));
        let mut e = fx.start(&provider);
        let s = e.create(&fx.project, "c");
        e.prompt(&s, "r1", "delegate").unwrap();
        let effects = fx.project.join("child-effects");
        e.until("the child's real command started", |_| effects.exists());
        let view = subagents(&mut e, &s);
        let child = view.children[0].session.clone();
        let child_key = format!("subagent:{}", view.children[0].call_id);
        let shared = e.handle.shared().clone();
        let parent_rt = shared.session(&s).unwrap();
        let child_rt = shared.session(&child).unwrap();
        shared.store.inject(fault).unwrap();
        let error = e
            .call(Call::Enqueue {
                session: s.clone(),
                request: "failed-follow-up".into(),
                mode: InputMode::FollowUp,
                text: "must never be admitted".into(),
                images: vec![],
            })
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Storage);
        let deadline = Instant::now() + Duration::from_secs(5);
        while parent_rt.active_run().is_some() || child_rt.active_run().is_some() {
            assert!(
                Instant::now() < deadline,
                "{fault:?}: failed parent/child did not unwind promptly"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(parent_rt.failure().is_some());
        assert!(matches!(
            lookup(&mut e, &s, "r1"),
            LookupResult::Known { open: true, .. }
        ));
        assert_eq!(
            lookup(&mut e, &s, "failed-follow-up"),
            LookupResult::Unknown
        );
        assert_eq!(std::fs::read_to_string(&effects).unwrap(), "started\n");
        let before = provider.requests().len();
        shared.store.clear_faults().unwrap();
        // Restored storage alone grants no recovery authority.
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(provider.requests().len(), before);
        assert_eq!(
            reopen_until_settled(&mut e, &s, "r1"),
            RunOutcome::Completed
        );
        assert_eq!(std::fs::read_to_string(&effects).unwrap(), "started\n");
        assert_eq!(subagents(&mut e, &s).children.len(), 1);
        let outcome = settled_by_lookup(&mut e, &child, &child_key);
        if matches!(fault, Fault::ReadOnly) {
            assert_eq!(outcome, RunOutcome::Completed);
            let child_history = e.snapshot(&child);
            let results = tool_results(&child_history);
            assert_eq!(results.len(), 1);
            assert!(
                results[0].1 && results[0].2.contains("interrupted"),
                "{results:?}"
            );
        } else {
            assert_eq!(outcome, RunOutcome::Aborted);
        }
        assert_eq!(tool_results(&e.snapshot(&s)).len(), 1);
    }
}

#[test]
fn a_child_storage_failure_keeps_the_parent_call_recoverable() {
    let fx = Fixture::new();
    let turns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let child_turns = turns.clone();
    let provider = Scripted::new(delegating(move |_| {
        if child_turns.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            R::Text("child streaming partial progress ".repeat(1000))
        } else {
            R::Text("recovered child answer".into())
        }
    }))
    .paced(Duration::from_millis(1));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "delegate").unwrap();
    e.until("the child began streaming", |_| {
        child_requests(&provider) == 1
    });
    let child = subagents(&mut e, &s).children[0].session.clone();
    let shared = e.handle.shared().clone();
    let parent_rt = shared.session(&s).unwrap();
    let child_rt = shared.session(&child).unwrap();
    shared.store.inject(Fault::ChunkFlush).unwrap();
    e.until("child storage failure reaches its waiting parent", |_| {
        parent_rt.failure().is_some()
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while parent_rt.active_run().is_some() || child_rt.active_run().is_some() {
        assert!(
            Instant::now() < deadline,
            "parent/child failed execution did not unwind"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(child_rt.failure().is_some());
    assert!(matches!(
        lookup(&mut e, &s, "r1"),
        LookupResult::Known { open: true, .. }
    ));
    assert_eq!(
        e.prompt(&s, "r2", "must wait for recovery")
            .unwrap_err()
            .code,
        ErrorCode::Storage
    );
    let parent_history = history(&mut e, &s);
    assert!(!parent_history.iter().any(|entry| matches!(
        &entry.kind,
        EntryKind::Message {
            message: Message::ToolResult { .. },
            ..
        }
    )));
    shared.store.clear_faults().unwrap();
    assert_eq!(
        reopen_until_settled(&mut e, &s, "r1"),
        RunOutcome::Completed
    );
    assert_eq!(
        tool_results(&e.snapshot(&s)),
        [("subagent".into(), false, "recovered child answer".into())]
    );
    assert_eq!(
        child_requests(&provider),
        2,
        "the existing child resumes once"
    );
    assert_eq!(subagents(&mut e, &s).children.len(), 1);
}

#[test]
fn a_stop_after_the_child_finished_still_delivers_its_result_once() {
    let fx = Fixture::new();
    let parent_turns = Arc::new(Mutex::new(0usize));
    let turns = parent_turns.clone();
    let inner = delegating(|_| R::Text("child answer".into()));
    let provider = Scripted::new(move |req, n| {
        let parent = user_texts(req) == ["late stop"];
        if parent
            && req
                .messages
                .iter()
                .any(|m| matches!(m, Message::ToolResult { .. }))
        {
            // The parent's turn after the result: hold it, so Stop lands after delivery.
            *turns.lock().unwrap() += 1;
            return R::Hang;
        }
        inner(req, n)
    });
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "late stop").unwrap();
    e.until("the parent was given the result", |_| {
        *parent_turns.lock().unwrap() == 1
    });
    e.call(Call::Stop { session: s.clone() }).unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Aborted);
    let snap = e.snapshot(&s);
    assert_eq!(
        tool_results(&snap),
        vec![("subagent".into(), false, "child answer".into())],
        "delivered once, and not replaced by an aborted result"
    );
    let view = subagents(&mut e, &s);
    assert_eq!(view.children.len(), 1);
    assert_eq!(view.children[0].status, "done");
    assert_eq!(child_requests(&provider), 1);
}

// ------------------------------------------------------------------------------------ retry

fn with_settings(fx: &Fixture, settings: serde_json::Value) {
    let dir = fx.config().config_dir;
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("settings.json"), settings.to_string()).unwrap();
}

/// Records when each request arrived.
fn timed(
    reply: impl Fn(usize) -> R + Send + Sync + 'static,
) -> (Scripted, Arc<Mutex<Vec<Instant>>>) {
    let at = Arc::new(Mutex::new(Vec::new()));
    let a = at.clone();
    let provider = Scripted::new(move |_, n| {
        a.lock().unwrap().push(Instant::now());
        reply(n)
    });
    (provider, at)
}

#[test]
fn whole_turn_retries_stop_at_max_retries_with_exponential_backoff() {
    let fx = Fixture::new();
    with_settings(&fx, json!({"retry": {"maxRetries": 2, "baseDelayMs": 60}}));
    let (provider, at) = timed(|_| R::Error("503: overloaded".into()));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "x").unwrap();
    assert_eq!(
        e.settled("r1"),
        RunOutcome::ModelError {
            detail: "503: overloaded".into()
        }
    );
    std::thread::sleep(Duration::from_millis(300));
    let at = at.lock().unwrap().clone();
    assert_eq!(at.len(), 3, "one attempt plus maxRetries");
    let gaps: Vec<Duration> = at.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(gaps[0] >= Duration::from_millis(60), "{gaps:?}");
    assert!(gaps[1] >= Duration::from_millis(120), "doubles: {gaps:?}");
    // Each failed attempt is kept as an error reply; none of them reaches the next request.
    let errors = texts(&e.snapshot(&s))
        .into_iter()
        .filter(|t| t.0 == "assistant")
        .count();
    assert_eq!(errors, 3);
    assert_eq!(user_texts(&provider.requests()[2]), ["x"]);

    // Retry disabled: one attempt.
    let fx = Fixture::new();
    with_settings(&fx, json!({"retry": {"enabled": false, "baseDelayMs": 1}}));
    let (provider, _) = timed(|_| R::Error("503: overloaded".into()));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "x").unwrap();
    assert!(matches!(e.settled("r1"), RunOutcome::ModelError { .. }));
    assert_eq!(provider.requests().len(), 1);
}

#[test]
fn stop_during_a_retry_wait_settles_aborted_promptly() {
    let fx = Fixture::new();
    with_settings(&fx, json!({"retry": {"baseDelayMs": 30000}}));
    let (provider, _) = timed(|_| R::Error("503: overloaded".into()));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "x").unwrap();
    e.until("the failed attempt is recorded", |ev| {
        ev.iter().any(|e| matches!(e, Event::Session { change: SessionChange::EntryAppended { entry }, .. }
            if matches!(&entry.kind, EntryKind::Message { message: Message::Assistant { stop_reason: StopReason::Error, .. }, .. })))
    });
    let started = Instant::now();
    e.call(Call::Stop { session: s.clone() }).unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Aborted);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(provider.requests().len(), 1, "no retry after Stop");
}

fn retry_checkpoint(
    e: &Engine,
    session: &str,
    failures: u32,
) -> (u64, crate::store::GenerationState) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let store = &e.handle.shared().store;
        if let Some(run) = store.open_run(session).unwrap() {
            let state = store.read(|c| Store::generation_state(c, run.id)).unwrap();
            if state.failures == failures && state.retry_until.is_some() {
                return (run.id, state);
            }
        }
        assert!(Instant::now() < deadline, "no retry checkpoint");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn retry_deadline_and_attempt_budget_survive_a_restart() {
    let fx = Fixture::new();
    with_settings(&fx, json!({"retry":{"maxRetries":1,"baseDelayMs":1500}}));
    let first = Scripted::new(|_, _| R::Error("503: overloaded".into()));
    let s;
    let until;
    {
        let mut e = fx.start(&first);
        s = e.create(&fx.project, "c");
        e.prompt(&s, "r1", "x").unwrap();
        until = retry_checkpoint(&e, &s, 1).1.retry_until.unwrap();
        e.handle.shutdown(Duration::from_millis(100));
    }
    let calls = Arc::new(Mutex::new(Vec::new()));
    let at = calls.clone();
    let retry = Scripted::new(move |_, _| {
        at.lock().unwrap().push(crate::util::now_ms());
        R::Error("503: overloaded".into())
    });
    let mut e = fx.start(&retry);
    assert!(matches!(
        settled_by_lookup(&mut e, &s, "r1"),
        RunOutcome::ModelError { .. }
    ));
    let times = calls.lock().unwrap();
    assert_eq!(first.requests().len(), 1);
    assert_eq!(
        times.len(),
        1,
        "restart must not replenish the retry budget"
    );
    assert!(times[0] >= until, "request before the committed deadline");
    assert_eq!(user_texts(&retry.requests()[0]), ["x"]);
}

#[test]
fn configured_retry_cap_bounds_the_durable_runtime_deadline() {
    let fx = Fixture::new();
    with_settings(
        &fx,
        json!({"retry": {
            "maxRetries": 2, "baseDelayMs": 30000, "maxAgentDelayMs": 1500
        }}),
    );
    let provider = Scripted::new(|_, _| R::Error("503: overloaded".into()));
    let mut e = fx.start(&provider);
    let session = e.create(&fx.project, "capped");
    let before = crate::util::now_ms();
    e.prompt(&session, "capped", "go").unwrap();
    let state = retry_checkpoint(&e, &session, 1).1;
    let after = crate::util::now_ms();
    let deadline = state.retry_until.unwrap();
    assert!(deadline >= before + 1500, "cap must not eliminate the wait");
    assert!(
        deadline <= after + 1500,
        "runtime must apply the configured cap"
    );
    e.call(Call::Stop { session }).unwrap();
    assert_eq!(e.settled("capped"), RunOutcome::Aborted);
    assert_eq!(provider.requests().len(), 1);
}

#[test]
fn an_expired_retry_deadline_does_not_restart_the_delay() {
    let fx = Fixture::new();
    with_settings(&fx, json!({"retry":{"baseDelayMs":30000}}));
    let first = Scripted::new(|_, _| R::Error("503: overloaded".into()));
    let s;
    {
        let mut e = fx.start(&first);
        s = e.create(&fx.project, "c");
        e.prompt(&s, "r1", "x").unwrap();
        let (run, mut state) = retry_checkpoint(&e, &s, 1);
        state.retry_until = Some(crate::util::now_ms() - 1000);
        e.handle
            .shared()
            .store
            .tx(|tx| Store::save_generation_state(tx, run, &state))
            .unwrap();
        e.handle.shutdown(Duration::from_millis(100));
    }
    let answer = Scripted::echo("after deadline");
    let start = Instant::now();
    let mut e = fx.start(&answer);
    assert_eq!(settled_by_lookup(&mut e, &s, "r1"), RunOutcome::Completed);
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "did not start a fresh 30-second delay"
    );
    assert_eq!(answer.requests().len(), 1);
}

#[test]
fn stop_during_a_restored_retry_wait_retires_the_run_without_another_request() {
    let fx = Fixture::new();
    with_settings(&fx, json!({"retry":{"baseDelayMs":30000}}));
    let provider = Scripted::new(|_, _| R::Error("503: overloaded".into()));
    let s;
    {
        let mut e = fx.start(&provider);
        s = e.create(&fx.project, "c");
        e.prompt(&s, "r1", "retire me").unwrap();
        retry_checkpoint(&e, &s, 1);
        e.handle.shutdown(Duration::from_millis(100));
    }
    let answer = Scripted::echo("next");
    {
        let mut e = fx.start(&answer);
        let start = Instant::now();
        e.call(Call::Stop { session: s.clone() }).unwrap();
        assert_eq!(settled_by_lookup(&mut e, &s, "r1"), RunOutcome::Aborted);
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(answer.requests().is_empty());
        assert!(e.snapshot(&s).entries.iter().any(|e| e.retired));
        e.handle.shutdown(Duration::from_millis(100));
    }
    let mut e = fx.start(&answer);
    assert!(answer.requests().is_empty());
    e.prompt(&s, "r2", "new instruction").unwrap();
    assert_eq!(e.settled("r2"), RunOutcome::Completed);
    assert_eq!(user_texts(&answer.requests()[0]), ["new instruction"]);
}

#[test]
fn a_successful_tool_generation_resets_the_retry_budget() {
    let fx = Fixture::new();
    with_settings(&fx, json!({"retry":{"maxRetries":1,"baseDelayMs":1}}));
    let provider = Scripted::new(|_, n| match n {
        0 | 2 => R::Error("503: overloaded".into()),
        1 => R::Tools(None, vec![("bash".into(), json!({"command":"true"}))]),
        _ => R::Text("done".into()),
    });
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "work").unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    assert_eq!(provider.requests().len(), 4);
}

#[test]
fn a_failed_retry_checkpoint_fails_closed_and_reclassifies_the_saved_error_once() {
    let fx = Fixture::new();
    with_settings(&fx, json!({"retry":{"maxRetries":1,"baseDelayMs":50}}));
    let provider = Scripted::new(|_, _| R::Error("503: overloaded".into()));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.handle
        .shared()
        .store
        .inject(Fault::RetryCheckpoint)
        .unwrap();
    e.prompt(&s, "r1", "work").unwrap();
    e.until("retry checkpoint failed closed", |ev| {
        storage_notice(ev).is_some()
    });
    assert_eq!(provider.requests().len(), 1);
    assert!(matches!(
        lookup(&mut e, &s, "r1"),
        LookupResult::Known { open: true, .. }
    ));
    assert_eq!(
        e.prompt(&s, "r2", "more").unwrap_err().code,
        ErrorCode::Storage
    );
    e.handle.shared().store.clear_faults().unwrap();
    assert!(matches!(
        reopen_until_settled(&mut e, &s, "r1"),
        RunOutcome::ModelError { .. }
    ));
    assert_eq!(
        provider.requests().len(),
        2,
        "saved failure counted once after reopen"
    );
}
