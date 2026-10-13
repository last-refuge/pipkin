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
//!
//! ASK-2: `questions` (one to four) asks a **questionnaire** instead, answered as a whole
//! ([`FORM_VERSION`]): stable question and option ids (the model's own, or `q1..`/`o1..`),
//! single-choice, multiple-choice and free-text questions, optional bounded Markdown
//! previews, per-answer notes and one note for the whole. The result is contract version 2:
//! `outcome` is `answered` (every question), `partial` (some), or `unanswered` (none, or
//! declined: `submitted` says which), and `questions` lists each question's submitted answer.
//! A preselected default is never an answer until the person submits it, and a note never
//! answers a question.

use pipkin_engine_protocol::{
    Answer, Block, FormAnswer, FormKind, FormOptionView, FormQuestionView, FormView, QuestionOption,
};
use serde_json::{Value, json};

use super::{Tool, ToolCtx, ToolError, ToolOutput};
use crate::extensions::questions::{
    Question, UI_MAX_MESSAGE_CHARS, UI_MAX_TITLE_CHARS, clip_utf16, option,
};
use crate::provider::{BoxFuture, ToolSpec};

pub const ASK_USER_QUESTION: &str = "ask_user_question";

/// The version of the `details.askUserQuestion` contract for a single question.
pub const RESULT_VERSION: u32 = 1;
/// The version of the `details.askUserQuestion` contract for a questionnaire.
pub const FORM_RESULT_VERSION: u32 = 2;
/// The questionnaire schema version carried on the question view.
pub const FORM_VERSION: u32 = 1;
/// Most questions in one questionnaire.
pub const MAX_QUESTIONS: usize = 4;
/// Longest question or option id.
pub const MAX_ID_CHARS: usize = 64;
/// Longest Markdown preview (UTF-16 units).
pub const MAX_PREVIEW_CHARS: usize = 4_000;
/// The title of a questionnaire without one of its own.
pub const FORM_TITLE: &str = "A few questions from the assistant";
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

pub const DESCRIPTION: &str = "Ask the person a question and wait for their answer. Use it when you need a decision only they can make (a choice between approaches, a missing requirement, permission for something consequential); do not use it for things you can find out yourself. One question: give `question`, with up to 10 options for a choice or none for a free-text answer. Several related questions (up to 4): give `questions` instead, each single-choice, multiple-choice or text, optionally with a Markdown preview and a note field; the person answers them together and may leave some unanswered. The result reports exactly what the person submitted; if they declined, continue without assuming an answer.";

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

/// `{"askUserQuestion": {"version": 2, ...fields}}`.
fn form_details(fields: Value) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("version".into(), FORM_RESULT_VERSION.into());
    if let Value::Object(fields) = fields {
        body.extend(fields);
    }
    json!({ "askUserQuestion": Value::Object(body) })
}

fn form_invalid(message: String) -> ToolError {
    ToolError {
        message,
        diagnostics: vec![],
        details: Some(form_details(json!({ "outcome": "invalid" }))),
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_CHARS
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn bounded<'a>(
    item: &'a Value,
    field: &str,
    max: usize,
    where_: &str,
) -> Result<Option<&'a str>, ToolError> {
    let text = item
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty());
    if let Some(text) = text
        && utf16_len(text) > max
    {
        return Err(form_invalid(format!(
            "{where_} {field} can be at most {max} characters"
        )));
    }
    Ok(text)
}

struct FormRequest {
    title: String,
    form: FormView,
}

