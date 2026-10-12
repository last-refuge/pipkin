# Google file/URL federation and external authorized-user credentials

2026-10-12, following `28b0e4d`. Protocol 11 / schema 9 unchanged. Native Google ADC now
handles external-account file/URL identity sources, workforce STS options, optional service
account impersonation and external-account authorized-user refresh grants.

## Reference and implementation

The baseline is Pi `d2a311097cbcf669e699479587332ae3988a49d0` with Google auth library
**10.6.2**. `scripts/capture-google-external-reference.mjs` executes the original GoogleAuth
external clients against synthetic files and a recording transporter. Eight retained cases
cover file text, file JSON, URL JSON, workforce billing, Basic client-auth precedence,
impersonation, omitted STS expiry and external-user refresh rotation. Versions, lockfile
integrity and executed source hashes are in `fixtures/native-google-external/reference.json`.
The script checks the reference revision, unchanged source/lockfile and installed version;
it does not independently verify the installed package tree against the registry archive.

Native preserves these contracts:

- Text subject files retain their bytes as UTF-8, including trailing newlines. JSON sources
  extract the configured top-level string field. URL suppliers send configured headers and
  use the same text/JSON rules. File sources must resolve to regular files. The subject is
  read again at refresh time; a valid cached Google access token does not reread its source.
- STS receives the RFC 8693 grant, audience, cloud-platform scope, requested access-token type,
  subject token and subject-token type. Workforce user-project options require a workforce
  audience; Basic client authentication takes precedence over those options.
- External impersonation calls the configured URL with the STS bearer and scope/lifetime
  JSON. Its payload intentionally differs from the separate impersonated-service-account
  credential type, which also carries delegates. Model billing comes from the ADC quota or
  environment override, not the STS workforce option.
- External authorized-user credentials use a refresh grant with Basic client authentication.
  Rotated refresh tokens are retained in the per-identity state, including when the original
  waiter cancels. Like the SDK, this path does not rewrite the user's ADC file; an engine
  restart reads the configured file again. It is not a durable credential-file update claim.
- Snake-case keys take precedence, with the SDK's camelCase aliases for external-account
  options. Omitted STS expiry retains a cached token without an invented expiration; an
  explicit expiry is checked. Native cache identity still includes configuration contents,
  endpoints and quota, and old/new identities never share rotated refresh state.

The shared ADC owner bounds the whole exchange to thirty seconds and isolates concurrent
identities. File and response bodies are capped at 1 MiB; malformed fields, excessive
lifetimes and remote failures produce errors without echoing subject tokens or response
secrets. HTTP(S) subject/STS URLs support configured metadata/issuer endpoints. Captured
semantic headers/form/JSON fields are compared; SDK metrics, user agents, retry middleware,
malformed-input coercions and live cloud policies are not claimed identical.

## Verification

Seven new tests cover all eight executed cases, raw subject preservation, file rotation at
refresh, camel-case options, workforce validation, Basic-auth precedence, external-user
rotation after cancellation, unchanged ADC files, no-expiry cache reuse, invalid/missing/
ambiguous/oversized sources, failed/oversized responses and lifetime overflow. A real stalled
subject response consumes the thirty-second budget, then a subsequent exchange succeeds.
A Vertex adapter test completes two HTTP streams for each of the file-federation and
external-user paths, checking bearer/quota headers and credential-cache reuse.

The initial focused suite passed 32 tests in `/tmp/pipkin-google-external-tests.log`.
The expanded suite passed 34 in `/tmp/pipkin-google-external-tests2.log`. Initial Clippy
reported two control-flow/formatting warnings; both were corrected without behavioral
changes. Final workspace checks passed **1581 tests, 0 failed, 53 ignored**, with clean
Clippy and formatting. Logs: `/tmp/pipkin-google-external-workspace.log`,
`/tmp/pipkin-google-external-clippy2.log`, `/tmp/pipkin-google-external-fmt.log`.

Still open: external-account AWS, executable and certificate credential sources, credential
retry comparisons, live Google acceptance and release/platform qualification. These are
provider authentication sources, independent of the out-of-scope Pi extension runtime.
Native remains opt-in.
