use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pipkin_engine_protocol::{EntryKind, InputMode, Message, Origin, RunOutcome};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

use super::*;
use crate::store::{RequestState, Store, StoreError};

// ---------------------------------------------------------------------------- fixture builder

/// `packages/durable/src/storage/sqlite/migrations.ts`: the migrator's `durable_schema` table
/// followed by `INITIAL_SCHEMA`, verbatim.
const REFERENCE_DDL: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS durable_schema (
			singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
			version INTEGER NOT NULL CHECK (version >= 0)
		) STRICT",
    "INSERT OR IGNORE INTO durable_schema (singleton, version) VALUES (1, 0)",
    "CREATE TABLE durable_metadata (
		singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
		next_id TEXT NOT NULL,
		next_seq INTEGER NOT NULL
	) STRICT",
    "INSERT INTO durable_metadata (singleton, next_id, next_seq) VALUES (1, '2', 1)",
    "CREATE TABLE record_ids (
		id INTEGER PRIMARY KEY,
		record_type TEXT NOT NULL CHECK (record_type IN ('conversation', 'entry', 'task', 'submission', 'document'))
	) STRICT",
    "CREATE TABLE conversations (
		id INTEGER PRIMARY KEY,
		owner_conversation_id INTEGER,
		owner_task_id INTEGER,
		record TEXT NOT NULL CHECK (json_valid(record))
	) STRICT",
    "CREATE INDEX conversations_by_owner_conversation ON conversations (owner_conversation_id, id)",
    "CREATE INDEX conversations_by_owner_task ON conversations (owner_task_id, id)",
    "CREATE TABLE entries (
		id INTEGER PRIMARY KEY,
		conversation_id INTEGER NOT NULL,
		head INTEGER,
		commit_seq INTEGER NOT NULL,
		record TEXT NOT NULL CHECK (json_valid(record))
	) STRICT",
    "CREATE INDEX entries_by_conversation ON entries (conversation_id, id DESC)",
    "CREATE INDEX entry_heads_by_conversation ON entries (conversation_id, id DESC) WHERE head IS NOT NULL",
    "CREATE TABLE tasks (
		id INTEGER PRIMARY KEY,
		conversation_id INTEGER NOT NULL,
		kind TEXT NOT NULL,
		status TEXT NOT NULL CHECK (status IN ('pending', 'running', 'waiting', 'completing', 'terminal')),
		abort_requested INTEGER NOT NULL CHECK (abort_requested IN (0, 1)),
		background INTEGER NOT NULL CHECK (background IN (0, 1)),
		record TEXT NOT NULL CHECK (json_valid(record))
	) STRICT",
    "CREATE INDEX tasks_by_status ON tasks (status, id)",
    "CREATE INDEX tasks_by_conversation ON tasks (conversation_id, id)",
    "CREATE INDEX tasks_by_kind ON tasks (kind, id)",
    "CREATE INDEX tasks_by_abort_requested ON tasks (abort_requested, id)",
    "CREATE INDEX tasks_by_background ON tasks (background, id)",
    "CREATE TABLE submissions (
		id INTEGER PRIMARY KEY,
		conversation_id INTEGER NOT NULL,
		request_id TEXT,
		status TEXT NOT NULL CHECK (status IN ('queued', 'placed', 'done', 'unanswered')),
		record TEXT NOT NULL CHECK (json_valid(record))
	) STRICT",
    "CREATE INDEX submissions_by_request ON submissions (conversation_id, request_id)",
    "CREATE INDEX submissions_by_conversation ON submissions (conversation_id, id)",
    "CREATE INDEX submissions_by_status ON submissions (status, id)",
    "CREATE TABLE documents (
		id INTEGER PRIMARY KEY,
		kind TEXT NOT NULL,
		family INTEGER NOT NULL CHECK (family IN (0, 1)),
		key_value TEXT NOT NULL,
		scope_kind TEXT NOT NULL CHECK (scope_kind IN ('session', 'conversation', 'task')),
		owner_id INTEGER NOT NULL,
		created_at INTEGER NOT NULL,
		retired_at INTEGER,
		record TEXT NOT NULL CHECK (json_valid(record))
	) STRICT",
    "CREATE INDEX documents_by_address
		ON documents (kind, scope_kind, owner_id, family, key_value, created_at DESC, retired_at)",
    "CREATE INDEX documents_by_scope ON documents (scope_kind, owner_id, id)",
    "CREATE INDEX documents_by_scope_kind ON documents (scope_kind, owner_id, kind, id)",
    "CREATE TABLE document_revisions (
		document_id INTEGER NOT NULL,
		seq INTEGER NOT NULL,
		kind TEXT NOT NULL CHECK (kind IN ('base', 'delta')),
		version INTEGER NOT NULL,
		content TEXT NOT NULL CHECK (json_valid(content)),
		PRIMARY KEY (document_id, seq)
	) STRICT",
    "CREATE INDEX document_revisions_by_kind ON document_revisions (document_id, kind, seq DESC)",
    "UPDATE durable_schema SET version = 1 WHERE singleton = 1",
];

