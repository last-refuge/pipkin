# Scorecard

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

## CLI coding stream rejection and output presentation — 2026-10-19

**Observed:** installed CLI2.1.301 Coding failed when a streamed tool was absent from its
initial inventory. Three earlier tool results were displayed as verbose notices. Stored
diagnostics identify the rejected record class, not the rejected tool name. Exit143 followed
Pipkin's observer-failure shutdown.

**Implemented:** accept valid late CLI-owned observations only in Coding; preserve strict
restricted/native catalogs. Show inert tool activity and thinking collapsed by default,
with expandable literal output, stable identity and offline caching.

**Measured offline:** 2,635 tests passed, 13 ignored, zero failures;
strict workspace/all-target Clippy, formatting and diff checks passed. Final source stayed
unchanged (manifest SHA256 `fdb72d5781a9a353f4b002594049af100b4991dd55d402b3cde66ff56ee5bdb4`). All five prompt-budget files stayed
byte-identical; zero Claude prompts. The decoder regression exercises `ContentBlockStart`
for a late tool and strict-policy refusal. Fake CLI fresh/resume tests complete without
native API fallback. GPUI tests prove collapsed output is excluded from selection until
expanded; offline cache and stable/inert transcript mapping checks pass. An extra blank
line in documentation was corrected and the actual diff check rerun successfully.

**Unverified:** updated installed-CLI behavior and real-window presentation. No old uncertain
conversation was unfenced or replayed; no installed Claude prompts were spent.

## CLI desktop controls — 2026-10-19 (slice35)

**Implemented:** creation-time effort selection; person-launched notification-only CLI children
with fresh/inherited context; active-child steering; root project-scoped MCP management; explicit
Skip-audit. Original connected approvals and frozen child policies remain enforced. Engine
schema32 backs up v31 before adding explicit person-child metadata; protocol41/desktop schema10
are unchanged.

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

**Observed / unverified:** no real-window interaction or installed-Claude inference was performed.
Child steering is tool-boundary delivery; if the child finishes first, non-delivery is recorded
without automatically restarting it. Model/effort stay immutable after creation; effective effort
is CLI-owned. Wider installed-CLI lifecycle and real desktop qualification remain open. No
credentials, running-app configuration, release/install, commit or push were changed.

## CLI desktop activation and routing — 2026-10-19

**Implemented:** explicit Pipkin orchestration choice in the existing Claude setup; immutable
CLI identity and connection-bound approval; durable prompt acknowledgements; images, queues,
steering, withdrawal, compaction RPC and Stop; CLI goal start/resume and native goal settings;
questions, plans, children, jobs, usage and context views. Existing Coding remains separate.
Public lookup cannot reconstruct approval, stale setup cancellation targets its original grant,
and Stop during resource preparation invalidates pending queue/goal intent. Unknown idle
execution cannot advertise Ready. Audit requirement changes preserve existing round limits.

**Measured offline:** **2,626 tests passed, 13 ignored, zero failures** across 48 workspace suite results. Strict all-target workspace Clippy (`-D warnings`), formatting and diff checks passed. Source stayed unchanged throughout the final gate; all five Claude prompt-budget files remained byte-identical. New regressions cover public metadata-only setup,
explicit consent/create/revoke/reapproval, prompt/goal admission before completion, image input,
queue withdrawal and snapshots, original-owner fences, core/adapter capabilities and absence of
API inference. Existing GPUI capability tests include the new execution profile. All five Claude
prompt-budget files remained unchanged; **zero new Claude prompts**.

**Corrected during verification:** fake auth metadata and prompt fixtures were aligned with their
contracts; a historical store assertion now expects the recognized public backend while still
asserting that native API execution is forbidden. The first complete passing run preceded the
final Stop-preparation regression. A later run caught the maximum-size prompt fixture
authenticating before CLI launch; its peer now waits for the turn-specific prompt marker.
Production handshake timeouts are unchanged. The final unchanged-source run supersedes it.

**Historical slice34 remainder:** the four desktop controls are implemented in slice35.
Installed Claude and real desktop qualification remain unverified.
This is source activation, not a release. No installed CLI inference, credential/configuration
changes, running-app interaction, release/install, commit or push. Details: the slice34 entry in
`docs/claude-cli-native-orchestration.md` and `dist/cli-desktop-routing/summary.json`.

## CLI thinking, attachments and usage metadata — 2026-10-19

**Measured offline:** **2,621 tests passed, 13 ignored, zero failures** across 48 workspace suite results.
All 14 new regressions passed in the final workspace run; focused feature and store-journal
runs also passed (13 and 36 checks respectively). Strict workspace/all-target Clippy
(`-D warnings`), formatting and diff checks passed. Source stayed frozen throughout the final
gate; all five Claude prompt-budget files remained byte-identical. **Zero new Claude prompts.**
Regression coverage includes approved effort across resume, frozen child overrides/repeats,
structured image stdin and MCP steering, duplicate/tamper/reopen protection, image-only messages,
queue boundaries, image/frame limits, thinking projection, optional usage subsets, cumulative
metadata separation, and retained completion evidence. Goal accounting still charges the four
inclusive token dimensions once; subset/cumulative telemetry does not add charges.

**Corrected:** the first workspace gate caught malformed stored image JSON being read as an
empty list. CLI input now uses strict bounded image reads before executable admission/launch;
launch-CAS and queued-corruption regressions cover it. The first focused steering assertion
also looked outside the RPC result object; its correction checks structured images and replay.

**Limits / unverified:** model/organization caps may clamp requested effort; unsupported effort
names are refused. Images obey native type/count/size/frame bounds and are queue batch boundaries.
Reported cost is not billed subscription cost. Public desktop routing and installed CLI/desktop
qualification remain; existing live native Write evidence does not qualify these new workflows.
No new Claude prompts, credential/configuration edits, release/install, running-app mutation,
commit or push. Details: `docs/claude-cli-native-orchestration.md` slice33 and
`dist/cli-thinking-attachments-usage/summary.json`.

## CLI concurrency, payload and post-Stop behavior — 2026-10-19

**Measured offline:** ten new regressions passed for bounded parallel tools, sequential
barriers, queued/active per-call cancellation and exact replay, forced cleanup, Stop joining
parallel workers, native-size UTF-8 prompts and tool payloads, structured image results,
response-envelope space, inert larger SDK echoes, and concurrent steering delivered once.
The expanded pre-launch Stop test proves a new explicit fresh turn after rejection. The full
workspace gate passed **2,607 tests, 13 ignored, zero failures** across 48 suite results. Strict
all-target Clippy, formatting and diff checks passed. Source stayed frozen and all five Claude
prompt-budget files remained byte-identical, including comparison with the prior milestone.

**Corrected:** the new 2 MiB prompt test exposed a leftover 1 MiB admission cap. The final
focused run passed after aligning all orchestration prompt boundaries with the native limit.

**Limits / unverified:** four workers per bridge, finite byte/count/journal/display ceilings;
see slice32 for exact limits. Safe pre-launch and clean-idle Stop allow new explicit user
input, while launched uncertain turns keep their effects and remain non-resumable. Installed
Claude concurrent MCP behavior, larger-payload and desktop lifecycle qualification remain
unverified. Public activation, thinking, attachments and other usage metadata remain pending.
Zero new Claude prompts; no release/install, running-app mutation, commit or push.
Details: `docs/claude-cli-native-orchestration.md` slice32 and
`dist/cli-concurrency-payload-stop/summary.json`.

## CLI aggregate background usage and goal allowances — 2026-10-19

**Implemented privately:** causal goal accounting includes root turns, children, auditors,
continuations and parent/child job wakes, deduplicated by native run receipt. Late usage stays
with the original goal. Absolute reconciliation survives recovery and cannot lower an already
recorded minimum. Scheduled turns and autonomous wakes consume worker-turn allowance; all
reported input/output/cache token dimensions contribute to the token allowance. Admission and
launch recheck current state, identity and limits. Completion waits for clean terminal evidence
and reads reconciled totals atomically; uncertain execution pauses provisional completion.

**Measured offline:** all nine new accounting regressions and the existing eight goal-lifecycle
tests passed in the full workspace run: **2,597 passed, 13 ignored, zero failures** across
48 suite results. Strict all-target Clippy, formatting and diff checks passed. Source stayed frozen
and all five prompt-budget files stayed byte-identical. Tests use owned fake-CLI/native MCP
peers, isolated stores and scripted providers. Zero Claude prompts.

**Corrected during verification:** a capacity-reservation fixture race, completion returning a
stale accounting snapshot, and fixture authentication starting before process launch. Final
workspace evidence supersedes the retained failed development runs; production timeouts are
unchanged.

**Limits / unverified:** token limits gate new invocations and can be exceeded by already
admitted concurrent work; missing settled usage blocks further token-limited work. Dollar
limits remain refused because trustworthy dollar usage is unavailable. Installed Claude and
desktop qualification, broader concurrency/payload/post-Stop parity, thinking/attachments and
other usage metadata, and public setup/model routing remain. Public activation is disabled.
No release/install, running-app mutation, commit or push. Details:
`docs/claude-cli-native-orchestration.md` slice31 and
`dist/cli-background-usage/summary.json`.

## CLI background delivery integration — 2026-10-19

**Measured offline:** four new regression tests cover eight scenarios: queued-user input
before ready wakes, ready-order delivery and goal scheduling priority; a job outliving its
launching turn's deadline; spawned-job cleanup before receipt commitment (wake on/off); and
same-UUID child job continuation with parent notification/wake, child Stop and parent
revocation. Child system text remains byte-identical and inherited context remains available.
API fixture providers received zero requests. The full workspace gate passed **2,588 tests,
13 ignored, zero failures** across48 suite results. Strict all-target Clippy, formatting and
diff checks passed. Source remained frozen; all five Claude prompt-budget files were unchanged.
Zero Claude prompts.

**Unverified / remaining:** installed Claude and desktop background lifecycle, broader parity,
aggregate background-descendant usage/goal allowances and public setup/model routing. Public
activation remains disabled. No release/install, running-app mutation, commit or push.
Details: `docs/claude-cli-native-orchestration.md` slice30 and
`dist/cli-background-integration/summary.json`.

## CLI mid-turn steering and queue batching — 2026-10-19

**Measured offline:**40 focused input-related tests and the full workspace gate passed:
**2,584 passed,13 ignored, zero failures**, plus strict all-target Clippy, formatting and
diff checks. Eight new regressions cover same-kind one/all batches, exact escaped input,
per-message outcomes, authenticated tool-boundary steering and response replay, withdrawal,
capacity waits, compaction barriers, oversized-message deferral, lost-connection uncertainty,
old-approval/orphan-intent refusal and atomic admission rollback. Original native tool output
is preserved separately from the supplemental user block. Source stayed frozen; all five
Claude prompt-budget ledgers were unchanged. Zero Claude prompts.

**Unverified:** installed Claude's response to the supplemental steering block, installed
batch execution and desktop behavior. Steering waits for a native tool-result boundary or
the next clean turn; it does not interrupt token generation. Public activation remains gated.
No release/install, app restart, commit or push. Details and evidence:
`docs/claude-cli-native-orchestration.md` slice29 and
`dist/cli-steering-batching/summary.json`.

## CLI context, compaction and queued inputs — 2026-10-19

**Measured offline:** seven focused native-MCP/fake-CLI tests passed for follow-up FIFO,
queued steering priority, same-UUID manual compaction, required boundaries, forbidden tool
effects, exact duplicate lookup, withdrawal during capacity wait, Stop, revocation and
recovery suppression. Context reports per-model-request input/cache-input counts, distinguishes
them from cumulative usage, and leaves capacity and post-compaction size unknown. An additional
regression covers a completed receipt later fenced as uncertain. Existing committed-order tests
caught the generic zero-revision context notification; the CLI publication now uses its committed
revision under the publication gate, without taking the session registry lock.

**Scope:** queued steering runs at the next clean CLI turn boundary, ahead of unreserved
follow-ups. Mid-turn steering and native API-style input batching remain unimplemented here.
Public orchestration remains disabled. Installed compaction behavior under the exact launch
policy, automatic compaction observations and desktop presentation remain **unverified**.

**Final verification passed:** **2,576 workspace tests passed, 13 ignored, zero failures**
across 48 suite results. Strict all-target workspace Clippy (`-D warnings`), formatting and
diff checks passed. The six existing committed-order regressions pass unchanged, and the
fenced-history context regression passes. Source remained frozen across the final gate;
all Claude budget ledgers remained byte-identical. **Zero new Claude prompts.**
Source-manifest SHA256 (Rust/SQL/manifests and compiled reference):
`dcec23111994a25ef20364480b4c9dd024c3b15cfb8d06bf6c2172d25f748a64`.
Evidence: `dist/cli-context-inputs/{summary.json,workspace.log,clippy.log,fmt.log,diff.log,source-before.json,source-after.json,budgets-before.json,budgets-after.json}`.
 Earlier failing focused and
workspace logs are retained separately; they are not the final verification result.
Zero additional Claude prompts; no release/install, running-app change, commit or push.
See slice28 in [the capability checklist](claude-cli-native-orchestration.md) and
[the handoff](claude-cli-orchestration-handoff.md).


## CLI goal lifecycle and independent auditors — offline passed 2026-10-18

**Measured offline:** explicit start/resume, atomic goal-turn scheduling, same-UUID worker
continuation, limits and stalls, Stop/lost approval/Unknown refusal, final-line fallback
isolation, mechanical evidence and fresh independent CLI auditors. Read-only catalogs and
native inspection receipts are enforced. Tests cover duplicate proposals, repair followed by
a second audit, stale/malformed/inconclusive results, and cancellation during an audit.

CLI goal usage comes from actual turn receipts, with no double charge for auditors. Missing
usage remains unknown and pauses token-limited goals; dollar-limited goals pause before
admission because the receipt contract has no cost measurement. Public desktop goal routing
and broader usage/concurrency parity remain part of the gated activation work.

**Passed:** 40 focused goal tests; final offline workspace gate **2,568 passed, 13 ignored,
zero failed** across 48 suite results; strict all-target workspace Clippy, fmt and diff checks.
Source remained unchanged across verification, and all Claude prompt ledgers stayed identical.
Source SHA256: `f0729d9b6c4ff024956d1d29e93254f35cce8d882f14a803136bd364dcec5182`.
Evidence: `dist/cli-goal-lifecycle/summary.json` and adjacent logs.

**Unverified:** installed-CLI goal/auditor behavior and desktop orchestration qualification.
Zero new Claude prompts; no installed Claude inference, release/install, running-app change,
commit or push. Public orchestration remains disabled. See slice27 in
[the capability checklist](claude-cli-native-orchestration.md) and
[the current handoff](claude-cli-orchestration-handoff.md).

## Claude CLI child continuation and inherited context — 2026-10-18

**Measured offline:**same-UUID child continuation under the original parent approval, including
inline/notify/wake delivery; frozen policy/context across separate parent turns and changed
profiles; refusal of busy/foreign/unknown children, overrides and lost approval; mandatory
native context read before effects/completion; latest-turn recovery and no API fallback.
The bounded context snapshot survives Unicode and worst-case JSON escaping intact, verified
with a failing-before/passing-after tool-harness regression.

Final offline workspace gate: **2,559 passed,13 ignored, zero failed** across48 suite results;
strict all-target Clippy, fmt/diff, frozen-source equality and unchanged Claude budget ledgers.
Evidence: `dist/cli-child-context/summary.json` and adjacent logs. Source SHA256:
`086435b9d1eb44bfae97209b8bbbf4363c1cdc7252b6730b6ef74de60243eba7`.

**Unverified:**installed-CLI child/context behavior and desktop orchestration qualification.
Zero new Claude prompts; no installed CLI process, app restart/install, commit or push.
Public orchestration remains disabled. Details: slice26 of
[the capability checklist](claude-cli-native-orchestration.md) and
[the current handoff](claude-cli-orchestration-handoff.md).

## CLI native-MCP fresh/resume — live passed 2026-10-18

- **Measured live:** installed CLI2.1.301/Sonnet; fresh native Write then same-UUID resume.
  Both roots Completed, both bridges closed, two completed native receipts, actual final file
  `native-resumed`, exit0/reaped/drained/projected for both. Zero API-provider requests.
- **Allowance:** exactly two newly authorized prompts used; no further prompts authorized.
  Earlier ledgers unchanged; all three earlier Unknown roots preserved without replay.
- **Evidence:** `dist/cli-sigint-live/live-summary.json`, `live-direct.log`, retained
  `live/probe-veABjp/`. Cargo-injected loader-path preflight failure retained separately;
  it used no prompt. The directly launched compiled test passed the normal preflight.
- **Offline gate passed:** 2,555 tests, 13 ignored, zero failures across 48 suite results;
  strict all-target workspace Clippy, fmt/diff and source equality. Production code is the
  previously qualified SIGINT fix; only the ignored test harness changed. Source SHA256:
  `04d392360004d816534d29a25ba151300d6f041f4d76d2ab8e83ca096417cb15`.
- **Scope:** fresh/resumed native Write only. Broader lifecycle, services/parity and desktop
  qualification remain; public orchestration stays disabled. No release/install/app changes.

## CLI MCP SIGINT closure — 2026-10-18

- **Identified:** installed CLI2.1.301's Unix MCP cleanup starts with SIGINT. The helper only
  caught SIGTERM, so SIGINT killed it before the authenticated close handshake. Installed
  executable inspected read-only; no CLI process, credential inspection or inference.
