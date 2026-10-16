# Claude Code setup redesign

## Minimum-version policy correction (current)

At the user's request, production compatibility now accepts stable CLI2.1.299 or newer, compared
numerically. Required flags, explicit path trust, normal login and immutable binding checks remain.
Bindings record the actual observed version, not the historical qualification baseline. This
supersedes the exact-version blockers recorded in the historical installation sections below.
No additional live prompts are authorized or claimed; newer-version checks use a fake CLI.
Full214 passed; package215/install216 activated `1.1.0-native-ffe95dc723a1`, retaining prior versions
and the running session. Fully quit/reopen. Evidence: `fixtures/claude-cli-setup/minimum-version-checks.md`.
The preceding dialog correction also preserves model-picker setup via Continue conversation
setup in Connections and keeps inside clicks from dismissing the dialog.

## First-class provider integration (current)

The global either/or connection switch below was rejected. Connections configures independent
providers; the normal model picker combines typed native IDs and CLI Haiku/Sonnet/Opus aliases.
Configuration alone neither selects a model nor creates a conversation. Cross-provider/CLI-alias
changes create separate conversations, preserving source history/draft and immutable execution.
A failed/cancelled change blocks only its source, with an explicit Keep action; unrelated native
and CLI conversations stay usable. New follows the current conversation's model/pending intent.
Authoritative model events and correlated native creation receipts prevent preference/catalog
ordering from silently choosing another backend. Picker rows remain stable during catalog changes.

Plan tasks30–32 now track integration, offline validation, documentation and retained-version install.
No further live prompts are authorized. Final full204 passed workspace tests (UI197), strict all-target
Clippy, fmt/diff, guard37 and app build. Package205/install206 activated `1.1.0-native-685ecb881596`,
retaining every prior version and leaving the running session untouched. Candidate/installed
probes and the installed synthetic-provider workflow passed; fully quit/reopen to use it.
Evidence: `fixtures/claude-cli-setup/provider-installation-observation.json`. Job191 caught CLI-scoped
Models hiding native providers; the adapter now reads the global native catalog without applying
its default to CLI. Later gates caught a missing native guard-index refresh, ordinary New's source
journal race, an obsolete UI creation fixture and an immediate-disconnect test ordering assumption.
All are documented in `fixtures/claude-cli-setup/provider-checks.md`, with the final passing gates.

## Connections-only global switch (superseded; rejected abstraction)

The initial chooser/sidebar design below was rejected in daily use. Read-only metadata in the
normal profile showed no CLI conversations and recent native GPT sessions; no transcript or
credentials were read. Source review found that opening/failing/closing setup left the previous
native composer sendable, and ordinary New did not retain a CLI routing preference.

Correction: CLI configuration/reapproval/recovery lives only in Settings → Connections. Remove
special sidebar/palette actions and the Ctrl+N backend chooser. Persist explicit connection intent
(namespace-isolated, not authority); New follows that choice. CLI New performs filesystem-only
preparation and opens Connections for scoped confirmation. Existing conversations stay immutable.
A mismatched conversation cannot submit, including after failed/cancelled setup, restart and a
journal delay. Choosing another connection retires unfinished setup continuation. The composer
shows its authoritative backend, not the requested connection or the assistant's self-description.

Validation: full185 passed workspace tests, strict all-target Clippy, fmt/diff, guard37 and app
build (UI197). This includes core routing/failure/journal tests, real core+adapter+fake CLI
fresh/resume with a configured native model and a provider that fails the test if invoked,
GPUI entry/label checks, and persistence tests. Package186/install187 updated the normal launcher
to `1.1.0-native-d43630fc7e1d`; prior binary retained unchanged and running session left untouched.
Offline probes and the installed synthetic-provider workflow passed. Fully quit/reopen to use
this correction. Evidence: `fixtures/claude-cli-setup/connections-installation-observation.json`.
Real native interaction/live CLI verification remains unverified. No real CLI execution or
inference was performed; current `latest` still points into2.1.301 and exact2.1.299 support is
unchanged.

## Initial redesign (superseded entry points)

User-approved goal: zero typed configuration values when Claude is installed/logged in and a project is selected. This changes presentation and metadata orchestration, not execution authority or tool scope.

## Work plan

1. Engine-owned, bounded filesystem discovery (saved choice, absolute PATH, known installation locations including mise); no executable/shell/package-manager invocation. Capture the selected project; remember installation/model hints, never grants.
2. Pure-core guided flow: discover → inspect canonical paths → explicit scoped confirmation → Check → one journaled metadata-only Create (or Return). Stop on failures/uncertainty; do not replay or automatically renew expired consent.
3. (Superseded by the correction above.) Normal New conversation chooser distinguishes native providers from Claude Code local CLI. Concise setup, project/executable pickers, advanced diagnostics/recovery, and prefilled reconnect.
4. Offline resolver/core/adapter/GPUI regression tests, workspace tests, strict Clippy and formatting. Native checks require an owned address/PID-guarded window. No additional live Claude inference is authorized.

## Safety and acceptance

- Discovery is not execution or trust; version/login probes require confirmation of captured executable and project.
- Ambiguous installations require selection. Relative/project-local PATH candidates and manager shims are not silently executed. No broad home scan or credential-file reads.
- Changing inputs, cancellation, disconnection and stale replies cannot carry consent into another scope. Existing conversation backend/model/mode remain immutable.
- A single explicit connect intent may coordinate metadata checks and one durable Create; it cannot send a prompt, approve Write, retry unknown outcomes or authorize a native fallback.
- Preferences are non-authoritative and namespace-isolated; restart never restores grants.
- Existing recovery mechanisms remain available without making every user read the protocol.

Status: implementation and offline validation complete (full177: workspace tests, strict all-target Clippy, fmt/diff, guard37, app build; UI196). Package178 built; install179 updated the normal launcher to `1.1.0-native-c0d0d0f1b997`, retaining the previous binary unchanged and leaving the running session/profile untouched. Candidate/installed offline probes and the installed synthetic-provider migration/coding/restart/archive/replay workflow passed. Evidence: `fixtures/claude-cli-setup/installation-observation.json`. Fully quit/reopen to use the new UI. The transient executable-busy failure in176 passed three isolated repetitions and full177; its underlying cause is not established. Native interaction and live CLI qualification of the redesigned flow remain unverified. No additional live inference is authorized or used.

The user's `latest` CLI symlink now points into2.1.301. The existing exact2.1.299 compatibility guard is unchanged; qualifying the newer version is separate work, not an automatic upgrade or fallback. The current-version executable was not invoked during this work.

The plan tool was unavailable during the earlier redesigns; it now accepts the advertised schema (revision50, tasks30–32).
