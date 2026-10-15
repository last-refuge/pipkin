# Use Claude CLI in Pipkin

Local Claude CLI chat and Write-only hosting is **on by default in normal Pipkin desktop
launches**, for both the child and in-process native engine. Open Pipkin normally, then choose
**New Local CLI conversation…** in the sidebar, command palette or Connections. No experimental
launch flags are needed. Existing conversations retain their backend; enabling the feature does
not automatically inspect an executable, trust a project, send a prompt or approve a Write.

The bridge runs your installed, unmodified Claude CLI using its normal login. It is not a native
provider, a sandbox, or general Claude Code tool parity. Exact path trust and per-Write approval
remain required. If Pipkin was already running when updated, fully quit and reopen it so the old
worker is not reused.

## Qualified configuration

- Linux/Wayland desktop, built `pipkin` app with its native worker/helper.
- **Exactly Claude CLI 2.1.299**. Other versions fail closed; an upgrade needs requalification.
- Fixed `haiku`, `sonnet`, or `opus` aliases. Live qualification used `haiku`.
- Chat fresh/resume; restricted Write fresh/resume; explicit Allow once, Deny and Stop.
- Write accepts only the closed `Write(file_path, content)` shape. Read, Edit, Bash, MCP,
  subagents, attachments, native tools/goals/queues/retries and native model switching are
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
   API account is not required for this bridge. Use **New Local CLI conversation…** in the
   palette or Connections. Choose Chat only or
   Restricted Write, an absolute executable path, a project, and a fixed alias.
2. **Inspect**; verify the captured executable, project, version and mode. Explicitly trust
   both paths, then **Check** using your existing CLI login.
3. **Create** promptly: preflight freshness is **30 seconds**, distinct from the five-minute
   setup ticket. Create stores metadata; it does not submit a prompt. The engine's authoritative
   readiness—not the metadata label—controls sending.
4. Send one small request in the scratch project. For Write, open **Review CLI approvals…**,
   explicitly **Refresh reviews**, inspect the full escaped arguments, and decide once.
   **Ctrl+Shift+C** copies the complete current review without deciding. Tab/Shift+Tab navigates;
   Page Up/Down scrolls, including the focused argument region. Do not approve by guessing
   a tab count. A consumed receipt is not proof of delivery, completion or the file effect.
5. Inspect the actual file and **Refresh workspace changes**. Changes reports captured-project
   Git state and can include unrelated work; it is not action attribution.

## Reapproval, uncertainty and disabling

After restart, use **Reapprove Local CLI…**, explicitly **Load saved session hints (read-only)**
and **Use saved paths, mode and alias**, then Inspect/trust/Check/Return again. Hints are not
approval. If an older uncertain creation blocks setup, **Resume Local CLI setup…**, select its
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
installed release-package qualification, real IME/accessibility and display-presentation latency
remain unverified. Native navigation used short arguments; nonzero long-argument scrolling is
headless-tested, not a measured native overflow case. This is experimental testing readiness,
not production or universal platform/tool qualification.
