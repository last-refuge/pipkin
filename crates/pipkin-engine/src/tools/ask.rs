//! `ask_user_question`: the model asks the person one question and gets a typed answer back.
//!
//! The call is routed through the session's question host ([`ToolHost::ask`]), so the same
//! presentation that shows extension questions shows it, and the same `AnswerQuestion` /
//! `CancelQuestion` calls settle it. The tool result is a versioned contract
//! ([`RESULT_VERSION`]): the model-visible text says what the person did, and `details`
//! carries the structured outcome under the `askUserQuestion` key:
//!
//! - `answered` with `answer: {kind: "choice", value, label, index}` or
//!   `{kind: "text", text}`;
//! - `unanswered` when the person declined, the question was withdrawn, or the host refused
//!   it (a notice explains); `choseOther: true` when the person picked "Other" and then
//!   declined to type an answer;
//! - `unavailable` (an error result) when no presentation can answer.
//!
//! A suggested option is never an answer: only what the person submits is returned. The call
//! is replay-safe: asking has no external effect, so after an engine restart an unsettled
//! question is simply asked again under a new runtime id (the engine's recovery contract
//! compares the recorded tool contract first, like `subagent`). A question answered before
//! the restart has its result committed and is never asked twice.
//!
//! Bounds are enforced here, before anything reaches a presentation: the question text,
//! the number of options, their labels and descriptions; labels must be unique and
//! non-empty. Option values the person sees are `o1..on` (labels are presentation text, not
//! identifiers), plus [`OTHER_VALUE`] when the model allows a free-text alternative.

use pipkin_engine_protocol::{Answer, Block, QuestionOption};
use serde_json::{Value, json};

use super::{Tool, ToolCtx, ToolError, ToolOutput};
use crate::extensions::questions::{
    Question, UI_MAX_MESSAGE_CHARS, UI_MAX_TITLE_CHARS, clip_utf16, option,
};
use crate::provider::{BoxFuture, ToolSpec};

pub const ASK_USER_QUESTION: &str = "ask_user_question";

/// The version of the `details.askUserQuestion` contract.
pub const RESULT_VERSION: u32 = 1;
/// Most choices one question offers.
pub const MAX_OPTIONS: usize = 10;
/// Longest option label (UTF-16 units, like the host's limits).
pub const MAX_LABEL_CHARS: usize = 200;
/// Longest option description.
pub const MAX_DESCRIPTION_CHARS: usize = 300;
/// Longest question text (the host's message limit).
pub const MAX_QUESTION_CHARS: usize = UI_MAX_MESSAGE_CHARS;
/// The value of the "Other" choice a question offers when `allow_other` is set.
pub const OTHER_VALUE: &str = "other";
/// The label of that choice.
pub const OTHER_LABEL: &str = "Other (type an answer)";
/// The title shown when the question text is too long to be the title itself.
pub const LONG_QUESTION_TITLE: &str = "A question from the assistant";
/// The title of the follow-up text question after "Other" was chosen.
pub const OTHER_FOLLOW_UP_TITLE: &str = "Your answer";

pub const DESCRIPTION: &str = "Ask the person one question and wait for their answer. Use it when you need a decision only they can make (a choice between approaches, a missing requirement, permission for something consequential); do not use it for things you can find out yourself. Give up to 10 options for a choice, or none for a free-text answer. The result reports exactly what the person submitted; if they declined, continue without assuming an answer.";

/// What the session host returns for a question.
#[derive(Clone, Debug, PartialEq)]
pub enum AskOutcome {
    /// The person submitted this answer.
    Answered(Answer),
    /// The person declined, the question was withdrawn, or the host refused to show it.
    Unanswered,
    /// No presentation can answer questions right now (the reason is for the model).
    Unavailable(String),
}

pub struct AskUserQuestionTool;

