# Orchestration dashboard: milestone 1 contracts

Status: **specification; owner decisions settled (§10).** This is milestone 1 of
[the dashboard plan](orchestration-dashboard-plan.md). It settles the navigation, identity,
status and coverage models, the capability matrix, the independent-viewer semantics and the
completed-child follow-up behaviour, and it specifies the protocol and storage changes and their
compatibility tests. Baseline `85d5d31`: protocol 41, storage schema 32. Milestone 2 has started:
the protocol 42 types and wire tests are in `pipkin-engine-protocol/src/activity.rs`; the engine
answers the new calls `Unsupported` and advertises none of the new capabilities until each one is
implemented.

All code references are to the baseline. Where the code contradicts an assumption in the plan,
this document takes precedence and says so.

## 1. Findings that shape the contract

These are verified against the code and constrain everything below.

| # | Fact | Where | Consequence |
|---|---|---|---|
| F1 | `OpenSession` is not a read. It calls `recover_session`, wakes compaction recovery, runs `ensure_extensions` and attaches demand. | `pipkin-engine/src/runtime/mod.rs:882` | Observation must never use `OpenSession`. |
| F2 | `shared.session(id)` materialises a `SessionRt` (resolves extension profiles, retains prompt resources). `HistoryPage`, `Subagents` and `SubagentHistory` all go through it. | `runtime/lifecycle.rs:44` | Observing history needs store-only reads (§4.4), or a list of 10,000 rows would build 10,000 runtimes. |
| F3 | `Shared::publish` broadcasts every `Event::Session` to every client, whether attached or not. Attachment is demand and resource retention, not subscription. | `runtime/mod.rs:156` | Live deltas for every session already reach the desktop. The risk is the opposite one: under load the 128-event / 2 MiB delivery queue collapses to `ResyncRequired` (`protocol/src/delivery.rs:8`). |
| F4 | `event_outbox` gets a row on every `sessions.revision` change, for root **and** child sessions. Retention is 4,096 cursors. | `store/schema.rs:172` | Durable invalidations can drive directory reconciliation without a new event stream. |
| F5 | Open questions live only in the in-memory `SessionRt` (`UiView`). CLI approvals are persisted (`external_permissions`). | `runtime/ask.rs`, `store/external/permissions.sql` | The attention state of an unloaded session can't include a question. A question only exists while its runtime is live, so the directory overlays live runtimes it finds and never materialises one. |
| F6 | `subagent_tasks` has separate `root` and `parent_session` columns, so nested children are representable. CLI orchestration children (`claudeCliAgent`) are also `subagent_tasks` rows. | `store/schema.rs:126`, `store/cli_agent_children.sql` | One child directory covers native and CLI-orchestrated children. The parent link is `parent_session`, not `root`. |
| F7 | `Stop { session }` stops whatever run is active. There is no run guard. | `api.rs`, `runtime/local_cli_runtime.rs:171` | A dashboard Stop can hit a newer run than the one observed. Needs a guarded `StopRun` (§4.5). |
| F8 | There is no project id. `SessionSummary.cwd` is the only project key; the desktop's `ProjectId` is derived client-side. | `view.rs:28` | Project filtering is by normalised `cwd`. |
| F9 | Handshake requires an exact protocol match (`runtime/mod.rs:781`). Capabilities are free-form strings. | `api.rs:126`, `runtime/mod.rs:794` | Every new call bumps the protocol. Capabilities express *configuration*, not version skew. |
| F10 | No index on `subagent_tasks.status` or `sessions.updated_at`. `ListSessions` returns every root, unpaged. | `store/mod.rs:740` | The directory needs a migration (§6). |
| F11 | Everything client-side keys on `AppState.selected`. That covers `availability()`, `Submit`, `EditDraft` and `submit_from_draft` (`selected.unwrap()`). The native adapter mirrors one `Current` with one `child`. Child steer text isn't a persisted draft. | `pipkin-core/src/state.rs:914, 1855, 2695`; `pipkin-app/src/adapters/native/mod.rs:818` | Target-parameterised core APIs are needed before any dashboard action (§5). |
| F12 | The workspace has no page abstraction. It is always nav + center + inspector, plus `Overlay`s. | `pipkin-ui/src/shell/workspace.rs:143` | The dashboard introduces `Page`. `Ctrl+Shift+A` and `Cmd+Shift+A` are unregistered. |

## 2. Navigation contract

- **Page model.** Add `Page { Conversation, Orchestration }` to `Workspace`. It sits beside
  `Overlay`, not inside it. The page swaps the centre and inspector regions; the nav stays put.
  An overlay (palette, prefs…) can open over either page.
