# ADR 0004: Stop and queue semantics

**Status:** Accepted (2026-10-11). Admission modes, queue, withdrawal, durable Stop intent,
aborted settlement and input retirement are implemented and unit-tested against the scripted
provider. The differential comparison of the **next provider request** against the reference
after Stop, steering and follow-ups has **not been run**, and `steeringMode`/`followUpMode`
`all` delivery is not covered by a test yet.

## Context

Reference semantics (audit `engine-tools-durable-usage.md` §3.5–3.6, `service-api.md` §4.4
`pi.agent-controller`):

- `prompt` = submit with `whenBusy:"reject"` → `busy` while a run is active; `steer` and
  `followUp` start a run when idle, otherwise queue (`agent-controller-provider.ts:37-57`).
- Boundaries (`durable/src/harness/inbox.ts:284-325`): `postTools` places writes plus the first
  steer (`steeringMode` `one-at-a-time`, default) or all steers; `final` places writes, steers
  and the first/all follow-ups (`followUpMode`); placed steers join the running run; placed
  follow-ups start the next run.
- Withdrawal (`cancelQueued`): queued → `cancelled`; placed → `already_consumed`; unknown →
  `not_found` (`submissions.ts:102-115`).
- Stop (`abort`, `scheduler.ts:321+`): one commit withdraws every queued **input** (queued
  writes stay), marks abort on the run's non-background tasks (generation, tools, owned
  subagent children, manual compaction; background compactions survive), signals running
  work, converts a committed streaming partial into an `aborted` assistant entry (excluded from
  future context, usage recorded), appends `Tool <name> was aborted` results for unstarted
  calls, and settles run inputs `unanswered/aborted`.
- **Retirement** (`durable/src/session/transaction.ts:822-845`): the same commit that settles a
  placed input `unanswered/aborted` appends a `pi.cancelled-input` entry with
  `edits:[{target:<pi.user entry>, action:"omit"}]`; completed tool calls/results stay in
  context ("Stop retires the instruction, not the audit trail").
- Desktop contract: `Stopping` persists until engine confirmation; a timeout is not success;
  no continuation after a user Stop (plan §4 "Stop and reopen"; e2e
  `stopping_a_prompt_flushes_its_instruction_before_a_new_prompt`, `..._replacement_goal`).

## Decision

1. **Same queue boundaries and outcomes.** `Call::Prompt` refuses with `Busy` while a run is
   open and records nothing; `Call::Enqueue{mode: steer|followUp}` queues (or starts a run when
   idle). Steers are delivered at the post-tools boundary and follow-ups after completion,
   honoring `steeringMode`/`followUpMode` (`config.rs::QueueDelivery`). `CancelQueued` returns
   `Cancelled | AlreadyConsumed | NotFound`. Queue length is bounded (`MAX_QUEUE` = 64).
2. **Stop is durable intent first.** `Call::Stop` (control lane) commits
   `runs.state='stopping'` + `stop_requested_at` and withdraws queued inputs in one
   transaction, replies `Stopping{run}`, then cancels the provider exchange, tool work and
   pending retry. Settlement is a separate committed `RunSettled{outcome: Aborted}` event. A
   run that completed before the Stop was observed stays `Completed`.
3. **Retirement uses a flag, not omit-edit entries.** On `Aborted` settlement the same
   transaction sets `entries.retired = 1` on every user entry of the run (the prompt and placed
   steers), finalizes any partial assistant as `StopReason::Aborted`, and appends aborted
   results for unstarted calls of the last tool-use message (`runtime/session.rs` settle path;
   `store::retire_entries`). Context derivation skips retired entries
   (`runtime/glue.rs`) and aborted/error assistant messages; history paging still shows them.
   This is an intentional representation change (I-03), not a behavior change: the
   observable contract — instruction excluded from the next request, audit trail and completed
   tool effects retained — must be proven by the differential context test before the row
   passes.
4. **Import mapping:** a reference `pi.cancelled-input` entry's `omit` edit maps to
   `retired = 1` on its target user entry; any other `omit`/`replace` edit (system baseline
   omissions, app writes) maps to the native context-edit representation owned by
   `E/context` and the importer. Until that representation exists, conversations containing
   unrecognized edits are blocked from conversion (plan §6 step 5).
5. **Durable Stop across restart:** an engine killed after the Stop intent commit settles the
   run `Aborted` on reopen instead of resuming it.

## Consequences

- Fewer synthetic entries in history; the desktop no longer needs to hide `pi.cancelled-input`
  rows.
- `retired` is a single bit per entry; it cannot express `replace` edits. If an audited
  workflow needs `replace` (none of the built-ins produce it, audit §3.10), add an explicit
  edit table rather than overloading the flag.
- Background compaction survival across Stop is not yet implementable (compaction is
  `Unsupported` in the native runtime) and stays an open row.

## References

- Plan §4 "Stop and reopen", P06/P07/P08.
- Audit `engine-tools-durable-usage.md` §3.5, §3.6; `service-api.md` §4.4.
- Native: `crates/pipkin-engine/src/runtime/{session,run,glue}.rs`,
  `crates/pipkin-engine/src/store/mod.rs` (`request_stop`, `settle_run`, `retire_entries`,
  `cancel_queued`).
- Tests (pass, 2026-10-11): `runtime/tests.rs` —
  `stop_settles_aborted_and_retires_the_instruction_from_the_next_context`,
  `stop_withdraws_queued_inputs_and_follow_ups_run_after_completion`,
  `queued_inputs_can_be_withdrawn_before_they_run`,
  `a_busy_session_refuses_a_second_prompt_without_recording_it`,
  `a_durable_stop_survives_a_restart_and_settles_aborted`; `store/tests.rs` —
  `stop_intent_is_durable_and_settlement_closes_the_run`,
  `queue_entries_are_ordered_withdrawable_and_bounded`.
