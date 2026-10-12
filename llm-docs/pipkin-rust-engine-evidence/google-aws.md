# Google AWS external-account federation

2026-10-12, following `d0e949b`. Protocol 11 / schema 9 unchanged. Native Vertex ADC now
supports AWS external-account federation using environment or configured metadata sources.
Bedrock credential resolution remains separate; Google does not use the AWS CLI profile chain.

## Reference and implementation

The reference is Pi `d2a311097cbcf669e699479587332ae3988a49d0` with Google auth library
**10.6.2**. `scripts/capture-google-aws-reference.mjs` executes `GoogleAuth` and the original
AWS supplier/signer with a fixed clock and synthetic recording transporter. Retained evidence
in `fixtures/native-google-aws/reference.json` includes thirteen cases, source hashes and
lockfile package integrity. The script checks the pin, unchanged reference lock/provider
files and SDK version; installed SDK bytes are not independently checked against the registry
archive. No request reaches a real AWS or Google account.

Supported behavior:

- Region precedence is `AWS_REGION`, `AWS_DEFAULT_REGION`, then the configured region
  endpoint's availability-zone response with its final character removed. The discovered
  region is retained for the identity's lifetime, matching the SDK. Credentials come from
  both `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY` together, plus optional session token;
  incomplete environment credentials use the configured metadata source instead.
- Metadata credentials reload the role name and credentials on refresh. IMDSv2 obtains
  separate session tokens for region and credential discovery, requests a 300-second TTL
  and forwards the correct token to subsequent GET requests. Google access-token caching
  avoids any supplier work while the token remains valid.
- The proof is a URL-component-encoded serialized POST GetCallerIdentity request. Twenty
  exact signature/proof vectors include temporary credentials, raw query order, encoded
  paths and a nondefault port. The signer uses Google's path/query canonicalization,
  retaining its post-sign audience header. It reuses native HMAC/SHA primitives rather
  than changing the Bedrock signer. The proof is then form-encoded for Google STS; Pipkin
  does not send the signed AWS request itself.
- ADC factory selection requires the original `credential_source.environment_id` keys.
  Other AWS source fields accept the SDK's camelCase aliases. A fully camelCase source
  object is handled by the SDK as an ordinary URL identity source; the captured case
  preserves that behavior rather than silently selecting a different identity mechanism.
  The same factory audit tightened executable selection to the original selector keys.
- Only the five relevant AWS environment values enter the owned task and hashed cache key.
  Session/region/credential changes isolate tokens; unrelated `AWS_PROFILE` values do not
  affect this Google source. Request environment isolation is respected. Cancellation
  releases only the waiter while the bounded owner finishes the exchange.

Metadata and STS work share a thirty-second owner budget. Metadata responses are bounded to
1 MiB; errors do not echo response bodies or credentials. Malformed types, unsupported AWS
versions and invalid URLs fail explicitly. This is supported JSON/HTTP behavior, not a claim
that all JavaScript malformed-value coercions or credential retry middleware are identical.
There is no new Node/runtime dependency and no Pi extension work.

## Verification

Five new tests cover twenty exact signature vectors, thirteen executed SDK discovery/grant
outcomes over two refreshes, region reuse with role rotation, IMDSv1/v2, partial/environment
sources, option aliases and source selection, cancellation/concurrency, environment identity
isolation, bounded/redacted failures, and a real thirty-second metadata timeout followed by
successful recovery. The Vertex external-account test now also completes two actual local
HTTP streams with an AWS proof, checking cached bearer/quota forwarding.

The focused Google suite passed **44 tests**. The first workspace run exposed a stale-record
race in the launcher fixture; clearing prior records before each iteration fixed it without
changing production launches. The focused launcher test passed. Final required checks passed
**1591 tests, 0 failed, 53 ignored**, with clean Clippy and formatting. Logs:
`/tmp/pipkin-google-aws-workspace.log` (initial failure),
`/tmp/pipkin-google-aws-launcher.log`, `/tmp/pipkin-google-aws-workspace2.log`,
`/tmp/pipkin-google-aws-clippy2.log`, `/tmp/pipkin-google-aws-fmt2.log`. The fixture clock is fixed only in the reference capture; native exchanges use
the real clock, and tests check the resulting timestamp and signed proof separately.

Remaining: certificate external-account sources, broader credential cache/retry/configuration
review, live cloud acceptance, sustained workloads and release/CI/platform qualification.
Native remains opt-in. These local comparisons do not qualify the entire provider or release.
