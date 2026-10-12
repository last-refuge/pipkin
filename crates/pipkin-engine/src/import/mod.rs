//! Import of legacy engine profiles into native storage.
//!
//! The reference engine keeps one directory per session under
//! `<agentDir>/experimental/sessions/<id>/` holding `meta.json` (`{createdAt, cwd}`) and the
//! worker-owned `session.sqlite` (`coding-agent/src/experimental/session-catalog.ts`). A worker
//! owns a session through a `proper-lockfile` lock directory `<dir>.lock` whose mtime it
//! refreshes every second.
//!
//! [`discover`] is read-only. [`import_session`] refuses a locked session, takes a consistent
//! copy with the SQLite backup API into `backup_dir/<id>/`, checks it, reads only the copy, and
//! writes every native record inside ONE store transaction together with its `import_map`
//! row, so a failure leaves nothing behind and a rerun is a no-op that returns the mapping.
//! Sessions whose semantics the native engine cannot represent without guessing (forks,
//! `replace` context edits, unknown entry kinds that affect the model context, in-flight
//! work) are refused with their reasons.
//!
//! Credentials and configuration are never imported implicitly: [`credential_entries`] and
//! [`config_files`] serve an explicit, user-approved import by the caller.

mod context;
mod delta;
mod legacy;
mod plan;
mod plugins;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::{Map, Value, json};

use crate::store::{Store, StoreError};

pub use plan::{IN_FLIGHT, ROOT_CONVERSATION_ID, child_session_id};

/// The `Origin::Imported` engine name and `import_map.source_engine` of reference sessions.
pub const ENGINE: &str = "pi";
/// The source format discovery reports.
pub const SOURCE_FORMAT: &str = "pi-durable-sqlite";
/// A lock directory refreshed more recently than this is held by a live writer. The reference
/// uses stale windows of 2 s (session worker), 10 s and 30 s (`server.ts`); the largest is used.
pub const LOCK_STALE: Duration = Duration::from_secs(30);

