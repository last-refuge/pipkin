# Durable delivery and memory-soak follow-up — 2026-10-11

Implementation on `native-engine`, on top of **63bd9ae** (the committed plugin-profile
import protection / durable-retry batch). Protocol **4**, engine schema **3**. Final release engine SHA-256:
`c1c8f62da82d767c819bae78563b711adf73cb799a83b1106193b000b44b4425`. The user's
pre-existing plan edit and `.claude/worktrees/` were left untouched. Native remains opt-in.
No live credentials, paid provider calls, existing desktop window, or desktop configuration
were used in this batch. All execution evidence below is headless.

## Delivery contract and implementation

- SQLite `event_outbox` records `(cursor, session, revision)` through session insert/revision
  triggers, in the **same transaction** as the authoritative change. Failure rolls back both.
  These compact records invalidate snapshots; they do not duplicate message/image payloads,
  auth challenges, or every progress delta. Admission/settlement/request identities and
  outcomes remain in the permanent request/run ledger and are recovered with `Lookup`.
- The cursor survives worker restarts. The newest **4096** records are retained even without
  clients; `CatchUp {after, limit}` returns at most **256**, with `through`, `head` and an
  explicit `reset` when the cursor is outside retention. Reset means full snapshots/Lookup,
  not permission to skip unknown outcomes or resend a prompt.
- `AckEvents {through}` accepts only monotonically advancing/repeated cursors delivered to
  that connection. Ack state is connection scoped, not a durable named-client registration.
  Reconnecting clients can supply their cursor; the desktop currently starts from zero,
  reopening/reconciling authoritative state, so restarting the desktop requires no saved ack.
- Engine and desktop-reader presentation queues each cap **128 events / 2 MiB serialized
  bodies**. Pressure replaces queued presentation with one `ResyncRequired` at the latest
  sequence. The reader queues one wake-up instead of one adapter message per streamed event.
  Private auth is recovered from `AuthState`, never the durable outbox.
- Replies have **16** reserved/backpressured slots and preserve the already-published event
  prefix while bypassing later progress. Input lanes have **64 normal / 8 control** slots,
  **16 / 4** active dispatch slots; the actual call selects the lane. Stop execution does not
  wait on a slow client draining output. The desktop pipe writer is bounded at 64 frames.
- Publication sequences are per connection and assigned under its delivery lock. Private
  events no longer manufacture gaps for other clients. Session commit/publication is ordered;
  snapshots serialize against commits and recheck their revision. Subagent publishers serialize
  and read revision before the view, avoiding old content labeled with a newer revision.
- The desktop polls catch-up every second, at most one bounded page per pass. A missing final
  event is recovered even when there is no later event to reveal a sequence gap. Resync keeps
  already-paged history and the selected child; pending requests continue to be reconciled.
- Schema 2 → 3 preserves a private `engine.sqlite.v2.bak`. Schema 1 → current also remains
  tested, preserving its v1 backup and existing history/run/request state. Older schemas were
  not retrospectively recording event cursors; clients still bootstrap from snapshots.

This implements state-based durable catch-up. It is not exact replay of all former event
objects, external attachment, serialize-once multi-subscriber fan-out, or global budgeting
of the GPUI/core bus and provider-internal stream queues. Those remain separate work.

## Automated evidence

Required checks passed on the final source:

```text
cargo test --workspace                         1331 passed, 0 failed, 53 ignored
cargo clippy --workspace --all-targets          clean
cargo fmt --all --check                        clean
git diff --check                              clean
```

The new coverage includes:

- `store::tests::outbox_invalidation_is_atomic_with_state_and_rolls_back_on_failure`
- `store::tests::outbox_is_bounded_paged_and_survives_reopen_with_explicit_reset`
- `store::tests::a_v2_database_is_backed_up_before_adding_the_outbox`
- `runtime::tests::catch_up_acknowledgments_are_monotone_and_connection_scoped`
- `runtime::tests::slow_client_coalesces_without_blocking_stop_or_another_client`
- `runtime::tests::concurrent_and_private_publications_have_contiguous_per_client_sequences`
- protocol delivery tests `pressure_coalesces_to_one_marker_with_the_latest_sequence` and
  `one_oversized_event_uses_bounded_recovery`
- engine delivery tests `replies_keep_the_published_prefix_and_bypass_future_progress` and
  `reply_capacity_backpressures_and_a_dropped_reader_releases_waiters`
- **Actual SIGKILL:**
  `crash_points::sigkill_between_state_commit_and_publication_is_recovered_from_the_outbox`
  kills after durable admission but before publication/acknowledgment; after restart catch-up
  finds it, the original key completes once, and the provider receives one request.
- **Desktop adapter/core/storage:**
  `adapters::native::e2e::durable_catch_up_recovers_a_missing_terminal_event_without_later_live_traffic`
  discards every live engine event after Open, yet history/outcome recover and the desktop
  journal closes without a second provider call, including three transient Lookup refusals.
  Failed forced resyncs remain pending so volatile state is also retried.
- Existing history-pagination, reconnect, Stop, subagent-inspector, auth and storage-fault
  tests passed in the full run. The subagent-inspector test was additionally repeated ten
  times successfully; the earlier intermittent timeout's precise cause is not proven.

Logs: `/tmp/pipkin-event-final-tests.log`, `/tmp/pipkin-event-final-clippy.log`,
`/tmp/pipkin-event-final-fmt.log`, `/tmp/pipkin-subagent-ordering-repeat.log`.