/// `questions`: one to four questions with stable ids, bounded texts and previews.
fn parse_form(args: &Value) -> Result<FormRequest, ToolError> {
    let raw = args
        .get("questions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if raw.is_empty() {
        return Err(form_invalid(
            "questions must list at least one question".into(),
        ));
    }
    if raw.len() > MAX_QUESTIONS {
        return Err(form_invalid(format!(
            "A questionnaire can have at most {MAX_QUESTIONS} questions"
        )));
    }
    let title = match bounded(args, "title", UI_MAX_TITLE_CHARS, "The questionnaire")? {
        Some(t) if !t.contains('\n') => t.to_owned(),
        Some(_) => {
            return Err(form_invalid(
                "The questionnaire title must be one line".into(),
            ));
        }
        None => FORM_TITLE.to_owned(),
    };
    let allow_note = args.get("note").and_then(Value::as_bool).unwrap_or(false);
    let mut questions: Vec<FormQuestionView> = Vec::with_capacity(raw.len());
    for (qi, item) in raw.iter().enumerate() {
        let n = qi + 1;
        let where_q = format!("Question {n}");
        let id = match item.get("id").and_then(Value::as_str).map(str::trim) {
            None | Some("") => format!("q{n}"),
            Some(id) if valid_id(id) => id.to_owned(),
            Some(id) => {
                return Err(form_invalid(format!(
                    "{where_q} id {id:?} must be 1-64 letters, digits, '_' or '-'"
                )));
            }
        };
        if questions.iter().any(|q| q.id == id) {
            return Err(form_invalid(format!("Question ids must be unique: {id:?}")));
        }
        let Some(prompt) = bounded(item, "prompt", MAX_QUESTION_CHARS, &where_q)? else {
            return Err(form_invalid(format!("{where_q} has no prompt")));
        };
        let description = bounded(item, "description", MAX_DESCRIPTION_CHARS, &where_q)?;
        let preview = bounded(item, "preview", MAX_PREVIEW_CHARS, &where_q)?;
        let placeholder = bounded(item, "placeholder", MAX_LABEL_CHARS, &where_q)?;
        let raw_options = item
            .get("options")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if raw_options.len() > MAX_OPTIONS {
            return Err(form_invalid(format!(
                "{where_q} can offer at most {MAX_OPTIONS} options"
            )));
        }
        let kind = match item.get("kind").and_then(Value::as_str).map(str::trim) {
            None | Some("") => {
                if raw_options.is_empty() {
                    FormKind::Text
                } else {
                    FormKind::Single
                }
            }
            Some("single") => FormKind::Single,
            Some("multiple") => FormKind::Multiple,
            Some("text") => FormKind::Text,
            Some(other) => {
                return Err(form_invalid(format!(
                    "{where_q} kind must be single, multiple or text, not {other:?}"
                )));
            }
        };
        if kind == FormKind::Text && !raw_options.is_empty() {
            return Err(form_invalid(format!(
                "{where_q} is a text question and cannot offer options"
            )));
        }
        if kind != FormKind::Text && raw_options.is_empty() {
            return Err(form_invalid(format!("{where_q} offers no options")));
        }
        let mut options: Vec<FormOptionView> = Vec::with_capacity(raw_options.len());
        for (oi, opt) in raw_options.iter().enumerate() {
            let where_o = format!("{where_q} option {}", oi + 1);
            let oid = match opt.get("id").and_then(Value::as_str).map(str::trim) {
                None | Some("") => format!("o{}", oi + 1),
                Some(oid) if valid_id(oid) => oid.to_owned(),
                Some(oid) => {
                    return Err(form_invalid(format!(
                        "{where_o} id {oid:?} must be 1-64 letters, digits, '_' or '-'"
                    )));
                }
            };
            if options.iter().any(|o| o.id == oid) {
                return Err(form_invalid(format!(
                    "{where_q} option ids must be unique: {oid:?}"
                )));
            }
            let Some(label) = bounded(opt, "label", MAX_LABEL_CHARS, &where_o)? else {
                return Err(form_invalid(format!("{where_o} has no label")));
            };
            if options.iter().any(|o| o.label == label) {
                return Err(form_invalid(format!(
                    "{where_q} option labels must be unique: {label:?}"
                )));
            }
            options.push(FormOptionView {
                id: oid,
                label: label.to_owned(),
                description: bounded(opt, "description", MAX_DESCRIPTION_CHARS, &where_o)?
                    .map(str::to_owned),
                preview: bounded(opt, "preview", MAX_PREVIEW_CHARS, &where_o)?.map(str::to_owned),
            });
        }
        let allow_other = kind != FormKind::Text
            && item
                .get("allow_other")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        let allow_note = item.get("note").and_then(Value::as_bool).unwrap_or(false);
        let default: Vec<String> = match item.get("default") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::String(one)) => vec![one.trim().to_owned()],
            Some(Value::Array(many)) => many
                .iter()
                .filter_map(Value::as_str)
                .map(|d| d.trim().to_owned())
                .collect(),
            Some(_) => {
                return Err(form_invalid(format!(
                    "{where_q} default must be an option id or a list of them"
                )));
            }
        };
        for d in &default {
            if !options.iter().any(|o| &o.id == d) {
                return Err(form_invalid(format!(
                    "{where_q} default {d:?} is not one of its options"
                )));
            }
        }
        if kind == FormKind::Single && default.len() > 1 {
            return Err(form_invalid(format!(
                "{where_q} is single-choice and can have one default"
            )));
        }
        questions.push(FormQuestionView {
            id,
            kind,
            prompt: prompt.to_owned(),
            description: description.map(str::to_owned),
            preview: preview.map(str::to_owned),
            options,
            allow_other,
            allow_note,
            default,
            placeholder: placeholder.map(str::to_owned),
        });
    }
    Ok(FormRequest {
        title,
        form: FormView {
            version: FORM_VERSION,
            questions,
            allow_note,
        },
    })
}

