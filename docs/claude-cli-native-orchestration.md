# Claude CLI native orchestration implementation

**Handoff:** see [claude-cli-orchestration-handoff.md](claude-cli-orchestration-handoff.md).
Implementation resumed on 2026-10-18. Child and job wakes now have focused offline
coverage (slice22 below). Slice24 identifies and fixes SIGINT-driven helper connection loss
with a failing-then-passing offline reproduction; final verification is recorded below.
Slice26 implements child continuation and inherited context with a passing offline workspace
gate. Slice27 adds goal lifecycle/auditors; slice28 adds context, compaction and queued input.
Slice34 adds explicit desktop activation and routing; slice35 adds effort, person children/steering, MCP settings and Skip-audit. Live qualification remains below.

Status: orchestration is available through explicit desktop setup in owned Linux builds.
Broader installed-CLI lifecycle qualification remains incomplete; dashboard tests do not
replace it. The live-qualified `cli-unattended-coding-v1` path retains its separate executor.
The previous dashboard exclusion was superseded by the owner’s 2026-10-19 request to finish,
test, commit and push it. See [dashboard qualification](orchestration-dashboard-plan.md) for
the protocol42/schema35 dashboard and its independent native-window checks. Historical slices
below retain their original scope. No additional live inference was authorized or used.

## Current capability checklist (360 plus resumed slices22–35)

Progress is tracked by working capabilities, not a percentage or the number of foundation tasks.
"Working offline" means integration with simulated CLI/SDK peers, **not** installed CLI or
real desktop qualification. Slice34 connects an explicit desktop orchestration choice; Coding
continues to use its separate executor.

| Capability | Current state |
| --- | --- |
| Orchestration dashboard | Implemented: global directory, independent Markdown viewer, explicit root actions and linked child follow-ups, durable drafts/control receipts. Native and fake-CLI tests; no new installed-Claude qualification. See the dashboard plan for measured gates. |
| Existing CLI Coding path | Separate executor; 2026-10-19 fix accepts late CLI-owned tool observations and collapses output/thinking with shared assistant Markdown formatting when expanded. Repeated per-turn init preserves work and revalidates the catalog. Coding aggregates held per-turn results at EOF, retaining errors and counting identified usage once. Uncertain interrupted turns remain fenced; updated live behavior awaits user use |
| Ordinary Coding clean Stop and historical recovery | Implemented offline: new clean Stops preserve continuation; older clean Stops can recover through fresh matching reconnect with a schema34 audit. Same CLI session and new instruction only; old receipts/actions preserved, no prompt replay. Unknown failures stay locked; installed recovery/resume remains unverified |
| Native bridge, tool effects, root execution/history/receipts | Working offline; installed CLI2.1.301/Sonnet fresh and same-UUID resumed native Write passed after SIGINT fix |
| Scoped native plans and questions | Working offline, including existing reply RPC contracts |
| Fresh inline CLI child → native effect → result | Working offline through347; parent Stop and MCP replay covered |
| Child profile/model restrictions and shared resource cache | Connected to the inline path; no API/default-model fallback |
| Fresh background children, notification-only | Working offline through360; original connected approval, durable handoff/cancellation and cleanup-only recovery |
| Child/job wake | Working in focused offline fixtures; original connected approval, same-UUID resume, cancellation, capacity wait and restart suppression; full workspace gate passed |
| Explicit child continuation | Working offline through slice26; same-UUID return under the original connected parent approval, frozen policy/resources, inline/notify/wake delivery; offline qualification recorded in slice26 |
| Inherited child context transport | Working offline through slice26 via mandatory read-only native `parent_context`; frozen role-labelled excerpt, no task concatenation or CLI transcript edits; offline qualification recorded in slice26 |
| MCP tools/resources/prompts and scoped form elicitation | Working offline through350 using the existing MCP service |
| Web search/fetch/read and native tool progress | Working offline through350; scoped retention/publication and bounded progress |
| Child-directory status/result/wait | Scoped routes tested empty/foreign through350; real child data reads/waits, failed-result notes and running-task views through360 |
| Jobs start/status/log/wait/cancel | Working offline through353; CLI-owned wake added in slice22 |
| Goal get/update/evidence-based completion proposal | Working offline through slice27; real scheduled goals, mechanical evidence and required independent CLI audit |
| Goal scheduling/start/resume/audit | Implemented and tested offline in slice27; atomic admission, same-UUID worker resume, fresh read-only auditors, allowances/stalls/Stop; dollar budgets safely refused while cost is unavailable; desktop routing added in slice34 |
| CLI context and automatic compaction observations | Implemented privately in slice28; last model request input is distinguished from cumulative usage; unknown capacity/post-compaction size stays unknown |
| Explicit CLI compaction | Implemented in slice28; same-UUID `/compact`, native effects denied, actual boundary plus clean completion required; installed behavior unverified; desktop routing added in slice34 |
| Follow-up queue and withdrawal | Implemented in slice28; bounded durable queue, exact-repeat lookup, FIFO, original connected ticket, cancellable capacity wait and recovery suppression; desktop routing added in slice34 |
| Mid-turn steering | Implemented in slice29 at authenticated native tool-result boundaries, with durable user entries and exact response replay; next-clean-turn fallback; installed behavior unverified; desktop routing added in slice34 |
| Queue batching | Implemented privately in slice29; one/all settings, bounded same-kind FIFO batches, per-message outcomes, compaction barriers and withdrawal; installed behavior unverified |
| Background delivery and scheduling integration | Implemented privately in slice30; user-input/ready-wake/goal ordering, independent completion deadline, committed job handoff and frozen child job resumes; offline tested, installed/desktop qualification remains |
| Aggregate background goal usage and allowances | Implemented privately in slice31; causal receipt totals, late/archive/recovery reconciliation, turn and token admission checks, unknown-usage refusal; installed qualification remains |
| Native tool concurrency and per-call cancellation | Implemented privately in slice32; four workers, ordered sequential barriers, byte-bounded pending work, exact cancellation receipts and forced-cleanup fencing; installed qualification remains |
| Native payload alignment | Implemented privately in slice32; 2 MiB prompts, 32 MiB MCP frames, response-envelope margin and matching bounded SDK observation path; finite resource ceilings documented below |
| Post-Stop behavior | Implemented/tested privately in slice32 for safe fresh/clean continuation and stale-ticket suppression; launched uncertain turns remain non-resumable with retained effects |
| Thinking and effort | Implemented privately in slice33; immutable requested CLI effort, inherited/frozen child profiles, thinking display; effective model/organization clamping remains CLI-owned |
| Attachments | Implemented in slice33; durable image identity, structured stdin/MCP delivery, image-only inputs, queue boundaries and exact replay; existing text-file/reference format retained; desktop routing added in slice34 |
| Remaining usage metadata | Implemented privately in slice33; optional token subsets, bounded telemetry and cumulative model/cost snapshots, once-only native totals and durable receipts; billed cost stays unavailable |
| Public setup/model/approval/input route | Implemented in slice34 for owned Linux desktop; explicit orchestration consent, fixed aliases, reconnect, prompts/images/queues, native views and CLI goal scheduling; offline tested |
| Creation-time desktop effort | Implemented in slice35; stale confirmation invalidation, immutable saved request and CLI-owned effective limits |
| Desktop child launch/steering | Implemented in slice35; explicit person receipts, fresh/inherited context, notification-only delivery, active tool-boundary steering; undelivered input is reported without restarting the child; auditors refuse steering |
| Desktop MCP settings | Implemented in slice35; selected root project scope, existing test/reconnect/tool/auth/import routes; configuration does not grant inference |
| Desktop Skip-audit | Implemented in slice35; explicit skipped verdict, matching-auditor cancellation, late receipt/usage reconciliation |
| Installed CLI lifecycle and desktop qualification | Fresh/resumed native Write passed for CLI2.1.301/Sonnet; wider lifecycle and desktop remain; current allowance exhausted |

User-directed implementation order: finish end-to-end workflows and reuse existing services in
batches, then broader refinement/qualification. Keep only focused correctness checks during
implementation; batch full workspace verification at integrated milestones and handoffs.

## Selected ownership boundary

Use the installed, unmodified CLI for inference/its agent loop and normal login. Use an
engine-owned **MCP tool channel** for Pipkin tool requests/results. The orchestrated launch
must disable CLI built-in tools and unrelated MCP servers/hooks; only the scoped Pipkin tool
bridge is offered. Native Pipkin owns tool effects, plans, jobs, questions, child lifecycle,
queues, goals, durable receipts and worker identity. There must not be two executors
for one call. No native/API model substitution, including child/profile/default-model paths.

This needs a separate immutable orchestration policy and execution driver, not a modification
to an existing binding and not a new `ApiAdapter` that serializes history into a user message.
The shared native run/tool services will be reused under explicit orchestration authority;
`require_native_execution` must not simply be removed to let historical external sessions in.
CLI output remains observation. Neither `<invoke>` text nor streamed tool-use observations can
admit native effects. The independent authenticated channel and owning runtime grant are the
execution boundary.

## Slice 1: protocol session (implemented, connected to the inactive native driver)

`execution/claude_cli/orchestration_protocol.rs` is a pure, bounded MCP state machine:

- Explicit initialize → initialized → ready sequence; fixed server protocol2025-06-18.
- Export the supplied native `ToolSpec` schemas, without name rewriting, hidden tools or an
  executable transcript parser. Unsupported grammar-only catalog entries fail closed.
- Capture `(session, generation, operation, native run)` on the engine side. Client-supplied
  scope fields cannot replace it. Opaque call keys additionally carry an instance fence.
- Typed RPC IDs distinguish numbers and strings. Canonical request fingerprints detect
  conflicting reuse. Identical active requests join the existing result; identical completed
  requests require durable receipt lookup, never another execution.
- Cancellation targets an outstanding call once; it does not manufacture a tool result or
  claim an effect was undone. Closed channels reject late completion/new requests.
- Completed result fingerprints reject contradictory replay. The caller must commit the
  actual native result before completing the protocol request and read that exact receipt on
  replay. This module is **not** the durable ledger or a proof of permission.
- Limits:256KiB request frame/catalog,128 catalog tools,4096 distinct request IDs,64 pending
  calls,1MiB response. Receipt bodies are not retained in the protocol session; catalog data
  is shared. No silent identity eviction. Resource/conflict failures fence the channel.

These initial limits are conservative transport limits, **not a claim of attachment or large
native-payload parity**. Before activation, service integration must reconcile native transport/output limits and
byte-bounded concurrent delivery before activation. Runtime validation and helper launch are
now exercised by slice4, not established by this pure module. Dynamic availability and real
installed-CLI MCP compatibility remain integration obligations.

## Slice 2: durable journal substrate (implemented, not an executor)

`store/cli_bridge.rs` / `store/cli_bridge.sql` introduce engine schema26, with an additive
migration and no protocol/desktop schema change. The reserved database identity
`claudeCliAgent` is deliberately still `Unsupported` to existing readers: no approved runtime
creator, model-picker route or native/API permission is added. Protocol-only tests seed raw
rows; slice5 adds a storage-only binding creator used by native-driver tests.
Native and historical `localClaudeCli` sessions cannot prepare a bridge; their guards remain.

- Immutable bridge identity records the captured session/generation/op/native run, owner,
  binding digest, capability digest and fixed `cli-native-orchestration-v1` policy. The future
  trusted driver must capture/verify that binding and authenticate the private transport;
  **matching a stored digest is not authentication**. This journal is not a sandbox.
- Preparing a duplicate identity is lookup only. Claiming the connection commits once before
  returning a non-serializable owner handle. A lost acknowledgement never issues a second
  handle; changed scope, owner or digests conflict without revoking the legitimate owner.
- Tool intents commit before a dispatch claim is returned. Duplicate calls remain Pending or
  return the committed result, never another claim. Typed RPC IDs and canonical argument
  fingerprints prevent aliasing; call IDs are stable per bridge/request. Completion commits
  before reply. Conflicting requests/results fence transactionally, preserving original results.
- Stop/disconnect, uncertain closure and startup retire outstanding intents to Unknown, not
  fabricated success/cancellation. Late workers cannot promote Unknown. Engine startup only
  retires old bridge authority; it never reconnects helpers or executes a stored intent.
- Removal intent revokes authority before cleanup. Removal deletes arguments/results while
  retaining permanent nonsecret identity reservations; it cannot make an old bridge fresh.
- Initial limits match the protocol request/pending/result ceilings and add a32MiB aggregate
  argument/result journal budget per run. These remain provisional; large native payload and
  attachment parity is not established. An invalid/oversize result fences without fabricating
  completion. Storage errors require the future driver to revoke its in-memory authority too.

**Task76 progress:** slice4 joins these records atomically to native tools/transcripts and
executes native tools through the authenticated channel. Slice5 adds immutable storage-only
session bindings; slice7 adds root admission/outcome journaling. Approved runtime creation,
CLI launch and supervised root evidence remain absent. A provisional Claim alone
is not execution authority; existing native-only execution guards remain unchanged.

## Slice 3: authenticated channel and dedicated helper (implemented, not CLI-wired)

- `execution/claude_cli/orchestration_wire.rs` mutually authenticates a fresh host-generated
  256-bit secret with HMAC-SHA256 challenge proofs. Proofs bind the full captured bridge
  identity digest and fresh client/server challenges. The secret is never sent on the socket.
  Every frame also authenticates direction, transcript, sequence and exact body. This is
  loopback IPC, **not encryption or a sandbox against root/same-UID code**.
- `runtime/cli_bridge_channel.rs` binds only IPv4 localhost on an ephemeral port and prepares
  a NEW journal identity. Wrong secrets/scopes do not consume the legitimate owner's grant.
  After peer verification, connection admission must commit before the helper gets its final
  proof. The listener is consumed; neither lost acknowledgements nor disconnect allow rebind.
  Normal runtime startup starts no listener and no new execution route.
- Eight failed peers maximum; two-second handshake, ten-second frame assembly/write bounds,
  and a thirty-minute maximum host lifetime. Frames are bounded before allocation, with one
  frame per half and no unbounded output queue. Cancelling a partial read/write, malformed
  frames, bad MAC/sequence, expiry or dropping a half revoke both halves. Reads are deliberately
  **not cancellation-safe**: slice4 uses a dedicated bounded reader task, never a select loop
  that discards partially consumed reads to handle unrelated events.
- `orchestration_helper.rs` adds dedicated `--claude-native-tool-bridge` early-main dispatch
  to both binaries, before logging/storage/GPUI. Normal mode reads no helper environment.
  Helper configuration is closed, size-bounded and loopback-only; the sensitive value belongs
  in the private child environment, never argv/diagnostics. The helper relays MCP JSON lines
  over the authenticated binary channel, never interprets CLI stdout or invokes a native tool.
- The stdio implementation currently supports **Unix pipes/sockets only** using nonblocking
  owned duplicates. Regular files, devices, terminals and unsupported platforms fail closed
  without output. This does not restrict the existing Coding path. Restoring descriptor flags,
  bounded output stalls, partial input and helper lifetime are tested offline.
- Retirement is explicit owner cleanup, not a peer endpoint. Drop closes sockets/cancels wire
  work without blocking SQLite in a destructor. The future driver must revoke in-memory work
  and stop processes before awaiting retirement, including on cancellation/storage failure.
  A cancelled preparation may leave an unacknowledged row; bind never reuses it and startup
  retires it. Helper exit/EOF is NOT native tool completion evidence.

An actual **debug engine helper process** is exercised against the host/protocol/journal with
an explicitly simulated no-effect tool and committed receipt replay. No installed Claude CLI,
account, native tool, native provider or GUI is exercised by those tests. The app entry point
is compiled, not live-UI-qualified. There is still no orchestration session creator or CLI
launch policy, and no normal model-picker capability is advertised. Those original slice3
fixtures did not qualify native execution; slice4 adds separate real-native-tool coverage.
Installed-CLI/MCP compatibility, orchestration lifecycle and dashboard remain unqualified.

## Slice 4: joined native tool driver (implemented, inactive)

- `store/cli_bridge/native.rs` atomically commits bridge admission, native `tool_calls`, a
  canonical authenticated-channel tool-call entry and permanent history IDs. Native result
  content/details, retained output, session revision/outbox and the exact MCP receipt commit
  together before reply. These reconstructed tool entries do not claim CLI text/round ordering
  or establish model-context replay. All calls are non-replayable, even normally read-only tools.
- Fencing/startup marks outstanding joined native calls Interrupted while keeping bridge
  receipts Unknown. No synthetic success/cancellation result is inserted. Actual effects may
  already exist. Late results cannot promote Unknown. Conflicting native metadata or retained
  output fences without overwriting the committed original, even if MCP-visible content matches.
- Native-specific channel preparation commits a domain-separated digest of the approved
  binding identity and native catalog. The existing capability digest remains the channel
  secret's digest, not the catalog. Only the server constructs the sealed Connected handoff;
  generic protocol-only connections cannot be promoted into native dispatch.
