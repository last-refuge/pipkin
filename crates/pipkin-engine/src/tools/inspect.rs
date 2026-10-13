//! `list_files` and `search_files`: bounded, read-only inspection of the session's directory
//! for reviewers and auditors (and anyone else). Both respect `.gitignore`, skip hidden
//! entries and binaries, never follow symlinks out of the directory, and cap their output so
//! a large tree cannot flood the model context. They execute nothing.

use std::path::{Path, PathBuf};

use pipkin_engine_protocol::Block;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{Tool, ToolCtx, ToolError, ToolOutput};
use crate::provider::{BoxFuture, ToolSpec};

pub const LIST_FILES: &str = "list_files";
pub const SEARCH_FILES: &str = "search_files";

/// Most entries `list_files` returns and most matches `search_files` returns.
pub const DEFAULT_MAX: usize = 200;
pub const HARD_MAX: usize = 2_000;
/// Files larger than this are not searched.
pub const SEARCH_FILE_MAX_BYTES: u64 = 2 * 1024 * 1024;
/// Longest matched line shown.
pub const LINE_MAX_CHARS: usize = 400;
/// Most files walked by one call, as a time bound.
pub const WALK_MAX_ENTRIES: usize = 50_000;

pub const LIST_DESCRIPTION: &str = "List files under a directory of the project (relative to it), respecting .gitignore and skipping hidden entries. Returns up to 200 paths (max 2000) sorted, with a note when more exist. Read-only.";
pub const SEARCH_DESCRIPTION: &str = "Search file contents under the project (or a subdirectory) for a regular expression, respecting .gitignore and skipping hidden and binary files. Returns up to 200 matches (max 2000) as path:line: text. Read-only.";

/// Resolve `path` inside `cwd`; a path escaping the directory is refused.
fn resolve_inside(cwd: &Path, path: &str) -> Result<PathBuf, ToolError> {
    let path = super::path::normalize_tool_path(path);
    let candidate = if path.trim().is_empty() || path == "." {
        cwd.to_path_buf()
    } else {
        let p = Path::new(&path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            cwd.join(p)
        }
    };
    let canonical_root = cwd
        .canonicalize()
        .map_err(|e| ToolError::new(format!("cannot resolve the project directory: {e}")))?;
    let canonical = candidate
        .canonicalize()
        .map_err(|_| ToolError::new(format!("{path}: no such directory in the project")))?;
    if !canonical.starts_with(&canonical_root) {
        return Err(ToolError::new(format!(
            "{path} is outside the project directory"
        )));
    }
    if !canonical.is_dir() {
        return Err(ToolError::new(format!("{path} is not a directory")));
    }
    Ok(canonical)
}

fn walker(root: &Path) -> ignore::Walk {
    ignore::WalkBuilder::new(root)
        .hidden(true)
        // A project's `.gitignore` applies whether or not the directory is a repository.
        .require_git(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .follow_links(false)
        .sort_by_file_path(|a, b| a.cmp(b))
        .build()
}

fn relative(root: &Path, path: &Path, cwd_root: &Path) -> String {
    path.strip_prefix(cwd_root)
        .or_else(|_| path.strip_prefix(root))
        .unwrap_or(path)
        .display()
        .to_string()
}

fn max_of(args: &Value) -> usize {
    args.get("max")
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .unwrap_or(DEFAULT_MAX)
        .clamp(1, HARD_MAX)
}

pub struct ListFilesTool;

impl Tool for ListFilesTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            constrained_sampling: None,
            name: LIST_FILES.into(),
            description: LIST_DESCRIPTION.into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Directory relative to the project (default: the project root)"},
                    "max": {"type": "integer", "description": "Most entries to return (default 200, at most 2000)"}
                }
            }),
        }
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolCtx,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let cwd = ctx.cwd.clone();
            let cancel = ctx.cancel.clone();
            let path = args
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let max = max_of(&args);
            tokio::task::spawn_blocking(move || list_files(&cwd, &path, max, &cancel))
                .await
                .map_err(|e| ToolError::new(e.to_string()))?
        })
    }
}

