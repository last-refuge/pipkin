# Pi provider / API / auth inventory (for Rust port)

Source: staged Pi at rev d2a311097cbcf669e699479587332ae3988a49d0 plus Pipkin overlays.
Root: `<pi-staged>/`.
Paths below are relative to `packages/ai/src/` unless prefixed `coding-agent/` (= `packages/coding-agent/src/`).
`file:N` / `file:N-M` are line references.

Reserved item: the Anthropic Claude subscription OAuth flow and every request quirk tied to it are
listed as "RESERVED - location only" (file + symbol), with no endpoints, client ids, headers or
request details.

---

## 0. Architecture in one page

- **API kind** (`Api` string, `types.ts:17-29`): a wire-protocol adapter module in `api/*.ts`. Each one
  exports `stream(model, ctx, opts)` and `streamSimple(model, ctx, simpleOpts)` and returns an
  `AssistantMessageEventStream` (`types.ts:288-301`). `streamSimple` maps the provider-neutral
  `reasoning: ThinkingLevel` onto that API's native knobs. Modules are loaded lazily through
  `api/*.lazy.ts` + `api/lazy.ts:46-98` (`lazyStream` turns setup errors into an `error` event).
- **Provider** (`models.ts:149-238`): `{id, name, baseUrl?, headers?, auth: {apiKey?, oauth?}, getModels,
  stream, streamSimple, refreshModels?, filterModels?, generateImages?, classify?}`, built with
  `createProvider()` (`models.ts:1039-1180`). A provider can map several APIs (`api: {"anthropic-messages": ..., ...}`)
  and dispatches on `model.api` (`models.ts:1068-1081`).
- **Models collection** (`models.ts:249-358`, impl `ModelsImpl` `models.ts:389-988`): resolves auth per
  request (`applyAuth`, `models.ts:842-874`), normalizes `Context` to `TranscriptContext`
  (`utils/transcript.ts:30-34`), then calls `provider.stream*`. coding-agent wraps this in
  `ModelRuntime` (`coding-agent/core/model-runtime.ts:171`), which layers models.json + extensions.
- **Contract for adapters** (`types.ts:362-377`, `types.ts:753-785`): emit `start` first; then
  `*_start/_delta/_end` per content block; finish with `done{reason}` or `error{reason: "error"|"aborted"}`.
  `streamSimple` may throw synchronously only for missing auth. Every adapter's catch block sets
  `stopReason = signal.aborted ? "aborted" : "error"`, strips scratch fields (`partialJson`,
  `partialArgs`, `customInput`, `index`, `streamIndex`) and pushes `error`.
- **Shared request helpers**: `api/simple-options.ts` (`buildBaseOptions` 36-65, `clampMaxTokensToContext` 18-22,
  thinking budgets 67-108); `api/transform-messages.ts` (cross-provider replay normalization, 64-235);
  `utils/provider-retry.ts` (SDK-style retry, 105-125); `utils/error-body.ts` (error string normalization);
  `utils/json-parse.ts:104-124` (`parseStreamingJson`: repair -> partial-json -> `{}` fallback).

---

## 1. API kinds (wire adapters)

Common to all OpenAI-SDK and Anthropic-SDK adapters:
- SDK client built per request with `dangerouslyAllowBrowser: true`, `fetch: options.fetch`,
  `defaultHeaders` = `{"User-Agent": "pi (<platform> <release>; <arch>)"}` (`utils/pi-user-agent.ts:17-19`)
  then `model.headers`, then dynamic headers, then `options.headers` (caller wins; a `null` value deletes).
- SDK is invoked with `maxRetries: 0`, `timeout: options.timeoutMs`, `signal`; retries happen in
  `retryProviderRequest` (section 6).
- `options.onPayload(params, model)` may replace the request body before send; `options.onResponse({status, headers})`
  fires after headers; `options.onProviderStreamEvent(rawEvent)` fires per parsed upstream event.
- Cache retention: `options.cacheRetention ?? (env PI_CACHE_RETENTION === "long" ? "long" : "short")`
  (e.g. `api/anthropic-messages.ts:69-77`, `api/openai-completions.ts:294-302`). `"none"` also suppresses sessionId-based cache keys/affinity.
- Surrogates: every outbound text goes through `sanitizeSurrogates` (`utils/sanitize-unicode.ts:21-25`, strips unpaired UTF-16 surrogates; a Rust port with `String` gets this for free on decode but must handle lossy input).

### 1.1 `anthropic-messages` — `api/anthropic-messages.ts`

- **Transport**: `@anthropic-ai/sdk` `client.beta.messages.create(params).asResponse()` (`:650`), then pi's own
  SSE decoder over the raw body (`iterateSseMessages` `:471-528`, `iterateAnthropicEvents` `:530-569`).
  Endpoint = SDK default `POST {baseUrl}/v1/messages` (beta path with `betas` -> `anthropic-beta` header).
  `PiAnthropic` subclass disables SDK default-credential discovery (`:335-339`).
- **Auth** (`createClient` `:982-1078`, `hasRequestAuth` `:316-323`):
  - API key: `x-api-key: <key>` (SDK `apiKey`). Request is allowed if apiKey OR header `authorization` /
    `x-api-key` / `cf-aig-authorization` is present, else throws `No API key for provider: X`.
  - `github-copilot` provider: `Authorization: Bearer <copilot token>` (SDK `authToken`) + copilot dynamic headers (`:992-1011`).
  - `ANTHROPIC_AUTH_TOKEN` env (resolved in `providers/anthropic.ts`): passed as header `Authorization: Bearer ...`.
  - Workload identity federation (only provider `anthropic`, only when no key/header): SDK `config.authentication =
    {type:"oidc_federation", federation_rule_id, service_account_id?, identity_token:{source:"file", path}}`
    from env `ANTHROPIC_FEDERATION_RULE_ID`, `ANTHROPIC_ORGANIZATION_ID`, `ANTHROPIC_IDENTITY_TOKEN_FILE`,
    optional `ANTHROPIC_SERVICE_ACCOUNT_ID`, `ANTHROPIC_WORKSPACE_ID` (`:349-370`); one cached client per config (`:377`, `:1052-1066`).
  - Always sets `accept: application/json`, `anthropic-dangerous-direct-browser-access: true`.
  - Session affinity (when `sessionId` and cache retention != none and `compat.sendSessionAffinityHeaders`,
    default true only for OpenRouter): `x-session-id` (format `openrouter`) else `x-session-affinity` (`:1039-1042`).
  - **RESERVED - location only**: subscription-OAuth token detection `isOAuthToken` (`:978-980`), the OAuth
    branch of `createClient` (`:1013-1034`), identity constants `claudeCodeVersion` (`:96`), `claudeCodeTools`
    / `toClaudeCodeName` / `fromClaudeCodeName` (`:98-132`, used at `:727-729`, `:1347`, `:1443`, `:1603`),
    OAuth-only beta entries in `getBetaFeatures` (`:1106`), and the OAuth `system` branch of `buildParams`
    (`:1167-1182`). The `isOAuth` flag threads through `convertMessages`/`convertTools`.
- **Request body** (`buildParams` `:1124-1287`):
  - `model`, `max_tokens = options.maxTokens ?? model.maxTokens`, `stream: true`, `betas[]`.
  - `system`: array of one `{type:"text", text, cache_control?}` built from the leading system message (`:1183-1192`).
  - `messages` from `convertMessages` (`:1308-1512`): user string or blocks (`text`, `image{source:{type:"base64",media_type,data}}`,
    empty text dropped); assistant blocks `text`, `thinking{thinking, signature}`, `redacted_thinking{data}`,
    `tool_use{id,name,input}`; thinking without signature -> plain `text` (or `signature:""` if
    `compat.allowEmptySignature`); consecutive toolResults merged into one `user` message of
    `tool_result{tool_use_id, content(string or blocks, image-only gets "(see attached image)"), is_error}`.
    Later system messages (mid-convo) become `{role:"system", content:[text | tool_removal | tool_addition]}`
    held until just before the next assistant message.
  - Tool call ids normalized to `[a-zA-Z0-9_-]`, max 64 (`:1290-1292`).
  - `tools` (`convertTools` `:1576-1611`): `{name, description, input_schema:{type:"object", properties, required},
    eager_input_streaming?:true, strict?:true, cache_control? (last tool only)}`. Strict only when
    `compat.supportsStrictTools` and schema avoids unsupported keywords (`:1543-1574`).
    Native mid-convo tool changes: fixed tool list + `DEFERRED_TOOL_PLACEHOLDER` (`:204-209`, `:1205-1219`).
  - `tool_choice`: string -> `{type}`; object `{type:"tool", name}` passthrough (`:1273-1279`).
  - `temperature` only if thinking disabled, not managed-effort, and `compat.supportsTemperature` (`:1195-1202`).
  - `metadata.user_id` from `options.metadata.user_id` (`:1266-1271`).
  - `fallbacks: [{model}]` when `compat.allowedFallbackModels` non-empty (`:1281-1284`).
  - **Thinking** (`:1233-1264`): managed effort models (`compat.supportsMidConvoEffort`): `thinking:{type:"adaptive",
    display, block_binding:{prefix_mismatch_behavior:"drop_block"}}`, `output_config:{effort:"high"}` plus per-turn
    `{role:"system", content:[], output_config:{effort}}` inserted before each historical assistant and at the end
    (`insertThinkingLevelMessages` `:1518-1532`). `forceAdaptiveThinking`: `{type:"adaptive", display}` +
    `output_config.effort`. Otherwise budget: `{type:"enabled", budget_tokens, display}`. Disabled:
    `{type:"disabled"}` unless `thinkingLevelMap.off === null`. `display` default `"summarized"`.
  - **Prompt caching** (`getCacheControl` `:79-93`): `cache_control:{type:"ephemeral", ttl?:"1h"}` (ttl only for
    long retention + `compat.supportsLongCacheRetention`). Placed on: system block(s), last tool (if
    `supportsCacheControlOnTools`), and last block of the final user/system message (`:1483-1509`).
  - **Betas** (`getBetaFeatures` `:1080-1122`): a configured `anthropic-beta` header in model/options headers
    overrides the computed list (null => none). Computed: `fine-grained-tool-streaming-2025-05-14` (tools present
    and `!supportsEagerToolInputStreaming`), `interleaved-thinking-2025-05-14` (budget thinking, opt-out via
    `interleavedThinking:false`), `server-side-fallback-2026-07-01`, `mid-conversation-output-config-2026-07-01` +
    `thinking-binding-controls-2026-08-01` (managed effort), `inline-tools-2026-09-15` (native tool changes) (`:190-195`).
