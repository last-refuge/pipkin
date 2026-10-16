# Use Claude CLI in Pipkin

Local Claude CLI chat and Write-only hosting is **on by default in normal Pipkin desktop
launches**, for both the child and in-process native engine. Open Pipkin normally, then choose
**Settings → Connections → Claude Code · local CLI → Configure provider…**. Select a CLI model
in the ordinary model picker alongside native providers; there is no exclusive application mode.
You can also start directly from the model picker: select a CLI alias, then use **Continue
conversation setup…** in Connections. Do not start provider configuration again; the chosen
conversation flow is already prepared. **Find Claude again** only refreshes discovery within that
flow, clears old path consent and keeps the dialog open. Outside-click or Close dismisses it.
There is no dedicated sidebar action or Ctrl+N chooser. No experimental launch flags or typed configuration paths are
needed for a detected installation and selected project. Existing conversations retain their backend; enabling the feature does
not automatically inspect an executable, trust a project, send a prompt or approve a Write.

This selectable provider runs your installed, unmodified Claude CLI using its normal login.
Its external execution backend is not Pipkin's native tool runner, a sandbox, or full tool parity. Exact path trust and per-Write approval
remain required. If Pipkin was already running when updated, fully quit and reopen it so the old
worker is not reused.

## Qualified configuration

- Linux/Wayland desktop, built `pipkin` app with its native worker/helper.
- **Claude CLI 2.1.299 or newer** (stable numeric releases). The minimum is compared numerically,
  not by exact equality. Required capabilities, scoped trust and normal CLI login are still checked.
  Each conversation binds the actual observed version; an executable/version change cannot silently
  rewrite an existing binding. Live qualification evidence remains limited to 2.1.299; accepting
  newer versions is a compatibility policy, not a claim that every newer release was live-tested.
- Fixed `haiku`, `sonnet`, or `opus` aliases. Live qualification used `haiku`.
- Chat fresh/resume; restricted Write fresh/resume; explicit Allow once, Deny and Stop.
- Write accepts only the closed `Write(file_path, content)` shape. Read, Edit, Bash, MCP,
  subagents, attachments, native tools/goals/queues/retries and in-place native model changes are
  unavailable in these conversations. CLI output is inert external history, not model context.

## Normal use and optional isolated testing

Start the installed **Pipkin** desktop entry or run `pipkin`. Use your normal profile and project;
no test launcher is required. Build from source with `cargo build -p pipkin-app` if needed.
For optional isolated testing, choose a short, unused private data location under your home
directory and an existing disposable project:

```sh
mkdir -m 700 "$HOME/.pipkin-cli-test"
./target/debug/pipkin --engine native \
  --data-dir "$HOME/.pipkin-cli-test" \
  --project /absolute/path/to/scratch-project
```

Use `--disable-local-claude-cli-write` for chat-only hosting, or `--disable-local-claude-cli`
to disable both. These process-local switches do not persist trust or approvals or change
existing conversation identities. Demo has no CLI execution; attached engines control their
own policy and reject desktop host overrides. Standalone servers and the engine library retain
explicit opt-in defaults. Legacy experimental enable flags remain accepted for old launch scripts
(execution alone retains its old chat-only meaning), but are unnecessary for daily desktop use.
Permission sockets require private, trusted ancestry;
`/tmp` is deliberately rejected. Do not reuse the retained qualification fixtures as your
normal workspace. No installation or billing/account changes are performed by this flow.

1. If first-run setup offers native provider sign-in, choose **Explore the workspace**; a native
   API account is not required. In **Settings → Connections**, use **Claude Code · local CLI →
   Configure provider…**. This configures an independent provider, not an application-wide mode.
   Pipkin captures the selected project and finds the executable on the engine host using a saved
   choice, absolute PATH entries and known installation locations (including mise). Discovery
   does not run Claude, a shell or a package manager. Multiple installations require a choice;
   missing installations use **Choose executable…**. **Change project…** opens a folder picker.
