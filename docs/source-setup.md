# Build, run and install Pipkin from source

No separate Pi checkout, manual patching, system Node/npm or server startup is needed. Source setup downloads
checksum-pinned Node **and private npm build tools**, fetches the exact paired Pi revision, stages Pipkin's
OAuth and **default-enabled native subagent** patches, builds the Rust app and verifies an offline handshake.
It does not authenticate a provider or make paid requests. Do not build as root.

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

Already have the checkout? `git pull --ff-only`, then run the same command. First build downloads source,
npm dependencies and Cargo crates and may take several minutes. A prerequisite check without downloads:

```sh
scripts/setup.sh --check
```

`setup.sh` without flags builds and checks only. `--run` launches with the Pipkin checkout as the project,
so you can use Pipkin to work on Pipkin. Connect your provider/account, choose an available model, and send
a small prompt deliberately (real requests may be billed). Subagents are included by default; Preferences
→ Allow subagents can block new calls. Model invocation and provider availability still determine actual use.

## 3. Run again, or install for your user

```sh
scripts/run.sh                         # last successful paired source build; no rebuild
scripts/run.sh --project /path/to/work  # override the default Pipkin project
scripts/setup.sh --install             # build, check and install; no sudo
```

- **Linux:** uses the normal versioned installer under `~/.local`, including desktop launcher/icons, notices,
  paired engine and private Node, offline checks and rollback. Launch from the app menu or `~/.local/bin/pipkin`.
  For a pacman-managed package rather than a per-user install, use the advanced `makepkg` workflow in
  `docs/getting-started.md`/`packaging/PKGBUILD`.
- **Mac:** builds/verifies a complete app (without needing DMG creation/mounting) and installs it into
  `~/Applications/Pipkin.app`. Open it in Finder or with `open ~/Applications/Pipkin.app`. Only an existing
  app with Pipkin's bundle identifier can be replaced; unrelated apps/symlinks are refused.

Your provider credentials, conversations and app data are not deleted/reset by setup or installation.
Build/runtime downloads are isolated under `dist/source-setup`; no daily-use Pi checkout is patched.
The shipped runtime is still minimal: npm is only a checkout-local build tool, not a global installation
or part of the installed runtime.

## Develop while Pipkin runs

Ask Pipkin to edit this checkout, then run `scripts/setup.sh` to produce the next build. Every build gets an
**immutable app/engine/runtime directory**. Only after the offline probe passes is `current` switched atomically;
a failed build leaves the previous working build runnable. Rebuilding does not overwrite the files used by
your running app/engine. Old build trees are retained (they use disk space; remove them only when unused).

To use the new build, **Quit the old Pipkin instance** (⌘Q on Mac, Ctrl-Q on Linux), then `scripts/run.sh`
or open the newly installed app. Closing a Mac window is not Quit. Do not launch against an old external
`--pi-dir` server: it can lack subagents even when the new client is built. Stop a separately launched server
in its own terminal if you intend to replace it. Do not use `PIPKIN_ENGINE_DEV=1` with source setup.

## Native engine candidate (development)

Pipkin also has a built-in Rust engine that needs **no Node, npm or Pi checkout** to build or run. It is a
development candidate: **Pi remains the default**, `setup.sh` still builds the Pi pairing, and the native
engine is only used when you ask for it with `--engine native`. Linux x86_64 only; no additional platform
is qualified by this path.

```sh
cargo run -p pipkin-app --release -- --engine native              # run from source with the built-in engine
cargo run -p pipkin-app --release -- --engine native --data-dir /tmp/pk-native   # fully isolated profile
scripts/package-native.sh                 # archive without Node/npm/Pi: dist/native/pipkin-native-*.tar.zst
scripts/test-native-package.sh            # build, package, install in a scratch prefix and probe it
```

