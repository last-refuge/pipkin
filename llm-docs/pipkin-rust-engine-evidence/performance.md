# Native engine performance (plan §7 scorecard)

**2026-10-11 follow-up:** the headless memory-budget failure and missing durable catch-up
have new implementation/measurement evidence in [durable delivery and memory](durable-delivery-memory.md).
This document retains its original historical results; broader qualification remains open.


Measured 2026-10-11 on the integrated `native-engine` tree (commit `9b64a99`; the harness files below
were uncommitted while it ran, so `machine.json` says `gitDirty: true`; no engine source was changed
for the recorded numbers except the one labelled experiment). Compared with the frozen reference
baseline in `fixtures/pi-sessions/BASELINE.md` (raw data `fixtures/pi-sessions/_baseline/`).

Every number is labelled **measured** (read from a run whose raw JSON is in
`fixtures/native-bench/2026-10-11/`), **derived** (computed from measured numbers) or **not measured**.
Plan targets are marked **met**, **missed** (with the measured value) or **unverified**.

## Machine and build

* Intel Core Ultra X7 358H, 16 logical CPUs, 62 GiB RAM (65,326,544 KiB), Linux 7.2.5-3-omarchy,
  cpufreq governor `powersave`. The same machine as the reference baseline.
* rustc 1.99.0. Workspace release profile (`opt-level 3`, `debug = "line-tables-only"`).
* Scratch profiles (engine data dir, config, project, home) under `target/tmp` on **btrfs** (the
  reference ran on the same disk; not tmpfs: the store uses WAL with `synchronous=FULL`, so every
  commit is an fsync and the filesystem matters).
* **The machine was not idle.** Other agents were building in parallel worktrees. Load average at the
  start of each test is in its JSON (`loadavgStart`/`loadavgEnd`): 1-min 2.1 to 3.4 during the main
  run, with 5/15-min averages of 11 to 15 from builds just before. The readiness rerun saw 2.3/5.8/9.6.
  p50 values were stable across repeated runs; treat p95/max as noisy, as in the reference.

## Method

Harness: `crates/pipkin-engine/tests/perf.rs` (all tests `#[ignore]`), driven by
`scripts/bench-native.sh`. It spawns the real release `target/release/pipkin-engine` worker and talks
the length-prefixed frame protocol over its stdin/stdout, exactly as the desktop's private worker does
(`host::run_stdio`). The engine gets `--data-dir`, `--config-dir`, `--default-model stub/scripted`, a
cleared environment (`PATH`, scratch `HOME`), and a `models.json` in its config dir:

```json
{ "providers": { "stub": { "baseUrl": "http://127.0.0.1:<port>/v1", "api": "openai-completions",
  "apiKey": "stub", "models": [ { "id": "scripted", ... }, { "id": "scripted-think", "reasoning": true, ... } ] } } }
```

This is the same custom-provider shape the reference baseline used (`_driver/lib.ts makeProfile`), so
every measurement exercises the real catalog/auth layer, the `openai-completions` adapter, SSE
decoding, the run loop, the durable store and frame encoding. The provider is a scripted
OpenAI-compatible HTTP/1.1 stub (std::net, chunked transfer, `TCP_NODELAY`) inside the test process,
replying with the reference stub's event shape (`role` chunk, content chunk(s), `finish_reason` with
usage `10/5/15`, `[DONE]`). Each test project is a fresh git repo with one commit (as in the reference).
No network, no credentials, no real provider.

Clocks: the stub and the client share one process and `Instant`. "Chunk arrival" = the stub's
`write` of that SSE chunk to the loopback socket returned. "Committed delta at the client" = the client
finished reading the frame that carries it. Memory from `/proc/<pid>/status` (VmRSS, VmHWM, RssAnon)
and `/proc/<pid>/smaps_rollup` (Pss); CPU from `/proc/<pid>/stat` utime+stime (10 ms ticks); I/O from
`/proc/<pid>/io`; fds from `/proc/<pid>/fd`; processes = engine + live descendants (scan of
`/proc/*/stat` ppid chains). Percentiles use the nearest-rank rule of the reference driver.

