# Context ownership and sustained desktop memory

2026-10-12, following `23df29c`; protocol 12 / engine schema 10 / desktop schema 8
unchanged. This follows the [128.07 MiB desktop server peak](desktop-soak.md).

## Confirmed allocation causes and changes

The token estimator previously made three full content copies: per-entry contributions,
a flattened message list, and the tool-ordered list. It now borrows ordinary entry
messages, flattens references, and estimates directly from ordered slots. Derived
compaction summaries and legacy system payloads still allocate their necessary values.
The same exclusion, head-marker, most-recent-usage and missing-result rules are retained.

Ordinary context derivation now moves its owned messages through tool-result ordering.
Repeated tool-call IDs within one assistant still receive repeated copies of the first
matching result; those extra occurrences are copied explicitly, with the last moved.
Unmatched and later-round results keep their prior behavior. No history is truncated.

Common HTTP request sending stores serialized JSON in `bytes::Bytes`. Request attempts
share those immutable bytes instead of cloning the complete Vec for every attempt.
A real local HTTP 429 → success test verifies the two >1 MiB Unicode request bodies
are byte-identical; retry policy, headers, cancellation and timeouts are unchanged.

Finally, runtime preparation now returns raw context entries and a transport/options
template. Generation builds its messages only when sending, dropping raw entries before
awaiting the provider. Compaction inspects those entries or supplies its own summary
messages; it no longer creates an outbound context only to discard it. Authentication,
resource snapshots, system-entry commits and provider configuration preparation stay
on the same path. No allocator tuning or durability changes are included.

## Reference and correctness checks

`scripts/capture-context-estimate-reference.mjs` executes actual pinned Pi
`estimateContext` and `orderToolResults` functions for 12 normalized context views.
Native compares both the complete ordered messages and the resulting token count.
Cases cover empty input, UTF-16 rounding, newest usage and component fallback,
repeated/missing/stray/late tool results, reused IDs in later rounds, image/thinking
estimates, usage before/after compaction, and reset handoff. Two captures regenerate
byte for byte. Source pin/hashes, inputs and expected outputs are retained at
`fixtures/native-context-estimate/`.

This is an executed pure-estimator comparison, not reference storage/context-edit
derivation. Native-specific large-history tests additionally verify borrowed estimates
and repeated-result ownership; existing generation, compaction, import and provider
tests exercise the actual runtime path. No Pi extension capability is added.

## Diagnostic and intermediate run

A private SQLite backup of the preceding synthetic desktop profile supplied 12
late-history prompts over four sessions. Temporary `/proc/self/status` probes identified
context preparation and HTTP construction as allocation-heavy phases. The probes were
removed before the final candidate. The diagnostic's sampled kernel peak fell from
88,528 to 65,260 KiB, with all 12 request lengths identical and all replies correct.
The baseline was instrumented, so these numbers are indicative attribution rather than
a controlled absolute performance qualification.

The first full 1,500-turn desktop repeat included borrowed estimation, owned ordering
and shared HTTP bytes, but still constructed the unused prepared-context copy. It
verified all 1,500 replies, 300 effects and complete durable outputs, then exited cleanly.
Server peak **123,900 KiB (121.00 MiB)** still exceeded the 100 MiB comparison target;
that result is retained rather than replaced by the short diagnostic. Desktop final
RSS was 206,356 KiB. Reports and diagnostic logs:
`fixtures/native-desktop-soak/2026-10-12-context-owned/`.

## Final candidate workload

The final candidate additionally defers outbound context construction. It uses the
unchanged 1,500-turn harness, four sessions and one active turn, with no intermediate
restart/reset: real GPUI desktop, real external native server, local scripted provider,
8/64 Ki-character streaming replies and 300 real 256 KiB bash outputs. The provider's
synthetic usage and disabled compaction make this a growing-history stress workload;
it does not establish realistic provider token accounting or compaction acceptance.
Temporary profiles use tmpfs, so storage-throughput qualification is excluded.

Raw compositor screenshots remain private; only hashes and written observations are
retained. The existing 1.25 scale is unchanged, and the guard is used for every synthetic
keystroke in the owned window on the current workspace. A focus change interrupted the
initial capture; the explicit active-window assertion stopped it. The already-created
conversation was verified before starting the workload. No user window was manipulated
and no workspace was switched.

Final executable SHA-256:
`edfec64850190b2f10b1460675b01e8e63b793cecf220d51937cfc411115bb9b`.
Reports: `fixtures/native-desktop-soak/2026-10-12-context-template/`. The provenance
files record the exact changed runtime-source hashes in addition to the base revision
and binary hash; the measured binaries include then-uncommitted implementation changes.

| Measurement | Original | Intermediate | Final candidate |
| --- | --- | --- | --- |
| Completed turns / verified durable outputs | 1,500 / 300 | 1,500 / 300 | 1,500 / 300 |
| Server VmHWM, KiB | 131,140 | 123,900 | **110,012** |
| Server VmHWM, MiB | 128.07 | 121.00 | **107.43** |
| Final server RSS, KiB | 59,212 | 93,216 | **85,632** |
| Final desktop RSS, KiB | 204,716 | 206,356 | 189,044 |
| Final desktop VmHWM, KiB | 204,716 | 206,356 | 197,896 |

The final peak is 21,128 KiB (16.11%) below the original, but **still above 100 MiB**.
Final settled server RSS is higher than the original; this is not a blanket claim of
lower memory or leak freedom. Last-half settled slopes are +5.292 KiB/turn for the
server and +19.680 KiB/turn for the desktop (same OLS definition as the original report).
Allocator retention and remaining context copies need further attribution. The original
defined text-only memory benchmark's pass does not hide this larger stress result.

Final duration is 274.657 seconds. Desktop/server fds stay at 54/23 in every sample;
desktop threads remain 38, server threads rise from 10 to 12. All recorded window samples
are mapped/visible/active, 1440×960 logical Wayland pixels. All 1,500 final replies and
300 unique ordered effects/durable tool outputs match exactly, with zero dropped bytes,
unsettled runs or finalized stream chunks; outbox rows remain 4,096. There are 1,800
provider calls with the same turn/tool-follow-up sequence as the original. Different
scratch paths increase individual serialized request lengths by 12–912 bytes; request
body byte identity across those independent profiles is not claimed. Node/Pi traps
remain unused; guarded Quit and server termination both exit 0.

The final UI shows its latest reply, composer and footer. Guarded performance-palette
input is responsive: 21 CPU handler-to-composer-paint samples, p95 3.789 ms, maximum
3.801 ms; rolling 600 transcript frames have CPU p95 3.798 ms with one mounted row and
one cached block. These are not display-presentation or whole-run latency percentiles.
Required workspace tests overlapped part of each repeat, so latency comparisons are
not qualified as quiet-machine speedups.

Required final checks: **1,656 passed, 0 failed, 56 ignored**, clean
`cargo clippy --workspace --all-targets` and `cargo fmt --all --check`.
Logs: `/tmp/pipkin-context-template-workspace.log`,
`/tmp/pipkin-context-template-clippy.log`, `/tmp/pipkin-context-template-fmt.log`.
The focused context suite, 15 runtime compaction tests and executed generation comparison
also pass. Reference regeneration is byte-identical; `git diff --check` passes.

The memory/resource gate remains open. Next work should separate live context allocations
from allocator retention, eliminate remaining needless copies, and verify realistic
usage/compaction as well as the repeated stress workload. Final installed/platform/CI,
notice review, native-default and Pi retirement remain outstanding; no ledger row becomes
fully qualified from this partial performance improvement.