- **Reproduced failure:** actual helper + native Write + exact-owned-PID SIGINT exited with
  signal2. `dist/cli-bridge-sigint/before-fix.log` retains the pre-fix failure.
- **Fixed and measured offline:** helper-only SIGINT/SIGTERM registration before connection;
  existing close handshake and independent model-terminal/Stop checks preserved. Six actual-
  helper tests pass, covering SIGINT with/without terminal and Stop, EOF/SIGTERM, malformed
  partial input and forged controls. `after-fix.log`.
- **Full verification passed:** 2,555 tests, 13 ignored, zero failures across 48 suite results;
  engine1594/app337/UI217, all remaining suites, strict all-target Clippy, fmt/diff and source
  equality. Logs: `dist/cli-bridge-sigint/`; SHA256
  `41ccf583ea2b1e5cc9bd712e904cf34c062d0979df511608fc93dad2d29ee8cc`. All prompt ledgers unchanged.
- **Still unverified:** installed fresh/resume lifecycle after this fix; no prompts remain.
  All prior live roots stay Unknown, no replay. Public orchestration remains disabled.

## CLI native-MCP live qualification — 2026-10-18

- **Observed live:** all three installed CLI2.1.301/Sonnet fresh attempts invoked native Write;
  each isolated project contains `native-first` and a completed native tool receipt.
- **Failed live:** all three roots became Unknown on tool-channel loss. No resume was sent;
  no replay or API fallback. **All three authorized prompts are spent.** The prepared resume
  ledger is unauthorized. Old unused ledger slots were superseded, not additional permission.
- **Passed offline:** hidden system-file flag metadata preflight (15 tests); authenticated
  early-input-close ordering (actual helper4, driver9, root1 with two exit cases, helper7).
  Dedicated helper SIGTERM handling also passed actual-helper4 and helper7. These tests do
  not identify or resolve the remaining installed-CLI failure.
- **Workspace gate:** the early-close checkpoint (`close3-*`) passed engine1593 (3 ignored),
  app337 (1 ignored), UI217, all remaining suites, strict all-target Clippy and fmt/diff;
  source SHA256 `38fedce6b4a3508331cb260cf5ccb4d0e4d68b79d5d13c0c3ae64c87a42640d3`.
  The later `signal-*` gate exposed an exporter health revision race (engine1592 passed,
  1 failed); remaining suites passed. The race fix passed nine focused tests, then the final `verified-*` gate: **2,553 passed,
  13 ignored, zero failed** across48 suite results (engine1594, app337, UI217); strict all-target
  Clippy, fmt/diff and frozen-source equality passed. Final source SHA256:
  `33a4ee1fc9123e5a54414b760dbb8308689c548a0b7d936c93a8331d269b36a3`.
- **Remaining:** SIGINT cause/fix is recorded above; qualify complete fresh/resume lifecycle and the wider
  feature checklist below. Public orchestration remains disabled. Logs and isolated evidence:
  `dist/cli-orchestration-resume/`; allowance history:
  `fixtures/claude-cli-probe/native-orchestration-probe.md`.

## CLI child/job wakes — resumed 2026-10-18

- **Implemented privately:** one-use connected approval tickets; same-UUID parent resume;
  atomic FollowUp/run/delivery admission; idle Stop epoch revocation; bounded capacity wait;
  original system/tool/resource snapshots; tracked job completion and restart suppression.
  Completion text/digest capture shares the publication gate. No API inference fallback.
- **Measured:** six child workflows (including seven wake/Stop/busy/capacity/reapproval cases),
  three job workflows (including wake/Stop/revocation/restart), and 29 root-storage tests.
  Storage tests cover rollback, duplicate lookup without launch authority, Prompt fingerprint
  compatibility, cross-mode conflicts and changed/corrupted completion evidence.
- **Passed:** frozen offline workspace engine1588 (2 ignored), app337 (1 ignored), UI217 and
  remaining suites; strict all-target workspace Clippy, fmt/diff/source equality. Logs and
  manifests: `dist/cli-orchestration-resume/`; source-manifest SHA256
  `c000b16f9d7c5ad7328bbfa7ecb316af86497e5e2ecac1d9e540a1ae549070b2`.
- **Remaining/unverified:** explicit child continuation; goal scheduling/auditors; inherited
  context; compaction/queues/steering; broader concurrency/payload/thinking/attachment/usage
  and MCP concurrency parity; public desktop activation and installed lifecycle qualification.
  No installed inference, release/install, running-app changes, commit or push. Prior live
  allowance remains exhausted. Schemas unchanged; public orchestration stays disabled.

## Background CLI child notifications — integrated milestone360

- **Implemented privately:** fresh background, notification-only children through the owned
  CLI runtime. Original connected approval survives normal parent completion. Unconfirmed
  delegation is cancelled on parent cancellation; only actual successful native tool receipts
  establish handoff. Child Stop/revocation/disconnect/shutdown still cancel owned work.
  Atomic running-task admission, actual receipt-based outcomes, failed-result notes and
  cleanup-only startup reconciliation reuse the existing task views. No API runner or wake.
- **Measured:**3 added real-engine workflows plus1 storage test; simulated CLI/SDK and real
  native Write. Parent completion before child tool activity, exact replay/2 process launches,
  Stop before handoff, child cancellation/Unknown, retained effects, result/wait/foreign reads,
  task phase publication/resnapshot and orphan/lost-settlement classification.6 total child
  workflows and12 storage tests passed359; zero model-provider requests.
- **Passed360:** frozen workspace engine1584/app337/UI217; strict all-target workspace Clippy,
  fmt/diff/source/live-ledger equality. `/home/jason/.pg.omyc8L`; snapshot
  `5cd4c5ffad1dccc5509611a426741142c025fead444885235dac7cb14a740c69`.
  Retained354–359 failures document a bad test filter, lint errors, incorrect resync/result-view
  assertions, and an existing adapter-test event/counter race. The race fix passed5 focused
  repeats and360; production behavior was not changed to accommodate it.
- **Incomplete/unverified:** wake/continuation, goal scheduling/audits, inherited context,
  context/queues/steering, full concurrency/post-Stop/payload/usage parity, public activation and
  installed qualification. Background work remains connection-bound; another generation slot
  is needed. No installed inference, release/install, app mutation or repository commit/push.
  Schema29/protocol41/desktop schema10; public disabled, live allowance0.

## Native CLI jobs and goal reporting — integrated milestone353

- **Implemented privately:** owned job startup through the existing process supervisor,
  status/log/wait/cancel, cleanup-only recovery; scoped goal get/update/evidence proposal,
  expected-view comparison and matched-run Stop pausing. Job wake and required goal audits
  explicitly refuse; no API inference host is substituted.
- **Measured:**4 real-engine workflows with simulated CLI/SDK, real owned processes and
  file evidence. Replay, forged/cross-service/expired authority, dropped startup waiter,
  Stop before spawn, foreign jobs, cleanup-only recovery, observation-only evidence refusal,
  actual completion evidence and required-audit refusal. Zero model-provider requests.
- **Passed353:** frozen workspace engine1580/app337/UI217, strict all-target workspace Clippy,
  fmt/diff/source/live-ledger equality. `/home/jason/.pg.E47DhW`; snapshot
  `c3da7195d4b644c5b48674b42d7826c7d784626c08062557ab1c930c05a561de`.
  351 passed the initial job tests;352 passed49 root tests and strict engine Clippy.
- **Still incomplete/unverified:** private seeded goals are not goal start/resume/scheduling;
  CLI-only auditors, wake/continuation, background children, inherited context, context/queues/
  steering, full limits/post-Stop/payload/usage parity, desktop activation and installed lifecycle
  qualification remain. Schema29/protocol41/desktop schema10; public disabled, live allowance0.
  No release/install, running-app mutation, repository commit/push or installed inference.

## Native CLI service batch — integrated milestone350

- **Implemented privately:** MCP search/describe/call/resources/read/prompts/render, per-call
  MCP form elicitation, web search/fetch/read, child-directory reads and bounded native tool
  progress. Existing domain services are reused, not an API inference host. Public execution
  guards remain; native web retention/publication has original pending-call authority.
- **Measured:**3 real-engine workflows with simulated CLI/SDK and owned local MCP/HTTP fixtures.
  Native MCP counter mutation, actual web retention, resources/prompts, form answer/Stop,
  uncertain remote effect preserved without replay; forged/cross-service/expired hosts rejected.
  Replay leaves counter1/HTTP requests2. Progress bounded to32KiB and rejected after settlement;
  source revisions ordered. Empty/foreign directory lookups covered. Zero model-provider requests.
- **Passed350:** frozen workspace engine1576/app337/UI217; strict all-target workspace Clippy,
  fmt/diff and source/live-ledger equality. `/home/jason/.pg.qnVGJw`; snapshot
  `7219ea1bc99a8a9121ce62185590ee19c652a68564417f9a7c36a5257cfaa004`.
  348 compile errors retained;349 targeted workflows and strict engine Clippy passed.
- **Unverified/incomplete:** installed CLI/external MCP/desktop qualification, complete concurrent
  MCP configuration/elicitation parity (ambiguous scoped forms decline), jobs/goals, background/
  wake/continuation, inherited context, context/queues/steering, full limits/post-Stop/payload/usage
  parity and public setup. Child task running presentation remains; native tool progress is now
  wired. Schema29/protocol41/desktop schema10; public disabled, live allowance0. No app mutation,
  installed inference, packaging/install, commit or push.

## Working inline CLI children — integrated milestone347

- **Implemented privately:** original-parent authorized fresh inline child runs through the
  CLI root runtime, native tools, result/task publication and tracked cleanup. No API runner.
  Root resource-cache snapshot and captured helper image are shared; child tool/system policy
  is frozen and checked. Saved metadata never creates a launch/continuation grant.
- **Measured:**3 real-engine tests with simulated CLI/SDK peers: actual native Write and result
  return, exact MCP replay without another child, parent Stop preserving Write/child Unknown,
  and API-profile refusal before child inference. All observe zero API provider requests.
  This is not installed CLI or actual desktop qualification.
- **Passed347:** frozen workspace engine1573/app337/UI217; strict all-target workspace Clippy,
  fmt/diff, source/live-ledger equality. `/home/jason/.pg.cTikc9`; snapshot
  `b5ab826af62afa6c664381170964ead2716a28fdf893f5f45524db03302474a9`.
 344 compile visibility error retained;345 initial workflow/root tests passed but Clippy caught
  a private-interface warning;346 retained missing fixture settings directory;347 passed after
  correction. No running-app, installed CLI, release/install or commit/push actions.
- **Remaining:** background/wake, child continuation/inherited transport, jobs/goals/web/MCP/
  progress services, context/queues/steering, full limits/post-Stop parity, public setup and live
  qualification. Inline children require an available generation slot; running child task
  presentation still needs progress integration. Schema29/protocol41/desktop schema10;
  public orchestration disabled and live allowance0.
- **Process change requested by user:** capability checklist (in the orchestration document),
  end-to-end implementation first, existing-service reuse in batches, focused checks during
  development and broad checks at integrated milestones/handoff—not more metadata-only slices.

## Native CLI child policy/metadata — private staging through343

- **Implemented, not a child launcher:** CLI-only profile/model selection and frozen tool,
  instruction/skill-listing, caps and delivery policy. API-selected profiles/models and thinking
  fail closed. Roleful bounded inherited context is captured, but launch conversion refuses it
  rather than flattening history or silently choosing fresh. Runtime group-cache integration
  and independently owned child launch are still absent; native subagent service still refuses.
- **Schema29:** atomic exact-native-intent child metadata (binding, permanent UUID ownership,
  task, policy, revision). Duplicate lookup grants nothing; detached bindings cannot be adopted.
  Large immutable policy snapshots live outside task-list rows. The reader bounds before loading
  and verifies hashes; removal preserves permanent UUID reservations. Migration28→29 and backup
  preservation were tested only on owned disposable databases, not the running app's database.
- **Passed343:**19 new tests (8 policy/11 storage), frozen offline workspace
  engine1570/app337/UI217, strict all-target workspace Clippy, fmt/diff, source/live-ledger
  equality. `/home/jason/.pg.wocULQ`; snapshot
  `5bc6d1f425b5585ce69969a70decd9cf165bff4cdcf437a9ace4e635a09aea6c`.
 340 initial unit gate passed;341 retained fixture Result-alias compile failure
  (`/home/jason/.pg.h4fiP9`);342 targeted policy/storage/Clippy passed. Policy/store fixtures
  simulate journal authority: no child inference, installed CLI or desktop qualification claim.
- **Next:** original-parent child owner grants and tracked admission/cleanup; launch/continuation,
  shared runtime snapshot capture, completion/wake; remaining87 services and77 workflows, then78
  public setup/qualification. Public orchestration stays disabled. No dashboards, live inference,
  installation, running-app mutation/restart, native window input, commit or push. Protocol41,
  desktop schema10 and live allowance0 unchanged.

## Native CLI orchestration — scoped plans and questions (task87 partial)

- **Implemented privately through335:** original-turn service authority and an engine-owned
  per-call ToolHost. Each service joins exact authenticated pending native intent, validates
  call cancellation/deadline and preserves publication → registry → storage ordering. Native
  plan read/update and checkpoints work without API hosts. Duplicate receipts do not repeat
  plan effects; Stop before/after an effect preserves known state and Unknown receipts.
- **Implemented through339:** native question calls use the existing question UI/RPC contracts.
  A bounded volatile grant is installed for the exact question ID before publication. Forged
  source labels, foreign/stale IDs and retired scopes cannot authorize replies. Only the exact
  live response is exempted from the native-only RPC guard; other mutations stay guarded.
  Drop/Stop/removal/disconnect/shutdown withdraw questions; CLI recovery never re-asks them.
- **Passed:**335 added4 scoped-host/plan tests.339 adds6 question tests: real-engine answer and
  decline, duplicate lookup, forged-source/late reply rejection, Stop/removal/disconnect/shutdown,
  and unit registration ordering/refusal. Actual plan/question/SQLite services, simulated CLI/MCP;
  no native provider inference. This is not installed CLI or actual desktop qualification.
- **Gate339 passed:** frozen offline workspace (engine1551/app337/UI217), strict all-target
  workspace Clippy, fmt/diff, source equality and unchanged live ledgers. Artifacts
  `/home/jason/.pg.ycvYCj`; snapshot
  `ded3fca48bedc2e7b32966525d685ceefa98065d1de39a9539ecfa9bd4773064`.
  Previous335 `/home/jason/.pg.MWuwT6`, snapshot
  `9c88187392da31b882e3295f6142786efa7467efce357dd28d0d5c6994211df3`.
- **Retained development failures:**332 missing error propagation;333 exposed the updater's
  legitimate plan read-back service bundle;334 passed after correction.336 caught a test RPC
  field typo;337 targeted tests passed.338 was interrupted when the engine stopped during
  compilation (`/home/jason/.pg.Trxffq`);339 checked identical source before the rerun. No engine
  or running-app restart was performed by this work.
- **Still incomplete:** CLI-only child creation/continuation/profile/snapshot ownership, jobs/
  goals/web/MCP host services, volatile progress, context/queues/steering/limits/post-Stop parity,
  public setup/model route and installed CLI/existing-desktop qualification. Backend remains
  Unsupported. No dashboards, live inference, installation, native window input, credential
  access, commit or push. Schema28/protocol41/desktop schema10 unchanged; live allowance0.

## Native CLI orchestration — owned admission/preparation (task86 complete; private foundation)

- **Implemented privately:** `cli_agent_runtime/admission.rs` synchronously moves an approved
  original Turn/request into a tracked engine task before any durable admission. A dropped
  result receiver cannot abandon preparation. Exact receipt lookup grants no new lease; changed
  input conflicts and Unknown stays fenced. Capacity is reserved before committing fresh input.
- Admission, active SessionRt attachment and initial input/run publication share the session
  lock/publication gate. Input is visible and stoppable before listener/asset preparation; other
  native commits cannot overtake it. Runtime inherits the same lifetime and capacity, without
  duplicate publication. The already-admitted spawn seam is now test-only.
- Listener/asset writers are awaited through cancellation/deadline cleanup, not detached before
  retirement. Normal preparation failures retire the original lease and release active state;
  unconfirmed settlement fails closed. Original independent owner/launch/receipt checks remain.
- **Passed:**7 new real-engine tests cover lost replies and exact lookup with exhausted capacity,
  invalid assets/catalog, cancelled lock wait, capacity refusal, conflicting duplicate input,
  actual Write then Stop/Unknown lookup, ordered early input publication, durable Stop during
  preparation, and removal during preparation. Fake CLI/SDK/MCP, real native effects/SQLite.
- **Gate331 passed:** frozen offline workspace (engine1541/app337/UI217), strict all-target
  workspace Clippy, fmt/diff, source equality and unchanged live ledgers. Artifacts
  `/home/jason/.pg.AtFFcd`; snapshot
  `470c8c7e4e9da7d7c4647b468d0d0d86bcf8900f4a496c9abf6134e9da6b4f77`.
  328 (`/home/jason/.pg.YAds8I`) caught an unused staged re-export in Clippy;329 passed the earlier
  builder slice (`/home/jason/.pg.HYnM5a`);330 targeted checks passed (`/home/jason/.pg.vxDBoa`).
