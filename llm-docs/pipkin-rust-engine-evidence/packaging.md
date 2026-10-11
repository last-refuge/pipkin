# Native engine packaging evidence (RE-23, Linux scope)

Recorded 2026-10-11 on x86_64 Arch/Omarchy (Linux 7.2.5-3-omarchy, rustc 1.99.0, Python 3.14.7), branch
`worktree-agent-aba083cda5c024894` after merging `native-engine` (38556a6 for the measured package; later merged up to ae95240). Every item is labelled
**measured** (read from a run recorded here), **observed** (seen in a run, not a number), or **unverified**.

Scope: a no-Node build/packaging path for the native-engine *candidate*. Pi remains the production default.
Not changed: the default engine, `scripts/package.sh` without `--engine`, `release.sh` default outputs,
PKGBUILD, macOS packaging and signing. No new platform qualification is claimed.

## What was built

| Piece | File | Notes |
| --- | --- | --- |
| Build identity without Node | `scripts/write-native-build-info.sh` (POSIX sh + coreutils) | Reads `pipkin --version --engine native`; records app version/revision/dirty, binary SHA-256, engine version/protocol/schema, desktop schema, Cargo.lock/toolchain hashes, rustc/cargo/python versions, platform/arch, custom-Rust-flags **boolean only**; `node`/`npm` are `null`. `write-build-info.mjs` is unchanged and still used by the Pi package. |
| Revision/dirty stamp | `crates/pipkin-app/build.rs` (unchanged) | Already stamps `PIPKIN_APP_REVISION`/`PIPKIN_APP_DIRTY` without Node. Engine protocol/schema come from the linked crates' constants (`pipkin_engine_protocol::PROTOCOL_VERSION`, `pipkin_engine::store::SCHEMA_VERSION`, `pipkin_engine::runtime::ENGINE_VERSION`), not a duplicated build-script value, so they cannot drift. `scripts/package-native.sh` sets `PIPKIN_REVISION` (the engine's existing compile-time `option_env!`) to the app revision (`-dirty` suffix on a dirty tree) so the worker's Welcome names its source. |
| Version line | `pipkin --version --engine native` | `pipkin <ver> (source <rev>, dirty: <d>, engine native <ver>, engine protocol <p>, engine schema <s>, schema <desktop>)`. The default `--version` line is byte-for-byte unchanged (the Pi `write-build-info.mjs` regex still matches it). |
| Package | `scripts/package-native.sh`, `scripts/package.sh --engine native` | Output under `dist/native/` (never `dist/stage` or `dist/pipkin-*.tar.zst`). Fails closed if any Node/npm/Pi file is staged or the license inventory lists npm packages. |
| Installer | `packaging/install.sh` | Detects `usr/lib/pipkin/native-engine.json`; skips Node checks; refuses a native archive that also carries `lib/pipkin/engine` or `lib/pipkin/runtime`; version name `<ver>-native-<binary sha12>`; probe `--diagnose --probe --engine native`. Desktop `Exec=` rewrite now keeps arguments after `pipkin` (the Pi entry has none, so its output is unchanged). |
| Diagnostics | `crates/pipkin-app/src/install.rs`, `diagnostics.rs`, `main.rs`, `controller.rs` | `--diagnose [--probe] --engine native`; native support metadata (Ctrl K → Copy diagnostics) when running with `--engine native`. |
| Runtime inventory | `scripts/write-runtime-inventory.py STAGE --native-engine` | Same readelf approach; `requiredNodeMinimum: null` and an `engine` note. |
| License inventory | `scripts/write-license-inventory.py` (unchanged) | Its `cargo metadata` walk from `pipkin-app` already covers the engine crates; with no staged engine it emits Rust packages only. |

## Commands run and results

