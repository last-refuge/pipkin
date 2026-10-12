//! Goal persistence (schema 14): at most one current (unarchived) goal per session, archived
//! history, an append-only event log, and migration receipts in `kv`.

use pipkin_engine_protocol::{GoalState, GoalView};
use rusqlite::{Connection, OptionalExtension, params};

use super::{Result, Store};

const COLUMNS: &str = "id, objective, revision, state, reason, epoch, turns, last_request, \
    provenance, archived, created_at, updated_at";

fn view_from_row(r: &rusqlite::Row) -> rusqlite::Result<(GoalView, bool)> {
    let state: String = r.get("state")?;
    Ok((
        GoalView {
            id: r.get::<_, i64>("id")? as u64,
            objective: r.get("objective")?,
            revision: r.get::<_, i64>("revision")? as u64,
            state: GoalState::parse(&state).unwrap_or(GoalState::Paused),
            reason: r.get("reason")?,
            turns: r.get::<_, i64>("turns")? as u64,
            epoch: r.get::<_, i64>("epoch")? as u64,
            provenance: r.get("provenance")?,
            last_request: r.get("last_request")?,
            created_at_ms: r.get("created_at")?,
            updated_at_ms: r.get("updated_at")?,
        },
        r.get::<_, i64>("archived")? != 0,
    ))
}

impl Store {
    pub fn goal_current(c: &Connection, session: &str) -> Result<Option<GoalView>> {
        Ok(c.query_row(
            &format!("SELECT {COLUMNS} FROM goals WHERE session = ?1 AND archived = 0"),
            [session],
            view_from_row,
        )
        .optional()?
        .map(|(view, _)| view))
    }

    pub fn goal_by_id(c: &Connection, session: &str, id: u64) -> Result<Option<GoalView>> {
        Ok(c.query_row(
            &format!("SELECT {COLUMNS} FROM goals WHERE session = ?1 AND id = ?2"),
            params![session, id as i64],
            view_from_row,
        )
        .optional()?
        .map(|(view, _)| view))
    }

    /// Archived goals, newest first.
    pub fn goal_history(c: &Connection, session: &str, limit: u32) -> Result<Vec<GoalView>> {
        let mut stmt = c.prepare(&format!(
            "SELECT {COLUMNS} FROM goals WHERE session = ?1 AND archived = 1 ORDER BY id DESC LIMIT ?2"
        ))?;
        let rows = stmt
            .query_map(params![session, limit as i64], view_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows.into_iter().map(|(v, _)| v).collect())
    }

    /// Insert a new current goal, archiving the previous one (as cancelled, with `replaced`
    /// as its reason) when there is one.
    pub fn goal_insert(
        tx: &Connection,
        session: &str,
        objective: &str,
        state: GoalState,
        reason: Option<&str>,
        provenance: &str,
        replaced: &str,
        now: i64,
    ) -> Result<GoalView> {
        Self::assert_session_live(tx, session)?;
        if let Some(current) = Self::goal_current(tx, session)? {
            let state = if current.state == GoalState::Active {
                GoalState::Cancelled
            } else {
                current.state
            };
            tx.execute(
                "UPDATE goals SET archived = 1, state = ?3, reason = COALESCE(?4, reason), updated_at = ?5
                 WHERE session = ?1 AND id = ?2",
                params![
                    session,
                    current.id as i64,
                    state.as_str(),
                    (current.state == GoalState::Active).then_some(replaced),
                    now
                ],
            )?;
            Self::goal_event(
                tx,
                session,
                current.id,
                "archived",
                &serde_json::json!({"reason": replaced}),
                now,
            )?;
        }
        let id: i64 = tx.query_row(
            "SELECT COALESCE(MAX(id), 0) + 1 FROM goals WHERE session = ?1",
            [session],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO goals(session, id, objective, revision, state, reason, epoch, turns,
                last_request, provenance, archived, created_at, updated_at)
             VALUES (?1, ?2, ?3, 1, ?4, ?5, 0, 0, NULL, ?6, 0, ?7, ?7)",
            params![
                session,
                id,
                objective,
                state.as_str(),
                reason,
                provenance,
                now
            ],
        )?;
        Self::goal_event(
            tx,
            session,
            id as u64,
            "created",
            &serde_json::json!({"state": state.as_str(), "provenance": provenance}),
            now,
        )?;
        Ok(Self::goal_by_id(tx, session, id as u64)?.expect("just inserted"))
    }

