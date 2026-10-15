use crate::store::ExternalRunKey;

use super::{CliScope, Fingerprint, PermissionRequest};

/// Host identities, not identities supplied by the hook. All fields participate
/// in equality; `owner` is the engine-assigned connection identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunScope {
    pub epoch: u64,
    pub owner: u64,
    pub key: ExternalRunKey,
    pub run_id: String,
    pub cli: CliScope,
}

impl RunScope {
    fn valid(&self) -> bool {
        [&self.key.session, &self.key.op, &self.run_id]
            .into_iter()
            .all(|s| {
                !s.is_empty()
                    && s.len() <= 128
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
            })
    }
}

/// Nonce is host-generated, unpredictable and unique, NEVER tool_use_id.
/// Public fields permit later IPC adapters to reconstruct an untrusted submission;
/// the gate always checks every field against its own admitted record.
#[derive(Clone, PartialEq, Eq)]
pub struct Ticket {
    pub scope: RunScope,
    pub nonce: [u8; 32],
    pub fingerprint: Fingerprint,
}

impl std::fmt::Debug for Ticket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Ticket([redacted])")
    }
}

/// Sensitive plaintext: intentionally not Debug or Serialize.
pub struct Review {
    pub ticket: Ticket,
    display: String,
}

impl Review {
    pub fn argument_display(&self) -> &str {
        &self.display
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    AllowOnce,
    Deny,
}

/// Persist atomically before any response bytes escape. AllowOnce is already
/// consumed, NOT an outstanding grant. Deny is terminal too. Bind every field,
/// including the exclusive deadline, in the durable compare-and-set operation.
#[derive(Clone, PartialEq, Eq)]
pub struct DecisionRecord {
    pub ticket: Ticket,
    pub decision: Decision,
    pub deadline: u64,
}

impl std::fmt::Debug for DecisionRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DecisionRecord([redacted])")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GateLimits {
    pub pending: usize,
    pub total: usize,
}

