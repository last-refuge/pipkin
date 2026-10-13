# Pipkin native capabilities: implementation and comparative review plan

Status: implementation in progress. ASK-1 (the first reviewable change, §13), PLAN-1 and PLAN-2 (§8), WEB-1 (§5), SUB-1 (§6), GOAL-1 and GOAL-2 (§9) and the per-tool recovery part of F0 item 4 landed on 2026-10-12; see [docs/ask-user-question.md](../docs/ask-user-question.md), [docs/plans.md](../docs/plans.md) and the progress notes under §8 and §13. Everything else remains proposed.

Prepared: 2026-10-11 (America/Phoenix). Local audit baseline: Git HEAD `67ba56f6bf525493bf9c33ad1cad8a353d558a92`, plus the working-tree changes present during inspection. External references were read on this date. Reference links to `main` are moving targets, not reproducible dependency pins.

## 1. Outcome

Make the useful capabilities behind the ten researched Pi extensions part of Pipkin's native experience. Extend Pipkin's Rust engine and GPUI interface where needed, retain the implementations that already work, and adopt demonstrably better behavior from the extensions. Installing ten TypeScript packages is not the implementation strategy.

The ten packages represent nine workstreams because the two MCP packages solve the same core need. Seven workstreams belong in the standard product: MCP, web access, subagents, structured questions, task plans, goals, and background jobs. Status information is native interface refinement. Remote observability is an optional integration.

“Best in class” means reliable execution and recovery, useful model behavior, clear user control, bounded resource use, and an excellent native interface. Popularity alone does not establish those qualities. The comparisons below distinguish inspected implementation from upstream claims and proposed Pipkin behavior. We have not installed or benchmarked the reference extensions in this audit.

### Coverage and disposition

| Original rank / package | Pipkin today | Decision | Workstream |
| --- | --- | --- | --- |
| 1. `pi-mcp-adapter` | No native MCP client/tool bridge found in the shipped engine path | Implement one native MCP service; study lazy discovery and lifecycle | MCP |
| 2. `pi-web-access` | No built-in search/fetch tools in the shipped tool set | Implement search, fetch, retained sources and citations; stage richer extraction | Web |
| 3. `pi-subagents` | Durable native child sessions and a paged native inspector exist | Extend execution/control selectively; retain durability and inspector | Subagents |
| 4. `@langfuse/pi-observability-plugin` | Usage ledger, tool history, errors and live output exist; no Langfuse exporter found | Native local diagnostics; optional asynchronous export | Observability |
| 5. `@juicesharp/rpiv-ask-user-question` | Extension-host select/confirm/input and GPUI dialogs exist; no model-visible question tool | Add a model tool and richer native questionnaire | Questions |
| 6. `pi-mcp-extension` | Same missing MCP capability as rank 1 | Use as a second reference for transports/auth/reconnection, not a second client | MCP |
| 7. `@juicesharp/rpiv-todo` | No model-visible persistent plan/task domain found | Implement a shared task-plan service used by ordinary work and goals | Plans |
| 8. `pi-goal-x` | Desktop-managed `/goal` continuation and restart-paused persistence exist | Upgrade to structured, engine-owned goals with evidence and optional audit | Goals |
| 9. `pi-background-tasks` | Foreground bash streams output; sessions can run while another conversation is selected | Add managed jobs with explicit lifecycle, results and notifications | Jobs |
| 10. `pi-powerline-footer` | Model/thinking controls, execution state and committed usage dialog exist | Refine native status, including a distinct context-occupancy view | Status |

The earlier download ranking is a discovery aid, not an install-share survey or dependency endorsement. This plan does not require preserving that ranking as the delivery order.

## 2. Evidence and implementation map

### Local sources inspected

Repository-relative links are used so this document remains useful in another checkout.

| Concern | Existing sources | What they establish |
| --- | --- | --- |
| Workspace | [Cargo.toml](../Cargo.toml), [README](../README.md) | Six crates now exist, including `pipkin-engine` and `pipkin-engine-protocol`; older three-crate prototype descriptions are historical |
| Engine tool execution | [tools/mod.rs](../crates/pipkin-engine/src/tools/mod.rs), [runtime/mod.rs](../crates/pipkin-engine/src/runtime/mod.rs), [runtime/run.rs](../crates/pipkin-engine/src/runtime/run.rs) | Shipped tools are read/write/edit/bash plus enabled root-session subagent; validation, streaming, durable intents and contract-aware replay already exist |
| Goals | [core/state.rs](../crates/pipkin-core/src/state.rs), [goal tests](../crates/pipkin-core/tests/goal.rs), [app storage](../crates/pipkin-app/src/storage.rs), [composer/goal strip](../crates/pipkin-ui/src/shell/center.rs) | `Goal { text, paused, turns }`; marker-based continuation; desktop persistence; stop/replacement and restart safeguards |
| Native subagents | [runtime/subagents.rs](../crates/pipkin-engine/src/runtime/subagents.rs), [store/schema.rs](../crates/pipkin-engine/src/store/schema.rs), [subagent inspector](../docs/subagent-inspector.md), [core/subagents.rs](../crates/pipkin-core/src/subagents.rs) | One durable child per parent call, same model/thinking/cwd, no recursive delegation, paged activity and bounded/virtualized presentation |
| Native question host | [extensions/questions.rs](../crates/pipkin-engine/src/extensions/questions.rs), [protocol/view.rs](../crates/pipkin-engine-protocol/src/view.rs), [UI overlays](../crates/pipkin-ui/src/shell/overlays.rs), [native extension contract](../docs/native-extensions.md) | Session-lifetime identities; cancellation/timeouts; three question kinds; response validation and existing GPUI rendering |
| Execution and recovery | [runtime/session.rs](../crates/pipkin-engine/src/runtime/session.rs), [runtime/lifecycle.rs](../crates/pipkin-engine/src/runtime/lifecycle.rs), [store/outbox.rs](../crates/pipkin-engine/src/store/outbox.rs), [protocol/api.rs](../crates/pipkin-engine-protocol/src/api.rs) | Durable requests, session revisions, attachment lifecycle, event catch-up and stop settlement |
| Bash | [tools/bash.rs](../crates/pipkin-engine/src/tools/bash.rs), [bash tests](../crates/pipkin-engine/src/tools/tests/bash.rs) | Streaming stdout/stderr, output spill, process-group cancellation on Unix; not a detached managed job API |
| Usage and context | [core/model.rs](../crates/pipkin-core/src/model.rs), [usage tests](../crates/pipkin-core/tests/usage.rs), [native transcript mapping](../crates/pipkin-app/src/adapters/native/transcript.rs), [context/compaction.rs](../crates/pipkin-engine/src/context/compaction.rs) | Committed model/tool usage, cache/reasoning subsets, absent-cost handling, and existing context estimation machinery |
| Credentials | [auth/store.rs](../crates/pipkin-engine/src/auth/store.rs), [auth/oauth](../crates/pipkin-engine/src/auth/oauth/mod.rs) | Private file storage and provider sign-in machinery; OS keychain backends are explicitly not yet qualified |
| Backends | [native adapter](../crates/pipkin-app/src/adapters/native/mod.rs), [Pi adapter](../crates/pipkin-app/src/adapters/pi/mod.rs), [demo adapter](../crates/pipkin-app/src/adapters/demo.rs) | Native, legacy Pi and demo paths must retain honest capability differences |

Search matches for `TodoWrite`, `WebSearch`, or `AskUserQuestion` in the Anthropic provider are wire-name compatibility entries, not implemented tools. Likewise, an OAuth MCP scope or a prompt pointing at MCP documentation does not establish an MCP client.

The current [native extension API](../docs/native-extensions.md) exposes agent control, history, questions, notices and status. It does not register model tools/providers/hooks, run classic Pi TypeScript extensions, or render terminal widgets. The first-party Rust `Tool` interface already exists; adding built-ins does not require a universal plugin platform.

### Baseline qualifications

