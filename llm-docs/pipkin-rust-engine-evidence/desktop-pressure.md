# Desktop event pressure and Stop

2026-10-11, branch `native-engine`, follow-up to `baf8eff`. Engine protocol 6 and schema 4
are unchanged. This records headless controller/adapter evidence, not a native window soak
or Pi differential qualification.

## Change

The native desktop previously bounded its core event bus only by 1024 events. A large mapped
history could therefore accumulate many copies; `Worker::emit` also blocked the same worker
that processed Cancel and Shutdown. Engine-side control lanes could not help a Stop which
had not left the desktop.

`App/event_queue.rs` now admits native events under 128-event / 8 MiB owned-payload budgets.
Accounting includes String and Vec capacity, nested transcript/tool/attachment text, diffs,
subagent descriptions, usage bucket names, questions, notices and queued prompts. Enum matches
are exhaustive. A lease retains the charge after dequeue, through controller coalescing and
application to the core. Dropping the receiver closes admission, discards its queued events,
and wakes blocked producers. Pi/demo retain their previous plain/count-bounded channel.

One indivisible event larger than 8 MiB is admitted **alone**, without truncating history or
full tool output. Thus the admitted backlog is bounded by `max(8 MiB, largest event)`, with a
count bound of 128. This is not an 8 MiB process/RSS cap: the blocked native producer can own
one additional prepared event; mirrors, core state, allocator metadata and transient mapping
allocations are separate. No event is dropped to handle ordinary byte pressure.

A dedicated native Stop worker has eight queued requests plus one active call. It shares the
existing multiplexed client, whose reader continues routing replies while presentation waits.
Stop captures the client/session at admission; reconnect cannot redirect it. Queue saturation
and call failures produce the usual `(conversation, generation, op)` StopFailed event, not a
false settlement. That failure may itself await UI consumption. Successful Stop still settles
only through authoritative engine events or catch-up/Lookup.

A pending submission barrier preserves Submit-before-Stop ordering. Stop waiting for an unsent
submission has the configured call timeout and reports a retryable Stop failure if it expires.
Per-conversation sending locks and submission versions prevent a queued old Stop from cancelling
a later local prompt. Closing the client wakes pending reply waiters immediately. Shutdown
closes UI admission before asking the adapter worker to exit; queued Stop work is discarded on
shutdown. External-server ownership rules remain unchanged.

## Executed tests

The normal native end-to-end harness now uses the same budgeted channel as production.
Nine new tests cover:

- `event_queue::tests::byte_permit_covers_the_dequeued_batch_and_keeps_fifo_lossless`: byte
  pressure despite free count slots, permit retained through a taken event, whole oversized
  event admission, FIFO and released accounting.
- `event_queue::tests::count_limit_and_receiver_drop_wake_blocked_producers`: count pressure,
  receiver teardown and producer failure without stranded admission waits.
- `event_queue::tests::payload_accounting_includes_spare_capacity_nested_text_and_vector_storage`:
  spare capacity and nested transcript/question allocations.
- `adapters::native::control::tests::stop_waits_for_submission_before_crossing_the_control_lane`:
  Stop waits for submission, then reaches the control lane.
- `adapters::native::control::tests::queued_stop_cannot_cancel_a_later_local_prompt`: an old
  queued Stop is skipped before the next distinct control call after a new local submission.
- `adapters::native::control::tests::stop_queue_is_bounded_and_reconnect_does_not_retarget_old_requests`:
  eight queued controls, explicit overflow refusal, old-connection failures and a fresh Stop
  on the replacement connection.
- `adapters::native::client::tests::close_wakes_pending_calls_without_waiting_for_the_peer`:
  immediate cancellation of reply waits even while the peer keeps its channel open.
- `ne2e::stop_crosses_desktop_byte_pressure_and_settlement_recovers_after_reconnect`: real
  engine/core/SQLite, large saved history, deliberately 1 KiB admission allowance with 128
  count slots, a blocked producer and a large leased UI batch held without application. Stop
  reaches a durable Aborted lookup result before that batch or later UI events are applied. Both ordinary drain and engine restart
  preserve exact prior assistant text, both user prompts, one execution per prompt and a
  settled desktop journal.
- `ne2e::shutdown_releases_a_native_worker_waiting_for_desktop_bytes`: shutdown while the
  producer is blocked completes within three seconds without consuming UI events.

Initial queue testing found that async-channel retained queued leases after receiver drop
while senders remained alive. Explicit close/drain fixed it. The first native suite run had
86 passes and one sandbox refusal to bind the existing Unix endpoint fixture; rerunning with
local socket access passed all 87 tests at that intermediate revision. A later ordering test
brings the final native suite to 88 tests. One final workspace attempt exposed a fixture
setup race: small residual events could fill the tiny test budget before the large Opened event.
The fixture now drains setup traffic until it receives and holds an actual large event lease,
then asserts producer blocking; it no longer depends on arrival timing. The failed attempt is
retained in `/tmp/pipkin-ui-pressure-workspace-setup-race.log`. The corrected pressure/reconnect
regression passed **10/10 repetitions** (`/tmp/pipkin-ui-pressure-repeat.log`).

Final required checks: **1404 passed, 0 failed, 53 ignored**, clean Clippy and formatting.
Logs: `/tmp/pipkin-ui-pressure-{workspace,clippy,fmt}.log`.

## Remaining scope

Lifecycle/catalog callbacks, normal desktop request admission, parsing/input-unit limits,
full-message/core retention, and broader desktop memory/latency qualification remain separate
work. Control calls still depend on bounded transport send/reply behavior and may fail or time
out; no hard real-time Stop latency is claimed. The full Pi/native differential and supported
platform/installed-window checks remain open. Native stays opt-in; no parity row is qualified.
