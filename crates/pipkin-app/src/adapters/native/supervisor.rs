//! Hosting the built-in engine for the desktop: an owned worker process (the default), or the
//! engine running inside this process (`--engine-host inprocess`).
//!
//! The worker is this same executable started with `--engine-worker`. It talks over private
//! stdin/stdout pipes carrying [`pipkin_engine_protocol::frame`] frames; its stderr goes to the
//! application's engine log file, never to a pipe that could fill. Only the one child this
//! supervisor spawned is ever waited on or killed: it is identified by its `Child` handle, which
//! is reaped before it is forgotten, so its pid cannot be reused under us. Restarts are capped
//! (three starts per minute) so a crash loop stops with a diagnosis instead of spinning.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use pipkin_engine::config::EngineConfig;
use pipkin_engine_protocol::frame::{self, FrameError};
use pipkin_engine_protocol::{ClientFrame, ServerFrame};

use super::client::{FrameReceiver, FrameSender, Link};

/// Hidden flag that turns this executable into the engine worker.
pub const WORKER_FLAG: &str = "--engine-worker";
const DATA_FLAG: &str = "--engine-data-dir";
const CONFIG_FLAG: &str = "--engine-config-dir";
/// Repeatable: a session extension (`builtin:<name>` or an executable path).
pub const EXTENSION_FLAG: &str = "--engine-extension";
pub const CLEAR_EXTENSIONS_FLAG: &str = "--engine-clear-extensions";

/// What the worker was asked to serve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerArgs {
    pub data_dir: PathBuf,
    pub config_dir: PathBuf,
    pub extensions: Vec<String>,
    pub clear_extensions: bool,
}

/// Starts allowed within `RESTART_WINDOW` before giving up.
const MAX_STARTS: usize = 3;
const RESTART_WINDOW: Duration = Duration::from_secs(60);
/// How long an orderly stop waits for the worker to exit before killing it.
pub const STOP_GRACE: Duration = Duration::from_secs(10);

/// Where the engine runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HostKind {
    /// A private worker process this application owns.
    #[default]
    Child,
    /// Inside this process, on the engine's own runtime threads.
    InProcess,
}

/// `--engine-worker --engine-data-dir <d> --engine-config-dir <c>`, if this run is the worker.
/// `None` when the flag is absent, so a normal launch never mistakes its arguments for these.
pub fn worker_args(args: &[String]) -> Option<Result<WorkerArgs, String>> {
    if !args.iter().any(|a| a == WORKER_FLAG) {
        return None;
    }
    let mut data = None;
    let mut config = None;
    let mut extensions = Vec::new();
    let mut clear_extensions = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) => (f, Some(v.to_owned())),
            None => (arg.as_str(), None),
        };
        if flag == CLEAR_EXTENSIONS_FLAG {
            if inline.is_some() {
                return Some(Err(format!("{flag} does not take a value")));
            }
            clear_extensions = true;
            continue;
        }
        if flag == EXTENSION_FLAG {
            match inline.or_else(|| it.next().cloned()) {
                Some(v) if !v.is_empty() => extensions.push(v),
                _ => return Some(Err(format!("{flag} needs a value"))),
            }
            continue;
        }
        let slot = match flag {
            WORKER_FLAG => continue,
            DATA_FLAG => &mut data,
            CONFIG_FLAG => &mut config,
            other => return Some(Err(format!("unknown worker argument {other:?}"))),
        };
        match inline.or_else(|| it.next().cloned()) {
            Some(v) if !v.is_empty() => *slot = Some(PathBuf::from(v)),
            _ => return Some(Err(format!("{flag} needs a value"))),
        }
    }
    if clear_extensions && !extensions.is_empty() {
        return Some(Err(
            "Cannot select extensions and clear the default together".into(),
        ));
    }
    Some(match (data, config) {
        (Some(data_dir), Some(config_dir)) => Ok(WorkerArgs {
            data_dir,
            config_dir,
            extensions,
            clear_extensions,
        }),
        _ => Err(format!("{WORKER_FLAG} needs {DATA_FLAG} and {CONFIG_FLAG}")),
    })
}

