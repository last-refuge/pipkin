//! Inactive, native-MCP-only CLI launch construction. No model request or admission here.
//! Assets must outlive the supervised CLI process; stdout never becomes native authority.
use super::{
    policy::{self, Environment, ModelAlias, TurnMode},
    preflight::FileStamp,
    process::{OutputMode, Spec},
};
use crate::{runtime::cli_bridge_channel::NativeLaunch, store::cli_agent::Binding};
use std::{
    ffi::OsString,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

pub const ID: &str = crate::store::cli_bridge::ORCHESTRATION_POLICY;
pub const FLAGS: &[&str] = &[
    "--print",
    "--tools",
    "--allowedTools",
    "--permission-mode",
    "--permission-prompts",
    "--dangerously-skip-permissions",
    "--input-format",
    "--output-format",
    "--verbose",
    "--include-partial-messages",
    "--effort",
    "--model",
    "--session-id",
    "--resume",
    "--mcp-config",
    "--strict-mcp-config",
    "--settings",
    "--setting-sources",
    "--no-chrome",
    "--disable-slash-commands",
    // The documented file variant is hidden from ordinary help in CLI2.1.301.
    // Preflight also exercises that exact option with metadata-only auth status.
    "--system-prompt",
];
pub const MAX_PROMPT_BYTES: usize = pipkin_engine_protocol::limits::MAX_PROMPT_BYTES;
// SDK envelopes can echo a native frame with their own metadata. Cumulative display
// stays small enough that worst-case JSON escaping still fits a native event frame.
pub const MAX_STREAM_LINE_BYTES: usize = super::orchestration_protocol::MAX_FRAME_BYTES + 4096;
pub const MAX_PROJECTED_BYTES: usize = pipkin_engine_protocol::limits::MAX_FRAME_BYTES / 8;
pub const MAX_STDOUT_BYTES: usize = 256 * 1024 * 1024;
const MAX_SYSTEM: usize = 1024 * 1024;
const MAX_IMAGE: u64 = 2 * 1024 * 1024 * 1024;

/// Private copy of the running engine image, not a project/discovery executable or PID path.
/// Keeping the actual image avoids PID reuse/path replacement executing a different program
/// after owner failure. Capture once per owning engine lifetime; never mutate the source image.
pub struct HelperImage {
    directory: tempfile::TempDir,
    path: PathBuf,
    stamp: FileStamp,
}
impl HelperImage {
    /// Ignored live qualification uses the engine binary built from this workspace;
    /// the libtest executable itself has no helper-mode entry point.
    #[cfg(test)]
    pub(crate) fn capture_test_binary_in(
        parent: &Path,
        binary: &Path,
    ) -> Result<Arc<Self>, &'static str> {
        Self::copy_in(
            parent,
            File::open(binary).map_err(|_| "build the debug engine helper first")?,
        )
    }
    pub fn capture_in(parent: &Path) -> Result<Arc<Self>, &'static str> {
        if !cfg!(target_os = "linux") {
            return Err("native helper image capture is unsupported on this platform");
        }
        let input = File::open("/proc/self/exe").map_err(|_| "cannot open running helper image")?;
        Self::copy_in(parent, input)
    }
    fn copy_in(parent: &Path, input: File) -> Result<Arc<Self>, &'static str> {
        let meta = input
            .metadata()
            .map_err(|_| "cannot inspect helper image")?;
        if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_IMAGE {
            return Err("invalid helper image size or type");
        }
        let directory = private_dir(parent)?;
        let path = directory.path().join("helper");
        let mut out = private_file(&path, true)?;
        let copied = std::io::copy(&mut input.take(MAX_IMAGE + 1), &mut out)
            .map_err(|_| "cannot snapshot helper image")?;
        if copied == 0 || copied > MAX_IMAGE {
            return Err("helper image exceeds supported bounds");
        }
        out.sync_all().map_err(|_| "cannot finish helper image")?;
        let stamp = FileStamp::read(&path)?;
        Ok(Arc::new(Self {
            directory,
            path,
            stamp,
        }))
    }
    pub(crate) fn assets_parent(&self) -> &Path {
        self.directory.path()
    }
    fn validate(&self) -> Result<(), &'static str> {
        if FileStamp::read(&self.path)? != self.stamp || !private_directory(self.directory.path()) {
            return Err("owned helper image changed");
        }
        Ok(())
    }
}
fn private_dir(parent: &Path) -> Result<tempfile::TempDir, &'static str> {
    if !parent.is_absolute() {
        return Err("private launch directory must be absolute");
    }
    let dir = tempfile::Builder::new()
        .prefix("pk-native-")
        .tempdir_in(parent)
        .map_err(|_| "cannot create private launch directory")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
            .map_err(|_| "cannot protect private directory")?;
    }
    Ok(dir)
}
fn private_directory(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::symlink_metadata(path)
            .is_ok_and(|m| m.is_dir() && m.permissions().mode() & 0o777 == 0o700)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}
