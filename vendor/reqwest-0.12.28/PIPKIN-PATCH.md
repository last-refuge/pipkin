# Pipkin's reqwest patch

Base: the complete crates.io `reqwest` 0.12.28 archive, SHA-256
`eddd3ca559203180a307f12d114c268abf583f59b03cb906fd0b3ff8646c1147`, matching
Pipkin's original Cargo.lock checksum. MIT and Apache-2.0 notices are retained unchanged.
This is a locally maintained additive patch; it is not an upstream release.

The sole behavior option is `ClientBuilder::http_proxy_tunnel(bool)`, disabled by default.
Pipkin's engine enables it to match pinned Pi/undici's `proxyTunnel: true`:

- Use the existing hyper-util CONNECT implementation for HTTP origins through HTTP or HTTPS
  proxies. HTTPS targets keep their existing path. Supply HTTP's default destination port 80
  explicitly because hyper-util's tunnel defaults a missing port to 443.
- Mark tunneled connections as origin-form connections for HTTP parsing/pooling.
- Put configured proxy authorization and custom proxy headers on CONNECT, never on the
  origin request inside the tunnel. Direct traffic and callers without the option are unchanged.
- Preserve existing TLS verification, proxy TLS separation, deadlines, redirect handling,
  response streaming and connection pooling. No extra process or local proxy service is added.

Only `src/async_impl/client.rs` and `src/connect.rs` differ from the registry archive, plus
this provenance note. The reviewable diff is `../reqwest-http-tunnel.patch`. The sibling
`reqwest-provenance.json` records all original file hashes and explicit patched-file hashes.
Run `python3 scripts/verify-reqwest-vendor.py` from the repository root to verify the tree.

Rebase the small patch deliberately when updating reqwest: verify the new registry archive,
regenerate provenance and the diff, and rerun Pipkin's proxy/reference, TLS, provider and
workspace tests. Do not edit the system Cargo cache. The exercised release configuration uses
Rustls on Linux; other TLS backends/platforms are not qualified by those checks.
