//! Bounded live presentation queue, shared by the engine and desktop. On pressure all queued
//! presentation is replaced by one resync marker. Replies never pass through this queue.
use crate::{Event, ServerFrame};
use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::{Arc, OnceLock};

pub const MAX_EVENTS: usize = 128;
pub const MAX_EVENT_BYTES: usize = 2 * 1024 * 1024;

/// One immutable event and its JSON, shared across subscriber queues. Sequence numbers belong
/// to each connection and are added only when writing the frame envelope.
#[derive(Clone)]
pub struct SharedEvent(Arc<Payload>);
struct Payload {
    event: Event,
    json: Vec<u8>,
}

impl From<Event> for SharedEvent {
    fn from(event: Event) -> Self {
        if matches!(event, Event::ResyncRequired) {
            return Self::resync();
        }
        // Stop serialization at the queue budget, rather than allocating an arbitrarily large
        // second copy just to discover that this event must be replaced with a resync marker.
        struct Bounded(Vec<u8>);
        impl Write for Bounded {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if bytes.len() > MAX_EVENT_BYTES.saturating_sub(self.0.len()) {
                    return Err(io::Error::other("event exceeds presentation budget"));
                }
                self.0.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut encoded = Bounded(Vec::new());
        if serde_json::to_writer(&mut encoded, &event).is_err() {
            return Self::resync();
        }
        Self(Arc::new(Payload {
            event,
            json: encoded.0,
        }))
    }
}
impl SharedEvent {
    fn resync() -> Self {
        static RESYNC: OnceLock<Arc<Payload>> = OnceLock::new();
        Self(
            RESYNC
                .get_or_init(|| {
                    Arc::new(Payload {
                        event: Event::ResyncRequired,
                        json: serde_json::to_vec(&Event::ResyncRequired)
                            .expect("resync event encodes"),
                    })
                })
                .clone(),
        )
    }
}

/// A connection's envelope referring to the shared payload. Network hosts write this directly;
/// in-process consumers can recover the typed frame without a JSON parse.
pub struct QueuedEvent {
    epoch: u64,
    seq: u64,
    payload: SharedEvent,
}
impl QueuedEvent {
    pub fn into_frame(self) -> ServerFrame {
        let event = match Arc::try_unwrap(self.payload.0) {
            Ok(payload) => payload.event,
            Err(payload) => payload.event.clone(),
        };
        ServerFrame::Event {
            epoch: self.epoch,
            seq: self.seq,
            event,
        }
    }

    /// Write the existing frame contract without cloning or reserializing its event body.
    pub fn write(&self, out: &mut impl Write) -> Result<(), crate::frame::FrameError> {
        use crate::frame::FrameError;
        let prefix = format!(
            "{{\"frame\":\"event\",\"epoch\":{},\"seq\":{},\"event\":",
            self.epoch, self.seq
        );
        let len = prefix.len() + self.payload.0.json.len() + 1;
        if len > crate::limits::MAX_FRAME_BYTES {
            return Err(FrameError::TooLarge(len));
        }
        out.write_all(&(len as u32).to_be_bytes())
            .and_then(|()| out.write_all(prefix.as_bytes()))
            .and_then(|()| out.write_all(&self.payload.0.json))
            .and_then(|()| out.write_all(b"}"))
            .and_then(|()| out.flush())
            .map_err(FrameError::Io)
    }
}

#[derive(Default)]
pub struct EventQueue {
    frames: VecDeque<QueuedEvent>,
    bytes: usize,
    resync: bool,
}

impl EventQueue {
    pub fn push(&mut self, epoch: u64, seq: u64, event: Event) {
        self.push_shared(epoch, seq, event.into());
    }

    pub fn push_shared(&mut self, epoch: u64, seq: u64, event: SharedEvent) {
        if self.resync
            || self.frames.len() >= MAX_EVENTS
            || self.bytes.saturating_add(event.0.json.len()) > MAX_EVENT_BYTES
            || matches!(event.0.event, Event::ResyncRequired)
        {
            self.frames.clear();
            self.bytes = 0;
            self.resync = true;
            self.frames.push_back(QueuedEvent {
                epoch,
                seq,
                payload: SharedEvent::resync(),
            });
        } else {
            self.bytes += event.0.json.len();
            self.frames.push_back(QueuedEvent {
                epoch,
                seq,
                payload: event,
            });
        }
    }

    pub fn pop_shared(&mut self) -> Option<QueuedEvent> {
        let frame = self.frames.pop_front()?;
        if !self.resync {
            self.bytes -= frame.payload.0.json.len();
        }
        if self.frames.is_empty() {
            self.resync = false;
        }
        Some(frame)
    }

    pub fn pop(&mut self) -> Option<ServerFrame> {
        self.pop_shared().map(QueuedEvent::into_frame)
    }