- **Still not delivered:** full ToolHost/CLI-only children, progress/context/workflows, queues/
  goals, payload/concurrency/per-call/post-Stop parity, approved public setup/model route and
  existing-desktop activation/qualification. Task76's private bridge/driver foundation is complete,
  not the whole goal. Backend remains Unsupported; actual installed SDK lifecycle is unqualified.
  No dashboards, live inference, release/install, running-app mutation, credential access, native
  UI input, commit or push. Live allowance remains0; schema28/protocol41/desktop schema10 unchanged.

## Native CLI orchestration — ordered committed text/tool publication (gate327)

- **Implemented privately:** SDK display projection and authenticated native-tool intent/result
  commits now share the same commit-through-delivery gate as `Shared::commit`. Scope/current-run
  checks precede writes; no async wait occurs under the publication gate. Native tool updates
  contain the actual committed entry/revision; deduplicated receipts emit no duplicate card.
  CLI stdout tool records remain inert. The publisher itself grants no execution authority.
- The SessionRt actor publishes committed input/run state before starting producers, streams
  text and native call/result entries through existing session events, and publishes successful
  settlement without a full snapshot refresh. Error retirement requests reconciliation once.
  Existing bounded client delivery handles slow consumers. This is commit order, not a claim
  to reconstruct cross-channel SDK causal history.
- **Passed:**5 new tests:3 gate/scope/revocation/failure/order tests,1 exact committed receipt/
  replay test, and1 real-engine wire test. The latter holds the simulated MCP Write until the
  client receives the first text delta, interleaves an ordinary native transaction, then observes
  native call/result, remaining text and settlement in revision order. Its event-built transcript
  equals stored history; no success-path resync or duplicate SDK tool card occurs. Actual native
  Write happens once; no provider inference is used.
- **Gate327 passed:** frozen offline workspace (engine1534/app337/UI217), strict all-target
  workspace Clippy, fmt/diff, identical source and unchanged live ledgers. Artifacts
  `/home/jason/.pg.XXBvXW`; snapshot
  `3953d483db481546b516d3df59ce98bfa8f27fb9a90a970436a9093764989977`.
 324 (`/home/jason/.pg.3l8lwY`) caught an early-return tuple mismatch.325
  (`/home/jason/.pg.3IA863`) exposed an invalid synthetic stream that skipped block0; the strict
  decoder correctly refused it. The fixture now emits contiguous blocks;326 targeted checks
  passed (`/home/jason/.pg.qPClRP`). No production decoder weakening or replay was added.
- **At gate327 (owned handoff now superseded above):** task86 still consumed an already admitted
  lease. Full ToolHost/CLI-only children, volatile progress,
  context/workflows, queues/goals and public desktop activation/qualification remain. These
  tests simulate CLI/SDK/MCP behavior; they do not qualify the installed CLI or desktop UI.
  Backend remains Unsupported. No dashboards, live inference, release/install, running-app
  mutation, native UI input, credential access, commit or push; live prompt allowance remains0.

## Native CLI orchestration — private SessionRt lifetime (gate323)

- **Implemented, private partial slice:** `runtime/cli_agent_runtime.rs` synchronously hands
  an already admitted root to an engine-owned task. Dropping its result waiter does not abandon
  execution/cleanup. Tracking begins before scheduling and covers original resources, native
  cleanup, durable retirement and active-slot release; foreign registries cannot adopt roots.
- The actor attaches SessionRt active/cancellation state, takes engine generation capacity,
  shares native process/FS mutation resources, and cancels lock-waiting handoffs on Stop,
  disconnect, shutdown, removal or deadline. Capacity refusal retires admission without launch.
  Active ownership is released before in-process settlement notification. Unconfirmed durable
  settlement fails the runtime closed instead of broadcasting fabricated success.
- Backend Stop recognizes a live independent orchestration reservation, signals first, and
  persists Stop intent for its attached run. It does not borrow native API recovery, goals,
  compaction or queue execution. Engine shutdown tracks this actor; removal waits for its
  lifetime before deleting payloads. Stored identity alone still cannot execute anything.
- **Passed:**8 new real-engine tests with an owned fake CLI/authenticated peer and actual native
  Write: dropped waiter, active state/Stop/Unknown, capacity refusal, foreign registry, shutdown,
  removal, actual connection drop and cancellation while the session admission lock is held.
  No native provider inference is used. These are not installed CLI/SDK qualification.
- **Gate323 passed:** frozen offline workspace (engine1529/app337/UI217), strict all-target
  workspace Clippy, fmt/diff and unchanged source/live ledgers. Artifacts
  `/home/jason/.pg.V84zEV`; snapshot
  `4e87e9449f22a08ee074916bded5ec81d230f7b81836b75e7c5c955f27162b20`.
 321 targeted lifecycle checks passed (`/home/jason/.pg.eK8qO9`);322 caught an incorrect tracker
  token import (`/home/jason/.pg.sor8uQ`), corrected before323. Failures remain retained.
- **Not complete:** task86 stays active. This entry consumes an already admitted lease; approved
  creator/admission handoff and revision-ordered live text/native-tool publication remain.
  The bounded display lane is deliberately drained, not published out of order; terminal
  reconciliation is requested once, not a full snapshot per token. This is not live streaming
  or desktop parity. Full CLI-only ToolHost/children/context/workflows and public activation
  remain; backend still Unsupported. No dashboard work, installed CLI inference, release/install,
  running-app mutation, UI input, credential access, commit or push. Live prompt allowance is0.

## Native CLI orchestration — independent revocable ownership (goal6, task85)

- **Implemented, still private:** a distinct Linux/master/host opt-in (default false), bounded
  connection grants, approved immutable creation and explicit return. Coding checks and stored
  bindings cannot authorize this route; duplicate creation is lookup only. Fresh UUIDs retain
  permanent ownership. Unknown/fenced/foreign history cannot receive return/start authority.
- The root coordinator requires a noncloneable turn owner and original matching lease, performs
  fresh native-policy checks each turn, and includes metadata probes in its absolute deadline.
  Cancellation awaits probe/process cleanup. Reservations remain held through cleanup, not merely
  until Stop is requested. Connection lifetime cancellation propagates without a registry lock;
  shutdown signals before acquiring it. Disconnect, removal and shutdown hooks are wired.
- **Passed:**8 new ownership/deadline regressions plus1 fixture-writer regression. They cover
  actual engine connection/opt-in handling; Coding rejection; lookup/reconnect/identity/lease
  fences; Stop, disconnect, session revocation, shutdown and connection-token cancellation of
  an owned fake root after **real native Write**. Effects stay once, receipts stay Unknown,
  and reapproval cannot replay them. A timed-out metadata subprocess is observed reaped before
  any inference launch. New fixtures simulate the SDK/MCP peer, not the installed CLI.
- **Gate320 passed:** frozen offline workspace (engine1521/app337/UI217), strict all-target
  workspace Clippy, fmt/diff, identical source and unchanged live ledgers. Artifacts
  `/home/jason/.pg.W0dByz`; snapshot
  `67028b434387d07601505c3eddaf412046bebb29542a79166046e0ba346d4497`.
- **Failed attempts retained:**317 (`/home/jason/.pg.eLk7QJ`) caught pinned-future borrow/drop
  ordering, corrected.318 (`/home/jason/.pg.Q1eeek`) passed new ownership cases but one old
  fake-executable setup hit ETXTBSY. Affected script fixtures now publish through an isolated
  writer process, so concurrent forks cannot inherit a writable script fd from the test process.
 319 (`/home/jason/.pg.jRPvVU`) passed all new cases but rejected an existing large fixture at
  the writer's initial argv-size bound; stdin publication fixes that setup limitation. No
  production retry, reduced test parallelism or inference-replay exception was added.
- **Still absent:** approved public setup/model route, SessionRt task ownership/live publication/
  settlement/Stop integration, full native ToolHost and CLI-only children, context/workflows,
  queues/goals and desktop qualification. The registry is not that integration; the backend
  still publicly decodes Unsupported. Installed SDK inventory/close ordering remain unverified.
  Zero live inference prompts remain. No dashboards, installed CLI execution, release/install,
  running-app changes, native UI input, credential access, commit or push.

## Goal Resume recovery — unconfirmed request (task84)

- **Observed:** the user's Resume clicks had no corresponding state transition in the bounded,
  read-only goal6 ledger; the goal remained paused for missing status, with no configured limits.
  This does not expose the running UI's memory or prove its exact failure path.
- **Source defect repaired:** a pending start/resume receipt silently discarded Resume. Core now
  requests a fresh attachment/status snapshot instead of resending an uncertain command. The
  banner labels that action **Check status**; loading and failed reads have visible feedback.
  A new explicit Resume is required after the status resolves. Existing scope/receipt fences
  remain; no ordinary same-generation projection or stale receipt releases an uncertain start.
- **Passed:**3 core regressions and1 native desktop-adapter e2e with deliberately dropped start
  receipts. Check status sends no model prompt; a later explicit Resume starts exactly one new
  turn, preserving conversation identity and draft. Read failure/retry and stale session/
  generation handling are covered. The refresh uses the existing explicit reattachment path,
  not a new transcript renderer. This regression covers an idle paused goal, not every scheduling
  race when Resume is pressed during an unrelated active response.
- **Gate316 passed:** frozen offline workspace (engine1512/app337/UI217), strict all-target
  workspace Clippy, fmt/diff and unchanged source/live ledgers. Artifacts
  `/home/jason/.pg.qtm1vC`; snapshot
  `49c8a81b3375bec6693eb9b8439c6ebc2f5d064662dc16111434e2432c980808`.
 313 (`/home/jason/.pg.De18qU`) caught a missing test-only EventKind qualification; corrected.
 314 (`/home/jason/.pg.AFx4of`) and315 (`/home/jason/.pg.EClUg3`) passed the new regressions
  but failed different fake-CLI metadata fixtures with executable-busy errors.316 passed on
  the same source, with normal test parallelism. The intermittent fixture failures remain
  recorded; no production retry, test weakening or installed CLI execution was added.
- **Not installed/native-window verified:** the running app/profile was not changed or restarted.
  The user rebuilds. Current-build recovery to try after the response settles: switch to another
  conversation and back, wait for loading, then Resume. Do not clear the goal/history or edit
  its database. No release/install, inference, credential access, commit or push.

## Native CLI orchestration — owned root vertical path (goal6, task76 ongoing)

- **Implemented, inactive:** the internal root coordinator joins committed prompt/launch intent,
  prepared process/assets, strict observer, native driver/clean close, actual process supervision,
  persisted response projection and root settlement. Scoped committed text updates use a bounded
  publication lane; this is not yet attached to SessionRt or the existing conversation UI.
- **Passed:** gate312 frozen workspace (engine1512/app336/UI217),13 new tests (8 coordinator,
  5 projection), existing helper/native-tool integrations, strict all-target workspace Clippy,
  fmt/diff and identical source/live ledgers. Owned fake CLI + authenticated peer simulation:
  actual native Write, persisted response, clean Completed and real supervisor facts; Stop,
  helper/deadline/publication/exit/trailing-data/prelaunch failures cannot become success.
  Projection checks rollback, byte offsets, repair, recovery and stale scope. Display loss stays
  distinct from execution uncertainty; missing usage is not invented in completion evidence.
- Artifacts `/home/jason/.pg.sXDCqw`; snapshot
  `04c7727324a4e80fe2fee6416c70dbbcc3b1c40181e1d2aab1c689c240648c84`.
  Gate309 failed on the fake script's argv dispatch and was cancelled during an unbounded test
  wait; corrected fixture/wait in310, full311 passed, display evidence re-gated in312.
- **Not qualified:** installed CLI/actual SDK lifecycle. New root fixtures use a simulated peer,
  not the copied helper image. Approved runtime creator, live SessionRt publication, full native
  services/context/workflows and desktop activation remain. Backend remains Unsupported.
- Scope is now **CLI integration only**: the user explicitly excluded dashboard implementation;
  tasks54–61 are cancelled, not delivered. No dashboard code was implemented. No installed CLI
  invocation/inference, credential/profile changes, release/install, UI input, commit or push.

## Native CLI orchestration — sealed root inputs and strict observations (goal6, task76 ongoing)

- **Implemented, inactive:** owner-only stored launch input snapshots; request/canonical-user
  reconciliation against permanent admission; fresh/resume mode read from the receipt; bounded
  nonpartial input loading; repeated checks inside launch CAS. The staged prepared-spec API no
  longer takes caller-selected prompt/mode/scope. Snapshot/spec creation grants no execution.
- Native stdout now has a separate exact-catalog observer: only bound Pipkin MCP tools, no CLI
  built-ins/foreign MCP/inventory omissions or Coding-policy opt-in. SDK tool records stay inert
  and are not emitted as native cards/receipts. Display callbacks carry scope and remain bounded;
  a terminal or decoder finish alone is not root success. Existing Coding policies are unchanged.
- **Passed:** gate308 frozen workspace (engine1499/app336/UI217),12 new unit tests, existing real
  helper/native-tool integrations, strict all-target workspace Clippy (`-D warnings`), fmt/diff,
  identical source/live ledgers. Artifacts `/home/jason/.pg.iOD6qm`; source snapshot
  `ba48b72e80d3c4becce5953fb0bc58b820def2735f5fe4217f6e05e6f56948af`.
- **Failed then corrected:**305 oversize fixture inserted BLOB into STRICT TEXT;306 body fixture
  violated json_valid. Schema constraints were retained, fixtures corrected,307 passed, and308
  re-gated the added pre-materialization/metadata guards. Details in slice9 of
  [the implementation note](claude-cli-native-orchestration.md).
- **Still absent:** approved runtime creator, root launch/supervision, persisted/live projection,
  full context/workflows and dashboard. Exact SDK inventory and root/helper-close ordering remain
  unqualified. Backend remains Unsupported; schema28/protocol41/desktop schema10 unchanged.
  No installed CLI invocation/inference, profile/credential changes, release/install, GUI input,
  commit or push. Zero live prompts remain; offline implementation can continue.

## Native CLI orchestration — owner-controlled channel closure (goal6, task76 ongoing)

- **Implemented, inactive:** scope-bound one-use close control, protocol/worker/queue quiescence,
  authenticated helper acknowledgment, durable close before the final frame, and owner reply
  after cleanup. Stale scope cannot consume the handle; owner drop synchronously blocks further
  dispatch. Stop/deadline/pending work/storage failure cannot become clean completion.
- **Passed:** gate304 frozen workspace (engine1487/app336/UI217), strict all-target workspace
  Clippy (`-D warnings`), fmt/diff, identical source and live ledgers.11 new tests:9 unit plus2
  actual debug-helper integrations with real native Write, clean helper exit/no fake MCP results,
  forged acknowledgment refusal, scope/quiescence, pending work, Stop, timeout and owner loss.
  Artifacts `/home/jason/.pg.F58Glr`; source snapshot
  `9a4af7e8ea902269ceaa92673787f2e256e586e3f0714ae0bc798ed0f32acdf4`.
  Targeted302 and full303 also passed; review after303 added the owner-drop dispatch guard.
- **Not delivered:** approved runtime creation, CLI root launch/supervision/projection, full
  context/ToolHost/workflows or dashboard. Clean MCP closure does not complete a model root;
  tests verify the root remains Running and becomes Unknown without independent root facts.
  No installed-CLI MCP compatibility or copied installed-app qualification is claimed.
  Backend remains Unsupported. See slice8 in [the implementation note](claude-cli-native-orchestration.md).
- Schema28/protocol41/desktop schema10 unchanged. No live prompts, installed CLI invocation,
  user-profile/credential changes, release/install, native-window input, commit or push.

## Native CLI orchestration — durable root journal (goal6, task76 ongoing)

- **Implemented, inactive:** schema28 atomically joins native prompt/run/user entry/outbox with
  permanent root receipts. Only new admission mints an owner lease. Launch intent is one-use
  and scoped to its prepared bridge. Fresh/resume eligibility comes from receipts; uncertain
  roots never replay. Stop, removal and startup retire authority before native recovery.
- **Implemented:** completion requires independent clean supervisor facts and closed-channel,
  joined native receipts. Contradictions preserve original outcomes while fencing context and
  active successors. Journaled tool dispatch rejects pre-launch/wrong-bridge authority.
  Existing Native/Coding identities and API-only guards remain unchanged.
- **Passed:** gate301 frozen full workspace (engine1478/app336/UI217), existing actual helper/
  native-tools integration, strict all-target workspace Clippy (`-D warnings`), fmt/diff,
  source equality and unchanged live ledgers.16 new unit tests include16-way admission,
  rollback, Stop/removal/contradiction checks, v27 backup/migration and five process-exit cuts
  with actual native Write effects. Known tool receipts survive an Unknown root; no replay.
  Supervisor facts are fixtures, not real CLI observations. Artifacts `/home/jason/.pg.w2d1gB`;
  source snapshot `38e59e0103499deb9540ec60017b5400ce0d8c7b7bab5af1452e9eb77d1d73db`.
  Targeted299/300 passed (`/home/jason/.pg.SfspyX`, `/home/jason/.pg.A1DzZL`). No new gate failures.
- **Not delivered:** approved runtime creator, actual CLI root launch/supervision, coordinated
  graceful channel closure, live publication, full context/ToolHost/workflows or dashboard.
  Post-launch uncertainty still forbids resume; full post-Stop/error/context parity is pending.
  Backend remains Unsupported and no capability is advertised. See slice7 of
  [the implementation note](claude-cli-native-orchestration.md).
