# Compaction journal and durable Stop intent

Historical storage-stage record. Runtime activation and subsequent evidence are in
[compaction-runtime.md](compaction-runtime.md); its results supersede the remaining-work
section below.

Storage stage after `0721977`, 2026-10-11. Native schema **6**, protocol **9**.
This is a prerequisite for runtime recovery, not a completed compaction recovery path.

## Persisted contract

`store/compactions.rs` adds stable per-session operation keys, immutable scope/run/input
identity, revisioned progress, and Select / Summarize / Retry / Terminal checkpoints.
A selected request preserves its model key, thinking level, provider affinity id,
summary messages, output limit, timeout, context tail/cut and prior token count. Stream
options are captured through an explicit non-secret allowlist. Credentials, headers,
resolved endpoints, environment, and arbitrary future provider options are not copied.

A saved provider response and reception time can survive before classification. The
runtime consumer must classify it with usage accounting, retry/terminal transition,
and any summary entry in one transaction. Compare-and-swap on the operation revision
prevents a stale response from replacing durable Stop or terminal state. A stale save
returns false; the caller must then perform no accounting or placement. Terminal jobs
cannot transition again. Pending enumeration loads ids rather than all saved prompts.

Conversation Stop marks foreground compaction rows in the same store transaction as
coding-run Stop intent. The mark is saved even when no coding run is open; background
rows remain running. On recovery a stopping row may only settle Aborted.

Schema migration creates the journal after the existing private backup step. Regression
fixtures for v1–v4 now remove this newer table when reconstructing their historical schema;
their original state-preservation assertions remain. A new v5 upgrade case proves the
backup retains history and has no compaction journal, while the upgraded store preserves
that history. Older runtimes refuse schema 6 rather than mutating it.

## Evidence

- Selected request/options and absolute retry deadline survive store close/reopen.
- Serialized selection excludes credential/header/environment/endpoint secrets and an
  unrecognized future auth option.
- Saved provider response survives reopen; injected transaction failure rolls back usage,
  summary entry and completion together. Retrying commits once; replay cannot count twice.
- Foreground Stop survives reopen, excludes background, and defeats a stale saved revision.
- Operation keys are idempotent; mismatched input/scope/run ownership is refused.
- v5 migration preserves history and creates a read-only-inspected backup (0600 on Unix).
- Runtime `Call::Stop` with no open coding run persists manually seeded foreground journal
  intent across engine shutdown; background remains pending. This proves the Stop hook,
  not execution or recovery of those seeded jobs.

Final workspace checks: **1445 passed, 0 failed, 53 ignored**, Clippy and formatting
clean. Logs: `/tmp/pipkin-compaction-journal-{workspace,clippy,fmt}.log`.
The first targeted compile found a test fixture using `.into()` for a serde JSON map;
fixed to collect its entries. No behavior was changed to accommodate that compile error.

## Remaining runtime work

The current compaction driver still runs from its future. It does not yet admit, execute,
classify or recover these journal records. CTX-CHECKPOINT remains PA with partial storage
evidence. Next integrate all manual/background/blocking/overflow paths, pin request state,
resolve credentials afresh, resume saved retries/responses on startup/reopen, and exercise
actual worker crashes at phase boundaries. Fix the generation/overflow handoff so a crash
between compaction and generation classification cannot compact twice or lose the retry.
Keep scheduling and checkpoint materialization bounded.

Activation must advance the schema again so an intermediate schema-6 runtime that has the
journal but no recovery consumer cannot silently reopen data with active journaled jobs.
No release/default/qualification claim is made by this storage stage.
