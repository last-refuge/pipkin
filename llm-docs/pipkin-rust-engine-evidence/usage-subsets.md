# Usage subset preservation — 2026-10-11

Implemented on `native-engine` on top of `e8993de`. Protocol 4 / schema 3 remain compatible:
the new JSON fields are optional and older records deserialize without them. The user's
pre-existing plan edit and `.claude/worktrees/` are untouched. Native remains opt-in.

## Contract and implementation

The pinned reference checkout was verified at `d2a311097cbcf669e699479587332ae3988a49d0`.
`ai/src/types.ts:429-450` defines `reasoning` as a subset of output and `cacheWrite1h` as a
subset of cache writes. `durable/src/harness/usage.ts:42-55` adds optional counters only once
reported. Native protocol `Usage` and core `UsageAmount` now preserve both as optional u64s:
absence remains distinct from reported zero, additions saturate, and neither subset is added
again to total tokens. Existing cost calculations use the preserved one-hour counter instead
of an emitter-only side channel. JSON storage/import and existing aggregate views carry them.

Anthropic records thinking-token deltas and one-hour cache writes. Bedrock records the TTL
breakdown. OpenAI Completions, Responses (including Codex/Azure), and Google's shared
Gemini/Vertex response handling record reasoning counts with the reference's zero defaults.
Other providers keep unreported values absent; Pi-message deserialization preserves supplied
fields. Both desktop adapters and the core totals retain the subsets. The usage panel labels
them as portions of cache writes/output and avoids claiming the engine is always Pi.

No database rewrite is required. Counts discarded by older native versions cannot be
reconstructed from aggregate totals; this change does not fabricate historical breakdowns.

## Evidence

Required checks on the final source:

- `cargo test --workspace`: **1334 passed, 0 failed, 53 ignored**.
- `cargo clippy --workspace --all-targets`: passed, no warnings.
- `cargo fmt --all --check` and `git diff --check`: passed.

The initial sandboxed workspace test attempt stopped at pi-client's 19 local-socket failures.
The complete suite was then run with local socket access and passed. Logs are
`/tmp/pipkin-usage-tests-host.log`, `/tmp/pipkin-usage-clippy.log`, and
`/tmp/pipkin-usage-fmt.log`.

New or strengthened assertions:

- Protocol `usage_optional_subsets_round_trip_and_accumulate_without_double_counting`:
  old JSON, absence versus zero, exact names, accumulation, overflow saturation, and totals.
- Store `usage_optional_subsets_survive_reopen_and_later_legacy_records`: disk-backed reopen
  and a later record without optional counters do not erase previously reported values.
- Import `imports_a_rich_session_losslessly_with_the_same_effective_context`: the Pi usage
  document's delta chain imports nonzero reasoning and one-hour counts and its total exactly.
- Existing Anthropic fragmented SSE, Bedrock signed EventStream, Gemini fragmented stream,
  OpenAI Completions and Responses tests now assert the subset counts alongside costs/totals.
- Every Codex reference-golden comparison now includes both optional fields. Existing captured
  fixtures already include reasoning counts; the prior comparison skipped them. No reference
  fixture was changed to make the test pass.
- Native and Pi desktop mapping tests retain both counts; invalid Pi optional counters make
  usage unavailable rather than silently losing data. Core totals include model/tool buckets
  while preserving unknown breakdowns and avoiding double counting.

This closes the identified missing-counter implementation gap in GEN-17 / USE-LEDGER.
It does not qualify either entire row: the complete native/Pi session differential and broader
provider/compaction/parent-child usage qualification remain open. The panel compiles and the
workspace GPUI tests pass; no new interactive desktop or live-provider check was performed.
