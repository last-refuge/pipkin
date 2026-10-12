//! Stop must not wait for transcript delivery to GPUI. One bounded worker uses the same
//! multiplexed connection (no second owner/client). Each request captures its connection and
//! session at admission; reconnect never redirects an old Stop to a replacement session.
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::{Client, Msg, lock};
use pipkin_core::{BackendRequest, ConversationId, OperationId};
use pipkin_engine_protocol::{Call, Reply, SessionSummary};

const CAPACITY: usize = 8;

#[derive(Default)]
struct Binding {
    client: Weak<Client>,
    sessions: HashMap<ConversationId, String>,
    orders: HashMap<ConversationId, Arc<SendOrder>>,
}

#[derive(Default)]
pub(super) struct SendOrder {
    version: AtomicU64,
    pub sending: Mutex<()>,
}

type SubmissionKey = (ConversationId, u64, OperationId);
#[derive(Default)]
struct Submission {
    done: Mutex<bool>,
    ready: Condvar,
}
impl Submission {
    fn finish(&self) {
        *lock(&self.done) = true;
        self.ready.notify_all();
    }
    fn wait(&self, timeout: Duration) -> bool {
        let (done, _) = self
            .ready
            .wait_timeout_while(lock(&self.done), timeout, |done| !*done)
            .unwrap_or_else(|e| e.into_inner());
        *done
    }
}

struct Stop {
    order: Arc<SendOrder>,
    version: u64,
    submission: Option<Arc<Submission>>,
    client: Weak<Client>,
    session: String,
    conversation: ConversationId,
    generation: u64,
    op: OperationId,
}

pub(super) struct Control {
    binding: Mutex<Binding>,
    submissions: Mutex<HashMap<SubmissionKey, Arc<Submission>>>,
    tx: Mutex<Option<mpsc::SyncSender<Stop>>>,
    closed: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
    feedback: mpsc::Sender<Msg>,
}

impl Control {
    pub fn new(feedback: mpsc::Sender<Msg>, timeout: Duration) -> Self {
        let (tx, rx) = mpsc::sync_channel::<Stop>(CAPACITY);
        let closed = Arc::new(AtomicBool::new(false));
        let done = closed.clone();
        let results = feedback.clone();
        let handle = thread::Builder::new()
            .name("native-stop".into())
            .spawn(move || {
                while let Ok(stop) = rx.recv() {
                    if done.load(Ordering::Acquire) {
                        break;
                    }
                    // Preserve Submit -> Stop ordering even when Cancel is clicked before the
                    // prompt's acknowledgement. Waiting is bounded and never holds the data worker.
                    let ready = stop.submission.as_ref().is_none_or(|s| s.wait(timeout));
                    if done.load(Ordering::Acquire) {
                        break;
                    }
                    let _sending = lock(&stop.order.sending);
                    if stop.version != stop.order.version.load(Ordering::Acquire) {
                        // A later local prompt superseded this queued Stop. Its old run has
                        // already settled; it must not cancel the new prompt.
                        continue;
                    }
                    let error = if !ready {
                        Some("The prompt is still awaiting submission. Try Stop again.".into())
                    } else {
                        match stop.client.upgrade() {
                            Some(client) => match client.call(
                                Call::Stop {
                                    session: stop.session,
                                },
                                timeout,
                            ) {
                                Ok(Reply::Stopping { .. }) => None,
                                Ok(other) => Some(format!("Unexpected Stop reply: {other:?}")),
                                Err(error) => {
                                    Some(format!("Could not ask the engine to stop: {error}"))
                                }
                            },
                            None => {
                                Some("The engine connection ended before Stop was sent.".into())
                            }
                        }
                    };
                    if let Some(message) = error {
                        let _ = results.send(Msg::StopFailed {
                            conversation: stop.conversation,
                            generation: stop.generation,
                            op: stop.op,
                            message,
                        });
                    }
                }
            })
            .expect("spawn native Stop worker");
        Self {
            binding: Mutex::new(Binding::default()),
            submissions: Mutex::default(),
            tx: Mutex::new(Some(tx)),
            closed,
            thread: Mutex::new(Some(handle)),
            feedback,
        }
    }

    pub fn submitting(&self, request: &BackendRequest) {
        if let BackendRequest::Submit {
            conversation,
            generation,
            op,
            ..
        } = request
        {
            if let Some(order) = self.order(*conversation) {
                order.version.fetch_add(1, Ordering::AcqRel);
            }
            lock(&self.submissions)
                .entry((*conversation, *generation, *op))
                .or_default();
        }
    }