- **Entry.** Add an **Orchestration** utility row directly above Commands
  (`shell/nav.rs:495`). It has an accessible name, e.g. "Orchestration, 4 running, 2 need
  attention". The badges come from engine-wide counts (§4.2), never from the loaded rows.
- **Shortcut.** `Ctrl+Shift+A` toggles the page (`Cmd+Shift+A` in the macOS menu). It is
  registered in `shell/actions.rs` with the other app bindings. A text input with focus doesn't
  consume it; check this in native testing.
- **Palette.** Two commands: "Open orchestration dashboard" and "Return to conversation" (the
  second is enabled only on the dashboard page). Each worker row also gets "Inspect <worker>"
  palette entries, as the subagent inspector does, so the virtualised list is fully reachable
  from the keyboard.
- **Return.** Entering the dashboard records a `ReturnPoint { conversation, child, scroll anchor,
  focus target }`. The return point is the selection itself, so drafts come back with it. Leaving
  the dashboard restores the return point, but only if that conversation still exists. It never
  re-attaches or bumps the conversation's `generation` unless it was actually detached. Entering
  and leaving the dashboard sends no `BackendRequest::Open`.
- **Scope and filters.** The default is **All projects**. Filters are project (normalised
  `cwd`), kind (root/child), backend (native/CLI chat/CLI coding/CLI orchestration), state
  group, and text search over title, task excerpt and cwd. An active filter is always shown as a
  chip with a "Clear filters" action. The dashboard never inherits `prefs.selected_project`.
- **Tabs.** Overview, Running, Needs attention, Finished, All. Overview shows Needs attention,
  then Running, then Recently finished; each section is capped and has a "Show all" link to the
  matching tab.
- **Narrow layout.** Below the existing inspector breakpoint, list and detail stack. Selecting a
  row pushes the detail view with an explicit Back button, and Escape also goes back. Focus
  returns to the row that was open.

## 3. Identity, status and coverage model

### 3.1 Identities

```text
WorkerKey   = { engine: EngineNs, session: SessionId }       // the worker's own session
RootKey     = { engine, root: SessionId }
ChildRef    = { root: SessionId, child: u64 }                // control authority (subagent-controls.md)
RunRef      = { session: SessionId, run: u64 }               // runs.id, globally unique per engine
ObserverGen = u64                                            // client-side, per detail observer
```

- `EngineNs` is the existing storage namespace (`controller.rs:303`: `native:{hash}`,
  `native-attach:{hash}`, `demo`). Rows from different namespaces never merge.
- A worker is keyed by **its own session id**, for root and child alike. This is unique
  (`subagent_tasks.child_session UNIQUE`). For a child, `ChildRef` is carried as well, because
  the engine authorises controls by `(root, child)` and refuses child numbers taken from another
  session.
- The parent is `parent_session` (F6). The dashboard draws real links only: a child row shows
  "in <parent title>" and its depth comes from the chain of parents. Rows are not grouped into a
  synthetic tree.
- A **run** is one execution attempt (`runs.id`). The worker's run history comes from `runs`
  rows. A new prompt or a follow-up creates a new `RunRef` (or a new linked child, §7); it never
  rewrites the previous outcome.
- An **observer generation** fences one detail observer's late responses (§5.3). It is separate
  from `ConversationState.generation`, which stays the attachment fence of the normal view.

### 3.2 Execution state

The engine computes one normalised state per worker. The client never infers it from output
quietness.

```rust
#[serde(rename_all = "camelCase")]
pub enum WorkState {
    Pending,                       // child created, first run not started
    Queued,                        // root has an admitted queue entry but no active run
    Running,
    Waiting { reason: WaitReason },
    Stopping,
    Completing,                    // child finished its run, result not yet recorded
    Ready,                         // root: idle after last run (or never run)
    Completed,                     // child: terminal success
    Failed { detail: Option<String> },
    Stopped,                       // root: last run aborted; child: aborted
    Interrupted,                   // run settled as Interrupted (engine restart etc.)
    Blocked { reason: String },    // SessionSummary.blocked (CLI readiness, etc.)
    #[serde(other)]
    Unknown,
}

pub enum WaitReason { Question, CliApproval, Input, #[serde(other)] Other }
```

Mapping from storage:

| Source | State |
|---|---|
| root with `runs.state='running'` | Running; Waiting(Question) if its live runtime has an open question; Waiting(CliApproval) if a pending `external_permissions` row exists |
| root `runs.state='stopping'` | Stopping |
| root with no open run, queued `requests` | Queued |
| root, no open run, last outcome Completed or no runs | Ready |
| root, last outcome `Aborted` | Stopped (still accepts a next prompt) |
| root, last outcome ModelError/NoModel/Failed | Failed{detail} (still accepts a next prompt) |
| root, last outcome Interrupted | Interrupted |
| `blocked` set | Blocked{reason}, overriding Ready/Stopped/Failed (never Running) |
| child `status` pending/running/waiting/completing | Pending/Running/Waiting(Input)/Completing |
| child `status` completed/failed/stopped | Completed/Failed/Stopped |
| anything else | Unknown, with the raw value kept in `detail` |