Only the Rust toolchain plus the Linux prerequisites above (and Python 3/binutils for the package
inventories) are needed; no Node download happens. Native data lives in `~/.local/share/pipkin/engine`
and its configuration and credentials in `~/.config/pipkin/engine`, separate from Pi's `~/.pi`; an explicit
`--data-dir DIR` (or `PIPKIN_DATA`) keeps both under `DIR`. To go back, run without `--engine native` (or
reinstall/roll back to a Pi package): native data is left untouched, and history written by the native engine
is native-only, so Pi will not show it. Claude subscription sign-in is not available in native mode yet.
`pipkin --diagnose --probe --engine native` checks a throwaway native worker without credentials or network.
See [packaging](packaging.md#native-engine-candidate-development) for the archive layout and installer.

## If something fails

- Missing tools: setup lists the missing prerequisites and stops; use the platform command above.
- Native compile/library errors: install the named development/runtime library through your distro. The
  script does not weaken platform checks or claim compatibility with every glibc/desktop/GPU combination.
- Download/checksum failure: retry after fixing network access; a bad cached Node archive must be removed
  from `dist/source-setup` before retrying. Never bypass checksum verification.
- “Attached engine does not support subagents”: quit the old app/server, rerun setup and launch via `run.sh`,
  not a vanilla Pi checkout. Setup refuses success unless the staged manifest identifies the subagent patch.
- Offline handshake succeeds but provider fails: use account settings/model refresh. Do not delete history
  or repeatedly resend file-changing prompts as an auth test.

## Qualification evidence

The implemented source path passed a real x86_64 Linux build and **per-user install into a disposable HOME**,
including the paired Node 22.23.3/subagent-enabled engine and offline handshake. The installed manifest retained
`pipkinSubagentsSha256` and `requiresBundledNode`; 28 staged guide/input files matched current source. No owner
profile, global Node/npm or desktop configuration was modified. Working-delta logs:
`/tmp/pipkin-source-setup-{live,install,workspace,clippy}.log`. Workspace tests, Clippy and formatting passed.

Fixtures cover supported/rejected targets, unknown output refusal, private npm versus minimal runtime,
subagent omission refusal, immutable rebuilds and failed-probe preservation of the last good build. Mac bundle
fixtures verify the app-only install path skips DMG tooling but retains signing/extracted-app probe requirements.
Native Apple Silicon run [37709798124](https://github.com/last-refuge/pipkin/actions/runs/37709798124)
on clean `5a873fa3646599613f9333d12fe8bd74949a826f` passed the actual one-command source build: private
Node/npm, patched engine, native binary and **offline handshake in 2.0s**. The subsequent complete ZIP/DMG
signature and copy-install/runtime checks passed (1.8s / 1.7s probes). This is native automated evidence,
not Finder interaction or a real paid provider/subagent invocation. Follow-up native run [37711156610](https://github.com/last-refuge/pipkin/actions/runs/37711156610)
on clean `d03ffe812cbcbcd60a141614d556ad6386bccdf7` also passed **`scripts/setup.sh --install`**:
private build/probe, complete app-only package (no DMG dependency), and actual per-user install to
`/private/tmp/pipkin-ci/source-home/Applications/Pipkin.app`, with signature verification after copying.
The source and app-only probes took 2.9s and 2.0s. The HOME was disposable, not the runner's regular
Applications folder. Evidence: `/tmp/pipkin-source-setup-macos-install-full.log`.

[Qualified ZIP/DMG artifact](https://github.com/last-refuge/pipkin/actions/runs/37709798124/artifacts/11521547936):
`pipkin-macos-ARM64-5a873fa3646599613f9333d12fe8bd74949a826f`. DMG SHA-256:
`d9c3ff0743246c29686be9051017c83db45f6e0eb0daf605332fe1249a78e80c`. Native log:
`/tmp/pipkin-source-setup-macos-full.log`. Owner-local Finder interaction, actual provider/subagent invocation and unqualified distro desktops still
require their own observation; the automated native source-install path is qualified, not every desktop
experience. Notice retention is not legal/security clearance.

See [getting started](getting-started.md), [support/privacy](support.md), and [subagent limitations](../packaging/pi-subagents/README.md).
