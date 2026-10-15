//! Read-only Git observation, without repository-configured executable diff/clean drivers.
//! Discovery and name listing use only non-content commands. Content comparison runs with a
//! private gitdir/config/index and captured HEAD/object store, never the source configuration.
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;

const LIMIT: usize = 16 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(10);

pub(super) struct Reader {
    dir: PathBuf,
    root: PathBuf,
    objects: PathBuf,
    private: tempfile::TempDir,
    pub base: String,
}

impl Reader {
    pub fn open(dir: &Path) -> Result<Self, String> {
        let dir = dir
            .canonicalize()
            .map_err(|_| "Project folder is unavailable.")?;
        let (info, cut) = run(
            command(&dir),
            &[
                "rev-parse",
                "--path-format=absolute",
                "--show-toplevel",
                "--git-path",
                "objects",
                "--git-path",
                "index",
                "--show-object-format",
            ],
            16 * 1024,
        )?;
        let info = std::str::from_utf8(&info).map_err(|_| "Unsupported Git path encoding.")?;
        let fields: Vec<_> = info.lines().collect();
        if cut || fields.len() != 4 || fields[..3].iter().any(|s| !Path::new(s).is_absolute()) {
            return Err("Git did not identify a bounded project workspace.".into());
        }
        let root = PathBuf::from(fields[0]);
        if !dir.starts_with(
            root.canonicalize()
                .map_err(|_| "Git worktree is unavailable.")?,
        ) {
            return Err("Git worktree does not contain the captured project.".into());
        }
        let (format, width) = match fields[3] {
            "sha1" => ("", 40),
            "sha256" => ("[extensions]\nobjectFormat = sha256\n", 64),
            _ => return Err("Unsupported Git object format.".into()),
        };
        let private = tempfile::Builder::new()
            .prefix("pipkin-git-")
            .tempdir()
            .map_err(|_| "Cannot prepare private Git observation.")?;
        let prepare = || -> std::io::Result<()> {
            fs::create_dir(private.path().join("objects"))?;
            fs::create_dir(private.path().join("refs"))?;
            fs::write(
                private.path().join("config"),
                format!(
                    "[core]\nrepositoryFormatVersion = 1\nbare = false\nattributesFile = /dev/null\n{format}"
                ),
            )?;
            match regular(Path::new(fields[2])) {
                Ok(file) => {
                    // Git's racy-clean protection depends on the index timestamp. A fresh
                    // copy timestamp can make same-size, same-tick edits appear unchanged.
                    let modified = file.metadata()?.modified()?;
                    let mut index = vec![];
                    file.take(LIMIT as u64 + 1).read_to_end(&mut index)?;
                    if index.len() > LIMIT {
                        return Err(std::io::Error::other("index too large"));
                    }
                    let mut copy = fs::File::create(private.path().join("index"))?;
                    copy.write_all(&index)?;
                    copy.set_times(fs::FileTimes::new().set_modified(modified))?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e),
            }
            Ok(())
        };
        prepare().map_err(|_| "Cannot capture a bounded regular Git index.")?;
        let head = run(command(&dir), &["rev-parse", "--verify", "HEAD"], 128)
            .ok()
            .and_then(|(bytes, cut)| {
                let head = String::from_utf8(bytes).ok()?.trim().to_owned();
                (!cut && head.len() == width && head.bytes().all(|b| b.is_ascii_hexdigit()))
                    .then_some(head)
            });
        if head.is_none() {
            // Do not turn an arbitrary HEAD/storage failure into an empty baseline.
            let (symbolic, cut) = run(command(&dir), &["symbolic-ref", "--quiet", "HEAD"], 1024)?;
            let symbolic = std::str::from_utf8(&symbolic)
                .map_err(|_| "Invalid Git branch identity.")?
                .trim();
            if cut || !symbolic.starts_with("refs/heads/") || symbolic.contains(['\n', '\r']) {
                return Err("Invalid Git branch identity.".into());
            }
            let (refs, cut) = run(
                command(&dir),
                &["for-each-ref", "--format=%(objectname)", "--", symbolic],
                128,
            )?;
            if cut || !refs.is_empty() {
                return Err("Git HEAD could not be verified.".into());
            }
        }
        fs::write(
            private.path().join("HEAD"),
            head.as_deref().unwrap_or("ref: refs/heads/unborn"),
        )
        .map_err(|_| "Cannot capture Git HEAD.")?;
        let mut reader = Self {
            dir,
            root,
            objects: fields[1].into(),
            private,
            base: head.unwrap_or_default(),
        };
        if reader.base.is_empty() {
            let (hash, cut) = reader.run(&["hash-object", "-t", "tree", "--stdin"], 128)?;
            let hash = String::from_utf8(hash).map_err(|_| "Invalid empty-tree identity.")?;
            let hash = hash.trim();
            if cut || hash.len() != width || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("Invalid empty-tree identity.".into());
            }
            reader.base = hash.into();
        }
        Ok(reader)
    }

    pub fn run(&self, args: &[&str], limit: usize) -> Result<(Vec<u8>, bool), String> {
        let mut cmd = command(&self.dir);
        cmd.arg("--git-dir")
            .arg(self.private.path())
            .arg("--work-tree")
            .arg(&self.root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_ATTR_NOSYSTEM", "1")
            .env("GIT_OBJECT_DIRECTORY", &self.objects);
        run(cmd, args, limit)
    }

    /// Names only, so source ignore rules are honored without invoking content filters.
    pub fn untracked(&self, args: &[&str], limit: usize) -> Result<(Vec<u8>, bool), String> {
        debug_assert!(args.contains(&"ls-files"));
        run(command(&self.dir), args, limit)
    }
}

