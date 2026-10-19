# Opus thinking metadata / retained diagnostics (2026-10-17)

Historical narrow repair evidence. The later authorized provider repairs supersede its strict
metadata policy: unused telemetry is now discarded without imposing a value schema, and display
compatibility is separate from execution outcome. See `docs/claude-cli-provider-review.md` and
`docs/scorecard.md` for the broader defects, repaired boundaries and current verification.

## Observed, without executing the installed CLI

Two recent failed prompt receipts were inspected through a read-only SQLite connection to
Pipkin's engine store (`mode=ro`, `query_only=ON`, bounded queries). No prompt text, raw stream,
account credentials or CLI-private history was read. Their immutable bindings record CLI 2.1.301
and the restricted no-tools policy. Both receipts have:

- `state=unknown`, no terminal observation; input delivered, root reaped and pipes drained.
- `supervisor_stop=observer_failure`, `projection_error=MalformedRecord`.
- `rejected_record=content_block_delta`, `parse_failure.kind=UnknownField`.
- Rejected field-label SHA-256:
  `b397529d5667ea5d41c7f9aca23c9dfa3691ae5d67421fe11eff7f3683406a4a`.

That digest equals SHA-256 of **`estimated_tokens`**. This is a schema-label hash, not a
credential or content hash. The original field value and full record were not retained;
its exact value is therefore unverified. No successful completion or safe replay follows
from the cleanup facts. These old receipts and invalidated bindings were not changed.

## Public schema and narrow fix

Official SDK definition fetched 2026-10-17:
<https://raw.githubusercontent.com/anthropics/anthropic-sdk-python/main/src/anthropic/types/beta/beta_thinking_delta.py>

`BetaThinkingDelta.estimated_tokens` is optional nullable integer metadata, documented as a
nonnegative, coarse per-frame thinking progress increment. It is not billable usage; the SDK
says `usage.output_tokens` remains authoritative. The decoder now accepts omitted/null/u64
only on `thinking_delta`, and discards the estimate. Text projection, block ordering, model
identity, permissions, terminal requirements and supervisor completion checks are unchanged.
Unknown fields and malformed types still fail closed.

Synthetic regression fragment, **not a captured real CLI record**:

```json
{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Synthetic thought","estimated_tokens":128}}
```

New unknown settlements also commit one inert error notice in the same transaction as their
receipt, retaining closed decoder/record/parse/supervisor categories across snapshots and
restart. Raw records, field labels/digests, stderr and private values are never displayed.
Settlement does not transiently duplicate an identical final error already in the snapshot.
No existing receipt is upgraded or backfilled; no replay, native fallback or new authority.

## Offline verification

- Job249: the new accepted-metadata regression **failed before the parser fix**, with
  `MalformedRecord` (owned TMPDIR `/home/jason/.pg.hbWERo` retained).
- Job250: CLI decoder/process tests, external runtime/store tests and the core snapshot/failure
  regression passed. Coverage includes nullable/integer metadata, byte-at-a-time UTF-8/JSON,
  unknown fields and malformed types, missing terminal, wrong block kind, unchanged usage,
  fake Opus fresh/resume with zero native requests, retained failure with no second launch,
  durable/idempotent notices after store reopen, and unchanged context fencing.
- Job251: full workspace gate failed before Clippy. Engine: 1331 passed, one pre-existing
  pipeline fixture failed during metadata preflight with `selected executable is busy; no
  automatic retry`, before stream decoding. Retained `/home/jason/.pg.QQ4aG0`. The fixture uses
  its own executable and already closes its writer before probing. The transient OS failure's
  cause is not established; no production/test retry, sleep or weakened busy-executable gate
  was added.
- Job252: all five isolated repeats passed, followed by a successful normal-parallel workspace
  run (engine1332, UI217), strict workspace/all-target Clippy, fmt and diff checks. This does not
  establish or fix the earlier transient busy-executable cause. Nonempty owned test directory
  `/home/jason/.pg.iZadjw` retained; the job exited successfully.

No real CLI prompts/probes, release build, packaging, installation, native input or profile
mutation. Live qualification remains limited to previously recorded 2.1.299 runs, not this
synthetic Opus regression or every newer supported CLI version. The ten-run ledger is unchanged:
`bf794518e35d7f1ce3a50b3058c4191bb9401e568fc645d77e10b0f404cf1f22`.
