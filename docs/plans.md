# Plans: the engine-owned task list

The native engine keeps one plan per session: a persistent, revisioned list of tasks with
status, dependencies, completion criteria and evidence. The model changes it with
`plan_update` and reads it with `plan_read`; clients use `UpdatePlan` and `Plan`; both go
through the same transactional engine service. This is PLAN-1 and PLAN-2 of the
[native capabilities plan](../llm-docs/native-capabilities-implementation-plan.md) (§8):
the engine service and the desktop's plan view.

Status: engine and desktop implemented and covered by automated tests (2026-10-12). The
native walkthrough of the plan view is still to do (see the scorecard).

## In the desktop

- **Plan strip** above the composer, shown while the plan has live tasks: *N of M tasks
  done*, the open-status counts and the active tasks' titles, with **Show plan**. It is
  labelled as task counts and predicts nothing.
- **Plan inspector tab** (next to Changes and Subagents; also *Show plan* in the palette).
  The tab badge counts open tasks. Open tasks are listed in the engine's order; finished
  tasks are counted and listed newest first under **Show finished tasks (N)**. The list is
  virtualized, so large plans mount only the rows in view.
- **Expand a task** (click, Enter or Space) for its detail, completion criteria, reason,
  evidence and owner. A stale completion says so.
- **Edits** from the expanded task go through the engine's plan service with a fresh request
  key, conditional on the revision the person saw: *Mark completed*, *Skip*, *Cancel task*
  for open work and *Reopen* for finished work, each with a fixed, honest reason
  (`Skipped by the person from the plan view`, and so on). One edit is in flight at a
  time; a refusal (a conflict, an unmet dependency) is shown on the task until dismissed,
  and the engine's published plan is what the view shows afterwards.
- The tab and strip appear only when the engine reported a plan (the `plans` capability
  path). The legacy Pi engine never reports one, so nothing is emulated. The demo's
  *subagents* scenario carries a simulated plan whose tasks are marked as such; edits there
  change only the demo's copy.

Core state lives in `PlanState` (`pipkin-core`): the engine-reported tasks and counts plus the
person's view choices (expanded task, history open, one pending edit, the last error). The
projection is reset when a conversation is reopened, so a reattachment to an engine without
plans cannot show a stale plan.

## Model

| Field | Meaning |
| --- | --- |
| `id` | Stable within the plan; never reused, even after archival |
| `title`, `detail` | What the task is |
| `status` | `pending`, `active`, `blocked`, `completed`, `skipped`, `cancelled` |
| `reason` | Required for `skipped`, `cancelled`, and for reopening finished work |
| `dependsOn` | Task ids this task waits for |
| `parent` | An optional grouping task |
| `criteria` | What must hold for the task to count as done |
| `evidence` | `{kind, reference, note?}` items: tool call ids, commands with exit status, paths, hashes, workspace revisions, labelled observations |
| `owner` | A reference to whatever does the work (a subagent child, a job) |
| `archived` | Separate from status; only finished tasks can be archived |
| `stale` | A completion whose basis changed (a dependency was reopened, or the criteria were revised); cleared by completing again |
| `revision` | The plan revision of the task's last change |

Counts in a view are task counts, never a prediction of time or of the outcome.

## Rules

- A batch of operations is atomic: the first rejected operation rejects the batch and nothing
  changes. The rejection names the operation index and the rule.
- `expectedRevision` makes a batch conditional; a mismatch is a `conflict`, never a lost
  update.
- Dependencies must exist, cannot be the task itself and cannot form a cycle. A task can be
  `active` or `completed` only when every dependency is `completed` or `skipped` (explicitly
  waived). Cancelled work never satisfies a dependent. The rule is checked for the tasks a
  batch touched; a stale completion stands flagged rather than failing the batch that
  reopened its dependency.
- Several tasks may be `active` at once.
- Tasks are created open (`pending` unless `active`/`blocked` is given).
- Archived tasks are read-only but still satisfy dependents and remain inspectable.
- Bounds (protocol `limits`): 500 live tasks, 100 operations per batch, titles 200
  characters, detail 4000, criteria 2000, reasons 1000, 50 dependencies and 20 evidence
  items per task.

## Operations

Operations are tagged by `op` and take camelCase fields; the model may also write the
snake_case spellings (`depends_on`, `add_evidence`, `clear_evidence`). Tasks are named by id
or by the `ref` of a task created earlier in the same batch.

| `op` | Fields |
| --- | --- |
| `create` | `ref?`, `title`, `detail?`, `criteria?`, `dependsOn?`, `parent?`, `status?`, `owner?` |
| `update` | `task`, `title?`, `detail?`, `criteria?`, `dependsOn?` (replaces), `parent?`, `status?`, `reason?`, `owner?`, `addEvidence?`, `clearEvidence?` |
| `reorder` | `task`, `after?` (omit for first) |
| `archive` | `task` (must be finished) |
| `archiveFinished` | archive every finished live task |

An accepted batch returns a receipt: the new revision, created ids by ref (or `#<op index>`)
and every changed id.

## Idempotency and recovery

Every batch is applied under a key and its outcome (receipt or rejection) is recorded in the
same transaction as the change. The same key returns the recorded outcome without applying
the batch again. Keys are namespaced so they cannot collide: `call:<call id>` for the
model's tool calls, `client:<request key>` for clients. `plan_update` is therefore
replay-safe: an interrupted call reruns after a restart and returns its receipt (or applies
the batch if the crash came first), through the same per-tool recovery registry as
`subagent` and `ask_user_question`. `plan_read` is pure.

Plan state lives outside the transcript (`plans`, `plan_tasks`, `plan_events`,
`plan_receipts`; schema 11), so compaction never touches it and it survives restarts.
`plan_events` is an append-only change log that keeps the prior state of reopened work.
Removing a session removes its plan rows.

## What the model sees

- `plan_update` reports the revision, the counts line and one line per created or changed
  task, not the whole list.
- `plan_read` lists every live task on one line (`#3 [active] Title (after #1, #2) [owner]
  [stale]`), or one task in full with its detail, criteria, reason and evidence; archived
  tasks on request.
- While the plan has live tasks, the model context carries a `plan` system section: the
  counts and the open tasks (up to 30), plus a note when completions are stale. It is a
  positional section patched like the others, so the cached prompt resources are unchanged.
  Finished work is counted, not listed.

Tool result details carry `plan: {version: 1, outcome: applied | rejected | read, ...}`.

## Scope

- Each session has its own plan; a subagent child's plan is separate from its parent's.
  Scoped task references between them are not implemented.
- Goals are not linked yet (GOAL-1 adds the reference).
- The desktop offers only status edits with fixed reasons; editing titles, criteria,
  dependencies or free-text reasons from the UI is not implemented. Typed links from a task
  to a subagent or job are shown as the owner reference text only.
- A crash between committing a plan batch and recording the tool result is covered by the
  receipt contract and the recovery registry at the store level; a process-kill fixture for
  that boundary has not been added.
