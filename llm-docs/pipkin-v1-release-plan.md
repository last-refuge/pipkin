# Pipkin v1.0: release qualification and delivery plan

## Purpose

Turn the current daily-use beta into an installable, recoverable, documented, and supportable v1.0. This is a release-hardening plan, not another broad feature milestone.

The owner reports heavy beta use, including building Pipkin itself and working on other projects. Treat that as owner-reported daily-use evidence. It does not establish clean-machine installation, other-platform support, or untested recovery/accessibility behavior.

**Recommended initial release scope:** x86_64 Arch/Omarchy, Hyprland, Wayland. Confirm this scope before implementation. Other environments remain experimental or unsupported until separately qualified.

## Owner-directed execution change (2026-10-06)

The owner accepted the remaining layout/menu regression item without further qualification and changed the
immediate priority to **a first macOS native CI build/port**, with Windows to follow only after the Mac build is
reviewed. The intended next release is 1.0; no final version/tag/publication has occurred. The original checklist
below remains the risk/evidence record, not a claim that waived or unobserved checks passed. See
`docs/macos-first-pass.md` for experimental artifact boundaries and the live gate record for actual CI outcomes.

- [x] First macOS CI build/artifact: Apple Silicon native build, core/transport/ownership checks and two scripted engine smoke checks passed in run 37482847877 on clean f52e8c2. Downloaded bundle hashes/permissions/notices were verified without executing it on Linux.
- [ ] Owner evaluates the Mac artifact; decide next Mac packaging/native changes before expanding scope.
- [ ] Windows CI/port after Mac review.
- [ ] Final 1.0 version, approval, tag and artifact publication. No experimental 0.0.1 artifact is relabelled as 1.0.

## Owner-directed v1.0 scope (2026-10-12)

With the built-in Rust engine default and Pi/Node retired, the owner redefined what remains for
1.0. The release-gate items below are **deferred past 1.0** by owner decision, not passed:
screen-reader (Orca) composer speech and workflow, extended automated soak and the day-long
real-window session (overall pressure and performance testing happens after 1.0), the
clean-install/upgrade/rollback walkthrough on a second machine, signing-key/verification
machinery, license and provenance appraisal, documentation consolidation, and the native
observation of the new feature surfaces. The gate record keeps their honest status.

The 1.0 list is now:

1. **JOB-1 / JOB-2** — managed jobs: supervisor, durable records, bounded logs, cancellation,
   completion events, the Jobs inspector (plan §9).
2. **STATUS-1 / STATUS-2** — the compact status model and the engine-fed context-usage view
   (plan §11).
3. **SUB-2 / SUB-3** — per-child Stop and Steer, then asynchronous children with launch
   receipts and durable completion delivery (plan §6).
4. **ASK-2** — the questionnaire contract: grouped questions, multi-select, previews, notes,
   background-conversation attention (plan §7).
5. **OBS-1, then OBS-2 / OBS-3** — local correlated diagnostics, then the opt-in exporter
   (plan §12).
6. **Leftovers inside finished slices** — web citation rendering and a fallback provider
   chain; goal cost limits and a pinned auditor model; subagent result artifact references;
   MCP OS-keychain storage, legacy SSE and image retention as artifacts; the live-provider
   checks each slice deferred.
7. **Owner-run native gates** on the supported desktop: minimize/restore, suspend/resume,
   cold boot, monitor unplug, the 150% scale and large-text matrix, IME commit/cancel,
   picker/drop/clipboard/links, the engine-unavailable state.
8. **Scope decision** — macOS (experimental build, no Developer ID/notarization, no
   interactive acceptance) and Windows (not supported): an explicit statement of what 1.0
   advertises.

Still post-1.0 unless pulled in: WEB-2 extraction breadth, MCP apps / remote HTML (Browse
plan), an independent job daemon, multi-goal workflows, the website.

## Evidence and working rules

Reference these documents and implementations, but verify their current state before changing code:

- `docs/native-gate-testing.md`: manual desktop, installation, and recovery gates.
- `docs/packaging.md`: packaging, engine staging, diagnostics, migration, and rollback.
- `docs/platforms.md`: support boundaries and cross-platform blockers.
- `docs/extensions.md`: native extension compatibility.
- `docs/beta.md`, `docs/scorecard.md`, `docs/rust-desktop-client-plan.md`: historical targets and evidence. Several entries are outdated or contradictory.
- `scripts/build-engine.sh`, `scripts/package.sh`, `scripts/release.sh`, `scripts/verify-install.sh`, `scripts/verify-release.sh`, `scripts/test-install.sh`.

Rules for every phase:

1. Distinguish **automated**, **observed**, **owner-reported**, **failed**, and **unverified** evidence. Record the build/engine revisions and relevant environment with results.
2. Preserve journal-before-send, engine-authoritative state, stale-event guards, and honest offline/simulated states.
3. Never turn recovery into blind prompt replay. Prompt deduplication is not a guarantee of exactly-once external tool effects.
4. Read the GPUI desktop skill before modifying views, text input, persistence, core state, or native tests. Follow `AGENTS.md`, including guarded synthetic input and desktop safety restrictions.
5. For code changes, require `cargo test --workspace`, `cargo clippy --workspace --all-targets`, and `cargo fmt --all --check`. Run applicable real-engine tests separately; ignored tests are not evidence of a pass.
6. Keep changes in focused commits. Do not modify desktop configuration, install system packages, or change display scale without approval.
7. Do not perform destructive recovery testing on the owner's working profile. Use disposable projects, profiles, and databases.

## Execution status

Phase 0 execution began on 2026-10-06 UTC. The current baseline and live tracker are in
[`docs/v1-release-gates.md`](../docs/v1-release-gates.md): 505 workspace tests passed, 33 real-engine tests
passed against the development engine, another 33 passed against the unpacked bundled engine, and the
scratch-prefix install/upgrade/rollback/uninstall checks passed. Working platform scope is recorded;
final scope/acceptance sign-off and the manual gates remain owner decisions.

## Release policy

A v1.0 candidate must have:

- No known silent loss of acknowledged drafts or submission intents.
- No known client-induced duplicate dispatch after reconnect or restart.
- No visible control that silently claims success without performing its action.
- No known misleading empty/success state for a failed operation.
- A tested install, upgrade, and recovery path on the supported platform.
- A pinned, identifiable app/engine pair and verifiable release artifacts.
- Accurate support, compatibility, and limitation documentation.
- Closed supported-platform native gates, or an explicit scope/acceptance decision for any unresolved gate. Documentation alone does not make a failed gate pass.

Original scope did not require Windows/macOS. The owner now prioritizes macOS CI/build work before the 1.0 cut;
Windows follows Mac review. Automatic updates, an embedded editor/terminal and additional agent features remain out of scope.

---

## Phase 0 — Establish the release baseline and freeze scope

**Dependency:** none.

### Tasks

- [ ] Confirm the supported platform and architecture with the owner.
- [x] Record the current Pipkin and Pi engine full revisions, toolchain, Node version, and database schema.
- [x] Capture the automated baseline, full real-engine suite results, and existing package/installer test results.
- [x] Record the owner's beta report: environments, projects/workflows exercised, duration if known, and known incidents. Do not invent measurements.
- [x] Replace stale claims such as “beta not started” with accurately attributed evidence in the current beta/native-gate records. Historical roadmap sections still need consolidation in Phase 5.
- [x] Create a release-gate record containing each gate's status, evidence, owner, and follow-up.
- [x] Establish the working freeze: fixes, qualification, onboarding/support, and release machinery. New feature requests go to the post-v1 backlog unless they resolve a release blocker.

### Deliverables

- A current release baseline and gate record, preferably maintained alongside `docs/native-gate-testing.md`.
- A short approved support statement.
- A prioritized blocker list linked to the phases below.

### Exit criteria

The team can identify what is being shipped, which engine it uses, what has actually passed, and what remains unverified. Daily-use beta evidence is recorded without overstating platform coverage.

---

## Phase 1 — Remove silent and misleading product behavior

**Dependency:** Phase 0.

### 1.1 Conversation naming

Current finding: `Command::RenameConversation` is a no-op in real mode in `crates/pipkin-core/src/state.rs`.

