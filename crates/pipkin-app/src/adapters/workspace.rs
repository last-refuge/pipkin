//! Read-only, bounded Git diffs. Session scans use literal file pathspecs so unrelated
//! project files are not collected. Both staged and unstaged changes are relative to HEAD.

use std::{io::Read, path::Path};

use pipkin_core::{DiffKind, DiffLine, FileChange, Hunk};

/// Total bytes of diff text read from `git diff`; more is cut and flagged.
const MAX_DIFF_BYTES: usize = 512 * 1024;
/// Untracked files shown, and how much of each is read.
const MAX_UNTRACKED_FILES: usize = 50;
const MAX_UNTRACKED_BYTES: usize = 64 * 1024;
/// Lines kept per file before the rest is summarized.
const MAX_LINES_PER_FILE: usize = 2000;
mod git;

#[derive(Clone, Debug, PartialEq)]
pub enum Workspace {
    /// The directory is not inside a Git repository (or Git is not installed).
    NotARepository,
    /// Git ran but could not answer.
    Unavailable(String),
    Changes {
        files: Vec<FileChange>,
        /// Output was cut to stay bounded.
        truncated: bool,
    },
}

/// The workspace's current changes, without attributing them to a tool or conversation.
/// Executable filters/attributes are deliberately not applied; see the private Git reader.
pub fn collect(dir: &Path) -> Workspace {
    collect_paths(dir, None)
}

/// Only uncommitted changes to files associated with the selected session.
pub fn collect_scoped(dir: &Path, paths: &[String]) -> Workspace {
    if paths.is_empty() {
        return Workspace::Changes {
            files: vec![],
            truncated: false,
        };
    }
    collect_paths(dir, Some(paths))
}

fn collect_paths(dir: &Path, paths: Option<&[String]>) -> Workspace {
    let git = match git::Reader::open(dir) {
        Ok(git) => git,
        Err(e) if e == "not a git repository" => return Workspace::NotARepository,
        Err(e) => return Workspace::Unavailable(e),
    };
    let mut diff_args = vec![
        "--literal-pathspecs",
        // User configuration must not change the shape of the output we parse: pin the path
        // prefixes (mnemonic prefixes would give `w/` and `i/`), quoting and rename detection.
        "-c",
        "core.quotepath=false",
        "-c",
        "diff.mnemonicPrefix=false",
        "-c",
        "diff.noprefix=false",
        "-c",
        "diff.renames=true",
        "diff",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--relative",
        "-M",
        "-U3",
        &git.base,
        "--",
    ];
    if let Some(paths) = paths {
        diff_args.extend(paths.iter().map(String::as_str));
    }
    let diff = git.run(&diff_args, MAX_DIFF_BYTES);
    let (text, mut truncated) = match diff {
        Ok(v) => v,
        Err(e) => return Workspace::Unavailable(e),
    };
    let mut files = parse_diff(&String::from_utf8_lossy(&text));
    let mut listing_args = vec![
        "--literal-pathspecs",
        "ls-files",
        "--others",
        "--exclude-standard",
        "-z",
        "--",
    ];
    if let Some(paths) = paths {
        listing_args.extend(paths.iter().map(String::as_str));
    }
    match git.untracked(&listing_args, MAX_DIFF_BYTES) {
        Ok((listing, cut)) => {
            truncated |= cut;
            let mut names: Vec<String> = listing
                .split(|b| *b == 0)
                .filter(|n| !n.is_empty())
                .map(|n| String::from_utf8_lossy(n).into_owned())
                .collect();
            names.sort();
            if names.len() > MAX_UNTRACKED_FILES {
                truncated = true;
                names.truncate(MAX_UNTRACKED_FILES);
            }
            for name in names {
                files.push(untracked_file(dir, &name));
            }
        }
        Err(e) => return Workspace::Unavailable(e),
    }
    Workspace::Changes { files, truncated }
}

