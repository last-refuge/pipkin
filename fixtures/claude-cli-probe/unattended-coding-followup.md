# Three-prompt unattended-coding follow-up

This is a **new, explicit allowance of up to three CLI test prompts**, granted after the
historical ten-run allowance was exhausted. The old ledger is unchanged (SHA256
`bf794518e35d7f1ce3a50b3058c4191bb9401e568fc645d77e10b0f404cf1f22`).

**Used: 3 / 3. Remaining: 0. Do not run another inference without new authorization.**
No replay, native/API inference fallback, credential inspection, CLI configuration changes,
installation, release build, user-app restart or user-profile mutation was performed by the
harness. The unmodified installed CLI used its own normal login and normal CLI-owned persistence.
The user remains responsible for rebuilding/installing Pipkin.

## Evidence

Installed executable: `/home/jason/.local/share/mise/installs/claude/2.1.301/claude`.
All actual prompts used Sonnet and `cli-unattended-coding-v1`. Files were confined by the test
instructions to owned disposable project fixtures; this is not sandbox evidence.

| Job | Model requests | Result |
| --- | ---: | --- |
| 270 | 0 | Live harness failed compilation (unavailable UUID dependency); corrected to a timestamp marker. |
| 271 | 0 | Public setup rejected the test-process environment before prompt admission. Fixture retained. |
| 272 | 0 | Metadata-only diagnostic identified the environment-override rejection. Cargo injects `LD_LIBRARY_PATH` into test processes. Production safety was not weakened. |
| 273 | 1 | Directly launched the compiled test binary from the original environment, without Cargo's injected loader variable. Real public-engine Inspect/Check/Create/Prompt completed: two Read calls, one Write, exact output contents, clean exit/pipe settlement, zero native requests. |
| 274 | 0 | Core regression failed compilation on an Option assertion; corrected. |
| 275 | 0 | Offline core execution-profile28 and fake desktop same-alias routing regression passed; live desktop harness compiled. |
| 276 | 2 | Real desktop core + adapter + desktop persistence + engine path: metadata-only legacy creation, selecting the same alias automatically opens coding, then fresh and resumed prompts complete. Four Read calls, two Write calls, exact output files, same coding identity, preserved legacy identity/draft, zero approval rows, zero native-provider requests, zero outstanding desktop journal entries. |

All three live requests completed with input delivered, projection finished, root reaped,
pipes drained, exit0, no stop/timeout, no parse/projection error and no presentation warning.
No uncertain operation was repeated. The tests are ignored by default and require explicit
executable/artifact-root parameters and durable one-shot permit files. They must not be rerun
merely by selecting a new artifact directory; the authorization above is exhausted.

Artifact root: `/home/jason/.pg.dgvElr`.

- Failed setup fixture: `.tmpSDgVmJ` (no model request).
- Public-engine success: `.tmpuR7FWp`.
- Desktop-path fresh/resume success: `.tmp6bh4RS`.
- `compile-live.jsonl`, `compile-app-live.jsonl`: compiled executable discovery.
- `app-source-diff.sha256`, `live-harnesses.sha256`: source evidence at job276.
- `prompt-1-consumed`: retired **setup-only** attempt; no prompt was sent.
- `prompt-2-consumed`, `app-fresh-consumed`, `app-resume-consumed`: the three actual requests.

Implementation regressions:

- `crates/pipkin-engine/src/runtime/external_live_tests.rs`
- `crates/pipkin-app/src/adapters/native/tests/local_cli_selection_e2e.rs`
- `crates/pipkin-core/tests/execution_profiles/connection_choice.rs`

## Scope, not a broad compatibility claim

Measured: real Read/Write execution, exact file effects, normal desktop command routing,
legacy same-alias transition, fresh/resume, receipt settlement and no approvals/native fallback.
This uses the actual core and adapter, but not synthetic input into a native GPUI window.

Not live-qualified: Bash, Edit, CLI-owned agents, other model aliases/releases, managed policy,
billing/extra usage, full descendant containment, native-window interaction or accessibility.
CLI-owned tools are **not native Pipkin plans/jobs/subagents/goals/queues**. That remaining
integration needs a separately structured authority/ownership contract; printed XML and CLI
observations remain inert and must never be used as a shortcut to native execution.

Final offline workspace gate is recorded in `docs/scorecard.md`.
