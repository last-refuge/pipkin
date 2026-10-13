//! Per-child controls (SUB-2) and background children (SUB-3) through the client contract:
//! a person's Stop settles through the child and tells the waiting parent; Steer reaches the
//! running child at its next step; both are authorized by the publishing session; `wait:
//! false` returns a receipt, the completion is delivered exactly once (also across a
//! restart) and suppressed when the launching run was stopped; person-launched children
//! never wake the model; the lookup tools read durable rows.

use std::time::{Duration, Instant};

use pipkin_engine_protocol::*;
use serde_json::json;

use super::tests::{Engine, Fixture};
use crate::provider::ProviderRequest;
use crate::provider::scripted::{Gate, Reply as R, Scripted};

fn first_user(req: &ProviderRequest) -> String {
    req.messages
        .iter()
        .find_map(|m| match m {
            Message::User { .. } => Some(m.text()),
            _ => None,
        })
        .unwrap_or_default()
}

fn last_user(req: &ProviderRequest) -> String {
    req.messages
        .iter()
        .rev()
        .find_map(|m| match m {
            Message::User { .. } => Some(m.text()),
            _ => None,
        })
        .unwrap_or_default()
}

fn last_is_user(req: &ProviderRequest) -> bool {
    matches!(
        req.messages
            .iter()
            .rev()
            .find(|m| !matches!(m, Message::System(_))),
        Some(Message::User { .. })
    )
}

fn last_tool_result(req: &ProviderRequest) -> Option<String> {
    req.messages.iter().rev().find_map(|m| match m {
        Message::ToolResult { .. } => Some(m.text()),
        _ => None,
    })
}

fn is_child(req: &ProviderRequest) -> bool {
    first_user(req).starts_with("child:")
}

/// Parent prompts are commands: `delegate <json>` calls `subagent`; `delegate-hold <json>`
/// does the same and then holds its reply at the parent gate; `lookup <tool> <json>` calls a
/// lookup tool. Anything else (a completion follow-up) is acknowledged. A child's first
/// user text names its behaviour: `child:hang` answers behind the child gate, `child:steer`
/// calls a tool behind the gate and then repeats the newest user text, `child:none`
/// answers at once.
fn script(
    child_gate: Gate,
    parent_gate: Gate,
) -> impl Fn(&ProviderRequest, usize) -> R + Send + Sync {
    move |req, _| {
        if is_child(req) {
            let kind = first_user(req)
                .trim_start_matches("child:")
                .split_whitespace()
                .next()
                .unwrap_or("none")
                .to_owned();
            return match kind.as_str() {
                "hang" => R::Gated(child_gate.clone(), Box::new(R::Text("child done".into()))),
                "write" => {
                    // Two successful writes of the same file in consecutive rounds (a
                    // round's calls run in parallel, so the order of two writes to one
                    // path within a round is not defined), plus a failed write.
                    let results = req
                        .messages
                        .iter()
                        .filter(|m| matches!(m, Message::ToolResult { .. }))
                        .count();
                    match results {
                        0 => R::Tools(
                            None,
                            vec![(
                                "write".into(),
                                json!({"path": "out.txt", "content": "made by child"}),
                            )],
                        ),
                        1 => R::Tools(
                            None,
                            vec![
                                (
                                    "write".into(),
                                    json!({"path": "out.txt", "content": "made by child again"}),
                                ),
                                (
                                    "write".into(),
                                    json!({"path": "/proc/pipkin-cannot-write/x", "content": "no"}),
                                ),
                            ],
                        ),
                        _ => R::Text(format!(
                            "long answer start {} long answer end",
                            "x".repeat(20_000)
                        )),
                    }
                }
                "steer" => match last_tool_result(req) {
                    None => R::Gated(
                        child_gate.clone(),
                        Box::new(R::Tools(
                            None,
                            vec![("list_files".into(), json!({"path": "."}))],
                        )),
                    ),
                    Some(_) => R::Text(format!("child end: {}", last_user(req))),
                },
                _ => R::Text("child done".into()),
            };
        }
        let text = last_user(req);
        if last_is_user(req) {
            let (cmd, args) = text.split_once(' ').unwrap_or((text.as_str(), "{}"));
            return match cmd {
                "delegate" | "delegate-hold" => R::Tools(
                    None,
                    vec![("subagent".into(), serde_json::from_str(args).unwrap())],
                ),
                "lookup" => {
                    let (tool, args) = args.split_once(' ').unwrap_or((args, "{}"));
                    R::Tools(
                        None,
                        vec![(tool.to_owned(), serde_json::from_str(args).unwrap())],
                    )
                }
                _ => R::Text(format!("noted: {text}")),
            };
        }
        let saw = format!("parent saw: {}", last_tool_result(req).unwrap_or_default());
        if first_user(req).starts_with("delegate-hold") {
            R::Gated(parent_gate.clone(), Box::new(R::Text(saw)))
        } else {
            R::Text(saw)
        }
    }
}

