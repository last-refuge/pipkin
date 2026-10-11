# ADR 0002: Protocol and transport

**Status:** Accepted (2026-10-11). Types, limits, frame codec, epoch/seq events, control lane
and protocol-version refusal are implemented in `pipkin-engine-protocol` (protocol version 1).
The **durable ordered outbox with acknowledged cursors and paged catch-up is not implemented**:
events are currently published in memory after commit (`runtime/mod.rs`, `Shared::publish`),
and a client that misses events must re-read snapshots. The desktop adapter that consumes this
contract is not implemented.

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
2. **Transport:** the worker's inherited stdin/stdout (ADR 0001). No public socket, no TCP
   listener, no server discovery. Platform-neutral pipes keep Windows engine work decoupled
   from Unix sockets.
3. **Shape:** `ClientFrame {id, lane, call}` → exactly one `ServerFrame::Reply {id, result}`;
   asynchronous `ServerFrame::Event {epoch, seq, event}` (`api.rs`). Typed `Call`/`Reply`/
   `SessionChange` variants replace generic Chord service calls and replicated-state ops;
   transcript updates are per-entry `EntryAppended`/`EntryReplaced`/`EntryDelta`, not
   whole-conversation reserialization.
4. **Ordering and gaps:** `seq` is monotonically increasing within one engine `epoch`; every
   session change also carries the session `revision`. A gap or a new epoch makes the client
   discard derived state and re-read `OpenSession` snapshots (`Reply::Snapshot`) and `Lookup`
   outcomes. Admission, settlement and question answers are committed before they are
   acknowledged or published (ADR 0003), so a resync never loses an outcome.
5. **Control lane:** `Call::Stop` and `Call::Shutdown` are `Lane::Control` and are handled
   ahead of queued normal calls (`Call::lane`).
6. **Versioning:** `PROTOCOL_VERSION = 1`; `Hello` with any other version is refused
   (`ErrorCode::Incompatible`); the reply `Welcome` carries engine version/revision, schema,
   epoch and capabilities. No guessing across versions.
7. **Errors:** a closed taxonomy (`Incompatible, NotFound, Conflict, Invalid, TooLarge, Busy,
   Unsupported, Storage, ShuttingDown, Internal`). `Unsupported` is the explicit result for
   reserved or not-yet-implemented capabilities (manual compaction today; the Claude
   subscription sign-in, ADR 0007).
8. **Secrets on the wire:** sign-in answers and API keys travel as `Secret`, whose `Debug`
   prints `[redacted]`; `AuthSnapshot`/`AuthChallenge` debug output omits URLs/codes. Auth
   state is client-scoped (`Shared::publish_to`) and never enters a durable outbox.
9. **Still to build (tracked, not waived):** durable outbox + acknowledged cursor + paged
   catch-up; coalesced/bounded progress with backpressure so a slow client cannot block Stop
   or grow memory without bound (today the per-client channel is an unbounded
   `std::sync::mpsc`); an opt-in owner-authenticated endpoint for external attachment
   (P19); serialize-once fan-out for multiple subscribers.

## Consequences

- One IPC hop instead of three; typed events per entry instead of path-op replication.
- The Chord replicated-state contract (`pi.transcript` ops) is not reproduced; parity is
  asserted on desktop-visible outcomes and effective model context, not on op streams. Rows
  `SVC-TRANSCRIPT-STATE` and `XPORT-*` in the parity ledger carry this.
- Unbounded per-client channels are a known gap against the plan's boundedness requirement
  until item 9 lands.

## References

- Plan §3 "Contract and transport".
- Audit `service-api.md` §4.2, §4.3, §4.7.
- Native: `crates/pipkin-engine-protocol/src/{api,frame,limits,view,content}.rs`,
  `crates/pipkin-engine/src/host.rs`, `crates/pipkin-engine/src/runtime/mod.rs`.