- Engine schema28; protocol41/desktop schema10 unchanged. No live prompts, installed CLI
  invocation, user-profile/credential changes, release/install, native-window input, commit
  or push. Both live inference allowances remain exhausted.

## Native CLI orchestration — private native-only launch construction (goal6, task76 ongoing)

- **Implemented, inactive:** distinct native-policy metadata preflight; sealed native channel
  launch handoff; binding and session/generation/op/run checks; private MCP/system snapshots;
  retained copy of the running Linux helper image; MCP-only command recipe. No CLI built-in
  executor is requested, no secrets/system/user text in argv, and no ambient channel override.
  Generic/Coding launch cannot upgrade into this policy. Existing identities remain unchanged.
- **Passed:** frozen gate298 workspace (engine1462/app336/UI217), actual debug-helper/native
  tools integration, strict all-target workspace Clippy (`-D warnings`), fmt/diff, identical
  source and live ledgers.16 new unit tests cover fake-CLI metadata, scope/identity/expiry/file
  guards, private cleanup and clean-environment execution of a copied running unit-test image.
  Native config identity supports different child aliases while the separate binding/model
  checks prevent session rebinding; existing Coding markers are unchanged.
  Artifacts `/home/jason/.pg.z25vF5`; source snapshot SHA256
  `6d23fe6ecf334dc8cd7ccf31ea4e8dad1ecb186da38a80589a004dbd18c23079`.
  Targeted296 and full297 also passed (`/home/jason/.pg.x6qTbO`, `/home/jason/.pg.YN0Slx`).
  Review after297 found the child-alias configuration mismatch;298 rechecked the fix and two
  regression tests. No new gate failures.
- **Not delivered/qualified:** approved runtime creator or installed-CLI launch; root admission,
  fresh/resume eligibility, terminal/supervision/live publication; full context, thinking,
  attachments, workflows or dashboard. The copied-image test is not copied installed-app MCP
  qualification. The launch recipe does not prove actual CLI plugin/memory/tool isolation.
  Backend remains Unsupported; no user-facing capability is exposed. See slice6 in
  [the implementation note](claude-cli-native-orchestration.md).
- Schema27/protocol41/desktop schema10 unchanged. No installed CLI invocation, live inference,
  user-profile/configuration/credential changes, release/install, native-window input, commit
  or push. Both live allowances remain exhausted.

## Native CLI orchestration — immutable storage bindings (goal6, task76 ongoing)

- **Implemented, inactive:** schema27 adds immutable orchestration bindings and permanent global
  CLI UUID/session/creation-key ownership. Existing Coding/legacy receipts, histories and Unknown
  outcomes are not rewritten. Removed child UUIDs, engine IDs and Create keys remain reserved
  across backends. Native/API guards remain unchanged.
- **Implemented:** exact Create receipts, frozen model/project/executable/account/policy and
  parent/root identity, CLI-only child identity inheritance, atomic UUID reservation and removal
  retention. Native channel preparation checks the durable binding digest; native admission
  rechecks binding presence. The sealed handoff captures its original deadline.
- **Passed:** frozen gate295 full workspace (engine1446/app336/UI217), actual debug-helper/native
  tools integration, strict workspace/all-target Clippy (`-D warnings`), fmt/diff, source equality
  and unchanged live ledgers.13 added unit tests cover bindings, migration, concurrent cross-policy
  ownership and channel refusal. Protocol41/desktop schema10 unchanged; engine schema27.
  Artifacts `/home/jason/.pg.UHG8zn`; source snapshot SHA256
  `09557bcaeba269766e5530c33a966d6c2d73c2cf54810a45f9bf8353380bca11`.
- **Development failure retained:**293 exposed stored-version validation using a raw CLI-report
  parser incorrectly, three additional downgrade fixtures needing v27 teardown, and an ancestor
  fixture trying to mutate newly immutable parentage. Fixed without suppression;294 rechecked.
  Artifacts `/home/jason/.pg.8MVXTn` (failed), `/home/jason/.pg.4Rz0ix` (targeted pass).
- **Not delivered:** approved runtime creation or installed-CLI launch, full root/workflow/context
  lifecycle, live publication, dashboard or native orchestration parity. Storage metadata is not
  proof of executable/login trust. Backend remains Unsupported and no new UI capability is exposed.
- No live prompts, user-profile/credential/configuration access, release/install, native-window
  input, commit or push. Both live allowances remain exhausted.

## Native CLI orchestration — atomic native tools (goal6, task76 ongoing)

- **Implemented, inactive:** bridge receipts now join native tool intents, canonical tool-call
  entries/history IDs, actual result entries and retained output atomically. Known results are
  immutable; unknown effects remain Unknown/Interrupted without synthetic results or replay.
  Native-only API guards and old backend identities remain unchanged.
- **Implemented:** authenticated-reader-only driver checks the captured native catalog digest,
  validates schemas through `tools::run`, and shares normal ToolCtx/mutation/process/output
  machinery. Bounded dedicated reader, initially serial workers, lifetime-wide cancellation,
  cooperative cleanup and durable retirement. No CLI stdout/markup execution authority.
- **Measured:**21 new tests (20 unit,1 actual debug-engine-helper integration). Real native
  Read/Write/Edit/Bash effects in owned temporary projects, not CLI inference. Replay preserves
  a subsequent manual edit; Stop reaps the owned shell process. Intent/result rollback, actual
  effect with failed result commit, scope/catalog fencing, generic-channel refusal and three
  native WriteTool WAL process-exit cuts preserve Unknown and refuse uncertain re-execution.
- **Passed:** frozen gate292 full workspace (engine1433/app336/UI217), strict workspace/all-target
  Clippy (`-D warnings`), fmt/diff, identical source and unchanged live ledgers. Artifacts
  `/home/jason/.pg.rPctl4`; source snapshot SHA256
  `5cade47b38079cb7c7cff3d8ea616486db7fd1f655495f5f2d5906bf846ee8ca`.
  Protocol41/engine schema26/desktop schema10 unchanged.
- **Development failures retained:**289 compile caught a missing required ToolHost method;
  the checkpoint-only host now explicitly refuses child service.290 passed tests but strict
  Clippy rejected a reader loop form; rewritten without suppression.291 caught catalog checks
  incorrectly comparing against the channel-secret capability digest. Native catalog identity
  is now separately committed into the binding digest, with a sealed native-specific handoff
  and refusal tests. Gate292 rechecked all source. Failures retained in
  `/home/jason/.pg.6b9c25`, `/home/jason/.pg.VfvYUo`, `/home/jason/.pg.zUccu3`.
- **At gate292 (storage identities advanced above):** approved orchestration binding/session/CLI UUID ownership and
  installed-CLI launch, root outcome/live-session publication/context/children/queues/goals,
  complete native
  tool-host services, large payloads, parallel/per-call parity, dashboard and installed UI.
  Existing Coding is unchanged. No public creator/model-picker capability or API fallback.
- No live inference, user-profile mutation, credential/configuration access, release/install,
  native-window input, commit or push. Both live prompt allowances remain exhausted.

## Native CLI orchestration — authenticated helper channel (goal6, task76 ongoing)

- **Implemented, inactive:** mutually authenticated, scope-bound loopback transport with
  per-frame HMAC/direction/transcript/sequence checks and bounded framing. The host commits
  its one-use connection claim before acknowledgement. No reconnect/rebind, automatic tool
  replay or native/API model fallback. The native-tool helper is registered before ordinary
  startup in both binaries, but no orchestration creator/CLI launch/model-picker route exists.
- **Measured:**27 new offline tests (24 unit,3 debug engine-helper integration), including
  wrong secret/scope, role/sequence/replay tampering, commit failure, stop/owner retirement,
  cancelled/partial I/O, output backpressure, lifetime limits, strict helper environment,
  coalesced MCP lines, refused regular-file stdio, and a real helper process relaying MCP plus
  committed **simulated no-effect** receipts. No native tool or installed CLI was executed by these helper fixtures.
  App helper dispatch is compiled, not live-window or installed-app qualified.
- **Passed:** frozen gate288 full workspace (engine1413/app336/UI217), strict workspace/all-target
  Clippy (`-D warnings`), fmt/diff, source equality and unchanged live ledgers.
  Artifacts `/home/jason/.pg.VPzXGL`; source snapshot SHA256
  `7f1a3e0eadb7466853bc512994b08cfa03076b4c8c4fe182b5b264433bcd7461`.
  Protocol41/engine schema26/desktop schema10 unchanged in this slice.
- **Development failures retained:**286's tests passed, then Clippy rejected a unit-value
  binding;287's tests passed, then Clippy rejected a single-pattern match. Both were corrected
  without lint suppression;288 rechecked the complete frozen source.
- **At gate288 (superseded in part above):** Unix pipe/socket helper stdio only; authenticated local IPC is not
  encryption or a same-UID/root sandbox. Cancellation-safe driver orchestration and explicit
  durable retirement still need integration; dropping a socket does not synchronously retire
  SQLite authority. Native tool/transcript atomic joins, actual execution, installed-CLI launch,
  context/children/queues/goals/lifecycle and dashboard are not delivered. The transport tests
  do not establish real Claude MCP compatibility or full orchestration parity. See
  [implementation boundaries](claude-cli-native-orchestration.md).
- No live prompts, credential/configuration inspection, user-profile mutation, installation,
  release build, native-window input, commit or push. Both live allowances remain exhausted.

## Native CLI orchestration — durable journal substrate (goal6, task76 ongoing)

- **Implemented, inactive:** engine schema26 adds an immutable bridge identity journal,
  single-use committed connection claim and durable bridge intent/result receipts.
  Reserved `claudeCliAgent` rows are still Unsupported; no public creator, new capability or
  CLI launch route exists. Native/legacy CLI identities cannot mint these claims. Existing
  native-only runtime/store guards remain unchanged. Protocol41/desktop schema10 unchanged.
- **Safety behavior measured:** canonical duplicate handling, typed IDs, concurrent same-call
  admission returning one claim, lost acknowledgements, failed transactions, stop/scope guards,
  conflicting requests/results, bounded count/pending/aggregate bytes, immutable completed
  receipts, late result rejection, removal and startup revocation. Five process-exit cuts
  exercise prepared/connected/intent/simulated-effect/completed states without running a CLI
  or native tool. v25 migration preserves native/CLI rows and Unknown receipts with a backup.
- **Passed:** frozen gate285,23 journal tests and full workspace
  (engine1389/app336/UI217), strict workspace/all-target Clippy (`-D warnings`), fmt/diff,
  frozen source equality and unchanged live ledgers. Artifacts `/home/jason/.pg.4OioUG`;
  source snapshot SHA256 `9d3c8a99ccb539256c0cc311a66a11cab260f908edbd486e860a5b1e6efca120`.
  Earlier targeted gates283/284 passed; existing downgrade fixtures were updated to remove
  the new additive tables when constructing historical schemas. No checks were suppressed.
- **At gate285 (superseded in part above):** authenticated helper transport, independently verified binding creation,
  atomic integration with native `tool_calls`/transcript/result records, actual tool executor,
  root-run lifecycle, children/context/queues/goals, dashboard or full orchestration parity.
  A journal Claim alone is not sufficient to execute anything. A stored capability digest is
  not authentication. Payload limits remain provisional, not large-payload/attachment parity.
  See [the ownership boundary](claude-cli-native-orchestration.md).
- No live prompts, user-profile mutation, installation, release build, credential/configuration
  access, native-window input, commit or push. Both live inference allowances remain exhausted.

## Native CLI orchestration — protocol foundation (ongoing goal6)

- **Implemented, not activated:** a separate bounded MCP protocol session for engine-owned
  tool requests. Native builtin schemas are exported unchanged; initialize/ready state, exact
  tool names, captured scope, instance-fenced call keys, typed IDs, duplicate/pending/receipt
  handling, cancellation, conflicting-result checks and resource fences are explicit. Arguments
  and response bodies are redacted from protocol diagnostics. No tool or process I/O occurs here.
- **At gate282 (superseded in part above):** authenticated helper transport, durable bridge admission, native
  execution driver, history/child/queue/goal integration, dashboard and full orchestration parity.
  No new capability is advertised and existing CLI policies/bindings are unchanged. The module
  must never be fed CLI stdout or assistant markup. See
  [implementation boundary and remaining steps](claude-cli-native-orchestration.md).
- **Measured offline:**17 protocol tests passed. Frozen gate282 passed full workspace
  (engine1366, app336, UI217), strict workspace/all-target Clippy (`-D warnings`), fmt/diff,
  source equality and unchanged live ledgers. Artifacts `/home/jason/.pg.SYwnfH`;
  source snapshot SHA256 `d0101af0e49c2b776cd2471b773924ff9327f8354e75137b23a3a39216a040ff`.
  These are protocol tests, not real MCP/CLI execution or native tool integration qualification.
- **Development failure:**281 passed its initial14 tests, then strict Clippy rejected deprecated
  `AtomicU64::fetch_update`. Replaced with `try_update`; added initialization, replay-consistency
  and redaction cases before282. No suppression or live rerun.
- **Authorization:** user now explicitly includes dashboard54–61 in the whole-plan goal.
  Both historical live allowances remain exhausted. No inference, installation, release build,
  credential/configuration access, native-window input, commit or push in this slice.

## CLI model selection and real coding follow-up

- **Observed:** receipt8, like receipt7, resumed `safe-restricted-no-tools-v1` with clean
  completion and zero tool actions. All five saved bindings were legacy chat-only. The model
  picker compared aliases alone, so selecting the same alias silently kept that old policy.
- **Implemented:** core selection now compares the requested coding route, not just the alias.
  Same-alias legacy selection creates a new coding identity via configured-provider setup,
  without another confirmation. Old bindings/history/drafts are preserved; pending journaled
  instructions cannot leak through a cancelled or completed switch. Labels distinguish legacy
  chat/Write, and the composer provides **Use coding model**. UI selection marks use the same
  pure-core predicate. No parsing, I/O, schema change or native authority was added to rendering.
- **Measured live:** a separately authorized three-prompt allowance was fully used. Job273
  passed real public-engine Read/Write; job276 passed the real desktop core/adapter/storage/engine
  same-alias transition and fresh/resumed turns. CLI2.1.301/Sonnet: six completed Read actions,
  three completed Write actions, exact file contents in disposable projects, zero approvals and
  zero native-provider requests. All turns completed with clean settlement and no presentation
  warning. Artifacts `/home/jason/.pg.dgvElr`; [full ledger](../fixtures/claude-cli-probe/unattended-coding-followup.md).
  The prior ten-run ledger is unchanged. No further live inference is authorized.
- **Measured offline:** job275 passed core execution-profile28 and the new fake desktop
  same-alias transition regression. **Frozen gate280 passed:** full workspace (app336,
  engine1349, UI217), strict workspace/all-target Clippy (`-D warnings`), fmt/diff, source
  before/after equality and unchanged live ledgers. Targeted fault-link and recovery regressions
  also passed. Artifacts `/home/jason/.pg.Y2aEVo`; source snapshot SHA256
  `219ce2463296c9c67f2ea3a496935e6736c5d287ba62045befd3c31ead26ec48` covers the crates diff,
  compiled native reference and separate hashes of untracked crate files. Ignored live tests
  did not run in this gate.
- **Failures retained:**270 failed harness compilation (UUID dependency);271/272 stopped before
  inference on the test runner's injected loader environment. Direct test-binary launch used the
  original environment; production safety was not weakened.274 failed compilation on an Option
  assertion in the regression, fixed before275. Gate277 failed the existing lost-Create recovery
  test after334 other app tests passed (`/home/jason/.pg.U7Rbzg`). The fault injector stored reply
  IDs across transport connections, allowing an abandoned old reply to swallow a new lookup.
  Characterization278 first hit an unrelated startup event; corrected279 deterministically
  reproduced the cross-connection timeout (`/home/jason/.pg.0Yz0Y3`). Reply-fault sets are now
  shared only by each link's sender/receiver, with a regression that abandons the old receiver
  and reuses the same ID on a new connection. No timeout increase, production retry or policy
  weakening. No uncertain prompts were repeated.
- **Still not implemented/qualified:** native Pipkin plans/jobs/subagents/goals/queues are not
  provided by CLI-owned tools. The full native workflow requirement remains open. Bash/Edit/CLI
  agent calls and other aliases/releases were not live-tested; native-window interaction,
  accessibility, billing/managed policy and full descendant containment remain unverified.
  No install, release build, user-app restart, credential inspection, configuration mutation,
  commit or push. User rebuilds. Dashboard54–61 remain planning-only.

## Unattended Claude CLI coding provider — preceding offline implementation baseline

The following records the earlier269 baseline. Its live-verification limitations are superseded
only by the explicitly bounded follow-up above, not by broad claims of native workflow parity.

- **Observed cause:** the reported2.1.301 turn (receipt7) completed cleanly under immutable
  `safe-restricted-no-tools-v1`, with no action receipts or stream warning. Tool-like `<invoke>`
  markup was retained assistant text, not an executed search. That chat-only policy deliberately
  launched with no tools; presentation fixes alone could not make it implement the plan.
- **Implemented:** new ordinary CLI conversations use `cli-unattended-coding-v1`: default CLI
  tools, `bypassPermissions`, no permission prompts/broker and no per-tool approvals. CLI-owned
  read/search/edit/write/shell/agent operations execute in the captured project. Hooks/additional
  MCP configuration are disabled; project instruction files are explicitly requested in a static
  system supplement. CLI login, identity checks, host opt-outs, Stop and no native fallback remain.
  New coding turns have a30-minute bound. This is not a sandbox or a promise to undo effects.
