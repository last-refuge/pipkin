# Scorecard

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
