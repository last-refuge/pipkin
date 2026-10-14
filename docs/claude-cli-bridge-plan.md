# Native local Claude CLI bridge: implementation plan

Status: **Phase 0 release gates open; Phase 1 foundation and Phase 2 transport foundation implemented**.
Workspace tests, Clippy and formatting pass. Standalone Rust probe/fake-process harness implemented;
authorized fresh/resume, exact Write deny/allow-once and pending-approval interruption checks
passed. Calibrated startup/discovery isolation and offline durable-lifecycle/crash-cut checks
passed. Persistent identities, an external ledger and native-execution refusal are implemented;
CLI execution, general permission UI, real CLI crash recovery/orphan cleanup, broader isolation
and billing qualification remain open. No CLI create/select RPC or runtime subprocess is enabled.
Progress and commands: [Qualification and implementation evidence](claude-cli-bridge-evidence.md).
Planning inspection: 2026-10-13. Locally installed Claude Code: **2.1.296**.

## 1. Goal and product contract

Let a person select their locally installed, unmodified Claude Code CLI as a conversation's execution backend in Pipkin. The CLI uses its own login and makes its own model requests. Pipkin remains a native Rust/GPUI desktop with no required Node/Python SDK or Pi runtime.

This is **running Claude Code locally**, not extracting its OAuth token or proxying Pipkin's Anthropic API requests through subscription credentials. Subscription eligibility, allowances, extra usage, and billing remain controlled by Anthropic and the person's Claude Code account/settings. Successful CLI output alone does not prove which allowance was charged.

Release gate: verify the actual supported launch configuration with an explicitly authorized live account, including observed billing behavior. If it still requires extra usage, report that honestly; do not advertise a subscription-limit workaround or silently switch to an API key.

Use a neutral UI heading such as **Local CLI**, with explanatory text “Runs your installed Claude Code.” Clearly distinguish this backend from direct Anthropic API/OAuth connections. Do not imply Anthropic endorsement.

### Non-goals for the first release

- No credential copying, new Pipkin-managed Claude login, binary modification, request-header impersonation, hidden API, or automatic installation/update.
- No transparent conversion of existing native/API conversations to CLI sessions.
- No promise of identical Pipkin tools, subagents, steering, compaction, model switching, or context semantics on day one.
- No automatic fallback between CLI, subscription OAuth, API key, or cloud-provider billing.
- No terminal-screen scraping or PTY automation as the primary transport.

## 2. Findings and architectural decision

Verified by local `claude --version`, `claude --help`, and `claude auth status --help` (no inference request):

- `--print`, `--output-format stream-json`, `--verbose`, and `--include-partial-messages` are available.
- Explicit `--session-id` and `--resume` are available; `--continue` targets the most recent session and is unsuitable for concurrent Pipkin conversations.
- `claude auth status` supports JSON. Initial planning did not inspect account state; the Phase 0 smoke now observes a CLI-reported Claude-account login (not billing proof).
- `--bare` explicitly disables OAuth/keychain reads: **never use it for this subscription-oriented route**. Official docs say bare mode may become the print-mode default; compatibility must be rechecked on CLI upgrades.
- Print mode skips the CLI workspace trust dialog and can load hooks/MCP/project configuration. Trust and configuration must be resolved before spawning it.
- Phase 0 now verifies basic streaming, explicit completed-turn resume, exact Write deny/allow-once, and controlled pending-approval interruption through the documented PermissionRequest hook on 2.1.296. The private ledger passes synthetic crash cuts and scoped approval lifetime tests. General permission hosting, real CLI crash reconciliation/orphan cleanup and broader control behavior remain unverified.

Existing code boundaries:

| Area | Current location | Planned responsibility |
| --- | --- | --- |
| Native agent loop | `crates/pipkin-engine/src/runtime/run.rs` | Dispatch by execution backend before API preparation, retries, tools, or compaction |
| Model exchange | `crates/pipkin-engine/src/provider/mod.rs` | Remain the direct-model contract; do not masquerade a whole CLI agent as an `ApiAdapter` |
| Durable state | `crates/pipkin-engine/src/store/{mod,schema,writes}.rs` | Backend identity, CLI session binding, external run ledger, committed transcript |
| Desktop boundary | `crates/pipkin-engine-protocol/src/{api,view,content,lib}.rs` | Backend selection/status, capabilities, permission and external-action events |
| Native desktop adapter | `crates/pipkin-app/src/adapters/native/` | Map engine events without owning subprocesses |
| Availability/state | `crates/pipkin-core/src/` | Backend-specific command availability and stale-event guards |
| Connections/settings UI | `crates/pipkin-ui/src/shell/{settings,onboarding}.rs` | Discovery, setup status, selection and explicit limitations |
| MCP | `crates/pipkin-engine/src/mcp/` | Existing client infrastructure; an outbound tool server would be new work |

