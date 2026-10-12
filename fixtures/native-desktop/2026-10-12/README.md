# Actual native Arch desktop walkthrough

See `llm-docs/pipkin-rust-engine-evidence/packaged-desktop.md` for outcomes and limits.
`report.json` records the observed GUI steps and measured durable-state comparisons;
`arch-package.json` is the actual package verifier output. This is not an automated GUI test
or evidence of live cloud-provider behavior.

`provider.py ROOT` is the exact local scripted provider used. Before launching, create
`ROOT/profile/engine-config/`. It binds loopback on a random port, writes a synthetic provider
configuration there, records the port in `ROOT/provider-port`, and logs synthetic requests in
`ROOT/provider-events.jsonl`. Configure default provider `visual-test`, model `coding`, with
compaction disabled in the isolated engine settings. When using a different isolated app
profile, copy the generated models configuration to that profile before launch.

Use an extracted native Arch package with `--engine native --data-dir PROFILE --project PROJECT`,
an isolated HOME, and Node/npm/npx/Pi traps. Initialize PROJECT as a Git repository. Pin only
the window you launched with `scripts/guard.sh`; all synthetic input must pass that guard.
Follow the evidence document's numbered steps. The provider emits real `write` tool calls for
prompts containing `Create greeting.rs`, slow streams for `Slow stream`, and an explicitly
scripted final reply after tool results. Stop the provider after testing.

The baseline and two corrected packages, raw synthetic provider log and window captures are
retained in the local `/tmp/pipkin-packaged-desktop-815qghxg` workspace. The final archive is
in `/tmp/pipkin-desktop-final-arch/packages/`. These temporary paths are not durable artifacts.
Images intentionally are not checked in: compositor opacity reveals underlying user content
even when cropping only the test window. Hashes in the report identify the observed captures.
Use fresh isolated profiles and capture new evidence when repeating this check.
