# Captured CLI workspace observation

CLI Changes are **project Git observations, not action receipts or attribution**. They never
parse approval arguments into paths, fabricate native tool evidence, authorize a decision,
restore CLI context, or enter model history/goals/automation.

## Scope and invalidation

- Known CLI profiles scan the captured project, including unrelated uncommitted files.
  Native profiles retain session-associated path scoping.
- Each background result binds a scan operation, connection, engine epoch, conversation,
  attachment generation, engine session, cwd, scope kind and invalidation revision.
  An older result cannot release a newer scan or publish into a replacement source.
- Accepted external action observations and run settlement invalidate the cached observation.
  Manual Refresh also invalidates. Work remains off render/GPUI; one scan is in flight per live
  adapter, with coalesced rescan intent. Waiting scans advance their poll deadline rather than
  spinning on an expired timer.
- Git data remains independent of approval/receipt state. A clean Git view does not prove that
  an uncertain Write had no effect. Effects outside the captured project or ignored by Git are
  not covered. Output/file previews are bounded; this is not a complete filesystem audit.

## Read-only Git boundary

`adapters/workspace/git.rs` separates non-content discovery/name listing from content comparison.
Discovery and name listing use fixed built-in Git commands with fsmonitor hooks, untracked caching
and lazy fetching disabled, no pager/terminal prompt, and no inherited Git-directory/config or
loader overrides. Name listing retains source ignore rules.

Content comparison uses a private temporary gitdir/config/index, captured HEAD and the source
object store. It does not load source/global executable clean/smudge/process filters, diff
commands or textconv drivers. The index copy is regular-file checked and bounded to 16 MiB;
Unix opens reject final symlinks and do not block opening a substituted FIFO. The private index
preserves the source index's modification time: assigning a fresh timestamp breaks Git's
racy-clean detection and can hide rapid same-size edits. A deterministic timestamp/content
regression also checks the original index remains untouched. Private files are
removed with their owner. Source repository contents/index are not written. SHA-1/SHA-256,
unborn repositories, linked worktrees and nested project directories are covered offline.
Stdout/stderr are concurrently drained with bounded retained output and process deadlines.

Comparisons deliberately do not reproduce custom filter/configuration semantics (for example,
LFS or user-specific normalization). This is not a sandbox, an atomic filesystem snapshot, or
protection against a hostile same-UID process. Git errors preserve old evidence with an unavailable
status rather than asserting no changes. Untracked symlinks/special files are not previewed;
these checks are not complete ancestor-race/filesystem containment.

## Evidence and remaining work

Job 99 passes workspace tests, strict Clippy, formatting and diff checks. Tests include configured
clean/smudge/process/textconv/external-diff/fsmonitor traps not executing, source index preservation,
ignore rules, SHA-256, linked/nested worktrees, symlink-index rejection, scope replacement, and
core→adapter→engine→fake CLI Write→Git inspection→commit/refresh clearing without native tool evidence.
The inspector labels project observation separately from native session-associated changes.

No real CLI account request or native desktop input was used. Durable uncertain metadata recovery,
redacted diagnostics and hints-only fresh handoffs are now implemented; see `CLI_RECOVERY_HARDENING.md`.
Actual helper/managed-policy/desktop qualification remains. The engine-kill fixture demonstrates
that an already-authorized action can finish after engine death; it does not qualify containment.
Production CLI execution remains default-off.
