# Native engine progress record

Companion to the execution contract `llm-docs/pipkin-rust-engine-plan.md` (plan §13). This file
records decisions, identities, status, commands, evidence, blockers and the next safe batch.
It does not qualify anything by itself: a gate closes only with the execution evidence the plan
requires. Failed runs are recorded here, not replaced by later successes.

**Current state (2026-10-11):** main implementation in progress on branch `native-engine`.
Nothing is qualified. The Claude subscription integration is **reserved for Codex — not
implemented or qualified**. Pi remains the default and only shipped engine.

## 1. Architecture decisions

Full records: `llm-docs/pipkin-rust-engine-adr/` (index in its `README.md`).

| ADR | Decision (summary) | Status |
| --- | --- | --- |
| 0001 Execution topology | One shipped `pipkin` executable; a private worker mode runs the GPUI-free `pipkin-engine` as an **owned child** over inherited pipes; one engine process for all conversations; in-process host (same API) for tests/headless/benchmark; not a sandbox | Accepted; engine host + stdio loop implemented, desktop supervisor/worker entry not yet |
| 0002 Protocol and transport | Length-prefixed JSON frames (32 MiB max); typed `Call`/`Reply`/`Event`; `epoch`+`seq` events, session `revision`, snapshot on gap; control lane for Stop/Shutdown; protocol version 1, mismatch refused | Accepted; durable outbox/acknowledged cursor and bounded fan-out not yet built |
| 0003 Durable store and recovery | One OS-locked SQLite store per data root; explicit schema v1, backup-before-migrate, newer refused untouched; short transactions; state-driven run loop; fail closed on storage error; `synchronous=FULL` (reference `NORMAL`) | Accepted; implemented for the scripted slice |
| 0004 Stop and queue | Reference queue boundaries and outcomes; durable Stop intent then cancellation; retirement by `entries.retired` flag instead of `pi.cancelled-input` omit-edit entries | Accepted; next-request differential pending |
| 0005 Tool replay and subagent recovery | Intent before effect; unsafe-by-default replay; `subagent` is the replay-safe registration with an atomic `(parent session, call)` → child mapping and once-only delivery; recorded-contract check still to port | Accepted; tools and contract check pending |
| 0006 Extension compatibility | Native versioned extension contract + out-of-process protocol; existing TypeScript facet packages need Node, so their rows stay **blocked** and run on the **legacy bridge** (Pi) | Proposed; RE-05 spike outstanding |
| 0007 Credential ownership and migration | Own data/config roots; `auth.json`-compatible private file (0600) until a qualified OS secret store; metadata-only lookup; explicit import, no sync; reserved Claude subscription boundary returns `Unsupported` | Accepted; auth implementation in progress |

## 2. Baseline inventory pointers

| What | Where |
| --- | --- |
| Reference audit (RE-01) | `llm-docs/pipkin-rust-engine-audit/` — `README.md`, `engine-tools-durable-usage.md`, `prompt-settings-layout.md`, `service-api.md`, `extensions-subagents.md`, `providers-auth.md` |
| Parity ledger | `llm-docs/pipkin-rust-engine-parity.md` (503 rows; 0 passed) |
| Known reference defects | Parity ledger §19 (K-01..K-11), e.g. the durable server ignoring `.agents/skills` (K-01) and the `PI_*` env bullet without injected variables (K-02) |
| Intentional improvements | Parity ledger §20 (I-01..I-11): same-key/different-payload `Conflict`, `synchronous=FULL`, retirement flag, retained tool output in SQLite, ... |
| Open owner decisions | Parity ledger §21 (D-01..D-09) |
| Providers | Ledger §3 (34 adapter rows), §4 (43 provider ids = 42 built-in + `faux`), §5 (custom providers/config) |
| OAuth/auth | Ledger §6 (generic lifecycle, onboarding contract, every OAuth flow, Claude subscription rows `CLS-*`) |
| Desktop workflows | Ledger §16 (37 `e2e.rs` tests, 6 `real_pi.rs` tests, flags) |
| Platforms | `docs/platforms.md` (Linux qualification scope; macOS experimental; Windows not supported) |
| Product contracts | `docs/{extensions,subagent-inspector,support,packaging,platforms,scorecard}.md` |

