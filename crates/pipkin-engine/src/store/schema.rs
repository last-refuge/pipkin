//! Schema versions and migrations. Version 2 adds durable retries; version 3 the change outbox.

use std::path::Path;

use rusqlite::Connection;

use super::{Result, StoreError};

/// The newest schema this build understands.
pub const SCHEMA_VERSION: u32 = 3;

const V1: &str = r#"
CREATE TABLE meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;

CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    cwd TEXT NOT NULL,
    title TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    origin TEXT NOT NULL CHECK (json_valid(origin)),
    model TEXT,
    thinking TEXT,
    subagents_enabled INTEGER NOT NULL DEFAULT 1 CHECK (subagents_enabled IN (0, 1)),
    parent TEXT REFERENCES sessions(id),
    root TEXT REFERENCES sessions(id),
    create_key TEXT UNIQUE,
    revision INTEGER NOT NULL DEFAULT 0,
    blocked TEXT
) STRICT;
CREATE INDEX sessions_by_parent ON sessions(parent, created_at);

CREATE TABLE entries (
    session TEXT NOT NULL REFERENCES sessions(id),
    id INTEGER NOT NULL,
    at_ms INTEGER NOT NULL,
    run INTEGER,
    retired INTEGER NOT NULL DEFAULT 0 CHECK (retired IN (0, 1)),
    partial INTEGER NOT NULL DEFAULT 0 CHECK (partial IN (0, 1)),
    kind TEXT NOT NULL,
    body TEXT NOT NULL CHECK (json_valid(body)),
    PRIMARY KEY (session, id)
) STRICT;
CREATE INDEX entries_by_kind ON entries(session, kind, id);
CREATE INDEX entries_by_run ON entries(session, run, id);

-- Committed stream deltas of a partial message; dropped when it is finalized.
CREATE TABLE entry_chunks (
    session TEXT NOT NULL,
    entry INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    delta TEXT NOT NULL CHECK (json_valid(delta)),
    PRIMARY KEY (session, entry, seq)
) STRICT;

-- Admission ledger and queue. The key is unique per session.
CREATE TABLE requests (
    session TEXT NOT NULL REFERENCES sessions(id),
    key TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    mode TEXT NOT NULL CHECK (mode IN ('prompt', 'steer', 'followUp')),
    state TEXT NOT NULL CHECK (state IN ('queued', 'running', 'settled', 'withdrawn')),
    queue_entry INTEGER,
    run INTEGER,
    outcome TEXT CHECK (outcome IS NULL OR json_valid(outcome)),
    text TEXT NOT NULL,
    images TEXT NOT NULL CHECK (json_valid(images)),
    model TEXT,
    created_at INTEGER NOT NULL,
    settled_at INTEGER,
    PRIMARY KEY (session, key)
) STRICT;
CREATE INDEX requests_by_state ON requests(session, state, queue_entry);
CREATE INDEX requests_by_run ON requests(run);
CREATE UNIQUE INDEX requests_by_queue_entry ON requests(session, queue_entry)
    WHERE queue_entry IS NOT NULL;

CREATE TABLE runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session TEXT NOT NULL REFERENCES sessions(id),
    request TEXT,
    state TEXT NOT NULL CHECK (state IN ('running', 'stopping', 'settled')),
    stop_requested_at INTEGER,
    outcome TEXT CHECK (outcome IS NULL OR json_valid(outcome)),
    started_at INTEGER NOT NULL,
    ended_at INTEGER
) STRICT;
CREATE INDEX runs_open ON runs(session, state);

CREATE TABLE tool_calls (
    session TEXT NOT NULL REFERENCES sessions(id),
    call_id TEXT NOT NULL,
    run INTEGER,
    name TEXT NOT NULL,
    arguments TEXT NOT NULL CHECK (json_valid(arguments)),
    state TEXT NOT NULL CHECK (state IN ('intent', 'done', 'interrupted')),
    replay_safe INTEGER NOT NULL CHECK (replay_safe IN (0, 1)),
    result_entry INTEGER,
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    PRIMARY KEY (session, call_id)
) STRICT;
CREATE INDEX tool_calls_by_state ON tool_calls(session, state);

-- Retained (bounded) complete tool output; the entry holds what the model saw.
CREATE TABLE tool_outputs (
    session TEXT NOT NULL,
    call_id TEXT NOT NULL,
    text TEXT NOT NULL,
    retained INTEGER NOT NULL,
    dropped INTEGER NOT NULL,
    PRIMARY KEY (session, call_id)
) STRICT;

CREATE TABLE usage (
    session TEXT NOT NULL REFERENCES sessions(id),
    bucket TEXT NOT NULL CHECK (bucket IN ('model', 'tool')),
    key TEXT NOT NULL,
    value TEXT NOT NULL CHECK (json_valid(value)),
    PRIMARY KEY (session, bucket, key)
) STRICT;

CREATE TABLE subagent_tasks (
    root TEXT NOT NULL REFERENCES sessions(id),
    child INTEGER NOT NULL,
    parent_session TEXT NOT NULL REFERENCES sessions(id),
    parent_call TEXT NOT NULL,
    child_session TEXT NOT NULL UNIQUE REFERENCES sessions(id),
    task TEXT NOT NULL,
    status TEXT NOT NULL,
    result TEXT,
    result_delivered INTEGER NOT NULL DEFAULT 0 CHECK (result_delivered IN (0, 1)),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (root, child),
    UNIQUE (parent_session, parent_call)
) STRICT;

