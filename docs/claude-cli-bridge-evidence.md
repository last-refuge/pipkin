# Local Claude CLI bridge — qualification and implementation evidence

Date: 2026-10-13. Status: **qualification gates open; Phase 1 storage/off-gate implemented;
CLI execution disabled**.
See [implementation plan](claude-cli-bridge-plan.md). No subscription/billing claim is established.

## Implemented scope

The standalone Rust example `crates/pipkin-engine/examples/claude_cli_probe.rs` and its
`claude_cli_probe/{process,stream,isolation,permission,qualification,lifecycle,interruption}.rs`
and focused interruption/recovery test modules support:

- Explicit executable opt-in, absolute-path/PATH discovery without a shell, version and
  help probes with deadlines. Current qualification target is exactly **2.1.296**;
  other versions fail closed before auth/inference. This is not a tested version range.
- An explicit `auth` mode asking the CLI for status, with only a fixed enum and login
  boolean emitted. Emails, organization IDs, config paths, raw status and stderr are
  excluded from JSON and Debug output. No CLI credential files/keychain tokens are read.
- A non-inference `isolation` mode using a disposable HOME/configuration and harmless
  hook/plugin/MCP marker controls. Positive controls must actually execute before
  suppression can pass; crashes, unreadable evidence, and early exits never prove isolation.
- Separate opt-in `permission-deny` / `permission-allow` modes using the current public
  `PermissionRequest` command-hook contract. They offer only Write (plus any CLI
  EndConversation tool), generate their own private project, and can authorize at most
  one exact file/content write. They are scripted qualification fixtures, **not a user
  approval UI or production permission broker**.
- An explicitly authorized `smoke` mode: one fixed no-tools prompt via stdin, fresh UUID
  or explicit resume UUID, no arbitrary CLI arguments/prompts, no retries or fallback.
  Both user-turn permission and executable/project trust are required at invocation.
- Same executable, environment snapshot, project and safety policy for auth preflight
  and smoke. Known routing override variables are reported by **name only** and block
  auth/smoke; they are not silently removed. CLI-reported `claude.ai` login is required.
- Concurrent bounded stdout/stderr pumps (8 queued 8-KiB chunks); per-line NDJSON limit
  256 KiB, probe stdout/stderr limits 128 KiB each, smoke stdout total limit 16 MiB.
  Stderr is counted but never retained. Timeouts: 10 seconds for probes; 60 for smoke.
- Unix owned-process-group supervision: TERM, grace (300 ms by default), KILL, and
  bounded reaping/draining. The root is observed with `waitid(WNOWAIT)` and kept unreaped
  until group signaling is finished, preventing PID/PGID reuse during cleanup. Pipe EOF
  does not skip the descendants' TERM grace. Future cancellation also kills the group.
- Bounded incremental NDJSON projection with UTF-8/JSON/type/session checks, sticky
  sanitized errors, unknown-event counters, terminal-result checks and fixed marker
  recognition. Partial/assistant counters are **not deduplicated transcript messages**.
- Smoke success requires matching init/result UUIDs, declared empty tool inventory,
  successful result subtype/flag, expected fixed answer, and clean process exit.
  The fresh turn supplies a memory code; resume asks for that code without resending it.
  Exit 0, an input flush, or arbitrary successful text alone is insufficient.

This code is an example target with tests enabled in `crates/pipkin-engine/Cargo.toml`.
It does not register an API adapter, modify runtime admission, migrate a database, offer
new desktop settings, or replace existing credentials. No new dependencies are required.

## Running the experiment

### No inference

```sh
cargo test -p pipkin-engine --example claude_cli_probe
cargo run -p pipkin-engine --example claude_cli_probe -- probe --trust-executable
```

Use `--executable /absolute/path/to/claude` when GUI PATH discovery is unsuitable.
Probe runs version/help in a private temporary directory, not the user's project.
No arguments or `--help` prints instructions and does not spawn Claude.

### Account lookup (separate opt-in)

```sh
cargo run -p pipkin-engine --example claude_cli_probe -- auth --trust-executable
```

