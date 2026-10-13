# Changelog

Release notes for Pipkin. The newest release is first. The GitHub release body for a version is
this file's section for it; the publish step in `.github/workflows/release.yml` extracts it.

## Unreleased

### Added

- A one-line installer for Linux x86_64: `curl -fsSL
  https://github.com/last-refuge/pipkin/releases/latest/download/install.sh | bash` downloads the
  prebuilt archive of the latest release, verifies it against `SHA256SUMS`, and runs the archive's
  installer into `~/.local`. It never builds from source and takes `--version`, `--prefix`,
  `--dry-run`, `--list`, `--rollback` and `--uninstall`. The script lives at `install.sh` in the
  repository, is published as a release asset by the release workflow, and was attached to the
  1.1.0 release by hand.

## 1.1.0 (2026-10-13)

A maintenance release on the same engine protocol (31) and store schema (20) as 1.0.0. It
carries one engine fix that 1.0.0 shipped with, the test and CI work around it, and the
documentation that caught up with the native engine. Upgrading is "download, verify,
`./install.sh`" as for 1.0.0; no migration runs.

### Fixed

- Goal audit: when the auditor requested changes before the executor's proposing turn had
  settled (its final reply after the proposal's result was still pending), that turn then
  settled with no decision and paused the goal with "the executor reported no goal status
  this turn" instead of running the repair turn; and when the verdict and the settlement
  advanced the goal at the same time, the turn one of them had just started could be
  mistaken for the person's input and pause the goal with "you sent a message". The
  findings are now recorded as that turn's `continue` decision, and a tick that finds the
  goal's own next turn already running leaves it alone, so the repair turn runs either way.
  This was the cause of the structured-goal audit test failing on the macOS CI runner; it
  could also happen on Linux.
- Engine tests that lost a race more often on macOS than on Linux: the structured-evidence
  goal test now gates the turn until it has observed the rejected proposal (and then checks
  that the accepted one completes the goal); the subagent artifact test no longer writes the
  same file twice within one parallel tool round; the bash progress test now proves
  throttling with a pause followed by a burst instead of per-line sleeps.

### Build and CI

- The release workflow now saves the Rust build cache from its Linux and macOS jobs, and the
  experimental macOS build workflow gained a best-effort Intel leg, so the Intel release
  build no longer compiles GPUI and every dependency from scratch (about 36 minutes against
  about 7 for Apple Silicon in the 1.0.0 runs). Nothing the release publishes changed.
- The release workflow's staging directories are ignored by git so release builds stamp
  themselves as clean.

### Documentation

- The Rust engine plan (`llm-docs/pipkin-rust-engine-plan.md`) is rewritten as the completion
  contract the conversion was held to, with the feature preservation matrix and the
  Opus to Codex handoff; the GPUI best-practices notes behind the `gpui-desktop-app` skill
  are added under `llm-docs/`.
- `docs/goals.md`, `docs/platforms.md` and `docs/macos-first-pass.md` reflect the audit fix,
  the macOS test status and the Intel build leg.

### Known limits

- Everything in the 1.0.0 known limits still applies except the macOS engine test item, which
  is now: with the fixes, the macOS build workflow has been observed green on Apple Silicon
  (runs 38430815306 and 38432999668) but not consistently. On the 1.1.0 commit itself (run
  38443588228) both legs failed again, one or two engine tests each (the goal2 structured
  evidence test, the goal3 stop-and-restart audit test, a publication sequence test and a
  remote-catalog timing test), every one of which passes in other runs. Engine tests on the
  macOS CI runners remain intermittent. The release archives are built and verified without
  running them, and the Intel archive stays best effort.
- One Linux native adapter end-to-end test (`workspace_scan_failure_keeps_the_last_diff_and_retry_recovers_without_a_prompt`)
  failed once on `main` in run 38431008244 and passed in the runs before and after it; it is
  being treated as intermittent, not as a product defect.

### Downloads and verification

| File | What it is |
| --- | --- |
| `pipkin-native-1.1.0-x86_64.tar.zst` | Linux x86_64 archive with `install.sh` at its root |
| `pipkin-1.1.0-aarch64-apple-darwin-experimental.dmg` / `.zip` | Experimental macOS app, Apple Silicon |
| `pipkin-1.1.0-x86_64-apple-darwin-experimental.dmg` / `.zip` | Experimental macOS app, Intel (best effort) |
| `build-info-*.json` | Source revision, binary hash and build inputs for each archive |
| `SHA256SUMS` | Checksums of every file above |

```sh
sha256sum --check --ignore-missing SHA256SUMS
tar --zstd -xf pipkin-native-1.1.0-x86_64.tar.zst && ./install.sh    # Linux: into ~/.local, no root
```

The release is **not signed**; `SHA256SUMS` proves only that a download is intact. The macOS
archives are ad-hoc signed and not notarized, so Gatekeeper warns on first launch.

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
- On the macOS CI runner a handful of engine unit tests failed or flaked across runs (the
  structured-goal audit and evidence tests, a subagent artifact re-read, and a timing-based
  bash progress test). Fixed in 1.1.0: three were races the tests
  themselves lost more often on macOS, and one was an engine race in the goal audit that
  could also pause a goal on Linux. The 1.0.0 binaries carry the engine race.
- The owner-run native gate rows in [docs/native-gate-testing.md](docs/native-gate-testing.md)
  keep their recorded status; soak and performance qualification happen after 1.0.
- Detailed diagnostics (`pipkin --diagnose`) can include sensitive log text; review before
  sharing. See [docs/support.md](docs/support.md).
