//! Last committed bounded tool output. An existing kv slot avoids changing the schema.
use super::*;
use crate::tools::text::BoundedOutput;

impl Store {
    pub(crate) fn save_tool_progress(
        c: &Connection,
        session: &str,
        call: &str,
        output: &BoundedOutput,
    ) -> Result<()> {
        if Self::tool_call_in(c, session, call)?.is_none_or(|call| call.state != ToolState::Intent)
        {
            return Err(StoreError::Corrupt(
                "Tool output checkpoint has no live intent".into(),
            ));
        }
        Self::kv_set(c, session, &format!("tool_progress:{call}"), &json(output))
    }

    pub(crate) fn tool_progress(
        c: &Connection,
        session: &str,
        call: &str,
    ) -> Result<BoundedOutput> {
        let raw: Option<String> = c
            .query_row(
                "SELECT value FROM kv WHERE session = ?1 AND key = ?2",
                params![session, format!("tool_progress:{call}")],
                |row| row.get(0),
            )
            .optional()?;
        raw.map(|value| parse_json(&value, "tool progress"))
            .transpose()
            .map(Option::unwrap_or_default)
    }

    pub(crate) fn clear_tool_progress(c: &Connection, session: &str, call: &str) -> Result<()> {
        c.execute(
            "DELETE FROM kv WHERE session = ?1 AND key = ?2",
            params![session, format!("tool_progress:{call}")],
        )?;
        Ok(())
    }
}
