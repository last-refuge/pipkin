//! Bounded provider progress without an intermediate channel. The engine's sink applies
//! backpressure at commit boundaries; a provider cannot enqueue faster than storage drains.
use std::sync::{Arc, Mutex};
use std::time::Instant;

use pipkin_engine_protocol::Delta;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use super::{FLUSH_BYTES, FLUSH_INTERVAL, Shared};
use crate::provider::{Model, StreamEvent, StreamSink};
use crate::runtime::session::SessionRt;

const MAX_DELTAS: usize = 128;

#[derive(Default)]
struct Pending {
    deltas: Vec<Delta>,
    bytes: usize,
    oldest: Option<Instant>,
    partial: Option<u64>,
    closed: bool,
}

pub(super) struct Progress {
    shared: Arc<Shared>,
    session: Arc<SessionRt>,
    run: u64,
    model: Model,
    cancel: CancellationToken,
    state: Mutex<Pending>,
    pub changed: Notify,
}
impl Progress {
    pub fn new(
        shared: Arc<Shared>,
        session: Arc<SessionRt>,
        run: u64,
        model: Model,
        cancel: CancellationToken,
    ) -> Arc<Self> {
        Arc::new(Self {
            shared,
            session,
            run,
            model,
            cancel,
            state: Mutex::new(Pending::default()),
            changed: Notify::new(),
        })
    }

    pub fn sink(self: &Arc<Self>) -> StreamSink {
        // A retained provider callback must not keep a finished runtime/session alive.
        let weak = Arc::downgrade(self);
        StreamSink::new(move |StreamEvent::Delta(delta)| {
            if let Some(progress) = weak.upgrade() {
                progress.push(delta);
            }
        })
    }

    fn push(&self, delta: Delta) {
        match delta {
            Delta::Text { block, text } => self.text(block, text, false),
            Delta::Thinking { block, text } => self.text(block, text, true),
            delta @ Delta::ToolCall { .. } => self.one(delta),
        }
    }

    fn text(&self, block: u32, text: String, thinking: bool) {
        if text.len() <= FLUSH_BYTES {
            self.one(if thinking {
                Delta::Thinking { block, text }
            } else {
                Delta::Text { block, text }
            });
            return;
        }
        let mut rest = text.as_str();
        while !rest.is_empty() && !self.cancel.is_cancelled() {
            let mut end = rest.len().min(FLUSH_BYTES);
            while !rest.is_char_boundary(end) {
                end -= 1;
            }
            let chunk = rest[..end].to_owned();
            self.one(if thinking {
                Delta::Thinking { block, text: chunk }
            } else {
                Delta::Text { block, text: chunk }
            });
            rest = &rest[end..];
        }
    }

