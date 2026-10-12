//! `ask_user_question` against a scripted host: the question it presents, the typed result
//! contract for every outcome, argument bounds, cancellation and the unavailable path.

use std::sync::{Arc, Mutex};

use pipkin_engine_protocol::{Answer, QuestionKind};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{ctx, execute, output_text, result_text, temp};
use crate::extensions::questions::Question;
use crate::provider::BoxFuture;
use crate::tools::ask::{
    ASK_USER_QUESTION, AskOutcome, AskUserQuestionTool, LONG_QUESTION_TITLE, OTHER_FOLLOW_UP_TITLE,
    OTHER_LABEL, OTHER_VALUE, RESULT_VERSION,
};
use crate::tools::{Tool, ToolCtx, ToolHost, run};

/// A host that answers each question from a script and records what it was asked.
struct Scripted {
    outcomes: Mutex<Vec<AskOutcome>>,
    asked: Mutex<Vec<Question>>,
    /// Cancel this token before answering (a Stop while the question is open).
    cancel_on_ask: Option<CancellationToken>,
}

impl Scripted {
    fn new(outcomes: Vec<AskOutcome>) -> Arc<Scripted> {
        Arc::new(Scripted {
            outcomes: Mutex::new(outcomes),
            asked: Mutex::new(vec![]),
            cancel_on_ask: None,
        })
    }

    fn asked(&self) -> Vec<Question> {
        self.asked.lock().unwrap().clone()
    }
}

impl ToolHost for Scripted {
    fn subagent<'a>(
        &'a self,
        _call_id: &'a str,
        _task: &'a str,
        _cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<(String, Value), String>> {
        Box::pin(async { Err("not in this test".into()) })
    }

    fn ask<'a>(
        &'a self,
        _call_id: &'a str,
        question: Question,
        _cancel: CancellationToken,
    ) -> BoxFuture<'a, AskOutcome> {
        self.asked.lock().unwrap().push(question);
        if let Some(cancel) = &self.cancel_on_ask {
            cancel.cancel();
        }
        let mut outcomes = self.outcomes.lock().unwrap();
        let outcome = if outcomes.is_empty() {
            AskOutcome::Unanswered
        } else {
            outcomes.remove(0)
        };
        Box::pin(async move { outcome })
    }
}

fn hosted(host: Arc<Scripted>) -> ToolCtx {
    let dir = temp();
    ToolCtx {
        host: Some(host),
        ..ctx(dir.path())
    }
}

fn choice(value: &str) -> AskOutcome {
    AskOutcome::Answered(Answer::Choice {
        value: value.into(),
    })
}

fn text(value: &str) -> AskOutcome {
    AskOutcome::Answered(Answer::Text {
        value: value.into(),
    })
}

fn contract(details: &Option<Value>) -> &Value {
    let details = details.as_ref().expect("details");
    &details["askUserQuestion"]
}

#[tokio::test]
async fn a_choice_question_presents_stable_values_and_returns_the_chosen_label() {
    let host = Scripted::new(vec![choice("o2")]);
    let ctx = hosted(host.clone());
    let (result, _) = execute(
        &AskUserQuestionTool,
        json!({
            "question": "Which approach?",
            "options": [
                {"label": "Rewrite", "description": "Start over"},
                {"label": "Patch"},
            ]
        }),
        &ctx,
    )
    .await;
    let output = result.unwrap();
    assert!(!output.is_error);
    assert_eq!(output_text(&output), "The person chose: Patch");
    let c = contract(&output.details);
    assert_eq!(c["version"], RESULT_VERSION);
    assert_eq!(c["outcome"], "answered");
    assert_eq!(
        c["answer"],
        json!({"kind": "choice", "value": "o2", "label": "Patch", "index": 1})
    );
    let asked = host.asked();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].kind, QuestionKind::Select);
    assert_eq!(asked[0].title, "Which approach?");
    assert_eq!(asked[0].message, None);
    let shown: Vec<(String, String, Option<String>)> = asked[0]
        .options
        .iter()
        .map(|o| (o.value.clone(), o.label.clone(), o.description.clone()))
        .collect();
    assert_eq!(
        shown,
        vec![
            ("o1".into(), "Rewrite".into(), Some("Start over".into())),
            ("o2".into(), "Patch".into(), None),
        ]
    );
}

