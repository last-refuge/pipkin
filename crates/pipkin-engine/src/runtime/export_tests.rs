//! Trace export (OBS-2) against a recording mock collector: a parent run with a tool call
//! and a child becomes correlated OTLP spans with metadata only; a restart resends nothing;
//! records from before export was enabled are never sent and a disabled exporter makes no
//! request; a rejecting collector backs off, drops after the bounded attempts and never
//! slows a turn or a Stop; a hanging collector does not delay commits; an invalid
//! configuration is reported, not posted to.

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pipkin_engine_protocol::*;
use serde_json::{Value, json};

use super::tests::{Engine, Fixture};
use crate::provider::ProviderRequest;
use crate::provider::mock::{MockResponse, MockServer, Recorded};
use crate::provider::scripted::{Gate, Reply as R, Scripted};

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

const TASK: &str = "child:survey the SECRET-TASK-TEXT";

fn script(gate: Gate) -> impl Fn(&ProviderRequest, usize) -> R + Send + Sync {
    move |req, _| {
        if first_user(req).starts_with("child:") {
            return R::Text("child done SECRET-ANSWER".into());
        }
        if !last_is_user(req) {
            return R::Text("done".into());
        }
        match user_text(req).as_str() {
            "delegate" => R::Tools(
                None,
                vec![
                    ("bash".into(), json!({"command": "echo SECRET-OUTPUT"})),
                    ("subagent".into(), json!({"task": TASK})),
                ],
            ),
            "hang" => R::Gated(gate.clone(), Box::new(R::Text("late".into()))),
            _ => R::Text("done".into()),
        }
    }
}

/// A collector that records every post and answers with the current status (or hangs).
struct Collector {
    rt: Option<tokio::runtime::Runtime>,
    server: MockServer,
    status: Arc<AtomicU16>,
    hang: Arc<Mutex<bool>>,
}

