## 4. Session / worker / server service API

Paths are relative to `ROOT/packages/`. Shorthand: `ce/` = `coding-agent/src/experimental/`, `svc/` = `coding-agent/src/experimental/services/`. Everything here is gated behind `PI_EXPERIMENTAL=1` (`ce/commands.ts:85-98`) and is excluded from npm/binary builds (`svc/README.md:12`).

### 4.1 Process architecture

Four process roles, all the same Node/Bun executable re-spawned with a role env var:

| Role | Entry | Spawned by | Owns |
|---|---|---|---|
| client (presentation) | `ce/cli.ts` -> `ce/commands.ts:67-82` (`pi client`) | user | TUI or one-shot prompt; `pi-client` `Client` over a Unix socket (or Radius relay) |
| coordinator | `ce/coordinator-entry.ts`, `ce/coordinator.ts:301-328` | server (`ensureCoordinator`, `ce/coordinator.ts:187-199`) | the stable **public** socket and a **control** socket; pure byte/JSON-line router, "never interprets Pi, session, worker, or lifecycle payloads" (`ce/coordinator.ts:260-261`) |
| server | `ce/server.ts:768-809` (`runServerProcess`) or foreground `startForegroundServer` (`ce/server.ts:721-740`) | client auto-activation (`activateServer`, `ce/server.ts:167-220`) or `pi server` | session catalog, server-scoped services, `SessionWorkerManager`, plugin builds, Radius relay host |
| session-worker | `ce/session-worker.ts:833-843` | server (`SessionWorkerManager.#launch`, `ce/session-worker-manager.ts:449-503`) | one Session: proper-lockfile on its dir, the durable `Harness` + SQLite, all session-scoped services |

**Spawning** (`ce/process.ts`): `spawnInternalProcess(role, args, {env})` (`:41-71`) runs `process.execPath` with `[--import source-resolver.ts]? <entry> ...args` (Bun binary: args only), `detached: true`, `stdio: "ignore"`, `cwd: process.cwd()`, env `__PI_INTERNAL_SPAWN=<coordinator|server|session-worker>` (`:6,8,59-64`), then `child.unref()`. The child validates and deletes the env var (`consumeInternalProcessRole`, `:29-33`) so grandchildren don't inherit it. `terminateInternalProcess` = SIGKILL + wait for exit (`:74-82`). Entry resolution: `coordinator.{ts,js}`, `server.{ts,js}`, `session-worker.{ts,js}` next to `process.ts`, or `dist/bundle/{coordinator,cli}.js` when bundled (`:84-98`). `source-resolver.ts` is a Node `registerHooks` resolver mapping `@earendil-works/*` tsconfig paths to workspace sources (`ce/source-resolver.ts:23-62`).

Arguments:
- coordinator: `[publicPath, controlPath]` (`ce/coordinator.ts:303-306`).
- server: `[serverDir, serverId, sessionDir, JSON{provider?,model}?]`, all absolute; extra args rejected (`ce/server.ts:768-783`, model parse `:742-765`).
- session-worker: one JSON arg `SessionWorkerOptions = {sessionDir, metadata:{id,createdAt,cwd,path}, provider?, model?, pluginManifestPaths: string[]}` (strict TypeBox, `ce/session-worker.ts:70-84`; abs-path checks `:752-760`) plus env `PI_SESSION_WORKER_CONTROL_ADDRESS` (coordinator control socket), `PI_SESSION_WORKER_CONTROL_TOKEN` (random UUID per launch), `PI_SESSION_WORKER_SESSION_KEY_BASE64` (base64url of the Session dir path), `PI_SESSION_WORKER_PEER_ID` (`worker-<uuid>`) (`ce/session-worker.ts:65-68`, set at `ce/session-worker-manager.ts:455-471`). Test-only env: `__PI_SESSION_WORKER_INITIAL_DEMAND_GRACE_MS`, `__PI_SESSION_WORKER_ORPHAN_DEMAND_GRACE_MS` (`ce/session-worker.ts:308-311`).

**Paths / sockets / files** (server dir default `~/.pi/server`, override `PI_SERVER_DIR`; `ce/server.ts:53-58`):
- Server dir created `0700`, must be owned by current uid (`ce/server.ts:60-69`). This is the only "auth" for local clients: filesystem permissions.
- `default-server-id` (UUIDv4, created `wx` mode `0600`) unless `--server-id`/`PI_SERVER_ID` (`ce/server.ts:98,106-133`). ServerId must be canonical lowercase UUIDv4 (`protocol/src/protocol.ts:12-19`).
- `<serverId>.sock` = **public** socket, owned by the coordinator (`server/src/transports/unix/address.ts:4-9`, `ce/server.ts:621`); chmod `0600` (`ce/coordinator.ts:570-572`).
- `control-<serverId>.sock` = coordinator control socket (`ce/server.ts:622`).
- `server-<serverId>-<12hex nonce>.sock` = the replaceable server's private listener (mode `0600`); the coordinator pipes each public connection to it (`ce/server.ts:623-624,484-489`; `ce/coordinator.ts:444-473`).
- Lock files (proper-lockfile): `launcher-<serverId>` (profile lock, stale 30 s, `ce/server.ts:95-97,135-146`), `activation-<serverId>` (serializes auto-activation vs foreground start, stale 20 s, `:222-235`).
- Plugin state: `plugin-packages-<serverId>.json`, `plugin-builds/<serverId>/<pkg>/` (+ facet bundle manifest), `session-plugin-packages-<serverId>-<sha256(sessionPath)[:24]>.json` (`ce/plugins/package.ts:26,75-76,160-163`).
- Session dir default `<agentDir>/experimental/sessions` (`ce/server.ts:91-93`; agentDir `~/.pi/agent`, `coding-agent/src/config.ts:605`).

