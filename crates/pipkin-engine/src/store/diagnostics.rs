//! Bounded structured diagnostics (plan §12, OBS-1): one row per engine fact about a
//! session's work, numbered per session, written in the transaction that commits the fact
//! wherever one exists. Each session keeps its newest [`MAX_PER_SESSION`] rows; older ones
//! are dropped as new ones arrive. Rows are read newest first by cursor, or all at once for
//! an export.

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use super::{Result, Store};

/// Rows kept per session.
pub const MAX_PER_SESSION: u32 = 2_000;

#[derive(Clone, Debug, PartialEq)]
pub struct DiagRow {
    pub session: String,
    pub seq: u64,
    pub at_ms: i64,
    pub kind: String,
    pub run: Option<u64>,
    pub request: Option<String>,
    pub call_id: Option<String>,
    pub record: Value,
}

pub fn row(r: &rusqlite::Row) -> rusqlite::Result<DiagRow> {
    let record: String = r.get("record")?;
    Ok(DiagRow {
        session: r.get("session")?,
        seq: r.get::<_, i64>("seq")? as u64,
        at_ms: r.get("at")?,
        kind: r.get("kind")?,
        run: r.get::<_, Option<i64>>("run")?.map(|n| n as u64),
        request: r.get("request")?,
        call_id: r.get("call_id")?,
        record: serde_json::from_str(&record).unwrap_or(Value::Null),
    })
}

impl Store {
    /// Append one record and drop the session's oldest beyond the bound.
    #[allow(clippy::too_many_arguments)]
    pub fn diag_append(
        tx: &Connection,
        session: &str,
        at_ms: i64,
        kind: &str,
        run: Option<u64>,
        request: Option<&str>,
        call_id: Option<&str>,
        record: &Value,
    ) -> Result<u64> {
        let next: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM diagnostics WHERE session = ?1",
            [session],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO diagnostics(session, seq, at, kind, run, request, call_id, record)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                session,
                next,
                at_ms,
                kind,
                run.map(|r| r as i64),
                request,
                call_id,
                record.to_string()
            ],
        )?;
        tx.execute(
            "DELETE FROM diagnostics WHERE session = ?1 AND seq <= ?2 - ?3",
            params![session, next, MAX_PER_SESSION as i64],
        )?;
        Ok(next as u64)
    }

    /// Newest first, strictly before `before` (a seq), at most `limit`; the flag says older
    /// rows remain.
    pub fn diagnostics_page(
        c: &Connection,
        session: &str,
        before: Option<u64>,
        limit: u32,
    ) -> Result<(Vec<DiagRow>, bool)> {
        let before = before.map_or(i64::MAX, |b| b as i64);
        let mut stmt = c.prepare_cached(
            "SELECT * FROM diagnostics WHERE session = ?1 AND seq < ?2
             ORDER BY seq DESC LIMIT ?3",
        )?;
        let rows = stmt
            .query_map(params![session, before, limit as i64 + 1], row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let has_older = rows.len() > limit as usize;
        Ok((rows.into_iter().take(limit as usize).collect(), has_older))
    }

    /// Every retained row, oldest first (an export).
    pub fn diagnostics_all(c: &Connection, session: &str) -> Result<Vec<DiagRow>> {
        let mut stmt =
            c.prepare_cached("SELECT * FROM diagnostics WHERE session = ?1 ORDER BY seq")?;
        let rows = stmt
            .query_map([session], row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn diagnostics_count(c: &Connection, session: &str) -> Result<u64> {
        Ok(c.query_row(
            "SELECT COUNT(*) FROM diagnostics WHERE session = ?1",
            [session],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .unwrap_or(0) as u64)
    }
}
