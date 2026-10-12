# Plain HTTP CONNECT parity

2026-10-12, following `bdc7a26`. Protocol 11 / schema 9 unchanged.

## Reference and implementation decision

Pinned Pi's `coding-agent/src/core/http-dispatcher.ts` configures undici with
`proxyTunnel: true`: both HTTP and HTTPS origins use CONNECT, through either HTTP or HTTPS
proxies. The native engine previously forwarded HTTP requests in absolute form. A proxy that
requires CONNECT could therefore reject an otherwise supported provider or credential endpoint.

Reqwest 0.12.28 has no public option for HTTP CONNECT; its connector-layer types also hide the
URI and connection internals. Pipkin now keeps a verified local copy with one additive option:
`ClientBuilder::http_proxy_tunnel(bool)`, false by default. The engine opts in. This retains
reqwest's actual HTTP parser, request builders, pool, TLS, deadlines, redirects and streaming;
there is no new local proxy listener or helper process in production.

The patch changes only two upstream Rust source files. It reuses hyper-util's CONNECT path,
marks HTTP tunnels as origin-form connections, and supplies port 80 explicitly (the underlying
tunnel otherwise defaults missing ports to 443). Configured proxy authentication/custom proxy
headers belong to CONNECT, not the tunneled origin request. HTTPS behavior and non-opted-in
reqwest consumers retain their original behavior. Existing NO_PROXY and configuration selection
remain in Pipkin's policy layer.

## Dependency provenance and packaging

The complete local crate was extracted from the cached registry archive after verifying its
SHA-256 against the previously committed Cargo.lock:
`eddd3ca559203180a307f12d114c268abf583f59b03cb906fd0b3ff8646c1147`.
The Cargo cache was not changed. `vendor/reqwest-provenance.json` records original and patched
file hashes; `vendor/reqwest-http-tunnel.patch` is the focused review diff. All MIT/Apache notices
are retained, and both changed files identify the local modification.

`scripts/verify-reqwest-vendor.py` verifies all 48 files and the patch. Tamper checks reject
changed original source, changed patched source, changed licenses/diff and extra files.
The native packager verifies this tree before building. The Linux qualification workflow also
runs verification and its tamper test; GitHub execution is not claimed here.

The actual native Rust license inventory was generated under
`/tmp/pipkin-http-tunnel-stage/usr/share/doc/pipkin/third-party/`. It contains both reqwest
notices, identifies the vendored source and includes the patch/provenance notes. Existing
license inventory fixture tests pass. This is provenance evidence, not blanket legal clearance
or clean release qualification. The measured build uses Rustls/Linux; other TLS feature/backend
combinations and platforms are not qualified by these checks.

## Executed comparisons and tests

`scripts/capture-http-tunnel-reference.mjs` executes **Node 26.8.1 / undici 8.10.2**, verifies
Pi revision `d2a311097cbcf669e699479587332ae3988a49d0`, unchanged lockfile/dispatcher and
installed package version, and records source hashes/integrity. It runs the pinned dependency
configuration directly, not the full Pi dispatcher/application. The installed undici tree is
not independently verified against its registry archive.

Eight local origin/proxy combinations exercise HTTP/HTTPS on both hops and default/explicit
ports. Native compares exact CONNECT authority and proxy credentials, then origin method,
path, request body and authorization. Proxy credentials never reach the origin, and origin
credentials never appear on CONNECT. TLS uses verified synthetic certificates.

Additional tests show the unmodified reqwest default still forwards; one CONNECT is reused
for two origin requests; a stalled CONNECT obeys request timeout and closes its socket;
NO_PROXY still connects directly; runtime environment/settings choose the intended route;
and remote catalog refresh goes through a real tunnel without provider credentials.

An initial compile omitted the new field in reqwest's Config declaration; it was corrected.
A later proxy run passed 21 tests and failed an old runtime fixture's expectation of one HTTP
400: its proxy refused CONNECT, producing a retryable transport error rather than an origin
HTTP response. That fixture now uses a real tunnel and two separately recorded origins, so it
continues to assert one non-retryable origin response and exact routing. The corrected focused
proxy run passed **22 tests**; the HTTP suite passed **8 tests**, including all eight reference
cases, pooling and timeout cleanup. Logs: `/tmp/pipkin-http-tunnel-check{,2}.log`,
`/tmp/pipkin-http-tunnel-tests{,2,3}.log`, `/tmp/pipkin-http-tunnel-http-tests.log`.
The native comparison also verifies bracketed IPv6 HTTP authorities retain the default port
(`[::1]:80`). Final workspace checks passed **1617 tests, 0 failed, 53 ignored**;
Clippy and formatting are clean. The first Clippy run identified an unused fixture binding
and a let-and-return expression; both were corrected before the final run. Logs:
`/tmp/pipkin-http-tunnel-workspace{,2}.log`, `/tmp/pipkin-http-tunnel-clippy{,2}.log`,
`/tmp/pipkin-http-tunnel-fmt.log`. Vendor verification and tamper checks were rerun successfully.
No installed package or GitHub workflow execution is claimed for this batch.

## Remaining work

Unusable-proxy errors outside model requests still use the reserved DNS failure path;
credential SDK retry middleware, OS/Node-specific trust configuration scope, remaining settings
and migration gaps, sustained workloads, clean release/CI/platform acceptance and default
cutover remain open. Native remains opt-in. This closes the plain-HTTP tunneling gap without
claiming full adapter or transport qualification.