- **Workflow:** mode selectors removed. After initial provider configuration, selecting its normal
  model alias or explicitly reconnecting automatically coordinates fresh checks/Create/Return.
  A different discovered executable is not run silently. Async tuples, journaling, drafts and
  uncertain creation/prompt fencing remain. Legacy histories/bindings keep their exact policy;
  use New for coding rather than migrating or replaying an old conversation.
- **Observation contract:** only structured root tool calls/results enter external activity, with
  init-inventory membership, correlation and action-name checks. Default inventory is not narrowed
  to a fixed tool list. CLI-owned nested telemetry may have a child session ID but must correlate
  to an observed root call; it never changes root actions/content/completion or runs native tools.
  Non-text output gets an inert placeholder; UTF-8-safe8KiB previews share a bounded display budget.
  Truncation warns without aborting useful work. Root authority/resource/terminal checks stay strict.
  Protocol41 adds Coding identity; engine/desktop schema versions are unchanged.
- **Measured:** frozen-source269 passed the full workspace (engine1349, app334, UI217), strict
  workspace/all-target Clippy (`-D warnings`), fmt, diff and before/after source-hash comparison.
  Artifacts retained at `/home/jason/.pg.0Bygbm`; source diff hash
  `a91fef1c8c17abd45bd642646bc61cb462fbddae6d1f4fdbc4f4ed6028944444` includes crates and the
  compiled native reference, with separate hashes for the two new Rust files in that directory.
  The live ledger remains `bf794518e35d7f1ce3a50b3058c4191bb9401e568fc645d77e10b0f404cf1f22`.
  Scoped268 passed decoder and external-runtime suites, core local-CLI42 and app
  local-CLI27. Fake-process coverage includes all named coding tool families, actual changes only
  in disposable fixture projects, fresh/resume, fourteen settled action receipts, zero permission
  rows/native inference, duplicate requests and cancellation. Configured connection/reconnect,
  native-model preference separation and legacy immutability passed through core/adapter/engine.
  Scoped UI19 and the full gate passed after fixing the obsolete focus expectations. These are
  offline/fake-process results, not live CLI qualification. Official CLI reference/headless docs
  corroborate default tools, bypass mode and `--permission-prompts none` semantics.
- **Development failures retained:**265 missed an exhaustive UI profile match;266 exposed old
  chat-profile/helper-flag expectations in desktop fixtures;267 hit a mutable-string SQL parameter
  type error;268 passed the backend/core/app checks but failed two obsolete UI tab positions after
  mode buttons were removed. They were corrected before269. Artifacts include
  `/home/jason/.pg.EoOoQV`, `/home/jason/.pg.o7xVNf`, `/home/jason/.pg.2bodsO` and
  `/home/jason/.pg.XTke1E`; preceding failures remain documented below.
- **Unverified:** real unattended default tools/flag combination on the installed CLI, billing,
  managed policy, full descendant containment and native accessibility/interaction. No live prompt,
  credential/configuration mutation, CLI install, release build, installed-app change, commit or
  push was performed. Historical live qualification is2.1.299 chat/Write only; accepted newer
  versions and the new policy are not thereby live-qualified. Dashboard54–61 remain planning-only.

## CLI provider repairs (preceding chat/legacy-Write policy)


- **Implemented:** documented per-block assistant envelopes now finalize blocks, not entire
  messages. Stream indices and SDK envelope identities preserve ordering/deduplication, including
  repeated identical text in distinct blocks. Unused thinking/context/progress metadata is
  discarded without a closed schema; known content fields remain typed. Complete blocks and
  final-result text reconcile recoverable display loss. Post-result system telemetry is drained;
  duplicate/conflicting terminal results and session identity changes remain hard failures.
- **Execution vs presentation:** provider runtime has one framing decoder (no competing smoke
  validator). Bounded asynchronous backpressure replaces `try_send` failure; cancellation, timeout
  and process-group signals remain live while delivery waits. Projection commits bounded batches,
  coalescing only unpublished replacements of the same entry. Storage errors still fence execution.
  Display warnings persist across snapshots/restart without invalidating confirmed completion.
  Closed diagnostic categories plus bounded block-state facts are retained, never raw rejected
  frames, message IDs, account data or stderr. Malformed/unresolved tool observations stay strict.
- **Known errors:** an explicit CLI error with finished transport, matching exit0/1, no supervisor
  stop and resolved permission/action evidence is recorded as a known provider error. Only a new
  explicit instruction may resume it; duplicate requests still resolve to the same receipt. Old
  Unknown receipts, cancellations and ambiguous effects are not upgraded or replayed. No schema
  migration: optional evidence fields default safely, and old receipts cannot acquire the new
  `modelError` resumption proof retroactively.
- **Connection lifecycle:** five-minute setup tickets and 30-second Create freshness remain.
  Established connection consent is separately owner-scoped/revocable until disconnect/shutdown;
  ticket pruning no longer cancels it. Expired setup IDs remain revocable by their original owner.
  Grants are bounded; fresh per-prompt identity/login checks, host gates and per-Write approvals
  remain. Write authority retains a bounded turn lifetime. Consent UI/documentation updated.
- **Measured:** frozen-source full264 passed workspace tests (engine1343, UI217), strict
  workspace/all-target Clippy (`-D warnings`), fmt and diff checks. Owned nonempty TMPDIR
  `/home/jason/.pg.HbUsgA` retained. Scoped259 and full263 also exercised the engine lifecycle;
  standalone MCP fixture compilation and focused decoder checks passed in263. Coverage includes
  documented thinking→text blocks, byte splits, telemetry, multi-block final-text reconciliation,
  slow consumers, blocked-delivery cancellation/timeout, burst output, clean CLI errors, explicit
  new turns, active-turn/setup expiry, foreign-owner denial/revocation, and warning/error
  persistence through store reopen. Existing crash/revocation/no-replay guards remain exercised.
  The ten-run live ledger SHA256 remains
  `bf794518e35d7f1ce3a50b3058c4191bb9401e568fc645d77e10b0f404cf1f22`.
- **Development failures retained:** jobs255–258 exposed old closed-metadata expectations,
  a missed removed-grant-field reference, optional-usage expectations and a fixture assuming init
  always commits separately. Atomic batching can roll init back with the failed display write;
  the corrected test verifies cursor0/1, no committed assistant output, cleanup and recovery.
  Job259's lint was corrected without suppression. Full260 failed an existing fake-editor
  launcher test with `Text file busy` (`/home/jason/.pg.FemefZ`): cause unestablished, not claimed
  fixed. Full261's nested MCP fixture builds encountered an in-progress normalization edit;
  standalone262 exposed its wrong enum name (`Kind` versus `BlockKind`), corrected before263.
  Full263 passed engine1343 but failed one UI assertion because the revised consent copy omitted
  the stop/fresh-context warning. The warning was restored and setup-lifetime assertions added;
  full264 passed on frozen source. Failed directories/logs retained. No production retries,
  reduced parallelism or gate weakening; slow-consumer sleeps are deliberate pressure injection.
- **Unverified:** real Opus completion after these repairs, native installed-app interactions,
  broad newer-version qualification, billing and full descendant containment. No live CLI
  prompts/probes, profile mutation, release build, packaging, installation, commit or push.
  User rebuilds; existing uncertain turns remain untouched. Dashboard implementation is pending.

## Historical CLI provider architecture review — further resumed-turn failure

- **Observed:** after one completed fresh CLI2.1.301 turn, the resumed turn was fenced on
  `ContentBlockStart` / `Sequence` / `ObserverFailure`, with two projected events and zero text
  bytes. Read-only receipt inspection; no raw prompt/stream or CLI-private data read.
- **Baseline defects (superseded by the repairs above):** the decoder treats a per-content-block assistant record as a
  whole-message final; both parsers reject documented trailing system events after a result;
  display-parser failure or bounded-channel pressure is coupled to process termination and
  context invalidation. Setup-ticket expiry also expires execution grants and can stop a turn.
- **Measured:** job253's temporary pure-decoder characterization reproduced the reported error
  class/cursor using documented interleaving, both parsers' post-result rejection, and a
  `SinkRejected` display backlog. These are confirmations of bad behavior, not compatibility
  passes. Harness retained in `/home/jason/.pg.6vpFzV`; temporary integration-test copy removed.
  Unchanged-source full254 passed workspace tests (UI217), strict workspace/all-target Clippy,
  fmt and diff checks. Owned nonempty TMPDIR `/home/jason/.pg.N58u7N` retained; exit zero.
  The live ten-run ledger is unchanged. Green existing tests do not negate these protocol defects.
- **Review:** [findings and replacement boundary](claude-cli-provider-review.md). Earlier green
  synthetic tests modeled combined whole-message snapshots and missed documented block ordering.
  The previous metadata patch does not establish that Opus or the CLI provider is usable.
- **Unverified / unchanged:** exact raw live sequence was not retained; no live CLI prompt/probe,
  profile mutation, new production fix, installed-app interaction, install or commit. Existing
  uncertain turns were not replayed or upgraded. Dashboard work remains planning-only.

## CLI Opus thinking metadata and retained errors

- **Observed:** bounded read-only engine receipt inspection found two CLI2.1.301 failures:
  `MalformedRecord` / `content_block_delta` / `UnknownField`, followed by supervisor observer
  failure. The retained field-label hash exactly matches `estimated_tokens`, documented in the
  official `BetaThinkingDelta` schema. Raw records/values were not retained or read; old unknown
  receipts/bindings were not changed. See [evidence](../fixtures/claude-cli-probe/thinking-metadata-regression.md).
- **Changed:** accept and discard only optional nullable unsigned thinking-token estimates;
  never infer usage or completion from them. Unknown mutations remain rejected. New unknown
  settlements atomically retain an inert transcript error with closed diagnostic categories,
  surviving snapshot replacement/navigation/restart. Core settlement avoids a transient duplicate
  of an identical durable error. No raw payload, stderr or private schema label is exposed.
- **Measured:** job249 reproduced `MalformedRecord` before the parser fix (retained owned TMPDIR
  `/home/jason/.pg.hbWERo`). Targeted250 passed decoder/process, external store/runtime and core
  snapshot/failure regressions, including fake Opus fresh/resume, unchanged usage, zero native
  requests, malformed-input fencing, retained diagnostics and no replay. Full251 stopped on an
  existing pipeline fixture's `selected executable is busy` preflight error (engine1331 passed,
  one failed; retained `/home/jason/.pg.QQ4aG0`). Its transient OS cause is unestablished, not
  claimed fixed; no retry/sleep/gate relaxation was added. All five isolated repeats and the
  normal-parallel workspace rerun252 passed (engine1332, UI217), along with strict workspace/
  all-target Clippy and fmt/diff checks. Nonempty owned TMPDIR `/home/jason/.pg.iZadjw` retained;
  job252 exited successfully. The live ten-run ledger remains unchanged.
- **Unverified:** actual Opus completion after this source fix, native error presentation and
  broader newer-CLI qualification. No live CLI prompts/probes, profile changes, native input,
  release build, packaging, installation, commit or push. The user handles rebuilding. Dashboard
  work remains planning-only and pending.

## Streaming table stability

- **Reproduced:** native streaming publishes `Synced` snapshots. `ItemsReset` discarded the
  Markdown cache and all measured list heights, so the deferred parsing path painted an empty,
  collapsed transcript before repainting the table at full height on every update. The new
  per-paint regression failed before the fix (job242, prefix 1/frame 0, zero registered blocks;
  retained `/home/jason/.pg.tckrjF`). Earlier table tests checked settled frames/token events
  and missed this snapshot-path regression.
- **Changed:** snapshots retain unchanged parses, refresh changed cached items outside render,
  and prune removed identities without eagerly parsing cold history or promoting unread cache
  entries. Surviving list identities retain measured height hints; structural differences use
  splices and content updates remeasure without resetting the list. Deferred misses keep the
  previous same-kind/expansion presentation until prepared; attachment generations fence that
  fallback. Selection survives updates to retained identities and clears on removal/reattachment.
- **Measured:** targeted244 passed 85 transcript tests and strict workspace/all-target Clippy.
  Per-frame tests cover the exact sample table streamed character by character through snapshots:
  no missing header frames, fixed column widths and bounded single-character vertical displacement.
  Additional tests cover an anchored reader inside a growing table, selection through completion,
  deferred cache misses, generation isolation, deleted identities, bounded refresh and LRU order.
  Full245 passed workspace tests (UI217), strict all-target Clippy and fmt/diff checks. The live
  CLI ledger remains `bf794518e35d7f1ce3a50b3058c4191bb9401e568fc645d77e10b0f404cf1f22`.
  Nonempty owned test directory `/home/jason/.pg.0WZLv2` was retained; the job exited successfully.
- **Unverified:** real compositor presentation/frame pacing and native pointer/keyboard/AT-SPI
  interaction. No native input, live CLI prompts, release packaging, installation, commit or push;
  the user handles rebuilding. All earlier uncommitted work is retained.

## Streamed Markdown tables

- **Changed:** GFM tables are parsed into row-major, individually selectable cells, including
  empty headers and missing cells. The transcript renders bordered grids with header emphasis,
  column alignment, wrapped inline formatting/links, and horizontal scrolling when columns
  cannot fit. Plain-text copy uses tabs between cells and newlines between rows.
- **Changed:** table hit testing follows the actual alignment of each shaped visual line and
  excludes clipped cells; blank space in a short cell still belongs to that cell. Link activation
  uses the same coordinate correction. Transcript Markdown cache misses are coalesced and prepared
  outside render, reading current content rather than installing captured stale text. Streaming
  refreshes the working set before painting; same-length replacements invalidate cached content.
  Questionnaire previews share the grid layout and are prepared outside render as well.
- **Measured:** full241 passed workspace tests (UI211), strict all-target Clippy and fmt/diff checks.
  Parser/document tests cover the Subagent vs Background job example, partial streaming prefixes,
  escaped pipes, inline styles, Unicode, empty cells, tabular copy, cache bounds and same-length
  replacements. Four new GPUI tests cover narrow/wide layouts, aligned selection/link activation,
  actual mouse-drag selection, horizontal scrolling/clipping, streaming remeasurement, tail following
  and preservation of an earlier scroll anchor. Existing 10,000-message boundedness tests pass.
  The live CLI ledger remains `bf794518e35d7f1ce3a50b3058c4191bb9401e568fc645d77e10b0f404cf1f22`.
  Nonempty owned test directory `/home/jason/.pg.PjDw3p` was retained; the job exited successfully.
- **Development failures, retained:** 234/236/239 caught compile-time integration/test issues.
  237/238 caught test assumptions about URL normalization, cache item counts and visible drag
  endpoints. The corrected fixtures assert the literal URL, single streaming-item revision,
  visible drag endpoints and exact cross-cell copy; targeted240 and full241 pass.
- **Unverified:** native pointer/keyboard, screen-reader and display-scale interaction for tables.
  No native input, live CLI prompts, release packaging, installation, commit or push; the user
  handles rebuilding. Earlier uncommitted label, plan and conversation-lifecycle work is retained.

## Conversation header simplification

- Close is now icon-only (×), retaining its tooltip, accessible name, keyboard focus and
  lifecycle/IME guards. The duplicate Subagents header button is removed; the right-pane tab
  and command-palette entry remain.
- Full233 passed workspace tests (UI200), strict all-target Clippy and fmt/diff checks.
  The close/reopen GPUI regression also checks the icon-only square control. No native input,
  live CLI prompts, release packaging or installation; the user handles rebuilding.

## Reversible Close / Closed conversations

- **Changed:** header Close moves a settled conversation out of the active list into a collapsible,
  project-filtered Closed section with explicit Reopen. Saved history-search hits identify closed
  results and explicitly reopen them. Namespaced, bounded preference IDs retain history, drafts,
  execution profile/model and plans; Close never calls RemoveSession, Cancel, Create or Submit.
- **Changed:** core guards cover incomplete/offline snapshots, active/unknown runs, pending/queued
  inputs, questions, subagents/jobs, active goals, provider changes, creation/approval uncertainty
  and unsaved drafts/storage errors. A complete attachment-scoped lifecycle barrier prevents Close
  between partial snapshot projections. Native goal start/resume uses exact request receipts after
  a fresh same-attachment snapshot; unknown results are not replayed. A later explicit reattachment
  refreshes their state. Global activity/question hints only restore visibility; they cannot select
  a conversation or grant execution authority.
- **Changed:** Close has a disabled-reason tooltip and an actual keyboard tab stop; Closed rows are
  bounded initially to 50 with Show more. IME composition prevents the Close control from acting.
  Closing the last active row retains the project and an empty workspace across catalog/restart.
- **Measured:** full232 passed workspace tests (UI200), strict all-target Clippy, fmt and diff checks.
  Coverage includes native snapshot ordering/nonselected activity, namespaced SQLite persistence,
  CLI close/restart/reopen with the same identity/draft and zero additional fake CLI probes or native
  inference, native completed-goal reopening without restarting it, and GPUI busy-close/reopen clicks.
  The live ledger remains `bf794518e35d7f1ce3a50b3058c4191bb9401e568fc645d77e10b0f404cf1f22`.
  Job232 exited 1 only after those gates passed: final `rmdir` found leftover test directories;
  `/home/jason/.pg.O3IgmS` was retained, not recursively deleted. Follow-up fmt/diff checks passed.
