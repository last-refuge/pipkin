//! The explicit import calls (`ImportDiscover`, `ImportSessions`, `ImportConfig`,
//! `ImportCredentials`): the protocol face of [`crate::import`].
//!
//! Nothing here runs unless a client asks, and nothing is ever written to the legacy profile.
//! - The agent directory is the one given, else `$PI_CODING_AGENT_DIR` from the engine's
//!   environment (a leading `~` is the home directory, as the reference expands it), else
//!   `~/.pi/agent`.
//! - Sessions are imported one by one (each in one store transaction, see
//!   [`crate::import::import_session`]); consistent copies of the sources are kept under
//!   `<data dir>/imports/<UTC timestamp>/`. One import runs at a time per engine process.
//! - Configuration files are copied into the native configuration directory, never over an
//!   existing native file.
//! - Credentials are merged into the native credential store through its transactional API:
//!   an existing native entry is never overwritten, the imported entry's JSON value is stored
//!   exactly as it was read, other entries keep their bytes (the store guarantees this), and
//!   credentials are copied only for explicitly selected providers. Values
//!   are never logged or returned.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use pipkin_engine_protocol::*;
use serde_json::Value;

use super::Shared;
use crate::auth::store::{CredentialStore, FileStore, StoredCredential};
use crate::import as legacy;

/// What a reserved credential's import reports.
pub const RESERVED_KEPT: &str =
    "kept in the Pi profile for a later integration; not imported as usable here";

/// One import (of any kind) at a time, so two clients cannot race on the same source.
static IMPORTS: Mutex<()> = Mutex::new(());

fn invalid(message: impl Into<String>) -> ProtoError {
    ProtoError::new(ErrorCode::Invalid, message)
}

fn internal(message: impl Into<String>) -> ProtoError {
    ProtoError::new(ErrorCode::Internal, message)
}

/// Run blocking import work off the async workers, holding the import lock.
async fn blocking<T: Send + 'static>(
    shared: &Arc<Shared>,
    work: impl FnOnce(&Shared) -> Result<T, ProtoError> + Send + 'static,
) -> Result<T, ProtoError> {
    let shared = shared.clone();
    tokio::task::spawn_blocking(move || {
        let _guard = IMPORTS.lock().unwrap_or_else(|e| e.into_inner());
        work(&shared)
    })
    .await
    .map_err(|e| internal(format!("the import task failed: {e}")))?
}

/// The legacy agent directory: the one given, else `$PI_CODING_AGENT_DIR`, else `~/.pi/agent`.
pub fn agent_dir(env: &crate::config::Env, given: Option<String>) -> Result<PathBuf, ProtoError> {
    let home = || {
        env.get("HOME").map(PathBuf::from).ok_or_else(|| {
            invalid("The Pi agent directory is unknown: set PI_CODING_AGENT_DIR or HOME")
        })
    };
    let raw = given
        .filter(|d| !d.trim().is_empty())
        .or_else(|| env.get("PI_CODING_AGENT_DIR"));
    let path = match raw {
        None => home()?.join(".pi").join("agent"),
        Some(raw) if raw == "~" => home()?,
        Some(raw) => match raw.strip_prefix("~/") {
            Some(rest) => home()?.join(rest),
            None => PathBuf::from(raw),
        },
    };
    if !path.is_absolute() {
        return Err(invalid(format!(
            "The Pi agent directory must be an absolute path, not {}",
            path.display()
        )));
    }
    Ok(path)
}

/// Pi stores facet selections beside its server sockets, not in the agent directory.
pub fn server_dir(env: &crate::config::Env, given: Option<String>) -> Result<PathBuf, ProtoError> {
    let raw = given.or_else(|| env.get("PI_SERVER_DIR"));
    let home = || {
        env.get("HOME").map(PathBuf::from).ok_or_else(|| invalid(
        "The Pi server directory is unknown: set PI_SERVER_DIR or supply serverDir before importing conversations"
    ))
    };
    let path = match raw.as_deref() {
        None => home()?.join(".pi/server"),
        Some("~") => home()?,
        Some(raw) if raw.starts_with("~/") => home()?.join(&raw[2..]),
        Some(raw) => PathBuf::from(raw),
    };
    if !path.is_absolute() {
        return Err(invalid("The Pi server directory must be an absolute path"));
    }
    Ok(path)
}

/// Unique, ordered, without empties.
fn dedupe(items: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    items
        .into_iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty() && seen.insert(s.clone()))
        .collect()
}

/// `YYYYMMDDTHHMMSS.mmmZ` (UTC) for a backup directory name.
pub fn utc_stamp(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}.{millis:03}Z",
        rem / 3600,
        (rem / 60) % 60,
        rem % 60
    )
}

