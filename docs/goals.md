# Goals: the engine-owned runner

When the native engine advertises the `goals` capability, it owns a conversation's goal:
the desktop sends intent (`/goal <objective>`, Pause, Resume, Clear) and mirrors the engine's
state. The desktop's own runner stays in place only for the legacy Pi engine and the demo,
which never report an engine goal. Exactly one scheduler exists for any conversation. This is
GOAL-1 of the [native capabilities plan](../llm-docs/native-capabilities-implementation-plan.md)
(§9); GOAL-2 (structured completion with evidence) and GOAL-3 (audit) are not built.

Status: implemented and covered by automated tests (2026-10-12).

## Lifecycle

| State | Meaning |
| --- | --- |
| `active` | The engine schedules continuations |
| `paused` | Not scheduling; the reason says why (Stop, you, a restart, an ambiguous reply, a message you sent, no window attached) |
| `blocked` | The executor reported it needs your input |
| `completed` | The executor reported the goal met; **not independently verified** |
| `failed` | The executor reported it unreachable, or a turn failed |
| `cancelled` | Archived or replaced |

One goal is current per conversation; replacing or archiving moves the old one into history
(`GoalHistory`). Records are stored in the engine (schema 14) with an event log.

## Scheduling rules

Kept from the desktop runner, now enforced in one place:

- The prompt of a turn is the desktop runner's, byte for byte; the executor ends its reply
  with exactly one status marker on the final line. The marker protocol is unchanged in this
  version.
- Scheduling intent (epoch, turn count, request key `goal:<id>:<epoch>`) is committed before
  the turn is sent, so a replay finds its admission and an old goal's turn can never be taken
  for a replacement's.
- A turn continues only on `[PIPKIN_GOAL_CONTINUE]`. `MET` completes, `BLOCKED` blocks,
  `UNREACHABLE` fails, and anything else pauses with "did not give a clear goal status".
  Nothing is ever resent on an unknown outcome.
- A settlement advances the goal only when it is the goal's own latest turn settling. Another
  input settling does not continue it.
- Stop pauses the goal that exists at that instant ("Stop requested."). A goal set afterwards
  is a replacement and is not paused by the old Stop.
- An engine restart pauses every active goal ("Paused after an engine restart; resume to
  continue."). The interrupted turn itself recovers and settles, but nothing continues until
  you resume.
- No continuation is scheduled while no client is attached to the conversation; the goal
  pauses with "no window is attached", which matches the desktop runner's behaviour on app
  close.
- A message you queued takes precedence: when the goal's turn settles with input waiting,
  the goal pauses ("you sent a message; resume to continue the goal"). A goal set while a run
  is active starts when that run settles.

## Controls

`/goal <objective>` sets or replaces the goal (fast path, no confirmation). `/goal pause`,
`/goal resume` and `/goal clear`, the strip's Pause, Resume and archive buttons, and the
palette's *Pause goal* / *Resume goal* / *Clear goal* send `GoalControl` to the engine.
Revise exists in the protocol (`GoalAction::Revise`) and bumps the objective revision; the
desktop does not expose it yet beyond replacing the goal. Resume is refused for completed,
failed or cancelled goals; set a new goal instead.

## Migration of desktop-kept goals

The desktop keeps goal records in its own database for the legacy runner. When a conversation
opens on an engine that owns goals and the engine reports no goal while the desktop holds a
record, the desktop offers the record once (`MigrateGoal`, keyed by the record's content).
The engine imports it **paused** with provenance `migrated:desktop`, keeping the desktop's
pause reason (such as `Goal met.`); it never manufactures a completed goal from that string.
When the engine already has a goal, that goal stands. Once the engine reports a goal, the
desktop retires its own record. Both schedulers never run at once: the desktop runner is
disabled the moment the engine reports goal ownership.

## Boundaries

- Completion is the executor's claim. GOAL-2 adds criteria, evidence and structured updates;
  GOAL-3 adds an audit. The state label says "Reported met".
- No persisted turn or cost limits yet; `turns` is recorded but not bounded.
- A subagent cannot own a goal; context inheritance never carries one.
- Keep-running after the window closes is not offered (the engine is app-owned).
- Legacy Pi conversations keep the desktop runner unchanged.