pub fn list_files(
    cwd: &Path,
    path: &str,
    max: usize,
    cancel: &CancellationToken,
) -> Result<ToolOutput, ToolError> {
    let root = resolve_inside(cwd, path)?;
    let cwd_root = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let mut entries = Vec::new();
    let mut more = false;
    let mut walked = 0usize;
    for entry in walker(&root).flatten() {
        if cancel.is_cancelled() {
            return Err(ToolError::new("aborted"));
        }
        walked += 1;
        if walked > WALK_MAX_ENTRIES {
            more = true;
            break;
        }
        if entry.depth() == 0 {
            continue;
        }
        let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
        let mut rel = relative(&root, entry.path(), &cwd_root);
        if is_dir {
            rel.push('/');
        }
        if entries.len() >= max {
            more = true;
            break;
        }
        entries.push(rel);
    }
    let mut text = if entries.is_empty() {
        "(no files)".to_owned()
    } else {
        entries.join("\n")
    };
    if more {
        text.push_str(&format!(
            "\n… more entries exist; narrow the path or raise max (at most {HARD_MAX})"
        ));
    }
    Ok(ToolOutput {
        content: Some(vec![Block::text(text)]),
        details: Some(json!({ "listFiles": { "count": entries.len(), "more": more } })),
        ..ToolOutput::default()
    })
}

pub struct SearchFilesTool;

impl Tool for SearchFilesTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            constrained_sampling: None,
            name: SEARCH_FILES.into(),
            description: SEARCH_DESCRIPTION.into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "A regular expression (Rust syntax)"},
                    "path": {"type": "string", "description": "Directory relative to the project (default: the project root)"},
                    "case_insensitive": {"type": "boolean"},
                    "max": {"type": "integer", "description": "Most matches to return (default 200, at most 2000)"}
                },
                "required": ["pattern"]
            }),
        }
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolCtx,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let cwd = ctx.cwd.clone();
            let cancel = ctx.cancel.clone();
            let pattern = args
                .get("pattern")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let path = args
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let ci = args
                .get("case_insensitive")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let max = max_of(&args);
            tokio::task::spawn_blocking(move || {
                search_files(&cwd, &pattern, &path, ci, max, &cancel)
            })
            .await
            .map_err(|e| ToolError::new(e.to_string()))?
        })
    }
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(8192)].contains(&0)
}

