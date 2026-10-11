//! Durable engine state in SQLite.
//!
//! One database per engine data root, opened by exactly one process (an OS lock on a sibling
//! file). `WAL` + `synchronous=FULL`: an acknowledged write has been fsynced. Schema versions are
//! explicit; a newer schema is refused untouched, and migrations run inside one transaction
//! after a consistent backup.
//!
//! Authoritative records: `sessions`, `entries`, `requests` (admission ledger and queue),
//! `runs`, `tool_calls`, `tool_outputs`, `usage`, `subagent_tasks`, `kv`, `import_map`.
//! Rebuildable projections: none yet (views are computed from the authoritative tables).
//!
//! No network, process or person is awaited while a transaction is open: callers prepare
//! everything, then commit in one short transaction.

mod schema;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use pipkin_engine_protocol::limits::{MAX_QUEUE, MAX_RETAINED_OUTPUT_BYTES, TOOL_PREVIEW_BYTES};
use pipkin_engine_protocol::*;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::Value;

pub use schema::SCHEMA_VERSION;

#[derive(Debug)]
pub enum StoreError {
    /// The database was written by a newer engine. Nothing was changed.
    NewerSchema(u32),
    /// Another process owns this data root.
    Locked(PathBuf),
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
    Corrupt(String),
    /// A test asked for this write to fail.
    Injected,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::NewerSchema(v) => write!(
                f,
                "the engine data was written by a newer version (schema {v}); this version only \
                 understands schema {SCHEMA_VERSION} and has not changed it"
            ),
            StoreError::Locked(p) => {
                write!(f, "another engine process owns {}", p.display())
            }
            StoreError::Sqlite(e) => write!(f, "database: {e}"),
            StoreError::Io(e) => write!(f, "i/o: {e}"),
            StoreError::Corrupt(why) => write!(f, "corrupt engine data: {why}"),
            StoreError::Injected => f.write_str("injected storage failure"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Sqlite(e)
    }
}