/// Foreground external server, before GPUI starts. Existing worker configuration flags
/// name its data/config roots and extensions; the endpoint is always explicit.
pub fn server_args(args: &[String]) -> Option<Result<(WorkerArgs, PathBuf), String>> {
    if !args
        .iter()
        .any(|a| a == "--engine-server" || a.starts_with("--engine-server="))
    {
        return None;
    }
    let mut endpoint = None;
    let mut worker = vec![WORKER_FLAG.to_owned()];
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if arg == WORKER_FLAG {
            return Some(Err(
                "--engine-server cannot be combined with --engine-worker".into(),
            ));
        }
        if arg == "--engine-server" || arg.starts_with("--engine-server=") {
            if endpoint.is_some() {
                return Some(Err("--engine-server may only be specified once".into()));
            }
            let value = arg
                .split_once('=')
                .map(|(_, v)| v.to_owned())
                .or_else(|| it.next().cloned());
            match value {
                Some(v) if !v.is_empty() && !v.starts_with("--") => {
                    endpoint = Some(PathBuf::from(v))
                }
                _ => return Some(Err("--engine-server needs an endpoint directory".into())),
            }
        } else {
            worker.push(arg.clone());
        }
    }
    Some(
        worker_args(&worker)
            .expect("worker flag provided")
            .map(|args| (args, endpoint.expect("endpoint parsed"))),
    )
}

pub fn run_server(args: WorkerArgs, endpoint: PathBuf) -> i32 {
    match engine_config(
        &args.data_dir,
        &args.config_dir,
        &args.extensions,
        args.clear_extensions,
    ) {
        Ok(config) => pipkin_engine::endpoint::run(config, &endpoint),
        Err(error) => {
            eprintln!("pipkin engine: {error}");
            2
        }
    }
}

/// Serve the engine over stdin/stdout until the desktop closes the channel. Returns the exit code.
pub fn run_worker(args: WorkerArgs) -> i32 {
    match engine_config(
        &args.data_dir,
        &args.config_dir,
        &args.extensions,
        args.clear_extensions,
    ) {
        Ok(config) => pipkin_engine::host::run_stdio(config),
        Err(e) => {
            eprintln!("pipkin engine: {e}");
            2
        }
    }
}

/// The engine configuration for these directories and extension specs.
pub fn engine_config(
    data_dir: &Path,
    config_dir: &Path,
    extensions: &[String],
    clear_extensions: bool,
) -> Result<EngineConfig, String> {
    let mut config = EngineConfig::new(data_dir, config_dir);
    if clear_extensions && !extensions.is_empty() {
        return Err("Cannot select extensions and clear the default together".into());
    }
    config.clear_extensions = clear_extensions;
    for spec in extensions {
        config
            .extensions
            .push(pipkin_engine::config::parse_extension(spec)?);
    }
    Ok(config)
}

/// The worker command line: program and arguments.
#[derive(Clone, Debug)]
pub struct WorkerSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// Where the worker's stderr is appended.
    pub log_path: PathBuf,
}

impl WorkerSpec {
    /// This executable as the engine worker for the given directories.
    pub fn engine(
        data_dir: &Path,
        config_dir: &Path,
        extensions: &[String],
        clear_extensions: bool,
        log_path: &Path,
    ) -> Result<WorkerSpec, String> {
        let program = std::env::current_exe()
            .map_err(|e| format!("cannot find the Pipkin executable to start the engine: {e}"))?;
        Ok(WorkerSpec {
            program,
            args: vec![
                WORKER_FLAG.into(),
                DATA_FLAG.into(),
                data_dir.as_os_str().to_owned(),
                CONFIG_FLAG.into(),
                config_dir.as_os_str().to_owned(),
            ]
            .into_iter()
            .chain(
                extensions
                    .iter()
                    .flat_map(|e| [EXTENSION_FLAG.into(), OsString::from(e)]),
            )
            .chain(clear_extensions.then(|| OsString::from(CLEAR_EXTENSIONS_FLAG)))
            .collect(),
            log_path: log_path.to_path_buf(),
        })
    }
}

#[derive(Clone)]
pub enum HostSpec {
    /// Attach to an independently owned local engine; never launch or stop its process.
    External(PathBuf),
    Child(WorkerSpec),
    InProcess {
        data_dir: PathBuf,
        config_dir: PathBuf,
        extensions: Vec<String>,
        clear_extensions: bool,
    },
    /// Tests: a link to a fake engine the test drives.
    #[cfg(test)]
    Fake(std::sync::Arc<dyn Fn() -> Result<Link, String> + Send + Sync>),
}

