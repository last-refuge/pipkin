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

## Current follow-up — desktop byte pressure and Stop

Bounded provider progress is committed as **`baf8eff`**. Native desktop events now use
128-event / 8 MiB owned-payload admission, charged until the controller applies each batch.
One indivisible oversized event is admitted alone without losing its contents. Stop uses a
separate bounded worker and preserves submission/connection ordering; shutdown releases
blocked UI producers. See [desktop pressure evidence](pipkin-rust-engine-evidence/desktop-pressure.md).

The real engine/core/storage tests prove durable Stop before UI consumption, exact saved
history and journal recovery with/without restart, and shutdown under byte pressure. Required
checks: **1404 passed, 0 failed, 53 ignored**, Clippy and formatting clean. Logs:
`/tmp/pipkin-ui-pressure-{workspace,clippy,fmt}.log`. No native-window soak was run.
Ledger counts remain **503 rows; 423 measured, 7 observed, 0 currently failed, 10 unverified,
63 not started; 0 qualified**. Lifecycle/catalog/request admission and total heap limits remain
separate work. Next: extension/profile compatibility and remaining provider/history gaps,
while completing the broader qualification gates. Native remains opt-in.

## Previous follow-up — bounded provider progress

Shared event delivery is committed as **`e87809f`**. The provider generation loop no longer
uses an unbounded stream-event channel. An engine-owned sink commits bounded batches directly,
with 40 ms flushing for small/silent streams and cancellation on checkpoint failure. Text is
split losslessly; indivisible oversized tool identities commit immediately. Late callbacks
cannot mutate completed entries or retain the engine. See
[provider progress evidence](pipkin-rust-engine-evidence/provider-progress.md).

Focused burst/Unicode, count/byte backlog, stale callback, failure, silent-stream and synchronous
producer Stop regressions pass. Required checks: **1395 passed, 0 failed, 53 ignored**;
Clippy and formatting clean (`/tmp/pipkin-progress-final-{workspace,clippy,fmt}.log`). The two
release streaming benchmarks passed: no missing markers, all cadence p95 values below 50 ms,
and all 24 scaling cases produce the expected output lengths. CPU/frame growth is approximately linear;
64-byte chunks show extra write amplification, recorded as a remaining performance concern.
Raw evidence: `fixtures/native-bench/2026-10-11-provider-progress/`.
At that revision, frontend bus byte/control-pressure work remained next; this change does not cap the full
assistant message or every parser allocation. Ledger counts remain **503 rows; 423 measured,
7 observed, 0 currently failed, 10 unverified, 63 not started; 0 qualified**. Native remains opt-in.

## Previous follow-up — shared event delivery

External attachment is committed as **`cb98d0a`**. The next change shares one immutable typed
and encoded event payload across subscriber queues. Private pipes and external sockets write
the cached body with a connection-specific envelope; in-process consumers retain typed frames.
Oversized encoding stops at the presentation budget and becomes a resync marker. Subscriber
pressure, reply barriers, auth scoping and per-client sequence numbers are preserved.
See [shared delivery evidence](pipkin-rust-engine-evidence/shared-event-delivery.md).

Initial focused and workspace tests passed. Clippy identified a large output-enum variant;
boxing its reply arm keeps the shared event wrapper small. Final required checks passed:
**1389 passed, 0 failed, 53 ignored**, Clippy and formatting clean. Logs:
`/tmp/pipkin-fanout-final-{workspace,clippy,fmt}.log`. The unchanged 1600-turn headless soak
passes the 100 MiB budget: **29,164 KiB idle / 39,912 KiB sampled peak**, 13 idle fds,
one process. Raw measurements are under `fixtures/native-bench/2026-10-11-shared-events/`.
Latency/CPU were higher than the older positional run; the evidence records both and does
not infer a speed improvement from unmatched runs. Counts remain **503 rows; 423 measured, 7 observed,
0 currently failed, 10 unverified, 63 not started; 0 qualified**. Frontend/provider-internal
queue budgets, extension compatibility, transport differentials and release gates remain open.

## Previous follow-up — external native attachment

Implemented on top of **`e738037`**, protocol **6** / schema **4**. A separately owned
foreground native server exposes an explicit private Unix endpoint. Both sides verify the
OS user; clients cannot shut down the server. Bounded handshake/client capacity, shared
runs/questions, disconnect, reconnect and stale-socket recovery are covered. The desktop
supports `--engine-attach` and endpoint-specific state; `--engine-server` runs without GPUI.
See [attachment evidence](pipkin-rust-engine-evidence/external-attachment.md) and
[user commands](../docs/native-attachment.md).