fn native_providers(shared: &Shared) -> BTreeSet<String> {
    FileStore::new(shared.config.auth_path())
        .list()
        .map(|l| l.into_iter().map(|c| c.provider).collect())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------- discover

pub async fn discover(
    shared: &Arc<Shared>,
    agent: Option<String>,
    server: Option<String>,
) -> Result<Reply, ProtoError> {
    let dir = agent_dir(&shared.config.env, agent)?;
    let server = server_dir(&shared.config.env, server)?;
    blocking(shared, move |shared| {
        Ok(Reply::ImportReport(report(shared, &dir, &server)))
    })
    .await
}

fn report(shared: &Shared, dir: &Path, server: &Path) -> ImportReport {
    let found = legacy::discover(dir, server, &shared.store);
    let native = native_providers(shared);
    ImportReport {
        agent_dir: dir.display().to_string(),
        sessions: found
            .sessions
            .into_iter()
            .map(|s| ImportSessionView {
                status: match s.status {
                    legacy::SessionStatus::Importable => ImportSessionStatus::Importable,
                    legacy::SessionStatus::AlreadyImported { native } => {
                        ImportSessionStatus::AlreadyImported { native }
                    }
                    legacy::SessionStatus::Locked => {
                        ImportSessionStatus::Locked { detail: s.detail }
                    }
                    legacy::SessionStatus::Blocked { reasons } => {
                        ImportSessionStatus::Blocked { reasons }
                    }
                },
                id: s.id,
                cwd: s.cwd,
                created_at_ms: s.created_at_ms,
            })
            .collect(),
        config: found.config.into_iter().map(|c| c.name).collect(),
        credentials: found
            .credentials
            .into_iter()
            .map(|c| ImportCredentialView {
                native_exists: native.contains(&c.provider),
                reserved: c.reserved.map(|_| RESERVED_KEPT.to_owned()),
                provider: c.provider,
                kind: c.kind,
            })
            .collect(),
        credentials_error: found.credentials_error,
    }
}

// ---------------------------------------------------------------------------- sessions

pub async fn sessions(
    shared: &Arc<Shared>,
    agent: Option<String>,
    server: Option<String>,
    ids: Vec<String>,
) -> Result<Reply, ProtoError> {
    let dir = agent_dir(&shared.config.env, agent)?;
    let server = server_dir(&shared.config.env, server)?;
    let ids = dedupe(ids);
    if ids.len() > 10_000 {
        return Err(ProtoError::new(
            ErrorCode::TooLarge,
            "Import at most 10000 sessions at a time",
        ));
    }
    let view = blocking(shared, move |shared| {
        Ok(import_sessions(shared, &dir, &server, &ids))
    })
    .await?;
    if view
        .sessions
        .iter()
        .any(|s| matches!(s.outcome, ImportSessionOutcome::Imported { .. }))
    {
        shared.publish(Event::SessionsChanged);
    }
    Ok(Reply::Imported(view))
}

fn count(manifest: &Value, key: &str) -> u64 {
    manifest["counts"][key].as_u64().unwrap_or(0)
}

fn import_sessions(shared: &Shared, dir: &Path, server: &Path, ids: &[String]) -> ImportedView {
    let backup = shared.config.data_dir.join("imports").join(format!(
        "{}-{}",
        utc_stamp(crate::util::now_ms()),
        crate::util::random_hex(2)
    ));
    let sessions = ids
        .iter()
        .map(|id| {
            let outcome = match legacy::import_session(&shared.store, dir, id, server, &backup) {
                Ok(o) if o.already_imported => ImportSessionOutcome::AlreadyImported {
                    native: o.native_session,
                },
                Ok(o) => {
                    log::info!(
                        "imported legacy session {id} as {} ({} children)",
                        o.native_session,
                        o.children.len()
                    );
                    ImportSessionOutcome::Imported {
                        children: o.children.len() as u32,
                        entries: count(&o.manifest, "entriesTotal"),
                        requests: count(&o.manifest, "requests"),
                        native: o.native_session,
                    }
                }
                Err(legacy::ImportError::Locked(detail)) => ImportSessionOutcome::Locked { detail },
                Err(legacy::ImportError::Blocked(reasons)) => {
                    ImportSessionOutcome::Blocked { reasons }
                }
                Err(e) => {
                    log::warn!("legacy session {id} was not imported: {e}");
                    ImportSessionOutcome::Failed {
                        detail: e.to_string(),
                    }
                }
            };
            ImportSessionResult {
                source: id.clone(),
                outcome,
            }
        })
        .collect();
    ImportedView {
        sessions,
        backup_dir: backup.is_dir().then(|| backup.display().to_string()),
    }
}

// ---------------------------------------------------------------------------- configuration

pub async fn config(
    shared: &Arc<Shared>,
    agent: Option<String>,
    items: Vec<String>,
) -> Result<Reply, ProtoError> {
    let dir = agent_dir(&shared.config.env, agent)?;
    let items = dedupe(items);
    let view = blocking(shared, move |shared| {
        Ok(import_config(shared, &dir, &items))
    })
    .await?;
    if view.items.iter().any(|i| {
        matches!(
            i.outcome,
            ConfigItemOutcome::Copied { .. } | ConfigItemOutcome::Partial { .. }
        )
    }) {
        let shared = shared.clone();
        tokio::task::spawn_blocking(move || {
            shared.auth.reload();
            shared.publish(Event::ModelsChanged);
        })
        .await
        .map_err(|e| internal(e.to_string()))?;
    }
    Ok(Reply::ConfigImported(view))
}

/// Copy `source` to `dest` unless `dest` exists. Returns whether it was copied.
fn copy_new(source: &Path, dest: &Path) -> std::io::Result<bool> {
    if std::fs::symlink_metadata(dest).is_ok() {
        return Ok(false);
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut input = std::fs::File::open(source)?;
    // `create_new` refuses a file that appeared meanwhile instead of replacing it.
    let mut output = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(e),
    };
    let written = std::io::copy(&mut input, &mut output).and_then(|_| output.sync_all());
    if let Err(e) = written {
        drop(output);
        let _ = std::fs::remove_file(dest);
        return Err(e);
    }
    Ok(true)
}

fn import_config(shared: &Shared, dir: &Path, items: &[String]) -> ConfigImportView {
    let mut by_item: HashMap<String, Vec<(PathBuf, PathBuf)>> = HashMap::new();
    for (source, rel) in legacy::config_files(dir) {
        let Some(first) = rel.components().next() else {
            continue;
        };
        by_item
            .entry(first.as_os_str().to_string_lossy().into_owned())
            .or_default()
            .push((source, rel));
    }
    let items = items
        .iter()
        .map(|name| {
            let outcome = match by_item.get(name) {
                None => ConfigItemOutcome::NotFound,
                Some(files) => {
                    let (mut copied, mut kept) = (0u32, 0u32);
                    let mut failed = None;
                    for (source, rel) in files {
                        match copy_new(source, &shared.config.config_dir.join(rel)) {
                            Ok(true) => copied += 1,
                            Ok(false) => kept += 1,
                            Err(e) => {
                                failed = Some(format!("{}: {e}", rel.display()));
                                break;
                            }
                        }
                    }
                    match (failed, copied, kept) {
                        (Some(detail), _, _) => ConfigItemOutcome::Failed { detail },
                        (None, 0, _) => ConfigItemOutcome::KeptExisting,
                        (None, files, 0) => ConfigItemOutcome::Copied { files },
                        (None, files, kept) => ConfigItemOutcome::Partial { files, kept },
                    }
                }
            };
            ConfigItemResult {
                name: name.clone(),
                outcome,
            }
        })
        .collect();
    ConfigImportView { items }
}

// ---------------------------------------------------------------------------- credentials

pub async fn credentials(
    shared: &Arc<Shared>,
    agent: Option<String>,
    providers: Vec<String>,
) -> Result<Reply, ProtoError> {
    let dir = agent_dir(&shared.config.env, agent)?;
    let providers = dedupe(providers);
    let view = blocking(shared, move |shared| {
        import_credentials(shared, &dir, &providers)
    })
    .await?;
    if view
        .providers
        .iter()
        .any(|p| p.outcome == CredentialImportOutcome::Imported)
    {
        let shared = shared.clone();
        tokio::task::spawn_blocking(move || {
            shared.auth.reload();
            shared.publish(Event::ModelsChanged);
        })
        .await
        .map_err(|e| internal(e.to_string()))?;
    }
    Ok(Reply::CredentialsImported(view))
}

fn import_credentials(
    shared: &Shared,
    dir: &Path,
    providers: &[String],
) -> Result<CredentialsImportView, ProtoError> {
    let entries = legacy::credential_entries(dir)
        .map_err(|why| invalid(format!("The Pi credentials could not be read: {why}")))?;
    let store = FileStore::new(shared.config.auth_path());
    let mut txn = store
        .begin()
        .map_err(|e| internal(format!("The credential store could not be opened: {e}")))?;
    let mut results = Vec::new();
    let mut changed = false;
    for provider in providers {
        let outcome = if let Some(value) = entries.get(provider) {
            if txn.get(provider).is_some() {
                CredentialImportOutcome::KeptExisting
            } else {
                // Stored as the exact JSON value read (not re-shaped through a typed form),
                // so the entry is preserved as it was; readers interpret it as usual.
                txn.set(provider, StoredCredential::Unsupported(value.clone()));
                changed = true;
                CredentialImportOutcome::Imported
            }
        } else {
            CredentialImportOutcome::NotFound
        };
        results.push(CredentialImportResult {
            provider: provider.clone(),
            outcome,
        });
    }
    if changed {
        txn.commit()
            .map_err(|e| internal(format!("The credentials could not be saved: {e}")))?;
        let imported: Vec<&str> = results
            .iter()
            .filter(|r| r.outcome == CredentialImportOutcome::Imported)
            .map(|r| r.provider.as_str())
            .collect();
        log::info!("imported saved credentials for {}", imported.join(", "));
    }
    Ok(CredentialsImportView { providers: results })
}

#[cfg(test)]
#[path = "import_tests.rs"]
mod tests;
