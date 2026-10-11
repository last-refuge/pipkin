# Plan for a built in Rust engine in Pipkin

## Objective and completion contract

Replace Pipkin's internal Pi engine with a Pipkin-owned Rust engine, preserve every existing engine capability and user workflow, and use the integration to improve resource use, reliability, subagent visibility and installation. **Feature parity is a release gate, not an aspiration.** An opt-in partial implementation is a development milestone; it does not complete this conversion.

The engine will be a GPUI-free Rust library compiled into the Pipkin distribution and developed, versioned and tested with the app. The recommended production host is a private worker mode of the same executable, supervised by Pipkin. This is built into the product and requires no Pi installation or Node runtime; it retains a process boundary for crash isolation. An in-process host remains a benchmarkable option, using the same engine API. The topology decision must not change the parity contract.

This is an implementation plan, not an implemented or qualified replacement. The user's requirement supersedes the earlier proposal's optional subagents, restricted provider scope and permission to defer existing features. No existing feature may be marked dropped, deferred beyond completion, archive-only or legacy-only and still count as a successful conversion. Keeping Pi temporarily is the continuity mechanism while parity is incomplete, not evidence that the Rust port is finished.

### Execution ownership and the final Claude subscription handoff

**Claude Opus 5.5 is the primary implementer. The Claude subscription integration is reserved for Codex in this chat after the main implementation.** The user reports that Opus's safeguards prevent that work. This is a division of work, not an instruction to work around those safeguards, remove the feature or substitute API billing.

| Owner | Work |
| --- | --- |
| Claude Opus 5.5 | Main Rust conversion: engine/storage/tools, subagents, extensions, desktop integration, importer, generic provider/auth interfaces, other providers including ChatGPT subscription support, performance, packaging and tests |
| Codex, after the user brings the work back here | Claude subscription feasibility review, provider-specific authentication and request integration, credential reuse/import/removal/refresh, applicable migration behavior, tests and final integration qualification |
| User | Bring the completed main implementation and handoff record back to this chat; participate in required account/native acceptance |

The reservation covers the **entire Claude subscription path**, not just the sign-in button: subscription-specific endpoints/request behavior, authentication, refresh, credential handling, model availability, context/cache/usage behavior and live qualification. Generic HTTP/SSE, credential storage and provider lifecycle interfaces remain primary-implementation work. Ordinary Anthropic API-key support is distinct and may be implemented in the main work; it never closes the Claude subscription parity row.

Opus should proceed autonomously through the rest of the plan, leave this one workstream explicitly **reserved for Codex — not implemented or qualified**, and produce the handoff described in section 13. Do not repeatedly stop the main conversion on this known reservation. Do not implement the reserved integration indirectly through a bridge or test helper. Keep existing Pi functionality available and preserve its source/overlays as migration and reference inputs.

This changes sequencing only. Claude subscription feasibility remains unresolved until the final handoff, so some rework may be needed then. A main-implementation handoff is a legitimate milestone; native beta, default cutover and Pi retirement still require complete parity. The final review must verify the then-current provider requirements and supported integration path; this assignment does not pre-claim feasibility or successful qualification.

### Meaning of all features

The baseline is the pinned Pi engine actually shipped with Pipkin, **plus both packaging overlays, model data, existing configuration/provider/extension behavior, and the desktop integration**. Include engine capabilities that have no prominent UI button. Audit the pinned source before declaring this inventory exhaustive; do not reduce scope to the four familiar coding tools or the two sign-in providers in onboarding.

Preserve observable behavior, effective model context, durability, extension outcomes, account/billing paths and existing user data. Internal classes, language, Chord transport and process layout may change when no supported behavior is lost. A Pi terminal-only widget already unsupported by Pipkin is not a regression; record that baseline explicitly. Conversely, a working custom tool or experimental facet cannot be discarded just because it is implemented in TypeScript.

Use these dispositions in the parity ledger: **native equivalent**, **native implementation of the same contract**, or **not present in the baseline** with evidence. During development, **pending audit**, **blocked**, **reserved for Codex** and **legacy bridge** are honest temporary statuses, none of which passes final parity. New upstream Pi features after the frozen reference are tracked separately; changes shipped in Pipkin during this project must enter the ledger before cutover.

### Release gates

| Gate | Required evidence | Outcome |
| --- | --- | --- |
| Baseline complete | Full engine, overlays, providers, extensions, resources, storage and desktop inventory; reference fixtures and known defects | A bounded, traceable definition of everything to preserve |
| Native alpha | Durable scripted coding slice, Stop/reopen, isolated profile and explicit missing-feature list | Opt-in evaluation while Pi remains available |
| Main implementation handoff | All non-reserved work implemented and its evidence recorded; Claude subscription boundary tested with generic fixtures; section 13 handoff complete | Return to Codex for the reserved integration; not full parity or release approval |
| Native beta | All mandatory ledger rows implemented; migration, provider and extension conformance; initial resource gains | Evaluate a complete candidate without changing ownership silently |
| Native default | No missing baseline capability; migration/resume and installed workflows qualified; measured advantages; no correctness blockers | Native becomes the default for an identified accepted candidate |
| Conversion complete | Every required row passes without Pi/Node; supported existing histories resume equivalently; build/runtime no longer depend on Pi | Retire the transitional runtime and code with backups and fixtures retained |

If a provider or extension contract cannot be reproduced, retain the current working engine and report the blocker. A narrower product can only be a separately requested scope change; it cannot satisfy this plan's no-loss promise.

## 1. Reference baseline and source audit

Planning inspection on 2026-10-11: Pipkin `75f5c35f028bd2f85052ac990a20e982f35573ab`, initially clean working tree; engine source pin `d2a311097cbcf669e699479587332ae3988a49d0`. The expected adjacent `../pi-fork/pi` checkout is absent on this machine. Local app code, tests and packaging overlays were inspected; a complete upstream-engine audit and execution comparison remain Phase 0 work. Historical planning and release measurements are not new qualification evidence.

| Area | Source of current behavior | Consequence for the conversion |
| --- | --- | --- |
| Desktop state | `crates/pipkin-core/src/{backend,protocol,state,model,onboarding}.rs` | Preserve commands, lifecycle, goals, draft/journal semantics and stale guards |
| Pi integration | `crates/pipkin-app/src/adapters/pi/` | Replace calls while retaining attachments, session mapping, auth, questions, child inspection and Changes |
| Wire protocol | `crates/pi-client/`, protocol 8 | Keep as the reference/transition path; native gets a typed contract |
| Shipped engine | `packaging/pi-engine-revision`, `scripts/build-engine.sh` | Reproduce the exact pinned archive with both overlays, not an arbitrary latest Pi |
| Sign-in overlay | `packaging/pi-onboarding.patch`, `packaging/pi-onboarding/` | Preserve start/reuse/answer/cancel/remove, per-client isolation and secret handling |
| Subagent overlay | `packaging/pi-subagents.patch`, `packaging/pi-subagents/` | Subagents already ship; preserve execution, recovery, ownership, settings and child views |
| Model catalog | `packaging/engine-model-data.md`, pinned model-data archive | Retain provider coverage, capability metadata and reproducible provenance |
| Desktop persistence | `storage.rs`, `cache.rs`, submission journal | Keep ownership stable first; preserve drafts, preferences, goals, search and unresolved requests |
| Tests | `adapters/pi/{e2e,real_pi,runtime_tests,tests}.rs`, core and UI tests | Extract behavior tests; ignored Pi checks require a running reference before they count |
| Product contracts | `docs/{extensions,subagent-inspector,support,packaging,platforms}.md` | Preserve actual workflows; some older documents describe the prototype and need source cross-checks |

The frozen engine identity must include `pipkinAuthBridgeSha256`, `pipkinSubagentsSha256`, model-data manifest hash, dependency lockfiles, runtime version and artifact hashes. Development-override builds are not automatically equivalent to the clean pinned package.

