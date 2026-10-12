# Uninterrupted mixed and real desktop soaks

2026-10-12, source `9d61b26779142734976860f5d3415592e35b05f1`, protocol 12,
engine schema 10, desktop schema 8. No runtime changes in this batch. Native remains
opt-in. These runs add sustained desktop and uninterrupted-worker evidence; they do
not close the resource or release-qualification gates.

## Real desktop

`scripts/qualify-native-desktop-soak.py` launches the actual release Pipkin desktop and
its external native server in isolated data/config/HOME/project directories, with a
local scripted OpenAI-compatible provider. The same executable serves both processes:
SHA-256 `c3a7159357487ef81f947477c3683db6cf42a6d17b0c4a8ecb515e02482e281e`.
No account or live model is used; bash execution, persistence, event delivery and GPUI
rendering are real. Node/npm/npx/Pi trap executables record any attempted invocation.

The first conversation is created and opened through the real UI using `scripts/guard.sh`.
The driver creates three additional sessions and submits 1,500 turns round-robin, one
active turn at a time. The desktop remains on the first conversation: 375 of the turns
belong to its visible transcript. Replies contain 8,192 characters, or 65,536 every
25th turn, in 1,024-character chunks separated by 10 ms. The non-ASCII prefix means
each reply contains two more UTF-8 bytes than characters. Every fifth turn runs a real
bash command that appends its unique turn number to a file and produces 256 KiB of
output plus its identifying suffix. No history reset or process restart occurs.

The provider reports synthetic token usage; retry and compaction are disabled. This is
an expanding-history stress workload, not real token-accounting/compaction acceptance.
The largest recorded provider request is 4,194,100 bytes. Full output assertions verify
all 1,500 final replies, all 300 ordered unique effects, and all 300 durable tool outputs
byte for byte with zero dropped bytes. There are exactly 1,800 provider requests, no
unsettled runs, no finalized stream chunks, and the durable outbox remains capped at
4,096 rows. No Node/Pi trap was invoked.

| Measurement | Result |
| --- | --- |
| Workload and final verification duration | 279.675 seconds |
| Desktop RSS, initial → final | 149,556 → 204,716 KiB (146.05 → 199.92 MiB) |
| Desktop VmHWM | 204,716 KiB |
| Server RSS, initial → final | 31,976 → 59,212 KiB (31.23 → 57.82 MiB) |
| Server VmHWM | **131,140 KiB (128.07 MiB)** |
| Desktop descriptors | 54 initially/finally; settled samples 54–55 |
| Server descriptors | 23 at every sample |
| Threads | Desktop 38 throughout; server 10 initially, then 12 |
| Desktop last-half settled RSS slope | +12.511 KiB/turn |
| Server last-half settled RSS slope | −7.288 KiB/turn, with substantial transient variation |
| Graceful desktop / server exits | 0 / 0 |

Samples occur every 25 completed turns, plus initial and final readings. Slopes are
ordinary least squares over samples at turns 750–1,500, including the second final idle
reading. The final server RSS is lower than several intermediate samples; the negative
slope does not establish leak freedom. Desktop RSS still grows. Kernel VmHWM captures
short peaks missed by settled samples. **The server exceeds the 100 MiB reference target
in this expanded single-active-turn workload. Resource qualification stays open.**

The actual Wayland window is mapped, visible and active in all recorded samples at
1440×960 logical pixels on the current workspace. Existing compositor scale is 1.25;
no display configuration was changed. Guarded command-palette actions work during
and after the soak. The final performance log records 42 handler-to-composer-paint CPU
samples, p50 3.110 ms, p95 3.859 ms, maximum 4.893 ms. Its rolling 600 transcript frames
have CPU p50 0.538 ms and p95 4.560 ms, with one mounted row and one cached block.
Mid-run rolling-frame p95 was 7.944 ms. These are CPU measurements, not display
presentation latency or a full-run percentile. The palette provides the input samples.

Full-size mid-run and final captures show the reply, composer and navigation footer.
An initial reading mistakenly suggested clipping; reinspection and a temporary GPUI
layout check did not reproduce it. That test and its diagnostic selector were removed;
no speculative layout fix is included. Raw screenshots remain private because compositor
transparency exposes unrelated desktop content. Only their hashes are retained. An
AT-SPI query by this app's PID returned no matching tree; accessibility is not passed.

