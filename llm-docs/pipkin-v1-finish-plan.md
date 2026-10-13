# Pipkin v1.0 finish plan

Owner-directed on 2026-10-12 (see the "Owner-directed v1.0 scope" section of
[pipkin-v1-release-plan.md](pipkin-v1-release-plan.md)). This is the working tracker for what
remains to call the built-in-engine Pipkin **1.0**. Items are marked here as they land; each
code item is committed only when the owner says so, after the repository gate passes:

```sh
cargo test --workspace && cargo clippy --workspace --all-targets && cargo fmt --all --check
```

Detailed requirements for each slice live in
[native-capabilities-implementation-plan.md](native-capabilities-implementation-plan.md);
the section numbers below point there. Every slice ships engine + protocol + core + adapter
+ UI (where it has one) + tests + docs + scorecard entry. "Engine done, UI later" is not done.

**Deferred past 1.0 by owner decision** (kept honestly marked in
[docs/v1-release-gates.md](../docs/v1-release-gates.md), never passed): screen-reader (Orca)
work, further soak tests, pressure and performance testing (the owner does this after 1.0),
the second-machine install/upgrade/rollback walkthrough, signing-key machinery, license and
provenance appraisal, documentation consolidation, native observation of the new feature
surfaces.

Legend: `[ ]` open · `[~]` in progress · `[x]` done (commit noted)

## 1. Managed jobs — JOB-1 / JOB-2 (plan §9)

Three finished slices wait on this: executable verification for goal audits (GOAL-3),
job-event waits (GOAL-2), and the delivery foundation for asynchronous children (SUB-3).

- [x] **JOB-1 supervisor and records.** (2026-10-12) `runtime/jobs.rs`, `store/jobs.rs`, protocol views
  and calls (start/status/list/read-log/cancel/result). Model tool `job_start` returns
  promptly with a stable id and launch receipt; `job_status`, `job_log`, `job_wait`/result,
  `job_cancel`. Launch intent persisted before spawn, receipt after; a crash between the two
  reconciles explicitly and never relaunches an ambiguous command. Engine-instance identity
  plus process nonce; a PID from a row alone is never signalled. States: starting, running,
  cancelling, succeeded, failed, timed-out, cancelled, interrupted, unknown. Logs are
  append-only private artifacts with byte cursors, bounded previews, retention quotas and
  explicit truncation. Completion is a durable event: one UI notification, and only by policy
  one idempotent model follow-up; restart/reconnect never duplicates either. User-started
  jobs default to notification only. Shared process-supervision primitive extracted from
  `bash` with its cancellation guarantees (process group, grandchildren, inherited pipes).
- [x] **JOB-2 native management.** (2026-10-12) Inspector Jobs view: active/finished split, command/cwd,
  elapsed, log tail (subscribed, coalesced), errors, Stop; typed links to plan tasks and
  subagents; the Subagents inspector stays as it is.
- [x] **Acceptance (automated).** Long build and dev-server fixtures keep the conversation
  usable while logs update; output floods cannot block Stop or grow memory/disk; cancellation
  reaches grandchildren without touching unrelated processes; a crash at each launch boundary
  never duplicates a command; reopen shows retained logs and truthful state; duplicate
  completion events produce one delivery; Stop/paused goals suppress automatic continuation
  while keeping results.
- [x] Docs: `docs/jobs.md`, scorecard entry, plan progress note.

JOB-3 (jobs outliving the engine) stays post-1.0.

## 2. Native status — STATUS-1 / STATUS-2 (plan §11)

- [x] **STATUS-1.** (2026-10-12) Compact status model derived on state changes: selected model/thinking,
  execution/waiting state, active jobs/children, usage summary that opens the existing
  dialog. Empty segments hidden; secondary detail collapses at narrow widths; keyboard
  reachable without noisy announcements.
- [x] **STATUS-2.** (2026-10-12) `ContextUsageView` from the engine's estimation/compaction path: tokens
  used, model capacity, remaining/output reserve where known, source (reported/estimated),
  measured revision, compaction state; updated after responses, model switch and compaction;
  stale/unavailable marked explicitly; never a percentage of cumulative usage.
- [x] **Acceptance.** Missing price stays unknown, not zero; reasoning/cache subsets not
  double-counted; child attribution does not inflate the root; context changes correctly after
  compaction and model switch; counts come from engine state, not mounted rows; coalescing
  keeps typing and scrolling smooth.
- [x] Docs and scorecard entry.

STATUS-3 (cached Git summary) is optional; take it only if cheap after STATUS-2.

## 3. Subagent controls — SUB-2 / SUB-3 (plan §6)

- [ ] **SUB-2 individual controls.** Engine-authoritative Stop child and Steer child with
  typed child authorization, durable request ids and settlement; the inspector offers them for
  the selected authorized child only (stale selection cannot stop a sibling); the parent is
  told how a stopped/failed child affects its wait.
- [ ] **SUB-3 asynchronous children.** Launch receipt, result/status lookup and durable
  completion delivery on the job notification infrastructure; a child stays a child session;
  user-origin launches do not authorize a model wake; model-origin launches use an explicit
  delivery policy.
