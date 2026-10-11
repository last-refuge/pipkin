//! Versioned local persistence: preferences, drafts and demo-owned conversation data.
//!
//! One SQLite database (WAL, `synchronous=FULL`) with ONE background writer thread fed by a
//! bounded channel. A draft acknowledgment is delivered only after its transaction has
//! committed. Demo conversations live in `demo_`-prefixed tables so a real backend can replace
//! them without any migration into engine storage. Drafts and session-scoped preferences
//! (selection, model) are keyed by a backend/profile namespace so numeric conversation IDs from
//! one backend can never be applied to another. Before migrating an existing database, a copy is
//! kept beside it as `<db>.bak-v<old version>`.
//!
//! The request journal records each submission's immutable payload *before* anything is sent
//! and tracks it to a terminal state, so a crash or lost acknowledgment can be reconciled
//! instead of replayed.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

use pipkin_core::*;
use rusqlite::{Connection, params};
use serde_json::{Value, json};

/// Writes queued beyond this are rejected as "busy" rather than blocking the UI thread.
const QUEUE_DEPTH: usize = 256;

pub const INJECTED_FAILURE: &str = "injected write failure";

/// Namespace for the simulated backend, and for data stored before namespaces existed.
pub const DEMO_NAMESPACE: &str = "demo";

/// Preference keys that refer to backend-owned identities rather than to the application.
const SESSION_PREF_KEYS: [&str; 3] = ["selected_project", "selected_conversation", "model"];

const MIGRATIONS: &[&str] = &[
    "CREATE TABLE prefs (key TEXT PRIMARY KEY, value TEXT NOT NULL);
     CREATE TABLE drafts (
        conversation_id INTEGER PRIMARY KEY,
        text TEXT NOT NULL,
        rev INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
     );",
    "CREATE TABLE demo_conversations (
        id INTEGER PRIMARY KEY,
        project_id INTEGER NOT NULL,
        title TEXT NOT NULL,
        updated_at INTEGER NOT NULL
     );",
    "CREATE TABLE drafts_ns (
        namespace TEXT NOT NULL,
        conversation_id INTEGER NOT NULL,
        text TEXT NOT NULL,
        rev INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        PRIMARY KEY (namespace, conversation_id)
     );
     INSERT INTO drafts_ns SELECT 'demo', conversation_id, text, rev, updated_at FROM drafts;
     DROP TABLE drafts;
     ALTER TABLE drafts_ns RENAME TO drafts;
     CREATE TABLE session_prefs (
        namespace TEXT NOT NULL,
        key TEXT NOT NULL,
        value TEXT NOT NULL,
        PRIMARY KEY (namespace, key)
     );
     INSERT INTO session_prefs SELECT 'demo', key, value FROM prefs
        WHERE key IN ('selected_project', 'selected_conversation', 'model');
     DELETE FROM prefs WHERE key IN ('selected_project', 'selected_conversation', 'model');",
    "CREATE TABLE request_journal (
        namespace TEXT NOT NULL,
        request_id TEXT NOT NULL,
        conversation_id INTEGER NOT NULL,
        payload TEXT NOT NULL,
        state TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        PRIMARY KEY (namespace, request_id)
     );",
    "CREATE TABLE projects (
        namespace TEXT NOT NULL,
        path TEXT NOT NULL,
        added_at INTEGER NOT NULL,
        PRIMARY KEY (namespace, path)
     );",
    "CREATE TABLE draft_attachments (
        namespace TEXT NOT NULL,
        conversation_id INTEGER NOT NULL,
        position INTEGER NOT NULL,
        path TEXT NOT NULL,
        name TEXT NOT NULL,
        size INTEGER,
        PRIMARY KEY (namespace, conversation_id, position)
     );",
    "CREATE TABLE transcript_cache (
        namespace TEXT NOT NULL,
        conversation_id INTEGER NOT NULL,
        synced_at INTEGER NOT NULL,
        has_older INTEGER NOT NULL,
        message_count INTEGER NOT NULL,
        items TEXT NOT NULL,
        title TEXT NOT NULL,
        project_path TEXT NOT NULL,
        updated_at INTEGER NOT NULL,
        PRIMARY KEY (namespace, conversation_id)
     );
     CREATE VIRTUAL TABLE transcript_search USING fts5(
        text,
        namespace UNINDEXED,
        conversation_id UNINDEXED,
        item_id UNINDEXED,
        at UNINDEXED,
        tokenize = 'unicode61 remove_diacritics 2'
     );",
    // Which engine may write a conversation. Keyed by the desktop conversation id alone: an
    // imported session keeps its Pi session id, so the id is the same in both namespaces.
    // Recorded in the import's transaction; never written to either engine's data.
    "CREATE TABLE conversation_owners (
        conversation_id INTEGER PRIMARY KEY,
        owner TEXT NOT NULL,
        engine TEXT NOT NULL,
        source_namespace TEXT NOT NULL,
        imported_at INTEGER NOT NULL
     );",
];

/// `conversation_owners.owner` for the built-in engine.
pub const OWNER_NATIVE: &str = "native";

/// Conversations whose saved copy is kept, per backend profile; the least recently synced go first.
const MAX_CACHED_CONVERSATIONS: i64 = 50;
/// Most hits one search returns.
pub const SEARCH_LIMIT: usize = 40;

/// States that still need reconciliation after a restart.
const OPEN_STATES: &str = "'intent', 'unknown', 'accepted'";

const PROMPT_HISTORY_LIMIT: usize = 200;
/// Skip oversized prompts rather than restore partial text into the composer.
const MAX_HISTORY_PROMPT_BYTES: usize = 64 * 1024;

pub const SCHEMA_VERSION: u32 = MIGRATIONS.len() as u32;

/// A read-only summary of a database for diagnostics: its schema version, size and integrity.
/// Opens read-only, so it never migrates, repairs or creates anything.
pub fn inspect(path: &Path) -> Result<String, String> {
    if !path.exists() {
        return Ok("none yet (created on first run)".into());
    }
    let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(sql_err)?;
    let version: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(sql_err)?;
    let check: String = conn
        .query_row("PRAGMA quick_check", [], |r| r.get(0))
        .map_err(sql_err)?;
    let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if version > SCHEMA_VERSION {
        return Err(format!(
            "schema {version} is newer than this build supports ({SCHEMA_VERSION}); \
             upgrade Pipkin, or restore a backup made before the newer version"
        ));
    }
    if check != "ok" {
        return Err(format!("integrity check says: {check}"));
    }
    Ok(format!("schema {version}, {bytes} bytes, integrity ok"))
}

pub type Ack = Box<dyn FnOnce(Result<(), String>) + Send>;

#[derive(Clone, Debug, PartialEq)]
pub struct DemoConversation {
    pub id: ConversationId,
    pub project: ProjectId,
    pub title: String,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StoredDraft {
    pub conversation: ConversationId,
    pub text: String,
    /// What was attached when the draft was saved. Files are re-checked when it is restored.
    pub attachments: Vec<Attachment>,
    pub rev: u64,
}

/// A journaled submission that never reached a terminal state.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenRequest {
    pub conversation: ConversationId,
    pub request: RequestId,
    pub text: String,
    pub attachments: Vec<Attachment>,
}

#[derive(Debug)]
pub struct Loaded {
    pub prefs: Prefs,
    pub drafts: Vec<StoredDraft>,
    pub conversations: Vec<DemoConversation>,
    /// Oldest first, so a later request for a conversation supersedes an earlier one.
    pub open_requests: Vec<OpenRequest>,
    /// Submitted text per conversation, oldest first, including terminal requests.
    pub prompt_histories: Vec<(ConversationId, Vec<String>)>,
    /// A restart never automatically resumes a goal: the last turn may be unresolved.
    pub goals: Vec<(ConversationId, String, Option<String>)>,
    /// Project folders the user opened, oldest first.
    pub projects: Vec<String>,
}

enum Msg {
    Draft {
        conversation: ConversationId,
        text: String,
        attachments: Vec<Attachment>,
        rev: u64,
        ack: Ack,
    },
    Prefs(Prefs),
    SetupCompletion {
        prefs: Prefs,
        ack: Ack,
    },
    Goal {
        conversation: ConversationId,
        text: Option<String>,
        paused: Option<String>,
    },
    Conversation(DemoConversation),
    Project(String),
    Intent {
        conversation: ConversationId,
        request: RequestId,
        payload: String,
        ack: Ack,
    },
    JournalState {
        request: RequestId,
        state: JournalState,
    },
    Cache(Box<CacheWrite>),
}

/// A conversation's saved copy, ready to write.
pub struct CacheWrite {
    pub conversation: ConversationId,
    pub meta: CacheMeta,
    pub items_json: String,
    pub kept: usize,
    pub has_older: bool,
    pub synced_at: i64,
    /// `(item id, time, text)` of each searchable message in the copy.
    pub searchable: Vec<(u64, i64, String)>,
}

/// What identifies a saved conversation when the engine is not there to say.
#[derive(Clone, Debug, PartialEq)]
pub struct CacheMeta {
    pub title: String,
    pub project_path: String,
    pub updated_at: i64,
}

/// What [`Storage::copy_conversations_from`] copied: rows per kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CopyReport {
    pub drafts: usize,
    pub goals: usize,
    pub journal: usize,
    pub caches: usize,
}

/// The desktop's record that the built-in engine owns a conversation imported from Pi.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeOwnership {
    pub conversation: ConversationId,
    /// The built-in engine's desktop namespace (`native:<hash of its data directory>`).
    pub engine: String,
    /// The Pi namespace the desktop data was copied from.
    pub source_namespace: String,
    /// Unix seconds.
    pub imported_at: i64,
}

/// A conversation known from its saved copy.
#[derive(Clone, Debug, PartialEq)]
pub struct CachedConversation {
    pub id: ConversationId,
    pub meta: CacheMeta,
    pub synced_at: i64,
}

/// A saved copy read back.
#[derive(Debug, PartialEq)]
pub struct CachedTranscript {
    pub items: Vec<TranscriptItem>,
    pub has_older: bool,
    pub synced_at: i64,
}

/// Receives a message when a write nobody is waiting on (preferences, journal updates, ...)
/// fails, so the failure reaches the user instead of only the log.
pub type ErrorSink = Arc<dyn Fn(String) + Send + Sync>;

/// Reports each kind of background failure once until that kind succeeds again.
struct Reporter {
    sink: Mutex<Option<ErrorSink>>,
    active: Mutex<std::collections::HashSet<&'static str>>,
}

impl Reporter {
    fn failed(&self, kind: &'static str, message: String) {
        log::warn!("{message}");
        if self.active.lock().unwrap().insert(kind)
            && let Some(sink) = self.sink.lock().unwrap().as_ref()
        {
            sink(message);
        }
    }

    fn succeeded(&self, kind: &'static str) {
        self.active.lock().unwrap().remove(kind);
    }

    fn settle(&self, kind: &'static str, result: Result<(), String>, what: &str) {
        match result {
            Ok(()) => self.succeeded(kind),
            Err(e) => self.failed(kind, format!("Could not save {what}: {e}")),
        }
    }
}

pub struct Storage {
    path: PathBuf,
    namespace: String,
    sender: Mutex<Option<SyncSender<Msg>>>,
    writer: Mutex<Option<JoinHandle<()>>>,
    fail_writes: Arc<AtomicBool>,
    #[cfg_attr(not(test), allow(dead_code))]
    fail_background: Arc<AtomicBool>,
    reporter: Arc<Reporter>,
}

fn sql_err(e: rusqlite::Error) -> String {
    e.to_string()
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn configure(conn: &Connection) -> Result<(), String> {
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(sql_err)?;
    // `journal_mode` returns a row, so it cannot go through `execute_batch`-style pragmas.
    conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))
        .map_err(sql_err)?;
    conn.execute_batch("PRAGMA synchronous=FULL;")
        .map_err(sql_err)
}

fn refuse_newer(conn: &Connection) -> Result<(), String> {
    let version: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(sql_err)?;
    if version > SCHEMA_VERSION {
        return Err(format!(
            "database schema version {version} is newer than this build supports ({SCHEMA_VERSION})"
        ));
    }
    Ok(())
}

fn migrate(conn: &mut Connection, path: &Path) -> Result<(), String> {
    refuse_newer(conn)?;
    let version: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(sql_err)?;
    if version > 0 && version < SCHEMA_VERSION {
        backup(conn, path, version)?;
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
        let tx = conn.transaction().map_err(sql_err)?;
        tx.execute_batch(sql).map_err(sql_err)?;
        tx.execute_batch(&format!("PRAGMA user_version = {}", i + 1))
            .map_err(sql_err)?;
        tx.commit().map_err(sql_err)?;
    }
    Ok(())
}

/// Copy the database beside itself before a migration. The WAL is checkpointed first so the
/// copy is complete without its `-wal` file.
fn backup(conn: &Connection, path: &Path, version: u32) -> Result<(), String> {
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .map_err(sql_err)?;
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".bak-v{version}"));
    std::fs::copy(path, PathBuf::from(&name))
        .map(|_| ())
        .map_err(|e| format!("cannot back up database before migration: {e}"))
}

impl Storage {
    /// Open (creating and migrating if needed) the database at `path` and start the writer.
    pub fn open(path: &Path, namespace: &str) -> Result<Storage, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        let mut conn = Connection::open(path).map_err(sql_err)?;
        // A newer build's database is looked at before anything is changed, so refusing it
        // leaves the file exactly as it was (even the journal mode stays as that build set it).
        refuse_newer(&conn)?;
        configure(&conn)?;
        migrate(&mut conn, path)?;

