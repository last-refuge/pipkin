# Desktop Local CLI setup handoff

Public API: `pipkin_core::local_cli::*` (also re-exported at crate root). No engine-protocol dependency. `AppState.local_cli` starts unavailable; native adapter emits `LifecycleEvent::LocalCliSetupAvailable(bool)` from the current connection's advertised capability; the controller applies `AppState::set_local_cli_available`. This is not a conversation execution grant. UI uses **only `AppState::local_cli_availability()`** for setup controls, alongside normal `availability()` for conversation controls. Metadata verification never grants native capabilities or clears `execution_blocked`.

Approval core/adapter port: see `CLI_APPROVAL_API.md`. It is separate from this metadata setup
flow. Command/backend/event routes, native RPC mapping and captured GPUI review controls are
implemented, including separately default-off current-grant coding admission and integrated
fake actor tests. Installed CLI/desktop qualification remains. Setup alone never grants
permission to deliver a tool decision.
Protocol 39 projects stored `SessionSummary.localCliMode` into distinct `LocalCliChatOnly` and
`LocalCliRestrictedWrite` profiles. Both keep fixed-model, text-only explicit prompts and no
native tools/automation; readiness remains independent. Missing/unknown policy fails closed.
Both may be reapproved, but a different mode still requires a fresh conversation.

## Conversation capability handoff (review 47)

Native in-place model/effort and attachment capabilities use the pure **`AppState::execution_capabilities()`** query (`ExecutionCapabilities { model_settings, attachments }`). These are profile capabilities, not transient readiness: combine with existing availability/read-only state for enabled controls. Do not duplicate profile comparisons in UI. Both fields are false for CLI chat, unsupported profiles, no selection, and real conversations without an authoritative `ExecutionContext`. Demo retains simulated native capabilities. Native contexts retain model settings and attachments. Core also rejects adding attachments to restricted profiles; existing saved attachments remain removable and block CLI submission until removed.

Catalog entries, restored caches, `Opened`, and history/status events do not grant execution capabilities. The core records receipt of a generation-correct, operation-free `ExecutionContext`; before that, real conversation commands fail closed (including MCP Test/Reconnect). Adapters must send this context for native sessions too. Explicit CLI submissions journal and send `model: None`, irrespective of saved native model preferences; those preferences are not changed.

## Shared provider/model routing

`provider_models()` combines native catalog entries and typed CLI aliases; `ProviderModel` never
infers backend from string prefixes. `SelectProviderModel` uses the normal picker. Native model
changes await scoped authoritative `ConversationModel`; cross-provider/CLI-alias changes create
separate conversations without forwarding source text. Native creation selects only its correlated
request/session receipt, never the newest catalog row. CLI retains immutable model/mode/tools.

`BeginProviderSetup` configures/checks CLI only: it neither selects a provider nor creates a session.
Pending selections are namespace-isolated per-source preferences, not authority. Failed/cancelled
changes block only that source (including journal-delay release); `KeepConversationProvider` is an
explicit opt-out. Other conversations remain usable. `next_provider_model()` follows the current
conversation or its pending choice; only a blank workspace uses the last choice. Unknown identities
fail closed. The former global `ConversationBackend` switch is removed, with read-only migration.

Adapters publish generation/op-guarded `ConversationModel` after `ExecutionContext`; UI labels
use that authoritative identity. While CLI is open, fetch the global native model catalog without
applying its default to CLI. The open picker captures typed rows so catalog changes cannot retarget
a selected index. Native thinking/attachment restrictions remain separate from picker availability.

## Guided desktop setup (protocol 40)

`BeginGuided` captures the current project and last CLI model preference, then issues a guarded
`Discover` request. Engine-owned discovery is filesystem-only (saved choice, absolute PATH,
bounded known installation locations); it never runs a shell, manager, CLI or login probe.
One candidate proceeds to metadata-only Inspect; multiple candidates await explicit selection.
`Configure` invalidates prior tickets/consent before inspecting the edited scope. File-picker
results must match `LocalCliState::input_revision()` and the captured draft.

`ConfirmAndConnect { setup }` explicitly approves BOTH visible canonical paths for the exact
captured ticket. A nonpersisted one-shot continuation coordinates Check followed by one journaled
Create or Return. Only a correlated successful Checked event can consume it; Status lookup cannot.
Failure, cancellation, expiry, editing and disconnect retire the continuation. No prompt or Write
approval is generated. `BeginGuidedExisting` reads saved hints and inspects them, without trust or
CLI execution. Saved namespace-isolated executable/model/tool-mode preferences are suggestions,
not grants. The manual APIs below remain available for advanced/recovery clients.

