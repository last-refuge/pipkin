//! Hosting the engine: an owned runtime, client connections, and the private worker's stdio
//! loop.
//!
//! [`EngineHandle::connect`] gives a [`Connection`]: frames in, frames out, the same contract
//! whether the client is in-process (direct channels) or a desktop talking to the worker over
//! inherited pipes ([`run_stdio`]). Every connection gets its own client id; sign-in attempts
//! and questions are scoped to it.

use std::sync::mpsc;
use std::time::Duration;

use pipkin_engine_protocol::{ClientFrame, ErrorCode, ProtoError, ServerFrame};

use crate::config::EngineConfig;

#[path = "host_requests.rs"]
pub(crate) mod requests;

/// Dropping the last input handle wakes the engine even when reply backpressure is full.
pub(crate) struct ClientLifetime(pub tokio_util::sync::CancellationToken);
impl Drop for ClientLifetime {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// A client's end of a connection.
pub struct Connection {
    pub(crate) control: tokio::sync::mpsc::Sender<ClientFrame>,
    pub(crate) tx: tokio::sync::mpsc::Sender<ClientFrame>,
    pub(crate) rx: crate::delivery::Receiver,
    pub(crate) client: u64,
    pub(crate) lifetime: std::sync::Arc<ClientLifetime>,
    pub(crate) requests: std::sync::Arc<requests::Requests>,
}

impl Connection {
    /// Queue a frame without blocking. Refuses when its lane is full or the engine is gone.
    pub fn send(&self, frame: ClientFrame) -> Result<(), String> {
        self.requests.send(&self.tx, &self.control, frame)
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<ServerFrame, mpsc::RecvTimeoutError> {
        self.rx.recv_timeout(timeout)
    }

    pub fn try_recv(&self) -> Result<ServerFrame, mpsc::TryRecvError> {
        self.rx.try_recv()
    }

    pub fn client_id(&self) -> u64 {
        self.client
    }

    /// Split into a sender handle and the receiver (for a reader thread).
    pub fn split(self) -> (Sender, crate::delivery::Receiver) {
        (
            Sender {
                tx: self.tx,
                control: self.control,
                _lifetime: self.lifetime,
                requests: self.requests,
            },
            self.rx,
        )
    }
}

/// The sending half of a [`Connection`].
#[derive(Clone)]
pub struct Sender {
    requests: std::sync::Arc<requests::Requests>,
    _lifetime: std::sync::Arc<ClientLifetime>,
    tx: tokio::sync::mpsc::Sender<ClientFrame>,
    control: tokio::sync::mpsc::Sender<ClientFrame>,
}

impl Sender {
    pub fn send(&self, frame: ClientFrame) -> Result<(), String> {
        self.requests.send(&self.tx, &self.control, frame)
    }
}

pub use crate::runtime::EngineHandle;

/// Start an engine with its own runtime threads.
pub fn start(config: EngineConfig) -> Result<EngineHandle, String> {
    crate::runtime::EngineHandle::start(config)
}

/// What the stdout writer does after forwarding one frame.
#[derive(Debug, PartialEq, Eq)]
enum WriterStep {
    Continue,
    Exit,
}

/// Forward one outbound frame to the client. A frame the client could never receive (too
/// large, or not encodable) is replaced by an error reply when it answered a call, and
/// dropped with a line on stderr when it was an event, so one bad frame never silences every
/// later answer. A write that fails on the pipe itself is fatal: the worker must exit so its
/// supervisor restarts it, instead of running blind while the client's calls time out.
fn forward(frame: &crate::delivery::Outbound, out: &mut impl std::io::Write) -> WriterStep {
    use pipkin_engine_protocol::frame::{self, FrameError};
    match frame.write(out) {
        Ok(()) => WriterStep::Continue,
        Err(e @ (FrameError::TooLarge(_) | FrameError::Decode(_))) => {
            eprintln!("pipkin engine: dropped a frame the client cannot receive: {e}");
            if let crate::delivery::Outbound::Reply(reply) = frame
                && let ServerFrame::Reply { id, .. } = **reply
            {
                let answer = ServerFrame::Reply {
                    id,
                    result: Err(ProtoError::new(
                        ErrorCode::TooLarge,
                        format!("the reply could not be delivered: {e}"),
                    )),
                };
                if let Err(e) = frame::write(out, &answer) {
                    eprintln!(
                        "pipkin engine: cannot write to the client: {e}; exiting so the client restarts the engine"
                    );
                    return WriterStep::Exit;
                }
            }
            WriterStep::Continue
        }
        Err(e) => {
            eprintln!(
                "pipkin engine: cannot write to the client: {e}; exiting so the client restarts the engine"
            );
            WriterStep::Exit
        }
    }
}

/// Allocator policy for dedicated engine processes (stdio workers and socket servers).
/// Embedding entry points deliberately leave their host application's policy unchanged.
pub(crate) fn configure_process_allocator() {
    // Keep large temporary context buffers
    // independently releasable instead of letting glibc's adaptive mmap threshold retain them
    // in thread arenas. Use one arena so request buffers freed on a different runtime thread
    // are reused rather than stranded in per-thread heaps. Never change an embedding
    // application's allocator policy.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: mallopt is thread-safe and accepts integer settings, not pointers.
    unsafe {
        libc::mallopt(libc::M_MMAP_THRESHOLD, 128 * 1024);
        libc::mallopt(libc::M_ARENA_MAX, 1);
    }
}

/// The private worker mode: serve one client over stdin/stdout frames until the client
/// closes the channel or sends `Shutdown`. Returns the process exit code.
pub fn run_stdio(config: EngineConfig) -> i32 {
    configure_process_allocator();
    use std::io::{BufReader, BufWriter};

    let handle = match start(config) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("pipkin engine: {e}");
            return 1;
        }
    };
    let conn = handle.connect("worker-stdio");
    let (sender, rx) = conn.split();
    let writer = std::thread::Builder::new()
        .name("engine-stdout".into())
        .spawn(move || {
            let stdout = std::io::stdout();
            let mut out = BufWriter::new(stdout.lock());
            while let Ok(frame) = rx.recv_shared() {
                if let WriterStep::Exit = forward(&frame, &mut out) {
                    // Running on without a way to answer would leave every later call to
                    // time out; exiting lets the client's supervisor start a fresh worker.
                    std::process::exit(3);
                }
            }
        })
        .expect("spawn writer");
    let stdin = std::io::stdin();
    let mut input = BufReader::new(stdin.lock());
    loop {
        match pipkin_engine_protocol::frame::read::<ClientFrame>(&mut input) {
            Ok(frame) => {
                if sender.send(frame).is_err() {
                    break;
                }
            }
            Err(pipkin_engine_protocol::frame::FrameError::Closed) => break,
            Err(e) => {
                eprintln!("pipkin engine: bad frame from client: {e}");
                break;
            }
        }
    }
    drop(sender);
    handle.shutdown(Duration::from_secs(10));
    // The engine's client table holds this connection's reply sender; the writer only sees the
    // end of its channel once the engine (and that table) is gone.
    drop(handle);
    let _ = writer.join();
    0
}