### Proposed shape

```text
Pipkin UI / pure core
  -> existing native engine protocol
    -> shared admission, queue, persistence and publication
      -> execution backend
        -> NativeAgent: existing provider + Pipkin tool loop
        -> LocalClaudeCli: Rust supervisor -> installed claude process
                              <- bounded NDJSON events
      -> committed history / usage / outcome -> desktop
```

Introduce a small run-level execution boundary, not a general plugin platform. Proposed modules:
`crates/pipkin-engine/src/execution/{mod,claude_cli/}.rs` and a focused runtime bridge module. Keep the existing native path behavior unchanged; share admission/settlement utilities where their semantics actually match.

**Exactly one agent loop owns execution.** CLI tool-use events are observations, not instructions for `run.rs::tool_round`. Store them as externally executed actions with explicit provenance; never feed them into pending native tool-call recovery. Claude Code owns model context, its tools, and automatic compaction. Pipkin owns presentation and durable operation tracking, not a second model-context replay.

## 3. Runtime, persistence, and safety contract

### Discovery and authentication

- Resolve an explicit configured executable first, then desktop-process PATH; support a user-selected absolute path for GUI environments and version-manager installs. Never guess by invoking a login shell.
- Probe only after user opt-in to this executable; apply deadlines, bounded output, and off-UI-thread execution even to `--version`, `--help`, and auth status. Executable paths are executable code, not a sandbox.
- Store executable/version/capability metadata, not credentials. Resolve and recheck version-manager symlink changes at launch; don't replace the user's CLI or pin their global installation.
- Let the person sign in using the CLI's own `claude auth login` flow in their terminal. Pipkin offers instructions and a Refresh status action. Do not read CLI auth files/keychain tokens or log account email, organization IDs, or secrets.
- Read the documented, nonsecret subset of auth status. A ready subscription-oriented connection must report a recognized Claude-account method; unknown/other methods get an explicit status, never an inferred subscription badge.
- Inspect effective auth routing using a validated launch environment. Detect inherited API keys, OAuth overrides, base-URL overrides, cloud-provider selectors, and configured key helpers by presence, not secret value. Ask the person to resolve conflicts or explicitly choose another billing route; do not silently strip credentials and claim subscription access. Do not disable authentication methods in the CLI itself.
- Confirm the same selected executable/configuration/environment is used for preflight and execution. Preserve legitimate proxy/CA settings and report unsupported engine-only network settings rather than assuming CLI parity.
- No billing settings are changed by Pipkin. Recheck status after authentication failures/account changes; never “repair” them by falling back to another connection.

### Process and stream lifecycle

- Initial target: one foreground `claude -p` child per admitted user turn; sequential turns resume the explicit CLI session ID. Keep launch arguments in a version-tested builder. Use stdin for prompts (not a shell command or process-list argument).
- Candidate streaming flags: `--print --output-format stream-json --verbose --include-partial-messages`, plus explicit model/session and validated safety settings. Phase 0 must establish the exact invocation and stdin framing; don't assume undocumented bidirectional control messages.
- Only one live writer per bound CLI session. Use shared process/generation budgets; no CLI background/remote/cloud dispatch flags in the MVP.
- Drain stdout and stderr concurrently. Bound line size, parse buffering, retained diagnostics and event queues; apply backpressure. Handle split UTF-8/JSON, unknown events, truncated lines and oversized frames without hanging or unbounded memory.
- Decode init/session metadata, text/thinking when exposed, completed assistant messages, external tool calls/results, permission denials, compaction notices, and final success/error/usage. Do not manufacture thinking the CLI does not expose.
- Deduplicate partial versus final messages and result summaries. Commit event projections before publication. Retain only bounded/redacted diagnostic metadata, not indefinite raw CLI output.
- A final CLI result, child exit, and background descendants are distinct facts. Success needs a valid final outcome and safe process settlement; EOF/exit 0 alone is not success. A result carrying an error is not success even if text exists.
- Supervise the owned process tree. On Stop, request documented graceful termination, drain final events within a deadline, then terminate remaining owned descendants. Confirm exit before publishing cancellation. Never signal arbitrary PIDs or the user's independent Claude sessions.
- Handle engine shutdown/crash, desktop detach, hung children and orphan descendants. Reuse existing process-supervision primitives only after reviewing their semantics; the Bash tool's immediate-kill policy is not automatically appropriate here.

