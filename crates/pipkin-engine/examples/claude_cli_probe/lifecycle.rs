//! Private Phase-0 ledger; deliberately unrelated to engine store migrations.
//!
//! All state changes commit before returning. `open` does NOT recover: after a
//! restart the owner must explicitly `recover` before accepting responses or
//! launching anything. Recovery never executes, retries, or replays work.
//! `complete` requires the caller to have verified a valid final result AND root
//! exit; `interrupted` requires confirmed root exit. This ledger cannot verify
//! either fact, fence a process, or undo an already consumed external grant.
//!
//! The parent owns the database directory (not a hostile-filesystem sandbox).
//! Timestamps are caller-supplied, trusted, nondecreasing milliseconds in one
//! clock domain. IDs are 1..=128 ASCII identifier characters; fingerprints are
//! exactly 64 hexadecimal characters, matched case-sensitively. No payloads,
//! paths, prompts, or raw database errors are recorded or returned.

use std::fs::OpenOptions;
use std::path::Path;
use std::time::Duration;

use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use serde::Serialize;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Ticket {
    pub session: String,
    pub generation: u64,
    pub run: u64,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[repr(i64)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Admitted = 0,
    LaunchIntent = 1,
    Running = 2,
    Stopping = 3,
    Completed = 4,
    Interrupted = 5,
    Unknown = 6,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[repr(i64)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    Pending = 0,
    Granted = 1,
    Consumed = 2,
    Denied = 3,
    Revoked = 4,
    Expired = 5,
}

#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub phase: Phase,
    pub context_valid: bool,
    pub resume_allowed: bool,
    pub queue_allowed: bool,
}

// Intentionally not Debug: Connection's diagnostics can expose database paths.
pub struct Ledger {
    connection: Connection,
}

type Result<T> = std::result::Result<T, &'static str>;
const STORAGE: &str = "ledger storage failure";
const SCHEMA: &str = "unsupported ledger schema";
const INVALID: &str = "invalid ledger input";
const TRANSITION: &str = "invalid or stale ledger transition";
const NOT_FOUND: &str = "ledger record not found";
const APPLICATION_ID: i64 = 0x504b5030;
const MAX_APPROVALS: i64 = 128;

// Fixed-width decimal TEXT preserves unsigned u64 ordering, including values
// above SQLite's signed INTEGER limit, without lossy casts or wraparound.
const RUNS: &str = "CREATE TABLE runs (
    session TEXT NOT NULL,
    generation TEXT NOT NULL CHECK(length(generation) = 20),
    run TEXT NOT NULL CHECK(length(run) = 20),
    phase INTEGER NOT NULL CHECK(phase BETWEEN 0 AND 6),
    context_valid INTEGER NOT NULL CHECK(context_valid IN (0, 1)),
    PRIMARY KEY(session, generation, run)
) WITHOUT ROWID";
const APPROVALS: &str = "CREATE TABLE approvals (
    session TEXT NOT NULL,
    generation TEXT NOT NULL,
    run TEXT NOT NULL,
    id TEXT NOT NULL,
    fingerprint TEXT NOT NULL CHECK(length(fingerprint) = 64),
    state INTEGER NOT NULL CHECK(state BETWEEN 0 AND 5),
    created_at TEXT NOT NULL CHECK(length(created_at) = 20),
    deadline TEXT NOT NULL CHECK(length(deadline) = 20),
    PRIMARY KEY(session, generation, run, id),
    FOREIGN KEY(session, generation, run) REFERENCES runs(session, generation, run)
) WITHOUT ROWID";

fn number(value: u64) -> String {
    format!("{value:020}")
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
}

fn validate_ticket(ticket: &Ticket) -> Result<()> {
    if identifier(&ticket.session) {
        Ok(())
    } else {
        Err(INVALID)
    }
}

fn validate_request(ticket: &Ticket, id: &str, fingerprint: &str) -> Result<()> {
    validate_ticket(ticket)?;
    if identifier(id)
        && fingerprint.len() == 64
        && fingerprint.bytes().all(|b| b.is_ascii_hexdigit())
    {
        Ok(())
    } else {
        Err(INVALID)
    }
}

fn phase(value: i64) -> Result<Phase> {
    match value {
        0 => Ok(Phase::Admitted),
        1 => Ok(Phase::LaunchIntent),
        2 => Ok(Phase::Running),
        3 => Ok(Phase::Stopping),
        4 => Ok(Phase::Completed),
        5 => Ok(Phase::Interrupted),
        6 => Ok(Phase::Unknown),
        _ => Err(SCHEMA),
    }
}

fn approval_state(value: i64) -> Result<ApprovalState> {
    match value {
        0 => Ok(ApprovalState::Pending),
        1 => Ok(ApprovalState::Granted),
        2 => Ok(ApprovalState::Consumed),
        3 => Ok(ApprovalState::Denied),
        4 => Ok(ApprovalState::Revoked),
        5 => Ok(ApprovalState::Expired),
        _ => Err(SCHEMA),
    }
}

