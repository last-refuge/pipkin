# Orchestration dashboard implementation plan

Status: implemented and qualified on Linux under the owner’s 2026-10-19 request to finish,
test, commit and push the dashboard. This supersedes the earlier exclusion from goal6.
The settled UX decisions remain in
[orchestration-dashboard-contracts.md](orchestration-dashboard-contracts.md).

Protocol 42 now implements the activity directory, store-only observation, guarded Stop and
linked child follow-up. Desktop work includes the independent dashboard page and history
projection, canonical root drafts, separate persisted child drafts, a secondary execution
attachment, and acknowledged control receipts. Qualification results and platform/provider limits are recorded below. No live Claude prompts are authorized or used for this work.

## Objective

Make Pipkin's **Orchestration** dashboard the single place to observe and direct agents across
all projects and conversations. Running work must be discoverable without visiting its session;
finished work must stay inspectable and actionable. Opening the dashboard is observation, not
permission to start, resume, retry or cancel anything.

## 1. Product and navigation contract

- Add **Orchestration** immediately above **Commands** in the left sidebar's utility section.
  Show running/attention badges with accessible labels, independent of the selected project.
- Proposed shortcut: **Ctrl+Shift+A**, currently unused by Pipkin's registered bindings.
  Check compositor/platform conflicts during owned native testing; do not change system shortcuts.
  Add **Open orchestration dashboard** and **Return to conversation** to the command palette.
- Use a full workspace page, not a transient modal or a cramped inspector tab. Returning to the
  prior conversation restores its selection, draft, focus and scroll position without stopping work.
- Default scope is **All projects**. Provide project, agent type, provider and status filters,
  search, and an explicit return to the unfiltered view. Never inherit a hidden project filter.
- Desktop layout: overview/list on the left, focused conversation/activity and composer on the
  right. At narrow widths, selecting a worker opens its detail view with an explicit Back action.

Suggested layout:

```text
Orchestration     All projects ▾     Search…     Running 4   Needs attention 2

Overview / Running / Needs attention / Finished / All

Workers                                  Selected worker
Project · Conversation · Agent           Project / Parent conversation / Worker
Status · Current activity · Last update  State · Provider/model · Live/Syncing/Offline
                                         Conversation stream, tools and results
Needs attention                          Earlier history / Jump to latest
Running
Recently finished                        Target: Project A → Conversation B → Worker C
                                         [Draft for this worker…]
                                         Send next prompt / Queue / Steer / Stop
```

The **Overview** shows attention, active work and recently finished work together. Completion
must not make a selected worker disappear or replace its detail pane with another worker.
The Finished view provides paged older results; entries do not vanish after a timer.

## 2. Define exactly what is being orchestrated

- Include top-level conversation agents and native subagents across the connected engine's
  projects and sessions. Show actual parent relationships; do not invent a nested agent tree.
- Include compatible local-CLI root sessions using their existing immutable provider/mode and
  execution-readiness rules. CLI-internal workers or unrelated agents launched outside Pipkin
  are not enumerable unless an explicit integration exposes them. State this coverage clearly.
- Background jobs are related activity, not AI agents. Surface their running/error state inside
  the owning worker's detail and counts without counting processes as agents.
- Separate **worker identity**, **run/attempt identity**, and **observation state**. A root can be
  ready for another prompt while its previous run is finished; its children/jobs may still run.
  Count each worker once. Do not count an idle parent as running merely because a child is active.
- Retain terminal outcomes, result availability and prior attempts. Sending another prompt adds
  a new run/attempt; it must not erase the previous result or rewrite a finished task as if it
  had never completed.
- Execution labels come from authoritative state: running, queued, waiting for input/approval,
  stopping, completed/ready, failed, stopped, unknown. Show the specific reason where available.
  Quiet output is not proof of a stall, completion or a safe retry.
- Display freshness separately: Live, Updating, Reconnecting, Cached/Offline or Unavailable.
  Unknown or disconnected work remains visible; do not silently classify it as finished.