A root's `Ready`, `Stopped` or `Failed` describes its *last run*. All three accept another
prompt, and the UI says "Ready for next prompt · last run failed". Child terminal states are
terminal; the only way forward is follow-up (§7).

### 3.3 Groups and counting

- **Needs attention:** Waiting(Question | CliApproval), plus the client-local OutcomeUnknown (an
  uncertain submission, from the journal), plus Failed/Interrupted runs the person hasn't seen.
  Blocked does not count: it's a setup state, shown in place, not an alert.
- **Running:** Pending, Queued, Running, Waiting(Input), Stopping, Completing.
- **Finished:** terminal children; roots whose last run settled within the "recent" window
  (default 24 h, sorted by `ended_at`). Older settled roots are in **All** only.
- Each worker belongs to exactly one group, by precedence attention > running > finished. A
  parent is never counted as running because a child is running; it shows "2 children active"
  as a related-activity line instead. Background jobs work the same way (`jobs_active`): a line
  inside the owning worker's detail, plus a separate "jobs" count, never counted as agents.

### 3.4 Freshness

Freshness is client-derived and shown separately from `WorkState`:

| Label | Meaning |
|---|---|
| Live | Connected; row revision is at or past the last durable invalidation for its session |
| Updating | An invalidation arrived and the refetch is in flight |
| Reconnecting | Connection lost or `ResyncRequired`; rows are retained, controls fenced |
| Cached | Shown from persisted dashboard cache, not yet reconciled this connection |
| Unavailable | Engine lacks the capability, or the row's session was removed |

Disconnected or unknown work stays in its last group with a Cached or Reconnecting badge. It is
never moved to Finished on the client's own initiative.

### 3.5 Coverage

The engine reports coverage with every directory page:

```rust
pub struct Coverage {
    pub roots: bool,               // all root sessions are enumerable
    pub children: bool,            // all subagent_tasks rows are enumerable (no cap)
    pub live_attention: bool,      // question overlay applied (false only if runtimes busy)
    pub excluded: Vec<CoverageGap>,
}
pub enum CoverageGap { CliInternalWorkers, ExternalAgents, #[serde(other)] Other }
```

The dashboard always shows a coverage footnote. For example: "Includes Pipkin conversations and
their subagents. Workers that the Claude CLI runs internally, and agents launched outside
Pipkin, are not visible." Imported sessions are ordinary roots.

Closed conversations come back from the engine like any other row; Closed is client-local
(`prefs.closed_conversations`). The dashboard shows a Closed badge and a read-only detail view,
and offers "Reopen and continue", which goes through the existing `ReopenConversation` path and
its safeguards. Sending from the dashboard never implicitly reopens a closed conversation.

## 4. Engine observation API (protocol 42)

All additions are read-only. They need no extension, model, CLI probe or attachment, and they
must not materialise a `SessionRt`.

### 4.1 Capabilities

| Capability | Gate |
|---|---|
| `activityDirectory` | `Activity`, `ActivityRows` calls |
| `activityObserve` | `ObserveHistory`, `ObserveChildren` calls |
| `guardedStop` | `StopRun` call |
| `subagentFollowUp` | `FollowUpSubagent` call (§7) |

`activityDirectory` and `activityObserve` are always advertised by a protocol-42 engine.
`subagentFollowUp` is advertised only when subagents are enabled for the engine. A client that
speaks 42 but sees a capability missing shows that surface as Unavailable. It doesn't fall back
silently (see §8 for the fallback that is allowed).

### 4.2 Directory