- `runtime/cli_bridge_driver.rs` owns the authenticated reader, protocol decoder and serial
  dispatcher. It verifies the actual schema catalog hash against the captured owner grant,
  checks for changed tool specs, captures stored scope/project/CLI alias, and reuses `tools::run`,
  schema validation, output bounds, `ToolCtx`, mutation locks, process permits and progress
  checkpoints. It has no model adapter/fallback and accepts no CLI stdout/text as authority.
- A dedicated reader buffers one bounded frame. Protocol pending limits bound queued requests;
  one tool worker and one outgoing response are active. Owner Stop/link loss/expiry/any MCP
  cancellation conservatively revoke the whole lifetime. Cooperative worker cleanup is bounded
  to five seconds, followed by abort; durable fencing follows cancellation, not the reverse.
  Every exit fences. EOF is not root-model completion. If the service future itself is dropped,
  its owner still must await captured-identity retirement; Drop cannot perform SQLite cleanup.
- MCP text/images remain structured. Initial1MiB MCP/native-result-entry limits and64MiB native
  transcript/retained-output budget per run supplement the32MiB bridge journal ceiling. These
  fail closed, not silently truncate into success; they are **not large-payload/attachment
  parity**. Execution is initially serial and cancellation lifetime-wide, not full native
  parallel/per-call behavior. No claim of complete workflow/tool-host parity is made.
- New offline tests exercise real native Write/Read through authenticated sockets; replay
  preserves a subsequent manual file edit; schema failures are actual native error results;
  unknown tools have no native intent; failed intent commit executes nothing; failed result
  commit leaves an actual Write effect Unknown; Stop/disconnect drop queued work; capability
  mismatch and conflicting IDs fence. Three process-exit cuts use real Native WriteTool and
  reopen committed WAL, preserving known results and refusing uncertain re-execution.
- A separate actual debug-engine-helper test exercises native Read/Write/Edit/Bash, scoped
  model environment, committed replay, and Stop/reaping of its owned shell process. It uses
  an isolated temporary project/home and no installed Claude CLI, provider inference or GUI.
  The default checkpoint host intentionally has no child/orchestration services. Normal runtime
  does not call this driver: reserved backend rows remain Unsupported with no runtime creator.

**Gate292:** all21 new tests, full workspace (engine1433/app336/UI217), strict workspace/all-target
Clippy, fmt/diff, frozen source equality and unchanged live ledgers passed. Artifacts
`/home/jason/.pg.rPctl4`, snapshot
`5cade47b38079cb7c7cff3d8ea616486db7fd1f655495f5f2d5906bf846ee8ca`.
Earlier failures289–291 are recorded in the scorecard; no lint/test was suppressed.

## Slice 5: immutable storage bindings and global CLI ownership (implemented, inactive)

- Engine schema27 adds `cli_agent_bindings` and permanent `cli_uuid_owners`. Migration copies
  existing legacy CLI UUID ownership, including removed sessions, without rewriting histories,
  backend/model identity, bindings or Unknown receipts. Original legacy receipts remain intact.
- `store/cli_agent.rs` provides **storage-only** Create/lookup. It freezes session/request key,
  CLI UUID, canonical project/executable paths, stable version, opaque config/account markers,
  orchestration policy, CLI model alias/thinking and parent/root identity into a digest. Exact
  repeats return the existing receipt, not a launch grant. Raw session/model/backend changes
  cannot rebind it. This validates metadata shape, not executable/login/approval evidence.
- One UUID/engine-session/creation-key namespace covers both external Coding and orchestration.
  Concurrent cross-policy creation yields one owner. Removal deletes binding payloads but keeps
  ownership, including child creation keys; neither another backend nor changed payload can
  reuse an old context or turn an uncertain Create into a fresh one.
- Child storage identities must inherit project/executable/version/config/account/policy from
  a live, enabled, unblocked orchestration parent. Another CLI alias may be chosen explicitly;
  a native/API profile, foreign parent or removing group is rejected. These are child identity
  records only, not implemented child launch/lifecycle/profile parity.
- Native channel preparation verifies the supplied binding digest against stored metadata;
  the driver checks bound context and native admission checks binding presence in its intent
  transaction. Unbound reserved rows remain useful only to protocol/journal fixtures, not tool
  dispatch. The sealed handoff also captures the host deadline so a caller cannot extend it.
- Offline tests cover exact repeats, failed commits/outbox rollback, identity conflicts,
  invalid aliases/versions/policies/paths, SQL immutability, bidirectional UUID collision after
  removal, child ownership, concurrent Coding/orchestration creation, reopening, v26 backup
  and preservation of Unknown/removed receipts, and native-channel binding mismatch.
  The actual native-helper test now uses the immutable storage creator, not a raw session row.
- **At gate295:** independently approved runtime creation/preflight for this policy, installed
  CLI launch/root outcome accounting, live session publication, model-context ownership,
  complete native ToolHost services and dashboard. No new capability or native/API fallback.

**Gate295:**13 new unit tests, actual native-helper integration, full workspace
(engine1446/app336/UI217), strict workspace/all-target Clippy, fmt/diff, source equality and
unchanged live ledgers passed. Artifacts `/home/jason/.pg.UHG8zn`, snapshot
`09557bcaeba269766e5530c33a966d6c2d73c2cf54810a45f9bf8353380bca11`.
Engine schema27; protocol41/desktop schema10 unchanged.293 failures and294 targeted pass are
recorded in the scorecard; historical downgrade fixtures now remove the additive v27 objects.

## Slice 6: distinct preflight and private launch construction (implemented, inactive)

- `preflight::check_orchestration` requires explicit executable/project trust and independently
  checks this policy's flags, stable version and normal CLI login metadata. The policy/config
  marker differs from Coding; Coding preflight cannot create orchestration metadata or launch
  it. Freshness/revalidation still apply. Native configuration identity is shared across child
  aliases; the separate immutable binding digest and launch model check freeze each alias.
  This permits a Haiku child of a Sonnet parent without allowing a Haiku preflight to launch
  that Sonnet session. Existing Coding markers are unchanged. No RPC/setup/model-picker route
  calls this function.
- `orchestration_policy.rs` constructs a native-MCP-only recipe: empty built-in tools, only
  `mcp__pipkin__*` allowed, strict owned MCP configuration, empty settings sources, disabled
  hooks/Chrome/slash commands, and no permission prompts. This is an offline-tested recipe,
  **not installed-CLI proof** of tool/plugin/memory isolation or MCP compatibility.
- Native-prepared servers alone mint `NativeLaunch`. Launch construction checks the durable
  binding digest (including detecting edits to detached metadata) and explicit session,
  generation, operation and run. Old assets cannot be reused for another run. Generic launch
  methods reject this policy instead of falling back to chat/Coding/native API inference.
- Private0700 directories contain0600 MCP/system files. Secrets go only in the MCP helper's
  environment configuration, never CLI argv or the parent's ambient environment; ambient
  native-channel overrides are rejected. System instructions use a separate system-role file;
  the current user prompt goes to stdin. This is not a history import/flattening shim.
- Linux `HelperImage` copies the actual running image from `/proc/self/exe` into an owned0700
  file, without modifying/hard-linking the source. The retained image, not a reusable PID path
  or mutable installation pathname, is the helper command. Capture once per engine lifetime
  on a background path; the image is bounded at2GiB. Files/image/private permissions are
  revalidated. RAII deletes only owned launch assets; copies can remain after a process crash.
- Initial limits:1MiB system snapshot, existing text-only prompt ceiling,16MiB stdout and
  at most30 minutes. Timeout is clamped to the remaining channel lifetime at construction;
  the future owner must enforce the absolute deadline at launch and throughout supervision.
  Thinking overrides, attachments and full context/payload parity are not implemented.
- `Assets` must outlive the CLI process. **Neither metadata, assets nor a command spec admit
  a model request.** The future owner still needs an approved runtime creator, durable prompt/
  launch intent, fresh/resume eligibility, atomic owner checks, process/driver cancellation,
  root terminal evidence and retirement. Dropping files alone cannot cancel/fence native work.

**Gate298:**16 added unit tests; full workspace (engine1462/app336/UI217), existing real debug
helper/native-tools integration, strict workspace/all-target Clippy, fmt/diff, frozen source
and unchanged live ledgers passed. Tests use owned fake CLI metadata fixtures, exercise scope/
identity/expiry/file refusal and private cleanup, and execute a copy of the running **unit-test
image** with a clean environment. That image test does not qualify copied installed-app MCP
startup. No installed CLI invocation/inference or native UI input occurred. Artifacts
`/home/jason/.pg.z25vF5`, snapshot
`6d23fe6ecf334dc8cd7ccf31ea4e8dad1ecb186da38a80589a004dbd18c23079`.
Gate296 targeted tests/strict Clippy and gate297 full workspace also passed
(`/home/jason/.pg.x6qTbO`, `/home/jason/.pg.YN0Slx`). Review after297 identified the child-alias
configuration mismatch;298 adds positive child inheritance and negative model-rebinding tests.

## Slice 7: durable native root journal (implemented, inactive)

- Engine schema28 adds `store/cli_agent_turns.rs` / `.sql`. One transaction commits a native
  request/run, canonical **user-role** entry, permanent root receipt and revision/outbox.
  Exact repeats return the receipt without another nonserializable owner lease. Changed
  request content, binding, generation or operation conflicts; concurrent admission has one
  owner. Native/API-only guards remain unchanged.
- An admitted owner must commit a one-use launch-intent transition naming its exact prepared
  bridge before spawning. Scope/owner, binding, live run and Stop/removal are checked. Lost
  launch acknowledgement cannot authorize another spawn. Native dispatch for journaled runs
  rejects merely-admitted state and a different bridge. Existing unjournaled protocol/driver
  fixtures remain substrate tests, not an approved runtime route.
- Fresh/resume is derived from receipts, not supplied by a caller. Unlaunched retirement is
  Rejected; new explicit work may still use the unused UUID (or resume a prior clean result).
  Any uncertain launch/root outcome permanently blocks this binding's new admission under
  the current conservative policy. Unknown receipts are never promoted or replayed. Foreign/
  unjournaled history cannot silently become a fresh native CLI context.
- Completion requires independent supervisor facts (input delivered, projection finished,
  root reaped, pipes drained, exit0, no cancellation/cleanup/protocol failure), matching CLI
  UUID, a cleanly closed channel, and actual joined native receipts for every bridge call.
  EOF, tool completion, protocol-only simulated results or final text alone are insufficient.
  At gate301 the driver still needed coordinated graceful close; slice8 adds its owner control.
  Root supervision remains absent: this journal does **not** manufacture supervisor facts or
  publish a live SessionRt.
- Duplicate completion is read-only. Contradictory evidence preserves the original receipt
  and permanently fences context/new admission, retiring any active successor and its bridge.
  Owners must also cancel affected in-memory/process work; SQLite cannot undo existing effects.
- Startup recovers roots before generic native recovery, without reconstructing leases or
  invoking inference. Removal intent retires active roots before payload deletion. Permanent
  nonsecret identity/fingerprint/outcome receipts remain, while native prompt/tool/history
  payloads are removed. Stop/retirement retains browsable history and retires the user entry.
- Initial admission is text-only; no queues/steers, clean-error resumption, post-Stop context
  repair, projection/usage finalization or attachment parity is claimed. Those remain explicit
  owner/context integration work, not an excuse to flatten history or switch model backends.

**Gate301:**16 new unit tests, full workspace (engine1478/app336/UI217), existing actual
helper/native-tools integration, strict workspace/all-target Clippy, fmt/diff, frozen source
and unchanged live ledgers passed. Five real process-exit cuts cover admission, launch intent,
actual native Write effect, committed tool receipt and root completion. Recovery preserves
known tools even when the root is Unknown and does not repeat the Write. **Root supervisor
facts in these tests are fixtures, not installed-CLI observations.** v27 backup/migration,
rollback,16-way admission, Stop, removal, contradiction and protocol-only receipt refusal are
covered. Engine schema28; protocol41/desktop schema10 unchanged. Artifacts
`/home/jason/.pg.w2d1gB`, snapshot
`38e59e0103499deb9540ec60017b5400ce0d8c7b7bab5af1452e9eb77d1d73db`.
Targeted299/300 also passed (`/home/jason/.pg.SfspyX`, `/home/jason/.pg.A1DzZL`).

## Slice 8: owner-controlled native channel closure (implemented, inactive)

- `cli_bridge_driver::serve_controlled` accepts an engine-owned, scope-bound close request;
  plain `serve` keeps its existing disconnect/cancellation behavior. A stale scope cannot
  consume the legitimate close handle. Finish is one-use; a lost acknowledgment cannot be
  retried as a fresh grant. Dropping the owner synchronously revokes further dispatch without
  storage I/O, before the driver next polls its receiver.
- Close requires initialized/quiescent MCP, no active worker, no queued work or buffered frame.
  Busy close fences rather than silently executing more queued effects or fabricating results.
  Stop, link loss and the captured absolute lifetime remain vetoes, including before the final
  owner acknowledgment. Existing bounded cooperative worker cleanup and durable retirement stay.
- Private authenticated control performs host quiesce → helper quiesced → durable bridge close
  → host closed. The helper stops its MCP input loop before acknowledging quiescence; partial
  authenticated-write cancellation fails closed. The handshake is bounded at10 seconds and
  cannot extend the channel lifetime. Native result receipts still precede their MCP replies.
- These control frames never appear on MCP stdout. Reserved control fields on MCP stdin are
  rejected, so a client/model request cannot forge the helper acknowledgment. Unknown/extended
  control shapes and wrong-direction final frames fail closed. This remains loopback IPC,
  not a sandbox against root/same-UID code.
- The owner receives a scoped `Closed` result only after driver cleanup/retirement. **Closed is
  not root-model success.** Root input delivery, terminal/projection evidence, process reaping,
  pipes, Stop state and final journal settlement remain independently required. No root launcher,
  approved runtime creator, live publication, full ToolHost service or UI capability is added.

**Gate304:**11 new tests (9 unit,2 actual debug-helper integrations); frozen full workspace
(engine1487/app336/UI217), strict all-target workspace Clippy, fmt/diff and identical source/live
ledgers passed. Real native Write plus helper exit0/no extra MCP output, forged-ack refusal,
quiescence, stale scope, pending/queued work, commit failure, Stop, timeout and owner loss are
covered. Root rows deliberately remain Running after clean MCP close; without root evidence
retirement makes them Unknown. No installed CLI invocation/inference or GUI input occurred.
Artifacts `/home/jason/.pg.F58Glr`, snapshot
`9a4af7e8ea902269ceaa92673787f2e256e586e3f0714ae0bc798ed0f32acdf4`.
Targeted302 and full303 also passed; review after303 added the synchronous owner-drop dispatch
flag regression. Schema28/protocol41/desktop schema10 remain unchanged.

## Slice 9: sealed launch inputs and strict root observations (implemented, inactive)

- `Store::cli_agent_launch_context()` requires the original owner lease and a live, admitted,
  unstopped root. Its nonserializable, redacted `LaunchContext` exposes immutable stored binding,
  scope, fresh/resume mode and prompt; it is a snapshot, **not execution authority**.
- The request and canonical user-role entry must match the permanent admission fingerprint,
  run/request join, model, prompt mode and unretired/nonpartial state. Payload byte bounds and
  partial/outcome/queue guards apply before loading or materializing entries. Changing all the
  mutable prompt representations consistently still cannot change the permanent admission.
  `begin_cli_agent_launch()` repeats these checks inside its one-use launch-intent transaction.
- `Prepared::launch_admitted_orchestration_spec()` accepts only that stored input snapshot and
  owned assets. The caller-selected prompt/mode/scope recipe is now module-internal. Fresh
  preflight, separate approval, durable launch CAS and supervised cancellation remain necessary;
  constructing a spec cannot spawn a process or authorize a replay.
- `orchestration_observer::Observer` binds the stored CLI UUID/scope and exact native catalog
  hash. Initialization must report precisely `mcp__pipkin__<native-name>` for every offered tool:
  omissions, duplicates, CLI built-ins and other MCP servers are rejected. It cannot opt into
  Coding's permissive CLI-owned-tools mode. Existing Coding/legacy decoder behavior is unchanged.
- SDK tool observations/results still undergo bounded framing/correlation checks, but are not
  emitted as native tool calls, results or duplicate cards. Only scoped display deltas/snapshots,
  initialization and terminal observations leave this wrapper. Their Debug output is redacted.
  Stale scopes cannot consume bytes. UUID/child-stream violations, missing terminal, trailing
  contradictions and sink failures remain errors. EOF/terminal/decoder finish are not root success.