- Closed conversations remain closed for local organization but are visible in dashboard
  history with a Closed badge. Viewing their history is read-only. Explicit continuation must
  deliberately reopen them using the existing lifecycle rules, not bypass Close safeguards.

## 3. Existing foundations and gaps

Reusable foundations:

- `pipkin-engine-protocol/src/api.rs`: session events, revisions, durable catch-up/acknowledgment,
  run settlement, queue, questions, subagents, goals and job events.
- `pipkin-engine-protocol/src/view.rs`: session catalog, snapshots and execution identities.
- `pipkin-core`: guarded commands/events, submission journals, per-conversation drafts,
  availability rules, subagent controls, Close/Reopen and uncertain-outcome handling.
- `pipkin-ui/src/transcript`: selectable virtualized Markdown, per-frame streaming regressions,
  cached preparation, bounded caches, preserved height hints and scroll anchors.
- Existing subagent inspector, child history paging, explicit Stop/Steer, approval reviews and
  sidebar/palette patterns. See `subagent-inspector.md` and `subagent-controls.md`.

Gaps that must be addressed, not hidden by the UI:

1. `SessionSummary` lists roots and does not supply a global work/status index.
2. The native adapter's `Live.current: Option<Current>` owns one selected conversation mirror
   and one selected child. It is not a multi-session dashboard subscription model.
3. Existing child directories are session-scoped and capped at 4,096 rows without an engine
   directory cursor. An "all agents" dashboard cannot silently inherit that unreachable tail.
4. The child activity inspector is a bounded preview, not the full conversation renderer.
5. Current child steering refuses finished children. "Next prompt" for a completed child needs
   an explicit engine-supported continuation contract; it cannot be implemented as stale Steer
   or by treating a child session as an unrestricted root.
6. Many existing UI actions/composer paths use the current conversation/selected child. Dashboard
   actions require explicit target routing independent of that selection.

## 4. Engine-wide observation layer

Add a capability-negotiated, read-only activity directory and observation contract. Final RPC
names and storage migrations are implementation details, to be specified before coding.

- Provide paged root/child summaries across projects, with stable identities, parent links,
  committed model/backend/mode, current work, terminal outcome, timestamps, observed activity,
  attention reason, result availability, capabilities and revision/freshness metadata.
- Supply authoritative aggregate counts independently of the currently loaded row page.
  Never show a partial first page as a complete inventory; expose loading/errors/coverage.
- Add indexed/cursor-based access for both active and finished work, including children beyond
  the existing per-session directory cap. Avoid opening every session or reading every history.
- Pair directory snapshots with an event cursor/revision floor. Reconcile live updates without
  duplicate rows, stale status regressions, lost completions or pagination gaps.
- Reuse durable invalidations/catch-up where suitable. On gaps, refresh affected authoritative
  snapshots before acknowledging coverage; show resynchronization and fence unsafe actions.
- Add a separate bounded observation handle for the selected detail stream, with history paging.
  Opening this handle must not change the normal conversation attachment, execute extensions,
  submit prompts, activate CLI readiness, or restart work as a side effect of loading history.
- Subscribe to lightweight summaries globally. Keep full live transcripts only for the selected
  worker initially; release detail observers when switching/leaving. Coalesce output updates,
  preserve terminal/attention events, and enforce queue, memory and request concurrency bounds.
- Keep older/unsupported engines honest: advertise unavailable coverage and read-only controls
  rather than pretending to have a complete live dashboard.

## 5. Core model and safe command routing

Introduce framework-free dashboard state and commands, then thin adapter/GPUI projections.

- Stable keys include backend/engine namespace, session/root identity and child identity where
  applicable. Run/operation identities and attachment/observer generations are separate fences.
- Use a dashboard selection independent of `AppState.selected`. Never implement observation or
  Send by temporarily selecting a conversation behind the person's back.