- The working tree already contains unrelated changes to protocol, runtime, storage, native extensions and durable prompt waits. Treat the inspected wait API as working-tree evidence, not a separately qualified release prerequisite. Rebase/re-audit before implementing these workstreams.
- [docs/architecture.md](../docs/architecture.md), [docs/PRODUCT.md](../docs/PRODUCT.md) and the prototype plan contain older descriptions. Prefer current code, [native extension documentation](../docs/native-extensions.md), [subagent documentation](../docs/subagent-inspector.md) and the dated [scorecard](../docs/scorecard.md) for current claims.
- Source and tests inspected here establish design and coverage, not successful native interaction on every platform. New UI must receive its own native verification.
- No current MCP, web, plan, job or remote-export implementation was found through the shipped-tool and crate-source audit. Recheck `builtin_tools`, `tools_for`, protocol capabilities and current branches before creating a new subsystem.

## 3. Shared architecture and decisions

### Ownership

| Layer | Responsibility |
| --- | --- |
| `pipkin-engine` | Tool execution, external connections, authoritative plans/goals/jobs/questions, scheduling, policy enforcement, durable state and usage |
| `pipkin-engine-protocol` | Versioned commands, snapshots, deltas, capabilities, bounds and typed errors |
| `pipkin-app` adapters/controller | Translate engine views and commands, maintain attachment/reconnect behavior, run platform integrations and desktop persistence |
| `pipkin-core` | GPUI-free presentation state, availability and stale-event rejection; mirror authoritative engine state |
| `pipkin-ui` | Native controls, previews, task directories, focus/accessibility and virtualization |

The engine owns execution even when there is no mounted UI row. GPUI rendering performs no I/O, parsing, process control or scheduling. Existing `(conversation, generation, op)` routing remains mandatory; add task/question/goal identity and revision where an operation outlives a turn. Background events must not be incorrectly dropped merely because the foreground operation changed.

### Foundation F0: extend the engine deliberately

1. Re-audit the current branch and capture exact local/upstream revisions, dependency licenses and reference versions. Preserve unrelated working-tree changes. Do not modify/build `zed/`.
2. Extend `ToolHost` with narrowly scoped operations for questions/plans/goals/jobs, or extract a session-services host if that keeps responsibilities clearer. Reuse `Tool`, `ToolCtx`, schema validation, output bounds, cancellation and durable tool intents.
3. Introduce a per-session effective tool catalog: built-ins, permitted MCP tools, and profile restrictions. Pin tool name/schema/implementation identity for a call. Reject duplicate names, unsupported schemas and policy violations. Keep tool ordering stable between requests.
4. Revise replay dispatch carefully. Today `replay_allowed` recognizes only `subagent`, and `one_tool` has a subagent-specific recovery fallback. Setting `replay_safe = true` on a new tool alone is insufficient. Use explicit per-tool recovery handlers and recorded compatible contracts. Never recover an unrelated tool using `subagent_tool()`.
5. Make local mutations transactionally idempotent by `(session, call_id)` or an explicit client request key. Persist the domain change, resulting revision, receipt/result and event outbox together. On replay return the recorded result. For remote side effects, an interrupted call remains uncertain unless the remote protocol supplies a usable idempotency/result-lookup contract.
6. Extend engine snapshots/deltas and advertise capabilities such as `questionnaires`, `plans`, `goals`, `backgroundJobs`, `mcp`, `web`, `contextUsage`. These names are proposals, not existing API. Follow the current protocol's strict version policy rather than hard-coding the version observed during this audit.
7. Add schema migrations with backups and refusal of newer unsupported stores. Keep demo state separate. Use the existing commit/publication/outbox path; avoid an unrelated second event bus or silent write failures.
8. Factor reusable configuration/credential references, output artifacts and notification delivery only where two workstreams actually need them. Start with small modules in existing crates; split crates only for a concrete dependency boundary.

### State domains remain distinct

- **Goal:** user-authorized outcome, constraints, completion requirements and continuation policy.
- **Plan task:** a step toward an outcome, optionally linked to a goal; does not itself execute code.
- **Subagent run:** a model-driven child session with its own transcript, tools and usage.
- **Process job:** a managed operating-system command with logs and an exit status.
- **Question:** a request for a human decision, with an explicit answer/cancel lifecycle.

Use references between these domains. Do not make a todo row simultaneously represent a process, child conversation and durable goal, or infer successful goal completion from every row being checked.

### Scope and defaults

Ship first-party native features first. Keep existing TypeScript compatibility limitations explicit; importing a config is not executing an extension. The legacy Pi adapter advertises only capabilities its actual service contract provides. Unsupported controls are hidden or explained, never silently emulated by scraping chat text. Demo fixtures cover each feature and remain marked **Demo · simulated agent**.

New requests should use existing effective project/session permissions. A read-only reviewer must have enforced restrictions, not merely a prompt. This plan does not promise an OS sandbox where none exists. New external credentials, tool outputs and fetched content stay out of telemetry unless configured for export.

## 4. MCP: combine the two references into one native service

### Comparative decision

