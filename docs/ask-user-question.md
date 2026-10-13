# `ask_user_question`: the model asks the person

The native engine offers root sessions a built-in `ask_user_question` tool. It lets the
model ask the person one question, or a **questionnaire** of up to four, and wait for what
they submit. The question is shown by the same presentation, and settled by the same
`AnswerQuestion` / `CancelQuestion` calls, as a question from a
[native extension](native-extensions.md); nothing is scraped from chat text. ASK-1 (one
question) and ASK-2 (the questionnaire contract, below) of the
[native capabilities plan](../llm-docs/native-capabilities-implementation-plan.md).

Status: implemented and covered by automated tests (2026-10-12). The desktop renders
select, input and questionnaire questions; model-originated questions have not had their own
native walkthrough yet (see [Boundaries](#boundaries)).

## Where it is offered

- Root sessions of the native engine, after the coding tools and `subagent`, in a stable
  order between requests.
- Not offered to subagent children: they have no presentation of their own to ask.
- Not available on the legacy Pi engine or the demo adapter.
- Advertised to clients as the `askUserQuestion` capability in `Welcome`.
- The system prompt is unchanged: like `subagent`, the tool contributes no prompt section;
  its own description carries the guidance.

## Arguments

| Field | Type | Rule |
| --- | --- | --- |
| `question` | string, required | Trimmed, non-empty, at most 2000 characters |
| `options` | array of `{label, description?}` | At most 10; labels trimmed, non-empty, unique, at most 200 characters; descriptions at most 300 |
| `allow_other` | boolean | With options, adds the choice `Other (type an answer)`; ignored without options |

Lengths are counted in UTF-16 units, like the question host's own limits. Bounds are
enforced in the engine before anything is shown. A violation is an error result for the
model (`outcome: invalid`) and no question is presented.

## What the person sees

- With options: a `select` whose values are `o1..on` (labels are presentation text, not
  identifiers), plus `other` when allowed. The question text is the title when it is one
  line of at most 300 characters; otherwise the title is *A question from the assistant*
  and the text is the message.
- Without options: an `input`.
- Choosing *Other* opens a follow-up `input` titled *Your answer* with the question as its
  message.
- The question has no deadline: it waits for the person. Stop withdraws it.

## Result contract (version 1)

The model-visible text states what the person did. `details.askUserQuestion` carries the
structured outcome:

| Outcome | `is_error` | Text | Details |
| --- | --- | --- | --- |
| Answered a choice | no | `The person chose: <label>` | `answer: {kind: "choice", value, label, index}` |
| Answered with text (free text, or after *Other*) | no | `The person answered: <text>` | `answer: {kind: "text", text}` |
| Declined / withdrawn / refused by the host | no | `The person did not answer (…)` | `outcome: "unanswered"`, plus `choseOther: true` when the follow-up was declined |
| No presentation connected | yes | `Cannot ask the person right now: …` | `outcome: "unavailable"`, `reason` |
| Invalid arguments | yes | the rule that failed | `outcome: "invalid"` |
| Stop while open | yes | `Tool ask_user_question was aborted` | — |

Every details object has `version: 1`. A suggested option is never an answer; only what the
person submits is returned. Declining is not a tool failure: the model is told to continue
without assuming an answer.

A host refusal (more than 20 questions already open, or a disposed host) also reads as
`unanswered`; the host posts a notice saying the question was not shown.

## Availability

The call is available when any client is connected to the engine. Question views are
published to every client, and a client that opens or reopens the session receives the open
question in its snapshot, so a question asked while another conversation is selected is
shown when the person returns to it. With no client connected the call fails at once as
`unavailable` rather than waiting for nobody.

## Durability and recovery

- The call is recorded as a durable tool intent like every other call. The submitted answer
  is persisted in the tool result entry (text and details) and is never asked twice.
- The tool is replay-safe: asking has no external effect. After an engine restart, an
  unsettled question is asked again under a new runtime id; the old id is refused with
  *That question is no longer open.* The recovery contract is the same as `subagent`'s:
  the current tool recovers only calls whose recorded contract (name, description,
  parameters, replay policy) equals its own.
- Engine shutdown disposes the question host, which withdraws open questions. That
  withdrawal is **not** recorded as the person's refusal: the call stays unsettled so the
  restarted engine asks again.
- Recovery is per tool. The engine's `recoverable_tool` registry names the tools that may
  recover their own interrupted calls (`subagent`, `ask_user_question`); every other
  interrupted call is recorded as interrupted and never rerun.

## Questionnaires (ASK-2, contract version 2)

`questions` (instead of `question`) asks one to four related questions answered as a whole.
Protocol 25 carries them as a `form` question (`QuestionKind::Form`, `QuestionView::form`,
schema version 1) and takes `Answer::Form`; capability `questionnaires`.

### Arguments

| Field | Rule |
| --- | --- |
| `title` | Optional one line, at most 300 characters; default *A few questions from the assistant* |
| `note` | `true` lets the person add one note about the whole questionnaire |
| `questions[]` | 1 to 4 |
| `questions[].id` | Optional stable id, 1–64 letters, digits, `_` or `-`, unique; default `q1..qn` |
| `questions[].prompt` | Required, trimmed, at most 2000 characters |
| `questions[].kind` | `single` (default with options), `multiple`, or `text` (default without options); `text` cannot offer options, the others must |
| `questions[].description` | At most 300 characters |
| `questions[].preview` | Markdown, at most 4000 characters, shown under the prompt |
| `questions[].options[]` | At most 10; `id` optional (default `o1..`, unique, same rule as question ids), `label` unique and at most 200, `description` at most 300, `preview` Markdown at most 4000 |
| `questions[].allow_other` | With options: the person may type an answer instead |
| `questions[].note` | The person may attach a note to this answer |
| `questions[].default` | An option id (or a list for `multiple`) the presentation preselects; must be one of the options; at most one for `single` |
| `questions[].placeholder` | Hint in an empty text field, at most 200 characters |

Every bound is checked in the engine before anything is shown; a violation is an error
result (`outcome: invalid`, version 2). Ids are what answers name; labels are presentation.

### What the person sees and submits

The desktop shows every question in one dialog: radio rows for `single`, check rows for
`multiple`, a text field for `text` or `allow_other`, a note field where allowed, the
previews as Markdown blocks (headings, paragraphs, lists, code), and one **Answer** button.
Enter toggles the highlighted option; nothing reaches the engine before Answer. Selections
are a draft the core keeps (`FormDraft`, seeded from `default`), texts live in the dialog's
fields; both survive collapsing the dialog, and a draft also survives the engine's re-ask
after a restart because the question carries its asker (`source: call:<id>`) and the ids are
stable. **Decline** settles the questionnaire unanswered.

The engine validates a submission against the questionnaire the person saw: unknown question
or option ids, a question answered twice, more than one choice for `single`, a choice for
`text`, typed text where `allow_other` is off, a note where none is allowed, and texts over
10,000 or notes over 2,000 characters are refused with the question left open. A question
with nothing selected and no text is unanswered; a note alone never answers it; a default
that the person never submitted is not an answer.

### Result contract (version 2)

| Outcome | `is_error` | `details.askUserQuestion` |
| --- | --- | --- |
| Every question answered | no | `outcome: "answered"`, `submitted: true`, `questions[]`, `note` |
| Some answered | no | `outcome: "partial"`; the text ends with *Do not assume answers for the questions left unanswered.* |
| Submitted with none answered | no | `outcome: "unanswered"`, `submitted: true` |
| Declined / withdrawn / refused by the host | no | `outcome: "unanswered"`, `submitted: false` |
| No presentation connected | yes | `outcome: "unavailable"`, `reason` |
| Invalid arguments | yes | `outcome: "invalid"` |

`questions[]` has, per question in order: `id`, `kind`, `answered`, `selected` (`{id,
label}` of each chosen option), `text`, `note`. The model-visible text lists each question
as `- <id> (<prompt>): <labels; other: text> (note: …)` or `not answered`, then the note.

### Attention

A waiting question marks its conversation **Needs your answer** in the conversation list
whether or not it is open (`LifecycleEvent::QuestionWaiting`, raised from every session's
`UiChanged`); the open conversation keeps its waiting strip and dialog. No system notification
is sent.

## Boundaries

- The dialog has no keyboard shortcut to submit: Enter toggles the highlighted option and
  Tab reaches the Answer button. Preview Markdown is drawn as plain blocks (no inline styles,
  links or images).
- Keyboard-only completion, IME input in the text fields, narrow layouts and screen-reader
  interaction for model-originated questions and questionnaires are unverified natively.
- Question durability on the native path is intent-level: the open question itself remains
  runtime-lifetime scoped and is reissued on recovery under a new id; the desktop keeps the
  draft across that re-ask by the asker's call id, but a closed desktop loses it.
- Questionnaires are the model tool's; extension questions stay single.