### Phase 0 audit procedure

1. Obtain a disposable read-only reference checkout/archive at the recorded pin; stage both overlays using the packaging process. Never patch the owner's separate Pi checkout or edit/build `zed/`.
2. Read the actual experimental server/session-worker composition and `packages/durable` specification, storage, scheduler, task/tool replay, context edits, compaction and tests. The older `packages/agent` loop alone does not define this engine.
3. Enumerate all registered tools, task types, extension/facet hooks, services, provider adapters, auth methods, model overrides, environment/config options, prompt resources and history operations. Include reachable optional branches, not only default settings.
4. Trace prompt admission, lost acknowledgement, Stop, steering, follow-up, crash during mutation, subagent creation/result delivery, sign-in refresh/removal, compaction and session switching. Record transactions, side effects, checkpoint/replay rules and event ordering.
5. Create `llm-docs/pipkin-rust-engine-parity.md` when implementation starts. Each row needs an ID, source and revision, baseline behavior, fixture, native owner/module, migration mapping, differential assertion, platform/provider coverage and evidence status. Enumerate provider and extension rows individually; one generic checkbox is insufficient.
6. Capture reference behavior with deterministic providers and disposable repositories. Record known defects rather than copying a defect or silently redefining parity: preserve the intended capability, document any correction and its regression test.
7. Measure current app-plus-engine startup, memory, CPU, serialization, database/output growth and package size. Old reports of a 453 MiB package and 625–702 MiB aggregate engine RSS are historical leads, not current baselines.

## 2. Mandatory feature preservation matrix

These are initial required groups, not a claim that the missing engine source has already been exhaustively audited. Every newly discovered baseline capability adds rows and work; it does not become optional automatically.

| ID | Preserve | Native implementation and acceptance proof |
| --- | --- | --- |
| P01 | Multi-turn generation, text/thinking streams, tool calls/results, errors, retry policy and provider context | Typed content blocks with opaque provider metadata; compare complete provider requests, tool ordering and terminal outcomes |
| P02 | Every installed coding tool and execution environment | Port `read`, `write`, `edit`, `bash` first, then every additional audited tool; compare arguments, outputs, file effects, timeouts and recovery |
| P03 | All provider/auth paths supported by the shipped engine and configured custom endpoints | Enumerate adapters and settings; per-provider golden requests/streams, capability tests and authorized live qualification; retain account and billing identity |
| P04 | Claude and ChatGPT subscription onboarding, saved credential reuse, removal, cancellation and refresh | Port the current `anthropic`/`openai-codex` contracts; API-key support is not a substitute; stale attempt and concurrent refresh tests |
| P05 | Catalog, conversation creation/rename/reopen, projects, stable item identity and paged history | Compare catalog and history projections before/after restart and import, including background sessions |
| P06 | Durable submissions, unknown outcomes, steering, follow-ups and withdrawal | Journal→admission→lookup reconciliation; identical accepted ordering; duplicate-key and crash-point tests |
| P07 | Stop, retry Stop, recovery and stopped-instruction retirement | Provider/tool/task cancellation plus committed settlement; inspect the next model context after replacement prompts/goals |
| P08 | Goals, continuation scheduling and goal replacement/pause/clear behavior | Retain the existing core as goal owner; test terminal event ordering and no continuation after a user Stop |
| P09 | Subagent execution, durable parent/child relationships, enable setting and recovery | Port the durable task behavior and overlay contract; child creation/result deduplication, restart after disabling new calls, cancellation and provenance checks |
| P10 | Subagent directory, live/snapshot activity, finished history and read-only child inspection | Match `list/activity/view/page/open/setEnabled`, ownership checks and stale selection guards; retain main-conversation draft/submit routing |
| P11 | Attachments, model/thinking selection, model refresh and usage/cache accounting | Preserve preprocessing, limits and capability rejection, per-session settings, cost metadata and provider-switch context |
| P12 | Prompt construction, project instructions, skills, templates and resource discovery | Golden precedence/discovery/context tests, including nested paths and dynamic resources; no silent prompt rewriting |
| P13 | Context management, compaction, omissions, reset/branch/history operations found in the audit | Compare effective next-request context and archival history; retain ordering, summaries, opaque blocks and context boundaries |
| P14 | Working extensions, custom tools/providers, hooks and experimental service/facet behaviors | Per-extension compatibility fixtures and durable-state import; questions alone do not prove extension compatibility |
| P15 | Select/confirm/input questions, notices and status | Preserve IDs, validation, deadlines, cancellation, reconnect and current question limits; pending questions cannot hang recovery |
| P16 | Tool output preview, complete retained output copy/save and artifact access | Retain outputs and attachment references across restart/import, with honest truncation and private file permissions |
| P17 | Offline reading/search, saved drafts, attachments, preferences and recovery UI | Keep the app database/cache contract; verify offline reopen, search navigation and failure acknowledgements |
| P18 | Session-scoped Changes, diffs, editor/terminal actions | Preserve successful edit/write attribution, staged changes, retirement after commit and unrelated-file exclusion; compare actual Git effects |
| P19 | External engine attachment, existing flags/config and multi-client behavior where supported | Keep the Pi adapter during transition; inventory native equivalents/compatibility shim requirements before removing any option |
| P20 | Packaging, diagnostics, profile isolation, upgrade/rollback and platform lifecycle | Installed no-Node native workflow; retain Linux scope and existing macOS work without inventing new platform qualification |

### Providers and extensions are critical-path work

Sequence providers as deterministic/local fixtures, a real API-key coding slice, ChatGPT subscription support and all other baseline adapters/custom-provider mechanisms, then the reserved Claude subscription work after the main implementation handoff. **This is delivery order, not scope reduction.** Verify request conversion, auth/refresh, reasoning, caching, images, tools, usage, model configuration, retries and transport peculiarities separately. Recheck provider endpoint and distribution requirements using current official documentation during implementation; source compatibility is not proof of live account compatibility.

A Rust port cannot execute arbitrary Node-dependent JavaScript by translation alone. Phase 0 must inventory what the shipped engine actually loads and decide the compatibility mechanism before promising a completion date. Port bundled behavior to native modules; design a versioned native tool/hook/provider API. For existing user extensions, preserve the supported contract via a proven compatibility layer or provide an equivalent port with migration. Test custom state, hooks, tool wrapping and recovery—not just startup.

If an embedded JavaScript runtime is considered, prove the required Node APIs/native modules and failure semantics in a spike; a generic JS interpreter is not Node compatibility and adds its own package/runtime cost. A legacy Node bridge is allowed during migration, but does not pass the all-Rust/no-Node completion gate. If arbitrary executable-code compatibility makes those goals incompatible, record the unresolved requirement and retain Pi; do not describe a narrowed extension subset as full parity. Already unsupported stable-Pi TUI widgets and slash commands remain documented separately in `docs/extensions.md`.

## 3. Target architecture

### Execution topology

**Recommended: one shipped Pipkin executable, an owned native engine child process, no Node/npm payload.** A private internal worker mode starts before GUI initialization; the engine itself is a GPUI-free library so a dedicated headless executable can be built later without redesign.

This retains crash isolation and independent engine restart without copying Pi's detached coordinator/worker tree. The desktop supervisor owns an actual child handle, private launch identity and child connection; no global server discovery or environment-based sweep of all user processes is needed in native mode. Switching conversations does not create new engine processes.

An in-process host removes IPC but couples engine failures and shutdown to the UI. Use direct typed channels in the headless harness and benchmark an optional in-process host against the supervised-child host with the same fixtures. Record the production choice in an ADR; neither host may put work on the render thread. A subprocess is **not a sandbox**: tools retain the user's permissions unless a separately qualified isolation policy is added.

### Proposed boundaries (names are provisional)

