# Final-response ownership and large-output memory

2026-10-12, following `368fdca`. Protocol 12 / schema 9 unchanged. Final release worker
SHA-256: `4cdc34831e3402d05de9c2065b7fa360d99caf239215cd1b7a2797c9064481c4`.

## Implementation

`runtime/run.rs::request` cloned the completed provider message into the transaction and
again into its entry, retaining the original for a return value that its sole caller discarded.
It now moves the message into the existing commit/publication transaction and returns unit.
Only the small usage record and error classification are retained separately. Publication
still happens after commit; final content, stop reasons, provider metadata, entry/run identity,
usage/reset accounting, and crash points are unchanged.

The no-progress/final-only path uses `Store::append_entry_owned` to move its payload through
SQLite serialization into the published entry. Borrowed store callers retain their existing
behavior: the returned copy is allocated only after SQL serialization/binding finishes. A
shared Cow-backed insertion helper supports both without duplicating the SQL. The first
implementation eagerly cloned borrowed inputs; code review corrected that before the final
verification. No allocator tuning, timeout changes, history truncation, weaker fsync policy,
publication-before-commit shortcut or new memory-limit exception was introduced.

## Measurements

Same Linux/glibc machine, release flags, btrfs scratch and local scripted HTTP provider as
[the preceding qualification run](qualification-performance.md). Raw final output:
`fixtures/native-bench/2026-10-12-owned-final-message-final/`. The intermediate ownership
implementation's measurements remain in `fixtures/native-bench/2026-10-12-owned-final-message/`;
only the final directory represents the final borrowed/owned insertion code.

| Measurement | Preceding run | Final implementation |
| --- | --- | --- |
| Worst 10 MiB response VmHWM, 3 repetitions | 118,496 KiB (115.72 MiB) | 98,112 KiB (95.81 MiB) |
| Worst peak including final-history reopen | Not recorded separately | 98,112 KiB (95.81 MiB) |
| Final output lengths | 27/27 exact | 27/27 exact, now asserted in the benchmark |
| Chunk-to-committed-client-update p95 | ≤45.767 ms | ≤45.851 ms; zero missing markers |
| 100 warmup + 1500-turn soak idle / peak | 30,112 / 41,588 KiB | 30,292 / 41,684 KiB |
| Soak idle fds / worker processes | 13→13 / 1 | 13→13 / 1 |
| Soak second-half RSS slope | +1.726 KiB/turn | +1.694 KiB/turn |

The defined large-response peak drops **20,384 KiB (19.91 MiB)**, bringing all measured
cases, including the reopened saved response, below 100 MiB. This is consistent with removing
the two full-response copies found in the code. It is not a whole-program allocation profile
or proof that every transient allocation is removed. Soak request bytes are identical at
466,676,683. Positive residual RSS growth is still recorded, not declared an absent leak.

The final release run repeats stream_scaling (three repetitions of nine sizes/chunk cases,
through 10 MiB), stream_latency (five cadences), and the unmodified 100+1500-turn text soak.
All three benchmarks passed. The harness now asserts zero lost latency markers, exact stored
output lengths, and zero leftover finalized chunks, and records memory after final-history
hydration. CPU/write scaling remains approximately linear over these measured sizes; the
1→10 MiB median CPU ratio is 9.167×, write-syscall bytes excluding client bytes 12.217×.
No desktop throughput improvement or universal scaling guarantee is claimed.

Command:

```sh
OUT=fixtures/native-bench/2026-10-12-owned-final-message-final SKIP_APP=1 \
TESTS='stream_scaling stream_latency soak_1500' scripts/bench-native.sh
```

Required workspace checks: **1634 passed, 0 failed, 53 ignored**, clean Clippy
(all targets) and formatting. Logs: `/tmp/pipkin-owned-final-workspace.log`,
`/tmp/pipkin-owned-final-clippy.log`, `/tmp/pipkin-owned-final-fmt.log`;
benchmark logs `/tmp/pipkin-owned-final-bench{,-final}.log`.

This closes the observed peak-memory miss for the defined isolated 10 MiB reply on this
machine. Sustained large replies with growing context, long tool-output/multi-session/desktop
workloads, non-glibc/platform behavior, fresh Pi comparisons and full release qualification
remain open. Native stays opt-in; no ledger row is fully qualified by this measurement alone.
