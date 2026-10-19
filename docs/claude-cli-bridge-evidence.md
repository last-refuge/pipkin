# Local Claude CLI bridge — qualification and implementation evidence

Qualification checkpoint: 2026-10-15. Status: **experimental chat/Write-only implementation
and scoped live/native testing complete; normal owned-desktop hosting is now default-on**.

### Daily-use integration follow-up

The user subsequently requested ordinary Pipkin integration enabled by default. Owned child and
in-process desktop hosts now enable chat/Write hosting without experimental launch flags.
`--disable-local-claude-cli` disables both; `--disable-local-claude-cli-write` keeps chat only.
Demo stays disabled, attached engines cannot be overridden, and standalone server/library
configuration remains opt-in. Explicit path trust, exact version policy, owner readiness, per-Write
review and all no-replay boundaries are unchanged. Existing conversations retain their backends.
The ten-run ledger below remains historical and exhausted; this change uses no additional inference.
Default-off statements in the qualification chronology describe that earlier checkpoint.

Full163 passed workspace/strict-Clippy/fmt/diff/guard/build gates after updating the obsolete UI
label assertion exposed by160. Two fixture timeouts in161 did not recur in three focused repeats
apiece or full163; their cause is not established. Package162 built the release candidate.
Job164 probed and installed it through the normal versioned installer, retaining the prior binary
unchanged and leaving the running app/profile alone. Seven installed argument-policy cases passed;
the ordinary desktop launcher needs no experimental flags. Installed mock-provider workflow166
passed migration/coding/restart/archive/replay checks (165 was an incorrect fixture-path invocation,
failing before worker startup). `daily-install-observation.json` records executable/hash and
unchanged budget. This is an offline installed-package check, not another live CLI qualification.
[Opt-in testing guide](local-claude-cli.md) describes the qualified slice and remaining limits.
See [implementation plan](claude-cli-bridge-plan.md). No subscription/billing claim is established.

## Phase 5 scoped qualification: installed CLI 2.1.299

The user authorized at most six short scratch-project live CLI runs plus metadata/isolation
checks and guarded owned-window desktop checks, then explicitly added four runs after job128.
**10/10 attempts consumed; no live allowance remains** in
`fixtures/claude-cli-probe/2.1.299/runtime-budget.txt`. Native fresh/resumed chat and Write
completed. The fresh Write full-review interaction FAILED; resumed Write separately passed
exact copied-review validation before Allow (see below). No production
activation, installation/binary modification, credential extraction or billing change.

Job111 correctly refused the upgraded installed 2.1.299 CLI under the old exact 2.1.296
pin, before auth/inference. The source now pins the **2.1.299 candidate**, not arbitrary
versions. Job115 passed metadata/auth and exercised startup/MCP isolation controls, plus
public-runtime readiness with no inference. Managed-policy isolation remains unverified.

Live job117's fresh chat became Unknown, retained partial text, and did not resume. Live
job121's new Write-deny case and job123's new Stop case also became Unknown before reviews
or decisions. Their closed diagnostics expose a message-delta schema incompatibility;
job123 specifically observes object-valued `context_management` rejected by a null-only
parser. These are **failed qualifications**, not successful deny/Stop coverage. There were
no native provider calls in the qualification runtimes; scratch canaries stayed clear.

Job118's synthetic published-schema regression failed before the nullable-metadata fix.
Job120 passed full workspace/strict Clippy/fmt/diff; job122 passed scoped CLI/runtime/store/
example tests and strict Clippy for closed diagnostics. Follow-on context-management parsing
accepts only the published closed numeric telemetry and discards it without events, native
context/compaction or authority. Jobs124 and126 passed full workspace/strict Clippy/fmt/diff
and rebuilt the qualification example. The example exercises the library actor/helper in
its own image, not yet desktop packaging.

Job125 reached an exact Write review, consumed one Allow, and independently observed exact
requested scratch bytes. CLI terminal success/exit0 were recorded, but omitted tool-result
`is_error` left the reported action Unknown, blocking completion/resume. No old receipt was
upgraded. Job126 adds the documented omission-default (reported success, **not action proof**),
with explicit null/malformed/missing-result uncertainty retained and full-runtime regressions.
The later native146 cases add completed chat fresh/resume and Write fresh evidence; Write
resume and corrected full-review validation subsequently passed in native158.

Jobs127/128 passed new-scope Deny/Stop controls: Denied/Revoked receipts, no expected scratch
file effect observed, positive owned-root cancellation cleanup, clear canaries and zero native
inference. Deny's run remains Unknown; explicit Stop is Interrupted. Neither implies complete
descendant containment. The original six-run budget was exhausted; the user explicitly
added four runs (ten cumulative) under the same restrictions. Job129 passed default-off,
process-local experimental host opt-in wiring through child/in-process/server startup;
these switches do not provide trust, readiness or approval. No general activation occurred.

Owned-window jobs130/138 used private app/engine data, same-workspace guarded input and
window-only captures/OCR. The initial Tab failure occurred before inspection; job137's full
workspace/strict-Clippy/fmt/diff/build gates passed the fix (UI189), including modal traversal,
composition preservation and independent full-argument scrolling. Native138 confirmed separate
path entry, then exposed focus stranded on the newly disabled Inspect button. Its neutral-focus
handoff and async-expiry focus-repair fixes passed full jobs139/142 (UI190). Native143 confirmed
post-Inspect navigation, but found trust toggled twice by key-down plus GPUI's key-up click.
Job144 reproduced the paired-event failure; capture-phase single-activation fixes passed
full gates at145 (UI192, guard37). Those earlier windows performed no
Check/Create/prompt/decision. Native138's stranded Quit
did not exit; the owned job was cancelled after a zero-bindings/prompts check. Native143 exited
cleanly (code0). `native-navigation-observation.json` retains these distinctions.
Native146 used slots7/8 for completed fresh/resumed chat with the exact remembered reply,
clean supervision and zero native runs (`native-chat-summary.json`). Slot9 fresh Write also
Completed with one Consumed Allow and exact expected regular-file bytes. **Its full-review
interaction failed:** the arguments handle was not a tab stop, so counted navigation activated
Allow before the intended Copy/validation. No decision was replayed and no clipboard contents
were retained. Separate observation files preserve this distinction; do not call it a review pass.
Job147 reproduces actual Tab unreachability. Explicit tab-stop and inert complete-review copy
shortcut fixes passed full jobs149/156 (UI193, guard37). Scoped157 additionally checks viewport
bounds. Native158 then used the final authorized run.
Job148 failed an unrelated exporter-health assertion. Twelve focused repetitions and the full
job149 rerun passed; the intermittent failure's cause is not established. The assertion now
reports only its enabled/state fields.
Restart checks150–155 used no inference. Full-window OCR missed the palette; owned-client
Wayland delivery and temporary closed-field focus/geometry diagnostics showed keyboard handling
worked, and tightly cropped OCR recognized the query. Reapproval remained disabled by the retained
older uncertain creation, not a keyboard failure. The temporary diagnostics were removed before
full156, which passed. Jobs150/151 were cancelled;153/155 exited0. These observations do not
count as visual inspection or accessibility qualification.
Native146 exited code0. A separate earlier metadata-only Create rejection after delayed Check
was explicitly looked up and parked, not replayed; setup guidance now distinguishes the 30-second
preflight freshness limit from the five-minute ticket. Prompt creation in a separate promptly
checked scope succeeded; the uncertain metadata identity remains retained.

The compositor reused an exited window address, so the input guard now additionally checks
pinned PID (37 stub tests passed). AT-SPI bus connection failed; real accessibility/IME remain
unverified, with no service or desktop configuration changes.

Job113 also exposed a crash-test barrier that waited for *any* final block rather than both
text blocks before SIGKILL. The barrier now waits for the exact durable text boundary; job114
full gates passed. Job119 later had an unexplained fixture metadata-spawn failure with an old
generic error. Redacted OS categories and a deterministic busy-executable/no-retry test improve
future diagnosis; job120 passed, but that does not prove the historical failure's cause.

See `fixtures/claude-cli-probe/2.1.299/README.md` and its immutable per-case JSON summaries.
Billing, managed policy and crash containment remain unqualified. Native146/158 exercise the built
desktop app/worker/helper, not an installed release package, real IME or accessibility qualification.
Unknown attempts consume budget and are never resent/resumed.

### Final native resumed Write (job158, attempt10)

The retained older uncertain creation was explicitly looked up and parked without replaying Create.
The independently Completed Write conversation was explicitly reapproved through saved hints,
Inspect, both trust choices, Check and Return. Before submission, its context was valid and the new
scratch target absent; reservation10 appended to the original ledger immediately before one prompt.

`native-write-resume-review.json` records exact complete escaped review comparison through
Ctrl+Shift+C **before any decision**, then Tab/PageDown/Space without a decision/effect and an exact
second comparison. A subsequent explicit Allow resulted in one consumed receipt.
`native-write-resume-observation.json` independently records Completed Resume, terminal success,
finished projection, delivered input, reaped root, drained pipes and exit0, with no supervisor or
parser failure. The new regular file contains exactly the expected bytes. Native runs remain zero,
configuration canaries clear and context valid. No prompt or decision was replayed.

After explicit Refresh workspace changes, inspector-only cropped OCR recognized both scratch
filenames (`native-write-changes-observation.json`). This is captured-project Git observation, not
attribution. Native158 exited0 after guarded Quit. Short arguments fit: actual native Tab navigation
and inert PageDown were exercised, but **nonzero native argument overflow scrolling is unmeasured**;
long-body independent scrolling is headless-tested. Full-window OCR had missed the palette, so
cropped OCR was used; no visual-inspection or accessibility pass is claimed.

The historical fresh Write premature-approval failure remains a failure. The final resume success
does not reclassify it or any old Unknown run. No further live runs are authorized.

### Final regression and documentation gate (job159)

Job159 exited0: `cargo test --workspace`, strict all-target Clippy (`-D warnings`), formatting,
`git diff --check`, guard37, desktop build and qualification-example build. UI193 passed,
including exact Copy, expiry, actual Tab reachability, independent long-argument scrolling and
viewport coverage. Temporary focus diagnostics are absent. No further live run was used.
`native-final-summary.json` rechecks the original ten-entry budget, four Completed desktop runs,
zero native runs/pending permissions, exact scratch files, clear canaries and closed owned windows;
it includes SHA-256 hashes of the per-case artifacts. Release instructions, supported-version
policy and limitations are in `docs/local-claude-cli.md`. Changes remain uncommitted.

## Offline Phase 4 recovery, assistance and crash checkpoint (job 110)

