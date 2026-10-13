# Pipkin v1.0 release gates

## Working scope and baseline

Started **2026-10-06 UTC**, following the owner's request to execute
[`llm-docs/pipkin-v1-release-plan.md`](../llm-docs/pipkin-v1-release-plan.md).

Working scope: **x86_64 Arch/Omarchy, Hyprland, Wayland**. This is the plan's recommended initial scope, not a new claim of support for other distributions, compositors, or operating systems. Final platform/acceptance sign-off remains an owner decision.

Feature freeze: prioritize release defects, qualification, onboarding/support, and release machinery. Additional features belong in the post-v1 backlog unless required to close a blocker.

### Identified baseline

| Item | Value |
| --- | --- |
| Pipkin revision tested | `1e3d54c0e92b36de0aac97a8e77117238aecf582` |
| Pi engine revision tested | `d4c871ef75bea57f43518e5e554c5a4765173a0b` |
| Engine source state | Clean tracked working tree when baseline began; generated/ignored build inputs are not identified by this check |
| Application/package version | `0.0.1` |
| Database schema | 7 (`MIGRATIONS.len()` in `storage.rs`) |
| Rust | `rustc 1.99.0 (b940084d7 2026-09-28)` |
| Cargo | `cargo 1.99.0 (5f94df478 2026-08-27)` |
| Node | `v26.8.1` (qualification at the advertised minimum Node version remains open) |
| GPUI revision | `a84689073d296dfd39987bc7dd478e43ef76d83a` |
| Machine/display | Linux x86_64; Hyprland 0.56.2; reported active monitor 2560×1600 at scale 1.6 |

### Owner-reported beta evidence

The owner reports heavy daily-use beta testing, including using Pipkin to build itself and using it on other projects, and describes it as being in great shape. Duration, project count, crash/loss rates, and new-user onboarding results have not been quantified. This establishes useful owner-reported workflow evidence, not a measured general reliability rate or a clean-machine/platform pass.

### Automated baseline: passed on 2026-10-06

| Check | Result and boundary |
| --- | --- |
| `cargo test --workspace` | **505 passed, 36 ignored**, no failures. Ignored tests are not counted as passes. |
| `cargo clippy --workspace --all-targets` | Passed, no warnings reported. |
| `cargo fmt --all --check` | Passed. |
| Development-engine release e2e suite | **33 passed**, serial, using the identified adjacent engine checkout. Includes the default 40-prompt soak tripwire; not an extended/day-long soak. |
| `scripts/package.sh` | Built `dist/pipkin-0.0.1-x86_64.tar.zst` from this app/engine pair. |
| `scripts/verify-install.sh --full dist/pipkin-0.0.1-x86_64.tar.zst` | Scratch-prefix checks and **33 real-engine tests passed against the unpacked engine**. Empty HOME/minimal PATH; not a desktop launcher or clean-system GUI test. |
| `scripts/test-install.sh dist/pipkin-0.0.1-x86_64.tar.zst` | All 12 scratch-prefix checks passed: install, discovery/probe, simulated version upgrade, rollback, uninstall, and data-marker preservation. Not a real schema-changing app upgrade. |

Artifact SHA-256:

```text
7e426a7eb6eab92e38cf2f426c57e1ee6bde259d8ffec583438737fd610082c6  pipkin-0.0.1-x86_64.tar.zst
```

Measured size: tarball **89,114,048 bytes** (about 85 MiB); staged installation **453 MiB**; engine staging output **200 MiB**. These are current baseline measurements, not guaranteed final release sizes. The build is not signed and is not a release candidate.

Full command output was captured locally in `/tmp/pipkin-v1-baseline/{workspace,clippy,fmt,e2e,package,verify-install,installer}.log`. These logs are temporary and are not committed; the durable evidence summary is this document. Archive fresh logs/artifact checksums with CI and release candidates in Phase 2/6.

## First Phase 1 fix (2026-10-06)

Confirmed that Pi's current experimental session service has no rename operation. Real-mode Rename is now
disabled in the command palette; F2 reports “Renaming conversations is not supported by this Pi engine”
instead of opening a dialog that silently does nothing. Demo rename remains enabled. A regression test
checks both modes.

Post-fix validation: **506 workspace tests passed (36 ignored)**, Clippy and formatting passed. This is a
working-tree change on top of the identified baseline above, not a rebuilt/requalified release artifact.
The toast/disabled palette state has not yet been walked in the native installed UI.

## Changes scan hardening (2026-10-06)

Implemented on top of **`4a6e633ad82a00d1225ff3dc4dede33cc830fed9`**, as an uncommitted working-tree delta.
Engine remains **`d4c871ef75bea57f43518e5e554c5a4765173a0b`**, tracked working tree clean at recheck.
Database schema remains 7. Source delta identifier (`git diff --binary -- crates/ | sha256sum`):
`93e1662b58fa90d35813f530a9fa78a4771467113ce05ba68ee016b7b72b56ce`.
This identifies the tested delta, not a clean release revision.

Behavior:

- Fresh untouched conversations still begin with the calm empty inspector; automatic scanning waits for work.
- Loading, a successful empty scan, unavailable scans, stale retained diffs, and non-Git folders are distinct.
- Failed scans expose the reason and retain only that conversation's last successful diff, explicitly marked out of date.
- Inspector refresh, failure-state Retry scan, and the palette's Refresh workspace changes share core availability.
  Explicit refresh can scan an untouched session, is read-only, and cannot stack duplicate client requests.
- Backend scanning remains off the UI thread, single-flight with one coalesced follow-up; generation/current-session
  guards prevent an old attachment's result from appearing as another project's state.
- Reattachment does not erase the previous diff before a successful scan. Refresh preserves selected paths rather
  than an obsolete row index. A scan revision invalidates cached diff text even when line counts stay the same.
- Missing Git/failed process launch is no longer classified as a non-Git folder.

### Automated evidence

- **511 workspace tests passed, 37 ignored**; Clippy/fmt clean.
- **34 e2e suite tests passed** against the development engine, serial; the new regression confirms that a
  missing project does not erase its diff, retry updates it, a non-Git folder is explicit, and scans send no prompt.
- Rebuilt package; `verify-install.sh --full` passed with **34 e2e suite tests against the unpacked engine**.
  `test-install.sh` passed all 12 scratch-prefix checks again.
- Targeted socket/mock-engine tests also cover malformed Git config, missing directory, non-Git folders,
  untouched-session behavior, repair/retry, routing, and no submit/steer/stop side effects.
- Core/UI-copy tests cover generation and project isolation, stale snapshot preservation, offline/interrupted
  refresh, duplicate retry suppression, equal-count diff cache invalidation, selected-path preservation, and
  distinct empty/progress/failure/non-Git wording.

Suite counts include the opt-in profile-seeding helper, which returns without seeding when `PIPKIN_SEED_ROOT`
is unset; they are not claims of 34 distinct engine scenarios. Seeding was separately executed for the native check.

Rebuilt `pipkin-0.0.1-x86_64.tar.zst`: **89,133,924 bytes**; SHA-256:

```text
0bcf98585fb27d9d805cc0fea100a089519c97a86daa43902dcfb41f711a35d0  pipkin-0.0.1-x86_64.tar.zst
```

This replaced the local `dist` baseline tarball. Both recorded checksums describe their respective builds;
this artifact is still unsigned, version 0.0.1, and not a release candidate or a clean-system desktop pass.

### Observed native evidence (not a screen-reader speech or visual-matrix pass)

Launched the current release binary with a disposable seeded profile/project on the existing Hyprland/Wayland
session. All keyboard input used `scripts/guard.sh` pinned to that launched PID, with an isolated guard-state file.
Read the launched process's AT-SPI tree and recorded:

1. Malformed project Git config: “Changes unavailable,” the actual Git error, “No current diff is available,”
   and an accessible Retry workspace changes scan button.
2. Restored config and guarded palette refresh: `notes.txt, 1 added, 0 removed`, with the failure banner absent.
3. Failed another scan: explicit “Showing the last successful scan … out of date” plus the same selected diff/file.
4. Restored config, requested refresh, and quit; the launched application and owned engine exited.

No prompt was sent in this native walkthrough. The user's profile/configuration was not modified. AT-SPI initially
showed an unnamed status container; the same bounded check found and corrected its accessible label before the
final observation above. Actual Orca speech, visual contrast/layout, mouse activation of Retry, and the broader
native gates remain unverified here.

