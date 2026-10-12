# Native engine performance qualification follow-up

Follow-up: [final-response ownership](owned-final-message.md) reduces the measured 10 MiB
peak below 100 MiB. The original measurements and failure below remain historical evidence.

2026-10-12. Engine source `4a9d4eb0e3ec92dce50d02ab160547c4e0e50172`, protocol 12 / schema 9.
Binary SHA-256 `8cc567be7256ab7b19620fd8d7574f4240f0fc2d65b00f5851b99c2c52964c70`.
Engine source is committed; benchmark repairs, this evidence and owner documentation changes
account for the dirty worktree. This is a headless Linux/glibc engine run, not an installed
release-candidate or desktop qualification. No Pi/Node process or live provider is used.

The release worker is exercised over stdio with a local HTTP scripted provider, WAL/FULL
persistence and btrfs scratch profiles. Machine: Core Ultra X7 358H, 16 logical CPUs, powersave
governor, kernel 7.2.5-3-omarchy, Rust 1.99.0. Full machine/load metadata and raw measurements:
`fixtures/native-bench/2026-10-12-qualification-final/`.

Command:

```sh
OUT=fixtures/native-bench/2026-10-12-qualification-final SKIP_APP=1 \
TESTS='readiness soak_1500 stream_latency stream_scaling history stop_latency' scripts/bench-native.sh
```

## Measured results

| Measurement | Result | Scope |
| --- | --- | --- |
| Fresh-profile prompt readiness, p95 | 33.480 ms | 20 measured starts, 2 warmups; warm filesystem/cache, not cold boot |
| Existing-profile open, p95 | 24.068 ms | Same 20 starts |
| Idle worker RSS, p95 | 23,500 KiB (22.95 MiB) | ≤40 MiB idle budget |
| 100 warmup + 1500 measured turns | All completed | Original single-session, text-only workload |
| Soak idle / peak RSS | 30,112 / 41,588 KiB (29.41 / 40.61 MiB) | Both below 100 MiB; peak is VmHWM |
| Soak idle descriptors / process count | 13 → 13 / one worker | No tool subprocesses in this workload |
| Soak second-half RSS slope | +1.726 KiB/turn | Positive residual growth retained; not proof of indefinitely flat memory |
| Soak turn p50 / p95 | 26.013 / 39.390 ms | Scripted provider, not model inference performance |
| Chunk-to-committed-client-update p95 | 28.206–45.767 ms | Five cadences (0/2/5/20/100 ms), zero missing markers; ≤50 ms |
| 100,000-entry warm initial page, p95 | 0.943 ms | 200 entries; first open 2.476 ms; full 200-page traversal checked |
| Stop to settled aborted, p95 | 6.065 ms streaming / 6.264 ms bash | 20 iterations each; not merely command acknowledgment |
| Stop to tool process gone, p95 | 6.282 ms | Zero descendants after each Stop |

Streaming scaling now includes the planned **10 MiB** endpoint, alongside 1/2/4/8 MiB
at 1024-byte chunks and 0.25/0.5/1/2 MiB at 64-byte chunks. Three repetitions per case:
**all 27 final output lengths match exactly**, and no entry chunks remain after orderly
exit. These invariants were checked in the generated JSON after the run. Across 1→10 MiB,
median wall time grows 9.145× and CPU 8.143× for 10× text; measured write syscalls excluding
client bytes grow 12.218×. These bounded observations are consistent with roughly linear
work, not universal proof for every provider/client workload. Client bytes have a discontinuity
because large final events exceed the delivery body budget and use resync; this harness
is not the desktop's full repeated-resync workload.

**Large-response memory remains open:** the 10 MiB reply peaks at **118,496 KiB
(115.72 MiB)**, above the 100 MiB reference budget. The text soak's pass must not be used
to hide that expanded-workload result. Full desktop, long tool-output and multi-session
soaks, unexplained residual growth analysis, and matched contemporary Pi comparisons remain
required. No 80% memory-reduction claim or package-size improvement is qualified by this run.

Engine-only size output now includes only the binary actually built, with its hash; previously
SKIP_APP=1 could report an older desktop artifact. Native worker: 110,348,096 bytes with debug
info, 18,586,544 stripped; 5,821,265 bytes zstd-19 of the stripped worker. These are **worker
binary measurements**, not desktop/package sizes. Full package targets remain open.

## Failed run and harness correction

The first run is retained at `fixtures/native-bench/2026-10-12-qualification/`. Readiness
failed its old assumption of exactly two history entries after restart. A diagnostic rerun
(`/tmp/pipkin-readiness-diagnostic.log`) identified the legitimate persisted system-context
snapshot between the user and assistant entries. The repaired check verifies exactly one
`ping` user entry with request key k1, one system snapshot, and one settled `ready` assistant
entry; it does not simply permit arbitrary extra entries. The entire six-benchmark sequence
then passed. `/tmp/pipkin-native-qualification-bench{,-final}.log` retains both runs.

Required workspace verification after the harness change: **1634 passed, 0 failed,
53 ignored**, clean Clippy/all-targets and formatting. Logs:
`/tmp/pipkin-qualification-workspace.log`, `/tmp/pipkin-qualification-clippy.log`,
`/tmp/pipkin-qualification-fmt.log`. `bash -n scripts/bench-native.sh` also passed.
Visual/installed/platform acceptance, uncached startup, complete archive/restore, relative
Pi performance, and native-default/Pi-removal gates remain open. No ledger row is declared
fully qualified from this partial performance evidence.