```rust
Call::Activity {
    filter: ActivityFilter,
    group: ActivityGroup,                 // Attention | Running | Finished | All
    after: Option<ActivityCursor>,
    limit: u32,                           // clamped to 1..=MAX_ACTIVITY_PAGE (200)
}
Reply::Activity(ActivityPage)

pub struct ActivityFilter {
    pub cwd: Option<String>,              // exact normalised match
    pub kind: Option<WorkerKind>,         // Root | Child
    pub backend: Option<ExecutionBackend>,
    pub text: Option<String>,             // ≤ 256 chars; title/task excerpt/cwd, case-folded
    pub failed: bool,                     // only last run Failed/Interrupted (unseen failures)
}

pub struct ActivityPage {
    pub rows: Vec<ActivityRow>,
    pub next: Option<ActivityCursor>,
    pub counts: ActivityCounts,           // engine-wide for this filter, same read txn
    pub through: u64,                     // outbox cursor at the read; see §4.3
    pub coverage: Coverage,
}

pub struct ActivityCursor(String);        // opaque; engine-encoded (group, sort key, session)

pub struct ActivityCounts {
    pub attention: u32, pub running: u32, pub finished_recent: u32,
    pub failed_recent: u32,               // subset of finished_recent; see §3.3
    pub total: u32, pub jobs_active: u32,
}

pub struct ActivityRow {
    pub session: SessionId,
    pub revision: u64,                    // sessions.revision of this worker
    pub kind: WorkerKind,
    pub root: SessionId,
    pub parent: Option<SessionId>,        // parent_session for children
    pub child: Option<u64>,               // ChildRef.child
    pub cwd: String,
    pub title: Option<String>,            // roots
    pub task_excerpt: Option<String>,     // children, SUBAGENT_TASK_EXCERPT_CHARS
    pub profile: Option<String>,
    pub origin: Option<SubagentOrigin>,
    pub continues: Option<u64>,           // follow-up link (§7)
    pub execution_backend: ExecutionBackend,
    pub local_cli_mode: Option<LocalCliMode>,
    pub model: Option<String>,
    pub state: WorkState,
    pub raw_status: Option<String>,       // a child's stored status, verbatim
    pub run: Option<RunBrief>,            // current run or last settled run
    pub children_active: u32,
    pub jobs_active: u32,
    pub has_result: bool,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub sort_at_ms: u64,                  // group-specific anchor, §4.2.1
}

pub struct RunBrief { pub id: u64, pub phase: Option<RunPhase>, pub outcome: Option<RunOutcome>,
                      pub started_at_ms: u64, pub ended_at_ms: Option<u64> }

Call::ActivityRows { sessions: Vec<SessionId> }   // ≤ 64; refetch for invalidations
Reply::ActivityRows { rows: Vec<ActivityRow>, missing: Vec<SessionId>, through: u64 }
```

#### 4.2.1 Ordering

Ordering is stable, and token activity never re-sorts it:

- Attention: `sort_at` = when the attention began, oldest first. Attention is a queue.
- Running: `sort_at` = current run's `started_at` (child: task `created_at`), newest first.
- Finished: `sort_at` = `ended_at` / task `updated_at` at terminal, newest first.
- All: `created_at`, newest first.

The keyset is `(sort_at, session)`, so pages are stable under inserts. A row that changes group
leaves one ordering and appears in another; the client handles that (§5.4), not the cursor.

### 4.3 Snapshot/event handoff

1. Each `Activity` page and each `ActivityRows` reply is read in one store read transaction, and
   it reports `through` = `max(event_outbox.cursor)` visible to that transaction.
2. The client keeps the minimum `through` across its loaded pages as its **floor**. It tails
   durable invalidations from the floor using the existing `CatchUp`/`AckEvents`.
3. For each `DurableEvent { session, revision }`, the client:
   - if a loaded row for `session` has `revision >= event.revision`: ignore it (duplicate);
   - otherwise, if `session` is a loaded row, or a child whose `root`/`parent` is loaded (an
     invalidation for an unloaded child can change `children_active` on its parent): batch it
     into `ActivityRows`;
   - otherwise, mark counts dirty and refetch counts (a 1-row `Activity` page) at most every
     500 ms.
4. `ActivityRows` results replace rows only when `revision` is strictly greater than the held
   one. Equal or older results are dropped, which gives regression protection under reordering.
   `missing` sessions are marked Unavailable ("removed"), not dropped silently.
5. `EventPage.reset` (the cursor fell out of 4,096-cursor retention) or `ResyncRequired` means:
   fence controls (§5.5), mark rows Reconnecting, re-read every loaded page from the start of its
   group, then resume tailing from the new floor. AckEvents comes only after the refetch commits.
6. `Event::Session` deltas (F3) are used **only** by the detail observer for the selected worker.
   The directory derives nothing from them, so delivery-queue collapse under a storm affects the
   live detail stream, and the directory recovers through step 5.

Whether `children_active`/`jobs_active` on a root bump the root's `revision` is the one open
engine question here. If they don't, step 3 needs the parent rule above. Verify in milestone 2
and keep the rule either way.

### 4.4 Detail observation

```rust
Call::ObserveHistory { session: SessionId, before: Option<EntryId>, limit: u32 }
Reply::Page { native_history_start, entries, has_older }        // existing reply shape

Call::ObserveChildren { session: SessionId, after: Option<u64>, limit: u32 }  // ≤ 500
Reply::ObservedChildren { rows: Vec<ActivityRow>, next: Option<u64>, through: u64 }
```