    pub fn first_seq(&self) -> Option<u64> {
        Some(self.frames.front()?.seq)
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn subscribers_share_one_payload_but_keep_independent_envelopes_and_pressure() {
        let payload = SharedEvent::from(Event::Notice {
            message: "a moderately large event 🦀\n".repeat(4096),
        });
        let mut queues: Vec<_> = (0..32).map(|_| EventQueue::default()).collect();
        for (i, queue) in queues.iter_mut().enumerate() {
            queue.push_shared(3, i as u64 + 1, payload.clone());
            assert!(Arc::ptr_eq(
                &queue.frames.front().unwrap().payload.0,
                &payload.0
            ));
            assert_eq!(queue.bytes, payload.0.json.len());
        }
        assert_eq!(Arc::strong_count(&payload.0), 33);
        // Only the unread subscriber overflows; the others retain their original sequence.
        for seq in 100..100 + MAX_EVENTS as u64 {
            queues[0].push(3, seq, Event::ModelsChanged);
        }
        assert_eq!(Arc::strong_count(&payload.0), 32);
        assert_eq!(queues[0].frames.len(), 1);
        assert!(matches!(
            queues[0].pop(),
            Some(ServerFrame::Event {
                event: Event::ResyncRequired,
                ..
            })
        ));
        for (i, queue) in queues.iter_mut().enumerate().skip(1) {
            let frame = queue.pop_shared().unwrap();
            let mut bytes = Vec::new();
            frame.write(&mut bytes).unwrap();
            let expected = ServerFrame::Event {
                epoch: 3,
                seq: i as u64 + 1,
                event: payload.0.event.clone(),
            };
            assert_eq!(bytes, crate::frame::encode(&expected).unwrap());
            assert_eq!(
                crate::frame::read::<ServerFrame>(&mut bytes.as_slice()).unwrap(),
                expected
            );
            assert_eq!(queue.bytes, 0);
        }
        assert_eq!(Arc::strong_count(&payload.0), 1);
    }

    #[test]
    fn shared_envelopes_preserve_escaping_extreme_ids_and_write_errors() {
        for event in [
            Event::ResyncRequired,
            Event::SessionsChanged,
            Event::ModelsChanged,
            Event::Notice {
                message: "quoted \" \u{0} \n \r \t 🦀".into(),
            },
        ] {
            let expected = ServerFrame::Event {
                epoch: u64::MAX,
                seq: u64::MAX,
                event: event.clone(),
            };
            let frame = QueuedEvent {
                epoch: u64::MAX,
                seq: u64::MAX,
                payload: event.into(),
            };
            let mut bytes = Vec::new();
            frame.write(&mut bytes).unwrap();
            assert_eq!(bytes, crate::frame::encode(&expected).unwrap());
            assert!(matches!(
                frame.write(&mut &mut [0u8; 2][..]),
                Err(crate::frame::FrameError::Io(_))
            ));
            assert_eq!(frame.into_frame(), expected);
        }
    }

    #[test]
    fn escaped_payload_size_is_bounded_and_oversize_does_not_retain_the_event() {
        let payload = SharedEvent::from(Event::Notice {
            message: "\u{0}".repeat(MAX_EVENT_BYTES / 2),
        });
        assert!(matches!(payload.0.event, Event::ResyncRequired));
        assert!(payload.0.json.len() < 100);
        let mut q = EventQueue::default();
        q.push_shared(1, 1, payload);
        q.pop().unwrap();
        q.push(1, 2, Event::ModelsChanged);
        assert!(matches!(
            q.pop(),
            Some(ServerFrame::Event {
                event: Event::ModelsChanged,
                seq: 2,
                ..
            })
        ));
        assert_eq!(q.bytes, 0);
    }

    #[test]
    fn pressure_coalesces_to_one_marker_with_the_latest_sequence() {
        let mut q = EventQueue::default();
        for seq in 1..=10000 {
            q.push(2, seq, Event::SessionsChanged);
        }
        assert_eq!(
            q.pop(),
            Some(ServerFrame::Event {
                epoch: 2,
                seq: 10000,
                event: Event::ResyncRequired
            })
        );
        assert!(q.pop().is_none());
        q.push(2, 10001, Event::ModelsChanged);
        assert!(matches!(
            q.pop(),
            Some(ServerFrame::Event {
                event: Event::ModelsChanged,
                ..
            })
        ));
    }
    #[test]
    fn one_oversized_event_uses_bounded_recovery() {
        let mut q = EventQueue::default();
        q.push(
            1,
            1,
            Event::Notice {
                message: "x".repeat(MAX_EVENT_BYTES + 1),
            },
        );
        assert!(matches!(
            q.pop(),
            Some(ServerFrame::Event {
                event: Event::ResyncRequired,
                ..
            })
        ));
    }
}
