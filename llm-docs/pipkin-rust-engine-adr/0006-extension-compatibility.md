# ADR 0006: Extension compatibility

**Current owner decision (2026-10-11):** Pi extensions are explicitly not required for Pipkin.
The [plan amendment](../pipkin-rust-engine-plan.md#owner-scope-change--2026-10-11-pi-extensions-are-not-required)
supersedes the extension-preservation and release-blocking decisions below. Keep existing
native implementations, but do not build a TypeScript compatibility layer, require package
ports, add remote extension services, or implement reload for cutover. Fifteen extension-specific
ledger rows are OS; built-in tools/providers/subagents and user-data preservation stay in scope.
The following record is retained as historical design/evidence, not an active compatibility gate.

**Status:** Proposed; implementation updated on top of `67ba56f` (2026-10-11). Native Rust
and process extension hosts exist (`E/extensions`, `docs/native-extensions.md`), with tested
ports of the questions and greeting examples. These are not TypeScript package compatibility:
custom remote services and hot reload remain missing. Native sticky/session profiles and
input-result waits are implemented; general submission identities remain open. The importer now validates the selected Pi coordination directory's server/session
profiles, honors explicit empty per-session overrides, and refuses nonempty or uncertain
selections before activation (IMP-PLUGIN-STATE). Profiles are rechecked before writing, and
accepted inspection data/hashes are retained in the import manifest. See the import/retry
evidence record for tested cases. Pi remains the continuity path for existing facet packages;
the no-feature-loss completion gate is unchanged.

## Context

What the baseline actually loads (audit `extensions-subagents.md` §5):

- Pipkin's `--pi-extension <path>` becomes `pi server -e <path>`
  (`crates/pipkin-app/src/controller.rs`; `adapters/pi/engine.rs:174-199`). That loads a
  **Chord facet plugin package** — not a classic Pi extension.
- Facet packages are TypeScript bundled by esbuild into CommonJS (`chord/src/node/bundle.ts`)
  and loaded by the session worker (`session` entry) and by a terminal client (`tui` entry,
  which Pipkin never runs) (`ce/plugins/bundled.ts:12-36`). A session facet can provide new
  remote services, use `AgentController` (prompt/steer/followUp/cancelQueued/abort/compact/
  lookup/waitForPrompt), `History.page` and `UiRequestHost` (select/confirm/input/notify/
  setStatus) (`ce/plugin.ts:1-28`). **Facets cannot contribute model-visible tools, prompt
  sections, hooks or providers** (no service exposes the `Registry`).
- Selection is sticky: omitting `-e` restores the server profile
  (`plugin-packages-<serverId>.json`), each session pins the selection it first opened with,
  and a running session refuses a different selection (`ce/plugins/package.ts:21-66`,
  `ce/server.ts:573-602`, `session-worker-manager.ts:136-142`).
- **Not loaded on the durable/server path:** classic extensions (`ExtensionAPI`,
  `registerTool`, `registerProvider`, hooks, commands), MCP servers, `codemode`, `tool-search`,
  `llama.cpp`, package-installed skills/extensions (§5.4: no file under
  `coding-agent/src/experimental/` calls the classic loader). The durable `Extension` API
  (tools/sections/hooks/wraps/tasks) is used only for the hard-coded `CodingTools`, `pi-prompt`
  and `subagent`.
- Pipkin presents only `pi.ui-requests` (select/confirm/input/notify/setStatus) from
  extensions; TUI widgets and plugin slash commands are already unsupported
  (`docs/extensions.md`).

A Rust engine cannot run arbitrary TypeScript facets without a JavaScript runtime, and a
generic JS interpreter is not Node compatibility (plan §2 "Providers and extensions are
critical-path work").

## Decision

1. **Native extension contract (in-process, Rust):** a versioned trait set covering what the
   baseline's built-in durable extensions use — tool registration (with explicit replay policy
   and output limits), prompt sections, generation/tool/compaction hooks — plus the facet-side
   capabilities a third party can actually use today: question host
   (select/confirm/input/notify/setStatus with the reference limits: 120 s default timeout,
   `0` = indefinite, 20 open questions, 20 notices, 10 000-char input, 300-char titles),
   session control (prompt/steer/follow-up/cancel/stop/lookup/wait) and history paging.
   Bundled behavior (coding tools, prompt sections, subagent) is ported to this contract.
2. **Out-of-process extension protocol (for third-party code):** extensions run as separate
   owned processes speaking a framed, versioned protocol to the engine (same framing as
   ADR 0002), with per-extension state persisted in `kv` and preserved byte-for-byte on import.
   This is language-neutral; it does not by itself make existing TypeScript facets run.
3. **Existing TypeScript facet packages need Node.** Running them natively would require
   either a Node runtime shim speaking (2) — which is a legacy bridge and fails the no-Node
   completion gate — or a port of each package to (1)/(2). Therefore:
   - rows `EXT-FACET-*` are **blocked** for native and served by the **legacy bridge** (the Pi
     engine via `--pi-extension`) during migration;
   - a conversation whose session has a non-empty facet selection stays on Pi until its
     packages are ported or the owner approves a scope change; the importer records the
     selection and refuses silent activation;
   - completion of the conversion is not claimed while any working baseline facet package
     lacks a native port (plan §2: a narrowed extension subset is not full parity).
4. **Classic extensions, MCP, codemode, tool-search, llama.cpp:** **not present in the
   baseline** (audit §5.4 evidence above). They are not added as parity requirements; adding
   them later is a separate product scope.
5. **Spike (RE-05) must prove,** with a representative facet (the upstream
   `pi-example-questions` and `pi-example-plugin` packages), that (1)/(2) can express
   questions, custom remote services, session control and reload, and must measure what a
   Node bridge would cost if the owner chooses to keep one.

## Native profile implementation (2026-10-11)

Protocol 7 / schema 5 implements sticky engine defaults, first-acquisition session pins,
explicit empty selections and active-selection conflicts for native extension ports.
`PrepareExtensions` validates executable identities/unique IDs and persists without activation;
`--engine-clear-extensions` repairs or clears the default without changing existing pins.
Session acquisition and profile selection share a registry lock. Failed first-pin writes keep
the session closed until explicit reopen persists the same selection. Imported plugin-free
roots pin empty in the import transaction. Unsupported Pi facet selections remain refused.

Thirteen observations captured by executing the pinned Pi profile helpers agree with native
protocol results. Native tests additionally cover conflicts, retirement, stale questions,
concurrent preparation/acquisition, storage faults and schema upgrade. See
[profile evidence](../pipkin-rust-engine-evidence/extension-profiles.md).
This implements the selection mechanism for native ports; TypeScript execution, custom remote
services and hot reload remain separate blockers. Input `waitForPrompt` was subsequently
implemented with protocol 8; see [wait evidence](../pipkin-rust-engine-evidence/prompt-waits.md). The tool/section/hook contract
in decision 1 remains a proposal, not a claim about the current native extension API.

## Consequences

- Extension compatibility remains a release blocker for native default and Pi retirement
  until each baseline facet package is ported or explicitly descoped by the owner.
- The native contract is wider than the facet API (it can add tools/sections/hooks), which is
  a capability addition, not a parity requirement; it must not be used to claim facet parity.
- Plugin TUI facets and slash commands stay unsupported, as today.

## References

- Plan §2 P14/P15, "Providers and extensions are critical-path work"; §11 RE-05, RE-16.
- Audit `extensions-subagents.md` §5.1–5.6.
- `docs/extensions.md` (product contract; its "fork `../pi-fork/pi`" location and
  "stable extension" wording are stale per audit §5.6).
- Pi-side fixtures: `crates/pipkin-app/src/adapters/pi/e2e.rs`
  (`the_engines_extension_question_service_is_there_and_refuses_an_answer_to_nothing`,
  `an_extension_in_the_engine_asks_three_questions_and_the_answers_reach_it`,
  `declining_ends_the_question_and_a_lost_connection_does_not_leave_it_answerable`).
- Native: `crates/pipkin-engine/src/extensions/` (trait host, process host, question host and
  example tests), with supported and missing features in `docs/native-extensions.md`.