**Coordinator control protocol** (newline-delimited JSON, each line `<= 128 MiB`, `ce/process.ts:100-106`; `COORDINATOR_PROTOCOL_VERSION = 3`, `ce/coordinator.ts:14`):
- server -> coord: `{type:"register_server", protocol:3, serverConnectionId, endpoint}` (`:91-96`); then `{type:"send", to:peerId, payload}` / `{type:"broadcast", payload}` (`:100-106`).
- coord -> server: `server_registered{serverConnectionId, peers[]}`, `server_replaced`, `peer_connected{peerId}`, `peer_disconnected{peerId}`, `message{from, payload}` (`:18-28`).
- worker -> coord: `{type:"register_peer", protocol:3, peerId}` then `{type:"send", to:"server", payload}` (`ce/session-worker.ts:347,362`).
- coord -> worker: `peer_registered{peerId, serverConnectionId?}`, `server_connected{serverConnectionId}`, `server_disconnected{serverConnectionId}`, `message{from:"server", payload}` (`ce/session-worker.ts:313-322`; coord side `ce/coordinator.ts:372-418`).
- A new `register_server` **replaces** the old server: public client connections are dropped, peers get `server_disconnected`/`server_connected`, the old server gets `server_replaced` (`ce/coordinator.ts:382-395`). Only the current server may broadcast (`:434-440`). Duplicate or `"server"` peerId is a protocol error (`:405`).
- Coordinator exits after 30 s with nothing registered at startup, or 250 ms after it becomes completely empty (`:263-264,483-500`); refuses to start if the socket path is live (`:574-596`).

**Server <-> worker payloads** (carried inside coordinator `message`; strict TypeBox):
- Commands (`ce/session-worker.ts:117-134`): `shutdown`; `discover_workers`; `session_demand{serverConnectionId, requestId, attachmentId, attached:boolean}`; `operation{requestId, scope:{serverConnectionId, attachmentId}, call: ServiceCall}`; `operation_cancel{requestId, scope}`.
- Events (`:136-182`), every one carrying the launch `token` and `sessionKey`: `worker_ready{sessionId, pid, metadata, pluginManifestPaths}`, `worker_failed{message}`, `demand_applied{requestId, attachmentId, attached}`, `demand_rejected{requestId, message}`, `operation_response{response: operation_result{requestId, scope, result?} | operation_error{requestId, scope, code?: RemoteServiceErrorCode, message}}`, `service_update{scope, subscriptionId, update}`.
- The manager drops any event whose `peerId/token/sessionKey/scope` does not match the record (`ce/session-worker-manager.ts:537-560,580-612,614-639,641-667`). This token+peerId pair is the worker "auth".

**Server lifecycle**: `startServer` (`ce/server.ts:532-718`) = lock profile -> restore plugin profile + build -> `ensureCoordinator` -> `SessionWorkerManager` -> `startServerBackend` (Unix server on the nonce socket) -> `coordinator.connect()` (register) -> `workers.discover(peerIds)` (re-adopt surviving workers after a server restart, 5 s timeout, `ce/session-worker-manager.ts:151-173`) -> refresh directory -> Radius relay. On `coordinator.replaced` the server detaches workers *without* stopping them and closes (`ce/server.ts:660-668`). `ServerLifetime` (`:270-343`): auto-activated servers (`keepAlive:false`) retire 1 s after connection count and worker count both hit 0, with a 10 s startup hold; foreground servers keep alive.

**Client auto-activation** (`ce/server.ts:167-220`): take `activation-<id>` lock, try to connect; if nothing (ENOENT/ECONNREFUSED/..., or hello `version` mismatch, `:237-264`) spawn a server process and poll-connect every 10 ms up to 10 s; on failure SIGKILL the child. Client discovery: `discoverUnixServers({directory})` lists `*.sock` (`ce/client-runtime.ts:79`); a `version` error on a discovered socket triggers re-activation (`:134-150`). `--connect <path>` must end in `<uuid>.sock` (`:225-230`); `--connect radius:<serverId>` uses the relay transport with token auth (`ce/radius-auth.ts`, `ce/radius-relay.ts`, not audited in depth) and auto-reattaches via `SessionManagement.attach` after reconnect (`ce/client-runtime.ts:156-169`).