impl Ledger {
    /// Exclusively reserves a new regular file. An initialization failure leaves
    /// it reserved; do not blindly delete/recreate it or retry external work.
    pub fn create(path: &Path) -> Result<Self> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let reservation = options.open(path).map_err(|_| STORAGE)?;
        let mut connection = Self::connect(path)?;
        Self::configure(&connection, true)?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| STORAGE)?;
        tx.execute_batch(RUNS).map_err(|_| STORAGE)?;
        tx.execute_batch(APPROVALS).map_err(|_| STORAGE)?;
        tx.pragma_update(None, "application_id", APPLICATION_ID)
            .map_err(|_| STORAGE)?;
        tx.pragma_update(None, "user_version", 1)
            .map_err(|_| STORAGE)?;
        tx.commit().map_err(|_| STORAGE)?;
        reservation.sync_all().map_err(|_| STORAGE)?;
        // Persist the new directory entry too, not just SQLite's contents.
        #[cfg(unix)]
        std::fs::File::open(
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )
        .and_then(|directory| directory.sync_all())
        .map_err(|_| STORAGE)?;
        Ok(Self { connection })
    }

    /// Existing-only. Validates the exact private v1 schema; does not change the
    /// journal mode, migrate, recover, expire approvals, or mutate ledger rows.
    pub fn open(path: &Path) -> Result<Self> {
        let connection = Self::connect(path)?;
        Self::validate_schema(&connection)?;
        Self::configure(&connection, false)?;
        Ok(Self { connection })
    }

    fn connect(path: &Path) -> Result<Connection> {
        if !std::fs::symlink_metadata(path)
            .map_err(|_| STORAGE)?
            .file_type()
            .is_file()
        {
            return Err(STORAGE);
        }
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|_| STORAGE)?;
        connection
            .busy_timeout(Duration::from_millis(1_000))
            .map_err(|_| STORAGE)?;
        Ok(connection)
    }

    fn configure(connection: &Connection, create: bool) -> Result<()> {
        let mode: String = connection
            .query_row(
                if create {
                    "PRAGMA journal_mode=WAL"
                } else {
                    "PRAGMA journal_mode"
                },
                [],
                |row| row.get(0),
            )
            .map_err(|_| STORAGE)?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(SCHEMA);
        }
        connection
            .execute_batch(
                "PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF;",
            )
            .map_err(|_| STORAGE)
    }

    fn validate_schema(connection: &Connection) -> Result<()> {
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|_| SCHEMA)?;
        let application: i64 = connection
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .map_err(|_| SCHEMA)?;
        if version != 1 || application != APPLICATION_ID {
            return Err(SCHEMA);
        }
        let mut statement = connection
            .prepare(
                "SELECT name, sql FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*' ORDER BY name",
            )
            .map_err(|_| SCHEMA)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|_| SCHEMA)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|_| SCHEMA)?;
        if rows
            != [
                ("approvals".to_owned(), APPROVALS.to_owned()),
                ("runs".to_owned(), RUNS.to_owned()),
            ]
        {
            return Err(SCHEMA);
        }
        Ok(())
    }

    fn transaction(&mut self) -> Result<Transaction<'_>> {
        self.connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| STORAGE)
    }

    /// Generations never decrease; run numbers strictly increase within a
    /// generation. A newer generation may reset its run counter, but can never
    /// bypass an invalid/interrupted/unknown session: use a NEW session instead.
    pub fn admit(&mut self, ticket: &Ticket) -> Result<()> {
        validate_ticket(ticket)?;
        let tx = self.transaction()?;
        let previous = tx
            .query_row(
                "SELECT generation, run, phase, context_valid FROM runs WHERE session=?1 ORDER BY generation DESC, run DESC LIMIT 1",
                [&ticket.session],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?, row.get::<_, bool>(3)?)),
            )
            .optional()
            .map_err(|_| STORAGE)?;
        let generation = number(ticket.generation);
        let run = number(ticket.run);
        if let Some((old_generation, old_run, old_phase, valid)) = previous
            && (old_phase != Phase::Completed as i64
                || !valid
                || generation < old_generation
                || (generation == old_generation && run <= old_run))
        {
            return Err(TRANSITION);
        }
        tx.execute(
            "INSERT INTO runs(session,generation,run,phase,context_valid) VALUES(?1,?2,?3,0,1)",
            params![ticket.session, generation, run],
        )
        .map_err(|_| TRANSITION)?;
        tx.commit().map_err(|_| STORAGE)
    }

    pub fn launch_intent(&mut self, ticket: &Ticket) -> Result<()> {
        self.transition(ticket, Phase::Admitted, Phase::LaunchIntent)
    }

    pub fn running(&mut self, ticket: &Ticket) -> Result<()> {
        self.transition(ticket, Phase::LaunchIntent, Phase::Running)
    }

    /// The caller has already confirmed a valid final result and process exit.
    /// Pending/Granted rows block completion, even if their deadlines elapsed:
    /// explicitly expire them through `approval` first (no clock is passed here).
    pub fn complete(&mut self, ticket: &Ticket) -> Result<()> {
        self.transition(ticket, Phase::Running, Phase::Completed)
    }

    /// The caller has already confirmed root exit after requesting stop.
    pub fn interrupted(&mut self, ticket: &Ticket) -> Result<()> {
        self.transition(ticket, Phase::Stopping, Phase::Interrupted)
    }

    fn transition(&mut self, ticket: &Ticket, from: Phase, to: Phase) -> Result<()> {
        validate_ticket(ticket)?;
        let tx = self.transaction()?;
        if current_phase(&tx, ticket)? != Some(from) {
            return Err(TRANSITION);
        }
        if to == Phase::Completed {
            let live: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM approvals WHERE session=?1 AND generation=?2 AND run=?3 AND state IN (0,1))",
                params![ticket.session, number(ticket.generation), number(ticket.run)],
                |row| row.get(0),
            ).map_err(|_| STORAGE)?;
            if live {
                return Err(TRANSITION);
            }
        }
        set_phase(&tx, ticket, to, to != Phase::Interrupted)?;
        tx.commit().map_err(|_| STORAGE)
    }

    /// Invalidates context and revokes all unissued approvals in one commit.
    /// Repeating stop on the current Stopping run is harmless.
    pub fn request_stop(&mut self, ticket: &Ticket) -> Result<()> {
        self.invalidate(ticket, Phase::Stopping)
    }

    /// Marks an uncertain active run unusable, without claiming process exit.
    /// Repeating Unknown on the current Unknown run is harmless.
    pub fn unknown(&mut self, ticket: &Ticket) -> Result<()> {
        self.invalidate(ticket, Phase::Unknown)
    }

    fn invalidate(&mut self, ticket: &Ticket, target: Phase) -> Result<()> {
        validate_ticket(ticket)?;
        let tx = self.transaction()?;
        let current = current_phase(&tx, ticket)?.ok_or(TRANSITION)?;
        if current != target
            && !matches!(
                current,
                Phase::Admitted | Phase::LaunchIntent | Phase::Running | Phase::Stopping
            )
        {
            return Err(TRANSITION);
        }
        set_phase(&tx, ticket, target, false)?;
        revoke(&tx, ticket)?;
        tx.commit().map_err(|_| STORAGE)
    }

    /// Returns the number of nonterminal runs made Unknown. Idempotent, with no
    /// external effects. Completed history remains valid and resumable if current.
    pub fn recover(&mut self) -> Result<usize> {
        let tx = self.transaction()?;
        let count = tx
            .execute(
                "UPDATE runs SET phase=6, context_valid=0 WHERE phase IN (0,1,2,3)",
                [],
            )
            .map_err(|_| STORAGE)?;
        tx.execute(
            "UPDATE approvals SET state=4 WHERE state IN (0,1) AND EXISTS (
                SELECT 1 FROM runs WHERE runs.session=approvals.session
                AND runs.generation=approvals.generation AND runs.run=approvals.run AND runs.phase=6
            )",
            [],
        )
        .map_err(|_| STORAGE)?;
        tx.commit().map_err(|_| STORAGE)?;
        Ok(count)
    }

    pub fn snapshot(&self, ticket: &Ticket) -> Result<Snapshot> {
        validate_ticket(ticket)?;
        // One statement gives historical state and currentness in one snapshot.
        let (state, valid, current): (i64, bool, bool) = self.connection.query_row(
            "SELECT phase, context_valid, NOT EXISTS (
                SELECT 1 FROM runs newer WHERE newer.session=runs.session AND
                (newer.generation>runs.generation OR (newer.generation=runs.generation AND newer.run>runs.run))
            ) FROM runs WHERE session=?1 AND generation=?2 AND run=?3",
            params![ticket.session, number(ticket.generation), number(ticket.run)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).optional().map_err(|_| STORAGE)?.ok_or(NOT_FOUND)?;
        let phase = phase(state)?;
        let allowed = current && valid && phase == Phase::Completed;
        Ok(Snapshot {
            phase,
            context_valid: valid,
            resume_allowed: allowed,
            queue_allowed: allowed,
        })
    }

    pub fn request(
        &mut self,
        ticket: &Ticket,
        id: &str,
        fingerprint: &str,
        now_ms: u64,
        ttl_ms: u64,
    ) -> Result<()> {
        validate_request(ticket, id, fingerprint)?;
        if !(1..=60_000).contains(&ttl_ms) {
            return Err(INVALID);
        }
        let deadline = now_ms.checked_add(ttl_ms).ok_or(INVALID)?;
        let tx = self.transaction()?;
        if current_phase(&tx, ticket)? != Some(Phase::Running) {
            return Err(TRANSITION);
        }
        let count: i64 = tx
            .query_row(
                "SELECT count(*) FROM approvals WHERE session=?1 AND generation=?2 AND run=?3",
                params![
                    ticket.session,
                    number(ticket.generation),
                    number(ticket.run)
                ],
                |row| row.get(0),
            )
            .map_err(|_| STORAGE)?;
        if count >= MAX_APPROVALS {
            return Err("ledger approval limit reached");
        }
        tx.execute(
            "INSERT INTO approvals(session,generation,run,id,fingerprint,state,created_at,deadline) VALUES(?1,?2,?3,?4,?5,0,?6,?7)",
            params![ticket.session, number(ticket.generation), number(ticket.run), id, fingerprint, number(now_ms), number(deadline)],
        ).map_err(|_| TRANSITION)?;
        tx.commit().map_err(|_| STORAGE)
    }

    /// False means no decision was applied: wrong scope/fingerprint, non-Pending,
    /// expired, or no longer Running. Malformed bindings also return false.
    pub fn decide(
        &mut self,
        ticket: &Ticket,
        id: &str,
        fingerprint: &str,
        allow: bool,
        now_ms: u64,
    ) -> Result<bool> {
        self.change_approval(
            ticket,
            id,
            fingerprint,
            now_ms,
            ApprovalState::Pending,
            if allow {
                ApprovalState::Granted
            } else {
                ApprovalState::Denied
            },
        )
    }

    /// The ONLY allow issuance gate: commit Consumed before sending an external
    /// allow response. A crash after commit may lose that response; NEVER replay.
    pub fn consume(
        &mut self,
        ticket: &Ticket,
        id: &str,
        fingerprint: &str,
        now_ms: u64,
    ) -> Result<bool> {
        self.change_approval(
            ticket,
            id,
            fingerprint,
            now_ms,
            ApprovalState::Granted,
            ApprovalState::Consumed,
        )
    }

    fn change_approval(
        &mut self,
        ticket: &Ticket,
        id: &str,
        fingerprint: &str,
        now_ms: u64,
        from: ApprovalState,
        to: ApprovalState,
    ) -> Result<bool> {
        if validate_request(ticket, id, fingerprint).is_err() {
            return Ok(false);
        }
        let tx = self.transaction()?;
        expire(&tx, ticket, now_ms)?;
        let changed = if current_phase(&tx, ticket)? == Some(Phase::Running) {
            tx.execute(
                "UPDATE approvals SET state=?6 WHERE session=?1 AND generation=?2 AND run=?3 AND id=?4 AND fingerprint=?5 AND state=?7 AND created_at<=?8 AND deadline>?8",
                params![ticket.session, number(ticket.generation), number(ticket.run), id, fingerprint, to as i64, from as i64, number(now_ms)],
            ).map_err(|_| STORAGE)? == 1
        } else {
            false
        };
        tx.commit().map_err(|_| STORAGE)?;
        Ok(changed)
    }

    /// Read with lazy expiry; this method can write. Unknown IDs return an error.
    /// Consumed is historical issuance, NOT a currently reusable grant.
    pub fn approval(&mut self, ticket: &Ticket, id: &str, now_ms: u64) -> Result<ApprovalState> {
        self.find_approval(ticket, id, now_ms)?.ok_or(NOT_FOUND)
    }

    /// Missing requests are normal during hook startup; never conflate them with I/O errors.
    pub fn find_approval(
        &mut self,
        ticket: &Ticket,
        id: &str,
        now_ms: u64,
    ) -> Result<Option<ApprovalState>> {
        validate_ticket(ticket)?;
        if !identifier(id) {
            return Err(INVALID);
        }
        let tx = self.transaction()?;
        if current_phase(&tx, ticket)? != Some(Phase::Running) {
            revoke(&tx, ticket)?;
        }
        expire(&tx, ticket, now_ms)?;
        let state: Option<i64> = tx.query_row(
            "SELECT state FROM approvals WHERE session=?1 AND generation=?2 AND run=?3 AND id=?4",
            params![ticket.session, number(ticket.generation), number(ticket.run), id],
            |row| row.get(0),
        ).optional().map_err(|_| STORAGE)?;
        tx.commit().map_err(|_| STORAGE)?;
        state.map(approval_state).transpose()
    }
}