This asks the CLI for status and may refresh its own login; it makes no model request.
Neither probe nor auth is a promise of plan-limit billing. Unexpected auth methods,
malformed output, configured alternative routing, or failed safety flags block smoke.

### Live qualification (permission required before running)

Use a dedicated scratch project, a newly generated UUID for the first turn, and the same
UUID/project/config for the second. Never retry/resume a timed-out or uncertain turn.

```sh
# Set SCRATCH to an absolute scratch-directory path and SESSION to a fresh UUID.
cargo run -p pipkin-engine --example claude_cli_probe -- smoke \
  --trust-executable --trust-project --allow-live-usage \
  --cwd "$SCRATCH" --session-id "$SESSION" --model sonnet
# Only after the first turn is confirmed successful:
cargo run -p pipkin-engine --example claude_cli_probe -- smoke \
  --trust-executable --trust-project --allow-live-usage \
  --cwd "$SCRATCH" --resume "$SESSION" --model sonnet
```

These calls **can consume included allowance or already-enabled extra usage**. The
harness does not change spending settings and cannot impose a reliable subscription
billing cap. Keep default CLI account configuration; do not replace its config directory
with an empty one and inadvertently change authentication. Do not run live mode in CI.

The fixed candidate flags are:

```text
--safe-mode --restricted
--strict-mcp-config --mcp-config {"mcpServers":{}}
--print --tools "" --disallowedTools mcp__*
--permission-mode dontAsk --permission-prompts none
--input-format text --output-format stream-json --verbose --include-partial-messages
--no-chrome --disable-slash-commands
--model <explicit alias> (--session-id <UUID> | --resume <UUID>)
```

`--bare` is intentionally absent: it disables OAuth/keychain auth. Empty
`--setting-sources` semantics were not established by the reviewed docs, so the candidate
uses documented `--restricted` instead. Managed policy/hooks still apply; these flags
are not an OS sandbox or proof that no startup code will execute. Their **combination
has now passed a two-turn live smoke check, but adversarial customization isolation and
permission hosting remain unverified**.

## Isolation and permission qualification

```sh
# No model requests; uses a disposable HOME/config with no copied credentials.
cargo run -p pipkin-engine --example claude_cli_probe -- isolation --trust-executable
```

The fixture has startup hooks in disposable user, shared-project, and local-project
settings, an explicit plugin with startup hooks, and a project MCP server that marks
its launch and exits. Fake user settings approve only that sentinel server, so MCP
positive controls do not silently stop at an approval gate. No real settings are edited.

Observed on CLI 2.1.296:

| Control/surface | Ordinary fixture control | Safe-mode policy | Restricted hook policy |
| --- | --- | --- | --- |
| `--init-only`: user/project/local hooks | All three executed | None executed | None executed |
| `--init-only`: explicitly supplied plugin | Executed | Suppressed | Executed (host-explicit plugin remains allowed) |
| `mcp list`: approved project MCP sentinel | Launched | Not launched | Not launched |

These are **startup/discovery observations**, not yet proof of all print-mode behavior,
installed-plugin configurations, or administrator policy. The live hook route never
passes `--plugin-dir`; it must not accept arbitrary extra CLI arguments. The initial MCP
control lacked explicit approval and did not run the sentinel; that unexercised result
was not treated as an isolation pass. Adding approval in the *fake* user profile calibrated
it before the successful comparison.

### Why qualify a hook instead of assuming an MCP wire contract?