That checkpoint's full baseline: **job 110**, protocol **39**, engine schema **25**, desktop schema **10**.
Workspace tests, strict all-target Clippy, formatting and diff checks passed; UI187 passed.
Job108 also passed focused approval-port11/setup28, creation/handoff E2E3, and coding crash2
(including its child fixture) tests. Job110 first passed Git workspace19 tests. All tests used
owned private `/home` temporary directories, ordinary parallelism and fake CLI/account fixtures;
no new live inference/account request or native desktop input occurred.

- Approval attempts are committed before any decision RPC. Exact first acknowledgments recheck
  captured identity, live run/attachment, eligibility and expiry; failed/stale/duplicate/retired
  acknowledgments cannot release or rearm a decision. Terminal receipt metadata is saved
  separately; save failure produces a captured-scope notice and leaves conservative disk state.
- Startup restores only retired plaintext-free metadata, exact external identity and desktop
  operation high water. No grant, original-owner connection proof, review body or request returns.
  Invalid/unreadable/over-bound data disables new scopes. Eight context slots/128 attempts per
  run persist by namespace, without uncertainty eviction. Known same-conversation scopes can
  advance to separately verified later runs. This conservative capacity is a documented limit.
- Fake app+engine restart tests cover both an unknown lost-Allow reply and an explicitly resolved
  Consumed receipt. No automatic lookup/relaunch, cross-owner lookup or second Allow/Deny occurs.
  Known receipt metadata does not regress when lookup lacks original-owner proof. Such unknown
  outcomes may remain unresolvable. Writer-failure tests send no decision and preserve Stop;
  a killed writer fixture retains committed uncertainty before any RPC.
- Captured fresh setup copies only saved executable/project/model/mode hints. It rejects busy,
  outcome-unknown, stale, read-only and malformed sources. No prompt, draft, transcript, goal,
  approval authority or session UUID transfers. Each next setup step remains explicit. Fake
  integration verifies a distinct CLI UUID, unchanged source draft, empty destination and zero
  inference/CLI launches. Redacted diagnostics are explicit Copy only, with no probes or raw
  errors, account data, IDs, paths, prompts or argument bodies.
- Real SIGKILL of an owned fake coding engine is tested during pending review and after Allow
  consumption/delivery but before the effect. Restart retains partial text, records Unknown,
  invalidates resume context, revokes Pending and retains Consumed, with no replay/native calls.
  **Containment is not passed:** the consumed fake can still finish its authorized action after
  engine death. Test-only captured pidfd cleanup is not a production orphan reaper. See
  `crates/pipkin-app/CLI_RECOVERY_HARDENING.md` for the measured limitation and operator guidance.
- Job104 found a test-harness missing type qualification; job105 passed scoped recovery gates.
  Job106 passed tests/Clippy but formatting failed because follow-on handoff edits occurred during
  the run; it is not a full-green baseline. Job107 caught a test comparison against a type without
  PartialEq; corrected. Job108 passed the focused gates. Job109 exposed a real Git observation
  issue: assigning a copied index a fresh timestamp could bypass racy-clean detection for rapid
  same-size edits. The copy now preserves source mtime; a deterministic regression checks both
  timestamp/content observation and an untouched source index. Job110 passes all final gates.

The offline Phase 4 slice is implemented. Phase 5 still requires explicitly authorized installed
CLI/helper/managed-policy and real native desktop qualification, plus billing observation and
release opt-in wiring/documentation. Crash containment remains unqualified; production stays off.
No additional live account request is authorized by these offline results.

## Earlier durable uncertain creation recovery checkpoint (job 103)

That checkpoint's full baseline: **job 103**, protocol **39**, engine schema **25**, desktop schema **9**.
Focused core setup/recovery tests (25), storage recovery tests (4, including its child fixture),
and restart/failure E2E tests (2) passed, followed by the full workspace, strict all-target
Clippy, formatting and diff checks; UI186 passed. Tests used an owned private `/home` TMPDIR
and ordinary parallelism. No live inference/account request or native desktop input occurred.

- Create/RepeatCreate journal the exact public session/setup identity before releasing any RPC.
  Only the first exact durable acknowledgment may release it, with captured generation/op,
  ticket/root/target/mode/trust and current eligibility rechecked. Early backend completions,
  stale/duplicate acknowledgments, write failure and retired scopes cannot release Create.
- Desktop startup restores at most eight parked identities plus one active unknown identity.
  It restores no trust, ticket, grant, pending request, or automatic lookup/recreation. Invalid,
  unreadable, conflicting or over-capacity records disable creation rather than becoming empty.
  SQL bounds text before decoding corrupt/oversized values. Records contain no arguments or prompts.
- Exact receipt-plus-catalog reconciliation retires only the matching pair. Failed retirement
  leaves a conservative read-only record. Recovery selection can swap the active identity at
  capacity without evicting any record. UI warns that selecting recovery retires the previous
  setup approval and can interrupt a run using it; lookup remains a separate explicit action.
- The integrated fake test loses the Create reply, restarts both app harness and engine, then
  recovers through explicit read-only lookup: one Create, zero automatic lookups/probes/launches,
  no restored grant and no native inference. A separate injected journal failure sends no Create
  and does not release it when storage recovers.
- A killed storage-writer fixture proves the committed identity survives before any Create RPC.
  This is **not** CLI-child/orphan containment or native desktop crash qualification. Namespace
  isolation, conflicting duplicates, exact deletion, full capacity, corrupt rows and failure
  acknowledgments are also covered.
- Job100 passed scoped setup/UI/coding regressions but failed strict Clippy on an integer-bound
  style check. Job101 exposed the new test's draft-failure injector not covering the creation
  writer; creation writes now honor both injected failure controls. Job102 passed full gates.
  Job103 additionally verifies pre-decode bounds and exact captured ticket/root/target checks.

At that checkpoint Phase 4 still required persistent **approval** uncertainty recovery, actionable redacted
diagnostics, safe fresh-context handoffs and broader crash/orphan/storage-failure hardening.
Do not weaken live-owner approval receipt isolation for archived lookup. Installed helper,
managed-policy, live/billing and real desktop qualification remain open. Production stays off;
no additional live account request is authorized.

## Earlier Changes observation and cancellation hardening checkpoint (job 99)

That checkpoint's full baseline: **job 99**, protocol 39/schema 25. Workspace tests, strict all-target
Clippy, formatting and diff checks passed; UI186 passed. Dedicated cancellation tests (4)
and coding actor tests (17) also passed before the full run. Private `/home` TMPDIR, ordinary
parallelism and cleanup of only owned fixtures were retained. No live inference or native input.

- CLI Changes now observe the captured project, not fabricated native edit paths. Scan results
  bind connection/epoch, conversation/generation, session/cwd, scan operation, scope kind and
  invalidation revision. Old/replaced results cannot publish or release newer scans. External
  action observations and settlement invalidate; work stays off render and coalesced.
- The inspector labels project Git state as **not proof of CLI actions**. A fake coding E2E
  verifies an allowed Write appears alongside unrelated project work, without native tools,
  and committing then explicitly refreshing clears the observation.
- Git content comparison now uses a private gitdir/config/index with captured HEAD/object
  store. Repository/global executable filters, textconv and external diff drivers are not
  activated; fsmonitor is disabled. Source ignore rules are retained through name-only listing.
  Bounded stdout/stderr are drained concurrently; index copies are bounded and regular-file
  checked. Tests cover driver traps, unchanged source index, SHA-256/unborn repositories,
  linked/nested worktrees and symlink-index rejection. Custom filter/config semantics are
  deliberately not reproduced; this is not a filesystem sandbox or complete effects audit.
  Details: `crates/pipkin-app/CLI_WORKSPACE_OBSERVATION.md`.
- Job 95 passed Git observer tests/Clippy; job 96 passed native adapter, Git and inspector
  scoped regressions/Clippy. Job 97 passed the app/core portion but found a native queued-prompt
  cancellation race: an outer select could check cancellation, then resume admission after
  its session lock became available without rechecking the token.
- Added a request-local cancellation scope checked inside native prompt/enqueue and external
  admission transactions. Cancellation is a domain error, never a storage fence. A deterministic
  regression manually polls the lock wait, cancels, then resumes admission without an outer
  cancellation poll; no prompt/steer/follow-up is journaled and the session remains usable.
- Job 98 passed native cancellation but correctly exposed the CLI owned preflight/admission
  task losing request-local context. That task now explicitly captures the scope; execution
  spawned after durable admission deliberately does not inherit it. The new public coding
  regression proves no launch/receipt/native call and no context/grant poisoning. Job 99 passes
  both regressions and all workspace gates; no retry workaround was added.

At that checkpoint Phase 4 still required durable uncertain creation/approval recovery, actionable diagnostics,
safe fresh-context handoffs and crash/orphan/storage-failure hardening remain. Installed helper,
managed-policy, live/billing and native desktop qualification are separate pending gates.
Production remains disabled; no additional live account request is authorized.

## Earlier current-grant coding and integrated fake actor checkpoint (job 94)

That checkpoint's full baseline: **job 94**, protocol 39/schema 25. Workspace tests, strict all-target
Clippy, formatting and diff checks passed using a private `/home` TMPDIR, normal parallelism
and cleanup of only that job's directory. No installed CLI inference/account request or native
desktop input was performed. Subsequent edits clarify comments/documentation only.

- Added a separate default-off `EngineConfig.enable_local_claude_cli_write`. Linux coding
  admission requires this AND the master CLI switch; neither has a production switch.
  Metadata/profile identity alone grants nothing. Setup/reapproval and owner-filtered readiness
  use the applicable host gates and exact immutable policy binding.
- Current grants capture `LaunchPolicy`; fresh turn preflight checks that policy. Public coding
  admission captures the owner, expiry and **persistent grant cancellation token**, never the
  temporary Turn child. Common dispatch rechecks the exact current grant under the setup
  registry lock, preserving duplicate-first lookup and admission/binding/supervision checks.
  Expected in-transaction consent/freshness rejection is a domain error value, not a storage
  fault that fences the session.
- Added owned `Feedback::PullOnly` actor transport. The server owns its Control and public weak
  registration without observer channels or detached drain tasks. Existing observed test paths
  remain supported. Listener/supervisor teardown and positive revocation evidence stay coupled.
- New `runtime/local_cli_coding_runtime_tests.rs` proves all four host-gate combinations,
  metadata without authority, public fresh/resume, duplicate no-relaunch, persistent grant
  lifetime after Turn drop, owner filtering/reapproval and old-grant revocation. Extended coding
  tests exercise pull-only Allow/Deny/Stop/revoke and tracked cleanup. All 16 coding tests passed
  job 91. The correct permission-server filter ran **11 tests** successfully in job 93.
- New `adapters/native/tests/cli_coding_e2e.rs` drives pure core → native adapter → embedded
  engine → fake subprocess/private socket → public approval APIs. It proves full captured
  review, Allow once, clean resume, Deny/Stop without effect or retry, and terminal receipt lookup.
  A deliberately lost Allow response leaves sticky Unknown; attempted Allow/Deny resends emit
  **no second decision RPC**, and only explicit read-only lookup resolves Consumed. The actor
  launches once in that case. No native provider request/tool card is produced.