### Commands

```sh
scripts/bench-native.sh                         # builds engine + app, runs everything, writes fixtures/native-bench/<date>/
# or one test:
PIPKIN_BENCH_OUT=fixtures/native-bench/2026-10-11 \
  cargo test --release -p pipkin-engine --test perf -- --ignored --nocapture --test-threads=1 --exact readiness
```

The recorded set came from: one full `scripts/bench-native.sh` run (`run.log`), then
`--exact soak_diagnostic` (added after the soak result), then a `--exact readiness` rerun (added the
existing-profile restart and `--version` rows; it overwrote `readiness.json`; `run.log` still holds the
first readiness run, whose numbers agree within noise: welcome p50 22.8 vs 22.5 ms), and one labelled
experiment (`experiment-last-head-index/`, see "Bugs found").

Raw files: `machine.json`, `readiness.json`, `soak-200.json`, `soak-1500.json`,
`soak-diag-new-session-every-10.json`, `soak-diag-glibc-fixed-mmap-threshold.json`,
`stream-latency.json`, `stream-scaling.json`, `history.json`, `stop-latency.json`,
`package-sizes.json`, `run.log`, `experiment-last-head-index/history.json`.

## Results

### 1. Engine readiness (measured, 20 warm runs after 2 discarded)

Each run: fresh data/config/project dirs; spawn; `Hello`; `CreateSession`; `OpenSession`; `Models`;
then `Prompt("ping")` to `RunSettled`. "Warm" = OS file cache warm (the binary was just run); each run
starts a new process and creates a new database.

| metric | p50 | p95 | min | max |
|---|---|---|---|---|
| `--version` exec (process + dynamic link floor) | 1.1 ms | 1.6 ms | 1.0 | 1.7 |
| spawn → `Welcome` reply, new profile (schema created) | 22.5 ms | 26.0 ms | 20.4 | 26.5 |
| spawn → session created | 24.3 ms | 27.9 ms | 22.1 | 28.3 |
| spawn → prompt-ready (session created, snapshot + models read) | 26.0 ms | 29.6 ms | 23.8 | 29.9 |
| spawn → `Welcome`, restart on an existing profile | 18.0 ms | 18.3 ms | 17.5 | 18.4 |
| spawn → existing session opened (snapshot), restart | 18.5 ms | 18.8 ms | 17.9 | 19.0 |
| first prompt round trip (stub answers at once) | 4.3 ms | 4.7 ms | 3.8 | 4.7 |
| exit after stdin close | 5.4 ms | 6.5 ms | 4.4 | 7.6 |

About 17 ms of the existing-profile start is spent inside engine startup before the first reply (exec
floor ~1 ms); where exactly was **not measured** (no profiler/strace on this machine).

### 2. Memory, processes and fds (measured)

Idle (2 s after the first prompt, one session): **VmRSS p50 25,288 KiB (24.7 MiB)**, range 25,028 to
25,424; **Pss p50 22,689 KiB (22.2 MiB)**; RssAnon ~12.9 MiB; **1 process**, 7 threads, 13 fds,
0 children (every run).

**200-turn soak** (reference shape: one session, `prompt <n>`, reply `answer <n>: ` + `"words "` x 40,
no tools), `soak-200.json`:

| point | VmRSS | Pss |
|---|---|---|
| idle before turn 1 | 24,232 KiB | 21,608 KiB |
| after turn 1 | 25,360 KiB | |
| after turn 200 | 33,760 KiB | 31,136 KiB |
| idle 2 s after | 33,760 KiB | 31,135 KiB |
| peak (50 ms sampler) / VmHWM | 33,544 / 33,760 KiB | |

Slope 42 KiB/turn (RSS and Pss; least squares over all samples). fds 13 → 13 (max 14), threads 8,
processes max 1. Turn latency p50 5.0 / p95 6.3 / max 8.2 ms; engine CPU 2.9 ms/turn.