impl Drop for Collector {
    fn drop(&mut self) {
        // A handler may be sleeping (the hung collector); do not wait for it.
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

impl Collector {
    fn start() -> Collector {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let status = Arc::new(AtomicU16::new(200));
        let hang = Arc::new(Mutex::new(false));
        let server = {
            let status = status.clone();
            let hang = hang.clone();
            rt.block_on(MockServer::start(move |_req: &Recorded, _| {
                if *hang.lock().unwrap() {
                    // No answer at all for longer than the exporter waits: the client sees
                    // its own timeout.
                    std::thread::sleep(Duration::from_secs(15));
                }
                MockResponse::json(status.load(Ordering::Relaxed), &json!({}))
            }))
        };
        Collector {
            rt: Some(rt),
            server,
            status,
            hang,
        }
    }

    fn endpoint(&self) -> String {
        format!("{}/v1/traces", self.server.url)
    }

    fn posts(&self) -> Vec<Recorded> {
        self.server
            .requests()
            .into_iter()
            .filter(|r| r.method == "POST")
            .collect()
    }

    /// Every span posted so far, in order.
    fn spans(&self) -> Vec<Value> {
        self.posts()
            .iter()
            .flat_map(|r| {
                let body = r.json();
                body["resourceSpans"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .flat_map(|rs| rs["scopeSpans"].as_array().cloned().unwrap_or_default())
                    .flat_map(|ss| ss["spans"].as_array().cloned().unwrap_or_default())
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn until_spans(&self, what: &str, done: impl Fn(&[Value]) -> bool) -> Vec<Value> {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let spans = self.spans();
            if done(&spans) {
                return spans;
            }
            assert!(
                Instant::now() < deadline,
                "never saw {what}: {} spans",
                spans.len()
            );
            std::thread::sleep(Duration::from_millis(30));
        }
    }
}

fn write_settings(fx: &Fixture, body: serde_json::Value) {
    std::fs::create_dir_all(fx.dir.path().join("config")).unwrap();
    std::fs::write(fx.dir.path().join("config/settings.json"), body.to_string()).unwrap();
}

fn export_settings(endpoint: &str) -> serde_json::Value {
    json!({"export": {
        "enabled": true,
        "endpoint": endpoint,
        "headers": {"Authorization": "Basic SECRET-HEADER"},
        "retryBaseMs": 10,
    }})
}

fn health(e: &mut Engine) -> ExportHealthView {
    match e.call(Call::ExportHealth) {
        Ok(Reply::ExportHealth(h)) => h,
        other => panic!("{other:?}"),
    }
}

fn until_health(
    e: &mut Engine,
    what: &str,
    done: impl Fn(&ExportHealthView) -> bool,
) -> ExportHealthView {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let h = health(e);
        if done(&h) {
            return h;
        }
        assert!(Instant::now() < deadline, "never saw {what}: {h:?}");
        std::thread::sleep(Duration::from_millis(30));
    }
}

fn attr<'a>(span: &'a Value, key: &str) -> Option<&'a Value> {
    span["attributes"]
        .as_array()?
        .iter()
        .find(|a| a["key"] == key)
        .map(|a| &a["value"])
}

fn attr_str(span: &Value, key: &str) -> Option<String> {
    attr(span, key).and_then(|v| v["stringValue"].as_str().map(str::to_owned))
}

#[test]
fn a_parent_and_child_run_become_correlated_metadata_spans_and_a_restart_resends_nothing() {
    let collector = Collector::start();
    let fx = Fixture::new();
    write_settings(&fx, export_settings(&collector.endpoint()));
    let provider = Scripted::new(script(Gate::new()));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "delegate").unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let spans = collector.until_spans("the parent run span", |spans| {
        spans.iter().filter(|sp| sp["name"] == "run").count() >= 2
    });
    // One trace for the parent and its child.
    let traces: std::collections::HashSet<String> = spans
        .iter()
        .map(|sp| sp["traceId"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(traces.len(), 1, "{traces:?}");
    assert_eq!(traces.iter().next().unwrap().len(), 32);
    let tool = spans
        .iter()
        .find(|sp| sp["name"] == "tool subagent")
        .expect("the subagent tool span");
    let runs: Vec<&Value> = spans.iter().filter(|sp| sp["name"] == "run").collect();
    let child_run = runs
        .iter()
        .find(|sp| attr_str(sp, "pipkin.parent_session").is_some())
        .expect("the child's run span");
    assert_eq!(child_run["parentSpanId"], tool["spanId"]);
    assert_eq!(
        attr_str(child_run, "pipkin.parent_session").as_deref(),
        Some(s.as_str())
    );
    let root_run = runs
        .iter()
        .find(|sp| attr_str(sp, "pipkin.parent_session").is_none())
        .unwrap();
    assert!(root_run.get("parentSpanId").is_none());
    assert_eq!(tool["parentSpanId"], root_run["spanId"]);
    assert_eq!(attr_str(root_run, "pipkin.request").as_deref(), Some("r1"));
    assert_eq!(
        attr_str(root_run, "langfuse.observation.type").as_deref(),
        Some("agent")
    );
    let generations: Vec<&Value> = spans
        .iter()
        .filter(|sp| sp["name"] == "generation")
        .collect();
    assert!(generations.len() >= 3, "{}", generations.len());
    assert!(
        attr_str(generations[0], "gen_ai.request.model")
            .unwrap()
            .contains('/')
    );
    assert!(attr(generations[0], "gen_ai.usage.input_tokens").is_some());
    assert_eq!(generations[0]["parentSpanId"], root_run["spanId"]);
    assert_eq!(generations[0]["kind"], 3);
    let bash = spans.iter().find(|sp| sp["name"] == "tool bash").unwrap();
    assert_eq!(attr_str(bash, "pipkin.outcome").as_deref(), Some("ok"));
    assert!(bash["startTimeUnixNano"].as_str().unwrap().len() > 15);
    // Metadata only: nothing the model, the person or a tool said.
    for post in collector.posts() {
        let body = String::from_utf8_lossy(&post.body);
        for secret in [
            "SECRET-TASK-TEXT",
            "SECRET-ANSWER",
            "SECRET-OUTPUT",
            "delegate",
        ] {
            assert!(!body.contains(secret), "payload leaks {secret}");
        }
        assert_eq!(post.header("authorization"), Some("Basic SECRET-HEADER"));
        assert_eq!(post.path, "/v1/traces");
        assert_eq!(
            post.json()["resourceSpans"][0]["resource"]["attributes"][0]["value"]["stringValue"],
            "pipkin-engine"
        );
    }
    let h = until_health(&mut e, "idle after sending", |h| {
        h.state == "idle" && h.queued == 0
    });
    assert!(h.enabled && h.sent >= spans.len() as u64);
    assert_eq!(
        h.endpoint.as_deref(),
        Some(collector.server.url.trim_start_matches("http://"))
    );
    assert_eq!(h.content, "metadata");
    assert!(h.last_success_ms.is_some() && h.last_error.is_none() && h.dropped == 0);
    assert!(!format!("{h:?}").contains("SECRET-HEADER"));
    let sent_before = h.sent;
    let spans_before = collector.spans().len();
    // A restart: the cursor is durable, nothing already sent goes again; new work does.
    e.handle.shutdown(Duration::from_millis(500));
    drop(e);
    let provider2 = Scripted::new(script(Gate::new()));
    let mut e = fx.start(&provider2);
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(
        collector.spans().len(),
        spans_before,
        "nothing resent after a restart"
    );
    assert_eq!(health(&mut e).sent, sent_before);
    e.prompt(&s, "r2", "plain").unwrap();
    assert_eq!(e.settled("r2"), RunOutcome::Completed);
    let spans = collector.until_spans("the second run", |spans| {
        spans
            .iter()
            .any(|sp| attr_str(sp, "pipkin.request").as_deref() == Some("r2"))
    });
    let ids: Vec<&str> = spans
        .iter()
        .map(|sp| sp["spanId"].as_str().unwrap())
        .collect();
    let mut unique = ids.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), ids.len(), "every span was posted once");
}

#[test]
fn records_before_enabling_are_never_sent_and_a_disabled_exporter_makes_no_request() {
    let collector = Collector::start();
    let fx = Fixture::new();
    let provider = Scripted::new(script(Gate::new()));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "plain").unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    std::thread::sleep(Duration::from_millis(300));
    assert!(collector.posts().is_empty(), "disabled: no requests");
    let h = health(&mut e);
    assert_eq!((h.enabled, h.state.as_str(), h.sent), (false, "off", 0));
    // Enabling later: the earlier run stays unsent; only what follows goes.
    write_settings(&fx, export_settings(&collector.endpoint()));
    until_health(&mut e, "enabled", |h| h.enabled && h.state == "idle");
    assert!(
        collector.posts().is_empty(),
        "history is not pushed on enable"
    );
    e.prompt(&s, "r2", "plain").unwrap();
    assert_eq!(e.settled("r2"), RunOutcome::Completed);
    let spans = collector.until_spans("the second run", |spans| {
        spans.iter().any(|sp| sp["name"] == "run")
    });
    assert!(
        spans
            .iter()
            .all(|sp| attr_str(sp, "pipkin.request").as_deref() != Some("r1")),
        "{spans:?}"
    );
    assert!(
        spans
            .iter()
            .any(|sp| attr_str(sp, "pipkin.request").as_deref() == Some("r2"))
    );
    // Disabling stops it again.
    write_settings(&fx, json!({"export": {"enabled": false}}));
    until_health(&mut e, "off", |h| !h.enabled && h.state == "off");
    let posts = collector.posts().len();
    e.prompt(&s, "r3", "plain").unwrap();
    assert_eq!(e.settled("r3"), RunOutcome::Completed);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(collector.posts().len(), posts);
}

