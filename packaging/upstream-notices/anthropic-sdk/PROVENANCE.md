# Anthropic SDK source port

- Package: `@anthropic-ai/sdk` 0.129.0, pinned by Pi
  `d2a311097cbcf669e699479587332ae3988a49d0` in `package-lock.json`.
- Source archive: <https://registry.npmjs.org/@anthropic-ai/sdk/-/sdk-0.129.0.tgz>
- Published lockfile integrity:
  `sha512-MH7LB20kNpGLUpPTe2OG5XvL/KhX8HUO9XyBaFjgFk6TLS4Xjt0VPIuMKywe7POKKPslX+QxSlVok9e+8kG5Nw==`
- The adjacent LICENSE is copied verbatim from that installed reference package.
  SHA-256: `8bf96984ff8bcfae7e48cae76a529e8a25317ba9e02abf7fd3cc64fdf95657a6`.
- Native port: `crates/pipkin-engine/src/provider/api/anthropic_federation.rs`, covering
  `lib/credentials/{oidc-federation,identity-token,token-cache,types}` behavior.
- Executed source hashes, deterministic wire/cache outputs, and reproduction script:
  `fixtures/native-anthropic-federation/reference.json` and
  `scripts/capture-anthropic-federation-reference.mjs`.

The SDK is reference input only; native packages do not include its JavaScript runtime.