- **Failed during development, retained:** targeted226 exposed missing CLI policy admission and
  constructor filtering; targeted227 exposed lost project selection when closing the initial/demo
  conversation; targeted228 stopped at snapshot-marker byte accounting. Full229 caught a shared
  button keyboard-focus regression from explicitly setting tab-stop false; optional opt-in now
  leaves existing focus behavior untouched, and the keyboard regressions pass. Full230 stopped at
  a test-only protocol variant typo. Full231 passed tests/Clippy but caught formatting of a late
  accessibility-label edit, normalized before full232. No assertions or timeouts were weakened.
- **Unverified:** real native keyboard/pointer/IME/accessibility interaction; no native input, live CLI
  prompts, release packaging or installation performed. The user rebuilds/installs. Prior uncommitted
  label and plan changes are retained; no new commit or push requested.

## Plan banner lifecycle and finished-plan rollover

- **Changed:** the banner requires open tasks, has an accessible dismiss button, and hides completed,
  skipped or cancelled-only plans. Dismissal is a conversation-local view choice, survives snapshot
  refreshes, and does not cancel work or remove inspector access. New plans use fresh task IDs and
  get a new banner; stale revisions/captured close actions cannot hide or revive the wrong plan.
- **Changed:** an independent batch of new tasks after all current tasks finish atomically archives
  the old plan. IDs, evidence and history remain; unfinished plans and explicit old-ID continuations
  are untouched. Old archived counts are omitted from the composer banner.
- **Measured:** targeted222 passed core plan, engine domain/runtime and GPUI pointer-click tests,
  including restart persistence and idempotent rollover. Full223 passed workspace tests (UI199),
  strict all-target Clippy and fmt/diff checks. Native accessibility interaction remains unverified. No release rebuild, packaging or installation requested/performed;
  the user handles rebuilding. Earlier backend-label removal remains uncommitted alongside this work.

## Conversation Backend label removal

- Removed the redundant Backend status row above the composer for all providers. The model
  picker remains authoritative; pending-provider warnings, send guards and the Demo marker remain.
- Full220 passed workspace tests (UI198), strict all-target Clippy and fmt/diff checks. The updated
  regression checks actual model identity rather than the removed label. No live CLI prompts or
  native interaction verification. Package/install221 completed as `1.1.0-native-da223967949e`,
  with isolated probes and synthetic installed workflow passing; previous versions and running
  session retained. Evidence: `/home/jason/.pg.yRLVzm/installation-observation.json`. Changes uncommitted.
- The user subsequently requested handling rebuilds themselves: do not package/rebuild/install
  future changes unless explicitly asked.

## CLI minimum-version policy (current)

- **User-reported acceptance:** the user reports that everything works and they can test, and
  requested committing all pending changes to main. This is not a new formal live qualification.
  Historical installation receipts marked uncommitted describe their build-time state.
- **Commit gates:** full219 passed workspace tests, strict all-target Clippy, fmt/diff, source-setup
  contracts and guard37. Gate217 had an unresolved intermittent creation-recovery lookup timeout;
  five isolated repeats218 passed. Test-only diagnostics added, no production behavior changed.
  See `fixtures/claude-cli-setup/commit-checks.md`.
- **Changed at user request:** stable CLI2.1.299 or newer is accepted numerically. This supersedes
  the exact-version blocker below; it is not a claim of live qualification for every newer version.
- **Preserved:** explicit scoped trust, required capabilities, normal login, immutable actual-version
  bindings, fresh per-turn checks, individual Write review and no native/API fallback.
- **Measured:** targeted213 passed version/preflight/setup tests and7 app fake-CLI workflows,
  including fake2.1.301 fresh/resumed turns and provider switching. Full214 passed workspace tests
  (UI198), strict all-target Clippy, fmt/diff, guard37 and app build. Package215/install216 activated
  `1.1.0-native-ffe95dc723a1`; isolated candidate/installed probes and synthetic installed workflow
  passed. Prior versions retained and running user session untouched; fully quit/reopen.
- **Unverified:** real2.1.301 runtime compatibility, native pointer/IME/accessibility and all prior
  billing/managed-policy/containment limits. No new live prompts; original ledger untouched.
  Evidence: `fixtures/claude-cli-setup/minimum-version-checks.md`.

## CLI setup dialog / continuation correction

- **Reproduced:**207 pointer-click test dispatched a setup action but also dismissed the dialog.
  Guided setup was missing modal occlusion; earlier tests missed the closing overlay.
- **Corrected:** guided/advanced panels occlude the backdrop; inside clicks preserve the dialog,
  outside clicks still close it. Connections continues an existing model-picker setup instead of
  replacing its purpose with provider-only configuration. Rediscovery clears old consent.
- **Measured:**209 passed 40 core setup and19 UI setup tests, including pointer interactions.
  Full210 passed workspace tests (UI198), strict all-target Clippy, fmt/diff, guard37 and build.
  Package211/install212 activated `1.1.0-native-0dded419540c`; candidate/installed offline probes
  and synthetic installed workflow passed. All prior versions retained, running session untouched;
  fully quit/reopen. No live CLI execution; exact2.1.299 policy unchanged. Native
  interaction/accessibility verification is not claimed. Evidence:
  `fixtures/claude-cli-setup/dialog-flow-checks.md`.

## First-class CLI provider integration (current)

- **Implemented:** normal typed provider/model catalog; independent Connections configuration;
  source-specific unfinished-selection guards; immutable cross-provider/CLI-alias creation;
  authoritative model labels; exact native creation receipts; stable picker rows; legacy migration.
  New follows the current conversation, not a global exclusive backend switch.
- **Measured:** final full204 passed workspace tests (UI197), strict all-target Clippy, fmt/diff, guard37
  and app build. Fake CLI fresh/resume, native switch and CLI return use the normal typed catalog.
  Source guards, journal races, exact receipts, stable picker rows and preference migration pass.
  Blank workspace/restart cannot auto-select an unrelated row when an explicit choice has no
  available saved target; explicit navigation to another provider stays available.
- **Failures corrected:**191 hidden native catalog;192/193 absent new native guard-index row;
  195 ordinary New blocking its earlier source prompt;197 old UI fixture missing creation receipt.
  200/201 exposed a fake-disconnect fixture ordering assumption: post-review disconnect now uses
  a barrier, with immediate disconnect separately tested. Three focused repetitions each and
  full202 passed without changing production execution policy. See
  `fixtures/claude-cli-setup/provider-checks.md` for details and retained failure evidence.
- **Installed:** package205/install206 activated `1.1.0-native-685ecb881596`; all four prior
  versions retained, previous binary hash unchanged, running user session left untouched.
  Candidate/installed offline probes and the installed synthetic-provider workflow passed.
  Fully quit/reopen to use it. Evidence: `fixtures/claude-cli-setup/provider-installation-observation.json`.
- **Limits:** no live prompts; ten-run ledger unchanged. Native picker,
  focus, accessibility and live CLI verification are not claimed. Exact2.1.299 policy remains.

## Connections-only global switch (superseded; rejected abstraction)

- **Observed:** read-only normal-profile metadata showed no CLI conversations and recent native
  GPT sessions (no transcript/credential reads). Source allowed the old native composer after
  unfinished/failed CLI setup; ordinary New also did not retain a CLI connection choice.
- **Implemented:** setup/reapproval/recovery only in Settings → Connections; no dedicated sidebar,
  palette setup action or Ctrl+N chooser. Explicit, namespace-isolated routing choice controls New.
  Failed/cancelled setup does not enable a mismatched composer; the same guard runs after durable
  intent persistence. Unknown saved choices fail closed. The composer identifies the authoritative
  backend. Existing identities, running work and per-Write approvals are unchanged.
- **Measured so far:** core routing tests passed; real core+adapter+fake CLI fresh/resume passed
  in184 with a configured native model and a native provider that panics if invoked. Native model
  preference remained unchanged, CLI intents omitted native model overrides, and no native calls
  occurred.184 also caught opening Settings on Appearance rather than Connections; opening order
  was corrected, and the regression remains in the suite.182 caught a fixture attempting to
  reapprove Sonnet with its old Opus manual draft; reconnect now tests authoritative saved hints.
- **Full gates:**185 passed workspace tests, strict all-target Clippy, fmt/diff, guard37 and app
  build (UI197). Persistence checks include namespace isolation and malformed-choice fail-closed.
  Package186/install187 updated the normal launcher to `1.1.0-native-d43630fc7e1d`, preserving the
  previous binary and running session. Candidate/installed offline probes and the installed
  synthetic-provider workflow passed. Evidence: `fixtures/claude-cli-setup/connections-installation-observation.json`.
  Real native interaction/live CLI verification of this correction remains unverified. The current `latest` link still resolves into2.1.301; exact2.1.299 support is
  unchanged. No extra live calls.

## Initial guided Claude Code redesign (entry points superseded above)

- **Implemented:** normal New conversation now offers Claude Code local CLI separately from
  native providers. Engine-owned bounded filesystem discovery, selected-project capture, saved
  path/model hints, model/tool selectors and native path pickers replace required manual paths.
  One scoped confirmation coordinates metadata Check and journaled Create/Return; no prompt,
  Write approval, retry, backend migration or persistent grant is implied. Protocol is now40.
- **Measured:** full177 passed workspace tests, strict all-target Clippy, fmt/diff, guard37 and
  app build (UI196). Fake-CLI end-to-end setup discovers from a known installation location,
  inspects without executing, requires confirmation, performs three metadata probes and creates
  exactly the captured conversation with no inference. Core tests cover ambiguity, stale input,
  unknown checks, cancellation, superseded confirmation, unsupported saved models and reconnect.
  Headless GPUI checks chooser behavior, explicit visible confirmation and stale folder pickers.
- **Failed then addressed:**172 exposed a stale protocol39 assertion after the protocol40 bump;
  it now checks40 and discovery serialization. Strict lint findings were corrected.176 had an
  existing fake-preflight executable-busy failure; three isolated repeats and full177 passed.
  The cause of that transient busy executable is not established or claimed fixed by rerunning.
- **Observed compatibility blocker:** the user's installed `latest` symlink now resolves into
  Claude2.1.301. The exact2.1.299 qualification policy is unchanged; current-version login/live
  execution was not probed or qualified. New setup reports supported-version/login failures
  through closed actionable categories, without raw CLI output.
- **Unverified:** real native picker/focus/accessibility interaction for the new flow and live
  CLI qualification of this redesign. No additional live Claude CLI prompt was submitted; the
  original ten-entry ledger remains unchanged. Package/install evidence is recorded separately
  under `fixtures/claude-cli-setup/`.
- **Installed:** package178 and install179 updated the normal launcher to
  `1.1.0-native-c0d0d0f1b997`; prior executable retained unchanged, running session/profile left
  untouched. Offline probes and installed synthetic migration/coding/restart/archive/replay
  checks passed. Fully quit/reopen to use the redesigned interface.

## Claude CLI daily-use default and local installation (2026-10-15)

- **Implemented at the user's request:** normal owned desktop child/in-process engines enable
  chat/Write hosting without experimental flags. Explicit path trust, owner readiness, exact
  version matching and per-Write review remain required. No conversation backend is changed.
  Disable switches work; demo stays off; attached-server overrides are refused; standalone
  server/library defaults remain opt-in.
- **Measured:** full163 passed workspace tests, strict Clippy, fmt/diff, guard37 and desktop
  build (UI193). Release package162 built. Candidate and installed offline handshakes passed164.
  Installed mock-provider migration/coding/restart/archive/durable-replay workflow passed166.
  Seven installed argument-policy cases passed; the desktop launcher has no experimental flags.
- **Installed locally:** versioned `1.1.0-native-c820caa101f3` via the normal installer; the prior
  binary is retained byte-identical and existing profile data was not migrated or modified by
  installation. The running user's app was not stopped; fully quit/reopen to use the update.
  Evidence: `fixtures/claude-cli-probe/2.1.299/daily-install-observation.json`.
- **Failed checks, retained honestly:**160 caught the obsolete default-off UI wording assertion,
  which was updated alongside the requested behavior.161 had two fixture timeouts; each passed
  three focused repetitions and full163, without an established timeout root cause.165 supplied
  the workflow's fixture argument incorrectly and failed before worker startup;166 corrected it.
- **Not newly measured:** live CLI/default-launch GUI interaction, billing, managed policy,
  full containment, real IME/accessibility. No additional live inference; the ten-run budget is
  unchanged. Installed offline testing is not a new live CLI qualification.

## Local Claude CLI: opt-in Write-only testing checkpoint (2026-10-15)

- **Measured (automated).** Full jobs149/156/159 passed workspace tests, strict all-target Clippy,
  fmt/diff, guard37 and app/helper-example builds (UI193). Job157 adds viewport coverage to
  exact Copy, actual argument Tab reachability, expiry and long-argument scrolling regressions.
- **Observed (live/native).** Installed unmodified CLI2.1.299, normal login, private scratch,
  default-off host flags and address+PID guarded input. Native146 fresh/resumed chat Completed.
  Native158 resumed Write validated the complete copied review before Allow, then independently
  recorded terminal success, clean supervision, one consumed Allow and exact regular-file bytes.
  Explicit Changes refresh listed both scratch files in an inspector-only OCR crop. Native158
  exited0. Ten cumulative authorized attempts consumed; no further live allowance remains.
- **Failed, retained.** Earlier Unknown cases were never upgraded or resumed. Native146 fresh
  Write Completed but its missing argument tab stop caused premature approval: its full-review
  interaction FAILED. Job147 reproduced it; later fixes and native158 do not erase that failure.
  Job148's exporter-health assertion failed intermittently; 12 focused repetitions and full149/156
  passed, without an established root cause. Full-window OCR missed a working palette during
  restart checks; cropped OCR and temporary closed-field diagnostics corrected that observation.
- **Unverified / limited.** Billing, managed policy, full descendant containment, installed
  release packages, real IME/accessibility and display-presentation latency. Native short-argument
  navigation was exercised; nonzero overflow scrolling remains headless-only. OCR is not visual
  inspection or accessibility proof. No Read/Edit/Bash/MCP/subagent parity or sandbox claim.
- **Ready scope.** Experimental chat/Write-only testing of the built Linux desktop, not general
  production activation. Protocol39, engine schema25, desktop schema10. See the
  [testing guide](local-claude-cli.md), [evidence](claude-cli-bridge-evidence.md), and
  [immutable attempt record](../fixtures/claude-cli-probe/2.1.299/README.md).

## Local Claude CLI: offline recovery/handoff checkpoint (job110)

- **Measured (automated/offline).** Workspace tests, strict all-target Clippy, formatting and
  diff checks passed; UI187 headless tests. Creation and approval write-ahead recovery,
  metadata-only restart, original-owner receipt isolation, explicit fresh setup with a new
  UUID, redacted diagnostics, storage failures and owned fixture SIGKILL are covered. Git
  observation preserves the copied index timestamp to retain racy-clean detection.
- **Measured limitation.** An already-authorized fake CLI action can complete after its engine
  dies. Restart retains Unknown and never replays, but this is not crash/orphan containment.
  Test-only pidfd cleanup does not implement a production reaper.
- **Unverified.** Current installed helper/managed-policy envelope, real desktop approval flow,
  IME/accessibility, live account/billing and real CLI crash behavior. No new live request or
  native input was performed for this checkpoint. Production execution remains default-off.
- Details: [bridge evidence](claude-cli-bridge-evidence.md) and
  [recovery limitations](../crates/pipkin-app/CLI_RECOVERY_HARDENING.md).

## Candidate observation: engine-unavailable state, diagnose probe (2026-10-12)

- **Measured (automated).** `pipkin --diagnose --probe` on the release candidate against a
  scratch data directory: protocol 31, schema 20, engine probe OK, clean stop.
- **Observed (native, engineering).** The candidate launched with `--engine-attach` at an
  endpoint directory with no server, pinned through `scripts/guard.sh` and captured by
  window region on Hyprland 0.56 at scale 1.6: the centre reads "Engine failed: cannot
  attach to /tmp/no-engine: …" with the concrete reason (the permission rule for a 0755
  directory; "No such file or directory" for a missing one) and the sidebar offers only New
  conversation. Two launches, both closed cleanly; nothing else on the desktop was touched.
- **Unverified.** The red strip over a saved conversation, cached history rendering with
  the engine down, sending disabled, and the clear after a restart (needs the owner's daily
  profile). All other §7 gates: owner-run.

## MCP leftovers: keychain credentials, binary artifacts (2026-10-12)

- **Measured (automated).** Unit tests: the keychain store round-trips credentials through
  a fake `secret-tool`, scopes items by store label, reads its own pending writes, refuses
  a second transaction while one is open, reports a missing client as an error and parses
  the client's search output; artifacts are decoded, named by content hash, deduplicated,
  kept inside the artifacts directory and bounded at 16 MiB. Engine tests: a `secret:`
  reference resolved from the keychain reaches the fixture server's environment, a missing
  client keeps the file with a warning and the reference fails as before; an image in a
  tool result is retained as a file named in the result and reaches the model as an image
  block; a blob resource is retained and named by path. The repository gate passed.
- **Unverified.** A live Secret Service (gnome-keyring or KeePassXC) behind the real
  `secret-tool`: owner-run on the supported desktop.
- **Decided.** Legacy HTTP+SSE stays unsupported for v1 (no real server needs it).

