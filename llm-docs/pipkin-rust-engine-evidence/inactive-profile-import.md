# Inactive Pi profile migration

Implemented after `e3786ac` on 2026-10-11 under the owner's explicit removal of Pi
extension compatibility from release scope. This supersedes the profile-refusal
policy in `import-protection-durable-retries.md`; durable retry behavior is unchanged.

## Contract

Historical package selections no longer prevent conversation migration. The importer
reads all matching server/session profile files in the selected coordination directory
and retains their exact bytes as base64, SHA-256, filename, source directory, and a
parsed JSON view when valid in the native `import_map` manifest. The manifest marks
activation disabled. Unknown versions, malformed JSON, non-UTF-8 bytes, moved session
paths, mismatched filename hashes, and selections belonging to other servers are
preserved as opaque data. No package resolution or execution occurs.

This is data preservation, not extension emulation. Root imports pin an empty native
profile in the same transaction as history and the manifest. An unrelated native
server default cannot activate extensions for the imported conversation. Pi source
files stay untouched. The existing SQLite backup retains source database state,
including documents and raw entries; normal context-equivalence checks still apply.

The importer still refuses unreadable/non-regular profile files, files exceeding the
existing 1 MiB per-file inspection limit, or profiles that change between snapshot
and commit. Discovery and execution independently read the current profiles. Adding
a selection after discovery is allowed when execution captures it successfully;
changing the captured snapshot before commit is refused. Import remains quiescent,
not a distributed transaction against a concurrently running Pi. Close Pi first.
Idempotent re-import returns the original manifest, not a later profile snapshot.
No schema or wire protocol change is needed.

## Evidence

Native importer regressions cover sticky and pinned selections, multiple servers,
explicit empty selections, copied/moved and symlink session paths, mismatched names,
unknown/malformed/non-UTF-8 content, unreadable/oversized refusal, snapshot change
refusal, and exact source-byte/hash retention in the stored manifest. The rich-history
context-equivalence regression now imports with a nonempty historical selection.
Both selected and unselected imports are tested against an unrelated native default.

The desktop core/NativeBackend/storage test
`a_plugin_selection_added_after_the_import_offer_is_preserved_without_activation`
adds a profile after the offer, confirms import, and checks successful ownership
transfer, the engine import mapping, unchanged source profile, and the empty native
profile. This is an automated desktop backend workflow, not an observed GUI run.

Targeted importer tests: 29 passed. Targeted desktop workflow: 1 passed.
Required workspace checks: **1430 passed, 0 failed, 53 ignored**; Clippy and formatting
clean. Logs: `/tmp/pipkin-import-scope-{workspace,clippy,fmt}.log`.

No live provider or installed GUI migration was tested in this batch. Release
qualification remains open; native remains opt-in. Ledger dispositions/counts do
not change: IMP-PLUGIN-STATE was already NC/measured and now records the current
owner-approved migration behavior rather than the superseded refusal policy.