- [x] Verify the current Pi metadata/rename contract: no rename operation is available in the current experimental session service.
- [x] Either implement authoritative rename with pending/error/reopen behavior, or remove/disable the real-mode action with a clear explanation. Real-mode palette entry disabled; F2 explains the limitation; demo behavior preserved.
- [x] Ensure buttons, shortcuts, and palette availability agree: real-mode rename cannot open the no-op dialog.
- [x] Test failure, restart, and conversation switching. The unsupported real-mode command emits no backend/persistence effects across failed/disconnected states and selected/background/missing targets; core initialization from the catalog preserves the original labels. Existing demo naming and real/demo palette tests pass. This is automated core reopen evidence, not an installed native shortcut walkthrough. Real names are prompt-derived display labels, not engine-confirmed renames.

### 1.2 Changes-pane scan failures

Initial finding: the Pi adapter mapped `Workspace::Unavailable` to an empty changes list. Addressed in the
working tree on 2026-10-06; see `docs/v1-release-gates.md` for identified code/artifact and evidence boundaries.

- [x] Represent loading, stale, unavailable, and genuinely empty states distinctly.
- [x] Preserve the previous successful diff as stale when a refresh fails.
- [x] Surface the failure reason and provide a bounded retry/refresh path.
- [x] Clear previous-project data from the selected inspector on project/conversation changes; never preserve another project's diff as the new project's state. Snapshots remain per conversation, with generation/current-session guards.
- [x] Test Git failure, missing project directory, non-Git project, and recovery. Core/mock-engine tests, real-engine regression, packaged-engine suite, and native AT-SPI failure/stale/recovery observation passed.

### 1.3 Interrupted work and unresolved operations

- [x] Explain that engine recovery may repeat a partially executed tool even though Pipkin does not resend the prompt. Recovery notice and support guide also warn about retained/external effects.
- [x] Show a clear status/recovery path for prolonged “Stopping” or unresolved outcomes. UI explains uncertain work, Check status, bounded stop-only retry and metadata-only diagnostics; installed visual walkthrough remains in Phase 4.
- [x] Offer safe reconciliation/reconnect/diagnostics actions where supported. Automatic reconnect and one-flight status/stop checks are tested; no timer creates settlement. Recovery notice links directly to Copy diagnostics.
- [x] Test connection loss during stop, engine restart during work, and late settlement. Core tests plus development/packaged engine suites cover dropped stop/reconnect, worker crash, late completion/cancellation and conversation guards; no blind replay.

### 1.4 Recent feature regression pass

- [x] Verify fresh-conversation Changes state and restoration of older conversations. Automated real-engine and socket tests cover untouched-session behavior and reopen/switch restoration; manual visual verification on the final candidate remains part of Phase 4.
- [x] Pane/menu regression item accepted by the owner without further checks (2026-10-06). Existing automated geometry tests remain evidence; no new native window/display/layout observation is claimed.
- [x] Verify effort selection remains engine-authoritative and conversation-specific. Core/adapter tests and `choosing_effort_is_confirmed_by_pi_and_restored_per_conversation` passed with development and packaged engines; native menu walkthrough remains in Phase 4.
- [x] Verify project-scoped `@` completion: headless GPUI mouse/Tab, Ctrl-click URL dispatch, spaces/Unicode, undo/redo, project/scan-epoch guards, new/deleted paths and IME suppression/unmark tests pass; filesystem tests cover missing/permission-denied roots and excluded symlink traversal. Query-open rescans replace the stale one-time index. This closes automated behavior, not installed native mouse/IME/URI-handler qualification in Phase 4.
- [x] Confirm `@` mention semantics are transparent: the first-run guide explicitly distinguishes inserted path references from file-content attachments. Native completion/IME qualification remains separate.

### Exit criteria

No known silent no-op remains in the advertised workflow. Changes failures cannot masquerade as “No changes yet.” Recovery states describe what is known and offer safe next steps. Targeted regressions and applicable real-engine tests pass.

---

## Phase 2 — Make builds and qualification repeatable

**Dependency:** Phase 0; may proceed alongside Phase 1. Must finish before qualifying the release package.