/// Frames to the worker go through a writer thread, so a send never blocks the adapter. Dropping
/// the last sender ends the thread, which closes the worker's stdin.
struct PipeSender(mpsc::SyncSender<ClientFrame>);

impl FrameSender for PipeSender {
    fn send(&mut self, frame: ClientFrame) -> Result<(), String> {
        self.0
            .try_send(frame)
            .map_err(|_| "the engine worker's input is closed".to_owned())
    }
}

fn pipe_sender(stdin: impl std::io::Write + Send + 'static) -> PipeSender {
    let (tx, rx) = mpsc::sync_channel::<ClientFrame>(64);
    thread::Builder::new()
        .name("native-engine-writer".into())
        .spawn(move || {
            let mut out = BufWriter::new(stdin);
            while let Ok(frame) = rx.recv() {
                if let Err(e) = frame::write(&mut out, &frame) {
                    log::warn!("cannot write to the engine worker: {e}");
                    break;
                }
            }
        })
        .expect("spawn engine writer");
    PipeSender(tx)
}

struct PipeReceiver(BufReader<ChildStdout>);

impl FrameReceiver for PipeReceiver {
    fn recv(&mut self) -> Result<ServerFrame, String> {
        frame::read::<ServerFrame>(&mut self.0).map_err(|e| match e {
            FrameError::Closed => "the engine worker exited".to_owned(),
            other => format!("the engine worker's output could not be read ({other})"),
        })
    }
}

#[cfg(unix)]
struct SocketReceiver(BufReader<std::os::unix::net::UnixStream>);
#[cfg(unix)]
impl FrameReceiver for SocketReceiver {
    fn recv(&mut self) -> Result<ServerFrame, String> {
        frame::read(&mut self.0).map_err(|e| format!("the external engine connection ended: {e}"))
    }
}

/// Starts, restarts (within a cap) and stops the engine.
pub struct Supervisor {
    spec: HostSpec,
    child: Option<Child>,
    inprocess: Option<pipkin_engine::host::EngineHandle>,
    external: Option<Box<dyn FnOnce() + Send>>,
    starts: Vec<Instant>,
    grace: Duration,
}

impl Supervisor {
    pub fn new(spec: HostSpec) -> Supervisor {
        Supervisor {
            spec,
            child: None,
            inprocess: None,
            external: None,
            starts: Vec::new(),
            grace: STOP_GRACE,
        }
    }

    #[cfg(test)]
    pub fn with_grace(mut self, grace: Duration) -> Supervisor {
        self.grace = grace;
        self
    }

    /// Whether the engine this supervisor started is still running.
    pub fn alive(&mut self) -> bool {
        match &mut self.child {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => self.inprocess.is_some() || self.external.is_some(),
        }
    }

    /// The pid of the worker, while there is one.
    #[cfg(test)]
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    /// Start the engine (stopping a dead or previous one first) and connect to it.
    pub fn start(&mut self) -> Result<Link, String> {
        self.stop();
        if let HostSpec::External(directory) = self.spec.clone() {
            return self.attach(&directory);
        }
        self.starts.retain(|t| t.elapsed() < RESTART_WINDOW);
        if self.starts.len() >= MAX_STARTS {
            return Err(format!(
                "the engine stopped {} times in {} seconds; see {} for details",
                self.starts.len(),
                RESTART_WINDOW.as_secs(),
                self.log_location()
            ));
        }
        self.starts.push(Instant::now());
        match self.spec.clone() {
            HostSpec::External(_) => unreachable!("handled before process restart limiting"),
            HostSpec::Child(spec) => self.spawn(&spec),
            HostSpec::InProcess {
                data_dir,
                config_dir,
                extensions,
                clear_extensions,
            } => {
                let handle = pipkin_engine::host::start(engine_config(
                    &data_dir,
                    &config_dir,
                    &extensions,
                    clear_extensions,
                )?)
                .map_err(|e| format!("cannot start the engine: {e}"))?;
                let (sender, receiver) = handle.connect("pipkin-desktop").split();
                self.inprocess = Some(handle);
                Ok(Link {
                    sender: Box::new(sender),
                    receiver: Box::new(receiver),
                })
            }
            #[cfg(test)]
            HostSpec::Fake(make) => make(),
        }
    }

