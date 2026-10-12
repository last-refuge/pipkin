# Native thinking cycle and atomic configuration

2026-10-11, following `5917287`. Protocol **10** / schema **8**.
Native remains opt-in; no release qualification or Pi retirement is claimed.

## Implemented service contract

Pinned `experimental/services/models-provider.ts:69-74` exposes `cycleThinking` through
the built-in Models service. The desktop currently selects levels explicitly, but the
service is available to connected clients. Native now implements `CycleThinking` and
advertises the `cycleThinking` capability. Older protocol versions refuse the handshake.

Cycling advances through the selected model's supported levels and wraps to the first.
A missing stored level defaults to `off` before advancing; an unknown/disabled level
starts at the first supported level. Missing and non-reasoning models use `off`. A model
that disables every level also falls back to `off`, matching the service's nullish
fallback without dividing/indexing into an empty list. Explicit `SetThinking` retains
its existing validation; it does not gain the cycle operation's fallback behavior.

Model selection, explicit level changes and cycles share one configuration/publication
order. Model selection previously committed model and thinking in separate transactions;
they now commit together with one revision and one durable invalidation. Events publish
only after the commit. A rejected storage write rolls back the fields and outbox entry.
Existing default-model and enabled-model persistence remains, and no provider/auth/billing
path changes. Cycle requests are ordinary mutations, not idempotent prompt admissions:
clients must resnapshot an uncertain result rather than blindly replay a cycle.

## Measured evidence

- `runtime::settings_tests::thinking_cycles_match_the_executed_reference_service` runs
  nine cases through the native serialized Call/Reply shape and durable store. Results
  match original `createModelsService/cycleThinking` and `getSupportedThinkingLevels`
  from pin `d2a311097cbcf669e699479587332ae3988a49d0`.
- `scripts/capture-thinking-reference.mjs` verifies the source pin and clean files,
  rewrites service import paths, and extracts the complete original level-selector
  function and constant. Document/configuration/facet dependencies are stubs; method
  bodies execute unchanged. Source hashes, inputs and outputs are retained in
  `fixtures/native-thinking/reference.json`. This is a function/service comparison,
  not a live-provider or legacy transport qualification.
- `thinking_cycles_are_serialized_notify_clients_and_survive_restart` races 23 independent
  clients. Every accepted cycle advances the durable revision and the observing client's
  ordered configuration events. A subsequent generation receives the resulting level,
  and engine restart retains it.
- `model_and_thinking_changes_commit_together_and_fail_without_partial_state` checks one
  revision/invalidation for a switch to a non-reasoning model, a write fault after SQL
  changes but before commit, failed cycle rollback, and missing-session refusal.

Targeted checks: `/tmp/pipkin-model-cycle-reference-tests.log` (19 tests passed).
An initial test compile used `SessionSummary.revision` instead of the snapshot revision;
the fixture field was corrected, with logs retained in `/tmp/pipkin-model-cycle-tests.log`.
Required checks: `/tmp/pipkin-model-cycle-{workspace,clippy,fmt}.log`; results in progress record.

SVC-MODELS-CYCLE moves PA → NC. Native UI menu/shortcut changes were not needed: the
existing explicit selection UI continues to work. All installed/platform/performance
and broader provider qualification gates remain separate.

## Confirmed remaining work: service session removal

SVC-MGMT-REMOVE remains PA for implementation, not excluded because the desktop lacks
a delete button. The frozen server exports it through `SessionManagement`, and
`experimental/client-runtime.ts:203-206` wraps it for non-interactive clients.
`experimental/services/server.ts:97-102` first releases attachments, then removes the
session and refreshes the directory. `experimental/server.ts:431-435` stops the worker,
deletes session storage, and removes its profile. Worker shutdown is implemented by
`session-worker-manager.ts:189-192`, attachment teardown by
`server/src/session-router.ts:72-88`, and storage deletion by `session-catalog.ts:81-82`.

The native equivalent must cover a root and its children: prevent new admissions while
removing, stop providers/tools/retries/compactions, release attachments and cached resources,
and remove session-owned records atomically with durable directory invalidation. It must
handle concurrent callers, teardown failures and restart without resurrecting deleted work.
Deleting imported native state must preserve original Pi data and backups. Tests must use
disposable sessions and verify unrelated roots/project files remain untouched. Optional
extension compatibility remains out of scope; it does not justify bypassing core lifecycle
or persistent-state cleanup.