**Worker lifecycle**:
- Launch (`ce/session-worker-manager.ts:449-503`): one pending launch per Session dir; 15 s readiness timeout (`:31`) else SIGKILL; `worker_ready` must echo pid, token, and the same plugin manifest list (`:641-693`).
- Worker startup (`ce/session-worker.ts:510-595`): connect control, **lock `metadata.path` with proper-lockfile** (`realpath`, stale 2 s, update 1 s, retry up to 8 s, `:518-523`), open Harness on `<dir>/session.sqlite` (`createCodingAgentHarness`, `:774-831`: `ModelRuntime.create()`, `SettingsManager.create(cwd)`, coding registry + `Subagent`, `Harness.open(openNodeSqliteStorage)`; initial model only when the root conversation is new, `:798-813`), subscribe to `harness.taskGraph`, build services, `harness.resume()` (continue interrupted turns, `:595`), send `worker_ready`. Failures send `worker_failed` (`:762-771`).
- `WorkerLifecycle` (`:185-306`) retires the worker (close + `process.exit(0)`) when **all** hold: initial-demand grace (10 s) elapsed or a demand arrived, no in-flight requests/demand handlers, Harness task graph empty (`setHarnessActive`, `:593`), no attachment demand left. Demands from a disconnected server generation become orphans and expire after 30 s (`:225-236`), so a worker survives a server restart and is re-adopted via `discover_workers`.
- Stop: `shutdown` command, 10 s grace, then SIGKILL (`ce/session-worker-manager.ts:372-401`). Unexpected exit/disconnect resolves `RoutedSessionHandle.terminated` with an error (`:695-712,505-521`); the router then invalidates the hosted Session and releases client attachments (publishing `attachment:null`, `server/src/session-router.ts:279-312`). **No automatic restart**: the next `attach` re-opens (spawns) the worker.
- Server-side worker operations have **no wall-clock timeout** (`ce/session-worker-manager.ts:317`); demand updates time out after 5 s and are compensated by a detach demand or worker kill (`:279-315,746-775`).
- Plugin-selection conflict: attaching to a running worker with a different plugin manifest list throws `SessionPluginSelectionConflictError` -> `service_invalid_value` (`:136-149`; `ce/server.ts:453-469`).