    pub fn owns_engine(&self) -> bool {
        !matches!(self.spec, HostSpec::External(_))
    }

    fn attach(&mut self, directory: &Path) -> Result<Link, String> {
        #[cfg(unix)]
        {
            let stream = pipkin_engine::endpoint::connect(directory)
                .map_err(|e| format!("cannot attach to {}: {e}", directory.display()))?;
            let control = stream.try_clone().map_err(|e| e.to_string())?;
            let writer = stream.try_clone().map_err(|e| e.to_string())?;
            self.external = Some(Box::new(move || {
                let _ = control.shutdown(std::net::Shutdown::Both);
            }));
            Ok(Link {
                sender: Box::new(pipe_sender(writer)),
                receiver: Box::new(SocketReceiver(BufReader::new(stream))),
            })
        }
        #[cfg(not(unix))]
        {
            let _ = directory;
            Err("external socket attachment is supported on Unix hosts only".into())
        }
    }

    fn log_location(&self) -> String {
        match &self.spec {
            HostSpec::Child(spec) => spec.log_path.display().to_string(),
            _ => "the application log".into(),
        }
    }

    fn spawn(&mut self, spec: &WorkerSpec) -> Result<Link, String> {
        if let Some(dir) = spec.log_path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        let log: File = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&spec.log_path)
            .map_err(|e| format!("cannot open {}: {e}", spec.log_path.display()))?;
        let mut child = Command::new(&spec.program)
            .args(&spec.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(log))
            .spawn()
            .map_err(|e| format!("cannot start the engine worker: {e}"))?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("the engine worker has no private channel".into());
        };
        self.child = Some(child);
        Ok(Link {
            sender: Box::new(pipe_sender(stdin)),
            receiver: Box::new(PipeReceiver(BufReader::new(stdout))),
        })
    }

    /// Stop what this supervisor started. The caller first asks the engine to shut down and
    /// closes its connection; this then waits up to the grace period for the worker to exit and
    /// kills it only if it has not. Idempotent.
    pub fn stop(&mut self) {
        if let Some(disconnect) = self.external.take() {
            disconnect();
        }
        if let Some(mut child) = self.child.take() {
            let deadline = Instant::now() + self.grace;
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        if !status.success() {
                            log::warn!("the engine worker exited with {status}");
                        }
                        break;
                    }
                    Ok(None) if Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(20));
                    }
                    Ok(None) => {
                        log::warn!("the engine worker did not stop in time; stopping it");
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                    Err(e) => {
                        log::warn!("cannot check the engine worker: {e}");
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                }
            }
        }
        if let Some(handle) = self.inprocess.take() {
            handle.shutdown(self.grace);
        }
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use pipkin_engine_protocol::{Call, PROTOCOL_VERSION, Reply, Welcome};

    use super::super::client::{CallError, Client, Inbound};
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn external_server_arguments_require_explicit_endpoint_and_engine_directories() {
        let good = server_args(&args(&[
            "--engine-server",
            "/endpoint",
            "--engine-data-dir",
            "/data",
            "--engine-config-dir",
            "/config",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(good.1, PathBuf::from("/endpoint"));
        assert_eq!(good.0.data_dir, PathBuf::from("/data"));
        assert!(server_args(&args(&["--engine-server"])).unwrap().is_err());
        assert!(
            server_args(&args(&["--engine-server", "--engine-data-dir", "/data"]))
                .unwrap()
                .is_err()
        );
        assert!(
            server_args(&args(&["--engine-server", "/endpoint", "--engine-worker"]))
                .unwrap()
                .is_err()
        );
        assert!(
            server_args(&args(&["--engine-server", "/endpoint"]))
                .unwrap()
                .is_err()
        );
        assert!(server_args(&args(&["--engine", "native"])).is_none());
    }

    #[test]
    fn worker_arguments_are_recognised_only_with_the_flag() {
        assert!(worker_args(&args(&["--demo", "normal"])).is_none());
        assert_eq!(
            worker_args(&args(&[
                "--engine-worker",
                "--engine-data-dir",
                "/d",
                "--engine-config-dir=/c"
            ])),
            Some(Ok(WorkerArgs {
                data_dir: PathBuf::from("/d"),
                config_dir: PathBuf::from("/c"),
                extensions: vec![],
                clear_extensions: false,
            }))
        );
        assert!(matches!(
            worker_args(&args(&["--engine-worker", "--engine-data-dir", "/d"])),
            Some(Err(_))
        ));
        assert!(matches!(
            worker_args(&args(&["--engine-worker", "--engine-data-dir"])),
            Some(Err(_))
        ));
        assert!(matches!(
            worker_args(&args(&["--engine-worker", "--demo", "x"])),
            Some(Err(_))
        ));
    }

    #[test]
    fn the_worker_command_is_this_executable_with_private_directories() {
        let extensions = vec!["builtin:questions".to_owned(), "/x/ext".to_owned()];
        let spec = WorkerSpec::engine(
            Path::new("/d"),
            Path::new("/c"),
            &extensions,
            false,
            Path::new("/l"),
        )
        .unwrap();
        assert_eq!(spec.program, std::env::current_exe().unwrap());
        let args: Vec<String> = spec
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            worker_args(&args),
            Some(Ok(WorkerArgs {
                data_dir: PathBuf::from("/d"),
                config_dir: PathBuf::from("/c"),
                extensions,
                clear_extensions: false,
            }))
        );
    }

    #[test]
    fn clearing_extension_defaults_reaches_worker_configuration_without_attached_or_mixed_selection()
     {
        let spec = WorkerSpec::engine(Path::new("/d"), Path::new("/c"), &[], true, Path::new("/l"))
            .unwrap();
        let args: Vec<String> = spec
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let parsed = worker_args(&args).unwrap().unwrap();
        assert!(parsed.clear_extensions);
        assert!(parsed.extensions.is_empty());
        assert!(
            engine_config(
                &parsed.data_dir,
                &parsed.config_dir,
                &parsed.extensions,
                parsed.clear_extensions
            )
            .unwrap()
            .clear_extensions
        );
        let mut mixed = args.clone();
        mixed.extend([EXTENSION_FLAG.into(), "builtin:questions".into()]);
        assert!(worker_args(&mixed).unwrap().is_err());
        assert!(
            engine_config(
                Path::new("/d"),
                Path::new("/c"),
                &["builtin:questions".into()],
                true
            )
            .is_err()
        );
    }

    /// A shell script standing in for the worker. Run through `sh` rather than executed, so a
    /// concurrent fork in another test cannot make it "text file busy".
    fn script(dir: &Path, name: &str, body: &str) -> WorkerSpec {
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        WorkerSpec {
            program: PathBuf::from("/bin/sh"),
            args: vec![path.into_os_string(), "--engine-worker".into()],
            log_path: dir.join("logs").join("engine.log"),
        }
    }

    /// `printf` arguments writing exactly these bytes.
    fn printf_bytes(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("\\{b:03o}")).collect()
    }

    fn welcome_frame(id: u64) -> Vec<u8> {
        frame::encode(&ServerFrame::Reply {
            id,
            result: Ok(Reply::Welcome(Welcome {
                protocol: PROTOCOL_VERSION,
                engine_version: "fake".into(),
                engine_revision: "fake".into(),
                schema: 1,
                epoch: 9,
                capabilities: vec![],
            })),
        })
        .unwrap()
    }

    fn connect(link: Link) -> (Client, mpsc::Receiver<Inbound>) {
        let (tx, rx) = mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        let client = Client::start(link, move |i| {
            let _ = tx.lock().unwrap().send(i);
        });
        (client, rx)
    }

    fn hello() -> Call {
        Call::Hello {
            protocol: PROTOCOL_VERSION,
            client: "test".into(),
        }
    }

    #[test]
    fn a_worker_speaks_frames_logs_stderr_and_stops_when_its_input_closes() {
        let dir = tempfile::tempdir().unwrap();
        let spec = script(
            dir.path(),
            "worker.sh",
            &format!(
                "echo \"worker started $*\" >&2\nprintf '{}'\ncat > /dev/null\necho 'worker input closed' >&2\nexit 0",
                printf_bytes(&welcome_frame(1))
            ),
        );
        let log = spec.log_path.clone();
        let mut supervisor =
            Supervisor::new(HostSpec::Child(spec)).with_grace(Duration::from_secs(5));
        let link = supervisor.start().unwrap();
        assert!(supervisor.alive());
        let (client, inbound) = connect(link);
        let reply = client.call(hello(), Duration::from_secs(10)).unwrap();
        assert!(matches!(reply, Reply::Welcome(Welcome { epoch: 9, .. })));
        // The orderly path: no answer to Shutdown from this fake, then the input closes.
        assert!(matches!(
            client.call(Call::Shutdown, Duration::from_millis(100)),
            Err(CallError::Lost(_))
        ));
        client.close();
        let started = Instant::now();
        supervisor.stop();
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "it exited on its own"
        );
        assert!(!supervisor.alive());
        assert!(matches!(
            inbound.recv_timeout(Duration::from_secs(5)).unwrap(),
            Inbound::Closed(_)
        ));
        let text = fs::read_to_string(log).unwrap();
        assert!(text.contains("worker started --engine-worker"), "{text}");
        assert!(text.contains("worker input closed"), "{text}");
    }

    #[test]
    fn a_worker_that_ignores_shutdown_is_killed_after_the_grace_period() {
        let dir = tempfile::tempdir().unwrap();
        let spec = script(
            dir.path(),
            "stubborn.sh",
            "trap '' TERM HUP INT\nexec sleep 30",
        );
        let mut supervisor =
            Supervisor::new(HostSpec::Child(spec)).with_grace(Duration::from_millis(200));
        let link = supervisor.start().unwrap();
        let pid = supervisor.pid().unwrap();
        let (client, _inbound) = connect(link);
        client.close();
        let started = Instant::now();
        supervisor.stop();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(!supervisor.alive());
        // The process is gone (reaped), not merely signalled.
        assert!(!Path::new(&format!("/proc/{pid}")).exists() || cfg!(not(target_os = "linux")));
    }

    #[test]
    fn worker_death_is_seen_and_restarts_are_capped() {
        let dir = tempfile::tempdir().unwrap();
        let spec = script(dir.path(), "dies.sh", "echo 'boom' >&2\nexit 3");
        let mut supervisor =
            Supervisor::new(HostSpec::Child(spec)).with_grace(Duration::from_secs(2));
        for _ in 0..MAX_STARTS {
            let link = supervisor.start().unwrap();
            let (client, inbound) = connect(link);
            assert!(matches!(
                inbound.recv_timeout(Duration::from_secs(5)).unwrap(),
                Inbound::Closed(_)
            ));
            assert!(matches!(
                client.call(hello(), Duration::from_secs(1)),
                Err(CallError::NotSent(_))
            ));
            let deadline = Instant::now() + Duration::from_secs(5);
            while supervisor.alive() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            assert!(!supervisor.alive());
        }
        let error = match supervisor.start() {
            Err(e) => e,
            Ok(_) => panic!("a crash loop must stop"),
        };
        assert!(error.contains("stopped 3 times"), "{error}");
        assert!(error.contains("engine.log"), "{error}");
    }

    #[test]
    fn a_missing_worker_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        let spec = WorkerSpec {
            program: dir.path().join("missing"),
            args: vec![],
            log_path: dir.path().join("engine.log"),
        };
        let mut supervisor = Supervisor::new(HostSpec::Child(spec));
        let error = match supervisor.start() {
            Err(e) => e,
            Ok(_) => panic!("nothing to start"),
        };
        assert!(error.contains("cannot start the engine worker"), "{error}");
    }

    #[test]
    fn the_in_process_engine_answers_hello() {
        let dir = tempfile::tempdir().unwrap();
        let mut supervisor = Supervisor::new(HostSpec::InProcess {
            data_dir: dir.path().join("engine"),
            config_dir: dir.path().join("engine-config"),
            extensions: vec![],
            clear_extensions: false,
        });
        let link = supervisor.start().unwrap();
        assert!(supervisor.alive());
        let (client, _inbound) = connect(link);
        let reply = client.call(hello(), Duration::from_secs(10)).unwrap();
        assert!(
            matches!(reply, Reply::Welcome(Welcome { protocol, .. }) if protocol == PROTOCOL_VERSION)
        );
        client.close();
        supervisor.stop();
        assert!(!supervisor.alive());
    }
}
