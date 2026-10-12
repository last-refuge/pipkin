# Constrained tools — 2026-10-11

Implementation on `native-engine`, based on **ea0a98b** (the committed positional-context
and child-paging batch). Protocol **5** / schema **4** already carry these optional tool
declarations; no additional wire/storage version change is needed. Native remains opt-in.
The owner's existing plan edit and `.claude/worktrees/` remain untouched.

## Implemented behavior

- `ToolSpec` now retains `constrainedSampling` in both directions through durable
  `ToolDeclaration`. Native tools can declare it through `Tool::spec`; positional baseline,
  patches, collapse and imported declarations preserve it. The Pi-message transport also
  includes it for legacy flat requests. The older `toolConstrainedSampling` request option
  remains a fallback; a tool's declaration takes precedence.
- JSON-schema `prefer` / `require` is resolved against each adapter's strict-mode capability.
  Strict schemas use the existing reference-derived converter (all properties required;
  optional properties nullable; additional properties disabled). Unsupported `prefer`
  falls back; unsupported `require` returns the reference error before model execution.
  Responses/Azure/Codex, Chat Completions, Mistral and Bedrock now invoke this contract;
  Anthropic and Google consume the newly preserved declaration instead of only the old option.
- Responses-family grammar tools choose nonempty OpenAI Lark before regex variants and
  infer the input property from the single required string property. Invalid definitions
  fail with the reference error. Endpoints without grammar support retain JSON function tools.
- Grammar declarations also work in positional additional-tools and client tool-search anchors.
  `custom_tool_call` input maps into the declared property, rather than always `input`.
  Initial, incremental, final and interrupted input retain that mapping. Rewrites after closure
  and non-monotone final input fail; interrupted text is preserved without executing the call.
- Replay emits matching custom call/output items, retaining `ctc_` ids only when valid for
  that model. Changing model removes the item id; changing to an endpoint without grammar
  support uses the original JSON arguments and function-call pair. A missing/non-string
  replay property fails rather than silently losing input. Codex SSE and WebSocket setup
  both initialize the shared stream processor with the tool-property map.

## Evidence

`fixtures/native-system/constrained-reference.json` contains **15 cases executed against
pinned Pi d2a311097cbcf669e699479587332ae3988a49d0**. The capture script verifies HEAD and
unchanged source files and records their SHA-256 hashes. It executes the unchanged pure
constrained-sampling module and the extracted, unchanged `convertResponsesTools` function
body in temporary TypeScript modules, without installing Pi dependencies.

Regenerate with:

```sh
node scripts/capture-constrained-reference.mjs PI_CHECKOUT fixtures/native-system/constrained-reference.json
```

`constr::constrained_declarations_match_executed_pinned_reference` compares declarations,
strict defaults, strict requirement/preference fallback and failures, grammar variant priority,
input-property inference and malformed-schema errors. This is a helper differential, not a
complete provider/backend differential.

Other focused tests:

- `constr::grammar_streams_and_replays_across_all_responses_adapters`: real local HTTP,
  fragmented SSE (including split Unicode), OpenAI/Azure/Codex request declarations,
  argument mapping, serialized/reloaded messages, custom replay, model changes and JSON
  fallback; Codex's compressed request is decoded before assertions.
- `constr::grammar_stream_rejects_rewrites_and_preserves_interrupted_input`: malformed
  non-monotone completion, closed input, and retained partial arguments.
- `constr::positional_grammar_anchors_preserve_declarations_and_input_properties`:
  additional-tools and tool-search declarations, deferred loading and effective tool maps.
- `constr::strict_tool_metadata_reaches_completions_mistral_and_google` and
  `brk::constrained_tool_metadata_reaches_bedrock_and_rejects_unsupported_require`:
  strict wire schemas and unsupported-capability refusal. The existing Anthropic
  `openrouter_affinity_strict_tools_and_no_cache` additionally asserts declaration precedence.
- `rt::constrained_grammar_tool_executes_and_replays_after_restart`: native engine with a
  local mock Responses provider; custom tool arguments pass the real executor's validation,
  write a fixture file, persist system declarations/call/result, and replay after disk reopen.

Final required checks: **1353 passed, 0 failed, 53 ignored**; Clippy and formatting pass.
Logs: `/tmp/pipkin-constrained-workspace.log`, `/tmp/pipkin-constrained-clippy.log`,
`/tmp/pipkin-constrained-fmt.log`. Focused tests: **7 passed**. During development, a harness
assertion incorrectly treated a zstd Codex body as plain JSON; the runtime test also initially
dropped its owned runtime inside async code. Those test-harness errors were corrected and
rerun; they are not hidden provider failures or claimed release passes.

## Remaining qualification

No live credentials, paid calls or desktop interactions were used. The new grammar HTTP tests
exercise SSE; grammar over a live WebSocket is not independently qualified by shared-code
coverage. Full native/Pi workflow differentials, complete live/provider/installed acceptance,
extension package compatibility, advanced credentials, proxy/lifecycle gaps and release gates
remain. This change supplies the native tool contract; it does not make unsupported TypeScript
facets or their tool registrations runnable. No default switch or Pi retirement occurred.
