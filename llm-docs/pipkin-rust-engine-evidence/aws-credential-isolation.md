# AWS credential isolation and bounded helpers

2026-10-12, following `0c3d742`. Protocol 11 / schema 9 unchanged.

## Defects and implementation

The prior global AWS cache used profile names for process credentials, profile/role names
for assumed roles, endpoint URLs for container credentials and one key for all EC2 metadata.
Different configurations could therefore reuse another identity's credentials. Bedrock's
ambient lookup also ignored the request's isolated-environment flag, and credential helpers
inherited the engine process environment instead of the request's effective environment.

Cache keys now hash source configuration, profile selection, region and endpoint, plus the
relevant identity: full helper environment and profile definition, source signing credentials
and role parameters, web-identity token contents/session, container authorization, or metadata
endpoint. Raw credentials and helper environment are not stored in cache keys or logged.
The cache retains at most 64 expiring identities, removes entries within five minutes of
expiry and evicts by least recent use. Permanent helper credentials remain uncached.

Bedrock honors isolated requests throughout environment/profile/region lookup. Helpers get
an explicit environment snapshot, including arbitrary request overrides and empty values.
They retain shell command semantics, run with closed stdin, bound stdout and stderr separately
to 1 MiB and retain the existing 60-second timeout. Cancellation, timeout and oversized output
kill the owned Unix process group; errors do not include helper output or command contents.
Expired helper credentials fail before signing. Token files must be regular UTF-8 files and
are read with a 1 MiB/30-second bound; credential HTTP responses are bounded to 1 MiB.

These are authentication fixes and bounded-resource improvements, not Pi extension support.

## Executed reference and regressions

`scripts/capture-aws-process-reference.mjs` runs the installed AWS SDK
`credential-provider-process` **3.972.70** from Pi's pinned dependency set. It checks Pi revision
`d2a311097cbcf669e699479587332ae3988a49d0`, unchanged lockfile/Bedrock adapter, and installed
package version. `fixtures/native-aws-process/reference.json` records lockfile integrity and
executed provider source SHA-256. The installed tree is not independently verified against
the registry archive. Only synthetic local shell helpers are executed; no cloud requests or
account credentials are used.

Twelve reference cases cover permanent/temporary credentials, whitespace, successful stderr,
shell expansion, expiry, invalid version/missing keys/invalid JSON, nonzero exit, and both
output limits. Native compares returned key/token/expiry fields and failure categories.
Error text intentionally redacts helper details. Invalid expiration strings are rejected
locally rather than preserving JavaScript's invalid-Date coercion. AccountId/CredentialScope
metadata are not compared here; the current Bedrock signing path consumes key/token fields.

Twelve added tests cover those reference outcomes; environment, parsing and redaction;
output bounds and recovery; cancellation/timeout descendant cleanup; bounded cache eviction;
identical profile names across homes and changed helper commands/environments; rotated source
keys and role-session/external-id parameters; changed web identity tokens/sessions; changed
container authorization/token files; distinct IMDS endpoints; token/HTTP body bounds; and an
actual local Bedrock event stream signed by an isolated helper identity despite conflicting
ambient credentials. Timeout cleanup tests shorten the helper budget to 150 ms; they do not
claim a measured 60-second timeout. Process tests are measured on Linux only.

## Verification and remaining work

Initial AWS focused run: **38 passed, 0 failed**, `/tmp/pipkin-aws-isolation-tests.log`.
Provider suite: **264 passed, 0 failed**, `/tmp/pipkin-aws-isolation-provider-tests.log`.
Final workspace: **1610 passed, 0 failed, 53 ignored**,
`/tmp/pipkin-aws-isolation-workspace2.log`. Clippy and formatting are clean in
`/tmp/pipkin-aws-isolation-clippy2.log` and `/tmp/pipkin-aws-isolation-fmt.log`.
The initial workspace run also passed. Its log and the initial Clippy log (one collapsible-if
warning, since corrected) are retained as `...-workspace.log` and `...-clippy.log`.
The final workspace run includes the cache style correction and retaining the helper cleanup
guard when waiting for a child fails.

Live AWS acceptance, SDK credential retry middleware, broader proxy/server-trust settings,
sustained desktop workloads, clean release/CI qualification and default cutover remain open.
Native remains opt-in. This batch does not qualify the whole adapter or retire Pi.
