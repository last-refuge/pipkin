//! Credential storage: the reference `auth.json` format behind a [`CredentialStore`] trait.
//!
//! File format (reference `auth/types.ts`, `coding-agent/core/auth-storage.ts`):
//! `{ "<providerId>": {"type":"api_key","key"?,"env"?} | {"type":"oauth","access","refresh","expires",...extra} }`,
//! pretty-printed with two-space indentation.
//!
//! Guarantees of [`FileStore`]:
//! - the directory is created `0700` and the file is written `0600`;
//! - every write is atomic (temporary file in the same directory, `fsync`, `rename`);
//! - writers hold a cross-process lock (`flock` on `auth.json.lock`, bounded wait) for the whole
//!   read-modify-write, so concurrent processes and in-process callers serialize;
//! - entries this engine does not understand (unknown `type`, invalid shape) and every entry a
//!   transaction did not change are written back byte-for-byte as they were read;
//! - an unreadable or invalid file is never overwritten (fail closed);
//! - [`CredentialStore::list`] is metadata only: it never resolves configured key values.
//!
//! OS secret stores (Secret Service, Keychain) are **not yet qualified**; a future backend
//! implements [`CredentialStore`] and replaces [`FileStore`] without changing callers.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

/// Why the credential store could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    /// The file exists but is not a valid credential file. It is never overwritten.
    Invalid(String),
    /// Another process (or caller) held the lock for longer than the bounded wait.
    Locked,
    Io(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Invalid(m) => write!(f, "auth.json is invalid: {m}"),
            StoreError::Locked => f.write_str("auth.json is locked by another process"),
            StoreError::Io(m) => write!(f, "auth.json could not be accessed: {m}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// A stored OAuth credential. `extra` keeps provider metadata (account id, client id, ...).
#[derive(Clone, PartialEq)]
pub struct OAuthCredential {
    pub access: String,
    pub refresh: String,
    /// Milliseconds since the epoch.
    pub expires: i64,
    pub extra: Map<String, Value>,
}

impl std::fmt::Debug for OAuthCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthCredential")
            .field("expires", &self.expires)
            .field("extra_keys", &self.extra.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// One provider's stored credential.
#[derive(Clone, PartialEq)]
pub enum StoredCredential {
    ApiKey {
        /// A config value: literal, `$VAR` template or `!command`.
        key: Option<String>,
        env: BTreeMap<String, String>,
        extra: Map<String, Value>,
    },
    OAuth(OAuthCredential),
    /// An entry this engine does not understand. Preserved untouched.
    Unsupported(Value),
}

impl std::fmt::Debug for StoredCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoredCredential::ApiKey { env, .. } => f
                .debug_struct("ApiKey")
                .field("env_keys", &env.keys().collect::<Vec<_>>())
                .finish_non_exhaustive(),
            StoredCredential::OAuth(c) => c.fmt(f),
            StoredCredential::Unsupported(_) => f.write_str("Unsupported"),
        }
    }
}

/// `api_key`, `oauth` or `unsupported`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialType {
    ApiKey,
    OAuth,
    Unsupported,
}

/// Non-secret metadata about one stored credential.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialInfo {
    pub provider: String,
    pub kind: CredentialType,
}

impl StoredCredential {
    pub fn kind(&self) -> CredentialType {
        match self {
            StoredCredential::ApiKey { .. } => CredentialType::ApiKey,
            StoredCredential::OAuth(_) => CredentialType::OAuth,
            StoredCredential::Unsupported(_) => CredentialType::Unsupported,
        }
    }

    pub fn api_key(key: impl Into<String>) -> StoredCredential {
        StoredCredential::ApiKey {
            key: Some(key.into()),
            env: BTreeMap::new(),
            extra: Map::new(),
        }
    }

