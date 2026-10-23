# Build, run and install Pipkin from source

Source setup builds the Rust app and built-in engine, then verifies an isolated offline handshake.
The probe uses a private, automatically removed directory under `dist/source-setup`, so a full
or quota-limited `/tmp` does not prevent its SQLite database from starting.
No Pi checkout, Node/npm, manual patches or separately started server is needed. It does not authenticate
providers or make paid requests. Do not build as root.

## 1. Install native build prerequisites once

The script never runs sudo, installs a package manager or modifies global Node/npm. Choose your platform:

**macOS (Apple Silicon or Intel):** install Xcode Command Line Tools (`xcode-select --install` if missing),
Rust via rustup, and these Homebrew build utilities:

```sh
brew install python coreutils gnu-tar findutils zstd cmake pkg-config
export PATH="$(brew --prefix coreutils)/libexec/gnubin:$(brew --prefix gnu-tar)/libexec/gnubin:$(brew --prefix findutils)/libexec/gnubin:$PATH"
```

No `brew install node` is needed. The source-installed app is ad-hoc signed, not notarized; macOS may still
require explicit approval. Do not disable Gatekeeper globally.

**Arch/Omarchy (x86_64):**

```sh
sudo pacman -S --needed base-devel git rustup python clang cmake zstd wayland libxkbcommon libxkbcommon-x11 libxcb fontconfig vulkan-icd-loader openssl
```

**Debian 12 / Ubuntu 24.04 (x86_64):** install Rust via rustup, then:

```sh
sudo apt install build-essential git python3 clang cmake pkg-config zstd libssl-dev libfontconfig1-dev libfreetype-dev libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev libxcb1-dev libxcb-xkb-dev libxcb-shape0-dev libxcb-xfixes0-dev libxcb-randr0-dev libxcb-render-util0-dev libvulkan-dev libegl1-mesa-dev libasound2-dev libdbus-1-dev
```

**Fedora (x86_64):** install Rust via rustup, then:

```sh
sudo dnf install git python3 clang cmake pkgconf-pkg-config zstd fontconfig-devel freetype-devel openssl-devel wayland-devel libxkbcommon-devel libxcb-devel xcb-util-renderutil-devel vulkan-loader-devel mesa-libEGL-devel alsa-lib-devel dbus-devel
```

