# Google certificate federation and credential transport settings

2026-10-12, following `d1809e7`. Protocol 11 / schema 9 unchanged. Native Vertex ADC now
supports certificate sources, certificate-chain subject tokens and mutual TLS for Google
credential exchanges. No JavaScript runtime, native TLS bypass or fixture CA is added to
production builds.

## Reference and boundaries

`scripts/capture-google-certificate-reference.mjs` executes Google auth library **10.6.2**
from Pi `d2a311097cbcf669e699479587332ae3988a49d0` against a local HTTPS server requiring
client authentication. Synthetic certificate/key fixtures live in
`fixtures/native-google-certificate/`; the issuing CA private key was discarded. The
reference Node process trusts the synthetic CA through `NODE_EXTRA_CA_CERTS`, while native
tests use a test-only trusted-root field. Neither disables verification.

Nineteen captured cases exercise explicit/env/gcloud-default config discovery, leaf-only and
ordered trust chains, missing or malformed config/identity files, a mismatched key and both
external and source-client impersonation. The fixture retains exact STS/IAM bodies, bearer
headers and the TLS peer certificate fingerprint. Reference revision, lockfile integrity and
executed SDK source hashes are recorded. The installed SDK tree is not independently checked
against the package registry archive.

The transport boundary was audited through the pinned `@google/genai` Node implementation:
`NodeAuth.addGoogleAuthHeaders` calls `GoogleAuth.getRequestHeaders` and copies headers.
The certificate identity belongs to the auth client's STS and impersonation transports.
`Impersonated.refreshToken` uses `sourceClient.request`, so an immediate certificate source
also authenticates that IAM request. It does not attach the certificate to genai's ordinary
model HTTP requests. Native preserves these boundaries; a real TLS Vertex test verifies a
certificate on STS and no certificate on either of two bearer-authenticated model streams.

## Implementation

- Certificate config requires exactly one explicit `certificate_config_location` or
  `use_default_certificate_config`. Defaults consult `GOOGLE_API_CERTIFICATE_CONFIG`, then
  `CLOUDSDK_CONFIG`, then the platform gcloud config location. An invalid explicit or env
  path fails rather than falling through to a different identity. Config supplies
  `cert_configs.workload.cert_path` and `key_path`.
- The leaf certificate is decoded and parsed as X.509. The optional trust chain becomes a
  JSON array of DER/base64 certificates: prepend a missing leaf, retain an already-first
  leaf, reject a leaf later in the chain. Like the SDK, non-PEM text in a trust-chain file is
  ignored; complete invalid certificate blocks fail. The certificate file itself supplies
  the TLS chain; `trust_chain_path` does not add trusted server roots.
- A private client identity is built with the same proxy, connection/pool and idle settings
  as the engine client. Ordinary turns and compaction now carry an atomic snapshot of that
  client and its settings into adapters. Network configuration enters the Google cache key,
  preventing cached credential state from hiding a configuration change. Standalone adapter
  test contexts can omit settings and use the existing direct-client defaults.
- All certificate/config/key/chain files are regular-file checked and limited to 1 MiB each.
  The existing thirty-second refresh owner bounds loading, TLS and exchanges. Errors do
  not echo file contents. Certificate rotation is read on refresh; a valid cached access
  token remains usable without rereading its source, matching the SDK. No source file is
  rewritten. Caller cancellation does not abandon the owned exchange.
- The enlarged parallel suite exposed an existing cache defect: the 64-identity cache chose
  an arbitrary idle entry for eviction. It now chooses the least recently used idle entry,
  never an active owner, with a deterministic capacity/recency regression test.

SDK JavaScript malformed-value coercions are not reproduced. Native can normalize a DER
certificate file into PEM for its identity builder. Local CA trust is test-only here; broader
server trust configuration, live enterprise/cloud acceptance and retry behavior are not
claimed qualified by this work.

## Verification

Seven tests were added: six certificate tests and one cache eviction test. They cover all
nineteen executed reference cases using real mutual TLS, refreshed certificate fingerprints,
mismatched keys, canceled/shared refresh, valid-token cache reuse, changed network identity,
oversized files, rejected untrusted servers, actual CONNECT proxy routing, a measured idle
timeout and successful recovery, and two Vertex streams without certificate disclosure.

An initial test build used the wrong `Env::isolated` argument type; it was corrected. The next
Google run passed 49 tests and failed the existing Vertex cache-reuse assertion. After fixing
arbitrary idle eviction, the focused suite passed **51 tests** in
`/tmp/pipkin-google-certificate-tests3.log`; Clippy was clean in
`/tmp/pipkin-google-certificate-clippy3.log`. Final workspace checks passed **1598 tests, 0 failed, 53 ignored**, with clean Clippy and
formatting. Logs: `/tmp/pipkin-google-certificate-workspace.log`,
`/tmp/pipkin-google-certificate-clippy3.log`, `/tmp/pipkin-google-certificate-fmt.log`. Earlier logs are retained, including the failed run in
`/tmp/pipkin-google-certificate-tests2.log`.

All known Google ADC source implementation gaps in the ledger are now covered by local
tests. Broader provider/cache/retry/configuration review, sustained workloads, live account
acceptance, release/CI/platform qualification and the default switch remain open. Native is
still opt-in; this does not qualify the whole provider or complete Pi retirement.