    /// Interpret one `auth.json` entry (reference validation, `auth-storage.ts:203-256`).
    pub fn from_value(value: &Value) -> StoredCredential {
        let Some(obj) = value.as_object() else {
            return StoredCredential::Unsupported(value.clone());
        };
        match obj.get("type").and_then(Value::as_str) {
            Some("api_key") => {
                let key = match obj.get("key") {
                    None => None,
                    Some(Value::String(s)) => Some(s.clone()),
                    Some(_) => return StoredCredential::Unsupported(value.clone()),
                };
                let mut env = BTreeMap::new();
                match obj.get("env") {
                    None => {}
                    Some(Value::Object(m)) => {
                        for (k, v) in m {
                            match v {
                                Value::String(s) => {
                                    env.insert(k.clone(), s.clone());
                                }
                                _ => return StoredCredential::Unsupported(value.clone()),
                            }
                        }
                    }
                    Some(_) => return StoredCredential::Unsupported(value.clone()),
                }
                let extra = obj
                    .iter()
                    .filter(|(k, _)| !matches!(k.as_str(), "type" | "key" | "env"))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                StoredCredential::ApiKey { key, env, extra }
            }
            Some("oauth") => {
                let access = obj.get("access").and_then(Value::as_str);
                let refresh = obj.get("refresh").and_then(Value::as_str);
                let expires = obj
                    .get("expires")
                    .and_then(Value::as_f64)
                    .filter(|e| e.is_finite());
                match (access, refresh, expires) {
                    (Some(access), Some(refresh), Some(expires)) => {
                        StoredCredential::OAuth(OAuthCredential {
                            access: access.to_owned(),
                            refresh: refresh.to_owned(),
                            expires: expires as i64,
                            extra: obj
                                .iter()
                                .filter(|(k, _)| {
                                    !matches!(k.as_str(), "type" | "access" | "refresh" | "expires")
                                })
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect(),
                        })
                    }
                    _ => StoredCredential::Unsupported(value.clone()),
                }
            }
            _ => StoredCredential::Unsupported(value.clone()),
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            StoredCredential::ApiKey { key, env, extra } => {
                let mut m = Map::new();
                m.insert("type".into(), Value::String("api_key".into()));
                if let Some(k) = key {
                    m.insert("key".into(), Value::String(k.clone()));
                }
                if !env.is_empty() {
                    m.insert(
                        "env".into(),
                        Value::Object(
                            env.iter()
                                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                                .collect(),
                        ),
                    );
                }
                for (k, v) in extra {
                    m.insert(k.clone(), v.clone());
                }
                Value::Object(m)
            }
            StoredCredential::OAuth(c) => {
                let mut m = Map::new();
                m.insert("type".into(), Value::String("oauth".into()));
                m.insert("refresh".into(), Value::String(c.refresh.clone()));
                m.insert("access".into(), Value::String(c.access.clone()));
                m.insert("expires".into(), Value::from(c.expires));
                for (k, v) in &c.extra {
                    if !v.is_null() {
                        m.insert(k.clone(), v.clone());
                    }
                }
                Value::Object(m)
            }
            StoredCredential::Unsupported(v) => v.clone(),
        }
    }
}

/// An exclusive read-modify-write over the whole store. The lock is held until the transaction
/// is committed or dropped; dropping without committing discards changes.
pub trait StoreTxn: Send {
    fn get(&self, provider: &str) -> Option<StoredCredential>;
    fn set(&mut self, provider: &str, credential: StoredCredential);
    /// Returns whether an entry existed.
    fn remove(&mut self, provider: &str) -> bool;
    fn commit(self: Box<Self>) -> Result<(), StoreError>;
}

/// App-owned credential storage, one credential per provider id.
pub trait CredentialStore: Send + Sync {
    /// Metadata only; never resolves configured key values or runs commands.
    fn list(&self) -> Result<Vec<CredentialInfo>, StoreError>;
    /// The stored credential, possibly expired, with key values unresolved.
    fn read(&self, provider: &str) -> Result<Option<StoredCredential>, StoreError>;
    /// Start an exclusive transaction. Blocks (bounded) while another holder has the lock.
    fn begin(&self) -> Result<Box<dyn StoreTxn>, StoreError>;
}

