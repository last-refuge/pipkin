# Compaction settings reference

The 46 cases in `reference.json` execute the pinned Pi SettingsManager's actual in-memory
storage, migration, merge and compaction getters under Node 26.8.1. The capture verifies
revision `d2a311097cbcf669e699479587332ae3988a49d0` and unchanged source files, and records
SHA-256 identities. Temporary copies rewrite import paths; production function bodies are
unchanged. Unused filesystem/network helpers throw if called. The real text/BOM and retry modules are
copied unchanged, including the exported default agent retry delay. The unrelated HTTP timeout
constant is not exercised or qualified.

Cases cover ordinary values, exact model keys, selected/unselected malformed overrides,
ordinary-before-override error order, per-field precedence, project merges and replacements,
null/nonobject policy shapes, zero/integral values and JavaScript safe-integer boundaries.
Project null replacement and retry getter defaults are compared as well.
`enabled` is normalized to consumer boolean truthiness. Default provider/model values prove
that one invalid compaction setting does not discard unrelated settings. Native compares
both successful policies and exact error messages, plus provider/model preservation.

This is not a complete application execution, filesystem locking test or provider run.
The native runtime test separately verifies failed generation/manual compaction before
provider calls, a live project correction, preserved unrelated settings and summary budget.

Regenerate from the pinned read-only checkout:

```sh
node scripts/capture-compaction-settings-reference.mjs PI_SOURCE fixtures/native-compaction-settings/reference.json
```
