# Native orchestration: live qualification evidence

## Latest result: fresh/resume passed after SIGINT fix

On 2026-10-18 the user authorized two new Sonnet prompts. Both passed with installed CLI2.1.301:
fresh native Write (`native-first`) and same-UUID resume (`native-resumed`). Both roots Completed,
both bridges closed and both native Write receipts completed. Actual final file verified.
Both process receipts show exit0/reaped/drained/projected; zero API requests. The new
`native-orchestration-sigint-pair-budget.json` is allowance2/spent2. Earlier ledgers are unchanged.
Evidence: `dist/cli-sigint-live/live-summary.json`, `live-direct.log`, and `live/probe-veABjp/`.
No further prompts remain. This narrow qualification does not enable public orchestration.
Final offline gate also passed: 2,555 tests, 13 ignored, zero failures; strict all-target
Clippy, fmt/diff, source equality and unchanged prior ledgers. Source SHA256:
`04d392360004d816534d29a25ba151300d6f041f4d76d2ab8e83ca096417cb15`.

The initial Cargo-launched test refused its inherited loader environment before reservation;
`live.log` retains it. The exact compiled test launched directly passed the unchanged preflight.
This metadata-only failure consumed no prompt. The earlier three failed live roots below remain
Unknown and must not be replayed or resumed.

## Earlier three-prompt qualification history

Status: **failed live; all three authorized Sonnet inference prompts consumed; no resume attempted**.
The user authorized two prompts and then one additional prompt on 2026-10-18. Each of the
three isolated fresh turns completed the actual native Write but ended with an Unknown root
after tool-channel loss. This does not qualify fresh/resume orchestration.

| Attempt | Owned project under `dist/cli-orchestration-resume/live/` | Ledger | Spent |
| --- | --- | --- | --- |
| 1 | `probe-kdsl9p` | `native-orchestration-budget.json` | 1 |
| 2 | `probe-GvrDk1` | `native-orchestration-close-budget.json` | 1 |
| 3 | `probe-LyyBAp` | `native-orchestration-signal-budget.json` | 1 |

The unused slots in the first two ledgers were superseded, not additive. Do not reuse them.
The separate `native-orchestration-resume-budget.json` remains unauthorized and unspent.
No fourth prompt is authorized. Preserve all three Unknown roots; do not resume or replay
any of them. Historical preparation and approval transitions follow below.

## Historical preparation and attempt log

The ignored test `runtime::cli_agent_root::tests::live_probe::installed_cli_native_write_then_resume`
uses the installed, unmodified `/home/jason/.local/share/mise/installs/claude/2.1.301/claude`,
Sonnet, a fresh isolated project/database, and the workspace's debug engine helper. It exposes
only a native Write tool restricted in code to `native-probe.txt` inside that project. No
subagent, job, network, shell or API inference tool is offered.

After explicit renewed authorization, the probe makes at most two inference launches:

1. Fresh CLI session: write `native-first` through the authenticated native MCP tool.
2. Resume the same CLI UUID: replace it with `native-resumed` through that same native tool.

Both turns must have confirmed process/helper closure, durable native receipts and actual file
effects. EOF, model text and SDK observations cannot satisfy the checks alone. The test asserts
zero requests to the scripted API provider and retains the isolated DB, per-turn outcome and
success report under `dist/cli-orchestration-resume/live/probe-*`. Once any inference attempt is reserved, the probe must not be restarted: inspect its retained
outcome; any further allowance needs explicit approval. A metadata-only failure with `spent:0`
may be repaired and continued under the same two-prompt approval.

`native-orchestration-budget.json` starts with `authorized:false`, `allowance:2`, `spent:0`.
An exclusive file lock and synced reservation consume each attempt before submission, even
if launch later fails. Exhausted or corrupt ledgers fail closed. The offline guard test uses
its own temporary ledger; it never authorizes or invokes the installed CLI.

Preparation (offline, no inference):

```sh
cargo build --offline -p pipkin-engine --bin pipkin-engine
cargo test --offline -p pipkin-engine --lib live_budget_requires_approval_reserves_durably_and_never_resets
```

Only after explicit approval and recording it in the new ledger:

```sh
cargo test --offline -p pipkin-engine --lib \
  runtime::cli_agent_root::tests::live_probe::installed_cli_native_write_then_resume \
  -- --exact --ignored
```

This qualifies only the actual native MCP/helper/CLI fresh-and-resume contract for the named
version/model/tool. It does not qualify child/job workflows, goals, context, every alias,
or the desktop. Public orchestration stays disabled; no release/install/app restart occurs.

Initial metadata check failed before admission or inference: CLI2.1.301 omits the documented
`--system-prompt-file` from ordinary help. Both prompt reservations remain unused. A separate
metadata-only `auth status` invocation accepted that option (exit0, valid status shape), while
an intentionally nonexistent option exited1 with unknown-option rejection. No status payload
or account details were retained. Evidence: `dist/cli-orchestration-resume/live-probe.log` and
`system-file-metadata-check.json`. The production preflight will check the documented file
option in the existing metadata invocation instead of assuming every supported flag is listed
in ordinary help. Source: <https://code.claude.com/docs/en/cli-reference#system-prompt-flags>.

