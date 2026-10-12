//! Shell-based AWS credential_process with explicit environment and bounded output.
use super::{AwsCredentials, BTreeMap, json_credentials, now_secs};
use serde_json::Value;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

const LIMIT: usize = 1024 * 1024; // Node exec's default maxBuffer, for each output stream.
struct Group(Option<u32>);
impl Drop for Group {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(id) = self.0 {
            // SAFETY: this is the group of our own, not-yet-reaped helper process.
            unsafe {
                libc::kill(-(id as libc::pid_t), libc::SIGKILL);
            }
        }
    }
}
pub(super) async fn credentials(
    command: &str,
    env: &BTreeMap<String, String>,
    cancel: &CancellationToken,
) -> Result<AwsCredentials, String> {
    execute(command, env, cancel, Duration::from_secs(60)).await
}
async fn execute(
    command: &str,
    env: &BTreeMap<String, String>,
    cancel: &CancellationToken,
    budget: Duration,
) -> Result<AwsCredentials, String> {
    if cancel.is_cancelled() {
        return Err("Request was aborted".into());
    }
    let mut cmd = if cfg!(windows) {
        let shell = env
            .get("ComSpec")
            .or_else(|| env.get("COMSPEC"))
            .map(String::as_str)
            .unwrap_or("cmd.exe");
        let mut cmd = tokio::process::Command::new(shell);
        cmd.arg("/C").arg(command);
        cmd
    } else {
        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.arg("-c").arg(command);
        cmd
    };
    cmd.env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd
        .spawn()
        .map_err(|_| "credential_process failed to start")?;
    let mut group = Group(child.id());
    let mut stdout = child
        .stdout
        .take()
        .ok_or("credential_process has no stdout")?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or("credential_process has no stderr")?;
    let work = async {
        let mut output = Vec::new();
        let mut error_bytes = 0usize;
        let (mut out, mut err) = (true, true);
        let (mut a, mut b) = ([0u8; 8192], [0u8; 8192]);
        while out || err {
            tokio::select! {
                read = stdout.read(&mut a), if out => {
                    let n = read.map_err(|_| "credential_process output read failed")?;
                    out = n != 0;
                    if output.len() + n > LIMIT {
                        return Err("credential_process stdout is too large".to_owned());
                    }
                    output.extend_from_slice(&a[..n]);
                },
                read = stderr.read(&mut b), if err => {
                    let n = read.map_err(|_| "credential_process output read failed")?;
                    err = n != 0;
                    error_bytes += n;
                    if error_bytes > LIMIT {
                        return Err("credential_process stderr is too large".to_owned());
                    }
                },
            }
        }
        let status = child
            .wait()
            .await
            .map_err(|_| "credential_process wait failed")?;
        group.0 = None;
        if !status.success() {
            return Err(format!("credential_process exited with {status}"));
        }
        let value: Value = serde_json::from_slice(&output)
            .map_err(|_| "credential_process returned invalid JSON")?;
        if value["Version"].as_u64() != Some(1) {
            return Err("credential_process output must have Version 1".into());
        }
        let creds = json_credentials(&value, "SessionToken")
            .map_err(|e| format!("credential_process {e}"))?;
        if value
            .get("Expiration")
            .is_some_and(|v| !v.is_null() && v.as_str() != Some(""))
        {
            let expiration = creds
                .expires_at
                .ok_or("credential_process returned invalid Expiration")?;
            if expiration <= now_secs() {
                return Err("credential_process returned expired credentials".into());
            }
        }
        Ok(creds)
    };
    tokio::select! {
        _ = cancel.cancelled() => Err("Request was aborted".into()),
        result = tokio::time::timeout(budget, work) => {
            result.map_err(|_| "credential_process timed out".to_owned())?
        },
    }
}

#[cfg(all(test, unix))]
#[path = "aws_process_tests.rs"]
mod tests;