## Subagent result artifacts (2026-10-12)

- **Measured (automated).** Engine test: a child that writes a file twice, fails a third
  write and answers with 20 KiB; the waiting parent gets the first 16 KiB, the continuation
  pointer and the file listed once (the failed write is absent); `subagent_result` reads on
  from the offset and locates text with `find` without returning a slice; the client call
  carries the bounded slice, its length and the artifacts. The repository gate passed.
- **Observed.** Nothing native: no desktop surface shows artifacts yet.

## Goal cost allowance and pinned auditor model (2026-10-12)

- **Measured (automated).** Engine tests: with the scripted model priced at a dollar a
  token, a $10 allowance pauses the goal after the first turn with the cost reason and the
  goal's dollars attributed from that turn; zero clears the limit; the action's wire form
  omits nothing and still reads an older desktop's form without the cost field. The
  `goals.auditorModel` setting routes the audit request to the pinned model while the
  executor keeps its own; an unknown pinned model pauses the goal naming the key and never
  substitutes another. The repository gate passed.
- **Observed.** Nothing native: the strip's "$x of $y" detail and `/goal cost` in a window
  are unverified.
- **Unverified.** A live model as auditor (owner-run with credentials).

## Web leftovers: citations, fallback chain, batch fetches (2026-10-12)

- **Measured (automated).** Engine tests: a Brave primary the fixture does not serve hands
  over to a SearXNG fallback (a misconfigured second fallback is skipped) and the result
  names the provider that answered; when every provider fails the last failure is the
  outcome; a batch fetch of three pages retains two sources, reports the failing page in
  place, refuses more than five or none. A core test keeps retained sources per
  conversation; a UI test covers the citation parser. The repository gate passed.
- **Observed.** Nothing native: clicking a citation in a window is unverified.

## Export configuration, OBS-3 (2026-10-12)

Export is configured through the engine from a Trace export settings page: off by default,
metadata as the first enabled level, a `content` level that adds prompts, replies and tool
texts bounded and redacted, the collector credential in the engine's private credential
store, a payload preview at either level and a Turn off control
([export.md](export.md)); protocol 28, capability `exportConfig`.

- **Measured (automated).** Engine tests: the configuration call refuses a missing or
  non-http endpoint and an unknown level, writes `export` to the user settings file and the
  health reports it at once; the credential lands in `export-auth.json` with mode 0600 and
  never in the settings file or the health view (only its header name), reaches the
  collector as the header, and stops once removed; turning export off keeps the rest; at the
  metadata level no prompt, reply or credential text is in any payload, at the content level
  the run's input and output, the completion and the tool arguments and output appear with
  the credential value and an `sk-` key replaced by `[redacted]`; the preview shows a sample
  or the session's own records at either level, redacted, refuses an unknown level or
  session, and sends nothing. Core tests cover the page's commands (one at a time, real
  engine only, the credential needing both parts and never printed, the preview landing with
  its level). The repository gate passed.
- **Observed.** Nothing native.
- **Unverified.** The page in a window; a real collector or Langfuse project. Not a
  release qualification.

## Trace export, OBS-2 (2026-10-12)

The engine's diagnostics records become OTLP/HTTP JSON spans posted to a configured
collector (Langfuse through its OTLP endpoint, with its observation attributes) from a
separate task with a durable per-session cursor, bounded batches, retry with backoff, a
drop policy and a reported health state ([export.md](export.md)); protocol 27, capability
`export`; the session dialog shows the exporter's state.

- **Measured (automated).** Engine tests against a recording mock collector: a parent run
  with a tool call and a child yields spans sharing one trace id, the child's run span
  parented on the parent's `subagent` tool span, generation spans with model and token
  attributes, and payloads that contain none of the prompt, task or tool output text; a
  restart resends nothing (the cursor is durable) while new work is sent; records from
  before export was enabled are never sent; a disabled exporter makes no request; a
  rejecting collector moves the state to backoff and the batch is dropped after the bounded
  attempts, while a slow collector and a rejecting one leave a turn and a Stop as fast as
  without export; the health view carries counts and a bounded error and never the headers;
  an invalid configuration is reported, not sent to. A core test covers the health line.
  The repository gate passed.
- **Observed.** Nothing native, nothing against a real collector.
- **Unverified.** A live OTLP collector or Langfuse project; OBS-3 (not built). Not a
  release qualification.

## Engine diagnostics, OBS-1 (2026-10-12)

The engine records bounded structured facts per session (run start with queue wait,
provider request timing with first-token time and attempt, retries, tool durations, Stop
latency, settlement, resumed runs), keyed by its own identities with parent/call for
children, read by cursor with a summary and exported as JSON lines
([diagnostics.md](diagnostics.md)); schema 19, protocol 26, capability `diagnostics`; the
session dialog gains an Engine diagnostics section.

- **Measured (automated).** Engine tests: a turn with a tool call leaves `run.start`,
  two `provider.request`, `tool.call` and `run.end` records that all name the run, with
  timings, token counts and the request key; paging by cursor and the export file (one line
  per record with provenance) hold; an unknown session is refused; a retried provider error
  records the error, the retry with its delay and the second attempt; a Stop records a
  cancelled request and an aborted run with its Stop latency; a child's `run.start` names
  its parent and the parent's call while the parent's figures exclude the child; records
  survive a restart, the resumed run is recorded, the interrupted exchange is not invented,
  the 2,000-row bound holds and the capability is advertised. Core tests cover one fetch and
  one export in flight at a time, real-engine gating, the answers, a refusal and the reset on
  disconnect. An app end-to-end test reads the summary and records through the dialog's
  commands and exports to a file under the engine's data directory. The repository gate
  passed.
- **Observed.** Nothing native.
- **Unverified.** The section in a window; first-token timing against a streaming adapter
  (the scripted provider does not stream); OBS-2/OBS-3 (not built). Not a release
  qualification.

## Questionnaires, ASK-2 (2026-10-12)

`ask_user_question` asks up to four related questions at once, with stable ids, single /
multiple / text kinds, bounded Markdown previews, per-answer and whole notes, engine-side
validation and a version-2 result that keeps partial answers, empty submissions and declines
distinct ([ask-user-question.md](ask-user-question.md)); protocol 25, capability
`questionnaires`; the desktop dialog, a draft kept across re-asks, and a *Needs your answer*
mark on any conversation.

- **Measured (automated).** Engine tests: the questionnaire is published with the model's
  ids (and `q1..`/`o1..` defaults), its source call and no deadline; unknown ids, duplicate
  answers, two choices for a single, a choice for a text, typed text where not allowed, a
  note where not allowed and an over-long text are refused with the questionnaire left open;
  a partial submission with an "other" text, two choices and notes is reported question by
  question with the right outcome and text; an empty submission with a default in place is
  unanswered-but-submitted (the default is not an answer), a decline is unanswered and not
  submitted; eleven argument bounds are refused before anything is shown; a restart re-asks
  the same questionnaire with the same ids and source under a new runtime id and the answer
  settles the original call without a new model turn; the single-question contract (ASK-1)
  and the extension question host's tests pass unchanged. Core tests cover the draft (seeded
  from defaults, per-kind toggles, unknown ids ignored, kept across a re-ask by the same
  asker, dropped otherwise), building the submission with blanks dropped and texts read only
  where allowed, one send per answer, and the attention flag for open and background
  conversations. A UI test covers the keyboard row order. An app end-to-end test has the
  model ask while another conversation is open, sees the mark, returns, edits the draft,
  submits a typed answer and a note, and the model receives exactly that. The repository gate
  passed.
- **Observed.** Nothing native.
- **Unverified.** The dialog in a window (mouse, keyboard, IME in its fields, both themes,
  narrow width, screen reader); preview rendering beyond the parser's blocks. Not a release
  qualification.

## Subagent controls and background children, SUB-2 / SUB-3 (2026-10-12)

A person can Stop or Steer one child from the inspector through engine-authorized requests,
and a `subagent` call can return at once while the child runs in the background with its
completion delivered exactly once ([subagent-controls.md](subagent-controls.md)); schema 18,
protocol 24, capability `subagentControls`.

- **Measured (automated).** Engine tests: a Stop records its note before the child's own
  Stop, the task settles as `aborted` through the child's run and the waiting parent's tool
  result says the child was stopped by the person; a child number from another session is
  refused and a finished child cannot be steered; a steer under one request key reaches the
  running child exactly once at its next step; `wait: false` returns a receipt, the parent
  gets exactly one follow-up under `subagent:<child>:done` and a restart delivers nothing
  again; a restart that interrupted the running child re-attaches the watcher and delivers
  once; a notify-only child is waited for with `subagent_wait` and never wakes the model;
  a person-launched child never asks the parent's model and repeats its request idempotently;
  stopping the launching run suppresses the wake while the result stays readable; the lookup
  tools read the durable row. Core tests cover selection-bound authorization, one request at
  a time, real-engine gating, settlement and refusal. A UI test covers the row projection. An
  app end-to-end test drives Stop and a person's launch through the native adapter. The
  existing restart, parent-stop, paging and profile regressions pass. The repository gate
  passed.
- **Observed.** Nothing native.
- **Unverified.** The controls in a window (keyboard, both themes, narrow width); a real
  crash between the note and the child's Stop rather than the ordering argument; thousands
  of background children (the directory bound is unchanged at 4,096 rows). Not a release
  qualification.

## Status line and context usage, STATUS-1 / STATUS-2 (2026-10-12)

The composer toolbar gains a compact status line derived from core state, and the engine
reports context occupancy from its own estimator ([status.md](status.md)); protocol 23,
capability `contextUsage`.

- **Measured (automated).** Engine tests: the figure is unavailable before any turn,
  reported with the scripted provider's request size after a turn and keyed by provider and
  model id, published as `ContextChanged` after settlement, and estimated again after a manual
  compaction; a pure-computation test covers reported versus estimated, the system prompt
  counted only when nothing was measured, staleness on a model switch, saturating remaining
  and retired entries. Core tests cover the derived line: empty segments omitted, counts from
  engine state (finished children excluded), context percent only from occupancy and
  capacity, unknown context unspoken. A UI test covers the compact labels. An app
  end-to-end test sees the reported figure reach the status line after a reply. The
  repository gate passed.
- **Observed.** Nothing native.
- **Unverified.** The line in a window at narrow widths, large text and both themes; the
  keyboard pass; STATUS-3 (not built). Not a release qualification.

## Background jobs, JOB-1 / JOB-2 (2026-10-12)

Commands run as supervised jobs outside the model turn with durable records, private
append-only logs read by byte cursor, whole-tree cancellation and exactly-once completion
delivery; the inspector gains a Jobs tab ([jobs.md](jobs.md)); schema 17, protocol 22,
capability `jobs`.

- **Measured (automated).** Engine tests: a start returns while the command runs and the
  turn continues; stdout and stderr land in the log in arrival order and are read by offset
  with continuation, including the tail form; non-zero exits, timeouts and cancellation are
  classified and a backgrounded grandchild is gone after cancel; output beyond the quota is
  dropped with the log marked truncated while the job completes; the per-session cap refuses
  a ninth job; a model job that asked to be woken gets exactly one follow-up with the tail,
  and a restart delivers nothing again; user-started jobs never wake the model and repeat a
  request key idempotently; a job whose run was stopped has its wake suppressed; an orderly
  shutdown stops a running job; after a simulated dead supervisor the orphan carrying the
  marker is killed and reported interrupted, a row without a receipt is reported unknown,
  nothing is relaunched, logs survive; pure reads are recoverable and `job_start` is not; the
  capability is advertised. Core tests cover syncs, the unseen count, log-tail fetching with
  coalescing and Stop gating; a UI test covers row projection; an app end-to-end test drives
  the Jobs tab through the native adapter. The repository gate passed.
- **Observed.** Nothing native.
- **Unverified.** The Jobs tab in a window; a real crash (SIGKILL) between intent and receipt
  rather than a reconstructed row; non-Linux orphan cleanup; JOB-3 (not built). Not a release
  qualification.

## MCP resources, prompts and elicitation, MCP-3 (2026-10-12)

Four more compact tools list and read resources (bounded slices, binary by size) and list
and render prompt templates; servers' declared capabilities are recorded and shown; a
server's request for input becomes native questions, typed per field, with decline, stop
and absence handled honestly ([mcp.md](mcp.md)); protocol 21.

- **Measured (automated).** Engine tests against the stdio fixture: resources and
  templates listed and searched, text read in slices with continuation offsets, binary
  reported by size, templates read by filled URI, unknown URIs as the server's error,
  `resources/list_changed` refreshing the cached list, counts and capabilities in the
  view; prompts listed with required/optional arguments, rendered with arguments, a missing
  required argument refused before the request, unknown prompts and bad names classified;
  pure reads in the recovery registry and the capabilities advertised; a server without
  resources reported beside the list and refused as unsupported; elicitation as four typed
  questions (integer re-asked on a non-number, enum as a choice, boolean as a confirmation)
  returning the typed content, declining a field declining the request, an empty form as a
  yes/no, URL elicitation declined without a question, and Stop cancelling the request and
  withdrawing the question. The repository gate passed.
- **Observed.** Nothing native.
- **Unverified.** Any third-party server's resources, prompts or elicitation; the questions
  in a window; subscriptions, completions, sampling and apps (not built). Not a release
  qualification.

## MCP management, HTTP transport and OAuth, MCP-2 (2026-10-12)

HTTP servers connect through the engine's own HTTP client, OAuth sign-in runs on the
engine's loopback/PKCE primitives with per-server token storage and refresh, and the new
Settings → MCP servers section adds, tests, reconnects, removes and signs in to servers,
switches tools off and imports pasted `mcpServers` configuration with a preview
([mcp.md](mcp.md)); protocol 20, capability `mcpManagement`.

- **Measured (automated).** Engine tests against an HTTP MCP fixture with a scripted
  authorization server: a 401 becomes "sign-in needed" and the search reports it per
  server; discovery through the challenge's resource metadata, dynamic client registration
  with a loopback redirect, PKCE and `resource` on the authorization request, exchange
  through the callback, the token reaching the server and never a view or event, revoke
  forgetting it, cancel settling without a credential, stdio servers refusing sign-in;
  tokens refreshing before expiry with the old one never reused, and a refused refresh
  asking for sign-in again without sending a stale token; add/edit (redacted literals kept
  on edit) /remove in user and project scope with invalid specs refused before writing,
  test reporting name/version/tools or the failure, reconnect clearing backoff, tool
  switches applying without a restart and refusing calls; previewed import writing only
  the chosen entries; `McpChanged` events. Core tests cover the page's state machine;
  adapter tests cover command-line splitting and spec conversion; an app end-to-end test
  drives the page's commands through the native adapter against the stdio fixture. The
  repository gate passed.
- **Observed.** Nothing native.
- **Unverified.** Any third-party server or authorization server; the section in a window
  (layout, keyboard operation, project switching); OS credential storage (not built). Not a
  release qualification.

## MCP servers and tools, MCP-1 (2026-10-12)

Configured MCP servers (stdio) are supervised by one engine-wide service with lazy starts,
cached catalogs, an idle reaper and bounded calls; the model reaches them through
`mcp_search`, `mcp_describe` and `mcp_call` ([mcp.md](mcp.md)); protocol 19 adds
`McpServers` and the `mcp` capability. Built on the official `rmcp` SDK pinned at 3.5.1.