## Routing contract

- Intent: `Command::LocalCli(LocalCliCommand)`.
- Effect: `BackendRequest::LocalCli { conversation, generation, op, request }`.
- Response: `BackendEvent { conversation, generation, op: Some(op), kind: EventKind::LocalCli(event) }`. Echo the request tuple unchanged, including failures/timeouts.
- New setups use reserved `ConversationId(0)` and a core-owned monotonic generation. Existing setups capture an explicit conversation ID and its attachment generation at `BeginExisting`; UI must supply its exact engine session ID, not infer one from selection. Every request gets a fresh setup operation ID. These ops are **not prompt/run ops**.
- Local CLI events are intercepted before ordinary conversation lookup, generation exemptions, and run-op matching. Wrong tuple, phase, ticket, root, or captured attachment results are ignored without consuming the pending operation. Exception: exact-ticket Revoke is cleanup, so its command and correlated success/failure receipt may use the captured attachment generation after the same conversation has been reopened. Current ownership is still required; the full pending `(conversation, generation, op, phase)` guard is unchanged. Check/Create and all other execution/setup results retain attachment freshness checks.
- Adapter must route reserved-ID effects/events even with no conversations or selected conversation. Never resolve an operation through whatever conversation is selected when its response arrives.
- **New adapter handshake:** `BackendRequest::RetireLocalCliInspect { conversation, generation, op }` is fire-and-forget cleanup, referring to the original Inspect tuple, not a fresh setup operation. Cancel (and Edit via cancellation) emits it if Inspect is pending before any ticket is known. Adapter should cancel that exact pending inspection and revoke a ticket if completion races retirement; do not emit a success receipt or infer that retirement undid a mutation. Late Inspect events are stale in core. The native adapter implements exact-tuple retirement and compensating revocation for racing results. It never retires a Create.

## Closed DTOs / engine mapping

`CliModel` is exactly `Haiku | Sonnet | Opus`. `CliMode` is `ChatOnly | RestrictedWrite | Unsupported` (default ChatOnly). Mode is captured at Inspect and checked on every setup reply. Unsupported modes cannot Inspect. Changing mode is an explicit Edit that retires the old inspection/ticket and clears both trust choices. All definitions are in `src/local_cli.rs`.

Protocol 37 introduced RestrictedWrite metadata setup. Protocol 39 projects its immutable
identity. A coding execution grant now additionally requires Linux and both default-off host
switches (`enable_local_claude_cli`, `enable_local_claude_cli_write`); chat opt-in alone is never
enough. Creation does not run a prompt, and policy-changing reapproval fails. Production
activation and installed CLI/desktop qualification remain pending.

| Core request | Native service | Success event |
| --- | --- | --- |
| `Discover { saved, project }` | DiscoverLocalCli | `Discovered(Vec<String>)` |
| `Inspect { mode, executable, target }` | InspectLocalCli | `Inspected(LocalCliSetupView)` |
| `Check { setup, trust_executable, trust_project }` | CheckLocalCli | `Checked(LocalCliSetupView)` |
| `Status { setup }` | LocalCliSetupStatus | `Status(LocalCliSetupView)` |
| `Revoke { setup }` | RevokeLocalCliSetup | `Revoked` |
| `Create { identity }` | CreateLocalCliSession(session, setup) | `Created(LocalCliCreated)` |
| `LookupCreation { identity }` | LookupLocalCliCreation(session, setup), **pure lookup** | `CreationLookup { identity, found: Option<LocalCliCreated> }` |
| `SessionStatus { session }` | LocalCliSessionStatus(session), **read-only** | `SessionStatus(LocalCliSessionView)` |

`LocalCliSetupView` mirrors only metadata: immutable mode, target, setup ID, canonical executable/project, qualified version, metadata stage, optional expiry/reason, plus the root reservation described below. Engine `execution_enabled` is deliberately not a local readiness grant. Map metadata stages explicitly; unknown stages must fail closed. `expires_at` is optional Unix **seconds**, on the injected AppState clock; use None if the engine exposes no absolute deadline. Backend expiry/not-found for a ticket maps to `Failed { phase, failure: Expired }`.

`LocalCliSessionView` holds the exact engine session ID and read-only canonical executable/project/mode/model/context-valid hints. Rendering these hints does not edit the draft, set trust, verify a ticket, or clear a conversation gate. UI may explicitly adopt hints through `Edit`.