Required checks: **1385 passed, 0 failed, 53 ignored**, Clippy and formatting clean.
Logs: `/tmp/pipkin-attach-final-{workspace,clippy,fmt}.log`. The first full run exposed
store lock retention through inherited descriptors; a failing deterministic regression
reproduced it, and explicit owner unlock fixes both store and endpoint lifetimes. Failed
runs remain recorded in the attachment evidence. Counts: **503 rows; NC 404, NE 25,
NP 30, PA 31, BL 12, LB 1; 423 measured, 7 observed, 0 currently failed, 10 unverified,
63 not started; 0 qualified**. Pi-specific attachment flags have explicit native equivalents,
not literal translations. Linux scripted/process evidence does not qualify live GUI, macOS,
other OS users, transport differentials or performance. Native remains opt-in.

## Previous follow-up — native session attachments and retirement

Implemented on top of **`c3134df`**, protocol **6** / schema **4**. OpenSession now holds
client attachment demand; ReleaseSession and disconnect release it without cancelling work.
Idle retirement releases provider pools by affinity ID, Codex fallback/debug state, prompt
caches, questions and extension tasks. In-flight work holds retirement; short disconnects
allow readoption. Old extension generations cannot write into new ones, and reopening waits
for prior teardown. The desktop releases its previous conversation when switching.
See [lifecycle evidence](pipkin-rust-engine-evidence/session-lifecycle.md).

Required checks: **1371 passed, 0 failed, 53 ignored**, Clippy and formatting clean.
Logs: `/tmp/pipkin-lifecycle-release-{workspace,clippy,fmt}.log`. The first workspace run
found a device-login mock race, now made deterministic; the second found a protocol fixture
pinned to version 5, now updated. Failed logs are retained in the evidence record. Final
review also made first provider identity assignment atomic and contained deactivation panics.
Counts remain **503 rows; 420 measured, 7 observed, 0 currently failed, 10 unverified,
66 not started; 0 qualified**. External attachment/server re-adoption, full differentials,
extension package compatibility and release qualification remain open. Native remains opt-in.

## Previous follow-up — verified WebSocket proxy TLS

Constrained-tool integration is committed as **`1077005`**. Codex WebSockets now support
HTTP and HTTPS CONNECT proxies for WS and WSS targets. TLS to the proxy and TLS inside the
tunnel verify certificates independently. Tests cover all four combinations, percent-decoded
proxy authentication without forwarding it to the target, NO_PROXY, connection reuse/close,
timeouts/cancellation, and refused/malformed/oversized proxy responses. Verification also
found and repaired grammar response conversion in the cached continuation path.
See [transport evidence](pipkin-rust-engine-evidence/websocket-proxy-tls.md).

Required checks: **1358 passed, 0 failed, 53 ignored**, Clippy and formatting clean. Counts remain **503 rows; 420 measured, 7 observed,
0 currently failed, 10 unverified, 66 not started; 0 qualified**. Remaining transport work:
conversation lifecycle cleanup, shared-client HTTP CONNECT parity, auth proxy error parity,
external attachment and full transport differentials. Native remains opt-in.

## Previous follow-up — constrained tool declarations and execution

The positional context/paging batch is committed as **`ea0a98b`**. The next batch carries
`constrainedSampling` through native tool specs and durable declarations. Strict JSON-schema
sampling now reaches the applicable adapters; Responses, Azure and Codex support grammar
custom tools, declared argument properties, paired replay, and interrupted-input preservation.
Fifteen declaration cases match executed pinned Pi helpers. A native runtime test executes a
custom grammar tool and verifies durable replay after restart. See
[constrained-tool evidence](pipkin-rust-engine-evidence/constrained-tools.md).

Required checks: **1353 passed, 0 failed, 53 ignored**, Clippy and formatting clean. Counts remain **503 rows; 420 measured,
7 observed, 0 currently failed, 10 unverified, 66 not started; 0 qualified**. This closes
specific implementation gaps within already measured adapter rows, not full release qualification.
Native remains opt-in; extension compatibility and the other release gates remain open.

## Previous follow-up — positional system context

