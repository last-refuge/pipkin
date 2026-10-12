# Native-only history provenance

2026-10-12, following `ee5b5d7`. Protocol **12**, engine schema **9** unchanged.

The existing importer saves `entry_floor` above the final imported entry. Native entries
allocate from that floor even after history/context changes. Native snapshots and history
pages now expose this immutable boundary; child pages use the child's own boundary. New
native sessions and imports with no entries start at 1. No import is repeated and neither
Pi source data nor model messages are rewritten.

The desktop maps each entry to the same item IDs/content as before and marks items at or
above that boundary as native-only. A tool result arriving after import marks its merged
call/result row native-only even if the call was imported; subagent paging preserves this
mark when joining a result to an older call. Activity folding does not span provenance
boundaries. Native transcript rows display “Native-only history · not in the Pi copy”; child
previews display “Native-only history”. The normal model-context and search text remain
unchanged. Source/imported rows get no such mark.

New offline copies persist the presentation property; older copies lacking it remain
unlabelled until refreshed. This is backward compatible for the desktop reading cache, not
a claim that an old cache establishes provenance. The strict native wire version increases
because snapshots/pages gained a required field. Source Pi compatibility is unchanged.

Measured coverage:

- `native_history_labels_preserve_ids_content_and_paged_tool_provenance`: mapped content/IDs,
  imported/native boundaries, split tool call/result pages and native-only sessions.
- `native_only_history_labels_survive_offline_copies_without_changing_text`: saved provenance,
  unchanged searchable content, legacy-cache decoding.
- `native_history_provenance_survives_subagent_page_stitching_and_previews`: page joining and
  subsequent live refresh retain one labelled tool and its bounded preview.
- Expanded `reference_fixtures_import_open_and_continue_equivalently`: frozen reference
  histories remain before the boundary; native continuation leaves the boundary unchanged.
  History page boundaries agree with the snapshot; existing source/context comparisons remain.
- Expanded `pi_conversations_are_imported_on_confirmation_with_their_desktop_data`: real
  engine/adapter/core import, original messages unlabelled, native continuation labelled,
  engine restart and reopened history preserve the exact labelled item IDs.

Initial all-target checks caught the theme color path and fixture constructors/patterns that
needed the new fields. They were corrected before test execution. The first focused mapper
run passed; the full final suite includes all additional provenance and restart coverage.
Logs: `/tmp/pipkin-history-labels-check{,2,3}.log`,
`/tmp/pipkin-history-labels-focused.log`, `/tmp/pipkin-history-labels-workspace.log`,
`/tmp/pipkin-history-labels-clippy.log`, `/tmp/pipkin-history-labels-fmt.log`.
Final review removed an unnecessary provenance constraint from saved-copy recovery and made
the label follow the app text scale. Checks repeated on that revision are recorded in
`/tmp/pipkin-history-labels-workspace2.log`, `/tmp/pipkin-history-labels-clippy2.log`
and `/tmp/pipkin-history-labels-fmt2.log`.
Final workspace result: **1634 passed, 0 failed, 53 ignored**. Clippy/all-targets
and formatting passed.

No visual GPUI observation, installed rollback, complete archive/restore exercise, live
account turn or platform qualification is claimed. Pi cannot display native-only entries
that do not exist in its source; its existing read-only/explicit-release notice stays in place.
Support instructions now distinguish complete stopped-engine archives from bounded desktop
caches and explain that rollback does not undo project edits. Full release qualification,
sustained workloads and native-default/Pi removal remain open.