Temporary logs: `/tmp/pipkin-changes-{workspace,clippy,e2e,package,verify-install,installer,seed}.log`.
Native tree captures remain under the disposable `/tmp/pk-changes-JzYiFq/` profile. They are local/temporary;
this summary is the durable gate evidence. Future CI/candidate runs must retain fresh evidence.

## Stop retires obsolete instructions (2026-10-06)

Owner-reported regression: submit “commit and push,” Stop, then enter a new prompt or `/goal count to 100`;
the agent finished the stopped request before handling the replacement.

Confirmed engine defect: aborted assistant messages were excluded from model context, but their original user
instructions remained. New durable settlement appends a context-omit edit atomically with an aborted placed input.
This retires the instruction from future model requests without deleting visible history or completed tool effects.
Queued inputs are still withdrawn by the engine's abort operation. This applies to all prompts, not only goals.

Pi fix committed/pushed on `client-history-and-ui-requests`:
**`d2a311097cbcf669e699479587332ae3988a49d0`**. Full `npm run check` passed; **223 targeted durable tests passed**
across cancellation/context, submissions, generation/recovery, inbox, conversations, compaction, tables and forks.
The new prompt/goal regressions failed before the fix and passed after it. Tests also verify durable reopen,
atomic settlement/retirement, idempotent repeated Stop, preservation of original entries, and withdrawal of queued work.
Tests used a temporary Vitest source-alias configuration because this checkout's AI package has no built `dist`;
no dependency installation or unrelated package build was performed.

Pipkin changes on top of **`a3ee2bd5d9feb86788a1118791b14b4895455bc4`**:

- Stop pauses the goal present at the instant it is requested, preventing continuation if completion races the stop.
- A replacement goal entered while stopping waits for confirmation, then starts without being paused by the old ack.
- A fresh goal also waits for the engine-owned queue to clear.
- The adapter synchronizes the latest replicated transcript before terminal settlement, so goal verdict parsing
  does not race the final reply's queued notification.

Tested source delta (`git diff --binary -- crates/ | sha256sum`):
`03f46b06a716420d50df117f6b697322f8e8e90db37f23d0b9509b81847c52ed`.

Validation: **523 workspace tests passed (40 ignored)**, Clippy/fmt clean; **37 real-engine e2e suite tests passed**
against the updated Pi checkout and **37 passed against the rebuilt bundled engine**. Suite counts include the
opt-in seeding helper, a no-op when its environment flag is unset. Both replacement-prompt and replacement-goal
regressions assert that the next scripted-provider request contains only the new task, not the stopped request or
withdrawn follow-up; the stopped prompt remains visible in the transcript. Scratch-prefix installer checks passed.
This is automated engine/controller qualification, not a new native keyboard/mouse or real-provider walkthrough.

Rebuilt `dist/pipkin-0.0.1-x86_64.tar.zst`, still unsigned and not a release candidate:

```text
5ac8cd98fccbd944e603db63a835a0543db11e60575dd686f43c4348686aec1f  pipkin-0.0.1-x86_64.tar.zst
```

**Deployment requires the updated engine as well as the app.** Rebuild/reinstall the bundled package and restart;
an app-only update against the old engine retains the context defect. Existing tool effects, commits or pushes
cannot be undone by Stop. No installed app, owner profile, or desktop configuration was changed during qualification.
Local temporary logs: `/tmp/pipkin-stop-{workspace,clippy,e2e,targeted,package,verify-install,installer}.log`.

## Phase 2: pinned source staging (2026-10-06)

Implemented on app base `55f90b0c0ad713f2fb4125058929f09705f529af` with Pi pin
`d2a311097cbcf669e699479587332ae3988a49d0` in `packaging/pi-engine-revision`:

- Default engine staging rejects unidentified, dirty/untracked, and unpinned Git sources before replacing output.
  Development overrides are explicit (`PIPKIN_ENGINE_DEV=1`) and identified in the manifest; release collection
  refuses this override.
- Staging archives tracked sources instead of arbitrary ignored checkout output, copies installed workspace
  dependencies, and validates/copies the required generated provider JSON. The manifest records the full source
  revision and generated-data manifest SHA-256, not only a short commit.
- Release collection selects the current version/architecture, not every old tarball in `dist`.
- Disposable source/staging tests cover pin matching, dirty/untracked/mismatched/invalid pin rejection, the explicit
  override, missing generated input without destroying prior output, preserved dependencies and exclusion of stale
  ignored output. Disposable release tests verify development rejection and exclusion of obsolete artifacts.

An initial Git-archive-only build **failed packaged qualification**: provider JSON is Git-ignored but mandatory.
After explicit asset validation/staging, **37 packaged-engine suite tests passed**, installer scratch-prefix checks
passed, and **531 workspace tests passed (40 ignored)** with Clippy/fmt clean. The suite includes the no-op opt-in
seeding helper. No clean desktop install, real-provider onboarding, accessibility or release acceptance is claimed.

The local rebuilt unsigned 0.0.1 artifact SHA-256 is:
`91484e0c573b0e8a4d83cef4efbc5057f75628ea00628f48a5bc5f5fec08e890`.
Its engine generated-data manifest SHA-256 is:
`b92d631bb8bb2cac6f824a03a814baad9452164c518426b8bc4bbc5c2142bb05`.

Phase 2 remains **partial**: CI, complete app/build-input identification, clean-checkout provisioning, and dependency
license/provenance auditing still need work. This is identified staging, not a claim of byte-for-byte reproducible
builds. Temporary logs: `/tmp/pipkin-pin-{package,verify,installer,workspace,clippy}.log`; the final verification log
records the corrected pass, while the failure cause/count above preserves the initial failure evidence.

## Phase 2: clean provisioning and CI implementation (2026-10-06)

On app base `fbc2076`, added `.github/workflows/qualification.yml` with full-SHA-pinned actions, Node 22.19.0,
the repository Rust toolchain, locked Rust checks, disposable packaging/guard tests, clean pinned Pi provisioning,
scripted-provider development-engine tests, and full bundled-engine/installer qualification. It needs no provider
credentials or paid inference. Job logs/metadata are retained for 30 days; this does not publish a release. Ubuntu
headless automation is not qualification of an additional supported native desktop. **Remote CI pass is not yet
observed**; adding a workflow alone does not close the Phase 2 exit gate.

Clean provisioning was actually attempted in a disposable checkout fetched from the public Pi repository at the
pin. `npm ci` succeeded, but live model-data hydration **failed with public-catalog connection timeouts**. Builds
now restore a checksummed, approximately 57 KiB generated-data snapshot paired with the full source pin. It is the
same validated runtime metadata used in previous qualified packages, not user credentials/session data; provenance
and update procedure are in `packaging/engine-model-data.md`. Release staging no longer relies on the owner's
ignored assets or a mutable catalog service. Development overrides still validate the developer's own data.

After snapshot restoration, the fresh fetched checkout remained clean and passed the source pin check. Packaging
from that checkout passed **37 full packaged-engine suite tests** and the installer checks. **531 workspace tests
passed (40 ignored)**, Clippy/fmt clean; source/staging, corruption/missing-input, exact-release collection and stub
native-input-guard tests passed. The generated-data manifest fingerprint remains the recorded `b92d631b…` value.
Local unsigned 0.0.1 package SHA-256:
`310750ed6795999f9bc36493199c4baa47a7ac4829e976e3bc89efb30b9c3da8`.

This verifies clean engine provisioning on the current host, not a clean desktop installation or byte-for-byte
reproducible app build. Complete app/build-input identification, bundled provenance/license auditing, remote green
CI, native gates, signing and release acceptance remain open. Logs:
`/tmp/pipkin-clean-engine-{fetch,npm,hydrate}.log` and `/tmp/pipkin-ci-{package,verify,installer,workspace,clippy,guard}.log`.

## Phase 2: app/build identification and first CI failure (2026-10-06)

