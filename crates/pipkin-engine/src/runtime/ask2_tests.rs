//! Questionnaires (ASK-2) through the client contract: the model asks several questions at
//! once, the view carries stable ids, the engine refuses malformed answers, a partial
//! submission is reported question by question, defaults are never answers, declining is
//! distinct from an empty submission, argument bounds are enforced before anything is
//! shown, and a restart re-asks the same questionnaire (same ids and source, new runtime
//! id).

use std::time::Duration;

use pipkin_engine_protocol::*;
use serde_json::json;

use super::tests::{Engine, Fixture};
use crate::provider::ProviderRequest;
use crate::provider::scripted::{Reply as R, Scripted};

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

fn form_args() -> serde_json::Value {
    json!({
        "title": "Before I start",
        "note": true,
        "questions": [
            {"id": "approach", "prompt": "Which approach?", "options": [
                {"id": "rewrite", "label": "Rewrite", "preview": "```rust\nfn new() {}\n```"},
                {"label": "Patch", "description": "Smaller"}
            ], "allow_other": true, "default": "rewrite"},
            {"id": "scope", "prompt": "Which parts?", "kind": "multiple", "options": [
                {"label": "Tests"}, {"label": "Docs"}, {"label": "Code"}
            ], "note": true},
            {"id": "name", "prompt": "What should it be called?", "placeholder": "a name"}
        ]
    })
}

/// The open question, polled from the snapshot (events of an earlier question would
/// otherwise satisfy a wait).
fn open_question(e: &mut Engine, s: &str) -> QuestionView {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(q) = e.snapshot(s).ui.questions.first() {
            return q.clone();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no question was asked"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn settled_by_lookup(e: &mut Engine, s: &str, key: &str) -> RunOutcome {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(Reply::Lookup(LookupResult::Known {
            open: false,
            outcome: Some(outcome),
            ..
        })) = e.call(Call::Lookup {
            session: s.into(),
            request: key.into(),
        }) {
            return outcome;
        }
        assert!(std::time::Instant::now() < deadline, "{key} did not settle");
        std::thread::sleep(Duration::from_millis(20));
    }
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

fn answer(e: &mut Engine, s: &str, q: &str, answer: Answer) -> Result<Reply, ProtoError> {
    e.call(Call::AnswerQuestion {
        session: s.into(),
        question: q.into(),
        answer,
    })
}

fn fa(question: &str, selected: &[&str], text: Option<&str>, note: Option<&str>) -> FormAnswer {
    FormAnswer {
        question: question.into(),
        selected: selected.iter().map(|s| s.to_string()).collect(),
        text: text.map(str::to_owned),
        note: note.map(str::to_owned),
    }
}

