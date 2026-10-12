//! Execute the same normalized provider responses as the frozen Pi Harness capture.
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use pipkin_engine_protocol::*;
use serde_json::{Value, json};

use super::{
    EngineHandle, registry_with,
    tests::{Engine, Fixture},
};
use crate::provider::{ApiAdapter, BoxFuture, StreamCtx};

fn reference() -> Value {
    serde_json::from_str(include_str!(
        "../../../../fixtures/native-generation/reference.json"
    ))
    .unwrap()
}

fn project(message: &Message) -> Value {
    let value = serde_json::to_value(message).unwrap();
    let content = match &value["content"] {
        Value::String(text) => json!([{"type":"text","text":text}]),
        content => content.clone(),
    };
    let mut projected = json!({"role":value["role"],"content":content});
    for field in [
        "stopReason",
        "errorMessage",
        "toolCallId",
        "toolName",
        "isError",
    ] {
        if let Some(value) = value.get(field).filter(|v| !v.is_null()) {
            projected[field] = value.clone();
        }
    }
    projected
}

struct Responses {
    case: Value,
    seed: AtomicBool,
    state: Mutex<(usize, usize, Vec<Value>)>,
}
impl ApiAdapter for Responses {
    fn api(&self) -> &'static str {
        "scripted"
    }
    fn stream<'a>(&'a self, ctx: StreamCtx<'a>) -> BoxFuture<'a, Message> {
        Box::pin(async move {
            if self.seed.load(Ordering::SeqCst) {
                let mut e = ctx.emitter();
                e.append_text("seed answer");
                return e.finish(ctx.req, StopReason::Stop, None);
            }
            let summary = ctx.req.system == crate::context::compaction::SUMMARIZATION_SYSTEM_PROMPT;
            let mut state = self.state.lock().unwrap();
            let response = if summary {
                let i = state.1;
                state.1 += 1;
                self.case["summaries"].get(i)
            } else {
                let i = state.0;
                state.0 += 1;
                self.case["responses"].get(i)
            };
            let message: Message = match response {
                Some(v) => serde_json::from_value(v.clone()).unwrap(),
                None => ctx.emitter().finish(
                    ctx.req,
                    StopReason::Error,
                    Some("Unexpected extra provider request".into()),
                ),
            };
            state.2.push(json!({"summary":summary,"messages":ctx.req.messages.iter().filter(|m| !matches!(m,Message::System(_))).map(project).collect::<Vec<_>>(),"response":project(&message)}));
            message
        })
    }
}

#[test]
fn generation_error_classifiers_match_executed_pi() {
    let reference = reference();
    for case in reference["classifications"].as_array().unwrap() {
        let message = serde_json::from_value(case["message"].clone()).unwrap();
        assert_eq!(
            json!(crate::provider::errors::is_retryable(&message)),
            case["retry"],
            "{}",
            case["case"]
        );
        assert_eq!(
            json!(crate::provider::errors::is_context_overflow(
                &message, 200000
            )),
            case["overflow"],
            "{}",
            case["case"]
        );
    }
    for case in reference["delays"].as_array().unwrap() {
        assert_eq!(
            json!(crate::provider::errors::retry_delay_ms(
                case["attempt"].as_u64().unwrap() as u32,
                7,
                1000
            )),
            case["value"]
        );
    }
}

