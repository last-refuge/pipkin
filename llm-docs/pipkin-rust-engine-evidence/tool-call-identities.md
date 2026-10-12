# Reused provider tool IDs and schema 10

2026-10-12, following `42b196d`; native protocol 12, engine schema **10**.
Native remains opt-in. Pi extensions remain outside the required scope.

## Executed failure and correction

The paired package benchmark exposed a correctness defect before its performance results
could be accepted. The existing local provider returns `call_1` for each tool invocation.
The actual pinned Pi package executed two requested bash effects; native returned two final
replies but executed only the first effect. The pinned durable harness identifies a tool task
using the assistant entry as well as its call ID (`packages/durable/src/harness/generation.ts`,
`startToolRound`). Native had treated `(session, provider call ID)` as a unique invocation.
Its answered-result lookup also conflated IDs reused across rounds of a single run.

New finalized responses now allocate a private durable ID when a tool ID collides with an
earlier call or another call in that response. An indexed per-session registry reserves IDs
from imported, retired and unexecuted finalized calls as well as executed intents/outputs.
The original provider ID is retained in private block metadata. Context assembly first pairs
results by durable ID, then restores original provider IDs and removes the private metadata
before adapter encoding. Provider-supplied values cannot inject that persistence metadata.
Recovery reads the already committed identity and never allocates a fresh invocation for it.

Schema 10 adds and backfills `tool_call_ids`. The version change also prevents older workers
from misinterpreting alias-bearing histories. Existing migration machinery creates the private
`engine.sqlite.v9.bak`; existing entries and saved outputs are not rewritten. This fix does
not replay commands that an earlier defective build skipped.

## Verification

`runtime::tests::repeated_provider_tool_ids_execute_distinct_calls_and_keep_recovery_idempotent`
failed before the correction and passes afterward. It covers repeated IDs within/across tool
rounds, another run after worker restart, distinct exact retained outputs, original IDs in
provider context, and settled-request replay without new effects. An injected tool-result
commit failure followed by restart marks the affected invocation interrupted without repeating
its effect, executes the next distinct call, and preserves prior results.

`store::tests::v9_tool_history_is_backed_up_and_preserved_before_invocation_aliases` verifies
schema-9 backup contents, version-10 migration, retained outputs and collision avoidance for
an imported/unexecuted call with no intent. Older-schema fixture tests remove the new table
when reconstructing their historical database shapes.

The actual package driver now handles equal-schema rollback or a real increasing-schema
upgrade. `fixtures/native-installed/2026-10-12-schema10-upgrade.json` records actual schema-9
and schema-10 artifacts: imported coding, migration, continuation, checksum-protected rollback,
older-worker refusal without modifying the migrated database, and opening a copy of the exact
pre-upgrade backup with the older worker. Every restored record stays equal except the expected
one-step startup event-epoch increment; the untouched backup stays logically identical.
Current-schema coding and closed-profile archive restoration also pass. No automatic downgrade
or conversion of post-upgrade history is claimed. The current native package passes the
installed coding/restart/retained-output/archive/uninstall driver with Node/Pi traps unused.

Both actual rebuilt packages share application SHA-256
`4f5453d4b7d07436bfb01209de07d7a4b8284a19fda4905d06e6bde2efe659b6`, built from `42b196d`
plus the working changes. Full artifact identities are retained in the paired benchmark and
schema-upgrade reports. The paired workload now passes 240 real effects; see
[paired performance](paired-performance.md).

The actual installed Pi/native rollback test also passes on these new artifacts (7.14 seconds),
including complete desktop/native archive restoration, ownership refusal, explicit release,
Pi continuation and retained native-only history. Exact artifact and test-input identities are
in `fixtures/native-installed/2026-10-12-schema10-cross-engine.json`. This is headless evidence;
the earlier schema-9 report remains preserved separately.

Final required workspace checks: **1,640 passed, 0 failed, 56 ignored**, clean Clippy/all-targets
and formatting. The ignored paired benchmark is executed separately. Initial failures are
retained as part of this record: the native repeated-ID regression; historical migration test
fixtures that initially omitted dropping the new table; and an endpoint fixture that observed
a listener briefly inherited by a concurrent fork. The endpoint test now waits, with a deadline,
for `ConnectionRefused` after dropping its stale listener, retaining the production ownership
and replacement-path assertions. The fork timing explanation is an inference from test setup
and the isolated passing rerun. The initial upgrade driver incorrectly required a byte-for-byte
logical dump after startup; it now explicitly verifies the epoch increment and every other row.
An initial Clippy constant-assertion warning in the new benchmark was corrected before the
clean rerun. These initial failures are not reported as passing runs.

The tests do not establish all provider wire formats, visual desktop acceptance, live accounts,
hosted CI or full migration qualification. Older history containing duplicate provider IDs is
preserved; this change protects new response commits and does not silently rewrite that history.
