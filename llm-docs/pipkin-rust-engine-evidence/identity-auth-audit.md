# Product identity, built-in help and OAuth reachability

2026-10-11, following `291e854`. Protocol **11**, schema **9**, unchanged.
Native remains opt-in. This resolves the final pending-audit dispositions; it does not
qualify every provider, installed workflow or release gate.

## Prompt identity and help (D-01, D-02)

The native prompt now names Pipkin. Its documentation section contains the compiled
`docs/native-engine-reference.md` instead of omitting help or pointing at a Pi package.
The reference covers engine/configuration ownership, account/billing separation, resources,
steering/Stop/recovery, compaction, usage, imports, Changes and diagnostics. It is available
offline to roots and children, requires no installation-relative paths, and adds no runtime
filesystem reads. Existing project instructions, skills, tool guidelines, section order and
positional prompt persistence remain intact. Pi SDK/TUI/extension documentation is not a
required product capability under the owner scope amendment.

Measured comparison: `scripts/capture-prompt-identity-reference.mjs` verifies a clean pinned
source at `d2a311097cbcf669e699479587332ae3988a49d0`, then executes its original
`buildSystemPromptSections` and tool contribution constants. Documentation paths are fixed
fixtures; skills are empty and their formatter is stubbed. Eight tool/cwd/context cases are
stored with source hashes in `fixtures/native-prompt-identity/reference.json`.
`context::prompt::tests::native_sections_match_executed_reference_except_documented_product_identity`
compares complete sections after exactly two declared transformations: `pi` → `Pipkin` in
the preamble and the embedded product reference in docs. This is a function-level comparison,
not a live-provider or full-reference scheduler qualification.

`runtime::glue::identity_tests::root_and_child_receive_embedded_pipkin_help_without_a_docs_installation_after_restart`
executes a delegated coding turn, restarts on the same history, and checks the prompt received
at the provider boundary for the parent, child and continued root. Scripted records positional
system messages; the test resolves them through the adapter's system-resolution method before
checking rendered text. Existing prompt goldens and resource-lifetime regressions also run.

PRM-PREAMBLE and PRM-DOCS move PA → NE. The instruction/skill loading behavior is unchanged;
legacy `.pi/skills` and `.pi/prompts` directories remain supported data paths.

## Provider identity (D-05)

The ledger's prior claim that other adapters had no user-agent omitted the shared HTTP
client: `provider/http.rs::client_with` already sends `pipkin/<package-version>` by default.
A native transport identity is the intentional default. Explicit configured/provider headers
retain precedence. `native_user_agent_is_sent_and_explicit_provider_identity_takes_precedence`
measures actual requests to a local HTTP server, including one header only and retained auth.

Codex's explicit `pi (<platform> <release>; <arch>)` user-agent and `originator: pi` remain
wire compatibility values in `openai_codex_responses.rs::base_headers`; existing SSE and
WebSocket goldens assert them. Claude subscription bearer identity, beta headers and separate
cached system identity remain unchanged and covered by their existing tests. This decision
preserves the working account/transport contracts; it neither changes billing nor claims a
live provider has approved a new wire identity. These constants are not Pi/Node dependencies.
ADP-UA moves PA → NE. Classic-only attribution APIs retain their previously audited NP scope.

## OAuth reachability (D-07)

The shipped onboarding overlay explicitly declares only `anthropic` and `openai-codex`
(`packaging/pi-onboarding/provider-auth.ts:4`). Native desktop `SignInProvider` and
`adapters/native/mapping.rs::provider_of` preserve exactly that set. The mapping regression
now includes every additional built-in OAuth provider plus a custom Radius gateway and verifies
that none becomes a desktop subscription choice or replaces a Claude/ChatGPT identity.

The native service implements the additional flows already present in Pi's authentication
library: OpenAI ChatGPT, Copilot, OpenRouter, Kimi, Meta, Radius browser/device and xAI.
They are exposed through native AuthStart/AuthAnswer, independent of the desktop allowlist.
Retain this implemented service capability under the active goal's direction to preserve
features and native advantages. No additional desktop login UI or permission decision is
needed to finish the engine conversion. This resolves reachability, not provider qualification.

The eight login rows move PA → NE. Existing local mock tests cover:

- `sign_in_with_chatgpt_registers_a_client_and_refreshes_with_it` and callback-port refusal;
- `github_copilot_device_flow_reads_models_and_enables_policies` (enterprise domain);
- `openrouter_exchanges_the_code_for_a_key_before_the_page`;
- `kimi_device_flow_header_auth_and_refresh_retries`;
- `meta_device_flow_mints_a_key_and_a_dead_session_fails`;
- `radius_browser_login_against_the_configured_gateway` and
  `radius_device_login_for_a_models_json_gateway`;
- `xai_device_flow_and_cancellation`.

`auth::snapshot_lists_sign_in_providers_with_metadata_only` covers all native service provider
IDs and the custom Radius configuration. Existing auth lifecycle tests cover attempt/challenge
ownership, cancellation, persistence, credential precedence and failed-refresh refusal without
API-key fallback. Stored-credential use and refresh remain required capabilities; rows with
partial or missing refresh evidence are not upgraded to full qualification by this audit.

## Validation and limitations

The capture's first sandbox run could not spawn Git to verify the pin; the permitted rerun
completed. An initial runtime assertion read the legacy leading-system field, which is empty
for durable positional prompts; it was corrected to inspect the adapter-resolved system text.
The reference comparison and HTTP identity assertions passed on their first execution.
Required checks: **1504 passed, 0 failed, 53 ignored**; Clippy and formatting clean.
Logs: `/tmp/pipkin-identity-workspace.log`, `/tmp/pipkin-identity-clippy.log`,
`/tmp/pipkin-identity-fmt.log`. No ignored test is counted as passed.

No live OAuth account, production provider, installed UI, non-Linux platform, performance
budget or full reference engine workflow is qualified by this batch. Required release gates
and the remaining legacy bridge disposition stay open. Product documentation is embedded in
builds; no package path or external Pi documentation payload is required.
