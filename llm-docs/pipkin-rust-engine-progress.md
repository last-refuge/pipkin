# Native engine progress record

Companion to the execution contract `llm-docs/pipkin-rust-engine-plan.md` (plan §13). This file
records decisions, identities, status, commands, evidence, blockers and the next safe batch.
It does not qualify anything by itself: a gate closes only with the execution evidence the plan
requires. Failed runs are recorded here, not replaced by later successes.

**Historical handoff state (2026-10-11, `native-engine` at `4fc4060`):** the main implementation has landed
on `native-engine` and every wave-2 worker branch is merged. The native engine is an opt-in
candidate (`--engine native`); Pi remains the default and only shipped engine. **Nothing is
qualified** and no parity row is passed. The Claude subscription integration is **reserved for
Codex — not implemented or qualified**; the native boundary reports it unavailable.

## Codex follow-up on 2026-10-11

The reserved Claude subscription path has been ported on top of `852a5df`:
[implementation/evidence](pipkin-rust-engine-evidence/claude-subscription.md). RE-03B was
reviewed against official guidance; the user confirmed approval and requested the Pi port.
RE-13B is implemented; the native release binary passed user-completed browser sign-in and a real Claude Opus 5.5 coding turn (4 scratch tests passed). Full live qualification remains pending. Native onboarding capability, saved-credential
reuse/removal, concurrent refresh, subscription request conversion and explicit credential
import now have local-fixture coverage. No default cutover or parity qualification occurred.
The older module reports below remain historical evidence, including their independent gaps.

## 1. Architecture decisions

Full records: `llm-docs/pipkin-rust-engine-adr/` (index in its `README.md`).

| ADR | Decision (summary) | Status |
| --- | --- | --- |
| 0001 Execution topology | One shipped `pipkin` executable; a private worker mode runs the GPUI-free `pipkin-engine` as an **owned child** over inherited pipes; one engine process for all conversations; in-process host (same API) for tests/headless/benchmark; not a sandbox | Accepted; implemented (`--engine-worker`, `App/adapters/native/supervisor.rs`, headless `pipkin-engine` binary) |
| 0002 Protocol and transport | Length-prefixed JSON frames (32 MiB max); typed `Call`/`Reply`/`Event`; `epoch`+`seq` events, session `revision`, snapshot on gap; control lane for Stop/Shutdown; protocol version mismatch refused | Accepted; protocol **2** (adds `Import*` calls); durable outbox/acknowledged cursor and bounded fan-out still not built |
| 0003 Durable store and recovery | One OS-locked SQLite store per data root; explicit schema v1, backup-before-migrate, newer refused untouched; short transactions; state-driven run loop; fail closed on storage error; `synchronous=FULL` (reference `NORMAL`) | Accepted; implemented; SIGKILL-of-the-worker test exists; disk-full injection missing |
| 0004 Stop and queue | Reference queue boundaries and outcomes; durable Stop intent then cancellation; retirement by `entries.retired` flag instead of `pi.cancelled-input` omit-edit entries | Accepted; implemented; `all` delivery modes untested |
| 0005 Tool replay and subagent recovery | Intent before effect; unsafe-by-default replay; `subagent` is the replay-safe registration with an atomic `(parent session, call)` → child mapping and once-only delivery; recorded-contract check | Accepted; implemented incl. the recorded-contract check; contract-mismatch and disabled-recovery tests not ported |
| 0006 Extension compatibility | Native versioned extension contract + out-of-process protocol; existing TypeScript facet packages need Node, so their rows stay **blocked** and run on the **legacy bridge** (Pi) | Proposed; native contract implemented (`E/extensions`, `docs/native-extensions.md`); TS facets blocked |
| 0007 Credential ownership and migration | Own data/config roots; `auth.json`-compatible private file (0600) until a qualified OS secret store; metadata-only lookup; explicit import, no sync; reserved Claude subscription boundary returns `Unsupported` | Accepted; implemented (`E/auth`, explicit credential import) |

