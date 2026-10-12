//! `ask_user_question` through the client contract: the model calls the tool, the question
//! is published like an extension's, the person's answer (or refusal) settles it, and the
//! next model request carries exactly what was submitted. Also: Stop, stale ids, a restart
//! while the question is open, no connected client, and children not being offered the tool.

use std::time::Duration;

use pipkin_engine_protocol::*;
use serde_json::json;

use super::tests::{Engine, Fixture};
use crate::provider::ProviderRequest;
use crate::provider::scripted::{Gate, Reply as R, Scripted};

const TOOL: &str = "ask_user_question";

fn last_tool_result(req: &ProviderRequest) -> Option<String> {
    req.messages.iter().rev().find_map(|m| match m {
        Message::ToolResult { .. } => Some(m.text()),
        _ => None,
    })
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

/// Asks `args` after every user message, then answers with the tool result's text.
fn asking(args: serde_json::Value) -> impl Fn(&ProviderRequest, usize) -> R + Send + Sync {
    move |req, _| {
        if last_is_user(req) {
            R::Tools(None, vec![(TOOL.into(), args.clone())])
        } else {
            R::Text(format!(
                "model saw: {}",
                last_tool_result(req).unwrap_or_default()
            ))
        }
    }
}

fn choice_args() -> serde_json::Value {
    json!({
        "question": "Which approach?",
        "options": [{"label": "Rewrite"}, {"label": "Patch", "description": "Smaller"}],
        "allow_other": true
    })
}

fn open_question(e: &mut Engine, s: &str) -> QuestionView {
    let s = s.to_owned();
    e.until("the model's question", |ev| {
        ev.iter().any(|e| matches!(e, Event::Session { session, change: SessionChange::UiChanged { ui }, .. } if *session == s && !ui.questions.is_empty()))
    });
    e.snapshot(&s).ui.questions[0].clone()
}

fn tool_results(snap: &SessionSnapshot) -> Vec<(bool, String, Option<serde_json::Value>)> {
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
            } if tool_name == TOOL => Some((*is_error, m.text(), extra.get("details").cloned())),
            _ => None,
        })
        .collect()
}

fn final_text(snap: &SessionSnapshot) -> String {
    snap.entries
        .iter()
        .rev()
        .find_map(|e| match &e.kind {
            EntryKind::Message {
                message: m @ Message::Assistant { .. },
                partial: false,
                ..
            } => Some(m.text()),
            _ => None,
        })
        .unwrap_or_default()
}

#[test]
fn the_persons_choice_reaches_the_model_exactly() {
    let fx = Fixture::new();
    let provider = Scripted::new(asking(choice_args()));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "go").unwrap();
    let q = open_question(&mut e, &s);
    assert_eq!(q.kind, QuestionKind::Select);
    assert_eq!(q.title, "Which approach?");
    let labels: Vec<&str> = q.options.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(labels, ["Rewrite", "Patch", "Other (type an answer)"]);
    assert_eq!(q.options[1].description.as_deref(), Some("Smaller"));
    assert_eq!(q.deadline_ms, None, "a model question waits for the person");
    // A wrong value is refused and the question stays open.
    let bad = e
        .call(Call::AnswerQuestion {
            session: s.clone(),
            question: q.id.clone(),
            answer: Answer::Choice {
                value: "Patch".into(),
            },
        })
        .unwrap_err();
    assert_eq!(bad.message, "That is not one of the choices.");
    e.call(Call::AnswerQuestion {
        session: s.clone(),
        question: q.id.clone(),
        answer: Answer::Choice {
            value: q.options[1].value.clone(),
        },
    })
    .unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let snap = e.snapshot(&s);
    assert!(snap.ui.questions.is_empty());
    let results = tool_results(&snap);
    assert_eq!(results.len(), 1);
    assert!(!results[0].0);
    assert!(
        results[0].1.starts_with("The person chose: Patch"),
        "{}",
        results[0].1
    );
    let details = results[0].2.as_ref().unwrap();
    assert_eq!(details["askUserQuestion"]["version"], 1);
    assert_eq!(
        details["askUserQuestion"]["answer"],
        json!({"kind": "choice", "value": "o2", "label": "Patch", "index": 1})
    );
    assert_eq!(final_text(&snap), "model saw: The person chose: Patch");
    // The same question id cannot be answered again.
    let stale = e
        .call(Call::AnswerQuestion {
            session: s.clone(),
            question: q.id.clone(),
            answer: Answer::Choice {
                value: q.options[0].value.clone(),
            },
        })
        .unwrap_err();
    assert_eq!(stale.message, "That question is no longer open.");
    // The model's request offered the tool, once, after subagent.
    let names: Vec<String> = provider.requests()[0]
        .tools
        .iter()
        .map(|t| t.name.clone())
        .collect();
    assert_eq!(names.iter().filter(|n| *n == TOOL).count(), 1);
    let subagent = names.iter().position(|n| n == "subagent").unwrap();
    assert_eq!(names.get(subagent + 1).map(String::as_str), Some(TOOL));
}

