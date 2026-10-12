//! Request/response matching over one engine link.
//!
//! A [`Link`] is a pair of blocking halves carrying protocol frames: the in-process host's
//! channels, a worker's private pipes, or a test's fake engine. [`Client`] owns the sending half
//! and a reader thread that owns the receiving half. Replies are routed to the call waiting on
//! their id; events (and the end of the link) go to a callback, in arrival order, so the adapter's
//! worker sees them on its own queue after any call that was in flight returns.
//!
//! A call never retries. Its result says exactly what is known: the frame never left
//! ([`CallError::NotSent`]), the engine refused it ([`CallError::Refused`]), or it was sent and no
//! answer came back ([`CallError::Lost`]), which for a prompt is an unknown outcome.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use pipkin_engine_protocol::{Call, ClientFrame, Event, ProtoError, Reply, ServerFrame};

/// The sending half of a link. `send` must not block indefinitely.
pub trait FrameSender: Send {
    fn send(&mut self, frame: ClientFrame) -> Result<(), String>;
}

/// The receiving half of a link. `recv` blocks until a frame arrives, and fails once the link
/// has ended (cleanly or not); after that it is never called again.
pub trait FrameReceiver: Send {
    fn recv(&mut self) -> Result<ServerFrame, String>;
}

pub struct Link {
    pub sender: Box<dyn FrameSender>,
    pub receiver: Box<dyn FrameReceiver>,
}

impl FrameSender for pipkin_engine::host::Sender {
    fn send(&mut self, frame: ClientFrame) -> Result<(), String> {
        pipkin_engine::host::Sender::send(self, frame)
    }
}

impl FrameReceiver for mpsc::Receiver<ServerFrame> {
    fn recv(&mut self) -> Result<ServerFrame, String> {
        mpsc::Receiver::recv(self).map_err(|_| "the engine closed the connection".to_owned())
    }
}

impl FrameReceiver for pipkin_engine::delivery::Receiver {
    fn recv(&mut self) -> Result<ServerFrame, String> {
        pipkin_engine::delivery::Receiver::recv(self)
            .map_err(|_| "the engine closed the connection".to_owned())
    }
}

/// What the reader thread hands to the adapter, in arrival order.
#[derive(Debug)]
pub enum Inbound {
    Event {
        epoch: u64,
        seq: u64,
        event: Box<Event>,
    },
    /// The link ended; every call still waiting has failed as `Lost`.
    Closed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum CallError {
    /// The frame was never handed to the engine: a definite refusal.
    NotSent(String),
    /// The engine answered with an error.
    Refused(ProtoError),
    /// The frame was sent and no answer arrived (timeout or a lost link): the outcome is unknown.
    Lost(String),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::NotSent(m) => write!(f, "not sent: {m}"),
            CallError::Refused(e) => f.write_str(&e.message),
            CallError::Lost(m) => write!(f, "no answer from the engine: {m}"),
        }
    }
}

type Waiter = SyncSender<Result<Reply, CallError>>;

struct Shared {
    pending: Mutex<HashMap<u64, Waiter>>,
    /// Why the link ended, once it has.
    closed: Mutex<Option<String>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct Client {
    sender: Mutex<Option<Box<dyn FrameSender>>>,
    shared: Arc<Shared>,
    next_id: AtomicU64,
    reader: Mutex<Option<JoinHandle<()>>>,
}

impl Client {
    /// Take over `link`. `inbound` is called from the reader thread for every event and once
    /// when the link ends.
    pub fn start(link: Link, inbound: impl Fn(Inbound) + Send + 'static) -> Client {
        let shared = Arc::new(Shared {
            pending: Mutex::new(HashMap::new()),
            closed: Mutex::new(None),
        });
        let reader_shared = shared.clone();
        let mut receiver = link.receiver;
        let reader = thread::Builder::new()
            .name("native-engine-reader".into())
            .spawn(move || {
                let reason = loop {
                    match receiver.recv() {
                        Ok(ServerFrame::Reply { id, result }) => {
                            let waiter = lock(&reader_shared.pending).remove(&id);
                            match waiter {
                                Some(waiter) => {
                                    let _ = waiter.try_send(result.map_err(CallError::Refused));
                                }
                                // A reply nobody waits for any more (its call timed out).
                                None => log::debug!("late reply {id} dropped"),
                            }
                        }
                        Ok(ServerFrame::Event { epoch, seq, event }) => inbound(Inbound::Event {
                            epoch,
                            seq,
                            event: Box::new(event),
                        }),
                        Err(reason) => break reason,
                    }
                };
                *lock(&reader_shared.closed) = Some(reason.clone());
                for (_, waiter) in lock(&reader_shared.pending).drain() {
                    let _ = waiter.try_send(Err(CallError::Lost(reason.clone())));
                }
                inbound(Inbound::Closed(reason));
            })
            .expect("spawn engine reader");
        Client {
            sender: Mutex::new(Some(link.sender)),
            shared,
            next_id: AtomicU64::new(1),
            reader: Mutex::new(Some(reader)),
        }
    }