- **SSE parsing** (`:663-859`): only `message_start|message_delta|message_stop|content_block_start|content_block_delta|content_block_stop`
  are parsed (JSON repaired); `event: error` throws its data; stream ending after `message_start` without `message_stop`
  throws `Anthropic stream ended before message_stop`.
  - `message_start`: `responseId=message.id`; `responseModel` if differs; fallback-model pricing; usage
    `input_tokens`, `output_tokens`, `cache_read_input_tokens`->cacheRead, `cache_creation_input_tokens`->cacheWrite,
    `cache_creation.ephemeral_1h_input_tokens`->cacheWrite1h; `totalTokens = input+output+cacheRead+cacheWrite`.
  - `content_block_start`: `text` -> text_start; `thinking` -> thinking_start (signature initial); `redacted_thinking` ->
    thinking block `{thinking:"[Reasoning redacted]", thinkingSignature: data, redacted:true}`; `tool_use` -> toolcall_start;
    `fallback` block allowed only before any content.
  - `content_block_delta`: `text_delta`, `thinking_delta`, `input_json_delta` (partial JSON accumulated, args reparsed each delta),
    `signature_delta` (appended to `thinkingSignature`, no event).
  - `content_block_stop` -> `text_end` / `thinking_end` / `toolcall_end` (final parse).
  - `message_delta`: `stop_reason` mapped; usage fields updated only when non-null; `output_tokens_details.thinking_tokens` -> `usage.reasoning`;
    `input_transformations` captured into a diagnostic.
  - **Stop reasons** (`mapStopReason` `:1613-1639`): `end_turn|pause_turn|stop_sequence`->stop, `max_tokens`->length,
    `tool_use`->toolUse, `refusal`->error (message = `stop_details.explanation`), `sensitive`->error, unknown -> throw.
- **streamSimple** (`:928-976`): no reasoning -> `thinkingEnabled:false`. `forceAdaptiveThinking` -> effort via
  `mapThinkingLevelToEffort` (`:908-926`: `thinkingLevelMap[level]` string wins; minimal/low->low, medium, high, default high).
  Else budget: `adjustMaxTokensForThinking` (budgets minimal 1024 / low 2048 / medium 8192 / high 16384, xhigh/max clamp to high;
  maxTokens = min(base+budget, model.maxTokens), at least 1024 answer tokens) then `thinkingBudgetTokens = min(budget, maxTokens-1024)`.
- **Errors**: message = `error.message` (SDK folds body). `providerThinkingLevel` recorded for managed-effort models.

### 1.2 `openai-completions` — `api/openai-completions.ts`

- **Transport**: `openai` SDK `client.chat.completions.create(params).withResponse()` (`:375-382`) => `POST {baseUrl}/chat/completions`, SSE `data:` chunks, `[DONE]`.
- **Auth**: `Authorization: Bearer <apiKey>` via SDK. If no key but `authorization`/`cf-aig-authorization` header present, apiKey `"unused"` (`getClientApiKey` `:87-91`).
- **Headers** (`createClient` `:757-800`): UA + `model.headers`; copilot dynamic headers for `github-copilot`;
  session affinity when `sessionId` (cache != none) and `compat.sendSessionAffinityHeaders`:
  `openrouter` -> `x-session-id`; else `x-client-request-id` + `x-session-affinity` (+ `session_id` for format `openai`).
- **Request body** (`buildParams` `:802-1010`):
  - `model`, `messages`, `stream:true`, `stream_options:{include_usage:true}` (unless `supportsUsageInStreaming === false`),
    `store:false` when `supportsStore`.
  - `prompt_cache_key = sessionId` (clamped to 64 chars, `api/openai-prompt-cache.ts`) when baseUrl contains `api.openai.com`
    and cache != none, or when long retention supported; `prompt_cache_retention:"24h"` for long retention.
  - `max_completion_tokens` or `max_tokens` (`compat.maxTokensField`); `temperature`.
  - `tools`: `{type:"function", function:{name, description, parameters, strict?}}` (strict included only when
    `supportsStrictMode`), or grammar custom tool `{type:"custom", custom:{name, description, format:{type:"grammar", grammar:{syntax, definition}}}}`
    (`convertTools` `:1482-1517`). Empty `tools: []` is sent when history has tool calls but no current tools (LiteLLM/Anthropic proxies).
    `tool_stream:true` for z.ai (`zaiToolStream`). `tool_choice` passthrough. `priority` = `compat.vllmPriority`.
  - **Thinking formats** (`:880-977`, controlled by `compat.thinkingFormat`):
    `openai` -> `reasoning_effort = thinkingLevelMap[level] ?? level` (or `thinkingLevelMap.off` string when off);
    `openrouter` -> `reasoning:{effort}` (off -> `thinkingLevelMap.off ?? "none"` unless null);
    `deepseek` -> `thinking:{type:"enabled"|"disabled"}` + `reasoning_effort`;
    `zai` -> `thinking:{type, clear_thinking:false}` + optional `reasoning_effort`;
    `qwen` -> top-level `enable_thinking` + optional `reasoning_effort`;
    `qwen-chat-template` -> `chat_template_kwargs:{enable_thinking, preserve_thinking:true}`;
    `chat-template` -> `chat_template_kwargs` from `compat.chatTemplateKwargs` with `$var` substitution
    (`thinking.enabled|effort|budget`, `omitWhenOff`) (`:1034-1075`);
    `baseten` -> `chat_template_args` + `reasoning_effort`;
    `together` -> `reasoning:{enabled}` + `reasoning_effort`;
    `string-thinking` -> top-level `thinking: "<effort>"`;
    `ant-ling` -> `reasoning:{effort}` only if mapped value is a string.
    Optional budget field `thinking_token_budget|thinking_budget|thinking_budget_tokens` (`:983-985`), budget clamped to leave 1024 answer tokens.
  - `provider` = `compat.openRouterRouting`; `providerOptions.gateway{only,order}` = `compat.vercelGatewayRouting`.
  - `samplingParams` (model defaults < per-thinking-level < request) merged last, overriding named fields (`:1003-1007`, `api/simple-options.ts:24-34`).
  - **Anthropic-style caching via OpenAI-compat** (`cacheControlFormat:"anthropic"`, auto for openrouter `anthropic/*`):
    `cache_control:{type:"ephemeral", ttl?:"1h"}` on first system/developer message, last tool, last text part of last user/assistant/tool message (`:1077-1191`).
- **Message conversion** (`convertMessages` `:1193-1480`):
  - Instruction role `developer` if `model.reasoning && supportsDeveloperRole` else `system`.
    Mid-convo system messages with tool additions emit a non-standard `{role:"system", tools:[...]}` (Kimi style) when `supportsMidConvoToolAdditions`.
  - User images -> `image_url:{url:"data:<mime>;base64,<data>"}`.
  - Assistant: content as plain string (never array); `tool_calls[{id,type:"function",function:{name,arguments(JSON)}}]` or `custom`;
    thinking replay: `reasoning_details` (OpenRouter structured JSON, stored in `thinkingSignature`) or the raw field named by the
    signature (`reasoning_content|reasoning|reasoning_text`); `requiresThinkingAsText` -> thinking prepended as text;
    `requiresReasoningContentOnAssistantMessages` -> `reasoning_content:""`; empty assistant messages skipped.
  - Tool results -> `{role:"tool", content(text or "(see attached image)"/"(no tool output)"), tool_call_id, name?}`;
    images then appended as a synthetic user message "Attached image(s) from tool result:"; `requiresAssistantAfterToolResult` inserts
    `"I have processed the tool results."`.
  - Tool call ids: `call|item` pipe ids collapsed to `<call>_<item>` <=40 chars (hash fallback); provider `openai` truncates to 40.
- **Stream parsing** (`:558-683`): `responseId ||= chunk.id`; `responseModel` if differs; usage from `chunk.usage` or `choice.usage`
  (`parseChunkUsage` `:1519-1560`: cacheRead = `prompt_tokens_details.cached_tokens ?? prompt_cache_hit_tokens ?? cached_tokens`,
  cacheWrite = `prompt_tokens_details.cache_write_tokens`, input = prompt - cacheRead - cacheWrite, output = completion_tokens,
  reasoning = `completion_tokens_details.reasoning_tokens`). Text from `delta.content`; thinking from first non-empty of
  `reasoning_content|reasoning|reasoning_text` (field name stored as `thinkingSignature`); `delta.reasoning_details[]` merged and stored as JSON
  in `thinkingSignature` at block end; tool calls keyed by `index` then `id`, args accumulated + reparsed. Blocks are only closed at stream end
  (all `*_end` events emitted after the loop).
- **Stop reasons** (`:1562-1586`): `stop|end|null`->stop, `length`->length, `tool_calls|function_call`->toolUse, `content_filter|network_error|other` -> error `Provider finish_reason: X`.
  Missing finish_reason -> error `Stream ended without finish_reason`, unless `supportsFinishReason:false` (then inferred stop/toolUse).
- **Errors**: `formatProviderError(normalizeProviderError(e))` + OpenRouter `error.metadata.raw` appended (`:719-727`).
- **Compat auto-detection** (`detectCompat` `:1593-1690`; explicit `model.compat` overrides per key `:1696-1734`):
  non-standard hosts (nvidia, cerebras, xai, together, chutes, deepseek, zai, moonshot, opencode, cloudflare, ant-ling) => no `store`, no developer role;
  `max_tokens` field for chutes/deepseek/moonshot/cf-gateway/together/nvidia/ant-ling/zai; reasoning_effort unsupported for grok/zai/moonshot/together/cf-gateway/nvidia/ant-ling;
  thinkingFormat deepseek/zai/together/ant-ling/openrouter; DeepSeek requires `reasoning_content` on assistants; strict mode off by default;
  OpenRouter: affinity on, format `openrouter`; long cache retention off for together/cloudflare/nvidia/ant-ling.

### 1.3 `openai-responses` — `api/openai-responses.ts` + `api/openai-responses-shared.ts`

- **Transport**: `openai` SDK `client.responses.create(params).withResponse()` => `POST {baseUrl}/responses`, SSE typed events.
- **Auth**: `Authorization: Bearer <apiKey>` (same `getClientApiKey` rule, `:58-62`). "Sign in with ChatGPT" detection:
  provider `openai`, baseUrl exactly `https://api.openai.com/v1`, key not starting `sk-` (`isChatGPTSignIn` `:40-47`) =>
  omit `prompt_cache_retention`, `prompt_cache_options`, `max_output_tokens`, `temperature` (`:329-346`).
- **Headers** (`:258-300`): UA, model headers, copilot dynamic headers, session headers when sessionId (cache != none):
  format `openrouter` -> `x-session-id`; else `x-client-request-id` (+ `session_id` for `openai`). (No `x-session-affinity` here.)