#[test]
fn other_leads_to_a_text_follow_up_whose_answer_is_returned() {
    let fx = Fixture::new();
    let provider = Scripted::new(asking(choice_args()));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "go").unwrap();
    let q = open_question(&mut e, &s);
    e.call(Call::AnswerQuestion {
        session: s.clone(),
        question: q.id.clone(),
        answer: Answer::Choice {
            value: "other".into(),
        },
    })
    .unwrap();
    let follow = {
        let s = s.clone();
        let first = q.id.clone();
        e.until("the follow-up", |ev| {
            ev.iter().any(|e| matches!(e, Event::Session { session, change: SessionChange::UiChanged { ui }, .. } if *session == s && ui.questions.iter().any(|x| x.id != first)))
        });
        e.snapshot(&s).ui.questions[0].clone()
    };
    assert_eq!(follow.kind, QuestionKind::Input);
    assert_eq!(follow.title, "Your answer");
    assert_eq!(follow.message.as_deref(), Some("Which approach?"));
    e.call(Call::AnswerQuestion {
        session: s.clone(),
        question: follow.id,
        answer: Answer::Text {
            value: "feature flag".into(),
        },
    })
    .unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let snap = e.snapshot(&s);
    assert_eq!(
        final_text(&snap),
        "model saw: The person answered: feature flag"
    );
}

#[test]
fn declining_settles_the_call_as_unanswered() {
    let fx = Fixture::new();
    let provider = Scripted::new(asking(json!({"question": "Name?"})));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "go").unwrap();
    let q = open_question(&mut e, &s);
    assert_eq!(q.kind, QuestionKind::Input);
    e.call(Call::CancelQuestion {
        session: s.clone(),
        question: q.id,
    })
    .unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let snap = e.snapshot(&s);
    let results = tool_results(&snap);
    assert_eq!(results.len(), 1);
    assert!(!results[0].0, "declining is not a tool error");
    assert!(results[0].1.starts_with("The person did not answer"));
    assert_eq!(
        results[0].2.as_ref().unwrap()["askUserQuestion"]["outcome"],
        "unanswered"
    );
    assert!(final_text(&snap).starts_with("model saw: The person did not answer"));
}

#[test]
fn stop_withdraws_the_question_and_aborts_the_run() {
    let fx = Fixture::new();
    let provider = Scripted::new(asking(json!({"question": "Name?"})));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "go").unwrap();
    let q = open_question(&mut e, &s);
    e.call(Call::Stop { session: s.clone() }).unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Aborted);
    let snap = e.snapshot(&s);
    assert!(snap.ui.questions.is_empty(), "{:?}", snap.ui.questions);
    let results = tool_results(&snap);
    assert_eq!(results.len(), 1);
    assert!(results[0].0);
    assert!(
        results[0].1.contains("Tool ask_user_question was aborted"),
        "{}",
        results[0].1
    );
    let late = e
        .call(Call::AnswerQuestion {
            session: s.clone(),
            question: q.id,
            answer: Answer::Text { value: "x".into() },
        })
        .unwrap_err();
    assert_eq!(late.message, "That question is no longer open.");
    assert_eq!(provider.requests().len(), 1, "no model turn after Stop");
}

