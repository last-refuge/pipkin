//! Native equivalents of Pi's sticky server/session plugin selections. Persistence is in
//! the engine's owned SQLite store; these records never reinterpret a Pi TypeScript package.
use std::collections::HashSet;
use std::path::Path;

use crate::store::Store;
use pipkin_engine_protocol::{ErrorCode, ExtensionSpec, ProtoError};
use serde::{Deserialize, Serialize};

pub(crate) const KEY: &str = "native_extension_profile";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    version: u32,
    extensions: Vec<ExtensionSpec>,
}

fn invalid(message: impl Into<String>) -> ProtoError {
    ProtoError::new(ErrorCode::Invalid, message)
}

/// Resolve executables now, while the configuring process's cwd is known. Unlike a PATH
/// lookup on every activation, this cannot silently select a different program after restart.
pub(crate) fn normalize(
    mut extensions: Vec<ExtensionSpec>,
) -> Result<Vec<ExtensionSpec>, ProtoError> {
    let mut ids = HashSet::new();
    for spec in &mut extensions {
        let id = match spec {
            ExtensionSpec::Builtin(name) => {
                if !matches!(name.as_str(), "questions" | "greeting") {
                    return Err(invalid(format!("Unknown native extension {name:?}")));
                }
                match name.as_str() {
                    "questions" => super::examples::QUESTIONS_EXAMPLE_ID.to_owned(),
                    _ => super::examples::GREETING_EXAMPLE_ID.to_owned(),
                }
            }
            ExtensionSpec::Process { id, argv } => {
                if id.trim().is_empty() || argv.is_empty() || argv.iter().any(|s| s.contains('\0'))
                {
                    return Err(invalid(
                        "A process extension needs an id and a valid executable argv",
                    ));
                }
                let given = Path::new(&argv[0]);
                let found = if !given.is_absolute() && given.components().count() == 1 {
                    // Preserve direct argv's PATH search, but record its resolved identity.
                    std::env::var_os("PATH").and_then(|paths| {
                        std::env::split_paths(&paths)
                            .map(|dir| dir.join(given))
                            .find(|p| {
                                p.metadata().is_ok_and(|m| {
                                    #[cfg(unix)]
                                    {
                                        use std::os::unix::fs::PermissionsExt;
                                        m.is_file() && m.permissions().mode() & 0o111 != 0
                                    }
                                    #[cfg(not(unix))]
                                    {
                                        m.is_file()
                                    }
                                })
                            })
                    })
                } else {
                    Some(given.to_path_buf())
                };
                let path = found.as_deref().ok_or_else(|| {
                    invalid(format!(
                        "Extension executable {} was not found on PATH",
                        given.display()
                    ))
                })?;
                let path = std::fs::canonicalize(path).map_err(|e| {
                    invalid(format!(
                        "Cannot open extension executable {}: {e}",
                        path.display()
                    ))
                })?;
                let metadata = path
                    .metadata()
                    .map_err(|e| invalid(format!("Cannot inspect extension executable: {e}")))?;
                if !metadata.is_file() {
                    return Err(invalid("Extension executable is not a regular file"));
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if metadata.permissions().mode() & 0o111 == 0 {
                        return Err(invalid("Extension executable is not executable"));
                    }
                }
                argv[0] = path
                    .to_str()
                    .ok_or_else(|| invalid("Extension executable path is not UTF-8"))?
                    .to_owned();
                id.clone()
            }
        };
        if !ids.insert(id.clone()) {
            return Err(invalid(format!(
                "Native extension ids must be unique: {id}"
            )));
        }
    }
    Ok(extensions)
}

fn encode(extensions: &[ExtensionSpec]) -> String {
    serde_json::to_string(&Profile {
        version: 1,
        extensions: extensions.to_vec(),
    })
    .expect("serializable profile")
}
fn decode(raw: &str) -> Result<Vec<ExtensionSpec>, ProtoError> {
    let profile: Profile = serde_json::from_str(raw)
        .map_err(|e| invalid(format!("Invalid native extension profile: {e}")))?;
    if profile.version != 1 {
        return Err(ProtoError::new(
            ErrorCode::Unsupported,
            "Unsupported native extension profile version",
        ));
    }
    normalize(profile.extensions)
}

pub(crate) fn default(store: &Store) -> Result<Vec<ExtensionSpec>, ProtoError> {
    store
        .meta(KEY)?
        .map(|s| decode(&s))
        .transpose()
        .map(Option::unwrap_or_default)
}

pub(crate) fn set_default(store: &Store, extensions: &[ExtensionSpec]) -> Result<(), ProtoError> {
    let encoded = encode(extensions);
    store.tx(|tx| {
        if extensions.is_empty() {
            tx.execute("DELETE FROM meta WHERE key = ?1", [KEY])?;
        } else {
            tx.execute("INSERT INTO meta(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", rusqlite::params![KEY, encoded])?;
        }
        Ok(())
    })?;
    Ok(())
}

pub(crate) fn initialize(
    store: &Store,
    explicit: &[ExtensionSpec],
    clear: bool,
) -> Result<(), ProtoError> {
    if clear {
        if !explicit.is_empty() {
            return Err(invalid(
                "Cannot select extensions and clear the default together",
            ));
        }
        return set_default(store, &[]);
    }
    if explicit.is_empty() {
        default(store)?;
    } else {
        set_default(store, &normalize(explicit.to_vec())?)?;
    }
    Ok(())
}

/// Resolve without a write, so the runtime can retain fail-closed state if first pinning
/// cannot commit. The registry serializes this with PrepareExtensions.
pub(crate) fn resolve(
    store: &Store,
    session: &str,
) -> Result<(Vec<ExtensionSpec>, bool), ProtoError> {
    if let Some(raw) = store.kv_get(session, KEY)? {
        Ok((decode(&raw)?, true))
    } else {
        Ok((default(store)?, false))
    }
}

pub(crate) fn pin(store: &Store, session: &str) -> Result<Vec<ExtensionSpec>, ProtoError> {
    if let Some(raw) = store.kv_get(session, KEY)? {
        return decode(&raw);
    }
    let fallback = default(store)?;
    decode(&store.kv_get_or_insert(session, KEY, &encode(&fallback))?)
}

pub(crate) fn set_session(
    store: &Store,
    session: &str,
    extensions: &[ExtensionSpec],
) -> Result<(), ProtoError> {
    store.tx(|tx| write_session(tx, session, extensions))?;
    Ok(())
}

pub(crate) fn write_session(
    tx: &rusqlite::Connection,
    session: &str,
    extensions: &[ExtensionSpec],
) -> crate::store::Result<()> {
    Store::kv_set(tx, session, KEY, &encode(extensions))
}
