//! Model-supplied ids identify a tool within its assistant message, not a durable invocation.
use std::collections::HashSet;

use pipkin_engine_protocol::{Block, Message};
use rusqlite::{Connection, params};

use super::{Result, Store};
use crate::util::{ORIGINAL_TOOL_CALL_ID, random_hex};

impl Store {
    /// Every finalized assistant message reserves its ids, including imports and calls
    /// that were never executed. Existing ids are retained when entries are retired.
    pub(super) fn remember_tool_ids(
        c: &Connection,
        session: &str,
        kind: &pipkin_engine_protocol::EntryKind,
    ) -> Result<()> {
        if let pipkin_engine_protocol::EntryKind::Message {
            message,
            partial: false,
            ..
        } = kind
        {
            for (id, _, _) in message.tool_calls() {
                c.execute(
                    "INSERT OR IGNORE INTO tool_call_ids(session, call_id) VALUES (?1,?2)",
                    params![session, id],
                )?;
            }
        }
        Ok(())
    }

    /// Run in the transaction that commits a new model response. A collision gets a private
    /// durable id; the original id stays on the block for provider-context projection. Recovery
    /// reads the committed response and never calls this again for the same invocation.
    pub fn disambiguate_tool_ids(
        c: &Connection,
        session: &str,
        message: &mut Message,
    ) -> Result<()> {
        let Message::Assistant { content, .. } = message else {
            return Ok(());
        };
        let mut seen = HashSet::new();
        let mut exists = c.prepare_cached(
            "SELECT EXISTS(SELECT 1 FROM tool_call_ids WHERE session=?1 AND call_id=?2)",
        )?;
        for block in content {
            if let Block::ToolCall { id, extra, .. } = block {
                // Incoming provider data cannot supply our persistence metadata.
                extra.remove(ORIGINAL_TOOL_CALL_ID);
                let used = |id: &str,
                            seen: &HashSet<String>,
                            stmt: &mut rusqlite::CachedStatement<'_>|
                 -> Result<bool> {
                    Ok(seen.contains(id)
                        || stmt.query_row(params![session, id], |r| r.get::<_, bool>(0))?)
                };
                if used(id, &seen, &mut exists)? {
                    let original = id.clone();
                    loop {
                        *id = format!("pipkin_{}", random_hex(16));
                        if !used(id, &seen, &mut exists)? {
                            break;
                        }
                    }
                    extra.insert(ORIGINAL_TOOL_CALL_ID.into(), original.into());
                }
                seen.insert(id.clone());
            }
        }
        Ok(())
    }
}
