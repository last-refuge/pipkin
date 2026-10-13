//! OS keychain credential storage behind [`CredentialStore`], over the Secret Service
//! command-line client (`secret-tool`, libsecret). Each credential is one keychain item:
//! its JSON form as the secret, found by the attributes `pipkin-store` (the store's label)
//! and `pipkin-provider` (the credential's id). The binary is injectable so tests run
//! against a fake; nothing links against libsecret. A missing or failing client is an
//! ordinary store error, never a silent fallback to a file.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::store::{CredentialInfo, CredentialStore, StoreError, StoreTxn};
use crate::auth::store::StoredCredential;

/// Where the client usually lives on the supported desktop.
pub const DEFAULT_SECRET_TOOL: &str = "/usr/bin/secret-tool";
/// The attribute that scopes items to one store.
pub const STORE_ATTR: &str = "pipkin-store";
/// The attribute that names the credential within a store.
pub const PROVIDER_ATTR: &str = "pipkin-provider";
/// Bounded wait for one client invocation.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(20);

/// A keychain-backed store: `label` scopes its items (`pipkin-mcp`, `pipkin-mcp-secrets`).
pub struct SecretToolStore {
    binary: PathBuf,
    label: String,
    lock: Arc<tokio::sync::Mutex<()>>,
    lock_wait: Duration,
}

impl SecretToolStore {
    pub fn new(binary: impl Into<PathBuf>, label: impl Into<String>) -> SecretToolStore {
        SecretToolStore {
            binary: binary.into(),
            label: label.into(),
            lock: Arc::new(tokio::sync::Mutex::new(())),
            lock_wait: super::store::DEFAULT_LOCK_WAIT,
        }
    }