#[test]
fn a_questionnaire_is_shown_with_stable_ids_and_a_partial_answer_is_reported_per_question() {
    let fx = Fixture::new();
    let provider = Scripted::new(asking(form_args()));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "go").unwrap();
    let q = open_question(&mut e, &s);
    assert_eq!(q.kind, QuestionKind::Form);
    assert_eq!(q.title, "Before I start");
    assert_eq!(
        q.source.as_deref().map(|s| s.starts_with("call:")),
        Some(true)
    );
    assert_eq!(q.deadline_ms, None);
    let form = q.form.clone().expect("a questionnaire");
    assert_eq!(form.version, 1);
    assert!(form.allow_note);
    let ids: Vec<&str> = form.questions.iter().map(|q| q.id.as_str()).collect();
    assert_eq!(ids, ["approach", "scope", "name"]);
    assert_eq!(form.questions[0].kind, FormKind::Single);
    assert_eq!(
        form.questions[0]
            .options
            .iter()
            .map(|o| o.id.as_str())
            .collect::<Vec<_>>(),
        ["rewrite", "o2"]
    );
    assert_eq!(
        form.questions[0].options[0].preview.as_deref(),
        Some("```rust\nfn new() {}\n```")
    );
    assert_eq!(form.questions[0].default, ["rewrite"]);
    assert!(form.questions[0].allow_other);
    assert_eq!(form.questions[1].kind, FormKind::Multiple);
    assert!(form.questions[1].allow_note);
    assert_eq!(form.questions[2].kind, FormKind::Text);
    assert_eq!(form.questions[2].placeholder.as_deref(), Some("a name"));
    // Malformed answers are refused and the questionnaire stays open.
    let refused = |e: &mut Engine, answers: Vec<FormAnswer>, note: Option<&str>| {
        answer(
            e,
            &s,
            &q.id,
            Answer::Form {
                answers,
                note: note.map(str::to_owned),
            },
        )
        .unwrap_err()
        .message
    };
    assert_eq!(
        refused(&mut e, vec![fa("colour", &["o1"], None, None)], None),
        "\"colour\" is not one of the questions."
    );
    assert_eq!(
        refused(&mut e, vec![fa("approach", &["Patch"], None, None)], None),
        "That is not one of the choices."
    );
    assert_eq!(
        refused(
            &mut e,
            vec![fa("approach", &["rewrite", "o2"], None, None)],
            None
        ),
        "Question \"approach\" takes one choice."
    );
    assert_eq!(
        refused(
            &mut e,
            vec![
                fa("scope", &["o1"], None, None),
                fa("scope", &["o2"], None, None)
            ],
            None
        ),
        "Question \"scope\" was answered twice."
    );
    assert_eq!(
        refused(&mut e, vec![fa("scope", &["o1"], Some("more"), None)], None),
        "Question \"scope\" does not take a typed answer."
    );
    assert_eq!(
        refused(&mut e, vec![fa("name", &["o1"], None, None)], None),
        "Question \"name\" takes text, not a choice."
    );
    assert_eq!(
        refused(
            &mut e,
            vec![fa("approach", &["o2"], None, Some("hm"))],
            None
        ),
        "Question \"approach\" does not take a note."
    );
    assert!(
        refused(
            &mut e,
            vec![fa("name", &[], Some(&"x".repeat(10_001)), None)],
            None
        )
        .starts_with("An answer can be at most")
    );
    assert_eq!(
        answer(&mut e, &s, &q.id, Answer::Text { value: "x".into() })
            .unwrap_err()
            .message,
        "This questionnaire needs its answers as a whole."
    );
    assert!(e.snapshot(&s).ui.questions.len() == 1, "still open");
    // A partial submission: a typed "other" for the first, two choices and a note for the
    // second, the third left out; a note for the whole.
    answer(
        &mut e,
        &s,
        &q.id,
        Answer::Form {
            answers: vec![
                fa("approach", &[], Some("  Hybrid  "), None),
                fa("scope", &["o1", "o3"], None, Some("skip the docs")),
            ],
            note: Some("keep it small".into()),
        },
    )
    .unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let snap = e.snapshot(&s);
    assert!(snap.ui.questions.is_empty());
    let results = tool_results(&snap);
    assert_eq!(results.len(), 1);
    assert!(!results[0].0);
    let text = &results[0].1;
    assert!(
        text.starts_with("The person answered 2 of 3 questions:\n"),
        "{text}"
    );
    assert!(
        text.contains("- approach (Which approach?): Hybrid\n"),
        "{text}"
    );
    assert!(
        text.contains("- scope (Which parts?): Tests; Code (note: skip the docs)\n"),
        "{text}"
    );
    assert!(
        text.contains("- name (What should it be called?): not answered"),
        "{text}"
    );
    assert!(
        text.contains("Do not assume answers for the questions left unanswered."),
        "{text}"
    );
    assert!(
        text.ends_with("Note from the person: keep it small"),
        "{text}"
    );
    let d = &results[0].2.as_ref().unwrap()["askUserQuestion"];
    assert_eq!(d["version"], 2);
    assert_eq!(d["outcome"], "partial");
    assert_eq!(d["submitted"], true);
    assert_eq!(d["note"], "keep it small");
    assert_eq!(d["questions"][0]["id"], "approach");
    assert_eq!(d["questions"][0]["answered"], true);
    assert_eq!(d["questions"][0]["selected"], json!([]));
    assert_eq!(d["questions"][0]["text"], "Hybrid");
    assert_eq!(
        d["questions"][1]["selected"],
        json!([{"id": "o1", "label": "Tests"}, {"id": "o3", "label": "Code"}])
    );
    assert_eq!(d["questions"][1]["note"], "skip the docs");
    assert_eq!(d["questions"][2]["answered"], false);
    assert_eq!(d["questions"][2]["kind"], "text");
    // The same runtime id cannot be answered again.
    assert_eq!(
        answer(
            &mut e,
            &s,
            &q.id,
            Answer::Form {
                answers: vec![],
                note: None
            }
        )
        .unwrap_err()
        .message,
        "That question is no longer open."
    );
}

#[test]
fn an_empty_submission_a_default_and_a_decline_are_all_unanswered_but_distinct() {
    let fx = Fixture::new();
    let provider = Scripted::new(asking(form_args()));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    // Submitting nothing: the default is not applied by the engine, a note alone answers
    // nothing, and the outcome says the person did submit.
    e.prompt(&s, "r1", "go").unwrap();
    let q = open_question(&mut e, &s);
    answer(
        &mut e,
        &s,
        &q.id,
        Answer::Form {
            answers: vec![fa("scope", &[], None, Some("later"))],
            note: Some("not now".into()),
        },
    )
    .unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let snap = e.snapshot(&s);
    let results = tool_results(&snap);
    assert!(!results[0].0);
    assert!(
        results[0]
            .1
            .starts_with("The person submitted the questionnaire without answering any question."),
        "{}",
        results[0].1
    );
    assert!(results[0].1.ends_with("Note from the person: not now"));
    let d = &results[0].2.as_ref().unwrap()["askUserQuestion"];
    assert_eq!(d["outcome"], "unanswered");
    assert_eq!(d["submitted"], true);
    assert_eq!(
        d["questions"][0]["answered"], false,
        "a default is not an answer"
    );
    assert_eq!(d["questions"][1]["note"], "later");
    // Declining: unanswered, not submitted.
    e.prompt(&s, "r2", "again").unwrap();
    let q = open_question(&mut e, &s);
    e.call(Call::CancelQuestion {
        session: s.clone(),
        question: q.id,
    })
    .unwrap();
    assert_eq!(e.settled("r2"), RunOutcome::Completed);
    let results = tool_results(&e.snapshot(&s));
    assert_eq!(results.len(), 2);
    assert!(
        results[1]
            .1
            .starts_with("The person did not answer (the questionnaire")
    );
    let d = &results[1].2.as_ref().unwrap()["askUserQuestion"];
    assert_eq!(
        (d["version"].as_u64(), d["outcome"].as_str()),
        (Some(2), Some("unanswered"))
    );
    assert_eq!(d["submitted"], false);
}

