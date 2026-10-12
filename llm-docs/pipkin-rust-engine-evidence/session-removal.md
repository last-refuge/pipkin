# Durable native session removal

2026-10-11, following `dfbca07`. Protocol **11**, schema **9**.
Native remains opt-in. This closes the missing built-in removal implementation;
installed workflows and broader release qualification remain separate gates.

## Contract and reference

The source audit in [model-cycle evidence](model-cycle.md#confirmed-remaining-work-service-session-removal)
traces the pinned built-in service through attachment release, worker shutdown,
storage/profile removal and directory refresh. Native `RemoveSession` removes a root
and its children; removing a child individually returns Invalid. No desktop delete
button or optional extension compatibility is added. This batch uses the pinned source
contract and native execution tests, not an executed full-reference lifecycle differential.

Removal first commits a durable receipt with the root's creation key. That transaction
also reads the complete child membership. A registry/publication barrier denies new
session demand and writes, cancels runs and compactions, and wakes prompt waiters.
The cleanup task outlives its caller. It waits for demand and retiring workers to drain,
then atomically deletes session-owned rows, import mappings and profile state, emits
durable deletion invalidations, and marks the receipt complete. Provider resources are
retired using their affinity identifiers. Project files, original Pi data and backups
are preserved. Repeated calls for a removed root succeed; old creation/import requests
cannot resurrect it. Import discovery explains this refusal before copying any files.

Eight cleanup jobs may be active. A ninth request returns Busy before committing an
intent. A committed cleanup that takes over ten seconds returns a pending Busy response;
it continues independently and can be checked by repeating the same root id. Store
failures roll back deletion and leave the durable intent available for recovery. Startup
finishes pending removals before recovering runs or compactions. On Linux, it reaps
orphaned tool processes before deleting their identifying records.

The desktop resynchronizes its catalog, attachment and tracked operations after either
live notification or durable catch-up. The core now drops absent live conversations as
well as absent cached conversations. A deleted selected conversation no longer remains
visible or blocks catch-up; subsequent coding work and restart remain usable.

## Measured native evidence

- `runtime::removal::tests` verifies root/child deletion, preservation of another usable
  root/project files/backups, idempotence, rejected child-only removal, old creation-key
  refusal, live child/queue/waiter cancellation, six concurrent removal clients, caller
  disappearance/restart, failed-intent preservation, deletion rollback, eight-job
  capacity recovery, and cancellation of concurrent manual and background compactions.
- `crash_points::removal_reaps_live_tools_and_recovers_both_durable_crash_boundaries`
  launches the real engine process and local HTTP provider. It removes an active shell
  tool normally, kills the engine after removal intent, and kills it after database
  deletion but before live notification. Restart has no session, no recovery provider
  call, no surviving marked shell processes and a durable invalidation. Project effects
  already written remain; the delayed effect does not run. Linux only.
- `adapters::native::e2e::removal_of_the_open_running_conversation_updates_desktop_even_without_live_events`
  drives the real native backend and pure desktop state, with live events delivered and
  with every live event dropped. Both remove the open running conversation, stay connected,
  recover through restart and complete a new coding turn. This is headless integration,
  not rendered-window acceptance.
- `import::tests::removal_of_an_import_preserves_original_history_profiles_and_backups`
  imports a rich reference-schema root/child fixture with a historical profile selection,
  removes the native copy, and compares source/profile/backup bytes before and after.
  Replayed import is refused without rewriting any preserved backup.
- `store::tests::a_v8_database_is_backed_up_before_removal_receipts_and_delete_events`
  verifies migration, retained data, a readable schema-8 backup and deletion invalidation.
  Older migration fixtures explicitly remove schema-9 objects when constructing their
  historical database shape. Existing newer-schema refusal checks still apply.

## Validation record and limits

The initial child cancellation test exposed a ten-second wait for an intentionally
suppressed child settlement. Removal now cancels that wait directly. Initial crash-test
execution could not bind localhost inside the sandbox; it was rerun with subprocess/socket
permissions. The first permitted test incorrectly attempted to acquire a second Store
writer lock; it now reads the live cursor through the protocol and inspects SQLite after
worker shutdown. The desktop test initially used the nonexistent library target; its
correct target is the `pipkin` binary. These failures were corrected, not waived.

Targeted logs: `/tmp/pipkin-removal-tests2.log`, `/tmp/pipkin-removal-expanded.log`,
`/tmp/pipkin-removal-crash2.log`, `/tmp/pipkin-removal-desktop2.log`.
Required checks: **1502 passed, 0 failed, 53 ignored**; Clippy and formatting clean.
Logs: `/tmp/pipkin-removal-workspace2.log`, `/tmp/pipkin-removal-clippy2.log`,
`/tmp/pipkin-removal-fmt.log`. The first full run found five migration-fixture failures:
synthetic v1/v2 databases still had schema-9 objects, and v5/v6/v7 checks hardcoded schema 8.
Fixtures now reconstruct the old schema and assert the current version while retaining all
historical backup and data-preservation assertions. No runtime failure was waived.

Removal is logical deletion, not secure erasure or immediate database shrinkage. Minimal
receipts remain for replay protection; original imports/backups and desktop draft/cache
storage are not erased by the engine service. Orphan process discovery after engine death
is currently Linux-only. No macOS/Windows process-tree qualification, full reference
scheduler comparison, native window acceptance or Pi retirement is claimed here.
