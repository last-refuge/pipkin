# Plugin-profile import protection and durable retries

Implemented 2026-10-11 on top of **`92629ec`** in the `native-engine` working tree.
Reference source was read locally and verified at
**`d2a311097cbcf669e699479587332ae3988a49d0`**:
`packages/coding-agent/src/experimental/plugins/package.ts`, `session-catalog.ts` and
`packages/durable/src/harness/generation.ts`. This is source-derived implementation plus
native regression/crash evidence, not an executed native-versus-Pi differential.

## Import protection

`crates/pipkin-engine/src/import/plugins.rs` reads Pi's coordination directory without
changing profiles or running package code. The import calls accept `serverDir`; otherwise
runtime resolution uses the engine's `PI_SERVER_DIR`, then `HOME/.pi/server`. An unknown
location is an error. An absent directory is treated as having no selections; unreadable
paths and dangling profile symlinks fail closed. Custom Pi coordination roots must be supplied
explicitly or through the environment; there is no scan of arbitrary directories on the machine.

The guard checks every server identity in that directory. Pi v1 server defaults are overridden
by the corresponding pinned session profile, including explicit empty session selections.
An empty selection for one server cannot erase another server's nonempty selection. Unsupported
versions, invalid JSON/shape, duplicate or relative package paths, unreadable/non-regular files,
session-path/hash mismatches and oversized profiles are refused. Nonempty selections remain
on Pi. Equivalent session paths through symlinks are recognized; a profile referring to the
same session id at an old absolute path blocks a copied history when its selection is nonempty.
An empty stale-path profile cannot override the current default.

Discovery reports the refusal. Execution checks again independently of discovery, captures
profile bytes before copying the database, and checks that snapshot again immediately before
writing the native records. It also rechecks the source session's live lock. Accepted profile
JSON and SHA-256 hashes are retained in the import manifest; source profiles remain untouched.
A refusal creates neither an imported native session nor a desktop ownership record. This is
protection for a quiescent import, not a distributed transaction with a concurrently running Pi;
existing live-session refusal and the instruction to close Pi still apply.

**Protocol 3** makes the import protection contract explicit and refuses older workers rather
than allowing them to ignore `serverDir`. TypeScript facet execution, native sticky selections,
reload and remote services remain separate blocked parity rows.

Measured tests:

- `imp::plugin_profiles_block_sticky_and_pinned_selections_without_touching_the_source`
- `imp::an_explicit_empty_session_selection_overrides_only_its_own_server`
- `imp::malformed_unreadable_and_mismatched_plugin_profiles_fail_closed`
- `imp::plugin_selection_is_rechecked_after_discovery_and_during_the_copy` — checks a changed
  discovery and the snapshot validator used before commit; it does not inject a concurrent
  filesystem write in the middle of SQLite's backup operation.
- `imp::another_sessions_pinned_plugins_do_not_block_an_unselected_session`
- `imp::a_copied_session_cannot_drop_a_profile_with_its_original_absolute_path`
- `imp::a_symlinked_session_keeps_its_plugin_selection`
- `rti::plugin_profile_directory_is_explicit_or_comes_from_the_engine_environment`
- `ne2e::a_plugin_selection_added_after_the_import_offer_never_transfers_ownership` — real
  desktop core/NativeBackend/storage flow; profile added between offer and confirmation;
  blocked result, no conversation activation or ownership transfer, profile bytes unchanged.
- Protocol `import_calls_round_trip_with_their_wire_names` covers explicit `serverDir` and
  defaulted discovery fields. Existing import/credential/ownership tests continue to pass.

The source's explicit empty **server** selection removes the profile file; an empty server
profile file is invalid. An explicit empty **session** profile is valid and is preserved.
The guard conservatively refuses malformed profiles in the inspected directory even when
an association with the requested session cannot be established.

## Durable retries and migration

