# minimatch component semantics

The native model-pattern parser in `crates/pipkin-engine/src/runtime/models/pattern/`
is adapted from minimatch 10.2.6's component AST, character-class translation and path
matching semantics. Source: https://github.com/isaacs/minimatch, package version 10.2.6,
as locked by the Pi reference at `d2a311097cbcf669e699479587332ae3988a49d0`.

The Blue Oak Model License 1.0.0 notice is retained alongside this file and distributed
with native packages. No JavaScript package or runtime is shipped for this implementation.
The native implementation uses only the options required by model scoping, not the whole
minimatch public API. Executed-reference fixtures record the comparison scope.
