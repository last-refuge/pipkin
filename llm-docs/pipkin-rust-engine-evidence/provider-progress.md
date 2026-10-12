# Bounded provider progress

Implemented on top of `e87809f`; protocol **6** / schema **4** unchanged.

## Implementation

The native generation loop no longer hands provider deltas through an unbounded channel.
`StreamCtx` carries an engine-owned `StreamSink`. It feeds a pending batch directly; reaching
16 KiB of UTF-8 text/tool-identity bytes or 128 deltas commits that batch before the producer
can send more. A notification and timer flush small batches 40 ms from the oldest pending
item, including when the provider sends one chunk and then becomes silent. The existing
SQLite transaction path isolates blocking writes with `block_in_place` and publishes only
after commit. No blocking channel waits on a consumer in the provider's own async task.

Large text/thinking deltas split at UTF-8 boundaries. A tool identity is indivisible: one
oversized identity flushes immediately without truncation rather than remaining in a backlog.
The batching target therefore bounds pending accumulation, not all transient allocations or
the total assistant message retained by the provider. Wire/parser/input-unit limits and the
desktop event bus's byte budget are separate concerns. This is not a claim that every byte
allocation in the streaming pipeline is now capped.

A failed checkpoint cancels the producer and leaves the session failed closed. Completion
closes the sink before final replacement; late callbacks cannot mutate a finalized entry.
Weak callback ownership prevents a retained sink from keeping a finished session/runtime alive.
Provider fixture collectors retain an explicit unbounded-channel conversion for collecting
all deltas in assertions; the production generation loop does not use that path.

## Tests

`pg::` = `E/runtime/run/progress.rs`:

- `synchronous_bursts_are_bounded_and_unicode_and_tiny_deltas_remain_lossless`: a large Unicode
  burst commits before the producer returns; 5,000 tiny deltas remain within the count/byte
  batching targets, and durable text is exact.
- `indivisible_tool_identity_is_committed_without_truncation_or_pending_backlog`.
- `closing_or_dropping_progress_rejects_late_provider_callbacks`.
- `checkpoint_failure_cancels_the_producer_and_stops_further_progress`.

Runtime regressions:

- `rt::provider_bursts_checkpoint_before_yield_and_silent_streams_flush_then_stop`: an actual
  adapter emits without awaiting, then stays silent. Burst checkpoints and the small-update
  timer both work; Stop settles Aborted with the received text preserved.
- `rt::stop_interrupts_a_provider_that_keeps_emitting_without_yielding`: a continuously emitting
  adapter has no async yield; Stop reaches it, settles durably within the test's two-second
  limit, and the final assistant retains all text already received. Lookup resolves settlement
  even if live presentation has coalesced.

Existing storage-fault, partial-recovery, provider, worker crash, desktop, and Stop regressions
are also included in workspace verification. Focused logs:
`/tmp/pipkin-progress-{tests,provider,burst-stop}.log`.

## Verification

Required checks passed: **1395 passed, 0 failed, 53 ignored**, Clippy and formatting clean.
Logs: `/tmp/pipkin-progress-final-{workspace,clippy,fmt}.log`. Initial focused/workspace checks
also passed; final verification includes the additional continuous-producer Stop regression
and the no-extra-copy path for deltas that already fit the batch budget.

Release command:
`SKIP_APP=1 TESTS='stream_latency stream_scaling' OUT=fixtures/native-bench/2026-10-11-provider-progress scripts/bench-native.sh`.
Both ignored benchmarks passed. Raw JSON, machine metadata and run log are retained in that
directory. Engine SHA-256:
`9b12197adf512740db732a0bccabe3f76deea440de2f4a38cf576a704bc2b0c4`.
Only the engine was rebuilt; the script's app-size output is an older artifact, not release
package qualification.

Chunk-to-committed-delivery p95 at **0/2/5/20/100 ms** source cadences was respectively
**25.860 / 42.876 / 44.330 / 45.092 / 43.365 ms**, with **zero missing markers**. These meet
the headless 50 ms p95 target on this run; they do not measure display-presentation latency.

All **24 scaling runs** (eight sizes/chunk combinations, three repetitions) retain exactly
the expected final text length. For **1/2/4/8 MiB** with 1 KiB chunks, median CPU was
**0.060/0.120/0.220/0.430 s**, wall time **99.723/187.808/366.697/733.253 ms**, and client
frame ratios **1/1.991/3.976/7.946**. CPU and frame work scale approximately with output size.
At eight times the output, non-client write calls' byte volume and physical writes grew
**8.553× / 8.544×** for 1 KiB chunks. The **64-byte** series (0.25–2 MiB) had CPU **7.667×**
and frame count **7.986×**, but non-client write bytes / physical writes **12.395× / 11.076×**.
That small-chunk write amplification is retained as a performance follow-up; the results do
not establish every linear-work gate or a matched Pi comparison. Client-byte ratios are not
used alone as a speed proof: large final events may require resync/snapshots.

This is Linux/glibc headless evidence with local scripted HTTP. It does not qualify live
providers, frontend/control pressure, indefinitely flat memory, other platforms, installed
GUI behavior or the full streaming release gate.
