# Reference engine baseline (Pi experimental server)

Measured on 2026-10-11 against the staged reference tree (Pi rev `d2a311097cbcf669e699479587332ae3988a49d0` plus
Pipkin overlays), run from source in a scratch copy. This is the baseline the native Rust engine is compared with.
Each number below is labelled **measured**, **derived** (computed from measured numbers) or **not measured**.

## Machine and runtime

* Intel Core Ultra X7 358H, 16 logical CPUs, 62 GiB RAM, Linux 7.2.5-3-omarchy (Omarchy/Hyprland session).
* Node v26.8.1 (mise; the tree requires Node >= 22.19.0), npm 11.19.0. No other Node was installed or used.
* The machine was **not idle**: other build jobs ran at the same time (load average 3.9 to 4.8 during the runs).
  Treat the p95 values as noisy; the p50 values were stable across repeated runs.
* Engine started from source: `node --import .../source-resolver.ts .../experimental/cli.ts server --provider stub --model scripted`
  with `PI_EXPERIMENTAL=1 PI_OFFLINE=1`, a private `PI_SERVER_DIR`, `PI_CODING_AGENT_DIR` = a scratch agent dir whose
  `models.json` points a custom `openai-completions` provider at a local stub (see any `*/scenario.md`), and
  `HOME` = a scratch dir. No credentials, no network providers.

## Method

The driver is `_driver/bench.ts`, run with `_driver/run.sh` (it uses the reference's own client runtime over the
Unix socket). Raw output is in `_baseline/*.json` (process command lines removed).

* **Startup** (`DRIVER=bench.ts ./run.sh startup 10`): each run uses a fresh agent dir, server dir and git project.
  The driver process (with its client modules) stays loaded, so the runs are warm: the OS file cache and
  Node's on-disk state are warm, but each run starts a new server, coordinator and session worker. 2 warm-up runs
  are discarded, then 10 are measured.
  * `serverReady`: spawn of the server process until it prints `Socket:` (coordinator and backend up).
  * `promptReady`: spawn until a client has connected, created a Session (`SessionManagement.create` with the
    project cwd), `prepareSession`, `attach`ed (which starts the session worker), and the session services
    (`Models, AgentController, Transcript, Subagents, History`) are ready with the transcript hydrated. A prompt
    can be sent at this point.
  * `firstPromptRoundTrip`: `prompt("ping")` until `waitForPrompt` settles and the conversation is idle, with a stub
    that answers at once.
  * Idle memory: 2 s after that first prompt, VmRSS (`/proc/<pid>/status`) and Pss (`/proc/<pid>/smaps_rollup`) of
    every process whose initial environment carries this profile's `PI_SERVER_DIR` and `PI_SERVER_ID` (the same
    rule as Pipkin's `engine::profile_pids`).
* **200-turn soak** (`DRIVER=bench.ts ./run.sh soak 200`): one Session, 200 sequential prompts `prompt <n>`, each
  answered by the stub with `answer <n>: ` + `"words " x 40` (no tools). Memory sampled after turn 1 and every 10
  turns, plus a 50 ms background sampler for the peak; request recording in the stub turned off.
* **Size**: `du -sb` of the scratch copy after `npm ci`, and of a second copy after `npm prune --omit=dev`.
  The pruned copy was checked by running the startup benchmark (5 runs) with its server, coordinator and worker
  started from the pruned tree (`PI_TREE=<pruned>`): it starts, attaches and answers a prompt.

## Results

### Startup to first prompt-ready (measured, 10 warm runs)

| metric | p50 | p95 | min | max |
|---|---|---|---|---|
| serverReady (spawn to `Socket:`) | 687.4 ms | 1060.4 ms | 666.5 ms | 1060.4 ms |
| promptReady (spawn to session attached and services ready) | 1161.9 ms | 1917.2 ms | 1145.1 ms | 1917.2 ms |
| first prompt round trip (stub answers at once) | 55.9 ms | 93.8 ms | | |

Raw promptReady values (ms): 1160.6, 1161.9, 1157.4, 1145.1, 1917.2, 1251.2, 1168.8, 1167.0, 1162.1, 1145.4.
The single 1917 ms run coincided with other load on the machine. An earlier identical 10-run batch (same machine,
same hour; its file was overwritten, numbers copied from its log) gave serverReady p50 687.1 / p95 706.8 ms and
promptReady p50 1165.4 / p95 1203.2 ms. A 5-run batch from the pruned tree gave serverReady p50 688.9 / p95 729.8 ms
and promptReady p50 1159.5 / p95 1236.8 ms.

With 10 samples, "p95" here is the largest sample.

### Idle memory with one attached session (measured)