Failures retained: the first full run exposed catch-up collapsing paged history (337 app tests
passed / 1 failed); refreshing the loaded range fixed it. The next full run exposed replies
bypassing an already-published storage error notice (565 engine tests passed / 1 failed);
a bounded event prefix before replies fixed that. Earlier compile attempts found the new
receiver type's test fixture and new Event match needed updating; one ignored Result warning
was fixed. Final verification did not suppress or ignore those tests.

## Memory investigation and fix

The original workload is unchanged: **100 warm-up + 1500 measured text turns**, one session,
scripted local HTTP provider, real release worker/SQLite/frame transport; scratch on the build
filesystem. No provider credentials. No external allocator environment override in the main
baseline or final run. Per-process RSS excludes the benchmark parent and desktop.

Raw measurements, including intermediate unsuccessful fixes, are retained:

| Directory under `fixtures/native-bench/` | Variant | Idle RSS after / sampled peak (KiB) | Second-half slope (KiB/turn) |
| --- | --- | --- | --- |
| `2026-10-11-event-baseline` | Original batch at 63bd9ae | 114804 / 116692 | 54.848 |
| `2026-10-11-event-fixed` | Bounded delivery + async trim every 32 runs | 44976 / 115984 | 49.234 |
| `2026-10-11-event-allocator` | Also pin large-allocation threshold, multiple arenas | 31272 / 102792 | 48.133 |
| `2026-10-11-event-final` | Also one arena; still async periodic trim | 27528 / 40300 | 5.860 |
| **`2026-10-11-event-reclaimed`** | **Final: one arena + 128 KiB mmap threshold + trim before each settlement** | **27456 / 39840** | **1.440** |

The directory named `event-final` was an intermediate candidate, not the final implementation.
Its async trimming still allowed post-turn retained memory to track growing transient buffers.
Intermediate scripts include later non-soak measurements after subsequent edits; only their
named soak results above are used to attribute each candidate. Final evidence is in
[`2026-10-11-event-reclaimed`](../../fixtures/native-bench/2026-10-11-event-reclaimed/).

Why this fix: glibc's adaptive large-allocation threshold and separate thread arenas retained
freed, increasingly large context/request buffers. Trimming alone, or threshold adjustment
alone, did not fix the sampled growth. The **private worker only** now sets `M_MMAP_THRESHOLD`
to 128 KiB and `M_ARENA_MAX` to 1 before starting its runtime. After a run's request future and
buffers have dropped, `malloc_trim(0)` runs through `block_in_place` **before** settlement is
published. No transcript/context entries are removed, no store durability is weakened, and no
user environment/system allocator configuration is changed. Embedded hosts do not set mallopt;
on Linux/glibc they do use the run-boundary trim. Other allocators/platforms are unchanged.

Final results:

- **26.8 MiB idle / 38.9 MiB sampled peak**, under the 100 MiB budget. The ignored release soak
  now asserts that budget for both idle and VmHWM, flat idle descriptors, and one process.
  Baseline was **112.1 MiB idle**, exceeding the budget.
- Idle fds **13 → 13**; one engine process, no descendants; 8 threads at idle in the main run.
- Final 500 turns (1100 → 1600): **26956 → 27456 KiB**, +500 KiB. This is not proof of a
  perfectly flat indefinite heap or the absence of every leak. Remaining growth/cache costs
  have not been individually attributed with a heap profiler. The measured budget failure is
  fixed; broader soak/platform qualification stays open.
- Rotation diagnostic (new session every ten turns): **27268 KiB idle**, second-half slope
  **1.609 KiB/turn**, 160 saved sessions, fds 13, one process. Per-session retention is not zero.
- Existing fixed-glibc-environment diagnostic: **27632 KiB idle**, sampled peak **38932 KiB**,
  second-half slope **1.177 KiB/turn**; consistent with the now built-in worker policy.
- Tradeoff: main-soak CPU **21.607 → 22.980 ms/measured turn**; p50 turn time
  **22.311 → 24.896 ms**, p95 **47.139 → 43.745 ms**. These are single local runs with load
  recorded, not a controlled performance regression qualification or a speedup claim.
- Final streaming p95 at five cadences: **16.230, 43.681, 44.766, 44.959, 47.611 ms**;
  **zero missing markers**. Fast deltas can first appear in the committed final entry.
  The raw harness's inherited note says 50 ms; actual `FLUSH_INTERVAL` remains **40 ms**,
  measured from the oldest pending delta. No flush-policy change was made in this batch.
- Stop (20 samples each): streaming Stop→aborted p95 **5.312 ms**; bash sleep Stop→aborted
  p95 **6.725 ms**, process gone p95 **6.797 ms**, no surviving descendants.

Reproduction:

```sh
SKIP_APP=1 TESTS='soak_1500 soak_diagnostic stream_latency stop_latency' \
  OUT=fixtures/native-bench/<new-label> scripts/bench-native.sh
```

The script records machine/build/load data and raw samples. With `SKIP_APP=1`, any existing app
binary in its size report is **not** a newly built/qualified app artifact; only the engine was
rebuilt for these measurements. No package-reduction or installed-app claim follows.

The ledger moves XPORT-OUTBOX and E2E-37 to **measured (part)**: durable invalidation recovery
and the headless memory budget have passing evidence. **0 rows qualified.** Native desktop
memory/latency, long streaming/tool workloads, power loss, external/multi-client endpoint
qualification, and non-glibc platforms remain unverified.
