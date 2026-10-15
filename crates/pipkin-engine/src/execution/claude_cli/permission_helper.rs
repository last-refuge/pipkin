//! Early-main command-hook client. Register this module alongside permission_wire,
//! then call maybe_run_from_env BEFORE logging, DB, GPUI, or any runtime startup.
//! Only the dedicated helper process creates a current-thread Tokio runtime.
//! No server is activated here and no CLI credentials are accessed.

use std::{
    ffi::OsString,
    fmt,
    path::{Path, PathBuf},
    time::Duration,
};

use super::permission_wire::{Binding, Decision, WireError};

pub const HOOK_FLAG: &str = "--claude-permission-hook";
pub const NONCE_ENV: &str = "PIPKIN_CLI_PERMISSION_NONCE";
pub const STDIN_TIMEOUT: Duration = Duration::from_secs(2);
pub const CONNECT_WRITE_TIMEOUT: Duration = Duration::from_secs(2);
pub const DECISION_TIMEOUT: Duration = Duration::from_secs(120);

const ALLOW_JSON: &[u8] = b"{\"hookSpecificOutput\":{\"hookEventName\":\"PermissionRequest\",\"decision\":{\"behavior\":\"allow\"}}}\n";
const DENY_JSON: &[u8] = b"{\"hookSpecificOutput\":{\"hookEventName\":\"PermissionRequest\",\"decision\":{\"behavior\":\"deny\",\"message\":\"Permission denied.\",\"interrupt\":true}}}\n";

/// Fixed, historically observed JSON shape; this helper is not live-qualified.
/// No updatedInput, permission rules, or CLI text.
pub fn decision_json(decision: Decision) -> &'static [u8] {
    match decision {
        Decision::AllowOnce => ALLOW_JSON,
        Decision::Deny => DENY_JSON,
    }
}

/// Arguments and ephemeral environment for parent policy construction; never a
/// shell command. Parent owns safe quoting if a hook API requires a command string.
/// Parent must create a private endpoint and a fresh run nonce (Binding::fresh),
/// and provide this environment only to the relevant run. No process is spawned.
pub struct HelperLaunch {
    arguments: [OsString; 3],
    run_nonce: Binding,
}

impl HelperLaunch {
    pub fn new(socket: &Path, run_nonce: Binding) -> Result<Self, WireError> {
        validate_path_spelling(socket)?;
        Ok(Self {
            arguments: [
                HOOK_FLAG.into(),
                "--socket".into(),
                socket.as_os_str().to_owned(),
            ],
            run_nonce,
        })
    }

    pub fn arguments(&self) -> &[OsString] {
        &self.arguments
    }

    /// Sensitive: pass directly to a child environment, never to diagnostics.
    pub fn environment(&self) -> (&'static str, OsString) {
        (NONCE_ENV, self.run_nonce.to_hex().into())
    }
}

impl fmt::Debug for HelperLaunch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HelperLaunch([redacted])")
    }
}

fn validate_path_spelling(path: &Path) -> Result<(), WireError> {
    // Closed canonical spelling also rejects NUL, symlinks' lexical aliases, and
    // platform-dependent relative paths. Filesystem provenance is checked later.
    let text = path.to_str().ok_or(WireError)?;
    if !text.starts_with('/')
        || text.len() > 4096
        || text.contains('\0')
        || text[1..]
            .split('/')
            .any(|c| c.is_empty() || c == "." || c == "..")
    {
        return Err(WireError);
    }
    Ok(())
}

// `args` excludes argv[0]. Consume only argv[1] in normal mode; no environment,
// filesystem, stdin, logger, or runtime access occurs on that path.
fn parse_args(mut args: impl Iterator<Item = OsString>) -> Option<Result<PathBuf, WireError>> {
    if args.next().as_deref() != Some(std::ffi::OsStr::new(HOOK_FLAG)) {
        return None;
    }
    Some((|| {
        if args.next().as_deref() != Some(std::ffi::OsStr::new("--socket")) {
            return Err(WireError);
        }
        let path = PathBuf::from(args.next().ok_or(WireError)?);
        if args.next().is_some() {
            return Err(WireError);
        }
        validate_path_spelling(&path)?;
        Ok(path)
    })())
}

