//! Internal restricted-Write command construction. NOT production qualification.
//! Public setup is metadata-only; coding execution remains internal and gated.
//! Never reuse chat-only bindings.
//! The CLI's managed policy remains effective; this is not universal interception.
#![allow(dead_code)] // Public coding execution remains gated.
use super::{
    permission_helper::HelperLaunch,
    policy::{self, Environment, ModelAlias, TurnMode},
    preflight::FileStamp,
    process::{OutputMode, Spec},
};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
};

pub(crate) const ID: &str = crate::store::EXTERNAL_PERMISSION_POLICY;
pub(crate) const EXTRA_FLAGS: &[&str] = &["--settings"];

/// Stable running host image, not a project script or replaceable executable path.
/// Linux-only until an equivalent image-lifetime route is qualified elsewhere.
/// /proc anchoring does not establish crash/orphan or PID-reuse containment.
pub(crate) struct CurrentHelper {
    path: PathBuf,
    stamp: FileStamp,
    pid: u32,
}
impl CurrentHelper {
    pub fn capture() -> Result<Self, &'static str> {
        if !cfg!(target_os = "linux") {
            return Err("coding helper image anchoring is unsupported on this platform");
        }
        let pid = std::process::id();
        let path = PathBuf::from(format!("/proc/{pid}/exe"));
        let stamp = FileStamp::read(&path)?;
        Ok(Self { path, stamp, pid })
    }
    fn validate(&self) -> Result<(), &'static str> {
        if self.pid != std::process::id() || FileStamp::read(&self.path)? != self.stamp {
            return Err("coding helper image changed");
        }
        Ok(())
    }
}

/// Per-run endpoint/nonce is separate from immutable CLI context identity. No Debug.
pub(crate) struct Hook {
    image: CurrentHelper,
    launch: HelperLaunch,
}
impl Hook {
    pub fn capture(launch: HelperLaunch) -> Result<Self, &'static str> {
        Ok(Self {
            image: CurrentHelper::capture()?,
            launch,
        })
    }
    fn settings(&self) -> Result<serde_json::Value, &'static str> {
        self.image.validate()?;
        let mut words = vec![shell_word(self.image.path.as_os_str())?];
        for argument in self.launch.arguments() {
            words.push(shell_word(argument)?);
        }
        // Leave room for bounded stdin/connect/output stages around the helper's
        // 120-second decision timeout. The broker request expires earlier (110s).
        Ok(settings(words.join(" "), 130))
    }
}
fn shell_word(word: &std::ffi::OsStr) -> Result<String, &'static str> {
    let text = word.to_str().ok_or("hook command path is not Unicode")?;
    if text.contains('\0') {
        return Err("invalid hook command path");
    }
    Ok(format!("'{}'", text.replace('\'', "'\\''")))
}
fn settings(command: String, timeout: u64) -> serde_json::Value {
    serde_json::json!({
        "permissions": {"ask": ["Write"]},
        "hooks": {"PermissionRequest": [{"matcher":"Write", "hooks":[{
            "type":"command", "command":command, "timeout":timeout, "onFailure":"block"
        }]}]}
    })
}
fn args(settings: serde_json::Value) -> Vec<OsString> {
    // --safe-mode suppresses this supplied hook. --bare/bypass/auto are forbidden.
    vec![
        "--mcp-config".into(),
        "{\"mcpServers\":{}}".into(),
        "--settings".into(),
        settings.to_string().into(),
        "--restricted".into(),
        "--strict-mcp-config".into(),
        "--disable-slash-commands".into(),
        "--no-chrome".into(),
    ]
}
/// Auth uses the same restricted configuration family, with an unconditional
/// failure hook because no broker/run exists yet. No tool or inference is requested.
pub(crate) fn metadata_args() -> Vec<OsString> {
    args(settings("exit 1".into(), 1))
}

