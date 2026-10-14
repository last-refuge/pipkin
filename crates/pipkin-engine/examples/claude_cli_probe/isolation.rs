//! Standalone Phase 0 fixtures, not a production sandbox. No CLI is invoked here.
//! Helper controls establish fixture mechanics only, not whether Claude loads them.
//! The private root must remain owned by the probe; hostile concurrent filesystem
//! mutation by the same user is outside this fixture's isolation guarantees.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tempfile::TempDir;

const NAMES: &[&str] = &[
    "user-hook",
    "project-hook",
    "local-hook",
    "plugin-hook",
    "project-mcp",
];
const CONTENTS: &[u8] = b"claude-cli-probe fixture marker\n";

pub struct Fixture {
    // Keep ownership even though root() exposes the canonical spelling.
    _temp: TempDir,
    root: PathBuf,
    pub project: PathBuf,
    pub plugin: PathBuf,
    pub user_config: PathBuf,
}

impl Fixture {
    pub fn new(helper_executable: &Path) -> Result<Self, &'static str> {
        if !helper_executable.is_absolute() {
            return Err("helper executable must be absolute");
        }
        let helper = path_text(helper_executable)?;
        let temp = tempfile::Builder::new()
            .prefix("claude-cli-probe-isolation-")
            .tempdir()
            .map_err(|_| "cannot create fixture root")?;
        // temp_dir() can itself contain symlinks (notably on macOS).
        let root = fs::canonicalize(temp.path()).map_err(|_| "cannot canonicalize fixture root")?;
        let fixture = Self {
            project: root.join("project"),
            plugin: root.join("plugin"),
            user_config: root.join("user-config"),
            root,
            _temp: temp,
        };
        for dir in [
            fixture.root.join("markers"),
            fixture.root.join("home"),
            fixture.root.join("xdg-config"),
            fixture.project.join(".claude"),
            fixture.plugin.join(".claude-plugin"),
            fixture.plugin.join("hooks"),
            fixture.user_config.clone(),
        ] {
            fs::create_dir_all(dir).map_err(|_| "cannot create fixture directory")?;
        }
        for (path, name) in [
            (fixture.user_config.join("settings.json"), "user-hook"),
            (
                fixture.project.join(".claude/settings.json"),
                "project-hook",
            ),
            (
                fixture.project.join(".claude/settings.local.json"),
                "local-hook",
            ),
            (fixture.plugin.join("hooks/hooks.json"), "plugin-hook"),
        ] {
            let command = hook_command(helper_executable, fixture.root(), name)?;
            let mut settings = hook_settings(&command);
            if name == "user-hook" {
                // Approve only our harmless MCP sentinel in this disposable profile so
                // the control exercises discovery rather than stopping at an approval gate.
                settings["enabledMcpjsonServers"] = json!(["pipkin-phase0-fixture"]);
            }
            write_json(&path, &settings)?;
        }
        write_json(
            &fixture.plugin.join(".claude-plugin/plugin.json"),
            &json!({"name": "pipkin-phase0-fixture", "version": "0.0.1"}),
        )?;
        // hooks/hooks.json is the plugin's standard automatically discovered path.
        // This stdio command deliberately marks and exits; it is NOT an MCP server.
        write_json(
            &fixture.project.join(".mcp.json"),
            &json!({"mcpServers": {"pipkin-phase0-fixture": {
                "type": "stdio",
                "command": helper,
                "args": ["fixture-mark", "--root", path_text(fixture.root())?,
                         "--name", "project-mcp"]
            }}}),
        )?;
        Ok(fixture)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    #[cfg(test)]
    pub fn marker_names() -> &'static [&'static str] {
        marker_names()
    }

    /// Only report our normal, fixed-content sentinel files, in enum order.
    #[cfg(test)]
    pub fn observed(&self) -> Vec<&'static str> {
        let Ok(dir) = marker_dir(self.root()) else {
            return Vec::new();
        };
        NAMES
            .iter()
            .copied()
            .filter(|name| own_marker(&dir.join(format!("{name}.marker"))).unwrap_or(false))
            .collect()
    }

    /// Qualification must distinguish a missing marker from unreadable/tampered evidence.
    pub fn checked_observed(&self) -> Result<Vec<&'static str>, &'static str> {
        let dir = marker_dir(self.root())?;
        let mut observed = Vec::new();
        for name in NAMES {
            if own_marker(&dir.join(format!("{name}.marker")))? {
                observed.push(*name);
            }
        }
        Ok(observed)
    }

    /// Never recurse or enumerate arbitrary directory entries, even on cleanup.
    pub fn clear_markers(&self) -> Result<(), &'static str> {
        let dir = marker_dir(self.root())?;
        for name in NAMES {
            let path = dir.join(format!("{name}.marker"));
            if own_marker(&path)? {
                fs::remove_file(path).map_err(|_| "cannot remove fixture marker")?;
            }
        }
        Ok(())
    }

    /// Apply with Command::env_clear().envs(...); this is not an env overlay.
    /// PATH is the sole inherited variable, for shells and helper executables.
    pub fn isolated_env(&self) -> BTreeMap<OsString, OsString> {
        let mut env = BTreeMap::new();
        env.insert("HOME".into(), self.root.join("home").into_os_string());
        env.insert(
            "CLAUDE_CONFIG_DIR".into(),
            self.user_config.clone().into_os_string(),
        );
        env.insert(
            "XDG_CONFIG_HOME".into(),
            self.root.join("xdg-config").into_os_string(),
        );
        if let Some(path) = std::env::var_os("PATH") {
            env.insert("PATH".into(), path);
        }
        for (key, value) in [
            ("TERM", "dumb"),
            // Documented Claude Code settings environment variables.
            ("DISABLE_AUTOUPDATER", "1"),
            ("DISABLE_TELEMETRY", "1"),
            ("DISABLE_ERROR_REPORTING", "1"),
        ] {
            env.insert(key.into(), value.into());
        }
        env
    }
}