**1500-prompt soak** (same reply, 100 warm-up turns then 1500 measured, one session), `soak-1500.json`:

| point | VmRSS | Pss |
|---|---|---|
| idle before | 23,864 KiB | 21,262 KiB |
| after turn 1 | 25,080 KiB | |
| after turn 1600 | **104,996 KiB (102.5 MiB)** | 102,394 KiB |
| peak (50 ms sampler) | 105,552 KiB | |

RSS slope **51.0 KiB/turn** over turns 100..1600 (55 KiB/turn over the second half): it does not level
off. The growth is all RssAnon (RssFile flat at 12.2 MiB). fds flat at 13 (max 14, slope 0), threads 8,
processes 1. Turn latency p50 13.0 / p95 20.8 / max 57 ms, engine CPU 9.7 ms/turn: per-turn work grows
with the context, because each turn re-sends the whole conversation (453.7 MB of request bodies to the
stub over the run, ~560 KB per request at the end) and rebuilds it from the store.

**Diagnosis of the RSS growth (measured, `soak_diagnostic`):**

| variant (1600 turns, same replies) | RSS before → after | slope (second half) |
|---|---|---|
| one session (above) | 23,864 → 104,996 KiB | 55 KiB/turn |
| a new session every 10 turns (context stays small) | 24,024 → 30,692 KiB | 0.9 KiB/turn |
| one session, `GLIBC_TUNABLES=glibc.malloc.mmap_threshold=131072:glibc.malloc.arena_max=1` | 23,612 → 32,368 KiB | 6.1 KiB/turn |

So the growth tracks context size, not turn count, and ~90% of it disappears when glibc's dynamic mmap
threshold is pinned and arenas are limited: it is mostly the allocator retaining freed memory after
ever-larger transient per-turn buffers (the full request body and its intermediate copies), not a
demonstrated leak. The remaining ~6 KiB/turn in the single-session run and ~0.9 KiB/turn (≈9 KiB per
session) with rotation are **not explained** (candidates: per-session runtime state kept for the
process lifetime, SQLite page cache up to its 2 MiB bound, fragmentation). No heap profiler was
available, so "no leak" is **not proven**. The tunable run is diagnostic only; it is not a
recommended production setting (it cost CPU: 11.0 vs 9.7 ms/turn).

### 3. Streaming overhead: provider chunk → committed `EntryDelta` at the client (measured)

One session, sequential replies; each SSE content chunk carries a unique marker. Deltas still pending
when a stream ends are not published separately; their latency is measured to the final committed
entry (`markersFirstSeenInFinalEntry`). `stream-latency.json`:

| cadence | chunks × bytes | p50 | p95 | max | min |
|---|---|---|---|---|---|
| burst (no gap) | 2000 × 32 B | 9.2 ms | 16.0 ms | 16.1 | 4.1 |
| 2 ms gap (500 chunks/s) | 1000 × 32 B | 29.1 ms | **53.1 ms** | 56.5 | 2.8 |
| 5 ms gap | 600 × 32 B | 29.1 ms | **52.4 ms** | 56.2 | 2.7 |
| 20 ms gap | 250 × 32 B | 29.0 ms | **53.2 ms** | 56.7 | 2.9 |
| 100 ms gap | 60 × 32 B | 2.9 ms | 3.4 ms | 3.6 | 2.6 |

The shape is the engine's commit cadence (`runtime/run.rs`: `FLUSH_INTERVAL = 50 ms` measured from the
previous flush, or 16 KiB pending), plus one durable commit (~2.6 to 2.9 ms here = the minimum, a WAL
fsync on btrfs). Sparse chunks are committed at once; dense chunks wait for the 50 ms timer, so the
worst case is ≈50 ms + one commit. UI frame/display latency: **not measured** (headless).

### 4. Streaming scaling: 1/2/4/8 MiB single replies (measured, 3 reps each, medians)