#[tokio::test]
async fn without_options_the_question_takes_free_text() {
    let host = Scripted::new(vec![text("call it pipkin-jobs")]);
    let ctx = hosted(host.clone());
    let (result, _) = execute(
        &AskUserQuestionTool,
        json!({"question": "What should the crate be called?"}),
        &ctx,
    )
    .await;
    let output = result.unwrap();
    assert_eq!(
        output_text(&output),
        "The person answered: call it pipkin-jobs"
    );
    assert_eq!(
        contract(&output.details)["answer"],
        json!({"kind": "text", "text": "call it pipkin-jobs"})
    );
    assert_eq!(host.asked()[0].kind, QuestionKind::Input);
    assert!(host.asked()[0].options.is_empty());
}

#[tokio::test]
async fn allow_other_adds_a_choice_that_leads_to_a_text_follow_up() {
    let host = Scripted::new(vec![
        choice(OTHER_VALUE),
        text("neither; use a feature flag"),
    ]);
    let ctx = hosted(host.clone());
    let (result, _) = execute(
        &AskUserQuestionTool,
        json!({
            "question": "Which approach?",
            "options": [{"label": "A"}, {"label": "B"}],
            "allow_other": true
        }),
        &ctx,
    )
    .await;
    let output = result.unwrap();
    assert_eq!(
        output_text(&output),
        "The person answered: neither; use a feature flag"
    );
    let asked = host.asked();
    assert_eq!(asked.len(), 2);
    let last = asked[0].options.last().unwrap();
    assert_eq!(
        (last.value.as_str(), last.label.as_str()),
        (OTHER_VALUE, OTHER_LABEL)
    );
    assert_eq!(asked[1].kind, QuestionKind::Input);
    assert_eq!(asked[1].title, OTHER_FOLLOW_UP_TITLE);
    assert_eq!(asked[1].message.as_deref(), Some("Which approach?"));
}

#[tokio::test]
async fn declining_the_follow_up_is_unanswered_and_says_other_was_chosen() {
    let host = Scripted::new(vec![choice(OTHER_VALUE), AskOutcome::Unanswered]);
    let ctx = hosted(host.clone());
    let (result, _) = execute(
        &AskUserQuestionTool,
        json!({"question": "Which?", "options": [{"label": "A"}], "allow_other": true}),
        &ctx,
    )
    .await;
    let output = result.unwrap();
    assert!(!output.is_error);
    assert!(output_text(&output).starts_with("The person did not answer"));
    let c = contract(&output.details);
    assert_eq!(c["outcome"], "unanswered");
    assert_eq!(c["choseOther"], true);
}

#[tokio::test]
async fn an_unanswered_question_is_reported_without_inventing_an_answer() {
    let host = Scripted::new(vec![AskOutcome::Unanswered]);
    let ctx = hosted(host.clone());
    let (result, _) = execute(
        &AskUserQuestionTool,
        json!({"question": "Proceed?", "options": [{"label": "Yes"}, {"label": "No"}]}),
        &ctx,
    )
    .await;
    let output = result.unwrap();
    assert!(!output.is_error, "declining is not a tool failure");
    assert!(output_text(&output).starts_with("The person did not answer"));
    let c = contract(&output.details);
    assert_eq!(c["outcome"], "unanswered");
    assert!(c.get("answer").is_none());
    assert!(c.get("choseOther").is_none());
}

#[tokio::test]
async fn no_presentation_is_a_typed_unavailable_error() {
    let host = Scripted::new(vec![AskOutcome::Unavailable("nobody is connected".into())]);
    let ctx = hosted(host.clone());
    let result = run(&AskUserQuestionTool, json!({"question": "Proceed?"}), &ctx).await;
    assert!(result.is_error);
    assert!(
        result_text(&result).contains("Cannot ask the person right now: nobody is connected"),
        "{}",
        result_text(&result)
    );
    let c = contract(&result.details);
    assert_eq!(c["outcome"], "unavailable");
    assert_eq!(c["reason"], "nobody is connected");
}