fn child_requests(p: &Scripted) -> Vec<ProviderRequest> {
    p.requests().into_iter().filter(is_child).collect()
}

fn subagents(e: &mut Engine, s: &str) -> SubagentsView {
    match e.call(Call::Subagents { session: s.into() }) {
        Ok(Reply::Subagents(v)) => v,
        other => panic!("{other:?}"),
    }
}

fn row(e: &mut Engine, s: &str, child: u64) -> SubagentRow {
    subagents(e, s)
        .children
        .into_iter()
        .find(|r| r.child == child)
        .unwrap_or_else(|| panic!("no child {child}"))
}

fn until_row(
    e: &mut Engine,
    s: &str,
    child: u64,
    what: &str,
    done: impl Fn(&SubagentRow) -> bool,
) -> SubagentRow {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let r = row(e, s, child);
        if done(&r) {
            return r;
        }
        assert!(
            Instant::now() < deadline,
            "child {child} never {what}: {r:?}"
        );
        std::thread::sleep(Duration::from_millis(30));
    }
}

fn stop(e: &mut Engine, s: &str, child: u64, request: &str) -> Result<SubagentRow, ProtoError> {
    match e.call(Call::StopSubagent {
        session: s.into(),
        child,
        request: request.into(),
    }) {
        Ok(Reply::Subagent(r)) => Ok(r),
        Ok(other) => panic!("{other:?}"),
        Err(e) => Err(e),
    }
}

fn steer(
    e: &mut Engine,
    s: &str,
    child: u64,
    request: &str,
    text: &str,
) -> Result<SubagentRow, ProtoError> {
    match e.call(Call::SteerSubagent {
        session: s.into(),
        child,
        request: request.into(),
        text: text.into(),
    }) {
        Ok(Reply::Subagent(r)) => Ok(r),
        Ok(other) => panic!("{other:?}"),
        Err(e) => Err(e),
    }
}

fn launch(e: &mut Engine, s: &str, request: &str, task: &str) -> Result<SubagentRow, ProtoError> {
    match e.call(Call::LaunchSubagent {
        session: s.into(),
        request: request.into(),
        task: task.into(),
        profile: None,
        context: None,
    }) {
        Ok(Reply::Subagent(r)) => Ok(r),
        Ok(other) => panic!("{other:?}"),
        Err(e) => Err(e),
    }
}

/// The parent's newest tool result: `(is_error, text)`.
fn parent_result(snap: &SessionSnapshot) -> (bool, String) {
    snap.entries
        .iter()
        .filter_map(|e| match &e.kind {
            EntryKind::Message {
                message: m @ Message::ToolResult { is_error, .. },
                ..
            } => Some((*is_error, m.text())),
            _ => None,
        })
        .next_back()
        .unwrap()
}

fn user_texts(snap: &SessionSnapshot) -> Vec<String> {
    snap.entries
        .iter()
        .filter_map(|e| match &e.kind {
            EntryKind::Message {
                message: m @ Message::User { .. },
                ..
            } => Some(m.text()),
            _ => None,
        })
        .collect()
}

fn woke(e: &mut Engine, child: u64) {
    let key = format!("subagent:{child}:done");
    e.until("the completion follow-up settled", |ev| {
        ev.iter().any(|e| matches!(e, Event::Session { change: SessionChange::RunSettled { request: Some(r), .. }, .. } if *r == key))
    });
}

