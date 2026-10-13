//! Local diagnostics (OBS-1) through the client contract: a turn with a tool call leaves
//! correlated `run.start` / `provider.request` / `tool.call` / `run.end` records with the
//! engine's timings; a retried provider error is recorded with its delay; a Stop is recorded
//! as a cancelled request and an aborted run with its Stop latency; a child's records name
//! its parent and call; the page cursor, the export file and the per-session bound hold;
//! the records survive a restart, which records the resumed run.

use std::time::Duration;

use pipkin_engine_protocol::*;
use serde_json::json;

use super::tests::{Engine, Fixture};
use crate::provider::ProviderRequest;
use crate::provider::scripted::{Gate, Reply as R, Scripted};
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

fn user_text(req: &ProviderRequest) -> String {
    req.messages
        .iter()
        .rev()
        .find_map(|m| match m {
            Message::User { .. } => Some(m.text()),
            _ => None,
        })
        .unwrap_or_default()
}

fn first_user(req: &ProviderRequest) -> String {
    req.messages
        .iter()
        .find_map(|m| match m {
            Message::User { .. } => Some(m.text()),
            _ => None,
        })
        .unwrap_or_default()
}

/// `tool` → one `bash echo` call then a reply; `fail-once` → an error on the first call of
/// the run, then a reply; `hang` → never answers; `delegate` → a subagent whose child
/// answers at once; anything else → a reply.
fn script(gate: Gate) -> impl Fn(&ProviderRequest, usize) -> R + Send + Sync {
    let failed = std::sync::Mutex::new(false);
    move |req, _| {
        if first_user(req).starts_with("child:") {
            return R::Text("child done".into());
        }
        let text = user_text(req);
        if !last_is_user(req) {
            return R::Text("done".into());
        }
        match text.as_str() {
            "tool" => R::Tools(None, vec![("bash".into(), json!({"command": "echo hi"}))]),
            "fail-once" => {
                let mut f = failed.lock().unwrap();
                if !*f {
                    *f = true;
                    R::Error("429 rate limit exceeded, retry later".into())
                } else {
                    R::Text("recovered".into())
                }
            }
            "hang" => R::Gated(gate.clone(), Box::new(R::Text("late".into()))),
            "delegate" => R::Tools(
                None,
                vec![("subagent".into(), json!({"task": "child:none"}))],
            ),
            _ => R::Text("done".into()),
        }
    }
}

fn diagnostics(e: &mut Engine, s: &str, before: Option<u64>, limit: u32) -> DiagnosticsView {
    match e.call(Call::Diagnostics {
        session: s.into(),
        before,
        limit,
    }) {
        Ok(Reply::Diagnostics(v)) => v,
        other => panic!("{other:?}"),
    }
}

/// Oldest first.
fn kinds(view: &DiagnosticsView) -> Vec<String> {
    let mut k: Vec<(u64, String)> = view
        .records
        .iter()
        .map(|r| (r.seq, r.kind.clone()))
        .collect();
    k.sort();
    k.into_iter().map(|(_, k)| k).collect()
}

fn find<'a>(view: &'a DiagnosticsView, kind: &str) -> Vec<&'a DiagnosticRecord> {
    let mut v: Vec<&DiagnosticRecord> = view.records.iter().filter(|r| r.kind == kind).collect();
    v.sort_by_key(|r| r.seq);
    v
}

fn write_settings(fx: &Fixture, body: serde_json::Value) {
    std::fs::create_dir_all(fx.dir.path().join("config")).unwrap();
    std::fs::write(fx.dir.path().join("config/settings.json"), body.to_string()).unwrap();
}

