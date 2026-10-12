# Remaining parity reconciliation

Code identity: **`e4c3a8c`**, branch `native-engine`, 2026-10-11. Reference identity remains
`d2a311097cbcf669e699479587332ae3988a49d0` plus the two pinned Pipkin overlays in the
[ledger](../pipkin-rust-engine-parity.md). This is a source/evidence reconciliation, not a new
full reference execution or a qualification run. The execution plan was not changed.

**Subsequent implementation:** plugin-profile import protection and durable retries are now
implemented; the real v1 → v2 migration/backup is tested. See
[follow-up evidence](import-protection-durable-retries.md) and the current ledger. The findings,
counts and recommended batch below are the historical reconciliation at `e4c3a8c`, retained
so the record is not rewritten as if these features already existed then.

## Result and scope

Claude subscription integration is committed and its requested lifecycle checks passed:
browser and copy-code login, saved reuse after restart, forced-expiry live refresh, a real
coding turn, imported fixture continuation, Stop and reopen. The missing paste field and
misleading post-login management message are fixed. See [Claude evidence](claude-subscription.md)
and its sanitized lifecycle JSON for actual results, earlier failures and acceptance limits.

The required source checks passed: **1304 tests, 0 failed, 53 ignored**;
`cargo clippy --workspace --all-targets` and `cargo fmt --all --check` passed. These checks
precede this documentation-only reconciliation. Ignored tests were not treated as passing.
Historical performance/package/platform observations retain their original build identities.

The ledger still contains **503 rows; 0 qualified**. Evidence labels now total **415 measured,
7 observed, 1 failed, 11 unverified, 69 not started**. Target dispositions are NC 401, NE 23,
PA 33, NP 30, BL 15, LB 1, RC 0. A measured row may cover part of a capability or demonstrate
a known mismatch; it is not a completed-work count. NP rows retain their baseline exclusions.

Review covered stale progress/ADR implementation claims, ledger gaps and unresolved dispositions,
source/test assertions for the corrections below, and the native release workflow. It did not
rerun every provider against Pi or prove every existing test sufficient. Pending reachability
questions remain PA; the old `../pi-fork/pi` reference checkout is absent on this machine.
Existing frozen fixtures and pinned audit citations remain available.

## Corrections supported by current code

Paths below are relative to the repository; test names are exact unless explicitly a module.

| Stale claim | Current evidence and correction |
| --- | --- |
| Tool rounds always run in parallel (GEN-03) | `runtime/run.rs::tool_round` serializes the whole round if any offered tool is sequential. `runtime/tests.rs::tool_rounds_run_in_parallel_unless_a_tool_is_sequential` asserts both orders. A reference setting override still needs comparison. |
| Retry disabled/override/cap untested (SET-RETRY-*) | `runtime/fault_tests.rs::whole_turn_retries_stop_at_max_retries_with_exponential_backoff` sets maxRetries=2, baseDelayMs=60, asserts three attempts and at least 60/120 ms gaps, then asserts one attempt with retries disabled. `provider/errors.rs::classification_matches_the_reference_patterns` asserts the 60000 ms cap at attempt 40. A runtime configured-cap boundary remains untested. |
| Durable retry needs an owner decision (REC-RETRY) | The existing restart test deliberately proves that delay and count reset. This is missing required behavior under the no-feature-loss instruction, not an accepted waiver. |
| Native/demo refusal untested | `pipkin-app/src/controller.rs::the_engine_is_pi_unless_native_is_chosen_explicitly` asserts native + demo is rejected. FLAG-DEMO-SPEED now records parser coverage only. |
| Codex WebSockets never use proxies | `provider/api/openai_codex/tests.rs` covers HTTP-proxy CONNECT, NO_PROXY, rejected tunnel fallback and unsupported proxies. ADP-CODEX-WS now points to ADP-PROXY for the narrower remaining HTTPS-proxy/TLS gaps. |
| Shared HTTP idle timeout untested | `provider/http.rs::the_idle_timeout_fails_a_stalled_body_and_zero_disables_it` covers the client; `runtime/settings_tests.rs::the_request_timeout_follows_provider_timeout_then_http_idle_timeout` covers precedence. Complete per-adapter timeout parity is still open. |
| Missing disk-full/read-only, unsafe-tool crash, disabled/mismatched subagent recovery tests | `runtime/fault_tests.rs` covers storage faults at admission, progress/finalization, tool-result and settlement boundaries; child recovery and Stop races. `tests/crash_points.rs` uses actual worker SIGKILL at admission, streaming, tool execution, Stop and child-run boundaries, and tests damaged-database refusal. These were already represented in ledger rows but stale in progress/ADRs. |
| Missing `all` queue delivery and compaction override tests | `runtime/settings_tests.rs` tests both `all` modes, one-at-a-time steering, compaction overrides, enabled-model scope and thinking-level settings. Progress and ADRs now agree with the ledger. |
| Desktop worker/extension host/auth are skeletons; Claude unavailable | ADRs 0001–0007 and their index now reflect implemented supervisor/worker, native extension hosts, tools, contract checks and auth. TypeScript parity remains blocked. |
| Old schema/protocol/test names | Native protocol is 2; engine schema is 1; desktop schema is 8. PCFG-10 now cites `anthropic_env_rules_and_subscription_token_precedence`. Native package CI is absent, but workspace CI already includes native tests. |

