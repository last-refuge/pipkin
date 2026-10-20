# Use Claude Code CLI in Pipkin

Choose **Claude Code CLI · Haiku / Sonnet / Opus** in the ordinary model picker. New CLI
conversations use the installed CLI's **default coding tools**, including file reads, search,
edits, writes, shell commands and CLI-owned agents. Setup also offers an explicit
**Pipkin orchestration** choice on Linux (see below). There is no new legacy chat/Write selector and
**no per-tool approval dialog**. The CLI runs unattended with `bypassPermissions` and
`--dangerously-skip-permissions`; submitted tasks can modify files and run commands with your
user's privileges. This is not a sandbox. Stop cannot undo effects.

The CLI uses its own normal login, not Pipkin's native subscription credentials or an API-key
fallback. Its tools execute inside the CLI, not through interpretation of transcript text.
Printed `<invoke>` or similar text is just text. Pipkin observes structured CLI tool calls and
results and retains them as external activity; it never turns them into native tool commands.

## Pipkin orchestration

In **Settings → Connections → Claude Code · local CLI → Configure provider…**, choose
**Pipkin orchestration**, review the executable/project/model and confirm the connection.
Then select a CLI model to create a separate orchestration conversation and confirm that
conversation's setup. Existing Coding conversations retain their original policy.

Claude uses its own login for inference while Pipkin executes tools and owns goals, children,
questions, queues, plans and jobs. Native attachments, steering, follow-ups and their history
are available. Reconnect requires explicit confirmation and never resends the previous message.
The selected CLI alias is fixed for the conversation; choose a separate conversation for a
new model. New orchestration setup offers Default, Low, Medium, High, Extra high and Max
requested effort. Effort is also fixed for that conversation; effective limits remain CLI-owned.

This desktop route has offline regression coverage. Broader installed-CLI and real-window
qualification remains outstanding. Child launch supports fresh or inherited context; steering
reaches an active child at its next native tool-result boundary. If it finishes first, Pipkin
reports non-delivery without restarting it. Independent auditors cannot be steered. MCP
settings use the selected conversation's project; Skip-audit records your explicit decision
and stops the matching auditor without cancelling the conversation's approval.
See [the capability checklist](claude-cli-native-orchestration.md) for the exact scope.

## Connect once, then select the model normally

1. Open **Settings → Connections → Claude Code · local CLI → Configure provider…** if the
   provider has not been configured. Discovery reads filesystem metadata without executing
   candidates, shells or package managers. Select the installed executable if necessary and
   use **Connect Claude Code**. This checks the selected installation and its normal login;
   provider configuration does not send a prompt or select a model.
2. Select a CLI model in the regular model picker. For the configured executable, Pipkin
   coordinates fresh checks and conversation creation automatically, without another path
   confirmation. If discovery finds a *different* executable, it does not execute it silently.
3. Send your coding instruction normally. Read/search/edit/shell tools do not wait for an
   approval panel. The model picker shows the actual CLI alias; there is no silent native/GPT
   fallback. Configured-provider reconnect similarly checks and returns without another
   confirmation, but does not submit or repeat a prompt.
4. Inspect actual files and **Refresh workspace changes** as needed. Changes shows project
   Git state, including unrelated edits; it is not proof that a particular CLI action caused them.

The selected project is the CLI working directory. Changing backend or CLI alias creates a
separate conversation; source history and drafts are not forwarded. New follows the current
model or pending explicit model choice. A failed/cancelled switch affects only its source and
can be abandoned with **Keep this conversation's provider**.

**Existing legacy chat-only and approval-backed Write conversations keep their immutable
policy and history.** Their model label now identifies the limitation. Click **Use coding model**
in the composer, or select the same CLI alias again in the normal model picker, to open a ready
coding conversation. Selecting the same alias no longer silently keeps the tool-disabled route.
New also uses the coding provider. Source history and unsent drafts stay in the old conversation;
nothing is forwarded, rebound or replayed. Legacy approval recovery remains for old receipts only.

## Compatibility and host policy

- The built Linux desktop accepts stable numeric CLI versions **2.1.299 or newer**, subject to
  the required flags, captured executable/project identity and normal CLI login checks. A
  binding records the actual version. Changed identities are not rewritten automatically.
- Normal owned desktop engines enable hosting by default. `--disable-local-claude-cli` disables
  new CLI execution; the retained `--disable-local-claude-cli-write` switch disables coding/tool
  execution (including legacy Write). Neither deletes history or login credentials.
- Demo does not execute the CLI. Attached hosts own their policy; standalone server/library
  defaults remain off. Legacy server opt-ins remain available.
- CLI tools are not Pipkin-native tools, goals, queues or subagents. Native orchestration
  commands, native retries, image attachments and in-place model changes are not advertised
  for these conversations. CLI-owned agents are part of CLI execution, not native worker rows.
  A separate in-development authenticated native-tool driver and immutable storage bindings
  have offline tests, but no approved runtime creator/CLI launch or enabled capability; see
  [implementation status](claude-cli-native-orchestration.md). It does not change this policy.
- Additional configured MCP servers and hooks are not activated by this provider. The fixed
  launch uses empty MCP configuration, disabled hooks and empty setting sources, without
  modifying the installed CLI or its configuration. Normal CLI login remains separate.
- Ordinary coding turns run until completion or Stop, without a fixed execution deadline.
  Long builds and quiet commands may continue beyond 30 minutes. Stop remains effective
  during tools and output backpressure; output limits and process cleanup still apply. After
  the CLI process exits, pending output has a separate 30-second drain deadline. Legacy
  chat/Write turns retain their three-minute bound, and native orchestration keeps its channel
  lifetime. Setup freshness (five minutes, with 30-second Create preflight freshness) is not
  connection lifetime.