- `ObserveHistory` is `HistoryPage` read **straight from the store**: no `shared.session()`, no
  recovery, no extensions, no attachment, no demand, no readiness. It works for root and child
  session ids alike, so a child's full transcript renders with the normal transcript machinery
  (closing gap 4 in plan §3).
- `ObserveChildren` is the cursor-paged replacement for the 4,096-capped `Subagents` directory,
  also store-only. Its rows are `ActivityRow`s, which already carry the parent, the follow-up link
  and the normalised state; `SubagentRow` keeps its shape. `Subagents` stays as it is for
  compatibility.
- Live growth of the selected worker uses the `Event::Session` stream already broadcast (F3),
  filtered by session id and stale-guarded by `ObserverGen`. A gap (`epoch/seq` discontinuity)
  re-reads the newest `ObserveHistory` page, the same continuity rule the subagent inspector uses.
- There is no "observe" registration on the engine. That's deliberate: nothing exists to leak
  demand or to need a release.

### 4.5 Guarded stop

```rust
Call::StopRun { session, expected_run: u64 }    // Control lane, like Stop
Reply::Stopping { run }
```

When `expected_run` differs from the active run, or there is no active run, the reply is
`Conflict` "That run already ended", and nothing is stopped. `Stop { session }` is unchanged and
stays the normal conversation view's call. A separate call (rather than an optional field on
`Stop`) keeps every existing `Stop` caller untouched, and a `StopRun` without its run fails to
decode instead of degrading to an unguarded stop. Stopping a child keeps `StopSubagent { session, child, request }`,
which is already scoped to `ChildRef`.

## 5. Client core contract (`pipkin-core`)

### 5.1 State

```rust
pub struct DashboardState {
    pub open: bool,
    pub return_point: Option<ReturnPoint>,
    pub tab: ActivityGroup,
    pub filter: ActivityFilter,
    pub sections: BTreeMap<ActivityGroup, Section>,  // rows: Vec<WorkerKey>, next, loading, error
    pub rows: HashMap<WorkerKey, WorkerRow>,          // bounded, §5.6
    pub counts: Option<(ActivityCounts, Freshness)>,
    pub floor: Option<u64>,
    pub selected: Option<WorkerKey>,                  // independent of AppState.selected
    pub detail: Option<Observer>,                     // at most one in the first release
    pub coverage: Option<Coverage>,
}
pub struct Observer { pub target: WorkerKey, pub gen: ObserverGen, pub entries: …, pub has_older: bool }
```

`DashboardState` lives on `AppState`. It never writes `AppState.selected`, `prefs.selected_*`
or `ConversationState.generation`.

### 5.2 Targets

```rust
pub enum Target { Root(ConversationId), Child { root: ConversationId, child: u64 } }
```

- `AppState::availability_for(target: Target) -> Availability`. `availability()` becomes
  `availability_for(current target)`. Both share one implementation, so the dashboard and the
  conversation view can't diverge on whether something is allowed.
- Every `Command` that acts gets an explicit target: `SubmitTo { target, draft_rev }`,
  `QueueFollowUpTo`, `SteerTo`, `StopTo { target, expected_run }`, `FollowUpChild` (§7). The
  existing `Submit` etc. become thin wrappers that fill in `current()`. `submit_from_draft` takes
  the target; there is no `selected.unwrap()` on that path.
- Each action captures `(target, draft.rev, row.revision, observer gen)` at the moment of
  activation. When the reply arrives, it is applied only to that target. Selection changes,
  reorders and completions in between can't retarget it.

### 5.3 Drafts

- Root drafts stay where they are (`ConversationState.draft`). The dashboard composer edits the
  same `Draft` through `EditDraftFor { target, … }`, so the two views never hold divergent
  unsaved text.
- Child drafts (steer or follow-up text) are new: `drafts_ns` gains a child key. Storage is
  namespace + conversation + `child` (0 = root), persisted through the same `SaveDraft` effect.
  A child's draft is never shown in, or sent from, the parent composer.

### 5.4 Selection stability

- When a selected row changes group, it stays selected, and the detail pane stays on it. The
  list keeps a "pinned" copy of the selected row in its old position until the person navigates,
  with a "Moved to Finished" marker.
- Rows don't reorder under the pointer or keyboard focus. Updates within a group that would move
  the row are deferred until focus leaves the list or 2 s pass without input.

### 5.5 Fencing

Controls are enabled only when all of these hold:

- the connection is Live;
- the row's freshness is Live;
- `availability_for(target)` allows the action;
- for root sends, the target conversation is attached for execution (§5.7).

