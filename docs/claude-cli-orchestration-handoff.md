# Claude CLI orchestration — implementation handoff

## Dashboard integration — 2026-10-19

The owner authorized completion and publication of the dashboard after the isolated CLI fixes
commit below. Protocol42 now adds global activity, store-only observation, run-guarded Stop
and linked child follow-ups. Schema35 adds a compact covering index for child summaries with
the standard pre-migration backup; SQLite maintains it in the same transactions as its source
rows. Existing transcript and result data stays unchanged.

The dashboard has separate observation state and one explicit secondary root attachment.
Native and Claude orchestration child follow-ups use their original permissions and recorded
context, create a new linked child, and never replay old requests. Unknown control receipts
restore as lookup-only. Ordinary CLI Coding retains its existing lifecycle and capability
limits. No new Claude prompts were spent.

Current tests, native observations, performance measurements and limitations are recorded in
[the dashboard plan](orchestration-dashboard-plan.md). The earlier “no dashboards” statements
in this handoff describe historical slices and no longer exclude this authorized feature.

## Historical CLI-fixes commit qualification — 2026-10-19

The CLI stream, presentation, lifetime, clean-Stop continuation and historical recovery fixes
were isolated from the unfinished orchestration dashboard work before commit. The committed
wire protocol remains 41; the local combined build documented below also contains dashboard
protocol 42 changes. Schema 33 is retained before recovery schema 34 to preserve compatibility
with databases already upgraded by the combined local build. Dashboard endpoints, guarded
StopRun and activity directory implementation remain outside this fixes commit.

**Measured on the isolated staged source:** 2,662 tests passed, 13 ignored, zero failures;
strict workspace/all-target Clippy and formatting passed. No Claude prompts. The combined-tree
2,680-test and release-build evidence below remains historical evidence for that local build.
Exact staged verification: `dist/cli-fixes-commit/verification.json`.

## Recovery of old clean Stops — 2026-10-19

**Implemented:** explicitly reconnecting an ordinary CLI Coding conversation can now recover
an older, invalidated binding when its saved receipt proves a clean user Stop. This supersedes
the historical-lock limitation in the earlier clean-Stop release below. Recovery requires
accepted initialization, delivered input, reaped process, drained pipes, confirmed cancellation,
no cleanup timeout or rejected stream beyond incomplete EOF, and no earlier unresolved run.
Only older receipts that omit the continuation-authorization field qualify; an explicit
negative authorization result from a newer build cannot be overridden.

Use **Reconnect Claude Code…** in the stopped conversation. Fresh project, executable/version,
CLI configuration/account, model and policy checks must match its saved identity. Inspection
alone does not unlock it; revoked setup checks cannot recover it. The verified reconnect writes
an immutable audit record and advances the binding generation atomically. The CLI session ID,
stopped receipt, completed actions and unresolved action results are preserved. Reconnect sends
no prompt. A subsequent new instruction resumes the existing CLI session; redelivery of the
old request still returns its original aborted receipt without executing it.

Engine schema34 adds the recovery audit and guarded binding transition, with the normal
pre-migration backup. Recovery adds no wire messages or desktop schema change. Migration does not itself
unlock anything. Timeout, observer failure, missing/contradictory evidence, older Unknown runs,
unsettled approvals/actions, removal and changed identities remain fenced. This recovery is
for ordinary Coding, not legacy approval-backed sessions or native orchestration owners.

**Observed:** read-only inspection found one eligible historical clean-Stop receipt (19) and
seven other locked conversations whose latest receipts remain Unknown. No live database was
edited and no live conversation has been recovered by this task. Fresh matching reconnect is
still required. Redacted evidence: `dist/cli-stop-recovery/observed.json`.

**Offline coverage:** full desktop core/adapter/engine reconnect and same-composer resume;
public RPC fresh/revoked/mismatched setup checks; duplicate-request non-replay; immutable receipt
and unknown-action retention; atomic rollback/CAS; schema33 backup/migration; restart and
removal. Synthetic CLI processes only. Installed-Claude resume and actual-window interaction
remain unverified. No Claude prompts were spent.

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

## Clean Stop continuation — 2026-10-19

**Cause:** settlement already recorded a clean user Stop as `Aborted`, but unconditionally
invalidated its immutable CLI binding. Admission also rejected all aborted history, making
an otherwise cleaned-up conversation permanently read-only.

**Implemented:** an evidence-backed clean Stop now preserves the binding and permits a
**new request** in Resume mode on the same CLI session. Eligibility requires a durable Stop,
accepted initialization, delivered input, explicit supervisor cancellation, reaped root,
drained pipes, still-active connection/consent authorization, no cleanup timeout and no malformed/rejected stream evidence except the
expected incomplete EOF. Both settlement and later admission use the same evidence predicate.
Missing/legacy evidence, early cancellation, observer/transport failures, crash recovery and
unconfirmed cleanup retain the existing fence. Binding generation and identity stay unchanged;
no schema migration or permission restoration is involved. The new persisted authorization
fact defaults to false for historical receipts. Disconnection, revoked/expired consent and
engine shutdown cannot authorize continuation even when their process cleanup was clean.

The old request is permanently settled as aborted, including after restart. Duplicate delivery
returns its existing receipt; it cannot launch again, and a changed payload under the same
request key is rejected. The next launch sends only the new instruction with the existing
`--resume` session ID. Completed actions stay completed; unresolved actions are marked Unknown
in both stored action records and transcript blocks. Old action callbacks cannot amend a
settled run. No queue drain or automatic continuation is added. Stop does not undo file effects,
and retaining CLI history does not guarantee a model will never choose to repeat an operation.

Offline coverage includes the public Coding RPC route, desktop core/adapter/engine composer
readiness and same-session continuation, duplicate requests, persistence/restart, pending-action
retirement, owned-process cleanup and a matrix of insufficient cancellation evidence. Legacy
approval-backed Stop still revokes old reviews; Deny remains fenced. Tests use synthetic CLI
processes only. Installed-Claude cancellation/resume and real-window interaction are unverified.