- **Request body** (`buildParams` `:302-389`): `model`, `input` (ResponseInput), `stream:true`, `store:false`,
  `prompt_cache_key = sessionId` (unless none), `prompt_cache_retention:"24h"` (long, unless explicit cache mode),
  `prompt_cache_options` (`supportsExplicitPromptCacheMode`: none -> `{mode:"explicit"}`, long -> `{ttl:"30m"}`) (`:97-114`),
  `max_output_tokens = max(maxTokens, 16)` (if `supportsMaxOutputTokens`), `temperature`, `service_tier`, `tools`, `tool_choice`,
  `reasoning:{effort, summary: reasoningSummary || "auto"}` + `include:["reasoning.encrypted_content"]` when effort set;
  off -> `reasoning:{effort: thinkingLevelMap.off ?? "none"}` (skipped for github-copilot or when off is null); xAI always includes encrypted content.
  `samplingParams` merged last.
- **Input conversion** (`convertResponsesMessages` shared `:145-354`): system -> `{role: developer|system, content}`; user ->
  `input_text`/`input_image{detail:"auto", image_url:data-URL}`; assistant thinking -> the stored reasoning item JSON (`thinkingSignature` is
  the full `ResponseReasoningItem`); text -> `{type:"message", role:"assistant", content:[{type:"output_text", text, annotations:[]}], status:"completed", id, phase}`
  where id/phase come from `textSignature` (`TextSignatureV1 {v:1,id,phase}` JSON or legacy plain id; >64 chars hashed);
  tool calls -> `function_call{id?, call_id, name, arguments}` or `custom_tool_call{..., input}`; pi tool call id format is `"<call_id>|<item_id>"`;
  item id dropped for different-model replays or wrong prefix (`fc_` / `ctc_`); foreign item ids hashed to `fc_<hash>`.
  Tool results -> `function_call_output|custom_tool_call_output{call_id, output}` (string or `input_text`/`input_image` array).
  Mid-convo tool additions -> `{type:"additional_tools", role:"developer", tools}` or a synthetic client `tool_search_call`/`tool_search_output` pair (`:184-212`).
- **Tools** (`convertResponsesTools` `:360-397`): `{type:"function", name, description, parameters, strict?, defer_loading?}` or grammar `{type:"custom", format:{type:"grammar", syntax, definition}}`.
- **Stream events** (`processResponsesStream` `:433-777`):
  `response.created` (responseId); `response.output_item.added` creates a slot per `output_index` for `reasoning` (thinking_start),
  `message` (text_start; `phase:"final_answer"` forces stop), `function_call`/`custom_tool_call` (toolcall_start, id `call_id|id`);
  `response.reasoning_summary_text.delta` / `response.reasoning_text.delta` -> thinking_delta; `response.reasoning_summary_part.done` -> `"\n\n"` delta;
  `response.output_text.delta` and `response.refusal.delta` -> text_delta; `response.function_call_arguments.delta|done`,
  `response.custom_tool_call_input.delta|done` -> toolcall_delta; `response.output_item.done` finalizes:
  reasoning -> `thinkingSignature = JSON.stringify(item)` (thinking text = summary or content), message -> `textSignature` V1, calls -> toolcall_end.
  `response.completed|incomplete` -> usage (`input = input_tokens - cached_tokens - cache_write_tokens`, cacheRead, cacheWrite,
  output, reasoning, total) + service-tier cost multiplier (flex 0.5, priority/fast 2, gpt-5.5 2.5) + stop mapping; backfills
  `encrypted_content` missing from item.done (Azure). `error` -> throw `Error Code X: msg`; `response.failed` -> throw.
  No terminal event -> throw; toolUse with unfinished calls -> throw.
- **Stop reasons** (`:779-809`): completed -> stop (-> toolUse if any tool call), incomplete+`max_output_tokens` -> length,
  incomplete other -> error `Response incomplete: <reason>`, failed/cancelled -> error, queued/in_progress -> stop. `rawStopReason = status[.reason]`.
- **Errors**: `formatProviderError(..., "OpenAI API error")`; `subscription_sharing_usage_limit_exceeded` gets a usage-page hint (`:222-229`).

### 1.4 `azure-openai-responses` — `api/azure-openai-responses.ts`

- `AzureOpenAI` SDK client (`:261-278`), `api-key` header via SDK, `apiVersion = opts.azureApiVersion || AZURE_OPENAI_API_VERSION || "v1"`.
- Base URL resolution (`:226-259`): `azureBaseUrl` opt > `AZURE_OPENAI_BASE_URL` > `https://{AZURE_OPENAI_RESOURCE_NAME}.openai.azure.com/openai/v1` > `model.baseUrl`;
  Azure hosts normalized to path `/openai/v1` (`:191-220`).
- Deployment name: `azureDeploymentName` opt > `AZURE_OPENAI_DEPLOYMENT_NAME_MAP` (`modelId=deployment,...`) > model id; sent as `model` (`:30-51`).
- Body like openai-responses but `prompt_cache_key = sessionId` always, no retention options, strict default true (`:280-355`). Same shared stream processor.

### 1.5 `openai-codex-responses` — `api/openai-codex-responses.ts` (ChatGPT subscription backend)

- **Endpoint**: `resolveCodexUrl` (`:641-647`): `{baseUrl}/codex/responses` (default base `https://chatgpt.com/backend-api`); WebSocket: same URL with `wss:` (`:649-654`).
- **Auth headers** (`:1640-1697`): `Authorization: Bearer <access token>`, `chatgpt-account-id: <JWT claim "https://api.openai.com/auth".chatgpt_account_id>`
  (`extractAccountId` `:1627-1638`), `originator: pi`, UA. SSE adds `OpenAI-Beta: responses=experimental`, `accept: text/event-stream`,
  `content-type: application/json`, `session-id` + `x-client-request-id` = sessionId; request body zstd-compressed (level 3, `content-encoding: zstd`) when Node zlib supports it (`:218-231`, `:379-383`).
  WebSocket headers: `OpenAI-Beta: responses_websockets=2026-02-06`, `session-id` / `x-client-request-id` = sessionId or uuidv7.
- **Body** (`buildRequestBody` `:527-600`): `model`, `store:false`, `stream:true`, `instructions` (leading system prompt or "You are a helpful assistant."),
  `input` (shared converter with `includeSystemPrompt:false`), `text:{verbosity: textVerbosity||"low"}`, `include:["reasoning.encrypted_content"]`,
  `prompt_cache_key` (sessionId, 64 chars), `tool_choice` (default `"auto"`), `parallel_tool_calls:true`, `temperature?`, `service_tier?`, `tools` (strict: null default),
  `reasoning:{effort, summary: reasoningSummary ?? "auto"}`; off -> `{effort: thinkingLevelMap.off ?? "none"}` (omitted when null).
- **Transport selection** (`stream` `:237-498`): `transport` option `sse|websocket|websocket-cached|auto` (default auto). Non-SSE tries WebSocket first:
  pooled per (sessionId, accountId) (`acquireWebSocket` ~`:1153-1248`, idle TTL 5 min, max age 55 min, connect timeout default 15 s);
  sends `{"type":"response.create", ...body}`; `websocket-cached|auto` sends only the input delta with `previous_response_id` when the prior body matches (`:1438-1477`).
  Retries once on `previous_response_not_found` and once on `websocket_connection_limit_reached` before start; any other transport failure before the first event
  marks the session as SSE-fallback permanently (`recordWebSocketFailure` `:978-986`) and falls through to SSE. Diagnostics `provider_transport_failure` appended.
- **SSE parse** (`parseSSE` `:799-859`): `\n\n` frames, `data:` lines, `[DONE]` ignored, invalid JSON -> `CodexProtocolError`.
  `mapCodexEvents` (`:743-788`): `error` and `response.failed` -> `CodexApiError(code)`; `response.done|completed|incomplete` normalized to `response.completed`
  (records `end_turn` -> `AssistantMessage.endTurn`), then shared Responses processor.
- **Retry** (SSE, `:385-461`): `maxRetries` default 0; retries 429/500/502/503/504 or text matching rate limit/overloaded/service unavailable/upstream connect/connection refused,
  except terminal quota text (`isTerminalRateLimitError` `:123-127`); delay = `retry-after-ms` / `retry-after` (validated against `maxRetryDelayMs`, default 60 s) else `1000 * 2^attempt`;
  network errors retried with backoff except `usage limit`. Header timeout = `timeoutMs` via `AbortSignal.timeout`.
- **Errors**: `parseErrorResponse` (`:1596-1621`): code `usage_limit_reached|usage_not_included|rate_limit_exceeded` or 429 => "You have hit your ChatGPT usage limit (<plan> plan). Try again in ~N min."
- Service tier pricing: flex 0.5, priority 2 (2.5 for gpt-5.5), response `default` with requested flex/priority keeps requested (`:602-639`).

### 1.6 `google-generative-ai` — `api/google-generative-ai.ts` + `api/google-shared.ts`

- `@google/genai` `GoogleGenAI({apiKey, httpOptions:{baseUrl, apiVersion:"", headers}})` (`:348-367`) => `models.generateContentStream` (Gemini `:streamGenerateContent?alt=sse` with `x-goog-api-key` header per SDK).
  Custom `fetch` rejected (`:87-89`); `timeoutMs` not forwarded.
- **Body** (`buildParams` `:369-429`): `model`, `contents` (`convertMessages` shared `:191-343`), `config:{temperature?, maxOutputTokens?, systemInstruction?, tools:[{functionDeclarations:[{name, description, parametersJsonSchema}]}],
  toolConfig:{functionCallingConfig:{mode}}, thinkingConfig, abortSignal}`.
  - Roles `user`/`model`; images `inlineData{mimeType,data}`; assistant thinking (same provider+model) -> `{thought:true, text, thoughtSignature?}`, cross-model -> plain text;
    `thoughtSignature` kept only for same provider/model and valid base64 (`:146-160`); tool call `functionCall{name,args,id?}` (`id` for claude-*, gpt-oss-*, Gemini>=3);
    tool results `functionResponse{name, response:{output|error}, parts?(images, Gemini>=3), id?}`, all merged in one user turn; Gemini<3 images in extra user message.
  - Function calling mode: `none`/`any` explicit; `VALIDATED` when any tool resolves strict (Gemini >= 3) (`:423-436`).
  - **Thinking** (`streamSimple` `:305-346`): off -> `thinkingConfig` disabled (`thinkingBudget:0` or lowest level for level-based models, `:102-111`).
    Level-based models (`gemini-3*-pro|flash`, `gemini-flash(-lite)-latest`, `gemma-4`) -> `thinkingLevel: MINIMAL|LOW|MEDIUM|HIGH` (`:72-100`);
    others `thinkingBudget` (2.5-pro 128/2048/8192/32768, 2.5-flash-lite 512/2048/8192/24576, 2.5-flash 128/2048/8192/24576, else -1) (`:431-471`), `includeThoughts:true`.