## 3. Work packages

Owner: **Opus** = Claude Opus 5.5 main implementation; **Codex** = reserved final integration.
Status vocabulary (plan §13): planned / in progress / blocked / implemented-but-unverified /
qualified. "Concurrent" = being implemented by a parallel Opus worker whose code is not yet on
`native-engine` at `e7cfc42`.

| ID | Work package | Owner | Status | Notes / evidence |
| --- | --- | --- | --- | --- |
| RE-01 | Pinned-source/overlay inventory and complete parity ledger | Opus | in progress | Audit committed; ledger created (503 rows). Open: 37 `PA` rows, provider-id count reconciliation (46 expected vs 43 found, D-09) |
| RE-02 | Frozen behavioral fixtures and performance baseline | Opus | planned | No reference traces or fresh measurements yet; identity frozen in §4 |
| RE-03A | Non-reserved provider/auth feasibility and inventory | Opus | in progress | Inventory done (ledger §3–§6); feasibility fixtures not run |
| RE-03B | Claude subscription feasibility review | Codex | planned (reserved) | Not started by design; location-only references in ledger §6.4 |
| RE-04 | Context, durable-task and extension-state import spike | Opus | planned | Importer work is concurrent (RE-19); spike fixtures not built |
| RE-05 | Extension compatibility spike and ADRs | Opus | in progress | ADRs 0001–0007 drafted; compatibility spike not run |
| RE-06 | Typed engine contract and private worker transport | Opus | in progress | `pipkin-engine-protocol` v1 + frame tests (measured); durable outbox/catch-up, backpressure missing |
| RE-07 | Engine library, host/supervisor and headless harness | Opus | in progress | GPUI-free library, `host::start`, `run_stdio`; no desktop supervisor or worker entry in `pipkin` |
| RE-08 | SQLite store, migrations, lock, request ledger and outbox | Opus | in progress | Store + 12 unit tests (measured); outbox, crash-before/after-ack process tests, disk-full injection missing |
| RE-09 | Durable scripted generation and NativeBackend | Opus | in progress | Engine side measured (`rt::` tests); NativeBackend concurrent |
| RE-10 | Stop, queue, steering and recovery | Opus | in progress | Stop/withdraw/restart unit tests (measured); steering and `all` delivery tests, differential missing |
| RE-11 | Coding tools and execution environments | Opus | in progress | Concurrent (`E/tools`); this tree has machinery stubs only |
| RE-12 | First provider and real coding slice | Opus | in progress | Concurrent adapters; shared HTTP/SSE/errors/model code + tests present |
| RE-13A | Generic credentials and non-reserved providers | Opus | in progress | Concurrent (`E/auth`); skeleton returns `Unsupported` |
| RE-13B | Claude subscription integration | Codex | planned (reserved) | Reserved; native boundary must stay explicitly unavailable |
| RE-14 | Context/resources, attachments, models and accounting | Opus | in progress | Concurrent (`E/context`); models/thinking/usage unit tests present |
| RE-15 | Durable subagent/task scheduler and recovery | Opus | in progress | Child mapping + once-only delivery (store test measured); runtime/recovery-contract tests missing |
| RE-16 | Native extension compatibility implementation | Opus | in progress / blocked | Native contract concurrent; existing TypeScript facet packages blocked (ADR 0006) |
| RE-17 | Multi-session engine, history and child presentation | Opus | planned | |
| RE-18 | Compaction, questions and remaining audited services | Opus | in progress | Compaction concurrent with RE-14; questions/notices not started (`AnswerQuestion` returns NotFound) |
| RE-19 | Production importer and atomic backend ownership | Opus | in progress | Concurrent (`E/import`); `import_map` table exists |
| RE-20 | Desktop integration including goals, offline state and Changes | Opus | in progress | Concurrent desktop backend |
| RE-21 | Full parity and fault qualification | Opus (+ Codex for reserved rows) | planned | Ledger exists; no differential harness |
| RE-22 | Integration advantages and performance | Opus | planned | |
| RE-23 | Native build, packaging, CI, support and rollback | Opus | planned | Model data already embedded without Node |
| RE-24 | Beta and accepted default | Opus + owner, after Codex | planned | Blocked on RE-13B and full parity |
| RE-25 | Pi retirement and conversion completion | Opus + owner, after Codex | planned | Blocked on RE-24 |