**Compatibility:** already-invalidated conversations remain read-only, including clean Stops
settled by older builds. The immutable binding fence is not reversed and historical receipts
are not rewritten. This fix applies to clean cancellations performed with the new build;
valid stopped conversations can reconnect after restart without replaying the stopped request.

**Measured offline:** 2,674 tests passed, 13 ignored, zero failures
across 48 workspace suite results. Strict workspace/all-target Clippy, formatting
and diff checks passed. Final source remained unchanged through the checks and release build
(manifest SHA256 `d0ca083e59b461fc78141e3d4ff2883cfb793e2dbb851511a66fd976c3aeffc0`). All five prompt-budget files stayed byte-identical;
zero Claude prompts. Release build and isolated native-engine startup probe passed.

Quit the old app and launch with `scripts/run.sh`. Evidence: `dist/cli-stop-fix/summary.json`
and `dist/cli-stop-fix/release-build.json`. Built binary:
`/home/jason/coding/pipkin/dist/source-setup/builds/226057fee234411bafd6bf1b97a26460/bin/pipkin`.

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

## Desktop controls — 2026-10-19 (slice35)

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

## Desktop activation and routing — 2026-10-19

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

**Historical slice34 remainder:** the four desktop controls listed at that checkpoint are
implemented in slice35 above. Wider installed-CLI and real-window qualification remain open.


Earlier sections below are historical milestones; their statements that all public
orchestration is disabled are superseded by slice34's explicit desktop mode.

## Thinking, attachments and usage metadata — 2026-10-19

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

## Concurrency, payload and post-Stop behavior — 2026-10-19

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

## Aggregate background usage and goal allowances — 2026-10-19

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

## Background delivery integration — 2026-10-19

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

## Mid-turn steering and queue batching — 2026-10-19

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

## Context, compaction, queues and queued steering — 2026-10-19

Implemented in the private CLI orchestration route; public activation stays gated. This
milestone supersedes older notes that list all context/compaction and queue work as absent.

- **Context:** use scoped per-assistant-message input/cache-input counts, labelled “last
  request input.” Terminal cumulative usage and the desktop transcript are not context-size
  estimates. Missing fields, overflow, capacity and post-compaction size remain unknown.
  Automatic/manual compaction boundaries invalidate the old measurement. Uncertain or
  unfinished turns cannot supply a current measurement; history remains displayed intact.
- **Manual compaction:** a typed queued action generates `/compact` with optional single-line
  instructions and resumes the same CLI UUID. Normal turns still disable slash commands.
  This action alone enables command dispatch with `--bare`, retaining the fixed system,
  disabled built-in tools/hooks and explicit native MCP server. Native tool admission is
  forbidden during compaction, while bridge initialization/closure remains allowed.
  Completion requires a scoped compaction boundary plus all normal clean terminal/process/
  drain/bridge/projection evidence. Missing boundaries fence the outcome and block continuation.
- **Queue:** bounded durable request/intent rows, original connected one-use tickets,
  synchronously tracked actors, capacity before run admission, exact duplicate lookup and
  conflicting-key refusal. Follow-ups retain FIFO. Withdrawal cancels even while waiting for
  capacity. Stop, revocation, failed preparation and Unknown suppress pending inputs.
  Restart withdraws orphaned queued rows while retaining text; it never rebuilds authority.
- **Queued steering:** priority over follow-ups among unreserved inputs, delivered as its own
  user turn at the next clean CLI turn boundary. Already-reserved work keeps its place.
  **Mid-turn/tool-step steering is not implemented by this slice.** Each input is a separate
  CLI turn; native API batching settings are not applied. These differences remain explicit
  parity items rather than being hidden behind a “complete” label.

Schema30 adds immutable input intent and bounded context-observation storage. Existing
request/queue/history/status presentation is reused; no API summarizer, API inference
fallback, fabricated CLI transcript or model-observed tool execution is introduced.

**Focused verification passed:** seven owned fake-CLI/native-MCP integration tests cover
ordering, duplicates, withdrawal, Stop/revocation/recovery, capacity waits, compaction with
same-UUID continuation, missing-boundary fencing, forbidden native Write and context reporting.
The initial focused logs retain the reproduced bridge-initialization failure; the latest
`dist/cli-context-inputs/focused.log` passes after moving the guard to actual effect admission.
The first full gate caught six existing committed-revision regressions from the new generic
context notification. CLI settlement now publishes the actual committed revision directly
under the publication gate; those existing tests remain unchanged. `before-final-fixes/`
retains that failed gate and the fixed lint warning. Final results supersede these logs.

**Final verification passed:** **2,576 workspace tests passed, 13 ignored, zero failures**
across 48 suite results. Strict all-target workspace Clippy (`-D warnings`), formatting and
diff checks passed. The six existing committed-order regressions pass unchanged, and the
fenced-history context regression passes. Source remained frozen across the final gate;
all Claude budget ledgers remained byte-identical. **Zero new Claude prompts.**
Source-manifest SHA256 (Rust/SQL/manifests and compiled reference):
`dcec23111994a25ef20364480b4c9dd024c3b15cfb8d06bf6c2172d25f748a64`.
Evidence: `dist/cli-context-inputs/{summary.json,workspace.log,clippy.log,fmt.log,diff.log,source-before.json,source-after.json,budgets-before.json,budgets-after.json}`.

**Unverified:** installed-CLI manual compaction under this precise launch policy, automatic
compaction telemetry and desktop behavior. The existing live fresh/resumed Write evidence
keeps its original scope. Zero additional Claude prompts; no release/install, running-app
mutation, commit or push. The user still owns rebuilding/installing Pipkin.

Next work: mid-turn steering and CLI batching semantics; remaining background integration;
broader concurrency/payload/post-Stop, thinking/attachments/usage and concurrent MCP coverage;
the existing desktop setup/model/approval route; renewed bounded installed-CLI and desktop
qualification. No dashboards or Pi extensions. See slice28 in the capability checklist.


## Goal start, scheduling, resume and independent auditors — 2026-10-18