/// Encoded like `storage.ts:64` (indexed strings are JSON-encoded).
fn indexed(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

struct Agent {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Agent {
    fn new() -> Agent {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("agent");
        std::fs::create_dir_all(sessions_dir(&root)).unwrap();
        Agent { _tmp: tmp, root }
    }

    fn dir(&self, id: &str) -> PathBuf {
        sessions_dir(&self.root).join(id)
    }

    fn backups(&self) -> PathBuf {
        self.root.parent().unwrap().join("backups")
    }

    /// A session directory with `meta.json` (tab-indented, as `session-catalog.ts:73-76`)
    /// and a reference-schema database in WAL mode.
    fn session(&self, id: &str) -> Legacy {
        let dir = self.dir(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("meta.json"),
            "{\n\t\"createdAt\": 1700000000000,\n\t\"cwd\": \"/work/project\"\n}\n",
        )
        .unwrap();
        let conn = Connection::open(dir.join("session.sqlite")).unwrap();
        conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get::<_, String>(0))
            .unwrap();
        conn.execute_batch("BEGIN").unwrap();
        for sql in REFERENCE_DDL {
            conn.execute(sql, []).unwrap();
        }
        conn.execute_batch("COMMIT").unwrap();
        Legacy { conn, seq: 1 }
    }
}

struct Legacy {
    conn: Connection,
    seq: i64,
}

impl Legacy {
    fn next_seq(&mut self) -> i64 {
        self.seq += 1;
        self.seq
    }

    fn claim(&self, id: i64, kind: &str) {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO record_ids (id, record_type) VALUES (?1, ?2)",
                params![id, kind],
            )
            .unwrap();
    }

    fn conversation(&mut self, record: Value) {
        let id = record["id"].as_i64().unwrap();
        self.claim(id, "conversation");
        self.conn
            .execute(
                "INSERT INTO conversations (id, owner_conversation_id, owner_task_id, record) VALUES (?1, ?2, ?3, ?4)",
                params![
                    id,
                    record["owner"]["conversationId"].as_i64(),
                    record["owner"]["taskId"].as_i64(),
                    record.to_string()
                ],
            )
            .unwrap();
    }

    fn entry(&mut self, record: Value) {
        let id = record["id"].as_i64().unwrap();
        self.claim(id, "entry");
        let seq = self.next_seq();
        self.conn
            .execute(
                "INSERT INTO entries (id, conversation_id, head, commit_seq, record) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    id,
                    record["conversationId"].as_i64().unwrap(),
                    record["head"].as_i64(),
                    seq,
                    record.to_string()
                ],
            )
            .unwrap();
    }

    fn task(&mut self, record: Value) {
        let id = record["id"].as_i64().unwrap();
        self.claim(id, "task");
        self.conn
            .execute(
                "INSERT INTO tasks (id, conversation_id, kind, status, abort_requested, background, record)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    id,
                    record["conversationId"].as_i64().unwrap(),
                    indexed(record["kind"].as_str().unwrap()),
                    record["state"]["status"].as_str().unwrap(),
                    record["abortRequested"].as_bool().unwrap_or(false) as i64,
                    record["background"].as_bool().unwrap_or(false) as i64,
                    record.to_string()
                ],
            )
            .unwrap();
    }

    fn submission(&mut self, record: Value) {
        let id = record["id"].as_i64().unwrap();
        self.claim(id, "submission");
        self.conn
            .execute(
                "INSERT INTO submissions (id, conversation_id, request_id, status, record) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    id,
                    record["conversationId"].as_i64().unwrap(),
                    record["requestId"].as_str().map(indexed),
                    record["status"].as_str().unwrap(),
                    record.to_string()
                ],
            )
            .unwrap();
    }

    /// A conversation-scoped singleton document: one base, then delta batches (stored ops).
    fn doc(&mut self, id: i64, kind: &str, conversation: i64, base: Value, deltas: &[Value]) {
        self.claim(id, "document");
        let created = self.next_seq();
        let record = json!({
            "id": id, "kind": kind, "createdAt": created,
            "scope": {"kind": "conversation", "conversationId": conversation},
            "history": if kind == "pi.agent" { "rewindable" } else { "latest" },
            "fork": if kind == "pi.agent" { "asOf" } else { "initial" },
        });
        self.conn
            .execute(
                "INSERT INTO documents (id, kind, family, key_value, scope_kind, owner_id, created_at, retired_at, record)
                 VALUES (?1, ?2, 0, ?3, 'conversation', ?4, ?5, NULL, ?6)",
                params![id, indexed(kind), indexed(""), conversation, created, record.to_string()],
            )
            .unwrap();
        self.conn
            .execute(
                "INSERT INTO document_revisions (document_id, seq, kind, version, content) VALUES (?1, ?2, 'base', 1, ?3)",
                params![id, created, base.to_string()],
            )
            .unwrap();
        for ops in deltas {
            let seq = self.next_seq();
            self.conn
                .execute(
                    "INSERT INTO document_revisions (document_id, seq, kind, version, content) VALUES (?1, ?2, 'delta', 1, ?3)",
                    params![id, seq, ops.to_string()],
                )
                .unwrap();
        }
    }
}

// ---------------------------------------------------------------------------- records