- Job 90 had one fake-executable spawn failure (cause unresolved); job 91 passed the scoped
  rerun and strict Clippy. Job 92 caught a test-only missing Tokio dependency; the fixture now
  uses bounded standard-library socket I/O instead, with no dependency addition. Jobs 93/94
  passed subsequent gates. No automatic execution/decision retry was introduced.

This closes the initial Write-only permission/UI/admission implementation, not production or
broader Edit/Bash support. GPUI headless tests and integrated core/adapter tests are separate;
no full native desktop input run is claimed. At that checkpoint captured Changes invalidation
was pending (now implemented above). Persistent uncertain recovery, diagnostics/handoffs,
crash/orphan hardening and applicable installed helper/managed
policy/live/billing/desktop qualification remain. No further live inference is authorized.

## Earlier captured GPUI approval panel checkpoint (offline/headless only)

Current versions remain **protocol 39 / schema 25**. Job **89** passed full workspace tests,
strict all-target Clippy, formatting and diff checks. Job 88 also passed before the final
viewport bounds, recovery warning and inert full-text fixture changes. Job 89 was that
checkpoint's full baseline, superseded above by job 94. Private `/home` TMPDIR fixtures were cleaned; normal parallelism retained.

- Added `pipkin-ui/src/shell/cli_approvals.rs`: coding-conversation header/palette entry, captured
  conversation/attachment/model-op dialog, explicit Refresh, complete inert Write review,
  Allow once / Deny, explicit read-only receipt lookup and reattachment. Opening, rendering,
  ordinary dialog Enter, closing and reattachment send no approval RPC automatically.
- Full escaped display is a cached plain-text entity prepared outside render, with scrolling
  and exact full-text Copy. No markdown/parser, native question/tool card, history/context,
  diagnostic or persistence projection. Unchanged clock updates do not clone the argument
  body. Closing/switching overlays clears the view cache without resetting core attempts.
- Buttons use shared core availability and captured callback identities. Per-review widget
  identities include attachment/run/invocation/digest rather than row index/action label.
  Changing selection does not redirect approvals. Core panel/reattachment/Stop helpers reject
  wrong selection/generation/op and make current-attachment reattachment a no-op.
- Core marks a decision attempted before effects. Unknown decisions survive refresh/close/
  reattachment and permit only explicit read-only recovery, never resend. Clock regression
  retires authority permanently but permits existing-scope receipt reattachment/lookup.
  Consumed is labelled NOT proof of delivery or action completion.
- Stop still uses the independent, ordered native control path, not the approval data RPC
  worker. The panel checks its captured root/attachment/model op before ordinary Stop; losing
  approval API capability does not disable Stop. The dialog disclaims undo, sandbox/universal
  mediation, treats arguments as untrusted text and states billing/restart-recovery limits.
- Six approval UI tests (four headless GPUI, two pure presentation/identity tests) cover full
  32KiB inert text and exact Copy, no open/render/Enter effects, cache clearing on overlay
  changes, captured decisions after selection changes, uncertainty across close/lookup,
  expiry/attachment callback retirement, clock-failure read-only reattachment, unique widget
  scopes and honest receipt/palette labels. UI suite: 185 passed. The core port suite now has
  six tests, including Stop independence and no live-scope reattachment. Earlier native-wire,
  broker/actor/permission, setup and native-isolation regressions pass in the workspace run.
- Job 86 passed scoped core/UI tests then Clippy caught an unused test import; removed it.
  Job 87 passed core tests then compilation caught use of private `now` instead of `now()`;
  fixed. Jobs 88/89 subsequently passed all gates.

At job 89 this was not an integrated coding actor flow: current-grant public coding admission
was still missing (now implemented in the job 94 checkpoint above). Next derive actor owner/expiry/revocation under the setup registry
from the exact current grant, using its persistent cancellation token (not the temporary Turn
child), retain admission rechecks, and integrate fake actor tests. Existing private coding
transport still requires test observer/control channels; public pull-only use needs a deliberate
owned path, not detached draining/retry work. Recovery/Changes/diagnostics and live/helper/
managed-policy/desktop qualification remain. No live Claude/account request or native input
was performed. Headless tests do not qualify actual focus, IME, accessibility or scroll behavior.

## Earlier approval command/event and native RPC checkpoint (job 85)

Current versions remain **protocol 39 / schema 25**. Job **85** passed five focused core-port
tests, full workspace tests, strict all-target Clippy, formatting and diff checks. Job 84 also
passed full gates before the final clock-regression receipt-reattachment fix. Job 85 is the
full baseline for this earlier checkpoint. Private `/home` TMPDIR fixtures were cleaned; normal parallelism retained.

- Added `cli_approval_port.rs`: dedicated Discover/Act/Reattach commands, backend requests,
  boxed events and API-capability lifecycle wiring. Discovery captures the actual conversation,
  attachment generation/model op and independent RPC op in a bounded pending map. Both outer
  and payload tuples are checked; independent discoveries may complete in reverse order.
- AppState centralizes discovery/decision/lookup availability. Only authoritative running coding
  contexts may discover/decide; retired attempted receipts may be looked up explicitly without
  a live model run. Reattach is read-only preparation and never restores decision authority,
  including after clock regression. Capability changes/events never emit approval requests.
- Native `Worker::Run` retains CLI prompt receipt proof from `Admitted.run`, pinned to connection,
  epoch and engine session. Missing proof can be established from prompt Lookup; an existing
  proof is not replaced by another receipt/connection. Discovery validates the returned prompt
  receipt; no selected-conversation or native-snapshot-run inference is used.
- Native RPC mapping is explicit and closed. It uses the existing connection on the non-GPUI
  worker, checks attachment/profile/readiness and captured receipt before discovery/decisions,
  and permits read-only receipt lookup after the model ends. Failures map to uncertainty; no
  decision retry, automatic lookup, prompt resend or replacement-connection redirection exists.
  Stop/Revoke retain the existing independent frontend control worker.
- Complete review bounds are checked before event delivery; boxed event, full string and vector
  capacities count against event admission. No review enters history, native questions/tools,
  model context, persistence, or error messages. Demo/offline paths explicitly fail closed.
- Five new core port tests cover selection-independent routing, attempt-before-effect,
  uncertainty through refresh/lookup, reverse/stale discovery, lifecycle/profile/capability
  loss, bounded pending queries and explicit read-only reattachment. Three fake-wire adapter
  tests cover missing/wrong prompt proof, exact decision/lookup mapping, uncertain calls with
  no retry, and replacement-owner connection refusal. A queue test covers full/spare payload
  capacity and box allocation. Earlier reducer/broker/actor/native-isolation regressions pass.
- Job 82 passed compile/core tests then caught a large command variant; its action was boxed.
  Job 83 passed scoped tests then caught growth of the generic event enum; the approval event
  was boxed, with explicit heap accounting. Jobs 84/85 passed all gates afterward.

Core and fake-protocol adapter flows are verified separately, not yet a complete
GPUI→core→adapter→coding actor permission flow. The thin GPUI controls are now implemented
above. Current-grant public coding admission and integrated fake-actor tests remain. Production remains disabled. No live Claude/account request or native desktop
qualification was performed. Recovery hardening, managed-policy/helper/live/desktop gates and
billing verification remain open. Contract: `crates/pipkin-core/CLI_APPROVAL_API.md`.

## Earlier immutable mode projection/profile checkpoint (job 81)

Versions at that checkpoint were **protocol 39 / schema 25**. Job **81** passed committed-snapshot lock-order
regressions, full workspace tests, strict all-target Clippy, formatting and diff checks. Jobs
85/89 above are newer. Tests used cleaned private `/home` TMPDIR fixtures with normal
parallelism; no live Claude/account requests or real desktop qualification were made.

- `SessionSummary.localCliMode` now projects immutable stored policy, independently of readiness.
  Native sessions have no CLI mode; unknown/missing CLI policy fails closed in the adapter.
  Stored mode is captured inside the committed snapshot without trust/session-registry locks,
  not conditionally in the ephemeral readiness overlay. A revision change after publication
  unlock therefore cannot erase mode and poison the adapter's immutable identity pin.
- Native adapter maps ChatOnly and RestrictedWrite explicitly to separate core profiles and
  refuses missing/unsupported mode at prompt admission. A later different mode is not an
  upgrade: it becomes Unsupported. Setup's chat opt-in still cannot create a coding grant or
  clear coding readiness. The runtime independently requires ChatOnly before clearing a block.
- Added `ExecutionProfile::LocalCliRestrictedWrite` and shared CLI policy helpers. Both CLI
  profiles accept only explicit nonempty attachment-free drafts, omit native model overrides,
  disable native tools/queues/goals/automation, and retain Stop after readiness loss. Approval
  lifecycle authority survives only the coding profile, never chat-only metadata.
- Setup/reapproval, fixed-model labels, native usage/billing suppression and thin UI capability
  tests now handle both profiles. Fake core→adapter→engine coding setup asserts the distinct
  profile while remaining blocked, with zero CLI inference launches and zero native requests.
- Expanded profile-policy tests across both modes; tested missing/null/unknown mode, every
  mode-change pair, snapshot/status/catalog mode projection, and chat-profile retirement of
  approval state. A bounded lock-order test verifies mode exists before readiness overlay.
- Job 78 passed workspace/all-target compile and core/native tests. Job 79 caught a test module
  name shadowing its policy import. Job 80 passed workspace tests, then caught an incorrect
  constant name in the concurrently added snapshot regression. Job 81 passed final gates.

This was identity projection, **not coding readiness/admission**. The guarded ports, receipt
retention, native RPC mapping and GPUI controls are now implemented above. Current-grant public
coding admission remains. See `crates/pipkin-core/CLI_APPROVAL_API.md`. Production remains disabled;
recovery hardening, live/helper/managed-policy/desktop qualification and billing remain open.

## Earlier pure-core approval reducer checkpoint (job 77)

Versions at that checkpoint were **protocol 38 / schema 25**. Job **77** passed the 12 focused reducer
tests, full workspace tests, all-target Clippy with warnings denied, formatting and diff checks.
Job 81 above is newer. This checkpoint used cleaned private `/home` TMPDIR fixtures and normal
parallelism. No live Claude/account request or native desktop qualification was performed.

- Added framework-free `pipkin-core/src/cli_approvals.rs`, owned by AppState: exact desktop
  conversation/attachment generation/model run op plus external epoch/session/binding
  generation/op/prompt identity, with independent monotonically allocated RPC ops.
