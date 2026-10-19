# Claude CLI provider review — 2026-10-17

**Historical review baseline:** this records the findings before the subsequently authorized
provider repairs. Line references and defect characterizations below refer to that baseline;
current implementation/verification is recorded in the **CLI provider repairs** section of
[the scorecard](scorecard.md). This review alone is not a compatibility qualification.
Previous source changes and the
pending orchestration-dashboard plan are preserved. No installed CLI execution, live prompts,
credential access, prompt replay, profile mutation, installation or commit.

## Evidence

Read-only, bounded queries of Pipkin's retained receipts show:

- Receipt 5: fresh turn, completed, clean result and exit.
- Receipt 6: resumed turn, Unknown, two committed projection events, zero projected text bytes;
  `Sequence`, `content_block_start`, `observer_failure`, no terminal observation. Input was
  delivered and cleanup reaped/drained the process. Bindings record CLI 2.1.301.

Raw stream frames were not retained. The exact predicate taken in the failed live run cannot
be proved from that receipt alone. However, the documented CLI sequence reproduces the same
error **and** the two-event projection cursor in an offline characterization.

Official references fetched 2026-10-17:

- <https://code.claude.com/docs/en/agent-sdk/streaming-output.md>, "Message flow": one assistant
  record per completed content block; records from the same API response share a message ID;
  each carries its own block and arrives before that block's `content_block_stop`.
- <https://code.claude.com/docs/en/agent-sdk/how-the-agent-loop-works.md>, "Message types":
  a result ends the agent loop, but trailing system events such as `prompt_suggestion` can follow.
- <https://raw.githubusercontent.com/anthropics/claude-agent-sdk-python/main/src/claude_agent_sdk/_internal/message_parser.py>:
  the official wrapper handles message types separately rather than treating every assistant
  record as a complete cumulative API-message snapshot.

Job253 ran three temporary, pure-decoder characterization tests. All three confirmed the
undesired current behavior; passing them is **not** compatibility success. The harness remains
in owned `/home/jason/.pg.6vpFzV/cli_stream_review.rs`; its temporary integration-test copy was
removed. No production source was changed for this review.

## Findings, ordered by impact

### P1 — Normal per-block assistant records are incorrectly treated as whole-message finals

`crates/pipkin-engine/src/execution/claude_cli/events.rs:667-741` treats assistant content as a
complete cumulative snapshot, enumerates its blocks from zero and sets `MessageState.finalized`.
`events.rs:779-783` rejects the next block start when that flag is set.

A normal sequence is:

```text
message_start
content_block_start(0, thinking)
assistant(complete thinking block, message ID m)
content_block_stop(0)
content_block_start(1, text)  <-- Pipkin rejects this
assistant(complete text block, same message ID m)
content_block_stop(1)
message_stop
result
```

Job253 reproduced the rejection at the marked step. This is an incorrect protocol model, not
Claude behaving unpredictably. Simply removing the `finalized` check is insufficient: subsequent
single-block assistant records are also incorrectly mapped to index zero. Reconciliation must
identify each block, not overwrite the preceding thinking/text/tool block. Store projection's
`final_blocks` rules in `store/external/prompts.rs:474-542` must agree with the revised decoder.

### P1 — Presentation failure is wired directly to process termination and permanent fencing

`runtime/external.rs:628-645` converts any display decoder/sink error into observer failure.
`execution/claude_cli/process.rs:337-365` then signals the CLI process group.
`store/external/prompts.rs:626-641,729` requires perfect projection for completion and invalidates
the session binding otherwise.

This creates a self-inflicted failure chain: optional presentation cannot be understood, so the
wrapper prevents the CLI from finishing, loses its completion evidence, then makes the entire
conversation unusable. Actual identity/permission/storage failures and cancellation still need
strict handling; an unfamiliar display event must not automatically take that path.

The bounded 128-event projection channel compounds this. `try_send` returns `SinkRejected` when
the consumer lags; the same process-killing path follows. Job253 reproduced decoder poisoning
with a backlogged channel and ordinary text deltas. Boundedness is necessary; a slow display
consumer should cause backpressure or coalescing, not an execution failure.