This milestone supersedes older notes below that say CLI goals or required audits are
unavailable. Implemented in the private CLI orchestration route. Explicit person intent can start a goal
or resume a resumable goal under the original connected root approval. One tracked controller
owns scheduling; each worker turn resumes the same CLI UUID. The controller never calls the
API goal scheduler, prompt runner, or API auditor. Public setup/desktop activation stays gated.

Goal epoch, turn count, generated input and run admission commit together after capacity is
reserved, with exact goal comparison and duplicate admission lookup. Limits, stalls, queued
human input, changed goals, Stop, lost approval and uncertain roots stop further scheduling.
No saved goal, binding, receipt or restart record reconstructs execution authority. Final-line
continue/blocked/unreachable markers remain labelled fallbacks scoped to the actual completed
turn; an old answer cannot drive a new turn. A completion marker alone cannot replace native
mechanical evidence or a required independent audit.

A required audit now launches a fresh CLI child from the authenticated completion-proposal
call. The frozen pending goal, claim fingerprint, CLI model alias, system and inspection-only
catalog are bound at creation. Custom profiles cannot add mutation, shell, network or child
launch tools, or inherited history. API model selections fail before inference. A successful
native inspection receipt is required before accepting an auditor's terminal. Approved,
changes-requested, inconclusive, malformed, stale and failed results use the existing goal
domain; requested changes feed the next worker turn and a later independent audit round.
Auditor continuation as a normal child is refused. Stop joins the child and prevents approval.
The completion tool now reports the actual goal/audit state instead of claiming every accepted
proposal is already completed and unaudited.

Goal token accounting reads CLI terminal receipts for the goal's worker turns and delegated
children, counting each once. It does not read the API usage ledger. Missing usage stays
unknown; a token-limited goal pauses, and partial totals are labelled as a known minimum.
**Dollar-limited goals pause before admission:** this CLI receipt contract has no dollar-cost
measurement. No API pricing estimate or invented zero is substituted. Broader aggregate/UI
usage qualification remains on the checklist.

**Verification passed:** 40 focused goal tests; **2,568 workspace tests passed, 13 ignored,
zero failures** across 48 suite results. Strict all-target workspace Clippy (`-D warnings`),
formatting and diff checks passed. Source and all Claude budget ledgers stayed unchanged.
Source-manifest SHA256: `f0729d9b6c4ff024956d1d29e93254f35cce8d882f14a803136bd364dcec5182`.
Evidence: `dist/cli-goal-lifecycle/{summary.json,workspace.log,clippy.log,fmt.log,diff.log,source-before.json,source-after.json}`.
Earlier focused logs retain initial fixture/test failures; the final results supersede them. All executables and MCP
peers in these tests are owned fixtures. **Zero new Claude prompts.** Live goal/auditor and
desktop behavior remain unverified; the previous live fresh/resumed Write qualification keeps
its original scope. No release/install, running-app mutation, commit or push.

The capability checklist's slice27 records that implementation and boundary. At that milestone, remaining work included:
context/compaction, queues/steering and remaining background integration; broader concurrency,
payload/post-Stop, thinking/attachments/usage and concurrent MCP qualification; the existing
desktop route and installed-CLI/desktop lifecycle qualification. No dashboards or Pi extensions.

## Child continuation and inherited context — 2026-10-18

Implemented both in the private CLI orchestration route. `subagent` can continue a completed
child by number with a new task, reusing its CLI UUID, frozen model/system/tools/resources and
original connected parent approval. Inline, notification-only and wake delivery are supported.
Busy/foreign/unknown children, changed policy overrides, lost approval and reconstructed
restart metadata do not authorize another model invocation. Task settlement/recovery now uses
the latest child turn, preserving uncertainty after a failed continuation.

Inherited context is a frozen, bounded role-labelled excerpt served through the read-only
native `parent_context` tool. Children must read it in each turn before other native tools or
successful completion. It is reference data, not task concatenation, a full parent-session
fork, fabricated CLI history, or permission. The existing bounded-excerpt semantics remain;
continuation keeps the original snapshot. A regression reproduced truncation at the ordinary
50KiB tool-output limit; the context tool now allows128KiB for worst-case JSON escaping/UTF-8.
See slice26 in the capability checklist for details.

**Verification passed:**76 focused child-related tests, the separate escaped/Unicode context
regression, and the final offline workspace gate: **2,559 passed,13 ignored, zero failures**
across48 suite results. Strict all-target workspace Clippy (`-D warnings`), formatting and
diff checks passed. Source stayed frozen across the final gate; SHA256:
`086435b9d1eb44bfae97209b8bbbf4363c1cdc7252b6730b6ef74de60243eba7`.
`context-bound-before.log` retains the failing output-limit reproduction;
`context-bound-after.log` passes. `interrupted-gate/` records the earlier gate deliberately
stopped before tests to fix that edge case. `summary.json` records the final passing gate.
Evidence directory: `dist/cli-child-context/`. These tests use owned CLI/MCP simulations and
scripted providers; they do not invoke the installed CLI. **Zero new Claude prompts.** Live
budget ledgers are checked unchanged. The earlier live fresh/resumed Write result remains
valid within its recorded scope; live child/context compatibility is still unverified.

Public orchestration remains disabled. No release build/install, running-app mutation,
commit or push. At this earlier milestone, remaining work included goals (now implemented
privately in the milestone above), context/compaction,
queues/steering and remaining background integration; full concurrency/payload/post-Stop,
thinking/attachments/usage and concurrent MCP qualification; existing desktop route and
broader installed-CLI/desktop lifecycle qualification. No dashboards or Pi extensions.

## Live fresh-and-resume verification — 2026-10-18

**PASSED with installed Claude CLI2.1.301 and Sonnet after the SIGINT fix.** The user explicitly
authorized two new prompts. One new isolated native-MCP session used exactly those two prompts:

| Turn | Native effect | Durable result |
| --- | --- | --- |
| Fresh | Wrote `native-first` to `native-probe.txt` | Completed; bridge closed |
| Same-UUID resume | Replaced it with `native-resumed` | Completed; bridge closed |