**Gate308:**12 new unit tests (5 storage,6 observer,1 prepared-spec); frozen full workspace
(engine1499/app336/UI217), strict all-target workspace Clippy, fmt/diff and identical source/live
ledgers passed, including the existing real helper/native-tool integrations. Fixtures use owned
fake metadata executables only; no installed CLI invocation, inference, GUI input or user-profile
mutation. Artifacts `/home/jason/.pg.iOD6qm`, snapshot
`ba48b72e80d3c4becce5953fb0bc58b820def2735f5fe4217f6e05e6f56948af`.
Gate305 (`/home/jason/.pg.ttBhZ7`) exposed a BLOB-in-STRICT-TEXT oversized test fixture;
306 (`/home/jason/.pg.W1WbWe`) exposed its invalid-JSON entry-body fixture. Both fixtures were
corrected without weakening schema constraints.307 passed; subsequent review added pre-load
partial/materialization, model/fingerprint and outcome/queue guards, all re-gated in308.
Schema28/protocol41/desktop schema10 are unchanged.

This is **not** model-history/context parity or persisted/live root text projection. No runtime
creator, root launcher/supervisor or public capability has been enabled. Exact inventory shape
and CLI terminal-versus-helper-EOF ordering still need installed-version qualification with a
renewed bounded inference allowance; do not infer compatibility or relax closure from stdout.

## Slice 10: owned root process and committed response projection (implemented, inactive)

- `runtime/cli_agent_root.rs` now joins the previously separate pieces: stored launch input,
  native-prepared listener/assets, strict decoder, native tool driver, process supervision and
  durable root settlement. The internal entry consumes the original lease/listener, commits
  launch intent before spawning, rechecks preflight/assets afterward and never retries or uses
  an API adapter. Slice11 now requires an independent approved orchestration turn owner.
- It marks Running only after receiving actual subprocess stdout. Scoped display observations
  commit through `cli_agent_turns/projection.rs` and enter a bounded publication lane. Deltas use
  byte offsets and incremental chunks; complete snapshots replace those chunks atomically.
  The in-memory cursor advances only after commit and cannot be recovered into a replay grant.
- Canonical assistant display entries use `claude-cli-native` provenance, one entry per SDK
  display block, in observed arrival order among native tool ledger entries. This does not claim
  reconstruction of cross-channel SDK causal order or implement model-history inheritance.
  SDK tool records remain inert and do not generate duplicate native tool cards.
- Terminal observation requests slice8's private close while the root is still supervised.
  Completion requires actual exit0/input delivery/reaping/pipe drain, strict decoder finish,
  committed projection, clean driver completion and all joined native receipts. Trailing
  contradictions remain vetoes. A final unterminated line cannot retroactively close the bridge.
- Stop, publication loss, helper failure, timeout, changed executable, launch commit failure,
  nonzero exit and contradictory trailing identity cannot manufacture success. Signal cleanup
  revokes native work first; assets outlive process/driver cleanup. Normal failures durably
  retire after cancellation. Drop revokes memory/process work; restart owns durable recovery.
- Retirement keeps native tool evidence but retires this root's display observations and user
  instruction. Final text alone is not root completion. Optional reported usage remains optional
  in completion evidence; aggregate native usage publication is not implemented here. Persistent
  display-loss diagnostics/notice do not turn independently verified execution into a fake failure.

**Gate312:**13 new tests (8 root coordinator,5 projection); frozen workspace
(engine1512/app336/UI217), strict all-target workspace Clippy, fmt/diff and identical source/live
ledgers passed. An owned fake CLI process plus authenticated SDK/MCP peer simulation performs a
**real native Write**, persists its model response and reaches Completed with actual supervisor
facts. Negative paths retain Unknown/Rejected without replay; byte offsets, snapshot repair,
rollback, recovery, stale scope, limits and display-loss/unknown-usage distinctions are covered.
The copied unit-test helper image is not executed by these new root fixtures. Existing real debug
helper/native-tool integrations also passed, separately. This is **not installed Claude CLI or
actual SDK lifecycle qualification**. No installed CLI invocation or inference occurred.
Artifacts `/home/jason/.pg.sXDCqw`, snapshot
`04c7727324a4e80fe2fee6416c70dbbcc3b1c40181e1d2aab1c689c240648c84`.
Gate309 (`/home/jason/.pg.xKsNb8`) failed because the fake launcher mistook the leading
`--mcp-config` of an inference recipe for a metadata call; its unbounded test wait was cancelled.
The fixture now recognizes `--print` anywhere in argv and uses a bounded wait.310 and311 passed;
312 additionally verifies honest display-loss/optional-usage evidence.

At gate312, approved runtime ownership was still absent; slice11 supplies its private registry.
SessionRt publication/settlement, full ToolHost/context/workflows and desktop activation remain.
The bounded publication lane is not a live UI subscription. SDK terminal/helper-EOF ordering and exact inventory remain subject
to future bounded live qualification. Schema28/protocol41/desktop schema10 remain unchanged;
`claudeCliAgent` still decodes Unsupported publicly.

## Slice 11: independent revocable orchestration ownership (implemented, private)

`runtime/cli_agent_owner.rs` provides a separate registry, not a Coding grant upgrade:

- Host opt-in `enable_local_claude_cli_orchestration` defaults false and additionally requires
  Linux and the master CLI flag. Ordinary desktop Coding opt-in does not enable it. There is
  still no public setup/creation/model route, and `claudeCliAgent` still decodes Unsupported.
- At most64 connected owners and128 grants. An explicit native-policy approval may create a
  new immutable binding with a newly minted UUID and install its first grant. Exact duplicate
  creation returns metadata without minting authority. Saved identities/reconnects are not
  grants. Explicit return requires the same root identity and eligible durable history;
  Unknown, fenced, foreign, removed or active work cannot become a new start.
- A noncloneable turn reserves the session before admission, gives it an opaque owner key,
  and is required alongside the original root lease. Foreign ownership cannot consume that
  lease. Fresh per-turn native preflight must reproduce the immutable binding; Coding approval
  is not reusable. Admission still repeats durable checks transactionally.
- Connection lifetime tokens propagate cancellation independently of registry/SQLite locking.
  Shutdown signals before locking. Stop/revoke/disconnect cancel, but retain the active turn
  reservation until cleanup drops it. Removal revokes grants and waits for that reservation.
  Dropping a turn revokes memory authority, never performs SQLite I/O or fabricates settlement.
- The coordinator includes fresh metadata in its absolute deadline, cancels and awaits the
  metadata supervisor rather than dropping its cleanup, then follows the existing launch CAS,
  native bridge, root projection and settlement path. Caller cancellation/shutdown remain
  independent vetoes. After possible execution, Unknown is retained and never replayed.

**Gate320:** frozen offline workspace (engine1521/app337/UI217), strict all-target Clippy,
fmt/diff and source/live-ledger equality passed.8 new ownership/deadline tests include actual
engine connection lifecycle and fake-root/native Write cancellation;1 additional test covers
verbatim fixture publication. The metadata-deadline case observes the owned probe pid reaped
without an inference launch. These tests do not qualify installed Claude/actual SDK lifecycle.
Artifacts `/home/jason/.pg.W0dByz`, snapshot
`67028b434387d07601505c3eddaf412046bebb29542a79166046e0ba346d4497`.
317 found a compile-time pinned-future lifetime issue;318 hit an executable-busy fixture;319
caught the isolated script writer's initial size limit. All are retained in the scorecard.
Affected fixtures now stream through an isolated writer process, with normal test parallelism
and no production retries. Schema28/protocol41/desktop schema10 are unchanged.

This registry alone is **not** SessionRt task tracking/live publication, a complete native
ToolHost, CLI-only child orchestration, queue/context/goal parity or an enabled desktop backend.
Slice12 adds private task attachment; the remaining integration and real SDK terminal/helper-EOF
ordering qualification still apply.

## Slice 12: private SessionRt task attachment (partial task86, not activation)

`runtime/cli_agent_runtime.rs` attaches an **already admitted** original root lease/owner to
an engine-owned task. The result receiver owns no execution lifetime: dropping it does not
cancel or abandon work. Task registration precedes scheduling; resource/turn drop precedes
tracker completion even for an unpolled task. Foreign owner registries are rejected before
session/store mutation.

The actor attaches SessionRt active state under its admission lock, retains generation capacity,
uses shared process/mutation resources, drains the bounded scoped projection lane, awaits root
and native cleanup/retirement, then clears active state before broadcasting committed settlement.
Cancellation/deadline can interrupt the initial admission-lock wait. Failed capacity/prelaunch
handoffs cancel and retire rather than silently orphaning an admitted request. No durable outcome
is invented if final storage reconciliation fails.

Stop dispatch recognizes a live independent root reservation, cancels before locks and persists
Stop intent for the attached run. There is no native API fallback, recovery, goal or queue driver
substitution. Actual connection drop and engine shutdown cancel and track the root; removal waits
for its lifetime and retains permanent receipts. This does not make Unsupported historical rows
runnable and introduces no public model/setup route.

**Gate323:**8 new real-engine lifetime tests use owned fake CLI/SDK/MCP simulations plus actual
native Write. Dropped result waiter, Stop/active state/Unknown preservation, capacity refusal,
foreign registry, shutdown, removal, actual connection drop and cancelled lock-waiting handoff
pass. Frozen offline workspace (engine1529/app337/UI217), strict all-target Clippy, fmt/diff and
source/live-ledger equality passed. Artifacts `/home/jason/.pg.V84zEV`, snapshot
`4e87e9449f22a08ee074916bded5ec81d230f7b81836b75e7c5c955f27162b20`.
321 targeted checks passed;322 caught a tracker-token import error, corrected before323.
No installed CLI inference or native-window input occurred.

At gate323, owned creator/admission and ordered live publication remained pending. Slice13
adds the latter; this still does not establish full workflow parity or desktop activation.
Schema28/protocol41/desktop schema10 remain unchanged. Native ToolHost/CLI-only children,
queues/goals/context and live qualification also remain.

## Slice 13: revision-ordered committed text and tool publication (private task86 progress)

`cli_bridge_driver/publication.rs` captures a scoped runtime sink and the same publication gate
used by `Shared::commit`. It validates the exact session/generation/op/run and current lifetime
before entering a write; committed delivery happens before releasing the gate. No await occurs
inside it. A failed transaction emits nothing; failure/revocation after commit does not retry it.
This sink is observation, not a replacement for original owner/lease/intent authority.

Root projection applies/finalizes under this gate. Native tool admission and settlement return
their actual committed entries/revisions from the same transaction, then publish before another
producer can overtake them. Pending/completed duplicate calls and already committed results
emit no new card. Ordinary native writers use the same gate. SDK tool records remain filtered;
there is still only one native executor and one canonical native call/result pair.

The private actor publishes its already committed input/run before launching producers, then
uses incremental existing session events for text and native tool entries. Successful final
RunChanged/RunSettled publication is ordered; error retirement requests a single reconciliation.
No new renderer, per-token full-history refresh or unbounded event queue is introduced. Existing
bounded client delivery still coalesces slow consumers. This orders committed observations, not
SDK causal history or model context.

**Gate327:**5 new tests (3 publication guard/order/failure,1 store receipt/replay,1 engine wire).
The fake SDK emits a first delta before an authenticated peer may request an actual native Write.
The client sees that delta, an interleaved ordinary native commit, native call/result, remaining
text and settlement in revision order. Its event-built entries equal stored history, with no
success-path resync or duplicate SDK tool card. Frozen offline workspace (engine1534/app337/UI217),
strict all-target Clippy, fmt/diff and source/live-ledger equality passed. Artifacts
`/home/jason/.pg.XXBvXW`, snapshot
`3953d483db481546b516d3df59ce98bfa8f27fb9a90a970436a9093764989977`.
324 caught a tuple-return compile error;325 rejected a synthetic stream that omitted block0;
326 targeted checks passed after fixing that fixture, without weakening the decoder.

At gate327, task86 remained active because the entry consumed an already admitted original
lease; slice14 closes that gap. Native ToolHost/CLI-only children, volatile tool progress,
context/queues/goals and public setup/model/desktop qualification remain. Public backend is still
Unsupported. No installed CLI inference or native UI input occurred; live allowance is0.

## Slice 14: owned admission and preparation (private task86 complete)

`cli_agent_runtime/admission.rs::Start` carries an original independently approved Turn and
bounded immutable request data, not an admitted lease. Synchronous submit registers an engine
owned task before returning a receiver. Admission, listener preparation, private launch assets,
fresh launch checks, native cleanup and durable retirement stay inside that task. Dropping the
receiver cannot strand admission; the outer tracker remains through the entire handoff and
cleanup. The old already-admitted spawn entry is now test-only.

- Exact fingerprint lookup returns only the durable receipt, even without capacity or valid
  launch assets. It grants no lease or replay. Conflicting input fails; Unknown remains fenced.
- New input reserves shared generation capacity before admission. Under the session lock and
  shared publication gate, it commits admission, attaches the original active lifetime, then
  publishes input/run before releasing the gate. Other writers cannot overtake that input.
  Stop during preparation therefore targets the actual run and durably records its intent.
- Listener and asset writers are awaited, not abandoned on a deadline/cancellation branch.
  Bind now awaits its SQLite preparation writer before returning; a detached writer cannot
  race owned retirement. After preparation, fresh ownership/deadline/asset checks still veto
  launch. Ordinary preparation failures retire the original lease and release its lifetime;
  failed settlement fails closed. Crash recovery remains separate and never resumes inference.
- The runtime inherits the same active lifetime and capacity slot rather than reacquiring or
  publishing input twice. Shared finalization clears active state before settlement delivery.

**Gate331:**7 new real-engine tests cover lost reply plus exact duplicate lookup under exhausted
capacity/cancellation, invalid assets/catalog retirement, cancelled lock wait/capacity refusal,
changed duplicate rejection, actual Write then Stop/Unknown lookup, input publication ordered
before an interleaved native writer, Stop while preparing, and removal during preparation.
Tests simulate CLI/SDK/MCP; effects and engine/SQLite lifecycle are real, not installed-CLI proof.
Frozen offline workspace (engine1541/app337/UI217), strict all-target Clippy, fmt/diff and
source/live-ledger equality passed. Artifacts `/home/jason/.pg.AtFFcd`, snapshot
`470c8c7e4e9da7d7c4647b468d0d0d86bcf8900f4a496c9abf6134e9da6b4f77`.
328 caught an unused staged re-export in strict Clippy;329 passed the earlier frozen builder
slice;330 passed targeted early-publication/Stop tests. Failures remain retained. Schema28,
protocol41 and desktop schema10 are unchanged. No live inference, API fallback or UI input.

## Slice 15: scoped native ToolHost and plans (task87 partial)

`cli_agent_host.rs` installs an engine-owned per-call service host, never the API-backed
`subagents::Host` or a caller-supplied host/factory. `Turn::services` delegates original in-memory
owner authority, not a new Turn or launch grant. Its scope, grant identity, turn reservation and
cancellation must still match; saved receipts/bindings cannot reconstruct this authority.

The authenticated driver constructs each host only after committing native intent. Host service
transactions join the exact bridge/call/name/run pending intent, validate the worker cancellation
and absolute deadline, and use publication → registry → storage lock order. Parent ownership
alone is insufficient after per-call cancellation. Checkpoints remain scoped to that actual call.

Native `plan_update`/`plan_read` use the existing plan store and event contract. Plan changes and
revision delivery share the commit gate; MCP replays return the actual tool receipt without a
second plan mutation/event. The update call may read its committed view for result formatting;
a read call cannot mutate. Other services remain closed until explicitly implemented.

**Gate335:**4 new tests cover service authority revocation/drop/foreign leases, native-versus-
protocol-only intent joins, real plan update/read/replay with existing revision-ordered events,
and cancellation before/after plan effects. Root Stop and independent call cancellation preserve
committed plans, refuse late writes and leave unconfirmed roots Unknown. Caller API hosts are
replaced, and native provider requests remain zero. CLI/SDK/MCP are simulated, not live-qualified.
Frozen offline workspace (engine1545/app337/UI217), strict all-target Clippy, fmt/diff and
source/live-ledger equality passed. Artifacts `/home/jason/.pg.MWuwT6`, snapshot
`9c88187392da31b882e3295f6142786efa7467efce357dd28d0d5c6994211df3`.
332 caught a missing error propagation;333 exposed the plan updater's required read-back service
bundle.334 targeted tests passed after correcting it. Failures remain retained. Full ToolHost,
CLI-only children and workflow parity remain incomplete; public orchestration stays disabled.

## Slice 16: scoped native questions and exact response grants (task87 partial)

`cli_agent_host/questions.rs` routes authenticated `ask_user_question` calls through the existing
session question host, UI view and AnswerQuestion/CancelQuestion RPCs. It does not borrow the API
question runner's restart/re-ask behavior. The engine-owned host validates the original scoped
owner, pending native intent, worker cancellation and deadline before/after waiting.

A bounded volatile registry holds an original host grant for the **exact generated question ID**.
The question host installs that grant before publishing the question; refusal grants nothing.
Registration callbacks take only the small registration-map locks, never registry/storage or
publication locks. Native UI publication remains the existing lifetime-guarded volatile path.
Dropping the asking future withdraws its question and removes only its own grant. Stop, disconnect,
removal, shutdown and scope/deadline cancellation cannot turn this into a replayable API question.

