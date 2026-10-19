//! Test-only script publication. Never used to launch/retry Claude.
//!
//! A writable executable fd in the multithreaded test process can be inherited by an
//! unrelated concurrent fork until that child execs. Closing the parent's fd is then not
//! enough: executing the new script can fail ETXTBSY. Publish in an isolated writer process
//! and wait for it to exit instead. No writable script fd ever enters the test process.
use std::{
    io::{self, Write},
    path::Path,
    process::{Command, Stdio},
};

pub(crate) fn write_script(path: &Path, contents: impl AsRef<[u8]>) -> io::Result<()> {
    let bytes = contents.as_ref();
    if bytes.len() > 64 * 1024 * 1024 {
        return Err(io::Error::other("script fixture too large"));
    }
    let mut child = Command::new("/bin/sh")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .args(["-c", "exec /bin/cat > \"$1\"", "fixture-writer"])
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()?;
    // Large stream fixtures exceed argv limits. The parent owns only a pipe, not the file.
    let written = child.stdin.take().expect("piped stdin").write_all(bytes);
    let status = child.wait()?;
    written?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("script fixture writer failed"))
    }
}

#[test]
fn script_fixture_is_written_verbatim_not_interpreted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("script ' with spaces");
    let script = "#!/bin/sh\nprintf '%s' '$(exit 93)'\n# \" $ ` ; Unicode: é\n";
    write_script(&path, script).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), script);
}
