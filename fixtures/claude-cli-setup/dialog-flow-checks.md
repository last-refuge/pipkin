# Setup dialog dismissal / continuation correction

User report: model picker → Connections → Configure provider → Find Claude again returned to the
conversation without usable setup.

- Reproduced in job207 using a GPUI pointer click: the setup command ran, but overlay became None.
  The guided panel did not occlude the dismissible backdrop. Prior tests checked command dispatch
  without asserting that the dialog stayed open.
- Both guided and advanced setup panels now occlude the backdrop. Inside clicks stay in the
  dialog; outside clicks still dismiss. No global button behavior or native input guard changed.
- Connections now offers Continue conversation setup when model-picker setup already exists.
  Opening it preserves the same scope, alias, purpose and pending operation instead of replacing
  it with provider-only configuration. Core also refuses that accidental purpose replacement.
- Rediscovery retires old tickets/trust and stale path-picker revisions, preserving setup purpose.
  It issues only discovery and optional metadata cleanup; no login/version probe, Create or prompt.
- Job209: 40 core setup tests and 19 CLI UI tests passed. Pointer tests cover Find again for both
  provider-only and conversation setup, inside/outside dismissal and confirmation. Model-picker
  entry verifies continuation does not issue duplicate discovery or switch to provider-only.
- Full210 passed workspace tests (UI198), strict all-target Clippy, fmt/diff, guard37 and app
  build. Package211/install212 activated `1.1.0-native-0dded419540c`; candidate/installed offline
  probes and the installed synthetic workflow passed. All previous versions retained, previous
  binary hash unchanged, running session untouched. See `dialog-installation-observation.json`;
  retained package/probes: `/home/jason/.pg.FQZQtI`. Fully quit/reopen to load the correction.
  No live prompts or installed-Claude execution. Exact
  2.1.299 support remains; newer versions must show a failure rather than fall back.
- Native pointer/focus/IME/accessibility verification remains unclaimed. Prior installations and
  running user session were retained. Changes remain uncommitted.