Only a matching live granted question permits a response RPC for the otherwise Unsupported CLI
backend. Copying the visible source/call ID, guessing another question ID, or answering after the
turn expires grants nothing. The generic native execution guard is unchanged for all other
mutations. Answer acceptance is not model/tool completion: actual tool receipts still precede MCP
reply, and interruption preserves Unknown rather than inventing a person's refusal or re-asking.

**Gate339:**6 new tests (4 real-engine tests,2 question-host unit tests) cover answer and explicit
decline through the existing RPC, exact ID versus forged-source questions, duplicate MCP lookup,
late replies, Stop/removal/disconnect/shutdown withdrawal, registration-before-publication and
refused registration. Native provider request count remains zero. Simulated CLI/MCP is not installed
CLI or desktop UI qualification. Frozen offline workspace (engine1551/app337/UI217), strict all-target
Clippy, fmt/diff and source/live-ledger equality passed. Artifacts `/home/jason/.pg.ycvYCj`, snapshot
`ded3fca48bedc2e7b32966525d685ceefa98065d1de39a9539ecfa9bd4773064`.
336 caught a test RPC field typo;337 targeted checks passed.338 was interrupted when the engine
stopped, during compilation, not a completed test gate (`/home/jason/.pg.Trxffq`).339 verified source
was unchanged from338 before rerunning. No running-app change/restart was performed by this work.
Native orchestration remains publicly disabled; task87 still needs CLI-only children and the
remaining services. Schema28/protocol41/desktop schema10 and live allowance0 are unchanged.

## Slice 17: frozen child policy and atomic metadata (task87 still partial)

`execution/claude_cli/orchestration_child.rs` captures a CLI-only child recipe from supplied
parent scope/binding, settings, tool declarations and instruction/skill-listing snapshot. It
never consults an API/provider catalog or default. Explicit models must be `claude-cli/haiku`,
`claude-cli/sonnet` or `claude-cli/opus`; an API-selected profile fails closed even if another
field would override it. Thinking remains unsupported. Worker "all" becomes an explicit frozen
catalog, and delegation/question tools are excluded. Later settings, descriptions or resource
changes cannot change that recipe. This is **data, not owner or launch authority**; wiring the
runtime's shared root resource lifetime into capture is still outstanding.

Inherited context stays bounded, role-tagged data, excluding retired/partial/system entries.
The current single-user CLI launch conversion explicitly refuses `inherit`, even for an empty
excerpt: no silent downgrade to fresh or concatenation of the parent's history into the task.
Task text is bounded64KiB, profile instructions32KiB, rendered system1MiB, stored snapshot2MiB.
The snapshot fingerprint includes the exact parent call, scope, binding, policy, catalog,
context, caps and delivery; separate calls cannot collide merely by selecting identical tasks.

`store/cli_agent_children.rs` adds a transaction-only metadata primitive. It joins the exact
pending authenticated **native** subagent intent (a protocol-only intent is insufficient),
checks the parent recipe, and atomically creates child identity/UUID ownership, task, frozen
policy and root revision. It freezes the selected CLI alias and disables further delegation.
Duplicate lookup precedes caps, emits no new revision and grants no execution authority.
Detached binding receipts cannot be adopted as tasks. A late task-insert failure rolls back
identity, permanent-UUID reservation and revision together.

**Schema29** adds `cli_agent_child_policies`, with bounded immutable snapshots whose digest is
bound into the child's immutable creation key. Large system/context snapshots are deliberately
outside `subagent_tasks.launch`, so ordinary task-list views do not reload them. The reader
bounds before loading and verifies the digest. Policy deletion follows binding removal; global
UUID ownership stays permanent. Migration from28 preserves existing bindings/root receipts and
backs up the old database; only disposable test databases were upgraded by this work.

**Gate343 passed:**19 new tests (8 policy,11 storage), frozen offline workspace
engine1570/app337/UI217, strict all-target workspace Clippy, fmt/diff, source and live-ledger
equality. Artifacts `/home/jason/.pg.wocULQ`, snapshot
`5bc6d1f425b5585ce69969a70decd9cf165bff4cdcf437a9ace4e635a09aea6c`.
340 passed the initial7 policy tests;341 caught a fixture Result-alias compile error
(`/home/jason/.pg.h4fiP9`);342 passed8 policy/184 storage tests and engine Clippy before the final
bounds/detached-binding cases. These are policy/store tests with simulated journal authority,
not child inference, installed CLI or desktop qualification.

**Not yet implemented:** original-parent delegated owner minting, tracked creation/admission and
failure cleanup, child launch/continuation, inline/background completion/wake, and shared runtime
snapshot capture. `cli_agent_host::subagent` still refuses execution; saved recipes/metadata do
not become capabilities. No API inference fallback, live inference, running-app mutation or UI
input was performed. Public backend remains Unsupported; protocol41/desktop schema10 and live
allowance0 are unchanged. Task87 and the whole objective remain incomplete.

## Slice 18: working fresh inline child workflow (task87 partial)

The native `subagent` host now runs `wait=true`, fresh-context children through the **same private
CLI root runtime**, not `subagents::Host` or an API adapter. `cli_agent_host/children.rs` owns an
engine-tracked actor before returning a waiter. It captures the child policy, performs fresh
CLI metadata checks, creates metadata under publication → registry → storage ordering, and
hands an original one-use child Turn directly into owned root admission. Dropping the waiter
does not drop that actor. No receiver or saved record reconstructs launch authority.

`cli_agent_owner/children.rs` mints that Turn only from the current original parent, matching
native pending call, captured CLI identity, worker cancellation and absolute deadline. Child
metadata preserves a fresh UUID, inherited account/executable/project/policy and selected CLI
alias. General `begin_turn`/return still cannot adopt children. A child grant is removed on Drop;
its initial input must match the frozen task, and it cannot admit a later continuation. The
owned admission and root coordinator check its exact tool/system policy; assets retain a system
digest and continue ordinary file-stamp validation. Root-provided system text is preserved.

The host carries the root session's instruction/skill resource-cache snapshot and existing
captured helper image into the child; child prompt rendering filters tools and adds its profile.
No second native mutation/process pool is created. Parent Stop/connection cancellation reaches
the child Turn, while worker/caller cancellation and deadline also cancel and await the child
root receiver. Actual successful root settlement supplies the returned text/task result; failure
records an explicit unconfirmed-result warning without discarding known native effects. Task
creation/result views publish under the shared gate. No goal or API scheduler is borrowed.

**Measured through347:**3 new real-engine workflow tests prove parent native subagent request →
child CLI runtime → real native Write → child result → parent completion; duplicate MCP request
returns the same receipt with exactly2 simulated model-process launches, not another child.
Parent Stop preserves the Write, records child Unknown and completes tracked cleanup. An
API-selected worker profile returns an error with no child and no provider requests. All3
observe zero scripted API provider requests. CLI/SDK peers are simulations, not installed CLI.

Frozen workspace347 passed engine1573/app337/UI217, strict all-target workspace Clippy,
fmt/diff and source/live-ledger equality. Artifacts `/home/jason/.pg.cTikc9`, snapshot
`b5ab826af62afa6c664381170964ead2716a28fdf893f5f45524db03302474a9`.
344 retained a method-visibility compile error;345 passed the first2 workflow tests and41 root
tests but caught a private-interface Clippy warning.346 retained a test fixture's missing settings
directory;347 corrected it and passed. No installed inference, app mutation/restart, packaging,
commit or push. Schema29/protocol41/desktop schema10 and live allowance0 unchanged.

This supersedes slice17's refusal of **all** children: inline fresh children now execute privately.
Background/wake, explicit continuation, inherited-context transport and full limits/lifecycle
refinement remain incomplete. Inline children currently require another available generation
slot; no capacity limit is bypassed. Initial child task display remains pending until its result;
running-progress presentation is part of the remaining progress work. Public activation and
live qualification still wait for the broader capability checklist, not just this milestone.

## Slice 19: MCP, web, directory reads and volatile progress (task87 partial)

The native host now connects seven MCP routes, three web routes and child-directory
status/result/answer/wait to the existing domain services. Each route checks its own tool name,
actual call where supplied, original turn and pending authenticated intent before polling its
operation. Worker/caller cancellation, engine shutdown and deadline terminate the scoped wait;
liveness is checked again before returning. There is no API inference host/model resolver.

MCP uses the engine's existing server manager. A data-only `mcp_config_at(cwd)` seam reads the
bound project's settings without relaxing public `mcp_cwd`/native-execution guards. Unlike the
old void refresh path, the host cannot silently keep another project's settings after an API
execution-guard refusal. Each call can carry its own form presenter; it reuses the existing form
parser but presents through that call's exact native question registration, never generic
session-name permission. A scoped elicitation-only ToolHost explicitly refuses inference.
MCP caller registration now uses pointer-matching RAII removal and cancellation, including on
future Drop, and decrements in-flight accounting only for the same live client. Ambiguous
concurrent elicitation involving scoped callers is declined rather than lending a grant to the
most recent caller. This does **not** qualify full concurrent MCP configuration/elicitation parity;
the shared manager's dynamic configuration semantics are retained, not replaced with another
executor or per-root server pool.

Web search splits the existing network operation from retention; fetch reuses the existing
network boundary. Native retention happens in the original-authority transaction under the
publication gate, with worker/caller cancellation checked at retention. Source metadata and a
real bumped revision publish before the tool receipt. `web_read` reads only this session's
retained source. Search-provider fallback remains the configured **search** service behavior,
not a model inference fallback. No network, source or JSON parsing moves into GPUI render.

The native host factory returns a bound service/progress pair. Progress is a UTF-8-safe32KiB
volatile view (revision0), keyed by the actual native call; the same publication → authority →
storage check rejects expired/cancelled/completed work. Caller-provided progress is replaced.
Progress does not become a transcript entry, mutation receipt or success evidence.

**Measured through350:**3 real-engine workflows use simulated CLI/SDK peers, the owned compiled
MCP fixture and a loopback HTTP fixture. They exercise MCP discovery/describe/counter mutation,
resources/read/prompts/render, search/fetch/read retention, empty/foreign directory lookups,
MCP form answer and Stop, and Stop after a remote effect before its response. Receipt replay
keeps the counter at1 and HTTP request count at2; Stop retains the counter and root Unknown.
Forged call ids, cross-service use, generic MCP RPC execution and expired hosts are refused.
Bounded progress is delivered once; an expired callback is silent. Native source revisions
remain ordered. All observe zero scripted model-provider requests. These are not installed CLI,
real external MCP provider or desktop-input qualification.

Frozen350 passed workspace engine1576/app337/UI217, strict all-target workspace Clippy,
fmt/diff and source/live-ledger equality. Artifacts `/home/jason/.pg.qnVGJw`, snapshot
`7219ea1bc99a8a9121ce62185590ee19c652a68564417f9a7c36a5257cfaa004`.
348 retained compile errors in the elicitation adapter;349 passed the first3 workflows and
strict engine Clippy;350 additionally covered owned progress/forged/expired calls. Schema29,
protocol41/desktop schema10, public-disabled state and live allowance0 remain unchanged.

Next: jobs/goals, background children/wake/continuation, then the context/queues/steering and
limits/payload/post-Stop work in77 and public setup/qualification in78. Child running-task
presentation remains separate from the now-wired native **tool** progress. No dashboards.

## Slice 20: scoped jobs and goal reporting (private, frozen353)

The CLI host reuses the existing process supervisor for job start/status/log/wait/cancel.
Startup is engine-tracked before its waiter can disappear. Job intent commits under original
pending native-call authority; spawn rechecks that authority, and actual process receipts are
retained even when scope is subsequently revoked. Failure to retain a spawn receipt kills/reaps
the process group and reports uncertainty. Active-job caps are rechecked transactionally.
Cleanup/recovery requires the immutable CLI binding plus exact native bridge/tool/job/run join;
it never relaunches a job or infers through an API host. Delivery is notification-only.
`wake=true` is explicitly refused until the CLI continuation path exists.

Goal get/update/completion proposal use the existing evidence domain. A scoped commit compares
the entire captured GoalView inside the original-authority transaction, preventing stale
validation from overwriting newer goal state. Actual GoalChanged revisions publish under the
shared gate. Stop pauses an active goal only when its last request matches the stopped actual
CLI run. These tests seed private goals: public goal creation/start/resume/scheduling are not
implemented. A required audit refuses explicitly before any API auditor can be launched.

Four real-engine tests use simulated CLI/SDK peers, real owned local processes and file-hash
evidence. They cover job start/log/status/wait/cancel, replay, wake/foreign/forged-call refusal,
dropped startup waiter, Stop before spawn, cleanup-only recovery and ordered job revisions;
goal get/update/proposal replay, observation-only rejection, actual file evidence, stale/forged
host refusal, matched Stop pausing and required-audit refusal. Zero model-provider requests.
This is not installed CLI, real crash or desktop qualification.

351 passed the first2 job tests;352 passed49 root tests and strict engine Clippy. Frozen353
passed workspace engine1580/app337/UI217, strict all-target workspace Clippy, fmt/diff and
source/live-ledger equality. Artifacts `/home/jason/.pg.E47DhW`, source snapshot
`c3da7195d4b644c5b48674b42d7826c7d784626c08062557ab1c930c05a561de`.
No schema/protocol changes, installed inference, packaging/install, running-app mutation or
repository commit/push. Public orchestration remains disabled and live allowance remains0.

## Slice 21: background notification children and task lifecycle (private, frozen360)

Fresh `wait=false, wake=false` children now use the same original-parent policy/creation and
independently owned CLI root runtime as inline children. The startup actor is tracked before
returning its waiter; submission takes engine ownership before the tool reports **starting**,
not an invented process-spawn confirmation. The original connected approval owns the child.
Normal parent completion does not cancel it; approval revocation, connection loss, child Stop,
shutdown and deadline still cancel/clean it up. Reapproval revokes the old approval too.

Parent cancellation before a successful durable native subagent-tool receipt cancels the
unconfirmed delegation. A recorded successful handoff only determines whether existing owned
work is orphaned; it never reconstructs launch authority. Metadata, SDK output and receipt
lookup cannot launch a second child. MCP replay returns the original tool receipt. Background
wake remains explicitly refused; there is no API subagent/wake/recovery runner.

The task becomes running atomically with actual owned child admission, and its parent revision
publishes under the shared publication gate. Completion requires the actual unfenced Completed
CLI root receipt; the result comes from retained nonpartial assistant text in that run. Failed
results retain effects/uncertainty and expose the no-replay warning through the existing note
field, not as a successful answer. Notification-only completion uses the existing task view.
Startup reconciles orphaned metadata or lost task settlement against joined native journals and
actual root receipts, before generic recovery. It neither adopts unjoined metadata nor infers.

**Measured:**3 additional real-engine workflows (background lifecycle across completion/child
Stop/revocation/disconnect/registry shutdown; wake refusal; Stop before native tool receipt),
plus a storage recovery test. Existing3 inline workflows remain covered. The background parent
finishes before the child peer connects or writes. Exactly2 simulated model-process launches,
actual native Write preservation, replay, task phases/revisions, successful/failed result views,
bounded wait, foreign reads, Unknown retention and metadata-only recovery are checked. Tests
observe zero scripted model-provider requests. Cancellation may replace queued presentation
frames with ResyncRequired; assertions check the authoritative resnapshot instead of requiring
all superseded frames. These are owned local fixtures, not installed CLI or real crash tests.

Frozen360 passed workspace engine1584/app337/UI217, strict all-target workspace Clippy,
fmt/diff and source/live-ledger equality. Artifacts `/home/jason/.pg.omyc8L`, source snapshot
`5cd4c5ffad1dccc5509611a426741142c025fead444885235dac7cb14a740c69`.
Failures are retained:354 selected zero child tests and failed Clippy;355/357 exposed incorrect
queued-view expectations;356 caught an existing adapter-test counter sampled after event
publication (moved before publication,5 focused repeats passed in357);358 exposed the existing
failed-result view contract.359 passed6 child/12 storage tests, then Clippy caught an unused
import.360 passed after correction. No test serialization or production retry was introduced.

Remaining: continuation/wake, goal scheduling/auditors, inherited context transport,
context/queues/steering, full concurrency/post-Stop/payload/usage parity and public activation /
installed qualification. Children still need an available generation slot. Connection-bound
background operation is not disconnected/restart continuation. Public remains disabled;
schema29/protocol41/desktop schema10 and live allowance0 are unchanged. No dashboard, installed
inference, release/install, running-app mutation or repository commit/push work was performed.

## Remaining implementation order

1. **Task76 — private bridge/driver foundation complete.** Independent approval/ownership,
   immutable bindings, root/bridge journals, authenticated native effects, owned launch and
   cleanup, SessionRt lifecycle and ordered publication are joined. This is not public setup,
   full ToolHost coverage or installed-CLI qualification; the backend remains Unsupported.
2. **Task77 — orchestration/context.** Implement the full native ToolHost with explicit CLI-only
   child/profile/model inheritance. Preserve history roles, instruction/skill snapshot ownership
   and context/compaction boundaries. Connect plans/jobs/goals/questions, volatile tool progress,
   queues/steering, background completion and Stop/restart. Reconcile byte/concurrency/per-call
   limits and post-Stop behavior without fallback or uncertain replay. Unknown work remains
   uncertain; native rows never authorize recovery inference.
