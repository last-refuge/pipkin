//! Durable passive writes produced by built-in compaction. They survive Stop and are
//! placed before queued user inputs at a tool/final boundary or an idle admission.
use super::*;

impl Store {
    pub fn admit_summary_write(
        c: &Connection,
        session: &str,
        compaction: u64,
        kind: &EntryKind,
    ) -> Result<u64> {
        c.execute("INSERT INTO summary_writes(session, compaction, state, body) VALUES (?1, ?2, 'queued', ?3) ON CONFLICT(compaction) DO NOTHING",
            params![session, compaction, json(kind)])?;
        Ok(c.query_row(
            "SELECT id FROM summary_writes WHERE compaction = ?1 AND session = ?2",
            params![compaction, session],
            |r| r.get(0),
        )?)
    }

    /// Writes precede selected user items, regardless of admission order. Refresh the head
    /// after each placement so later writes in this SAME boundary can become stale.
    /// Equality is allowed by the reference: only a cut before the current range is stale.
    pub fn place_summary_writes(c: &Connection, session: &str, now: i64) -> Result<Vec<Entry>> {
        let mut entries = Vec::new();
        let mut head = Self::last_head(c, session)?.map(|(_, cut)| cut);
        loop {
            let rows = {
                let mut q = c.prepare("SELECT id, body FROM summary_writes WHERE session = ?1 AND state = 'queued' ORDER BY id LIMIT 64")?;
                q.query_map([session], |r| {
                    Ok((r.get::<_, u64>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
            };
            if rows.is_empty() {
                break;
            }
            for (id, raw) in rows {
                let kind: EntryKind = parse_json(&raw, "queued summary")?;
                let EntryKind::Compaction { first_kept, .. } = &kind else {
                    return Err(StoreError::Corrupt(
                        "queued summary is not a compaction".into(),
                    ));
                };
                if head.is_some_and(|head| *first_kept < head) {
                    c.execute(
                        "UPDATE summary_writes SET state = 'stale', body = NULL WHERE id = ?1",
                        [id],
                    )?;
                    continue;
                }
                let entry = Self::append_entry(c, session, now, None, &kind)?;
                head = Some(*first_kept);
                c.execute("UPDATE summary_writes SET state = 'placed', entry = ?2, body = NULL WHERE id = ?1", params![id, entry.id])?;
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    /// Bound accepted non-blocking summaries before contacting the provider. Moving an
    /// accepted operation into the write queue consumes the same slot, not another one.
    pub fn pending_summaries(c: &Connection, session: &str) -> Result<u64> {
        Ok(c.query_row("SELECT (SELECT COUNT(*) FROM compactions WHERE session = ?1 AND scope != 'blocking' AND state != 'settled') + (SELECT COUNT(*) FROM summary_writes WHERE session = ?1 AND state = 'queued')", [session], |r| r.get(0))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::compactions::{CompactionOutcome, CompactionProgress, CompactionScope};

    fn setup(s: &Store) {
        s.create_session("s", "/tmp", None, &Origin::Native, None, None, None, 1)
            .unwrap();
        s.tx(|c| {
            for _ in 0..4 {
                Store::append_entry(
                    c,
                    "s",
                    1,
                    None,
                    &EntryKind::Message {
                        message: Message::user_text("old context", 1),
                        mode: InputMode::Prompt,
                        request: None,
                        partial: false,
                    },
                )?;
            }
            Ok(())
        })
        .unwrap();
    }
    fn write(c: &Connection, key: &str, cut: u64) -> Result<u64> {
        let job = Store::admit_compaction(c, "s", key, None, CompactionScope::Manual, None)?;
        let kind = EntryKind::Compaction {
            summary: key.into(),
            first_kept: cut,
            tokens_before: 100,
        };
        let id = Store::admit_summary_write(c, "s", job.id, &kind)?;
        assert_eq!(Store::admit_summary_write(c, "s", job.id, &kind)?, id);
        Store::save_compaction(
            c,
            &job,
            &CompactionProgress::Terminal {
                result: CompactionOutcome::Submitted { submission: id },
            },
        )?;
        Ok(id)
    }

    #[test]
    fn one_boundary_keeps_equal_heads_and_rejects_only_backward_writes() {
        let s = Store::memory().unwrap();
        setup(&s);
        s.tx(|c| {
            write(c, "first", 3)?;
            write(c, "equal", 3)?;
            write(c, "backward", 2)?;
            write(c, "forward", 4)?;
            let placed = Store::place_summary_writes(c, "s", 2)?;
            let reference: serde_json::Value = serde_json::from_str(include_str!(
                "../../../../fixtures/native-queue/inbox-reference.json"
            ))
            .unwrap();
            let expected: Vec<_> = reference["cases"][0]["placed"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|e| e["summary"].as_str())
                .collect();
            assert_eq!(
                placed
                    .iter()
                    .filter_map(|e| match &e.kind {
                        EntryKind::Compaction { summary, .. } => Some(summary.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
                expected
            );
            assert!(Store::place_summary_writes(c, "s", 3)?.is_empty());
            let stale: u64 = c.query_row(
                "SELECT COUNT(*) FROM summary_writes WHERE state = 'stale'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(stale, 1);
            assert_eq!(Store::pending_summaries(c, "s")?, 0);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn queued_write_reopens_and_failed_placement_rolls_back_the_head_and_identity() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        setup(&s);
        let id = s.tx(|c| write(c, "durable", 3)).unwrap();
        drop(s);
        let s = Store::open(dir.path()).unwrap();
        let failed: Result<()> = s.tx(|c| {
            assert_eq!(Store::place_summary_writes(c, "s", 2)?.len(), 1);
            Err(StoreError::Injected)
        });
        assert!(failed.is_err());
        assert!(s.read(|c| Store::last_head(c, "s")).unwrap().is_none());
        assert_eq!(s.read(|c| Store::pending_summaries(c, "s")).unwrap(), 1);
        s.tx(|c| {
            assert_eq!(write(c, "durable", 3)?, id);
            assert_eq!(Store::place_summary_writes(c, "s", 3)?.len(), 1);
            Ok(())
        })
        .unwrap();
        assert_eq!(s.read(|c| Store::pending_summaries(c, "s")).unwrap(), 0);
    }

    #[test]
    fn v7_upgrade_preserves_history_and_marks_only_tool_boundaries_already_crossed() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        setup(&s);
        let (run, crossed, pending) = s.tx(|c| {
            let run = Store::start_run(c, "s", None, 1)?;
            let assistant = |reason, partial| EntryKind::Message {
                message: Message::Assistant { content: vec![], api:"scripted".into(), provider:"scripted".into(), model:"scripted".into(), usage:Default::default(), stop_reason:reason, error_message:None, timestamp:1, extra:Default::default() },
                mode: InputMode::Prompt, request:None, partial,
            };
            let crossed = Store::append_entry(c, "s", 1, Some(run), &assistant(StopReason::ToolUse, false))?.id;
            Store::append_entry(c, "s", 2, Some(run), &assistant(StopReason::Stop, true))?;
            let pending = Store::append_entry(c, "s", 3, Some(run), &assistant(StopReason::ToolUse, false))?.id;
            c.execute_batch("DROP TABLE jobs; DROP TABLE goal_events; DROP TABLE goals; ALTER TABLE subagent_tasks DROP COLUMN launch; DROP TABLE web_sources; DROP TABLE plan_receipts; DROP TABLE plan_events; DROP TABLE plan_tasks; DROP TABLE plans; DROP TABLE tool_call_ids; DROP TRIGGER event_session_delete; DROP TRIGGER removed_session_insert; DROP TABLE session_removals; DROP TABLE summary_writes; DROP TABLE tool_boundaries; PRAGMA user_version = 7")?;
            Ok((run, crossed, pending))
        }).unwrap();
        let history = s.page("s", None, 100).unwrap().0;
        drop(s);
        let s = Store::open(dir.path()).unwrap();
        assert_eq!(s.page("s", None, 100).unwrap().0, history);
        s.read(|c| {
            for (id, expected) in [(crossed, true), (pending, false)] {
                let found: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM tool_boundaries WHERE run = ?1 AND assistant = ?2)", params![run, id], |r| r.get(0))?;
                assert_eq!(found, expected);
            }
            Ok(())
        }).unwrap();
        let backup = Connection::open_with_flags(
            dir.path().join("engine.sqlite.v7.bak"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        assert_eq!(super::super::schema::version(&backup).unwrap(), 7);
        assert_eq!(
            s.read(super::super::schema::version).unwrap(),
            crate::store::SCHEMA_VERSION
        );
    }
}
