# Native build-path retirement

Following `676fb82`, source setup, source launcher, Linux package wrapper, Arch PKGBUILD,
release collector and macOS packaging use the built-in Rust engine. New installs reject Pi
runtime archives. Explicit rollback still validates and activates a retained historical install;
this preserves recovery without provisioning a retired runtime. Source builds remain immutable
and a failed probe leaves the last good build selected.

Removed obsolete Pi build/runtime download, launch and paired benchmark scripts. Frozen capture
tooling, source patches/notices, model snapshot and migration readers remain as provenance;
`scripts/REFERENCE-CAPTURES.md` points to the complete old harness at `76ca778`.

Local validation: **1,439 passed, zero failed, 9 ignored benchmarks**, clean workspace Clippy
and formatting. Source setup, installer upgrade/rollback/tamper, exact release collection and
macOS bundle/ZIP/DMG policy fixtures pass. Python/shell syntax checks pass. macOS fixtures
are not a native compilation or desktop claim. Linux/macOS CI now contain no Pi build job;
their native execution traps detect accidental Node/npm/Pi use.

Current user instructions describe the native default, isolated profile locations, supported
subscription sign-in and historical rollback. Experimental platform, signing, legal review and
manual accessibility/display gates remain separately labelled. No release has been published.
Final installed artifact and hosted verification remain the next step.

## Clean installed candidate

A disposable checkout of **`cc8931ccd66803341790f2b9d6a3282633c88efb`** produced the final
local native-default archive, with app source stamp **dirty: false**. The first cache-linked
build was honestly stamped dirty; after excluding the untracked cache link, the app build
script was rerun and this clean binary was rebuilt and reverified. No source changes were hidden.

The installed candidate passes real coding/tools, migration, restart, full archive restoration,
retained output/replay, versioned upgrade and explicit rollback, older-schema refusal and
uninstall data retention. All use disposable profiles and scripted providers; Node/npm/Pi traps
remain unused. Both native packages are real binaries with different hashes; the prior package
is the recorded runtime-retirement artifact. No owner install or account was changed.

Exact checksums, schema/protocol, logs and upgrade/rollback report are retained in
[`fixtures/native-default/2026-10-12-final`](../../fixtures/native-default/2026-10-12-final).
Linux [38265023304](https://github.com/last-refuge/pipkin/actions/runs/38265023304) and macOS
[38265027473](https://github.com/last-refuge/pipkin/actions/runs/38265027473) are still running
on this exact source commit. Final capability accounting and their outcomes remain open;
this records actual installed evidence without claiming complete release qualification.