**Session catalog** (`ce/session-catalog.ts`): one dir per Session under the session dir, name = Session id (`/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/`, `:17`), containing `meta.json` = `{createdAt:number(ms), cwd:string}` (tab-indented, `:73-76`) and worker-owned `session.sqlite` (`:15-16`). `listSessions` skips entries without valid meta (`:29-55`); `createSession` mkdirs (EEXIST -> "Session <id> already exists"), id defaults to `randomUUID()` (`:58-78`); `deleteSession` = `rm -rf` (`:81-83`). The server merges live worker records into the list (`ce/server.ts:405-409`) and resolves new-Session `cwd` (absolute, existing dir, realpath'd; else `service_invalid_value`, `ce/server.ts:76-89`).

### 4.2 Transport: client <-> server wire, Chord services

- **Framing**: 4-byte big-endian length prefix + CBOR payload, default max frame 16 MiB (`protocol/src/framing.ts:6,28`; `protocol/src/codec.ts:3,56-61`).
- **Handshake**: client's first frame `{type:"hello", version}`; `PROTOCOL_VERSION = 8` (`protocol/src/protocol.ts:5,29-33`). Server replies `{type:"hello", version:8, serverId}` or `{type:"hello_error", error:{code:"version"|"invalid_request", message}}` (`:65-73`; `server/src/server.ts:262-296`). 5 s handshake timeout (`server/src/server.ts:42`). During handshake the server calls `serverServices.attachClient(presentation)` creating that connection's server-scoped service endpoint (`:272-286`). No credential exchange on Unix.
- **Client -> server**: `request{id, target, call}`, `cancel{id, target}` (`protocol/src/protocol.ts:49-62`). `target` = `{serverId}` (server services) or `{serverId, sessionId, attachmentId}` (session services) (`:36-47`). Client ids are `request-<n>` (`client/src/client.ts:245`).
- **Server -> client**: `response{id, ok:true, result?}` | `response{id, ok:false, error:{code,message}}`, `service_update{subscriptionId, update}`, `attachment{attachment: SessionTarget|null}` (out-of-band route change after attach/detach/worker death) (`protocol/src/protocol.ts:74-104`; `server/src/session-router.ts:192-196,254-260`).
- **Error codes on the wire**: Chord `service_not_allowed | service_not_found | service_mode_mismatch | service_member_not_found | service_member_mismatch | service_instance_not_found | service_stale_instance | service_invalid_value` (`chord/src/services/errors.ts:1-10`); server `wrong_server | session_not_found | session_ambiguous | session_not_attached | server_draining` (`server/src/errors.ts:3-9`); protocol `invalid_request | version | cancelled | internal_error` (message "Internal server error") (`server/src/server.ts:306-404,512-521`). Any other thrown error (e.g. `"Unknown model: ..."`) becomes `internal_error` at the server boundary unless it carries one of these codes; inside the worker hop, an error with a known `code` is preserved, others become plain `"Session worker operation failed: <msg>"` (`ce/session-worker.ts:616-635`; `ce/session-worker-manager.ts:603-608`).
- **Service definition**: `defineService<T>(id, {local?})` returns frozen `{id, local}`; ids starting `$chord.` reserved (`chord/src/api.ts:73-85`). Remote contracts are type-checked: every member is either `ReplicatedState<Json>` or `(...jsonArgs, context: Context) => Promise<Json|void>` (`chord/src/types.ts:164-182`). `local:true` services (`pi.local.*`) never enter an RPC catalogue (`chord/src/services/provider.ts:467-469` (error `service_not_allowed` "Service X is process-local")).
- **Call shape**: `ServiceCall = {serviceId, instance?:{key, generation>=1}, member, args: Json[]}` (`chord/src/types.ts:236-242`); the `Context` arg is dropped on the wire, cancellation travels as `cancel`.
- **Control calls** on pseudo-service `$chord.service`: `catalogue()` -> `[{serviceId, mode:"singleton"|"keyed"}]`; `subscribe(subscriptionId, serviceId, mode)` -> snapshot; `unsubscribe(subscriptionId)` (`chord/src/services/wire.ts:40-88`). The server installs a per-subscription path-dictionary encoder (`server/src/server.ts:358-367`).
- **Replicated state stream**: snapshot = `{serviceId, mode, instances:[{instance?, members:[{name, kind:"method"} | {name, kind:"state", sequence>=0, ops:[["r", value]]}]}]}`; updates `ServiceProviderUpdate` = `state{instance?, member, sequence>=1, ops: Op[]}` | `reset{snapshot}` (rebaseline after 100 buffered deliveries, `chord/src/services/provider.ts:453-457`) | `unavailable` | `replaced{snapshot}` | `spawned{instance}` | `closed{instance}` (`chord/src/types.ts:221-235`; wire forms `chord/src/services/wire.ts:26-38`). `Op` = `["r",v] | ["s",path,v] | ["d",path] | ["a",path,str] (string append) | ["t",path,n] (truncate) | ["p",path,start,deleteCount,items] (splice) | ["m",path,perm] (reorder)` (`chord/src/delta/index.ts:32-40`); `WireOp` adds `["#", id, path]` path interning, numeric path refs, and path-omitting short tuples (`:42-66`). Consumers serialize listener delivery; >100 pending deliveries collapse to the newest (sequence may skip) (`chord/src/types.ts:43-57`).
- **Facet host**: providers are facets (`{id, setup(env)}`) using `env.provide/use/observe/provideMany/replicatedState/own/onActivate` (`chord/src/types.ts:281-307`); `FacetHost.services` is a `RemoteServiceProvider`, `reload(facets)` hot-swaps same-shape facets (`:325-330`).
- **Server routing**: `RoutedServerServiceAttachment.invokeService(call, publish, ctx)` for server services; `RoutedSessionAttachment` for the selected Session (`server/src/types.ts:22-68`). `requireAttachment` enforces `target.sessionId/attachmentId == current` else `session_not_attached` (`server/src/session-router.ts:224-231`). Each attach mints a new `attachmentId` (UUID), releases the previous attachment, opens the worker via `host.openSession` (`:160-197`).
- **Client service sources** (`svc/connection.ts`): `ServerServiceSource.connection: ReplicatedState<{status:"connecting",attempt} | {status:"connected",since} | {status:"disconnected",since,reason,retryAt:null}>` and `SessionServiceSource.attachment: ReplicatedState<{status:"detached"} | {status:"attaching"|"attached"|"degraded", sessionId}>` (`:18-39`) are **local** control states. Session bindings rebind on every `attachment` envelope, guarded by an `#attachmentRevision` counter + exact `{serverId,sessionId,attachmentId}` comparison (`:212-258,384-389`); `whenAttached(sessionId)` / `whenDetached()` wait for rehydration (`:293-304`).

### 4.3 Stale-guarding / generation fields (summary)

| Field | Where | Guards |
|---|---|---|
| `serverConnectionId` (UUID per server process registration) | `ce/coordinator.ts:59`; worker `WorkerLifecycle.beginRequest/setDemand` reject "stale server generation" (`ce/session-worker.ts:238-277`) | requests/demand from a replaced server |
| `attachmentId` (UUID per client attach) | `SessionTarget`; router `session_not_attached`; worker demand key `(serverConnectionId, attachmentId)` | calls after detach/re-attach |
| worker `token` + `peerId` + `sessionKey` | `ce/session-worker-manager.ts:43-55,537-639` | spoofed/stale worker events |
| `instance {key, generation}` | `chord/src/types.ts:201-204`; error `service_stale_instance` | keyed instances (e.g. `pi.subagent-transcript`) |
| state `sequence` per member | `chord/src/types.ts:221-228` | gap detection; `reset` rebaseline |
| `SessionDirectoryState.revision`, `ModelsState.catalog.revision` | `svc/server.ts:39-54`; `svc/models-provider.ts:51-66` | monotonic counters for consumers |
| `ProviderAuthState.attempt`, `challenge.id` | `svc/provider-auth-provider.ts:69-73,277-292` | answers to superseded sign-in attempts |
| `AgentPromptRequest.requestId` | `svc/agent-controller.ts:12-18` | idempotent prompt/steer/follow-up retry |

### 4.4 Service catalogue

Scopes: **server** services are provided per client connection by `createExperimentalServerServices` (`svc/server.ts:27-139`, assembled in `ce/server.ts:431-473`); **session** services are facets in the worker's `FacetHost` (`svc/worker.ts:54-159`), one `RemoteServiceEndpoint` per `(serverConnectionId, attachmentId)` scope (`:120-142`); **presentation** services are `local:true`, in-process in the client TUI. All methods also take a trailing `context: Context` (omitted below). Server-scope mutations are serialized through one promise tail (`svc/server.ts:55-63`).

#### `pi.session-directory` - SessionDirectory (server, singleton) - `svc/sessions.ts:21-30`
- `state: ReplicatedState<{revision:number; sessions: SessionSummary[]}>`, `SessionSummary = {serverId, sessionId, createdAt:number, cwd:string}` (`:4-13`).
- Impl `svc/server.ts:39-54`: one shared `replicatedState` for all clients; `revision` starts 1, +1 on each refresh after create/remove/startup. Sorted by sessionId then createdAt (`ce/server.ts:442-445`).

#### `pi.session-management` - SessionManagement (server) - `svc/sessions.ts:32-39`
| Method | Params -> Result | Impl / errors |
|---|---|---|
| `create` | `{id?:string; cwd?:string}` -> `SessionSummary` | `createSession` in catalog + directory refresh (`svc/server.ts:97-102`; `ce/server.ts:417-420,446`). Errors: `service_invalid_value` "Session cwd must be an absolute path" / "...is not an existing directory"; "Invalid session ID"; "Session <id> already exists". Does **not** start a worker. |
| `remove` | `sessionId` -> void | `presentation.prepareSessionRemoval` (release routed attachments) -> `workers.closeSession` -> `rm -rf` dir -> remove plugin profile -> refresh (`svc/server.ts:103-108`; `ce/server.ts:447-452`). `session_not_found`. |
| `attach` | `sessionId` -> void | `presentation.attachSession` -> router opens/spawns worker, sends `session_demand attached:true`, emits `attachment` envelope (`svc/server.ts:109-112`; `server/src/session-router.ts:160-197`). Client wrapper also awaits `whenAttached` (`ce/client-runtime.ts:208-211`). |
| `detach` | () -> void | releases attachment (demand `attached:false`), clears prepared plugin paths (`svc/server.ts:113-117`). |

#### `pi.presentation-plugins` - PresentationPlugins (server) - `svc/plugins.ts:4-12`
- `prepareSession({sessionId, packagePaths: string[]|null})` -> `JsonValue` (`{<bundlesKey>: FacetBundleArtifact[]}`, `ce/plugins/bundled.ts:32-36`). Builds/selects plugin packages for that Session (null = stored or server default), persists the selection, checks worker conflict (`ce/server.ts:453-469,573-602`). Must precede `attach` when selecting plugins (`ce/client.ts:66-73`).
- `reload()` -> `JsonValue` (rebuilt TUI artifacts); error "No Session plugin selection is prepared" (`svc/server.ts:88-94`).

#### `pi.provider-auth` - ProviderAuth (server, per connection) - `svc/provider-auth.ts`
- `state: ReplicatedState<ProviderAuthState>`: `{credentialsKnown, credentialLookupFailed, hasExistingCredentials, existingProviders: SignInProvider[], attempt:string|null, provider, status:"idle"|"connecting"|"waiting"|"prompt"|"done"|"failed", message, url:string|null, deviceCode:string|null, challenge: SignInChallenge|null}` (`:14-28`). `SignInProvider = "anthropic"|"openai-codex"` (`:4`). `SignInChallenge = {id, type:"text"|"secret"|"select"|"manual_code", message, placeholder?, options?:{id,label,description?}[]}` (`:6-12`).
- `start(provider)` -> attempt id (string); returns before interaction. Runs `ModelRuntime.login(provider,"oauth",{signal,prompt,notify},{getDeviceId})` in background. `auth_url` event -> `status:"waiting", url`; `device_code` -> `url=verificationUri, deviceCode=userCode`; `info/progress` -> `message`; prompts -> `status:"prompt", challenge` (`svc/provider-auth-provider.ts:80-142,145-203`). Success -> `done` "Connected. Choose a model to continue."; any error -> `failed` "Could not complete sign-in. Try again." (provider error text deliberately never replicated, `:189-197`).
- `reuse(provider)` -> attempt id; verifies a saved OAuth credential and refreshes with `allowNetwork:false`, checks `getProviderAuthStatus().configured` and that models exist (`:204-254`).
- `answer(attempt, challengeId, response)` -> void; response <= 8192 chars, select must be an option id; errors "The sign-in question has expired", "The selected option is not available" (`:277-292`).
- `cancel(attempt)` -> void (no-op if stale) (`:293-308`). `remove(provider)` -> void: `runtime.logout(provider)`; error "Could not remove the saved connection. Try again." (`:255-276`).
- Common errors: "Wait for connection removal to finish", "Sign-in is not available", "This provider is not available for sign-in". **OAuth only; no API-key flow in this service.** Credentials live in Pi's auth storage (`~/.pi/agent/auth.json`, `coding-agent/src/config.ts:625`, via `DefaultAuthStorage`, `coding-agent/src/core/model-runtime.ts:216`); device id from `SettingsManager.getOrCreateDeviceId()` (`ce/server.ts:437-440`). The auth runtime is lazily created in the server process, not the worker (`ce/server.ts:427-441`).

#### `pi.session-plugins` - SessionPlugins (session) - `svc/plugins.ts:15-19`
- `reload()` -> void: reloads plugin facets via `facetHost.reload(candidate.facets)`, disposes retired generation; serialized (`svc/worker.ts:68-117`). Before ready: "Session plugins are not ready".

#### `pi.models` - Models (session) - `svc/models.ts`
- `state: ReplicatedState<ModelsState>` = `{catalog:{revision, availableModels: {provider, modelId, name, reasoning:boolean}[]}, configuration:{model:{provider,modelId}|null, thinkingLevel}, refresh: {status:"idle"|"refreshing"|"done"} | {status:"warning", errors: Record<string,string>}}` (`:4-24`).
- `ModelThinkingLevel` = `off|minimal|low|medium|high|xhigh|max` (descriptions `svc/slash-commands-provider.ts:9-17`).
- `select({provider, modelId})`: `ModelRuntime.getModel` else "Unknown model: p/m"; clamps current thinking level; `conversation.configure({model, thinkingLevel})` (durable `pi.agent` doc); **also persists as the global default** via `settingsManager.setDefaultModelAndProvider` + `flush()` (`svc/models-provider.ts:93-103`).
- `selectThinking(level)`: must be in supported levels else "Thinking level X is unavailable; choose one of: ..." (`:104-110`). `cycleThinking()`: next supported level (`:69-74`). `getThinkingLevels()` -> `ModelThinkingLevel[]` (`["off"]` when no model; `getSupportedThinkingLevels(model)`, `:47-50`).
- `refresh()`: `refresh.status="refreshing"`, `modelRuntime.refresh({signal})`, then new catalog (revision+1) and `done` or `warning{errors}` (`:78-92`).
- `configuration` is synced from the conversation's `pi.agent` DocumentState subscription, so changes by other clients/compaction propagate (`:121-134,146-163`). Catalog = available snapshot plus the selected model if missing (`:51-66`).

#### `pi.agent-controller` - AgentController (session) - `svc/agent-controller.ts`, impl `svc/agent-controller-provider.ts`
Types: `AgentPromptRequest = {message:string; images: {type:"image", data(base64), mimeType}[] | null; requestId?: string|null}` (`:3-19`). `AgentOperationResponse = {accepted:true, operationId, error:null} | {accepted:false, operationId:null, error:{code,message}}`; `AgentQueueResponse` same with `entryId` (`:21-34`). Error codes: `"busy"` (durable `ConversationBusy`, message "Conversation <id> is busy", `durable/src/errors.ts:20-26`) or `"operation_failed"` (`svc/agent-controller-provider.ts:118-121`).

| Method | Result | Durable call |
|---|---|---|
| `prompt(req)` | `AgentOperationResponse`; rejected `busy` while a run is active | `conversation.submit({type:"input", content, whenBusy:"reject", requestId?})`; `operationId = String(submission.id)` (`:37-47`) |
| `steer(req)` | `AgentQueueResponse` | same with `whenBusy:"steer"` (inject into active run, or start one when idle) (`:20-34,48`) |
| `followUp(req)` | `AgentQueueResponse` | `whenBusy:"followUp"` (queue after the active run, or start one) (`:49`) |
| `cancelQueued(entryId)` | `{outcome:"cancelled"|"already_consumed"|"not_found"}` | `harness.abortSubmission(id, ctx, conversation.id)`; non-positive-integer id -> `not_found` (`:50-57,102-105`) |
| `abort()` | void | `conversation.abort(ctx)`: withdraws queued input, aborts run + compaction (`:58`) |
| `compact({customInstructions:string|null})` | `AgentOperationResponse`; `operationId` = compaction **task id** | `conversation.compact(instr, ctx)` (`:76-83`) |
| `lookup(requestId)` | `{found:false}` \| `{found:true, operationId, status:"queued"|"placed"|"done"|"unanswered", reason: "aborted"|"model_error"|"no_model"|"stale"|null, detail:string|null}` | `harness.submissionByRequest(...)` + `submission.status()` (`:59-75`) |
| `waitForPrompt(operationId)` | `{status:"done", text, reason:null}` \| `{status:"unanswered", text:null, reason}` | `harness.submission(id).wait()`; answer entry's assistant text blocks concatenated; unknown id throws "Unknown prompt: <id>" (`:84-98`) |

Images: content becomes `[{type:"text",text}, ...images]` else plain string (`:112-116`). Model/thinking change is **not** on the controller; it is `pi.models`. Dropped vs the old harness: `navigate()`, `nextRun()`, `resume()` (`svc/README.md:42-47`). The TUI chooses prompt vs steer by `liveOf(view).run !== undefined` (`ce/client-tui.ts:563-572`), follow-up has its own key (`:574-587`), Esc calls `abort` only while running (`:597-610`).

#### `pi.transcript` - Transcript (session) - `svc/transcript.ts`
- `state: ReplicatedState<ConversationView>` where `ConversationView = {conversation: ConversationRecord; entries: EntryRecord[] (active context: head marker then entries since newest reset/compaction); docs: Record<string, JsonObject>}` with docs `pi.live`, `pi.inbox`, `pi.agent`, `pi.usage` (+`pi.provider`) (`durable/src/harness/view.ts:26-32`). `pi.agent` = `AgentState {model?, thinkingLevel?, extensions?, tools?, instructions?, cwd?}` (`durable/src/harness/types.ts:306-319`).
- Impl: `conversation.viewState(ctx)` is an `AttachedReplicatedState` fed by the Harness's exact per-commit ops; the worker keeps no reducer (`svc/transcript-provider.ts:6-15`; `svc/README.md:30`). Streaming, tool progress, queue, retries, compaction status, usage all arrive through this state.

#### `pi.history` - History (session) - `svc/history.ts`
- `page({before: number|null, limit:number})` -> `{entries: JsonValue[] (raw EntryRecords, newest first), more:boolean}`; `limit` clamped 1..200 (`HISTORY_MAX_PAGE`, `:30`); `before` must be a positive safe int else "history.page: before must be a positive entry id or null"; `before===1` -> empty (`svc/history-provider.ts:8-23`). Uses `conversation.entries({maxEntryId: before-1}, limit)`; covers the whole history past resets/compactions.

#### `pi.ui-requests` - UiRequests (session) + `pi.local.ui-request-host` - UiRequestHost (session-local)
- `state: ReplicatedState<{requests: UiRequest[]; status: Record<string,string>; notices: UiNotice[]}>` (`svc/ui-requests.ts:35-41`). `UiRequest = {id:"q<n>", kind:"select"|"confirm"|"input", title(<=300), message(<=2000)|null, items:{value,label,description|null}[]|null, placeholder|null, defaultValue|null, createdAt(ms), deadline(ms)|null}` (`:3-26`). `UiNotice = {id:"n<n>", level:"info"|"warning"|"error", message(<=500), at}` (`:28-33`).
- `respond(id, value: Json)` -> `{accepted:boolean, reason:string|null}`: confirm needs boolean ("A confirmation needs yes or no."), others string ("An answer must be text."), select must match an item value ("That is not one of the choices."), input <= 10000 chars, unknown id "That question is no longer open." (`svc/ui-requests-provider.ts:156-171`). `cancel(id)` -> `{cancelled:boolean}` (`:172-177`).
- Host side (facets inside the worker): `select(title, items, {message?, timeoutMs?})` -> `string|undefined`, `confirm(title, opts)` -> `boolean|undefined`, `input(title, {placeholder?, defaultValue?, ...})` -> `string|undefined`, `notify(level, msg)`, `setStatus(key(<=80), text(<=300)|null)` (`svc/ui-requests.ts:73-89`; `svc/ui-requests-provider.ts:122-152`).
- Flow/timeouts: default 120 s (`UI_DEFAULT_TIMEOUT_MS`), `timeoutMs:0` waits forever; timeout posts info notice `No answer to "<title>"; it was cancelled.` and resolves `undefined`; caller abort also resolves `undefined`; max 20 pending (else warning notice and immediate `undefined`), max 20 notices FIFO (`svc/ui-requests.ts:93-97`; `svc/ui-requests-provider.ts:65-120`). Facet disposal cancels all (`:183-186,196`). First responder wins; no per-client ownership.

#### `pi.subagents` / `pi.subagent-transcript` (session) - covered elsewhere
`Subagents`: `activity: ReplicatedState<TaskGraph>`, `configuration: ReplicatedState<{enabled}>`, `list()`, `view(conversationId)`, `open(conversationId)` (mounts keyed `pi.subagent-transcript` instance), `page(conversationId, HistoryPageRequest)`, `setEnabled(enabled)`; `SubagentTranscript.state: ReplicatedState<ConversationView>` keyed by child id (`svc/subagents.ts:6-33`; impl `svc/subagents-provider.ts:15-160`).

#### `pi.local.slash-commands` - SlashCommands (presentation-local) - `svc/slash-commands.ts`
- `register(cmd)` (throws "Slash command /x is already registered"), `replace(cmd)` (stack same-name for hot reload), `list()`, `subscribe(listener)`; returns disposer (`svc/slash-commands-provider.ts:24-75`). Name regex `^[a-z0-9][a-z0-9:-]*$`.
- `SlashCommandContribution = {name, description?, argumentHint?, getArgumentCompletions?(prefix) -> {value,label,description?}[]|null, run(args, ctx) -> AgentOperationResponse | AgentQueueResponse | undefined}` (`svc/slash-commands.ts:4-20`).
- Dispatch: `/name args` split on first space; unknown -> status "Unknown slash command: /name"; queue vs operation results rendered by the TUI (`ce/client-tui.ts:522-560`).
- Built-ins (`svc/slash-commands-provider.ts:86-214`):
  - `/model [provider/model|modelId]`: exact unique match (case-insensitive) else "Unknown model: ..."; no arg -> `PresentationUI.select("Select model:")`; then `Models.select`; status "Selected p/m." Completions filter on `"p/m name"`.
  - `/thinking [level]`: validates against `getThinkingLevels`; no arg -> select with descriptions; `Models.selectThinking`; status "Thinking level: X."
  - `/compact [instructions]`: status "Compacting..." and returns `AgentController.compact` result.
  - `/reload`: `PresentationPlugins.reload` -> `SessionPlugins.reload` -> reload local presentation facets; status "Reloaded plugins."
  - Example plugin adds `/hello` (`svc/README.md:38`). The standalone durable TUI also has `/agents`, `/tasks` (`ce/durable/README.md:33-46`), not part of this service set.

#### `pi.local.presentation-ui` - PresentationUI (presentation-local) - `svc/presentation-ui.ts:3-20`
- `select(title, items:{value,label,description?}[], selectedValue|undefined)` -> `string|undefined`; `showStatus(message)` -> void. Implemented by the client TUI.

### 4.5 Non-interactive client flow (`ce/client.ts:21-86`)
`pi client` with no session/prompt lists `serverId\tsessionId` across discovered servers. With a prompt: create Session (exactly one server required) -> `PresentationPlugins.prepareSession` -> `SessionManagement.attach` -> `AgentController.prompt` -> `waitForPrompt` -> print text; non-accepted -> throws `error.message`; unanswered -> "Prompt was not answered: <reason>". `-c/-r` attach the newest Session (`svc/README.md:30`). Services bound per server: server `[SessionDirectory, SessionManagement, PresentationPlugins]`, session `[Models, AgentController, Transcript]` (`ce/client-runtime.ts:186-223`).

### 4.6 Standalone durable runtime (`ce/durable/runtime.ts`) - single-process reference
No sockets: one process opens `~/.pi/agent/experimental/durable-sessions/<cwd-hash>/<session>/session.sqlite` under a lock (`ce/durable/README.md:12-16`). Exposes `DurableView {session, conversation: ConversationView, conversations:{id,label,title?}[], models:{provider,modelId,name,contextWindow}[], notices:{id,level,message}[] (last 20), tasks?: TaskGraph}` and `DurableController {submit(text, whenBusy:"steer"|"followUp"), compact(instr?), abort(), cycleThinking(), setModel(ref), toggleTasks(), switchConversation(id)}` (`:52-79`). Commands are serialized through one promise queue except `abort` (`:227-232,282`). Compaction outcome messages via `waitForTask` (`:253-280`): "Compacted.", "Compaction summary queued; it is placed at the next turn boundary.", "Compaction summary dropped: the context changed under it.", "Nothing to compact: ...", "Compaction aborted.", "Compaction <status>: <error>". `cycleThinking` errors "Current model does not support thinking" / "No model selected" (`:243-291`). Unanswered submissions (except `aborted`) raise "No answer: <reason> <detail>" (`:233-242`). It calls `harness.resume()` on open (`:332`) and close writes no outcome, so `--continue` resumes the turn (`:352`).

### 4.7 Port notes (observations)
- The whole stack is three IPC layers: CBOR-framed client protocol -> coordinator byte pipe -> JSON-line coordinator routing to workers. A Rust port can collapse coordinator+server if hot server replacement is not needed; keep `(serverConnectionId, attachmentId)` scoping and the worker token checks.
- Chord's replicated-state op format (`r/s/d/a/t/p/m` + wire path interning) is the transcript streaming contract; `Transcript` is just the durable Harness's `viewState` ops forwarded unchanged.
- Model selection writes both the conversation's durable `pi.agent` doc and the global `settings.json` default. Thinking levels are model-derived.
- Auth is OAuth-only (anthropic, openai-codex) and lives in the server process via `ModelRuntime`; workers read the same `auth.json`.
