# Pipkin-native Rust engine: comprehensive replatforming plan

## Recommendation and status

**Build a Pipkin-owned Rust engine behind the existing backend boundary. Port the behavior Pipkin needs, not Pi's entire implementation.** Keep Pi available during development and migration, then remove Pi/Node from the default distribution once correctness, provider, migration, performance and installed-workflow gates pass.

This is a proposal, not an implemented engine or a release decision. The user requested a plan for a possible true break from Pi. The architecture and scope below are recommended decisions requiring confirmation; performance numbers are targets, not measurements. Do not reinterpret existing Pi qualification as qualification of the Rust replacement.

The product direction becomes **a native coding-agent application whose engine is Pipkin**, rather than a desktop presentation constrained by Pi's experimental services. Preserve useful Pi semantics and attribution; stop inheriting its transport, plugin framework, release cadence and credential-directory ownership as product constraints.

Do not combine this with a desktop rewrite, a universal extension runtime, or a promise to port every Pi provider. Do not put the new engine into the existing release candidate merely because it compiles.

### Decision gates: exploration is not default-cutover authorization

This plan authorizes no paid requests, credential import, package installation, publication or destructive migration by itself. Those actions require the relevant owner approval. Implementation can begin with isolated offline fixtures after Phase 0 scope approval.

| Decision | Evidence needed | Result |
| --- | --- | --- |
| Proceed with the native implementation | Both subscription paths feasible; supported legacy context can be reconstructed; measured local overhead justifies the work; explicit current-feature inventory | Commit to the retained scope and implementation backlog, or pause/re-scope with blockers recorded |
| Offer native as an opt-in alpha | Durable scripted coding slice, Stop/reopen behavior, bounded transport and an isolated profile; clear unsupported-feature labels | Native can be evaluated without changing existing conversation ownership |
| Make native the default | Required provider/workflow parity, migration and rollback evidence, performance/support targets and approved installed-native checks | Change the default only in an identified accepted candidate |
| Retire legacy Pi support | Required users migrated or explicitly accommodated; retained/exported legacy history; published compatibility and recovery policy | Remove the legacy distribution/dependencies only after an explicit sunset decision |

If the primary goal were only reducing package size, a smaller compiled/bundled Pi payload would be a lower-risk alternative worth benchmarking. The justification for the full replatform is the combination of **product independence, simpler lifecycle/support, and lower local overhead**. Do not invest in a rewrite merely because Rust is expected to be faster.

## 1. What exists today

Planning inspection: Pipkin `9714869a6a7af1c9eb5ab1b6c849fc817c92e01d`; adjacent Pi checkout and release source pin `d2a311097cbcf669e699479587332ae3988a49d0`. These identify inspected sources, not a freshly qualified package. Concurrent working-tree auth/subagent changes appeared during planning; they were not modified or fully audited here. Phase 0 must inventory the actual current source/overlays, including any newly delivered capabilities, rather than freeze this starting inventory as permanent scope.

| Area | Current implementation | Migration implication |
| --- | --- | --- |
| Desktop | Rust/GPUI in `pipkin-ui`; pure state and backend port in `pipkin-core` | Retain the app, composer, transcript, inspector and state machine |
| Backend port | `Backend::{start, request, shutdown}`, `BackendRequest`, `BackendEvent`, `LifecycleEvent` | Introduce a second adapter rather than teach views about engine internals |
| Pi adapter | `pipkin-app/src/adapters/pi/`, including session/transcript mapping, attachments, auth and workspace scans | Separate reusable desktop behavior from Pi-specific calls |
| Transport | `pi-client`: protocol 8, framed CBOR, Chord replicas/deltas, trusted local Unix sockets | Leave intact for legacy Pi; do not require the native engine to speak it |
| Engine | Pi experimental server/coordinator/session workers; workers open `pi-durable` over SQLite | This is a durable execution system, not just a simple LLM loop |
| Models/auth | Pi ModelRuntime and `pi-ai`; pinned catalog data; Pipkin's staged OAuth bridge | Provider behavior and sign-in must be audited and ported explicitly |
| App data | Drafts, attachment references, preferences, goals, submission journal and history cache in `pipkin.sqlite3` | Keep this schema/ownership stable initially |
| Engine data | Pi sessions under `<agent dir>/experimental/sessions/<id>/{meta.json,session.sqlite}`; auth/config under the agent dir | Import is a separate workstream, not a database rename |
| Packaging | Pi source + production npm dependencies + private checksum-pinned Node | Native default removes an entire build/runtime/dependency supply chain |

The deployed engine is **more than the clean Pi revision**: `scripts/build-engine.sh` applies `packaging/pi-onboarding.patch` and the `packaging/pi-onboarding/` service sources to staging, recording `pipkinAuthBridgeSha256`. Capture that bridge and model-data fingerprint alongside the Pi source pin when freezing reference behavior.

