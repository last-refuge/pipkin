# Stop and compaction lifecycle

Implemented after `7c27693` on 2026-10-11. Protocol 9 / schema 5 unchanged.

## Reference and fix

Pinned Pi `d2a311097cbcf669e699479587332ae3988a49d0`:
`packages/durable/src/harness/harness.ts:104-108,145-147` creates a manual compaction
task and delegates conversation abort to the scheduler. `harness/scheduler.ts:321-342`
marks ordinary foreground tasks for abort without crossing background boundaries.
`harness/compaction.ts` checks abort before committing the summary.

Native Stop previously signalled only the active coding run. A manual compaction uses
no coding run, so Stop returned successfully while that summary continued. Manual
compactions now register independent child cancellation tokens in the session. Stop
signals every registered foreground compaction, including when there is no active
coding run. Registration and Stop share the session lock; a new manual operation after
Stop remains usable. A drop guard removes exactly its registration on every exit and
retains session demand until cleanup. Storage failure cancels registered manual work too.

Summary commit now takes the same session lock and checks cancellation again. A summary
that commits before Stop remains completed; a summary whose Stop wins cannot move the
context boundary, even if the provider returns a late successful response. Background
threshold compactions keep independent cancellation scope and survive a foreground Stop.

## Measured behavior

- Two simultaneous manual compactions with no active coding run both abort on Stop;
  their registrations disappear, context is unchanged, and a subsequent manual compact
  succeeds. Existing generic RPC-cancellation tests also rerun.
- A threshold-triggered background summary remains pending through Stop, then commits
  after the stopped coding turn settles. Its summary source and next provider request
  exclude the stopped instruction.
- Stop after a completed coding run is a no-op across repeated calls: the answer,
  completed outcome and instruction stay in the next request context.
- Stop during the first real bash tool of a sequential round produces aborted results
  for both the running command and the unstarted write. The write never touches the file.

The first workspace run failed the new sequential-tool test because it incorrectly
assumed built-in bash/write were sequential; they are parallel in both engines. The test
now wraps the real bash implementation in an explicit sequential declaration. Production
tool scheduling is unchanged. Failed log: `/tmp/pipkin-stop-compaction-workspace.log`.
Final workspace checks: **1439 passed, 0 failed, 53 ignored**, Clippy and formatting
clean. Logs: `/tmp/pipkin-stop-compaction-final-{workspace,clippy,fmt}.log`.
These are scripted-provider/runtime tests, with a real bash process in the sequential
tool case. No live-provider, GUI, or installed-workflow qualification is inferred.

## Remaining compaction requirement

The source audit also confirmed persistent compaction checkpoints: select, summarize
with fixed model/thinking/options/tail/cut, and retry with an absolute deadline. Native
compaction still keeps those values in its future. Completed summaries are durable,
but a manual/background operation in flight is not recovered as the same operation
on restart; a blocking compaction can be recomputed by the resumed coding run. This is
an implementation gap, not an equivalent recovery claim. `CTX-CHECKPOINT` records the
required work explicitly. Summary retry/usage and placement qualification also remain
open. None of this is Pi extension compatibility; it is built-in context management.