### Durable identity and recovery

Add versioned storage for backend kind/config reference and a CLI binding containing session UUID, canonical project path, executable/version, local account/config identity marker (nonsecret), and context validity. Existing sessions default to `NativeAgent`; migrations back up first and reject unsupported future schemas.

For each external turn, persist admission key, run ID, launch intent, send/ack state if observable, last committed event cursor/message IDs, external tool provenance, final result, exit state, and interrupted/unknown status. Record launch intent before spawning and sending input. A local write/flush or CLI echo is not proof the remote task completed.

Crash window rule: after a prompt may have been sent, **never automatically resend it**. Disable native provider retries and automatic tool replay for this backend. If supported public CLI status/history facilities cannot conclusively reconcile a run, expose Outcome unknown, preserve partial history, block queued delivery, and require the person's decision. No exactly-once claim across the external process boundary.

Pipkin's DB is authoritative for UI history; the CLI's own persisted session is authoritative for resumed model context. Retain the explicit binding, not the “latest” session. Missing/deleted/externally modified CLI history must produce a blocked or reconciliation state, not a silent empty-context continuation. Document that export of Pipkin history alone cannot recreate a CLI session.

**Stop/reset semantics are a release concern.** Native Pipkin retires stopped inputs from future model context; terminating Claude Code does not necessarily remove them from its history. After interrupted/uncertain turns, mark the binding unsafe to resume. Require a fresh CLI session with an explicit user-reviewed handoff, unless a supported and tested CLI operation provides the required retirement semantics. Do not claim native Stop parity by merely killing the process. Likewise, reset creates a fresh CLI session; no private transcript editing.

Changing execution backend creates a new conversation (optionally with an explicit summary handoff). Model/effort changes within a CLI session are offered only if tested between completed turns. Bindings belong to the engine host: a remote native engine runs that host's CLI, not the desktop's; disclose this and limit the initial release to local-engine configurations.

### Trust, permissions, tools, and context

- Require explicit project trust before any print-mode spawn. Explain that CLI tools/hooks may execute locally and can access files/network beyond the project; neither Pipkin nor these flags create a security sandbox.
- Phase 0 must establish an auth-preserving configuration policy. Evaluate documented safe/restricted modes, setting sources, explicit settings, and strict MCP configuration. `--safe-mode` preserves auth according to installed help but its interoperability must be tested; `--bare` is not an acceptable substitute.
- Default managed bridge sessions must exclude unapproved project/user hooks, plugins, agents and MCP auto-discovery, or present a precise opt-in before loading them. Preserve administrator policy. If available CLI controls cannot deliver this boundary, block that configuration rather than silently inheriting executable customizations.
- Read-only milestone: explicit read-tool allowlist, deny unanswered permissions, no shell or write tools. Broad `--allowedTools Bash`, auto mode, and `--dangerously-skip-permissions` are not defaults.
- Coding milestone: integrate a documented permission host. Phase 0 evaluated the MCP `--permission-prompt-tool` path (historical schema found, current compatibility unverified) and proved a basic alternative through the currently documented `PermissionRequest` command hook. Safe mode disables either host; the independently tested restricted-mode policy must be used and further qualified. The hook omits `tool_use_id`, so generate a host-owned per-invocation request ID rather than inventing one from CLI fields. Bind approvals to session/run/tool/request ID, validate arguments, display them, and deny expired/stale requests or disconnected hosts. Account for CLI rules/hooks that approve before asking the host; any preapprovals require explicit consent. If this cannot be supported reliably in Rust with the public interface, retain a clearly labeled read-only MVP, not permission bypass.
- Keep Claude Code's own system prompt. Append only bridge-specific presentation/integration instructions through supported flags. Document which CLAUDE.md/skills/settings are loaded; don't double-inject Pipkin's full prompt or pretend all Pipkin resources apply. Freeze this context policy per session and explicitly reset/handoff when it changes.
- Optional later Pipkin tool integration uses a narrow, session-scoped MCP server over stdio or an authenticated local endpoint. It exposes only approved tools, propagates cancellation/permissions, and uses Pipkin's mutation locks/admission ledger. Never expose arbitrary engine administration or unauthenticated control endpoints.
- Choose a single executor per tool category: CLI-owned file/shell tools OR Pipkin tools via MCP. No duplicate file/shell executor, recursive delegation, or tool calls replayed from display history. The MCP server and permission host are distinct responsibilities, even if they share transport machinery.