#[test]
fn a_restart_asks_the_open_question_again_under_a_new_id() {
    let fx = Fixture::new();
    let s;
    let first;
    {
        let provider = Scripted::new(asking(choice_args()));
        let mut e = fx.start(&provider);
        s = e.create(&fx.project, "c");
        e.prompt(&s, "r1", "go").unwrap();
        first = open_question(&mut e, &s);
        e.handle.shutdown(Duration::from_millis(200));
    }
    let provider = Scripted::new(asking(choice_args()));
    let mut e = fx.start(&provider);
    e.snapshot(&s);
    let again = open_question(&mut e, &s);
    assert_ne!(
        again.id, first.id,
        "a retired lifetime's id never comes back"
    );
    assert_eq!(again.title, first.title);
    assert_eq!(again.options, first.options);
    assert!(
        tool_results(&e.snapshot(&s)).is_empty(),
        "shutdown did not settle the question as the person's refusal"
    );
    let old = e
        .call(Call::AnswerQuestion {
            session: s.clone(),
            question: first.id.clone(),
            answer: Answer::Choice {
                value: again.options[0].value.clone(),
            },
        })
        .unwrap_err();
    assert_eq!(old.message, "That question is no longer open.");
    e.call(Call::AnswerQuestion {
        session: s.clone(),
        question: again.id,
        answer: Answer::Choice {
            value: again.options[0].value.clone(),
        },
    })
    .unwrap();
    let outcome = {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            if let Ok(Reply::Lookup(LookupResult::Known {
                open: false,
                outcome: Some(outcome),
                ..
            })) = e.call(Call::Lookup {
                session: s.clone(),
                request: "r1".into(),
            }) {
                break outcome;
            }
            assert!(std::time::Instant::now() < deadline, "r1 did not settle");
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    assert_eq!(outcome, RunOutcome::Completed);
    let snap = e.snapshot(&s);
    let results = tool_results(&snap);
    assert_eq!(results.len(), 1, "one result for one call: {results:?}");
    assert!(results[0].1.starts_with("The person chose: Rewrite"));
    assert_eq!(final_text(&snap), "model saw: The person chose: Rewrite");
    assert_eq!(
        provider.requests().len(),
        1,
        "the recovered call reran without a new model turn before the answer"
    );
}

#[test]
fn with_no_connected_client_the_call_fails_as_unavailable() {
    let fx = Fixture::new();
    let gate = Gate::new();
    let g = gate.clone();
    let provider = Scripted::new(move |req, _| {
        if last_is_user(req) {
            R::Gated(
                g.clone(),
                Box::new(R::Tools(
                    None,
                    vec![(TOOL.into(), json!({"question": "Anyone there?"}))],
                )),
            )
        } else {
            R::Text(format!(
                "model saw: {}",
                last_tool_result(req).unwrap_or_default()
            ))
        }
    });
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "go").unwrap();
    e.until("the model was asked", |_| !provider.requests().is_empty());
    let Engine { handle, conn, .. } = e;
    drop(conn);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while handle.shared().has_clients() {
        assert!(std::time::Instant::now() < deadline, "client never left");
        std::thread::sleep(Duration::from_millis(10));
    }
    gate.open();
    // The call settles while nobody is connected; only then does a client come back.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let done = handle
            .shared()
            .store
            .read(|c| crate::store::Store::tools_in_state(c, &s, "done"))
            .unwrap();
        if done.iter().any(|call| call.name == TOOL) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the call did not settle"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let conn = handle.connect("again");
    let mut e = Engine {
        handle,
        conn,
        next: 0,
        events: vec![],
    };
    e.call(Call::Hello {
        protocol: PROTOCOL_VERSION,
        client: "again".into(),
    })
    .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let snap = loop {
        let snap = e.snapshot(&s);
        if snap.run.is_none() && !tool_results(&snap).is_empty() {
            break snap;
        }
        assert!(std::time::Instant::now() < deadline, "r1 did not settle");
        std::thread::sleep(Duration::from_millis(20));
    };
    let results = tool_results(&snap);
    assert_eq!(results.len(), 1);
    assert!(results[0].0, "unavailable is an error for the model");
    assert!(
        results[0].1.contains("Cannot ask the person right now"),
        "{}",
        results[0].1
    );
    assert_eq!(
        results[0].2.as_ref().unwrap()["askUserQuestion"]["outcome"],
        "unavailable"
    );
    assert!(snap.ui.questions.is_empty());
}

#[test]
fn children_are_not_offered_the_tool() {
    let fx = Fixture::new();
    let provider = Scripted::new(|req: &ProviderRequest, _| {
        let is_child = req
            .messages
            .iter()
            .any(|m| matches!(m, Message::User { .. }) && m.text() == "survey");
        if is_child {
            R::Text("child done".into())
        } else if last_is_user(req) {
            R::Tools(None, vec![("subagent".into(), json!({"task": "survey"}))])
        } else {
            R::Text("parent done".into())
        }
    });
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "delegate").unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let requests = provider.requests();
    let child = requests
        .iter()
        .find(|r| r.messages.iter().any(|m| m.text() == "survey"))
        .expect("the child asked its model");
    assert!(
        child.tools.iter().all(|t| t.name != TOOL),
        "{:?}",
        child.tools
    );
    assert!(child.tools.iter().all(|t| t.name != "subagent"));
    let parent = &requests[0];
    assert!(parent.tools.iter().any(|t| t.name == TOOL));
}

#[test]
fn the_capability_is_advertised() {
    let fx = Fixture::new();
    let provider = Scripted::echo("x");
    let mut e = fx.start(&provider);
    let welcome = match e.call(Call::Hello {
        protocol: PROTOCOL_VERSION,
        client: "t".into(),
    }) {
        Ok(Reply::Welcome(w)) => w,
        other => panic!("{other:?}"),
    };
    assert!(welcome.capabilities.iter().any(|c| c == "askUserQuestion"));
}
