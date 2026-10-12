# Frozen Pi reference tooling

The `capture-*-reference.mjs` files, `capture-pi-views.ts`, `stage-model-data.sh`,
`stub-provider.mjs`, `test-provider-auth.sh` and archived Pi patches are provenance and
optional reference-recapture tooling. They are not invoked by current build, package, CI,
installation or runtime paths. Ordinary Rust tests consume frozen fixtures without Node.

To reproduce the historical paired-runtime benchmarks or old Pi packaging, check out
`76ca778` in a separate disposable checkout. That revision retains the old adapter,
client, Pi build scripts and benchmark drivers. Do not mix those scripts with current
Pipkin; its native protocol does not speak Pi's retired runtime protocol.