Important evidence already available:

- `crates/pipkin-app/src/adapters/pi/e2e.rs` tests real execution with a scripted provider, lost acknowledgements, queueing, Stop, crash recovery, switching sessions, attachments, compaction/history, tool output and offline cache.
- The Stop-context fix at the pinned Pi revision prevents an aborted instruction from being attempted again after a replacement prompt or goal. This must survive the port.
- `docs/v1-release-gates.md` records a historical identified Linux staging baseline of **453 MiB total / 200 MiB engine**, with an approximately **85 MiB archive**. Subsequent private-runtime changes mean this is not necessarily today's package size.
- The identified 1500-prompt automated soak recorded roughly **625–702 MiB aggregate Pi-engine RSS** and **21–52 MiB headless app-harness RSS**. That app harness is not the GPUI window. Re-measure the current package before comparing.
- Several architecture/product documents still describe the prototype. For current behavior use source, the onboarding contract, support docs and identified release evidence rather than treating those old descriptions as current inventory.

### Source walkthrough to complete before implementation

Trace one prompt end-to-end, then trace Stop, a queued follow-up, a crash during a write, OAuth refresh, and compaction. For each path record: ownership, admission transaction, checkpoints, side effects, publication, settlement and reopen behavior.

Read these source groups at the frozen reference revision:

1. **App boundary:** `pipkin-core/src/{backend,protocol,state,model,onboarding}.rs`; `pipkin-app/src/{controller,storage,cache,diagnostics,install}.rs`; `adapters/pi/{mod,engine,session,transcript,attach,workspace,auth}.rs`.
2. **Actual engine composition:** Pi `packages/coding-agent/src/experimental/{session-worker,session-catalog,session-worker-manager,server,coordinator}.ts`; `durable/{harness-setup,prompt}.ts`; `services/{agent-controller-provider,history-provider,models-provider,ui-requests-provider}.ts`; staged auth bridge sources.
3. **Durable semantics:** Pi `packages/durable/src/{session,harness,storage/sqlite,tools,env}`; especially submissions/inbox, scheduler, generation, tool intent/replay, context edits, compaction, view and usage. Read the complete durable specification and relevant conformance tests before designing storage/recovery. The older `packages/agent` loop is not the authoritative engine currently used by Pipkin.
4. **Provider semantics:** Pi `packages/ai/src/{types,models,api,providers,auth}`; credential locking in coding-agent auth storage; Anthropic and OpenAI Codex request conversion, login, refresh and stream handling.
5. **Release coupling:** `scripts/{build-engine,package,verify-install,write-build-info.mjs}`, Node/model-data staging, `packaging/{install.sh,PKGBUILD}`, macOS packaging and both qualification workflows.

Output: a feature inventory and behavior matrix with a source/test reference and a disposition (**retain / replace / import-only / defer / drop**) for every user-facing capability. This document is the starting map, not a claim that the complete engine has already been audited.

## 2. Compatibility and scope

### Required for replacing Pi as the default

- Real multi-turn coding: text/reasoning streaming, tool calls/results, correct continuation and failures.
- `read`, `write`, `edit`, `bash` with tested argument validation, cancellation, output bounds and project working directory.
- Durable conversation catalog, history paging, stable identities, partial progress, tool results and usage accounting.
- Durable prompt/queue admission and request lookup; steer, follow-up, withdrawal and session switching while other runs continue.
- Stop confirmed only after owned work settles; stopped instruction omitted from future context while visible history/effects remain.
- Correct model/thinking capabilities and per-conversation restoration; images/text attachments and explicit rejection of unsupported capabilities.
- Context construction, project `AGENTS.md`, skills and supported prompt resources; compaction and old-history access.
- Existing goals and goal-pausing/replacement semantics. Keep scheduling in the current core during the port; do not create a second goal controller inside the engine.
- Current onboarding's **Claude Pro/Max and ChatGPT Plus/Pro** subscription paths, cancelled/retried login, explicit reuse, refresh and expired-auth failures. The current bridge uses `anthropic` and `openai-codex`; generic OpenAI API-key support is not a substitute for ChatGPT subscription support.
- Offline reading/search through the existing cache, changes scanning, complete *retained* tool-result retrieval, editor/terminal actions and privacy-conscious diagnostics.
- Safe import/resumption of supported existing conversations; no silent loss of drafts, context omissions, usage or unresolved request identities.
- Existing select/confirm/input question behavior, deadlines, cancellation, notices and status via an engine-neutral question host. This does **not** promise that Pi JavaScript extensions can run in Rust.

### Provider rollout