An eight-turn smoke preceded the full run and exited cleanly. Welcome → Explore and
new-conversation creation were guarded; explicit native attachment still shows Welcome
for a fresh profile. This is recorded behavior, not a completed onboarding review.
The user brought the test window onto the current workspace. Synthetic input was sent
only through `scripts/guard.sh` after checking the owned PID and current workspace;
no other window was focused/moved/resized and no workspace was switched by automation.

Raw reproducible reports: `fixtures/native-desktop-soak/2026-10-12/`. Runtime scratch:
`/tmp/pipkin-desktop-soak-20261012-full`; initial smoke scratch has the same prefix with
`-smoke`. Both owned desktop/server pairs were reaped. The runner deliberately requires
`ROOT/go` after guarded UI setup and `ROOT/finish` after guarded Quit:

```sh
python3 scripts/qualify-native-desktop-soak.py target/release/pipkin /tmp/FRESH-ROOT
```

The harness itself never sends UI input or changes focus/workspaces. Its identity report
records binary SHA and source revision; the observations report records the harness SHA.
The desktop scratch directory is on **tmpfs**; this run does not establish disk-backed
fsync latency or storage throughput. The copied machine metadata was captured by the
overlapping headless benchmark: its btrfs scratch path and starting load describe that
benchmark, not the desktop run. CPU/kernel/toolchain are shared. Both workloads share the machine; this is not an idle
machine latency comparison or an installed release-candidate test.

## Uninterrupted headless mixed workload

The existing [mixed benchmark](mixed-workload.md) now also ran all 720 turns in one
worker lifetime. `PERF_MIXED_RESTART_EVERY=1440` disables intermediate restarts for
the 720-turn workload; the one recorded restart happens after all turns, for final
reopen/replay verification. It still uses eight sessions and two concurrently active
turns, 64 KiB streamed replies and 1 MiB real tool output per turn. The 720 unique
effects, final replies, spill files/links and durable output rows all pass their existing
assertions, with zero lost/dropped output, unsettled runs or finalized chunks.

Worker SHA-256: `172c85c21ca8fbd633bba69d0f102fcdce7ac09705b07b29cdc709558913912f`.
Raw reports: `fixtures/native-bench/2026-10-12-uninterrupted-mixed/`.

| Measurement | Result |
| --- | --- |
| Duration | 275.225 seconds |
| Initial / final settled RSS | 27,080 / 31,380 KiB |
| Worker VmHWM / sampled 50 ms peak | 101,976 / 101,440 KiB |
| Final reopen RSS | 29,128 KiB |
| Idle descriptors / settled descendants | 13 throughout / zero |
| Last-half settled RSS slope | +2.797 KiB/turn |
| Two-turn batch p50 / p95 | 748.639 / 785.887 ms |
| Provider requests / cumulative bytes | 1,440 / 4,259,651,458 |
| Database / complete spill-file bytes | 807,178,240 / 754,984,690 |

The 99.59 MiB kernel peak is below 100 MiB in this run, with no intermediate process
reset. The positive idle slope remains explicit. The database retains the verified
tool output and history; no claim of indefinite bounded resource usage follows.

```sh
OUT=fixtures/native-bench/2026-10-12-uninterrupted-mixed SKIP_APP=1 \
PERF_MIXED_RESTART_EVERY=1440 TESTS='mixed::mixed_soak' scripts/bench-native.sh
```

## Remaining qualification

The desktop workload succeeds functionally but the expanded server-memory target does
not. Next work is to attribute/reduce the transient peak and residual desktop growth,
then repeat the same workload. Long-duration use, realistic usage/compaction, long-running
tools, remaining reference/release comparisons, final installed candidate, CI/platform
and notice review remain open. No row is fully qualified by these measurements; Pi
extensions remain outside scope.

Required checks after the harness/evidence batch: **1,653 passed, 0 failed, 56 ignored**;
`cargo clippy --workspace --all-targets` and `cargo fmt --all --check` pass. Python AST
parsing and `git diff --check` pass. Logs: `/tmp/pipkin-desktop-soak-workspace.log`,
`/tmp/pipkin-desktop-soak-clippy.log`, `/tmp/pipkin-desktop-soak-fmt.log`.
