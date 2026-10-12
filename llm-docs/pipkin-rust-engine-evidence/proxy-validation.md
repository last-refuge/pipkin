# Invalid proxy diagnostics before dispatch

2026-10-12, following `367e389`. Protocol 11 / schema 9 unchanged.

The shared native client previously sent unusable proxy configurations to
`unusable-proxy.invalid:9` to prevent direct fallback. Models had an earlier policy check,
but login, token and other shared-client requests could report a DNS failure instead of the
actual unsupported or malformed proxy setting.

The locally patched reqwest now has an optional `request_validator` callback on its HTTP/1
and HTTP/2 dispatch service. Pipkin uses the same immutable `ProxyConfig` for validation and
routing. Every request is checked before network I/O, including redirects and pool reuse;
NO_PROXY exclusions and unrelated scheme settings still permit direct connections. Invalid
configuration returns a typed builder error. Shared sending does not retry builder errors.
OAuth reports the exact proxy policy message; catalog refresh includes it instead of hiding
it behind a generic refresh failure. The sentinel DNS route is removed.

Validation is disabled by default for other reqwest consumers. The experimental HTTP/3
feature rejects clients opting into this hook because it has a separate service path;
Pipkin's measured Rustls/Linux client enables HTTP/2, not HTTP/3. This is not a claim that
unmeasured platforms, TLS backends, or experimental HTTP/3 were tested. Patch/provenance
hashes and modification notices are updated; the same two upstream source files differ.

`scripts/capture-proxy-validation-reference.mjs` executes the actual pinned Pi resolver
(TypeScript, Node 26.8.1), checking the commit/source and recording hashes. Eight captured
cases cover SOCKS/PAC, malformed URLs, host/port exclusions, schemes and a valid proxy.
Native matches routing and unsupported-protocol messages exactly. Malformed URL diagnostics
intentionally omit the raw URL, which can contain passwords, and use the Rust parse reason.
This executes the resolver, not the complete Pi application or external proxy services.

Tests verify exact errors, no direct-origin requests, no configuration retries, OAuth code
submission rejection, catalog diagnostics, redirect rejection, NO_PROXY and validation on an
open pooled connection. The focused proxy run passed **26 tests**. The initial test compile
attempt crossed a private OAuth helper boundary; the OAuth case was moved into that module,
then passed. The initial reference capture hit the sandbox's child-process restriction;
the authorized rerun completed. Logs: `/tmp/pipkin-proxy-validation-check.log`,
`/tmp/pipkin-proxy-validation-http.log`, `/tmp/pipkin-proxy-validation-tests2.log`.

Final required checks passed **1622 tests, 0 failed, 53 ignored**, with clean Clippy and
formatting. Logs: `/tmp/pipkin-proxy-validation-workspace.log`,
`/tmp/pipkin-proxy-validation-clippy.log`, `/tmp/pipkin-proxy-validation-fmt.log`.
Vendor integrity/tamper checks and license-inventory fixtures pass. No installed package or GitHub workflow execution is claimed here.

Credential SDK retries, OS/Node-specific trust scope, remaining settings/migration gaps,
sustained workloads, release/platform qualification and native-default retirement remain
open. No row is fully qualified by this batch.