All failures name `LocalCliPhase`; map transport errors/timeouts/lost acknowledgements to `Unknown`, definitive refusal to `Rejected`, identity collision to `Conflict`, and expired tickets to `Expired`. Do not map missing creation lookup to a failed create; a successful pure lookup returning None has its own event. Do not automatically retry any setup service call.

## Reservation and creation safety

Inspect's engine-generated, globally unique setup UUID is the source of restart-safe identity (not a core clock/counter). For a New target, the adapter supplies `root_candidate = Some(EngineSessionId(format!("cli-{}", setup)))` in every setup view. For Existing, supply None. Setup IDs must be nonempty ASCII alphanumeric/hyphen/underscore, at most 124 bytes; resulting root IDs are at most 128 bytes. Engine generation and collision checks remain authoritative. No OS I/O or probabilistic IDs are generated in core.

Core validates and freezes this reservation at Inspect. Create always uses that exact `(session, setup)` pair. The adapter must call the service with that exact root ID, propagate conflict (never choose another), and verify the returned summary ID is that root. Create/lookup success carries **both** the pair and the adapter's collision-checked `ConversationId` for that exact engine session. Never hash blindly if the native catalog mapper resolves collisions.

Core waits for that exact desktop ID in the catalog, then selects/opens it once. Completion and catalog may arrive in either order. Unrelated/newer catalog entries do not complete setup. Adapter should publish/refresh its catalog after successful Create or successful lookup; it must not infer success from a generic catalog entry alone.

Create failure, lost ack, Cancel, expiry, and disconnect retain unresolved creation identity. Only an explicit `LookupCreation` may resolve it. None means absence at lookup time, **not permission to automatically repeat Create**. `RepeatCreate` is a distinct UI action available only after that lookup returned None and the original verified ticket is still valid; it sends the identical pair under a fresh desktop operation. If the ticket was revoked/expired/disconnected, repeat remains unavailable; do not substitute a fresh ticket or root. Cancel/edit/new-setup cannot discard an unresolved mutation. Explicit bounded parking below is the only way to start another setup while retaining that uncertainty. Lookup remains possible after reconnect even though the approval ticket is gone. Creation identities are also journaled by the desktop before any Create RPC. Restart restores them as unknown, lookup-only identities, without tickets, trust, grants, or automatic requests. Approval-decision recovery uses a separate write-ahead metadata journal; it restores neither decision authority nor original-owner lookup proof (see `CLI_APPROVAL_API.md`).

## Durable creation handshake

Create and RepeatCreate emit `Effect::JournalLocalCliCreation`, not an immediate backend request. The controller durably commits the exact public pair to the bounded, namespace-scoped desktop journal, then calls `local_cli_creation_persisted(conversation, generation, op, identity, result)`. Only a matching first successful acknowledgment, with the original verified ticket/root/mode/target/trust and live scope still intact, releases Create. Backend completions before the acknowledgment and duplicate/stale acknowledgments are ignored. Failure or scope retirement never releases it later automatically.

Before constructing the UI model, call `restore_local_cli_creations(storage.load_cli_creations())` independently of ordinary cache fallback. It restores at most nine unique identities (eight parked plus one active) and sends nothing. Invalid, unreadable, conflicting or over-capacity recovery disables new creation; it must never become an empty journal. Exact successful receipt-plus-catalog reconciliation emits `RetireLocalCliCreation`; retirement deletes only that exact pair. A failed retirement conservatively leaves an extra read-only record for the next restart. No raw arguments, prompts, tickets, or grants are stored.

## Recovery parking: parent UI handshake

The UI/adapter implement this handshake. Buttons dispatch through `Command::LocalCli(...)`, gated by `local_cli_availability()`:

| Button | Availability | Command |
| --- | --- | --- |
| Keep recovery record and start another setup | `park_creation` | `ParkCreationAndBeginNew(draft)` |
| Select recovery identity (one per retained pair) | `recover_creation` | `RecoverCreation(identity.clone())` |
| Look up creation | `lookup_creation` | `LookupCreation` (existing) |

Read retained exact pairs from `local_cli.parked_creations()` (a read-only slice); display both session and setup identities. `LOCAL_CLI_RECOVERY_CAPACITY` is 8. No removal/forget action or silent eviction exists. Capacity full disables parking a **new** pair and leaves the active unresolved pair untouched. Re-parking an already retained pair uses no additional slot. Explain the capacity limit rather than suggesting that absence is final.