```text
pipkin-ui → pipkin-core                         existing desktop intent/state
                    ↑
pipkin-app controller → NativeBackend adapter → pipkin-engine-protocol
                              │ owned child + private framed channel
                              ▼
                     pipkin-engine             no GPUI, one async runtime
                       ├─ durable store + per-session execution state
                       ├─ provider/auth adapters
                       ├─ execution environment + coding tools
                       ├─ durable subagent/task scheduler
                       ├─ native extension/tool/provider registry
                       └─ prompt/context/compaction + question host

PiBackend → pi-client → legacy Pi              continuity/reference during conversion
```

- Add `pipkin-engine` and a small shared protocol crate. Start providers, storage and tools as well-separated modules; split crates only when that improves ownership/testing, not to create a framework upfront.
- Keep `pipkin-core` independent of engine implementation, HTTP, SQLite and GPUI. Wire frames are not core commands.
- Use a single async runtime with bounded concurrency; isolate blocking SQLite/filesystem/process work from its scheduler. No network, parsing or storage on the UI render path.
- One authoritative mutation/commit sequence per session. At most one active model generation per conversation; independent conversations can progress concurrently within explicit limits.
- Keep desktop drafts/journal/preferences separate from authoritative engine history initially. No need to merge databases just to remove Node.
- Workspace Git scans and desktop launches remain app services initially; extract shared helpers without changing semantics.

### Advantages to deliver with the conversion

These are planned deliverables with acceptance tests. Rust alone does not establish any performance gain.

| Advantage | Built-in implementation | Acceptance evidence |
| --- | --- | --- |
| Smaller installation and lower background cost | Replace Pi/Node/dependency staging and per-session worker processes with one compiled engine and shared services | Same-workload package, RSS, CPU and process measurements against the frozen reference; budgets in section 7 |
| Responsive streaming as conversations grow | Typed append/update events, durable cursors, cached projections and paged hydration instead of full transcript replication | Linear work at 1/2/4/8 MiB, bounded memory, no regression in measured input/frame responsiveness |
| Better subagent inspection | Native task identities and indexed child history let the inspector page beyond its current 100-row/80-entry presentation bounds | Large child directories and long child transcripts remain fully reachable with bounded hydration; live/snapshot/finished states remain accurate |
| More reliable coordination | One durable task scheduler shares provider limits, manages child lifetimes and reconciles parent results | Crash/restart at child creation and result handoff produces one child and one parent-visible result; waiting parents cannot exhaust execution slots |
| More precise tool and Changes integration | Emit typed tool lifecycle and successful file-mutation records tied to conversation/run/call IDs | Changes no longer needs to infer native mutations from provider-shaped JSON; preserve its current session scope, Git diff and commit-retirement tests |
| Understandable recovery and diagnostics | Correlate desktop request, engine run, provider attempt and child/tool identities in one redacted diagnostic model | A fixture failure identifies admission, auth, provider, tool or storage cause without including secrets or private prompt/output text |
| Simpler releases and headless testing | App and engine share a release identity; engine runs without GUI initialization, live accounts or a JS provider stub | Clean native Cargo build and installed offline probe need no Node/Pi; protocol/schema mismatch and upgrade refusal remain tested |

Require the package/runtime simplification, equivalent-or-better measured responsiveness, paged subagent inspection, typed mutation events and unified diagnostics for the native release. Set resource-reduction budgets after remeasurement. If an aspirational budget is missed, report the measured result and revise it explicitly; do not compensate by cutting features.

Later opportunities include opt-in per-child cancellation, task budgets, richer context inspection, and qualified execution isolation. They need their own product contracts and are not prerequisites for preserving Pi behavior. No new individual cancel control should imply a guarantee the scheduler cannot enforce.

### Contract and transport

Start with a versioned, length-bounded typed message protocol over inherited/private child pipes (not a public TCP listener). Framed JSON is a reasonable initial encoding for inspectability; keep encoding replaceable and measure it before choosing CBOR or another codec. Support platform-neutral pipes so Windows engine work is not coupled to Unix sockets.

Private child pipes cover the default single-app host. P19 separately requires preserving supported explicit external attachment and multi-client workflows: design an opt-in owner-authenticated local endpoint or compatibility shim, using the same engine contract and single-writer ownership. Specify who owns shutdown when several clients attach. Do not remove today's external flags until their native equivalents or compatible translations pass; a pipe-only implementation does not close P19.

Contract operations: initialize/capabilities, catalog, create/rename/open conversation, history page, submit, lookup, steer/follow-up/withdraw, stop, set model/thinking, refresh models, auth start/reuse/answer/cancel/remove, question response/cancel, notices/status, fetch retained output, subagent directory/activity/open/page/enable and orderly shutdown. Add every audited compaction/branch/reset/resource/extension operation to the same contract. The initial contract must not silently omit less-visible baseline services.

Every run/update is routed through `(conversation, generation, op)` at the adapter boundary. Engine frames additionally identify engine epoch, durable request/run identity and committed revision/sequence. Keep UI attachment generations distinct from durable run identities: detaching a view must not cancel the run.

- Atomic initial snapshot + ordered incremental updates; gaps cause a new snapshot, not guessed state.
- Append/update specific transcript items instead of reserializing/remapping a whole conversation on every token. Preserve item IDs and revisions.
- Lossless admission, terminal settlement and question answers; bounded/coalesced progress. Use a durable ordered event/outbox sequence with acknowledged cursors and paged catch-up, plus a separate priority control lane for Stop/shutdown. Persist semantic events before publishing; when a consumer falls behind, resynchronize without dropping unacknowledged outcomes. Keep auth challenges volatile and scoped to the client, not in the durable outbox. A slow presentation cannot block cancellation or create unbounded memory growth.
- Serialize a committed immutable batch once, not once per subscriber. Bound frames, strings, arrays, attachment payloads, queue length and retained output by bytes as well as counts.
- No unsolicited replay of mutations after reconnect. Look up the existing journaled key first; prompt retry remains an explicit user decision. Safe queue reconciliation uses the same durable key.
- Preserve existing `BackendEvent` behavior first. If incremental real-mode events need new typed variants, change core/tests and both adapters deliberately; never use demo-only `Token` behavior as proof of real-mode parity.

## 4. Durability, cancellation and tools: build these first

### Durable execution model

Use SQLite with explicit schema versions, single ownership/OS locking, atomic migrations/backups, newer-schema refusal and tested corruption recovery. Begin with `WAL` + `synchronous=FULL` for acknowledged writes; benchmark honest group commit rather than relaxing durability to win a chart. Reconfirm the reference engine's SQLite pragmas during the source audit before claiming a stronger guarantee; process-crash tests are not power-loss proof.

Store conversation/configuration, immutable entries, context omissions/compaction boundaries, submission keys/status, generation checkpoints, tool intent/result and usage. Start with a small durable run/task state machine, then cover every audited task/recovery contract, including subagents and extension state. Chord implementation details may disappear only after equivalent observable behavior is proven.

1. Desktop journals input before dispatch (retain the existing ordering).
2. Engine admits a scoped unique request key, payload fingerprint and queue/run record atomically; acknowledge only after commit. Same key/same payload returns the original; same key/different payload is a conflict.
3. Commit prepared provider request identity and tool intent before issuing external work; never await network, processes or people while holding a transaction.
4. Publish only committed partial progress. Batch compact text/tool deltas with bounded checkpoint cadence; flush terminal content, usage and settlement before publishing terminal success.
5. A possibly committed storage error fails the open writer/session closed and requires reopen/reconciliation. Never manufacture success, discard the request journal, or continue from guessed in-memory state.

Do not claim universal exactly-once execution. A process can die after an external effect but before its result is committed. Provider attempts can also incur spend before their response is observed.

### Tool recovery and concurrency

