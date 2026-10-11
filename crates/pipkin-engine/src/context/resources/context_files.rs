//! Project instruction files (reference `coding-agent/src/core/resource-loader.ts:184-270`,
//! `loadContextFileFromDir`, `findShadowedContextFile`, `loadProjectContextFiles`; git layout
//! from `footer-data-provider.ts:16-48`, `findGitPaths`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::context::{js, paths};

/// Per-directory candidates; the first regular file wins.
pub const CONTEXT_FILE_NAMES: [&str; 5] = [
    "AGENTS.override.md",
    "AGENTS.md",
    "AGENTS.MD",
    "CLAUDE.md",
    "CLAUDE.MD",
];

/// One instruction file: its path as discovered (not canonicalized) and its text without a
/// leading BOM. No size limit, like the reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextFile {
    pub path: PathBuf,
    pub content: String,
}

/// The first candidate in `dir` that is a regular file (symlinks followed). A candidate that
/// exists but cannot be read is logged and the next one is tried.
fn context_file_in(dir: &Path) -> Option<ContextFile> {
    for name in CONTEXT_FILE_NAMES {
        let path = dir.join(name);
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        match std::fs::read(&path) {
            Ok(bytes) => {
                let text = js::decode_utf8(bytes);
                return Some(ContextFile {
                    content: js::strip_bom(&text).to_string(),
                    path,
                });
            }
            Err(err) => log::warn!("Could not read {}: {err}", path.display()),
        }
    }
    None
}

struct GitPaths {
    repo_dir: PathBuf,
    common_git_dir: PathBuf,
}

/// `findGitPaths`: the nearest `.git` (directory, or `gitdir:` file) at or above `cwd`.
fn git_paths(cwd: &Path) -> Option<GitPaths> {
    let mut dir = cwd.to_path_buf();
    loop {
        let git = dir.join(".git");
        if let Ok(meta) = std::fs::metadata(&git) {
            if meta.is_file() {
                let content = std::fs::read(&git).ok().map(js::decode_utf8)?;
                let content = js::trim(&content);
                if let Some(target) = content.strip_prefix("gitdir: ") {
                    let git_dir = paths::normalize(&dir.join(js::trim(target)));
                    if !git_dir.join("HEAD").exists() {
                        return None;
                    }
                    let common = git_dir.join("commondir");
                    let common_git_dir = if common.exists() {
                        let text = std::fs::read(&common).ok().map(js::decode_utf8)?;
                        paths::normalize(&git_dir.join(js::trim(&text)))
                    } else {
                        git_dir
                    };
                    return Some(GitPaths {
                        repo_dir: dir,
                        common_git_dir,
                    });
                }
            } else if meta.is_dir() {
                if !git.join("HEAD").exists() {
                    return None;
                }
                return Some(GitPaths {
                    repo_dir: dir,
                    common_git_dir: git,
                });
            }
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// `findShadowedContextFile`: when `cwd` is in a linked worktree nested inside its main
/// checkout, the main checkout's context file with the same name as the worktree root's own
/// file (returned with a canonical directory). Bare layouts, sibling worktrees and submodules
/// shadow nothing.
fn shadowed_context_file(cwd: &Path) -> Option<PathBuf> {
    let git = git_paths(cwd)?;
    let common_git_dir = paths::canonical(&git.common_git_dir);
    let worktree_root = paths::canonical(&git.repo_dir);
    let main_root = common_git_dir.parent()?.to_path_buf();
    // `startsWith(main + sep)`: a strict descendant (never true for a filesystem-root main,
    // whose `main + sep` is `//`).
    if main_root.parent().is_none()
        || worktree_root == main_root
        || !worktree_root.starts_with(&main_root)
    {
        return None;
    }
    if paths::canonical(&main_root.join(".git")) != common_git_dir {
        return None;
    }
    let own = context_file_in(&worktree_root)?;
    Some(main_root.join(own.path.file_name()?))
}

/// `loadProjectContextFiles`: the global file in `config_dir` first, then one file per
/// directory from the filesystem root down to `cwd` (no stop at a git root or home),
/// deduplicated by path, with the nested-worktree shadowing rule. `cwd` is made absolute
/// lexically; symlinks in it are not resolved.
pub fn load_context_files(cwd: &Path, config_dir: &Path) -> Vec<ContextFile> {
    let cwd = paths::absolute(cwd);
    let config_dir = paths::absolute(config_dir);
    let mut files = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    if let Some(global) = context_file_in(&config_dir) {
        seen.insert(global.path.clone());
        files.push(global);
    }

    let shadowed = shadowed_context_file(&cwd);
    let mut ancestors = Vec::new();
    let mut dir = cwd.clone();
    loop {
        if let Some(file) = context_file_in(&dir) {
            let is_shadowed = shadowed
                .as_ref()
                .is_some_and(|shadowed| paths::canonical(&file.path) == *shadowed);
            if !is_shadowed && !seen.contains(&file.path) {
                seen.insert(file.path.clone());
                ancestors.push(file);
            }
        }
        if !dir.pop() {
            break;
        }
    }
    ancestors.reverse();
    files.extend(ancestors);
    files
}