**Engine schema 2** adds `generation_state`, keyed by run: consecutive failure count,
handled error-entry id, absolute retry deadline and overflow-compaction marker. Errors are
committed before classification. Classification is keyed by the saved entry id, so a crash
before the retry checkpoint safely reclassifies that error, and a crash after it never counts
it twice. The deadline derives from the saved response timestamp, so recovery does not start
an extra full delay. An expired deadline proceeds immediately. A terminal decision stays
terminal, and an interrupted retry request retains its preceding failure count.

Successful generations reset the retry state in the same transaction as the final response.
Waiting holds no generation permit. Stop cancels a restored wait, settles aborted and retires
its instruction; reopening cannot revive it. A failure saving a retry checkpoint fails the
session closed until recovery. The retry state never contains authentication material.

The v1 → v2 migration uses the existing backup-before-migrate mechanism. Its regression now
exercises a real older schema containing a session, admitted request, open run and entry;
all survive, and the backup retains schema 1 and the original request. Unix backups are private
(mode 0600). Newer-schema refusal remains in place. Schema 1 never stored prior retry deadlines;
upgrading preserves its data but cannot recover an in-memory deadline that was never saved.

Measured tests:

- `rtf::retry_deadline_and_attempt_budget_survive_a_restart`
- `rtf::an_expired_retry_deadline_does_not_restart_the_delay`
- `rtf::stop_during_a_restored_retry_wait_retires_the_run_without_another_request`
- `rtf::a_successful_tool_generation_resets_the_retry_budget`
- `rtf::a_failed_retry_checkpoint_fails_closed_and_reclassifies_the_saved_error_once`
- `cp::sigkill_around_retry_classification_preserves_the_deadline_and_attempt_budget` — actual
  worker SIGKILL after saving a response, after saving a retry checkpoint, and after saving a
  terminal decision; exact remaining attempt budget and no request before the saved deadline;
  settled errors do not revive on another restart.
- `st::a_v1_database_is_backed_up_before_adding_durable_retry_state`

The former test that asserted an immediate retry and reset budget after restart was replaced
by the intended-behavior assertions above. Existing exponential-backoff, disabled-retry,
Stop, compaction, storage-fault, tool/subagent recovery and actual worker crash tests still pass.

## Verification and ledger

Final commands, on the changed source:

| Command | Result |
| --- | --- |
| `cargo test --workspace` | **1319 passed, 0 failed, 53 ignored** |
| `cargo clippy --workspace --all-targets` | Passed, no warnings |
| `cargo fmt --all --check` | Passed |
| `git diff --check` | Passed |

Local run logs: `/tmp/pipkin-import-retry-final-tests.log`,
`/tmp/pipkin-import-retry-clippy.log`; the focused process-crash run is
`/tmp/pipkin-retry-crash.log`. Temporary logs are not durable repository artifacts.

Earlier attempts are not erased from the evidence:

- A sandboxed engine test attempt could not bind the local provider-fixture sockets; rerunning
  with the required host socket access passed.
- Two protocol test expectations needed updating: omission of an unspecified `serverDir`,
  and the explicit protocol version assertion changing from 2 to 3. Both are corrected.
- Clippy found one needless borrow in the new process test; corrected.
- One full workspace attempt timed out in the existing
  `a_parent_delegates_and_the_inspector_pages_through_its_subagents` test while waiting for
  the inspector to reflect three children/two finished. The isolated rerun and final full
  workspace run passed. No subagent behavior was changed to mask this failure; its cause
  remains unconfirmed and the timeout should be monitored during event-delivery qualification.

Ledger changes: **IMP-PLUGIN-STATE** is now measured native protection/refusal (NC),
**REC-RETRY** and **GEN-07** cite intended durable behavior, **DUR-MIGRATE-BACKUP** has measured
v1 → v2 evidence, and related profile/schema/protocol notes are current. Totals:
**503 rows, 417 measured, 7 observed, 1 failed, 10 unverified, 68 not started; 0 qualified**.
No performance remeasurement, real power-loss, macOS/Windows migration, live-provider rerun or
full reference differential is claimed. The existing memory-soak failure and durable
outbox/backpressure work remain open.