    #[cfg(test)]
    pub fn is_closed(&self) -> bool {
        lock(&self.shared.closed).is_some()
    }

    /// Send `call` and wait up to `timeout` for its reply.
    pub fn call(&self, call: Call, timeout: Duration) -> Result<Reply, CallError> {
        if let Some(reason) = lock(&self.shared.closed).clone() {
            return Err(CallError::NotSent(reason));
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::sync_channel(1);
        lock(&self.shared.pending).insert(id, tx);
        let lane = call.lane();
        let sent = match lock(&self.sender).as_mut() {
            Some(sender) => sender.send(ClientFrame { id, lane, call }),
            None => Err("the connection is closing".to_owned()),
        };
        if let Err(reason) = sent {
            lock(&self.shared.pending).remove(&id);
            return Err(CallError::NotSent(reason));
        }
        match rx.recv_timeout(timeout) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                lock(&self.shared.pending).remove(&id);
                // The reply may have raced the timeout; prefer it.
                rx.try_recv().unwrap_or_else(|_| {
                    // Best effort, without another blocking wait. Cancellation only stops
                    // the RPC: an admitted prompt remains durable and must be looked up.
                    let cancel_id = self.next_id.fetch_add(1, Ordering::SeqCst);
                    let call = Call::CancelRequest { request: id };
                    if let Some(sender) = lock(&self.sender).as_mut() {
                        let _ = sender.send(ClientFrame {
                            id: cancel_id,
                            lane: call.lane(),
                            call,
                        });
                    }
                    Err(CallError::Lost("the engine did not answer in time".into()))
                })
            }
            Err(RecvTimeoutError::Disconnected) => {
                Err(CallError::Lost("the engine connection ended".into()))
            }
        }
    }

    /// Close the sending half, so a worker reading its input sees the end of it. The reader
    /// keeps running until the engine closes its side.
    pub fn close(&self) {
        let reason = "the connection is closing".to_owned();
        *lock(&self.shared.closed) = Some(reason.clone());
        for (_, waiter) in lock(&self.shared.pending).drain() {
            let _ = waiter.try_send(Err(CallError::Lost(reason.clone())));
        }
        lock(&self.sender).take();
    }

    /// Wait up to `timeout` for the reader thread to finish. Returns whether it did.
    pub fn join_reader(&self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let done = lock(&self.reader).as_ref().is_none_or(|h| h.is_finished());
            if done {
                if let Some(handle) = lock(&self.reader).take() {
                    let _ = handle.join();
                }
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.close();
    }
}

/// Where an event sits relative to what this client has seen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeqCheck {
    /// The next event: apply it.
    InOrder,
    /// Events were missed (or repeated): discard derived state and re-read.
    Gap,
    /// The engine restarted its event stream: re-read everything.
    NewEpoch,
}

/// Tracks `(epoch, seq)` of one connection's events. The epoch comes from `Welcome`; the first
/// event after it sets the sequence baseline. Protocol 4 sequences events per connection;
/// private events for a different client do not create gaps.
#[derive(Clone, Debug, Default)]
pub struct Sequencer {
    epoch: Option<u64>,
    last: Option<u64>,
}

