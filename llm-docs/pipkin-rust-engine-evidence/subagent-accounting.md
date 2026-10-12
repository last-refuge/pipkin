# Subagent capacity and aggregate usage

Follow-up to `4a0601f`, 2026-10-11; protocol 11 / schema 9 unchanged. Pi extensions remain
out of scope. Tests exercise the built-in subagent tool, scripted generation and real local
shell tools; no paid provider or desktop acceptance is claimed.

## Confirmed accounting defect and fix

The new runtime regression failed on the prior implementation: a waiting parent displayed
15 tokens although its own request and two child requests had committed 45 tokens. Child
usage was present in separate ledgers but never included in the parent's view. The previous
USE-TOTAL claim that child usage was attributed to the parent's subagent tool bucket was false.

Pinned `durable/src/harness/harness.ts::usage()` sums all conversation ledgers through
`harness/usage.ts::addUsageState`, preserving model and tool identities. Native views now sum
the requested conversation tree from authoritative ledger rows using the indexed parent
relationship. Spend is not copied into ancestors or reattributed to a tool. Snapshots and
streamed usage share this query; independent roots remain isolated. Existing optional counters
and costs use the same `Usage::add` implementation as ordinary ledger accumulation.

Whenever a child commits usage, its ancestor revisions and durable invalidations advance in
the same transaction. Live ancestor usage updates follow that commit under the publication
lock. Compaction accounting now emits usage changes as well, through the same path. This
keeps displayed parent totals current while children run and makes missed notifications
recoverable through the existing changefeed/snapshot mechanism.

## Runtime regressions

Three new tests in `runtime/subagent_accounting_tests.rs`:

1. Saturate both generation permits with parent requests while a third parent queues. After
   delegation, two children acquire the same capacity while their parents wait. All three
   groups settle, every permit returns, and each parent/child generates exactly as expected.
   Children still cannot delegate; this proves the supported parent-child path, not arbitrary
   recursive delegation.
2. Two children each execute a shell-tool round. The parent sees 45 tokens while their final
   requests are gated, then 90 when the group completes (60 input / 30 output). A live parent
   usage event includes the intermediate total. Replay before and after restart leaves totals
   unchanged, and a separate root's work does not contaminate them.
3. Inject a SQLite failure specifically when the ancestor revision is updated. The child's
   final usage transaction rolls back, and the parent retains its original 15 tokens. Restart
   recovers the child and parent, producing exactly 45 committed tokens without double counting.

The rich import regression now separately verifies that root ledger bytes/values preserve
their original counters while the public view includes child usage. Optional counters and
the independently reported subagent tool bucket remain intact; the child-only view is unchanged.

## Validation and boundaries

The capacity test passed before the accounting fix; the aggregate-usage regression failed
15 versus 45, then passed. The fault test initially used a provider cache identity instead of
the durable child session ID; corrected to read the task mapping. The first full workspace
run failed the old import-only-root assertion, which was strengthened as described above.
No failing requirement was waived.

Final checks: **1526 passed, 0 failed, 53 ignored**; Clippy and formatting clean.
Logs: `/tmp/pipkin-retry-usage-workspace2.log`, `/tmp/pipkin-retry-usage-clippy2.log`,
`/tmp/pipkin-retry-usage-fmt2.log`.
Copilot login retry changes were validated in the same working tree; see
[Copilot evidence](copilot-login-retries.md). Runtime capacity/accounting evidence closes the
two formerly unverified row claims. The usage contract comparison is a source audit, not an
executed full-harness differential. Larger-scale aggregation performance, installed workflows,
platform qualification and the native-default/Pi retirement gates remain open.