## 4. Revisions and identities

| Item | Value |
| --- | --- |
| App base | `75f5c35f028bd2f85052ac990a20e982f35573ab` (`75f5c35`, "Add session-scoped composer prompt history") |
| Native branch | `native-engine`; head when this record was written: `e7cfc42` ("Add durable session runtime ...") over `ee106d8`, `7ab9801`, `d53a2c3` |
| Dirty state | The owner's checkout has an uncommitted edit of `llm-docs/pipkin-rust-engine-plan.md` (the current execution contract; read-only for implementers). Worktrees are otherwise clean at commit time |
| Reference engine pin | `d2a311097cbcf669e699479587332ae3988a49d0` (`packaging/pi-engine-revision`) |
| Auth bridge overlay hash (`pipkinAuthBridgeSha256`) | `370dbef417d1cdeb68239f0d283d4dceb36f5ed539e500e0ba041238aad94e3a` = `sha256(pi-onboarding.patch ‖ pi-onboarding/provider-auth.ts ‖ pi-onboarding/provider-auth-provider.ts)` |
| Subagents overlay hash (`pipkinSubagentsSha256`) | `9fb846e55a931d829e2ca25bde08ae98d2f128c14ceb6f1bc033b0a2e6fb3664` = `sha256(pi-subagents.patch ‖ pi-subagents/subagents.ts ‖ pi-subagents/subagents-provider.ts)` |
| Model-data manifest hash | `b92d631bb8bb2cac6f824a03a814baad9452164c518426b8bc4bbc5c2142bb05` (`crates/pipkin-engine/model-data/manifest.json`, equal to the archive's `.manifest.json`) |
| Model-data archive | `packaging/pi-model-data-d2a311097cbcf669e699479587332ae3988a49d0.tar.zst`, sha256 `2be5f68d90695e71372c01ccbd08d5728b93691e07c45918846997c8949bcabb` |
| Native engine schema | 1 (`crates/pipkin-engine/src/store/schema.rs::SCHEMA_VERSION`) |
| Native protocol | 1 (`crates/pipkin-engine-protocol/src/lib.rs::PROTOCOL_VERSION`) |
| Reference protocol / app DB schema (unchanged) | Pi protocol 8; desktop `pipkin.sqlite3` schema 7 |
| Toolchain | Rust 1.99.0 (`rustc 1.99.0 (b940084d7 2026-09-28)`), installed user-locally via rustup (`~/.cargo/bin`) with the owner's approval |
| Reference package/runtime artifact hashes, dependency lockfile hashes | TODO(RE-02): record from a clean `scripts/build-engine.sh` + `scripts/package.sh` run |
| Fixture identities | TODO(RE-02): none frozen yet |

## 5. Commands

```sh
# Mandatory for every batch (AGENTS.md)
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all --check

# Native engine crates only
cargo test -p pipkin-engine -p pipkin-engine-protocol

# Overlay identities, exactly as scripts/build-engine.sh computes them
cat packaging/pi-onboarding.patch packaging/pi-onboarding/provider-auth.ts \
    packaging/pi-onboarding/provider-auth-provider.ts | sha256sum
cat packaging/pi-subagents.patch packaging/pi-subagents/subagents.ts \
    packaging/pi-subagents/subagents-provider.ts | sha256sum
sha256sum crates/pipkin-engine/model-data/manifest.json \
    packaging/pi-model-data-d2a311097cbcf669e699479587332ae3988a49d0.tar.zst

# Pi reference workflows (need a staged reference engine; never on the owner's projects)
PIPKIN_PI_REPO=<staged engine dir> cargo test -p pipkin-app -- --ignored
```

The rustup toolchain is user-local; in a non-login shell use `~/.cargo/bin/cargo` explicitly.
Ledger counts are recomputed by parsing each table's `Disp.` and `Evidence` columns (first
disposition code per row).

## 6. Evidence