#[test]
fn a_persons_stop_settles_the_child_and_tells_the_waiting_parent() {
    let fx = Fixture::new();
    let (child_gate, parent_gate) = (Gate::new(), Gate::new());
    let provider = Scripted::new(script(child_gate.clone(), parent_gate));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", r#"delegate {"task":"child:hang"}"#)
        .unwrap();
    e.until("the child asked its model", |_| {
        !child_requests(&provider).is_empty()
    });
    let r = row(&mut e, &s, 1);
    assert_eq!(
        (r.status.as_str(), r.background, r.origin.as_str()),
        ("running", false, "model")
    );
    // Another session cannot act on this child by number.
    let other = e.create(&fx.project, "other");
    assert_eq!(
        stop(&mut e, &other, 1, "x").unwrap_err().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        steer(&mut e, &other, 1, "x", "hi").unwrap_err().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        stop(&mut e, &s, 7, "x").unwrap_err().code,
        ErrorCode::NotFound
    );
    // The Stop: the note is on record at once; the status settles through the child.
    let stopped = stop(&mut e, &s, 1, "stop-1").unwrap();
    assert_eq!(
        stopped.note.as_deref(),
        Some(super::subagents::STOPPED_BY_PERSON)
    );
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let snap = e.snapshot(&s);
    let (is_error, text) = parent_result(&snap);
    assert!(is_error, "{text}");
    assert!(
        text.contains("stopped before it answered") && text.contains("Stopped by the person"),
        "{text}"
    );
    let r = until_row(&mut e, &s, 1, "settled", |r| r.status == "aborted");
    assert_eq!(r.note.as_deref(), Some("Stopped by the person"));
    assert!(!r.has_result);
    // Repeating the request changes nothing; a finished child cannot be steered.
    let again = stop(&mut e, &s, 1, "stop-1").unwrap();
    assert_eq!(again.status, "aborted");
    assert_eq!(
        steer(&mut e, &s, 1, "st", "more").unwrap_err().code,
        ErrorCode::Invalid
    );
    let Ok(Reply::SubagentResult(result)) = e.call(Call::SubagentResult {
        session: s.clone(),
        child: 1,
    }) else {
        panic!()
    };
    assert_eq!((result.status.as_str(), result.result), ("aborted", None));
    assert_eq!(
        child_requests(&provider).len(),
        1,
        "the child was not asked again"
    );
}

#[test]
fn a_steering_message_reaches_the_running_child_once() {
    let fx = Fixture::new();
    let (child_gate, parent_gate) = (Gate::new(), Gate::new());
    let provider = Scripted::new(script(child_gate.clone(), parent_gate));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", r#"delegate {"task":"child:steer"}"#)
        .unwrap();
    e.until("the child asked its model", |_| {
        !child_requests(&provider).is_empty()
    });
    let r = steer(&mut e, &s, 1, "st-1", "use the other file").unwrap();
    assert_eq!(r.status, "running");
    // The same request key admits nothing new.
    steer(&mut e, &s, 1, "st-1", "use the other file").unwrap();
    child_gate.open();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let (is_error, text) = parent_result(&e.snapshot(&s));
    assert!(
        !is_error && text.contains("child end:") && text.contains("use the other file"),
        "{text}"
    );
    let last = child_requests(&provider).into_iter().last().unwrap();
    let steers = last
        .messages
        .iter()
        .filter(|m| matches!(m, Message::User { .. }) && m.text().contains("use the other file"))
        .count();
    assert_eq!(steers, 1);
    assert_eq!(row(&mut e, &s, 1).status, "done");
}

