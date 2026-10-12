# Native Arch package

2026-10-12, following `c64e59b`, protocol 12/schema 10 unchanged. Native remains opt-in.

## Implementation and actual build

The existing Arch recipe always built Pi and required npm. `PIPKIN_ENGINE=native makepkg`
now builds/stages the native candidate in makepkg's build directory, without a Pi checkout
or npm build dependency. Both selectors retain the package name `pipkin`, with pkgrel 3;
the native desktop entry explicitly selects native. Default/Pi mode preserves its existing
build path. Invalid selectors fail before building. `makepkg --printsrcinfo` verifies both
dependency declarations and selector rejection was executed separately.

Two actual unsigned x86_64 Arch artifacts were built locally with makepkg 7.1.0, the pinned
Rust/Cargo 1.99 toolchain, Cargo offline and Node/npm/npx/Pi trap executables first on PATH.
No trap was invoked. The first build used makepkg's global full-debug override, producing
a needlessly large payload. Native mode now adds `!debug` while retaining `!strip`/`!lto`:
Cargo's release line tables remain intact, with no additional full-DWARF override or stripping
after the binary hash is recorded. Other host makepkg compilation flags remain in effect.

| Actual native Arch artifact | Installed payload bytes | Archive bytes |
| --- | ---: | ---: |
| Initial full-debug override | 1,046,267,353 | 211,521,077 |
| Cargo release line tables | 361,217,499 | 76,328,657 |

This removes 65.48% of the initial payload and 63.91% of its archive size. It is a comparison
of two native Arch build configurations, **not a new Pi/native comparison**. Previously
recorded generic package comparisons retain their own identities and measurements.

The final binary SHA-256 is
`e976a8519458b295b0a20c42237b72fd1d55d579f1567a0ebe226c52b03d9c15`, built from `c64e59b`
plus working changes. Its archive SHA-256 is
`ad40ef39a004e2dcca5de00615f42c3d446dca6ebb8367891483f766371c3378`.
Exact artifact identities, package metadata, recipe hashes and measurements are in
`fixtures/native-installed/arch/2026-10-12-{native,build-comparison}.json`.
`.BUILDINFO` reports the global makepkg options; the identified PKGBUILD overrides them.
Do not mistake its global `debug`/`strip` values for the recipe's effective options.

The Rust toolchain on this machine is user-managed rather than registered in pacman as
`rust`/`cargo`. `pacman -T` confirmed all other declared dependencies present; direct
Rust/Cargo version checks confirmed the pinned toolchain. These builds used `--nodeps`
for that local setup, not `-s` or `-i`. No system packages, user installation or display
configuration changed. The normal documented command retains makepkg dependency checking.

## Verification and limits

`scripts/verify-native-arch-package.py` verifies the actual `.PKGINFO`, `.BUILDINFO` and
`.MTREE`, exact recipe/binary identities, current installed guides, explicit native desktop
selection, absence of Pi/Node payload/dependencies, Rust-only inventory and copied notice
hashes. Source provision for the existing MPL dependency is present; notice applicability
and provenance review flags remain open. This does not establish license clearance.

The extracted package binary passes an isolated offline diagnostic handshake and the real
local-provider workflow: import frozen fork histories, write/edit/bash/read, preserve full
retained output, restart without repeated settled effects, restore the closed data/config
archive with exact file modes/bytes and continue coding. Runtime Node/Pi traps remain unused.
The verifier's initial run used the wrong fixture directory and failed before the coding
workflow; correcting it to the existing `fixtures/native-fork-import` corpus retained all
assertions. No engine regression is inferred from that harness error.

Required workspace checks pass: **1,640 passed, 0 failed, 56 ignored**, clean Clippy/all-targets
and formatting. `PKG-PKGBUILD` advances from not started to observed; no row becomes fully
qualified from this artifact alone. The package was built and its extracted payload exercised;
a system pacman installation, clean-chroot build, GUI acceptance, hosted CI, signatures,
full provenance review and final native-default/Pi retirement are not claimed.
