//! Desktop event admission. Permits cover queued events AND the batch being applied by GPUI.
//! A single indivisible event larger than the byte budget is admitted exclusively, preserving
//! full history/tool results. This bounds backlog, not the size of one event or core state.
use std::sync::{Arc, Condvar, Mutex};

use pipkin_core::BackendEvent;

#[cfg(test)]
mod approval_tests;
mod size;

pub const NATIVE_EVENTS: usize = 128;
pub const NATIVE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Default)]
struct Usage {
    count: usize,
    bytes: usize,
    closed: bool,
    #[cfg(test)]
    waiting: usize,
}

pub(crate) struct Budget {
    count: usize,
    bytes: usize,
    usage: Mutex<Usage>,
    available: Condvar,
}

impl Budget {
    fn close(&self) {
        self.usage.lock().unwrap().closed = true;
        self.available.notify_all();
    }

    fn acquire(self: &Arc<Self>, bytes: usize) -> Result<Permit, ()> {
        let mut usage = self.usage.lock().unwrap();
        while !usage.closed
            && usage.count != 0
            && (usage.count >= self.count || bytes > self.bytes.saturating_sub(usage.bytes))
        {
            #[cfg(test)]
            {
                usage.waiting += 1;
            }
            usage = self.available.wait(usage).unwrap();
            #[cfg(test)]
            {
                usage.waiting -= 1;
            }
        }
        if usage.closed {
            return Err(());
        }
        usage.count += 1;
        usage.bytes += bytes;
        Ok(Permit {
            budget: self.clone(),
            bytes,
        })
    }
}

struct Permit {
    budget: Arc<Budget>,
    bytes: usize,
}
impl Drop for Permit {
    fn drop(&mut self) {
        let mut usage = self.budget.usage.lock().unwrap();
        usage.count -= 1;
        usage.bytes -= self.bytes;
        self.budget.available.notify_all();
    }
}

pub struct Lease {
    event: Option<BackendEvent>,
    _permit: Option<Permit>,
}
impl Lease {
    #[cfg(test)]
    pub fn bytes(&self) -> usize {
        size::event_bytes(self.event.as_ref().unwrap())
    }

    /// Retain this lease until the moved event has been applied or discarded.
    pub fn take(&mut self) -> BackendEvent {
        self.event.take().expect("event taken once")
    }
}

#[derive(Clone)]
pub enum EventSender {
    Plain(async_channel::Sender<BackendEvent>),
    Budgeted {
        tx: async_channel::Sender<Lease>,
        budget: Arc<Budget>,
    },
}
impl From<async_channel::Sender<BackendEvent>> for EventSender {
    fn from(tx: async_channel::Sender<BackendEvent>) -> Self {
        Self::Plain(tx)
    }
}
impl EventSender {
    pub fn send_blocking(&self, event: BackendEvent) -> Result<(), ()> {
        match self {
            Self::Plain(tx) => tx.send_blocking(event).map_err(|_| ()),
            Self::Budgeted { tx, budget } => {
                let permit = budget.acquire(size::event_bytes(&event))?;
                tx.send_blocking(Lease {
                    event: Some(event),
                    _permit: Some(permit),
                })
                .map_err(|_| ())
            }
        }
    }
    #[cfg(test)]
    pub fn stats(&self) -> (usize, usize, usize) {
        match self {
            Self::Budgeted { budget, .. } => {
                let u = budget.usage.lock().unwrap();
                (u.count, u.bytes, u.waiting)
            }
            Self::Plain(_) => panic!("not budgeted"),
        }
    }
    pub fn close(&self) {
        match self {
            Self::Plain(tx) => {
                tx.close();
            }
            Self::Budgeted { tx, budget } => {
                budget.close();
                tx.close();
            }
        }
    }
}

pub enum EventReceiver {
    Plain(async_channel::Receiver<BackendEvent>),
    Budgeted {
        rx: async_channel::Receiver<Lease>,
        budget: Arc<Budget>,
    },
}
impl EventReceiver {
    pub async fn recv(&self) -> Result<Lease, async_channel::RecvError> {
        match self {
            Self::Plain(rx) => rx.recv().await.map(|event| Lease {
                event: Some(event),
                _permit: None,
            }),
            Self::Budgeted { rx, .. } => rx.recv().await,
        }
    }
    pub fn try_recv(&self) -> Result<Lease, async_channel::TryRecvError> {
        match self {
            Self::Plain(rx) => rx.try_recv().map(|event| Lease {
                event: Some(event),
                _permit: None,
            }),
            Self::Budgeted { rx, .. } => rx.try_recv(),
        }
    }
}
impl Drop for EventReceiver {
    fn drop(&mut self) {
        if let Self::Budgeted { rx, budget } = self {
            budget.close();
            rx.close();
            while rx.try_recv().is_ok() {}
        }
    }
}

pub fn bounded(count: usize, bytes: usize) -> (EventSender, EventReceiver) {
    assert!(count > 0 && bytes > 0);
    let budget = Arc::new(Budget {
        count,
        bytes,
        usage: Mutex::new(Usage::default()),
        available: Condvar::new(),
    });
    let (tx, rx) = async_channel::bounded(count);
    (
        EventSender::Budgeted {
            tx,
            budget: budget.clone(),
        },
        EventReceiver::Budgeted { rx, budget },
    )
}

pub fn plain(count: usize) -> (EventSender, EventReceiver) {
    let (tx, rx) = async_channel::bounded(count);
    (tx.into(), EventReceiver::Plain(rx))
}

#[cfg(test)]
mod tests;