#[test]
fn a_turn_with_a_tool_call_leaves_correlated_timed_records() {
    let fx = Fixture::new();
    let provider = Scripted::new(script(Gate::new()));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    assert_eq!(
        diagnostics(&mut e, &s, None, 10).summary,
        DiagnosticsSummary::default()
    );
    e.prompt(&s, "r1", "tool").unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let view = diagnostics(&mut e, &s, None, 50);
    assert_eq!(
        kinds(&view),
        [
            "run.start",
            "provider.request",
            "tool.call",
            "provider.request",
            "run.end"
        ]
    );
    let run = view.records[0].run.expect("every record names the run");
    assert!(view.records.iter().all(|r| r.run == Some(run)));
    let start = find(&view, "run.start")[0];
    assert_eq!(start.request.as_deref(), Some("r1"));
    assert_eq!(start.data["mode"], "prompt");
    assert!(start.data["queueWaitMs"].as_u64().is_some());
    assert_eq!(start.data["parent"], serde_json::Value::Null);
    let requests = find(&view, "provider.request");
    assert_eq!(requests[0].data["attempt"], 1);
    assert_eq!(requests[0].data["outcome"], "completed");
    assert!(requests[0].data["durationMs"].as_u64().is_some());
    assert_eq!(
        requests[0].data["model"].as_str().map(|m| m.contains('/')),
        Some(true)
    );
    assert!(requests[0].data["tokens"].is_object());
    let tool = find(&view, "tool.call")[0];
    assert_eq!(tool.data["name"], "bash");
    assert_eq!(tool.data["outcome"], "ok");
    assert!(tool.data["durationMs"].as_u64().is_some());
    assert!(tool.call_id.is_some());
    assert!(tool.data["retainedBytes"].as_u64().unwrap() >= 2);
    let end = find(&view, "run.end")[0];
    assert_eq!(end.data["outcome"], "completed");
    assert_eq!(end.data["stopRequested"], false);
    assert_eq!(end.request.as_deref(), Some("r1"));
    let s1 = &view.summary;
    assert_eq!(
        (
            s1.records,
            s1.runs,
            s1.provider_requests,
            s1.tool_calls,
            s1.retries,
            s1.cancellations
        ),
        (5, 1, 2, 1, 0, 0)
    );
    assert!(s1.provider_p50_ms.is_some() && s1.tool_p50_ms.is_some());
    assert!(s1.queue_wait_max_ms.is_some());
    // Paging newest first by cursor.
    let page = diagnostics(&mut e, &s, None, 2);
    assert_eq!(page.records.len(), 2);
    assert!(page.has_older);
    assert_eq!(page.records[0].kind, "run.end");
    let rest = diagnostics(&mut e, &s, Some(page.records[1].seq), 10);
    assert_eq!(rest.records.len(), 3);
    assert!(!rest.has_older);
    // Export: one JSON line per record, with provenance, oldest first.
    let Ok(Reply::DiagnosticsExported { path, records }) =
        e.call(Call::ExportDiagnostics { session: s.clone() })
    else {
        panic!()
    };
    assert_eq!(records, 5);
    let body = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<serde_json::Value> = body
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 5);
    assert_eq!(lines[0]["kind"], "run.start");
    assert_eq!(lines[0]["source"], "pipkin-engine");
    assert_eq!(lines[0]["session"], s);
    assert!(path.starts_with(fx.dir.path().to_str().unwrap()), "{path}");
    assert_eq!(
        e.call(Call::Diagnostics {
            session: "nope".into(),
            before: None,
            limit: 5
        })
        .unwrap_err()
        .code,
        ErrorCode::NotFound
    );
}

#[test]
fn a_retried_provider_error_and_a_stop_are_recorded_as_such() {
    let fx = Fixture::new();
    write_settings(
        &fx,
        json!({"retry": {"baseDelayMs": 20, "maxAgentDelayMs": 50}}),
    );
    let gate = Gate::new();
    let provider = Scripted::new(script(gate.clone()));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "fail-once").unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let view = diagnostics(&mut e, &s, None, 50);
    assert_eq!(
        kinds(&view),
        [
            "run.start",
            "provider.request",
            "provider.retry",
            "provider.request",
            "run.end"
        ]
    );
    let requests = find(&view, "provider.request");
    assert_eq!(requests[0].data["outcome"], "error");
    assert!(
        requests[0].data["error"]
            .as_str()
            .unwrap()
            .contains("rate limit"),
        "{:?}",
        requests[0].data
    );
    assert_eq!(requests[1].data["attempt"], 2);
    assert_eq!(requests[1].data["outcome"], "completed");
    let retry = find(&view, "provider.retry")[0];
    assert_eq!(retry.data["attempt"], 1);
    assert!(retry.data["delayMs"].as_u64().unwrap() <= 50);
    assert_eq!((view.summary.provider_errors, view.summary.retries), (1, 1));
    // A Stop while the model hangs.
    e.prompt(&s, "r2", "hang").unwrap();
    e.until("the model was asked", |_| provider.requests().len() >= 3);
    e.call(Call::Stop { session: s.clone() }).unwrap();
    assert_eq!(e.settled("r2"), RunOutcome::Aborted);
    gate.open();
    let view = diagnostics(&mut e, &s, None, 50);
    let newest: Vec<&str> = view
        .records
        .iter()
        .take(3)
        .map(|r| r.kind.as_str())
        .collect();
    assert_eq!(newest, ["run.end", "provider.request", "run.start"]);
    assert_eq!(view.records[1].data["outcome"], "cancelled");
    let end = &view.records[0];
    assert_eq!(end.data["outcome"], "aborted");
    assert_eq!(end.data["stopRequested"], true);
    assert!(end.data["stopLatencyMs"].as_u64().is_some());
    assert_eq!(end.request.as_deref(), Some("r2"));
    assert_eq!(view.summary.cancellations, 1);
    assert!(view.summary.stop_latency_max_ms.is_some());
}