3. **Task78 — approved setup, public model route and qualification.** Expose the route through
   existing conversations/model selection only when its services work. Preserve old identities
   and drafts; make transitions explicit. Verify actual CLI tools, plans/jobs/children, questions,
   queues/steering and cancellation/restart through the existing desktop/engine with renewed
   bounded live authorization and user-owned rebuild/install. Dashboard implementation remains
   excluded; the user will build it using the completed integration.

## Verification boundary

Earlier CLI Coding qualification and orchestration are separate paths. Three initial native-MCP
probes wrote their files but ended Unknown; preserve them without replay/resume. After the
SIGINT fix, a new explicitly authorized two-prompt pair passed fresh and same-UUID resumed
native Write with CLI2.1.301/Sonnet. Both roots Completed and both bridges closed with durable
native receipts and exit0/process-drain/projection evidence. No API fallback occurred.
All current inference allowance is spent; earlier unused slots are superseded, not additive.
More real prompts require a new explicitly bounded allowance. Native-window input requires
the owned-window guard; release builds/install remain user-owned. Wider feature and desktop
qualification remain; the public orchestration route stays disabled.

## Slice22: child/job completion wakes (resumed 2026-10-18)

Original connected root approvals now mint one-use continuation tickets for authenticated
`subagent` and `job_start` calls. A settled completion waits for the parent to become idle,
then resumes its existing CLI UUID with a canonical FollowUp. Admission commits the input,
run and delivery marker together. Duplicate receipts are lookup-only; changed completion
evidence, failed handoff, stopped/uncertain parent history and revoked approvals cannot
admit a wake. Restart suppresses saved pending wakes instead of reconstructing permission.

- Fixed idle Stop routing: connected roots retain cancellation authority after their foreground
  Turn retires. Stop invalidates the pending continuation epoch even without an active run.
- Continuations wait for engine capacity before committing input. Stop, removal, disconnect,
  shutdown and deadlines cancel the wait. Fresh child admission still requires available
  capacity; this does not deliver general queue or concurrency parity.
- Generated completion text and its digest are captured under one publication gate, using
  the original system/tool/resource snapshot. Admission revalidates after waiting/preflight.
- The tracked job-start actor returns its actual startup receipt before awaiting completion,
  owns the continuation through cleanup, and never routes CLI jobs to the API follow-up runner.
- Fake CLI fixtures recognize `--resume`, record UUID/mode/turn and use separate handshake
  files per turn. They cannot accidentally reuse an earlier acknowledgement or close file.

Focused evidence is retained under `dist/cli-orchestration-resume/`: child-tests-3 (six tests,
including seven wake/cancellation/capacity/reapproval scenarios), job-tests (three tests,
including three wake/Stop/revocation scenarios), wake-storage-tests-3 (29 root-storage tests,
including atomic wake rollback/replay, cross-mode compatibility and corrupted-evidence checks).
The integrated gate passed on frozen production source: engine1588 (2 ignored), app337
(1 ignored), UI217, remaining workspace suites, strict workspace Clippy, fmt and diff checks.
Manifest `gate-source-before.json` equals `gate-source-after.json`, SHA256
`c000b16f9d7c5ad7328bbfa7ecb316af86497e5e2ecac1d9e540a1ae549070b2`.
Earlier failed checks are retained.
No installed CLI inference, release build/install, app restart, commit or push was performed.

Still remaining: explicit child continuation, CLI-only goal scheduling and auditors, inherited
context transport, compaction/queues/steering, broader payload/thinking/attachment/usage and
MCP concurrency parity, public desktop routing, and installed/desktop qualification. The last
step still needs renewed bounded inference authorization; no prompts were added to the budget.

## Slice23: installed preflight and tool-channel closure (2026-10-18)

CLI2.1.301 omits the documented `--system-prompt-file` option from ordinary help. Native
preflight now checks its public companion flag and exercises the actual file option during
the existing metadata-only auth-status invocation. Unknown-option rejection still blocks
admission. A real metadata check accepted the option and rejected a nonexistent one; no
authentication payload was retained. Fifteen preflight tests passed. Executable test fixtures
now use isolated script publication after retained ETXTBSY failures, including the app's fake
editor fixture. The production editor launcher is unchanged.

All three newly authorized Sonnet native-MCP probes wrote `native-first` with a durable native
tool receipt but failed root settlement on tool-channel loss. Each root remains Unknown;
none was replayed/resumed. The last two reports identify `DriverError::Channel`, followed by
supervisor cancellation, process reaping and pipe drain. The cause of installed channel loss
is still unidentified. All current inference allowance is spent; old unused slots are
superseded, not additive. The separate resume ledger remains unauthorized and unspent.
See `fixtures/claude-cli-probe/native-orchestration-probe.md` for ledgers, logs and owned data.

Offline actual-helper tests reproduced the unsupported ordering of MCP stdin EOF before the
model terminal. The helper now reports authenticated InputClosed while preserving its pending
wire read. The host requires initialized/quiescent protocol, no queued/running work and settled
receipts, acknowledges channel closure, then waits for the original owner's model terminal.
Dedicated helper SIGTERM handling follows the same handshake. Actual-helper tests cover EOF
and SIGTERM with/without the root terminal; partial input, spoofed controls and unsettled work
still fail. Process exit, drained pipes, projection and cancellation checks remain mandatory.
These repairs passed offline tests but **did not fix the installed lifecycle failure**.

Focused checks passed: actual-helper4, driver9, root1 (successful and nonzero model exit),
helper7, plus the installed-probe budget guard. The `close3-*` checkpoint passed all workspace
suites (engine1593 with3 ignored, app337 with1 ignored, UI217), strict all-target Clippy and
fmt/diff/source equality, hash
`38fedce6b4a3508331cb260cf5ccb4d0e4d68b79d5d13c0c3ae64c87a42640d3`.
Earlier failed gates retained an outdated helper EOF fixture and the fake-editor ETXTBSY race.

The later `signal-*` gate exposed a separate exporter health race: an older background cycle
could overwrite a credential-clear/configuration reply with stale status. Background health
updates now carry a configuration revision, checked under the health/publication lock. A
regression supplies an old cycle after credential removal and disabling; it cannot restore
the old status. All nine focused export tests passed. Final `verified-*` gate passed: 2,553 tests,13 ignored,
zero failed (engine1594/app337/UI217 and all other suites), strict all-target Clippy, fmt/diff
and source equality; SHA256
`33a4ee1fc9123e5a54414b760dbb8308689c548a0b7d936c93a8331d269b36a3`.
The retired test fixtures and failed logs remain retained. Public orchestration stays disabled;
no release/install, running-app mutation, commit or push was performed.

## Slice24: SIGINT-driven helper connection loss (2026-10-18)

Installed CLI2.1.301's connected Unix MCP cleanup first sends SIGINT to the MCP server PID,
then escalates to SIGTERM/SIGKILL. Read-only inspection of the installed executable established
this behavior without launching it. The helper previously caught only SIGTERM. An actual debug
helper/native Write fixture reproduced signal2 termination when SIGINT arrived with stdin still
open, matching the abrupt channel loss. Prior probe transcripts also contain a successful native
tool result and a final assistant text record; those observations do not establish root success.

The dedicated helper now registers both signals before connection and routes either through
its existing authenticated InputClosed/Closed handshake. The host still waits for the original
owner's terminal and checks process exit, output drain, projection and receipts. A helper exit
never supplies model completion; owner cancellation still wins. No normal app signal handling
or public orchestration route changes. Six actual-helper tests passed after the fix, including
SIGINT with/without the owner terminal and after Stop. The pre-fix SIGINT test fails with signal2.
Full workspace verification passed: 2,555 tests, 13 ignored, zero failed; strict all-target
Clippy, fmt/diff, frozen-source and prompt-ledger equality. Evidence: `dist/cli-bridge-sigint/`;
source SHA256 `41ccf583ea2b1e5cc9bd712e904cf34c062d0979df511608fc93dad2d29ee8cc`.

No Claude process or prompts, network model calls, credential/config edits, app restart,
release/install, commit or push. All exhausted budgets and the three historical Unknown roots
are preserved. Installed fresh/resume qualification remains unverified; future live prompts
need renewed bounded authorization. The remaining parity checklist is unchanged.

## Slice25: installed fresh-and-resume qualification (2026-10-18)

The user authorized two new Sonnet prompts after the SIGINT repair. In one new isolated project,
installed CLI2.1.301 completed a fresh native Write (`native-first`) and then resumed the same
UUID to write `native-resumed`. Both root receipts are Completed, both native tool receipts are
completed, both bridges closed, and the file contains the final marker. Durable evidence for
both turns records a successful terminal, exit0, reaped/drained process and finished projection,
with no parse/stream/projection/cleanup failure. API provider requests remain zero.

The new pair ledger is allowance2/spent2. Prior ledgers are byte-for-byte unchanged and prior
Unknown roots remain untouched. An initial Cargo-driven preflight refusal consumed no prompt:
Cargo's injected loader-path variable conflicts with the production environment guard. The
same built test binary launched directly passed preflight; the guard was not changed.
Only the ignored harness changed to select this newly authorized pair. Final offline workspace
verification passed: 2,555 tests, 13 ignored, zero failures; strict all-target Clippy, fmt/diff,
source equality and unchanged earlier ledgers. Source SHA256:
`04d392360004d816534d29a25ba151300d6f041f4d76d2ab8e83ca096417cb15`. Evidence: `dist/cli-sigint-live/` and `live/probe-veABjp/` beneath it.

This is installed fresh/resumed native Write qualification, not every workflow or the desktop.
Public orchestration remains disabled; the capability checklist still applies. No release,
installation, app restart, commit or push occurred. No further inference is authorized.

## Slice 26: explicit child continuation and inherited context (2026-10-18)

The native `subagent` tool accepts `child` plus the next `task` to continue an existing child.
It reuses the same immutable CLI binding and UUID through the existing supervised resume path.
The original connected parent approval retains the child's policy and concrete tool/resource
objects in memory. Neither saved policy JSON nor a session UUID reconstructs that ownership.
Reapproval, disconnect/removal, or restart cannot adopt old child grants. Profile, context,
model and thinking overrides are refused. Later profile/settings changes do not rewrite the
child's original system prompt, catalog, context snapshot or CLI alias. API fallback remains
absent. The ordinary native/API child host explicitly refuses this CLI-only continuation input.

Inline, notification-only and wake continuations use the same tracked worker and native effect
journal as fresh children. Admission atomically reserves the next request/run and changes the
task to running with its old result cleared; it rechecks child readiness, frozen concurrency
caps and the authenticated parent intent. Busy children, unresolved deliveries, foreign child
numbers, unknown outcomes and duplicate continuation intents do not create another run.
Each wake is bound to the exact latest child request and parent call; a stale wake cannot
suppress a newer delivery. Settlement and startup recovery read the **latest** child turn,
so an earlier completed turn cannot hide a failed/unknown continuation. Existing notes remain.

`context: inherit` now captures a bounded, consistent parent revision and recent entries under
the live native parent call. Existing excerpt rules retain up to20 user/assistant text messages
and16K characters, with omission metadata; partial/retired/system/tool/thinking content is
excluded. Aggregate stored input is bounded before loading. Fresh children do not load parent
history. The immutable snapshot stays in the frozen policy and is served as role-labelled JSON
by a child-only, read-only `parent_context` native tool. The system instruction requires reading
it before work. Native journal checks enforce a successful context-read receipt in **each**
child turn before any other tool effect or root completion. The context tool has a128KiB
output limit so the bounded excerpt survives worst-case JSON escaping and UTF-8 without
truncating its JSON value. A printed SDK tool call, terminal,
or empty excerpt cannot bypass this requirement.

This is **bounded reference-context transport via MCP**, not a fork of the parent's complete
Claude session or role-prefilled CLI history. Parent messages are never concatenated into the
task/system prompt, fabricated as CLI transcript entries, or treated as effect authority. The
new task remains exact; continuation keeps the original excerpt instead of silently capturing
new parent history. This preserves Pipkin's bounded-excerpt inheritance contract.

Offline fixtures exercise same-UUID fresh/continued children, all three delivery modes,
context-before-effects/completion, exact snapshot/system/task preservation, duplicate MCP
receipt replay, cross-parent-turn continuation, changed profiles, lost approval, busy/foreign
children, policy overrides, Stop/Unknown and metadata-only restart recovery. Evidence and the
final gate result are recorded in `dist/cli-child-context/` and the current handoff. No installed
CLI process or new Claude prompt is used. Public orchestration remains disabled; live child
compatibility and desktop qualification remain unverified.

**Final gate passed:**2,559 tests passed,13 ignored, zero failures across48 suite results;
strict all-target workspace Clippy, formatting and diff checks passed. The76-test child-focused
batch and the separate escaped/Unicode context regression also passed. The latter reproduced
ordinary output truncation before the fix. Source remained frozen and every Claude budget
ledger remained byte-identical. Source-manifest SHA256:
`086435b9d1eb44bfae97209b8bbbf4363c1cdc7252b6730b6ef74de60243eba7`. Final evidence:
`dist/cli-child-context/{summary.json,workspace.log,clippy.log,fmt.log,diff.log,source-before.json,source-after.json}`.


## Slice 27: goal lifecycle and independent CLI auditors (2026-10-18)

`runtime/cli_agent_goals.rs` owns a tracked private goal controller. `GoalOwner` retains the
original connected root grant and Stop epoch, and prevents duplicate controllers. Explicit
start or resume is person intent; model goal tools cannot create a goal, resume it, or widen
its allowances. The controller keeps its captured resources across scheduled turns and uses
only the owned CLI admission/runtime. An idle clean root is required to acquire the controller;
shared generation capacity may then be awaited without committing input. Public RPC routing
and the existing desktop goal controls remain part of the gated desktop activation work.

The next goal epoch, turn count and request/input/run are committed in one transaction under
the original live owner after capacity acquisition. Exact goal comparison rejects stale
preparation; duplicate lookup returns the existing receipt without another lease or turn.
Scheduling responds only to its own completed CLI turn. Structured decisions take priority;
final-line continue/blocked/unreachable fallbacks read only that turn's persisted answer.
Ambiguous replies pause, and a `MET` marker cannot bypass mechanical evidence or required
audits. Limits and stalled progress pause; queued human messages take precedence. Stop,
revocation, removal/shutdown and uncertain roots never authorize replay. Restart retains only
data and the existing recovery pauses goals. Explicit return still requires a live approval
and clean CLI history; a new registry cannot adopt a stored controller.

Required audits are inline fresh CLI children authorized by the actual native completion
proposal. They use a frozen goal/claim fingerprint, fresh context, immutable CLI alias and
inspection-only tool catalog. The engine intersects even an unrestricted custom auditor
profile with `AUDITOR_TOOLS`; mutation, shell, network, delegation and parent-history transport
are absent. A pinned API model is refused without inference. The policy, pending goal and
native proposal are rechecked atomically before child creation. An auditor must leave an
actual successful native inspection receipt; SDK text or an `approved` sentence alone cannot
complete it. Normal child-continuation requests cannot reuse an auditor.

The existing audit domain settles clean results: approval completes; requested changes are
included in the next goal input; subsequent rounds use another fresh auditor UUID. Stale file
or plan fingerprints, malformed/inconclusive replies, failures and Stop do not approve.
Round allowances are checked before creating another auditor. Completion-tool text and
structured outcome now reflect the actual state, including verifying, changes requested and
paused states, instead of always claiming an unaudited completion.

Goal usage is counted from CLI terminal receipts for the goal's worker/delegated turns, using
a deduplicated traversal. This fixes the API-ledger path incorrectly reporting zero CLI spend
and prevents charging inline auditors twice. Unknown/missing fields are not invented: known
tokens are a labelled minimum and token-limited scheduling pauses on incomplete usage.
Dollar-limited goals pause before inference because the CLI receipt contract contains no
money measurement. No API-price approximation is used. This is scoped goal accounting;
complete aggregate/UI usage parity remains separately unqualified.

Offline coverage includes start/continuation/resume, same worker UUID, duplicate/stale atomic
admission, limits/stalls/missing usage/cost refusal, old-answer isolation, evidence-only
completion, fresh auditors, read-only/profile/API-model restrictions, duplicate proposal
receipts, two audit rounds with repair input, stale/malformed/inconclusive/no-inspection
verdicts, Stop during audit, lost approval, uncertain roots and non-adoption after restart.
40 focused goal tests passed. The final offline workspace gate passed **2,568 tests, 13 ignored,
zero failures**, plus strict all-target Clippy, formatting and diff checks. Source stayed frozen;
SHA256: `f0729d9b6c4ff024956d1d29e93254f35cce8d882f14a803136bd364dcec5182`.
Evidence: `dist/cli-goal-lifecycle/summary.json` and adjacent logs. Zero new Claude prompts;
all budget ledgers remained byte-identical. Live goal/auditor compatibility and desktop qualification
remain unverified. Public orchestration remains disabled; no release build/install, app restart,
commit or push is part of this slice.