### Tasks

- [x] Pin the exact Pi engine full revision used by the release rather than implicitly packaging whichever adjacent checkout exists. `packaging/pi-engine-revision` pins `d2a311097cbcf669e699479587332ae3988a49d0`.
- [x] Make release builds reject dirty/unidentified engine sources, or use a clean checkout created from the pin. Dirty/untracked or mismatched revisions are rejected before staging; `PIPKIN_ENGINE_DEV=1` is explicit and recorded in the manifest.
- [x] Record app version/revision, full engine revision, protocol/compatibility information, and relevant build inputs in the manifest/diagnostics. Compiled app stamp appears in `--version`/`--diagnose`; packaged `build-info.json` records identities, hashes, compatibility and tool versions with explicit dirty/custom-flag state. Release builds reject dirty/unidentified app sources.
- [x] Verify required generated engine assets and production dependencies exist. Generated provider data is validated against the source contracts, copied explicitly and fingerprinted. The staged engine passed the full scripted-provider suite; dependency provenance/license auditing remains separate.
- [x] Add CI for Rust tests, Clippy, formatting, and installer tests. Pinned-action workflow passed remotely in runs 37421406544 (528457c) and 37422216176 (12c9dba); future/final candidates still require their own pass.
- [x] Add a real-engine CI qualification job using the pinned engine and scripted provider, without credentials or paid requests. Development and packaged engines, clean locked provisioning, immutable generated inputs and bare probe passed in remote runs 37421406544 and 37422216176; see the gate record.
- [x] Test the packaged engine, not only the development checkout; run `verify-install.sh --full` or its equivalent. The pinned/staged engine passed 37 suite tests; candidate changes still require fresh qualification.
- [ ] Audit bundled licenses/provenance and runtime dependencies. Partial: locked/staged license inventories, immutable notice inputs and non-executing native/ABI inventory are packaged; 59 license review flags, 10 missing notices and native/static/foreign-component findings remain. See `docs/bundled-licenses.md`.
- [x] Ensure release artifact collection cannot accidentally include obsolete packages from previous builds. `release.sh` collects only the current version/architecture, rejects the development engine override, and passes disposable collection tests.

### Exit criteria

A clean build can produce and test the same identified app/engine pair without relying on the owner's development checkout state. Compatibility rejection is tested. Required CI checks are established and green.

---

## Phase 3 — Qualify clean installation, upgrade, and rollback

**Dependencies:** Phases 1 and 2.

Use a second supported machine when available, otherwise a fresh supported user/system environment. A scratch-prefix check is useful but is not a substitute for the desktop install gate.

### Tasks

- [ ] Install the actual package without an adjacent Pi checkout or development environment.
- [ ] Launch from the desktop launcher; verify icon, window identity, and engine discovery.
- [ ] Run `pipkin --diagnose --probe` from the installed copy.
- [ ] Configure a provider through the documented workflow and obtain a real answer.
- [ ] Exercise a real project with file-changing work and inspect the diff.
- [ ] Save a draft with attachments, quit, reopen, and verify it.
- [ ] Upgrade with saved drafts/history present; verify the new bundled engine is used and data survives.
- [ ] Test rollback, newer-schema refusal, and restoring the correct backup in a disposable profile.
- [ ] Test the generic installer's upgrade/rollback/uninstall behavior on the supported environment if it will be offered for v1.0.
- [ ] Confirm uninstall preserves user data and all required dependencies are documented.

### Exit criteria

A person can install, launch, use, upgrade, and recover Pipkin from the shipped artifact without source checkouts. Data-preservation and rollback results are recorded with package versions and environment details.

---

## Phase 4 — Close supported-platform native gates

**Dependencies:** Phases 1–3. Use the installed candidate where possible.

### Required checks