If rustup is not installed, use the official instructions at [rustup.rs](https://rustup.rs), then reopen your
terminal (or source `~/.cargo/env`). Rust is pinned by `rust-toolchain.toml`; rustup installs/selects it when Cargo runs. Python 3.11+ is required.
Use your distro's normal GPU driver (a working Vulkan-capable driver on Linux). These commands are build
prerequisites, not a claim of clean-machine/native acceptance on every desktop. GNU tar/coreutils/findutils
are normally already supplied on Linux. Windows, musl/Alpine and Linux ARM are not currently supported by
this source installer; it fails explicitly rather than downloading a wrong-architecture runtime.

## 2. Clone and build (one command)

```sh
git clone https://github.com/last-refuge/pipkin.git
cd pipkin
scripts/setup.sh --run
```

Already have the checkout? `git pull --ff-only`, then run the same command. First build downloads Cargo dependencies and may take several minutes. A prerequisite check without downloads:

```sh
scripts/setup.sh --check
```

`setup.sh` without flags builds and checks only. `--run` launches with the Pipkin checkout as the project,
so you can use Pipkin to work on Pipkin. Connect your provider/account, choose an available model, and send
a small prompt deliberately (real requests may be billed). Subagents are included by default; Preferences
→ Allow subagents can block new calls. Model invocation and provider availability still determine actual use.

## 3. Run again, or install for your user

```sh
scripts/run.sh                         # last successful native source build; no rebuild
scripts/run.sh --project /path/to/work  # override the default Pipkin project
scripts/setup.sh --install             # build, check and install; no sudo
```

- **Linux:** uses the normal versioned installer under `~/.local`, including desktop launcher/icons, notices,
  built-in engine, offline checks and rollback. Launch from the app menu or `~/.local/bin/pipkin`.
  For a pacman-managed package rather than a per-user install, use the advanced `makepkg` workflow in
  `docs/getting-started.md`/`packaging/PKGBUILD`.
- **Mac:** builds/verifies a complete app (without needing DMG creation/mounting) and installs it into
  `~/Applications/Pipkin.app`. Open it in Finder or with `open ~/Applications/Pipkin.app`. Only an existing
  app with Pipkin's bundle identifier can be replaced; unrelated apps/symlinks are refused.

Your provider credentials, conversations and app data are not deleted/reset by setup or installation.
Immutable source builds are retained under `dist/source-setup`; no Pi checkout is fetched or patched.

## Develop while Pipkin runs

Ask Pipkin to edit this checkout, then run `scripts/setup.sh` to produce the next build. Every build gets an
**immutable native build directory**. Only after the offline probe passes is `current` switched atomically;
a failed build leaves the previous working build runnable. Rebuilding does not overwrite the files used by
your running app/engine. Old build trees are retained; use the explicit cleanup command below to prune unused ones.

To use the new build, **Quit the old Pipkin instance** (⌘Q on Mac, Ctrl-Q on Linux), then `scripts/run.sh`
or open the newly installed app. Closing a Mac window is not Quit.

## Direct native build and packaging

```sh
cargo run -p pipkin-app --release
cargo run -p pipkin-app --release -- --data-dir /tmp/pk-native
scripts/package.sh                 # dist/native/pipkin-native-*.tar.zst
scripts/test-native-package.sh     # installed migration, coding and restart in scratch profiles
```

Native engine data lives in `~/.local/share/pipkin/engine` and configuration/credentials in
`~/.config/pipkin/engine`; an explicit `--data-dir DIR` keeps both under `DIR`. Pi source profiles
remain unchanged by import. Claude subscription browser and copy-code sign-in are available in
Settings → Connections. `pipkin --diagnose --probe` checks an isolated native worker without accounts.
`--engine native` remains an optional alias. Legacy Pi runtime flags are retired.

## If something fails

- Missing tools: setup lists the missing prerequisites and stops; use the platform command above.
- Native compile/library errors: install the named development/runtime library through your distro. The
  script does not weaken platform checks or claim compatibility with every glibc/desktop/GPU combination.
- Cargo download/checksum failure: fix connectivity and retry without bypassing verification.
- Offline handshake succeeds but provider fails: use account settings/model refresh. Do not delete history
  or repeatedly resend file-changing prompts as an auth test.

## Qualification evidence

Source-setup fixtures cover supported/rejected targets, unidentified output refusal, Rust-only builds,
isolated probes, immutable rebuilds and failed-probe preservation of the last good build. Native package
checks cover installed migration, real coding tools, restart and archive restoration without Pi/Node.
Mac bundle fixtures exercise signing/extraction and app-only installation separately from actual hosted
native builds. See [conversion evidence](../llm-docs/pipkin-rust-engine-evidence/runtime-retirement.md)
and [hosted qualification](../llm-docs/pipkin-rust-engine-evidence/hosted-qualification.md).
Historical Pi-paired source-install measurements remain in Git before the native build-path retirement;
they do not qualify the new package. Interactive macOS, additional desktops, signing and clean-machine
acceptance remain separately tracked. Notice retention is not legal clearance.


## Keep build storage manageable

Development and test builds use line-table debug information and disable incremental compilation.
Backtraces retain file/line information, but full variable/type debugging requires an explicit
profile override. This saves disk space at the cost of slower recompilation after edits. Release
settings are unchanged. These settings do not remove existing artifacts; the next build/test
recreates only the artifacts it needs.

On **Linux**, run this from the checkout:

```sh
python3 scripts/clean-builds.py                 # preview only; never deletes
python3 scripts/clean-builds.py --apply         # prune unused immutable source builds
python3 scripts/clean-builds.py --keep 5        # preview keeping the five newest instead of three
python3 scripts/clean-builds.py --debug          # also preview Cargo's debug-artifact cleanup
python3 scripts/clean-builds.py --debug --apply # also clean target/debug; next tests rebuild
```

The default retains the three newest source builds, plus `current` and any builds used by
running processes or held by source-launch locks. Only marked setup-owned directories under
`dist/source-setup/builds` are eligible; unknown names are retained and invalid markers/symlinks
cause refusal. Sizes are allocated-block estimates; shared filesystem extents can affect the
space actually recovered. Installed versions, app data, conversations, `target/release`, and
unrelated `dist` files are never cleanup targets. No automatic schedule or deletion is enabled.

The command refuses while Cargo/compiler/test activity or setup/packaging is detected for this
checkout. Setup and `scripts/run.sh` coordinate with cleanup through directory locks; the launcher
keeps its particular immutable build locked across exec. Process checks also protect older
launchers. Don't directly launch historical build binaries while cleanup runs; use `scripts/run.sh`
or the installed launcher. `--debug` delegates to Cargo's own build locking and refuses if a
running process is using `target/debug`. Custom Cargo target environment overrides are rejected.

Process inspection is Linux-only and refuses when app/build processes cannot be inspected.
The two non-dumpable systemd user-session services are specifically recognized; opaque app
processes are never silently skipped. macOS cleanup is not implemented; setup/launch still work.
Tests use disposable files/processes: `python3 scripts/test-clean-builds.py` and
`python3 scripts/test-source-setup.py`.