The [archived official MCP example](https://web.archive.org/web/20250806012109/https://docs.anthropic.com/en/docs/claude-code/sdk#custom-permission-prompt-tool)
documents `tool_name`, `input`, optional `tool_use_id`, and a JSON-stringified allow/deny
payload inside MCP text content. That is historical evidence, not a version-pinned
2.1.296 contract. Current docs fully specify the `PermissionRequest` hook's stdin/stdout
schema, so this experiment uses that supported alternative. No SDK wire protocol is inferred.

The hook route is separately configured with `--restricted`, strict empty MCP configuration,
explicit `--settings`, `--permission-mode default`, and `--permission-prompts none`.
It does **not** use `--safe-mode` (which disables the hook) or `--bare` (which disables
subscription auth). `permissions.ask: ["Write"]` exercises the hook; earlier managed
permissions/hooks may still affect behavior and are not bypassed.

The fixture hook:

- Has a 5-second CLI timeout and `onFailure: "block"` (requires CLI 2.1.295+).
- Reads at most 64 KiB with its own 2-second stdin deadline; malformed input/timeouts
  return explicit deny JSON. Merely exiting 2 is **not** a PermissionRequest denial.
- Checks the expected session UUID, canonical scratch cwd, event, Write tool, exact target
  and exact content; no `tool_use_id` is assumed because this hook does not provide one.
- Records a validated-request marker separately from generic rejection, preventing a
  mismatched request from falsely qualifying the intended Write denial.
- For allow-once, atomically consumes a fixed private grant before answering. Prior denials,
  repeat grants, unexpected fields, symlinks and invalid audit state fail closed. It returns
  no `updatedInput` or `updatedPermissions`; no persistent permission rules are granted.
- Verifies the actual target's existence/content separately from the approval decision.
  Only `NotFound` establishes absence; permission/I/O errors mean uncertainty.

Live modes require **new explicit permission** before invocation. The earlier authorization
for two no-tools requests does not cover these Write tests. Each mode makes one model turn,
with no automatic retry; scratch files are owned by the fixture and removed afterwards.

```sh
# Only after approval; SESSION_DENY and SESSION_ALLOW must be distinct fresh UUIDs.
cargo run -p pipkin-engine --example claude_cli_probe -- permission-deny \
  --trust-executable --trust-project --allow-live-usage --session-id "$SESSION_DENY" --model sonnet
cargo run -p pipkin-engine --example claude_cli_probe -- permission-allow \
  --trust-executable --trust-project --allow-live-usage --session-id "$SESSION_ALLOW" --model sonnet
```

**Live permission results passed after new explicit authorization (two CLI turns):**

- Deny: the exact correlated Write reached the hook, explicit denial interrupted the
  turn, and the target was absent. CLI exit 1 and `result_success:false` are the expected
  denied-task outcome, **not** a model-success claim. Qualification passed; 2,852 ms.
- Allow once: the exact Write reached the hook, the single grant was consumed, and the
  target held exactly the specified content. CLI exit 0 and successful result; 4,552 ms.
- Both reported CLI-account auth, an inventory containing only Write, and no unexpected
  project-hook/MCP markers. The real user settings were not modified. Scratch files were
  fixture-owned and cleaned up. Times include cleanup grace and are not benchmarks.
- `fixtures/claude-cli-probe/2.1.296/permission-summary.json` retains only aggregate,
  sanitized observations. One CLI turn may contain multiple model exchanges; two turns
  were authorized and run, with no extra live retry or billing-setting change.

This establishes the basic hook round-trip on this version/account, not a general
person-facing approval broker. Controlled pending-approval cancellation is now measured
below; other live failure/timeout paths, managed precedence, other tools, and stale
approval across resumed runs remain unverified.

## Lifecycle and interruption qualification

The standalone example now has a private, versioned SQLite ledger (`lifecycle.rs`),
**not** a production store migration or desktop approval UI:

- Commits admission and launch intent before execution; `Running` means the process may
  execute, not that a remote request was acknowledged. WAL/FULL durability, explicit
  reopen/recovery, bounded busy waits and exact private schema checks.
- Approval identity is `(CLI session, generation, run, host request ID, argument fingerprint)`.
  The fingerprint binds only the validated, fixed scratch Write. No prompt or credential
  content is stored. Production conversation identity/routing is not wired yet.
- Decisions expire, cannot be replayed, and require atomic one-shot consumption. Linux
  parent/hook deadlines share BOOTTIME (including suspend); the helper also bounds its wait.
  A consumed grant is already issued and **cannot be undone** by Stop.
- Stop revokes Pending/Granted approvals and invalidates context before releasing the
  independent cancellation token. The supervisor also fences approvals before its first
  group signal on timeout, protocol/pipe/output failure, root-exit cleanup or future drop.
  Failed revocation still proceeds with cleanup, but cannot qualify a settled interruption.
  The synchronous fixture fence has a SQLite busy timeout, not a hard filesystem-I/O
  deadline; production worker supervision still needs separate design/qualification.
- Root reaping, pipe drainage, cleanup deadline and approval revocation are separate
  report facts. None establishes containment of escaped descendants.
- Recovery turns unfinished runs into Unknown, revokes unissued approvals, and blocks
  resume/queued delivery. It never starts a process, replays a tool, or resends a prompt.
  Interrupted/Unknown bindings require a **fresh-session, user-reviewed handoff**; no
  CLI transcript editing or assumed removal of stopped inputs.

**Offline measured:** 81 example tests pass. Real SIGKILL tests cut a synthetic executor
at eight durable boundaries: admission, launch intent, submission receipt, pending
approval, granted approval, consumed grant, external effect, and confirmed completion.
Reopening preserves effect/receipt counts without replay, rejects late replies/completion,
and permits resume/queue eligibility only for confirmed completion. Separate child-process
controls exercise actual hook waiting, allow/write calibration, pending cancellation,
absent-host expiry, and pre-signal ordering/fence failure. These are **not real CLI crash
recovery or print-mode timeout/disconnection evidence**.

New live mode (one turn, fresh UUID, generated scratch only):

```sh
# Requires fresh explicit live consent; never auto-retry or reuse an interrupted session.
cargo run -p pipkin-engine --example claude_cli_probe -- interrupt \
  --trust-executable --trust-project --allow-live-usage --session-id "$SESSION_INTERRUPT" --model sonnet
```

It waits for the exact pending Write, revokes it, attempts a stale allow/consume (both must
fail), stops the owned group, verifies no target/grant, reopens its ledger and checks that
continuation, late completion and queue eligibility remain blocked. The driver never
approves the Write.

**Authorized live interruption passed (one CLI turn):** exact pending request observed;
late allow/consume rejected; no target or grant; Write-only inventory matched the fresh
session. Stop reported Cancelled, root reaped, pipes drained, approval revocation confirmed,
and no cleanup timeout. The reopened binding remained Interrupted/invalid with resume,
queue delivery and late completion blocked. Elapsed 2,773 ms includes cleanup grace.
No terminal model result was projected; this is **interruption success, not model success**.
Sanitized evidence: `fixtures/claude-cli-probe/2.1.296/interruption-summary.json`.
The one-turn authorization is consumed; no live retry, resume, shell tool, real-project edit,
or billing-setting change was performed. Billing remains unverified.

The ledger is deleted with its private fixture. This demonstrates a recovery contract,
not durable production conversation storage. Manual `smoke --resume` remains a separate
transport experiment, not a safe-resume implementation.

## Observed / measured

- Installed executable discovery, `claude --version`, and `claude --help`: **2.1.296**.
- No-inference probe successfully resolved that CLI and found all required help flags;
  no known environment routing overrides were present. Redacted output:

  ```json
  {"mode":"probe","cli_version":"2.1.296","missing_flags":[],"routing_override_names":[],"billing_verified":false,"launch_policy_verified":false}
  ```

- **Earlier no-tools live check passed (two CLI turns):** the CLI reported a Claude-account
  login under the same safety arguments/environment used for smoke. Fresh Sonnet-alias
  request: expected marker, zero declared tools, explicit UUID matched, exit 0; 4,553 ms
  including supervisor cleanup grace. Resume: same UUID, remembered the original code
  without it appearing in the second prompt, zero declared tools, exit 0; 2,632 ms.
  Each run contained two well-formed unknown envelopes, counted but not interpreted.
  No API fallback, billing setting change, or extra CLI turn was made in that qualification.
- Sanitized aggregate observations: `fixtures/claude-cli-probe/2.1.296/live-summary.json`.
  This is **not a raw NDJSON replay fixture**: account/session identifiers and raw CLI
  content are not retained. Times are one sample each, not a latency benchmark.
- Synthetic decoder and shell-child tests cover arbitrary chunk/UTF-8 boundaries,
  size limits, malformed/truncated records, unknown events, duplicate/mismatched IDs,
  missing/error terminal results, marker recognition and redaction; cancellation before
  spawn/during execution, ignored TERM, pipe floods, simultaneous stdout/stderr, retained
  process identity, closed-pipe descendant cleanup grace, stdin-as-data, missing
  executable and nonzero exit after successful result.
- All test streams are **fabricated fixtures inline in the Rust tests**, not captured
  Claude responses. They establish local contract behavior only, not CLI parity.
- **Passed at Phase 0 completion:** `cargo test -p pipkin-engine --example claude_cli_probe` — 81 tests
  (transport: 33; isolation/permission slice: 58). New coverage includes correlated exact-request decisions,
  one-shot/concurrent grants, prior-denial rejection, symlink/foreign evidence refusal,
  permission stdin bounds/deadlines, opt-in modes, Write-only inventory, and false-pass
  prevention for failed isolation runs or metadata errors. Lifecycle coverage adds scoped
  deadlines/one-shot decisions, actual killed-host fault cuts, pending-approval interruption,
  pre-signal fencing, and cancellation racing with already-failed cleanup.
- **Passed:** calibrated non-inference `isolation --trust-executable` controls on 2.1.296;
  sanitized evidence: `fixtures/claude-cli-probe/2.1.296/isolation-summary.json`.
- **Passed:** `cargo test --workspace && cargo clippy --workspace --all-targets && cargo fmt --all --check` (exit 0). The previous planning-turn disk-I/O failure did not recur; its root cause was not established or repaired here.
- Read-only code review identified numeric-PGID reuse after reaping, descendants losing
  their grace after closing pipes, and insufficient smoke-answer validation. These were
  corrected with retained leader identity, explicit grace settlement, and fixed fresh/
  memory-recall markers; regression tests cover each change.
- The isolation/permission review caught three additional false-pass risks: a failed MCP
  invocation mistaken for suppression, an unrelated denied Write mistaken for the intended
  request, and metadata errors mistaken for target absence. Clean-run controls, validated
  exact-request evidence, and NotFound-only absence checks now cover those cases.
- Lifecycle review found cancellation could reach supervision before revocation, and
  autonomous failure cleanup had no fence. An independent token plus an owned pre-signal
  fence now covers those paths. A second regression ensures cancellation during already-
  failed cleanup cannot relabel timeout/protocol failure as Interrupted.
- A short-grace signal-order test initially failed when its foreground sleep deferred the
  shell's TERM trap. The ordering fixture now uses a builtin loop; eight repeat runs and
  the subsequent full workspace/test/Clippy/format gates passed. No additional live turn
  was used to diagnose or fix this test fixture.

## Phase 1 — storage and disabled-backend foundation

**Implemented, not an enabled executor.** Schema 21 adds immutable execution identity,
nonsecret CLI bindings and separate external run/action tables. Existing sessions, including
imports, migrate to NativeAgent with the existing backup/future-schema refusal behavior.
Unknown backend values remain readable but disabled. Protocol 32 deliberately rejects older
peer handshakes; old serialized summaries still default to NativeAgent.

External admission is idempotent but a duplicate does **not** authorize another launch.
State transitions compare `(session, generation, op, expected state)`. Startup atomically
marks unfinished external runs Unknown and invalidates their bindings, without replay.
Interrupted/Unknown outcomes fence the generation; stopping/unfinished work cannot admit a
follow-up. `context_valid` alone is not resume or execution authorization. Completion requires
caller-verified final result/root exit and rejects Observed/Running/Unknown action metadata.
Actual supervision, permission revocation and CLI-history reconciliation are not implemented
by this storage API.

Removal deletes readable history/binding/ledger records, not the CLI's own history. Minimal
permanent UUID ownership receipts remain in this store: a previously bound UUID cannot be
reintroduced as a fresh context after removal, including never-launched/completed bindings.
Paths are shape-validated only; no filesystem qualification, account probe or credential I/O
occurs in these constructors. CLI actions have explicit external provenance and never become
native `tool_calls` intents.

Engine RPC admission and native execution/recovery sinks refuse CLI/unknown backends, including
native-looking children of disabled roots. Saved compaction requests, pending native tool
calls, retry state and follow-ups cannot bypass this boundary. External sessions do not pin
or initialize native extensions. Core/adapter snapshot gating uses existing conversation and
generation guards, refuses operation-scoped context changes, and blocks delayed intent/queue/
goal delivery. History, rename, removal and creation of new native conversations remain usable.
The gate is coarse; full capability/status and external transcript projection remain future work.

**Offline measured:** protocol/core tests, 113 native adapter tests and four execution-boundary
runtime tests passed (job #27). Store-filtered tests passed: 67 tests. Integration fixtures
use Scripted providers, temporary stores and deliberately foreign native recovery records;
no installed CLI or live model is used. They cover saved compaction executor refusal, no native
tool/provider execution or target-file mutation on recovery, extension suppression, old-peer
refusal, readable history and continued native conversation execution.

**Passed (jobs #28 and #29, exit 0):** `cargo test --workspace`,
`cargo clippy --workspace --all-targets`, `cargo fmt --all --check` and `git diff --check`.
The workspace run includes the 81 Phase 0 example tests. No new live CLI request was used.

Read-only review found unsafe UUID reuse after removal, unresolved actions being stranded by
completion, and a recovery path checking only the child identity. Permanent receipts, atomic
completion refusal and an ancestor-aware recovery guard now have regression tests. The stale
protocol-31 test assertion was updated to 32. A bounded follow-up source review found no
remaining issue in those fixes. Native-only guards check identity independently of temporary
availability, so enabling an external executor later cannot enable native execution sinks.

No CLI create/select RPC, CLI runtime process, production approval UI or API fallback is enabled.
No additional live inference, billing change, credential extraction or installation was performed.
No new desktop/IME/accessibility validation is claimed.

## Phase 2 — shared transport foundation (runtime still disabled)

The qualified supervisor/summary decoder now live under
`crates/pipkin-engine/src/execution/claude_cli/` and are shared by the probe. The original
81 tests are preserved as 30 library and 51 example tests. A bounded, nonblocking stdout
observer fails closed on rejection/panic and still fences/signals/drains/reaps the owned group.
Synchronous callbacks must not block; the fixture SQLite revocation callback is still not a
production permission broker. Process groups are still not a sandbox or crash/orphan proof.

Typed preflight checks an explicitly approved executable/project candidate, exact version and
required flags, and the CLI's public auth status. It rejects routing/executable environment
overrides by presence before narrowing the environment; preserves selected HOME/config and
proxy/CA settings; never opens CLI credential files. Public account/organization metadata is
hashed into a nonsecret binding marker rather than logged. Candidate/config directory changes,
binding mismatch and expired preflight refuse launch. Filesystem rechecks are not a race-proof
sandbox or atomic lock against concurrent external login/configuration changes.

The only launch policy is currently **chat-only (`--tools ""`)**, not editing or general read-tool
support. Prompts are stdin data, sessions are explicit, and no arbitrary flags or API fallback
are accepted. The new pure decoder emits bounded, provisional external display events; it
reconciles partial/final text without appending the terminal answer twice. Tool observations
cannot become native tool calls. Strict new projection/auth schemas remain live-unverified.

**Passed:** 78 shared CLI module tests and 51 example tests (job #32); full workspace tests,
Clippy all targets, formatting and diff checks (job #33). A fake executable exercises actual
preflight/launch/supervision/projection, including wrong-session output, queue pressure, and
successful terminal output followed by a hung process. An initial integration assertion counted
an empty block-start as duplicate text; it now checks the assembled answer exactly once.
Read-only review found no blocking issue within this foundation's scope.

No installed CLI, live model, real account or desktop was used for this increment. Runtime
admission/settlement, durable transcript projection, opt-in setup and permissions remain ahead.

## Remaining unverified / blocked gates

1. **Billing:** the CLI-reported Claude-account route and successful requests were
   observed, but no before/after usage-dashboard evidence was obtained. The person's
   participation is needed to establish actual allowance/spend behavior, which may
   remain inconclusive. Token counts, cost estimates and `authMethod` are not billing proof.
2. **Broader launch behavior:** the exact no-tools candidate, stdin framing, accepted stream
   projection and one completed-turn resume passed on 2.1.296. Normal-terminal comparison,
   other accounts/models/versions, full tool-event projection and usage projection are
   unverified. A controlled live stop at pending Write approval now passes; interruption
   during other stages of CLI execution remains unverified. Raw wire capture/replay is not provided by the redacted summary.
3. **Configuration isolation:** controlled startup hooks and MCP discovery now have
   calibrated positive/negative evidence. Full print-mode/user-plugin coverage and
   managed-policy cases remain open. Restricted mode permits host-explicit plugins;
   arbitrary caller flags must never be admitted. Do not bypass administrator policy.
4. **Permission hosting:** historical MCP schema recovered, but 2.1.296 compatibility
   remains untested. The current documented PermissionRequest hook passed exact live
   Write deny/allow-once checks. Scoped approval expiry/cancellation/disconnection now
   have offline fixture coverage, plus live pending-Write cancellation. General UI approvals,
   live expiry/disconnection/failure, and every-tool coverage remain unimplemented or unverified.
   `canUseTool` SDK types alone are not wire-protocol evidence.
5. **Recovery and context:** the disposable ledger passes synthetic crash/reopen and
   unsafe-binding gates. Phase 1 now persists bindings and fences unfinished external work,
   but runtime CLI supervision/reconciliation remains unimplemented. The probe's `--resume`
   is manual qualification only. Neither path can reconcile real CLI crashes,
   confirm missing/edited CLI history, or retire stopped inputs from CLI context.
6. **Process containment:** the Unix implementation covers the owned process group, not
   descendants that deliberately `setsid`/detach. Root exit and pipe closure at live
   pending-approval cancellation were observed; graceful internal cleanup and real engine
   crash/orphan behavior remain unverified. No process-tree containment guarantee, Windows
   support, macOS run, desktop test, or latency/billing performance claim is made.
7. **Product release:** CLI product-use/branding conditions and current terms must be
   rechecked before distribution; this experiment does not establish legal approval.

Phase 0 remains open. The Phase 1 storage/off-gate foundation is implemented; full external
projection/capabilities and the rest of Phases 2–5 remain ahead. Do not wire the spike into runtime
execution until the applicable launch/auth/trust and permission gates are resolved.

## Documentation evidence (not live verification)

Official docs reviewed on 2026-10-13:

- https://code.claude.com/docs/en/headless.md — stdin, streaming, print-mode trust caveats,
  `--bare` auth, explicit resume, interrupted-context caveats.
- https://code.claude.com/docs/en/cli-reference.md — CLI flags and auth status.
- https://code.claude.com/docs/en/authentication.md — credential precedence; a key in the
  environment wins in print mode; profiles/federation/gateways also affect routing.
- https://code.claude.com/docs/en/settings-reference.md — managed policy/hook precedence
  and explicit approval of fixture project MCP servers from disposable user settings.
- https://code.claude.com/docs/en/hooks.md#permissionrequest — current command-hook input,
  allow/deny output, absence of tool_use_id, and onFailure:block semantics.
- https://web.archive.org/web/20250806012109/https://docs.anthropic.com/en/docs/claude-code/sdk#custom-permission-prompt-tool — historical official MCP permission-tool example.
- https://code.claude.com/docs/en/agent-sdk/typescript.md — documented stream envelopes;
  not proof of a Rust host control protocol or permission-tool wire schema.
- https://code.claude.com/docs/en/legal-and-compliance.md — unmodified binary with
  user-owned login versus intermediating subscription credentials; product/branding conditions.
