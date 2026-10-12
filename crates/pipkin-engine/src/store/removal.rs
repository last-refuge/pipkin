//! Durable removal intent and atomic root/subtree cleanup. No project/source files are touched.
use super::*;

impl Store {
    fn removal_members_in(c: &Connection, root: &str) -> Result<Vec<String>> {
        Ok(c.prepare("SELECT id FROM sessions WHERE id=?1 OR root=?1")?
            .query_map([root], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn removal_tool_intents(&self, root: &str) -> Result<Vec<(String, String)>> {
        self.read(|c| Ok(c.prepare("SELECT session, call_id FROM tool_calls WHERE state='intent' AND session IN (SELECT id FROM sessions WHERE id=?1 OR root=?1)")?.query_map([root], |r| Ok((r.get(0)?,r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?))
    }
    pub fn assert_session_live(c: &Connection, session: &str) -> Result<()> {
        let removed: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_removals WHERE root = ?1 OR root =
             (SELECT root FROM sessions WHERE id = ?1))",
            [session],
            |r| r.get(0),
        )?;
        if removed {
            Err(StoreError::Removed)
        } else {
            Ok(())
        }
    }

    /// None means unknown; Some(false) is pending and Some(true) already completed.
    pub fn removal_state(&self, root: &str) -> Result<Option<bool>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT complete FROM session_removals WHERE root = ?1",
                [root],
                |r| r.get(0),
            )
            .optional()?)
        })
    }

    pub fn begin_removal(&self, root: &str) -> Result<Vec<String>> {
        self.tx(|tx| {
            if tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_removals WHERE root=?1)",
                [root],
                |r| r.get::<_, bool>(0),
            )? {
                return Self::removal_members_in(tx, root);
            }
            let row = tx
                .query_row(
                    "SELECT parent, create_key FROM sessions WHERE id=?1",
                    [root],
                    |r| {
                        Ok((
                            r.get::<_, Option<String>>(0)?,
                            r.get::<_, Option<String>>(1)?,
                        ))
                    },
                )
                .optional()?;
            let Some((parent, key)) = row else {
                return Err(StoreError::Removed);
            };
            if parent.is_some() {
                return Err(StoreError::Corrupt(
                    "only a root session can be removed".into(),
                ));
            }
            tx.execute(
                "INSERT INTO session_removals(root,create_key,requested_at) VALUES (?1,?2,?3)",
                params![root, key, now_ms()],
            )?;
            Self::removal_members_in(tx, root)
        })
    }

    pub fn pending_removals(&self) -> Result<Vec<String>> {
        self.read(|c| {
            Ok(c.prepare(
                "SELECT root FROM session_removals WHERE complete=0 ORDER BY requested_at",
            )?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Caller has stopped all live work. One transaction drops the whole group, emits
    /// invalidations via deletion triggers and retains only its idempotent removal receipt.
    pub fn finish_removal(&self, root: &str) -> Result<Vec<String>> {
        self.tx(|tx| {
            let pending: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM session_removals WHERE root=?1 AND complete=0)", [root], |r| r.get(0))?;
            if !pending { return Ok(vec![]); }
            let group = "SELECT id FROM sessions WHERE id=?1 OR root=?1";
            let affinities = tx.prepare(&format!("SELECT value FROM kv WHERE key='provider_session_id' AND session IN ({group})"))?
                .query_map([root], |r| r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            for table in ["summary_writes", "compactions"] {
                tx.execute(&format!("DELETE FROM {table} WHERE session IN ({group})"),[root])?;
            }
            for table in ["tool_boundaries", "generation_state"] {
                tx.execute(&format!("DELETE FROM {table} WHERE run IN (SELECT id FROM runs WHERE session IN ({group}))"),[root])?;
            }
            tx.execute("DELETE FROM subagent_tasks WHERE root=?1",[root])?;
            for table in ["entry_chunks","entries","requests","tool_outputs","tool_calls","usage","kv","runs","plan_tasks","plan_events","plan_receipts","plans"] {
                tx.execute(&format!("DELETE FROM {table} WHERE session IN ({group})"),[root])?;
            }
            tx.execute(&format!("DELETE FROM import_map WHERE native_session IN ({group})"),[root])?;
            tx.execute("DELETE FROM sessions WHERE id=?1 OR root=?1",[root])?;
            tx.execute("UPDATE session_removals SET complete=1 WHERE root=?1",[root])?;
            Ok(affinities)
        })
    }
}
