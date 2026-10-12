# Context estimation reference

`reference.json` executes the pinned Pi durable `estimateContext` and `orderToolResults`
functions. Twelve normalized context views cover empty input, UTF-16 rounding, latest
usage and component fallback, repeated/missing/stray/late tool results, reused call IDs,
images/thinking, usage before/after a compaction head, and reset handoff.

The native test deserializes the same input entries, derives model context, and compares
both its complete ordered messages and token estimate. The reference fixture explicitly
constructs normalized per-entry contributions: it does not claim execution of reference
storage scanning or context edits. Original source revision and hashes are in the JSON.

```sh
node scripts/capture-context-estimate-reference.mjs /path/to/pinned/pi fixtures/native-context-estimate/reference.json
cargo test -p pipkin-engine --lib context_estimates_and_ordering_match_executed_pi_reference
```

The capture refuses another revision or modified reference source. The pinned checkout's
installed dependencies and source resolver are required only for offline regeneration;
native tests consume the frozen JSON without Node or Pi. Two actual captures on 2026-10-12
matched byte for byte. No live provider, account, user profile or extension execution.
