# Installed native qualification records

`2026-10-12-upgrade.json` records two actual Linux native package identities and installer
output from `scripts/test-native-upgrade.py`. Temporary paths in the captured output belong
to disposable test prefixes. The archives themselves are not checked in.

Reproduce with retained, trusted native archives whose hashes identify their content:

```sh
python3 scripts/test-native-upgrade.py PREVIOUS.tar.zst CURRENT.tar.zst REPORT.json
```

The current archive must contain the repository's current installer. The driver requires
different app binary hashes and a current schema at least as new as the previous artifact,
uses each artifact's actual protocol, and traps attempted Node/npm/Pi invocations. Equal
schemas exercise compatible rollback. Increasing schemas exercise real migration, unchanged
older-worker refusal and opening the pre-upgrade backup with the old worker. See
`llm-docs/pipkin-rust-engine-evidence/installed-upgrade-restore.md` for assertions and limits.

`2026-10-12-schema10-upgrade.json` records the actual schema-9 → schema-10 path, continued
coding and closed-profile restoration. All saved records in the older restored profile stay
equal except the expected startup event epoch increment; the migration backup stays intact.
See `llm-docs/pipkin-rust-engine-evidence/tool-call-identities.md` for the schema change.

`2026-10-12-cross-engine.json` identifies actual matched Pi/native packages used by the
headless installed migration/rollback test. It covers closed desktop/native profile archive
restoration and explicit ownership release before Pi resumes. With a scratch HOME, cleared
environment, explicit Cargo/Rustup caches and trusted local archives, reproduce with:

```sh
PIPKIN_PI_REPO=PINNED_ENGINE PIPKIN_PI_PACKAGE=PI_ARCHIVE PIPKIN_NATIVE_PACKAGE=NATIVE_ARCHIVE \
  cargo test -p pipkin-app installed_pi_native_rollback_preserves_history_and_requires_ownership_release -- --ignored --nocapture
```

`2026-10-12-package-comparison.json` records paired package sizes and exact build identities.
Both archives must contain an identical app binary/revision stamp. The comparator checks
native runtime-payload absence and fails if either provisional reduction target is missed:

```sh
python3 scripts/compare-native-package.py PI.tar.zst NATIVE.tar.zst REPORT.json
```

These are actual local Linux x86_64 artifacts, not visual desktop or hosted CI observations.
See `llm-docs/pipkin-rust-engine-evidence/installed-pi-rollback.md` for assertions and limits.

`2026-10-12-schema10-cross-engine.json` repeats that installed Pi/native workflow with the
schema-10 identity fix and the paired benchmark's exact new artifacts; all assertions pass.
