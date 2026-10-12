# Orphaned and faulted built-in task recovery

2026-10-12, following `2369416` on `native-engine`. Protocol 12 and engine schema 10
are unchanged. Pi extensions remain out of scope. Native remains opt-in.

## Executed reference and migration

`scripts/capture-orphan-import-reference.mjs` executes the actual pinned Pi Harness,
scheduler, SQLite store, abort cleanup, and context derivation at
`d2a311097cbcf669e699479587332ae3988a49d0`. It checks the reference revision and clean
relevant source trees, and records source/database hashes in each fixture. The reference
checkout is read-only. All profiles and provider responses are synthetic; no cloud requests
or user profile access occur.

`fixtures/native-orphan-import/` retains three blocked and four settled databases:

| Case | Actual Pi result | Native checks |
| --- | --- | --- |
| Newer generation definition | Reopen stays blocked (`task_too_old`); explicit abort orphans the task, leaves the submission unanswered, saves the committed partial as an aborted assistant entry, clears live state | Blocked import refuses unchanged; settled import preserves the task, submission, partial and next-request context |
| Newer compaction definition | Reopen stays blocked; explicit abort orphans the task and removes live compaction status | Same refusal, complete task/live-state preservation and unchanged context |
| Newer tool definition | Reopen stays blocked; explicit abort orphans the task, marks its slot done, clears unsettled progress; context supplies the missing-result error | Same refusal and settled-state preservation; next-request context matches, including the missing tool result |
| Faulted generation | An actual stream saves partial text, then supplies a final message containing a non-JSON function; scheduler records faulted task/unanswered submission with the diagnostic, converts the partial and clears live state | Original task/submission diagnostic retained; failed durable admission preserved; partial remains visible but is omitted from model context as in Pi |

The importer tests also compare source bytes before/after and require repeat import to be
a no-op. Runtime tests import all four settled cases, restart, verify no provider requests
or tool effects, select the native scripted model, and replay the original failed admission
where present. A new coding turn then performs a real shell append. Its first provider
request contains exactly the projected Pi-derived history, and another restart/replay leaves
the file at one append with exactly two provider requests. Existing history entries remain
unchanged. This tests real native tools with a scripted provider, not a live cloud model.

These checks follow plan §6: quiesce/reconcile source work before ownership transfer; import
itself never resumes queued or unsupported checkpoints. They do not establish automatic
continuation of arbitrary in-flight Pi task definitions or optional extension task registries.

## Native cleanup failure and fix

An additional regression reproduced a related native error in `abort_child`, used when a
parent cannot wait for its old child again. With a real child shell already running and the
database read-only, the old code only logged its failed `request_stop`. The child continued
executing. The regression failed its five-second unwind bound before the fix.

The failure now calls the existing storage-failure cancellation path for child and parent,
returns the error, and does not label the still-unsettled task aborted. Recovery and Stop
callers propagate the error before settling the parent or recording a final interrupted
result. Cleanup attempts all orphaned children even if an earlier one fails. A successful
retry persists Stop, then uses recovery to reconcile an inactive child, including its earlier
storage-failure state. Task-status write failures also propagate on this cleanup path.

The regression verifies prompt cancellation, open admission, no false aborted label, no
automatic execution when storage becomes writable, and explicit Stop retry without replay.
The shell's `started` append occurs once and its delayed `finished` append never occurs.
The existing orphaned foreground-compaction cancellation test still passes.

## Reproduction and verification

```sh
node scripts/capture-orphan-import-reference.mjs PI_CHECKOUT fixtures/native-orphan-import
cargo test -p pipkin-engine orphan -- --nocapture
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all --check
```

Focused checks: eight passed after the fix. Final required workspace checks: **1,647 passed,
0 failed, 56 ignored**; Clippy/all-targets and formatting are clean. Logs are `/tmp/pipkin-orphan-{workspace,clippy,fmt}.log` and
`/tmp/pipkin-orphan-after.log`; the failing cleanup regression is retained in
`/tmp/pipkin-orphan-stop-before.log`. Initial test compilation used an inaccessible import
normalizer; it was corrected to the existing runtime fixture projection before execution.
The first reference invocation hit sandbox subprocess restrictions; the same offline capture
ran successfully on the host.

`REC-ORPHAN` now has measured evidence for these required built-in migration and native
cleanup paths. Full release qualification is still open: remaining reference comparisons,
installed/desktop fault coverage, sustained desktop use, CI/platform and notice review, and
the native-default/Pi retirement gate are not established by these tests.