- Decisions become attempted before returning a request description. Failed delivery,
  absent/Pending/Granted lookup and list refresh cannot make an attempted ID decidable again.
  Only explicit read-only lookup can resolve uncertainty; Consumed is not delivery or action
  proof. Stale tuples are ignored; malformed matching replies retire the scope atomically.
- Complete inert displays are bounded, never truncated; changed invocation identity/deadline/
  still-retained body, duplicate invocations, oversized lists and capacity overflow fail closed.
  Debug redacts arguments and approval IDs. Plaintext is retired on expiry/lifetime loss.
- Eight conversation slots, 128 seen invocations per run, 16 full reviews per list and 32 KiB
  per display. Unknown decisions cannot be evicted. Resolved sequential runs reuse a slot only
  with a newer external receipt/model op; old runs cannot be rebound. The eight-conversation
  lifetime bound and lack of restart persistence remain explicit integration limitations.
- AppState advances the injected clock and retires on disconnect and command/event lifecycle
  changes. Selection switching alone preserves the original conversation's reviews. Soft
  retirement keeps read-only lookup usable after run termination; attachment/ownership loss
  additionally invalidates pending lookup. Clock regression fails authority closed without
  starving subsequent read-only recovery.
- Tests cover both decisions, all tuple dimensions, duplicate/malformed payloads, full display
  limits, sticky uncertainty through refresh/lookup, expiry, clock regression, reattachment,
  capacity/no-eviction and sequential run reuse, plus AppState selection and lifecycle handling.
  Jobs 74/75 caught wrapper-name and test-helper compile errors; job 76 passed scoped core
  tests/Clippy, followed by job 77's full gates with the additional lifecycle tests.

This is a **staged reducer**, not an enabled frontend. Dedicated command/request/event routes,
feature negotiation, guarded initial run discovery, adapter DTO mapping/event-queue accounting,
GPUI full review/AllowOnce/Deny/lookup and coding readiness/current-grant admission
remain; the distinct profile is now implemented above. Lifecycle hooks do not grant authority from chat-only metadata. Integration contract:
`crates/pipkin-core/CLI_APPROVAL_API.md`. Production execution stays disabled; recovery, live
managed-policy/helper/desktop qualification and billing verification remain open.

## Earlier owner-scoped approval API checkpoint (job 73)

Versions at that checkpoint were **protocol 38 / schema 25**. Job **73** passed full workspace tests,
all-target Clippy with warnings denied, formatting and diff checks. Job 77 above is newer. Normal test parallelism and cleaned private `/home` TMPDIR fixtures were retained.

- Added closed approval run/ID/review/decision/receipt DTOs and `LocalCliApprovals`,
  `DecideLocalCliApproval` (control lane), and `LookupLocalCliApproval`. Unix Hello advertises
  API availability only. There is still no public coding launch route or execution grant.
- Owner is derived from the connection, never JSON. Pending reviews resolve through a bounded
  weak controller registry; decisions use the exact cached ticket with epoch/session/binding
  generation/op/prompt/invocation/digest checks. Raw arguments and IDs have redacted Debug;
  no CLI-private UUID, run nonce or account/config marker is in these DTOs.
- Read-only durable lookup survives controller retirement. Same-decision terminal duplicates
  acknowledge only a receipt and cannot re-enter delivery. Consumed is never action-completion
  or delivery proof. Postcommit cancellation/delivery failure remains uncertain and cannot
  authorize a resend; even Pending/absent lookup is not a retry grant.
- Coding bind installs one exact actor-owned weak registration before launch. Disconnect
  removes owner membership and cancels before registry unlock, rejecting late installation.
  Registry retirement does not synchronously wait for broker/publication persistence; tracked
  actor/listener cleanup seals before signals. The registry does not retain session demand.
- Fake Rust CLI/socket tests exercise the actual public RPC dispatch for AllowOnce/Deny,
  foreign-owner refusal, every identity dimension, duplicate/read-only recovery after actor
  retirement, postcommit cancellation without file effects, and disconnect/replacement-owner
  isolation. A publication-lock test verifies nonblocking registry retirement before owned
  durable cleanup. All native-provider/run/request/tool isolation regressions remain passing.
- Job 70 passed compile/protocol/existing actor checks. Job 71 caught a test checking the chat
  fixture's invocation filename rather than the coding fixture's launch marker; it was fixed.
  Job 72 passed the new focused coding tests and full workspace, then caught a needless-return
  Clippy lint. After that fix job 73 passed all gates.

API contract and frontend handoff: `docs/claude-cli-approval-api.md`. The reducer is now implemented
above; command/backend routing, frontend mapping/GPUI full review and AllowOnce/Deny/recovery controls, authoritative coding
profile/readiness and current-grant public admission remain pending. No live Claude/account
request was made. Production remains disabled; live/helper/managed-policy/native qualification,
recovery hardening and billing verification are not claimed.

## Earlier explicit mode setup checkpoint (job 69)

Versions at that checkpoint were **protocol 37 / schema 25**. Job **69** passed full workspace tests,
all-target Clippy with warnings denied, formatting and diff checks. A dedicated core → adapter
→ embedded-engine coding-setup test passed first. Job 73 above is newer; private
`/home` TMPDIR fixtures were cleaned and normal test parallelism was retained.

- `InspectLocalCli` captures `LocalCliMode::{ChatOnly,RestrictedWrite}`. Absent wire mode
  defaults only to ChatOnly; unknown modes are Unsupported and rejected before filesystem
  inspection, while null is rejected by decoding. Setup/status views echo the immutable mode.
- RestrictedWrite uses its distinct metadata preflight and creates its own immutable binding
  and CLI UUID. Neither direction of cross-policy existing-session reapproval is accepted.
  Creation acknowledgement recovery remains an exact lookup after ticket revocation.
- The public grant registry and public chat router independently refuse coding authority.
  Even with chat execution opted in, coding metadata verification/create/reapproval leaves
  the conversation blocked, with no prompt admission, CLI turn launch or native inference.
- Pure-core draft/Inspect/view mode identities are stale-guarded. A changed mode retires the
  prior setup and clears both trust choices; different-mode replies cannot verify a ticket.
  Adapter mismatched-mode inspections retire their returned tickets instead of adopting them.
- The thin setup UI offers Chat only and Restricted Write (setup only), displays the captured
  mode, disables mode changes for existing targets, and explicitly adopts saved mode hints
  only on user action. Coding is labelled execution-disabled, not sandboxed or universally
  mediated. Approval review/decision controls are still pending.
- Fake-process public API and end-to-end tests verify mode identity, no activation, refusal
  of cross-policy reapproval, read-only saved hints and exact creation recovery. No installed
  Claude or live account requests were made; native desktop qualification remains unverified.
- Job 66 caught remaining mode fields in test fixtures. Job 67 passed protocol/engine/core
  checks but used the wrong app test target (`--lib`; app is a binary). Job 68 passed scoped
  adapter/UI tests and strict Clippy, but formatting caught the new E2E test added during that
  job. Job 69 formatted and ran the new E2E plus all final gates successfully.

The owner-scoped API is now implemented above. Next: pure-core/thin-GPUI review controls, then
public coding admission derived from current grants. Production execution remains disabled.

## Earlier owned coding actor checkpoint (job 65)

Versions at that checkpoint were **protocol 36 / schema 25**. Job **65** passed the full workspace
suite, all-target Clippy with warnings denied, formatting and diff checks. It also passed
45 external-runtime/controller/listener tests first. Normal parallelism and a private
`/home` TMPDIR were used; that job directory was cleaned. Job 69 above is newer.

- The internal coding entry point now reserves the broker instead of the ordinary Running
  CAS, tracks filesystem binding before offload, builds the fixed Write command only after
  successful binding, and owns listener/process futures through cleanup and settlement.
  No public setup or request handler selects this entry point.
- Process `BeforeSignal` seals authority without cancelling the run itself. The supervisor
  also signals descendants after normal root exit: cancellation feedback there was incorrect.
  Completion/clean cancellation require explicit positive revocation evidence; completion also
  requires the retained seal to have no unresolved requests, a live lease, clean independent
  process evidence and accepted projection with known action-result status.
- Intentional consent revoke/expiry is distinct from protocol/delivery failure. Unexpected
  failure or unknown action results settle Unknown rather than manufacture Stop or leave a
  stuck active receipt. Failed endpoint binding or lost control receiver never launches a CLI.
- A successful peer can read and close immediately: its pending-disconnect watcher is retired
  before bytes escape. Delivery errors still fence and cannot retry. Sealing releases session
  demand, so a retained read-only control no longer blocks removal.
- Preflight freshness is checked after filesystem validation and command construction too;
  a deterministic synthetic-clock test covers expiry during validation.
- Rust fake-CLI subprocesses use real sockets, broker decisions, process supervision, receipts
  and action projection. Tests cover fresh/resume, duplicate lookup, owner mismatch, Stop,
  deny, grant revoke, unknown results, nonzero exit, disconnect, shutdown, setup/control loss,
  expiry before admission, refusal through the chat route and removal with a retained control.
  Fixture file effects occur only in their own temporary project after AllowOnce; native
  provider requests and native run/request/tool rows remain absent.
- Job **61** passed existing lifecycle regressions. Jobs **62/63** exposed incorrect normal-exit
  cancellation and completion handling; these were fixed, alongside the peer-close race found
  during review. Job **64** passed scoped actor/controller/preflight checks and strict Clippy.
  Job 65 then added removal/admission checks and reran the full workspace.

These are fake CLI/peer fixtures, not installed-Claude or actual desktop helper qualification.
No live Claude/account request was made. Production execution remains disabled. Next: public
connection-owned approval APIs/core/UI (mode setup is now implemented above), then remaining recovery and live gates.

## Earlier Write policy and action projection checkpoint (job 60)

Versions at that checkpoint were **protocol 36 / schema 25**. Job **60** passed `cargo test --workspace`,
`cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all --check`, and
`git diff --check`. Normal parallelism and a dedicated private `/home` TMPDIR were used;
that job directory was cleaned. This superseded job 57; job 65 above is newer.

- Added internal `write_policy.rs` and policy-specific preflight. Chat-only remains the public
  default. Coding metadata has its own immutable policy/config marker, requires `--settings`,
  and cannot accept a chat binding or use the no-tools launch path (or vice versa).
- The fixed Write command uses restricted/default permission mode, Write-only inventory,
  an ask rule and `PermissionRequest` hook with `onFailure: block`. It never uses safe-mode
  (which suppresses the hook), broad allowed tools, bypass/auto mode, or prompt text in argv.
  Hook arguments are POSIX-quoted; the nonce is environment-only. Linux helper image anchoring
  uses the running host's `/proc/<pid>/exe` metadata, not a replaceable project script.
  This is not live packaging, managed-policy, billing or crash/PID-reuse qualification.
