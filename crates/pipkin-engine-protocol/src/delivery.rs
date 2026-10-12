//! Bounded live presentation queue, shared by the engine and desktop. On pressure all queued
//! presentation is replaced by one resync marker. Replies never pass through this queue.
use crate::{Event, ServerFrame};
use std::collections::VecDeque;

pub const MAX_EVENTS: usize = 128;
pub const MAX_EVENT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Default)]
pub struct EventQueue {
    frames: VecDeque<(ServerFrame, usize)>,
    bytes: usize,
    resync: bool,
}

impl EventQueue {
    pub fn push(&mut self, epoch: u64, seq: u64, event: Event) {
        // Count serialized bytes without allocating a second copy of a potentially huge entry.
        struct Counter(usize);
        impl std::io::Write for Counter {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0 = self.0.saturating_add(b.len());
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut count = Counter(0);
        let _ = serde_json::to_writer(&mut count, &event);
        if self.resync
            || self.frames.len() >= MAX_EVENTS
            || self.bytes.saturating_add(count.0) > MAX_EVENT_BYTES
            || matches!(event, Event::ResyncRequired)
        {
            self.frames.clear();
            self.bytes = 0;
            self.resync = true;
            self.frames.push_back((
                ServerFrame::Event {
                    epoch,
                    seq,
                    event: Event::ResyncRequired,
                },
                0,
            ));
        } else {
            self.bytes += count.0;
            self.frames
                .push_back((ServerFrame::Event { epoch, seq, event }, count.0));
        }
    }

    pub fn pop(&mut self) -> Option<ServerFrame> {
        let (frame, bytes) = self.frames.pop_front()?;
        self.bytes -= bytes;
        if self.frames.is_empty() {
            self.resync = false;
        }
        Some(frame)
    }

    pub fn first_seq(&self) -> Option<u64> {
        match &self.frames.front()?.0 {
            ServerFrame::Event { seq, .. } => Some(*seq),
            _ => unreachable!("only events enter the presentation queue"),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
