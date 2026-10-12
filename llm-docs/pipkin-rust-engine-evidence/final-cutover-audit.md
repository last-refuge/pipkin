# Native-default completion audit

This audit follows the owner's native-engine goal and explicit exclusion of Pi extensions.
It does not edit the owner's in-progress plan. The earlier build-path candidate is `cc8931c`;
the subsequent offline recovery fix is recorded below. Release publication, additional desktop
platforms and general 1.0 manual acceptance are separate from retiring the Pi dependency.

## Requirements and inspected evidence

| Required outcome | Authoritative evidence | Current conclusion |
| --- | --- | --- |
| Preserve required behavior; account for every ledger capability | `pipkin-rust-engine-parity.md`: 504 rows; 412 NC + 39 NE, 37 baseline exclusions, 16 explicit owner scope exclusions. Current test citations checked against Rust sources; historical Pi tests labelled as such. | All 451 required NC/NE rows have native measured or observed evidence. Final qualification stamp waits for the updated candidate below. Finite tests are not a proof of every provider/environment combination. |
| Providers, Claude subscription and account/billing identity | `claude-subscription.md`, provider fixtures, federation/ADC/AWS/proxy/catalog evidence, real worker secret-redaction capture; live browser/copy-code login, reuse, forced refresh, coding, Stop and reopen previously recorded | Required paths implemented; no silent API-key fallback. Concurrent remove/login now exercises a real held credential commit and exact Busy behavior. Unused optional provider logins retain their stated mock-only limits. |
| History/context, tools, configuration, usage and subagents | Executed generation/context/queue/model-scope/tool corpora, import fixtures, subagent accounting and recovery tests, compaction/resource evidence; current ledger citations | Required implementation retained. Added direct settings propagation/restart, compaction disabled/enabled and exact child-failure assertions close stale integration gaps. Source-derived assertions remain labelled; no claim of exhaustive live provider qualification. |
| Durable admission, unknown outcomes, Stop and failures | Durable-delivery, crash-points, fault tests, native end-to-end workflows | Lost actual Stop frame is checked/retried without resending its prompt; failed desktop journal admits nothing and retains the draft until explicit successful resubmission. Existing crash/unsafe-tool recovery evidence retained. |
| Offline saved history and reconnect | New native end-to-end cache/search/rejoin regression | Found a real gap: a selection made while offline was discarded, and reconnect sent Synced instead of the first Opened. Adapter now remembers latest selection/generation plus initial-open requirement; cached history becomes live without replay. Previously opened sessions retain refresh semantics. |
| Safe migration, identities, child tasks and extension metadata | Imported reference sessions, fork/orphan/profile evidence, installed migration and provenance labels | Source copies remain unchanged; unsupported extension code is never activated. Historical selection alone does not block import. Native-only history and prior ownership records remain readable/restorable. |
| No Pi runtime or automatic fallback | Cargo workspace/lockfile, app controller/native adapter; retired Pi adapter/client removed; default/retired-flag tests | Native default implemented. Same binary hosts the owned worker; explicit native host and in-process paths remain. There is no Pi runtime fallback. |
| No Pi/Node build or installed dependency | Rust-only setup/package/Arch/release/macOS scripts; CI traps; runtime/license inventory and installed package verifier | Obsolete launch/build/download scripts removed. Reference captures/patches/model snapshot/notices remain provenance only. Explicit rollback may reactivate a retained historical installation; current builds never provision it. |
| Performance and integration advantages | `paired-performance.md`, `server-allocator.md`, `sustained-compaction.md`, package comparisons | Defined paired gates pass; measured readiness/RSS/package gains, actual 1,500-turn desktop memory budget and 720-turn/232-compaction workload pass. No additional soak required. Warm-cache/Linux boundaries and unmeasured display latency remain explicit. |
| Supported installed desktop and live coding | `packaged-desktop.md`, `native-walkthrough.md`, Claude lifecycle evidence | Actual Arch/Omarchy Wayland coding/Changes/Stop/crash recovery/Quit and live Claude requests observed. Experimental Xwayland keyboard failure remains outside supported Wayland conversion acceptance. |
| Identified package, migration/resume, upgrade/rollback, backups and diagnostics | Clean `cc8931c` archive and `fixtures/native-default/2026-10-12-final` | All installed workflow checks pass, dirty=false identity recorded. Because the subsequent offline fix changes production code, rebuild and recheck its final binary before completion. |
| Required source checks | Full workspace after offline fix and seven targeted regressions: **1,446 passed, zero failed, nine ignored benchmarks**; clean all-target Clippy and formatting | Passed locally. Ignored benchmarks are not counted as fresh passes; their separately executed evidence is retained. |
| Hosted qualification | Earlier corrected Linux `38262069375` and macOS `38262827277` passed. Native-default build-path runs `38265023304`/`38265027473` are in flight. | Inspect terminal outcomes and qualify the final recovery-fix candidate; do not infer success from workflow presence or restart live jobs. |
| Remote backup | Existing `origin/codex/native-engine-qualification` branch; push every completed batch | Runtime and build-path changes backed up. Push this tested recovery fix immediately. Main/release tags unchanged. |

## Retained limitations

- No Pi extension compatibility is required or claimed. Frozen profiles and history are preserved.
- Pure X11, additional Linux desktops, interactive macOS/Windows, IME/Orca/display-scale,
  suspend/resume/cold boot, true power-loss guarantees and display-presentation latency keep
  their existing failed/unverified labels. They are not invented as conversion passes.
- Developer ID/notarization, signing/publication/update distribution and dependency legal
  review remain broader release work. The candidate is unsigned/unpublished, not a v1 release.
- Reference comparisons are finite. Extreme model glob/locale inputs and every live provider
  account combination are not exhaustively exercised. Explicit input bounds/refusals and
  documented native improvements remain visible; no missing required implementation is
  excused by calling it an improvement.

Completion remains unproven until the final binary and hosted checks above pass and their
identities/outcomes are recorded. The production code delta after `cc8931c` is the offline
read-only attachment fix plus native-engine display text; no new engine/schema changes.

## Updated clean local candidate

The recovery fix is pushed as **`4b42079e88474450f47409f11cae1ea227270fa3`**. Its clean
source checkout produces a **dirty: false** archive; installed migration/coding/restart,
actual upgrade and rollback against clean `cc8931c`, schema refusal, full archive restoration
and continued coding all pass. Node/npm/Pi traps remain unused. Exact identities and logs:
[`fixtures/native-default/2026-10-12-recovery-final`](../../fixtures/native-default/2026-10-12-recovery-final).
Resolved Cargo metadata confirms no transitive GPUI/UI/app/Pi-client dependency in the engine
or core and no Pi-client package anywhere in the resolved workspace.

The `cc8931c` macOS native app/ZIP run passed. The updated candidate's hosted runs
`38266456562` (Linux) and `38266459911` (macOS) remain queued/running at this recording.
These are the remaining candidate checks; do not rebuild unchanged code while they run.
