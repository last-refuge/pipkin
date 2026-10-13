# Changelog

Release notes for Pipkin. The newest release is first. The GitHub release body for a version is
this file's section for it; the publish step in `.github/workflows/release.yml` extracts it.

## 1.0.0 (2026-10-13)

The first release of Pipkin: a native Rust/GPUI desktop coding agent with a built-in Rust
engine. One binary carries the app and the engine. Nothing in the package needs Node, npm or
a Pi checkout. Engine protocol 31, store schema 20.

### Supported platform

- **x86_64 Linux on Arch/Omarchy with Hyprland on Wayland**, with the built-in engine. This
  is the one supported configuration; see [docs/platforms.md](docs/platforms.md).
- **macOS: experimental and unsupported.** The Apple Silicon and Intel archives are built and
  checked in CI (strict code-signature verification and an offline engine probe of the
  extracted app), but they are ad-hoc signed, not Developer ID signed and not notarized, so
  Gatekeeper warns on first launch. No interactive acceptance has been run on a Mac. The Intel
  archive is best effort and may be absent from a release.
- **Windows: not supported and not built.**
- **Other Linux desktops and distributions:** expected to work, unverified. Issues there are
  welcome and are not treated as release blockers.

### What is in 1.0.0

- **Built-in engine.** The Rust engine is the default and the Pi/Node runtime is retired. The
  engine runs as a private worker owned by the app, with stale-guarded events, replay
  recovery per tool and a scratch-profile probe (`pipkin --diagnose --probe`).
- **Conversations.** A GPU-drawn streaming transcript with cross-message selection, readable
  tool calls, a working-tree changes view, drafts with visible save status, history and
  search, and an offline cache of past conversations.
- **Questions and plans.** The engine can ask structured questions (`ask_user_question`, with
  the questionnaire contract) and keep an engine-owned plan that the desktop shows and lets
  you edit.
- **Web access.** `web_search`, `web_fetch` and `web_read` with retained sources, citations in
  answers, a search fallback chain and batched fetches.
- **Subagents.** Profiles with frozen launch policies, per-child controls, background
  children, bounded answers and a listing of each child's artifacts.
- **Goals.** Engine-scheduled goals with structured progress, checked evidence, allowances,
  cost limits, an independent audit and a pinned auditor model.
- **MCP.** Supervised stdio servers with compact tool access, the Streamable HTTP transport,
  OAuth sign-in, resources, prompt templates, elicitation, credentials kept in the keychain
  store and binary content retained as artifacts.
- **Jobs and status.** Supervised background jobs with a Jobs inspector, a status line in the
  composer and engine-reported context usage.
- **Observability and export.** Engine diagnostics records with a session-dialog section, an
  OTLP trace exporter, and configurable export with a content level, a credential store and a
  preview.
- **Onboarding.** Claude subscription sign-in under Settings → Connections (browser and
  copy-code login) and provider setup in the app, never in the terminal.
- **Packaging.** A Linux archive with a versioned per-user installer (install, upgrade,
  rollback, uninstall), an Arch `PKGBUILD`, and macOS app, ZIP and drag-to-Applications DMG
  builds, each carrying build identity, license and runtime inventories.

### Downloads and verification

| File | What it is |
| --- | --- |
| `pipkin-native-1.0.0-x86_64.tar.zst` | Linux x86_64 archive with `install.sh` at its root |
| `pipkin-1.0.0-aarch64-apple-darwin-experimental.dmg` / `.zip` | Experimental macOS app, Apple Silicon |
| `pipkin-1.0.0-x86_64-apple-darwin-experimental.dmg` / `.zip` | Experimental macOS app, Intel (best effort) |
| `build-info-*.json` | Source revision, binary hash and build inputs for each archive |
| `SHA256SUMS` | Checksums of every file above |

```sh
sha256sum --check --ignore-missing SHA256SUMS
tar --zstd -xf pipkin-native-1.0.0-x86_64.tar.zst && ./install.sh    # Linux: into ~/.local, no root
```

The release is **not signed**: there is no release signing key yet, so `SHA256SUMS` proves
only that a download is intact, not who built it. `scripts/verify-release.sh DIR` runs the
same check. Upgrades are "download, verify, `./install.sh`"; `./install.sh --rollback`
reactivates the previous version, subject to the schema and backup rules in
[docs/packaging.md](docs/packaging.md). There is no in-app update check.

### Known limits

- A screen reader does not yet speak typed characters in the composer; Orca work is deferred
  past 1.0.
- On macOS the engine cannot find and stop a background job's process after it restarts;
  the job is marked interrupted with "the process is gone". Linux stops it.
- On the macOS CI runner a handful of engine unit tests fail or flake across runs (the
  structured-goal audit and evidence tests, a subagent artifact re-read, and a timing-based
  bash progress test). They are macOS issues to investigate and do not affect the Linux
  build, whose full suite passes in CI.
- The owner-run native gate rows in [docs/native-gate-testing.md](docs/native-gate-testing.md)
  keep their recorded status; soak and performance qualification happen after 1.0.
- Detailed diagnostics (`pipkin --diagnose`) can include sensitive log text; review before
  sharing. See [docs/support.md](docs/support.md).
