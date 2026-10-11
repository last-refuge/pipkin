# Subagent inspector

Subagents and workspace Changes are separate inspector tabs. The conversation header and
command palette provide a direct **Show subagent tasks** entry point, including when the
inspector is closed or uses a narrow-window drawer. Opening activity never moves the
composer away from the main conversation.

## Presentation contract

- A flat task directory reflects the actual root-session → child relationship. There is
  no invented nested tree, percentage, ETA, phase, or individual cancellation control.
- Titles are UI-derived from a short first task sentence (including an initial `You are`
  instruction), with a stable `Subagent <id>` fallback. They are not engine-authored agent
  names. The engine currently provides only a bounded task excerpt, not a complete prompt.
- Task rows wrap their descriptions. Running, Pending, Waiting, Completing, Completed,
  Failed and Stopped have explicit text and library icons; unknown statuses remain Unknown.
- Counts cover the whole available directory. Current work is visible by default; Completed,
  Failed and Stopped tasks move into **Show finished tasks (N)** history, collapsed initially.
  Unknown statuses stay visible rather than being mistaken for success. The header/tab badge
  counts active tasks, not accumulated history, and has no number when none are active.
- History is a UI projection, not deletion of engine records. Expanding it shows the latest
  available directory entries first (reverse engine order), bounded to 100 finished rows.
  Current work has a separate 100-row bound, so old history cannot crowd it out. A bound
  notice explains either limit. History resets to collapsed on conversation/generation
  changes, not routine status updates or selection changes. In Pi mode this is not pagination; see the native paging contract below.
- Selecting a task replaces the directory with its focused read-only activity view.
  **All tasks** returns to the directory. A selected task remains readable when it finishes,
  even with history collapsed or outside its bound (Pi mode; native paging is described
  below). Task execution status is independent of activity
  transport: Loading, Live, Snapshot, and unavailable/disconnected are not conflated.
- The activity view retains the adapter's bounded plain-text previews and shortening
  notices. It is not a full-history browser or a Markdown editor. Changes remain one tab
  away and retain the selected file/diff.
- Labels, excerpts, sorting and status counts are prepared on model updates, not by prompt
  parsing during render. Child content, subscription settlement, selection and stale-event
  rejection remain core/backend-owned.
- Palette entries **Inspect subagent <id>: …** and **Show all subagent tasks** provide
  non-pointer navigation. They exist only for an advertised directory; existing work is
  inspectable when new calls are disabled.
- History expansion is core state (`SubagentState::history`, `ToggleSubagentHistory`), so the
  palette and the directory agree. It resets to collapsed when a conversation is entered or
  re-attached, not on status updates or selection changes.

## Native engine paging contract (`--engine native`)

The native adapter marks its directory as pageable (`EventKind::SubagentDirectory`); Pi and the
demo keep sending `SubagentsSynced`/`SubagentView`, and **everything above is unchanged for
them** (100-row bounds and notice, newest-80-entry preview, "Recent activity preview" footer).
Pi and the demo answer a `LoadOlderSubagent` request with an honest
`SubagentOlderFailed` ("not available"); the core never sends one to them because their views
are not pageable.

**Directory.** The engine publishes up to `MAX_SUBAGENT_ROWS` (4,096) children per session,
oldest first; the core holds that directory, the UI projects it.
- Current work is listed in full (no 100-row cap). Finished history opens with its first page
  of 100 (newest first) and **Show N more finished tasks (shown of total)** reveals the next
  page (`ShowMoreSubagentHistory`). Pages are prefixes of one newest-first ordering, so order
  is stable and no task appears twice; a task finishing while history is open joins at the top.
- A selected task stays readable (title, status, activity) whether or not its row is revealed.
- If the engine reports its directory truncated, a notice says later tasks are not listed. The
  engine has no directory cursor in this build, so tasks beyond its 4,096-row bound are not
  reachable; within it, every task is.

