# Matched packaged Pi/native performance

2026-10-12, following `42b196d`, native protocol 12/schema 10. These are local Linux
headless measurements, with fresh disk profiles and warm executable/filesystem caches.
Native remains opt-in; this is evidence for the defined workload, not full release approval.

## Identity and workload

`scripts/bench-paired-engines.sh` extracts two trusted local production archives, verifies their
identical application binary/build stamp, and runs both production adapters/core/storage paths
against the packaged engines. The Pi package uses frozen upstream
`d2a311097cbcf669e699479587332ae3988a49d0`, the existing overlays/model data and private Node
22.23.3. The native package contains no Pi/Node runtime. Both contain app SHA-256
`4f5453d4b7d07436bfb01209de07d7a4b8284a19fda4905d06e6bde2efe659b6`, built from `42b196d`
plus working changes. Exact archive/build identities, machine data, test input hashes, all
40 profile observations and logs are in `fixtures/native-bench/2026-10-12-paired/`.

The final recorded run used an Intel Core Ultra X7 358H, 16 logical CPUs, kernel
7.2.5-3-omarchy, Btrfs scratch storage and the workspace release profile. Twenty fresh profiles
per engine run six coding turns each, alternating which engine runs first. Each turn executes
real bash with an append effect and 1 MiB output, then a 72 KiB final response through a local
scripted HTTP provider. All **240 effects**, 240 exact replies and 480 provider requests are
asserted. Repeated provider IDs remain original in outgoing context. All owned engine processes
exit after each profile. Profiles/account environment are isolated; no live account is used.

Readiness runs from host/adapter/core creation to client Ready, including storage startup.
Conversation open/model selection is measured separately. Turn latency ends at core idle,
not first token, stream-chunk commit or painted output. Memory covers the tagged engine process
set, excluding the test/provider/desktop process and transient tool subprocesses. Peak RSS is
sampled every 5 ms; per-process kernel high-water marks are also retained. Summed RSS can count
shared pages more than once across Pi processes, so idle PSS is reported separately. CPU is
the engine process tick delta over the six coding turns, with 10 ms tick resolution.

## Final results

All values below are p95 (20 profiles per engine; 120 turns per engine).

| Measurement | Pi | Native |
| --- | ---: | ---: |
| Warm client readiness | 830.414 ms | 42.984 ms |
| Conversation open and model selection | 470.938 ms | 6.708 ms |
| Idle engine RSS | 464,284 KiB | 32,048 KiB |
| Idle engine PSS | 343,899 KiB | 29,124 KiB |
| Sampled peak engine RSS | 494,304 KiB | 37,708 KiB |
| Engine CPU per six-turn profile | 480 ms | 150 ms |
| Submit-to-idle turn latency | 87.822 ms | 50.125 ms |

Idle RSS falls **93.10%**, sampled peak RSS **92.37%**. Native maximum idle/peak RSS is
32,060/38,084 KiB. All five explicit gates pass: p95 warm readiness ≤250 ms, maximum idle
≤40 MiB, maximum sampled peak ≤100 MiB, and ≥80% p95 idle/peak RSS reduction versus Pi.
This is a measured engine/process comparison, not a claim of those reductions for the entire
desktop application. Similar median turn latency (Pi 40.218 ms, native 41.318 ms) also makes
clear that these local provider timings do not imply uniformly faster real model responses.

## Failures, reproducibility and limits

The first comparison found a real native repeated-tool-ID defect: Pi executed both effects,
native only one. It was fixed with durable invocation identities and covered through recovery
and schema migration before these results were accepted; see [identity evidence](tool-call-identities.md).
An earlier benchmark layout failed Pi readiness with longer on-disk socket paths; the harness
now keeps private short control sockets in `/tmp` while durable state stays on Btrfs. That
failed attempt is not evidence of a Pi production defect. A two-pair/two-turn fixed smoke run
passed. The first full run also passed, but other qualification work could overlap it; its raw
report, machine and log remain under `preliminary-*`. The final full run was repeated after
the other qualification jobs finished and is the result reported above. Ordinary host activity
was not disabled; no cache eviction, CPU isolation or statistical confidence interval is claimed.

The dedicated ignored test is excluded from the broad installed Pi/CI e2e command because
it requires two actual artifacts and a release test build. Its script is the explicit runner.
Required workspace tests, Clippy and formatting pass; no hosted CI run is inferred.

These measurements close the matched Pi execution comparison for this workload. Sustained
large-context memory, residual long-soak slopes, small-chunk write amplification, long-running
tool/desktop workloads, uncached startup and broader platform/release checks remain open.
The short six-turn profiles are not substitutes for the separately recorded long soak.
