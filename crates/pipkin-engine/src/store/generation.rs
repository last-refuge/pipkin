//! Checkpoints for whole-generation retries, separate from provider HTTP retries.

use rusqlite::{Connection, OptionalExtension, params};

use super::{Result, Store};

#[derive(Clone, Debug, Default)]
pub struct GenerationState {
    /// Consecutive failed responses since the last successful generation in this run.
    pub failures: u32,
    /// The error entry already classified, so recovery cannot count it twice.
    pub handled_error: Option<u64>,
    /// Absolute Unix milliseconds. None on a handled error means terminal (no retry).
    pub retry_until: Option<i64>,
    pub overflow_compacted: bool,
}

impl Store {
    pub fn generation_state(c: &Connection, run: u64) -> Result<GenerationState> {
        Ok(c.query_row(
            "SELECT failures, handled_error, retry_until, overflow_compacted FROM generation_state WHERE run = ?1",
            [run as i64],
            |r| Ok(GenerationState {
                failures: r.get(0)?, handled_error: r.get(1)?,
                retry_until: r.get(2)?, overflow_compacted: r.get(3)?,
            }),
        ).optional()?.unwrap_or_default())
    }

    pub fn save_generation_state(c: &Connection, run: u64, state: &GenerationState) -> Result<()> {
        c.execute(
            "INSERT INTO generation_state(run, failures, handled_error, retry_until, overflow_compacted)
             VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(run) DO UPDATE SET
             failures = excluded.failures, handled_error = excluded.handled_error,
             retry_until = excluded.retry_until, overflow_compacted = excluded.overflow_compacted",
            params![run as i64, state.failures, state.handled_error, state.retry_until, state.overflow_compacted],
        )?;
        Ok(())
    }

    pub fn reset_generation_state(c: &Connection, run: u64) -> Result<()> {
        c.execute("DELETE FROM generation_state WHERE run = ?1", [run as i64])?;
        Ok(())
    }
}
