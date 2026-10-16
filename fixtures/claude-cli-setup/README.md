# Guided Claude setup — offline evidence

This is a UI/discovery/orchestration change, not new live CLI qualification.

## First-class provider integration (current)

Typed native/CLI models share the ordinary picker. Connections configures providers independently;
there is no global either/or switch. Pending changes protect only their source conversation.
The fake adapter workflow now checks CLI fresh/resume, native catalog visibility while CLI is open,
CLI→native exact-session creation, and returning to the still-usable CLI chat. No native inference
or additional real CLI execution is authorized.191 caught hidden native catalog entries; fixed by
reading global native models without applying that default to the CLI session. Final full204 passed
workspace tests, strict all-target Clippy, fmt/diff, guard37 and app build (UI197). Further corrected
races/fixtures are documented in `provider-checks.md`. Package205/install206 activated
`1.1.0-native-685ecb881596`; candidate/installed probes and the installed synthetic workflow passed.
All prior versions were retained and the running session left untouched. Evidence uses `provider-*`
names. Fully quit/reopen to use the new provider picker.

## Connections-only global switch (superseded; rejected abstraction)

The initial sidebar/chooser UX was rejected: failed/unfinished setup left the old native chat
sendable, and New did not retain a CLI routing choice. Normal-profile metadata was inspected
read-only (backend/model only): no CLI sessions, recent native GPT sessions. No transcript or
credentials were read.

Full185 passed workspace tests, strict all-target Clippy, fmt/diff, guard37 and app build (UI197).
The corrected flow lives in Settings → Connections only. New follows a saved, namespace-isolated
connection choice; existing conversations never change backend. A mismatch blocks submitting,
including after failed/cancelled setup, restart and journal delay. Unknown stored choices fail
closed. The composer identifies its actual backend.

The real adapter/engine fake-CLI test performs a fresh and resumed CLI turn with a native model
configured. The native provider fails the test if invoked; no native request occurred and the
native model preference was preserved.184 caught opening the wrong Settings page; fixed before
full185.182 caught an immutable-model mismatch in the old manual test fixture; the reconnect
case now exercises saved-hint guided setup rather than substituting another model.

Versioned package/install observations for this correction use `connections-*` filenames.
Real native interaction and live qualification remain unverified. The current `latest` link
still points into2.1.301; the exact2.1.299 policy and original exhausted live ledger are unchanged.

## Initial guided redesign evidence (entry points superseded)


- Full177 passed workspace tests, strict all-target Clippy, fmt/diff, guard37 and app build; UI196.
- Real adapter/engine with a disposable fake CLI: automatic installation/project resolution, no executable invocation before confirmation, exactly three metadata probes after confirmation, captured metadata-only creation, zero native or CLI model requests.
- Pure core: no typed configuration in the normal case; ambiguity requires a choice; confirmation is ticket-scoped; Create waits for its durable journal; duplicate/stale responses cannot continue; cancellation/supersession/unknown checks retire continuation; reconnect cannot change the source session or substitute an unsupported model.
- GPUI headless: ordinary conversation chooser, confirmation fits the default test viewport and sends one Check, neutral focus never approves, stale folder-picker result cannot overwrite newer setup.
- Existing176 fake preflight test failed with executable-busy; three focused repeats and full177 passed. This is not a root-cause fix claim.
- User's CLI `latest` symlink points into2.1.301; exact2.1.299 support remains unchanged. No current-version executable/auth/live check was performed.
- No new live prompts; original `../claude-cli-probe/2.1.299/runtime-budget.txt` SHA256 remains `bf794518e35d7f1ce3a50b3058c4191bb9401e568fc645d77e10b0f404cf1f22`.

The package root is an owned private disposable directory, not the user's live profile. Installation observations are added only after the versioned install and offline smoke complete. No real native picker, IME, accessibility, visual-presentation or new live-CLI pass is claimed.