## 4. Capability contract and UI

Introduce per-session execution capabilities (not just global engine capabilities); enforce them in both engine validation and `AppState::availability`. Version the protocol as required.

| Feature | First supported scope |
| --- | --- |
| Text streaming, history, explicit model alias | MVP, tested per supported CLI version |
| Follow-ups | Durable Pipkin queue, delivered after confirmed completion only |
| Resume | Completed turns and valid explicit bindings only |
| Stop | Owned-process termination; unsafe context binding invalidated |
| File/shell actions | Read-only first; coding after permission integration gate |
| Tool/permission UI | External provenance; no native tool execution from CLI events |
| Usage | Reported tokens/cost when present; unknown is unknown, not zero or exact subscription billing |
| Thinking/effort | Only exposed supported values; no assumed mapping to every native thinking level |
| Steering during a turn | Disabled initially; not silently treated as a follow-up |
| Images/attachments | Explicitly unavailable until public transport semantics are tested |
| Native compaction/context editing | Disabled; CLI owns its context and compaction |
| Pipkin subagents/goals/plans/jobs/tools | Disabled as agent capabilities until their MCP/lifecycle contracts are implemented |
| CLI subagents/background work | Disable where supported in MVP; observe and supervise any remaining work, never mislabel it as native Pipkin children |
| Backend switching/fork/export/import | Readable history retained; fresh explicit handoff, no opaque-session cloning promise |

Connection UI shows missing CLI, untrusted executable/project, unsupported version, signed out, conflicting billing configuration, ready, permission blocked, and process/protocol failures. Distinguish engine readiness from CLI readiness. Display the selected executable, version and engine host without secrets.

Existing direct-provider connections and stored credentials remain unchanged. The new route is opt-in. Missing CLI must not prevent Pipkin startup, browsing history, or use of other providers. An unsupported CLI upgrade disables only new CLI runs, with an actionable message and no automatic downgrade/install.

## 5. Delivery phases and exit criteria

### Phase 0 — feasibility and subscription qualification (blocking)

Progress: offline `claude_cli_probe` example and synthetic stream/process tests added;
version/help discovery and explicitly authorized fresh/resume and Write deny/allow-once
turns observed on 2.1.296 with a CLI-reported Claude-account login; a further authorized
pending-Write interruption passed with no grant or target. Controlled startup hooks and
project MCP discovery have calibrated isolation evidence. The disposable ledger has scoped
approval lifetime, pre-signal revocation, no-replay crash-cut and unsafe-resume tests (81 tests
total). General permission UI, live expiry/disconnection, real CLI crash/orphan recovery,
managed-policy and broader print-mode isolation, and actual subscription billing remain open
gates. See the evidence file and sanitized summaries.

Deliver `docs/claude-cli-bridge-evidence.md` with versioned sanitized fixtures and measured/observed/failed/unverified labels.

1. Review current official CLI, headless, authentication, permission, and product-integration terms; confirm unmodified local CLI + user-owned sign-in is the supported intended surface. Flag any unresolved distribution/branding questions before release.
2. Build a disposable Rust spike/fake harness, not a production provider adapter. Verify stdout schema, final results, stdin, session IDs, explicit resume, allowed models/effort, auth-preserving customization isolation, termination, background descendants, and permission hosting.
3. With explicit permission, use a scratch project and small live prompt; record account-route status without identifiers, selected model, CLI version/config, result, and before/after usage observations. Compare direct terminal CLI and bridge invocation under the same account/config. No automatic extra-usage purchase or enabling spend.
4. Repeat a completed-turn resume, a permission denial and a controlled interruption. Establish whether public facilities can reconcile unknown outcomes; if not, commit to the blocked/explicit-handoff behavior above.

