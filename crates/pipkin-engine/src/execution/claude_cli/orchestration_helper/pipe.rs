//! Dedicated-process, nonblocking pipe/socket stdio. Never regular files, devices or terminals.
use std::{
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    pin::Pin,
    task::{Context, Poll, ready},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf, unix::AsyncFd};
struct Descriptor {
    fd: OwnedFd,
    flags: libc::c_int,
}
impl AsRawFd for Descriptor {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}
impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: the owned duplicate is live. Restore the shared open-file-description flags.
        unsafe {
            libc::fcntl(self.as_raw_fd(), libc::F_SETFL, self.flags);
        }
    }
}
pub(super) struct Pipe(AsyncFd<Descriptor>);
impl Pipe {
    pub(super) fn new(fd: RawFd, write: bool) -> io::Result<Self> {
        // SAFETY: duplicating the borrowed descriptor gives us an independently owned fd.
        let raw = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: stat points to writable storage and the owned descriptor is live.
        if unsafe { libc::fstat(raw, stat.as_mut_ptr()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let mode = unsafe { stat.assume_init() }.st_mode & libc::S_IFMT;
        if mode != libc::S_IFIFO && mode != libc::S_IFSOCK {
            return Err(io::Error::other("bridge stdio must be a pipe or socket"));
        }
        let flags = unsafe { libc::fcntl(raw, libc::F_GETFL) };
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        let access = flags & libc::O_ACCMODE;
        if (write && access == libc::O_RDONLY) || (!write && access == libc::O_WRONLY) {
            return Err(io::Error::other("invalid bridge stdio direction"));
        }
        let descriptor = Descriptor { fd, flags };
        if unsafe { libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(AsyncFd::new(descriptor)?))
    }
}
impl AsyncRead for Pipe {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        loop {
            let mut ready = ready!(self.0.poll_read_ready(cx))?;
            let result = ready.try_io(|fd| {
                let target = buf.initialize_unfilled();
                // SAFETY: target is writable for its length and fd is live/nonblocking.
                let n =
                    unsafe { libc::read(fd.as_raw_fd(), target.as_mut_ptr().cast(), target.len()) };
                if n < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(n as usize)
                }
            });
            match result {
                Ok(Ok(n)) => {
                    buf.advance(n);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(e)) => return Poll::Ready(Err(e)),
                Err(_) => {}
            }
        }
    }
}
impl AsyncWrite for Pipe {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        loop {
            let mut ready = ready!(self.0.poll_write_ready(cx))?;
            let result = ready.try_io(|fd| {
                // SAFETY: buf is readable for its length and fd is live/nonblocking.
                let n = unsafe { libc::write(fd.as_raw_fd(), buf.as_ptr().cast(), buf.len()) };
                if n < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(n as usize)
                }
            });
            if let Ok(result) = result {
                return Poll::Ready(result);
            }
        }
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
