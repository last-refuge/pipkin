# Native Arch package qualification

The reports identify actual local `makepkg` artifacts. The archives themselves are retained
outside Git. Build with the repository's pinned Rust toolchain and declared build dependencies:

```sh
(cd packaging && PIPKIN_ENGINE=native makepkg)
python3 scripts/verify-native-arch-package.py ARCHIVE.pkg.tar.zst REPORT.json
```

For this host, the pinned Rust toolchain is user-managed rather than registered as pacman's
`rust`/`cargo` packages. The recorded build used `--nodeps` after `pacman -T` confirmed all
other declared dependencies present and `rustc -V`/`cargo -V` confirmed the pinned toolchain.
That is a local qualification accommodation, not a removal of declared build dependencies.
It neither installed system packages nor used `makepkg -s`/`-i`. `BUILDDIR` and `PKGDEST`
pointed into a disposable `/tmp/pipkin-native-arch-*` directory. Cargo ran offline, with
Node/npm/npx/Pi trap executables first on PATH; none were invoked.

Native mode retains Cargo's release line tables (`!strip`) and disables makepkg's full-debug
override (`!debug`). The initial full-debug artifact is identified separately for size
comparison. The final report checks actual package metadata and binary identity, desktop
selection, notice inventories, and the extracted binary's probe/import/coding/restart/closed
archive restoration. It does not establish a system pacman transaction, GUI acceptance,
signatures, hosted CI, full provenance clearance or final Pi retirement.
