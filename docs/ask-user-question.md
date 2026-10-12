# `ask_user_question`: the model asks the person

The native engine offers root sessions a built-in `ask_user_question` tool. It lets the
model ask the person one question and wait for what they submit. The question is shown by
the same presentation, and settled by the same `AnswerQuestion` / `CancelQuestion` calls,
as a question from a [native extension](native-extensions.md); nothing is scraped from chat
text. This is the first slice (ASK-1) of the
[native capabilities plan](../llm-docs/native-capabilities-implementation-plan.md).

Status: implemented and covered by automated engine tests (2026-10-12). The desktop already
renders select and input questions; model-originated questions have not had their own
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

## Boundaries

- One question per call, single choice or free text. Grouped questionnaires, multiple
  selection, previews and per-answer notes are ASK-2 in the plan and need a protocol
  extension.
- The desktop shows the question in the open conversation's dialog. A background
  conversation's question does not yet raise an attention badge or notification; it appears
  when that conversation is reopened.
- The dialog's accessible name is now `Question: <title>` (it no longer claims an extension
  asked). Keyboard-only completion, IME input in the follow-up text field, narrow layouts
  and screen-reader interaction for model-originated questions are unverified beyond the
  existing select/input coverage.
- Question durability on the native path is intent-level: the open question itself remains
  runtime-lifetime scoped and is reissued on recovery rather than restored.
