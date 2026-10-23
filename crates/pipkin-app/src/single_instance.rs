//! Linux desktop rendezvous. Acquire before storage or engine startup; worker/diagnostic
//! entry points bypass this entirely. The kernel releases both lock and socket on exit.
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        linux::net::SocketAddrExt,
        unix::{
            fs::{MetadataExt, OpenOptionsExt},
            net::{SocketAddr, UnixListener, UnixStream},
        },
    },
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const MAX_FRAME: usize = 64 * 1024;
const TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LaunchRequest {
    pub projects: Vec<String>,
}
impl LaunchRequest {
    pub fn new(projects: &[std::path::PathBuf]) -> Self {
        Self {
            projects: projects
                .iter()
                .filter_map(|path| match fs::canonicalize(path) {
                    Ok(path) if path.is_dir() => Some(path.to_string_lossy().into_owned()),
                    _ => {
                        eprintln!(
                            "pipkin: --project {} is not a folder; ignoring it",
                            path.display()
                        );
                        None
                    }
                })
                .collect(),
        }
    }
}

pub enum Launch {
    Primary(Instance),
    Forwarded,
}
pub struct Instance {
    // Keep the lock until the GUI and its orderly engine shutdown have returned.
    _lock: File,
    stop: Arc<AtomicBool>,
    listener: Option<thread::JoinHandle<()>>,
    pub requests: async_channel::Receiver<LaunchRequest>,
}
impl Drop for Instance {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
        }
    }
}

pub fn acquire(data_dir: &Path, request: LaunchRequest) -> Result<Launch, String> {
    acquire_inner(data_dir, request).map_err(|e| format!("Could not open the existing Pipkin window: {e}. Quit the running Pipkin instance and try again; no second engine was started."))
}

fn acquire_inner(data_dir: &Path, request: LaunchRequest) -> io::Result<Launch> {
    fs::create_dir_all(data_dir)?;
    // O_NOFOLLOW prevents a replaced lock symlink from selecting an unrelated inode.
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(data_dir.join("desktop.lock"))?;
    let metadata = lock.metadata()?;
    if !metadata.is_file() || metadata.uid() != unsafe { libc::geteuid() } || metadata.nlink() != 1
    {
        return Err(io::Error::other(
            "desktop lock is not an owned regular file",
        ));
    }
    // Inode identity also handles relative paths/symlink aliases. Abstract sockets avoid
    // Unix path-length limits and stale socket files without removing a live owner's lock.
    let address = SocketAddr::from_abstract_name(format!(
        "pipkin-desktop-{}-{}-{}",
        metadata.uid(),
        metadata.dev(),
        metadata.ino()
    ))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        // SAFETY: lock owns a valid descriptor; flock receives no pointers.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            let listener = UnixListener::bind_addr(&address)?;
            listener.set_nonblocking(true)?;
            let (tx, requests) = async_channel::bounded(16);
            let stop = Arc::new(AtomicBool::new(false));
            let stopped = stop.clone();
            let listener =
                thread::Builder::new()
                    .name("pipkin-launch".into())
                    .spawn(move || {
                        while !stopped.load(Ordering::Relaxed) {
                            match listener.accept() {
                                Ok((mut stream, _)) => {
                                    let result = (|| -> io::Result<()> {
                                        same_user(&stream)?;
                                        stream.set_read_timeout(Some(TIMEOUT))?;
                                        stream.set_write_timeout(Some(TIMEOUT))?;
                                        let request: LaunchRequest =
                                            serde_json::from_slice(&read_frame(&mut stream)?)
                                                .map_err(io::Error::other)?;
                                        if request.projects.len() > 64
                                            || request.projects.iter().any(|p| {
                                                !Path::new(p).is_absolute() || p.contains('\0')
                                            })
                                        {
                                            return Err(io::Error::other("invalid launch request"));
                                        }
                                        tx.try_send(request).map_err(io::Error::other)?;
                                        stream.write_all(b"OK")
                                    })();
                                    if let Err(error) = result {
                                        log::warn!("desktop launch request refused: {error}");
                                    }
                                }
                                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                                    thread::sleep(Duration::from_millis(25))
                                }
                                Err(error) => {
                                    log::warn!("desktop launch listener: {error}");
                                    break;
                                }
                            }
                        }
                    })?;
            return Ok(Launch::Primary(Instance {
                _lock: lock,
                stop,
                listener: Some(listener),
                requests,
            }));
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::WouldBlock {
            return Err(error);
        }
        match UnixStream::connect_addr(&address) {
            Ok(mut stream) => {
                same_user(&stream)?;
                stream.set_read_timeout(Some(TIMEOUT))?;
                stream.set_write_timeout(Some(TIMEOUT))?;
                let frame = serde_json::to_vec(&request).map_err(io::Error::other)?;
                if frame.len() > MAX_FRAME {
                    return Err(io::Error::other("too many project paths"));
                }
                stream.write_all(&(frame.len() as u32).to_be_bytes())?;
                stream.write_all(&frame)?;
                let mut ack = [0; 2];
                stream.read_exact(&mut ack)?;
                return if &ack == b"OK" {
                    Ok(Launch::Forwarded)
                } else {
                    Err(io::Error::other("invalid launch acknowledgment"))
                };
            }
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_) => thread::sleep(Duration::from_millis(25)),
        }
    }
}
fn read_frame(stream: &mut UnixStream) -> io::Result<Vec<u8>> {
    let mut size = [0; 4];
    stream.read_exact(&mut size)?;
    let size = u32::from_be_bytes(size) as usize;
    if size > MAX_FRAME {
        return Err(io::Error::other("launch request too large"));
    }
    let mut frame = vec![0; size];
    stream.read_exact(&mut frame)?;
    Ok(frame)
}
fn same_user(stream: &UnixStream) -> io::Result<()> {
    let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of_val(&credentials) as libc::socklen_t;
    // SAFETY: pointers refer to initialized writable storage of the supplied size.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    if credentials.uid != unsafe { libc::geteuid() } {
        return Err(io::Error::other("launch peer is a different user"));
    }
    Ok(())
}