Exit: supported launch/safety contract documented, live route works, subscription billing evidence is understood (or explicitly inconclusive), and permission strategy has a proven path. If billing or product-use assumptions fail, stop and revise the proposal rather than build a bypass. A technical preview may proceed without a subscription billing claim; the advertised subscription objective remains unmet.

### Phase 1 — backend identity, capabilities, and storage

Implement the run-level dispatch and backend types; migrations, CLI bindings, external action provenance, turn ledger, protocol/status/capability additions. Keep new backend feature-gated/off by default. Default all old sessions to native execution.

Foundation implemented (storage-only; no new live inference):
- Schema **21** backs up/migrates old sessions to `NativeAgent`; immutable backend/binding
  identity is independent of provider/model and import origin. Unknown backends fail closed.
- Nonsecret CLI bindings, a separate `(session, generation, op)` run/action ledger, atomic
  unfinished-to-Unknown startup recovery, and permanent per-store UUID receipts prevent
  rebinding a removed context. Completion refuses unresolved action metadata. A duplicate
  admission never authorizes relaunch; only fresh bindings or completed prior runs admit work.
- Protocol **32** carries execution identity and refuses older peer handshakes rather than
  allowing an old client to mistake an external session for native execution.
- Native admission, saved compaction, tool replay, retries, queues, extension startup,
  goals, jobs and subagents are fenced. Core/adapter snapshot gating is conversation/generation
  scoped; delayed journal acknowledgements cannot bypass it. History/rename/removal remain usable.
- This is a coarse disabled-backend gate, not the full capability/status or external transcript
  projection. There is no CLI constructor RPC, backend selector, subprocess integration,
  production approval broker, CLI credential access or API fallback. Those remain future work.

Exit: fake-backend tests demonstrate CLI events cannot enter native tool replay, retry, or compaction; old session/protocol compatibility and stale-event routing pass. Both engine and UI reject unavailable capabilities.

### Phase 2 — read-only vertical slice

