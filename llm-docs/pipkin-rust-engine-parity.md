# Native engine parity ledger

The parity ledger required by `llm-docs/pipkin-rust-engine-plan.md` §1 step 5 and §2. Every
row is one baseline capability that the native engine must preserve (or a recorded reason it is
not part of the baseline). **Nothing in this ledger is "passed".** A row only passes with a
differential assertion against the frozen reference plus native evidence; no differential run
exists yet. Rows whose native test exists and passed in this worktree say so and cite the test.

- **Reference baseline:** upstream pin `d2a311097cbcf669e699479587332ae3988a49d0` staged by
  `scripts/build-engine.sh` with `packaging/pi-onboarding.patch` + `packaging/pi-onboarding/*.ts`
  (bridge sha256 `370dbef417d1cdeb68239f0d283d4dceb36f5ed539e500e0ba041238aad94e3a`) and
  `packaging/pi-subagents.patch` + `packaging/pi-subagents/*.ts` (subagents sha256
  `9fb846e55a931d829e2ca25bde08ae98d2f128c14ceb6f1bc033b0a2e6fb3664`), model data manifest
  `b92d631bb8bb2cac6f824a03a814baad9452164c518426b8bc4bbc5c2142bb05` (archive
  `2be5f68d90695e71372c01ccbd08d5728b93691e07c45918846997c8949bcabb`), plus the Pipkin desktop
  at app base `75f5c35`.
- **Native revision cited:** branch `native-engine` at `e7cfc42`. Native tests cited as
  "measured" passed on 2026-10-11 with Rust 1.99.0: `cargo test -p pipkin-engine -p
  pipkin-engine-protocol` → 34 + 5 passed, 0 failed, 0 ignored (debug build).
- **Audit sources:** `llm-docs/pipkin-rust-engine-audit/` (model-produced from source reading;
  a claim there is a lead for a differential test, not evidence).
- **ADRs:** `llm-docs/pipkin-rust-engine-adr/0001`–`0007`.

## Legend

Path shorthand (reference, relative to `<pi-staged>/packages/`): `ai/` = `ai/src/`, `ca/` =
`coding-agent/src/`, `ce/` = `ca/experimental/`, `svc/` = `ce/services/`, `D/` = `durable/src/`,
`chord/` = `chord/src/`. Pipkin: `E/` = `crates/pipkin-engine/src/`, `P/` =
`crates/pipkin-engine-protocol/src/`, `App/` = `crates/pipkin-app/src/`, `Core/` =
`crates/pipkin-core/src/`, `pkg/` = `packaging/`. Audit files: `A-tools` =
`engine-tools-durable-usage.md`, `A-prov` = `providers-auth.md`, `A-svc` = `service-api.md`,
`A-ext` = `extensions-subagents.md`, `A-prompt` = `prompt-settings-layout.md`.

Tests: `st::` = `E/store/tests.rs`, `rt::` = `E/runtime/tests.rs`, `cfg::` = `E/config.rs`,
`err::` = `E/provider/errors.rs`, `mdl::` = `E/provider/model.rs`, `sse::` =
`E/provider/sse.rs`, `frm::` = `P/frame.rs`, `cnt::` = `P/content.rs`, `e2e::` =
`App/adapters/pi/e2e.rs` (Pi reference workflow; ignored unless a real engine is configured),
`rpi::` = `App/adapters/pi/real_pi.rs`, `ovl-sub::` = `pkg/pi-subagents/experimental-subagents.test.ts`,
`ovl-auth::` = `pkg/pi-onboarding/provider-auth-provider.test.ts`. "planned:" marks a test
name that does not exist yet.

**Disposition** (plan §1 vocabulary only):

| Code | Plan term |
| --- | --- |
| `NE` | native equivalent |
| `NC` | native implementation of the same contract |
| `NP` | not present in the baseline (evidence cited in the row) |
| `PA` | pending audit |
| `BL` | blocked |
| `RC` | reserved for Codex — not implemented or qualified |
| `LB` | legacy bridge |

`NE`/`NC` here is the **target** disposition; it is not a pass. **Evidence** uses only
`measured`, `observed`, `failed`, `unverified`, `not started`. "not started (in progress)" means a
concurrent worker owns that module and its code is not in this tree. Owner: **Opus** = main
implementation; **Codex** = reserved final integration.

## Status summary (2026-10-11, native `e7cfc42`)

503 rows. **Rows passed: 0.** Rows with differential evidence: 0. Rows with a passing native
unit test covering at least part of the row: 71 (cited per row; none is a differential pass).

