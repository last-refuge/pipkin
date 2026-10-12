# Google executable ADC helpers

2026-10-12, following `d721b3a`. Protocol 11 / schema 9 unchanged. Native external-account
ADC supports configured executable subject-token suppliers. These are Google authentication
helpers, independent of Pi extensions; no JavaScript runtime is added to Pipkin.

## Reference and contract

`scripts/capture-google-executable-reference.mjs` executes Google auth library **10.6.2**
from Pi `d2a311097cbcf669e699479587332ae3988a49d0`. The original `GoogleAuth` client invokes
real local helper processes and reads real output caches with synthetic credentials. The
fixture `fixtures/native-google-executable/reference.json` retains thirteen outcomes, the
helper environment, command parsing results, package integrity metadata and source hashes.
The script verifies the reference revision, unchanged lock/provider files and SDK version;
it does not independently verify the installed SDK tree against its registry archive.

Native preserves the supported contracts:

- `GOOGLE_EXTERNAL_ACCOUNT_ALLOW_EXECUTABLES` must equal `1`, including for an output-cache
  lookup. Commands use the SDK's double-quote grouping and direct process execution. A shell
  is used only when explicitly configured as the executable. Single quotes and shell syntax
  retain the SDK's literal argument behavior.
- Helpers receive the effective request environment plus the Google audience, token type,
  noninteractive flag, configured output file and impersonated email. Isolated requests do
  not inherit ambient variables. The environment snapshot is included in the hashed token
  cache identity; changing helper credentials or disabling helpers cannot reuse another
  configuration's access token. Model requests/history are not cloned into refresh owners.
- Version-1 OIDC `id_token`/`jwt` and SAML2 responses are accepted. A successful output-file
  response requires an expiration. Valid cached replies bypass the helper. Missing or
  empty/non-file caches, expired replies and unsuccessful replies trigger execution;
  malformed caches fail. The helper owns cache writes; Pipkin does not rewrite its ADC file
  or output cache. Stdout and stderr contribute to the JSON response, as in the SDK.
- The default helper timeout is 30 seconds; configured timeouts range from 5 to 120 seconds.
  The owned exchange budget adds the helper allowance to the existing 30-second network
  budget, including when it is nested under service-account impersonation. Caller cancellation
  releases the waiter while the bounded owner finishes and preserves the obtained token.
- Output files and combined process output are capped at 1 MiB. Helper failures, malformed
  output, excessive output and failures reported inside JSON do not echo response secrets.
  On Unix, timeout/error cleanup kills the helper's process group, including descendants
  retaining output pipes. Successful processes are awaited even after their pipes close.

The process-group cleanup and secret-safe errors improve on the reference's single-child
termination and verbatim error output. Rust rejects malformed response field types rather
than reproducing JavaScript coercions. UTF-8 environment values are supported. Cross-stream
chunk scheduling, live identity-provider policies and SDK retry middleware are not claimed
identical. All process measurements here are on Linux; other platforms remain unverified.

## Verification

Five new tests cover the thirteen executed SDK cases, four command parsings, default/maximum
budget selection, a real 31-second successful helper, a five-second timeout with descendant
termination and subsequent recovery, cancellation with shared refresh, environment isolation,
response/file bounds, redacted errors, no-expiry responses and successful exit after closing
pipes. The existing Vertex external-account test now also executes a helper and completes
two actual local HTTP streams, checking the STS subject, bearer/quota forwarding and reuse.

An initial compile caught a module visibility error, which was corrected. An initial test
filter matched zero tests; this was not counted as verification. The corrected Google suite
passed **39 tests** (see `/tmp/pipkin-google-executable-tests2.log`). Final workspace checks passed **1586 tests, 0 failed, 53 ignored**, with clean Clippy and
formatting. Logs: `/tmp/pipkin-google-executable-workspace.log`,
`/tmp/pipkin-google-executable-clippy2.log`, `/tmp/pipkin-google-executable-fmt.log`.

Remaining: AWS/certificate external-account sources, credential retry/configuration review,
live cloud acceptance, sustained desktop workloads and release qualification. Native remains
opt-in; this evidence does not qualify the whole provider or authorize the default switch.
