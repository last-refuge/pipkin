//! Binary MCP content (images in tool results, blob resources) retained as files the person
//! and the model can reach by path, instead of being dropped or inlined as base64. Files
//! live under `<data_dir>/mcp-artifacts/<server>/<sha256 prefix>.<ext>`, named by content so
//! a repeated result is one file. Oversized content is reported, not stored.

use std::path::{Path, PathBuf};

use base64::Engine as _;

/// Largest retained artifact.
pub const MAX_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Retained {
    pub path: PathBuf,
    pub bytes: usize,
    pub sha256: String,
}

fn extension(mime: &str) -> &'static str {
    match mime.split(';').next().unwrap_or_default().trim() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "application/pdf" => "pdf",
        "application/json" => "json",
        "text/plain" => "txt",
        _ => "bin",
    }
}

/// A server name is already validated (letters, digits, `-`, `_`, `.`); anything else is
/// replaced so the path stays inside the artifacts directory.
fn safe_dir(server: &str) -> String {
    server
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => c,
            _ => '_',
        })
        .collect()
}

/// Decode `base64` and keep it under `data_dir` for `server`. A payload over
/// [`MAX_ARTIFACT_BYTES`] or undecodable base64 is an error naming the reason, never a
/// partial file.
pub fn retain(data_dir: &Path, server: &str, mime: &str, base64: &str) -> Result<Retained, String> {
    // Four base64 characters carry three bytes: refuse before decoding what cannot fit.
    if base64.len() / 4 * 3 > MAX_ARTIFACT_BYTES + 3 {
        return Err(format!(
            "content of about {} bytes exceeds the {} byte artifact limit",
            base64.len() / 4 * 3,
            MAX_ARTIFACT_BYTES
        ));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64.trim())
        .map_err(|e| format!("content is not valid base64: {e}"))?;
    if bytes.len() > MAX_ARTIFACT_BYTES {
        return Err(format!(
            "content of {} bytes exceeds the {} byte artifact limit",
            bytes.len(),
            MAX_ARTIFACT_BYTES
        ));
    }
    let sha256 = crate::web::sha256_hex(&bytes);
    let dir = data_dir.join("mcp-artifacts").join(safe_dir(server));
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let path = dir.join(format!("{}.{}", &sha256[..16], extension(mime)));
    if !path.exists() {
        let tmp = dir.join(format!(".{}.{}.tmp", &sha256[..16], std::process::id()));
        std::fs::write(&tmp, &bytes).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .map_err(|e| format!("cannot place {}: {e}", path.display()))?;
    }
    Ok(Retained {
        path,
        bytes: bytes.len(),
        sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_is_decoded_named_by_hash_and_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let png = "iVBORw0KGgoAAAANSUhEUg==";
        let one = retain(dir.path(), "fx", "image/png", png).unwrap();
        assert_eq!(one.bytes, 16);
        assert!(
            one.path
                .ends_with(format!("mcp-artifacts/fx/{}.png", &one.sha256[..16]))
        );
        assert_eq!(std::fs::read(&one.path).unwrap().len(), 16);
        let again = retain(dir.path(), "fx", "image/png", png).unwrap();
        assert_eq!(again, one, "the same content is one file");
        let other = retain(dir.path(), "../x", "application/octet-stream", "AAAA").unwrap();
        assert!(other.path.to_string_lossy().contains("mcp-artifacts/___x/"));
        assert!(other.path.extension().is_some_and(|e| e == "bin"));
        assert!(retain(dir.path(), "fx", "image/png", "not base64!").is_err());
        let huge = "A".repeat((MAX_ARTIFACT_BYTES / 3 + 2) * 4);
        assert!(
            retain(dir.path(), "fx", "image/png", &huge)
                .unwrap_err()
                .contains("exceeds")
        );
    }
}