Implemented on top of **`7872c91`**. Protocol **5** / schema **4** preserve ordered system
sections and tool declarations in durable history. Runtime commits minimal patches and
compaction baselines; imported systems now participate in context equality and replay.
Provider encoding retains supported positions/tool anchors or collapses the current state.
Ten pure planner/replay cases match output captured by executing the pinned Pi reference.
See [context and paging evidence](pipkin-rust-engine-evidence/positional-system.md).

Verification also reproduced and fixed the subagent inspector paging race: an identical live
page no longer cancels an older page in flight. Hidden system-only pages advance the raw cursor.
Required checks: **1346 passed, 0 failed, 53 ignored**, Clippy and formatting clean.
The repeated 1600-turn Linux/glibc headless soak passes at **28,940 KiB idle / 40,148 KiB peak**,
with 13 idle fds and one process. Desktop/platform/performance qualification remains open.

Ledger: **503 rows; 420 measured, 7 observed, 0 currently failed, 10 unverified,
66 not started; 0 qualified**. PRM-PATCH is measured with bounded differential evidence;
provider/extension/installed acceptance is not inferred from those pure cases. Native remains opt-in.

## Previous follow-up — usage subset preservation

Implemented on top of **`e8993de`**. Protocol, adapters, persistence/import, aggregate views,
core and both desktop adapters now retain optional `reasoning` and `cacheWrite1h` counts.
The usage panel shows them as subsets; costs and total tokens do not double count them.
The existing Codex reference-golden comparisons now assert those fields too.
See [usage evidence](pipkin-rust-engine-evidence/usage-subsets.md).

Required checks: **1334 passed, 0 failed, 53 ignored**, Clippy and formatting clean.
Ledger counts remain **503 rows; 419 measured, 7 observed, 0 currently failed, 10 unverified,
67 not started; 0 qualified**. GEN-17 / USE-LEDGER retain partial qualification status, but
no longer have the missing-counter implementation gap. Positional system prompt updates,
extension compatibility, provider/external attachment gaps, and release qualification remain.

## Previous follow-up — durable delivery and memory reclamation

The prior import/retry batch is committed as **`63bd9ae`**. This follow-up (2026-10-11)
implements protocol **4** / schema **3**: transactionally recorded invalidations,
acknowledged bounded catch-up pages, explicit retention resets, bounded per-client and
reader presentation, separate reply/control capacity, and serialized commit/publication.
Catch-up preserves paged history and the selected subagent; a missed final event is recovered
without requiring subsequent live traffic. A real SIGKILL before publication is covered.

Linux/glibc private workers now reuse one allocator arena, keep large allocations releasable,
and trim free pages before settlement. The unchanged 100 + 1500-turn headless soak went from
**114,804 KiB idle / 116,692 KiB sampled peak** to **27,456 / 39,840 KiB**. Second-half
slope fell from **54.848 to 1.440 KiB/turn**; idle fds stayed 13 and there was one process.
This fixes the measured 100 MiB budget failure, not every desktop/platform memory gate.
See [implementation, tests and raw measurements](pipkin-rust-engine-evidence/durable-delivery-memory.md).

Final required checks: **1331 passed, 0 failed, 53 ignored**, Clippy and formatting clean.
Ledger: **503 rows; 419 measured, 7 observed, 0 currently failed, 10 unverified,
67 not started; 0 qualified**. Both new measured rows are explicitly partial. Historical
failures remain in their evidence records. Dispositions remain NC 402, NE 23, PA 33,
NP 30, BL 14, LB 1, RC 0. Native remains opt-in; Pi remains the default.

## Historical follow-up — plugin import protection and durable retries

Implemented on top of `92629ec` (2026-10-11). See
[import/retry evidence](pipkin-rust-engine-evidence/import-protection-durable-retries.md).
Plugin profiles are now validated during discovery and rechecked before import activation;
unsupported selections stay on Pi, without a native session or ownership transfer. Native
retry deadlines, handled error entries and failure counts survive restart; Stop cancels the
restored wait. Engine schema **2** adds generation state with a tested private v1 backup;
protocol **3** adds explicit import `serverDir` and prevents an older worker from accepting
an import without the new guard.

Ledger: **503 rows; 417 measured, 7 observed, 1 failed, 10 unverified, 68 not started;
0 qualified**. Final required checks: **1319 passed, 0 failed, 53 ignored**; Clippy and
formatting passed. Earlier attempts, including a subagent-inspector timeout, are retained in
the evidence record. NC 402, NE 23, PA 33, NP 30, BL 14, LB 1, RC 0. Import protection is
measured refusal, not TypeScript execution compatibility. The next priorities are bounded
durable event delivery and the unresolved memory-soak failure; full qualification remains open.