Fresh engine per case, one warm-up turn, then one reply streamed by the stub with no gaps.
`stream-scaling.json` (`medians`, `linearity`, all `rows`):

| reply | chunk | wall | engine CPU | frames / bytes to client | DB write syscalls (wchar − client) | write_bytes | DB file after exit | VmHWM |
|---|---|---|---|---|---|---|---|---|
| 0.25 MiB | 64 B | 54 ms | 0.05 s | 4,104 / 1.41 MB | 2.57 MB | 3.92 MB | 1.27 MB | 30 MiB |
| 0.5 MiB | 64 B | 104 ms | 0.09 s | 8,200 / 2.81 MB | 7.42 MB | 10.0 MB | 2.39 MB | 32 MiB |
| 1 MiB | 64 B | 203 ms | 0.17 s | 16,392 / 5.63 MB | 16.9 MB | 22.2 MB | 4.62 MB | 37 MiB |
| 2 MiB | 64 B | 389 ms | 0.34 s | 32,776 / 11.27 MB | 34.0 MB | 44.0 MB | 9.08 MB | 46 MiB |
| 1 MiB | 1 KiB | 47 ms | 0.03 s | 424 / 1.57 MB | 2.73 MB | 4.98 MB | 1.80 MB | 36 MiB |
| 2 MiB | 1 KiB | 127 ms | 0.08 s | 1,432 / 3.86 MB | 9.56 MB | 14.6 MB | 4.28 MB | 46 MiB |
| 4 MiB | 1 KiB | 295 ms | 0.18 s | 3,480 / 8.50 MB | 29.0 MB | 45.8 MB | 9.31 MB | 64 MiB |
| 8 MiB | 1 KiB | 660 ms | 0.37 s | 7,576 / 17.8 MB | 62.3 MB | 99.2 MB | 19.3 MB | 100.7 MiB |

Linearity (derived, ratio to the smallest size of the same chunk size):

* 64 B chunks, 0.25 → 2 MiB (8×): wall 7.2×, CPU 6.8×, client frames 8.0×, client bytes 8.0×, DB file
  7.2×, write syscalls 13.2×, write_bytes 11.2×. Per doubling the write ratio falls 2.9 → 2.3 → 2.0:
  a step once the WAL passes its 1000-page auto-checkpoint (small replies never checkpoint inside the
  measured window), then linear. No quadratic term: a per-delta rewrite of the growing message would
  give ~64× here.
* 1 KiB chunks, 1 → 8 MiB: wall 14.1×, CPU 12.3×, writes 19.9 to 22.8×. **These are not a clean
  linearity series**: with no gaps the stub outruns the 50 ms/16 KiB commit loop, and the share of the
  reply committed as separate deltas (chunk rows + `EntryDelta` frames) versus folded into the final
  entry varies from run to run (8 MiB reps: 3,464 to 8,056 delta frames; wall 329 to 668 ms; writes 53
  to 105 MB). Per-rep rows are in the JSON. The final text is written once (`replace_entry`, then the
  chunk rows are deleted; `entryChunksLeft` = 0 in every run).
* Peak memory grows with reply size: VmHWM ≈ 25 MiB + ~9.5 × reply (8 MiB reply → 100 to 105 MiB
  peak VmHWM, and still 77 to 82 MiB RSS right after the run settled). That is several transient full copies of the message (accumulated text,
  JSON for the store, the `EntryReplaced` frame). Linear, but large.
* Client traffic ≈ 2.1 to 2.2 × reply text for 1 KiB chunks (deltas + one final `EntryReplaced` with the
  whole message; largest frame 8.39 MB for 8 MiB) and ≈ 5.4× for 64 B chunks (per-frame JSON overhead).
* 10 MiB: **not measured** (largest case 8 MiB, as asked).

### 5. History (measured)

Entries inserted through the public store API (`Store::open`, `create_session`, `Store::append_entry`
in one transaction; alternating user prompts and ~260-byte assistant replies), store closed, then the
worker opened on that data dir. `OpenSession` returns the default 200-entry window. `history.json`:

