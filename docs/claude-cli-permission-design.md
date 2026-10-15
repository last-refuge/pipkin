# Local CLI permission path — implementation contract

Status: **Write-only integration and scoped live/native qualification recorded; owned desktop hosting now default-on at the user's request**.
Native146/158 and the ten-run ledger are documented in `claude-cli-bridge-evidence.md`;
`local-claude-cli.md` describes experimental testing and unqualified limits. The fresh Write
review failure remains recorded; resumed Write validated full arguments before Allow.
Job 99 was the offline workspace/strict-Clippy/fmt/diff checkpoint (protocol39/schema25).
Public coding admission requires both host capabilities and an exact current owner grant;
immutable profile identity alone grants nothing. No additional live account request is authorized.

## Boundaries

- Use the installed CLI's documented `PermissionRequest` command hook, not native tools,
  `ApiAdapter`, credential extraction, arbitrary CLI flags, or permission bypass.
- The installed Pipkin executable provides a Rust helper mode before logging, options,
  storage, engine or GPUI initialization. No example/Python dependency.
- Only requests actually delivered to the hook are mediated. CLI preapproved rules and
  managed hooks can act before this hook. Coding activation requires policy qualification
  and explicit consent to any supported preapprovals. Neither broker nor process groups
  provide a sandbox or universal interception.
- Initial offline foundation accepts exact bounded **Write** requests only. Edit and
  foreground Bash require further closed schemas, tests and version-specific qualification.
  AskUserQuestion, MCP elicitation, background jobs and native feature parity stay disabled.
- The internal `restricted-write-approval-v1` policy is distinct from the immutable chat-only
  binding. New policy means new conversation/CLI UUID, never in-place upgrade. Its preflight
  and command builder are implemented offline. Public setup now captures an explicit mode
  and can verify/create RestrictedWrite metadata. Coding grants additionally require Linux,
  `enable_local_claude_cli` and `enable_local_claude_cli_write`. Normal owned desktop hosts
  enable both; `--disable-local-claude-cli` and `--disable-local-claude-cli-write` opt out.
  Standalone servers/library configuration remain opt-in; attached hosts cannot be overridden.
  The public prompt handler derives admission from the current connection-owned grant.

## Identities and data

A permission invocation has engine incarnation, approving connection owner, external run
key `(session, binding generation, op)`, prompt receipt ID, a fresh host-generated invocation
nonce, full-input digest, exact policy/tool, and expiry. This is separate from desktop
`(conversation, attachment generation, operation)` and from observed CLI `tool_use_id`.
The hook has no reliable tool-use ID: never correlate by order, name or matching arguments.

Wire request/reply echo a fresh invocation nonce and original-payload digest under a
per-run secret nonce, on a dedicated private same-user Unix endpoint. They cannot approve
arbitrary requests or call engine administrative APIs. No reconnect/retry after uncertainty.
Payloads and nonce values are absent from Debug/errors/logs. Bounds apply before parsing.

Review arguments are complete bounded inert plaintext, prepared outside rendering;
control/bidi characters are visibly escaped. Truncated/redacted arguments cannot be approved.
Raw arguments remain ephemeral for owner-scoped review, not in the durable approval ledger.
The ledger retains identity, digest, tool, timestamps and decision metadata only.

## Authorization and durability

`Pending -> Granted -> Consumed`, or `Denied / Expired / Revoked`.

1. Validate scope and request; durably record Pending before publication to the person.
2. Explicit AllowOnce/Deny is checked against every identity, digest, expiry and live grant.
3. Commit the decision. A failed/unknown commit never creates usable authority.
4. Commit one-shot consumption before any allow bytes can escape. A duplicate decision is
   a read-only receipt, not another grant; a duplicate consumption cannot send again.
5. The in-memory gate serializes consumption/delivery with revocation. Delivery is bounded,
   synchronous/nonblocking inside that fence; no await while holding it.
6. Failed/lost delivery after consumption remains uncertain and is never reissued.

Consumed is **not tool completion or proof of an effect**. An already released allow cannot
be undone. Stop, disconnect, lease expiry, removal, shutdown and protocol failures close
in-memory authorization and durably revoke unresolved permissions before signaling. Persistence
failure still permits cleanup, but cannot prove clean revocation/cancellation. Coding settlement
must positively establish revocation and resolve pending permissions/actions, otherwise Unknown.
Startup revokes unresolved permissions and fences interrupted contexts; it never rebuilds grants.

