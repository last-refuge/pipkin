# History qualification and remaining-capability reachability

2026-10-11, following `ea6150f`. No protocol/schema change. The pinned reference is
`d2a311097cbcf669e699479587332ae3988a49d0`, plus Pipkin's onboarding/subagent overlays.
The native engine remains opt-in; these checks do not qualify a release.

## Newly measured workflows

`runtime::tests::session_rename_notifies_other_clients_survives_restart_and_can_be_cleared`
uses the public native protocol to rename a populated conversation, observes another
client's catalog invalidation, verifies trimming/Unicode, restarts on the same database,
clears the title, and checks unchanged history. A missing session refuses rather than
creating a phantom conversation. P05-RENAME moves PA → NC.

`native::e2e::history_across_a_real_compaction_pages_once_and_survives_updates_and_restart`
runs the real AppState, NativeBackend, desktop SQLite, engine and compaction with only
provider answers scripted. Four real turns are summarized through the engine protocol.
After reopening, the live window starts at the summary; LoadOlder restores every original
message exactly once. A later coding turn preserves those messages and sends the summary
to the model. App/engine restart and repaging retain all five user messages. This closes
the specific missing compaction-paging workflow in CTX-BOUNDARY-UI. It is headless desktop
integration evidence, not rendered/native-window acceptance.

`runtime::settings_tests::custom_shell_setting_reaches_real_tools_and_changes_between_turns`
uses a private executable wrapper that records one side effect and delegates to `/bin/sh`.
A real bash tool runs through it; removing `shellPath` makes the next turn use the default
shell. Both tool results persist correctly. Existing tool-unit coverage remains.

## Audited dispositions

Searches included ignored source paths (`rg --no-ignore`) and both shipped overlays;
an empty search result alone was not treated as proof of absence. Entry points and
registrations were inspected too.

- **GEN-11 → NP:** `ai/src/providers/all.ts:136` constructs the built-in provider list
  without faux. `ai/src/providers/faux.ts:696` supplies the only production-source
  `fetchDeferred` implementation. The durable poll phase exists for that testing/custom
  provider, but no shipped built-in provider produces it. Native scripted tests remain;
  this does not claim implementation of generic deferred-provider compatibility.
- **SET-CACHEWARMING → NP:** `CacheWarmer` is constructed in classic
  `coding-agent/src/core/sdk.ts:310` and managed by classic `AgentSession`. The shipped
  desktop launches `pi server` (`adapters/pi/engine.rs:174`), whose session-worker path
  builds the durable registry in `experimental/durable/harness-setup.ts:52`. That registry
  installs coding tools and the prompt, not the classic SDK/AgentSession/cache warmer.
  Neither Pipkin overlay adds it. No background warming calls are being removed.
- **SET-SHELLPATH → NE:** that same harness creates `NodeExecutionEnv({cwd})` at
  `harness-setup.ts:71`, without the optional `shellPath` supported by
  `durable/src/env/node.ts:612`. Native already honors the setting through
  `runtime/run.rs`; the new runtime test measures this additional behavior. This does
  not qualify every shell or platform.
- **AUTH-PROVIDER-AUTH-EXT → NC:** the shipped onboarding overlay's
  `packaging/pi-onboarding/provider-auth.ts:4` exposes Anthropic/Codex sign-in;
  `provider-auth-provider.ts:24,171-173` calls `login(..., "oauth", ...)`. The classic
  interactive API-key login is not a desktop workflow. Native API-key configuration
  already has `SetApiKey`/removal/metadata-notification tests. No missing desktop API-key
  form is claimed, and no form is added just to reproduce a classic CLI path.
- **CTX-EDIT-REPLACE → OS:** custom replacement-edit producer compatibility is excluded
  with Pi extensions. The durable context reader supports replacement values, but built-in
  production edits only omit entries (`harness/prompt.ts:74`); replacement producers are
  custom examples/tests, not the shipped coding registry or Pipkin overlays. Ordinary
  history, omission, Stop, compaction and paging remain required. The importer still
  explicitly refuses a source session containing unsupported replacement edits and
  preserves its source bytes; the existing refusal regression stays. A historical
  extension selection alone remains importable. No arbitrary custom-edited context is
  silently imported or discarded.

## Confirmed required work: remote model catalogs

Historical finding, since implemented in `e92c4f1`; see
[catalog refresh evidence](remote-catalog.md). The following records what the audit found
before that implementation.

PCFG-11 and ENV-OFFLINE remain PA. Native `RefreshModels` currently rebuilds local
settings/auth/models configuration. Pinned `core/model-runtime.ts:223-239` wraps every
built-in except Radius in `withRemoteCatalog`, using `models-store.json` and treating
**presence** of `PI_OFFLINE` as disabling network. The desktop Models service refreshes
that runtime (`experimental/services/models-provider.ts:78`).

Required implementation details from `core/remote-catalog-provider.ts` and
`ai/src/models.ts:533-610`:

1. Restore cached overlays before network/auth resolution; apply only catalogs newer
   than the embedded generation timestamp. Merge by model type and id, then preserve
   user configuration/credential-dependent catalog behavior.
2. Resolve credentials before network refresh, use the configured proxy, exclude Radius
   from this generic source, and request supported model types. Catalog requests must
   not disclose provider credentials to the catalog host.
3. Four-hour freshness, ETag only with a cached body, 304 freshness updates, 404/501
   fallback, transient failures preserving body/validator, and Last-Modified handling.
4. Four-second attempts and two immediate retries for transport/transient HTTP errors
   (`utils/management-http.ts`); cancellation and stale-refresh publication guards.
5. Atomic cache persistence, warnings without losing the usable catalog, offline restore,
   and integration tests through RefreshModels. Offline means environment-variable
   presence, including an empty value; `Env::get` currently filters empty values, so
   the implementation needs an explicit presence check.

These are implementation requirements, not completed work. No remote catalog network
request was made during this audit.

## Verification and earlier failures

Final workspace/Clippy/fmt results are recorded in the progress record; logs are
`/tmp/pipkin-history-audit-final-{workspace,clippy,fmt}.log`.
The first app-test command incorrectly selected a nonexistent library target; the
binary target was then used. The first new paging test retained an extra Arc to the
engine across restart, correctly triggering the profile ownership lock; cleanup then
panicked on the poisoned fixture lock. Releasing that test-only handle/connection before
restart fixed the fixture. The complete paging/restart assertions were retained.