/// Returns None without I/O in ordinary mode. Helper stdout supports ONLY Unix
/// pipes/sockets, emitting fixed allow/deny JSON; other fd types are rejected
/// before any bytes. Parent must immediately `std::process::exit(code)` on
/// Some(code), including errors. Stdout failure returns 2, not success, but exit 2
/// does NOT intrinsically block PermissionRequest: future hook registration must
/// configure `onFailure: block`. Partial/late output cannot be retracted.
/// Assumes exclusive stdin/stdout/open-file-description use and prompt OS fd
/// classification/control and nonblocking socket syscalls (not hard real time).
/// Requires trusted root/same-UID ancestry; not a sandbox against malicious same UID.
pub fn maybe_run_from_env() -> Option<i32> {
    let socket = parse_args(std::env::args_os().skip(1))?;
    #[cfg(unix)]
    let decision = (|| {
        let socket = socket?;
        let nonce = std::env::var(NONCE_ENV).map_err(|_| WireError)?;
        let nonce = Binding::from_hex(&nonce)?;
        let payload = unix::read_stdin()?;
        let request = super::permission_wire::RequestFrame::new(nonce, payload)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .map_err(|_| WireError)?;
        runtime.block_on(unix::exchange(&socket, &request, DECISION_TIMEOUT))
    })()
    .unwrap_or(Decision::Deny);
    #[cfg(not(unix))]
    let decision = {
        let _ = socket;
        Decision::Deny
    };
    Some(if emit(decision_json(decision)).is_ok() {
        0
    } else {
        2
    })
}

#[cfg(unix)]
fn emit(bytes: &[u8]) -> Result<(), WireError> {
    unix::write_stdout(bytes)
}

#[cfg(not(unix))]
fn emit(_bytes: &[u8]) -> Result<(), WireError> {
    // No bounded pipe/socket implementation on this platform.
    Err(WireError)
}

/// Host-side endpoint provenance check. Caller performs filesystem work off rendering.
/// The helper itself runs this in its bounded metadata workers.
#[cfg(unix)]
pub(crate) fn check_endpoint(path: &Path) -> Result<(), WireError> {
    unix::private_endpoint(path).map(|_| ())
}

#[cfg(unix)]
mod unix {
    use super::super::permission_wire::{self, MAX_STDIN_BYTES, RequestFrame};
    use super::*;
    use std::{
        fs,
        os::{
            fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
            unix::fs::{FileTypeExt, MetadataExt},
        },
        sync::mpsc,
        time::Instant as StdInstant,
    };
    use tokio::{
        net::UnixStream,
        time::{Instant, timeout_at},
    };

