# Executed durable generation comparison

2026-10-12, following `9fd1bb5` on `native-engine`. Protocol 12/schema 10 unchanged.
Native remains opt-in; Pi extension compatibility remains out of scope.

## Reference and scope

The capture script and frozen corpus are `scripts/capture-generation-reference.mjs` and
`fixtures/native-generation/reference.json`. Pi's actual durable Harness, scheduler, context,
CodingTools, retry, compaction and settlement code execute at pinned revision
`d2a311097cbcf669e699479587332ae3988a49d0`; source identities and clean-tree checks are
recorded. Synthetic normalized provider responses replace only the model exchange. This
comparison does not test cloud transports, credentials, the full Pi system prompt, accounting,
stream timing or platform GUI behavior. No user profiles or external services are accessed.

Twenty-five cases compare terminal outcomes and error details, complete projected provider
request/response traces (including summary requests), visible history, and compaction count.
Native uses production tools; attempted writes inside final stop/length messages must create
no file. Every native case then restarts and replays its original durable input key, checking
that history/outcome remain unchanged and no further model request occurs. The same corpus
also compares raw retry/overflow classification and capped retry-delay calculations.

## Reproduced differences and fixes

1. **Final responses executed tools.** Native started pending calls on any response except
   error/abort. Actual Pi starts them only for `toolUse`; `stop` and `length` are final even
   when tool-shaped content is present. The failing native cases created the probe file.
   Native now preserves such content without executing it.
2. **Overflow retried as a transient failure.** A response such as `503 prompt too long`
   matched both classifiers. Native could retry it with compaction disabled, with no usable
   summary prefix, after failed summarization, or after its one overflow compaction. Actual
   Pi excludes overflow from ordinary retries. Native now applies the same exclusion,
   including provider-specific Cerebras bodyless errors. Throttling/rate-limit exclusions
   still permit ordinary retry, and successful overflow compaction still gets one new request.
3. **Provider abort silently requested again.** A normalized aborted response without native
   user cancellation was treated as an interrupted exchange to resume. Pi settles it as a
   model error. Native now records that terminal outcome, retaining its partial text and
   diagnostic. User Stop continues to use the separate cancellation/retirement path. Missing
   provider error text now uses Pi's stop-reason diagnostic instead of a generic message.

The initial differential run reproduced all three failures. The other history mismatch in
the successful compaction case was a test projection error: it compared native effective
context with complete Pi history. The test now uses full native history pages; no production
history behavior was changed for that mismatch.

## Baseline corrections

`isContextOverflow(message, window)` detects successful usage overflow and zero-output length
responses, but the shipped durable generation path calls it only for error responses and
answers `stop`/`length` first. Both executed corpora confirm that successful/length signals
remain final answers. The old GEN-09 wording incorrectly treated the SDK helper's entire
contract as durable generation behavior. This audit corrects the row without removing a
shipped feature or adding unrequested compaction.

The generic Harness has a `toolExecution` override. The shipped coding-agent
`createHarnessSettings` does not forward it, and neither Pipkin overlay introduces it.
Per-tool sequential execution remains the relevant native contract; its existing scheduling
tests remain in the suite. Generic extension or custom Harness settings are not added here.

## Verification

```sh
node scripts/capture-generation-reference.mjs PI_CHECKOUT fixtures/native-generation/reference.json
cargo test -p pipkin-engine generation_reference -- --nocapture
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all --check
```

The two differential tests pass: **25 runtime/restart cases, 48 retry/overflow classifier
cases and seven delay calculations**. Final required checks: **1,649 passed, 0 failed,
56 ignored**; clean Clippy/all-targets and formatting. Existing real SIGKILL retry-budget
checks also pass in the workspace suite. Logs: `/tmp/pipkin-generation-{before,after,workspace,clippy,fmt}.log` and
`/tmp/pipkin-generation-reference-final.log`. No timing measurement is inferred from scaled
one/two-millisecond retry policies. Crash-during-retry and user Stop coverage remain in their
existing dedicated tests, not part of the reference capture.

GEN-01/03/04/06/07/08/09 evidence is reconciled within this scope; ledger aggregate counts
are unchanged. Broader reference and release qualification, installed/desktop fault checks,
sustained desktop workloads, CI/platform and notice review, and native-default/Pi removal
remain open. These tests do not establish full qualification of every affected ledger row.