| Command | Result |
| --- | --- |
| `scripts/test-native-package.sh` (full: release build, package, install), first run | **observed** all checks pass except one listing check that wrongly expected `./install.sh` (the archive stores `install.sh`); check fixed. |
| `scripts/test-native-package.sh` (full), final run on the clean committed tree `9a5e19e` (after merging native-engine `ae95240`) | **observed** 23/23 checks pass (adds the installed-docs check), `native package verified`; installed tree 345,187,990 bytes, binary 338,835,128, archive 72,572,288 (**measured**); probe `hello: protocol 1, engine 0.0.1 revision 9a5e19e…, schema 1; sessions in a fresh profile: 0`. |
| `PIPKIN_NATIVE_OUT=<scratch> scripts/package-native.sh` on the committed (clean) tree, then `scripts/test-native-package.sh --no-build <archive>` | **observed** 22/22 checks pass (before the installed-docs check was added), `native package verified`. Build: incremental release build 1m12s (**measured**, dependencies cached). |
| `scripts/test-install-paths.sh` (new; fixture archives, both installer branches, mixed native→Pi rollback) | **observed** 12/12 pass |
| `scripts/test-native-build-info.sh` (new; counterpart of `test-build-info.sh`) | **observed** pass |
| `scripts/test-runtime-inventory.py` (extended with `--native-engine`) | **observed** pass |
| `scripts/test-license-inventory.py`, `scripts/test-build-stamp.sh`, `scripts/test-build-info.sh` (Pi, Node-based, unchanged), `scripts/test-source-setup.py` | **observed** pass |
| `scripts/test-package-docs.py STAGE --native` (new mode) | **observed** pass inside the final `test-native-package.sh` run (it correctly failed on an earlier stage built before the docs edit) |
| `cargo test --workspace`, `cargo clippy --workspace --all-targets`, `cargo fmt --all --check` | **observed** 1179 passed / 0 failed / 53 ignored; clippy 0 warnings; fmt clean |
| `cargo test -p pipkin-app` new tests: 6 in `install::tests`, 1 in `diagnostics::tests`, 2 in `tests/native_diagnose.rs` (real binary, `env_clear` + `PATH=/usr/bin:/bin`) | **observed** pass |

What `test-native-package.sh` asserts (all **observed** passing):

* archive has `usr/bin/pipkin`, the marker, desktop entry, licences, `build-info.json`, both inventories and `install.sh`;
* no archive or installed path matches `node|npm|npx|node_modules|pi-test.sh|package(-lock).json|*.js|*.mjs|*.cjs|*.ts|*.node|usr/lib/pipkin/{engine,runtime}`;
* license inventory ecosystems == `{rust}`; runtime inventory has exactly `usr/bin/pipkin` and `requiredNodeMinimum: null`; build-info binary hash matches;
* `install.sh` (run under `env -i` with node/npm/npx/pi shims that record use) took the native path; desktop entry `Exec="<prefix>/bin/pipkin" --engine native` and `desktop-file-validate` passes;
* installed `pipkin --version` and `--version --engine native` under `env -i HOME=<scratch> PATH=/usr/bin:/bin`;
* installed `pipkin --diagnose --probe --engine native` under the same bare env exits 0: `engine probe: OK (hello: protocol 1, engine 0.0.1 revision df0da2693862e3749bdf77ffbe8330f6c542ef1c, schema 1; sessions in a fresh profile: 0; stopped cleanly in 0.0s)`;
* the probe created nothing in the (scratch) user profile; with shims first on PATH it still passes and **no shim was invoked**;
* Pi stays the default: plain `pipkin --diagnose` on the native install exits non-zero with `engine: NOT FOUND` and the `--diagnose --engine native` hint;
* uninstall removes links and versions; app data survives.

Host note (**observed**): this machine has no `/usr/bin/node`, so the bare-PATH runs could not have used a system Node in any case; the shim run covers hosts that do.

## Sizes (measured, bytes)