- [ ] **Acceptance.** Existing restart, parent-stop and paging regressions pass; parallel
  calls cannot duplicate children after crash/replay; partial failures keep completed results;
  large outputs stay retrievable; inspector performance bounded for thousands of rows.
- [ ] Docs and scorecard entry.

## 4. Questionnaire contract — ASK-2 (plan §7)

- [ ] Versioned questionnaire: stable question/option ids, 1–4 questions, single/multiple/
  free-text kinds, optional bounded Markdown previews, per-answer notes and a global note;
  bounds validated in the engine; option labels are presentation, not identifiers; a default
  is never an answer until submitted; a note does not imply acceptance of unanswered
  questions; partial answers and cancellation stay distinct.
- [ ] Native presentation of grouped questions, multi-select and previews; a
  background-conversation attention badge; question durability beyond intent-level re-ask.
- [ ] Acceptance: bounds, ids, partial/decline/cancel outcomes, restart re-ask, children
  excluded; docs and scorecard entry.

## 5. Observability — OBS-1, then OBS-2 / OBS-3 (plan §12)

- [ ] **OBS-1 correlation and local diagnostics.** Engine identities reused; provider request
  timing, first-token timing, retries, tool duration, cancellation, queue wait and terminal
  outcome recorded as bounded structured records with provenance; model latency kept distinct
  from UI frame latency.
- [ ] **OBS-2 exporter interface.** Asynchronous sinks fed from stable engine events with
  parent/child correlation and committed usage; OTLP evaluated alongside the documented
  Langfuse mapping; bounded buffers, retry/backoff, drop/spool policy, visible export health;
  export failure never delays model or tool work; bounded flush at shutdown.
- [ ] **OBS-3 opt-in configuration.** Off by default; metadata-only first; prompt/tool/image/
  source content only with an explicit setting; credentials redacted before enqueueing; payload
  preview and a disable control; the existing credential abstraction.
- [ ] **Acceptance.** Scripted parent/child run yields correctly related traces and
  exactly-once local accounting; a slow or rejecting collector cannot block Stop, streaming or
  commits; disabled export makes no requests; metadata-only payloads carry no bodies; restart
  preserves correlation without resending history.
- [ ] Docs and scorecard entry.

## 6. Leftovers inside finished slices

- [ ] Web: desktop citation rendering (click to open a retained source), a configured
  fallback provider chain, bounded batch fetches (plan §5).
- [ ] Goals: cost limits beyond tokens, a pinned auditor model setting; job-event waits once
  JOB-1 lands (plan §8).
- [ ] Subagents: result artifact references, a small structured parallel group (plan §6).
- [ ] MCP: OS-keychain credential storage behind the existing store trait, legacy HTTP+SSE
  only if a real server needs it, image/binary content retained as artifacts (plan §4).
- [ ] Plans: goal linkage and scoped task references to children, a crash fixture at the
  batch/result boundary (plan §8 PLAN-1 note).
- [ ] Live-provider checks each slice deferred (Brave/SearXNG, a real MCP server, a live model
  as auditor), run with owner-authorized credentials and recorded separately from fixtures.

## 7. Owner-run native gates (supported desktop: x86_64 Omarchy/Hyprland/Wayland)

Procedures in [docs/native-gate-testing.md](../docs/native-gate-testing.md). Engineering
prepares the candidate and fixes findings; the owner runs and records each.

- [ ] Minimize/restore
- [ ] Suspend/resume during a run
- [ ] Cold boot
- [ ] Physical monitor unplug/replug
- [ ] 150% scale and large-text matrix, both themes, narrow window
- [ ] IME commit/cancel (fcitx5), including path completion
- [ ] Picker, drag-and-drop, clipboard, links and launching
- [ ] Engine-unavailable native state (actionable error, cached history, dispatch off)

## 8. Scope decision

- [ ] Explicit 1.0 statement for macOS (experimental build; no Developer ID/notarization; no
  interactive acceptance) and Windows (not supported), recorded in `docs/platforms.md` and
  the release notes.

## Still post-1.0 unless pulled in

WEB-2 extraction breadth; MCP apps / remote HTML (the Browse plan); JOB-3 and an independent
job daemon; multi-goal selection and workflows; the website; everything in the deferred list
above.

## Progress log

- 2026-10-12 — Plan created. Baseline: built-in engine default, Pi/Node retired; ASK-1,
  PLAN-1/2, WEB-1, SUB-1, GOAL-1/2/3, MCP-1/2/3 shipped (through `7fa5bab`).
- 2026-10-12 — JOB-1 / JOB-2 shipped: supervised jobs with durable records, private logs,
  exactly-once completion delivery, restart reconciliation, five model tools, the client
  calls and the Jobs inspector tab. Open from §1: nothing (JOB-3 stays post-1.0).
- 2026-10-12 — STATUS-1 / STATUS-2 shipped: engine context occupancy with reported and
  estimated sources, the derived status line and the usage dialog's context section. Open
  from §2: nothing required (STATUS-3 optional).