    pub fn submitted(&self, conversation: ConversationId, generation: u64, op: OperationId) {
        if let Some(submission) = lock(&self.submissions).remove(&(conversation, generation, op)) {
            submission.finish();
        }
    }

    pub fn order(&self, conversation: ConversationId) -> Option<Arc<SendOrder>> {
        lock(&self.binding).orders.get(&conversation).cloned()
    }

    pub fn bind(&self, client: &Arc<Client>, sessions: &[(ConversationId, SessionSummary)]) {
        let mut binding = lock(&self.binding);
        let orders = sessions
            .iter()
            .map(|(id, session)| {
                let order = if binding.sessions.get(id) == Some(&session.id) {
                    binding.orders.get(id).cloned().unwrap_or_default()
                } else {
                    Arc::default()
                };
                (*id, order)
            })
            .collect();
        *binding = Binding {
            client: Arc::downgrade(client),
            sessions: sessions.iter().map(|(id, s)| (*id, s.id.clone())).collect(),
            orders,
        };
    }

    pub fn clear(&self, client: &Arc<Client>) {
        let mut binding = lock(&self.binding);
        if binding.client.ptr_eq(&Arc::downgrade(client)) {
            *binding = Binding::default();
        }
    }

    /// Returns false only if this is not a Stop. All Stops are either admitted or explicitly
    /// refused; never fall back to the blocked data worker or silently retry on reconnect.
    pub fn request(&self, request: &BackendRequest) -> bool {
        let BackendRequest::Cancel {
            conversation,
            generation,
            op,
        } = request
        else {
            return false;
        };
        let fail = |message: &str| {
            let _ = self.feedback.send(Msg::StopFailed {
                conversation: *conversation,
                generation: *generation,
                op: *op,
                message: message.into(),
            });
        };
        let stop = {
            let binding = lock(&self.binding);
            let Some(session) = binding.sessions.get(conversation) else {
                fail("This conversation is not connected to the engine.");
                return true;
            };
            let order = binding
                .orders
                .get(conversation)
                .expect("catalogue order")
                .clone();
            Stop {
                version: order.version.load(Ordering::Acquire),
                order,
                submission: lock(&self.submissions)
                    .get(&(*conversation, *generation, *op))
                    .cloned(),
                client: binding.client.clone(),
                session: session.clone(),
                conversation: *conversation,
                generation: *generation,
                op: *op,
            }
        };
        match lock(&self.tx).as_ref().map(|tx| tx.try_send(stop)) {
            Some(Ok(())) => {}
            Some(Err(mpsc::TrySendError::Full(_))) => {
                fail("Too many Stop requests are pending. Try Stop again.")
            }
            _ => fail("The engine connection is closing."),
        }
        true
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        lock(&self.tx).take();
        for submission in lock(&self.submissions).drain().map(|(_, s)| s) {
            submission.finish();
        }
    }

    pub fn reap(&self) {
        let mut thread = lock(&self.thread);
        if thread.as_ref().is_some_and(JoinHandle::is_finished) {
            let _ = thread.take().unwrap().join();
        }
    }
}
impl Drop for Control {
    fn drop(&mut self) {
        self.close();
        self.reap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::native::client::testing::link;
    use pipkin_core::RequestId;
    use pipkin_engine_protocol::ServerFrame;

    fn stop(op: u64) -> BackendRequest {
        BackendRequest::Cancel {
            conversation: ConversationId(1),
            generation: 2,
            op: OperationId(op),
        }
    }
    fn bind(control: &Control, client: &Arc<Client>, session: &str) {
        *lock(&control.binding) = Binding {
            client: Arc::downgrade(client),
            sessions: [(ConversationId(1), session.to_owned())].into(),
            orders: [(ConversationId(1), Arc::default())].into(),
        };
    }

    #[test]
    fn stop_waits_for_submission_before_crossing_the_control_lane() {
        let (link, incoming, peer) = link();
        let client = Arc::new(Client::start(link, |_| {}));
        let (feedback, failures) = mpsc::channel();
        let control = Control::new(feedback, Duration::from_secs(2));
        bind(&control, &client, "session");
        control.submitting(&BackendRequest::Submit {
            conversation: ConversationId(1),
            generation: 2,
            op: OperationId(3),
            request: RequestId("request".into()),
            text: "task".into(),
            attachments: vec![],
            model: None,
        });
        assert!(control.request(&stop(3)));
        assert!(
            incoming.recv_timeout(Duration::from_millis(50)).is_err(),
            "Stop cannot overtake the unsent prompt"
        );
        control.submitted(ConversationId(1), 2, OperationId(3));
        let frame = incoming.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            frame.call,
            Call::Stop {
                session: "session".into()
            }
        );
        peer.send(ServerFrame::Reply {
            id: frame.id,
            result: Ok(Reply::Stopping { run: Some(1) }),
        })
        .unwrap();
        control.close();
        lock(&control.thread).take().unwrap().join().unwrap();
        assert!(failures.try_recv().is_err());
        client.close();
        drop(peer);
        assert!(client.join_reader(Duration::from_secs(1)));
    }

