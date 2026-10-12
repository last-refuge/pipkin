# Actual installed Pi rollback and matched package sizes

2026-10-12, following `d112c77`, native protocol 12/schema 9 unchanged. These are
headless Linux x86_64 measurements. Native remains opt-in; visual acceptance is separate.

## Artifact identity

The Pi package was rebuilt from a clean temporary checkout at the frozen upstream pin
`d2a311097cbcf669e699479587332ae3988a49d0`, with the existing auth/subagent overlays and
model data recorded in the ledger. Production dependency pruning succeeded; the archive
contains private Node 22.23.3. Existing reference checkouts were not modified. No extension
compatibility was added to native.

The native and Pi archives contain the identical unstripped application binary, SHA-256
`ba43e882a836dc6ad9008d5329a133c7ffa18d078876076a83ae0d188315dfff`, built from `d112c77`
plus the dirty working tree. Exact archive hashes, complete build inputs, overlay hashes,
private runtime identity and inventory counts are retained in
`fixtures/native-installed/2026-10-12-package-comparison.json`. This identifies actual tested
artifacts, not a claim that a clean checkout alone reconstructs the dirty build.

## Installed cross-engine workflow

The explicitly executed ignored test
`adapters::pi::e2e::installed_pi_native_rollback_preserves_history_and_requires_ownership_release`
passed; the final-run duration is recorded in the fixture report. It uses actual installed Pi/native engine processes, desktop SQLite,
both adapters and the real core/controller effects with a scripted local HTTP provider:

1. Install Pi into a scratch prefix containing spaces, start its bundled engine, create a
   conversation, execute a real write tool, finish a reply, and flush an unsent desktop draft.
2. Stop Pi, install native, explicitly confirm credential and conversation import, retain
   the desktop conversation identity/draft, and execute a native write and reply.
3. Close the worker and desktop storage. Archive the complete desktop database and native
   data/config directories with GNU tar. Retain originals; restore the same paths. Assert
   every restored file's bytes and permissions, and original Pi session database bytes.
4. Roll back into the actual installed Pi package. Restored ownership makes the conversation
   read-only; the old draft survives. Submission through the core and directly through the
   Pi adapter is refused without additional provider calls.
5. Explicitly request and confirm release back to Pi, execute a third real write and reply,
   and compare the provider's user context: Pi sees its baseline and resumed prompts; native
   saw the baseline and native-only prompt. There are exactly six provider requests total.
6. Reopen the retained native worker/profile. Its branch remains readable with the native-only
   label and does not contain Pi's later prompt. All three project files remain intact;
   this is separate retained history, not bidirectional synchronization. Reactivating native
   restores its launcher selector.

The harness now restores ownership and configures the Pi adapter's ownership callback exactly
as the production controller does. Native workers run with a cleared environment and scratch
HOME; no live account or user profile is used. Artifact and test input hashes are in
`fixtures/native-installed/2026-10-12-cross-engine.json`. Reproduction is documented alongside it.

Initial test iterations found harness errors: a Rust JSON macro needed parentheses around an
indexed array, the draft needed the real `FlushDraft` command, and provider-body assertions
needed the existing helper for structured message content. These were corrected without
relaxing assertions. No engine defect is inferred from these harness failures.

## Matched package measurement

`scripts/compare-native-package.py` verifies archive/app hashes and identical application
binaries, extracts trusted local build archives into one scratch filesystem, measures GNU
`du -sb` of each `usr/`, and compares original compressed archive lengths. It also verifies
native contains no Pi/Node/npm/JavaScript runtime payload and records inventory counts.

| Measurement | Pi | Native | Reduction |
| --- | ---: | ---: | ---: |
| Installed apparent bytes | 661,543,156 | 369,345,042 | 44.17% |
| Archive bytes | 157,994,870 | 77,902,093 | 50.69% |
| npm inventory packages | 155 | 0 | 100% |

Both provisional targets (40% installed, 25% archive) pass. This supersedes the earlier
indicative cross-revision comparison against historical package sizes. Both current packages
still contain the transitional application with both adapters compiled in; the measurement
isolates legacy runtime payload removal and does not predict final post-retirement binary size.

## Verification and limits

Required workspace checks: **1,638 passed, 0 failed, 55 ignored**, clean Clippy/all-targets
and formatting. The new ignored cross-engine test was run separately and passed. Exact matched
Pi package verification and install/upgrade/rollback/uninstall pass; exact native package
verification, installed migration/coding/restart/restore and uninstall pass. YAML parses locally.
CI now builds the paired artifacts and runs the cross-engine workflow and package comparator;
no hosted runner execution is claimed.

A review caught a shell comment interrupting the verifier's continued environment assignment;
the comment now precedes the assignment so the full suite receives the unpacked Pi path.
The first full-verifier attempt under the restricted sandbox failed local Unix/HTTP socket
operations (EPERM), including its installed handshake; that is not a qualified host run.
The first host run passed 36 tests and failed the Changes scan fixture: it expected an
external edit to appear in a new, untouched conversation. The current shared contract is
session-scoped Changes. The fixture now first asserts unrelated-file exclusion, then uses
a real tool write to establish session attribution before testing missing-project failure,
recovery and non-Git state without further inference. This aligns it with the already-passing
native counterpart and preserves the recovery assertions. The corrected full installed Pi suite passes **37 tests, 0 failures**, followed by `install verified`. This includes the frozen Pi extension reference tests only; it adds no native extension requirement.

This closes the defined installed rollback/profile restoration and package-size measurements.
It does not establish visual desktop acceptance, live credentials, all import formats,
cross-schema downgrade, arbitrary backup relocation, other Linux distributions, licensing
clearance or remaining provider/performance comparisons. Pi remains the default, and no row
is promoted to fully qualified solely from these tests.