fn current_phase(connection: &Connection, ticket: &Ticket) -> Result<Option<Phase>> {
    let row = connection.query_row(
        "SELECT generation,run,phase FROM runs WHERE session=?1 ORDER BY generation DESC,run DESC LIMIT 1",
        [&ticket.session],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?)),
    ).optional().map_err(|_| STORAGE)?;
    match row {
        Some((generation, run, state))
            if generation == number(ticket.generation) && run == number(ticket.run) =>
        {
            Ok(Some(phase(state)?))
        }
        _ => Ok(None),
    }
}

fn set_phase(connection: &Connection, ticket: &Ticket, phase: Phase, valid: bool) -> Result<()> {
    connection.execute(
        "UPDATE runs SET phase=?4,context_valid=?5 WHERE session=?1 AND generation=?2 AND run=?3",
        params![ticket.session, number(ticket.generation), number(ticket.run), phase as i64, valid],
    ).map_err(|_| STORAGE)?;
    Ok(())
}

fn revoke(connection: &Connection, ticket: &Ticket) -> Result<()> {
    connection.execute(
        "UPDATE approvals SET state=4 WHERE session=?1 AND generation=?2 AND run=?3 AND state IN (0,1)",
        params![ticket.session, number(ticket.generation), number(ticket.run)],
    ).map_err(|_| STORAGE)?;
    Ok(())
}

