# Configured extra certificate roots

2026-10-12, following `d948887`. Protocol 11 / schema 9 unchanged.

## Gap and implementation

The native HTTP and Codex WebSocket clients previously used bundled WebPKI roots only.
Pi inherits Node's `NODE_EXTRA_CA_CERTS` startup configuration. A private provider, credential
endpoint or HTTPS proxy trusted by that CA file could therefore fail after migration.

`provider/trust.rs` now loads that same configuration name from the engine's effective
`Env`, including its isolated-environment rules. It accepts PEM certificate bundles, keeps
bundled roots, validates additional certificates and bounds regular-file reads to 1 MiB.
Missing/unreadable/oversized files and invalid certificate entries produce diagnostics; they
never disable peer or hostname verification. Like measured Node behavior, valid certificates
before a malformed entry remain loaded, and certificates after it are not loaded. Empty
files and non-PEM text add nothing. Neither file contents nor filenames appear in diagnostics.

`NetSettings` owns the roots alongside proxy and idle settings. Ordinary HTTP, sign-in,
credential exchange and certificate-authentication clients inherit them through the shared
client builder. Codex WebSockets use the same roots for both target TLS and HTTPS proxy TLS.
The existing tests' private CA injection remains test-only; production adds certificates only
from the configured file, never from fixtures.

Native also gains live certificate-file reload at the next network snapshot. In-flight
requests retain their previous snapshot. A WebSocket pool entry records a hash of endpoint,
proxy, handshake headers and trusted roots, so changed trust/identity/destination cannot reuse
an old connection or its continuation state. This also fixes the prior possibility of reusing
a socket after a destination or credential change within the same session/account. Native
still preserves Pi's session-level SSE fallback after a WebSocket transport failure; restoring
a CA does not silently undo that fallback. New sessions can use WebSocket again.

## Executed reference

`scripts/capture-extra-ca-reference.mjs` executes **Node v26.8.1** and Pi's pinned **undici
8.10.2** against a real local HTTPS server. Each case launches an isolated child environment
so Node sees `NODE_EXTRA_CA_CERTS` at startup. It checks Pi revision
`d2a311097cbcf669e699479587332ae3988a49d0`, unchanged lockfile/dispatcher and installed undici
version. `fixtures/native-extra-ca/reference.json` records runtime version, package integrity,
entry and dispatcher source hashes. The installed dependency tree is not independently checked
against a registry archive. The runner configures undici directly; it does not execute Pi's
whole dispatcher module or qualify the whole transport.

Ten cases capture TLS success/failure, loaded root counts and warning presence for no setting,
a valid CA, duplicates, text preceding a CA, empty/garbage files, a malformed certificate,
valid-before-malformed, malformed-before-valid and a missing file. All use synthetic public
fixture certificates and local sockets; no account credentials or external service is used.

## Verification

Five new tests cover:

- All ten executed reference cases over verified local HTTPS, including root/warning counts.
- Root removal/restoration and retained active HTTP snapshots; the Google identity client
  builder inherits the configured trust roots as well.
- File size/type limits, isolated empty overrides and bounded diagnostic content.
- A running engine's network snapshot reloads changed CA contents and rebuilds its client.
- Codex WSS through both HTTP and HTTPS proxies, normal reuse, revoked-root rejection,
  reference-compatible fallback, a fresh session after restoration, changed destination
  isolation and rejected hostname mismatch, using production CA configuration.

Initial compilation caught one old NetSettings test literal without the new field and a wrong
WebSocket debug accessor call in the new fixture. The first TLS run passed two tests and
failed an incorrect expectation that restoring roots clears the reference's permanent SSE
fallback. The next run passed four tests and failed because the running-engine test constructed
an HTTP request future outside a Tokio reactor. Both fixture mistakes were corrected without
relaxing production behavior. Logs are retained in `/tmp/pipkin-extra-ca-check.log` and
`/tmp/pipkin-extra-ca-tests{,2,3}.log`.

Corrected focused suite: **5 passed, 0 failed**, `/tmp/pipkin-extra-ca-tests4.log`.
Required workspace checks: **1615 passed, 0 failed, 53 ignored**; clean Clippy and formatting.
Logs: `/tmp/pipkin-extra-ca-workspace.log`, `/tmp/pipkin-extra-ca-clippy2.log`,
`/tmp/pipkin-extra-ca-fmt.log`. No live account or desktop acceptance is inferred from these
local TLS and engine tests.

## Remaining qualification

This closes the extra-CA-file gap, not every Node TLS configuration option. OS trust-store
selection and Node/OpenSSL-specific trust flags need explicit scope/audit before claiming
complete trust configuration parity. Plain HTTP proxy tunneling, unusable-proxy diagnostics
outside model requests, credential retry middleware, live corporate deployments, sustained
workloads and clean release/CI/platform qualification remain open. Native is still opt-in.
