//! Background jobs through the model's tools and the client calls: a start returns while the
//! command runs and the turn continues; logs are read by byte cursor with a quota; cancel
//! kills the process group including grandchildren; timeouts; completion delivery (one
//! follow-up for a model job that asked to be woken, none for a user job, suppressed after a
//! Stop); restart reconciliation (interrupted with its orphan killed, unknown without a
//! receipt); shutdown stops jobs; the `Jobs`/`JobLog`/`JobCancel`/`JobStart` calls.

use std::time::Duration;

use pipkin_engine_protocol::*;
use serde_json::json;

use super::tests::{Engine, Fixture};
use crate::provider::ProviderRequest;
use crate::provider::scripted::{Reply as R, Scripted};
use crate::store::Store;

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

/// After a user message `tool args-json [; tool args-json ...]`, call those tools in
/// sequence (one per model turn); then echo the last result.
fn calling_from_prompt() -> impl Fn(&ProviderRequest, usize) -> R + Send + Sync {
    move |req, _| {
        let text = req
            .messages
            .iter()
            .rev()
            .find_map(|m| match m {
                Message::User { .. } => Some(m.text()),
                _ => None,
            })
            .unwrap_or_default();
        // Only prompts written as tool steps are scripted; a completion follow-up from the
        // engine gets a plain answer.
        if !text.starts_with("job_") {
            return R::Text("noted".into());
        }
        let steps: Vec<&str> = text.split(" ;; ").collect();
        let done = req
            .messages
            .iter()
            .rev()
            .take_while(|m| !matches!(m, Message::User { .. }))
            .filter(|m| matches!(m, Message::ToolResult { .. }))
            .count();
        if last_is_user(req) || done < steps.len() {
            let step = steps[done.min(steps.len() - 1)];
            let (tool, args) = step.split_once(' ').unwrap_or((step, "{}"));
            R::Tools(
                None,
                vec![(tool.to_owned(), serde_json::from_str(args).unwrap())],
            )
        } else {
            R::Text(format!(
                "model saw: {}",
                last_tool_result(req).unwrap_or_default()
            ))
        }
    }
}

fn tool_results(snap: &SessionSnapshot) -> Vec<(String, bool, String, Option<serde_json::Value>)> {
    snap.entries
        .iter()
        .filter_map(|e| match &e.kind {
            EntryKind::Message {
                message:
                    m @ Message::ToolResult {
                        tool_name,
                        is_error,
                        extra,
                        ..
                    },
                ..
            } if tool_name.starts_with("job_") => Some((
                tool_name.clone(),
                *is_error,
                m.text(),
                extra.get("details").cloned(),
            )),
            _ => None,
        })
        .collect()
}

fn run(e: &mut Engine, s: &str, key: &str, prompt: &str) -> (bool, String, serde_json::Value) {
    e.prompt(s, key, prompt).unwrap();
    assert_eq!(e.settled(key), RunOutcome::Completed);
    let snap = e.snapshot(s);
    let results = tool_results(&snap);
    let Some((name, is_error, text, details)) = results.last().cloned() else {
        panic!(
            "no job tool result after {key}; entries: {:?}",
            snap.entries
                .iter()
                .map(|e| format!("{:?}", e.kind))
                .collect::<Vec<_>>()
        );
    };
    // Refusals carry no details; successes always do.
    let details = match details {
        Some(d) => d["job"].clone(),
        None if is_error => serde_json::Value::Null,
        None => panic!("{name} after {key} had no details: {text:?}; all results: {results:?}"),
    };
    (is_error, text, details)
}

fn jobs(e: &mut Engine, s: &str) -> JobsView {
    match e.call(Call::Jobs { session: s.into() }) {
        Ok(Reply::Jobs(v)) => v,
        other => panic!("{other:?}"),
    }
}

fn job(e: &mut Engine, s: &str, id: u64) -> JobView {
    jobs(e, s)
        .jobs
        .into_iter()
        .find(|j| j.id == id)
        .unwrap_or_else(|| panic!("no job {id}"))
}

fn wait_state(e: &mut Engine, s: &str, id: u64, state: JobState) -> JobView {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        let j = jobs(e, s).jobs.into_iter().find(|j| j.id == id);
        if let Some(j) = &j
            && j.state == state
        {
            return j.clone();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "job {id} never reached {state:?}: {j:?}"
        );
        std::thread::sleep(Duration::from_millis(30));
    }
}