- Added closed inert `ExternalBlock::Action` reports and receipt-scoped SQLite action anchors.
  Mixed text/action indices survive projection. Tool results require exact prompt/source ID;
  duplicate/orphan/type-change/budget violations roll back. Approval IDs are never guessed.
- Missing/unknown action outcomes cannot complete a receipt. Recovery retains reports,
  marks pending observations Unknown and never replays. Raw action APIs still reject
  receipt-backed runs. Schema 25 creates a v24 backup before supporting the new history.
- The native adapter renders bounded action notices, not native tool calls/results, approval
  controls or retry affordances. Action output never becomes native model context or prompt
  answer text. Decoder/SQLite tests verify input arguments are discarded.
- Job **58** passed 125 transport/preflight/policy tests and strict workspace Clippy. Job **59**
  passed storage, native transcript and protocol suites plus strict workspace Clippy.
  Job 60 then reran the full workspace, including cross-prompt action isolation and actual
  decoder-to-receipt projection fixtures.

No Claude/account request or actual Write tool execution occurred. Coding actor/BeforeSignal
wiring, public mode selection/approval protocol, core/UI approval state and remaining Phase 4–5
work are still open. Production execution remains disabled; chat-only actor behavior is unchanged.

## Earlier private IPC checkpoint (job 57)

Versions at that checkpoint were **protocol 35 / schema 24**. Job **57** passed the full workspace
suite, all-target Clippy with warnings denied, formatting and diff checks, using normal
parallelism and a private `/home` temporary directory cleaned afterward. This is the
full baseline at that point; job 60 above is newer. Production execution remains disabled.

- `runtime/external_permission_server.rs` binds one private Unix listener per reserved run,
  outside the controller's captured project. Duplicate/retired claims cannot recreate a
  listener. Ancestry/peer-UID checks do not imply hostile-same-UID containment.
- The socket only submits hook requests; owner decisions use a separate internal Control
  interface. Reviews follow durable Pending and transport registration. Reply delivery
  is one bounded nonblocking write inside the consumption/fence boundary, with no retries.
- Bounds: 16 concurrent connections, 128 accepted connections per run, a two-second total
  handshake, bounded frames/review queue, and captured request/lease deadlines. Queue
  pressure, malformed authenticated input, extra data, disconnect and retirement fail closed.
- Unpolled/dropped/aborted servers fence. Server and connection cleanup retain engine task
  tracker ownership. Binding performs filesystem work off rendering; an actor offloading
  that work must track it before offload and recheck its lease before any CLI launch.
- Job 57 passed **22 controller/listener tests** using real SQLite and private Unix sockets.
  Cases include owner/digest mismatch, consume-before-reply, deny, disconnect, extra data,
  wrong run nonce, full review queue, connection capacity, expiry/revocation, abort/drop,
  endpoint location and duplicate listener claims.
- The packaged headless helper additionally roundtrips allow/deny with a fake private peer,
  preserving original input and producing fixed JSON without startup or a file effect.
  This is separate from the controller/socket fixtures—not a live CLI or desktop coding test.
- Job **54** passed its 18 tests but caught a test-only `len_zero` lint; job **55** passed
  21 tests and strict workspace Clippy. Job **56** exposed an insufficiently private
  positive-test directory: the helper correctly refused it. The fixture now explicitly
  sets its own directory to 0700; production checks were not weakened. Job 57 reran all gates.

The listener is not instantiated by the chat-only actor. Coding launch policy/setup,
actor/supervisor integration, public approval APIs/core/UI, external-action projection and
later recovery/live qualification remain open. No live CLI/account request was made.

## Earlier durable controller checkpoint (job 53)

Current versions remain **protocol 35 / schema 24**. Job **53** passed the full workspace
suite, all-target Clippy with warnings denied, formatting and diff checks, using normal
parallelism and a private `/home` temporary directory cleaned afterward. It supersedes
job 48 as that checkpoint's full baseline; job 57 above is newer.

- `runtime/external_permissions.rs` composes the gate with real receipt-backed transactions:
  exclusive current-epoch/policy Running reservation, Pending before review, atomic decision
  plus consumption, post-commit monotonic expiry/revocation checks, owner-only views and
  read-only lost-acknowledgement lookup. Failed delivery and callback panic cannot reissue.
- Twelve controller fixtures cover foreign/stale/digest answers, duplicate reservation,
  no native provider requests/tool rows, commit/delivery failure, removal/drop, expiry,
  read-only consumed receipts, and revocation racing a committed decision before delivery.
  Cleanup retains unresolved evidence; failed revocation still cancels without confirmation.
- Removal exposed the intentional live-session guard in ordinary `Shared::commit` (job 50).
  A narrow publication-serialized receipt-scoped cleanup transaction now revokes permission
  metadata after removal/storage fencing without weakening ordinary session commits.