/// Default lock wait (reference async lock: stale after 30 s).
pub const DEFAULT_LOCK_WAIT: Duration = Duration::from_secs(30);

/// `auth.json` in a directory the engine owns.
pub struct FileStore {
    path: PathBuf,
    lock_wait: Duration,
}

impl FileStore {
    pub fn new(path: impl Into<PathBuf>) -> FileStore {
        FileStore {
            path: path.into(),
            lock_wait: DEFAULT_LOCK_WAIT,
        }
    }

    /// Override the bounded lock wait (tests).
    pub fn with_lock_wait(mut self, wait: Duration) -> FileStore {
        self.lock_wait = wait;
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn lock_path(&self) -> PathBuf {
        let mut name = self
            .path
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_else(|| "auth.json".into());
        name.push(".lock");
        self.path.with_file_name(name)
    }

    fn ensure_dir(&self) -> Result<(), StoreError> {
        let Some(dir) = self.path.parent() else {
            return Ok(());
        };
        if dir.as_os_str().is_empty() || dir.exists() {
            return Ok(());
        }
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| StoreError::Io(e.to_string()))
    }

    fn load(&self) -> Result<Document, StoreError> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => Document::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Document::default()),
            Err(e) => Err(StoreError::Io(e.to_string())),
        }
    }

    fn acquire(&self) -> Result<File, StoreError> {
        self.ensure_dir()?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(self.lock_path())
            .map_err(|e| StoreError::Io(e.to_string()))?;
        let deadline = Instant::now() + self.lock_wait;
        let mut delay = Duration::from_millis(5);
        loop {
            // SAFETY: `flock` on a valid, owned descriptor; released when `lock` is closed.
            let rc = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if rc == 0 {
                return Ok(lock);
            }
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::EWOULDBLOCK) {
                return Err(StoreError::Io(err.to_string()));
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(StoreError::Locked);
            }
            std::thread::sleep(delay.min(deadline - now));
            delay = (delay * 2).min(Duration::from_millis(200));
        }
    }

    fn write_atomic(&self, text: &str) -> Result<(), StoreError> {
        self.ensure_dir()?;
        let dir = self
            .path
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "auth.json".into());
        let tmp = dir.join(format!(".{name}.{}.tmp", crate::util::random_hex(6)));
        let result = (|| -> std::io::Result<()> {
            let mut f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?;
            // The mode on creation is subject to the umask; make it exact.
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
            std::fs::rename(&tmp, &self.path)?;
            if let Ok(d) = File::open(dir) {
                let _ = d.sync_all();
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result.map_err(|e| StoreError::Io(e.to_string()))
    }
}

impl CredentialStore for FileStore {
    fn list(&self) -> Result<Vec<CredentialInfo>, StoreError> {
        Ok(self
            .load()?
            .entries
            .iter()
            .map(|e| CredentialInfo {
                provider: e.provider.clone(),
                kind: StoredCredential::from_value(&e.value).kind(),
            })
            .collect())
    }

    fn read(&self, provider: &str) -> Result<Option<StoredCredential>, StoreError> {
        Ok(self.load()?.get(provider).map(StoredCredential::from_value))
    }

    fn begin(&self) -> Result<Box<dyn StoreTxn>, StoreError> {
        let lock = self.acquire()?;
        let doc = self.load()?;
        Ok(Box::new(FileTxn {
            store: FileStore {
                path: self.path.clone(),
                lock_wait: self.lock_wait,
            },
            doc,
            dirty: false,
            _lock: lock,
        }))
    }
}

struct FileTxn {
    store: FileStore,
    doc: Document,
    dirty: bool,
    _lock: File,
}

impl StoreTxn for FileTxn {
    fn get(&self, provider: &str) -> Option<StoredCredential> {
        self.doc.get(provider).map(StoredCredential::from_value)
    }

    fn set(&mut self, provider: &str, credential: StoredCredential) {
        let value = credential.to_value();
        if self.doc.get(provider) == Some(&value) {
            return;
        }
        self.doc.set(provider, value);
        self.dirty = true;
    }

    fn remove(&mut self, provider: &str) -> bool {
        let existed = self.doc.remove(provider);
        self.dirty |= existed;
        existed
    }

    fn commit(self: Box<Self>) -> Result<(), StoreError> {
        if !self.dirty {
            return Ok(());
        }
        self.store.write_atomic(&self.doc.render())
    }
}

/// One top-level entry with its original text (if unchanged since reading).
#[derive(Clone, Debug)]
struct DocEntry {
    provider: String,
    value: Value,
    raw: Option<String>,
}

/// `auth.json` as an ordered list of entries that remembers their original bytes.
#[derive(Clone, Debug, Default)]
struct Document {
    entries: Vec<DocEntry>,
}

impl Document {
    fn parse(text: &str) -> Result<Document, StoreError> {
        let text = text.trim_start_matches('\u{feff}');
        if text.trim().is_empty() {
            return Ok(Document::default());
        }
        let value: Value =
            serde_json::from_str(text).map_err(|e| StoreError::Invalid(e.to_string()))?;
        let Value::Object(map) = value else {
            return Err(StoreError::Invalid("expected an object".into()));
        };
        let spans = top_level_spans(text)
            .ok_or_else(|| StoreError::Invalid("unexpected structure".into()))?;
        let mut entries: Vec<DocEntry> = Vec::new();
        for (provider, raw) in spans {
            let Some(value) = map.get(&provider) else {
                continue;
            };
            // Duplicate keys: the last occurrence wins (as JSON.parse does).
            entries.retain(|e| e.provider != provider);
            entries.push(DocEntry {
                provider,
                value: value.clone(),
                raw: Some(raw.to_owned()),
            });
        }
        Ok(Document { entries })
    }

    fn get(&self, provider: &str) -> Option<&Value> {
        self.entries
            .iter()
            .find(|e| e.provider == provider)
            .map(|e| &e.value)
    }

    fn set(&mut self, provider: &str, value: Value) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.provider == provider) {
            e.value = value;
            e.raw = None;
        } else {
            self.entries.push(DocEntry {
                provider: provider.to_owned(),
                value,
                raw: None,
            });
        }
    }

    fn remove(&mut self, provider: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.provider != provider);
        before != self.entries.len()
    }

    /// Two-space pretty JSON (reference `JSON.stringify(data, null, 2)`), reusing the original
    /// bytes of unchanged entries.
    fn render(&self) -> String {
        if self.entries.is_empty() {
            return "{}".into();
        }
        let mut out = String::from("{\n");
        for (i, e) in self.entries.iter().enumerate() {
            out.push_str("  ");
            out.push_str(&serde_json::to_string(&e.provider).unwrap_or_default());
            out.push_str(": ");
            match &e.raw {
                Some(raw) => out.push_str(raw),
                None => {
                    let pretty = serde_json::to_string_pretty(&e.value).unwrap_or_default();
                    out.push_str(&pretty.replace('\n', "\n  "));
                }
            }
            if i + 1 < self.entries.len() {
                out.push(',');
            }
            out.push('\n');
        }
        out.push('}');
        out
    }
}

