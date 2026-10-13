//! Schema versions and migrations. Version 2 adds durable retries; version 3 the change outbox.

use std::path::Path;

use rusqlite::Connection;

use super::{Result, StoreError};

/// The newest schema this build understands.
pub const SCHEMA_VERSION: u32 = 19;

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

// Version 4 introduces positional system message bodies; older runtimes must refuse them.
// Version 5 pins extension selections in kv/meta; older engines would ignore these choices.
const V6: &str = r#"
CREATE TABLE compactions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session TEXT NOT NULL REFERENCES sessions(id),
    operation_key TEXT NOT NULL,
    run INTEGER REFERENCES runs(id),
    scope TEXT NOT NULL CHECK (scope IN ('manual', 'background', 'blocking')),
    instructions TEXT,
    state TEXT NOT NULL CHECK (state IN ('running', 'stopping', 'settled')),
    revision INTEGER NOT NULL DEFAULT 0,
    progress TEXT NOT NULL CHECK (json_valid(progress)),
    UNIQUE(session, operation_key)
) STRICT;
CREATE INDEX compactions_pending ON compactions(session, state, id) WHERE state != 'settled';
CREATE INDEX compactions_run ON compactions(run, id) WHERE run IS NOT NULL;
"#;
// V7 activates journal execution. V6 runtimes must refuse active recoverable jobs.
const V7: &str = "CREATE INDEX compactions_recovery ON compactions(id) WHERE state != 'settled';";
const V8: &str = r#"
CREATE TABLE summary_writes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session TEXT NOT NULL REFERENCES sessions(id),
    compaction INTEGER NOT NULL UNIQUE REFERENCES compactions(id),
    state TEXT NOT NULL CHECK (state IN ('queued', 'placed', 'stale')),
    body TEXT CHECK (body IS NULL OR json_valid(body)),
    entry INTEGER,
    CHECK ((state = 'queued') = (body IS NOT NULL)),
    CHECK ((state = 'placed') = (entry IS NOT NULL))
) STRICT;
CREATE INDEX summary_writes_pending ON summary_writes(session, id) WHERE state = 'queued';
CREATE TABLE tool_boundaries (
    run INTEGER NOT NULL REFERENCES runs(id),
    assistant INTEGER NOT NULL,
    PRIMARY KEY(run, assistant)
) STRICT;
-- A later user/assistant message in the same run proves the old runtime already crossed
-- this tool boundary. Do not consume another steer when upgrading a streaming run.
INSERT INTO tool_boundaries(run, assistant)
SELECT e.run, e.id FROM entries e
WHERE e.run IS NOT NULL AND json_extract(e.body, '$.message.role') = 'assistant'
AND json_extract(e.body, '$.message.stopReason') = 'toolUse'
AND EXISTS (SELECT 1 FROM entries n WHERE n.session = e.session AND n.run = e.run
    AND n.id > e.id AND json_extract(n.body, '$.message.role') IN ('assistant', 'user'));
"#;
const V9: &str = r#"
CREATE TABLE session_removals (
    root TEXT PRIMARY KEY,
    create_key TEXT UNIQUE,
    requested_at INTEGER NOT NULL,
    complete INTEGER NOT NULL DEFAULT 0 CHECK (complete IN (0, 1))
) STRICT;
CREATE TRIGGER event_session_delete AFTER DELETE ON sessions BEGIN
    INSERT INTO event_outbox(session, revision) VALUES (OLD.id, OLD.revision + 1);
END;
CREATE TRIGGER removed_session_insert BEFORE INSERT ON sessions
WHEN EXISTS (SELECT 1 FROM session_removals WHERE root = NEW.id OR root = NEW.root
    OR (NEW.create_key IS NOT NULL AND create_key = NEW.create_key)) BEGIN
    SELECT RAISE(ABORT, 'session removal is committed');
END;
"#;
// A complete index includes imported/retired/unexecuted calls, not just executed intents.
// Version 10 also changes semantics: private ids on collided calls must be projected back
// to their provider originals. Older engines refuse this version before modifying it.
const V10: &str = r#"
CREATE TABLE tool_call_ids (
    session TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    call_id TEXT NOT NULL,
    PRIMARY KEY (session, call_id)
) STRICT;
INSERT OR IGNORE INTO tool_call_ids(session, call_id)
SELECT e.session, json_extract(j.value, '$.id') FROM entries e,
    json_each(e.body, '$.message.content') j
WHERE e.partial=0 AND json_extract(e.body, '$.message.role')='assistant'
    AND j.type='object' AND json_extract(j.value, '$.type')='toolCall'
    AND json_type(j.value, '$.id')='text';
