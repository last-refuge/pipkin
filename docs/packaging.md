# Packaging, install, upgrade and diagnostics

## What gets installed

```
/usr/bin/pipkin                         the application
/usr/lib/pipkin/engine/                 a paired Pi engine (sources, production node_modules, engine.json)
/usr/lib/pipkin/runtime/                private checksum-pinned Node binary, manifest and full Node LICENSE
/usr/share/applications/pipkin.desktop  launcher entry (StartupWMClass=pipkin)
/usr/share/licenses/pipkin/             MIT licence for Pipkin, plus the font and icon licences
/usr/share/doc/pipkin/README.md         installed-help index (not the source marketing README)
/usr/share/doc/pipkin/SECURITY.md       private reporting and current fix/support policy
/usr/share/doc/pipkin/docs/             onboarding, recovery, compatibility and labelled historical notes
/usr/share/doc/pipkin/build-info.json   source/binary identity, compatibility and public build inputs
/usr/share/doc/pipkin/third-party/      conservative dependency inventory, notices and MPL source
/usr/share/doc/pipkin/runtime-inventory.json  staged native format, direct shared libraries and ABI references
/usr/share/icons/hicolor/{128,256,512}x…/apps/pipkin.png
```

Installed help includes current Markdown guides and the qualification plan, without requiring an adjacent source
checkout to read setup/privacy/recovery instructions. Developer commands and screenshot/fixture links still require
the full source repository; these are not installed source tooling. `scripts/test-package-docs.py dist/stage` checks
that packaged guides match their source bytes and is required by CI.

The paired engine declares `requiresBundledNode` and runs on the package's private, checksum-pinned Node 22.23.3 (target `x86_64-unknown-linux-gnu`). Neither the installer nor the Arch package needs a system Node to run. Building the package still requires Node/npm. `engine.json` records the engine version, the protocol it speaks (8) and the oldest Pipkin it supports. Pipkin refuses an engine with another
protocol, an incomplete one, or one that needs a newer Pipkin, and says why (`pipkin --diagnose`).

With no flags, Pipkin looks for the engine at `PIPKIN_ENGINE_DIR`, then `../lib/pipkin/engine` relative to its own
binary (so a relocated prefix works), then `/usr/lib/pipkin/engine`. When it finds one it launches and owns it:
server coordination profile under `~/.pi/server`, credentials/configuration under Pi's `~/.pi/agent`, and durable
experimental sessions under `<agent dir>/experimental/sessions/<session ID>/` (`meta.json` and `session.sqlite`).
The desktop cache and journal are separate; see [data ownership and recovery](support.md). `--pi-repo` (a checkout) and
`--pi-dir` / `--pi-server-id` (an engine already running) still win, for development.

## Building

```
scripts/package.sh [PI_CHECKOUT]      # dist/pipkin-<ver>-<arch>.tar.zst and dist/stage/
cd packaging && makepkg -si           # an Arch package; set PI_CHECKOUT to the clean pinned checkout
scripts/verify-install.sh [PKG]       # unpack into a scratch prefix and check it with a bare environment
scripts/verify-install.sh --full      # ...and run the real-engine workflow tests against the unpacked engine
```

Release engine sources must match the full revision in `packaging/pi-engine-revision` and have a clean Git
working tree. `scripts/check-engine-source.sh PI_CHECKOUT` verifies both before staging. Prepare a clean checkout
of that revision with `npm ci`. Release builds restore the checksummed provider-data snapshot paired with the pin;
they do not fetch a mutable public model catalog. `scripts/stage-model-data.sh PI_CHECKOUT` can restore/validate that
snapshot for development-engine qualification. See `packaging/engine-model-data.md` for provenance and pin updates.
`PIPKIN_ENGINE_DEV=1 scripts/package.sh PI_CHECKOUT` is an explicit development override, not release qualification.

