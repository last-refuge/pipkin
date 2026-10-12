# Durable generation reference

`reference.json` captures actual execution of Pi's durable Harness at
`d2a311097cbcf669e699479587332ae3988a49d0`. Reproduce with:

```sh
node scripts/capture-generation-reference.mjs PI_CHECKOUT fixtures/native-generation/reference.json
cargo test -p pipkin-engine generation_reference -- --nocapture
```

The script checks the pinned revision and clean relevant source trees, and records source
hashes and Node version. Each case uses an isolated MemoryStorage and disposable execution
directory, actual CodingTools, and the local faux model catalog. A Models proxy supplies
deterministic normalized responses directly to `streamSimple`/`completeSimple`; it does not
replace generation, context derivation, tool dispatch, retry, compaction or settlement code.
There are no external services or user profiles. No Pi code is used by normal native tests.

Twenty-five cases cover normal answers, unavailable/unoffered tools, stop/length tool content,
empty tool rounds, retry success/exhaustion/disabled policy, account-limit precedence,
subscription transient errors, overlapping overflow/transient error text, Cerebras bodyless
errors, successful/failed/exhausted overflow compaction, successful/length overflow signals,
and provider abort/error messages with and without error text.

Expected traces retain complete content blocks, tool arguments and diagnostics, control
reasons, error messages, visible history and summary counts. They omit system-prompt and
provider/accounting identity fields, IDs/timestamps of durable records, and wall-clock timing
from the differential assertions; those have separate tests. Raw task outcomes are retained
as capture diagnostics. Retry/overflow classifiers and retry-delay calculations are also
executed directly and recorded.

Native tests run each case through the production runtime and built-in tools, comparing
provider request/response traces, history, outcomes and summary counts. They verify that the
write-shaped final responses never create a file, then restart and replay the settled input
to assert unchanged history/outcome and no new provider request.

Background compaction is disabled in this corpus so it isolates overflow recovery. The
initial capture mistakenly used a large background-token margin, which triggered threshold
compaction; the final fixture uses zero. The history comparison uses full history pages,
not the truncated effective-context range after compaction.