[pi-mcp-adapter](https://github.com/nicobailon/pi-mcp-adapter) emphasizes cached discovery, lazy server startup and compact model-facing access. [pi-mcp-extension](https://github.com/irahardianto/pi-mcp-extension) documents transport support, paginated discovery, refresh notifications, stable names, cancellation, reconnect and OAuth. Adopt these complementary ideas. Neither package should become a second competing MCP owner in Pipkin.

The adapter now also compares itself with newer Pi built-in MCP. That does not prove Pipkin's pinned/native engine has that support. Do not import upstream performance percentages as Pipkin targets without reproducing the workload.

### Implementation

**Decisions (2026-10-12, owner-confirmed).** Build MCP-1 on the official `rmcp` SDK pinned to an exact release (`=3.5.1`, Apache-2.0, MSRV 1.88, tokio; client features only), qualify stdio first and Streamable HTTP second, skip legacy HTTP+SSE unless a server in actual use needs it (it is a stated non-goal of the SDK), expose the compact `mcp_search` / `mcp_describe` / `mcp_call` surface (pinned direct tools are a later option), configure servers under `mcp.servers` in settings with a previewed import of the common `mcpServers` shape and no auto-launch of repository-discovered executables, and keep credentials as environment-variable references plus the engine's private file store, with OAuth deferred to MCP-2. Build spike on this date: adding `rmcp 3.5.1` with `client`, `transport-child-process`, `transport-streamable-http-client-reqwest` and `client-side-sse` compiles in this workspace in about 11 s and adds `nix`, `process-wrap`, `sse-stream` and `tokio-stream`; its reqwest transport depends on reqwest 0.13.5, which would sit beside the vendored patched reqwest 0.12.28 and bypass the engine's proxy/CA/idle-timeout policy for MCP HTTP traffic. Resolution: use the stdio transport as shipped, and for HTTP implement the SDK's client trait over the engine's own reqwest 0.12 client (feature without the reqwest implementation) so one network policy applies; accept a second reqwest only if that trait is not usable, and record the licenses of the new crates in the bundled-licenses audit before shipping.

**MCP-1 — connection and tool execution.** Add `engine/mcp/` with configuration, server identity, transport supervision, catalog and invocation modules. Evaluate the [official Rust SDK](https://github.com/modelcontextprotocol/rust-sdk); pin a tested release and negotiated protocol version. Qualify stdio and Streamable HTTP first. Legacy SSE is a compatibility slice only if needed by real servers.

- Configuration supports user defaults and explicit project overrides, with an inspectable effective configuration and source location. Scope credentials separately. Import selected standard/Pi configs through previewed, validated conversion; do not auto-launch every executable discovered in a repository.
- Server identity includes scope and configuration revision. States distinguish stopped, connecting, ready, authentication required, backoff and failed.
- Cache tool metadata, page through discovery, process catalog-change notifications and invalidate on config/schema changes. Do not send every installed tool schema to every model request.
- Provide compact `tool_search`/describe/invoke access first, or an equivalent stable MCP proxy. Retain the resolved server/tool identity in the durable call record so policy, usage, transcript and traces see the actual operation. Allow explicitly selected direct tools later.
- Resolve and validate remote tool arguments against their captured schema. Retain structured results and supported content blocks; bound previews and provide artifact references for large content. Preserve remote tool errors separately from transport failures.
- Start servers on demand; stop idle stateless servers. Support a keep-alive policy for stateful servers. Define whether a process is per session or shared, and reference-count its callers. Never kill an active call during idle cleanup.
- Forward cancellation and bound timeouts/retries. A transport reconnect may restore discovery; it must not blindly replay a mutation whose reply was lost. Treat MCP tool annotations as hints, not proof of read-only behavior or idempotency.

**Progress (2026-10-12).** MCP-1 done for stdio: `mcp/` (configuration with `$NAME`/`secret:key` references and per-server revisions; the service with lazy `TokioChildProcess` starts, start backoff, cached and persisted catalogs invalidated by `tools/list_changed`, idle reaper with keep-alive and in-flight protection, bounded calls with cancellation), `tools/mcp.rs` (`mcp_search`, `mcp_describe`, `mcp_call`; `details.mcp` v1 with the resolved server/tool/revision; `mcp_call` never replayed), `runtime/mcp.rs` (per-session merged settings with project scope, the `McpServers` call, protocol 19, capability `mcp`), the fixture server `examples/mcp_fixture.rs` and `runtime/mcp_tests.rs`. See [docs/mcp.md](../docs/mcp.md). Not done: Streamable HTTP over the engine's reqwest (SDK trait), previewed `mcpServers` import, image/binary content retention as artifacts, per-session process scoping, and MCP-2/MCP-3 below.

**MCP-2 — authentication and management.** Add a native Connections settings page: add/test/edit/remove server, authentication flow, tool enablement, diagnostics and reconnect. Reuse proven OAuth primitives, but maintain MCP-specific audience/resource/token scoping. Prefer OS credential storage after its platform backend is qualified; until then document the existing private-file fallback accurately. Never expose secret values in snapshots or tool arguments. Revoke/clear credentials explicitly.

**Progress (2026-10-12).** MCP-2 done: Streamable HTTP through the SDK's client trait over the engine's reqwest (`mcp/http.rs`; no second reqwest, one network policy), OAuth on the engine's primitives (`mcp/oauth.rs`: RFC 9728/8414 discovery, RFC 7591 registration, PKCE, RFC 8707 `resource`, loopback callback with manual paste, refresh, per-server tokens in the private `mcp-auth.json`), server states `authRequired`, management calls (upsert/remove/disabled tools/test/reconnect/auth start-answer-cancel-revoke/import preview-apply; protocol 20, capability `mcpManagement`, `McpChanged` event), previewed `mcpServers` import (`mcp/import.rs`), the core `McpState` with commands and the GPUI "MCP servers" settings section (`pipkin-ui/src/shell/mcp_settings.rs`). See [docs/mcp.md](../docs/mcp.md). Not done: OS credential storage (the private file is documented as the fallback), client-credential/enterprise grants, legacy SSE, native verification of the page, and MCP-3 below.

**MCP-3 — richer protocol features.** Add resources, prompt templates and elicitation after tools work. Map bounded forms onto native questions. Advertise unsupported capabilities honestly. Sampling, task extensions and interactive MCP apps are separate opt-ins. Remote HTML does not run in GPUI; embedded browser feasibility follows the existing [Browse plan](right-pane-browse-plan.md).

**Progress (2026-10-12).** MCP-3 done for tools-adjacent features: resources and resource templates (`mcp_resources`, `mcp_read` with bounded slices and binary-by-size), prompt templates (`mcp_prompts`, `mcp_prompt` with required-argument checks), declared server capabilities recorded and shown with `unsupported` refusals, list-changed invalidation for both, and form elicitation mapped onto the session's native questions field by field (typed answers, re-ask on bad numbers, decline/cancel/absent handled; URL elicitation declined; no sampling or roots advertised); protocol 21, capabilities `mcpResources`, `mcpPrompts`, `mcpElicitation`. Not done: resource subscriptions and completions, sampling, tasks, MCP apps / remote HTML (the Browse plan), native verification.

### Acceptance

- Scripted stdio and HTTP servers exercise pagination, duplicate names, tool changes, oversized output, malformed schema, cancellation, auth expiry, reconnect and clean shutdown.
- A side-effect counter server proves reply loss and restart do not execute a mutation twice.
- A many-server fixture measures idle process count, memory, discovery latency and model schema size. Unused servers must not stay running without a declared reason.
- Native UI shows actual origin/tool, connection failure and sign-in state; keyboard operation and project switching cannot route a call to another server scope.

## 5. Web access: useful research with recoverable sources

### Comparative decision

[pi-web-access](https://github.com/nicobailon/pi-web-access) provides configurable search/extraction, stored content retrieval and bounded paging/search within fetched sources. Adopt provider independence and source recovery. Stage PDF/video and specialized extraction separately. A model subscription or an available HTTP client does not by itself provide a supported search entitlement.

### Implementation

**WEB-1 — search and fetch.** Add `engine/web/` and built-in `web_search`, `web_fetch`, `web_read` tools (proposed names). Define a search-provider trait, start with one documented API-key provider and an optional configured self-hosted provider, and make provider selection visible. Choose/pin the actual provider during implementation using current official documentation. Do not depend on undisclosed free endpoints or silently reuse unrelated OAuth credentials.

- Normalize results into source id, requested/final URL, title, excerpt, provider, retrieved-at timestamp, and publication date when actually supplied. Preserve empty results as success; classify authentication/rate-limit/network/invalid-response errors separately.
- Direct fetch supports HTTP(S), redirects, timeouts, content-type checks and bounded decompression. Extract readable HTML and plain text off the UI thread. Preserve the original fetched material or a retrievable artifact alongside any derived representation.
- Store sources under engine-owned private artifact storage with session/project provenance, content hash, byte count, retention policy and access scope. Return bounded excerpts plus stable read/find cursors. An expired artifact produces an explicit expired result; refetching creates a new version.
- Format citations from verified source records. Clicking one opens the source externally initially. A persisted citation must remain meaningful after a transient cache expires: retain URL/title/date and any evidence excerpt attached to a goal/task.
- Support bounded batches, cancellation and a configured fallback chain. Do not send the same query/page to additional providers without a configured routing policy. Label summaries as derived, not verbatim source content.

**Progress (2026-10-12).** WEB-1 done: `web/` (policy, extraction, Brave and SearXNG providers, service), `store/web.rs` (schema 12 `web_sources` with expiry and per-call idempotency), `tools/web.rs` (`web_search`, `web_fetch`, `web_read`), `runtime/web.rs`, the `WebSources` call (protocol 14) and capability `web`. See [docs/web-access.md](../docs/web-access.md). Not done: desktop citation rendering (click to open), batch fetches and a fallback provider chain, live-provider verification, and WEB-2 below.

**WEB-2 — extraction breadth.** Add local text extraction for PDFs only after packaging and hostile/oversized-document limits are qualified. Add OCR, authenticated pages, screenshots/video and browser automation as separate capabilities with explicit cost/data routing. Keep source retrieval independent from the Browse/webview milestone.

**Network boundary.** Validate schemes and every redirect; reject unintended local/private network targets and credential-bearing cross-origin redirects. Allow a specifically configured local search service without granting arbitrary fetched URLs access to the local network. Remote extractors need their own egress policy. Content and search snippets are untrusted evidence, never instructions that can modify engine configuration.

### Acceptance

- Fixtures cover a normal page, redirect chain, charset errors, compressed oversized response, malicious HTML, duplicate results, rate limits and missing credentials.
- Tool returns remain bounded while `web_read` can recover all permitted retained content. Cancellation stops network/extraction work.
- Citation tests distinguish original and redirected URLs, preserved excerpts and expired material. Published dates are never invented.
- A fixed research task produces inspectable source links and survives compaction/reopen. Fetching cannot access private endpoints except explicitly configured ones.

## 6. Subagents: extend a strong durable base

### Current implementation and comparison

The native [subagent runtime](../crates/pipkin-engine/src/runtime/subagents.rs) atomically creates/finds a child for `(parent session, call id)`, uses an idempotent child prompt key and ties parent cancellation to the child. The tool currently accepts a task string, inherits parent model/thinking/cwd, receives fresh task context and prevents recursive delegation. The [inspector](../docs/subagent-inspector.md) already offers active/history separation, virtualized directories, paged native activity and honest connection states.

[pi-subagents](https://github.com/nicobailon/pi-subagents) adds named roles, configurable model/context, foreground/background work and orchestration. Its [configuration](https://github.com/nicobailon/pi-subagents/blob/main/docs/configuration.md) also exposes extensive budgets, controls and workflows. Adopt the useful small subset; a large tool schema and scripting platform are not necessary for the next release.

### Implementation

**SUB-1 — profiles and execution policy.** Add versioned agent profiles with role/name, instructions, optional explicit model/thinking override, allowed tools and context mode. Default remains inherited model with a fresh explicit task. Start with Worker and Reviewer/Scout. Enforce their tool restrictions in `tools_for` and invocation, including MCP and shell access. Do not call a profile read-only while it can run arbitrary bash or unrestricted network tools.

- Preserve exact child identity and request-key recovery. Persist profile/model/policy revision at launch so edited settings cannot silently change a recovering child.
- Reuse the existing model/auth catalog and show unavailable routes as errors. Never silently substitute a cheaper or different model.
- Add bounded context inheritance as an explicit option. Record the source revision and any omitted content. Preserve user requirements; children must not inherit ownership of the parent's goal or permission to spawn another scheduler.
- Add concurrency and total-child caps, result artifact references and per-child usage. A small structured parallel group is enough initially; delay general script workflows and recursive spawning.

**Progress (2026-10-12).** SUB-1 done: `profiles.rs` (built-in worker/reviewer/scout, settings-defined profiles with validation, launch records, bounded context excerpts), schema 13 (`subagent_tasks.launch`), the extended `subagent` tool (`profile`, `context`), model/thinking validation without substitution, total and concurrent caps checked in the creation transaction, enforcement in the child's tool catalog and a `profile` system section, protocol 15 (`SubagentRow.profile`) and the inspector row label. See [docs/subagent-profiles.md](../docs/subagent-profiles.md). Not done: structured parallel groups, result artifact references, SUB-2 and SUB-3 below.

**SUB-2 — individual controls.** Add engine-authoritative Stop child and, separately, Steer child. The current inspector explicitly has no individual cancellation contract. New controls require typed child authorization, durable request ids and settlement; do not reach into child state from the UI. Report how a stopped/failed child affects its waiting parent.

**SUB-3 — asynchronous children.** Add a launch receipt, result/status lookup and durable completion delivery using the same notification/job infrastructure as process jobs. A child session remains a child session, not a shell command running a second Pipkin binary. User-origin launches do not automatically authorize a model wake; model-origin launches use an explicit delivery policy.

**Progress (2026-10-12).** SUB-2 and SUB-3 done: schema 18 (`subagent_tasks.origin`, `.delivery`, `.note`), engine-authoritative `StopSubagent` (note first, then the child's own Stop; settlement through the child's run; the waiting parent told "stopped by the person"), `SteerSubagent` (durable `steer:<request>` admission), `LaunchSubagent` (user-origin, notification-only, idempotent per request), `SubagentResult`; `subagent` gains `wait`/`wake`, with a detached watcher per background child, exactly-once delivery under `subagent:<child>:done` on the jobs delivery rules (suppressed when the launching run was stopped; user-origin never wakes), restart re-attachment; tools `subagent_status`, `subagent_result`, `subagent_wait`; protocol 24 and capability `subagentControls`; core commands/requests/events with selection-bound authorization; the inspector's Stop, Steer and background-launch controls. See [docs/subagent-controls.md](../docs/subagent-controls.md). Not done: parallel groups, result artifact references, native observation.

### Acceptance

- Existing restart, parent-stop and paging regressions continue to pass.
- Parallel calls cannot duplicate children after crash/replay; profile changes do not alter admitted work.
- Child controls target only the selected authorized child; stale selection cannot stop a sibling. Parent cancellation settles all owned children according to policy.
- Read-only profiles actually reject write/edit/bash and mutating MCP calls; context inheritance cannot transfer the parent's goal runner.
- Partial failures preserve completed results. Large outputs remain retrievable. Inspector performance stays bounded for thousands of rows/entries.

## 7. Structured questions: finish the model-to-human path

### Current implementation and comparison

Pipkin already renders extension questions and returns validated select/confirm/input answers. The native host uses session-lifetime ids, first-answer-wins semantics, cancellation and timeouts. This is a useful foundation, but a model cannot currently call an `ask_user_question` tool in the shipped engine.

The [rpiv questionnaire](https://github.com/juicesharp/rpiv-mono/tree/main/packages/rpiv-ask-user-question) supports grouped questions, choices, custom text, multiple selections, notes and previews. Its [schema](https://github.com/juicesharp/rpiv-mono/blob/main/packages/rpiv-ask-user-question/docs/tool-schema.md) distinguishes partial answers and cancellation. Native Pipkin should preserve those distinctions while retaining its own dismiss-versus-decline behavior.

### Implementation

**ASK-1 — direct tool.** Add `ask_user_question` to the first-party tool catalog when an eligible presentation is attached. Route through a session host, not a second UI channel. Begin with single-choice plus custom text, optional descriptions and explicit submit/cancel. Return typed answers to the model and persist the actual submitted answer in history.

**ASK-2 — questionnaire contract.** Extend the protocol with a versioned questionnaire: stable question/option ids, 1–4 questions, single/multiple/free-text kinds, optional bounded Markdown previews, per-answer notes and a global note. Validate all bounds in the engine. Option labels are presentation text, not identifiers.

- A suggested/default option is never an answer until the user submits. A note does not imply acceptance of every unanswered question.
- Preserve drafts when collapsing or switching tabs; keep transcript reading possible. Escape may dismiss the presentation while the request remains pending, matching Pipkin's existing behavior. Explicit Decline/Cancel settles it as unanswered.
- Distinguish waiting for the user from active model generation. Background-conversation questions produce one attention badge and optional notification without stealing focus.
- Decide question durability explicitly: existing extension requests are runtime-lifetime scoped. For model tool questions, persist the tool request and submitted answer. On reconnect rehydrate the same live request; after engine restart reissue only an unsettled question through a documented recovery path, with new runtime ids and no duplicate answered prompt.
- Gate on actual UI capability, including legacy clients. A session with no answering surface must not hang forever: return a typed unavailable outcome or suspend with a visible deadline according to mode.

### Acceptance

- End-to-end scripted model calls the tool, the user supplies an answer, and the next request receives exactly that typed answer.
- Reject duplicate ids, invalid selections, excessive text and stale answers; two windows racing to answer produce one accepted result.
- Test partial submission, multi-select, free text, notes, timeout, collapse/reopen, disconnect, restart and cancellation while answering.
- Native verification covers multiline IME, keyboard-only completion, focus restoration, narrow layouts and real screen-reader interaction. Existing select/input support is not proof that the new form passes those gates.

## 8. Persistent plans/todos: one service for ordinary work and goals

### Comparative decision

No structured plan domain exists in the inspected shipped tool/protocol path. A Markdown checklist in a message, queued prompts and subagent records are different things.

[rpiv-todo](https://github.com/juicesharp/rpiv-mono/tree/main/packages/rpiv-todo) demonstrates session-scoped persistence and dependency validation. Its [tool contract](https://github.com/juicesharp/rpiv-mono/blob/main/packages/rpiv-todo/docs/tool-schema.md) records full post-mutation snapshots in tool results. Pipkin should use its existing transactional engine store and deltas, keeping compact model context rather than appending an entire growing task list on every mutation.

### Implementation

**PLAN-1 — domain and storage.** Add engine-owned plan/task tables and typed snapshots. A plan belongs to one root conversation, optionally a goal; children have separate plans unless deliberately assigned a scoped task reference.

Proposed record: task id, plan id, optional parent task, title, detail, status, dependency ids, owner/run references, completion criteria, evidence references, revision and timestamps. Statuses: pending, active, blocked, completed, skipped and cancelled. A reason is required for skipped/cancelled work. Archival is separate from status. Use stable ids that are not reused after clear.

- Add compact model operations for read/create/update/reorder/archive. Use optimistic revisions and idempotent mutation receipts.
- Validate references and cycles before commit. Define dependency completion rules, including explicitly waived skipped work. A rejected batch is atomic and changes nothing.
- Allow reopening completed tasks with an explicit reason; preserve the previous completion/evidence revision. This is useful when tests fail or requirements change.
- Allow several active tasks when actual parallel runs exist; do not enforce a single-active-task convention that contradicts our subagent model.
- Persist task history and current state independently of transcript compaction. Inject a concise active-plan summary with a retrieval tool for details. Fork/import semantics must be explicit: copy a snapshot with provenance, not shared mutable ownership.

**Progress (2026-10-12).** PLAN-1 done: `plans.rs` (pure domain), `store/plans.rs` (schema 11: `plans`, `plan_tasks`, `plan_events`, `plan_receipts`), `tools/plan.rs` (`plan_update`, `plan_read`), `runtime/plans.rs` (service, `UpdatePlan`/`Plan` calls, `PlanChanged`, `plan` snapshot field, `plan` system section), protocol 13, capability `plans`. Idempotent receipts keyed `call:<id>` / `client:<key>` satisfy F0 item 5 for this domain. See [docs/plans.md](../docs/plans.md). Not done: goal linkage, scoped task references to children, a crash fixture at the batch/result boundary, and PLAN-2 below.

**Progress (2026-10-12).** PLAN-2 done: core `PlanState` projection with `PlanSynced`/`PlanEditApplied`/`PlanEditFailed` events and `TogglePlanTask`/`TogglePlanHistory`/`EditPlanTask` commands; native adapter mapping and `UpdatePlan` round trip; demo coverage in the subagents scenario; `shell/plan.rs` (virtualized Plan inspector tab with expandable task detail and status edits), a composer-side plan strip, the `ShowPlan` action and palette entry. Native verification of the view is still open (see `docs/scorecard.md`).

**PLAN-2 — UI.** Show a collapsible progress strip and an inspector plan view. Completed tasks move into history but remain inspectable. Expand one task to see criteria/evidence and linked agent/job. The model receives user edits through the same revisioned engine service. Task-count progress is labelled as task counts, not a prediction of time or overall goal completion.

### Acceptance

- Invalid/self/cyclic/cross-plan dependencies cause no mutation; concurrent edits produce a clear conflict, not lost updates.
- Crash between mutation and reply returns the same receipt after recovery. Compaction/restart preserve tasks, dependencies and evidence.
- Reopening or revising a task invalidates affected completion claims. A child cannot overwrite an unrelated parent plan.
- Large plans remain paged/virtualized; collapsed views show active work, completed counts and hidden-item counts honestly.

## 9. Goals: retain the safeguards, replace the weak completion contract

### Current implementation

`SetGoal`/`ClearGoal`, `advance_goal`, `start_goal_if_idle` and `restore_goal` live in [core/state.rs](../crates/pipkin-core/src/state.rs). Desktop [storage](../crates/pipkin-app/src/storage.rs) saves objective text and pause reason. The scheduler submits a prompt with one of four final-line markers: continue, met, blocked or unreachable. `turns` is an in-process counter, not a persisted spending budget. A “met” marker sets the pause reason to `Goal met.`; there is no typed completed goal/evidence record.

Retain these verified design properties: journaling before submission, no continuation on ambiguous replies, no blind resend after an unknown outcome, restart-paused restoration, queue priority, and stop/replacement race handling. The [goal tests](../crates/pipkin-core/tests/goal.rs) and native/Pi adapter end-to-end tests are starting regression cases, not disposable prototype code.

### Comparison with `pi-goal-x`

The [package documentation](https://pi.dev/packages/pi-goal-x) describes persistent objectives/tasks, flexible or ordered execution, revision controls and an optional completion auditor. Source inspection of [core tools](https://github.com/tmonk/pi-goal-x/blob/main/extensions/goal-core-tools.ts), [completion](https://github.com/tmonk/pi-goal-x/blob/main/extensions/goal-completion.ts), [state](https://github.com/tmonk/pi-goal-x/blob/main/extensions/goal-state.ts) and [auditor](https://github.com/tmonk/pi-goal-x/blob/main/extensions/goal-auditor.ts) supports the more specific distinctions below.

| Dimension | Pipkin now | Target decision |
| --- | --- | --- |
| Lifecycle | Text plus optional pause reason | Explicit draft/active/waiting/paused/blocked/verifying/completed/cancelled/failed states |
| Completion | Executor-authored final marker | Typed completion proposal with criterion-level evidence; audit policy determines acceptance |
| Plan | No structured goal tasks | Link to the shared plan service; support dependencies and optional strict ordering |
| Requirements | One objective string | Preserve original objective plus revisioned acceptance criteria and constraints |
| Control | Set/replace/clear; Stop pauses | Add pause/resume/revise/status/history without retyping the objective |
| Budget | In-process turn count only | Persist actual goal-attributed usage and optional user-selected limits |
| Recovery | Desktop restart pauses | Engine-authoritative state; retain explicit resume and unknown-outcome protection |
| Audit | None | Optional isolated reviewer with recorded evidence, bounded retries and cost |
| Scope | One current goal per conversation | One active goal per conversation initially; archived history; multiple selectable goals later |
| Scheduling | Desktop completion event drives next prompt | One engine owner schedules continuations after durable settlement |

The upstream source is a reference, not the quality ceiling. Its auditor includes bash while relying on read-only instructions; tool-name-based progress estimates are not measured completion. Some audit-ledger writes are best-effort. Pipkin should enforce restricted reviewer tools, display actual activity, and commit required evidence before declaring completion. Its strict continuation contract is optional upstream; retain Pipkin's conservative behavior when the next action is ambiguous.

### GOAL-1 — typed lifecycle, control and migration

1. Add engine-owned `GoalRecord`, `GoalEvent` and snapshot/delta types. Record id, objective revision, state/reason, plan reference, criteria, active request, continuation epoch, timestamps, audit policy and optional limits. Proposed modules: `engine/goals/`, `store/goals.rs`, and a GPUI-free goal projection in core.
2. Move authoritative scheduling to the engine behind a capability flag. The desktop sends intent and mirrors state. When the backend advertises native goals, disable `advance_goal` scheduling for that conversation. Exactly one owner must exist; legacy Pi keeps its existing desktop runner until separately supported.
3. Preserve `/goal <objective>` as the fast direct path. Offer guided planning when requested or when requirements actually need clarification; do not require a confirmation ritual for every already-clear goal. Add explicit Pause, Resume, Revise and Archive actions and palette equivalents.
4. Migrate existing desktop goal records using an idempotent migration key containing backend/session identity and legacy goal revision. Import objective/pause reason as paused, with provenance. Never manufacture an audited completion from the old `Goal met.` string. Retain the old record until engine acknowledgement; do not run both schedulers during fallback.
5. Reconcile outstanding journal requests before activation. Engine restart pauses runnable goals until policy permits an explicit resume. Initially match today's behavior on app close: no additional goal turn after the controlling attachment is gone. Keep-running-after-close is a later opt-in requiring engine process lifetime support.

**Progress (2026-10-12).** GOAL-1 done: `goals.rs` (marker verdicts, the unchanged turn prompt, request keys), `store/goals.rs` (schema 14: `goals` with one current goal per session, archived history, `goal_events`, migration receipts in `kv`), `runtime/goals.rs` (the single scheduler: committed intent before send, settlement hook, Stop/restart/detachment/human-input pauses, Set/Pause/Resume/Revise/Archive, `MigrateGoal`), protocol 16 (`GoalView`, `GoalChanged`, the goal calls, capability `goals`), core mirroring with the desktop runner disabled on engine ownership and a one-shot migration offer, strip/palette/slash controls, and goal prompts displayed as their objective. See [docs/goals.md](../docs/goals.md). Not done: the plan link, criteria/limits (GOAL-2), audit (GOAL-3), guided planning, and keep-running after close.

### GOAL-2 — structured tools and trustworthy progress

- Add compact `get_goal`, `update_goal` and `propose_goal_completion` operations. Creation/resume/limit increases require user intent; tool availability alone does not authorize starting persistent autonomous work.
- Replace marker parsing on the native path with validated structured updates. Keep marker handling only for the legacy runner during transition.
- Associate every generated continuation with `(goal_id, goal_revision, continuation_epoch, request_key)`. Commit scheduling intent before enqueueing. Old completion/audit/job events cannot advance a replacement goal.
- User input, explicit Stop and unresolved questions take precedence. Integrate with the real engine queue instead of generating a second parallel queue. An expected wait on a known job uses an event subscription; it does not burn turns polling.
- Attach criteria to concrete evidence: tool/result ids, test exit status and command, artifact paths/hashes, relevant workspace revision, or a clearly labelled manual observation. Executor prose is a claim. Mechanical evidence is checked deterministically where possible; model judgment remains identified as judgment.
- Revising requirements or changing relevant artifacts invalidates stale evidence/audit acceptance. Keep prior history for inspection. Do not use task counts, elapsed time, or “tests passed” alone as proof of a user-facing outcome.
- Add optional token/cost/time/run limits and an explicit continuation allowance. Limits are visible and never invented by the model. Count executor, child and auditor usage once. Explain that provider-reported usage can arrive late: exact token/dollar hard stops require reservation/output caps or a conservative stop-before-next-call policy, not an impossible zero-overshoot promise.
- Detect repeated no-progress states using durable task/evidence revisions and repeated failures, then pause with an actionable reason. Use configurable, tested thresholds; do not let model wording alone reset the guard.

**Progress (2026-10-12).** GOAL-2 done: `tools/goal.rs` (`get_goal`, `update_goal`, `propose_goal_completion`), criteria with engine-checked evidence (tool-call ledger, file hashes, observations as judgment), structured decisions with the marker as a labelled fallback, person-set turn/token allowances checked before each turn with per-turn token attribution, stall detection on a durable progress signature, stale marking on revision, schema 15 and protocol 17, strip detail and `/goal turns|tokens|criteria`. See [docs/goals.md](../docs/goals.md). Not done: cost limits, job-event waits (JOB-1), and GOAL-3 below.

### GOAL-3 — optional independent completion audit

Use the native subagent foundation with a dedicated auditor profile; require SUB-1 policy enforcement first. The audit receives objective/criteria, relevant plan/evidence, workspace identity and the executor's labelled claim. It must not inherit the parent's goal runner, write tools, unrestricted shell, arbitrary extensions or recursive delegation.

Provide bounded read/search/list tools for inspection. If verification requires executing tests, run an explicit verification job under its real permissions and record it as potentially mutating execution; do not label arbitrary shell execution read-only. Pin the reviewer model at launch and report missing credentials without switching routes silently.

Auditor result: approved, changes requested, or inconclusive, plus criterion findings and evidence references. Malformed output, missing evidence, cancellation and provider failure cannot become approval. Default to a small bounded number of repair/audit rounds (initial proposal: two), then pause for review. User-selected limits include the audit cost. Skipping a required audit requires an explicit user action and is recorded as unreviewed completion, never “auditor approved.”

Persist completion proposal, audit receipt and final transition atomically where applicable. Freeze or record the inspected workspace/evidence revision; if relevant files change during review, mark the verdict stale and rerun or request a new completion proposal. Do not accept a previous revision's approval for a rewritten goal.

**Progress (2026-10-12).** GOAL-3 done: the built-in `auditor` profile (read, list_files, search_files, plan_read; no network, no delegation), the `verifying` state (schema 16 rebuilds the goals table; protocol 18), audit rounds bounded at two by default with findings fed into the repair turn, stale detection on a claim fingerprint, Stop/restart handling that never approves, `SetAudit`/`SkipAudit` with the skip recorded as unreviewed, and the audit's tokens counted once through the conversation tree. New bounded inspection tools `list_files` and `search_files` for every session. See [docs/goals.md](../docs/goals.md). Not done: executable verification jobs (JOB-1), a pinned auditor model setting, cost accounting beyond tokens.

### Acceptance and comparative evaluation

- Port all existing goal race/restart tests to the engine path before removing the old scheduler there.
- Crash after schedule commit, before send, after acceptance and before reply: one continuation, no duplicate work and no replay of an uncertain effect.
- Stop during completion/audit, replace during stop, late auditor result, queued human message, unanswered question and detached UI all produce deterministic states.
- A task marked complete with failing verification cannot satisfy a required passing criterion. An unavailable test is “unverified,” not pass or product failure.
- A required audit that times out/aborts fails closed; evidence-store failure prevents a completed state.
- Compare the current Pipkin runner, the pinned reference and the proposed implementation on the same tasks/models: straightforward fix, multi-step migration, false completion claim, missing environment, long interruption, changed criteria and failed audit. Record correct completion, false acceptance, unnecessary questions, recovery, wall time, model calls and tokens. Do not call the new implementation best in class until it improves on the old one without increasing unsafe completion or losing recovery guarantees.

## 10. Background jobs: durable records with honest process lifetime

### Current implementation and comparison

Existing bash executes within its tool call, streams bounded output and spills large output. Unix cancellation kills its process group. Background conversation execution means another conversation may be selected while work runs; it is not a job registry with durable handles and independent result delivery.

[pi-background-tasks](https://github.com/ismailsaleekh/pi-background-tasks) documents named jobs, bounded logs, result retrieval and completion delivery, alongside optional delegated/Fusion workflows. Its documented reload survival has specific limits; durable metadata does not imply arbitrary process survival across crashes. Adopt job ownership and delivery, while reusing Pipkin subagents for delegated model work.

### Implementation

**JOB-1 — supervisor and records.** Add `runtime/jobs.rs`, `store/jobs.rs` and protocol views. Proposed operations: start/status/list/read-log/cancel/result. Model tool start returns promptly with a stable job id and launch receipt; it does not hold the foreground model turn until exit.

Record owner session, originating call/user request, project/cwd, command or argv, environment references, start identity, timeout, output limits, state, exit status, log/artifact references and completion-delivery policy. Keep secrets out of display metadata.

- Use a shared process-supervision primitive extracted from bash, preserving its cancellation guarantees. Persist launch intent before spawn and receipt after spawn. Because spawning is not a database transaction, a crash between spawn and receipt requires explicit reconciliation; never automatically relaunch an ambiguous command.
- Use engine-instance identity plus an OS process identity/nonce or an owned supervisor connection. Never signal a reused PID based on a database row alone.
- States: starting/running/cancelling/succeeded/failed/timed-out/cancelled/interrupted/unknown. Exit code zero establishes command success, not goal completion. A server becoming ready is distinct from exiting successfully.
- Logs are append-only private artifacts with byte cursors, bounded previews, retention/disk quotas and explicit truncation. The UI subscribes to coalesced updates; it does not repeatedly reload entire files.
- Completion becomes a durable event. Deliver one UI notification and, only when requested by policy, one idempotent model follow-up. Restart or reconnect must not duplicate notifications or wakeups. A user-started job defaults to notification only.

**JOB-2 — native management.** Add an inspector Jobs view with active/finished split, command/cwd, elapsed time, log tail, errors and Stop. Keep typed links to plan tasks and subagents. Do not replace the working Subagents inspector with a generic tree that loses child-session details.

**Progress (2026-10-12).** JOB-1 and JOB-2 done: `store/jobs.rs` (schema 17), `runtime/jobs.rs` (intent before spawn, receipt after, `PIPKIN_JOB` marker with engine epoch and nonce, process-group kill plus marker-based straggler cleanup shared with `bash` through `tools/process.rs`, 8 MiB log quota with explicit truncation, coalesced progress, exactly-once completion delivery with `job:<id>:done` wakes and suppression after a Stop, startup reconciliation to `interrupted`/`unknown`, shutdown and session-removal cleanup), `tools/jobs.rs` (`job_start`, `job_status`, `job_log`, `job_wait`, `job_cancel`), the `Jobs`/`JobLog`/`JobCancel`/`JobStart` calls and `JobsChanged` (protocol 22, capability `jobs`), the core `JobsState` and the GPUI Jobs inspector tab. See [docs/jobs.md](../docs/jobs.md). Not done: JOB-3, user-started jobs from the desktop, non-Linux orphan matching.

**JOB-3 — longer lifetimes.** Initially qualify jobs across conversation changes and UI attachment loss only while their engine process remains alive. For an app-owned engine, explicitly define close behavior before exposing “keep running.” An independently running engine may continue jobs if its ownership policy permits. On engine restart retain records/logs but mark unverified processes interrupted/unknown. True restart-surviving jobs require a separate supervised worker/daemon protocol and are a later milestone.

### Acceptance

- Long build and dev-server fixtures let the conversation continue while logs update. Output floods cannot block Stop or grow memory/disk without limit.
- Cancellation handles grandchildren and inherited pipes; no unrelated process is killed. Maintain separate Unix and non-Unix qualification.
- Crash at each launch boundary never duplicates a command. Reopen shows retained logs and truthful process state.
- Duplicate completion events produce one model delivery. Stop/pause goals suppress automatic continuation while preserving job results.

## 11. Native status: absorb the footer's information, preserve our layout

### Current implementation and comparison

Pipkin already has model/thinking controls, run status, a goal strip, subagent counts and a Session usage dialog. `SessionUsage` contains committed model/tool totals and optional costs. The engine has context-estimation machinery, but lifetime session token consumption is not current context occupancy.

[pi-powerline-footer](https://github.com/nicobailon/pi-powerline-footer) highlights model/thinking, Git state, context and token/cost information; its current scope also includes queue and editor helpers. Use it as an information-design reference. Preserve Pipkin's authoritative queue, composer and native controls.

### Implementation

**STATUS-1.** Add a compact status model derived on state changes: selected model/thinking, execution/waiting state, active jobs/children, and a usage summary that opens the existing dialog. Hide empty segments and collapse secondary detail at narrow widths. Support keyboard and screen-reader access without continuous noisy announcements.

**STATUS-2.** Publish `ContextUsageView` from the engine's existing estimation/compaction path: tokens used, model capacity, remaining/output reserve where known, source (`reported`/`estimated`), measured revision and compaction state. Update after model responses, model switch and compaction. Mark stale/unavailable values explicitly. Never calculate context percentage from cumulative usage.

**Progress (2026-10-12).** STATUS-1 and STATUS-2 done: `runtime/context_usage.rs` (occupancy from the compaction estimator with reported/estimated/unavailable sources, capacity, reserve, remaining, staleness and compaction state; published after settlement, compaction and model switch; protocol 23, capability `contextUsage`), the core `StatusLine` derived from state with an accessible sentence, the GPUI status line in the composer toolbar with narrow-width collapsing, and a context section in the Session usage dialog. See [docs/status.md](../docs/status.md). Not done: STATUS-3, subscription quotas, native verification.

**STATUS-3.** Show cached Git branch/dirty summary from the existing workspace-change machinery after confirming its ownership and refresh policy. No Git subprocess during render. Treat subscription balance/quota as a separate provider capability; reported API-equivalent cost is not necessarily the user's invoice.

Do not adopt animated rainbow indicators, generated loading messages, a duplicate queue, or a second managed shell merely to match a terminal package. Composer stash/delayed send are separate optional UX proposals and need evidence of user benefit.

### Acceptance

- Missing price remains unknown, not zero. Reasoning/cache subsets are not double-counted. Child attribution does not inflate the root total.
- Context changes correctly after compaction/model switch and distinguishes estimates from reported values.
- Active counts update from engine state, not mounted rows. Narrow layout, larger text, dark/light themes and keyboard navigation remain usable.
- Coalescing prevents rapid token/status updates from degrading typing or scrolling; measure against the same build's baseline.

## 12. Observability: local correctness first, optional Langfuse export

### Comparative decision

The [Langfuse Pi plugin](https://github.com/langfuse/pi-observability-plugin) traces prompts, generations, usage, tools and nested agents. Its documentation describes an experimental integration and content/media export. Pipkin should keep ordinary execution independent of any remote tracing service, while providing useful local diagnostics and an explicit exporter.

### Implementation

**OBS-1 — correlation and local diagnostics.** Reuse session/run/request/call/child identities from the engine. Add provider request timing, first-token timing, retries, tool duration, cancellation, queue wait and terminal outcome. Store/export bounded structured records with provenance. Distinguish model generation latency from UI frame or display-presentation latency.

**OBS-2 — exporter interface.** Feed asynchronous sinks from stable engine events, with parent/child correlation and usage from the committed ledger. Evaluate OpenTelemetry/OTLP plus the current documented Langfuse mapping rather than importing the TypeScript plugin. Use bounded buffers, retry/backoff, drop/spool policy and a visible export-health state. Export failure must never fail or delay the model/tool operation. Shutdown gets a bounded flush.

**OBS-3 — opt-in configuration.** User chooses endpoint/project and content level. Default export off; metadata-only is the first enabled level. Prompt, tool body, image and source-content export require an explicit content setting. Redact credentials before enqueueing. Offer an example payload preview and a disable control. Use the existing credential abstraction; do not make remote telemetry a prerequisite for local costs or debugging.

### Acceptance

- A scripted parent/child run produces correctly related traces and exactly-once local accounting, including retries and errors. Remote delivery may be retried; use stable ids to avoid duplicate records where the backend permits.
- Collector unavailable, slow or rejecting payloads cannot block Stop, streaming or durable commits; buffer limits hold.
- Disabled export makes no exporter requests. Metadata-only payloads contain no prompt, environment, credential, tool-output or image bodies.
- Session restart preserves correlation boundaries without silently resending all historical content.

## 13. Delivery sequence and reviewable implementation slices

Each slice includes protocol/store changes where needed, engine behavior, native adapter/core projection, UI, tests and documentation. “Engine done; UI later” is not a shipped capability. Suggested size is relative complexity, not a calendar promise.

| Slice | Depends on | Deliverable / exit | Size |
| --- | --- | --- | --- |
| F0 | Current branch baseline | Capability/replay design, bounded tool host, schema migration strategy, reference revision manifest | M |
| ASK-1/2 | F0 | Model asks a grouped native question and receives durable typed answers | M |
| PLAN-1/2 | F0 | Persistent validated plan, compact tool API and native progress view | M |
| MCP-1 | F0 | Stdio + HTTP tools with lazy lifecycle and safe uncertain-outcome behavior | L |
| MCP-2 | MCP-1, ASK foundation | Connections settings, auth and explicit config import | L |
| WEB-1 | F0 | Search/fetch/read with inspectable citations and retained sources | M–L |
| SUB-1 | F0 | Profiles, tool restrictions, context/model policy and immutable launch records | M |
| GOAL-1 | PLAN-1, F0 | Engine-owned goal lifecycle, migration, no dual scheduler, native controls | L |
| GOAL-2 | GOAL-1, PLAN-2, ASK | Typed continuation/completion, criteria/evidence and persisted limits | M–L |
| JOB-1/2 | F0 | Managed jobs, bounded logs, completion events and native inspector | L |
| SUB-2/3 | SUB-1, job delivery foundation | Per-child controls and optional async children | M–L |
| GOAL-3 | GOAL-2, SUB-1; JOB-1 for executable verification | Optional restricted audit, bounded repair and evidence-gated completion | L |
| STATUS-1/2 | F0; consume feature views as available | Accurate native status/context and existing usage dialog refinement | M |
| OBS-1 | F0; extend as slices land | Local correlated diagnostics and timings | S–M |
| OBS-2/3 | OBS-1 | Optional exporter with privacy/performance qualification | M |
| Later | Relevant preceding gates | MCP rich resources/apps, WEB-2, independent job daemon, multi-goal selection/workflows | Separate scope |

Recommended work order is F0, Questions and Plans first as reusable foundations; MCP/Web next for immediate capability; Subagent profiles and typed Goals next; managed Jobs and optional Auditing after their lifecycle contracts are sound. Status and local instrumentation can accompany each slice. This refines the initial product-priority list into a dependency-aware implementation order.

### First reviewable change

Implement the smallest native model-visible question path using existing select/input UI and a versioned tool-result contract. Include unavailable-UI, cancellation, stale-id and replay tests. This validates the model → engine → protocol → GPUI → engine → model path before building more stateful tools. Do not block it on MCP or a generic extension marketplace.

**Progress (2026-10-12).** Done: `ask_user_question` built-in (`tools/ask.rs`, `runtime/ask.rs`), `ToolHost::ask`, result contract v1, `askUserQuestion` capability, and the generalized `recoverable_tool` / `replay_allowed` recovery registry replacing the subagent-only special case (F0 item 4). Tests: `tools/tests/ask.rs` (unit) and `runtime/ask_tests.rs` (end to end: answer, Other follow-up, decline, Stop, restart re-ask, no client, children excluded, capability). Not done from ASK-1/2: grouped questionnaires, multi-select, previews, notes, a background-conversation attention badge, question durability beyond intent-level re-ask, and native verification of the model-originated flow (see `docs/scorecard.md`). The remaining F0 items (effective tool catalog, request-key idempotency for local mutations, snapshot/delta extensions, migrations) are still open and should be taken with PLAN-1.

### Changes intentionally deferred

- Arbitrary classic TypeScript extension execution in the Rust engine.
- A universal workflow scripting language, recursive multi-agent swarms or remote-machine orchestration.
- Automatic model routing, broad provider subscriptions/credential reuse and background provider attribution hacks.
- MCP apps/remote HTML inside GPUI before the separate webview gate.
- PDF/video/OCR/browser automation as prerequisites for basic web research.
- Full task-management/project-management software or copying external extension configuration wholesale.

## 14. Comparative evaluation and release gates

### Reproducible comparison harness

Before each implementation, record the exact upstream tag/commit and package version, exact Pipkin revision, model/provider route, tool configuration, fixture revision and environment. The researched moving branches are sufficient for this plan, not a benchmark lockfile. Follow the relevant repository instructions when running third-party code; keep it in disposable profiles/workspaces with explicit credentials only for approved provider trials.

Run credential-free scripted fixtures for deterministic lifecycle behavior, then a small fixed live-model task set where it measures actual tool usability. Compare current Pipkin, the selected extension and proposed Pipkin on identical inputs. Keep provider variability separate from deterministic engine correctness. Record success rate, wrong/duplicate effects, recovery behavior, prompt/tool overhead, total usage, latency and native interaction quality. Report repeated samples and variation; do not select only favorable runs.

| Capability | Minimum comparison workload | Required property |
| --- | --- | --- |
| MCP | Many configured servers, few used; catalog update; lost mutation reply | Bounded idle resources and context; no blind mutation replay |
| Web | Multi-source research with a large page, bad result and expired content | Traceable citations and recoverable evidence; explicit failures |
| Subagents | Parallel review/implementation with one failure and one interruption | Correct ownership, preserved partial results, no duplicate child |
| Questions | Multi-choice + custom answer, partial answers, delayed reply | Exact user intent reaches the model without fabricated answers |
| Plans | Dependency graph, edits, compaction, restart and reopened task | Consistent durable state and clear progress |
| Goals | False completion, revised criteria, restart, failed audit, exhausted allowance | No unsupported completion or uncontrolled continuation |
| Jobs | Flooding logs, dev server, cancellation, ambiguous launch | Bounded resources and honest process state |
| Status | Compaction/model switch, unknown cost, narrow screen | Accurate quantities and usable presentation |
| Observability | Nested work with collector outage and content disabled | Useful correlation without execution dependency or unwanted export |

### Automated and native gates

For every code-bearing slice, run the repository-required gate:

```sh
cargo test --workspace && cargo clippy --workspace --all-targets && cargo fmt --all --check
```

Add meaningful state-transition, migration, socket/process and adapter end-to-end tests appropriate to the change. Retain existing regression suites; do not replace durability tests with UI snapshots. Capture crash-boundary fixtures, wrong-session/stale-generation cases, duplicate events, cancellation races, output limits and unavailable-backend behavior.

Native verification follows the GPUI skill and `scripts/guard.sh`: launch only an owned disposable-profile window, pin its PID, stay on the current workspace and send synthetic input only through the guard. No compositor changes, packages or scale changes without the required user authorization. Verify keyboard flow, focus restoration, long content, narrow drawers, light/dark themes, selection and responsiveness. Real IME, screen reader, display scale, suspend/resume and clean packaged launch remain separately measured/observed/unverified.

Update the relevant feature docs and scorecard with precise boundaries. Existing passing tests do not establish a new release candidate. New capabilities must appear correctly in the packaged native engine, not only an in-process test host. Missing dependencies/auth get actionable setup states rather than inert controls.

## 15. Risks and explicit design resolutions

| Risk | Resolution |
| --- | --- |
| Parallel code work changes the audited baseline | Re-audit touched protocol/store/runtime files before each slice; capture exact revision and preserve unrelated changes |
| Two schedulers continue the same goal | Capability-selected single owner, idempotent migration, durable continuation epoch and no silent fallback |
| Replay-safe flag accidentally reruns an external mutation | Per-tool recovery handler plus immutable recorded contract; uncertain remote effects stay uncertain |
| Tool catalogs consume model context or disturb caching | Compact discovery, stable ordering and staged activation; measure on each supported provider |
| “Read-only” auditor can still mutate through bash/MCP | Enforced tool policy, explicit verification jobs, no claims of OS sandboxing without one |
| Stale evidence approves changed work | Bind evidence/audit to goal and workspace revisions; invalidate on relevant changes |
| Durable job rows imply process survival that does not exist | Separate record durability, UI detachment, engine lifetime and independent worker survival |
| Task lists, goals and agents become inconsistent copies | Shared plan service plus typed links, independent execution records, transactional updates |
| OAuth/keys leak into prompts, snapshots or traces | Scoped credential references, redacted metadata and opt-in body export |
| Existing users lose saved goals or legacy Pi functionality | Idempotent paused migration, backend namespaces and capability-gated legacy path |
| Native UI grows into a wall of indicators | Compact progress/status, on-demand inspector detail, virtualized content and narrow-layout review |

## 16. Source register and audit limits

Primary external references consulted:

1. [Pi package catalog](https://pi.dev/packages?type=extension) — identifies the ten package candidates; download counts are not unique-user statistics.
2. [pi-mcp-adapter](https://github.com/nicobailon/pi-mcp-adapter) — README, lazy lifecycle/discovery and comparison discussion.
3. [pi-mcp-extension](https://github.com/irahardianto/pi-mcp-extension) and [catalog entry](https://pi.dev/packages/pi-mcp-extension) — transport/auth/reconnect documentation; catalog version observed: `1.5.1`.
4. [Official MCP Rust SDK](https://github.com/modelcontextprotocol/rust-sdk) and [SDK documentation](https://modelcontextprotocol.io/docs/sdk) — candidate implementation dependency, not yet selected/qualified in Pipkin.
5. [pi-web-access](https://github.com/nicobailon/pi-web-access) — search/fetch, source retention/retrieval and network-policy documentation.
6. [pi-subagents](https://github.com/nicobailon/pi-subagents) and [configuration](https://github.com/nicobailon/pi-subagents/blob/main/docs/configuration.md) — roles, context/model control and execution modes.
7. [rpiv-ask-user-question](https://github.com/juicesharp/rpiv-mono/tree/main/packages/rpiv-ask-user-question) and [tool schema](https://github.com/juicesharp/rpiv-mono/blob/main/packages/rpiv-ask-user-question/docs/tool-schema.md) — question/answer semantics.
8. [rpiv-todo](https://github.com/juicesharp/rpiv-mono/tree/main/packages/rpiv-todo) and [tool schema](https://github.com/juicesharp/rpiv-mono/blob/main/packages/rpiv-todo/docs/tool-schema.md) — persistence, dependency validation and task transitions.
9. [pi-goal-x](https://github.com/tmonk/pi-goal-x), [catalog](https://pi.dev/packages/pi-goal-x), [goal entry point](https://github.com/tmonk/pi-goal-x/blob/main/extensions/goal.ts), [core tools](https://github.com/tmonk/pi-goal-x/blob/main/extensions/goal-core-tools.ts), [completion](https://github.com/tmonk/pi-goal-x/blob/main/extensions/goal-completion.ts), [auditor](https://github.com/tmonk/pi-goal-x/blob/main/extensions/goal-auditor.ts), [state](https://github.com/tmonk/pi-goal-x/blob/main/extensions/goal-state.ts) — catalog version observed: `0.32.3`; source inspection is of moving `main`, which may differ from that release.
10. [pi-background-tasks](https://github.com/ismailsaleekh/pi-background-tasks) and [catalog/README](https://pi.dev/packages/pi-background-tasks) — job lifecycle, delivery and documented survival limits; catalog version observed: `2.6.9`.
11. [pi-powerline-footer](https://github.com/nicobailon/pi-powerline-footer) — status information and ancillary editor/queue scope.
12. [Langfuse Pi plugin](https://github.com/langfuse/pi-observability-plugin) — tracing coverage and optional configuration.

This is a source-backed implementation plan, not a security audit, native acceptance result, runtime benchmark of the extensions or promise of compatibility with every upstream feature. Each slice's implementation review must close its named evidence gaps before marking the capability complete.

### Planning-change validation

- Checked all 44 repository-local Markdown link targets; none were missing.
- The required workspace test/Clippy/format command completed successfully: 1,430 tests passed, 53 ignored, no failures; Clippy and formatting passed. Ignored tests remain unverified.
- The first sandboxed test attempt failed on local socket operations (`Operation not permitted`). The successful rerun used the approved unsandboxed execution path. No application code was changed to work around the environment.
- The shared checkout advanced to `03b7e5e30509eb42b68c312ca98ee76b1ffd50f5` during this work. Checks ran against the live shared checkout; this is not an immutable release-candidate qualification. The initial audit revision and working-tree caveat above remain the provenance for source findings.
- This task added only this plan. No extensions were installed, provider trials performed, native UI exercised, or proposed features implemented.