- Default tools to unsafe replay; never automatically rerun an interrupted `write`, `edit` or `bash` merely because the engine restarted. The subagent overlay is a distinct replay-safe registration case and must retain its existing resume behavior and checks.
- Only replay a tool when its durable recorded policy **and** current implementation declare it safe; otherwise append an interrupted/uncertain result with retained output and make the effect boundary visible.
- Serialize mutations to the same execution-environment/path across conversations. Validate edit preconditions and conflicting/overlapping replacements before mutation; test Unicode, CRLF, permissions, symlinks, missing paths, file modes and interruption.
- Define concurrency explicitly: bound parallel reads, avoid racing dependent model tool calls, and serialize potentially conflicting writes/shell work until a proven policy permits more parallelism.
- The shell tool intentionally invokes the configured shell; internal Git/editor/engine launches use argv, never interpolate paths into a shell string.
- Drain stdout and stderr concurrently; bounded previews and retained artifacts must not let a full pipe deadlock a child. Publish retained/dropped lengths and retention limits honestly; “full output” means the retained result, not bytes the engine discarded.
- Own and reap tool children. Graceful interrupt → bounded escalation → confirm settlement. Test descendants and platform-specific process-tree handling; escaped/detached external effects cannot be promised cancelled.

### Stop and reopen

Persist Stop intent before cancellation. Cancel provider streams, pending retry/compaction and owned tool work; settle or withdraw queued input per the current semantics. Commit terminal run state and context retirement together. `Stopping` remains until engine confirmation; timeout is not success. A raced completed run may remain completed.

Keep the stopped prompt and completed tool effects in history, but exclude the retired instruction from subsequent model context. Preserve result ordering/tool-call validity so a replacement prompt or goal cannot reactivate it. Flush final transcript updates before terminal settlement so goal verdict parsing cannot race the final reply.

Specify recovery separately for admission, interrupted generation, committed tool intent, uncertain external effects and persisted Stop. Retain safe automatic recovery where established; require explicit reconciliation where effects cannot be confirmed. Any change to today's automatic-resume policy needs an owner-approved behavior decision and tests, not an accidental consequence of a new scheduler.

### Durable subagents and task ownership

Subagents are a required engine workstream, not an ordinary stateless tool wrapper. Preserve the current root conversation, owner task, parent call and child conversation relationship, along with the tool's context/model inheritance and result semantics discovered in Phase 0.

- Commit child creation and its parent-call mapping atomically under a stable durable key. Recovery looks up that child instead of creating another one. Commit parent result delivery with a uniqueness constraint so replay cannot append the result twice.
- Persist task phase, dependencies, cancellation intent, child output and terminal result. Define recovery for every phase, including a child completed before its parent observed the result, or a root stopped during child creation. Match audited parent/child cancellation semantics before adding controls.
- Bound active provider work, shell children and queued tasks across the whole engine. A parent waiting for children releases its active execution permit; fair scheduling keeps one large task tree from starving another conversation. A shared limit must not deadlock nested work.
- Preserve disabling **new** subagent calls while allowing the overlay's already-admitted replay-safe work to recover. Record tool contract, replay policy and implementation provenance; missing or incompatible registrations stop recovery with an explicit error. Stronger provenance is a proposed improvement over the overlay's documented public-contract-only limitation.
- Enforce root/task/call ownership before returning any child directory, activity, snapshot or page. Route events through both conversation generation and selected child identity. Selecting a child never changes the main conversation's composer or submit target.
- Replace retained per-child mounts with bounded subscriptions and indexed pagination while preserving live/snapshot fallback. Keep status independent of transport health, preserve finished history, and do no prompt parsing or polling in render.
- Test simultaneous children, enable/disable during work, restart, parent Stop, registration changes, stale selection, reconnect, long history and parent result consumption. Import the same task graph and enable state, not merely rendered child messages.

## 5. Provider and credential work

Use a provider trait over normalized requests, incremental content blocks, usage, terminal reasons and structured error classification. Keep provider-specific options and opaque metadata extensible so normalization does not erase a baseline capability. Share tested HTTP/SSE utilities; use maintained Rust crates where they cover the actual endpoints. Generic SDK support is not proof that subscription endpoints work.

Golden request/stream tests must cover interleaved text/thinking/tool arguments, fragmented UTF-8/JSON/SSE, tool-call IDs, reasoning signatures and redacted thinking, images, usage/cache counts, tool-result ordering, provider/model switching, aborted content exclusion, empty responses, malformed/incomplete streams, context overflow, rate limits and retry-after. Never execute an incomplete or invalid streamed tool argument. Keep opaque provider metadata for valid replay, but out of ordinary diagnostics/UI text.

Preserve prompt-cache behavior: stable provider session identity, deterministic prompt sections/tools and provider-specific cache hints. Avoid changing timestamps/prompts on every turn. Faster local code that destroys upstream caching can be slower and more expensive overall.

Generic auth infrastructure and non-reserved providers are early workstreams. Apply the following requirements to every provider; the Claude subscription-specific review and implementation belong to Codex at the final handoff:

- Validate whether existing public-client login flows/endpoints and distribution terms are usable for Pipkin. Do not assume Pi's identifiers, scopes or account permissions transfer automatically; do not invent credentials or copy private client secrets.
- Native OAuth uses provider-required PKCE/state checks, loopback/device/manual challenges, explicit cancellation and stale-attempt guards. Refresh is serialized per credential and persisted atomically; failed refresh must not silently fall back to a different billing credential.
- Store secrets outside ordinary app/engine state and journals. Prefer macOS Keychain / qualified Linux Secret Service; explicitly handle absent/locked stores, with a documented user-approved private-file fallback if needed. Do not block first-run indefinitely or silently degrade security.
- Make credential lookup metadata-only. Reuse/import is explicit consent, unsupported credentials stay untouched, and cross-process refresh/import coordination is tested.
- Default logs/diagnostics are allowlisted metadata. Keep tokens, auth headers, callback URLs/codes, prompts and private errors out of logs/crash annotations; richer debugging is explicit and reviewed.
- Test proxy/custom CA/network timeout behavior and Finder/desktop-launcher environments, not only terminal PATH/env behavior. Custom endpoints receive only their own configured credential; never forward a subscription token to an arbitrary host.

**Cutover blocker:** if either existing subscription flow or any other baseline provider/auth path is unavailable in native form, retain the working Pi path and keep its parity row blocked. Do not label API-key-only onboarding, a different billing path or partial provider coverage as feature parity.

### Boundary Opus must leave ready

Keep provider-specific behavior behind the same registration and capability interfaces used by other providers. The final Claude subscription adapter must be attachable without rewriting the scheduler, transcript, importer or sign-in UI. Stabilize and document:

- Distinct provider, credential-kind and billing-path identity. Preserve the existing Claude subscription identity and imported references without equating them with Anthropic API-key credentials.
- The generic auth lifecycle: metadata-only availability, start/challenge/answer/cancel/reuse/remove, refresh coordination, credential-store access, client/attempt isolation and structured errors. Test it with fictitious providers and non-secret local fixtures.
- Provider request/stream types, capability/model lookup, opaque replay metadata, usage/cache fields and cancellation. Reserve extension points for provider-specific requirements without inventing Claude subscription implementation details.
- Migration hooks for credential references and provider-specific state. Preserve records without activating Claude subscription conversations on an unavailable native provider; leave authoritative legacy ownership unchanged until final qualification.
- An explicit unavailable capability when the adapter is absent. Native must return a clear unsupported/not-yet-implemented result, never fake authentication, expose an actionable dead-end sign-in flow, panic through `todo!()`, silently choose an API key or retry work on Pi. Independent native conversations remain usable.

A compile-safe absent adapter and generic contract tests are acceptable at the handoff. They are not Claude subscription tests or parity evidence. Keep release qualification separate from development checks: the non-reserved suite can pass, while the parity report visibly lists this reservation and blocks beta/default/retirement. Do not hide it behind ignored tests or label the whole provider matrix green.