Native candidate, revision `df0da26` (clean; the final run at `9a5e19e` is within 0.1%, see above), release profile (`debug = "line-tables-only"`, unstripped, like the Pi package's `!strip`):

| Item | Bytes |
| --- | --- |
| `usr/bin/pipkin` | 338,998,112 |
| installed version directory (`du -sb`) | 345,345,187 |
| archive `pipkin-native-0.0.1-x86_64.tar.zst` | 72,638,449 |
| `usr/share/doc/pipkin` (of which `third-party/` 5,641,113; `docs/` 308,424) | 5,962,801 |
| icons / licences / marker | 366,369 / 17,407 / 199 |
| stripped copy of the binary (`strip`, measurement only) | 52,146,776 |
| stripped installed tree / its zstd archive (measurement only, not shipped) | 58,493,851 / 20,599,672 |

The performance worker's independent measurement (its branch, `performance.md`) was 51.3 MB stripped `pipkin`; consistent with the 52.1 MB here at a later revision.

Pi package comparison (**historical**, not re-measured: building it needs Node/npm and network, which this task did not use): `docs/v1-release-gates.md` records the 2026-10-06 baseline as tarball **89,114,048 bytes**, staged installation **453 MiB**, engine staging **200 MiB**, *before* the private Node runtime was added (2026-10-07), so the current Pi package is larger than that baseline. `fixtures/pi-sessions/BASELINE.md` measured the pruned Pi engine tree at 154,944,381 bytes (excl. `.git`), excluding Node. Derived: the unstripped native archive is ~18% smaller than the historical Pi tarball (72.6 vs 89.1 MB) and the installed tree ~27% smaller (345 MB vs 453 MiB = 475 MB); a stripped native package would be ~20.6 MB archived. These cross-revision comparisons are indicative only.

## Inventories (measured)

* `runtime-inventory.json`: one ELF artifact (`usr/bin/pipkin`, x86-64), NEEDED `ld-linux-x86-64.so.2, libc.so.6, libgcc_s.so.1, libm.so.6, libxcb.so.1, libxkbcommon-x11.so.0, libxkbcommon.so.0`; highest referenced `GLIBC_2.44` (built on this rolling distribution; do not assume older glibc works). Vulkan/Wayland/fontconfig are loaded at runtime (dlopen), outside this direct-NEEDED inventory, as for the Pi package.
* `third-party/inventory.json`: 607 Rust packages, 0 npm; 34 flagged `reviewRequired` (same policy as the Pi package). Engine-relevant declared licences: reqwest `MIT OR Apache-2.0`, rustls `Apache-2.0 OR ISC OR MIT`, rustls-webpki `ISC`, ring `Apache-2.0 AND ISC` (6 notices), webpki-roots 0.26.11 and 1.0.9 `CDLA-Permissive-2.0`, zstd `MIT`, zstd-sys `BSD-3-Clause`, tokio-tungstenite `MIT`, tokio/hyper `MIT`, regex/unicode-normalization `MIT OR Apache-2.0`; the one MPL-2.0 crate (`option-ext`) has its source provided.
* Engine-related review flags: **yaml-rust2 0.13.0** ships no licence file in its crate (flagged; adding an immutable upstream notice to `packaging/upstream-notices.json` needs a network fetch and was not done); `pipkin-engine` and `pipkin-engine-protocol` use the repository MIT notice (flagged like the other workspace crates). The remaining flags are pre-existing GPUI/graphics crates.

## Unverified / not done

* **Pi package not rebuilt** here (needs Node/npm, a pinned Pi checkout and an HTTPS Node download). `scripts/test-install.sh` and `scripts/verify-install.sh` (Pi archive) were therefore **not run**; the Pi branch of `install.sh` was exercised only with fixture archives (`test-install-paths.sh`), and the desktop `Exec=` rewrite was checked to produce the previous output for `Exec=pipkin`.
* Not run in CI: `.github/workflows/qualification.yml` was not changed (outside this task's files); the new tests are not yet in the workflow.
* No GUI launch of the installed native candidate (no display in the test shell); no real provider request, no sign-in.
* `write-native-build-info.sh` is written for POSIX sh but only run with `/bin/sh` = bash here (no dash/busybox installed).
* No Arch (`makepkg`) package for the native candidate; no macOS native packaging; no signing.
* Not a clean-machine or other-distribution test; GLIBC_2.44 limits portability as for the Pi package.
* License inventory is evidence, not legal clearance; `yaml-rust2` notice gap is open.