    fn one(&self, delta: Delta) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed || self.cancel.is_cancelled() {
            return;
        }
        let bytes = match &delta {
            Delta::Text { text, .. } | Delta::Thinking { text, .. } => text.len(),
            Delta::ToolCall { id, name, .. } => id.len().saturating_add(name.len()),
        };
        if state.deltas.len() >= MAX_DELTAS
            || state.bytes.saturating_add(bytes) > FLUSH_BYTES
            || state.oldest.is_some_and(|t| t.elapsed() >= FLUSH_INTERVAL)
        {
            self.commit(&mut state);
        }
        if self.cancel.is_cancelled() {
            return;
        }
        if state.deltas.is_empty() {
            state.oldest = Some(Instant::now());
            self.changed.notify_one();
        }
        state.bytes = state.bytes.saturating_add(bytes);
        state.deltas.push(delta);
        // An indivisible tool identity exceeding the batching target is committed immediately,
        // never truncated or left in the pending backlog. Text is split at UTF-8 boundaries.
        if state.bytes >= FLUSH_BYTES || state.deltas.len() >= MAX_DELTAS {
            self.commit(&mut state);
        }
    }

    fn commit(&self, state: &mut Pending) {
        if !state.deltas.is_empty()
            && self
                .shared
                .flush(
                    &self.session,
                    self.run,
                    &self.model,
                    &mut state.partial,
                    &mut state.deltas,
                )
                .is_err()
        {
            self.cancel.cancel();
        }
        state.bytes = 0;
        state.oldest = None;
    }

    pub fn wait_time(&self) -> Option<std::time::Duration> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .oldest
            .map(|t| FLUSH_INTERVAL.saturating_sub(t.elapsed()))
    }

    pub fn flush(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.closed {
            self.commit(&mut state);
        }
    }

    /// The final message supersedes uncommitted progress. Close before publishing it so a
    /// background or stale provider callback cannot mutate the finalized entry.
    pub fn finish(&self) -> Option<u64> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.closed = true;
        state.deltas.clear();
        state.bytes = 0;
        state.oldest = None;
        state.partial
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::scripted::{self, Scripted};
    use crate::runtime::tests::{Engine, Fixture};
    use crate::store::Store;
    use pipkin_engine_protocol::{Block, EntryKind, Message};
    use std::time::Duration;

    fn fixture() -> (Fixture, Engine, Arc<Progress>) {
        let fx = Fixture::new();
        let mut engine = fx.start(&Scripted::echo("unused"));
        let session = engine.create(&fx.project, "c");
        let shared = engine.handle.shared();
        let run = shared
            .store
            .tx(|tx| Store::start_run(tx, &session, None, 0))
            .unwrap();
        let progress = Progress::new(
            shared.clone(),
            shared.session(&session).unwrap(),
            run,
            scripted::model(),
            CancellationToken::new(),
        );
        (fx, engine, progress)
    }
    fn on_runtime<T: Send + 'static>(
        progress: &Arc<Progress>,
        f: impl FnOnce(Arc<Progress>) -> T + Send + 'static,
    ) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        let p = progress.clone();
        progress.shared.spawn(async move {
            let _ = tx.send(f(p));
        });
        rx.recv_timeout(Duration::from_secs(10)).unwrap()
    }
    fn text(progress: &Progress) -> String {
        let id = progress.state.lock().unwrap().partial.unwrap();
        let entry = progress
            .shared
            .store
            .entry(&progress.session.id, id)
            .unwrap()
            .unwrap();
        let EntryKind::Message { message, .. } = entry.kind else {
            panic!()
        };
        message.text()
    }

    #[test]
    fn synchronous_bursts_are_bounded_and_unicode_and_tiny_deltas_remain_lossless() {
        let (_fx, _engine, progress) = fixture();
        let big = "x🦀\n".repeat(40_000);
        let expected = format!("{big}{}", "z".repeat(5000));
        on_runtime(&progress, move |p| {
            p.push(Delta::Text {
                block: 0,
                text: big,
            });
            assert!(
                p.state.lock().unwrap().partial.is_some(),
                "burst commits before producer returns"
            );
            for _ in 0..5000 {
                p.push(Delta::Text {
                    block: 0,
                    text: "z".into(),
                });
                let s = p.state.lock().unwrap();
                assert!(s.bytes < FLUSH_BYTES && s.deltas.len() < MAX_DELTAS);
            }
            p.flush();
        });
        assert_eq!(text(&progress), expected);
    }

    #[test]
    fn indivisible_tool_identity_is_committed_without_truncation_or_pending_backlog() {
        let (_fx, _engine, progress) = fixture();
        let id = "x".repeat(FLUSH_BYTES * 2);
        let expected = id.clone();
        on_runtime(&progress, move |p| {
            p.push(Delta::ToolCall {
                block: 0,
                id,
                name: "read".into(),
            })
        });
        let s = progress.state.lock().unwrap();
        assert_eq!(s.bytes, 0);
        assert!(s.deltas.is_empty());
        let entry = progress
            .shared
            .store
            .entry(&progress.session.id, s.partial.unwrap())
            .unwrap()
            .unwrap();
        let EntryKind::Message {
            message: Message::Assistant { content, .. },
            ..
        } = entry.kind
        else {
            panic!()
        };
        assert!(matches!(&content[0], Block::ToolCall { id, .. } if id == &expected));
    }

    #[test]
    fn closing_or_dropping_progress_rejects_late_provider_callbacks() {
        let (_fx, _engine, progress) = fixture();
        let sink = progress.sink();
        on_runtime(&progress, |p| {
            p.push(Delta::Text {
                block: 0,
                text: "saved".into(),
            });
            p.flush();
            p.push(Delta::Text {
                block: 0,
                text: "uncommitted".into(),
            });
            p.finish();
        });
        sink.send(StreamEvent::Delta(Delta::Text {
            block: 0,
            text: "late".repeat(FLUSH_BYTES),
        }));
        assert_eq!(text(&progress), "saved");
        assert!(progress.state.lock().unwrap().deltas.is_empty());
        let weak = Arc::downgrade(&progress);
        drop(progress);
        assert!(
            weak.upgrade().is_none(),
            "sink must not own progress or the runtime"
        );
        sink.send(StreamEvent::Delta(Delta::Text {
            block: 0,
            text: "gone".into(),
        }));
    }

    #[test]
    fn checkpoint_failure_cancels_the_producer_and_stops_further_progress() {
        let (_fx, _engine, progress) = fixture();
        progress.shared.store.fail_next_write();
        on_runtime(&progress, |p| {
            p.push(Delta::Text {
                block: 0,
                text: "x".repeat(FLUSH_BYTES * 3),
            });
            assert!(p.cancel.is_cancelled());
            assert!(p.session.failure().is_some());
            let s = p.state.lock().unwrap();
            assert!(s.partial.is_none() && s.deltas.is_empty());
            assert_eq!(s.bytes, 0);
        });
    }
}
