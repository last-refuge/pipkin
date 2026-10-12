# Native-default runtime candidate

2026-10-12, following `76ca778`. Ordinary application startup, `--version`, and
`--diagnose --probe` select the built-in engine. `--engine native` remains an equivalent
explicit alias; native child/in-process hosting and external attachment remain available.
Demo mode remains explicitly simulated. Retired Pi launch flags fail with migration/native
attachment guidance instead of silently choosing a different engine.

The Pi runtime adapter and `pi-client` crate are removed from source, workspace members,
application dependencies and Cargo.lock. Native sessions retain the shared stable desktop
identity algorithm. The import source, source-profile namespace, credentials/history/config
readers, ownership records for older-installation rollback, raw imported records and backups
remain. No user profile is changed by this source change. Import remains explicit.

The full legacy desktop mapping was captured and compared equal before removing the Pi
adapter. Its synthetic golden at `fixtures/native-transcript/retired-pi-history.txt` retains
all mapped item IDs, ordering, tool statuses and payloads as an ordinary native test.
Historical paired/rollback drivers and the retired runtime sources remain recoverable at
`76ca778`; their recorded measurements and frozen fixtures remain in the repository. Tests
specific to the removed client/adapter/Node discovery are retired with that implementation;
native workflow, migration, lifecycle, provider and core tests remain required.

This is the application runtime cutover candidate. Legacy setup/package/release scripts and
CI jobs still require their separate removal/update, followed by final release-candidate
qualification. This document does not claim that the entire retirement goal or publication
is complete. Experimental macOS/X11 and general desktop manual limitations remain recorded.

Validation: **1,439 workspace tests passed, zero failed, nine ignored benchmarks**.
Controller checks pass after the final wording/style cleanup; Clippy/all-targets and fmt are
clean. The installed native-default candidate passes plain --version/--diagnose --probe,
explicit native aliases, migration, real coding tools against a local scripted HTTP provider,
restart, retained full output, durable replay, complete closed native archive restore,
uninstall/data retention and unused Node/npm/npx/Pi trap shims.
[Raw logs and identity](../../fixtures/native-default/2026-10-12/report.json) identify the
`76ca778` dirty runtime candidate, not a published or final release build. Archive 76,945,467
bytes; installed tree 365,128,355 bytes; 613 Rust inventory entries, no npm packages.
The final release/build-path batch will rebuild and qualify its own identified artifact.