Both receipts contain a successful model terminal, exit0, reaped process, drained pipes and
finished projection, with no cleanup timeout, parse failure, stream diagnostic or incomplete
presentation. Both native Write calls have completed durable receipts; the actual final file
contains `native-resumed`. Both turns used the same Claude UUID. Scripted API requests: zero.

Evidence: `dist/cli-sigint-live/live-summary.json`, `live-direct.log`, and retained
`live/probe-veABjp/{result.json,turn-1.json,turn-2.json,data/engine.sqlite,project/native-probe.txt}`
under that directory. `native-orchestration-sigint-pair-budget.json` records allowance2/spent2.
All earlier budget hashes are unchanged; the three earlier Unknown roots remain untouched.
No further prompts are authorized. The initial Cargo-driven probe failed metadata preflight
before reservation because Cargo injects a loader-path variable. Running the exact compiled
test binary directly passed the normal preflight; no production environment guard was weakened.
That initial failure remains in `live.log` and consumed zero prompts.

This qualifies **fresh and resumed native Write for this CLI/version/model/catalog**. It does
not qualify every tool, child/job/goal workflow, model alias, cancellation/restart path or the
desktop. Public native orchestration stays disabled and the remaining parity work is unchanged.
No release/install, running-app change, commit or push. The test harness alone changed to select
this new bounded pair; production code is the SIGINT fix already tested below. Final offline
verification passed: **2,555 tests passed, 13 ignored, zero failures** across 48 suite results;
strict all-target workspace Clippy, fmt/diff and frozen-source equality. Source SHA256:
`04d392360004d816534d29a25ba151300d6f041f4d76d2ab8e83ca096417cb15`. Full logs are in `dist/cli-sigint-live/`.

## Connection-loss fix — 2026-10-18

**Identified and reproduced offline: unhandled SIGINT killed the dedicated MCP helper.**
Read-only inspection of the installed2.1.301 executable shows that connected Unix MCP cleanup
sends SIGINT to the server PID first, then escalates to SIGTERM after100ms and SIGKILL after
another400ms. Pipkin handled only SIGTERM. The new actual-helper regression completed a native
Write, sent SIGINT to its exact owned helper, and reproduced termination with signal2 before
clean closure. It failed before the fix (`dist/cli-bridge-sigint/before-fix.log`). This explains
why adding EOF/SIGTERM handling alone did not cover the CLI's actual cleanup path.

The dedicated helper now registers SIGINT and SIGTERM before connecting. Both stop MCP input
through the existing authenticated InputClosed/Closed handshake. Neither is a model terminal.
No root completion, resume, retry or API fallback authority is added. The normal engine/app
invocation does not register these helper-only handlers.

All six actual-helper lifecycle tests pass after the fix (`after-fix.log`), including SIGINT
with/without a later owner terminal and SIGINT after owner Stop, plus existing EOF/SIGTERM,
partial-input and forged-control cases. Source changes are limited to the helper and its
integration tests. The full offline workspace gate passed: **2,555 tests passed, 13 ignored,
zero failed** across 48 suite results; engine1594/app337/UI217 and all remaining suites.
Strict all-target Clippy, fmt/diff and frozen-source equality passed. Logs are in
`dist/cli-bridge-sigint/`; source-manifest SHA256:
`41ccf583ea2b1e5cc9bd712e904cf34c062d0979df511608fc93dad2d29ee8cc`.

**Zero new Claude prompts and no installed CLI process were used.** The executable was only
read to inspect cleanup behavior. The exact three owned probe transcripts were read for
record types and tool-result evidence; no credential files or other session transcripts were
read. Binary identity/behavior metadata is retained in `dist/cli-bridge-sigint/cleanup-evidence.json`;
no executable source is copied into the repository. Prompt ledgers remain unchanged/exhausted.

The three prior live roots remain Unknown and must not be resumed/replayed. This is a failing-
then-passing offline reproduction of the installed shutdown sequence, **not a new installed
fresh/resume qualification**. Public native orchestration stays disabled. Remaining feature
work below is unchanged; no release/install, app restart, commit or push was performed.

## Resume note — 2026-10-18

Implementation resumed at the user's request. The original handoff below is historical.
Child wake/root continuation and job wake now have focused offline workflow tests, including
same-UUID resume, idle/busy Stop, capacity wait, reapproval and restart suppression. The
resumed source fixes the missed integration-test InputMode initializer and the idle Stop
routing defect. Completion text/digest capture now shares the publication gate. The resumed production
workspace gate passed (engine1588/app337/UI217, strict Clippy, fmt/diff, frozen-source equality);
public orchestration is still disabled and the objective is not
complete. See slice22 in `claude-cli-native-orchestration.md` for current scope/evidence.

A complete backup of all 163 initially dirty/untracked files is retained at
`dist/cli-orchestration-resume/starting-source.tar.gz`, with SHA256 manifest alongside it.
Logs from this resumed work are in the same directory. Installed-inference consumption is
recorded below. No release build/install, running-app change, commit or push was performed.

Installed qualification has now consumed all three newly authorized Sonnet prompts. Each
isolated fresh turn performed a native Write, but none settled successfully: the native driver
lost its channel, the supervisor cancelled the CLI, and all three roots remain Unknown.
**No same-UUID resume was attempted. Do not resume/replay these roots or use superseded unused
ledger slots. No further inference is authorized.** The separate resume ledger is false/0.
At that checkpoint, the channel-loss cause was unresolved. The SIGINT diagnosis and offline
fix above supersede that diagnosis; the later successful live pair above supersedes the
qualification status of this checkpoint.