| Date | Revision | Command | Result | Notes |
| --- | --- | --- | --- | --- |
| 2026-10-11 | `75f5c35` (unmodified tree, before native work) | `cargo test --workspace` | **613 passed, 0 failed, 43 ignored** | Baseline for regressions; ignored = real-engine/opt-in tests |
| 2026-10-11 | `e7cfc42` | `cargo test -p pipkin-engine -p pipkin-engine-protocol` (debug) | **39 passed** (34 engine + 5 protocol), 0 failed, 0 ignored | Native unit tests only; no differential evidence |
| — | — | `cargo test --workspace` on `native-engine` head | TODO: record pass/fail/ignored counts | Required before any batch is called done |
| — | — | `cargo clippy --workspace --all-targets`, `cargo fmt --all --check` on `native-engine` head | TODO | |
| — | — | Reference traces (RE-02) | TODO | Deterministic provider, disposable repositories |
| — | — | Differential runs (RE-21) | TODO | none yet |
| — | — | Performance raw results (RE-02/RE-22): readiness, RSS, streaming p95, Stop p95, soak, package bytes | TODO | Historical 453 MiB / 625–702 MiB figures are leads only |
| — | — | Fault injection (SIGKILL at journal/admission/ack/tool intent/Stop; disk full; competing writer) | TODO | |
| — | — | Native desktop walkthrough (guarded via `scripts/guard.sh`) | TODO | IME, screen reader, scale, suspend/resume, cold boot, display latency remain open gates |
| — | — | Owner-authorized live provider checks | TODO | Never automated; never with paid shadow execution |

Retained evidence paths: TODO (create under a private, non-committed location; commit only
redacted summaries). Keep private account/session material out of committed evidence.

## 7. Current blockers

1. **Claude subscription path** — reserved for Codex (RE-03B/RE-13B). Native default and Pi
   retirement are blocked until it is qualified; beta needs it too.
2. **TypeScript facet packages** (`--pi-extension`) — cannot run natively without Node; rows
   `EXT-FACET-*`, `UIQ-*`, `E2E-33..35`, `FLAG-PI-EXTENSION` stay blocked on the legacy bridge
   until ports or an owner scope decision exist (ADR 0006).
3. **External attachment / multi-client (P19)** — `--pi-dir`, `--pi-server-id`,
   `E2E-08/17` have no native design yet; they stay on the Pi adapter.
4. **No differential harness** — no row can pass until reference traces (RE-02) and the
   backend-parameterized workflow harness exist.
5. **Owner decisions** D-01..D-09 (ledger §21), notably the `docs` prompt section, provider
   identity strings, live catalog refresh, and interactive login for providers Pipkin's
   onboarding never offered.

## 8. Safe next batch

1. Merge the concurrent tools, adapters, auth, context/compaction, importer, extensions and
   desktop-backend branches into `native-engine` one at a time, running the three mandatory
   commands after each; record counts in §6 and re-inventory affected ledger rows.
2. Build the durable outbox with acknowledged cursors and bounded per-client delivery (ADR 0002
   item 9) before the desktop adapter depends on event streams.
3. Port the two subagent overlay recovery tests and add the recorded-contract check (ADR 0005
   item 4).
4. Add the process-level fault tests for RE-08 (SIGKILL before/after acknowledgement and at
   tool intent) using the in-process host plus a child-process harness.
5. Stage the clean reference engine in a disposable directory and capture the first
   deterministic traces for GEN-01, STOP-RETIRE and Q-STEER-ONE (RE-02).

## 9. Owned files and concurrent changes

- This record, the parity ledger and the ADRs are owned by the documentation worker; other
  workers append evidence and update rows for their modules.
- Concurrent workers own: `crates/pipkin-engine/src/provider/api/*`, `src/auth/*`,
  `src/tools/*`, `src/context/*`, `src/import/*`, a native extensions module, and a native
  desktop backend under `crates/pipkin-app/src/adapters/`. When their changes land, re-check
  every ledger row naming those modules ("not started (in progress)").
- Any Pipkin feature shipped during this project must enter the ledger before cutover (plan §1).

## 10. Handoff

At the end of the main implementation, create
`llm-docs/pipkin-rust-engine-claude-subscription-handoff.md` (plan §13 "Opus to Codex handoff
package") and link it here. TODO: not created yet.
