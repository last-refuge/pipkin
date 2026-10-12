# Packaged native desktop qualification

2026-10-12, following `9915177`, protocol 12 / engine schema 10 / desktop schema 8.
Native remains opt-in. Reports and the local scripted provider are retained in
`fixtures/native-desktop/2026-10-12/`; identities distinguish all three tested binaries.

## Failure and fixes

The preceding native Arch package opened Welcome correctly, but Explore entered an empty
workspace with focus assigned to an unmounted composer. Ctrl+K and Ctrl+N then did nothing.
Workspace now focuses its mounted root when no conversation exists, transfers focus to the
composer after asynchronous creation, and restores root focus when the selected conversation
disappears. Overlay focus is preserved. Explicit composer/transcript focus shortcuts also
retain a mounted target. The GPUI regression covers empty and welcome-derived workspaces,
demo and real asynchronous creation, typing, and removal. It reproduced the failure before
the fix. An intermediate test setup lacked a selected project; correcting that setup made
it match the actual `--project` launch, without weakening the keyboard assertions.

The walkthrough also exposed `guard.sh` passing literal punctuation to `wtype -k`.
Ctrl+. failed with `Unknown key '.'`; Stop was first exercised through the command palette.
The guard now maps already-allowed period/comma keys to XKB names. Its safety policy is
unchanged. Added regression cases fail before the fix (33 passed / 2 failed) and pass after
(35 / 0). Real Ctrl+, opens Settings, and real Ctrl+. stops streaming on the final package.
Shared status, recovery and Settings copy now refers to Pipkin/the engine rather than Pi.
The error hint directs users to in-app diagnostics, avoiding a default-Pi CLI diagnostic in
a native session. No recovery semantics or demo marker changed.

## Executed workflow

An isolated project and profile used a loopback OpenAI-compatible scripted provider, visibly
named **Local scripted qualification**. Responses identify their scripted origin; the native
engine and file tools are real. No cloud credentials were needed or accessed.

1. Fresh Welcome → Explore → palette → new native conversation → composer typing.
2. A real write tool created `greeting.rs`; the UI showed its completed tool card and +1/-0
   Changes diff. Two provider requests completed the tool round trip.
3. A long streamed response displayed live text. A follow-up was queued, then Stop cancelled
   the request and withdrew the queued input. The follow-up never reached the provider.
   The test's initial prompt called it “must survive Stop”; that expectation was incorrect.
   The existing Pi/native contract withdraws queued inputs on Stop, as the executed tests and
   `queued-summary-writes.md` establish. No engine behavior was changed to fit the walkthrough.
4. After “Draft saved,” SIGKILL of the verified test app caused its owned worker to exit.
   Reopening the profile with the final rebuilt package restored the draft, partial response
   and diff. Provider request count stayed at three: no replay on reopen.
5. On the final package, Ctrl+. stopped a second stream. A subsequent real write-tool turn
   completed. Both its provider requests excluded the stopped instructions and withdrawn
   follow-up. Reusing the provider ID `visual-reused` produced two distinct durable tool
   calls, both done, confirming the earlier identity fix in a desktop workflow.
6. Another saved draft survived SIGKILL and relaunch of the exact same final binary. Ordered
   rows of entries, chunks, requests, runs, calls and outputs were hashed before/after and
   remained identical. Six provider requests stayed six; the file hash was unchanged.
   The four runs are settled (two completed, two aborted); the two write calls are done.
7. Guarded Ctrl+Q shut down the app and worker. The isolated fixture provider was stopped.
   No Node/npm/npx/Pi trap fired.

All synthetic input went through the exact-PID window guard on the original workspace.
No unrelated windows were focused, moved or resized; no workspace, display or compositor
settings changed. Captures were viewed from the test window at the existing 1.25 scale.
Compositor opacity ghosts underlying content into even exact-window crops, so raw images
remain local and only hashes are retained. This observation does not qualify scale changes.

## Artifact and checks

The final actual Arch archive SHA-256 is
`066712546451ba2ee9dc7398425c20a4e99e20ed3928e48a2767d81dcab02845`;
the app binary is `97362a7b177a8f3aacd92b44b2f6e320a82a70e75faee64117684eb22c171221`.
It records `9915177` plus working changes. Source hashes are in the desktop report.
The offline makepkg build used dependency traps and the previously documented `--nodeps`
exception for the user-managed Rust toolchain, without installing packages.

The actual archive verifier passes metadata/binary/recipe identity, 31 installed guides,
1,079 notice hashes, and isolated import/write/edit/bash/read/restart/archive restore with
settled-effect replay protection. Workspace tests: **1,641 passed, 0 failed, 56 ignored**;
Clippy/all-targets and formatting pass. Guard tests: **35 passed, 0 failed**.

These results add observed desktop evidence to E2E-01/03/09/11/14, PKG-INSTALL and PLAT-LINUX.
They do not qualify every workflow, all providers or all platforms. Sustained GUI use,
remaining behavioral comparisons, clean-chroot/hosted CI, provenance/license review and
native-default/Pi retirement remain open. No system pacman install, live Claude recheck,
IME/screen reader, suspend/resume, cold boot or presentation-latency claim is made.