## Historical reconciliation — 2026-10-11, `e4c3a8c`

Claude subscription integration and its two observed UI fixes are committed in **`e4c3a8c`**.
Browser and copy-code sign-in, saved-credential reuse after restart, a forced-expiry live refresh,
a real Opus 5.5 coding turn, and imported-Pi-fixture continuation/Stop/reopen passed. Copy-code
login initially exposed a missing input mapping; successful connection then exposed misleading
management-page copy. Both are fixed. The final restarted desktop returned
`COPY_CODE_RESTART_OK` using saved Anthropic OAuth.
[Detailed evidence](pipkin-rust-engine-evidence/claude-subscription.md) retains failed attempts
and distinguishes harness errors, actual fixes and unverified cases.

The remaining ledger was reconciled against code and assertions; see
[remaining work and corrections](pipkin-rust-engine-evidence/parity-reconciliation.md).
It has **503 rows: 415 measured, 7 observed, 1 failed, 11 unverified, 69 not started;
0 qualified**. Dispositions: NC 401, NE 23, PA 33, NP 30, BL 15, LB 1, RC 0.
These labels include partial evidence and tests demonstrating known differences, not 415
completed capabilities. Required source checks passed: **1304 tests, 0 failed, 53 ignored**,
Clippy, and formatting. The subsequent reconciliation changes documentation only.

The native engine remains opt-in; Pi remains the default. Actual gaps include plugin-profile
import detection, durable retry state, outbox/backpressure, usage counters, positional system
prompt updates, external attachment, TypeScript facet compatibility and the memory-soak failure.
Historical sections below retain their original evidence; this summary, sections 1–3 and 7–8
are the current interpretation. No default cutover or parity qualification occurred.

## 1. Architecture decisions

Full records: `llm-docs/pipkin-rust-engine-adr/` (index in its `README.md`).

| ADR | Decision (summary) | Status |
| --- | --- | --- |
| 0001 Execution topology | One shipped `pipkin` executable; a private worker mode runs the GPUI-free `pipkin-engine` as an **owned child** over inherited pipes; one engine process for all conversations; in-process host (same API) for tests/headless/benchmark; not a sandbox | Accepted; implemented (`--engine-worker`, `App/adapters/native/supervisor.rs`, headless `pipkin-engine` binary) |
| 0002 Protocol and transport | Length-prefixed JSON frames (32 MiB max); typed `Call`/`Reply`/`Event`; `epoch`+`seq` events, session `revision`, snapshot on gap; control lane for Stop/Shutdown; protocol version mismatch refused | Accepted; protocol **5**, positional system declarations, durable invalidation outbox/acknowledged catch-up and bounded delivery implemented; serialize-once fan-out/external attachment open |
| 0003 Durable store and recovery | One OS-locked SQLite store per data root; explicit schema v4, backup-before-migrate, newer refused untouched; short transactions; state-driven run loop; fail closed on storage error; `synchronous=FULL` (reference `NORMAL`) | Accepted; implemented; actual worker SIGKILL and injected disk-full/read-only cases tested; v1 → current, v2 → v3 and v3 → v4 backup/migration tested; power loss unverified |
| 0004 Stop and queue | Reference queue boundaries and outcomes; durable Stop intent then cancellation; retirement by `entries.retired` flag instead of `pi.cancelled-input` omit-edit entries | Accepted; implemented; `all` delivery modes tested; differential qualification and general Stop-after-completion race pending |
| 0005 Tool replay and subagent recovery | Intent before effect; unsafe-by-default replay; `subagent` is the replay-safe registration with an atomic `(parent session, call)` → child mapping and once-only delivery; recorded-contract check | Accepted; implemented incl. recorded-contract check; mismatch, disabled recovery and Stop-child tests pass |
| 0006 Extension compatibility | Native versioned extension contract + out-of-process protocol; existing TypeScript facet packages need Node, so their rows stay **blocked** and run on the **legacy bridge** (Pi) | Proposed; native contract implemented (`E/extensions`, `docs/native-extensions.md`); TS facets blocked |
| 0007 Credential ownership and migration | Own data/config roots; `auth.json`-compatible private file (0600) until a qualified OS secret store; metadata-only lookup; explicit import, no sync; Claude subscription OAuth and explicit import implemented; failed refresh never changes billing credentials | Accepted; implemented (`E/auth`, explicit credential import) |