#[cfg(test)]
pub fn marker_names() -> &'static [&'static str] {
    NAMES
}

/// Entry point for `claude_cli_probe fixture-mark --root ABS --name ENUM`.
/// Requires an existing canonical root and real markers directory. Never creates
/// directories, follows marker symlinks, truncates a file, or accepts path names.
pub fn mark(root: &Path, name: &str) -> Result<(), &'static str> {
    if !NAMES.contains(&name) {
        return Err("unknown fixture marker");
    }
    let path = marker_dir(root)?.join(format!("{name}.marker"));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&path) {
        Ok(mut file) => file
            .write_all(CONTENTS)
            .map_err(|_| "cannot write fixture marker"),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            if own_marker(&path)? {
                Ok(())
            } else {
                Err("fixture marker disappeared")
            }
        }
        Err(_) => Err("cannot create fixture marker"),
    }
}

fn marker_dir(root: &Path) -> Result<PathBuf, &'static str> {
    if !root.is_absolute() {
        return Err("fixture root must be absolute and canonical");
    }
    let canonical = fs::canonicalize(root).map_err(|_| "invalid fixture root")?;
    // Compare raw spelling: Path equality normalizes some dot/trailing components.
    if canonical.as_os_str() != root.as_os_str()
        || !fs::symlink_metadata(root)
            .map_err(|_| "invalid fixture root")?
            .is_dir()
    {
        return Err("fixture root must be absolute and canonical");
    }
    let dir = root.join("markers");
    if !fs::symlink_metadata(&dir)
        .map_err(|_| "missing marker directory")?
        .is_dir()
    {
        return Err("marker directory must not be a symlink");
    }
    Ok(dir)
}

fn normal_marker(metadata: &fs::Metadata) -> bool {
    if !metadata.is_file() || metadata.len() != CONTENTS.len() as u64 {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // A hard link isn't an independently owned fixture sentinel either.
        if metadata.nlink() != 1 {
            return false;
        }
    }
    true
}

fn own_marker(path: &Path) -> Result<bool, &'static str> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err("cannot inspect fixture marker"),
    };
    if !normal_marker(&metadata) {
        return Err("existing path is not an owned fixture marker");
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|_| "cannot open fixture marker")?;
    if !normal_marker(&file.metadata().map_err(|_| "cannot inspect open marker")?) {
        return Err("existing path is not an owned fixture marker");
    }
    let mut contents = Vec::new();
    file.take(CONTENTS.len() as u64 + 1)
        .read_to_end(&mut contents)
        .map_err(|_| "cannot read fixture marker")?;
    if contents != CONTENTS {
        return Err("existing file has unexpected marker contents");
    }
    Ok(true)
}

