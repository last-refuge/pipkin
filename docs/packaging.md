# Packaging, install, upgrade and diagnostics

The default package contains one Rust binary, the native-engine identity marker, desktop launcher,
icons, documentation, license inventory and native library inventory. It ships no Pi source/runtime,
Node executable or npm tree. Native worker, in-process and explicitly attached native host modes
remain supported. `--engine native` is optional; Pi launch flags are retired.

```sh
scripts/package.sh                          # dist/native/pipkin-native-<version>-x86_64.tar.zst
(cd packaging && makepkg)                   # native Arch package
scripts/test-native-package.sh              # build + isolated installed workflow verification
scripts/verify-install.sh PATH.tar.zst       # verify an existing native archive
scripts/release.sh                          # clean identified checkout; optional PIPKIN_SIGN_KEY
```

`release.sh` assembles files locally and never publishes a release. Publication is the `Release` GitHub
Actions workflow (`.github/workflows/release.yml`): a `v<version>` tag whose version matches the workspace
builds the Linux archive and the macOS ZIP/DMG, verifies them, and publishes them with one `SHA256SUMS`
and the matching `CHANGELOG.md` section as the release body. A `workflow_dispatch` run of it is a dry run.
No signed release is claimed.
Builds use locked Rust dependencies and pinned source/vendor inputs. Python, binutils and native
build libraries are prerequisites; Pi/Node/npm are neither build nor runtime dependencies.
`build-info.json` identifies source, binary, protocol/schema and public build inputs. The inventory
retains ported Pi/Anthropic/minimatch/brace-expansion notices and flags incomplete license evidence;
it is not legal clearance. Native ABI compatibility follows the actual build machine and libraries.

The Linux installer uses a versioned user prefix, checks checksums/native libraries, probes a private
worker without accounts and atomically activates the new binary. New Pi packages are rejected.
Explicit rollback can reactivate an intact historical Pi installation already retained in that prefix;
this preserves recovery and does not provision a Pi/Node dependency for current Pipkin.

Linux CI runs required workspace checks and installed coding/migration/recovery with Node/npm/Pi
execution traps. Experimental macOS CI builds and verifies an ad-hoc signed native app and ZIP;
Finder, Gatekeeper/notarization and interactive acceptance are separate. Historical paired-Pi
packaging evidence remains in Git and the evidence ledger.

## Upgrade and rollback

- **Application and engine upgrade together**: the engine is compiled into `/usr/bin/pipkin`.
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

Checked here with a throwaway gpg key (signature verifies; a modified tarball fails the checksum). Releases are
published on [GitHub Releases](https://github.com/last-refuge/pipkin/releases) with checksums only: no real
release key exists yet, so `SHA256SUMS` proves integrity, not publisher identity. There is no in-app update
check: an update is "download, verify, `./install.sh`", with `--rollback` selecting a retained prior package,
subject to the schema/backup requirements above.
