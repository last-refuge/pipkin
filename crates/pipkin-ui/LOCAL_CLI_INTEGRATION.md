# Local CLI setup and approval UI integration

Normal owned native desktop engines now enable chat/Write hosting by default. The sidebar,
palette and Connections entry points require no experimental launcher flags. This is host
availability only: path trust, setup checks and exact Write approvals remain explicit; existing
conversation backends are unchanged. Demo remains disabled and attached engines own their policy.

The UI owns presentation only. All setup effects are `Command::LocalCli` → core → the installed controller effect handler. No UI filesystem, path resolution, login, provider, subprocess, or transport calls are added.

## Controller wiring

`Model::set_local_cli_catalog(available, sessions, cx)` is the UI integration hook:

- `available` must be the adapter's actually advertised setup capability, not inferred from real mode, a model, or a CLI conversation. Core's default is false, so setup remains visibly disabled until advertised. The concurrent core/adapter work now provides `LifecycleEvent::LocalCliSetupAvailable` → `set_local_cli_available`; the controller handles that path. This hook also calls the same core setter, not a parallel availability policy.
- `sessions` replaces the authoritative snapshot of `(desktop ConversationId, EngineSessionId)` pairs. Populate each engine ID from the captured source's **backend session ID** / adapter catalog mapping, never by formatting the desktop ID, using a selected session after async work, or using a native model ID.
- Refresh this snapshot with authoritative catalog changes; supply an empty snapshot when unavailable. `Model::set_connection` clears these IDs whenever disconnected. Core/adapter lifecycle updates own capability invalidation. The new-setup capability path is wired by the concurrent controller work. The adapter now exposes `NativeBackend::local_cli_sessions()` specifically for this hook; the controller feeds that snapshot on Catalog/capability lifecycle updates for reapproval.
- At implementation time `pipkin-core/src/` exposes `ConversationState::{profile, execution_blocked}` and typed Local CLI IDs, but no `Source`/backend-session-ID accessor. This bridge avoids guessing IDs. If core grows such an accessor, the bridge can be replaced with that captured authoritative source.
- Reapproval captures the desktop and engine IDs synchronously before `BeginExisting`. Loading SessionStatus hints is a separate explicit action. Hints do not check either trust box; using them is an explicit draft edit.
- Preserve authoritative `ExecutionContext` updates. MetadataVerified does not unlock the composer: it uses core's `read_only_reason` (profile/readiness/ownership), while sending still uses core availability.

The initial setup handoff was UI-only. Subsequent core/adapter/controller integration is recorded
in `docs/claude-cli-bridge-evidence.md`; approval routing uses the dedicated core port described
in `crates/pipkin-core/CLI_APPROVAL_API.md`.

## UX and recovery

Ctrl+N remains native. Navigation, the palette, and Settings → Connections offer “New Local CLI conversation…”. Settings and the palette offer reapproval and “Resume setup…”. Resume and closing are presentation-only; neither dispatches a probe or discards pending mutation state.

Executable/project inputs are dedicated ComposerEditors with retained subscriptions. Only user Changed events dispatch Edit. Reconciliation uses silent set_text, preserving core trust. Alias choices are separate from the native model picker. Existing-session aliases come from explicit saved-hint loading.

Inspect displays captured resolved paths; two separate unchecked approvals precede Check. Create/Return is gated by core. Unknown Create permits pure LookupCreation; RepeatCreate remains disabled until confirmed absence, and uses the original identity. Revoke uses core `local_cli_availability().revoke` directly, including after Created, uncertain Create, and Existing Return/Open; it never discards creation identity.

After an explicit Lookup returns None, “Keep recovery record and start another setup” parks the exact pair through core. The UI lists both session and setup IDs and the eight-parked-record bound. Exact identities are committed before Create and survive restart (eight parked plus one active); no trust, tickets, grants, or automatic requests are restored. An unreadable journal disables creation visibly. At capacity, selecting another record swaps the unresolved active identity into its slot; there is no eviction/forget fallback. Recovery is explicit, works read-only when core permits it, restores no approval/trust, and never sends Lookup/Create automatically. Generic catalog/history selection is not recovery evidence.

Model/effort menus, shortcuts, palette entries, and attachment picker/drop/paste entry points use `execution_capabilities()`. CLI conversations display “Claude CLI · fixed model”, not the selected native model; unsupported contexts say so. Saved attachments remain removable. The lease warning remains explicit: five minutes after Inspect, expiry/revocation stops active work, and interrupted context requires a fresh conversation. Preflight freshness for Create is separately 30 seconds; a delayed/uncertain Create requires explicit lookup, never blind resubmission.

New setup offers immutable Chat only or Restricted Write mode. Restricted Write is explicitly labelled experimental **Write-only**, with no Read/Edit/Bash/MCP/subagent parity, no sandbox or all-action-mediation claim, and a fresh-conversation requirement for policy changes. Setup alone does not enable execution; both host capabilities and a current owner grant are required; the owned desktop supplies the capabilities by default, not the grant. Mode changes are core Edit commands, clearing both trust choices. Existing-mode buttons are disabled; explicit saved-hint adoption includes the stored mode. Unknown modes cannot be adopted for Inspect. Ticket display includes the captured mode. Approval controls are implemented below, but cannot grant readiness or bypass the separate host policy/current owner grant.