Parking is available only after a correlated explicit Lookup returned None, with no pending operation and no confirmed result. It records the exact pair and starts a new `Editing`/`New` flow, clearing active identity, approval, trust, and lookup-absence evidence. It sends no Inspect/Lookup/Create automatically; if a still-valid approval exists it may emit cleanup Revoke. Normal Begin/Edit remain blocked for an unresolved active identity; parking is an explicit, separately labeled choice, not an automatic fallback.

Recovery requires no pending operation. It accepts only an identity already in the retained list, uses a fresh global generation with target `New`, enters `CreateUnknown`, and restores **no** ticket or trust. If another unresolved active identity is not already parked, it swaps into the selected record's slot, retaining every identity even at capacity. Otherwise the selected entry remains in the list while active. Old setup authority may be explicitly retired through cleanup Revoke. Recovery itself does not send Lookup/Create: the user explicitly chooses Lookup next. Lookup is permitted in read-only mode; Create/RepeatCreate/Inspect/Check are forbidden in this flow, including after another None. After None the user may explicitly park it again (starting another setup requires writable mode).

Wrong/stale receipts, lookup failures, None, Cancel, disconnect, and generic catalog entries never remove retained records. Removal occurs only when an exact correlated Create/Lookup success receipt has reconciled with its collision-checked desktop catalog ID. Catalog-first and receipt-first both work; no selection occurs on catalog evidence alone. A failed cleanup Revoke cannot downgrade an already confirmed `created: Some(...)` from `Reconciling` to `CreateUnknown`: the exact catalog entry still completes it.

## UI flow

- `BeginNew(draft)` or `BeginExisting { conversation, session }` performs no automatic inspection/probe. Existing targets must already have `LocalCliChatOnly` profile and must not be owned by another engine.
- `Edit(draft)` drops ticket and both trust choices, retires pending results, and revokes any old approval. It is unavailable while creation is unresolved.
- `Inspect` returns metadata and enters `AwaitingTrust`; **both** `TrustExecutable` and `TrustProject` start false and must be explicitly checked before `Check`.
- Check's `MetadataVerified` enters `Verified` only; a lost check enters `CheckUnknown`. Only explicit `Status` polls it; no automatic check/retry.
- New setup offers `Create`. Existing reapproval never does: after Verified, `ReturnToConversation` explicitly selects/opens/refreshes the **captured** conversation. It does not clear `execution_blocked`; the adapter's normal authoritative `ExecutionContext` snapshot remains responsible for readiness.
- Cancel/Revoke invalidate local ticket/trust and request backend revocation when possible. They do not cancel or erase an uncertain Create. UI must not disable Revoke merely because a creation identity exists: use `local_cli_availability().revoke` directly. Revoke also remains available after Existing Return/Open advances the attachment generation. Disconnect retires pending replies and invalidates tickets/trust; reconnect does not automatically probe or retry setup.

Stages: `Editing, Inspecting, AwaitingTrust, Checking, CheckUnknown, Verified, Creating, CreateUnknown, Reconciling, Created, Failed`. `Reconciling` means lookup in flight or exact completion waiting for catalog. `error` carries a closed failure plus display reason where applicable. Read state for presentation; issue commands rather than mutating trust/tickets/pending state directly.

## Fresh-context assistance and redacted diagnostics

`FreshSetup(captured_target)` requires an exact live Existing target, writable connection,
no pending setup/uncertain creation, a non-busy source run, and matching saved mode/model/path
hints. Running, stopping and outcome-unknown source runs are ineligible. It retires any current
setup ticket, then prepares a separate New/Editing flow with only executable/project/model/mode.
It does not copy transcript, prompts, drafts, arguments, goals, approval records, trust, tickets
or the CLI session UUID. Source conversations remain untouched. Inspect, both trust choices,
Check, journaled Create and a new user message are separate actions. Crash recovery may still
leave an external process alive: inspect processes/files first; fresh metadata is not containment.

`CopyDiagnostics { conversation, generation }` checks the captured setup route and emits only
CopyText. It never probes. The report contains closed enums, booleans/counts and static guidance
for authentication, version/flag policy, expired approval, unknown creation, invalid context and
recovery storage. Paths, account metadata, IDs, raw error strings, prompts, arguments and transcript
text are excluded by construction. Rendering only reads static guidance; report construction is
an explicit command effect.

## Verification scope

Run `cargo test -p pipkin-core` and `cargo clippy -p pipkin-core --all-targets`. No workspace/native/UI/live execution verification is claimed until adapter/UI exhaustiveness and transport mapping are implemented by their owners.