fn log(e: &mut Engine, s: &str, id: u64, offset: u64, limit: Option<u64>) -> JobLogView {
    match e.call(Call::JobLog {
        session: s.into(),
        id,
        offset,
        limit,
    }) {
        Ok(Reply::JobLog(v)) => v,
        other => panic!("{other:?}"),
    }
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

#[cfg(target_os = "linux")]
fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map(|s| !s.contains(") Z "))
        .unwrap_or(false)
}

#[test]
fn a_job_starts_at_once_runs_beside_the_turn_and_is_read_by_cursor() {
    let fx = Fixture::new();
    let provider = Scripted::new(calling_from_prompt());
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    let marker = fx.project.join("started");
    let prompt = format!(
        "job_start {} ;; job_status {{}}",
        json!({"command": format!("echo begin; touch {}; sleep 0.2; echo oops >&2; sleep 0.2; echo end", marker.display()), "name": "slow"})
    );
    let started = std::time::Instant::now();
    let (is_error, text, details) = run(&mut e, &s, "r1", &prompt);
    assert!(!is_error, "{text}");
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(details["outcome"], "status");
    let snap = e.snapshot(&s);
    let results = tool_results(&snap);
    assert_eq!(results[0].0, "job_start");
    assert!(
        results[0].2.starts_with("Started job #1 (slow) in"),
        "{}",
        results[0].2
    );
    assert_eq!(results[0].3.as_ref().unwrap()["job"]["job"]["id"], 1);
    assert!(
        results[1].2.starts_with("#1 slow: is running")
            || results[1].2.starts_with("#1 slow: succeeded"),
        "{}",
        results[1].2
    );
    let finished = wait_state(&mut e, &s, 1, JobState::Succeeded);
    assert_eq!(finished.exit_code, Some(0));
    assert_eq!(finished.origin, "model");
    assert!(finished.started_at_ms.is_some() && finished.ended_at_ms.is_some());
    assert!(marker.exists());
    // The log holds both streams in arrival order, read by byte offset with a continuation.
    let full = log(&mut e, &s, 1, 0, None);
    assert_eq!(full.text, "begin\noops\nend\n");
    assert_eq!(full.total_bytes, 15);
    assert!(full.done && !full.truncated);
    let part = log(&mut e, &s, 1, 6, Some(3));
    assert_eq!(
        (part.text.as_str(), part.offset, part.next_offset),
        ("oop", 6, 9)
    );
    let past = log(&mut e, &s, 1, 100, None);
    assert_eq!(
        (past.text.as_str(), past.offset, past.next_offset),
        ("", 15, 15)
    );
    // The same through the model's tools, including the tail form.
    let (is_error, text, details) = run(
        &mut e,
        &s,
        "r2",
        r#"job_log {"id": 1, "offset": 0, "limit": 5}"#,
    );
    assert!(!is_error, "{text}");
    assert!(
        text.starts_with("Job #1 log, bytes 0–5 of 15 (finished) — continue with offset 5\nbegin"),
        "{text}"
    );
    assert_eq!(details["nextOffset"], 5);
    let (is_error, text, _) = run(
        &mut e,
        &s,
        "r3",
        r#"job_log {"id": 1, "tail": true, "limit": 4}"#,
    );
    assert!(!is_error && text.ends_with("end\n"), "{text}");
    let (is_error, text, _) = run(&mut e, &s, "r4", r#"job_wait {"id": 1}"#);
    assert!(!is_error, "{text}");
    assert!(
        text.starts_with("#1 slow: succeeded (exit code 0)"),
        "{text}"
    );
    assert!(text.contains("Last output:\nbegin\noops\nend"), "{text}");
    let (is_error, text, _) = run(&mut e, &s, "r5", r#"job_status {"id": 9}"#);
    assert!(is_error && text.contains("No job #9"), "{text}");
    let (is_error, text, _) = run(&mut e, &s, "r6", r#"job_start {"command": ""}"#);
    assert!(is_error && text.contains("needs a command"), "{text}");
    // The snapshot carries the list, active first.
    let snap = e.snapshot(&s);
    assert_eq!(snap.jobs.jobs.len(), 1);
    assert_eq!(snap.jobs.active, 0);
    assert_eq!(snap.jobs.jobs[0].state, JobState::Succeeded);
}

#[test]
fn failures_timeouts_and_cancellation_are_reported_and_kill_grandchildren() {
    let fx = Fixture::new();
    let provider = Scripted::new(calling_from_prompt());
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    let (is_error, text, _) = run(
        &mut e,
        &s,
        "r1",
        r#"job_start {"command": "echo bad >&2; exit 3"} ;; job_wait {"id": 1}"#,
    );
    assert!(!is_error, "{text}");
    assert!(
        text.starts_with("#1 echo bad >&2; exit 3: failed (exit code 3)"),
        "{text}"
    );
    assert!(text.contains("Last output:\nbad"), "{text}");
    let (is_error, text, _) = run(
        &mut e,
        &s,
        "r2",
        r#"job_start {"command": "sleep 30", "timeout": 0.3} ;; job_wait {"id": 2, "timeout": 10}"#,
    );
    assert!(!is_error, "{text}");
    assert!(text.starts_with("#2 sleep 30: timed out"), "{text}");
    assert!(text.contains("Timed out after 0.3 seconds"), "{text}");
    // A grandchild is in the group: cancellation kills it too.
    let pid_file = fx.project.join("grandchild.pid");
    let command = format!("(sleep 60 & echo $! > {}; wait)", pid_file.display());
    let (is_error, text, _) = run(
        &mut e,
        &s,
        "r3",
        &format!(
            "job_start {}",
            json!({"command": command, "name": "nested"})
        ),
    );
    assert!(!is_error, "{text}");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !pid_file.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "grandchild never started"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let grandchild: u32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(job(&mut e, &s, 3).state, JobState::Running);
    let (is_error, text, details) = run(&mut e, &s, "r4", r#"job_cancel {"id": 3}"#);
    assert!(!is_error, "{text}");
    assert!(text.starts_with("#3 nested: was cancelled"), "{text}");
    assert_eq!(details["job"]["state"], "cancelled");
    #[cfg(target_os = "linux")]
    {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while alive(grandchild) {
            assert!(
                std::time::Instant::now() < deadline,
                "grandchild {grandchild} survived"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    let _ = grandchild;
    // Cancelling again is harmless; the client call reports the same.
    let (is_error, text, _) = run(&mut e, &s, "r5", r#"job_cancel {"id": 3}"#);
    assert!(
        !is_error && text.starts_with("#3 nested: was cancelled"),
        "{text}"
    );
    match e.call(Call::JobCancel {
        session: s.clone(),
        id: 3,
    }) {
        Ok(Reply::Job(j)) => assert_eq!(j.state, JobState::Cancelled),
        other => panic!("{other:?}"),
    }
    // Output beyond the quota is dropped and the log says so, while the job keeps running.
    let (is_error, text, _) = run(
        &mut e,
        &s,
        "r6",
        r#"job_start {"command": "head -c 9000000 /dev/zero | tr '\\0' 'x'; echo done"} ;; job_wait {"id": 4, "timeout": 30}"#,
    );
    assert!(!is_error, "{text}");
    assert!(text.contains("succeeded (exit code 0)"), "{text}");
    assert!(text.contains("(truncated at the quota)"), "{text}");
    let j = job(&mut e, &s, 4);
    assert_eq!(j.log_bytes, super::jobs::MAX_LOG_BYTES);
    assert!(j.log_truncated);
    // The per-session cap.
    for i in 0..super::jobs::MAX_ACTIVE_JOBS {
        e.call(Call::JobStart {
            session: s.clone(),
            request: format!("fill-{i}"),
            command: "sleep 30".into(),
            name: None,
            cwd: None,
            timeout_secs: None,
        })
        .unwrap();
    }
    let err = e
        .call(Call::JobStart {
            session: s.clone(),
            request: "one-too-many".into(),
            command: "sleep 30".into(),
            name: None,
            cwd: None,
            timeout_secs: None,
        })
        .unwrap_err();
    assert!(err.message.contains("may run at once"), "{}", err.message);
    assert_eq!(jobs(&mut e, &s).active, super::jobs::MAX_ACTIVE_JOBS);
}

#[test]
fn completion_wakes_the_model_once_when_asked_and_never_for_user_or_stopped_jobs() {
    let fx = Fixture::new();
    let provider = Scripted::new(calling_from_prompt());
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    // Model job with wake: the turn ends, the job finishes, one follow-up arrives.
    let (is_error, text, _) = run(
        &mut e,
        &s,
        "r1",
        r#"job_start {"command": "sleep 0.3; echo finished-output", "name": "build", "wake": true}"#,
    );
    assert!(
        !is_error && text.contains("you will get a follow-up"),
        "{text}"
    );
    let woken = wait_state(&mut e, &s, 1, JobState::Succeeded);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let j = job(&mut e, &s, 1);
        if j.delivery == "woken" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "never delivered: {j:?}"
        );
        std::thread::sleep(Duration::from_millis(30));
    }
    let _ = woken;
    e.until("the follow-up turn settled", |ev| {
        ev.iter().any(|e| matches!(e, Event::Session { change: SessionChange::RunSettled { request: Some(r), .. }, .. } if r == "job:1:done"))
    });
    let snap = e.snapshot(&s);
    let users = user_texts(&snap);
    assert_eq!(users.len(), 2, "{users:?}");
    assert!(
        users[1].starts_with("Background job #1 (build) succeeded (exit code 0) after"),
        "{}",
        users[1]
    );
    assert!(
        users[1].contains("Last output:\n```\nfinished-output\n```"),
        "{}",
        users[1]
    );
    assert!(users[1].contains("Use job_log with id 1"), "{}", users[1]);
    // A restart delivers nothing again.
    e.handle.shutdown(Duration::from_millis(300));
    drop(e);
    let provider2 = Scripted::new(calling_from_prompt());
    let mut e = fx.start(&provider2);
    e.snapshot(&s);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(
        user_texts(&e.snapshot(&s)).len(),
        2,
        "no second wake after restart"
    );
    assert_eq!(job(&mut e, &s, 1).delivery, "woken");
    // A user-started job never wakes the model.
    match e.call(Call::JobStart {
        session: s.clone(),
        request: "u1".into(),
        command: "echo user-job".into(),
        name: Some("mine".into()),
        cwd: None,
        timeout_secs: None,
    }) {
        Ok(Reply::Job(j)) => {
            assert_eq!(j.origin, "user");
            assert!(!j.wake);
            assert_eq!(j.id, 2);
        }
        other => panic!("{other:?}"),
    }
    // The same request key returns the same job, never a second one.
    match e.call(Call::JobStart {
        session: s.clone(),
        request: "u1".into(),
        command: "echo other".into(),
        name: None,
        cwd: None,
        timeout_secs: None,
    }) {
        Ok(Reply::Job(j)) => assert_eq!((j.id, j.command.as_str()), (2, "echo user-job")),
        other => panic!("{other:?}"),
    }
    wait_state(&mut e, &s, 2, JobState::Succeeded);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while job(&mut e, &s, 2).delivery == "pending" {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(30));
    }
    assert_eq!(job(&mut e, &s, 2).delivery, "notified");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(user_texts(&e.snapshot(&s)).len(), 2);
    // A model job whose run the person stopped: completion is recorded, no wake.
    e.prompt(&s, "r2", r#"job_start {"command": "sleep 0.5; echo late", "wake": true} ;; job_wait {"id": 3, "timeout": 30}"#)
        .unwrap();
    wait_state(&mut e, &s, 3, JobState::Running);
    e.call(Call::Stop { session: s.clone() }).unwrap();
    assert_eq!(e.settled("r2"), RunOutcome::Aborted);
    wait_state(&mut e, &s, 3, JobState::Succeeded);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while job(&mut e, &s, 3).delivery == "pending" {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(30));
    }
    assert_eq!(job(&mut e, &s, 3).delivery, "suppressed");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        user_texts(&e.snapshot(&s)).len(),
        3,
        "the stopped prompt only"
    );
    assert!(e.snapshot(&s).run.is_none());
}

#[test]
fn restarts_mark_orphans_interrupted_or_unknown_and_never_relaunch() {
    let fx = Fixture::new();
    let marker = fx.project.join("ran-count");
    let s;
    let pid;
    {
        let provider = Scripted::new(calling_from_prompt());
        let mut e = fx.start(&provider);
        s = e.create(&fx.project, "c");
        let command = format!("echo x >> {}; sleep 60", marker.display());
        let (is_error, text, _) = run(
            &mut e,
            &s,
            "r1",
            &format!("job_start {}", json!({"command": command, "name": "long"})),
        );
        assert!(!is_error, "{text}");
        let j = wait_state(&mut e, &s, 1, JobState::Running);
        pid = j.id;
        let _ = pid;
        // Orderly shutdown stops the job and says so.
        e.handle.shutdown(Duration::from_secs(2));
    }
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "x\n");
    {
        let provider = Scripted::new(calling_from_prompt());
        let mut e = fx.start(&provider);
        e.snapshot(&s);
        let j = job(&mut e, &s, 1);
        assert_eq!(j.state, JobState::Cancelled, "{j:?}");
        assert_eq!(j.delivery, "notified");
        // Simulate a supervisor that died: a running row from another engine epoch whose
        // process still carries the marker, and a starting row without a receipt.
        let shared = e.handle.shared();
        let sess = shared.session(&s).unwrap();
        let (token_row, orphan) = shared
            .commit(&sess, |tx| {
                let row = Store::job_create(
                    tx,
                    &s,
                    &crate::store::jobs::JobIntent {
                        name: Some("orphan"),
                        command: "sleep 60",
                        cwd: fx.project.to_str().unwrap(),
                        origin: "model",
                        call_id: None,
                        run: None,
                        task: None,
                        timeout_secs: None,
                        wake: true,
                        engine_epoch: 1,
                        nonce: "dead",
                        log_path: "",
                    },
                    0,
                )?;
                Store::job_receipt(tx, &s, row.id, 4242, 0)?;
                let unknown = Store::job_create(
                    tx,
                    &s,
                    &crate::store::jobs::JobIntent {
                        name: Some("maybe"),
                        command: "touch should-not-exist",
                        cwd: fx.project.to_str().unwrap(),
                        origin: "model",
                        call_id: None,
                        run: None,
                        task: None,
                        timeout_secs: None,
                        wake: true,
                        engine_epoch: 1,
                        nonce: "dead2",
                        log_path: "",
                    },
                    0,
                )?;
                Ok(((row.token(), unknown.id), vec![]))
            })
            .unwrap();
        // A real process carrying the dead supervisor's marker.
        let mut child = std::process::Command::new("sleep")
            .arg("60")
            .env(super::jobs::JOB_ENV, &token_row)
            .spawn()
            .unwrap();
        let orphan_pid = child.id();
        let _ = orphan;
        e.handle.shutdown(Duration::from_millis(300));
        drop(e);
        let provider = Scripted::new(calling_from_prompt());
        let mut e = fx.start(&provider);
        e.snapshot(&s);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let list = jobs(&mut e, &s);
            let two = list.jobs.iter().find(|j| j.id == 2).cloned();
            let three = list.jobs.iter().find(|j| j.id == 3).cloned();
            if two
                .as_ref()
                .is_some_and(|j| j.state == JobState::Interrupted)
                && three.as_ref().is_some_and(|j| j.state == JobState::Unknown)
            {
                // Orphans are found and killed through /proc; elsewhere the engine can only
                // report that it lost the process.
                let expected = if cfg!(target_os = "linux") {
                    "process was stopped on restart"
                } else {
                    "the process is gone"
                };
                let reason = two.unwrap().reason.unwrap();
                assert!(reason.contains(expected), "{reason}");
                assert!(
                    three
                        .unwrap()
                        .reason
                        .unwrap()
                        .contains("may or may not have run")
                );
                break;
            }
            assert!(std::time::Instant::now() < deadline, "{list:?}");
            std::thread::sleep(Duration::from_millis(30));
        }
        #[cfg(target_os = "linux")]
        {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while alive(orphan_pid) {
                assert!(std::time::Instant::now() < deadline, "orphan survived");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        let _ = orphan_pid;
        // Reap what the engine killed (the test is its parent).
        let _ = child.wait();
        assert!(
            !fx.project.join("should-not-exist").exists(),
            "an unknown job is never relaunched"
        );
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            user_texts(&e.snapshot(&s)).len(),
            1,
            "interrupted jobs wake nobody"
        );
        assert_eq!(jobs(&mut e, &s).active, 0);
        // Logs survive the restart.
        let l = log(&mut e, &s, 1, 0, None);
        assert_eq!(l.total_bytes, 0);
        assert!(l.done);
    }
}

#[test]
fn job_reads_are_recoverable_and_the_capability_is_advertised() {
    let fx = Fixture::new();
    let provider = Scripted::new(calling_from_prompt());
    let mut e = fx.start(&provider);
    let shared = e.handle.shared();
    for tool in ["job_status", "job_log", "job_wait", "job_cancel"] {
        assert!(shared.recoverable_tool(tool).is_some(), "{tool}");
    }
    assert!(shared.recoverable_tool("job_start").is_none());
    let welcome = match e.call(Call::Hello {
        protocol: PROTOCOL_VERSION,
        client: "again".into(),
    }) {
        Ok(Reply::Welcome(w)) => w,
        other => panic!("{other:?}"),
    };
    assert!(welcome.capabilities.iter().any(|c| c == "jobs"));
    let s = e.create(&fx.project, "c");
    assert!(jobs(&mut e, &s).jobs.is_empty());
    let err = e
        .call(Call::JobLog {
            session: s.clone(),
            id: 1,
            offset: 0,
            limit: None,
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}