| stored entries | DB size | first `OpenSession` | warm `OpenSession` p50 / p95 | reply | `HistoryPage(50)` p50 / p95 | `HistoryPage(200)` p95 | full backward walk (500/page) |
|---|---|---|---|---|---|---|---|
| 1,000 | 0.8 MB | 1.5 ms | 0.85 / 0.94 ms | 90 KB | 0.17 / 0.17 ms | 0.70 ms | 2 pages, 5 ms |
| 10,000 | 6.4 MB | 4.8 ms | **3.2 / 3.6 ms** | 91 KB | 0.22 / **0.28 ms** | 0.76 ms | 20 pages, 63 ms |
| 100,000 | 63.6 MB | 30.5 ms | 22.0 / 27.1 ms | 91 KB | 0.19 / 0.25 ms | 1.01 ms | 200 pages, 608 ms (3.3 ms/page p95) |

Pages are bounded and constant-time; every entry is reached exactly once by paging; engine RSS stays
~27 MiB with 100,000 entries. **`OpenSession` is not bounded: it grows linearly with history**
(0.85 → 3.2 → 22 ms) although it returns 200 entries — see "Bugs found" (a query plan; verified with a
one-character fix: 0.71 / 0.83 / 0.66 ms). Sessions with many `runs` rows were not built (direct inserts
create none), so the cost of the `open_run` scan for long-lived sessions is **not measured**.

### 6. Stop latency (measured, 20 runs after 2 discarded)

One session. (a) The stub sent a partial reply and holds the stream open (keep-alive comments); Stop
sent 20 ms after the first committed delta. (b) The model called `bash` with `sleep 30`; Stop sent 20 ms
after a `sleep` process appeared under the engine. `stop-latency.json`:

| | `Stopping` reply | Stop → `RunSettled{Aborted}` | other |
|---|---|---|---|
| streaming | p50 2.1 / p95 2.3 ms | **p50 4.1 / p95 4.6 ms** (max 4.9) | provider connection closed: p50 2.2 / p95 2.4 ms |
| `bash sleep 30` | p50 2.2 / p95 2.8 ms | **p50 4.9 / p95 5.5 ms** (max 5.5) | `sleep` gone: p50 4.9 / p95 5.6 ms |

Every run settled `Aborted`. After each tool stop the engine had 0 descendants (no orphaned process);
the `sleep` process disappeared within ~0.1 ms of the settle event (the 200 µs poll saw it gone just
after the event arrived, so "killed strictly before settlement was published" is **not shown** by this
data). No escalation step exists to time: the bash tool kills the process group with SIGKILL at once
(`tools/bash.rs`). Stop under heavy storage latency: **not measured**.

### 7. Package bytes (measured for binaries only)

`package-sizes.json` (`strip -o`, then compressors on the stripped file):

| binary | unstripped | stripped | gzip -9 | xz -6 | zstd -19 | dynamic libraries |
|---|---|---|---|---|---|---|
| `target/release/pipkin-engine` (headless worker) | 94,304,976 B (89.9 MiB) | **14,818,984 B (14.1 MiB)** | 5.96 MB | 4.41 MB | 4.75 MB | libc, libm, libgcc_s only |
| `target/release/pipkin` (desktop app; contains the engine as `--engine-worker`) | 337,112,424 B (321.5 MiB) | **51,279,176 B (48.9 MiB)** | 20.6 MB | 14.5 MB | 15.7 MB | + libxcb, libxkbcommon(-x11), libXau, libXdmcp |

(Unstripped sizes include the `line-tables-only` debug info of the workspace release profile.)

