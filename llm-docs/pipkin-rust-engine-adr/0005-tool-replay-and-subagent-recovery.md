# ADR 0005: Tool replay and subagent recovery contract

**Status:** Accepted (2026-10-11). Tool-call intent/result rows, the unsafe-by-default replay
rule, and the atomic subagent child/task mapping with once-only result delivery are
implemented in the store and runtime. The coding tools themselves (`read`, `write`, `edit`,
`bash`) and the shared tool machinery are **in progress** in `E/tools` (in this tree
`builtin_tools()` is empty and `tools::run` returns an error result). The recorded
**contract-provenance check** for subagent recovery (the overlay's `recoveryContract`) is not yet
implemented.

## Context

Reference (audit `engine-tools-durable-usage.md` §3.7, `extensions-subagents.md` §6):

- Tool task phases `call` (resolve, `prepareArguments`, validate, `beforeTool` hooks) →
  intent checkpoint `{phase:"execute", arguments, replay, recoveryExtension?,
  recoveryContract?}` → execute → result (`durable/src/harness/tool.ts:35-154`).
- On reopen in `execute`: rerun only if the stored `replay === "safe"` **and** the currently
  resolved tool's `replay === "safe"`; otherwise the result is the durable partial output plus
  `[error] Tool <name> was interrupted and may have partially run` (code `interrupted`), the
  task fails and owned child conversations are aborted. `read`, `write`, `edit`, `bash` are
  `unsafe` (no `replay` field).
- `subagent` is `replay:"safe"`: a rerun finds the child it created
  (`scanConversations({ownerTaskId})`) and resubmits under `requestId: "subagent:<taskId>"`,
  which admission deduplicates (`coding-agent/src/experimental/durable/subagent.ts:24-55`).
- Pipkin's overlay (`packaging/pi-subagents.patch`, `tool.ts` hunk) lets an interrupted
  `subagent` call resume through the registry even after the session **disabled** subagents,
  but only when the recorded `recoveryExtension === "subagent"` and the recorded
  `recoveryContract` (JSON of name, description, parameters, replay) equals the candidate's; a
  removed registration or changed description/replay gets no recovery authority
  (`harness-tools-recovery.test.ts` added by the patch). New calls while disabled fail with
  `Tool subagent is not available`.

## Decision

1. **Intent before effect.** `tool_calls(state='intent', arguments, replay_safe)` is
   committed before a tool executes (`store::tool_intent`). The result commit sets
   `state='done'`, links `result_entry`, and stores the retained output (`tool_outputs`) in
   the same transaction.
2. **Unsafe by default.** On resume, an `intent` row whose recorded `replay_safe = 0`, or whose
   current implementation is not replay-safe, becomes `state='interrupted'` with the
   reference-format result (`tools::interrupted`: partial output + `<harness>` `[error] Tool
   <name> was interrupted and may have partially run`). `write`, `edit` and `bash` are never
   rerun because the engine restarted.
3. **Subagent is the replay-safe registration**, implemented by the session runtime through
   `tools::ToolHost::subagent` (`runtime/subagents.rs`):
   - one transaction creates (or, on replay, finds) exactly one child session for
     `(parent_session, parent_call)` together with its `subagent_tasks` row (unique
     constraints on both the pair and `child_session`);
   - the child prompt is admitted under the durable key `subagent:<call id>`;
   - the child inherits model, thinking and cwd, runs **without** the `subagent` tool (no
     nested delegation, matching the reference's `{remove:["subagent"]}`);
   - the answer (concatenated text of the child's final reply, no separator) becomes the tool
     result; `mark_task_delivered` flips `result_delivered` once, so replay cannot append the
     result twice;
   - Stop of the parent call stops the child.
4. **Disabled subagents:** `sessions.subagents_enabled` hides the tool from new requests;
   already-admitted calls recover. To match the overlay's safety boundary, recovery must also
   check a recorded contract (name, description, parameter schema, replay policy) against the
   current registration and refuse recovery with an explicit interrupted result on mismatch.
   **Open item:** add a `contract` column (or `kv` record) at intent time and the check at
   recovery, with the two overlay tests ported. Until then the row SUB-RECOVERY-CONTRACT is
   open.
5. **Provenance improvement (proposed, I-05):** record the engine build/revision alongside the
   contract so recovery can distinguish an identical-contract code change; the overlay README
   admits this is unproven (`packaging/pi-subagents/README.md:17`). This needs owner sign-off
   before it changes recovery outcomes.
6. **Concurrency:** writes to the same canonical path are serialized across sessions
   (`tools::MutationLocks`); tool processes are bounded engine-wide
   (`EngineConfig::max_tool_processes`); a parent waiting on children must release its
   generation permit so nested work cannot deadlock (plan §4 "Durable subagents") — the
   permit-release behavior needs a dedicated test.

## Consequences

- Same observable recovery outcomes as the reference for built-in tools; one fewer source of
  duplicated side effects than "always replay".
- Subagent children are full native sessions (`sessions.parent/root`), so the inspector can
  page them with the ordinary history API instead of the reference's 64-mount keyed
  transcript cap.
- Wrapped tools and extension tools (ADR 0006) default to unsafe; an extension must declare
  replay safety explicitly and is subject to the same contract check.

## References

- Plan §4 "Tool recovery and concurrency", "Durable subagents and task ownership", P02/P09.
- Audit `engine-tools-durable-usage.md` §1, §3.7; `extensions-subagents.md` §6.
- Native: `crates/pipkin-engine/src/tools/mod.rs`, `runtime/{run,subagents}.rs`,
  `store/mod.rs` (`tool_intent`, `tool_result`, `create_task`, `mark_task_delivered`).
- Tests (pass, 2026-10-11): `store/tests.rs` —
  `a_child_task_is_created_once_per_parent_call_and_delivered_once`,
  `tool_output_is_retained_with_honest_drop_counts`. Runtime-level subagent and tool-replay
  tests are pending the tools work package.
