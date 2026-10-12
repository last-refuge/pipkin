# Dedicated-server allocator initialization

2026-10-12, following `281de0f`, protocol 12 / engine schema 10 / desktop schema 8
unchanged. This follows the [remaining 107.43 MiB desktop server peak](context-memory.md).

## Cause and fix

The original allocator-retention fix lived inside `host::run_stdio`. It sets glibc's
mmap threshold to 128 KiB and maximum arenas to one before starting engine threads.
`endpoint::run`, used by both `pipkin --engine-server` and `pipkin-engine --listen`,
started the runtime directly and missed that initialization. The real desktop soak
attaches to this dedicated socket server, explaining why its memory behavior differed
from the stdio benchmark.

Both process-owning entry points now call the same `configure_process_allocator`
helper before runtime creation. Settings are unchanged from the established stdio
policy. `host::start`, `EngineHandle::start`, and direct `Server::start` remain suitable
for embedding and do not change the embedding process's allocator policy. The helper
is active only on Linux/glibc; no musl/macOS memory qualification is inferred.

No request, tool, storage or context behavior changes in this fix. The preceding
context-copy improvements remain in place. No new runtime dependency or platform
configuration is introduced.

## Allocation diagnostic

A private copy of the earlier synthetic 1,500-turn SQLite database supplied 12 more
prompts across four sessions. Temporary `mallinfo2` probes ran immediately before and
after the existing settlement-time `malloc_trim`. At the last boundary, glibc reported
8,083,296 bytes in use inside arenas, 2,207,824 in mmap allocations, and 135,776,416 free
inside arenas. Thus about 9.82 MiB was allocated while substantial free allocator space
remained; the measured settled RSS was 61,512 KiB. Free arena bytes are address-space
accounting, not a claim that all 129.49 MiB were resident. This is evidence of retention,
not a complete process-memory profile or proof of no leaks.

With the server initialization fixed and instrumentation removed, the same 12-prompt
diagnostic peaks at 48,308 KiB and ends at 27,504 KiB RSS, with all replies correct.
Run-specific scratch paths differ, so byte-identical provider bodies are not claimed.
The temporary probes are not included in the shipped implementation.

## Desktop regression workload

The unchanged four-session, 1,500-turn workload now runs with an explicit peak limit:

```sh
python3 scripts/qualify-native-desktop-soak.py target/release/pipkin \
  /tmp/FRESH-ROOT --engine-peak-limit-kib 102400
```

The new optional limit records a `memoryGate` in the report and makes the runner fail
after guarded Quit and orderly server shutdown if sampled kernel VmHWM exceeds it.
Failed measurements and shutdown evidence are retained. This checks an actual process
resource result rather than merely asserting that an initializer was called. Without
the option, exploratory runs continue to record memory without imposing a budget.

The workload still verifies all 1,500 exact replies, 300 ordered unique real shell
effects, byte-exact durable outputs, zero dropped output, zero unsettled runs/finalized
chunks, bounded outbox rows, and no Node/Pi trap invocation. History is not reset,
compacted or shortened to pass the memory check. Provider usage is synthetic and
compaction is disabled; realistic usage/compaction remains a separate qualification.
The profile is on tmpfs and does not establish disk-throughput performance.

## Measured result and limits

The final release binary SHA-256 is
`5c99dcbfbd4673e3dc5619c868fa6bbc27343f754050fb0dfc8400c0f0154bd2`.
Source identity and raw synthetic-only logs are in
[`2026-10-12-server-allocator`](../../fixtures/native-desktop-soak/2026-10-12-server-allocator/).
The workload finishes in 275.79 seconds. All 1,500 replies, 300 effects and durable
outputs, and 1,800 provider requests verify. Outbox rows remain capped at 4,096;
no unfinished runs, finalized chunks or forbidden runtime invocations remain.
The desktop and dedicated server both exit with status zero.

| Actual desktop workload | Server peak (KiB) | Final server RSS (KiB) |
| --- | ---: | ---: |
| Original desktop soak | 131,140 | 59,212 |
| Context-copy reduction (`281de0f`) | 110,012 | 85,632 |
| Shared dedicated-process allocator policy | **56,168** | **38,648** |

The explicit 102,400 KiB peak assertion passes. Final server RSS is also below
40 MiB. This closes the reproduced Linux/glibc dedicated-server budget failure
for this workload. Last-half server RSS slope is +1.965 KiB/turn; this is not a
claim of indefinite leak freedom. Idle server fds remain 23→23. Desktop final
RSS is 188,160 KiB, peak 196,480 KiB, last-half slope +16.954 KiB/turn, fds 54→54.
The GUI is excluded from the engine budget; its growth and realistic compaction
remain separate qualification items. Slopes use all samples at turn ≥750,
including the final settled sample.

Observed: the owned Wayland window remained mapped, visible and active at every
sample. The final reply, composer and footer rendered; the guarded performance
palette and Quit responded. Existing display scale was unchanged. Screenshots
stay private because compositor transparency includes unrelated desktop content;
only hashes and observations are retained. No new accessibility, IME, cold-boot,
suspend/resume, Xwayland or macOS claim is made.

Required workspace checks pass: **1,656 tests passed, 0 failed, 56 ignored**;
Clippy with all targets and formatting are clean. Python parsing and diff checks
pass. Workspace checks overlapped the beginning of the soak, so duration is not
an idle-machine latency comparison. Protocol/schema versions are unchanged.