### P1 — Documented events after a successful result are rejected by both parsers

`execution/claude_cli/events.rs:454-455` rejects every record after an outcome.
`execution/claude_cli/stream.rs:113-119` independently rejects every record after a result.
Job253 confirmed that a successful result followed by a system `prompt_suggestion` is rejected
by both. Therefore changing only the rich display decoder would still leave a second fatal
validator underneath it. A valid result must remain authoritative while allowed trailing
observability is drained; duplicate/conflicting terminal results remain a separate concern.

### P2 — Forward compatibility is implemented as repeated field allowlisting

`events.rs` uses closed `deny_unknown_fields` schemas for stream events and deltas. That caused
the previous `estimated_tokens` failure. Supporting that one field did not address the design.

Unknown presentation metadata should normally be ignored or recorded as a bounded unsupported
observation. Validate the fields actually consumed. Maintain strictness for session identity,
authority-bearing permission messages, resource limits and contradictory completion evidence;
do not apply the same policy indiscriminately to thinking/progress telemetry.

### P2 — Setup-ticket expiry is also the execution-grant lifetime

`runtime/local_cli_setup.rs:23,224-232` uses a five-minute ticket deadline as the grant expiry.
`runtime/external.rs:663-677` stops a running turn when that grant expires. Short-lived setup
freshness has consequently become a provider-session interruption policy. This is separate
from the reported decoder failure, but it contributes directly to the clunky provider lifecycle.

Review this authority policy explicitly: separate setup freshness, connection ownership,
revocable scoped consent, and per-turn checks. Do not silently make grants perpetual as a parser
fix. Per-Write approval and actual executable/account/project identity changes remain distinct.

### P2 — Tests validate the wrong abstraction; selectable provider does not imply tool parity

The multi-block decoder fixture (`events/tests.rs:208-243`) and the new fake Opus fixture
(`runtime/external_tests.rs`, `thinking_records`) emit a combined assistant snapshot after the
blocks, rather than the documented per-block interleaving. They passed while missing a normal
thinking-to-text transition. A large green suite did not establish this contract.

The provider is selectable in the normal picker, but the execution slice is still chat-only or
restricted Write; it cannot transparently perform native plan/tool/queue/subagent operations.
That limitation is currently intentional and documented, not justification for parser failures.
A normal provider experience needs honest capability exposure and shared lifecycle UX, without
pretending the CLI and Pipkin can both own the same tool execution or conversation context.

## Recommended replacement boundary

Keep the installed, unmodified CLI and its normal login. Preserve the normal provider/model
picker. Replace the brittle normalization/lifecycle boundary rather than adding another exception:

1. **One framing/envelope boundary.** Bound input and verify identity once. Route lifecycle,
   permissions, assistant blocks, stream previews and optional telemetry independently. Do not
   leave two competing terminal validators.
2. **Separate execution outcome from presentation quality.** Stream previews are best effort;
   reconcile complete assistant blocks and bounded final-result text. A verified clean result
   with incomplete live display can be completed with a presentation warning. Unknown execution
   still requires missing/contradictory execution evidence, not merely unfamiliar telemetry.
3. **Correct block normalization.** Correlate API message ID, stream block index and SDK envelope
   identity. Cover repeated blocks, empty/omitted thinking, multiple text/tool blocks, and both
   partial and non-partial output. Never duplicate or reinterpret observed CLI tools as native
   tool calls. Tool permissions stay on their explicit authority channel.
4. **Bounded pressure handling.** Coalesce replaceable display state or backpressure the producer;
   retain ordered durable lifecycle/permission events. Never turn a UI backlog into an automatic
   rerun, silent data loss or unnecessary process kill.
5. **Provider-grade state and UX.** Separate connected/ready, running, completed, recoverable
   provider error, display warning and genuinely uncertain execution. Reconnect and trust policy
   should reflect actual invalidation rather than a transient rendering assumption or setup
   ticket age. Model/backend/mode identity and drafts/history remain preserved.