## Slice 28: context, compaction and queued human input — 2026-10-19

The private input route creates one nonserializable ticket per newly queued request, bound to
the original connected root grant and continuation epoch. A bounded queue uses the existing
request rows and QueueChanged/withdrawal views. A separate immutable intent row identifies
input kind, generation, operation and ticket. Exact duplicates return the same queue entry
without another actor; conflicting reuse fails before mutation. Each original ticket is
tracked synchronously, waits for the existing root to finish and for shared capacity, then
atomically consumes its own row into one CLI run. The same CLI UUID, root resources and
supervised admission/cleanup path are reused. No API prompt runner or queue recovery starts it.

Follow-ups preserve FIFO. Steering has priority among unreserved inputs and is delivered as
its own user turn at the **next clean CLI turn boundary**. It does not interrupt a running
process or claim to reach an in-progress model/tool step. Already-reserved work keeps its
place. Mid-turn streaming/tool-step steering remains a distinct parity item. Each queued
message remains its own turn; native API batching settings are not applied to CLI input.
Goals yield to queued person intent. Stop, withdrawal, revocation, failed preparation,
uncertain history and shutdown suppress pending work. Individual withdrawal cancels a
capacity wait without waiting for the held session lock. Startup withdraws orphaned queue
rows and retains their text; stored data never reconstructs tickets or dispatches after restart.

Explicit compaction sends only a host-generated `/compact` plus optional single-line
instructions to an existing clean CLI session. Normal turns still disable slash commands.
For that typed compaction turn only, the launcher omits that flag and adds `--bare`, retaining
the fixed system, disabled built-in tools/hooks, and explicit MCP configuration. The bridge
can initialize and close, but rejects native tool admission before intent/effects. A result
claiming success without an observed scoped `compact_boundary` cannot complete the turn;
its uncertain history is fenced and queued work cannot continue. Actual completion still
requires all existing terminal/process/drain/bridge/projection evidence. No native/API
summarizer, fabricated CLI transcript or replacement context is used.

Scoped automatic/manual compaction boundaries and per-assistant-message usage are bounded
observations. Pipkin reports the last model request's **input** (input plus both cache-input
fields), with a distinct source/label, never cumulative result usage or estimated desktop
transcript size. Missing/overflowing values, capacity and post-compaction occupancy stay
unknown. A boundary invalidates the previous occupancy measurement. Data from an unfinished
or uncertain turn (including a later-fenced completed receipt) cannot become a current measurement.
Settlement publishes the committed CLI revision under the publication gate, preserving native
event order without acquiring the session registry. Native compaction entries are not
created and displayed history is retained. Schema30 adds only input intent and context
observation storage; it does not persist execution authority.