**Activity.** Selecting a task reads its newest page (`SubagentHistory`, 200 entries,
`SubagentPage { has_older }`). The view previews every loaded entry (each preview still bounded
to 4,096 characters with "Preview shortened").
- **Load earlier activity** (top of the activity list) asks for the page before the oldest
  shown entry (`LoadOlderSubagent { child, before }`). One request is in flight at a time; the
  control shows "Loading earlier activity…", then the page is prepended above the reading
  position (the entry at the top of the view stays at the top). "Beginning of this task’s
  activity" marks the start. A failed page shows its reason and can be asked for again.
- Answers are stale-guarded in the core: conversation and generation, the selected child, and
  the exact `before` cursor in flight. Older pages add only entries strictly older than those
  shown; a tool call and its result split across a page boundary are merged once.
- Live updates: commits in the selected child's own engine session (entries, settlement) mark
  it dirty, and the adapter re-reads only the **newest page**, at most every 250 ms. If it
  overlaps what is shown, older pages already loaded are kept; if more new entries arrived
  than one page holds (continuity cannot be proven), the view restarts from the newest page.
- Selecting another task, returning to **All tasks**, or a directory change that removes the
  task clears its pages; selecting it again starts from its newest page.
- Previews are prepared in the core on each model update (only new or changed entries are
  re-prepared, text is shared, not copied); rendering does no I/O or parsing.

**Bounded mounting.** The directory and the activity view are virtualized GPUI lists: only
rows in view (plus a small overdraw) are mounted, whatever the number of tasks or loaded
entries. List updates splice only the changed range and keep the reading position.

**Keyboard and accessibility paths.**
- Palette: **Show more finished tasks** (enabled while more history exists) and **Load earlier
  subagent activity** (enabled while the selected task has older activity and nothing is in
  flight) appear for paged directories only. **Inspect subagent <id>: …** lists all current
  tasks plus finished tasks through the revealed pages (at least the newest 100, so finished
  work stays inspectable with history collapsed); Pi keeps its first-100 list.
- The history toggle, **Show more**, and **Load earlier activity** are labelled buttons
  ("Show more finished subagent tasks, N of M shown", "Load earlier subagent activity");
  loading, start-of-activity and errors are Status/Alert regions. Because rows are virtualized,
  Tab reaches only mounted task rows; the palette is the complete keyboard path.

## Reproducible, credential-free fixture

```sh
cargo run -p pipkin-app --release -- --demo subagents --data-dir /tmp/pipkin-subagents-demo
```

Use a new disposable directory. This scenario explicitly displays **Demo · simulated
agent**, with frozen running/completed/failed counters. Its child snapshots contain
additional simulated-content notices. It executes no counter, tool or provider request
and proves nothing about a real engine's progress. Demo child selection is allowed only
for an opened session explicitly advertising that synthetic service; it does not enable
subagents in unsupported demo sessions or change real-mode preferences policy.

## Verification — 2026-10-08

**Automated:** workspace tests, Clippy for all targets, formatting, and release build passed.
Presentation tests cover Unicode/long excerpts, engine statuses and shipped icon paths,
complete counts with a bounded directory, selected detail outside the directory bound,
conversation replacement, and independent loading/live/snapshot/disconnected labels.
Palette tests cover advertised versus unavailable directories and reading existing work
with new calls disabled. Demo/backend/core tests cover matching synthetic snapshots and
rejection of wrong child/conversation/generation events without modifying parent drafts,
transcript, run state, read-only policy or demo mode.

**Native measured:** launched only an owned throwaway-profile demo window on the current
Wayland workspace. Every synthetic key/text input used the PID-pinned `scripts/guard.sh`.
Captured dark desktop (1440×960), light desktop with Large text, and light Large-text
narrow-window drawer (720×960). After one fix batch, captured completed/running focused
snapshots and the deduplicated directory. Keyboard palette navigation selected Counter B,
returned to all tasks, and selected Counter A. The owned window was closed afterward.
OCR extracted titles, all three status labels, tabs, All tasks, Snapshot, child contents,
and the read-only/main-conversation routing footer. The initial owned-window AT-SPI tree
exposed task buttons with status/action labels and separate inspector landmarks.

