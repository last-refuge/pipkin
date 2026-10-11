//! Reading the reference engine's session storage (`packages/durable/src/storage/sqlite`).
//!
//! Every table keeps its authoritative value in a `record` JSON column; indexed columns are
//! copies (and some are JSON-encoded strings, `storage.ts:64`), so this reader uses the
//! records. Documents are materialized like `storage.ts:542-577`: the newest base, then every
//! later delta of the same version.

use std::collections::BTreeMap;
use std::path::Path;

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::Value;

use super::delta;

/// The only source schema version this importer understands (`migrations.ts:85`).
pub const SUPPORTED_SCHEMA: i64 = 1;

/// One stored entry: its raw record text (byte for byte) and the parsed record.
#[derive(Clone, Debug)]
pub struct LegacyEntry {
    pub id: i64,
    pub conversation: i64,
    pub raw: String,
    pub record: Value,
}

impl LegacyEntry {
    pub fn kind(&self) -> &str {
        self.record
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("")
    }

    pub fn head(&self) -> Option<&Value> {
        self.record.get("head").filter(|v| !v.is_null())
    }

    pub fn model(&self) -> Option<&Vec<Value>> {
        self.record.get("model").and_then(Value::as_array)
    }

    pub fn edits(&self) -> Option<&Vec<Value>> {
        self.record.get("edits").and_then(Value::as_array)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocScope {
    Session,
    Conversation(i64),
    Task(i64),
}

/// The current incarnation of a document with its materialized latest value.
#[derive(Clone, Debug)]
pub struct LegacyDoc {
    pub id: i64,
    pub kind: String,
    pub key: Option<String>,
    pub scope: DocScope,
    pub version: i64,
    pub value: Value,
    pub record: Value,
}

#[derive(Clone, Debug, Default)]
pub struct Legacy {
    pub schema_version: i64,
    pub conversations: BTreeMap<i64, Value>,
    /// Ordered by id.
    pub entries: Vec<LegacyEntry>,
    pub tasks: BTreeMap<i64, Value>,
    /// Ordered by id: (id, parsed record).
    pub submissions: Vec<(i64, Value)>,
    pub documents: Vec<LegacyDoc>,
    pub retired_documents: u64,
}

impl Legacy {
    pub fn doc(&self, kind: &str, conversation: i64) -> Option<&LegacyDoc> {
        self.documents.iter().find(|d| {
            d.kind == kind && d.key.is_none() && d.scope == DocScope::Conversation(conversation)
        })
    }

    pub fn entry(&self, id: i64) -> Option<&LegacyEntry> {
        self.entries
            .binary_search_by_key(&id, |e| e.id)
            .ok()
            .map(|i| &self.entries[i])
    }
}

/// Percent-encode a filesystem path for an SQLite `file:` URI.
fn uri_path(path: &Path) -> String {
    let mut out = String::new();
    for b in path.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Open the reference database without writing it.
///
/// With no write-ahead log content the file alone is the database, and it is opened
/// `immutable` (SQLite creates no `-wal`/`-shm` file and takes no lock). With WAL content (an
/// unclean shutdown, or a writer that never checkpointed) it is opened read-only through the
/// normal locking protocol so the WAL's committed frames are seen consistently; SQLite may then
/// touch the shared-memory index (`-shm`), never the database or the WAL.
pub fn open_source(path: &Path) -> rusqlite::Result<Connection> {
    let wal = wal_path(path);
    let has_wal = std::fs::metadata(&wal)
        .map(|m| m.len() > 0)
        .unwrap_or(false);
    let uri = if has_wal {
        format!("file:{}?mode=ro", uri_path(path))
    } else {
        format!("file:{}?mode=ro&immutable=1", uri_path(path))
    };
    let conn = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(std::time::Duration::from_millis(2000))?;
    conn.execute_batch("PRAGMA query_only = 1")?;
    Ok(conn)
}

pub fn wal_path(path: &Path) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push("-wal");
    std::path::PathBuf::from(name)
}

#[derive(Debug)]
pub enum ReadError {
    /// SQLite reported the database busy or locked: a writer owns it.
    Busy(String),
    /// The database is not a supported reference session store.
    Unsupported(String),
    Corrupt(String),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Busy(m) | ReadError::Unsupported(m) | ReadError::Corrupt(m) => {
                f.write_str(m)
            }
        }
    }
}

pub fn classify(e: rusqlite::Error) -> ReadError {
    match &e {
        rusqlite::Error::SqliteFailure(f, _)
            if matches!(
                f.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            ) =>
        {
            ReadError::Busy(format!("the session database is in use ({e})"))
        }
        rusqlite::Error::SqliteFailure(f, _)
            if matches!(
                f.code,
                rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt
            ) =>
        {
            ReadError::Corrupt(format!("the session database is damaged ({e})"))
        }
        _ => ReadError::Corrupt(format!("the session database could not be read ({e})")),
    }
}

fn parse(raw: &str, what: &str) -> Result<Value, ReadError> {
    serde_json::from_str(raw).map_err(|e| ReadError::Corrupt(format!("{what}: invalid JSON ({e})")))
}

fn has_table(conn: &Connection, name: &str) -> Result<bool, ReadError> {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [name],
        |_| Ok(()),
    )
    .optional()
    .map(|r| r.is_some())
    .map_err(classify)
}