- The parser now accepts documented inert common metadata and suggestions. It never opens
  metadata paths or applies suggestions; non-default modes, subagents and unknown envelope
  fields deny. Duplicate-key rejection includes ignored nested metadata. Public source:
  [hook reference](https://code.claude.com/docs/en/hooks.md), read 2026-10-14. This is not
  installed-version/live qualification. No CLI/account request was made.
- Job **49** caught a reservation callback return-type mismatch, corrected before subsequent
  tests. Job **51** passed the then-current 11 controller tests and caught a collapsible-if
  lint. Job **52** passed 72 permission-filtered tests and strict workspace Clippy; job 53
  then reran the complete gates. New code has manual inspection, not a fresh independent
  subagent review (this conversation has reached its 64-subagent limit).

The controller is not instantiated by the chat-only executor. Actor-owned listener/lifetime
integration, action projection, coding policy/setup, public approval APIs/core/UI and live
qualification remain pending. No production coding activation is claimed.

## Earlier permission foundations checkpoint (job 48)

Versions: **protocol 35 / schema 24**. Job **48** passed `cargo test --workspace`,
`cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all --check`, and
`git diff --check`, with normal parallelism and a dedicated, subsequently removed `/home`
temporary directory. This was the foundation baseline, not coding activation.

- Added `execution/claude_cli/permission/`: bounded closed Write parsing, duplicate-key
  rejection, complete escaped inert review, full original-payload SHA-256, exact scoped
  one-shot decisions, persistence-before-delivery and permanent failure fencing.
  At this checkpoint the minimal five-field envelope rejected optional metadata;
  the later controller checkpoint adds documented inert metadata coverage.
- Schema 24 adds an immutable receipt-backed approval ledger and v23 backup migration.
  Exact repeat decisions are receipts, never repeat authorization. Consumption is durable
  before release and never means tool completion. Stop, settlement, epoch/startup recovery,
  and removal intent revoke unresolved authority. Removal rollback/failed-cleanup tests
  preserve revocation and do not claim already-consumed effects were undone.
- `permission_wire.rs` and `permission_helper.rs` add bounded private Unix IPC framing and
  early helper dispatch in both desktop/headless binary entry points, before logging/storage/GPUI.
  Nonces and full payload digests bind replies; unknown replies and timeout fail closed,
  with no reconnect. These are client/codec foundations: **no production listener exists**.
- Helper filesystem checks run in bounded metadata-only workers; late results cannot write.
  Stdout supports only pipes/sockets, with bounded classification and nonblocking delivery.
  Other descriptor types are rejected before bytes. Stdout failure needs the future hook's
  qualified `onFailure: block` configuration; exit 2 alone is not a PermissionRequest denial.
  Trusted root/same-UID ancestry is not malicious-same-UID isolation or a sandbox.
- Job **46** passed 47 selected permission tests, both packaged-helper tests and strict
  workspace Clippy. Reviews **61/62** found I/O deadline, Debug redaction and removal gaps;
  fixes and regressions are included in job 48. Job **47** passed tests/Clippy but caught
  formatting while the additional stdout-metadata hardening was being edited; job 48
  reran the complete sequence afterward. No follow-up independent review is claimed.

The broker controller is added above; actor/listener wiring, receipt-aware action projection,
coding policy/setup, protocol/core approval controls and UI are still pending. No live CLI/account request, billing verification, coding
capability, native UI qualification or crash/orphan guarantee is established. See the
[permission integration contract](claude-cli-permission-design.md).

## Earlier chat-only integration checkpoint (job 45)

Protocol **35** adds capability-aware clients and exact read-only setup/session lookup;
this checkpoint used schema **23**. The experimental public text-only Prompt/Stop dispatcher exists,
but `EngineConfig.enable_local_claude_cli` defaults false and no production switch is exposed.
Desktop setup, capability guards, and exact creation reconciliation are implemented.
Job **45** passed tests, all-target Clippy with warnings denied, formatting and diff checks,
superseding job 39 for chat-only integration. Job 48 above is the newer additive baseline.

- Job **40** passed core tests and all 19 protocol tests.
- Job **41** exposed a bad regression fixture attempting to change the immutable CLI model;
  it was corrected to change readiness instead, without weakening the database trigger.
- Job **42** passed 23 local-CLI setup/runtime/status tests, one snapshot lock regression,
  12 external-executor tests, and four execution-boundary tests. Fake executables only.
- Fresh execution requires the approving connection's grant; another live connection can
  read prior receipts but cannot probe or launch through that grant. RPC summary overlays
  also withhold readiness from unapproved connections. Duplicate receipts remain available
  after approval revocation or an execution-disabled restart.
- Snapshot volatile caches are captured before publication; ephemeral trust is read after
  publication is released. This avoids registry/publication inversions with removal and
  CLI admission. Native stored snapshot fields still use revision checks.
- At this historical baseline, approval expired **five minutes after Inspect**, not after
  Check or the latest prompt, stopping active work. The later [provider repairs](scorecard.md)
  remove that connection-lifetime coupling while retaining setup freshness, revocation and
  fencing of uncertain contexts. This does not retroactively qualify the newer behavior.
- Unknown creation is never automatically retried. Explicit bounded recovery parking keeps
  exact identities while permitting a new setup; restart persistence is still pending.
- The core→native-adapter→embedded-engine fake-CLI test verifies saved native model
  preferences do not enter CLI requests, visible external answers, explicit revoke/reapprove,
  exactly one fresh and one resumed launch, and zero native inference. Metadata-only setup
  also remains covered with the execution flag off.
- Reapproval advances the stored revision without serializing a global execution grant.
  A failed revision commit cancels/removes the exact new grant and marks setup failed.
- Review 54 found no blocking issue in its bounded follow-up of grant revision/rollback,
  snapshot locking, capability/model guards, exact-ticket cleanup, and recovery parking.
- Full job **43** failed under `/tmp` quota exhaustion; strict lint job **44** found a
  test-module ordering issue, corrected before job 45. Investigation reproduced OS
  `EDQUOT` and SQLite extended code 4874 (`SQLITE_IOERR_SHMSIZE`): filesystem free space
  was not the user's available quota. Test-fixture cleanup now tolerates a poisoned mutex
  so a primary failure does not cause a second panic/abort. Job 45 used a dedicated short
  temporary directory under `/home`, preserving parallel tests; it was removed afterward.
  No user files were deleted, quota changed, tests disabled, or production fallback added.

No new live CLI/account request, production activation, billing verification, real native
UI/IME/accessibility qualification, permission broker, or crash/orphan guarantee is claimed.

## Implemented scope

The standalone Rust example `crates/pipkin-engine/examples/claude_cli_probe.rs` and its
`claude_cli_probe/{isolation,permission,qualification,lifecycle,interruption}.rs`, shared
`src/execution/claude_cli/{process,stream}.rs`, and focused recovery tests support:

- Explicit executable opt-in, absolute-path/PATH discovery without a shell, version and
  help probes with deadlines. Current qualification target is exactly **2.1.296**;
  other versions fail closed before auth/inference. This is not a tested version range.
- An explicit `auth` mode asking the CLI for status, with only a fixed enum and login
  boolean emitted. Emails, organization IDs, config paths, raw status and stderr are
  excluded from JSON and Debug output. No CLI credential files/keychain tokens are read.
- A non-inference `isolation` mode using a disposable HOME/configuration and harmless
  hook/plugin/MCP marker controls. Positive controls must actually execute before
  suppression can pass; crashes, unreadable evidence, and early exits never prove isolation.
- Separate opt-in `permission-deny` / `permission-allow` modes using the current public
  `PermissionRequest` command-hook contract. They offer only Write (plus any CLI
  EndConversation tool), generate their own private project, and can authorize at most
  one exact file/content write. They are scripted qualification fixtures, **not a user
  approval UI or production permission broker**.
- An explicitly authorized `smoke` mode: one fixed no-tools prompt via stdin, fresh UUID
  or explicit resume UUID, no arbitrary CLI arguments/prompts, no retries or fallback.
  Both user-turn permission and executable/project trust are required at invocation.
- Same executable, environment snapshot, project and safety policy for auth preflight
  and smoke. Known routing override variables are reported by **name only** and block
  auth/smoke; they are not silently removed. CLI-reported `claude.ai` login is required.
- Concurrent bounded stdout/stderr pumps (8 queued 8-KiB chunks); per-line NDJSON limit
  256 KiB, probe stdout/stderr limits 128 KiB each, smoke stdout total limit 16 MiB.
  Stderr is counted but never retained. Timeouts: 10 seconds for probes; 60 for smoke.
- Unix owned-process-group supervision: TERM, grace (300 ms by default), KILL, and
  bounded reaping/draining. The root is observed with `waitid(WNOWAIT)` and kept unreaped
  until group signaling is finished, preventing PID/PGID reuse during cleanup. Pipe EOF
  does not skip the descendants' TERM grace. Future cancellation also kills the group.
- Bounded incremental NDJSON projection with UTF-8/JSON/type/session checks, sticky
  sanitized errors, unknown-event counters, terminal-result checks and fixed marker
  recognition. Partial/assistant counters are **not deduplicated transcript messages**.
- Smoke success requires matching init/result UUIDs, declared empty tool inventory,
  successful result subtype/flag, expected fixed answer, and clean process exit.
  The fresh turn supplies a memory code; resume asks for that code without resending it.
  Exit 0, an input flush, or arbitrary successful text alone is insufficient.

This code is an example target with tests enabled in `crates/pipkin-engine/Cargo.toml`.
It does not register an API adapter, modify runtime admission, migrate a database, offer
new desktop settings, or replace existing credentials. No new dependencies are required.

## Running the experiment

### No inference

```sh
cargo test -p pipkin-engine --example claude_cli_probe
cargo run -p pipkin-engine --example claude_cli_probe -- probe --trust-executable
```

Use `--executable /absolute/path/to/claude` when GUI PATH discovery is unsuitable.
Probe runs version/help in a private temporary directory, not the user's project.
No arguments or `--help` prints instructions and does not spawn Claude.

### Account lookup (separate opt-in)

```sh
cargo run -p pipkin-engine --example claude_cli_probe -- auth --trust-executable
```

This asks the CLI for status and may refresh its own login; it makes no model request.
Neither probe nor auth is a promise of plan-limit billing. Unexpected auth methods,
malformed output, configured alternative routing, or failed safety flags block smoke.

### Live qualification (permission required before running)

Use a dedicated scratch project, a newly generated UUID for the first turn, and the same
UUID/project/config for the second. Never retry/resume a timed-out or uncertain turn.

```sh
# Set SCRATCH to an absolute scratch-directory path and SESSION to a fresh UUID.
cargo run -p pipkin-engine --example claude_cli_probe -- smoke \
  --trust-executable --trust-project --allow-live-usage \
  --cwd "$SCRATCH" --session-id "$SESSION" --model sonnet
# Only after the first turn is confirmed successful:
cargo run -p pipkin-engine --example claude_cli_probe -- smoke \
  --trust-executable --trust-project --allow-live-usage \
  --cwd "$SCRATCH" --resume "$SESSION" --model sonnet
```

These calls **can consume included allowance or already-enabled extra usage**. The
harness does not change spending settings and cannot impose a reliable subscription
billing cap. Keep default CLI account configuration; do not replace its config directory
with an empty one and inadvertently change authentication. Do not run live mode in CI.

The fixed candidate flags are:

```text
--safe-mode --restricted
--strict-mcp-config --mcp-config {"mcpServers":{}}
--print --tools "" --disallowedTools mcp__*
--permission-mode dontAsk --permission-prompts none
--input-format text --output-format stream-json --verbose --include-partial-messages
--no-chrome --disable-slash-commands
--model <explicit alias> (--session-id <UUID> | --resume <UUID>)
```

`--bare` is intentionally absent: it disables OAuth/keychain auth. Empty
`--setting-sources` semantics were not established by the reviewed docs, so the candidate
uses documented `--restricted` instead. Managed policy/hooks still apply; these flags
are not an OS sandbox or proof that no startup code will execute. Their **combination
has now passed a two-turn live smoke check, but adversarial customization isolation and
permission hosting remain unverified**.

## Isolation and permission qualification

```sh
# No model requests; uses a disposable HOME/config with no copied credentials.
cargo run -p pipkin-engine --example claude_cli_probe -- isolation --trust-executable
```

The fixture has startup hooks in disposable user, shared-project, and local-project
settings, an explicit plugin with startup hooks, and a project MCP server that marks
its launch and exits. Fake user settings approve only that sentinel server, so MCP
positive controls do not silently stop at an approval gate. No real settings are edited.

Observed on CLI 2.1.296:

| Control/surface | Ordinary fixture control | Safe-mode policy | Restricted hook policy |
| --- | --- | --- | --- |
| `--init-only`: user/project/local hooks | All three executed | None executed | None executed |
| `--init-only`: explicitly supplied plugin | Executed | Suppressed | Executed (host-explicit plugin remains allowed) |
| `mcp list`: approved project MCP sentinel | Launched | Not launched | Not launched |

These are **startup/discovery observations**, not yet proof of all print-mode behavior,
installed-plugin configurations, or administrator policy. The live hook route never
passes `--plugin-dir`; it must not accept arbitrary extra CLI arguments. The initial MCP
control lacked explicit approval and did not run the sentinel; that unexercised result
was not treated as an isolation pass. Adding approval in the *fake* user profile calibrated
it before the successful comparison.

### Why qualify a hook instead of assuming an MCP wire contract?

The [archived official MCP example](https://web.archive.org/web/20250806012109/https://docs.anthropic.com/en/docs/claude-code/sdk#custom-permission-prompt-tool)
documents `tool_name`, `input`, optional `tool_use_id`, and a JSON-stringified allow/deny
payload inside MCP text content. That is historical evidence, not a version-pinned
2.1.296 contract. Current docs fully specify the `PermissionRequest` hook's stdin/stdout
schema, so this experiment uses that supported alternative. No SDK wire protocol is inferred.

The hook route is separately configured with `--restricted`, strict empty MCP configuration,
explicit `--settings`, `--permission-mode default`, and `--permission-prompts none`.
It does **not** use `--safe-mode` (which disables the hook) or `--bare` (which disables
subscription auth). `permissions.ask: ["Write"]` exercises the hook; earlier managed
permissions/hooks may still affect behavior and are not bypassed.

The fixture hook:

- Has a 5-second CLI timeout and `onFailure: "block"` (requires CLI 2.1.295+).
- Reads at most 64 KiB with its own 2-second stdin deadline; malformed input/timeouts
  return explicit deny JSON. Merely exiting 2 is **not** a PermissionRequest denial.
- Checks the expected session UUID, canonical scratch cwd, event, Write tool, exact target
  and exact content; no `tool_use_id` is assumed because this hook does not provide one.
- Records a validated-request marker separately from generic rejection, preventing a
  mismatched request from falsely qualifying the intended Write denial.
- For allow-once, atomically consumes a fixed private grant before answering. Prior denials,
  repeat grants, unexpected fields, symlinks and invalid audit state fail closed. It returns
  no `updatedInput` or `updatedPermissions`; no persistent permission rules are granted.
- Verifies the actual target's existence/content separately from the approval decision.
  Only `NotFound` establishes absence; permission/I/O errors mean uncertainty.

Live modes require **new explicit permission** before invocation. The earlier authorization
for two no-tools requests does not cover these Write tests. Each mode makes one model turn,
with no automatic retry; scratch files are owned by the fixture and removed afterwards.

```sh
# Only after approval; SESSION_DENY and SESSION_ALLOW must be distinct fresh UUIDs.
cargo run -p pipkin-engine --example claude_cli_probe -- permission-deny \
  --trust-executable --trust-project --allow-live-usage --session-id "$SESSION_DENY" --model sonnet
cargo run -p pipkin-engine --example claude_cli_probe -- permission-allow \
  --trust-executable --trust-project --allow-live-usage --session-id "$SESSION_ALLOW" --model sonnet
```

**Live permission results passed after new explicit authorization (two CLI turns):**

- Deny: the exact correlated Write reached the hook, explicit denial interrupted the
  turn, and the target was absent. CLI exit 1 and `result_success:false` are the expected
  denied-task outcome, **not** a model-success claim. Qualification passed; 2,852 ms.
- Allow once: the exact Write reached the hook, the single grant was consumed, and the
  target held exactly the specified content. CLI exit 0 and successful result; 4,552 ms.
- Both reported CLI-account auth, an inventory containing only Write, and no unexpected
  project-hook/MCP markers. The real user settings were not modified. Scratch files were
  fixture-owned and cleaned up. Times include cleanup grace and are not benchmarks.
- `fixtures/claude-cli-probe/2.1.296/permission-summary.json` retains only aggregate,
  sanitized observations. One CLI turn may contain multiple model exchanges; two turns
  were authorized and run, with no extra live retry or billing-setting change.

This establishes the basic hook round-trip on this version/account, not a general
person-facing approval broker. Controlled pending-approval cancellation is now measured
below; other live failure/timeout paths, managed precedence, other tools, and stale
approval across resumed runs remain unverified.

## Lifecycle and interruption qualification

The standalone example now has a private, versioned SQLite ledger (`lifecycle.rs`),
**not** a production store migration or desktop approval UI:

- Commits admission and launch intent before execution; `Running` means the process may
  execute, not that a remote request was acknowledged. WAL/FULL durability, explicit
  reopen/recovery, bounded busy waits and exact private schema checks.
- Approval identity is `(CLI session, generation, run, host request ID, argument fingerprint)`.
  The fingerprint binds only the validated, fixed scratch Write. No prompt or credential
  content is stored. Production conversation identity/routing is not wired yet.
- Decisions expire, cannot be replayed, and require atomic one-shot consumption. Linux
  parent/hook deadlines share BOOTTIME (including suspend); the helper also bounds its wait.
  A consumed grant is already issued and **cannot be undone** by Stop.
- Stop revokes Pending/Granted approvals and invalidates context before releasing the
  independent cancellation token. The supervisor also fences approvals before its first
  group signal on timeout, protocol/pipe/output failure, root-exit cleanup or future drop.
  Failed revocation still proceeds with cleanup, but cannot qualify a settled interruption.
  The synchronous fixture fence has a SQLite busy timeout, not a hard filesystem-I/O
  deadline; production worker supervision still needs separate design/qualification.
- Root reaping, pipe drainage, cleanup deadline and approval revocation are separate
  report facts. None establishes containment of escaped descendants.
- Recovery turns unfinished runs into Unknown, revokes unissued approvals, and blocks
  resume/queued delivery. It never starts a process, replays a tool, or resends a prompt.
  Interrupted/Unknown bindings require a **fresh-session, user-reviewed handoff**; no
  CLI transcript editing or assumed removal of stopped inputs.

**Offline measured:** 81 example tests pass. Real SIGKILL tests cut a synthetic executor
at eight durable boundaries: admission, launch intent, submission receipt, pending
approval, granted approval, consumed grant, external effect, and confirmed completion.
Reopening preserves effect/receipt counts without replay, rejects late replies/completion,
and permits resume/queue eligibility only for confirmed completion. Separate child-process
controls exercise actual hook waiting, allow/write calibration, pending cancellation,
absent-host expiry, and pre-signal ordering/fence failure. These are **not real CLI crash
recovery or print-mode timeout/disconnection evidence**.

New live mode (one turn, fresh UUID, generated scratch only):

```sh
# Requires fresh explicit live consent; never auto-retry or reuse an interrupted session.
cargo run -p pipkin-engine --example claude_cli_probe -- interrupt \
  --trust-executable --trust-project --allow-live-usage --session-id "$SESSION_INTERRUPT" --model sonnet
```

It waits for the exact pending Write, revokes it, attempts a stale allow/consume (both must
fail), stops the owned group, verifies no target/grant, reopens its ledger and checks that
continuation, late completion and queue eligibility remain blocked. The driver never
approves the Write.

**Authorized live interruption passed (one CLI turn):** exact pending request observed;
late allow/consume rejected; no target or grant; Write-only inventory matched the fresh
session. Stop reported Cancelled, root reaped, pipes drained, approval revocation confirmed,
and no cleanup timeout. The reopened binding remained Interrupted/invalid with resume,
queue delivery and late completion blocked. Elapsed 2,773 ms includes cleanup grace.
No terminal model result was projected; this is **interruption success, not model success**.
Sanitized evidence: `fixtures/claude-cli-probe/2.1.296/interruption-summary.json`.
The one-turn authorization is consumed; no live retry, resume, shell tool, real-project edit,
or billing-setting change was performed. Billing remains unverified.

The ledger is deleted with its private fixture. This demonstrates a recovery contract,
not durable production conversation storage. Manual `smoke --resume` remains a separate
transport experiment, not a safe-resume implementation.

## Observed / measured

- Installed executable discovery, `claude --version`, and `claude --help`: **2.1.296**.
- No-inference probe successfully resolved that CLI and found all required help flags;
  no known environment routing overrides were present. Redacted output:

  ```json
  {"mode":"probe","cli_version":"2.1.296","missing_flags":[],"routing_override_names":[],"billing_verified":false,"launch_policy_verified":false}
  ```

- **Earlier no-tools live check passed (two CLI turns):** the CLI reported a Claude-account
  login under the same safety arguments/environment used for smoke. Fresh Sonnet-alias
  request: expected marker, zero declared tools, explicit UUID matched, exit 0; 4,553 ms
  including supervisor cleanup grace. Resume: same UUID, remembered the original code
  without it appearing in the second prompt, zero declared tools, exit 0; 2,632 ms.
  Each run contained two well-formed unknown envelopes, counted but not interpreted.
  No API fallback, billing setting change, or extra CLI turn was made in that qualification.
- Sanitized aggregate observations: `fixtures/claude-cli-probe/2.1.296/live-summary.json`.
  This is **not a raw NDJSON replay fixture**: account/session identifiers and raw CLI
  content are not retained. Times are one sample each, not a latency benchmark.
- Synthetic decoder and shell-child tests cover arbitrary chunk/UTF-8 boundaries,
  size limits, malformed/truncated records, unknown events, duplicate/mismatched IDs,
  missing/error terminal results, marker recognition and redaction; cancellation before
  spawn/during execution, ignored TERM, pipe floods, simultaneous stdout/stderr, retained
  process identity, closed-pipe descendant cleanup grace, stdin-as-data, missing
  executable and nonzero exit after successful result.
- All test streams are **fabricated fixtures inline in the Rust tests**, not captured
  Claude responses. They establish local contract behavior only, not CLI parity.
- **Passed at Phase 0 completion:** `cargo test -p pipkin-engine --example claude_cli_probe` — 81 tests
  (transport: 33; isolation/permission slice: 58). New coverage includes correlated exact-request decisions,
  one-shot/concurrent grants, prior-denial rejection, symlink/foreign evidence refusal,
  permission stdin bounds/deadlines, opt-in modes, Write-only inventory, and false-pass
  prevention for failed isolation runs or metadata errors. Lifecycle coverage adds scoped
  deadlines/one-shot decisions, actual killed-host fault cuts, pending-approval interruption,
  pre-signal fencing, and cancellation racing with already-failed cleanup.
- **Passed:** calibrated non-inference `isolation --trust-executable` controls on 2.1.296;
  sanitized evidence: `fixtures/claude-cli-probe/2.1.296/isolation-summary.json`.
- **Passed:** `cargo test --workspace && cargo clippy --workspace --all-targets && cargo fmt --all --check` (exit 0). The previous planning-turn disk-I/O failure did not recur; its root cause was not established or repaired here.
- Read-only code review identified numeric-PGID reuse after reaping, descendants losing
  their grace after closing pipes, and insufficient smoke-answer validation. These were
  corrected with retained leader identity, explicit grace settlement, and fixed fresh/
  memory-recall markers; regression tests cover each change.
- The isolation/permission review caught three additional false-pass risks: a failed MCP
  invocation mistaken for suppression, an unrelated denied Write mistaken for the intended
  request, and metadata errors mistaken for target absence. Clean-run controls, validated
  exact-request evidence, and NotFound-only absence checks now cover those cases.
- Lifecycle review found cancellation could reach supervision before revocation, and
  autonomous failure cleanup had no fence. An independent token plus an owned pre-signal
  fence now covers those paths. A second regression ensures cancellation during already-
  failed cleanup cannot relabel timeout/protocol failure as Interrupted.
- A short-grace signal-order test initially failed when its foreground sleep deferred the
  shell's TERM trap. The ordering fixture now uses a builtin loop; eight repeat runs and
  the subsequent full workspace/test/Clippy/format gates passed. No additional live turn
  was used to diagnose or fix this test fixture.

## Phase 1 — storage and disabled-backend foundation

**Implemented, not an enabled executor.** Schema 21 adds immutable execution identity,
nonsecret CLI bindings and separate external run/action tables. Existing sessions, including
imports, migrate to NativeAgent with the existing backup/future-schema refusal behavior.
Unknown backend values remain readable but disabled. Protocol 32 deliberately rejects older
peer handshakes; old serialized summaries still default to NativeAgent.

External admission is idempotent but a duplicate does **not** authorize another launch.
State transitions compare `(session, generation, op, expected state)`. Startup atomically
marks unfinished external runs Unknown and invalidates their bindings, without replay.
Interrupted/Unknown outcomes fence the generation; stopping/unfinished work cannot admit a
follow-up. `context_valid` alone is not resume or execution authorization. Completion requires
caller-verified final result/root exit and rejects Observed/Running/Unknown action metadata.
Actual supervision, permission revocation and CLI-history reconciliation are not implemented
by this storage API.

Removal deletes readable history/binding/ledger records, not the CLI's own history. Minimal
permanent UUID ownership receipts remain in this store: a previously bound UUID cannot be
reintroduced as a fresh context after removal, including never-launched/completed bindings.
Paths are shape-validated only; no filesystem qualification, account probe or credential I/O
occurs in these constructors. CLI actions have explicit external provenance and never become
native `tool_calls` intents.

Engine RPC admission and native execution/recovery sinks refuse CLI/unknown backends, including
native-looking children of disabled roots. Saved compaction requests, pending native tool
calls, retry state and follow-ups cannot bypass this boundary. External sessions do not pin
or initialize native extensions. Core/adapter snapshot gating uses existing conversation and
generation guards, refuses operation-scoped context changes, and blocks delayed intent/queue/
goal delivery. History, rename, removal and creation of new native conversations remain usable.
The gate is coarse; full capability/status and external transcript projection remain future work.

**Offline measured:** protocol/core tests, 113 native adapter tests and four execution-boundary
runtime tests passed (job #27). Store-filtered tests passed: 67 tests. Integration fixtures
use Scripted providers, temporary stores and deliberately foreign native recovery records;
no installed CLI or live model is used. They cover saved compaction executor refusal, no native
tool/provider execution or target-file mutation on recovery, extension suppression, old-peer
refusal, readable history and continued native conversation execution.

**Passed (jobs #28 and #29, exit 0):** `cargo test --workspace`,
`cargo clippy --workspace --all-targets`, `cargo fmt --all --check` and `git diff --check`.
The workspace run includes the 81 Phase 0 example tests. No new live CLI request was used.

Read-only review found unsafe UUID reuse after removal, unresolved actions being stranded by
completion, and a recovery path checking only the child identity. Permanent receipts, atomic
completion refusal and an ancestor-aware recovery guard now have regression tests. The stale
protocol-31 test assertion was updated to 32. A bounded follow-up source review found no
remaining issue in those fixes. Native-only guards check identity independently of temporary
availability, so enabling an external executor later cannot enable native execution sinks.

No CLI create/select RPC, CLI runtime process, production approval UI or API fallback is enabled.
No additional live inference, billing change, credential extraction or installation was performed.
No new desktop/IME/accessibility validation is claimed.

## Phase 2 — shared transport foundation (runtime still disabled)

The qualified supervisor/summary decoder now live under
`crates/pipkin-engine/src/execution/claude_cli/` and are shared by the probe. The original
81 tests are preserved as 30 library and 51 example tests. A bounded, nonblocking stdout
observer fails closed on rejection/panic and still fences/signals/drains/reaps the owned group.
Synchronous callbacks must not block; the fixture SQLite revocation callback is still not a
production permission broker. Process groups are still not a sandbox or crash/orphan proof.

Typed preflight checks an explicitly approved executable/project candidate, exact version and
required flags, and the CLI's public auth status. It rejects routing/executable environment
overrides by presence before narrowing the environment; preserves selected HOME/config and
proxy/CA settings; never opens CLI credential files. Public account/organization metadata is
hashed into a nonsecret binding marker rather than logged. Candidate/config directory changes,
binding mismatch and expired preflight refuse launch. Filesystem rechecks are not a race-proof
sandbox or atomic lock against concurrent external login/configuration changes.

The only launch policy is currently **chat-only (`--tools ""`)**, not editing or general read-tool
support. Prompts are stdin data, sessions are explicit, and no arbitrary flags or API fallback
are accepted. The new pure decoder emits bounded, provisional external display events; it
reconciles partial/final text without appending the terminal answer twice. Tool observations
cannot become native tool calls. Strict new projection/auth schemas remain live-unverified.

**Passed:** 78 shared CLI module tests and 51 example tests (job #32); full workspace tests,
Clippy all targets, formatting and diff checks (job #33). A fake executable exercises actual
preflight/launch/supervision/projection, including wrong-session output, queue pressure, and
successful terminal output followed by a hung process. An initial integration assertion counted
an empty block-start as duplicate text; it now checks the assembled answer exactly once.
Read-only review found no blocking issue within this foundation's scope.

No installed CLI, live model, real account or desktop was used for this transport increment.
At that checkpoint, runtime admission/settlement and durable projection remained ahead.

## Phase 2 — durable receipts, transcript and internal executor

Protocol **33** adds display-only `ExternalMessage`/`ExternalBlock` types; they cannot encode
native executable tools and never contribute to native model context or tool-result synthesis.
The desktop adapter displays literal external user/assistant/thinking history, reconciles only
external user request IDs, and refuses native deltas against external entries. Execution stays gated.

Schema **22** adds separate external prompt receipts and projection records. Admission, external
user entry and run identity are atomic; identical request keys resolve before eligibility checks
and never authorize another launch. Cursor/generation/op CAS protects projection and settlement.
Only evidence-backed completed prompt receipts authorize resume: legacy receipt-less completed
ledger rows block both resume and a fresh launch into the same context. Startup recovery closes
unfinished receipts as unknown, preserves partial text, invalidates the binding and never replays.

`runtime/external.rs` is an **internal-only**, chat-only executor. It commits launch intent before
spawning, owns bounded supervision/projection, saves before publishing, commits Stop before
cancelling, and requires decoder/projection acceptance plus `Report::clean_exit()` before success.
Its `projection_finished` settlement witness is deliberately conservative: supervisor timeout or
observer failure vetoes it even when the terminal text and root exit code look successful.
Lookup, WaitForPrompt and snapshots now read external receipts without native-table fallback.
There is no public CLI Prompt/Stop activation or setup/selection route yet.

**Passed:** targeted external/context tests and 120 native adapter tests (job #35); full workspace
tests, Clippy all targets, formatting and diff checks (job #36). Seven fake-process runtime tests
cover admission/streaming/duplicate suppression, stop, hung-after-terminal handling and failures;
two read-path tests cover active visibility and recovered partial answers. Review #27 identified
legacy raw-ledger resume eligibility; the receipt-backed check and reopen regression fix it.
Bounded follow-up review #28 found no remaining issue in that fix.
Existing engine responsiveness fixes were preserved while the previously reverted bridge changes
were reapplied with the person's explicit permission. No installed CLI, live inference, real account,
desktop interaction, billing verification or real crash/orphan reconciliation occurred in this slice.

## Phase 2 — explicit metadata setup and orderly shutdown

Protocol **34** adds `InspectLocalCli`, `CheckLocalCli`, `LocalCliSetupStatus`,
`RevokeLocalCliSetup`, and `CreateLocalCliSession`. Inspection only captures filesystem identities;
both executable and project trust are required before bounded version/help/auth-status probes.
Tickets are owner-scoped, expire after five minutes, and are bounded to 32 total/four per client;
at most two checks run concurrently. Revocation, expiry, RPC cancellation, disconnect and shutdown
cannot publish late verification. Replies always report `execution_enabled: false`.

Creation uses the setup UUID as its durable request key. Exact repeats resolve before ticket
expiry/restart checks, but confer no fresh grant and never spawn a probe. Creation revalidates
metadata and serializes its commit against revocation. Existing-target checks require the same
saved project/executable/account/config/model and a valid context; they cannot rebind it.
Schema **23** permits only immutable `haiku`/`sonnet`/`opus` aliases in CLI session model storage,
behind immutable backend identity and native-only execution guards. The trigger change is a real
versioned migration with a v22 backup; legacy NULL models cannot silently acquire an alias.

The internal executor registers work before checking shutdown and keeps its task registration
through owned process cleanup and settlement. Engine shutdown waits concurrently with other
teardown within its deadline; timeout still leaves uncertain receipts for recovery. Prepared
metadata is revalidated again at actual actor launch, not just before admission. Metadata probes
are tracked and drained too, including after their requesting RPC is cancelled.

**Passed:** 19 protocol tests (job #37); targeted setup, internal executor, external store and
preflight tests (job #38); full workspace tests, Clippy all targets, formatting and diff checks
(job #39), including 12 setup and 12 internal-executor tests. The shutdown regression verifies
an owned fake metadata root is reaped (`waitpid` reports `ECHILD`). Review #33 found no issue in
shutdown/model persistence. Review #34 found wire cancellation/disconnect and create/revoke
ordering gaps; fixes and real-Connection regression tests passed, and bounded follow-up #35
found no remaining issue in those fixes. All new processes were fake executables: no installed
CLI, real account, live inference, desktop interaction or crash/orphan qualification was used.

This is not desktop-ready: public Prompt/Stop readiness, capability-aware core/UI, permission
hosting and the remaining qualification gates are still unfinished. Metadata verification is
neither inference permission nor proof of subscription billing.

## Remaining unverified / blocked gates

1. **Billing:** the CLI-reported Claude-account route and successful requests were
   observed, but no before/after usage-dashboard evidence was obtained. The person's
   participation is needed to establish actual allowance/spend behavior, which may
   remain inconclusive. Token counts, cost estimates and `authMethod` are not billing proof.
2. **Broader launch behavior:** the exact no-tools candidate, stdin framing, accepted stream
   projection and one completed-turn resume passed on 2.1.296. Normal-terminal comparison,
   other accounts/models/versions, full tool-event projection and usage projection are
   unverified. A controlled live stop at pending Write approval now passes; interruption
   during other stages of CLI execution remains unverified. Raw wire capture/replay is not provided by the redacted summary.
3. **Configuration isolation:** controlled startup hooks and MCP discovery now have
   calibrated positive/negative evidence. Full print-mode/user-plugin coverage and
   managed-policy cases remain open. Restricted mode permits host-explicit plugins;
   arbitrary caller flags must never be admitted. Do not bypass administrator policy.
4. **Permission hosting:** historical MCP schema recovered, but 2.1.296 compatibility
   remains untested. The current documented PermissionRequest hook passed exact live
   Write deny/allow-once checks. Scoped approval expiry/cancellation/disconnection now
   have offline fixture coverage, plus live pending-Write cancellation. General UI approvals,
   live expiry/disconnection/failure, and every-tool coverage remain unimplemented or unverified.
   `canUseTool` SDK types alone are not wire-protocol evidence.
5. **Recovery and context:** the disposable ledger passes synthetic crash/reopen and
   unsafe-binding gates. Phase 2 adds durable external receipts and synthetic internal-runtime
   supervision, but public activation and real crash/orphan reconciliation remain unfinished.
   The probe's `--resume` is manual qualification only. Neither path can reconcile real CLI crashes,
   confirm missing/edited CLI history, or retire stopped inputs from CLI context.
6. **Process containment:** the Unix implementation covers the owned process group, not
   descendants that deliberately `setsid`/detach. Root exit and pipe closure at live
   pending-approval cancellation were observed; graceful internal cleanup and real engine
   crash/orphan behavior remain unverified. No process-tree containment guarantee, Windows
   support, macOS run, desktop test, or latency/billing performance claim is made.
7. **Product release:** CLI product-use/branding conditions and current terms must be
   rechecked before distribution; this experiment does not establish legal approval.

At this earlier checkpoint, release qualification remained open (superseded by the scoped
Phase 5 checkpoint above). Durable external projection, default-off chat-only routing,
capabilities and desktop setup are verified with fake executables. Production activation, coding,
handoffs/recovery persistence, real crash/orphan recovery and authorized live/native qualification
remain ahead. The production permission broker and its qualification are still required for coding.

## Documentation evidence (not live verification)

Official docs reviewed on 2026-10-13:

- https://code.claude.com/docs/en/headless.md — stdin, streaming, print-mode trust caveats,
  `--bare` auth, explicit resume, interrupted-context caveats.
- https://code.claude.com/docs/en/cli-reference.md — CLI flags and auth status.
- https://code.claude.com/docs/en/authentication.md — credential precedence; a key in the
  environment wins in print mode; profiles/federation/gateways also affect routing.
- https://code.claude.com/docs/en/settings-reference.md — managed policy/hook precedence
  and explicit approval of fixture project MCP servers from disposable user settings.
- https://code.claude.com/docs/en/hooks.md#permissionrequest — current command-hook input,
  allow/deny output, absence of tool_use_id, and onFailure:block semantics.
- https://web.archive.org/web/20250806012109/https://docs.anthropic.com/en/docs/claude-code/sdk#custom-permission-prompt-tool — historical official MCP permission-tool example.
- https://code.claude.com/docs/en/agent-sdk/typescript.md — documented stream envelopes;
  not proof of a Rust host control protocol or permission-tool wire schema.
- https://code.claude.com/docs/en/legal-and-compliance.md — unmodified binary with
  user-owned login versus intermediating subscription credentials; product/branding conditions.