The reference staged engine (`BASELINE.md`): pruned production tree 154,944,381 B (147.8 MiB) excluding
`.git`, of which `node_modules` 110,133,031 B, 18,348 files; and that excludes the Node runtime itself.
The headless engine binary is **~9.6% of the pruned reference tree** (derived; 14.1 / 147.8 MiB), with no
Node, npm or Pi payload. This is **not** the plan's package metric: the shipped desktop runs the engine
from its own executable, so the installed-byte and archive-byte comparison needs the packaging worker's
real package of each candidate (app + resources, with and without the staged Pi/Node payload).
Installed/archive package reduction: **not measured here**.

## Comparison with the reference baseline

Ratios are native ÷ reference (derived). Reference numbers are from `BASELINE.md` (10 warm runs; 3
Node processes: server, coordinator, session worker).

| metric | reference (Pi) | native | ratio | status |
|---|---|---|---|---|
| ready to accept a client: `Socket:` vs `Welcome` (p50 / p95) | 687.4 / 1060.4 ms | 22.5 / 26.0 ms | 0.033 / 0.025 | measured |
| prompt-ready (p50 / p95) | 1161.9 / 1917.2 ms | 26.0 / 29.6 ms | 0.022 / 0.015 | measured (definitions differ slightly: reference attaches a session worker and hydrates 5 services; native creates + opens a session and reads models) |
| first prompt round trip (p50 / p95) | 55.9 / 93.8 ms | 4.3 / 4.7 ms | 0.077 / 0.050 | measured |
| idle total VmRSS (p50) | 470,256 KiB | 25,288 KiB | 0.054 (−94.6%) | measured |
| idle total Pss (p50) | 314,695 KiB | 22,689 KiB | 0.072 (−92.8%) | measured |
| processes, idle with one session | 3 | 1 | | measured |
| processes during a bash tool | 5 | engine + tool process(es) (exact count during the tool not recorded) | | partly measured |
| tool processes after Stop | survived a worker SIGKILL (scenario h) | 0 descendants after every Stop | | measured (different scenario: Stop, not SIGKILL) |
| 200-turn soak: total RSS after turn 200 | 565,624 KiB | 33,760 KiB | 0.060 (−94.0%) | measured |
| 200-turn soak: total Pss after turn 200 | 410,039 KiB | 31,136 KiB | 0.076 (−92.4%) | measured |
| 200-turn soak: growth | +97,520 KiB RSS | +9,528 KiB RSS | 0.098 | measured |
| 200-turn turn latency (p50 / p95) | 5.5 / 25.4 ms | 5.0 / 6.3 ms | 0.91 / 0.25 | measured |
| 1500-prompt soak | not measured | 102.5 MiB after 1600 turns, 51 KiB/turn | | measured (native only) |
| fds | not measured | 13 idle, flat over 1600 turns | | measured (native only) |
| CPU per turn | not measured | 2.9 ms (200-turn), 9.7 ms (turns 100..1600) | | measured (native only) |
| streaming overhead, stop latency, history paging, stream scaling | not measured | sections 3 to 6 | | measured (native only) |
| engine payload bytes | 147.8 MiB pruned tree + Node runtime | 14.1 MiB stripped binary | ~0.096 (excl. Node) | measured, not a package comparison |
| cold start (caches dropped) | not measured | not measured | | not measured (needs root) |

## Plan §7 targets