`scripts/package.sh` downloads the Linux Node archive over HTTPS during the build, verifies its pinned SHA-256, and extracts only the exact regular Node executable and full LICENSE into a private versioned runtime. Installation itself performs no network request or global Node changes. The hash is a build-input integrity check, **not publisher authentication**; review the release signature policy before trusting an artifact. `scripts/build-engine.sh` stages the identified tracked sources through Git archive, not arbitrary ignored build
output. It separately restores/validates pinned generated provider JSON and copies installed workspace dependencies,
then prunes dev dependencies offline. The manifest records the full source revision, generated-data manifest hash,
protocol, minimum client and whether the development override was used. It does not yet identify every build input
or certify third-party dependency provenance; those audits and CI qualification remain release gates.

Packaging generates a conservative Linux Rust/staged npm inventory and copies found package/vendored notices.
It also provides the current MPL-2.0 dependency's source. Missing declarations/notices and inherited-license
applicability are explicitly flagged. See [bundled-licenses.md](bundled-licenses.md): this is a partial audit, not
release clearance. `runtime-inventory.json` inspects staged native files with readelf, without executing them;
it records direct shared-library/ABI references and flags foreign artifacts. Static/dlopen and vendored/native
provenance still require review. The local build references GLIBC_2.44; do not assume the artifact works on older
Linux systems. Python 3 and binutils are build/qualification dependencies, not new installed-app runtime requirements.

`pipkin --version` and `--diagnose` report the compiled full app revision and explicit dirty/unknown state.
`build-info.json` records app version/revision/dirty state and binary hash, protocol/schema, full engine revision,
generated-data fingerprint, source/staged dependency lock hashes, Cargo lock/toolchain hashes, Rust/Cargo/Node/npm
versions, platform/architecture and whether custom Rust flags were present. Raw flag values and arbitrary environment
variables are not included. This identifies inputs and the actual binary; it is not a claim of bit-reproducible builds
or an authenticity signature. `release.sh` rejects dirty/unidentified app sources as well as development engines.

`.github/workflows/qualification.yml` runs locked Rust checks, disposable packaging policy tests, a clean pinned
engine with scripted-provider tests, and bundled-engine/installer tests. Actions and Node are pinned; the Rust
version comes from `rust-toolchain.toml`. Logs and engine metadata are retained as CI evidence, not published
release artifacts. The Ubuntu runner is headless automation, not an additional supported native desktop. Workflow
presence is not a green gate: refer to `v1-release-gates.md` for observed run results.

The engine is source plus dependencies run by Node's
TypeScript support, as Pi's own `pi-test.sh` does; Pi does not yet publish a compiled experimental server, so a
compiled engine is a Pi-side follow-up. The original installed-size measurement was about 850 MiB. The
2026-10-06 v1 baseline stages **453 MiB total**, with **200 MiB of engine** and an **89,114,048-byte tarball**
(about 85 MiB). Sizes vary with engine dependencies and binary debug information; see
[v1-release-gates.md](v1-release-gates.md) for the exact identified artifact.

`makepkg` needs `!lto` (set in the PKGBUILD): its LTO flags break linking the bundled SQLite.

## Upgrade and rollback

- **Application and engine upgrade together**: one package replaces `/usr/bin/pipkin` and `/usr/lib/pipkin/engine`.
  Quit the app and its owned engine before replacing an Arch installation; do not assume an in-flight process can
  safely keep reading files from a replaced engine directory. Generic versioned installs retain the old directory,
  but interruption/ownership semantics still apply (see [support](support.md)).
- **Data upgrades**: Pipkin's database migrates in place on first start. Before migrating it writes a backup of the
  old schema next to the database; a test upgrades from every earlier schema and checks the draft survives and a
  backup exists.
- **Rollback**: reinstall the previous package (`pacman -U` from the cache). A database written by a newer Pipkin
  is refused untouched with the message "schema N is newer than this build supports"; restore the backup made at
  that upgrade (the `.bak` for the older schema) to go back. Drafts typed after the upgrade are not in that backup.
  The generic installer's `--rollback` rechecks the retained app/runtime checksums and an offline scratch-profile
  handshake before changing the selected version. Failure leaves the current selection and launcher intact.
  This checks the package; it does not prove compatibility with your existing profile's schema.
- **Damage**: a corrupt database is set aside and the newest intact backup restored, with a notice.

## Diagnostics

