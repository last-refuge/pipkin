//! Blocking file helpers for the file tools and the per-file mutation lock
//! (`tools/file-mutation-queue.ts`).

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use super::path::{mutation_key, node_error};
use super::{ToolCtx, ToolError};

/// Run `work` (blocking) while holding the mutation lock of `path`'s canonical file. Locks are
/// taken in arrival order; other files never wait. Not a lock against `bash` or other processes.
/// The work always runs to completion once started, so a cancelled call still releases the lock
/// only after its write settled.
pub async fn with_mutation_lock<T, F>(ctx: &ToolCtx, path: PathBuf, work: F) -> Result<T, ToolError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, ToolError> + Send + 'static,
{
    let key = tokio::task::spawn_blocking(move || mutation_key(&path))
        .await
        .map_err(|e| ToolError::new(e.to_string()))?
        .map_err(ToolError::new)?;
    let lock = ctx.locks.lock_for(key);
    let _guard = lock.lock().await;
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| ToolError::new(e.to_string()))?
}

/// `env.writeFile`: `mkdir -p` of the parent, then write the text as-is. The error carries
/// Node's message.
pub fn write_text(path: &Path, content: &str) -> Result<(), (String, io::Error)> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| (node_error(&e, "mkdir", parent), e))?;
    }
    std::fs::write(path, content).map_err(|e| (node_error(&e, "open", path), e))
}

/// Positional read that fills `buf` as far as the file allows; returns the bytes read.
pub fn read_at(file: &File, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        let n = read_at_once(file, offset + total as u64, &mut buf[total..])?;
        if n == 0 {
            break;
        }
        total += n;
    }
    Ok(total)
}

#[cfg(unix)]
fn read_at_once(file: &File, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
    use std::os::unix::fs::FileExt;
    loop {
        match file.read_at(buf, offset) {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            other => return other,
        }
    }
}

#[cfg(windows)]
fn read_at_once(file: &File, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
    use std::os::windows::fs::FileExt;
    file.seek_read(buf, offset)
}

#[cfg(not(any(unix, windows)))]
fn read_at_once(file: &File, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = file;
    file.seek(SeekFrom::Start(offset))?;
    file.read(buf)
}