/// Just the schema version (`durable_schema`), for discovery.
pub fn schema_version(conn: &Connection) -> Result<i64, ReadError> {
    if !has_table(conn, "durable_schema")? {
        return Err(ReadError::Unsupported(
            "not a reference session store (no durable_schema table)".into(),
        ));
    }
    conn.query_row(
        "SELECT version FROM durable_schema WHERE singleton = 1",
        [],
        |r| r.get(0),
    )
    .optional()
    .map_err(classify)?
    .ok_or_else(|| ReadError::Unsupported("schema metadata is missing".into()))
}

/// Read everything inside one read transaction (one consistent snapshot).
pub fn read(conn: &Connection) -> Result<Legacy, ReadError> {
    conn.execute_batch("BEGIN").map_err(classify)?;
    let result = read_in(conn);
    let _ = conn.execute_batch("COMMIT");
    result
}

fn read_in(conn: &Connection) -> Result<Legacy, ReadError> {
    let schema_version = schema_version(conn)?;
    if schema_version != SUPPORTED_SCHEMA {
        return Err(ReadError::Unsupported(format!(
            "source schema version {schema_version} is not supported (this importer reads version {SUPPORTED_SCHEMA})"
        )));
    }
    for table in [
        "durable_metadata",
        "conversations",
        "entries",
        "tasks",
        "submissions",
        "documents",
        "document_revisions",
    ] {
        if !has_table(conn, table)? {
            return Err(ReadError::Unsupported(format!("table {table} is missing")));
        }
    }
    let mut legacy = Legacy {
        schema_version,
        ..Legacy::default()
    };

    let rows = query_pairs(conn, "SELECT id, record FROM conversations ORDER BY id")?;
    for (id, raw) in rows {
        legacy
            .conversations
            .insert(id, parse(&raw, &format!("conversation {id}"))?);
    }

    let mut stmt = conn
        .prepare("SELECT id, conversation_id, record FROM entries ORDER BY id")
        .map_err(classify)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(classify)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(classify)?;
    for (id, conversation, raw) in rows {
        let record = parse(&raw, &format!("entry {id}"))?;
        legacy.entries.push(LegacyEntry {
            id,
            conversation,
            raw,
            record,
        });
    }

    for (id, raw) in query_pairs(conn, "SELECT id, record FROM tasks ORDER BY id")? {
        legacy.tasks.insert(id, parse(&raw, &format!("task {id}"))?);
    }
    for (id, raw) in query_pairs(conn, "SELECT id, record FROM submissions ORDER BY id")? {
        legacy
            .submissions
            .push((id, parse(&raw, &format!("submission {id}"))?));
    }

    legacy.retired_documents = conn
        .query_row(
            "SELECT COUNT(*) FROM documents WHERE retired_at IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map_err(classify)? as u64;
    let docs = query_pairs(
        conn,
        "SELECT id, record FROM documents WHERE retired_at IS NULL ORDER BY id",
    )?;
    for (id, raw) in docs {
        let record = parse(&raw, &format!("document {id}"))?;
        legacy.documents.push(materialize(conn, id, record)?);
    }
    Ok(legacy)
}

fn query_pairs(conn: &Connection, sql: &str) -> Result<Vec<(i64, String)>, ReadError> {
    let mut stmt = conn.prepare(sql).map_err(classify)?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .map_err(classify)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(classify)?;
    Ok(rows)
}

fn materialize(conn: &Connection, id: i64, record: Value) -> Result<LegacyDoc, ReadError> {
    let corrupt = |why: String| ReadError::Corrupt(format!("document {id}: {why}"));
    let kind = record
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| corrupt("record has no kind".into()))?
        .to_owned();
    let key = record.get("key").and_then(Value::as_str).map(str::to_owned);
    let scope = match record.get("scope") {
        Some(s) => match s.get("kind").and_then(Value::as_str) {
            Some("session") => DocScope::Session,
            Some("conversation") => DocScope::Conversation(
                s.get("conversationId")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| corrupt("conversation scope without id".into()))?,
            ),
            Some("task") => DocScope::Task(
                s.get("taskId")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| corrupt("task scope without id".into()))?,
            ),
            _ => return Err(corrupt("unknown scope".into())),
        },
        None => return Err(corrupt("record has no scope".into())),
    };
    let base: Option<(i64, i64, String)> = conn
        .query_row(
            "SELECT seq, version, content FROM document_revisions
             WHERE document_id = ?1 AND kind = 'base' ORDER BY seq DESC LIMIT 1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(classify)?;
    let Some((base_seq, version, content)) = base else {
        return Err(corrupt("missing a required base".into()));
    };
    let mut value = parse(&content, &format!("document {id} base"))?;
    let mut stmt = conn
        .prepare(
            "SELECT kind, version, content FROM document_revisions
             WHERE document_id = ?1 AND seq > ?2 ORDER BY seq",
        )
        .map_err(classify)?;
    let tail = stmt
        .query_map([id, base_seq], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(classify)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(classify)?;
    for (rev_kind, rev_version, content) in tail {
        if rev_kind != "delta" || rev_version != version {
            return Err(corrupt(
                "crosses a stored version boundary without a base".into(),
            ));
        }
        let ops = delta::parse_ops(&parse(&content, &format!("document {id} delta"))?)
            .map_err(|e| corrupt(e.to_string()))?;
        value = delta::apply(value, ops).map_err(|e| corrupt(e.to_string()))?;
    }
    Ok(LegacyDoc {
        id,
        kind,
        key,
        scope,
        version,
        value,
        record,
    })
}
