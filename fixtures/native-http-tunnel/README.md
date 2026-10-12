# CONNECT transport reference

Captured by `scripts/capture-http-tunnel-reference.mjs` with Pi's pinned undici and
`EnvHttpProxyAgent({proxyTunnel:true, allowH2:false})`. Eight real local combinations cover
HTTP/HTTPS origins through HTTP/HTTPS proxies, each with default and explicit origin ports.
The capture retains CONNECT authority/authentication and the method/path/body/authentication
seen by the origin. Credentials and certificates are synthetic public fixtures.

TLS fixture material comes from `../native-tls/`; servers/proxies listen only on loopback.
The proxy forwards only to the fixture origin, never to the authority supplied in CONNECT.
Native comparisons live in `provider/http.rs`. This tests the pinned dependency configuration,
not the full Pi application or all proxy-selection and retry paths.
