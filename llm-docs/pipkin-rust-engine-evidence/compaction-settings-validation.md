# Compaction settings validation on use

2026-10-12, following `cf5043a`. Protocol 11 / schema 9 unchanged.

Previously, a malformed compaction model override failed typed deserialization of the whole
settings file. Native then warned and used defaults, losing unrelated settings and applying a
different compaction policy. Pi keeps those settings and validates the ordinary token setting
and selected model override when the policy is resolved.

Native now retains model overrides as JSON until that exact model is selected. Ordinary token
errors are retained separately from their typed values; policy resolution rejects them rather
than using placeholder defaults. Per-field resolution checks the ordinary setting before the
selected override, matching reference error order. Unselected invalid overrides do not affect
other models. Numeric tokens must be non-negative JavaScript safe integers, including integral
float spellings. Null/scalar policy shapes and enabled consumer truthiness follow the executed
reference. The native backgroundTokens option also reports invalid tokens on use.

Run preparation resolves this policy before credential or provider requests. Failed generation
and manual compaction report the setting path/value error; a live global/project correction
can restore generation and compaction without restart. Existing admitted compaction jobs
retain their pinned request/summary budget under the durable recovery contract.

`scripts/capture-compaction-settings-reference.mjs` runs the actual pinned SettingsManager
with its in-memory storage, migration, merge and getters. Temporary copies only rewrite import
paths; unused I/O/network dependencies throw if invoked. The actual text/BOM and retry modules are copied
unchanged, so retry getter defaults use the pinned exported constant. This is not a complete Pi application or filesystem locking run. See the fixture
README for provenance and capture constraints.

The final capture contains **46 cases**, including successful policies and **29 errors**.
Native compares exact errors, effective policies and preserved default provider/model values.
A runtime test adds invalid settings after successful coding turns, confirms both generation
and manual compaction fail without provider calls, repairs the selected override through
project settings, then verifies a successful coding turn and the original summary budget.
An unrelated invalid model override, disabled automatic compaction and queue mode survive.
The source global settings file is unchanged by loading and recovery.

The focused compaction suite passed **49 tests** (47 unit/runtime plus 2 crash/recovery tests)
against the initial 37-case capture; the final workspace run includes all 46 cases. Logs:
`/tmp/pipkin-compaction-settings-check.log`, `/tmp/pipkin-compaction-settings-tests.log`.
Clippy identified a manual numeric range check; it was changed to RangeInclusive::contains.
The first workspace run with expanded project-null cases found a real merge defect: native
ignored null overrides, keeping the global compaction entry instead of reporting its selected
null replacement as invalid. The merge now preserves explicit null replacements. Existing
nullish-default getters (retry settings, queue modes and skills) normalize null to their own
defaults without dropping unrelated settings; compaction token nulls remain errors. Three
additional reference cases compare retry getter defaults and project clearing. The corrected
focused suite again passes all 49 tests, now with all 46 settings cases. The failed workspace
log is retained at `/tmp/pipkin-compaction-settings-workspace.log`; corrected focused log:
`/tmp/pipkin-compaction-settings-tests2.log`. The initial retry-default fixture used the verified numeric value as a stub. The final
capture now imports the actual pinned retry module and records its hash; all 46 case inputs
and results were compared and remained identical. Final required checks passed **1626 tests, 0 failed, 53 ignored**, with clean Clippy and
formatting. Logs: `/tmp/pipkin-compaction-settings-workspace2.log`,
`/tmp/pipkin-compaction-settings-clippy2.log`, `/tmp/pipkin-compaction-settings-fmt2.log`.

This closes the invalid-compaction-override fallback gap. The existing choice to apply classic
per-model overrides remains deliberate: Pi's durable harness calls the getter without a model
and ignores them. HTTP timeout invalid-value behavior, other settings/model-scope gaps,
credential retries, trust scope, rollback labels, sustained workloads and release/platform
qualification remain open. No row is fully qualified by this batch.
