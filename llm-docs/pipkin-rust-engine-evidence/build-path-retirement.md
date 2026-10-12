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
