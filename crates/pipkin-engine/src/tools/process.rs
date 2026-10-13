//! Process supervision shared by `bash` and background jobs: pipe readers, whole-group
//! kills and exit-code mapping, with the cancellation guarantees `bash` documents (SIGKILL to
//! the process group at once; descendants are not given a chance to trap TERM).

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::mpsc;

pub const READ_CHUNK: usize = 64 * 1024;

pub fn spawn_reader<R>(
    mut pipe: R,
    stream: usize,
    tx: mpsc::Sender<(usize, Vec<u8>)>,
) -> tokio::task::JoinHandle<()>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buf = vec![0u8; READ_CHUNK];
        loop {
            match pipe.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send((stream, buf[..n].to_vec())).await.is_err() {
                        break;
                    }
                }
            }
        }
    })
}

#[cfg(unix)]
pub fn kill_group(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        // SAFETY: plain syscalls on the group (or pid) of a child we have not reaped yet.
        unsafe {
            if libc::kill(-(pid as libc::pid_t), libc::SIGKILL) != 0 {
                libc::kill(pid as libc::pid_t, libc::SIGKILL);
            }
        }
    }
}

#[cfg(not(unix))]
pub fn kill_group(child: &mut tokio::process::Child) {
    let _ = child.start_kill();
}

pub fn exit_code(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    1
}
