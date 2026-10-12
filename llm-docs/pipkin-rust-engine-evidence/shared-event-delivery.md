# Shared native event delivery

Implemented on top of `cb98d0a`, protocol **6** / schema **4** unchanged.

## Implementation and scope

A runtime broadcast now constructs one immutable typed event plus one bounded JSON body.
Each client's presentation queue keeps a shared reference, with its own epoch/sequence
and reply barrier. Private stdout and external Unix writers add a small connection-specific
frame envelope and write the cached event bytes directly. They do not clone or reserialize
the event body. In-process consumers retain the existing typed frame API; the last consumer
can move the event, while concurrent typed consumers clone it when needed.

The 128-event / 2 MiB serialized-body budget remains per client. This budget is not a bound
on the entire heap: the shared payload retains both its typed value and encoded bytes, and
queues/envelopes also have overhead. Encoding aborts at the body budget; oversized events
release their original payload and require durable resync. One slow subscriber coalesces
independently. Sign-in publications remain client-local. Replies retain separate reserved
capacity and ordering; they are individually encoded because their request IDs and results
belong to a single client.

This closes serialize-once event fan-out, not frontend core-bus or provider-internal queue
budgeting, full transport differentials, or complete streaming/performance qualification.
The JSON wire format, durable outbox and replay behavior remain unchanged.

## Evidence

- `dq::subscribers_share_one_payload_but_keep_independent_envelopes_and_pressure` checks
  32 subscriber queues share the same allocation, all retain their own sequence, and pressure
  on one releases only that subscriber's reference. Every healthy envelope is byte-identical
  to the existing frame encoder and readable with the existing decoder.
- `dq::shared_envelopes_preserve_escaping_extreme_ids_and_write_errors` covers JSON control
  characters/Unicode, maximum epoch/sequence values, resync envelopes and write errors.
- `dq::escaped_payload_size_is_bounded_and_oversize_does_not_retain_the_event` checks encoded
  size rather than raw text length and confirms recovery admits later ordinary events.
- `delivery::shared_transport_preserves_reply_barriers_and_client_local_sequences` reads
  serialized output after private and shared events around a reply barrier.
- Existing pressure, durable catch-up, concurrent/private sequence, socket, worker-process,
  auth and desktop integration tests exercise the unchanged consumer contract.

Focused protocol/delivery tests passed; logs `/tmp/pipkin-fanout-{protocol,delivery}.log`.
Required checks passed: **1389 passed, 0 failed, 53 ignored**, Clippy and formatting clean.
Logs: `/tmp/pipkin-fanout-final-{workspace,clippy,fmt}.log`. The first workspace run also
passed; its Clippy warning about a large output enum was resolved by boxing the reply arm
before the final checks.

## Repeated headless memory soak

The unchanged **100 warmup + 1500 measured turns** passed on Linux/glibc:
**29,164 KiB idle**, **39,912 KiB sampled peak**, **40,408 KiB VmHWM**, **one process**,
idle descriptors **13 → 13**, and second-half RSS slope **1.482 KiB/turn**. This stays below
the existing 100 MiB idle/peak budget. Provider request bytes remain **459260683**, matching
the earlier workload. Raw measurements and machine metadata:
`fixtures/native-bench/2026-10-11-shared-events/`. Engine SHA-256:
`2db93f2d9e73a22871b02978155511848d5500144916788642b8b35c93f0cb01`.

Turn p50/p95 were **31.622/57.418 ms**, CPU **27.807 ms/measured turn**. The older positional
run recorded **26.173/44.934 ms** and **23.520 ms/turn**. These are not matched before/after
runs: intervening engine changes and system load differ (reported one-minute load 8.22 vs
5.66). The slower values are retained; no latency/CPU improvement or non-regression gate is
claimed from this soak. Shared allocation and avoided per-subscriber encoding are established
by code and focused tests, not a comparative end-to-end speed benchmark.

Command: `SKIP_APP=1 TESTS=soak_1500 OUT=fixtures/native-bench/2026-10-11-shared-events scripts/bench-native.sh`.
Only the engine was rebuilt; the script's app-size output describes an older artifact and is
not native package qualification. This run does not prove indefinitely flat memory, an
external multi-client soak, desktop responsiveness, other allocators/platforms, or live-provider
behavior.