#[test]
fn questionnaire_bounds_are_refused_before_anything_is_shown() {
    let cases: Vec<(serde_json::Value, &str)> = vec![
        (
            json!({"questions": []}),
            "questions must list at least one question",
        ),
        (
            json!({"questions": [{"prompt": "a"}, {"prompt": "b"}, {"prompt": "c"}, {"prompt": "d"}, {"prompt": "e"}]}),
            "at most 4 questions",
        ),
        (
            json!({"questions": [{"id": "x", "prompt": "a"}, {"id": "x", "prompt": "b"}]}),
            "Question ids must be unique",
        ),
        (
            json!({"questions": [{"id": "bad id", "prompt": "a"}]}),
            "must be 1-64 letters",
        ),
        // A missing prompt is refused by the tool schema before the parser.
        (
            json!({"questions": [{"prompt": "   ", "options": [{"label": "A"}]}]}),
            "has no prompt",
        ),
        (
            json!({"questions": [{"prompt": "a", "kind": "text", "options": [{"label": "A"}]}]}),
            "cannot offer options",
        ),
        (
            json!({"questions": [{"prompt": "a", "kind": "multiple"}]}),
            "offers no options",
        ),
        (
            json!({"questions": [{"prompt": "a", "options": [{"label": "A"}, {"label": "A"}]}]}),
            "option labels must be unique",
        ),
        (
            json!({"questions": [{"prompt": "a", "options": [{"label": "A"}], "default": "zz"}]}),
            "is not one of its options",
        ),
        (
            json!({"questions": [{"prompt": "a", "preview": "x".repeat(4001), "options": [{"label": "A"}]}]}),
            "preview can be at most 4000",
        ),
        (
            json!({"title": "a\nb", "questions": [{"prompt": "a"}]}),
            "title must be one line",
        ),
    ];
    for (args, expected) in cases {
        let fx = Fixture::new();
        let provider = Scripted::new(asking(args.clone()));
        let mut e = fx.start(&provider);
        let s = e.create(&fx.project, "c");
        e.prompt(&s, "r1", "go").unwrap();
        assert_eq!(e.settled("r1"), RunOutcome::Completed);
        let snap = e.snapshot(&s);
        assert!(snap.ui.questions.is_empty(), "{args}");
        let results = tool_results(&snap);
        assert!(results[0].0, "{args}: {}", results[0].1);
        assert!(results[0].1.contains(expected), "{args}: {}", results[0].1);
        assert_eq!(
            results[0].2.as_ref().unwrap()["askUserQuestion"]["outcome"],
            "invalid"
        );
        assert_eq!(
            results[0].2.as_ref().unwrap()["askUserQuestion"]["version"],
            2
        );
        e.handle.shutdown(Duration::from_millis(100));
    }
}

#[test]
fn a_restart_asks_the_same_questionnaire_again_with_the_same_ids_and_source() {
    let fx = Fixture::new();
    let s;
    let first;
    {
        let provider = Scripted::new(asking(form_args()));
        let mut e = fx.start(&provider);
        s = e.create(&fx.project, "c");
        e.prompt(&s, "r1", "go").unwrap();
        first = open_question(&mut e, &s);
        e.handle.shutdown(Duration::from_millis(200));
    }
    let provider = Scripted::new(asking(form_args()));
    let mut e = fx.start(&provider);
    e.snapshot(&s);
    let again = open_question(&mut e, &s);
    assert_ne!(again.id, first.id);
    assert_eq!(again.source, first.source, "the same call asks again");
    assert_eq!(again.form, first.form, "same questions, same ids");
    assert!(tool_results(&e.snapshot(&s)).is_empty());
    answer(
        &mut e,
        &s,
        &again.id,
        Answer::Form {
            answers: vec![fa("approach", &["o2"], None, None)],
            note: None,
        },
    )
    .unwrap();
    assert_eq!(settled_by_lookup(&mut e, &s, "r1"), RunOutcome::Completed);
    let results = tool_results(&e.snapshot(&s));
    assert_eq!(results.len(), 1, "one result for one call");
    assert!(results[0].1.contains("- approach (Which approach?): Patch"));
    assert_eq!(
        provider.requests().len(),
        1,
        "no new model turn before the answer"
    );
}