#[tokio::test]
async fn the_default_host_has_no_presentation() {
    struct Bare;
    impl ToolHost for Bare {
        fn subagent<'a>(
            &'a self,
            _: &'a str,
            _: &'a str,
            _: CancellationToken,
        ) -> BoxFuture<'a, Result<(String, Value), String>> {
            Box::pin(async { Err("no".into()) })
        }
    }
    let dir = temp();
    let ctx = ToolCtx {
        host: Some(Arc::new(Bare)),
        ..ctx(dir.path())
    };
    let result = run(&AskUserQuestionTool, json!({"question": "Proceed?"}), &ctx).await;
    assert!(result.is_error);
    assert_eq!(contract(&result.details)["outcome"], "unavailable");
}

#[tokio::test]
async fn a_stop_while_the_question_is_open_aborts_the_call() {
    let cancel = CancellationToken::new();
    let host = Arc::new(Scripted {
        outcomes: Mutex::new(vec![AskOutcome::Unanswered]),
        asked: Mutex::new(vec![]),
        cancel_on_ask: Some(cancel.clone()),
    });
    let dir = temp();
    let ctx = ToolCtx {
        host: Some(host),
        cancel,
        ..ctx(dir.path())
    };
    let result = run(&AskUserQuestionTool, json!({"question": "Proceed?"}), &ctx).await;
    assert!(result.is_error);
    assert!(
        result_text(&result).contains("Tool ask_user_question was aborted"),
        "{}",
        result_text(&result)
    );
}

#[tokio::test]
async fn a_long_or_multiline_question_becomes_the_message_under_a_generic_title() {
    let long = "x".repeat(301);
    let host = Scripted::new(vec![text("a"), text("b")]);
    let ctx = hosted(host.clone());
    execute(&AskUserQuestionTool, json!({"question": long}), &ctx)
        .await
        .0
        .unwrap();
    execute(
        &AskUserQuestionTool,
        json!({"question": "Two lines\nof question"}),
        &ctx,
    )
    .await
    .0
    .unwrap();
    for q in host.asked() {
        assert_eq!(q.title, LONG_QUESTION_TITLE);
        assert!(q.message.is_some());
    }
}

#[tokio::test]
async fn arguments_are_bounded_before_anything_is_shown() {
    let cases: Vec<(Value, &str)> = vec![
        (json!({"question": "   "}), "must not be empty"),
        (
            json!({"question": "y".repeat(2001)}),
            "at most 2000 characters",
        ),
        (
            json!({"question": "q", "options": (0..11).map(|i| json!({"label": format!("o{i}")})).collect::<Vec<_>>()}),
            "at most 10 options",
        ),
        (
            json!({"question": "q", "options": [{"label": "A"}, {"label": "A"}]}),
            "must be unique",
        ),
        (
            json!({"question": "q", "options": [{"label": " "}]}),
            "has no label",
        ),
        (
            json!({"question": "q", "options": [{"label": "l".repeat(201)}]}),
            "label can be at most 200",
        ),
        (
            json!({"question": "q", "options": [{"label": "A", "description": "d".repeat(301)}]}),
            "description can be at most 300",
        ),
    ];
    for (args, expected) in cases {
        let host = Scripted::new(vec![text("never")]);
        let ctx = hosted(host.clone());
        let (result, _) = execute(&AskUserQuestionTool, args.clone(), &ctx).await;
        let error = result.expect_err(&format!("{args}"));
        assert!(
            error.message.contains(expected),
            "{args}: {}",
            error.message
        );
        assert_eq!(contract(&error.details)["outcome"], "invalid");
        assert!(host.asked().is_empty(), "nothing shown for {args}");
    }
}

#[tokio::test]
async fn schema_validation_rejects_the_wrong_shapes() {
    let host = Scripted::new(vec![]);
    let ctx = hosted(host.clone());
    let result = run(
        &AskUserQuestionTool,
        json!({"question": "q", "options": "A, B"}),
        &ctx,
    )
    .await;
    assert!(result.is_error);
    assert!(result_text(&result).contains("Validation failed for tool \"ask_user_question\""));
    let result = run(&AskUserQuestionTool, json!({"options": []}), &ctx).await;
    assert!(result.is_error);
    assert!(host.asked().is_empty());
}

#[test]
fn the_tool_is_replay_safe_and_named_for_the_catalog() {
    let spec = AskUserQuestionTool.spec();
    assert_eq!(spec.name, ASK_USER_QUESTION);
    assert!(AskUserQuestionTool.replay_safe());
    assert!(!AskUserQuestionTool.sequential());
    assert_eq!(spec.parameters["required"], json!(["question"]));
}