- [ ] **Minimize/restore:** draft, caret focus, transcript scroll, and rendering survive.
- [ ] **Suspend/resume during work:** reconnect or an honest offline state; draft intact; no duplicate client dispatch.
- [ ] **Monitor unplug/replug:** pane edges remain reachable; layout recovers as the window/viewport shrinks and grows.
- [ ] **Scale and text:** both themes, enlarged text, narrow layouts, focus rings, menus, and path completion at the supported scales. Record actual scale values, including compositor snapping.
- [ ] **IME:** actual compose, candidate selection, commit, and cancel; candidate geometry follows the caret; Enter does not submit during composition.
- [ ] **Keyboard:** complete normal workflow, menus, completion, attachments, queue controls, dialogs, and recovery without a pointer.
- [ ] **Orca:** typed-character/caret speech and a full workflow through navigation, composer, transcript, tools, and status announcements. This is a known partial/failing gate until demonstrated otherwise.
- [ ] **Native interactions:** file picker, real drag-and-drop, clipboard image/text, file links, editor/terminal launch, and extension questions.
- [ ] **Unavailable engine:** actionable startup error, readable saved history, and no misleading ready/send state.
- [ ] **Cold launch after reboot:** record recovery and time to usable UI, not only window mapping.

### Gate policy

Fix supported-platform failures before release. If a gate cannot be closed, obtain an explicit decision to defer/change the acceptance criterion and document the limitation prominently. Do not advertise an accessibility or IME workflow as supported solely because its API exists.

Display-presentation latency is a measurement follow-up unless a specific latency guarantee is part of the release promise. Do not substitute CPU frame submission for presentation measurement.

### Exit criteria

The supported desktop workflow passes the agreed native gates. Every remaining limitation has an explicit acceptance decision and accurate public wording.

---

## Phase 5 — Finish onboarding, support, and compatibility documentation

**Dependencies:** Phase 1 for behavior; Phases 3–4 for final installation/platform claims. Drafting can happen earlier.

### Tasks

- [x] Document first-run provider setup, missing credentials, expired authentication, and model refresh. `docs/getting-started.md` describes the current workflow; clean-desktop/new-user observation remains open.
- [x] Add an in-app **Copy diagnostics** action with clear privacy expectations. Ctrl K copies an allowlisted metadata snapshot, never logs/errors/paths/session text; hostile-manifest and headless clipboard tests passed. Detailed CLI diagnostics remain review-before-sharing; native installed clipboard observation remains separate.
- [x] Document locations and ownership of drafts, sessions, credentials, caches, backups, and logs. `docs/support.md` distinguishes app cache/journal, engine profile and agent credentials; `--data-dir` alone does not isolate the engine.
- [x] Document what quitting does to active engine work and how to recover after interruption. Owned versus external engines and possible repeated partial tool effects are explicit; recovery tests/native gates remain separate.
- [x] Explain workspace-wide Changes semantics, partial cached-history search coverage, and `@` references versus attachments. `docs/getting-started.md` publishes these boundaries.
- [x] Publish the extension compatibility matrix. `docs/extensions.md`, README and the first-run guide explicitly exclude stable Pi `ctx.ui.*`/TUI routing from the current promise; the experimental remote question contract is retained.
- [ ] Publish the exact supported platform, engine compatibility, runtime dependencies, and unsupported environments. Working scope/protocol/schema/runtime requirements are now explicit in `docs/platforms.md`; final supported-scope approval and native qualification remain open.
- [ ] Replace historical/contradictory status claims with current evidence; keep old measurements labelled historical. Beta/privacy, platform claims and the scorecard are consolidated against the gate record; remaining roadmap/status references need final review.
- [ ] Update README commands, repository links, screenshots, and version/support labels.
- [ ] Observe several new users going from installation to first reply where feasible; record stalls and fix blocking onboarding problems. Do not fabricate a participant count or completion rate.
- [x] Provide a bug-report template and documented diagnostics/recovery procedure. `.github/ISSUE_TEMPLATE/bug-report.md` and `docs/support.md` warn that current CLI reports contain unredacted log/error text. Share-safe in-app diagnostics are implemented/tested. GitHub private vulnerability reporting is enabled and documented in `SECURITY.md`; no response SLA or approved-release maintenance window is claimed.

### Exit criteria

A new user can follow the installation/setup instructions without undocumented developer knowledge. Support reports can identify the app/engine pair safely. Public claims match tested behavior and qualification results.

---