## Receipt-backed controller (offline)

`runtime/external_permissions.rs` now composes the pure gate with `Shared` and real
SQLite transactions. It is not called by the chat-only executor and exposes no RPC;
the private listener below adapts its internal methods.

- `Broker::reserve` consumes an exclusive LaunchIntent → Running CAS with exact receipt,
  current engine epoch and coding policy validation. Future actor wiring must replace its
  ordinary Running CAS, not call this after the run is already Running.
- `Lease` takes engine-owned connection identity, grant revocation/expiry and owned-run
  cancellation. Never construct those fields from helper or approval JSON.
- `admit` binds the private run nonce and original payload digest, then commits Pending
  before returning a complete owner-only review. `lookup` is read-only receipt recovery
  within the same incarnation; it cannot restore grants.
- `decide` checks exact identity and immutable digest, commits decision plus one-shot
  consumption atomically, then rechecks monotonic expiry/cancellation before synchronous
  bounded delivery. Failed/lost delivery or panic cannot reissue; panic cancels the run.
- `seal` retires authority without signaling and retains unresolved-request evidence.
  `fence` seals before cancellation and returns revocation confirmation separately.
  Cleanup-only metadata transactions remain publication-serialized but work after removal
  or a session storage fence; the ordinary `Shared::commit` live guard stays unchanged.
- Native provider requests/tool rows remain absent in the controller fixtures. A synthetic
  race revokes the grant after commit and before delivery; consumption remains recorded,
  no allow is delivered, and the run is cancelled.

