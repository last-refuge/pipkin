# Platforms: what is supported, what was checked, what blocks the rest

## 1.0 statement (adopted by the owner on 2026-10-13 with the 1.0.0 release)

Pipkin 1.0 supports **x86_64 Linux on Omarchy/Arch with Hyprland on Wayland**, with the
built-in engine. Everything else is explicitly outside 1.0 support:

- **macOS**: an experimental build only. It is not signed with a Developer ID and not
  notarized, no interactive acceptance has been run on a Mac, and no issue on it blocks a
  1.0 release. The release notes say "experimental, unsupported".
- **Windows**: not supported and not built. The release notes say so.
- **Other Linux desktops and distributions**: expected to work but unverified; issues there
  are welcome and are not release blockers.

The owner adopted this statement on 2026-10-13 (finish plan §8); it is repeated in the 1.0.0 release
notes (`CHANGELOG.md`). The table below records the evidence behind it.

**Supported scope: x86_64 Arch/Omarchy, Hyprland/Wayland.** The owner-run native gate rows in
[native gates](native-gate-testing.md) keep their recorded status. See the current [release-gate record](v1-release-gates.md); successful headless
Ubuntu CI does not qualify an Ubuntu desktop or an additional supported distribution.

Native observations below include historical author-machine checks (Arch/Omarchy, Hyprland 0.56, Intel + NVIDIA,
125% scale). They are not a complete final-candidate matrix. Later scale and accessibility evidence is separately
recorded in [native gates](native-gate-testing.md); do not read that historical setup as the current display setting.

| Target | Status | Evidence |
| --- | --- | --- |
| x86_64 Omarchy/Arch, Wayland (Hyprland) | **Supported (1.0.0)** | Owner-reported daily use; current locked development/packaged engine checks and scratch installer checks passed. Historical 1500-prompt measurement is not final-candidate soak. Clean installed desktop/native gates remain open |
| Arch/other Linux, X11 (also Xwayland) | **Builds and starts; checked under Xwayland only** | The `x11` GPUI feature is enabled alongside `wayland` (one binary; the display server picks at run time). Launched with `WAYLAND_DISPLAY` unset on Xwayland: it renders correctly. A pure X11 session, other window managers, and IME/screen reader on X11 were not tried |
| Other Wayland compositors (GNOME, KDE) | **Unverified** | Same Wayland code path as Hyprland, but portals (file picker), decorations, clipboard and IME were not tried on them |
| Other Linux distributions | **Experimental; native install/runtime unqualified** | Generic installer mechanics passed in scratch prefixes, not a distribution desktop matrix. Native ABI and external libraries vary by artifact; the local app references GLIBC_2.44. Ubuntu 24.04 headless CI builds its own artifact and is not proof that the local binary runs there |
| macOS | **Experimental build published with 1.0.0; not supported** | The 1.0.0 release workflow built and verified the Apple Silicon and Intel app/ZIP/DMG; a handful of engine unit tests fail or flake on the macOS CI runner (see the 1.0.0 changelog known limits). Observed green Apple Silicon native build and two scripted engine smoke checks in run 37482847877 on f52e8c2, with an experimental app archive. See [first-pass limits](macos-first-pass.md); actual interactive native acceptance and Intel/universal builds remain unverified |
| Windows | **Not supported** | See the blockers below |

## Cross-platform implementation and remaining boundaries

- **GPUI already selects native backends** through `gpui_platform::application()` and target-specific
  dependencies. Enabling Wayland/X11 features does not itself disable the macOS/Windows backends; the
  previous claim here that these native backends were not enabled was incorrect.
- **Engine transport** (`crates/pi-client/src/unix.rs`): Linux uses `SO_PEERCRED`; macOS now uses kernel
  `getpeereid`, retaining owner-only directory/socket and server-id checks. Windows still needs an
  authenticated transport on both the Rust client and pinned Pi server sides.
- **Engine lifecycle** (`engine.rs`, `macos_processes.rs`): Linux retains `/proc` discovery; macOS uses
  kernel uid/argument-environment inspection with exact environment identity. Native runner ownership
  and engine startup/shutdown tests passed; interactive application cleanup remains unobserved. Windows ownership remains unimplemented.
- **Small `/proc` reads**: server UUIDs now use OS entropy (`/dev/urandom`) on both Unix targets, but
  RSS/performance instrumentation remains Linux-oriented. Zero/missing readings on macOS are not memory-soak evidence.
- **Paths and launching** (`launch.rs`, `platform.rs`): XDG directories and editor/terminal discovery still
  assume Linux conventions. Explicit launch arguments and terminal startup are evaluation workarounds.
- **Packaging, signing and updates**: the first macOS archive contains an experimental native `.app`,
  notices and identity, but no engine, Developer ID signing or notarization. Windows has no artifact yet.
- **Accessibility and IME** need actual VoiceOver and Narrator/NVDA checks; none are claimed.

## Engine compatibility and runtime requirements

Packages pair the app with the exact Pi revision in `packaging/pi-engine-revision`; current protocol is **8** and
app database schema is **7**. An external server must satisfy the app's compatibility checks; arbitrary/latest
Pi revisions are not promised compatible. Requalify any changed pair, and do not bypass a protocol/schema refusal.

For the current Linux package, the Pi engine uses a paired private Node 22.23.3 runtime. A system Node is not required to run Pipkin; building from source still requires Node/npm. Runtime requirements include Git, a working Vulkan-capable graphics setup and desktop libraries listed in [PKGBUILD](../packaging/PKGBUILD). The bundled runtime does not provide a GPU driver.
Inspect the artifact's [native/runtime inventory](bundled-licenses.md#native-runtime-inspection) for direct ELF
library/ABI references. It is not a complete static/dlopen dependency audit or portability guarantee. Optional
native VM/sandbox components and foreign source-tree artifacts remain provenance/runtime review findings; their
presence does not extend platform support or promise a sandbox.

## Credentials and tool discovery

Pipkin does not provide its own credential store. Its logs/caches can still contain sensitive provider/tool/error
text and must not be assumed credential-free. Pi's own mechanism (login, API keys, `models.json` under `~/.pi/agent`) is used
by the engine; Pipkin shows status and offers a refresh. The installed engine uses its private Node and `git` from `PATH`; `pipkin --diagnose` reports the Node runtime and engine.

## Isolation

Tools run with the engine's permissions (as in Pi itself). Pipkin does not sandbox them and does not claim to.
