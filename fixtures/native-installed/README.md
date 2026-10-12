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