Engine paths in this table are under `crates/pipkin-engine/src/` or
`crates/pipkin-engine/tests/`; protocol accounting is under `crates/pipkin-engine-protocol/`.

## Remaining implementation work, in priority order

### A. Migration and durable correctness

- **IMP-PLUGIN-STATE / EXT-PROFILE-***: importer code reads session storage and config but
  does not discover server/session plugin profiles. A facet-dependent conversation is therefore
  not recognized as such. Add profile discovery, preserve selection and refuse unsupported
  activation before transferring ownership. ADR 0006 describes the required refusal, not an
  existing safeguard. Test sticky selection, explicit empty selection and conflicting profiles.
- **REC-RETRY / GEN-07**: persist retry attempt and deadline with the run; resume the remaining
  wait and preserve the attempt budget. Replace the current difference assertion with parity
  assertions, including Stop during the restored wait.
- **XPORT-OUTBOX**: runtime publishes in memory through unbounded channels; snapshots repair
  gaps but there is no durable acknowledged cursor, paged event replay, bounded/coalesced progress
  or serialize-once fan-out. Implement these and prove slow clients cannot starve Stop or grow
  memory indefinitely. Keep auth secrets outside the durable event log.
- **PRM-PATCH**: runtime constructs a flat session system prompt; positional system entries,
  minimal patches and tool add/remove anchors are not generated. Imported `pi.system` entries
  retained in kv do not establish equivalent next-request behavior.
- **GEN-17 / USE-LEDGER**: protocol `Usage` lacks reasoning and cacheWrite1h counters. The
  internal 1h-cache cost multiplier does not preserve the separate counter. Carry both through
  adapters, storage, import, aggregation and views, with appropriate schema/protocol review.
- **REC-ORPHAN / CTX-EDIT-REPLACE / FORK**: generic task-definition recovery is absent;
  unsupported imported in-flight work, replace edits and forks are refused. Preserve those
  explicit refusals until equivalent continuation exists; refusal is continuity on Pi, not
  successful conversion of those histories.

### B. Provider and configuration completeness

- **ADP-ANTHROPIC-FEDERATION**: workload-identity federation exchange is refused, not ported.
  **ADP-VERTEX** lacks external-account/impersonated ADC and metadata-server fallback.
  **ADP-BEDROCK** refuses SSO and MFA role profiles. Keep clear errors while implementing the
  missing credential paths and their request fixtures.
- **ADP-CODEX-WS**: pooled sockets close at engine shutdown, not per conversation.
  **ADP-PROXY**: plain HTTP is forwarded rather than tunneled; HTTPS-proxy WebSocket tunneling
  is absent; WSS-in-tunnel and TLS-to-proxy fixtures are missing. Sign-in/refresh proxy errors
  also differ. HTTP-proxy WebSocket support is already implemented and should not be redone.
- **ADP-TIMEOUT / SET-IDLE**: finish per-adapter stalled/zero-timeout comparisons, HTTP-version
  behavior, project/global timeout semantics and invalid-value handling. **PCFG-14** still has
  minimatch/extglob and locale-order differences. **SET-COMPACTION-OVERRIDES** differs in
  invalid-value handling and applies classic behavior where the durable reference ignores it.
- **ADP-ATTRIBUTION / SET-TELEMETRY / ADP-UA** and **PCFG-11 / ENV-OFFLINE**: attribution,
  provider identity and remote catalog refresh need the recorded product decisions plus
  implementation/tests. Embedded pinned model data already works.
- **GEN-11, ADP-OR-IMAGES, ADP-CLASSIFIERS, PRV-typesafe, SET-CACHEWARMING,
  SET-SHELLPATH** retain pending reachability audits. Do not silently reclassify a working
  baseline capability out of scope to close the ledger.

### C. Extensions, services and desktop compatibility

- **EXT-FACET-*, EXT-PROFILE-*, EXT-CONFLICT, EXT-RELOAD, SVC-PLUGINS-*,
  SVC-SESSION-PLUGINS-RELOAD, E2E-34/35, FLAG-PI-EXTENSION**: native Rust/process extension
  ports exist, but existing TypeScript facets do not run natively. Sticky/per-session
  selection, conflict handling, custom remote services, hot reload and `waitForPrompt` remain.
  Port the baseline capabilities/packages and their desktop question workflows; retain Pi
  during migration. The no-Node conversion gate is not satisfied by a Node bridge.
- **MULTI-CLIENT / E2E-08/17 / FLAG-PI-DIR / FLAG-PI-SERVER-ID**: an in-process second client
  is tested; an owner-authenticated external native endpoint and its lifecycle are absent.
