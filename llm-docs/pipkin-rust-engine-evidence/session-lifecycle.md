# Native session attachment and retirement evidence

Implementation on top of `c3134df`, protocol **6**, store schema **4**. Pi remains the
default. This evidence closes particular native lifecycle gaps, not external transport or
extension-package compatibility qualification.

## Contract and ownership

`OpenSession` holds one idempotent attachment per client/session. `ReleaseSession` releases
only that client's attachment; it neither deletes history nor cancels work. The desktop
releases its previous conversation when switching to another. Multiple connections can keep
the same session warm independently.

The runtime registry serializes acquisition and retirement. All executing session operations,
runs, queue transitions, compactions and extension calls retain a session Arc. A lifetime can
retire only when its registry reference is the sole remaining strong reference and it has no
attachments. A storage failure remains fail-closed until explicit reopen/recovery. Unattached
new runtimes have a ten-second initial grace. Disconnect releases attachments after accepted
calls finish and keeps a thirty-second reconnect grace. Explicit release ends attachment
demand immediately; a 100 ms maintenance tick reclaims idle resources.

Retirement closes adapters using the saved **provider affinity ID**, which differs from the
stored session ID. Codex retirement also clears fallback/debug state; its standalone debug
socket-close operation still preserves those counters. Durable history, usage, request keys
and provider affinity IDs remain in SQLite. First assignment uses an atomic insert-or-read,
so concurrent first requests cannot create multiple provider IDs; a failed identity write
refuses the provider exchange instead of using an untracked ID.

Extension hosts acquire demand through a weak, generation-checked registry reference. Stale
hosts cannot dispatch operations or publish UI into a replacement lifetime. Reopening waits
for the preceding extension shutdown before activating the next generation. Questions and
notices get opaque IDs scoped to the lifetime; this deliberately strengthens the reference's
`q<n>`/`n<n>` scheme so delayed answers cannot resolve a replacement question. Standalone
QuestionHost fixtures retain the reference counter format.

Dropping the final connection sender wakes cleanup even if a live receiver has stopped
consuming its bounded reply queue. Accepted calls are drained before client attachments are
removed; late OpenSession tasks cannot recreate abandoned demand. In-flight sign-in operations
are cancelled on disconnect without cancelling durable runs or compaction.

## Executed checks

`E/runtime/lifecycle/tests.rs` exercises:

- Independent/idempotent client ownership; correct provider affinity cleanup; compatible
  reopening; actual engine protocol mismatch refusal.
- Detach during a gated run and follow-up: both settle, then retire, with history preserved.
- Disconnect grace and readoption; cloned input handles retain ownership.
- Sixty-four accepted OpenSession calls with an unread reply queue: disconnect drains and
  removes all attachment demand.
- Initial grace and an unpolled extension call holding demand; old host calls refused.
- Reopen while old extension deactivation is gated: new activation waits, old UI and calls
  are rejected, and a fresh question host works.
- Manual compaction with a gated model response survives client disconnect, commits and retires.
- A delayed answer to a retired question cannot answer its replacement.
- Fail-closed state survives retirement attempts until explicit reopen.
- Real maintenance-tick retirement after release, followed by history reopening.
- Sixteen concurrent first users receive the same persisted provider identity.

`cdx::retiring_a_session_discards_pooled_connections_and_fallback_state` uses a real local
WebSocket server to verify a new handshake and clean fallback/debug state after retirement.
The desktop's existing conversation-switch, subagent, recovery and extension suites are
included in the required workspace run.

`ext::a_panicking_deactivation_still_aborts_tasks_and_releases_questions` verifies that a
broken deactivation hook cannot strand other extension tasks or prevent lifecycle completion.

The first workspace run exposed a pre-existing timing race in
`oauth::xai_device_flow_and_cancellation`: its mock returned tokens after one pending poll,
before the test necessarily observed the device code. The mock now remains pending until the
test observes the code and a poll, then explicitly authorizes it. No production OAuth behavior
changed for that fix. The failed run is retained at `/tmp/pipkin-lifecycle-workspace.log`.

A second run caught an import protocol fixture pinned to version 5; its expected version and
ReleaseSession wire round-trip now cover protocol 6. That failed run is retained at
`/tmp/pipkin-lifecycle-final-workspace.log`.

Final required checks: **1371 passed, 0 failed, 53 ignored**, Clippy and formatting clean.
Logs: `/tmp/pipkin-lifecycle-release-workspace.log`, `/tmp/pipkin-lifecycle-release-clippy.log`,
`/tmp/pipkin-lifecycle-release-fmt.log`. Reconnect
grace arithmetic is exercised with an advanced retirement cutoff; this is not a measured
thirty-second wall-clock soak. No live provider, desktop automation, installed build, external
attach server, OS trust or private credentials were used.

## Reference and remaining qualification

Source comparison: pinned `d2a311097cbcf669e699479587332ae3988a49d0`,
`packages/coding-agent/src/experimental/session-worker.ts` (`WorkerLifecycle`, lines 185–306):
initial demand, in-flight request holds, harness work, attachments, and orphan demand grace.
The native engine uses one process and session resource lifetimes instead of one process per
session. The tests above are native assertions; no executed reference lifecycle differential
is claimed.

Still open: authenticated external attach, server-generation re-adoption across an independent
server restart, full service/transport differential tests, TypeScript facet/profile/reload
compatibility, and installed/live/platform qualification. Native remains opt-in.