#[test]
fn a_childs_records_name_its_parent_and_the_parents_call_is_timed() {
    let fx = Fixture::new();
    let provider = Scripted::new(script(Gate::new()));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "delegate").unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let parent = diagnostics(&mut e, &s, None, 50);
    let call = find(&parent, "tool.call")[0];
    assert_eq!(call.data["name"], "subagent");
    assert_eq!(call.data["outcome"], "ok");
    let call_id = call.call_id.clone().unwrap();
    let Ok(Reply::Subagents(view)) = e.call(Call::Subagents { session: s.clone() }) else {
        panic!()
    };
    let child = view.children[0].session.clone();
    let child_view = diagnostics(&mut e, &child, None, 50);
    assert_eq!(
        kinds(&child_view),
        ["run.start", "provider.request", "run.end"]
    );
    let start = find(&child_view, "run.start")[0];
    assert_eq!(start.data["parent"], s);
    assert_eq!(start.data["parentCall"], call_id);
    assert_eq!(
        start.request.as_deref(),
        Some(format!("subagent:{call_id}").as_str())
    );
    // The parent's records never include the child's.
    assert_eq!(parent.summary.provider_requests, 2);
    assert_eq!(parent.summary.tool_calls, 1);
}

#[test]
fn records_survive_a_restart_which_records_the_resumed_run_and_the_bound_holds() {
    let fx = Fixture::new();
    let s;
    {
        let gate = Gate::new();
        let provider = Scripted::new(script(gate.clone()));
        let mut e = fx.start(&provider);
        s = e.create(&fx.project, "c");
        e.prompt(&s, "r0", "plain").unwrap();
        assert_eq!(e.settled("r0"), RunOutcome::Completed);
        e.prompt(&s, "r1", "hang").unwrap();
        e.until("the model was asked", |_| provider.requests().len() >= 2);
        e.handle.shutdown(Duration::from_millis(200));
    }
    let open = Gate::new();
    open.open();
    let provider = Scripted::new(script(open));
    let mut e = fx.start(&provider);
    e.snapshot(&s);
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(Reply::Lookup(LookupResult::Known {
            open: false,
            outcome: Some(RunOutcome::Completed),
            ..
        })) = e.call(Call::Lookup {
            session: s.clone(),
            request: "r1".into(),
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "r1 did not settle");
        std::thread::sleep(Duration::from_millis(20));
    }
    let view = diagnostics(&mut e, &s, None, 50);
    let k = kinds(&view);
    assert_eq!(
        k,
        [
            "run.start",
            "provider.request",
            "run.end",
            "run.start",
            "run.resumed",
            "provider.request",
            "run.end"
        ],
        "{k:?}"
    );
    let resumed = find(&view, "run.resumed")[0];
    assert_eq!(resumed.data["reason"], "restart");
    assert_eq!(resumed.request.as_deref(), Some("r1"));
    // The interrupted exchange left no request record (the engine never saw it end); the
    // resumed run's request is attempt 1 of a fresh generation.
    assert_eq!(view.summary.runs, 2);
    // Bound: the newest 2,000 rows per session are kept.
    let shared = e.handle.shared().clone();
    shared
        .store
        .tx(|tx| {
            for i in 0..2_100u64 {
                Store::diag_append(
                    tx,
                    &s,
                    i as i64,
                    "probe",
                    None,
                    None,
                    None,
                    &json!({"i": i}),
                )?;
            }
            Ok(())
        })
        .unwrap();
    let count = shared
        .store
        .read(|c| Store::diagnostics_count(c, &s))
        .unwrap();
    assert_eq!(count, 2_000);
    let newest = diagnostics(&mut e, &s, None, 1);
    assert_eq!(newest.records[0].kind, "probe");
    assert_eq!(newest.records[0].data["i"], 2_099);
    assert_eq!(newest.summary.records, 2_000);
    // The capability is advertised.
    let Ok(Reply::Welcome(w)) = e.call(Call::Hello {
        protocol: PROTOCOL_VERSION,
        client: "diag".into(),
    }) else {
        panic!()
    };
    assert!(w.capabilities.iter().any(|c| c == "diagnostics"));
}
