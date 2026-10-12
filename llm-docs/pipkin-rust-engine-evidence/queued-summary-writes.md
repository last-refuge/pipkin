# Durable queued summary placement

2026-10-11, following `138d879`. Schema **8**, protocol **9**. This implements
Q-WRITES for Pipkin's built-in compaction; it adds no Pi extension compatibility.
Native remains opt-in and broader release qualification is still open.

## Contract and implementation

Manual/background summary classification atomically records usage, a stable passive
write keyed by the compaction operation, and terminal `Submitted` state. When busy,
the summary waits in SQLite instead of changing context mid-turn. Blocking/overflow
summaries still place directly for their owning generation.

All accepted writes place in admission order before selected users at a post-tools
or successful final boundary, or before the next idle input admission. Writes update
the current cut as they place. Only a strictly older cut is stale; equal cuts are
valid. Stale summaries retain their actual usage without replacing context. Placed
and stale records keep their identity but release the stored summary body.

Stop withdraws queued inputs and cancels unfinished foreground compactions. An
already accepted passive write survives Stop and restart, waiting for a later
boundary. An aborted or failed generation does not itself flush passive writes.
Successful settlement and summary placement share one transaction.

A durable `(run, assistant-entry)` marker records each completed tool boundary with
write placement and selected-user consumption. Restart after tool results therefore
cannot skip the boundary or consume a second one-at-a-time steer. Schema-7 upgrades
backfill boundaries already proven by a later user/assistant message in the same run;
they preserve history and create the private schema-7 backup.

Each session admits at most 64 unfinished nonblocking compactions plus queued writes.
The next manual request is refused before provider contact; background admission waits.
Moving a completed operation into the write queue uses its existing slot. Blocking
compaction remains available so queue pressure cannot prevent progress. Recovery
keeps the existing eight-driver bound. These component limits do not prove a global
memory bound; older databases may contain more than 64 accepted operations.

## Reference evidence

Pinned Pi `d2a311097cbcf669e699479587332ae3988a49d0`:

- `harness/compaction.ts:400-436`: nonblocking summaries use stable write submissions.
- `harness/submissions.ts:154-211`: busy admission queues; idle admission places.
- `harness/inbox.ts`: writes precede selected users, equal cuts are accepted, later
  writes may become stale in the same boundary, and abort withdraws users only.
- `harness/generation.ts`: post-tools and successful-answer boundaries; abort/error
  settlement does not itself place writes.

`scripts/capture-inbox-reference.mjs` executes the original inbox functions against
a transaction recorder. It verifies the pinned checkout and records source hashes.
`fixtures/native-queue/inbox-reference.json` captures post-tools/one, final/one,
final/all and Stop cases. Native tests compare summary placement and the three user
selection cases to these outputs. This is a bounded function differential, not an
end-to-end reference scheduler or provider qualification run.

## Measured checks

- Store: ordered/equal/stale heads against the reference capture, idempotent admission,
  reopen, rollback of placement/head/identity, and schema-7 backup/history preservation.
- Runtime: a completed write survives Stop/restart until the next input; writes precede
  an earlier follow-up at final settlement and a steer after real shell-tool completion;
  follow-ups remain outside the active tool run; stale usage is counted once.
- Capacity: 64 accepted summaries survive Stop and all place after the next input;
  the 65th contacts no provider, and capacity becomes available after placement.
- Actual SIGKILL before/after the tool boundary: one summary, one boundary marker,
  one selected steer in the active run, remaining users in later runs, and exactly one
  shell side effect. Recovery requests contain the correct selected input.
- Actual SIGKILL after busy summary submission: the queued write survives, waits for
  final settlement, requires no second summary request, and counts usage exactly once.

Final verification results are recorded in the progress record. Logs:
`/tmp/pipkin-summary-writes-final-{workspace,clippy,fmt}.log`.

Earlier failures are retained: the first un-escalated engine-lib run could not bind
test sockets (143 failures); two old assertions needed the new submitted-result and
strict-staleness semantics. A Stop/restart test incorrectly assumed the input was the
last model message despite positional system messages. The first new crash test
looked for old tool/user entries in the intentionally compacted live window; it now
uses HistoryPage to verify preserved full history. No missing history or failed
runtime behavior was waived. The first reference-capture attempt used a staged
checkout and hit a sandbox subprocess restriction; the successful capture uses the
verified pinned checkout directly without installing dependencies.

## Still open

Full scheduler/reference traces, broader context/history audits, live-provider and
desktop compaction acceptance, sustained workload/resource qualification and release
platform/install gates remain. Generic extension writes/resets are outside scope.
