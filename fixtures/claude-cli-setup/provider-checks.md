# First-class provider integration — offline checks

## Scope

Connections configures independent providers, not an exclusive application mode. The ordinary
model picker contains typed native IDs plus CLI Haiku/Sonnet/Opus. Current conversation identity
controls display/routing; cross-provider or CLI-alias selection creates a separate conversation.
Source text/history is not forwarded. Failed/cancelled selection protects only that source.
Ordinary New follows its current model and preserves already-submitted work. Provider-only
configuration neither chooses a model nor creates a session.

## Measured

- Final full204 passed workspace tests (UI197), strict all-target Clippy, formatting/diff checks,
  guard37 and application build, including final preference-snapshot and native-default regressions.
  Earlier full198 and202 also passed;199 passed tests/Clippy but formatting caught a late-added test.
  Final review added a blank-workspace/restart fence: explicit provider intent without an available
  saved target cannot auto-select an unrelated catalog row; explicit navigation remains available.
  Candidate203 was built/probed before that last fence but never installed; it is superseded.
- Fake core/adapter/engine workflow: CLI fresh and resumed prompts with a native provider that
  panics if invoked; native model remains listed while CLI is open; switching to native confirms
  its captured model on the exact new session; returning to CLI remains usable. No native inference.
- Core: typed ID collisions, scoped cancellation/failure/restart, independent native chat/New,
  model-event generation/op guards, exact native creation receipts, no catalog-row guessing,
  explicit Keep vs late completion, journal cancellation even after model acknowledgment/Keep,
  and ordinary New preserving an earlier same-model submission.
- Preferences: bounded typed parsing, namespace isolation, legacy global-choice migration limited
  to its source, non-authoritative tool-mode/path/model hints. Selection and completed creation
  coalesce preference writes to a final snapshot, not an intermediate cleared source guard.
- GPUI: normal picker on CLI/native; stable typed rows despite catalog changes; no-current CLI
  selection opens Connections; native effort/attachment restrictions remain; explicit trust,
  stale picker and neutral-focus checks remain in the suite.

## Failures corrected, not hidden

- 191: CLI-scoped Models returned no native catalog. Fetch global native models without applying
  their default to CLI.
- 192/193: newly created native session was absent from the adapter's execution-guard index.
  Refresh authoritative session metadata before SetModel; do not bypass the execution guard.
- 195: ordinary same-model New temporarily blocked an already-submitted source prompt.
  Do not create a provider-change guard for ordinary same-model New. Add a regression test.
- 197: old empty-workspace UI fixture assumed catalog arrival alone selects a created session.
  Fixture now supplies the exact captured native creation receipt.
- 200/201: fake coding child disconnected immediately after sending its permission frame, while
  the test assumed a review must arrive first. Production may correctly settle Unknown without
  delivering that review. Added a post-review fixture barrier and a separate immediate-disconnect
  case accepting either review-delivery ordering; both still require Unknown, no effect and no
  native execution. Three focused repetitions of each and full202 passed. No production execution
  policy was changed. Failed gate root `/home/jason/.pg.tGQjPF` retained.
- Intermediate compile/Clippy checks caught new fixture fields and oversized storage Prefs messages;
  fixtures were updated and the writer message boxed.

## Limits

No additional live CLI prompts or current-version CLI probes. Exact2.1.299 policy unchanged;
installed latest still resolves into2.1.301. Original ten-run ledger hash remains
`bf794518e35d7f1ce3a50b3058c4191bb9401e568fc645d77e10b0f404cf1f22`.
Native interaction, file-picker/focus/IME/accessibility verification of this redesign is unverified.
Billing, managed policy and complete descendant containment remain unverified. Historical run9's
review failure is unchanged. Source changes remain uncommitted.

## Installed observation

Package205/install206 activated `1.1.0-native-685ecb881596`; SHA-256
`685ecb8815967ad0e0426cc1aa14b01887b874dedd4c62e1ebea52a3216cb5ff`.
All four prior versions remain; the previous executable's hash is unchanged. The running user
session was not stopped. Candidate/installed offline probes and the installed synthetic-provider
migration/coding/restart/archive/replay workflow passed. See `provider-installation-observation.json`
and retained package/probe logs under `/home/jason/.pg.KnR5bF`. Fully quit/reopen to load the new UI.
