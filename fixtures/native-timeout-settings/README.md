# HTTP timeout settings reference

The 31 cases in `reference.json` execute the pinned SettingsManager and the original
`parseHttpIdleTimeoutMs` in-memory under Node 26.8.1. The capture checks revision
`d2a311097cbcf669e699479587332ae3988a49d0`, unchanged sources and SHA-256 identities.
Temporary copies rewrite only import paths. The text/BOM and retry modules are unchanged;
unused filesystem/network helpers throw if called. The HTTP dispatcher module is copied
unchanged except its undici import, which points to throwing stubs. No dispatcher is created
and no network request is made by this capture.

Both global-only and global/project-merged getter results are recorded. Cases cover absent,
zero/disabled, fractional, decimal/scientific/base-prefixed strings, ECMAScript whitespace,
negative values, null, booleans, arrays/objects and project replacement/repair. The largest
captured interval is JavaScript's maximum safe integer in milliseconds. This does not qualify
larger interval representation, actual long-duration timers or every provider SDK's timeout.

Native compares exact getter values/errors and preserves unrelated provider settings. Separate
runtime/HTTP tests cover rejection before requests, sign-in restart after live repair, retained
invalid snapshots and disabled timeouts. Native intentionally stays available for repair instead
of terminating startup when global timeout configuration is invalid.

```sh
node scripts/capture-timeout-settings-reference.mjs PI_SOURCE fixtures/native-timeout-settings/reference.json
```