- **Measured (automated).** Engine tests against a scripted fixture server: search, describe
  and call through the client contract with the resolved server/tool/revision in the durable
  result and an environment reference reaching the process but never a view; classified
  errors (unconfigured, unknown server/tool, invalid arguments against the cached schema,
  disabled, start failures with backoff, missing environment and secret references, a
  server's own error kept separate from transport failures, large results bounded); a lost
  reply times out as unknown effect and is never re-sent while the server stays usable; a
  restart during a call records it interrupted and never reruns it (the side-effect counter
  stays at one); a `tools/list_changed` notification refreshes the catalog; idle servers stop
  and restart on use, kept-alive ones stay, and a call longer than the idle allowance is not
  reaped; project settings add servers with project scope, a changed revision stops the old
  process and discards its catalog, removal makes the server unknown, invalid entries are
  warnings; the catalog persists across a restart; read-only profiles exclude the tools. A
  pre-existing recovery race (startup and a client's open both recording the same
  interrupted call) was fixed and is covered by the restart test. The repository gate
  passed.
- **Observed.** Nothing native.
- **Unverified.** Any third-party MCP server; Streamable HTTP; OAuth; resources and prompts;
  a native inspector. Not a release qualification.

## Independent goal audit, GOAL-3 (2026-10-12)

An audit-required goal sends accepted completion proposals to a read-only `auditor`
subagent whose JSON verdict completes, repairs (bounded) or pauses the goal
([goals.md](goals.md)); schema 16 (goals table rebuilt for the `verifying` state), protocol
18, plus bounded `list_files`/`search_files` inspection tools for every session.

- **Measured (automated).** Engine tests: an approved audit completes the goal through a
  child launched with the auditor profile and only inspection tools, with the verifying
  state published and the audit's tokens counted; requested changes feed the next turn's
  prompt, a second refusal pauses at the round limit, and skipping completes as unreviewed;
  an inconclusive or malformed verdict pauses and never approves; a file changed during
  the audit makes approval stale; Stop stops the auditor child and pauses, a late approval
  changes nothing, and a restart pauses with the pending audit stopped on resume before a
  fresh round approves. Pure tests cover verdict parsing (last JSON object only, unknown
  verdicts rejected), fingerprints and the findings section; tool tests cover listing and
  search bounds, ignores, binaries and path escapes. The repository gate passed.
- **Observed.** Nothing native.
- **Unverified.** A live model as auditor; audits on real repositories; the strip's audit
  text and `/goal audit` commands in a window. Not a release qualification.

## Structured goal progress, GOAL-2 (2026-10-12)

Goals carry criteria with engine-checked evidence, structured decisions from three goal
tools, person-set turn and token allowances and stall detection ([goals.md](goals.md),
schema 15, protocol 17).

- **Measured (automated).** Engine tests: a structured `continue` schedules the next turn
  and an observation-only claim stays unverified; a proposal naming a missing tool call is
  rejected with the unmet criteria and rejected items while a matching file hash is accepted;
  a proposal with a real tool call id for every criterion completes the goal and attributes
  the turn's tokens; `blocked`/`unreachable` need a reason and stop scheduling; a turn
  allowance pauses with its reason and resuming after raising it continues; a token
  allowance below one turn's usage pauses before the second turn; the stall guard pauses
  after three unchanged continued turns (configurable); revising the objective or
  replacing the criteria makes met claims unverified; a marker still works as a fallback and
  a turn with neither pauses; the tools are root-only and refuse without a goal. Pure tests
  cover evidence checks (missing, failed, open, hash match, stale, outside-project paths),
  judgment versus proof, stale marking and the progress signature. The repository gate
  passed.
- **Observed.** Nothing native; the strip's criteria and allowance text and the new slash
  commands were not exercised in a window.
- **Unverified.** Live-model use of the goal tools; token attribution against a real
  provider's late usage reports. Not a release qualification.

## Engine-owned goals, GOAL-1 (2026-10-12)

The native engine owns a conversation's goal ([goals.md](goals.md)): schema 14 records with
history and events, scheduling from explicit markers with committed intent, control calls and
desktop-record migration. Protocol 16, capability `goals`. The desktop runner is disabled
for conversations whose engine reports goal ownership.

- **Measured (automated).** Engine tests: continuation only on explicit markers and
  completion on `MET`; blocked, unreachable and ambiguous replies stop scheduling with their
  reasons and nothing is resent; Stop pauses the existing goal and a replacement runs on its
  own with the old one in history; a restart pauses the goal while the interrupted turn
  still settles, and resume starts a new epoch; no turn is scheduled with no client
  attached; a queued human message pauses the goal; migration imports a desktop record as
  paused exactly once and never replaces an engine goal; pause, resume, revise and archive
  controls; the capability and input validation. Core tests: engine ownership disables the
  desktop runner and routes set/pause/resume/clear as intent; a desktop-kept record is
  offered once and retired when the engine reports; the legacy runner keeps working without
  an engine report. End to end with the real engine: a goal runs two turns to completion
  with the desktop journaling nothing and goal prompts shown as their objective. All existing
  desktop goal tests pass unchanged. The repository gate passed.
- **Observed.** Nothing native; the strip's new Pause and Resume buttons were not exercised
  in a window.
- **Unverified.** Migration against a real pre-existing desktop database; behaviour across a
  live-model goal with tool use; the detachment rule with the real desktop closing. Not a
  release qualification.

## Subagent profiles, SUB-1 (2026-10-12)

The `subagent` tool takes a profile and a context mode; the resolved policy is frozen in a
launch record (schema 13) and enforced for the child's life
([subagent-profiles.md](subagent-profiles.md)). Protocol 15 carries the profile on rows.

- **Measured (automated).** Engine tests: a reviewer child is offered only reads, its
  `write` is refused and never runs, and its instructions arrive as a system section; a
  worker keeps everything but delegation and questions; a scout reaches the web but not
  `bash`; unknown profiles, bad context values, unavailable models and invalid thinking
  levels fail without creating a child and without substituting a model; misconfigured
  settings profiles refuse every launch with the reason; inherited context is a bounded,
  labelled excerpt with the parent's revision recorded, and fresh is the default; the
  total and concurrent caps refuse the extra call in the same transaction that would create
  the child; a profile edited after launch does not change a child recovering across a
  restart while a new child does get it; directory rows carry the profile. Unit tests cover
  profile labels, settings parsing and validation, launch-record round trips and excerpt
  bounds. Existing subagent recovery, Stop and paging regressions still pass. The
  repository gate passed.
- **Observed.** Nothing native; the inspector shows the profile text on a row but no
  walkthrough was done.
- **Unverified.** Live-model behaviour with restricted tool sets; profile overrides naming
  real provider models. Not a release qualification.

## Web access, WEB-1 (2026-10-12)

The native engine offers `web_search`, `web_fetch` and `web_read` with retained sources
([web-access.md](web-access.md)): schema 12, protocol 14, capability `web`.

- **Measured (automated).** Against a local fixture server: readable text extracted with
  scripts, styles and markup removed; titles and published dates taken from the page only;
  slices and `find` by character offset; relative and absolute redirect chains with the
  final and requested URLs recorded; redirect loops, credential-bearing cross-origin
  redirects and non-http schemes refused; Windows-1252 and `<meta charset>` decoding; JSON
  bodies; unsupported content types, compressed responses, oversized bodies and non-2xx
  statuses as typed errors that retain nothing; loopback, link-local, metadata-service,
  credential and `file:` URLs refused with no connection when private hosts are not allowed;
  Stop aborting a hanging fetch and the service reporting cancellation; SearXNG-format search
  with duplicate URLs dropped, no invented dates, empty results as success, and 429/403/
  non-JSON answers classified; missing provider or key as `not_configured` without key
  material in history; sources readable after a restart; expiry of the oldest content with
  the citation kept; per-call idempotent source recording; the tool order, recovery registry
  and capability. Unit tests cover the policy's address classes, entity decoding, charset
  parsing and attribute parsing. The repository gate passed.
- **Observed.** Nothing against a live provider or a real site.
- **Unverified.** Brave and SearXNG responses from live instances; behaviour behind the
  engine's HTTP proxy; real-world pages (encodings, size, hostile markup beyond the
  fixtures); the 8 MiB bound against a slow server. Not a release qualification.

## Plan view, PLAN-2 (2026-10-12)

The desktop shows the engine's plan ([plans.md](plans.md)): a strip above the composer, a Plan
inspector tab with a virtualized task list, expandable task detail, and status edits that go
back through the engine's plan service.

- **Measured (automated).** Core tests: a plan is unavailable until an opened session
  reports one and resets on reopen; other-generation and other-conversation plans land
  where they belong; expand/history survive resyncs and collapse when a task disappears;
  edits send one `UpdatePlan` under a fresh request key conditional on the revision, one at
  a time, with answers matched by request and refusals kept until dismissed. UI tests: row
  projection (open first, finished counted then listed newest first, expansion marked),
  labels and counts lines, in-place reprojection at a new revision. Adapter: the demo's
  simulated plan in the subagents scenario snapshot. End to end with the real engine: the
  model's `plan_update` reaches the desktop's projection; *Mark completed* and *Reopen* round
  trip through the engine and come back as its state with the recorded reason; an edit the
  engine refuses reports the rule and changes nothing. The repository gate passed.
- **Observed.** Nothing native yet: no walkthrough of the Plan tab, strip or edit buttons in
  a real window.
- **Unverified.** Keyboard-only expansion and edits, screen-reader naming of task rows and
  the error alert, narrow drawer layout, light/dark themes, and thousands-of-rows scrolling
  in a real window. Not a release qualification.

## Persistent plans, PLAN-1 (2026-10-12)

The native engine now owns a per-session plan ([plans.md](plans.md)): schema 11 tables, the
`plan_update`/`plan_read` tools, `UpdatePlan`/`Plan` calls, `PlanChanged` events, `plan` in
snapshots and a `plan` system section. Protocol 13.

- **Measured (automated).** Domain tests cover stable ids, in-batch refs, atomic rejection,
  revision conflicts, self/missing/cyclic dependencies, the dependency rule and waived
  (skipped) work, reasons for skip/cancel/reopen, stale completions and their clearing,
  archival and read-only archived tasks, reorder, parent cycles, bounds, evidence and
  rendering. Engine tests cover the model's batch reaching the snapshot, the published
  change and the next request's `plan` section; rejected batches as tool errors that change
  nothing; `plan_read`; client idempotency by key and `conflict` on stale revisions; store
  replay under a key; restart persistence; a child's separate plan; removal of plan rows;
  the capability and schema version. Existing schema-upgrade tests pass with version 11.
  The repository gate passed on this change.
- **Observed.** Nothing native: there is no plan UI yet (PLAN-2).
- **Unverified.** A process kill between a committed batch and its tool result; prompt
  cache behaviour on real providers with a changing `plan` section; large-plan paging in a
  UI. Not a release qualification.

## Model-visible questions, ASK-1 (2026-10-12)

The native engine now offers root sessions a built-in `ask_user_question` tool routed through
the existing session question host ([ask-user-question.md](ask-user-question.md)). This entry
records the automated boundary only.

- **Measured (automated).** Engine tests cover: the person's choice, free text and the
  *Other* follow-up reaching the next model request verbatim; refusal of a wrong or stale
  answer id; declining as a non-error `unanswered` result; Stop withdrawing the question and
  aborting the call; an engine restart re-asking the open question under a new id with one
  result and no second model turn; no connected client failing the call as `unavailable`;
  children not being offered the tool; the `askUserQuestion` capability. Unit tests cover
  the result contract, argument bounds and schema validation. The repository gate
  (`cargo test --workspace`, Clippy, fmt) passed on this change; see the commit for counts.
- **Observed.** Nothing native yet: the desktop renders these questions with its existing
  select/input dialog, which was previously observed for extension questions only.
- **Unverified.** Native keyboard-only completion, IME composition in the *Other* text
  field, narrow layouts, screen-reader interaction and a background conversation's question
  being noticed without reopening it. Not a release qualification.

## Latest recorded automated qualification (2026-10-06)

The owner reports heavy daily-use beta, including building Pipkin itself and using it on other projects.
The latest recorded path-completion working delta on app base `9dda58f` (committed as `6296cf2`) passed
**542 workspace tests (40 ignored)**, Clippy/fmt, **37 development-engine suite tests**, and **37 unpacked
packaged-engine suite tests** with Pi pin `d2a311097cbcf669e699479587332ae3988a49d0`. These are identified
historical checks of that delta, not automatic qualification of every later HEAD or an approved release candidate. The paired Pi fix passed its full check and **223 targeted durable tests**. Scratch-prefix
install, upgrade, rollback and uninstall checks also passed. A prior disposable native AT-SPI walkthrough observed
scan failure, retained stale diff, and guarded palette refresh/recovery; Stop/replacement-task behavior was qualified
with automated real-engine regressions, not a new native walkthrough. This is not a clean-system,
native visual-matrix/mouse/URI-handler, or screen-reader speech pass. Headless GPUI input/mouse/clipboard tests
do not close those native gates. Native and clean-system qualification remain separately tracked.
Full app/engine revisions, environment, artifact checksum, evidence boundaries, and open gates are in
[v1-release-gates.md](v1-release-gates.md).

## Historical prototype scorecard

The measurements, test counts, and recommendation below describe the original prototype; they are not current
release qualification. Later IME/Orca/scale observations are in `native-gate-testing.md` and the product plan.

Reference machine: see `docs/baseline.md` (Omarchy, Hyprland 0.56.2, Intel RPL-S via Vulkan, 2560×1080 @ 60 Hz, scale 1, Rust 1.99.0). Release build. Results are split into measured, observed, failed and unverified. Thresholds are proposed targets, not benchmarks.

### Historical automated snapshot (135 tests; not current HEAD)
- pipkin-core 13 · pipkin-ui 73 (composer 33 incl. GPUI input-handler tests, transcript 39, command registry 1) · pipkin-app 49 (all seven scenarios headless, determinism, storage incl. SIGKILL after acknowledged saves). 135 tests.

### Historical native/prototype gates
| Gate | Result | Evidence |
| --- | --- | --- |
| Native operation | **Partly verified.** Native Wayland launch, repeated resize 1440→1024→720 with correct re-layout, light/dark switch. Minimize/restore and suspend/resume **unverified**. | Screenshots in `docs/captures/` |
| Text editing | **Partly verified.** Typing, Enter-send, multiline paste, grow-then-scroll observed natively; emoji/combining/undo and IME-state rules covered by tests. **Real IME composition not exercised:** fcitx5 runs on this machine but has no composing engine installed (no input methods beyond keyboard layouts) and installing one needs system packages. | `text::` tests; native captures |
| Transcript selection | **Verified natively** for mouse drag across prose, list and code; Ctrl+C copy pasted into the composer. Offscreen/streaming selection verified by tests only. | `docs/captures/select-*.png` |
| Scroll correctness | **Verified natively** for the reading-above-stream case: after a real wheel scroll up mid-stream the viewport stayed identical across 4 s of continued streaming and "Jump to latest" appeared. Prepend, expansion, per-conversation restore and resize anchoring by tests only. | `docs/captures/scrolled-away-jump-to-latest.png`, `transcript::` tests |
| Keyboard and accessibility | Core workflow by keyboard works natively. **The live AT-SPI tree was inspected** (python Atspi): 3 landmarks (Navigation, Conversation, Changes inspector), a `log` "Conversation transcript" with per-message `article`s and tool buttons carrying state, a `status bar` "Run status: …", labelled entries and buttons, a `document frame` for the diff. Two labeling bugs found and fixed (empty headings, search entry named "Message"). Composer caret/selection are not exposed as AT-SPI text. **No screen reader was run** (Orca is not installed): unverified. | AT-SPI dump |
| Scale and themes | Both themes and three window sizes checked at scale 1. **125% observed** (this machine's display is now at 125%; the M2–M4 window checks were all taken at it, with layout and text correct in what was captured; not examined item by item). **150% display scale unverified.** Large text size not captured. | captures |
| Large histories | 10,000-message conversation opens at once, **4 mounted rows**, 200-item pages reach all 10,000 unique ids (test). Repeated-traversal memory **unmeasured**. | log, `adapters::tests` |
| Responsiveness | **Measured (pre-presentation).** Input-to-frame, from the platform input-handler call to the end of the composer paint: n=628 keystrokes at a 30 ms cadence, p50 3.3 ms, p95 4.8 ms, max 11.4 ms, including inside the 10,000-message conversation. Transcript CPU prepaint+paint over 600 frames: p50 1.1 ms, p95 1.7 ms. This is frame **submission**, not display presentation, and excludes compositor input delivery; adding up to one 16.7 ms vsync interval still leaves a wide margin against the 50 ms target, but the true presentation figure is **unmeasured**. Native scroll-frame p95 at 60 Hz not separately sampled. | `Log performance stats`, `text::latency` |
| Startup / memory | Window mapped in 175–218 ms warm (5 runs). **Approximate cold:** binary and font pages evicted from the page cache (posix_fadvise) before each of 5 launches: 197–258 ms; shared libraries and GPU driver stayed cached, so a true cold boot is **unmeasured**. Idle RSS **140 MB** (< 250 MB), 148 MB after 628 keystrokes in the 10,000-message conversation. "Mapped" is not first content frame. | measurement loops |
| Persistence | **Verified natively**: draft survived `kill -9` after "Draft saved"; relaunch restored draft. Acknowledged-save kill test passes. Injected failure covered by tests; native visual check pending. | `docs/captures/persist.png` |
| Application correctness | **Verified natively** (followup scenario): steer row "You (steering)" with Pi acknowledging it, two queued follow-ups listed with remove buttons, Stop moves the conversation to "Stopping…" and it stays there until confirmation, then "Run stopped." with the queue retained. Also verified by core and headless scenario tests: stale events dropped, unknown outcome never resent, palette and buttons share `availability()`. | `docs/captures/steer-queue-running.png`, `stopping.png`, `stopped-queue-kept.png` |
| Visual quality | Subjective, for owner review. Observed: consistent spacing and states in both themes. Known gaps: inline-code size, translucent window under the user's Omarchy opacity rule. | captures |

### Historical incident
During the native steer/queue/cancel pass another application (Minecraft) took keyboard focus mid-run, so some keystrokes intended for Pipkin may have reached it. Automation was stopped; subsequent input is guarded (sent only when Pipkin is the active window).

### Historical framework recommendation: **Conditional go** (not a v1 release approval)
GPUI delivered the hard parts on the target machine: real input-handler composer, variable-height virtualized transcript with document-level selection, diff virtualization, native Wayland rendering. Conditions before calling it settled, all blocked on things outside this session's remit (system packages, changing the user's display scale, root, or physical suspend): (1) a real IME composition test (install an fcitx5 engine), (2) a screen-reader pass (install Orca) and composer text exposure, (3) 125%/150% display scale and suspend/resume/minimize checks, (4) input-to-paint measured against display presentation (needs compositor presentation-time tooling), and a true cold boot. Estimated cost: a few hours of manual checks plus bounded fixes. Windows, macOS, other Linux desktops and X11 remain unverified.

### Historical capture matrix status
Captured (`docs/captures/`): dark streaming + diff, light + diff, light 1024 and 720, palette, selection and paste, persistence recovery, 10,000-message history, steer/queue/running, stopping, stopped with queue kept, scrolled-away with Jump to latest, empty state, dark 720 px with navigation drawer. **Not captured:** failure/recovery scenario, expanded-tool close-up, screen recording.