    /// Root/UID-owned, non-writable ancestry; immediate parent must be UID-owned
    /// and private (0700 or stricter). Socket must also have no group/other bits.
    /// Symlinks and world-writable ancestors (including /tmp) are not accepted.
    /// Host should use an owned 0700 directory under its trusted home/runtime dir.
    pub(super) fn private_endpoint(path: &Path) -> Result<(u64, u64), WireError> {
        validate_path_spelling(path)?;
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        let parent = path.parent().ok_or(WireError)?;
        for ancestor in parent.ancestors() {
            let meta = fs::symlink_metadata(ancestor).map_err(|_| WireError)?;
            if !meta.is_dir()
                || (meta.uid() != uid && meta.uid() != 0)
                || meta.mode() & 0o022 != 0
                || (ancestor == parent && (meta.uid() != uid || meta.mode() & 0o077 != 0))
            {
                return Err(WireError);
            }
        }
        let meta = fs::symlink_metadata(path).map_err(|_| WireError)?;
        if !meta.file_type().is_socket() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
            return Err(WireError);
        }
        Ok((meta.dev(), meta.ino()))
    }

    pub(super) async fn exchange(
        socket: &Path,
        request: &RequestFrame,
        decision_timeout: Duration,
    ) -> Result<Decision, WireError> {
        exchange_checked(
            socket,
            request,
            decision_timeout,
            Instant::now() + CONNECT_WRITE_TIMEOUT,
            private_endpoint,
        )
        .await
    }

    // The detached worker owns ONLY a path and metadata checker, never a request,
    // stream or output fd. A stuck filesystem cannot stall runtime teardown, and
    // a late result cannot connect, authorize or write. At most two workers per
    // invocation, sequentially; a timed-out first check never starts the second.
    async fn endpoint_before<F>(
        socket: &Path,
        deadline: Instant,
        check: F,
    ) -> Result<(u64, u64), WireError>
    where
        F: FnOnce(&Path) -> Result<(u64, u64), WireError> + Send + 'static,
    {
        if Instant::now() >= deadline {
            return Err(WireError);
        }
        let path = socket.to_owned();
        let (send, receive) = tokio::sync::oneshot::channel();
        std::thread::Builder::new()
            .spawn(move || {
                let _ = send.send(check(&path));
            })
            .map_err(|_| WireError)?;
        let result = timeout_at(deadline, receive)
            .await
            .map_err(|_| WireError)?
            .map_err(|_| WireError)?;
        // timeout_at may poll an already-ready result after its deadline.
        if Instant::now() >= deadline {
            return Err(WireError);
        }
        result
    }

    // Checker/deadline parameters are a local test seam, not a hook input.
    pub(super) async fn exchange_checked<F>(
        socket: &Path,
        request: &RequestFrame,
        decision_timeout: Duration,
        deadline: Instant,
        check: F,
    ) -> Result<Decision, WireError>
    where
        F: Fn(&Path) -> Result<(u64, u64), WireError> + Clone + Send + 'static,
    {
        let endpoint = endpoint_before(socket, deadline, check.clone()).await?;
        // Tokio uses a nonblocking socket: connect is part of the SAME total
        // connect/write budget, not a blocking std UnixStream::connect beforehand.
        let mut stream = timeout_at(deadline, UnixStream::connect(socket))
            .await
            .map_err(|_| WireError)?
            .map_err(|_| WireError)?;
        // SAFETY: geteuid has no preconditions.
        if stream.peer_cred().map_err(|_| WireError)?.uid() != unsafe { libc::geteuid() }
            || endpoint_before(socket, deadline, check).await? != endpoint
            || Instant::now() >= deadline
        {
            return Err(WireError);
        }
        permission_wire::write_request(&mut stream, request, deadline).await?;
        if Instant::now() >= deadline {
            return Err(WireError);
        }
        // One reply only, no reconnect/retry, and one absolute read deadline.
        permission_wire::read_reply(
            &mut stream,
            request,
            Instant::now() + decision_timeout.min(DECISION_TIMEOUT),
        )
        .await
    }

    struct NonblockingFd {
        fd: OwnedFd,
        flags: libc::c_int,
    }

    fn duplicate_fd(fd: RawFd) -> Result<OwnedFd, WireError> {
        // SAFETY: descriptor duplication does not perform filesystem/device I/O;
        // the successful duplicate is uniquely owned.
        let raw = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
        if raw < 0 {
            return Err(WireError);
        }
        Ok(unsafe { OwnedFd::from_raw_fd(raw) })
    }

    impl NonblockingFd {
        fn duplicate(fd: RawFd) -> Result<Self, WireError> {
            let fd = duplicate_fd(fd)?;
            // This early helper is the sole stdin/stdout user. O_NONBLOCK affects
            // the shared open-file description; restore it when the guard drops.
            let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
            if flags < 0
                || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                    < 0
            {
                return Err(WireError);
            }
            Ok(Self { fd, flags })
        }
    }

    impl Drop for NonblockingFd {
        fn drop(&mut self) {
            // SAFETY: fd is still owned and open until after drop returns.
            unsafe {
                libc::fcntl(self.fd.as_raw_fd(), libc::F_SETFL, self.flags);
            }
        }
    }

    fn wait(fd: RawFd, events: libc::c_short, deadline: StdInstant) -> Result<(), WireError> {
        loop {
            let remaining = deadline
                .checked_duration_since(StdInstant::now())
                .ok_or(WireError)?;
            if remaining.is_zero() {
                return Err(WireError);
            }
            let millis = remaining
                .as_millis()
                .saturating_add(1)
                .min(i32::MAX as u128) as i32;
            let mut pollfd = libc::pollfd {
                fd,
                events,
                revents: 0,
            };
            // SAFETY: one valid pollfd, bounded timeout, no borrowed Rust I/O buffers.
            let result = unsafe { libc::poll(&mut pollfd, 1, millis) };
            if result > 0 {
                if StdInstant::now() >= deadline || pollfd.revents & libc::POLLNVAL != 0 {
                    return Err(WireError);
                }
                // HUP/ERR are handled by the following read/write (including EOF).
                return Ok(());
            }
            if result == 0
                || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
            {
                return Err(WireError);
            }
        }
    }

    fn retryable() -> bool {
        matches!(
            std::io::Error::last_os_error().kind(),
            std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
        )
    }

    pub(super) fn read_fd(fd: RawFd, deadline: StdInstant) -> Result<String, WireError> {
        let input = NonblockingFd::duplicate(fd)?;
        let mut bytes = Vec::new();
        loop {
            wait(input.fd.as_raw_fd(), libc::POLLIN, deadline)?;
            let mut buffer = [0u8; 4096];
            let limit = buffer.len().min(MAX_STDIN_BYTES + 1 - bytes.len());
            // SAFETY: buffer has at least limit writable bytes and fd is open.
            let read =
                unsafe { libc::read(input.fd.as_raw_fd(), buffer.as_mut_ptr().cast(), limit) };
            if StdInstant::now() >= deadline {
                return Err(WireError);
            }
            if read == 0 {
                return String::from_utf8(bytes).map_err(|_| WireError);
            }
            if read < 0 {
                if retryable() {
                    continue;
                }
                return Err(WireError);
            }
            bytes.extend_from_slice(&buffer[..read as usize]);
            if bytes.len() > MAX_STDIN_BYTES {
                return Err(WireError);
            }
        }
    }

    pub(super) fn read_stdin() -> Result<String, WireError> {
        let deadline = StdInstant::now() + STDIN_TIMEOUT;
        let (send, receive) = mpsc::sync_channel(1);
        // Never tokio::io::stdin / spawn_blocking: runtime teardown must not wait
        // for an unkillable reader. Exactly one helper thread, poll + nonblocking
        // read, and an outer total deadline even for pathological filesystem fds.
        std::thread::Builder::new()
            .spawn(move || {
                let _ = send.send(read_fd(libc::STDIN_FILENO, deadline));
            })
            .map_err(|_| WireError)?;
        receive
            .recv_timeout(deadline.saturating_duration_since(StdInstant::now()))
            .map_err(|_| WireError)?
    }

    pub(super) fn write_stdout(bytes: &[u8]) -> Result<(), WireError> {
        write_fd(
            libc::STDOUT_FILENO,
            bytes,
            StdInstant::now() + CONNECT_WRITE_TIMEOUT,
        )
    }

    fn pipe_or_socket(fd: &OwnedFd) -> Result<(), WireError> {
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: fd is owned; fstat initializes metadata on success.
        if unsafe { libc::fstat(fd.as_raw_fd(), metadata.as_mut_ptr()) } != 0 {
            return Err(WireError);
        }
        let kind = unsafe { metadata.assume_init() }.st_mode & libc::S_IFMT;
        if !matches!(kind, libc::S_IFIFO | libc::S_IFSOCK) {
            return Err(WireError);
        }
        Ok(())
    }

    // Even fstat of a rejected filesystem/device descriptor can stall. The worker
    // only inspects an owned duplicate; it never receives bytes or writes output.
    // A late result is dropped, never used to authorize a later write.
    pub(super) fn output_before<F>(
        fd: RawFd,
        deadline: StdInstant,
        inspect: F,
    ) -> Result<OwnedFd, WireError>
    where
        F: FnOnce(&OwnedFd) -> Result<(), WireError> + Send + 'static,
    {
        if StdInstant::now() >= deadline {
            return Err(WireError);
        }
        let fd = duplicate_fd(fd)?;
        let (send, receive) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .spawn(move || {
                let result = inspect(&fd).map(|()| fd);
                let _ = send.send(result);
            })
            .map_err(|_| WireError)?;
        let fd = receive
            .recv_timeout(deadline.saturating_duration_since(StdInstant::now()))
            .map_err(|_| WireError)??;
        if StdInstant::now() >= deadline {
            return Err(WireError);
        }
        Ok(fd)
    }

    pub(super) fn write_fd(fd: RawFd, bytes: &[u8], deadline: StdInstant) -> Result<(), WireError> {
        // poll + O_NONBLOCK do not bound regular-file/device writes. Classify
        // before writing anything (including deny/error output). The early helper
        // exclusively owns this fd: no concurrent close/dup2 or flag changes.
        let checked = output_before(fd, deadline, pipe_or_socket)?;
        let output = NonblockingFd::duplicate(checked.as_raw_fd())?;
        let mut remaining = bytes;
        while !remaining.is_empty() {
            wait(output.fd.as_raw_fd(), libc::POLLOUT, deadline)?;
            // SAFETY: remaining is readable for its entire length; fd is open.
            let written = unsafe {
                libc::write(
                    output.fd.as_raw_fd(),
                    remaining.as_ptr().cast(),
                    remaining.len(),
                )
            };
            // Includes successful final writes and errors: never report success
            // after the absolute deadline, even if poll reported readiness.
            if StdInstant::now() >= deadline {
                return Err(WireError);
            }
            if written < 0 {
                if retryable() {
                    continue;
                }
                return Err(WireError);
            }
            if written == 0 {
                return Err(WireError);
            }
            remaining = &remaining[written as usize..];
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "permission_helper_tests.rs"]
mod tests;
