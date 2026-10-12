# Subagent recovery after storage failure

2026-10-12, following `2aea0e8`, protocol 12 / engine schema 10 unchanged.
This covers the built-in subagent tool; Pi extension compatibility remains out of scope.

## Confirmed gaps

A new runtime regression starts a real shell command in a child, makes the database
read-only, then attempts to enqueue a parent follow-up. The failed admission cancels the
parent. Its child Stop also cannot commit, so the child fails closed and cancels execution.
However, the parent waited ten seconds for a settlement that the failed database could not
record. The regression failed its five-second unwind bound on the prior implementation.
The old SUB-CANCEL ledger wording was too broad: the child was cancelled through the failed
Stop commit; the confirmed defects were waiting for impossible settlement and recovery.

After storage was restored, reopening the parent previously found its child's existing
request key without resuming its suspended execution. A child that independently failed
storage during streaming also never settled, leaving the parent waiting indefinitely.
Neither condition should turn an unresolved replay-safe call into a final error result.

## Implementation

- Parent replay explicitly reconciles the existing durable child before idempotent admission.
  Its child/task identity and original request are reused. Recovery still waits for the prior
  execution to unwind; a still-failed child keeps the parent failed closed for another reopen.
- A child storage failure propagates to the waiting parent, preserving the parent tool intent
  and request as unsettled. It is not delivered as a completed tool-error result.
- If cancelling a child cannot persist Stop, the parent waits for execution to unwind instead
  of waiting for a settlement notification. It does not label the unconfirmed child aborted.
  The existing bounded cleanup wait remains ten seconds; the tested real shell exits promptly.
- If Stop can commit, its existing durable abort semantics remain unchanged. Restoring
  writable storage alone does not authorize either session to resume.

## Executed regressions

`runtime::fault_tests::a_parent_storage_failure_cancels_child_tools_and_reopens_without_repeating_effects`
executes both a database-wide read-only fault and an admission-only fault. A real child
shell appends `started`, sleeps, and would append `finished`. Parent admission fails, its
request stays open, the rejected follow-up remains unknown, and both executions unwind
within five seconds. After restoring writes, no provider request occurs until explicit
parent reopen. The same single child and parent result are recovered; the file contains
exactly one `started` and no `finished`. Read-only recovery records the unsafe child command
interrupted and never re-executes it. With admission-only failure, child Stop can commit
and the child remains aborted.

`runtime::fault_tests::a_child_storage_failure_keeps_the_parent_call_recoverable`
fails a child's streamed-progress transaction. Both sessions fail closed, the parent admits
no new prompt and publishes no final subagent result. Explicit parent reopen resumes the
same child once; one successful child result reaches the parent.

The first regression initially had a Rust ownership error in its test setup; cloning the
shared Arc corrected that before the baseline failure was reproduced. The corrected
baseline failed, then the fixed regression passed. All six selected storage-failure tests
pass, including both new regressions. Logs are retained locally at
`/tmp/pipkin-child-storage-before2.log` and `/tmp/pipkin-child-storage-final.log`.

This is executed native runtime evidence with SQLite faults, a scripted provider and real
local shell work. It is not an executed Pi differential, GUI storage-fault test, physical
disk exhaustion or power-loss test. Broader release/default-switch gates remain open.

Required checks: **1,643 workspace tests passed, 0 failed, 56 ignored**; Clippy/all-targets
and formatting pass. Workspace log: `/tmp/pipkin-subagent-recovery-workspace.log`;
companion `-clippy.log` and `-fmt.log` identify the other checks. Ledger SUB-CANCEL gains
this evidence; no disposition or qualification count changes. The evidence summary's stale
51 not-started cell was corrected to 50, matching the actual row count and section totals.
