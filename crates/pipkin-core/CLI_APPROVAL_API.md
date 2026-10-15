# CLI approval core, adapter and UI (offline integration verified; production disabled)

`cli_approvals.rs` is framework-free and owned by `AppState`. No engine-protocol dependency,
serialization, native tool/question, history/model-context insertion, I/O, or automatic request
exists in the reducer. Dedicated command/backend/event ports and the native RPC mapping are
implemented, as are captured-run GPUI controls and separately default-off current-grant coding
admission. Integrated fake actor flows pass job 94. Engine API:
`docs/claude-cli-approval-api.md` (protocol 39/schema 25).
`LocalCliRestrictedWrite` is now a distinct authoritative core profile projected from the stored
session policy. This is identity only; both host switches and current owner consent are required.
Production remains disabled pending hardening and actual qualification.

## Identity and requests

`CliApprovalContext` pins desktop conversation, attachment generation and **model run op** to
an external `CliApprovalRun` (engine epoch/session/**binding generation**/external op/prompt
receipt). These two generations and two op domains are NOT interchangeable. A request also
has a distinct monotonically allocated desktop **RPC op**. A reply must match the complete
context and outstanding RPC op. Wrong tuples are ignored without consuming the pending op;
malformed matching replies retire the scope, never partially adopt a list.

`bind` requires trusted authoritative context; it is not a user command or backend grant.
Discovery is guarded by `(conversation, generation, RPC op, model run op)` in a bounded
pending map.
The native adapter now retains `Admitted.run` in `Worker::Run` as a CLI prompt receipt, bound
to connection/epoch/session. Discovery checks it against `LocalCliApprovalRun.prompt` before
installing context. Lost-ack recovery may populate missing proof from a verified prompt Lookup;
an existing proof is never replaced by a different receipt/connection. It is never derived from
a native snapshot run or whichever conversation is selected. Binding the selected
conversation to an unvalidated run returned by a list is NOT sufficient. An initial guarded
list may be installed by bind/request(List)/apply(Listed) only after discovery's pending op and
backend run identity have been validated; do not invent authority from a cached DTO.

`request` marks a decision attempted **before** returning a request description. The AppState
port emits `JournalCliApproval` first, not a decision RPC. Only the exact durable acknowledgment
may release one backend effect for that returned request. An adapter rejection or a request never sent is
still conservatively Failed/Unknown here, never an opportunity to re-send. One operation per
scope is pending at a time. Request descriptions are data, not generic native tool execution.
UI must use `AppState::cli_approval_discovery_available` and `cli_approval_availability`, which
combine reducer policy with connection, actual attachment, ownership, authoritative coding
profile and active-run checks before binding or issuing decisions. Never grant this from chat-only profile or metadata mode.

## Uncertainty and bounds

- An attempted ID is permanently non-decidable within its run, including Deny attempts.
  Failed/absent/Pending/Granted receipts cannot restore Allow or Deny. Only explicit read-only
  Lookup can resolve an attempt. Consumed is NOT delivery or action-completion proof.
- Completed receipt knowledge cannot regress on a later failed/absent/pending lookup. Stop is
  a separate run action, not undo, and never proof that a prior action did not occur.
- At most 8 conversation slots, 128 seen invocation identities per run, 16 reviews per list,
  32 KiB per complete ASCII-escaped argument display. No truncation or silent eviction. Over
  capacity fails closed. Argument bodies/IDs redact Debug. Expired, missing or attempted
  reviews are not rehydrated by a refresh. A changed digest, deadline, or still-retained body
  for an existing invocation invalidates the whole reply.
- Conversation slots retain high-water engine epoch/prompt receipt identities. Resolved
  sequential runs reuse a slot only with a strictly newer external receipt and desktop model
  op. Old runs cannot be rebound. Unknown decisions block replacement, not silently disappear.
  The eight-conversation bound now persists per desktop namespace for saved attempts. Known
  scopes can advance to separately verified later runs in that same conversation; other slots
  are not silently evicted. At capacity, new conversation scopes fail closed, including after
  restart. Do not delete recovery records or reset a profile to bypass uncertainty.
- `tick` uses an injected millisecond UI clock; local expiry retires display/decision ability.
  Clock regression retires all authority permanently for this reducer instance, but explicit
  lookup remains possible. Engine monotonic expiry remains the real authorization boundary.
- `retire` removes authority and plaintext, retaining attempted IDs/uncertainty. Repeated
  retirement does not cancel a read-only lookup after a run ended. `invalidate` additionally
  retires pending lookups for attachment/ownership loss. `retire_all` is used on disconnect.
  Rebinding a retired run never restores decisions. Switching selection alone does not retire
  another conversation's active reviews.