    /// The client when it exists at `binary` (or the default path), else `None`.
    pub fn available(binary: Option<&Path>) -> Option<PathBuf> {
        let path = binary
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_SECRET_TOOL));
        path.is_file().then_some(path)
    }

    fn run(&self, args: &[&str], stdin: Option<&str>) -> Result<(bool, String), StoreError> {
        let mut command = Command::new(&self.binary);
        command
            .args(args)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command
            .spawn()
            .map_err(|e| StoreError::Io(format!("{}: {e}", self.binary.display())))?;
        if let Some(input) = stdin
            && let Some(mut pipe) = child.stdin.take()
        {
            // A closed pipe (the client refused early) is reported through its exit.
            let _ = pipe.write_all(input.as_bytes());
        }
        let started = Instant::now();
        let output = loop {
            match child.try_wait() {
                Ok(Some(_)) => break child.wait_with_output(),
                Ok(None) if started.elapsed() < CLIENT_TIMEOUT => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(None) => {
                    let _ = child.kill();
                    return Err(StoreError::Io(format!(
                        "{} did not finish within {} s",
                        self.binary.display(),
                        CLIENT_TIMEOUT.as_secs()
                    )));
                }
                Err(e) => return Err(StoreError::Io(e.to_string())),
            }
        }
        .map_err(|e| StoreError::Io(e.to_string()))?;
        Ok((
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
        ))
    }

    /// The secret stored under `provider`, verbatim.
    fn lookup(&self, provider: &str) -> Result<Option<String>, StoreError> {
        let (ok, out) = self.run(
            &["lookup", STORE_ATTR, &self.label, PROVIDER_ATTR, provider],
            None,
        )?;
        Ok(ok.then(|| out.trim_end_matches('\n').to_owned()))
    }

    /// A plain secret value (not a credential) stored under `provider`, for `secret:<key>`
    /// references.
    pub fn lookup_secret(&self, key: &str) -> Result<Option<String>, StoreError> {
        self.lookup(key)
    }

    fn store(&self, provider: &str, secret: &str) -> Result<(), StoreError> {
        let label = format!("Pipkin {} {provider}", self.label);
        let (ok, _) = self.run(
            &[
                "store",
                &format!("--label={label}"),
                STORE_ATTR,
                &self.label,
                PROVIDER_ATTR,
                provider,
            ],
            Some(secret),
        )?;
        if ok {
            Ok(())
        } else {
            Err(StoreError::Io(format!(
                "the keychain refused to store {provider} (is the Secret Service running and unlocked?)"
            )))
        }
    }

    fn clear(&self, provider: &str) -> Result<(), StoreError> {
        // A missing item is not an error (`clear` exits 0 either way).
        self.run(
            &["clear", STORE_ATTR, &self.label, PROVIDER_ATTR, provider],
            None,
        )
        .map(|_| ())
    }

    /// Every item of this store: `(provider, secret)`.
    fn search(&self) -> Result<Vec<(String, String)>, StoreError> {
        let (_, out) = self.run(
            &["search", "--all", "--unlock", STORE_ATTR, &self.label],
            None,
        )?;
        Ok(parse_search(&out))
    }

    fn acquire(&self) -> Result<tokio::sync::OwnedMutexGuard<()>, StoreError> {
        let started = Instant::now();
        loop {
            if let Ok(guard) = self.lock.clone().try_lock_owned() {
                return Ok(guard);
            }
            if started.elapsed() > self.lock_wait {
                return Err(StoreError::Locked);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// `secret-tool search` output: `[path]` headers, then `key = value` lines, blank between
/// items. Only the secret and the provider attribute are read.
fn parse_search(out: &str) -> Vec<(String, String)> {
    let mut items = Vec::new();
    let mut provider: Option<String> = None;
    let mut secret: Option<String> = None;
    let flush = |provider: &mut Option<String>, secret: &mut Option<String>, items: &mut Vec<_>| {
        if let (Some(p), Some(s)) = (provider.take(), secret.take()) {
            items.push((p, s));
        }
        *provider = None;
        *secret = None;
    };
    for line in out.lines() {
        if line.starts_with('[') {
            flush(&mut provider, &mut secret, &mut items);
            continue;
        }
        if let Some((key, value)) = line.split_once(" = ") {
            match key.trim() {
                "secret" => secret = Some(value.to_owned()),
                k if k == format!("attribute.{PROVIDER_ATTR}") => {
                    provider = Some(value.trim().to_owned());
                }
                _ => {}
            }
        }
    }
    flush(&mut provider, &mut secret, &mut items);
    items
}

fn parse_credential(secret: &str) -> StoredCredential {
    match serde_json::from_str::<Value>(secret) {
        Ok(v) => StoredCredential::from_value(&v),
        Err(_) => StoredCredential::Unsupported(Value::String(String::new())),
    }
}

impl CredentialStore for SecretToolStore {
    fn list(&self) -> Result<Vec<CredentialInfo>, StoreError> {
        Ok(self
            .search()?
            .into_iter()
            .map(|(provider, secret)| CredentialInfo {
                provider,
                kind: parse_credential(&secret).kind(),
            })
            .collect())
    }

    fn read(&self, provider: &str) -> Result<Option<StoredCredential>, StoreError> {
        Ok(self.lookup(provider)?.map(|s| parse_credential(&s)))
    }

    fn begin(&self) -> Result<Box<dyn StoreTxn>, StoreError> {
        let guard = self.acquire()?;
        Ok(Box::new(KeychainTxn {
            store: SecretToolStore {
                binary: self.binary.clone(),
                label: self.label.clone(),
                lock: self.lock.clone(),
                lock_wait: self.lock_wait,
            },
            pending: BTreeMap::new(),
            _guard: guard,
        }))
    }
}

/// Buffered writes applied on commit, under the store's lock; reads see pending writes.
struct KeychainTxn {
    store: SecretToolStore,
    /// `Some` to store, `None` to clear.
    pending: BTreeMap<String, Option<StoredCredential>>,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

impl StoreTxn for KeychainTxn {
    fn get(&self, provider: &str) -> Option<StoredCredential> {
        match self.pending.get(provider) {
            Some(pending) => pending.clone(),
            None => self.store.read(provider).ok().flatten(),
        }
    }

    fn set(&mut self, provider: &str, credential: StoredCredential) {
        self.pending.insert(provider.to_owned(), Some(credential));
    }

    fn remove(&mut self, provider: &str) -> bool {
        let existed = self.get(provider).is_some();
        self.pending.insert(provider.to_owned(), None);
        existed
    }

    fn commit(self: Box<Self>) -> Result<(), StoreError> {
        for (provider, change) in &self.pending {
            match change {
                Some(credential) => {
                    let secret = serde_json::to_string(&credential.to_value())
                        .map_err(|e| StoreError::Io(e.to_string()))?;
                    self.store.store(provider, &secret)?;
                }
                None => self.store.clear(provider)?,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::auth::store::OAuthCredential;

    /// A fake `secret-tool` over a directory: `store` (secret on stdin), `lookup`, `clear`
    /// and `search`, keyed by the attribute pairs in the order given.
    pub(crate) fn fake_secret_tool(dir: &Path) -> PathBuf {
        let items = dir.join("items");
        std::fs::create_dir_all(&items).unwrap();
        let script = dir.join("secret-tool");
        std::fs::write(
            &script,
            format!(
                r#"#!/bin/sh
dir="{items}"
cmd=$1; shift
label=""
key=""
for a in "$@"; do
  case "$a" in
    --label=*) label=${{a#--label=}};;
    --*) ;;
    *) if [ -z "$pending" ]; then pending=$a; else key="$key$pending=$a;"; pending=""; fi;;
  esac
done
f="$dir/$key"
case "$cmd" in
  store) cat > "$f"; printf '%s' "$label" > "$f.label";;
  lookup) if [ -f "$f" ]; then cat "$f"; else exit 1; fi;;
  clear) rm -f "$f" "$f.label";;
  search) for g in "$dir"/*; do
      case "$g" in *.label) continue;; esac
      [ -f "$g" ] || continue
      n=$(basename "$g")
      case "$n" in "$key"*) printf '[/fake/%s]\nlabel = %s\nsecret = %s\n' "$n" "$(cat "$g.label")" "$(cat "$g")"
        printf '%s' "$n" | tr ';' '\n' | sed -n 's/^\(.*\)=\(.*\)$/attribute.\1 = \2/p'
        echo;;
      esac
    done;;
  *) exit 2;;
esac
"#,
                items = items.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    fn oauth(access: &str) -> StoredCredential {
        StoredCredential::OAuth(OAuthCredential {
            access: access.into(),
            refresh: "r".into(),
            expires: 1,
            extra: Default::default(),
        })
    }

    #[test]
    fn items_round_trip_through_the_client_and_stores_are_scoped_by_label() {
        let dir = tempfile::tempdir().unwrap();
        let tool = fake_secret_tool(dir.path());
        let store = SecretToolStore::new(&tool, "pipkin-mcp");
        assert!(store.list().unwrap().is_empty());
        assert_eq!(store.read("docs").unwrap(), None);
        let mut txn = store.begin().unwrap();
        txn.set("docs", oauth("tok"));
        txn.set(
            "other",
            StoredCredential::ApiKey {
                key: Some("k".into()),
                env: Default::default(),
                extra: Default::default(),
            },
        );
        assert!(
            txn.get("docs").is_some(),
            "a transaction reads its own writes"
        );
        txn.commit().unwrap();
        assert_eq!(store.read("docs").unwrap(), Some(oauth("tok")));
        let mut listed: Vec<_> = store
            .list()
            .unwrap()
            .into_iter()
            .map(|i| i.provider)
            .collect();
        listed.sort();
        assert_eq!(listed, ["docs", "other"]);
        // Another label sees nothing; plain secrets live beside credentials.
        let secrets = SecretToolStore::new(&tool, "pipkin-mcp-secrets");
        assert!(secrets.list().unwrap().is_empty());
        assert_eq!(secrets.lookup_secret("docs").unwrap(), None);
        let mut txn = secrets.begin().unwrap();
        txn.set(
            "api",
            StoredCredential::Unsupported(Value::String("plain".into())),
        );
        txn.commit().unwrap();
        assert_eq!(
            secrets.lookup_secret("api").unwrap().as_deref(),
            Some("\"plain\"")
        );
        // Removal, and a second transaction while one is open waits then gives up.
        let mut txn = store.begin().unwrap();
        assert!(txn.remove("docs"));
        assert!(!txn.remove("absent"));
        let mut quick = SecretToolStore::new(&tool, "pipkin-mcp");
        quick.lock = store.lock.clone();
        quick.lock_wait = Duration::from_millis(20);
        assert!(matches!(quick.begin(), Err(StoreError::Locked)));
        txn.commit().unwrap();
        assert_eq!(store.read("docs").unwrap(), None);
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[test]
    fn a_missing_client_is_an_error_not_a_fallback() {
        let store = SecretToolStore::new("/nonexistent/secret-tool", "pipkin-mcp");
        assert!(matches!(store.read("x"), Err(StoreError::Io(_))));
        assert!(matches!(store.list(), Err(StoreError::Io(_))));
        assert!(SecretToolStore::available(Some(Path::new("/nonexistent"))).is_none());
    }

    #[test]
    fn search_output_is_parsed_per_item() {
        let out = "[/org/freedesktop/secrets/collection/login/5]\nlabel = Pipkin pipkin-mcp docs\nsecret = {\"type\":\"oauth\"}\ncreated = 2026-10-12\nattribute.pipkin-provider = docs\nattribute.pipkin-store = pipkin-mcp\n\n[/org/freedesktop/secrets/collection/login/6]\nsecret = x\nattribute.pipkin-provider = two\n";
        assert_eq!(
            parse_search(out),
            vec![
                ("docs".to_owned(), "{\"type\":\"oauth\"}".to_owned()),
                ("two".to_owned(), "x".to_owned())
            ]
        );
    }
}
