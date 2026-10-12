# Frozen executed tool reference corpora

Generated from Pi `d2a311097cbcf669e699479587332ae3988a49d0` using the existing
`crates/pipkin-engine/src/tools/tests/reference/corpus-{edit,read}.mts` generators.
There are 1,500 edit cases and 1,000 read cases. Both native comparisons passed against
the freshly generated JSON, including retained file contents, text, errors and details.
The compressed inputs now run in ordinary Rust tests without Node or a Pi checkout.
The existing PIPKIN_EDIT_CORPUS/PIPKIN_READ_CORPUS JSON overrides remain for new captures.
Provenance records the pinned revision, generator and exact decoded/compressed hashes.
These are finite deterministic reference cases, not exhaustive correctness claims.
