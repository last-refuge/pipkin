//! Starting the person's editor or terminal, outside the application.
//!
//! Nothing here goes through a shell: a command is split into words and run with an argument
//! list, so a file name can never become part of a command. A file must lie inside the
//! project it came from, which is checked on the real path, after symlinks.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use pipkin_core::Launch;

/// What to run, from the person's settings and environment.
#[derive(Clone, Debug, Default)]
pub struct LaunchConfig {
    /// The editor command (`--editor`, `PIPKIN_EDITOR`, `VISUAL`, `EDITOR`), as words.
    pub editor: Option<String>,
    /// The terminal command (`--terminal`, `PIPKIN_TERMINAL`, `TERMINAL`), as words.
    pub terminal: Option<String>,
}

impl LaunchConfig {
    /// The settings from the environment, with explicit values (command-line flags) winning.
    pub fn from_env(editor: Option<String>, terminal: Option<String>) -> LaunchConfig {
        let env = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        LaunchConfig {
            editor: editor
                .or_else(|| env("PIPKIN_EDITOR"))
                .or_else(|| env("VISUAL"))
                .or_else(|| env("EDITOR")),
            terminal: terminal
                .or_else(|| env("PIPKIN_TERMINAL"))
                .or_else(|| env("TERMINAL")),
        }
    }
}

/// Editors that draw in the terminal they are started in, so need one opened for them.
const TERMINAL_EDITORS: &[&str] = &[
    "vi", "vim", "nvim", "neovim", "nano", "pico", "micro", "helix", "hx", "kak", "joe", "mg",
    "ne", "emacs",
];

/// Terminals tried, in order, when none is configured and `xdg-terminal-exec` is absent.
const FALLBACK_TERMINALS: &[&str] = &[
    "foot",
    "alacritty",
    "kitty",
    "wezterm",
    "ghostty",
    "gnome-terminal",
    "konsole",
    "xterm",
];

/// A command that is ready to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub argv: Vec<String>,
    pub cwd: PathBuf,
}

fn words(command: &str) -> Vec<String> {
    command.split_whitespace().map(str::to_owned).collect()
}

fn file_name(program: &str) -> &str {
    Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(program)
}

/// Whether `program` is found as an executable on `PATH` (or is itself a path to one).
fn on_path(program: &str, path: &str) -> bool {
    let candidate = Path::new(program);
    if candidate.components().count() > 1 {
        return candidate.is_file();
    }
    std::env::split_paths(path).any(|dir| dir.join(program).is_file())
}

fn terminal_words(config: &LaunchConfig, path: &str) -> Result<Vec<String>, String> {
    if let Some(configured) = &config.terminal {
        let w = words(configured);
        return if w.is_empty() {
            Err("the terminal setting is empty".into())
        } else {
            Ok(w)
        };
    }
    if on_path("xdg-terminal-exec", path) {
        return Ok(vec!["xdg-terminal-exec".into()]);
    }
    FALLBACK_TERMINALS
        .iter()
        .find(|t| on_path(t, path))
        .map(|t| vec![(*t).to_owned()])
        .ok_or_else(|| {
            "no terminal found. Set one with --terminal or the TERMINAL environment variable."
                .to_owned()
        })
}

/// Decide what to run for `launch`, or why it cannot be run. `path` is the `PATH` to search.
pub fn plan(launch: &Launch, config: &LaunchConfig, path: &str) -> Result<Plan, String> {
    match launch {
        Launch::Terminal { cwd } => {
            let dir = std::fs::canonicalize(cwd)
                .map_err(|e| format!("the project folder {cwd} is not available: {e}"))?;
            if !dir.is_dir() {
                return Err(format!("{cwd} is not a folder"));
            }
            Ok(Plan {
                argv: terminal_words(config, path)?,
                cwd: dir,
            })
        }
        Launch::Editor { path: file, root } => {
            let root = std::fs::canonicalize(root)
                .map_err(|e| format!("the project folder {root} is not available: {e}"))?;
            let real = std::fs::canonicalize(file)
                .map_err(|_| format!("{file} is not there any more (it may have been deleted)"))?;
            if !real.starts_with(&root) {
                return Err(format!(
                    "{file} is outside the project, so it was not opened"
                ));
            }
            if !real.is_file() {
                return Err(format!("{file} is not a file"));
            }
            let target = real.display().to_string();
            let mut argv = match &config.editor {
                Some(editor) => {
                    let w = words(editor);
                    if w.is_empty() {
                        return Err("the editor setting is empty".into());
                    }
                    if TERMINAL_EDITORS.contains(&file_name(&w[0])) {
                        let mut v = terminal_words(config, path)?;
                        // Terminals agree on `-e` for "run this", and `xdg-terminal-exec` takes
                        // the command directly.
                        if file_name(&v[0]) != "xdg-terminal-exec" {
                            v.push("-e".into());
                        }
                        v.extend(w);
                        v
                    } else {
                        w
                    }
                }
                None => vec!["xdg-open".into()],
            };
            argv.push(target);
            Ok(Plan { argv, cwd: root })
        }
    }
}

