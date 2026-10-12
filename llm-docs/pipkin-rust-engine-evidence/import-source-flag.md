# Explicit Pi profile source in native mode

2026-10-12, following `6b148ee`. Protocol 11 / schema 9 unchanged.

`--engine native --pi-agent-dir DIR` now selects the Pi profile source for import discovery,
conversation imports, config copies and credential merges. The CLI makes relative paths
absolute as it already does in Pi mode. The native adapter sends this source explicitly on
all existing import calls, including rediscovery after import. An omitted source retains the
engine's `PI_CODING_AGENT_DIR` / `~/.pi/agent` default. Native configuration/data roots and
engine process environment are unchanged. There is no automatic startup import.

The source also works with `--engine-attach`: the attached engine reads that path using the
existing import protocol. Pi launch/server options still require Pi mode. Server coordination
profile selection is separate; this does not repurpose `--pi-dir` or load Pi extensions.
The support document explains precedence, confirmations and attachment semantics.

Two new tests exercise CLI admission/absolute paths for owned and attached host options,
and a real native adapter + core + engine + SQLite workflow with two disposable Pi profiles.
The end-to-end test selects one profile despite another in the engine environment, discovers
and confirms only the selected conversation, separately confirms config/credential imports,
checks native destination files and unchanged source bytes, repeats discovery, then continues
the imported history through a scripted provider. Startup and discovery import nothing;
credential values are synthetic and omitted from notices. This is headless workflow evidence,
not a newly observed desktop or live-provider run.

The initial test passed CLI parsing but expected another confirmation offer after all selected
sessions were already imported. The existing core correctly returns Idle with an informational
notice for an empty offer. The fixture expectation was corrected; both tests then passed in
0.33 seconds. Logs: `/tmp/pipkin-import-source-tests.log` (initial failure),
`/tmp/pipkin-import-source-tests2.log` (passed).

Required workspace checks passed **1624 tests, 0 failed, 53 ignored**, with clean Clippy and
formatting. Logs: `/tmp/pipkin-import-source-workspace.log`,
`/tmp/pipkin-import-source-clippy.log`, `/tmp/pipkin-import-source-fmt.log`. Native remains opt-in. This closes FLAG-PI-AGENT-DIR's
explicit-source wiring gap; it does not qualify the full import surface or release candidate.
