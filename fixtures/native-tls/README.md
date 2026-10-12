# Local TLS transport fixtures

These are deliberately public, local-only test credentials. They authenticate `localhost`
in `openai_codex/tls_tests.rs`; the private key is a fixture, not a production secret.
The CA is added only to an individual test adapter's Rustls config, never to the OS or the
production trust store. Tests also verify that production roots reject it and that using
`127.0.0.1` fails hostname verification (the certificate has only `DNS:localhost`).

- `ca.der`: self-signed RSA-2048 CA certificate; its signing key was discarded.
- `localhost.der`: RSA-2048 leaf, CA-signed, SAN `DNS:localhost`, serverAuth EKU.
- `localhost-key.der`: unencrypted PKCS#8 DER key for that test leaf.
- Validity: 2026-10-12 02:14:29 UTC through 2036-10-09 02:14:29 UTC.
- Leaf SHA-256 fingerprint:
  `7ADDCF79AD53D8988642134387A9D73B1EA982BBEC1C5C46DB4BB5314A2D266E`.

Generated locally with OpenSSL `req -x509` (CA), `req -newkey` (leaf), `x509 -req`
(sign with `subjectAltName=DNS:localhost`, `basicConstraints=critical,CA:FALSE`,
`keyUsage=critical,digitalSignature,keyEncipherment`, `extendedKeyUsage=serverAuth`),
then `x509 -outform DER` / `pkcs8 -topk8 -nocrypt -outform DER`. No external certificate
service or network was used. Regenerate this complete set together before expiry.