fn usage(input: u64, output: u64, cost: f64) -> Value {
    json!({"input": input, "output": output, "cacheRead": 0, "cacheWrite": 0,
           "totalTokens": input + output,
           "cost": {"input": cost, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": cost}})
}

fn user(id: i64, conv: i64, content: Value, ts: i64) -> Value {
    json!({"id": id, "conversationId": conv, "kind": "pi.user",
           "model": [{"role": "user", "content": content, "timestamp": ts}]})
}

fn assistant(id: i64, conv: i64, content: Value, stop: &str, ts: i64, task: i64) -> Value {
    json!({"id": id, "conversationId": conv, "kind": "pi.assistant", "byTaskId": task,
           "model": [{"role": "assistant", "content": content, "api": "anthropic-messages",
                      "provider": "anthropic", "model": "claude-test", "usage": usage(10, 5, 0.25),
                      "stopReason": stop, "timestamp": ts, "responseId": format!("resp_{id}")}]})
}

fn tool_result(
    id: i64,
    conv: i64,
    call: &str,
    name: &str,
    text: &str,
    ts: i64,
    task: i64,
) -> Value {
    json!({"id": id, "conversationId": conv, "kind": "pi.tool-result", "byTaskId": task,
           "data": {"diagnostics": []},
           "model": [{"role": "toolResult", "toolCallId": call, "toolName": name,
                      "content": [{"type": "text", "text": text}], "isError": false,
                      "details": {"exitCode": 0}, "timestamp": ts}]})
}

fn system(id: i64, conv: i64, edits: Value) -> Value {
    let mut v = json!({"id": id, "conversationId": conv, "kind": "pi.system",
           "model": [{"role": "system", "content": "", "sections": {"preamble": "You are Pi."},
                      "timestamp": 1}]});
    if edits.as_array().is_some_and(|a| !a.is_empty()) {
        v["edits"] = edits;
    }
    v
}

fn terminal_task(
    id: i64,
    conv: i64,
    kind: &str,
    input: Value,
    outcome: Value,
    owner: Option<i64>,
) -> Value {
    let mut v = json!({"id": id, "conversationId": conv, "kind": kind, "version": 1,
           "input": input, "background": false, "abortRequested": false,
           "state": {"status": "terminal", "outcome": outcome}});
    if let Some(o) = owner {
        v["owner"] = json!(o);
    }
    v
}

fn input_submission(id: i64, conv: i64, key: Option<&str>, status: &str, extra: Value) -> Value {
    let mut v = json!({"id": id, "conversationId": conv, "type": "input", "status": status});
    if let Some(k) = key {
        v["requestId"] = json!(k);
    }
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    v
}

fn idle_docs(l: &mut Legacy, base_id: i64, conv: i64) {
    l.doc(base_id, "pi.inbox", conv, json!({"items": []}), &[]);
    l.doc(base_id + 1, "pi.live", conv, json!({}), &[]);
}

const ROOT: &str = "sess-1";

/// A settled prompt, a tool round, a stopped run (cancelled input), a compaction, system
/// entries, an unknown kind, a subagent child, usage with deltas, agent/provider docs.
fn rich_session(agent: &Agent) -> Legacy {
    let mut l = agent.session(ROOT);
    l.conversation(json!({"id": 1}));
    // --- settled prompt (user content as a plain string, as agent-controller writes it)
    l.entry(system(2, 1, json!([])));
    l.entry(user(3, 1, json!("hello"), 1_700_000_001_000));
    l.task(terminal_task(
        4,
        1,
        "pi.generation",
        json!({}),
        json!({"status": "completed", "result": null}),
        None,
    ));
    l.entry(assistant(
        5,
        1,
        json!([{"type": "text", "text": "hi there"}]),
        "stop",
        1_700_000_002_000,
        4,
    ));
    l.submission(input_submission(
        6,
        1,
        Some("req-1"),
        "done",
        json!({"entry": 3, "answer": 5}),
    ));
    // --- tool round
    l.entry(user(
        7,
        1,
        json!([{"type": "text", "text": "list files"},
        {"type": "image", "data": "AAAA", "mimeType": "image/png"}]),
        1_700_000_003_000,
    ));
    l.task(terminal_task(
        8,
        1,
        "pi.generation",
        json!({}),
        json!({"status": "completed", "result": null}),
        None,
    ));
    l.entry(assistant(
        9,
        1,
        json!([{"type": "thinking", "thinking": "hm", "thinkingSignature": "sig"},
        {"type": "toolCall", "id": "call-1", "name": "bash", "arguments": {"command": "ls"}}]),
        "toolUse",
        1_700_000_004_000,
        8,
    ));
    l.task(terminal_task(
        10,
        1,
        "pi.tool",
        json!({"assistant": 9, "callId": "call-1"}),
        json!({"status": "completed", "result": null}),
        Some(8),
    ));
    l.entry(tool_result(
        11,
        1,
        "call-1",
        "bash",
        "a.txt\nb.txt",
        1_700_000_005_000,
        10,
    ));
    l.entry(assistant(
        12,
        1,
        json!([{"type": "text", "text": "two files"}]),
        "stop",
        1_700_000_006_000,
        8,
    ));
    l.submission(input_submission(
        13,
        1,
        Some("req-2"),
        "done",
        json!({"entry": 7, "answer": 12}),
    ));
    // --- stopped run: the instruction is retired, completed tool effects stay
    l.entry(user(14, 1, json!("delete everything"), 1_700_000_007_000));
    l.task(terminal_task(
        15,
        1,
        "pi.generation",
        json!({}),
        json!({"status": "aborted"}),
        None,
    ));
    l.entry(assistant(
        16,
        1,
        json!([{"type": "text", "text": "Deleting"}]),
        "aborted",
        1_700_000_008_000,
        15,
    ));
    l.submission(input_submission(
        17,
        1,
        Some("req-3"),
        "unanswered",
        json!({"entry": 14, "reason": "aborted"}),
    ));
    l.entry(
        json!({"id": 18, "conversationId": 1, "kind": "pi.cancelled-input", "byTaskId": 15,
                   "edits": [{"target": 14, "action": "omit"}]}),
    );
    // --- a withdrawn queued input (never had an entry) and an unkeyed input
    l.submission(input_submission(
        19,
        1,
        Some("req-4"),
        "unanswered",
        json!({"reason": "aborted"}),
    ));
    l.submission(input_submission(
        20,
        1,
        None,
        "unanswered",
        json!({"reason": "aborted"}),
    ));
    // --- a model error
    l.entry(user(21, 1, json!("again"), 1_700_000_009_000));
    l.entry(json!({"id": 22, "conversationId": 1, "kind": "pi.assistant", "byTaskId": 23,
        "model": [{"role": "assistant", "content": [], "api": "anthropic-messages", "provider": "anthropic",
                   "model": "claude-test", "usage": usage(0, 0, 0.0), "stopReason": "error",
                   "errorMessage": "529 overloaded", "timestamp": 1_700_000_010_000_i64}]}));
    l.task(terminal_task(
        23,
        1,
        "pi.generation",
        json!({}),
        json!({"status": "failed", "error": {"message": "529 overloaded"}}),
        None,
    ));
    l.submission(input_submission(
        24,
        1,
        Some("req-5"),
        "unanswered",
        json!({"entry": 21, "reason": "model_error", "detail": "529 overloaded"}),
    ));
    // --- subagent round: tool task 27 owns child conversation 28
    l.entry(user(25, 1, json!("ask a helper"), 1_700_000_011_000));
    l.task(terminal_task(
        26,
        1,
        "pi.generation",
        json!({}),
        json!({"status": "completed", "result": null}),
        None,
    ));
    l.entry(assistant(
        27,
        1,
        json!([{"type": "toolCall", "id": "call-sub", "name": "subagent",
        "arguments": {"task": "count the files"}}]),
        "toolUse",
        1_700_000_012_000,
        26,
    ));
    l.task(terminal_task(
        28,
        1,
        "pi.tool",
        json!({"assistant": 27, "callId": "call-sub"}),
        json!({"status": "completed", "result": null}),
        Some(26),
    ));
    l.conversation(json!({"id": 29, "owner": {"conversationId": 1, "taskId": 28}}));
    l.entry(user(30, 29, json!("count the files"), 1_700_000_013_000));
    l.task(terminal_task(
        31,
        29,
        "pi.generation",
        json!({}),
        json!({"status": "completed", "result": null}),
        None,
    ));
    l.entry(assistant(
        32,
        29,
        json!([{"type": "text", "text": "There are "}, {"type": "text", "text": "2."}]),
        "stop",
        1_700_000_014_000,
        31,
    ));
    l.submission(input_submission(
        33,
        29,
        Some("subagent:28"),
        "done",
        json!({"entry": 30, "answer": 32}),
    ));
    l.entry(tool_result(
        34,
        1,
        "call-sub",
        "subagent",
        "There are 2.",
        1_700_000_015_000,
        28,
    ));
    l.entry(assistant(
        35,
        1,
        json!([{"type": "text", "text": "Helper says 2"}]),
        "stop",
        1_700_000_016_000,
        26,
    ));
    l.submission(input_submission(
        36,
        1,
        Some("req-6"),
        "done",
        json!({"entry": 25, "answer": 35}),
    ));
    // --- unknown app kind without model content
    l.entry(json!({"id": 37, "conversationId": 1, "kind": "app.bookmark", "data": {"label": "here", "n": 1.5}}));
    // --- compaction keeping entries from 25; a later system baseline omits the older one
    l.task(terminal_task(
        38,
        1,
        "pi.compaction",
        json!({"reason": "threshold"}),
        json!({"status": "completed", "result": {"entryId": 39}}),
        None,
    ));
    l.entry(
        json!({"id": 39, "conversationId": 1, "kind": "pi.compaction", "head": 25, "byTaskId": 38,
        "data": {"reason": "threshold"},
        "model": [{"role": "user", "content": [{"type": "text", "text":
            format!("{}Goal: test{}", context::SUMMARY_PREFIX, context::SUMMARY_SUFFIX)}],
            "timestamp": 1_700_000_017_000_i64}]}),
    );
    l.entry(system(40, 1, json!([{"target": 2, "action": "omit"}])));
    l.entry(user(41, 1, json!("after compaction"), 1_700_000_018_000));
    // --- documents
    l.doc(
        50,
        "pi.agent",
        1,
        json!({"model": {"provider": "anthropic", "modelId": "claude-test"},
        "thinkingLevel": "medium", "extensions": {"remove": []}}),
        &[],
    );
    l.doc(
        51,
        "pi.provider",
        1,
        json!({"sessionId": "0190-provider-session"}),
        &[],
    );
    l.doc(
        52,
        "pi.usage",
        1,
        json!({"models": {}, "tools": {}}),
        &[
            json!([["s", ["models", "anthropic/claude-test"], usage(10, 5, 0.25)]]),
            json!([
                ["s", ["models", "anthropic/claude-test", "input"], 30],
                ["s", ["tools", "subagent"], usage(7, 3, 0.1)]
            ]),
        ],
    );
    idle_docs(&mut l, 53, 1);
    l.doc(
        55,
        "pi.agent",
        29,
        json!({"model": {"provider": "anthropic", "modelId": "claude-test"},
        "extensions": {"remove": ["subagent"]}}),
        &[],
    );
    l.doc(
        56,
        "pi.usage",
        29,
        json!({"models": {"anthropic/claude-test": usage(4, 4, 0.01)}, "tools": {}}),
        &[],
    );
    idle_docs(&mut l, 57, 29);
    l
}

/// Every file of a directory with its bytes.
fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let e = e.unwrap();
        if e.path().is_file() {
            out.insert(
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).unwrap(),
            );
        }
    }
    out
}

