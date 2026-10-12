use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read};
use std::net::Shutdown;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::host::EngineHandle;
use pipkin_engine_protocol::{
    Call, ClientFrame, ErrorCode, PROTOCOL_VERSION, ProtoError, ServerFrame, frame,
};

const SOCKET: &str = "engine.sock";
const MAX_CLIENTS: usize = 32;
const HELLO_LIMIT: usize = 4096;
const HELLO_TIMEOUT: Duration = Duration::from_secs(3);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

type Peers = Arc<Mutex<HashMap<u64, UnixStream>>>;

fn denied(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}
fn owner() -> u32 {
    // SAFETY: geteuid has no pointer arguments or preconditions.
    unsafe { libc::geteuid() }
}
fn private_metadata(meta: &fs::Metadata) -> io::Result<()> {
    if meta.uid() != owner() || meta.mode() & 0o077 != 0 {
        return Err(denied(
            "endpoint files must belong to this user and deny group/other access",
        ));
    }
    Ok(())
}
fn directory(path: &Path, create: bool) -> io::Result<PathBuf> {
    if create {
        match fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
        {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(denied("endpoint directory must not be a symlink"));
    }
    private_metadata(&meta)?;
    path.canonicalize()
}

/// Verify the connected process, not just a potentially replaced filesystem entry.
fn verify_peer(stream: &UnixStream) -> io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let uid = {
        let mut credentials = std::mem::MaybeUninit::<libc::ucred>::uninit();
        let mut size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: descriptor is live and the output buffer/length match SO_PEERCRED.
        let rc = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                credentials.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        if size as usize != std::mem::size_of::<libc::ucred>() {
            return Err(denied("invalid peer credentials"));
        }
        // SAFETY: getsockopt initialized the full buffer, checked above.
        unsafe { credentials.assume_init().uid }
    };
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    let uid = {
        let (mut uid, mut gid) = (0, 0);
        // SAFETY: descriptor is live and output pointers refer to uid_t/gid_t values.
        if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
            return Err(io::Error::last_os_error());
        }
        uid
    };
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    )))]
    let uid = {
        return Err(denied("peer credentials are unsupported on this platform"));
        #[allow(unreachable_code)]
        0
    };
    require_owner(uid)
}

fn require_owner(uid: u32) -> io::Result<()> {
    if uid != owner() {
        return Err(denied("endpoint peer belongs to another user"));
    }
    Ok(())
}

/// Connect only to an explicit, private socket owned by the same OS user. Callers then send
/// Hello before other engine frames. No engine is launched if the endpoint is unavailable.
pub fn connect(path: &Path) -> io::Result<UnixStream> {
    let path = directory(path, false)?.join(SOCKET);
    let meta = fs::symlink_metadata(&path)?;
    if !meta.file_type().is_socket() {
        return Err(denied("endpoint is not a socket"));
    }
    private_metadata(&meta)?;
    let stream = UnixStream::connect(&path)?;
    verify_peer(&stream)?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    Ok(stream)
}

