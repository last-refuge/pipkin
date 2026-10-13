# Getting started

Pipkin **1.0.0** is published on [GitHub Releases](https://github.com/last-refuge/pipkin/releases) with SHA-256
checksums (no signing key yet). The supported target is x86_64 Arch/Omarchy with Hyprland/Wayland; macOS builds
are experimental and other desktops and pure X11 are unverified. See the [changelog](../CHANGELOG.md),
[platforms](platforms.md) and the [release gates](v1-release-gates.md) for what was and was not checked.

## Recommended: one-command source setup

After the [native prerequisites](source-setup.md#1-install-native-build-prerequisites-once), run
`scripts/setup.sh --run` builds and offline-checks the Rust app and built-in engine, then launches it.
`scripts/run.sh` starts the last successful build; `scripts/setup.sh --install` installs for your user.
No Pi checkout, Node/npm or manual server setup is required. See [source setup](source-setup.md).

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