The early-close checkpoint passed its full workspace gate (`close3-*`, engine1593/app337/UI217,
strict Clippy/fmt/diff, source equality; hash
`38fedce6b4a3508331cb260cf5ccb4d0e4d68b79d5d13c0c3ae64c87a42640d3`).
The later `signal-*` gate passed all but one engine export test, which exposed stale exporter
health overwriting a credential-clear reply. The revision fence and deterministic regression
passed nine focused tests. Final `verified-*` gate passed: 2,553 tests,13 ignored, zero failed;
engine1594/app337/UI217, all remaining suites, strict all-target Clippy, fmt/diff and source
equality. Final source SHA256:
`33a4ee1fc9123e5a54414b760dbb8308689c548a0b7d936c93a8331d269b36a3`.
Source was unchanged across that checkpoint gate. The newer SIGINT fix and its full gate
are recorded above.

Latest consumption/evidence is in `fixtures/claude-cli-probe/native-orchestration-probe.md`.
The SIGINT reproduction/fix above is the next completed investigation. Any future installed
qualification still requires renewed explicit bounded inference approval. Preserve Unknown/no-replay
semantics. At this checkpoint, child continuation and inherited context were also pending;
the latest implementation section above supersedes those two items. Goals/auditors,
queues/steering, limits/usage/MCP parity and desktop activation remain.
No public activation, release/install, running-app mutation, commit or push was performed.

## Original handoff — read with the resume note above

**The goal is NOT complete. Public native orchestration remains disabled.**

The user requested this handoff to try something else. Implementation is stopped for the
handoff. No new build/test job was started for this document; `job_status` reported no running
job. Do not interpret an active plan task as unattended coding continuing in the background.

**Current source is newer than the last passing gate.** Job360 is the last fully verified
milestone. After360, a child-wake/continuation implementation was partially written. Those
changes have **not been compiled, formatted, tested or Clippy-qualified**. There are known
outdated tests/fixtures. Do not describe the current working tree as passing360, or enable it.

The most recent user-facing estimate was roughly **30–35% remaining**. This was judgment,
not a measured percentage. Use the capability checklist, not that estimate, to assess readiness.

## Objective and plan

Goal6: **finish out the whole plan and get full orchestration parity in for claude cli models.**
The installed, unmodified Claude CLI must be the inference executor for native Pipkin workflows,
including tools, plans, jobs, subagents, questions, goals, queues, steering, Stop and history,
through the existing desktop. **No API inference fallback.**

Plan revision127:

- #74 active: full native Pipkin workflow parity for Claude CLI.
- #77 active, under74: context, children, queues, goals and lifecycle integration.
- #87 active, under77: scoped CLI-only ToolHost/service integration.
- #78 pending after77: expose and qualify in the existing desktop.
- #76 private foundation and #86 completed; these do not mean full orchestration is delivered.
- Dashboard tasks54–61 cancelled by the user. **Do not build dashboards.** The user will build
  them using the integration.

`get_goal` at handoff reported paused after the last turn omitted a status report; its last
progress note still describes360. The handoff pause is user-directed, not a technical coding
blocker. Resume implementation only when the user asks to resume or hands this to a successor.

## Workspace and non-negotiable constraints

- Repo: `/home/jason/coding/pipkin`.
- HEAD at handoff: `826615ed89cda13c6206e88fc0880dd3ab5f57f9`.
- The implementation is a **large dirty, uncommitted tree**, including many untracked Rust
  modules. Preserve it. Do not reset/clean/revert the tree wholesale; that would lose earlier
  work, not just the unfinished wake changes. No repository commit or push was made.
- The user rebuilds and installs. **No release packaging/build/install, running-app mutation
  or restart, commit or push.** Offline tests and Clippy are allowed when work resumes.
- **Zero installed-CLI inference prompts remain authorized.** The original10 and follow-up3
  are consumed. No installed inference until renewed explicit bounded permission. Never evade
  this by changing directories, UUIDs or profiles.
- Known installed executable:
  `/home/jason/.local/share/mise/installs/claude/2.1.301/claude`.
  Stable numeric versions >=2.1.299 are supported subject to checks; qualification is specific
  to tested version/model/tools, not every alias/version.
- No credential inspection, CLI config/install modification, untrusted executable execution,
  uncertain replay, or API/native-provider inference substitution.
- No more subagent reviewers. Existing fake CLI/SDK fixtures are allowed; they are not installed
  CLI qualification. No synthetic UI input was used in these batches. Future input requires
  an owned-window guard; never manipulate unrelated windows.
- Read `.agents/skills/gpui-desktop-app/SKILL.md` and its `references/architecture.md` before
  implementation; both were read for this handoff.
- Pure Rust core, thin GPUI; `(conversation,generation,op)` stale guards; no render I/O/parsing.
  Preserve virtualization, selection, scrolling, caches and per-conversation drafts/history.
- Use owned short TMPDIRs, e.g. `mktemp -d /home/jason/.pg.XXXXXX`; `/tmp` is constrained.
  Normal parallel tests, not serialized suites. Retain failures; delete only owned disposable
  artifacts. Freeze source during full gates and nested MCP fixture builds, including the
  compiled `docs/native-engine-reference.md`.
- User DB, if necessary, is read-only only:
  `/home/jason/.local/share/pipkin/engine/engine.sqlite`, `mode=ro`, `PRAGMA query_only=ON`.
  Never mutate it directly. Goal conversation: `11fda2e5-be2f-4ed7-9b89-fb63e0019874`.

## Authoritative docs and evidence

- `docs/claude-cli-native-orchestration.md`: capability checklist and implementation slices1–21.
  Describes verified milestones through360; **does not qualify the new wake WIP**.
- `docs/scorecard.md`: measured/failed/unverified separation, newest completed milestone360.
- `docs/local-claude-cli.md`: separate Coding path and qualification boundaries.
- `docs/native-engine-reference.md`: compiled reference. Keep frozen during verification.

### Last fully passing milestone: job360

Artifacts: `/home/jason/.pg.omyc8L/360-*`.

Commands passed on frozen source:

```sh
cargo test --offline --workspace
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo fmt --all --check
git diff --check
```

- Engine1584 passed (2 ignored), app337 passed (1 ignored), UI217 passed; other workspace
  suites also passed. These counts refer to the named suites, not one aggregated total.