/// Never follow the final symlink or block opening a replaced FIFO on Unix.
pub(super) fn regular(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    Ok(file)
}

fn command(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.env_clear();
    // No inherited Git-directory/config injection or dynamic-loader overrides. HOME/XDG
    // preserve ignore rules in discovery/listing only; isolated content scans disable them.
    for key in ["PATH", "HOME", "XDG_CONFIG_HOME", "SYSTEMROOT"] {
        if let Some(value) = std::env::var_os(key) {
            cmd.env(key, value);
        }
    }
    cmd.args([
        "--no-pager",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.untrackedCache=false",
    ])
    .current_dir(dir)
    .env("GIT_OPTIONAL_LOCKS", "0")
    .env("GIT_NO_LAZY_FETCH", "1")
    .env("LC_ALL", "C")
    .env("GIT_TERMINAL_PROMPT", "0")
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    cmd
}

fn bounded(mut input: impl Read, limit: usize) -> std::io::Result<(Vec<u8>, bool)> {
    let mut out = vec![];
    let mut buffer = [0; 16384];
    let mut cut = false;
    loop {
        match input.read(&mut buffer) {
            Ok(0) => break,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
            Ok(n) => {
                let take = n.min(limit.saturating_sub(out.len()));
                out.extend_from_slice(&buffer[..take]);
                cut |= take < n;
            }
        }
    }
    Ok((out, cut))
}

fn run(mut cmd: Command, args: &[&str], limit: usize) -> Result<(Vec<u8>, bool), String> {
    let mut child = cmd
        .args(args)
        .spawn()
        .map_err(|_| "Cannot start Git observation.")?;
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let out = std::thread::spawn(move || bounded(stdout, limit));
    let err = std::thread::spawn(move || bounded(stderr, 4096));
    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let output = out.join();
    let error = err.join();
    let output = output
        .map_err(|_| "Git reader failed.")?
        .map_err(|_| "Git output was incomplete.")?;
    let (error, _) = error
        .map_err(|_| "Git reader failed.")?
        .map_err(|_| "Git diagnostic output was incomplete.")?;
    match status {
        Some(status) if status.success() => Ok(output),
        Some(_) if String::from_utf8_lossy(&error).contains("not a git repository") => {
            Err("not a git repository".into())
        }
        Some(_) => Err(
            "Git could not inspect this project (configuration, history or permissions).".into(),
        ),
        None => Err("Git observation timed out.".into()),
    }
}