The command shape follows Anthropic's [SDK command documentation](https://code.claude.com/docs/en/agent-sdk/slash-commands#compact-history-with-compact)
and [CLI flags](https://code.claude.com/docs/en/cli-reference). The exact compact launch/MCP
lifecycle remains installed-CLI qualification work; offline fixtures are not that evidence.

**Final verification passed:** **2,576 workspace tests passed, 13 ignored, zero failures**
across 48 suite results. Strict all-target workspace Clippy (`-D warnings`), formatting and
diff checks passed. The six existing committed-order regressions pass unchanged, and the
fenced-history context regression passes. Source remained frozen across the final gate;
all Claude budget ledgers remained byte-identical. **Zero new Claude prompts.**
Source-manifest SHA256 (Rust/SQL/manifests and compiled reference):
`dcec23111994a25ef20364480b4c9dd024c3b15cfb8d06bf6c2172d25f748a64`.
Evidence: `dist/cli-context-inputs/{summary.json,workspace.log,clippy.log,fmt.log,diff.log,source-before.json,source-after.json,budgets-before.json,budgets-after.json}`.
 Tests exercise priority/FIFO,
duplicates, withdrawal during capacity wait, Stop/revocation/recovery, compaction boundaries,
refusal of compaction tool effects, same-UUID continuation and truthful context accounting.
Zero additional Claude prompts are authorized or used. Public activation, mid-turn steering,
CLI batching semantics and live/desktop qualification remain separate work. No release,
install, running-app change, commit or push.


## Slice 29: mid-turn steering and queue batching — 2026-10-19

Implemented in the private CLI orchestration route. This milestone supersedes slice28's
next-turn-only steering and one-message-per-turn limits. Public activation remains disabled.

- **Mid-turn steering:** an original connected user ticket can join a running root at the
  next authenticated native tool-result boundary. The MCP response preserves the tool output
  and adds a separately labelled block of exact user messages with escaped JSON boundaries.
  Canonical user entries, request-to-run membership and the exact replayable tool response
  commit in one transaction. Replaying the same call returns the same response and cannot
  consume another message. Tool observations in CLI stdout still authorize nothing.
- **Queue batching:** `steeringMode` and `followUpMode` now apply to private CLI input.
  `one-at-a-time` selects one; `all` selects a bounded FIFO batch of the same kind. Steering
  retains priority among unreserved inputs. A batch uses one CLI invocation with each exact
  message and request ID preserved in a JSON envelope. Every member retains its own durable
  request, canonical entry and outcome tied to that run. Compaction remains a separate action;
  follow-up batching cannot cross it. Already-reserved work keeps its place.
- **Ownership and failures:** only live tickets from the exact original grant may participate.
  Revocation followed by reapproval cannot adopt old tickets; stored intent rows cannot mint
  new tickets. Selection happens under the publication/registry/storage ordering, with capacity
  reserved before turn admission. Withdrawal before selection removes a member. After selection,
  the root owns its outcome. Members wait for their delivering root's cleanup, without waiting
  on unrelated later turns. Lost delivery/connection or uncertain completion settles members
  with the root's uncertain outcome; no automatic replay or fresh invocation follows.
- **Bounds:** at most the existing64 queued inputs; selected text totals at most96KiB before
  JSON escaping, with final prompt/result limits checked before commitment. A message too large
  for a mid-turn boundary stays queued intact for a standalone turn. If a tool result leaves
  insufficient space, the steering remains queued. There is no silent truncation.

Schema31 adds immutable delivery membership linking each additional request to its run,
canonical entry, delivery channel and native call when applicable. Failed batch admission
rolls back every member. Preparation rejection, Stop/uncertainty and recovery settle members
with their root; orphaned pending inputs remain withdrawal-only on recovery.

**Semantic boundary:** steering reaches a native tool-result boundary, not an interruption
of model token generation or a synthetic SDK user-message record. If no suitable boundary
occurs, it runs at the next clean CLI turn. Follow-ups are never injected mid-turn. Mixed
steer/follow-up batches remain separate to preserve steering priority and typed compaction.
Installed Claude must still be qualified for how it follows the supplemental user block;
offline peers establish transport, ownership, ordering and persistence, not model behavior.

**Verification passed:** all40 focused input-related tests passed, including the seven new
integration cases and atomic batch rollback test. The full workspace gate passed **2,584 tests,
13 ignored, zero failures** across48 suite results. Strict all-target workspace Clippy
(`-D warnings`), formatting and diff checks passed. Source remained frozen through the final
gate; all five Claude prompt-budget ledgers remained byte-identical. **Zero new Claude prompts.**
Source-manifest SHA256 (Rust/SQL/manifests and compiled reference):
`a4a5260f5a91458ea2fc911ada095b5ec045c11e238c3c9b7bdd2947edc1621e`.
Evidence: `dist/cli-steering-batching/{summary.json,focused-final.log,workspace.log,clippy.log,fmt.log,diff.log,source-before.json,source-after.json,budgets-before.json,budgets-after.json}`.
Earlier focused logs retain fixture failures (missing settings directory, first-line-only
stdin capture and incorrect expectations for the no-goal error response); the final results
supersede them. Tests cover same-UUID batching/resume, per-message persistence and outcomes,
mid-turn one/all modes, exact replay, withdrawal, capacity waits, compaction barriers,
oversized-message deferral, connection-loss uncertainty and non-adoption after reapproval
or recovery. Original tool output remains separate from the added user steering block.

**Remaining:** installed-CLI and desktop qualification of these paths; remaining background
integration; broader concurrency/payload/post-Stop, thinking/attachments/usage and concurrent
MCP coverage; the existing desktop setup/model/approval route. Previous live fresh/resumed
native Write evidence retains its original scope. No additional Claude prompts, release build,
install, running-app mutation, commit or push. No dashboards or Pi extensions.

## Slice 30: background delivery integration (private, offline)

Implemented in the private CLI orchestration route (slice30). This closes the remaining
background **delivery and scheduling** item from slice29. Public activation remains disabled;
aggregate usage, broader parity and installed/desktop qualification retain their separate gates.

- **Scheduling:** completed child/job wakes register with the original connected grant and
  its current continuation epoch. Queued user input goes first; ready wakes run in observed
  readiness order; new goal turns yield to ready wakes. Already-reserved work keeps its place.
  One-use tickets are bounded to 1,024 engine-wide and removed when their tracked actors finish.
  Neither a saved request nor reapproval can adopt an old ticket.
- **Long-running jobs:** completion delivery gets a fresh 30-minute window when completion is
  observed, covering scheduling, capacity waits and the follow-up turn. It no longer expires
  at the launching turn's deadline. The job supervisor still owns process timeout/cancellation;
  Stop, revocation, disconnect and shutdown still suppress continuation. No timeout causes replay.
- **Confirmed handoff:** the tracked job-start actor now waits for the actual successful
  authenticated native tool receipt before detaching, including notification-only jobs. If
  startup loses its result/authority, it cancels any retained process and suppresses its wake.
  A receipt committed before normal parent teardown preserves the confirmed background job.
- **Child-owned job wakes:** a worker's job can resume that same child UUID under its original
  live parent grant, frozen model/system/tools and inherited-context snapshot. It waits for
  the child actor's result and its original parent notification to settle. Admission atomically
  marks the child running and consumes the job wake; after process/bridge cleanup, the parent
  directory receives the child's updated result or uncertainty. Failed admission cannot replace
  the previous child result. This does not create another parent wake or allow nested subagents.
- **Recovery:** existing cleanup-only recovery remains in force. Saved jobs, child policies,
  bindings and receipt rows are evidence, never permission to reconstruct inference authority.
  Generic job completion cannot route CLI sessions through the API runner.

No schema change (schema31). New code is concentrated in `cli_agent_owner/wake.rs`,
`cli_agent_owner/{children,goals}.rs`, `cli_agent_host/{wake,jobs}.rs`,
`store/cli_agent_turns.rs` and `store/jobs.rs`. `cli_agent_root/background_tests.rs` uses only
owned shell executables, local native-MCP peers and isolated engine stores. Its four tests
cover eight scenarios: queued-input/ready-wake/goal ordering, expiration of the original turn
deadline, cancellation after spawn before a committed receipt with wake both off/on, and child
job continuation with notify/parent-wake/child-Stop/parent-revocation. Resumed child system
text is byte-identical; inherited context remains available; scripted API providers receive
zero requests. Existing child/job lifecycle, capacity, replay and recovery tests remain required.

**Verification passed:** all four new focused tests (eight scenarios) and the full workspace
gate passed: **2,588 tests passed, 13 ignored, zero failures** across48 suite results. Strict
all-target workspace Clippy (`-D warnings`), formatting and diff checks passed. Source stayed
frozen through the gate, and all five Claude prompt-budget files remained byte-identical,
including comparison with the previous milestone. **Zero new Claude prompts.**
Source-manifest SHA256 (Rust/SQL/manifests and compiled reference):
`4c5275620f40641cfc71e518fb52785c3beb834acf00b0f0052addfee9eb2e3c`.
Evidence: `dist/cli-background-integration/{summary.json,focused-final.log,workspace.log,clippy.log,fmt.log,diff.log,source-before.json,source-after.json,budgets-before.json,budgets-after.json}`.
The earlier `check.log` retains a corrected test-fixture compile error; the final focused
and workspace runs above supersede it.

**Remaining:** broader concurrency/payload/post-Stop, thinking/attachments/aggregate usage
coverage; the existing desktop setup/model/approval route; installed-CLI and desktop lifecycle
qualification, including background interactions with compaction/steering and goal allowances.
Offline ordering tests establish transport and ownership, not installed Claude behavior or
complete goal usage accounting across every background descendant. Existing live native Write
proof retains its recorded scope. Zero additional Claude prompts; no release build/install,
running-app mutation, commit or push. No dashboards or Pi extensions.

## Slice 31: aggregate background usage and goal allowances (private, offline)

Implemented privately in slice31. This closes slice30's aggregate background-descendant
accounting and goal-allowance implementation gap. Public orchestration remains disabled.

- **Causal accounting:** scheduled goal turns, native child/auditor calls, explicit child
  continuations, parent wakes and child-owned job wakes are joined through their actual
  native call/run receipts. Further wakes and descendants retain the originating goal.
  Each run is counted once, even if multiple paths reach it. Unrelated user turns are excluded;
  a late receipt for an archived goal cannot charge its replacement. Goal/wake request-name
  prefixes are reserved at admission so an ordinary input cannot claim those identities.
- **Durability:** admission and terminal cleanup reconcile absolute totals. Recovery repairs
  a crash between receipt commitment and publication without starting any inference. Repeated
  reconciliation does not add usage again or lower an already-recorded minimum. Current-goal
  updates use the existing ordered `GoalChanged` event; archived goals retain their own totals.
  No new schema is required (schema31).
- **Reported tokens:** input, output, cache-read and cache-creation tokens all contribute.
  In-flight usage is pending; a clean pre-launch rejection spends zero tokens; an uncertain,
  fenced or partially reported terminal receipt leaves usage incomplete. Known fields remain
  a minimum, with a retained note. Arithmetic saturates at the storage limit and marks overflow
  incomplete. No streaming-message double counting, API usage fallback or invented dollar cost.
- **Allowances:** scheduled worker turns and autonomous completion wakes (including child job
  wakes) consume turn allowance. Child work within an admitted iteration contributes tokens;
  initial/explicit child calls and independent auditors do not separately consume worker-turn
  allowance. Auditor round limits and existing child limits still apply. A rejected admitted
  reservation keeps its turn slot. Token/dollar/state/identity checks run again at admission and
  immediately before committing launch intent, covering changed limits during capacity or
  preparation waits. A wake cannot spend a paused, completed, replaced or exhausted goal.
- **Coordination:** the goal controller waits for pending background inference and wake delivery
  before scheduling another worker. It lets an already-reserved last turn finish before applying
  the next-turn limit. A worker cannot certify completion while its background work is unsettled.
  Completion waits for the proposing wake’s terminal receipt; an uncertain execution result
  pauses a provisionally completed goal, including during recovery. The controller reconciles
  receipts and reads goal totals in one transaction before deciding to continue or finish.
  Stop, lost ownership and recovery never replay or adopt the work; already-running descendants
  can still report their actual usage to the original goal after it stops or is replaced.

**Allowance boundary:** token allowances gate new invocations using reported receipts; they are
not hard mid-turn token caps or predictions. Already-admitted concurrent work can report an
overshoot. Missing settled usage blocks further token-limited work. Dollar-limited CLI goals
remain refused because trustworthy dollar usage is unavailable; subscription costs are not
inferred from API prices. Changing an allowance does not reconstruct a consumed wake ticket.
Unknown execution remains non-resumable under the existing guards even if a limit is changed.

**Verification:** all nine new accounting regressions and the eight existing goal-lifecycle
tests passed in the full workspace gate: **2,597 passed, 13 ignored, zero failures** across
48 suite results. Strict all-target workspace Clippy (`-D warnings`), formatting and diff checks passed.
Source remained frozen, and all five Claude prompt-budget files stayed byte-identical.
**Zero new Claude prompts.** Source-manifest SHA256 (Rust/SQL/manifests and compiled reference):
`7af59f5ad39636781b6d8c5cc26e5a8801e6bc72480c8cebe43c8188f943d4e3`.
Evidence: `dist/cli-background-usage/{summary.json,new-regressions.log,workspace.log,clippy.log,fmt.log,diff.log,source-before.json,source-after.json,budgets-before.json,budgets-after.json}`.
Preliminary failures are retained: the capacity fixture reserved only currently available permits
before cleanup released the last one; completion could return a goal snapshot older than the
terminal usage receipt; and the fixture peer started authentication before CLI launch.
The final gate supersedes these runs after fixing full-capacity reservation, atomic accounting
snapshots and launch-synchronized fixture connection. Production timeouts were not weakened.

**Remaining:** broader concurrency/payload/post-Stop parity, thinking/attachments and other
usage metadata; the existing desktop setup/model/approval route; installed-CLI and desktop
qualification of the completed workflows. Previous live native Write evidence retains its
original scope. Zero new Claude prompts; no release build/install, running-app mutation,
commit or push. No dashboards or Pi extensions.

## Slice32: concurrency, payload and post-Stop behavior — 2026-10-19

Implemented privately in slice32 under the existing no-uncertain-replay contract. Public
orchestration remains disabled; installed-CLI and desktop qualification remain separate.

- **Concurrent tools:** four native workers per bridge, with FIFO dispatch and out-of-order
  completion. A tool marked sequential waits for earlier workers and blocks later dispatch
  until it settles. MCP supplies individual calls, not the native runner's complete round;
  these are ordered exclusive barriers, not a claim of SDK round reconstruction. Existing
  mutation locks and engine-wide process permits still apply. Protocol identities and pending
  duplicates never create another worker. Root publication/steering stays in commit order;
  one queued steering message is delivered once even when native results finish concurrently.
- **Per-call cancellation:** queued cancellation commits an actual pre-execution abort receipt
  without invoking the tool; duplicates replay that receipt. Active cancellation signals only
  that call and commits its actual returned result, preserving unrelated work. A tool that
  does not settle within five seconds fences the bridge; remaining cleanup is bounded, aborted
  workers are joined, and unfinished effects remain Unknown. Owner Stop, disconnect, expiry
  and protocol/storage failures still revoke the entire lifetime. No synthetic success or
  effect rollback is claimed.
- **Payloads:** orchestration prompts now use the native 2 MiB limit at queue, admission,
  stored-context and launch boundaries, including exact UTF-8 stdin delivery. MCP request and
  response frames use the native 32 MiB ceiling; stored response bodies reserve 1 KiB for the
  JSON-RPC envelope. Structured native image results and retained details remain structured.
  Oversize input is refused, never silently shortened. The older chat/Coding prompt policy
  retains its own limits.
- **Observation path:** orchestration SDK lines allow a native frame plus 4 KiB of metadata;
  supervised stdout is bounded at 256 MiB and cumulative projected display at 4 MiB. SDK tool
  echoes retain correlation digests but do not duplicate or charge the actual native tool
  output. That output remains in the MCP/native receipt. Exact inventory, UUID, contradictory
  replay and malformed-stream checks remain active; SDK content never authorizes an effect.
- **Resource bounds:** at most 64 pending calls and 64 MiB of encoded queued/active arguments,
  plus the bounded incoming frame; four workers bound the retained completion queue, and one
  response writer applies backpressure. The durable bridge argument/result journal is capped
  at 256 MiB and native transcript/retained output at 512 MiB per run. Catalog, request-count,
  child/system snapshot and native per-tool retention limits remain. These are explicit finite
  resource ceilings, not unbounded payload support or measured process-RSS caps. Exceeding a
  lifetime/output quota fences without replay or fabricated completion.
- **Post-Stop:** worker cancellation inherits Stop directly rather than waiting for the bridge
  event loop. Disconnect also signals active workers immediately. Safe pre-launch rejection
  permits a new explicit fresh turn after cleanup; Stop after a clean idle turn or during an
  unadmitted capacity wait permits a new explicit same-UUID continuation. Old queued inputs
  and background tickets remain cancelled. A launched turn without confirmed completion
  remains Unknown/non-resumable, with known effects retained. This does not introduce repair
  or adoption of an uncertain CLI transcript, reapproval-based replay, or API fallback.

No schema migration (engine31). Code spans the bridge driver, orchestration policy/observer,
shared decoder's private exact-inventory mode, root admission/store limits and native receipts.

**Verification passed:** ten new focused regressions, the expanded pre-launch Stop regression,
and the full workspace gate: **2,607 passed, 13 ignored, zero failures** across 48 suite results.
Strict all-target workspace Clippy (`-D warnings`), formatting and diff checks passed. Source
remained frozen; all five Claude prompt-budget files stayed byte-identical, including comparison
with slice31. **Zero new Claude prompts.** Source-manifest SHA256 (Rust/SQL/manifests and compiled
reference): `4741f1b6cfa0b289cba6915395e51749f10931b605b165448f4f04622e8f2751`.
Evidence: `dist/cli-concurrency-payload-stop/{summary.json,parity-final.log,new-regressions.log,prelaunch-stop.log,workspace.log,clippy.log,fmt.log,diff.log,source-before.json,source-after.json,budgets-before.json,budgets-after.json}`.
The initial 2 MiB prompt test exposed the remaining 1 MiB runtime admission cap; the corrected
run verifies the exact native boundary through stdin and refuses one byte over it before
admission. The superseded failure is retained in
`dist/cli-concurrency-payload-stop/parity-pre-admission-limit.log`.
Owned fake CLI/native-MCP peers and isolated files/stores only. No installed Claude inference,
credential/configuration changes, release/install, running-app mutation, commit or push.

**Remaining:** thinking, attachments and other usage metadata; public setup/model/approval
routing in the existing desktop; installed-CLI and desktop qualification, including concurrent
MCP behavior, larger payloads and the documented post-Stop boundaries. No dashboards or Pi
extensions. Unknown execution is deliberately not made resumable by this milestone.

## Resumed slice33 — thinking, attachments and usage metadata (2026-10-19)

Implemented privately in slice33. Public native orchestration remains disabled.

- **Thinking:** approved creation accepts CLI effort `low`, `medium`, `high`, `xhigh` and `max`;
  the requested level is part of the immutable binding and launch arguments. Every fresh
  preflight, clean resume and explicit return checks the same identity. Children/auditors
  inherit it unless their captured profile explicitly overrides it; continuation keeps the
  captured value. Detached edits cannot rebind a live owner. CLI help must advertise `--effort`.
  `off`, `minimal`, unknown levels and `ultracode` are refused rather than silently mapped.
  The setting is a **request**, not proof of an effective level: CLI model/organization caps
  can clamp it. Thinking display remains a separate native block, not an extra token charge.
- **Attachments:** PNG/JPEG/GIF/WebP base64 image inputs now travel through original root
  admission, canonical user entries, durable requests, exact duplicate identity and queued
  follow-ups. Image-only messages work. Launch uses one structured `stream-json` user message
  on stdin, with text and images separate; ordinary text/compaction retains its existing input
  path. No image data goes into arguments or a mutable file reference. Invalid encoding/type,
  empty images, over eight images, over 8 MiB decoded per image or a combined frame over the
  native 32 MiB ceiling are rejected before admission. Prompt text retains its 2 MiB limit.
  Existing desktop text-file/marked-project-reference preparation remains the upstream format.
- **Queue/steering:** image messages are FIFO batch boundaries and run intact in their own
  invocation; adjacent text-only messages still obey one/all batching. Mid-turn image steering
  uses structured MCP image blocks, with exact native response replay and atomic user-history
  delivery. If the actual tool result leaves insufficient frame space, the message stays queued
  for a later boundary/clean turn. Compaction rejects attachments. Inherited child context keeps
  its existing bounded text-excerpt contract; it does not silently claim to include images.
- **Usage:** retain reported input/output/cache totals plus optional thinking-output and one-hour
  cache subsets. Invalid optional dimensions stay unknown without erasing valid dimensions;
  a subset exceeding its reported total is ignored. Retain bounded, allowlisted result timing,
  turn count, stop reason, speed/tier, cache TTL/server-tool details and model-usage/context
  metadata. Unknown content/account fields are discarded; debug output remains redacted.
  CLI model totals and reported cost are stored as session snapshots, never added to each new
  invocation. Reported cost is not a subscription bill and does not enable dollar-limited goals.
- **Persistence/accounting:** terminal metadata survives in the immutable completion receipt.
  When display blocks exist, the last assistant entry carries invocation usage and the separate
  metadata snapshot; complete
  token dimensions populate native usage exactly once across all thinking/text blocks. Missing
  dimensions stay explicit in the receipt. Reasoning/cache subsets and cumulative model totals
  do not inflate native/goal token totals. Old receipts deserialize without the new optional
  fields. No schema migration (engine31), owner reconstruction, uncertain replay or API fallback.

**Verification:** **2,621 tests passed, 13 ignored, zero failures** across 48 workspace suite results.
All 14 new regressions passed in the final workspace run; focused feature and store-journal
runs also passed (13 and 36 checks respectively). Strict workspace/all-target Clippy
(`-D warnings`), formatting and diff checks passed. Source stayed frozen throughout the final
gate; all five Claude prompt-budget files remained byte-identical. **Zero new Claude prompts.**
Source-manifest SHA256: `5e0b5305475496068c30166644bdde759d620a98fcc99a6458fea31b0a0663ac`.
Evidence: `dist/cli-thinking-attachments-usage/{summary.json,features-current.log,store-after-fix.log,new-regressions.log,workspace.log,clippy.log,fmt.log,diff.log,source-before.json,source-after.json,budgets-before.json,budgets-after.json}`.
Owned fake CLI/native-MCP peers and isolated
stores/files only; zero installed Claude inference prompts. No credential/configuration edits,
release/install, running-app mutation, commit or push. The first focused run had a test assertion
reading `content` outside the RPC `result` envelope; the corrected test verifies structured image
content and exact replay. The failed log is retained as `features-first.log`.
The first full workspace run also caught a genuine storage boundary: the generic history reader
can treat malformed legacy image JSON as an empty list. CLI executable/queued input now strictly
parses the bounded stored image value before lookup, admission or launch; malformed data cannot
become an empty attachment list. The original launch-CAS regression and a new queued-corruption
regression cover this repair. The failed gate is retained as `workspace-before-store-fix.log`.
A subsequent test-harness-only lint repair maps unneeded channel error payloads to `()`; the
full workspace and strict gate passed again on the final frozen source.

**Remaining:** public setup/model/approval/input routing in the existing desktop, followed by
installed-CLI and desktop qualification of the completed workflows (including effort, images,
metadata, concurrency and post-Stop behavior). No dashboards or Pi extensions. Unknown launched
execution remains non-resumable; changing that contract is not a hidden activation requirement.

Contract references checked without inference: [CLI effort and model behavior](https://code.claude.com/docs/en/model-config),
[structured image input](https://code.claude.com/docs/en/agent-sdk/streaming-vs-single-mode),
[SDK result/model metadata](https://github.com/anthropics/claude-agent-sdk-python/blob/main/src/claude_agent_sdk/types.py),
[thinking-token subset](https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/types/beta/beta_output_tokens_details.py)
and [cache TTL subsets](https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/types/beta/beta_cache_creation.py).


## Slice34: desktop activation and routing — 2026-10-19

Implemented in slice34. Owned Linux desktop hosts now offer **Pipkin orchestration** as an
explicit choice alongside **Claude Code Coding** in the existing Claude connection setup.
This activates the desktop route in source; it is not a release or installed-CLI qualification.

- **Setup and identity:** public `claudeCliAgent` backend and `orchestration` mode map to a
  separate desktop execution profile. The selected Haiku/Sonnet/Opus alias is frozen at
  creation. Orchestration requires its own visible confirmation and metadata preflight;
  configured Coding consent does not silently authorize it. Existing Coding, chat and Write
  bindings retain their executor and policy. Unknown/mismatched identities remain blocked.
- **Connection lifetime:** creation installs the original connected owner; exact creation and
  prompt repeats are receipt lookups. Explicit return revalidates the saved binding and bumps
  the session revision so the desktop can clear its prior blocked state. Setup cancellation
  targets the exact approval token, so stale cancellation cannot revoke a newer approval.
  Reconnect, saved preferences, status views and receipt lookups never recreate authority.
- **Input routing:** prompt admission responds after durable commitment, independently of
  inference completion. Root resources use Pipkin's tool catalog and frozen project/skill
  context, with one captured helper image per engine. Text/images, follow-up/steer queues,
  withdrawal, explicit compaction RPC, Stop and read-only prompt lookup/wait use the existing
  CLI owner/actors; there is no API fallback. Request cancellation remains effective before
  admission, then committed work belongs to the engine's joined lifetime. Queue/goal resource
  preparation captures the original approval and Stop epoch; a stopped or replaced request
  cannot adopt fresh authority when preparation finishes.
- **Desktop controls and views:** native questions, plans, child history/status, job logs,
  web sources, queue, usage and context now survive snapshots for this backend. Attachments,
  steering and follow-ups are enabled in core/adapter policy. Plan edits, job cancellation,
  child enable/disable and child Stop have explicit routes. Model/effort mutation on an
  existing immutable CLI conversation remains refused.
- **Goals:** start/resume/replacement use the CLI-only scheduler and acknowledge initial
  commitment without awaiting the entire goal. Pause/archive, turn/token limits, criteria
  and audit settings use native goal state; changing the audit requirement preserves the
  existing round limit unless a replacement is supplied. Dollar allowances remain refused. Concurrent
  replacement while a controller still owns the goal is refused; finish/stop that work first.
- **Host policy:** owned Linux desktop CLI hosting enables availability; standalone engine
  configuration stays off by default, and disable-CLI/disable-write settings disable desktop
  orchestration hosting. No running Pipkin instance was restarted or reconfigured.

**Verification:** **2,626 tests passed, 13 ignored, zero failures** across 48 workspace suite results. Strict all-target workspace Clippy (`-D warnings`), formatting and diff checks passed. Source stayed unchanged throughout the final gate; all five Claude prompt-budget files remained byte-identical. Offline fake executable/MCP peers only; **zero Claude
prompts**. New coverage includes public setup/trust/create/revoke/reapproval, lookup without
regrant, immutable alias refusal, core/adapter identity and capabilities, durable prompt
acknowledgement before completion, queue withdrawal/snapshots, public goal admission and
limits, receipt lookup after revocation, and absence of API provider calls. Existing GPUI
capability tests also cover the new profile. Evidence: `dist/cli-desktop-routing/`.
The pre-final workspace run exposed a fixture race in the existing maximum-size prompt test:
its fake helper authenticated before CLI launch. The fixture now waits for that turn’s prompt
marker, matching real helper startup; production handshake deadlines are unchanged. The
failed run is retained as `workspace-before-launch-sync.log`.
Final source-manifest SHA256: `ac3c1c964028857fe20d32c426011607af1499191b36fb3b9baad1006cfd7bc8`. Verification details and
source/budget comparisons are recorded in `dist/cli-desktop-routing/summary.json`.

**Historical slice34 remainder:** effort selection, person child launch/steering, session MCP
settings and Skip-audit were outstanding here and are implemented in slice35. Installed CLI
and real desktop qualification remain open.


Earlier slices are historical; the current capability checklist describes activation.


## Slice35 — desktop controls (2026-10-19)

Implemented the remaining four desktop controls through the explicit orchestration executor.

- **Effort at creation:** setup offers Default, Low, Medium, High, Extra high and Max.
  Changing the draft invalidates prior confirmation; requested effort is carried through the
  setup ticket and frozen binding. Existing conversations retain their saved effort. Coding
  refuses an orchestration effort override; effective model/account clamping remains CLI-owned.
- **Person-launched children:** the existing child controls now launch notification-only CLI
  children with fresh or inherited context and frozen profile/tool restrictions. Explicit
  person intent has its own durable receipt and original connected approval; it does not
  fabricate a parent model run or tool call. Exact retries read the original child, even after
  approval is revoked; altered retries and new work without approval fail. Stop during resource
  preparation invalidates the captured intent. Committed children are engine-owned and settled.
- **Child steering:** existing controls queue input for the exact active child. Delivery occurs
  at an authenticated native tool-result boundary. If the child finishes first, the message
  is withdrawn and non-delivery is shown in its task note; it never silently restarts the child.
  Independent auditors refuse steering. Frozen tools and original connected ownership remain
  unchanged. Model-issued explicit continuation remains a separate workflow.
- **MCP settings:** root CLI conversations can manage the native MCP service using their selected
  project scope, including test/reconnect/tool toggles and existing authentication/import routes.
  Configuration access does not grant inference approval. CLI child sessions cannot widen their
  frozen policy through root configuration RPCs; Coding remains on its separate policy.
- **Skip-audit:** the existing action is accepted only for a waiting/reviewable audit. It records
  an explicit skipped verdict, completes the goal, and cancels only its matching auditor. Late
  cancellation/settlement must preserve that decision. Actual auditor usage is retained, including
  unknown usage; unrelated failed work remains eligible to pause a goal.
- **Persistence:** engine schema32 adds an explicit user-origin child-policy shape with run0;
  original model-origin policies stay valid. The v31 migration backs up before changing the
  trigger and preserves existing CLI identity. Protocol41 and desktop schema10 are unchanged;
  new setup effort is an optional additive field. Recovery receipts never mint execution grants.

**Measured offline:** **2,632 tests passed, 13 ignored, zero failures** across 48 workspace suite results.
Strict workspace/all-target Clippy (`-D warnings`), formatting and diff checks passed.
The final source manifest stayed unchanged; all five Claude prompt-budget files remained
byte-identical. **Zero Claude prompts.** Evidence: `dist/cli-desktop-controls/summary.json`;
source-manifest SHA256 `7c880bafa3a521dc3da4e3c845cbe3ffd9f29f61a15e9705d4de272b04b2e7c7`.

Regressions cover effort confirmation invalidation and immutable setup, fresh/inherited person
children, stale preparation after Stop, duplicate/altered/revoked launch receipts, delivered and
undelivered child steering, auditor steering refusal, project-scoped MCP test/reconnect/settings,
Skip-audit during an actual fake auditor turn, late usage reconciliation, unrelated failed-worker
handling, and schema31→32 backup/identity preservation. No API inference was used.

**Corrected during verification:** person-child catalog validation now handles a person intent
without inventing a parent inference run. Skip-audit records the running auditor's identity so
late cancellation/usage cannot overwrite the person's verdict. The older service test now expects
explicit root MCP configuration to pass while native API execution remains refused. The failed
pre-final evidence is retained in `dist/cli-desktop-controls/`. Enlarged setup payloads are boxed
in the app's bounded control queue and demo event actions to keep their outer messages compact.

No installed Claude prompts, credentials, running-app changes, install/release or push are part
of this batch. Wider installed-CLI workflows and real-window qualification remain unverified.

## CLI coding observation and output fix — 2026-10-19

A real CLI2.1.301 **Coding** conversation failed on `ContentBlockStart` with
`UnsupportedTool`; the supervisor then terminated the CLI (exit143). The stored diagnostic
identifies a tool outside the initial inventory, but does not retain that rejected tool's name.
This was the separate `cli-unattended-coding-v1` route, not a native orchestration turn.

- Coding now accepts valid, bounded CLI-owned tool observations even when absent from the
  startup inventory. These remain inert observations; UUID, invocation correlation, record
  bounds and completion checks stay enforced. Restricted Write/chat and the exact native
  orchestration catalog retain their closed inventory checks.
- CLI actions are compact, collapsed activity rows with a status. Their bounded output can
  be expanded and selected as literal formatted output. Thinking is also collapsed; empty
  thinking blocks produce no row. Assistant answers remain normal text. Display-only activity
  cannot become a native tool call, permission decision or retry action, and survives offline
  transcript caching with stable item identity.
- An already interrupted conversation remains fenced: its process was stopped without a
  confirmed result. No history or context-valid flag was rewritten, and no work was replayed.
  Continue deliberately in a new conversation after inspecting any existing effects.

**Measured offline:** 2,635 tests passed, 13 ignored, zero failures;
strict workspace/all-target Clippy, formatting and diff checks passed. Final source stayed
unchanged (manifest SHA256 `fdb72d5781a9a353f4b002594049af100b4991dd55d402b3cde66ff56ee5bdb4`). All five prompt-budget files stayed
byte-identical; zero Claude prompts. The decoder regression exercises `ContentBlockStart`
for a late tool and strict-policy refusal. Fake CLI fresh/resume tests complete without
native API fallback. GPUI tests prove collapsed output is excluded from selection until
expanded; offline cache and stable/inert transcript mapping checks pass. An extra blank
line in documentation was corrected and the actual diff check rerun successfully.

Release build and isolated native-engine probe passed; `scripts/run.sh` now selects the updated
build. Verification and build receipts are recorded in `dist/cli-output-fix/`.
No installed Claude inference was used for this fix. Real-window presentation remains unverified.

## CLI Markdown output — 2026-10-19

Expanded CLI activity and thinking now use the same cached Markdown parser and block layout
as assistant replies: headings, emphasis, links, lists, quotes, tables and fenced code.
Rows remain collapsed by default. The status title stays outside the Markdown body, and
selection uses the original document block positions, including table cells. Parsing stays
outside rendering; content changes invalidate the existing document cache. CLI observations
remain display-only.

Regressions compare the expanded activity body with assistant Markdown, refresh equal-length
content changes, and check collapsed selection, formatted copy, code line breaks and actual
table-column geometry in the GPUI harness. Real-window presentation remains unverified.

**Measured offline:** 2,636 tests passed, 13 ignored, zero failures. Strict workspace/all-target
Clippy, formatting and diff checks passed. All five Claude prompt-budget files remained
unchanged; zero Claude prompts. Source-manifest SHA256
`ece296cf222ce17933c50458edbd4dc55b615e7fcf538447503e3b59c1919496`. Evidence:
`dist/cli-markdown/summary.json`. GPUI harness checks are not real-window qualification.

Release build and isolated native-engine startup probe passed. `scripts/run.sh` selects
the new build; quit the old app first. Build receipt: `dist/cli-markdown/release-build.json`.

## CLI continuation initialization fix — 2026-10-19

**Observed:** the latest resumed CLI2.1.301 Coding turn stopped with `Sequence`, record
`System`, supervisor `ObserverFailure` and exit143. No terminal result was committed.
The decoder rejected repeated `system/init` records, treating per-turn initialization as a
second process start. Raw rejected frames were not retained, so their exact payload is unknown.
The new offline continuation fixture reproduced the same `Sequence` failure before the fix.

**Implemented:** accept repeated initialization before the terminal result, revalidating
session identity, record bounds, tool names and catalog policy each time. Restricted sessions
must retain their original inventory; native orchestration must retain its exact catalog.
Coding can refresh its CLI-owned inventory. Reinitialization does not reset messages, pending
actions, tool correlation or resource counters, and emits no second initialization to storage.
Terminal contradictions and initialization after a result remain failures.

The fake public Coding route now includes two background completion/initialization cycles
before its final answer, exercised on fresh and resumed prompts. Decoder regressions cover
continued text, pending tool results, catalog refresh, invalid records, altered catalogs and
terminal boundaries. The interrupted real conversation remains fenced; no uncertain work was
replayed or history rewritten.

Protocol reference: Anthropic's [SDK changelog](https://github.com/anthropics/claude-agent-sdk-typescript/blob/main/CHANGELOG.md),
v0.3.247, describes per-turn `system/init` frames; v0.3.239 describes one-shot results held
back for background subagents. These are protocol references, not live qualification.

**Verification repair:** the first workspace attempt hit a native goal test race: its resumed
answer could finish before Stop while the test required a paused goal. The fixture now holds
both turns until cancellation; no goal runtime behavior changed. Its isolated recheck passed.
The original failure is retained in `dist/cli-reinit-fix/workspace-first-attempt.log`.
The next run exposed an outdated fixture expectation: the completed prompt correctly retained
both new background progress messages as well as its final answer. The assertion now checks
all three texts; production output handling is unchanged. That failure is retained in
`dist/cli-reinit-fix/workspace-second-attempt.log`.

**Measured offline:** 2,639 tests passed, 13 ignored, zero failures across 48 workspace suite
results. Strict workspace/all-target Clippy, formatting and diff checks passed. Source stayed
unchanged throughout the final gates (manifest SHA256
`05640f2e6594c087445d91f841866a0f85fc6204a25a422456c89bd7c4e85b72`). All five prompt-budget
files remained byte-identical; zero Claude prompts. Evidence: `dist/cli-reinit-fix/summary.json`.
Installed-CLI inference and real-window verification remain unverified.

Release build and isolated native-engine startup probe passed. `scripts/run.sh` selects the
updated build; quit the old app first. Build receipt: `dist/cli-reinit-fix/release-build.json`.

## CLI batched completion results — 2026-10-19

**Observed:** the resumed Coding receipt stored success and clean exit0, then rejected a
second `Result` with `Sequence`/`ObserverFailure`. The raw second frame was not retained;
its precise payload/outcome cannot be reconstructed from the receipt. Read-only inspection
of the installed CLI2.1.301 emitter confirmed its background-result queue flushes several
held per-turn results after the assistant content. This is distinct from duplicate delivery.
Anthropic's [SDK changelog](https://github.com/anthropics/claude-agent-sdk-typescript/blob/main/CHANGELOG.md)
documents held background results (v0.3.239) and per-turn `usage` versus cumulative
`modelUsage` (v0.3.223).

**Implemented:** Coding retains a bounded result aggregate and publishes one terminal event
at stream EOF. Independent supervised exit/drain/cancellation/permission checks still govern
settlement. Distinct result UUIDs add per-turn usage; identical semantic redelivery adds
nothing. Conflicting reuse, missing identity on multiple results, foreign sessions, invalid
mutation surfaces and resource overflow remain failures. Missing usage stays unknown, and
an error or unknown outcome cannot be upgraded by a later successful result. Further root
activity requires a new result; replaying an older result cannot close it. Only the last
result may repair incomplete display, preventing held intermediate summaries from replacing
the final answer. Native orchestration and restricted connections keep their single-result
policy and existing catalog/permission boundaries.

The public fake Coding fixture now flushes three identified results after its background
continuations and verifies fresh/resume, aggregate usage and no replay. Additional tests
cover prior failure followed by success, Stop after results but before exit, partial pipe
chunks, missing final newline, unfinished continuation, duplicate deliveries, pending tools,
identity conflicts, usage overflow and result-count bounds. No old live receipt or history
was rewritten. Existing unknown conversations remain fenced because later raw results were
not retained; no uncertain work is replayed.

**Measured offline:** 2,645 tests passed, 13 ignored, zero failures across 48 workspace suite
results. Strict workspace/all-target Clippy, formatting and diff checks passed. Source stayed
unchanged during the final gates (manifest SHA256
`1af00b4690af2693665dd38370780ec61554ab798b05924b6a0c1ae35eacfe69`). All five prompt-budget
files stayed byte-identical; zero Claude prompts. Evidence: `dist/cli-result-fix/summary.json`.
An early focused test used an already-invalid token value for overflow; the final regression
adds valid counts beyond the aggregate storage bound. Its early log is retained separately.
Installed-CLI inference and real-window behavior remain unverified.

Release build and isolated native-engine probe passed. `scripts/run.sh` selects this build;
quit the old app first. Build receipt: `dist/cli-result-fix/release-build.json`.

## CLI activity presentation — 2026-10-19

CLI tool observations and thinking now join the same consecutive-step groups as native
model activity. Groups start collapsed, remain separated by assistant commentary and user
messages, and use the existing summary, elapsed-time and expand/collapse controls. Expanded
rows use familiar tool icons and friendly labels, quiet success checks, explicit failed or
unknown states, and a thinking preview. The provider is still identified by the conversation
and accessible row labels. A failed step remains visible on a folded group even while work
continues. The live activity strip now counts CLI actions as well as native tools.

Expanded output keeps the shared cached Markdown renderer, including tables and selectable
text. Snapshot refreshes retain group and output expansion by item identity. Saved activity
labels are recognized without a cache migration; unrecognized labels remain visible verbatim.
These are presentation-only observations: no native tool identity, execution, diff, or retry
controls are created, and no missing command arguments or paths are invented.

**Measured offline:** 2,666 tests passed, 13 ignored, zero failures across
48 workspace suite results. Strict workspace/all-target Clippy, formatting and diff checks
passed. The final source stayed unchanged throughout the gates and release build (manifest
SHA256 `1fd61c71722b0e0ba47889d8333ddce8f9a97ecacb4b79f57ff729b8f1119017`).
All five prompt-budget files remained byte-identical; zero Claude prompts. The GPUI harness
verified sixty CLI steps folding around commentary, independent group expansion, refreshed
Markdown details, and expansion persistence across snapshots. Real-window appearance and
installed-CLI inference remain unverified.

The initial focused run passed the new regressions but an existing file-link fixture could
not write its temporary directory inside the sandbox. The unrestricted offline workspace
run passed; Clippy then requested an equivalent iterator cleanup. The final complete run
above includes that cleanup. Earlier logs are retained separately.

Release build and isolated native-engine startup probe passed. Quit the old app, then use
`scripts/run.sh` for this build. Evidence: `dist/cli-presentation/summary.json` and
`dist/cli-presentation/release-build.json`.

## Ordinary CLI coding lifetime — 2026-10-19

**Observed:** read-only receipt 18 records `TimedOut` and an incomplete `EndOfStream`
after 1,800.634 seconds (13:04–13:34 Arizona time). The ordinary Coding launch policy
imposed an absolute 30-minute deadline, regardless of tool activity. The supervisor
terminated the process while a Bash action remained unsettled. This was a lifetime-policy
failure, not a newly rejected CLI record shape. Evidence: `dist/cli-timeout-fix/observed.json`.

**Implemented:** ordinary Coding has no fixed execution deadline. Long builds and quiet
commands may continue until completion or Stop. The shared supervisor now represents the
execution deadline explicitly as optional. Metadata probes, legacy chat/Write, native
orchestration channel lifetimes and approved probe budgets retain their existing bounds.
Output byte limits, Stop/cancellation, owner-drop process cleanup and uncertainty rules
remain in effect. Once an unbounded Coding process exits, output delivery has a separate
30-second drain deadline. Exit is observed even during delivery backpressure; termination
and reaping remain bounded. No timeout renews credentials, grants authority or replays work.

Synthetic subprocess regressions advance a virtual clock by 31 minutes during quiet work
and blocked delivery, then complete normally. Other cases cover cancellation without an
execution deadline, output caps and a stuck observer after root exit. Existing deadline,
process-tree cleanup and fresh/resume tests remain part of workspace verification.

**Measured offline:** 2,669 tests passed, 13 ignored, zero failures across
48 workspace suite results. Strict workspace/all-target Clippy, formatting and diff checks
passed. The final source stayed unchanged throughout the gates and release build (manifest
SHA256 `62d114e4ead4a4ffdcd1b326e2ae160aef4542f8991ef4d50ffdc68dc8f03061`).
All five prompt-budget files stayed byte-identical; zero Claude prompts. The 31-minute
elapsed-time cases use a virtual clock with real synthetic subprocesses, not real Claude
inference. Installed-CLI long-turn behavior and real-window qualification remain unverified.

Release build and isolated native-engine startup probe passed. Quit the old app and launch
with `scripts/run.sh`. Evidence: `dist/cli-timeout-fix/summary.json` and
`dist/cli-timeout-fix/release-build.json`.
The historical interrupted receipt is unchanged and remains uncertain; inspect existing
project files in a new conversation before continuing, rather than replaying the old prompt.

## Clean CLI Stop continuation — 2026-10-19

**Implemented:** confirmed user cancellation retains the CLI conversation for a new instruction.
The previous request stays aborted and non-replayable, including across restart. The next
prompt uses the same CLI session with only the newly submitted text. Completed actions retain
their results; unfinished actions remain Unknown in the ledger and transcript. Approval
revocation, disconnection, shutdown, malformed output and unconfirmed cleanup cannot grant
continuation. Historical invalidated bindings are unchanged.

**Observed offline:** desktop composer readiness after Stop, same-session resumed completion,
public RPC request deduplication, restart durability, stale action rejection and insufficient
cleanup/authorization evidence. Synthetic CLI fixtures only. **Unverified:** installed-Claude
Stop/resume and real-window interaction; no live inference was authorized or performed.

**Measured offline:** 2,674 tests passed, 13 ignored, zero failures
across 48 workspace suite results. Strict workspace/all-target Clippy, formatting
and diff checks passed. Final source remained unchanged through the checks and release build
(manifest SHA256 `d0ca083e59b461fc78141e3d4ff2883cfb793e2dbb851511a66fd976c3aeffc0`). All five prompt-budget files stayed byte-identical;
zero Claude prompts. Release build and isolated native-engine startup probe passed.

Quit the old app and launch with `scripts/run.sh`. Evidence: `dist/cli-stop-fix/summary.json`
and `dist/cli-stop-fix/release-build.json`. Built binary:
`/home/jason/coding/pipkin/dist/source-setup/builds/226057fee234411bafd6bf1b97a26460/bin/pipkin`.

## Recovery of old clean CLI Stops — 2026-10-19

**Implemented:** fresh matching **Reconnect Claude Code…** can recover an ordinary Coding
conversation invalidated by an older build after a provably clean Stop. Schema34 records an
immutable recovery audit and advances the binding generation; it does not rewrite old receipts,
restore old execution grants or replay prompts. Earlier Unknown runs, missing cleanup evidence,
newer explicit negative authorization, changed identity and revoked setup checks remain blocked.
Migration alone does not unlock conversations.

**Observed offline:** desktop reconnect then same-composer resume, fresh/revoked/mismatched
metadata setup, duplicate request non-replay, restart durability, immutable receipts and unknown
actions, rollback/CAS, migration backup and removal. **Observed read-only:** one historical
candidate (receipt19), seven excluded Unknown conversations; live data unchanged. **Unverified:**
installed-Claude recovery/resume and actual-window interaction. Zero Claude prompts.

**Measured offline:** 2,680 tests passed, 13 ignored, zero failures across 48 workspace
suite results. Strict workspace/all-target Clippy, formatting and diff checks passed. Checked
source manifest SHA256: `4ba8b18ed420bd247d4ec02bb404efcf7ba22a5876568d2e0e14bf0b92dace93`.
All five prompt-budget files stayed byte-identical; zero Claude prompts. Evidence:
`dist/cli-stop-recovery/summary.json`.

Release build and isolated native-engine startup probe passed with the checked source
unchanged. Quit the old app and run `scripts/run.sh`, then choose **Reconnect Claude Code…**
in the eligible stopped conversation. No live recovery has been performed yet. Evidence:
`dist/cli-stop-recovery/release-build.json`. Built binary:
`/home/jason/coding/pipkin/dist/source-setup/builds/ef6e412c1cc64e14860717c31fee0d9d/bin/pipkin`.
