# Codex WebSocket proxy TLS — 2026-10-11

Implemented on `native-engine` on top of **1077005**. Protocol 5 / schema 4 are unchanged.
Native remains opt-in; Pi remains the default. The owner's existing plan edit and untracked
worktrees are untouched. No live account, paid provider call or desktop interaction was used.

## Changes

- The WebSocket transport now tunnels through both HTTP and HTTPS proxies. For HTTPS proxies,
  TLS is negotiated and verified before sending CONNECT or proxy credentials. WSS negotiates
  a separate verified TLS session to the target inside the tunnel. Production uses the standard
  WebPKI roots; no insecure verification or trust-store modification is introduced.
- A boxed async socket lets the target TLS layer wrap either TCP or the proxy's TLS stream.
  Existing timeouts/cancellation encompass DNS, proxy TLS, CONNECT, target TLS and upgrade.
  CONNECT parsing remains bounded at 16 KiB and now requires a valid HTTP/1.x 2xx status.
- Proxy URL credentials retain percent decoding and Basic authorization on CONNECT only.
  Target authorization is sent only in the WebSocket handshake. NO_PROXY uses the existing
  explicit environment-selection rules. Pooling, closing and reconnecting work through TLS.
- The new grammar WebSocket test exposed a continuation gap: saved response items were encoded
  with the old message-only converter, losing their custom-tool shape. Continuation now uses
  the request's effective grammar declarations, matching subsequent full-context conversion
  without cloning the whole conversation. The next frame can send only the new tool output/input.
- `tokio-rustls` and `webpki-roots` are direct dependencies using versions already in the lockfile;
  no package versions changed or runtime dependencies were downloaded.

## Measured evidence

Five tests in `provider/api/openai_codex/tls_tests.rs`:

1. `http_and_https_proxies_carry_ws_and_wss_with_grammar_continuation`: all four proxy/target
   combinations; real local TLS, custom tool declaration/input, cached response ID and input
   delta, tunnel reuse, explicit close/reconnect, and separation of proxy/target credentials.
2. `tls_proxy_and_target_certificates_are_verified_independently`: untrusted proxy CA,
   incorrect proxy hostname, incorrect target hostname inside a trusted TLS proxy, and an
   untrusted target CA behind a plain HTTP proxy all fail before a target WebSocket handshake.
3. `no_proxy_bypasses_the_tls_proxy_without_disabling_target_tls`: direct WSS works with the
   trusted fixture while the configured HTTPS proxy sees no request.
4. `proxy_tls_handshake_obeys_connect_timeout_and_cancellation`: a stalled proxy TLS handshake
   respects the connect deadline and cancellation independently.
5. `tls_connect_rejects_refused_malformed_and_oversized_proxy_responses`: 407, invalid HTTP
   status lines and oversized heads fail without reaching a target WebSocket handshake.

Fixtures are documented in `fixtures/native-tls/README.md`. The localhost test key is deliberately
public. Only test-local configs trust its CA; the CA signing key was discarded. Tests use actual
Rustls certificate and hostname verification, not a permissive verifier. Fixture tasks own their
accepted connections and abort them when dropped.

Final required checks: **1358 passed, 0 failed, 53 ignored**; Clippy and formatting clean.
Logs: `/tmp/pipkin-proxy-final-workspace.log`, `/tmp/pipkin-proxy-final-clippy.log`,
`/tmp/pipkin-proxy-final-fmt.log`. The first workspace run passed tests but had a Clippy warning
about Tungstenite's fixed large callback error type; a scoped test-only allowance and a second
complete run resolved it. Earlier HTTP-proxy, fallback, reconnect and provider tests still pass.

## Remaining boundaries

This is local native transport evidence, not an executed Pi WebSocket differential or live
proxy/provider qualification. Conversation-level provider cleanup remains unimplemented in
the runtime (adapter-level close/reconnect is tested). Shared HTTP client forwarding versus
CONNECT for plain HTTP targets and sign-in/refresh proxy error formatting also remain open.
Authenticated external attachment, multi-client lifecycle and the wider release gates remain.
No default cutover, Pi retirement, desktop/package benchmark or platform qualification is claimed.