/// What one submitted answer means for the model.
fn form_answer_json(q: &FormQuestionView, a: Option<&FormAnswer>) -> (bool, Value, String) {
    let selected: Vec<&FormOptionView> = a
        .map(|a| {
            a.selected
                .iter()
                .filter_map(|id| q.options.iter().find(|o| &o.id == id))
                .collect()
        })
        .unwrap_or_default();
    let text = a.and_then(|a| a.text.as_deref());
    let note = a.and_then(|a| a.note.as_deref());
    let answered = !selected.is_empty() || text.is_some();
    let kind = match q.kind {
        FormKind::Single => "single",
        FormKind::Multiple => "multiple",
        FormKind::Text => "text",
    };
    let mut line = format!("- {} ({}): ", q.id, clip_utf16(&q.prompt, 120));
    if !answered {
        line.push_str("not answered");
    } else {
        let mut parts: Vec<String> = selected.iter().map(|o| o.label.clone()).collect();
        if let Some(text) = text {
            parts.push(if selected.is_empty() {
                text.to_owned()
            } else {
                format!("other: {text}")
            });
        }
        line.push_str(&parts.join("; "));
    }
    if let Some(note) = note {
        line.push_str(&format!(" (note: {note})"));
    }
    let json = json!({
        "id": q.id,
        "kind": kind,
        "answered": answered,
        "selected": selected.iter().map(|o| json!({"id": o.id, "label": o.label})).collect::<Vec<_>>(),
        "text": text,
        "note": note,
    });
    (answered, json, line)
}

fn form_answered(form: &FormView, answers: &[FormAnswer], note: Option<&str>) -> ToolOutput {
    let mut lines = Vec::with_capacity(form.questions.len());
    let mut items = Vec::with_capacity(form.questions.len());
    let mut answered = 0;
    for q in &form.questions {
        let a = answers.iter().find(|a| a.question == q.id);
        let (ok, json, line) = form_answer_json(q, a);
        answered += usize::from(ok);
        items.push(json);
        lines.push(line);
    }
    let total = form.questions.len();
    let outcome = if answered == total {
        "answered"
    } else if answered > 0 {
        "partial"
    } else {
        "unanswered"
    };
    let mut text = if answered == 0 {
        "The person submitted the questionnaire without answering any question. Do not assume answers; continue without them, or ask again later if they are still needed.".to_owned()
    } else {
        format!(
            "The person answered {answered} of {total} question{}:\n{}",
            if total == 1 { "" } else { "s" },
            lines.join("\n")
        )
    };
    if answered > 0 && answered < total {
        text.push_str("\nDo not assume answers for the questions left unanswered.");
    }
    if let Some(note) = note {
        text.push_str(&format!("\nNote from the person: {note}"));
    }
    ToolOutput {
        content: Some(vec![Block::text(text)]),
        details: Some(form_details(json!({
            "outcome": outcome,
            "submitted": true,
            "questions": items,
            "note": note,
        }))),
        ..ToolOutput::default()
    }
}

fn form_unanswered() -> ToolOutput {
    ToolOutput {
        content: Some(vec![Block::text(
            "The person did not answer (the questionnaire was declined or withdrawn). Do not assume answers; continue without them, or ask again later if they are still needed.",
        )]),
        details: Some(form_details(
            json!({ "outcome": "unanswered", "submitted": false }),
        )),
        ..ToolOutput::default()
    }
}

