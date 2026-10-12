# Google ADC discovery, impersonation and cache isolation

2026-10-12, following `8abc540`. Protocol 11 / schema 9 unchanged. Native Vertex previously
handled only service-account and authorized-user files, and reused a global token cache keyed
by path/endpoint. It now handles impersonated-service-account files and metadata credentials,
with independent refresh ownership keyed by the parsed credentials and selected configuration.

## Reference and implementation

The reference is Pi `d2a311097cbcf669e699479587332ae3988a49d0`, Google auth library **10.6.2**
and gcp-metadata **8.1.2**. `scripts/capture-google-adc-reference.mjs` executes the original
GoogleAuth/Impersonated clients with a recording synthetic transporter, and Compute with a
synthetic metadata supplier. It records package versions, lockfile integrity values and source
hashes in `fixtures/native-google-adc/reference.json`. The capture verifies the Pi revision,
unchanged source/lockfile and installed versions; it does not independently verify installed
package trees against registry archives. No live credentials or cloud services are used.

Native behavior:

- Explicit credential files take precedence and fail if missing/invalid. They never silently
  fall back to a VM identity. With no explicit path, the normal gcloud ADC file is tried first;
  only its absence permits metadata fallback. Lowercase credential-path environment spelling
  is supported, and `GOOGLE_CLOUD_QUOTA_PROJECT` overrides the file's billing project.
- Impersonation first obtains source credentials, then sends a bearer-authenticated IAM
  `generateAccessToken` request with delegates, cloud-platform scope and requested lifetime.
  Target identity comes from the configured impersonation URL, while the IAM endpoint is
  constructed as the SDK does, including its explicit `endpoint` override. Source billing
  accompanies the IAM request; target/override billing accompanies Vertex requests. Nested
  impersonation is supported, with a depth bound and pre-exchange universe consistency checks.
- Metadata fallback uses the Google instance default-service-account token path and the
  cloud-platform scope, sends `Metadata-Flavor: Google` and requires that response header.
  `GCE_METADATA_IP` precedes `GCE_METADATA_HOST`; the default is the link-local metadata IP.
  Detection modes are parsed case-insensitively. Explicit `none` disables discovery; `bios-only`
  checks serverless environment markers or Linux DMI/MAC indicators before requesting a token.
- The native path requests the token directly instead of performing a separate availability
  probe. Metadata traffic has a three-second timeout. It does not reproduce the SDK's race
  between IP and DNS availability probes or its unbounded on-GCP timeout. Linux residency
  detection is implemented; no macOS/Windows cloud-residency qualification is claimed.
- Provider discovery remains consistent with Pi's file/configured-key checks. On a deployment
  using metadata without a local file, the existing `gcp-vertex-credentials` key marker selects
  ADC; project and location remain required. No unsolicited startup network probe is added.

The old global cache could reuse an identity after the credential file at its path was
replaced, and held one lock across unrelated network exchanges. Native now reads the bounded
file before selecting a cache entry and hashes its parsed contents together with path,
endpoint, metadata configuration and quota. Up to 64 identities retain independent refresh
owners; idle entries may be evicted. Cancellation stops the waiter while its bounded owner
finishes, making the result available to another caller. No model history is cloned into an
authentication task. Credential and response bodies are capped at 1 MiB, lifetime arithmetic
is checked, and the queue/exchange has a thirty-second budget (file lookup has its own bound).

## Verification

Eight new tests cover:

- Two executed SDK impersonation grants, with/without delegates, source/target billing and
  cached reuse; the scope/metadata-supplier contract from an executed Compute client.
- Explicit-file failure and disabled discovery without metadata requests; case-insensitive
  detection and a synthetic serverless marker; metadata headers, oversized and stalled bodies.
- Replacement credentials at one path, quota changes, shared refresh after cancellation and
  independent progress for a second identity.
- Invalid universe/nesting rejection before source exchange.
- Two actual Vertex streams for each new credential path, including metadata via the existing
  ADC marker, bearer/quota headers and reuse without repeated credential exchanges.

The initial new fixture omitted a required model ID, failing seven tests before authentication;
19 existing Google checks passed. The fixture was corrected; all 26 then passed. Adding the
Vertex stream test produced 27 passing Google checks. Logs: `/tmp/pipkin-google-adc-tests.log`,
`/tmp/pipkin-google-adc-tests2.log`, `/tmp/pipkin-google-adc-tests3.log`. Both required workspace runs passed **1574 tests, 0 failed, 53 ignored**. Final review added
inherited universe handling for nested impersonation; the final run includes that assertion.
Logs: `/tmp/pipkin-google-adc-workspace2.log`, `/tmp/pipkin-google-adc-clippy2.log`,
`/tmp/pipkin-google-adc-fmt2.log`; Clippy and formatting are clean.

Still required: external-account federation (including its distinct source types and
external-account authorized-user credentials), credential retry behavior comparison, live
cloud acceptance and broader release/platform qualification. Local metadata fixtures are
not evidence of a real GCE/Cloud Run deployment. Native remains opt-in.