struct Request {
    question: String,
    options: Vec<QuestionOption>,
    allow_other: bool,
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

fn invalid(message: String) -> ToolError {
    ToolError {
        message,
        diagnostics: vec![],
        details: Some(details(json!({ "outcome": "invalid" }))),
    }
}

/// `{"askUserQuestion": {"version": 1, ...fields}}`.
fn details(fields: Value) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("version".into(), RESULT_VERSION.into());
    if let Value::Object(fields) = fields {
        body.extend(fields);
    }
    json!({ "askUserQuestion": Value::Object(body) })
}

fn parse(args: &Value) -> Result<Request, ToolError> {
    let question = args
        .get("question")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if question.is_empty() {
        return Err(invalid("The question must not be empty".into()));
    }
    if utf16_len(question) > MAX_QUESTION_CHARS {
        return Err(invalid(format!(
            "The question can be at most {MAX_QUESTION_CHARS} characters"
        )));
    }
    let allow_other = args
        .get("allow_other")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let raw = args
        .get("options")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if raw.len() > MAX_OPTIONS {
        return Err(invalid(format!(
            "A question can offer at most {MAX_OPTIONS} options"
        )));
    }
    let mut options = Vec::with_capacity(raw.len());
    for (index, item) in raw.iter().enumerate() {
        let label = item
            .get("label")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        if label.is_empty() {
            return Err(invalid(format!("Option {} has no label", index + 1)));
        }
        if utf16_len(label) > MAX_LABEL_CHARS {
            return Err(invalid(format!(
                "Option {} label can be at most {MAX_LABEL_CHARS} characters",
                index + 1
            )));
        }
        if options.iter().any(|o: &QuestionOption| o.label == label) {
            return Err(invalid(format!(
                "Option labels must be unique: \"{label}\""
            )));
        }
        let description = item
            .get("description")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|d| !d.is_empty());
        if let Some(description) = description
            && utf16_len(description) > MAX_DESCRIPTION_CHARS
        {
            return Err(invalid(format!(
                "Option {} description can be at most {MAX_DESCRIPTION_CHARS} characters",
                index + 1
            )));
        }
        options.push(option(&format!("o{}", index + 1), label, description));
    }
    if options.is_empty() && allow_other {
        // A free-text question already takes any answer.
        return Ok(Request {
            question: question.to_owned(),
            options,
            allow_other: false,
        });
    }
    Ok(Request {
        question: question.to_owned(),
        options,
        allow_other,
    })
}

/// The question as the person sees it: the text is the title when it fits, otherwise a
/// generic title with the text as the message.
fn presented(question: &str, options: Vec<QuestionOption>) -> Question {
    let fits = utf16_len(question) <= UI_MAX_TITLE_CHARS && !question.contains('\n');
    let (title, message) = if fits {
        (question.to_owned(), None)
    } else {
        (LONG_QUESTION_TITLE.to_owned(), Some(question.to_owned()))
    };
    let mut q = if options.is_empty() {
        Question::input(title)
    } else {
        Question::select(title, options)
    };
    if let Some(message) = message {
        q = q.with_message(message);
    }
    q
}

fn answered_text(text: &str) -> ToolOutput {
    ToolOutput {
        content: Some(vec![Block::text(format!("The person answered: {text}"))]),
        details: Some(details(json!({
            "outcome": "answered",
            "answer": { "kind": "text", "text": text },
        }))),
        ..ToolOutput::default()
    }
}

fn answered_choice(index: usize, value: &str, label: &str) -> ToolOutput {
    ToolOutput {
        content: Some(vec![Block::text(format!("The person chose: {label}"))]),
        details: Some(details(json!({
            "outcome": "answered",
            "answer": { "kind": "choice", "value": value, "label": label, "index": index },
        }))),
        ..ToolOutput::default()
    }
}

fn unanswered(chose_other: bool) -> ToolOutput {
    let mut fields = json!({ "outcome": "unanswered" });
    if chose_other {
        fields["choseOther"] = Value::Bool(true);
    }
    ToolOutput {
        content: Some(vec![Block::text(
            "The person did not answer (the question was declined or withdrawn). Do not assume an answer; continue without it, or ask again later if it is still needed.",
        )]),
        details: Some(details(fields)),
        ..ToolOutput::default()
    }
}