## 2. Baseline inventory pointers

| What | Where |
| --- | --- |
| Reference audit (RE-01) | `llm-docs/pipkin-rust-engine-audit/` — `README.md`, `engine-tools-durable-usage.md`, `prompt-settings-layout.md`, `service-api.md`, `extensions-subagents.md`, `providers-auth.md` |
| Parity ledger | `llm-docs/pipkin-rust-engine-parity.md` (503 rows; 0 passed; refreshed at `4fc4060`: 376 measured, 7 observed, 1 failed, 26 unverified, 93 not started; 17 rows compared against reference-derived output) |
| Frozen reference fixtures (RE-02) | `fixtures/pi-sessions/` (8 scenarios a–h with `session.sqlite`, `requests.jsonl`, `view.json`, `history.json`; `BASELINE.md` for the reference measurements) |
| Evidence | `llm-docs/pipkin-rust-engine-evidence/` — `performance.md` (RE-22), `packaging.md` (RE-23), `native-walkthrough.md` (guarded GUI walkthrough) |
| Claude subscription handoff | `llm-docs/pipkin-rust-engine-claude-subscription-handoff.md` |
| Known reference defects | Parity ledger §19 (K-01..K-11) |
| Intentional improvements | Parity ledger §20 (I-01..I-13) |
| Open owner decisions | Parity ledger §21 (D-01..D-11), each with the state implemented at `4fc4060` |
| Providers | Ledger §3 (34 adapter rows), §4 (43 provider ids = 42 built-in + `faux`), §5 (custom providers/config) |
| OAuth/auth | Ledger §6 (generic lifecycle, onboarding contract, every OAuth flow, Claude subscription rows `CLS-*`) |
| Desktop workflows | Ledger §16 (37 Pi `e2e.rs` workflows with their native counterparts, 6 `real_pi.rs` tests, flags) |
| Native extensions | `docs/native-extensions.md` |
| Platforms | `docs/platforms.md` (Linux qualification scope; macOS experimental; Windows not supported) |
| Product contracts | `docs/{extensions,subagent-inspector,support,packaging,platforms,scorecard}.md` |

## 3. Work packages

Owner: **Opus** = Claude Opus 5.5 main implementation; **Codex** = reserved final integration.
Status vocabulary (plan §13): planned / in progress / blocked / implemented-but-unverified /
qualified. Nothing is qualified. "Implemented-but-unverified" means the code is merged and has
native tests (cited in the ledger), but no qualification differential, live provider run or
platform gate has closed it.