impl From<StoreError> for ProtoError {
    fn from(e: StoreError) -> Self {
        ProtoError::new(ErrorCode::Storage, e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Holds the OS lock on the data root for as long as the store is open.
struct DirLock {
    _file: std::fs::File,
}

#[cfg(unix)]
fn lock_dir(dir: &Path) -> Result<DirLock> {
    use std::os::unix::io::AsRawFd;
    let path = dir.join("engine.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(StoreError::Io)?;
    // SAFETY: flock on a file descriptor we own.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        return Err(StoreError::Locked(dir.to_path_buf()));
    }
    Ok(DirLock { _file: file })
}

#[cfg(not(unix))]
fn lock_dir(dir: &Path) -> Result<DirLock> {
    let path = dir.join("engine.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(StoreError::Io)?;
    Ok(DirLock { _file: file })
}

pub struct Store {
    conn: Mutex<Connection>,
    path: PathBuf,
    fail_next: AtomicBool,
    _lock: Option<DirLock>,
}

/// A session row as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionRow {
    pub id: String,
    pub cwd: String,
    pub title: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub origin: Origin,
    pub model: Option<String>,
    pub thinking: Option<String>,
    pub subagents_enabled: bool,
    pub parent: Option<String>,
    pub root: Option<String>,
    pub revision: u64,
    pub blocked: Option<String>,
}

impl SessionRow {
    pub fn summary(&self) -> SessionSummary {
        SessionSummary {
            id: self.id.clone(),
            cwd: self.cwd.clone(),
            title: self.title.clone(),
            created_at_ms: self.created_at_ms,
            updated_at_ms: self.updated_at_ms,
            origin: self.origin.clone(),
            model: self.model.clone(),
            blocked: self.blocked.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestState {
    Queued,
    Running,
    Settled,
    Withdrawn,
}

impl RequestState {
    fn as_str(self) -> &'static str {
        match self {
            RequestState::Queued => "queued",
            RequestState::Running => "running",
            RequestState::Settled => "settled",
            RequestState::Withdrawn => "withdrawn",
        }
    }

    fn parse(s: &str) -> RequestState {
        match s {
            "queued" => RequestState::Queued,
            "running" => RequestState::Running,
            "withdrawn" => RequestState::Withdrawn,
            _ => RequestState::Settled,
        }
    }
}

/// One admitted input (prompt, steer or follow-up) under its durable key.
#[derive(Clone, Debug, PartialEq)]
pub struct RequestRow {
    pub session: String,
    pub key: String,
    pub fingerprint: String,
    pub mode: InputMode,
    pub state: RequestState,
    pub queue_entry: Option<u64>,
    pub run: Option<u64>,
    pub outcome: Option<RunOutcome>,
    pub text: String,
    pub images: Vec<ImageInput>,
    pub model: Option<String>,
    pub created_at_ms: i64,
}

impl RequestRow {
    pub fn lookup(&self) -> LookupResult {
        LookupResult::Known {
            mode: self.mode,
            entry: self.queue_entry,
            run: self.run,
            open: matches!(self.state, RequestState::Queued | RequestState::Running),
            outcome: self.outcome.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Admission {
    New(RequestRow),
    /// The same key and payload was admitted before.
    Existing(RequestRow),
    /// The key exists with a different payload.
    Conflict,
    /// The queue is at its limit.
    Full,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    Running,
    Stopping,
    Settled,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunRow {
    pub id: u64,
    pub session: String,
    pub request: Option<String>,
    pub state: RunState,
    pub stop_requested: bool,
    pub outcome: Option<RunOutcome>,
    pub started_at_ms: i64,
}

impl RunRow {
    pub fn view(&self) -> Option<RunView> {
        let phase = match self.state {
            RunState::Running if !self.stop_requested => RunPhase::Running,
            RunState::Running | RunState::Stopping => RunPhase::Stopping,
            RunState::Settled => return None,
        };
        Some(RunView {
            id: self.id,
            request: self.request.clone(),
            phase,
            started_at_ms: self.started_at_ms,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolState {
    /// Intent committed; the effect may or may not have started.
    Intent,
    Done,
    /// The engine stopped before a result was committed; effects are unconfirmed.
    Interrupted,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolCallRow {
    pub session: String,
    pub call_id: String,
    pub run: Option<u64>,
    pub name: String,
    pub arguments: Value,
    pub state: ToolState,
    pub replay_safe: bool,
    pub result_entry: Option<u64>,
}

/// A child task created by a subagent tool call.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskRow {
    pub root: String,
    pub child: u64,
    pub parent_session: String,
    pub parent_call: String,
    pub child_session: String,
    pub task: String,
    pub status: String,
    pub result: Option<String>,
    pub result_delivered: bool,
    pub created_at_ms: i64,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("engine records serialize")
}

fn parse_json<T: serde::de::DeserializeOwned>(raw: &str, what: &str) -> Result<T> {
    serde_json::from_str(raw).map_err(|e| StoreError::Corrupt(format!("{what}: {e}")))
}

fn mode_str(mode: InputMode) -> &'static str {
    match mode {
        InputMode::Prompt => "prompt",
        InputMode::Steer => "steer",
        InputMode::FollowUp => "followUp",
    }
}

fn parse_mode(s: &str) -> InputMode {
    match s {
        "steer" => InputMode::Steer,
        "followUp" => InputMode::FollowUp,
        _ => InputMode::Prompt,
    }
}

/// A stable digest of an input's payload, for same-key conflict detection.
pub fn fingerprint(mode: InputMode, text: &str, images: &[ImageInput]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(mode_str(mode).as_bytes());
    h.update([0]);
    h.update((text.len() as u64).to_be_bytes());
    h.update(text.as_bytes());
    for image in images {
        h.update(image.mime_type.as_bytes());
        h.update([0]);
        h.update((image.data.len() as u64).to_be_bytes());
        h.update(image.data.as_bytes());
    }
    let digest = h.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Cut `text` to at most `max` bytes at a char boundary.
pub fn clip(text: &str, max: usize) -> (&str, bool) {
    if text.len() <= max {
        return (text, false);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}

impl Store {
    /// Open (creating) the store under `dir`, holding the directory lock.
    pub fn open(dir: &Path) -> Result<Store> {
        std::fs::create_dir_all(dir).map_err(StoreError::Io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
        let lock = lock_dir(dir)?;
        let path = dir.join("engine.sqlite");
        let conn = schema::open(&path)?;
        Ok(Store {
            conn: Mutex::new(conn),
            path,
            fail_next: AtomicBool::new(false),
            _lock: Some(lock),
        })
    }

    /// A store in memory, for tests and the headless harness.
    pub fn memory() -> Result<Store> {
        let conn = Connection::open_in_memory()?;
        schema::configure(&conn, false)?;
        schema::migrate(&conn, None)?;
        Ok(Store {
            conn: Mutex::new(conn),
            path: PathBuf::from(":memory:"),
            fail_next: AtomicBool::new(false),
            _lock: None,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Make the next transaction fail before commit (fault injection).
    pub fn fail_next_write(&self) {
        self.fail_next.store(true, Ordering::SeqCst);
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Run `f` in one immediate transaction and commit. Nothing is published by callers until
    /// this returns `Ok`.
    pub fn tx<T>(&self, f: impl FnOnce(&Transaction) -> Result<T>) -> Result<T> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let value = f(&tx)?;
        if self.fail_next.swap(false, Ordering::SeqCst) {
            // Dropping the transaction rolls it back.
            return Err(StoreError::Injected);
        }
        tx.commit()?;
        Ok(value)
    }

    /// Read-only access (still serialized with writers; reads are short).
    pub fn read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = self.conn();
        f(&conn)
    }

    pub fn integrity_check(&self) -> Result<()> {
        self.read(|c| {
            let ok: String = c.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
            if ok == "ok" {
                Ok(())
            } else {
                Err(StoreError::Corrupt(ok))
            }
        })
    }

    /// Consistent online backup (includes WAL content).
    pub fn backup_to(&self, dest: &Path) -> Result<()> {
        let conn = self.conn();
        let mut target = Connection::open(dest)?;
        let backup = rusqlite::backup::Backup::new(&conn, &mut target)?;
        backup.run_to_completion(256, std::time::Duration::from_millis(0), None)?;
        Ok(())
    }

    // ------------------------------------------------------------------ meta

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        self.read(|c| {
            Ok(
                c.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
                    .optional()?,
            )
        })
    }

    /// Bump and return the engine epoch (one per engine start).
    pub fn next_epoch(&self) -> Result<u64> {
        self.tx(|tx| {
            let current: Option<String> = tx
                .query_row("SELECT value FROM meta WHERE key = 'epoch'", [], |r| {
                    r.get(0)
                })
                .optional()?;
            let next = current.and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) + 1;
            tx.execute(
                "INSERT INTO meta(key, value) VALUES('epoch', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [next.to_string()],
            )?;
            Ok(next)
        })
    }

    // ------------------------------------------------------------------ sessions

    fn session_from_row(r: &rusqlite::Row) -> rusqlite::Result<SessionRow> {
        let origin: String = r.get("origin")?;
        Ok(SessionRow {
            id: r.get("id")?,
            cwd: r.get("cwd")?,
            title: r.get("title")?,
            created_at_ms: r.get("created_at")?,
            updated_at_ms: r.get("updated_at")?,
            origin: serde_json::from_str(&origin).unwrap_or_default(),
            model: r.get("model")?,
            thinking: r.get("thinking")?,
            subagents_enabled: r.get::<_, i64>("subagents_enabled")? != 0,
            parent: r.get("parent")?,
            root: r.get("root")?,
            revision: r.get::<_, i64>("revision")? as u64,
            blocked: r.get("blocked")?,
        })
    }

    /// Create a session, or return the one created earlier under the same key.
    pub fn create_session(
        &self,
        id: &str,
        cwd: &str,
        key: Option<&str>,
        origin: &Origin,
        model: Option<&str>,
        thinking: Option<&str>,
        parent: Option<(&str, &str)>,
        now: i64,
    ) -> Result<(SessionRow, bool)> {
        self.tx(|tx| {
            if let Some(key) = key
                && let Some(existing) = tx
                    .query_row(
                        "SELECT * FROM sessions WHERE create_key = ?1",
                        [key],
                        Self::session_from_row,
                    )
                    .optional()?
            {
                return Ok((existing, false));
            }
            tx.execute(
                "INSERT INTO sessions(id, cwd, title, created_at, updated_at, origin, model,
                    thinking, subagents_enabled, parent, root, create_key, revision)
                 VALUES (?1, ?2, NULL, ?3, ?3, ?4, ?5, ?6, 1, ?7, ?8, ?9, 0)",
                params![
                    id,
                    cwd,
                    now,
                    json(origin),
                    model,
                    thinking,
                    parent.map(|p| p.0),
                    parent.map(|p| p.1),
                    key
                ],
            )?;
            let row = tx.query_row(
                "SELECT * FROM sessions WHERE id = ?1",
                [id],
                Self::session_from_row,
            )?;
            Ok((row, true))
        })
    }

    pub fn session(&self, id: &str) -> Result<Option<SessionRow>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT * FROM sessions WHERE id = ?1",
                [id],
                Self::session_from_row,
            )
            .optional()?)
        })
    }

    /// Top-level sessions, most recently updated first.
    pub fn root_sessions(&self) -> Result<Vec<SessionRow>> {
        self.read(|c| {
            let mut stmt = c.prepare(
                "SELECT * FROM sessions WHERE parent IS NULL ORDER BY updated_at DESC, id",
            )?;
            let rows = stmt
                .query_map([], Self::session_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    pub fn all_sessions(&self) -> Result<Vec<SessionRow>> {
        self.read(|c| {
            let mut stmt = c.prepare("SELECT * FROM sessions ORDER BY created_at, id")?;
            let rows = stmt
                .query_map([], Self::session_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Increment and return the session's revision; touches `updated_at`.
    pub fn bump(tx: &Connection, session: &str, now: i64) -> Result<u64> {
        tx.execute(
            "UPDATE sessions SET revision = revision + 1, updated_at = MAX(updated_at, ?2)
             WHERE id = ?1",
            params![session, now],
        )?;
        Ok(tx.query_row(
            "SELECT revision FROM sessions WHERE id = ?1",
            [session],
            |r| r.get::<_, i64>(0),
        )? as u64)
    }

    pub fn set_session_field(&self, session: &str, field: SessionField) -> Result<u64> {
        self.tx(|tx| {
            match &field {
                SessionField::Title(t) => tx.execute(
                    "UPDATE sessions SET title = ?2 WHERE id = ?1",
                    params![session, t],
                )?,
                SessionField::Model(m) => tx.execute(
                    "UPDATE sessions SET model = ?2 WHERE id = ?1",
                    params![session, m],
                )?,
                SessionField::Thinking(t) => tx.execute(
                    "UPDATE sessions SET thinking = ?2 WHERE id = ?1",
                    params![session, t],
                )?,
                SessionField::SubagentsEnabled(e) => tx.execute(
                    "UPDATE sessions SET subagents_enabled = ?2 WHERE id = ?1",
                    params![session, *e as i64],
                )?,
                SessionField::Blocked(b) => tx.execute(
                    "UPDATE sessions SET blocked = ?2 WHERE id = ?1",
                    params![session, b],
                )?,
            };
            Store::bump(tx, session, now_ms())
        })
    }

    // ------------------------------------------------------------------ entries

    fn entry_from_row(r: &rusqlite::Row) -> rusqlite::Result<(Entry, bool)> {
        let body: String = r.get("body")?;
        let kind: EntryKind = serde_json::from_str(&body).unwrap_or_else(|e| EntryKind::Notice {
            level: NoticeLevel::Error,
            text: format!("This entry could not be read ({e})."),
        });
        Ok((
            Entry {
                id: r.get::<_, i64>("id")? as u64,
                at_ms: r.get("at_ms")?,
                run: r.get::<_, Option<i64>>("run")?.map(|v| v as u64),
                retired: r.get::<_, i64>("retired")? != 0,
                kind,
            },
            r.get::<_, i64>("partial")? != 0,
        ))
    }

    /// Append an entry, assigning the next id in the session.
    pub fn append_entry(
        tx: &Connection,
        session: &str,
        at_ms: i64,
        run: Option<u64>,
        kind: &EntryKind,
    ) -> Result<Entry> {
        let next: i64 = tx.query_row(
            "SELECT COALESCE(MAX(id), 0) + 1 FROM entries WHERE session = ?1",
            [session],
            |r| r.get(0),
        )?;
        let next = next.max(Self::entry_floor(tx, session)? as i64);
        Self::insert_entry(tx, session, next as u64, at_ms, run, kind, false)
    }

    /// Imported sessions keep their legacy entry ids; native ids continue above them.
    fn entry_floor(tx: &Connection, session: &str) -> Result<u64> {
        Ok(tx
            .query_row(
                "SELECT value FROM kv WHERE session = ?1 AND key = 'entry_floor'",
                [session],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .and_then(|v| v.parse().ok())
            .unwrap_or(1))
    }

    pub fn insert_entry(
        tx: &Connection,
        session: &str,
        id: u64,
        at_ms: i64,
        run: Option<u64>,
        kind: &EntryKind,
        retired: bool,
    ) -> Result<Entry> {
        let partial = matches!(kind, EntryKind::Message { partial: true, .. });
        tx.execute(
            "INSERT INTO entries(session, id, at_ms, run, retired, partial, kind, body)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                session,
                id as i64,
                at_ms,
                run.map(|r| r as i64),
                retired as i64,
                partial as i64,
                entry_kind_name(kind),
                json(kind)
            ],
        )?;
        Ok(Entry {
            id,
            at_ms,
            run,
            retired,
            kind: kind.clone(),
        })
    }

    /// Replace an entry's body (finalizing a streamed message); drops its stream chunks.
    pub fn replace_entry(tx: &Connection, session: &str, entry: &Entry) -> Result<()> {
        let partial = matches!(entry.kind, EntryKind::Message { partial: true, .. });
        let n = tx.execute(
            "UPDATE entries SET body = ?3, partial = ?4, retired = ?5, kind = ?6
             WHERE session = ?1 AND id = ?2",
            params![
                session,
                entry.id as i64,
                json(&entry.kind),
                partial as i64,
                entry.retired as i64,
                entry_kind_name(&entry.kind)
            ],
        )?;
        if n != 1 {
            return Err(StoreError::Corrupt(format!(
                "entry {} of {session} is missing",
                entry.id
            )));
        }
        if !partial {
            tx.execute(
                "DELETE FROM entry_chunks WHERE session = ?1 AND entry = ?2",
                params![session, entry.id as i64],
            )?;
        }
        Ok(())
    }

    /// Append a committed streaming delta (linear: no rewrite of the growing message).
    pub fn append_chunks(
        tx: &Connection,
        session: &str,
        entry: u64,
        deltas: &[Delta],
    ) -> Result<()> {
        let mut seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM entry_chunks WHERE session = ?1 AND entry = ?2",
            params![session, entry as i64],
            |r| r.get(0),
        )?;
        let mut stmt = tx.prepare_cached(
            "INSERT INTO entry_chunks(session, entry, seq, delta) VALUES (?1, ?2, ?3, ?4)",
        )?;
        for delta in deltas {
            seq += 1;
            stmt.execute(params![session, entry as i64, seq, json(delta)])?;
        }
        Ok(())
    }

    fn chunks(c: &Connection, session: &str, entry: u64) -> Result<Vec<Delta>> {
        let mut stmt = c.prepare_cached(
            "SELECT delta FROM entry_chunks WHERE session = ?1 AND entry = ?2 ORDER BY seq",
        )?;
        let raw = stmt
            .query_map(params![session, entry as i64], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        raw.iter().map(|d| parse_json(d, "stream chunk")).collect()
    }

    /// An entry with any committed stream chunks applied.
    fn materialize(c: &Connection, session: &str, entry: Entry, partial: bool) -> Result<Entry> {
        if !partial {
            return Ok(entry);
        }
        let deltas = Self::chunks(c, session, entry.id)?;
        Ok(apply_deltas(entry, &deltas))
    }

    pub fn entry(&self, session: &str, id: u64) -> Result<Option<Entry>> {
        self.read(|c| Self::entry_in(c, session, id))
    }

    pub fn entry_in(c: &Connection, session: &str, id: u64) -> Result<Option<Entry>> {
        let row = c
            .query_row(
                "SELECT * FROM entries WHERE session = ?1 AND id = ?2",
                params![session, id as i64],
                Self::entry_from_row,
            )
            .optional()?;
        match row {
            Some((entry, partial)) => Ok(Some(Self::materialize(c, session, entry, partial)?)),
            None => Ok(None),
        }
    }

    /// Entries strictly before `before` (newest first internally), returned oldest first.
    pub fn page(
        &self,
        session: &str,
        before: Option<u64>,
        limit: u32,
    ) -> Result<(Vec<Entry>, bool)> {
        self.read(|c| {
            let before = before.map_or(i64::MAX, |b| b as i64);
            let mut stmt = c.prepare_cached(
                "SELECT * FROM entries WHERE session = ?1 AND id < ?2 ORDER BY id DESC LIMIT ?3",
            )?;
            let rows = stmt
                .query_map(
                    params![session, before, limit as i64 + 1],
                    Self::entry_from_row,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let has_older = rows.len() > limit as usize;
            let mut out = Vec::with_capacity(rows.len().min(limit as usize));
            for (entry, partial) in rows.into_iter().take(limit as usize) {
                out.push(Self::materialize(c, session, entry, partial)?);
            }
            out.reverse();
            Ok((out, has_older))
        })
    }

    /// The live window: newest entries, starting no earlier than the newest head marker
    /// (compaction or reset). Older history is pageable.
    pub fn window(&self, session: &str, limit: u32) -> Result<(Vec<Entry>, bool)> {
        let marker = self.read(|c| Self::last_head(c, session))?;
        let (mut entries, mut has_older) = self.page(session, None, limit)?;
        if let Some((marker_id, _)) = marker
            && let Some(pos) = entries.iter().position(|e| e.id == marker_id)
        {
            if pos > 0 {
                has_older = true;
            }
            entries.drain(..pos);
        }
        Ok((entries, has_older))
    }

    /// The newest head marker and where its context range starts: a compaction keeps entries
    /// from `first_kept`; a reset starts after itself.
    pub fn last_head(c: &Connection, session: &str) -> Result<Option<(u64, u64)>> {
        let row: Option<(i64, String)> = c
            .query_row(
                "SELECT id, body FROM entries WHERE session = ?1 AND kind IN ('compaction', 'reset')
                 ORDER BY +id DESC LIMIT 1",
                [session],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match row {
            Some((id, body)) => match parse_json::<EntryKind>(&body, "head marker")? {
                EntryKind::Compaction { first_kept, .. } => Ok(Some((id as u64, first_kept))),
                EntryKind::Reset { .. } => Ok(Some((id as u64, id as u64 + 1))),
                _ => Ok(None),
            },
            None => Ok(None),
        }
    }

    /// The raw context range: the newest head marker (if any) followed by every later-range
    /// entry that is committed (not partial) and is not itself a head marker. Retirement and
    /// message-level filtering happen in the context builder.
    pub fn context_entries(&self, session: &str) -> Result<Vec<Entry>> {
        self.read(|c| {
            let marker = Self::last_head(c, session)?;
            let mut out = Vec::new();
            let from = match marker {
                Some((marker_id, from)) => {
                    if let Some(entry) = Self::entry_in(c, session, marker_id)? {
                        out.push(entry);
                    }
                    from
                }
                None => 0,
            };
            let mut stmt = c.prepare_cached(
                "SELECT * FROM entries WHERE session = ?1 AND id >= ?2 AND partial = 0
                 AND kind NOT IN ('compaction', 'reset') ORDER BY id",
            )?;
            let rows = stmt
                .query_map(params![session, from as i64], Self::entry_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            out.extend(rows.into_iter().map(|(entry, _)| entry));
            Ok(out)
        })
    }

    /// Partial (streaming) entries, materialized; used by recovery.
    pub fn partial_entries(&self, session: &str) -> Result<Vec<Entry>> {
        self.read(|c| {
            let mut stmt =
                c.prepare("SELECT * FROM entries WHERE session = ?1 AND partial = 1 ORDER BY id")?;
            let rows = stmt
                .query_map([session], Self::entry_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter()
                .map(|(e, p)| Self::materialize(c, session, e, p))
                .collect()
        })
    }

    pub fn retire_entries(tx: &Connection, session: &str, ids: &[u64]) -> Result<()> {
        let mut stmt = tx.prepare_cached(
            "UPDATE entries SET retired = 1, body = json_set(body, '$.retired', json('true'))
             WHERE session = ?1 AND id = ?2",
        )?;
        for id in ids {
            stmt.execute(params![session, *id as i64])?;
        }
        Ok(())
    }

    pub fn entries_of_run(c: &Connection, session: &str, run: u64) -> Result<Vec<Entry>> {
        let mut stmt =
            c.prepare_cached("SELECT * FROM entries WHERE session = ?1 AND run = ?2 ORDER BY id")?;
        let rows = stmt
            .query_map(params![session, run as i64], Self::entry_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(e, p)| Self::materialize(c, session, e, p))
            .collect()
    }

    pub fn entry_count(&self, session: &str) -> Result<u64> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM entries WHERE session = ?1",
                [session],
                |r| r.get::<_, i64>(0),
            )? as u64)
        })
    }

    // ------------------------------------------------------------------ requests and queue

    fn request_from_row(r: &rusqlite::Row) -> rusqlite::Result<RequestRow> {
        let outcome: Option<String> = r.get("outcome")?;
        let images: String = r.get("images")?;
        let state: String = r.get("state")?;
        let mode: String = r.get("mode")?;
        Ok(RequestRow {
            session: r.get("session")?,
            key: r.get("key")?,
            fingerprint: r.get("fingerprint")?,
            mode: parse_mode(&mode),
            state: RequestState::parse(&state),
            queue_entry: r.get::<_, Option<i64>>("queue_entry")?.map(|v| v as u64),
            run: r.get::<_, Option<i64>>("run")?.map(|v| v as u64),
            outcome: outcome.and_then(|o| serde_json::from_str(&o).ok()),
            text: r.get("text")?,
            images: serde_json::from_str(&images).unwrap_or_default(),
            model: r.get("model")?,
            created_at_ms: r.get("created_at")?,
        })
    }

    pub fn request_in(c: &Connection, session: &str, key: &str) -> Result<Option<RequestRow>> {
        Ok(c.query_row(
            "SELECT * FROM requests WHERE session = ?1 AND key = ?2",
            params![session, key],
            Self::request_from_row,
        )
        .optional()?)
    }

    pub fn request(&self, session: &str, key: &str) -> Result<Option<RequestRow>> {
        self.read(|c| Self::request_in(c, session, key))
    }

    /// Admit an input under its key, in the caller's transaction. A prompt is `Running` (its
    /// run is started by the caller in the same transaction); steers/follow-ups are `Queued`.
    pub fn admit(
        tx: &Connection,
        session: &str,
        key: &str,
        mode: InputMode,
        text: &str,
        images: &[ImageInput],
        model: Option<&str>,
        now: i64,
    ) -> Result<Admission> {
        let fp = fingerprint(mode, text, images);
        if let Some(existing) = Self::request_in(tx, session, key)? {
            return Ok(if existing.fingerprint == fp {
                Admission::Existing(existing)
            } else {
                Admission::Conflict
            });
        }
        let (state, queue_entry) = match mode {
            InputMode::Prompt => (RequestState::Running, None),
            InputMode::Steer | InputMode::FollowUp => {
                let queued: i64 = tx.query_row(
                    "SELECT COUNT(*) FROM requests WHERE session = ?1 AND state = 'queued'",
                    [session],
                    |r| r.get(0),
                )?;
                if queued as usize >= MAX_QUEUE {
                    return Ok(Admission::Full);
                }
                let next: i64 = tx.query_row(
                    "SELECT COALESCE(MAX(queue_entry), 0) + 1 FROM requests WHERE session = ?1",
                    [session],
                    |r| r.get(0),
                )?;
                (RequestState::Queued, Some(next as u64))
            }
        };
        tx.execute(
            "INSERT INTO requests(session, key, fingerprint, mode, state, queue_entry, run,
                outcome, text, images, model, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, ?7, ?8, ?9, ?10)",
            params![
                session,
                key,
                fp,
                mode_str(mode),
                state.as_str(),
                queue_entry.map(|v| v as i64),
                text,
                json(&images),
                model,
                now
            ],
        )?;
        Ok(Admission::New(
            Self::request_in(tx, session, key)?.expect("just inserted"),
        ))
    }

    pub fn set_request_run(tx: &Connection, session: &str, key: &str, run: u64) -> Result<()> {
        tx.execute(
            "UPDATE requests SET run = ?3, state = 'running' WHERE session = ?1 AND key = ?2",
            params![session, key, run as i64],
        )?;
        Ok(())
    }

    pub fn settle_request(
        tx: &Connection,
        session: &str,
        key: &str,
        outcome: &RunOutcome,
        now: i64,
    ) -> Result<()> {
        tx.execute(
            "UPDATE requests SET state = 'settled', outcome = ?3, settled_at = ?4
             WHERE session = ?1 AND key = ?2 AND state != 'withdrawn'",
            params![session, key, json(outcome), now],
        )?;
        Ok(())
    }

    /// The queue, oldest first.
    pub fn queue(&self, session: &str) -> Result<Vec<RequestRow>> {
        self.read(|c| Self::queue_in(c, session))
    }

    pub fn queue_in(c: &Connection, session: &str) -> Result<Vec<RequestRow>> {
        let mut stmt = c.prepare_cached(
            "SELECT * FROM requests WHERE session = ?1 AND state = 'queued' ORDER BY queue_entry",
        )?;
        let rows = stmt
            .query_map([session], Self::request_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn queue_view(rows: &[RequestRow]) -> Vec<QueueEntryView> {
        rows.iter()
            .map(|r| QueueEntryView {
                entry: r.queue_entry.unwrap_or(0),
                request: r.key.clone(),
                mode: r.mode,
                text: r.text.clone(),
                images: r.images.len() as u32,
            })
            .collect()
    }

    /// Mark queued requests as taken by `run` (they are now part of it).
    pub fn consume_queued(tx: &Connection, session: &str, keys: &[String], run: u64) -> Result<()> {
        let mut stmt = tx.prepare_cached(
            "UPDATE requests SET state = 'running', run = ?3
             WHERE session = ?1 AND key = ?2 AND state = 'queued'",
        )?;
        for key in keys {
            stmt.execute(params![session, key, run as i64])?;
        }
        Ok(())
    }

    pub fn cancel_queued(&self, session: &str, entry: u64) -> Result<(CancelQueuedOutcome, u64)> {
        self.tx(|tx| {
            let row: Option<String> = tx
                .query_row(
                    "SELECT state FROM requests WHERE session = ?1 AND queue_entry = ?2",
                    params![session, entry as i64],
                    |r| r.get(0),
                )
                .optional()?;
            let outcome = match row.as_deref() {
                None => CancelQueuedOutcome::NotFound,
                Some("queued") => {
                    tx.execute(
                        "UPDATE requests SET state = 'withdrawn', outcome = ?3
                         WHERE session = ?1 AND queue_entry = ?2",
                        params![session, entry as i64, json(&RunOutcome::Aborted)],
                    )?;
                    CancelQueuedOutcome::Cancelled
                }
                Some("withdrawn") => CancelQueuedOutcome::NotFound,
                Some(_) => CancelQueuedOutcome::AlreadyConsumed,
            };
            let rev = Store::bump(tx, session, now_ms())?;
            Ok((outcome, rev))
        })
    }

    // ------------------------------------------------------------------ runs

    fn run_from_row(r: &rusqlite::Row) -> rusqlite::Result<RunRow> {
        let state: String = r.get("state")?;
        let outcome: Option<String> = r.get("outcome")?;
        Ok(RunRow {
            id: r.get::<_, i64>("id")? as u64,
            session: r.get("session")?,
            request: r.get("request")?,
            state: match state.as_str() {
                "running" => RunState::Running,
                "stopping" => RunState::Stopping,
                _ => RunState::Settled,
            },
            stop_requested: r.get::<_, Option<i64>>("stop_requested_at")?.is_some(),
            outcome: outcome.and_then(|o| serde_json::from_str(&o).ok()),
            started_at_ms: r.get("started_at")?,
        })
    }

    pub fn start_run(
        tx: &Connection,
        session: &str,
        request: Option<&str>,
        now: i64,
    ) -> Result<u64> {
        tx.execute(
            "INSERT INTO runs(session, request, state, started_at) VALUES (?1, ?2, 'running', ?3)",
            params![session, request, now],
        )?;
        let id = tx.last_insert_rowid() as u64;
        if let Some(key) = request {
            Self::set_request_run(tx, session, key, id)?;
        }
        Ok(id)
    }

    pub fn run(&self, id: u64) -> Result<Option<RunRow>> {
        self.read(|c| Self::run_in(c, id))
    }

    pub fn run_in(c: &Connection, id: u64) -> Result<Option<RunRow>> {
        Ok(c.query_row(
            "SELECT * FROM runs WHERE id = ?1",
            [id as i64],
            Self::run_from_row,
        )
        .optional()?)
    }

    pub fn open_run(&self, session: &str) -> Result<Option<RunRow>> {
        self.read(|c| Self::open_run_in(c, session))
    }

    pub fn open_run_in(c: &Connection, session: &str) -> Result<Option<RunRow>> {
        Ok(c.query_row(
            "SELECT * FROM runs WHERE session = ?1 AND state IN ('running', 'stopping') ORDER BY id DESC LIMIT 1",
            [session],
            Self::run_from_row,
        )
        .optional()?)
    }

    /// Every unsettled run, for recovery after a restart.
    pub fn unsettled_runs(&self) -> Result<Vec<RunRow>> {
        self.read(|c| {
            let mut stmt =
                c.prepare("SELECT * FROM runs WHERE state IN ('running', 'stopping') ORDER BY id")?;
            let rows = stmt
                .query_map([], Self::run_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Persist Stop intent before any cancellation is attempted.
    pub fn request_stop(&self, session: &str, now: i64) -> Result<Option<(u64, u64)>> {
        self.tx(|tx| {
            let Some(run) = Self::open_run_in(tx, session)? else {
                return Ok(None);
            };
            tx.execute(
                "UPDATE runs SET state = 'stopping', stop_requested_at = COALESCE(stop_requested_at, ?2)
                 WHERE id = ?1",
                params![run.id as i64, now],
            )?;
            let rev = Store::bump(tx, session, now)?;
            Ok(Some((run.id, rev)))
        })
    }

    pub fn settle_run(tx: &Connection, run: u64, outcome: &RunOutcome, now: i64) -> Result<()> {
        tx.execute(
            "UPDATE runs SET state = 'settled', outcome = ?2, ended_at = ?3 WHERE id = ?1",
            params![run as i64, json(outcome), now],
        )?;
        Ok(())
    }

    /// Requests a run took (its prompt and any steers/follow-ups it consumed).
    pub fn requests_of_run(c: &Connection, run: u64) -> Result<Vec<RequestRow>> {
        let mut stmt =
            c.prepare_cached("SELECT * FROM requests WHERE run = ?1 ORDER BY created_at")?;
        let rows = stmt
            .query_map([run as i64], Self::request_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    // ------------------------------------------------------------------ tools

    fn tool_from_row(r: &rusqlite::Row) -> rusqlite::Result<ToolCallRow> {
        let args: String = r.get("arguments")?;
        let state: String = r.get("state")?;
        Ok(ToolCallRow {
            session: r.get("session")?,
            call_id: r.get("call_id")?,
            run: r.get::<_, Option<i64>>("run")?.map(|v| v as u64),
            name: r.get("name")?,
            arguments: serde_json::from_str(&args).unwrap_or(Value::Null),
            state: match state.as_str() {
                "done" => ToolState::Done,
                "interrupted" => ToolState::Interrupted,
                _ => ToolState::Intent,
            },
            replay_safe: r.get::<_, i64>("replay_safe")? != 0,
            result_entry: r.get::<_, Option<i64>>("result_entry")?.map(|v| v as u64),
        })
    }

    /// Commit the intent to run a tool before any effect starts.
    pub fn tool_intent(
        tx: &Connection,
        session: &str,
        call_id: &str,
        run: Option<u64>,
        name: &str,
        arguments: &Value,
        replay_safe: bool,
        now: i64,
    ) -> Result<()> {
        tx.execute(
            "INSERT INTO tool_calls(session, call_id, run, name, arguments, state, replay_safe,
                started_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'intent', ?6, ?7)
             ON CONFLICT(session, call_id) DO NOTHING",
            params![
                session,
                call_id,
                run.map(|r| r as i64),
                name,
                json(arguments),
                replay_safe as i64,
                now
            ],
        )?;
        Ok(())
    }

    pub fn tool_call(&self, session: &str, call_id: &str) -> Result<Option<ToolCallRow>> {
        self.read(|c| Self::tool_call_in(c, session, call_id))
    }

    pub fn tool_call_in(
        c: &Connection,
        session: &str,
        call_id: &str,
    ) -> Result<Option<ToolCallRow>> {
        Ok(c.query_row(
            "SELECT * FROM tool_calls WHERE session = ?1 AND call_id = ?2",
            params![session, call_id],
            Self::tool_from_row,
        )
        .optional()?)
    }

    pub fn tools_in_state(c: &Connection, session: &str, state: &str) -> Result<Vec<ToolCallRow>> {
        let mut stmt = c.prepare_cached(
            "SELECT * FROM tool_calls WHERE session = ?1 AND state = ?2 ORDER BY started_at",
        )?;
        let rows = stmt
            .query_map(params![session, state], Self::tool_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Record a tool's settlement and its retained output.
    pub fn tool_result(
        tx: &Connection,
        session: &str,
        call_id: &str,
        state: ToolState,
        result_entry: u64,
        retained: &str,
        dropped: u64,
        now: i64,
    ) -> Result<()> {
        let state = match state {
            ToolState::Intent => "intent",
            ToolState::Done => "done",
            ToolState::Interrupted => "interrupted",
        };
        tx.execute(
            "UPDATE tool_calls SET state = ?3, result_entry = ?4, ended_at = ?5
             WHERE session = ?1 AND call_id = ?2",
            params![session, call_id, state, result_entry as i64, now],
        )?;
        let (kept, clipped) = clip(retained, MAX_RETAINED_OUTPUT_BYTES);
        let dropped = dropped
            + if clipped {
                (retained.len() - kept.len()) as u64
            } else {
                0
            };
        tx.execute(
            "INSERT INTO tool_outputs(session, call_id, text, retained, dropped)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(session, call_id) DO UPDATE SET text = excluded.text,
                retained = excluded.retained, dropped = excluded.dropped",
            params![session, call_id, kept, kept.len() as i64, dropped as i64],
        )?;
        Ok(())
    }

    pub fn tool_output(&self, session: &str, call_id: &str) -> Result<Option<ToolOutputView>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT text, retained, dropped FROM tool_outputs WHERE session = ?1 AND call_id = ?2",
                params![session, call_id],
                |r| {
                    Ok(ToolOutputView {
                        text: r.get(0)?,
                        retained_bytes: r.get::<_, i64>(1)? as u64,
                        dropped_bytes: r.get::<_, i64>(2)? as u64,
                    })
                },
            )
            .optional()?)
        })
    }

    // ------------------------------------------------------------------ usage

    pub fn add_usage(
        tx: &Connection,
        session: &str,
        bucket: &str,
        key: &str,
        usage: &Usage,
    ) -> Result<()> {
        let existing: Option<String> = tx
            .query_row(
                "SELECT value FROM usage WHERE session = ?1 AND bucket = ?2 AND key = ?3",
                params![session, bucket, key],
                |r| r.get(0),
            )
            .optional()?;
        let mut total: Usage = match existing {
            Some(raw) => parse_json(&raw, "usage")?,
            None => Usage::default(),
        };
        total.add(usage);
        tx.execute(
            "INSERT INTO usage(session, bucket, key, value) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(session, bucket, key) DO UPDATE SET value = excluded.value",
            params![session, bucket, key, json(&total)],
        )?;
        Ok(())
    }

    pub fn usage(&self, session: &str) -> Result<UsageView> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT bucket, key, value FROM usage WHERE session = ?1 ORDER BY bucket, key",
            )?;
            let rows = stmt
                .query_map([session], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut view = UsageView::default();
            for (bucket, key, raw) in rows {
                let usage: Usage = parse_json(&raw, "usage")?;
                if bucket == "tool" {
                    view.tools.push((key, usage));
                } else {
                    view.models.push((key, usage));
                }
            }
            Ok(view)
        })
    }

    // ------------------------------------------------------------------ kv (extension state)

    pub fn kv_get(&self, session: &str, key: &str) -> Result<Option<String>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT value FROM kv WHERE session = ?1 AND key = ?2",
                params![session, key],
                |r| r.get(0),
            )
            .optional()?)
        })
    }

    pub fn kv_set(tx: &Connection, session: &str, key: &str, value: &str) -> Result<()> {
        tx.execute(
            "INSERT INTO kv(session, key, value) VALUES (?1, ?2, ?3)
             ON CONFLICT(session, key) DO UPDATE SET value = excluded.value",
            params![session, key, value],
        )?;
        Ok(())
    }

    // ------------------------------------------------------------------ subagent tasks

    fn task_from_row(r: &rusqlite::Row) -> rusqlite::Result<TaskRow> {
        Ok(TaskRow {
            root: r.get("root")?,
            child: r.get::<_, i64>("child")? as u64,
            parent_session: r.get("parent_session")?,
            parent_call: r.get("parent_call")?,
            child_session: r.get("child_session")?,
            task: r.get("task")?,
            status: r.get("status")?,
            result: r.get("result")?,
            result_delivered: r.get::<_, i64>("result_delivered")? != 0,
            created_at_ms: r.get("created_at")?,
        })
    }

    /// The child task for a parent call, if it was created.
    pub fn task_for_call(
        c: &Connection,
        parent_session: &str,
        call: &str,
    ) -> Result<Option<TaskRow>> {
        Ok(c.query_row(
            "SELECT * FROM subagent_tasks WHERE parent_session = ?1 AND parent_call = ?2",
            params![parent_session, call],
            Self::task_from_row,
        )
        .optional()?)
    }

    pub fn task_by_child(c: &Connection, root: &str, child: u64) -> Result<Option<TaskRow>> {
        Ok(c.query_row(
            "SELECT * FROM subagent_tasks WHERE root = ?1 AND child = ?2",
            params![root, child as i64],
            Self::task_from_row,
        )
        .optional()?)
    }

    pub fn task_by_session(c: &Connection, child_session: &str) -> Result<Option<TaskRow>> {
        Ok(c.query_row(
            "SELECT * FROM subagent_tasks WHERE child_session = ?1",
            [child_session],
            Self::task_from_row,
        )
        .optional()?)
    }

    /// Create the child task and its parent-call mapping atomically. Returns the existing
    /// task when the call already has one (recovery never creates a second child).
    pub fn create_task(
        tx: &Connection,
        root: &str,
        parent_session: &str,
        parent_call: &str,
        child_session: &str,
        task: &str,
        now: i64,
    ) -> Result<(TaskRow, bool)> {
        if let Some(existing) = Self::task_for_call(tx, parent_session, parent_call)? {
            return Ok((existing, false));
        }
        let next: i64 = tx.query_row(
            "SELECT COALESCE(MAX(child), 0) + 1 FROM subagent_tasks WHERE root = ?1",
            [root],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO subagent_tasks(root, child, parent_session, parent_call, child_session,
                task, status, result, result_delivered, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', NULL, 0, ?7, ?7)",
            params![
                root,
                next,
                parent_session,
                parent_call,
                child_session,
                task,
                now
            ],
        )?;
        Ok((
            Self::task_by_child(tx, root, next as u64)?.expect("just inserted"),
            true,
        ))
    }

    pub fn set_task_status(
        tx: &Connection,
        child_session: &str,
        status: &str,
        result: Option<&str>,
        now: i64,
    ) -> Result<()> {
        tx.execute(
            "UPDATE subagent_tasks SET status = ?2, result = COALESCE(?3, result), updated_at = ?4
             WHERE child_session = ?1",
            params![child_session, status, result, now],
        )?;
        Ok(())
    }

    /// Mark the result delivered to the parent; false if it already was (dedup).
    pub fn mark_task_delivered(tx: &Connection, child_session: &str) -> Result<bool> {
        let n = tx.execute(
            "UPDATE subagent_tasks SET result_delivered = 1
             WHERE child_session = ?1 AND result_delivered = 0",
            [child_session],
        )?;
        Ok(n == 1)
    }

    pub fn tasks(&self, root: &str, limit: u32) -> Result<(Vec<TaskRow>, bool)> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT * FROM subagent_tasks WHERE root = ?1 ORDER BY child LIMIT ?2",
            )?;
            let rows = stmt
                .query_map(params![root, limit as i64 + 1], Self::task_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let truncated = rows.len() > limit as usize;
            Ok((rows.into_iter().take(limit as usize).collect(), truncated))
        })
    }

    pub fn all_tasks(&self) -> Result<Vec<TaskRow>> {
        self.read(|c| {
            let mut stmt = c.prepare("SELECT * FROM subagent_tasks ORDER BY root, child")?;
            let rows = stmt
                .query_map([], Self::task_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    // ------------------------------------------------------------------ import map

    pub fn imported(&self, engine: &str, source: &str) -> Result<Option<String>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT native_session FROM import_map WHERE source_engine = ?1 AND source_session = ?2",
                params![engine, source],
                |r| r.get(0),
            )
            .optional()?)
        })
    }

    pub fn record_import(
        tx: &Connection,
        engine: &str,
        source: &str,
        native: &str,
        manifest: &Value,
        now: i64,
    ) -> Result<()> {
        tx.execute(
            "INSERT INTO import_map(source_engine, source_session, native_session, manifest, imported_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![engine, source, native, json(manifest), now],
        )?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SessionField {
    Title(Option<String>),
    Model(Option<String>),
    Thinking(Option<String>),
    SubagentsEnabled(bool),
    Blocked(Option<String>),
}

fn entry_kind_name(kind: &EntryKind) -> &'static str {
    match kind {
        EntryKind::Message { .. } => "message",
        EntryKind::Compaction { .. } => "compaction",
        EntryKind::Reset { .. } => "reset",
        EntryKind::Notice { .. } => "notice",
        EntryKind::ModelChange { .. } => "modelChange",
        EntryKind::ThinkingChange { .. } => "thinkingChange",
        EntryKind::Custom { .. } => "custom",
    }
}

/// Apply committed stream deltas to a partial assistant message.
pub fn apply_deltas(mut entry: Entry, deltas: &[Delta]) -> Entry {
    if let EntryKind::Message {
        message: Message::Assistant { content, .. },
        ..
    } = &mut entry.kind
    {
        for delta in deltas {
            apply_delta(content, delta);
        }
    }
    entry
}

pub fn apply_delta(content: &mut Vec<pipkin_engine_protocol::Block>, delta: &Delta) {
    use pipkin_engine_protocol::Block;
    let index = match delta {
        Delta::Text { block, .. }
        | Delta::Thinking { block, .. }
        | Delta::ToolCall { block, .. } => *block as usize,
    };
    while content.len() <= index {
        content.push(match delta {
            Delta::Thinking { .. } => Block::Thinking {
                thinking: String::new(),
                signature: None,
                redacted: false,
                extra: Default::default(),
            },
            Delta::ToolCall { id, name, .. } => {
                Block::tool_call(id.clone(), name.clone(), Value::Null)
            }
            Delta::Text { .. } => Block::text(""),
        });
    }
    match (delta, &mut content[index]) {
        (Delta::Text { text, .. }, Block::Text { text: t, .. }) => t.push_str(text),
        (Delta::Thinking { text, .. }, Block::Thinking { thinking, .. }) => thinking.push_str(text),
        _ => {}
    }
}

/// A bounded preview of tool output, cut at a char boundary.
pub fn preview(text: &str) -> (String, bool) {
    let (head, cut) = clip(text, TOOL_PREVIEW_BYTES);
    (head.to_owned(), cut)
}

#[cfg(test)]
mod tests;