Any fence shows the reason in place ("Reconnecting, controls paused").

### 5.6 Bounds

| Item | Bound |
|---|---|
| Rows held | 2,000 (LRU outside the visible window and selection) |
| In-flight `Activity`/`ActivityRows` | 2 per kind |
| Detail observers | 1 |
| Counts refresh | 500 ms coalescing |
| Prepared row summaries | built in update, never in render |

### 5.7 Execution attachment for dashboard sends

The engine's `Prompt` needs no attachment (it takes `session`), but the desktop's journal, run
state and receipt handling live on `ConversationState`, gated by `opened` and `generation`. The
first release therefore defines **send to root** as follows:

1. The person activates Send on a root in the dashboard.
2. The core asks the adapter for an *execution attachment* to that one conversation. It reuses
   `BackendRequest::Open` for that conversation's own generation, without changing
   `AppState.selected`. The adapter grows from one `Current` to `Current` plus at most one
   `Secondary` attachment.
3. Once `ConversationLifecycleReady(true)` arrives for that generation, the journaled submission
   proceeds exactly as in the conversation view. The draft rev and target captured at step 1 are
   checked again.
4. The secondary attachment is released when the dashboard target changes, or when the run
   settles while the page is closed.

Observation never triggers step 2; only an explicit action does. This keeps plan §6's rule that
the extension, CLI-readiness and recovery side effects of `OpenSession` (F1) happen only for a
target someone acts on.

## 6. Storage migration (schema 33)

```sql
CREATE INDEX sessions_by_updated ON sessions(updated_at DESC, id);
CREATE INDEX subagent_tasks_by_status ON subagent_tasks(status, updated_at DESC);
CREATE INDEX runs_by_session_ended ON runs(session, ended_at DESC);
```

The directory composes these queries with the in-memory question overlay (F5). The measured
10,000-row read exceeded 50 ms because child scans traversed large retained result/launch
records. Schema35 therefore adds `subagent_tasks_activity_cover`, a compact covering index of
the exact summary projection (including excerpt, result-presence flag and profile). This
replaces the proposed manually maintained `activity` table: SQLite backfills and maintains
the projection transactionally, without extra mutable summary state. Its excerpt expression
must match `SUBAGENT_TASK_EXCERPT_CHARS` (160). Existing rows/results are unchanged; upgrading
schema34 writes `engine.sqlite.v34.bak`. Older engines refuse schema35 normally.

## 7. Completed-child follow-up contract (proposed)

```rust
Call::FollowUpSubagent { session: SessionId, child: u64, request: String, text: String }
Reply::Subagent(SubagentRow)          // the NEW child
```

- Valid only for a terminal child (completed, failed or stopped) of `session`, authorised by
  `ChildRef`. Otherwise the reply is `Invalid` "That task is still running; use Steer".
