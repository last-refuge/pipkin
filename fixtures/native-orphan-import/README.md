# Built-in task recovery reference

Captured by `scripts/capture-orphan-import-reference.mjs` from Pi revision
`d2a311097cbcf669e699479587332ae3988a49d0`, with source hashes in each `reference.json`.
These are synthetic SQLite sessions, not user profiles. Capture requires the pinned Pi
source checkout and its dependencies; normal native tests consume only the frozen fixtures.

```sh
node scripts/capture-orphan-import-reference.mjs PI_CHECKOUT fixtures/native-orphan-import
```

For `generation`, `compaction`, and `tool`, the actual Pi Harness creates a built-in task
with a newer definition version, closes SQLite, reopens, verifies `task_too_old` blocking,
and explicitly aborts it. `blocked.sqlite` is the pre-abort database; `session.sqlite`
is the settled orphaned history. `reference.json` records task and submission outcomes,
remaining live state, full visible entries, and actual Pi-derived next-request context.

For `faulted`, an actual generation commits streamed partial text and receives a final
message containing a non-JSON function. The scheduler faults the task, retains the partial
as an aborted assistant entry, settles its submission with the original diagnostic, and
clears its live run. Only the settled database is captured for this case.

The model is Pi's local faux provider (with a malformed stream injected for the fault case).
No cloud account or network is used, and the interrupted `bash` call in the tool fixture
is never executed. No source records are rewritten to make native import pass.

Native tests verify blocked-source refusal without mutation; settled task/submission/live
state preservation; context equivalence; retained partial history; idempotent import; and
reopen followed by a new real shell effect and settled-admission replay. Task-definition
extension compatibility and automatic execution of imported in-flight work are not claimed.
