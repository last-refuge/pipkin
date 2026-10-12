# Extra server certificate reference

`reference.json` was captured by `scripts/capture-extra-ca-reference.mjs` using the Pi
checkout and installed dependency root identified in the artifact. The runner uses Node
startup environments, pinned undici and a real local HTTPS server. It does not invoke the
entire Pi application or qualify all HTTP transport behavior.

The server key/certificate and CA are the intentionally public synthetic fixtures in
`../native-google-certificate/`. Temporary CA bundles and isolated child HOME directories
are removed after capture. No real account credentials or network service are used.

Native comparisons live in `provider/trust_tests.rs`; configured WebSocket trust is tested
in `provider/api/openai_codex/tls_tests.rs`. The capture records original Node's loaded root
counts and warning presence, including its retention of valid certificates before a malformed
one. Native additionally measures live reload and safe connection reuse.