1. Deterministic Rust scripted provider and local HTTP fixture server: no paid inference or credentials.
2. Anthropic Messages API-key path: useful development slice, not default-cutover completeness.
3. Anthropic subscription auth **and** the current OpenAI Codex subscription request/auth path. Spike both in Phase 0; do not discover incompatibility after building the whole engine.
4. OpenAI Responses API-key path and an explicitly configured OpenAI-compatible endpoint for advanced/local use.
5. Other providers only from the usage inventory and owner-approved support matrix.

Returning Pi users may already use providers outside the wizard. Inventory them before defining cutover scope; either port those required paths, retain a clearly labelled legacy backend for those conversations, or obtain an explicit scope reduction. Never silently switch provider, account, model, credential source or billing path.

### Deliberate break from Pi

| Pi surface | Recommended native-engine policy |
| --- | --- |
| Chord/CBOR service protocol, dynamic catalog/facets | Replace with a small typed Pipkin engine contract |
| Server/coordinator + process per session | Replace with one supervised native engine hosting bounded concurrent sessions |
| Arbitrary Pi JS/TS extensions and facet hot reload | Not supported natively at first; identify extension-dependent sessions and retain legacy access or block conversion with an explanation |
| Stable Pi extension API/TUI widgets | Already not bridged by Pipkin; do not add a JavaScript runtime to claim compatibility |
| Skills, `AGENTS.md`, prompt text/templates | Preserve useful text-resource formats with explicit discovery/precedence rules; importing a resource must not execute code |
| Pi provider breadth | Publish an explicit supported matrix, not “all OpenAI-compatible services” or “all Pi providers” |
| Shared `~/.pi` ownership | New Pipkin-owned data/config/secrets; explicit one-time import, no continuous dual writes |
| Pi session schema and arbitrary task/plugin state | Versioned import of supported data, not a promise of binary/schema compatibility |
| MCP, WASM plugins, subagents, remote daemon, full task framework | Do not add them merely for upstream parity. Any current/in-flight Pipkin capability (including concurrent subagent work) needs an explicit retain/defer decision in Phase 0; otherwise separate later proposals |

Use existing Pi code as a behavioral reference; record upstream origins and retain applicable MIT notices if translating/adapting implementation. No GPL Zed code is copied. A true product break does not erase attribution or dependency audits.

## 3. Target architecture

### Execution topology

**Recommended: one shipped Pipkin executable, an owned native engine child process, no Node/npm payload.** A private internal worker mode starts before GUI initialization; the engine itself is a GPUI-free library so a dedicated headless executable can be built later without redesign.

This retains crash isolation and independent engine restart without copying Pi's detached coordinator/worker tree. The desktop supervisor owns an actual child handle, private launch identity and child connection; no global server discovery or environment-based sweep of all user processes is needed in native mode. Switching conversations does not create new engine processes.

An in-process engine would avoid IPC, but ties engine failures and shutdown more closely to the UI. Benchmark the supervised-child design first; only choose embedding if measured overhead justifies giving up that boundary. A subprocess is **not a sandbox**: tools retain the user's permissions unless a separately qualified isolation policy is added.

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
                       └─ prompt/context/compaction + question host

