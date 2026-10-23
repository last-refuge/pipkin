# Getting started

Pipkin **1.1.0** is published on [GitHub Releases](https://github.com/last-refuge/pipkin/releases) with SHA-256
checksums (no signing key yet). The supported target is x86_64 Arch/Omarchy with Hyprland/Wayland; macOS builds
are experimental and other desktops and pure X11 are unverified. See the [changelog](../CHANGELOG.md),
[platforms](platforms.md) and the [release gates](v1-release-gates.md) for what was and was not checked.

## Launching an already running app

On Linux, launching Pipkin again reuses the window for the same data folder instead of
starting a second desktop or engine. `--project /path/to/project` is forwarded to that
window; its existing engine settings remain in effect. Distinct `--data-dir` profiles can
run independently. Version checks, diagnostics and engine-worker/server commands remain
separate entry points. Window activation is verified on Hyprland; other compositors may
apply their own focus policy.

When upgrading from a build without single-instance support, quit all older Pipkin windows
once before opening the updated build. Future repeated launches will bring that app forward.

## Local Claude CLI

Normal Pipkin desktop launches enable chat and restricted Write hosting through the installed
CLI's normal login. CLI2.1.299 or newer is accepted; live qualification evidence is for2.1.299. Configure **Settings → Connections →
Claude Code · local CLI**. There is no dedicated sidebar action or Ctrl+N backend chooser.
Connections configures independent providers; choose **Claude Code CLI · Haiku/Sonnet/Opus**
in the ordinary model picker alongside native providers. Configuration alone selects no model.
Pipkin resolves the installation and selected project; review the paths and choose
**Trust these paths and connect**. No experimental launch flags or manually typed paths are needed
in the normal detected-installation case. Follow the [daily-use guide](local-claude-cli.md),
including explicit path trust and per-Write review. Existing conversations do not change backends.
The composer identifies its actual backend/model. Cross-provider selection opens a separate chat;
a failed/cancelled change blocks only its source until completed or explicitly kept. Other chats
remain usable. New follows the current conversation's model, not another conversation's choice.
A previously published release may predate this integration; use the updated build and fully
quit/reopen any older running instance. Billing, managed policy and complete crash containment
remain unverified. Opt out with `--disable-local-claude-cli`.

## Install a release (Linux x86_64)

```bash
curl -fsSL https://github.com/last-refuge/pipkin/releases/latest/download/install.sh | bash
```

This downloads the prebuilt archive of the latest release, verifies it against the release's `SHA256SUMS`
and installs it into `~/.local` without root. It never builds from source. `--version X.Y.Z` pins a release,
`--prefix DIR` changes the location, and `--rollback`, `--uninstall` and `--list` manage the versioned
install afterwards. Source setup below is for developing Pipkin.

## Recommended: one-command source setup

After the [native prerequisites](source-setup.md#1-install-native-build-prerequisites-once), run
`scripts/setup.sh --run` builds and offline-checks the Rust app and built-in engine, then launches it.
`scripts/run.sh` starts the last successful build; `scripts/setup.sh --install` installs for your user.
No Pi checkout, Node/npm or manual server setup is required. See [source setup](source-setup.md).
For build-cache maintenance on Linux, `python3 scripts/clean-builds.py` previews unused source
builds; see [build storage cleanup](source-setup.md#keep-build-storage-manageable).

## Build a pacman-managed Arch package

Install native prerequisites from `packaging/PKGBUILD`, then run as your normal user:

```sh
(cd packaging && makepkg -si)
pipkin --version
pipkin --diagnose --probe
```

A working Vulkan driver, Git and the declared desktop libraries remain required. A successful build is
not clean-machine desktop qualification. Current artifacts are unsigned and unpublished.

## First run and provider sign-in

From a newly installed package, open Pipkin from its desktop launcher or run `pipkin`. The shared first-run GUI
welcomes you, offers **Claude Pro/Max** and **ChatGPT Plus/Pro** OAuth sign-in, then asks for a project folder and
an available model. Native credentials live under `~/.config/pipkin/engine` (or the explicit data directory).
Settings → Connections provides account management, including Claude browser and copy-code login.
Use `--pi-agent-dir DIR` only to select a historical import source; it is not a live Pi runtime profile.
Keep credential files private. Environment API keys set in a terminal may not reach a desktop launcher.
The installer cannot authenticate your provider or prove a paid model request succeeds.

After selecting a model, send a small prompt. **This makes a real provider request and may be billed.** Demo mode
is simulated; the scripted-provider test is offline. Neither establishes that your paid provider works.

### Missing or expired authentication

- No usable models: open **Ctrl K → Refresh models** (or the empty-state Refresh models button). Refresh is an
  engine catalog operation, not provider login or a guarantee of network access.
- Missing key/unauthorized/expired token: read the visible failure; check the selected provider and the agent
  directory used by the engine. Reauthenticate in Connections or renew the provider key, then reconnect/restart as needed.
- Models still missing: check provider configuration and network/proxy access. Do not delete saved sessions or the
  desktop database to fix credentials. Do not repeatedly resend a file-changing prompt as an authentication test.
- `pipkin --diagnose --probe` proves an offline handshake, **not** provider authentication or account balance.

The setup workflow is documented from the native implementation. A new-user installation to
real first reply on a clean supported desktop remains an open qualification gate; no participant results are claimed.

## Hide projects or threads in orchestration

Use **Hide** on a project header, or select a thread and choose **Hide thread** / **Hide project**
in its details. The thread dialog also has **Hide thread**. A hidden root thread hides its child
tasks too. Hiding is a saved dashboard preference: it removes those items from the graph, list,
search results and attention counts, including running work. It does not stop work, archive
conversations, mark results reviewed, or discard drafts and history.

Open **Hidden (N)** beside Graph/List to restore individual projects or threads, or choose
**Show all hidden**. These choices survive restart and ordinary filter changes. Showing a
project keeps individually hidden threads hidden; showing a thread inside a hidden project
also requires showing the project. Restored items still follow the current search, status
and 30/60/90-day recency filters. Conversations remain accessible outside orchestration.

## Know what the controls mean

- **Stop** retires stopped input from future model context in the pinned engine; history and completed tool effects
  remain. It is not undo. A raced successful completion may remain completed.
- **Steer/queue** are engine-authoritative. If settlement is unknown, wait for reconciliation rather than copying
  and resending the prompt. Timeout does not prove a tool stopped.
- **Changes** shows the current uncommitted Git diff for files associated with the selected session's successful
  `edit`/`write` calls. It includes staged, unstaged, and untracked changes—not a history of tool operations.
  Committed or reverted changes disappear, and subsequent edits compare against the latest HEAD. A lightweight
  background scan detects external commits too. Session file scope excludes unrelated project files; if multiple
  tools edit the same session-owned file, its diff reflects that file's current combined uncommitted state.
  Refresh does not send agent input. Earlier session file paths are recovered from bounded engine history.
- **Prompt history** is separate for each session. In the conversation input, Up recalls older prompts when the
  caret is on the first visual line; Down moves toward newer prompts on the last visual line. Moving past the newest
  prompt restores your unsent draft. Editing a recalled prompt starts a new draft rather than changing history.
  The latest 200 journaled prompts survive app restarts; loaded conversation messages also populate recall history.
  Recall changes text only: it never sends a prompt or reattaches old files. Selection, IME composition, and `@` path
  completion retain their normal arrow-key behavior.
- **Search** covers locally cached history, not all engine history. Older material may not yet be cached.
- **`@` paths** are project-scoped references inserted into the text. Selecting one does **not** attach file contents.
  Use the attachment picker/drag-and-drop when you want an attachment; missing/deleted paths cannot become files by
  being mentioned. Tab/Enter or a popup click inserts the selected reference; Ctrl-click opens a linked path.
  Each newly opened mention query refreshes a bounded, read-only path snapshot off the UI thread. It is not a
  live filesystem watch or an exhaustive index: dependency/VCS folders, symlinks and unreadable descendants are
  skipped. Newly created/deleted files appear on the next query refresh. A failed root scan is shown as unavailable,
  not an empty success; repair access or select the correct project, then reopen the query to retry.
  Native picker/drop/IME qualification remains separately tracked.
- **Pi extensions** are outside scope and are not loaded; imported metadata is preserved.

See [support and recovery](support.md) for data ownership, quitting and diagnostic privacy.