- **Stream** (`:106-252`): per chunk `candidates[0].content.parts`: `text` parts with `thought:true` -> thinking, else text (block switches emit end/start);
  `thoughtSignature` retained per block (`textSignature` for text); `functionCall` -> toolcall_start + one delta (full JSON) + end; id synthesized
  `<name>_<ms>_<n>` when missing/duplicate. `finishReason` -> stop mapping (any tool call => toolUse). `usageMetadata`: input = promptTokenCount - cachedContentTokenCount,
  output = candidates + thoughts, cacheRead = cachedContentTokenCount, reasoning = thoughtsTokenCount, total = totalTokenCount.
- **Stop** (`google-shared.ts:441-469`): STOP->stop, MAX_TOKENS->length, everything else (SAFETY, RECITATION, MALFORMED_FUNCTION_CALL, ...) -> error `Provider stopped with: <reason>`.
- **Retry**: `retryGoogleRequest` (`:494-515`) wraps the initial call with `retryProviderRequest` (ApiError has `status` but no headers => backoff only).

### 1.7 `google-vertex` — `api/google-vertex.ts`

- Same conversion/stream as 1.6 with `GoogleGenAI({vertexai:true, apiVersion:"v1", ...})`.
- Auth: explicit API key (ignored if `gcp-vertex-credentials` marker or `<placeholder>`), else ADC with `project` (opt > `GOOGLE_CLOUD_PROJECT` > `GCLOUD_PROJECT`)
  and `location` (opt > `GOOGLE_CLOUD_LOCATION`); `GOOGLE_APPLICATION_CREDENTIALS` -> `googleAuthOptions.keyFilename` (`:96-110`, `:357-460`).
- Catalog baseUrl `https://{location}-aiplatform.googleapis.com` is a template; custom baseUrl only used if it lacks `{location}` (`:407-422`, `baseUrlResourceScope: COLLECTION`).

### 1.8 `mistral-conversations` — `api/mistral-conversations.ts`

- Raw `fetch` `POST {baseUrl}/v1/chat/completions` (`:297-339`), headers `authorization: Bearer <key>`, `accept: text/event-stream`, `content-type: application/json`, UA,
  `x-affinity: <sessionId>` when caching (unless overridden) (`:341-358`).
- Timeout: `AbortSignal.timeout(timeoutMs ?? 60000)` combined with caller signal, applied to the WHOLE request including body streaming (`:307-308`). No retry.
- Body (`:513-535`, camelCase -> snake_case at `:372-432`): `model`, `stream:true`, `messages`, `tools[{type:"function", function}]`, `temperature`, `max_tokens`, `tool_choice`
  (`auto|none|any|required|{type:"function",function:{name}}`), `prompt_mode:"reasoning"` (reasoning models without thinkingLevelMap) or `reasoning_effort` (models with map; off uses map.off),
  `prompt_cache_key = sessionId`.
- Tool call ids: 9-char alphanumeric, deterministic hash with collision map (`:237-267`). Thinking replay as content chunk `{type:"thinking", thinking:[{type:"text", text}]}`.
- Stream (`:445-511`, `:562-761`): `data:` events, `[DONE]`; `delta.content` string or chunk array (`text`, `thinking`); empty deltas ignored (GLM quirk);
  tool calls keyed by `index` (id `"null"` replaced), args accumulated; all tool ends emitted after loop. Usage: cached tokens from any of
  `prompt_tokens_details.cached_tokens|promptTokensDetails.cachedTokens|...|num_cached_tokens` (`:541-560`).
- Stop (`:922-937`): stop->stop, length|model_length->length, tool_calls->toolUse, else error. Error format `Mistral API error (<status>): <body truncated 4000>`.

### 1.9 `bedrock-converse-stream` — `api/bedrock-converse-stream.ts`

- AWS SDK v3 `BedrockRuntimeClient.send(ConverseStreamCommand)` (`:251-286`) => `POST /model/{modelId}/converse-stream` (AWS event-stream binary framing).
- **Auth/config** (`:157-243`): profile (`options.profile` / scoped `AWS_PROFILE`) wins over ambient keys; region = ARN region > `region` opt > `AWS_REGION` > `AWS_DEFAULT_REGION` >
  endpoint region > `us-east-1` (when no ambient profile) (`:1194-1201`); explicit endpoint `model.baseUrl` only when non-standard host or no region/profile configured (`:1217-1242`);
  static keys `AWS_ACCESS_KEY_ID/SECRET/SESSION_TOKEN` (`:1203-1215`); bearer token (`bearerToken` opt > apiKey > `AWS_BEARER_TOKEN_BEDROCK`) -> `config.token`, `authSchemePreference:["httpBearerAuth"]`;
  `AWS_BEDROCK_SKIP_AUTH=1` -> dummy credentials; proxy via `NodeHttpHandler` + Http(s)ProxyAgent; `AWS_BEDROCK_FORCE_HTTP1=1`.
  Custom headers injected by build middleware (pre-SigV4), reserved `authorization`, `host`, `x-amz-*` dropped (`:463-494`).
- **Body**: `modelId`, `messages`, `system:[{text}, {cachePoint:{type:"default", ttl?:"1h"}}]`, `inferenceConfig:{maxTokens (Claude default model.maxTokens), temperature}`,
  `toolConfig:{tools:[{toolSpec:{name,description,inputSchema:{json},strict?}}], toolChoice:{auto|any|tool}}` (omitted for `none`) (`:1133-1175`),
  `additionalModelRequestFields` (Claude only, `:1254-1312`): adaptive models (opus/sonnet 4.6+, opus-5, sonnet-5, fable-5) `thinking:{type:"adaptive", display?, block_binding?}` +
  `output_config:{effort}` (+`anthropic_beta:["thinking-binding-controls-2026-08-01"]`); else `thinking:{type:"enabled", budget_tokens}` + `anthropic_beta:["interleaved-thinking-2025-05-14"]`;
  `display` omitted on GovCloud. `requestMetadata` passthrough.
  Messages: images `{image:{format,source:{bytes}}}`, thinking `{reasoningContent:{reasoningText:{text, signature}}}` (signature only for Claude), redacted -> `{reasoningContent:{redactedContent: bytes}}`,
  tool results merged into one user message with status success/error; cache point appended to last user message (`:1122-1131`). Caching only for Claude 3.5-haiku/3.7-sonnet/4.x/5.x or `AWS_BEDROCK_FORCE_CACHE=1`.
- **Stream** (`:300-330`): `messageStart`, `contentBlockStart` (toolUse), `contentBlockDelta` (`text`, `toolUse.input`, `reasoningContent.text|signature|redactedContent`),
  `contentBlockStop`, `messageStop.stopReason`, `metadata.usage` (input, output, cacheReadInputTokens, cacheWriteInputTokens, cacheDetails ttl 1h -> cacheWrite1h, totalTokens);
  in-stream exceptions (`internalServerException`, `modelStreamErrorException`, `validationException`, `throttlingException`, `serviceUnavailableException`) thrown.
  Redacted chunks buffered and base64-joined into `thinkingSignature` (`:688-692`).
- **Stop** (`:1177-1192`): end_turn|stop_sequence->stop, max_tokens|model_context_window_exceeded->length, tool_use->toolUse, else error.
- **Errors** (`:371-460`): prefixes `Internal server error|Model stream error|Validation error|Throttling error|Service unavailable: <msg>`; diagnostic with status/requestId.
- Retry/timeout: SDK default retry strategy; `maxRetries`/`timeoutMs` not wired.

### 1.10 `pi-messages` — `api/pi-messages.ts` (Radius gateway protocol)

- `POST {baseUrl}/messages[?debug=1]`, headers `authorization: Bearer <key>`, `accept: text/event-stream`, `content-type: application/json` + options headers (`:355-410`).
- Body: `{model: id, context: TranscriptContext, options:{temperature, maxTokens, reasoning, cacheRetention, sessionId, toolChoice}}`.
- Response: SSE `data:` frames each holding a serialized pi event (`PiMessagesEvent` `:54-88`): `start|text_*|thinking_*|toolcall_*`, `done{reason, usage, responseId, providerThinkingLevel, rewrite}`,
  `error{...}`; `contentSignature` -> `textSignature`/`thinkingSignature`, `redacted` passthrough. Non-2xx -> `PiMessagesResponseError` (`<status> <statusText>: <message> (<code>)`). No retry.

### 1.11 Cloudflare wrappers (not separate API kinds)

- `api/cloudflare.ts:1-19` base URL templates: Workers AI `https://api.cloudflare.com/client/v4/accounts/{CLOUDFLARE_ACCOUNT_ID}/ai/v1`,
  AI Gateway `https://gateway.ai.cloudflare.com/v1/{CLOUDFLARE_ACCOUNT_ID}/{CLOUDFLARE_GATEWAY_ID}/{compat|openai|anthropic}`.
- `providers/cloudflare-stream.ts`: `cloudflareStreams()` substitutes `{CLOUDFLARE_ACCOUNT_ID}`/`{CLOUDFLARE_GATEWAY_ID}` from the resolved provider `env` before dispatching to openai-completions / openai-responses / anthropic-messages.
- `api/cloudflare-ai-binding.ts`: Workers `env.AI.fetch` passthrough + sentinel auth header (Worker-only; irrelevant for desktop port).

### 1.12 Faux — `providers/faux.ts`

- API `faux`, provider `faux`, model `faux-1`, baseUrl `http://localhost:0` (`:24-30`). Scripted responses (`setResponses`, factories), token-sized deltas
  (3-5 tokens, `tokensPerSecond` pacing `:331-337`), usage estimate (chars/4 with prefix cache simulation), abort -> aborted message, deferred handles. Only adapter implementing `fetchDeferred/cancelDeferred`.
  Registered via `compat.ts:162` (`registerFauxProvider`).

### 1.13 Non-chat APIs (FYI)

`openrouter-images` (`api/openrouter-images.ts`, chat-completions images), classifiers `typesafe-system-one` (`POST {base}/systemone`, Bearer),
`cloudflare-workers-ai-system-one` (`POST {base}/run`), `llama-cpp-classify` (`/completion`). Shared in `api/system-one-shared.ts`.

### 1.14 Cross-provider replay normalization — `api/transform-messages.ts:64-235`

- Images replaced with placeholder text when `model.input` lacks `image`.
- Same model = same provider AND api AND model id. Cross-model: redacted thinking dropped; thinking -> text (empty dropped); `thoughtSignature` stripped; tool ids normalized via adapter callback (mapping applied to results).
- Errored/aborted assistant messages are skipped entirely.
- Orphan tool calls get synthetic `toolResult{isError:true, "No result provided"}` before the next assistant/user or at the end; system messages arriving between call and result are held until results close.