#[test]
fn a_rejecting_or_hanging_collector_never_slows_work_and_the_batch_is_dropped_after_retries() {
    let collector = Collector::start();
    collector.status.store(503, Ordering::Relaxed);
    let fx = Fixture::new();
    write_settings(&fx, export_settings(&collector.endpoint()));
    let gate = Gate::new();
    let provider = Scripted::new(script(gate.clone()));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    let started = Instant::now();
    e.prompt(&s, "r1", "delegate").unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    assert!(started.elapsed() < Duration::from_secs(5));
    let h = until_health(&mut e, "a retry", |h| {
        h.state == "backoff" || h.state == "failed"
    });
    assert!(
        h.last_error.as_deref().unwrap_or("").contains("503"),
        "{h:?}"
    );
    // After the bounded attempts the batch is dropped and counted; nothing was lost
    // locally (the diagnostics records stay).
    // (The exporter moves on to the next batch at once, so `failed` may be fleeting.)
    let h = until_health(&mut e, "the drop", |h| h.dropped > 0);
    assert!(h.dropped >= 5, "{h:?}");
    assert!(collector.posts().len() >= 5);
    let Ok(Reply::Diagnostics(view)) = e.call(Call::Diagnostics {
        session: s.clone(),
        before: None,
        limit: 10,
    }) else {
        panic!()
    };
    assert!(view.summary.records >= 5);
    // A hanging collector: the turn and a Stop are as fast as ever.
    *collector.hang.lock().unwrap() = true;
    collector.status.store(200, Ordering::Relaxed);
    let started = Instant::now();
    e.prompt(&s, "r2", "plain").unwrap();
    assert_eq!(e.settled("r2"), RunOutcome::Completed);
    assert!(started.elapsed() < Duration::from_secs(5));
    until_health(&mut e, "sending to the hung collector", |h| {
        h.state == "sending"
    });
    e.prompt(&s, "r3", "hang").unwrap();
    e.until("the model was asked", |_| provider.requests().len() >= 5);
    let started = Instant::now();
    e.call(Call::Stop { session: s.clone() }).unwrap();
    assert_eq!(e.settled("r3"), RunOutcome::Aborted);
    assert!(started.elapsed() < Duration::from_secs(3));
    gate.open();
    // Shutdown with a post in flight is bounded.
    let started = Instant::now();
    e.handle.shutdown(Duration::from_millis(500));
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn an_invalid_configuration_is_reported_and_nothing_is_posted() {
    let collector = Collector::start();
    let fx = Fixture::new();
    write_settings(
        &fx,
        json!({"export": {"enabled": true, "kind": "zipkin", "endpoint": collector.endpoint()}}),
    );
    let provider = Scripted::new(script(Gate::new()));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "plain").unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let h = until_health(&mut e, "the configuration error", |h| h.state == "failed");
    assert!(h.enabled);
    assert!(
        h.last_error.as_deref().unwrap().contains("not supported"),
        "{h:?}"
    );
    write_settings(
        &fx,
        json!({"export": {"enabled": true, "endpoint": "ftp://x"}}),
    );
    let h = until_health(&mut e, "the endpoint error", |h| {
        h.last_error
            .as_deref()
            .is_some_and(|e| e.contains("http(s)"))
    });
    assert_eq!(h.sent, 0);
    std::thread::sleep(Duration::from_millis(200));
    assert!(collector.posts().is_empty());
    // The capability is advertised.
    let Ok(Reply::Welcome(w)) = e.call(Call::Hello {
        protocol: PROTOCOL_VERSION,
        client: "x".into(),
    }) else {
        panic!()
    };
    assert!(w.capabilities.iter().any(|c| c == "export"));
}