- Every response/change/control is scoped to its captured target, generation, operation/request
  and relevant revision. Late responses cannot replace another worker's stream or affect it.
- Maintain attention/active/finished groups, filters, pagination, stable selection, per-target
  scroll state, observation state and bounded prepared row summaries outside render.
- Use canonical per-target drafts: the dashboard and normal conversation view must not keep
  divergent unsaved drafts. Switching target, filtering or navigating preserves the draft.
  A child's steering/follow-up draft must never enter its parent's composer by accident.
- Route commands through shared policy/admission helpers that accept an explicit target; do not
  duplicate or weaken the normal conversation's availability, durability and capability checks.
- A completed item's identity remains selected when its status/group changes. Do not continually
  re-sort rows by token activity while someone is reading, typing or navigating by keyboard.

## 6. In-dashboard orchestration controls

The focused detail pane shows the target project, conversation/parent and worker immediately
above its composer and actions. All actions report pending, accepted, refused or unknown outcomes.

- **Idle/completed root:** send the next prompt in that same conversation/context and provider.
- **Running root:** offer Queue follow-up and Steer only where actually supported, with clear
  differences between them. Stop targets the observed run, not whichever run happens to be new.
- **Active native child:** expose existing authorized Stop/Steer, scoped to root + child + run.
- **Finished native child:** implement an explicit Follow up contract. Recommended behavior is
  a new linked continuation task/attempt using an explicitly defined prior-context policy,
  preserving the old result, parent ownership and permission limits. Show the continuation
  relationship. Do not silently reopen a terminal attempt, copy context to a different provider,
  or fall back to an unrestricted root session. If a backend cannot support it, explain that.
- **Needs attention:** open the exact question or approval review in context. Reuse existing full
  argument review and one-shot decision semantics; summaries are never approval authority.
- **Navigation:** Open owning conversation, inspect related children/jobs, inspect results and
  available changed-file evidence. Preserve the dashboard's place when returning.
- Separate read-only observation from execution attachment/readiness. Establish an authorized
  control context only for the explicitly acted-on target, without stealing the normal viewer's
  attachment. A Live badge alone is not an execution grant; never fan out setup/probes to every row.
- Gate Send/Steer/Stop/Follow up on fresh authoritative state. Capture target and draft revision
  at activation; a row reorder, selection switch or delayed completion cannot retarget the action.
- Journal submissions/control receipts through existing durable paths. After a timeout/lost reply,
  offer status lookup/reconciliation, never blind resend or automatic retry.
- Opening the dashboard grants no new trust, CLI capability, tools, billing path or model access.
  No cross-backend fallback, hidden conversation creation or automatic context forwarding.
- Keep controls per-worker in the first release. Bulk Stop, broadcast prompts and autonomous
  orchestration need separate preview/confirmation and policy design; they are not implicit scope.

## 7. History, persistence and recovery

- Persist lightweight directory/history metadata as needed, filters/pins, per-target drafts and
  last dashboard selection. Namespace local organization by backend/engine identity.
- Keep durable engine history/results authoritative. Use paged retrieval rather than duplicating
  complete transcripts or approval payloads into a new dashboard database.
- Completion moves work into recently finished/history without deleting it. Older history stays
  reachable across app restart; unavailable/deleted server history is explicitly labeled.
- On reconnect, show retained data as cached until reconciled. Restore counts, observer cursors
  and selection only against current identities. Do not replay prompts, continuations or approvals.
- Keep observation, Close/Reopen, dashboard pins and explicit destructive deletion distinct.

## 8. Implementation milestones and acceptance gates

1. **UX and contracts:** settle navigation, identity/status/coverage model, capability matrix,
   independent viewer semantics and completed-child follow-up behavior. Specify APIs and any
   protocol/schema migrations with compatibility tests.
2. **Global observation:** implement indexed paged summaries, counts and snapshot/event recovery;
   expose root and native-child activity without selecting or executing sessions.