fn unavailable(reason: String) -> ToolError {
    ToolError {
        message: format!("Cannot ask the person right now: {reason}"),
        diagnostics: vec![],
        details: Some(details(json!({
            "outcome": "unavailable",
            "reason": reason,
        }))),
    }
}

impl Tool for AskUserQuestionTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            constrained_sampling: None,
            name: ASK_USER_QUESTION.into(),
            description: DESCRIPTION.into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "question": {
                        "type": "string",
                        "description": "The question, exactly as the person will read it"
                    },
                    "options": {
                        "type": "array",
                        "description": "Up to 10 choices for a single-choice question. Omit for a free-text answer.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "label": {"type": "string", "description": "The choice as shown (unique, at most 200 characters)"},
                                "description": {"type": "string", "description": "What choosing it means (at most 300 characters)"}
                            },
                            "required": ["label"]
                        }
                    },
                    "allow_other": {
                        "type": "boolean",
                        "description": "With options: also offer \"Other\" so the person can type their own answer"
                    }
                },
                "required": ["question"]
            }),
        }
    }

    fn replay_safe(&self) -> bool {
        true
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolCtx,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let request = parse(&args)?;
            let Some(host) = &ctx.host else {
                return Err(ToolError::new("No execution host is configured"));
            };
            let mut options = request.options.clone();
            if request.allow_other {
                options.push(option(OTHER_VALUE, OTHER_LABEL, None));
            }
            let question = presented(&request.question, options);
            let outcome = host.ask(&ctx.call_id, question, ctx.cancel.clone()).await;
            if ctx.cancel.is_cancelled() {
                return Err(ToolError::new(
                    "The question was withdrawn: the run was stopped",
                ));
            }
            match outcome {
                AskOutcome::Unavailable(reason) => Err(unavailable(reason)),
                AskOutcome::Unanswered => Ok(unanswered(false)),
                AskOutcome::Answered(Answer::Text { value }) => Ok(answered_text(&value)),
                AskOutcome::Answered(Answer::Confirm { value }) => {
                    // The host never pairs a confirm answer with these questions; report
                    // what was submitted rather than inventing a choice.
                    Ok(answered_text(if value { "yes" } else { "no" }))
                }
                AskOutcome::Answered(Answer::Choice { value }) => {
                    if value == OTHER_VALUE && request.allow_other {
                        let follow_up = Question::input(OTHER_FOLLOW_UP_TITLE)
                            .with_message(clip_utf16(&request.question, UI_MAX_MESSAGE_CHARS));
                        let outcome = host.ask(&ctx.call_id, follow_up, ctx.cancel.clone()).await;
                        if ctx.cancel.is_cancelled() {
                            return Err(ToolError::new(
                                "The question was withdrawn: the run was stopped",
                            ));
                        }
                        return match outcome {
                            AskOutcome::Unavailable(reason) => Err(unavailable(reason)),
                            AskOutcome::Unanswered => Ok(unanswered(true)),
                            AskOutcome::Answered(Answer::Text { value })
                            | AskOutcome::Answered(Answer::Choice { value }) => {
                                Ok(answered_text(&value))
                            }
                            AskOutcome::Answered(Answer::Confirm { value }) => {
                                Ok(answered_text(if value { "yes" } else { "no" }))
                            }
                        };
                    }
                    match request
                        .options
                        .iter()
                        .enumerate()
                        .find(|(_, o)| o.value == value)
                    {
                        Some((index, chosen)) => {
                            Ok(answered_choice(index, &chosen.value, &chosen.label))
                        }
                        // The host validates choices against the question it showed; an
                        // unknown value can only come from a host that is not this one.
                        None => Err(ToolError::new(format!(
                            "The answer \"{value}\" is not one of the offered choices"
                        ))),
                    }
                }
            }
        })
    }
}