    /// Change the current goal's state (and reason).
    pub fn goal_set_state(
        tx: &Connection,
        session: &str,
        id: u64,
        state: GoalState,
        reason: Option<&str>,
        now: i64,
    ) -> Result<()> {
        tx.execute(
            "UPDATE goals SET state = ?3, reason = ?4, updated_at = ?5
             WHERE session = ?1 AND id = ?2 AND archived = 0",
            params![session, id as i64, state.as_str(), reason, now],
        )?;
        Self::goal_event(
            tx,
            session,
            id,
            "state",
            &serde_json::json!({"state": state.as_str(), "reason": reason}),
            now,
        )
    }

    /// Record the next scheduled turn before it is admitted: epoch and turns advance, the
    /// request key is remembered. Conditional on the goal still being current, active and
    /// at `expected_epoch`: a tick that lost a race (archive, replacement, pause, another
    /// tick) schedules nothing and returns `None`.
    pub fn goal_schedule_turn(
        tx: &Connection,
        session: &str,
        id: u64,
        expected_epoch: u64,
        key: &str,
        now: i64,
    ) -> Result<Option<GoalView>> {
        let changed = tx.execute(
            "UPDATE goals SET epoch = epoch + 1, turns = turns + 1, last_request = ?3,
                reason = NULL, updated_at = ?4
             WHERE session = ?1 AND id = ?2 AND archived = 0 AND state = 'active' AND epoch = ?5",
            params![session, id as i64, key, now, expected_epoch as i64],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        Self::goal_event(
            tx,
            session,
            id,
            "scheduled",
            &serde_json::json!({"request": key}),
            now,
        )?;
        Self::goal_by_id(tx, session, id)
    }

    pub fn goal_revise(
        tx: &Connection,
        session: &str,
        id: u64,
        objective: &str,
        now: i64,
    ) -> Result<()> {
        tx.execute(
            "UPDATE goals SET objective = ?3, revision = revision + 1, state = 'active', reason = NULL,
                updated_at = ?4 WHERE session = ?1 AND id = ?2",
            params![session, id as i64, objective, now],
        )?;
        Self::goal_event(
            tx,
            session,
            id,
            "revised",
            &serde_json::json!({"objective": objective}),
            now,
        )
    }

    pub fn goal_archive(
        tx: &Connection,
        session: &str,
        id: u64,
        reason: &str,
        now: i64,
    ) -> Result<()> {
        tx.execute(
            "UPDATE goals SET archived = 1,
                state = CASE WHEN state = 'active' THEN 'cancelled' ELSE state END,
                reason = CASE WHEN state = 'active' THEN ?3 ELSE reason END,
                updated_at = ?4
             WHERE session = ?1 AND id = ?2",
            params![session, id as i64, reason, now],
        )?;
        Self::goal_event(
            tx,
            session,
            id,
            "archived",
            &serde_json::json!({"reason": reason}),
            now,
        )
    }

    /// Every active goal becomes paused (engine start: nothing continues without an explicit
    /// resume). Returns the sessions touched.
    pub fn pause_active_goals(&self, reason: &str, now: i64) -> Result<Vec<String>> {
        self.tx(|tx| {
            let mut stmt = tx
                .prepare("SELECT session, id FROM goals WHERE archived = 0 AND state = 'active'")?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(stmt);
            let mut sessions = Vec::new();
            for (session, id) in rows {
                Self::goal_set_state(tx, &session, id, GoalState::Paused, Some(reason), now)?;
                Self::bump(tx, &session, now)?;
                sessions.push(session);
            }
            Ok(sessions)
        })
    }

    fn goal_event(
        tx: &Connection,
        session: &str,
        goal: u64,
        kind: &str,
        data: &serde_json::Value,
        now: i64,
    ) -> Result<()> {
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM goal_events WHERE session = ?1",
            [session],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO goal_events(session, seq, at_ms, goal, kind, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![session, seq, now, goal as i64, kind, data.to_string()],
        )?;
        Ok(())
    }

    /// Migration receipts: the key of an imported desktop goal, recorded once.
    pub fn goal_migration_seen(c: &Connection, session: &str, key: &str) -> Result<bool> {
        Ok(c.query_row(
            "SELECT 1 FROM kv WHERE session = ?1 AND key = ?2",
            params![session, format!("goal_migration:{key}")],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
    }

    pub fn goal_migration_record(
        tx: &Connection,
        session: &str,
        key: &str,
        now: i64,
    ) -> Result<()> {
        Self::kv_set(
            tx,
            session,
            &format!("goal_migration:{key}"),
            &now.to_string(),
        )
    }
}
