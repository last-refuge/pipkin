# Synthetic Google certificate federation fixtures

These certificates and private keys are public test data, generated locally with OpenSSL
3.6.4 for Pipkin's tests. They are not account credentials and must never be used outside tests.
The ephemeral issuing CA private key was discarded. The CA, server, original client and
rotated client certificates are valid for 100 years from generation on 2026-10-12; the server
has localhost/127.0.0.1 names and serverAuth, while both client leaves have clientAuth.

`reference.json` records Google auth library 10.6.2 from the pinned Pi reference. The capture
executes real local mutually authenticated TLS exchanges, with Node trusting only the added
synthetic CA alongside its ordinary roots. No verification is disabled. Source hashes,
reference revision and package lock integrity are retained. Reproduce from the repository
root with NODE_EXTRA_CA_CERTS set to the absolute path of this directory's ca.pem and run
`scripts/capture-google-certificate-reference.mjs PI_CHECKOUT SDK_ROOT OUTPUT_JSON`.

Native tests trust this CA through a test-only NetSettings field. There is no production
insecure TLS flag or automatically trusted fixture CA. The certificate chain is a subject
credential; it does not extend the client's trusted server-root store.