fn expire(connection: &Connection, ticket: &Ticket, now_ms: u64) -> Result<()> {
    connection.execute(
        "UPDATE approvals SET state=5 WHERE session=?1 AND generation=?2 AND run=?3 AND state IN (0,1) AND deadline<=?4",
        params![ticket.session, number(ticket.generation), number(ticket.run), number(now_ms)],
    ).map_err(|_| STORAGE)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;

    const FP: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OTHER_FP: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn ticket(session: &str) -> Ticket {
        Ticket {
            session: session.into(),
            generation: 1,
            run: 1,
        }
    }

    fn setup() -> (tempfile::TempDir, Ledger, Ticket) {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::create(&dir.path().join("ledger.sqlite")).unwrap();
        (dir, ledger, ticket("session"))
    }

    fn start(ledger: &mut Ledger, ticket: &Ticket) {
        ledger.admit(ticket).unwrap();
        ledger.launch_intent(ticket).unwrap();
        ledger.running(ticket).unwrap();
    }

    fn assert_blocked(ledger: &Ledger, ticket: &Ticket, phase: Phase) {
        let snapshot = ledger.snapshot(ticket).unwrap();
        assert_eq!(snapshot.phase, phase);
        assert!(!snapshot.resume_allowed);
        assert!(!snapshot.queue_allowed);
    }

    #[test]
    fn ordered_lifecycle_and_only_current_completed_resumes() {
        let (_dir, mut ledger, ticket) = setup();
        assert!(ledger.running(&ticket).is_err());
        assert!(ledger.snapshot(&ticket).is_err());
        ledger.admit(&ticket).unwrap();
        assert_blocked(&ledger, &ticket, Phase::Admitted);
        assert!(ledger.admit(&ticket).is_err());
        assert!(ledger.running(&ticket).is_err());
        assert!(ledger.complete(&ticket).is_err());
        let next = Ticket {
            run: 2,
            ..ticket.clone()
        };
        assert!(ledger.admit(&next).is_err());
        ledger.launch_intent(&ticket).unwrap();
        assert_blocked(&ledger, &ticket, Phase::LaunchIntent);
        assert!(ledger.launch_intent(&ticket).is_err());
        ledger.running(&ticket).unwrap();
        assert_blocked(&ledger, &ticket, Phase::Running);
        assert!(ledger.interrupted(&ticket).is_err());
        ledger.complete(&ticket).unwrap();
        let snapshot = ledger.snapshot(&ticket).unwrap();
        assert!(snapshot.context_valid && snapshot.resume_allowed && snapshot.queue_allowed);
        assert!(ledger.request_stop(&ticket).is_err());
        assert!(ledger.unknown(&ticket).is_err());
        assert!(ledger.admit(&ticket).is_err());
        assert!(
            ledger
                .admit(&Ticket {
                    generation: 0,
                    run: 99,
                    ..ticket.clone()
                })
                .is_err()
        );
        ledger.admit(&next).unwrap();
        assert_blocked(&ledger, &ticket, Phase::Completed);
        assert!(ledger.complete(&ticket).is_err());
        assert!(ledger.request_stop(&ticket).is_err());
    }

    #[test]
    fn deadlines_are_exclusive_and_checked_across_full_u64_range() {
        let (_dir, mut ledger, ticket) = setup();
        start(&mut ledger, &ticket);
        for (now, ttl) in [(0, 0), (0, 60_001), (u64::MAX, 1), (u64::MAX - 2, 3)] {
            assert!(ledger.request(&ticket, "bad", FP, now, ttl).is_err());
        }
        ledger.request(&ticket, "pending", FP, 100, 10).unwrap();
        assert_eq!(
            ledger.approval(&ticket, "pending", 109).unwrap(),
            ApprovalState::Pending
        );
        assert!(!ledger.decide(&ticket, "pending", FP, true, 110).unwrap());
        assert_eq!(
            ledger.approval(&ticket, "pending", 110).unwrap(),
            ApprovalState::Expired
        );
        ledger.request(&ticket, "grant", FP, 110, 10).unwrap();
        assert!(ledger.decide(&ticket, "grant", FP, true, 119).unwrap());
        assert!(!ledger.consume(&ticket, "grant", FP, 120).unwrap());
        assert_eq!(
            ledger.approval(&ticket, "grant", 120).unwrap(),
            ApprovalState::Expired
        );
        ledger
            .request(&ticket, "last", FP, u64::MAX - 2, 2)
            .unwrap();
        assert!(
            ledger
                .decide(&ticket, "last", FP, true, u64::MAX - 1)
                .unwrap()
        );
        assert!(!ledger.consume(&ticket, "last", FP, u64::MAX).unwrap());
        assert_eq!(
            ledger.approval(&ticket, "last", u64::MAX).unwrap(),
            ApprovalState::Expired
        );
    }

    #[test]
    fn exact_scope_duplicate_and_late_decisions_fail_closed() {
        let (_dir, mut ledger, ticket) = setup();
        assert!(ledger.request(&ticket, "id", FP, 0, 100).is_err());
        start(&mut ledger, &ticket);
        ledger.request(&ticket, "id", FP, 10, 100).unwrap();
        assert!(ledger.request(&ticket, "id", OTHER_FP, 10, 100).is_err());
        assert!(!ledger.consume(&ticket, "id", FP, 10).unwrap());
        assert!(!ledger.decide(&ticket, "id", FP, true, 9).unwrap());
        assert!(!ledger.decide(&ticket, "wrong", FP, true, 10).unwrap());
        assert!(!ledger.decide(&ticket, "id", OTHER_FP, true, 10).unwrap());
        for malformed in ["", "wrong", "payload text"] {
            assert!(!ledger.decide(&ticket, "id", malformed, true, 10).unwrap());
            assert!(!ledger.consume(&ticket, "id", malformed, 10).unwrap());
            assert!(!ledger.decide(&ticket, malformed, FP, true, 10).unwrap());
        }
        for wrong in [
            Ticket {
                session: "other".into(),
                ..ticket.clone()
            },
            Ticket {
                generation: 2,
                ..ticket.clone()
            },
            Ticket {
                run: 2,
                ..ticket.clone()
            },
        ] {
            assert!(!ledger.decide(&wrong, "id", FP, true, 10).unwrap());
            assert!(!ledger.consume(&wrong, "id", FP, 10).unwrap());
            assert!(ledger.approval(&wrong, "id", 10).is_err());
        }
        assert!(ledger.decide(&ticket, "id", FP, true, 10).unwrap());
        assert!(!ledger.decide(&ticket, "id", FP, false, 10).unwrap());
        assert!(!ledger.consume(&ticket, "wrong", FP, 10).unwrap());
        assert!(!ledger.consume(&ticket, "id", OTHER_FP, 10).unwrap());
        assert!(ledger.consume(&ticket, "id", FP, 10).unwrap());
        assert!(!ledger.consume(&ticket, "id", FP, 10).unwrap());
        ledger.request(&ticket, "denied", FP, 10, 100).unwrap();
        assert!(ledger.decide(&ticket, "denied", FP, false, 10).unwrap());
        assert!(!ledger.consume(&ticket, "denied", FP, 10).unwrap());
        assert!(!ledger.decide(&ticket, "denied", FP, true, 10).unwrap());
        assert_eq!(
            ledger.approval(&ticket, "denied", 10).unwrap(),
            ApprovalState::Denied
        );
        ledger.complete(&ticket).unwrap();
        start(
            &mut ledger,
            &Ticket {
                generation: 2,
                run: 0,
                ..ticket.clone()
            },
        );
        assert!(!ledger.consume(&ticket, "id", FP, 10).unwrap());
        assert!(!ledger.decide(&ticket, "id", FP, true, 10).unwrap());
        assert!(ledger.request(&ticket, "late", FP, 10, 100).is_err());
    }

    #[test]
    fn stop_revokes_pending_and_granted_but_preserves_consumed() {
        let (_dir, mut ledger, ticket) = setup();
        start(&mut ledger, &ticket);
        for id in ["pending", "granted", "consumed"] {
            ledger.request(&ticket, id, FP, 0, 100).unwrap();
        }
        for id in ["granted", "consumed"] {
            assert!(ledger.decide(&ticket, id, FP, true, 1).unwrap());
        }
        assert!(ledger.consume(&ticket, "consumed", FP, 1).unwrap());
        assert!(ledger.complete(&ticket).is_err());
        ledger.request_stop(&ticket).unwrap();
        ledger.request_stop(&ticket).unwrap();
        assert_blocked(&ledger, &ticket, Phase::Stopping);
        assert!(!ledger.snapshot(&ticket).unwrap().context_valid);
        for id in ["pending", "granted"] {
            assert_eq!(
                ledger.approval(&ticket, id, 1).unwrap(),
                ApprovalState::Revoked
            );
            assert!(!ledger.decide(&ticket, id, FP, true, 1).unwrap());
            assert!(!ledger.consume(&ticket, id, FP, 1).unwrap());
        }
        assert_eq!(
            ledger.approval(&ticket, "consumed", 1000).unwrap(),
            ApprovalState::Consumed
        );
        assert!(!ledger.consume(&ticket, "consumed", FP, 1).unwrap());
        assert!(ledger.complete(&ticket).is_err());
        assert!(
            ledger
                .admit(&Ticket {
                    generation: 99,
                    ..ticket.clone()
                })
                .is_err()
        );
        ledger.interrupted(&ticket).unwrap();
        assert_blocked(&ledger, &ticket, Phase::Interrupted);
        assert!(!ledger.snapshot(&ticket).unwrap().context_valid);
        assert!(
            ledger
                .admit(&Ticket {
                    generation: 99,
                    ..ticket.clone()
                })
                .is_err()
        );
        assert!(ledger.running(&ticket).is_err());
    }

    #[test]
    fn completion_requires_resolved_approvals() {
        let (_dir, mut ledger, ticket) = setup();
        start(&mut ledger, &ticket);
        ledger.request(&ticket, "id", FP, 0, 10).unwrap();
        assert!(ledger.complete(&ticket).is_err());
        ledger.decide(&ticket, "id", FP, true, 1).unwrap();
        assert!(ledger.complete(&ticket).is_err());
        assert_eq!(
            ledger.approval(&ticket, "id", 10).unwrap(),
            ApprovalState::Expired
        );
        ledger.complete(&ticket).unwrap();
        assert!(ledger.snapshot(&ticket).unwrap().resume_allowed);
    }

    #[test]
    fn explicit_recovery_revokes_without_continuation_or_replay() {
        let (dir, mut ledger, running) = setup();
        start(&mut ledger, &running);
        for id in ["pending", "granted", "consumed"] {
            ledger.request(&running, id, FP, 0, 100).unwrap();
        }
        ledger.decide(&running, "granted", FP, true, 1).unwrap();
        ledger.decide(&running, "consumed", FP, true, 1).unwrap();
        ledger.consume(&running, "consumed", FP, 1).unwrap();
        let admitted = ticket("admitted");
        ledger.admit(&admitted).unwrap();
        let launching = ticket("launching");
        ledger.admit(&launching).unwrap();
        ledger.launch_intent(&launching).unwrap();
        let stopping = ticket("stopping");
        start(&mut ledger, &stopping);
        ledger.request_stop(&stopping).unwrap();
        let completed = ticket("completed");
        start(&mut ledger, &completed);
        ledger.complete(&completed).unwrap();
        let interrupted = ticket("interrupted");
        start(&mut ledger, &interrupted);
        ledger.request_stop(&interrupted).unwrap();
        ledger.interrupted(&interrupted).unwrap();
        drop(ledger);
        let mut ledger = Ledger::open(&dir.path().join("ledger.sqlite")).unwrap();
        // Opening alone is observational, NOT automatic recovery or expiration.
        assert_eq!(ledger.snapshot(&running).unwrap().phase, Phase::Running);
        assert_eq!(
            ledger.approval(&running, "granted", 1).unwrap(),
            ApprovalState::Granted
        );
        assert_eq!(ledger.recover().unwrap(), 4);
        assert_eq!(ledger.recover().unwrap(), 0);
        for t in [&running, &admitted, &launching, &stopping] {
            assert_blocked(&ledger, t, Phase::Unknown);
            assert!(!ledger.snapshot(t).unwrap().context_valid);
            assert!(
                ledger
                    .admit(&Ticket {
                        generation: 99,
                        ..t.clone()
                    })
                    .is_err()
            );
            assert!(ledger.running(t).is_err());
            assert!(ledger.complete(t).is_err());
        }
        for id in ["pending", "granted"] {
            assert_eq!(
                ledger.approval(&running, id, 1).unwrap(),
                ApprovalState::Revoked
            );
            assert!(!ledger.decide(&running, id, FP, true, 1).unwrap());
            assert!(!ledger.consume(&running, id, FP, 1).unwrap());
        }
        assert_eq!(
            ledger.approval(&running, "consumed", 1).unwrap(),
            ApprovalState::Consumed
        );
        assert!(ledger.snapshot(&completed).unwrap().resume_allowed);
        assert_blocked(&ledger, &interrupted, Phase::Interrupted);
        drop(ledger);
        let ledger = Ledger::open(&dir.path().join("ledger.sqlite")).unwrap();
        assert_blocked(&ledger, &running, Phase::Unknown);
        assert!(ledger.snapshot(&completed).unwrap().resume_allowed);
    }

    #[test]
    fn unknown_invalidates_and_new_session_is_required() {
        let (_dir, mut ledger, ticket) = setup();
        start(&mut ledger, &ticket);
        ledger.request(&ticket, "id", FP, 0, 100).unwrap();
        ledger.decide(&ticket, "id", FP, true, 1).unwrap();
        ledger.unknown(&ticket).unwrap();
        ledger.unknown(&ticket).unwrap();
        assert_blocked(&ledger, &ticket, Phase::Unknown);
        assert!(!ledger.snapshot(&ticket).unwrap().context_valid);
        assert_eq!(
            ledger.approval(&ticket, "id", 1).unwrap(),
            ApprovalState::Revoked
        );
        assert!(!ledger.consume(&ticket, "id", FP, 1).unwrap());
        assert!(
            ledger
                .admit(&Ticket {
                    generation: u64::MAX,
                    run: u64::MAX,
                    ..ticket.clone()
                })
                .is_err()
        );
        assert!(ledger.interrupted(&ticket).is_err());
        start(
            &mut ledger,
            &Ticket {
                session: "fresh".into(),
                ..ticket
            },
        );
    }

    #[test]
    fn identifiers_fingerprints_and_per_run_rows_are_bounded() {
        let (_dir, mut ledger, ticket) = setup();
        assert!(
            ledger
                .admit(&Ticket {
                    session: "bad\nidentifier".into(),
                    ..ticket.clone()
                })
                .is_err()
        );
        start(&mut ledger, &ticket);
        for id in ["".to_owned(), "x".repeat(129), "payload text".to_owned()] {
            assert!(ledger.request(&ticket, &id, FP, 0, 1).is_err());
        }
        for fingerprint in ["".to_owned(), "f".repeat(65), "g".repeat(64)] {
            assert!(ledger.request(&ticket, "id", &fingerprint, 0, 1).is_err());
        }
        for i in 0..MAX_APPROVALS {
            ledger
                .request(&ticket, &format!("id-{i}"), FP, 0, 1)
                .unwrap();
        }
        assert!(ledger.request(&ticket, "overflow", FP, 0, 1).is_err());
        ledger.approval(&ticket, "id-0", 1).unwrap();
        // Expiration does not erase durable uniqueness or free the audit cap.
        assert!(ledger.request(&ticket, "overflow", FP, 1, 1).is_err());
        ledger.complete(&ticket).unwrap();
        let next = Ticket { run: 2, ..ticket };
        start(&mut ledger, &next);
        ledger.request(&next, "id-0", FP, 1, 1).unwrap();
    }

    #[test]
    fn exclusive_create_existing_only_open_and_exact_schema() {
        let (dir, ledger, _) = setup();
        let path = dir.path().join("ledger.sqlite");
        assert!(Ledger::create(&path).is_err());
        let missing = dir.path().join("missing.sqlite");
        assert!(Ledger::open(&missing).is_err());
        assert!(!missing.exists());
        for connection in [&ledger.connection, &Ledger::open(&path).unwrap().connection] {
            assert_eq!(
                connection
                    .pragma_query_value(None, "synchronous", |r| r.get::<_, i64>(0))
                    .unwrap(),
                2
            );
            assert_eq!(
                connection
                    .pragma_query_value(None, "busy_timeout", |r| r.get::<_, i64>(0))
                    .unwrap(),
                1000
            );
            assert_eq!(
                connection
                    .pragma_query_value(None, "journal_mode", |r| r.get::<_, String>(0))
                    .unwrap(),
                "wal"
            );
        }
        drop(ledger);
        let raw = Connection::open(&path).unwrap();
        for version in [0, 2, 999] {
            raw.pragma_update(None, "user_version", version).unwrap();
            assert!(matches!(Ledger::open(&path), Err(SCHEMA)));
            assert_eq!(
                raw.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                    .unwrap(),
                version
            );
        }
        raw.pragma_update(None, "user_version", 1).unwrap();
        raw.pragma_update(None, "application_id", 0).unwrap();
        assert!(matches!(Ledger::open(&path), Err(SCHEMA)));
        raw.pragma_update(None, "application_id", APPLICATION_ID)
            .unwrap();
        raw.execute_batch("CREATE TABLE unexpected (value TEXT)")
            .unwrap();
        assert!(matches!(Ledger::open(&path), Err(SCHEMA)));
        raw.execute_batch("DROP TABLE unexpected; DROP TABLE approvals")
            .unwrap();
        assert!(matches!(Ledger::open(&path), Err(SCHEMA)));
    }

    #[test]
    fn tickets_support_full_unsigned_range_without_stale_wraparound() {
        let (_dir, mut ledger, ticket) = setup();
        let first = Ticket {
            generation: u64::MAX - 1,
            run: u64::MAX,
            ..ticket
        };
        start(&mut ledger, &first);
        ledger.complete(&first).unwrap();
        let last = Ticket {
            generation: u64::MAX,
            run: 0,
            ..first.clone()
        };
        start(&mut ledger, &last);
        ledger.complete(&last).unwrap();
        assert!(ledger.admit(&first).is_err());
        assert!(ledger.admit(&last).is_err());
        assert!(ledger.snapshot(&last).unwrap().resume_allowed);
        assert!(!ledger.snapshot(&first).unwrap().resume_allowed);
    }

    #[test]
    fn two_connections_race_to_decide_and_consume_at_most_once() {
        let (dir, mut ledger, ticket) = setup();
        start(&mut ledger, &ticket);
        for i in 0..16 {
            let id = format!("race-{i}");
            ledger.request(&ticket, &id, FP, 0, 100).unwrap();
            let barrier = Arc::new(Barrier::new(2));
            let handles: Vec<_> = (0..2)
                .map(|_| {
                    let mut other = Ledger::open(&dir.path().join("ledger.sqlite")).unwrap();
                    let ticket = ticket.clone();
                    let id = id.clone();
                    let barrier = barrier.clone();
                    thread::spawn(move || {
                        barrier.wait();
                        let decided = other.decide(&ticket, &id, FP, true, 1).unwrap();
                        barrier.wait();
                        let consumed = other.consume(&ticket, &id, FP, 1).unwrap();
                        (decided as usize, consumed as usize)
                    })
                })
                .collect();
            let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
            assert_eq!(results.iter().map(|r| r.0).sum::<usize>(), 1);
            assert_eq!(results.iter().map(|r| r.1).sum::<usize>(), 1);
            assert_eq!(
                ledger.approval(&ticket, &id, 1).unwrap(),
                ApprovalState::Consumed
            );
        }
    }
}