`AppState` advances the clock, retires on disconnect, and reconciles lifetime before/after
commands/events. Conversation removal, generation/ownership/profile loss invalidate operations;
Stop/terminal run/readiness loss retire decision authority while preserving lookup. These hooks
are retirement only, not an approval admission path. Only `LocalCliRestrictedWrite` retains
review authority; changing to chat-only retires it. Tests seed a coding context directly; they do
not demonstrate an actual frontend RPC flow or permission to execute coding.

## Durable attempt and receipt recovery

`Effect::JournalCliApproval` contains the captured decision request (IDs and choice, no arguments).
The controller commits `CliApprovalRecovery::attempt` and returns the exact request/result through
`cli_approval_persisted`. The port rechecks pending identity, attachment/run, owner eligibility,
clock/deadline and live authority. Failed, duplicate, stale, expired or retired acknowledgments
never release a decision. Responses arriving before release are ignored. A failed journal still
retires the attempted decision in memory; storage recovering does not retry it.

Matched terminal receipt knowledge emits `SaveCliApprovalRecovery`. This saves metadata, not
proof of delivery/action; a failed save leaves conservative uncertainty on disk and reports a
captured-scope notice. Stored knowledge never regresses. Conflicting identity/digest/choice or
replacement of an unresolved run fails closed in a transaction. Desktop schema **10** stores
bounded JSON with a closed schema, pre-decode byte bounds, namespace isolation and eight contexts /
128 attempts per context. It contains no raw arguments, prompts, grants, nonce authority or
connection proof. Engine protocol/schema remain **39/25**.

Startup loads this journal independently of normal cache fallback. It validates the entire set
before installation. Unreadable/malformed/over-capacity data disables new review/decision scopes.
Restored views are retired, plaintext-free and unattached (desktop generation zero); exact
external run identities and desktop operation high water are retained. No lookup or RPC is sent.
Explicit reattachment changes only the desktop attachment, never the external run or authority.

**Original-owner isolation is unchanged.** Restart does not reconstruct the native adapter's
connection/prompt proof. Lookup without it fails closed rather than querying a foreign owner or
an archived cross-owner API. An unknown result can therefore remain unresolvable after restart;
inspect files/transcript and never replay it. Already observed terminal metadata remains useful,
but cannot authorize a decision. Full capacity remains a visible conservative limitation.

## Implemented port and remaining integration

- `Command::CliApproval`: Discover captures a running coding target; Act captures an exact
  context and boxed List/Decide/Lookup action. Reattach explicitly moves retired receipts to
  the current attachment for read-only recovery; it sends nothing and cannot restore decisions.
- `BackendRequest::CliApproval`: Discover carries a guarded query, Known carries the reducer's
  request. The adapter maps protocol DTOs explicitly and checks current attachment/profile,
  connection/epoch and prompt receipt before reads/decisions. Lookup may outlive the model run.
- `EventKind::CliApproval(Box<_>)` is routed before normal model-op matching. Both outer and
  payload tuples are checked. Independent discoveries can complete in reverse order without
  reusing operation IDs. No event emits another approval request.
- `LifecycleEvent::CliApprovalsAvailable` wires Unix engine API availability through the
  controller/Model to AppState. It grants neither a coding profile nor readiness. Demo and
  offline backends fail closed. Disconnect retires pending operations and all authority.
- Native RPCs use the existing connection on the non-GPUI worker; server decisions are control
  lane calls. Stop/Revoke retain their separate existing frontend control worker. No automatic
  polling, decision retry, uncertain lookup or request redirection is performed.
- Full argument/string/vector capacities and the event box are counted in the event queue.
  Argument displays never pass through native questions/tools/history or error notices.

Core ports, fake-protocol adapter tests, headless GPUI→core tests and core→adapter→embedded
engine→fake coding actor flows are implemented. These are not real native desktop qualification. The thin GPUI panel uses these commands with captured scope, complete inert text and
explicit read-only recovery (`crates/pipkin-ui/LOCAL_CLI_INTEGRATION.md`). Opening, closing,
rendering, ordinary Enter and reattachment do not send approval RPCs. Presentation cache
retirement never resets core attempts. Stop is additionally guarded by the captured generation/
model op and current selection through `cli_approval_stop_command`; API loss does not disable
ordinary Stop. Reattach is unavailable/no-op for an already-current attachment.

The grant-derived actor path remains default-off pending actual qualification. Polling/list
updates must not trigger decisions or automatic uncertain lookups.

GPUI must render the complete escaped display as plain inert text, not markdown or native tool
cards. Allow once and Deny use one shared availability function; after an attempt show only
read-only receipt recovery and independent run Stop. Preserve attempted state across view
closure/reopen; never instantiate a fresh reducer to clear uncertainty. No render-time I/O or
parsing. Saved attempt/receipt metadata must never become replayable requests or model history.
