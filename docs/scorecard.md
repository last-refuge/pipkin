# Scorecard

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
