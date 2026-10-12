//! Read-only import guard for Pi's pinned `experimental/plugins/package.ts` v1 profiles.
//! All server identities in the selected coordination directory are checked: importing a
//! session must not discard a selection merely because it belonged to another server id.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::ImportError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Snapshot {
    directory: PathBuf,
    files: BTreeMap<String, Vec<u8>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Profile {
    version: u32,
    package_paths: Vec<String>,
    #[serde(default)]
    session_path: Option<String>,
}

fn blocked(path: &Path, why: &str) -> ImportError {
    ImportError::Blocked(vec![format!(
        "Pi plugin profile {} {why}; keep this conversation on Pi",
        path.display()
    )])
}

impl Snapshot {
    pub(super) fn read(directory: &Path) -> Result<Self, ImportError> {
        let mut files = BTreeMap::new();
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // A dangling symlink is unreadable, not proof of an absent profile directory.
                if std::fs::symlink_metadata(directory).is_ok() {
                    return Err(blocked(directory, "cannot be read"));
                }
                return Ok(Self {
                    directory: directory.into(),
                    files,
                });
            }
            Err(_) => return Err(blocked(directory, "cannot be read")),
        };
        for entry in entries {
            let entry = entry.map_err(|_| blocked(directory, "cannot be listed"))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return Err(blocked(directory, "contains an unreadable filename"));
            };
            if !(name.starts_with("plugin-packages-")
                || name.starts_with("session-plugin-packages-"))
                || !name.ends_with(".json")
            {
                continue;
            }
            let path = entry.path();
            // These are small configuration files, never executable plugin code.
            use std::io::Read;
            if !std::fs::metadata(&path).is_ok_and(|m| m.is_file()) {
                return Err(blocked(&path, "is not a readable regular file"));
            }
            let file = std::fs::File::open(&path).map_err(|_| blocked(&path, "cannot be read"))?;
            let mut bytes = Vec::new();
            file.take(1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| blocked(&path, "cannot be read"))?;
            if bytes.len() > 1024 * 1024 {
                return Err(blocked(&path, "exceeds the 1 MiB import inspection limit"));
            }
            files.insert(name.to_owned(), bytes);
        }
        Ok(Self {
            directory: directory.into(),
            files,
        })
    }

    /// Session selection wins over the corresponding server default, including explicit [].
    /// Other server identities still apply independently. Malformed profiles fail closed.
    pub(super) fn check(&self, session: &Path) -> Result<(), ImportError> {
        let mut defaults = BTreeMap::new();
        let mut pinned = BTreeMap::new();
        for (name, bytes) in &self.files {
            let path = self.directory.join(name);
            let profile: Profile = serde_json::from_slice(bytes)
                .map_err(|_| blocked(&path, "is invalid or has an unsupported format"))?;
            let session_profile = name.starts_with("session-plugin-packages-");
            let raw: serde_json::Value = serde_json::from_slice(bytes).expect("parsed profile");
            if !session_profile && raw.get("sessionPath").is_some() {
                return Err(blocked(&path, "has an unexpected session path"));
            }
            if profile.version != 1
                || profile
                    .package_paths
                    .iter()
                    .any(|p| p.is_empty() || !Path::new(p).is_absolute())
                || profile.package_paths.iter().collect::<BTreeSet<_>>().len()
                    != profile.package_paths.len()
                || (!session_profile
                    && (profile.session_path.is_some() || profile.package_paths.is_empty()))
            {
                return Err(blocked(&path, "has an unsupported selection"));
            }
            if session_profile {
                let Some(selected_session) = profile.session_path.as_deref() else {
                    return Err(blocked(&path, "has no session path"));
                };
                if !Path::new(selected_session).is_absolute() {
                    return Err(blocked(&path, "has a relative session path"));
                }
                let hash = format!("{:x}", Sha256::digest(selected_session.as_bytes()));
                let suffix = format!("-{}.json", &hash[..24]);
                let Some(server) = name
                    .strip_prefix("session-plugin-packages-")
                    .and_then(|s| s.strip_suffix(&suffix))
                    .filter(|s| !s.is_empty())
                else {
                    return Err(blocked(&path, "does not match its session path"));
                };
                let selected = Path::new(selected_session);
                let same = selected == session
                    || std::fs::canonicalize(selected)
                        .ok()
                        .zip(std::fs::canonicalize(session).ok())
                        .is_some_and(|(a, b)| a == b);
                // A copied/moved agent directory retains absolute paths in Pi's profiles.
                // A nonempty selection for the same session id is ambiguous, never evidence
                // that the copied conversation is plugin-free. Empty stale profiles must not
                // override a current server default.
                if !same
                    && selected.file_name() == session.file_name()
                    && !profile.package_paths.is_empty()
                {
                    return Err(blocked(
                        &path,
                        "selects plugins for this session id at another path",
                    ));
                }
                if same
                    && pinned
                        .insert(
                            server.to_owned(),
                            (path.clone(), profile.package_paths.clone()),
                        )
                        .is_some()
                {
                    return Err(blocked(
                        &path,
                        "conflicts with another profile for the same session",
                    ));
                }
            } else {
                let server = name
                    .strip_prefix("plugin-packages-")
                    .unwrap()
                    .strip_suffix(".json")
                    .unwrap();
                if server.is_empty() {
                    return Err(blocked(&path, "has no server identity"));
                }
                defaults.insert(server.to_owned(), (path, profile.package_paths));
            }
        }
        for (server, selection) in pinned {
            defaults.insert(server, selection);
        }
        let reasons: Vec<String> = defaults.values().filter(|(_, packages)| !packages.is_empty()).map(|(path, packages)| format!(
            "Pi plugin profile {} selects {} package(s) that the native engine cannot run; keep this conversation on Pi",
            path.display(), packages.len()
        )).collect();
        if reasons.is_empty() {
            Ok(())
        } else {
            Err(ImportError::Blocked(reasons))
        }
    }

    pub(super) fn unchanged(&self) -> Result<(), ImportError> {
        if Self::read(&self.directory)? != *self {
            return Err(ImportError::Blocked(vec!["Pi plugin profiles changed during import; nothing was activated. Close Pi and try again".into()]));
        }
        Ok(())
    }

    pub(super) fn manifest(&self) -> serde_json::Value {
        serde_json::json!({
            "directory": self.directory,
            "profiles": self.files.iter().map(|(name, bytes)| serde_json::json!({
                "file": name,
                "sha256": format!("{:x}", Sha256::digest(bytes)),
                "selection": serde_json::from_slice::<serde_json::Value>(bytes).expect("validated profiles"),
            })).collect::<Vec<_>>()
        })
    }
}