## 2. Baseline inventory pointers

| What | Where |
| --- | --- |
| Reference audit (RE-01) | `llm-docs/pipkin-rust-engine-audit/` — `README.md`, `engine-tools-durable-usage.md`, `prompt-settings-layout.md`, `service-api.md`, `extensions-subagents.md`, `providers-auth.md` |
| Parity ledger | `llm-docs/pipkin-rust-engine-parity.md` (503 rows; 0 qualified; current evidence: 420 measured, 7 observed, 0 currently failed, 10 unverified, 66 not started; partial evidence is explicit in each row) |
| Frozen reference fixtures (RE-02) | `fixtures/pi-sessions/` (8 scenarios a–h with `session.sqlite`, `requests.jsonl`, `view.json`, `history.json`; `BASELINE.md` for the reference measurements) |
| Evidence | `llm-docs/pipkin-rust-engine-evidence/` — `performance.md` (RE-22), `packaging.md` (RE-23), `native-walkthrough.md` (guarded GUI walkthrough) |
| Claude subscription handoff | `llm-docs/pipkin-rust-engine-claude-subscription-handoff.md` |
| Known reference defects | Parity ledger §19 (K-01..K-11) |
| Intentional improvements | Parity ledger §20 (I-01..I-13) |
| Open owner decisions | Parity ledger §21 (D-01..D-11), with the implemented state and open sign-offs; D-11 is resolved |
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
native tests (cited in the ledger), but the full qualification differential and applicable
platform/provider gates have not closed it; Claude has separate live evidence.

