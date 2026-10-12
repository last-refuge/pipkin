# Native queue reference fixtures

Both corpora execute pinned Pi source at
`d2a311097cbcf669e699479587332ae3988a49d0` without user profiles or cloud services.

- `inbox-reference.json`: inbox selection/placement primitives, captured by
  `scripts/capture-inbox-reference.mjs` (see queued-summary-writes evidence).
- `runtime-reference.json`: eleven complete durable Harness runs captured by
  `scripts/capture-queue-runtime-reference.mjs`. A gated first model response permits
  deterministic busy rejection, queued steers/follow-ups and withdrawal before the real
  scheduler reaches a tool or final boundary. Four cases fail the first response, inspect
  the retained queue, then submit explicit input. Default and `all` delivery modes are
  included. Only model responses are supplied by the capture script.

The runtime corpus records source hashes, Node version, queue state, every projected model
request, visible message history and per-submission outcome/reason/answer text. Projections
retain role, text and tool names; IDs, timing, usage and system prompts are outside its scope.
The unknown-tool case exercises the real scheduler's unavailable-tool result and post-tools
boundary. It does not test coding-tool effects or extension loading.

Regenerate the runtime corpus with a clean local checkout and its installed dependencies:

```sh
node scripts/capture-queue-runtime-reference.mjs PI_CHECKOUT fixtures/native-queue/runtime-reference.json
cargo test -p pipkin-engine queue_reference_tests
```

The native runtime additionally reopens its SQLite engine while failed-run queues are held;
the reference capture itself uses MemoryStorage and makes no reference-restart claim.