pub fn search_files(
    cwd: &Path,
    pattern: &str,
    path: &str,
    case_insensitive: bool,
    max: usize,
    cancel: &CancellationToken,
) -> Result<ToolOutput, ToolError> {
    if pattern.trim().is_empty() {
        return Err(ToolError::new("pattern must not be empty"));
    }
    let regex = regex::RegexBuilder::new(pattern)
        .case_insensitive(case_insensitive)
        .size_limit(1 << 20)
        .build()
        .map_err(|e| ToolError::new(format!("invalid pattern: {e}")))?;
    let root = resolve_inside(cwd, path)?;
    let cwd_root = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let mut lines = Vec::new();
    let mut matches = 0usize;
    let mut files_searched = 0usize;
    let mut more = false;
    let mut walked = 0usize;
    'files: for entry in walker(&root).flatten() {
        if cancel.is_cancelled() {
            return Err(ToolError::new("aborted"));
        }
        walked += 1;
        if walked > WALK_MAX_ENTRIES {
            more = true;
            break;
        }
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if meta.len() > SEARCH_FILE_MAX_BYTES {
            continue;
        }
        let Ok(bytes) = std::fs::read(entry.path()) else {
            continue;
        };
        if is_binary(&bytes) {
            continue;
        }
        files_searched += 1;
        let text = String::from_utf8_lossy(&bytes);
        let rel = relative(&root, entry.path(), &cwd_root);
        for (i, line) in text.lines().enumerate() {
            if regex.is_match(line) {
                if matches >= max {
                    more = true;
                    break 'files;
                }
                matches += 1;
                let shown: String = line.trim_end().chars().take(LINE_MAX_CHARS).collect();
                lines.push(format!("{rel}:{}: {shown}", i + 1));
            }
        }
    }
    let mut text = if lines.is_empty() {
        format!("No matches in {files_searched} file(s).")
    } else {
        lines.join("\n")
    };
    if more {
        text.push_str(&format!(
            "\n… more matches exist; narrow the pattern or path, or raise max (at most {HARD_MAX})"
        ));
    }
    Ok(ToolOutput {
        content: Some(vec![Block::text(text)]),
        details: Some(
            json!({ "searchFiles": { "matches": matches, "filesSearched": files_searched, "more": more } }),
        ),
        ..ToolOutput::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::tests::output_text;

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::create_dir_all(root.join(".hidden")).unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(
            root.join("src/main.rs"),
            "fn main() {\n    println!(\"Hello\");\n}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/deep/lib.rs"),
            "pub fn hello() {}\n// hello again\n",
        )
        .unwrap();
        std::fs::write(root.join("target/out.rs"), "fn hello() {}").unwrap();
        std::fs::write(root.join(".hidden/secret.rs"), "fn hello() {}").unwrap();
        std::fs::write(root.join("blob.bin"), b"hel\0lo").unwrap();
        dir
    }

    #[test]
    fn listing_respects_ignores_hides_dotfiles_and_bounds_output() {
        let dir = project();
        let cancel = CancellationToken::new();
        let out = list_files(dir.path(), "", 100, &cancel).unwrap();
        let text = output_text(&out);
        assert!(
            text.contains("src/main.rs") && text.contains("src/deep/lib.rs"),
            "{text}"
        );
        assert!(text.contains("src/") && text.contains("blob.bin"), "{text}");
        assert!(
            !text.contains("target/out.rs") && !text.contains(".hidden"),
            "{text}"
        );
        let out = list_files(dir.path(), "src", 1, &cancel).unwrap();
        let text = output_text(&out);
        assert!(text.contains("more entries exist"), "{text}");
        assert_eq!(out.details.unwrap()["listFiles"]["more"], true);
        assert!(list_files(dir.path(), "../", 10, &cancel).is_err());
        assert!(list_files(dir.path(), "nope", 10, &cancel).is_err());
        assert!(list_files(dir.path(), "src/main.rs", 10, &cancel).is_err());
    }

    #[test]
    fn searching_skips_binaries_and_ignored_files_and_bounds_matches() {
        let dir = project();
        let cancel = CancellationToken::new();
        let out = search_files(dir.path(), "hello", "", true, 100, &cancel).unwrap();
        let text = output_text(&out);
        assert!(text.contains("src/main.rs:2:"), "{text}");
        assert!(
            text.contains("src/deep/lib.rs:1: pub fn hello() {}"),
            "{text}"
        );
        assert!(text.contains("src/deep/lib.rs:2:"), "{text}");
        assert!(
            !text.contains("target/") && !text.contains("blob.bin") && !text.contains(".hidden"),
            "{text}"
        );
        let out = search_files(dir.path(), "hello", "src", false, 1, &cancel).unwrap();
        let text = output_text(&out);
        assert!(text.contains("more matches exist"), "{text}");
        let out = search_files(dir.path(), "Hello", "", false, 10, &cancel).unwrap();
        assert_eq!(
            out.details.unwrap()["searchFiles"]["matches"],
            1,
            "case-sensitive by default"
        );
        let out = search_files(dir.path(), "zzz", "", false, 10, &cancel).unwrap();
        assert!(output_text(&out).starts_with("No matches in"));
        assert!(search_files(dir.path(), "(", "", false, 10, &cancel).is_err());
        assert!(search_files(dir.path(), "", "", false, 10, &cancel).is_err());
        cancel.cancel();
        assert!(search_files(dir.path(), "hello", "", false, 10, &cancel).is_err());
    }
}