impl Sequencer {
    pub fn welcome(&mut self, epoch: u64) {
        self.epoch = Some(epoch);
        self.last = None;
    }

    pub fn observe(&mut self, epoch: u64, seq: u64) -> SeqCheck {
        if self.epoch != Some(epoch) {
            self.epoch = Some(epoch);
            self.last = Some(seq);
            return SeqCheck::NewEpoch;
        }
        let check = match self.last {
            None => SeqCheck::InOrder,
            Some(last) if seq == last.wrapping_add(1) => SeqCheck::InOrder,
            Some(_) => SeqCheck::Gap,
        };
        self.last = Some(seq);
        check
    }
}

#[cfg(test)]
pub mod testing {
    //! A scriptable fake engine on the other end of a channel link.

    use super::*;

    pub struct ChanSender(pub mpsc::Sender<ClientFrame>);

    impl FrameSender for ChanSender {
        fn send(&mut self, frame: ClientFrame) -> Result<(), String> {
            self.0
                .send(frame)
                .map_err(|_| "fake engine gone".to_owned())
        }
    }

    /// A link whose engine side is a pair of channels the test drives.
    pub fn link() -> (Link, mpsc::Receiver<ClientFrame>, mpsc::Sender<ServerFrame>) {
        let (to_engine, from_client) = mpsc::channel();
        let (to_client, from_engine) = mpsc::channel();
        (
            Link {
                sender: Box::new(ChanSender(to_engine)),
                receiver: Box::new(from_engine),
            },
            from_client,
            to_client,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::testing::link;
    use super::*;
    use pipkin_engine_protocol::{ErrorCode, Lane};

    fn collect() -> (impl Fn(Inbound) + Send + 'static, mpsc::Receiver<Inbound>) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        (
            move |i| {
                let _ = lock(&tx).send(i);
            },
            rx,
        )
    }

    #[test]
    fn close_wakes_pending_calls_without_waiting_for_the_peer() {
        let (link, from_client, peer) = link();
        let client = Arc::new(Client::start(link, |_| {}));
        let caller = client.clone();
        let (done, result) = mpsc::channel();
        let thread = thread::spawn(move || {
            done.send(caller.call(Call::ListSessions, Duration::from_secs(60)))
                .unwrap();
        });
        from_client.recv_timeout(Duration::from_secs(2)).unwrap();
        client.close();
        assert!(matches!(
            result.recv_timeout(Duration::from_secs(1)).unwrap(),
            Err(CallError::Lost(_))
        ));
        assert!(matches!(
            client.call(Call::ListSessions, Duration::from_secs(1)),
            Err(CallError::NotSent(_))
        ));
        thread.join().unwrap();
        drop(peer);
        assert!(client.join_reader(Duration::from_secs(1)));
    }

    #[test]
    fn replies_match_their_calls_and_events_flow_separately() {
        let (link, from_client, to_client) = link();
        let (sink, inbound) = collect();
        let client = Arc::new(Client::start(link, sink));
        let engine = thread::spawn(move || {
            let a = from_client.recv().unwrap();
            let b = from_client.recv().unwrap();
            assert_eq!(b.lane, Lane::Control, "Stop travels in the control lane");
            to_client
                .send(ServerFrame::Event {
                    epoch: 1,
                    seq: 1,
                    event: Event::SessionsChanged,
                })
                .unwrap();
            // Answer out of order.
            to_client
                .send(ServerFrame::Reply {
                    id: b.id,
                    result: Ok(Reply::Stopping { run: Some(3) }),
                })
                .unwrap();
            to_client
                .send(ServerFrame::Reply {
                    id: a.id,
                    result: Err(ProtoError::new(ErrorCode::Busy, "busy")),
                })
                .unwrap();
        });
        let c1 = client.clone();
        let first = thread::spawn(move || c1.call(Call::ListSessions, Duration::from_secs(5)));
        thread::sleep(Duration::from_millis(50));
        let second = client.call(
            Call::Stop {
                session: "s".into(),
            },
            Duration::from_secs(5),
        );
        assert_eq!(second, Ok(Reply::Stopping { run: Some(3) }));
        assert!(matches!(
            first.join().unwrap(),
            Err(CallError::Refused(ProtoError {
                code: ErrorCode::Busy,
                ..
            }))
        ));
        engine.join().unwrap();
        assert!(matches!(
            inbound.recv_timeout(Duration::from_secs(5)).unwrap(),
            Inbound::Event {
                epoch: 1,
                seq: 1,
                event,
            } if *event == Event::SessionsChanged
        ));
    }

    #[test]
    fn a_timeout_is_lost_and_a_late_reply_is_dropped() {
        let (link, from_client, to_client) = link();
        let (sink, _inbound) = collect();
        let client = Client::start(link, sink);
        let result = client.call(Call::ListSessions, Duration::from_millis(30));
        assert!(matches!(result, Err(CallError::Lost(_))));
        let frame = from_client.recv().unwrap();
        let cancellation = from_client.recv().unwrap();
        assert_eq!(cancellation.call, Call::CancelRequest { request: frame.id });
        assert_eq!(cancellation.lane, pipkin_engine_protocol::Lane::Control);
        to_client
            .send(ServerFrame::Reply {
                id: frame.id,
                result: Ok(Reply::Ok),
            })
            .unwrap();
        // The next call gets its own reply, never the stale one.
        let engine = thread::spawn(move || {
            let next = from_client.recv().unwrap();
            to_client
                .send(ServerFrame::Reply {
                    id: next.id,
                    result: Ok(Reply::Sessions { sessions: vec![] }),
                })
                .unwrap();
        });
        assert_eq!(
            client.call(Call::ListSessions, Duration::from_secs(5)),
            Ok(Reply::Sessions { sessions: vec![] })
        );
        engine.join().unwrap();
    }

    #[test]
    fn a_closed_link_fails_waiting_calls_as_lost_and_new_calls_as_not_sent() {
        let (link, from_client, to_client) = link();
        let (sink, inbound) = collect();
        let client = Arc::new(Client::start(link, sink));
        let c = client.clone();
        let waiting = thread::spawn(move || c.call(Call::ListSessions, Duration::from_secs(10)));
        let _ = from_client.recv().unwrap();
        drop(to_client);
        assert!(matches!(waiting.join().unwrap(), Err(CallError::Lost(_))));
        assert!(matches!(
            inbound.recv_timeout(Duration::from_secs(5)).unwrap(),
            Inbound::Closed(_)
        ));
        assert!(client.is_closed());
        assert!(matches!(
            client.call(Call::ListSessions, Duration::from_secs(1)),
            Err(CallError::NotSent(_))
        ));
        assert!(client.join_reader(Duration::from_secs(5)));
    }

    #[test]
    fn a_send_failure_is_not_sent() {
        let (link, from_client, _to_client) = link();
        drop(from_client);
        let (sink, _inbound) = collect();
        let client = Client::start(link, sink);
        assert!(matches!(
            client.call(Call::ListSessions, Duration::from_secs(1)),
            Err(CallError::NotSent(_))
        ));
    }

    #[test]
    fn sequencer_detects_gaps_and_new_epochs() {
        let mut s = Sequencer::default();
        s.welcome(4);
        assert_eq!(
            s.observe(4, 17),
            SeqCheck::InOrder,
            "first event sets the baseline"
        );
        assert_eq!(s.observe(4, 18), SeqCheck::InOrder);
        assert_eq!(s.observe(4, 20), SeqCheck::Gap);
        assert_eq!(s.observe(4, 21), SeqCheck::InOrder);
        assert_eq!(s.observe(4, 21), SeqCheck::Gap, "a repeat is not in order");
        assert_eq!(s.observe(5, 1), SeqCheck::NewEpoch);
        assert_eq!(s.observe(5, 2), SeqCheck::InOrder);
        s.welcome(6);
        assert_eq!(s.observe(6, 99), SeqCheck::InOrder);
    }
}