| ID | Work package | Owner | Status | Evidence pointer |
| --- | --- | --- | --- | --- |
| RE-01 | Pinned-source/overlay inventory and complete parity ledger | Opus | in progress | Ledger reconciled at `e4c3a8c`; open: 33 `PA` rows, provider-id count (D-09), owner decisions D-01..D-11 |
| RE-02 | Frozen behavioral fixtures and performance baseline | Opus | implemented-but-unverified | `b2ff206`: `fixtures/pi-sessions/` 8 scenarios + `BASELINE.md`; reference package/runtime artifact hashes still TODO (§4) |
| RE-03A | Non-reserved provider/auth feasibility and inventory | Opus | implemented-but-unverified | All non-reserved adapters and OAuth flows implemented and tested against local mocks (ledger §3–§6); no live provider call |
| RE-03B | Claude subscription feasibility review | Codex | reviewed; user-confirmed approval | `pipkin-rust-engine-evidence/claude-subscription.md`; user-confirmed approval; live access subsequently exercised in RE-13B |
| RE-04 | Context, durable-task and extension-state import spike | Opus | implemented-but-unverified | `8290556`; all 8 reference fixtures import and continue equivalently (`rti::reference_fixtures_import_open_and_continue_equivalently`); plugin profile protection now detects/refuses nonempty selections (IMP-PLUGIN-STATE) |
| RE-05 | Extension compatibility spike and ADRs | Opus | in progress | ADR 0006 still Proposed; native ports of the two baseline examples (`ext::`, `extp::`); TS facets blocked |
| RE-06 | Typed engine contract and private worker transport | Opus | in progress | Protocol v5; typed frame/import tests; durable outbox, acknowledged bounded catch-up, gap/reset and pressure tests implemented; external attach/fan-out and complete qualification remain |
| RE-07 | Engine library, host/supervisor and headless harness | Opus | implemented-but-unverified | `nsup::` (7), `ew::` (2), `wp::` (2: SIGKILL + second-writer refusal), headless `pipkin-engine` binary |
| RE-08 | SQLite store, migrations, lock, request ledger and outbox | Opus | in progress | `st::` (12), `wp::admission_survives_sigkill_and_is_found_by_lookup_after_restart`; durable invalidation outbox implemented; schema v4 with tested v1/v2/v3 upgrade backups; injected disk-full/read-only faults and damaged-database refusal pass (`rtf::`, `cp::`) |
| RE-09 | Durable scripted generation and NativeBackend | Opus | implemented-but-unverified | `rt::` (19), `ne2e::` (20 workflows), `nt::` (9); observed in `native-walkthrough.md` |
| RE-10 | Stop, queue, steering and recovery | Opus | implemented-but-unverified | Stop/queue/steer/restart measured (`rt::`, `ne2e::`); `all` modes tested (`rts::`); durable retry wait/count and Stop across restart now pass (REC-RETRY); general Stop-after-completion race remains |
| RE-11 | Coding tools and execution environments | Opus | implemented-but-unverified | `5768dda`; `trd::`/`twr::`/`ted::`/`tbs::`/`tout::`/`trun::`/`tsch::` incl. reference-captured goldens; corpus tests ignored with no recorded run; sequential-tool rounds tested (`rt::tool_rounds_run_in_parallel_unless_a_tool_is_sequential`); reference setting override needs comparison |
| RE-12 | First provider and real coding slice | Opus | implemented-but-unverified | Walkthrough §3 (real `write` tool via an `openai-completions` custom provider to a local stub); perf harness uses the same path; Claude live coding slice now passed (RE-13B) |
| RE-13A | Generic credentials and non-reserved providers | Opus | implemented-but-unverified | `dd43920`, 10 wire adapters + `provider/api/common`; `auth::`, `oauth::`, adapter suites; Codex adapter goldens are reference captures |
| RE-13B | Claude subscription integration | Codex | implemented; live smoke passed | `pipkin-rust-engine-evidence/claude-subscription.md`; browser/copy-code sign-in, refresh, saved reuse and Pi-fixture Stop/reopen passed; full qualification pending |
| RE-14 | Context/resources, attachments, models and accounting | Opus | implemented-but-unverified | `384da75`, `e4fe919`; `drv::`/`prm::`/`res::`/`cmp::`; usage subsets and positional system patches now preserved and tested; see follow-up evidence |
| RE-15 | Durable subagent/task scheduler and recovery | Opus | implemented-but-unverified | `rt::subagents_run_once_per_call_deliver_their_answer_and_are_listed`, `rt::a_subagent_call_interrupted_by_a_restart_resumes_its_one_child`; disabled/contract-mismatch and Stop-child recovery tests pass (`rtf::`); actual child-run SIGKILL passes (`cp::`) |
| RE-16 | Native extension compatibility implementation | Opus | implemented-but-unverified (native) / blocked (TS facets) | `01e0511`, `e4fe919`; `--engine-extension`; TypeScript facet packages stay `BL / LB` (ADR 0006) |
| RE-17 | Multi-session engine, history and child presentation | Opus | implemented-but-unverified | `7ba1340` paged subagent inspection (`core-sub::`, `ui-sub::`, `nt::child_history_pages_by_cursor_and_the_selected_child_refreshes_live`); history paging flat after fixes (perf-ev) |
| RE-18 | Compaction, questions and remaining audited services | Opus | implemented-but-unverified | `rt::manual_and_overflow_compaction_summarize_older_context`, `rt::extension_questions_are_published_answered_and_validated`; background/threshold runtime paths untested; `waitForPrompt`, `RemoveSession` absent (PA) |
| RE-19 | Production importer and atomic backend ownership | Opus | implemented-but-unverified | `8290556`, `21ff99a` (protocol v2 `Import*`, desktop palette flow); desktop ownership record written in the import's copy transaction; Pi mode read-only for owned conversations (core + Pi adapter boundary), explicit release from the palette (D-11, IMP-OWNERSHIP) |
| RE-20 | Desktop integration including goals, offline state and Changes | Opus | implemented-but-unverified | `6b7980f`; `ne2e::`, `nmap::`; walkthrough observed onboarding, first conversation, Changes, SIGKILL recovery |
| RE-21 | Full parity and fault qualification | Opus (+ Codex for reserved rows) | planned | Ledger refreshed; no backend-parameterized differential harness; storage-fault injection and actual worker crash boundaries pass; full differential, power-loss and remaining ledger assertions are open |
| RE-22 | Integration advantages and performance | Opus | in progress | `c539808`, `ae95240`; `performance.md`: most §7 targets met; headless 1500-prompt RSS budget now passes; desktop/streaming/platform qualification and diagnostics-correlation advantage remain |
| RE-23 | Native build, packaging, CI, support and rollback | Opus | in progress | `c8d005c`; `packaging.md` (Linux candidate verified by `scripts/test-native-package.sh`); not in CI; no PKGBUILD/macOS package |
| RE-24 | Beta and accepted default | Opus + owner, after Codex | planned | Blocked on full parity, remaining live/platform acceptance and performance/release gates; RE-13B lifecycle smoke passed |
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
| Native engine schema | **4** (`crates/pipkin-engine/src/store/schema.rs::SCHEMA_VERSION`) |
| Native protocol | **5** (`crates/pipkin-engine-protocol/src/lib.rs::PROTOCOL_VERSION`; v5 adds typed positional system declarations; v4 added durable catch-up; v3 guarded plugin-profile imports) |
| Native engine version | `0.0.1` (`pipkin_engine::runtime::ENGINE_VERSION` = crate version) |
| Reference protocol / app DB schema (unchanged) | Pi protocol 8; desktop `pipkin.sqlite3` schema 8 (`storage.rs` migration assertion) |
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