| Disposition (target) | Rows |
| --- | --- |
| `NC` native implementation of the same contract | 370 |
| `NE` native equivalent | 23 |
| `NP` not present in the baseline (evidence in row) | 30 |
| `PA` pending audit | 37 |
| `BL` blocked (19 of these also run on the `LB` legacy bridge today) | 28 |
| `LB` legacy bridge only | 1 |
| `RC` reserved for Codex — not implemented or qualified (plus `IMP-AUTH`'s subscription part) | 14 |

| Evidence | Rows |
| --- | --- |
| measured (native unit test passes for part of the row; no differential) | 71 |
| unverified (code exists, no test) | 10 |
| not started (in progress — owned by a concurrent worker, code not in this tree) | 176 |
| not started | 246 |
| observed / failed | 0 |

| Section | Rows | NC | NE | NP | PA | BL | LB | RC |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 Generation (P01) | 20 | 16 | 1 | 2 | 1 | 0 | 0 | 0 |
| 2 Tools (P02) | 23 | 18 | 0 | 5 | 0 | 0 | 0 | 0 |
| 3 Wire adapters (P03) | 34 | 26 | 2 | 1 | 4 | 0 | 0 | 1 |
| 4 Provider ids (P03) | 43 | 41 | 1 | 0 | 1 | 0 | 0 | 0 |
| 5 Provider config (P03/P19) | 14 | 9 | 1 | 2 | 2 | 0 | 0 | 0 |
| 6 Credentials / OAuth (P04) | 59 | 35 | 1 | 1 | 9 | 0 | 0 | 13 |
| 7 Service API / transport | 48 | 33 | 0 | 5 | 4 | 6 | 0 | 0 |
| 8 Durable / queue / Stop (P05–P08) | 43 | 38 | 0 | 2 | 3 | 0 | 0 | 0 |
| 9 Subagents (P09/P10) | 17 | 17 | 0 | 0 | 0 | 0 | 0 | 0 |
| 10 Attachments / models / usage (P11) | 9 | 7 | 2 | 0 | 0 | 0 | 0 | 0 |
| 11 Prompt / resources (P12) | 23 | 16 | 0 | 4 | 3 | 0 | 0 | 0 |
| 12 Context / compaction (P13) | 14 | 12 | 0 | 1 | 1 | 0 | 0 | 0 |
| 13 Extensions / questions (P14/P15) | 24 | 1 | 2 | 5 | 1 | 15 | 0 | 0 |
| 14 Tool output (P16) | 5 | 5 | 0 | 0 | 0 | 0 | 0 | 0 |
| 15 Settings / env | 34 | 27 | 0 | 1 | 6 | 0 | 0 | 0 |
| 16 Desktop workflows (P05–P19) | 60 | 41 | 10 | 0 | 2 | 6 | 1 | 0 |
| 17 Import (plan §6) | 15 | 14 | 0 | 0 | 0 | 1 | 0 | 0 |
| 18 Packaging / platform (P20) | 18 | 14 | 3 | 1 | 0 | 0 | 0 | 0 |

Counts are produced by parsing the tables (first disposition code per row); re-run the count
whenever rows change. Release gates: native beta needs every mandatory row implemented; native
default and Pi retirement need every row passing with no `PA`, `BL`, `LB` or `RC` left (plan §1
"Release gates").

---

## 1. Generation and run loop (P01)

| ID | Source | Baseline behavior | Fixture / test | Owner · module | Migration mapping | Differential assertion | Coverage | Disp. | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| GEN-01 | D/harness/generation.ts (prepare→request→classify) | Multi-turn: user → streamed assistant → answer settles run `done` | rt::a_prompt_is_admitted_durably_streamed_settled_and_looked_up | Opus · E/runtime/run.rs | `pi.user`/`pi.assistant` entries → native `entries` | Same provider request sequence and terminal outcome for a scripted 3-turn exchange | scripted; every adapter | NC | measured: native unit pass; differential not started |
| GEN-02 | D/harness/generation.ts; ai/types.ts:769-785 | Text and thinking streamed as separate content blocks; partial committed every `partialIntervalMs`=100 ms into `pi.live.generation.message` | st::streaming_chunks_are_appended_and_materialized_then_dropped_on_finalize | Opus · E/runtime/run.rs, E/store (`entry_chunks`) | live partial → materialized aborted entry on import | Committed partial content equals reference partial at kill points | scripted | NC | measured: native unit pass; differential not started |
| GEN-03 | D/harness/generation.ts `startToolRound` | `toolUse` → append assistant, one tool task per offered call, parallel unless a tool/setting is sequential, wait allSettled | planned: rt::tool_round_parallel_and_sequential | Opus · E/runtime/run.rs + E/tools (in progress) | `pi.tool-result` → tool_calls + entries | Tool result order and content identical; sequential mode serializes | scripted | NC | not started (in progress) |
| GEN-04 | D/harness/tool.ts:64-66; generation.ts:569-574 | Unknown/unoffered tool → error result `Tool <name> is not available`, code `tool_unavailable` | planned: rt::unoffered_tool_call_is_unavailable | Opus · E/tools::unavailable | n/a | Result text and `<harness>` block byte-identical | scripted | NC | unverified (helper exists, no test) |
| GEN-05 | D/harness/context.ts:129-163 | Orphan tool call gets synthetic `Tool result unavailable: history ends before this call completed.`; unmatched results dropped | planned: ctx::orphan_tool_call_synthesized | Opus · E/context (in progress) | applied at context derivation of imported history | Next request messages identical | scripted | NC | not started (in progress) |
| GEN-06 | D/harness/context.ts:8,73-80 | Assistant messages with stopReason aborted/error/deferred excluded from next context | rt::stop_settles_aborted_and_retires_the_instruction_from_the_next_context | Opus · E/runtime/glue.rs | entries keep stopReason | Next request excludes them | scripted | NC | measured: native unit pass (aborted case); differential not started |
| GEN-07 | D/harness/generation.ts; ai/utils/retry.ts:186-252 | Whole-turn retry on retryable error: enabled, maxRetries 3, base 2000 ms ×2^n, cap 60 s; error assistant appended; aborts never retried | rt::model_errors_retry_then_settle_and_missing_models_are_reported; err::classification_matches_the_reference_patterns | Opus · E/runtime/run.rs, E/provider/errors.rs | `retry` phase durable `until` → resume retry on reopen | Same attempts, delays (scaled), appended error entries | scripted | NC | measured: native unit pass; differential not started |
| GEN-08 | ai/utils/retry.ts:7-28 | Non-retryable provider-limit patterns (quota, billing, subscription sharing) settle `model_error` | err::classification_matches_the_reference_patterns | Opus · E/provider/errors.rs | n/a | Pattern table identical on reference corpus | text classification | NE | measured: native unit pass; differential not started |
| GEN-09 | ai/utils/overflow.ts:38-181; generation.ts | Context overflow detection (regex list, Cerebras bodyless 400/413, silent overflow, Xiaomi length-0) → one blocking `overflow` compaction per attempt, then re-prepare or fail `model_error` | err:: (patterns); planned: rt::overflow_compacts_once_then_retries | Opus · E/provider/errors.rs + E/context (in progress) | n/a | Same compaction trigger and outcome | scripted | NC | measured (patterns only); recovery not started |
| GEN-10 | D/harness/generation.ts:194-199 | Stream options per request: settings.stream, signal, `sessionId` (pi.provider), reasoning = thinking unless off | planned: prov::request_carries_session_id_and_reasoning | Opus · E/runtime/run.rs | `pi.provider.sessionId` → kv/session field | Captured request headers/body equal | every adapter | NC | not started |
| GEN-11 | D/harness/generation.ts deferred/poll | `deferred` stop reason → `poll` phase with durable `pollAt` (only faux implements fetchDeferred) | planned: rt::deferred_is_not_offered | Opus · E/runtime | n/a | Reachable only with faux; native scripted equivalent | faux only | PA | not started |
| GEN-12 | ai/api/transform-messages.ts:64-235 | Cross-provider replay: images→placeholder when unsupported; cross-model thinking→text, signatures stripped, tool ids normalized, errored/aborted skipped | planned: prov::cross_model_replay_normalization | Opus · E/provider (in progress) | stored opaque blocks preserved | Converted request body equal for provider/model switch fixtures | all adapters | NC | not started (in progress) |
| GEN-13 | ai/types.ts:397-427; P/content.rs | Opaque signatures (textSignature/thinkingSignature/thoughtSignature, redacted) kept for valid replay, out of UI/diagnostics | cnt::reference_shaped_messages_round_trip_including_unknown_fields | Opus · P/content.rs | entry model JSON round-trips unknown fields | Replayed body includes identical opaque fields | all adapters | NC | measured: native unit pass (round-trip); differential not started |
| GEN-14 | adapters `mapStopReason` (A-prov §1) | Per-API stop reason mapping; unknown → error | planned: per-adapter golden tests | Opus · E/provider/api (in progress) | n/a | Same `stopReason`/`rawStopReason` | per adapter | NC | not started (in progress) |
| GEN-15 | ai/utils/sanitize-unicode.ts:21-25 | Unpaired UTF-16 surrogates stripped from outbound text | sse::events_survive_any_fragmentation_including_split_utf8 (input side) | Opus · E/provider/http.rs | n/a | Lossy input produces same outbound text | all adapters | NC | measured (input split UTF-8 only); outbound not started |
| GEN-16 | ai/utils/json-parse.ts:104-124 | Streaming tool-call JSON repaired/partially parsed per delta; final parse at block end; incomplete args never executed | planned: prov::incomplete_tool_arguments_are_not_executed | Opus · E/provider (in progress) | n/a | Same final arguments; invalid → not executed | all adapters | NC | not started (in progress) |
| GEN-17 | ai/types.ts:429-450; models.ts:1198-1218 | Usage fields input/output/cacheRead/cacheWrite/cacheWrite1h/reasoning/total; tiered cost; 1h cache write ×2 | mdl::cost_uses_tiers_and_doubles_long_cache_writes; st::usage_accumulates_per_bucket | Opus · E/provider/model.rs, E/store | `pi.usage` doc → `usage` rows | Same usage/cost per response | all models | NC | measured: native unit pass; differential not started |
| GEN-18 | D/harness/generation.ts `prepare` | Request pins model/thinking at prepare; a change applies from the next request | rt::thinking_and_model_selection_are_validated_and_persisted | Opus · E/runtime | `pi.agent` model/thinkingLevel → sessions.model/thinking | Mid-run change does not alter in-flight request | scripted | NC | measured: native unit pass; differential not started |
| GEN-19 | D/harness/generation.ts `answer` + `onYield` hooks | `onYield` hooks may continue a run (built-ins: none) | n/a | Opus · E/extensions (planned) | n/a | Built-ins register none; covered by EXT-NATIVE | n/a | NP (no built-in uses it; A-ext §5.5) | not started |
| GEN-20 | D/harness/types.ts:129-133 | Tool result `control` {terminate, addTools, handoff}: not used by built-ins | n/a | n/a | n/a | n/a | n/a | NP (A-tools §1 "not used by built-ins") | not started |

## 2. Coding tools and execution environment (P02)

Tool order the model sees on the durable path: `read, write, edit, bash, subagent`
(A-tools §1.0). Native module `E/tools` is in progress; in this tree `builtin_tools()` is empty.

| ID | Source | Baseline behavior | Fixture / test | Owner · module | Migration mapping | Differential assertion | Coverage | Disp. | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| TOOL-ORDER | D/tools/index.ts:19-22; ce/session-worker.ts:783-784 | Tools offered in order read, write, edit, bash, subagent | planned: tools::builtin_order | Opus · E/tools | n/a | Tool declarations (name/description/schema/order) byte-equal in request | all adapters | NC | not started (in progress) |
| TOOL-READ | D/tools/read.ts | `read{path, offset?, limit?}`; head 2000 lines/50 KiB; offset diagnostics; BOM skip; changed-while-read retry; `Offset N is beyond end of file` | planned: tools::read_* golden set | Opus · E/tools/read (in progress) | result entries import verbatim | Result content + `<harness>` block identical for fixture files | Linux, macOS | NC | not started (in progress) |
| TOOL-READ-IMG | D/tools/read.ts:115-128; tools/image.ts:16-53 | Image sniff (JPEG/PNG non-APNG/GIF/WEBP/BMP) → error `unsupported_image`, no content | planned: tools::read_rejects_images | Opus · E/tools/read | n/a | Same diagnostic text | all | NC | not started (in progress) |
| TOOL-READ-PATH | D/tools/path-utils.ts:16-30 | Path variants retried when missing: U+202F AM/PM, NFD, curly quote | planned: tools::read_path_variants | Opus · E/tools | n/a | Same resolved file | macOS screenshots names | NC | not started (in progress) |
| TOOL-WRITE | D/tools/write.ts | `write{path, content}`; mkdir -p; writes as-is; `Successfully wrote to <path>` | planned: tools::write_* | Opus · E/tools/write (in progress) | n/a | Same file bytes and result text | Linux, macOS | NC | not started (in progress) |
| TOOL-EDIT | D/tools/edit.ts, edit-diff.ts | `edit{path, edits[]}`; argument repair; BOM/CRLF preserve; exact then fuzzy (NFKC, quotes, dashes, spaces) matching; uniqueness/overlap/no-change errors; `details{diff, patch, firstChangedLine}` | planned: tools::edit_* golden set (Unicode, CRLF, overlap, fuzzy) | Opus · E/tools/edit (in progress) | `details.patch` kept for Changes | Same file bytes, error texts, diff/patch | Linux, macOS | NC | not started (in progress) |
| TOOL-BASH | D/tools/bash.ts; env/node.ts:213-830 | `bash{command, timeout?}`; shell = settings shellPath > /bin/bash > which bash > sh; `-c`; detached process group; stdout+stderr interleaved; tail retention; exit ≠0 → `Command exited with code N`; timeout/abort texts; SIGKILL group | planned: tools::bash_* (timeout, group kill, exit code) | Opus · E/tools/bash (in progress) | n/a | Same result text, exit handling, no orphan processes | Linux, macOS (Windows Git Bash path NP for support) | NC | not started (in progress) |
| TOOL-BASH-SPILL | env/node.ts:776-790,1194-1218; bash.ts:79-82 | Output >50 KiB/2000 lines spilled to `$TMPDIR/tmp-*/pi-output-<uuid>.log`, `[info] Full output: <path>`; never deleted | planned: tools::bash_full_output_retained | Opus · E/tools/bash + E/store `tool_outputs` | spill file paths kept as text | Native retains output in SQLite (I-04); diagnostic wording difference recorded | all | NC | not started (in progress) |
| TOOL-BASH-ENV | env/node.ts:265-276 | Child env = worker env (no PI_* vars injected on durable path) | planned: tools::bash_env_inherits | Opus · E/tools/bash | n/a | Same env keys (minus engine-internal) | all | NC | not started (in progress) |
| TOOL-SUBAGENT | ca/experimental/durable/subagent.ts:24-55 | `subagent{task}`; replay-safe; one child per tool task; child without subagent; answer = concatenated text | st::a_child_task_is_created_once_per_parent_call_and_delivered_once | Opus · E/runtime/subagents.rs | owner/task mapping → subagent_tasks | Same child count, result text, child context | scripted | NC | measured: store unit pass; runtime test not started |
| TOOL-VALIDATE | D/harness/tool.ts:68-70,202-208 | `prepareArguments` repair then TypeBox validation with coercion; failure → `invalid_arguments` with validator text | planned: tools::invalid_arguments_text | Opus · E/tools::run (in progress) | n/a | Same coercions and error texts (validator text may differ: record) | all | NC | not started (in progress) |
| TOOL-LIMITS | D/harness/tool.ts:230-234,486-548; truncate.ts | Output limits 51200 B / 2000 lines, head (bash tail); `<harness>` diagnostics block; truncation warning text | planned: tools::bounding_and_diagnostics | Opus · E/tools | n/a | Byte-identical bounded content and diagnostics | all | NC | not started (in progress) |
| TOOL-ERROR | D/harness/tool.ts:310-320 | Throw from execute → `isError`, retained output, `tool_error` diagnostic, task failed | planned: tools::execute_error | Opus · E/tools | n/a | Same result | all | NC | not started (in progress) |
| TOOL-ABORT | D/harness/tool.ts:156-160 | Abort → partial output + `[error] Tool <name> was aborted` | planned: rt::stop_during_tool_aborts_with_partial_output | Opus · E/tools::aborted, E/runtime | n/a | Same result text | all | NC | unverified (helper exists, no test) |
| TOOL-INTERRUPT | D/harness/tool.ts:114-154 | Unsafe tool interrupted by crash → `[error] Tool <name> was interrupted and may have partially run`, never rerun | planned: rt::interrupted_unsafe_tool_is_not_rerun | Opus · E/runtime/run.rs, E/tools::interrupted | `pi.tool` execute-phase task → tool_calls(state=interrupted) | Same result; effect count 1 | all | NC | unverified (helper exists, no test) |
| TOOL-MUTEX | D/tools/file-mutation-queue.ts:33-55 | write/edit serialized per (env, canonical path) in-process | planned: tools::same_path_mutations_serialize | Opus · E/tools::MutationLocks | n/a | No interleaved writes; native also serializes across sessions | all | NC | unverified (type exists, no test) |
| TOOL-PATH | D/tools/path-utils.ts; env/node.ts:72-86 | Unicode spaces→space, leading `@` stripped, `~` expanded, `file://` decoded, relative to conversation cwd | planned: tools::path_normalization | Opus · E/tools | n/a | Same resolved path | all | NC | not started (in progress) |
| TOOL-CWD | ce/durable/harness-setup.ts:60-82 | One execution env per conversation cwd (`pi.agent.cwd`); missing cwd → bash error text | planned: tools::missing_cwd | Opus · E/tools | session cwd from meta.json | Same | all | NC | not started (in progress) |
| TOOL-GREP | ca/core/tools/grep.ts | Classic-only tool; never registered on the durable server path | n/a | n/a | n/a | n/a | n/a | NP (A-tools §1.0: registry is `CodingTools` + `pi-prompt` + `subagent` only) | not started |
| TOOL-FIND | ca/core/tools/find.ts | Classic-only | n/a | n/a | n/a | n/a | n/a | NP (A-tools §1.0) | not started |
| TOOL-LS | ca/core/tools/ls.ts | Classic-only | n/a | n/a | n/a | n/a | n/a | NP (A-tools §1.0) | not started |
| TOOL-POWERSHELL | ca/core/tools/powershell.ts | Classic-only | n/a | n/a | n/a | n/a | n/a | NP (A-tools §1.0) | not started |
| TOOL-READ-IMG-CLASSIC | ca/core/tools/read.ts; utils/image-resize-core.ts | Classic read returns resized images | n/a | n/a | n/a | n/a | n/a | NP (durable read rejects images, A-tools §1.1) | not started |

## 3. Wire adapters (P03)

Native adapter registry `E/provider/api::all()` is empty in this tree; adapters are being
implemented concurrently (owner module `E/provider/api/<kind>`). Shared plumbing exists:
`E/provider/{http,sse,errors,model}.rs`. Golden fixtures run against the shared mock server
(`E/provider/mock.rs`) and compare the complete request (URL, headers minus secrets, body) and
the normalized event stream with captures from the reference adapter fed the same SSE bytes.

| ID | Source | Baseline behavior | Fixture / test | Owner · module | Migration mapping | Differential assertion | Coverage | Disp. | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| ADP-SSE | ai/api/anthropic-messages.ts:471-569; codex `parseSSE` | SSE framing tolerant of any fragmentation incl. split UTF-8 | sse::events_survive_any_fragmentation_including_split_utf8; sse::byte_at_a_time_matches | Opus · E/provider/sse.rs | n/a | Same event sequence for fragmented captures | all SSE adapters | NE | measured: native unit pass; differential not started |
| ADP-HTTP-RETRY | ai/utils/provider-retry.ts:1-125 | Request-level retry before headers: `x-should-retry`, 408/409/429/≥500/network; `retry-after(-ms)` capped by maxRetryDelayMs (60 s) else min(0.5·2^i, 8) s jitter; default maxRetries 0 | planned: http::retry_policy | Opus · E/provider/http.rs | n/a | Same attempt count/delays on mock 429/503 | anthropic, completions, responses, azure, google | NC | not started |
| ADP-ERRFMT | ai/utils/error-body.ts | Error text `<prefix> (<status>): <body>` truncated 4000; classified by retry/overflow regexes | err::classification_matches_the_reference_patterns | Opus · E/provider/http.rs, errors.rs | n/a | Same error strings for mock error bodies | all | NC | measured (classification only); formatting not started |
| ADP-ANTHROPIC | ai/api/anthropic-messages.ts | `POST {base}/v1/messages` streaming; x-api-key / Bearer / cf-aig auth; content conversion; tool id normalization; strict tools; stop mapping; `Anthropic stream ended before message_stop` | planned: prov::anthropic_messages_golden | Opus · E/provider/api/anthropic_messages (in progress) | n/a | Golden request + event stream | anthropic (API key), fireworks, kimi-coding, minimax(-cn), opencode(-go), openrouter, vercel, cloudflare-gw, github-copilot | NC | not started (in progress) |
| ADP-ANTHROPIC-THINK | anthropic-messages.ts:1233-1264,908-976 | Adaptive/managed effort (`output_config.effort`, per-turn system effort), forced adaptive, budget thinking (1024/2048/8192/16384, ≥1024 answer tokens), disabled unless map.off null | planned: prov::anthropic_thinking_matrix | Opus · same | n/a | Body thinking fields per level/model | anthropic-messages models | NC | not started (in progress) |
| ADP-ANTHROPIC-CACHE | anthropic-messages.ts:79-93,1483-1509 | `cache_control` ephemeral (+ttl 1h for long) on system, last tool, last block of final user/system message; `PI_CACHE_RETENTION` | planned: prov::anthropic_cache_control | Opus · same | n/a | Same cache_control placement | anthropic-messages | NC | not started (in progress) |
| ADP-ANTHROPIC-BETAS | anthropic-messages.ts:1080-1122 | Computed `anthropic-beta` list (fine-grained tool streaming, interleaved thinking, fallback, mid-conversation config, thinking binding, inline tools) unless header override | planned: prov::anthropic_betas | Opus · same | n/a | Same header | anthropic-messages | NC | not started (in progress) |
| ADP-ANTHROPIC-AFFINITY | anthropic-messages.ts:1039-1042 | `x-session-id`/`x-session-affinity` when compat.sendSessionAffinityHeaders | planned: prov::anthropic_affinity | Opus · same | pi.provider.sessionId import | Same header | openrouter, fireworks | NC | not started (in progress) |
| ADP-ANTHROPIC-FEDERATION | anthropic-messages.ts:349-370,1052-1066 | Workload identity federation from `ANTHROPIC_FEDERATION_RULE_ID`/`ORGANIZATION_ID`/`IDENTITY_TOKEN_FILE` (+service account/workspace) | planned: prov::anthropic_federation_token_exchange | Opus · same + E/auth | n/a | Same token exchange request (mock) | anthropic | NC | not started (in progress) |
| ADP-ANTHROPIC-OAUTH | anthropic-messages.ts `isOAuthToken` :978-980, OAuth branch of `createClient` :1013-1034, `claudeCodeVersion` :96, `claudeCodeTools`/`toClaudeCodeName`/`fromClaudeCodeName` :98-132, OAuth betas :1106, OAuth `system` branch :1167-1182 | Claude subscription request behavior (location only) | reserved | **Codex** · (absent adapter; E/auth returns Unsupported) | preserve imported references, do not activate | reserved | anthropic subscription | RC | not started |
| ADP-OAI-COMPLETIONS | ai/api/openai-completions.ts | `POST {base}/chat/completions` SSE; Bearer; usage in stream; tool calls by index; blocks closed at stream end; finish_reason mapping; `Stream ended without finish_reason` | planned: prov::openai_completions_golden | Opus · E/provider/api/openai_completions (in progress) | n/a | Golden request + events | 20+ providers (see §4) | NC | not started (in progress) |
| ADP-OAI-COMPLETIONS-THINK | openai-completions.ts:880-1075 | thinkingFormat `openai`, `openrouter`, `deepseek`, `zai`, `qwen`, `qwen-chat-template`, `chat-template` ($var kwargs), `baseten`, `together`, `string-thinking`, `ant-ling`; optional budget field | planned: prov::completions_thinking_formats (11 cases) | Opus · same | n/a | Same body per format/level | per provider compat | NC | not started (in progress) |
| ADP-OAI-COMPLETIONS-COMPAT | openai-completions.ts:1593-1734 | `detectCompat` host rules (store, developer role, max_tokens field, reasoning_effort support, strict off) + explicit `model.compat` per-key override | planned: prov::completions_compat_detection | Opus · same | models.json compat import | Same compat result per baseUrl | all completions providers | NC | not started (in progress) |
| ADP-OAI-COMPLETIONS-CACHE | openai-completions.ts:1077-1191, openai-prompt-cache.ts | `prompt_cache_key`=sessionId (≤64) for api.openai.com/long; `prompt_cache_retention:"24h"`; Anthropic-style cache_control for openrouter `anthropic/*` | planned: prov::completions_cache | Opus · same | n/a | Same fields | openai-compatible, openrouter | NC | not started (in progress) |
| ADP-OAI-RESPONSES | ai/api/openai-responses.ts, openai-responses-shared.ts | `POST {base}/responses` typed SSE; reasoning items with encrypted content; text signature V1; `call_id`/`item_id` pair tool ids; tool-search/additional_tools; service tier cost multipliers | planned: prov::openai_responses_golden | Opus · E/provider/api/openai_responses (in progress) | stored reasoning items replayed | Golden request + events | openai, github-copilot, meta, xai, opencode(-go), cloudflare-gw | NC | not started (in progress) |
| ADP-OAI-RESPONSES-CHATGPT | openai-responses.ts:40-47,329-346 | "Sign in with ChatGPT" (provider `openai`, base api.openai.com/v1, non-`sk-` key) omits retention/options/max_output_tokens/temperature | planned: prov::responses_chatgpt_signin_omissions | Opus · same | n/a | Same omitted fields | openai OAuth | NC | not started (in progress) |
| ADP-AZURE | ai/api/azure-openai-responses.ts | `api-key` header; base URL from opt > `AZURE_OPENAI_BASE_URL` > resource name; deployment map; api-version; strict default true | planned: prov::azure_responses_golden | Opus · E/provider/api/azure_openai_responses (in progress) | n/a | Golden request | azure-openai-responses | NC | not started (in progress) |
| ADP-CODEX-SSE | ai/api/openai-codex-responses.ts:237-498,799-859 | `{base}/codex/responses`; Bearer + `chatgpt-account-id` from JWT; `originator: pi`; zstd body; SSE retry loop (429/5xx, retry-after, terminal quota text); usage-limit message | planned: prov::codex_sse_golden | Opus · E/provider/api/openai_codex_responses (in progress) | n/a | Golden request + events + retry | openai-codex | NC | not started (in progress) |
| ADP-CODEX-WS | openai-codex-responses.ts:649-654,1153-1477,978-986 | WebSocket transport (`auto`/`websocket`/`websocket-cached`), pooled per (session, account), `previous_response_id` delta, retry on specific errors, permanent SSE fallback | planned: prov::codex_websocket_fallback | Opus · same | n/a | Same frames and fallback | openai-codex | NC | not started (in progress) |
| ADP-GOOGLE | ai/api/google-generative-ai.ts, google-shared.ts | Gemini `streamGenerateContent?alt=sse`, `x-goog-api-key`; thinking levels/budgets per model family; thoughtSignature replay; synthesized call ids | planned: prov::google_golden | Opus · E/provider/api/google_generative_ai (in progress) | n/a | Golden request + events | google, opencode | NC | not started (in progress) |
| ADP-VERTEX | ai/api/google-vertex.ts | Vertex (`vertexai:true`), API key or ADC with project/location; `{location}` base template | planned: prov::vertex_golden | Opus · E/provider/api/google_vertex (in progress) | n/a | Golden request (ADC token mocked) | google-vertex | NC | not started (in progress) |
| ADP-MISTRAL | ai/api/mistral-conversations.ts | `POST {base}/v1/chat/completions` raw fetch; whole-request 60 s timeout; 9-char tool ids; thinking chunks; `x-affinity`; no retry | planned: prov::mistral_golden | Opus · E/provider/api/mistral_conversations (in progress) | n/a | Golden request + events | mistral | NC | not started (in progress) |
| ADP-BEDROCK | ai/api/bedrock-converse-stream.ts | ConverseStream (AWS event-stream), SigV4/profile/keys/bearer/skip-auth, region resolution, cache points, Claude adaptive/budget thinking, proxy | planned: prov::bedrock_golden | Opus · E/provider/api/bedrock_converse_stream (in progress) | n/a | Golden request + events | amazon-bedrock | NC | not started (in progress) |
| ADP-PI-MESSAGES | ai/api/pi-messages.ts | Radius gateway `POST {base}/messages`, Bearer, SSE of serialized pi events | planned: prov::pi_messages_golden | Opus · E/provider/api/pi_messages (in progress) | n/a | Golden request + events | radius | NC | not started (in progress) |
| ADP-CF-WRAP | ai/providers/cloudflare-stream.ts; api/cloudflare.ts | URL template substitution `{CLOUDFLARE_ACCOUNT_ID}`/`{CLOUDFLARE_GATEWAY_ID}`, `cf-aig-authorization` | planned: prov::cloudflare_templates | Opus · E/provider (in progress) | n/a | Same URL/headers | cloudflare-ai-gateway, cloudflare-workers-ai | NC | not started (in progress) |
| ADP-CF-BINDING | ai/api/cloudflare-ai-binding.ts | Workers `env.AI` binding (Worker-only) | n/a | n/a | n/a | n/a | n/a | NP (A-prov §1.11: Worker-only, not reachable on desktop) | not started |
| ADP-FAUX | ai/providers/faux.ts | Scripted test provider `faux/faux-1`, deferred handles | E/provider/scripted.rs (native scripted provider) | Opus · E/provider/scripted.rs | n/a | Native scripted provider replaces faux for tests; faux is not user-visible | tests only | NE | measured: used by rt:: tests |
| ADP-OR-IMAGES | ai/api/openrouter-images.ts | Image-generation API kind | planned: audit reachability | Opus · E/provider | n/a | Reachability on durable path to be confirmed | openrouter images | PA | not started |
| ADP-CLASSIFIERS | ai/api/system-one-shared.ts (typesafe, cloudflare system-one, llama-cpp classify) | Non-chat classifier APIs (used by classic codemode) | planned: audit reachability | Opus · E/provider | n/a | Reachability on durable path to be confirmed | typesafe, cloudflare, openrouter, vercel, opencode | PA | not started |
| ADP-TIMEOUT | ai SDK timeouts; ca/core/sdk.ts:316-330; http-dispatcher.ts:81-113 | `timeoutMs` = retry.provider.timeoutMs ?? httpIdleTimeoutMs (300 s; 0 → max); undici headers/body timeout; HTTP/1.1 only | planned: http::idle_timeout | Opus · E/provider/http.rs | settings import | Same timeout behavior on stalled mock | all | NC | not started |
| ADP-PROXY | ai/utils/node-http-proxy.ts:150-310; http-dispatcher.ts:45-50 | `HTTP(S)_PROXY`/`ALL_PROXY`/`NO_PROXY` (env + settings httpProxy); SOCKS/PAC rejected | planned: http::proxy_env | Opus · E/provider/http.rs | settings httpProxy import | Requests route via proxy fixture | all | NC | not started |
| ADP-ABORT | ai/utils/abort*.ts | Abort ends stream with `aborted`, partial kept | rt::stop_settles_aborted_and_retires_the_instruction_from_the_next_context | Opus · E/runtime + adapters | n/a | Same aborted message shape | scripted (adapters pending) | NC | measured (scripted provider only); adapters not started |
| ADP-UA | ai/utils/pi-user-agent.ts:17-19 | `User-Agent: pi (<platform> <release>; <arch>)` default header | planned: prov::user_agent | Opus · E/provider/http.rs | n/a | Header value differs by design? decide | all | PA | not started |
| ADP-ATTRIBUTION | ca/core/provider-attribution.ts; sdk.ts:316-330 | When install telemetry enabled: OpenRouter referer/title/categories, NVIDIA billing origin, Cloudflare UA, OpenCode `x-opencode-session`/`x-opencode-client: pi` | planned: prov::attribution_headers | Opus · E/provider | `enableInstallTelemetry` setting | Same headers (identity strings need owner decision) | openrouter, nvidia, cloudflare, opencode | PA | not started |

## 4. Provider ids (P03)

One row per provider id in A-prov §2 (42 built-in providers + `faux` = 43 ids). Each built-in
provider's pinned model data is embedded and hash-checked natively
(`mdl::embedded_files_match_the_pinned_manifest`, measured); request behavior depends on the
adapter rows above. Migration for every row: `auth.json` entry for that id → explicit import
(ADR 0007); `models.json` provider/model overrides for that id → explicit config import.
Differential for every row: golden request/stream for a text + tool turn against a mock server,
plus model list equality with the reference catalog (incl. `models.json` overrides).
Authorized live qualification is separate and owner-run.

| ID | Source | Baseline: API kind(s) (catalog count) · auth path(s) | Fixture / test | Owner · module | Migration | Differential | Coverage | Disp. | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| PRV-amazon-bedrock | ai/providers/amazon-bedrock.ts; data/amazon-bedrock.json | bedrock-converse-stream (185) · bearer `AWS_BEARER_TOKEN_BEDROCK`; AWS profile/keys/ECS/IRSA ambient | planned: prov::bedrock_golden | Opus · E/provider/api + E/auth (in progress) | import + ambient (never copied) | as above | Linux, macOS | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-ant-ling | data/ant-ling.json | openai-completions (3) · `ANT_LING_API_KEY` | planned: prov::completions_provider_matrix | Opus · same | import | as above | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-anthropic | ai/providers/anthropic.ts; data/anthropic.json | anthropic-messages (16) · `ANTHROPIC_API_KEY`, `ANTHROPIC_OAUTH_TOKEN` (read as API-key source), `ANTHROPIC_AUTH_TOKEN` → Bearer; federation env; OAuth subscription = reserved (rows CLS-*) | planned: prov::anthropic_messages_golden (API key) | Opus (API key) · E/provider/api/anthropic_messages; Codex (subscription) | API-key entries import; subscription entries preserved, not activated | as above (API key only) | API key paths | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-azure-openai-responses | data/azure-openai-responses.json | azure-openai-responses (44) · `AZURE_OPENAI_API_KEY` + `AZURE_OPENAI_BASE_URL`/`RESOURCE_NAME`/`API_VERSION`/`DEPLOYMENT_NAME_MAP` | planned: prov::azure_responses_golden | Opus · same | import | as above | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-baseten | data/baseten.json | openai-completions (22) · `BASETEN_API_KEY` | planned: prov::completions_provider_matrix | Opus · same | import | as above | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-cerebras | data/cerebras.json | openai-completions (2) · `CEREBRAS_API_KEY`; bodyless 400/413 = overflow | planned: prov::completions_provider_matrix | Opus · same | import | as above + overflow | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-cloudflare-ai-gateway | ai/providers/cloudflare-auth.ts:74-103; data/cloudflare-ai-gateway.json | anthropic-messages (12), openai-completions (18), openai-responses (24) · `CLOUDFLARE_API_KEY` as `cf-aig-authorization` + `CLOUDFLARE_ACCOUNT_ID`/`GATEWAY_ID` | planned: prov::cloudflare_templates | Opus · same | import | as above | 3 API kinds | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-cloudflare-workers-ai | data/cloudflare-workers-ai.json | openai-completions (18), classifier (3) · `CLOUDFLARE_API_KEY` + `CLOUDFLARE_ACCOUNT_ID` | planned: prov::cloudflare_templates | Opus · same | import | as above | chat only (classifier PA) | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-deepseek | data/deepseek.json | openai-completions (2) · `DEEPSEEK_API_KEY`; reasoning_content replay | planned: prov::completions_provider_matrix | Opus · same | import | as above | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-fireworks | data/fireworks.json | anthropic-messages (14), openai-completions (8) · `FIREWORKS_API_KEY` | planned: prov::anthropic_messages_golden, completions matrix | Opus · same | import | as above | 2 API kinds | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-github-copilot | ai/api/github-copilot-headers.ts; data/github-copilot.json | anthropic-messages (10), openai-completions (6), openai-responses (18) · `COPILOT_GITHUB_TOKEN`; OAuth device flow (OA-COPILOT); static + dynamic Copilot headers; `filterModels` by credential | planned: prov::copilot_headers_and_filter | Opus · same + E/auth | import oauth entry (with enterpriseUrl, availableModelIds) | as above | 3 API kinds | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-google | data/google.json | google-generative-ai (22) · `GEMINI_API_KEY` | planned: prov::google_golden | Opus · same | import | as above | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-google-vertex | ai/providers/google-vertex.ts:13-90; data/google-vertex.json | google-vertex (14) · `GOOGLE_CLOUD_API_KEY` or ADC (`GOOGLE_APPLICATION_CREDENTIALS`/gcloud default) + project + location | planned: prov::vertex_golden | Opus · same | ambient ADC never copied | as above | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-groq | data/groq.json | openai-completions (7) · `GROQ_API_KEY` | planned: prov::completions_provider_matrix | Opus · same | import | as above | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-huggingface | data/huggingface.json | openai-completions (76) · `HF_TOKEN` | planned: prov::completions_provider_matrix | Opus · same | import | as above | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-kimi-coding | data/kimi-coding.json | anthropic-messages (4) · `KIMI_API_KEY`; OAuth device flow (OA-KIMI, header auth) | planned: prov::anthropic_messages_golden | Opus · same + E/auth | import | as above | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-meta | data/meta.json | openai-responses (5) · `META_API_KEY`; OAuth device flow + API key mint (OA-META) | planned: prov::openai_responses_golden | Opus · same + E/auth | import | as above | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-minimax | data/minimax.json | anthropic-messages (3) · `MINIMAX_API_KEY` | planned: prov::anthropic_messages_golden | Opus · same | import | as above | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-minimax-cn | data/minimax-cn.json | anthropic-messages (3) · `MINIMAX_CN_API_KEY` | planned: prov::anthropic_messages_golden | Opus · same | import | as above | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-mistral | data/mistral.json | mistral-conversations (32) · `MISTRAL_API_KEY` | planned: prov::mistral_golden | Opus · same | import | as above | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-moonshotai | data/moonshotai.json | openai-completions (4) · `MOONSHOT_API_KEY`; Kimi mid-conversation tool additions | planned: prov::completions_provider_matrix | Opus · same | import | as above | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-moonshotai-cn | data/moonshotai-cn.json | openai-completions (4) · `MOONSHOT_API_KEY` | planned: prov::completions_provider_matrix | Opus · same | import | as above | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-nvidia | data/nvidia.json | openai-completions (21) · `NVIDIA_API_KEY`; model header `NVCF-POLL-SECONDS: 3600` | planned: prov::completions_provider_matrix | Opus · same | import | as above | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-openai | data/openai.json | openai-responses (44) · `OPENAI_API_KEY`; OAuth "Sign in with ChatGPT" (OA-OPENAI-CHATGPT) | planned: prov::openai_responses_golden | Opus · same + E/auth | import | as above | API key + OAuth | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-openai-codex | data/openai-codex.json | openai-codex-responses (9) · OAuth only (OA-CODEX); ChatGPT subscription onboarding | planned: prov::codex_sse_golden | Opus · E/provider/api/openai_codex_responses + E/auth | import oauth entry (accountId) | as above | SSE + WebSocket | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-opencode | ai/providers/opencode-headers.ts; data/opencode.json | anthropic-messages (17), google-generative-ai (7), openai-completions (26), openai-responses (30), typesafe (2) · `OPENCODE_API_KEY`; `x-opencode-session` | planned: per-kind goldens | Opus · same | import | as above | 4 chat kinds | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-opencode-go | data/opencode-go.json | anthropic-messages (4), openai-completions (19), openai-responses (6) · `OPENCODE_API_KEY` | planned: per-kind goldens | Opus · same | import | as above | 3 kinds | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-openrouter | data/openrouter.json | anthropic-messages (15), openai-completions (385), images (59), typesafe (13) · `OPENROUTER_API_KEY`; OAuth PKCE → API key (OA-OPENROUTER); routing `provider`; affinity | planned: prov::openrouter_matrix | Opus · same + E/auth | import | as above | chat kinds (images PA) | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-qwen-token-plan | data/qwen-token-plan.json | openai-completions (20) · `QWEN_TOKEN_PLAN_API_KEY` | planned: prov::completions_provider_matrix | Opus · same | import | as above | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-qwen-token-plan-individual | data/qwen-token-plan-individual.json | openai-completions (9) · `QWEN_TOKEN_PLAN_API_KEY` | planned: prov::completions_provider_matrix | Opus · same | import | as above | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-qwen-token-plan-cn | data/qwen-token-plan-cn.json | openai-completions (20) · `QWEN_TOKEN_PLAN_CN_API_KEY` | planned: prov::completions_provider_matrix | Opus · same | import | as above | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-radius | ai/providers/radius-config.ts:4,87; data/radius.json | pi-messages (28 baseline) · `RADIUS_API_KEY`; OAuth (OA-RADIUS); dynamic catalog from `{gateway}/v1/config`; `PI_RADIUS_GATEWAY` | planned: prov::pi_messages_golden, radius_dynamic_catalog | Opus · same + E/auth | import | as above | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-together | data/together.json | openai-completions (17) · `TOGETHER_API_KEY` | planned: prov::completions_provider_matrix | Opus · same | import | as above | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-typesafe | data/typesafe.json | classifier only (1) · `TYPESAFE_API_KEY` | planned: audit reachability | Opus · E/provider | import | reachability | classifier | PA | not started; catalog data hash-checked |
| PRV-vercel-ai-gateway | data/vercel-ai-gateway.json | anthropic-messages (253), typesafe (4) · `AI_GATEWAY_API_KEY`; `vercelGatewayRouting` | planned: prov::anthropic_messages_golden | Opus · same | import | as above | chat | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-xai | data/xai.json | openai-responses (4) · `XAI_API_KEY`; OAuth device flow (OA-XAI); always encrypted reasoning | planned: prov::openai_responses_golden | Opus · same + E/auth | import | as above | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-xiaomi | data/xiaomi.json | openai-completions (6) · `XIAOMI_API_KEY`; length-stop overflow detection | planned: prov::completions_provider_matrix | Opus · same | import | as above | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-xiaomi-token-plan-cn | data/xiaomi-token-plan-cn.json | openai-completions (4) · `XIAOMI_TOKEN_PLAN_CN_API_KEY` | planned: prov::completions_provider_matrix | Opus · same | import | as above | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-xiaomi-token-plan-ams | data/xiaomi-token-plan-ams.json | openai-completions (4) · `XIAOMI_TOKEN_PLAN_AMS_API_KEY` | planned: prov::completions_provider_matrix | Opus · same | import | as above | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-xiaomi-token-plan-sgp | data/xiaomi-token-plan-sgp.json | openai-completions (4) · `XIAOMI_TOKEN_PLAN_SGP_API_KEY` | planned: prov::completions_provider_matrix | Opus · same | import | as above | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-zai | data/zai.json | openai-completions (7) · `ZAI_API_KEY`; thinkingFormat zai, `tool_stream` | planned: prov::completions_provider_matrix | Opus · same | import | as above | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-zai-coding-cn | data/zai-coding-cn.json | openai-completions (4) · `ZAI_CODING_CN_API_KEY` | planned: prov::completions_provider_matrix | Opus · same | import | as above | — | NC | not started (in progress); catalog data hash-checked by mdl::embedded_files_match_the_pinned_manifest |
| PRV-faux | ai/providers/faux.ts:24-30; compat.ts:162 | faux (`faux-1`, `http://localhost:0`) · none; test/demo only | E/provider/scripted.rs | Opus · E/provider/scripted.rs | n/a | not user-visible | tests | NE | measured: scripted provider used by rt:: tests |

Count note: the caller's brief anticipated 46 provider ids; A-prov §2 lists 42 built-in ids plus
`faux` (43), matching the 42 provider data files embedded natively. The three custom-provider
mechanisms (`models.json` providers, `models.json` `"oauth": "radius"`, extension
`registerProvider`) are rows PCFG-02, PCFG-03 and PCFG-04 below. If the 46 came from another
source, the difference must be reconciled by RE-01 before this section is called complete.

## 5. Provider configuration, custom providers and credential resolution (P03, P19)

| ID | Source | Baseline behavior | Fixture / test | Owner · module | Migration mapping | Differential assertion | Coverage | Disp. | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| PCFG-01 | ca/core/model-config.ts:294-333 | `models.json` JSONC (comments, BOM) with TypeBox validation; invalid → empty config + `Invalid models.json schema:` error, providers still load | planned: auth::models_json_invalid_is_reported | Opus · E/auth (in progress) | copy `~/.pi/agent/models.json` on explicit import | Same catalog and error text | all | NC | not started (in progress) |
| PCFG-02 | provider-composer.ts:542-683; model-config.ts:84-257 | Custom providers/models (upsert by id; defaults name=id, contextWindow 128000, maxTokens 16384), baseUrl/headers/compat/authHeader, `modelOverrides` last | planned: auth::models_json_layers | Opus · E/auth (in progress) | import | Same composed model list incl. compat merges | custom endpoints | NC | not started (in progress) |
| PCFG-03 | model-runtime.ts:260-275 | `"oauth": "radius"` creates an extra Radius provider at baseUrl minus `/v1` | planned: auth::models_json_radius_oauth | Opus · E/auth (in progress) | import | Same provider set | radius | NC | not started (in progress) |
| PCFG-04 | provider-composer.ts (extension `registerProvider`) | Extension-registered providers | n/a | n/a | n/a | n/a | n/a | NP (A-ext §5.4: classic extensions not loaded on durable path) | not started |
| PCFG-05 | auth/resolve.ts:46-93; provider-composer.ts:417-481 | Credential precedence: explicit key → runtime key → stored auth.json (owns provider) → models.json `apiKey` → ambient | planned: auth::precedence_matrix | Opus · E/auth (in progress) | n/a | Same chosen credential per fixture | all | NC | not started (in progress) |
| PCFG-06 | ca/core/runtime-credentials.ts; main.ts:831-834 | CLI `--api-key` runtime key | n/a | n/a | n/a | n/a | n/a | NP (Pipkin launches `pi server` without `--api-key`; App/adapters/pi/engine.rs:174-199) | not started |
| PCFG-07 | resolve-config-value.ts:80-140 | Config values: `!command` (configured shell, 10 s, cached per process), `$VAR`/`${VAR}` templates with credential env, `$$`/`$!` escapes, unresolved if any var missing | planned: auth::config_values | Opus · E/auth (in progress) | values imported verbatim | Same resolution | all | NC | not started (in progress) |
| PCFG-08 | model-runtime.ts:547-694 | Header merge: auth headers < caller headers < attribution; `authHeader:true` adds Bearer; per-model headers; `null` deletes; Copilot `auth.baseUrl` replaces model baseUrl | planned: auth::header_merge | Opus · E/auth + E/provider (in progress) | n/a | Same final headers | all | NC | not started (in progress) |
| PCFG-09 | utils/provider-env.ts:45-52 | Env lookup: credential `env` > process env | cfg::isolated_env_hides_the_process_environment | Opus · E/config.rs | n/a | Same lookup | all | NC | measured (isolation only); credential env not started |
| PCFG-10 | env-api-keys.ts:151-195 | `getEnvApiKey` returns `<authenticated>` for Vertex ADC and Bedrock AWS chains; skips `ANTHROPIC_AUTH_TOKEN` | planned: auth::ambient_availability | Opus · E/auth (in progress) | n/a | Same availability | vertex, bedrock | NC | not started (in progress) |
| PCFG-11 | ca/core/remote-catalog-provider.ts:13-140; model-runtime.ts:226-238 | Built-in providers (except radius) refresh catalogs from `https://pi.dev/api/models/providers/<id>` every 4 h (ETag, 4 s) unless `PI_OFFLINE`; cached in `models-store.json` | planned: decide | Opus · E/provider/model.rs | `models-store.json` not imported | Whether native fetches a live catalog is an owner decision (plan §11: release builds must not need it) | all | PA | not started |
| PCFG-12 | ca/core/model-resolver.ts:20-62 | Default model per provider and initial root model (`--provider/--model`, settings default*) | rt::model_errors_retry_then_settle_and_missing_models_are_reported (missing model) | Opus · E/runtime/models.rs | settings defaults import | Same initial model for a fresh session | all | NC | measured (missing-model path only); differential not started |
| PCFG-13 | models.ts:1220-1252 | Thinking levels per model (`thinkingLevelMap`, null = unsupported, xhigh/max only when mapped), clamp up then down | mdl::thinking_levels_follow_the_reference_rules | Opus · E/provider/model.rs | n/a | Same levels for every catalog model | all models | NE | measured: native unit pass; full-catalog differential not started |
| PCFG-14 | settings `enabledModels` | Model scoping patterns (classic `--models` / cycling) | planned: audit whether the server catalog filters by it | Opus · E/runtime/models.rs | settings import | — | — | PA | not started |

## 6. Credentials, sign-in and OAuth flows (P04)

### 6.1 Generic credential and sign-in lifecycle (Opus, primary work)

| ID | Source | Baseline behavior | Fixture / test | Owner · module | Migration mapping | Differential assertion | Coverage | Disp. | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| AUTH-STORE | ca/core/auth-storage.ts:24-256 | `auth.json` `{provider: api_key{key?,env?} or oauth{access,refresh,expires,...extra}}`, pretty JSON, file 0600 / dir 0700, validation keeps extra fields | planned: auth::store_round_trip_and_modes | Opus · E/auth (in progress) | explicit import copies entries verbatim (ADR 0007) | Same parsed credentials; unknown fields preserved | Linux, macOS | NC | not started (in progress) |
| AUTH-LOCK | auth-storage.ts:49-200 | Cross-process locking (proper-lockfile; sync 10×20 ms; async stale 30 s, compromised-lock detection) | planned: auth::concurrent_writers | Opus · E/auth (in progress) | n/a | No lost update under two writers | Linux, macOS | NC | not started (in progress) |
| AUTH-REFRESH | ai/auth/resolve.ts:102-191 | Refresh when `now + max(5 min, minValidity) >= expires`, inside the store lock (double-checked), 15 s bound, not cancelled by caller; failure → error, never another credential | planned: auth::refresh_serialized_and_no_fallback | Opus · E/auth (in progress) | n/a | Same refresh trigger; one refresh for N concurrent requests; failure surfaces | all OAuth providers (non-reserved) | NC | not started (in progress) |
| AUTH-PKCE | ai/auth/oauth/pkce.ts | PKCE S256, 32 random bytes base64url | planned: auth::pkce_vectors | Opus · E/auth/oauth (in progress) | n/a | RFC 7636 vectors | all PKCE flows | NC | not started (in progress) |
| AUTH-LOOPBACK | ai/auth/oauth/callback-server.ts:50-148 | Loopback callback server (host `PI_OAUTH_CALLBACK_HOST` default 127.0.0.1, port, path, state check, 404/400/409 pages, timeout) | planned: auth::loopback_state_mismatch | Opus · E/auth/oauth (in progress) | n/a | Same responses for good/bad state | Linux, macOS | NC | not started (in progress) |
| AUTH-MANUAL | callback-server.ts:155-183 | `waitForCallbackOrManualInput` races loopback vs pasted `manual_code`; aborted prompt clears challenge | planned: auth::manual_code_race | Opus · E/auth (in progress) | n/a | Whichever wins settles once | browser flows | NC | not started (in progress) |
| AUTH-DEVICE | ai/auth/oauth/device-code.ts | RFC 8628 polling: interval 5 s (min 1), slow_down +5 s, deadline expires_in | planned: auth::device_polling | Opus · E/auth/oauth (in progress) | n/a | Same poll schedule on mock | device flows | NC | not started (in progress) |
| AUTH-REDACT | svc/provider-auth-provider.ts:189-197; P/api.rs `Secret` | Provider error text never replicated; secrets never in logs/state | planned: auth::no_secret_in_debug_or_events | Opus · P/api.rs, P/view.rs, E/auth | n/a | Grep of all emitted frames/logs for fixture secrets is empty | all | NC | unverified (redacted Debug impls exist; no test) |
| AUTH-SECRETSTORE | plan §5 | Baseline uses a private file (no OS secret store) | planned: auth::private_file_fallback_documented | Opus · E/auth (in progress) | ADR 0007 | Native keeps private file until a qualified store is approved | Linux, macOS | NE | not started (in progress) |
| AUTH-PROVIDER-AUTH-EXT | ai/auth/types.ts:170-240 | `ApiKeyAuth.login` interactive API-key entry (classic `/login`) | n/a | Opus · E/auth (`Call::SetApiKey`) | n/a | Pipkin onboarding never offered API-key entry through the server (A-svc §4.4 "OAuth only") | — | PA | not started |

### 6.2 Pipkin onboarding service contract (overlay `pi.provider-auth`)

| ID | Source | Baseline behavior | Fixture / test | Owner · module | Migration mapping | Differential assertion | Coverage | Disp. | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| ONB-STATE | pkg/pi-onboarding/provider-auth.ts:1-43 | Replicated state `{credentialsKnown, credentialLookupFailed, hasExistingCredentials, existingProviders, attempt, provider, status, message, url, deviceCode, challenge}` | ovl-auth:: (reference); rpi::real_server_provider_auth_boundary | Opus · P/view.rs `AuthSnapshot`, E/auth | n/a | Desktop onboarding state machine sees the same transitions | anthropic (RC part), openai-codex | NC | not started (in progress) |
| ONB-LOOKUP | provider-auth-provider.ts (creation) | Lists saved OAuth entries for the two providers without secrets; read failure → `credentialLookupFailed` (fail closed) | planned: auth::lookup_failure_fails_closed | Opus · E/auth (in progress) | n/a | Same flags for unreadable file | — | NC | not started (in progress) |
| ONB-START | provider-auth-provider.ts:80-203 | `start(p)` cancels active attempt, new attempt UUID, background login; `auth_url`→waiting+url; `device_code`→url+code; info/progress→message; prompt→challenge; success `Connected. Choose a model to continue.`; error → generic failed | planned: auth::start_happy_path_fictitious_provider | Opus · E/auth (in progress) | n/a | Same status sequence | openai-codex (+ fictitious) | NC | not started (in progress) |
| ONB-REUSE | provider-auth-provider.ts:204-254 | `reuse(p)` requires stored OAuth, refresh with `allowNetwork:false`, checks configured + a model available | planned: auth::reuse_offline | Opus · E/auth (in progress) | imported credential | Same outcome | openai-codex | NC | not started (in progress) |
| ONB-ANSWER | provider-auth-provider.ts:277-292 | `answer(attempt, challenge, response)` ≤8192 chars; select must be an option; errors `The sign-in question has expired` / `The selected option is not available` | planned: auth::stale_attempt_and_bad_option | Opus · E/auth (in progress) | n/a | Same errors | — | NC | not started (in progress) |
| ONB-CANCEL | provider-auth-provider.ts:293-308 | `cancel(attempt)` aborts login, no-op if stale | planned: auth::cancel_stale_is_noop | Opus · E/auth (in progress) | n/a | Same | — | NC | not started (in progress) |
| ONB-REMOVE | provider-auth-provider.ts:255-276 | `remove(p)` → logout (delete auth.json entry); error `Could not remove the saved connection. Try again.`; `Wait for connection removal to finish` during removal | planned: auth::remove_deletes_only_that_entry | Opus · E/auth (in progress) | n/a | Same file effect | openai-codex | NC | not started (in progress) |
| ONB-SCOPE | A-svc §4.4 (per connection) | Attempts/state are per client connection | planned: auth::attempts_are_client_scoped | Opus · E/runtime (`publish_to`), E/auth | n/a | Second client does not see/answer first's attempt | — | NC | not started (in progress) |
| ONB-PROVIDERS | pkg/pi-onboarding/provider-auth.ts:4 | Sign-in providers are exactly `anthropic` and `openai-codex`; others → `This provider is not available for sign-in` | planned: auth::unknown_provider_refused | Opus · E/auth | n/a | Same refusal | — | NC | not started (in progress) |
| ONB-DEVICEID | ca/core/settings-manager.ts:1176-1181 | Settings `deviceId` UUID created lazily (used by `openai` ChatGPT sign-in) | planned: auth::device_id_persisted | Opus · E/config + E/auth | settings deviceId import (global only) | Same id reused | openai | NC | not started (in progress) |

### 6.3 OAuth flows (one row per flow and lifecycle step)

Non-onboarding OAuth logins (Copilot, OpenRouter, Kimi, Meta, Radius, xAI, OpenAI ChatGPT) are
reachable only through the classic `pi` CLI `/login`, not through Pipkin's onboarding service
(A-svc §4.4: `SignInProvider = "anthropic" | "openai-codex"`). Their **stored credentials** are
used and refreshed by the shipped engine (shared `auth.json`), so refresh/use rows are baseline;
whether native must also offer their interactive login is **pending audit** (owner decision).

| ID | Source | Baseline behavior | Fixture / test | Owner · module | Migration mapping | Differential assertion | Coverage | Disp. | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| OA-CODEX-BROWSER | ai/auth/oauth/openai-codex.ts:289-330 | PKCE + state, loopback `127.0.0.1:1455/auth/callback`, authorize with codex params | planned: auth::codex_browser_mock | Opus · E/auth/oauth/openai_codex (in progress) | n/a | Same authorize URL params and token exchange (mock) | openai-codex | NC | not started (in progress) |
| OA-CODEX-DEVICE | openai-codex.ts:187-357 | Device flow `deviceauth/usercode` → poll `deviceauth/token` → exchange; 15 min timeout | planned: auth::codex_device_mock | Opus · same | n/a | Same requests/poll handling | openai-codex | NC | not started (in progress) |
| OA-CODEX-METHOD | openai-codex.ts:406-428 | Login prompt select `browser` (default) / `device_code` | planned: auth::codex_method_select | Opus · same | n/a | Same challenge | openai-codex | NC | not started (in progress) |
| OA-CODEX-MANUAL | openai-codex.ts:69-97 | Manual input accepts URL, `code#state`, query string or raw code; bind failure → manual only | planned: auth::codex_manual_parse | Opus · same | n/a | Same parse results | openai-codex | NC | not started (in progress) |
| OA-CODEX-TOKEN | openai-codex.ts:145-185 | Token endpoint (authorization_code / refresh_token); credential `{access, refresh, expires, accountId}` (JWT claim required) | planned: auth::codex_token_and_account_id | Opus · same | auth.json `openai-codex` entry import | Same credential fields | openai-codex | NC | not started (in progress) |
| OA-CODEX-REFRESH | openai-codex.ts refresh + AUTH-REFRESH | Serialized refresh; stale attempt and concurrent refresh tests (plan P04) | planned: auth::codex_concurrent_refresh | Opus · same | n/a | One refresh, persisted atomically | openai-codex | NC | not started (in progress) |
| OA-CODEX-REUSE | ONB-REUSE | Saved ChatGPT subscription reuse | e2e: none; rpi::real_server_provider_auth_boundary | Opus · E/auth | imported entry | Same | openai-codex | NC | not started (in progress) |
| OA-CODEX-REMOVE | ONB-REMOVE | Remove saved ChatGPT subscription | planned: auth::remove_deletes_only_that_entry | Opus · E/auth | n/a | Same | openai-codex | NC | not started (in progress) |
| OA-CODEX-CANCEL | ONB-CANCEL | Cancel / retry login | planned: auth::cancel_then_retry | Opus · E/auth | n/a | Same | openai-codex | NC | not started (in progress) |
| OA-OPENAI-CHATGPT | ai/auth/oauth/openai-chatgpt.ts:16-298 | "Sign in with ChatGPT" for provider `openai`: dynamic client, deviceId agent host, PKCE+nonce, port 1455 (explicit error if busy), token requires id_token + scope | planned: auth::openai_chatgpt_mock | Opus · E/auth/oauth (in progress) | import oauth entry (clientId, scopes) | Same requests | openai | PA (login reachability) | not started |
| OA-OPENAI-CHATGPT-REFRESH | openai-chatgpt.ts:162-224 | Refresh with stored clientId; `expires = now + expires_in − 3 min` | planned: auth::openai_chatgpt_refresh | Opus · same | imported entry | Same | openai | NC | not started (in progress) |
| OA-COPILOT | ai/auth/oauth/github-copilot.ts:10-485 | Device flow (enterprise domain prompt), GitHub token → Copilot token, base URL from `proxy-ep`, model list + policy enable on login | planned: auth::copilot_device_mock | Opus · E/auth/oauth (in progress) | import oauth entry | Same requests | github-copilot | PA (login reachability) | not started |
| OA-COPILOT-REFRESH | github-copilot.ts:313-349,135-166 | Refresh = re-fetch Copilot token with GitHub token; 429 retry loop; `expires = expires_at − 5 min` | planned: auth::copilot_refresh | Opus · same | imported entry | Same | github-copilot | NC | not started (in progress) |
| OA-OPENROUTER | ai/auth/oauth/openrouter.ts | PKCE loopback random port/path (no state), exchange → API key, never expires | planned: auth::openrouter_mock | Opus · E/auth/oauth (in progress) | import (stored as oauth with access=key) | Same | openrouter | PA (login reachability) | not started |
| OA-OPENROUTER-USE | openrouter.ts (`refresh` identity) | Stored key used as API key | planned: auth::openrouter_stored_key | Opus · same | imported entry | Same | openrouter | NC | not started (in progress) |
| OA-KIMI | ai/auth/oauth/kimi-coding.ts | Device flow at auth.kimi.com (host env overrides) | planned: auth::kimi_device_mock | Opus · E/auth/oauth (in progress) | import | Same | kimi-coding | PA (login reachability) | not started |
| OA-KIMI-REFRESH | kimi-coding.ts | Refresh with 3 retries on 429/5xx; auth as header not apiKey | planned: auth::kimi_refresh | Opus · same | imported entry | Same | kimi-coding | NC | not started (in progress) |
| OA-META | ai/auth/oauth/meta.ts | Device flow → identity token → mint API key at `api.meta.ai/muse-code/key` | planned: auth::meta_mock | Opus · E/auth/oauth (in progress) | import | Same | meta | PA (login reachability) | not started |
| OA-META-REFRESH | meta.ts | "Refresh" re-mints key every 24 h; 401/403 → re-login | planned: auth::meta_remint | Opus · same | imported entry | Same | meta | NC | not started (in progress) |
| OA-RADIUS-BROWSER | ai/auth/oauth/radius.ts | Discovery `{gateway}/v1/oauth`, PKCE loopback `127.0.0.1:1456` | planned: auth::radius_browser_mock | Opus · E/auth/oauth (in progress) | import | Same | radius | PA (login reachability) | not started |
| OA-RADIUS-DEVICE | radius.ts | Device flow `{gateway}/v1/oauth/device` | planned: auth::radius_device_mock | Opus · same | import | Same | radius | PA (login reachability) | not started |
| OA-RADIUS-REFRESH | radius.ts | Token refresh; `expires = now + expires_in − 60 s` | planned: auth::radius_refresh | Opus · same | imported entry | Same | radius | NC | not started (in progress) |
| OA-XAI | ai/auth/oauth/xai.ts | Device flow at auth.x.ai | planned: auth::xai_device_mock | Opus · E/auth/oauth (in progress) | import | Same | xai | PA (login reachability) | not started |
| OA-XAI-REFRESH | xai.ts | Refresh keeps old refresh token if none returned | planned: auth::xai_refresh | Opus · same | imported entry | Same | xai | NC | not started (in progress) |
| OA-EXT | ca/core/provider-composer.ts:40-50,353-372 | Legacy extension OAuth adapter | n/a | n/a | n/a | n/a | n/a | NP (A-ext §5.4: classic extensions not loaded) | not started |

### 6.4 Claude subscription (reserved for Codex — location-only references)

All references are locations only (A-prov §4.1, §1.1): `ai/auth/oauth/anthropic.ts`
(`parseAuthorizationInput` :27, `postJson` :74, `exchangeAuthorizationCode` :94, `loginAnthropic`
:138, `loginAnthropicCopyCode` :191, `refreshAnthropicToken` :231, `anthropicOAuth` :269-298),
`ai/auth/oauth/load.ts:33-36`, `ai/providers/anthropic.ts` (`lazyOAuth`), the `ADP-ANTHROPIC-OAUTH`
symbols, `pkg/pi-onboarding/provider-auth.ts:4`, `App/adapters/pi/auth.rs:69-79`,
`Core/onboarding.rs` (`SignInProvider::Claude`). No endpoints, client ids or request details are
recorded here. API-key or generic-fixture tests never close these rows.

| ID | Area | Baseline behavior (summary) | Fixture / test | Owner · module | Migration mapping | Differential assertion | Coverage | Disp. | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| CLS-ONBOARD-BROWSER | onboarding | Claude Pro/Max browser sign-in via `pi.provider-auth` `start("anthropic")` | reserved | Codex · E/auth (absent adapter) | n/a | reserved | anthropic subscription | RC | not started |
| CLS-ONBOARD-COPYCODE | onboarding | Headless copy-code variant (`loginAnthropicCopyCode`) and manual code answer | reserved | Codex | n/a | reserved | — | RC | not started |
| CLS-CANCEL | onboarding | Cancel / retry a Claude sign-in attempt | reserved | Codex | n/a | reserved | — | RC | not started |
| CLS-REUSE | credential reuse | `reuse("anthropic")` offline refresh + model check | reserved | Codex | n/a | reserved | — | RC | not started |
| CLS-IMPORT | credential import | Explicit import of a stored `anthropic` OAuth entry from `~/.pi/agent/auth.json` | reserved | Codex · E/import (hook only) | preserve entry; do not activate | reserved | — | RC | not started |
| CLS-REMOVE | credential removal | `remove("anthropic")` | reserved | Codex | n/a | reserved | — | RC | not started |
| CLS-REFRESH | refresh | `refreshAnthropicToken` under the store lock; concurrent refresh | reserved | Codex | n/a | reserved | — | RC | not started |
| CLS-ENV-TOKEN | credential source | `ANTHROPIC_OAUTH_TOKEN` env read as a key source, then detected as a subscription token on the request path | reserved | Codex | n/a | reserved | — | RC | not started |
| CLS-REQUEST | request behavior | Subscription-specific request/identity/system/tool-name/beta behavior (ADP-ANTHROPIC-OAUTH symbols) | reserved | Codex · E/provider/api/anthropic_messages (extension point only) | n/a | reserved | — | RC | not started |
| CLS-MODELS | model availability | Which models are available under a subscription credential | reserved | Codex · E/auth::availability | n/a | reserved | — | RC | not started |
| CLS-CONTEXT-CACHE-USAGE | context / cache / usage | Prompt caching, usage/cost accounting and context behavior under the subscription | reserved | Codex | `pi.usage` preserved | reserved | — | RC | not started |
| CLS-MIGRATION-ACTIVATION | migration | Activating imported Claude-subscription conversations on native | reserved | Codex · E/import | stay Pi-owned until qualified | reserved | — | RC | not started |
| CLS-DESKTOP | desktop | Onboarding UI Claude option and status (`Core/onboarding.rs`) on the native backend | reserved | Codex · App/adapters/native (planned) | n/a | reserved | — | RC | not started |
| CLS-BOUNDARY | generic boundary (Opus) | Native lists `anthropic` sign-in `available:false` with reason, `AuthStart{anthropic}` → `Unsupported`, stored subscription credential makes models unavailable (no API-key substitution, no Pi retry) | planned: auth::reserved_provider_is_explicitly_unavailable | Opus · E/auth, P/view.rs | preserve, do not activate | Not a parity test; proves the boundary only | — | NC | unverified (skeleton returns Unsupported; no test) |

## 7. Service API (A-svc §4.4) and transport

Each reference service method is mapped to a native `Call` (P/api.rs). Desktop-visible behavior
is the differential target; Chord replicated-state op streams are not reproduced (ADR 0002).

| ID | Source | Baseline behavior | Fixture / test | Owner · module | Migration mapping | Differential assertion | Coverage | Disp. | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| SVC-DIR-STATE | svc/sessions.ts:21-30; svc/server.ts:39-54 | `pi.session-directory` replicated `{revision, sessions[{serverId, sessionId, createdAt, cwd}]}` | rt::sessions_are_created_once_per_key_and_listed | Opus · `Call::ListSessions`, `Event::SessionsChanged` | meta.json → sessions | Same session list after create/remove/restart | — | NC | measured: native unit pass; differential not started |
| SVC-MGMT-CREATE | svc/server.ts:97-102; ce/server.ts:76-89,417-420 | `create({id?, cwd?})`; cwd must be absolute existing dir (realpath); id regex; duplicate error; no worker start | rt::sessions_are_created_once_per_key_and_listed; st::create_is_idempotent_under_its_key | Opus · `Call::CreateSession` (idempotent request key) | session id map in import_map | Same validation errors; native adds idempotent key (I-01-related) | — | NC | measured: native unit pass; validation differential not started |
| SVC-MGMT-REMOVE | svc/server.ts:103-108 | `remove(id)`: release attachments, stop worker, `rm -rf` session dir, remove plugin profile | planned: rt::remove_session (if required) | Opus · (no native Call yet) | n/a | The desktop adapter never calls it (App/adapters/pi/mod.rs calls only create/attach); confirm no other reachable caller | — | PA | not started |
| SVC-MGMT-ATTACH | svc/server.ts:109-112; session-router.ts:160-197 | `attach(id)` mints attachmentId, opens/spawns worker, sends attachment envelope | rpi::real_server_switching_between_sessions | Opus · `Call::OpenSession` (snapshot + window) | n/a | Switching sessions yields equal visible state | — | NC | not started |
| SVC-MGMT-DETACH | svc/server.ts:113-117 | `detach()` releases attachment; detaching never cancels runs | e2e::switching_conversations_keeps_each_ones_run_and_settles_what_ended_while_away | Opus · implicit (no attachment state) | n/a | Background run continues | — | NC | not started |
| SVC-PLUGINS-PREPARE | svc/plugins.ts:4-12; ce/server.ts:453-469,573-602 | `prepareSession({sessionId, packagePaths})` builds/selects facet packages, persists selection, checks worker conflict | e2e::an_extension_in_the_engine_asks_three_questions_and_the_answers_reach_it | legacy (Pi) | plugin profile JSON preserved | see EXT-* | — | BL | not started |
| SVC-PLUGINS-RELOAD | svc/server.ts:88-94 | `reload()` rebuilds TUI artifacts | n/a | legacy (Pi) | n/a | see EXT-RELOAD | — | BL | not started |
| SVC-SESSION-PLUGINS-RELOAD | svc/worker.ts:68-117 | `pi.session-plugins.reload()` hot-reloads session facets | n/a | legacy (Pi) | n/a | see EXT-RELOAD | — | BL | not started |
| SVC-AUTH | svc/provider-auth.ts | `pi.provider-auth` state/start/reuse/answer/cancel/remove (detailed in ONB-* rows) | see ONB-* | Opus · `Call::Auth*` (E/auth in progress) | — | see ONB-* | anthropic (RC part), openai-codex | NC | not started (in progress) |
| SVC-MODELS-STATE | svc/models.ts:4-24 | `{catalog{revision, availableModels[{provider, modelId, name, reasoning}]}, configuration{model, thinkingLevel}, refresh}`; catalog includes selected model if missing | planned: rt::models_view_includes_selected_model | Opus · `Call::Models`, `ModelsView` | n/a | Same model list and configuration | all providers | NC | not started |
| SVC-MODELS-SELECT | svc/models-provider.ts:93-103 | `select({provider, modelId})`: unknown → `Unknown model: p/m`; clamps thinking; writes `pi.agent`; **also persists global default** in settings.json | rt::thinking_and_model_selection_are_validated_and_persisted | Opus · `Call::SetModel` | sessions.model | Same clamp and error; global-default write is an open decision (D-04) | — | NC | measured: native unit pass (session part); global default not started |
| SVC-MODELS-THINKING | svc/models-provider.ts:104-110 | `selectThinking(level)` validated: `Thinking level X is unavailable; choose one of: ...` | rt::thinking_and_model_selection_are_validated_and_persisted | Opus · `Call::SetThinking` | sessions.thinking | Same error text | — | NC | measured: native unit pass; differential not started |
| SVC-MODELS-CYCLE | svc/models-provider.ts:69-74 | `cycleThinking()` next supported level | n/a | — | n/a | Desktop does not call it (TUI only) | — | PA | not started |
| SVC-MODELS-LEVELS | svc/models-provider.ts:47-50 | `getThinkingLevels()` (`["off"]` with no model) | planned: rt::thinking_levels_view | Opus · `ThinkingView` | n/a | Same levels | — | NC | not started |
| SVC-MODELS-REFRESH | svc/models-provider.ts:78-92 | `refresh()` → refreshing → done or warning{errors}; re-reads models.json/auth | e2e::a_provider_configured_after_start_appears_after_a_refresh_and_can_then_be_used | Opus · `Call::RefreshModels` | n/a | Same visible outcome | — | NC | not started |
| SVC-AGENT-PROMPT | svc/agent-controller-provider.ts:37-47 | `prompt(req)` → `{accepted, operationId}` or `busy` | rt::a_prompt_is_admitted_durably_streamed_settled_and_looked_up; rt::a_busy_session_refuses_a_second_prompt_without_recording_it | Opus · `Call::Prompt` | submissions → requests | Same acceptance/busy outcomes | — | NC | measured: native unit pass; differential not started |
| SVC-AGENT-STEER | agent-controller-provider.ts:20-48 | `steer(req)` joins active run at postTools or starts a run | planned: rt::steer_joins_the_running_run | Opus · `Call::Enqueue{steer}` | inbox steer → requests(mode=steer) | Same placement in next request | — | NC | not started |
| SVC-AGENT-FOLLOWUP | agent-controller-provider.ts:49 | `followUp(req)` queues after run or starts one | rt::stop_withdraws_queued_inputs_and_follow_ups_run_after_completion | Opus · `Call::Enqueue{followUp}` | inbox followUp → requests | Same order | — | NC | measured: native unit pass; differential not started |
| SVC-AGENT-CANCELQ | agent-controller-provider.ts:50-57,102-105 | `cancelQueued(entryId)` → cancelled/already_consumed/not_found | rt::queued_inputs_can_be_withdrawn_before_they_run; st::queue_entries_are_ordered_withdrawable_and_bounded | Opus · `Call::CancelQueued` | n/a | Same outcomes | — | NC | measured: native unit pass; differential not started |
| SVC-AGENT-ABORT | agent-controller-provider.ts:58 | `abort()` withdraws queued inputs, aborts run + compaction | rt::stop_settles_aborted_and_retires_the_instruction_from_the_next_context | Opus · `Call::Stop` | n/a | See STOP-* | — | NC | measured: native unit pass; differential not started |
| SVC-AGENT-COMPACT | agent-controller-provider.ts:76-83 | `compact({customInstructions})` → operationId = compaction task id | planned: rt::manual_compaction | Opus · `Call::Compact` (returns `Unsupported` today) + E/context (in progress) | n/a | See CTX-MANUAL | — | NC | not started (in progress) |
| SVC-AGENT-LOOKUP | agent-controller-provider.ts:59-75 | `lookup(requestId)` → found/status queued, placed, done or unanswered + reason/detail; never admits | rt::a_prompt_is_admitted_durably_streamed_settled_and_looked_up | Opus · `Call::Lookup` | request ids preserved by importer | Same status for every journal state | — | NC | measured: native unit pass; differential not started |
| SVC-AGENT-WAIT | agent-controller-provider.ts:84-98 | `waitForPrompt(id)` → done text or unanswered reason; `Unknown prompt: <id>` | n/a (desktop does not call it; facets and `pi client` do) | Opus · E/extensions (session control) | n/a | Needed for facet session control | — | PA | not started |
| SVC-TRANSCRIPT-STATE | svc/transcript.ts; D/harness/view.ts:26-32 | `ConversationView {conversation, entries (active context), docs{pi.live, pi.inbox, pi.agent, pi.usage, pi.provider}}` streamed as Chord ops | rt:: (snapshot + EntryAppended/Delta) | Opus · `Call::OpenSession`, `SessionChange::*` | docs → typed tables | Desktop-visible transcript equal at every committed revision | — | NC | measured (native events exist in rt:: tests); differential not started |
| SVC-HISTORY-PAGE | svc/history-provider.ts:8-23 | `page({before, limit})` newest first, limit 1..200, `before` validation text, `before===1` empty; covers past resets/compactions | st::context_starts_at_the_newest_head_marker_and_pages_reach_everything | Opus · `Call::HistoryPage` (oldest first, limit ≤500) | entry ids mapped by importer | Same reachable entries; ordering/limit differences are adapter-normalized | — | NC | measured: native unit pass; differential not started |
| SVC-UI-STATE | svc/ui-requests.ts:35-41 | `{requests, status, notices}` | e2e::the_engines_extension_question_service_is_there_and_refuses_an_answer_to_nothing | Opus · `UiView` (no producer yet) | n/a | See UIQ-* | — | BL | not started |
| SVC-UI-RESPOND | svc/ui-requests-provider.ts:156-171 | `respond(id, value)` validation texts | e2e::an_extension_in_the_engine_asks_three_questions_and_the_answers_reach_it | Opus · `Call::AnswerQuestion` (always NotFound today) | n/a | Same texts | — | BL | not started |
| SVC-UI-CANCEL | ui-requests-provider.ts:172-177 | `cancel(id)` → `{cancelled}` | e2e::declining_ends_the_question_and_a_lost_connection_does_not_leave_it_answerable | Opus · `Call::CancelQuestion` | n/a | Same | — | BL | not started |
| SVC-SUB-ACTIVITY | svc/subagents.ts:6-33; subagents-provider.ts | `activity: TaskGraph` (whole session live task graph) | rpi::real_adapter_tracks_and_controls_subagents_per_attached_session | Opus · `SubagentsView`, `SessionChange::SubagentsChanged` | task graph → subagent_tasks | Same visible statuses | — | NC | not started |
| SVC-SUB-CONFIG | subagents-provider.ts:35-56 | `configuration {enabled}` derived from root `pi.agent.extensions` | rpi::real_server_subagent_service_enforces_session_setting | Opus · sessions.subagents_enabled | `pi.agent.extensions` add/remove `subagent` → flag | Same enabled value after import | — | NC | not started |
| SVC-SUB-LIST | subagents-provider.ts:57-100 | `list()` validated rows `{conversationId, taskId, callId, task, status}` (no cap) | planned: rt::subagent_directory | Opus · `Call::Subagents` (≤4096 rows) | child ids mapped | Same rows | — | NC | not started |
| SVC-SUB-VIEW | subagents-provider.ts:113-120 | `view(id)` one-shot ConversationView; `Not a subagent of this session` | planned: rt::subagent_view_ownership | Opus · `Call::SubagentHistory` | — | Same ownership refusals | — | NC | not started |
| SVC-SUB-OPEN | subagents-provider.ts:121-145 | `open(id)` mounts keyed live transcript; cap 64 per worker | planned: rt::child_live_updates | Opus · `SessionChange::ChildChanged` + page | — | Live child updates without mount cap (I-06) | — | NC | not started |
| SVC-SUB-PAGE | subagents-provider.ts:146-148 | `page(id, {before, limit})` like history | planned: rt::subagent_history_paging | Opus · `Call::SubagentHistory` | — | Same entries | — | NC | not started |
| SVC-SUB-ENABLE | subagents-provider.ts:149-178 | `setEnabled(bool)` edits root `pi.agent` in one commit; non-boolean error | rpi::real_server_subagent_service_enforces_session_setting | Opus · `Call::SetSubagentsEnabled` | flag | Same effect on new calls | — | NC | not started |
| SVC-SLASH | svc/slash-commands*.ts (`pi.local.slash-commands`) | Process-local TUI slash commands (`/model`, `/thinking`, `/compact`, `/reload`) | n/a | n/a | n/a | n/a | n/a | NP (process-local to the terminal presentation; `docs/extensions.md` row "Slash commands") | not started |
| SVC-PRESENTATION-UI | svc/presentation-ui.ts | Local TUI `select`/`showStatus` | n/a | n/a | n/a | n/a | n/a | NP (terminal presentation only) | not started |
| SVC-DURABLE-STANDALONE | ce/durable/runtime.ts | Single-process durable TUI runtime | n/a | n/a | n/a | n/a | n/a | NP (not Pipkin's path; A-svc §4.6) | not started |
| SVC-CLIENT-ONESHOT | ce/client.ts:21-86 | `pi client` one-shot prompt | n/a | n/a | n/a | n/a | n/a | NP (CLI, not used by Pipkin) | not started |
| XPORT-HELLO | protocol/src/protocol.ts:5,29-73 | Handshake with version 8; mismatch → `hello_error{version}` | planned: host::hello_rejects_other_protocol | Opus · `Call::Hello`, `PROTOCOL_VERSION`=1 | n/a | Mismatch refused, never guessed | — | NC | unverified (implemented; no test) |
| XPORT-FRAME | protocol/src/framing.ts:6,28 | 4-byte length frames, 16 MiB max | frm::frames_round_trip_and_end_cleanly; frm::oversized_and_truncated_frames_are_refused; frm::garbage_is_a_decode_error_not_a_panic | Opus · P/frame.rs | n/a | Bounded frames refused, not truncated | — | NC | measured: native unit pass |
| XPORT-CANCEL | protocol.ts:49-62 | `cancel{id}` cancels an in-flight request | n/a | Opus | n/a | Native has no generic request cancel; Stop/Auth cancel cover used cases — confirm desktop use | — | PA | not started |
| XPORT-ERRORS | chord/services/errors.ts; server/src/errors.ts | Error code taxonomy incl. `session_not_attached`, `service_stale_instance` | n/a | Opus · P/api.rs `ErrorCode` | n/a | Desktop error handling equivalent | — | NC | unverified |
| XPORT-STALE | A-svc §4.3 | Stale guards: serverConnectionId, attachmentId, worker token, instance generation, state sequence, auth attempt, requestId | planned: host::gap_triggers_snapshot | Opus · epoch/seq/revision (ADR 0002) | n/a | Desktop drops stale events | — | NC | not started |
| XPORT-OUTBOX | plan §3 | Durable ordered outbox with acknowledged cursors, paged catch-up, bounded progress | planned: host::slow_consumer_cannot_block_stop | Opus · E/runtime (not built) | n/a | Plan requirement (no baseline equivalent; Chord collapses >100 deliveries) | — | NC | not started |
| SVC-PROC-WORKERS | ce/session-worker-manager.ts; session-worker.ts:185-306 | Per-session workers with retirement grace, survive server restart (re-adopted), 15 s readiness, no auto-restart | e2e::the_app_owns_the_engine_restarts_it_after_a_crash_and_stops_it_on_quit | Opus · ADR 0001 (single owned child) | n/a | Observable: sessions resume after engine restart; process count independent of sessions | Linux, macOS | NC | not started |
| SVC-PROC-LOCK | ce/session-worker.ts:518-523 | One writer per session dir (proper-lockfile) | st::a_newer_schema_is_refused_untouched_and_the_lock_is_exclusive | Opus · E/store lock_dir | refuse import while source locked | Second engine refused | Linux, macOS | NC | measured: native unit pass |
| SVC-RADIUS-RELAY | ce/radius-relay.ts | `--connect radius:<serverId>` relay | n/a | n/a | n/a | n/a | n/a | NP (Pipkin never uses relay transport) | not started |

## 8. Durable storage, admission, queue, Stop and recovery (P05–P08)

| ID | Source | Baseline behavior | Fixture / test | Owner · module | Migration mapping | Differential assertion | Coverage | Disp. | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| DUR-SCHEMA | D/storage/sqlite/migrations.ts:9-125 | STRICT schema v1, contiguous migrations in one transaction, newer schema refused | st::a_newer_schema_is_refused_untouched_and_the_lock_is_exclusive | Opus · E/store/schema.rs | source schema version recorded in import manifest | Newer refused untouched | Linux, macOS | NC | measured: native unit pass |
| DUR-PRAGMAS | D/storage/sqlite/node.ts:16-17,140-198 | WAL, synchronous=NORMAL, autocheckpoint 1000, busy 5000 ms, BEGIN IMMEDIATE, TRUNCATE checkpoint on close | st::acknowledged_writes_survive_reopen | Opus · E/store/schema.rs (FULL, I-02) | backup via SQLite backup API | Acknowledged write survives kill; power-loss not claimed | Linux, macOS | NC | measured: native unit pass (reopen); kill test not started |
| DUR-MIGRATE-BACKUP | plan §4 | Older schema backed up before migration | planned: st::older_schema_backed_up_then_migrated | Opus · E/store/schema.rs::migrate | n/a | Backup exists and verifies | — | NC | unverified (code path exists; only v1 exists) |
| DUR-ATOMIC | D/session/transaction.ts | Nothing observable before commit; rollback on throw | st::an_injected_failure_rolls_back_and_admits_nothing | Opus · E/store::tx | n/a | No partial admission | — | NC | measured: native unit pass |
| DUR-FAILCLOSED | plan §4 step 5 | (reference: transaction failure throws; worker state) | rt::a_storage_failure_fails_the_session_closed_until_it_is_reopened | Opus · E/runtime/session.rs | n/a | Session refuses mutations until reopen | — | NC | measured: native unit pass |
| DUR-DISKFULL | plan §9 fault list | Disk full / read-only database | planned: st::disk_full_fails_closed | Opus · E/store | n/a | Fail closed, no lost acknowledged rows | Linux | NC | not started |
| DUR-CORRUPT | docs/packaging.md "Damage" (desktop DB) | Corrupt engine data detection/recovery | planned: st::corruption_detected_and_set_aside | Opus · E/store::integrity_check | n/a | Refuse/recover per policy | — | PA | not started |
| DUR-IDS | D/types.ts:39; record_ids | Global monotonically increasing ids; root conversation id 1 | planned: imp::ids_mapped_stably | Opus · E/store, E/import (in progress) | legacy ids → native ids in import manifest | Desktop item identity stable after import | — | NC | not started (in progress) |
| ADM-KEY | D/harness/submissions.ts:160-219 | Same `(conversation, requestId)` + same type → original id (content ignored); different type → error | st::same_key_same_payload_is_the_original_and_changed_payload_conflicts | Opus · E/store::admit | request ids preserved | Same key/same payload identical; changed payload → Conflict (I-01) | — | NC | measured: native unit pass; differential not started |
| ADM-BUSY | submissions.ts; agent-controller-provider.ts:118-121 | Busy + reject → `ConversationBusy` (`busy`), nothing written | rt::a_busy_session_refuses_a_second_prompt_without_recording_it | Opus · E/runtime/session.rs | n/a | Same | — | NC | measured: native unit pass; differential not started |
| ADM-IDLE | submissions.ts:160-219 | Idle + empty inbox: input appended, placed, run started | rt::a_prompt_is_admitted_durably_streamed_settled_and_looked_up | Opus · E/runtime/session.rs | n/a | Same | — | NC | measured: native unit pass; differential not started |
| ADM-LOOKUP | agent-controller-provider.ts:59-75 | Lookup statuses/reasons (`aborted`, `model_error`, `no_model`, `stale`, `reset`, `faulted`, orphan reasons) | rt::a_prompt_is_admitted_durably_streamed_settled_and_looked_up | Opus · `LookupResult`, `RunOutcome` | reason mapping table in importer | Every reference reason maps to a native outcome | — | NC | measured (done/aborted only); full mapping not started |
| ADM-BOUNDS | ce/process.ts:100; framing 16 MiB | Effective input caps were frame sizes only | rt::inputs_are_bounded_before_anything_is_recorded | Opus · P/limits.rs | n/a | Native explicit limits (2 MiB text, 8 images × 8 MiB); recorded difference D-03 | — | NC | measured: native unit pass |
| Q-STEER-ONE | D/harness/inbox.ts:284-325 | `steeringMode: one-at-a-time` places the first steer at postTools | planned: rt::steer_one_at_a_time | Opus · E/runtime | inbox items → requests queue | Same next request | — | NC | not started |
| Q-STEER-ALL | inbox.ts | `steeringMode: all` places all steers | planned: rt::steer_all | Opus · E/config QueueDelivery | settings import | Same | — | NC | not started |
| Q-FOLLOW-ONE | inbox.ts | `followUpMode: one-at-a-time` | rt::stop_withdraws_queued_inputs_and_follow_ups_run_after_completion | Opus · E/runtime | — | Same run order | — | NC | measured: native unit pass; differential not started |
| Q-FOLLOW-ALL | inbox.ts | `followUpMode: all` | planned: rt::follow_up_all | Opus · E/runtime | settings import | Same | — | NC | not started |
| Q-WITHDRAW | submissions.ts:102-115 | Withdraw queued → aborted; placed → already_placed | rt::queued_inputs_can_be_withdrawn_before_they_run | Opus · E/store::cancel_queued | — | Same outcomes | — | NC | measured: native unit pass; differential not started |
| Q-WRITES | inbox.ts (`write` submissions) | App `write` submissions (entries), stale head check | n/a | — | write entries imported as entries | Desktop never submits writes; compaction uses them internally | — | PA | not started |
| Q-BOUND | (no reference cap) | Queue length unbounded in reference | st::queue_entries_are_ordered_withdrawable_and_bounded | Opus · P/limits.rs MAX_QUEUE=64 | import refuses >64 queued? decide | Recorded difference D-03 | — | NC | measured: native unit pass |
| STOP-INTENT | plan §4; D/scheduler.ts:321+ | Abort marks tasks abort_requested in one commit, then signals | st::stop_intent_is_durable_and_settlement_closes_the_run | Opus · E/store::request_stop | `abort_requested` tasks → runs.stopping | Stop durable before reply | — | NC | measured: native unit pass; differential not started |
| STOP-WITHDRAW | inbox.ts:343-351 | Stop withdraws queued inputs (writes stay) | rt::stop_withdraws_queued_inputs_and_follow_ups_run_after_completion | Opus · E/runtime/session.rs | — | Same | — | NC | measured: native unit pass; differential not started |
| STOP-PARTIAL | generation.ts:256-287 | Committed streaming partial → `aborted` assistant entry (usage recorded, excluded from context) | rt::stop_settles_aborted_and_retires_the_instruction_from_the_next_context | Opus · E/runtime/session.rs | live partial → aborted entry | Same entry + usage | — | NC | measured: native unit pass; usage part not started |
| STOP-UNSTARTED | generation.ts:256-287 | Unstarted sequential calls get `Tool <name> was aborted` results | planned: rt::stop_mid_tool_round | Opus · E/runtime/session.rs | — | Same tool exchange validity | — | NC | unverified (implemented; no test) |
| STOP-RETIRE | D/session/transaction.ts:822-845 | `pi.cancelled-input` omit edit on stopped user entry (prompt + placed steers) | rt::stop_settles_aborted_and_retires_the_instruction_from_the_next_context | Opus · E/store::retire_entries (ADR 0004) | omit edit → `retired=1` | Next request after Stop+new prompt identical | — | NC | measured: native unit pass; differential not started |
| STOP-GOAL | e2e::stopping_a_prompt_flushes_its_instruction_before_a_replacement_goal | Replacement goal after Stop does not revive the instruction; no continuation after user Stop | e2e (Pi) | Opus · Core goals + App/adapters/native (planned) | goals in desktop DB | Same goal outcome | desktop | NC | not started |
| STOP-RACE | plan §4 | A run completed before Stop observed stays completed | planned: rt::stop_after_completion_keeps_completed | Opus · E/runtime | — | Same | — | NC | not started |
| STOP-RESTART | plan §4 | Persisted Stop across crash | rt::a_durable_stop_survives_a_restart_and_settles_aborted | Opus · E/runtime/session.rs | — | Settles aborted on reopen | — | NC | measured: native unit pass |
| STOP-BACKGROUND | D/scheduler.ts abort traversal | Background (threshold) compactions survive Stop | planned: rt::background_compaction_survives_stop | Opus · E/context (in progress) | — | Same | — | NC | not started (in progress) |
| STOP-CHILDREN | scheduler.ts abort traversal | Stop aborts owned subagent child conversations | planned: rt::stop_aborts_children | Opus · E/runtime/subagents.rs | — | Child settles aborted | — | NC | not started |
| REC-RESUME | ce/session-worker.ts:595; scheduler.ts:230-256 | On open: running → pending; `harness.resume()` continues interrupted turns automatically | rt::an_engine_restart_mid_run_resumes_the_run_without_duplicating_the_prompt | Opus · E/runtime (open path) | task phases → run state | Same automatic resume policy (no prompt resend) | — | NC | measured: native unit pass; differential not started |
| REC-REQUEST | generation.ts:182-206 | Reopen in `request`: partial → aborted entry, re-send with pinned model/thinking/options | rt::an_engine_restart_mid_run_resumes_the_run_without_duplicating_the_prompt | Opus · E/runtime/run.rs | — | Same re-sent request | — | NC | measured: native unit pass; differential not started |
| REC-RETRY | generation.ts retry phase | Durable retry `until` honored after reopen | planned: rt::retry_wait_survives_restart | Opus · E/runtime/run.rs | — | Same | — | NC | not started |
| REC-ORPHAN | live.ts:156-187 | Missing/too-new task definition → blocked; orphaned/faulted cleanup | planned: rt::unknown_task_blocks_explicitly | Opus · E/runtime | unsupported imported task → conversation stays on Pi | Explicit error, no silent new work | — | NC | not started |
| REC-JOURNAL | e2e::a_dropped_acknowledgment_then_a_relaunch_never_runs_the_prompt_twice | Desktop journal → lookup reconciliation | e2e (Pi) | Opus · App/adapters/native (planned) | journal keys unchanged | Same | desktop | NC | not started (in progress) |
| DOC-PROVIDER | D/harness/provider.ts | `pi.provider {sessionId: uuidv7}` per conversation/fork for cache affinity | planned: rt::provider_session_id_stable | Opus · E/runtime (kv) | import `pi.provider.sessionId` | Same session id sent before/after import | prompt caching providers | NC | not started |
| DOC-AGENT | D/harness/agent.ts:41-50 | `pi.agent {model, thinkingLevel, extensions, tools, instructions, cwd}` rewindable | planned: imp::agent_doc_mapping | Opus · sessions columns + kv | fields → columns; unknown → kv | Same effective config | — | NC | not started (in progress) |
| DOC-LIVE | D/harness/live.ts:43-71 | `pi.live` run/generation/tools/compactions status | rt:: (RunView) | Opus · RunView, ToolProgress | live not imported (quiesce first) | Same visible run status | — | NC | measured (run view in rt:: tests); differential not started |
| FORK | D/session/forks.ts | `fork(at)` conversation branching | n/a | n/a | forked conversations (if any exist) must import | n/a | n/a | NP (A-tools §3.10: no branch API in server services) — import still required | not started |
| RESET | D/harness/types.ts:515-519 | `reset(handoff?)` writes `pi.reset` head marker | n/a | Opus · E/context (head markers) | `pi.reset` entries → head markers | Imported context starts after reset | — | NP (not reachable via services; A-tools §3.10) — import still required | not started |
| P05-CATALOG | Core/state.rs; App/adapters/pi/mod.rs | Conversation catalog, create/rename/reopen, projects, stable item identity | e2e::a_real_run_edits_files_shows_output_and_diff_and_reopens_the_same_history | Opus · `Call::ListSessions/CreateSession/RenameSession/OpenSession` | desktop conversation ↔ session id map | Catalog equal before/after restart and import | desktop | NC | not started |
| P05-RENAME | P/api.rs `RenameSession` | Session titles (reference has none in the catalog; desktop-side titles) | planned: rt::rename_session | Opus · E/runtime | desktop titles stay in desktop DB | Same visible titles | desktop | PA | not started |
| P08-GOALS | Core/state.rs (goals) | Goals, continuation scheduling, replacement/pause/clear; paused on reopen | e2e::stopping_a_prompt_flushes_its_instruction_before_a_replacement_goal | Opus · Core (unchanged owner) + native adapter | desktop DB unchanged | Same terminal event ordering | desktop | NC | not started |

## 9. Subagents (P09, P10)

| ID | Source | Baseline behavior | Fixture / test | Owner · module | Migration mapping | Differential assertion | Coverage | Disp. | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| SUB-CREATE | subagent.ts:35-42 | One child per tool task (`scanConversations({ownerTaskId})`), ownership `{kind:"task"}` | st::a_child_task_is_created_once_per_parent_call_and_delivered_once | Opus · E/store::create_task, E/runtime/subagents.rs | owner task → subagent_tasks | One child after crash at creation | — | NC | measured: store unit pass; crash test not started |
| SUB-INHERIT | agent.ts:133-139; subagent.ts | Child copies parent agent doc (model, thinking, tools filter, instructions, cwd), `extensions:{remove:["subagent"]}` | planned: rt::child_inherits_and_cannot_delegate | Opus · E/runtime/subagents.rs | — | Child request has same model/tools minus subagent | — | NC | not started |
| SUB-SUBMIT | subagent.ts:44-46 | Child prompt `requestId: "subagent:<taskId>"` (idempotent) | planned: rt::child_admission_is_idempotent | Opus · E/runtime/subagents.rs (`subagent:<call id>`) | key mapping recorded | One admission on replay | — | NC | not started |
| SUB-RESULT | subagent.ts:13-17 | Answer = concatenated text parts of child answer; non-done → `Subagent <id> failed: <status>` | planned: rt::child_answer_and_failure_text | Opus · E/runtime/subagents.rs | — | Same tool result text | — | NC | not started |
| SUB-DELIVER-ONCE | plan §4 | Parent result delivered once despite replay | st::a_child_task_is_created_once_per_parent_call_and_delivered_once | Opus · E/store::mark_task_delivered | — | One parent-visible result | — | NC | measured: store unit pass |
| SUB-RECOVERY | tool.ts:114-154 (+ overlay) | Replay-safe rerun finds child + submission, waits | planned: rt::subagent_resumes_after_restart | Opus · E/runtime/run.rs | in-flight child tasks preserved | Child count 1, result once | — | NC | not started |
| SUB-RECOVERY-DISABLED | pi-subagents.patch tool.ts hunk; ovl test | Resume after subagents disabled + restart; new calls fail `Tool subagent is not available` | planned: rt::disabled_subagents_still_recover (port of overlay test) | Opus · E/runtime | enable flag import | Run counts 2, later call errors | — | NC | not started |
| SUB-RECOVERY-CONTRACT | pi-subagents.patch tool.ts hunk | Recorded `recoveryContract` mismatch (removed registration, changed description/replay) → no recovery, `interrupted` | planned: rt::subagent_contract_mismatch_is_interrupted (port) | Opus · E/runtime (ADR 0005 open item) | recorded contract imported from checkpoint | Same refusal | — | NC | not started |
| SUB-ENABLE | subagents-provider.ts:149-178 | Per-session enable stored in root agent doc; default enabled | rpi::real_server_subagent_service_enforces_session_setting | Opus · sessions.subagents_enabled | `pi.agent.extensions` → flag | Same | — | NC | not started |
| SUB-DIR | subagents-provider.ts:57-108 | Directory rows, status mapping (done/aborted/failed or raw pending/running/waiting/completing), ownership validation | planned: rt::subagent_directory_rows | Opus · `SubagentsView` | — | Same rows/statuses | — | NC | not started |
| SUB-LIVE | subagents-provider.ts:121-145 | Live child transcript (keyed), 64-mount cap | planned: rt::child_live_updates | Opus · `ChildChanged` | — | Live updates; cap removed (I-06) | — | NC | not started |
| SUB-PAGE | subagents-provider.ts:146-148 | Child history paging ≤200 | planned: rt::subagent_history_paging | Opus · `Call::SubagentHistory` | — | All child entries reachable | — | NC | not started |
| SUB-CANCEL | tool.ts failed-ending | Failing/aborting the tool task aborts owned children | planned: rt::stop_aborts_children | Opus · E/runtime/subagents.rs | — | Same | — | NC | not started |
| SUB-PERMITS | plan §4 | Waiting parent releases execution permit; nested work cannot deadlock | planned: rt::nested_children_do_not_deadlock | Opus · E/runtime (Semaphore) | — | Plan requirement | — | NC | not started |
| SUB-UI-DIRECTORY | docs/subagent-inspector.md | Flat directory, statuses, finished history (100-row bounds), selection never moves composer | App UI tests (existing); rpi::real_adapter_tracks_and_controls_subagents_per_attached_session | Opus · App/adapters/native (planned) | — | Same visible inspector | desktop | NC | not started |
| SUB-UI-INSPECT | docs/subagent-inspector.md | Read-only child activity view; Loading/Live/Snapshot/unavailable distinct | App UI tests | Opus · App/adapters/native (planned) | — | Same | desktop | NC | not started |
| SUB-IMPORT | plan §6 step 4 | Import task graph, parent/child/call mapping and enable state | planned: imp::subagent_graph | Opus · E/import (in progress) | conversations owner → subagent_tasks | Same directory after import | — | NC | not started (in progress) |

## 10. Attachments, models and usage (P11)

| ID | Source | Baseline behavior | Fixture / test | Owner · module | Migration mapping | Differential assertion | Coverage | Disp. | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| ATT-IMAGES | svc/agent-controller.ts:3-19; agent-controller-provider.ts:112-117 | Prompt images `{data(base64), mimeType}` → content `[text, ...images]`; stored verbatim; no resize/limits on server path | e2e::attachments_reach_the_model_as_text_and_image_and_come_back_as_chips | Opus · `ImageInput`, E/runtime | base64 in entries preserved | Same request content | vision models | NC | not started |
| ATT-CAPABILITY | transform-messages.ts | Images replaced by placeholder when model lacks `image` input | planned: prov::image_placeholder_for_text_models | Opus · E/provider (in progress) | — | Same | text-only models | NC | not started (in progress) |
| ATT-DESKTOP | Core/state.rs; App/storage.rs | Desktop preprocessing, attachment chips, draft persistence, missing-file block | e2e::a_draft_with_attachments_survives_a_restart_and_a_missing_file_blocks_sending | Opus · desktop (unchanged) | desktop DB unchanged | Same | desktop | NE | not started |
| MOD-SELECT | see SVC-MODELS-* | Per-session model/thinking selection, restored per conversation | e2e::choosing_effort_is_confirmed_by_pi_and_restored_per_conversation | Opus · `Call::SetModel/SetThinking` | `pi.agent` → session columns | Same | — | NC | measured (rt:: unit); e2e not started |
| MOD-SWITCH-CONTEXT | transform-messages.ts | Provider/model switch mid-conversation replays history with normalization | planned: prov::switch_provider_mid_conversation | Opus · E/provider (in progress) | — | Same next request | all | NC | not started (in progress) |
| MOD-OFFLINE-CATALOG | pkg/engine-model-data.md | Catalog available offline from pinned data | mdl::embedded_files_match_the_pinned_manifest; mdl::the_catalog_loads_every_provider | Opus · E/provider/model.rs (include_str!) | n/a | Same model metadata | all | NE | measured: native unit pass |
| USE-LEDGER | D/harness/usage.ts:7-55 | `pi.usage {models{"p/m": Usage}, tools{name: Usage}}` per conversation, every assistant append incl. errors/aborted, summarization attempts | st::usage_accumulates_per_bucket | Opus · E/store usage | `pi.usage` → usage rows | Same totals after a scripted session and after import | — | NC | measured: native unit pass; differential not started |
| USE-TOTAL | Harness.usage() | Session total sums all conversations incl. subagent children | planned: rt::usage_includes_children | Opus · E/store | — | Same totals | — | NC | not started |
| USE-DESKTOP | Core model usage display | Usage/cache accounting shown by desktop | e2e (Pi transcripts) | Opus · UsageView + native adapter | — | Same displayed values | desktop | NC | not started |

## 11. Prompt construction and resources (P12)

| ID | Source | Baseline behavior | Fixture / test | Owner · module | Migration mapping | Differential assertion | Coverage | Disp. | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| PRM-SECTIONS | ce/durable/prompt.ts:20,26-74 | Fixed section order preamble, tools, rules, docs, project_context, skills, cwd; untagged durable sections with pre-tagged text | planned: ctx::system_prompt_golden | Opus · E/context (in progress) | n/a | Byte-identical system prompt for fixture projects (modulo D-01 docs paths) | all | NC | not started (in progress) |
| PRM-PREAMBLE | ca/core/system-prompt.ts:146-147 | Exact preamble text naming "pi" | planned: ctx::preamble | Opus · E/context | n/a | Text decision (product name) recorded as D-02 | all | PA | not started |
| PRM-TOOLS | system-prompt.ts:148-151 | `- name: snippet` for read/write/edit/bash + trailer sentence | planned: ctx::tools_section | Opus · E/context | n/a | Same | all | NC | not started (in progress) |
| PRM-RULES | system-prompt.ts:81-118 | Rule bullets in order incl. edit guidelines and PI_* bullet | planned: ctx::rules_section | Opus · E/context | n/a | Same (see K-02 for PI_* bullet) | all | NC | not started (in progress) |
| PRM-DOCS | system-prompt.ts:153-160; config.ts:444-456 | Pi documentation section with package README/docs/examples paths | planned: decide | Opus · E/context | n/a | Native has no Pi docs; decision D-01 | all | PA | not started |
| PRM-CWD | system-prompt.ts:170 | `<cwd>` with forward slashes | planned: ctx::cwd_section | Opus · E/context | session cwd | Same | Linux, macOS | NC | not started (in progress) |
| PRM-NODATE | A-prompt §2.1 | No date/time injected (cache-stable prompt) | planned: ctx::prompt_is_stable_across_turns | Opus · E/runtime/glue.rs | n/a | Identical prompt across turns | all | NC | not started (in progress) |
| PRM-PATCH | D/harness/prompt.ts:66-142 | `pi.system` positional entries: full baseline after head marker (omitting older system entries), minimal patches, remove-all/re-add-all on order change; tool add/remove on last entry | planned: ctx::system_entries_plan | Opus · E/context (in progress) | `pi.system` entries imported; baseline/omit edits preserved | Same mid-conversation system updates | APIs with mid-convo system | NC | not started (in progress) |
| PRM-THROWING-SECTION | D/harness/prompt.ts:25-48 | A throwing section keeps previously shown text | planned: ctx::failing_section_keeps_text | Opus · E/context | n/a | Same | — | NC | not started (in progress) |
| PRM-AGENTS-NAMES | ca/core/resource-loader.ts:185 | Per directory first of `AGENTS.override.md`, `AGENTS.md`, `AGENTS.MD`, `CLAUDE.md`, `CLAUDE.MD`; non-files skipped; BOM stripped | planned: ctx::agents_file_precedence | Opus · E/context (in progress) | n/a | Same files chosen | case-sensitive FS (Linux), macOS | NC | not started (in progress) |
| PRM-AGENTS-GLOBAL | resource-loader.ts:242-246 | Global file from agentDir emitted first | planned: ctx::global_agents_first | Opus · E/context; E/config config_dir | explicit import of `~/.pi/agent/AGENTS.md` | Same order | — | NC | not started (in progress) |
| PRM-AGENTS-WALK | resource-loader.ts:253-265 | Ancestor walk to filesystem root (no git/HOME stop), outermost first, dedupe by path | planned: ctx::ancestor_walk | Opus · E/context | n/a | Same list | — | NC | not started (in progress) |
| PRM-AGENTS-WORKTREE | resource-loader.ts:214-230 | Nested linked-worktree shadowing of main repo's same-name file | planned: ctx::worktree_shadowing | Opus · E/context | n/a | Same | git worktrees | NC | not started (in progress) |
| PRM-AGENTS-NOTRUST | A-prompt §2.3 | Context files loaded regardless of trust; no size limit | planned: ctx::no_truncation | Opus · E/context | n/a | Same | — | NC | not started (in progress) |
| PRM-AGENTS-CACHE | ce/durable/prompt.ts:27-39 | Context files and skills loaded once per cwd per worker (cached) | planned: ctx::resource_cache_semantics | Opus · E/context | n/a | When edits to AGENTS.md take effect (worker lifetime vs native lifetime) — record difference | — | PA | not started |
| PRM-SKILLS-DISCOVERY | ca/core/skills.ts:452-503; prompt.ts:34 | Durable server: `<agentDir>/skills`, `<cwd>/.pi/skills` (not trust-gated), settings `skills` resolved against cwd; no `.agents/skills`, no packages | planned: ctx::skill_discovery_durable | Opus · E/context (in progress) | explicit import of `~/.pi/agent/skills` | Same skill set (see K-01) | — | NC | not started (in progress) |
| PRM-SKILLS-FORMAT | skills.ts:67-343 | Frontmatter parse; name rules (warnings only); required description ≤1024 (warn); SKILL.md stops recursion; ignore files honored; symlinks followed | planned: ctx::skill_parsing | Opus · E/context | n/a | Same accepted skills and diagnostics | — | NC | not started (in progress) |
| PRM-SKILLS-COLLISION | skills.ts:421-450 | Same realpath skipped; same name → collision diagnostic, first wins | planned: ctx::skill_collisions | Opus · E/context | n/a | Same | — | NC | not started (in progress) |
| PRM-SKILLS-LISTING | skills.ts:355-383 | `<available_skills>` XML listing (escaped), excludes `disable-model-invocation`, read-tool wording | planned: ctx::skills_section_golden | Opus · E/context | n/a | Byte-identical | — | NC | not started (in progress) |
| PRM-SKILL-CMD | ca/core/agent-session.ts:2100-2124 | `/skill:name args` explicit invocation | n/a | n/a | n/a | n/a | n/a | NP (classic AgentSession only; durable server passes text verbatim, A-prompt §2.1/§2.4) | not started |
| PRM-TEMPLATES | ca/core/prompt-templates.ts | Prompt templates `/name args` | n/a | n/a | n/a | n/a | n/a | NP (classic only; experimental client sets `noPromptTemplates`, A-ext §5.4) | not started |
| PRM-SYSTEM-MD | resource-loader.ts:1201-1227 | `SYSTEM.md` / `APPEND_SYSTEM.md` | n/a | n/a | n/a | n/a | n/a | NP (durable server ignores both, A-prompt §2.1) | not started |
| PRM-TRUST | ca/core/project-trust.ts | Project trust prompt | n/a | n/a | n/a | n/a | n/a | NP (durable path uses `projectTrusted: true`, A-prompt §7.7) | not started |

## 12. Context management and compaction (P13)

| ID | Source | Baseline behavior | Fixture / test | Owner · module | Migration mapping | Differential assertion | Coverage | Disp. | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| CTX-DERIVE | D/harness/context.ts:68-163 | Context = newest head marker + range; omit/replace edits (newest per target wins); drop aborted/error/deferred; order tool results; fork-aware | st::context_starts_at_the_newest_head_marker_and_pages_reach_everything | Opus · E/store::context_entries, E/runtime/glue.rs, E/context | head markers, edits, forks imported | Same next request for imported fixtures | — | NC | measured (head marker + paging); edits/differential not started |
| CTX-EDIT-REPLACE | context.ts | `replace` edits supported (not produced by built-ins) | planned: imp::replace_edit_blocks_or_maps | Opus · E/context, E/import | preserve; block conversion if unsupported | Same context | — | PA | not started |
| CTX-THRESHOLD | generation.ts:303-322; compaction.ts | Blocking compaction when tokens > contextWindow − reserveTokens (16384) | planned: ctx::threshold_blocking_compaction | Opus · E/context (in progress) | — | Same trigger point/tokens estimate | — | NC | not started (in progress) |
| CTX-BACKGROUND | generation.ts:303-322 | Background compaction when > blocking − backgroundTokens (32768) | planned: ctx::background_compaction | Opus · E/context (in progress) | — | Same | — | NC | not started (in progress) |
| CTX-OVERFLOW | generation.ts | Overflow error → one blocking `overflow` compaction per attempt | planned: ctx::overflow_compaction | Opus · E/context (in progress) | — | Same | — | NC | not started (in progress) |
| CTX-MANUAL | agent-controller-provider.ts:76-83 | Manual compaction with instructions; Esc/Stop aborts it | planned: rt::manual_compaction | Opus · `Call::Compact` (Unsupported today), E/context | — | Same summary request | — | NC | not started (in progress) |
| CTX-ESTIMATE | compaction.ts:307-322; ai/utils/estimate.ts | Token estimate: last usage + chars/4 (images 4800) + planned system entries | planned: ctx::estimate | Opus · E/context (in progress) | — | Same estimates | — | NC | not started (in progress) |
| CTX-CUT | compaction.ts:255-295 | `selectCut` keepRecentTokens 20000 walk-back with candidate rules | planned: ctx::select_cut | Opus · E/context (in progress) | — | Same cut entry | — | NC | not started (in progress) |
| CTX-SUMMARY-PROMPT | compaction.ts:57-207 | Exact summarization system prompt, format, serialization (2000-char tool result truncation), cacheRetention none, maxTokens min(0.8·reserve, model) | planned: ctx::summary_request_golden | Opus · E/context (in progress) | — | Byte-identical request | — | NC | not started (in progress) |
| CTX-SUMMARY-RESULT | compaction.ts | Valid summary rules; failure texts; retry via durable retry; usage recorded | planned: ctx::summary_failures | Opus · E/context (in progress) | — | Same outcomes | — | NC | not started (in progress) |
| CTX-PLACEMENT | compaction.ts:407-436 | `pi.compaction` head marker with wrapped summary; non-blocking placed via write submission (`compaction:<taskId>`), stale if passed | planned: ctx::placement_and_stale | Opus · E/context (in progress) | `pi.compaction` → head marker entry | Same context after compaction | — | NC | not started (in progress) |
| CTX-BOUNDARY-UI | e2e::history_before_a_compaction_is_paged_in_once_and_in_order | Desktop pages history before a compaction once, in order | e2e (Pi) | Opus · `BoundaryMoved`, HistoryPage | — | Same visible history | desktop | NC | not started |
| CTX-SETTINGS | ca/core/settings-manager.ts:23-33,914-960 | `compaction.enabled/reserveTokens/keepRecentTokens` (+`modelOverrides` per model) | cfg::project_settings_override_global_and_defaults_apply | Opus · E/config.rs CompactionSettings | settings import | Same effective policy | — | NC | measured (settings parse); modelOverrides not started |
| CTX-HOOK | D/harness hooks `beforeCompact` | Hooks can decline or supply a summary (no built-in uses it) | n/a | Opus · E/extensions (planned) | — | — | — | NP (no built-in registers it; A-ext §5.5) | not started |

## 13. Extensions and extension questions (P14, P15)

Baseline mechanism = Chord facet plugin packages loaded via `--pi-extension` → `pi server -e`
(A-ext §5.1–5.2). Arbitrary TypeScript facet compatibility needs Node; see ADR 0006.

| ID | Source | Baseline behavior | Fixture / test | Owner · module | Migration mapping | Differential assertion | Coverage | Disp. | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| EXT-FACET-SESSION | ce/plugins/bundled.ts:12-30; ce/session-worker.ts:819 | Session facet of a facet package (esbuild CJS bundle) runs in the session worker | e2e::an_extension_in_the_engine_asks_three_questions_and_the_answers_reach_it | Opus · E/extensions (planned; native contract) · Pi for existing packages | session plugin profile recorded; conversation stays Pi-owned while a selection is non-empty | Same question/answer/service outcomes for each baseline package | per package | BL / LB | not started |
| EXT-FACET-SERVICES | chord facet `provide`/`provideMany` | A session facet can provide new remote services (e.g. `pi.example-plugin.greeting`) | planned: ext::custom_service_fixture | Opus · E/extensions (planned) | — | Same service results | per package | BL / LB | not started |
| EXT-FACET-CONTROL | ce/plugin.ts:1-28 (AgentController, History) | Facets can prompt/steer/followUp/cancelQueued/abort/compact/lookup/waitForPrompt and page history | planned: ext::session_control_fixture | Opus · E/extensions (planned) | — | Same effects through native contract | per package | BL / LB | not started |
| EXT-FACET-TUI | ce/plugins/package.ts:87-88 | TUI facet artifacts for terminal clients | n/a | n/a | n/a | n/a | n/a | NP (Pipkin never runs TUI facets; `docs/extensions.md`) | not started |
| EXT-PROFILE-STICKY | ce/plugins/package.ts:21-34 | Omitting `-e` restores `plugin-packages-<serverId>.json`; explicit list persisted | planned: ext::profile_restore | Opus · E/extensions + App flags | server profile JSON read by importer | Same effective selection | — | BL / LB | not started |
| EXT-PROFILE-SESSION | package.ts:37-66; ce/server.ts:573-602 | Each session pins the selection it first opened with | planned: ext::session_pins_selection | Opus · E/extensions (kv) | per-session profile → kv | Same | — | BL / LB | not started |
| EXT-CONFLICT | session-worker-manager.ts:136-142 | Different selection for an active session → `Session <id> is active with a different plugin selection` | planned: ext::selection_conflict | Opus · E/extensions | — | Same refusal | — | BL / LB | not started |
| EXT-RELOAD | svc/worker.ts:68-117 | `SessionPlugins.reload()` hot-reloads facets, disposes retired generation | n/a (Pipkin does not call reload) | — | — | Desktop never calls reload | — | PA | not started |
| EXT-NATIVE | plan §2; ADR 0006 | Native versioned extension contract (tools, sections, hooks, questions, session control) | planned: ext::native_contract_conformance | Opus · E/extensions (planned) | n/a | Capability addition; not parity evidence | — | NC | not started |
| EXT-DURABLE-BUILTIN | D/harness/{define,registry}.ts | Durable Extension API used only by built-ins (CodingTools, pi-prompt, subagent) | covered by TOOL-*, PRM-*, SUB-* | Opus · E/tools, E/context, E/runtime | — | — | — | NE | not started (in progress) |
| EXT-CLASSIC | ca/core/extensions/* | Classic `ExtensionAPI` extensions (tools, commands, providers, hooks, UI) | n/a | n/a | n/a | n/a | n/a | NP (A-ext §5.4: no experimental file calls the classic loader) | not started |
| EXT-MCP | ca/core/mcp-servers.ts; extensions/mcp | MCP servers | n/a | n/a | n/a | n/a | n/a | NP (A-ext §5.4) | not started |
| EXT-BUILTIN-CLASSIC | ca/extensions/index.ts:7-14 | `codemode`, `tool-search`, `mcp`, `llama.cpp` built-in extensions | n/a | n/a | n/a | n/a | n/a | NP (A-ext §5.4) | not started |
| EXT-PACKAGES | ca/core/package-manager.ts | npm/git packages (extensions/skills/prompts/themes) | n/a | n/a | n/a | n/a | n/a | NP (A-ext §5.4: `loadSkills` not given package paths) | not started |
| UIQ-SELECT | svc/ui-requests*.ts | `select(title, items, {message, timeoutMs})` → value or undefined | e2e::an_extension_in_the_engine_asks_three_questions_and_the_answers_reach_it | Opus · UiView/QuestionView + E/extensions host (planned) | — | Same question ids/validation | — | BL / LB | not started |
| UIQ-CONFIRM | same | `confirm` → boolean; `A confirmation needs yes or no.` | same | same | — | Same | — | BL / LB | not started |
| UIQ-INPUT | same | `input` with placeholder/default; ≤10 000 chars | same | same | — | Same | — | BL / LB | not started |
| UIQ-NOTIFY | ui-requests-provider.ts:54 | Notices info/warning/error ≤500 chars, 20 kept FIFO | same | same + `Call::DismissNotices` | — | Same | — | BL / LB | not started |
| UIQ-STATUS | ui-requests-provider.ts:122-152 | `setStatus(key≤80, text≤300 or null)` | same | same | — | Same | — | BL / LB | not started |
| UIQ-DEADLINE | ui-requests.ts:93-97 | Default 120 s timeout (0 = forever) with notice `No answer to "<title>"; it was cancelled.`; abort → undefined | e2e::declining_ends_the_question_and_a_lost_connection_does_not_leave_it_answerable | same | — | Same | — | BL / LB | not started |
| UIQ-LIMIT | ui-requests.ts:94; P/limits.rs MAX_OPEN_QUESTIONS | 20 pending max, extra refused with warning notice | planned: ext::question_limit | same | — | Same | — | BL / LB | not started |
| UIQ-NOHANG | plan P15 | Pending questions cannot hang recovery; facet disposal cancels all | planned: ext::questions_cancelled_on_shutdown | same | questions never imported (volatile) | Same | — | BL / LB | not started |
| UIQ-MULTICLIENT | A-svc §4.4 | First responder wins; no per-client ownership | planned: ext::first_responder_wins | same | — | Same | — | BL / LB | not started |
| UIQ-DESKTOP | docs/extensions.md | Desktop dialogs, strip, deadline text, palette entries, reconnect behavior | App UI tests (existing) | Opus · desktop (unchanged) | — | Same | desktop | NE | not started |

## 14. Tool output retention and artifacts (P16)

| ID | Source | Baseline behavior | Fixture / test | Owner · module | Migration mapping | Differential assertion | Coverage | Disp. | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| OUT-LIVE | D/harness/tool.ts:339-386; output.ts:251-258 | Live progress window (head/tail ≤50 KiB/2000 lines) committed ≤ every 100 ms, ~100 KiB/s | planned: rt::tool_progress_is_coalesced | Opus · `SessionChange::ToolProgress` (volatile) | live slots not imported | Same visible progress cadence (bounded) | — | NC | not started |
| OUT-ENTRY | tool.ts:512-548 | Bounded content stored in the tool-result entry (what the model saw) + details + diagnostics | planned: tools::result_entry_shape | Opus · E/store tool_result | entries imported verbatim | Same entry content | — | NC | not started (in progress) |
| OUT-RETAINED | env/node.ts:776-790 (bash spill only) | Full bash output in temp spill file (never cleaned) | st::tool_output_is_retained_with_honest_drop_counts | Opus · E/store tool_outputs (I-04) | spill paths kept as text; files not copied | Native retains ≤16 MiB per call in SQLite with dropped-byte count | — | NC | measured: native unit pass |
| OUT-FETCH | e2e::the_complete_output_of_a_long_tool_result_is_copied_and_saved_not_the_preview | Desktop copies/saves the complete output, not the preview | e2e (Pi) | Opus · `Call::ToolOutput` + native adapter | — | Same copied bytes (up to retention) | desktop | NC | not started |
| OUT-RECOVERY | tool.ts:463-474 | Interrupted/aborted results rebuilt from last committed partial output | planned: rt::interrupted_result_uses_committed_partial | Opus · E/runtime | — | Same | — | NC | not started |

## 15. Settings keys the durable harness reads (and server-level keys)

Durable harness reads only `stream` (provider retry/idle timeout), `compaction`, `retry`,
`steeringMode`, `followUpMode` and `skills` (A-prompt §7.4, `ce/durable/harness-setup.ts:25-49`,
`prompt.ts:34`). Server/model-runtime keys are listed separately. Native loader: `E/config.rs`
(`Settings`; unknown keys kept in `other`). Migration for every row: explicit import of
`~/.pi/agent/settings.json` and `<cwd>/.pi/settings.json` semantics (project file is read in
place, natively too).

| ID | Source | Baseline behavior | Fixture / test | Owner · module | Migration mapping | Differential assertion | Coverage | Disp. | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| SET-MERGE | settings-manager.ts:191-254,473-476 | Global + project `.pi/settings.json` deep merge; arrays/scalars replace; project trusted on durable path | cfg::project_settings_override_global_and_defaults_apply | Opus · E/config.rs | import global file | Same effective settings | — | NC | measured: native unit pass; differential not started |
| SET-INVALID | settings-manager.ts | Invalid file → warn, defaults | cfg::invalid_files_warn_and_fall_back | Opus · E/config.rs | — | Same | — | NC | measured: native unit pass |
| SET-MIGRATIONS | settings-manager.ts:504-563 | `queueMode`→`steeringMode`, `websockets`→`transport`, old `skills{}` object, `retry.maxDelayMs`→`retry.provider.maxRetryDelayMs` | planned: cfg::legacy_keys_migrate | Opus · E/config.rs | import applies migrations | Same values | — | NC | not started |
| SET-RETRY-ENABLED | settings-manager.ts:987-1006 | `retry.enabled` (true) | rt::model_errors_retry_then_settle_and_missing_models_are_reported | Opus · E/config RetrySettings | — | Same | — | NC | measured (defaults); key override not started |
| SET-RETRY-MAX | same | `retry.maxRetries` (3) | same | same | — | Same | — | NC | measured (defaults); override not started |
| SET-RETRY-BASE | same | `retry.baseDelayMs` (2000) | same | same | — | Same | — | NC | measured (defaults); override not started |
| SET-RETRY-CAP | same | `retry.maxAgentDelayMs` (60000) | same | same | — | Same | — | NC | measured (defaults); override not started |
| SET-PROV-TIMEOUT | settings-manager.ts:1034-1040; harness-setup.ts:25-35 | `retry.provider.timeoutMs` (falls back to httpIdleTimeoutMs) | planned: cfg::provider_timeout | Opus · E/config ProviderRetrySettings | — | Same | — | NC | not started |
| SET-PROV-RETRIES | same | `retry.provider.maxRetries` (default 0) | planned: http::retry_policy | same | — | Same | — | NC | not started |
| SET-PROV-DELAY | same | `retry.provider.maxRetryDelayMs` (60000) | planned: http::retry_policy | same | — | Same | — | NC | not started |
| SET-IDLE | http-dispatcher.ts:4,19-35 | `httpIdleTimeoutMs` (300000; 0/"disabled" → max) | planned: http::idle_timeout | Opus · E/config `http_idle_timeout_ms` | — | Same | — | NC | not started |
| SET-COMPACTION-ENABLED | settings-manager.ts:23-33 | `compaction.enabled` (true) | cfg::project_settings_override_global_and_defaults_apply | Opus · CompactionSettings | — | Same | — | NC | measured (parse); behavior not started |
| SET-COMPACTION-RESERVE | same | `compaction.reserveTokens` (16384) | same | same | — | Same | — | NC | measured (parse); behavior not started |
| SET-COMPACTION-KEEP | same | `compaction.keepRecentTokens` (20000) | same | same | — | Same | — | NC | measured (parse); behavior not started |
| SET-COMPACTION-OVERRIDES | settings-manager.ts:914-960 | `compaction.modelOverrides{"p/m":{reserveTokens, keepRecentTokens}}` | planned: cfg::compaction_model_overrides | Opus · E/config | — | Whether durable getter applies overrides: confirm | — | PA | not started |
| SET-STEERING | settings-manager.ts:830-842 | `steeringMode` (`one-at-a-time`) | planned: rt::steer_all | Opus · QueueDelivery | — | Same | — | NC | not started |
| SET-FOLLOWUP | same | `followUpMode` (`one-at-a-time`) | planned: rt::follow_up_all | Opus · QueueDelivery | — | Same | — | NC | not started |
| SET-SKILLS | skills.ts:477-503 | `skills` paths resolved against cwd | planned: ctx::skill_discovery_durable | Opus · E/config `skills` | — | Same | — | NC | not started (in progress) |
| SET-DEFAULT-MODEL | settings defaultProvider/defaultModel | Initial root model; written by model select (global default) | planned: rt::default_model_from_settings | Opus · E/config + E/runtime/models.rs | import | Same initial model; write-back decision D-04 | — | NC | not started |
| SET-DEFAULT-THINKING | settings defaultThinkingLevel | Initial thinking level | planned: rt::default_thinking_from_settings | Opus · same | import | Same | — | NC | not started |
| SET-DEVICEID | settings-manager.ts:1176-1181 | `deviceId` (global only) for OpenAI ChatGPT sign-in | see ONB-DEVICEID | Opus · E/config | import | Same | — | NC | not started |
| SET-HTTPPROXY | main.ts:594,871 (global only) | `httpProxy` sets HTTP(S)_PROXY if unset | planned: http::proxy_env | Opus · E/config `http_proxy` | import | Same | — | NC | not started |
| SET-TRANSPORT | settings `transport` | `auto`, `sse` or `websocket` (Codex) | planned: prov::codex_websocket_fallback | Opus · E/config | import | Same | openai-codex | NC | not started |
| SET-WS-TIMEOUT | settings `websocketConnectTimeoutMs` | Codex WebSocket connect timeout | planned: same | Opus · E/config | import | Same | openai-codex | NC | not started |
| SET-THINKING-BUDGETS | settings `thinkingBudgets.*` | Budget overrides for budget-thinking APIs | planned: prov::thinking_budgets_override | Opus · E/config | import | Same budgets in body | anthropic, google, bedrock, completions | NC | not started |
| SET-TELEMETRY | settings `enableInstallTelemetry`; `PI_TELEMETRY` | Gates provider attribution headers | see ADP-ATTRIBUTION | Opus · E/config | import | — | — | PA | not started |
| SET-SHELLPATH | settings `shellPath` | Custom shell for bash (A-tools §1.4 says not set on the Pipkin path) | planned: tools::shell_path_setting | Opus · E/config `shell_path` | import | Confirm whether durable path honors it | — | PA | not started |
| SET-ENABLED-MODELS | settings `enabledModels` | Model scoping | see PCFG-14 | Opus · E/config `enabled_models` | import | — | — | PA | not started |
| SET-CACHEWARMING | settings `cacheWarming` (global) | Cache warming (`ca/core/cache-warmer.ts`) | planned: audit reachability on durable path | Opus | — | — | — | PA | not started |
| SET-TUI-KEYS | settings terminal/tui/theme/editor keys | Terminal presentation settings | n/a | n/a | n/a | n/a | n/a | NP (terminal-only, A-prompt §7.4) | not started |
| ENV-OFFLINE | `PI_OFFLINE` | Disables network catalog refresh | see PCFG-11 | Opus · E/config Env | — | — | — | PA | not started |
| ENV-CACHE-RETENTION | `PI_CACHE_RETENTION=long` | Long prompt-cache TTL across adapters | planned: prov::cache_retention_long | Opus · E/provider | env | Same cache fields | caching adapters | NC | not started (in progress) |
| ENV-OAUTH-HOST | `PI_OAUTH_CALLBACK_HOST`, `KIMI_OAUTH_HOST`/`KIMI_CODE_OAUTH_HOST`, `PI_RADIUS_GATEWAY` | OAuth/gateway host overrides | planned: auth::host_overrides | Opus · E/auth (in progress) | env | Same | — | NC | not started (in progress) |
| ENV-AGENT-DIR | `PI_CODING_AGENT_DIR`; `--pi-agent-dir` | Legacy agent directory location | planned: imp::discovers_agent_dir | Opus · E/import (in progress) | importer source discovery | Same discovered source | — | NC | not started (in progress) |

## 16. Desktop workflows (P05, P06, P07, P16–P19)

### 16.1 Workflow tests in `App/adapters/pi/e2e.rs`

Each test is a reference workflow against the real Pi engine (ignored unless an engine is
configured). Target: parameterize the harness by backend (plan §8 "Dual-backend rollout") so the
same assertions run against the native backend. Owner for all rows: **Opus ·
`App/adapters/native` (planned; desktop backend in progress by a concurrent worker)**. Migration
for all rows: desktop DB/journal unchanged; conversation ↔ session id mapping by the importer.
Evidence "Pi reference exists" means the test exists in the tree; it was not run for this ledger.

| ID | Test (e2e::) | Baseline behavior | Native fixture | Differential assertion | Coverage | Disp. | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- |
| E2E-01 | the_engine_starts_serves_and_stops_without_leaving_processes | Owned engine starts, serves, stops; no leftover processes | same test, native backend | No owned processes after quit | Linux, macOS | NC | not started (Pi reference exists) |
| E2E-02 | workspace_scan_failure_keeps_the_last_diff_and_retry_recovers_without_a_prompt | Scan failure keeps last diff; retry recovers without sending a prompt | same | Same | Linux | NC | not started (Pi reference exists) |
| E2E-03 | a_real_run_edits_files_shows_output_and_diff_and_reopens_the_same_history | Edit/write/bash run, output, diff, reopen same history | same | Same files, diff, history | Linux | NC | not started (Pi reference exists) |
| E2E-04 | a_dropped_acknowledgment_then_a_relaunch_never_runs_the_prompt_twice | Lost ack + relaunch → lookup, no double run | same | One admission | Linux | NC | not started (Pi reference exists) |
| E2E-05 | a_lost_prompt_is_proven_unsent_and_only_an_explicit_retry_sends_it | Unsent prompt detected via lookup; only explicit retry sends | same | Same | Linux | NC | not started (Pi reference exists) |
| E2E-06 | the_app_owns_the_engine_restarts_it_after_a_crash_and_stops_it_on_quit | Supervisor restart after crash; stop on quit | same (owned child) | Same | Linux, macOS | NC | not started (Pi reference exists) |
| E2E-07 | an_engine_that_cannot_start_is_retried_a_few_times_then_reported_with_its_output | Bounded start retries then report with output | same | Same | Linux | NC | not started (Pi reference exists) |
| E2E-08 | an_engine_the_app_did_not_start_is_never_stopped_by_it | External engine never stopped by the app | n/a for pipe-only native; P19 endpoint | Same once external attach exists | Linux | NC | not started (Pi reference exists) |
| E2E-09 | stopping_a_run_waits_for_the_engine_and_the_conversation_takes_the_next_prompt | Stopping until engine confirms; next prompt accepted | same | Same | Linux | NC | not started (Pi reference exists) |
| E2E-10 | a_lost_stop_request_remains_unsettled_and_can_be_checked_then_retried_without_a_prompt | Lost Stop stays unsettled; check + retry without prompt | same | Same | Linux | NC | not started (Pi reference exists) |
| E2E-11 | stopping_a_prompt_flushes_its_instruction_before_a_new_prompt | Stopped instruction excluded from next prompt's context | same + request capture | Same next request | Linux | NC | not started (Pi reference exists) |
| E2E-12 | stopping_a_prompt_flushes_its_instruction_before_a_replacement_goal | Same for replacement goal | same | Same | Linux | NC | not started (Pi reference exists) |
| E2E-13 | a_follow_up_waits_in_the_engines_queue_then_runs_by_itself | Follow-up queued then runs as its own run | same | Same order | Linux | NC | not started (Pi reference exists) |
| E2E-14 | a_queued_input_removed_before_it_runs_never_reaches_the_model | Withdrawn queued input never reaches model | same | Same | Linux | NC | not started (Pi reference exists) |
| E2E-15 | a_steer_reaches_the_next_model_turn_of_the_active_run | Steer joins next turn of active run | same | Same next request | Linux | NC | not started (Pi reference exists) |
| E2E-16 | a_run_and_its_queue_are_picked_up_again_after_the_app_restarts | Run + queue restored after app restart | same | Same | Linux | NC | not started (Pi reference exists) |
| E2E-17 | a_run_started_elsewhere_is_shown_and_can_be_stopped | Run started by another client shown and stoppable | P19 multi-client (native endpoint not designed) | Same once multi-client exists | Linux | PA | not started (Pi reference exists) |
| E2E-18 | switching_conversations_keeps_each_ones_run_and_settles_what_ended_while_away | Background runs continue; settlements while away | same | Same | Linux | NC | not started (Pi reference exists) |
| E2E-19 | attachments_reach_the_model_as_text_and_image_and_come_back_as_chips | Text/image attachments reach model; chips on reload | same | Same request content | Linux | NC | not started (Pi reference exists) |
| E2E-20 | a_draft_with_attachments_survives_a_restart_and_a_missing_file_blocks_sending | Draft attachments persist; missing file blocks send | same (desktop-only) | Same | Linux | NE | not started (Pi reference exists) |
| E2E-21 | a_follow_up_whose_acknowledgment_is_lost_is_admitted_exactly_once | Lost follow-up ack → one admission | same | One admission | Linux | NC | not started (Pi reference exists) |
| E2E-22 | a_follow_up_that_never_reached_the_engine_is_sent_again_under_its_key_once | Unsent follow-up resent once under same key | same | Same | Linux | NC | not started (Pi reference exists) |
| E2E-23 | a_lost_follow_up_acknowledgment_then_a_kill_and_relaunch_runs_it_once | Lost ack + kill + relaunch → one run | same | Same | Linux | NC | not started (Pi reference exists) |
| E2E-24 | losing_the_connection_mid_run_reconnects_and_the_run_still_settles | Reconnect mid-run; run settles | same (pipe loss = engine restart) | Same settlement | Linux | NC | not started (Pi reference exists) |
| E2E-25 | a_prompt_that_cannot_be_journaled_is_not_sent_and_sends_once_storage_recovers | Journal failure blocks send until storage recovers | same (desktop-only) | Same | Linux | NE | not started (Pi reference exists) |
| E2E-26 | an_engine_crash_mid_run_is_recovered_by_the_engine_and_the_prompt_is_not_duplicated | Engine crash mid-run resumes, no duplicate prompt | same | Same | Linux | NC | not started (Pi reference exists) |
| E2E-27 | a_provider_configured_after_start_appears_after_a_refresh_and_can_then_be_used | models.json change visible after refresh | same | Same | Linux | NC | not started (Pi reference exists) |
| E2E-28 | choosing_effort_is_confirmed_by_pi_and_restored_per_conversation | Effort confirmed and restored per conversation | same | Same | Linux | NC | not started (Pi reference exists) |
| E2E-29 | history_before_a_compaction_is_paged_in_once_and_in_order | Pre-compaction history paged once, in order | same (needs compaction) | Same | Linux | NC | not started (Pi reference exists) |
| E2E-30 | the_complete_output_of_a_long_tool_result_is_copied_and_saved_not_the_preview | Copy/save full retained output | same | Same bytes | Linux | NC | not started (Pi reference exists) |
| E2E-31 | a_changed_file_opens_in_the_editor_and_a_terminal_in_the_project_from_a_real_run | Editor/terminal actions from Changes | same (desktop services) | Same argv | Linux | NE | not started (Pi reference exists) |
| E2E-32 | a_saved_conversation_is_readable_and_searchable_without_the_engine_and_the_engine_takes_over_again | Offline read/search from cache; engine takes over | same | Same | Linux | NC | not started (Pi reference exists) |
| E2E-33 | the_engines_extension_question_service_is_there_and_refuses_an_answer_to_nothing | Question service present; refuses answer to unknown id | same | Same refusal | Linux | BL / LB | not started (Pi reference exists) |
| E2E-34 | an_extension_in_the_engine_asks_three_questions_and_the_answers_reach_it | Facet asks three questions; answers reach it | needs facet port (ADR 0006) | Same | Linux | BL / LB | not started (Pi reference exists) |
| E2E-35 | declining_ends_the_question_and_a_lost_connection_does_not_leave_it_answerable | Decline ends question; lost connection not answerable | needs facet port | Same | Linux | BL / LB | not started (Pi reference exists) |
| E2E-36 | seed_a_profile_to_look_at | Fixture helper that seeds a profile for manual inspection (not a workflow assertion) | native equivalent helper | n/a | Linux | NC | not started (Pi reference exists) |
| E2E-37 | many_prompts_in_one_conversation_do_not_grow_memory_or_open_files | 1500-prompt soak, RSS/fd growth bound | same (perf, RE-22) | No sustained growth; record absolute values | Linux | NC | not started (Pi reference exists) |

### 16.2 Real-engine protocol tests in `App/adapters/pi/real_pi.rs`

| ID | Test (rpi::) | Baseline behavior | Native fixture | Owner · module | Differential assertion | Coverage | Disp. | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| RPI-01 | real_server_speaks_the_protocol_pipkin_implements | Handshake/catalogue/services Pipkin requires exist | planned: host::hello_and_capabilities | Opus · P/api.rs | Required capabilities advertised | Linux | NC | not started (Pi reference exists) |
| RPI-02 | real_server_provider_auth_boundary | Provider-auth service boundary (providers, refusals) | planned: auth::boundary | Opus · E/auth (in progress) | Same refusals; anthropic part reserved | Linux | NC | not started (in progress) |
| RPI-03 | real_server_session_lifecycle_through_the_adapter | Create/attach/prompt lifecycle | planned: native adapter test | Opus · App/adapters/native (planned) | Same lifecycle | Linux | NC | not started |
| RPI-04 | real_server_switching_between_sessions | Switching sessions keeps state | planned | same | Same | Linux | NC | not started |
| RPI-05 | real_server_subagent_service_enforces_session_setting | Disabled subagents refuse new calls | planned: rt::disabled_subagents_still_recover | Opus · E/runtime | Same | Linux | NC | not started |
| RPI-06 | real_adapter_tracks_and_controls_subagents_per_attached_session | Adapter tracks children per session | planned | Opus · App/adapters/native | Same | Linux | NC | not started |

### 16.3 Other desktop capabilities and existing flags (P17–P19)

| ID | Source | Baseline behavior | Fixture / test | Owner · module | Migration mapping | Differential assertion | Coverage | Disp. | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| DSK-OFFLINE | App/cache.rs, App/storage.rs | Offline reading/search from desktop cache | E2E-32 | Opus · desktop (unchanged) | cache keyed by conversation id (mapped) | Same | desktop | NE | not started |
| DSK-DRAFTS | App/storage.rs (schema 7) | Drafts, preferences, goals, journal, prompt history | existing App storage tests | Opus · desktop (unchanged) | unchanged | Same | desktop | NE | not started |
| DSK-CHANGES | App/adapters/pi/session_changes.rs, workspace.rs | Session-scoped Changes from recorded successful edit/write, Git diff, retirement after commit, unrelated files excluded | existing App tests + E2E-03 | Opus · native typed mutation events + App | n/a | Same Changes set and Git effects | desktop | NC | not started |
| DSK-EDITOR-TERMINAL | App/launch.rs | Editor/terminal launches via argv | E2E-31 | Opus · desktop (unchanged) | n/a | Same | desktop | NE | not started |
| DSK-DIAG | App/diagnostics.rs; docs/support.md | `--diagnose`, `--probe`, palette Copy diagnostics (metadata allowlist) | existing App tests | Opus · App + native engine identity | n/a | Native engine identity reported; no Node checks | desktop | NC | not started |
| DSK-DEMO | App/demo/*; AGENTS.md | Demo mode with "Demo · simulated agent" marker | existing demo tests | Opus · desktop (unchanged) | n/a | Marker kept | desktop | NE | not started |
| FLAG-PI-REPO | App/controller.rs | `--pi-repo <engine>` launches a source/installed Pi engine | n/a | Pi adapter (kept during transition) | n/a | Native selector `--engine` is new (plan §8) | — | LB | not started |
| FLAG-PI-DIR | App/controller.rs; docs/support.md | `--pi-dir` selects owned profile or attaches to running server | n/a | Pi adapter; native P19 endpoint (not designed) | n/a | External attach equivalent required | — | BL | not started |
| FLAG-PI-SERVER-ID | App/controller.rs | `--pi-server-id` external server | n/a | same | n/a | same | — | BL | not started |
| FLAG-PI-AGENT-DIR | App/controller.rs | `--pi-agent-dir` agent dir (credentials/config) | planned: imp::agent_dir_flag | Opus · E/import source | importer source | Native uses own config root; flag maps to import source | — | NC | not started |
| FLAG-PI-EXTENSION | App/controller.rs:139-143,301 | `--pi-extension <path>` → `pi server -e` | E2E-34 | see EXT-FACET-SESSION | profile recorded | — | — | BL / LB | not started |
| FLAG-DATA-DIR | docs/support.md | `--data-dir`/`PIPKIN_DATA` isolate desktop data (not engine profile) | existing tests | Opus · App + EngineConfig.data_dir | n/a | Native: `--data-dir` isolates engine state too (improvement, plan §6) | — | NC | not started |
| FLAG-PROJECT | App/controller.rs | `--project` initial project | existing | desktop | n/a | Same | — | NE | not started |
| FLAG-EDITOR-TERMINAL | App/controller.rs | `--editor`, `--terminal` | existing | desktop | n/a | Same | — | NE | not started |
| FLAG-DEMO-SPEED | App/controller.rs | `--demo <scenario>`, `--speed` | existing | desktop | n/a | Same | — | NE | not started |
| FLAG-VERSION | App/install.rs | `--version` (protocol 8, schema 7 reported) | existing | Opus · App | n/a | Reports native protocol/schema too | — | NC | not started |
| MULTI-CLIENT | A-svc §4.4; e2e E2E-17 | Several clients attached; per-connection auth; first responder wins questions | E2E-17 | Opus · P19 endpoint (not designed) | n/a | Same multi-client outcomes | — | PA | not started |

## 17. Import and migration (plan §6)

Owner for all rows: **Opus · `E/import` (in progress)**. Source: `~/.pi/agent/experimental/sessions/<id>/{meta.json, session.sqlite}`
(A-prompt §7.3), `~/.pi/agent/{auth,models,settings}.json`, `~/.pi/server/*plugin-packages*.json`,
desktop `pipkin.sqlite3`.

| ID | Source | Baseline data | Fixture / test | Migration mapping | Differential assertion | Disp. | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- |
| IMP-DISCOVER | ce/session-catalog.ts | Session dirs with valid meta.json; invalid skipped | planned: imp::discovery_report | catalog → report (format/version, deps, active work) without mutation | Same session list | NC | not started (in progress) |
| IMP-LOCKED | ce/session-worker.ts:518-523 | Live worker holds proper-lockfile | planned: imp::refuses_live_source | refuse while source locked | Refusal | NC | not started (in progress) |
| IMP-BACKUP | plan §6 step 2 | WAL-mode session.sqlite | planned: imp::backup_api_copy | SQLite backup API, permissions kept, integrity verified | Integrity ok | NC | not started (in progress) |
| IMP-ENTRIES | D/entries.ts | `pi.user/assistant/system/tool-result/reset/compaction/cancelled-input` + app kinds | planned: imp::entry_kinds_round_trip | kinds → native entries; head markers; omit edits → retired/edits; unknown kinds preserved verbatim and block | Visible history + next request equal | NC | not started (in progress) |
| IMP-DOCS | D/documents.ts | `pi.agent`, `pi.usage`, `pi.provider`, `pi.inbox`, `pi.live` base+deltas | planned: imp::documents | agent → session cols/kv; usage → usage; provider.sessionId → kv; inbox → queued requests (quiesced); live not imported | Same config/usage/session id | NC | not started (in progress) |
| IMP-SUBMISSIONS | D/harness/submissions.ts | Submissions with request ids, statuses, reasons | planned: imp::request_keys_preserved | → requests with same keys/outcomes; unconfirmed journal rows never become new prompts | Lookup equal for every key | NC | not started (in progress) |
| IMP-TASKS | D/scheduler.ts | Task graph incl. in-flight tool/generation/compaction phases | planned: imp::inflight_tasks_block_or_map | quiesce; interrupted tools → interrupted results; unsupported → stays on Pi | No new work executed by import | NC | not started (in progress) |
| IMP-CHILDREN | conversations owner_task | Subagent children and parent call mapping, enable state | planned: imp::subagent_graph | → subagent_tasks + sessions.parent/root | Same directory + continuation | NC | not started (in progress) |
| IMP-PLUGIN-STATE | ce/plugins/package.ts | Server/session plugin profiles; facet state (none durable in baseline) | planned: imp::plugin_selection_recorded | profile → kv; conversation stays Pi-owned if selection non-empty | Not silently activated | BL | not started |
| IMP-AUTH | auth.json | Credentials | planned: imp::auth_explicit | explicit per-provider copy; anthropic subscription preserved, not activated | Same credentials; no sync | NC (RC for anthropic subscription) | not started (in progress) |
| IMP-CONFIG | models.json, settings.json, AGENTS.md, skills | Config/resources | planned: imp::config_explicit | explicit copy to config root | Same effective config | NC | not started (in progress) |
| IMP-DESKTOP-IDS | App/storage.rs | Desktop conversation/item refs, drafts, goals, journal | planned: imp::desktop_refs_follow | legacy→native id map; desktop rows untouched | Drafts/goals still attached | NC | not started (in progress) |
| IMP-OWNERSHIP | plan §6 step 7 | Authoritative backend per conversation | planned: imp::atomic_ownership_transfer | `import_map` + desktop backend field recorded atomically after validation | Exactly one writer | NC | not started (in progress) |
| IMP-IDEMPOTENT | plan §6 | Rerun/interrupted import | planned: imp::rerun_is_idempotent | resumable manifest | Second run no-op; source untouched | NC | not started (in progress) |
| IMP-ROLLBACK | plan §6 | Rollback to Pi after native work | planned: imp::native_only_history_labelled | native-only history labelled; older code refuses newer schema | Pi source untouched | NC | not started |

## 18. Packaging, platform and lifecycle (P20)

| ID | Source | Baseline behavior | Fixture / test | Owner · module | Differential assertion | Coverage | Disp. | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| PKG-ENGINE-STAGE | scripts/build-engine.sh | Clean pinned Pi + overlays + model data + node_modules, `engine.json` identity | rpi:: + verify-install | Pi (reference) | Reference identity frozen (progress record) | Linux | NE (reference) | not started |
| PKG-NO-NODE | plan §7 | Native build/runtime without Node/npm/Pi | planned: CI native-only build job | Opus · RE-23 | `cargo build` from clean tree without Node | Linux, macOS | NC | not started |
| PKG-WORKER-ENTRY | ADR 0001 | Private worker mode of the `pipkin` executable | planned: app::worker_mode_starts_before_gpui | Opus · App main + E/host::run_stdio | Owned child starts/stops | Linux, macOS | NC | not started |
| PKG-MODEL-DATA | pkg/engine-model-data.md | Pinned model data with provenance | mdl::embedded_files_match_the_pinned_manifest | Opus · E/provider/data.rs | Manifest hash equal (b92d631b…) | all | NE | measured: native unit pass |
| PKG-BUILDINFO | scripts/write-build-info.mjs | Build info needs Node | planned: Rust/shell replacement | Opus · RE-23 | Same fields without Node | Linux, macOS | NC | not started |
| PKG-INSTALL | packaging/install.sh; scripts/test-install.sh | Versioned install/upgrade/rollback/uninstall; offline probe | existing install tests | Opus · RE-23 | Same with native engine | Linux | NC | not started |
| PKG-PKGBUILD | packaging/PKGBUILD | Arch package (`!lto`, deps) | makepkg | Opus · RE-23 | Package without Node runtime dir | Arch | NC | not started |
| PKG-RUNTIME-NODE | packaging/node-runtime.json; runtime_tests.rs | Private checksum-pinned Node 22.23.3 | App runtime tests (3) | Opus · RE-23/RE-25 | Removed from native package | Linux | NC | not started |
| PKG-NOTICES | packaging/upstream-notices*; docs/bundled-licenses.md | Third-party notices, upstream attribution | scripts/test-package-docs.py | Opus · RE-23 | Attribution retained | all | NC | not started |
| PKG-SIZE | docs/packaging.md (453 MiB historical) | Package size | planned: RE-02 remeasure | Opus · RE-22 | ≥40% installed / ≥25% archive reduction (provisional) | Linux | NC | not started |
| PKG-SCHEMA-REFUSAL | App/install.rs; E/store/schema.rs | App DB schema 7 and engine protocol 8 refusal; native schema 1/protocol 1 refusal | st::a_newer_schema_is_refused_untouched_and_the_lock_is_exclusive | Opus | Newer refused untouched | all | NC | measured (native store only) |
| PKG-CI | .github/workflows/qualification.yml | Locked Rust checks, clean pinned engine, scripted-provider tests, installer tests | CI | Opus · RE-23 | Native jobs non-ignored | Ubuntu headless | NC | not started |
| PLAT-LINUX | docs/platforms.md | x86_64 Arch/Omarchy Hyprland/Wayland qualification build | native gates | Opus | Same workflows natively | Linux | NC | not started |
| PLAT-X11 | docs/platforms.md | Builds/starts under Xwayland | observed historically | desktop | Unchanged | X11 | NE | not started |
| PLAT-MACOS | docs/platforms.md; App/adapters/pi/macos_processes.rs | Experimental macOS build; engine smoke checks | CI run 37482847877 (historical) | Opus | Native engine lifecycle tests on macOS | macOS | NC | not started |
| PLAT-WINDOWS | docs/platforms.md | Not supported | n/a | n/a | n/a | Windows | NP (docs/platforms.md: "Not supported") | not started |
| PLAT-PROCESS-TREE | env/node.ts:278-303 | Bash process group SIGKILL; owned engine reaping | planned: tools::descendants_reaped | Opus · E/tools + supervisor | No orphaned owned children | Linux, macOS | NC | not started |
| PLAT-PERF | plan §7 | Readiness, RSS, streaming latency, Stop latency, soak | planned: RE-02/RE-22 harness | Opus | Targets measured vs remeasured Pi | Linux | NC | not started |

## 19. Known reference defects (recorded, not silently fixed)

Each defect keeps the intended capability; any native correction needs its own regression test
and an entry under §20 or an owner decision under §21.

| ID | Where | Defect | Native handling |
| --- | --- | --- | --- |
| K-01 | ca/core/skills.ts:452-503; ce/durable/prompt.ts:34; A-prompt §2.4 | The durable server ignores `.agents/skills` (cwd/ancestors and `~/.agents/skills`) and package-installed skills, which the classic loader discovers; and it loads `<cwd>/.pi/skills` without the trust gate the classic loader applies | Reproduce the durable set for parity (PRM-SKILLS-DISCOVERY). Adding `.agents/skills` is an owner decision, not a silent fix |
| K-02 | ca/core/tools/bash.ts:45-48,194-208; A-tools §1.4 | The rules section tells the model "You can inspect PI_* environment variables for current model and session details", but the durable bash tool never injects `PI_*` variables (`exposeSessionEnvironment` unset) | Record; decide whether to drop the bullet (prompt change) or inject variables (env change). Either is a behavior change needing a test |
| K-03 | env/node.ts:776-790,1194-1218 | Bash full-output spill files in `$TMPDIR` are never deleted; unbounded temp growth | Native retains output in SQLite with a quota (I-04); spill paths in imported history stay as text |
| K-04 | D/harness/submissions.ts:160-219 | Same request key with different content silently returns the original submission | Native returns `Conflict` (I-01) |
| K-05 | ce/plugins/package.ts:27-28; App/adapters/pi/engine.rs:183-185 | Plugin selection is sticky: omitting `--pi-extension` restores the last list and Pipkin never sends an empty list, so a removed extension keeps loading | Record; native selection must be explicit and clearable; import reports the sticky profile |
| K-06 | ca/core/model-config.ts (schema) vs types; A-prov §5 | `chatTemplateKwargs` `$var` schema allows only `thinking.enabled` and `thinking.effort` while types also allow `thinking.budget` | Accept the schema's set; record |
| K-07 | A-tools §8.3 | The server path performs no image size/count/MIME validation; images stored verbatim as base64 inside entry JSON | Native enforces explicit limits (D-03) |
| K-08 | svc/subagents-provider.ts:101-145 | Every `view/open/page` rescans the whole child list; 64 live mounts per worker shared by all clients, never released before worker retirement | Native pages indexed `subagent_tasks`; no mount cap (I-06) |
| K-09 | packaging/pi-subagents/README.md:17; tool.ts:163-164 | Subagent recovery contract is metadata-only (not a code signature); identical-contract code changes are not proven safe | Native ports the contract check (SUB-RECOVERY-CONTRACT); provenance proposal I-05 |
| K-10 | durable/README.md:563 | `synchronous=NORMAL`: newest commit may be lost on power failure | Native `FULL` (I-02); power loss still not claimed |
| K-11 | docs/extensions.md | Says the contract lives in `../pi-fork/pi` and calls `--pi-extension` packages extensions; shipped engine is the pinned archive + overlays and loads facet packages (A-ext §5.6) | Documentation fix outside this ledger |

## 20. Intentional improvements (each needs its own regression test)

| ID | Improvement | Row(s) | Test |
| --- | --- | --- | --- |
| I-01 | Same key + different payload → `Conflict` instead of silently returning the original | ADM-KEY | st::same_key_same_payload_is_the_original_and_changed_payload_conflicts (measured) |
| I-02 | `synchronous=FULL` instead of `NORMAL` | DUR-PRAGMAS | st::acknowledged_writes_survive_reopen (measured; not a power-loss proof) |
| I-03 | Stop retirement via an entry `retired` flag instead of `pi.cancelled-input` omit-edit entries (same observable context) | STOP-RETIRE | rt::stop_settles_aborted_and_retires_the_instruction_from_the_next_context (measured; differential pending) |
| I-04 | Retained tool output stored in SQLite (`tool_outputs`, ≤16 MiB per call) with honest dropped-byte counts, instead of never-cleaned temp files | OUT-RETAINED, TOOL-BASH-SPILL | st::tool_output_is_retained_with_honest_drop_counts (measured) |
| I-05 | (Proposed) record engine provenance with the subagent recovery contract | SUB-RECOVERY-CONTRACT | not started; needs owner sign-off |
| I-06 | Child history paged from indexed tasks; no 64-mount cap; inspector can go beyond 100-row/80-entry presentation bounds | SUB-LIVE, SUB-PAGE, SUB-UI-* | not started |
| I-07 | One owned engine child, no global server discovery or process sweep | SVC-PROC-WORKERS, E2E-01/06 | not started |
| I-08 | Explicit byte/count limits on inputs, frames, queue (refusal, never truncation) | ADM-BOUNDS, Q-BOUND, XPORT-FRAME | rt::inputs_are_bounded_before_anything_is_recorded; frm:: tests (measured) |
| I-09 | `--data-dir` isolates engine state too | FLAG-DATA-DIR | not started |
| I-10 | `CreateSession` idempotent under a request key | SVC-MGMT-CREATE | st::create_is_idempotent_under_its_key (measured) |
| I-11 | Typed successful-mutation events for Changes instead of provider-shaped JSON inference | DSK-CHANGES | not started |

## 21. Open owner decisions and recorded differences

| ID | Question | Rows |
| --- | --- | --- |
| D-01 | The `docs` prompt section points the model at Pi's README/docs/examples paths. What does the native prompt say (Pipkin docs, nothing, or Pi docs shipped as data)? | PRM-DOCS |
| D-02 | Preamble text names "pi"; keep verbatim for cache/behavior parity or rename? | PRM-PREAMBLE |
| D-03 | Native input limits (2 MiB text, 8 images × 8 MiB, queue 64) where the reference had none beyond frame size | ADM-BOUNDS, Q-BOUND, ATT-IMAGES |
| D-04 | Reference model selection also rewrites the global default model in `settings.json`; keep this cross-session side effect? | SVC-MODELS-SELECT, SET-DEFAULT-MODEL |
| D-05 | Identity strings sent to providers (`User-Agent: pi (...)`, `originator: pi`, attribution headers) | ADP-UA, ADP-ATTRIBUTION, ADP-CODEX-SSE |
| D-06 | Live remote catalog refresh from `pi.dev` (runtime, not release input) | PCFG-11, ENV-OFFLINE |
| D-07 | Interactive login for OAuth providers that Pipkin's onboarding never offered (Copilot, OpenRouter, Kimi, Meta, Radius, xAI, OpenAI ChatGPT) | OA-* rows marked PA |
| D-08 | When edits to AGENTS.md/skills take effect (reference: per worker lifetime) | PRM-AGENTS-CACHE |
| D-09 | Reference ids 46 vs 43 provider ids found (see §4 count note) | §4 |
