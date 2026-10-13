# Native gate testing notes (owner-run)

**v1 qualification is active:** see [v1-release-gates.md](v1-release-gates.md) for the current identified build,
automated package evidence, owner-reported daily-use beta, and the release tracker. The beta below is no longer
“not started,” but individual manual checks still need recorded results; heavy use does not implicitly pass them.

These checks need a person at the machine and a real desktop session. They are **unverified** unless a result is recorded below. Launch with `scripts/try-m2.sh` (managed engine, scripted provider), or `cargo run -p pipkin-app --release -- --demo normal`.

## v1 candidate (prepared 2026-10-12)

The candidate for the owner-run gates below is the code at commit **`9c07a1b6a660`** on
`main` (schema 20, protocol 31; this note lands in the docs-only commit that follows it).
`pipkin --version` on the candidate prints `source 9c07a1b6a660…, engine protocol 31,
engine schema 20`. Prepared by engineering: `cargo test --workspace`,
`cargo clippy --workspace --all-targets` and `cargo fmt --all --check` pass (1655 tests),
and `cargo build --release -p pipkin-app` produces `target/release/pipkin`.

Launch forms for the gates:

- Real engine, scratch data: `target/release/pipkin --data-dir /tmp/pipkin-gate --project <dir>`.
- Demo content (no provider): `target/release/pipkin --demo normal --data-dir /tmp/pipkin-gate`.
- Engine unavailable (gate 13): `target/release/pipkin --engine-attach /tmp/no-engine --data-dir <your daily data dir>` after `mkdir -m 700 /tmp/no-engine` (a private endpoint directory with no server; a 0755 directory is refused by the endpoint's permission rule instead, which is also an unavailable state but a different reason).
- Pinning with `scripts/guard.sh pin <pid>`: the window's pid can differ from the shell's `$!` by a few; if the pin times out, take the pid from `hyprctl clients -j` (class `pipkin`) and pin that.

Gates asked for in the finish plan (§7) map to the rows: minimize/restore 1, suspend/resume
2, cold boot 3, monitor unplug/replug 19, 150% scale and large text 6, IME commit/cancel 5,
picker/drop/clipboard/links 12 and 20, engine-unavailable 13. Record each with the date,
compositor, scale and input-method engine; fixes go back to engineering and the candidate
commit is bumped here.

| # | Gate | Steps | Pass when | Result |
|---|------|-------|-----------|--------|
| 1 | Minimize/restore | Open a conversation with text in the composer, minimize, restore. | Focus returns to the composer, draft intact, transcript scroll position unchanged, no blank frame. | unverified (engineering typed a draft into its own test window on 2026-10-13 but the compositor move was denied by the permission system; note that this Hyprland takes Lua-quoted dispatchers, e.g. `hyprctl dispatch 'movetoworkspacesilent("special:x,address:0x…")'`) |
| 2 | Suspend/resume | Same setup with a run in progress (send a prompt first); suspend, resume. | App redraws; engine reconnects or shows "Not connected" with the saved copy; no duplicate send; draft intact. | unverified |
| 3 | Cold boot | Reboot, launch Pipkin. | Reopens the last conversation from the cache; time to usable window noted. | unverified |
| 4 | Screen reader (Orca) | `orca` running; focus the composer (Ctrl+L). Type text, move the caret with arrows and Ctrl+Left, select with Shift+Left. Then tab through navigation, transcript, tool blocks, run status. | Typed characters and caret moves are spoken; every control has a name and role; run status changes are announced. Known: on focus Orca reads the composer text, but typed characters and caret moves were **not** spoken in the last attempt (caret events arrive, no speech). | partial |
| 5 | IME | fcitx5 + an engine (pinyin worked once). Compose, pick a candidate, commit; also cancel with Esc. | Candidate window follows the caret; Enter during composition does not send; cancel leaves no stray text. Only pinyin commit has been observed. | partial |
| 6 | 150% scale | Set the display to 150% (Hyprland may snap to 1.6). Check light and dark themes, enlarged text, focus rings, a narrow window. | No clipped text or controls; focus ring visible; layout usable. One capture at 1.6 looked correct. | partial |
| 7 | Presentation latency | Needs a high-speed camera or compositor timing. | Not measurable from inside the app. | unverified |

## Install gates (need sudo, so owner-run)

| # | Gate | Steps | Pass when | Result |
|---|------|-------|-----------|--------|
| 8 | Clean install | `cd packaging && makepkg -d -f`, then `sudo pacman -U pipkin-*.pkg.tar.zst` on a machine or fresh user with no Pi checkout. | Pipkin appears in the launcher (Walker/rofi) with its icon; opening it shows the window; `pipkin --diagnose --probe` ends with no problems; a conversation with a real provider works. | unverified as an install; measured 2026-10-12 on the development candidate: `--diagnose --probe` against a scratch data directory reports protocol 31, schema 20 and "engine probe: OK" |
| 9 | Upgrade | Install again over it (bump `pkgrel`); open Pipkin with a saved draft first. | Draft and history are still there; `--diagnose` shows the new engine. | unverified |
| 10 | Rollback | `sudo pacman -U` the older package after the upgrade. | If the schema changed, Pipkin says the database is newer and stays untouched; restoring the backup works. | unverified |
| 11 | Long soak | `PIPKIN_SOAK_ROUNDS=20000` (see docs/packaging.md), and the real app open for a day. | Memory and open files stay flat. | partial (1500 prompts run) |

## Platform and release gates (M6)

| # | Gate | Steps | Pass when | Result |
|---|------|-------|-----------|--------|
| 12 | Drag and drop | Drag two files and a folder from a file manager onto the conversation. | The two files appear as attachment chips; a message says folders were skipped; the area tints while dragging. Needs a real drag, so it was only built, not tried. | unverified |
| 13 | Engine unavailable | `mkdir -m 700 /tmp/no-engine` and start with `--engine-attach /tmp/no-engine`, then open a saved conversation. | A red strip says the engine is not available with an actionable reason, the cached history still renders, sending is off, and the strip clears after a restart without the flag. | partial — observed 2026-10-12 by engineering on the candidate (Hyprland 0.56, scale 1.6, scratch profile, window launched and pinned through `guard.sh`, captured by region): with no conversation selected the centre reads "Engine failed: cannot attach to /tmp/no-engine: …" naming the reason (the permission rule for a 0755 directory; "No such file or directory" for a missing one), and the sidebar offers only New conversation. Second observation 2026-10-13, real engine in the scratch profile `/tmp/pipkin-gate`: Ctrl+N, a typed prompt and Return created a conversation whose run failed honestly with "No model is configured" (Retry offered, composer reports "No models available", "Draft saved"), and the conversation persisted in the profile. The relaunch against the empty endpoint with that saved conversation open was not captured: the permission system denied the step. Owner-run remainder: launch `--engine-attach /tmp/no-engine --data-dir /tmp/pipkin-gate` (the profile now holds that conversation) and confirm the red strip, the cached history, sending off, and the clear after a restart without the flag. |
| 14 | X11 session | Log into an X11 session (or use Xwayland), run Pipkin; try the IME and Orca. | Renders, types, resizes; IME and screen reader work. Rendering was seen under Xwayland only. | partial |
| 15 | Other compositors | Run on GNOME and KDE Wayland: file picker, clipboard, decorations, IME. | Everything in the walkthrough works. | unverified |
| 16 | Generic installer on another distribution | `./install.sh` from the tarball on Fedora/Debian/Ubuntu using packaged Node (without system Node); upgrade, rollback, uninstall. | Same results as `scripts/test-install.sh`, and a launcher entry works. | unverified (checked on Arch in a scratch prefix) |
| 17 | Signed release | Sign with a real key, download elsewhere, run `verify-release.sh --require-signature`. | Verifies; a tampered file fails. | partial (throwaway key only) |
| 18 | Beta | See `beta.md` and `v1-release-gates.md`. | Targets met with recorded evidence. | owner reports heavy daily use; quantitative targets and other-user qualification remain open |
| 19 | Monitor unplug/replug | With the window on an external monitor and a run in progress, unplug the monitor, wait ten seconds, plug it back in (or toggle it with `hyprctl keyword monitor`). | The window survives on the remaining output, keeps its scale when it returns, no blank or frozen frame, the run's output is complete, no duplicate send. | unverified |
| 20 | Picker, clipboard, links and launching | Attach a file through the picker (portal); copy a code block and paste it elsewhere; paste text and an image into the composer; click a web citation `[s1]` and a plain link in a reply; use "open in editor" and "open terminal". | The picker opens and the chip appears; the clipboard carries the code; pasted text and image land in the composer; links open in the default browser; editor and terminal launch in the project directory. | unverified |

Record results here with the date, compositor, scale and input-method engine used.
