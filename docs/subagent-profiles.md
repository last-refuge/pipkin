# Subagent profiles

A `subagent` call can name a **profile**: a role that fixes what the child may do. The
engine resolves the profile at launch, freezes the result into a launch record stored with
the child's task, and enforces that record for the child's whole life. Editing a profile
later changes only children launched afterwards. This is SUB-1 of the
[native capabilities plan](../llm-docs/native-capabilities-implementation-plan.md) (§6);
the [subagent inspector](subagent-inspector.md) is unchanged apart from showing the profile.

Status: implemented and covered by automated engine tests (2026-10-12).

## Built-in profiles

| Profile | Tools | Context | Access, honestly |
| --- | --- | --- | --- |
| `worker` (default) | Every tool a child can have | fresh | full access |
| `reviewer` | `read`, `plan_read`, `web_read` | fresh | read-only |
| `scout` | `read`, `plan_read`, `web_search`, `web_fetch`, `web_read` | fresh | workspace read-only; web access |

No child is ever offered `subagent` or `ask_user_question`: children do not delegate further
and do not ask the person. `reviewer` and `scout` carry short instructions that reach the
child as a `profile` system section (patched like any other positional section).

A profile is called read-only only when it can neither change the workspace nor reach the
network. `scout` reaches the network, so it is labelled as such.

## Settings-defined profiles

`settings.json` (user or project) may add profiles or replace built-ins by name under
`subagents.profiles`:

```json
{
  "subagents": {
    "profiles": {
      "tester": {
        "description": "Runs the tests and reports",
        "instructions": "Run the relevant tests and report exact failures.",
        "tools": ["read", "bash"],
        "model": "anthropic/claude-sonnet-4-5",
        "thinking": "medium",
        "context": "inherit"
      }
    },
    "maxConcurrent": 4,
    "maxTotal": 64
  }
}
```

`tools` is `"all"` or a list of tool names a child can have; naming an unknown tool, or one
never available to children, makes every launch fail with the reason rather than silently
narrowing the profile. `model` is a `provider/modelId` key; `thinking` a level name;
`context` is `fresh` or `inherit`.

## Launch

`subagent {task, profile?, context?}`. The call's `context` overrides the profile's default.

1. The parent's merged settings are read; profiles and caps come from them.
2. The model is the profile's override or the parent's model, and must exist in the catalog
   and be available; otherwise the call fails and **no other model is substituted**. The
   thinking level is validated the same way.
3. With `context: inherit`, a bounded excerpt of the parent's recent user and assistant
   messages (at most 20 messages and 16 KiB, oldest first, never thinking, tool rounds or
   system entries) is appended to the task inside a labelled `<parent_context>` block that
   records the parent's revision, the number of messages and how many were left out. The
   excerpt is text only: no goal, no permissions, no scheduler travels with it.
4. Caps are checked in the same transaction that creates the child: at most `maxTotal`
   children per conversation and `maxConcurrent` live (not done, failed or stopped) at once.
   A refused call is a tool error; no child row is created.
5. The child session is created with the resolved model and thinking, the composed prompt is
   stored as its task, and the launch record is stored beside it.

A replayed call (after a restart) finds its existing child and uses the stored prompt and
record; nothing is re-resolved.

## Enforcement

- The child's tool catalog (`tools_for`) is filtered by its launch record, so a disallowed
  tool is never offered to the model; a call to one anyway is recorded as
  `Tool <name> is not available`, like any unoffered tool.
- The record is read from the child's task row, never from current settings, so a running
  or recovering child keeps the policy it was launched with across settings edits and
  engine restarts.
- Children launched before launch records existed (schema 13) have no record and keep the
  full tool set they always had.
- The directory row carries the profile name (`SubagentRow.profile`, protocol 15); the
  inspector shows it on the task row as given, never inferred from the task text.

## Boundaries

- No per-child Stop or Steer controls (SUB-2) and no asynchronous children (SUB-3).
- No structured parallel groups: the model may issue several `subagent` calls in one round
  and the concurrency cap bounds them, but there is no group result.
- Result artifacts: a child's answer is the parent's tool result, retained like any tool
  output; there is no separate artifact reference yet.
- Upgrading across this change: a `subagent` call that was in flight on the old tool
  contract is recorded as interrupted after the upgrade (the contract check protects
  recovery), and the model may delegate again.
