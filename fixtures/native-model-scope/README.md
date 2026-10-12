# Executed reference model scoping

Captured with `scripts/capture-model-scope-reference.mjs` against Pi commit
`d2a311097cbcf669e699479587332ae3988a49d0`, Node 26.8.1, locked minimatch 10.2.6
and brace-expansion 5.0.12. Each JSON records source and dependency hashes, versions,
locale, input models/patterns, and actual ordered selections, thinking levels and warnings.

The capture executes `resolveModelScopeFromModels` from the pinned source and the actual
thinking-level validator. Unused formatting throws if called; chat-model equality and the
medium default are supplied locally. It does not run the complete Pi service or desktop.
Dependencies are loaded from a separately installed matching reference; none are installed
by this script or included in native packages.

- `reference.json`: 304 cases under en-US. Exact/fuzzy matching, ambiguous IDs, aliases,
  dated models, colon suffixes, levels, warnings, ordered deduplication, hidden components,
  globstar, negation, escapes, braces/sequences, extglobs, character classes, UTF-16 and
  deterministic composed patterns.
- Four `ordering-*.json` files: 28 cases each under sv-SE, tr-TR, de-DE and th-TH.
  Both input orders test stable sorting when collation considers IDs equal.

Reproduce with a clean checkout at the pin and installed dependencies matching its lock:

```sh
LC_ALL=en_US.UTF-8 node scripts/capture-model-scope-reference.mjs "$PI_SOURCE" "$PI_INSTALLED" fixtures/native-model-scope/reference.json
LC_ALL=sv_SE.UTF-8 node scripts/capture-model-scope-reference.mjs "$PI_SOURCE" "$PI_INSTALLED" fixtures/native-model-scope/ordering-sv-SE.json --ordering-only
# Repeat the ordering command for tr_TR/tr-TR, de_DE/de-DE and th_TH/th-TH.
cargo test -p pipkin-engine runtime::models::tests
```

The 416 cases establish their recorded outcomes, not universal minimatch or ICU equivalence.
See `llm-docs/pipkin-rust-engine-evidence/model-scope.md` for runtime checks and limits.
