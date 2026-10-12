# Fork migration and installed coding workflow

2026-10-11, following `c508954`. Protocol 11 / schema 9 unchanged.

## Implemented

The importer previously refused every conversation with a Pi `parent` relation. It now
materializes visible ancestor entries through the inclusive minimum of each intervening
fork cutoff. Ownerless forks become independent native roots; task ownership determines
subagent parent/root relations and usage aggregation separately from history ancestry.
Each branch keeps its own current documents, including settings Pi copied at the historical
fork point. Original ancestry and source records remain in provenance and the source backup.

Missing ancestors, missing or invisible cutoff entries, malformed relations and cycles
refuse the entire import. Unsupported context semantics and in-flight work still refuse;
this does not activate extensions or add a new native branch-creation API. Inherited user
entries do not acquire another conversation's durable request identity. Source entry counts
exclude inherited copies; the manifest separately reports materialized native entry counts.

The stale native diagnostic saying Claude subscription sign-in was unavailable is removed.

## Executed reference and native tests

`scripts/capture-fork-import-reference.mjs` executes the pinned Pi revision
`d2a311097cbcf669e699479587332ae3988a49d0` through its real Node SQLite adapter,
fork-document selector/copy operations, context bounds and context derivation. It creates
only synthetic records in a temporary profile, preserves the resulting closed SQLite
database, and records source hashes and expected contexts in `fixtures/native-fork-import/`.
The original sources are used without changing their function bodies. The fixture database
SHA-256 is `5e74641cf2db93a083793d9dd354e30fe238ca537797c4497123e15ac6aa66f3`.

Five new Rust tests cover:

- Seven imported contexts compared with executed Pi results: the original root, direct and
  nested forks, a rewind into an ancestor, inherited missing tool results, omissions,
  compaction, reset and a branch without local entries. As-of settings differ correctly
  from the parent's later settings. Source bytes and idempotence are checked.
- Failed import leaves no native records or mapping; retry imports all branches.
- Invalid/cyclic ancestry refuses without a partial import.
- Fork ancestry and subagent ownership can point to different conversations; root task
  grouping and usage totals follow ownership without duplicating ancestor spend.
- Native protocol import, engine restart, independent catalog listing, complete paged
  history, and actual provider requests continuing each of the seven contexts.

All 40 importer/import-runtime tests passed in `/tmp/pipkin-fork-import-all2.log`.
Final full workspace checks: **1548 passed, 0 failed, 53 ignored**, clean Clippy and formatting.
Logs: `/tmp/pipkin-fork-workspace2.log`, `/tmp/pipkin-fork-clippy2.log`,
`/tmp/pipkin-fork-fmt2.log`. Earlier failed test builds used incorrect test-helper API names;
the initial capture also used `readDocument` instead of Pi's `document` method. These were
harness development failures, corrected before the passing results.

## Installed package and CI

`scripts/test-native-package.sh` now invokes `scripts/test-installed-native-workflow.py`
against the installed executable under its scratch prefix (including spaces in the path).
The workflow uses a scratch HOME/config/project and synthetic credentials with a loopback
scripted OpenAI-compatible HTTP provider. It imports the real Pi fixture database, continues
one nested branch, and executes real built-in `write`, `edit`, `bash` and `read` operations.
Assertions check the edited Rust file, tool-result messages and expected inherited context.
After process restart, the original durable key returns its settled outcome without another
model request or shell effect; a new key continues with the retained tool history. Reimport
is a no-op and source files retain their bytes. This is an installed real-tool workflow,
not a live model or GUI acceptance test. An initial harness assertion expected string
content instead of legal text blocks; normalizing the wire projection fixed that assertion
(`/tmp/pipkin-installed-workflow-first.log`).

The final Linux x86_64 native-only package built and passed the expanded package checks,
including Node/npm/npx/Pi trap shims, install/uninstall, notice retention and data preservation.
It identifies `c508954` with a dirty tree. Archive SHA-256:
`7eabb9dcbe0f9f74da4cf7459d079cc70fcdd4ca45a8a38b873376b0e43d4f58`.
Installed tree: **360122888 bytes**; archive: **75783817 bytes** (unstripped).
Logs: `/tmp/pipkin-fork-package-build2.log`, `/tmp/pipkin-fork-package-checks2.log`.
This is not a clean release comparison, platform qualification, or proof of default cutover.

`.github/workflows/qualification.yml` adds an independent native job which provisions no Pi
checkout or Node setup, puts failing dependency shims on the build PATH, builds the locked
native package, runs the installed workflow and retains identity/inventory/log evidence.
The YAML parses locally; **no GitHub runner execution has been observed**. That gate remains
unverified. The existing Pi qualification path remains until cutover is justified.

Remaining migration boundaries include unsupported replacement-context imports and broader
desktop/rollback acceptance. Other provider, sustained-use, release and platform gates remain
open; this evidence does not qualify all parity rows or switch the default.