/// Start `plan` detached from this process (its own process group, no input or output), and
/// reap it in the background so it never lingers as a zombie.
pub fn spawn(plan: &Plan) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let (program, args) = plan
        .argv
        .split_first()
        .ok_or_else(|| "nothing to run".to_owned())?;
    let mut child = Command::new(program)
        .args(args)
        .current_dir(&plan.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("could not start {program}: {e}"))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Plan and start `launch`, returning a message for the person if it could not be.
pub fn run(launch: &Launch, config: &LaunchConfig) -> Result<(), String> {
    let path = std::env::var("PATH").unwrap_or_default();
    spawn(&plan(launch, config, &path)?)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn editor(file: &Path, root: &Path) -> Launch {
        Launch::Editor {
            path: file.display().to_string(),
            root: root.display().to_string(),
        }
    }

    fn config(editor: Option<&str>, terminal: Option<&str>) -> LaunchConfig {
        LaunchConfig {
            editor: editor.map(str::to_owned),
            terminal: terminal.map(str::to_owned),
        }
    }

    #[test]
    fn a_gui_editor_gets_the_real_path_as_one_argument_and_no_shell() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a file; rm -rf $HOME.txt");
        std::fs::write(&file, "x").unwrap();
        let p = plan(
            &editor(&file, dir.path()),
            &config(Some("code --wait"), None),
            "",
        )
        .unwrap();
        let real = std::fs::canonicalize(&file).unwrap().display().to_string();
        assert_eq!(p.argv, ["code", "--wait", &real]);
        assert_eq!(p.cwd, std::fs::canonicalize(dir.path()).unwrap());
    }

    #[test]
    fn with_no_editor_set_the_desktop_opens_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("n.txt");
        std::fs::write(&file, "x").unwrap();
        let p = plan(&editor(&file, dir.path()), &config(None, None), "").unwrap();
        assert_eq!(p.argv[0], "xdg-open");
        assert_eq!(p.argv.len(), 2);
    }

    #[test]
    fn a_terminal_editor_is_started_inside_a_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("n.txt");
        std::fs::write(&file, "x").unwrap();
        let real = std::fs::canonicalize(&file).unwrap().display().to_string();
        let p = plan(
            &editor(&file, dir.path()),
            &config(Some("nvim -R"), Some("foot --title pipkin")),
            "",
        )
        .unwrap();
        assert_eq!(
            p.argv,
            ["foot", "--title", "pipkin", "-e", "nvim", "-R", &real]
        );
        // `xdg-terminal-exec` takes the command as it is.
        let bin = tempfile::tempdir().unwrap();
        let exec = bin.path().join("xdg-terminal-exec");
        std::fs::write(&exec, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&exec, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = bin.path().display().to_string();
        let p = plan(
            &editor(&file, dir.path()),
            &config(Some("/usr/bin/vim"), None),
            &path,
        )
        .unwrap();
        assert_eq!(p.argv, ["xdg-terminal-exec", "/usr/bin/vim", &real]);
    }

    #[test]
    fn a_file_outside_the_project_or_reached_through_a_link_out_of_it_is_refused() {
        let project = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let outside = elsewhere.path().join("secret.txt");
        std::fs::write(&outside, "x").unwrap();
        let e = plan(
            &editor(&outside, project.path()),
            &config(Some("code"), None),
            "",
        )
        .unwrap_err();
        assert!(e.contains("outside the project"), "{e}");
        // A `..` path and a symlink both resolve to where they really point.
        let dotdot = project
            .path()
            .join("..")
            .join(elsewhere.path().file_name().unwrap())
            .join("secret.txt");
        assert!(
            plan(
                &editor(&dotdot, project.path()),
                &config(Some("code"), None),
                ""
            )
            .is_err()
        );
        let link = project.path().join("link.txt");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let e = plan(
            &editor(&link, project.path()),
            &config(Some("code"), None),
            "",
        )
        .unwrap_err();
        assert!(e.contains("outside the project"), "{e}");
    }

    #[test]
    fn a_missing_file_a_folder_and_empty_settings_say_what_is_wrong() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("gone.txt");
        let e = plan(&editor(&gone, dir.path()), &config(None, None), "").unwrap_err();
        assert!(e.contains("not there any more"), "{e}");
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let e = plan(&editor(&sub, dir.path()), &config(None, None), "").unwrap_err();
        assert!(e.contains("not a file"), "{e}");
        let file = dir.path().join("f.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(plan(&editor(&file, dir.path()), &config(Some("  "), None), "").is_err());
        assert!(
            plan(
                &editor(&file, &dir.path().join("nope")),
                &config(None, None),
                ""
            )
            .is_err()
        );
    }

    #[test]
    fn a_terminal_opens_in_the_project_and_one_must_be_found() {
        let dir = tempfile::tempdir().unwrap();
        let launch = Launch::Terminal {
            cwd: dir.path().display().to_string(),
        };
        let p = plan(&launch, &config(None, Some("alacritty --class x")), "").unwrap();
        assert_eq!(p.argv, ["alacritty", "--class", "x"]);
        assert_eq!(p.cwd, std::fs::canonicalize(dir.path()).unwrap());
        let e = plan(&launch, &config(None, None), "").unwrap_err();
        assert!(e.contains("no terminal found"), "{e}");
        // The first fallback that exists is used.
        let bin = tempfile::tempdir().unwrap();
        for name in ["kitty", "xterm"] {
            let f = bin.path().join(name);
            std::fs::write(&f, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let p = plan(
            &launch,
            &config(None, None),
            &bin.path().display().to_string(),
        )
        .unwrap();
        assert_eq!(p.argv, ["kitty"]);
        let missing = Launch::Terminal {
            cwd: "/nonexistent-pipkin-dir".into(),
        };
        assert!(plan(&missing, &config(None, Some("foot")), "").is_err());
    }

    #[test]
    fn a_started_command_runs_detached_in_the_right_folder_with_its_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out.txt");
        let script = dir.path().join("fake-editor");
        // Publish the executable in a separate process: an unrelated concurrent fork
        // must not inherit a writable script descriptor and cause ETXTBSY at launch.
        let status = Command::new("/bin/sh")
            .env_clear()
            .args(["-c", "printf '%s' \"$2\" > \"$1\"", "fixture-writer"])
            .arg(&script)
            .arg(format!(
                "#!/bin/sh\npwd > {out}\nfor a in \"$@\"; do echo \"arg:$a\" >> {out}; done\n",
                out = out.display()
            ))
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let file = dir.path().join("hello world.txt");
        std::fs::write(&file, "x").unwrap();
        run(
            &editor(&file, dir.path()),
            &config(Some(&script.display().to_string()), None),
        )
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let text = loop {
            if let Ok(t) = std::fs::read_to_string(&out)
                && t.contains("arg:")
            {
                break t;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the command never ran"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        let real_dir = std::fs::canonicalize(dir.path()).unwrap();
        assert!(text.starts_with(&real_dir.display().to_string()), "{text}");
        assert!(
            text.contains(&format!(
                "arg:{}",
                real_dir.join("hello world.txt").display()
            )),
            "{text}"
        );

        let e = run(
            &Launch::Terminal {
                cwd: dir.path().display().to_string(),
            },
            &config(None, Some("/nonexistent/terminal")),
        )
        .unwrap_err();
        assert!(e.contains("could not start"), "{e}");
    }
}