        let (tx, rx) = sync_channel::<Msg>(QUEUE_DEPTH);
        let fail_writes = Arc::new(AtomicBool::new(false));
        let fail_background = Arc::new(AtomicBool::new(false));
        let reporter = Arc::new(Reporter {
            sink: Mutex::new(None),
            active: Mutex::new(Default::default()),
        });
        let (fail, fail_bg, report) = (
            fail_writes.clone(),
            fail_background.clone(),
            reporter.clone(),
        );
        let ns = namespace.to_string();
        let writer = std::thread::Builder::new()
            .name("pi-storage-writer".into())
            .spawn(move || {
                // Ends when every sender is dropped, after the queue has been drained.
                while let Ok(msg) = rx.recv() {
                    write(&mut conn, &ns, &fail, &fail_bg, &report, msg);
                }
            })
            .map_err(|e| format!("cannot start storage writer: {e}"))?;
        Ok(Storage {
            path: path.to_path_buf(),
            namespace: namespace.to_string(),
            sender: Mutex::new(Some(tx)),
            writer: Mutex::new(Some(writer)),
            fail_writes,
            fail_background,
            reporter,
        })
    }

    /// Route failures of background writes to `sink` (once per kind until it succeeds again).
    pub fn set_error_sink(&self, sink: ErrorSink) {
        *self.reporter.sink.lock().unwrap() = Some(sink);
    }

    /// Make preference, journal-update and project writes fail (a test control).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_background_failure(&self, fail: bool) {
        self.fail_background.store(fail, Ordering::SeqCst);
    }

    /// Open the database, setting a damaged file aside instead of failing on it: the newest
    /// intact migration backup is restored if there is one, otherwise a fresh database is
    /// started. A database from a newer build is never touched (that is an error), and neither
    /// is anything that merely cannot be written.
    pub fn open_recovering(path: &Path, namespace: &str) -> Result<(Storage, Startup), String> {
        match Storage::open(path, namespace) {
            Ok(storage) => Ok((storage, Startup::Clean)),
            Err(error) if is_damage(&error) => recover(path, namespace, &error),
            Err(error) => Err(error),
        }
    }

    pub fn load_all(&self) -> Result<Loaded, String> {
        let conn = Connection::open(&self.path).map_err(sql_err)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(sql_err)?;
        let mut prefs = Prefs::default();
        let mut stmt = conn
            .prepare("SELECT key, value FROM prefs")
            .map_err(sql_err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(sql_err)?;
        for row in rows {
            let (key, value) = row.map_err(sql_err)?;
            if let Ok(v) = serde_json::from_str::<Value>(&value) {
                apply_pref(&mut prefs, &key, &v);
            }
        }
        let mut stmt = conn
            .prepare("SELECT key, value FROM session_prefs WHERE namespace = ?1")
            .map_err(sql_err)?;
        let rows = stmt
            .query_map([&self.namespace], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(sql_err)?;
        for row in rows {
            let (key, value) = row.map_err(sql_err)?;
            if let Ok(v) = serde_json::from_str::<Value>(&value) {
                apply_pref(&mut prefs, &key, &v);
            }
        }
        let mut drafts = conn
            .prepare(
                "SELECT conversation_id, text, rev FROM drafts WHERE namespace = ?1
                 ORDER BY conversation_id",
            )
            .map_err(sql_err)?
            .query_map([&self.namespace], |r| {
                Ok(StoredDraft {
                    conversation: ConversationId(r.get::<_, i64>(0)? as u64),
                    text: r.get(1)?,
                    attachments: vec![],
                    rev: r.get::<_, i64>(2)? as u64,
                })
            })
            .map_err(sql_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql_err)?;
        let attached = conn
            .prepare(
                "SELECT conversation_id, path, name, size FROM draft_attachments
                 WHERE namespace = ?1 ORDER BY conversation_id, position",
            )
            .map_err(sql_err)?
            .query_map([&self.namespace], |r| {
                Ok((
                    r.get::<_, i64>(0)? as u64,
                    Attachment {
                        path: r.get(1)?,
                        name: r.get(2)?,
                        size: r.get::<_, Option<i64>>(3)?.map(|s| s as u64),
                        error: None,
                    },
                ))
            })
            .map_err(sql_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql_err)?;
        for (conversation, attachment) in attached {
            if let Some(d) = drafts.iter_mut().find(|d| d.conversation.0 == conversation) {
                d.attachments.push(attachment);
            }
        }
        let conversations = if self.namespace != DEMO_NAMESPACE {
            vec![]
        } else {
            conn.prepare(
                "SELECT id, project_id, title, updated_at FROM demo_conversations ORDER BY id",
            )
            .map_err(sql_err)?
            .query_map([], |r| {
                Ok(DemoConversation {
                    id: ConversationId(r.get::<_, i64>(0)? as u64),
                    project: ProjectId(r.get::<_, i64>(1)? as u64),
                    title: r.get(2)?,
                    updated_at: r.get(3)?,
                })
            })
            .map_err(sql_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql_err)?
        };
        let open_requests = conn
            .prepare(&format!(
                "SELECT conversation_id, request_id, payload FROM request_journal
                 WHERE namespace = ?1 AND state IN ({OPEN_STATES}) ORDER BY rowid"
            ))
            .map_err(sql_err)?
            .query_map([&self.namespace], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(sql_err)?
            .filter_map(|row| {
                let (conversation, request, payload) = row.ok()?;
                let (text, attachments) = decode_payload(&payload)?;
                Some(OpenRequest {
                    conversation: ConversationId(conversation as u64),
                    request: RequestId(request),
                    text,
                    attachments,
                })
            })
            .collect();
        // rowid is submission order (timestamps have only second precision). Rank after
        // filtering so autonomous/invalid/oversized entries cannot crowd out user prompts.
        // Older payloads lack origin; retain them as user prompts for compatibility.
        let mut stmt = conn
            .prepare(
                "WITH payloads AS (
                    SELECT rowid AS sequence, conversation_id,
                        CASE WHEN json_valid(payload) THEN payload ELSE '{}' END AS body
                    FROM request_journal WHERE namespace = ?1
                 ), prompts AS (
                    SELECT sequence, conversation_id, json_extract(body, '$.text') AS text,
                        ROW_NUMBER() OVER (
                            PARTITION BY conversation_id ORDER BY sequence DESC
                        ) AS position
                    FROM payloads
                    WHERE json_type(body, '$.text') = 'text'
                        AND COALESCE(json_extract(body, '$.origin'), '') != 'Goal'
                        AND length(CAST(json_extract(body, '$.text') AS BLOB)) <= ?2
                 )
                 SELECT conversation_id, text FROM prompts WHERE position <= ?3
                 ORDER BY conversation_id, sequence",
            )
            .map_err(sql_err)?;
        let rows = stmt
            .query_map(
                params![
                    self.namespace,
                    MAX_HISTORY_PROMPT_BYTES as i64,
                    PROMPT_HISTORY_LIMIT as i64
                ],
                |r| {
                    Ok((
                        ConversationId(r.get::<_, i64>(0)? as u64),
                        r.get::<_, String>(1)?,
                    ))
                },
            )
            .map_err(sql_err)?;
        let mut prompt_histories: Vec<(ConversationId, Vec<String>)> = vec![];
        for row in rows {
            let (conversation, text) = row.map_err(sql_err)?;
            if let Some((id, prompts)) = prompt_histories.last_mut()
                && *id == conversation
            {
                prompts.push(text);
            } else {
                prompt_histories.push((conversation, vec![text]));
            }
        }
        let goals = conn
            .prepare(
                "SELECT key, value FROM session_prefs WHERE namespace = ?1 AND key LIKE 'goal:%'",
            )
            .map_err(sql_err)?
            .query_map([&self.namespace], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(sql_err)?
            .filter_map(|row| {
                let (key, value) = row.ok()?;
                let id = key.strip_prefix("goal:")?.parse::<u64>().ok()?;
                let value: Value = serde_json::from_str(&value).ok()?;
                Some((
                    ConversationId(id),
                    value.get("text")?.as_str()?.to_string(),
                    value
                        .get("paused")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                ))
            })
            .collect();
        let projects = conn
            .prepare("SELECT path FROM projects WHERE namespace = ?1 ORDER BY rowid")
            .map_err(sql_err)?
            .query_map([&self.namespace], |r| r.get::<_, String>(0))
            .map_err(sql_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql_err)?;
        Ok(Loaded {
            prefs,
            drafts,
            conversations,
            open_requests,
            prompt_histories,
            goals,
            projects,
        })
    }

    fn send(&self, msg: Msg) -> Result<(), Msg> {
        let guard = self.sender.lock().unwrap();
        match guard.as_ref() {
            None => Err(msg),
            Some(tx) => tx.try_send(msg).map_err(|e| match e {
                TrySendError::Full(m) | TrySendError::Disconnected(m) => m,
            }),
        }
    }

    /// Queue a draft write. `ack` runs on the writer thread only after the transaction has
    /// committed (or with the error). The caller keeps its own copy of the text either way.
    pub fn save_draft(
        &self,
        conversation: ConversationId,
        text: String,
        attachments: Vec<Attachment>,
        rev: u64,
        ack: impl FnOnce(Result<(), String>) + Send + 'static,
    ) {
        let msg = Msg::Draft {
            conversation,
            text,
            attachments,
            rev,
            ack: Box::new(ack),
        };
        if let Err(Msg::Draft { ack, .. }) = self.send(msg) {
            ack(Err("storage is busy or closed".into()));
        }
    }

    /// Queue a journal entry for a submission. `ack` runs on the writer thread only after the
    /// transaction has committed (or with the error); nothing may be sent before it reports Ok.
    /// Legacy callers are treated as Draft-origin user submissions.
    #[cfg(test)]
    pub fn journal_intent(
        &self,
        conversation: ConversationId,
        request: RequestId,
        text: &str,
        attachments: &[Attachment],
        model: Option<&str>,
        ack: impl FnOnce(Result<(), String>) + Send + 'static,
    ) {
        self.journal_intent_with_origin(
            conversation,
            PendingIntent {
                request,
                origin: IntentOrigin::Draft,
                text: text.into(),
                attachments: attachments.to_vec(),
                model: model.map(str::to_string),
            },
            ack,
        );
    }

    /// Journal the origin along with the immutable submission payload, so autonomous Goal
    /// prompts can be excluded from restored user history. Commit/ack semantics are unchanged.
    pub fn journal_intent_with_origin(
        &self,
        conversation: ConversationId,
        intent: PendingIntent,
        ack: impl FnOnce(Result<(), String>) + Send + 'static,
    ) {
        let msg = Msg::Intent {
            conversation,
            request: intent.request,
            payload: encode_payload(
                &intent.text,
                &intent.attachments,
                intent.model.as_deref(),
                Some(intent.origin),
            ),
            ack: Box::new(ack),
        };
        if let Err(Msg::Intent { ack, .. }) = self.send(msg) {
            ack(Err("storage is busy or closed".into()));
        }
    }

    /// Advance a request's journal state. If this write is lost the entry stays open, which on
    /// restart reads as "outcome unknown": the safe direction.
    pub fn journal_state(&self, request: RequestId, state: JournalState) {
        if self.send(Msg::JournalState { request, state }).is_err() {
            log::warn!("journal update dropped: storage busy or closed");
        }
    }

    pub fn save_goal(
        &self,
        conversation: ConversationId,
        text: Option<String>,
        paused: Option<String>,
    ) {
        if self
            .send(Msg::Goal {
                conversation,
                text,
                paused,
            })
            .is_err()
        {
            log::warn!("goal write dropped: storage busy or closed");
        }
    }

    pub fn save_prefs(&self, prefs: &Prefs) {
        if self.send(Msg::Prefs(prefs.clone())).is_err() {
            log::warn!("preferences write dropped: storage busy or closed");
        }
    }

    pub fn complete_setup(
        &self,
        prefs: Prefs,
        ack: impl FnOnce(Result<(), String>) + Send + 'static,
    ) {
        let msg = Msg::SetupCompletion {
            prefs,
            ack: Box::new(ack),
        };
        if let Err(Msg::SetupCompletion { ack, .. }) = self.send(msg) {
            ack(Err("storage is busy or closed".into()));
        }
    }

    /// Remember a project folder. Idempotent.
    pub fn save_project(&self, path: String) {
        if self.send(Msg::Project(path)).is_err() {
            log::warn!("project write dropped: storage busy or closed");
        }
    }

    /// Keep a copy of a conversation's transcript, and make its messages searchable.
    pub fn save_cache(
        &self,
        conversation: ConversationId,
        meta: CacheMeta,
        items: &[TranscriptItem],
        has_older: bool,
        synced_at: i64,
    ) {
        let (items_json, kept) = crate::cache::encode_items(items);
        let searchable = items[items.len() - kept.min(items.len())..]
            .iter()
            .filter_map(|i| Some((i.id.0, i.at, crate::cache::search_text(i)?)))
            .collect();
        let write = CacheWrite {
            conversation,
            meta,
            items_json,
            kept,
            has_older,
            synced_at,
            searchable,
        };
        if self.send(Msg::Cache(Box::new(write))).is_err() {
            log::warn!("transcript copy dropped: storage busy or closed");
        }
    }

    /// The conversations that have a saved copy, most recently synced first.
    pub fn cached_conversations(&self) -> Result<Vec<CachedConversation>, String> {
        let conn = self.read_connection()?;
        let mut stmt = conn
            .prepare(
                "SELECT conversation_id, title, project_path, updated_at, synced_at
                 FROM transcript_cache WHERE namespace = ?1
                 ORDER BY synced_at DESC, rowid DESC",
            )
            .map_err(sql_err)?;
        stmt.query_map([&self.namespace], |r| {
            Ok(CachedConversation {
                id: ConversationId(r.get::<_, i64>(0)? as u64),
                meta: CacheMeta {
                    title: r.get(1)?,
                    project_path: r.get(2)?,
                    updated_at: r.get(3)?,
                },
                synced_at: r.get(4)?,
            })
        })
        .map_err(sql_err)?
        .collect::<Result<_, _>>()
        .map_err(sql_err)
    }

    /// The saved copy of a conversation, if there is one.
    pub fn load_cache(
        &self,
        conversation: ConversationId,
    ) -> Result<Option<CachedTranscript>, String> {
        let conn = self.read_connection()?;
        let row = conn.query_row(
            "SELECT items, has_older, synced_at FROM transcript_cache
             WHERE namespace = ?1 AND conversation_id = ?2",
            params![self.namespace, conversation.0 as i64],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            },
        );
        match row {
            Ok((json, has_older, synced_at)) => Ok(Some(CachedTranscript {
                items: crate::cache::decode_items(&json),
                has_older: has_older != 0,
                synced_at,
            })),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(sql_err(e)),
        }
    }

    /// Search the saved copies' messages for `query`: every word, the last as a prefix.
    pub fn search(&self, query: &str, limit: usize) -> Result<SearchResults, String> {
        let conn = self.read_connection()?;
        let (conversations_searched, messages_searched): (i64, i64) = conn
            .query_row(
                "SELECT count(*), coalesce(sum(message_count), 0) FROM transcript_cache
                 WHERE namespace = ?1",
                [&self.namespace],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(sql_err)?;
        let mut results = SearchResults {
            query: query.to_owned(),
            hits: vec![],
            conversations_searched: conversations_searched as usize,
            messages_searched: messages_searched as usize,
            truncated: false,
        };
        let Some(expression) = fts_expression(query) else {
            return Ok(results);
        };
        let mut stmt = conn
            .prepare(
                "SELECT conversation_id, item_id, at,
                        snippet(transcript_search, 0, char(2), char(3), '\u{2026}', 14)
                 FROM transcript_search
                 WHERE transcript_search MATCH ?1 AND namespace = ?2
                 ORDER BY rank LIMIT ?3",
            )
            .map_err(sql_err)?;
        let rows = stmt
            .query_map(params![expression, self.namespace, limit as i64 + 1], |r| {
                Ok(SearchHit {
                    conversation: ConversationId(r.get::<_, i64>(0)? as u64),
                    item: ItemId(r.get::<_, i64>(1)? as u64),
                    at: r.get(2)?,
                    snippet: r.get(3)?,
                })
            })
            .map_err(sql_err)?;
        for hit in rows {
            results.hits.push(hit.map_err(sql_err)?);
        }
        if results.hits.len() > limit {
            results.hits.truncate(limit);
            results.truncated = true;
        }
        Ok(results)
    }

    fn read_connection(&self) -> Result<Connection, String> {
        let conn = Connection::open(&self.path).map_err(sql_err)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(sql_err)?;
        Ok(conn)
    }

    pub fn save_conversation(&self, conversation: DemoConversation) {
        if self.send(Msg::Conversation(conversation)).is_err() {
            log::warn!("conversation write dropped: storage busy or closed");
        }
    }

    /// Copy (never move) the desktop data of `conversations` from the `source` namespace into
    /// this storage's namespace: drafts with their attachments, goals, the request journal
    /// (which is also the prompt history, and carries any unresolved requests), and the saved
    /// transcript copy with its search index. Anything the target already has for a
    /// conversation is kept as it is. All in one transaction; the source is never changed.
    /// Used when conversations move to another engine under the same desktop ids.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn copy_conversations_from(
        &self,
        source: &str,
        conversations: &[ConversationId],
    ) -> Result<CopyReport, String> {
        self.import_conversations_from(source, &[], conversations, 0)
    }

    /// After an import into the built-in engine (this storage's namespace): copy the desktop
    /// data of `imported` and `existing` (imported earlier) from the Pi namespace `source`, as
    /// [`Storage::copy_conversations_from`] does, and in the SAME transaction record that the
    /// built-in engine now owns each conversation in `imported` (only those: the engine
    /// reported them imported now). Either all of it commits or none of it does. Neither
    /// engine's own data is touched.
    pub fn import_conversations_from(
        &self,
        source: &str,
        imported: &[ConversationId],
        existing: &[ConversationId],
        imported_at: i64,
    ) -> Result<CopyReport, String> {
        let target = self.namespace.as_str();
        if source == target {
            return Err("cannot copy a namespace onto itself".into());
        }
        let mut conn = self.read_connection()?;
        let tx = conn.transaction().map_err(sql_err)?;
        let mut report = CopyReport::default();
        for conversation in imported.iter().chain(existing) {
            copy_conversation(&tx, target, source, *conversation, &mut report)?;
        }
        for conversation in imported {
            tx.execute(
                "INSERT INTO conversation_owners
                    (conversation_id, owner, engine, source_namespace, imported_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(conversation_id) DO UPDATE SET owner = excluded.owner,
                    engine = excluded.engine, source_namespace = excluded.source_namespace,
                    imported_at = excluded.imported_at",
                params![
                    conversation.0 as i64,
                    OWNER_NATIVE,
                    target,
                    source,
                    imported_at
                ],
            )
            .map_err(sql_err)?;
        }
        tx.commit().map_err(sql_err)?;
        Ok(report)
    }

    /// Every conversation the built-in engine owns, oldest import first. Read fresh from the
    /// database each time, so an import made by another window is seen.
    pub fn native_owned(&self) -> Result<Vec<NativeOwnership>, String> {
        let conn = self.read_connection()?;
        let mut stmt = conn
            .prepare(
                "SELECT conversation_id, engine, source_namespace, imported_at
                 FROM conversation_owners WHERE owner = ?1 ORDER BY imported_at, conversation_id",
            )
            .map_err(sql_err)?;
        stmt.query_map([OWNER_NATIVE], ownership_row)
            .map_err(sql_err)?
            .collect::<Result<_, _>>()
            .map_err(sql_err)
    }

    /// Whether the built-in engine owns `conversation`, read fresh from the database.
    pub fn native_owner(
        &self,
        conversation: ConversationId,
    ) -> Result<Option<NativeOwnership>, String> {
        let conn = self.read_connection()?;
        match conn.query_row(
            "SELECT conversation_id, engine, source_namespace, imported_at
             FROM conversation_owners WHERE owner = ?1 AND conversation_id = ?2",
            params![OWNER_NATIVE, conversation.0 as i64],
            ownership_row,
        ) {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(sql_err(e)),
        }
    }

    /// Hand `conversation` back to Pi: delete only the desktop's ownership record (nothing in
    /// either engine, and none of the desktop copies). Returns whether there was a record.
    pub fn release_ownership(&self, conversation: ConversationId) -> Result<bool, String> {
        let conn = self.read_connection()?;
        conn.execute(
            "DELETE FROM conversation_owners WHERE conversation_id = ?1",
            [conversation.0 as i64],
        )
        .map(|n| n > 0)
        .map_err(sql_err)
    }

    /// Make draft saves fail with `"injected write failure"` (developer control).
    pub fn set_write_failure(&self, fail: bool) {
        self.fail_writes.store(fail, Ordering::SeqCst);
    }

    pub fn write_failure(&self) -> bool {
        self.fail_writes.load(Ordering::SeqCst)
    }

    /// Drain the queue and join the writer. Safe to call more than once.
    pub fn shutdown(&self) {
        drop(self.sender.lock().unwrap().take());
        if let Some(handle) = self.writer.lock().unwrap().take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Storage {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn ownership_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<NativeOwnership> {
    Ok(NativeOwnership {
        conversation: ConversationId(r.get::<_, i64>(0)? as u64),
        engine: r.get(1)?,
        source_namespace: r.get(2)?,
        imported_at: r.get(3)?,
    })
}

/// Copy one conversation's desktop data from `source` into `target` inside `tx`, keeping what
/// the target already has.
fn copy_conversation(
    tx: &rusqlite::Transaction<'_>,
    target: &str,
    source: &str,
    conversation: ConversationId,
    report: &mut CopyReport,
) -> Result<(), String> {
    let id = conversation.0 as i64;
    // A draft (and its attachments) only when the target has none.
    let copied = tx
        .execute(
            "INSERT INTO drafts (namespace, conversation_id, text, rev, updated_at)
             SELECT ?1, conversation_id, text, rev, updated_at FROM drafts
             WHERE namespace = ?2 AND conversation_id = ?3
               AND NOT EXISTS (SELECT 1 FROM drafts
                               WHERE namespace = ?1 AND conversation_id = ?3)",
            params![target, source, id],
        )
        .map_err(sql_err)?;
    if copied > 0 {
        report.drafts += copied;
        tx.execute(
            "INSERT OR IGNORE INTO draft_attachments
                (namespace, conversation_id, position, path, name, size)
             SELECT ?1, conversation_id, position, path, name, size FROM draft_attachments
             WHERE namespace = ?2 AND conversation_id = ?3",
            params![target, source, id],
        )
        .map_err(sql_err)?;
    }
    report.goals += tx
        .execute(
            "INSERT OR IGNORE INTO session_prefs (namespace, key, value)
             SELECT ?1, key, value FROM session_prefs
             WHERE namespace = ?2 AND key = 'goal:' || ?3",
            params![target, source, id],
        )
        .map_err(sql_err)?;
    // In submission order, so restored histories keep their order.
    report.journal += tx
        .execute(
            "INSERT OR IGNORE INTO request_journal
                (namespace, request_id, conversation_id, payload, state, created_at,
                 updated_at)
             SELECT ?1, request_id, conversation_id, payload, state, created_at, updated_at
             FROM request_journal WHERE namespace = ?2 AND conversation_id = ?3
             ORDER BY rowid",
            params![target, source, id],
        )
        .map_err(sql_err)?;
    let cached = tx
        .execute(
            "INSERT INTO transcript_cache
                (namespace, conversation_id, synced_at, has_older, message_count, items,
                 title, project_path, updated_at)
             SELECT ?1, conversation_id, synced_at, has_older, message_count, items,
                    title, project_path, updated_at
             FROM transcript_cache WHERE namespace = ?2 AND conversation_id = ?3
               AND NOT EXISTS (SELECT 1 FROM transcript_cache
                               WHERE namespace = ?1 AND conversation_id = ?3)",
            params![target, source, id],
        )
        .map_err(sql_err)?;
    if cached > 0 {
        report.caches += cached;
        tx.execute(
            "INSERT INTO transcript_search (text, namespace, conversation_id, item_id, at)
             SELECT text, ?1, conversation_id, item_id, at FROM transcript_search
             WHERE namespace = ?2 AND conversation_id = ?3",
            params![target, source, id],
        )
        .map_err(sql_err)?;
    }
    Ok(())
}

fn write(
    conn: &mut Connection,
    ns: &str,
    fail: &AtomicBool,
    fail_background: &AtomicBool,
    reporter: &Reporter,
    msg: Msg,
) {
    let injected = || {
        fail_background
            .load(Ordering::SeqCst)
            .then(|| Err(INJECTED_FAILURE.to_string()))
    };
    match msg {
        Msg::Draft {
            conversation,
            text,
            attachments,
            rev,
            ack,
        } => {
            let result = if fail.load(Ordering::SeqCst) {
                Err(INJECTED_FAILURE.to_string())
            } else {
                write_draft(conn, ns, conversation, &text, &attachments, rev)
            };
            ack(result);
        }
        Msg::Prefs(prefs) => {
            let result = injected().unwrap_or_else(|| write_prefs(conn, ns, &prefs));
            reporter.settle("preferences", result, "your preferences");
        }
        Msg::SetupCompletion { prefs, ack } => {
            ack(injected().unwrap_or_else(|| write_prefs(conn, ns, &prefs)));
        }
        Msg::Goal {
            conversation,
            text,
            paused,
        } => {
            let result = injected().unwrap_or_else(|| {
                let key = format!("goal:{}", conversation.0);
                if let Some(text) = text {
                    conn.execute(
                        "INSERT INTO session_prefs (namespace, key, value) VALUES (?1, ?2, ?3)
                         ON CONFLICT(namespace, key) DO UPDATE SET value = excluded.value",
                        params![
                            ns,
                            key,
                            json!({ "text": text, "paused": paused }).to_string()
                        ],
                    )
                } else {
                    conn.execute(
                        "DELETE FROM session_prefs WHERE namespace = ?1 AND key = ?2",
                        params![ns, key],
                    )
                }
                .map(|_| ())
                .map_err(sql_err)
            });
            reporter.settle("goal", result, "the goal");
        }
        Msg::Conversation(c) => {
            let result = injected().unwrap_or_else(|| write_conversation(conn, &c));
            reporter.settle("conversation", result, "the conversation list");
        }
        Msg::Project(path) => {
            let result = injected().unwrap_or_else(|| {
                conn.execute(
                    "INSERT OR IGNORE INTO projects (namespace, path, added_at) VALUES (?1, ?2, ?3)",
                    params![ns, path, now_secs()],
                )
                .map(|_| ())
                .map_err(sql_err)
            });
            reporter.settle("projects", result, "the project folder");
        }
        Msg::Intent {
            conversation,
            request,
            payload,
            ack,
        } => {
            let result = if fail.load(Ordering::SeqCst) {
                Err(INJECTED_FAILURE.to_string())
            } else {
                write_intent(conn, ns, conversation, &request, &payload)
            };
            ack(result);
        }
        Msg::Cache(write) => {
            let result = injected().unwrap_or_else(|| write_cache(conn, ns, &write));
            reporter.settle(
                "cache",
                result,
                "a copy of the conversation for offline reading",
            );
        }
        Msg::JournalState { request, state } => {
            let result =
                injected().unwrap_or_else(|| write_journal_state(conn, ns, &request, state));
            reporter.settle(
                "journal",
                result,
                "what became of a message (if you quit now, Pipkin asks the engine about it next time)",
            );
        }
    }
}

fn write_draft(
    conn: &mut Connection,
    ns: &str,
    id: ConversationId,
    text: &str,
    attachments: &[Attachment],
    rev: u64,
) -> Result<(), String> {
    let tx = conn.transaction().map_err(sql_err)?;
    tx.execute(
        "INSERT INTO drafts (namespace, conversation_id, text, rev, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(namespace, conversation_id) DO UPDATE SET
            text = excluded.text, rev = excluded.rev, updated_at = excluded.updated_at",
        params![ns, id.0 as i64, text, rev as i64, now_secs()],
    )
    .map_err(sql_err)?;
    // The text and its attachments commit together, so a draft never comes back half-saved.
    tx.execute(
        "DELETE FROM draft_attachments WHERE namespace = ?1 AND conversation_id = ?2",
        params![ns, id.0 as i64],
    )
    .map_err(sql_err)?;
    for (position, a) in attachments.iter().enumerate() {
        tx.execute(
            "INSERT INTO draft_attachments (namespace, conversation_id, position, path, name, size)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                ns,
                id.0 as i64,
                position as i64,
                a.path,
                a.name,
                a.size.map(|s| s as i64)
            ],
        )
        .map_err(sql_err)?;
    }
    tx.commit().map_err(sql_err)
}

fn encode_payload(
    text: &str,
    attachments: &[Attachment],
    model: Option<&str>,
    origin: Option<IntentOrigin>,
) -> String {
    let mut payload = json!({
        "text": text,
        "attachments": attachments
            .iter()
            .map(|a| json!({"path": a.path, "name": a.name, "size": a.size}))
            .collect::<Vec<_>>(),
        "model": model,
    });
    if let Some(origin) = origin {
        payload["origin"] = json!(match origin {
            IntentOrigin::Draft => "Draft",
            IntentOrigin::Retry => "Retry",
            IntentOrigin::Queue => "Queue",
            IntentOrigin::Goal => "Goal",
            IntentOrigin::Steer => "Steer",
            IntentOrigin::FollowUp => "FollowUp",
        });
    }
    payload.to_string()
}

fn decode_payload(payload: &str) -> Option<(String, Vec<Attachment>)> {
    let v: Value = serde_json::from_str(payload).ok()?;
    let text = v.get("text")?.as_str()?.to_string();
    let attachments = v
        .get("attachments")?
        .as_array()?
        .iter()
        .filter_map(|a| {
            Some(Attachment {
                path: a.get("path")?.as_str()?.to_string(),
                name: a.get("name")?.as_str()?.to_string(),
                size: a.get("size").and_then(Value::as_u64),
                error: None,
            })
        })
        .collect();
    Some((text, attachments))
}

fn write_intent(
    conn: &mut Connection,
    ns: &str,
    conversation: ConversationId,
    request: &RequestId,
    payload: &str,
) -> Result<(), String> {
    let tx = conn.transaction().map_err(sql_err)?;
    tx.execute(
        "INSERT INTO request_journal
            (namespace, request_id, conversation_id, payload, state, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 'intent', ?5, ?5)",
        params![ns, request.0, conversation.0 as i64, payload, now_secs()],
    )
    .map_err(sql_err)?;
    tx.commit().map_err(sql_err)
}

/// How the database was opened.
#[derive(Debug, PartialEq)]
pub enum Startup {
    Clean,
    /// The file was damaged and set aside; the message says what was done about it.
    Recovered(String),
}

fn is_damage(error: &str) -> bool {
    let error = error.to_lowercase();
    ["file is not a database", "malformed", "corrupt"]
        .iter()
        .any(|needle| error.contains(needle))
}

/// `path` with `suffix` appended to its file name.
fn beside(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// The migration backups of `path`, newest schema version first, that pass an integrity check.
fn intact_backups(path: &Path) -> Vec<(u32, PathBuf)> {
    let (Some(dir), Some(file)) = (path.parent(), path.file_name()) else {
        return vec![];
    };
    let prefix = format!("{}.bak-v", file.to_string_lossy());
    let mut found: Vec<(u32, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let version = name.strip_prefix(&prefix)?.parse::<u32>().ok()?;
            let ok = Connection::open_with_flags(
                entry.path(),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .ok()
            .and_then(|c| {
                c.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))
                    .ok()
            })
            .is_some_and(|verdict| verdict == "ok");
            ok.then(|| (version, entry.path()))
        })
        .collect();
    found.sort_by_key(|(version, _)| std::cmp::Reverse(*version));
    found
}

fn recover(path: &Path, namespace: &str, error: &str) -> Result<(Storage, Startup), String> {
    let aside = beside(path, &format!(".damaged-{}", now_secs()));
    std::fs::rename(path, &aside).map_err(|e| {
        format!("the database is damaged ({error}) and could not be set aside: {e}")
    })?;
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::rename(beside(path, suffix), beside(&aside, suffix));
    }
    for (version, backup) in intact_backups(path) {
        if std::fs::copy(&backup, path).is_ok()
            && let Ok(storage) = Storage::open(path, namespace)
        {
            return Ok((
                storage,
                Startup::Recovered(format!(
                    "Your saved data was damaged. It was restored from the backup made before \
                     the schema {version} upgrade, so recent drafts may be missing. The damaged \
                     file was kept as {}.",
                    aside.display()
                )),
            ));
        }
        let _ = std::fs::remove_file(path);
    }
    let storage = Storage::open(path, namespace)?;
    Ok((
        storage,
        Startup::Recovered(format!(
            "Your saved data was damaged and could not be restored, so Pipkin started fresh. \
             Drafts and unfinished sends from before are gone. The damaged file was kept as {}.",
            aside.display()
        )),
    ))
}

/// Terminal states are final: a late or duplicate update can never reopen a settled request.
fn write_journal_state(
    conn: &mut Connection,
    ns: &str,
    request: &RequestId,
    state: JournalState,
) -> Result<(), String> {
    conn.execute(
        &format!(
            "UPDATE request_journal SET state = ?1, updated_at = ?2
             WHERE namespace = ?3 AND request_id = ?4 AND state IN ({OPEN_STATES})"
        ),
        params![state.as_str(), now_secs(), ns, request.0],
    )
    .map(|_| ())
    .map_err(sql_err)
}

/// An FTS5 expression for what a person typed: each word quoted, the last one a prefix, so
/// punctuation never reaches FTS5's own syntax.
fn fts_expression(query: &str) -> Option<String> {
    let words: Vec<String> = query
        .split_whitespace()
        .map(|w| w.replace('"', ""))
        .filter(|w| !w.is_empty())
        .collect();
    let last = words.len().checked_sub(1)?;
    Some(
        words
            .iter()
            .enumerate()
            .map(|(i, w)| {
                if i == last {
                    format!("\"{w}\"*")
                } else {
                    format!("\"{w}\"")
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    )
}

fn write_cache(conn: &mut Connection, ns: &str, w: &CacheWrite) -> Result<(), String> {
    let tx = conn.transaction().map_err(sql_err)?;
    let id = w.conversation.0 as i64;
    tx.execute(
        "INSERT INTO transcript_cache
            (namespace, conversation_id, synced_at, has_older, message_count, items,
             title, project_path, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(namespace, conversation_id) DO UPDATE SET
            synced_at = excluded.synced_at, has_older = excluded.has_older,
            message_count = excluded.message_count, items = excluded.items,
            title = excluded.title, project_path = excluded.project_path,
            updated_at = excluded.updated_at",
        params![
            ns,
            id,
            w.synced_at,
            w.has_older as i64,
            w.kept as i64,
            w.items_json,
            w.meta.title,
            w.meta.project_path,
            w.meta.updated_at
        ],
    )
    .map_err(sql_err)?;
    tx.execute(
        "DELETE FROM transcript_search WHERE namespace = ?1 AND conversation_id = ?2",
        params![ns, id],
    )
    .map_err(sql_err)?;
    for (item, at, text) in &w.searchable {
        tx.execute(
            "INSERT INTO transcript_search (text, namespace, conversation_id, item_id, at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![text, ns, id, *item as i64, at],
        )
        .map_err(sql_err)?;
    }
    // Keep the most recently synced conversations only.
    let stale: Vec<i64> = {
        let mut stmt = tx
            .prepare(
                "SELECT conversation_id FROM transcript_cache WHERE namespace = ?1
                 ORDER BY synced_at DESC, rowid DESC LIMIT -1 OFFSET ?2",
            )
            .map_err(sql_err)?;
        stmt.query_map(params![ns, MAX_CACHED_CONVERSATIONS], |r| r.get(0))
            .map_err(sql_err)?
            .collect::<Result<_, _>>()
            .map_err(sql_err)?
    };
    for old in stale {
        tx.execute(
            "DELETE FROM transcript_cache WHERE namespace = ?1 AND conversation_id = ?2",
            params![ns, old],
        )
        .map_err(sql_err)?;
        tx.execute(
            "DELETE FROM transcript_search WHERE namespace = ?1 AND conversation_id = ?2",
            params![ns, old],
        )
        .map_err(sql_err)?;
    }
    tx.commit().map_err(sql_err)
}

fn write_conversation(conn: &mut Connection, c: &DemoConversation) -> Result<(), String> {
    let tx = conn.transaction().map_err(sql_err)?;
    tx.execute(
        "INSERT INTO demo_conversations (id, project_id, title, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET
            project_id = excluded.project_id, title = excluded.title, updated_at = excluded.updated_at",
        params![c.id.0 as i64, c.project.0 as i64, c.title, c.updated_at],
    )
    .map_err(sql_err)?;
    tx.commit().map_err(sql_err)
}

fn write_prefs(conn: &mut Connection, ns: &str, p: &Prefs) -> Result<(), String> {
    let entries: Vec<(&str, Value)> = vec![
        ("setup_completed", json!(p.setup_completed)),
        (
            "theme",
            json!(if p.theme == Theme::Light {
                "light"
            } else {
                "dark"
            }),
        ),
        (
            "text_size",
            json!(match p.text_size {
                TextSize::Small => "small",
                TextSize::Normal => "normal",
                TextSize::Large => "large",
            }),
        ),
        ("reduced_motion", json!(p.reduced_motion)),
        ("nav_width", json!(p.nav_width)),
        ("inspector_width", json!(p.inspector_width)),
        ("inspector_open", json!(p.inspector_open)),
        ("window_size", json!(p.window_size.map(|(w, h)| [w, h]))),
        ("selected_project", json!(p.selected_project.map(|i| i.0))),
        (
            "selected_conversation",
            json!(p.selected_conversation.map(|i| i.0)),
        ),
        ("model", json!(p.model)),
    ];
    let tx = conn.transaction().map_err(sql_err)?;
    for (key, value) in entries {
        if SESSION_PREF_KEYS.contains(&key) {
            tx.execute(
                "INSERT INTO session_prefs (namespace, key, value) VALUES (?1, ?2, ?3)
                 ON CONFLICT(namespace, key) DO UPDATE SET value = excluded.value",
                params![ns, key, value.to_string()],
            )
        } else {
            tx.execute(
                "INSERT INTO prefs (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value.to_string()],
            )
        }
        .map_err(sql_err)?;
    }
    tx.commit().map_err(sql_err)
}

fn apply_pref(p: &mut Prefs, key: &str, v: &Value) {
    match key {
        "setup_completed" => p.setup_completed = v.as_bool().unwrap_or(p.setup_completed),
        "theme" => match v.as_str() {
            Some("light") => p.theme = Theme::Light,
            Some("dark") => p.theme = Theme::Dark,
            _ => {}
        },
        "text_size" => match v.as_str() {
            Some("small") => p.text_size = TextSize::Small,
            Some("normal") => p.text_size = TextSize::Normal,
            Some("large") => p.text_size = TextSize::Large,
            _ => {}
        },
        "reduced_motion" => p.reduced_motion = v.as_bool().unwrap_or(p.reduced_motion),
        "nav_width" => p.nav_width = v.as_f64().map_or(p.nav_width, |x| x as f32),
        "inspector_width" => p.inspector_width = v.as_f64().map_or(p.inspector_width, |x| x as f32),
        "inspector_open" => p.inspector_open = v.as_bool().unwrap_or(p.inspector_open),
        "window_size" => {
            p.window_size = v.as_array().and_then(|a| match a.as_slice() {
                [w, h] => Some((w.as_f64()? as f32, h.as_f64()? as f32)),
                _ => None,
            })
        }
        "selected_project" => p.selected_project = v.as_u64().map(ProjectId),
        "selected_conversation" => p.selected_conversation = v.as_u64().map(ConversationId),
        "model" => p.model = v.as_str().map(str::to_string),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    fn tmp_db() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.sqlite3");
        (dir, path)
    }

    fn save_and_wait(s: &Storage, conv: u64, text: &str, rev: u64) -> Result<(), String> {
        let (tx, rx) = mpsc::channel();
        s.save_draft(
            ConversationId(conv),
            text.to_string(),
            vec![],
            rev,
            move |r| {
                tx.send(r).unwrap();
            },
        );
        rx.recv_timeout(Duration::from_secs(10)).unwrap()
    }

    #[test]
    fn conversation_data_is_copied_into_another_namespace_once_and_never_moved() {
        let (dir, path) = tmp_db();
        let (one, two, other) = (ConversationId(11), ConversationId(12), ConversationId(13));
        let attachment = Attachment {
            path: dir.path().join("a.txt").display().to_string(),
            name: "a.txt".into(),
            size: Some(2),
            error: None,
        };
        let pi = Storage::open(&path, "pi:test").unwrap();
        let (tx, rx) = mpsc::channel();
        pi.save_draft(
            one,
            "draft one".into(),
            vec![attachment.clone()],
            3,
            move |r| {
                tx.send(r).unwrap();
            },
        );
        rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
        save_and_wait(&pi, other.0, "not asked for", 1).unwrap();
        pi.save_goal(one, Some("ship it".into()), Some("Run stopped.".into()));
        for (request, text) in [("r1", "first prompt"), ("r2", "second prompt")] {
            let (tx, rx) = mpsc::channel();
            pi.journal_intent(one, RequestId(request.into()), text, &[], None, move |r| {
                tx.send(r).unwrap();
            });
            rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
        }
        pi.journal_state(RequestId("r1".into()), JournalState::Completed);
        let items = vec![TranscriptItem {
            id: ItemId(1024),
            at: 5,
            kind: ItemKind::User {
                text: "searchable zebra words".into(),
                attachments: vec![],
                delivery: Delivery::Sent,
                steer: false,
            },
        }];
        let meta = CacheMeta {
            title: "One".into(),
            project_path: "/work".into(),
            updated_at: 9,
        };
        pi.save_cache(one, meta.clone(), &items, true, 42);
        pi.save_cache(two, meta, &items, false, 43);
        pi.shutdown();

        let native = Storage::open(&path, "native:test").unwrap();
        // The target already has its own draft for `two`: it is kept.
        save_and_wait(&native, two.0, "native draft", 1).unwrap();
        let report = native
            .copy_conversations_from("pi:test", &[one, two])
            .unwrap();
        assert_eq!(
            report,
            CopyReport {
                drafts: 1,
                goals: 1,
                journal: 2,
                caches: 2
            }
        );
        let loaded = native.load_all().unwrap();
        let draft = loaded
            .drafts
            .iter()
            .find(|d| d.conversation == one)
            .unwrap();
        assert_eq!((draft.text.as_str(), draft.rev), ("draft one", 3));
        assert_eq!(draft.attachments, vec![attachment]);
        assert_eq!(
            loaded
                .drafts
                .iter()
                .find(|d| d.conversation == two)
                .unwrap()
                .text,
            "native draft"
        );
        assert!(loaded.drafts.iter().all(|d| d.conversation != other));
        assert_eq!(
            loaded.goals,
            vec![(one, "ship it".into(), Some("Run stopped.".into()))]
        );
        assert_eq!(
            loaded.prompt_histories,
            vec![(one, vec!["first prompt".into(), "second prompt".into()])]
        );
        assert_eq!(
            loaded.open_requests.len(),
            1,
            "the unresolved request comes along"
        );
        assert_eq!(loaded.open_requests[0].request, RequestId("r2".into()));
        let cache = native.load_cache(one).unwrap().unwrap();
        assert_eq!(
            (cache.items, cache.has_older, cache.synced_at),
            (items, true, 42)
        );
        let hits = native.search("zebra", 10).unwrap().hits;
        assert_eq!(hits.len(), 2);
        assert!(
            hits.iter()
                .any(|h| h.conversation == one && h.item == ItemId(1024))
        );

        // Again: nothing new, nothing duplicated.
        let again = native
            .copy_conversations_from("pi:test", &[one, two])
            .unwrap();
        assert_eq!(
            again,
            CopyReport {
                journal: 0,
                ..CopyReport::default()
            }
        );
        assert_eq!(native.search("zebra", 10).unwrap().hits.len(), 2);
        assert!(
            native
                .copy_conversations_from("native:test", &[one])
                .is_err()
        );
        native.shutdown();

        // The source is untouched.
        let source = Storage::open(&path, "pi:test").unwrap().load_all().unwrap();
        assert_eq!(source.drafts.len(), 2);
        assert_eq!(source.goals.len(), 1);
        assert_eq!(source.open_requests.len(), 1);
    }

    #[test]
    fn goals_are_namespaced_and_clear_survives_restart() {
        let (_dir, path) = tmp_db();
        {
            let s = Storage::open(&path, "real-a").unwrap();
            s.save_goal(ConversationId(7), Some("ship v1".into()), None);
            s.shutdown();
        }
        let s = Storage::open(&path, "real-a").unwrap();
        assert_eq!(
            s.load_all().unwrap().goals,
            vec![(ConversationId(7), "ship v1".into(), None)]
        );
        s.save_goal(ConversationId(7), None, None);
        s.shutdown();
        assert!(
            Storage::open(&path, "real-a")
                .unwrap()
                .load_all()
                .unwrap()
                .goals
                .is_empty()
        );
        assert!(
            Storage::open(&path, "real-b")
                .unwrap()
                .load_all()
                .unwrap()
                .goals
                .is_empty()
        );
    }

    #[test]
    fn setup_completion_is_only_acknowledged_after_a_durable_write() {
        let (_dir, path) = tmp_db();
        let s = Storage::open(&path, "setup-test").unwrap();
        let prefs = Prefs {
            setup_completed: true,
            ..Prefs::default()
        };
        let (tx, rx) = std::sync::mpsc::channel();
        s.set_background_failure(true);
        s.complete_setup(prefs.clone(), {
            let tx = tx.clone();
            move |result| tx.send(result).unwrap()
        });
        assert!(
            rx.recv_timeout(std::time::Duration::from_secs(2))
                .unwrap()
                .is_err()
        );
        assert!(!s.load_all().unwrap().prefs.setup_completed);
        s.set_background_failure(false);
        s.complete_setup(prefs, move |result| tx.send(result).unwrap());
        assert!(
            rx.recv_timeout(std::time::Duration::from_secs(2))
                .unwrap()
                .is_ok()
        );
        s.shutdown();
        let reopened = Storage::open(&path, "setup-test").unwrap();
        assert!(reopened.load_all().unwrap().prefs.setup_completed);
    }

    #[test]
    fn round_trip_prefs_drafts_and_conversations() {
        let (_dir, path) = tmp_db();
        let prefs = Prefs {
            setup_completed: true,
            theme: Theme::Light,
            text_size: TextSize::Large,
            reduced_motion: true,
            nav_width: 301.5,
            inspector_width: 512.0,
            inspector_open: false,
            window_size: Some((1280.0, 800.0)),
            selected_project: Some(ProjectId(2)),
            selected_conversation: Some(ConversationId(11)),
            model: Some("pi-opus".into()),
        };
        {
            let s = Storage::open(&path, DEMO_NAMESPACE).unwrap();
            save_and_wait(
                &s,
                3,
                "h\u{e9}llo \u{1F9D1}\u{200D}\u{1F680} \u{645}\u{631}\u{62d}\u{628}\u{627}",
                7,
            )
            .unwrap();
            save_and_wait(&s, 3, "second", 8).unwrap();
            s.save_prefs(&prefs);
            s.save_conversation(DemoConversation {
                id: ConversationId(40),
                project: ProjectId(1),
                title: "New \u{2728} conversation".into(),
                updated_at: 123,
            });
            s.shutdown();
        }
        let s = Storage::open(&path, DEMO_NAMESPACE).unwrap();
        let loaded = s.load_all().unwrap();
        assert_eq!(loaded.prefs, prefs);
        assert_eq!(
            loaded.drafts,
            vec![StoredDraft {
                conversation: ConversationId(3),
                text: "second".into(),
                attachments: vec![],
                rev: 8
            }]
        );
        assert_eq!(loaded.conversations.len(), 1);
        assert_eq!(loaded.conversations[0].title, "New \u{2728} conversation");
    }

    #[test]
    fn defaults_when_empty() {
        let (_dir, path) = tmp_db();
        let loaded = Storage::open(&path, DEMO_NAMESPACE)
            .unwrap()
            .load_all()
            .unwrap();
        assert_eq!(loaded.prefs, Prefs::default());
        assert!(loaded.drafts.is_empty() && loaded.conversations.is_empty());
    }

    #[test]
    fn injected_failure_reports_error_and_writes_nothing() {
        let (_dir, path) = tmp_db();
        let s = Storage::open(&path, DEMO_NAMESPACE).unwrap();
        s.set_write_failure(true);
        assert!(s.write_failure());
        let err = save_and_wait(&s, 1, "unsaved text", 1).unwrap_err();
        assert_eq!(err, "injected write failure");
        assert!(s.load_all().unwrap().drafts.is_empty());
        s.set_write_failure(false);
        save_and_wait(&s, 1, "unsaved text", 2).unwrap();
        assert_eq!(s.load_all().unwrap().drafts[0].text, "unsaved text");
    }

    #[test]
    fn ack_is_delivered_only_after_commit() {
        let (_dir, path) = tmp_db();
        let s = Storage::open(&path, DEMO_NAMESPACE).unwrap();
        let (tx, rx) = mpsc::channel();
        let reader_path = path.clone();
        s.save_draft(ConversationId(5), "durable".into(), vec![], 3, move |r| {
            // Inside the ack, an independent connection must already see the committed row.
            let conn = Connection::open(&reader_path).unwrap();
            let seen: Option<String> = conn
                .query_row(
                    "SELECT text FROM drafts WHERE namespace = 'demo' AND conversation_id = 5 AND rev = 3",
                    [],
                    |r| r.get(0),
                )
                .ok();
            tx.send((r, seen)).unwrap();
        });
        let (result, seen) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(result, Ok(()));
        assert_eq!(seen.as_deref(), Some("durable"));
    }

    #[test]
    fn shutdown_drains_queue() {
        let (_dir, path) = tmp_db();
        let s = Storage::open(&path, DEMO_NAMESPACE).unwrap();
        for rev in 1..=50u64 {
            s.save_draft(ConversationId(1), format!("rev {rev}"), vec![], rev, |_| {});
        }
        s.shutdown();
        s.save_draft(ConversationId(1), "late".into(), vec![], 99, |r| {
            assert!(r.is_err())
        });
        let drafts = Storage::open(&path, DEMO_NAMESPACE)
            .unwrap()
            .load_all()
            .unwrap()
            .drafts;
        assert_eq!(drafts[0].rev, 50);
    }

    #[test]
    fn migrates_an_older_user_version() {
        let (_dir, path) = tmp_db();
        {
            // A "version 1" database: no demo_conversations table yet.
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(MIGRATIONS[0]).unwrap();
            conn.execute_batch("PRAGMA user_version = 1").unwrap();
            conn.execute("INSERT INTO drafts VALUES (9, 'old draft', 4, 0)", [])
                .unwrap();
            conn.execute("INSERT INTO prefs VALUES ('theme', '\"light\"')", [])
                .unwrap();
        }
        let s = Storage::open(&path, DEMO_NAMESPACE).unwrap();
        let loaded = s.load_all().unwrap();
        assert_eq!(loaded.drafts[0].text, "old draft");
        assert_eq!(loaded.prefs.theme, Theme::Light);
        assert!(loaded.conversations.is_empty());
        let conn = Connection::open(&path).unwrap();
        let v: u32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
    }

    #[test]
    fn an_upgrade_from_every_earlier_schema_keeps_the_draft_and_makes_a_backup() {
        for from in 1..SCHEMA_VERSION {
            let (_dir, path) = tmp_db();
            {
                let conn = Connection::open(&path).unwrap();
                for sql in &MIGRATIONS[..from as usize] {
                    conn.execute_batch(sql).unwrap();
                }
                conn.execute_batch(&format!("PRAGMA user_version = {from}"))
                    .unwrap();
                // Drafts gained a namespace column in schema 3.
                let insert = if from < 3 {
                    "INSERT INTO drafts VALUES (9, 'unsent words', 4, 0)".to_owned()
                } else {
                    format!(
                        "INSERT INTO drafts (namespace, conversation_id, text, rev, updated_at) \
                         VALUES ('{DEMO_NAMESPACE}', 9, 'unsent words', 4, 0)"
                    )
                };
                conn.execute(&insert, []).unwrap();
            }
            let s = Storage::open(&path, DEMO_NAMESPACE).unwrap();
            let loaded = s.load_all().unwrap();
            assert!(
                loaded.drafts.iter().any(|d| d.text == "unsent words"),
                "the draft survived an upgrade from schema {from}"
            );
            assert!(
                intact_backups(&path).iter().any(|(v, _)| *v == from),
                "a backup of schema {from} was kept for rolling back"
            );
            s.shutdown();
            assert!(
                inspect(&path)
                    .unwrap()
                    .contains(&format!("schema {SCHEMA_VERSION}"))
            );
        }
    }

    #[test]
    fn inspect_reports_without_changing_anything() {
        let (_dir, path) = tmp_db();
        assert!(inspect(&path).unwrap().contains("none yet"));
        assert!(!path.exists(), "inspecting does not create a database");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(MIGRATIONS[0]).unwrap();
            conn.execute_batch("PRAGMA user_version = 1").unwrap();
        }
        assert!(inspect(&path).unwrap().contains("schema 1"));
        let conn = Connection::open(&path).unwrap();
        let v: u32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 1, "an old database is not migrated by looking at it");
        conn.execute_batch(&format!("PRAGMA user_version = {}", SCHEMA_VERSION + 1))
            .unwrap();
        assert!(inspect(&path).unwrap_err().contains("newer"));
    }

    #[test]
    fn namespaces_isolate_drafts_and_session_prefs_but_share_app_prefs() {
        let (_dir, path) = tmp_db();
        let demo = Prefs {
            theme: Theme::Light,
            selected_conversation: Some(ConversationId(3)),
            model: Some("demo-model".into()),
            ..Prefs::default()
        };
        {
            let s = Storage::open(&path, DEMO_NAMESPACE).unwrap();
            save_and_wait(&s, 3, "demo draft", 1).unwrap();
            s.save_prefs(&demo);
            s.shutdown();
        }
        let real = Storage::open(&path, "pi:local").unwrap();
        let loaded = real.load_all().unwrap();
        assert!(loaded.drafts.is_empty());
        assert!(loaded.conversations.is_empty());
        assert_eq!(loaded.prefs.selected_conversation, None);
        assert_eq!(loaded.prefs.model, None);
        assert_eq!(loaded.prefs.theme, Theme::Light, "app prefs are shared");

        save_and_wait(&real, 3, "real draft", 1).unwrap();
        real.save_prefs(&Prefs {
            selected_conversation: Some(ConversationId(3)),
            model: Some("real-model".into()),
            ..Prefs::default()
        });
        real.shutdown();
        let back = Storage::open(&path, DEMO_NAMESPACE)
            .unwrap()
            .load_all()
            .unwrap();
        assert_eq!(back.drafts[0].text, "demo draft");
        assert_eq!(back.prefs.model.as_deref(), Some("demo-model"));
    }

    #[test]
    fn migration_moves_existing_rows_into_demo_namespace_and_backs_up() {
        let (_dir, path) = tmp_db();
        {
            // A "version 2" database: global drafts and prefs.
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(MIGRATIONS[0]).unwrap();
            conn.execute_batch(MIGRATIONS[1]).unwrap();
            conn.execute_batch("PRAGMA user_version = 2").unwrap();
            conn.execute("INSERT INTO drafts VALUES (9, 'old draft', 4, 0)", [])
                .unwrap();
            for (k, v) in [
                ("theme", "\"light\""),
                ("selected_conversation", "9"),
                ("model", "\"m\""),
            ] {
                conn.execute("INSERT INTO prefs VALUES (?1, ?2)", [k, v])
                    .unwrap();
            }
        }
        let loaded = Storage::open(&path, "pi:local")
            .unwrap()
            .load_all()
            .unwrap();
        assert!(loaded.drafts.is_empty());
        assert_eq!(loaded.prefs.selected_conversation, None);
        assert_eq!(loaded.prefs.theme, Theme::Light);

        let demo = Storage::open(&path, DEMO_NAMESPACE)
            .unwrap()
            .load_all()
            .unwrap();
        assert_eq!(demo.drafts[0].text, "old draft");
        assert_eq!(demo.prefs.selected_conversation, Some(ConversationId(9)));
        assert_eq!(demo.prefs.model.as_deref(), Some("m"));

        // The pre-migration copy is a readable version-2 database with the old layout.
        let mut bak = path.as_os_str().to_owned();
        bak.push(".bak-v2");
        let conn = Connection::open(PathBuf::from(bak)).unwrap();
        let v: u32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 2);
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM drafts WHERE conversation_id = 9",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    fn journal_and_wait(
        s: &Storage,
        conv: u64,
        request: &str,
        text: &str,
        attachments: &[Attachment],
    ) -> Result<(), String> {
        let (tx, rx) = mpsc::channel();
        s.journal_intent_with_origin(
            ConversationId(conv),
            PendingIntent {
                request: RequestId(request.into()),
                origin: IntentOrigin::Draft,
                text: text.into(),
                attachments: attachments.to_vec(),
                model: Some("pi-sonnet".into()),
            },
            move |r| tx.send(r).unwrap(),
        );
        rx.recv_timeout(Duration::from_secs(10)).unwrap()
    }

    fn settle_journal(s: &Storage, request: &str, state: JournalState) {
        s.journal_state(RequestId(request.into()), state);
        s.shutdown(); // drains the queue, so the update has been written
    }

    #[test]
    fn journal_intent_is_durable_and_recoverable_until_terminal() {
        let (_dir, path) = tmp_db();
        let attachment = Attachment {
            path: "/p/a.txt".into(),
            name: "a.txt".into(),
            size: Some(12),
            error: None,
        };
        {
            let s = Storage::open(&path, DEMO_NAMESPACE).unwrap();
            journal_and_wait(
                &s,
                1,
                "r1",
                "caf\u{e9} \u{1F680}",
                std::slice::from_ref(&attachment),
            )
            .unwrap();
            journal_and_wait(&s, 2, "r2", "second", &[]).unwrap();
            journal_and_wait(&s, 1, "r3", "settled", &[]).unwrap();
            s.journal_state(RequestId("r1".into()), JournalState::Accepted);
            s.journal_state(RequestId("r2".into()), JournalState::Unknown);
            s.journal_state(RequestId("r3".into()), JournalState::Completed);
            // A late update can never reopen or rewrite a settled request.
            s.journal_state(RequestId("r3".into()), JournalState::Accepted);
            s.shutdown();
        }
        let open = Storage::open(&path, DEMO_NAMESPACE)
            .unwrap()
            .load_all()
            .unwrap()
            .open_requests;
        assert_eq!(
            open,
            vec![
                OpenRequest {
                    conversation: ConversationId(1),
                    request: RequestId("r1".into()),
                    text: "caf\u{e9} \u{1F680}".into(),
                    attachments: vec![attachment],
                },
                OpenRequest {
                    conversation: ConversationId(2),
                    request: RequestId("r2".into()),
                    text: "second".into(),
                    attachments: vec![],
                },
            ]
        );
        // Other namespaces never see them.
        let other = Storage::open(&path, "pi:local")
            .unwrap()
            .load_all()
            .unwrap();
        assert!(other.open_requests.is_empty());
    }

    #[test]
    fn prompt_history_survives_terminal_states_and_is_scoped() {
        let (_dir, path) = tmp_db();
        {
            let s = Storage::open(&path, "pi:first").unwrap();
            for (request, text, state) in [
                ("r1", "submitted", JournalState::Completed),
                ("r2", "retry", JournalState::Rejected),
                ("r3", "steer", JournalState::Cancelled),
            ] {
                journal_and_wait(&s, 1, request, text, &[]).unwrap();
                s.journal_state(RequestId(request.into()), state);
            }
            journal_and_wait(&s, 2, "r4", "other session", &[]).unwrap();
            journal_and_wait(&s, 1, "r5", "unsettled", &[]).unwrap();
            s.shutdown();
        }
        {
            let s = Storage::open(&path, "pi:second").unwrap();
            journal_and_wait(&s, 1, "r1", "other namespace", &[]).unwrap();
            settle_journal(&s, "r1", JournalState::Completed);
        }
        let loaded = Storage::open(&path, "pi:first")
            .unwrap()
            .load_all()
            .unwrap();
        assert_eq!(
            loaded.prompt_histories,
            vec![
                (
                    ConversationId(1),
                    vec![
                        "submitted".into(),
                        "retry".into(),
                        "steer".into(),
                        "unsettled".into()
                    ]
                ),
                (ConversationId(2), vec!["other session".into()]),
            ]
        );
        assert_eq!(loaded.open_requests.len(), 2);
        let other = Storage::open(&path, "pi:second")
            .unwrap()
            .load_all()
            .unwrap();
        assert_eq!(
            other.prompt_histories,
            vec![(ConversationId(1), vec!["other namespace".into()])]
        );
        assert!(other.open_requests.is_empty());
    }

    #[test]
    fn journaled_origins_restore_user_prompts_but_not_goals() {
        let (_dir, path) = tmp_db();
        let s = Storage::open(&path, DEMO_NAMESPACE).unwrap();
        let origins = [
            IntentOrigin::Draft,
            IntentOrigin::Retry,
            IntentOrigin::Queue,
            IntentOrigin::Steer,
            IntentOrigin::FollowUp,
            IntentOrigin::Goal,
        ];
        let states = [
            JournalState::Unknown,
            JournalState::Accepted,
            JournalState::Rejected,
            JournalState::Completed,
            JournalState::Failed,
            JournalState::Cancelled,
            JournalState::Queued,
            JournalState::Superseded,
        ];
        let mut expected = vec![];
        for origin in origins {
            for state in states {
                let text = format!("{origin:?}-{state:?}");
                let request = RequestId(text.clone());
                let (tx, rx) = mpsc::channel();
                s.journal_intent_with_origin(
                    ConversationId(1),
                    PendingIntent {
                        request: request.clone(),
                        origin,
                        text: text.clone(),
                        attachments: vec![],
                        model: Some("pi-sonnet".into()),
                    },
                    move |r| tx.send(r).unwrap(),
                );
                rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
                s.journal_state(request, state);
                if origin != IntentOrigin::Goal {
                    expected.push(text);
                }
            }
        }
        s.shutdown();
        let conn = Connection::open(&path).unwrap();
        let payload: String = conn
            .query_row(
                "SELECT payload FROM request_journal WHERE request_id = 'Goal-Completed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let payload: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(payload["origin"], "Goal");
        assert_eq!(payload["model"], "pi-sonnet");
        drop(conn);
        let loaded = Storage::open(&path, DEMO_NAMESPACE)
            .unwrap()
            .load_all()
            .unwrap();
        assert_eq!(loaded.prompt_histories, vec![(ConversationId(1), expected)]);
        // Origin filtering applies only to prompt recall, not request reconciliation.
        assert_eq!(loaded.open_requests.len(), origins.len() * 2);
        assert!(
            loaded
                .open_requests
                .iter()
                .any(|r| r.text == "Goal-Unknown")
        );
    }

    #[test]
    fn prompt_history_is_recent_bounded_and_chronological() {
        let (_dir, path) = tmp_db();
        Storage::open(&path, DEMO_NAMESPACE).unwrap().shutdown();
        let mut conn = Connection::open(&path).unwrap();
        for i in 0..205 {
            let origin = ["Draft", "Retry", "Steer", "FollowUp"][i % 4];
            write_intent(
                &mut conn,
                DEMO_NAMESPACE,
                ConversationId(1),
                &RequestId(format!("r{i}")),
                &json!({"text": format!("prompt {i}"), "origin": origin}).to_string(),
            )
            .unwrap();
        }
        // These newer rows must not displace valid user entries. The byte cap is UTF-8,
        // not a character count; no partial prompt should be restored.
        for (request, payload) in [
            (
                "goal",
                json!({"text": "autonomous", "origin": "Goal"}).to_string(),
            ),
            (
                "large",
                json!({"text": "é".repeat(MAX_HISTORY_PROMPT_BYTES / 2 + 1)}).to_string(),
            ),
            ("broken", "not json".into()),
            ("no-text", json!({"attachments": []}).to_string()),
        ] {
            write_intent(
                &mut conn,
                DEMO_NAMESPACE,
                ConversationId(1),
                &RequestId(request.into()),
                &payload,
            )
            .unwrap();
        }
        // Submission order, not terminal-update order (or coarse timestamps), is authoritative.
        conn.execute(
            "UPDATE request_journal SET created_at = 0, updated_at = 999, state = 'completed'",
            [],
        )
        .unwrap();
        write_intent(
            &mut conn,
            DEMO_NAMESPACE,
            ConversationId(2),
            &RequestId("session2".into()),
            &encode_payload("legacy payload", &[], None, None),
        )
        .unwrap();
        drop(conn);
        let loaded = Storage::open(&path, DEMO_NAMESPACE)
            .unwrap()
            .load_all()
            .unwrap();
        assert_eq!(
            loaded.prompt_histories,
            vec![
                (
                    ConversationId(1),
                    (5..205).map(|i| format!("prompt {i}")).collect()
                ),
                (ConversationId(2), vec!["legacy payload".into()]),
            ]
        );
    }

    #[test]
    fn terminal_request_is_not_recovered() {
        let (_dir, path) = tmp_db();
        let s = Storage::open(&path, DEMO_NAMESPACE).unwrap();
        journal_and_wait(&s, 1, "r1", "done", &[]).unwrap();
        settle_journal(&s, "r1", JournalState::Rejected);
        let s = Storage::open(&path, DEMO_NAMESPACE).unwrap();
        assert!(s.load_all().unwrap().open_requests.is_empty());
    }

    #[test]
    fn duplicate_request_id_and_injected_failure_are_reported() {
        let (_dir, path) = tmp_db();
        let s = Storage::open(&path, DEMO_NAMESPACE).unwrap();
        journal_and_wait(&s, 1, "r1", "once", &[]).unwrap();
        assert!(journal_and_wait(&s, 1, "r1", "again", &[]).is_err());
        s.set_write_failure(true);
        assert_eq!(
            journal_and_wait(&s, 1, "r2", "x", &[]).unwrap_err(),
            INJECTED_FAILURE
        );
        s.set_write_failure(false);
        let open = s.load_all().unwrap().open_requests;
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].text, "once");
    }

    #[test]
    fn journal_ack_is_delivered_only_after_commit() {
        let (_dir, path) = tmp_db();
        let s = Storage::open(&path, DEMO_NAMESPACE).unwrap();
        let (tx, rx) = mpsc::channel();
        let reader = path.clone();
        s.journal_intent(
            ConversationId(7),
            RequestId("r7".into()),
            "durable",
            &[],
            None,
            move |r| {
                let conn = Connection::open(&reader).unwrap();
                let seen: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM request_journal WHERE request_id = 'r7'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                tx.send((r, seen)).unwrap();
            },
        );
        let (result, seen) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(result, Ok(()));
        assert_eq!(seen, 1);
    }

    #[test]
    fn projects_round_trip_in_order_and_are_namespaced() {
        let (_dir, path) = tmp_db();
        {
            let s = Storage::open(&path, "pi:a").unwrap();
            s.save_project("/work/one".into());
            s.save_project("/work/two".into());
            s.save_project("/work/one".into()); // idempotent
            s.shutdown();
            let other = Storage::open(&path, "pi:b").unwrap();
            other.save_project("/elsewhere".into());
            other.shutdown();
        }
        let a = Storage::open(&path, "pi:a").unwrap().load_all().unwrap();
        assert_eq!(a.projects, ["/work/one", "/work/two"]);
        let b = Storage::open(&path, "pi:b").unwrap().load_all().unwrap();
        assert_eq!(b.projects, ["/elsewhere"]);
        assert!(
            Storage::open(&path, DEMO_NAMESPACE)
                .unwrap()
                .load_all()
                .unwrap()
                .projects
                .is_empty()
        );
    }

    #[test]
    fn refuses_a_newer_database() {
        let (_dir, path) = tmp_db();
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(&format!("PRAGMA user_version = {}", SCHEMA_VERSION + 1))
                .unwrap();
        }
        let err = Storage::open(&path, DEMO_NAMESPACE)
            .err()
            .expect("must refuse");
        assert!(err.contains("newer"), "{err}");
    }

    fn kill_text(conv: u64, rev: u64) -> String {
        format!("draft for conversation {conv}, revision {rev} \u{2014} \u{1F9EA} e\u{301}")
    }

    /// Child half of the kill test: only active when the parent sets the env switch.
    #[test]
    fn kill_child_writer() {
        let Some(dir) = std::env::var_os("PIPKIN_KILL_DIR") else {
            return;
        };
        let s = Storage::open(&PathBuf::from(dir).join("kill.sqlite3"), DEMO_NAMESPACE).unwrap();
        for rev in 1..=100_000u64 {
            let conv = rev % 3 + 1;
            let (tx, rx) = mpsc::channel();
            s.save_draft(
                ConversationId(conv),
                kill_text(conv, rev),
                vec![],
                rev,
                move |r| {
                    tx.send(r).unwrap();
                },
            );
            rx.recv().unwrap().unwrap();
            println!("ACK {conv} {rev}");
        }
    }

    #[test]
    fn acknowledged_drafts_survive_sigkill() {
        let dir = tempfile::tempdir().unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "storage::tests::kill_child_writer",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("PIPKIN_KILL_DIR", dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut acked: Vec<(u64, u64)> = Vec::new();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        for line in lines {
            let line = line.unwrap();
            if let Some(rest) = line.strip_prefix("ACK ") {
                let mut parts = rest.split(' ').map(|p| p.parse::<u64>().unwrap());
                acked.push((parts.next().unwrap(), parts.next().unwrap()));
                if acked.len() >= 40 {
                    break;
                }
            }
        }
        child.kill().unwrap(); // SIGKILL on unix: no destructors, no flush.
        child.wait().unwrap();
        assert!(
            acked.len() >= 40,
            "child produced only {} acks",
            acked.len()
        );

        let s = Storage::open(&dir.path().join("kill.sqlite3"), DEMO_NAMESPACE).unwrap();
        let drafts = s.load_all().unwrap().drafts;
        for conv in 1..=3u64 {
            let max_acked = acked
                .iter()
                .filter(|a| a.0 == conv)
                .map(|a| a.1)
                .max()
                .unwrap();
            let stored = drafts
                .iter()
                .find(|d| d.conversation == ConversationId(conv))
                .unwrap();
            assert!(
                stored.rev >= max_acked,
                "conversation {conv}: lost acknowledged rev {max_acked}"
            );
            assert_eq!(
                stored.text,
                kill_text(conv, stored.rev),
                "conversation {conv} corrupted"
            );
        }
    }

    fn save_with(s: &Storage, conv: u64, text: &str, attachments: Vec<Attachment>, rev: u64) {
        let (tx, rx) = mpsc::channel();
        s.save_draft(
            ConversationId(conv),
            text.to_string(),
            attachments,
            rev,
            move |r| {
                tx.send(r).unwrap();
            },
        );
        rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
    }

    fn attachment(name: &str, size: Option<u64>) -> Attachment {
        Attachment {
            path: format!("/work/{name}"),
            name: name.into(),
            size,
            error: None,
        }
    }

    #[test]
    fn drafts_keep_their_attachments_in_order_and_replace_them_together() {
        let (_dir, path) = tmp_db();
        {
            let s = Storage::open(&path, "pi:test").unwrap();
            save_with(
                &s,
                1,
                "look",
                vec![attachment("b.txt", Some(2)), attachment("a.png", None)],
                1,
            );
            save_with(&s, 2, "other", vec![attachment("z.txt", Some(9))], 1);
            // A later save replaces the whole set, not adds to it.
            save_with(&s, 1, "look again", vec![attachment("c.txt", Some(3))], 2);
            s.shutdown();
        }
        let loaded = Storage::open(&path, "pi:test").unwrap().load_all().unwrap();
        let one = loaded
            .drafts
            .iter()
            .find(|d| d.conversation.0 == 1)
            .unwrap();
        assert_eq!(one.text, "look again");
        assert_eq!(one.attachments, [attachment("c.txt", Some(3))]);
        let two = loaded
            .drafts
            .iter()
            .find(|d| d.conversation.0 == 2)
            .unwrap();
        assert_eq!(two.attachments, [attachment("z.txt", Some(9))]);
        // Order and a missing size survive too.
        {
            let s = Storage::open(&path, "pi:test").unwrap();
            save_with(
                &s,
                3,
                "",
                vec![attachment("b.txt", Some(2)), attachment("a.png", None)],
                1,
            );
        }
        let loaded = Storage::open(&path, "pi:test").unwrap().load_all().unwrap();
        let three = loaded
            .drafts
            .iter()
            .find(|d| d.conversation.0 == 3)
            .unwrap();
        assert_eq!(
            three
                .attachments
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>(),
            ["b.txt", "a.png"]
        );
        assert_eq!(three.attachments[1].size, None);
    }

    #[test]
    fn attachments_are_kept_per_backend_namespace() {
        let (_dir, path) = tmp_db();
        let demo = Storage::open(&path, DEMO_NAMESPACE).unwrap();
        save_with(&demo, 1, "d", vec![attachment("demo.txt", Some(1))], 1);
        demo.shutdown();
        let real = Storage::open(&path, "pi:real").unwrap();
        save_with(&real, 1, "r", vec![], 1);
        real.shutdown();
        let loaded = Storage::open(&path, DEMO_NAMESPACE)
            .unwrap()
            .load_all()
            .unwrap();
        assert_eq!(loaded.drafts[0].attachments.len(), 1);
        let loaded = Storage::open(&path, "pi:real").unwrap().load_all().unwrap();
        assert!(loaded.drafts[0].attachments.is_empty());
    }

    #[test]
    fn a_damaged_database_is_set_aside_and_a_fresh_one_started() {
        let (dir, path) = tmp_db();
        std::fs::write(&path, b"this is not a database at all, just text bytes").unwrap();
        let (storage, startup) = Storage::open_recovering(&path, "pi:test").unwrap();
        let Startup::Recovered(message) = startup else {
            panic!("expected a recovery");
        };
        assert!(message.contains("could not be restored"), "{message}");
        assert!(message.contains(".damaged-"), "{message}");
        let kept: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".damaged-"))
            .collect();
        assert_eq!(kept.len(), 1, "the damaged file is kept, not deleted");
        assert_eq!(
            std::fs::read(kept[0].path()).unwrap(),
            b"this is not a database at all, just text bytes"
        );
        // The fresh database works.
        save_with(&storage, 1, "after", vec![], 1);
        storage.shutdown();
        let loaded = Storage::open(&path, "pi:test").unwrap().load_all().unwrap();
        assert_eq!(loaded.drafts[0].text, "after");
    }

    #[test]
    fn a_damaged_database_is_restored_from_the_newest_intact_backup() {
        let (_dir, path) = tmp_db();
        {
            let s = Storage::open(&path, "pi:test").unwrap();
            save_with(
                &s,
                1,
                "kept in backup",
                vec![attachment("a.txt", Some(1))],
                1,
            );
            s.shutdown();
        }
        let mut backup = path.as_os_str().to_owned();
        backup.push(format!(".bak-v{SCHEMA_VERSION}"));
        // `copy` of a WAL database needs it checkpointed; opening and closing did that.
        std::fs::copy(&path, PathBuf::from(&backup)).unwrap();
        // An older, equally damaged backup must not be chosen over a good one.
        let mut worse = path.as_os_str().to_owned();
        worse.push(".bak-v1");
        std::fs::write(PathBuf::from(&worse), b"garbage").unwrap();
        std::fs::write(&path, b"damaged damaged damaged damaged").unwrap();
        let _ = std::fs::remove_file(PathBuf::from(format!("{}-wal", path.display())));
        let _ = std::fs::remove_file(PathBuf::from(format!("{}-shm", path.display())));

        let (storage, startup) = Storage::open_recovering(&path, "pi:test").unwrap();
        let Startup::Recovered(message) = startup else {
            panic!("expected a recovery");
        };
        assert!(message.contains("restored from the backup"), "{message}");
        let loaded = storage.load_all().unwrap();
        assert_eq!(loaded.drafts[0].text, "kept in backup");
        assert_eq!(loaded.drafts[0].attachments.len(), 1);
    }

    #[test]
    fn a_database_from_a_newer_build_is_refused_and_left_untouched() {
        let (_dir, path) = tmp_db();
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("CREATE TABLE future (x); PRAGMA user_version = 99;")
                .unwrap();
        }
        let before = std::fs::read(&path).unwrap();
        let error = match Storage::open_recovering(&path, "pi:test") {
            Err(error) => error,
            Ok(_) => panic!("a newer database must not be opened"),
        };
        assert!(error.contains("newer"), "{error}");
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            !names.iter().any(|n| n.contains("damaged")),
            "a newer file is not damaged: {names:?}"
        );
    }

    #[test]
    fn a_failed_background_write_is_reported_once_until_it_works_again() {
        let (_dir, path) = tmp_db();
        let s = Storage::open(&path, "pi:test").unwrap();
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = seen.clone();
        s.set_error_sink(Arc::new(move |m| sink.lock().unwrap().push(m)));

        s.set_background_failure(true);
        s.save_prefs(&Prefs::default());
        s.save_prefs(&Prefs::default());
        s.journal_state(RequestId("r".into()), JournalState::Completed);
        s.save_project("/p".into());
        // A draft write is acknowledged on its own channel; flush the queue through it.
        save_with(&s, 1, "x", vec![], 1);
        {
            let seen = seen.lock().unwrap();
            assert_eq!(seen.len(), 3, "one per kind, not one per write: {seen:?}");
            assert!(seen.iter().any(|m| m.contains("your preferences")));
            assert!(seen.iter().any(|m| m.contains("asks the engine about it")));
            assert!(seen.iter().any(|m| m.contains("project folder")));
            assert!(seen.iter().all(|m| m.contains(INJECTED_FAILURE)));
        }

        s.set_background_failure(false);
        s.save_prefs(&Prefs::default());
        save_with(&s, 1, "y", vec![], 2);
        s.set_background_failure(true);
        s.save_prefs(&Prefs::default());
        save_with(&s, 1, "z", vec![], 3);
        assert_eq!(
            seen.lock().unwrap().len(),
            4,
            "a new failure after a success is reported again"
        );
    }

    fn text_item(id: u64, role_user: bool, text: &str) -> TranscriptItem {
        TranscriptItem {
            id: ItemId(id),
            at: id as i64,
            kind: if role_user {
                ItemKind::User {
                    text: text.into(),
                    attachments: vec![],
                    delivery: Delivery::Sent,
                    steer: false,
                }
            } else {
                ItemKind::Assistant {
                    text: text.into(),
                    streaming: false,
                }
            },
        }
    }

    fn meta(title: &str) -> CacheMeta {
        CacheMeta {
            title: title.into(),
            project_path: "/work/p".into(),
            updated_at: 5,
        }
    }

    /// Wait until every queued write has been committed (a draft ack follows them in order).
    fn flush(s: &Storage) {
        save_and_wait(s, 9_999, "flush", 1).unwrap();
    }

    #[test]
    fn a_saved_copy_reads_back_and_a_newer_one_replaces_it_entirely() {
        let (_dir, path) = tmp_db();
        let s = Storage::open(&path, "pi:test").unwrap();
        assert_eq!(s.load_cache(ConversationId(1)).unwrap(), None);
        s.save_cache(
            ConversationId(1),
            meta("t"),
            &[
                text_item(1024, true, "first question"),
                text_item(2048, false, "first answer"),
            ],
            true,
            100,
        );
        flush(&s);
        let copy = s.load_cache(ConversationId(1)).unwrap().unwrap();
        assert_eq!(
            (copy.items.len(), copy.has_older, copy.synced_at),
            (2, true, 100)
        );
        s.save_cache(
            ConversationId(1),
            meta("t"),
            &[text_item(3072, true, "only this now")],
            false,
            200,
        );
        flush(&s);
        let copy = s.load_cache(ConversationId(1)).unwrap().unwrap();
        assert_eq!(
            (copy.items.len(), copy.has_older, copy.synced_at),
            (1, false, 200)
        );
        // The replaced messages are no longer found.
        assert!(s.search("first", 10).unwrap().hits.is_empty());
        assert_eq!(s.search("only", 10).unwrap().hits.len(), 1);
    }

    #[test]
    fn search_finds_words_anywhere_with_the_match_marked_and_says_what_it_covered() {
        let (_dir, path) = tmp_db();
        let s = Storage::open(&path, "pi:test").unwrap();
        s.save_cache(
            ConversationId(1),
            meta("t"),
            &[
                text_item(1024, true, "How do I rotate the signing keys?"),
                text_item(
                    2048,
                    false,
                    "Rotate them with the keytool, then restart the service.",
                ),
            ],
            false,
            1,
        );
        s.save_cache(
            ConversationId(2),
            meta("t"),
            &[text_item(1024, true, "unrelated chatter about lunch")],
            false,
            2,
        );
        flush(&s);
        let r = s.search("rotate keys", 10).unwrap();
        assert_eq!(r.hits.len(), 1, "every word must match: {:?}", r.hits);
        assert_eq!(
            (r.hits[0].conversation, r.hits[0].item),
            (ConversationId(1), ItemId(1024))
        );
        assert!(
            r.hits[0].snippet.contains("\u{2}rotate\u{3}"),
            "{}",
            r.hits[0].snippet
        );
        assert_eq!((r.conversations_searched, r.messages_searched), (2, 3));
        // The last word matches as a prefix, so results appear while typing.
        assert_eq!(s.search("keyt", 10).unwrap().hits.len(), 1);
        // Accents and case are ignored.
        assert_eq!(
            s.search("LUNCH", 10).unwrap().hits[0].conversation,
            ConversationId(2)
        );
        assert!(s.search("zebra", 10).unwrap().hits.is_empty());
    }

    #[test]
    fn what_was_typed_never_reaches_the_search_syntax() {
        let (_dir, path) = tmp_db();
        let s = Storage::open(&path, "pi:test").unwrap();
        s.save_cache(
            ConversationId(1),
            meta("t"),
            &[text_item(1024, true, "a \"quoted\" word and x*y NEAR(z)")],
            false,
            1,
        );
        flush(&s);
        for q in [
            "\"", "\"quoted", "AND", "OR NOT", "x*y", "NEAR(", "col:umn", "(", "-", "\\", "''",
        ] {
            assert!(s.search(q, 10).is_ok(), "{q:?} must not be an error");
        }
        assert_eq!(s.search("quoted", 10).unwrap().hits.len(), 1);
        assert!(s.search("   ", 10).unwrap().hits.is_empty());
        assert!(s.search("", 10).unwrap().hits.is_empty());
    }

    #[test]
    fn results_are_limited_and_say_so() {
        let (_dir, path) = tmp_db();
        let s = Storage::open(&path, "pi:test").unwrap();
        let items: Vec<_> = (1..=30)
            .map(|n| text_item(n * 1024, true, "common phrase here"))
            .collect();
        s.save_cache(ConversationId(1), meta("t"), &items, false, 1);
        flush(&s);
        let r = s.search("common", 10).unwrap();
        assert_eq!((r.hits.len(), r.truncated), (10, true));
        let r = s.search("common", 50).unwrap();
        assert_eq!((r.hits.len(), r.truncated), (30, false));
    }

    #[test]
    fn copies_and_searches_belong_to_their_backend_profile() {
        let (_dir, path) = tmp_db();
        let a = Storage::open(&path, "pi:a").unwrap();
        let b = Storage::open(&path, "pi:b").unwrap();
        a.save_cache(
            ConversationId(1),
            meta("t"),
            &[text_item(1024, true, "only in a")],
            false,
            1,
        );
        flush(&a);
        assert!(b.load_cache(ConversationId(1)).unwrap().is_none());
        assert!(b.search("only", 10).unwrap().hits.is_empty());
        assert_eq!(b.search("only", 10).unwrap().conversations_searched, 0);
        assert_eq!(a.search("only", 10).unwrap().hits.len(), 1);
    }

    #[test]
    fn only_the_most_recently_synced_conversations_are_kept() {
        let (_dir, path) = tmp_db();
        let s = Storage::open(&path, "pi:test").unwrap();
        for n in 1..=(MAX_CACHED_CONVERSATIONS as u64 + 5) {
            s.save_cache(
                ConversationId(n),
                meta("t"),
                &[text_item(1024, true, &format!("conversation number {n}"))],
                false,
                n as i64,
            );
        }
        flush(&s);
        assert!(
            s.load_cache(ConversationId(1)).unwrap().is_none(),
            "the oldest was dropped"
        );
        assert!(s.load_cache(ConversationId(5)).unwrap().is_none());
        assert!(s.load_cache(ConversationId(6)).unwrap().is_some());
        assert!(
            s.load_cache(ConversationId(MAX_CACHED_CONVERSATIONS as u64 + 5))
                .unwrap()
                .is_some()
        );
        let r = s.search("conversation", 100).unwrap();
        assert_eq!(r.conversations_searched, MAX_CACHED_CONVERSATIONS as usize);
        assert!(
            r.hits.iter().all(|h| h.conversation.0 > 5),
            "dropped copies are not searchable"
        );
    }

    #[test]
    fn a_failed_copy_write_is_reported_once_and_does_not_lose_the_old_copy() {
        let (_dir, path) = tmp_db();
        let s = Storage::open(&path, "pi:test").unwrap();
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = seen.clone();
        s.set_error_sink(Arc::new(move |m| sink.lock().unwrap().push(m)));
        s.save_cache(
            ConversationId(1),
            meta("t"),
            &[text_item(1024, true, "kept")],
            false,
            1,
        );
        flush(&s);
        s.set_background_failure(true);
        s.save_cache(
            ConversationId(1),
            meta("t"),
            &[text_item(1024, true, "lost")],
            false,
            2,
        );
        s.save_cache(
            ConversationId(1),
            meta("t"),
            &[text_item(1024, true, "lost again")],
            false,
            3,
        );
        save_and_wait(&s, 9_999, "flush", 2).unwrap();
        assert_eq!(seen.lock().unwrap().len(), 1, "{:?}", seen.lock().unwrap());
        assert!(seen.lock().unwrap()[0].contains("offline reading"));
        assert_eq!(
            s.load_cache(ConversationId(1)).unwrap().unwrap().synced_at,
            1
        );
    }

    #[test]
    fn a_database_from_before_the_cache_gains_it_on_upgrade_keeping_its_drafts() {
        let (_dir, path) = tmp_db();
        {
            let conn = Connection::open(&path).unwrap();
            for sql in &MIGRATIONS[..6] {
                conn.execute_batch(sql).unwrap();
            }
            conn.execute_batch("PRAGMA user_version = 6").unwrap();
            conn.execute(
                "INSERT INTO drafts (namespace, conversation_id, text, rev, updated_at) VALUES ('pi:test', 1, 'kept draft', 1, 1)",
                [],
            )
            .unwrap();
        }
        let s = Storage::open(&path, "pi:test").unwrap();
        assert_eq!(s.load_all().unwrap().drafts[0].text, "kept draft");
        s.save_cache(
            ConversationId(1),
            meta("t"),
            &[text_item(1024, true, "after upgrade")],
            false,
            1,
        );
        flush(&s);
        assert_eq!(s.search("upgrade", 5).unwrap().hits.len(), 1);
        assert!(
            path.with_extension("sqlite3.bak-v6").exists(),
            "a backup was made before migrating"
        );
    }

    #[test]
    fn conversations_with_a_saved_copy_are_listed_with_what_identifies_them() {
        let (_dir, path) = tmp_db();
        let s = Storage::open(&path, "pi:test").unwrap();
        let other = Storage::open(&path, "pi:other").unwrap();
        s.save_cache(
            ConversationId(1),
            CacheMeta {
                title: "first".into(),
                project_path: "/work/a".into(),
                updated_at: 11,
            },
            &[text_item(1024, true, "hello")],
            false,
            100,
        );
        s.save_cache(
            ConversationId(2),
            CacheMeta {
                title: "second".into(),
                project_path: "/work/b".into(),
                updated_at: 22,
            },
            &[text_item(1024, true, "world")],
            false,
            200,
        );
        other.save_cache(
            ConversationId(3),
            meta("not ours"),
            &[text_item(1024, true, "x")],
            false,
            1,
        );
        flush(&s);
        flush(&other);
        let list = s.cached_conversations().unwrap();
        assert_eq!(
            list.iter().map(|c| c.id.0).collect::<Vec<_>>(),
            [2, 1],
            "newest sync first"
        );
        assert_eq!(
            list[0],
            CachedConversation {
                id: ConversationId(2),
                meta: CacheMeta {
                    title: "second".into(),
                    project_path: "/work/b".into(),
                    updated_at: 22
                },
                synced_at: 200,
            }
        );
        assert_eq!(other.cached_conversations().unwrap().len(), 1);
    }

    // ------------------------------------------------------------- conversation ownership

    fn draft_text(s: &Storage, conversation: u64) -> Option<String> {
        s.load_all()
            .unwrap()
            .drafts
            .into_iter()
            .find(|d| d.conversation.0 == conversation)
            .map(|d| d.text)
    }

    #[test]
    fn a_database_from_before_ownership_gains_it_on_upgrade_and_a_newer_one_is_still_refused() {
        let (_dir, path) = tmp_db();
        {
            let conn = Connection::open(&path).unwrap();
            for sql in &MIGRATIONS[..7] {
                conn.execute_batch(sql).unwrap();
            }
            conn.execute_batch("PRAGMA user_version = 7").unwrap();
            conn.execute(
                "INSERT INTO drafts (namespace, conversation_id, text, rev, updated_at)
                 VALUES ('pi:test', 1, 'kept draft', 1, 1)",
                [],
            )
            .unwrap();
        }
        let s = Storage::open(&path, "pi:test").unwrap();
        assert_eq!(SCHEMA_VERSION, 8);
        assert_eq!(draft_text(&s, 1).as_deref(), Some("kept draft"));
        assert_eq!(s.native_owned().unwrap(), vec![]);
        assert!(path.with_extension("sqlite3.bak-v7").exists());
        s.shutdown();
        drop(s);

        // A build newer than this one is refused, and its file left exactly as it was.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(&format!("PRAGMA user_version = {}", SCHEMA_VERSION + 1))
                .unwrap();
        }
        let before = std::fs::read(&path).unwrap();
        let err = Storage::open(&path, "pi:test").err().expect("must refuse");
        assert!(err.contains("newer"), "{err}");
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn an_import_records_ownership_of_what_it_imported_in_the_copy_transaction() {
        let (_dir, path) = tmp_db();
        let pi = Storage::open(&path, "pi:test").unwrap();
        save_and_wait(&pi, 1, "draft one", 1).unwrap();
        save_and_wait(&pi, 2, "draft two", 1).unwrap();
        pi.shutdown();

        let native = Storage::open(&path, "native:abc").unwrap();
        let (one, two) = (ConversationId(1), ConversationId(2));
        // `two` was imported earlier (and perhaps released since): copied, not re-owned.
        let report = native
            .import_conversations_from("pi:test", &[one], &[two], 1_700)
            .unwrap();
        assert_eq!(report.drafts, 2);
        assert_eq!(
            native.native_owned().unwrap(),
            vec![NativeOwnership {
                conversation: one,
                engine: "native:abc".into(),
                source_namespace: "pi:test".into(),
                imported_at: 1_700,
            }]
        );
        assert!(native.native_owner(one).unwrap().is_some());
        assert_eq!(native.native_owner(two).unwrap(), None);
        // The record is the desktop's, keyed by the conversation alone: the Pi namespace sees it.
        let pi = Storage::open(&path, "pi:test").unwrap();
        assert!(pi.native_owner(one).unwrap().is_some());
        // Nothing to copy, nothing recorded.
        assert_eq!(
            native
                .import_conversations_from("pi:test", &[], &[], 1)
                .unwrap(),
            CopyReport::default()
        );
        assert!(
            native
                .import_conversations_from("native:abc", &[one], &[], 1)
                .is_err(),
            "never onto itself"
        );
        assert_eq!(native.native_owned().unwrap().len(), 1);
    }

    #[test]
    fn a_failed_ownership_record_rolls_the_whole_import_copy_back() {
        let (_dir, path) = tmp_db();
        let pi = Storage::open(&path, "pi:test").unwrap();
        save_and_wait(&pi, 1, "draft one", 1).unwrap();
        pi.shutdown();
        let native = Storage::open(&path, "native:abc").unwrap();
        // Make the ownership insert fail after the data copy has run in the same transaction.
        Connection::open(&path)
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER refuse_owner BEFORE INSERT ON conversation_owners
                 BEGIN SELECT RAISE(ABORT, 'injected ownership failure'); END;",
            )
            .unwrap();
        let err = native
            .import_conversations_from("pi:test", &[ConversationId(1)], &[], 1)
            .unwrap_err();
        assert!(err.contains("injected ownership failure"), "{err}");
        assert_eq!(draft_text(&native, 1), None, "the copy was rolled back too");
        assert_eq!(native.native_owned().unwrap(), vec![]);

        // Without the fault, both land together.
        Connection::open(&path)
            .unwrap()
            .execute_batch("DROP TRIGGER refuse_owner")
            .unwrap();
        native
            .import_conversations_from("pi:test", &[ConversationId(1)], &[], 1)
            .unwrap();
        assert_eq!(draft_text(&native, 1).as_deref(), Some("draft one"));
        assert_eq!(native.native_owned().unwrap().len(), 1);
    }

    #[test]
    fn releasing_removes_only_the_ownership_record() {
        let (_dir, path) = tmp_db();
        let pi = Storage::open(&path, "pi:test").unwrap();
        save_and_wait(&pi, 1, "pi draft", 1).unwrap();
        pi.save_cache(
            ConversationId(1),
            meta("t"),
            &[text_item(1024, true, "searchable zebra")],
            false,
            1,
        );
        flush(&pi);
        let native = Storage::open(&path, "native:abc").unwrap();
        native
            .import_conversations_from("pi:test", &[ConversationId(1)], &[], 1)
            .unwrap();
        save_and_wait(&native, 1, "native-only draft", 9).unwrap();

        assert!(pi.release_ownership(ConversationId(1)).unwrap());
        assert!(!pi.release_ownership(ConversationId(1)).unwrap(), "once");
        assert_eq!(pi.native_owner(ConversationId(1)).unwrap(), None);
        // Both namespaces keep everything they had.
        assert_eq!(draft_text(&pi, 1).as_deref(), Some("pi draft"));
        assert_eq!(draft_text(&native, 1).as_deref(), Some("native-only draft"));
        assert_eq!(pi.search("zebra", 5).unwrap().hits.len(), 1);
        assert_eq!(native.search("zebra", 5).unwrap().hits.len(), 1);
    }
}