INSERT OR IGNORE INTO tool_call_ids SELECT session, call_id FROM tool_calls;
INSERT OR IGNORE INTO tool_call_ids SELECT session, call_id FROM tool_outputs;
"#;
// Version 11 adds plans (one per session): task bodies, a change log and idempotent receipts.
const V11: &str = r#"
CREATE TABLE plans (
    session TEXT PRIMARY KEY REFERENCES sessions(id),
    revision INTEGER NOT NULL DEFAULT 0,
    next_task INTEGER NOT NULL DEFAULT 1,
    updated_at INTEGER NOT NULL
) STRICT;
CREATE TABLE plan_tasks (
    session TEXT NOT NULL REFERENCES sessions(id),
    id INTEGER NOT NULL,
    position INTEGER NOT NULL,
    body TEXT NOT NULL CHECK (json_valid(body)),
    archived INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0, 1)),
    status TEXT NOT NULL CHECK (status IN ('pending', 'active', 'blocked', 'completed', 'skipped', 'cancelled')),
    PRIMARY KEY (session, id)
) STRICT;
CREATE INDEX plan_tasks_order ON plan_tasks(session, archived, position);
CREATE TABLE plan_events (
    session TEXT NOT NULL REFERENCES sessions(id),
    seq INTEGER NOT NULL,
    at_ms INTEGER NOT NULL,
    revision INTEGER NOT NULL,
    task INTEGER NOT NULL,
    kind TEXT NOT NULL,
    data TEXT NOT NULL CHECK (json_valid(data)),
    PRIMARY KEY (session, seq)
) STRICT;
CREATE TABLE plan_receipts (
    session TEXT NOT NULL REFERENCES sessions(id),
    key TEXT NOT NULL,
    body TEXT NOT NULL CHECK (json_valid(body)),
    PRIMARY KEY (session, key)
) STRICT;
"#;
// Version 12 adds retained web sources (search results and fetched pages) per session.
const V12: &str = r#"
CREATE TABLE web_sources (
    session TEXT NOT NULL REFERENCES sessions(id),
    id INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('search', 'fetch')),
    provider TEXT NOT NULL,
    requested_url TEXT NOT NULL,
    final_url TEXT NOT NULL,
    title TEXT,
    excerpt TEXT,
    published_at TEXT,
    retrieved_at INTEGER NOT NULL,
    content_type TEXT,
    bytes INTEGER NOT NULL DEFAULT 0,
    sha256 TEXT,
    text_chars INTEGER NOT NULL DEFAULT 0,
    expired INTEGER NOT NULL DEFAULT 0 CHECK (expired IN (0, 1)),
    query TEXT,
    call_id TEXT NOT NULL,
    text TEXT,
    body BLOB,
    PRIMARY KEY (session, id)
) STRICT;
CREATE INDEX web_sources_by_call ON web_sources(session, call_id);
"#;
// Version 13 records the policy a subagent was launched with (profile, tools, model,
// context), so enforcement never reads current settings for a running or recovering child.
const V13: &str = "ALTER TABLE subagent_tasks ADD COLUMN launch TEXT;";
// Version 14 adds engine-owned goals: one current goal per session, archived history and an
// event log. The desktop's own goal records are imported as paused through MigrateGoal.
const V14: &str = r#"
CREATE TABLE goals (
    session TEXT NOT NULL REFERENCES sessions(id),
    id INTEGER NOT NULL,
    objective TEXT NOT NULL,
    revision INTEGER NOT NULL DEFAULT 1,
    state TEXT NOT NULL CHECK (state IN ('active', 'paused', 'blocked', 'completed', 'failed', 'cancelled')),
    reason TEXT,
    epoch INTEGER NOT NULL DEFAULT 0,
    turns INTEGER NOT NULL DEFAULT 0,
    last_request TEXT,
    provenance TEXT NOT NULL,
    archived INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0, 1)),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (session, id)
) STRICT;
CREATE UNIQUE INDEX goals_current ON goals(session) WHERE archived = 0;
CREATE TABLE goal_events (
    session TEXT NOT NULL REFERENCES sessions(id),
    seq INTEGER NOT NULL,
    at_ms INTEGER NOT NULL,
    goal INTEGER NOT NULL,
    kind TEXT NOT NULL,
    data TEXT NOT NULL CHECK (json_valid(data)),
    PRIMARY KEY (session, seq)
) STRICT;
"#;
// Version 15 adds structured goal progress: criteria with evidence, limits, usage, stall
// detection and the executor's decision.
const V15: &str = r#"
ALTER TABLE goals ADD COLUMN criteria TEXT NOT NULL DEFAULT '[]';
ALTER TABLE goals ADD COLUMN limits TEXT NOT NULL DEFAULT '{}';
ALTER TABLE goals ADD COLUMN usage_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE goals ADD COLUMN turn_tokens_start INTEGER;
ALTER TABLE goals ADD COLUMN stall INTEGER NOT NULL DEFAULT 0;
ALTER TABLE goals ADD COLUMN signature TEXT;
ALTER TABLE goals ADD COLUMN note TEXT;
ALTER TABLE goals ADD COLUMN linked_tasks TEXT NOT NULL DEFAULT '[]';
ALTER TABLE goals ADD COLUMN decision TEXT;
"#;
// Version 16 adds the `verifying` goal state and the audit record. SQLite cannot change a
// CHECK constraint in place, so the table is rebuilt and its rows copied.
const V16: &str = r#"
CREATE TABLE goals_v16 (
    session TEXT NOT NULL REFERENCES sessions(id),
    id INTEGER NOT NULL,
    objective TEXT NOT NULL,
    revision INTEGER NOT NULL DEFAULT 1,
    state TEXT NOT NULL CHECK (state IN ('active', 'verifying', 'paused', 'blocked', 'completed', 'failed', 'cancelled')),
    reason TEXT,
    epoch INTEGER NOT NULL DEFAULT 0,
    turns INTEGER NOT NULL DEFAULT 0,
    last_request TEXT,
    provenance TEXT NOT NULL,
    archived INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0, 1)),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    criteria TEXT NOT NULL DEFAULT '[]',
    limits TEXT NOT NULL DEFAULT '{}',
    usage_tokens INTEGER NOT NULL DEFAULT 0,
    turn_tokens_start INTEGER,
    stall INTEGER NOT NULL DEFAULT 0,
    signature TEXT,
    note TEXT,
    linked_tasks TEXT NOT NULL DEFAULT '[]',
    decision TEXT,
    audit TEXT NOT NULL DEFAULT '{}',
    PRIMARY KEY (session, id)
) STRICT;
INSERT INTO goals_v16(session, id, objective, revision, state, reason, epoch, turns, last_request,
    provenance, archived, created_at, updated_at, criteria, limits, usage_tokens, turn_tokens_start,
    stall, signature, note, linked_tasks, decision)
