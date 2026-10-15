//! Offline permission foundation; not a policy, sandbox, hook server or activation path.
//!
//! # Integration contract
//! Capture the canonical cwd and exact private CLI UUID before parsing. This module
//! validates string shape/equality only: it does not canonicalize, inspect paths,
//! constrain Write destinations, or establish filesystem safety. Closed Write
//! input plus documented common metadata is covered by synthetic tests, NOT live
//! schema qualification. Metadata paths are never opened; suggestions are never
//! applied. Non-default permission modes, subagents and unknown fields deny. The
//! eventual hook adapter must block on errors, disconnect or timeout; never turn
//! missing output into implicit permission or fall back to a weaker policy.
//!
//! Prepare requests off the UI thread. Render `argument_display()` in a literal
//! plaintext widget, never markdown/HTML/terminal markup. It is complete, escaped,
//! and bounded; do not offer approval for a truncated or substituted presentation.
//! Do not log request bytes, displays, private UUIDs, or callback errors.
//!
//! A runtime must serialize ALL admission, decisions and fencing under the same
//! lock, retaining it across these synchronous callbacks (no await). Callbacks must
//! be bounded and non-reentrant. Delivery must also be nonblocking and use an
//! already-reserved, bounded transport tied to the exact nonce/request, not enqueue
//! an authorization for later re-evaluation/retry. No unbounded pipe/socket write.
//! The injected monotonic timestamps share one clock domain; deadlines are exclusive.
//! Persistence must also enforce the deadline at commit if callbacks take time.
//!
//! `PermissionGate::new` requires durable, exclusive fresh-run reservation. A
//! restarted owner MUST reject that reservation for every previously opened run,
//! even if no decision was recorded or the owner/epoch changed. Reservation must
//! be unique on the underlying ExternalRunKey/run ID, not just the full scope.
//! Never reconstruct a fresh gate for an old external run. The
//! decision callback atomically validates live scope/deadline and durably records
//! the decision AND consumption, rejecting any existing nonce/decision. Success
//! means committed, not queued. On uncertainty fail closed. These contracts cannot
//! be implemented by an in-memory callback in production. This module does no I/O.
//!
//! Stop, owner loss, epoch/config change and shutdown must fence before releasing
//! the lock; persist run invalidation in the host. Fencing cannot undo bytes already
//! issued. Delivery success means only response delivery, NEVER tool completion.
//! Receipt-backed composition lives in runtime/external_permissions.rs. Coding
//! actor/IPC/UI activation and installed-version qualification remain future work.

mod gate;
mod request;

pub use gate::{
    Decision, DecisionRecord, GateError, GateLimits, PermissionGate, Review, RunScope, Ticket,
};
pub use request::{
    CliScope, Fingerprint, MAX_DISPLAY_BYTES, MAX_INPUT_BYTES, PermissionRequest, RequestError,
};

#[cfg(test)]
mod tests;