- Source snapshot and live-budget ledger equality passed before/after the gate.
- Source snapshot SHA256:
  `5cd4c5ffad1dccc5509611a426741142c025fead444885235dac7cb14a740c69`.
- `360-source-before`, `360-source-after`, `360-source-handoff`, `360-workspace.log` and
  `360-clippy.log` are retained.

Other useful integrated milestones:

| Job | Scope | Artifacts / result |
| --- | --- | --- |
| 353 | Jobs and goal reporting | `/home/jason/.pg.E47DhW`; engine1580/app337/UI217 plus strict frozen checks |
| 350 | MCP/web/forms/directory/progress | `/home/jason/.pg.qnVGJw`; engine1576/app337/UI217 plus strict frozen checks |
| 347 | Fresh inline children | `/home/jason/.pg.cTikc9`; engine1573/app337/UI217 plus strict frozen checks |
| 343 | Frozen child policy/schema29 | `/home/jason/.pg.wocULQ`; engine1570/app337/UI217 plus strict frozen checks |
| 339 / 335 / 331 | Questions / plans / owned admission | Historical frozen milestones; details in orchestration doc |

Retained354–359 failures matter:

-354 used an incorrect child-test filter (zero child tests ran), then failed Clippy.
-355/357 incorrectly expected all queued task-view events after cancellation. The bounded
  delivery queue legitimately replaces presentation frames with `ResyncRequired`.
-356 exposed an existing app test race: its catalog-call counter was sampled after publishing
  an event. `pipkin-app/src/adapters/native/tests.rs` now samples before publishing. Five
  focused repeats passed357 and the workspace passed360; production behavior was not changed.
-358 exposed the existing result-view contract: failed child results have `result=None`.
  Uncertainty is now also recorded in the task's note so result/status views show the warning.
-359 passed6 child workflows and12 storage tests, then found an unused import via Clippy.
  It was removed before360.

## What is verified through360

Everything here is **private/offline integration unless explicitly marked live**.

### Separate existing Coding path

`cli-unattended-coding-v1`, `LaunchPolicy::Coding`, `ExecutionProfile::LocalCliCoding` exist.
Live jobs273/276 qualified CLI2.1.301/Sonnet fresh/resume and Read/Write through desktop
core/adapter/SQLite. This does not qualify native orchestration, Bash/Edit/agents, other aliases
or new CLI releases. Existing Coding identities are not silently upgraded.

### Native orchestration infrastructure

- Independent approval, immutable binding/model/policy/root identity and permanent CLI UUID
  ownership. Public `claudeCliAgent` still decodes to `ExecutionBackend::Unsupported`.
- `EngineConfig.enable_local_claude_cli_orchestration` defaults false; Linux plus master/new
  flags required. Private tests opt in. Do not expose the route yet.
- CLI built-in tools disabled; only the exact scoped `mcp__pipkin__*` catalog offered through
  owned MCP config. Empty settings sources, disabled hooks/Chrome/slash/permission prompts.
- Independent localhost authenticated native channel, mutual HMAC, one-use bridge claim,
  durable intent before effect and actual receipt before reply. Stdout/XML/SDK tool events
  are observations, never execution authority.
- Sealed helper image/assets, supervised process/pipes/deadlines, owner-controlled close
  handshake, durable root launch journal and streamed canonical projection.
- Success requires actual supervisor/process/projection/native-receipt evidence, not terminal
  text or EOF alone. Unknown outcomes retain known effects and prohibit replay/successors.
- Engine-tracked admission/startup/cleanup survives dropped caller receivers. SessionRt lifetime,
  Stop, disconnect, removal, shutdown and shared ordered publication are integrated privately.

### Services

- Plans/checkpoints and exact-question-ID reply grants. Answer/decline/Stop/removal/disconnect/
  shutdown tested; displayed source text is not permission.
- Seven MCP routes plus per-call form elicitation using existing MCP services. Ambiguous scoped
  concurrent elicitation declines; full concurrent configuration/elicitation parity unqualified.
- Web search/fetch/read reuse network/domain services; scoped source retention/revisions.
- Bounded UTF-8-safe native progress, actual call IDs, expired callbacks silent.
- Job start/status/log/wait/cancel through existing process supervisor. Owned startup, actual
  spawn receipts, replay and cleanup-only recovery tested. **Job wake is still refused.**
- Goal get/update/evidence-based proposal with expected-view comparison and matching-run Stop
  pausing. Real file evidence vs observation-only claims tested. Private tests seed goals.
  **No CLI goal start/resume/scheduling or required audit yet; required audit explicitly refuses
  rather than launching an API auditor.**

### Children through360

- Frozen CLI-only profile/model/tool/system/resource/caps/delivery policy. API-selected profiles
  rejected even with overrides. Aliases only `claude-cli/haiku|sonnet|opus`; absent inherits.
  Thinking unsupported; no nested delegation/questions.
- Fresh inline and fresh notification-only background (`wait=false,wake=false`) execution uses
  independently owned CLI roots, not `subagents::Host`'s API runner.
- Exact pending native parent call creates metadata/task/binding/UUID/policy atomically.
  Duplicate receipts or saved policy metadata never grant another launch.
- Background work survives normal parent completion under the original connected approval.
  Parent cancellation before a successful native tool receipt cancels unconfirmed delegation.
  Child Stop, approval revocation, disconnect and shutdown revoke owned work.
- Task running state is atomic with actual child admission; actual parent revision publishes
  under the shared gate. Completion uses an unfenced Completed root receipt and retained
  assistant text from that run. Unknown effects remain inspectable, with no-replay notes.
- Cleanup-only startup reconciliation classifies orphaned metadata/lost task settlement;
  no API recovery, child relaunch or reconstruction of authority.
- Real-engine fixtures cover parent completion before child activity, actual native Write,
  exact MCP replay and2 simulated model-process launches, cancellation, handoff uncertainty,
  result/wait/foreign reads and recovery. Zero scripted model-provider requests.
- At360, **child wake was explicitly refused**. Current WIP changes that private path but
  has no verification. Children still need available generation capacity.

