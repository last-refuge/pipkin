# AWS login-session credentials

2026-10-12, following `fe4a15a`. Protocol 11 / schema 9 unchanged. This implements the
separate `login_session` profile branch discovered in Pi's pinned AWS credential chain.
IAM Identity Center SSO was completed separately; these caches and refresh protocols differ.

## Contract and source evidence

`scripts/capture-aws-login-reference.mjs` executes the original `fromLoginCredentials` from
Pi revision `d2a311097cbcf669e699479587332ae3988a49d0` with synthetic files and a recording
HTTP handler. `fixtures/native-aws-login/reference.json` records versions, package-lock
integrities, source hashes, six cases, wire bodies, decoded proof headers/claims, signature
verification results, credential outcomes and saved metadata. Installed versions and the
reference source/lockfile are checked; installed bytes are not independently compared with
registry archives. All private-key and credential material in this fixture is generated
synthetic test material. It cannot authenticate to AWS.

The cases cover valid cached credentials, mandatory refresh, the default 900-second refresh
lifetime, transient failure with a still-valid token, expired credentials after failure, and
access denial with a still-valid token. The capture uses one SDK attempt to isolate these
outcomes. It does not qualify the SDK's retry middleware, live IAM policies or proxy behavior.

Native implementation:

- Reads `<SHA-256(login_session)>.json` under `AWS_LOGIN_CACHE_DIRECTORY`, otherwise
  `HOME/.aws/login/cache`. Existing credential-chain ordering remains intact.
- Validates required cache fields and returns cached temporary credentials when more than
  five minutes remain. Within that window it POSTs the reference JSON refresh grant to
  `/v1/token` on the profile region's Signin endpoint (or configured service/global endpoint).
- Builds an ES256 DPoP JWT with the cached P-256 key's public JWK, a fresh UUID, current
  issue time, POST method and actual endpoint. Ring parses and validates the key and emits
  the fixed-width signature. SEC1 CLI keys are wrapped in PKCS#8 with explicit curve OIDs;
  PKCS#8 keys are also accepted. No JavaScript or external signer is invoked.
- Preserves account and unknown cache metadata, updates access/refresh credentials and expiry,
  and writes through the existing private, locked, atomic JSON store. An intervening login
  with changed key/client/access/refresh data is not overwritten.
- Shares one refresh owner per cache path. A canceled waiter does not abandon a rotating
  refresh; another caller waits for it and rereads its committed cache. Session owners are
  bounded, and the queue/exchange has a thirty-second total budget. Files and responses are
  capped at 1 MiB. HTTPS is required except for loopback test/development endpoints.
- Falls back to still-valid credentials on ordinary refresh/persistence failure. Access
  denial is final, including malformed or oversized denial bodies; expired credentials are
  never returned. Error text excludes remote response content and cached secrets.

The private atomic writer, newer-login check, resource bounds and cancellation ownership are
native improvements. SDK timing, signature bytes (randomized), retry middleware and malformed
input acceptance are not claimed identical. The initial `aws login` remains external; this
work adds no login UI or AWS CLI runtime dependency.

## Verification

Eight new tests cover the six SDK outcomes, cryptographic proof verification and unique proof
IDs, cache filename/default/override locations, metadata and file permissions, shared refresh
after cancellation, protection of a newer login, malformed/oversized data and a real stalled
HTTP response. A Bedrock adapter test refreshes credentials, sends two signed ConverseStream
requests, decodes tool-use responses, and verifies that the second turn reuses the saved cache.

Initial five tests passed in `/tmp/pipkin-login-tests.log`. Expanded focused results are in
`/tmp/pipkin-login-tests2.log`. Both full workspace runs passed **1566 tests, 0 failed,
53 ignored**. Final review added normalized endpoint binding for DPoP (host casing/default
ports); the corrected implementation and assertion passed the final rerun in
`/tmp/pipkin-login-workspace2.log`. All-targets Clippy and formatting were clean in
`/tmp/pipkin-login-clippy2.log` and `/tmp/pipkin-login-fmt2.log`. No live AWS account was used, and no release/platform
qualification is inferred from these local tests.