/// Unlock explicitly on every return path, including a bind failure after flock succeeds.
/// Closing the file alone is insufficient while a concurrent fork holds its CLOEXEC copy.
struct EndpointLock {
    file: File,
    owner_pid: u32,
}
impl EndpointLock {
    fn acquire(file: File) -> io::Result<Self> {
        // SAFETY: file owns a live descriptor; flock does not use pointers.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "endpoint already has an owner",
            ));
        }
        Ok(Self {
            file,
            owner_pid: std::process::id(),
        })
    }
}
impl Drop for EndpointLock {
    fn drop(&mut self) {
        if self.owner_pid == std::process::id() {
            // SAFETY: file remains live until after the Drop body.
            unsafe {
                libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}

struct Endpoint {
    socket: PathBuf,
    identity: (u64, u64),
    _lock: EndpointLock,
    owner_pid: u32,
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        if self.owner_pid != std::process::id() {
            return;
        }
        // Never remove a replacement path. Keep the lock inode so another opener cannot
        // lock a different inode while this owner is releasing the old one.
        if fs::symlink_metadata(&self.socket).is_ok_and(|m| (m.dev(), m.ino()) == self.identity) {
            let _ = fs::remove_file(&self.socket);
        }
    }
}
impl Endpoint {
    fn bind(path: &Path) -> io::Result<(Self, UnixListener)> {
        let directory = directory(path, true)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(directory.join("owner.lock"))?;
        let meta = lock.metadata()?;
        if !meta.is_file() {
            return Err(denied("endpoint lock is not a regular file"));
        }
        private_metadata(&meta)?;
        let lock = EndpointLock::acquire(lock)?;
        let socket = directory.join(SOCKET);
        match fs::symlink_metadata(&socket) {
            Ok(meta) => {
                private_metadata(&meta)?;
                if !meta.file_type().is_socket() {
                    return Err(denied("refusing to replace a non-socket endpoint"));
                }
                match UnixStream::connect(&socket) {
                    Ok(_) => {
                        return Err(io::Error::new(
                            io::ErrorKind::AddrInUse,
                            "endpoint is already listening",
                        ));
                    }
                    Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
                        fs::remove_file(&socket)?
                    }
                    Err(error) => return Err(error),
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let listener = UnixListener::bind(&socket)?;
        listener.set_nonblocking(true)?;
        let meta = fs::symlink_metadata(&socket)?;
        let endpoint = Endpoint {
            socket,
            identity: (meta.dev(), meta.ino()),
            _lock: lock,
            owner_pid: std::process::id(),
        };
        fs::set_permissions(&endpoint.socket, fs::Permissions::from_mode(0o600))?;
        Ok((endpoint, listener))
    }
}

/// An optional listener sharing an existing engine. Drop disconnects its clients and joins
/// its I/O threads; it never stops the engine passed to start or deletes durable data.
pub struct Server {
    _endpoint: Endpoint,
    stopped: Arc<AtomicBool>,
    peers: Peers,
    accept: Option<JoinHandle<()>>,
}
impl Server {
    pub fn start(engine: Arc<EngineHandle>, directory: &Path) -> io::Result<Self> {
        let (endpoint, listener) = Endpoint::bind(directory)?;
        let stopped = Arc::new(AtomicBool::new(false));
        let peers = Arc::new(Mutex::new(HashMap::new()));
        let (stop, connections) = (stopped.clone(), peers.clone());
        let accept = thread::Builder::new()
            .name("engine-attach".into())
            .spawn(move || {
                let mut next = 0;
                let mut tasks: Vec<JoinHandle<()>> = Vec::new();
                while !stop.load(Ordering::SeqCst) {
                    let stream = match listener.accept() {
                        Ok((s, _)) => s,
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::park_timeout(Duration::from_millis(20));
                            continue;
                        }
                        Err(_) => break,
                    };
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let mut i = 0;
                    while i < tasks.len() {
                        if tasks[i].is_finished() {
                            let _ = tasks.swap_remove(i).join();
                        } else {
                            i += 1;
                        }
                    }
                    if verify_peer(&stream).is_err() {
                        continue;
                    }
                    let mut peers = connections.lock().unwrap_or_else(|e| e.into_inner());
                    if peers.len() >= MAX_CLIENTS {
                        continue;
                    }
                    let Ok(control) = stream.try_clone() else {
                        continue;
                    };
                    next += 1;
                    let id = next;
                    peers.insert(id, control);
                    drop(peers);
                    let thread_connections = connections.clone();
                    let engine = engine.clone();
                    match thread::Builder::new()
                        .name("engine-attach-client".into())
                        .spawn(move || {
                            let _ = serve(engine, stream);
                            thread_connections
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .remove(&id);
                        }) {
                        Ok(task) => tasks.push(task),
                        Err(_) => {
                            connections
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .remove(&id);
                        }
                    }
                }
                for stream in connections
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .values()
                {
                    let _ = stream.shutdown(Shutdown::Both);
                }
                for task in tasks {
                    let _ = task.join();
                }
            })?;
        Ok(Self {
            _endpoint: endpoint,
            stopped,
            peers,
            accept: Some(accept),
        })
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        for stream in self
            .peers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
        {
            let _ = stream.shutdown(Shutdown::Both);
        }
        if let Some(task) = self.accept.take() {
            task.thread().unpark();
            let _ = task.join();
        }
    }
}

fn read_before(stream: &mut UnixStream, mut bytes: &mut [u8], deadline: Instant) -> io::Result<()> {
    while !bytes.is_empty() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "attachment Hello timed out",
            ));
        }
        stream.set_read_timeout(Some(left))?;
        match stream.read(bytes) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "attachment ended during Hello",
                ));
            }
            Ok(n) => bytes = &mut bytes[n..],
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn hello(stream: &mut UnixStream) -> io::Result<ClientFrame> {
    let deadline = Instant::now() + HELLO_TIMEOUT;
    let mut header = [0; 4];
    read_before(stream, &mut header, deadline)?;
    let len = u32::from_be_bytes(header) as usize;
    if len > HELLO_LIMIT {
        return Err(denied("attachment Hello exceeds the limit"));
    }
    let mut body = vec![0; len];
    read_before(stream, &mut body, deadline)?;
    let first: ClientFrame = serde_json::from_slice(&body).map_err(io::Error::other)?;
    let error = match &first.call {
        Call::Hello { protocol, .. } if *protocol == PROTOCOL_VERSION => None,
        Call::Hello { .. } => Some(ProtoError::new(
            ErrorCode::Incompatible,
            format!("the engine speaks protocol {PROTOCOL_VERSION}"),
        )),
        _ => Some(ProtoError::new(
            ErrorCode::Invalid,
            "Hello is required before attachment calls",
        )),
    };
    if let Some(error) = error {
        let _ = frame::write(
            stream,
            &ServerFrame::Reply {
                id: first.id,
                result: Err(error),
            },
        );
        return Err(denied("attachment handshake refused"));
    }
    stream.set_read_timeout(None)?;
    Ok(first)
}

fn serve(engine: Arc<EngineHandle>, mut stream: UnixStream) -> io::Result<()> {
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let first = hello(&mut stream)?;
    let (sender, receiver) = engine.connect_external("external").split();
    sender.send(first).map_err(io::Error::other)?;
    let control = stream.try_clone()?;
    let mut output = BufWriter::new(stream.try_clone()?);
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = stopped.clone();
    let writer = thread::Builder::new()
        .name("engine-attach-writer".into())
        .spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                match receiver.recv_shared_timeout(Duration::from_millis(100)) {
                    Ok(frame) => {
                        if frame.write(&mut output).is_err() {
                            break;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => break,
                }
            }
            stop.store(true, Ordering::SeqCst);
            let _ = output.get_ref().shutdown(Shutdown::Both);
        })?;
    let mut input = BufReader::new(stream);
    while !stopped.load(Ordering::SeqCst) {
        match frame::read::<ClientFrame>(&mut input) {
            Ok(frame) => {
                if sender.send(frame).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    stopped.store(true, Ordering::SeqCst);
    let _ = control.shutdown(Shutdown::Both);
    drop(sender);
    let _ = writer.join();
    Ok(())
}

#[cfg(test)]
mod tests;