6. **Protocol-based verification.** Replace unrealistic fixtures, then test documented block
   interleaving, unknown telemetry, post-result events, burst/slow consumers, CLI-reported errors,
   cancellation, crashes, permission revocation and fresh/resume/restart. Keep bounded redacted
   sequence diagnostics (event kind/index/state, not raw content) for future investigations.

No recovery shortcut is proposed for the already-interrupted turn. Its old result remains
unconfirmed; a future parser fix must not rewrite it as success or replay it automatically.
Broader execution-authority changes need an explicit reviewed contract, not accidental permission
expansion under the label of compatibility.

## Review verification

- Job253: three offline characterizations confirmed the documented interleaving failure,
  post-result rejection in both decoders, and sink-pressure poisoning. No CLI process launched.
- Job254: unchanged application source passed workspace tests (UI217), strict workspace/
  all-target Clippy, fmt and diff checks. Nonempty owned TMPDIR `/home/jason/.pg.N58u7N`
  retained; exit zero. This green suite does not negate the protocol defects above. The live
  ten-run ledger hash remains `bf794518e35d7f1ce3a50b3058c4191bb9401e568fc645d77e10b0f404cf1f22`.
- No real Opus run, installed UI behavior, broader CLI-version qualification or remediation is
  claimed. Dashboard implementation remains pending and out of scope.

## Follow-up: usable coding versus native agent parity

Receipt8 repeated the legacy routing problem: it resumed `safe-restricted-no-tools-v1`,
completed cleanly, and recorded zero actions. All five saved CLI bindings were legacy chat-only.
`providers.rs` treated the same typed CLI alias as the same route regardless of execution profile;
the picker therefore could not select the coding provider from that historical conversation.
The follow-up fixes that comparison and source guard, adds honest legacy labels and an explicit
**Use coding model** action, and preserves old identities/history/drafts without forwarding work.
The [new three-prompt ledger](../fixtures/claude-cli-probe/unattended-coding-followup.md) records
real2.1.301/Sonnet Read/Write through engine and desktop routes, including fresh/resume. It does
not qualify native orchestration or interaction with a running GPUI window.

The architecture audit establishes why enabling CLI default tools alone is **not** the full
requested native workflow:

- `ExecutionProfile::LocalCliCoding` still denies native plans/goals/jobs/subagents/queues and
  attachments. Readiness must not be used to bypass those capability guards.
- Native `ProviderRequest` supplies system context, ordered messages and tool specifications to
  Pipkin's agent loop. The external CLI path instead runs a separate CLI-owned loop and context.
- Native tools execute under `ToolCtx`/`ToolHost`, with Pipkin's durable progress, plan, job,
  question and subagent services. Structured CLI activity is observational; the transcript
  adapter deliberately renders it without native tool execution/retry authority.

Full native parity remains **unimplemented**. Its acceptance contract must include:

1. An explicit structured tool request/response channel (for example a narrowly scoped MCP
   bridge), independently authorized by the owning conversation/generation/op. Neither printed
   XML nor stream observations can invoke native tools. CLI-owned and Pipkin-owned execution
   must never both execute one call.
2. The same native tool schemas, argument validation, output bounds, durable call identities,
   plan/job/question services and cancellation semantics. A lost tool reply with possible
   effects stays uncertain; transport reconnect must not repeat it.
3. Defined context ownership: instruction/skill snapshots, conversation history, compaction,
   root/child relationships and tool results. Serializing the whole transcript as a new user
   prompt is not equivalent to preserving message roles or native context.
4. CLI-only inference for root and child models through the installed CLI's normal login,
   with explicit model/policy identities and no native/API billing fallback. No implicit
   migration of existing bindings or uncertain receipts.
5. End-to-end tests of real native plans, jobs, child completion, queued follow-ups, steering,
   questions, Stop/restart and stale replies before advertising those capabilities. Existing
   fake CLI tests and the bounded Read/Write qualification do not satisfy these criteria.

This contract is not a parity claim. An inactive authenticated helper/native-tool driver now
has offline atomic tool/root-journal, immutable storage-binding, private launch/preflight and real-native-tool tests, but no
approved runtime orchestration creator/installed-CLI launch or full lifecycle integration. See [current implementation](claude-cli-native-orchestration.md).
