//! Preserve Pi coordination profiles as opaque, inactive migration data.
//! Extension compatibility is out of scope: package selections never activate code or
//! prevent history import. All server identities are retained, including unknown formats.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use sha2::{Digest, Sha256};

use super::ImportError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Snapshot {
    directory: PathBuf,
    files: BTreeMap<String, Vec<u8>>,
}

fn blocked(path: &Path, why: &str) -> ImportError {
    ImportError::Blocked(vec![format!(
        "Legacy plugin profile {} {why}; source profiles must be preserved before migration",
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

    pub(super) fn unchanged(&self) -> Result<(), ImportError> {
        if Self::read(&self.directory)? != *self {
            return Err(ImportError::Blocked(vec!["Legacy plugin profiles changed during import; nothing was activated. Close the source application and try again".into()]));
        }
        Ok(())
    }

    pub(super) fn manifest(&self) -> serde_json::Value {
        serde_json::json!({
            "directory": self.directory,
            "activation": "disabled",
            "preservation": "opaque-source-bytes",
            "profiles": self.files.iter().map(|(name, bytes)| serde_json::json!({
                "file": name,
                "sha256": format!("{:x}", Sha256::digest(bytes)),
                "sourceBase64": base64::engine::general_purpose::STANDARD.encode(bytes),
                "selection": serde_json::from_slice::<serde_json::Value>(bytes).ok(),
            })).collect::<Vec<_>>()
        })
    }
}
