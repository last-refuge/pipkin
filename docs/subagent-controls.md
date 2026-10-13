# Subagent controls and background children

The native engine lets a person **Stop** or **Steer** one child from the Subagents inspector
(SUB-2), and lets a `subagent` call return at once while the child runs in the
**background**, with its completion delivered exactly once (SUB-3). This is §6 of the
[native capabilities plan](../llm-docs/native-capabilities-implementation-plan.md); profiles
and the launch record are in [subagent-profiles.md](subagent-profiles.md), the inspector's
presentation in [subagent-inspector.md](subagent-inspector.md).

Status: implemented and covered by automated tests (2026-10-12). Nothing native was
observed; see [Boundaries](#boundaries). Schema 18, protocol 24, capability
`subagentControls`.

## Stop and Steer (SUB-2)

Both act on one child, named by the number the engine published for the open session
(`StopSubagent { session, child, request }`, `SteerSubagent { session, child, request, text }`).
A number from another session is refused (`notFound`): the authorization is the `(root,
child)` pair on the engine's task row, never UI state. The inspector offers the controls only
for the selected child, and the core sends a command only when its number is the selected
one, the child is active, and no earlier control is still in flight, so a stale selection
cannot reach a sibling.

- **Stop** records the note `Stopped by the person` on the task row first, then issues the
  child session's own Stop. The task's status settles through the child's run: whoever waits
  on the child (its parent's `subagent` call, or the background watcher) records `aborted`
  when the run settles; a child with nothing running is settled at once. The person's Stop is
  durable before anything is cancelled, so a crash between the two leaves a child that is
  stopped on restart, never one that runs on with a note nobody honours. Repeating the request
  changes nothing.
- A waiting parent is told the difference: its tool result reads `Subagent N was stopped
  before it answered (Stopped by the person). Its result will not arrive; do not start it
  again unless asked.` rather than a bare `aborted`. A child that fails on its own keeps the
  `unanswered (…)` wording.
- **Steer** admits `text` into the child under the durable key `steer:<request>`
  (`InputMode::Steer`); the child's run takes it at its next step, as the main conversation's
  steering does. The same request key admits nothing twice. A finished child, or one whose
  Stop is on record, is refused (`invalid`).

## Background children (SUB-3)

`subagent` gains `wait` (default `true`) and `wake` (default `true`, background only):

| Call | Returns | Completion |
| --- | --- | --- |
| `subagent {task}` | the child's answer (the call waits) | inline, as before |
| `subagent {task, wait: false}` | at once: `Subagent #N started in the background …` | the row records the answer; one follow-up message wakes the model (`wake`) |
| `subagent {task, wait: false, wake: false}` | the same receipt | recorded and notified only; nothing interrupts the model |

The receipt's details carry `child` and `conversationId`. The child is a child session like
any other (same profile, model, caps and launch record; listed in the directory; its
activity is pageable) and is not a shell command running a second engine. A replayed call
after a restart finds the same child and keeps the policy it was launched with: the row's
`delivery`, not the arguments, says whether the call waits.

Lookups (replay-safe reads, offered to root sessions alongside `subagent`; never to children):

| Tool | Does |
| --- | --- |
| `subagent_status {child?}` | Lists the session's children (`#N state: task (background[, started by the person]) — note — answer available`) or one of them |
| `subagent_result {child}` | The child's recorded answer in full, or why there is none |
| `subagent_wait {child, timeout}` | Waits up to `timeout` seconds (default 60, at most 600) for the child to finish, then its answer; a child still running is reported as such |

Every result carries `details.subagent` (version 1) with the child's number, status,
delivery and origin.

### Delivery

A background child's completion is a durable delivery, on the same rules as a job's:

- `delivery` on the task row starts as `notify` or `wake` and becomes `notified`, `woken` or
  `suppressed` once delivered. Clients always see the change as `SubagentsChanged`.
- `wake` sends one follow-up to the parent session under the fixed request key
  `subagent:<child>:done` (`Background subagent #N (task) finished.` plus up to 4 KiB of the
  answer; the rest is read with `subagent_result`). The key makes the wake idempotent: a
  restart that finds the row delivered sends nothing; one that finds it pending sends it once.
- If the run that launched the child was stopped by the person, the wake is `suppressed`:
  the result stays on the row and in `subagent_result`, and nothing continues on its own.
- A child the person launched (below) is always `notified`: user-origin launches never
  authorize a model wake.

A detached watcher per background child records the settlement and delivers. On startup the
engine re-attaches a watcher to every background child whose delivery is still pending (its
run is resumed by ordinary session recovery) and finishes deliveries a restart cut off.

### Results and artifacts

A waiting parent receives the child's answer inline up to 16 KiB; a longer answer ends with
"[Answer continues: N of M characters shown; read the rest with subagent_result child C
offset N.]". `subagent_result` reads a bounded slice from a character `offset` (at most
16 KiB per read) or, with `find`, the places where a text occurs with context and no slice.
Every result lists the child's **artifacts** after the answer: the files its settled `write`
and `edit` calls touched (each once, resolved against the child's cwd; a call whose result
was an error is not listed) and the web sources it retained (final URL and title), at most
100. `SubagentResultView` carries them as `artifacts` with `resultChars`, `resultOffset` and
`resultTruncated`; the tool's `details.subagent` repeats them. A retained source is a
reference, not a copy: the parent fetches it again to read it in its own conversation.

### Launching from the inspector

`LaunchSubagent { session, request, task, profile?, context? }` starts a background child on
the person's behalf: origin `user`, delivery `notify`, call id `user:<request>`, so the same
request never launches twice. The directory row says **Started by you · Background**. The
profile and caps resolve as for a model launch; a profile or model that is not available is
refused with the same message. The task is bounded at 64 KiB.

## Protocol

`SubagentRow` gains `origin` (`model` | `user`), `background`, `delivery`, `note` and
`has_result`; `Reply::Subagent(SubagentRow)` answers the three controls and
`Reply::SubagentResult(SubagentResultView)` answers `SubagentResult { session, child }`.
In the desktop core, `Command::StopSubagent(child)`, `Command::SteerSubagent { child, text }`
and `Command::LaunchSubagent(task)` become `BackendRequest`s under a fresh `RequestId`,
answered by `SubagentControlDone` / `SubagentControlFailed` naming that request; a refusal is
shown as the directory's error. The demo adapter refuses every control with its simulated
marker.

## Boundaries

- Nothing native was observed for this change: the controls, the launch row and the notes in
  a window, keyboard reach and both themes are unverified.
- Stop settles through the child's run. A child whose storage has failed cannot settle; the
  note stays on record and explicit recovery finishes it, as for any other stopped run.
- Steering is one message at a time per request; there is no transcript of a steer in the
  parent, only in the child's activity.
- A background child outlives the parent run that started it (like a job). Stopping the
  parent suppresses the wake but does not stop the child; use the inspector to stop it.
- No structured parallel groups (plan §6 leftover). Artifact references name files and
  sources only; jobs a child started are its own and are not referenced.
- The inspector's launch row has no profile or context choice yet (worker, fresh); the call
  accepts both.