/// Every row of every native table, for "nothing was written" checks.
fn dump(store: &Store) -> Vec<String> {
    store
        .read(|c| {
            let mut out = Vec::new();
            for table in [
                "sessions",
                "entries",
                "requests",
                "usage",
                "subagent_tasks",
                "kv",
                "import_map",
            ] {
                let mut stmt = c.prepare(&format!("SELECT * FROM {table}"))?;
                let n = stmt.column_count();
                let rows = stmt
                    .query_map([], |r| {
                        let mut row = format!("{table}:");
                        for i in 0..n {
                            row.push_str(&format!("{:?}|", r.get_ref(i)?));
                        }
                        Ok(row)
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                out.extend(rows);
            }
            out.sort();
            Ok(out)
        })
        .unwrap()
}

/// The reference context of a conversation, read straight from a legacy database.
fn reference_context(db: &Path, conversation: i64) -> Vec<Value> {
    let conn = legacy::open_source(db).unwrap();
    let source = legacy::read(&conn).unwrap();
    let entries: Vec<&legacy::LegacyEntry> = source
        .entries
        .iter()
        .filter(|e| e.conversation == conversation)
        .collect();
    context::reference_messages(&entries).unwrap()
}

fn native_context(store: &Store, session: &str) -> Vec<Value> {
    context::native_messages(&store.context_entries(session).unwrap())
}

fn texts(messages: &[Value]) -> Vec<String> {
    messages
        .iter()
        .map(|m| {
            let role = m["role"].as_str().unwrap().to_owned();
            let text: String = m["content"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|b| b["text"].as_str().or(b["name"].as_str()))
                .collect();
            format!("{role}:{text}")
        })
        .collect()
}

fn import(agent: &Agent, store: &Store, id: &str) -> Result<ImportOutcome, ImportError> {
    import_session(
        store,
        &agent.root,
        id,
        &agent.root.join("server"),
        &agent.backups(),
    )
}

// ---------------------------------------------------------------------------- tests

#[test]
fn imports_a_rich_session_losslessly_with_the_same_effective_context() {
    let agent = Agent::new();
    drop(rich_session(&agent));
    let source_dir = agent.dir(ROOT);
    let before = snapshot(&source_dir);
    let store = Store::memory().unwrap();

    let report = discover(&agent.root, &agent.root.join("server"), &store);
    assert_eq!(report.sessions.len(), 1);
    let found = &report.sessions[0];
    assert_eq!(found.status, SessionStatus::Importable, "{found:?}");
    assert_eq!(found.cwd.as_deref(), Some("/work/project"));
    assert_eq!(found.created_at_ms, Some(1_700_000_000_000));
    assert_eq!(found.schema_version, Some(1));
    assert!(found.sqlite_bytes > 0);

    let outcome = import(&agent, &store, ROOT).unwrap();
    assert!(!outcome.already_imported);
    assert_eq!(outcome.native_session, ROOT);
    let child = child_session_id(ROOT, 29);
    assert_eq!(outcome.children, vec![child.clone()]);

    // The source is untouched, byte for byte.
    assert_eq!(snapshot(&source_dir), before);

    // Session rows.
    let root = store.session(ROOT).unwrap().unwrap();
    assert_eq!(root.cwd, "/work/project");
    assert_eq!(
        root.origin,
        Origin::Imported {
            engine: "pi".into(),
            source: ROOT.into()
        }
    );
    assert_eq!(root.model.as_deref(), Some("anthropic/claude-test"));
    assert_eq!(root.thinking.as_deref(), Some("medium"));
    assert!(root.subagents_enabled);
    assert_eq!(root.created_at_ms, 1_700_000_000_000);
    assert_eq!(root.updated_at_ms, 1_700_000_018_000);
    assert!(root.parent.is_none());
    let child_row = store.session(&child).unwrap().unwrap();
    assert_eq!(child_row.parent.as_deref(), Some(ROOT));
    assert_eq!(child_row.root.as_deref(), Some(ROOT));
    assert!(!child_row.subagents_enabled);
    assert_eq!(store.root_sessions().unwrap().len(), 1);

    // Entries keep their legacy ids; native ids continue above them.
    let (all, _) = store.page(ROOT, None, 1000).unwrap();
    let ids: Vec<u64> = all.iter().map(|e| e.id).collect();
    assert_eq!(
        ids,
        vec![
            2, 3, 5, 7, 9, 11, 12, 14, 16, 18, 21, 22, 25, 27, 34, 35, 37, 39, 40, 41
        ]
    );
    assert_eq!(
        store.kv_get(ROOT, "entry_floor").unwrap().as_deref(),
        Some("42")
    );
    let next = store
        .tx(|tx| {
            Store::append_entry(
                tx,
                ROOT,
                1,
                None,
                &EntryKind::Notice {
                    level: pipkin_engine_protocol::NoticeLevel::Info,
                    text: "native".into(),
                },
            )
        })
        .unwrap();
    assert_eq!(next.id, 42);
    let (child_entries, _) = store.page(&child, None, 100).unwrap();
    assert_eq!(
        child_entries.iter().map(|e| e.id).collect::<Vec<_>>(),
        vec![30, 32]
    );

    // Entry mapping.
    let by_id = |id: u64| all.iter().find(|e| e.id == id).unwrap().clone();
    match by_id(3).kind {
        EntryKind::Message {
            message: Message::User { content, .. },
            request,
            mode,
            ..
        } => {
            assert_eq!(content, vec![pipkin_engine_protocol::Block::text("hello")]);
            assert_eq!(request.as_deref(), Some("req-1"));
            assert_eq!(mode, InputMode::Prompt);
        }
        other => panic!("{other:?}"),
    }
    assert!(by_id(14).retired, "the stopped instruction is retired");
    assert!(!by_id(16).retired);
    assert!(by_id(2).retired, "the omitted system baseline is retired");
    assert!(matches!(
        by_id(18).kind,
        EntryKind::Custom { ref custom_type, .. } if custom_type == "pi.cancelled-input"
    ));
    match by_id(39).kind {
        EntryKind::Compaction {
            summary,
            first_kept,
            tokens_before,
        } => {
            assert_eq!(summary, "Goal: test");
            assert_eq!(first_kept, 25);
            assert_eq!(tokens_before, 0);
        }
        other => panic!("{other:?}"),
    }
    match by_id(37).kind {
        EntryKind::Custom { custom_type, data } => {
            assert_eq!(custom_type, "app.bookmark");
            assert_eq!(data["data"]["label"], "here");
        }
        other => panic!("{other:?}"),
    }
    match by_id(40).kind {
        EntryKind::Custom { custom_type, data } => {
            assert_eq!(custom_type, "pi.system");
            assert_eq!(data["edits"][0]["target"], 2);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(by_id(39).at_ms, 1_700_000_017_000);

    // Lossless side records: normalized/raw entries byte for byte, envelopes otherwise.
    let source_db = source_dir.join("session.sqlite");
    let raw_of = |id: i64| -> String {
        Connection::open(&source_db)
            .unwrap()
            .query_row("SELECT record FROM entries WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap()
    };
    for id in [3, 39, 37, 14] {
        assert_eq!(
            store.kv_get(ROOT, &format!("pi.entry.{id}")).unwrap(),
            Some(raw_of(id)),
            "entry {id}"
        );
    }
    let envelope: Value =
        serde_json::from_str(&store.kv_get(ROOT, "pi.entry.11").unwrap().unwrap()).unwrap();
    assert_eq!(envelope["byTaskId"], 10);
    assert_eq!(envelope["data"], json!({"diagnostics": []}));
    assert!(envelope.get("model").is_none());
    let normalized = outcome.manifest["normalizedEntries"].as_array().unwrap();
    assert!(normalized.contains(&json!(3)) && normalized.contains(&json!(14)));
    assert!(
        !normalized.contains(&json!(5)),
        "assistants round-trip exactly"
    );

    // Effective context: identical to the reference, head marker first, kept range, the
    // stopped instruction omitted, the aborted/errored assistants excluded.
    let reference = reference_context(&source_db, 1);
    let native = native_context(&store, ROOT);
    assert_eq!(native, reference);
    assert_eq!(
        texts(&native),
        vec![
            format!(
                "user:{}Goal: test{}",
                context::SUMMARY_PREFIX,
                context::SUMMARY_SUFFIX
            ),
            "user:ask a helper".into(),
            "assistant:subagent".into(),
            "toolResult:There are 2.".into(),
            "assistant:Helper says 2".into(),
            "user:after compaction".into(),
        ]
    );
    assert_eq!(
        native_context(&store, &child),
        reference_context(&source_db, 29)
    );

    // Requests.
    let req = |k: &str| store.request(ROOT, k).unwrap().unwrap();
    assert_eq!(req("req-1").outcome, Some(RunOutcome::Completed));
    assert_eq!(req("req-1").state, RequestState::Settled);
    assert_eq!(req("req-1").text, "hello");
    assert_eq!(
        req("req-1").fingerprint,
        crate::store::fingerprint(InputMode::Prompt, "hello", &[])
    );
    assert_eq!(req("req-2").images.len(), 1);
    assert_eq!(req("req-2").text, "list files");
    assert_eq!(req("req-3").outcome, Some(RunOutcome::Aborted));
    assert_eq!(req("req-4").state, RequestState::Withdrawn);
    assert_eq!(
        req("req-5").outcome,
        Some(RunOutcome::ModelError {
            detail: "529 overloaded".into()
        })
    );
    assert!(store.request(&child, "subagent:28").unwrap().is_some());
    assert_eq!(outcome.manifest["counts"]["inputSubmissionsWithoutKey"], 1);
    assert!(store.queue(ROOT).unwrap().is_empty());
    assert!(store.open_run(ROOT).unwrap().is_none());

    // Usage from the base + deltas.
    let usage = store.usage(ROOT).unwrap();
    assert_eq!(usage.models.len(), 1);
    assert_eq!(usage.models[0].0, "anthropic/claude-test");
    assert_eq!(usage.models[0].1.input, 30);
    assert_eq!(usage.models[0].1.cost.total, 0.25);
    assert_eq!(usage.tools[0].0, "subagent");
    assert_eq!(usage.tools[0].1.output, 3);
    assert_eq!(store.usage(&child).unwrap().models[0].1.input, 4);

    // Provider prompt-cache identity and preserved records.
    assert_eq!(
        store
            .kv_get(ROOT, "provider_session_id")
            .unwrap()
            .as_deref(),
        Some("0190-provider-session")
    );
    let tasks: Value =
        serde_json::from_str(&store.kv_get(ROOT, "pi.tasks").unwrap().unwrap()).unwrap();
    assert!(tasks.as_array().unwrap().iter().all(|t| t["id"] != 28));
    assert!(store.kv_get(&child, "pi.owner-task").unwrap().is_some());
    assert!(store.kv_get(ROOT, "pi.doc.pi.live").unwrap().is_some());

    // Subagent task row.
    let (rows, truncated) = store.tasks(ROOT, 10).unwrap();
    assert!(!truncated);
    assert_eq!(rows.len(), 1);
    let t = &rows[0];
    assert_eq!(t.child, 1);
    assert_eq!(t.parent_session, ROOT);
    assert_eq!(t.parent_call, "call-sub");
    assert_eq!(t.child_session, child);
    assert_eq!(t.task, "count the files");
    assert_eq!(t.status, "completed");
    assert_eq!(t.result.as_deref(), Some("There are 2."));
    assert!(t.result_delivered);

    // Manifest.
    assert_eq!(outcome.manifest["schemaVersion"], 1);
    assert_eq!(outcome.manifest["counts"]["entries"]["pi.user"], 7);
    assert_eq!(outcome.manifest["counts"]["subagentTasks"], 1);
    assert_eq!(outcome.manifest["conversations"]["29"], json!(child));
    assert_eq!(
        outcome.manifest["hashes"]["sourceSqliteSha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    // The backup is a self-contained consistent copy that passes an integrity check.
    let backup = agent.backups().join(ROOT).join("session.sqlite");
    let copy = Connection::open(&backup).unwrap();
    let n: i64 = copy
        .query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 22);
    assert!(agent.backups().join(ROOT).join("meta.json").is_file());
    store.integrity_check().unwrap();

    // Discovery now reports it as imported.
    let report = discover(&agent.root, &agent.root.join("server"), &store);
    assert_eq!(
        report.sessions[0].status,
        SessionStatus::AlreadyImported {
            native: ROOT.into()
        }
    );
}

#[test]
fn a_rerun_returns_the_mapping_and_writes_nothing() {
    let agent = Agent::new();
    drop(rich_session(&agent));
    let store = Store::memory().unwrap();
    let first = import(&agent, &store, ROOT).unwrap();
    let rows = dump(&store);
    std::fs::remove_dir_all(agent.backups()).unwrap();
    let again = import(&agent, &store, ROOT).unwrap();
    assert!(again.already_imported);
    assert_eq!(again.native_session, first.native_session);
    assert_eq!(again.children, first.children);
    assert_eq!(again.manifest, first.manifest);
    assert_eq!(dump(&store), rows);
    assert!(!agent.backups().exists(), "a rerun takes no backup");
}

#[test]
fn an_injected_failure_rolls_back_every_native_record() {
    let agent = Agent::new();
    drop(rich_session(&agent));
    let store = Store::memory().unwrap();
    let empty = dump(&store);
    store.fail_next_write();
    let err = import(&agent, &store, ROOT).unwrap_err();
    assert!(
        matches!(err, ImportError::Store(StoreError::Injected)),
        "{err}"
    );
    assert_eq!(dump(&store), empty);
    assert!(store.imported(ENGINE, ROOT).unwrap().is_none());
    // The next attempt succeeds.
    assert!(!import(&agent, &store, ROOT).unwrap().already_imported);
}

#[test]
fn a_reset_with_handoff_keeps_the_new_context_only() {
    let agent = Agent::new();
    let mut l = agent.session("reset-1");
    l.conversation(json!({"id": 1}));
    l.entry(user(2, 1, json!("old work"), 10));
    l.entry(assistant(
        3,
        1,
        json!([{"type": "text", "text": "done"}]),
        "stop",
        11,
        9,
    ));
    l.entry(
        json!({"id": 4, "conversationId": 1, "kind": "pi.reset", "head": 4,
        "model": [{"role": "user", "content": "Continue from the plan.", "timestamp": 12}]}),
    );
    l.entry(user(5, 1, json!("next"), 13));
    l.entry(json!({"id": 6, "conversationId": 1, "kind": "pi.reset", "head": 6}));
    l.entry(user(7, 1, json!("fresh"), 14));
    idle_docs(&mut l, 20, 1);
    drop(l);
    let store = Store::memory().unwrap();
    import(&agent, &store, "reset-1").unwrap();
    let db = agent.dir("reset-1").join("session.sqlite");
    let native = native_context(&store, "reset-1");
    assert_eq!(native, reference_context(&db, 1));
    assert_eq!(texts(&native), vec!["user:fresh".to_owned()]);
    match store.entry("reset-1", 4).unwrap().unwrap().kind {
        EntryKind::Reset { handoff: Some(m) } => assert_eq!(m.text(), "Continue from the plan."),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        store.entry("reset-1", 6).unwrap().unwrap().kind,
        EntryKind::Reset { handoff: None }
    ));
    // No pi.agent document: subagents stay enabled (the reference default selection).
    assert!(store.session("reset-1").unwrap().unwrap().subagents_enabled);
}

fn blocked(agent: &Agent, id: &str) -> Vec<String> {
    let store = Store::memory().unwrap();
    let report = discover(&agent.root, &agent.root.join("server"), &store);
    let found = report.sessions.iter().find(|s| s.id == id).unwrap();
    let SessionStatus::Blocked { reasons } = &found.status else {
        panic!("{found:?}")
    };
    match import(agent, &store, id) {
        Err(ImportError::Blocked(r)) => assert_eq!(&r, reasons),
        other => panic!("{other:?}"),
    }
    assert!(dump(&store).is_empty());
    reasons.clone()
}

#[test]
fn a_replace_edit_blocks_the_session() {
    let agent = Agent::new();
    let mut l = agent.session("replace-1");
    l.conversation(json!({"id": 1}));
    l.entry(user(2, 1, json!("x"), 10));
    l.entry(json!({"id": 3, "conversationId": 1, "kind": "app.rewrite",
        "edits": [{"target": 2, "action": "replace",
                   "messages": [{"role": "user", "content": "y", "timestamp": 10}]}]}));
    idle_docs(&mut l, 20, 1);
    drop(l);
    let reasons = blocked(&agent, "replace-1");
    assert!(
        reasons.iter().any(|r| r.contains("action replace")),
        "{reasons:?}"
    );
}

#[test]
fn an_unknown_kind_with_model_messages_or_a_fork_blocks_the_session() {
    let agent = Agent::new();
    let mut l = agent.session("odd-1");
    l.conversation(json!({"id": 1}));
    l.entry(json!({"id": 2, "conversationId": 1, "kind": "app.inject",
        "model": [{"role": "user", "content": "secret instructions", "timestamp": 1}]}));
    l.conversation(json!({"id": 3, "parent": {"conversationId": 1, "at": 2}}));
    idle_docs(&mut l, 20, 1);
    drop(l);
    let reasons = blocked(&agent, "odd-1");
    assert!(
        reasons.iter().any(|r| r.contains("app.inject")),
        "{reasons:?}"
    );
    assert!(reasons.iter().any(|r| r.contains("fork")), "{reasons:?}");
}

#[test]
fn in_flight_work_blocks_the_session() {
    let agent = Agent::new();
    let mut l = agent.session("live-1");
    l.conversation(json!({"id": 1}));
    l.entry(user(2, 1, json!("running"), 10));
    l.submission(input_submission(
        3,
        1,
        Some("r"),
        "placed",
        json!({"entry": 2}),
    ));
    l.submission(input_submission(4, 1, Some("q"), "queued", json!({})));
    l.task(
        json!({"id": 5, "conversationId": 1, "kind": "pi.generation", "version": 1, "input": {},
        "background": false, "abortRequested": false,
        "state": {"status": "running", "checkpoint": {"phase": "request"}}}),
    );
    l.doc(
        20,
        "pi.live",
        1,
        json!({}),
        &[json!([["s", ["run"], {"taskId": 5, "inputs": [3]}]])],
    );
    l.doc(
        21,
        "pi.inbox",
        1,
        json!({"items": [{"id": 4, "mode": "steer", "content": "hi"}]}),
        &[],
    );
    drop(l);
    let reasons = blocked(&agent, "live-1");
    assert!(reasons[0].starts_with(IN_FLIGHT), "{reasons:?}");
    for part in [
        "live run",
        "inbox",
        "task 5",
        "submission 3 is placed",
        "submission 4 is queued",
    ] {
        assert!(reasons[0].contains(part), "{part}: {reasons:?}");
    }
}

fn set_mtime(path: &Path, age: std::time::Duration) {
    let f = std::fs::File::open(path).unwrap();
    f.set_modified(std::time::SystemTime::now() - age).unwrap();
}

#[test]
fn a_fresh_lock_reports_locked_and_a_stale_one_does_not() {
    let agent = Agent::new();
    drop(rich_session(&agent));
    let lock = lock_path(&agent.dir(ROOT));
    std::fs::create_dir(&lock).unwrap();
    let store = Store::memory().unwrap();
    let report = discover(&agent.root, &agent.root.join("server"), &store);
    assert_eq!(
        report.sessions.len(),
        1,
        "the lock directory is not a session"
    );
    assert_eq!(report.sessions[0].status, SessionStatus::Locked);
    assert!(matches!(
        import(&agent, &store, ROOT),
        Err(ImportError::Locked(_))
    ));
    assert!(dump(&store).is_empty());
    assert!(
        !agent.backups().exists(),
        "nothing is copied from a locked session"
    );

    set_mtime(&lock, LOCK_STALE + std::time::Duration::from_secs(5));
    assert_eq!(
        discover(&agent.root, &agent.root.join("server"), &store).sessions[0].status,
        SessionStatus::Importable
    );
    import(&agent, &store, ROOT).unwrap();
}

#[test]
fn committed_wal_content_is_seen_and_the_source_is_not_written() {
    let agent = Agent::new();
    let mut l = rich_session(&agent);
    // A writer that never checkpointed: the newest entry lives only in the WAL.
    l.conn
        .query_row("PRAGMA wal_autocheckpoint = 0", [], |r| r.get::<_, i64>(0))
        .unwrap();
    l.entry(user(60, 1, json!("only in the wal"), 1_700_000_019_000));
    let db = agent.dir(ROOT).join("session.sqlite");
    let main_before = std::fs::read(&db).unwrap();
    assert!(std::fs::metadata(legacy::wal_path(&db)).unwrap().len() > 0);
    let wal_before = std::fs::read(legacy::wal_path(&db)).unwrap();

    let store = Store::memory().unwrap();
    import(&agent, &store, ROOT).unwrap();
    assert_eq!(std::fs::read(&db).unwrap(), main_before);
    assert_eq!(std::fs::read(legacy::wal_path(&db)).unwrap(), wal_before);
    assert!(store.entry(ROOT, 60).unwrap().is_some());
    assert_eq!(native_context(&store, ROOT), reference_context(&db, 1));
    drop(l);
}

#[test]
fn sessions_without_storage_or_meta_are_reported_not_imported() {
    let agent = Agent::new();
    let dir = agent.dir("empty-1");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("meta.json"), r#"{"createdAt": 1, "cwd": "/x"}"#).unwrap();
    std::fs::create_dir_all(agent.dir("nometa-1")).unwrap();
    let store = Store::memory().unwrap();
    let report = discover(&agent.root, &agent.root.join("server"), &store);
    let status = |id: &str| {
        report
            .sessions
            .iter()
            .find(|s| s.id == id)
            .unwrap()
            .status
            .clone()
    };
    assert!(matches!(status("empty-1"), SessionStatus::Blocked { .. }));
    assert!(matches!(status("nometa-1"), SessionStatus::Blocked { .. }));
    assert!(matches!(
        import(&agent, &store, "empty-1"),
        Err(ImportError::Blocked(_))
    ));
    assert!(matches!(
        import(&agent, &store, "missing"),
        Err(ImportError::NotFound(_))
    ));
    assert!(matches!(
        import(&agent, &store, "../etc"),
        Err(ImportError::InvalidId(_))
    ));
}

#[test]
fn credentials_and_config_are_listed_without_values() {
    let agent = Agent::new();
    std::fs::write(
        agent.root.join("auth.json"),
        r#"{"openrouter": {"type": "api_key", "key": "sk-or-SECRET"},
            "openai-codex": {"type": "oauth", "access": "tok-SECRET", "refresh": "r", "expires": 1},
            "anthropic": {"type": "oauth", "access": "sub-SECRET", "refresh": "r", "expires": 1}}"#,
    )
    .unwrap();
    std::fs::write(agent.root.join("models.json"), "{}").unwrap();
    std::fs::write(agent.root.join("AGENTS.md"), "# rules").unwrap();
    std::fs::create_dir_all(agent.root.join("skills/review")).unwrap();
    std::fs::write(agent.root.join("skills/review/SKILL.md"), "x").unwrap();
    let store = Store::memory().unwrap();
    let report = discover(&agent.root, &agent.root.join("server"), &store);
    let printed = format!("{report:?}");
    assert!(!printed.contains("SECRET"), "{printed}");
    let mut creds: Vec<(String, String, bool)> = report
        .credentials
        .iter()
        .map(|c| (c.provider.clone(), c.kind.clone(), c.reserved.is_some()))
        .collect();
    creds.sort();
    assert_eq!(
        creds,
        vec![
            ("anthropic".into(), "oauth".into(), false),
            ("openai-codex".into(), "oauth".into(), false),
            ("openrouter".into(), "api_key".into(), false),
        ]
    );
    let names: Vec<&str> = report.config.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["models.json", "AGENTS.md", "skills"]);

    let entries = credential_entries(&agent.root).unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries["anthropic"]["access"], "sub-SECRET");
    assert_eq!(entries["openrouter"]["key"], "sk-or-SECRET");

    let files = config_files(&agent.root);
    let rel: Vec<PathBuf> = files.iter().map(|(_, r)| r.clone()).collect();
    assert_eq!(
        rel,
        vec![
            PathBuf::from("models.json"),
            PathBuf::from("AGENTS.md"),
            PathBuf::from("skills/review/SKILL.md")
        ]
    );
    assert!(files.iter().all(|(s, _)| s.is_file()));

    std::fs::write(agent.root.join("auth.json"), "\"sk-SECRET\"").unwrap();
    let err = credential_entries(&agent.root).unwrap_err();
    assert!(!err.contains("SECRET"));
    let none = Agent::new();
    assert!(credential_entries(&none.root).unwrap().is_empty());
}

#[test]
fn subagent_selection_follows_the_reference_rules() {
    use plan::subagents_enabled as on;
    assert!(on(None));
    assert!(on(Some(&json!({}))));
    assert!(on(Some(
        &json!({"extensions": {"add": [], "remove": ["x"]}})
    )));
    assert!(!on(Some(&json!({"extensions": {"remove": ["subagent"]}}))));
    assert!(on(Some(
        &json!({"extensions": ["coding-tools", "subagent"]})
    )));
    assert!(!on(Some(&json!({"extensions": ["coding-tools"]}))));
    assert!(!on(Some(&json!({"tools": {"remove": ["subagent"]}}))));
    assert!(!on(Some(&json!({"tools": ["read", "bash"]}))));
}

/// Real reference sessions, when present: `fixtures/pi-sessions/` in the repository (or
/// `PIPKIN_PI_SESSION_FIXTURES`), holding either an agent dir (`experimental/sessions/<id>`)
/// or session directories directly. Each must import or be refused with reasons; imported ones
/// must keep their effective context.
#[test]
fn real_reference_fixtures_import_when_present() {
    let root = std::env::var_os("PIPKIN_PI_SESSION_FIXTURES")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pi-sessions")
        });
    if !root.is_dir() {
        eprintln!("no reference session fixtures at {}", root.display());
        return;
    }
    let agent = Agent::new();
    let source_sessions = if sessions_dir(&root).is_dir() {
        sessions_dir(&root)
    } else {
        root.clone()
    };
    let mut ids = Vec::new();
    for e in std::fs::read_dir(&source_sessions).unwrap() {
        let e = e.unwrap();
        let name = e.file_name().to_string_lossy().into_owned();
        if !e.path().is_dir() || !e.path().join("meta.json").is_file() {
            continue;
        }
        let dest = agent.dir(&name);
        std::fs::create_dir_all(&dest).unwrap();
        for f in std::fs::read_dir(e.path()).unwrap() {
            let f = f.unwrap();
            if f.path().is_file() {
                std::fs::copy(f.path(), dest.join(f.file_name())).unwrap();
            }
        }
        ids.push(name);
    }
    ids.sort();
    let store = Store::memory().unwrap();
    for id in ids {
        let db = agent.dir(&id).join("session.sqlite");
        match import(&agent, &store, &id) {
            Ok(outcome) => {
                eprintln!("{id}: imported ({} children)", outcome.children.len());
                let requests = source_sessions.join(&id).join("requests.jsonl");
                let mut sessions = vec![(id.clone(), 1i64)];
                for (conv, native) in outcome.manifest["conversations"].as_object().unwrap() {
                    if native != &json!(id) {
                        sessions.push((native.as_str().unwrap().to_owned(), conv.parse().unwrap()));
                    }
                }
                // `subagents.json` is the reference Subagents service list (status `done` is
                // the service's name for a completed task).
                let listed = source_sessions.join(&id).join("subagents.json");
                if listed.is_file() {
                    let listed: Value =
                        serde_json::from_str(&std::fs::read_to_string(listed).unwrap()).unwrap();
                    let (rows, _) = store.tasks(&id, 100).unwrap();
                    assert_eq!(rows.len(), listed.as_array().unwrap().len(), "{id}");
                    for (row, want) in rows.iter().zip(listed.as_array().unwrap()) {
                        let conv = want["conversationId"].as_i64().unwrap();
                        assert_eq!(row.child_session, child_session_id(&id, conv));
                        assert_eq!(row.parent_call, want["callId"].as_str().unwrap());
                        assert_eq!(row.task, want["task"].as_str().unwrap());
                        let status = match want["status"].as_str().unwrap() {
                            "done" => "completed",
                            "aborted" => "stopped",
                            other => other,
                        };
                        assert_eq!(row.status, status);
                        eprintln!(
                            "  task {}: {} delivered={} result={:?}",
                            row.child, row.status, row.result_delivered, row.result
                        );
                    }
                }
                for (native, conv) in sessions {
                    let context = native_context(&store, &native);
                    assert_eq!(context, reference_context(&db, conv), "{native}");
                    if requests.is_file() {
                        let checked = matches_last_request(&requests, &context);
                        eprintln!("  {native}: {checked}");
                    }
                }
            }
            Err(ImportError::Blocked(reasons)) => eprintln!("{id}: blocked: {reasons:?}"),
            Err(e) => panic!("{id}: {e}"),
        }
    }
}