2. Review the resolved paths, model (last-used CLI preference, otherwise Sonnet) and tools
   (**Chat only** by default, or **Write with approval**). Select **Trust these paths and connect**
   only if you trust both the executable and project shown. Provider configuration checks CLI
   login/version but selects no model and creates no conversation. Choose **Claude Code CLI ·
   Haiku/Sonnet/Opus** in the normal model picker, alongside native models. Selection prepares a
   separate CLI conversation and opens Connections for fresh scoped confirmation, then one
   journaled metadata-only creation. No prompt or Write is authorized. Preferences remember
   paths/model/tool-mode hints, never grants or credentials. Native provider sign-in stays separate.
3. Wait for the conversation's authoritative readiness before sending. The composer's model
   picker shows the actual **Claude Code CLI** model; there is no separate Backend label. Choosing Claude does not convert
   an old GPT/native chat. A failed/cancelled model change blocks only its initiating conversation;
   use **Keep this conversation’s provider** to abandon that change explicitly. Other conversations
   remain usable. Switching to a native provider or another CLI alias creates a separate chat,
   without transferring source history/draft. `Ctrl+N` follows this conversation's model (or its
   pending choice); a blank workspace uses the last saved choice. CLI setup/reapproval/recovery
   stays in Connections. Execution authority never persists. You no longer need to
   race the 30-second preflight freshness window or manually sequence Inspect/Check/Create.
   The limits remain enforced internally; expired or uncertain operations do not automatically
   retry or renew consent. Unsupported versions and signed-out CLI status have actionable errors.
   Advanced path editing, diagnostics and recovery remain available.
4. Send one small request in the scratch project. For Write, open **Review CLI approvals…**,
   explicitly **Refresh reviews**, inspect the full escaped arguments, and decide once.
   **Ctrl+Shift+C** copies the complete current review without deciding. Tab/Shift+Tab navigates;
   Page Up/Down scrolls, including the focused argument region. Do not approve by guessing
   a tab count. A consumed receipt is not proof of delivery, completion or the file effect.
5. Inspect the actual file and **Refresh workspace changes**. Changes reports captured-project
   Git state and can include unrelated work; it is not action attribution.

## Reapproval, uncertainty and disabling

After restart, use **Reconnect Claude Code…**. Saved session metadata is loaded and inspected
without executing Claude. Review the captured paths and confirm again to check and return to the
same conversation. Hints and preferences are not approval. If an older uncertain creation blocks setup, **Resume Local CLI setup…**, select its
exact recovery identity when necessary, and **Look up creation (read-only)**. Only after an
explicit absence result may you **Keep recovery record and start another setup**. Do not
blindly repeat Create; absence is not proof an earlier request can never finish.

An uncertain prompt or approval must never be resent. Retained Unknown runs remain Unknown;
compatibility fixes do not upgrade them. Resume requires independently completed context.
Stop revokes authority and supervises cancellation but cannot undo effects or guarantee all
descendants died. After a crash inspect processes and files before starting separate work.
Original-owner approval uncertainty can remain unresolvable; reopening does not restore authority.

To disable execution, exit and relaunch with `--disable-local-claude-cli` (or disable only Write
with `--disable-local-claude-cli-write`). History, CLI sessions and login credentials are not deleted. Do not bypass an unsupported-version, ownership or readiness gate.

## Evidence and limits

[Qualification evidence](claude-cli-bridge-evidence.md) and
[the append-only ten-run record](../fixtures/claude-cli-probe/2.1.299/README.md) separate successes
from failures. Native146 fresh Write completed but **failed its review interaction**; native158
resumed Write passed exact copied-review validation before Allow, clean completion, independent
file verification and Changes listing. No authorization remains for additional live qualification.

Billing/plan allowance/extra usage, managed-policy isolation, complete descendant containment,
live qualification of the redesigned installed setup, real IME/accessibility and display-presentation latency
remain unverified. Native navigation used short arguments; nonzero long-argument scrolling is
headless-tested, not a measured native overflow case. This is experimental testing readiness,
not production or universal platform/tool qualification.
