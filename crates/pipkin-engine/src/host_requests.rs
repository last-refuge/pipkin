//! Connection-local RPC cancellation. Register before enqueueing so control traffic can
//! cancel a normal frame still waiting in its lane. Queue/task limits also bound this map.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use pipkin_engine_protocol::{ClientFrame, Lane};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub(crate) struct Requests(Mutex<HashMap<u64, CancellationToken>>);

impl Requests {
    pub(crate) fn send(
        &self,
        normal: &Sender<ClientFrame>,
        control: &Sender<ClientFrame>,
        frame: ClientFrame,
    ) -> Result<(), String> {
        let mut pending = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if pending.contains_key(&frame.id) {
            return Err("the request id is already pending on this connection".into());
        }
        let lane = if frame.call.lane() == Lane::Control {
            control
        } else {
            normal
        };
        let id = frame.id;
        // Hold the lock through enqueue: the receiver cannot miss this registration.
        pending.insert(id, CancellationToken::new());
        if lane.try_send(frame).is_err() {
            pending.remove(&id);
            return Err("the engine is stopped or its input queue is full".into());
        }
        Ok(())
    }

    pub(crate) fn guard(self: &Arc<Self>, id: u64) -> Request {
        let cancel = self.0.lock().unwrap_or_else(|e| e.into_inner())[&id].clone();
        Request {
            requests: self.clone(),
            id,
            cancel,
        }
    }

    pub(crate) fn cancel(&self, id: u64) {
        if let Some(token) = self.0.lock().unwrap_or_else(|e| e.into_inner()).get(&id) {
            token.cancel();
        }
    }
}

pub(crate) struct Request {
    requests: Arc<Requests>,
    id: u64,
    pub(crate) cancel: CancellationToken,
}

impl Drop for Request {
    fn drop(&mut self) {
        self.requests
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipkin_engine_protocol::Call;

    #[test]
    fn refused_frames_and_dropped_requests_do_not_leak_cancellation_state() {
        let requests = Arc::new(Requests::default());
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let (control, _controls) = tokio::sync::mpsc::channel(1);
        let frame = |id| ClientFrame {
            id,
            lane: Lane::Normal,
            call: Call::ListSessions,
        };
        requests.send(&tx, &control, frame(1)).unwrap();
        assert!(requests.send(&tx, &control, frame(1)).is_err());
        assert!(requests.send(&tx, &control, frame(2)).is_err());
        assert_eq!(requests.0.lock().unwrap().len(), 1);
        rx.try_recv().unwrap();
        let guard = requests.guard(1);
        requests.cancel(1);
        assert!(guard.cancel.is_cancelled());
        drop(guard);
        assert!(requests.0.lock().unwrap().is_empty());
        requests.send(&tx, &control, frame(2)).unwrap();
        assert!(!requests.guard(2).cancel.is_cancelled());
    }
}
