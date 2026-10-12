# ADR 0002: Protocol and transport

**Status:** Accepted; updated 2026-10-11 on top of `63bd9ae`. Protocol **4** adds
`CatchUp`, `AckEvents`, `EventPage` and `ResyncRequired`; schema **3** records ordered session
invalidations in the same transaction as the state change. Engine and desktop-reader presentation
queues are bounded. See [delivery and memory evidence](../pipkin-rust-engine-evidence/durable-delivery-memory.md).
Protocol 3's import-profile protection is retained. Protocol 6 adds session attachments;
ADR 0008 adds the explicit local endpoint. Event broadcasts now share one encoded body
across clients; see [shared delivery evidence](../pipkin-rust-engine-evidence/shared-event-delivery.md).

Protocol **5** adds typed positional system messages; schema **4** protects their durable bodies
from older runtimes. See [context evidence](../pipkin-rust-engine-evidence/positional-system.md).

## Context

Reference wire stack (audit `service-api.md` §4.2–4.3): CBOR frames with a 4-byte big-endian
length, 16 MiB default max (`protocol/src/framing.ts:6,28`), `hello` handshake with
`PROTOCOL_VERSION = 8`, `request/cancel/response/service_update/attachment` envelopes, Chord
replicated-state ops (`r/s/d/a/t/p/m` with path interning, `chord/src/delta/index.ts:32-66`),
`reset` rebaseline after 100 buffered deliveries, and stale guards on `serverConnectionId`,
`attachmentId`, worker token/peer/sessionKey, keyed `instance.generation`, per-member
`sequence`, auth `attempt`/`challenge.id` and prompt `requestId`. Three IPC layers carry it
(client → coordinator byte pipe → JSON-line routing to workers, `ce/process.ts:100-106`).

The desktop core already routes every backend event through `(conversation, generation, op)`
(AGENTS.md non-negotiable; `crates/pipkin-core/src/{backend,protocol}.rs`).

## Decision

1. **Encoding:** 4-byte big-endian length + UTF-8 JSON body
   (`crates/pipkin-engine-protocol/src/frame.rs`). JSON keeps the private channel inspectable;
   the codec is isolated so CBOR or another encoding can replace it after measurement without
   touching message types. Max frame `MAX_FRAME_BYTES` = 32 MiB; a larger or truncated frame is
   a refusal, never a truncation (`limits.rs`; tests `frames_round_trip_and_end_cleanly`,
   `oversized_and_truncated_frames_are_refused`, `garbage_is_a_decode_error_not_a_panic`).
2. **Transport:** the default worker's inherited stdin/stdout (ADR 0001), plus explicit
   same-user Unix attachment (ADR 0008). No TCP listener or server discovery.
   Platform-neutral private pipes keep Windows engine work decoupled from Unix sockets.
3. **Shape:** `ClientFrame {id, lane, call}` → exactly one `ServerFrame::Reply {id, result}`;
   asynchronous `ServerFrame::Event {epoch, seq, event}` (`api.rs`). Typed `Call`/`Reply`/
   `SessionChange` variants replace generic Chord service calls and replicated-state ops;
   transcript updates are per-entry `EntryAppended`/`EntryReplaced`/`EntryDelta`, not
   whole-conversation reserialization.
4. **Ordering and gaps:** `seq` is monotonically increasing per connection within one engine `epoch`; every
   session change also carries the session `revision`. A gap or a new epoch makes the client
   discard derived state and re-read `OpenSession` snapshots (`Reply::Snapshot`) and `Lookup`
   outcomes. Admission, settlement and question answers are committed before they are
   acknowledged or published (ADR 0003), so a resync never loses an outcome.
5. **Control lane:** `Call::Stop` and `Call::Shutdown` are `Lane::Control` and are handled
   ahead of queued normal calls (`Call::lane`).
6. **Versioning:** `PROTOCOL_VERSION = 6`; `Hello` with any other version is refused
   (`ErrorCode::Incompatible`); the reply `Welcome` carries engine version/revision, schema,
   epoch and capabilities. No guessing across versions.