| target | result | status |
|---|---|---|
| Runtime dependency: no Node/npm/Pi/daemon | Headless engine links only libc/libm/libgcc_s and ran every test with a cleared environment; the desktop default-path check is outside this harness | **met for the engine binary**; desktop/package path unverified here |
| Topology: one engine process, independent of conversation count; no orphaned owned children | 1 engine process throughout (incl. 160 sessions in the rotation soak); 0 descendants after every Stop; desktop + worker pair not measured here | **met (engine side)**; desktop pair unverified |
| Readiness p95 ≤250 ms warm | 26.0 ms (new profile), 18.3 ms (existing profile) to `Welcome`; 29.6 ms to prompt-ready | **met** |
| Readiness ≤1 s uncached | not measured (dropping caches needs root) | **unverified** |
| GUI-ready, true cold boot | not measured (headless harness) | **unverified** |
| Memory ≤40 MiB idle | 24.7 MiB RSS / 22.2 MiB Pss | **met** |
| ≤100 MiB for the single-active-session scripted workload (excl. tools) | 200-turn reference workload: 33.0 MiB. 1600-turn single session: **102.5 MiB** RSS at the end, still rising (allocator retention, see §2). One 8 MiB reply: 100 to 105 MiB peak. The plan does not define "the workload" | **met for the 200-turn workload; missed for the 1500-prompt soak (102.5 MiB) and transiently for an 8 MiB reply** |
| ≥80% less than comparable Pi workload | idle Pss −92.8%, 200-turn Pss −92.4% | **met** |
| Streaming overhead p95 ≤50 ms | **52.4 to 53.2 ms** p95 (max 56.7) at 2/5/20 ms chunk gaps; 3.4 ms at 100 ms gaps; 16 ms for a burst. Durability was not weakened to measure this | **missed by ~3 ms** (cause and fix in "Bugs found") |
| Streaming scaling linear, bounded checkpoints | Linear wall/CPU/client bytes (64 B series 8×→7.2×/6.8×/8.0×); writes linear after a WAL-checkpoint step; final text written once; peak RSS ~9.5× reply size; 10 MiB not run | **met for linearity (≤8 MiB)**; memory peak flagged |
| History ≥10,000 items, bounded paging, initial page p95 ≤100 ms warm | 10k: OpenSession p95 3.6 ms, HistoryPage(50) p95 0.28 ms. But OpenSession cost is O(history) (22 ms at 100k) | **met at 10k**; "no full-history work per conversation selection" **missed** (O(n) index scan; bug below) |
| Stop p95 ≤100 ms | 4.6 ms streaming, 5.5 ms during `bash sleep` (process gone p95 5.6 ms); all `Aborted` | **met**; bounded escalation and slow-storage stop unverified |
| Resource stability ≥1500 prompts | fds/threads/processes flat; RSS +51 KiB/turn without levelling (mostly allocator retention; residual 6 KiB/turn unexplained); long streaming/tool-output/multi-session soak not run | **missed / needs follow-up** (sustained RSS growth) |
| Package ≥40% installed / ≥25% archive reduction | binary sizes only | **unverified** (needs packaging output) |
| First run; UI regression | not in scope of this headless harness | **unverified** |

## Bugs and performance issues found

1. **`OpenSession` is O(history): `Store::last_head` scans the whole session.** (Not fixed: the file is
   not owned by this task.) `crates/pipkin-engine/src/store/mod.rs` `last_head` runs
   `SELECT id, body FROM entries WHERE session = ?1 AND kind IN ('compaction', 'reset') ORDER BY id DESC LIMIT 1`.
   `EXPLAIN QUERY PLAN` on the V1 schema: `SEARCH entries USING INDEX sqlite_autoindex_entries_1 (session=?)`,
   i.e. SQLite walks the `(session, id)` primary key backwards checking `kind` on every row instead of
   using `entries_by_kind (session, kind, id)`; in a session without a compaction/reset it visits every
   entry. Measured: warm `OpenSession` p50 0.85 / 3.2 / 22.0 ms at 1k / 10k / 100k entries. With the
   single change `ORDER BY +id DESC` the plan becomes `SEARCH entries USING INDEX entries_by_kind
   (session=? AND kind=?)` + a temp b-tree over the (few) markers, and the same test gives 0.71 / 0.83 /
   0.66 ms (`experiment-last-head-index/history.json`; the change was reverted after the run and is not
   in the measured engine). `last_head` is also called by `context_entries`, so every turn pays the same
   scan. Related, unmeasured: `open_run` (`... WHERE session = ?1 AND state != 'settled' ORDER BY id DESC
   LIMIT 1`) scans every `runs` row of the session via `runs_open (session=?)`; `state IN ('running',
   'stopping')` would seek.
