# Status line and context usage

The composer toolbar carries a compact status line for the open conversation, and the
engine reports how full the model context is. This is STATUS-1 and STATUS-2 of the
[native capabilities plan](../llm-docs/native-capabilities-implementation-plan.md) (§11).

Status: implemented and covered by automated tests (2026-10-12). Nothing native was
observed; see [Boundaries](#boundaries).

## The status line (STATUS-1)

Derived from core state on each render, never from mounted rows:

| Segment | Shown when | Source |
| --- | --- | --- |
| Phase (`Working`, `Stopping`, `Waiting for you`, `Outcome unknown`, `Failed`) | not idle | the run state; a question from the model or an extension means waiting |
| `N queued` | the engine's queue or unconfirmed sends are non-empty | queue state |
| `N questions` | questions are open | the question host |
| `N jobs` | background jobs are active | the engine's job list |
| `N agents` | subagents are pending, running, waiting or completing | the engine's directory |
| Context (`41.2k / 200k · 20%`) | the engine reported a known figure | STATUS-2 below |
| Session usage (`120k tok · $0.42`) | the engine reported committed totals | the usage ledger; missing cost stays unreported, never zero |

The model and effort stay on the existing model button. Empty segments are omitted. At
narrow widths (under about 760 px) the job, agent and session-usage chips collapse; the
phase, queue, questions and context remain. The line is one status region whose accessible
name is a full sentence in reading order ("Working, model M, 2 queued, context 20 percent
used (reported), 120000 session tokens"); nothing is announced on its own. The context and
usage chips are buttons that open the Session usage dialog, which gains a "Context for the
next request" section.

## Context usage (STATUS-2)

The engine publishes `ContextUsageView` (protocol 23, capability `contextUsage`) in every
snapshot and as `ContextChanged` after a run settles, after a compaction and after a model
switch:

| Field | Meaning |
| --- | --- |
| `tokens`, `source` | What the next request would carry: `reported` when the newest assistant usage in the live context measured everything and nothing has been added since; `estimated` when something was added after it (the reported base plus estimates) or nothing has been measured (estimates plus the cached system prompt); `unavailable` before any content |
| `capacity` | The selected model's context window from the catalog, when it says |
| `reserve`, `remaining` | The compaction `reserveTokens` setting when compaction is on, and `capacity − reserve − tokens` |
| `measuredEntry`, `measuredModel`, `stale` | Which response the figure rests on; stale when that response came from a different model than the one selected now |
| `compaction` | `idle`, `background` (a summary is being prepared alongside work), `blocking`, or `off` |
| `revision` | The session revision the figure was computed at |

The computation is the compaction path's own estimator (`estimate_context`): a reported
usage already includes the system prompt, so the prompt is added only to an unmeasured
estimate. Percentages are computed by the desktop from `tokens` and `capacity` only; the
session's cumulative token totals are never turned into a context percentage.

## Boundaries

- **STATUS-3 (Git summary) is not built.** The Changes inspector already shows the scan;
  the line does not repeat it.
- **Subscription quotas** are not shown; reported cost is the provider's API-equivalent
  figure, not an invoice.
- **Nothing native was verified.** The line renders in GPUI; the narrow layout, large text
  and keyboard pass through it have not been driven in a window.
