//! Bounded filesystem-only discovery. Nothing here executes a candidate or grants trust.
use std::path::{Path, PathBuf};

use super::policy::{Environment, resolve};

const MAX_PATHS: usize = 128;
const MAX_RESULTS: usize = 16;

pub fn discover(saved: Option<&Path>, project: &Path, env: &Environment) -> Vec<String> {
    let project = project.canonicalize().ok();
    let eligible = |p: &Path| -> Option<String> {
        if p.as_os_str().len() > 4096 || p.components().any(|c| c.as_os_str() == "shims") {
            return None;
        }
        let resolved = resolve(Some(p), env).ok()?;
        if project
            .as_ref()
            .is_some_and(|root| p.starts_with(root) || resolved.starts_with(root))
            || resolved.components().any(|c| c.as_os_str() == "shims")
        {
            return None;
        }
        resolved
            .to_str()
            .filter(|s| s.len() <= 4096 && !s.chars().any(char::is_control))
            .map(str::to_owned)
    };
    // A saved path is only a suggestion. Inspection and confirmation still follow.
    if let Some(found) = saved.and_then(eligible) {
        return vec![found];
    }
    let mut paths = Vec::new();
    if let Some(path) = env.get(std::ffi::OsStr::new("PATH")) {
        paths.extend(
            std::env::split_paths(path)
                .filter(|p| p.is_absolute())
                .take(MAX_PATHS)
                .map(|p| p.join("claude")),
        );
    }
    if let Some(home) = env
        .get(std::ffi::OsStr::new("HOME"))
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        for suffix in [
            ".local/bin/claude",
            ".claude/local/claude",
            ".local/share/mise/installs/claude/latest/claude",
            ".local/share/mise/installs/claude-code/latest/bin/claude",
        ] {
            paths.push(home.join(suffix));
        }
    }
    for path in [
        "/usr/local/bin/claude",
        "/usr/bin/claude",
        "/opt/homebrew/bin/claude",
    ] {
        paths.push(path.into());
    }
    let mut results = Vec::new();
    for path in paths {
        if let Some(candidate) = eligible(&path)
            && !results.contains(&candidate)
        {
            results.push(candidate);
            if results.len() == MAX_RESULTS {
                break;
            }
        }
    }
    results
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        ffi::OsString,
        os::unix::fs::{PermissionsExt, symlink},
    };
    fn executable(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "#!/bin/sh\nexit 93\n").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    #[test]
    fn filesystem_discovery_deduplicates_symlinks_and_never_uses_project_or_shims() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let project = root.join("project");
        std::fs::create_dir(&project).unwrap();
        for suffix in [
            "bin/claude",
            "other/claude",
            "project/claude",
            "shims/claude",
        ] {
            executable(&root.join(suffix));
        }
        std::fs::create_dir(root.join("link")).unwrap();
        symlink(root.join("bin/claude"), root.join("link/claude")).unwrap();
        let env: Environment = [
            (
                OsString::from("PATH"),
                std::env::join_paths([
                    root.join("project"),
                    root.join("shims"),
                    root.join("bin"),
                    root.join("link"),
                    root.join("other"),
                ])
                .unwrap(),
            ),
            ("HOME".into(), root.into()),
        ]
        .into();
        let found = discover(None, &project, &env);
        assert!(found.contains(&root.join("bin/claude").to_string_lossy().into_owned()));
        assert!(found.contains(&root.join("other/claude").to_string_lossy().into_owned()));
        assert!(
            !found
                .iter()
                .any(|p| p.contains("shims") || p.contains("project"))
        );
        assert_eq!(
            found
                .iter()
                .filter(|p| p.ends_with("/bin/claude") && p.starts_with(root.to_str().unwrap()))
                .count(),
            1
        );
        assert_eq!(
            discover(Some(&root.join("other/claude")), &project, &env),
            vec![root.join("other/claude").to_str().unwrap()]
        );
    }
    #[test]
    fn desktop_path_can_find_mise_without_running_mise_or_a_shell() {
        let dir = tempfile::tempdir().unwrap();
        let installed = dir
            .path()
            .join(".local/share/mise/installs/claude/latest/claude");
        executable(&installed);
        let env = [
            ("PATH".into(), "relative:.:/nonexistent".into()),
            ("HOME".into(), dir.path().into()),
        ]
        .into();
        assert!(
            discover(None, Path::new("/nonexistent-project"), &env)
                .contains(&installed.to_str().unwrap().to_owned())
        );
    }
}