1. **Correctness and migration gaps:** positional system patches and reasoning/1h-cache counters
   are now preserved and tested, including imported system context. Plugin-profile import protection
   and durable retry waits/counts are implemented. Constrained/grammar tool
   support is now implemented with focused differential/runtime evidence. TypeScript execution
   and remaining unsupported-history conversion (replacement edits/forks) remain open.
2. **Transport and compatibility:** durable invalidation replay and bounded engine/reader delivery
   now have passing recovery/pressure evidence. External Unix attachment and serialize-once event fan-out are implemented;
   provider progress and native frontend event backlogs are bounded, with oversized frontend events admitted alone; Stop bypasses frontend delivery waits. Lifecycle/catalog and normal-request admission remain separate work. TypeScript facets, sticky profiles,
   remote services and reload still depend on Pi.
3. **Performance:** E2E-37's headless 100 MiB budget now passes on Linux/glibc; an indefinitely flat
   heap, desktop/long-tool/streaming soaks and other allocators/platforms remain unverified.
   Package reduction needs a comparable clean reference build; desktop latency gates remain open.
4. **Qualification:** no complete backend-parameterized differential harness; targeted test,
   live-provider, platform, installed-app and native-package CI gates remain. Claude lifecycle
   smoke has passed but is not full provider/platform qualification.
5. **Product decisions/reachability:** ledger D-01..D-10 remain open where stated; D-11 ownership
   transfer is implemented and resolved. A known missing baseline behavior is required work,
   not automatically a request to waive parity.

## 8. Next implementation batches

1. Completed this batch: plugin-profile import fails closed; retry count/deadline survive restart,
   with failure injection, actual SIGKILL and desktop ownership regression tests.
2. Completed next batch: bounded delivery, atomic invalidation outbox, acknowledged catch-up,
   commit/snapshot ordering, and the headless memory-budget fix; see delivery/memory evidence.
   Remaining transport and broader memory qualification boundaries are listed above.
3. Usage counters and positional prompt updates are implemented and tested. Constrained/grammar
   tool support is implemented with passing targeted/workspace checks. HTTPS proxy/WSS support and cached grammar continuation are now implemented with local TLS tests. Session lifecycle cleanup and external Unix attachment are now implemented. Serialize-once event delivery is implemented. Native frontend event byte/control pressure is implemented. Next: remaining shared-client proxy gaps, lifecycle/request admission and facet ports/profile lifecycle while
   keeping Pi as the continuity path.
4. Add the specific missing assertions in the reconciliation report; build the full native/Pi
   differential harness. Existing disk-full, child recovery and `all` delivery tests do not
   need to be reimplemented.
5. Wire native package/install qualification into CI, finish supported-platform and installed
   live checks, and remeasure release advantages before beta/default cutover and Pi retirement.

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

## Historical handoff state at `4fc4060` (superseded by current reconciliation)

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

## Historical handoff verification

Earlier verification (2026-10-11, at the handoff commit `ae95240`, with the two performance
fixes): `cargo test --workspace --no-fail-fast` **1171 passed, 0 failed, 53 ignored** (measured);
clippy had one test-only warning, fixed in the same commit; `cargo fmt --all --check` clean.

At `4fc4060` (after merging import end-to-end wiring, paged subagent inspection,
provider-helper consolidation and no-Node packaging):

- Final workspace check (native-engine at the commit recording this line, after all merges): `cargo test --workspace --no-fail-fast` **1296 passed, 0 failed, 53 ignored**; `cargo clippy --workspace --all-targets` 0 warnings; `cargo fmt --all --check` clean (after formatting one merged file). Measured 2026-10-11. The memory investigation (ledger row E2E-37) was stopped unfinished on the owner's instruction to wrap up; its partial branch `worktree-agent-a88568455d7803b4a` is not merged and the row stays failed.
