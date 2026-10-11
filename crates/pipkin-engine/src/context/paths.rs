//! Path helpers with the reference's semantics (`coding-agent/src/utils/paths.ts`): Node's
//! lexical `path.resolve`, `~` expansion, `file://` URLs, and `realpath` with a raw fallback.

use std::path::{Component, Path, PathBuf};

/// Node `path.resolve(path)`: absolute against the process directory, then `.`/`..` folded
/// lexically. Symlinks are not resolved.
pub fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        normalize(path)
    } else {
        let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        normalize(&base.join(path))
    }
}

/// Lexically fold `.` and `..` components (`..` at the root stays at the root).
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() && !out.has_root() {
                    out.push("..");
                }
            }
            Component::Normal(part) => out.push(part),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// `resolvePath(input, base, {trim})`: optional trim, `~`/`~/` expansion, `file://` URLs, then
/// absolute against `base`.
pub fn resolve(input: &str, base: &Path, trim: bool) -> PathBuf {
    let text = if trim { input.trim() } else { input };
    let expanded = expand(text);
    if expanded.is_absolute() {
        normalize(&expanded)
    } else {
        normalize(&absolute(base).join(expanded))
    }
}

fn expand(text: &str) -> PathBuf {
    if let Some(home) = std::env::home_dir() {
        if text == "~" {
            return home;
        }
        if let Some(rest) = text.strip_prefix("~/") {
            return home.join(rest);
        }
    }
    if text.starts_with("file://")
        && let Some(path) = url::Url::parse(text)
            .ok()
            .and_then(|url| url.to_file_path().ok())
    {
        return path;
    }
    PathBuf::from(text)
}

/// `canonicalizePath`: `realpath`, or the path unchanged when that fails.
pub fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The file name as a string (lossy), empty for none.
pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Directory entries sorted by name bytes, like Node's `readdirSync` (libuv sorts scandir
/// results with `strcmp`). Unreadable directories yield `None`.
pub fn sorted_entries(dir: &Path) -> Option<Vec<std::fs::DirEntry>> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .collect();
    entries.sort_by(|a, b| {
        a.file_name()
            .as_encoded_bytes()
            .cmp(b.file_name().as_encoded_bytes())
    });
    Some(entries)
}

/// `(is_dir, is_file)` of a directory entry, following a symlink (a broken one yields `None`).
pub fn entry_kind(entry: &std::fs::DirEntry) -> Option<(bool, bool)> {
    let file_type = entry.file_type().ok()?;
    if file_type.is_symlink() {
        let meta = std::fs::metadata(entry.path()).ok()?;
        Some((meta.is_dir(), meta.is_file()))
    } else {
        Some((file_type.is_dir(), file_type.is_file()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_folds_dots_like_node() {
        assert_eq!(normalize(Path::new("/a/./b/../c/")), PathBuf::from("/a/c"));
        assert_eq!(normalize(Path::new("/../x")), PathBuf::from("/x"));
        assert_eq!(
            resolve("  ./x/../y ", Path::new("/base"), true),
            PathBuf::from("/base/y")
        );
        assert_eq!(
            resolve("/abs", Path::new("/base"), false),
            PathBuf::from("/abs")
        );
    }

    #[test]
    fn tilde_and_file_urls_expand() {
        if let Some(home) = std::env::home_dir() {
            assert_eq!(resolve("~/s", Path::new("/base"), true), home.join("s"));
            assert_eq!(resolve("~", Path::new("/base"), true), home);
        }
        assert_eq!(
            resolve("file:///tmp/a%20b", Path::new("/base"), true),
            PathBuf::from("/tmp/a b")
        );
    }
}
