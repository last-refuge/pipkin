# Durable compaction runtime

2026-10-11, following journal foundation `74898a4`. Schema **7**, protocol **9**.
This closes the missing runtime implementation in CTX-CHECKPOINT. Native remains opt-in;
this evidence does not qualify the entire context subsystem or the release.

## Implemented behavior

Manual, threshold background, threshold blocking and overflow compactions now admit a
journal operation before executing. Overflow uses a stable run/error-entry key, so a crash
between summary placement and generation classification reuses the completed operation.
The generation loop waits for pending owned compaction even if settings changed at restart,
and stops on compaction storage failure instead of sending another coding request.

Selection saves the summary messages, model key, thinking, affinity, output limit, timeout,
non-secret stream options, tail/cut, token estimate. Recovered attempts
resolve current credentials/network configuration around that selected request; they do not
rebuild it from the current conversation model or context. Credentials, headers, endpoints
and environment remain excluded from the checkpoint. Retry waits use saved absolute time,
and the persisted attempt count prevents restart from replenishing the retry budget.
As in the pinned reference, retry settings are reread when a response is classified; changing
the settings deliberately changes the policy for subsequent decisions, not an already-saved
retry deadline.

Provider responses are saved before classification. Classification checks the operation
revision and commits usage, retry/terminal transition and any summary entry in one store
transaction. Stop/stale responses cannot overwrite a settled operation. A late valid summary
whose cut has already been passed contributes its actual usage but does not replace the head.
A response received but not yet saved when the process dies may require another provider
request; remote exactly-once billing is not claimed.

One recovery scheduler starts with the engine and wakes on explicit reopen/background
admission. It pages identities in batches of 32 and has at most eight recovery futures.
A separate eight-permit compaction pool limits concurrent summary drivers
without consuming the coding generation pool. Live callers and recovery share one execution
claim per operation. The partial index on pending job ids supports ordered recovery paging
without scanning all settled jobs. These are component bounds, not a total-engine heap proof.

Conversation Stop persists foreground intent and cancels active foreground drivers; queued
foreground jobs settle without contacting the provider. Background scope survives Stop.
Engine shutdown leaves unfinished checkpoints recoverable. Explicit reopen resumes after
storage failure. Schema 7 prevents a journal-only schema-6 runtime from ignoring active jobs;
upgrade preserves pending operations and writes the private schema-6 backup.

## Reference audit

Pinned Pi `d2a311097cbcf669e699479587332ae3988a49d0`,
`packages/durable/src/harness/compaction.ts:33-52,135-225`, preserves the selected model,
thinking, stream options, cut/tail and attempt, then stores an absolute retry deadline.
It reads `runtime.settings.retry` when classifying a response. Native follows that live-policy
rule; this supersedes an intermediate implementation that pinned the retry policy too.
Native additionally saves the received response before classification so recovery can avoid
another provider call at that boundary. No executed differential is claimed by this source audit.

The same source audit confirmed the next required gap: `compaction.ts:400-436` routes
manual/background summaries through `admitSubmission`, and `submissions.ts:154-211` queues
writes while busy for placement at the next boundary. Native still appends at classification.
The journal makes this recoverable but does not resolve queued placement (Q-WRITES); this
built-in behavior remains required independently of extension compatibility.

## Measured evidence

Real worker subprocesses and local HTTP stubs (`tests/crash_points.rs`):

- `sigkill_compaction_boundaries_recover_without_open_and_account_once`: SIGKILL after
  admission, selection, response save and classification; startup recovers without OpenSession.
  Four seeded coding turns plus one summary produce exactly 75 tokens, one summary entry,
  and five provider calls. A second restart adds none. Changing session model/settings after
  selection does not change the saved summary input or chosen model.
- `sigkill_compaction_retry_keeps_deadline_and_attempt_count`: SIGKILL after saving an
  error response or classifying it; restart never retries before the saved deadline and
  does not reset the attempt count or gain extra attempts after restart.
- `sigkill_after_overflow_summary_reuses_it_before_retrying_coding`: kill after summary
  placement and before generation handoff; restart completes the coding turn with one
  summary, even when compaction is subsequently disabled.

Runtime regressions (`runtime/compaction_tests.rs`):

- Eighteen pending manual jobs execute at most eight summary requests concurrently. Stop
  prevents the ten waiting jobs from contacting the provider and releases all permits.
- An injected classification write failure preserves the saved response and rolls back
  usage/summary together. OpenSession resumes it without a network request; usage is counted once.
- Stop with no open coding run persists foreground abort, while a background job resumes
  on startup and commits its summary without OpenSession.
- A real threshold-triggered blocking compaction holds the coding request. Shutdown/restart
  resumes the saved summary before coding even when compaction is disabled in the new settings.
- A newer summary wins over a late summary at the same cut; only one head is placed and both
  provider attempts are counted once.
- Retry policy remains live: disabling retries while a retry response is pending prevents
  further attempts, while the already-committed retry still runs.
- Orphaned-child abort reaches foreground compaction even without an open child coding run.
- Existing manual/overflow, request-cancel, detached manual lifetime, foreground Stop,
  background survival, settings override and tool Stop regressions continue to pass.

Store regressions additionally cover schema-6 activation backup, preservation of pending
jobs, secret exclusion, operation identity, revision CAS, and transaction rollback.

## Verification and limits

Final required checks: **1455 passed, 0 failed, 53 ignored**; `cargo test --workspace`,
`cargo clippy --workspace --all-targets` and `cargo fmt --all --check` pass. Logs:
`/tmp/pipkin-compaction-runtime-final-{workspace,clippy,fmt}.log`.
Earlier targeted compilations found an unavailable UUID crate reference (replaced with the
existing UUID utility) and a test calling a private driver (changed to wake the actual
recovery scheduler). No product behavior was relaxed for either compile correction.

No live provider, installed desktop or platform acceptance is inferred from these tests.
An executed Pi/native compaction differential, generic queued-write placement equivalence
(Q-WRITES), context replacement history, and broader release qualification remain open.