The first remote run, [37417402906](https://github.com/last-refuge/pipkin/actions/runs/37417402906), **failed** during
Rust test linking: Ubuntu lacked `libxkbcommon-x11`. Toolchain/Node installation passed. Added the missing runner
development library; the Arch package already declares its runtime equivalent. A passing remote retry is still
required. CI-generated logs are explicitly ignored so they do not falsely mark app sources dirty.

Implemented against app base `b4e478ef3f753eb6127cabd149d893cc844a1e42`:

- A compiled full app revision and explicit dirty/unknown state in `--version` and `--diagnose`; build-stamp watches
  follow source files and Git ref/index changes, including worktrees.
- Packaged `usr/share/doc/pipkin/build-info.json` records the binary hash, app/engine identities, protocol/schema,
  model-data fingerprint, source/staged npm lock hashes, Cargo lock/toolchain hashes, tool versions, platform and
  custom-Rust-flag presence. It excludes raw flags and arbitrary environment values. Engine metadata now records
  whether offline production pruning succeeded rather than assuming success.
- Release creation rejects unidentified/dirty app checkouts. Disposable tests verify these refusals, manifest field
  mapping, separate source/staged lock fingerprints and omission of a test secret supplied through Rust flags.
  Build-stamp tests separately exercise clean/dirty sources, a new commit, worktrees and unidentified sources.

**531 workspace tests passed (40 ignored)**, Clippy/fmt clean; packaging policy tests passed. A package built from
the fresh pinned Pi checkout passed **37 packaged-engine suite tests** plus scratch-prefix installer checks.
This local package truthfully reports app base `b4e478e…`, **dirty: true** (the tested implementation delta), engine
`d2a311097…`, protocol **8**, schema **7**, and successful production pruning. It is not a clean candidate.
Its unsigned 0.0.1 SHA-256 is `83372e61c2d3bf67755491ce2e5c6d5e7f681fe60f6208d0b57bc2fb638b3730`.

A report is identification evidence, not an authenticity signature or a bit-reproducibility guarantee. Runtime/
dependency provenance, complete diagnostics privacy auditing, native acceptance, signing, and final candidate
qualification remain open. Logs: `/tmp/pipkin-first-ci-failed.log` and
`/tmp/pipkin-identify-{workspace,clippy,package,verify,installer}.log`.

## Phase 2: dependency/notice inventory and bare-runtime CI failure (2026-10-06)

Added conservative locked Linux Rust and actual staged npm/workspace inventory generation. Packages now retain
found package/vendored notices and provide the unmodified registry source of the current MPL-2.0 `option-ext`
dependency. GPL-or-permissive alternatives for `self_cell` and `node-forge` are recorded as Apache-2.0 and BSD-3-Clause,
respectively. The inventory omits host cache paths, rejects npm package links outside the engine, and marks missing
or inherited notices/declarations for review. Fixture tests cover retained notices, workspace deduplication,
embedded-fixture exclusion, source provision, stale generated-output removal and external-link rejection.
Python is now declared as a build dependency; it is not an installed-app runtime requirement.

The actual tree produced **552 Rust** and **155 npm/workspace** records with **59 review flags**. Several published
crates/npm packages omit notices or declarations. **The license/provenance gate remains open**; findings and scope
are documented in `docs/bundled-licenses.md`. This is notice retention and triage, not automatic legal clearance.

On app base `985e0c6`, **531 workspace tests passed (40 ignored)**, Clippy/fmt clean, inventory/build-info fixtures
passed, and the rebuilt package passed **37 packaged-engine suite tests** plus local scratch-prefix installer
checks. Local unsigned 0.0.1 artifact SHA-256:
`9fb555aec6dfca84e4c9e03ccb923c39449d24a2ad23a873139bae8f088c2f06`.
Qualification covers the tested working delta, not a clean approved candidate.

Remote retry [37419027177](https://github.com/last-refuge/pipkin/actions/runs/37419027177) **failed** the bare-environment
probe: `setup-node` installed Node in the tool cache, outside the verifier's intentionally minimal `/usr/bin:/bin`
PATH. Rust/policy checks and development-engine qualification passed; **37 packaged-engine suite tests also passed**,
but the bare probe did not, so neither installation nor overall CI is green. The CI installer step was not reached.
The runner now exposes only the declared Node runtime in `/usr/bin`, without weakening the verifier's environment
or modifying the owner's desktop. Another observed green run is still required.
Logs: `/tmp/pipkin-second-ci-failed.log`, `/tmp/pipkin-license-inventory.log` and
`/tmp/pipkin-licenses-{workspace,clippy,package,verify,installer}.log`.

## Phase 2: immutable upstream notice recovery (2026-10-06)

Recovered **33 upstream notice files for 20 Rust package versions**, anchored to the exact Git revisions in the
published crates' VCS metadata. `packaging/upstream-notices.json` retains commit URLs and content SHA-256 hashes;
packaging verifies them and uses the cached files without network access. These supplemental repository notices
remain flagged for applicability review, not automatically cleared.

The actual staged inventory still has **552 Rust / 155 npm-workspace records and 59 review flags**, but entries
with no copied notice decreased from **32 to 12**. Five Rust and seven npm versions still lack notice files in the
inventory. License/data/runtime provenance review remains open. Details are in `docs/bundled-licenses.md`.

On app base `528457c`, **531 workspace tests passed (40 ignored)**, Clippy/fmt clean; notice provenance/fixture tests,
**37 packaged-engine suite tests**, and local scratch-prefix installer checks passed. The unsigned working-delta
0.0.1 package SHA-256 is `eb8fecee4a4d2c8f7c6ba39262a8085cbabdb6331fe81d7883e66f5efd216924`.
Logs: `/tmp/pipkin-notices-{workspace,clippy,package,verify,installer}.log`.

Remote CI run [37421406544](https://github.com/last-refuge/pipkin/actions/runs/37421406544) is still in progress; it is
not yet a green gate. CI concurrency now lets an identified run finish while queuing the latest update, rather than
repeatedly cancelling qualification as release-hardening commits arrive. Changed candidates still require their
own pass. No native acceptance, legal clearance, signing or v1 publication is claimed.

## Phase 2 green CI and Phase 5 support drafts (2026-10-06)

Observed **green remote CI**:

- [37421406544](https://github.com/last-refuge/pipkin/actions/runs/37421406544), full app revision
  `528457c091762bf2e3f5700f275a23ab3eb46e3e`.
- [37422216176](https://github.com/last-refuge/pipkin/actions/runs/37422216176), full app revision
  `12c9dbab4453116939bd7c2f74ed84ad1b25e97d` (includes immutable notice recovery).

Both completed successfully with Rust/policy checks, pinned clean-engine provisioning, development and packaged
scripted-provider qualification, bare-environment probe and scratch installer checks. They use the pinned Pi engine
`d2a311097cbcf669e699479587332ae3988a49d0`, Node 22.19.0, repo Rust toolchain, and headless Ubuntu 24.04.
These establish the CI mechanics/automation gate, **not native Ubuntu support or approval of a v1 candidate**.
License/provenance findings and candidate-specific final qualification remain open.

Added `docs/getting-started.md`, `docs/support.md` and a privacy-conscious bug-report template. They document fresh
pinned-source preparation, provider setup/expired credentials/model refresh, app versus engine/credential storage,
`--data-dir` not isolating the server, owned versus external engine shutdown, possible repeated partial tool effects,
workspace-wide Changes, cached-history search and reference-versus-attachment semantics. The advertised extension
promise explicitly excludes stable `ctx.ui.*`/TUI routing. README no longer promises infallible draft saving or
exactly-once tool effects; IME/accessibility and generic Linux support claims are bounded by actual evidence.

Corrected the old claim that `--diagnose` is automatically safe to share: its engine-log tail and arbitrary errors
are **not comprehensively redacted**. Reports must be reviewed before sharing. Share-safe in-app Copy diagnostics
and redaction tests remain open. First-run/new-user/clean-desktop results are not claimed by documentation alone.

Documentation working-delta checks on app base `12c9dba`: **531 workspace tests passed (40 ignored)**, Clippy/fmt clean.
Logs: `/tmp/pipkin-onboarding-{workspace,clippy}.log`. No destructive owner-profile testing or desktop changes.

## Phase 5: metadata-only Copy diagnostics (2026-10-06)

Added **Ctrl K → Copy diagnostics**, available without a selected conversation and in real/demo/offline states. The
controller prepares an allowlisted launch-source snapshot outside render: app version/full revision/dirty state,
client protocol/supported schema, build platform/mode and configured engine manifest revision/protocol. Only a
40-hex revision is retained from manifest identity; arbitrary manifest names, version/error strings and paths are
not copied. External/missing/rejected/unmanifested sources are explicitly unidentified; demo remains simulated.
The snapshot does not claim running-server identity, database health, Node availability or provider authentication.

No log tails, credentials, environment values, private errors, session IDs, prompts or attachment/project paths enter
the report. Palette grouping, copied-report footer and feedback explain the scope/privacy. When controller metadata
is unavailable, the action explains that and leaves the clipboard unchanged rather than claiming success.
Automated tests cover hostile manifest/build strings, missing/broken source files, known/unknown/demo identity,
registry availability and a headless GPUI clipboard copy/unavailable path with private state present. **No owner
clipboard or desktop was touched; native clipboard observation on the installed candidate remains unverified.**
Detailed CLI diagnostics remain potentially sensitive and review-before-sharing.

On app base `b66f7e7`, **536 workspace tests passed (40 ignored)**, Clippy/fmt clean, **37 development-engine and
37 packaged-engine suite tests passed**, and scratch-prefix installer checks passed. A first attempt failed 36
engine tests because the previously saved `/tmp` checkout pointer no longer existed; no packaging ran. A fresh
public clean checkout at the pinned Pi revision was provisioned with `npm ci` and the validated snapshot; the retry
passed. The owner's Pi source/profile was not changed. Logs: `/tmp/pipkin-copy-diagnostics-{workspace,clippy,ui,
provision,engine,engine-retry,package,verify,installer}.log`.

Unsigned 0.0.1 working-delta package SHA-256:
`9eb633740fef82037f6c1b0d1f2351c4b011d2d5b5f5a8c43ea21f55df5214f5`.
Remote [37440342638](https://github.com/last-refuge/pipkin/actions/runs/37440342638) passed for clean full app revision
`b66f7e7cd1b3fa89f853f8a207e8641d435eaf75` (before this feature). This changed candidate still needs its own green run.

## Phase 1: recovery review, safe support link and effort verification (2026-10-06)

Reviewed the existing recovery implementation rather than adding a second reconciliation path. `stopping_detail`
explains unconfirmed work and stop-only retry; core availability gates one in-flight check/stop request. Core tests
`failed_stop_can_be_retried_but_clock_and_status_checks_never_settle_it`,
`recovery_notice_and_check_failure_are_per_conversation_and_do_not_replay_work`, and unknown-status failure tests
cover timers, reconnect, late settlement and conversation guards. Real-engine
`a_lost_stop_request_remains_unsettled_and_can_be_checked_then_retried_without_a_prompt` drops the stop over a fault
proxy, reconnects, checks and retries without another provider request. Worker-crash and interruption tests remain
in the full suite. The user's Stop intent cannot override a raced engine-confirmed completion.

Recovery now links directly to metadata-only **Diagnostics** instead of directing users only to potentially sensitive
CLI output. Unknown outcome explicitly warns that lack of acknowledgement is not proof tools never ran, keeps
sending disabled until reconciliation, and tells users to inspect actual/external effects before repeating work.
Copy tests protect that wording. Native installed recovery-layout/button observation remains a Phase 4 gate.

Effort authority/per-conversation restoration is covered by core generation-guard tests and the real-engine
`choosing_effort_is_confirmed_by_pi_and_restored_per_conversation` test. This closes the automated behavior task,
not the native menu walkthrough.

**Corrected a documentation error:** the pinned Pi server resolves durable session storage to
`<agent dir>/experimental/sessions/<session ID>/`, containing `meta.json` and worker-owned `session.sqlite`.
`~/.pi/server` is the coordination profile, not that session store. `docs/support.md` and `docs/packaging.md` now
state the correct ownership/backup boundary, verified against `resolveSessionDirectory`, `session-catalog.ts` and
the suite's `RawSession::attach` path. External servers may override their session directory.

On app base `14ee65d`: **537 workspace tests passed (40 ignored)**, Clippy/fmt clean, **37 development-engine and
37 packaged-engine suite tests passed**, scratch-prefix installer checks passed. No owner profile/desktop changes
or paid requests. Unsigned working-delta 0.0.1 artifact SHA-256:
`c28b47172aef759281de710aa0e1c3c2fa23c6b2802d3b66868910fcd898de67`.
Logs: `/tmp/pipkin-recovery-review-{workspace,clippy,engine,package,verify,installer}.log`.
Remote [37445564908](https://github.com/last-refuge/pipkin/actions/runs/37445564908) was still in progress at the
last check; this delta/final candidate must obtain its own green qualification.

## Phase 2: npm notices, native/runtime inspection and CI evidence loss (2026-10-06)

Recovered two immutable MIT notices using the published npm version's `gitHead`: `@esbuild/linux-x64` 0.28.2
and `standardwebhooks` 1.1.1. The latter uses **libraries/LICENSE**, not the repository-root Apache specification
license. Cached URLs/revisions/hashes are retained and verified offline. Missing notices decreased **12 → 10**;
**552 Rust / 155 npm-workspace entries and 59 review flags** remain. Failed/missing upstream lookup findings are
recorded in `docs/bundled-licenses.md`; no unrelated current notice or fabricated copyright was substituted.

Added packaged `runtime-inventory.json` and retained CI evidence. The readelf-only scanner never executes binaries;
fixtures verify direct dependencies/ABI references, foreign ELF/non-ELF flags, symlink deduplication, deterministic
regeneration, path omission and non-execution. Python/binutils are explicit build dependencies. The current tree
has **33 native artifacts** (including build object files), **seven foreign architecture/format flags**. Optional
VM components, bundled libkrun/libcap-ng and C++ runtime links need additional review; the wrapper npm license is
not proof of native/static component clearance. The local app references **GLIBC_2.44**; do not assume portability
of this artifact to older Linux systems or count headless Ubuntu CI as native support.

Remote [37445564908](https://github.com/last-refuge/pipkin/actions/runs/37445564908), on `14ee65d`, **failed**:
36 packaged-engine tests passed and `a_real_run_edits_files_shows_output_and_diff_and_reopens_the_same_history`
failed. The verifier's `tail -5` discarded the assertion detail, so the cause is **unknown**, not a diagnosed or
fixed flake. Removed truncation and added `--locked` so future CI retains the complete test failure. Installer
checks were not reached in that failed job. Remote
[37447515009](https://github.com/last-refuge/pipkin/actions/runs/37447515009) on clean full app revision
`77fa11d6838d232678b9f00874db3faab2752e8f` **passed** the full workflow. That later pass does not establish the
cause of the earlier failure; recurrence must be investigated during stabilization.

Local validation on app base `77fa11d`: **537 workspace tests passed (40 ignored)**, Clippy/fmt clean, inventory
fixtures passed, packaging and **37 unpacked-engine tests** passed, installer checks passed. Repeated the full
37-test packaged suite with the untruncated verifier; passed again. No new development-engine run is claimed for
this packaging-only delta. Unsigned working-delta 0.0.1 package SHA-256:
`f332d4ab60c86f58e9465e18102d7ecbf3f2fc8bb3d458441898f234f6cfd25d`.
A final rebuild includes hashes for foreign-format native addons; all 37 packaged tests and installer checks
passed against that final working-delta artifact too.
Logs: `/tmp/pipkin-runtime-audit-{workspace,clippy,package,verify,full-verify,installer}.log`,
`/tmp/pipkin-diag-ci-failed.log`. No owner profile/configuration/clipboard changes or paid requests.

## Phase 1: unsupported rename lifecycle regression (2026-10-06)

Added `unsupported_real_rename_never_saves_a_local_title_across_failure_switch_and_reopen`. For ready,
reconnecting, offline, failed and incompatible connections, it attempts rename against selected/background/missing
conversation IDs and verifies unchanged labels/selection, no backend or persistence effects, and no success notes.
Fresh core initialization from the catalog/preferences models reopen; no rejected local title can enter a save
through this command. This is not an actual installed process restart or native F2/menu observation. Existing demo
rename and real/demo palette regressions remain green. Real titles are prompt-derived display names; the current
engine has no title/rename contract. An initial test assertion incorrectly expected catalog title updates and was
removed after checking that actual contract; it was a test assumption, not a diagnosed product regression.

On app base `a849ec3`: **538 workspace tests passed (40 ignored)**, Clippy/fmt clean, and **37 development-engine
suite tests passed** against the clean pinned engine. Only a core test was added; no fresh packaged artifact is
claimed here. Candidate CI still requalifies packaging. Logs:
`/tmp/pipkin-rename-regression-{workspace,clippy,engine}.log`.

## Phase 5: private security route and README boundary corrections (2026-10-06)

GitHub private vulnerability reporting was disabled. Enabled it for `last-refuge/pipkin` and confirmed
`GET /repos/last-refuge/pipkin/private-vulnerability-reporting` returns **enabled: true**. Added `SECURITY.md`
with the actual advisory-report URL, minimal synthetic/redacted evidence guidance, credential-rotation advice,
current qualification-only fix scope, paired app/engine updates and execution/data boundaries. No report was filed;
report delivery/response time is not observed, and no SLA, security audit or approved-release maintenance window
is promised. Support docs and public bug-report instructions now point to this private route.

README no longer says the engine bundle replaces all external dependencies or that every external server stops
on quit. It scopes qualification to x86_64 Arch/Omarchy/Hyprland/Wayland, links native ABI/runtime findings, explains
prompt-derived real conversation labels and demo-only rename, and replaces the infallible “Never lose work” heading.
These are corrected source claims, not installed/native acceptance or final release support approval.

On app base `1d256cd`: **538 workspace tests passed (40 ignored)**, Clippy/fmt clean; documentation-only delta,
no new package or provider/native run claimed. Logs: `/tmp/pipkin-security-policy-{workspace,clippy}.log`.
Remote qualification for `a849ec3` and queued `1d256cd` had not finished at the last check; new candidates still
need their own qualification. Owner-profile/desktop/clipboard state was not changed.

## Phase 1: project-path completion freshness and IME unmark (2026-10-06)

Found two release defects: the index was collected only when the project changed, hiding new/deleted paths during
same-project use; IME `unmark_text` left completion suppressed until another edit. New query openings now refresh
the bounded, read-only snapshot off the UI thread (not every character/render), with one current-root scan and a
root/scan-epoch guard against stale results, including A→B→A switches. Matches/links are cleared while scanning;
loading, no matches and root-scan failure are distinct. A new query can retry a repaired folder. Scans remain
snapshots, not live filesystem watches; inaccessible descendants/dependency folders/symlinks are excluded.
Unmark refreshes query state; composing/disabled/selection states still suppress completion.

Automated GPUI tests use disposable real directory fixtures for Tab/arrow navigation, spaces/Unicode, link
recognition, undo/redo, file create/delete and rapid project switches. A rendered-option mouse event inserts the
reference; Ctrl-click dispatches the exact percent-encoded project URL through the **test platform**, not the owner's
URI handler. IME unmark permits completion without submission, while active composition blocks it. Pure filesystem
tests prove missing-root error, symlink non-traversal and actual permission-denied root handling under local UID 1000
(the permission assertion explicitly does not qualify a root-user run). Initial synthetic-index test expectations
were corrected to include discovered directory candidates. No native mouse, IME, URI handler or visual matrix pass
is claimed; those remain Phase 4.

On app base `9dda58f`: **542 workspace tests passed (40 ignored)**, Clippy/fmt clean, **37 development-engine and
37 unpacked packaged-engine tests passed**, scratch installer checks passed. Unsigned working-delta 0.0.1 artifact
SHA-256: `b79c439234de8095e5176f61b0d3a384262db4926720b6b9b50e0879ac969cd4`.
Logs: `/tmp/pipkin-mentions-review-{targeted,mouse,workspace,clippy,engine,package,verify,installer}.log`.
No owner profile/configuration/clipboard changes or paid requests.

Observed green remote [37451076672](https://github.com/last-refuge/pipkin/actions/runs/37451076672) on clean app
`a849ec390c0a38f41879190c84a95035f9918f5a`, including native-inventory fixtures/retention and all packaging steps.
Queued `1d256cd` run 37451996755 was cancelled as the newest pending revision replaced it; `9dda58f` run 37452621241
is in progress. This completion change still needs its own candidate qualification.

## Phase 5: status consolidation and installed help (2026-10-06)

Corrected a remaining unsafe beta statement: home substitution in `--diagnose --probe` does **not** make output
credential-free or safe to post. Beta reporting now prefers limited metadata and links privacy/private security
instructions; duplicate-dispatch targets no longer imply universal exactly-once model/tool effects. The 1500-prompt
measurement is explicitly historical. Scorecard counts are an identified 6296cf2 working-delta snapshot, not a moving
claim that every HEAD passed; old prototype gates/framework recommendations are visibly nested as historical.

`docs/platforms.md` now distinguishes the working x86_64 Arch/Omarchy/Hyprland/Wayland scope from final support
approval, historical local native observations, headless Ubuntu CI and generic installer mechanics. It records
protocol 8/schema 7, Node/desktop/Git requirements and ABI/native review boundaries. Corrected broad claims that
Pipkin logs are credential-free, a paired package makes upgrade infallible, or rollback is automatically usable.

Packages previously carried only the marketing README and one native manual, leaving published setup/recovery
instructions absent. Installed help now has a focused index, SECURITY policy, current Markdown guides, qualification
plan/provenance and dependency recipe. It intentionally does not include full source tooling/screenshots; historical
notes and developer commands are labelled as such. `test-package-docs.py` verifies **25 guides/inputs** against exact
source bytes and is required after packaging in CI. Local disposable fixtures also rejected missing/stale support
text without changing the staged artifact. Packaging metadata uses the actual `last-refuge/pipkin` repository URL.

On app base `6296cf2`: **542 workspace tests passed (40 ignored)**, Clippy/fmt clean, documentation check passed,
**37 unpacked packaged-engine tests passed**, scratch installer checks passed. No new development-engine run or
native/new-user observation is claimed for this docs/packaging delta. Unsigned working-delta artifact SHA-256:
`d257c78fcd750cfbf731a83d36763b2818c4d61b8b85e3e6e4044d611b8bbae5`.
Its guide-byte check preceded the post-run plan/journal update; final clean-candidate CI must capture fresh guides.
Logs: `/tmp/pipkin-support-consolidation-{workspace,clippy,package,verify,installer}.log`.

Observed green remote [37452621241](https://github.com/last-refuge/pipkin/actions/runs/37452621241) for clean full
app `9dda58fefed4853a3cc7502e13f7af65ed550258`. Run 37454852910 on 6296cf2 remains in progress at the last check.
Remaining roadmap references, screenshot/version labels, final support acceptance and native/new-user gates are
not silently closed by these documentation corrections.

## Owner scope change and first macOS CI/port pass (2026-10-06)

The owner accepted the remaining layout/menu regression work without more checks, asked for platform builds
before calling the next release 1.0, and then prioritized **macOS first, Windows after Mac review**. This is an
acceptance/priority change, not invented native evidence. No final 1.0 version, tag or publication was made.

Added `.github/workflows/macos-build.yml`: macOS 15 runner, pinned Rust/actions/Node, short private temporary
paths, core/Unix transport tests, native ownership tests, two scripted-provider engine smoke checks, actual
native release compilation and an experimental `.app` archive with checksum/build identity and target-specific
Rust notices/MPL source. It records the actual runner architecture rather than pretending to be universal.
The app archive does **not** bundle Pi or Node, and has no Developer ID signing/notarization. Engine checkout
and immutable snapshot provisioning are separate for CI/evaluation. `docs/macos-first-pass.md` explains the limits.

Transport now uses kernel `getpeereid` on macOS, retaining filesystem ownership/privacy and server-id checks;
Linux `SO_PEERCRED` is unchanged. macOS owned-process discovery uses kernel uid plus exact NUL-delimited
environment identity, excluding argv/prefix impostors and unrelated identities. A native disposable-child test
must pass on the runner; local tests cover parser truncation and malformed/argument-only inputs. No environment
buffer is logged. macOS does not apply Linux's forced stale-launcher-lock cleanup optimization. Corrected the
old platform-doc claim that GPUI native backends were disabled: `gpui_platform::application()` already selects them.

Local Linux working-delta validation: **544 workspace tests passed (40 ignored)**, Clippy/fmt clean, license
inventory policy tests (including unbundled Darwin target), macOS bundle fixture policy checks, **37 development
and 37 unpacked packaged-engine tests**, installer and **26 installed-guide/input checks** passed. Bundle fixtures
are not a native Mac build/launch claim. Working-delta Linux package SHA-256:
`d85b919eb583a6dbba8a051c5c484a6d80cefcb9b8decaa39ccdba830039e11d` (before final platform/plan/journal notes).
Logs: `/tmp/pipkin-macos-pass-{workspace,clippy,engine,package,verify,installer}.log`.

Observed green CI for clean `6296cf212f5c12077ab03435a3710207db9efe8b` (37454852910) and clean
`44b3a507ff0c744c4b7fafa1e3e5288191fb457d` (37456363171). The new macOS workflow has not yet run at this entry;
its existence is not green compilation or support acceptance. Finder/PATH, native clipboard/IME/accessibility,
editor/terminal defaults, sleep/wake, notarization and full engine packaging remain Mac follow-up work.

### Fresh extended automated soak before the Mac delta

Clean app `44b3a507ff0c744c4b7fafa1e3e5288191fb457d` and pinned engine d2a3110 completed **1500 sequential
prompts**, 40 repeated words per scripted response, one conversation, 375-prompt warm-up. The isolated harness
sampled `/proc` RSS (confirmed 4096-byte pages) and app file descriptors; this is not a real GPUI window/day-long
session or an engine-fd measurement. Subsequent peaks:

| Engine | Baseline → peak engine KiB | Baseline → peak app KiB | App fd baseline → peak | Duration |
| --- | --- | --- | --- | --- |
| Development | 640972 → 719244 | 21932 → 52708 | 14 → 19 | 87.97 s |
| Unpacked package | 640200 → 716900 | 22060 → 50572 | 15 → 19 | 95.63 s |

Passed the existing generous per-prompt resource-growth tripwire, not proof of flat memory or no leaks. The full
packaged suite also passed 37 tests with the 1500-round override, and installer checks passed. Clean unsigned
Linux archive SHA-256: `d3e827cb7c40246acaf4f00a973e00ece114564586cd6b7caf29e82d4ba0e95e`;
binary SHA-256: `b4ffb7ae66919353656cbf0770570f0d80666f99842b66ea3d1cf4b5d8172000`. Build identity recorded
44b3a50 with dirty=false, protocol 8/schema 7 and Node 26.8.1/npm 11.19.0. Logs:
`/tmp/pipkin-extended-soak-{development,package,docs,packaged,packaged-metrics,installer,workspace,clippy}.log`.
This is a fresh baseline, not final-RC soak acceptance and not Mac resource instrumentation.

## macOS runner feedback: portable server identity (2026-10-06)

First native run [37478670155](https://github.com/last-refuge/pipkin/actions/runs/37478670155) on clean
`f5fee65e4d12f219e68af852642bb103d7319c2b`: **native macOS release compilation passed**, core/Unix transport
checks passed, and all three owned-process tests passed, including actual kernel discovery of a disposable child
and exclusion of an unrelated identity. The run nevertheless **failed before engine startup** because the test
fixture required `/proc/sys/kernel/random/uuid`. No app archive was produced; no smoke/native UI pass is claimed.
Linux qualification run 37478670080 on the same clean source passed.

Inspection found the same Linux-only UUID dependency in real CLI-managed startup and diagnostics probing (the
previous platform-doc "fallback" claim was incorrect). Replaced all three paths with one UUIDv4 helper using OS
entropy from `/dev/urandom`, setting version/variant bits and failing rather than substituting timestamps when
entropy is unavailable. Disposable tests check canonical/distinct ids, remembered profile identity and explicit
identity without overwriting the stored default; these are now included in Mac CI. The test project root is
canonicalized so macOS `/tmp` aliases cannot disagree with engine-recorded cwd/history paths.

Local working-delta verification: **546 workspace tests passed (40 ignored)**, Clippy/fmt clean, **37 development
and 37 unpacked packaged-engine tests**, installer and 26 guide/input checks passed. Logs:
`/tmp/pipkin-macos-uuid-{workspace,clippy,engine,package,verify,installer}.log`. Unsigned working-delta Linux
archive SHA-256 `371910604f838c60f39178557b160354cd04c997067b5ac4c81f74b40474dc26` (before this journal entry).
A fresh native run is still required for the corrected engine smoke checks and actual Mac artifact production.
Failure details retained locally in `/tmp/pipkin-macos-ci-{failure,full}.log` and remotely as build evidence.

## macOS smoke checks passed; locked notice-input provisioning correction (2026-10-06)

Native run [37481551475](https://github.com/last-refuge/pipkin/actions/runs/37481551475) on clean
`11a7496` passed actual **aarch64-apple-darwin** release compilation, core/transport/ownership and portable identity
checks, owned engine startup/clean shutdown (2.25 s), and scripted-provider edit/diff/history reopening (6.60 s).
It then **failed** in offline license metadata collection: `bit-set 0.8.0` was locked but absent from the runner's
cache because limited smoke checks do not compile the whole workspace-unified feature graph. No Mac archive
was produced by that failed run; this is not a native GUI/provider-authentication/installed acceptance claim.

The workflow now explicitly `cargo fetch --locked --target "$target"` before the unchanged offline inventory
operation. This supplies checksum-locked audit inputs without dropping notices, weakening offline collection or
mutating the lockfile. First-pass help links now point to readable source guides when viewed outside the source
checkout. Failure log: `/tmp/pipkin-macos-uuid-ci-failure.log`. Fresh artifact-producing CI remains required.

## First downloadable macOS artifact: observed green (2026-10-06)

[Native run 37482847877](https://github.com/last-refuge/pipkin/actions/runs/37482847877) passed on clean full
app `f52e8c254488c4524c4cb986530eee7048b4f138`: macOS 15 **Apple Silicon/aarch64**, pinned engine d2a3110,
core/Unix peer transport, native owned-process identification, portable/remembered identity tests, release
compilation, owned engine startup/shutdown, scripted file-edit/diff/history reopening and artifact creation.
Intel/universal and interactive GUI acceptance are not claimed. Separate Linux run 37481551428 on clean
11a7496 also passed; f52e8c2 Linux qualification was in progress at the last check.

Download Actions artifact **`pipkin-macos-ARM64-f52e8c254488c4524c4cb986530eee7048b4f138`**, then extract the
contained `pipkin-0.0.1-aarch64-apple-darwin-experimental.zip`. Archive SHA-256:
`f1e17d1f85153ac70642f14344ec8eae97d080f1eea153691e5ae2f8f8d296f3`.
Binary SHA-256: `cf80029d9876e1bd2df479037126b349be31ad768ef004a5efa77d0a625dfdf4`.

Downloaded artifact inspection on Linux verified SHA256SUMS, binary/build-info identity, Mach-O magic,
executable permissions, bundle plist and all retained notice hashes **without executing the binary**. It contains
460 target-reachable Rust entries with 53 review flags, retained source/notices and direct system Mach-O library
references. That inventory is not legal clearance or a full native/static-component audit. Local artifact review
root is recorded in `/tmp/pipkin-macos-artifact-review-root`.

This closes the owner's requested **first Mac CI/build pass**, not the 1.0 release. Pi/Node are not bundled;
Developer ID/notarization, Finder startup, provider authentication, Cmd conventions/native input, accessibility,
clipboard and real interactive lifecycle checks remain Mac evaluation topics. Windows follows owner Mac review.
No version/tag/publication change was made. First-pass instructions and platform scope now link the actual green
run rather than treating workflow existence as evidence.

## Owner Mac launch failure: seal the completed bundle (2026-10-06)

Owner-reported first downloaded Mac launch was rejected as “damaged and can't be opened.” After correcting
smart-quote shell syntax, the owner reported `codesign --verify --deep --strict` output: **“code has no resources
but signature indicates they must be present.”** This is failed downloaded-bundle signature evidence, not a GUI
crash or a completed Gatekeeper assessment. Prior native compilation/engine smoke passes remain valid, but
f52e8c2/ba836a7 archives are **not launch-qualified**. Static archive/hash checks did not establish bundle validity.

The packager previously relied on the linker's bare-executable signature and never sealed the completed app.
It now signs the complete `.app` with an explicit **ad-hoc** identity, requires strict/deep codesign verification,
archives it, extracts into a disposable directory and repeats verification plus final executable-hash matching.
Nothing inside the app changes after signing. Embedded metadata retains pre-sign binary identity; final signed
binary identity stays in the external report to avoid hashing a signature that seals its own hash. This is not
Developer ID signing, notarization, publisher authentication or proof of Gatekeeper acceptance.

Local bundle-policy fixture checks passed, including signature-induced executable changes and rejection of
resource tampering after archive extraction before checksums/publication. **546 workspace tests passed (40
ignored)**, Clippy/fmt clean. No new real-engine run is claimed for this packaging-only delta; Mac CI must repeat
its engine/build checks and perform the new actual codesign/archive verification. Logs:
`/tmp/pipkin-macos-bundle-signature-{workspace,clippy}.log`. Native signature correction remains unverified until
that new runner succeeds; normal quarantined download trust still requires a separate acceptance decision.

## Corrected completed-bundle signature: observed green (2026-10-06)

[Mac run 37488722228](https://github.com/last-refuge/pipkin/actions/runs/37488722228) on clean full app
`91e1586000a1ed999d28b04fe9dbf4dcae316c83` passed compilation/transport/ownership/engine smoke checks and
**actual strict signature verification of the completed and re-extracted bundles**. Both codesign calls reported
“valid on disk” and “satisfies its Designated Requirement.” This addresses the owner-reported missing-resource
signature failure in the prior packaging. It does not establish quarantined download/Gatekeeper acceptance.

Corrected Apple Silicon archive SHA-256:
`1821666d514fbe75eabd92f3dcc0e313925be93f06a891bda1b7a33153a8526b`.
Final signed executable SHA-256: `be8c087d709a624a10707d19d061f78d59ae99a8794c1bab0e379b3f3c31d2ca`;
pre-bundle-signing executable SHA-256: `85dd8c5b7e8490ddb37126a70cc6e08bccaa7ec3076a898bccc072458ab995e8`.
Downloaded archive checks confirmed CodeResources presence, archive/final executable hashes and separation of
embedded pre-sign identity from the external final hash, without executing it locally. Native log:
`/tmp/pipkin-macos-bundle-signature-ci-full.log`; review root pointer:
`/tmp/pipkin-macos-signed-artifact-root`. Linux run 37488722412 remains in progress at this check.

Artifact: **`pipkin-macos-ARM64-91e1586000a1ed999d28b04fe9dbf4dcae316c83`**. Extract into a fresh directory and
replace the old app as a whole, not by merging resources. Actual owner launch retry/Gatekeeper assessment is
still required. The signature is ad-hoc only; Developer ID/notarization remain absent. No release/signing-key
promise or successful GUI launch is inferred. Preserved the concurrent unrelated redacted-thinking change
`ef861bd`; only packaging/tests/documentation were modified here.

## Owner Mac launch and first-run implementation foundation (2026-10-06)

The owner reported strict codesign verification succeeded on the corrected bundle, while `spctl` returned
“rejected.” After app-scoped quarantine removal, the owner reported **“It runs!”** and the UI showed no Pi server
in `~/.pi/server`. This is owner-observed launch, not normal Gatekeeper acceptance or successful
provider setup. It confirms the experimental archive's absent engine/runtime produces an unsuitable first-run
experience; signing trust and onboarding are separate workstreams.

The owner approved a beautiful Pipkin-branded welcome → provider connection → project → productive conversation,
with self-contained engine/runtime and no manual-server setup as the default, and authorized implementation.
Development-only contract: `llm-docs/pipkin-onboarding-plan.md`; matching future UI surface brief under
`.impeccable/surfaces/`. These contracts are not installed as public help or embedded in the app.

Implemented framework-independent `pipkin-core::onboarding`: explicit first-run disposition, secret-free engine/
provider/model/project readiness, distinct pending/failure states, existing-connection discovery without implicit consent,
completion eligibility and retry-epoch guards. Returning completed/saved-work, demo and explicit external-server
launches preserve workspace/recovery behavior. Eight isolated tests exercise the readiness matrix, delayed
results, project preservation and non-wrapping epoch exhaustion. The policy is **not yet wired to a shipped view,
provider mutation or persisted completion**; no new-user completion or beautiful rendered surface is claimed.

The pinned experimental service has no provider-login/API-key-write RPC; Pi auth orchestration belongs to its
ModelRuntime/pi-ai Models and credential store. A typed secret-safe integration is required rather than a fake
connection button, plaintext credential wizard persistence or silent external credential reuse. Bundle/runtime
and auth implementation precede final surface wiring; the owner-approved visual direction is recorded now.

Local validation: **554 workspace tests passed (40 ignored)**, eight onboarding policy tests, Clippy/fmt clean.
Logs: `/tmp/pipkin-onboarding-{policy-tests,workspace,clippy}.log`. No native visual review, new package/engine run,
new credential storage, owner-profile modification or system/package installation was performed in this foundation
pass. Prototype-era product-context statements were noted, not silently rewritten as an unrelated side effect.

## Onboarding: self-contained Mac engine/runtime implementation (2026-10-06)

Implemented the next onboarding foundation: Mac packaging stages the clean pinned Pi engine plus a minimal
architecture-matched **Node 22.23.3 LTS** runtime. Immutable archive URLs/checksums in `packaging/node-runtime.json`
were recorded from the official versioned Node manifest (HTTPS/checksum evidence, not an independent signature
or security audit). Only `bin/node` and the full Node/dependency LICENSE are extracted from exact regular archive
members; no global Node/npm installation is changed. Runtime provenance and hashes are retained separately from
Rust/npm engine notices. Node's copied executable is not rewritten; final app signing/archive checks remain required.

The app's existing relative engine discovery resolves `Contents/lib/pipkin/engine`; the paired runtime lives next
to it. A `requiresBundledNode` manifest flag explicitly selects that runtime, prepending only the owned child PATH
and preserving tool search paths. Missing declared payload is an error, never a silent fallback to global Node.
Development/external engines and Linux packages without that flag keep their existing runtime behavior. CLI
diagnostics use the selected runtime and no longer misreport missing Wayland as a Mac display requirement.

Mac CI now provisions Node 22.23.3, stages/prunes Pi, collects actual Rust/npm notices, runs lifecycle and
file-edit/history smoke checks against the staged engine/runtime, packages/signs it and extracts the archive.
It then requires the **actual extracted Pipkin CLI** to discover/start/stop the bundled engine with a bare system
PATH and a private short temporary HOME/agent profile, without source overrides or provider credentials.
This is an implemented qualification gate, **not yet a passing native result**. Finder/UI and auth remain separate.

Local validation: **557 workspace tests passed (40 ignored)**, Clippy/fmt clean; Node archive hash/minimal-payload/
notice/safe-extraction fixtures and bundle signing/extraction/bare-environment fixtures passed. Runtime tests cover
explicit discovery, missing-runtime refusal, path ordering/spaces and selected-runtime diagnostic version. **37
development and 37 unpacked Linux packaged-engine tests**, installer and 26 installed-guide/input checks passed.
Logs: `/tmp/pipkin-bundled-node-{workspace,clippy,engine,package,verify,installer}.log`. Working-delta unsigned Linux
archive SHA-256: `5ca22fcd153340c61ea8a054956c65282783b3eab1be38f40a8e58d875a8e3a3` (before this journal entry).
No native Mac bundle/runtime success, implemented provider login, setup UI, credential reuse consent or first-reply
observation is claimed. The full onboarding plan remains open; no owner profile or desktop/runtime configuration
was modified. Signing trust, notices/provenance review and provider integration remain explicit workstreams.

## Self-contained Mac runtime: first green native artifact (2026-10-06)

Native Apple Silicon run [37507592573](https://github.com/last-refuge/pipkin/actions/runs/37507592573) passed
on clean `c91bbfe17e27d5f42ebceda9c0784d8bd5c93e6e`. The pinned production Pi + bundled Node 22.23.3 pair
passed lifecycle/owned-process cleanup and scripted-provider file-edit/diff/history reopen tests. The completed
app and extracted archive passed strict/deep codesign verification. **The actual extracted CLI**, with private
short HOME/agent/TMPDIR and bare `/usr/bin:/bin:/usr/sbin:/sbin` PATH, found the bundled engine/runtime and
reported Node **22.23.3**, protocol8/schema7, handshake **OK in 1.9s**. A third signature check after the probe
confirmed startup did not mutate sealed bundle resources. No owner credentials or source-engine override used.

Artifact `pipkin-macos-ARM64-c91bbfe17e27d5f42ebceda9c0784d8bd5c93e6e`:
- Archive SHA-256: `31c95536ce8e82c2c7d86faa15b4185029247dfce4d96294a8331a363cc46590`.
- Final signed app binary SHA-256: `091ffa8f234de0c9c08bb11effa2759d6e88bf0019fc7fd7828c450e44845940`.
- Bundled Node binary SHA-256: `68f4d07ca49e0500cc135c7e0a445093e228e42e126ac22306d045f0a8c2636b`.
- Static downloaded-archive review on Linux independently checked archive/app/Node/full-LICENSE hashes,
  exact clean Pi revision, production-prune flag and explicit bundled-runtime declaration; 615 Rust/npm
  package inventory entries retained. This is not license/security clearance or Linux execution of Mac code.

Evidence log `/tmp/pipkin-bundled-node-ci-full.log`; downloaded artifact pointer
`/tmp/pipkin-bundled-macos-artifact-root`. Public evaluation instructions now distinguish the old unbundled
artifacts from this self-contained one. The archive embeds the instructions as they existed at its clean source
revision; updated instructions do not retroactively alter its resource seal. Concurrent `5da88cd` (live tool
activity status) preserved. The onboarding plan's automated runtime milestone is now checked; actual Finder,
provider-authentication/credential-consent UI, native visual review and owner first reply remain **unverified**.
Ad-hoc signing still supplies no publisher identity/notarization; no 1.0 release/tag/publication authorized here.

## Linux private-runtime installer iteration (2026-10-07)

The generic x86_64 Linux tarball now stages a checksum-pinned private Node 22.23.3 (with full
LICENSE) alongside the paired Pi engine. `./install.sh` checks architecture, Git, basic tools,
missing direct libraries, app/Node payload hashes and a throwaway offline installed-engine
handshake **before** activating a version. It prints TTY-only colored ASCII progress, respects
`NO_COLOR`, and preserves the old version when a checksum or handshake fails. It neither invokes
a system package manager nor modifies system Node; missing GPU/desktop libraries need distro-specific
installation. The Arch package also carries the private runtime; building needs Node/npm, running
does not require a system Node package.

Local scratch-prefix verification passed install, upgrade, rollback, uninstall, accidental-path
refusal, altered app/Node rejection, failed-engine refusal, paths with spaces and desktop entry
validation. The unpacked Linux artifact passed 37 scripted-provider real-engine tests; an isolated
Wayland GUI launched from the unpacked layout reached the shared OAuth provider screen with a fake,
non-working system `node` ahead of PATH. This is observed behavior on the author machine, not a
clean third-party install or provider-authenticated first reply. Signed release, portable GLIBC/GPU
coverage, native distro matrix and owner sign-off remain open.

## Gate tracker

Status meanings: **passed (automated)**, **owner-reported**, **partial**, **open**, **failed**, or **unverified**. Every candidate should update these with its app/engine revisions and evidence. Historical passes must not silently become qualification of a changed candidate.

| Gate | Phase | Current status | Ownership / next action |
| --- | --- | --- | --- |
| Release scope and final acceptance policy | 0 | Working scope recorded; final sign-off open | Owner confirms advertised support and any acceptance exceptions |
| Test/package baseline | 0 | Passed (automated), identified above | Engineering reruns after code/candidate changes |
| Daily-use beta | 0 | Owner-reported | Owner supplies dates/incidents if available; do not invent metrics |
| Real-mode rename | 1 | Addressed: palette disabled, F2 explains; automated lifecycle/no-save regression passed | Native installed walkthrough remains; authoritative rename is not advertised |
| Changes scan failure | 1 | Addressed in working tree: automated/core/real-engine/package checks passed; native AT-SPI failure/stale/recovery observed | Broader native visual/Orca/mouse checks remain open; repeat on the final candidate |
| Interrupted tools/unresolved operations | 1 | Passed (automated) behavior; recovery/status/stop-only retry and metadata support guidance present | Native installed walkthrough/final-candidate qualification remain; never infer settlement from timeout |
| Recent UI regression walkthrough | 1/4 | Partial: targeted automated/native evidence exists | Engineering + owner verify menus, effort, pane sizing, and mentions in installed build |
| Pinned clean engine build and manifest | 2 | Passed (automated): full engine pin, clean-source checks, immutable generated inputs and build identity | Requalify changed candidates; development overrides are not release approval |
| CI and retained evidence | 2 | Passed (automated) on 9dda58f; earlier 14ee65d packaged-test failure recorded; fuller failure output now retained | Required future/final candidate checks must also pass |
| Runtime/dependency/license audit | 2 | Partial: inventories, immutable notices and MPL source; 59 review flags, 10 entries without notices; native/ABI findings documented | Resolve/appraise notice applicability, data/asset provenance and external runtime dependencies |
| Clean installed desktop workflow | 3 | Unverified | Owner/second supported environment: actual package installation, launcher, provider, real reply |
| Real app upgrade/rollback and schema backup restore | 3 | Partial: automated migration/installer tests; manual installed workflow open | Owner/engineering use disposable profile and actual old/new packages |
| Minimize/restore | 4 | Unverified in gate record | Owner runs documented native procedure |
| Suspend/resume during work | 4 | Unverified in gate record | Owner runs documented native procedure; verify reconnect and no duplicate client dispatch |
| Physical monitor unplug/replug | 4 | Open: layout regression covered, physical transition not recorded | Owner tests actual transition and reachable splitters |
| Scale/text/layout matrix | 4 | Partial: historical observations at 125%/1.6; full matrix not recorded | Owner/engineering check both themes, enlarged text, narrow layouts, focus and popups |
| IME commit/cancel | 4 | Partial: historical pinyin commit observation | Owner verifies actual commit/cancel and candidate geometry, including path completion |
| Keyboard + Orca workflow | 4 | Partial; typed-character/caret speech previously unresolved | Engineering fix/verify; owner runs complete native workflow |
| Picker/drop/clipboard/links/launching | 4 | Partial; not fully walked on installed candidate | Owner performs real interactions; engineering fixes findings |
| Engine-unavailable native state | 4 | Automated behavior covered; native installed observation open | Owner/engineering check actionable error, cached history, and disabled dispatch |
| Cold reboot/start | 4 | Unverified | Owner records recovery and usable startup, not just window mapping |
| Onboarding/diagnostics/compatibility docs | 5 | Partial: setup/support/recovery guides, extension exclusions and report template published in source | Metadata-only diagnostics/privacy tests passed; private vulnerability route enabled/documented; native installed clipboard check, final support acceptance and new-user observations remain |
| Extended soak + real-window day | 6 | Open; 40-prompt baseline tripwire passed, historical 1500-prompt measurement exists | Engineering + owner run documented longer workloads and record app/engine resources |
| Real signing key and independently verifiable candidate | 6 | Open; signature tooling previously exercised with throwaway key | Owner manages real key; engineering builds/verifies candidate |
| RC stabilization and final release | 6/7 | Not started | Owner approves gates; engineering publishes identified signed artifacts and update/recovery policy |

## Immediate execution queue

1. Close remaining rename/mention/layout regressions and reconcile automated evidence with installed native walkthroughs; recovery behavior/messaging is audited.
2. Resolve dependency/license/provenance findings; metadata-only Copy diagnostics and pinned builds/remote CI are established.
3. Schedule owner-run clean-install and native gates while onboarding/support work proceeds.

For step details and exit criteria, use the sequenced plan. `docs/native-gate-testing.md` remains the manual procedure reference; unrecorded owner use must not be assumed to have passed a particular gate.

## 1.0.0 release (2026-10-13)

The owner declared 1.0 on 2026-10-13. The workspace version moved from `0.0.1` to `1.0.0`, the
platform statement at the top of [platforms](platforms.md) was adopted, and tag `v1.0.0` was
built and published by the `Release` workflow (`.github/workflows/release.yml`): the Linux
x86_64 archive (built, installed and probed in a scratch prefix on the runner), the macOS Apple
Silicon ZIP/DMG (strict signature check and offline probe of the extracted app) and a best-effort
Intel macOS build, with one `SHA256SUMS` and the `CHANGELOG.md` section as the release body.
The release is unsigned; no release key exists.

The gate tracker above is **not** rewritten by this: rows marked open, partial or unverified
keep that status. Items the owner deferred past 1.0 (Orca, soak and performance, second-machine
install walkthrough, signing key, license appraisal) are listed in
[`llm-docs/pipkin-v1-release-plan.md`](../llm-docs/pipkin-v1-release-plan.md).
