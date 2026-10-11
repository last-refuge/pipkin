//! Path handling shared by the file tools (`tools/path-utils.ts`, `env/node.ts` `resolvePath`)
//! and Node-style error texts, so messages match what the reference shows the model.

use std::io;
use std::path::{Component, Path, PathBuf};

use unicode_normalization::UnicodeNormalization;

/// Unicode spaces become ASCII spaces and one leading `@` is dropped.
pub fn normalize_tool_path(path: &str) -> String {
    let normalized: String = path
        .chars()
        .map(|c| match c {
            '\u{a0}' | '\u{2000}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}' => ' ',
            c => c,
        })
        .collect();
    match normalized.strip_prefix('@') {
        Some(rest) => rest.to_owned(),
        None => normalized,
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .or_else(std::env::home_dir)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// `fileURLToPath` for POSIX: `file:` scheme, empty (or `localhost`) host, percent-decoded path
/// without an encoded `/`. `None` keeps the text as an ordinary path, like the reference.
fn file_url_to_path(text: &str) -> Option<PathBuf> {
    let url = url::Url::parse(text).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    if url
        .host_str()
        .is_some_and(|h| !h.is_empty() && h != "localhost")
    {
        return None;
    }
    let path = url.path();
    let lower = path.to_ascii_lowercase();
    if lower.contains("%2f") {
        return None;
    }
    let bytes = percent_decode(path.as_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
    }
    #[cfg(not(unix))]
    {
        Some(PathBuf::from(String::from_utf8_lossy(&bytes).into_owned()))
    }
}

fn percent_decode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%'
            && i + 2 < input.len()
            && let (Some(h), Some(l)) = (hex(input[i + 1]), hex(input[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(input[i]);
        i += 1;
    }
    out
}

/// `path.resolve(cwd, p)`: lexical, `.`/`..` collapsed, no trailing separator.
pub fn resolve_lexical(cwd: &Path, path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(name) => out.push(name),
        }
    }
    if out.as_os_str().is_empty() {
        out.push("/");
    }
    out
}

/// `env.absolutePath`: `~`, `~/`, `file://` URLs, then resolved against `cwd`.
pub fn absolute_path(cwd: &Path, path: &str) -> PathBuf {
    let candidate = if path == "~" {
        home_dir()
    } else if let Some(rest) = path.strip_prefix("~/") {
        home_dir().join(rest)
    } else if path.starts_with("file://") {
        file_url_to_path(path).unwrap_or_else(|| PathBuf::from(path))
    } else {
        PathBuf::from(path)
    };
    resolve_lexical(cwd, &candidate)
}

/// `resolveToolPath`.
pub fn resolve_tool_path(cwd: &Path, path: &str) -> PathBuf {
    absolute_path(cwd, &normalize_tool_path(path))
}

/// `resolveReadToolPath`: when the path does not exist, try the macOS screenshot (` AM.` /
/// ` PM.` with U+202F), NFD, and curly-apostrophe variants. An lstat error other than "not
/// found" is an error, with Node's text.
pub fn resolve_read_tool_path(cwd: &Path, path: &str) -> Result<PathBuf, String> {
    let resolved = resolve_tool_path(cwd, path);
    let text = resolved.to_string_lossy().into_owned();
    let am_pm = regex::Regex::new(r"(?i) (AM|PM)\.").expect("valid regex");
    let nfd: String = text.nfd().collect();
    let variants = [
        text.clone(),
        am_pm.replace_all(&text, "\u{202f}$1.").into_owned(),
        nfd.clone(),
        text.replace('\'', "\u{2019}"),
        nfd.replace('\'', "\u{2019}"),
    ];
    let mut seen: Vec<&str> = Vec::new();
    for variant in &variants {
        if seen.contains(&variant.as_str()) {
            continue;
        }
        seen.push(variant);
        // The first variant keeps the exact (possibly non-UTF-8) path.
        let candidate = if seen.len() == 1 {
            resolved.clone()
        } else {
            PathBuf::from(variant)
        };
        match std::fs::symlink_metadata(&candidate) {
            Ok(_) => return Ok(candidate),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(node_error(&e, "lstat", &candidate)),
        }
    }
    Ok(resolved)
}

/// The reference `FileError` code of an I/O error.
pub fn file_error_code(error: &io::Error) -> &'static str {
    #[cfg(unix)]
    let errno = error.raw_os_error();
    #[cfg(not(unix))]
    let errno: Option<i32> = None;
    match errno {
        Some(libc::ENOENT) => "not_found",
        Some(libc::EACCES) | Some(libc::EPERM) => "permission_denied",
        Some(libc::ENOTDIR) => "not_directory",
        Some(libc::EISDIR) => "is_directory",
        Some(libc::EINVAL) => "invalid",
        _ => match error.kind() {
            io::ErrorKind::NotFound => "not_found",
            io::ErrorKind::PermissionDenied => "permission_denied",
            io::ErrorKind::InvalidInput => "invalid",
            _ => "unknown",
        },
    }
}

