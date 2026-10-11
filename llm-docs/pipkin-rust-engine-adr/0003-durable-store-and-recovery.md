# ADR 0003: Durable store and recovery

**Status:** Accepted (2026-10-11). Schema version 1, the OS-locked store, admission ledger,
runs, streamed-chunk commits, tool-call intent/result rows, retained tool output, usage,
subagent tasks, `kv` and `import_map` are implemented with unit tests
(`crates/pipkin-engine/src/store/`). Power-loss behavior, disk-full/read-only injection,
corruption recovery and migration from a non-empty older native schema are **not yet tested**.

## Context

Reference (audit `engine-tools-durable-usage.md` §3): one durable `Harness` per session with
one serialized line of atomic commits; `STRICT` tables `conversations, entries, tasks,
submissions, documents, document_revisions, record_ids` with one global id space
(`durable/src/storage/sqlite/migrations.ts:9-85`); documents stored as base + Chord deltas;
schema version 1 with newer-schema refusal (`migrations.ts:95-125`). Pragmas
`journal_mode=WAL`, **`synchronous=NORMAL`**, `wal_autocheckpoint=1000`, busy timeout 5000 ms,
`BEGIN IMMEDIATE` per commit, `wal_checkpoint(TRUNCATE)` on close
(`storage/sqlite/node.ts:16-17,140-198`); "newest commit may be lost on power failure"
(`durable/README.md:563`). One process owns a storage via `proper-lockfile` on the session
directory (`ce/session-worker.ts:518-523`). Tasks are phase state machines whose phases must
each commit durable progress (`scheduler.ts:893-896`); on open every `running` task becomes
`pending` and scheduling resumes with `harness.resume()` (`scheduler.ts:230-256`,
`ce/session-worker.ts:595`).

## Decision

1. **One database per engine data root** (`<data_dir>/engine.sqlite` family; `Store::open`),
   owned by exactly one process through an OS lock on a sibling file (`lock_dir`); a second
   opener gets `StoreError::Locked`. Sessions share the database (no per-session worker
   files).
2. **Pragmas:** `journal_mode=WAL` (verified, otherwise refuse), **`synchronous=FULL`**,
   `foreign_keys=ON`, `busy_timeout=5000` (`store/schema.rs::configure`). An acknowledged write
   has been fsynced. This is a deliberate strengthening over the reference `NORMAL`
   (intentional improvement I-02 in the parity ledger); process-kill tests do not prove power
   loss and none is claimed.
3. **Schema:** explicit `PRAGMA user_version`; `SCHEMA_VERSION = 1`; ordered `MIGRATIONS`.
   A newer database is refused **before** any pragma change (`schema::open`); an older one is
   backed up with the SQLite backup API next to itself, then migrated inside one transaction.
   Database file mode `0600` on Unix.
4. **Authoritative records** (all `STRICT`, JSON payloads `json_valid`-checked): `sessions`
   (create key unique, revision, `blocked`), `entries` (with `retired` and `partial` flags),
   `entry_chunks` (committed stream deltas of a partial message, dropped on finalize),
   `requests` (admission ledger and queue; primary key `(session, key)`, payload
   `fingerprint`), `runs` (`running|stopping|settled`, `stop_requested_at`), `tool_calls`
   (`intent|done|interrupted`, `replay_safe`), `tool_outputs` (retained bounded output with
   dropped-byte count), `usage`, `subagent_tasks` (unique `(parent_session, parent_call)` and
   unique `child_session`, `result_delivered`), `kv` (engine/extension state preserved
   verbatim), `import_map`. No rebuildable projections yet.
5. **Transactions:** every mutation is one short transaction that bumps the session revision;
   nothing awaits network, processes or people inside it (`Store::tx`). Events are published
   only after commit.
6. **Admission:** same key + same payload returns the original admission; same key with a
   different payload is `ErrorCode::Conflict` (intentional improvement I-01: the reference
   ignores content, `harness/submissions.ts:160-219`). Busy + prompt is refused without
   recording anything.
7. **State-driven run loop** (`runtime/run.rs`): the next step is derived from committed state
   — unresolved tool calls of the newest assistant message → execute (intent first); a final
   answer → settle `Completed` and start the next queued run; otherwise request the model.
   Streamed progress is committed in batches (time/byte thresholds) before publication; the
   final message replaces the partial one with its usage in one commit. A resumed run after a
   crash continues from the record; it never re-admits the prompt.
8. **Fail closed:** a storage error during a session mutation cancels the active run and marks
   the session failed; further mutations are refused (`ErrorCode::Storage`) until the session is
   reopened, which reconciles from committed state (`runtime/session.rs`; test
   `a_storage_failure_fails_the_session_closed_until_it_is_reopened`). Nothing manufactures
   success or discards the request ledger.
9. **Recovery on open:** unsettled runs are resumed; a durable Stop intent settles the run
   `Aborted` (`a_durable_stop_survives_a_restart_and_settles_aborted`); partial streaming
   entries are materialized from committed chunks. Tool recovery follows ADR 0005.

## Consequences

- One writer for all sessions simplifies ownership and backup but serializes commits across
  sessions; commit cadence must be measured against the 50 ms streaming budget (plan §7).
  Group commit may be benchmarked; relaxing `synchronous` to win a chart is excluded.
- The reference document/delta model (`pi.live`, `pi.inbox`, `pi.agent`, `pi.usage`,
  `pi.provider`) is replaced by typed columns/tables. The importer (RE-19, `E/import`) must map
  every document, including `pi.provider.sessionId` (provider cache affinity) and
  `pi.agent.extensions` (subagent enable state), into these rows or `kv`; unknown payloads are
  preserved verbatim and block conversion of that conversation.
- Not yet covered: disk-full/read-only injection, corruption set-aside, migration of a
  non-empty older native schema, power-loss, competing-writer across processes beyond the lock
  test. They are open rows in the parity ledger (DUR-*).

## References

- Plan §4 "Durable execution model", §11 "Schema and protocol review checklist".
- Audit `engine-tools-durable-usage.md` §3.1–3.7.
- Native: `crates/pipkin-engine/src/store/{mod,schema}.rs`, `runtime/{run,session}.rs`.
- Tests (pass, 2026-10-11, `e7cfc42`): `store/tests.rs` —
  `create_is_idempotent_under_its_key`,
  `same_key_same_payload_is_the_original_and_changed_payload_conflicts`,
  `an_injected_failure_rolls_back_and_admits_nothing`,
  `streaming_chunks_are_appended_and_materialized_then_dropped_on_finalize`,
  `a_newer_schema_is_refused_untouched_and_the_lock_is_exclusive`,
  `acknowledged_writes_survive_reopen`; `runtime/tests.rs` —
  `an_engine_restart_mid_run_resumes_the_run_without_duplicating_the_prompt`,
  `a_storage_failure_fails_the_session_closed_until_it_is_reopened`.
