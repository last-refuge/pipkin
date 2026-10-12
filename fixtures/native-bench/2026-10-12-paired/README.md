# Matched installed-engine performance

Actual local production archives, same app binary, frozen Pi runtime versus native schema 10.
`paired-engines.json`, `machine.json`, `artifacts.json` and `run.log` describe the final run.
`preliminary-*` retains the earlier passing full run that could overlap other qualification jobs.

With trusted retained local archives and the repository toolchain:

```sh
scripts/bench-paired-engines.sh PI.tar.zst NATIVE.tar.zst OUTPUT_DIRECTORY
```

Default: 20 alternating pairs, six coding turns per engine profile. Optional
`PERF_PAIRED_RUNS`/`PERF_PAIRED_TURNS` change workload size. Durable scratch defaults to
`target/pb`; `PIPKIN_BENCH_SCRATCH` selects another disk location (tmpfs/ramfs refused).
The script clears provider/account/proxy environment and uses disposable profiles.

Final assertions: 240 real bash effects, 480 provider requests, exact 72 KiB replies, original
provider tool IDs, no owned engine processes after shutdown, all five performance gates pass.
The initial native implementation missed a tool effect on repeated IDs; the recorded final
packages include the identity fix and schema migration.

See `llm-docs/pipkin-rust-engine-evidence/paired-performance.md` for methodology and boundaries.
This is warm, headless engine measurement; it excludes desktop rendering, live providers and
transient tool subprocess memory. It does not qualify the entire product or hosted CI.