#[cfg(unix)]
fn private_file(path: &Path, executable: bool) -> Result<File, &'static str> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mode = if executable { 0o700 } else { 0o600 };
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
        .map_err(|_| "cannot create private launch file")?;
    file.set_permissions(std::fs::Permissions::from_mode(mode))
        .map_err(|_| "cannot protect launch file")?;
    Ok(file)
}
#[cfg(not(unix))]
fn private_file(_path: &Path, _executable: bool) -> Result<File, &'static str> {
    Err("private launch files unsupported on this platform")
}
fn save(path: &Path, bytes: &[u8]) -> Result<FileStamp, &'static str> {
    let mut file = private_file(path, false)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| "cannot finish private launch file")?;
    FileStamp::read(path)
}
/// Engine-owned ephemeral launch files, NOT edits to the installed CLI/user configuration.
/// No Debug/serialization: the MCP file contains the private channel capability.
pub struct Assets {
    directory: tempfile::TempDir,
    image: Arc<HelperImage>,
    launch: NativeLaunch,
    mcp: PathBuf,
    system: PathBuf,
    system_text: Arc<str>,
    mcp_stamp: FileStamp,
    system_stamp: FileStamp,
    system_digest: [u8; 32],
}
impl Assets {
    pub(crate) fn system_text(&self) -> &str {
        &self.system_text
    }
    pub(crate) fn image(&self) -> Arc<HelperImage> {
        self.image.clone()
    }
    pub(crate) fn matches_system(&self, system: &str) -> bool {
        use sha2::Digest;
        let digest: [u8; 32] = sha2::Sha256::digest(system.as_bytes()).into();
        digest == self.system_digest
    }
    pub fn new(
        parent: &Path,
        image: Arc<HelperImage>,
        launch: NativeLaunch,
        system: &str,
    ) -> Result<Self, &'static str> {
        if system.trim().is_empty() || system.len() > MAX_SYSTEM || system.contains('\0') {
            return Err("invalid native system snapshot");
        }
        image.validate()?;
        if launch.expired() {
            return Err("native channel expired");
        }
        let directory = private_dir(parent)?;
        let mcp = directory.path().join("mcp.json");
        let system_path = directory.path().join("system.txt");
        let config = serde_json::json!({"mcpServers":{"pipkin":{"type":"stdio","command":image.path.to_str().ok_or("helper path is not Unicode")?,
            "args":[super::orchestration_helper::FLAG],"env":{(super::orchestration_helper::CHANNEL_ENV):launch.environment_value()}}}});
        let mcp_stamp = save(
            &mcp,
            &serde_json::to_vec(&config)
                .map_err(|_| "cannot encode private launch configuration")?,
        )?;
        let system_stamp = save(&system_path, system.as_bytes())?;
        Ok(Self {
            directory,
            image,
            launch,
            mcp,
            system: system_path,
            system_text: Arc::from(system),
            mcp_stamp,
            system_stamp,
            system_digest: {
                use sha2::Digest;
                sha2::Sha256::digest(system.as_bytes()).into()
            },
        })
    }
    pub fn validate(&self, binding: &Binding) -> Result<(), &'static str> {
        if !self.launch.matches(binding) || self.launch.expired() {
            return Err("native launch binding changed or expired");
        }
        self.image.validate()?;
        if !private_directory(self.directory.path())
            || FileStamp::read(&self.mcp)? != self.mcp_stamp
            || FileStamp::read(&self.system)? != self.system_stamp
        {
            return Err("private launch snapshot changed");
        }
        Ok(())
    }
}
pub struct Turn<'a> {
    pub executable: &'a Path,
    pub cwd: &'a Path,
    pub environment: &'a Environment,
    pub binding: &'a Binding,
    pub scope: &'a super::orchestration_protocol::Scope,
    pub model: ModelAlias,
    pub mode: TurnMode,
    pub prompt: &'a str,
    pub images: &'a [pipkin_engine_protocol::ImageInput],
}
pub fn turn_spec(turn: Turn<'_>, assets: &Assets) -> Result<Spec, &'static str> {
    assets.validate(turn.binding)?;
    if !assets.launch.matches_scope(turn.scope)
        || turn.binding.create.identity.launch_policy != ID
        || turn.binding.create.model != turn.model.as_str()
        || super::orchestration_input::thinking(turn.binding.create.thinking.as_deref()).is_err()
        || Path::new(&turn.binding.create.identity.project_path) != turn.cwd
        || Path::new(&turn.binding.create.identity.executable_path) != turn.executable
        || !policy::valid_uuid(&turn.binding.create.identity.cli_session_id)
        || super::orchestration_input::validate(turn.prompt, turn.images).is_err()
    {
        return Err("invalid native CLI launch identity or prompt");
    }
    let env = policy::launch_environment(turn.environment)?;
    let mut args: Vec<OsString> = ["--mcp-config"].into_iter().map(Into::into).collect();
    args.push(assets.mcp.as_os_str().into());
    args.extend(
        [
            "--strict-mcp-config",
            "--setting-sources",
            "",
            "--settings",
            "{\"disableAllHooks\":true}",
            "--print",
            "--tools",
            "",
            "--allowedTools",
            "mcp__pipkin__*",
            "--permission-mode",
            "bypassPermissions",
            "--permission-prompts",
            "none",
            "--dangerously-skip-permissions",
            "--input-format",
            if turn.images.is_empty() {
                "text"
            } else {
                "stream-json"
            },
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--no-chrome",
            "--disable-slash-commands",
            "--system-prompt-file",
        ]
        .into_iter()
        .map(OsString::from),
    );
    args.push(assets.system.as_os_str().into());
    args.extend(
        [
            "--model",
            turn.model.as_str(),
            if turn.mode == TurnMode::Fresh {
                "--session-id"
            } else {
                "--resume"
            },
            &turn.binding.create.identity.cli_session_id,
        ]
        .into_iter()
        .map(OsString::from),
    );
    if let Some(effort) =
        super::orchestration_input::thinking(turn.binding.create.thinking.as_deref())?
    {
        args.extend([OsString::from("--effort"), OsString::from(effort)]);
    }
    let mut spec = policy::metadata_spec(turn.executable, turn.cwd, &env, args);
    spec.input = super::orchestration_input::wire(turn.prompt, turn.images)?;
    spec.mode = OutputMode::Stream;
    spec.timeout = Some(assets.launch.remaining().min(Duration::from_secs(30 * 60)));
    spec.max_stdout = MAX_STDOUT_BYTES;
    assets.validate(turn.binding)?;
    Ok(spec)
}
#[cfg(all(test, target_os = "linux"))]
mod tests;
