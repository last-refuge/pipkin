//! Hosting the engine: an owned runtime, client connections, and the private worker's stdio
//! loop.
//!
//! [`EngineHandle::connect`] gives a [`Connection`]: frames in, frames out, the same contract
//! whether the client is in-process (direct channels) or a desktop talking to the worker over
//! inherited pipes ([`run_stdio`]). Every connection gets its own client id; sign-in attempts
//! and questions are scoped to it.

use std::sync::mpsc;
use std::time::Duration;

use pipkin_engine_protocol::{ClientFrame, ServerFrame};

use crate::config::EngineConfig;

/// A client's end of a connection.
pub struct Connection {
    pub(crate) tx: tokio::sync::mpsc::UnboundedSender<ClientFrame>,
    pub(crate) rx: mpsc::Receiver<ServerFrame>,
    pub(crate) client: u64,
}

impl Connection {
    /// Queue a frame for the engine. Fails only when the engine is gone.
    pub fn send(&self, frame: ClientFrame) -> Result<(), String> {
        self.tx
            .send(frame)
            .map_err(|_| "the engine has stopped".to_owned())
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
    pub fn split(self) -> (Sender, mpsc::Receiver<ServerFrame>) {
        (Sender { tx: self.tx }, self.rx)
    }
}

/// The sending half of a [`Connection`].
#[derive(Clone)]
pub struct Sender {
    tx: tokio::sync::mpsc::UnboundedSender<ClientFrame>,
}

impl Sender {
    pub fn send(&self, frame: ClientFrame) -> Result<(), String> {
        self.tx
            .send(frame)
            .map_err(|_| "the engine has stopped".to_owned())
    }
}

pub use crate::runtime::EngineHandle;

/// Start an engine with its own runtime threads.
pub fn start(config: EngineConfig) -> Result<EngineHandle, String> {
    crate::runtime::EngineHandle::start(config)
}

/// The private worker mode: serve one client over stdin/stdout frames until the client
/// closes the channel or sends `Shutdown`. Returns the process exit code.
pub fn run_stdio(config: EngineConfig) -> i32 {
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
            while let Ok(frame) = rx.recv() {
                if pipkin_engine_protocol::frame::write(&mut out, &frame).is_err() {
                    break;
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
    let _ = writer.join();
    0
}