fn path_text(path: &Path) -> Result<&str, &'static str> {
    path.to_str()
        .filter(|text| !text.contains('\0'))
        .ok_or("fixture command paths must be UTF-8 without NUL")
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn hook_command(helper: &Path, root: &Path, name: &str) -> Result<String, &'static str> {
    if !NAMES.contains(&name) {
        return Err("unknown fixture marker");
    }
    Ok(format!(
        "{} fixture-mark --root {} --name {}",
        shell_quote(path_text(helper)?),
        shell_quote(path_text(root)?),
        shell_quote(name),
    ))
}

fn hook_settings(command: &str) -> Value {
    let entry = json!([{"hooks": [{"type": "command", "command": command}]}]);
    json!({"hooks": {"SessionStart": entry.clone(), "Setup": entry}})
}

fn write_json(path: &Path, value: &Value) -> Result<(), &'static str> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| "cannot encode fixture JSON")?;
    let mut file = File::options()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| "cannot create fixture JSON")?;
    file.write_all(&bytes)
        .map_err(|_| "cannot write fixture JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Fixture {
        // Never executed; JSON generation needs only an absolute executable path.
        Fixture::new(&std::env::current_exe().unwrap()).unwrap()
    }

    fn read_json(path: &Path) -> Value {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn helper_controls_are_idempotent_and_cleanup_is_fixed() {
        let f = fixture();
        assert!(f.observed().is_empty());
        assert_eq!(Fixture::marker_names(), marker_names());
        for name in marker_names() {
            mark(f.root(), name).unwrap();
            mark(f.root(), name).unwrap();
        }
        assert_eq!(f.observed(), marker_names());
        let unrelated = f.root().join("markers/unrelated.marker");
        fs::write(&unrelated, b"leave alone").unwrap();
        f.clear_markers().unwrap();
        f.clear_markers().unwrap();
        assert!(f.observed().is_empty());
        assert_eq!(fs::read(unrelated).unwrap(), b"leave alone");
        let root = f.root().to_owned();
        drop(f);
        assert!(!root.exists());
    }

    #[test]
    fn settings_plugin_and_mcp_shapes() {
        let helper = std::env::current_exe().unwrap();
        let f = Fixture::new(&helper).unwrap();
        for (path, name) in [
            (f.user_config.join("settings.json"), "user-hook"),
            (f.project.join(".claude/settings.json"), "project-hook"),
            (f.project.join(".claude/settings.local.json"), "local-hook"),
            (f.plugin.join("hooks/hooks.json"), "plugin-hook"),
        ] {
            let value = read_json(&path);
            assert_eq!(value["hooks"].as_object().unwrap().len(), 2);
            for event in ["SessionStart", "Setup"] {
                let entries = value["hooks"][event].as_array().unwrap();
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0]["hooks"].as_array().unwrap().len(), 1);
                assert_eq!(entries[0]["hooks"][0]["type"], "command");
                assert_eq!(
                    entries[0]["hooks"][0]["command"],
                    hook_command(&helper, f.root(), name).unwrap()
                );
            }
        }
        let manifest = read_json(&f.plugin.join(".claude-plugin/plugin.json"));
        assert_eq!(manifest["name"], "pipkin-phase0-fixture");
        assert_eq!(manifest["version"], "0.0.1");
        let mcp = read_json(&f.project.join(".mcp.json"));
        assert_eq!(mcp["mcpServers"].as_object().unwrap().len(), 1);
        let server = &mcp["mcpServers"]["pipkin-phase0-fixture"];
        assert_eq!(server["type"], "stdio");
        assert_eq!(server["command"], helper.to_str().unwrap());
        assert_eq!(
            server["args"],
            json!([
                "fixture-mark",
                "--root",
                f.root().to_str().unwrap(),
                "--name",
                "project-mcp"
            ])
        );
        assert!(f.observed().is_empty());
    }

    #[test]
    fn environment_is_an_allowlist() {
        let f = fixture();
        let env = f.isolated_env();
        let allowed = [
            "HOME",
            "CLAUDE_CONFIG_DIR",
            "XDG_CONFIG_HOME",
            "PATH",
            "TERM",
            "DISABLE_AUTOUPDATER",
            "DISABLE_TELEMETRY",
            "DISABLE_ERROR_REPORTING",
        ];
        assert!(env.keys().all(|key| allowed.iter().any(|name| key == name)));
        for key in ["HOME", "CLAUDE_CONFIG_DIR", "XDG_CONFIG_HOME"] {
            let path = Path::new(env.get(std::ffi::OsStr::new(key)).unwrap());
            assert!(path.starts_with(f.root()) && path.is_dir());
        }
        assert_eq!(
            env.get(std::ffi::OsStr::new("PATH")).cloned(),
            std::env::var_os("PATH")
        );
        assert_eq!(
            env.get(std::ffi::OsStr::new("TERM")),
            Some(&OsString::from("dumb"))
        );
    }

    #[test]
    fn reject_names_escapes_and_nonmarkers() {
        let f = fixture();
        for name in [
            "",
            "unknown",
            "../user-hook",
            "/user-hook",
            "user-hook.marker",
            "project-hook/../../escape",
            "user-hook\0",
        ] {
            assert!(mark(f.root(), name).is_err());
        }
        for root in [
            PathBuf::from("."),
            f.root().join("."),
            f.root().join("project/.."),
            f.root().join("missing"),
        ] {
            assert!(mark(&root, "user-hook").is_err());
        }
        let path = f.root().join("markers/user-hook.marker");
        fs::write(&path, b"not ours").unwrap();
        assert!(mark(f.root(), "user-hook").is_err());
        assert!(f.clear_markers().is_err());
        assert!(f.observed().is_empty());
        assert_eq!(fs::read(&path).unwrap(), b"not ours");
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(mark(f.root(), "user-hook").is_err());
        assert!(f.clear_markers().is_err());
        assert!(Fixture::new(Path::new("relative-helper")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn reject_symlinks_and_hardlinks_without_touching_targets() {
        use std::os::unix::fs::symlink;
        let f = fixture();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("target");
        fs::write(&target, CONTENTS).unwrap();
        let marker = f.root().join("markers/user-hook.marker");
        symlink(&target, &marker).unwrap();
        assert!(mark(f.root(), "user-hook").is_err());
        assert!(f.clear_markers().is_err());
        assert!(f.observed().is_empty());
        fs::remove_file(&marker).unwrap();
        symlink(outside.path().join("missing"), &marker).unwrap();
        assert!(mark(f.root(), "user-hook").is_err());
        assert!(!outside.path().join("missing").exists());
        fs::remove_file(&marker).unwrap();
        fs::hard_link(&target, &marker).unwrap();
        assert!(mark(f.root(), "user-hook").is_err());
        assert!(f.clear_markers().is_err());
        fs::remove_file(&marker).unwrap();
        let alias = outside.path().join("alias");
        symlink(f.root(), &alias).unwrap();
        assert!(mark(&alias, "user-hook").is_err());
        fs::remove_dir(f.root().join("markers")).unwrap();
        symlink(outside.path(), f.root().join("markers")).unwrap();
        assert!(mark(f.root(), "user-hook").is_err());
        assert!(f.clear_markers().is_err());
        assert!(f.observed().is_empty());
        assert!(!outside.path().join("user-hook.marker").exists());
        assert_eq!(fs::read(target).unwrap(), CONTENTS);
    }

    #[cfg(unix)]
    #[test]
    fn shell_quotes_preserve_arguments_without_execution() {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;
        let temp = tempfile::tempdir().unwrap();
        let directory = temp
            .path()
            .join("space ' \" $HOME `false` ;\n tab\t backslash\\");
        fs::create_dir(&directory).unwrap();
        let helper = directory.join("helper ' \" $(false) \\ .sh");
        // A separate script receives arguments; no real probe or CLI is executed.
        fs::write(&helper, b"#!/bin/sh\nprintf '%s\\000' \"$@\"\n").unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        let root = directory.join("root ' \" ; $(exit 19) `exit 20`\n\\");
        let command = hook_command(&helper, &root, "project-hook").unwrap();
        let output = Command::new("/bin/sh")
            .args(["-c", &command])
            .env_clear()
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let expected = [
            "fixture-mark",
            "--root",
            root.to_str().unwrap(),
            "--name",
            "project-hook",
        ]
        .into_iter()
        .flat_map(|arg| arg.as_bytes().iter().copied().chain([0]))
        .collect::<Vec<_>>();
        assert_eq!(output.stdout, expected);
        assert_eq!(shell_quote(""), "''");
        assert!(hook_command(Path::new("/nul\0"), &root, "user-hook").is_err());
        use std::os::unix::ffi::OsStrExt;
        assert!(path_text(Path::new(std::ffi::OsStr::from_bytes(b"/bad\xff"))).is_err());
    }
}
