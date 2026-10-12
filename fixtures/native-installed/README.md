# Installed native qualification records

`2026-10-12-upgrade.json` records two actual Linux native package identities and installer
output from `scripts/test-native-upgrade.py`. Temporary paths in the captured output belong
to disposable test prefixes. The archives themselves are not checked in.

Reproduce with retained, trusted native archives whose hashes identify their content:

```sh
python3 scripts/test-native-upgrade.py PREVIOUS.tar.zst CURRENT.tar.zst REPORT.json
```

The current archive must contain the repository's current installer. The driver requires
different app binary hashes and equal engine schema versions, uses each artifact's actual
protocol, and traps attempted Node/npm/Pi invocations. See
`llm-docs/pipkin-rust-engine-evidence/installed-upgrade-restore.md` for assertions and limits.

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
