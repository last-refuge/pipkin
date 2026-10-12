# Stored provider refresh and catalog publication

Follow-up to `1429251`, 2026-10-11. Tests use invented credentials and local HTTP servers;
no account sign-in or live provider qualification is claimed.

Five new tests in `crates/pipkin-engine/src/auth/refresh_tests.rs` exercise the real auth
service, HTTP exchanges and atomic credential store:

- Copilot: eight concurrent resolutions produce one token exchange and one catalog request.
  Access token, enterprise domain, account model IDs and five-minute expiry skew persist;
  unrelated credentials remain intact. Reopening retains the catalog without another request.
- Copilot failures: denied/invalid token and malformed/rate-limited model responses preserve
  saved bytes, perform no retry, and never switch to a configured API-key billing path.
- Caller cancellation during catalog response: refreshed credentials and the live catalog
  both update even after the resolving caller is aborted.
- Radius: default and custom gateways receive their own refresh requests; eight resolutions
  deduplicate to one request. Rotated refresh token, access, scope and one-minute expiry skew
  persist and survive reopening.
- Radius failures: invalid grant, missing rotating token and rate limits leave saved credentials
  intact and never fall back to the environment API key.

The cancellation regression failed before the fix: the owned refresh task persisted the new
token, but catalog reload ran only in the cancelled caller. Reload now runs inside the owned
task, after releasing the credential file lock. It also observes a refresh won by another
request/process. The existing bounded refresh timeout is retained.

Pinned source audit at `d2a311097cbcf669e699479587332ae3988a49d0`:
`packages/ai/src/auth/oauth/github-copilot.ts` explicitly disables model-request retries in
`refreshGitHubCopilotToken`; login retries are separate. The ledger's refresh retry claim was
incorrect. Radius `auth/oauth/radius.ts` requires a rotated refresh token and uses the same
expiry skew. These are source comparisons, not executed differential tests.

Validation: five targeted tests pass. Full workspace: **1509 passed, 0 failed, 53 ignored**.
Clippy and formatting pass after removing one test-only unnecessary clone warning.
Logs: `/tmp/pipkin-refresh-targeted2.log`, `/tmp/pipkin-refresh-workspace.log`,
`/tmp/pipkin-refresh-clippy.log`, `/tmp/pipkin-refresh-fmt.log`.
Initial targeted run: three passed, two failed. One failure reproduced the cancellation bug;
the other exposed a custom-gateway test fixture missing its required `pi-messages` API field.
Both are corrected. No failed requirement was waived.

## Gaps identified during this batch

The Radius catalog gap below is subsequently implemented and tested in
[radius-catalog.md](radius-catalog.md). Copilot login retry parsing is subsequently fixed and tested in [copilot-login-retries.md](copilot-login-retries.md).

Reference `packages/ai/src/providers/radius.ts` restores independent persisted model metadata,
migrates legacy OAuth `gatewayConfig`, and refreshes authenticated `{gateway}/v1/config`.
Native currently restores only legacy credential metadata, explicitly excludes Radius from
public catalog refresh, and has no private gateway fetch. Token rotation drops legacy
`gatewayConfig`. PRV-radius therefore reopens for required implementation; successful OAuth
refresh does not qualify dynamic catalogs.

Copilot login's Retry-After parser also needs follow-up: reference accepts HTTP dates and
rejects non-finite numbers, while the native numeric-only parser may panic on non-finite input.
This does not affect the no-retry refresh path measured here.

Native remains opt-in. Full provider/platform/installed qualification remains open.