The parser accepts documented inert `transcript_path`, `prompt_id`, `scratchpad_dir`,
`effort`, and `permission_suggestions`. Paths are never opened and suggestions are never
applied. If present, `permission_mode` must be `default`. Unknown envelope fields, subagent
metadata and `tool_use_id` still deny. Duplicate keys are rejected even in ignored nested
suggestions. This follows the [public hook reference](https://code.claude.com/docs/en/hooks.md)
read on 2026-10-14; synthetic coverage is not installed-version/live qualification.

## Private listener (offline)

`runtime/external_permission_server.rs` connects real Unix sockets to the controller;
the internal coding actor instantiates it, exercised by offline fixtures only. The socket accepts hook requests, never owner decisions
or engine administrative calls. `Control` provides owner-scoped reviews, decisions and
read-only lookup for the future native protocol adapter.

- A controller grants at most one listener claim. Failed binding burns that claim;
  duplicate binding cannot replace or cancel an existing listener.
- The endpoint is a newly created private directory/socket outside the controller's
  captured project, not a caller-supplied current workspace. Ancestry and peer UID are
  checked; this remains a trusted same-user boundary, not malicious-same-UID containment.
- At most 16 connections and 128 accepted connections per run, with a two-second total
  handshake budget. Reviews are emitted only after durable Pending and transport
  registration. Queue overflow, authenticated malformed requests, extra data, pending
  disconnect, expiry and revocation fence authority.
- Replies are prepared before approval and released in one bounded nonblocking write
  inside the controller's fence, after durable consumption. Partial/WouldBlock delivery
  never resumes or retries. A matching reply is not evidence of a completed tool action.
- Unpolled/dropped/aborted servers fence; connection tasks independently retain the engine
  tracker until cleanup. Only the newly created endpoint directory is removed.
- Binding does filesystem work. If offloaded, its caller must register/retain a tracker
  token before offloading and recheck the execution lease before spawning any CLI.

Real-socket/controller fixtures and a packaged headless-helper exchange with a fake
private peer cover the offline path. They do not invoke Claude, execute Write, or qualify
an actual desktop coding session. The ordinary chat-only actor is unchanged.

## Distinct Write policy and inert action projection (offline)

`execution/claude_cli/write_policy.rs` builds the closed restricted-Write command and
per-run PermissionRequest hook. `preflight::check_for_policy` gives it a separate immutable
binding/configuration marker, checks `--settings` support, and uses the restricted auth
route with an unconditional blocking hook before a broker exists. Ordinary `check` and
`launch_spec` remain chat-only. Neither launch path can accept the other's binding.

The hook command has safely quoted host arguments, `onFailure: block`, Write-only inventory,
explicit default permission mode, and no broad allow rule, safe-mode, bypass, or auto mode.
Prompt bytes stay on stdin; the per-run nonce stays in the child environment. On Linux the
helper anchors the currently running host image through `/proc/<engine-pid>/exe`, validating
its metadata rather than executing a replaceable project pathname. This remains subject to
packaging/live qualification and is **not** a crash/orphan/PID-reuse guarantee. Other platforms
fail closed for this staged coding route.

Protocol **36** adds closed inert `ExternalBlock::Action` reports. Schema **25** backs up v24
and indexes source action anchors alongside external prompt/message receipts. The projector
supports mixed text/action indices, matches results only by exact prompt/source tool ID,
rejects duplicates/orphans/type changes, and retains cumulative display bounds. Raw ledger
APIs still reject receipt-backed runs. Approval IDs are never fabricated or correlated by
name/order/arguments. Missing/unknown result evidence cannot complete a receipt; recovery
retires pending observations as Unknown without replay. Source-reported success/failure is
not independent filesystem proof.

The native adapter renders these reports as bounded notices, not native tools, results,
approvals or retry controls. Tool output is excluded from prompt answer text and native
model context. Fake-executable, decoder/SQLite, migration, native-adapter and protocol tests
cover these paths; no actual Write or Claude request is performed.

## Owned coding actor (offline)

The internal coding route in `runtime/external.rs` and `external_coding.rs` now reserves the
broker instead of the ordinary Running CAS, tracks filesystem binding before offload,
and owns the permission listener alongside the process supervisor. Failed endpoint binding
or loss of the control receiver settles Unknown without spawning a model process. Preflight
freshness is rechecked after filesystem work and command construction, and the captured
coding lease is rechecked before spawn.

`BeforeSignal` calls the broker's **authority seal**, not its cancellation method: the
supervisor also signals descendants after a normal root exit, so feeding that cleanup back
into the run cancellation token would incorrectly cancel successful turns. Both completion
and clean cancellation require explicit positive supervisor revocation confirmation.
Completion additionally requires a live lease, no unresolved seal evidence, clean independent
process evidence and accepted projection with known action-result status. Consent revoke/
expiry can record Stop; protocol/delivery failures cannot manufacture intentional Stop.

A disconnect watcher retires before reply bytes can escape, so a peer reading and immediately
closing a successful reply does not incorrectly fence the run. Delivery errors still fence
and never retry. Sealing releases the broker's session demand: retained read-only controls
must not block session removal. Listener/handler/bind work remains tracked through cleanup.

The fake CLI is a Rust test subprocess using real private sockets and receipts. It writes only
its own temporary fixture after AllowOnce. This does not invoke Claude or qualify the actual
CLI/desktop helper packaging boundary. The public actor remains chat-only/default-off.

## Integration still required

- Schema 24 approval ledger and strict parser/gate/helper are additive offline foundations.
  Common hook metadata now has explicit synthetic coverage; actual installed-version
  envelopes and managed-policy behavior still require authorized live qualification.
- The helper accepts pipe/socket stdout only; even rejected descriptor metadata is inspected
  in a bounded worker. Filesystem checks cannot authorize after timeout. Stdout failure
  requires qualified hook `onFailure: block`, not an assumption that exit 2 alone denies.
- The internal actor now uses the Write-only decoder and receipt-aware projector. Keep
  unknown action evidence as Unknown settlement, never success or a stuck active receipt.
- Approval DTOs were introduced in protocol 38; protocol 39 adds immutable mode projection.
  The advertised approval API is neither execution readiness nor a general tool service.
- Public owner-scoped admission, pull-only controller ownership and core→adapter→engine→fake
  CLI/socket flows now pass offline tests, including a lost decision reply with exactly one
  decision RPC and explicit read-only recovery. Captured GPUI controls pass headless tests.
  Admission captures owner/lease/persistent `grant.cancel`, then rechecks the exact grant under
  the setup registry lock. The temporary preflight Turn token must never own actor lifetime.
  Keep production execution disabled until applicable qualification gates pass.
- Captured-project Changes observation/invalidation is implemented with source/operation
  guards and isolated non-executable Git comparison. It never attributes files to CLI actions
  or reads permission arguments as paths. Persistent uncertain recovery remains. Inert action
  notices must never become native tool controls, model context, replay or automation.
- Qualify the Linux helper-image packaging/lifetime boundary with the actual desktop host;
  the offline command builder is not that qualification.
- Fake crash/race/backpressure tests precede any live/native qualification. Billing, managed
  policy isolation, real crash/orphan recovery and actual native input remain separate gates.