pub(crate) struct Turn<'a> {
    pub executable: &'a Path,
    pub cwd: &'a Path,
    pub environment: &'a Environment,
    pub uuid: &'a str,
    pub mode: TurnMode,
    pub model: ModelAlias,
    pub prompt: &'a str,
}
pub(crate) fn turn_spec(turn: Turn<'_>, hook: &Hook) -> Result<Spec, &'static str> {
    if !policy::valid_uuid(turn.uuid) {
        return Err("invalid explicit CLI session identifier");
    }
    if turn.prompt.trim().is_empty()
        || turn.prompt.len() > policy::MAX_PROMPT_BYTES
        || turn.prompt.contains('\0')
    {
        return Err("CLI prompt is empty, oversized or contains NUL");
    }
    // Reserved broker environment must not arrive through the caller's snapshot.
    let mut env = policy::launch_environment(turn.environment)?;
    let (key, value) = hook.launch.environment();
    env.insert(key.into(), value);
    let mut arguments = args(hook.settings()?);
    arguments.extend(
        [
            "--print",
            "--tools",
            "Write",
            "--disallowedTools",
            "mcp__*",
            "--permission-mode",
            "default",
            "--permission-prompts",
            "none",
            "--input-format",
            "text",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--model",
            turn.model.as_str(),
            if turn.mode == TurnMode::Resume {
                "--resume"
            } else {
                "--session-id"
            },
            turn.uuid,
        ]
        .into_iter()
        .map(OsString::from),
    );
    let mut spec = policy::metadata_spec(turn.executable, turn.cwd, &env, arguments);
    spec.input = turn.prompt.as_bytes().to_vec();
    spec.input.push(b'\n');
    spec.mode = OutputMode::Stream;
    spec.timeout = Some(Duration::from_secs(180));
    spec.max_stdout = 16 * 1024 * 1024;
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::claude_cli::{permission_helper::NONCE_ENV, permission_wire::Binding};
    #[test]
    fn quoting_never_interpolates_paths() {
        assert_eq!(
            shell_word("a'$(touch nope);\n".as_ref()).unwrap(),
            "'a'\\''$(touch nope);\n'"
        );
        assert!(shell_word("nul\0".as_ref()).is_err());
    }
    #[test]
    fn metadata_has_blocking_failure_not_safe_mode_or_permission_bypass() {
        let args = metadata_args();
        let value: serde_json::Value = serde_json::from_str(args[3].to_str().unwrap()).unwrap();
        let hook = &value["hooks"]["PermissionRequest"][0]["hooks"][0];
        assert_eq!(hook["command"], "exit 1");
        assert_eq!(hook["onFailure"], "block");
        assert!(
            !args
                .iter()
                .any(|a| a == "--safe-mode" || a == "--bare" || a == "--allowedTools")
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn changed_image_and_inherited_reserved_environment_fail_closed() {
        let nonce = Binding::fresh().unwrap();
        let mut hook =
            Hook::capture(HelperLaunch::new(Path::new("/private/socket"), nonce).unwrap()).unwrap();
        hook.image.pid = hook.image.pid.wrapping_add(1);
        assert!(hook.settings().is_err());
        hook.image.pid = std::process::id();
        assert!(hook.settings().is_ok());
        let mut env: Environment = [("HOME".into(), "/fixture/home".into())]
            .into_iter()
            .collect();
        env.insert(NONCE_ENV.into(), "".into());
        assert!(
            turn_spec(
                Turn {
                    executable: Path::new("/fixture/claude"),
                    cwd: Path::new("/project"),
                    environment: &env,
                    uuid: "11111111-1111-4111-8111-111111111111",
                    mode: TurnMode::Fresh,
                    model: ModelAlias::Sonnet,
                    prompt: "prompt"
                },
                &hook
            )
            .is_err()
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn fixed_write_policy_uses_stable_image_and_no_prompt_or_nonce_in_arguments() {
        let nonce = Binding::fresh().unwrap();
        let hook = Hook::capture(
            HelperLaunch::new(Path::new("/private/quote'$(never)/socket"), nonce).unwrap(),
        )
        .unwrap();
        let env: Environment = [
            ("HOME".into(), "/fixture/home".into()),
            ("PATH".into(), "/usr/bin:/bin".into()),
        ]
        .into_iter()
        .collect();
        for mode in [TurnMode::Fresh, TurnMode::Resume] {
            let spec = turn_spec(
                Turn {
                    executable: Path::new("/fixture/claude"),
                    cwd: Path::new("/project"),
                    environment: &env,
                    uuid: "11111111-1111-4111-8111-111111111111",
                    mode,
                    model: ModelAlias::Sonnet,
                    prompt: "private prompt canary",
                },
                &hook,
            )
            .unwrap();
            assert_eq!(spec.input, b"private prompt canary\n");
            assert_eq!(
                spec.env.get(std::ffi::OsStr::new(NONCE_ENV)).unwrap(),
                &OsString::from(nonce.to_hex())
            );
            let argv: Vec<_> = spec.args.iter().map(|s| s.to_str().unwrap()).collect();
            for forbidden in [
                "--safe-mode",
                "--bare",
                "--allowedTools",
                "--dangerously-skip-permissions",
                "--continue",
            ] {
                assert!(!argv.contains(&forbidden));
            }
            assert!(argv.windows(2).any(|p| p == ["--tools", "Write"]));
            assert!(
                argv.windows(2)
                    .any(|p| p == ["--permission-mode", "default"])
            );
            let settings: serde_json::Value = serde_json::from_str(argv[3]).unwrap();
            let command = settings["hooks"]["PermissionRequest"][0]["hooks"][0]["command"]
                .as_str()
                .unwrap();
            assert!(command.starts_with(&format!("'/proc/{}/exe'", std::process::id())));
            assert!(command.contains("--claude-permission-hook"));
            assert!(command.contains("quote'\\''$(never)"));
            assert!(!argv.join(" ").contains("private prompt canary"));
            assert!(!argv.join(" ").contains(&nonce.to_hex()));
            assert_eq!(settings["permissions"]["ask"], serde_json::json!(["Write"]));
        }
    }
}
