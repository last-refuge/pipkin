# Executed durable queue comparison

2026-10-12, following `e6951d4` on `native-engine`. Protocol 12/schema 10 unchanged.
Native remains opt-in. Pi extension compatibility is out of scope.

## Reference and reproduced failures

`scripts/capture-queue-runtime-reference.mjs` executes the pinned Pi durable Harness,
scheduler, submissions and inbox with controlled normalized model responses. Source pin and
hashes are recorded in `fixtures/native-queue/runtime-reference.json`. Eleven cases compare
queue order/modes, complete projected provider requests, visible history and each submitted
input's outcome/reason/answer. Cases cover final and post-tools boundaries, one/all delivery,
withdrawal, busy rejection and explicit submission after model failure.

The original native differential run passed the seven ordinary/withdrawal cases but failed
the four error cases: native automatically consumed pending inputs after a model failure.
Its startup recovery did the same, and new idle prompts/enqueues could bypass older waiting
inputs. These behaviors can cause unintended model calls and alter instruction order.

Automatic successor scheduling now requires successful completion, including when reopening
the engine. Explicit new input resumes retained work through the same final-boundary selection
used after ordinary success. Selection and admission occur in one transaction, preserving
original queue order and configured steering/follow-up grouping. A Prompt queued behind older
work keeps its original Prompt fingerprint for duplicate detection while using the existing
follow-up delivery representation. No new schema or wire representation is required.

## Verification scope

- The eleven Pi/native cases pass. Native additionally shuts down/reopens the same SQLite
  profile during all four failed-queue cases and checks that reopening sends nothing.
- A full 64-input queue rejects a new idle Prompt without recording it or sending a request.
  Withdrawal frees capacity. Resubmitting the admitted Prompt returns its original admission;
  changed content or using Enqueue with that key conflicts. All retained inputs run in order,
  once each; another restart and key replay sends nothing.
- A real headless worker is killed with SIGKILL immediately after settlement and before queue
  scheduling. Successful work resumes its pending follow-ups on restart. Failed work remains
  paused until explicit input, then runs in order without duplicating the original request.
- The desktop core/adapter/journal workflow covers a new Prompt grouped with an older retained
  steer after failure/reconnect. Durable reconciliation must settle the new prompt's journal
  even though the grouped run's live settlement event names the older steer.

An intermediate test timeout was a test assumption: `RunSettled` names the run's first input,
not every grouped input. The differential now waits for the new input's own durable result.
No production settlement protocol change was needed for that test correction. An initial
desktop test also incorrectly waited for `Idle` after a model error; the core correctly
reports `Failed`, and the corrected test checks that explicit state before reopening.

The reference corpus was regenerated independently and compared byte-for-byte. It uses
MemoryStorage; native reopen and SIGKILL tests supply the additional SQLite/process evidence.
Normalized model replies do not qualify cloud transports, authentication, timing, accounting,
system prompts, a rendered GUI or sustained desktop use.

## Commands and release status

```sh
node scripts/capture-queue-runtime-reference.mjs PI_CHECKOUT OUTPUT_JSON
cargo test -p pipkin-engine queue_reference_tests
cargo test -p pipkin-engine --test crash_points sigkill_after_settlement
cargo test -p pipkin-app a_prompt_grouped_with_a_retained_steer
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all --check
```

Final required checks: **1,653 passed, 0 failed, 56 ignored**, clean Clippy/all-targets and
formatting. The desktop test uses a newly opened harness after restarting the engine. Logs:
`/tmp/pipkin-queue-{before,after,crash,desktop,reference,workspace,clippy,fmt}.log`.
These comparisons strengthen existing measured rows, so aggregate evidence counts stay
445 measured, 9 observed, 1 unverified and 49 not started, with 0 fully qualified.
Remaining reference/release gates, installed/desktop fault checks, sustained desktop use,
CI/platform and notice review, native-default and Pi retirement remain open.