const META_FILE: &str = "meta.json";
const STORAGE_FILE: &str = "session.sqlite";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionStatus {
    Importable,
    AlreadyImported {
        native: String,
    },
    /// A writer owns the session (fresh lock directory) or it could not be read consistently.
    Locked,
    Blocked {
        reasons: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiscoveredSession {
    pub id: String,
    pub path: PathBuf,
    pub cwd: Option<String>,
    pub created_at_ms: Option<i64>,
    pub sqlite_bytes: u64,
    pub wal_bytes: u64,
    pub meta_bytes: u64,
    pub source_format: &'static str,
    pub schema_version: Option<i64>,
    pub status: SessionStatus,
    /// Why the status is `Locked`, when known.
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigItem {
    /// `models.json`, `settings.json`, `AGENTS.md`, `CLAUDE.md`, `skills`, `prompts`, ...
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

/// One saved credential: provider id and kind only, never a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialInfo {
    pub provider: String,
    /// The stored `type` (`api_key`, `oauth`, ...).
    pub kind: String,
    /// Set when this engine will not import it.
    pub reserved: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiscoveryReport {
    pub agent_dir: PathBuf,
    pub sessions_dir: PathBuf,
    pub sessions: Vec<DiscoveredSession>,
    pub config: Vec<ConfigItem>,
    pub credentials: Vec<CredentialInfo>,
    /// `auth.json` exists but could not be read (no contents are included).
    pub credentials_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImportOutcome {
    pub native_session: String,
    /// Native child sessions created for subagent conversations.
    pub children: Vec<String>,
    /// True when an earlier import is returned and nothing was written.
    pub already_imported: bool,
    pub manifest: Value,
}

#[derive(Debug)]
pub enum ImportError {
    InvalidId(String),
    NotFound(String),
    Locked(String),
    Blocked(Vec<String>),
    /// A native session with this id exists and was not created by an import of it.
    Conflict(String),
    /// The source (or its backup) is not readable as a reference session store.
    Source(String),
    Io(String),
    Store(StoreError),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::InvalidId(id) => write!(f, "{id:?} is not a session id"),
            ImportError::NotFound(id) => write!(f, "session {id} was not found"),
            ImportError::Locked(why) => write!(f, "the session is in use: {why}"),
            ImportError::Blocked(reasons) => {
                write!(f, "the session cannot be imported: {}", reasons.join("; "))
            }
            ImportError::Conflict(why) | ImportError::Source(why) | ImportError::Io(why) => {
                f.write_str(why)
            }
            ImportError::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ImportError {}

impl From<StoreError> for ImportError {
    fn from(e: StoreError) -> Self {
        ImportError::Store(e)
    }
}

fn io(what: &str, path: &Path, e: std::io::Error) -> ImportError {
    ImportError::Io(format!("{what} {}: {e}", path.display()))
}

/// `session-catalog.ts:17`.
pub fn is_session_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

pub fn sessions_dir(agent_dir: &Path) -> PathBuf {
    agent_dir.join("experimental").join("sessions")
}

fn read_meta(dir: &Path) -> Result<plan::Meta, String> {
    let raw = std::fs::read_to_string(dir.join(META_FILE))
        .map_err(|e| format!("meta.json is unreadable ({e})"))?;
    let value: Value =
        serde_json::from_str(&raw).map_err(|e| format!("meta.json is not JSON ({e})"))?;
    let created_at = value
        .get("createdAt")
        .and_then(Value::as_f64)
        .ok_or("meta.json has no numeric createdAt")?;
    let cwd = value
        .get("cwd")
        .and_then(Value::as_str)
        .ok_or("meta.json has no cwd")?;
    Ok(plan::Meta {
        created_at: created_at as i64,
        cwd: cwd.to_owned(),
    })
}

fn lock_path(dir: &Path) -> PathBuf {
    let mut name = dir.as_os_str().to_owned();
    name.push(".lock");
    PathBuf::from(name)
}

/// A fresh `proper-lockfile` lock (`<realpath(dir)>.lock`), described.
fn fresh_lock(dir: &Path) -> Option<String> {
    let mut candidates = vec![lock_path(dir)];
    if let Ok(real) = std::fs::canonicalize(dir) {
        candidates.push(lock_path(&real));
    }
    for lock in candidates {
        let Ok(meta) = std::fs::metadata(&lock) else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            return Some(format!("{} exists", lock.display()));
        };
        match SystemTime::now().duration_since(modified) {
            Ok(age) if age >= LOCK_STALE => continue,
            Ok(age) => {
                return Some(format!(
                    "{} was refreshed {} ms ago",
                    lock.display(),
                    age.as_millis()
                ));
            }
            Err(_) => return Some(format!("{} is dated in the future", lock.display())),
        }
    }
    None
}

/// Size and mtime of the database and its WAL, to detect a change while reading.
fn stamp(sqlite: &Path) -> Vec<Option<(u64, Option<SystemTime>)>> {
    [sqlite.to_path_buf(), legacy::wal_path(sqlite)]
        .iter()
        .map(|p| {
            std::fs::metadata(p)
                .ok()
                .map(|m| (m.len(), m.modified().ok()))
        })
        .collect()
}

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn sha256_file(path: &Path) -> Result<String, ImportError> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| io("open", path, e))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| io("read", path, e))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn read_import(store: &Store, source: &str) -> Result<Option<(String, Value)>, StoreError> {
    store.read(|c| {
        let row: Option<(String, String)> = c
            .query_row(
                "SELECT native_session, manifest FROM import_map
                 WHERE source_engine = ?1 AND source_session = ?2",
                params![ENGINE, source],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(row.map(|(native, manifest)| {
            (
                native,
                serde_json::from_str(&manifest).unwrap_or(Value::Null),
            )
        }))
    })
}

fn children_of(manifest: &Value, native: &str) -> Vec<String> {
    manifest
        .get("conversations")
        .and_then(Value::as_object)
        .map(|m| {
            m.values()
                .filter_map(Value::as_str)
                .filter(|s| *s != native)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------- discovery

/// List every legacy session and the importable configuration and credentials. Read-only.
pub fn discover(agent_dir: &Path, server_dir: &Path, store: &Store) -> DiscoveryReport {
    let dir = sessions_dir(agent_dir);
    let mut report = DiscoveryReport {
        agent_dir: agent_dir.to_path_buf(),
        sessions_dir: dir.clone(),
        ..DiscoveryReport::default()
    };
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .map(|it| {
            it.filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    for name in names {
        if !is_session_id(&name) {
            continue;
        }
        let path = dir.join(&name);
        // proper-lockfile lock directories sit next to the session directories.
        if name.ends_with(".lock") && !path.join(META_FILE).exists() {
            continue;
        }
        report
            .sessions
            .push(inspect(store, &name, &path, server_dir));
    }

    for name in [
        "models.json",
        "settings.json",
        "AGENTS.override.md",
        "AGENTS.md",
        "AGENTS.MD",
        "CLAUDE.md",
        "CLAUDE.MD",
        "skills",
        "prompts",
    ] {
        let path = agent_dir.join(name);
        if let Ok(meta) = std::fs::metadata(&path) {
            report.config.push(ConfigItem {
                name: name.to_owned(),
                is_dir: meta.is_dir(),
                path,
            });
        }
    }

    match read_auth(agent_dir) {
        Ok(Some(entries)) => {
            for (provider, value) in entries {
                let kind = value
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_owned();
                let reserved = None;
                report.credentials.push(CredentialInfo {
                    provider,
                    kind,
                    reserved,
                });
            }
        }
        Ok(None) => {}
        Err(why) => report.credentials_error = Some(why),
    }
    report
}

fn inspect(store: &Store, id: &str, path: &Path, server_dir: &Path) -> DiscoveredSession {
    let sqlite = path.join(STORAGE_FILE);
    let meta = read_meta(path);
    let mut found = DiscoveredSession {
        id: id.to_owned(),
        path: path.to_path_buf(),
        cwd: meta.as_ref().ok().map(|m| m.cwd.clone()),
        created_at_ms: meta.as_ref().ok().map(|m| m.created_at),
        sqlite_bytes: file_len(&sqlite),
        wal_bytes: file_len(&legacy::wal_path(&sqlite)),
        meta_bytes: file_len(&path.join(META_FILE)),
        source_format: SOURCE_FORMAT,
        schema_version: None,
        status: SessionStatus::Importable,
        detail: None,
    };
    match store.imported(ENGINE, id) {
        Ok(Some(native)) => {
            found.status = SessionStatus::AlreadyImported { native };
            return found;
        }
        Ok(None) => {}
        Err(e) => {
            found.status = SessionStatus::Blocked {
                reasons: vec![format!("the native store could not be read ({e})")],
            };
            return found;
        }
    }
    let meta = match meta {
        Ok(meta) => meta,
        Err(why) => {
            found.status = SessionStatus::Blocked { reasons: vec![why] };
            return found;
        }
    };
    if let Some(why) = fresh_lock(path) {
        found.status = SessionStatus::Locked;
        found.detail = Some(why);
        return found;
    }
    if !sqlite.is_file() {
        found.status = SessionStatus::Blocked {
            reasons: vec!["no session.sqlite: the session was never opened".into()],
        };
        return found;
    }
    if let Err(e) = plugins::Snapshot::read(server_dir) {
        found.status = SessionStatus::Blocked {
            reasons: vec![e.to_string()],
        };
        return found;
    }
    let before = stamp(&sqlite);
    let read = legacy::open_source(&sqlite)
        .map_err(legacy::classify)
        .and_then(|conn| {
            let version = legacy::schema_version(&conn).ok();
            legacy::read(&conn).map(|l| (version, l))
        });
    if stamp(&sqlite) != before {
        found.status = SessionStatus::Locked;
        found.detail = Some("the session database changed while it was read".into());
        return found;
    }
    match read {
        Ok((version, legacy)) => {
            found.schema_version = version;
            found.status = match plan::plan(id, &meta, &legacy) {
                Ok(_) => SessionStatus::Importable,
                Err(reasons) => SessionStatus::Blocked { reasons },
            };
        }
        Err(legacy::ReadError::Busy(why)) => {
            found.status = SessionStatus::Locked;
            found.detail = Some(why);
        }
        Err(e) => {
            if let Ok(conn) = legacy::open_source(&sqlite) {
                found.schema_version = legacy::schema_version(&conn).ok();
            }
            found.status = SessionStatus::Blocked {
                reasons: vec![e.to_string()],
            };
        }
    }
    found
}

// ---------------------------------------------------------------------------- import

/// Import one legacy session (see the module docs). Idempotent: a session imported before
/// returns its recorded mapping and nothing is written, not even a backup.
pub fn import_session(
    store: &Store,
    agent_dir: &Path,
    session_id: &str,
    server_dir: &Path,
    backup_dir: &Path,
) -> Result<ImportOutcome, ImportError> {
    if !is_session_id(session_id) {
        return Err(ImportError::InvalidId(session_id.to_owned()));
    }
    if let Some((native, manifest)) = read_import(store, session_id)? {
        return Ok(ImportOutcome {
            children: children_of(&manifest, &native),
            native_session: native,
            already_imported: true,
            manifest,
        });
    }
    let dir = sessions_dir(agent_dir).join(session_id);
    if !dir.is_dir() {
        return Err(ImportError::NotFound(session_id.to_owned()));
    }
    let meta = read_meta(&dir).map_err(ImportError::Source)?;
    if let Some(why) = fresh_lock(&dir) {
        return Err(ImportError::Locked(why));
    }
    let sqlite = dir.join(STORAGE_FILE);
    if !sqlite.is_file() {
        return Err(ImportError::Blocked(vec![
            "no session.sqlite: the session was never opened".into(),
        ]));
    }
    if store.session(session_id)?.is_some() {
        return Err(ImportError::Conflict(format!(
            "a native session {session_id} already exists and was not imported from this source"
        )));
    }

    let plugins = plugins::Snapshot::read(server_dir)?;

    // Consistent copy, read only from the copy.
    let target_dir = backup_dir.join(session_id);
    std::fs::create_dir_all(&target_dir).map_err(|e| io("create", &target_dir, e))?;
    let backup = target_dir.join(STORAGE_FILE);
    let partial = target_dir.join(format!("{STORAGE_FILE}.partial"));
    let _ = std::fs::remove_file(&partial);
    let before = stamp(&sqlite);
    let source_sha256 = sha256_file(&sqlite)?;
    copy_database(&sqlite, &partial)?;
    if stamp(&sqlite) != before {
        let _ = std::fs::remove_file(&partial);
        return Err(ImportError::Locked(
            "the session database changed while it was copied".into(),
        ));
    }
    std::fs::rename(&partial, &backup).map_err(|e| io("rename", &partial, e))?;
    let meta_backup = target_dir.join(META_FILE);
    std::fs::copy(dir.join(META_FILE), &meta_backup).map_err(|e| io("copy", &meta_backup, e))?;
    let backup_sha256 = sha256_file(&backup)?;
    let meta_sha256 = sha256_file(&meta_backup)?;

    let copy = Connection::open_with_flags(
        &backup,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| ImportError::Source(format!("the backup could not be opened ({e})")))?;
    let integrity: String = copy
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .map_err(|e| ImportError::Source(format!("integrity check failed ({e})")))?;
    if integrity != "ok" {
        return Err(ImportError::Source(format!(
            "the session database failed its integrity check: {integrity}"
        )));
    }
    let legacy = legacy::read(&copy).map_err(|e| match e {
        legacy::ReadError::Unsupported(why) => ImportError::Blocked(vec![why]),
        other => ImportError::Source(other.to_string()),
    })?;
    drop(copy);
    let plan = plan::plan(session_id, &meta, &legacy).map_err(ImportError::Blocked)?;

    let now = now_ms();
    let mut manifest = plan.summary.clone();
    if let Value::Object(m) = &mut manifest {
        m.insert("engine".into(), json!(ENGINE));
        m.insert("source".into(), json!(session_id));
        m.insert("format".into(), json!(SOURCE_FORMAT));
        m.insert("schemaVersion".into(), json!(legacy.schema_version));
        m.insert("sourceDir".into(), json!(dir.to_string_lossy()));
        m.insert("backupDir".into(), json!(target_dir.to_string_lossy()));
        m.insert(
            "hashes".into(),
            json!({
                "sourceSqliteSha256": source_sha256,
                "backupSqliteSha256": backup_sha256,
                "metaSha256": meta_sha256,
            }),
        );
        m.insert("importedAt".into(), json!(now));
        m.insert("pluginProfiles".into(), plugins.manifest());
    }
    plugins.unchanged()?;
    if let Some(why) = fresh_lock(&dir) {
        return Err(ImportError::Locked(why));
    }
    write_plan(store, session_id, &plan, &manifest, now)?;
    Ok(ImportOutcome {
        native_session: session_id.to_owned(),
        children: plan.sessions.iter().skip(1).map(|s| s.id.clone()).collect(),
        already_imported: false,
        manifest,
    })
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// SQLite online backup of the source into `dest` (a new file), then made self-contained.
fn copy_database(source: &Path, dest: &Path) -> Result<(), ImportError> {
    let src = legacy::open_source(source).map_err(|e| match legacy::classify(e) {
        legacy::ReadError::Busy(why) => ImportError::Locked(why),
        other => ImportError::Source(other.to_string()),
    })?;
    let mut dst = Connection::open(dest)
        .map_err(|e| ImportError::Io(format!("create {}: {e}", dest.display())))?;
    {
        let backup = rusqlite::backup::Backup::new(&src, &mut dst)
            .map_err(|e| ImportError::Source(format!("backup could not start ({e})")))?;
        loop {
            match backup.step(-1) {
                Ok(rusqlite::backup::StepResult::Done) => break,
                Ok(rusqlite::backup::StepResult::More) => continue,
                Ok(_) => {
                    return Err(ImportError::Locked(
                        "the session database is in use (backup busy)".into(),
                    ));
                }
                Err(e) => {
                    return Err(match legacy::classify(e) {
                        legacy::ReadError::Busy(why) => ImportError::Locked(why),
                        other => ImportError::Source(other.to_string()),
                    });
                }
            }
        }
    }
    dst.query_row("PRAGMA journal_mode = DELETE", [], |r| {
        r.get::<_, String>(0)
    })
    .map_err(|e| ImportError::Io(format!("finalize backup: {e}")))?;
    Ok(())
}

fn mode_str(mode: pipkin_engine_protocol::InputMode) -> &'static str {
    match mode {
        pipkin_engine_protocol::InputMode::Prompt => "prompt",
        pipkin_engine_protocol::InputMode::Steer => "steer",
        pipkin_engine_protocol::InputMode::FollowUp => "followUp",
    }
}

fn to_json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).expect("import records serialize")
}

/// Every native record of the plan plus its `import_map` row, in one transaction.
fn write_plan(
    store: &Store,
    source: &str,
    plan: &plan::Plan,
    manifest: &Value,
    now: i64,
) -> Result<(), ImportError> {
    store.tx(|tx| {
        for s in &plan.sessions {
            tx.execute(
                "INSERT INTO sessions(id, cwd, title, created_at, updated_at, origin, model,
                    thinking, subagents_enabled, parent, root, create_key, revision, blocked)
                 VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, 0, NULL)",
                params![
                    s.id,
                    s.cwd,
                    s.created_at,
                    s.updated_at,
                    to_json(&s.origin),
                    s.model,
                    s.thinking,
                    s.subagents_enabled as i64,
                    s.parent,
                    s.root
                ],
            )?;
        }
        for s in &plan.sessions {
            for e in &s.entries {
                Store::insert_entry(tx, &s.id, e.id, e.at_ms, None, &e.kind, false)?;
            }
            Store::retire_entries(tx, &s.id, &s.retired)?;
            for (key, value) in &s.kv {
                Store::kv_set(tx, &s.id, key, value)?;
            }
            // Preserve Pi selections only as inactive provenance. Pin an empty native
            // selection so an unrelated server default cannot activate extension code.
            if s.parent.is_none() {
                crate::extensions::profiles::write_session(tx, &s.id, &[])?;
            }
            for (bucket, key, value) in &s.usage {
                tx.execute(
                    "INSERT INTO usage(session, bucket, key, value) VALUES (?1, ?2, ?3, ?4)",
                    params![s.id, bucket, key, value],
                )?;
            }
        }
        for r in &plan.requests {
            tx.execute(
                "INSERT INTO requests(session, key, fingerprint, mode, state, queue_entry, run,
                    outcome, text, images, model, created_at, settled_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, ?6, ?7, ?8, NULL, ?9, ?10)",
                params![
                    r.session,
                    r.key,
                    r.fingerprint,
                    mode_str(r.mode),
                    if r.withdrawn { "withdrawn" } else { "settled" },
                    to_json(&r.outcome),
                    r.text,
                    to_json(&r.images),
                    r.created_at,
                    r.settled_at
                ],
            )?;
        }
        for t in &plan.tasks {
            tx.execute(
                "INSERT INTO subagent_tasks(root, child, parent_session, parent_call,
                    child_session, task, status, result, result_delivered, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    t.root,
                    t.child as i64,
                    t.parent_session,
                    t.parent_call,
                    t.child_session,
                    t.task,
                    t.status,
                    t.result,
                    t.result_delivered as i64,
                    t.created_at,
                    t.updated_at
                ],
            )?;
        }
        Store::record_import(tx, ENGINE, source, source, manifest, now)?;
        Ok(())
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------- credentials, config

/// `auth.json` entries, or `None` when there is no file. Error text never includes contents.
fn read_auth(agent_dir: &Path) -> Result<Option<Map<String, Value>>, String> {
    let path = agent_dir.join("auth.json");
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("auth.json could not be read ({})", e.kind())),
    };
    match serde_json::from_str::<Value>(&raw) {
        Ok(Value::Object(map)) => Ok(Some(map)),
        Ok(_) => Err("auth.json is not a JSON object".into()),
        Err(e) => Err(format!(
            "auth.json is not valid JSON (line {}, column {})",
            e.line(),
            e.column()
        )),
    }
}

/// The raw `auth.json` entries, for an explicit, user-approved credential import. The caller
/// merges them into the native credential store and must never log the values. Claude
/// subscription entries retain their OAuth kind and extras. Missing file: an empty map.
pub fn credential_entries(agent_dir: &Path) -> Result<Map<String, Value>, String> {
    Ok(read_auth(agent_dir)?.unwrap_or_default())
}

/// Files to copy for an explicit configuration import: `(source, destination relative to the
/// native config dir)`. Global instruction files keep their names; `skills/` and `prompts/`
/// are listed file by file (symlinks are followed; depth is bounded).
pub fn config_files(agent_dir: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut out = Vec::new();
    for name in [
        "models.json",
        "settings.json",
        "AGENTS.override.md",
        "AGENTS.md",
        "AGENTS.MD",
        "CLAUDE.md",
        "CLAUDE.MD",
    ] {
        let path = agent_dir.join(name);
        if std::fs::metadata(&path).is_ok_and(|m| m.is_file()) {
            out.push((path, PathBuf::from(name)));
        }
    }
    for dir in ["skills", "prompts"] {
        walk(&agent_dir.join(dir), Path::new(dir), 0, &mut out);
    }
    out
}

fn walk(dir: &Path, relative: &Path, depth: usize, out: &mut Vec<(PathBuf, PathBuf)>) {
    const MAX_DEPTH: usize = 16;
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut items: Vec<_> = read.filter_map(|e| e.ok()).collect();
    items.sort_by_key(|e| e.file_name());
    for item in items {
        let path = item.path();
        let rel = relative.join(item.file_name());
        match std::fs::metadata(&path) {
            Ok(m) if m.is_dir() => walk(&path, &rel, depth + 1, out),
            Ok(m) if m.is_file() => out.push((path, rel)),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