## 6. Existing user-data migration and rollback

Native storage should have one understandable Pipkin ownership root: engine sessions/artifacts under the app data root, non-secret config under the platform config root and credentials in the chosen secret store. `--data-dir`/test-profile selection must isolate native state; tests explicitly disable ambient credential fallback and use a disposable project. macOS/Windows path conventions are resolved by platform code, not hardcoded Linux paths.

Build the importer in parallel with execution, not after native becomes the default:

1. Discover legacy app and Pi directories without mutation. Report source format/version, extension/provider dependencies and active/unresolved work. Refuse live conversion while another writer owns the source.
2. Back up coherently (SQLite backup API or closed consistent copy including WAL state), preserve permissions and verify integrity. A raw `session.sqlite` copy while a worker is live is not a valid migration procedure.
3. Import into a new destination transactionally with a resumable migration manifest and explicit legacy→native IDs. Preserve catalog/cwd, timestamps, entries, attachment provenance, usage, selected model/effort, provider identity, resets/compactions and **context-omit edits**. Preserve desktop conversation/item references so drafts/goals/cache/journal still refer to the right conversation.
4. Translate settled requests and queue records with their durable keys; preserve parent/child task/call mappings and per-session subagent settings. Never turn a submitted-but-unconfirmed journal row into a brand-new native prompt. Quiesce and reconcile in-flight work before an explicit ownership transfer; never let import itself execute queued work. Preserve normal native queue/resume behavior after that transfer. Interrupted tools/plugin tasks need an equivalent recovery mapping; unresolved cases remain on Pi and block completion for that format.
5. Preserve unknown entry/plugin payloads byte-for-byte with version/provenance and stop conversion of that conversation until its semantics are understood. Temporary read-only preview is useful for diagnosis, but archive-only access is not parity for a previously resumable conversation. Import tests compare visible history, the next provider request, extension state and subagent continuations, not just row counts.
6. Offer explicit credential/resource import separately; no automatic copying of tokens into the transcript database and no continuous sync with Pi auth files.
7. Validate counts, ID references, usage totals, retained artifacts, request lookup, effective model context, extension/task state and reopening before atomically recording native ownership. Import is idempotent and never deletes or rewrites the legacy source. Every supported baseline format must have a lossless continuation path before conversion is complete.

Rollback changes engine selection, not just installed binaries. Retain the known-good package and legacy source snapshot; older code refuses newer schemas untouched. A conversation is writable by exactly one engine. After new native work, Pi cannot read native state: label that history native-only, offer export/archive access, and explain that rollback does not move it back or undo project changes. Do not implement bidirectional live conversion during this port.

## 7. Performance and setup scorecard

Freeze representative hardware/OS/build flags, workloads and the actual app+engine artifact first. Compare release builds on the same machine and use recorded scripted-provider streams to isolate engine overhead from network/model latency. Capture p50/p95, CPU time, allocations where actionable, peak/steady RSS, process/fd counts, bytes serialized/written and package bytes. Include real-provider observations separately.

**Provisional targets; Phase 0 validates budgets and measurement definitions. These are goals, not measured gains or justification for weakening parity:**

| Metric | Target / gate |
| --- | --- |
| Runtime dependency | Native default works without Node, npm, Pi checkout or a separately started daemon; Rust source builds require no Pi/npm provisioning |
| Topology | One desktop + one engine process, independent of conversation count; tool children counted separately; no orphaned owned children |
| Engine readiness | p95 ≤250 ms warm and ≤1 s uncached engine startup on reference hardware; measure GUI-ready separately, true cold boot separately |
| Engine memory | ≤40 MiB idle; ≤100 MiB for the defined single-active-session scripted workload, excluding tool subprocess RSS; ≥80% less than remeasured comparable Pi workload |
| Streaming overhead | Provider chunk arrival → committed backend update p95 ≤50 ms under the reference load; report UI frame/display latency separately |
| Streaming scaling | 10 MiB generated text incurs linear incremental serialization/write work plus bounded checkpoints, not quadratic full-transcript snapshots; bytes/CPU at 1/2/4/8 MiB expose regressions |
| Historical data | ≥10,000 stored items with bounded paged hydration; no full-history load per token or conversation selection; initial page p95 ≤100 ms warm |
| Stop | p95 ≤100 ms to dispatch cancellation/interrupt locally; bounded escalation tested; never turn an unconfirmed stop into success to meet latency |
| Resource stability | ≥1500 prompts after warm-up plus a long streaming/tool-output/multi-session soak; no sustained unexplained RSS/fd/task growth; record absolute values and slopes |
| Package | Zero Pi/Node/npm payload; ≥40% installed-byte and ≥25% archive-byte reduction against the newly measured equivalent release build, or an explained/reviewed miss |
| First run | Fresh installed profile reaches provider/project/model selection with no terminal or daemon setup; actual authorized first reply measured separately from provider/browser waiting |
| UI regression | Existing input/frame responsiveness does not regress under background sessions, large output or slow storage; native evidence required |

Commit cadence and progress buffers must meet both durability and stream latency budgets. If 50 ms cannot be met under durable writes, report the measured limit and tune batching/storage deliberately; do not publish uncommitted text as a shortcut. Use append buffers, prepared statements and bounded progress artifacts before speculative lock-free or custom allocator work.

Do not promise faster model inference, universal Linux portability or elimination of GPU/system-library requirements. Rust removes the JS runtime/packaging layer; signing/notarization, GPUI native libraries, shell/Git availability and provider outages remain real support responsibilities.

## 8. Delivery phases and exit gates

Estimate after Phase 0, by parity-ledger row and work package. Full provider, durable-task and extension compatibility makes the earlier 12–20-week restricted-scope estimate unsuitable as a commitment. Plan for a multi-month project; report optimistic/likely/pessimistic effort and critical-path dependencies once the engine audit and compatibility spikes are complete. The initial audit can be timeboxed to 1–2 engineering weeks, producing either a complete baseline or named unresolved areas. A useful alpha arrives earlier than complete conversion.

| Phase | Deliverables | Exit gate |
| --- | --- | --- |
| 0 — Freeze and audit | Exact engine+overlays, feature ledger, full provider/extension inventory, reference tests, fresh performance baseline, non-reserved auth/import/extension spikes, topology ADR | Claude subscription reservation recorded; no silent scope exclusions; missing source/behavior and compatibility blockers recorded; implementation estimate updated |
| 1 — Durable foundation | GPUI-free engine, typed host API/transport, owned worker, SQLite ledger, scripted provider, native adapter and fault injection | Commit/ack/lookup/restart/Stop invariants pass; no file-changing tools before recovery is proven |
| 2 — Coding slice | First provider, tool intent/results, initial coding tools, context/resources, retained outputs, usage and reopen | Scripted edit/test/diff workflow and interruption tests pass in an isolated project; opt-in alpha only |
| 3 — Main engine implementation | All non-reserved provider/auth paths, remaining tools/tasks, subagents, extensions, questions, compaction, configuration and background sessions | All non-reserved engine rows have implementation and differential evidence; Claude subscription remains explicitly open |
| 4 — Migration and desktop integration | Resumable importer, backend ownership, subagent/task state mapping, journal/context preservation, offline cache, Changes and rollback UX | Non-reserved migration/workflow evidence passes; Claude subscription state preserved with activation deferred; legacy sources unchanged |
| 5 — Advantages, packaging and handoff | Profile/tune, paged subagents, diagnostics, development no-Node packages, inventories, CI and section 13 handoff | Non-reserved checks and gains recorded; missing Claude subscription integration visible; return the implementation to Codex |
| 5B — Codex Claude subscription integration | Review feasibility; implement reserved auth/provider/migration behavior; qualify the complete provider and packaged workflow matrix | Claude subscription rows pass; re-run affected shared tests and full release checks; no remaining parity gap |
| 6 — Beta and default | Dogfooding, fault/soak runs, clean-user setup, real-provider and native acceptance | All mandatory rows pass, no data-loss/duplicate-effect/Stop/auth blocker; candidate accepted |
| 7 — Complete conversion | Retire Pi runtime/client/build paths, retain importer/fixtures/notices and known-good backups | Feature-complete native build and runtime pass without Pi or Node; no required conversation stranded on legacy |