#[test]
fn generation_control_flow_matches_executed_pi() {
    let reference = reference();
    let mut failures = vec![];
    for case in reference["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let fx = Fixture::new();
        let settings = |enabled| {
            std::fs::create_dir_all(fx.config().config_dir.clone()).unwrap();
            std::fs::write(fx.config().config_dir.join("settings.json"),json!({
                "retry":case.get("retry").unwrap_or(&reference["retry"]),
                "compaction":{"enabled":enabled,"keepRecentTokens":5,"reserveTokens":1000,"backgroundTokens":0}
            }).to_string()).unwrap();
        };
        settings(false);
        let provider = Arc::new(Responses {
            case: case.clone(),
            seed: AtomicBool::new(true),
            state: Mutex::new((0, 0, vec![])),
        });
        let start = || {
            let handle = EngineHandle::start_with(
                fx.config(),
                registry_with(provider.clone()),
                crate::tools::builtin_tools(),
            )
            .unwrap();
            let conn = handle.connect("generation-reference");
            Engine {
                handle,
                conn,
                next: 0,
                events: vec![],
            }
        };
        let mut e = start();
        let session = e.create(&fx.project, "create");
        e.call(Call::SetSubagentsEnabled {
            session: session.clone(),
            enabled: false,
        })
        .unwrap();
        for i in 0..case["seed"].as_u64().unwrap_or(0) {
            let key = format!("seed{i}");
            e.prompt(&session, &key, "earlier conversation with several words")
                .unwrap();
            assert_eq!(e.settled(&key), RunOutcome::Completed);
        }
        provider.seed.store(false, Ordering::SeqCst);
        settings(case["compaction"].as_bool().unwrap_or(false));
        e.prompt(&session, "main", "perform the requested turn")
            .unwrap();
        let outcome = e.settled("main");
        let expected = &case["expected"];
        let wanted = if expected["status"] == "done" {
            RunOutcome::Completed
        } else {
            RunOutcome::ModelError {
                detail: expected["detail"].as_str().unwrap().into(),
            }
        };
        if outcome != wanted {
            failures.push(format!("{name}: outcome {outcome:?} != {wanted:?}"));
        }
        let state = provider.state.lock().unwrap();
        let trace = json!(state.2);
        if trace != expected["requests"] {
            failures.push(format!(
                "{name}: provider trace differs ({} generation, {} summary vs {} expected total)",
                state.0,
                state.1,
                expected["requests"].as_array().unwrap().len()
            ));
            std::fs::write(
                format!("/tmp/pipkin-generation-{name}.json"),
                serde_json::to_string_pretty(
                    &json!({"actual":trace,"expected":expected["requests"]}),
                )
                .unwrap(),
            )
            .unwrap();
        }
        drop(state);
        let entries = e
            .handle
            .shared()
            .store
            .page(&session, None, 1000)
            .unwrap()
            .0;
        let history: Vec<_>=entries.iter().filter_map(|entry| match &entry.kind {
            EntryKind::Message {message,..} if !matches!(message,Message::System(_))=>Some(project(message)),
            EntryKind::Compaction {summary,..}=>Some(json!({"role":"user","content":[{"type":"text","text":crate::context::derive::summary_message_text(summary)}]})),
            _=>None,
        }).collect();
        if json!(history) != expected["history"] {
            failures.push(format!("{name}: history differs"));
        }
        let compactions = entries
            .iter()
            .filter(|e| matches!(e.kind, EntryKind::Compaction { .. }))
            .count();
        if json!(compactions) != expected["compactions"] {
            failures.push(format!(
                "{name}: compaction count {compactions} != {}",
                expected["compactions"]
            ));
        }
        if fx.project.join("must-not-write.txt").exists() {
            failures.push(format!("{name}: tool executed despite final stop reason"));
        }
        e.handle.shutdown(Duration::from_secs(5));
        drop(e);
        let mut reopened = start();
        reopened.snapshot(&session);
        reopened
            .prompt(&session, "main", "perform the requested turn")
            .unwrap();
        if reopened
            .handle
            .shared()
            .store
            .page(&session, None, 1000)
            .unwrap()
            .0
            != entries
        {
            failures.push(format!("{name}: settled history changed on restart/replay"));
        }
        if json!(provider.state.lock().unwrap().2) != trace {
            failures.push(format!(
                "{name}: settled request executed again on restart/replay"
            ));
        }
        match reopened
            .call(Call::Lookup {
                session: session.clone(),
                request: "main".into(),
            })
            .unwrap()
        {
            Reply::Lookup(LookupResult::Known {
                open: false,
                outcome: Some(saved),
                ..
            }) if saved == outcome => {}
            other => failures.push(format!("{name}: durable settlement changed: {other:?}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