- **SVC-MGMT-REMOVE, SVC-MODELS-CYCLE, SVC-AGENT-WAIT, XPORT-CANCEL, Q-WRITES**:
  preserve PA status until desktop/facet reachability is resolved. Generic removal, request
  cancellation and write submissions have no equivalent call today.
- **FLAG-PI-AGENT-DIR** is refused in native mode instead of selecting an import source;
  **IMP-ROLLBACK** does not label native-only history item by item. Ownership transfer itself
  is implemented: Pi is read-only until explicit release. D-11 is resolved.

### D. Missing assertions and acceptance checks

The following remain tests/observations to add; code existence alone cannot close them:

| Area | Remaining assertions |
| --- | --- |
| Recovery/storage | DUR-MIGRATE-BACKUP: a real non-empty older native schema upgrade/backup; physical power-loss separately. OUT-RECOVERY: retained partial tool output after SIGKILL (the existing crash command writes a marker file, not stdout, so it does not prove this). |
| Stop/compaction | STOP-RACE: a completed run stays completed when Stop arrives late; STOP-UNSTARTED: later calls in a stopped sequential round get aborted results. STOP-BACKGROUND: threshold compaction survives Stop; SVC-AGENT-ABORT/CTX-MANUAL: manual compaction abort; CTX-SUMMARY-RESULT/PLACEMENT: failed-summary retry/usage and stale-summary discard; SET-COMPACTION-ENABLED: disabled behavior. The child-finished Stop test is not the general completed-run assertion. |
| Subagents/accounting | SUB-PERMITS: bounded-permit progress under waiting parents (children currently cannot delegate); SUB-RESULT: failed-child text; USE-TOTAL: parent/child aggregate usage without duplication. |
| Auth/providers | OA-COPILOT-REFRESH and OA-RADIUS-REFRESH; ONB-REMOVE concurrent-removal race; non-Claude live provider acceptance. Claude: natural expiry, other supported models, installed interactive paths and copied private production history remain unverified; forced expiry and a frozen imported Pi fixture passed. |
| Settings and prompt | Configured runtime retry cap; provider retry/delay, transport and thinking-budget pass-through; WebSocket connect timeout; PI_CACHE_RETENTION env path; unoffered-tool runtime error (GEN-04); PRM-THROWING-SECTION failure preservation where reachable; RenameSession mutation (P05-RENAME), beyond existing title mapping. |
| Desktop and reference | Native editor/terminal flag workflows, extension question UI lifecycle, E2E-36 inspection fixture, ignored read/edit reference corpora, and the full native-vs-Pi workflow/provider-context differential. E2E-36 is a helper, not a capability proof. |

The 11 rows whose primary evidence label is still `unverified` are OA-COPILOT-REFRESH,
OA-RADIUS-REFRESH, DUR-MIGRATE-BACKUP, STOP-RACE, STOP-BACKGROUND, SUB-PERMITS, USE-TOTAL,
PRM-THROWING-SECTION, OUT-RECOVERY, SET-WS-TIMEOUT and FLAG-EDITOR-TERMINAL. Other missing
assertions remain inside `measured (part)` rows; this list is not the entire backlog.

### E. Performance, release and platform gates

- **E2E-37 remains failed:** recorded headless soak grew approximately 51 KiB/turn to
  102.5 MiB, with flat descriptors/threads/processes. Diagnostics suggested mostly allocator
  retention, with a residual unexplained component. No new run supersedes that result; the
  incomplete investigation branch is unmerged. Desktop soak and large-reply peak remain.
- **RE-21**: build the full backend-parameterized differential harness and compare next
  provider requests as well as visible output. Existing import/golden comparisons are useful
  but not an end-to-end generation qualification matrix.
- **PKG-CI**: add native package/install qualification to `.github/workflows/qualification.yml`.
  Workspace CI already executes non-ignored native tests. The release/package job still builds Pi.
- **PKG-PKGBUILD, PKG-NOTICES, PKG-SIZE, PKG-ENGINE-STAGE**: native Arch packaging, the
  documented yaml-rust2 notice gap, a clean identified reference package and comparable package
  reduction measurements remain. A locally tested native Linux archive is already available;
  this report does not remeasure it or claim the provisional reduction targets passed.
- **PLAT-X11/MACOS/LINUX/PERF**: native Xwayland/macOS checks, installed native interactive
  qualification, IME, screen reader, display scale, suspend/resume, cold boot and actual display
  presentation latency remain unverified. Windows remains outside the baseline scope.
- **D-01..D-10** retain their recorded prompt/identity/catalog/login/input-limit/cache/spill
  decisions and provider-count audit. D-11 is resolved. Do not confuse a pending product
  choice with permission to drop a baseline feature. Default cutover and Pi retirement wait
  for the qualification, compatibility and measured-advantage gates.

## Recommended next batch

Start with plugin-profile import detection/refusal and durable retry state. They directly affect
whether imported conversations retain their behavior and whether restarting respects an already
admitted run. Follow with bounded durable event delivery and the reproducible memory-soak
investigation, then the remaining provider/context/compatibility work and qualification gates.