2. **Streaming overhead p95 is ~3 ms over the 50 ms target by construction.** `runtime/run.rs` flushes
   pending deltas when `FLUSH_INTERVAL` (50 ms) has elapsed since the previous flush (or 16 KiB are
   pending), then commits durably (~2.6 to 2.9 ms per fsync commit on this btrfs disk), so a delta that
   arrives just after a flush is published ≈50 ms + one commit later (measured p95 52.4 to 53.2 ms, max
   56.7 ms). Keeping `synchronous=FULL`, the target can be met by budgeting the commit inside the
   interval (for example a 40 ms timer, or a deadline measured from the oldest pending delta minus the
   observed commit time). Not changed here (file owned by the lead).
3. **RSS grows ~51 KiB/turn over a long single session** (102.5 MiB after 1600 turns, not levelling).
   Diagnostics point at glibc retention after growing per-turn transient buffers (6 KiB/turn with a
   pinned mmap threshold and one arena; 0.9 KiB/turn when sessions rotate), not a proven leak. Options
   for the owner: reduce full-context copies per turn (request JSON is built from a fresh read of all
   context entries each turn), cap/trim allocator retention (`malloc_trim` after a run, `mallopt`
   M_MMAP_THRESHOLD at worker start, or a different allocator), then re-run `soak_1500` and
   `soak_diagnostic`. The residual growth needs a heap profiler to attribute.
4. **Peak memory ≈ 9.5 × the reply size for very large replies** (8 MiB reply → ~103 MiB VmHWM). Linear,
   but it means one huge reply can exceed the 100 MiB budget transiently. Likely several full copies at
   finalization (accumulated message, JSON body for `replace_entry`, the `EntryReplaced` event frame,
   the client channel); not attributed.

No quadratic behavior was found in streaming: chunk rows are appended (`append_chunks`), the final
message is written once, and the chunk rows are deleted after finalization.

## Not measured

* Cold start with dropped caches; GUI-ready; display/frame latency; IME, screen reader, display scale,
  suspend/resume (not applicable to a headless engine run).
* Real provider latency or token throughput (scripted stub only, by design).
* Allocations (no heap profiler); where the ~17 ms of engine startup goes.
* Long streaming / tool-output / multi-session soak (the 1500-prompt soak is text-only; the rotation
  diagnostic used many sessions but sequentially).
* Exact process count while a tool runs (the stop test only checked for the `sleep` descendant and for
  zero descendants afterwards).
* Installed and archive package bytes of a real native package versus the Pi package.
* Sessions with many `runs`/`requests`/`tool_calls` rows for the history test (direct inserts create
  entries only).

## After fixes (2026-10-11, lead)

The two engine bugs reported above were fixed on `native-engine` and re-measured with the same
harness (`cargo test --release -p pipkin-engine --test perf -- --ignored --nocapture
--test-threads=1 history stream_latency`; same machine, other builds still running). Raw data:
`fixtures/native-bench/2026-10-11-after-fixes/`.

1. `Store::last_head` now orders by `+id` (the session/kind index is used), and open-run
   lookups use `state IN ('running', 'stopping')`. **Measured** warm OpenSession p50/p95:
   0.69/0.75 ms (1k entries), 0.92/1.01 ms (10k), 0.65/0.80 ms (100k) — flat in history size
   (previously 0.85 / 3.2 / 22 ms p50). HistoryPage(50) p95 ≤ 0.53 ms at every size.
2. Streaming commits now run from the **oldest pending delta** with a 40 ms deadline, leaving
   room for the durable commit inside the 50 ms budget. Durability is unchanged (every
   published delta is committed first; `synchronous=FULL`). **Measured** chunk arrival →
   committed `EntryDelta` at the client: p95 43.7 ms (2 ms gaps), 44.3 ms (5 ms), 45.1 ms
   (20 ms); max 49.7 ms; burst p95 15.5 ms. The plan §7 streaming-overhead target (p95 ≤ 50 ms)
   is now **met** on this machine under this load.

Still open from the list above: RSS growth over a single very long session, and peak memory
for very large single replies.