/// An untracked file shown as an all-added change, bounded and never read if it is not a plain file.
fn untracked_file(dir: &Path, name: &str) -> FileChange {
    let path = dir.join(name);
    // A symlink or special file is listed but never followed or read.
    let Ok(meta) = std::fs::symlink_metadata(&path) else {
        return marker(name, "file is no longer there");
    };
    if !meta.file_type().is_file() {
        return marker(name, "not a regular file");
    }
    let mut bytes = Vec::new();
    let read = git::regular(&path).and_then(|f| {
        f.take(MAX_UNTRACKED_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
    });
    if read.is_err() {
        return marker(name, "cannot be read");
    }
    if bytes.contains(&0) {
        return marker(name, "binary file");
    }
    let cut = bytes.len() > MAX_UNTRACKED_BYTES;
    bytes.truncate(MAX_UNTRACKED_BYTES);
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<DiffLine> = text
        .lines()
        .enumerate()
        .take(MAX_LINES_PER_FILE)
        .map(|(i, l)| DiffLine {
            kind: DiffKind::Add,
            old_no: None,
            new_no: Some(i as u32 + 1),
            text: l.to_owned(),
        })
        .collect();
    let added = lines.len() as u32;
    if cut || text.lines().count() > MAX_LINES_PER_FILE {
        lines.push(note_line("… file shown only in part"));
    }
    FileChange {
        path: name.to_owned(),
        added,
        removed: 0,
        hunks: vec![Hunk {
            header: format!("@@ -0,0 +1,{added} @@ new file"),
            lines,
        }],
    }
}

fn marker(name: &str, why: &str) -> FileChange {
    FileChange {
        path: name.to_owned(),
        added: 0,
        removed: 0,
        hunks: vec![Hunk {
            header: format!("({why})"),
            lines: vec![],
        }],
    }
}

fn note_line(text: &str) -> DiffLine {
    DiffLine {
        kind: DiffKind::Context,
        old_no: None,
        new_no: None,
        text: text.to_owned(),
    }
}

/// Parse `git diff` unified output. Tolerant: anything unrecognized between files is skipped.
pub fn parse_diff(text: &str) -> Vec<FileChange> {
    let mut files: Vec<FileChange> = Vec::new();
    let mut current: Option<FileChange> = None;
    let mut hunk: Option<Hunk> = None;
    let (mut old_no, mut new_no) = (0u32, 0u32);
    let mut lines_in_file = 0usize;
    let mut old_path = String::new();

    fn close_hunk(file: &mut Option<FileChange>, hunk: &mut Option<Hunk>) {
        if let (Some(f), Some(h)) = (file.as_mut(), hunk.take()) {
            f.hunks.push(h);
        }
    }
    fn close_file(
        files: &mut Vec<FileChange>,
        file: &mut Option<FileChange>,
        hunk: &mut Option<Hunk>,
    ) {
        close_hunk(file, hunk);
        if let Some(f) = file.take() {
            files.push(f);
        }
    }

    for line in text.lines() {
        if line.starts_with("diff --git ") {
            close_file(&mut files, &mut current, &mut hunk);
            current = Some(FileChange {
                path: String::new(),
                added: 0,
                removed: 0,
                hunks: vec![],
            });
            lines_in_file = 0;
            old_path.clear();
            continue;
        }
        let Some(file) = current.as_mut() else {
            continue;
        };
        if hunk.is_none() {
            if let Some(path) = line.strip_prefix("--- ") {
                old_path = strip_prefix_path(path, "a/");
                continue;
            }
            if let Some(path) = line.strip_prefix("+++ ") {
                let new = strip_prefix_path(path, "b/");
                file.path = if new == "/dev/null" {
                    old_path.clone()
                } else {
                    new
                };
                continue;
            }
            if let Some(to) = line.strip_prefix("rename to ") {
                file.path = to.to_owned();
                continue;
            }
            if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
                if file.path.is_empty() {
                    file.path = line
                        .split(" and ")
                        .nth(1)
                        .and_then(|s| s.strip_suffix(" differ"))
                        .map(|s| strip_prefix_path(s, "b/"))
                        .unwrap_or_default();
                }
                file.hunks.push(Hunk {
                    header: "(binary file)".into(),
                    lines: vec![],
                });
                continue;
            }
        }
        if let Some(rest) = line.strip_prefix("@@ ") {
            close_hunk(&mut current, &mut hunk);
            if let Some((o, n)) = parse_hunk_header(rest) {
                old_no = o;
                new_no = n;
            }
            hunk = Some(Hunk {
                header: line.to_owned(),
                lines: vec![],
            });
            continue;
        }
        let Some(h) = hunk.as_mut() else { continue };
        let Some(first) = line.chars().next() else {
            continue;
        };
        if lines_in_file >= MAX_LINES_PER_FILE {
            if lines_in_file == MAX_LINES_PER_FILE {
                h.lines.push(note_line("… more changes not shown"));
                lines_in_file += 1;
            }
            // Keep counting so the totals stay honest.
            match first {
                '+' => file.added += 1,
                '-' => file.removed += 1,
                _ => {}
            }
            continue;
        }
        let body = line[first.len_utf8()..].to_owned();
        match first {
            '+' => {
                file.added += 1;
                h.lines.push(DiffLine {
                    kind: DiffKind::Add,
                    old_no: None,
                    new_no: Some(new_no),
                    text: body,
                });
                new_no += 1;
            }
            '-' => {
                file.removed += 1;
                h.lines.push(DiffLine {
                    kind: DiffKind::Remove,
                    old_no: Some(old_no),
                    new_no: None,
                    text: body,
                });
                old_no += 1;
            }
            ' ' => {
                h.lines.push(DiffLine {
                    kind: DiffKind::Context,
                    old_no: Some(old_no),
                    new_no: Some(new_no),
                    text: body,
                });
                old_no += 1;
                new_no += 1;
            }
            // "\ No newline at end of file"
            '\\' => h.lines.push(note_line(line)),
            _ => continue,
        }
        lines_in_file += 1;
    }
    close_file(&mut files, &mut current, &mut hunk);
    files.retain(|f| !f.path.is_empty());
    files
}