| ID | Work package | Owner | Status | Evidence pointer |
| --- | --- | --- | --- | --- |
| RE-01 | Pinned-source/overlay inventory and complete parity ledger | Opus | in progress | Ledger refreshed at `4fc4060`; open: 37 `PA` rows, provider-id count (D-09), owner decisions D-01..D-11 |
| RE-02 | Frozen behavioral fixtures and performance baseline | Opus | implemented-but-unverified | `b2ff206`: `fixtures/pi-sessions/` 8 scenarios + `BASELINE.md`; reference package/runtime artifact hashes still TODO (§4) |
| RE-03A | Non-reserved provider/auth feasibility and inventory | Opus | implemented-but-unverified | All non-reserved adapters and OAuth flows implemented and tested against local mocks (ledger §3–§6); no live provider call |
| RE-03B | Claude subscription feasibility review | Codex | reviewed; user-confirmed approval | `pipkin-rust-engine-evidence/claude-subscription.md`; no independent agreement or live-access verification |
| RE-04 | Context, durable-task and extension-state import spike | Opus | implemented-but-unverified | `8290556`; all 8 reference fixtures import and continue equivalently (`rti::reference_fixtures_import_open_and_continue_equivalently`); plugin state not read (IMP-PLUGIN-STATE) |
| RE-05 | Extension compatibility spike and ADRs | Opus | in progress | ADR 0006 still Proposed; native ports of the two baseline examples (`ext::`, `extp::`); TS facets blocked |
| RE-06 | Typed engine contract and private worker transport | Opus | in progress | Protocol v2 + frame and import-call round-trip tests (`frm::`, `P/view.rs`); gap detection (`ncl::sequencer_detects_gaps_and_new_epochs`); durable outbox/acknowledged cursor missing (XPORT-OUTBOX) |
| RE-07 | Engine library, host/supervisor and headless harness | Opus | implemented-but-unverified | `nsup::` (7), `ew::` (2), `wp::` (2: SIGKILL + second-writer refusal), headless `pipkin-engine` binary |
| RE-08 | SQLite store, migrations, lock, request ledger and outbox | Opus | in progress | `st::` (12), `wp::admission_survives_sigkill_and_is_found_by_lookup_after_restart`; outbox, disk-full injection, corruption policy and a real migration missing |
| RE-09 | Durable scripted generation and NativeBackend | Opus | implemented-but-unverified | `rt::` (19), `ne2e::` (20 workflows), `nt::` (9); observed in `native-walkthrough.md` |
| RE-10 | Stop, queue, steering and recovery | Opus | implemented-but-unverified | Stop/queue/steer/restart measured (`rt::`, `ne2e::`); gaps: `all` delivery modes, durable retry wait (REC-RETRY), Stop-after-completion race |
| RE-11 | Coding tools and execution environments | Opus | implemented-but-unverified | `5768dda`; `trd::`/`twr::`/`ted::`/`tbs::`/`tout::`/`trun::`/`tsch::` incl. reference-captured goldens; corpus tests ignored with no recorded run; tool rounds always parallel (GEN-03) |
| RE-12 | First provider and real coding slice | Opus | implemented-but-unverified | Walkthrough §3 (real `write` tool via an `openai-completions` custom provider to a local stub); perf harness uses the same path; no live provider |
| RE-13A | Generic credentials and non-reserved providers | Opus | implemented-but-unverified | `dd43920`, 10 wire adapters + `provider/api/common`; `auth::`, `oauth::`, adapter suites; Codex adapter goldens are reference captures |
| RE-13B | Claude subscription integration | Codex | implemented; live smoke passed | `pipkin-rust-engine-evidence/claude-subscription.md`; browser/copy-code sign-in, refresh, saved reuse and Pi-fixture Stop/reopen passed; full qualification pending |
| RE-14 | Context/resources, attachments, models and accounting | Opus | implemented-but-unverified | `384da75`, `e4fe919`; `drv::`/`prm::`/`res::`/`cmp::`; gap: no `reasoning`/`cacheWrite1h` usage counters |
| RE-15 | Durable subagent/task scheduler and recovery | Opus | implemented-but-unverified | `rt::subagents_run_once_per_call_deliver_their_answer_and_are_listed`, `rt::a_subagent_call_interrupted_by_a_restart_resumes_its_one_child`; overlay recovery tests (disabled, contract mismatch) not ported |
| RE-16 | Native extension compatibility implementation | Opus | implemented-but-unverified (native) / blocked (TS facets) | `01e0511`, `e4fe919`; `--engine-extension`; TypeScript facet packages stay `BL / LB` (ADR 0006) |
| RE-17 | Multi-session engine, history and child presentation | Opus | implemented-but-unverified | `7ba1340` paged subagent inspection (`core-sub::`, `ui-sub::`, `nt::child_history_pages_by_cursor_and_the_selected_child_refreshes_live`); history paging flat after fixes (perf-ev) |
| RE-18 | Compaction, questions and remaining audited services | Opus | implemented-but-unverified | `rt::manual_and_overflow_compaction_summarize_older_context`, `rt::extension_questions_are_published_answered_and_validated`; background/threshold runtime paths untested; `waitForPrompt`, `RemoveSession` absent (PA) |
| RE-19 | Production importer and atomic backend ownership | Opus | implemented-but-unverified | `8290556`, `21ff99a` (protocol v2 `Import*`, desktop palette flow); desktop ownership record written in the import's copy transaction; Pi mode read-only for owned conversations (core + Pi adapter boundary), explicit release from the palette (D-11, IMP-OWNERSHIP) |
| RE-20 | Desktop integration including goals, offline state and Changes | Opus | implemented-but-unverified | `6b7980f`; `ne2e::`, `nmap::`; walkthrough observed onboarding, first conversation, Changes, SIGKILL recovery |
| RE-21 | Full parity and fault qualification | Opus (+ Codex for reserved rows) | planned | Ledger refreshed; no backend-parameterized differential harness; fault matrix incomplete (disk full, tool-intent crash) |
| RE-22 | Integration advantages and performance | Opus | in progress | `c539808`, `ae95240`; `performance.md`: most §7 targets met; missed: 1500-prompt RSS stability; diagnostics-correlation advantage not done |
| RE-23 | Native build, packaging, CI, support and rollback | Opus | in progress | `c8d005c`; `packaging.md` (Linux candidate verified by `scripts/test-native-package.sh`); not in CI; no PKGBUILD/macOS package |
| RE-24 | Beta and accepted default | Opus + owner, after Codex | planned | Blocked on RE-13B and full parity |
| RE-25 | Pi retirement and conversion completion | Opus + owner, after Codex | planned | Blocked on RE-24 |

