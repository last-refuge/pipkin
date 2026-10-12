# Durable partial tool output and Stop recovery

Follow-up to `6253ebb`, 2026-10-11. Protocol 11 and schema 9 remain unchanged.
Pi extensions remain out of scope. These are local engine/runtime tests with actual shell
processes and SIGKILL; they do not establish installed desktop or platform qualification.

## Reproduced losses

The existing command-interruption test checked the interruption warning and absence of a
repeated side effect, but never checked output. Strengthening it to print a prefix and wait
for its published progress exposed the loss: after SIGKILL, neither the recovered result nor
the next model request contained that prefix. Log: `/tmp/pipkin-tool-recovery-before.log`.

The new storage-fault test also exposed an early-reopen race: clearing the session failure
while its canceled execution was still unwinding let that execution settle Aborted. A storage
failure was thereby mistaken for a user Stop. Log: `/tmp/pipkin-tool-checkpoint-faults.log`.
Recovery now preserves the failure while an execution is active and clears it only once
that execution has finished, before reconciling and resuming the durable run.

## Implementation

The tool publisher commits the bounded model-visible output window, dropped-byte count, and
dropped-line count in a session/call-scoped `kv` checkpoint before publishing progress.
Existing byte/rate bounds and progress coalescing remain in force. Checkpoints use the
existing transaction/publication lock and fail-closed storage handling. Failed checkpoints
cancel execution and are never published. This retains the last committed window, not every
byte written by an external process between checkpoints.

Unsafe interrupted calls rebuild their error results from that checkpoint, preserving
partial text and truncation diagnostics. Saving the final result, retained output, tool-call
state, and deleting the transient checkpoint happens in one transaction. Rollback retains
the previous checkpoint. A successful ordinary result continues to use the existing larger
retained-output buffer.

Recovery of a durable Stop now kills the outstanding tool process groups before settlement,
rebuilds aborted results from saved output, and settles the associated tool-call/output rows.
Previously this branch produced empty results and bypassed orphan-process cleanup.

The source contract was compared with pinned Pi
`d2a311097cbcf669e699479587332ae3988a49d0`,
`packages/durable/src/harness/tool.ts::fromSlot`, `truncated`, and `appendToolResult`.
Both preserve committed bounded text, append truncation diagnostics when needed, and append
the interrupted/aborted error. Native built-in progress emits text; final tool details still
use their existing result path. Arbitrary extension progress/details are out of scope.
This is a source audit, not an executed full-harness differential.

## Regression evidence

- Strengthened existing SIGKILL test: published prefix survives into stored history and the
  next provider request; side effect executes once; orphaned process cannot finish later.
- New SIGKILL immediately after checkpoint commit and before progress publication: output
  survives even though no progress event was delivered; the command is not rerun.
- New large-output test, both ordinary SIGKILL and SIGKILL after durable Stop: UTF-8 tail,
  output bounds, dropped-byte counts, truncation/error messages, retained output, and
  checkpoint deletion survive settlement. Stop makes no further provider request; both paths
  leave no tool process alive.
- New runtime storage-fault test: read-only failure during progress preserves the prior
  checkpoint and suppresses failed output. A final-result insertion failure rolls back the
  checkpoint deletion. Immediate reopening recovers each run without replaying the effect.

The complete process-crash suite passed **17 tests**. The new runtime fault test passed after
the reopen fix. Logs: `/tmp/pipkin-tool-recovery-crashes.log` and
`/tmp/pipkin-tool-checkpoint-faults2.log`. The first checkpoint build had a serialization-helper
type error; it was fixed before successful execution. No failed behavior was waived.

Required checks: **1529 passed, 0 failed, 53 ignored**; Clippy and formatting clean.
Logs: `/tmp/pipkin-output-recovery-workspace.log`, `/tmp/pipkin-output-recovery-clippy.log`,
`/tmp/pipkin-output-recovery-fmt.log`. OUT-RECOVERY gains measured
runtime evidence; sustained streaming/tool storage overhead, full reference traces, installed
workflows and native-default/Pi retirement qualification remain open.
