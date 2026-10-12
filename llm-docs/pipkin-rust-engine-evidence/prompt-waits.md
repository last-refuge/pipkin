# Durable input waits — 2026-10-11

## Identity and scope

Base `67ba56f`, branch `native-engine`; protocol **8**, schema **5** unchanged.
Real native engine/store/protocol tests use scripted providers on Linux. One process-extension
case runs POSIX sh. No GUI or live provider was exercised. TypeScript package loading and
full Pi retirement qualification remain open; the owner subsequently excluded TypeScript
package loading from release requirements.

`WaitForPrompt { session, request }` returns the durable final answer of a prompt, steer or
follow-up, or an unanswered reason. Native `Admission.operation_id` / process `operationId`
exposes the request key as an opaque handle even when the extension did not supply a key.
Rust hosts call `wait_for_prompt`; process hosts call `waitForPrompt`. Text joins the final
answer's text blocks only. Imported keyed inputs resolve their original preserved Pi
submission's answer/reason. Unknown keys refuse; a wait never admits a replacement input.

## Correctness and bounds

- Waiters register their notification before reading committed state, preventing lost wakeups.
  Reads fetch only request status/run/outcome columns while pending, excluding input images
  and text. Completed native results read the indexed run's last final assistant entry;
  imported results read the preserved submission/answer identity. Later runs cannot change
  which answer is returned.
- Engine-wide pending waits are capped at 64. Each connection also holds at most 64 wait tasks
  through reply delivery. Excess waits return Busy. Normal/control capacity remains separate.
  The reply-pressure test admits at most 160 unread normal-lane wait calls (64 queued calls,
  16 normal tasks, 64 wait/reply tasks and 16 buffered replies), then verifies disconnect
  drains the tasks. This is a task-count bound, not a total heap-byte qualification.
- Dropping a Rust wait, cancelling a process wait request, or disconnecting its client releases
  the wait without stopping work. Engine shutdown ends waits. Storage faults return an error
  without inventing a terminal outcome; the input remains recoverable.
- Startup now enumerates queued sessions as well as open runs. This closes a crash boundary
  between committing a run's settlement and scheduling the next queued input. A test
  reconstructs that committed boundary in SQLite with the engine stopped, verifies startup
  makes progress before any client opens/waits, then restarts again to verify no duplicate.
  This case is deterministic state reconstruction, not an actual SIGKILL measurement.

## Executed reference comparison

Pinned revision `d2a311097cbcf669e699479587332ae3988a49d0`, service source
`packages/coding-agent/src/experimental/services/agent-controller-provider.ts`, SHA-256
`773495ee99cb24511f536d5db61437679323299a5bdd01684815cf8c49c0362c`.

```sh
node scripts/capture-prompt-wait-reference.mjs "$PI_SOURCE" fixtures/pi-prompt-waits/results.json
```

Captured with Node v26.8.1 against the available pinned-source `ref-run/pi` tree. The script
checks the source hash and invokes the original controller with controlled durable submission
and entry records. Eleven observations cover text-only concatenation, missing/non-assistant
answers, first-model-message selection, aborted/model_error/no_model/reset/stale, and unknown
or invalid reference IDs. Native protocol tests compare every result/error. Separate rich
import assertions check original historical keyed answers and reasons after real import.
This is an executed service-result projection comparison; it does not run Pi's scheduler.

Native IDs are opaque request keys, not Pi's positive numeric submission IDs. This API exposes
input waits; historical Pi write/compaction and unkeyed submission handles are still not
available. Native compaction currently completes synchronously without a durable handle.
The owner subsequently removed Pi extension support from scope. SVC-AGENT-WAIT is now OS;
this optional compatibility gap no longer blocks release. Core history/context behavior remains required.

## Tests and discovered failure

Twelve `runtime/wait/tests.rs` tests cover final answers through tool/thinking rounds, later
turns/restart, queued withdrawal/Stop, model/no-model failures, foreign/unknown keys, 64-wait
pressure, unread reply pressure, storage-fault recovery, 32 immediate-settlement races,
reference projection, steer/follow-up mapping, Rust cancellation/shutdown, process cancellation,
and startup of queued work with no open run (some cases share one test).

The first full workspace run failed an existing endpoint ownership test:
`single_owner_reclaims_only_stale_sockets_and_does_not_delete_a_replacement` returned
`endpoint already has an owner` after an earlier bind failure. The failed bind held its flock
in an ordinary File before constructing Endpoint; a concurrently forked process could retain
that open-file description until exec. EndpointLock now explicitly unlocks on every return
path after successful lock acquisition, including errors before Endpoint construction. A new
regression keeps a cloned descriptor alive across that error scope and verifies reacquisition
and exclusion of a second live owner. The existing successful-owner test is retained.
Original failing log: `/tmp/pipkin-wait-workspace.log`.

Final required checks all completed successfully:

```text
cargo test --workspace                 1430 passed; 0 failed; 53 ignored
cargo clippy --workspace --all-targets  clean
cargo fmt --all --check                clean
```

Final logs: `/tmp/pipkin-wait-{workspace,clippy,fmt}-complete.log`.
The intermediate successful `-final` and `-verified` logs precede the last changes and are
retained alongside the original failure; only `-complete` verifies the final code.