---

## 2. Provider ids

Auth helper `envApiKeyAuth(name, envVars)` (`auth/helpers.ts:9-31`): stored key wins, else first non-empty env var. Env map also in `env-api-keys.ts:73-127`.
Catalog JSON: `providers/data/<id>.json` = `{ "<api>": { "<type>:<modelId>": Model } }`, manifest `providers/data/.manifest.json` (schemaVersion 6, generatedAt 2026-10-04).
Registry of built-ins: `providers/all.ts`. Default model per provider: `coding-agent/core/model-resolver.ts:20-62`.

| Provider id | API kind(s) (catalog count) | Base URL | API key env | Other auth | Notes |
|---|---|---|---|---|---|
| amazon-bedrock | bedrock-converse-stream (185) | https://bedrock-runtime.us-east-1.amazonaws.com (some eu-central-1) | (bearer) AWS_BEARER_TOKEN_BEDROCK | AWS profile / keys / ECS / IRSA (`providers/amazon-bedrock.ts`) | compat supportsStrictMode |
| ant-ling | openai-completions (3) | https://api.ant-ling.com/v1 | ANT_LING_API_KEY | - | thinkingFormat ant-ling |
| anthropic | anthropic-messages (16) | https://api.anthropic.com | ANTHROPIC_OAUTH_TOKEN, ANTHROPIC_API_KEY; ANTHROPIC_AUTH_TOKEN -> `Authorization: Bearer` | federation env (1.1); OAuth "Anthropic (Claude Pro/Max)" = RESERVED | `providers/anthropic.ts`; compat forceAdaptiveThinking, supportsMidConvo*, allowedFallbackModels, promptCache {short:300,long:3600} |
| azure-openai-responses | azure-openai-responses (44) | "" (from env, 1.4) | AZURE_OPENAI_API_KEY | - | AZURE_OPENAI_BASE_URL / RESOURCE_NAME / API_VERSION / DEPLOYMENT_NAME_MAP |
| baseten | openai-completions (22) | https://inference.baseten.co/v1 | BASETEN_API_KEY | - | thinkingFormat baseten, chatTemplateArgs |
| cerebras | openai-completions (2) | https://api.cerebras.ai/v1 | CEREBRAS_API_KEY | - | bodyless 400/413 => overflow |
| cloudflare-ai-gateway | anthropic-messages (12), openai-completions (18, `/compat`), openai-responses (24, `/openai`) | gateway URL template | CLOUDFLARE_API_KEY (+ CLOUDFLARE_ACCOUNT_ID, CLOUDFLARE_GATEWAY_ID) | auth sent as `cf-aig-authorization: Bearer <key>` with `Authorization: null`, `x-api-key: null` (`providers/cloudflare-auth.ts:74-103`) | |
| cloudflare-workers-ai | openai-completions (18), classifier (3) | Workers AI URL template | CLOUDFLARE_API_KEY (+ CLOUDFLARE_ACCOUNT_ID) | - | |
| deepseek | openai-completions (2) | https://api.deepseek.com | DEEPSEEK_API_KEY | - | thinkingFormat deepseek, reasoning_content replay |
| fireworks | anthropic-messages (14, base /inference), openai-completions (8, /inference/v1) | https://api.fireworks.ai/inference | FIREWORKS_API_KEY | - | anthropic compat: sendSessionAffinityHeaders, no cache_control on tools, allowEmptySignature |
| github-copilot | anthropic-messages (10), openai-completions (6), openai-responses (18) | https://api.individual.githubcopilot.com (per-credential from token `proxy-ep`) | COPILOT_GITHUB_TOKEN | OAuth device flow (4.4) | static model headers `User-Agent: GitHubCopilotChat/0.35.0`, `Editor-Version: vscode/1.107.0`, `Editor-Plugin-Version: copilot-chat/0.35.0`, `Copilot-Integration-Id: vscode-chat`; dynamic `X-Initiator: user|agent`, `Openai-Intent: conversation-edits`, `Copilot-Vision-Request: true` (`api/github-copilot-headers.ts`); `filterModels` by `credential.availableModelIds` |
| google | google-generative-ai (22) | https://generativelanguage.googleapis.com/v1beta | GEMINI_API_KEY | - | |
| google-vertex | google-vertex (14) | https://{location}-aiplatform.googleapis.com | GOOGLE_CLOUD_API_KEY | gcloud ADC / service account + GOOGLE_CLOUD_PROJECT(/GCLOUD_PROJECT) + GOOGLE_CLOUD_LOCATION (`providers/google-vertex.ts:13-90`, ADC default path `~/.config/gcloud/application_default_credentials.json`) | |
| groq | openai-completions (7) | https://api.groq.com/openai/v1 | GROQ_API_KEY | - | |
| huggingface | openai-completions (76) | https://router.huggingface.co/v1 | HF_TOKEN | - | |
| kimi-coding | anthropic-messages (4) | https://api.kimi.com/coding | KIMI_API_KEY | OAuth device flow (4.6) | compat forceAdaptiveThinking, allowEmptySignature |
| meta | openai-responses (5) | https://api.meta.ai/v1 | META_API_KEY | OAuth device flow + API key mint (4.7) | |
| minimax / minimax-cn | anthropic-messages (3 each) | https://api.minimax.io/anthropic / https://api.minimaxi.com/anthropic | MINIMAX_API_KEY / MINIMAX_CN_API_KEY | - | |
| mistral | mistral-conversations (32) | https://api.mistral.ai | MISTRAL_API_KEY | - | |
| moonshotai / moonshotai-cn | openai-completions (4 each) | https://api.moonshot.ai/v1 / https://api.moonshot.cn/v1 | MOONSHOT_API_KEY | - | Kimi mid-convo tool additions |
| nvidia | openai-completions (21) | https://integrate.api.nvidia.com/v1 | NVIDIA_API_KEY | - | model header `NVCF-POLL-SECONDS: 3600` |
| openai | openai-responses (44) | https://api.openai.com/v1 | OPENAI_API_KEY | OAuth "Sign in with ChatGPT" (4.3) | compat explicit prompt cache, additional tools, tool search, grammar tools |
| openai-codex | openai-codex-responses (9) | https://chatgpt.com/backend-api | (none) | OAuth only (4.2) | "OpenAI Codex (legacy)" |
| opencode | anthropic-messages (17, /zen), google-generative-ai (7), openai-completions (26), openai-responses (30) (all /zen/v1), typesafe (2) | https://opencode.ai/zen(/v1) | OPENCODE_API_KEY | - | `x-opencode-session: <sessionId>` added by `providers/opencode-headers.ts` (+ coding-agent adds `x-opencode-client: pi`) |
| opencode-go | anthropic-messages (4, /zen/go), openai-completions (19), openai-responses (6) (/zen/go/v1) | https://opencode.ai/zen/go(/v1) | OPENCODE_API_KEY | - | same session header |
| openrouter | anthropic-messages (15, https://openrouter.ai/api), openai-completions (385), images (59), typesafe (13) | https://openrouter.ai/api/v1 | OPENROUTER_API_KEY | OAuth PKCE -> API key (4.5) | x-session-id affinity, anthropic cache_control for `anthropic/*`, routing `provider` field |
| qwen-token-plan / -individual / -cn | openai-completions (20 / 9 / 20) | https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1 (cn: cn-beijing) | QWEN_TOKEN_PLAN_API_KEY (both intl) / QWEN_TOKEN_PLAN_CN_API_KEY | - | thinkingFormat qwen* |
| radius | pi-messages (28 baseline) | https://radius.pi.dev/v1 (gateway https://radius.pi.dev) | RADIUS_API_KEY | OAuth (4.8); dynamic catalog from `{gateway}/v1/config` (`providers/radius-config.ts:4,87`) | models.json `"oauth": "radius"` creates more Radius providers |
| together | openai-completions (17) | https://api.together.ai/v1 | TOGETHER_API_KEY | - | thinkingFormat together |
| typesafe | classifier only (1) | https://api.typesafe.ai/v1/ | TYPESAFE_API_KEY | - | |
| vercel-ai-gateway | anthropic-messages (253), typesafe (4) | https://ai-gateway.vercel.sh | AI_GATEWAY_API_KEY | - | |
| xai | openai-responses (4) | https://api.x.ai/v1 | XAI_API_KEY | OAuth device flow (4.9) | always include encrypted reasoning |
| xiaomi / -token-plan-cn / -ams / -sgp | openai-completions (6/4/4/4) | https://api.xiaomimimo.com/v1, https://token-plan-{cn,ams,sgp}.xiaomimimo.com/v1 | XIAOMI_API_KEY, XIAOMI_TOKEN_PLAN_{CN,AMS,SGP}_API_KEY | - | length-stop overflow detection |
| zai / zai-coding-cn | openai-completions (7 / 4) | https://api.z.ai/api/coding/paas/v4 / https://open.bigmodel.cn/api/coding/paas/v4 | ZAI_API_KEY / ZAI_CODING_CN_API_KEY | - | thinkingFormat zai, tool_stream |
| faux | faux | http://localhost:0 | - | - | tests/demo |

`getEnvApiKey()` (`env-api-keys.ts:151-195`) returns `"<authenticated>"` for google-vertex (ADC file + project + location) and amazon-bedrock (any AWS credential source); skips `ANTHROPIC_AUTH_TOKEN`.

coding-agent request-time extras (`coding-agent/core/provider-attribution.ts`, applied in `coding-agent/core/sdk.ts:316-330` via `transformHeaders`, only when install telemetry enabled):
OpenRouter `HTTP-Referer: https://pi.dev`, `X-OpenRouter-Title: pi`, `X-OpenRouter-Categories: cli-agent`; NVIDIA `X-BILLING-INVOKE-ORIGIN: Pi`;
Cloudflare `User-Agent: pi-coding-agent`; OpenCode (always, when sessionId) `x-opencode-session`, `x-opencode-client: pi`.

---

## 3. Normalized types (`types.ts`)

### 3.1 Messages / content

```
TextContent      {type:"text", text, textSignature?}                 // :397-401 (Responses item id / TextSignatureV1 JSON; Gemini thoughtSignature)
ThinkingContent  {type:"thinking", thinking, thinkingSignature?, redacted?}   // :403-411 (Anthropic signature | Responses reasoning item JSON | OpenRouter reasoning_details JSON | completions field name | Gemini signature | redacted payload)
ImageContent     {type:"image", data(base64), mimeType}               // :413-417
ToolCall         {type:"toolCall", id, name, arguments: JsonObject, thoughtSignature?, namespace?}  // :419-427
SystemMessage    {role:"system", content: string|TextContent[], sections?: Record<string,string|null>, toolsAdded?: Tool[], toolsRemoved?: {name}[], timestamp}  // :524-540
UserMessage      {role:"user", content: string|(Text|Image)[], timestamp}  // :542-546
AssistantMessage {role:"assistant", content:(Text|Thinking|ToolCall)[], api, provider, model, responseModel?, responseId?,
                  providerThinkingLevel?, thinkingLevel?, diagnostics?, usage, stopReason, deferred?, errorMessage?, rawStopReason?, endTurn?, timestamp}  // :548-572
ToolResultMessage{role:"toolResult", toolCallId, toolName, content:(Text|Image)[], details?, usage?, nestedCalls?, isError, timestamp}  // :596-610
Message = System | User | Assistant | ToolResult                         // :612
Tool {name, description, parameters: JSON Schema (TypeBox), constrainedSampling?: false | {type:"json_schema", strict:"prefer"|"require"} | {type:"grammar", variants:{openai_lark?, openai_regex?}}}  // :695-722
Context {systemPrompt?, messages, tools?}  -> normalizeContext -> TranscriptContext {messages} with a leading SystemMessage (timestamp 0) carrying prompt + toolsAdded  // :734-751, utils/transcript.ts:10-34
```
System message replay: `getCurrentTools`, `getCurrentSystemMessage`, `collapseSystemMessages` (APIs without mid-convo system messages fold later system messages into the leading one), `resolveTranscript(ctx, supportsMidConvo)` (`utils/transcript.ts:58-121`);
rendering `getSystemMessageText` = content + sections joined `\n\n`; `renderSystemMessageUpdate` frames section changes (`utils/text.ts`).

### 3.2 Usage (`:429-450`)
`input` (non-cached input), `output` (includes reasoning), `cacheRead`, `cacheWrite`, `cacheWrite1h?` (subset of cacheWrite), `reasoning?` (subset of output), `totalTokens`,
`cost{input, output, cacheRead, cacheWrite, total}` in USD. `calculateCost` (`models.ts:1198-1218`): rates are $/M tokens; tier = highest `inputTokensAbove` < (input+cacheRead+cacheWrite);
1h cache writes priced at 2x input rate.

### 3.3 StopReason (`:452`)
`"pending" | "stop" | "length" | "toolUse" | "error" | "aborted" | "deferred"`. `done` carries stop/length/toolUse/deferred; `error` carries error/aborted (`:769-785`).

### 3.4 Event protocol (`:769-785`)
`start{partial}`, `text_start|thinking_start|toolcall_start{contentIndex, partial}`, `*_delta{contentIndex, delta, partial}`, `text_end|thinking_end{contentIndex, content}`,
`toolcall_end{contentIndex, toolCall}`, `done{reason, message}`, `error{reason, error}`. `partial` is the live shared object (not a snapshot).
`AssistantMessageEventStream` = async-iterable FIFO with `result()` promise (`utils/event-stream.ts:26-110`).

### 3.5 Model (`:1099-1146`)
`id, name, api, provider, baseUrl, input: ("text"|"image")[], inputLimits?{maxRequestBytes, images{resize{maxWidth,maxHeight,maxBytes,jpegQuality}, maxPerMessage, maxPerRequest}},
cost{input, output, cacheRead, cacheWrite, tiers?[{inputTokensAbove,...}]}, headers?, type?:"chat", reasoning: bool, thinkingLevelMap?, promptCache?{short?,long? seconds},
contextWindow, maxTokens, samplingParams?, samplingParamsByThinkingLevel?, compat?` (compat type depends on api: completions `:791-868`, responses/azure/codex `:871-892`,
anthropic `:895-957`, bedrock `:960-963`, mistral `:966-969`). Image models add `type:"image", output[]`; classifier `type:"classifier"`.

### 3.6 Options
- `ProviderRequestOptions` (`:134-187`): `signal, telemetryContext, apiKey, fetch, env (ProviderEnv overrides process.env), onPayload, onResponse, headers (null deletes), timeoutMs, maxRetries, maxRetryDelayMs (default 60000, 0 = no cap)`.
- `StreamOptions` (`:189-239`): + `onProviderStreamEvent, temperature, samplingParams, maxTokens, transport ("sse"|"websocket"|"websocket-cached"|"auto"), cacheRetention ("none"|"short"|"long", default short), sessionId, websocketConnectTimeoutMs, metadata`.
- `SimpleStreamOptions` (`:352-360`): + `toolChoice ("auto"|"none"), reasoning?: ThinkingLevel, deferred?, thinkingBudgets?{minimal,low,medium,high}`.
- Per-API option interfaces: `AnthropicOptions` (`api/anthropic-messages.ts:231-291`), `OpenAICompletionsOptions` (`api/openai-completions.ts:154-159`), `OpenAIResponsesOptions` (`api/openai-responses.ts:117-122`),
  `OpenAICodexResponsesOptions` (`api/openai-codex-responses.ts:79-85`), `AzureOpenAIResponsesOptions` (`api/azure-openai-responses.ts:58-66`), `GoogleOptions` (`api/google-generative-ai.ts:47-54`),
  `GoogleVertexOptions` (+project, location), `MistralOptions` (`api/mistral-conversations.ts:37-39`), `BedrockOptions` (`api/bedrock-converse-stream.ts:79-118`), `PiMessagesOptions` (`api/pi-messages.ts:33-38`).

### 3.7 Thinking levels
`ThinkingLevel = minimal|low|medium|high|xhigh|max`, `ModelThinkingLevel = off|...` (`:85-87`). `thinkingLevelMap[level]`: string = provider-native value, `null` = unsupported, missing = default
(xhigh/max are only supported when explicitly mapped). `getSupportedThinkingLevels` / `clampThinkingLevel` (`models.ts:1220-1252`: nearest supported level upward, then downward).

| API | off | levels |
|---|---|---|
| anthropic-messages | `thinking:{type:"disabled"}` (unless map.off null); managed effort keeps adaptive | adaptive: `output_config.effort` low/medium/high (+xhigh/max via map); budget: 1024/2048/8192/16384 tokens |
| openai-completions | per thinkingFormat (`reasoning_effort` = map.off, `thinking:{type:"disabled"}`, `enable_thinking:false`, `reasoning:{effort:"none"}`...) | `map[level] ?? level` as effort; optional token budget |
| openai-responses / azure | `reasoning.effort = map.off ?? "none"` | `reasoning.effort = map[level] ?? level`, summary auto |
| openai-codex-responses | `reasoning.effort = map.off ?? "none"` | same + `text.verbosity` |
| google / vertex | `thinkingBudget:0` or lowest thinkingLevel | thinkingLevel MINIMAL..HIGH or model-specific budgets; xhigh/max -> high |
| mistral | `reasoning_effort = map.off` or omitted | `reasoning_effort = map[level] ?? "high"` or `prompt_mode:"reasoning"` |
| bedrock (Claude) | omitted | adaptive effort (xhigh native on opus-4-7/4-8/5, sonnet-5, fable-5) or budget 1024..16384 |
| pi-messages | passes `reasoning` level through | |

---

## 4. OAuth / auth

### 4.0 Framework (`auth/`)
- Credential = `{type:"api_key", key?, env?}` | `{type:"oauth", access, refresh, expires(ms epoch), ...extra}` (`auth/types.ts:17-37`). One per provider id.
- `CredentialStore {read, list, modify(fn) (serialized per provider, only write path), delete}` (`auth/types.ts:65-94`); default `InMemoryCredentialStore` (`auth/credential-store.ts`).
- `ApiKeyAuth {name, login?(interaction), check?, resolve({ctx, credential?, signal}) -> {auth:{apiKey?, headers?, baseUrl?}, env?, source?}}`;
  `OAuthAuth {name, isSubscription?, loginLabel?, login(interaction, {getDeviceId?}), refresh(credential, signal), toAuth(credential)}` (`auth/types.ts:170-240`).
- Interaction: `prompt({type: text|secret|select|manual_code, message, placeholder?, options?, signal?}) -> string`, `notify({type: info|auth_url|device_code|progress, ...})` (`auth/types.ts:125-161`).
- Resolution order (`auth/resolve.ts:46-93`): explicit `apiKey` override -> stored credential (oauth -> OAuth path; api_key -> ApiKeyAuth.resolve with stored key) -> ambient (`apiKey.resolve` with env/files). Stored credential owns the provider: no env fallback after it.
- OAuth refresh (`auth/resolve.ts:102-191`): refresh when `now + max(5 min, minOAuthValidityMs) >= expires`; done inside `store.modify` (double-checked under lock), refresh call bounded by 15 s timeout and NOT cancelled by caller signal; failures -> `ModelsError("oauth")`.
- `lazyOAuth` (`auth/helpers.ts:40-59`) + `auth/oauth/load.ts` (variable-specifier dynamic import; Bun can pre-register bundled loaders).
- Shared flow helpers: PKCE S256 with 32 random bytes base64url (`auth/oauth/pkce.ts`); loopback server `startOAuthCallbackServer({host, port (0=any), path, state?, complete, timeoutMs})` with
  state check, 404/400/409 pages, `redirectUri = http://<host>:<port><path>` (`auth/oauth/callback-server.ts:50-148`); `waitForCallbackOrManualInput` races loopback vs `manual_code` paste (`:155-183`);
  RFC 8628 device polling `pollOAuthDeviceCodeFlow` (default interval 5 s, min 1 s, slow_down +5 s or server interval, deadline = expires_in) (`auth/oauth/device-code.ts`).
  Callback host override env `PI_OAUTH_CALLBACK_HOST` (default 127.0.0.1).

### 4.1 Anthropic (Claude Pro/Max) — RESERVED, location only
- Flow module: `auth/oauth/anthropic.ts` — symbols `parseAuthorizationInput` (:27), `postJson` (:74), `exchangeAuthorizationCode` (:94), `loginAnthropic` (:138, browser),
  `loginAnthropicCopyCode` (:191, headless copy-code), `refreshAnthropicToken` (:231), export `anthropicOAuth: OAuthAuth` (:269-298).
- Loader `loadAnthropicOAuth` (`auth/oauth/load.ts:33-36`); provider wiring `providers/anthropic.ts` (`lazyOAuth({name:"Anthropic (Claude Pro/Max)", isSubscription:true})`).
- Request-side handling: `api/anthropic-messages.ts` symbols listed in 1.1 (RESERVED). Env var `ANTHROPIC_OAUTH_TOKEN` is read as an API-key source by `providers/anthropic.ts`.

### 4.2 OpenAI Codex (ChatGPT Plus/Pro, provider `openai-codex`) — `auth/oauth/openai-codex.ts`
- Public client id `app_EMoamEEZ73f0CkXaXp7hrann`; auth base `https://auth.openai.com`; scope `openid profile email offline_access` (`:22-35`).
- Login prompt: select `browser` (default) | `device_code` (`:406-428`).
- Browser: PKCE + 16-byte hex state; authorize `https://auth.openai.com/oauth/authorize?response_type=code&client_id&redirect_uri=http://localhost:1455/auth/callback&scope&code_challenge&code_challenge_method=S256&state&id_token_add_organizations=true&codex_cli_simplified_flow=true&originator=pi` (`:289-308`);
  loopback on `127.0.0.1:1455` path `/auth/callback` (if bind fails, manual paste only); manual input accepts URL, `code#state`, query string or raw code (`:69-97`).
- Device: `POST /api/accounts/deviceauth/usercode {client_id}` -> `{device_auth_id, user_code, interval}`; verification URI `https://auth.openai.com/codex/device`; poll `POST /api/accounts/deviceauth/token {device_auth_id, user_code}`
  (403/404/`deviceauth_authorization_pending` = pending, `slow_down`) -> `{authorization_code, code_verifier}`, exchanged with redirect `https://auth.openai.com/deviceauth/callback`; 15 min timeout (`:187-357`).
- Token: `POST https://auth.openai.com/oauth/token` form `grant_type=authorization_code|refresh_token, client_id, code, code_verifier, redirect_uri | refresh_token` -> `{access_token, refresh_token, expires_in}`;
  credential `{type:"oauth", access, refresh, expires: now+expires_in*1000, accountId}` (accountId from JWT claim, required) (`:145-185`, `:317-330`).
- `toAuth` -> `{apiKey: access}`; the adapter re-derives `chatgpt-account-id` from the JWT.

### 4.3 OpenAI "Sign in with ChatGPT" (provider `openai`) — `auth/oauth/openai-chatgpt.ts`
- Dynamic client registration style: authorize `https://auth.openai.com/api/accounts/authorize` with `client_id=dynamic_agent_client`, `agent_name_hint=Pi`, `ext_agent_host_id=urn:uuid:<deviceId>`
  (requires `LoginOptions.getDeviceId()` UUID), `response_type=code`, `redirect_uri=http://127.0.0.1:1455/auth/callback`, `resource=https://api.openai.com/v1`,
  scope `openid profile email offline_access resource.invoke chatgpt.tokens.use.direct`, state, PKCE S256, nonce (`:16-29`, `:233-298`).
- Callback returns the issued `client_id` alongside `code`; stored as `credential.clientId`; token `POST https://auth.openai.com/api/accounts/oauth/token` (form, includes `resource`); response must include `id_token`
  and scope containing `chatgpt.tokens.use.direct`; `expires = now + expires_in - 3 min`; credential keeps `scopes`, `clientId`. Refresh uses stored clientId (`:162-224`).
- Port 1455 in use -> explicit error (shared with Codex CLI). `toAuth` -> `{apiKey: access}`; request path is plain openai-responses with field omissions (1.3).

### 4.4 GitHub Copilot — `auth/oauth/github-copilot.ts`
- Device flow. Client id is a public GitHub OAuth App id stored base64-obfuscated: `decode("SXYxLmI1MDdhMDhjODdlY2ZlOTg=")` (`:10-11`). Scope `read:user`.
- Prompt "GitHub Enterprise URL/domain (blank for github.com)" (`:434-445`). URLs per domain: `https://<domain>/login/device/code`, `https://<domain>/login/oauth/access_token`
  (grant `urn:ietf:params:oauth:grant-type:device_code`), Copilot token `GET https://api.<domain>/copilot_internal/v2/token` with `Authorization: Bearer <github token>` + COPILOT_HEADERS (`:52-62`, `:313-349`).
- Credential: `refresh = GitHub access token`, `access = Copilot token`, `expires = expires_at*1000 - 5 min`, `enterpriseUrl`, `availableModelIds` (from `GET {base}/models` with `X-GitHub-Api-Version: 2026-06-01`; on login also POST `{base}/models/{id}/policy {"state":"enabled"}` for policy-gated models) (`:168-195`, `:373-485`).
- Base URL from token `proxy-ep=proxy.X` -> `https://api.X`, else enterprise `https://copilot-api.<domain>`, else `https://api.individual.githubcopilot.com` (`:69-87`). `toAuth` -> `{apiKey: access, baseUrl}`.
- 429 handling: `fetchWithRateLimitRetry` (5 s per attempt, retry-after or 500*2^n) (`:135-166`).

### 4.5 OpenRouter — `auth/oauth/openrouter.ts`
- PKCE loopback on random port, random path `/oauth/callback/<uuid>` (no state); authorize `https://openrouter.ai/auth?callback_url&code_challenge&code_challenge_method=S256`;
  exchange `POST https://openrouter.ai/api/v1/auth/keys` (JSON) -> API key; credential `{access: key, refresh: "", expires: MAX_SAFE_INTEGER}`; `refresh` is identity; 5 min login timeout, 30 s exchange timeout.

### 4.6 Kimi Code — `auth/oauth/kimi-coding.ts`
- Device flow at `https://auth.kimi.com` (override `KIMI_CODE_OAUTH_HOST` / `KIMI_OAUTH_HOST`), public client id `17e5f671-d194-4dfb-9706-5516cb48c098`;
  `POST /api/oauth/device_authorization`, poll `POST /api/oauth/token` (device grant, waits before first poll); refresh same endpoint with up to 3 retries on 429/5xx (1 s, 2 s, 4 s).
  `toAuth` -> `{headers: {Authorization: "Bearer <access>"}}` (header, not apiKey).

### 4.7 Meta — `auth/oauth/meta.ts`
- Device flow `https://auth.meta.com/oidc/device/authorization/` + `/oidc/device/token/`, public client id `1031625952748946`; identity token then mints an API key at
  `POST https://api.meta.ai/muse-code/key` (`x-api-version: 1.0.0`). Credential `{refresh: identityToken, access: apiKey, expires: now+24h}`; "refresh" re-mints; 401/403 => re-login.

### 4.8 Radius — `auth/oauth/radius.ts`
- Discovery `GET {gateway}/v1/oauth` -> `authorizationEndpoint`; client id `pi-gateway`, scope `gateway offline_access`; browser PKCE loopback `127.0.0.1:1456/oauth/callback`
  or device flow `POST {gateway}/v1/oauth/device`; token `POST {gateway}/v1/oauth/token`; `expires = now + expires_in - 60 s`. `toAuth` -> `{apiKey: access}`.

### 4.9 xAI — `auth/oauth/xai.ts`
- Device flow, public client id `b1a00492-073a-47ea-816f-4c329264a828`, scope `openid profile email offline_access grok-cli:access api:access`,
  `https://auth.x.ai/oauth2/device/code`, `https://auth.x.ai/oauth2/token`; expires = now + (expires_in ?? 3600) - 5 min; refresh keeps old refresh token if none returned.

### 4.10 Extension OAuth adapter
`coding-agent/core/provider-composer.ts:40-50, 353-372`: legacy `{login(callbacks), refreshToken, getApiKey, modifyModels?}` mapped onto `OAuthAuth` (callbacks `onAuth/onDeviceCode/onPrompt/onProgress/onManualCodeInput/onSelect`).

### 4.11 auth.json format and storage (`coding-agent/core/auth-storage.ts`)
- Path `<agentDir>/auth.json` (`coding-agent/config.ts:625`). Shape: `{ "<providerId>": Credential }`, pretty JSON (2-space).
  Example: `{"openai-codex": {"type":"oauth","access":"...","refresh":"...","expires":1760000000000,"accountId":"..."}, "openrouter": {"type":"api_key","key":"$OPENROUTER_KEY"}}`.
- Validation (`:203-256`): api_key `key?: string`, `env?: Record<string,string>`; oauth requires string `access`, `refresh`, finite `expires`; extra fields preserved.
- `key` is a config value: `!command`, `$ENV`/`${ENV}` templates, `$$`/`$!` escapes, resolved on read with `credential.env` (`:441-447`, `coding-agent/core/resolve-config-value.ts`).
- File created `0600` in a `0700` dir (`:24-25`, `:59-65`); cross-process locking with `proper-lockfile` (sync lock retries; async lock stale 30 s, with compromised-lock detection) (`:49-200`);
  in-process read cache keyed by file revision. `RuntimeCredentials` overlays non-persisted runtime keys (CLI `--api-key`) (`coding-agent/core/runtime-credentials.ts`, `coding-agent/main.ts:831-834`).

### 4.12 Pipkin provider-auth overlay (`coding-agent/experimental/services/`)
- Service contract `provider-auth.ts`: `SignInProvider = "anthropic" | "openai-codex"`; replicated `ProviderAuthState {credentialsKnown, credentialLookupFailed, hasExistingCredentials, existingProviders[],
  attempt, provider, status: idle|connecting|waiting|prompt|done|failed, message, url, deviceCode, challenge}`; `SignInChallenge {id, type: text|secret|select|manual_code, message, placeholder?, options?}`;
  methods `start(provider)`, `reuse(provider)`, `answer(attempt, challenge, response)`, `cancel(attempt)`, `remove(provider)`; service id `pi.provider-auth` (`provider-auth.ts:1-43`).
- Implementation `createProviderAuthService({getRuntime, getDeviceId})` (`provider-auth-provider.ts:22-319`), wired from `server.ts:37,68`:
  - On creation lists credentials (no secrets) -> `existingProviders` = oauth entries for the two providers; read failure sets `credentialLookupFailed` (fail closed).
  - `start`: rejects other providers / during removal; cancels any active attempt; new attempt UUID; calls `runtime.login(provider, "oauth", {signal, prompt, notify}, {getDeviceId})` in the background; returns immediately.
    `notify(auth_url)` -> status waiting + url + instructions; `device_code` -> url = verificationUri, deviceCode = userCode; `info|progress` -> message.
    `prompt` -> publishes a challenge (copy only, never the answer), resolves when `answer()` matches attempt + challenge id; select answers must be one of the options; response capped at 8192 chars.
    Prompt abort (e.g. callback won the manual_code race) clears the challenge back to `waiting`. Success -> `done` "Connected. Choose a model to continue."; any error -> generic `failed` (error text never replicated).
  - `reuse`: requires a stored oauth credential, then `runtime.refresh({providers:[p], allowNetwork:false})`, checks `getProviderAuthStatus(p).configured` and a model of that provider in `getAvailableSnapshot()`.
  - `cancel`: aborts the login signal, rejects pending prompt, resets state. `remove`: `runtime.logout(provider)` (deletes auth.json entry), refresh list, reset.
  - Device id source: settings `deviceId` (UUID created on first need, `coding-agent/core/settings-manager.ts:1176-1181`).

---

## 5. models.json / custom providers / precedence

- Paths (`coding-agent/config.ts:581-630`): agent dir = `$PI_CODING_AGENT_DIR` (`<APP>_CODING_AGENT_DIR`, tilde-expanded) else `~/<configDir ".pi">/agent`; files `models.json`, `auth.json`, `settings.json`;
  project settings `<cwd>/.pi/settings.json`. Dynamic catalog cache `models-store.json` next to models.json (`coding-agent/core/model-runtime.ts:217-224`).
- Loader `ModelConfig.load` (`coding-agent/core/model-config.ts:294-333`): missing file = empty; JSONC (comments stripped, BOM stripped); TypeBox schema validation; errors surfaced via `getError()` (providers still load without it). Reloaded on `ModelRuntime.refresh()` (`model-runtime.ts:840`).
- Schema (`model-config.ts:84-257`):
```
{ "providers": { "<providerId>": {
    "name"?, "baseUrl"?, "apiKey"? (config value), "api"? (Api string), "oauth"?: "radius",
    "headers"?: {name: configValue}, "compat"?: OpenAICompletionsCompat | OpenAIResponsesCompat | AnthropicMessagesCompat,
    "authHeader"?: bool,                 // add Authorization: Bearer <resolved key>
    "models"?: [ModelDefinition],        // upsert by id
    "modelOverrides"?: { "<modelId>": ModelOverride } } } }
ModelDefinition {id, name?, api?, baseUrl?, reasoning?, thinkingLevelMap?, input?, inputLimits?, cost?{input,output,cacheRead,cacheWrite,tiers?},
                 promptCache?{short?,long?}, contextWindow?, maxTokens?, samplingParams?, samplingParamsByThinkingLevel?, headers?, compat?}
ModelOverride   = same minus id/api/baseUrl; cost fields individually optional
```
  Note: chatTemplateKwargs `$var` in the schema allows only `thinking.enabled|thinking.effort` (types also allow `thinking.budget`).
- Composition (`coding-agent/core/provider-composer.ts`, `composeModelProvider` `:542-683`; runtime `model-runtime.ts:294-312`):
  - A provider with no models.json entry and no extension is the built-in untouched.
  - Provider entry must set at least one of baseUrl/headers/compat/modelOverrides/models/apiKey/oauth/authHeader (`:309-323`); `oauth` requires `baseUrl` and creates a Radius provider at `baseUrl` minus `/v1` (`model-runtime.ts:260-275`).
  - Built-in models get provider `baseUrl` override (except radius) and provider `compat` merged (`:325-330`).
  - New/custom model (`modelFromJson` `:210-249`): `api` = model > provider > defaults (same-id built-in, same-api built-in, any openai-completions, first); `baseUrl` likewise (required);
    defaults `name=id`, `reasoning=false`, `input=["text"]`, `cost=0`, `contextWindow=128000`, `maxTokens=16384`; compat = provider compat merged with model compat (nested routing/kwargs objects shallow-merged).
  - Order of layers: built-in -> models.json provider/baseUrl/compat -> models.json `models` upserts -> extension `registerProvider` models -> extension OAuth `modifyModels` -> models.json `modelOverrides` last (`:553-575`).
  - Streaming of custom APIs: extension `streamSimple` for its api, else built-in provider if it already serves that api, else the global API registry (`getApiProvider(model.api)`, `compat.ts:142`) (`:582-603`).
- Auth precedence for a request (`composeApiKeyAuth` `:417-481` + `auth/resolve.ts`):
  1. Explicit request `apiKey` option (e.g. SDK caller).
  2. Runtime key (CLI `--api-key`, in-memory `RuntimeCredentials`).
  3. Stored auth.json credential for that provider (oauth or api_key; api_key `key` may be `!cmd`/`$ENV`).
  4. models.json `apiKey` (config value; `!cmd` runs via configured shell with 10 s timeout, cached for process lifetime; `$VAR` templates must all resolve).
  5. Built-in ambient resolution (provider env vars from section 2, AWS chain, ADC, Cloudflare env, federation).
  Then models.json/extension provider `headers` (config values) are merged over auth headers; `authHeader:true` adds `Authorization: Bearer <apiKey>`; per-model headers
  (modelOverrides.headers < models[].headers < extension) merged in `ModelRuntime.getAuth` (`model-runtime.ts:547-566`, `provider-composer.ts:508-527`).
  Final request merge: auth headers < caller `options.headers` < `transformHeaders` (attribution); `env` = resolved env + caller env; `auth.baseUrl` (Copilot) replaces `model.baseUrl` (`model-runtime.ts:650-694`).
- Env var lookup order inside adapters: `options.env[name]` (provider-scoped, from stored credential `env` or resolver) > `process.env` > Bun `/proc/self/environ` fallback (`utils/provider-env.ts:45-52`).
- Remote catalog: built-in providers (except radius) refresh from `https://pi.dev/api/models/providers/<id>` (ETag, 4 s attempt timeout, every 4 h) unless `PI_OFFLINE` is set (`coding-agent/core/remote-catalog-provider.ts:13-15,61-140`, `model-runtime.ts:226-238`).
- Settings relevant to models (`coding-agent/core/settings-manager.ts:133-190`): `defaultProvider`, `defaultModel`, `defaultThinkingLevel`, `modelThinkingLevels`, `enabledModels`, `transport`,
  `thinkingBudgets`, `retry`, `httpProxy`, `httpIdleTimeoutMs`, `websocketConnectTimeoutMs`, `cacheWarming`, `deviceId`.

---

## 6. Retry policy, transport, timeouts, proxies, abort

### 6.1 Request-level (SDK-equivalent) retry — `utils/provider-retry.ts:1-125`
- Used by anthropic-messages, openai-completions, openai-responses, azure, google (via `retryGoogleRequest`). Wraps only the initial request (until headers), never mid-stream.
- `maxRetries` default 0 in pi-ai (coding-agent passes `settings.retry.provider.maxRetries`, also default undefined -> 0).
- Retryable: header `x-should-retry: true|false` overrides; else no status (network) or 408/409/429/>=500.
- Delay: `retry-after-ms`, else `retry-after` (seconds or HTTP date), each rejected with `Server requested Ns retry delay (max: Ms)` if > `maxRetryDelayMs` (default 60000, 0 disables);
  else `min(0.5*2^i, 8) s * (1 - rand*0.25)`. Sleep is abortable; abort -> `AbortError("Request aborted")`.
- Codex SSE has its own loop (1.5); Mistral, pi-messages: none; Bedrock: AWS SDK default; Copilot/Kimi OAuth helpers: own small loops.

### 6.2 Agent-level retry (whole assistant turn) — `utils/retry.ts`
- `isRetryableAssistantError(msg)` (`:247-252`): `stopReason=="error"` and errorMessage matches `RETRYABLE_PROVIDER_ERROR_PATTERN` (`:30-102`: overloaded, rate limit, 429/5xx codes, network/socket/timeouts,
  `ended without`, `stream ended before message_stop|a terminal response event`, `retry delay`, explicit retry guidance, `ResourceExhausted`, ChatGPT sharing-unavailable) and NOT
  `NON_RETRYABLE_PROVIDER_LIMIT_ERROR_PATTERN` (`:7-28`: OpenCode Go/Free limits, insufficient_quota, out of budget, quota exceeded, billing, `subscription_sharing_usage_limit_exceeded`).
- `retryAssistantCall` (`:186-236`) / coding-agent `AgentSession` (`coding-agent/core/agent-session.ts:3663-3727`): settings `retry.enabled` (true), `maxRetries` (3), `baseDelayMs` (2000),
  `maxAgentDelayMs` (60000); delay = `baseDelayMs * 2^(attempt-1)` capped (`:123-127`). Aborts never retried. Context overflow handled before retry (compaction).
- Overflow detection `utils/overflow.ts:137-171`: regex list (`:38-62`) excluding throttling (`:74-78`); Cerebras bodyless 400/413; silent overflow (input+cacheRead > contextWindow on stop);
  Xiaomi-style length stop with output 0 and input >= 99% of window. `isRecoverableLength` (`:179-181`).

### 6.3 Timeouts
- `timeoutMs`: passed to OpenAI/Anthropic SDKs (`timeout`, SDK default 10 min); Codex = header timeout for SSE and idle timeout for WebSocket frames; Mistral = whole-request timeout (default 60 s);
  Google/Vertex/Bedrock/pi-messages ignore it.
- coding-agent sets `timeoutMs = settings.retry.provider.timeoutMs ?? httpIdleTimeoutMs` (default 300000; 0 => 2147483647) and `websocketConnectTimeoutMs` from settings (`coding-agent/core/sdk.ts:316-330`).
- Global undici dispatcher (`coding-agent/core/http-dispatcher.ts:81-113`): `EnvHttpProxyAgent` with `headersTimeout = bodyTimeout = httpIdleTimeoutMs` (300 s default, 0 disables), HTTP/1.1 only
  (`allowH2:false`), `proxyTunnel:true`, happy-eyeballs attempt timeout 2 s; replaces global fetch with undici's.
- OAuth refresh: 15 s; OAuth request helpers 30 s (Kimi, Meta), OpenRouter exchange 30 s.

### 6.4 Proxies
- Settings `httpProxy` sets `HTTP_PROXY`/`HTTPS_PROXY` if unset (`http-dispatcher.ts:45-50`); undici `EnvHttpProxyAgent` honours `HTTP(S)_PROXY`/`NO_PROXY` for all fetch-based adapters.
- `utils/node-http-proxy.ts:150-310` (`resolveHttpProxyUrlForTarget`): `<scheme>_proxy` / `all_proxy` (lower then upper case, provider env first), `no_proxy` matching (exact, suffix, `*`, ports, IPv6);
  only http/https proxies (SOCKS/PAC rejected). Used by Bedrock (NodeHttpHandler + proxy agents) and Codex WebSocket under Bun.

### 6.5 Abort
- `options.signal` threads into SDK calls, raw fetches, SSE readers (reader cancelled on abort), WebSocket (close `aborted`), retry sleeps. Abort after start -> `error` event with `stopReason:"aborted"`;
  partial content kept (scratch fields stripped). `combineAbortSignals` (`utils/abort-signals.ts`), `operationSignal`/`raceWithAbortSignal` (`utils/abort.ts`).
- Session resources: `registerSessionResourceCleanup` closes cached Codex WebSockets (`session-resources.ts`, `api/openai-codex-responses.ts:965`).

### 6.6 Error string normalization — `utils/error-body.ts`
- `normalizeProviderError`: status from `statusCode` (Mistral) > `status` (openai/genai) > `$metadata.httpStatusCode` / `$response.statusCode` (AWS); body from string `body`, plain-object `error`, `$response.body`;
  truncated to 4000 chars. `formatProviderError(norm, prefix?)`: `"<prefix> (<status>): <body|message>"` or `"<status>: <body>"`; message kept when it already contains the body.
  The resulting `errorMessage` text is what the retry/overflow regexes classify, so a port must keep these formats stable.