- It creates a **new child** with:
  - `continues = Some(child)` (new nullable column `subagent_tasks.continues`, schema 33);
  - the same `parent_session`, profile, model/backend and permission limits as the original;
  - `origin = User`, delivery `notified` (it never wakes the parent model, matching
    `LaunchSubagent`'s user semantics).
- **Context policy (`continue`):** the new child starts from the prior child's full recorded
  transcript plus its result, followed by the follow-up text. A third `context` mode beside
  `fresh` and `inherit` is reserved for this purpose and isn't exposed through `LaunchSubagent`.
  Nothing from the parent beyond what the original child already had is added.
- It is idempotent on `request` (key `followup:<request>`). `Lookup`-style recovery comes from
  `SubagentRow` lookup by request, so a lost reply is reconciled, never resent.
- The original child, its result and its runs are untouched. The UI shows "Follow-up of
  <task>" on the new child and "Continued in <task>" on the old one.
- Refusals: the root is blocked or closed by the engine, subagents are disabled, the profile was
  removed, or the backend is `ClaudeCliAgent` with an invalid child policy. Each refusal has a
  specific message, and none falls back to a root session or a different backend.

## 8. Capability matrix

The rows in the first table are what the dashboard offers for each worker. Where the column
heading is a backend, the root is on that backend.

| Worker / action | Observe | Send next | Queue | Steer | Stop | Follow up | Answer/approve |
|---|---|---|---|---|---|---|---|
| Native root, Ready/Stopped/Failed | ✓ | ✓ | — | — | — | — | — |
| Native root, Running | ✓ | — | ✓ | ✓ | ✓ guarded | — | ✓ question |
| CLI chat/coding/restricted root | ✓ | ✓ if not Blocked | as `availability` today | — | ✓ guarded | — | ✓ CLI approval |
| CLI orchestration root (`claudeCliAgent`) | ✓ | per readiness; Blocked today | — | — | ✓ | — | ✓ |
| Native or CLI child, active | ✓ | — | — | ✓ | ✓ | — | ✓ question |
| Child, terminal | ✓ | — | — | — | — | ✓ if `subagentFollowUp` | — |
| Closed root | ✓ read-only | via Reopen | — | — | — | — | — |
| Any row while not Live | ✓ cached | — | — | — | — | — | — |

The second table shows what the dashboard offers depending on which engine it is connected to.

| Engine | Dashboard |
|---|---|
| Protocol 42 with `activityDirectory` | Full |
| Protocol 42 without it (not expected) | Unavailable |
| Protocol ≠ 42 | Handshake fails today, as it does for every other surface. No partial dashboard. |
| Demo adapter | Simulated rows with the "Demo · simulated agent" marker |

"Answer/approve" opens the existing question or CLI-approval review, scoped to that conversation
and run. Summaries never carry decision authority, and every decision goes through the existing
one-shot path.

## 9. Compatibility and contract tests

Protocol tests use the existing inline pattern: `round_trip` plus `serde_json::json!` wire
assertions, as in `view.rs:907` `import_tests`.

- **Wire names:** `Activity`, `ActivityRows`, `ObserveHistory`, `ObserveChildren`, `StopRun`,
  `FollowUpSubagent`, every `WorkState`/`WaitReason`/`CoverageGap` variant, and `ActivityCursor`
  as an opaque string.
- **Fail-closed decoding:** an unknown `WorkState` decodes to `Unknown`; an unknown `CoverageGap`
  decodes to `Other`; an unknown `ExecutionBackend` in a row decodes to `Unsupported`, and the
  client offers no controls for it.
- **Defaults:** `Stop` keeps its protocol 41 shape; `StopRun` without `expected_run` is refused.
  `ActivityRow.continues` and `raw_status` absent → `None`. As elsewhere in `Call`, variant
  fields are snake_case on the wire (`expected_run`), while struct fields are camelCase.
- `assert_eq!(PROTOCOL_VERSION, 42)`, and the `lib.rs` changelog comment gains an entry for 42.

Migration tests:

- A v32 → v33 upgrade with populated sessions, children and runs keeps every row and creates the
  indices. `continues` is NULL on existing tasks.
- A v33 database opened by a v32 engine is refused with `NewerSchema`.
- `.sqlite.v32.bak` is written.

Engine contract tests, which become the milestone 2 acceptance tests:

- `Activity` and `ObserveHistory` over 10,000 sessions build zero `SessionRt`s, call
  `ensure_extensions` zero times, and leave the extension-dispatch and CLI-probe counters at zero.
- More than 4,096 children are reachable through `ObserveChildren` and `Activity { kind: Child }`.
- Counts equal a brute-force recount under concurrent inserts and settlements.
- The handoff (§4.3) converges with out-of-order events, duplicate settled events and an outbox
  reset.
- `StopRun { expected_run: stale }` returns `Conflict` and the newer run keeps running.
- `FollowUpSubagent` creates exactly one linked child per request key; a replay returns the same
  child; a running child is refused; the original child is byte-identical afterwards.

## 10. Owner decisions (settled 2026-10-19)

The owner accepted every recommendation:

1. **Follow-up context (§7):** the new child starts from the prior child's full transcript and
   result, not the result alone.
2. **Secondary execution attachment (§5.7):** sending to a root from the dashboard uses one
   extra adapter attachment. The conversation selection never changes behind the person's back.
3. **Recent window:** Finished in Overview shows the last 24 hours.
4. **Shortcut:** `Ctrl+Shift+A` / `Cmd+Shift+A`.
5. **Unseen failures count as attention:** yes, until the person views them. Seen state is
   client-local; the client finds candidates with `ActivityFilter.failed` within the recent
   window, and `ActivityCounts.failed_recent` bounds how many there are.

The implementation does not rely on every child/job event bumping its root: coalesced global
invalidations refresh the directory, while row revisions fence individual updates (§4.3).


## Implementation notes — 2026-10-19 qualification

- The desktop uses a dedicated core dashboard state and a separate transcript projection. Its
  read calls use `ObserveHistory`; only an explicit root send/queue/steer opens a secondary
  execution attachment. Root text and attachments are submitted through the normal durable
  intent journal. Keyboard send/queue/steer/Stop routes to the dashboard target while it is open.
- Child follow-up uses a new request key, child session and `continues` link. Full recorded
  history/result is reference data; old execution requests and tool journals are never replayed.
  Oversized full context is refused before launch. Native and Claude orchestration children
  retain their original backend, model and frozen permissions; absent profiles refuse clearly.
- `LookupFollowUpSubagent` reads a committed receipt without admission, including after restart.
  Dashboard controls persist an acknowledged, namespaced receipt before dispatch. Unknown
  receipts restore as lookup-only; no automatic resend is allowed.
- Directory windows are bounded to 2,000 rows, entry requests to the existing history page
  limit, and selected history to 2,000 mapped items. Older-history paging pauses tail refresh;
  **Latest history** resumes it. Paging beyond a directory window starts a new bounded window.
- A short-lived composed directory cache is fenced by the durable outbox cursor and the set
  of live questions. It never opens session runtimes or probes CLI executables.
- Failure acknowledgments are local to the backend namespace and are sent as `seenFailures`
  revision hints on directory reads. They do not mutate engine history or grant execution.
- Review requests, job details, attachment management and workspace-change inspection use the
  explicit **Open owning conversation / review requests** action and their existing controls.
  Reading the dashboard never answers a question, grants approval, or reopens a closed session.

The Overview lists each worker once, with unseen failures taking attention priority. `total`
is the distinct worker count. `finished_recent` and `failed_recent` are outcome metrics for
24 hours, so an unseen failed worker can contribute to both the attention badge and the
finished outcome metric; these metrics must not be summed. The Finished tab retains older
results as well. Viewing a failure acknowledges its current revision only.

Child lifecycle publication bumps the parent where applicable, but the desktop also coalesces
all session invalidations into directory refreshes. Correctness does not depend on a child or
job update bumping a particular ancestor's revision. Directory refresh and detail observation
remain separate from the execution attachment.


## Recent-workspace visibility amendment — 2026-10-22

`ActivityFilter` accepts optional `inactiveDays` and `excludedRoots`. Omitted fields retain
the original unfiltered directory API. Orchestration sends its saved 30/60/90-day window
and desktop-local closed root identities. Live states, open runs, active child work and
background jobs bypass age/closed exclusion. Other rows must be within the inclusive
last-activity window and outside excluded root families. This predicate runs before
counts and pagination. With `inactiveDays`, All's cursor anchor is last activity rather
than creation time. Other group anchors are unchanged. Observation never updates activity.
Raw `ActivityRows` remains an unrestricted read; clients must apply the same predicate
before pinning its result into a filtered workspace. Visibility policy does not delete
sessions, cancel work, reopen a runtime, or change history access outside orchestration.

## Unreviewed completion amendment — 2026-10-22

Orchestration sends `reviewedCompletions` (including an empty map) to opt into successful
result attention. Each session acknowledgment identifies the execution backend, run ID and
run start timestamp; unrelated session revisions cannot make an acknowledged turn unread.
Terminal children without a run use their immutable session identity and creation time.
Clients that omit this optional filter retain the previous success-attention behavior.

Successful settled roots and completed child results show **Finished · unread**, an amber
node border/status and project attention count, and appear in Needs you/Overview until
explicitly marked reviewed or superseded by another turn. Waiting-for-input children also
count as attention. Engine counts, pagination, the graph, list and dialog agree. Finished
outcome metrics still overlap attention and must not be summed. The existing recent-workspace
window and closed-root exclusion apply to unreviewed settled results too.

**Mark reviewed** is available in the selected inspector, list detail and centered dialog.
It acknowledges only the captured session/turn and detail generation; stale clicks cannot
clear a newer result. Review state is saved in the desktop backend namespace and survives
restart and filter changes. Merely selecting, opening or refreshing a thread does not
acknowledge success. Reviewed results show **Awaiting next steps**. A new live turn clears
the completion presentation; its successful finish requires review again. These local
acknowledgments never send prompts, grant approvals or modify engine history.


## Manual visibility amendment — 2026-10-22

`ActivityFilter.hiddenProjects` and `hiddenSessions` are optional client-local exact-match
sets, omitted when empty. Project paths match row `cwd`; session identities match either
`session` or `root` so hiding a root also hides its descendants. Unlike age/closed exclusion,
explicit hiding overrides live states, pending approvals and active children/jobs. This
predicate applies before directory counts and pagination, to raw selected-row pinning, and
to the desktop's retained rows. The engine never persists or mutates sessions for visibility.

The core saves hidden project paths and hidden thread IDs with display title/project metadata
in dashboard preferences, independently of ordinary filters, recency and review receipts.
The metadata supports restoration even when no hidden rows are loaded. Hiding the selected
row clears its detail selection/generation and pending send, preserving drafts and ongoing
work. Show project does not clear individually hidden threads; Show thread does not clear a
hidden project. Show all clears only manual hiding. Restoration retains ordinary filters and
recency. The Hidden manager remains accessible in both graph and list, including an empty
workspace. No hide/show command sends a prompt, grants approval or attaches a runtime.
