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