/// `--- a/path\t` to `path`; `/dev/null` is kept as is.
fn strip_prefix_path(raw: &str, prefix: &str) -> String {
    let path = raw.trim_end_matches('\t');
    path.strip_prefix(prefix).unwrap_or(path).to_owned()
}

/// `-12,3 +14,4 @@ ...` to the first old and new line numbers.
fn parse_hunk_header(rest: &str) -> Option<(u32, u32)> {
    let mut parts = rest.split_whitespace();
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let start = |s: &str| s.split(',').next()?.parse::<u32>().ok();
    Some((start(old)?, start(new)?))
}

#[cfg(test)]
mod tests {
    use std::process::Command as Cmd;

    use super::*;

    fn run(dir: &Path, args: &[&str]) {
        let status = Cmd::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "-c",
                "commit.gpgsign=false",
                // Fixture commits must not leave detached maintenance changing .git
                // while the read-only collector's before/after snapshots run.
                "-c",
                "maintenance.auto=false",
                "-c",
                "gc.auto=0",
            ])
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?}");
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        run(dir.path(), &["init", "-q", "-b", "main"]);
        dir
    }

    fn commit_all(dir: &Path, msg: &str) {
        run(dir, &["add", "-A"]);
        run(dir, &["commit", "-q", "-m", msg]);
    }

    fn changes(dir: &Path) -> (Vec<FileChange>, bool) {
        match collect(dir) {
            Workspace::Changes { files, truncated } => (files, truncated),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    #[cfg(unix)]
    fn inspection_never_runs_configured_filters_textconv_diff_or_fsmonitor() {
        use std::os::unix::fs::PermissionsExt;
        let dir = repo();
        let root = dir.path();
        std::fs::write(root.join("a.txt"), "old\n").unwrap();
        commit_all(root, "baseline");
        let driver = root.join(".git/driver");
        std::fs::write(&driver, "#!/bin/sh\ntouch \"$0.ran\"\ncat \"$@\"\n").unwrap();
        std::fs::set_permissions(&driver, std::fs::Permissions::from_mode(0o700)).unwrap();
        for key in [
            "filter.trap.clean",
            "filter.trap.smudge",
            "filter.trap.process",
            "diff.trap.textconv",
            "diff.external",
            "core.fsmonitor",
        ] {
            run(root, &["config", key, driver.to_str().unwrap()]);
        }
        run(root, &["config", "filter.trap.required", "true"]);
        std::fs::write(root.join(".gitattributes"), "*.txt filter=trap diff=trap\n").unwrap();
        std::fs::write(
            root.join(".git/info/attributes"),
            "*.txt filter=trap diff=trap\n",
        )
        .unwrap();
        std::fs::write(root.join(".git/info/exclude"), "private.txt\n").unwrap();
        std::fs::write(root.join("private.txt"), "must not be displayed\n").unwrap();
        std::fs::write(root.join("a.txt"), "new\n").unwrap();
        let index = std::fs::read(root.join(".git/index")).unwrap();
        let (files, _) = changes(root);
        assert!(
            files
                .iter()
                .any(|f| f.path == "a.txt" && f.added == 1 && f.removed == 1)
        );
        assert!(!files.iter().any(|f| f.path == "private.txt"));
        assert!(!root.join(".git/driver.ran").exists());
        assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
        assert!(!root.join(".git/index.lock").exists());
    }

    #[test]
    #[cfg(unix)]
    fn an_index_symlink_fails_closed_without_reading_its_target() {
        let dir = repo();
        let root = dir.path();
        let target = root.join("private-index-target");
        std::fs::write(&target, "not an index; never display me").unwrap();
        std::os::unix::fs::symlink(&target, root.join(".git/index")).unwrap();
        assert!(matches!(collect(root), Workspace::Unavailable(_)));
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "not an index; never display me"
        );
    }

    #[test]
    fn linked_worktree_and_subdirectory_scans_stay_in_the_captured_project() {
        let dir = repo();
        let root = dir.path();
        std::fs::create_dir(root.join("nested")).unwrap();
        std::fs::write(root.join("nested/a.txt"), "old\n").unwrap();
        commit_all(root, "baseline");
        let target = tempfile::tempdir().unwrap();
        run(
            root,
            &[
                "worktree",
                "add",
                "--detach",
                target.path().to_str().unwrap(),
            ],
        );
        std::fs::write(target.path().join("nested/a.txt"), "new\n").unwrap();
        let (files, _) = changes(&target.path().join("nested"));
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "a.txt");
        assert_eq!((files[0].added, files[0].removed), (1, 1));
        assert!(changes(root).0.is_empty());
    }

    #[test]
    fn sha256_unborn_and_committed_repositories_have_real_baselines() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        run(root, &["init", "-q", "--object-format=sha256"]);
        std::fs::write(root.join("a.txt"), "old\n").unwrap();
        assert_eq!(changes(root).0.len(), 1);
        commit_all(root, "baseline");
        assert!(changes(root).0.is_empty());
        std::fs::write(root.join("a.txt"), "new\n").unwrap();
        let (files, _) = changes(root);
        assert_eq!((files[0].added, files[0].removed), (1, 1));
    }

    #[test]
    fn a_clean_repository_has_no_changes() {
        let dir = repo();
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        commit_all(dir.path(), "init");
        assert_eq!(changes(dir.path()), (vec![], false));
    }

    #[test]
    fn a_modified_tracked_file_shows_real_hunks_with_line_numbers() {
        let dir = repo();
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\nfour\n").unwrap();
        commit_all(dir.path(), "init");
        std::fs::write(dir.path().join("a.txt"), "one\nTWO\nthree\nfour\nfive\n").unwrap();
        let (files, _) = changes(dir.path());
        assert_eq!(files.len(), 1);
        let f = &files[0];
        assert_eq!((f.path.as_str(), f.added, f.removed), ("a.txt", 2, 1));
        assert_eq!(f.hunks.len(), 1);
        let kinds: Vec<_> = f.hunks[0]
            .lines
            .iter()
            .map(|l| (l.kind, l.text.as_str(), l.old_no, l.new_no))
            .collect();
        assert!(
            kinds.contains(&(DiffKind::Remove, "two", Some(2), None)),
            "{kinds:?}"
        );
        assert!(kinds.contains(&(DiffKind::Add, "TWO", None, Some(2))));
        assert!(kinds.contains(&(DiffKind::Add, "five", None, Some(5))));
        assert!(kinds.contains(&(DiffKind::Context, "one", Some(1), Some(1))));
    }

    #[test]
    fn staged_and_unstaged_edits_both_count() {
        let dir = repo();
        std::fs::write(dir.path().join("a.txt"), "a\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "b\n").unwrap();
        commit_all(dir.path(), "init");
        std::fs::write(dir.path().join("a.txt"), "a2\n").unwrap();
        run(dir.path(), &["add", "a.txt"]);
        std::fs::write(dir.path().join("b.txt"), "b2\n").unwrap();
        let (files, _) = changes(dir.path());
        let names: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(names, ["a.txt", "b.txt"]);
    }

    #[test]
    fn a_new_untracked_file_is_shown_as_all_added() {
        let dir = repo();
        std::fs::write(dir.path().join("a.txt"), "a\n").unwrap();
        commit_all(dir.path(), "init");
        std::fs::write(dir.path().join("hello.txt"), "hello\nworld\n").unwrap();
        let (files, _) = changes(dir.path());
        assert_eq!(files.len(), 1);
        assert_eq!(
            (files[0].path.as_str(), files[0].added, files[0].removed),
            ("hello.txt", 2, 0)
        );
        assert!(
            files[0].hunks[0]
                .lines
                .iter()
                .all(|l| l.kind == DiffKind::Add)
        );
        assert_eq!(files[0].hunks[0].lines[1].new_no, Some(2));
    }

    #[test]
    fn deleted_and_renamed_files_are_reported() {
        let dir = repo();
        std::fs::write(dir.path().join("gone.txt"), "x\ny\n").unwrap();
        std::fs::write(
            dir.path().join("old.txt"),
            "same content that is long enough to match\nline two\nline three\n",
        )
        .unwrap();
        commit_all(dir.path(), "init");
        std::fs::remove_file(dir.path().join("gone.txt")).unwrap();
        std::fs::rename(dir.path().join("old.txt"), dir.path().join("new.txt")).unwrap();
        run(dir.path(), &["add", "-A"]);
        let (files, _) = changes(dir.path());
        let gone = files
            .iter()
            .find(|f| f.path == "gone.txt")
            .expect("deleted file listed");
        assert_eq!((gone.added, gone.removed), (0, 2));
        assert!(files.iter().any(|f| f.path == "new.txt"), "{files:?}");
    }

    #[test]
    fn binary_and_non_regular_files_are_marked_not_dumped() {
        let dir = repo();
        std::fs::write(dir.path().join("a.txt"), "a\n").unwrap();
        commit_all(dir.path(), "init");
        std::fs::write(dir.path().join("blob.bin"), [0u8, 1, 2, 0, 255]).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", dir.path().join("link")).unwrap();
        let (files, _) = changes(dir.path());
        let blob = files.iter().find(|f| f.path == "blob.bin").unwrap();
        assert!(blob.hunks[0].header.contains("binary"));
        let link = files.iter().find(|f| f.path == "link").unwrap();
        assert!(
            link.hunks[0].header.contains("not a regular file"),
            "a symlink is never followed"
        );
        assert!(link.hunks[0].lines.is_empty());
    }

    #[test]
    fn a_binary_change_to_a_tracked_file_is_marked() {
        let dir = repo();
        std::fs::write(dir.path().join("img.bin"), [0u8, 1, 2, 3]).unwrap();
        commit_all(dir.path(), "init");
        std::fs::write(dir.path().join("img.bin"), [0u8, 9, 9, 9, 9]).unwrap();
        let (files, _) = changes(dir.path());
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "img.bin");
        assert_eq!(files[0].hunks[0].header, "(binary file)");
    }

    #[test]
    fn a_repository_with_no_commits_shows_staged_and_untracked_files() {
        let dir = repo();
        std::fs::write(dir.path().join("staged.txt"), "s\n").unwrap();
        run(dir.path(), &["add", "staged.txt"]);
        std::fs::write(dir.path().join("loose.txt"), "l\n").unwrap();
        let (files, _) = changes(dir.path());
        let names: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(names, ["staged.txt", "loose.txt"]);
    }

    #[test]
    fn paths_with_spaces_and_unicode_survive() {
        let dir = repo();
        std::fs::write(dir.path().join("a b.txt"), "x\n").unwrap();
        std::fs::write(dir.path().join("caf\u{e9}.txt"), "x\n").unwrap();
        commit_all(dir.path(), "init");
        std::fs::write(dir.path().join("a b.txt"), "y\n").unwrap();
        std::fs::write(dir.path().join("caf\u{e9}.txt"), "y\n").unwrap();
        let (files, _) = changes(dir.path());
        let names: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert!(
            names.contains(&"a b.txt") && names.contains(&"caf\u{e9}.txt"),
            "{names:?}"
        );
    }

    #[test]
    fn a_huge_diff_is_bounded_and_flagged() {
        let dir = repo();
        std::fs::write(dir.path().join("big.txt"), "x\n".repeat(10)).unwrap();
        commit_all(dir.path(), "init");
        let big: String = (0..300_000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(dir.path().join("big.txt"), &big).unwrap();
        let (files, truncated) = changes(dir.path());
        assert!(truncated, "the diff exceeded its byte bound");
        let lines: usize = files
            .iter()
            .flat_map(|f| &f.hunks)
            .map(|h| h.lines.len())
            .sum();
        assert!(lines <= MAX_LINES_PER_FILE + 2, "{lines}");
    }

    #[test]
    fn many_untracked_files_are_capped() {
        let dir = repo();
        std::fs::write(dir.path().join("a.txt"), "a\n").unwrap();
        commit_all(dir.path(), "init");
        for i in 0..(MAX_UNTRACKED_FILES + 10) {
            std::fs::write(dir.path().join(format!("f{i:03}.txt")), "x\n").unwrap();
        }
        let (files, truncated) = changes(dir.path());
        assert_eq!(files.len(), MAX_UNTRACKED_FILES);
        assert!(truncated);
    }

    #[test]
    fn a_directory_that_is_not_a_repository_is_said_so() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(collect(dir.path()), Workspace::NotARepository);
        assert!(matches!(
            collect(&dir.path().join("missing")),
            Workspace::Unavailable(_)
        ));
    }

    #[test]
    fn collecting_never_modifies_the_repository() {
        let dir = repo();
        std::fs::write(dir.path().join("a.txt"), "a\n").unwrap();
        commit_all(dir.path(), "init");
        std::fs::write(dir.path().join("a.txt"), "b\n").unwrap();
        std::fs::write(dir.path().join("new.txt"), "n\n").unwrap();
        let before = snapshot(dir.path());
        let _ = collect(dir.path());
        let _ = collect(dir.path());
        assert_eq!(snapshot(dir.path()), before);
    }

    /// Every file under the repo (including .git) with its length and mtime.
    fn snapshot(dir: &Path) -> Vec<(String, u64, std::time::SystemTime)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for entry in std::fs::read_dir(&d).unwrap().flatten() {
                let meta = entry.metadata().unwrap();
                if meta.is_dir() {
                    stack.push(entry.path());
                } else {
                    out.push((
                        entry.path().display().to_string(),
                        meta.len(),
                        meta.modified().unwrap(),
                    ));
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn parse_diff_tolerates_noise_and_truncated_input() {
        assert!(parse_diff("").is_empty());
        assert!(parse_diff("not a diff at all\n+stray\n").is_empty());
        let partial = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n a\n-b\n+";
        let files = parse_diff(partial);
        assert_eq!(files[0].path, "x");
        let hunk = &files[0].hunks[0];
        assert_eq!(hunk.lines.len(), 3);
        assert_eq!(parse_hunk_header("-1,2 +3 @@ foo"), Some((1, 3)));
        assert_eq!(parse_hunk_header("garbage"), None);
    }
}