#[test]
fn a_background_child_returns_a_receipt_and_wakes_the_parent_exactly_once() {
    let fx = Fixture::new();
    let (child_gate, parent_gate) = (Gate::new(), Gate::new());
    let provider = Scripted::new(script(child_gate, parent_gate));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", r#"delegate {"task":"child:none","wait":false}"#)
        .unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let (is_error, text) = parent_result(&e.snapshot(&s));
    assert!(
        !is_error && text.contains("started in the background") && text.contains("follow-up"),
        "{text}"
    );
    let r = until_row(&mut e, &s, 1, "delivered", |r| r.delivery == "woken");
    assert_eq!(
        (r.status.as_str(), r.background, r.has_result),
        ("done", true, true)
    );
    woke(&mut e, 1);
    let users = user_texts(&e.snapshot(&s));
    assert_eq!(users.len(), 2, "{users:?}");
    assert!(
        users[1].starts_with("Background subagent #1 (child:none) finished.")
            && users[1].contains("Answer:\nchild done"),
        "{}",
        users[1]
    );
    // Lookups read the durable row.
    e.prompt(&s, "r2", r#"lookup subagent_result {"child":1}"#)
        .unwrap();
    assert_eq!(e.settled("r2"), RunOutcome::Completed);
    let (is_error, text) = parent_result(&e.snapshot(&s));
    assert!(
        !is_error && text.contains("Subagent #1 finished.") && text.contains("child done"),
        "{text}"
    );
    e.prompt(&s, "r3", "lookup subagent_status {}").unwrap();
    assert_eq!(e.settled("r3"), RunOutcome::Completed);
    let (_, text) = parent_result(&e.snapshot(&s));
    assert!(
        text.contains("#1 finished: child:none (background) — answer available"),
        "{text}"
    );
    assert_eq!(
        e.call(Call::SubagentResult {
            session: s.clone(),
            child: 2
        })
        .unwrap_err()
        .code,
        ErrorCode::NotFound
    );
    // A restart delivers nothing again.
    let before = user_texts(&e.snapshot(&s)).len();
    e.handle.shutdown(Duration::from_millis(300));
    drop(e);
    let provider2 = Scripted::new(script(Gate::new(), Gate::new()));
    let mut e = fx.start(&provider2);
    e.snapshot(&s);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(user_texts(&e.snapshot(&s)).len(), before, "no second wake");
    assert_eq!(row(&mut e, &s, 1).delivery, "woken");
}

#[test]
fn a_notify_only_child_is_waited_for_and_never_wakes_the_model() {
    let fx = Fixture::new();
    let (child_gate, parent_gate) = (Gate::new(), Gate::new());
    let provider = Scripted::new(script(child_gate.clone(), parent_gate));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(
        &s,
        "r1",
        r#"delegate {"task":"child:hang","wait":false,"wake":false}"#,
    )
    .unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let (_, text) = parent_result(&e.snapshot(&s));
    assert!(text.contains("will not interrupt you"), "{text}");
    assert_eq!(row(&mut e, &s, 1).delivery, "notify");
    e.prompt(&s, "r2", r#"lookup subagent_wait {"child":1,"timeout":1}"#)
        .unwrap();
    assert_eq!(e.settled("r2"), RunOutcome::Completed);
    let (is_error, text) = parent_result(&e.snapshot(&s));
    assert!(!is_error && text.contains("Still running"), "{text}");
    child_gate.open();
    e.prompt(&s, "r3", r#"lookup subagent_wait {"child":1,"timeout":30}"#)
        .unwrap();
    assert_eq!(e.settled("r3"), RunOutcome::Completed);
    let (_, text) = parent_result(&e.snapshot(&s));
    assert!(
        text.contains("finished") && text.contains("child done"),
        "{text}"
    );
    let r = until_row(&mut e, &s, 1, "delivered", |r| r.delivery == "notified");
    assert_eq!(r.status, "done");
    let users = user_texts(&e.snapshot(&s));
    assert!(
        users.iter().all(|u| !u.starts_with("Background subagent")),
        "{users:?}"
    );
}

#[test]
fn a_person_launched_child_runs_in_the_background_and_never_wakes_the_model() {
    let fx = Fixture::new();
    let provider = Scripted::new(script(Gate::new(), Gate::new()));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    assert_eq!(
        launch(&mut e, &s, "L0", "   ").unwrap_err().code,
        ErrorCode::Invalid
    );
    let r = launch(&mut e, &s, "L1", "child:none").unwrap();
    assert_eq!(
        (r.origin.as_str(), r.background, r.delivery.as_str()),
        ("user", true, "notify")
    );
    assert_eq!(r.call_id, "user:L1");
    // The same request is the same child.
    let again = launch(&mut e, &s, "L1", "child:none").unwrap();
    assert_eq!(again.child, r.child);
    assert_eq!(subagents(&mut e, &s).children.len(), 1);
    let done = until_row(&mut e, &s, r.child, "delivered", |r| {
        r.delivery == "notified"
    });
    assert_eq!((done.status.as_str(), done.has_result), ("done", true));
    let Ok(Reply::SubagentResult(result)) = e.call(Call::SubagentResult {
        session: s.clone(),
        child: r.child,
    }) else {
        panic!()
    };
    assert_eq!(result.result.as_deref(), Some("child done"));
    assert!(
        provider.requests().iter().all(is_child),
        "the parent's model was never asked"
    );
    assert!(user_texts(&e.snapshot(&s)).is_empty());
}

#[test]
fn a_restart_reattaches_a_background_child_and_delivers_once() {
    let fx = Fixture::new();
    let (child_gate, parent_gate) = (Gate::new(), Gate::new());
    let provider = Scripted::new(script(child_gate, parent_gate));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", r#"delegate {"task":"child:hang","wait":false}"#)
        .unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    e.until("the child asked its model", |_| {
        !child_requests(&provider).is_empty()
    });
    e.handle.shutdown(Duration::from_millis(300));
    drop(e);
    let open = Gate::new();
    open.open();
    let provider2 = Scripted::new(script(open, Gate::new()));
    let mut e = fx.start(&provider2);
    e.snapshot(&s);
    let r = until_row(&mut e, &s, 1, "delivered after the restart", |r| {
        r.delivery == "woken"
    });
    assert_eq!(r.status, "done");
    woke(&mut e, 1);
    let users = user_texts(&e.snapshot(&s));
    assert_eq!(users.len(), 2, "{users:?}");
    assert!(users[1].contains("child done"), "{}", users[1]);
}

#[test]
fn stopping_the_launching_run_suppresses_the_wake_but_keeps_the_result() {
    let fx = Fixture::new();
    let (child_gate, parent_gate) = (Gate::new(), Gate::new());
    let provider = Scripted::new(script(child_gate.clone(), parent_gate));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(
        &s,
        "r1",
        r#"delegate-hold {"task":"child:hang","wait":false}"#,
    )
    .unwrap();
    e.until("the parent holds its reply after the receipt", |_| {
        provider
            .requests()
            .iter()
            .any(|r| !is_child(r) && last_tool_result(r).is_some())
    });
    e.call(Call::Stop { session: s.clone() }).unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Aborted);
    assert_eq!(
        row(&mut e, &s, 1).status,
        "running",
        "a background child outlives the run"
    );
    child_gate.open();
    let r = until_row(&mut e, &s, 1, "delivered", |r| r.delivery == "suppressed");
    assert_eq!((r.status.as_str(), r.has_result), ("done", true));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(user_texts(&e.snapshot(&s)).len(), 1, "no follow-up");
}

#[test]
fn a_long_answer_is_delivered_bounded_with_artifact_references_and_read_by_offset_or_find() {
    let fx = Fixture::new();
    let provider = Scripted::new(script(Gate::new(), Gate::new()));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", r#"delegate {"task":"child:write"}"#)
        .unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    assert_eq!(
        std::fs::read_to_string(fx.project.join("out.txt")).unwrap(),
        "made by child again"
    );
    // The waiting parent got the first 16 KiB, a pointer to the rest, and the artifacts:
    // the file once (two successful writes, one failed write), no sources.
    let (is_error, text) = parent_result(&e.snapshot(&s));
    assert!(!is_error, "{}", &text[..80]);
    assert!(text.starts_with("long answer start xxx"), "{}", &text[..80]);
    assert!(!text.contains("long answer end"), "the tail is not inline");
    assert!(
        text.contains("[Answer continues: 16384 of 20034 characters shown; read the rest with subagent_result child 1 offset 16384.]"),
        "{}",
        &text[text.len() - 300..]
    );
    let file = fx.project.join("out.txt").display().to_string();
    assert!(
        text.ends_with(&format!("\nArtifacts:\n- file: {file}")),
        "{}",
        &text[text.len() - 200..]
    );
    assert!(
        !text.contains("pipkin-cannot-write"),
        "a failed write is not an artifact"
    );
    // Reading by offset continues where the inline answer stopped.
    e.prompt(
        &s,
        "r2",
        r#"lookup subagent_result {"child":1,"offset":16384}"#,
    )
    .unwrap();
    assert_eq!(e.settled("r2"), RunOutcome::Completed);
    let (is_error, text) = parent_result(&e.snapshot(&s));
    assert!(!is_error, "{}", &text[..80]);
    assert!(
        text.contains("Answer (characters 16384-20034 of 20034):")
            && text.contains("long answer end"),
        "{}",
        &text[..120]
    );
    assert!(!text.contains("More follows"), "{text}");
    assert!(text.contains(&format!("- file: {file}")), "{text}");
    // `find` locates text anywhere in the answer without returning the slice.
    e.prompt(
        &s,
        "r3",
        r#"lookup subagent_result {"child":1,"find":"ANSWER END"}"#,
    )
    .unwrap();
    assert_eq!(e.settled("r3"), RunOutcome::Completed);
    let (is_error, text) = parent_result(&e.snapshot(&s));
    assert!(!is_error, "{text}");
    assert!(
        text.contains("\"ANSWER END\" occurs 1 time(s) in its answer") && text.contains("@20024:"),
        "{text}"
    );
    assert!(
        text.chars().count() < 600,
        "no slice with a find: {}",
        text.len()
    );
    assert!(
        text.contains(&format!("\nArtifacts:\n- file: {file}")),
        "{text}"
    );
    // The client call carries the bounded slice and the artifacts too.
    let Ok(Reply::SubagentResult(result)) = e.call(Call::SubagentResult {
        session: s.clone(),
        child: 1,
    }) else {
        panic!()
    };
    assert_eq!(result.result_chars, 20034);
    assert_eq!(result.result.as_ref().unwrap().chars().count(), 16384);
    assert!(result.result_truncated);
    assert_eq!(result.artifacts.len(), 1);
    assert_eq!(
        (
            result.artifacts[0].kind.as_str(),
            result.artifacts[0].reference.as_str()
        ),
        ("file", file.as_str())
    );
}