/// GPUI requests standard platform activation. Hyprland additionally supports explicit
/// activation of our own PID, avoiding stale Wayland input serials on a launcher click.
pub fn raise_on_hyprland() {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        return;
    }
    thread::spawn(|| {
        let Ok(mut child) = std::process::Command::new("hyprctl")
            .args([
                "dispatch",
                &format!(
                    "hl.dsp.focus({{ window = \"pid:{}\" }})",
                    std::process::id()
                ),
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        else {
            return;
        };
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                _ if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                _ => thread::sleep(Duration::from_millis(25)),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> LaunchRequest {
        LaunchRequest { projects: vec![] }
    }
    #[test]
    fn second_launch_forwards_projects_without_owning_the_profile() {
        let dir = tempfile::tempdir().unwrap();
        let Launch::Primary(owner) = acquire(dir.path(), request()).unwrap() else {
            panic!()
        };
        let project = LaunchRequest::new(&[dir.path().into()]);
        let expected = project.projects.clone();
        assert!(matches!(
            acquire(dir.path(), project).unwrap(),
            Launch::Forwarded
        ));
        assert_eq!(owner.requests.recv_blocking().unwrap().projects, expected);
        drop(owner);
        assert!(matches!(
            acquire(dir.path(), request()).unwrap(),
            Launch::Primary(_)
        ));
    }
    #[test]
    fn separate_profiles_are_independent_but_symlink_aliases_reuse_the_owner() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let Launch::Primary(owner) = acquire(a.path(), request()).unwrap() else {
            panic!()
        };
        let _other = acquire(b.path(), request()).unwrap();
        let alias = b.path().join("alias");
        std::os::unix::fs::symlink(a.path(), &alias).unwrap();
        assert!(matches!(
            acquire(&alias, request()).unwrap(),
            Launch::Forwarded
        ));
        owner.requests.recv_blocking().unwrap();
    }
    #[test]
    fn simultaneous_launches_elect_one_primary() {
        let dir = tempfile::tempdir().unwrap();
        thread::scope(|scope| {
            let gate = Arc::new(std::sync::Barrier::new(6));
            let threads: Vec<_> = (0..6)
                .map(|_| {
                    let gate = gate.clone();
                    let path = dir.path();
                    scope.spawn(move || {
                        gate.wait();
                        acquire(path, request()).unwrap()
                    })
                })
                .collect();
            let launches: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
            assert_eq!(
                launches
                    .iter()
                    .filter(|l| matches!(l, Launch::Primary(_)))
                    .count(),
                1
            );
        });
    }
    #[test]
    fn crashed_owner_does_not_leave_a_stale_instance() {
        struct Child(std::process::Child);
        impl Drop for Child {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let mut child = Child(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "single_instance::tests::crash_helper"])
                .env("PIPKIN_INSTANCE_TEST_DIR", dir.path())
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while !dir.path().join("ready").exists() {
            assert!(Instant::now() < deadline, "helper did not become ready");
            thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(
            acquire(dir.path(), request()).unwrap(),
            Launch::Forwarded
        ));
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        assert!(matches!(
            acquire(dir.path(), request()).unwrap(),
            Launch::Primary(_)
        ));
    }
    #[test]
    fn crash_helper() {
        let Some(dir) = std::env::var_os("PIPKIN_INSTANCE_TEST_DIR") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        let _owner = acquire(&dir, request()).unwrap();
        fs::write(dir.join("ready"), "ready").unwrap();
        loop {
            thread::park();
        }
    }
    #[test]
    fn oversized_request_is_refused_without_poisoning_the_listener() {
        let dir = tempfile::tempdir().unwrap();
        let Launch::Primary(owner) = acquire(dir.path(), request()).unwrap() else {
            panic!()
        };
        let meta = fs::metadata(dir.path().join("desktop.lock")).unwrap();
        let address = SocketAddr::from_abstract_name(format!(
            "pipkin-desktop-{}-{}-{}",
            meta.uid(),
            meta.dev(),
            meta.ino()
        ))
        .unwrap();
        let mut stream = UnixStream::connect_addr(&address).unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        stream
            .write_all(&((MAX_FRAME + 1) as u32).to_be_bytes())
            .unwrap();
        let mut response = [0; 2];
        assert_eq!(stream.read(&mut response).unwrap(), 0);
        assert!(owner.requests.is_empty());
        assert!(matches!(
            acquire(dir.path(), request()).unwrap(),
            Launch::Forwarded
        ));
        owner.requests.recv_blocking().unwrap();
    }
}