| process | VmRSS | Pss |
|---|---|---|
| server (`pi`, the process the launcher starts) | ~185 MiB (184,916 KiB) | ~128 MiB (131,451 KiB) |
| coordinator (`__PI_INTERNAL_SPAWN=coordinator`) | ~106 MiB (108,292 KiB) | ~58 MiB (59,571 KiB) |
| session worker (`__PI_INTERNAL_SPAWN=session-worker`) | ~172 MiB (175,848 KiB) | ~120 MiB (122,570 KiB) |
| **total, 3 processes** | **p50 470,256 KiB (~459 MiB)**, range 468,256 to 473,724 | **p50 314,695 KiB (~307 MiB)**, range 312,694 to 318,304 |

Per-process rows are from one representative run; the totals are over the 10 measured runs. RSS double-counts
shared pages between the three Node processes; Pss splits them, so the Pss total is the better "memory this
engine costs" figure.

### 200-turn text conversation (measured)

| point | total VmRSS | total Pss |
|---|---|---|
| idle before turn 1 | 468,104 KiB | 312,700 KiB |
| after turn 1 | 469,964 KiB | |
| after turn 200 | 565,624 KiB | 410,039 KiB |
| idle 2 s after turn 200 | 565,628 KiB | |
| peak (50 ms sampler) | 565,348 KiB (the sampler missed the final snapshot by a few KiB) | |

Growth over 200 turns: about +95 MiB in total, RSS (+97,520 KiB) and Pss (+97,339 KiB) alike (derived). Per process (RSS / Pss KiB):

| process | turn 0 | turn 50 | turn 100 | turn 150 | turn 200 |
|---|---|---|---|---|---|
| server | 186,104 / 132,630 | 201,628 / 148,149 | 211,480 / 158,001 | 211,856 / 158,377 | 211,928 / 158,449 |
| coordinator | 106,940 / 58,168 | 107,608 / 58,836 | 107,612 / 58,840 | 110,120 / 61,348 | 110,124 / 61,352 |
| session worker | 175,060 / 121,902 | 212,204 / 158,870 | 216,364 / 163,030 | 229,520 / 176,186 | 243,572 / 190,238 |

The session worker was still growing at turn 200 (about +0.27 MiB per turn over the last 100 turns, derived).
The server levelled off after about 70 turns. No garbage collection was forced, so this does not show whether the
growth is retained. Turn latency with an instant stub (prompt to settled and idle, measured): p50 5.5 ms,
p95 25.4 ms, max 86 ms (the first turn).

### Process count (measured)

* Idle with one attached session: **3** processes (server, coordinator, session worker), in every run.
  The launcher script `pi-test.sh` adds a 4th (a bash parent) when it is used; the driver started Node directly.
* During the 200-turn soak: at most 3 (sampled every 50 ms).
* During a bash tool call (scenario `h`): 5: the 3 above plus `bash -c <command>` and its child. When the worker was
  SIGKILLed, those tool processes survived it (they were killed by the driver). The server started a replacement
  worker immediately, without a client asking.
* After SIGTERM to the server: 0 left in every run (the coordinator and worker exit with it).
* Subagent runs (scenario `f`) were not checked for extra processes (**not measured**).

### Size of the staged engine (measured)

| tree | bytes (`du -sb`) |
|---|---|
| staged source tree as given (incl. `.git`, no `node_modules`) | 32,984,461 (31.5 MiB) |
| same, excluding `.git` | 24,399,005 (23.3 MiB) |
| scratch copy after `npm ci` (incl. `.git`) | 413,731,353 (394.6 MiB) |
| its `node_modules` (root; plus `packages/ai/node_modules` 18,531,904 and `packages/coding-agent/node_modules` 1,880,441, included in the total) | 360,334,547 (343.6 MiB) |
| after `npm prune --omit=dev` (incl. `.git`) | 163,529,837 (156.0 MiB) |
| pruned, excluding `.git` | 154,944,381 (147.8 MiB) |
| pruned `node_modules` | 110,133,031 (105.0 MiB) |

Files (excluding `.git`): 27,628 after `npm ci`, 18,348 after pruning. Top-level packages in `node_modules`: 234,
then 109. npm 11 skipped the dependency install scripts (`canvas`, `cpu-features`, `esbuild`, `protobufjs`, `ssh2`,
`@google/genai`, `autoevals`) because they are not in `allowScripts`; the server, worker, tools and the
openai-completions provider ran without them. The size includes the whole monorepo workspace (TUI, evals, examples),
not only what the server needs.

## Not measured

* Cold start (after a reboot or with the OS file cache dropped): **not measured**; dropping caches needs root.
* Display-presentation latency, IME, screen reader, display scale, suspend/resume: not applicable to this engine
  baseline and **not measured**.
* Real provider latency or token throughput: **not measured** (stub provider only, by design).
* CPU time per turn, open file descriptors, sqlite file growth over the soak: **not measured**.
* Memory after a forced GC, and whether the worker's growth is a leak: **not measured**.
* A compiled/bundled engine (Bun binary or bundled Node): **not measured**; everything ran from TypeScript source
  through Node's type stripping.
