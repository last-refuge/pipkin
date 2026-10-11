# Phase 0 reference audit (RE-01)

Read-only audit of the reference engine staged exactly as `scripts/build-engine.sh` stages a clean
release: `git archive` of `d2a311097cbcf669e699479587332ae3988a49d0` (fetched from the pinned
source repository named in `scripts/setup.py`), `packaging/pi-onboarding.patch` +
`packaging/pi-onboarding/*.ts`, `packaging/pi-subagents.patch` + `packaging/pi-subagents/*.ts`, and
the pinned model-data archive. `<pi-staged>` in these files is that staged tree.

| File | Covers |
| --- | --- |
| `engine-tools-durable-usage.md` | Live tool set, tool machinery, durable schema/pragmas, submissions, Stop/retirement, recovery, compaction, usage, attachments, retained output |
| `prompt-settings-layout.md` | System prompt sections and exact text, AGENTS.md/skills/templates discovery, settings, env vars, data layout |
| `service-api.md` | Process topology, wire transport, every service method and stale guard |
| `extensions-subagents.md` | Facet plugins vs classic extensions vs durable extensions, subagent overlay contract |
| `providers-auth.md` | Wire adapters, provider ids, normalized types, OAuth flows, models.json, retry/transport |

The Claude subscription OAuth flow and its request quirks are recorded **by location only**
(reserved for the final Codex integration). Audit reports are model-produced from source reading;
behavior claims are verified by the native differential tests listed in the parity ledger, not by
these notes alone.