/// Split a valid JSON object into `(key, raw value text)` pairs. Returns `None` when `text` is
/// not an object (callers validate with serde first).
fn top_level_spans(text: &str) -> Option<Vec<(String, &str)>> {
    let b = text.as_bytes();
    let mut i = skip_ws(b, 0);
    if b.get(i) != Some(&b'{') {
        return None;
    }
    i += 1;
    let mut out = Vec::new();
    loop {
        i = skip_ws(b, i);
        match b.get(i)? {
            b'}' => return Some(out),
            b',' => {
                i += 1;
                continue;
            }
            b'"' => {}
            _ => return None,
        }
        let key_end = scan_string(b, i)?;
        let key: String = serde_json::from_str(&text[i..key_end]).ok()?;
        i = skip_ws(b, key_end);
        if b.get(i) != Some(&b':') {
            return None;
        }
        i = skip_ws(b, i + 1);
        let end = scan_value(b, i)?;
        out.push((key, &text[i..end]));
        i = end;
    }
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\n' | b'\r') {
        i += 1;
    }
    i
}

/// `b[start]` is `"`; returns the index after the closing quote.
fn scan_string(b: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return Some(i + 1),
            _ => i += 1,
        }
    }
    None
}

fn scan_value(b: &[u8], start: usize) -> Option<usize> {
    match b.get(start)? {
        b'"' => scan_string(b, start),
        b'{' | b'[' => {
            let mut depth = 0usize;
            let mut i = start;
            while i < b.len() {
                match b[i] {
                    b'"' => {
                        i = scan_string(b, i)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(i + 1);
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            None
        }
        _ => {
            let mut i = start;
            while i < b.len() && !matches!(b[i], b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r')
            {
                i += 1;
            }
            Some(i)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn writes_create_a_private_dir_and_file_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg/nested/auth.json");
        let store = FileStore::new(&path);
        let mut txn = store.begin().unwrap();
        txn.set("openai", StoredCredential::api_key("sk-test"));
        txn.commit().unwrap();
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        assert_eq!(mode(&dir.path().join("cfg")), 0o700);
        assert_eq!(mode(&path), 0o600);
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text,
            "{\n  \"openai\": {\n    \"type\": \"api_key\",\n    \"key\": \"sk-test\"\n  }\n}"
        );
        // No temporary files are left behind.
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        let mut names = names;
        names.sort();
        assert_eq!(names, vec!["auth.json", "auth.json.lock"]);
        assert_eq!(
            store.read("openai").unwrap(),
            Some(StoredCredential::api_key("sk-test"))
        );
    }

    #[test]
    fn untouched_and_unknown_entries_are_preserved_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let original = "{\n  \"anthropic\": {\"type\":\"oauth\",\"refresh\":\"r\",  \"access\":\"a\",\"expires\":1.76e12,\"note\":\"x\\u0041\"},\n  \"weird\": {\"type\": \"future_kind\", \"blob\": [1, 2, {\"a\": \"}\"}]},\n  \"num\": 12.50,\n  \"openai\": {\"type\": \"api_key\", \"key\": \"old\"}\n}\n";
        std::fs::write(&path, original).unwrap();
        let store = FileStore::new(&path);
        assert!(matches!(
            store.read("weird").unwrap(),
            Some(StoredCredential::Unsupported(_))
        ));
        assert!(matches!(
            store.read("anthropic").unwrap(),
            Some(StoredCredential::OAuth(_))
        ));
        let mut txn = store.begin().unwrap();
        txn.set("openai", StoredCredential::api_key("new"));
        txn.set("xai", StoredCredential::api_key("k"));
        txn.commit().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"anthropic\": {\"type\":\"oauth\",\"refresh\":\"r\",  \"access\":\"a\",\"expires\":1.76e12,\"note\":\"x\\u0041\"},"), "{text}");
        assert!(
            text.contains(
                "\"weird\": {\"type\": \"future_kind\", \"blob\": [1, 2, {\"a\": \"}\"}]},"
            )
        );
        assert!(text.contains("\"num\": 12.50,"));
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["openai"], json!({"type": "api_key", "key": "new"}));
        assert_eq!(parsed["xai"]["key"], "k");
        // Order of existing entries is kept; new entries go last.
        let order: Vec<_> = top_level_spans(&text)
            .unwrap()
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(order, vec!["anthropic", "weird", "num", "openai", "xai"]);
        // Removing one entry keeps the others' bytes.
        let mut txn = store.begin().unwrap();
        assert!(txn.remove("openai"));
        assert!(!txn.remove("missing"));
        txn.commit().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"expires\":1.76e12"));
        assert!(!text.contains("openai"));
    }

    #[test]
    fn invalid_files_are_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(&path, "{ not json").unwrap();
        let store = FileStore::new(&path);
        assert!(matches!(store.list(), Err(StoreError::Invalid(_))));
        assert!(matches!(store.read("x"), Err(StoreError::Invalid(_))));
        assert!(matches!(store.begin(), Err(StoreError::Invalid(_))));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
        std::fs::write(&path, "[]").unwrap();
        assert!(matches!(store.list(), Err(StoreError::Invalid(_))));
    }

    #[test]
    fn list_is_metadata_only() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("ran");
        let path = dir.path().join("auth.json");
        std::fs::write(
            &path,
            json!({
                "a": {"type": "api_key", "key": format!("!touch {}", marker.display())},
                "b": {"type": "oauth", "access": "x", "refresh": "y", "expires": 1},
                "c": {"type": "oauth", "access": "x"}
            })
            .to_string(),
        )
        .unwrap();
        let list = FileStore::new(&path).list().unwrap();
        assert_eq!(
            list.iter().map(|i| i.kind).collect::<Vec<_>>(),
            vec![
                CredentialType::ApiKey,
                CredentialType::OAuth,
                CredentialType::Unsupported
            ]
        );
        assert!(!marker.exists());
    }

    #[test]
    fn the_lock_serializes_writers_across_handles_with_a_bounded_wait() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        // A separate handle has its own descriptor, exactly like another process.
        let a = FileStore::new(&path);
        let b = FileStore::new(&path).with_lock_wait(Duration::from_millis(150));
        let held = a.begin().unwrap();
        let started = Instant::now();
        assert_eq!(b.begin().err(), Some(StoreError::Locked));
        assert!(started.elapsed() >= Duration::from_millis(140));
        drop(held);
        // Released: the other handle proceeds and sees committed data.
        let mut txn = b.begin().unwrap();
        txn.set("x", StoredCredential::api_key("1"));
        txn.commit().unwrap();

        // A waiter blocks until the holder commits, then reads its write.
        let holder = a.begin().unwrap();
        let path2 = path.clone();
        let waiter = std::thread::spawn(move || {
            let store = FileStore::new(&path2);
            let txn = store.begin().unwrap();
            txn.get("x")
        });
        std::thread::sleep(Duration::from_millis(50));
        let mut holder = holder;
        holder.set("x", StoredCredential::api_key("2"));
        holder.commit().unwrap();
        assert_eq!(waiter.join().unwrap(), Some(StoredCredential::api_key("2")));
    }

    #[test]
    fn concurrent_writers_never_lose_updates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let threads: Vec<_> = (0..8)
            .map(|n| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let store = FileStore::new(&path);
                    let mut txn = store.begin().unwrap();
                    txn.set(&format!("p{n}"), StoredCredential::api_key(format!("k{n}")));
                    txn.commit().unwrap();
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let list = FileStore::new(&path).list().unwrap();
        assert_eq!(list.len(), 8);
    }

    #[test]
    fn oauth_round_trip_keeps_extra_fields() {
        let v = json!({"type": "oauth", "access": "a", "refresh": "r", "expires": 9007199254740991i64,
            "accountId": "acc", "availableModelIds": ["m1"]});
        let c = StoredCredential::from_value(&v);
        let StoredCredential::OAuth(o) = &c else {
            panic!()
        };
        assert_eq!(o.expires, 9007199254740991);
        assert_eq!(c.to_value(), v);
        assert!(!format!("{c:?}").contains("\"a\""));
    }
}
