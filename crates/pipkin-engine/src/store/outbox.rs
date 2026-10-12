//! A compact durable changefeed. Invalidations share the state transaction (SQLite triggers),
//! so death between commit and live publication cannot hide a change. Content remains in its
//! authoritative tables; catch-up pages never duplicate large replies or credentials.
use super::*;

impl Store {
    pub fn catch_up(&self, after: u64, limit: u32) -> Result<EventPage> {
        self.read(|c| {
            let (first, head): (u64, u64) = c.query_row(
                "SELECT COALESCE(MIN(cursor), 0), COALESCE(MAX(cursor), 0) FROM event_outbox",
                [], |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let reset = after > head || (first > 0 && after < first - 1);
            if reset {
                return Ok(EventPage { events: vec![], through: head, head, reset });
            }
            let mut stmt = c.prepare_cached(
                "SELECT cursor, session, revision FROM event_outbox WHERE cursor > ?1 ORDER BY cursor LIMIT ?2"
            )?;
            let events = stmt.query_map(params![after, limit.clamp(1, 256)], |r| {
                Ok(DurableEvent { cursor: r.get(0)?, session: r.get(1)?, revision: r.get(2)? })
            })?.collect::<rusqlite::Result<Vec<_>>>()?;
            let through = events.last().map_or(after, |e| e.cursor);
            Ok(EventPage { events, through, head, reset })
        })
    }
}
