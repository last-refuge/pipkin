# Native request cancellation

Implemented after `03b7e5e` on 2026-10-11. Native protocol **9**, schema **5**.

## Audit and behavior

This is required desktop behavior. The Pi adapter calls `pending.cancel()` after its
short-call, prompt, and queue admission timeouts (`adapters/pi/mod.rs`), and pi-client
explicitly documents that accepted work may still complete. The pinned Pi checkout
was verified at `d2a311097cbcf669e699479587332ae3988a49d0`.
`packages/server/src/server.ts:298-305` cancels only an active request on the calling
connection with the matching target; missing/completed requests have no future effect.
Its request handler retains the request until completion and reports cancellation
without rolling back committed work. These are source observations, not an executed
transport differential.

Native `CancelRequest { request }` uses the independent control lane. Requests register
before enqueueing, so a cancellation can overtake a queued normal request without being
lost. Each connection owns its own bounded pending map; duplicate pending ids are
refused, failed sends remove registrations, and a drop guard removes completed or
panicked request tasks. Unknown/completed ids create no cancellation tombstones.

Prompt/queue/Stop/wait requests may drop their futures at cancellation-safe suspension
points. A queued cancellation prevents dispatch. Manual compaction gets the request's
cancellation token, so its provider stream can stop without committing a new context
boundary. Started blocking imports and catalog refreshes retain their task slots until
the blocking work finishes: cancellation must not turn them into unlimited detached
jobs. Committed transactions, spawned coding runs and sign-in attempts are not undone;
Stop and AuthCancel remain their explicit lifecycle controls.

The desktop client sends a best-effort cancellation after an RPC timeout, without
waiting again. The result stays `Lost`; `Cancelled` is not a definite admission refusal.
Lost prompt/queue acknowledgments still reconcile by durable request key. A cancellation
acknowledgment means the signal was handled, not that a mutation was rolled back.

## Verification

- Queued prompt behind all 16 occupied normal slots is cancelled through control capacity;
  in-flight lock waiters are cancelled too; none journal an input, and a later prompt runs.
- Two connections waiting on the same admitted input: cancelling one frees only its wait
  permit; the other receives the final answer and the provider executes once.
- Unknown/completed cancellation has no effect on a future request with that id.
- Cancelling a manual provider-backed compaction leaves context unchanged.
- Cancellation during serialized imports finishes cleanup and leaves the connection usable.
  This is a cleanup test; it does not measure peak process memory under repeated imports.
- Duplicate/full-queue refusal and request guard cleanup leave no stale registrations.
- Desktop timeout sends the control cancellation, retains `Lost`, and drops late replies.
- Existing worker, socket, disconnect, durable-admission, crash and delivery tests rerun.

Required final workspace checks: **1435 passed, 0 failed, 53 ignored**; Clippy and
formatting clean. Logs: `/tmp/pipkin-request-cancel-final-{workspace,clippy,fmt}.log`.
No live provider, GUI, or installed workflow was exercised. Full transport differential
and sustained desktop qualification remain open; native is still opt-in.
