//! Retained web sources (`web_sources`, schema 12): one row per search result or fetched
//! page, with the extracted text and (for text/markup) the original bytes, bounded per
//! session. Expiry keeps the metadata (URL, title, date, hash) so a citation stays meaningful
//! after its content is gone; reading an expired source is an explicit outcome.

use rusqlite::{Connection, OptionalExtension, params};

use super::{Result, Store};
use crate::web::{
    Fetched, MAX_RETAINED_BYTES, MAX_RETAINED_SOURCES, RETAINED_BODY_BYTES, SourceRecord,
    providers::SearchResult,
};

fn record_from_row(r: &rusqlite::Row) -> rusqlite::Result<SourceRecord> {
    Ok(SourceRecord {
        id: r.get::<_, i64>("id")? as u64,
        kind: r.get("kind")?,
        provider: r.get("provider")?,
        requested_url: r.get("requested_url")?,
        final_url: r.get("final_url")?,
        title: r.get("title")?,
        excerpt: r.get("excerpt")?,
        published: r.get("published_at")?,
        retrieved_at_ms: r.get("retrieved_at")?,
        content_type: r.get("content_type")?,
        bytes: r.get::<_, i64>("bytes")? as u64,
        sha256: r.get("sha256")?,
        text_chars: r.get::<_, i64>("text_chars")? as u64,
        expired: r.get::<_, i64>("expired")? != 0,
        query: r.get("query")?,
        call_id: r.get("call_id")?,
    })
}

const COLUMNS: &str = "session, id, kind, provider, requested_url, final_url, title, excerpt, \
    published_at, retrieved_at, content_type, bytes, sha256, text_chars, expired, query, call_id";

impl Store {
    fn next_source_id(c: &Connection, session: &str) -> Result<u64> {
        Ok(c.query_row(
            "SELECT COALESCE(MAX(id), 0) + 1 FROM web_sources WHERE session = ?1",
            [session],
            |r| r.get::<_, i64>(0),
        )? as u64)
    }

    /// Record each search result as a metadata-only source (idempotent per call id: a
    /// replayed call returns the rows it recorded).
    pub fn record_search(
        tx: &Connection,
        session: &str,
        call_id: &str,
        provider: &str,
        query: &str,
        results: &[SearchResult],
        now: i64,
    ) -> Result<Vec<SourceRecord>> {
        Self::assert_session_live(tx, session)?;
        let existing = Self::web_sources_of_call(tx, session, call_id)?;
        if !existing.is_empty() {
            return Ok(existing);
        }
        let first = Self::next_source_id(tx, session)?;
        let mut records = Vec::with_capacity(results.len());
        for (id, result) in (first..).zip(results.iter()) {
            tx.execute(
                &format!(
                    "INSERT INTO web_sources({COLUMNS}) VALUES (?1, ?2, 'search', ?3, ?4, ?4, ?5, ?6, ?7, ?8, NULL, 0, NULL, 0, 0, ?9, ?10)"
                ),
                params![
                    session,
                    id as i64,
                    provider,
                    result.url,
                    result.title,
                    result.excerpt,
                    result.published,
                    now,
                    query,
                    call_id
                ],
            )?;
            records.push(Self::web_source(tx, session, id)?.expect("just inserted"));
        }
        Ok(records)
    }

    /// Retain a fetched page: metadata, text and (bounded) original bytes; then expire the
    /// oldest retained content beyond the session's bounds.
    pub fn record_fetch(
        tx: &Connection,
        session: &str,
        call_id: &str,
        fetched: &Fetched,
        now: i64,
    ) -> Result<SourceRecord> {
        Self::assert_session_live(tx, session)?;
        if let Some(existing) = Self::web_sources_of_call(tx, session, call_id)?
            .into_iter()
            .next()
        {
            return Ok(existing);
        }
        let id = Self::next_source_id(tx, session)?;
        let sha = crate::web::sha256_hex(&fetched.bytes);
        let body: Option<&[u8]> =
            (fetched.bytes.len() <= RETAINED_BODY_BYTES).then_some(fetched.bytes.as_slice());
        tx.execute(
            &format!(
                "INSERT INTO web_sources({COLUMNS}, text, body) VALUES (?1, ?2, 'fetch', 'fetch', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 0, NULL, ?13, ?14, ?15)"
            ),
            params![
                session,
                id as i64,
                fetched.requested_url,
                fetched.final_url,
                fetched.title,
                fetched.text.chars().take(300).collect::<String>(),
                fetched.published,
                now,
                fetched.content_type,
                fetched.bytes.len() as i64,
                sha,
                fetched.text.chars().count() as i64,
                call_id,
                fetched.text,
                body,
            ],
        )?;
        Self::expire_web_sources(tx, session)?;
        Ok(Self::web_source(tx, session, id)?.expect("just inserted"))
    }

    /// Drop retained content (keep metadata) of the oldest sources beyond the bounds.
    fn expire_web_sources(tx: &Connection, session: &str) -> Result<()> {
        let mut stmt = tx.prepare(
            "SELECT id, bytes + text_chars FROM web_sources
             WHERE session = ?1 AND expired = 0 AND kind = 'fetch' ORDER BY id DESC",
        )?;
        let rows = stmt
            .query_map([session], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? as u64))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut total: u64 = 0;
        let mut expire = Vec::new();
        for (n, (id, size)) in rows.into_iter().enumerate() {
            total += size;
            if n >= MAX_RETAINED_SOURCES || total > MAX_RETAINED_BYTES {
                expire.push(id);
            }
        }
        for id in expire {
            tx.execute(
                "UPDATE web_sources SET expired = 1, text = NULL, body = NULL
                 WHERE session = ?1 AND id = ?2",
                params![session, id],
            )?;
        }
        Ok(())
    }

    pub fn web_source(c: &Connection, session: &str, id: u64) -> Result<Option<SourceRecord>> {
        Ok(c.query_row(
            &format!("SELECT {COLUMNS} FROM web_sources WHERE session = ?1 AND id = ?2"),
            params![session, id as i64],
            record_from_row,
        )
        .optional()?)
    }

    pub fn web_source_text(c: &Connection, session: &str, id: u64) -> Result<Option<String>> {
        Ok(c.query_row(
            "SELECT text FROM web_sources WHERE session = ?1 AND id = ?2",
            params![session, id as i64],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
    }

    pub fn web_sources(c: &Connection, session: &str) -> Result<Vec<SourceRecord>> {
        let mut stmt = c.prepare(&format!(
            "SELECT {COLUMNS} FROM web_sources WHERE session = ?1 ORDER BY id"
        ))?;
        let rows = stmt
            .query_map([session], record_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn web_sources_of_call(
        c: &Connection,
        session: &str,
        call_id: &str,
    ) -> Result<Vec<SourceRecord>> {
        let mut stmt = c.prepare(&format!(
            "SELECT {COLUMNS} FROM web_sources WHERE session = ?1 AND call_id = ?2 ORDER BY id"
        ))?;
        let rows = stmt
            .query_map(params![session, call_id], record_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}