7. **Errors:** a closed taxonomy (`Incompatible, NotFound, Conflict, Invalid, TooLarge, Busy,
   Unsupported, Storage, ShuttingDown, Internal`). `Unsupported` is the explicit result for
   not-yet-implemented capabilities. Manual compaction and Claude subscription sign-in
   are implemented; ADR 0007 records the latter's evidence boundary.
8. **Secrets on the wire:** sign-in answers and API keys travel as `Secret`, whose `Debug`
   prints `[redacted]`; `AuthSnapshot`/`AuthChallenge` debug output omits URLs/codes. Auth
   state is client-scoped (`Shared::publish_to`) and never enters a durable outbox.
9. **Durable catch-up:** SQLite triggers append `(cursor, session, revision)` on session
   creation/revision changes, atomically with the mutation. These are snapshot invalidations,
   not a second copy of transcript deltas. Admission and terminal outcomes remain in the
   permanent request/run ledger. `CatchUp {after, limit}` returns at most 256 records plus
   `through`, `head`, and `reset`; retention is the latest 4096 records regardless of clients.
   A cursor outside that range explicitly requires full snapshots/Lookup. It never silently
   skips outcomes. The client re-reads state before `AckEvents {through}`; acknowledgments
   are monotone and limited to cursors delivered on that connection. Ack state is connection
   scoped, not a persistent client identity. A reconnect may supply its last cursor; the
   desktop currently starts at zero and rebuilds from authoritative state after reconnect.
10. **Bounded delivery:** engine and desktop reader each retain at most 128 live events / 2 MiB
    of serialized event bodies. Overflow coalesces the whole pending presentation prefix into
    one `ResyncRequired`, carrying its latest sequence; private auth state is reread, never
    persisted. Replies have 16 separate slots with asynchronous backpressure, preserving
    already-published events but bypassing later progress. Engine input has 64 normal / 8
    control slots and 16 / 4 active dispatch slots; the actual Call determines its lane.
    The desktop pipe writer has 64 slots. Full input is refused, not unboundedly queued.
    The adapter polls catch-up every second (one page per pass), so a missing final event
    with no subsequent live traffic is still recovered. Paged history and the selected child
    survive resync. Session commit/publication is serialized; snapshots verify their revision.
11. **Shared event bodies:** serialize each broadcast once into a bounded immutable payload,
    shared by all client queues. Add connection-specific epoch/sequence envelopes at write
    time. Private pipe and external socket hosts write the cached bytes; in-process consumers
    retain typed frames. Oversized encoding stops at the presentation budget and becomes a
    resync marker. Serialized-body budgets are not total heap limits: one typed value and its
    encoded bytes remain shared until the last subscriber releases them. Reply encoding is
    per request. Frontend and provider-internal queue budgets remain separate work.

12. **Still open:** full external-attachment qualification and end-to-end frontend
    presentation budgeting. The shared GPUI/core event bus
    and provider-internal stream channels are outside this transport change; this ADR does
    not claim every queue in the application is now bounded.

## Consequences

- One IPC hop instead of three; typed events per entry instead of path-op replication.
- The Chord replicated-state contract (`pi.transcript` ops) is not reproduced; parity is
  asserted on desktop-visible outcomes and effective model context, not on op streams. Rows
  `SVC-TRANSCRIPT-STATE` and `XPORT-*` in the parity ledger carry this.
- Catch-up replays durable invalidations and re-reads saved outcomes; it does not replay
  mutations, old auth challenges, or every historical progress delta.

## References

- Plan §3 "Contract and transport".
- Audit `service-api.md` §4.2, §4.3, §4.7.
- Native: `crates/pipkin-engine-protocol/src/{api,frame,limits,view,content}.rs`,
  `crates/pipkin-engine/src/host.rs`, `crates/pipkin-engine/src/runtime/mod.rs`.
