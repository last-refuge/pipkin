# Sustained automatic-compaction qualification

2026-10-12, following `ca8c89b`, protocol 12 / engine schema 10 unchanged.
No production runtime change in this batch.

## Workload

The existing mixed benchmark gains opt-in `PERF_MIXED_COMPACT=1`. It retains eight
attached sessions, two concurrent turns, real 1 MiB shell outputs with unique effects,
and 64 KiB replies streamed in 64 chunks. The new mode configures a 65,536-token
window, 8,192 reserved tokens, 16,384 recent tokens and foreground automatic compaction.
Reported input usage is serialized message bytes divided by four, rounded up; output
usage is sized to the scripted response. This exercises growing token estimates and
repeated threshold crossings. It is **not** an actual provider tokenizer or an evaluation
of model-generated summary quality. The local provider supplies a recognizable bounded
summary; the harness checks that later generation requests contain it.

Assertions retain all original workload checks: every unique shell effect, full saved
reply, complete spill file, distinct saved spill link and durable tool output; settled
runs; empty finalized chunks; stable idle fds; no settled descendants; restart/replay
without extra calls. Additional checks require repeated saved compactions covering all
eight sessions, a one-to-one match with summarizer calls, subsequent use of summaries,
and generation requests below 1 MiB despite retained full history.

No history is deleted, durability weakened or user profile touched. The workload uses
a private btrfs profile and a local HTTP stub; GUI/provider/test-client memory is excluded
from worker measurements. This is local engine qualification, not live-provider or desktop
acceptance. Fixed-usage mixed benchmarks keep their existing default behavior.

## Measured results

The 96-turn pilot with a restart at turn 48 and a final reopen passes: 24 saved
compactions, 216 total provider requests, all 96 tool effects/outputs/replies exact.
The full **720-turn run has no intermediate restart** and a final reopen/replay check.
All 720 unique effects, 64 KiB replies, 1 MiB spill files, saved links and durable outputs
verify. There are **232 saved compactions**, 232 summarizer calls, 1,376 generation
requests containing the summary and 1,672 total provider calls. The largest generation
request is **210,977 bytes**, independent of the retained full transcript size.

| Worker measurement | Full run |
| --- | ---: |
| Initial RSS | 27,124 KiB |
| Final RSS | 29,396 KiB |
| Kernel peak VmHWM | 39,312 KiB (38.39 MiB) |
| Final reopened RSS | 29,200 KiB |
| Idle file descriptors | 13 → 13 |
| Settled tool descendants | 0 |
| Last-half RSS slope | +1.326 KiB/turn |
| Workload duration | 263.752 seconds |

The sampled 50 ms RSS peak is 37,884 KiB; kernel VmHWM captures the higher peak above.
The database retains 754,984,690 bytes of full tool output (plus history/metadata) and
ends at 811,048,960 bytes. Model-context compaction does not discard durable transcript
or output data. Positive residual slopes are recorded; this bounded run does not prove
indefinite leak freedom. Workspace checks and a separate isolated Xwayland inspection
overlapped portions of the run; timing is not an idle-machine latency comparison.

Raw [full run](../../fixtures/native-bench/2026-10-12-sustained-compaction/) and
[pilot](../../fixtures/native-bench/2026-10-12-compaction-pilot/) retain samples, logs,
source identity and binary hash. The runner exits successfully. Required checks pass:
**1,656 tests passed, 0 failed, 56 ignored**, clean Clippy/all-targets and formatting;
benchmark shell syntax and diff checks pass. No additional ignored test is introduced:
this is an explicitly executed mode of the existing mixed benchmark.

## Hosted qualification

The Linux/macOS workflow changes are committed as `ca8c89b`. GitHub initially rejected
the isolated qualification-branch push because the OAuth credential lacked `workflow`
scope. After explicit user authorization, the push succeeded and both workflows started:
[Linux qualification](https://github.com/last-refuge/pipkin/actions/runs/38259320193) and
[experimental macOS](https://github.com/last-refuge/pipkin/actions/runs/38259324745).
Results were still pending when this local evidence was recorded. Both runs identify
`ca8c89b`; the subsequent benchmark/documentation additions do not change runtime code.