3. **Core and adapter state:** implement independent selection, bounded subscriptions/caches,
   stale guards, capability decisions and target-owned drafts.
4. **Dashboard shell:** add the sidebar entry, shortcut/palette actions, stable virtualized groups,
   filters/search, freshness/coverage labels and selected detail routing.
5. **Live stream and retained results:** share transcript machinery safely across independent
   viewers; implement paging, copy/selection, stable streaming tables and result/history browsing.
6. **Orchestration actions:** add targeted next prompts, queues, steering, stopping, review flows
   and the engine-supported child continuation path, with durable receipts and unknown handling.
7. **Persistence/recovery:** restore finished history, dashboard preferences/drafts and cached
   observations; prove reconnect/crash behavior without replay or selection leakage.
8. **Verification and release readiness:** run all gates below, document coverage and remaining
   platform/provider limitations. The current owner request authorizes commit and push to main after verification.

The first five milestones form a useful read-only oversight slice, **not** the completed feature.
The requested orchestration dashboard is complete only when action and recovery milestones pass.

### Required automated scenarios

- Multiple projects, multiple root conversations, active/finished children, and related jobs;
  all remain discoverable without selecting each session. Counts do not double-count workers.
- Running → waiting/approval → settled/failed/stopped transitions; finished selected detail stays
  visible and ready for its supported next action. Parent completion does not hide active children.
- Exact-target prompts/steers/stops while switching selection/projects, typing, filtering, receiving
  late responses, reconnecting or starting a newer run. No sibling/session receives the action.
- Completed-child continuation preserves prior evidence/context policy and creates exactly one
  linked attempt under its receipt; refusal/uncertainty cannot cause a replacement or replay.
- Closed/unsupported/CLI sessions respect existing organization, readiness and capability fences.
  Opening the dashboard triggers zero prompts, inference, CLI probes, approvals or work restarts.
- Durable snapshot/event handoff, out-of-order updates, duplicate terminal events, event gaps,
  loss of connectivity and crash during prompt admission or approval; no hidden uncertain work.
- More than 4,096 children and 10,000 directory/history rows remain reachable by paging with
  bounded mounting and caches. Full transcript subscriptions stay bounded independently of count.
- Concurrent summary update storms and a selected growing Markdown table: inspect every painted
  frame, preserve content/widths/reading anchors/selection, and retain the non-selected draft.
- `cargo test --workspace`, strict all-target Clippy, formatting and diff checks.

### Native acceptance (separate evidence)

Use an owned scratch profile/window and guarded input only. Verify sidebar placement, shortcut,
keyboard focus/return path, narrow layout, accessible names/status announcements, prompt-target
clarity and live streaming. Record frame pacing/memory under multi-session load; do not label
screen-reader, IME, scale or compositor behavior verified without measuring the real thing.
Use offline/fake backends for automated coverage. Existing CLI live-prompt allowance is exhausted;
any further live-provider qualification requires a separately authorized budget.

## Later orchestration capabilities (not first-release requirements)

Pinned multi-stream views with an explicit subscription cap; named agent groups; task hand-offs
and dependency graphs; launch templates; opt-in notifications; carefully confirmed bulk controls.
These should build on the same global identities, observation feed and command receipts, rather
than creating a second execution engine inside the dashboard.

## Delivery qualification — 2026-10-19

Milestones 1–7 are implemented. The desktop exposes **Orchestration** above Commands,
`Ctrl+Shift+A` / `Cmd+Shift+A`, overview/status/project/backend/kind/search filters, paged
worker history, Markdown/tool output, and explicit owner-review navigation. Dashboard reads
do not change the normal selection or open workers for execution. Root send/queue/steer use
one secondary attachment and the existing journal; child controls persist an acknowledged
receipt first. Follow-up creates a linked child under the original policy and includes the
recorded prior context, refusing oversized combined context before creating a task.

