# Proxy validation reference

`reference.json` was captured by `scripts/capture-proxy-validation-reference.mjs` from
Pi revision `d2a311097cbcf669e699479587332ae3988a49d0`, executing its actual TypeScript
proxy resolver with Node 26.8.1. The script checks the commit and unchanged source files,
records their hashes, clears ambient proxy variables, and uses only synthetic configurations.
It does not execute the complete Pi application or make network requests.

Eight cases cover unsupported SOCKS/PAC, malformed URLs, NO_PROXY host/port exclusions,
scheme selection and a valid proxy. Native compares proxy choices and unsupported-protocol
messages exactly. For malformed URLs, native intentionally omits the raw URL (which might
contain credentials) and uses Rust's parse reason instead of Node's.

Regenerate from the pinned read-only checkout:

```sh
node scripts/capture-proxy-validation-reference.mjs PI_CHECKOUT fixtures/native-proxy-validation/reference.json
```
