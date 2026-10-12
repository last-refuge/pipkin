# Prompt resource, WebSocket connection, and native launcher checks

2026-10-11, following `af059d5`. These checks replace the final three explicitly unverified
row labels with bounded evidence. They do not close partially measured rows or release gates.
Protocol 11 / schema 9 and the production implementation are unchanged in this batch.

## WebSocket connection timeout

`openai_codex/tests.rs` now runs an actual local TCP endpoint that accepts a WebSocket
request but never sends handshake headers. Ordinary HTTP requests pass through to a local
SSE fixture. `websocketConnectTimeoutMs = 200` ends the stalled handshake with the exact
connect-timeout diagnostic, closes its socket, and completes the request through SSE. The
next request uses the saved session fallback without another WebSocket attempt. The request
idle timeout is five seconds, so it cannot explain the 200 ms connect failure.

With the connect timeout disabled (`0`), a 100 ms stream-idle timeout does not terminate the
pending connection: it remains pending after 350 ms. Cancellation then returns Aborted,
closes the socket, emits no content deltas, and records no transport failure or SSE request.
The first test run incorrectly expected a debug-stat row for a handshake that never
completed; the assertion now permits absent stats while still asserting zero failures.
The failure is retained in `/tmp/pipkin-websocket-timeout-tests.log`.

Runtime `websocket_connect_timeout_setting_reaches_requests_including_zero_and_live_edits`
changes settings between three turns (200, 0, 1250), verifying the exact values reach provider
requests. All **14 tests** matching `websocket_` passed in
`/tmp/pipkin-websocket-timeout-tests2.log`. Existing proxy/TLS tests remain separate evidence;
these tests stall the plain local WebSocket handshake, not DNS or TLS on a real service.
No full Pi differential or live provider qualification is claimed.

## Editor and terminal flags

`adapters/native/e2e.rs::native_editor_and_terminal_flags_launch_from_a_real_tool_change`
runs the real core, native adapter, SQLite journal and engine with a scripted model. The
built-in write tool creates a file whose name contains spaces and a semicolon. Its Changes
entry drives the actual OpenTerminal and OpenInEditor commands and Launch effects.

Options are parsed with `--engine native --editor ... --terminal ...`, then passed through
the production LaunchConfig and launcher. Scratch recorder executables verify the process
working directory and exact argv. Both a graphical editor command with an option and a
terminal editor named `nvim` with an option are covered; the latter is actually invoked by
the configured recorder terminal. The file path remains one literal argument. No user
editor, terminal window, desktop configuration, or synthetic UI input is involved.

Targeted test passed: `/tmp/pipkin-native-launch-tests.log`. This is headless desktop
integration and process evidence, not a guarded native GUI acceptance run.

## Throwing prompt-section applicability

Pinned Pi `d2a311097cbcf669e699479587332ae3988a49d0` has a general callback exception
handler in `packages/durable/src/harness/prompt.ts::renderSections`: keep previously shown
text on a render exception; propagate an aborted context. Its built-in coding prompt is
registered by `coding-agent/src/experimental/durable/prompt.ts::createPiPrompt`, which
loads project files/skills once per cwd and renders fixed keys from that snapshot.

Audited supporting paths: `core/system-prompt.ts::buildSystemPromptSections`,
`core/resource-loader.ts::loadContextFileFromDir` / `loadProjectContextFiles`,
`core/footer-data-provider.ts::findGitPaths`, `core/skills.ts` loaders and
`utils/paths.ts::canonicalizePath`. Ordinary resource read/stat/parse failures are caught
by the loaders. The custom-section-name exception in `buildSystemPromptSections` is not
used by `createPiPrompt`; the built-in registration supplies no custom-section map.

Native has fixed, pure `SystemPrompt::sections` rendering from typed, frozen resource
snapshots and embedded product documentation. There is no user-provided section callback
or fallible section provider. Arbitrary callback loading/recovery would add the Pi extension
compatibility the owner excluded. PRM-THROWING-SECTION is therefore recorded as **NE**:
the supported built-in prompt retains its loaded content across resource loss without
adding an exception-capable renderer. This is not an assertion that Rust panics are caught.

The new runtime regression loads project instructions and skill metadata, then replaces
both source files with directories. A tool-selection change forces section rerendering:
the live root keeps the original instructions/skill text, and the next coding turn completes.
A fresh root skips the unreadable candidates, loads fallback CLAUDE.md instructions, and
retains ordinary built-in sections. Existing cache tests separately cover root/child sharing,
tool changes, edited resources, restart/retirement and executed-reference cache lifetimes.
Targeted test passed: `/tmp/pipkin-prompt-resource-loss.log`.

The row's native-equivalence decision rests on this supported registration audit plus runtime
resource-failure evidence. It does not claim a native port of arbitrary throwing callbacks
or full prompt-generation qualification against an executed Pi trace.

## Workspace verification

**1534 passed, 0 failed, 53 ignored**; Clippy, formatting and diff checks clean.
Logs: `/tmp/pipkin-final-row-checks-workspace.log`, `/tmp/pipkin-final-row-checks-clippy.log`,
`/tmp/pipkin-final-row-checks-fmt.log`. The ledger has 444 measured, 7 observed, zero explicitly
unverified and 53 not-started labels. None is fully release-qualified; partial row coverage,
reference comparisons, sustained use, installed workflows and cutover remain open.