impl Default for GateLimits {
    fn default() -> Self {
        Self {
            pending: 16,
            total: 128,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateError {
    Invalid,
    WrongScope,
    Fenced,
    ClockRegressed,
    Expired,
    Capacity,
    Duplicate,
    NotPending,
    FingerprintChanged,
    Persistence,
    Delivery,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Pending,
    Consumed,
    Denied,
    Expired,
    Revoked,
}

struct Entry {
    nonce: [u8; 32],
    fingerprint: Fingerprint,
    deadline: u64,
    state: State,
}

/// Pure per-run state, with no locks, timers, I/O, async methods or retry queue.
/// The host must hold its serialization lock across each complete method call.
/// Terminal entries remain until the gate is dropped; no eviction/reuse.
/// No Clone/Deserialize/recovery path can resurrect an issued permission.
pub struct PermissionGate {
    scope: RunScope,
    limits: GateLimits,
    last_now: u64,
    fenced: bool,
    entries: Vec<Entry>,
}

impl PermissionGate {
    /// `reserve_fresh_run` must durably reserve this exact run exclusively and
    /// reject previously reserved/recovered runs, even after a crash or owner/
    /// epoch change. Uniqueness is on the underlying external run, not the owner.
    /// No successful no-op is allowed. The caller owns persistent recovery/fencing.
    pub fn new<E>(
        scope: RunScope,
        limits: GateLimits,
        now: u64,
        reserve_fresh_run: impl FnOnce(&RunScope) -> Result<(), E>,
    ) -> Result<Self, GateError> {
        if !scope.valid()
            || limits.pending == 0
            || limits.pending > 32
            || limits.total < limits.pending
            || limits.total > 128
        {
            return Err(GateError::Invalid);
        }
        reserve_fresh_run(&scope).map_err(|_| GateError::Persistence)?;
        Ok(Self {
            scope,
            limits,
            last_now: now,
            fenced: false,
            entries: Vec::new(),
        })
    }

    /// Permanent fence: no future admission or allow, including after failures.
    /// Does not undo prior consumption/delivery or claim a process was stopped.
    pub fn fence(&mut self) {
        self.fenced = true;
        for entry in &mut self.entries {
            if entry.state == State::Pending {
                entry.state = State::Revoked;
            }
        }
    }

    pub fn is_fenced(&self) -> bool {
        self.fenced
    }

    fn advance(&mut self, now: u64) -> Result<(), GateError> {
        if now < self.last_now {
            self.fence();
            return Err(GateError::ClockRegressed);
        }
        self.last_now = now;
        for entry in &mut self.entries {
            if entry.state == State::Pending && now >= entry.deadline {
                entry.state = State::Expired;
            }
        }
        if self.fenced {
            return Err(GateError::Fenced);
        }
        Ok(())
    }

    /// Host supplies a fresh nonce and an absolute monotonic deadline. Requests
    /// must already have been parsed outside rendering. Admission grants nothing.
    pub fn admit(
        &mut self,
        scope: &RunScope,
        nonce: [u8; 32],
        request: PermissionRequest,
        now: u64,
        deadline: u64,
    ) -> Result<Review, GateError> {
        self.advance(now)?;
        if scope != &self.scope || request.scope != self.scope.cli {
            return Err(GateError::WrongScope);
        }
        if nonce == [0; 32] {
            return Err(GateError::Invalid);
        }
        if deadline <= now {
            return Err(GateError::Expired);
        }
        if self.entries.iter().any(|entry| entry.nonce == nonce) {
            return Err(GateError::Duplicate);
        }
        if self.entries.len() >= self.limits.total
            || self
                .entries
                .iter()
                .filter(|entry| entry.state == State::Pending)
                .count()
                >= self.limits.pending
        {
            return Err(GateError::Capacity);
        }
        let fingerprint = request.fingerprint();
        self.entries.push(Entry {
            nonce,
            fingerprint,
            deadline,
            state: State::Pending,
        });
        Ok(Review {
            ticket: Ticket {
                scope: self.scope.clone(),
                nonce,
                fingerprint,
            },
            display: request.argument_display().to_owned(),
        })
    }

    /// Consume locally BEFORE calling persistence (including if it panics), then
    /// require durable decision+consumption success BEFORE bounded synchronous
    /// delivery. Neither callback may await, re-enter or retain a response for
    /// replay. Errors expose no callback details; ambiguity fences the whole run.
    /// A successful return means response delivery only, not tool execution.
    pub fn decide<P, D>(
        &mut self,
        ticket: &Ticket,
        decision: Decision,
        now: u64,
        persist: impl FnOnce(&DecisionRecord) -> Result<(), P>,
        deliver: impl FnOnce(&[u8]) -> Result<(), D>,
    ) -> Result<(), GateError> {
        self.advance(now)?;
        if ticket.scope != self.scope {
            return Err(GateError::WrongScope);
        }
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.nonce == ticket.nonce)
            .ok_or(GateError::NotPending)?;
        if ticket.fingerprint != entry.fingerprint {
            return Err(GateError::FingerprintChanged);
        }
        if entry.state == State::Expired {
            return Err(GateError::Expired);
        }
        if entry.state != State::Pending {
            return Err(GateError::NotPending);
        }
        entry.state = match decision {
            Decision::AllowOnce => State::Consumed,
            Decision::Deny => State::Denied,
        };
        let record = DecisionRecord {
            ticket: ticket.clone(),
            decision,
            deadline: entry.deadline,
        };
        // Fail closed on unwinding as well as Err: restore eligibility for other
        // pending entries only after both synchronous callbacks have succeeded.
        self.fenced = true;
        if persist(&record).is_err() {
            self.fence();
            return Err(GateError::Persistence);
        }
        let bytes: &[u8] = match decision {
            Decision::AllowOnce => b"{\"hookSpecificOutput\":{\"hookEventName\":\"PermissionRequest\",\"decision\":{\"behavior\":\"allow\"}}}\n",
            Decision::Deny => b"{\"hookSpecificOutput\":{\"hookEventName\":\"PermissionRequest\",\"decision\":{\"behavior\":\"deny\",\"message\":\"Permission denied.\",\"interrupt\":true}}}\n",
        };
        if deliver(bytes).is_err() {
            self.fence();
            return Err(GateError::Delivery);
        }
        self.fenced = false;
        Ok(())
    }
}