SELECT session, id, objective, revision, state, reason, epoch, turns, last_request, provenance,
    archived, created_at, updated_at, criteria, limits, usage_tokens, turn_tokens_start, stall,
    signature, note, linked_tasks, decision FROM goals;
DROP TABLE goals;
ALTER TABLE goals_v16 RENAME TO goals;
CREATE UNIQUE INDEX goals_current ON goals(session) WHERE archived = 0;
"#;
const V17: &str = r#"
CREATE TABLE jobs (
    session TEXT NOT NULL REFERENCES sessions(id),
    id INTEGER NOT NULL,
    name TEXT,
    command TEXT NOT NULL,
    cwd TEXT NOT NULL,
    origin TEXT NOT NULL CHECK (origin IN ('model', 'user')),
    call_id TEXT,
    run INTEGER,
    task INTEGER,
    timeout_secs REAL,
    wake INTEGER NOT NULL DEFAULT 0 CHECK (wake IN (0, 1)),
    state TEXT NOT NULL CHECK (state IN ('starting', 'running', 'cancelling', 'succeeded', 'failed',
        'timed_out', 'cancelled', 'interrupted', 'unknown')),
    exit_code INTEGER,
    reason TEXT,
    engine_epoch INTEGER NOT NULL,
    pid INTEGER,
    nonce TEXT NOT NULL,
    log_path TEXT,
    log_bytes INTEGER NOT NULL DEFAULT 0,
    log_truncated INTEGER NOT NULL DEFAULT 0 CHECK (log_truncated IN (0, 1)),
    delivery TEXT NOT NULL DEFAULT 'pending' CHECK (delivery IN ('pending', 'notified', 'woken', 'suppressed', 'none')),
    created_at INTEGER NOT NULL,
    started_at INTEGER,
    ended_at INTEGER,
    updated_at INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (session, id)
) STRICT;
CREATE INDEX jobs_active ON jobs(session, state);
"#;
// SUB-2/SUB-3: who launched a child and how its completion is delivered. `inline` children
// answer their parent's waiting tool call; `notify` and `wake` are asynchronous policies that
// become `notified`, `woken` or `suppressed` once delivered. `note` records a person's Stop.
const V18: &str = r#"
ALTER TABLE subagent_tasks ADD COLUMN origin TEXT NOT NULL DEFAULT 'model';
ALTER TABLE subagent_tasks ADD COLUMN delivery TEXT NOT NULL DEFAULT 'inline';
ALTER TABLE subagent_tasks ADD COLUMN note TEXT;
"#;
// OBS-1: bounded structured diagnostics per session (provider timing, retries, tool
// durations, queue waits, settlements), written in the transactions that commit the facts.
const V19: &str = r#"
CREATE TABLE diagnostics (
    session TEXT NOT NULL REFERENCES sessions(id),
    seq INTEGER NOT NULL,
    at INTEGER NOT NULL,
    kind TEXT NOT NULL,
    run INTEGER,
    request TEXT,
    call_id TEXT,
    record TEXT NOT NULL CHECK (json_valid(record)),
    PRIMARY KEY (session, seq)
) STRICT;
"#;
const MIGRATIONS: &[&str] = &[
    V1, V2, V3, "", "", V6, V7, V8, V9, V10, V11, V12, V13, V14, V15, V16, V17, V18, V19,
];

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