Transport foundation implemented, still disabled in runtime: shared bounded supervision,
nonblocking stdout observation, explicit trusted-candidate preflight, fixed version-qualified
**no-tools/chat-only** launch policy, and bounded display-only stream projection. Fake CLI
integration covers stdin-only prompts, identity mismatch, backpressure and terminal-before-exit
failure. All 81 original qualification tests remain (30 shared-library + 51 example tests);
new coverage brings the shared CLI module to 78 tests. Full workspace gates pass (job #33).
This is not the end-to-end read-only milestone: durable prompt/projection receipts, runtime
routing, desktop setup/capabilities and live strict-schema qualification remain ahead.

Implement discovery/preflight, bounded subprocess supervisor and decoder; admission-to-stream-to-settlement mapping; explicit-session completed-turn resume; cancellation and conservative crash recovery. Add fake CLI programs and replay fixtures before real-account tests.

Exit: a selected trusted project streams and resumes in the desktop, read-only tools appear as external actions, queues remain serialized, no secrets enter logs, and no other conversation/process is affected by Stop. All advertised MVP capabilities work without relying on native provider credentials.

### Phase 3 — safe coding and person interaction

Add documented permission hosting and UI integration; explicit consent for any preapproved rules; stale approvals, denial, timeout, disconnect, and cancellation paths. Allow editing/shell tools only after the host/configuration boundary is verified. Treat AskUserQuestion/MCP elicitation as separate protocols to test, not automatically covered by permission support.

Exit: in a scratch project, approve a bounded edit and command, deny another, cancel a pending approval, and restart after an uncertain mutation without replaying it. Changes inspector reports actual Git/file changes. No permission-bypass flags are needed. Without this gate, the product stays labeled read-only.

### Phase 4 — setup UX, optional tool parity, and hardening

Complete Connections/model picker/help, per-session capability visibility, diagnostics, backend selection and handoffs. Optionally add a narrowly scoped MCP integration for Pipkin plans/goals/jobs/asks/subagents; design each lifecycle explicitly and keep unsupported features disabled. Do not block basic local-CLI support on full tool parity.

Exit: signed-out/missing/upgraded CLI, auth conflicts, denied actions, account changes and lost CLI session history all give actionable messages. Existing API/native conversations behave unchanged; optional tool execution occurs once under its intended owner.

### Phase 5 — release qualification

Run the matrix below, publish minimum/tested CLI versions and platform support, and update `docs/{native-engine-reference,getting-started,diagnostics,live-provider-checks,scorecard}.md` as applicable. Extend `pipkin --engine native --diagnose --probe` with non-inference CLI checks only when configured/opted in; no auth installation, mutation or model prompt during diagnostics.

Exit: workspace gates pass; native desktop and authorized live subscription checks have evidence; unsupported/unmeasured behavior remains labeled. Feature can be disabled without deleting history, CLI sessions, or credentials. Ship initially as opt-in local-engine support.

## 6. Verification matrix

- **Unit/contract:** argument construction without shell interpolation; executable/path changes; auth/env conflict detection; capability mapping; NDJSON chunk boundaries/UTF-8; oversize/unknown records; text/final deduplication; usage not double-counted; nested tool IDs; redacted failures.
- **Fake CLI integration:** missing/hung executable; stdout/stderr floods; malformed output; exit 0 without result; result error; exit after final result; permission host unavailable; Stop during each phase; descendants surviving parent; two conversations in one project; engine restart; CLI history missing; version changes; no implicit `--continue`.
- **Durability/fault injection:** crash before spawn, after spawn, after stdin delivery before ack, after tool effect before event commit, after final result before settlement, and after settlement before queued follow-up. Persist partial state and never replay an uncertain prompt/tool action. Simulate storage failure during streaming and stop further unsafe execution.
- **Safety/context:** untrusted repo hooks/plugins/MCP do not execute; auth remains usable with approved isolation; managed policy is honored; no default API fallback; late permission answer cannot authorize a new run; stopped input cannot silently return via CLI resume; no account/token exposure in support bundles.
- **Desktop:** text streaming, tool provenance, permission dialogs, follow-up queue, Stop, unknown outcome, conversation switching, reconnect and blocked capabilities. All events retain `(conversation, generation, op)` guards; render does no I/O or parsing; `pipkin-core` stays GPUI-free.
- **Live, opt-in:** terminal versus bridge same-account behavior, actual account-route status, usage observation, multi-turn resume, tiny edit/command with approvals, rate/extra-usage error presentation. No assertion of exact billing from token/cost fields alone. Never run live tests in default CI.
- **Regression:** `cargo test --workspace && cargo clippy --workspace --all-targets && cargo fmt --all --check`.
- **Native test safety:** read the GPUI skill's native-testing reference first; synthetic input only via `scripts/guard.sh`, pinned to the launched window. No changes to unrelated windows, compositor settings or packages.
- **Performance:** measure CLI startup, first event/token, stream-to-paint latency, idle/peak memory and teardown. Separate CLI/network latency from bridge overhead; set release budgets using measurements, not guessed parity.

## 7. Main risks and decision gates

1. **Billing/terms assumptions:** local CLI ownership is not a guarantee of plan-limit billing. Phase 0 is the go/no-go gate; keep policy/billing claims separate from technical success.
2. **CLI changes:** print-mode defaults, auth behavior and event schemas can change. Feature detection plus a tested version range; unsupported changes fail closed for new runs.
3. **Two durable histories:** Pipkin records and CLI context can diverge. Explicit identity, conservative unknown outcomes and fresh-session handoffs beat unsafe automatic repair.
4. **Permissions/customizations:** print mode skips trust; CLI config may run code before a prompt. Auth-preserving isolation and a real permission host are prerequisites for safe coding support.
5. **Native feature parity:** the CLI is another agent, not a model socket. Advertise capabilities honestly; add MCP parity separately instead of layering two agent loops.

## Sources and verification scope

Official references read on 2026-10-13 (recheck during Phase 0 and before release):

- https://code.claude.com/docs/en/headless.md — print mode, streaming, trust caveats, bare-mode auth and explicit resume.
- https://code.claude.com/docs/en/cli-reference.md — public flags, auth status, session and permission integration.
- https://code.claude.com/docs/en/legal-and-compliance.md — unmodified binary, user-owned authentication, product integration and branding; distinguish hosting/running Claude Code from intermediating subscription credentials.

Local inspection: `claude --version` => `2.1.296 (Claude Code)`; `claude --help`; `claude auth status --help`; repository files listed above. No live Claude inference, account status lookup, credential access, permission-host test, installation, or billing modification was performed while creating this plan.
