# Local CLI approval API (introduced in protocol 38; current protocol 39)

This is an owner-scoped facade over the existing receipt-backed broker and private listener.
It does **not** itself enable coding execution. RestrictedWrite additionally requires both
separate default-off host gates and a current owner grant. Core/native adapter/engine/fake CLI
flows pass offline tests, and GPUI controls pass headless tests. Production activation and
actual helper/CLI/desktop qualification remain pending.

## Calls

| Call | Meaning |
| --- | --- |
| `LocalCliApprovals { session }` | Read current owner's ephemeral pending reviews. Returns an optional external run identity and a bounded list (at most 16) of full escaped argument displays. No active owned controller returns an empty list and no run. |
| `DecideLocalCliApproval { id, decision }` | Explicit `AllowOnce` or `Deny`, on the control lane. Resolves the exact cached live ticket; does not reconstruct authority from client fields or receipts. |
| `LookupLocalCliApproval { id }` | Read-only durable receipt lookup, independent of the active controller. No probe, process, admission, delivery or retry. |

Unix Hello advertises `localCliApprovals` as API availability, **not readiness or a grant**.
No raw review is broadcast as an engine event, stored in native tools/history, or included in
model context. The current API is pull-based; explicit guarded frontend discovery/refresh is
implemented, but no automatic polling/notification handling exists yet. Render must never perform the requests or parse their contents.

## Identity and privacy

The ID contains engine epoch, exact engine session, binding generation, external operation,
external prompt receipt ID, invocation nonce and SHA256 of the complete original hook input.
It contains no CLI-private UUID, run nonce, account/config marker or caller-supplied owner.
The engine derives owner from its authenticated connection. Unknown identity fields and
unknown decisions are rejected by decoding. Run/key/hash bounds and current epoch are checked
before lookup or decision. Pending argument displays are already complete, bounded and escaped
by the broker; never truncate an approvable view or turn it into a native tool card.

Review and ID Debug implementations redact contents. Only the owner receives full review data.
A different connection sees no pending run/reviews and no receipt for that owner's key. After
reconnect/restart, absence is not proof that an earlier decision had no effect. Never resend.
The expiry timestamp is a display hint; actual authority uses the broker's monotonic deadline.

## Decision and uncertainty semantics

- The broker commits decision plus consumption before its single bounded transport delivery.
- A successful receipt says **Consumed**, not delivered or action completed. A CLI-reported
  action result is separate evidence and is never correlated to approvals by guessed order,
  tool name, arguments or fabricated tool-use ID.
- A delivery/persistence/postcommit cancellation failure can return `Cancelled` with an uncertain
  outcome. Read the receipt explicitly; do not send another allow.
- Same-decision duplicates with a committed Consumed/Denied receipt return only that receipt,
  even after actor retirement. They never enter the delivery callback again. A conflicting
  decision or any other non-Pending state cannot acquire authority.
- Pending/Granted/absent lookup results are metadata, **not permission to retry**. The future
  core must retain the attempted identity and suppress reapproval after uncertain submission.
- Denial retires the run's authority. Neither Stop nor denial undoes effects already released.

## Lifetime and lock ordering

The coding actor installs one weak controller registration per session before CLI launch and
holds its exact registration guard through teardown. Up to 64 active registrations are allowed;
there is no live-entry eviction/replacement. The registry tracks current connection owners,
rejects late installation after disconnect/shutdown, and never retains session demand itself.

Registry lookup clones the controller and releases its lock before taking broker/store locks.
Owner retirement removes membership and makes cancellation visible before releasing the
registry lock. Durable retirement stays with tracked actor/listener cleanup and BeforeSignal;
registry shutdown must not synchronously wait for publication or persistence before the engine's
shutdown deadline. Registration drop removes only its own weak identity, never a newer run.

Public admission derives owner, lease and persistent revocation token from the current setup
grant and rechecks that exact grant under its registry lock. Both host execution switches are
default-off; these approval endpoints cannot replace those gates.

## Frontend integration contract

- The pure-core reducer and command/backend/event ports now provide exact tuple/run guards,
  bounded complete reviews and sticky uncertain-decision recovery
  (`crates/pipkin-core/CLI_APPROVAL_API.md`). Native discovery validates retained backend prompt
  receipts against connection/epoch/session before exposing a returned run.
  The external prompt receipt is the CLI `Admitted.run`; compare it with the captured backend
  run, not whatever conversation is selected when a reply arrives. The wire binding generation
  is different from the desktop attachment generation; both must be guarded appropriately.
- Reject late reviews after run/attachment/connection change; a list refresh or Pending lookup
  must not resurrect an attempted allowance. Do not automatically retry decisions.
- Closed protocol DTO mapping is implemented in the native adapter. Only a current owner's full
  view may enable explicit Allow once / Deny; raw arguments remain ephemeral and redacted.
  The core's shared availability queries, not UI-side checks or selected-conversation aliases,
  drive the implemented controls (`crates/pipkin-ui/LOCAL_CLI_INTEGRATION.md`). They are verified
  headlessly, with separate integrated core→adapter→engine→fake coding actor tests, not real
  desktop input/accessibility or installed CLI qualification.
- Thin GPUI review must show the full inert display and captured scope, without truncation,
  executable links, native ToolCall/Question treatment, or render-time I/O/parsing.
- `SessionSummary.localCliMode` now carries immutable stored policy identity; missing/unknown
  mode fails closed in the adapter. `LocalCliRestrictedWrite` is a distinct core profile, not
  a grant. Authoritative readiness and current-grant admission are enforced independently;
  helper/managed-policy/live/native desktop qualification remains open.