/// (role, text, tool names) of an OpenAI chat-completions request message.
fn project_request(m: &Value) -> (String, String, Vec<String>) {
    let text = match &m["content"] {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect(),
        _ => String::new(),
    };
    let tools = m["tool_calls"]
        .as_array()
        .map(|c| {
            c.iter()
                .map(|t| t["function"]["name"].as_str().unwrap_or("").to_owned())
                .collect()
        })
        .unwrap_or_default();
    (m["role"].as_str().unwrap_or("").to_owned(), text, tools)
}

/// The same projection of a native context message.
fn project_native(m: &Value) -> (String, String, Vec<String>) {
    let role = match m["role"].as_str().unwrap_or("") {
        "toolResult" => "tool",
        other => other,
    };
    let blocks = m["content"].as_array().cloned().unwrap_or_default();
    let text = blocks
        .iter()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect();
    let tools = blocks
        .iter()
        .filter(|b| b["type"] == "toolCall")
        .map(|b| b["name"].as_str().unwrap_or("").to_owned())
        .collect();
    (role.to_owned(), text, tools)
}

/// The reference's newest provider request (with tools) for this conversation must be a prefix
/// of the imported context: what it sent is exactly what the native engine would send, plus
/// the answer the reference received afterwards.
fn matches_last_request(requests: &Path, context: &[Value]) -> String {
    let native: Vec<_> = context.iter().map(project_native).collect();
    let mut last: Option<Vec<(String, String, Vec<String>)>> = None;
    for line in std::fs::read_to_string(requests).unwrap().lines() {
        let request: Value = serde_json::from_str(line).unwrap();
        if request["body"]["tools"].is_null() {
            continue; // a summarization request
        }
        let messages: Vec<_> = request["body"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            // Reasoning models get the system prompt as `developer`.
            .filter(|m| m["role"] != "system" && m["role"] != "developer")
            .map(project_request)
            .collect();
        if messages.first() == native.first() {
            last = Some(messages);
        }
    }
    let Some(sent) = last else {
        return "no matching provider request".into();
    };
    assert!(
        native.len() > sent.len() && native[..sent.len()] == sent[..],
        "native context {native:?} does not extend the reference request {sent:?}"
    );
    format!(
        "matches the reference's last request ({} messages) plus {} later",
        sent.len(),
        native.len() - sent.len()
    )
}

fn plugin_profile(agent: &Agent, server: &str, session: Option<&Path>, packages: Value) -> PathBuf {
    use sha2::{Digest, Sha256};
    let dir = agent.root.join("server");
    std::fs::create_dir_all(&dir).unwrap();
    let (name, value) = match session {
        Some(session) => {
            let session = session.to_str().unwrap();
            let hash = format!("{:x}", Sha256::digest(session.as_bytes()));
            (
                format!("session-plugin-packages-{server}-{}.json", &hash[..24]),
                json!({"version":1,"sessionPath":session,"packagePaths":packages}),
            )
        }
        None => (
            format!("plugin-packages-{server}.json"),
            json!({"version":1,"packagePaths":packages}),
        ),
    };
    let path = dir.join(name);
    std::fs::write(&path, value.to_string()).unwrap();
    path
}

fn assert_plugin_blocked(agent: &Agent, store: &Store) {
    let report = discover(&agent.root, &agent.root.join("server"), store);
    let row = report.sessions.iter().find(|s| s.id == ROOT).unwrap();
    assert!(
        matches!(&row.status, SessionStatus::Blocked { reasons } if reasons.iter().any(|r| r.contains("plugin profile"))),
        "{:?}",
        row.status
    );
    assert!(matches!(
        import(agent, store, ROOT),
        Err(ImportError::Blocked(_))
    ));
    assert!(store.session(ROOT).unwrap().is_none());
    assert!(store.imported(ENGINE, ROOT).unwrap().is_none());
    assert!(
        !agent.backups().exists(),
        "refused before copying or activating"
    );
}

#[test]
fn plugin_profiles_block_sticky_and_pinned_selections_without_touching_the_source() {
    for pinned in [false, true] {
        let agent = Agent::new();
        let _legacy = rich_session(&agent);
        let store = Store::memory().unwrap();
        let profile = plugin_profile(
            &agent,
            "server-one",
            pinned.then(|| agent.dir(ROOT)).as_deref(),
            json!(["/plugins/questions"]),
        );
        let before = std::fs::read(&profile).unwrap();
        assert_plugin_blocked(&agent, &store);
        assert_eq!(std::fs::read(profile).unwrap(), before);
    }
}

#[test]
fn an_explicit_empty_session_selection_overrides_only_its_own_server() {
    let agent = Agent::new();
    let _legacy = rich_session(&agent);
    let store = Store::memory().unwrap();
    plugin_profile(&agent, "one", None, json!(["/plugins/sticky"]));
    plugin_profile(&agent, "one", Some(&agent.dir(ROOT)), json!([]));
    let other = plugin_profile(&agent, "two", None, json!(["/plugins/other"]));
    assert_plugin_blocked(&agent, &store);
    std::fs::remove_file(other).unwrap();
    let out = import(&agent, &store, ROOT).unwrap();
    let profiles = out.manifest["pluginProfiles"]["profiles"]
        .as_array()
        .unwrap();
    assert_eq!(profiles.len(), 2);
    assert!(
        profiles
            .iter()
            .any(|p| p["selection"]["packagePaths"] == json!([]))
    );
    assert!(
        profiles
            .iter()
            .all(|p| p["sha256"].as_str().unwrap().len() == 64)
    );
}

#[test]
fn malformed_unreadable_and_mismatched_plugin_profiles_fail_closed() {
    for malformed in [
        "not json",
        r#"{"version":2,"packagePaths":["/p"]}"#,
        r#"{"version":1,"packagePaths":[]}"#,
        r#"{"version":1,"packagePaths":["/p"],"unknown":true}"#,
        r#"{"version":1,"packagePaths":["/p","/p"]}"#,
        r#"{"version":1,"packagePaths":["relative"]}"#,
        r#"{"version":1,"packagePaths":["/p"],"sessionPath":null}"#,
    ] {
        let agent = Agent::new();
        let _legacy = rich_session(&agent);
        let store = Store::memory().unwrap();
        let path = plugin_profile(&agent, "one", None, json!(["/p"]));
        std::fs::write(path, malformed).unwrap();
        assert_plugin_blocked(&agent, &store);
    }
    let agent = Agent::new();
    let _legacy = rich_session(&agent);
    let store = Store::memory().unwrap();
    let profile = plugin_profile(&agent, "one", Some(&agent.dir(ROOT)), json!([]));
    std::fs::write(
        &profile,
        json!({"version":1,"sessionPath":"/wrong/session","packagePaths":[]}).to_string(),
    )
    .unwrap();
    assert_plugin_blocked(&agent, &store);
    std::fs::remove_file(&profile).unwrap();
    std::fs::create_dir(profile).unwrap();
    assert_plugin_blocked(&agent, &store);
}

#[test]
fn plugin_selection_is_rechecked_after_discovery_and_during_the_copy() {
    let agent = Agent::new();
    let _legacy = rich_session(&agent);
    let store = Store::memory().unwrap();
    let server = agent.root.join("server");
    assert!(matches!(
        discover(&agent.root, &server, &store).sessions[0].status,
        SessionStatus::Importable
    ));
    let before = plugins::Snapshot::read(&server).unwrap();
    plugin_profile(&agent, "new-server", None, json!(["/plugins/new"]));
    assert!(matches!(before.unchanged(), Err(ImportError::Blocked(_))));
    assert_plugin_blocked(&agent, &store);
}

#[test]
fn another_sessions_pinned_plugins_do_not_block_an_unselected_session() {
    let agent = Agent::new();
    let _legacy = rich_session(&agent);
    let store = Store::memory().unwrap();
    plugin_profile(
        &agent,
        "one",
        Some(&agent.dir("unrelated")),
        json!(["/plugins/elsewhere"]),
    );
    assert!(import(&agent, &store, ROOT).is_ok());
}

#[test]
fn a_copied_session_cannot_drop_a_profile_with_its_original_absolute_path() {
    let agent = Agent::new();
    let _legacy = rich_session(&agent);
    let store = Store::memory().unwrap();
    plugin_profile(
        &agent,
        "one",
        Some(&Path::new("/previous/agent/experimental/sessions").join(ROOT)),
        json!(["/plugins/questions"]),
    );
    assert_plugin_blocked(&agent, &store);
}

#[cfg(unix)]
#[test]
fn a_symlinked_session_keeps_its_plugin_selection() {
    let agent = Agent::new();
    let _legacy = rich_session(&agent);
    let store = Store::memory().unwrap();
    let alias = agent.root.join("alias");
    std::os::unix::fs::symlink(agent.dir(ROOT), &alias).unwrap();
    plugin_profile(&agent, "one", Some(&alias), json!(["/plugins/questions"]));
    assert_plugin_blocked(&agent, &store);
}