## Unfinished changes after360: child wake / root continuation

### Status

**Written but uncompiled/untested/unformatted.** No wake test was added or run. The old
wake-refusal test and fake CLI script have not been updated. Last job remains360; there is no
new successful gate for these edits. `git diff --check` passed during handoff, which is only a
whitespace check, not compilation or behavior verification.

### Intended workflow — design intent, not a verified claim

An authenticated native `subagent` request with background wake would mint a one-use ticket
from the original connected ROOT approval while the parent call is pending. After the child
settles and the parent becomes idle with confirmed history, the ticket would mint one owned
root Turn. The existing CLI runtime would resume the same CLI UUID using a new engine-generated
FollowUp input. Admission would atomically mark delivery `woken`; failed/cancelled pre-admission
work would suppress it, never retry or use API inference. Stop, including idle Stop, would
invalidate existing pending tickets. Restart would suppress saved wakes, not recreate tickets.

### New files

1. `crates/pipkin-engine/src/runtime/cli_agent_owner/wake.rs`
   - `WakeTicket`, non-Clone, holds original registry/grant, cancellation epoch, originating
     scope/call and `issued` flag.
   - `ServiceAuthority::child_wake_ticket()` checks original authority, root-only role and exact
     pending native `subagent` intent.
   - `try_begin()` waits read-only on Busy, checks grant pointer/liveness/readiness, captures
     stored Wake data, reserves a root Turn and permanently consumes the ticket.
2. `crates/pipkin-engine/src/store/cli_agent_children/wake.rs`
   - Cloneable sealed **data**, not authority: child/parent/call/run, generated request/text and
     task digest. Request key `cli-child-wake:<child-session>`.
   - Checks finished task still `wake`, successful native handoff, original parent Completed /
     unfenced and not stopped. Revalidates task digest and exact FollowUp input in admission.
3. `crates/pipkin-engine/src/runtime/cli_agent_host/wake.rs`
   - `Host::deliver_child_wake()` formats existing domain wake text, waits on original ticket,
     submits the owned CLI root with captured resources, then suppresses any undelivered wake.
   - No generic API enqueue/runner used. Test peer forwarding is sketched but not exercised.

### Modified integration points

- `runtime/cli_agent_owner.rs`: Grant has `continuations: Mutex<CancellationToken>`;
  `Registry::stop()` cancels/replaces this epoch even when idle. Turn holds optional Wake data
  and wake cancellation; exposes `input_mode()` and uses wake-aware admission.
- `runtime/cli_agent_owner/children.rs`: removes the Wake rejection, initializes new fields and
  gives Wake children connected-approval lifetime like Notify children.
- `runtime/cli_agent_host/children.rs`: adds captured `root_system`, mints wake ticket before
  creation, removes wake refusal, changes background response wording and invokes delivery
  after child settlement. `child_view` exposed within host modules.
- `runtime/cli_agent_host.rs`: declares wake module.
- `store/cli_agent_turns.rs`: `Input` now has `InputMode`; Prompt fingerprint domain remains
  `pipkin-cli-agent-turn-v1`, FollowUp uses `pipkin-cli-agent-followup-v1`. New
  `admit_cli_agent_turn_with_wake()` validates optional Wake and atomically marks `woken` /
  publishes task revision with input/run admission. Launch-context checks accept Prompt or
  FollowUp and validate the corresponding canonical mode/fingerprint.
- `store/cli_agent_children.rs`: exports Wake, leaves live `wake` delivery pending on result
  settlement, explicitly suppresses it during startup recovery.
- `execution/claude_cli/orchestration_policy.rs`: Assets retains bounded system text in an Arc
  with an accessor, avoiding rereading a system file to construct continuation resources.
- `runtime/cli_agent_runtime.rs` and `runtime/cli_agent_runtime/admission.rs`: supply actual
  system text to resource capture; lookup uses the Turn's input mode.
- `runtime/subagents.rs`: existing `subagent_wake_text()` formatter made visible within runtime.
- Input literals in owner/children, observer/policy/bridge/root/storage tests were updated to
  explicitly use Prompt. Audit for missed call sites; compilation has not checked this.

### Known immediate gaps and review points

1. `cli_agent_root/child_tests.rs::background_wake_is_refused_before_child_creation_or_inference`
   still asserts the OLD behavior. Do not call this a passing wake test. Replace/extend it
   with actual owned parent -> child -> resumed root workflows and explicit refusal cases.
2. Fake `children` CLI in `cli_agent_root/tests.rs` only extracts `--session-id`, accepts only
   `offline root fixture` / `offline child fixture`, and reuses ack/closed files per UUID.
   A resumed wake needs `--resume` UUID support, generated wake input handling and distinct
   per-turn handshake files. Do not mistake fixture EOF/old ack reuse for valid lifecycle.
3. `Wake::text()` appears unused; strict Clippy may flag it. There may be other compile/lint
   issues. No compile result is available.
4. Review Prompt fingerprint compatibility and cross-mode conflicts; duplicate lookup must
   never issue a lease or turn a Prompt into FollowUp. Verify canonical entry/request modes,
   launch CAS checks, removal/permanent receipts and unchanged schema behavior.
5. Prove exactly one wake admission/launch, successful native handoff, changed task digest,
   forged metadata, API-profile refusal, unavailable capacity and lost/dropped receivers.
6. Exercise Stop before/after ticket creation, during parent-busy wait, idle Stop before wake,
   preparation cancellation, disconnect/reapproval/removal/shutdown, parent Unknown and
   restart suppression. No old epoch may regain rights after a later turn or reapproval.
7. Verify original system/tool/resource snapshots cannot widen or be replaced, including root
   resume after intervening activity. Do not flatten historical roles into an invented prompt.
8. Review failure delivery (`woken` means admitted, not completed inference), bounded wait/
   deadlines, worker/actor tracking, lock order and ordered task/input publication.
9. The wake worker currently polls Busy at20ms and hands an owned Start to existing admission;
   capacity failure suppresses rather than adding a general queue. This is not full queue or
   concurrency parity. Job wakes and goal scheduling are NOT implemented by this code.