```
pipkin --version
pipkin --diagnose            # versions, paths, database health, engine manifest and checks, Node, display, log tail
pipkin --diagnose --probe    # also starts a throwaway engine (offline, empty profile) and checks it answers
```

The report replaces the home directory with `~` and exits non-zero when something blocks Pipkin. It does not
read the credential store, but **includes an engine-log tail and arbitrary error text**, which can contain secrets,
private endpoints, prompts or source paths. Home substitution is not comprehensive redaction. Review and redact it
before sharing; do not assume it is safe to paste into an issue. The engine's own output is in
`<data dir>/engine.log` (default `~/.local/share/pipkin/`). See [diagnostics privacy](support.md#diagnostics-privacy).

## Soak

`PIPKIN_PI_REPO=<engine dir> PIPKIN_SOAK_ROUNDS=1500 cargo test -p pipkin-app --release many_prompts -- --ignored --nocapture`
sends prompts through one conversation of a real engine and fails if memory or open files keep growing.
**Historical, not current-candidate qualification:** a prior 1500-prompt bundled-engine run reported app 22 -> 52 MiB,
engine 633 -> 700 MiB, open files 15 -> 19. Current RC soak and real-window day-long evidence remain open.

## Generic Linux install, upgrade and rollback

`scripts/package.sh` also leaves `install.sh` at the root of the release tarball. It checks the Linux architecture, essential system tools, the packaged app/runtime hashes, and the copied engine with an offline throwaway-profile handshake before switching the active version. Progress uses color on interactive terminals and plain ASCII otherwise; it cannot verify a Vulkan driver or grant the archive publisher trust. A missing desktop library/unsupported ABI is reported without silently changing system packages. The script hands provider/project/model setup to the Pipkin GUI, never collecting credentials in the terminal. It installs into `~/.local`
(or `--prefix DIR`) under `lib/pipkin/versions/<version>/` with a `current` link, so:

```
./install.sh                # install or upgrade; keeps the newest 2 versions (--keep N)
./install.sh --list         # versions, with the current one marked
./install.sh --rollback     # switch back to the previous version in one step
./install.sh --uninstall    # remove links and versions; your drafts and history are not touched
```

Each version carries its paired engine (the binary finds it relative to itself), reducing accidental mixed
app/engine installs. This is not a guarantee of a healthy upgrade or schema-compatible rollback. Verify the
identified pair and preserve compatible app/engine data backups before relying on an old version as a fallback. `scripts/test-install.sh` checks
install, upgrade, list, rollback and uninstall in a scratch prefix, including that data survives.

## Releases: checksums and signatures

```
scripts/release.sh                       # dist/release/: the tarball and SHA256SUMS
PIPKIN_SIGN_KEY=<gpg key id> scripts/release.sh   # also SHA256SUMS.asc (detached signature)
scripts/verify-release.sh DIR [--require-signature]   # what a downloader runs
```

Checked here with a throwaway gpg key (signature verifies; a modified tarball fails the checksum). No real
release key exists, nothing is published, and there is no in-app update check: an update is "download, verify,
`./install.sh`", with `--rollback` selecting a retained prior package, subject to the schema/backup requirements above.

## Native engine candidate (development)

Pipkin's built-in Rust engine can be packaged on its own, with **no Node, npm or Pi** used to build it or
shipped in it. This is a development candidate for x86_64 Linux only: **Pi remains the default engine**, the
default `scripts/package.sh`, `release.sh`, Pi PKGBUILD mode and macOS packaging are unchanged, nothing is signed, and no
new platform is qualified by it.

```
scripts/package-native.sh               # dist/native/pipkin-native-<ver>-<arch>.tar.zst and dist/native/stage/
scripts/package.sh --engine native      # the same; `--engine pi` (or no flag) is the existing Pi package
scripts/test-native-package.sh          # build, package, install into a scratch prefix, bare-env probe
(cd packaging && PIPKIN_ENGINE=native makepkg)  # Arch candidate, no Pi checkout or npm build dependency
python3 scripts/verify-native-arch-package.py packaging/pipkin-*.pkg.tar.zst REPORT.json
```

Build requirements: Rust (pinned by `rust-toolchain.toml`), a C compiler/cmake for the bundled SQLite and zstd,
Python 3 and binutils (inventories), zstd and coreutils. Build identity is written by the POSIX
`scripts/write-native-build-info.sh` instead of the Node `write-build-info.mjs` (which the Pi package still uses).

The Arch recipe defaults to Pi during qualification; `PIPKIN_ENGINE=native` selects the native
candidate explicitly and stages it in makepkg's private build directory. Both modes produce the
`pipkin` package and replace one another rather than installing conflicting binaries. Native
metadata omits npm from build dependencies; runtime desktop/GPU/Git requirements are unchanged.
Native mode retains Cargo's release line tables and disables makepkg's extra full-debug override;
`!strip` still preserves the binary exactly as identified by `build-info.json`.
`makepkg` alone builds a package without installing it or installing dependencies. Inspect the
candidate and back up compatible app/engine data before an eventual `pacman -U` replacement.
The native desktop entry passes `--engine native`; the binary's no-flag default remains Pi.

The archive contains `usr/bin/pipkin` (the app; it runs the engine as a private `pipkin --engine-worker` child),
`usr/lib/pipkin/native-engine.json` (a marker naming the engine version/protocol/schema), a desktop entry that
launches `pipkin --engine native`, icons, licences, installed help (`packaging/native-README.md`),
`build-info.json`, a Rust-only `third-party/inventory.json` and `runtime-inventory.json`. There is no
`lib/pipkin/engine` or `lib/pipkin/runtime`; packaging fails if any Node/npm/Pi file appears. `./install.sh`
recognises the marker, skips the Node checks, verifies the binary hash and runs
`pipkin --diagnose --probe --engine native` in a throwaway profile before activating the version
(`<ver>-native-<binary hash>`), so `--list`/`--rollback` work across Pi and native versions in one prefix.

- **Selecting it:** the engine is always chosen explicitly with `--engine native` (the desktop entry does this).
  The marker never changes the default: `pipkin` without the flag still looks for a Pi engine, and
  `pipkin --diagnose` on a native install says the Pi engine is missing and points to `--engine native`.
- **Isolation:** native data lives in `~/.local/share/pipkin/engine` and configuration (including provider
  credentials) in `~/.config/pipkin/engine`, separate from Pi's `~/.pi`. `--data-dir DIR` (or `PIPKIN_DATA`)
  puts everything under `DIR` (`DIR/engine`, `DIR/engine-config`) for disposable tests.
- **Rollback:** reinstall or `./install.sh --rollback` to a Pi package. Native data is left in place and is not
  migrated back: history written by the native engine is native-only and Pi does not read it.
- **Claude subscription:** browser and copy-code sign-in are available under Settings → Connections.

```
pipkin --version --engine native             # adds engine version, protocol and schema to the version line
pipkin --diagnose --engine native            # engine identity, data/config directories, database, log tail
pipkin --diagnose --probe --engine native    # also starts a throwaway worker: Hello + ListSessions, then stops
```

The native probe starts this binary as a worker in a new scratch directory with a cleared environment (no
inherited API keys or proxies), no network access and no Node or Pi, checks the protocol/schema and an empty
session list, shuts it down and deletes the scratch directory; your own engine data is not touched. The same
privacy caution as the Pi report applies to the log tail.

Measured sizes for one candidate (2026-10-11, unstripped release binary with debug information, like the Pi
package's `!strip`): installed version **345,345,187 bytes** (binary 338,998,112), archive **72,638,449 bytes**.
A stripped copy measured 58,493,851 bytes installed / 20,599,672 bytes archived (measurement only; the package
is not stripped). For comparison, the **historical** 2026-10-06 Pi baseline staged 453 MiB with a
89,114,048-byte tarball, before the private Node runtime was added. See
`llm-docs/pipkin-rust-engine-evidence/packaging.md` for commands, inventories and what is unverified.

## Display servers

The binary is built with both `wayland` and `x11`; it uses Wayland when `WAYLAND_DISPLAY` is set, else X11.
The Arch package depends on `libxkbcommon-x11` and `libxcb` for that. See `platforms.md` for what was checked.