    #[test]
    fn queued_stop_cannot_cancel_a_later_local_prompt() {
        let (link, incoming, peer) = link();
        let client = Arc::new(Client::start(link, |_| {}));
        let (feedback, _) = mpsc::channel();
        let control = Control::new(feedback, Duration::from_secs(2));
        bind(&control, &client, "session");
        {
            let mut binding = lock(&control.binding);
            binding
                .sessions
                .insert(ConversationId(2), "sentinel".into());
            binding.orders.insert(ConversationId(2), Arc::default());
        }
        control.request(&stop(1));
        let first = incoming.recv_timeout(Duration::from_secs(1)).unwrap();
        control.request(&stop(2)); // Waits behind the unanswered Stop.
        control.submitting(&BackendRequest::Submit {
            conversation: ConversationId(1),
            generation: 2,
            op: OperationId(3),
            request: RequestId("new".into()),
            text: "new prompt".into(),
            attachments: vec![],
            model: None,
        });
        peer.send(ServerFrame::Reply {
            id: first.id,
            result: Ok(Reply::Stopping { run: None }),
        })
        .unwrap();
        control.submitted(ConversationId(1), 2, OperationId(3));
        control.request(&BackendRequest::Cancel {
            conversation: ConversationId(2),
            generation: 2,
            op: OperationId(4),
        });
        let current = incoming.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(
            current.call,
            Call::Stop {
                session: "sentinel".into()
            },
            "the superseded Stop must be skipped before the next queued control call"
        );
        peer.send(ServerFrame::Reply {
            id: current.id,
            result: Ok(Reply::Stopping { run: Some(3) }),
        })
        .unwrap();
        control.close();
        lock(&control.thread).take().unwrap().join().unwrap();
        assert!(
            incoming.try_recv().is_err(),
            "the superseded Stop must not reach the wire"
        );
        client.close();
        drop(peer);
        assert!(client.join_reader(Duration::from_secs(1)));
    }

    #[test]
    fn stop_queue_is_bounded_and_reconnect_does_not_retarget_old_requests() {
        let (link1, incoming1, peer1) = link();
        let client1 = Arc::new(Client::start(link1, |_| {}));
        let (feedback, failures) = mpsc::channel();
        let control = Control::new(feedback, Duration::from_secs(5));
        bind(&control, &client1, "old");
        control.request(&stop(0));
        incoming1.recv_timeout(Duration::from_secs(1)).unwrap(); // Hold this reply.
        for op in 1..=CAPACITY {
            control.request(&stop(op as u64));
        }
        control.request(&stop(99));
        assert!(
            matches!(failures.recv_timeout(Duration::from_secs(1)).unwrap(), Msg::StopFailed { op: OperationId(99), message, .. } if message.contains("Too many"))
        );
        let (link2, incoming2, peer2) = link();
        let client2 = Arc::new(Client::start(link2, |_| {}));
        bind(&control, &client2, "replacement");
        client1.close();
        // Every queued request captured client1. Even while client2 is bound, all nine fail
        // on their original connection and none is sent to the replacement.
        for _ in 0..=CAPACITY {
            assert!(matches!(
                failures.recv_timeout(Duration::from_secs(1)).unwrap(),
                Msg::StopFailed { .. }
            ));
        }
        assert!(incoming2.try_recv().is_err());
        control.request(&stop(100));
        let frame = incoming2.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(
            frame.call,
            Call::Stop {
                session: "replacement".into()
            }
        );
        peer2
            .send(ServerFrame::Reply {
                id: frame.id,
                result: Ok(Reply::Stopping { run: None }),
            })
            .unwrap();
        control.close();
        lock(&control.thread).take().unwrap().join().unwrap();
        client2.close();
        drop((peer1, peer2));
        assert!(client1.join_reader(Duration::from_secs(1)));
        assert!(client2.join_reader(Duration::from_secs(1)));
    }
}