Live attempt after the preflight repair: **failed, no replay**. The first Sonnet turn called
`mcp__pipkin__write`; the owned project contains `native-first` and SQLite retains its completed
native tool receipt. Root settlement is `unknown` after tool-channel closure interrupted the
process. No second prompt or same-UUID resume was attempted. The ledger records `spent:1`.
Retained evidence: `dist/cli-orchestration-resume/live-probe-2.log`,
`live/probe-kdsl9p/turn-1.json`, and that directory's project/database.

The offline actual-helper test reproduced an unsupported shutdown order: MCP stdin EOF
before the model terminal. The repaired helper sends an authenticated input-closed control;
the host requires quiescence and settled receipts, closes the tool channel, then continues
waiting for the original owner's terminal. Process exit and projection checks still apply.
A pending authenticated read stays alive across stdin EOF; canceling that read would revoke
the link. Tests do not qualify the fixed implementation against the installed CLI.

The unused second reservation cannot validate resume of this uncertain session. A new fresh
and resume qualification pair requires a new explicit bounded approval. The failed session
must remain inspectable and must never be automatically retried or resumed.

Follow-up authorization: user answered “Authorize one extra prompt” to a maximum of three
Sonnet prompts across this qualification. Round one spent one and is permanently stopped;
its unused slot is superseded by the new pair. The active ledger is
`native-orchestration-close-budget.json`, authorized for two launches with zero spent initially.
The harness now references only that ledger and will stop on the first failed turn. This is
two remaining launches, not two plus the old unused reservation. No original ledger is reset.

Pre-live gate note: `close-workspace.log` passed engine1593 before an outdated helper
integration fixture timed out waiting for unacknowledged EOF exit. That expectation is now
updated to assert the explicit close handshake. `close2-*` is the replacement gate; no
inference was sent by the failed gate's runner.

The second live attempt also wrote its file but ended Unknown with driver Channel failure.
It spent one of its two reservations and did not resume. Retained project: `probe-GvrDk1`.
Two of the user's three authorized prompts have now been used. Offline actual-helper tests
now cover graceful SIGTERM as well as stdin EOF, preserving the requirement for an owned model
terminal. This is a candidate compatibility fix, not yet installed qualification.

The final already-authorized prompt is reserved in `native-orchestration-signal-budget.json`
(allowance1, initially authorized=true, spent0) for one new isolated fresh native Write. It
supersedes the previous unused slot; total allowance remains three. Both uncertain sessions
are untouched. If fresh succeeds, the harness retains the live owner for up to five minutes
and writes `fresh-ready.json`; it cannot send a resume prompt unless the separately prepared
`native-orchestration-resume-budget.json` receives NEW explicit user authorization. That ledger
starts false with allowance1 and spent0. Timeout sends no prompt and does not imply approval.

Final authorized live attempt: **failed** (`live-probe-4.log`, exit101). The SIGTERM-compatible
helper again delivered the real Write and completed its native receipt, but the driver then
reported `Channel`. The root supervisor cancelled the process (exit143), reaped it and drained
both pipes; no decoder/parse/record diagnostic or cleanup timeout was reported. Cancellation
was a consequence of the channel failure, not evidence that the CLI naturally exited143.
The owned project is `probe-LyyBAp`; `turn-1.json` and `failure-summary.json` retain the result.
No fresh-ready marker was produced and no resume prompt was sent.

The actual cause of the installed channel loss remains unidentified. EOF and SIGTERM shutdown
handling are covered offline, but neither repair has qualified the installed lifecycle. Before
requesting further inference, improve reason-specific channel/helper diagnostics or reproduce
the failure without inference. Do not weaken terminal/process/receipt requirements to pass.
Public orchestration remains disabled. All three current prompt allowances are spent.

Final offline verification after the independent exporter health race repair: `verified-*`
passed 2,553 tests (13 ignored, zero failures), strict all-target Clippy, fmt/diff and source
manifest equality. Engine1594/app337/UI217; hash
`33a4ee1fc9123e5a54414b760dbb8308689c548a0b7d936c93a8331d269b36a3`.
This offline success does not supersede the three failed live outcomes above. Earlier exhausted
probe-budget files retain their original SHA256 values. No release/install/app restart or
commit/push occurred.

Subsequent offline diagnosis (2026-10-18): the installed2.1.301 executable's connected Unix MCP
cleanup sends SIGINT before SIGTERM; the dedicated helper lacked a SIGINT handler. The new
actual-helper test failed with signal2 after a completed native Write, then passed after both
signals were routed through the existing authenticated close handshake. All six helper lifecycle
tests pass, including owner Stop and absent model terminal. Evidence lives separately under
`dist/cli-bridge-sigint/` (binary identity/behavior metadata, before-fix and after-fix logs).
No installed CLI process was launched and no further prompt was consumed. This repairs the
reproduced shutdown cause; it does not rewrite any failed live result or qualify live resume.

The SIGINT fix's full offline gate passed: 2,555 tests, 13 ignored, zero failures; strict
all-target Clippy, fmt/diff and source equality. Source SHA256 `41ccf583ea2b1e5cc9bd712e904cf34c062d0979df511608fc93dad2d29ee8cc`.
All prompt-ledger SHA256 values match before/after; zero new prompts. See
`dist/cli-bridge-sigint/summary.json` for the verification record.