The UI states the selected mode's limitations, CLI-owned login for chat, and unverified billing/account allowance/already-enabled extra usage. Demo entry points stay disabled and retain the “Demo · simulated agent” marker. Existing native_usage filtering and billing-unverified session labels remain intact.

## Captured approval panel

The coding conversation header and palette offer **CLI approvals / Review CLI approvals…**.
Opening the dialog only captures its conversation, attachment generation and model operation;
it sends nothing. A normal dialog Enter does not decide. Selection changes do not redirect
reviews or callbacks. Reopen explicitly to capture a different run.

- Refresh reviews is explicit and uses core discovery availability. API capability alone is
  not readiness. Root/attachment/profile/ownership/capability loss retires authority in core.
- The request picker retains exact invocation identity. Only its selected request's complete
  ASCII-escaped display appears, in a cached plain-text entity with scrolling and no markdown,
  links, tools or parsing. Changes are prepared outside render; full text is not cloned on
  unchanged clock updates. Scope/digest/expiry hints are separate from untrusted arguments.
- Allow once / Deny and read-only receipt lookup use shared core availability. Every callback
  carries the captured context/ID. Button identity includes run/attachment/invocation/digest,
  rather than an index or reusable action label. An attempted decision never becomes available
  again after timeout, refresh, close or reattachment. Consumed is explicitly NOT proof of
  delivery or action completion. There is no decision retry or automatic lookup/polling.
- Copy and Ctrl+Shift+C read the complete current scoped review, never a snippet, without
  deciding or probing. The argument region is an explicit Tab stop with independent scrolling;
  modal Tab/Shift+Tab stays inside the dialog. Plain activation is handled once across native
  key-down/key-up; disabled/expired controls recover to a neutral anchor, not another action.
  No review is put into a
  composer, native question/tool card, history, model context, diagnostics or persistence.
- Closing or switching overlays clears the presentation cache only. It neither denies nor
  resets core attempts. Reattach is explicit read-only preparation for an older attachment;
  it does not send a lookup or restore decisions, including after clock regression.
- Stop checks the captured conversation/generation/model op and current selection, then uses
  the ordinary guarded Stop path (whose native control worker is independent of data RPCs).
  An old panel cannot Stop a different selection/run. It never claims to undo effects.
- The dialog is viewport bounded and labels the lack of sandbox/universal mediation, untrusted
  argument text, unverified billing and durable metadata-only approval recovery. Restart restores
  neither plaintext nor owner/connection proof; unknown outcomes may remain unresolvable. The
  eight-context recovery bound persists across restart and cannot be bypassed by reopening.
  No real desktop accessibility,
  IME, focus or scrolling qualification is claimed by the headless tests.

Current-grant public coding admission and core→adapter→engine→fake actor flows pass job 94,
separately from the headless GPUI tests. Native146/158 subsequently exercised the built Linux
app/worker/helper with installed CLI2.1.299: completed fresh/resumed chat and Write, with the
fresh Write review failure retained and resumed Write exact-copy validation before Allow passed.
Full156 and scoped157 passed. Desktop hosting is now default-on, with explicit disable flags; see `docs/local-claude-cli.md`.
Installed release packaging, real IME/accessibility and broader platform/tool coverage remain
unqualified.

## Captured project Changes

Known CLI profiles show captured-project Git observations, not native session-edit evidence.
The inspector explicitly disclaims CLI action proof; unrelated project changes can appear.
External action/settlement invalidation and scan operation/source guards live in the adapter,
not render. Executable Git filters are disabled. Details and limits:
`crates/pipkin-app/CLI_WORKSPACE_OBSERVATION.md`. Job 99 verifies fake coding/Git integration
and inspector labels, not actual desktop input or complete filesystem effects coverage.

## Verification scope

`src/shell/local_cli/tests.rs` includes pure core/presentation tests and headless GPUI entity/input-handler tests. These do not launch a CLI, make live calls, inject native input, or verify real IME/accessibility behavior. Native input/live transport evidence is recorded separately in `docs/claude-cli-bridge-evidence.md`; real IME/accessibility remain unverified. Native short-argument navigation is not a nonzero overflow-scrolling measurement.

Scoped parking/capability integration coverage includes explicit park/recover, read-only recovery without automatic requests, bounded retention without eviction, exact-ticket Revoke with a retained creation identity, native/CLI/unsupported picker and attachment gates, and catalog-ID-change notifications. Native UI fixtures now supply an authoritative `ExecutionContext` rather than relying on catalog/history as a capability grant.

The following scoped setup-only results are historical. Later full workspace gates and the
six approval presentation/headless tests are recorded in the bridge evidence document. Neither
headless test family makes live CLI/account calls or injects native input.

- `cargo test -p pipkin-ui`: 178 passed, including 10 Local CLI tests.
- `cargo clippy -p pipkin-ui --all-targets --no-deps -- -D warnings`: passed.
- `cargo fmt -p pipkin-ui -- --check`: passed.
- `git diff --check -- crates/pipkin-ui`: passed.