fn form_unavailable(reason: String) -> ToolError {
    ToolError {
        message: format!("Cannot ask the person right now: {reason}"),
        diagnostics: vec![],
        details: Some(form_details(json!({
            "outcome": "unavailable",
            "reason": reason,
        }))),
    }
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
                    },
                    "title": {
                        "type": "string",
                        "description": "With questions: a one-line title for the questionnaire"
                    },
                    "note": {
                        "type": "boolean",
                        "description": "With questions: let the person add a note about the whole questionnaire"
                    },
                    "questions": {
                        "type": "array",
                        "description": "Instead of question: 1 to 4 related questions answered together. Each needs a prompt; options make it a choice (kind single by default, or multiple), none makes it text.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": {"type": "string", "description": "Stable id to recognize the answer by (letters, digits, '_' or '-'; default q1, q2, ...)"},
                                "prompt": {"type": "string", "description": "The question, exactly as the person will read it (at most 2000 characters)"},
                                "kind": {"type": "string", "enum": ["single", "multiple", "text"], "description": "single (one choice), multiple (any number), or text (typed answer)"},
                                "description": {"type": "string", "description": "A short clarification under the prompt (at most 300 characters)"},
                                "preview": {"type": "string", "description": "Markdown the person can examine before answering, e.g. a snippet (at most 4000 characters)"},
                                "options": {
                                    "type": "array",
                                    "description": "Up to 10 choices",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "id": {"type": "string", "description": "Stable id (default o1, o2, ...)"},
                                            "label": {"type": "string", "description": "The choice as shown (unique, at most 200 characters)"},
                                            "description": {"type": "string", "description": "What choosing it means (at most 300 characters)"},
                                            "preview": {"type": "string", "description": "Markdown shown for this choice (at most 4000 characters)"}
                                        },
                                        "required": ["label"]
                                    }
                                },
                                "allow_other": {"type": "boolean", "description": "With options: the person may type their own answer instead"},
                                "note": {"type": "boolean", "description": "The person may attach a note to this answer"},
                                "default": {"type": "string", "description": "Option id to preselect (never an answer until the person submits)"},
                                "placeholder": {"type": "string", "description": "Hint shown in an empty text field"}
                            },
                            "required": ["prompt"]
                        }
                    }
                }
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
            let source = format!("call:{}", ctx.call_id);
            let Some(host) = &ctx.host else {
                return Err(ToolError::new("No execution host is configured"));
            };
            if args.get("questions").is_some() {
                let request = parse_form(&args)?;
                let question =
                    Question::form(request.title.clone(), request.form.clone()).with_source(source);
                let outcome = host.ask(&ctx.call_id, question, ctx.cancel.clone()).await;
                if ctx.cancel.is_cancelled() {
                    return Err(ToolError::new(
                        "The question was withdrawn: the run was stopped",
                    ));
                }
                return match outcome {
                    AskOutcome::Unavailable(reason) => Err(form_unavailable(reason)),
                    AskOutcome::Unanswered => Ok(form_unanswered()),
                    AskOutcome::Answered(Answer::Form { answers, note }) => {
                        Ok(form_answered(&request.form, &answers, note.as_deref()))
                    }
                    // The host answers a questionnaire only as a whole.
                    AskOutcome::Answered(_) => {
                        Err(ToolError::new("The answer did not match the questionnaire"))
                    }
                };
            }
            let request = parse(&args)?;
            let mut options = request.options.clone();
            if request.allow_other {
                options.push(option(OTHER_VALUE, OTHER_LABEL, None));
            }
            let question = presented(&request.question, options).with_source(source.clone());
            let outcome = host.ask(&ctx.call_id, question, ctx.cancel.clone()).await;
            if ctx.cancel.is_cancelled() {
                return Err(ToolError::new(
                    "The question was withdrawn: the run was stopped",
                ));
            }
            match outcome {
                AskOutcome::Unavailable(reason) => Err(unavailable(reason)),
                AskOutcome::Unanswered => Ok(unanswered(false)),
                AskOutcome::Answered(Answer::Form { .. }) => {
                    Err(ToolError::new("The answer did not match the question"))
                }
                AskOutcome::Answered(Answer::Text { value }) => Ok(answered_text(&value)),
                AskOutcome::Answered(Answer::Confirm { value }) => {
                    // The host never pairs a confirm answer with these questions; report
                    // what was submitted rather than inventing a choice.
                    Ok(answered_text(if value { "yes" } else { "no" }))
                }
                AskOutcome::Answered(Answer::Choice { value }) => {
                    if value == OTHER_VALUE && request.allow_other {
                        let follow_up = Question::input(OTHER_FOLLOW_UP_TITLE)
                            .with_message(clip_utf16(&request.question, UI_MAX_MESSAGE_CHARS))
                            .with_source(source);
                        let outcome = host.ask(&ctx.call_id, follow_up, ctx.cancel.clone()).await;
                        if ctx.cancel.is_cancelled() {
                            return Err(ToolError::new(
                                "The question was withdrawn: the run was stopped",
                            ));
                        }
                        return match outcome {
                            AskOutcome::Unavailable(reason) => Err(unavailable(reason)),
                            AskOutcome::Unanswered => Ok(unanswered(true)),
                            AskOutcome::Answered(Answer::Form { .. }) => {
                                Err(ToolError::new("The answer did not match the question"))
                            }
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