#[cfg(test)]
mod writer_tests {
    use super::*;
    use crate::delivery::Outbound;
    use pipkin_engine_protocol::frame;

    struct Broken;
    impl std::io::Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn reply(id: u64, message: String) -> Outbound {
        Outbound::Reply(Box::new(ServerFrame::Reply {
            id,
            result: Err(ProtoError::new(ErrorCode::Invalid, message)),
        }))
    }

    #[test]
    fn a_reply_too_large_to_send_becomes_an_error_reply_and_the_writer_goes_on() {
        let huge = "x".repeat(pipkin_engine_protocol::limits::MAX_FRAME_BYTES + 1);
        let mut out = Vec::new();
        assert_eq!(forward(&reply(7, huge), &mut out), WriterStep::Continue);
        let written: ServerFrame = frame::read(&mut out.as_slice()).unwrap();
        match written {
            ServerFrame::Reply { id, result } => {
                assert_eq!(id, 7);
                let e = result.unwrap_err();
                assert_eq!(e.code, ErrorCode::TooLarge);
                assert!(e.message.contains("exceeds the limit"), "{}", e.message);
            }
            other => panic!("{other:?}"),
        }
        // The next frame still goes out.
        assert_eq!(
            forward(&reply(8, "fine".into()), &mut out),
            WriterStep::Continue
        );
    }

    #[test]
    fn a_broken_pipe_ends_the_worker_instead_of_leaving_it_blind() {
        assert_eq!(
            forward(&reply(1, "fine".into()), &mut Broken),
            WriterStep::Exit
        );
    }
}
