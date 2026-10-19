//! Dedicated MCP stdio relay, before logger/DB/GPUI/runtime startup. It never runs tools,
//! parses CLI stdout, reads credentials, reconnects or fabricates MCP replies. Native host
//! authentication/admission is mandatory. Unsupported stdio/platforms fail without output.
use super::orchestration_wire::Rejected;
use std::ffi::{OsStr, OsString};
#[cfg(any(unix, test))]
use std::time::Duration;
pub const FLAG: &str = "--claude-native-tool-bridge";
pub const CHANNEL_ENV: &str = "PIPKIN_CLI_NATIVE_TOOL_CHANNEL";
#[cfg(unix)]
const LIFETIME: Duration = Duration::from_secs(30 * 60);
fn requested(mut args: impl Iterator<Item = OsString>) -> Option<bool> {
    if args.next().as_deref() != Some(OsStr::new(FLAG)) {
        return None;
    }
    Some(args.next().is_none())
}
/// Normal invocation does not inspect environment, descriptors, filesystem or network.
/// Caller must process::exit on Some, including malformed helper args and failures.
pub fn maybe_run_from_env() -> Option<i32> {
    if !requested(std::env::args_os().skip(1))? {
        return Some(2);
    }
    #[cfg(unix)]
    let result = (|| {
        let value = std::env::var(CHANNEL_ENV).map_err(|_| Rejected)?;
        let launch = crate::runtime::cli_bridge_channel::Launch::from_environment_value(&value)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .map_err(|_| Rejected)?;
        runtime.block_on(async {
            // Dedicated helper only: Claude's Unix MCP cleanup sends SIGINT before
            // SIGTERM, potentially before emitting the model terminal. Either signal
            // must stop input via the authenticated handshake, not kill the transport.
            let mut interrupt =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                    .map_err(|_| Rejected)?;
            let mut terminate =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .map_err(|_| Rejected)?;
            // Before connection: never write a terminal/file or read devices/files as MCP.
            let input = pipe::Pipe::new(0, false).map_err(|_| Rejected)?;
            let output = pipe::Pipe::new(1, true).map_err(|_| Rejected)?;
            let deadline = tokio::time::Instant::now() + LIFETIME;
            let channel = launch.connect(deadline).await?;
            let (reader, writer) = channel.split();
            relay(input, output, reader, writer, deadline, async move {
                tokio::select! {
                    _ = interrupt.recv() => {},
                    _ = terminate.recv() => {},
                }
            })
            .await
        })
    })();
    #[cfg(not(unix))]
    let result: Result<(), Rejected> = Err(Rejected);
    Some(if result.is_ok() { 0 } else { 2 })
}

#[cfg(any(unix, test))]
async fn line<R: tokio::io::AsyncBufRead + Unpin>(
    input: &mut R,
) -> Result<Option<Vec<u8>>, Rejected> {
    use tokio::io::AsyncBufReadExt;
    let mut line = Vec::new();
    loop {
        let chunk = input.fill_buf().await.map_err(|_| Rejected)?;
        if chunk.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Err(Rejected)
            };
        }
        let newline = chunk.iter().position(|b| *b == b'\n');
        let used = newline.map_or(chunk.len(), |n| n + 1);
        let take = newline.unwrap_or(chunk.len());
        if line.len() + take > super::orchestration_wire::REQUEST_BYTES {
            return Err(Rejected);
        }
        line.extend_from_slice(&chunk[..take]);
        input.consume(used);
        if newline.is_some() {
            if line.is_empty() {
                return Err(Rejected);
            }
            return Ok(Some(line));
        }
    }
}
#[cfg(unix)]
async fn relay<I, O, R, W>(
    input: I,
    mut output: O,
    mut reader: super::orchestration_wire::Reader<R>,
    mut writer: super::orchestration_wire::Writer<W>,
    deadline: tokio::time::Instant,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<(), Rejected>
where
    I: tokio::io::AsyncRead + Unpin,
    O: tokio::io::AsyncWrite + Unpin,
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    use super::orchestration_control::{self, Control};
    use tokio::io::AsyncWriteExt;
    let mut input_closed = false;
    let closing = {
        // Keep the authenticated read alive across stdin EOF. Dropping a partially
        // consumed wire read revokes both halves and would prevent the close proof.
        let responses = async {
            loop {
                let payload = reader.read(deadline).await?;
                if payload.contains(&b'\n') || payload.contains(&b'\r') {
                    return Err(Rejected);
                }
                let value: serde_json::Value =
                    serde_json::from_slice(&payload).map_err(|_| Rejected)?;
                for control in [Control::Quiesce, Control::Closed] {
                    if control.matches(&value) {
                        return Ok(control);
                    }
                }
                if orchestration_control::reserved(&value)
                    || value.get("jsonrpc").and_then(|v| v.as_str()) != Some("2.0")
                {
                    return Err(Rejected);
                }
                let write = async {
                    output.write_all(&payload).await.map_err(|_| Rejected)?;
                    output.write_all(b"\n").await.map_err(|_| Rejected)?;
                    output.flush().await.map_err(|_| Rejected)
                };
                tokio::time::timeout_at(
                    deadline.min(tokio::time::Instant::now() + Duration::from_secs(10)),
                    write,
                )
                .await
                .map_err(|_| Rejected)??;
            }
        };
        tokio::pin!(responses);
        let first = {
            let requests = async {
                let mut input = tokio::io::BufReader::new(input);
                while let Some(payload) = line(&mut input).await? {
                    let value: serde_json::Value =
                        serde_json::from_slice(&payload).map_err(|_| Rejected)?;
                    if orchestration_control::reserved(&value) {
                        return Err(Rejected);
                    }
                    writer.write(&payload, deadline).await?;
                }
                Ok(())
            };
            tokio::select! {biased;
                _=tokio::time::sleep_until(deadline)=>Err(Rejected),
                _=shutdown=>Ok(None),
                result=requests=>result.map(|()|None),
                result=&mut responses=>result.map(Some),
            }
        }?;
        match first {
            Some(control) => control,
            None => {
                // Normal CLI shutdown can close MCP stdin or signal the helper before
                // its result record. A canceled partial wire write still revokes the link.
                // This proves only request quiescence, never model completion.
                input_closed = true;
                let closing_deadline =
                    deadline.min(tokio::time::Instant::now() + Duration::from_secs(10));
                writer
                    .write(
                        &serde_json::to_vec(&Control::InputClosed.value()).map_err(|_| Rejected)?,
                        closing_deadline,
                    )
                    .await?;
                tokio::time::timeout_at(closing_deadline, &mut responses)
                    .await
                    .map_err(|_| Rejected)??
            }
        }
    };
    if closing == Control::Closed {
        return if input_closed { Ok(()) } else { Err(Rejected) };
    }
    // If terminal and EOF raced, InputClosed already acknowledges quiescence.
    // Otherwise the canceled input loop must not have left a partial wire write.
    let deadline = deadline.min(tokio::time::Instant::now() + Duration::from_secs(10));
    if !input_closed {
        writer
            .write(
                &serde_json::to_vec(&Control::Quiesced.value()).map_err(|_| Rejected)?,
                deadline,
            )
            .await?;
    }
    let final_frame = reader.read(deadline).await?;
    let value: serde_json::Value = serde_json::from_slice(&final_frame).map_err(|_| Rejected)?;
    if !Control::Closed.matches(&value) {
        return Err(Rejected);
    }
    // Deliberately no MCP line: a control acknowledgment is not a tool result/root success.
    Ok(())
}

#[cfg(unix)]
mod pipe;
#[cfg(test)]
mod tests;
