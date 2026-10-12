//! Bounded per-client delivery. Presentation coalesces; replies have reserved, backpressured
//! capacity and are never discarded. No publisher waits on a slow reader.
use pipkin_engine_protocol::{Event, ServerFrame, delivery::EventQueue};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Default)]
struct State {
    events: EventQueue,
    replies: VecDeque<(ServerFrame, OwnedSemaphorePermit, u64)>,
    seq: u64,
    closed: bool,
}
struct Shared {
    state: Mutex<State>,
    ready: Condvar,
    replies: Arc<Semaphore>,
}
#[derive(Clone)]
pub(crate) struct Delivery(Arc<Shared>);
pub struct Receiver(Arc<Shared>);

pub(crate) fn channel() -> (Delivery, Receiver) {
    let shared = Arc::new(Shared {
        state: Mutex::new(State::default()),
        ready: Condvar::new(),
        replies: Arc::new(Semaphore::new(16)),
    });
    (Delivery(shared.clone()), Receiver(shared))
}
impl Delivery {
    pub fn event(&self, epoch: u64, event: Event) -> bool {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed {
            return false;
        }
        state.seq += 1;
        let seq = state.seq;
        state.events.push(epoch, seq, event);
        self.0.ready.notify_one();
        true
    }
    pub async fn reply(&self, frame: ServerFrame) {
        let Ok(permit) = self.0.replies.clone().acquire_owned().await else {
            return;
        };
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed {
            return;
        }
        let barrier = state.seq;
        state.replies.push_back((frame, permit, barrier));
        self.0.ready.notify_one();
    }
    pub fn close(&self) {
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .closed = true;
        self.0.replies.close();
        self.0.ready.notify_all();
    }
}
impl Receiver {
    fn take(state: &mut State) -> Option<ServerFrame> {
        // Preserve events already published when the reply was queued, but never wait for
        // later progress. The prefix is bounded; continuous producers cannot starve a reply.
        if let Some((_, _, barrier)) = state.replies.front()
            && state.events.first_seq().is_none_or(|seq| seq > *barrier)
        {
            return state.replies.pop_front().map(|(frame, _, _)| frame);
        }
        state
            .events
            .pop()
            .or_else(|| state.replies.pop_front().map(|(frame, _, _)| frame))
    }

    pub fn try_recv(&self) -> Result<ServerFrame, mpsc::TryRecvError> {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        Self::take(&mut state).ok_or(if state.closed {
            mpsc::TryRecvError::Disconnected
        } else {
            mpsc::TryRecvError::Empty
        })
    }
    pub fn recv(&self) -> Result<ServerFrame, mpsc::RecvError> {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(frame) = Self::take(&mut state) {
                return Ok(frame);
            }
            if state.closed {
                return Err(mpsc::RecvError);
            }
            state = self.0.ready.wait(state).unwrap_or_else(|e| e.into_inner());
        }
    }
    pub fn recv_timeout(&self, timeout: Duration) -> Result<ServerFrame, mpsc::RecvTimeoutError> {
        let deadline = Instant::now() + timeout;
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(frame) = Self::take(&mut state) {
                return Ok(frame);
            }
            if state.closed {
                return Err(mpsc::RecvTimeoutError::Disconnected);
            }
            let wait = deadline.saturating_duration_since(Instant::now());
            if wait.is_zero() {
                return Err(mpsc::RecvTimeoutError::Timeout);
            }
            state = self
                .0
                .ready
                .wait_timeout(state, wait)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}
impl Drop for Receiver {
    fn drop(&mut self) {
        Delivery(self.0.clone()).close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipkin_engine_protocol::Reply;

    #[tokio::test]
    async fn replies_keep_the_published_prefix_and_bypass_future_progress() {
        let (tx, rx) = channel();
        tx.event(
            1,
            Event::Notice {
                message: "storage failed".into(),
            },
        );
        tx.reply(ServerFrame::Reply {
            id: 7,
            result: Ok(Reply::Ok),
        })
        .await;
        tx.event(1, Event::ModelsChanged);
        assert!(matches!(
            rx.try_recv(),
            Ok(ServerFrame::Event {
                event: Event::Notice { .. },
                ..
            })
        ));
        assert!(matches!(
            rx.try_recv(),
            Ok(ServerFrame::Reply { id: 7, .. })
        ));
        assert!(matches!(
            rx.try_recv(),
            Ok(ServerFrame::Event {
                event: Event::ModelsChanged,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn reply_capacity_backpressures_and_a_dropped_reader_releases_waiters() {
        let (tx, rx) = channel();
        for id in 0..16 {
            tx.reply(ServerFrame::Reply {
                id,
                result: Ok(Reply::Ok),
            })
            .await;
        }
        let pending = tokio::spawn({
            let tx = tx.clone();
            async move {
                tx.reply(ServerFrame::Reply {
                    id: 16,
                    result: Ok(Reply::Ok),
                })
                .await
            }
        });
        tokio::task::yield_now().await;
        assert!(
            !pending.is_finished(),
            "the seventeenth reply must wait for capacity"
        );
        drop(rx);
        tokio::time::timeout(Duration::from_secs(1), pending)
            .await
            .unwrap()
            .unwrap();
        assert!(!tx.event(1, Event::ModelsChanged));
    }
}