Phases 3 and 4 can overlap once the data contract is stable. Provider/extension work and migration are critical-path work, not cleanup after the GUI works. Phase 5 profiling starts earlier, but its gains cannot replace parity gates. The known Claude subscription reservation does not block Phases 0–5; it must be resolved in Phase 5B before Phases 6–7. Revisit shared contracts if that final integration exposes a gap.

### Dual-backend rollout

- Add an explicit development selector such as `--engine pi|native` (proposed, not an existing flag). Default remains Pi until cutover approval; demo keeps **“Demo · simulated agent.”**
- Associate each conversation with its authoritative backend. Do not fallback from a failed native prompt to Pi, or vice versa: that could duplicate work.
- Parameterize the current workflow harness by backend. Pi transport-specific conformance stays in `pi-client`; portable workflow assertions run against both implementations.
- Differential tests use isolated copies of fixture projects/profiles and independently scripted provider streams. **Never shadow-execute both engines on the same real project or paid provider.** Normalize nondeterministic IDs/timestamps, not errors, usage, context differences or side effects.
- Ship native as opt-in alpha, then beta. Required workflows become non-ignored CI tests for native; a skipped/no-op test is not a pass.
- Native candidate packages contain no Node/Pi; transitional packages are labelled explicitly. Keep the previous Pi package available until every required capability and migration path is complete. A separate legacy artifact preserves continuity for blocked cases but cannot close their native parity rows.
- Remove `pi-client`, Pi adapter, staged patches and Pi/Node build scripts only after every dependent capability has a native equivalent. Preserve existing flag/config behavior with translation/deprecation where possible; any externally observable compatibility contract needs its own test. Keep the importer, golden fixtures and upstream attribution. Do not remove the legacy transport while a required external attachment/multi-client workflow still depends on it.

### Release/build changes

Replace Pi-revision/npm/model-data staging in native releases with locked Cargo inputs, an identified Pipkin engine revision/protocol/schema, versioned model metadata with provenance, target and binary hashes. Build-info tooling currently uses Node (`write-build-info.mjs`); replacing the engine alone does not remove that build dependency. Replace that tooling and native CI's JS test provider with Rust/existing build tools as part of Phase 5.

Keep reproducible-input identification, notices/source obligations, runtime ABI inventory, installer preflight, offline throwaway probe, schema refusal and version retention. Requalify macOS completed-bundle signing/extraction and Linux installed-library/ABI requirements. Windows engine pipes/process handling can be tested early, but Windows GUI support/signing remains a separate approval after Mac review. No platform is supported solely because cross-compilation succeeds.

## 9. Verification matrix and ownership

Use the Opus/Codex ownership split above and assign an accountable owner to each workstream: **engine/storage**, **providers/auth**, **tools/subagents/extensions**, **desktop adapter**, **import/recovery**, **packaging/performance**. With two engineers, combine ownership but keep separate directories and a written protocol/schema contract. The product owner reviews parity evidence and native/account acceptance; inventory findings expand the required implementation instead of silently narrowing provider or extension coverage. Avoid parallel edits to core protocol/storage migrations without an integrator.

Mandatory test categories:

- Pure state/reducer tests and backend-independent workflow tests for every baseline capability. Development CI can validate the non-reserved implementation, explicitly reporting the reserved Claude subscription rows; release qualification fails on any unresolved required row or skipped required native test.
- Admission/lookup key conflicts; crash before/after journal write, engine admission, acknowledgement, tool intent, side effect/result and Stop retirement.
- Fault injection: engine/UI SIGKILL, dropped/duplicated/delayed frames, stale generations, disconnect during Stop, competing writers, disk full/read-only, migration failure, poisoned writer, provider partial errors and expired/locked credentials.
- Boundedness/property/fuzz tests for frames, provider SSE/JSON, streamed tool arguments, history cursors, Unicode, importer records and progress backpressure; slow UI and storage must not starve Stop.
- Fixture migration from each supported legacy/native schema, effective-context comparison, usage/ID preservation, idempotency and rollback/newer-schema refusal.
- Headless engine qualification without GUI init, live network or credentials; installed bare-PATH/no-Node/no-Pi profile probe and full packaged workflow tests.
- Linux and macOS native engine lifecycle/process-tree tests; Windows-specific engine tests when that work begins.
- Guarded native desktop walkthrough: first-run, returning/imported profile, cached history, concurrent sessions, Stop/replacement goal, recovery, picker/output actions, locked/expired auth and quit/reopen. Synthetic input only through `scripts/guard.sh`; no user desktop/config/profile changes.
- Owner-authorized live provider tests separately from scripted tests; no automatic paid calls or credential import in qualification automation.

Every milestone records **measured / observed / failed / unverified**, exact app+engine revisions, fixtures/build flags, artifact identity and evidence. Existing accessibility/IME/scale/sleep/wake/cold-boot/display-presentation gates remain open until the real native procedures are performed for the changed candidate.

