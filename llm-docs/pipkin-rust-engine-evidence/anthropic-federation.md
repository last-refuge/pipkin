# Anthropic workload identity federation

2026-10-11, following `d211c03`. Protocol 11 / schema 9 unchanged. This closes an actual
missing implementation: the native credential resolver recognized federation environment
variables, but the Anthropic adapter explicitly refused to exchange them. The ledger's old
"measured (refusal only)" label was not evidence of usable federation.

## Reference and implementation

Pinned Pi `d2a311097cbcf669e699479587332ae3988a49d0` registers federation in
`packages/ai/src/api/anthropic-messages.ts::getAnthropicFederation` and delegates exchange
and refresh to `@anthropic-ai/sdk` **0.129.0**. The SDK sources inspected and executed were
`lib/credentials/{oidc-federation,identity-token,token-cache,types}`. Source hashes and the
lockfile integrity are recorded in `fixtures/native-anthropic-federation/reference.json`.
The SDK package is reference input, not a native build/runtime dependency.

Native federation now:

- Requires rule, organization and token-file environment values for the Anthropic provider;
  optional service-account and workspace values enter the token exchange body.
- Reads and trims the current projected identity-token file on each exchange and POSTs the
  RFC 7523 JWT-bearer grant to `{base}/v1/oauth/token` with the SDK's OAuth/federation betas.
- Sends the minted bearer token and OAuth beta on model requests, with ordinary API tool
  names/system content. It does not apply Claude subscription identity or billing behavior.
- Retains one current configuration's in-memory cache per adapter, including workspace,
  endpoint and file path in its key. Concurrent mandatory refreshes share one exchange.
  Requests with more than 120 seconds left use the cached token; 30–120 seconds trigger
  advisory background refresh; 30 seconds or less wait for fresh credentials. Advisory
  failures retain the valid token and back off for five seconds; mandatory refresh bypasses
  that backoff. Engine restart rereads the file and exchanges again.
- Invalidates a token on HTTP 401 before consuming the error response. As in Pi's
  `maxRetries: 0` SDK request, that rejection is surfaced; the next request exchanges again.
  A late 401 cannot discard a newer token. Status observation leaves other adapters' shared
  retry behavior unchanged.
- Keeps explicit API keys, bearer tokens, subscription credentials and configured auth
  headers ahead of federation. Other Anthropic-compatible providers do not acquire it.

Native safeguards are explicit differences from the SDK internals: cache-owned refreshes
outlive a canceled waiter but are bounded to 30 seconds or the shorter request timeout,
including body reads. Identity files must resolve to regular files and are capped at 1 MiB;
trimmed assertions keep the SDK's 16 Ki UTF-16-unit limit. Token response bodies are capped
at 1 MiB, invalid/non-finite/unrepresentable expiry is refused without a panic, and errors
retain bounded standard OAuth fields while omitting/redacting credential material. HTTPS
is required except for SDK-compatible loopback HTTP development endpoints. The exchange
User-Agent identifies Pipkin, while protocol headers match the SDK. This is not a live
Anthropic organization acceptance claim.

The source port's exact MIT notice and provenance are stored under
`packaging/upstream-notices/anthropic-sdk/`. Linux native/Pi packaging and experimental macOS
packaging include it. No JavaScript runtime is added.

## Measured evidence

Nine new tests cover:

1. Exchange body/headers, cached reuse, workspace switching and rotated identity files.
2. Eight concurrent callers joining a refresh after its first waiter cancels.
3. Advisory failure/backoff followed by mandatory refresh from a rotated file.
4. 401 invalidation and a late rejection of an older token.
5. Explicit credential/header precedence and refusal to apply federation to another provider.
6. Missing/invalid token response fields, empty/oversized assertions, insecure endpoints and
   secret redaction, without sending model requests after failed authentication.
7. A real stalled token body bounded by the configured total timeout.
8. Two exchange fixtures and six cache transitions executed from the original pinned SDK,
   compared with native request bodies/headers and token/cache outcomes. The advisory exchange
   count is compared after its refresh settles; task scheduling timing is not claimed equal.
9. Real native engine protocol turns with ambient federation resolution, cached reuse and
   session continuation after engine restart with a new identity assertion.

All nine passed in `/tmp/pipkin-federation-tests3.log`. Existing Anthropic adapter tests passed
before the new tests were added. The original SDK capture initially hit sandbox subprocess
restrictions; rerunning with local execution permission succeeded. Initial test development
also caught private test-module access and a missing new notice in the macOS policy fixture;
the runtime test was moved into its runtime suite and the packaging fixture now asserts the
notice is retained. Those failures are recorded in `/tmp/pipkin-federation-tests2.log` and
`/tmp/pipkin-federation-package-macos.log`. The corrected macOS policy test passed in
`/tmp/pipkin-federation-package-macos2.log`; it is a mocked packaging-policy test, not native
macOS execution.

Reproduce reference capture with `scripts/capture-anthropic-federation-reference.mjs`
and the read-only pinned Pi checkout plus its SDK directory. Capture uses fake fetch/clock
inputs and temporary synthetic token files, never live credentials.

Full workspace checks passed: **1543 passed, 0 failed, 53 ignored**, clean Clippy and
formatting. A fresh Linux native-only package passed installation, notice retention,
worker handshake/session listing, Node/npm/Pi trap-shim checks and uninstall/data preservation.
The candidate identifies `d211c03` with a dirty tree; archive SHA-256 is
`c4396027491f5cd6ee84eb64f7b5878516fd77c8ff9412af460e20dfc3a9a0f6`.
These package checks did not submit a coding turn. Logs and the stale subscription diagnostic
discovered during the probe are recorded in the progress record. ADP-ANTHROPIC-FEDERATION
now has implementation, local runtime and executed SDK-contract evidence. Live organization,
proxy/TLS federation deployments, installed desktop acceptance and full release qualification
remain unverified; those are not inferred from local fixture success.