10. Keep public activation disabled until end-to-end tests and broader qualification pass.

## Remaining whole-objective work (reconciled 2026-10-18)

Child/job wakes, explicit child continuation, inherited context, goal start/scheduling/resume
and independent auditors are implemented privately with offline tests. They still need
installed-CLI and desktop qualification.

- Connect the private goal lifecycle to the existing desktop route with its person-owned
  controls. Preserve safe refusal of dollar allowances and unknown token usage.
- Context/compaction, queues, steering and background delivery across lifecycle boundaries.
- Full concurrency/payload/per-call limits, post-Stop behavior, thinking, attachments and
  aggregate/missing usage semantics. Current conservative bounds are not blanket parity.
- Full concurrent MCP configuration/elicitation qualification.
- Task78: existing desktop setup/model/approval route, correct capability guards, preserved old
  bindings/drafts/history, and actual desktop lifecycle. No dashboards.
- Broader installed-CLI child/job/goal/MCP/Stop/restart and desktop qualification require
  renewed bounded inference allowance and a user-owned rebuild/install when ready. The
  recorded CLI2.1.301/Sonnet fresh/resumed native Write pair already passed.

## Architecture invariants to preserve

- **One inference executor:** installed CLI only for this backend. Generic
  `runtime/subagents.rs::Host` is an API inference/recovery host; do not reuse its runner.
- **Metadata is not authority:** original live registry/grant/Turn/lease, independent
  authenticated channel and exact actual pending call are required for effects. Saved recipes,
  UUIDs, SDK IDs and fingerprints do not mint ownership.
- **Unknown never replays:** keep known effects and permanent UUID/receipt reservations;
  no fallback/reapproval/successor that silently retries uncertain work.
- **Publication -> registry -> storage** lock order. No await under these locks. Do not call
  `Shared::publish_subagents()` while already holding the publication gate: it acquires it.
  Existing `child_view()` reads/publishes under an already-held gate.
- Volatile question callbacks must not publish while holding publication/registry/store locks;
  existing session/question lock ordering matters. Display source is not reply permission.
- Track actors synchronously before returning a waiter, own resources through cleanup, and
  await writes/probes. Cancellation does not undo effects or justify synthesizing success.
- Actual durable revisions only; deduplication emits no new mutation events. A resync can
  replace queued presentation frames. Existing child result views expose answers only for
  `done`; failed/unknown guidance belongs in `note`.
- Root-group instruction/skill cache is a captured snapshot. `Resources::captured()` alone does
  not load it; `Shared::system_sections()` populates it.

## Verification recipe when implementation resumes

First inspect/fix the unfinished wake batch rather than running the old refusal test unchanged.
Use a new owned short directory and retain its path/logs:

```sh
export TMPDIR="$(mktemp -d /home/jason/.pg.XXXXXX)"
cargo fmt --all
cargo check --offline -p pipkin-engine --all-targets
# After updating fixtures/tests:
cargo test --offline -p pipkin-engine cli_agent_root
cargo test --offline -p pipkin-engine store::cli_agent_children
cargo test --offline -p pipkin-engine store::cli_agent_turns
cargo clippy --offline -p pipkin-engine --all-targets -- -D warnings
# Freeze all relevant source before the integrated gate:
cargo test --offline --workspace
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo fmt --all --check
git diff --check
```

Use actual nonzero test counts. `cli_agent_root::child_tests` selects zero tests; the correct
qualified filter is `cli_agent_root::tests::child_tests` (or broader `cli_agent_root`).
`mcp_tests::fixture_bin()` performs a nested cargo build; never change source during it.
The test script writer uses `/bin/sh` -> `exec /bin/cat` to avoid inherited-writable-fd ETXTBSY;
do not add production retries or serialize tests to hide fixture failures.

Frozen source manifest command:

```sh
{ git diff --binary -- crates docs/native-engine-reference.md;
  git ls-files --others --exclude-standard -- crates | sort |
    while IFS= read -r f; do sha256sum "$f"; done; }
```

**This is a diff plus untracked-file HASH manifest, not a complete source backup.** Do not try
to restore untracked files from it or apply it blindly to the existing dirty tree.

## Handoff observations and hashes

At handoff:
- HEAD unchanged; no running test job reported.
- `git diff --check` passed on the current WIP.
- No new compile, tests, Clippy or fmt check was run on the WIP.
- WIP source manifest: `/home/jason/.pg.omyc8L/handoff-wake-wip-source`.
  SHA256 `09de7715b8239e669ec8ab6ac69b2fff62d47a95086b6dee69924d6e1d184296`.

New WIP files:

| File | SHA256 |
| --- | --- |
| `runtime/cli_agent_host/wake.rs` | `59f71bb19c65cd645639eb510e116ddf355ff851d93844e9b69de3863f620b94` |
| `runtime/cli_agent_owner/wake.rs` | `ee64d312d97617668c6a14735d522d3f76cdbdc6e97707ed63db28bcac77863d` |
| `store/cli_agent_children/wake.rs` | `718ddf608f770db6a75aab9ad42fb0e5baef688fc46764d7f677aa0e0ffbf70f` |

These paths are under `crates/pipkin-engine/src/`. They identify handoff contents, not passing
implementations. Existing plan127 file hashes describe360 and may now be stale for touched files.

Live ledgers were rehashed unchanged during handoff:

- `fixtures/claude-cli-probe/2.1.299/runtime-budget.txt`:
  `bf794518e35d7f1ce3a50b3058c4191bb9401e568fc645d77e10b0f404cf1f22`
- `fixtures/claude-cli-probe/unattended-coding-followup.md`:
  `dc0a3eada8f5e9003a57f1e96b04a1875478d456e81e880a6197e7d525cd6519`

Schemas at the last verified milestone: engine29 / protocol41 / desktop10. The wake WIP adds no
schema migration. Do not claim objective completion until all remaining capabilities and the
installed/desktop qualification have objective-wide evidence.
