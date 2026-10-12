//! Plan persistence: one plan per session (`plans`), its tasks as JSON bodies with indexed
//! status/archival/position (`plan_tasks`), an append-only change log (`plan_events`) and
//! idempotent receipts by `(session, key)` (`plan_receipts`). The rules live in
//! [`crate::plans`]; this module only loads a state, applies the pure result and writes it
//! back inside the caller's transaction.

use pipkin_engine_protocol::{PlanOp, PlanReceipt, PlanRejected, PlanTaskView, PlanView};
use rusqlite::{Connection, OptionalExtension, params};

use super::{Result, Store, StoreError};
use crate::plans::{self, PlanState};

impl Store {
    /// The whole plan (live and archived tasks in display order).
    pub fn plan_state(c: &Connection, session: &str) -> Result<PlanState> {
        let head = c
            .query_row(
                "SELECT revision, next_task FROM plans WHERE session = ?1",
                [session],
                |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64)),
            )
            .optional()?;
        let Some((revision, next_id)) = head else {
            return Ok(PlanState::default());
        };
        let mut stmt =
            c.prepare("SELECT body FROM plan_tasks WHERE session = ?1 ORDER BY position, id")?;
        let tasks = stmt
            .query_map([session], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(|body| {
                serde_json::from_str::<PlanTaskView>(&body)
                    .map_err(|e| StoreError::Corrupt(format!("plan task body: {e}")))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(PlanState {
            revision,
            next_id,
            tasks,
        })
    }

    pub fn plan_view(c: &Connection, session: &str, include_archived: bool) -> Result<PlanView> {
        Ok(plans::view(
            &Self::plan_state(c, session)?,
            include_archived,
        ))
    }

    /// The recorded outcome of an earlier batch under `key`.
    pub fn plan_receipt(
        c: &Connection,
        session: &str,
        key: &str,
    ) -> Result<Option<std::result::Result<PlanReceipt, PlanRejected>>> {
        let body: Option<String> = c
            .query_row(
                "SELECT body FROM plan_receipts WHERE session = ?1 AND key = ?2",
                params![session, key],
                |r| r.get(0),
            )
            .optional()?;
        let Some(body) = body else {
            return Ok(None);
        };
        let value: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| StoreError::Corrupt(format!("plan receipt: {e}")))?;
        if let Some(ok) = value.get("ok") {
            return Ok(Some(Ok(serde_json::from_value(ok.clone()).map_err(
                |e| StoreError::Corrupt(format!("plan receipt: {e}")),
            )?)));
        }
        if let Some(rejected) = value.get("rejected") {
            return Ok(Some(Err(serde_json::from_value(rejected.clone())
                .map_err(|e| {
                    StoreError::Corrupt(format!("plan receipt: {e}"))
                })?)));
        }
        Err(StoreError::Corrupt(
            "plan receipt has neither ok nor rejected".into(),
        ))
    }

    /// Apply a batch under the idempotency `key`. A key seen before returns its recorded
    /// outcome unchanged. Returns the outcome and whether the plan changed in this call.
    pub fn apply_plan(
        tx: &Connection,
        session: &str,
        key: &str,
        ops: &[PlanOp],
        expected_revision: Option<u64>,
        now: i64,
    ) -> Result<(std::result::Result<PlanReceipt, PlanRejected>, bool)> {
        Self::assert_session_live(tx, session)?;
        if let Some(recorded) = Self::plan_receipt(tx, session, key)? {
            return Ok((recorded, false));
        }
        let state = Self::plan_state(tx, session)?;
        let outcome = plans::apply(&state, ops, expected_revision, now);
        let body = match &outcome {
            Ok(applied) => serde_json::json!({ "ok": applied.receipt }),
            Err(rejected) => serde_json::json!({ "rejected": rejected }),
        };
        tx.execute(
            "INSERT INTO plan_receipts(session, key, body) VALUES (?1, ?2, ?3)",
            params![session, key, body.to_string()],
        )?;
        let applied = match outcome {
            Ok(applied) => applied,
            Err(rejected) => return Ok((Err(rejected), false)),
        };
        tx.execute(
            "INSERT INTO plans(session, revision, next_task, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(session) DO UPDATE SET revision = excluded.revision,
                next_task = excluded.next_task, updated_at = excluded.updated_at",
            params![
                session,
                applied.state.revision as i64,
                applied.state.next_id as i64,
                now
            ],
        )?;
        let changed: std::collections::HashSet<u64> =
            applied.receipt.changed.iter().copied().collect();
        for (position, task) in applied.state.tasks.iter().enumerate() {
            if changed.contains(&task.id) {
                let body = serde_json::to_string(task).expect("serializes");
                tx.execute(
                    "INSERT INTO plan_tasks(session, id, position, body, archived, status)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(session, id) DO UPDATE SET position = excluded.position,
                        body = excluded.body, archived = excluded.archived,
                        status = excluded.status",
                    params![
                        session,
                        task.id as i64,
                        position as i64,
                        body,
                        task.archived as i64,
                        task.status.as_str()
                    ],
                )?;
            } else {
                tx.execute(
                    "UPDATE plan_tasks SET position = ?3 WHERE session = ?1 AND id = ?2
                     AND position != ?3",
                    params![session, task.id as i64, position as i64],
                )?;
            }
        }
        let mut seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM plan_events WHERE session = ?1",
            [session],
            |r| r.get(0),
        )?;
        for event in &applied.events {
            seq += 1;
            tx.execute(
                "INSERT INTO plan_events(session, seq, at_ms, revision, task, kind, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    session,
                    seq,
                    now,
                    applied.state.revision as i64,
                    event.task as i64,
                    event.kind,
                    event.data.to_string()
                ],
            )?;
        }
        Ok((Ok(applied.receipt), true))
    }

    /// The newest `limit` change-log rows of a plan, newest first, as
    /// `{seq, atMs, revision, task, kind, data}`.
    pub fn plan_events(
        c: &Connection,
        session: &str,
        limit: u32,
    ) -> Result<Vec<serde_json::Value>> {
        let mut stmt = c.prepare(
            "SELECT seq, at_ms, revision, task, kind, data FROM plan_events
             WHERE session = ?1 ORDER BY seq DESC LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![session, limit as i64], |r| {
                let data: String = r.get(5)?;
                Ok(serde_json::json!({
                    "seq": r.get::<_, i64>(0)?,
                    "atMs": r.get::<_, i64>(1)?,
                    "revision": r.get::<_, i64>(2)?,
                    "task": r.get::<_, i64>(3)?,
                    "kind": r.get::<_, String>(4)?,
                    "data": serde_json::from_str::<serde_json::Value>(&data).unwrap_or(serde_json::Value::Null),
                }))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}
