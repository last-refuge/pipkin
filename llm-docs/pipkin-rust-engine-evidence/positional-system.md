# Positional system context — 2026-10-11

Implementation on `native-engine`, on top of **7872c91**. Protocol **5**, engine schema **4**.
Native remains opt-in. The user's existing plan edit and `.claude/worktrees/` are untouched.
No live credentials, paid model calls, desktop windows or desktop configuration were used.

## Implemented contract

- Typed system messages preserve text or block content, ordered section patches, null removals,
  tool additions/removals, constrained-sampling declaration data, timestamp and opaque metadata.
  Invalid section values fail deserialization. The protocol crate itself enables ordered JSON.
- The pure planner implements the pinned `durable/harness/prompt.ts` rules: unchanged state
  emits nothing; changed values/deletions use a minimal patch; reordered sections remove then
  re-add; changed tool declarations remove/re-add, with complete tool order repair when needed.
  Tool changes ride on the final patch. Compaction/reset gets a complete fresh baseline once,
  retiring retained older system entries atomically with the new baseline.
- Runtime preparation commits the system entries before issuing the provider request. Restart
  replays them, with new project instructions producing a patch instead of rewriting the past.
  Planner replay only copies system metadata, not the entire conversation. Request resolution
  avoids redundant history copies and consumes its owned transcript when collapsing systems.
- Imported `pi.system` records, including imports made by earlier native versions, now contribute
  their positional messages. Import equivalence includes them on both sides instead of filtering
  them away. Source envelopes remain preserved. Malformed new system imports fail closed.
- Capable transports retain section updates at their positions. Other transports receive the
  replayed current system prompt/tools. Chat Completions supports additive tool anchors, with
  fallback to current tools for removals/redefinitions. Responses, Azure and Codex share
  `additional_tools` / client tool-search anchors. Anthropic supports inline additions,
  removals and same-name redefinitions, retaining the initial request-level tool declarations.
  Anthropic defers updates until the next assistant or transcript end; transform normalization
  keeps system messages out of tool-call/result pairs. Pi-message transport retains the transcript.
- Compaction estimates system text and tool declaration tokens and excludes system instructions
  from summarization text. Both native and previously imported system entries stay out of the UI.
  Child paging advances through pages containing only hidden system metadata.
- Schema 3 → 4 first writes private `engine.sqlite.v3.bak`; no table rewrite is needed, but
  the version prevents older runtimes from interpreting unsupported message bodies. Protocol 5
  similarly refuses incompatible desktop/worker pairs. Earlier migration tests remain passing.

## Executed reference evidence

`fixtures/native-system/prompt-reference.json` records **10 cases executed against pinned Pi
revision d2a311097cbcf669e699479587332ae3988a49d0**. Source file hashes are embedded. Capture
with `node scripts/capture-system-reference.mjs PI_CHECKOUT OUTPUT_JSON`; the script verifies
HEAD and unchanged source files, copies four modules to a temporary directory, and rewrites
only their runtime import paths to local modules. Function bodies are unmodified. No Pi or
Node dependency is added to the Rust build or runtime. A second capture compared byte-identical.

`context::system::tests::planner_and_collapse_match_executed_pinned_reference_cases` compares
planned entries (including serialized section order and omit edits) and collapsed results:
initial/unchanged state, minimal changes, reordered sections, additions, removals, redefinitions,
tool order repair, compaction rebasing and an already rebased head. Native tests also cover
legacy native imports, deletion/re-add replay and preserved declarations.

Provider request tests cover Anthropic tool-result adjacency and inline replacement/removal,
Responses additional-tools/tool-search versus collapsed requests, and Chat Completions
additions versus non-additive fallback. Runtime tests cover a stable baseline across turns,
disk reopen and a project-instruction patch. Store tests verify the schema-3 backup. Existing
imported reference fixtures and request continuation tests continue passing; their conversational
projections exclude system metadata while the import equality check now includes it.

## Subagent paging race found during verification

A full-suite run reproduced the previously intermittent inspector timeout while loading older
child history. An identical newest-page refresh cleared `loading_older`; the older page then
arrived with a cursor the core considered stale. A deterministic core test failed on the old
behavior (`None` versus `Some(ItemId(100))`). The fix retains the pending cursor when child and
oldest visible item are unchanged, while genuine range/selection changes still invalidate it.
`an_identical_live_page_does_not_cancel_an_older_page_in_flight`, all 10 core subagent tests,
and the full inspector end-to-end test now pass. The native adapter test additionally verifies
advancing the raw cursor through a page containing only hidden system metadata.

## Checks and memory

Final required checks: **1346 passed, 0 failed, 53 ignored**; Clippy and formatting pass.
Logs: `/tmp/pipkin-system-final-workspace.log`, `/tmp/pipkin-system-final-clippy.log`, and
`/tmp/pipkin-system-final-fmt.log`. Initial failures exposed the older tests' assumptions that
systems never appear in context, a fixed protocol-version assertion, the serialized tool-field
ordering difference versus reference, and the paging race above. They were fixed and rerun.

The unchanged 100 warmup + 1500 measured headless soak passed after the context changes:
**28,940 KiB idle; 40,148 KiB sampled peak; 40,296 KiB VmHWM**, stable idle fds **13 → 13**,
**one process**, second-half slope **1.911 KiB/turn**. Turn p50/p95 **26.173/44.934 ms**;
provider request bytes **459260683**, equal to the preceding headless baseline workload.
Raw evidence: `fixtures/native-bench/2026-10-11-positional-system/`. Engine SHA-256:
`0475dc9200f87b9398673355f3edf482abd90ae92b24d08bee081f76843bbb5a`.
The script used `SKIP_APP=1 TESTS=soak_1500`; any app/package size from its sizes step is an
older artifact, not package qualification. This is Linux/glibc headless evidence, not proof of
indefinitely flat memory, desktop responsiveness, other allocators, or other platforms.

## Remaining boundaries

PRM-PATCH is now measured, with an executed differential for the pure planning/replay contract;
it is not fully qualified. Complete backend differentials, every provider's live/installed
request path, replacement context edits/forks, and custom extension prompt sections remain
part of the overall release gates. `ToolSpec` still lacks executable constrained/grammar tool
support even though system declarations preserve its data; that adapter/extension gap is not
closed by the new anchors. No default switch or Pi removal is claimed.
