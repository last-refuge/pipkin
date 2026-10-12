# Prompt resource lifetimes and provider reachability

2026-10-11, following `e92c4f1`. Reference pin:
`d2a311097cbcf669e699479587332ae3988a49d0`, including Pipkin's two shipped overlays.
Protocol 9 / schema 8 unchanged. No default switch or release qualification is claimed.

## Fixed resource snapshot mismatch

The previous native cache stored rendered sections per session and keyed them by cwd,
tool names and configured skill paths. Changing the offered tools therefore reread project
instructions and skills. A newly started child also loaded a new snapshot, potentially
seeing different instructions from its parent after a filesystem edit.

Pinned `experimental/durable/prompt.ts:26-39` caches resources per cwd in the prompt
registry. `experimental/session-worker.ts:778-789` creates one registry per root worker,
shared by its child conversations. Sections still render for each request's selected tools.

Native now keeps a resource snapshot per cwd within a live root/child group. Tool changes
rerender sections from that snapshot. Independent root sessions never share snapshots,
even in the same directory. A child keeps the snapshot alive when the root alone retires.
When the final live member retires, the registry releases the group mapping; a new lifetime
loads edited files. Retiring hosts can finish teardown without leaking old resources into
the new group. Engine restart also loads a new snapshot. Weak ownership and regression
assertions verify release of the old allocation; this is not a sustained heap measurement.

PRM-AGENTS-CACHE moves PA → NC. This preserves resource-cache semantics within native's
existing managed session lifecycle. It does not claim the same process topology, exact
worker idle timeout, or identical wall-clock retirement timing as Pi. Instruction/skill
discovery rules and actual skill-file reads by tools are unchanged; only prompt metadata
and instruction snapshots are cached.

Measured tests:

- `runtime::tests::prompt_resources_stay_frozen_across_tool_changes_and_subagents`:
  mutate AGENTS.md and skill metadata after the first turn, disable/re-enable subagents,
  execute a real subagent tool, and verify all parent/child requests retain original
  resources. A separate root and an engine restart see the edits.
- `runtime::lifecycle::tests::prompt_resources_follow_the_live_root_group_and_are_freed_after_retirement`:
  retire/reopen the root while a child remains live; retain original instructions.
  Retire the entire group, reopen and see edited instructions; the old cache allocation
  is freed and its registry mapping removed.
- `runtime::glue::tests::prompt_resources_match_the_executed_reference_cache_lifetimes`:
  seven cases compare native resource snapshots with original `createPiPrompt` behavior
  across tool changes, child/shared cwd, another cwd, independent roots and restart.
  `scripts/capture-prompt-resources-reference.mjs` verifies the pin and clean source,
  rewrites import paths only, and uses controlled resource loaders and a section renderer.
  Original cache function bodies execute; full prompt text/filesystem discovery/provider
  execution are outside that differential. Source hash and output are retained in
  `fixtures/native-prompt-resources/reference.json`.

The corrected regression failed before the production fix at the original-instruction
assertion (`/tmp/pipkin-prompt-resources-reproduced.log`) and passed afterward. Earlier
test development caught temporary-lifetime compile errors and a test provider that failed
to skip positional system entries when identifying the latest user message; both fixture
errors were corrected before reproducing the real bug. The first reference capture was
blocked by sandbox subprocess restrictions, then succeeded with local execution permission.
Targeted logs: `/tmp/pipkin-prompt-resources-{fixed,lifecycle,reference}.log`.
An initial workspace run exposed an existing Kimi OAuth test race: accelerated mock
authorization completed before its polling assertion observed the device code. The mock
now remains pending until the test observes the code, then authorizes; header-auth and
three-attempt refresh assertions remain intact. The failure is retained in
`/tmp/pipkin-prompt-audit-workspace.log`.
Final required checks: `/tmp/pipkin-prompt-audit-verified-{workspace,clippy,fmt}.log`
(results in progress record).

## Provider paths absent from the shipped durable coding registry

Searches included ignored files with `rg --no-ignore`, and inspected registrations,
entry points and both overlays; absence of a search match alone was not the conclusion.

- **ADP-OR-IMAGES, ADP-CLASSIFIERS, PRV-typesafe → NP for the shipped path.**
  `experimental/durable/harness-setup.ts:52-56` installs coding tools and the prompt;
  `durable/src/tools/index.ts:19-22` installs read/write/edit/bash. Pipkin's subagent
  overlay adds delegation, and onboarding adds credential management. None installs
  codemode. Generation (`durable/src/harness/generation.ts:402`) uses `streamSimple`;
  compaction (`harness/compaction.ts:174`) uses `completeSimple`. Image-generation and
  classifier methods in `core/model-runtime.ts:783-811` are called by the classic
  codemode execution bridge (`extensions/codemode/execute.ts:656,665`), not these tools.
  The durable function named `classify` at `generation.ts:419` classifies a completed
  assistant's stop reason; it does not call an external classifier model.
  Native retains image/classifier catalog metadata and rejects it as a chat selection.
  Chat models with image inputs, read-tool images and attachment migration remain required.
- **ADP-ATTRIBUTION and SET-TELEMETRY → NP for the shipped path.**
  `mergeProviderAttributionHeaders` is called by classic `core/sdk.ts:331`; that SDK
  wraps stream options with `transformHeaders`. The durable worker constructs
  `ModelRuntime` directly (`session-worker.ts:778-789`) and its `createHarnessSettings`
  forwards timeouts/retries, without this transform. Installation reporting is called
  by classic `modes/interactive/interactive-mode.ts:1337-1356`, not the server worker.
  Neither Pipkin overlay installs the transform or reporter. Native adds no tracking
  service or synthetic Pi attribution. Provider/model-configured headers and special
  authentication/billing headers remain required and unchanged. ADP-UA is separate.

These NP dispositions concern Pipkin's frozen shipped path; the APIs do exist elsewhere
in upstream Pi. No required native behavior was removed and no extension capability was
added. Provider-wire identity, OAuth comparisons and service removal/thinking-cycle audits
remain separate work. Live providers, installed workflows, sustained use and platform
qualification remain open.