-- Per-session engine and extension state (preserved verbatim on import).
CREATE TABLE kv (
    session TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (session, key)
) STRICT;

CREATE TABLE import_map (
    source_engine TEXT NOT NULL,
    source_session TEXT NOT NULL,
    native_session TEXT NOT NULL UNIQUE REFERENCES sessions(id),
    manifest TEXT NOT NULL CHECK (json_valid(manifest)),
    imported_at INTEGER NOT NULL,
    PRIMARY KEY (source_engine, source_session)
) STRICT;
"#;

/// Ordered migrations; index `i` produces version `i + 1`.
const V2: &str = r#"
CREATE TABLE generation_state (
    run INTEGER PRIMARY KEY REFERENCES runs(id),
    failures INTEGER NOT NULL CHECK (failures >= 0),
    handled_error INTEGER,
    retry_until INTEGER,
    overflow_compacted INTEGER NOT NULL CHECK (overflow_compacted IN (0, 1))
) STRICT;
"#;

const V3: &str = r#"
CREATE TABLE event_outbox (
    cursor INTEGER PRIMARY KEY AUTOINCREMENT,
    session TEXT NOT NULL,
    revision INTEGER NOT NULL
) STRICT;
CREATE TRIGGER event_session_insert AFTER INSERT ON sessions BEGIN
    INSERT INTO event_outbox(session, revision) VALUES (NEW.id, NEW.revision);
END;
CREATE TRIGGER event_session_revision AFTER UPDATE OF revision ON sessions
WHEN NEW.revision != OLD.revision BEGIN
    INSERT INTO event_outbox(session, revision) VALUES (NEW.id, NEW.revision);
END;
-- Bounded retention, even with no client connected. Older cursors must resnapshot.
CREATE TRIGGER event_retention AFTER INSERT ON event_outbox BEGIN
    DELETE FROM event_outbox WHERE cursor <= NEW.cursor - 4096;
END;
"#;

const MIGRATIONS: &[&str] = &[V1, V2, V3];

pub fn configure(conn: &Connection, wal: bool) -> Result<()> {
    if wal {
        let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(StoreError::Corrupt(format!(
                "could not enable WAL (journal mode {mode})"
            )));
        }
    }
    conn.execute_batch(
        "PRAGMA synchronous = FULL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;",
    )?;
    Ok(())
}

pub fn version(conn: &Connection) -> Result<u32> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? as u32)
}

/// Bring the schema to [`SCHEMA_VERSION`]. A newer database is refused without changes. An
/// existing older database is backed up next to itself first.
pub fn migrate(conn: &Connection, path: Option<&Path>) -> Result<()> {
    let current = version(conn)?;
    if current > SCHEMA_VERSION {
        return Err(StoreError::NewerSchema(current));
    }
    if current == SCHEMA_VERSION {
        return Ok(());
    }
    if current > 0
        && let Some(path) = path
    {
        let backup = path.with_extension(format!("sqlite.v{current}.bak"));
        let mut target = Connection::open(&backup)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&backup, std::fs::Permissions::from_mode(0o600))
                .map_err(StoreError::Io)?;
        }
        let b = rusqlite::backup::Backup::new(conn, &mut target)?;
        b.run_to_completion(256, std::time::Duration::from_millis(0), None)?;
    }
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| -> Result<()> {
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
            conn.execute_batch(sql)?;
            conn.execute_batch(&format!("PRAGMA user_version = {}", i + 1))?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// SQLite's own verdict that a file is not a (readable) database.
fn is_damage(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(f, _)
            if matches!(f.code, rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase)
    )
}

/// The damaged-database policy: refuse to start, name the file, change nothing. The engine
/// data is the only copy of the conversations, so it is never set aside or repaired
/// automatically; the person decides (move it away to start empty, or restore a backup).
fn damaged(path: &Path, detail: &str) -> StoreError {
    StoreError::Corrupt(format!(
        "{} is damaged ({detail}). It was left untouched; move it (with any -wal and -shm files \
         next to it) somewhere else to start with empty engine data, or restore it from a backup",
        path.display()
    ))
}

/// Before anything writes to an existing database: it must be readable as SQLite and pass
/// `PRAGMA quick_check` (structure, not foreign keys or content).
fn check_existing(conn: &Connection, path: &Path) -> Result<u32> {
    let current = match version(conn) {
        Ok(v) => v,
        Err(StoreError::Sqlite(e)) if is_damage(&e) => return Err(damaged(path, &e.to_string())),
        Err(e) => return Err(e),
    };
    if current == 0 || current > SCHEMA_VERSION {
        return Ok(current);
    }
    let verdict: std::result::Result<String, rusqlite::Error> =
        conn.query_row("PRAGMA quick_check", [], |r| r.get(0));
    match verdict {
        Ok(ok) if ok == "ok" => Ok(current),
        Ok(problem) => Err(damaged(path, &problem)),
        Err(e) if is_damage(&e) => Err(damaged(path, &e.to_string())),
        Err(e) => Err(e.into()),
    }
}

pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    // Check before configuring so a newer or damaged database is not even switched to WAL.
    let current = check_existing(&conn, path)?;
    if current > SCHEMA_VERSION {
        return Err(StoreError::NewerSchema(current));
    }
    configure(&conn, true)?;
    migrate(&conn, Some(path))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(conn)
}
