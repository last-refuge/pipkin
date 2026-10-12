# Installed native upgrade, rollback and archive restore

2026-10-12, following `cf32a31`. Native remains opt-in; engine protocol 12/schema 9
are unchanged. This is Linux x86_64 headless installed qualification, not desktop acceptance.

## Defect and fix

The installer previously switched to a retained rollback directory without rechecking its
binary or asking its engine to start. A new scratch regression failed before the fix:
`FAIL rollback activated a tampered prior binary`. Rollback now verifies the retained app
checksum, native/legacy package shape, and the private Node checksum for a legacy package,
then requires an offline handshake in a disposable profile before changing the current
symlink or launcher. A failed check leaves the selected working package intact. Installation
and rollback share the same offline probe function.

`scripts/test-install-paths.sh` now passes 16 checks. It exercises altered app/runtime
payloads, a checksum-valid fake package whose probe fails, unchanged selection/launcher
after rejection, successful native-to-Pi launcher rollback, and previous installer checks.
The Pi branch here uses fixture executables; it is not an installed real Pi qualification.

## Real package workflow

`scripts/test-native-upgrade.py PREVIOUS_ARCHIVE CURRENT_ARCHIVE REPORT.json` installs two
different real native builds into one scratch prefix containing spaces. The older archive
uses its own bundled installer; later upgrade and rollback use the current archive's
installer, checked against the source. Exact archive/binary hashes, revisions, markers,
installer hash and installer command output are retained in
`fixtures/native-installed/2026-10-12-upgrade.json`.

- Previous: retained candidate based on `c508954`, engine protocol 11/schema 9.
- Current: candidate built from `cf32a31` plus this batch, engine protocol 12/schema 9.
- Both were built from dirty trees; the full binary/archive hashes identify the tested
  artifacts rather than implying a clean commit-only reconstruction.
- Current app SHA-256: `2af370ba6a8a28499692276f91977f7394cca094617bb83a8dd94745228fbaff`.

The driver uses the expanded `scripts/test-installed-native-workflow.py` to verify:

1. Import all seven fixture fork conversations without rewriting the source, select the
   local HTTP provider, and execute real write/edit/bash/read tools in the project.
2. Receive exactly the reference-derived user context for the imported branch. The bash
   tool makes one observable effect and produces 1 MiB plus a suffix of retained output.
3. Close the worker, upgrade the installed package, reopen, replay the settled request
   without repeating tools/provider calls, and complete a new turn with prior tool context.
4. Refuse a deliberately damaged retained real binary without changing the current target
   or native launcher, then restore that test binary and successfully roll back.
5. Reopen with the older package, compare all visible fixture entries, and complete another
   coding turn. Separately, give that older installed worker a copied database with a newer
   schema: it refuses startup and leaves the database hash unchanged.
6. With the worker closed, create a private complete native data/config archive, retain the
   original directories, and restore to the same paths using permission-preserving tar.
   Every restored file hash and file/directory permission matches; SQLite integrity passes.
7. Remove temporary spill files, reactivate the newer native package, compare restored
   entries, retrieve the full 1 MiB tool output with zero dropped bytes, and verify all three
   settled request outcomes. Replaying the original request still produces no extra effect.
8. Continue after restore with all four tool results still in context. Project changes
   remain in place, the one bash effect still appears once, reimport reports already imported,
   original Pi fixture files remain byte-identical, and uninstall succeeds.

Node/npm/npx/Pi trap executables stay unused throughout the native workflow. A real local
HTTP provider supplies scripted replies; no account, credential import or live provider
is exercised. The same-build archive/restore test is now part of every native package
verification; the two-artifact driver requires a retained earlier native package.

## Verification and documentation

Native package build and `scripts/test-native-package.sh --no-build` pass with the new
archive workflow. The two-artifact upgrade driver passes with the current bundled installer.
Shell/Python syntax and diff checks pass. Required workspace checks pass: **1,638 passed,
0 failed, 54 ignored**, clean Clippy with all targets and formatting.

The first archive test used the wrong wire field spelling for `ToolOutput`; correcting
`callId` to `call_id` fixed the test. Python's safe tar extraction filter then changed
directory modes, so the fixture switched to the actual trusted-archive GNU tar procedure
with `--preserve-permissions`; byte and permission assertions now pass without relaxation.

Installed help now correctly exposes existing Claude subscription sign-in and explains
rollback checks. Support documentation describes private archives, permission preservation,
same-path restoration, separate project/Pi backups and the distinction between transient
spill paths and durable retained tool output.

## Remaining qualification

This closes the defined native archive/restore and compatible native package upgrade/rollback
workflow. It does not establish full desktop-profile restoration (drafts, cache, journal,
ownership UI), rollback into an actual installed Pi runtime, cross-schema downgrade or
universal archive relocation. The artificially newer-schema test proves refusal, not a
migration between two real schema versions. Real credentials, account refresh, visual history
labels and desktop release acceptance remain separate gates. No ledger row is promoted to
fully qualified and no default/runtime-removal switch is made here.