## 4. Revisions and identities

| Item | Value |
| --- | --- |
| App base | `75f5c35f028bd2f85052ac990a20e982f35573ab` (`75f5c35`, "Add session-scoped composer prompt history") |
| Native branch | `native-engine`; head for this record: `4fc40608c00e75e9e04c45f3d3ccb9a8b3360a2e` (`4fc4060`, "Merge no-Node native packaging and diagnostics"); ledger first drafted at `e7cfc42` |
| Dirty state | The owner's checkout has an uncommitted edit of `llm-docs/pipkin-rust-engine-plan.md` (the current execution contract; read-only for implementers). Worker worktrees under `.claude/worktrees/` are untracked |
| Reference engine pin | `d2a311097cbcf669e699479587332ae3988a49d0` (`packaging/pi-engine-revision`) |
| Auth bridge overlay hash (`pipkinAuthBridgeSha256`) | `370dbef417d1cdeb68239f0d283d4dceb36f5ed539e500e0ba041238aad94e3a` = `sha256(pi-onboarding.patch ‖ pi-onboarding/provider-auth.ts ‖ pi-onboarding/provider-auth-provider.ts)` |
| Subagents overlay hash (`pipkinSubagentsSha256`) | `9fb846e55a931d829e2ca25bde08ae98d2f128c14ceb6f1bc033b0a2e6fb3664` = `sha256(pi-subagents.patch ‖ pi-subagents/subagents.ts ‖ pi-subagents/subagents-provider.ts)` |
| Model-data manifest hash | `b92d631bb8bb2cac6f824a03a814baad9452164c518426b8bc4bbc5c2142bb05` (`crates/pipkin-engine/model-data/manifest.json`, equal to the archive's `.manifest.json`) |
| Model-data archive | `packaging/pi-model-data-d2a311097cbcf669e699479587332ae3988a49d0.tar.zst`, sha256 `2be5f68d90695e71372c01ccbd08d5728b93691e07c45918846997c8949bcabb` |
| Native engine schema | **1** (`crates/pipkin-engine/src/store/schema.rs::SCHEMA_VERSION`) |
| Native protocol | **2** (`crates/pipkin-engine-protocol/src/lib.rs::PROTOCOL_VERSION`; 1 → 2 added `ImportDiscover`/`ImportSessions`/`ImportConfig`/`ImportCredentials` in `21ff99a`) |
| Native engine version | `0.0.1` (`pipkin_engine::runtime::ENGINE_VERSION` = crate version) |
| Reference protocol / app DB schema (unchanged) | Pi protocol 8; desktop `pipkin.sqlite3` schema 7 |
| Toolchain | Rust 1.99.0 (`rustc 1.99.0 (b940084d7 2026-09-28)`), installed user-locally via rustup (`~/.cargo/bin`) with the owner's approval |
| Fixture identities | `fixtures/pi-sessions/` (scenarios `a-text-multiturn` … `h-worker-killed-mid-tool`, made by the pinned reference + overlays with a local scripted provider; `_driver/` regenerates them); Codex adapter goldens `crates/pipkin-engine/src/provider/api/openai_codex/fixtures/` (`regenerate.sh`); tool goldens `crates/pipkin-engine/src/tools/tests/golden-{edit,validate}.json` (`tests/reference/*.mts`) |
| Reference package/runtime artifact hashes, dependency lockfile hashes | TODO(RE-02): record from a clean `scripts/build-engine.sh` + `scripts/package.sh` run (the Pi package was not rebuilt in this phase) |

## 5. Commands

```sh
# Mandatory for every batch (AGENTS.md)
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all --check

# Native engine crates only
cargo test -p pipkin-engine -p pipkin-engine-protocol

# Native desktop backend (unit + e2e workflows; run by default)
cargo test -p pipkin-app adapters::native

# Performance harness (ignored benchmarks; writes fixtures/native-bench/<date>/)
scripts/bench-native.sh

# No-Node native package candidate (Linux)
scripts/package-native.sh            # or: scripts/package.sh --engine native
scripts/test-native-package.sh       # build, package, install, probe, uninstall checks

# Run the native candidate
cargo run -p pipkin-app --release -- --engine native

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
disposition code and first evidence word per row).

## 6. Evidence

| Date | Revision | Command | Result | Notes |
| --- | --- | --- | --- | --- |
| 2026-10-11 | `75f5c35` (unmodified tree, before native work) | `cargo test --workspace` | **613 passed, 0 failed, 43 ignored** | Baseline for regressions; ignored = real-engine/opt-in tests |
| 2026-10-11 | `e7cfc42` | `cargo test -p pipkin-engine -p pipkin-engine-protocol` (debug) | **39 passed** (34 engine + 5 protocol), 0 failed, 0 ignored | Native unit tests only |
| 2026-10-11 | integrated tree before the final runtime wiring (`9b64a99` era) | `cargo test --workspace` | **failed**: 3 test-expectation failures (exact-text expectations changed by ordered JSON keys; native tool-output copy expectation) | Fixed afterwards (`9b64a99` "Fix post-merge test expectations"); recorded here as a failed run |
| 2026-10-11 | integrated tree | engine runtime tests ×3, native e2e ×5, restart e2e ×10 | 19/19 (×3); 17/17 (×5); restart e2e flake diagnosed as a harness race and fixed, then 10/10 | From the interim status record |
| 2026-10-11 | `ae95240` (handoff commit) | `cargo test --workspace --no-fail-fast` | **1171 passed, 0 failed, 53 ignored** | Ignored: opt-in real-Pi tests, 8 perf benchmarks, 2 tool corpus comparisons. `cargo clippy --workspace --all-targets`: one test-only warning, fixed in the same commit; `cargo fmt --all --check`: clean |
| 2026-10-11 | packaging worker branch after merging `ae95240` (final package run at `9a5e19e`) | `cargo test --workspace`; clippy; fmt | **1179 passed / 0 failed / 53 ignored**; clippy 0 warnings; fmt clean | `llm-docs/pipkin-rust-engine-evidence/packaging.md` |
| 2026-10-11 | `9b64a99` + harness (dirty) | `scripts/bench-native.sh` (perf::, release) | measured; see `performance.md` | Missed at that run: streaming p95 52.4–53.2 ms, `OpenSession` O(history), 1500-prompt RSS growth |
| 2026-10-11 | `ae95240` | `cargo test --release -p pipkin-engine --test perf -- --ignored history stream_latency` | streaming p95 43.7–45.1 ms; `OpenSession` flat (0.65–0.92 ms p50) | `performance.md` "After fixes"; raw data `fixtures/native-bench/2026-10-11-after-fixes/`. Still open: single-session RSS growth, large-reply peak |
| 2026-10-11 | `9a5e19e` | `scripts/test-native-package.sh` (full) | 23/23 checks, native package verified | Installed tree 345 MB, archive 72.6 MB (unstripped); Pi package not rebuilt |
| 2026-10-11 | `ae95240` release build | Guarded native desktop walkthrough via `scripts/guard.sh` | observed: onboarding (Claude disabled with the reserved reason), first conversation with a real `write` tool, Changes, relaunch and desktop SIGKILL recovery | `native-walkthrough.md`; one copy defect fixed afterwards, not re-observed. IME, screen reader, scale, suspend/resume, cold boot, display latency remain unverified |
| — | `4fc4060` | `cargo test --workspace && cargo clippy --workspace --all-targets && cargo fmt --all --check` | see "Handoff verification" below | Run by the lead |
| — | — | Differential runs (RE-21) | TODO | Only reference-derived goldens and import equivalence exist |
| — | — | Fault injection: disk full, read-only DB, SIGKILL at tool intent | TODO | SIGKILL after admission is covered by `wp::admission_survives_sigkill_and_is_found_by_lookup_after_restart` |
| — | — | Owner-authorized live provider checks, real ChatGPT sign-in | TODO | Never automated; never with paid shadow execution |

Retained evidence: `fixtures/native-bench/2026-10-11/`, `fixtures/native-bench/2026-10-11-after-fixes/`,
`llm-docs/pipkin-rust-engine-evidence/`. Keep private account/session material out of committed
evidence (the walkthrough's uncropped first capture stayed in a scratchpad and is not evidence).

## 7. Current blockers

1. **Claude subscription path** — reserved for Codex (RE-03B/RE-13B). Native default and Pi
   retirement are blocked until it is qualified; beta needs it too.
2. **TypeScript facet packages** (`--pi-extension`) — cannot run natively without Node; rows
   `EXT-FACET-*`, `EXT-PROFILE-*`, `EXT-CONFLICT`, `E2E-34/35`, `FLAG-PI-EXTENSION` stay
   blocked on the legacy bridge until ports or an owner scope decision exist (ADR 0006). The
   native question host and native extension contract are implemented separately.
3. **External attachment / multi-client (P19)** — `--pi-dir`, `--pi-server-id`, `E2E-08` have
   no native design; they stay on the Pi adapter.
4. **No qualification differential harness** — no row can pass until the backend-parameterized
   workflow harness compares native runs with reference traces (RE-21).
5. **Owner decisions** D-01..D-11 (ledger §21). Several are implemented one way and need
   sign-off: D-01 docs section omitted, D-04 global default model written, D-07 all OAuth logins
   offered, D-10 spill file kept, D-11 copy plus desktop ownership record (Pi read-only until released).

## 8. Safe next batch

1. Record the final `4fc4060` workspace check in "Handoff verification" (lead).
2. Close the highest-severity non-reserved gaps from the ledger: durable outbox with acknowledged cursors
   (XPORT-OUTBOX); single-session RSS growth over long sessions (E2E-37); disk-full and
   tool-intent crash injection (DUR-DISKFULL, TOOL-INTERRUPT); proxy handling that honours the
   engine environment and `httpProxy` (ADP-PROXY, SET-HTTPPROXY); Codex WebSocket close at
   conversation end.
3. Port the two subagent overlay recovery tests (SUB-RECOVERY-DISABLED, SUB-RECOVERY-CONTRACT)
   and add Stop-aborts-children and Stop-after-completion tests.
4. Add the missing settings behavior tests (`all` delivery modes, retry cap, durable retry
   wait, `compaction.modelOverrides`, legacy-key migrations `websockets` and `skills{}`).
5. Build the backend-parameterized workflow harness and capture native-vs-reference traces for
   GEN-01, STOP-RETIRE and Q-STEER-ONE (RE-21); wire the native package tests into CI (RE-23).

## 9. Owned files and concurrent changes

- This record, the parity ledger and the ADRs are owned by the documentation worker; other
  workers append evidence and update rows for their modules.
- All wave-2 worker branches are merged into `native-engine` (see "Current state of the work"
  below); no concurrent worker owns engine modules at `4fc4060`.
- Any Pipkin feature shipped during this project must enter the ledger before cutover (plan §1).

## 10. Handoff

`llm-docs/pipkin-rust-engine-claude-subscription-handoff.md` (plan §13 "Opus to Codex handoff
package") was added in `ae95240`. Its §2 note that the ledger still said "not started (in
progress)" is resolved by the refresh at `4fc4060`.

## Current state of the work (replaces the interim status of 2026-10-11)

Branch `native-engine` at `4fc4060` (not pushed). Merged and committed:

- **Engine core:** protocol v2, store (schema 1), runtime (admission, run loop, Stop retirement,
  queue and steering, crash recovery, fail-closed storage, subagents with recorded contracts,
  compaction manual/threshold/background/overflow, questions and extensions, real system
  prompt), headless `pipkin-engine` worker binary with process tests.
- **Providers and auth:** all 10 wire adapters incl. the ChatGPT subscription backend
  (`openai-codex-responses`) on the shared `provider/api/common` helpers (`aeebc8e`);
  credentials, `models.json` layering and every non-reserved OAuth flow.
- **Tools, context, extensions:** read/write/edit/bash with reference-captured goldens; context
  derivation, prompt, resources and compaction; native extension contract, process extensions,
  question host (`--engine-extension`).
- **Import:** importer, protocol v2 `Import*` calls, desktop palette flow with separate
  confirmations for conversations and credentials (`21ff99a`).
- **Desktop:** `NativeBackend` behind `--engine native` (`--engine-host child|inprocess`), 20
  native e2e workflows, paged subagent inspection (`7ba1340`), engine-neutral onboarding copy.
- **Evidence:** performance harness and results with two fixes (`c539808`, `ae95240`), guarded
  native walkthrough (`4ad9438`), no-Node Linux package candidate with installed diagnostics
  (`c8d005c`), parity ledger refresh at `4fc4060`.

Wave-2 branches (performance, no-Node packaging, import end-to-end wiring, paged subagent
inspection, provider-helper consolidation) are all merged; no worker branch is outstanding.

Not done: Claude subscription (reserved for Codex — not implemented or qualified: the adapter
refuses `anthropic` OAuth credentials, auth reports it unavailable, onboarding shows it
disabled); differential qualification harness; durable outbox; per-conversation ownership
transfer; Codex WebSocket session cleanup at conversation end; diagnostics-correlation
advantage; CI wiring of native package tests; macOS native build; owner-authorized live
provider checks; IME/screen reader/scale/suspend/cold-boot gates (unverified).

## Handoff verification

Earlier verification (2026-10-11, at the handoff commit `ae95240`, with the two performance
fixes): `cargo test --workspace --no-fail-fast` **1171 passed, 0 failed, 53 ignored** (measured);
clippy had one test-only warning, fixed in the same commit; `cargo fmt --all --check` clean.

At `4fc4060` (after merging import end-to-end wiring, paged subagent inspection,
provider-helper consolidation and no-Node packaging):

- Final workspace check (native-engine at the commit recording this line, after all merges): `cargo test --workspace --no-fail-fast` **1296 passed, 0 failed, 53 ignored**; `cargo clippy --workspace --all-targets` 0 warnings; `cargo fmt --all --check` clean (after formatting one merged file). Measured 2026-10-11. The memory investigation (ledger row E2E-37) was stopped unfinished on the owner's instruction to wrap up; its partial branch `worktree-agent-a88568455d7803b4a` is not merged and the row stays failed.