For every implementation batch, run:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all --check
```

Native engine, migration, package and performance checks supplement these; they do not replace them. Existing opt-in Pi tests stay available as the frozen reference during the migration.

## 10. First actionable batch

Do **Phase 0**, then implement the smallest durable vertical slice—not a broad source translation.

1. Recover the missing pinned source in a disposable reference checkout, reproduce Pi plus auth/subagent overlays and model data, and extract the existing e2e scenario inventory into a backend-independent acceptance checklist.
2. Produce source-grounded behavior ADRs for Stop/queue/recovery, durable subagents, extension compatibility, execution topology, protocol, credential ownership and migration. Audit the entire shipped provider/extension surface, then use actual usage to order work, not remove coverage.
3. Run non-reserved provider/auth, extension-host and legacy-context/task-import spikes on disposable profiles. Record Claude subscription feasibility as reserved for Codex, preserve the interface/migration requirements, and continue the main implementation. Use official provider documentation for the work being performed; leave the reserved provider-specific review to the final handoff.
4. Measure current readiness, engine resources, streaming serialization and complete package/runtime bytes; add a reproducible scripted benchmark fixture and explicit thresholds.
5. Add a GPUI-free engine + worker entry point with a scripted provider. Prove **journal → durable admission → streamed committed answer → request lookup → kill/reopen** before adding real tools.
6. Add **Stop → retirement commit → new prompt/goal** as the first behavioral regression gate; then intent/result crash tests and the four tools.

**Success means every baseline workflow and resumable session works with the built-in Rust engine, Pi and Node are absent from the native runtime/build path, and the measured integration advantages meet the agreed targets.**

## 11. Implementation work packages

All packages below are **planned / not started by this document**. Dependency order matters; source files/crates named here are proposed destinations, not claims that they exist. Each package should be a reviewable change with its own tests, evidence and rollback path. Do not open one enormous “port Pi” change.

| ID | Work package and deliverable | Depends on | Definition of done |
| --- | --- | --- | --- |
| RE-01 | Pinned-source/overlay inventory and complete parity ledger | None | All tools, providers, tasks, extensions, resources, flags and app workflows mapped to source/test; no unexamined group called complete |
| RE-02 | Frozen behavioral fixtures and performance baseline | RE-01 | Exact source/overlay/model/artifact identity; deterministic reference traces and raw measurements retained |
| RE-03A | Non-reserved provider/auth feasibility and inventory — Opus | RE-01 | Other provider/auth paths mapped and tested; Claude subscription inventory references and open reservation recorded |
| RE-03B | Claude subscription feasibility review — Codex | Main implementation handoff | Current supported integration requirements reviewed; implementation approach or specific blocker recorded |
| RE-04 | Context, durable-task and extension-state import spike | RE-01 | Settled, compacted, stopped, interrupted, queued and child/plugin fixtures preserve next-request semantics and IDs |
| RE-05 | Extension compatibility spike and ADRs | RE-02, RE-03A, RE-04 | Native extension contract and legacy compatibility strategy proven for representative hooks/custom tools/providers; topology/storage/recovery ADRs complete |
| RE-06 | Typed engine contract and private worker transport | RE-05 | API/capability/epoch/limits/sequence/snapshot tests, priority control channel and durable catch-up |
| RE-07 | Engine library, host/supervisor and headless harness | RE-06 | No GPUI dependency; owned startup/death/restart/quit cleanup passes; in-process test host uses the same API |
| RE-08 | SQLite store, migrations, lock, request ledger and outbox | RE-05–07 | Key conflicts, writer failure, backup/newer-schema refusal and crash-before/after-ack tests pass |
| RE-09 | Durable scripted generation and NativeBackend | RE-08 | Journal→admission→committed stream→settlement→lookup→kill/reopen passes through the app core |
| RE-10 | Stop, queue, steering and recovery | RE-09 | Stop/completion races, retirement, withdrawal, unknown outcomes and replacement-goal tests pass |
| RE-11 | Coding tools and execution environments | RE-08–10 | Initial four tools, then every P02 row: validation, effects, interruption, conflict policy, output retention and child cleanup |
| RE-12 | First provider and real coding slice | RE-09–11 | Golden request/stream tests and scripted multi-turn tools pass; isolated live smoke recorded separately |
| RE-13A | Generic credentials and non-reserved providers — Opus | RE-03A, RE-08, RE-12 | All non-reserved P03/P04 rows pass; generic auth interfaces tested; reserved capability is explicitly unavailable |
| RE-13B | Claude subscription integration — Codex | RE-03B, RE-13A, main implementation handoff | Reserved auth/request/refresh/reuse/removal/context/model/usage and migration behavior implemented and qualified |
| RE-14 | Context/resources, attachments, models and accounting | RE-09, RE-12 | Deterministic precedence, image/capability handling, provider-switch context and usage restoration |
| RE-15 | Durable subagent/task scheduler and recovery | RE-08–11, RE-14 | Parent/child identity, deduplicated result delivery, enable/recovery contracts and bounded shared resources pass |
| RE-16 | Native extension compatibility implementation | RE-05, RE-11–15 | Each baseline extension contract and state fixture works; no required Pi/Node bridge remains |
| RE-17 | Multi-session engine, history and child presentation | RE-09–16 | Background work, live/snapshot/reconnect, stable IDs, bounded paging and slow-consumer tests; main draft routing unchanged |
| RE-18 | Compaction, questions and remaining audited services | RE-10, RE-14–17 | Every P13/P15/P19 row mapped; effective context, hook ordering, deadlines and multi-client contracts pass |
| RE-19 | Production importer and atomic backend ownership | RE-04, RE-08, RE-13–18 | Migration interruption/rerun, IDs/journal/artifacts/context/task preservation, equivalent continuation and untouched source proven |
| RE-20 | Desktop integration including goals, offline state and Changes | RE-17–19 | All P05–P20 desktop assertions pass through NativeBackend, including scoped Changes retirement and child inspection |
| RE-21 | Full parity and fault qualification | Starts RE-09; completes RE-20 | Every required ledger row passes; no missing/skipped native coverage; differential differences explained by intentional improvements |
| RE-22 | Integration advantages and performance | RE-02, RE-17–21 | Targets measured; paged child history and diagnostics qualified; no durability tradeoff |
| RE-23 | Native build, packaging, CI, support and rollback | RE-07, RE-19–22 | No-Node build/runtime, installed workflow/probe, provenance and old/new-package migration pass |
| RE-24 | Beta and accepted default | RE-21–23 | Complete candidate passes soak/native/account acceptance; prior package/data fallback retained |
| RE-25 | Pi retirement and conversion completion | RE-24 | No required capability or supported data depends on Pi; obsolete runtime/build paths removed; importer/fixtures/notices retained |

Each provider and nontrivial extension needs a child work item under RE-13A/RE-13B or RE-16. Subagents and credential removal are already mandatory, not contingent future scope. Split work packages into reviewable changes; the dependency table is not permission to combine an entire provider or importer into one large patch.

### Dependencies during the two implementation stages

References to RE-03 or RE-13, including ranges such as RE-09–16 and RE-13–18, mean **the A work plus the documented generic interface** during Opus's main implementation. RE-03B/RE-13B do not block independent storage, tools, extensions, subagents, importer, desktop, performance or packaging development. RE-21–23 can record non-reserved qualification and development artifacts, but remain incomplete for release until the B work is qualified. Do not mark their aggregate parity claims passed at the handoff.

After RE-13B, Codex completes the affected portions of RE-19–23: provider-specific migration/continuation, onboarding and desktop integration, full parity/fault checks, performance regressions and rebuilt installed packages. Then RE-24/RE-25 may proceed through their existing acceptance gates. Final live-account evidence remains separate from generic fixture evidence. No dependency interpretation may permit default cutover or Pi removal before the reserved work passes.

### First native foundation change: intentionally narrow

After the feasibility gate, the first implementation change should contain only:

- The protocol types and hard limits, a private worker entry, supervisor and GPUI-free engine skeleton.
- A disposable store with durable `submit`/`lookup`, explicit commit acknowledgements and a scripted answer.
- Native adapter wiring behind an explicit selector, leaving the current default untouched.
- Tests for duplicate keys, changed-payload conflicts, stale events, restart/reconciliation and orderly owned-child shutdown.
- Baseline timings/RSS for that slice, without calling it full-agent performance.

Do not include OAuth UI redesign, provider breadth, legacy import activation, a new plugin system, or default-engine changes in that first change. Add Stop/context retirement immediately next, before allowing file-changing tools.

### Schema and protocol review checklist

Before RE-08 admits real work, document:

- Which tables/records are authoritative, their foreign keys and unique request-key scope, and which projections are rebuildable.
- Stable conversation/entry/run/tool/request identities; their mapping to existing desktop IDs; ownership/lock granularity and who may open each database.
- Schema version and migration order; transaction boundaries for admission, tool intent, progress, Stop and terminal settlement; acknowledgement/publication ordering.
- Recovery action for every persisted phase, including a missing tool/provider definition or unsupported imported task. No checkpoint silently becomes new work.
- Output/artifact retention quotas, deletion semantics, private file modes, backup/export scope and reclamation rules that do not erase unresolved operations.
- Maximum frame/attachment/queue sizes; sequence overflow policy; engine epoch and capability negotiation; snapshot consistency and error taxonomy.
- Model metadata source/version/provenance, unsupported-capability handling and offline catalog behavior. Mutable live catalog fetches must not become mandatory release-build inputs.

Build the schema incrementally, covering every baseline durable workflow before parity sign-off. Persist provider-specific opaque fields where required for replay, not arbitrary executable objects or a clone of Pi's entire document system.

## 12. Risk register and stop conditions

| Risk | Early signal | Mitigation / decision |
| --- | --- | --- |
| Subscription auth cannot be shipped equivalently | The required supported login/request path is unavailable | RE-03A is early work; Claude subscription RE-03B is intentionally late and remains a release gate; retain Pi if blocked, never substitute API billing silently |
| Deferred Claude subscription work needs interface rework | Final integration requires capabilities absent from generic auth/request/migration contracts | Document interfaces and assumptions at handoff; reserve integration time; re-run affected qualification instead of treating Opus completion as full parity |
| Durable semantics drift while tests appear green | Different next-model context, request keys, tool recovery or Stop behavior | Golden context assertions, shared workflow tests, crash points and retained source references; correctness blocks cutover |
| Unsafe side effects repeat on reopen | Tool intent exists without a committed result | Unsafe by default; interrupted result and reconciliation; no generic exactly-once promise |
| Legacy import loses semantic state | History looks correct but omitted instructions return, compaction changes, or pending journal keys disappear | Compare effective context and lookup identities, not just rows; block conversion until equivalent continuation is possible |
| Current feature scope grows during the port | New capabilities ship after inventory | Add parity rows and re-estimate; do not silently omit behavior newly shipped in Pipkin |
| Provider maintenance becomes the dominant cost | Endpoint/model-specific exceptions or auth regressions grow | Shared HTTP primitives, per-provider fixtures and assigned maintainers; extend delivery rather than narrow parity silently |
| Extension compatibility conflicts with a Rust-only runtime | Required Node APIs/native modules or dynamic facets cannot run equivalently | Complete RE-05 early; port/prove each contract; retain Pi and mark blocked if full compatibility is unresolved |
| Single native engine has excessive blast radius | One bad session or blocking job stalls every conversation | Bounded per-session work, isolated blocking jobs, fail-closed session state and supervisor recovery; reconsider process topology if measured isolation needs require it |
| Performance gains require weakened durability | Targets met only by skipping commits, unbounded buffering or reduced acknowledgement guarantees | Refuse the shortcut; profile batching/checkpoints and revise provisional targets with evidence |
| Native build still depends on JS tooling | No-Node runtime passes but build-info/provider fixtures still need npm/Node | RE-23 replaces those native build paths and tests a fresh native source build |
| Simpler install is overstated | Bare CLI probe passes but Finder/launcher, keychain, ABI/GPU or first login fails | Qualify actual installed workflows on supported targets; keep signing/system requirements explicit |
| Dual backend silently duplicates work | Automatic fallback/resubmit after engine failure | Persist authoritative backend and request identity; lookup/reconcile only; no shadow execution on real projects |
| Rewrite stretches indefinitely | Framework/plugin/provider work precedes a durable coding slice | Enforce RE-09/10 vertical-slice gates, slice work into milestones and re-estimate full parity rather than dropping scope |

**Stop rollout and fix the implementation** for unresolved credential leakage, changed-billing fallback, data loss, repeated unsafe effects, invalid Stop settlement or unsafe import activation. A performance miss triggers investigation/review; it is not permission to bypass a correctness gate or reduce feature scope.

## 13. Handoff, evidence and completion checklist

Keep this file as the execution contract. During implementation, maintain a companion `llm-docs/pipkin-rust-engine-progress.md` (create when work begins) with:

- Architecture decisions, complete baseline providers/features/platforms, known reference defects and explicitly identified improvements; no scope exception silently closes a parity gap.
- Work-package status: planned, in progress, blocked, implemented-but-unverified, or qualified, with accountable owner.
- Exact app/native-engine/Pi reference/overlay revisions and dirty state; package hashes, schema/protocol versions and fixture identities.
- Commands, retained evidence paths, pass/fail/skip counts, performance raw results, observed native/account checks and unresolved findings.
- Current blockers, safe next batch, owned files and any concurrent source changes that require re-inventory.

No checkbox is closed by source inspection alone when its gate requires execution. Record failed runs and diagnosed fixes rather than replacing failures with a later successful summary. Keep private account/session material out of committed evidence.

### Opus to Codex handoff package

At the end of the main implementation, create `llm-docs/pipkin-rust-engine-claude-subscription-handoff.md` and link it from the progress record. Prepare this locally for the user to bring back to this chat; do not automatically message another chat or start the final work.

The handoff must contain:

1. Exact app/engine commit, branch/worktree path, dirty diff if any, schema/protocol versions, build/artifact identity and commands to build/run the isolated native profile. Identify changes still uncommitted; do not assume another chat sees the same checkout.
2. A concise statement that the main implementation is ready for the reserved integration, with any other unfinished or unverified work separately listed. Do not hide unrelated failures behind the reservation.
3. Every Claude subscription parity row and its status, including the relevant P03/P04, context/usage, migration and onboarding assertions. Keep them reserved/unverified, never passed through API-key or generic-fixture tests.
4. Exact module/symbol references for provider registration, request/stream conversion, auth lifecycle/challenges, credential storage/refresh locking, model capabilities, cancellation, importer hooks and UI availability. Include a non-secret generic provider example and relevant ADRs.
5. Frozen Pi pin, model-data identity, existing auth-overlay/source paths and applicable baseline test references. Supply locations and known requirements without asking Opus to implement the reserved flow; include no credentials, account material, callback codes or private session data.
6. Reproducible generic/non-reserved fixtures, commands and pass/fail/skip counts; artifact paths for fault, migration, performance and installed tests. Note which checks must be rerun after the reserved adapter is added.
7. Data ownership and rollback state: legacy profiles remain intact, Claude subscription conversations have not been activated on an unavailable native provider, and there has been no silent account/billing fallback.
8. The final sequence for Codex: RE-03B feasibility review → RE-13B implementation → affected migration/desktop integration → complete provider/fault suite → rebuilt package and native/account acceptance → beta/default/retirement gates. If feasibility remains blocked, preserve Pi and report it rather than claim completion.

**Opus's stopping point:** all non-reserved implementation work and available qualification are complete, other gaps are honestly recorded, the native candidate builds with the reserved adapter absent, and this handoff is ready. Full conversion is still pending Codex's final integration and qualification.

### Default-cutover checklist

- [ ] Codex completes RE-03B/RE-13B and affected migration, desktop and package qualification; the Opus handoff alone does not satisfy this gate.
- [ ] Complete baseline feature/provider/extension/platform ledger; every required row passes without a legacy bridge or archive-only substitute.
- [ ] Native engine has no GPUI dependency; core remains engine/transport independent.
- [ ] All coding tools, queue, Stop, goals, subagent execution/inspection/recovery, extensions, Changes, attachments, models, usage, history, compaction and question workflows qualified.
- [ ] Every baseline provider/auth path qualified, including both subscription flows, saved-account reuse/removal, cancelled/retried login and expired-token refresh.
- [ ] Admission, unknown outcomes, interrupted tools and storage failures qualified without unsafe replay.
- [ ] Import preserves effective context, IDs, journal keys, artifacts, extension state and child tasks; all baseline formats resume equivalently; legacy source untouched.
- [ ] Native/legacy backend ownership is explicit; no automatic cross-backend fallback or paid shadow execution.
- [ ] Performance targets and integration advantages measured on the identified package; misses reviewed without weakening durability or calling unmeasured gains achieved.
- [ ] Native build and default installation contain no Pi/Node/npm dependency; inventories, provenance and signing policy retained.
- [ ] Actual supported installed desktop workflow, upgrade/rollback and owner-authorized first replies observed.
- [ ] Fault/soak/beta findings resolved; unresolved native accessibility/lifecycle gates explicitly accepted or left open, never invented as passes.
- [ ] Previous known-good package, compatible data backups and native-only-history rollback guidance retained.
- [ ] Identified default candidate accepted; conversion stays incomplete until no required workflow or supported data needs Pi. Retirement is then an explicit release step.

## Related records

- [Current packaging](../docs/packaging.md), [support/data ownership](../docs/support.md), [extensions](../docs/extensions.md), [platform scope](../docs/platforms.md).
- [Identified release evidence](../docs/v1-release-gates.md), [native verification procedures](../docs/native-gate-testing.md), [existing desktop-client plan](../docs/rust-desktop-client-plan.md).
- [Current onboarding implementation contract](pipkin-onboarding-plan.md) (developer planning, not installed public help).
