# Pi engine audit, parts 5 and 6

ROOT = `<pi-staged>`. Unqualified paths are relative to `ROOT/packages/`. Pipkin paths are absolute.
The staged tree is the pinned upstream revision `d2a311097cbcf669e699479587332ae3988a49d0`
(`/home/jason/coding/pipkin/packaging/pi-engine-revision`) **with Pipkin's two overlays already applied**:
`pi-onboarding.patch` plus the copied `provider-auth*.ts` files, and `pi-subagents.patch` plus the copied
`subagents.ts`/`subagents-provider.ts` files. `diff` shows the staged `coding-agent/src/experimental/services/subagents{,-provider}.ts`
are byte-identical to `/home/jason/coding/pipkin/packaging/pi-subagents/`. `session-worker.ts:30,783-784` and
`services/worker.ts:23,81` contain the patched lines.

## 5. Plugins / extensions / facets

### 5.1 Three separate extension mechanisms (do not conflate them)

| Mechanism | Where | Loaded by the experimental server/session worker (Pipkin's path)? |
| --- | --- | --- |
| Chord **facet plugin packages** ("experimental plugins") | `coding-agent/src/experimental/plugins/*`, `chord/` | **Yes**: the only third-party code it loads |
| **Classic Pi extensions** (`ExtensionAPI`, `pi.registerTool`, ...) | `coding-agent/src/core/extensions/*`, `core/resource-loader.ts`, `core/package-manager.ts` | **No** (details in 5.4) |
| **Durable `Extension`s** (tools, sections, hooks, wraps, tasks) | `durable/src/harness/{define,registry,types}.ts` | Yes, but only the hard-coded set: `CodingTools`, `pi-prompt`, `subagent` |

### 5.2 What the experimental server and session worker load at runtime

**CLI → server.** `pi server -e <path>` is repeatable (`coding-agent/src/cli/experimental/commands/server.ts:33,40,49`). Paths are passed as
`pluginPackages` only when the list is non-empty (`:60`). Pipkin launches `pi server [--provider P --model M] [-e <ext>...]`
with `PI_EXPERIMENTAL=1`, `PI_SERVER_DIR`, `PI_SERVER_ID`, optional `PI_CODING_AGENT_DIR`
(`/home/jason/coding/pipkin/crates/pipkin-app/src/adapters/pi/engine.rs:174-199`). Pipkin's `--pi-extension` flag becomes `-e`
(`/home/jason/coding/pipkin/crates/pipkin-app/src/controller.rs:139-143,301`). **So Pipkin's "extension" is a chord facet plugin package, not a classic extension.**

**Server directory.** `resolveServerDirectory` = option, then `$PI_SERVER_DIR`, then `~/.pi/server`
(`experimental/server.ts:53-58`). The directory must be private (mode 0700, `:60-64`).

**Server-level plugin profile** (`experimental/plugins/package.ts`):
- `restoreServerPluginPackageProfile(dir, serverId, configured?)` (`:21-34`) works on the file `<dir>/plugin-packages-<serverId>.json`.
  - `configured === undefined` (no `-e`): the profile **restores the previously persisted list**, or `[]`.
  - An explicit list is normalized and written. An explicit empty list deletes the file. The CLI never sends an empty list (`server.ts:60`).
- **Per-Session profile:** `<dir>/session-plugin-packages-<serverId>-<sha256(sessionPath)[0..24]>.json`, read, written and removed at `:37-66,160-163`.
- **Profile JSON:** `{version:1, [sessionPath], packagePaths:[abs...]}`. It is strictly validated (no extra keys, non-empty strings, the session path must match) at `:99-131`. It is written with mode `0o600` (`:133-147`).
- Paths are `path.resolve`d and must be unique (`:149-158`). Otherwise the errors are "Plugin package path must not be empty" or "Plugin package paths must be unique".
- **Build:** `createServerPluginPackage` (`:69-97`) bundles each package with chord's esbuild bundler into
  `<dir>/plugin-builds/<serverId>/<label>-plugin-<sha256(path)[0..12]>/chord-facets.json` (`:75-76,165-171`).
  - Default facet entries are `{session: "src/session.ts", tui: "src/tui.ts"}` (`:13`).
  - Builds are serialized per package (`:77-95`). `build()` returns **only the `tui` artifact** (`:87-88`). The `session` entry is loaded from the manifest path by the worker.

**Selection resolution** (`experimental/server.ts:551-619`):
- The default selection is built from the server profile at startup (`:571`).
- `resolveSessionPlugins(metadata, requested?)` (`:573-602`):
  - With an explicit request, it builds, then persists the per-Session profile.
  - Otherwise it uses the cache, then the per-Session profile, and falls back to the default. It then **writes the default as the Session's profile on first open** (`:597-599`). So Sessions keep the plugin set they first opened with.
- `assertSessionPluginManifestPaths` refuses a different selection for a Session whose worker is running. It raises `SessionPluginSelectionConflictError` "Session <id> is active with a different plugin selection" (`experimental/session-worker-manager.ts:36-41,136-142`). This is mapped to `RoutedServerError("service_invalid_value")` (`server.ts:460-462`).
- `openSession` passes `selected.manifestPaths` to the worker (`server.ts:476-479`).
- Deleting a Session removes its profile (`:451,603-606`).

**Server service `pi.presentation-plugins`** (`experimental/services/plugins.ts:4-12`), mode singleton:
- `prepareSession({sessionId, packagePaths: string[] | null}) → JsonValue`. `server.ts:453-469` returns `{packagePaths, presentationPlugins}`.
- `reload() → JsonValue`.
- The presentation data is `{presentationFacetBundles: FacetBundleArtifact[]}` (`plugins/bundled.ts:9,32-36`).
- A client turns that data into loaders with `createPresentationFacetLoaders` (`bundled.ts:39-49`). It loads only artifacts the server sent. The errors are "Invalid presentation plugin data" and "Invalid presentation plugin bundle list".

**Session worker.**
- The worker args include `pluginManifestPaths: string[]` (`session-worker.ts:82,144,517`).
- `createCodingAgentHarness` (`:776-829`) builds the registry as `createCodingRegistry` (= `CodingTools` + `createPiPrompt`; `durable/harness-setup.ts:52-57`) plus `Subagent` (patch, `:783-784`).
- It sets `facetLoader = createSessionPluginFacetLoader(manifestPaths)` (`:819`). That function makes one loader per manifest and uses the `session` entry only if the manifest has one; otherwise there are no facets (`plugins/bundled.ts:12-30`).
- The bundle external `@earendil-works/pi-coding-agent/experimental/plugin` is resolved to the engine's own `experimental/plugin.{ts,js}` (`bundled.ts:10,51-55`).
- Harness settings never set `extensions`, so the default durable selection is *every installed extension* (`harness-setup.ts:25-49`; `durable/src/harness/agent.ts:220-236`).

**Session services host** (`experimental/services/worker.ts:54-159`):
- Built-in facets (`:75-83`):
  - `@pi/agent-controller-runtime` → `pi.agent-controller`
  - `@pi/session-plugins-runtime` → `pi.session-plugins` `{reload()}`
  - models → `pi.models`
  - transcript → `pi.transcript`
  - `@pi/history` → `pi.history`
  - `@pi/subagents` → `pi.subagents` + keyed `pi.subagent-transcript` (patch)
  - ui-requests → `pi.ui-requests` + local `pi.local.ui-request-host`
- The plugin facets are added to the same `createFacetHost` (`:84-96`).
- `SessionPlugins.reload()` reloads **only plugin facets** through `facetHost.reload` and serializes reloads. On failure it disposes the candidate, and `AggregateError` "Session plugin reload and cleanup failed" is possible (`:97-117`).
- Each `(serverConnectionId, attachmentId)` scope gets its own remote endpoint (`:120-142,161-163`).

### 5.3 Chord facet API: what a facet can contribute

- **Definition.** `defineFacet({id, setup(env)})` (`chord/src/api.ts:69-71`; `types.ts:303-306`).
- **`FacetEnvironment`** (`chord/src/types.ts:281-301`):
  - `use(service)` declares a hard singleton dependency.
  - `observe(service, handler)` watches a keyed service.
  - `provide(service, impl)` installs a singleton.
  - `provideMany(service)` returns a `ServiceSpawner` that does `.spawn(key, impl)`.
  - `replicatedState(initial)` returns a `MutableReplicatedState` with `change(ctx, draft=>…)` and `replace` (`:59-71`).
  - `own(dispose)`, `onActivate(cb)`, `onDeactivate(cb)`.
- **Services.** `defineService<T>(id, {local?})` (`api.ts:73-85`). The id must be non-empty, and the `$chord.` prefix is reserved. A `local:true` service is process-local and never published remotely (`types.ts:126-131`).
- **Loaders.** `createStaticFacetLoader`, `combineFacetLoaders` (`api.ts:32-60`). `FacetHost{services, reload(facets), dispose}` (`types.ts:325-339`).
- **Bundle format.** `chord-facets.json` `{format:"chord.facet-bundle", formatVersion:2, plugin:{id,version?}, entries:{name:{file, integrity(sha256 SRI), externalImports, sourceMap?}}}`. A transport artifact is `{format:"chord.facet-bundle-artifact", formatVersion:2, plugin, entryName, entry, source, sourceMapContents?}` (`chord/src/node/manifest.ts:1-39`). The output is CommonJS `.cjs` built by esbuild (`node/bundle.ts:120-145`).
- **Package metadata.** `package.json` needs a non-empty `name` and `version`, plus `peerDependencies` and an optional `chord: {facets:{name: path|false}, external, sourceMap}` (`node/package.ts:84-150`).
  - Defaults apply only if the file exists. `false` removes a default (`:155-196`).
  - The error when nothing is configured is "Facet package <name> has no configured or conventional facet entries".
- **Plugin API surface** (`coding-agent/src/experimental/plugin.ts:1-28`) exports:
  - `AgentController`: `prompt/steer/followUp/cancelQueued/abort/compact/lookup/waitForPrompt` (`services/agent-controller.ts:64-84`).
  - `History.page`.
  - `PresentationUI` (local): `select`, `showStatus` (`services/presentation-ui.ts:10-20`).
  - `SlashCommands` (local `pi.local.slash-commands`): `register/replace/list/subscribe` of `{name, description?, argumentHint?, getArgumentCompletions?, run(args, ctx)}` (`services/slash-commands.ts:12-30`).
  - `UiRequests`, `UiRequestHost`.
- **Session vs presentation facets.**
  - A **session** facet runs in the worker. It can provide new remote services (for example, the example's `pi.example-plugin.greeting`) and use `UiRequestHost`, `AgentController` and `History`.
  - A **tui/presentation** facet runs in the client process. It can use `SlashCommands`, `PresentationUI` and remote services (`coding-agent/examples/plugins/pi-example-plugin/src/{session,tui,contract}.ts`).
- **What a facet cannot contribute.** Facets **cannot contribute durable tools, prompt sections, hooks or model providers**. No service exposes the `Registry`/`Harness`, which stays inside the worker and is installed before the facets load (`session-worker.ts:783-791`).
- **UiRequestHost limits** (`services/ui-requests.ts:93-97`):
  - Default timeout is 120 000 ms (0 means indefinite).
  - Max pending is 20; further asks are refused (`ui-requests-provider.ts:73`).
  - Max notices kept is 20 (`:54`).
  - Input answers are at most 10 000 chars ("An answer can be at most 10000 characters.", `:166-167`).
  - Titles are truncated to 300 chars (`:83`).

### 5.4 Classic extension system (not loaded by the durable/experimental path)

**Discovery** (`core/extensions/loader.ts`):
- Sources:
  1. `<cwd>/.pi/extensions/`
  2. `<agentDir>/extensions/`
  3. Configured paths (`:828-876`)
- `agentDir` = `$PI_CODING_AGENT_DIR` or `~/.pi/agent` (`config.ts:581,605-611`). `CONFIG_DIR_NAME` = `piConfig.configDir` or `.pi`.
- Per directory (`:780-822`), one level deep:
  - `*.ts`/`*.js` files
  - `*/index.{ts,js}`
  - `*/package.json` with `pi.extensions` (`:744-778`, `core/pi-manifest.ts:5-11`; the `pi` manifest also lists `skills`, `prompts`, `themes`)
- An extension is a default-exported `ExtensionFactory = (pi: ExtensionAPI) => void|Promise<void>` (`types.ts:2004`).

**Resource/package layer** (`core/resource-loader.ts`, `core/package-manager.ts`):
- Settings keys `packages` (npm/git sources), `extensions`, `skills`, `prompts`, `themes`, `enableSkillCommands` (`core/settings-manager.ts:159-164`).
- Auto-discovered dirs are `<agentDir>/{extensions,skills,prompts,themes}` and `<cwd>/.pi/{…}`, plus `~/.agents/skills` and project `.agents/skills` (`package-manager.ts:2413-2460,475`).
- Packages install to `<agentDir>/npm`, `<agentDir>/git` or `<cwd>/.pi/{npm,git}`. Project scope requires trust (`:2085-2093,2166-2174`).
- Project trust gates project extensions. A pre-trust pass loads only user/global/CLI extensions (`resource-loader.ts:497-502`).
- Built-ins `builtin:<name>`: `llama.cpp`, `codemode` (replaceable), `tool-search` (replaceable), `mcp` (replaceable) (`coding-agent/src/extensions/index.ts:7-14`; `resource-loader.ts:706-736`). The CLI flag is `--extension/-e <path>` for the *classic* `pi` (`cli/args.ts:179,316`).

**`ExtensionAPI`** (`core/extensions/types.ts:1551-1876`):
- **Events** via `on(event, handler)`:
  - Project and resources: `project_trust`, `resources_discover`.
  - Session lifecycle: `session_start`, `session_info_changed`, `session_before_switch`, `session_before_fork`, `session_before_compact`, `session_compact`, `session_compact_failed`, `session_shutdown`, `session_before_tree`, `session_tree`.
  - MCP: `mcp_servers_change`.
  - Context: `context`, `context_with_system`, `cache_warming_decision`.
  - Provider: `before_provider_request`, `before_provider_headers`, `after_provider_response`, `provider_stream_event`.
  - Agent: `before_agent_start`, `agent_start`, `agent_end`, `agent_settled`.
  - UI prompt: `ui_prompt_start`, `ui_prompt_end`.
  - Turns and messages: `turn_start`, `turn_end`, `message_start`, `message_update`, `message_end`.
  - Tool execution: `tool_execution_start`, `tool_execution_update`, `tool_execution_end`.
  - Selection: `model_select`, `thinking_level_select`.
  - Interception: `tool_call`, `tool_result`, `user_bash`, `input`.
- **Registration and actions:**
  - `registerTool` (`ToolDefinition`, `:565-640`: `name`, `label`, `description`, `promptSnippet`, `promptGuidelines`, TypeBox `parameters`, `prepareArguments`, `outputSchema`, `exposure`, `namespace`, `annotations`, `defaultActive`, `executionMode`, `execute`, `renderCall`/`renderResult`).
  - `registerCommand`, `registerShortcut`, `registerFlag`/`getFlag`.
  - `registerMessageRenderer`, `registerMarkdownTransformer`, `registerEntryRenderer`, `registerToolRenderer`.
  - `sendMessage`, `sendUserMessage`, `appendEntry`, `setSessionName`/`getSessionName`, `setLabel`, `exec`.
  - `getActiveTools`/`getAllTools`/`setActiveTools`, `getCommands`, `getSettings`, `setModel`, `get`/`setThinkingLevel`.
  - `registerProvider` (`Provider` object or `name+ProviderConfig`), `unregisterProvider`.
  - `registerMcpServer`/`unregisterMcpServer`/`getMcpServers`.
  - `registerVirtualModel`/`unregisterVirtualModel`.
  - `events: EventBus`.
- **`ctx.ui`** (`ExtensionUIContext`, `:149-299`):
  - Dialogs and notices: `select`, `confirm`, `input`, `notify`, `editor`, `custom`.
  - Terminal and status: `onTerminalInput`, `setStatus`, working message/indicator, `setWidget`/`setFooter`/`setHeader`/`setTitle`.
  - Editor: `pasteToEditor`, `set`/`getEditorText`, `addAutocompleteProvider`, `set`/`getEditorComponent`.
  - Themes and tools: `getAllThemes`/`getTheme`/`setTheme`, `get`/`setToolsExpanded`.
- **MCP:**
  - `core/mcp-servers.ts` only validates and stores registrations (`:1-6,281-289`). Exposure is `codemode|deferred|direct|hidden` (default `codemode`, `:17-45`). Configs are stdio or HTTP with OAuth (`:45-115`).
  - The built-in `mcp` extension connects servers from `<agentDir>/mcp.json` and, for trusted projects, `<cwd>/.pi/mcp.json` (`coding-agent/src/extensions/mcp/config.ts:4-5,147-148`).
  - It uses the standalone client in `packages/mcp` (stdio and Streamable HTTP transports; `mcp/README.md`).

**Durable path loads none of this.** No file under `coding-agent/src/experimental/` calls `discoverAndLoadExtensions`, `loadExtensions`, the `ExtensionRunner`, `McpServerRegistry` or `builtInExtensions`.
- The only `core/` resource code it uses is:
  - `loadProjectContextFiles` and `loadSkills({…, skillPaths: settings.getSkillPaths(), includeDefaults: true})` inside the `pi-prompt` durable extension (`experimental/durable/prompt.ts:26-74`). So AGENTS-style context files and skills **do** reach the system prompt, cached per cwd.
  - The experimental TUI client builds `DefaultResourceLoader` with `noExtensions/noSkills/noPromptTemplates/noContextFiles: true`, just for themes (`experimental/client-tui.ts:735-742`).
- **Missing in the engine Pipkin runs:**
  - Classic `registerTool` tools, custom commands, `registerProvider` providers, MCP servers, codemode, tool-search, llama.cpp and hooks.
  - Skills or extensions installed through `packages`. `loadSkills` here is not given package-manager paths.

### 5.5 Durable package extension API (`durable/src/harness/`)

- **Constructors** (`define.ts:6-44`):
  - `defineExtension(ext)`, `defineTool(tool)`, `section(key, render, {tag?})` (tagged `<key>…</key>` unless `tag:false`).
  - `hook(task, handlers)` → `{task: name, handlers}`.
  - `wrapTool(tool, wrapper)`, `wrapSection(key, wrapper)`.
- **`Extension`** (`types.ts:266-276`): `{name, tools?, sections?, hooks?, wraps?, tasks?}`.
- **`ToolRegistration`** (`types.ts:210-235`): the pi-ai `Tool` (`name`, `description`, TypeBox `parameters`) plus:
  - `replay?: "safe"|"unsafe"` (default unsafe)
  - `executionMode?`
  - `prepareArguments?` (pure)
  - `outputLimits?{maxBytes,maxLines,retain}`
  - `execute(args, api: ToolExecutionApi, ctx) → ToolExecutionResult{content?, isError?, details?, diagnostics?, usage?, control?{addTools, terminate, handoff}}` (`:130-160`)
- **`ToolExecutionApi`** (`:166-207`):
  - Identity and context: `taskId`, `conversationId`, `callId`, `registry` snapshot, `agent()`, `env`.
  - Output and reporting: `output()`, `outputWindow`, `diagnostic()`, `details()`.
  - Persistence: `commit(tx=>…)`, `memo`.
  - Tasks: `createTask`, `getTask`, `waitForTask`.
  - `conversation(id)` returns a `ConversationHandle{submit, abort, waitForIdle}` (`:119-128`).
- **Prompt sections.** `PromptSection{key, render(PromptInput), tag?}` (`:237-256`). The key must match `/^[a-z][a-z0-9_-]*$/` and must not be `instructions` (`registry.ts:7,96-109`).
- **Hooks** (`types.ts:614-674`):
  - Generation: `beforeRequest`, `afterResponse`, `onYield` (may `continue`), `afterTools`.
  - Tool: `beforeTool` (first `block` wins; otherwise `arguments` replace; a throw blocks), `afterTool` (replaces the result).
  - Compaction: `beforeCompact` (`decline` or a `summary`).
  - `HookApi` = reads plus the task's memos.
- **Registry** (`registry.ts:60-114`):
  - Built-in tasks are Generation, Tool and Compaction (`:10`).
  - `install` replaces in place by name; `uninstall` removes by name. Each call publishes synchronously to subscribers.
  - Tool names must be unique within one extension (they may repeat across extensions). Task names must be globally unique.
- **Selection per conversation** (`agent.ts`):
  - `AgentDoc` is kind `pi.agent`, conversation scope, `history:"rewindable"`, `fork:"asOf"` (`:42-50`).
  - `AgentState.extensions`: an array selects exactly those, in order; an `{add, remove}` object edits the host default (settings `extensions`, otherwise all installed) (`types.ts:306-317`; `agent.ts:220-236`).
  - `tools` filters tools (an array or `{remove}`).
  - Tools are composed by name with later extensions winning, then wraps are applied. A throwing or renaming wrap drops its target (`agent.ts:154-213,238-256`).
- **`configure(tx, convId, AgentChange)`** (`agent.ts:68-71,87-120`): each given field replaces the stored one, `null` clears it, `undefined` leaves it. `extensions`/`tools` take objects and are stored as names.
- **New conversations.** A new **task-owned** conversation copies its owner conversation's whole stored agent doc (`createAgent`, `agent.ts:133-139`).

### 5.6 `docs/extensions.md` vs the code

The doc is `/home/jason/coding/pipkin/docs/extensions.md`.

- **Accurate claims:**
  - Services `pi.ui-requests` (remote: `state{requests,status,notices}`, `respond`, `cancel`) and local `UiRequestHost` (`select/confirm/input/notify/setStatus`) (`ui-requests.ts:53-91`).
  - The 2-minute default timeout with 0 meaning indefinite (`:93`).
  - The 20-open-questions cap (`:94`).
  - Slash commands from a TUI facet are process-local (`pi.local.slash-commands`, `slash-commands.ts:30`), so they are not visible remotely.
- **Discrepancies and gaps:**
  1. **Pi location.** The doc says the contract lives "in the fork `../pi-fork/pi`". The shipped engine is the *clean pinned upstream archive* plus Pipkin patches (`/home/jason/coding/pipkin/scripts/build-engine.sh:33-60`). `ui-requests*` comes from the pin itself: no Pipkin patch adds it.
  2. **"Extension" naming.** `--pi-extension` loads a **chord facet plugin package** (`pi server -e`), not a classic Pi extension. The doc's last table row understates the gap. Stable-API extensions are **not loaded at all** by the experimental worker: their tools, commands, providers, MCP servers and hooks are also absent, not just `ctx.ui.*` routing (5.4).
  3. **Sticky selection, not described.** Omitting `--pi-extension` makes `pi server` *restore* the last persisted list from `plugin-packages-<serverId>.json` (`package.ts:27-28`). Pipkin never sends an empty list to clear it (`engine.rs:183-185`). Each Session also pins its first selection (`server.ts:597-599`).
  4. **Missing input limits.** The doc omits the input-length limit (10 000 chars) and the title truncation (300).
  5. **Notices.** "Latest three messages" is a Pipkin presentation choice. The engine keeps 20 notices (`ui-requests.ts:95`).
  6. **Plugin tools.** The doc does not say that experimental plugins cannot add model-visible tools, prompt sections or providers (5.3). This matters for a Rust port's plugin story.

## 6. Subagents overlay

### 6.1 Staging and what the patch changes

- **Build steps.** `scripts/build-engine.sh` in clean (non-`PIPKIN_ENGINE_DEV`) mode (`/home/jason/coding/pipkin/scripts/build-engine.sh:33-104`):
  1. `git archive` of the pinned commit.
  2. Temporary `git init`, then `git apply --check && git apply` of `pi-onboarding.patch` and then `pi-subagents.patch`. After that, `rm -rf .git`.
  3. Install `pi-onboarding/provider-auth{,-provider}.ts`.
  4. `install -m 0644` of `pi-subagents/{subagents.ts,subagents-provider.ts}` into `experimental/services/` (`:68-72`).
  5. Assertions: `registry.install(Subagent)` is present in `session-worker.ts` and `createSubagentsServiceFacet` in `worker.ts`. Otherwise the build fails with "native subagents were not applied to the staged engine" (`:73-76`).
  6. `pipkinSubagentsSha256 = sha256(patch ‖ subagents.ts ‖ subagents-provider.ts)` goes into `engine.json` (`:77-78,104`).
- **Dev mode.** `PIPKIN_ENGINE_DEV=1` stages a dev checkout verbatim with no subagent overlay.
- **Onboarding patch.** It only adds the `ProviderAuth` (`pi.provider-auth`) singleton server service (`packaging/pi-onboarding.patch:1-83`). It does not touch subagents.
- **Already upstream at the pin:** the `Subagent` durable extension (`coding-agent/src/experimental/durable/subagent.ts`), `History`/`createHistory`, and the harness task graph and views.

**`pi-subagents.patch`** (`/home/jason/coding/pipkin/packaging/pi-subagents.patch`) makes four changes:
1. **`experimental/services/worker.ts`** (`patch:1-19`) imports `createSubagentsServiceFacet` and adds `await createSubagentsServiceFacet(options.harness, options.conversation.id, BACKGROUND_CONTEXT)` to the built-in facets. In the staged tree this is `worker.ts:23,81`.
2. **`experimental/session-worker.ts`** (`patch:21-49`) imports `Subagent`, hoists `const registry = createCodingRegistry(...)`, calls `registry.install(Subagent)`, and passes `registry` to `Harness.open` (staged `:30,783-791`). **Upstream never installed the subagent tool in the experimental worker.**
3. **`durable/src/harness/tool.ts`** (`patch:50-137`; staged `tool.ts:35-45,93-108,114-154,163-171`):
   - **Checkpoint schema.** `ToolTaskCheckpoint`'s `execute` phase gains optional `recoveryExtension?: string` and `recoveryContract?: string`.
   - **Writing the intent.** When the execute intent is committed, `runtime.registry.tools()` is searched for the *identical* object (`registered === tool`). If found (that is, the tool is unwrapped), the two fields are recorded:
     - `recoveryExtension` = the owning extension name.
     - `recoveryContract` = `JSON.stringify({name, description, parameters, replay})`.
     - This happens for **every** unwrapped tool, not just `subagent`, so persisted checkpoints change shape for all tools.
   - **Recovery** (`execute` phase):
     - `selected` = the tool from the current agent selection.
     - The fallback is used only if all of these hold: `replay==="safe"`, `call.name==="subagent"`, `prior==="subagent"`, and the contract is recorded. It then takes `registry.extension("subagent").tools` filtered to the same name, `replay:"safe"` and the same contract.
     - `candidate = selected ?? fallback`. For `subagent` with a recorded contract, a candidate whose contract differs is rejected (`tool = undefined`).
     - It reruns only if the stored and current replays are both `safe`, clearing the slot progress first. Otherwise it settles `failed` with "Tool subagent was interrupted and may have partially run", and the owned conversations are aborted.
     - New calls to a deselected tool still fail in the `call` phase with `tool_unavailable` "Tool subagent is not available" (`tool.ts:63-66`).
4. **`durable/test/harness-tools-recovery.test.ts`** (`patch:142-216`) adds two tests:
   - Resume after `configure({extensions: []})` and a restart. The run counts are 2, and a later new call errors.
   - A removed registration, a changed description or a changed replay policy each get **no** recovery authority. The result is an error containing "interrupted".

### 6.2 The `subagent` durable tool (`coding-agent/src/experimental/durable/subagent.ts:24-55`)

- **Extension.** `name: "subagent"` with one tool, `name: "subagent"`.
- **Description (exact text):** "Delegate a self-contained task to a subagent with the same tools and get its answer back. Give it everything it needs to know; it does not see this conversation."
- **Parameters:** `Type.Object({ task: Type.String({ description: "What the subagent should do" }) })`. As JSON Schema: `{"type":"object","properties":{"task":{"type":"string","description":"What the subagent should do"}},"required":["task"]}`.
- **Replay policy:** `replay: "safe"`, because a rerun finds the child it created and the submission it made (`:32-33`).
- **Execution:**
  1. In one `api.commit`, it looks for `tx.scanConversations({ownerTaskId: api.taskId}, 1)`. If none exists, it runs `tx.createConversation({ownership: {kind: "task", taskId: api.taskId}})`, then `configure(tx, child, {extensions: {remove: [Subagent]}})` (`:35-42`).
     - `createAgent` has already copied the *parent's* stored agent doc (model, thinkingLevel, tools filter, instructions, cwd).
     - The `configure` call then **overwrites** `extensions` with `{remove:["subagent"]}`. Any array selection or add/remove edits stored on the parent are dropped in the child, so the child cannot delegate further.
  2. `api.details({conversationId: child})` publishes the link before completion (`:43`).
  3. `handle.submit({type: "input", content: args.task, requestId: \`subagent:${api.taskId}\`})`, then `.wait()` (`:44-46`).
     - The request id makes the admission idempotent on replay: `admitSubmission` returns the existing submission (`durable/src/harness/submissions.ts:167-175`).
  4. If `settled.status !== "done"` or the type is not `input`, it throws `Subagent ${child} failed: ${status}`. In practice the only other settled status is `unanswered` (`types.ts:74-76`).
  5. **Answer text** (`subagent.ts:13-17`): the concatenation, with no separator, of every `type:"text"` part in `AssistantEntry(answer).model[0].content`. It is `""` if the entry is missing.
  6. Returns `{content: [{type: "text", text}], details: {conversationId: child}}`.
- **Parent/child mapping:**
  - The child's `ConversationRecord.owner = {conversationId: parent, taskId}`.
  - The task is a `pi.tool` task whose input is `{assistant: EntryId, callId}` (`tool.ts:33`).
  - Aborting or failing the tool task aborts its owned conversations. The child persists after success.

### 6.3 Service contract (`experimental/services/subagents.ts`; impl `subagents-provider.ts`)

- **Services.**
  - `pi.subagents`: singleton, facet `@pi/subagents`.
  - `pi.subagent-transcript`: keyed (`provideMany`). The key is the decimal child conversation id; the value is `{state: ReplicatedState<ConversationView>}` (`subagents.ts:14-19`).
- **Construction** (`subagents-provider.ts:15-28`). This runs once per worker with `BACKGROUND_CONTEXT`:
  - It gets the root `Conversation`, `harness.taskGraph()`, and `harness.documentState(AgentDoc, rootId)`.
  - If the agent doc is missing, it throws "Root conversation <id> has no agent document".
  - `enabled` = whether the root's resolved agent tools include `"subagent"`.
- **Replicated state.** The service has two replicated states:
  - **`activity: ReplicatedState<TaskGraph>`** is the *whole Session's* live task graph, not filtered to subagents. It is `{tasks: {"<id>": {id, kind, conversationId, owner?, background, abortRequested, state, conversations[]}}}`. Terminal tasks drop out (`durable/src/harness/task-graph.ts:20-47`).
  - **`configuration: ReplicatedState<{enabled: boolean}>`** is re-derived on every root `AgentDoc` change. A revision counter discards stale async results. Errors are logged as "Subagent configuration sync failed" (`subagents-provider.ts:35-56`).
- **Events.** There are no explicit events; clients observe those two replicated states and keyed transcript states.
- **Methods.** Wire args are positional, with the context appended by the host. For example, Pipkin's `call("pi.subagents", "setEnabled", [bool])` is at `/home/jason/coding/pipkin/crates/pipkin-app/src/adapters/pi/mod.rs:1121`.
  - **`list() → SubagentSummary[]`** (`:57-100`):
    - Pages `tx.scanConversations({ownerConversationId: rootId}, 128, cursor)` until exhausted. **There is no overall cap.**
    - Keeps only children that pass every check:
      - The owner conversation is the root.
      - The owner task `kind === "pi.tool"`.
      - The input is `{assistant: safe int, callId: string}`.
      - The `AssistantEntry.model[0]` is an assistant message containing a `toolCall` with that id and `name === "subagent"`.
    - **Row:** `{conversationId, taskId, callId, task: arguments.task or "Subagent", status}`.
    - **Status:** a terminal task maps `completed` to `done`, `aborted` to `aborted`, and anything else to `failed`. A non-terminal task keeps its raw `pending|running|waiting|completing`.
  - **Child validation** `child(id)` (`:101-108`):
    - Rejects an id that is not a safe integer ≥ 1 or not in `list()` with "Not a subagent of this session". Every `view`/`open`/`page` call re-runs a full `list()` scan.
    - A missing conversation raises "Subagent conversation no longer exists".
  - **`view(id) → ConversationView`** (`:113-120`): a one-shot `viewState().value`, disposed immediately. The view is `{conversation: ConversationRecord, entries: active EntryRecords (head marker first), docs: {pi.agent, live, inbox, provider, usage}}` (`durable/src/harness/view.ts:26-33,48-55`).
  - **`open(id) → void`** (`:121-145`):
    - Validates *before* the cache.
    - It is idempotent per id: it caches the in-flight promise.
    - It refuses a 65th mount with "Too many child transcripts are open in this session". The cap is 64 per worker, shared by all clients.
    - It spawns the keyed `pi.subagent-transcript` instance `String(id)` with a live `viewState`, which is disposed only at facet teardown (worker retirement). On failure it removes the cache entry so the call can be retried.
    - Pipkin then subscribes keyed (`mod.rs:2392-2397`).
  - **`page(id, {before: number|null, limit}) → {entries: JsonValue[], more: boolean}`** (`:146-148`) delegates to `createHistory(child).page` (`history-provider.ts:6-25`):
    - Entries come newest first.
    - `limit` is truncated and clamped to 1..200 (`HISTORY_MAX_PAGE`, `history.ts:30`).
    - `before` must be null or a positive safe integer; otherwise "history.page: before must be a positive entry id or null". `before === 1` returns an empty page with `more: false`.
    - `more` = whether a next cursor exists.
  - **`setEnabled(enabled: boolean) → void`** (`:149-178`):
    - A non-boolean raises "setEnabled requires a boolean".
    - It edits the **root conversation's `pi.agent` doc** (`AgentState.extensions`) in one commit:
      - If the stored value is an array, it adds or removes `"subagent"`.
      - Otherwise it uses the `{add, remove}` form. Enable removes `"subagent"` from `remove` and adds it to `add`. Disable adds it to `remove` and filters it from `add`.
    - It then re-reads the effective selection and publishes `configuration.enabled`.
- **Enable setting.**
  - **Key:** `"subagent"` in the root `pi.agent.extensions`.
  - **Storage:** the Session's SQLite database (`sessionStoragePath`).
  - **Default:** enabled, because settings carry no `extensions`, so every installed extension is selected.
  - **Caveats:**
    - `AgentDoc` is `rewindable`/`fork: asOf`, so forks and rewinds take the setting as of that point.
    - Disabling only hides the tool for **new** calls. An in-flight call keeps running, and after a restart it resumes through the patch's fallback (6.1).
- **Recovery and replay, as a whole:**
  - The tool is `replay: "safe"`.
  - A crashed call reruns `execute`, finds the existing owned child (`ownerTaskId`), and re-submits with the same `requestId`, which deduplicates to the original submission. It then waits for that submission's answer.
  - The slot progress is cleared before the rerun.
  - The README admits that identical-contract code replacement and wrapped tools are not proven safe (`/home/jason/coding/pipkin/packaging/pi-subagents/README.md:17`). The contract is metadata-only, not a code signature (`tool.ts:163-164`).
- **Limits summary:**
  - Engine side:
    - Scan page size 128 (no total cap).
    - Live transcript mounts capped at 64 per worker.
    - History page size up to 200.
    - Task graph scan page 256 (`task-graph.ts:56`).
  - Pipkin side: 100 children and the latest 80 entries in the preview (README `:20`).