## Phase 6 — Cut and stabilize a signed release candidate

**Dependencies:** Phases 1–5.

### Tasks

- [ ] Set consistent release-candidate versions in Cargo, package metadata, manifests, and release output.
- [ ] Build from clean, pinned sources; retain an immutable record of the app/engine pair.
- [ ] Run all required CI, the full real-engine suite, installer checks, and packaged-engine qualification against that candidate.
- [ ] Run the extended automated soak with a documented round count and workload.
- [ ] Run a real-window day-long session; measure app and engine memory/file descriptors, and distinguish initial cache growth from sustained growth.
- [ ] Recheck crash/reconnect, saved-draft recovery, unknown submission, storage-write failure, and upgrade paths in disposable profiles.
- [ ] Establish the real release signing key, safe key handling, and independently published fingerprint.
- [ ] Generate checksums/signatures; download and verify on a separate environment, including a tampered-artifact rejection check.
- [ ] Publish the candidate, known limitations, install/update/rollback instructions, and changelog.
- [ ] Complete a bounded stabilization period with the owner and available testers. Fix release blockers only; rebuild/requalify any changed candidate.

### Exit criteria

The candidate is identifiable, downloadable, verifiable, installable, and stable in the agreed workflows. There are no unresolved release-blocking incidents. Qualification results refer to the artifact being approved, not an older build.

---

## Phase 7 — Publish v1.0 and establish maintenance

**Dependency:** Phase 6 accepted by the owner.

### Tasks

- [ ] Review and sign off the gate record and known limitations.
- [ ] Set the final `1.0.0` version consistently and tag the exact sources.
- [ ] Build/test the final artifacts; do not merely rename an artifact carrying an RC version.
- [ ] Publish signed artifacts, checksums, verification instructions, and the release notes.
- [ ] Publish explicit supported-platform and extension-compatibility statements.
- [ ] Document the manual update process and tested rollback procedure. An in-app updater is not required.
- [ ] Define how bugs/security reports are received and how app/engine compatibility changes are released.
- [ ] Retain the prior working package and recovery instructions.
- [ ] Move non-blocking issues to a prioritized post-v1 backlog.

### Exit criteria

v1.0 is available through a documented release channel, can be verified by users, and has a practical support and update/recovery policy.

## Post-v1 backlog — explicitly not required for this release

- Windows/macOS transports, process ownership, native input/accessibility, installers, and signing/notarization.
- GNOME/KDE, pure X11, other Linux distributions, and additional architectures.
- Provider speed/priority-tier controls.
- Automatic update checks/installations.
- Compiled/smaller engine packaging; measure current artifact size before setting a target.
- Engine-wide search beyond locally cached history.
- Additional extension/TUI compatibility beyond the v1 contract.
- Embedded terminal/editor, cloud/team features, and patch reversion.
- Additional performance tooling and display-presentation measurements.

## Execution order at a glance

```text
0. Scope, baseline, and evidence
          |
          +--> 1. Product/recovery hardening --+
          |                                    |
          +--> 2. Pinned builds and CI --------+--> 3. Installed-product qualification
                                                       |
                                                       v
                                                 4. Native gates
                                                       |
                     5. Onboarding/support drafts -----+--> Finalize docs
                                                       |
                                                       v
                                                 6. Signed RC + soak
                                                       |
                                                       v
                                                 7. Publish v1.0
```

## Final go/no-go checklist

- [ ] Supported platform and release scope are approved.
- [ ] Silent no-ops, misleading empty states, and known client-induced data-loss/duplicate-dispatch defects are closed.
- [ ] Recovery semantics are tested and explained, including partial tool execution.
- [ ] Clean install, upgrade, rollback, and data preservation pass.
- [ ] Required native gates pass, or scope/criteria changes are explicitly accepted and disclosed.
- [ ] App and engine are pinned; the packaged pair passes required automated and real-engine checks.
- [ ] Soak/stabilization evidence is recorded for the candidate.
- [ ] Onboarding, diagnostics privacy, support, and compatibility documentation are accurate.
- [ ] Signed artifacts and verification/rollback instructions are published.
- [ ] The owner approves v1.0.
