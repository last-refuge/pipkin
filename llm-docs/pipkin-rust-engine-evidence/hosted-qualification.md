# Hosted native qualification

The committed migration is backed up on `origin/codex/native-engine-qualification`.
The initial candidate is `ca8c89b`; completed local evidence through `0c8c3f5` is also pushed.
The user granted the missing GitHub workflow scope after the initial rejected push.

- Linux run [38259320193](https://github.com/last-refuge/pipkin/actions/runs/38259320193):
  native engine tests and release compilation passed. Offline notice collection failed on
  missing bit-set 0.8.0 from the fresh cache; installed checks were not reached. The package
  script now fetches the locked target dependency graph before building. Local package/install
  migration, coding and restart checks pass; hosted rerun is required.
- Experimental macOS run [38259324745](https://github.com/last-refuge/pipkin/actions/runs/38259324745):
  native engine compiled; library checks reported 840 passed, nine failed, two ignored.
  Five failures shared the socket handshake path. Two exposed case-insensitive config
  discovery duplicating AGENTS.md/AGENTS.MD. One test assumed a case-sensitive pathname;
  one process-cleanup test incorrectly used Linux /proc on every Unix target.

The follow-up explicitly makes accepted streams blocking before handshake/reader threads,
retaining existing bounded handshake/write deadlines. A delayed-Hello/later-call regression
exercises an initially nonblocking socket on Linux too. Configuration discovery/import now
uses real directory entry names, preserving distinct case variants on case-sensitive
filesystems while avoiding invented aliases on macOS. Path expectation follows filesystem
lookup behavior; the non-Linux Unix process test checks named non-zombie sleep via ps.
No production process-cleanup policy or peer-credential checks are weakened.

Local engine tests pass (853 passed, two ignored), and the full workspace passes
**1,657 tests, zero failed, 56 ignored**, with clean Clippy/all-targets and formatting.
These are Linux results; macOS remains failed pending the corrected hosted run. A
`native_only` manual workflow option avoids rebuilding the transitional Pi package during
native iteration; normal push/PR behavior is unchanged.

User-directed execution policy: finish conversion on the supported Arch/Omarchy Wayland
scope; reuse existing successful evidence and pursue concrete release blockers. Stop adding
soaks or expanding experimental-platform acceptance. Push each completed batch. Broader
application 1.0 signing/publication/manual-platform work is separate from this conversion;
no release publication is claimed here.

Cutover preparation after `9a8d0eb`: required local checks pass **1,659 tests, zero failed,
54 ignored**, clean Clippy/all-targets and formatting. Read/edit reference corpora are now
ordinary tests. Shared native/Pi desktop helpers are decoupled without behavior changes.
Both native jobs retain build caches even after failures; Linux gains the native-only manual
option too. The successful local package is still opt-in and not a final native-only release.

Follow-up: Linux native-only run [38262069375](https://github.com/last-refuge/pipkin/actions/runs/38262069375)
executes the package fix on `252938a`; the superseded run's legacy job was cancelled to
avoid duplicate Pi work. Corrected macOS run [38260978574](https://github.com/last-refuge/pipkin/actions/runs/38260978574)
passes engine/protocol/process and worker/diagnostic checks. One native adapter reconnect
test fails (94 pass): it accepts an old Ready before processing disconnect. The test now
observes disconnect before restarting and locally passes. Full follow-up workspace:
**1,664 passed, zero failed, 54 ignored**, clean Clippy/fmt. See
[focused assertions](final-parity-assertions.md). Hosted macOS rerun remains required.

## Corrected hosted outcomes

- Linux [38262069375](https://github.com/last-refuge/pipkin/actions/runs/38262069375)
  on `252938a` **passed**, including actual native packaging, installed migration/coding/restart,
  dependency notices and unused Node/npm/Pi traps. Completed 2026-10-12 14:36:58 UTC.
- macOS [38262827277](https://github.com/last-refuge/pipkin/actions/runs/38262827277)
  on `76ca778` **passed** engine/protocol, actual worker/diagnostics and native adapter tests.
  This run did not package the app or qualify interactive macOS behavior.

These close the specific earlier hosted failures; they are not the final native-default artifact.
The build-path retirement replaces both legacy Pi CI jobs with native-only checks, full workspace
checks on Linux and actual native app/ZIP packaging on macOS. The final candidate needs its own run.

## Native-only packaging candidate

macOS [38265027473](https://github.com/last-refuge/pipkin/actions/runs/38265027473)
on `cc8931c` **passed**, including native engine/worker/adapter tests, actual release build,
target-specific notices, ad-hoc signed native app/ZIP, extracted signature and isolated
plain diagnostic handshake. No Pi/Node invocation occurred. This is automated native
packaging evidence, not Finder/Gatekeeper or interactive macOS acceptance.

The final audit subsequently found and fixed offline saved-conversation reattachment in
`4b42079`. Linux [38266456562](https://github.com/last-refuge/pipkin/actions/runs/38266456562)
and macOS [38266459911](https://github.com/last-refuge/pipkin/actions/runs/38266459911)
qualify that production fix. Do not substitute the prior green run for their outcomes.

## Recovery candidate outcomes and fixture correction

macOS [38266459911](https://github.com/last-refuge/pipkin/actions/runs/38266459911)
on **`4b42079` passed**, including engine/worker/adapter checks, release compilation,
native app/ZIP packaging and extracted signature/diagnostic verification. This is automated
experimental-platform evidence; interactive macOS acceptance remains unverified.

Linux [38265023304](https://github.com/last-refuge/pipkin/actions/runs/38265023304)
on the earlier `cc8931c` candidate passed. Final Linux
[38266456562](https://github.com/last-refuge/pipkin/actions/runs/38266456562) failed in
`adapters::workspace::tests::collecting_never_modifies_the_repository`: the before snapshot
contained `.git/objects/maintenance.lock`, which detached Git maintenance from the fixture's
setup commit removed before the after snapshot. All other files' lengths/mtimes agreed.
The failure precedes packaging and is retained, not counted as a successful qualification.

The fixture Git helper now disables automatic maintenance and garbage collection for setup
commands. The collector and full repository snapshot assertion are unchanged; no production
code changes. Local required checks pass: **1,446/0/9 ignored**, clean Clippy/fmt.
A new Linux run must pass before completion. The
successful final macOS run and clean local recovery package retain the same production code.

## Final conversion qualification

Corrected Linux [38267642502](https://github.com/last-refuge/pipkin/actions/runs/38267642502)
on **`0c3a3034d073aa3c5755a0c0c4761516eb21e440` passed every step**: 1,446 workspace tests,
zero failures, nine ignored benchmarks; Clippy/formatting; policy fixtures; actual native
package and installed migration/coding/restart/archive restoration/durable replay. Node/npm/
npx/Pi traps were unused. The earlier failed run remains recorded above.

Final macOS **38266459911** also completed successfully. The only code delta from its
`4b42079` revision to `0c3a303` is the `cfg(test)` Git setup correction. Production sources,
Cargo dependencies, build scripts and workflows are unchanged. Both binaries were built
from clean checkouts, with their distinct target/source stamps preserved.

Job metadata, package logs, build identities and checksums are committed in
[`fixtures/native-default/2026-10-12-recovery-final`](../../fixtures/native-default/2026-10-12-recovery-final).
This closes hosted conversion qualification. No interactive macOS, publisher signing,
notarization or release publication is inferred from automated CI.