PiBackend → pi-client → legacy Pi              transition/testing only
```

- Add `pipkin-engine` and a small shared protocol crate. Start providers, storage and tools as well-separated modules; split crates only when that improves ownership/testing, not to create a framework upfront.
- Keep `pipkin-core` independent of engine implementation, HTTP, SQLite and GPUI. Wire frames are not core commands.
- Use a single async runtime with bounded concurrency; isolate blocking SQLite/filesystem/process work from its scheduler. No network, parsing or storage on the UI render path.
- One authoritative mutation/commit sequence per session. At most one active model generation per conversation; independent conversations can progress concurrently within explicit limits.
- Keep desktop drafts/journal/preferences separate from authoritative engine history initially. No need to merge databases just to remove Node.
- Workspace Git scans and desktop launches remain app services initially; extract shared helpers without changing semantics.

### Contract and transport

Start with a versioned, length-bounded typed message protocol over inherited/private child pipes (not a public TCP listener). Framed JSON is a reasonable initial encoding for inspectability; keep encoding replaceable and measure it before choosing CBOR or another codec. Support platform-neutral pipes so Windows engine work is not coupled to Unix sockets.

Contract operations: initialize/capabilities, catalog, create/open conversation, history page, submit, lookup, steer/follow-up/withdraw, stop, set model/thinking, refresh models, auth attempts, question response/cancel, fetch retained output, orderly shutdown. Add public compaction only if required by the inventory; internal compaction is mandatory.

Every run/update is routed through `(conversation, generation, op)` at the adapter boundary. Engine frames additionally identify engine epoch, durable request/run identity and committed revision/sequence. Keep UI attachment generations distinct from durable run identities: detaching a view must not cancel the run.

- Atomic initial snapshot + ordered incremental updates; gaps cause a new snapshot, not guessed state.
- Append/update specific transcript items instead of reserializing/remapping a whole conversation on every token. Preserve item IDs and revisions.
- Lossless admission, terminal settlement and question answers; bounded/coalesced progress. A slow presentation can request a fresh paged snapshot without blocking cancellation or causing unbounded buffers.
- Serialize a committed immutable batch once, not once per subscriber. Bound frames, strings, arrays, attachment payloads, queue length and retained output by bytes as well as counts.
- No unsolicited replay of mutations after reconnect. Look up the existing journaled key first; prompt retry remains an explicit user decision. Safe queue reconciliation uses the same durable key.
- Preserve existing `BackendEvent` behavior first. If incremental real-mode events need new typed variants, change core/tests and both adapters deliberately; never use demo-only `Token` behavior as proof of real-mode parity.

## 4. Durability, cancellation and tools: build these first

### Durable execution model

Use SQLite with explicit schema versions, single ownership/OS locking, atomic migrations/backups, newer-schema refusal and tested corruption recovery. Begin with `WAL` + `synchronous=FULL` for acknowledged writes; benchmark honest group commit rather than relaxing durability to win a chart. Existing Pi SQLite uses `NORMAL`, so document any stronger guarantee precisely; process-crash tests are not power-loss proof.

Store conversation/configuration, immutable entries, context omissions/compaction boundaries, submission keys/status, generation checkpoints, tool intent/result and usage. A small durable run/task state machine is sufficient initially; do not reproduce every Chord document/task feature merely because Pi has it.

1. Desktop journals input before dispatch (retain the existing ordering).
2. Engine admits a scoped unique request key, payload fingerprint and queue/run record atomically; acknowledge only after commit. Same key/same payload returns the original; same key/different payload is a conflict.
3. Commit prepared provider request identity and tool intent before issuing external work; never await network, processes or people while holding a transaction.
4. Publish only committed partial progress. Batch compact text/tool deltas with bounded checkpoint cadence; flush terminal content, usage and settlement before publishing terminal success.
5. A possibly committed storage error fails the open writer/session closed and requires reopen/reconciliation. Never manufacture success, discard the request journal, or continue from guessed in-memory state.

Do not claim universal exactly-once execution. A process can die after an external effect but before its result is committed. Provider attempts can also incur spend before their response is observed.

### Tool recovery and concurrency

- Default tools to unsafe replay. Pi's current built-ins do not opt into safe replay; do not automatically rerun an interrupted `write`, `edit` or `bash` in the replacement.
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

## 5. Provider and credential work

Use a narrow provider trait over normalized requests, incremental content blocks, usage, terminal reasons and structured error classification. Share tested HTTP/SSE utilities; use maintained Rust crates where they cover the actual endpoints. Generic SDK support is not proof that subscription endpoints work.

Golden request/stream tests must cover interleaved text/thinking/tool arguments, fragmented UTF-8/JSON/SSE, tool-call IDs, reasoning signatures and redacted thinking, images, usage/cache counts, tool-result ordering, provider/model switching, aborted content exclusion, empty responses, malformed/incomplete streams, context overflow, rate limits and retry-after. Never execute an incomplete or invalid streamed tool argument. Keep opaque provider metadata for valid replay, but out of ordinary diagnostics/UI text.

Preserve prompt-cache behavior: stable provider session identity, deterministic prompt sections/tools and provider-specific cache hints. Avoid changing timestamps/prompts on every turn. Faster local code that destroys upstream caching can be slower and more expensive overall.

Auth is an early critical-path workstream:

- Validate whether existing public-client login flows/endpoints and distribution terms are usable for Pipkin. Do not assume Pi's identifiers, scopes or account permissions transfer automatically; do not invent credentials or copy private client secrets.
- Native OAuth uses provider-required PKCE/state checks, loopback/device/manual challenges, explicit cancellation and stale-attempt guards. Refresh is serialized per credential and persisted atomically; failed refresh must not silently fall back to a different billing credential.
- Store secrets outside ordinary app/engine state and journals. Prefer macOS Keychain / qualified Linux Secret Service; explicitly handle absent/locked stores, with a documented user-approved private-file fallback if needed. Do not block first-run indefinitely or silently degrade security.
- Make credential lookup metadata-only. Reuse/import is explicit consent, unsupported credentials stay untouched, and cross-process refresh/import coordination is tested.
- Default logs/diagnostics are allowlisted metadata. Keep tokens, auth headers, callback URLs/codes, prompts and private errors out of logs/crash annotations; richer debugging is explicit and reviewed.
- Test proxy/custom CA/network timeout behavior and Finder/desktop-launcher environments, not only terminal PATH/env behavior. Custom endpoints receive only their own configured credential; never forward a subscription token to an arbitrary host.

**Cutover blocker:** if either existing subscription flow is unavailable or unsupported in native form, keep Pi as the relevant backend or revisit scope explicitly. Do not label API-key-only onboarding as feature parity.

## 6. Existing user-data migration and rollback

Native storage should have one understandable Pipkin ownership root: engine sessions/artifacts under the app data root, non-secret config under the platform config root and credentials in the chosen secret store. `--data-dir`/test-profile selection must isolate native state; tests explicitly disable ambient credential fallback and use a disposable project. macOS/Windows path conventions are resolved by platform code, not hardcoded Linux paths.

Build the importer in parallel with execution, not after native becomes the default:

1. Discover legacy app and Pi directories without mutation. Report source format/version, extension/provider dependencies and active/unresolved work. Refuse live conversion while another writer owns the source.
2. Back up coherently (SQLite backup API or closed consistent copy including WAL state), preserve permissions and verify integrity. A raw `session.sqlite` copy while a worker is live is not a valid migration procedure.
3. Import into a new destination transactionally with a resumable migration manifest and explicit legacy→native IDs. Preserve catalog/cwd, timestamps, entries, attachment provenance, usage, selected model/effort, provider identity, resets/compactions and **context-omit edits**. Preserve desktop conversation/item references so drafts/goals/cache/journal still refer to the right conversation.
4. Translate supported settled requests and queue records with their durable keys. Never turn a submitted-but-unconfirmed journal row into a brand-new native prompt. Do not run imported queues automatically; reconcile/approve activation first. Interrupted tools/plugin tasks become blocked/uncertain or remain legacy-only, not blindly translated checkpoints.
5. Preserve unknown entry/plugin payloads as labelled opaque archival data; mark conversation archive-only if executable context cannot be reconstructed safely. Import tests compare both visible history and next provider request, not just row counts.
6. Offer explicit credential/resource import separately; no automatic copying of tokens into the transcript database and no continuous sync with Pi auth files.
7. Validate counts, ID references, usage totals, request lookup, effective model context and reopening before atomically recording native ownership. Import is idempotent and never deletes or rewrites the legacy source.

Rollback changes engine selection, not just installed binaries. Retain the known-good package and legacy source snapshot; older code refuses newer schemas untouched. A conversation is writable by exactly one engine. After new native work, Pi cannot read native state: label that history native-only, offer export/archive access, and explain that rollback does not move it back or undo project changes. Do not implement bidirectional live conversion during this port.

## 7. Performance and setup scorecard

Freeze representative hardware/OS/build flags, workloads and the actual app+engine artifact first. Compare release builds on the same machine and use recorded scripted-provider streams to isolate engine overhead from network/model latency. Capture p50/p95, CPU time, allocations where actionable, peak/steady RSS, process/fd counts, bytes serialized/written and package bytes. Include real-provider observations separately.

**Provisional targets; Phase 0 validates budgets and measurement definitions:**

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

Effort ranges below are **engineering person-weeks**, not deadlines; authentication, data formats and native acceptance can extend them. With two experienced engineers and overlapping provider/import work, budget roughly **12–20 calendar weeks to a qualified default replacement**, revising after Phase 0. A single-engineer implementation is longer. A useful alpha is earlier, but is not a safe cutover.

| Phase | Deliverables | Exit gate | Indicative effort |
| --- | --- | --- | --- |
| 0 — Audit and risk spikes | Behavior inventory, frozen Pi+bridge reference, fresh performance baseline, auth spikes for both subscriptions, sample session import/context inspection, topology ADR | Owner approves retained/dropped scope, credential policy, platform matrix and budgets; subscription feasibility demonstrated or blocker recorded | 1–2 person-weeks |
| 1 — Native execution foundation | Engine modules/protocol, owned-worker startup/shutdown, durable admission/lookup/checkpoints, scripted provider, headless harness and fault injection | Admission/ack/crash/Stop tests pass; snapshot+incremental events respect stale guards; no GPUI dependency in engine | 2–3 |
| 2 — Real coding slice | Anthropic API-key streaming, four tools, prompt construction, retained outputs, usage and native adapter; basic conversation reopen | Real scripted edit/test/diff/history workflow; unsafe tool interruption never auto-replays; first opt-in paid coding smoke with explicit authorization | 2–3 |
| 3 — Workflow and provider parity | Steers/follow-ups, background sessions, attachments, model/effort, compaction, native questions; both subscription sign-ins/refresh | Shared behavior suite passes against both engines except approved differences; owner live subscription sign-in/refresh/first reply observed | 3–5 |
| 4 — Migration and support | Tested importer, identity/journal reconciliation, credential/resource consent, ownership rules, diagnostics and rollback UX/docs | Disposable old→native upgrade, interruption, rerun import and rollback passes; no legacy data changed; unsafe conversions blocked visibly | 2–4, overlaps Phases 2–3 |
| 5 — Performance and distributions | Profile/tune, no-Node Linux/macOS packages, revised inventories/build identity/install probes and CI | Performance budgets and installed tests pass for identified candidates; Mac sealed bundle/signing policy retained; no Node runtime path used | 2–3 |
| 6 — Beta and cutover | Opt-in native dogfooding, owner review, repeated fault/soak runs, clean-user setup and candidate acceptance | No unresolved data-loss/duplicate-side-effect/Stop/auth blockers; approved native gates; previous package/data fallback retained | 2–4, includes observation time |

### Dual-backend rollout

- Add an explicit development selector such as `--engine pi|native` (proposed, not an existing flag). Default remains Pi until cutover approval; demo keeps **“Demo · simulated agent.”**
- Associate each conversation with its authoritative backend. Do not fallback from a failed native prompt to Pi, or vice versa: that could duplicate work.
- Parameterize the current workflow harness by backend. Pi transport-specific conformance stays in `pi-client`; portable workflow assertions run against both implementations.
- Differential tests use isolated copies of fixture projects/profiles and independently scripted provider streams. **Never shadow-execute both engines on the same real project or paid provider.** Normalize nondeterministic IDs/timestamps, not errors, usage, context differences or side effects.
- Ship native as opt-in alpha, then beta. Required workflows become non-ignored CI tests for native; a skipped/no-op test is not a pass.
- After cutover, default packages contain no Node/Pi. If legacy runtime compatibility remains necessary, make it a separate explicit legacy artifact/backend, not a hidden dependency in the native package. Retain the previous package while evaluating migration.
- Remove `pi-client`, Pi adapter, staged auth patch, engine/model-data/Node scripts and legacy flags only after the compatibility/import sunset decision. Keep golden fixtures/upstream attribution. Do not rename all Pi references before a replacement exists.

### Release/build changes

Replace Pi-revision/npm/model-data staging in native releases with locked Cargo inputs, an identified Pipkin engine revision/protocol/schema, versioned model metadata with provenance, target and binary hashes. Build-info tooling currently uses Node (`write-build-info.mjs`); replacing the engine alone does not remove that build dependency. Replace that tooling and native CI's JS test provider with Rust/existing build tools as part of Phase 5.

Keep reproducible-input identification, notices/source obligations, runtime ABI inventory, installer preflight, offline throwaway probe, schema refusal and version retention. Requalify macOS completed-bundle signing/extraction and Linux installed-library/ABI requirements. Windows engine pipes/process handling can be tested early, but Windows GUI support/signing remains a separate approval after Mac review. No platform is supported solely because cross-compilation succeeds.

## 9. Verification matrix and ownership

Assign one accountable owner to each workstream before starting: **engine/storage**, **providers/auth**, **desktop adapter**, **import/recovery**, **packaging/performance**. With two engineers, combine ownership but keep separate directories and a written protocol/schema contract. Owner supplies supported-provider/scope decisions and authorized native/account acceptance. Avoid parallel edits to core protocol/storage migrations without an integrator.

Mandatory test categories:

- Pure state/reducer tests and backend-independent workflow tests for every retained capability.
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

1. Freeze/reproduce the current Pi+bridge package and extract the existing e2e scenario inventory into a backend-independent acceptance checklist.
2. Produce source-grounded behavior ADRs for Stop/queue/recovery, execution topology, protocol, credential ownership and migration. Audit real provider/extension usage before declaring scope.
3. Run subscription-auth and legacy-context-import spikes on disposable profiles. Record blockers and owner decisions before committing to the replacement date.
4. Measure current readiness, engine resources, streaming serialization and complete package/runtime bytes; add a reproducible scripted benchmark fixture and explicit thresholds.
5. Add a GPUI-free engine + worker entry point with a scripted provider. Prove **journal → durable admission → streamed committed answer → request lookup → kill/reopen** before adding real tools.
6. Add **Stop → retirement commit → new prompt/goal** as the first behavioral regression gate; then intent/result crash tests and the four tools.

**Success is not “the Pi code now compiles in Rust.” It is a smaller, faster, Pipkin-owned application that installs and recovers more simply without weakening the safety and workflow semantics already earned.**

## 11. Implementation work packages

All packages below are **planned / not started by this document**. Dependency order matters; source files/crates named here are proposed destinations, not claims that they exist. Each package should be a reviewable change with its own tests, evidence and rollback path. Do not open one enormous “port Pi” change.

| ID | Work package and deliverable | Depends on | Definition of done |
| --- | --- | --- | --- |
| RE-01 | Current-feature/source inventory, including staged overlays, in-flight subagents and credential removal | None | Every capability has source/test references, disposition, accountable owner and owner-approved exception if dropped |
| RE-02 | Reproducible current baseline and standalone benchmark fixtures | RE-01 | Exact reference app/engine/bridge/artifact identity; raw data and repeated p50/p95/RSS/package measurements retained |
| RE-03 | Subscription authentication feasibility spikes | RE-01 | Both providers' login/refresh/request paths and distribution constraints recorded; authorized live verification or explicit remaining blocker |
| RE-04 | Legacy import/context feasibility spike | RE-01 | Representative settled, compacted, stopped and interrupted fixtures inspected; effective context and ID/key translation demonstrated offline |
| RE-05 | ADRs for topology, protocol, durability/recovery, secret storage and import ownership | RE-02–04 | Decisions and alternatives recorded; Phase 0 proceed/re-scope gate approved |
| RE-06 | Shared typed protocol and private child transport | RE-05 | Framing/version/capabilities/limits/epoch/snapshot sequencing tested; malformed input rejected without executing work |
| RE-07 | GPUI-free engine library, private worker entry and supervisor | RE-06 | Startup/readiness, engine death, bounded restart, orderly quit and actual child cleanup pass in disposable profiles |
| RE-08 | SQLite store, lock, migrations and request ledger | RE-05–07 | Admission/payload-key conflicts, poisoned writer, disk failure, backup/newer-schema refusal and crash-before/after-ack tests pass |
| RE-09 | Durable scripted generation and `NativeBackend` | RE-08 | Journal→admission→committed stream→terminal settlement→lookup→kill/reopen passes through the existing core |
| RE-10 | Stop, queue and interrupted-generation semantics | RE-09 | Lost Stop, completion race, context retirement, withdrawal, queued-key reconciliation and replacement-goal regressions pass |
| RE-11 | Execution environment and four coding tools | RE-08–10 | Validation, intent/result transactions, unsafe recovery, path conflicts, output retention and child cancellation pass |
| RE-12 | Anthropic request/stream adapter and coding smoke | RE-09–11 | Golden conversion/stream tests and scripted multi-turn tools pass; any live smoke is explicit and isolated |
| RE-13 | Native credentials and both subscription providers | RE-03, RE-08, RE-12 | Login/reuse/removal/cancel/refresh, secret-store failures and stale attempts tested; required owner live acceptance recorded |
| RE-14 | Prompt resources, attachments, model/effort and usage | RE-09, RE-12–13 | Deterministic prompt precedence, capability rejection, image handling, provider-switch context and usage restoration tested |
| RE-15 | Multi-session execution, paging and incremental presentation | RE-09–14 | Background work, rapid switching, gap resync, stable item IDs, bounded hydration and slow-consumer tests pass |
| RE-16 | Compaction and engine-neutral question host | RE-10, RE-14–15 | Overflow/retry/stale-summary/history behavior and question deadlines/answers/cancellation tested without requiring Pi facets |
| RE-17 | Production importer, backend ownership and recovery UI | RE-04, RE-08, RE-14–16 | Migration crash/rerun/idempotency, ID/journal/context preservation, unsupported-state blocking and no legacy mutation proven |
| RE-18 | Core/workflow parity suite and fault matrix | Begins with RE-09; completes after RE-17 | Every retained scenario runs against native; approved differences listed; required tests cannot silently skip |
| RE-19 | Profiling/tuning and repeatable performance report | RE-02, RE-15–18 | Budgets measured against identified comparable builds; scaling, long output and resource-growth misses resolved or explicitly reviewed |
| RE-20 | Native-only packaging, CI and support/upgrade documents | RE-07, RE-13, RE-17–19 | No-Node build/runtime path, packaged workflow/probe, inventories, signatures and actual old/new-package migration verified |
| RE-21 | Opt-in dogfooding, clean-user acceptance and default cutover | RE-18–20 | Candidate evidence and owner approval meet Phase 6; previous package/data fallback and native-only-history warning retained |
| RE-22 | Legacy deprecation and removal | RE-21 plus sunset approval | No hidden Pi runtime remains; agreed legacy access/import supported; obsolete tooling removed with notices/fixtures retained |

RE-13 includes credential removal/sign-out if the current concurrent auth work delivers it. RE-01 decides whether subagents require additional packages; if retained, add explicit ownership, parent/child cancellation, durable reporting, presentation and migration tests rather than treating them as ordinary tool calls.

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

Implement only the schema needed by the retained workflow. Persist provider-specific opaque fields where required for replay, not arbitrary executable objects or a clone of Pi's entire document system.

## 12. Risk register and stop conditions

| Risk | Early signal | Mitigation / decision |
| --- | --- | --- |
| Subscription auth cannot be shipped equivalently | Login works only with inappropriate identifiers/scopes, or the correct subscription request path is unavailable | RE-03 is a proceed gate; retain Pi or obtain an explicit scope change, never substitute API billing silently |
| Durable semantics drift while tests appear green | Different next-model context, request keys, tool recovery or Stop behavior | Golden context assertions, shared workflow tests, crash points and retained source references; correctness blocks cutover |
| Unsafe side effects repeat on reopen | Tool intent exists without a committed result | Unsafe by default; interrupted result and reconciliation; no generic exactly-once promise |
| Legacy import loses semantic state | History looks correct but omitted instructions return, compaction changes, or pending journal keys disappear | Compare effective context and lookup identities, not just rows; archive/block unsupported states |
| Current feature scope grows during the port | New subagents/extensions/provider flows ship after inventory | Track source changes explicitly; re-baseline scope and estimate, do not silently omit new user-facing behavior |
| Provider maintenance becomes the dominant cost | Growing endpoint/model-specific exceptions or auth regressions | Narrow supported matrix, shared HTTP primitives, golden fixtures and assigned provider-maintenance owner |
| Single native engine has excessive blast radius | One bad session or blocking job stalls every conversation | Bounded per-session work, isolated blocking jobs, fail-closed session state and supervisor recovery; reconsider process topology if measured isolation needs require it |
| Performance gains require weakened durability | Targets met only by skipping commits, unbounded buffering or reduced acknowledgement guarantees | Refuse the shortcut; profile batching/checkpoints and revise provisional targets with evidence |
| Native build still depends on JS tooling | No-Node runtime passes but build-info/provider fixtures still need npm/Node | RE-20 replaces or isolates those build paths and tests a fresh native source build |
| Simpler install is overstated | Bare CLI probe passes but Finder/launcher, keychain, ABI/GPU or first login fails | Qualify actual installed workflows on supported targets; keep signing/system requirements explicit |
| Dual backend silently duplicates work | Automatic fallback/resubmit after engine failure | Persist authoritative backend and request identity; lookup/reconcile only; no shadow execution on real projects |
| Rewrite stretches indefinitely | Framework/plugin/provider work precedes a durable coding slice | Enforce RE-09/10 vertical-slice gates, narrow scope and review at every milestone |

**Stop/re-scope immediately** for unresolved credential leakage, changed-billing fallback, data loss, repeated unsafe effects, invalid Stop settlement or unsafe import activation. A performance miss triggers investigation/review; it is not permission to bypass a correctness gate.

## 13. Handoff, evidence and completion checklist

Keep this file as the execution contract. During implementation, maintain a companion `llm-docs/pipkin-rust-engine-progress.md` (create when work begins) with:

- Approved decisions and scope exceptions, including supported providers/features/platforms.
- Work-package status: planned, in progress, blocked, implemented-but-unverified, or qualified, with accountable owner.
- Exact app/native-engine/Pi reference/overlay revisions and dirty state; package hashes, schema/protocol versions and fixture identities.
- Commands, retained evidence paths, pass/fail/skip counts, performance raw results, observed native/account checks and unresolved findings.
- Current blockers, safe next batch, owned files and any concurrent source changes that require re-inventory.

No checkbox is closed by source inspection alone when its gate requires execution. Record failed runs and diagnosed fixes rather than replacing failures with a later successful summary. Keep private account/session material out of committed evidence.

### Default-cutover checklist

- [ ] Retained feature/provider/platform scope approved; all exceptions visible to users.
- [ ] Native engine has no GPUI dependency; core remains engine/transport independent.
- [ ] Required coding, queue, Stop, goals, attachments, model/effort, usage, history, compaction and question workflows qualified.
- [ ] Both existing subscription flows qualified, including cancelled/retried login and expired-token refresh.
- [ ] Admission, unknown outcomes, interrupted tools and storage failures qualified without unsafe replay.
- [ ] Import preserves effective context, IDs, journal keys and user work; unsupported sessions fail safely; legacy source untouched.
- [ ] Native/legacy backend ownership is explicit; no automatic cross-backend fallback or paid shadow execution.
- [ ] Performance targets measured on the identified package; misses explicitly reviewed without weakening durability.
- [ ] Native build and default installation contain no Pi/Node/npm dependency; inventories, provenance and signing policy retained.
- [ ] Actual supported installed desktop workflow, upgrade/rollback and owner-authorized first replies observed.
- [ ] Fault/soak/beta findings resolved; unresolved native accessibility/lifecycle gates explicitly accepted or left open, never invented as passes.
- [ ] Previous known-good package, compatible data backups and native-only-history rollback guidance retained.
- [ ] Owner approves the identified default candidate; legacy sunset remains a separate decision.

## Related records

- [Current packaging](../docs/packaging.md), [support/data ownership](../docs/support.md), [extensions](../docs/extensions.md), [platform scope](../docs/platforms.md).
- [Identified release evidence](../docs/v1-release-gates.md), [native verification procedures](../docs/native-gate-testing.md), [existing desktop-client plan](../docs/rust-desktop-client-plan.md).
- [Current onboarding implementation contract](pipkin-onboarding-plan.md) (developer planning, not installed public help).