**Limits / unverified:** the tool harness cannot open PNGs for visual inspection; captures
and OCR are not an aesthetic or contrast acceptance review. Screen-reader interaction,
owner acceptance, real-engine live/disconnection interaction, and native macOS interaction
remain unverified. The later 1010px capture still used the existing open drawer and is not
proof of a 280px docked inspector. Long-content/many-child behavior has pure tests, not a
complete native visual matrix. No owner profile, credentials, clipboard, compositor config
or unrelated window was changed. Local captures/logs are in
`/tmp/pipkin-subagents-visual.UjBulA/` (temporary, not tracked release artifacts).

### Finished-history follow-up — 2026-10-08

The owner reported that the new tab looks great, but completed agents accumulated.
The current-work/history split addresses that without deleting durable engine results.
Workspace tests, all-target Clippy and formatting passed for an isolated tree containing
only this follow-up's changes; release build and native checks passed before separate
concurrent session-Changes work began. That in-progress work is not part of this change.
Added tests cover
running → completed removal from default rows, retained selected detail, history expansion/
collapse, history bounds and reverse directory order, disabled-by-default history after
scope changes, and failed/stopped/unknown classification. Existing backend guards are unchanged.

An owned disposable demo window was tested using PID-pinned guarded keyboard input only.
Dark desktop and light Large-text 720px drawer captures/OCR show only Counter A in the
current directory, **Show finished tasks (2)**, and an active-only **Subagents 1** badge.
The palette still opened completed Counter B's Snapshot with history collapsed, then
returned to the collapsed directory. The window was closed and guard unpinned.
Captures: `/tmp/pipkin-history-native.yTQDou/`. PNG visual inspection is still unavailable
in this harness; the disclosure click path, actual engine completion transitions and Mac
interaction were not natively exercised. History visibility/order and completion transitions
are covered by pure presentation tests, not claimed as real-engine/native interaction evidence.

### Native paging — 2026-10-11

**Automated (measured in tests, not natively observed):**
- Core (`pipkin-core/tests/subagents.rs`, `src/subagents.rs`): 5,000-entry child paged to the
  start in 24 bounded pages with exact order and no duplicates; stale generation, wrong child
  and wrong cursor pages rejected; single flight; failed page retry; live newest-page refresh
  keeps paged-in history and restarts on a gap; selection change resets paging and drops late
  answers; boundary tool call/result merged once in both directions; Pi keeps one history
  page and the 80-entry preview window; history collapses on conversation re-entry.
- UI (`pipkin-ui/src/shell/subagents/tests.rs`, GPUI test window): 1,000 tasks mount fewer
  than 60 rows, 9 history pages reveal all 1,000 once, the oldest task mounts after scrolling
  to the end; 5,000 child entries mount fewer than 80 rows while 24 pages are prepended, and
  the entry at the top of the view stays at the top after each page. Palette tests cover the
  paging entries (native only) and the Pi first-100 list.
- Native adapter (`adapters/native/tests.rs`, fake engine): directory marked pageable with
  truncation; newest/older pages by cursor (`SubagentHistory { before }`); a refused page and
  a stale attachment answered with `SubagentOlderFailed` and no extra call; commits in the
  selected child's session trigger coalesced newest-page re-reads, other sessions do not.
- End to end (`adapters/native/e2e.rs`, in-process engine + scripted provider): a parent
  spawns three subagents; the long child (276 tool/assistant entries) is held at its final
  turn while the inspector pages to its first entry; releasing it delivers the answer live
  without losing the paged-in start; all three finish.
- Found and fixed on the way: the engine records finished tasks as `done`/`aborted`, which the
  native mapping rejected (directory went unavailable once a child finished); both spellings
  are now accepted.

**Not verified:** no native window, screen reader, or visual check of the new controls was
run for this change; directories larger than the engine's 4,096-row bound are not pageable.