#[cfg(not(unix))]
fn errno_text(_errno: i32) -> Option<(&'static str, &'static str)> {
    None
}

#[cfg(unix)]
fn errno_text(errno: i32) -> Option<(&'static str, &'static str)> {
    Some(match errno {
        libc::ENOENT => ("ENOENT", "no such file or directory"),
        libc::EACCES => ("EACCES", "permission denied"),
        libc::EPERM => ("EPERM", "operation not permitted"),
        libc::ENOTDIR => ("ENOTDIR", "not a directory"),
        libc::EISDIR => ("EISDIR", "illegal operation on a directory"),
        libc::EEXIST => ("EEXIST", "file already exists"),
        libc::ELOOP => ("ELOOP", "too many symbolic links encountered"),
        libc::ENAMETOOLONG => ("ENAMETOOLONG", "name too long"),
        libc::EROFS => ("EROFS", "read-only file system"),
        libc::ENOSPC => ("ENOSPC", "no space left on device"),
        libc::EINVAL => ("EINVAL", "invalid argument"),
        libc::EMFILE => ("EMFILE", "too many open files"),
        libc::EBUSY => ("EBUSY", "resource busy or locked"),
        libc::ETXTBSY => ("ETXTBSY", "text file is busy"),
        libc::EIO => ("EIO", "i/o error"),
        libc::EAGAIN => ("EAGAIN", "resource temporarily unavailable"),
        libc::EBADF => ("EBADF", "bad file descriptor"),
        libc::EFBIG => ("EFBIG", "file too large"),
        libc::EXDEV => ("EXDEV", "cross-device link not permitted"),
        _ => return None,
    })
}

/// Node's error text: `<CODE>: <description>, <syscall> '<path>'`.
pub fn node_error(error: &io::Error, syscall: &str, path: &Path) -> String {
    match error.raw_os_error().and_then(errno_text) {
        Some((code, text)) => format!("{code}: {text}, {syscall} '{}'", path.display()),
        None => format!("{error}, {syscall} '{}'", path.display()),
    }
}

/// Node's error text for a failed spawn: `spawn <program> <CODE>`.
pub fn spawn_error(error: &io::Error, program: &Path) -> String {
    match error.raw_os_error().and_then(errno_text) {
        Some((code, _)) => format!("spawn {} {code}", program.display()),
        None => format!("spawn {} {error}", program.display()),
    }
}

/// The key that serializes mutations of one file: its canonical path, or for a file that does
/// not exist yet, its canonical parent joined with its name.
pub fn mutation_key(path: &Path) -> Result<PathBuf, String> {
    match std::fs::canonicalize(path) {
        Ok(canonical) => Ok(canonical),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
                return Ok(path.to_path_buf());
            };
            if parent == path {
                return Ok(path.to_path_buf());
            }
            Ok(mutation_key(parent)?.join(name))
        }
        Err(e) => Err(node_error(&e, "realpath", path)),
    }
}