Protocol42 supplies the read and control APIs. Engine schema35 adds a covering index for the
child summary projection; upgrades back up the old schema first. The projection is maintained
by SQLite in the same transaction as its source records. A shared, short-lived composed cache
avoids copying the entire directory for each page. There is no separate mutable activity table.

**Observed in isolated native Wayland windows:** the sidebar entry and shortcut; independent
child history with the shared Markdown table/code renderer; child follow-up completing through
a local synthetic provider; separate child and ordinary-conversation drafts surviving navigation;
Escape returning to the original conversation; a split layout at 1440×960 and single-detail
layout with Back at 720×800 (logical window sizes). The windows used scratch profiles and
`guard.sh` for every synthetic key. They were closed after verification. The user's running
app, conversations, credentials and compositor configuration were not changed.

**Measured release performance:** ten requests over a 10,000-worker filtered fixture with
large retained child results: uncached page median **26.84 ms**, range **18.98–30.08 ms**;
cached page median **1.51 ms**. The projection index brought the fresh page below the 50 ms
threshold. Measurements include the local protocol request and response. Retained evidence:
`dist/orchestration-dashboard/performance.json`, native captures in the same directory.

**Unverified in this slice:** installed-Claude dashboard actions, other operating systems,
screen-reader operation, real IME composition, different display scales, suspend/resume,
long-duration multi-session memory soak and compositor presentation latency. No new live
Claude prompts were authorized or spent. Existing offline CLI lifecycle checks remain the
qualification for that backend. Reviews, job details, attachment changes and workspace changes
use **Open owning conversation / review requests** and its existing controls.

**Final automated gates:** `cargo test --offline --workspace --no-fail-fast`: **2,692 passed,
13 ignored, zero failures**, across 48 suite results. Strict workspace/all-target Clippy,
`cargo fmt --all --check` and `git diff --check` passed. Tests include exact-target secondary
send, stale observation, separate drafts, persisted/lost control receipts, native and fake-CLI
follow-up idempotency and original policy, oversized-context refusal without a stranded child,
stale Stop during CLI preparation, 4,100 children under one root, 10,000 workers without runtime
creation, schema35 backfill/backup and transactionally updated projection values. No Claude
prompts. Source/test-input manifest: `dist/orchestration-dashboard/tested-source.json`;
manifest SHA256 `d817a7b878615b38b407658a4f4d384c6da91682b8767c799939455bdc1f5960`.

**Release:** the locked release build passed, followed by an isolated `--diagnose --probe`:
protocol42/schema35, no sessions in the fresh profile, and a clean worker shutdown. Run the
updated desktop with `scripts/setup.sh --run` after quitting the previous app. The dashboard
is available from the sidebar or `Ctrl+Shift+A` (`Cmd+Shift+A` on macOS; macOS unverified here).

### Presentation refinement (2026-10-19)

The dashboard now gives the conversation priority over instrumentation. Two-line task rows
use state icons and short project names; selected tasks have an amber edge. A compact header,
status pill and familiar composer replace the metadata stack and full-width action buttons.
Filters, coverage/count explanations and task metadata are disclosures. Zero-count sidebar
badges are hidden. Errors, blocked reasons and uncertain-action recovery remain visible.
The owner-conversation and latest-history actions have labeled icon buttons; narrow windows
show one pane at a time. Existing target guards, drafts and execution semantics are unchanged.

Keyboard navigation stays inside the dashboard, supports forward/reverse Tab and preserves
marked IME composition. The composer, transcript, task details and filters remain independent
of the selected conversation. Native verification uses isolated local-provider fixtures;
no live Claude prompts are needed.

**Verification:** 2,693 workspace tests passed, 13 ignored; strict Clippy, formatting and
the release build passed. Guarded native checks covered the 1440×960 and 720×800 layouts
and keyboard access to all three disclosures. Evidence is retained locally in
`dist/orchestration-dashboard-redesign/`. No real-IME or screen-reader qualification is claimed.