## Stop, restart and uncertainty

In ordinary CLI coding conversations, a clean **Stop** settles the current instruction as
stopped and keeps the conversation available. Wait for cleanup, then send a new instruction
in the same composer. Pipkin resumes the existing CLI session and sends only that new text;
it does not resend the stopped instruction. Completed tool results stay visible, while tools
without a confirmed result remain **Result unknown**. Inspect existing changes before asking
Claude to repeat a tool operation: Stop cannot undo file effects.

Continuation requires a confirmed CLI session, delivered input, user-requested cancellation,
owned-process cleanup, still-active connection authorization and no malformed/rejected stream evidence beyond the expected incomplete
EOF. Early cancellation before session initialization, crashes, transport failures, lost
ownership and unconfirmed cleanup still make the conversation read-only. Disconnect still
revokes the connection's execution authorization; reconnect does not send a prompt.
For conversations locked by an older build after a clean Stop, choose **Reconnect Claude Code…**
in that conversation. Pipkin checks the saved cleanup evidence and freshly verifies the same
project, CLI installation, account, configuration and model. If those checks pass, it unlocks
the existing conversation without sending a prompt or changing the stopped request's record.
Then send a new instruction; Pipkin resumes the same CLI session without replaying the old
request. Existing Unknown turns, timeouts and rejected-output failures remain locked.
Recovery is explicit; updating the app alone does not unlock conversations. After restart,
reconnect to a valid conversation before sending a new instruction.

An uncertain prompt, creation or old approval decision must not be resent. Duplicate request
IDs resolve to their existing receipts, not a second launch. For uncertain creation use the
explicit read-only lookup in Connections; absence alone never triggers automatic recreation.
Fresh work requires a new instruction, not a retry of an uncertain operation.

A confirmed success can continue. An explicit CLI-reported error can accept a new instruction
only with independently finished transport/exit and resolved action evidence. Outside the
confirmed clean Stop path, missing terminal results, identity conflicts, storage failures
and unconfirmed cleanup remain conservative.

Streaming presentation is separate from execution. Completed blocks repair previews; optional
telemetry and CLI-owned child observations do not establish root completion. Tool output previews
are bounded, with truncation/warnings instead of terminating a build because its output is large.
Root tool IDs/results stay correlated, including tools newly reported in the CLI's default
inventory. No arbitrary markup or output becomes execution authority.

Diagnostics retain bounded event/error/block facts, not raw rejected frames, account data or
stderr. Existing settled receipts are never rewritten. See [the scorecard](scorecard.md).

## Optional isolated testing and evidence

Normal use needs no test launcher. For an isolated profile, use a short private directory under
HOME and a disposable project; do not reuse retained qualification fixtures or the user's live
profile for experiments. No installation/account/billing changes are performed by setup.

[Historical qualification](claude-cli-bridge-evidence.md) and the
[ten-run ledger](../fixtures/claude-cli-probe/2.1.299/README.md) concern CLI2.1.299 chat/restricted
Write, not unattended coding. A separately authorized [three-prompt follow-up](../fixtures/claude-cli-probe/unattended-coding-followup.md)
passed on **CLI2.1.301 / Sonnet**: real Read/Write calls through the engine and the desktop
core/adapter path, same-alias legacy-to-coding selection, fresh/resumed turns, exact file contents,
zero approval rows and zero native-provider requests. All three prompts are used; no further live
inference is authorized. This is not a native-window interaction test or full Pipkin orchestration
parity. Bash/Edit/CLI-agent execution, other aliases/releases, billing/extra usage, managed policy,
full descendant containment and native accessibility remain unverified live.

The official [CLI reference](https://code.claude.com/docs/en/cli-reference) documents `--tools
default`, bypass mode and the static system supplement. The [unattended-run guidance](https://code.claude.com/docs/en/headless#turn-off-permission-prompts-in-unattended-runs)
explains that `--permission-prompts none` denies otherwise-unresolved prompts instead of waiting;
the chosen permission mode decides tools first. This is documentation evidence, not a live test.
The earlier offline implementation passed frozen-source workspace/Clippy/fmt gate269. The
follow-up's routing/live evidence and final gate status are recorded in [the scorecard](scorecard.md).

## Reading CLI activity and interrupted conversations

CLI tool activity appears as a compact status row. Expand it to inspect or copy the bounded
output preview. Expanded output and thinking use the same Markdown formatting as replies,
including headings, lists, links, tables and fenced code blocks. Thinking is collapsed too;
normal answers remain in the conversation.

Coding tool observations may arrive after the startup inventory and are still display-only.
Claude can emit another initialization while continuing after background tasks; Pipkin
validates it without resetting the work already recorded.
Coding can also emit several completion records when background work finishes. Pipkin
collects these until the CLI exits, retaining any failure and counting each identified turn
once. A result alone does not mark the process complete.
Pipkin orchestration retains its exact native tool catalog. If a previous build interrupted a
conversation because an output record was rejected, the update does not replay that work or
clear its uncertainty. Inspect existing project effects, then start a fresh conversation.

CLI tool calls and thinking use Pipkin's compact activity groups: consecutive steps collapse
into one progress line, and you can expand the group and each step's Markdown output.
Failed or unknown steps remain marked; the status strip includes CLI activity counts.
Assistant commentary keeps the same Markdown presentation as other models.
