# Pipkin installed help

This package is a Pipkin release build using the built-in Rust engine. It carries only the `pipkin`
binary (which also runs the engine as a private `pipkin --engine-worker` process), the desktop entry, icons,
notices and inventories. It contains **no Node runtime, npm tree or external engine**, and it does not install system
Node, Git or graphics/desktop libraries. Releases are published with SHA-256 checksums only (no signing key yet)
and are supported on x86_64 Linux with Hyprland/Wayland; other platforms are experimental or unverified.

- Both `pipkin` and the desktop entry use the built-in engine. `--engine native` is an optional alias.
- Native engine data lives in its own directories (`~/.local/share/pipkin/engine` and `~/.config/pipkin/engine`,
  or under an explicit `--data-dir`), separate from legacy profiles in `~/.pi`. History written by the native engine is
  native-only: legacy engines do not read it.
- Claude subscription sign-in is available under Settings → Connections, including browser and copy-code login.
- `./install.sh --rollback` can reactivate a retained previous installation, including a historical legacy-engine build;
  native data is left in place and is not migrated back. The installer checks the retained binary and runs
  an offline engine probe before activating it; a damaged or unresponsive target leaves the current version selected.

`pipkin --diagnose --engine native` reports the engine identity and directories; add `--probe` to start a
throwaway worker in a scratch profile (no credentials, no network) and check that it answers. Detailed
diagnostics can include sensitive log text; review before sharing.

`build-info.json` identifies this build; `runtime-inventory.json` records direct native library/ABI references;
`third-party/inventory.json` records conservative Rust dependency notices and open review flags. Neither
inventory is legal clearance or a complete binary SBOM.

- [Data ownership, diagnostics privacy and recovery](docs/support.md)
- [Install/update/rollback and the native candidate](docs/packaging.md)
- [Bundled notices and inventory scope](docs/bundled-licenses.md)
